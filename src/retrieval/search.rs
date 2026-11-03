use super::*;
use crate::workspace::{check_cancel, files};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Lexical,
    Semantic,
    #[default]
    Hybrid,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    pub mode: SearchMode,
    pub scope: Option<ObjectId>,
    pub descendants: bool,
    pub source_types: Vec<String>,
    pub levels: Vec<u8>,
    pub limit: usize,
    pub rerank: bool,
}
impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: SearchMode::Hybrid,
            scope: None,
            descendants: true,
            source_types: vec![],
            levels: vec![0, 1, 2],
            limit: 20,
            rerank: false,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub kind: String,
    pub level: u8,
    pub chunk_id: String,
    pub heading: String,
    pub excerpt: String,
    pub score: f64,
    pub match_sources: Vec<String>,
    pub source_ref: SourceRef,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    pub generation: String,
    pub freshness: String,
    pub degraded: Vec<String>,
    pub diagnostics: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Expansion {
    pub hops: usize,
    pub max_objects: usize,
}
impl Default for Expansion {
    fn default() -> Self {
        Self {
            hops: 1,
            max_objects: 50,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextRequest {
    pub search: SearchRequest,
    pub expansion: Expansion,
    pub token_budget: usize,
}
impl Default for ContextRequest {
    fn default() -> Self {
        Self {
            search: SearchRequest::default(),
            expansion: Expansion::default(),
            token_budget: 4000,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextBundle {
    pub fragments: Vec<SearchHit>,
    pub text: String,
    pub used_tokens: usize,
    pub token_budget: usize,
    pub truncated: bool,
    pub degraded: Vec<String>,
    pub generation: String,
}
#[derive(Clone)]
pub struct RetrievalService(pub(crate) WorkspaceHandle);
impl RetrievalService {
    pub async fn search(&self, req: SearchRequest, cancel: CancellationToken) -> R<SearchResult> {
        check_cancel(&cancel)?;
        let embedder = self
            .0
            .providers
            .embedder
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let reranker = self
            .0
            .providers
            .reranker
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut degraded = vec![];
        let vector = if req.mode != SearchMode::Lexical {
            match embedder {
                Some(e) => match e.embed(vec![req.query.clone()], cancel.clone()).await {
                    Ok(batch) => {
                        if batch.model_id != e.model_id() || batch.vectors.len() != 1 {
                            return Err(E::InvalidFormat("query embedding mismatch".into()));
                        }
                        Some((
                            batch.model_id,
                            index::normalize(
                                batch.vectors.into_iter().next().unwrap(),
                                batch.dimensions,
                            )?,
                        ))
                    }
                    Err(E::Cancelled) => return Err(E::Cancelled),
                    Err(e) if req.mode == SearchMode::Semantic => return Err(e),
                    Err(_) => {
                        degraded.push("embedding_failed".into());
                        None
                    }
                },
                None if req.mode == SearchMode::Semantic => {
                    return Err(E::CapabilityUnavailable("embedder".into()))
                }
                None => {
                    degraded.push("lexical_only".into());
                    None
                }
            }
        } else {
            None
        };
        let r = req.clone();
        let mut result=self.0.call(cancel.clone(),move|s,c|{
            s.ensure_writable()?;
let tree=files::scan(&s.root)?;
let status=index::refresh(s,&tree,c)?;
let catalog=Catalog::from_tree(&tree);
let scope=r.scope.as_ref().map(|id|catalog.scope(id,r.descendants));
let db=index::connect(&s.root)?;

            let mut query=db.prepare("SELECT c.id,c.document_id,c.heading,c.text,c.level,c.line,c.revision,d.path,d.kind,d.title,d.metadata FROM chunks c JOIN documents d ON d.id=c.document_id")?;

            let rows=query.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,u8>(4)?,row.get::<_,i64>(5)? as usize,row.get::<_,String>(6)?,row.get::<_,String>(7)?,row.get::<_,String>(8)?,row.get::<_,String>(9)?,row.get::<_,String>(10)?)))?.collect::<Result<Vec<_>,_>>()?;

            let mut candidates=BTreeMap::new();let mut exact=BTreeSet::new();let q=r.query.trim().to_lowercase();
            for(chunk_id,id,heading,text,level,line,revision,path,kind,title,metadata)in rows{
                let source_allowed=if r.source_types.is_empty(){ObjectKind::ALL.iter().any(|k|k.name()==kind)}else{r.source_types.contains(&kind)};
                if !source_allowed||!r.levels.contains(&level)||scope.as_ref().map(|scope|ObjectId::parse(&id).map(|i|!scope.contains(&i)).unwrap_or(true)).unwrap_or(false){continue}

                if q==id.to_lowercase()||q==title.to_lowercase()||serde_json::from_str::<serde_json::Value>(&metadata).ok().and_then(|m|m.get("aliases").and_then(|a|a.as_array()).map(|a|a.iter().any(|x|x.as_str().map(|s|s.to_lowercase()==q).unwrap_or(false)))).unwrap_or(false){exact.insert(chunk_id.clone());
}

                candidates.insert(chunk_id.clone(),SearchHit{id:id.clone(),kind,level,chunk_id,heading,excerpt:text,score:0.0,match_sources:vec![],source_ref:SourceRef{repository_id:s.repository.repository_id.clone(),path,object_id:id,revision:Revision(revision),line}
}
);

            }
            let mut ranks=BTreeMap::<String,f64>::new();let mut sources=BTreeMap::<String,Vec<String>>::new();
            if r.mode!=SearchMode::Semantic{let tokens=index::tokenize(&r.query);
if !tokens.is_empty(){let expr=tokens.into_iter().map(|s|format!("\"{}\"",s.replace('"',"\"\""))).collect::<Vec<_>>().join(" OR ");
let mut q=db.prepare("SELECT id FROM chunks_fts WHERE chunks_fts MATCH ?1 ORDER BY bm25(chunks_fts)")?;
let matches=q.query_map([expr],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
for(rank,id)in matches.into_iter().filter(|id|candidates.contains_key(id)).take(50).enumerate(){*ranks.entry(id.clone()).or_default()+=1.0/(61+rank)as f64;
sources.entry(id).or_default().push("lexical".into());
}
}
}

            if let Some((model,query))=vector{let mut stmt=db.prepare("SELECT chunk_id,dimensions,vector FROM embeddings WHERE model=?1")?;
let vectors=stmt.query_map([model],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)? as usize,row.get::<_,Vec<u8>>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
let mut scored=vec![];
for(id,dims,bytes)in vectors{if dims!=query.len()||bytes.len()!=dims*4||!candidates.contains_key(&id){continue}
let score=bytes.as_chunks::<4>().0.iter().zip(&query).map(|(b,q)|f32::from_le_bytes(*b)as f64*(*q as f64)).sum::<f64>();
if score.is_finite(){scored.push((id,score));
}
}
scored.sort_by(|a,b|b.1.total_cmp(&a.1));
if scored.is_empty(){if r.mode==SearchMode::Semantic{return Err(E::IndexNotReady)}
degraded.push("semantic_index_pending".into());
}
for(rank,(id,_))in scored.into_iter().take(50).enumerate(){*ranks.entry(id.clone()).or_default()+=1.0/(61+rank)as f64;
sources.entry(id).or_default().push("semantic".into());
}
}

            for id in exact{*ranks.entry(id.clone()).or_default()+=1.0;sources.entry(id).or_default().push("exact".into());}
            let mut hits=vec![];
let mut partial=false;
for(id,score)in ranks{if let Some(mut hit)=candidates.remove(&id){let path=files::safe_path(&s.root,&hit.source_ref.path)?;
if std::fs::read(path).map(|b|format::revision(&b)!=hit.source_ref.revision).unwrap_or(true){partial=true;
continue}
hit.score=score;
hit.match_sources=sources.remove(&id).unwrap_or_default();
hits.push(hit)}
}
hits.sort_by(|a,b|b.score.total_cmp(&a.score).then(a.chunk_id.cmp(&b.chunk_id)));
hits.truncate(if r.rerank{r.limit.clamp(1,100)*2}
else{r.limit.clamp(1,100)}
);

            Ok(SearchResult{hits,generation:status.generation,freshness:if partial{"partial"}else{"current"}.into(),degraded,diagnostics:status.diagnostics})
        }).await?;
        if req.rerank {
            if let Some(reranker) = reranker {
                match reranker
                    .rerank(
                        req.query,
                        result.hits.iter().map(|h| h.excerpt.clone()).collect(),
                        cancel,
                    )
                    .await
                {
                    Ok(ranked) => {
                        let mut seen = BTreeSet::new();
                        if ranked.iter().all(|r| {
                            r.index < result.hits.len()
                                && r.score.is_finite()
                                && seen.insert(r.index)
                        }) && !ranked.is_empty()
                        {
                            let mut hits = ranked
                                .into_iter()
                                .map(|r| {
                                    let mut hit = result.hits[r.index].clone();
                                    hit.score = r.score;
                                    hit
                                })
                                .collect::<Vec<_>>();
                            hits.sort_by(|a, b| b.score.total_cmp(&a.score));
                            result.hits = hits
                        } else {
                            result.degraded.push("rerank_invalid".into())
                        }
                    }
                    Err(E::Cancelled) => return Err(E::Cancelled),
                    Err(_) => result.degraded.push("rerank_failed".into()),
                }
            } else {
                result.degraded.push("reranker_unavailable".into())
            }
        }
        result.hits.truncate(req.limit.clamp(1, 100));
        Ok(result)
    }
    pub async fn context(
        &self,
        req: ContextRequest,
        cancel: CancellationToken,
    ) -> R<ContextBundle> {
        let result = self.search(req.search, cancel.clone()).await?;
        let mut hits = result.hits;
        let expansion = req.expansion;
        let generation = result.generation.clone();
        hits = self
            .0
            .call(cancel, move |s, _| {
                let tree = files::scan(&s.root)?;
                if files::fingerprint(&tree) != generation {
                    return Err(E::RevisionConflict("context changed; retry".into()));
                }
                let catalog = Catalog::from_tree(&tree);
                let mut seen = hits.iter().map(|h| h.id.clone()).collect::<BTreeSet<_>>();
                let mut frontier = seen.clone();
                for _ in 0..expansion.hops.min(2) {
                    let mut next = BTreeSet::new();
                    for (o, _, _) in catalog.objects.values() {
                        for rel in o.metadata.relations() {
                            let target = if frontier.contains(rel.source.as_str()) {
                                Some(rel.target)
                            } else if frontier.contains(rel.target.as_str()) {
                                Some(rel.source)
                            } else {
                                None
                            };
                            if let Some(id) = target {
                                if seen.len() >= expansion.max_objects.clamp(1, 100) {
                                    break;
                                }
                                if seen.insert(id.to_string()) {
                                    let view = catalog.view(
                                        &id,
                                        ReadView::Abstract,
                                        &s.repository.repository_id,
                                    )?;
                                    next.insert(id.to_string());
                                    hits.push(SearchHit {
                                        id: id.to_string(),
                                        kind: view.metadata.kind.name().into(),
                                        level: 0,
                                        chunk_id: format!("{id}:0:0"),
                                        heading: String::new(),
                                        excerpt: view.content,
                                        score: 0.0,
                                        match_sources: vec!["relation".into()],
                                        source_ref: view.source_ref,
                                    });
                                }
                            }
                        }
                    }
                    frontier = next;
                }
                Ok(hits)
            })
            .await?;
        let mut text = String::new();
        let mut fragments = vec![];
        let mut truncated = false;
        for mut hit in hits {
            let header = format!(
                "\n[{}:{} L{} {}:{}]\n",
                hit.source_ref.repository_id,
                hit.id,
                hit.level,
                hit.source_ref.path,
                hit.source_ref.line
            );
            let used = estimate_tokens(&text);
            let available = req.token_budget.saturating_sub(used);
            if estimate_tokens(&header) >= available {
                truncated = true;
                break;
            }
            let body = truncate_tokens(&hit.excerpt, available - estimate_tokens(&header));
            if body != hit.excerpt {
                truncated = true
            }
            hit.excerpt = body.clone();
            text.push_str(&header);
            text.push_str(&body);
            fragments.push(hit);
            if estimate_tokens(&text) >= req.token_budget {
                truncated = true;
                break;
            }
        }
        Ok(ContextBundle {
            used_tokens: estimate_tokens(&text),
            text,
            fragments,
            token_budget: req.token_budget,
            truncated,
            degraded: result.degraded,
            generation: result.generation,
        })
    }
}
