mod capabilities;
mod config;
mod credentials;
mod error;
mod legacy;
mod mock;
mod protocols;
mod registry;
mod replay;
mod sse;
mod transport;
mod usage;

pub use capabilities::*;
pub use config::*;
pub use error::*;
pub use mock::*;
pub use registry::*;
pub use replay::*;
pub use transport::{
    HttpBody, HttpModelProvider, HttpRequest, HttpResponse, HttpTransport, ReqwestTransport,
};

pub(crate) fn legacy_profile(
    selection: &ModelSelection,
    root: &std::path::Path,
) -> Result<Option<(String, String)>, ProviderError> {
    match selection {
        ModelSelection::Mock => Ok(None),
        ModelSelection::Legacy {
            profile_ref,
            format,
            ..
        } => {
            let connection = legacy::resolve(root, profile_ref, format)?;
            let secret = credentials::Secret::resolve(&connection.auth)?.ok_or_else(|| {
                ProviderError::configuration("legacy profile requires authentication")
            })?;
            Ok(Some((connection.endpoint, secret.0)))
        }
        ModelSelection::Reference { .. } => Err(ProviderError::configuration(
            "use ProviderRegistry to resolve configured model references",
        )),
    }
}

pub(crate) type CompatCache =
    std::sync::Mutex<Option<(String, String, std::sync::Arc<HttpModelProvider>)>>;
pub(crate) fn compat_http(
    endpoint: &str,
    key: &str,
    model: &str,
    format: crate::agent::ApiFormat,
    cache: bool,
    capacity: &ModelCapabilities,
    stored: &CompatCache,
) -> Result<std::sync::Arc<HttpModelProvider>, ProviderError> {
    let protocol = match format {
        crate::agent::ApiFormat::OpenAi => ProtocolKind::OpenaiChat,
        crate::agent::ApiFormat::Anthropic => ProtocolKind::AnthropicMessages,
    };
    let t = TransportConfig {
        mode: TransportMode::Json,
        ..Default::default()
    };
    let o = ProtocolOptions::default();
    let mut d = ResolvedModelDescriptor {
        provider_ref: "legacy-http".into(),
        protocol,
        model_id: model.into(),
        capabilities: capacity.clone(),
        generation: EffectiveGenerationOptions {
            max_output_tokens: capacity.max_output_tokens,
            cache: if cache {
                CacheIntent::ProviderDefault
            } else {
                CacheIntent::Disabled
            },
            ..Default::default()
        },
        ..Default::default()
    };
    registry::finalize_descriptor(&mut d, endpoint, "legacy-http", &t, &o)?;
    let mut guard = stored
        .lock()
        .map_err(|_| ProviderError::configuration("HTTP provider lock unavailable"))?;
    if let Some((fp, secret, p)) = &*guard {
        if fp == &d.configuration_key && secret == key {
            return Ok(p.clone());
        }
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|_| ProviderError::configuration("cannot initialize HTTP client"))?;
    let p = std::sync::Arc::new(HttpModelProvider::new(
        d.clone(),
        endpoint.into(),
        Some(credentials::Secret(key.into())),
        t,
        o,
        std::sync::Arc::new(ReqwestTransport::new(client)),
    )?);
    *guard = Some((d.configuration_key, key.into(), p.clone()));
    Ok(p)
}
