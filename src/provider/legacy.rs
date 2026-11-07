use super::*;
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    endpoint: String,
    api_key_env: String,
}
pub(crate) struct LegacyConnection {
    pub endpoint: String,
    pub auth: AuthConfig,
    pub protocol: ProtocolKind,
    pub capacity: ModelCapabilities,
    pub cache: CacheIntent,
}
pub(crate) fn resolve(
    root: &Path,
    profile_ref: &str,
    format: &LegacyFormat,
) -> Result<LegacyConnection, ProviderError> {
    let path = crate::workspace::files::safe_path(root, ".lumen/model-profiles.yaml")
        .map_err(|_| ProviderError::configuration("unsafe profile path"))?;
    let profiles: BTreeMap<String, Profile> = match std::fs::read(path) {
        Ok(b) => serde_yaml_ng::from_slice(&b)
            .map_err(|_| ProviderError::configuration("invalid legacy model profiles"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(_) => return Err(ProviderError::configuration("cannot read legacy profiles")),
    };
    let (endpoint, auth) = if let Some(p) = profiles.get(profile_ref) {
        (
            p.endpoint.clone(),
            AuthConfig::Env {
                variable: p.api_key_env.clone(),
            },
        )
    } else if profile_ref == "default" {
        (
            std::env::var("LUMEN_MODEL_ENDPOINT").map_err(|_| {
                ProviderError::configuration(
                    "missing legacy model endpoint; history remains readable",
                )
            })?,
            AuthConfig::Env {
                variable: "LUMEN_MODEL_KEY".into(),
            },
        )
    } else {
        return Err(ProviderError::configuration(
            "missing local legacy model profile",
        ));
    };
    let capacity = ModelCapabilities {
        context_tokens: env_capacity("LUMEN_CONTEXT_TOKENS", 128_000)?,
        max_output_tokens: env_capacity("LUMEN_OUTPUT_TOKENS", 4096)?,
        ..Default::default()
    };
    Ok(LegacyConnection {
        endpoint,
        auth,
        protocol: match format {
            LegacyFormat::Openai => ProtocolKind::OpenaiChat,
            LegacyFormat::Anthropic => ProtocolKind::AnthropicMessages,
        },
        capacity,
        cache: if std::env::var("LUMEN_PROMPT_CACHE").is_ok_and(|v| v == "0" || v == "false") {
            CacheIntent::Disabled
        } else {
            CacheIntent::ProviderDefault
        },
    })
}
fn env_capacity(name: &str, default: usize) -> Result<usize, ProviderError> {
    match std::env::var(name) {
        Ok(s) => s
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| ProviderError::configuration(format!("invalid {name}"))),
        Err(_) => Ok(default),
    }
}
