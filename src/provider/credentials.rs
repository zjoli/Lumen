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
}
