use super::*;
use async_trait::async_trait;
#[async_trait]
pub trait GitBackend: Send + Sync {
    async fn init(req: crate::workspace::InitRequest, cancel: CancellationToken) -> R<Self>
    where
        Self: Sized;
    async fn clone_repo(req: crate::workspace::CloneRequest, cancel: CancellationToken) -> R<Self>
    where
        Self: Sized;
    async fn read_remote(&self, cancel: CancellationToken) -> R<Option<RemoteInfo>>;
    async fn write_remote(
        &self,
        url: RemoteConfig,
        cancel: CancellationToken,
    ) -> R<Option<RemoteInfo>>;
    async fn status(&self, cancel: CancellationToken) -> R<GitStatus>;
    async fn head(&self, cancel: CancellationToken) -> R<Option<CommitId>>;
    async fn compare(&self, pair: CommitPair, cancel: CancellationToken) -> R<CommitRelation>;
    async fn list_tree(&self, tree: TreeRef, cancel: CancellationToken) -> R<Vec<TreeEntry>>;
    async fn read_blob(&self, blob: BlobRef, cancel: CancellationToken) -> R<Vec<u8>>;
    async fn diff(&self, req: GitDiffRequest, cancel: CancellationToken) -> R<GitDiff>;
    async fn log(
        &self,
        req: GitLogRequest,
        page: PageRequest,
        cancel: CancellationToken,
    ) -> R<Page<CommitInfo>>;
    async fn stage(&self, paths: Vec<String>, cancel: CancellationToken) -> R<()>;
    async fn commit(&self, req: GitCommitRequest, cancel: CancellationToken) -> R<CommitOutcome>;
    async fn fetch(&self, cancel: CancellationToken) -> R<FetchReport>;
    async fn push(&self, cancel: CancellationToken) -> R<PushReport>;
    async fn fast_forward(&self, target: CommitId, cancel: CancellationToken) -> R<()>;
    async fn begin_merge(&self, target: CommitId, cancel: CancellationToken) -> R<MergeState>;
    async fn abort_merge(&self, cancel: CancellationToken) -> R<AbortReport>;
    async fn read_conflicts(&self, cancel: CancellationToken) -> R<Vec<GitConflict>>;
}
pub struct Libgit2Backend {
    workspace: WorkspaceHandle,
}
impl Libgit2Backend {
    pub fn new(workspace: WorkspaceHandle) -> Self {
        Self { workspace }
    }
    pub fn workspace(&self) -> &WorkspaceHandle {
        &self.workspace
    }
}
#[async_trait]
impl GitBackend for Libgit2Backend {
    async fn init(req: crate::workspace::InitRequest, c: CancellationToken) -> R<Self> {
        Ok(Self::new(
            crate::workspace::WorkspaceService::init(req, c).await?,
        ))
    }
    async fn clone_repo(req: crate::workspace::CloneRequest, c: CancellationToken) -> R<Self> {
        Ok(Self::new(
            crate::workspace::WorkspaceService::clone(req, c).await?,
        ))
    }
    async fn read_remote(&self, c: CancellationToken) -> R<Option<RemoteInfo>> {
        self.workspace.versions().remote(c).await
    }
    async fn write_remote(&self, url: RemoteConfig, c: CancellationToken) -> R<Option<RemoteInfo>> {
        self.workspace.versions().set_remote(url, c).await
    }
    async fn status(&self, c: CancellationToken) -> R<GitStatus> {
        self.workspace.versions().status(c).await
    }
    async fn head(&self, c: CancellationToken) -> R<Option<CommitId>> {
        Ok(self.status(c).await?.head)
    }
    async fn compare(&self, pair: CommitPair, c: CancellationToken) -> R<CommitRelation> {
        self.workspace
            .call(c, move |s, _| {
                let repo = repository(s)?;
                let (a, b) = (oid(&pair.local)?, oid(&pair.other)?);
                let (ahead, behind) = repo.graph_ahead_behind(a, b)?;
                Ok(CommitRelation {
                    ahead,
                    behind,
                    common_ancestor: repo.merge_base(a, b).is_ok(),
                })
            })
            .await
    }
    async fn list_tree(&self, tree: TreeRef, c: CancellationToken) -> R<Vec<TreeEntry>> {
        self.workspace
            .call(c, move |s, _| {
                Ok(service::version_tree(s, &tree)?
                    .into_iter()
                    .map(|(path, bytes)| TreeEntry { path, bytes })
                    .collect())
            })
            .await
    }
    async fn read_blob(&self, blob: BlobRef, c: CancellationToken) -> R<Vec<u8>> {
        self.workspace
            .call(c, move |s, _| {
                files::safe_path(&s.root, &blob.path)?;
                if let Some(stage) = blob.stage {
                    if !(1..=3).contains(&stage) {
                        return Err(E::InvalidFormat("invalid conflict stage".into()));
                    }
                    let repo = repository(s)?;
                    let index = repo.index()?;
                    let entry = index
                        .get_path(Path::new(&blob.path), stage)
                        .ok_or_else(|| E::NotFound(blob.path.clone()))?;
                    return Ok(repo.find_blob(entry.id)?.content().to_vec());
                }
                service::version_tree(s, &blob.version)?
                    .remove(&blob.path)
                    .ok_or(E::NotFound(blob.path))
            })
            .await
    }
    async fn diff(&self, req: GitDiffRequest, c: CancellationToken) -> R<GitDiff> {
        self.workspace.versions().diff(req, c).await
    }
    async fn log(
        &self,
        req: GitLogRequest,
        page: PageRequest,
        c: CancellationToken,
    ) -> R<Page<CommitInfo>> {
        self.workspace.versions().history(req, page, c).await
    }
    async fn stage(&self, paths: Vec<String>, c: CancellationToken) -> R<()> {
        self.workspace
            .call(c, move |s, _| service::stage_paths(s, &paths))
            .await
    }
    async fn commit(&self, req: GitCommitRequest, c: CancellationToken) -> R<CommitOutcome> {
        self.workspace.versions().commit(req, c).await
    }
    async fn fetch(&self, c: CancellationToken) -> R<FetchReport> {
        self.workspace.versions().fetch(c).await
    }
    async fn push(&self, c: CancellationToken) -> R<PushReport> {
        self.workspace.versions().push(c).await
    }
    async fn fast_forward(&self, target: CommitId, c: CancellationToken) -> R<()> {
        self.workspace
            .call(c, move |s, c| {
                s.ensure_writable()?;
                if !service::status(s)?.dirty.is_empty() {
                    return Err(E::DirtyWorktree);
                }
                service::fast_forward(s, &target, c)
            })
            .await
    }
    async fn begin_merge(&self, target: CommitId, c: CancellationToken) -> R<MergeState> {
        self.workspace
            .call(c, move |s, c| {
                s.ensure_writable()?;
                if !service::status(s)?.dirty.is_empty() {
                    return Err(E::DirtyWorktree);
                }
                service::begin_merge(s, &target, c)
            })
            .await
    }
    async fn abort_merge(&self, c: CancellationToken) -> R<AbortReport> {
        self.workspace.versions().abort_merge(c).await
    }
    async fn read_conflicts(&self, c: CancellationToken) -> R<Vec<GitConflict>> {
        Ok(self.workspace.versions().conflicts(c).await?.files)
    }
}
