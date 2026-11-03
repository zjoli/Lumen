mod index;
mod maintenance;
mod plugin;
mod search;
use crate::{
    knowledge::*,
    workspace::{WorkspaceError as E, WorkspaceHandle, WorkspaceResult as R},
};
use async_trait::async_trait;
pub(crate) use index::{model_status, refresh};
pub use index::{ChangeScope, IndexSelection, IndexService, IndexStatus};
pub(crate) use maintenance::start_maintenance;
pub use plugin::RetrievalPlugin;
pub use search::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmbeddingBatch {
    pub model_id: String,
    pub dimensions: usize,
    pub vectors: Vec<Vec<f32>>,
}
#[async_trait]
pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;
    async fn embed(&self, texts: Vec<String>, cancel: CancellationToken) -> R<EmbeddingBatch>;
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RankedCandidate {
    pub index: usize,
    pub score: f64,
}
#[async_trait]
pub trait Reranker: Send + Sync {
    async fn rerank(
        &self,
        query: String,
        candidates: Vec<String>,
        cancel: CancellationToken,
    ) -> R<Vec<RankedCandidate>>;
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SummaryInput {
    pub title: String,
    pub content: String,
    pub level: u8,
    pub max_chars: usize,
}
#[async_trait]
pub trait SummaryGenerator: Send + Sync {
    fn id(&self) -> &str;
    async fn generate(&self, input: SummaryInput, cancel: CancellationToken) -> R<String>;
}
#[derive(Default)]
pub(crate) struct Providers {
    pub embedder: RwLock<Option<Arc<dyn Embedder>>>,
    pub reranker: RwLock<Option<Arc<dyn Reranker>>>,
    pub summary: RwLock<Option<Arc<dyn SummaryGenerator>>>,
}
impl crate::workspace::Workspace {
    pub fn set_embedder(&self, provider: Option<Arc<dyn Embedder>>) {
        *self
            .providers
            .embedder
            .write()
            .unwrap_or_else(|e| e.into_inner()) = provider;
        self.maintenance_notify.notify_one();
    }
    pub fn set_reranker(&self, provider: Option<Arc<dyn Reranker>>) {
        *self
            .providers
            .reranker
            .write()
            .unwrap_or_else(|e| e.into_inner()) = provider;
    }
    pub fn set_summary_generator(&self, provider: Option<Arc<dyn SummaryGenerator>>) {
        *self
            .providers
            .summary
            .write()
            .unwrap_or_else(|e| e.into_inner()) = provider;
        self.maintenance_notify.notify_one();
    }
}
#[derive(Clone)]
pub struct SummaryService(pub(crate) WorkspaceHandle);
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryRequest {
    pub id: ObjectId,
    pub level: u8,
    pub max_chars: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SummaryResult {
    pub content: String,
    pub source_ref: SourceRef,
    pub generated: bool,
    pub coverage: Vec<ObjectId>,
    pub truncated: bool,
}
impl SummaryService {
    pub async fn read(&self, req: SummaryRequest, cancel: CancellationToken) -> R<SummaryResult> {
        let generator_id = self
            .0
            .providers
            .summary
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|g| g.id().to_string())
            .unwrap_or_default();
        self.0
            .call(cancel, move |s, c| {
                s.ensure_writable()?;
                let tree = crate::workspace::files::scan(&s.root)?;
                index::refresh(s, &tree, c)?;
                let catalog = Catalog::from_tree(&tree);
                let view = catalog.view(
                    &req.id,
                    if req.level == 0 {
                        ReadView::Abstract
                    } else if req.level == 1 {
                        ReadView::Overview
                    } else {
                        return Err(E::InvalidFormat("summary level must be 0 or 1".into()));
                    },
                    &s.repository.repository_id,
                )?;
                let coverage = catalog.scope(&req.id, true).into_iter().collect::<Vec<_>>();
                let key = format!("{}:{}", summary_key(&catalog, &req)?, generator_id);
                let db = index::connect(&s.root)?;
                let cached = db
                    .query_row(
                        "SELECT body FROM summaries WHERE cache_key=?1 AND generator=?2",
                        [&key, &generator_id],
                        |r| r.get::<_, String>(0),
                    )
                    .ok();
                let generated = cached.is_some();
                let text = cached.unwrap_or(view.content);
                let max = req
                    .max_chars
                    .unwrap_or(if req.level == 0 { 256 } else { 4000 });
                Ok(SummaryResult {
                    truncated: text.chars().count() > max,
                    content: text.chars().take(max).collect(),
                    source_ref: view.source_ref,
                    generated,
                    coverage,
                })
            })
            .await
    }
    pub async fn generate(
        &self,
        req: SummaryRequest,
        cancel: CancellationToken,
    ) -> R<SummaryResult> {
        let generator = self
            .0
            .providers
            .summary
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| E::CapabilityUnavailable("summary generator".into()))?;
        let r = req.clone();
        let (key, input) = self
            .0
            .call(cancel.clone(), move |s, _| {
                s.ensure_writable()?;
                let c = Catalog::from_tree(&crate::workspace::files::scan(&s.root)?);
                let view = c.view(&r.id, ReadView::Full, &s.repository.repository_id)?;
                let mut content = view.content;
                for id in c.scope(&r.id, true) {
                    if id != r.id {
                        content.push_str(
                            &c.view(&id, ReadView::Abstract, &s.repository.repository_id)?
                                .content,
                        )
                    }
                }
                Ok((
                    summary_key(&c, &r)?,
                    SummaryInput {
                        title: view.metadata.title,
                        content,
                        level: r.level,
                        max_chars: r.max_chars.unwrap_or(if r.level == 0 { 256 } else { 4000 }),
                    },
                ))
            })
            .await?;
        if req.level > 1 {
            return Err(E::InvalidFormat("summary level must be 0 or 1".into()));
        }
        let body = generator.generate(input, cancel.clone()).await?;
        let r = req.clone();
        let generator_id = generator.id().to_owned();
        self.0
            .call(cancel.clone(), move |s, _| {
                let c = Catalog::from_tree(&crate::workspace::files::scan(&s.root)?);
                if summary_key(&c, &r)? != key {
                    return Err(E::RevisionConflict(r.id.to_string()));
                }
                index::connect(&s.root)?.execute(
                    "INSERT OR REPLACE INTO summaries(cache_key,body,generator)VALUES(?1,?2,?3)",
                    rusqlite::params![format!("{key}:{generator_id}"), body, generator_id],
                )?;
                Ok(())
            })
            .await?;
        self.read(req, cancel).await
    }
}
fn summary_key(c: &Catalog, r: &SummaryRequest) -> R<String> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"lumen-summary-v1");
    hash.update(serde_json::to_string(r)?.as_bytes());
    for id in c.scope(&r.id, true) {
        if let Some((_, rev, _)) = c.objects.get(&id) {
            hash.update(id.as_str().as_bytes());
            hash.update(rev.0.as_bytes());
        }
    }
    Ok(hash.finalize().to_hex().to_string())
}
pub(crate) fn cached_summary(
    root: &std::path::Path,
    catalog: &Catalog,
    id: &ObjectId,
    level: u8,
    generator: &str,
) -> R<Option<String>> {
    if generator.is_empty() {
        return Ok(None);
    }
    if level == 0
        && catalog
            .objects
            .get(id)
            .is_some_and(|(o, _, _)| o.metadata.summary.is_some())
    {
        return Ok(None);
    }
    let key = summary_key(
        catalog,
        &SummaryRequest {
            id: id.clone(),
            level,
            max_chars: None,
        },
    )?;
    use rusqlite::OptionalExtension;
    Ok(index::connect(root)?
        .query_row(
            "SELECT body FROM summaries WHERE cache_key=?1 AND generator=?2",
            [format!("{key}:{generator}").as_str(), generator],
            |r| r.get::<_, String>(0),
        )
        .optional()?)
}
pub fn estimate_tokens(text: &str) -> usize {
    let ascii = text.chars().filter(|c| c.is_ascii()).count();
    let other = text.chars().count() - ascii;
    other + ascii.div_ceil(4)
}
pub(crate) fn truncate_tokens(text: &str, budget: usize) -> String {
    let mut ascii = 0;
    let mut other = 0;
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii() {
            ascii += 1
        } else {
            other += 1
        }
        if other + (ascii as usize).div_ceil(4) > budget {
            break;
        }
        out.push(ch)
    }
    out
}
