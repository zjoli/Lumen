use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ContentPart {
    Text(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AssistantMessage {
    pub content: Vec<ContentPart>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolResultMessage {
    #[serde(default)]
    pub terminate: bool,
    pub tool_call_id: String,
    pub content: Vec<ContentPart>,
    pub is_error: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum AgentMessage {
    System(String),
    User(Vec<ContentPart>),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    Custom { kind: String, content: Value },
}

#[derive(Clone, Debug, PartialEq)]
pub enum StopReason {
    Completed,
    Aborted,
    ModelError,
    ToolError,
    PluginRequested,
    MaxTurns,
}
