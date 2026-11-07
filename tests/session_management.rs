use async_trait::async_trait;
use lumen::{
    agent::*,
    context::{ContextConfig, ContextWindowPlugin},
    session::*,
    versioning::CommitRequest,
    workspace::{InitRequest, ValidationScope, WorkspaceError as E, WorkspaceService},
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
fn c() -> CancellationToken {
    CancellationToken::new()
}
fn text(s: &str) -> Vec<ContentPart> {
    vec![ContentPart::Text(s.into())]
}
fn metadata(title: &str) -> SessionMetadata {
    SessionMetadata::new(
        Some(title.into()),
        SessionModel::default(),
        SessionContext::default(),
    )
}
fn manager(dir: &std::path::Path) -> SessionManager {
    SessionManager::new(SessionRepository::standalone(dir).unwrap())
}
async fn run(manager: &SessionManager, input: &str) -> SessionRunResult {
    let handle = manager.active().unwrap();
    let runtime = handle.runtime().unwrap();
    let provider = Arc::new(MockProvider::new(vec![AssistantMessage {
        content: text("回答"),
        tool_calls: vec![],
    }]));
    let mut agent = AgentLoop::new(provider.clone());
    agent.register_plugin(Arc::new(ContextWindowPlugin::new(
        runtime,
        provider,
        ContextConfig::default(),
    )));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    SessionRunner::run(
        handle,
        agent,
        text(input),
        "core rules",
        c(),
        tx,
        manager.activation(),
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn stable_identity_switch_restore_metadata_and_ui_are_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let a = m.create(metadata("first")).await.unwrap();
    let aid = a.info.identity.session_id.clone();
    assert_eq!(a.info.message_count, 0);
    assert!(run(&m, "question alpha").await.primary_error.is_none());
    let ui = SessionUiState {
        draft: "first draft".into(),
        chat_scroll: 12,
        ..Default::default()
    };
    m.save_ui(&ui).unwrap();
    let b = m.create(metadata("second")).await.unwrap();
    assert_ne!(b.info.identity.session_id, aid);
    assert!(run(&m, "question beta").await.primary_error.is_none());
    let restored = m.switch(&aid, false, None).await.unwrap();
    assert_eq!(restored.ui.draft, "first draft");
    assert_eq!(restored.history.messages.len(), 2);
    assert!(restored.history.messages[0].text.contains("alpha"));
    let identity = restored.info.identity.clone();
    m.close(None).unwrap();
    drop(m);
    let mut fresh = manager(dir.path());
    let restored = fresh.restore_recent().await.unwrap().unwrap();
    assert_eq!(restored.info.identity, identity);
    assert_eq!(restored.ui.chat_scroll, 12);
    let runtime = fresh.active().unwrap().runtime().unwrap();
    assert!(runtime
        .append(AgentMessage::User(text("outside run")))
        .await
        .is_err());
    assert!(runtime.set_writer(None).await.is_err());
}
#[tokio::test]
async fn failed_switch_and_busy_lease_keep_original_session_usable() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = manager(dir.path());
    let mut b = manager(dir.path());
    let first = a.create(metadata("one")).await.unwrap();
    let second = b.create(metadata("two")).await.unwrap();
    assert!(matches!(
        a.switch(&second.info.identity.session_id, false, None)
            .await,
        Err(E::SessionBusy(_))
    ));
    assert_eq!(a.active().unwrap().identity, first.info.identity);
    assert!(matches!(
        a.switch(&format!("session_{}", uuid::Uuid::now_v7()), false, None)
            .await,
        Err(E::NotFound(_))
    ));
    assert_eq!(a.active().unwrap().identity, first.info.identity);
    assert!(run(&a, "still works").await.primary_error.is_none());
    b.close(None).unwrap();
    assert!(a
        .switch(&second.info.identity.session_id, false, None)
        .await
        .is_ok());
}
#[tokio::test]
async fn rename_archive_cas_search_pagination_and_index_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let first = m.create(metadata("original")).await.unwrap();
    let id = first.info.identity.session_id.clone();
    let renamed = m
        .rename(&id, "new title".into(), &first.info.revision)
        .await
        .unwrap();
    assert!(matches!(
        m.rename(&id, "stale".into(), &first.info.revision).await,
        Err(E::RevisionConflict(_))
    ));
    assert!(run(&m, "unique body needle").await.primary_error.is_none());
    let found = m
        .list(SessionQuery {
            query: "needle".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(found.items.len(), 1);
    let archived = m.archive(&id, true, &renamed.revision).await.unwrap();
    assert!(matches!(run_archived(&m).await, Err(E::SessionArchived(_))));
    assert!(m
        .list(SessionQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    m.archive(&id, false, &archived.revision).await.unwrap();
    m.create(metadata("second")).await.unwrap();
    let page = m
        .list(SessionQuery {
            limit: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(page.next_cursor.is_some());
    let next = m
        .list(SessionQuery {
            limit: 1,
            cursor: page.next_cursor.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(next.items.len(), 1);
    assert_ne!(page.items[0].identity, next.items[0].identity);
    assert!(matches!(
        m.list(SessionQuery {
            query: "changed".into(),
            cursor: page.next_cursor,
            ..Default::default()
        })
        .await,
        Err(E::CursorExpired)
    ));
    std::fs::remove_file(dir.path().join(".lumen/session-index.sqlite")).unwrap();
    assert_eq!(
        m.list(SessionQuery {
            query: "needle".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .len(),
        1
    );
}
async fn run_archived(m: &SessionManager) -> lumen::workspace::WorkspaceResult<SessionRunResult> {
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    SessionRunner::run(
        m.active().unwrap(),
        AgentLoop::new(Arc::new(MockProvider::new(vec![]))),
        text("x"),
        "core",
        c(),
        tx,
        m.activation(),
    )
    .await
}
#[tokio::test]
async fn run_records_are_removed_before_checkpoint_replay_and_hidden_from_history_search() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let view = m.create(metadata("history")).await.unwrap();
    for _ in 0..3 {
        assert!(run(&m, "visible question").await.primary_error.is_none());
    }
    let loaded = m
        .repository()
        .load(&view.info.identity.session_id)
        .await
        .unwrap();
    assert_eq!(loaded.segments.len(), 3);
    let report = replay(&loaded.metadata.id, &loaded.segments).unwrap();
    assert_eq!(report.health, SessionHealth::Complete);
    assert_eq!(report.history.len(), 6);
    assert!(report.records.iter().all(|record|!matches!(record,AgentMessage::Custom{kind,..}if kind=="lumen.segment_open"||kind=="lumen.run_end")));
    assert!(SessionRuntime::from_records(report.records, None).is_ok());
    let a = m.history(&loaded.metadata.id, None, 2).await.unwrap();
    let b = m
        .history(&loaded.metadata.id, a.before.as_deref(), 2)
        .await
        .unwrap();
    assert_ne!(
        history_key(&a.messages[0].record_ref),
        history_key(&b.messages[0].record_ref)
    );
    assert!(m
        .list(SessionQuery {
            query: "lumen.context_commit".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(m
        .list(SessionQuery {
            query: "core rules".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .is_empty());
}
#[tokio::test]
async fn forked_segment_chain_is_readable_but_not_writable() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let view = m.create(metadata("fork")).await.unwrap();
    run(&m, "q").await;
    let loaded = m
        .repository()
        .load(&view.info.identity.session_id)
        .await
        .unwrap();
    let original = &loaded.segments[0];
    let mut records = decode_records(&original.bytes).unwrap();
    let fork = format!("segment_{}", uuid::Uuid::now_v7());
    if let AgentMessage::Custom { content, .. } = &mut records[0] {
        content["segment_id"] = serde_json::json!(fork);
    }
    let mut bytes = vec![];
    for record in records {
        serde_json::to_writer(&mut bytes, &SessionRecord { message: record }).unwrap();
        bytes.push(b'\n');
    }
    std::fs::write(
        dir.path()
            .join(format!("sessions/{}/{fork}.jsonl", loaded.metadata.id)),
        bytes,
    )
    .unwrap();
    let info = m.inspect(&loaded.metadata.id).await.unwrap();
    assert_eq!(info.health, SessionHealth::Diverged);
    m.close(None).unwrap();
    assert!(matches!(
        m.switch(&loaded.metadata.id, false, None).await,
        Err(E::SessionUnavailable(_))
    ));
    let view = m.switch(&loaded.metadata.id, true, None).await.unwrap();
    assert!(!view.writable);
    assert_eq!(view.info.health, SessionHealth::Diverged);
    assert_eq!(view.history.messages.len(), 4);
}
#[tokio::test]
async fn corrupted_context_is_readonly_and_does_not_hide_other_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let good = m.create(metadata("good")).await.unwrap();
    let bad = m.create(metadata("bad")).await.unwrap();
    let segment = format!("segment_{}", uuid::Uuid::now_v7());
    let record = SessionRecord {
        message: AgentMessage::Custom {
            kind: "lumen.context_commit".into(),
            content: serde_json::json!({"schema_version":999}),
        },
    };
    std::fs::write(
        dir.path().join(format!(
            "sessions/{}/{segment}.jsonl",
            bad.info.identity.session_id
        )),
        format!("{}\n", serde_json::to_string(&record).unwrap()),
    )
    .unwrap();
    m.switch(&good.info.identity.session_id, false, None)
        .await
        .unwrap();
    assert!(m
        .switch(&bad.info.identity.session_id, false, None)
        .await
        .is_err());
    assert_eq!(m.active().unwrap().identity, good.info.identity);
    let view = m
        .switch(&bad.info.identity.session_id, true, None)
        .await
        .unwrap();
    assert!(!view.writable);
    let items = m.list(SessionQuery::default()).await.unwrap().items;
    assert_eq!(items.len(), 2);
    assert!(items
        .iter()
        .any(|item| item.health == SessionHealth::Corrupt));
}
#[tokio::test]
async fn legacy_import_preserves_source_and_checkpoints_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("old.jsonl");
    let old_id = format!("session_{}", uuid::Uuid::now_v7());
    let source_records = [
        AgentMessage::User(text("first old")),
        AgentMessage::Custom {
            kind: "lumen.session_start".into(),
            content: serde_json::json!({"session_id":old_id,"schema_version":1}),
        },
        AgentMessage::User(text("second old")),
    ];
    let bytes = source_records
        .iter()
        .map(|m| {
            format!(
                "{}\n",
                serde_json::to_string(&SessionRecord { message: m.clone() }).unwrap()
            )
        })
        .collect::<String>();
    std::fs::write(&source, &bytes).unwrap();
    let mut m = manager(&dir.path().join("store"));
    let ids = m.import_legacy(&source).await.unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[1], old_id);
    assert_eq!(m.import_legacy(&source).await.unwrap(), ids);
    assert_eq!(std::fs::read_to_string(source).unwrap(), bytes);
    assert_eq!(
        m.list(SessionQuery::default()).await.unwrap().items.len(),
        2
    );
    let view = m.switch(&old_id, false, None).await.unwrap();
    assert_eq!(view.history.messages[0].text, "second old");
    assert!(run(&m, "continue old").await.primary_error.is_none());
}
#[tokio::test]
async fn interrupted_open_segment_recovers_partial_tail_and_never_replays_tools() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let view = m.create(metadata("recover")).await.unwrap();
    let id = view.info.identity.session_id;
    let repo = m.repository().clone();
    let segment = repo.begin(&id).await.unwrap();
    let header = SegmentOpen {
        model: None,
        context: None,
        schema_version: 1,
        session_id: id.clone(),
        run_id: "interrupted".into(),
        segment_id: segment.clone(),
        parent: None,
        legacy_prefix: vec![],
        config_hash: "x".into(),
        started_at: now(),
    };
    repo.append(
        &id,
        &segment,
        &AgentMessage::Custom {
            kind: "lumen.segment_open".into(),
            content: serde_json::to_value(header).unwrap(),
        },
    )
    .await
    .unwrap();
    repo.append(&id, &segment, &AgentMessage::User(text("preserved")))
        .await
        .unwrap();
    use std::io::Write;
    let path = dir
        .path()
        .join(format!("sessions/{id}/{segment}.jsonl.open"));
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"{partial")
        .unwrap();
    m.close(None).unwrap();
    let restored = m.switch(&id, false, None).await.unwrap();
    assert_eq!(restored.info.health, SessionHealth::Interrupted);
    assert_eq!(restored.history.messages[0].text, "preserved");
    assert!(run(&m, "continue").await.primary_error.is_none());
    assert!(!dir
        .path()
        .join(format!("sessions/{id}/{segment}.jsonl.open"))
        .exists());
}
struct Hanging;
#[async_trait]
impl ModelProvider for Hanging {
    async fn stream(
        &self,
        _: ModelRequest,
        _: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn running_session_rejects_switch_and_archive_then_cancels_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let view = m.create(metadata("busy")).await.unwrap();
    let handle = m.active().unwrap();
    let runtime = handle.runtime().unwrap();
    let provider = Arc::new(Hanging);
    let mut agent = AgentLoop::new(provider.clone());
    agent.register_plugin(Arc::new(ContextWindowPlugin::new(
        runtime,
        provider,
        ContextConfig::default(),
    )));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let h = handle.clone();
    let task = tokio::spawn(async move {
        SessionRunner::run(h, agent, text("question"), "core", c(), tx, 1).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .unwrap();
    assert_eq!(handle.phase(), SessionPhase::Running);
    assert!(matches!(
        m.create(metadata("cannot create")).await,
        Err(E::SessionBusy(_))
    ));
    assert!(matches!(
        m.archive(&view.info.identity.session_id, true, &view.info.revision)
            .await,
        Err(E::SessionBusy(_))
    ));
    assert!(m.close(None).is_err());
    assert!(handle.cancel(Some("wrong-run")).is_err());
    handle.cancel(None).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.result.unwrap().reason, StopReason::Aborted);
    assert!(result.cleanup_error.is_none());
    assert_eq!(handle.phase(), SessionPhase::Ready);
}
#[tokio::test]
async fn workspace_metadata_is_mutable_but_closed_segments_remain_immutable() {
    let dir = tempfile::tempdir().unwrap();
    let w = WorkspaceService::init(
        InitRequest {
            root: dir.path().join("knowledge"),
            author_name: "test".into(),
            author_email: "test@example.com".into(),
        },
        c(),
    )
    .await
    .unwrap();
    let mut m = SessionManager::new(SessionRepository::workspace(w.clone()).await.unwrap());
    let view = m.create(metadata("before")).await.unwrap();
    assert!(run(&m, "workspace q").await.primary_error.is_none());
    w.validate(ValidationScope::WorkingTree, c()).await.unwrap();
    w.versions()
        .commit(
            CommitRequest {
                message: "initial session".into(),
            },
            c(),
        )
        .await
        .unwrap();
    let loaded = m
        .repository()
        .load(&view.info.identity.session_id)
        .await
        .unwrap();
    let path = w.root().join(format!(
        "sessions/{}/{}.jsonl",
        loaded.metadata.id, loaded.segments[0].id
    ));
    let original = std::fs::read(&path).unwrap();
    m.rename(&loaded.metadata.id, "after".into(), &view.info.revision)
        .await
        .unwrap();
    w.versions()
        .commit(
            CommitRequest {
                message: "rename session".into(),
            },
            c(),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::write(&path, b"{}\n").unwrap();
    assert!(w
        .versions()
        .commit(
            CommitRequest {
                message: "illegal rewrite".into()
            },
            c()
        )
        .await
        .is_err());
    m.close(None).unwrap();
    w.close(c()).await.unwrap();
}

struct CleanupFailure {
    root: std::path::PathBuf,
    id: String,
}
#[async_trait]
impl ModelProvider for CleanupFailure {
    async fn stream(
        &self,
        _: ModelRequest,
        _: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        let directory = self.root.join("sessions").join(&self.id);
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.to_string_lossy().ends_with(".jsonl.open") {
                std::fs::create_dir(path.with_extension("")).unwrap();
            }
        }
        Err(AgentError::Model("primary model failure".into()))
    }
}
#[tokio::test]
async fn cleanup_failure_does_not_hide_primary_error_and_faulted_handle_can_be_reopened() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let view = m.create(metadata("fail cleanup")).await.unwrap();
    let handle = m.active().unwrap();
    let provider = Arc::new(CleanupFailure {
        root: dir.path().into(),
        id: view.info.identity.session_id.clone(),
    });
    let mut agent = AgentLoop::new(provider.clone());
    agent.register_plugin(Arc::new(ContextWindowPlugin::new(
        handle.runtime().unwrap(),
        provider,
        ContextConfig::default(),
    )));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let result = SessionRunner::run(
        handle.clone(),
        agent,
        text("question"),
        "core",
        c(),
        tx,
        m.activation(),
    )
    .await
    .unwrap();
    assert!(result
        .primary_error
        .unwrap()
        .contains("primary model failure"));
    assert!(result.cleanup_error.is_some());
    assert_eq!(handle.phase(), SessionPhase::Faulted);
    for entry in std::fs::read_dir(
        dir.path()
            .join("sessions")
            .join(&view.info.identity.session_id),
    )
    .unwrap()
    {
        let path = entry.unwrap().path();
        if path.is_dir() {
            std::fs::remove_dir(path).unwrap();
        }
    }
    let restored = m
        .switch(&view.info.identity.session_id, false, None)
        .await
        .unwrap();
    assert!(restored.writable);
    assert!(run(&m, "retry without replaying prior tools")
        .await
        .primary_error
        .is_none());
}
#[tokio::test]
async fn empty_existing_session_is_distinct_from_missing_and_bad_recent_pointer_does_not_block_catalog(
) {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let empty = m.create(metadata("empty")).await.unwrap();
    assert!(m.view().await.unwrap().history.messages.is_empty());
    assert!(m
        .repository()
        .load(&empty.info.identity.session_id)
        .await
        .is_ok());
    m.close(None).unwrap();
    let missing = format!("session_{}", uuid::Uuid::now_v7());
    m.repository().set_last(&missing).unwrap();
    assert!(m.restore_recent().await.unwrap().is_none());
    assert!(!m.startup_diagnostics.is_empty());
    assert_eq!(
        m.list(SessionQuery::default()).await.unwrap().items.len(),
        1
    );
}
#[tokio::test]
async fn legacy_preview_is_readonly_and_complete_before_import() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.jsonl");
    let bytes = format!(
        "{}\n{}\n",
        serde_json::to_string(&SessionRecord {
            message: AgentMessage::System("hidden core".into())
        })
        .unwrap(),
        serde_json::to_string(&SessionRecord {
            message: AgentMessage::User(text("visible legacy"))
        })
        .unwrap()
    );
    std::fs::write(&path, &bytes).unwrap();
    let views = preview_legacy(&path).unwrap();
    assert_eq!(views.len(), 1);
    assert!(!views[0].writable);
    assert_eq!(views[0].history.messages.len(), 1);
    assert_eq!(views[0].history.messages[0].text, "visible legacy");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
}

#[tokio::test]
async fn close_invalidates_old_handles_and_releases_lease_even_with_external_arc() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = manager(dir.path());
    let view = a.create(metadata("close")).await.unwrap();
    let old = a.active().unwrap();
    a.close(None).unwrap();
    assert_eq!(old.phase(), SessionPhase::Closed);
    assert!(old.runtime().is_err());
    let mut b = manager(dir.path());
    assert!(b
        .switch(&view.info.identity.session_id, false, None)
        .await
        .is_ok());
}
#[tokio::test]
async fn legacy_working_segments_continue_without_rewriting_context_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old-context.jsonl");
    let provider = Arc::new(MockProvider::new(vec![AssistantMessage {
        content: text("old answer"),
        tool_calls: vec![],
    }]));
    {
        let runtime = SessionRuntime::open_file(&path).unwrap();
        let mut agent = AgentLoop::new(provider.clone()).with_session(runtime.clone());
        agent.register_plugin(Arc::new(ContextWindowPlugin::new(
            runtime,
            provider,
            ContextConfig::default(),
        )));
        agent
            .run(
                text("old question"),
                &mut AgentState {
                    messages: vec![],
                    turn_count: 0,
                },
                c(),
            )
            .await
            .unwrap();
    }
    let original = std::fs::read(&path).unwrap();
    let mut m = manager(&dir.path().join("directory"));
    let ids = m.import_legacy(&path).await.unwrap();
    m.switch(&ids[0], false, None).await.unwrap();
    assert!(run(&m, "continue checkpoint").await.primary_error.is_none());
    let report = replay(
        &ids[0],
        &m.repository().load(&ids[0]).await.unwrap().segments,
    )
    .unwrap();
    assert_eq!(report.health, SessionHealth::Complete);
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[tokio::test]
async fn corrupt_complete_line_exposes_valid_prefix_only_and_disables_continuation() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let view = m.create(metadata("corrupt line")).await.unwrap();
    let id = view.info.identity.session_id;
    let segment = format!("segment_{}", uuid::Uuid::now_v7());
    let bytes = format!(
        "{}\n{{invalid}}\n{}\n",
        serde_json::to_string(&SessionRecord {
            message: AgentMessage::User(text("readable prefix"))
        })
        .unwrap(),
        serde_json::to_string(&SessionRecord {
            message: AgentMessage::User(text("must not skip corruption"))
        })
        .unwrap()
    );
    std::fs::write(
        dir.path().join(format!("sessions/{id}/{segment}.jsonl")),
        bytes,
    )
    .unwrap();
    m.close(None).unwrap();
    let view = m.switch(&id, true, None).await.unwrap();
    assert!(!view.writable);
    assert_eq!(view.history.messages.len(), 1);
    assert_eq!(view.history.messages[0].text, "readable prefix");
    assert!(m.switch(&id, false, None).await.is_err());
}
#[test]
#[ignore = "invoked by session_lease_is_enforced_in_another_process"]
fn session_lock_child_process() {
    let root = std::env::var_os("LUMEN_TEST_SESSION_LEASE_ROOT").unwrap();
    let id = std::env::var("LUMEN_TEST_SESSION_LEASE_ID").unwrap();
    let repo = SessionRepository::standalone(std::path::PathBuf::from(root)).unwrap();
    assert!(matches!(repo.lease(&id), Err(E::SessionBusy(_))));
}
#[tokio::test]
async fn session_lease_is_enforced_in_another_process() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manager(dir.path());
    let view = m.create(metadata("cross process")).await.unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "session_lock_child_process", "--ignored"])
        .env("LUMEN_TEST_SESSION_LEASE_ROOT", dir.path())
        .env("LUMEN_TEST_SESSION_LEASE_ID", view.info.identity.session_id)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
