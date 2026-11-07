use async_trait::async_trait;
use futures_util::stream;
use lumen::{agent::*, context::*, skills::SkillRegistry};
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;
fn c() -> CancellationToken {
    CancellationToken::new()
}
fn user(s: &str) -> Vec<ContentPart> {
    vec![ContentPart::Text(s.into())]
}
fn answer(s: &str) -> AssistantMessage {
    AssistantMessage {
        content: user(s),
        tool_calls: vec![],
    }
}
fn state() -> AgentState {
    AgentState {
        messages: vec![],
        turn_count: 0,
    }
}
#[derive(Default)]
struct Recording {
    requests: Mutex<Vec<ModelRequest>>,
    answers: Mutex<VecDeque<AssistantMessage>>,
}
#[async_trait]
impl ModelProvider for Recording {
    async fn stream(
        &self,
        request: ModelRequest,
        _: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        self.requests.lock().unwrap().push(request);
        let a = self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| answer("done"));
        Ok(Box::pin(stream::iter(vec![ModelEvent::Complete(a)])))
    }
}
struct ShortSummary;
#[async_trait]
impl CompactionService for ShortSummary {
    async fn summarize(
        &self,
        _: ModelRequest,
        _: usize,
        _: CancellationToken,
    ) -> Result<CompactionOutput, AgentError> {
        Ok(CompactionOutput::text("Prior explanations complete. Unresolved questions and evidence remain in the transcript.".into()))
    }
}
struct BrokenSummary;
#[async_trait]
impl CompactionService for BrokenSummary {
    async fn summarize(
        &self,
        _: ModelRequest,
        _: usize,
        _: CancellationToken,
    ) -> Result<CompactionOutput, AgentError> {
        Err(AgentError::Model("offline".into()))
    }
}
fn config() -> ContextConfig {
    ContextConfig {
        context_tokens: 20_000,
        output_tokens: 256,
        tool_reserve: 1024,
        safety_tokens: 512,
        summary_tokens: 512,
        ..Default::default()
    }
}
fn agent(session: Arc<SessionRuntime>, provider: Arc<Recording>) -> AgentLoop {
    let mut agent = AgentLoop::new(provider.clone()).with_session(session.clone());
    agent.register_plugin(Arc::new(
        ContextWindowPlugin::new(session, provider, config())
            .with_compactor(Arc::new(ShortSummary)),
    ));
    agent
}
struct Reference {
    content: String,
    required: bool,
}
#[async_trait]
impl AgentPlugin for Reference {
    async fn before_request(
        &self,
        event: &mut BeforeRequest<'_>,
        _: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        if let BeforeRequest::Collect(c) = event {
            c.offer(ContextContribution {
                id: "reference".into(),
                region: ContextRegion::Current,
                source: "test-source".into(),
                revision: "v1".into(),
                content: self.content.clone(),
                required: self.required,
                retire: false,
            });
        }
        Ok(())
    }
}
#[tokio::test]
async fn coordinator_runs_last_and_new_loop_preserves_prefix_without_duplicate_references() {
    let session = SessionRuntime::memory();
    session
        .append(AgentMessage::System("stable core".into()))
        .await
        .unwrap();
    let provider = Arc::new(Recording::default());
    let mut state = state();
    for input in ["first question", "second question"] {
        let mut a = agent(session.clone(), provider.clone());
        a.register_plugin(Arc::new(Reference {
            content: "evidence".into(),
            required: false,
        }));
        assert!(a
            .run(user(input), &mut state, c())
            .await
            .unwrap()
            .error
            .is_none());
    }
    let requests = provider.requests.lock().unwrap();
    assert_eq!(
        requests[0].messages[0],
        AgentMessage::System("stable core".into())
    );
    assert_eq!(
        &requests[1].messages[..requests[0].messages.len()],
        &requests[0].messages
    );
    assert_eq!(
        serde_json::to_string(&requests[1].messages)
            .unwrap()
            .matches("test-source")
            .count(),
        1
    );
    assert!(!requests[1]
        .messages
        .iter()
        .any(|m| matches!(m, AgentMessage::Custom { .. })));
}
#[tokio::test]
async fn duplicate_coordinators_legacy_mutators_and_wrong_session_are_rejected_before_input() {
    let provider = Arc::new(Recording::default());
    for mode in 0..3 {
        let session = SessionRuntime::memory();
        let mut a = agent(session.clone(), provider.clone());
        if mode == 0 {
            a.register_plugin(Arc::new(ContextWindowPlugin::new(
                session.clone(),
                provider.clone(),
                config(),
            )));
        } else if mode == 1 {
            a.register_plugin(Arc::new(CompactionPlugin {
                max_messages: 2,
                keep_recent: 1,
            }));
        } else {
            a = AgentLoop::new(provider.clone()).with_session(session.clone());
            a.register_plugin(Arc::new(ContextWindowPlugin::new(
                SessionRuntime::memory(),
                provider.clone(),
                config(),
            )));
        }
        assert!(a
            .run(user("do not submit"), &mut state(), c())
            .await
            .unwrap()
            .error
            .is_some());
        assert!(session.snapshot().await.messages.is_empty());
    }
    assert!(provider.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn optional_oversize_is_omitted_required_oversize_stops_and_user_is_durable() {
    for required in [false, true] {
        let session = SessionRuntime::memory();
        let provider = Arc::new(Recording::default());
        let mut a = agent(session.clone(), provider.clone());
        a.register_plugin(Arc::new(Reference {
            content: "x".repeat(40_000),
            required,
        }));
        let result = a
            .run(user("current request"), &mut state(), c())
            .await
            .unwrap();
        assert_eq!(result.error.is_some(), required);
        assert_eq!(
            provider.requests.lock().unwrap().len(),
            usize::from(!required)
        );
        assert!(session
            .snapshot()
            .await
            .messages
            .contains(&AgentMessage::User(user("current request"))));
    }
}
#[tokio::test]
async fn repeated_compaction_keeps_original_constraints_and_latest_correction() {
    let session = SessionRuntime::memory();
    let provider = Arc::new(Recording::default());
    provider
        .answers
        .lock()
        .unwrap()
        .extend((0..12).map(|i| answer(&format!("explanation {i}: {}", "a".repeat(4000)))));
    let mut state = state();
    let mut compactions = 0;
    for i in 0..12 {
        let input = if i == 0 {
            "每天只有30分钟，必须中文回答".to_string()
        } else if i == 4 {
            "纠正：每天45分钟".to_string()
        } else {
            format!("question {i}")
        };
        let result = agent(session.clone(), provider.clone())
            .run(user(&input), &mut state, c())
            .await
            .unwrap();
        assert!(result.error.is_none(), "{:?}", result.error);
        compactions += result
            .events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ContextCompactionEnd { .. }))
            .count();
    }
    assert!(compactions >= 3, "{compactions}");
    let last =
        serde_json::to_string(&provider.requests.lock().unwrap().last().unwrap().messages).unwrap();
    assert!(last.contains("每天只有30分钟"));
    assert!(last.contains("每天45分钟"));
    assert!(last.contains("Historical summary"));
    assert_eq!(
        session
            .snapshot()
            .await
            .messages
            .iter()
            .filter(|m| matches!(m, AgentMessage::Assistant(_)))
            .count(),
        12
    );
}
#[tokio::test]
async fn summary_failure_retains_prior_checkpoint_and_stops_at_hard_limit() {
    let session = SessionRuntime::memory();
    let provider = Arc::new(Recording::default());
    provider
        .answers
        .lock()
        .unwrap()
        .extend((0..4).map(|_| answer(&"a".repeat(6000))));
    let mut state = state();
    for i in 0..3 {
        let mut a = AgentLoop::new(provider.clone()).with_session(session.clone());
        let mut settings = config();
        settings.soft_percent = 50;
        settings.target_percent = 25;
        a.register_plugin(Arc::new(
            ContextWindowPlugin::new(session.clone(), provider.clone(), settings)
                .with_compactor(Arc::new(BrokenSummary)),
        ));
        let result = a
            .run(user(&format!("question {i}")), &mut state, c())
            .await
            .unwrap();
        assert!(result.error.is_none(), "{:?}", result.error);
    }
    let old = session.snapshot().await.window;
    let mut a = AgentLoop::new(provider.clone()).with_session(session.clone());
    a.register_plugin(Arc::new(
        ContextWindowPlugin::new(session.clone(), provider.clone(), config())
            .with_compactor(Arc::new(BrokenSummary)),
    ));
    let result = a
        .run(user(&"huge current input".repeat(300)), &mut state, c())
        .await
        .unwrap();
    assert!(result.error.is_some());
    assert_eq!(session.snapshot().await.window.manifest, old.manifest);
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
}
#[tokio::test]
async fn file_replay_restores_window_and_locked_writer_prevents_concurrent_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let provider = Arc::new(Recording::default());
    let first;
    {
        let session = SessionRuntime::open_file(&path).unwrap();
        assert!(SessionRuntime::open_file(&path).is_err());
        let mut state = state();
        assert!(agent(session.clone(), provider.clone())
            .run(user("original"), &mut state, c())
            .await
            .unwrap()
            .error
            .is_none());
        first = session.snapshot().await.window.projection;
    }
    let session = SessionRuntime::open_file(&path).unwrap();
    assert_eq!(session.snapshot().await.window.projection, first);
    assert!(agent(session, provider.clone())
        .run(user("continue"), &mut state(), c())
        .await
        .unwrap()
        .error
        .is_none());
    assert_eq!(
        &provider.requests.lock().unwrap()[1].messages[..first.len()],
        &first
    );
}
#[tokio::test]
async fn partial_file_tail_is_backed_up_and_malformed_complete_record_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    std::fs::write(&path, b"{incomplete").unwrap();
    {
        let session = SessionRuntime::open_file(&path).unwrap();
        assert!(session.snapshot().await.messages.is_empty());
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    std::fs::write(&path, b"{malformed}\n").unwrap();
    assert!(SessionRuntime::open_file(&path).is_err());
}
struct BigTool;
#[async_trait]
impl AgentTool for BigTool {
    fn name(&self) -> &str {
        "big"
    }
    async fn execute(
        &self,
        _: serde_json::Value,
        _: ToolContext,
    ) -> Result<ToolResult, AgentError> {
        Ok(ToolResult {
            content: user(&"中".repeat(10_000)),
            is_error: false,
            terminate: false,
        })
    }
}
#[tokio::test]
async fn large_tool_output_is_archived_with_bounded_visible_result_and_legal_pairing() {
    let session = SessionRuntime::memory();
    let provider = Arc::new(Recording::default());
    provider
        .answers
        .lock()
        .unwrap()
        .push_back(AssistantMessage {
            content: vec![],
            tool_calls: vec![ToolCall {
                id: "call".into(),
                name: "big".into(),
                arguments: json!({}),
            }],
        });
    let mut a = agent(session.clone(), provider.clone());
    a.register_tool(Arc::new(BigTool));
    assert!(a
        .run(user("run"), &mut state(), c())
        .await
        .unwrap()
        .error
        .is_none());
    let snapshot = session.snapshot().await;
    let artifact = snapshot
        .messages
        .iter()
        .find_map(|m| match m {
            AgentMessage::Custom { kind, content } if kind == "lumen.tool_artifact" => {
                content["id"].as_str().map(str::to_owned)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        session.read_artifact(&artifact).await.unwrap().content,
        user(&"中".repeat(10_000))
    );
    let requests = provider.requests.lock().unwrap();
    validate_tool_groups(&requests[1].messages).unwrap();
    let visible = requests[1]
        .messages
        .iter()
        .find_map(|m| {
            if let AgentMessage::ToolResult(r) = m {
                Some(r)
            } else {
                None
            }
        })
        .unwrap();
    let ContentPart::Text(text) = &visible.content[0];
    assert!(text.len() <= 1024);
    assert!(text.contains(&artifact));
}
#[tokio::test]
async fn interrupted_call_is_closed_with_unknown_result_and_never_executed_again() {
    let session = SessionRuntime::memory();
    session
        .append(AgentMessage::User(user("previous")))
        .await
        .unwrap();
    session
        .append(AgentMessage::Assistant(AssistantMessage {
            content: vec![],
            tool_calls: vec![ToolCall {
                id: "pending".into(),
                name: "big".into(),
                arguments: json!({}),
            }],
        }))
        .await
        .unwrap();
    let provider = Arc::new(Recording::default());
    let result = agent(session, provider.clone())
        .run(user("continue"), &mut state(), c())
        .await
        .unwrap();
    assert!(result.error.is_none());
    assert!(!result
        .events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolExecutionStart(_))));
    let requests = provider.requests.lock().unwrap();
    validate_tool_groups(&requests[0].messages).unwrap();
    assert!(requests[0]
        .messages
        .iter()
        .any(|m| matches!(m,AgentMessage::ToolResult(r)if r.is_error)));
}
#[tokio::test]
async fn skills_load_deduplicate_retire_reactivate_and_resources_cannot_escape() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("skills");
    std::fs::create_dir_all(root.join("learn")).unwrap();
    std::fs::write(
        root.join("learn/SKILL.md"),
        "---\nname: learn\ndescription: Explain a concept\n---\nAsk the learner to explain first.",
    )
    .unwrap();
    std::fs::write(dir.path().join("outside"), "secret").unwrap();
    let registry = SkillRegistry::discover(&[("test".into(), root)]).unwrap();
    let session = SessionRuntime::memory();
    let provider = Arc::new(Recording::default());
    let mut state = state();
    for (i, name) in ["skill_load", "skill_load", "skill_finish", "skill_load"]
        .iter()
        .enumerate()
    {
        provider
            .answers
            .lock()
            .unwrap()
            .push_back(AssistantMessage {
                content: vec![],
                tool_calls: {
                    let mut calls = vec![ToolCall {
                    id: format!("s{i}"),
                    name: (*name).into(),
                    arguments: json!({"id":"test:learn"}),
                    }];
                    if i == 0 { calls.push(ToolCall { id:"progress".into(), name:"skill_progress".into(), arguments:json!({"id":"test:learn","step":"exercise 2","completed":["exercise 1"]}) }); }
                    calls
                },
            });
        provider.answers.lock().unwrap().push_back(answer("done"));
        let mut a = agent(session.clone(), provider.clone());
        a.register_plugin(registry.clone());
        registry.register_tools(&mut a, session.clone());
        let result = a
            .run(user(&format!("step {i}")), &mut state, c())
            .await
            .unwrap();
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(
            session
                .snapshot()
                .await
                .window
                .pins
                .contains_key("skill:test:learn"),
            i != 2
        );
        assert_eq!(
            session
                .snapshot()
                .await
                .window
                .pins
                .contains_key("skill-progress:test:learn"),
            i < 2
        );
    }
    {
        let requests = provider.requests.lock().unwrap();
        assert_eq!(
            serde_json::to_string(&requests[3].messages)
                .unwrap()
                .matches("Ask the learner to explain first.")
                .count(),
            1
        );
    }
    provider
        .answers
        .lock()
        .unwrap()
        .push_back(AssistantMessage {
            content: vec![],
            tool_calls: vec![ToolCall {
                id: "escape".into(),
                name: "skill_resource".into(),
                arguments: json!({"id":"test:learn","path":"../../outside"}),
            }],
        });
    let mut a = agent(session.clone(), provider.clone());
    registry.register_tools(&mut a, session);
    let result = a.run(user("read resource"), &mut state, c()).await.unwrap();
    assert!(result.error.is_none());
    assert!(result
        .messages
        .iter()
        .any(|m| matches!(m,AgentMessage::ToolResult(r)if r.tool_call_id=="escape"&&r.is_error)));
}

