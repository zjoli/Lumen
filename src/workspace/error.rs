use serde::Serialize;
use thiserror::Error;

pub type WorkspaceResult<T> = Result<T, WorkspaceError>;

#[derive(Debug, Error, Serialize)]
#[serde(tag = "code", content = "message")]
pub enum WorkspaceError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("ambiguous: {0}")]
    Ambiguous(String),
    #[error("invalid format: {0}")]
    InvalidFormat(String),
    #[error("unsupported schema: {0}")]
    UnsupportedSchema(u32),
    #[error("invalid relation: {0}")]
    InvalidRelation(String),
    #[error("duplicate ID: {0}")]
    DuplicateId(String),
    #[error("revision conflict: {0}")]
    RevisionConflict(String),
    #[error("object is referenced: {0}")]
    Referenced(String),
    #[error("asset exceeds 10 MiB")]
    AssetTooLarge,
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("workspace busy")]
    WorkspaceBusy,
    #[error("workspace has uncommitted changes")]
    DirtyWorktree,
    #[error("merge requires resolution")]
    NeedsResolution,
    #[error("cursor expired")]
    CursorExpired,
    #[error("capability unavailable: {0}")]
    CapabilityUnavailable(String),
    #[error("index not ready")]
    IndexNotReady,
    #[error("versioning error: {0}")]
    Versioning(String),
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("authentication required")]
    AuthRequired,
    #[error("remote advanced; synchronize again")]
    RemoteAdvanced,
    #[error("network operation failed")]
    Network,
    #[error("cancelled")]
    Cancelled,
    #[error("recovery required: {0}")]
    RecoveryRequired(String),
    #[error("I/O error: {0}")]
    Io(String),
}
impl From<std::io::Error> for WorkspaceError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}
impl From<serde_json::Error> for WorkspaceError {
    fn from(e: serde_json::Error) -> Self {
        Self::InvalidFormat(e.to_string())
    }
}
impl From<serde_yaml_ng::Error> for WorkspaceError {
    fn from(e: serde_yaml_ng::Error) -> Self {
        Self::InvalidFormat(e.to_string())
    }
}
impl From<rusqlite::Error> for WorkspaceError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Io(e.to_string())
    }
}
impl From<git2::Error> for WorkspaceError {
    fn from(e: git2::Error) -> Self {
        match e.code() {
            git2::ErrorCode::Auth => Self::AuthRequired,
            git2::ErrorCode::NotFastForward => Self::RemoteAdvanced,
            git2::ErrorCode::Locked => Self::WorkspaceBusy,
            git2::ErrorCode::User => Self::Cancelled,
            _ => Self::Versioning(format!("{:?}/{:?}", e.class(), e.code())),
        }
    }
}
