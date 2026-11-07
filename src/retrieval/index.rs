use super::*;
use crate::workspace::{check_cancel, files, State, WorkspaceEventKind};
use rusqlite::{params, Connection};
use std::path::Path;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IndexStatus {
    pub generation: String,
    pub files_ready: bool,
    pub lexical_ready: bool,
    pub semantic_ready: bool,
    pub pending: usize,
    pub diagnostics: Vec<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexSelection {
    All,
    Lexical,
    Relations,
    LearningState,
    Vectors,
    Summaries,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ChangeScope {
    pub paths: Vec<String>,
}
#[derive(Clone)]
pub struct IndexService(pub(crate) WorkspaceHandle);
pub(crate) fn connect(root: &Path) -> R<Connection> {
    let db = Connection::open(files::safe_path(root, ".lumen/index.sqlite")?)?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
CREATE TABLE IF NOT EXISTS index_state(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS documents(id TEXT PRIMARY KEY,path TEXT NOT NULL,kind TEXT NOT NULL,title TEXT NOT NULL,metadata TEXT NOT NULL,revision TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS edges(source TEXT,target TEXT,kind TEXT,typed INTEGER);
CREATE INDEX IF NOT EXISTS edges_target ON edges(target);
CREATE TABLE IF NOT EXISTS chunks(id TEXT PRIMARY KEY,document_id TEXT NOT NULL,heading TEXT NOT NULL,text TEXT NOT NULL,level INTEGER NOT NULL,line INTEGER NOT NULL,revision TEXT NOT NULL);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(id UNINDEXED,terms);
CREATE TABLE IF NOT EXISTS summaries(cache_key TEXT PRIMARY KEY,body TEXT NOT NULL,generator TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS embeddings(chunk_id TEXT,revision TEXT,model TEXT,dimensions INTEGER,vector BLOB,PRIMARY KEY(chunk_id,model));
CREATE TABLE IF NOT EXISTS learning_state(id TEXT PRIMARY KEY,state TEXT NOT NULL);")?;
    Ok(db)
}
const INDEX_VERSION: &str = "1:chunks-v1:han-bigram-v1";
pub(crate) fn refresh(
    s: &mut State,
    tree: &files::FileTree,
    cancel: &CancellationToken,
) -> R<IndexStatus> {
    check_cancel(cancel)?;
    let generation = files::fingerprint(tree);
    let mut db = connect(&s.root)?;
    let old = db
        .query_row(
            "SELECT value FROM index_state WHERE key='generation'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok();
    let old_version = db
        .query_row(
            "SELECT value FROM index_state WHERE key='schema'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok();
    let catalog = Catalog::from_tree(tree);
    let diagnostics = catalog
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.path, d.message))
        .collect::<Vec<_>>();
    if old.as_deref() == Some(&generation) && old_version.as_deref() == Some(INDEX_VERSION) {
        let count = db.query_row("SELECT count(*) FROM chunks", [], |r| {
            r.get::<_, i64>(0).map(|n| n as usize)
        })?;
        let embedded=db.query_row("SELECT count(DISTINCT chunk_id) FROM embeddings WHERE chunk_id IN(SELECT id FROM chunks)",[],|r|r.get::<_,i64>(0).map(|n|n as usize))?;
        return Ok(IndexStatus {
            generation,
            files_ready: true,
            lexical_ready: true,
            semantic_ready: count > 0 && embedded == count,
            pending: count.saturating_sub(embedded),
            diagnostics,
        });
    }
    s.emit(WorkspaceEventKind::IndexProgress, Some(generation.clone()));
    let tx = db.transaction()?;
    if old_version.as_deref() != Some(INDEX_VERSION) {
        tx.execute("DELETE FROM embeddings", [])?;
    }
    tx.execute_batch("DELETE FROM documents;DELETE FROM edges;DELETE FROM chunks;DELETE FROM chunks_fts;DELETE FROM learning_state;")?;
    for (o, rev, path) in catalog.objects.values() {
        check_cancel(cancel)?;
        let id = o.metadata.id.as_str();
        tx.execute(
            "INSERT INTO documents VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id,
                path,
                o.metadata.kind.name(),
                o.metadata.title,
                serde_json::to_string(&o.metadata)?,
                rev.0
            ],
        )?;
        for rel in o.metadata.relations() {
            tx.execute(
                "INSERT INTO edges VALUES(?1,?2,?3,1)",
                params![rel.source.as_str(), rel.target.as_str(), rel.kind],
            )?;
        }
        for event in pulldown_cmark::Parser::new(&o.body) {
            if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link { dest_url, .. }) = event
            {
                if let Some(target) = dest_url.strip_prefix("lumen:") {
                    if ObjectId::parse(target).is_ok() {
                        tx.execute(
                            "INSERT INTO edges VALUES(?1,?2,'link',0)",
                            params![id, target],
                        )?;
                    }
                }
            }
        }
        let raw = tree.get(path).unwrap();
        let prefix = raw.len().saturating_sub(o.body.len());
        let line = raw[..prefix].iter().filter(|b| **b == b'\n').count() + 1;
        let abstracts = catalog
            .view(
                &o.metadata.id,
                ReadView::Abstract,
                &s.repository.repository_id,
            )?
            .content;
        insert_chunk(
            &tx,
            id,
            0,
            0,
            "",
            &abstracts,
            1,
            rev,
            &o.metadata.title,
            &o.metadata.aliases,
        )?;
        if matches!(
            o.metadata.kind,
            ObjectKind::Domain | ObjectKind::Topic | ObjectKind::Unit
        ) {
            let overview = catalog
                .view(
                    &o.metadata.id,
                    ReadView::Overview,
                    &s.repository.repository_id,
                )?
                .content;
            insert_chunk(
                &tx,
                id,
                1,
                0,
                "",
                &overview,
                1,
                rev,
                &o.metadata.title,
                &o.metadata.aliases,
            )?;
        }
        for (i, chunk) in chunks(&o.body, line).into_iter().enumerate() {
            insert_chunk(
                &tx,
                id,
                2,
                i,
                &chunk.0,
                &chunk.1,
                chunk.2,
                rev,
                &o.metadata.title,
                &o.metadata.aliases,
            )?;
        }
    }
    for (e, rev, path) in &catalog.events {
        let id = &e.metadata.id;
        tx.execute(
            "INSERT INTO documents VALUES(?1,?2,'learning_event',?1,?3,?4)",
            params![id, path, serde_json::to_string(&e.metadata)?, rev.0],
        )?;
        insert_chunk(&tx, id, 2, 0, "", &e.body, 1, rev, id, &[])?;
    }
    for (path, bytes) in tree {
        if path.starts_with("sessions/")
            && path.ends_with(".jsonl")
            && crate::workspace::validate_segment(path, bytes).is_ok()
        {
            let rev = format::revision(bytes);
            let id = path
                .trim_end_matches(".jsonl")
                .rsplit('/')
                .next()
                .unwrap_or(path);
            let text = crate::session::decode_records(bytes)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|message| match message {
                    crate::agent::AgentMessage::User(parts) => Some(parts),
                    crate::agent::AgentMessage::Assistant(a) => Some(a.content),
                    _ => None,
                })
                .flatten()
                .map(|crate::agent::ContentPart::Text(text)| text)
                .collect::<Vec<_>>()
                .join("\n");
            tx.execute(
                "INSERT INTO documents VALUES(?1,?2,'session',?1,'{}',?3)",
                params![id, path, rev.0],
            )?;
            for (i, chunk) in chunks(&text, 1).into_iter().enumerate() {
                insert_chunk(&tx, id, 2, i, &chunk.0, &chunk.1, chunk.2, &rev, id, &[])?;
            }
        }
    }
    for state in crate::knowledge::derive_state(&catalog, catalog.objects.keys().cloned().collect())
    {
        tx.execute(
            "INSERT INTO learning_state VALUES(?1,?2)",
            params![state.object_id.as_str(), serde_json::to_string(&state)?],
        )?;
    }
    tx.execute("DELETE FROM embeddings WHERE NOT EXISTS(SELECT 1 FROM chunks c WHERE c.id=embeddings.chunk_id AND c.revision=embeddings.revision)",[])?;
    tx.execute(
        "INSERT OR REPLACE INTO index_state VALUES('generation',?1)",
        [&generation],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO index_state VALUES('schema',?1)",
        [INDEX_VERSION],
    )?;
    tx.commit()?;
    s.emit(WorkspaceEventKind::IndexReady, Some(generation.clone()));
    let pending=db.query_row("SELECT count(*) FROM chunks c WHERE NOT EXISTS(SELECT 1 FROM embeddings e WHERE e.chunk_id=c.id)",[],|r|r.get::<_,i64>(0).map(|n|n as usize))?;
    Ok(IndexStatus {
        generation,
        files_ready: true,
        lexical_ready: true,
        semantic_ready: pending == 0,
        pending,
        diagnostics,
    })
}
#[allow(clippy::too_many_arguments)]
fn insert_chunk(
    db: &Connection,
    id: &str,
    level: u8,
    n: usize,
    heading: &str,
    text: &str,
    line: usize,
    rev: &Revision,
    title: &str,
    aliases: &[String],
) -> R<()> {
    let chunk_id = format!("{id}:{level}:{n}");
    db.execute(
        "INSERT INTO chunks VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![chunk_id, id, heading, text, level, line as i64, rev.0],
    )?;
    let terms = tokenize(&format!("{title} {} {text}", aliases.join(" "))).join(" ");
    db.execute(
        "INSERT INTO chunks_fts(id,terms) VALUES(?1,?2)",
        params![chunk_id, terms],
    )?;
    Ok(())
}
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut word = String::new();
    let mut last = None;
    for c in text.chars() {
        if matches!(c as u32,0x3400..=0x9fff|0x20000..=0x3134f) {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word))
            }
            out.push(c.to_string());
            if let Some(p) = last {
                out.push(format!("{p}{c}"))
            }
            last = Some(c);
        } else {
            last = None;
            if c.is_alphanumeric() {
                word.extend(c.to_lowercase())
            } else if !word.is_empty() {
                out.push(std::mem::take(&mut word))
            }
        }
    }
    if !word.is_empty() {
        out.push(word)
    }
    out
}
fn chunks(text: &str, start_line: usize) -> Vec<(String, String, usize)> {
    let mut sections = vec![];
    let mut heading = String::new();
    let mut start = 0;
    let mut line = start_line;
    for (event, range) in pulldown_cmark::Parser::new(text).into_offset_iter() {
        if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Heading { .. }) = event {
            if range.start > start {
                sections.push((heading.clone(), text[start..range.start].to_string(), line));
            }
            start = range.start;
            line = start_line + text[..start].bytes().filter(|b| *b == b'\n').count();
            heading = text[range.clone()]
                .lines()
                .next()
                .unwrap_or("")
                .trim_start_matches('#')
                .trim()
                .into();
        }
    }
    if start < text.len() {
        sections.push((heading, text[start..].into(), line));
    }
    if sections.is_empty() {
        sections.push((String::new(), String::new(), start_line));
    }
    let mut result = vec![];
    for (h, section, l) in sections {
        if estimate_tokens(&section) <= 800 {
            result.push((h, section, l));
            continue;
        }
        let chars = section.char_indices().collect::<Vec<_>>();
        let mut first = 0;
        while first < chars.len() {
            let mut end = first;
            let mut tokens = 0.0;
            while end < chars.len() {
                let n = if chars[end].1.is_ascii() { 0.25 } else { 1.0 };
                if tokens + n > 800.0 {
                    break;
                }
                tokens += n;
                end += 1;
            }
            if end == first {
                end += 1
            }
            let a = chars[first].0;
            let b = chars.get(end).map(|x| x.0).unwrap_or(section.len());
            result.push((
                h.clone(),
                section[a..b].into(),
                l + section[..a].bytes().filter(|b| *b == b'\n').count(),
            ));
            if end == chars.len() {
                break;
            }
            let mut overlap = 0.0;
            let mut next = end;
            while next > first + 1 && overlap < 100.0 {
                next -= 1;
                overlap += if chars[next].1.is_ascii() { 0.25 } else { 1.0 };
            }
            first = next;
        }
    }
    result
}
impl IndexService {
    pub async fn status(&self, cancel: CancellationToken) -> R<IndexStatus> {
        self.refresh(ChangeScope::default(), cancel).await
    }
    pub async fn refresh(&self, _scope: ChangeScope, cancel: CancellationToken) -> R<IndexStatus> {
        let model = self
            .0
            .providers
            .embedder
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|e| e.model_id().to_string());
        self.0
            .call(cancel, move |s, c| {
                s.ensure_writable()?;
                let tree = files::scan(&s.root)?;
                let status = refresh(s, &tree, c)?;
                model_status(&s.root, status, model.as_deref())
            })
            .await
    }
    pub async fn rebuild(
        &self,
        selection: IndexSelection,
        cancel: CancellationToken,
    ) -> R<IndexStatus> {
        self.0
            .call(cancel.clone(), move |s, c| {
                s.ensure_writable()?;
                let db = connect(&s.root)?;
                if matches!(selection, IndexSelection::All | IndexSelection::Vectors) {
                    db.execute("DELETE FROM embeddings", [])?;
                }
                if matches!(selection, IndexSelection::All | IndexSelection::Summaries) {
                    db.execute("DELETE FROM summaries", [])?;
                }
                db.execute("DELETE FROM index_state WHERE key='generation'", [])?;
                let tree = files::scan(&s.root)?;
                refresh(s, &tree, c)
            })
            .await?;
        if matches!(selection, IndexSelection::All | IndexSelection::Vectors) {
            let embedder = self
                .0
                .providers
                .embedder
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(embedder) = embedder {
                self.embed_missing(embedder, cancel.clone()).await?;
            }
        }
        self.status(cancel).await
    }
    pub async fn embed_missing(
        &self,
        embedder: Arc<dyn Embedder>,
        cancel: CancellationToken,
    ) -> R<()> {
        let _gate = tokio::select! {
            _=cancel.cancelled()=>return Err(E::Cancelled),
            gate=self.0.embedding_gate.lock()=>gate,
        };
        loop {
            let model = embedder.model_id().to_owned();
            let batch=self.0.call(cancel.clone(), move |s,_| {
                s.ensure_writable()?;
                let db=connect(&s.root)?;
                let mut statement=db.prepare("SELECT c.id,c.revision,c.text FROM chunks c WHERE NOT EXISTS(SELECT 1 FROM embeddings e WHERE e.chunk_id=c.id AND e.revision=c.revision AND e.model=?1) LIMIT 32")?;
                let rows=statement.query_map([model], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?;
                Ok(rows.collect::<Result<Vec<_>,_>>()?)
            }).await?;
            if batch.is_empty() {
                return Ok(());
            }
            let vectors = embedder
                .embed(batch.iter().map(|x| x.2.clone()).collect(), cancel.clone())
                .await?;
            if vectors.model_id != embedder.model_id()
                || vectors.vectors.len() != batch.len()
                || vectors.dimensions == 0
            {
                return Err(E::InvalidFormat("embedding model/batch mismatch".into()));
            }
            let dimensions = vectors.dimensions;
            let model = vectors.model_id;
            let vectors = vectors
                .vectors
                .into_iter()
                .map(|v| normalize(v, dimensions))
                .collect::<R<Vec<_>>>()?;
            self.0.call(cancel.clone(), move |s,_| {
                s.ensure_writable()?;
                let mut db=connect(&s.root)?;
                let tx=db.transaction()?;
                use rusqlite::OptionalExtension;
                let old=tx.query_row("SELECT dimensions FROM embeddings WHERE model=?1 LIMIT 1",[&model],|r|r.get::<_,i64>(0)).optional()?;
                if old.is_some_and(|n|n!=dimensions as i64){return Err(E::InvalidFormat("dimensions changed without a new model ID".into()))}
                for ((id,revision,_),vector) in batch.into_iter().zip(vectors) {
                    let bytes=vector.iter().flat_map(|n|n.to_le_bytes()).collect::<Vec<_>>();
                    tx.execute("INSERT OR REPLACE INTO embeddings SELECT ?1,?2,?3,?4,?5 WHERE EXISTS(SELECT 1 FROM chunks WHERE id=?1 AND revision=?2)",params![id,revision,model,dimensions as i64,bytes])?;
                }
                tx.commit()?;
                Ok(())
            }).await?;
        }
    }
}

