use super::*;
use crate::{
    agent::AgentError,
    workspace::{WorkspaceError as E, WorkspaceResult as R},
};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

pub struct SessionHandle {
    pub identity: SessionIdentity,
    pub(crate) metadata: Mutex<(SessionMetadata, String)>,
    pub(crate) runtime: Option<Arc<SessionRuntime>>,
    pub(crate) phase: Mutex<SessionPhase>,
    pub(crate) _lease: Mutex<Option<Arc<SessionLease>>>,
    pub(crate) repo: Arc<SessionRepository>,
    pub(crate) cancel: Mutex<Option<(String, tokio_util::sync::CancellationToken)>>,
}
impl SessionHandle {
    pub fn phase(&self) -> SessionPhase {
        *self.phase.lock().unwrap()
    }
    pub fn metadata(&self) -> (SessionMetadata, String) {
        self.metadata.lock().unwrap().clone()
    }
    pub fn runtime(&self) -> R<Arc<SessionRuntime>> {
        if self.phase() == SessionPhase::Closed {
            return Err(E::SessionUnavailable("session handle is closed".into()));
        }
        self.runtime
            .clone()
            .ok_or_else(|| E::SessionUnavailable("session is read-only".into()))
    }
    pub fn cancel(&self, run_id: Option<&str>) -> R<()> {
        if let Some((id, token)) = self.cancel.lock().unwrap().as_ref() {
            if run_id.is_some_and(|expected| id != expected) {
                return Err(E::NotFound("run ID does not match active run".into()));
            }
            *self.phase.lock().unwrap() = SessionPhase::Cancelling;
            token.cancel();
        }
        Ok(())
    }
}
pub struct SessionManager {
    repo: Arc<SessionRepository>,
    active: Option<Arc<SessionHandle>>,
    activation: u64,
    pub startup_diagnostics: Vec<String>,
}
impl SessionManager {
    pub fn new(repo: SessionRepository) -> Self {
        Self {
            repo: Arc::new(repo),
            active: None,
            activation: 0,
            startup_diagnostics: vec![],
        }
    }
    pub fn repository(&self) -> &Arc<SessionRepository> {
        &self.repo
    }
    pub fn active(&self) -> Option<Arc<SessionHandle>> {
        self.active.clone()
    }
    pub fn activation(&self) -> u64 {
        self.activation
    }
    fn idle(&self) -> R<()> {
        if self
            .active
            .as_ref()
            .is_some_and(|h| matches!(h.phase(), SessionPhase::Running | SessionPhase::Cancelling))
        {
            return Err(E::SessionBusy(
                "stop the current run and wait for completion".into(),
            ));
        }
        Ok(())
    }
    pub async fn create(&mut self, metadata: SessionMetadata) -> R<SessionView> {
        self.idle()?;
        metadata.validate()?;
        self.repo.save_metadata(&metadata, None).await?;
        self.switch(&metadata.id, false, None).await
    }
    pub async fn switch(
        &mut self,
        id: &str,
        readonly: bool,
        ui: Option<&SessionUiState>,
    ) -> R<SessionView> {
        self.idle()?;
        if self.active.as_ref().is_some_and(|h| {
            h.identity.session_id == id
                && h.phase() == SessionPhase::Ready
                && h.runtime.is_some()
                && !readonly
        }) {
            return self.view().await;
        }
        // Acquire target first. A failure leaves the previous handle and UI untouched.
        let lease = if readonly {
            None
        } else if let Some(lease) = self
            .active
            .as_ref()
            .filter(|handle| handle.identity.session_id == id)
            .and_then(|handle| handle._lease.lock().unwrap().clone())
        {
            Some(lease)
        } else {
            Some(Arc::new(self.repo.lease(id)?))
        };
        if !readonly {
            self.repo.recover(id).await?;
        }
        let mut loaded = self.repo.load(id).await?;
        let report = replay(id, &loaded.segments)?;
        let healthy = matches!(
            report.health,
            SessionHealth::Complete | SessionHealth::Interrupted
        );
        let writable = !readonly && !loaded.metadata.archived && healthy;
        if !readonly && !healthy {
            return Err(E::SessionUnavailable(report.diagnostics.join("; ")));
        }
        if !readonly && loaded.metadata.archived {
            return Err(E::SessionArchived(id.into()));
        }
        if loaded.legacy && writable {
            loaded.revision = self.repo.save_metadata(&loaded.metadata, None).await?;
            loaded.legacy = false;
        }
        let runtime = if writable {
            Some(SessionRuntime::from_records(report.records, None).map_err(agent_error)?)
        } else {
            None
        };
        if let Some(runtime) = &runtime {
            runtime.mark_managed();
        }
        let handle = Arc::new(SessionHandle {
            identity: self.repo.identity(id),
            metadata: Mutex::new((loaded.metadata, loaded.revision)),
            runtime,
            phase: Mutex::new(SessionPhase::Ready),
            _lease: Mutex::new(lease),
            repo: self.repo.clone(),
            cancel: Mutex::new(None),
        });
        let info = self.inspect(id).await?;
        let history = history_page(
            &replay(id, &self.repo.load(id).await?.segments)?.history,
            None,
            50,
        )?;
        let target_ui = self.repo.read_ui(id);
        if let (Some(old), Some(ui)) = (&self.active, ui) {
            self.repo.save_ui(&old.identity.session_id, ui)?;
        }
        self.repo.set_last(id)?;
        if let Some(old) = &self.active {
            *old.phase.lock().unwrap() = SessionPhase::Closed;
            old._lease.lock().unwrap().take();
        }
        self.activation += 1;
        self.active = Some(handle);
        Ok(SessionView {
            info,
            history,
            ui: target_ui,
            writable,
            activation: self.activation,
        })
    }
    pub async fn restore_recent(&mut self) -> R<Option<SessionView>> {
        let Some(id) = self.repo.last_session() else {
            return Ok(None);
        };
        match self.switch(&id, false, None).await {
            Ok(view) => Ok(Some(view)),
            Err(error) => match self.switch(&id, true, None).await {
                Ok(mut view) => {
                    view.info.diagnostics.push(error.to_string());
                    Ok(Some(view))
                }
                Err(read_error) => {
                    self.startup_diagnostics.push(format!(
                        "recent session {id} unavailable: {error}; {read_error}"
                    ));
                    Ok(None)
                }
            },
        }
    }
    pub fn close(&mut self, ui: Option<&SessionUiState>) -> R<()> {
        self.idle()?;
        if let (Some(active), Some(ui)) = (&self.active, ui) {
            self.repo.save_ui(&active.identity.session_id, ui)?;
        }
        if let Some(old) = &self.active {
            *old.phase.lock().unwrap() = SessionPhase::Closed;
            old._lease.lock().unwrap().take();
        }
        self.active = None;
        self.activation += 1;
        Ok(())
    }
    pub async fn view(&self) -> R<SessionView> {
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| E::NotFound("no active session".into()))?;
        let info = self.inspect(&active.identity.session_id).await?;
        let loaded = self.repo.load(&active.identity.session_id).await?;
        let report = replay(&loaded.metadata.id, &loaded.segments)?;
        Ok(SessionView {
            history: history_page(&report.history, None, 50)?,
            ui: self.repo.read_ui(&loaded.metadata.id),
            info,
            writable: active.runtime.is_some() && !loaded.metadata.archived,
            activation: self.activation,
        })
    }
    pub async fn inspect(&self, id: &str) -> R<ManagedSessionInfo> {
        let loaded = self.repo.load(id).await?;
        self.info(loaded)
    }
    fn info(&self, loaded: LoadedSession) -> R<ManagedSessionInfo> {
        let report = replay(&loaded.metadata.id, &loaded.segments)?;
        Ok(ManagedSessionInfo {
            identity: self.repo.identity(&loaded.metadata.id),
            updated_at: report
                .updated_at
                .unwrap_or_else(|| loaded.metadata.created_at.clone()),
            metadata: loaded.metadata,
            revision: loaded.revision,
            message_count: report.history.len(),
            health: report.health,
            diagnostics: report.diagnostics,
            last_stop: report.last_stop,
            legacy: loaded.legacy,
        })
    }
    pub async fn list(&self, query: SessionQuery) -> R<SessionPage> {
        let tree = self.repo.files().await?;
        let generation = crate::workspace::files::fingerprint(&tree);
        let query_hash = content_hash(&serde_json::to_vec(&(
            &query.query,
            query.include_archived,
        ))?);
        let offset = if let Some(cursor) = query.cursor {
            let parts: Vec<_> = cursor.split(':').collect();
            if parts.len() != 3 || parts[0] != generation || parts[1] != query_hash {
                return Err(E::CursorExpired);
            }
            parts[2].parse::<usize>().map_err(|_| E::CursorExpired)?
        } else {
            0
        };
        // Cache each session summary and visible search text by exact durable content hash.
        let db = rusqlite::Connection::open(crate::workspace::files::safe_path(
            self.repo.root(),
            ".lumen/session-index.sqlite",
        )?)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,fingerprint TEXT,info TEXT,search TEXT)")?;
        let ids: BTreeSet<_> = tree
            .keys()
            .filter_map(|path| path.split('/').nth(1).map(str::to_owned))
            .collect();
        let mut items = vec![];
        for id in ids {
            let local: crate::workspace::files::FileTree = tree
                .iter()
                .filter(|(path, _)| path.starts_with(&format!("sessions/{id}/")))
                .map(|(p, b)| (p.clone(), b.clone()))
                .collect();
            let hash = crate::workspace::files::fingerprint(&local);
            let cached = db
                .query_row(
                    "SELECT info,search FROM sessions WHERE id=?1 AND fingerprint=?2",
                    rusqlite::params![id, hash],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .ok();
            let (info, search) = if let Some((info, search)) = cached {
                (serde_json::from_str::<ManagedSessionInfo>(&info)?, search)
            } else {
                let loaded = match self.repo.load_from(&id, &local) {
                    Ok(loaded) => loaded,
                    Err(error) => {
                        let mut metadata = SessionMetadata::new(
                            Some(format!("无法读取的会话 {id}")),
                            SessionModel::default(),
                            SessionContext::default(),
                        );
                        metadata.id = id.clone();
                        metadata.title_source = "imported".into();
                        metadata.created_at = "1970-01-01T00:00:00Z".into();
                        items.push(ManagedSessionInfo {
                            identity: self.repo.identity(&id),
                            metadata,
                            revision: String::new(),
                            updated_at: String::new(),
                            message_count: 0,
                            health: SessionHealth::Corrupt,
                            diagnostics: vec![error.to_string()],
                            last_stop: None,
                            legacy: false,
                        });
                        continue;
                    }
                };
                let report = replay(&id, &loaded.segments)?;
                let search = report
                    .history
                    .iter()
                    .map(|m| m.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .to_lowercase();
                let info = self.info(loaded)?;
                db.execute(
                    "INSERT OR REPLACE INTO sessions VALUES(?1,?2,?3,?4)",
                    rusqlite::params![id, hash, serde_json::to_string(&info)?, search],
                )?;
                (info, search)
            };
            let q = query.query.to_lowercase();
            if (query.include_archived || !info.metadata.archived)
                && (info.metadata.title.to_lowercase().contains(&q)
                    || search.contains(&q)
                    || id.contains(&q))
            {
                items.push(info);
            }
        }
        let q = query.query.to_lowercase();
        items.sort_by(|a, b| {
            b.metadata
                .title
                .to_lowercase()
                .contains(&q)
                .cmp(&a.metadata.title.to_lowercase().contains(&q))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.identity.session_id.cmp(&b.identity.session_id))
        });
        let limit = if query.limit == 0 {
            20
        } else {
            query.limit.clamp(1, 100)
        };
        let end = offset.saturating_add(limit).min(items.len());
        if offset > items.len() {
            return Err(E::CursorExpired);
        }
        let next_cursor = (end < items.len()).then(|| format!("{generation}:{query_hash}:{end}"));
        Ok(SessionPage {
            items: items[offset..end].to_vec(),
            next_cursor,
            generation,
            query: query.query,
        })
    }
    pub async fn update(
        &mut self,
        metadata: SessionMetadata,
        expected: &str,
    ) -> R<ManagedSessionInfo> {
        self.idle()?;
        let id = metadata.id.clone();
        let _lease = if self
            .active
            .as_ref()
            .is_some_and(|h| h.identity.session_id == id && h._lease.lock().unwrap().is_some())
        {
            None
        } else {
            Some(self.repo.lease(&id)?)
        };
        let current = self.repo.load(&id).await?;
        if current.metadata.created_at != metadata.created_at {
            return Err(E::InvalidFormat(
                "session creation time is immutable".into(),
            ));
        }
        let revision = self.repo.save_metadata(&metadata, Some(expected)).await?;
        if let Some(active) = self.active.as_ref().filter(|h| h.identity.session_id == id) {
            *active.metadata.lock().unwrap() = (metadata, revision);
        }
        self.inspect(&id).await
    }
    pub async fn rename(
        &mut self,
        id: &str,
        title: String,
        expected: &str,
    ) -> R<ManagedSessionInfo> {
        let mut metadata = self.inspect(id).await?.metadata;
        metadata.title = title;
        metadata.title_source = "user".into();
        self.update(metadata, expected).await
    }
    pub async fn archive(
        &mut self,
        id: &str,
        archived: bool,
        expected: &str,
    ) -> R<ManagedSessionInfo> {
        let mut metadata = self.inspect(id).await?.metadata;
        metadata.archived = archived;
        self.update(metadata, expected).await
    }
    pub async fn history(&self, id: &str, before: Option<&str>, limit: usize) -> R<HistoryPage> {
        let loaded = self.repo.load(id).await?;
        history_page(&replay(id, &loaded.segments)?.history, before, limit)
    }
    pub fn save_ui(&self, ui: &SessionUiState) -> R<()> {
        if let Some(active) = &self.active {
            self.repo.save_ui(&active.identity.session_id, ui)?;
        }
        Ok(())
    }
}
pub(crate) fn agent_error(error: AgentError) -> E {
    E::SessionUnavailable(error.to_string())
}
