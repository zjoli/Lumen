use super::{AgentMessage, AssistantMessage, ToolCall, ToolResultMessage};

#[derive(Clone, Debug)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        reason: super::StopReason,
    },
    TurnStart,
    TurnEnd {
        assistant: AssistantMessage,
        tool_results: Vec<ToolResultMessage>,
    },
    MessageStart(AgentMessage),
    MessageDelta {
        delta: String,
    },
    MessageEnd(AgentMessage),
    ToolCallStart(ToolCall),
    ToolExecutionStart(ToolCall),
    ToolExecutionEnd(ToolResultMessage),
    ContextCompactionEnd {
        summary: String,
    },
    Error(String),
}