pub(crate) fn normalize(mut v: Vec<f32>, dimensions: usize) -> R<Vec<f32>> {
    if v.len() != dimensions || v.iter().any(|n| !n.is_finite()) {
        return Err(E::InvalidFormat(
            "invalid embedding dimensions/value".into(),
        ));
    }
    let norm = v.iter().map(|n| (*n as f64).powi(2)).sum::<f64>().sqrt();
    if norm == 0.0 {
        return Err(E::InvalidFormat("zero embedding".into()));
    }
    for n in &mut v {
        *n = (*n as f64 / norm) as f32;
    }
    Ok(v)
}

pub(crate) fn model_status(
    root: &Path,
    mut status: IndexStatus,
    model: Option<&str>,
) -> R<IndexStatus> {
    let db = connect(root)?;
    match model {
        None => {
            status.semantic_ready = false;
            status.pending = 0;
        }
        Some(model) => {
            let count =
                db.query_row("SELECT count(*) FROM chunks", [], |r| r.get::<_, i64>(0))? as usize;
            let ready=db.query_row("SELECT count(*) FROM chunks c WHERE EXISTS(SELECT 1 FROM embeddings e WHERE e.chunk_id=c.id AND e.revision=c.revision AND e.model=?1)",[model],|r|r.get::<_,i64>(0))? as usize;
            status.pending = count.saturating_sub(ready);
            status.semantic_ready = status.pending == 0;
        }
    }
    Ok(status)
}
