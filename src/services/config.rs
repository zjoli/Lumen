use super::*;
use crate::{
    retrieval::*,
    workspace::{files, WorkspaceHandle},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceEndpoint {
    pub endpoint: String,
    #[serde(default = "no_auth")]
    pub auth: AuthConfig,
    #[serde(default = "request_timeout")]
    pub timeout_ms: u64,
}
fn no_auth() -> AuthConfig {
    AuthConfig::None
}
fn request_timeout() -> u64 {
    25_000
}
impl ServiceEndpoint {
    pub fn validate(&self) -> R<()> {
        if matches!(self.auth, AuthConfig::Local { .. }) {
            return Err(E::Configuration(
                "services.yaml credentials use env or none".into(),
            ));
        }
        let url = reqwest::Url::parse(&self.endpoint)
            .map_err(|_| E::Configuration("invalid service endpoint".into()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(100..=120_000).contains(&self.timeout_ms)
        {
            return Err(E::Configuration("service endpoint must be HTTP(S), without credentials, query or fragment; timeout_ms must be 100–120000".into()));
        }
        if let AuthConfig::Env { variable } = &self.auth {
            if variable.is_empty()
                || !variable
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(E::Configuration(
                    "invalid credential environment reference".into(),
                ));
            }
        }
        Ok(())
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingConfig {
    pub connection: ServiceEndpoint,
    pub model: String,
    #[serde(default)]
    pub dimensions: Option<usize>,
    #[serde(default)]
    pub state_scope: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RerankConfig {
    pub connection: ServiceEndpoint,
    pub model: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchConfig {
    pub connection: ServiceEndpoint,
    pub protocol: SearchProtocol,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchProtocol {
    Searxng,
    Jina,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryConfig {
    pub model: String,
}
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServicesConfig {
    pub schema_version: u32,
    pub search: Option<SearchConfig>,
    pub reader: Option<ServiceEndpoint>,
    pub embedding: Option<EmbeddingConfig>,
    pub reranker: Option<RerankConfig>,
    pub summary: Option<SummaryConfig>,
}
impl ServicesConfig {
    pub fn load(root: &Path) -> R<Self> {
        let path = files::safe_path(root, ".lumen/services.yaml")?;
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    schema_version: 1,
                    ..Default::default()
                })
            }
            Err(_) => return Err(E::Configuration("cannot read services.yaml".into())),
        };
        let config: Self = serde_yaml_ng::from_slice(&bytes).map_err(|_| {
            E::Configuration("invalid services.yaml schema or duplicate keys".into())
        })?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> R<()> {
        if self.schema_version != 1 {
            return Err(E::Configuration(
                "unsupported services schema version".into(),
            ));
        }
        for endpoint in [
            self.search.as_ref().map(|s| &s.connection),
            self.reader.as_ref(),
            self.embedding.as_ref().map(|s| &s.connection),
            self.reranker.as_ref().map(|s| &s.connection),
        ]
        .into_iter()
        .flatten()
        {
            endpoint.validate()?;
        }
        if self.embedding.as_ref().is_some_and(|e| {
            e.model.trim().is_empty() || e.dimensions.is_some_and(|d| d == 0 || d > 65536)
        }) || self
            .reranker
            .as_ref()
            .is_some_and(|r| r.model.trim().is_empty())
            || self
                .summary
                .as_ref()
                .is_some_and(|s| s.model.trim().is_empty() || s.model == "mock")
        {
            return Err(E::Configuration(
                "invalid service model or dimensions".into(),
            ));
        }
        Ok(())
    }
    /// No credentials are resolved here. Missing credentials never prevent local reads.
    pub fn attach(&self, workspace: &WorkspaceHandle, http: ServiceHttp) -> R<()> {
        self.validate()?;
        let summary = if let Some(config) = &self.summary {
            let registry = Arc::new(ProviderRegistry::new(workspace.root()));
            let selection = registry
                .selection(&config.model, None)
                .map_err(provider_error)?;
            let descriptor = registry.describe(&selection).map_err(provider_error)?;
            Some(Arc::new(ModelSummary::new(
                registry,
                selection,
                descriptor.configuration_key,
            )) as Arc<dyn SummaryGenerator>)
        } else {
            None
        };
        let embedder = self
            .embedding
            .clone()
            .map(|c| Arc::new(HttpEmbedder::new(c, http.clone())) as Arc<dyn Embedder>);
        let reranker = self
            .reranker
            .clone()
            .map(|c| Arc::new(HttpReranker::new(c, http.clone())) as Arc<dyn Reranker>);
        workspace.set_embedder(embedder);
        workspace.set_reranker(reranker);
        workspace.set_summary_generator(summary);
        Ok(())
    }
    pub fn describe(&self) -> Value {
        json!({"search":self.search.as_ref().map(|s|json!({"protocol":s.protocol,"configured":true})),"reader":self.reader.is_some(),"embedding":self.embedding.as_ref().map(|s|json!({"model":s.model,"dimensions":s.dimensions})),"reranker":self.reranker.as_ref().map(|s|json!({"model":s.model})),"summary":self.summary.as_ref().map(|s|json!({"model":s.model}))})
    }
}
