mod anthropic_messages;
mod openai_chat;
mod openai_responses;
use super::sse::SseEvent;
use super::*;
use crate::agent::*;
use serde_json::{json, Value};
pub(crate) struct EncodedRequest {
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}
pub(crate) trait ProtocolAdapter: Send + Sync {
    fn encode(
        &self,
        d: &ResolvedModelDescriptor,
        r: &ModelRequest,
    ) -> Result<EncodedRequest, ProviderError>;
    fn decoder(&self, d: &ResolvedModelDescriptor) -> Box<dyn ResponseDecoder>;
    fn decode_json(
        &self,
        d: &ResolvedModelDescriptor,
        b: &[u8],
    ) -> Result<ModelResponse, ProviderError>;
}
pub(crate) trait ResponseDecoder: Send {
    fn push(&mut self, event: SseEvent) -> Result<Vec<ModelEvent>, ProviderError>;
    fn finish(&self) -> Result<(), ProviderError>;
    fn usage(&self) -> TokenUsage {
        TokenUsage::default()
    }
}
pub(crate) fn adapter(
    protocol: ProtocolKind,
    t: TransportMode,
    o: ProtocolOptions,
) -> Result<Box<dyn ProtocolAdapter>, ProviderError> {
    Ok(match protocol {
        ProtocolKind::OpenaiChat => Box::new(openai_chat::Chat {
            mode: t,
            options: o,
        }),
        ProtocolKind::OpenaiResponses => Box::new(openai_responses::Responses { mode: t }),
        ProtocolKind::AnthropicMessages => Box::new(anthropic_messages::Messages {
            mode: t,
            options: o,
        }),
        ProtocolKind::Mock => {
            return Err(ProviderError::configuration("mock is not an HTTP protocol"))
        }
    })
}
pub(crate) fn parse(b: &[u8]) -> Result<Value, ProviderError> {
    serde_json::from_slice(b).map_err(|_| ProviderError::protocol("invalid JSON response"))
}
pub(crate) fn required(v: &Value, key: &str) -> Result<String, ProviderError> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::protocol(format!("missing or empty {key}")))
}
pub(crate) fn text(c: &[ContentPart]) -> String {
    c.iter().map(|ContentPart::Text(s)| s.as_str()).collect()
}
pub(crate) fn validate(d: &ResolvedModelDescriptor, r: &ModelRequest) -> Result<(), ProviderError> {
    if r.max_output_tokens == 0 {
        return Err(ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            "output tokens must be positive",
        ));
    }
    if !r.tools.is_empty() && !d.capabilities.tools {
        return Err(ProviderError::unsupported(
            "model does not support this tool workflow",
        ));
    }
    if r.messages
        .iter()
        .any(|m| matches!(m, AgentMessage::Custom { .. }))
    {
        return Err(ProviderError::protocol("unprojected Custom message"));
    }
    crate::context::validate_tool_groups(&r.messages).map_err(|_| {
        ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            "incomplete or malformed tool history",
        )
    })?;
    let mut names = std::collections::BTreeSet::new();
    for t in &r.tools {
        if t.name.is_empty() || !names.insert(&t.name) {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "invalid or duplicate tool declaration",
            ));
        }
    }
    if let Some(c) = &r.tool_choice {
        let kind = match c {
            ToolChoice::Auto => "auto",
            ToolChoice::None => "none",
            ToolChoice::Required => "required",
            ToolChoice::Named(name) => {
                if !r.tools.iter().any(|t| t.name == *name) {
                    return Err(ProviderError::new(
                        ProviderErrorKind::InvalidRequest,
                        "selected tool is not declared",
                    ));
                }
                "named"
            }
        };
        if r.tools.is_empty() && matches!(c, ToolChoice::Required | ToolChoice::Named(_)) {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "required tool choice without tools",
            ));
        }
        if !matches!(c, ToolChoice::None) && !d.capabilities.tool_choice.iter().any(|s| s == kind) {
            return Err(ProviderError::unsupported(
                "tool choice unsupported by model",
            ));
        }
    }
    validate_replay(&r.messages, d)
}
fn generation(body: &mut Value, d: &ResolvedModelDescriptor) {
    if let Some(t) = d.generation.temperature {
        body["temperature"] = json!(t);
    }
}
fn output(d: &ResolvedModelDescriptor, r: &ModelRequest) -> usize {
    r.max_output_tokens
        .min(d.capabilities.max_output_tokens)
        .min(d.generation.max_output_tokens)
}
fn encoded(body: Value, headers: Vec<(String, String)>) -> Result<EncodedRequest, ProviderError> {
    Ok(EncodedRequest {
        body: serde_json::to_vec(&body)
            .map_err(|_| ProviderError::protocol("request encoding failed"))?,
        headers,
    })
}
fn complete(
    d: &ResolvedModelDescriptor,
    mut a: AssistantMessage,
    reason: ModelFinishReason,
    usage: TokenUsage,
    id: Option<String>,
    items: Option<Vec<Value>>,
) -> Result<ModelResponse, ProviderError> {
    match &reason {
        ModelFinishReason::OutputLimit | ModelFinishReason::Refusal => {
            a.tool_calls.clear();
            a.provider_replay = None;
        }
        ModelFinishReason::Stop if !a.tool_calls.is_empty() => {
            return Err(ProviderError::protocol(
                "tool calls without tool finish reason",
            ))
        }
        ModelFinishReason::ToolCalls if a.tool_calls.is_empty() => {
            return Err(ProviderError::protocol("tool finish without calls"))
        }
        ModelFinishReason::Other(_) => {
            return Err(ProviderError::protocol("unsupported model finish reason"))
        }
        _ => (),
    }
    let mut ids = std::collections::BTreeSet::new();
    if a.tool_calls.iter().any(|c| {
        c.id.is_empty() || c.name.is_empty() || !c.arguments.is_object() || !ids.insert(&c.id)
    }) {
        return Err(ProviderError::protocol("invalid or duplicate tool call"));
    }
    if !matches!(
        reason,
        ModelFinishReason::OutputLimit | ModelFinishReason::Refusal
    ) {
        if let Some(items) = items {
            a.provider_replay = Some(ProviderReplay::new(d, &a, items));
        }
    }
    a.completion = Some(ModelCompletion {
        finish_reason: reason.clone(),
        request_id: None,
        response_id: id.clone(),
    });
    Ok(ModelResponse {
        message: a,
        finish_reason: reason,
        usage,
        request_id: None,
        response_id: id,
        metrics: Default::default(),
    })
}
fn check_projection(a: &AssistantMessage, b: &AssistantMessage) -> Result<(), ProviderError> {
    if a.content != b.content || a.tool_calls != b.tool_calls {
        return Err(ProviderError::protocol("native replay projection mismatch"));
    }
    Ok(())
}
fn tool_start(key: String, id: String, name: String) -> ModelEvent {
    ModelEvent::ToolCallStarted { key, id, name }
}
