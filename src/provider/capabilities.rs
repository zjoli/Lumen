use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolKind {
    #[default]
    Mock,
    OpenaiChat,
    OpenaiResponses,
    AnthropicMessages,
}
impl ProtocolKind {
    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::OpenaiChat => "chat/completions",
            Self::OpenaiResponses => "responses",
            Self::AnthropicMessages => "messages",
            Self::Mock => "",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CacheIntent {
    Disabled,
    #[default]
    ProviderDefault,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Named(String),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelCapabilities {
    pub context_tokens: usize,
    pub max_output_tokens: usize,
    #[serde(default = "streaming_default")]
    pub streaming: bool,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub tool_choice: Vec<String>,
    #[serde(default)]
    pub temperature: bool,
    #[serde(default)]
    pub cache_modes: Vec<CacheIntent>,
    #[serde(default = "count_method")]
    pub token_count_method: String,
}
fn count_method() -> String {
    "byte_estimate".into()
}
fn streaming_default() -> bool {
    true
}
impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            context_tokens: 128_000,
            max_output_tokens: 4096,
            streaming: true,
            tools: true,
            tool_choice: vec![
                "auto".into(),
                "none".into(),
                "required".into(),
                "named".into(),
            ],
            temperature: false,
            cache_modes: vec![CacheIntent::Disabled, CacheIntent::ProviderDefault],
            token_count_method: count_method(),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GenerationOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheIntent>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EffectiveGenerationOptions {
    pub max_output_tokens: usize,
    pub temperature: Option<f64>,
    pub cache: CacheIntent,
}
impl Default for EffectiveGenerationOptions {
    fn default() -> Self {
        Self {
            max_output_tokens: 4096,
            temperature: None,
            cache: CacheIntent::ProviderDefault,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LegacyFormat {
    Openai,
    Anthropic,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelSelection {
    Reference {
        model_ref: String,
        #[serde(default)]
        overrides: GenerationOverrides,
    },
    Legacy {
        profile_ref: String,
        model: String,
        format: LegacyFormat,
    },
    Mock,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ResolvedModelDescriptor {
    pub model_ref: Option<String>,
    pub provider_ref: String,
    pub protocol: ProtocolKind,
    pub model_id: String,
    pub capabilities: ModelCapabilities,
    pub generation: EffectiveGenerationOptions,
    pub configuration_key: String,
    pub compatibility_key: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenEstimate {
    pub tokens: usize,
    pub method: String,
    pub exact: bool,
}
