use crate::{
    agent::{AgentMessage, StopReason, TokenUsage},
    knowledge::ObjectId,
    workspace::{WorkspaceError as E, WorkspaceResult as R},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionIdentity {
    pub store_id: String,
    pub session_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionModel {
    pub profile_ref: String,
    pub model: String,
    pub format: String,
}
impl Default for SessionModel {
    fn default() -> Self {
        Self {
            profile_ref: "default".into(),
            model: "mock".into(),
            format: "openai".into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionContext {
    pub context_tokens: usize,
    pub output_tokens: usize,
}
impl Default for SessionContext {
    fn default() -> Self {
        Self {
            context_tokens: 128_000,
            output_tokens: 4096,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMetadata {
    pub schema_version: u32,
    pub id: String,
    pub created_at: String,
    pub title: String,
    pub title_source: String,
    pub archived: bool,
    #[serde(default)]
    pub targets: Vec<ObjectId>,
    #[serde(default)]
    pub model: SessionModel,
    #[serde(default)]
    pub context: SessionContext,
}
impl SessionMetadata {
    pub fn new(title: Option<String>, model: SessionModel, context: SessionContext) -> Self {
        Self {
            schema_version: 1,
            id: format!("session_{}", uuid::Uuid::now_v7()),
            created_at: now(),
            title: title.clone().unwrap_or_else(|| "新会话".into()),
            title_source: if title.is_some() {
                "user"
            } else {
                "first_message"
            }
            .into(),
            archived: false,
            targets: vec![],
            model,
            context,
        }
    }
    pub fn validate(&self) -> R<()> {
        crate::workspace::validate_typed_id(&self.id, "session")?;
        if self.schema_version != 1 {
            return Err(E::UnsupportedSchema(self.schema_version));
        }
        if self.title.trim().is_empty()
            || self.title.chars().count() > 200
            || self.model.profile_ref.is_empty()
            || self.model.model.is_empty()
            || !matches!(self.model.format.as_str(), "openai" | "anthropic")
            || self.context.context_tokens == 0
            || self.context.output_tokens == 0
            || self.context.output_tokens >= self.context.context_tokens
            || !matches!(
                self.title_source.as_str(),
                "user" | "first_message" | "imported"
            )
        {
            return Err(E::InvalidFormat("invalid session metadata".into()));
        }
        chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map_err(|e| E::InvalidFormat(e.to_string()))?;
        Ok(())
    }
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionHealth {
    Complete,
    Interrupted,
    Corrupt,
    Diverged,
    Unsupported,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionPhase {
    Ready,
    Running,
    Cancelling,
    Faulted,
    Closed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManagedSessionInfo {
    pub identity: SessionIdentity,
    pub metadata: SessionMetadata,
    pub revision: String,
    pub updated_at: String,
    pub message_count: usize,
    pub health: SessionHealth,
    pub diagnostics: Vec<String>,
    pub last_stop: Option<StopReason>,
    pub legacy: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionQuery {
    pub query: String,
    pub include_archived: bool,
    pub limit: usize,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct SessionPage {
    pub items: Vec<ManagedSessionInfo>,
    pub next_cursor: Option<String>,
    pub generation: String,
    pub query: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionUiState {
    pub draft: String,
    pub chat_scroll: usize,
    pub reading: Option<ReadingPosition>,
    pub reference: Option<ReferenceState>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadingPosition {
    pub id: ObjectId,
    pub revision: String,
    pub scroll: usize,
    pub heading: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceState {
    pub id: ObjectId,
    pub title: String,
    pub heading: Option<String>,
    pub selection: Option<(String, String)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordRef {
    pub segment_id: String,
    pub line: usize,
    pub hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub record_ref: RecordRef,
    pub role: String,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryPage {
    pub messages: Vec<HistoryMessage>,
    pub before: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionView {
    pub info: ManagedSessionInfo,
    pub history: HistoryPage,
    pub ui: SessionUiState,
    pub writable: bool,
    pub activation: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegmentParent {
    pub segment_id: String,
    pub hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegmentOpen {
    pub schema_version: u32,
    pub session_id: String,
    pub run_id: String,
    pub segment_id: String,
    pub parent: Option<SegmentParent>,
    #[serde(default)]
    pub legacy_prefix: Vec<SegmentParent>,
    pub config_hash: String,
    #[serde(default)]
    pub model: Option<SessionModel>,
    #[serde(default)]
    pub context: Option<SessionContext>,
    pub started_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunEnd {
    pub schema_version: u32,
    pub run_id: String,
    pub reason: StopReason,
    pub error: Option<String>,
    pub ended_at: String,
    #[serde(default)]
    pub message_range: Option<[usize; 2]>,
    #[serde(default)]
    pub error_kind: Option<String>,
    #[serde(default)]
    pub usage: Vec<TokenUsage>,
    #[serde(default)]
    pub compaction_usage: Vec<TokenUsage>,
}
#[derive(Clone, Debug)]
pub struct StoredSegment {
    pub id: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct ReplayReport {
    pub records: Vec<AgentMessage>,
    pub history: Vec<HistoryMessage>,
    pub health: SessionHealth,
    pub diagnostics: Vec<String>,
    pub head: Option<SegmentParent>,
    pub legacy_prefix: Vec<SegmentParent>,
    pub last_stop: Option<StopReason>,
    pub updated_at: Option<String>,
}
