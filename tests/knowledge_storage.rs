use lumen::{agent::*, knowledge::*, retrieval::*, versioning::*, workspace::*};
use serde_json::json;
use std::{fs, sync::Arc};
use tokio_util::sync::CancellationToken;
fn c() -> CancellationToken {
    CancellationToken::new()
}
async fn init(path: &std::path::Path) -> WorkspaceHandle {
    WorkspaceService::init(
        InitRequest {
            root: path.into(),
            author_name: "Test".into(),
            author_email: "test@example.com".into(),
        },
        c(),
    )
    .await
    .unwrap()
}
fn object(kind: ObjectKind, title: &str, body: &str) -> KnowledgeObject {
    KnowledgeObject {
        metadata: ObjectMetadata::new(kind, title),
        body: body.into(),
    }
}
async fn save(w: &WorkspaceHandle, o: KnowledgeObject) -> ObjectView {
    w.knowledge().create(o, c()).await.unwrap()
}
async fn commit(w: &WorkspaceHandle, message: &str) -> CommitId {
    match w
        .versions()
        .commit(
            CommitRequest {
                message: message.into(),
            },
            c(),
        )
        .await
        .unwrap()
    {
        CommitOutcome::Created(id) => id,
        CommitOutcome::NoChanges => w.versions().status(c()).await.unwrap().head.unwrap(),
    }
}
async fn change(w: &WorkspaceHandle, id: &ObjectId, body: &str) {
    let view = w
        .knowledge()
        .read(id.clone(), ReadView::Full, c())
        .await
        .unwrap();
    w.knowledge()
        .update(
            UpdateObject {
                object: KnowledgeObject {
                    metadata: view.metadata,
                    body: body.into(),
                },
                expected_revision: view.revision,
            },
            c(),
        )
        .await
        .unwrap();
}
#[tokio::test]
async fn markdown_revision_graph_and_atomic_batch() {
    let dir = tempfile::tempdir().unwrap();
    let w = init(&dir.path().join("notes")).await;
    let domain = object(ObjectKind::Domain, "计算机科学", "");
    let mut topic = object(ObjectKind::Topic, "数据库事务", "");
    topic.metadata.member_of.push(domain.metadata.id.clone());
    let mut concept = object(
        ObjectKind::Concept,
        "事务隔离性",
        "# 事务隔离性\n\n并发事务之间的可见性。\n",
    );
    concept.metadata.member_of.push(topic.metadata.id.clone());
    concept
        .metadata
        .extensions
        .insert("custom".into(), json!({"nested":[1,"二"]}));
    let id = concept.metadata.id.clone();
    w.knowledge()
        .apply(
            ChangeSet {
                changes: vec![
                    ObjectChange::Create { object: domain },
                    ObjectChange::Create {
                        object: topic.clone(),
                    },
                    ObjectChange::Create { object: concept },
                ],
            },
            c(),
        )
        .await
        .unwrap();
    let view = w
        .knowledge()
        .read(id.clone(), ReadView::Full, c())
        .await
        .unwrap();
    assert_eq!(
        view.metadata.extensions["custom"],
        json!({"nested":[1,"二"]})
    );
    let stale = view.revision.clone();
    let updated = KnowledgeObject {
        metadata: view.metadata,
        body: "新解释".into(),
    };
    w.knowledge()
        .update(
            UpdateObject {
                object: updated.clone(),
                expected_revision: stale.clone(),
            },
            c(),
        )
        .await
        .unwrap();
    assert!(matches!(
        w.knowledge()
            .update(
                UpdateObject {
                    object: updated,
                    expected_revision: stale
                },
                c()
            )
            .await,
        Err(WorkspaceError::RevisionConflict(_))
    ));
    assert_eq!(
        w.knowledge()
            .relations(
                topic.metadata.id.clone(),
                RelationQuery {
                    incoming: true,
                    ..Default::default()
                },
                PageRequest::default(),
                c()
            )
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let mut a = object(ObjectKind::Concept, "A", "");
    let mut b = object(ObjectKind::Concept, "B", "");
    a.metadata.requires.push(b.metadata.id.clone());
    b.metadata.requires.push(a.metadata.id.clone());
    let apath = w.root().join(a.metadata.id.path());
    assert!(w
        .knowledge()
        .apply(
            ChangeSet {
                changes: vec![
                    ObjectChange::Create { object: a },
                    ObjectChange::Create { object: b }
                ]
            },
            c()
        )
        .await
        .is_err());
    assert!(!apath.exists());
    assert!(w
        .knowledge()
        .delete(
            DeleteObject {
                id: topic.metadata.id.clone(),
                expected_revision: format::revision(
                    &fs::read(w.root().join(topic.metadata.id.path())).unwrap()
                )
            },
            c()
        )
        .await
        .is_err());
    w.close(c()).await.unwrap();
    w.close(c()).await.unwrap();
}
#[tokio::test]
async fn lexical_scope_external_edits_and_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("notes");
    let w = init(&root).await;
    let topic = save(&w, object(ObjectKind::Topic, "数据库", "")).await;
    let mut o = object(
        ObjectKind::Concept,
        "隔离级别",
        "读已提交可以避免脏读。 Serializable isolation prevents anomalies.",
    );
    o.metadata.member_of.push(topic.metadata.id.clone());
    o.metadata.aliases.push("Isolation".into());
    let view = save(&w, o).await;
    let id = view.metadata.id.clone();
    let result = w
        .retrieval()
        .search(
            SearchRequest {
                query: "隔离级别".into(),
                scope: Some(topic.metadata.id),
                ..Default::default()
            },
            c(),
        )
        .await
        .unwrap();
    assert!(result.hits.iter().any(|h| h.id == id.as_str()));
    assert!(result.degraded.contains(&"lexical_only".into()));
    let context = w
        .retrieval()
        .context(
            ContextRequest {
                search: SearchRequest {
                    query: "Isolation".into(),
                    ..Default::default()
                },
                token_budget: 70,
                ..Default::default()
            },
            c(),
        )
        .await
        .unwrap();
    assert!(context.used_tokens <= 70);
    fs::write(root.join(id.path()), "broken frontmatter").unwrap();
    let result = w
        .retrieval()
        .search(
            SearchRequest {
                query: "隔离级别".into(),
                ..Default::default()
            },
            c(),
        )
        .await
        .unwrap();
    assert!(result.hits.iter().all(|h| h.id != id.as_str()));
    assert!(!result.diagnostics.is_empty());
    fs::write(
        root.join(id.path()),
        format::encode(&view.metadata, &view.content).unwrap(),
    )
    .unwrap();
    w.close(c()).await.unwrap();
    fs::remove_dir_all(root.join(".lumen")).unwrap();
    let w = WorkspaceService::open(root, c()).await.unwrap();
    assert!(!w
        .retrieval()
        .search(
            SearchRequest {
                query: "脏读".into(),
                ..Default::default()
            },
            c()
        )
        .await
        .unwrap()
        .hits
        .is_empty());
    w.close(c()).await.unwrap();
}
#[tokio::test]
async fn events_sessions_assets_and_immutable_history() {
    let dir = tempfile::tempdir().unwrap();
    let w = init(&dir.path().join("notes")).await;
    let concept = save(
        &w,
        object(ObjectKind::Concept, "原子性", "全部成功或全部失败"),
    )
    .await;
    let event = w
        .learning()
        .append(
            AppendEvent {
                event_kind: EventKind::Answer,
                occurred_at: None,
                targets: vec![concept.metadata.id.clone()],
                session_id: None,
                supersedes: None,
                data: json!({"outcome":"partial"}),
                body: "第一次回答".into(),
            },
            c(),
        )
        .await
        .unwrap();
    w.learning()
        .append(
            AppendEvent {
                event_kind: EventKind::Correction,
                occurred_at: None,
                targets: vec![concept.metadata.id.clone()],
                session_id: None,
                supersedes: Some(event.metadata.id.clone()),
                data: json!({"outcome":"correct"}),
                body: "更正".into(),
            },
            c(),
        )
        .await
        .unwrap();
    let state = w
        .learning()
        .state(vec![concept.metadata.id], c())
        .await
        .unwrap();
    assert_eq!(state[0].outcome.as_deref(), Some("correct"));
    assert_eq!(state[0].evidence_ids.len(), 1);
    let session_id = format!("session_{}", uuid::Uuid::now_v7());
    let segment = w.sessions().begin(session_id.clone(), c()).await.unwrap();
    w.sessions()
        .append(
            segment.segment_id.clone(),
            AgentMessage::User(vec![ContentPart::Text("Hello".into())]),
            c(),
        )
        .await
        .unwrap();
    w.sessions().finish(segment.segment_id, c()).await.unwrap();
    assert_eq!(
        w.sessions().load(session_id, c()).await.unwrap().segments[0]
            .messages
            .len(),
        1
    );
    let asset = w
        .resources()
        .add_asset(
            AddAsset {
                bytes: b"image-content".to_vec(),
                extension: "png".into(),
                media_type: "image/png".into(),
            },
            c(),
        )
        .await
        .unwrap();
    assert_eq!(
        w.resources()
            .read_asset(
                asset,
                ByteRange {
                    start: 0,
                    end: Some(5)
                },
                c()
            )
            .await
            .unwrap(),
        b"image"
    );
    commit(&w, "records").await;
    let path = w.root().join(event.path().unwrap());
    let text = fs::read_to_string(&path).unwrap();
    fs::write(path, text.replace("第一次回答", "悄悄修改")).unwrap();
    assert!(w
        .versions()
        .commit(
            CommitRequest {
                message: "invalid rewrite".into()
            },
            c()
        )
        .await
        .is_err());
    w.close(c()).await.unwrap();
}
#[tokio::test]
async fn two_devices_sync_merge_conflict_abort_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let remote_path = dir.path().join("remote.git");
    let bare = git2::Repository::init_bare(&remote_path).unwrap();
    bare.set_head("refs/heads/main").unwrap();
    drop(bare);
    let a = init(&dir.path().join("a")).await;
    a.versions()
        .set_remote(Some(remote_path.to_str().unwrap().into()), c())
        .await
        .unwrap();
    let concept = save(&a, object(ObjectKind::Concept, "Concurrency", "base\n")).await;
    let id = concept.metadata.id;
    let initial = a
        .versions()
        .sync(
            SyncRequest {
                message: Some("first".into()),
            },
            c(),
        )
        .await
        .unwrap();
    assert!(initial.error.is_none(), "{:?}", initial.error);
    let b = WorkspaceService::clone(
        CloneRequest {
            root: dir.path().join("b"),
            url: remote_path.to_str().unwrap().into(),
            config: LocalConfig {
                author_name: "B".into(),
                author_email: "b@example.com".into(),
                ..Default::default()
            },
        },
        c(),
    )
    .await
    .unwrap();
    save(&a, object(ObjectKind::Concept, "A only", "one")).await;
    a.versions()
        .sync(
            SyncRequest {
                message: Some("device A".into()),
            },
            c(),
        )
        .await
        .unwrap();
    save(&b, object(ObjectKind::Concept, "B only", "two")).await;
    let merged = b
        .versions()
        .sync(
            SyncRequest {
                message: Some("device B".into()),
            },
            c(),
        )
        .await
        .unwrap();
    assert!(merged.error.is_none(), "{:?}", merged.error);
    assert!(merged.merge.unwrap().merged);
    a.versions().pull(c()).await.unwrap();
    assert_eq!(
        a.knowledge()
            .list(ObjectFilter::default(), PageRequest::default(), c())
            .await
            .unwrap()
            .items
            .len(),
        3
    );
    let base = commit(&a, "checkpoint").await;
    change(&a, &id, "from A\n").await;
    let delta = a
        .versions()
        .diff(
            DiffRequest {
                object_id: Some(id.clone()),
                ..Default::default()
            },
            c(),
        )
        .await
        .unwrap();
    assert!(delta.files[0].patch.contains("+from A"));
    assert!(serde_json::to_value(&delta).unwrap()["files"][0]
        .get("old")
        .is_none());
    let r = a
        .versions()
        .sync(
            SyncRequest {
                message: Some("edit A".into()),
            },
            c(),
        )
        .await
        .unwrap();
    assert!(r.error.is_none());
    change(&b, &id, "from B\n").await;
    commit(&b, "edit B").await;
    let conflict = b.versions().pull(c()).await.unwrap();
    assert!(conflict.needs_resolution);
    assert!(!b.versions().conflicts(c()).await.unwrap().files.is_empty());
    assert!(matches!(
        b.knowledge().read(id.clone(), ReadView::Full, c()).await,
        Err(WorkspaceError::NeedsResolution)
    ));
    b.versions().abort_merge(c()).await.unwrap();
    assert_eq!(
        b.knowledge()
            .read(id.clone(), ReadView::Full, c())
            .await
            .unwrap()
            .content,
        "from B\n"
    );
    b.versions().pull(c()).await.unwrap();
    let view = a
        .knowledge()
        .read(id.clone(), ReadView::Full, c())
        .await
        .unwrap();
    let resolved = format::encode(&view.metadata, "resolved\n").unwrap();
    b.versions()
        .resolve(
            ResolveRequest {
                path: id.path(),
                content: Some(resolved),
            },
            c(),
        )
        .await
        .unwrap();
    b.versions().continue_merge(c()).await.unwrap();
    b.versions().push(c()).await.unwrap();
    a.versions().pull(c()).await.unwrap();
    let view = a
        .knowledge()
        .read(id.clone(), ReadView::Full, c())
        .await
        .unwrap();
    assert_eq!(view.content, "resolved\n");
    a.versions()
        .restore(
            RestoreRequest {
                commit: base,
                items: vec![RestoreItem {
                    id: id.clone(),
                    expected_revision: ExpectedRevision::Match(view.revision),
                }],
            },
            c(),
        )
        .await
        .unwrap();
    assert_eq!(
        a.knowledge()
            .read(id, ReadView::Full, c())
            .await
            .unwrap()
            .content,
        "base\n"
    );
    a.close(c()).await.unwrap();
    b.close(c()).await.unwrap();
}
struct TestEmbedder;
#[async_trait::async_trait]
impl Embedder for TestEmbedder {
    fn model_id(&self) -> &str {
        "test-v1"
    }
    async fn embed(
        &self,
        texts: Vec<String>,
        _: CancellationToken,
    ) -> WorkspaceResult<EmbeddingBatch> {
        Ok(EmbeddingBatch {
            model_id: "test-v1".into(),
            dimensions: 2,
            vectors: texts
                .into_iter()
                .map(|t| {
                    if t.contains("apple") {
                        vec![1.0, 0.0]
                    } else {
                        vec![0.0, 1.0]
                    }
                })
                .collect(),
        })
    }
}
#[tokio::test]
async fn embeddings_tools_and_context_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let w = init(&dir.path().join("notes")).await;
    let concept = save(&w, object(ObjectKind::Concept, "apple", "apple fruit")).await;
    w.set_embedder(Some(Arc::new(TestEmbedder)));
    w.index()
        .rebuild(IndexSelection::Vectors, c())
        .await
        .unwrap();
    let result = w
        .retrieval()
        .search(
            SearchRequest {
                query: "apple".into(),
                mode: SearchMode::Semantic,
                ..Default::default()
            },
            c(),
        )
        .await
        .unwrap();
    assert_eq!(result.hits[0].id, concept.metadata.id.as_str());
    for name in KNOWLEDGE_TOOLS {
        let tool = KnowledgeTool {
            name,
            workspace: w.clone(),
        };
        jsonschema::validator_for(&tool.input_schema()).unwrap();
    }
    let tool = KnowledgeTool {
        name: "knowledge_create",
        workspace: w.clone(),
    };
    let result = tool
        .execute(
            json!({"metadata":{"type":"concept","title":"工具创建"},"body":"新知识"}),
            ToolContext { cancel: c() },
        )
        .await
        .unwrap();
    assert!(!result.is_error);
    let mut agent = AgentLoop::new(Arc::new(MockProvider::new(vec![AssistantMessage {
        content: vec![ContentPart::Text("done".into())],
        tool_calls: vec![],
    }])));
    agent.register_plugin(Arc::new(RetrievalPlugin::new(w.clone())));
    let mut state = AgentState {
        messages: vec![],
        turn_count: 0,
    };
    agent
        .run(vec![ContentPart::Text("apple".into())], &mut state, c())
        .await
        .unwrap();
    assert_eq!(state.messages.len(), 2);
    assert!(!state
        .messages
        .iter()
        .any(|m| matches!(m, AgentMessage::System(_))));
    let cancelled = c();
    cancelled.cancel();
    assert!(matches!(
        w.knowledge()
            .read(concept.metadata.id, ReadView::Full, cancelled)
            .await,
        Err(WorkspaceError::Cancelled)
    ));
    w.close(c()).await.unwrap();
}
#[cfg(unix)]
#[tokio::test]
async fn rejects_symlink_escape() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let w = init(&dir.path().join("notes")).await;
    let outside = dir.path().join("outside");
    fs::write(&outside, b"untouched").unwrap();
    let id = ObjectId::new(ObjectKind::Concept);
    symlink(&outside, w.root().join(id.path())).unwrap();
    assert!(w.knowledge().read(id, ReadView::Full, c()).await.is_err());
    assert_eq!(fs::read(outside).unwrap(), b"untouched");
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn recovery_replays_transaction_and_recovers_only_orphan_segments() {
    let dir = tempfile::tempdir().unwrap();
    let w = init(&dir.path().join("notes")).await;
    let live_session = format!("session_{}", uuid::Uuid::now_v7());
    let live = w.sessions().begin(live_session, c()).await.unwrap();
    let second = WorkspaceService::open(w.root().to_path_buf(), c())
        .await
        .unwrap();
    assert!(w.root().join(format!("{}.open", live.path)).exists());
    second.close(c()).await.unwrap();
    w.sessions()
        .append(
            live.segment_id.clone(),
            AgentMessage::User(vec![ContentPart::Text("still live".into())]),
            c(),
        )
        .await
        .unwrap();
    w.sessions().finish(live.segment_id, c()).await.unwrap();
    let orphan_session = format!("session_{}", uuid::Uuid::now_v7());
    let orphan_segment = format!("segment_{}", uuid::Uuid::now_v7());
    let path = format!("sessions/{orphan_session}/{orphan_segment}.jsonl");
    fs::create_dir_all(w.root().join("sessions").join(&orphan_session)).unwrap();
    let mut bytes = serde_json::to_vec(&SessionRecord {
        message: AgentMessage::User(vec![ContentPart::Text("complete".into())]),
    })
    .unwrap();
    bytes.extend_from_slice(b"\n{\"incomplete");
    fs::write(w.root().join(format!("{path}.open")), bytes).unwrap();
    let o = object(ObjectKind::Concept, "Recovered", "pending content");
    let id = o.metadata.id.clone();
    let content = format::encode(&o.metadata, &o.body).unwrap();
    let tx = json!({"entries":[{"path":id.path(),"before":null,"after":content}]});
    fs::write(
        w.root().join(".lumen/transactions/pending.json"),
        serde_json::to_vec(&tx).unwrap(),
    )
    .unwrap();
    let recovered = w.recover(c()).await.unwrap();
    assert_eq!(recovered.transactions, 1);
    assert_eq!(recovered.sessions, 1);
    assert_eq!(
        w.knowledge()
            .read(id, ReadView::Full, c())
            .await
            .unwrap()
            .content,
        "pending content"
    );
    assert_eq!(
        w.sessions()
            .load(orphan_session, c())
            .await
            .unwrap()
            .segments[0]
            .messages
            .len(),
        1
    );
    w.close(c()).await.unwrap();
}
#[tokio::test]
async fn patch_distinguishes_clear_and_omission_and_crlf_is_readable() {
    let dir = tempfile::tempdir().unwrap();
    let w = init(&dir.path().join("notes")).await;
    let mut o = object(ObjectKind::Concept, "Patchable", "original body");
    o.metadata.summary = Some("short".into());
    o.metadata.aliases = vec!["keep alias".into()];
    o.metadata.extensions.insert("keep".into(), json!(1));
    let old = save(&w, o).await;
    let updated = w
        .knowledge()
        .patch(
            PatchObject {
                id: old.metadata.id.clone(),
                metadata_patch: serde_json::from_value(
                    json!({"summary":null,"extensions":{"new":2}}),
                )
                .unwrap(),
                body: Some(String::new()),
                expected_revision: old.revision,
            },
            c(),
        )
        .await
        .unwrap();
    assert!(updated.metadata.summary.is_none());
    assert_eq!(updated.metadata.aliases, vec!["keep alias"]);
    assert_eq!(updated.metadata.extensions.len(), 2);
    assert!(updated.content.is_empty());
    let encoded = format::encode(&updated.metadata, "# Heading\n\n正文\n").unwrap();
    let crlf = String::from_utf8(encoded).unwrap().replace('\n', "\r\n");
    let decoded = format::parse_object(&crlf).unwrap();
    assert_eq!(decoded.body, "# Heading\r\n\r\n正文\r\n");
    let tool = KnowledgeTool {
        name: "knowledge_update",
        workspace: w.clone(),
    };
    let result=tool.execute(json!({"id":updated.metadata.id,"metadata_patch":{"summary":"new"},"expected_revision":updated.revision}),ToolContext{cancel:c()}).await.unwrap();
    assert!(!result.is_error);
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn semantic_merge_errors_require_resolution_even_without_text_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let remote_path = dir.path().join("remote.git");
    git2::Repository::init_bare(&remote_path)
        .unwrap()
        .set_head("refs/heads/main")
        .unwrap();
    let a = init(&dir.path().join("a")).await;
    a.versions()
        .set_remote(Some(remote_path.to_str().unwrap().into()), c())
        .await
        .unwrap();
    let concept = save(&a, object(ObjectKind::Concept, "Prerequisite", "knowledge")).await;
    let report = a
        .versions()
        .sync(
            SyncRequest {
                message: Some("base".into()),
            },
            c(),
        )
        .await
        .unwrap();
    assert!(report.error.is_none());
    let b = WorkspaceService::clone(
        CloneRequest {
            root: dir.path().join("b"),
            url: remote_path.to_str().unwrap().into(),
            config: LocalConfig {
                author_name: "B".into(),
                author_email: "b@example.com".into(),
                ..Default::default()
            },
        },
        c(),
    )
    .await
    .unwrap();
    a.knowledge()
        .delete(
            DeleteObject {
                id: concept.metadata.id.clone(),
                expected_revision: concept.revision.clone(),
            },
            c(),
        )
        .await
        .unwrap();
    commit(&a, "remove unused concept").await;
    let mut question = object(ObjectKind::Question, "New check", "explain it");
    question.metadata.tests.push(concept.metadata.id.clone());
    save(&b, question).await;
    let report = b
        .versions()
        .sync(
            SyncRequest {
                message: Some("add check".into()),
            },
            c(),
        )
        .await
        .unwrap();
    assert!(report.error.is_none());
    let merged = a.versions().pull(c()).await.unwrap();
    assert!(merged.needs_resolution);
    let conflicts = a.versions().conflicts(c()).await.unwrap();
    assert!(conflicts.files.is_empty());
    assert!(!conflicts.diagnostics.is_empty());
    let encoded = format::encode(&concept.metadata, &concept.content).unwrap();
    a.versions()
        .resolve(
            ResolveRequest {
                path: concept.metadata.id.path(),
                content: Some(encoded),
            },
            c(),
        )
        .await
        .unwrap();
    a.versions().continue_merge(c()).await.unwrap();
    assert!(
        a.validate(ValidationScope::WorkingTree, c())
            .await
            .unwrap()
            .valid
    );
    a.close(c()).await.unwrap();
    b.close(c()).await.unwrap();
}

