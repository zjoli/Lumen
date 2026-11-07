use crate::agent::TokenUsage;
use serde_json::Value;
pub(crate) fn openai(u: &Value, chat: bool) -> TokenUsage {
    let (input, output, detail) = if chat {
        (
            "prompt_tokens",
            "completion_tokens",
            "prompt_tokens_details",
        )
    } else {
        ("input_tokens", "output_tokens", "input_tokens_details")
    };
    TokenUsage {
        input_tokens: u[input].as_u64(),
        output_tokens: u[output].as_u64(),
        cached_tokens: u[detail]["cached_tokens"].as_u64(),
        cache_write_tokens: None,
        reasoning_tokens: u[if chat {
            "completion_tokens_details"
        } else {
            "output_tokens_details"
        }]["reasoning_tokens"]
            .as_u64(),
    }
}
pub(crate) fn anthropic(u: &Value) -> TokenUsage {
    TokenUsage {
        input_tokens: u["input_tokens"].as_u64().map(|n| {
            n.saturating_add(u["cache_read_input_tokens"].as_u64().unwrap_or(0))
                .saturating_add(u["cache_creation_input_tokens"].as_u64().unwrap_or(0))
        }),
        output_tokens: u["output_tokens"].as_u64(),
        cached_tokens: u["cache_read_input_tokens"].as_u64(),
        cache_write_tokens: u["cache_creation_input_tokens"].as_u64(),
        reasoning_tokens: None,
    }
}
pub(crate) fn merge(to: &mut TokenUsage, from: TokenUsage) {
    if from.input_tokens.is_some() {
        to.input_tokens = from.input_tokens;
    }
    if from.output_tokens.is_some() {
        to.output_tokens = from.output_tokens;
    }
    if from.cached_tokens.is_some() {
        to.cached_tokens = from.cached_tokens;
    }
    if from.cache_write_tokens.is_some() {
        to.cache_write_tokens = from.cache_write_tokens;
    }
    if from.reasoning_tokens.is_some() {
        to.reasoning_tokens = from.reasoning_tokens;
    }
}
