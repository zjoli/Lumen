use super::*;
use crate::{
    agent::{AgentMessage, SessionRecord},
    workspace::{files, WorkspaceError as E, WorkspaceHandle, WorkspaceResult as R},
};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct SessionRepository {
    root: PathBuf,
    store_id: String,
    workspace: Option<WorkspaceHandle>,
}
pub struct SessionLease {
    _file: fs::File,
    pub identity: SessionIdentity,
}
impl Drop for SessionLease {
    fn drop(&mut self) {
        // A concurrently spawned child can briefly inherit the open file description
        // before exec closes CLOEXEC descriptors. Closing only our descriptor then
        // leaves flock held in that child; explicitly release the completed lease.
        let _ = fs2::FileExt::unlock(&self._file);
    }
}
pub struct LoadedSession {
    pub metadata: SessionMetadata,
    pub revision: String,
    pub segments: Vec<StoredSegment>,
    pub legacy: bool,
}
impl SessionRepository {
    pub(crate) async fn publish_entries(
        &self,
        entries: BTreeMap<String, Option<Vec<u8>>>,
    ) -> R<()> {
        if let Some(w) = &self.workspace {
            return w
                .call(CancellationToken::new(), move |s, _| {
                    s.ensure_writable()?;
                    for path in entries.keys() {
                        if files::safe_path(&s.root, path)?.exists() {
                            return Err(E::RevisionConflict(path.clone()));
                        }
                    }
                    files::apply(&s.root, entries)?;
                    Ok(())
                })
                .await;
        }
        let _lock = lock_file(&self.root, ".lumen/locks/catalog.lock")?;
        files::apply_checked(
            &self.root,
            entries.clone(),
            entries.keys().map(|p| (p.clone(), None)).collect(),
        )?;
        Ok(())
    }
    pub async fn workspace(workspace: WorkspaceHandle) -> R<Self> {
        let info = workspace.info(CancellationToken::new()).await?;
        Ok(Self {
            root: info.root,
            store_id: info.repository_id,
            workspace: Some(workspace),
        })
    }
    pub fn standalone(root: impl Into<PathBuf>) -> R<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        fs::create_dir_all(files::safe_path(&root, ".lumen/locks")?)?;
        fs::create_dir_all(files::safe_path(&root, "sessions")?)?;
        let _init = lock_file(&root, ".lumen/locks/store.lock")?;
        files::recover(&root)?;
        let path = files::safe_path(&root, "store.yaml")?;
        let id = if path.exists() {
            let value: serde_json::Value = serde_yaml_ng::from_slice(&fs::read(path)?)?;
            if value["schema_version"] != 1 {
                return Err(E::UnsupportedSchema(
                    value["schema_version"].as_u64().unwrap_or(0) as u32,
                ));
            }
            value["store_id"]
                .as_str()
                .ok_or_else(|| E::InvalidFormat("missing store ID".into()))?
                .to_string()
        } else {
            let id = uuid::Uuid::now_v7().to_string();
            files::atomic_write(
                &root,
                "store.yaml",
                serde_yaml_ng::to_string(&serde_json::json!({"schema_version":1,"store_id":id}))?
                    .as_bytes(),
            )?;
            id
        };
        Ok(Self {
            root,
            store_id: id,
            workspace: None,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn store_id(&self) -> &str {
        &self.store_id
    }
    pub fn identity(&self, id: &str) -> SessionIdentity {
        SessionIdentity {
            store_id: self.store_id.clone(),
            session_id: id.into(),
        }
    }
    pub fn lease(&self, id: &str) -> R<SessionLease> {
        crate::workspace::validate_typed_id(id, "session")?;
        Ok(SessionLease {
            _file: lock_file(&self.root, &format!(".lumen/locks/session-{id}.lock"))?,
            identity: self.identity(id),
        })
    }
    pub async fn files(&self) -> R<files::FileTree> {
        if let Some(w) = &self.workspace {
            return w
                .call(CancellationToken::new(), |s, _| {
                    let tree = files::scan(&s.root)?;
                    Ok(tree
                        .into_iter()
                        .filter(|(p, _)| p.starts_with("sessions/"))
                        .collect())
                })
                .await;
        }
        let mut out = BTreeMap::new();
        let root = files::safe_path(&self.root, "sessions")?;
        if root.exists() {
            for entry in fs::read_dir(root)? {
                let entry = entry?;
                let id = entry.file_name().to_string_lossy().to_string();
                crate::workspace::validate_typed_id(&id, "session")?;
                let dir = files::safe_path(&self.root, &format!("sessions/{id}"))?;
                if !dir.is_dir() {
                    return Err(E::InvalidPath(dir.display().to_string()));
                }
                for entry in fs::read_dir(dir)? {
                    let name = entry?.file_name().to_string_lossy().to_string();
                    if name.ends_with(".open") || name.starts_with(".lumen-") {
                        continue;
                    }
                    let relative = format!("sessions/{id}/{name}");
                    out.insert(
                        relative.clone(),
                        fs::read(files::safe_path(&self.root, &relative)?)?,
                    );
                }
            }
        }
        Ok(out)
    }
    pub async fn load(&self, id: &str) -> R<LoadedSession> {
        self.load_from(id, &self.files().await?)
    }
    pub fn load_from(&self, id: &str, tree: &files::FileTree) -> R<LoadedSession> {
        crate::workspace::validate_typed_id(id, "session")?;
        let prefix = format!("sessions/{id}/");
        let metadata = tree.get(&format!("{prefix}session.yaml"));
        let mut segments = vec![];
        for (path, bytes) in tree
            .range(prefix.clone()..)
            .take_while(|(path, _)| path.starts_with(&prefix))
        {
            if path.ends_with("/session.yaml") {
                validate_session_file(path, bytes)?;
                continue;
            }
            if let Some(name) = path.strip_suffix(".jsonl") {
                crate::workspace::validate_typed_id(name.rsplit('/').next().unwrap(), "segment")?;
                segments.push(StoredSegment {
                    id: name.rsplit('/').next().unwrap().into(),
                    bytes: bytes.clone(),
                });
            } else {
                return Err(E::InvalidPath(path.clone()));
            }
        }
        if metadata.is_none() && segments.is_empty() {
            return Err(E::NotFound(id.into()));
        }
        let (metadata, revision, legacy) = if let Some(bytes) = metadata {
            (
                serde_yaml_ng::from_slice(bytes)?,
                content_hash(bytes),
                false,
            )
        } else {
            let report = replay(id, &segments)?;
            let title = report
                .history
                .iter()
                .find(|m| m.role == "user")
                .map(|m| m.text.chars().take(60).collect())
                .unwrap_or_else(|| "导入会话".into());
            let mut meta = SessionMetadata::new(
                Some(title),
                SessionModel::default(),
                SessionContext::default(),
            );
            meta.id = id.into();
            meta.title_source = "imported".into();
            if let Ok(uuid) = uuid::Uuid::parse_str(id.trim_start_matches("session_")) {
                if let Some(timestamp) = uuid.get_timestamp() {
                    let (seconds, nanos) = timestamp.to_unix();
                    if let Some(time) = chrono::DateTime::from_timestamp(seconds as i64, nanos) {
                        meta.created_at = time.to_rfc3339();
                    }
                }
            }
            (meta, "legacy".into(), true)
        };
        Ok(LoadedSession {
            metadata,
            revision,
            segments,
            legacy,
        })
    }
    pub async fn save_metadata(
        &self,
        metadata: &SessionMetadata,
        expected: Option<&str>,
    ) -> R<String> {
        metadata.validate()?;
        let path = format!("sessions/{}/session.yaml", metadata.id);
        let bytes = serde_yaml_ng::to_string(metadata)?.into_bytes();
        let expected = expected.map(str::to_owned);
        let revision = content_hash(&bytes);
        if let Some(w) = &self.workspace {
            w.call(CancellationToken::new(), move |s, _| {
                s.ensure_writable()?;
                let current = fs::read(files::safe_path(&s.root, &path)?)
                    .ok()
                    .map(|bytes| content_hash(&bytes));
                if current != expected {
                    return Err(E::RevisionConflict(path));
                }
                files::apply_checked(
                    &s.root,
                    BTreeMap::from([(path.clone(), Some(bytes))]),
                    BTreeMap::from([(path, expected)]),
                )?;
                Ok(())
            })
            .await?;
        } else {
            let _lock = lock_file(&self.root, ".lumen/locks/catalog.lock")?;
            let current = fs::read(files::safe_path(&self.root, &path)?)
                .ok()
                .map(|bytes| content_hash(&bytes));
            if current != expected {
                return Err(E::RevisionConflict(path));
            }
            files::atomic_write(&self.root, &path, &bytes)?;
        }
        Ok(revision)
    }
    pub async fn begin(&self, id: &str) -> R<String> {
        if let Some(w) = &self.workspace {
            return Ok(w
                .sessions()
                .begin(id.into(), CancellationToken::new())
                .await?
                .segment_id);
        }
        let segment = format!("segment_{}", uuid::Uuid::now_v7());
        files::atomic_write(
            &self.root,
            &format!("sessions/{id}/{segment}.jsonl.open"),
            b"",
        )?;
        Ok(segment)
    }
    pub async fn append(&self, id: &str, segment: &str, message: &AgentMessage) -> R<()> {
        if let Some(w) = &self.workspace {
            return w
                .sessions()
                .append(segment.into(), message.clone(), CancellationToken::new())
                .await;
        }
        let path = segment_path(id, segment, true)?;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(files::safe_path(&self.root, &path)?)?;
        let mut line = serde_json::to_vec(&SessionRecord {
            message: message.clone(),
        })?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.sync_data()?;
        Ok(())
    }
    pub async fn finish(&self, id: &str, segment: &str) -> R<()> {
        if let Some(w) = &self.workspace {
            w.sessions()
                .finish(segment.into(), CancellationToken::new())
                .await?;
            return Ok(());
        }
        let open = files::safe_path(&self.root, &segment_path(id, segment, true)?)?;
        let closed = segment_path(id, segment, false)?;
        validate_session_file(&closed, &fs::read(&open)?)?;
        if files::safe_path(&self.root, &closed)?.exists() {
            return Err(E::RevisionConflict("closed segment already exists".into()));
        }
        fs::rename(open, files::safe_path(&self.root, &closed)?)?;
        sync_dir(&self.root.join("sessions").join(id))?;
        Ok(())
    }
    pub async fn recover(&self, id: &str) -> R<()> {
        // Caller holds the session lease. Recovery must not race a live managed writer.
        if let Some(w) = &self.workspace {
            let id = id.to_owned();
            return w
                .call(CancellationToken::new(), move |s, _| {
                    s.ensure_writable()?;
                    crate::workspace::recover_session_segments(&s.root, &id)?;
                    Ok(())
                })
                .await;
        }
        let dir = files::safe_path(&self.root, &format!("sessions/{id}"))?;
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let name = entry?.file_name().to_string_lossy().to_string();
            if !name.ends_with(".jsonl.open") {
                continue;
            }
            let path = format!("sessions/{id}/{name}");
            let bytes = fs::read(files::safe_path(&self.root, &path)?)?;
            let end = bytes
                .iter()
                .rposition(|b| *b == b'\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            let closed = path.trim_end_matches(".open");
            validate_session_file(closed, &bytes[..end])?;
            if end != bytes.len() {
                files::atomic_write(
                    &self.root,
                    &format!(".lumen/recovered-{}.jsonl", uuid::Uuid::now_v7()),
                    &bytes,
                )?;
                files::atomic_write(&self.root, &path, &bytes[..end])?;
            }
            fs::rename(
                files::safe_path(&self.root, &path)?,
                files::safe_path(&self.root, closed)?,
            )?;
        }
        sync_dir(&self.root.join("sessions").join(id))?;
        Ok(())
    }
    pub fn read_ui(&self, id: &str) -> SessionUiState {
        self.read_local(&format!("session-ui/{id}.json"))
            .unwrap_or_default()
    }
    pub fn save_ui(&self, id: &str, ui: &SessionUiState) -> R<()> {
        self.write_local(&format!("session-ui/{id}.json"), ui)
    }
    pub fn last_session(&self) -> Option<String> {
        self.read_local("last-session.json").ok()
    }
    pub fn set_last(&self, id: &str) -> R<()> {
        self.write_local("last-session.json", &id)
    }
    fn read_local<T: serde::de::DeserializeOwned>(&self, name: &str) -> R<T> {
        Ok(serde_json::from_slice(&fs::read(files::safe_path(
            &self.root,
            &format!(".lumen/{name}"),
        )?)?)?)
    }
    pub(crate) fn write_local<T: serde::Serialize + ?Sized>(&self, name: &str, value: &T) -> R<()> {
        files::atomic_write(
            &self.root,
            &format!(".lumen/{name}"),
            &serde_json::to_vec(value)?,
        )
    }
}
pub(crate) fn lock_file(root: &Path, path: &str) -> R<fs::File> {
    let path = files::safe_path(root, path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    fs2::FileExt::try_lock_exclusive(&file)
        .map_err(|_| E::SessionBusy(path.display().to_string()))?;
    Ok(file)
}
fn segment_path(id: &str, segment: &str, open: bool) -> R<String> {
    crate::workspace::validate_typed_id(id, "session")?;
    crate::workspace::validate_typed_id(segment, "segment")?;
    Ok(format!(
        "sessions/{id}/{segment}.jsonl{}",
        if open { ".open" } else { "" }
    ))
}
pub(crate) fn sync_dir(path: &Path) -> R<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()?;
    }
    Ok(())
}
pub(crate) struct ManagedWriter {
    pub repo: Arc<SessionRepository>,
    pub session_id: String,
    pub segment_id: String,
    pub active: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl TranscriptWriter for ManagedWriter {
    async fn append(&self, message: &AgentMessage) -> Result<(), crate::agent::AgentError> {
        if !self.active.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(crate::agent::AgentError::Session(
                "writer binding expired".into(),
            ));
        }
        self.repo
            .append(&self.session_id, &self.segment_id, message)
            .await
            .map_err(|e| crate::agent::AgentError::Session(e.to_string()))
    }
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    #[test]
    fn dropping_lease_unlocks_even_if_an_inherited_descriptor_remains_open() {
        let dir = tempfile::tempdir().unwrap();
        let repository = SessionRepository::standalone(dir.path()).unwrap();
        let id = format!("session_{}", uuid::Uuid::now_v7());
        let lease = repository.lease(&id).unwrap();
        let inherited = lease._file.try_clone().unwrap();
        assert!(repository.lease(&id).is_err());
        drop(lease);
        let next = repository.lease(&id).unwrap();
        drop(inherited);
        assert!(repository.lease(&id).is_err());
        drop(next);
        assert!(repository.lease(&id).is_ok());
    }
}
