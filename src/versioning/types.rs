use crate::{knowledge::*, workspace::LocalConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitId(pub String);
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Version {
    WorkingTree,
    Index,
    #[default]
    Head,
    Commit(CommitId),
}
pub type TreeRef = Version;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobRef {
    pub version: Version,
    pub path: String,
    pub stage: Option<i32>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitPair {
    pub local: CommitId,
    pub other: CommitId,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitRelation {
    pub ahead: usize,
    pub behind: usize,
    pub common_ancestor: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteInfo {
    pub url: String,
}
pub type RemoteConfig = Option<String>;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionStatus {
    pub head: Option<CommitId>,
    pub dirty: Vec<String>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub merging: bool,
    pub remote: Option<RemoteInfo>,
    pub last_fetch: Option<String>,
}
pub type GitStatus = VersionStatus;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiffRequest {
    pub from: Version,
    pub to: Version,
    pub object_id: Option<ObjectId>,
}
impl Default for DiffRequest {
    fn default() -> Self {
        Self {
            from: Version::Head,
            to: Version::WorkingTree,
            object_id: None,
        }
    }
}
pub type GitDiffRequest = DiffRequest;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileDelta {
    pub path: String,
    #[serde(skip_serializing, default)]
    pub old: Option<Vec<u8>>,
    #[serde(skip_serializing, default)]
    pub new: Option<Vec<u8>>,
    pub patch: String,
    pub binary: bool,
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffResult {
    pub files: Vec<FileDelta>,
}
pub type GitDiff = DiffResult;
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct HistoryQuery {
    pub object_id: Option<ObjectId>,
}
pub type GitLogRequest = HistoryQuery;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitInfo {
    pub id: CommitId,
    pub parents: Vec<CommitId>,
    pub message: String,
    pub author: String,
    pub timestamp: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitRequest {
    pub message: String,
}
pub type GitCommitRequest = CommitRequest;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "commit", rename_all = "snake_case")]
pub enum CommitOutcome {
    Created(CommitId),
    NoChanges,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FetchReport {
    pub remote_head: Option<CommitId>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullReport {
    pub head: CommitId,
    pub merged: bool,
    pub needs_resolution: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushReport {
    pub head: CommitId,
    pub pushed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SyncRequest {
    pub message: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncReport {
    pub local_commit: Option<CommitId>,
    pub fetch: Option<FetchReport>,
    pub merge: Option<PullReport>,
    pub push: Option<PushReport>,
    pub pending_push: bool,
    pub remote_state: String,
    pub index_status: Option<crate::retrieval::IndexStatus>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitConflict {
    pub path: String,
    pub base: Option<Vec<u8>>,
    pub ours: Option<Vec<u8>>,
    pub theirs: Option<Vec<u8>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConflictReport {
    pub files: Vec<GitConflict>,
    pub diagnostics: Vec<crate::workspace::Diagnostic>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolveRequest {
    pub path: String,
    pub content: Option<Vec<u8>>,
}
pub type ResolutionReceipt = ChangeReceipt;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbortReport {
    pub backup: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestoreItem {
    pub id: ObjectId,
    pub expected_revision: ExpectedRevision,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestoreRequest {
    pub commit: CommitId,
    pub items: Vec<RestoreItem>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthRequest {
    pub url: String,
    pub config: LocalConfig,
}
#[derive(Clone, Default)]
pub enum CredentialMaterial {
    #[default]
    Anonymous,
    Https {
        username: String,
        token: String,
    },
    Ssh {
        username: String,
        private_key: PathBuf,
        passphrase: Option<String>,
    },
    Agent {
        username: String,
    },
}
pub type MergeState = PullReport;
