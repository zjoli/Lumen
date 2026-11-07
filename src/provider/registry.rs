use super::credentials::Secret;
use super::*;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub struct ResolvedModel {
    pub descriptor: ResolvedModelDescriptor,
    pub provider: Arc<dyn crate::agent::ModelProvider>,
}
pub struct ProviderRegistry {
    root: PathBuf,
    http: Option<Arc<dyn HttpTransport>>,
    clients: Mutex<BTreeMap<String, reqwest::Client>>,
}
impl ProviderRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            http: None,
            clients: Mutex::new(BTreeMap::new()),
        }
    }
    pub fn with_transport(root: impl Into<PathBuf>, http: Arc<dyn HttpTransport>) -> Self {
        Self {
            http: Some(http),
            ..Self::new(root)
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn config(&self) -> Result<Option<ProviderConfig>, ProviderError> {
        ProviderConfig::load(&self.root)
    }
    pub fn default_selection(&self) -> Result<ModelSelection, ProviderError> {
        if let Some(c) = self.config()? {
            return Ok(ModelSelection::Reference {
                model_ref: c.default_model,
                overrides: Default::default(),
            });
        }
        let model = std::env::var("LUMEN_MODEL").unwrap_or_else(|_| "mock".into());
        if model == "mock" {
            return Ok(ModelSelection::Mock);
        }
        let format = match std::env::var("LUMEN_MODEL_FORMAT")
            .unwrap_or_else(|_| "openai".into())
            .as_str()
        {
            "openai" => LegacyFormat::Openai,
            "anthropic" => LegacyFormat::Anthropic,
            _ => return Err(ProviderError::configuration("unknown legacy model format")),
        };
        Ok(ModelSelection::Legacy {
            profile_ref: "default".into(),
            model,
            format,
        })
    }
    pub fn selection(
        &self,
        name: &str,
        current: Option<&ModelSelection>,
    ) -> Result<ModelSelection, ProviderError> {
        if name == "mock" {
            return Ok(ModelSelection::Mock);
        }
        if let Some(c) = self.config()? {
            if !c.models.contains_key(name) {
                return Err(ProviderError::configuration(
                    "unknown configured model alias",
                ));
            }
            return Ok(ModelSelection::Reference {
                model_ref: name.into(),
                overrides: Default::default(),
            });
        }
        let (profile_ref, format) = match current {
            Some(ModelSelection::Legacy {
                profile_ref,
                format,
                ..
            }) => (profile_ref.clone(), format.clone()),
            _ => (
                "default".into(),
                match std::env::var("LUMEN_MODEL_FORMAT")
                    .unwrap_or_else(|_| "openai".into())
                    .as_str()
                {
                    "openai" => LegacyFormat::Openai,
                    "anthropic" => LegacyFormat::Anthropic,
                    _ => return Err(ProviderError::configuration("unknown legacy model format")),
                },
            ),
        };
        Ok(ModelSelection::Legacy {
            profile_ref,
            model: name.into(),
            format,
        })
    }
    fn client(&self, t: &TransportConfig) -> Result<reqwest::Client, ProviderError> {
        let key = serde_json::to_string(t).unwrap();
        let mut clients = self
            .clients
            .lock()
            .map_err(|_| ProviderError::configuration("client pool unavailable"))?;
        if let Some(c) = clients.get(&key) {
            return Ok(c.clone());
        }
        let c = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(std::time::Duration::from_millis(t.connect_timeout_ms))
            .build()
            .map_err(|_| ProviderError::configuration("cannot initialize HTTP client"))?;
        clients.insert(key, c.clone());
        Ok(c)
    }
    pub async fn resolve(
        &self,
        s: &ModelSelection,
        cancel: CancellationToken,
    ) -> Result<ResolvedModel, ProviderError> {
        self.resolve_inner(s, cancel, true)
    }
    pub fn describe(&self, s: &ModelSelection) -> Result<ResolvedModelDescriptor, ProviderError> {
        Ok(self
            .resolve_inner(s, CancellationToken::new(), false)?
            .descriptor)
    }
    fn resolve_inner(
        &self,
        s: &ModelSelection,
        cancel: CancellationToken,
        credentials: bool,
    ) -> Result<ResolvedModel, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::cancelled());
        }
        if matches!(s, ModelSelection::Mock) {
            let p = MockProvider::demonstration();
            return Ok(ResolvedModel {
                descriptor: crate::agent::ModelProvider::descriptor(&p).clone(),
                provider: Arc::new(p),
            });
        }
        let (mut descriptor, endpoint, scope, secret, transport, options) = match s {
            ModelSelection::Reference {
                model_ref,
                overrides,
            } => {
                let config = self.config()?.ok_or_else(|| {
                    ProviderError::configuration("missing providers.yaml for model reference")
                })?;
                let m = config.models.get(model_ref).ok_or_else(|| {
                    ProviderError::configuration("model reference is not configured")
                })?;
                let p = &config.providers[&m.provider];
                let mut caps = m.capabilities.clone();
                caps.streaming = p.transport.mode == TransportMode::Sse;
                super::config::validate_generation(&caps, overrides)?;
                let generation = EffectiveGenerationOptions {
                    max_output_tokens: overrides
                        .max_output_tokens
                        .or(m.defaults.max_output_tokens)
                        .unwrap_or(caps.max_output_tokens)
                        .min(caps.max_output_tokens),
                    temperature: overrides.temperature.or(m.defaults.temperature),
                    cache: overrides.cache.or(m.defaults.cache).unwrap_or_default(),
                };
                let endpoint =
                    format!("{}/{}", p.base_url.trim_end_matches('/'), p.protocol.path());
                (
                    ResolvedModelDescriptor {
                        model_ref: Some(model_ref.clone()),
                        provider_ref: m.provider.clone(),
                        protocol: p.protocol,
                        model_id: m.model_id.clone(),
                        capabilities: caps,
                        generation,
                        ..Default::default()
                    },
                    endpoint,
                    p.state_scope.clone(),
                    if credentials {
                        Secret::resolve(&p.auth)?
                    } else {
                        None
                    },
                    p.transport.clone(),
                    p.protocol_options.clone(),
                )
            }
            ModelSelection::Legacy {
                profile_ref,
                model,
                format,
            } => {
                let p = super::legacy::resolve(&self.root, profile_ref, format)?;
                (
                    ResolvedModelDescriptor {
                        model_ref: None,
                        provider_ref: format!("legacy:{profile_ref}"),
                        protocol: p.protocol,
                        model_id: model.clone(),
                        generation: EffectiveGenerationOptions {
                            max_output_tokens: p.capacity.max_output_tokens,
                            cache: p.cache,
                            ..Default::default()
                        },
                        capabilities: p.capacity,
                        ..Default::default()
                    },
                    p.endpoint,
                    profile_ref.clone(),
                    if credentials {
                        Secret::resolve(&p.auth)?
                    } else {
                        None
                    },
                    TransportConfig {
                        mode: TransportMode::Json,
                        ..Default::default()
                    },
                    ProtocolOptions::default(),
                )
            }
            ModelSelection::Mock => unreachable!(),
        };
        if cancel.is_cancelled() {
            return Err(ProviderError::cancelled());
        }
        finalize_descriptor(&mut descriptor, &endpoint, &scope, &transport, &options)?;
        if descriptor.generation.cache == CacheIntent::ProviderDefault
            && !descriptor
                .capabilities
                .cache_modes
                .contains(&CacheIntent::ProviderDefault)
        {
            descriptor.diagnostics.push("cache hints are not declared for this model; server automatic caching may still apply".into());
        }
        let provider = HttpModelProvider::new(
            descriptor.clone(),
            endpoint,
            secret,
            transport.clone(),
            options,
            match &self.http {
                Some(t) => t.clone(),
                None => Arc::new(ReqwestTransport::new(self.client(&transport)?)),
            },
        )?;
        Ok(ResolvedModel {
            descriptor,
            provider: Arc::new(provider),
        })
    }
    pub fn models(&self) -> Result<serde_json::Value, ProviderError> {
        Ok(match self.config()? {
            Some(c) => {
                serde_json::json!({"default_model":c.default_model,"models":c.models.iter().map(|(alias,m)|serde_json::json!({"alias":alias,"provider":m.provider,"model_id":m.model_id,"capabilities":m.capabilities})).collect::<Vec<_>>()})
            }
            None => {
                serde_json::json!({"default_model":self.default_selection()?,"models":[],"legacy":true})
            }
        })
    }
    pub fn providers(&self) -> Result<serde_json::Value, ProviderError> {
        Ok(match self.config()? {Some(c)=>serde_json::json!(c.providers.iter().map(|(name,p)|serde_json::json!({"provider":name,"protocol":p.protocol,"auth":match p.auth {AuthConfig::None=>"none",AuthConfig::Env{..}=>"env"},"configured":true})).collect::<Vec<_>>()),None=>serde_json::json!({"legacy":true})})
    }
}
pub(crate) fn finalize_descriptor(
    d: &mut ResolvedModelDescriptor,
    endpoint: &str,
    scope: &str,
    t: &TransportConfig,
    o: &ProtocolOptions,
) -> Result<(), ProviderError> {
    let mut route = reqwest::Url::parse(endpoint)
        .map_err(|_| ProviderError::configuration("invalid model endpoint"))?;
    if !matches!(route.scheme(), "http" | "https") || route.host_str().is_none() {
        return Err(ProviderError::configuration(
            "model endpoint must be HTTP(S)",
        ));
    }
    // Legacy query and userinfo may contain credentials: exclude them even from fingerprints.
    route.set_query(None);
    route.set_fragment(None);
    let _ = route.set_username("");
    let _ = route.set_password(None);
    let route_hash = blake3::hash(route.as_str().as_bytes()).to_hex().to_string();
    d.compatibility_key = hash(&(
        &d.provider_ref,
        &route_hash,
        scope,
        d.protocol,
        "adapter-v1",
        &d.model_id,
    ));
    d.configuration_key = hash(&(
        &d.provider_ref,
        route_hash,
        scope,
        d.protocol,
        &d.model_id,
        &d.capabilities,
        &d.generation,
        t,
        o,
    ));
    Ok(())
}
fn hash(v: &impl serde::Serialize) -> String {
    blake3::hash(&serde_json::to_vec(v).unwrap())
        .to_hex()
        .to_string()
}
