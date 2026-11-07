use super::{AgentMessage, AssistantMessage};
pub use crate::provider::{
    CacheIntent, MockProvider, ModelCapabilities, ProviderError, ToolChoice,
};
use crate::provider::{ResolvedModelDescriptor, TokenEstimate};
use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::{
    pin::Pin,
    sync::{Arc, OnceLock},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRequest {
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<ToolDeclaration>,
    #[serde(default = "default_output_tokens")]
    pub max_output_tokens: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_intent: Option<CacheIntent>,
}
fn default_output_tokens() -> usize {
    4096
}
impl Default for ModelRequest {
    fn default() -> Self {
        Self {
            messages: vec![],
            tools: vec![],
            max_output_tokens: 4096,
            tool_choice: None,
            cache_intent: None,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}
/// Final encoded body; no mutation or re-encoding is permitted after preparation.
pub struct PreparedRequest {
    pub(crate) request: ModelRequest,
    pub(crate) body: Arc<[u8]>,
    pub(crate) semantic_headers: Vec<(String, String)>,
    pub(crate) provider_key: String,
    pub(crate) instance_id: Option<String>,
    pub(crate) estimate: TokenEstimate,
}
impl PreparedRequest {
    pub fn request(&self) -> &ModelRequest {
        &self.request
    }
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    pub fn estimate(&self) -> &TokenEstimate {
        &self.estimate
    }
    pub fn estimated_input_tokens(&self) -> usize {
        self.estimate.tokens
    }
    pub fn fingerprint(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(self.provider_key.as_bytes());
        h.update(&self.body);
        h.update(&serde_json::to_vec(&self.semantic_headers).unwrap());
        h.finalize().to_hex().to_string()
    }
}
pub fn estimate_tokens(text: &str) -> usize {
    text.len()
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelFinishReason {
    Stop,
    ToolCalls,
    OutputLimit,
    Refusal,
    Other(String),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModelCompletion {
    pub finish_reason: ModelFinishReason,
    pub request_id: Option<String>,
    pub response_id: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ModelResponse {
    pub message: AssistantMessage,
    pub finish_reason: ModelFinishReason,
    pub usage: TokenUsage,
    pub request_id: Option<String>,
    pub response_id: Option<String>,
    pub metrics: ModelMetrics,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelMetrics {
    pub first_content_ms: Option<u64>,
    pub elapsed_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRequestReport {
    #[serde(default)]
    pub summary: bool,
    pub configuration_key: String,
    pub model_id: String,
    pub request_id: Option<String>,
    pub response_id: Option<String>,
    pub finish_reason: ModelFinishReason,
    pub usage: TokenUsage,
    pub metrics: ModelMetrics,
}
impl ModelResponse {
    pub fn report(&self, descriptor: &ResolvedModelDescriptor) -> ModelRequestReport {
        ModelRequestReport {
            summary: false,
            configuration_key: descriptor.configuration_key.clone(),
            model_id: descriptor.model_id.clone(),
            request_id: self.request_id.clone(),
            response_id: self.response_id.clone(),
            finish_reason: self.finish_reason.clone(),
            usage: self.usage.clone(),
            metrics: self.metrics.clone(),
        }
    }
}
impl From<AssistantMessage> for ModelResponse {
    fn from(message: AssistantMessage) -> Self {
        let finish_reason = if message.tool_calls.is_empty() {
            ModelFinishReason::Stop
        } else {
            ModelFinishReason::ToolCalls
        };
        Self {
            message,
            finish_reason,
            usage: Default::default(),
            request_id: None,
            response_id: None,
            metrics: Default::default(),
        }
    }
}
#[derive(Clone, Debug)]
pub enum ModelEvent {
    Started,
    TextDelta(String),
    ToolCallStarted {
        key: String,
        id: String,
        name: String,
    },
    ToolArgumentsDelta {
        key: String,
        delta: String,
    },
    Completed(Box<ModelResponse>),
}
impl ModelEvent {
    pub fn completed(response: impl Into<ModelResponse>) -> Self {
        Self::Completed(Box::new(response.into()))
    }
}
pub type ModelEventStream = Pin<Box<dyn Stream<Item = Result<ModelEvent, ProviderError>> + Send>>;
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn descriptor(&self) -> &ResolvedModelDescriptor {
        static DESCRIPTOR: OnceLock<ResolvedModelDescriptor> = OnceLock::new();
        DESCRIPTOR.get_or_init(ResolvedModelDescriptor::default)
    }
    fn supports_summary(&self) -> bool {
        true
    }
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities::default()
    }
    fn configuration_key(&self) -> String {
        std::any::type_name::<Self>().into()
    }
    fn prepare(&self, request: ModelRequest) -> Result<PreparedRequest, ProviderError> {
        let body: Arc<[u8]> = serde_json::to_vec(&request)
            .map_err(|_| ProviderError::protocol("request serialization failed"))?
            .into();
        let tokens = body.len() + 64;
        Ok(PreparedRequest {
            request,
            body,
            semantic_headers: vec![],
            provider_key: self.configuration_key(),
            instance_id: None,
            estimate: TokenEstimate {
                tokens,
                method: "byte_estimate".into(),
                exact: false,
            },
        })
    }
    async fn stream_prepared(
        &self,
        request: PreparedRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, ProviderError> {
        if request.provider_key != self.configuration_key() {
            return Err(ProviderError::protocol(
                "prepared request provider mismatch",
            ));
        }
        self.stream(request.request, cancel).await
    }
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, ProviderError>;
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}
