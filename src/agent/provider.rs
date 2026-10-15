use super::{AgentError, AgentMessage, AssistantMessage, ToolCall};
use async_trait::async_trait;
use futures_core::Stream;
use std::{pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct ModelRequest {
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<ToolDeclaration>,
}

#[derive(Clone, Debug)]
pub enum ModelEvent {
    Start,
    TextDelta(String),
    ToolCall(ToolCall),
    Complete(AssistantMessage),
    Error(String),
}
pub type ModelEventStream = Pin<Box<dyn Stream<Item = ModelEvent> + Send>>;

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, AgentError>;
}

#[derive(Clone)]
pub struct MockProvider {
    responses: Arc<std::sync::Mutex<Vec<AssistantMessage>>>,
}
impl MockProvider {
    pub fn new(responses: Vec<AssistantMessage>) -> Self {
        Self {
            responses: Arc::new(std::sync::Mutex::new(responses)),
        }
    }
}
#[async_trait]
impl ModelProvider for MockProvider {
    async fn stream(
        &self,
        _request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        if cancel.is_cancelled() {
            return Err(AgentError::Aborted);
        }
        let mut responses = self
            .responses
            .lock()
            .map_err(|_| AgentError::Model("mock lock poisoned".into()))?;
        if responses.is_empty() {
            return Err(AgentError::Model("mock responses exhausted".into()));
        }
        let msg = responses.remove(0);
        let events = vec![ModelEvent::Start, ModelEvent::Complete(msg)];
        Ok(Box::pin(futures_util::stream::iter(events)))
    }
}

#[derive(Clone, Debug)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}
