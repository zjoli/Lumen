use super::{AgentError, AgentMessage, AssistantMessage, ToolCall};
use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::{pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRequest {
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<ToolDeclaration>,
    #[serde(default = "default_output_tokens")]
    pub max_output_tokens: usize,
}
fn default_output_tokens() -> usize {
    4096
}

#[derive(Clone, Debug)]
pub struct ModelCapabilities {
    pub context_tokens: usize,
    pub max_output_tokens: usize,
}
impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            context_tokens: 128_000,
            max_output_tokens: 4096,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}

/// Immutable encoded request. The byte count is a conservative estimate, not a tokenizer.
pub struct PreparedRequest {
    pub(crate) request: ModelRequest,
    pub(crate) payload: Option<serde_json::Value>,
    pub(crate) provider_key: String,
    pub(crate) input_tokens: usize,
}
impl PreparedRequest {
    pub fn request(&self) -> &ModelRequest {
        &self.request
    }
    pub fn estimated_input_tokens(&self) -> usize {
        self.input_tokens
    }
    pub fn fingerprint(&self) -> String {
        let body = serde_json::to_vec(&(&self.provider_key, &self.request, &self.payload)).unwrap();
        blake3::hash(&body).to_hex().to_string()
    }
}

pub fn estimate_tokens(text: &str) -> usize {
    text.len()
}

#[derive(Clone, Debug)]
pub enum ModelEvent {
    Start,
    TextDelta(String),
    ToolCall(ToolCall),
    Complete(AssistantMessage),
    Error(String),
    Usage(TokenUsage),
}
pub type ModelEventStream = Pin<Box<dyn Stream<Item = ModelEvent> + Send>>;

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn supports_summary(&self) -> bool {
        true
    }
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities::default()
    }
    fn configuration_key(&self) -> String {
        std::any::type_name::<Self>().into()
    }
    fn prepare(&self, request: ModelRequest) -> Result<PreparedRequest, AgentError> {
        let input_tokens = serde_json::to_vec(&request)
            .map_err(|e| AgentError::Model(e.to_string()))?
            .len()
            + 64;
        Ok(PreparedRequest {
            request,
            payload: None,
            provider_key: self.configuration_key(),
            input_tokens,
        })
    }
    async fn stream_prepared(
        &self,
        request: PreparedRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        if request.provider_key != self.configuration_key() {
            return Err(AgentError::Model(
                "prepared request provider mismatch".into(),
            ));
        }
        self.stream(request.request, cancel).await
    }
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
    fn supports_summary(&self) -> bool {
        false
    }
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}
