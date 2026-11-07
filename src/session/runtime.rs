use crate::agent::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::Arc,
};
use tokio::sync::Mutex;

const COMMIT_KIND: &str = "lumen.context_commit";

#[async_trait]
pub trait TranscriptWriter: Send + Sync {
    async fn append(&self, message: &AgentMessage) -> Result<(), AgentError>;
}

struct FileWriter(std::sync::Mutex<std::fs::File>);
#[async_trait]
impl TranscriptWriter for FileWriter {
    async fn append(&self, message: &AgentMessage) -> Result<(), AgentError> {
        let mut f = self
            .0
            .lock()
            .map_err(|_| AgentError::Session("file lock poisoned".into()))?;
        let offset = f.seek(SeekFrom::End(0)).map_err(session_error)?;
        let mut line = serde_json::to_vec(&SessionRecord {
            message: message.clone(),
        })
        .map_err(session_error)?;
        line.push(b'\n');
        if let Err(error) = f.write_all(&line).and_then(|_| f.sync_data()) {
            // Roll back a failed append while holding the exclusive session lease.
            f.set_len(offset).map_err(session_error)?;
            f.sync_data().map_err(session_error)?;
            return Err(session_error(error));
        }
        Ok(())
    }
}
fn session_error(e: impl std::fmt::Display) -> AgentError {
    AgentError::Session(e.to_string())
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct WindowState {
    pub epoch: u64,
    pub through: usize,
    pub projection: Vec<AgentMessage>,
    pub keys: BTreeSet<String>,
    pub config: String,
    pub manifest: String,
    #[serde(default)]
    pub pins: BTreeMap<String, AgentMessage>,
    #[serde(default)]
    pub control_through: usize,
    #[serde(default)]
    pub regions: BTreeMap<String, crate::context::ContextRegion>,
    #[serde(default)]
    pub report: BTreeMap<String, usize>,
}

#[derive(Serialize, Deserialize)]
struct ContextCommit {
    schema_version: u32,
    operation_id: String,
    parent: u64,
    parent_manifest: String,
    epoch: u64,
    through: usize,
    raw_hash: String,
    reset: Option<Vec<AgentMessage>>,
    append: Vec<AgentMessage>,
    keys: BTreeSet<String>,
    config: String,
    manifest: String,
    pins: BTreeMap<String, AgentMessage>,
    control_through: usize,
    regions: BTreeMap<String, crate::context::ContextRegion>,
    report: BTreeMap<String, usize>,
}
#[derive(Clone)]
pub struct SessionSnapshot {
    pub messages: Vec<AgentMessage>,
    pub window: WindowState,
    pub pending: Vec<crate::context::ContextContribution>,
}
struct RuntimeData {
    messages: Vec<AgentMessage>,
    window: WindowState,
    pending: Vec<crate::context::ContextContribution>,
}

/// Application-owned single-session runtime. All durable state is replayable from records.
pub struct SessionRuntime {
    data: Mutex<RuntimeData>,
    writer: Mutex<Option<Arc<dyn TranscriptWriter>>>,
    pub(crate) run_lock: Mutex<()>,
    force_compact: std::sync::atomic::AtomicBool,
    failed: std::sync::atomic::AtomicBool,
    managed: std::sync::atomic::AtomicBool,
}
impl SessionRuntime {
    pub fn memory() -> Arc<Self> {
        Self::from_records(vec![], None).unwrap()
    }
    pub fn from_records(
        records: Vec<AgentMessage>,
        writer: Option<Arc<dyn TranscriptWriter>>,
    ) -> Result<Arc<Self>, AgentError> {
        let mut messages = vec![];
        let mut window = WindowState::default();
        let mut pending = vec![];
        let mut operations = BTreeMap::new();
        for record in records {
            if let AgentMessage::Custom { kind, content } = &record {
                if kind == "lumen.session_start" {
                    messages.clear();
                    window = WindowState::default();
                    pending.clear();
                    operations.clear();
                    continue;
                }
                if kind == "lumen.context_pending" {
                    if let Some(items) = content.get("items") {
                        let items: Vec<crate::context::ContextContribution> =
                            serde_json::from_value(items.clone()).map_err(session_error)?;
                        pending.extend(items);
                    } else {
                        pending
                            .push(serde_json::from_value(content.clone()).map_err(session_error)?);
                    }
                    continue;
                }
                if kind == COMMIT_KIND {
                    let c: ContextCommit =
                        serde_json::from_value(content.clone()).map_err(session_error)?;
                    let hash = blake3::hash(content.to_string().as_bytes())
                        .to_hex()
                        .to_string();
                    if let Some(previous) = operations.insert(c.operation_id.clone(), hash.clone())
                    {
                        if previous == hash {
                            continue;
                        }
                        return Err(AgentError::Session(
                            "conflicting context operation ID".into(),
                        ));
                    }
                    if c.schema_version != 1 {
                        return Err(AgentError::Session(
                            "unsupported required context journal version".into(),
                        ));
                    }
                    if c.parent != window.epoch
                        || c.parent_manifest != window.manifest
                        || c.epoch != c.parent + u64::from(c.reset.is_some())
                        || c.through < window.through
                        || c.through > messages.len()
                        || c.raw_hash != raw_hash(&messages[..c.through])
                    {
                        return Err(AgentError::Session(
                            "invalid context commit parent or coverage".into(),
                        ));
                    }
                    if let Some(reset) = c.reset {
                        window.projection = reset;
                    }
                    window.projection.extend(c.append);
                    window.epoch = c.epoch;
                    window.through = c.through;
                    window.keys.extend(c.keys);
                    window.config = c.config;
                    window.manifest = c.manifest;
                    window.pins = c.pins;
                    window.control_through = c.control_through;
                    window.regions = c.regions;
                    window.report = c.report;
                    if window.control_through > pending.len() {
                        return Err(AgentError::Session(
                            "invalid pending context watermark".into(),
                        ));
                    }
                    continue;
                }
            }
            messages.push(record);
        }
        Ok(Arc::new(Self {
            data: Mutex::new(RuntimeData {
                messages,
                window,
                pending,
            }),
            writer: Mutex::new(writer),
            run_lock: Mutex::new(()),
            force_compact: std::sync::atomic::AtomicBool::new(false),
            failed: std::sync::atomic::AtomicBool::new(false),
            managed: std::sync::atomic::AtomicBool::new(false),
        }))
    }
    pub fn open_file(path: &Path) -> Result<Arc<Self>, AgentError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(session_error)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(session_error)?;
        fs2::FileExt::try_lock_exclusive(&f).map_err(session_error)?;
        let mut bytes = vec![];
        f.read_to_end(&mut bytes).map_err(session_error)?;
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            let backup = path.with_extension(format!("{}.recovery", uuid::Uuid::now_v7()));
            std::fs::write(backup, &bytes).map_err(session_error)?;
            let end = bytes
                .iter()
                .rposition(|b| *b == b'\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            f.set_len(end as u64).map_err(session_error)?;
            f.sync_data().map_err(session_error)?;
            bytes.truncate(end);
        }
        let records = bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|line| {
                serde_json::from_slice::<SessionRecord>(line)
                    .map(|r| r.message)
                    .map_err(session_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_records(
            records,
            Some(Arc::new(FileWriter(std::sync::Mutex::new(f)))),
        )
    }
    pub async fn set_writer(
        &self,
        writer: Option<Arc<dyn TranscriptWriter>>,
    ) -> Result<(), AgentError> {
        let _lease = self
            .run_lock
            .try_lock()
            .map_err(|_| AgentError::Session("cannot replace writer during a run".into()))?;
        if self.managed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AgentError::Session(
                "managed writer can only be bound by SessionRunner".into(),
            ));
        }
        *self.writer.lock().await = writer;
        Ok(())
    }
    pub(crate) async fn bind_managed_writer(
        &self,
        writer: Option<Arc<dyn TranscriptWriter>>,
    ) -> Result<(), AgentError> {
        let _lease = self
            .run_lock
            .try_lock()
            .map_err(|_| AgentError::Session("run still owns writer binding".into()))?;
        self.managed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        *self.writer.lock().await = writer;
        Ok(())
    }
    pub fn is_faulted(&self) -> bool {
        self.failed.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub(crate) fn mark_managed(&self) {
        self.managed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    /// Keep older sessions in the file while making the new session the replay target.
    pub async fn start_new(&self, session_id: &str) -> Result<(), AgentError> {
        let _lease = self.run_lock.lock().await;
        let mut data = self.data.lock().await;
        self.write(&AgentMessage::Custom {
            kind: "lumen.session_start".into(),
            content: serde_json::json!({"schema_version":1,"session_id":session_id}),
        })
        .await?;
        data.messages.clear();
        data.window = WindowState::default();
        data.pending.clear();
        self.force_compact
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    pub async fn snapshot(&self) -> SessionSnapshot {
        let data = self.data.lock().await;
        SessionSnapshot {
            messages: data.messages.clone(),
            window: data.window.clone(),
            pending: data.pending.clone(),
        }
    }
    pub async fn append(&self, message: AgentMessage) -> Result<(), AgentError> {
        let mut data = self.data.lock().await;
        self.write(&message).await?;
        data.messages.push(message);
        Ok(())
    }
    pub(crate) async fn offer_batch(
        &self,
        contributions: Vec<crate::context::ContextContribution>,
    ) -> Result<(), AgentError> {
        let mut data = self.data.lock().await;
        self.write(&AgentMessage::Custom {
            kind: "lumen.context_pending".into(),
            content: serde_json::json!({"schema_version":1,"operation_id":uuid::Uuid::now_v7().to_string(),"items":contributions}),
        })
        .await?;
        data.pending.extend(contributions);
        Ok(())
    }
    pub async fn save_artifact(&self, result: &ToolResultMessage) -> Result<String, AgentError> {
        let content = serde_json::to_value(result).map_err(session_error)?;
        let id = blake3::hash(content.to_string().as_bytes())
            .to_hex()
            .to_string();
        let mut data = self.data.lock().await;
        if !data.messages.iter().any(|m| matches!(m, AgentMessage::Custom { kind, content } if kind == "lumen.tool_artifact" && content["id"] == id)) {
            let message = AgentMessage::Custom { kind: "lumen.tool_artifact".into(), content: serde_json::json!({"id":id,"result":content}) };
            self.write(&message).await?;
            data.messages.push(message);
        }
        Ok(id)
    }
    pub async fn read_artifact(&self, id: &str) -> Result<ToolResultMessage, AgentError> {
        let data = self.data.lock().await;
        data.messages
            .iter()
            .find_map(|m| match m {
                AgentMessage::Custom { kind, content }
                    if kind == "lumen.tool_artifact" && content["id"] == id =>
                {
                    Some(serde_json::from_value(content["result"].clone()).map_err(session_error))
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                Err(AgentError::Context(
                    "artifact not found in this session".into(),
                ))
            })
    }
    async fn write(&self, message: &AgentMessage) -> Result<(), AgentError> {
        if self.failed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AgentError::Session(
                "previous durable write failed; reopen/resume the session before continuing".into(),
            ));
        }
        let binding = self.writer.lock().await;
        if self.managed.load(std::sync::atomic::Ordering::SeqCst) && binding.is_none() {
            return Err(AgentError::Session(
                "no active run writer for managed session".into(),
            ));
        }
        if let Some(writer) = binding.as_ref() {
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(5), writer.append(message))
                    .await
                    .unwrap_or_else(|_| {
                        Err(AgentError::Session(
                    "durable write timed out; outcome unknown, reopen/resume before continuing"
                        .into(),
                ))
                    });
            if let Err(error) = result {
                self.failed.store(true, std::sync::atomic::Ordering::SeqCst);
                return Err(error);
            }
        }
        Ok(())
    }
    pub async fn commit(
        &self,
        previous: &WindowState,
        next: WindowState,
        reset: bool,
    ) -> Result<(), AgentError> {
        let mut data = self.data.lock().await;
        if data.window.epoch != previous.epoch
            || data.window.manifest != previous.manifest
            || next.through > data.messages.len()
        {
            return Err(AgentError::Session("stale context checkpoint".into()));
        }
        if next.epoch != previous.epoch + u64::from(reset)
            || next.through < previous.through
            || next.control_through < previous.control_through
            || next.control_through > data.pending.len()
            || (!reset && !next.projection.starts_with(&previous.projection))
        {
            return Err(AgentError::Session(
                "invalid checkpoint projection or watermark".into(),
            ));
        }
        let append = if reset {
            vec![]
        } else {
            next.projection[previous.projection.len()..].to_vec()
        };
        let commit = ContextCommit {
            schema_version: 1,
            operation_id: uuid::Uuid::now_v7().to_string(),
            parent: previous.epoch,
            parent_manifest: previous.manifest.clone(),
            epoch: next.epoch,
            through: next.through,
            raw_hash: raw_hash(&data.messages[..next.through]),
            reset: reset.then(|| next.projection.clone()),
            append,
            keys: next.keys.difference(&previous.keys).cloned().collect(),
            config: next.config.clone(),
            manifest: next.manifest.clone(),
            pins: next.pins.clone(),
            control_through: next.control_through,
            regions: next.regions.clone(),
            report: next.report.clone(),
        };
        self.write(&AgentMessage::Custom {
            kind: COMMIT_KIND.into(),
            content: serde_json::to_value(commit).map_err(session_error)?,
        })
        .await?;
        data.window = next;
        Ok(())
    }
    pub fn request_compaction(&self) {
        self.force_compact
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub(crate) fn take_compaction(&self) -> bool {
        self.force_compact
            .swap(false, std::sync::atomic::Ordering::SeqCst)
    }
}
fn raw_hash(messages: &[AgentMessage]) -> String {
    blake3::hash(&serde_json::to_vec(messages).unwrap())
        .to_hex()
        .to_string()
}

pub struct WorkspaceTranscriptWriter {
    pub archive: crate::workspace::SessionArchive,
    pub segment_id: String,
}
#[async_trait]
impl TranscriptWriter for WorkspaceTranscriptWriter {
    async fn append(&self, message: &AgentMessage) -> Result<(), AgentError> {
        self.archive
            .append(
                self.segment_id.clone(),
                message.clone(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .map_err(session_error)
    }
}