struct JournalWriter {
    records: Mutex<Vec<AgentMessage>>,
    fail_commit: bool,
    cancel_after_commit: Option<CancellationToken>,
}
#[async_trait]
impl TranscriptWriter for JournalWriter {
    async fn append(&self, message: &AgentMessage) -> Result<(), AgentError> {
        let is_commit =
            matches!(message,AgentMessage::Custom{kind,..}if kind=="lumen.context_commit");
        if self.fail_commit && is_commit {
            return Err(AgentError::Session("injected write failure".into()));
        }
        self.records.lock().unwrap().push(message.clone());
        if is_commit {
            if let Some(cancel) = &self.cancel_after_commit {
                cancel.cancel();
            }
        }
        Ok(())
    }
}
#[tokio::test]
async fn failed_checkpoint_never_sends_and_completed_commit_survives_cancellation() {
    for fail in [true, false] {
        let token = c();
        let writer = Arc::new(JournalWriter {
            records: Mutex::new(vec![]),
            fail_commit: fail,
            cancel_after_commit: (!fail).then(|| token.clone()),
        });
        let session = SessionRuntime::from_records(vec![], Some(writer.clone())).unwrap();
        let provider = Arc::new(Recording::default());
        let result = agent(session.clone(), provider.clone())
            .run(user("must survive"), &mut state(), token)
            .await
            .unwrap();
        assert!(result.error.is_some());
        assert!(provider.requests.lock().unwrap().is_empty());
        let replay =
            SessionRuntime::from_records(writer.records.lock().unwrap().clone(), None).unwrap();
        assert_eq!(
            replay.snapshot().await.messages,
            session.snapshot().await.messages
        );
        assert_eq!(
            replay.snapshot().await.window.manifest,
            session.snapshot().await.window.manifest
        );
        assert_eq!(replay.snapshot().await.window.projection.is_empty(), fail);
        assert_eq!(
            result
                .events
                .iter()
                .any(|e| matches!(e, AgentEvent::ContextPrepared { .. })),
            !fail
        );
    }
}
struct CancelAfterTool;
#[async_trait]
impl AgentPlugin for CancelAfterTool {
    async fn after_tool_call(
        &self,
        _: &mut AfterToolCall<'_>,
        _: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        Err(AgentError::Aborted)
    }
}
#[tokio::test]
async fn completed_tool_result_is_durable_even_if_after_hook_is_cancelled() {
    let session = SessionRuntime::memory();
    let provider = Arc::new(Recording::default());
    provider
        .answers
        .lock()
        .unwrap()
        .push_back(AssistantMessage {
            content: vec![],
            tool_calls: vec![ToolCall {
                id: "complete-before-cancel".into(),
                name: "big".into(),
                arguments: json!({}),
            }],
        });
    let mut a = agent(session.clone(), provider);
    a.register_tool(Arc::new(BigTool));
    a.register_plugin(Arc::new(CancelAfterTool));
    let result = a.run(user("execute"), &mut state(), c()).await.unwrap();
    assert_eq!(result.reason, StopReason::Aborted);
    let raw = session.snapshot().await.messages;
    assert!(raw.iter().any(|m|matches!(m,AgentMessage::ToolResult(r)if r.tool_call_id=="complete-before-cancel"&&!r.is_error)));
    let visible: Vec<_> = raw
        .into_iter()
        .filter(|m| !matches!(m, AgentMessage::Custom { .. }))
        .collect();
    validate_tool_groups(&visible).unwrap();
}
#[tokio::test]
async fn compaction_checkpoint_replays_pins_and_user_corrections_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.jsonl");
    let provider = Arc::new(Recording::default());
    provider
        .answers
        .lock()
        .unwrap()
        .extend((0..7).map(|_| answer(&"x".repeat(4000))));
    let expected;
    {
        let session = SessionRuntime::open_file(&path).unwrap();
        let mut state = state();
        for i in 0..7 {
            let mut a = agent(session.clone(), provider.clone());
            a.register_plugin(Arc::new(Reference {
                content: "mandatory constraint".into(),
                required: true,
            }));
            let result = a
                .run(user(&format!("q{i}")), &mut state, c())
                .await
                .unwrap();
            assert!(result.error.is_none());
        }
        expected = session.snapshot().await.window;
    }
    let session = SessionRuntime::open_file(&path).unwrap();
    let restored = session.snapshot().await.window;
    assert!(restored.epoch > 1);
    assert_eq!(restored.projection, expected.projection);
    assert_eq!(restored.pins, expected.pins);
    assert_eq!(restored.report, expected.report);
    assert_eq!(restored.manifest, expected.manifest);
}

