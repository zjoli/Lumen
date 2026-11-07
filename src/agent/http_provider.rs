//! Compatibility constructors. New application code resolves providers through ProviderRegistry.
use super::*;
use crate::provider::*;
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
#[derive(Clone, Copy, Debug)]
pub enum ApiFormat {
    OpenAi,
    Anthropic,
}
#[derive(Clone)]
pub struct HttpProvider {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub format: ApiFormat,
    pub cache_enabled: bool,
    capacity: ModelCapabilities,
    cached: Arc<crate::provider::CompatCache>,
}
impl HttpProvider {
    pub fn openai(
        endpoint: impl Into<String>,
        key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: key.into(),
            model: model.into(),
            format: ApiFormat::OpenAi,
            cache_enabled: true,
            capacity: Default::default(),
            cached: Default::default(),
        }
    }
    pub fn anthropic(
        endpoint: impl Into<String>,
        key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            format: ApiFormat::Anthropic,
            ..Self::openai(endpoint, key, model)
        }
    }
    pub fn with_capabilities(mut self, capacity: ModelCapabilities) -> Self {
        self.capacity = capacity;
        self
    }
    fn bound(&self) -> Result<Arc<HttpModelProvider>, ProviderError> {
        crate::provider::compat_http(
            &self.endpoint,
            &self.api_key,
            &self.model,
            self.format,
            self.cache_enabled,
            &self.capacity,
            &self.cached,
        )
    }
}
#[async_trait]
impl ModelProvider for HttpProvider {
    fn capabilities(&self) -> ModelCapabilities {
        self.capacity.clone()
    }
    fn configuration_key(&self) -> String {
        self.bound()
            .map(|p| p.configuration_key())
            .unwrap_or_else(|_| "invalid-http-provider".into())
    }
    fn prepare(&self, r: ModelRequest) -> Result<PreparedRequest, ProviderError> {
        self.bound()?.prepare(r)
    }
    async fn stream_prepared(
        &self,
        r: PreparedRequest,
        c: CancellationToken,
    ) -> Result<ModelEventStream, ProviderError> {
        self.bound()?.stream_prepared(r, c).await
    }
    async fn stream(
        &self,
        r: ModelRequest,
        c: CancellationToken,
    ) -> Result<ModelEventStream, ProviderError> {
        self.bound()?.stream(r, c).await
    }
}