struct TestSummary;
#[async_trait::async_trait]
impl SummaryGenerator for TestSummary {
    fn id(&self) -> &str {
        "summary-test-v1"
    }
    async fn generate(&self, input: SummaryInput, _: CancellationToken) -> WorkspaceResult<String> {
        Ok(format!("Generated: {}", input.title))
    }
}
struct PendingEmbedder(Arc<tokio::sync::Notify>);
#[async_trait::async_trait]
impl Embedder for PendingEmbedder {
    fn model_id(&self) -> &str {
        "pending-test-v1"
    }
    async fn embed(&self, _: Vec<String>, _: CancellationToken) -> WorkspaceResult<EmbeddingBatch> {
        self.0.notify_one();
        std::future::pending().await
    }
}
#[tokio::test]
async fn summary_cache_is_readable_and_background_work_stops_on_close() {
    let dir = tempfile::tempdir().unwrap();
    let w = init(&dir.path().join("notes")).await;
    let concept = save(
        &w,
        object(ObjectKind::Concept, "Summary target", "Original source"),
    )
    .await;
    let original = fs::read(w.root().join(concept.metadata.id.path())).unwrap();
    w.set_summary_generator(Some(Arc::new(TestSummary)));
    let request = SummaryRequest {
        id: concept.metadata.id.clone(),
        level: 0,
        max_chars: None,
    };
    let result = w.summaries().generate(request, c()).await.unwrap();
    assert!(result.generated);
    assert_eq!(
        w.knowledge()
            .read(concept.metadata.id, ReadView::Abstract, c())
            .await
            .unwrap()
            .content,
        "Generated: Summary target"
    );
    assert_eq!(
        fs::read(w.root().join(result.source_ref.path)).unwrap(),
        original
    );
    let entered = Arc::new(tokio::sync::Notify::new());
    w.set_embedder(Some(Arc::new(PendingEmbedder(entered.clone()))));
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), w.close(c()))
        .await
        .unwrap()
        .unwrap();
}