struct SummaryProvider {
    requests: Mutex<Vec<ModelRequest>>,
}
#[async_trait]
impl ModelProvider for SummaryProvider {
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            context_tokens: 6000,
            max_output_tokens: 512,
        }
    }
    async fn stream(
        &self,
        request: ModelRequest,
        _: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        self.requests.lock().unwrap().push(request);
        Ok(Box::pin(stream::iter(vec![ModelEvent::Usage(TokenUsage{input_tokens:Some(100),output_tokens:Some(20),..Default::default()}),ModelEvent::Complete(answer(&json!({"scope":"prior","completed":[],"decisions":[],"failed_attempts":[],"open_questions":[],"next_steps":[],"evidence_refs":[],"coverage":[]}).to_string()))])))
    }
}
#[tokio::test]
async fn oversized_summary_is_split_into_bounded_legal_requests_with_usage_and_tools() {
    let provider = Arc::new(SummaryProvider {
        requests: Mutex::new(vec![]),
    });
    let mut messages = vec![AgentMessage::System("stable core".into())];
    for i in 0..10 {
        messages.push(AgentMessage::User(user(&format!("question{i}"))));
        messages.push(AgentMessage::Assistant(AssistantMessage {
            content: user(&"x".repeat(600)),
            tool_calls: vec![ToolCall {
                id: format!("call{i}"),
                name: "lookup".into(),
                arguments: json!({}),
            }],
        }));
        messages.push(AgentMessage::ToolResult(ToolResultMessage {
            tool_call_id: format!("call{i}"),
            content: user(&"e".repeat(400)),
            is_error: false,
            terminate: false,
        }));
    }
    let tools = vec![ToolDeclaration {
        name: "lookup".into(),
        description: "lookup".into(),
        input_schema: json!({"type":"object"}),
    }];
    let output = ModelCompactionService::new(provider.clone())
        .summarize(
            ModelRequest {
                messages,
                tools: tools.clone(),
                max_output_tokens: 512,
            },
            512,
            c(),
        )
        .await
        .unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&output.text).unwrap()["scope"].is_string());
    let requests = provider.requests.lock().unwrap();
    assert!(requests.len() > 2);
    assert_eq!(output.usage.len(), requests.len());
    for request in requests.iter() {
        validate_tool_groups(&request.messages).unwrap();
        assert_eq!(request.tools[0].name, "lookup");
        let prepared = provider.prepare(request.clone()).unwrap();
        assert!(prepared.estimated_input_tokens() + request.max_output_tokens + 1024 <= 6000);
    }
}
#[tokio::test]
async fn malformed_skill_is_diagnostic_and_crlf_skill_is_discoverable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("bad")).unwrap();
    std::fs::create_dir(dir.path().join("good")).unwrap();
    std::fs::write(dir.path().join("bad/SKILL.md"), "no metadata").unwrap();
    std::fs::write(
        dir.path().join("good/SKILL.md"),
        "---\r\nname: good\r\ndescription: valid skill\r\n---\r\nInstructions",
    )
    .unwrap();
    let registry = SkillRegistry::discover(&[("test".into(), dir.path().into())]).unwrap();
    assert_eq!(registry.list().len(), 1);
    assert_eq!(registry.diagnostics().len(), 1);
}

