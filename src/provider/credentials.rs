use super::{AuthConfig, ProviderError};
#[derive(Clone)]
pub(crate) struct Secret(pub String);
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl Secret {
    pub fn resolve(auth: &AuthConfig) -> Result<Option<Self>, ProviderError> {
        match auth {
            AuthConfig::None => Ok(None),
            AuthConfig::Local { .. } => Err(ProviderError::configuration(
                "local credentials require a configuration directory",
            )),
            AuthConfig::Env { variable } => std::env::var(variable)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(|s| Some(Self(s)))
                .ok_or_else(|| {
                    ProviderError::configuration(format!(
                        "missing credential environment variable {variable}"
                    ))
                }),
        }
    }
    pub fn resolve_at(
        auth: &AuthConfig,
        root: &std::path::Path,
    ) -> Result<Option<Self>, ProviderError> {
        if let AuthConfig::Local { credential } = auth {
            let values = super::management::read_credentials(root)?;
            return values
                .get(credential)
                .filter(|s| !s.trim().is_empty())
                .cloned()
                .map(|s| Some(Self(s)))
                .ok_or_else(|| {
                    ProviderError::configuration("API Key 尚未配置，请使用 /provider key <名称>")
                });
        }
        Self::resolve(auth)
    }
}
