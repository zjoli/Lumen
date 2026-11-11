use super::*;
use crate::retrieval::*;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

pub struct HttpEmbedder {
    config: EmbeddingConfig,
    http: ServiceHttp,
    key: String,
}
impl HttpEmbedder {
    pub fn new(config: EmbeddingConfig, http: ServiceHttp) -> Self {
        let key = format!(
            "{}:{}",
            config.model,
            blake3::hash(
                format!(
                    "{}\n{}\n{:?}",
                    config.connection.endpoint, config.state_scope, config.dimensions
                )
                .as_bytes()
            )
            .to_hex()
        );
        Self { config, http, key }
    }
}
#[async_trait]
impl Embedder for HttpEmbedder {
    fn model_id(&self) -> &str {
        &self.key
    }
    async fn embed(&self, texts: Vec<String>, cancel: CancellationToken) -> R<EmbeddingBatch> {
        if texts.is_empty() || texts.iter().any(|s| s.trim().is_empty()) {
            return Err(E::InvalidFormat(
                "embedding inputs must not be empty".into(),
            ));
        }
        let mut vectors = vec![];
        let mut dimensions = self.config.dimensions;
        for batch in texts.chunks(16) {
            let mut body =
                json!({"model":self.config.model,"input":batch,"encoding_format":"float"});
            if let Some(d) = self.config.dimensions {
                body["dimensions"] = json!(d);
            }
            let response = self
                .http
                .json(&self.config.connection, body, cancel.clone())
                .await?;
            #[derive(Deserialize)]
            struct Item {
                index: usize,
                embedding: Vec<f32>,
            }
            let mut data: Vec<Item> = serde_json::from_value(response["data"].clone())
                .map_err(|_| E::InvalidFormat("invalid embeddings response".into()))?;
            data.sort_by_key(|i| i.index);
            if data.len() != batch.len() || data.iter().enumerate().any(|(i, v)| v.index != i) {
                return Err(E::InvalidFormat("embedding count or index mismatch".into()));
            }
            for item in data {
                let d = *dimensions.get_or_insert(item.embedding.len());
                if d == 0
                    || d > 65536
                    || item.embedding.len() != d
                    || item.embedding.iter().any(|n| !n.is_finite())
                    || item.embedding.iter().all(|n| *n == 0.0)
                {
                    return Err(E::InvalidFormat(
                        "invalid embedding vector or dimensions".into(),
                    ));
                }
                vectors.push(item.embedding);
            }
        }
        Ok(EmbeddingBatch {
            model_id: self.key.clone(),
            dimensions: dimensions.unwrap_or(0),
            vectors,
        })
    }
}
pub struct HttpReranker {
    config: RerankConfig,
    http: ServiceHttp,
}
impl HttpReranker {
    pub fn new(config: RerankConfig, http: ServiceHttp) -> Self {
        Self { config, http }
    }
}
#[async_trait]
impl Reranker for HttpReranker {
    async fn rerank(
        &self,
        query: String,
        candidates: Vec<String>,
        cancel: CancellationToken,
    ) -> R<Vec<RankedCandidate>> {
        if candidates.is_empty() {
            return Ok(vec![]);
        }
        let count = candidates.len();
        let response=self.http.json(&self.config.connection,json!({"model":self.config.model,"query":query,"documents":candidates,"top_n":count,"return_documents":false}),cancel).await?;
        #[derive(Deserialize)]
        struct Item {
            index: usize,
            relevance_score: f64,
        }
        let data: Vec<Item> = serde_json::from_value(response["results"].clone())
            .map_err(|_| E::InvalidFormat("invalid reranker response".into()))?;
        let mut seen = std::collections::BTreeSet::new();
        if data.len() != count
            || data.iter().any(|v| {
                v.index >= count || !v.relevance_score.is_finite() || !seen.insert(v.index)
            })
        {
            return Err(E::InvalidFormat(
                "reranker returned invalid or incomplete candidate indices".into(),
            ));
        }
        Ok(data
            .into_iter()
            .map(|i| RankedCandidate {
                index: i.index,
                score: i.relevance_score,
            })
            .collect())
    }
}
pub struct ModelSummary {
    registry: Arc<ProviderRegistry>,
    selection: ModelSelection,
    key: String,
}
impl ModelSummary {
    pub fn new(registry: Arc<ProviderRegistry>, selection: ModelSelection, key: String) -> Self {
        Self {
            registry,
            selection,
            key,
        }
    }
}
#[async_trait]
impl SummaryGenerator for ModelSummary {
    fn id(&self) -> &str {
        &self.key
    }
    async fn generate(&self, input: SummaryInput, cancel: CancellationToken) -> R<String> {
        let resolved = self
            .registry
            .resolve(&self.selection, cancel.clone())
            .await
            .map_err(provider_error)?;
        if resolved.descriptor.configuration_key != self.key {
            return Err(E::Configuration(
                "summary model changed; run /services reload".into(),
            ));
        }
        let max_output = resolved.descriptor.generation.max_output_tokens;
        let prompt=format!("Summarize the following knowledge in the source language. Preserve qualifications and source references; do not add facts. Level {} (0=short abstract, 1=structured overview). At most {} characters. Title: {}\n\n{}",input.level,input.max_chars,input.title,input.content);
        let text = complete_text(resolved.provider, prompt, max_output, cancel).await?;
        Ok(text.chars().take(input.max_chars).collect())
    }
}