#[tokio::test]
async fn standalone_new_session_survives_restart_without_erasing_old_archive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("standalone.jsonl");
    {
        let runtime = SessionRuntime::open_file(&path).unwrap();
        runtime
            .append(AgentMessage::User(user("old session")))
            .await
            .unwrap();
        runtime.start_new("new-session").await.unwrap();
        runtime
            .append(AgentMessage::User(user("new session")))
            .await
            .unwrap();
    }
    let runtime = SessionRuntime::open_file(&path).unwrap();
    assert_eq!(
        runtime.snapshot().await.messages,
        vec![AgentMessage::User(user("new session"))]
    );
    assert!(std::fs::read_to_string(path)
        .unwrap()
        .contains("old session"));
}

#[tokio::test]
async fn reference_evicted_by_compaction_can_be_supplied_again_for_a_new_question() {
    let session = SessionRuntime::memory();
    let provider = Arc::new(Recording::default());
    provider
        .answers
        .lock()
        .unwrap()
        .extend((0..8).map(|_| answer(&"x".repeat(4000))));
    let mut state = state();
    let mut compactions = 0;
    for i in 0..8 {
        let mut a = agent(session.clone(), provider.clone());
        a.register_plugin(Arc::new(Reference {
            content: "current source exact detail".into(),
            required: false,
        }));
        let result = a
            .run(user(&format!("question{i}")), &mut state, c())
            .await
            .unwrap();
        assert!(result.error.is_none());
        compactions += result
            .events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ContextCompactionEnd { .. }))
            .count();
        let text =
            serde_json::to_string(&provider.requests.lock().unwrap().last().unwrap().messages)
                .unwrap();
        assert!(text.contains("current source exact detail"));
    }
    assert!(compactions > 0);
}

