use super::{AgentError, AgentMessage, AgentState, AssistantMessage, ToolCall, ToolResultMessage};
use async_trait::async_trait;

pub struct PluginContext<'a> {
    pub state: &'a mut AgentState,
    pub events: &'a mut super::EventLog,
}
pub struct BeforeRequest<'a> {
    pub messages: &'a mut Vec<AgentMessage>,
}
pub struct BeforeToolCall<'a> {
    pub assistant: &'a AssistantMessage,
    pub call: &'a mut ToolCall,
}
pub enum ToolDecision {
    Allow,
    Block { reason: String, terminate: bool },
}
pub struct AfterToolCall<'a> {
    pub call: &'a ToolCall,
    pub result: &'a mut ToolResultMessage,
}
pub enum Continuation {
    Default,
    Continue,
    End,
}
pub struct TurnEnd<'a> {
    pub assistant: &'a AssistantMessage,
    pub results: &'a [ToolResultMessage],
}

#[async_trait]
pub trait AgentPlugin: Send + Sync {
    async fn before_agent_start(&self, _ctx: &mut PluginContext<'_>) -> Result<(), AgentError> {
        Ok(())
    }
    async fn on_agent_end(
        &self,
        _reason: &super::StopReason,
        _ctx: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    async fn before_request(
        &self,
        _event: &mut BeforeRequest<'_>,
        _ctx: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    async fn before_tool_call(
        &self,
        _event: &mut BeforeToolCall<'_>,
        _ctx: &mut PluginContext<'_>,
    ) -> Result<ToolDecision, AgentError> {
        Ok(ToolDecision::Allow)
    }
    async fn after_tool_call(
        &self,
        _event: &mut AfterToolCall<'_>,
        _ctx: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    async fn after_turn(
        &self,
        _event: &mut TurnEnd<'_>,
        _ctx: &mut PluginContext<'_>,
    ) -> Result<Continuation, AgentError> {
        Ok(Continuation::Default)
    }
}
