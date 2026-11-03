mod error;
pub(crate) mod files;
mod session;
use crate::{knowledge::*, retrieval, versioning};
pub use error::*;
use serde::{Deserialize, Serialize};
pub(crate) use session::finish_all as session_finish_all;
pub use session::*;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
pub type WorkspaceHandle = Arc<Workspace>;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryConfig {
    pub schema_version: u32,
    pub repository_id: String,
    pub default_language: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LocalConfig {
    pub author_name: String,
    pub author_email: String,
    pub credential_ref: Option<String>,
    pub ssh_private_key: Option<PathBuf>,
    pub context_tokens: usize,
    pub search_limit: usize,
}
impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            author_name: String::new(),
            author_email: String::new(),
            credential_ref: None,
            ssh_private_key: None,
            context_tokens: 4000,
            search_limit: 20,
        }
    }
}
pub type LocalConfigPatch = serde_json::Map<String, serde_json::Value>;
#[derive(Clone, Debug)]
pub struct InitRequest {
    pub root: PathBuf,
    pub author_name: String,
    pub author_email: String,
}
#[derive(Clone, Debug)]
pub struct CloneRequest {
    pub root: PathBuf,
    pub url: String,
    pub config: LocalConfig,
}
#[derive(Clone, Debug, Serialize)]
pub struct WorkspaceInfo {
    pub root: PathBuf,
    pub repository_id: String,
    pub schema_version: u32,
}
#[derive(Clone, Debug, Serialize)]
pub struct WorkspaceStatus {
    pub info: WorkspaceInfo,
    pub version: versioning::VersionStatus,
    pub index: retrieval::IndexStatus,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub path: String,
    pub message: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ValidationScope {
    WorkingTree,
    Index,
    Commit(versioning::CommitId),
}
#[derive(Clone, Debug, Serialize)]
pub struct RecoveryReport {
    pub transactions: usize,
    pub sessions: usize,
    pub needs_resolution: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct CloseReport {
    pub closed: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct WorkspaceEvent {
    pub operation_id: String,
    pub generation: Option<String>,
    pub kind: WorkspaceEventKind,
}
#[derive(Clone, Debug, Serialize)]
pub enum WorkspaceEventKind {
    FilesChanged,
    IndexProgress,
    IndexReady,
    IndexFailed,
    SyncProgress,
    SyncCompleted,
    NeedsResolution,
    RecoveryRequired,
}
type Job = Box<dyn FnOnce(&mut State) + Send>;
pub struct Workspace {
    root: PathBuf,
    tx: mpsc::Sender<Job>,
    events: broadcast::Sender<WorkspaceEvent>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
    closed: Arc<AtomicBool>,
    pub(crate) maintenance_notify: Arc<tokio::sync::Notify>,
    maintenance_cancel: CancellationToken,
    maintenance: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(crate) embedding_gate: tokio::sync::Mutex<()>,
    pub(crate) providers: retrieval::Providers,
    pub(crate) credentials: std::sync::RwLock<Arc<dyn versioning::CredentialProvider>>,
}
pub(crate) struct State {
    pub root: PathBuf,
    pub repository: RepositoryConfig,
    pub config: LocalConfig,
    pub events: broadcast::Sender<WorkspaceEvent>,
    pub segment_locks: std::collections::BTreeMap<String, fs::File>,
}
pub struct WorkspaceService;
enum OpenMode {
    Open,
    Init(LocalConfig),
    Clone(String, LocalConfig, versioning::CredentialMaterial),
}
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl WorkspaceService {
    pub async fn init(
        req: InitRequest,
        cancel: CancellationToken,
    ) -> WorkspaceResult<WorkspaceHandle> {
        if req.author_name.trim().is_empty() || req.author_email.trim().is_empty() {
            return Err(WorkspaceError::Configuration(
                "author_name and author_email required".into(),
            ));
        }
        Self::start(
            req.root,
            OpenMode::Init(LocalConfig {
                author_name: req.author_name,
                author_email: req.author_email,
                ..Default::default()
            }),
            cancel,
        )
        .await
    }
    pub async fn open(
        path: impl Into<PathBuf>,
        cancel: CancellationToken,
    ) -> WorkspaceResult<WorkspaceHandle> {
        Self::start(path.into(), OpenMode::Open, cancel).await
    }
    pub async fn clone(
        req: CloneRequest,
        cancel: CancellationToken,
    ) -> WorkspaceResult<WorkspaceHandle> {
        let credential = versioning::SystemCredentials
            .resolve(
                versioning::AuthRequest {
                    url: req.url.clone(),
                    config: req.config.clone(),
                },
                cancel.clone(),
            )
            .await?;
        Self::start(
            req.root,
            OpenMode::Clone(req.url, req.config, credential),
            cancel,
        )
        .await
    }
    async fn start(
        root: PathBuf,
        mode: OpenMode,
        cancel: CancellationToken,
    ) -> WorkspaceResult<WorkspaceHandle> {
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()?.join(root)
        };
        let closed = Arc::new(AtomicBool::new(false));
        let worker_closed = closed.clone();
        let (tx, mut rx) = mpsc::channel::<Job>(64);
        let (events, _) = broadcast::channel(256);
        let event_tx = events.clone();
        let (worker_tx, worker_rx) = oneshot::channel();
        let worker_root = root.clone();
        let thread = thread::Builder::new()
            .name("lumen-workspace".into())
            .spawn(move || {
                let state = prepare(worker_root, mode, &cancel, event_tx);
                match state {
                    Err(e) => {
                        let _ = worker_tx.send(Err(e));
                    }
                    Ok(mut state) => {
                        let _ = worker_tx.send(Ok(()));
                        while let Some(job) = rx.blocking_recv() {
                            job(&mut state);
                            if worker_closed.load(Ordering::Acquire) {
                                break;
                            }
                        }
                    }
                }
            })?;
        worker_rx
            .await
            .map_err(|_| WorkspaceError::RecoveryRequired("workspace worker stopped".into()))??;
        let workspace = Arc::new(Workspace {
            root,
            tx,
            events,
            thread: Mutex::new(Some(thread)),
            closed,
            maintenance_notify: Arc::new(tokio::sync::Notify::new()),
            maintenance_cancel: CancellationToken::new(),
            maintenance: Mutex::new(None),
            embedding_gate: tokio::sync::Mutex::new(()),
            providers: Default::default(),
            credentials: std::sync::RwLock::new(Arc::new(versioning::SystemCredentials)),
        });
        let maintenance = retrieval::start_maintenance(
            Arc::downgrade(&workspace),
            workspace.subscribe(),
            workspace.maintenance_notify.clone(),
            workspace.maintenance_cancel.clone(),
        );
        *workspace
            .maintenance
            .lock()
            .map_err(|_| WorkspaceError::WorkspaceBusy)? = Some(maintenance);
        Ok(workspace)
    }
}
use crate::versioning::CredentialProvider;
impl Workspace {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn knowledge(self: &Arc<Self>) -> KnowledgeService {
        KnowledgeService(self.clone())
    }
    pub fn resources(self: &Arc<Self>) -> ResourceService {
        ResourceService(self.clone())
    }
    pub fn learning(self: &Arc<Self>) -> LearningService {
        LearningService(self.clone())
    }
    pub fn sessions(self: &Arc<Self>) -> SessionArchive {
        SessionArchive(self.clone())
    }
    pub fn retrieval(self: &Arc<Self>) -> retrieval::RetrievalService {
        retrieval::RetrievalService(self.clone())
    }
    pub fn index(self: &Arc<Self>) -> retrieval::IndexService {
        retrieval::IndexService(self.clone())
    }
    pub fn summaries(self: &Arc<Self>) -> retrieval::SummaryService {
        retrieval::SummaryService(self.clone())
    }
    pub fn versions(self: &Arc<Self>) -> versioning::VersionService {
        versioning::VersionService(self.clone())
    }
    pub fn subscribe(&self) -> broadcast::Receiver<WorkspaceEvent> {
        self.events.subscribe()
    }
    pub(crate) async fn call<T: Send + 'static>(
        &self,
        cancel: CancellationToken,
        f: impl FnOnce(&mut State, &CancellationToken) -> WorkspaceResult<T> + Send + 'static,
    ) -> WorkspaceResult<T> {
        if self.closed.load(Ordering::Acquire) {
            return Err(WorkspaceError::Configuration("workspace closed".into()));
        }
        check_cancel(&cancel)?;
        let child = cancel.child_token();
        let _guard = CancelOnDrop(child.clone());
        let (tx, rx) = oneshot::channel();
        let job: Job = Box::new(move |state| {
            let result = (|| {
                check_cancel(&child)?;
                let lock = fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(files::safe_path(
                        &state.root,
                        ".lumen/locks/workspace.lock",
                    )?)?;
                fs2::FileExt::try_lock_exclusive(&lock)
                    .map_err(|_| WorkspaceError::WorkspaceBusy)?;
                let result = f(state, &child);
                let _ = fs2::FileExt::unlock(&lock);
                result
            })();
            let _ = tx.send(result);
        });
        self.tx
            .send(job)
            .await
            .map_err(|_| WorkspaceError::RecoveryRequired("workspace worker stopped".into()))?;
        rx.await
            .map_err(|_| WorkspaceError::RecoveryRequired("workspace worker stopped".into()))?
    }
    pub async fn info(&self, cancel: CancellationToken) -> WorkspaceResult<WorkspaceInfo> {
        self.call(cancel, |s, _| Ok(s.info())).await
    }
    pub async fn config(&self, cancel: CancellationToken) -> WorkspaceResult<LocalConfig> {
        self.call(cancel, |s, _| Ok(s.config.clone())).await
    }
    pub async fn configure(
        &self,
        patch: LocalConfigPatch,
        cancel: CancellationToken,
    ) -> WorkspaceResult<LocalConfig> {
        self.call(cancel, move |s, _| {
            let mut value = serde_json::to_value(&s.config)?;
            let map = value.as_object_mut().unwrap();
            for (k, v) in patch {
                map.insert(k, v);
            }
            let cfg: LocalConfig = serde_json::from_value(value)?;
            if cfg.context_tokens == 0 || cfg.search_limit == 0 {
                return Err(WorkspaceError::Configuration(
                    "budgets must be positive".into(),
                ));
            }
            files::atomic_write(
                &s.root,
                ".lumen/config.yaml",
                serde_yaml_ng::to_string(&cfg)?.as_bytes(),
            )?;
            s.config = cfg.clone();
            Ok(cfg)
        })
        .await
    }
    pub async fn status(&self, cancel: CancellationToken) -> WorkspaceResult<WorkspaceStatus> {
        let model = self
            .providers
            .embedder
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|e| e.model_id().to_string());
        self.call(cancel, move |s, c| {
            let tree = files::scan(&s.root)?;
            let catalog = Catalog::from_tree(&tree);
            let index = retrieval::refresh(s, &tree, c)?;
            let index = retrieval::model_status(&s.root, index, model.as_deref())?;
            Ok(WorkspaceStatus {
                info: s.info(),
                version: versioning::status(s)?,
                index,
                diagnostics: catalog.diagnostics,
            })
        })
        .await
    }
    pub async fn validate(
        &self,
        scope: ValidationScope,
        cancel: CancellationToken,
    ) -> WorkspaceResult<ValidationReport> {
        self.call(cancel, move |s, _| {
            let tree = match scope {
                ValidationScope::WorkingTree => files::scan(&s.root)?,
                ValidationScope::Index => versioning::index_tree(s)?,
                ValidationScope::Commit(id) => versioning::commit_tree(s, &id)?,
            };
            Ok(Catalog::from_tree(&tree).report())
        })
        .await
    }
    pub async fn recover(&self, cancel: CancellationToken) -> WorkspaceResult<RecoveryReport> {
        self.call(cancel, |s, _| {
            let transactions = files::recover(&s.root)?;
            versioning::recover_git(s)?;
            let sessions = session::recover_segments(&s.root)?;
            Ok(RecoveryReport {
                transactions,
                sessions,
                needs_resolution: versioning::merging(s)?,
            })
        })
        .await
    }
    pub async fn close(&self, cancel: CancellationToken) -> WorkspaceResult<CloseReport> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(CloseReport { closed: true });
        }
        self.maintenance_cancel.cancel();
        let maintenance = self
            .maintenance
            .lock()
            .map_err(|_| WorkspaceError::WorkspaceBusy)?
            .take();
        if let Some(task) = maintenance {
            task.await.map_err(|_| {
                WorkspaceError::RecoveryRequired("maintenance worker panicked".into())
            })?;
        }
        let closed = self.closed.clone();
        self.call(cancel, move |s, _| {
            session::finish_all(s)?;
            closed.store(true, Ordering::Release);
            Ok(())
        })
        .await?;
        let join = self
            .thread
            .lock()
            .map_err(|_| WorkspaceError::WorkspaceBusy)?
            .take();
        if let Some(join) = join {
            tokio::task::spawn_blocking(move || join.join())
                .await
                .map_err(|_| WorkspaceError::RecoveryRequired("worker join failed".into()))?
                .map_err(|_| WorkspaceError::RecoveryRequired("worker panicked".into()))?;
        }
        Ok(CloseReport { closed: true })
    }
}
impl State {
    pub fn info(&self) -> WorkspaceInfo {
        WorkspaceInfo {
            root: self.root.clone(),
            repository_id: self.repository.repository_id.clone(),
            schema_version: 1,
        }
    }
    pub fn emit(&self, kind: WorkspaceEventKind, generation: Option<String>) {
        let _ = self.events.send(WorkspaceEvent {
            operation_id: uuid::Uuid::now_v7().to_string(),
            generation,
            kind,
        });
    }
    pub fn ensure_writable(&self) -> WorkspaceResult<()> {
        if fs::read_dir(files::safe_path(&self.root, ".lumen/transactions")?)?.any(|entry| {
            entry
                .map(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                .unwrap_or(true)
        }) {
            return Err(WorkspaceError::RecoveryRequired(
                "unfinished file transaction; call recover".into(),
            ));
        }
        if self.root.join(".lumen/git-transition.json").exists() {
            return Err(WorkspaceError::RecoveryRequired(
                "interrupted Git update; call recover".into(),
            ));
        }
        if versioning::merging(self)? {
            Err(WorkspaceError::NeedsResolution)
        } else {
            Ok(())
        }
    }
}
pub(crate) fn check_cancel(c: &CancellationToken) -> WorkspaceResult<()> {
    if c.is_cancelled() {
        Err(WorkspaceError::Cancelled)
    } else {
        Ok(())
    }
}
fn prepare(
    root: PathBuf,
    mode: OpenMode,
    cancel: &CancellationToken,
    events: broadcast::Sender<WorkspaceEvent>,
) -> WorkspaceResult<State> {
    check_cancel(cancel)?;
    let new = !matches!(mode, OpenMode::Open);
    if new {
        if root.exists() && fs::read_dir(&root)?.next().is_some() {
            return Err(WorkspaceError::Configuration(
                "destination must be empty".into(),
            ));
        }
        if root
            .parent()
            .and_then(|p| git2::Repository::discover(p).ok())
            .is_some()
        {
            return Err(WorkspaceError::Configuration(
                "nested repositories are not supported".into(),
            ));
        }
        fs::create_dir_all(&root)?;
    }
    if fs::symlink_metadata(&root)?.file_type().is_symlink() {
        return Err(WorkspaceError::InvalidPath(root.display().to_string()));
    }
    let config = match mode {
        OpenMode::Open => None,
        OpenMode::Init(cfg) => {
            versioning::init_repository(&root)?;
            for d in files::DIRECTORIES {
                fs::create_dir_all(root.join(d))?
            }
            let repo = RepositoryConfig {
                schema_version: 1,
                repository_id: uuid::Uuid::now_v7().to_string(),
                default_language: "zh-CN".into(),
            };
            files::atomic_write(
                &root,
                "repository.yaml",
                serde_yaml_ng::to_string(&repo)?.as_bytes(),
            )?;
            files::atomic_write(
                &root,
                ".gitignore",
                b"/.lumen/\n/sessions/**/*.open\n**/.lumen-*.tmp\n",
            )?;
            files::atomic_write(
                &root,
                ".gitattributes",
                b"*.md text eol=lf\n*.yaml text eol=lf\n*.jsonl text eol=lf\n",
            )?;
            Some(cfg)
        }
        OpenMode::Clone(url, cfg, material) => {
            versioning::clone_repository(&root, &url, material, cancel)?;
            Some(cfg)
        }
    };
    git2::Repository::open(&root)?;
    for p in [".lumen", ".lumen/locks", ".lumen/transactions"] {
        fs::create_dir_all(files::safe_path(&root, p)?)?
    }
    let config = match config {
        Some(c) => {
            files::atomic_write(
                &root,
                ".lumen/config.yaml",
                serde_yaml_ng::to_string(&c)?.as_bytes(),
            )?;
            c
        }
        None => {
            let p = files::safe_path(&root, ".lumen/config.yaml")?;
            if p.exists() {
                serde_yaml_ng::from_slice(&fs::read(p)?)?
            } else {
                LocalConfig::default()
            }
        }
    };
    let startup_lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(files::safe_path(&root, ".lumen/locks/workspace.lock")?)?;
    fs2::FileExt::try_lock_exclusive(&startup_lock).map_err(|_| WorkspaceError::WorkspaceBusy)?;
    let repository: RepositoryConfig =
        serde_yaml_ng::from_slice(&fs::read(files::safe_path(&root, "repository.yaml")?)?)?;
    if repository.schema_version != 1 {
        return Err(WorkspaceError::UnsupportedSchema(repository.schema_version));
    }
    let id = uuid::Uuid::parse_str(&repository.repository_id)
        .map_err(|_| WorkspaceError::InvalidFormat("repository_id must be UUIDv7".into()))?;
    if id.get_version_num() != 7 {
        return Err(WorkspaceError::InvalidFormat(
            "repository_id must be UUIDv7".into(),
        ));
    }
    files::recover(&root)?;
    session::recover_segments(&root)?;
    let mut state = State {
        root,
        repository,
        config,
        events,
        segment_locks: Default::default(),
    };
    if new {
        let tree = files::scan(&state.root)?;
        Catalog::from_tree(&tree).require_valid()?;
        if git2::Repository::open(&state.root)?.is_empty()? {
            versioning::commit_local(&mut state, "Initialize knowledge repository", cancel)?;
        }
    }
    versioning::recover_git(&state)?;
    if !versioning::merging(&state)? {
        let tree = files::scan(&state.root)?;
        retrieval::refresh(&mut state, &tree, cancel)?;
    }
    Ok(state)
}