struct CancelPendingWriter {
    records: Mutex<Vec<AgentMessage>>,
    cancel: CancellationToken,
}
#[async_trait]
impl TranscriptWriter for CancelPendingWriter {
    async fn append(&self, message: &AgentMessage) -> Result<(), AgentError> {
        self.records.lock().unwrap().push(message.clone());
        if matches!(message,AgentMessage::Custom{kind,..}if kind=="lumen.context_pending") {
            self.cancel.cancel();
            tokio::task::yield_now().await;
        }
        Ok(())
    }
}
#[tokio::test]
async fn cancellation_during_skill_state_commit_cannot_diverge_memory_from_journal() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("learn")).unwrap();
    std::fs::write(
        dir.path().join("learn/SKILL.md"),
        "---\nname: learn\ndescription: learning\n---\nInstructions",
    )
    .unwrap();
    let registry = SkillRegistry::discover(&[("test".into(), dir.path().into())]).unwrap();
    let token = c();
    let writer = Arc::new(CancelPendingWriter {
        records: Mutex::new(vec![]),
        cancel: token.clone(),
    });
    let runtime = SessionRuntime::from_records(vec![], Some(writer.clone())).unwrap();
    let provider = Arc::new(Recording::default());
    provider
        .answers
        .lock()
        .unwrap()
        .push_back(AssistantMessage {
            content: vec![],
            tool_calls: vec![ToolCall {
                id: "load".into(),
                name: "skill_load".into(),
                arguments: json!({"id":"test:learn"}),
            }],
        });
    let mut a = agent(runtime.clone(), provider);
    registry.register_tools(&mut a, runtime.clone());
    let result = a
        .run(user("load skill"), &mut state(), token)
        .await
        .unwrap();
    assert_eq!(result.reason, StopReason::Aborted);
    let replay =
        SessionRuntime::from_records(writer.records.lock().unwrap().clone(), None).unwrap();
    let expected = runtime.snapshot().await;
    let actual = replay.snapshot().await;
    assert_eq!(actual.messages, expected.messages);
    assert_eq!(actual.pending.len(), 1);
    assert_eq!(actual.pending.len(), expected.pending.len());
    assert!(actual.messages.iter().any(|message|matches!(message,AgentMessage::ToolResult(result)if result.tool_call_id=="load"&&!result.is_error)));
}
