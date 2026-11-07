use super::{AgentMessage, AssistantMessage, ToolCall, ToolResultMessage};

#[derive(Clone, Debug)]
pub enum AgentEvent {
    SessionActivated(Box<crate::session::SessionView>),
    SessionEvent {
        identity: crate::session::SessionIdentity,
        run_id: String,
        activation: u64,
        event: Box<AgentEvent>,
    },
    MessageCommitted(AgentMessage),
    ContextCompactionStart,
    ContextDegraded {
        reason: String,
    },
    ContextPrepared {
        epoch: u64,
        input_tokens: usize,
        input_budget: usize,
        manifest: String,
    },
    UsageReported(super::TokenUsage),
    CompactionUsageReported(super::TokenUsage),
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
