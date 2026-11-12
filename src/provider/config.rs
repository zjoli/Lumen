use super::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub schema_version: u32,
    pub default_model: String,
    pub providers: BTreeMap<String, ProviderConnection>,
    pub models: BTreeMap<String, ModelDefinition>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConnection {
    pub protocol: ProtocolKind,
    pub base_url: String,
    pub state_scope: String,
    pub auth: AuthConfig,
    #[serde(default)]
    pub transport: TransportConfig,
    #[serde(default)]
    pub protocol_options: ProtocolOptions,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthConfig {
    Env { variable: String },
    Local { credential: String },
    None,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    #[default]
    Sse,
    Json,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TransportConfig {
    pub mode: TransportMode,
    pub connect_timeout_ms: u64,
    pub first_byte_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub request_timeout_ms: u64,
}
impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            mode: TransportMode::Sse,
            connect_timeout_ms: 10_000,
            first_byte_timeout_ms: 60_000,
            idle_timeout_ms: 30_000,
            request_timeout_ms: 120_000,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ProtocolOptions {
    pub output_token_field: Option<OutputTokenField>,
    pub stream_usage: Option<bool>,
    pub cache_control: Option<bool>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputTokenField {
    MaxTokens,
    MaxCompletionTokens,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDefinition {
    pub provider: String,
    pub model_id: String,
    pub capabilities: ModelCapabilities,
    #[serde(default)]
    pub defaults: GenerationOverrides,
}
impl ProviderConfig {
    pub fn load(root: &Path) -> Result<Option<Self>, ProviderError> {
        let path = crate::workspace::files::safe_path(root, ".lumen/providers.yaml")
            .map_err(|_| ProviderError::configuration("unsafe provider config path"))?;
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ProviderError::configuration("cannot read providers.yaml")),
        };
        // Never forward parser diagnostics containing source lines or credential-bearing URLs.
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&bytes).map_err(|_| {
            ProviderError::configuration(
                "invalid providers.yaml (duplicate keys or malformed YAML)",
            )
        })?;
        let config: Self = serde_yaml_ng::from_value(value).map_err(|_| {
            ProviderError::configuration(
                "invalid providers.yaml schema (unknown/duplicate fields, values or keys)",
            )
        })?;
        config.validate()?;
        Ok(Some(config))
    }
    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.schema_version != 1 {
            return Err(ProviderError::configuration(
                "unsupported provider config version",
            ));
        }
        if !(self.models.contains_key(&self.default_model)
            || self.models.is_empty() && self.default_model.is_empty())
        {
            return Err(ProviderError::configuration(
                "default_model must reference a configured model",
            ));
        }
        for (id, p) in &self.providers {
            if id.trim().is_empty()
                || id == "mock"
                || p.state_scope.trim().is_empty()
                || p.protocol == ProtocolKind::Mock
            {
                return Err(ProviderError::configuration(
                    "invalid provider identity, protocol or state_scope",
                ));
            }
            let url = reqwest::Url::parse(&p.base_url)
                .map_err(|_| ProviderError::configuration("invalid provider base_url"))?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(ProviderError::configuration(
                    "base_url must be HTTP(S), without userinfo, query or fragment",
                ));
            }
            if [
                p.transport.connect_timeout_ms,
                p.transport.first_byte_timeout_ms,
                p.transport.idle_timeout_ms,
                p.transport.request_timeout_ms,
            ]
            .iter()
            .any(|n| *n == 0 || *n > 86_400_000)
            {
                return Err(ProviderError::configuration(
                    "timeouts must be between 1ms and 24 hours",
                ));
            }
            if p.protocol != ProtocolKind::OpenaiChat
                && (p.protocol_options.output_token_field.is_some()
                    || p.protocol_options.stream_usage.is_some())
            {
                return Err(ProviderError::configuration(
                    "chat protocol options used for a different protocol",
                ));
            }
            if p.protocol != ProtocolKind::AnthropicMessages
                && p.protocol_options.cache_control.is_some()
            {
                return Err(ProviderError::configuration(
                    "cache_control option is specific to Messages",
                ));
            }
            if let AuthConfig::Env { variable } = &p.auth {
                if variable.trim().is_empty() {
                    return Err(ProviderError::configuration(
                        "credential env reference is empty",
                    ));
                }
            }
            if let AuthConfig::Local { credential } = &p.auth {
                if credential.is_empty()
                    || credential.len() > 128
                    || !credential
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                {
                    return Err(ProviderError::configuration(
                        "invalid local credential reference",
                    ));
                }
            }
        }
        for (id, m) in &self.models {
            if id.trim().is_empty()
                || id == "mock"
                || m.model_id.trim().is_empty()
                || !self.providers.contains_key(&m.provider)
            {
                return Err(ProviderError::configuration(
                    "model alias/id/provider reference is invalid",
                ));
            }
            let c = &m.capabilities;
            if self.providers[&m.provider].transport.mode == TransportMode::Sse && !c.streaming {
                return Err(ProviderError::unsupported(
                    "non-streaming model requires explicit JSON transport mode",
                ));
            }
            if c.context_tokens == 0
                || c.max_output_tokens == 0
                || c.max_output_tokens >= c.context_tokens
                || c.token_count_method != "byte_estimate"
            {
                return Err(ProviderError::configuration(
                    "model capacity or token counting method is invalid",
                ));
            }
            if c.tool_choice
                .iter()
                .any(|s| !matches!(s.as_str(), "auto" | "none" | "required" | "named"))
            {
                return Err(ProviderError::configuration(
                    "invalid tool choice capability",
                ));
            }
            validate_generation(c, &m.defaults)?;
        }
        Ok(())
    }
}
pub(crate) fn validate_generation(
    c: &ModelCapabilities,
    g: &GenerationOverrides,
) -> Result<(), ProviderError> {
    if g.max_output_tokens == Some(0) {
        return Err(ProviderError::configuration(
            "output tokens must be positive",
        ));
    }
    if let Some(t) = g.temperature {
        if !t.is_finite() || !(0.0..=2.0).contains(&t) {
            return Err(ProviderError::configuration("invalid temperature"));
        }
        if !c.temperature {
            return Err(ProviderError::unsupported(
                "model does not support temperature",
            ));
        }
    }
    Ok(())
}
