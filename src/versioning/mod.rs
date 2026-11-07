mod auth;
mod backend;
mod service;
mod types;
use crate::{
    knowledge::*,
    workspace::{
        check_cancel, files, State, WorkspaceError as E, WorkspaceHandle, WorkspaceResult as R,
    },
};
pub use auth::*;
pub use backend::*;
pub use service::VersionService;
pub(crate) use service::{commit_local, commit_tree, index_tree, merging, recover_git, status};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use tokio_util::sync::CancellationToken;
pub use types::*;
/// Configure the embedded transport before starting the application runtime.
///
/// # Safety
/// Must be called before spawning threads or starting any libgit2 operation.
pub unsafe fn configure_transport_timeouts(connect_ms: i32, io_ms: i32) -> R<()> {
    if connect_ms <= 0 || io_ms <= 0 {
        return Err(E::Configuration(
            "transport timeouts must be positive".into(),
        ));
    }
    // SAFETY: the caller guarantees libgit2's global options cannot be accessed concurrently.
    unsafe {
        git2::opts::set_server_connect_timeout_in_milliseconds(connect_ms)?;
        git2::opts::set_server_timeout_in_milliseconds(io_ms)?;
    }
    Ok(())
}
pub(crate) fn init_repository(root: &Path) -> R<()> {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    git2::Repository::init_opts(root, &opts)?;
    Ok(())
}
pub(crate) fn clone_repository(
    root: &Path,
    url: &str,
    credential: CredentialMaterial,
    cancel: &CancellationToken,
) -> R<()> {
    validate_url(url)?;
    let mut fetch = git2::FetchOptions::new();
    fetch.remote_callbacks(auth::callbacks(credential, cancel, url));
    git2::build::RepoBuilder::new()
        .branch("main")
        .fetch_options(fetch)
        .clone(url, root)?;
    Ok(())
}
fn oid(id: &CommitId) -> R<git2::Oid> {
    git2::Oid::from_str(&id.0).map_err(|_| E::InvalidFormat("invalid commit ID".into()))
}
fn repository(s: &State) -> R<git2::Repository> {
    Ok(git2::Repository::open(&s.root)?)
}
fn head(repo: &git2::Repository) -> R<Option<CommitId>> {
    match repo.head() {
        Ok(r) => Ok(r.target().map(|id| CommitId(id.to_string()))),
        Err(e)
            if matches!(
                e.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}
fn tree_files(repo: &git2::Repository, tree: &git2::Tree<'_>) -> R<files::FileTree> {
    fn walk(
        repo: &git2::Repository,
        tree: &git2::Tree<'_>,
        prefix: &str,
        out: &mut files::FileTree,
    ) -> R<()> {
        for entry in tree {
            let name = entry
                .name()
                .map_err(|_| E::InvalidPath("non-UTF8 git path".into()))?;
            let path = format!("{prefix}{name}");
            if entry.kind() == Some(git2::ObjectType::Tree) {
                walk(repo, &repo.find_tree(entry.id())?, &format!("{path}/"), out)?;
            } else {
                if !files::managed(&path) {
                    return Err(E::InvalidPath(format!("unmanaged tracked path: {path}")));
                }
                if entry.filemode() == 0o120000 || entry.kind() != Some(git2::ObjectType::Blob) {
                    return Err(E::InvalidPath(path));
                }
                out.insert(path, repo.find_blob(entry.id())?.content().to_vec());
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(repo, tree, "", &mut out)?;
    Ok(out)
}
pub(crate) fn validate_candidate(s: &State, tree: &files::FileTree, parents: &[CommitId]) -> R<()> {
    Catalog::from_tree(tree).require_valid()?;
    let config: crate::workspace::RepositoryConfig = serde_yaml_ng::from_slice(
        tree.get("repository.yaml")
            .ok_or_else(|| E::InvalidFormat("missing repository.yaml".into()))?,
    )?;
    if config.repository_id != s.repository.repository_id {
        return Err(E::InvalidFormat("repository identity changed".into()));
    }
    for parent in parents {
        for (path, bytes) in commit_tree(s, parent)? {
            if (path.starts_with("records/")
                || path.starts_with("sessions/") && path.ends_with(".jsonl"))
                && tree.get(&path) != Some(&bytes)
            {
                return Err(E::InvalidFormat(format!(
                    "immutable history changed: {path}"
                )));
            }
        }
    }
    Ok(())
}
