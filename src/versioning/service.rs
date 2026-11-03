use super::*;
use crate::workspace::WorkspaceEventKind;
#[derive(Clone)]
pub struct VersionService(pub(crate) WorkspaceHandle);
pub(crate) fn merging(s: &State) -> R<bool> {
    Ok(repository(s)?.state() == git2::RepositoryState::Merge
        || s.root.join(".lumen/merge-base.json").exists())
}
pub(crate) fn status(s: &State) -> R<VersionStatus> {
    let repo = repository(s)?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let dirty = repo
        .statuses(Some(&mut opts))?
        .iter()
        .filter_map(|e| e.path().ok().map(str::to_owned))
        .collect();
    let head = head(&repo)?;
    let remote = remote(s)?;
    let last_fetch = fs::read_to_string(s.root.join(".lumen/last-fetch")).ok();
    let counts = if last_fetch.is_some() {
        head.as_ref().and_then(|h| {
            repo.find_reference("refs/remotes/origin/main")
                .ok()
                .and_then(|r| r.target())
                .and_then(|other| repo.graph_ahead_behind(oid(h).ok()?, other).ok())
        })
    } else {
        None
    };
    Ok(VersionStatus {
        head,
        dirty,
        ahead: counts.map(|c| c.0),
        behind: counts.map(|c| c.1),
        merging: merging(s)?,
        remote,
        last_fetch,
    })
}
pub(crate) fn commit_tree(s: &State, id: &CommitId) -> R<files::FileTree> {
    let repo = repository(s)?;
    let tree = repo.find_commit(oid(id)?)?.tree()?;
    tree_files(&repo, &tree)
}
pub(crate) fn index_tree(s: &State) -> R<files::FileTree> {
    let repo = repository(s)?;
    let index = repo.index()?;
    if index.has_conflicts() {
        return Err(E::NeedsResolution);
    }
    let mut out = BTreeMap::new();
    for entry in index.iter() {
        let path = std::str::from_utf8(&entry.path)
            .map_err(|_| E::InvalidPath("non-UTF8 index path".into()))?
            .to_string();
        if !files::managed(&path) || entry.mode == 0o120000 {
            return Err(E::InvalidPath(path));
        }
        out.insert(path, repo.find_blob(entry.id)?.content().to_vec());
    }
    Ok(out)
}
pub(super) fn version_tree(s: &State, v: &Version) -> R<files::FileTree> {
    match v {
        Version::WorkingTree => files::scan(&s.root),
        Version::Index => index_tree(s),
        Version::Head => {
            let repo = repository(s)?;
            match head(&repo)? {
                Some(id) => commit_tree(s, &id),
                None => Ok(BTreeMap::new()),
            }
        }
        Version::Commit(id) => commit_tree(s, id),
    }
}
pub(super) fn stage_paths(s: &State, paths: &[String]) -> R<()> {
    let repo = repository(s)?;
    let mut index = repo.index()?;
    for path in paths {
        if !files::managed(path) {
            return Err(E::InvalidPath(path.clone()));
        }
        let p = files::safe_path(&s.root, path)?;
        if p.is_file() {
            index.add_path(Path::new(path))?;
        } else {
            match index.remove_path(Path::new(path)) {
                Ok(()) => (),
                Err(e) if e.code() == git2::ErrorCode::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
    }
    index.write()?;
    Ok(())
}
fn stage_managed(s: &State) -> R<()> {
    let repo = repository(s)?;
    let mut paths = files::scan(&s.root)?.into_keys().collect::<BTreeSet<_>>();
    let index = repo.index()?;
    for entry in index.iter() {
        let path =
            String::from_utf8(entry.path).map_err(|_| E::InvalidPath("non UTF-8 path".into()))?;
        if !files::managed(&path) {
            return Err(E::InvalidPath(format!("unmanaged index entry: {path}")));
        }
        paths.insert(path);
    }
    stage_paths(s, &paths.into_iter().collect::<Vec<_>>())
}
fn signature(s: &State) -> R<git2::Signature<'static>> {
    if s.config.author_name.trim().is_empty() || s.config.author_email.trim().is_empty() {
        return Err(E::Configuration(
            "author_name and author_email required".into(),
        ));
    }
    Ok(git2::Signature::now(
        &s.config.author_name,
        &s.config.author_email,
    )?)
}
fn create_commit(
    s: &State,
    message: &str,
    parents: &[CommitId],
    cancel: &CancellationToken,
) -> R<CommitId> {
    if message.trim().is_empty() {
        return Err(E::Configuration("commit message required".into()));
    }
    let repo = repository(s)?;
    let expected = head(&repo)?;
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Err(E::NeedsResolution);
    }
    let candidate = index_tree(s)?;
    validate_candidate(s, &candidate, parents)?;
    check_cancel(cancel)?;
    let tree = repo.find_tree(index.write_tree()?)?;
    let commits = parents
        .iter()
        .map(|id| repo.find_commit(oid(id)?).map_err(E::from))
        .collect::<R<Vec<_>>>()?;
    let refs = commits.iter().collect::<Vec<_>>();
    let sig = signature(s)?;
    let id = repo.commit(None, &sig, &sig, message, &tree, &refs)?;
    if let Some(old) = expected {
        repo.reference_matching("refs/heads/main", id, true, oid(&old)?, "lumen commit")?;
    } else {
        repo.reference("refs/heads/main", id, false, "lumen initial commit")?;
    }
    repo.set_head("refs/heads/main")?;
    Ok(CommitId(id.to_string()))
}
pub(crate) fn commit_local(
    s: &mut State,
    message: &str,
    cancel: &CancellationToken,
) -> R<CommitOutcome> {
    s.ensure_writable()?;
    crate::workspace::session_finish_all(s)?;
    let repo = repository(s)?;
    if repo.state() != git2::RepositoryState::Clean {
        return Err(E::WorkspaceBusy);
    }
    let expected = head(&repo)?;
    stage_managed(s)?;
    let candidate = index_tree(s)?;
    let parents = expected.iter().cloned().collect::<Vec<_>>();
    validate_candidate(s, &candidate, &parents)?;
    if let Some(id) = expected {
        if commit_tree(s, &id)? == candidate {
            return Ok(CommitOutcome::NoChanges);
        }
    }
    let id = create_commit(s, message, &parents, cancel)?;
    crate::retrieval::refresh(s, &candidate, &CancellationToken::new())?;
    Ok(CommitOutcome::Created(id))
}
fn remote(s: &State) -> R<Option<RemoteInfo>> {
    let repo = repository(s)?;
    let remote = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().ok().map(|url| RemoteInfo { url: url.into() }));
    if let Some(info) = &remote {
        validate_url(&info.url)?;
    }
    Ok(remote)
}
fn fetch_remote(
    s: &State,
    material: CredentialMaterial,
    cancel: &CancellationToken,
) -> R<FetchReport> {
    check_cancel(cancel)?;
    let repo = repository(s)?;
    let info = remote(s)?.ok_or_else(|| E::Configuration("origin is not configured".into()))?;
    let mut callbacks = auth::callbacks(material, cancel, &info.url);
    callbacks.update_tips(|_, _, _| true);
    let mut options = git2::FetchOptions::new();
    options.remote_callbacks(callbacks);
    repo.find_remote("origin")?
        .fetch(&[] as &[&str], Some(&mut options), None)?;
    files::atomic_write(
        &s.root,
        ".lumen/last-fetch",
        chrono::Utc::now().to_rfc3339().as_bytes(),
    )?;
    let remote_head = repo
        .find_reference("refs/remotes/origin/main")
        .ok()
        .and_then(|r| r.target())
        .map(|id| CommitId(id.to_string()));
    Ok(FetchReport { remote_head })
}
fn push_remote(
    s: &State,
    material: CredentialMaterial,
    cancel: &CancellationToken,
) -> R<PushReport> {
    s.ensure_writable()?;
    check_cancel(cancel)?;
    let repo = repository(s)?;
    let id = head(&repo)?.ok_or_else(|| E::NotFound("HEAD".into()))?;
    let info = remote(s)?.ok_or_else(|| E::Configuration("origin is not configured".into()))?;
    let rejection = std::sync::Arc::new(std::sync::Mutex::new(None));
    let capture = rejection.clone();
    let mut callbacks = auth::callbacks(material, cancel, &info.url);
    callbacks.push_update_reference(move |_, status| {
        if status.is_some() {
            *capture.lock().unwrap_or_else(|e| e.into_inner()) = Some(true);
        }
        Ok(())
    });
    let mut options = git2::PushOptions::new();
    options.remote_callbacks(callbacks);
    repo.find_remote("origin")?
        .push(&["refs/heads/main:refs/heads/main"], Some(&mut options))?;
    if rejection
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
    {
        return Err(E::RemoteAdvanced);
    }
    Ok(PushReport {
        head: id,
        pushed: true,
    })
}
#[derive(serde::Serialize, serde::Deserialize)]
struct MergeCheckpoint {
    ours: CommitId,
    theirs: CommitId,
}
fn merge_checkpoint(s: &State) -> R<MergeCheckpoint> {
    Ok(serde_json::from_slice(&fs::read(files::safe_path(
        &s.root,
        ".lumen/merge-base.json",
    )?)?)?)
}
fn replace_files(s: &State, target: &files::FileTree) -> R<()> {
    let current = files::scan(&s.root)?;
    let mut changes = BTreeMap::new();
    for (path, bytes) in target {
        if current.get(path) != Some(bytes) {
            changes.insert(path.clone(), Some(bytes.clone()));
        }
    }
    for path in current.keys() {
        if !target.contains_key(path) {
            changes.insert(path.clone(), None);
        }
    }
    files::apply(&s.root, changes)?;
    Ok(())
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Transition {
    before: CommitId,
    after: CommitId,
}
pub(crate) fn recover_git(s: &State) -> R<()> {
    if s.root.join(".lumen/merge-base.json").exists() {
        let checkpoint = merge_checkpoint(s)?;
        let repo = repository(s)?;
        let current =
            head(&repo)?.ok_or_else(|| E::RecoveryRequired("missing merge HEAD".into()))?;
        if current != checkpoint.ours {
            let commit = repo.find_commit(oid(&current)?)?;
            if commit.parent_ids().collect::<Vec<_>>()
                == vec![oid(&checkpoint.ours)?, oid(&checkpoint.theirs)?]
            {
                repo.cleanup_state()?;
                fs::remove_file(s.root.join(".lumen/merge-base.json"))?;
            } else {
                return Err(E::RecoveryRequired("HEAD changed during merge".into()));
            }
        }
    }
    let path = files::safe_path(&s.root, ".lumen/git-transition.json")?;
    if !path.exists() {
        return Ok(());
    }
    let transition: Transition = serde_json::from_slice(&fs::read(&path)?)?;
    let repo = repository(s)?;
    let current = head(&repo)?.ok_or_else(|| E::RecoveryRequired("missing HEAD".into()))?;
    let target = if current == transition.before {
        &transition.before
    } else if current == transition.after {
        &transition.after
    } else {
        return Err(E::RecoveryRequired(
            "HEAD changed during interrupted checkout".into(),
        ));
    };
    let base = commit_tree(s, &transition.before)?;
    let incoming = commit_tree(s, &transition.after)?;
    for (p, b) in files::scan(&s.root)? {
        if base.get(&p) != Some(&b) && incoming.get(&p) != Some(&b) {
            return Err(E::RecoveryRequired(format!("external change: {p}")));
        }
    }
    let tree = commit_tree(s, target)?;
    replace_files(s, &tree)?;
    repo.index()?
        .read_tree(&repo.find_commit(oid(target)?)?.tree()?)?;
    let mut index = repo.index()?;
    index.read_tree(&repo.find_commit(oid(target)?)?.tree()?)?;
    index.write()?;
    fs::remove_file(path)?;
    Ok(())
}
pub(super) fn fast_forward(s: &State, target: &CommitId, cancel: &CancellationToken) -> R<()> {
    check_cancel(cancel)?;
    let repo = repository(s)?;
    let old = head(&repo)?.ok_or_else(|| E::NotFound("HEAD".into()))?;
    if !repo.graph_descendant_of(oid(target)?, oid(&old)?)? {
        return Err(E::RemoteAdvanced);
    }
    let tree = commit_tree(s, target)?;
    validate_candidate(s, &tree, std::slice::from_ref(&old))?;
    files::atomic_write(
        &s.root,
        ".lumen/git-transition.json",
        &serde_json::to_vec(&Transition {
            before: old.clone(),
            after: target.clone(),
        })?,
    )?;
    replace_files(s, &tree)?;
    let mut index = repo.index()?;
    index.read_tree(&repo.find_commit(oid(target)?)?.tree()?)?;
    index.write()?;
    repo.reference_matching(
        "refs/heads/main",
        oid(target)?,
        true,
        oid(&old)?,
        "lumen fast-forward",
    )?;
    fs::remove_file(s.root.join(".lumen/git-transition.json"))?;
    Ok(())
}
pub(super) fn begin_merge(
    s: &State,
    target: &CommitId,
    cancel: &CancellationToken,
) -> R<PullReport> {
    check_cancel(cancel)?;
    let repo = repository(s)?;
    let ours = head(&repo)?.ok_or_else(|| E::NotFound("HEAD".into()))?;
    if repo.merge_base(oid(&ours)?, oid(target)?).is_err() {
        return Err(E::Versioning(
            "unrelated histories; clone the existing repository".into(),
        ));
    }
    files::atomic_write(
        &s.root,
        ".lumen/merge-base.json",
        &serde_json::to_vec(&MergeCheckpoint {
            ours: ours.clone(),
            theirs: target.clone(),
        })?,
    )?;
    let annotation = repo.find_annotated_commit(oid(target)?)?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout
        .safe()
        .allow_conflicts(true)
        .conflict_style_merge(true);
    repo.merge(&[&annotation], None, Some(&mut checkout))?;
    Ok(PullReport {
        head: ours,
        merged: false,
        needs_resolution: true,
    })
}
fn continue_merge(s: &mut State, cancel: &CancellationToken) -> R<CommitId> {
    let checkpoint = merge_checkpoint(s)?;
    let repo = repository(s)?;
    if repo.state() != git2::RepositoryState::Merge {
        return Err(E::RecoveryRequired(
            "merge was interrupted before applying; abort and retry".into(),
        ));
    }
    if head(&repo)?.as_ref() != Some(&checkpoint.ours) {
        return Err(E::RecoveryRequired("merge HEAD changed".into()));
    }
    let tree = index_tree(s)?;
    validate_candidate(
        s,
        &tree,
        &[checkpoint.ours.clone(), checkpoint.theirs.clone()],
    )?;
    let id = create_commit(
        s,
        "Merge synchronized knowledge",
        &[checkpoint.ours, checkpoint.theirs],
        cancel,
    )?;
    repo.cleanup_state()?;
    fs::remove_file(s.root.join(".lumen/merge-base.json"))?;
    crate::retrieval::refresh(s, &tree, &CancellationToken::new())?;
    Ok(id)
}
fn pull_fetched(s: &mut State, fetch: &FetchReport, cancel: &CancellationToken) -> R<PullReport> {
    s.ensure_writable()?;
    if !status(s)?.dirty.is_empty() {
        return Err(E::DirtyWorktree);
    }
    let repo = repository(s)?;
    let local = head(&repo)?.ok_or_else(|| E::NotFound("HEAD".into()))?;
    let other = match &fetch.remote_head {
        None => {
            return Ok(PullReport {
                head: local,
                merged: false,
                needs_resolution: false,
            })
        }
        Some(o) => o,
    };
    if local == *other || repo.graph_descendant_of(oid(&local)?, oid(other)?)? {
        return Ok(PullReport {
            head: local,
            merged: false,
            needs_resolution: false,
        });
    }
    if repo.graph_descendant_of(oid(other)?, oid(&local)?)? {
        fast_forward(s, other, cancel)?;
        let tree = files::scan(&s.root)?;
        crate::retrieval::refresh(s, &tree, &CancellationToken::new())?;
        return Ok(PullReport {
            head: other.clone(),
            merged: false,
            needs_resolution: false,
        });
    }
    begin_merge(s, other, cancel)?;
    if !repo.index()?.has_conflicts() {
        match continue_merge(s, cancel) {
            Ok(head) => {
                return Ok(PullReport {
                    head,
                    merged: true,
                    needs_resolution: false,
                })
            }
            Err(E::Cancelled) => return Err(E::Cancelled),
            Err(_) => (),
        }
    }
    s.emit(WorkspaceEventKind::NeedsResolution, None);
    Ok(PullReport {
        head: local,
        merged: false,
        needs_resolution: true,
    })
}
impl VersionService {
    pub async fn remote(&self, cancel: CancellationToken) -> R<Option<RemoteInfo>> {
        self.0.call(cancel, |s, _| remote(s)).await
    }
    pub async fn set_remote(
        &self,
        url: RemoteConfig,
        cancel: CancellationToken,
    ) -> R<Option<RemoteInfo>> {
        self.0
            .call(cancel, move |s, _| {
                s.ensure_writable()?;
                let repo = repository(s)?;
                if let Some(url) = &url {
                    validate_url(url)?;
                    if repo.find_remote("origin").is_ok() {
                        repo.remote_set_url("origin", url)?;
                    } else {
                        repo.remote("origin", url)?;
                    }
                } else if repo.find_remote("origin").is_ok() {
                    repo.remote_delete("origin")?;
                }
                let _ = fs::remove_file(s.root.join(".lumen/last-fetch"));
                remote(s)
            })
            .await
    }
    pub async fn status(&self, cancel: CancellationToken) -> R<VersionStatus> {
        self.0.call(cancel, |s, _| status(s)).await
    }
    pub async fn diff(&self, req: DiffRequest, cancel: CancellationToken) -> R<DiffResult> {
        self.0
            .call(cancel, move |s, _| {
                let from = version_tree(s, &req.from)?;
                let to = version_tree(s, &req.to)?;
                let paths = from
                    .keys()
                    .chain(to.keys())
                    .cloned()
                    .collect::<BTreeSet<_>>();
                Ok(DiffResult {
                    files: paths
                        .into_iter()
                        .filter(|p| {
                            req.object_id
                                .as_ref()
                                .map(|id| id.path() == *p)
                                .unwrap_or(true)
                        })
                        .filter(|p| from.get(p) != to.get(p))
                        .map(|path| {
                            let old = from.get(&path).cloned();
                            let new = to.get(&path).cloned();
                            let old_bytes = old.as_deref().unwrap_or(&[]);
                            let new_bytes = new.as_deref().unwrap_or(&[]);
                            let binary = [old_bytes, new_bytes].iter().any(|bytes| {
                                bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
                            });
                            let mut diff = git2::Patch::from_buffers(
                                old_bytes,
                                old.as_ref().map(|_| Path::new(&path)),
                                new_bytes,
                                new.as_ref().map(|_| Path::new(&path)),
                                None,
                            )?;
                            let buffer = diff.to_buf()?;
                            let patch = String::from_utf8_lossy(&buffer).into_owned();
                            drop(diff);
                            let truncated = patch.chars().count() > 100_000;
                            Ok(FileDelta {
                                old,
                                new,
                                path,
                                patch: patch.chars().take(100_000).collect(),
                                binary,
                                truncated,
                            })
                        })
                        .collect::<R<Vec<_>>>()?,
                })
            })
            .await
    }
    pub async fn history(
        &self,
        query: HistoryQuery,
        page: PageRequest,
        cancel: CancellationToken,
    ) -> R<Page<CommitInfo>> {
        self.0
            .call(cancel, move |s, cancel| {
                let repo = repository(s)?;
                let head = head(&repo)?.ok_or_else(|| E::NotFound("HEAD".into()))?;
                let mut walk = repo.revwalk()?;
                walk.push(oid(&head)?)?;
                walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
                let mut items = vec![];
                for id in walk {
                    check_cancel(cancel)?;
                    let commit = repo.find_commit(id?)?;
                    if let Some(id) = &query.object_id {
                        let path = id.path();
                        let current = commit
                            .tree()?
                            .get_path(Path::new(&path))
                            .ok()
                            .map(|e| e.id());
                        if commit.parent_count() > 0
                            && commit.parents().all(|p| {
                                p.tree()
                                    .ok()
                                    .and_then(|t| t.get_path(Path::new(&path)).ok().map(|e| e.id()))
                                    == current
                            })
                        {
                            continue;
                        }
                    }
                    items.push(CommitInfo {
                        id: CommitId(commit.id().to_string()),
                        parents: commit
                            .parent_ids()
                            .map(|id| CommitId(id.to_string()))
                            .collect(),
                        message: commit.message().unwrap_or("").into(),
                        author: commit.author().name().unwrap_or("").into(),
                        timestamp: commit.time().seconds(),
                    });
                }
                paginate(&items, page, &head.0)
            })
            .await
    }
    pub async fn read_at(
        &self,
        id: ObjectId,
        commit: CommitId,
        cancel: CancellationToken,
    ) -> R<ObjectView> {
        self.0
            .call(cancel, move |s, _| {
                Catalog::from_tree(&commit_tree(s, &commit)?).view(
                    &id,
                    ReadView::Full,
                    &s.repository.repository_id,
                )
            })
            .await
    }
    pub async fn restore(
        &self,
        req: RestoreRequest,
        cancel: CancellationToken,
    ) -> R<ChangeReceipt> {
        self.0
            .call(cancel, move |s, c| {
                s.ensure_writable()?;
                let source = commit_tree(s, &req.commit)?;
                let mut current = files::scan(&s.root)?;
                let mut changes = BTreeMap::new();
                for item in req.items {
                    let path = item.id.path();
                    crate::knowledge::check_revision(
                        current.get(&path),
                        &item.expected_revision,
                        &path,
                    )?;
                    let bytes = source
                        .get(&path)
                        .ok_or_else(|| E::NotFound(path.clone()))?
                        .clone();
                    let object = format::parse_object(
                        std::str::from_utf8(&bytes).map_err(|_| E::InvalidFormat(path.clone()))?,
                    )?;
                    for asset in object.metadata.attachments {
                        let b = source
                            .get(&asset.path)
                            .ok_or_else(|| E::NotFound(asset.path.clone()))?
                            .clone();
                        if current.get(&asset.path).is_some_and(|old| *old != b) {
                            return Err(E::RevisionConflict(asset.path));
                        }
                        current.insert(asset.path.clone(), b.clone());
                        changes.insert(asset.path, Some(b));
                    }
                    current.insert(path.clone(), bytes.clone());
                    changes.insert(path, Some(bytes));
                }
                Catalog::from_tree(&current).require_valid()?;
                check_cancel(c)?;
                let changed = files::apply(&s.root, changes)?;
                crate::retrieval::refresh(s, &current, &CancellationToken::new())?;
                Ok(ChangeReceipt {
                    changed,
                    generation: files::fingerprint(&current),
                })
            })
            .await
    }
    pub async fn commit(&self, req: CommitRequest, cancel: CancellationToken) -> R<CommitOutcome> {
        self.0
            .call(cancel, move |s, c| commit_local(s, &req.message, c))
            .await
    }
    async fn credential(&self, cancel: CancellationToken) -> R<CredentialMaterial> {
        let url = self
            .remote(cancel.clone())
            .await?
            .ok_or_else(|| E::Configuration("origin is not configured".into()))?
            .url;
        let config = self.0.config(cancel.clone()).await?;
        let provider = self
            .0
            .credentials
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        provider.resolve(AuthRequest { url, config }, cancel).await
    }
    pub async fn fetch(&self, cancel: CancellationToken) -> R<FetchReport> {
        let material = self.credential(cancel.clone()).await?;
        self.0
            .call(cancel, move |s, c| fetch_remote(s, material, c))
            .await
    }
    pub async fn push(&self, cancel: CancellationToken) -> R<PushReport> {
        let material = self.credential(cancel.clone()).await?;
        self.0
            .call(cancel, move |s, c| push_remote(s, material, c))
            .await
    }
    pub async fn pull(&self, cancel: CancellationToken) -> R<PullReport> {
        let material = self.credential(cancel.clone()).await?;
        self.0
            .call(cancel, move |s, c| {
                s.ensure_writable()?;
                if !status(s)?.dirty.is_empty() {
                    return Err(E::DirtyWorktree);
                }
                let fetch = fetch_remote(s, material, c)?;
                pull_fetched(s, &fetch, c)
            })
            .await
    }
    pub async fn sync(&self, req: SyncRequest, cancel: CancellationToken) -> R<SyncReport> {
        let material = self.credential(cancel.clone()).await?;
        self.0
            .call(cancel, move |s, c| {
                s.ensure_writable()?;
                let mut report = SyncReport {
                    local_commit: None,
                    fetch: None,
                    merge: None,
                    push: None,
                    pending_push: true,
                    remote_state: "unknown".into(),
                    index_status: None,
                    error: None,
                };
                s.emit(WorkspaceEventKind::SyncProgress, None);
                let result = (|| -> R<()> {
                    crate::workspace::session_finish_all(s)?;
                    if !status(s)?.dirty.is_empty() {
                        let message = req.message.ok_or_else(|| {
                            E::Configuration("commit message required for local changes".into())
                        })?;
                        if let CommitOutcome::Created(id) = commit_local(s, &message, c)? {
                            report.local_commit = Some(id)
                        }
                    }
                    let fetch = fetch_remote(s, material.clone(), c)?;
                    report.fetch = Some(fetch.clone());
                    report.remote_state = "known".into();
                    let merge = pull_fetched(s, &fetch, c)?;
                    report.merge = Some(merge.clone());
                    if merge.needs_resolution {
                        return Err(E::NeedsResolution);
                    }
                    report.remote_state = "unknown".into();
                    let push = push_remote(s, material, c)?;
                    report.push = Some(push);
                    report.remote_state = "known".into();
                    report.pending_push = false;
                    let tree = files::scan(&s.root)?;
                    report.index_status = Some(crate::retrieval::refresh(
                        s,
                        &tree,
                        &CancellationToken::new(),
                    )?);
                    Ok(())
                })();
                if let Err(e) = result {
                    report.error = Some(e.to_string())
                } else {
                    s.emit(WorkspaceEventKind::SyncCompleted, None)
                }
                Ok(report)
            })
            .await
    }
    pub async fn conflicts(&self, cancel: CancellationToken) -> R<ConflictReport> {
        self.0
            .call(cancel, |s, _| {
                let repo = repository(s)?;
                let index = repo.index()?;
                let mut files = vec![];
                for conflict in index.conflicts()? {
                    let c = conflict?;
                    let path = c
                        .our
                        .as_ref()
                        .or(c.their.as_ref())
                        .or(c.ancestor.as_ref())
                        .ok_or_else(|| E::InvalidFormat("empty conflict".into()))?
                        .path
                        .clone();
                    let read = |entry: Option<git2::IndexEntry>| -> R<Option<Vec<u8>>> {
                        entry
                            .map(|e| {
                                repo.find_blob(e.id)
                                    .map(|b| b.content().to_vec())
                                    .map_err(E::from)
                            })
                            .transpose()
                    };
                    files.push(GitConflict {
                        path: String::from_utf8(path)
                            .map_err(|_| E::InvalidPath("UTF-8".into()))?,
                        base: read(c.ancestor)?,
                        ours: read(c.our)?,
                        theirs: read(c.their)?,
                    });
                }
                let diagnostics = Catalog::from_tree(&files::scan(&s.root)?).diagnostics;
                Ok(ConflictReport { files, diagnostics })
            })
            .await
    }
    pub async fn resolve(
        &self,
        req: ResolveRequest,
        cancel: CancellationToken,
    ) -> R<ResolutionReceipt> {
        self.0
            .call(cancel, move |s, _| {
                if !merging(s)? {
                    return Err(E::Configuration("no merge in progress".into()));
                }
                if !files::managed(&req.path) {
                    return Err(E::InvalidPath(req.path));
                }
                if let Some(bytes) = &req.content {
                    if ObjectKind::ALL
                        .iter()
                        .any(|k| req.path.starts_with(&format!("{}/", k.directory())))
                    {
                        let o = format::parse_object(
                            std::str::from_utf8(bytes)
                                .map_err(|_| E::InvalidFormat("UTF-8".into()))?,
                        )?;
                        if o.metadata.id.path() != req.path {
                            return Err(E::InvalidPath(req.path));
                        }
                    }
                }
                let changed =
                    files::apply(&s.root, BTreeMap::from([(req.path.clone(), req.content)]))?;
                stage_paths(s, &[req.path])?;
                Ok(ChangeReceipt {
                    changed,
                    generation: files::fingerprint(&files::scan(&s.root)?),
                })
            })
            .await
    }
    pub async fn continue_merge(&self, cancel: CancellationToken) -> R<CommitId> {
        self.0.call(cancel, continue_merge).await
    }
    pub async fn abort_merge(&self, cancel: CancellationToken) -> R<AbortReport> {
        self.0
            .call(cancel, |s, _| {
                let checkpoint = merge_checkpoint(s)?;
                let backup = format!(
                    ".lumen/transactions/merge-backup-{}.bak",
                    uuid::Uuid::now_v7()
                );
                files::atomic_write(
                    &s.root,
                    &backup,
                    &serde_json::to_vec(&files::scan(&s.root)?)?,
                )?;
                let baseline = commit_tree(s, &checkpoint.ours)?;
                replace_files(s, &baseline)?;
                let repo = repository(s)?;
                let mut index = repo.index()?;
                index.read_tree(&repo.find_commit(oid(&checkpoint.ours)?)?.tree()?)?;
                index.write()?;
                repo.cleanup_state()?;
                fs::remove_file(s.root.join(".lumen/merge-base.json"))?;
                crate::retrieval::refresh(s, &baseline, &CancellationToken::new())?;
                Ok(AbortReport { backup })
            })
            .await
    }
}
impl crate::workspace::Workspace {
    pub fn set_credential_provider(&self, provider: std::sync::Arc<dyn CredentialProvider>) {
        *self.credentials.write().unwrap_or_else(|e| e.into_inner()) = provider;
    }
}
