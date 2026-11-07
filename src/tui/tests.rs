use super::*;
use crate::agent::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, layout::Position, style::Color, Terminal};
use std::sync::{Arc, Mutex};

fn screen(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        let mut x = 0;
        while x < buffer.area.width {
            let symbol = buffer[(x, y)].symbol();
            out.push_str(symbol);
            x += text::width(symbol).max(1) as u16;
        }
        out.push('\n');
    }
    out
}

#[test]
fn three_regions_and_native_caret_survive_multiline_cjk_and_resizing() {
    for (width, height) in [
        (128, 40),
        (100, 30),
        (88, 24),
        (80, 24),
        (60, 16),
        (40, 12),
        (24, 9),
    ] {
        for theme in [Theme::Auto, Theme::Light, Theme::Dark] {
            let mut ui = Ui::new("mock".into(), theme);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for input in [
                String::new(),
                "中文输入".into(),
                "/the".into(),
                "中文\n第二行 abc".into(),
                "长输入".repeat(200),
            ] {
                ui.editor.set(input);
                terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
                let g = render::geometry(terminal.backend().buffer().area, &ui);
                let caret = terminal.get_cursor_position().unwrap();
                assert!(g.input.contains(caret), "{width}x{height}: {caret:?}");
                assert!(caret.y < g.input.bottom() - 1);
                assert!(screen(&terminal).contains("Agent"));
                if width >= 88 {
                    let tree = g.tree.unwrap();
                    assert_eq!(tree.right() - 1, g.content.x);
                    assert_eq!(
                        terminal.backend().buffer()[(g.content.x, g.content.y)].symbol(),
                        "┬"
                    );
                    assert_eq!(
                        terminal.backend().buffer()[(g.content.x, g.input.bottom() - 1)].symbol(),
                        "┴"
                    );
                }
                assert_eq!(g.content.bottom() - 1, g.input.y);
                assert_eq!(
                    terminal.backend().buffer()[(g.content.x, g.input.y)].symbol(),
                    "├"
                );
                assert_eq!(
                    terminal.backend().buffer()[(g.content.right() - 1, g.input.y)].symbol(),
                    "┤"
                );
                if theme == Theme::Auto {
                    for cell in &terminal.backend().buffer().content {
                        assert!(matches!(cell.bg, Color::Reset | Color::Blue),
                            "auto must inherit the terminal background outside selection highlights");
                        assert!(
                            !matches!(cell.fg, Color::Rgb(..)),
                            "auto must use the terminal palette"
                        );
                    }
                }
            }
        }
    }
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    let start = terminal.get_cursor_position().unwrap();
    ui.editor.insert("中a");
    terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    assert_eq!(
        terminal.get_cursor_position().unwrap(),
        Position::new(start.x + 3, start.y)
    );
    ui.editor.left();
    terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    assert_eq!(
        terminal.get_cursor_position().unwrap(),
        Position::new(start.x + 2, start.y)
    );
}

#[test]
fn tiny_terminal_and_tree_drawer_render_without_panics() {
    for (width, height) in [(80, 20), (40, 12), (24, 9), (16, 6), (1, 1)] {
        let mut ui = Ui::new("mock".into(), Theme::Auto);
        ui.focus = Focus::Tree;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
        if width >= 24 && height >= 9 {
            assert!(screen(&terminal).contains("知识目录"));
        }
    }
}

#[test]
fn input_shortcuts_do_not_trigger_reader_actions_and_completion_is_available() {
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.view = View::Read;
    ui.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert_eq!(ui.editor.text, "a");
    ui.editor.set("/the".into());
    assert_eq!(ui.candidates().len(), 3);
    ui.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(ui.editor.text, "/theme auto ");
    ui.key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert_eq!(ui.focus, Focus::Tree);
    assert_eq!(ui.editor.text, "/theme auto ");
}

struct TestProvider {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    wait: bool,
}
#[async_trait::async_trait]
impl ModelProvider for TestProvider {
    async fn stream(
        &self,
        request: ModelRequest,
        _: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        self.requests.lock().unwrap().push(request);
        let wait = self.wait;
        Ok(Box::pin(async_stream::stream! {
            yield ModelEvent::Start; yield ModelEvent::TextDelta("并发事务的解释。".into());
            if wait { std::future::pending::<()>().await; }
            yield ModelEvent::Complete(AssistantMessage { content:vec![ContentPart::Text("并发事务的解释。".into())],tool_calls:vec![] });
        }))
    }
}
async fn fixture() -> (tempfile::TempDir, WorkspaceHandle, ObjectId) {
    let dir = tempfile::tempdir().unwrap();
    let w = WorkspaceService::init(
        InitRequest {
            root: dir.path().join("学习工作区"),
            author_name: "Test".into(),
            author_email: "test@example.com".into(),
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let domain = KnowledgeObject {
        metadata: ObjectMetadata::new(ObjectKind::Domain, "计算机科学"),
        body: String::new(),
    };
    let mut topic = KnowledgeObject {
        metadata: ObjectMetadata::new(ObjectKind::Topic, "数据库"),
        body: String::new(),
    };
    topic.metadata.member_of.push(domain.metadata.id.clone());
    let mut concept = KnowledgeObject {
        metadata: ObjectMetadata::new(ObjectKind::Concept, "事务隔离性"),
        body: "# 事务隔离性\n\n## 核心解释\n\n旧的解释。\n\n```sql\nSELECT balance;\n```".into(),
    };
    concept.metadata.member_of.push(topic.metadata.id.clone());
    let id = concept.metadata.id.clone();
    w.knowledge()
        .apply(
            ChangeSet {
                changes: vec![domain, topic, concept]
                    .into_iter()
                    .map(|object| ObjectChange::Create { object })
                    .collect(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    (dir, w, id)
}
fn runtime(w: WorkspaceHandle, wait: bool) -> (Runtime, Arc<Mutex<Vec<ModelRequest>>>) {
    let requests = Arc::new(Mutex::new(vec![]));
    let mut app = Application::new();
    app.workspace = Some(w);
    app.provider = Some(Arc::new(TestProvider {
        requests: requests.clone(),
        wait,
    }));
    let mut rt = Runtime::new(app, Theme::Auto);
    rt.update_workspace();
    (rt, requests)
}
async fn navigate(rt: &mut Runtime) {
    let event = tokio::time::timeout(Duration::from_secs(5), rt.nav_rx.recv())
        .await
        .unwrap()
        .unwrap();
    rt.navigation(event);
}

#[tokio::test]
async fn reading_is_not_replaced_by_streaming_answers_and_commands_restore_positions() {
    let (dir, w, id) = fixture().await;
    let (mut rt, requests) = runtime(w.clone(), false);
    navigate(&mut rt).await;
    assert_eq!(rt.ui.tree.objects.len(), 3);
    rt.submit("/open 事务隔离性".into());
    navigate(&mut rt).await;
    let mut terminal = Terminal::new(TestBackend::new(128, 40)).unwrap();
    terminal.draw(|f| render::draw(f, &mut rt.ui)).unwrap();
    assert!(screen(&terminal).contains("SELECT balance;"));
    rt.ui.reference = Some(Reference {
        id: id.clone(),
        title: "事务隔离性".into(),
        heading: Some("核心解释".into()),
        selection: None,
    });
    let old = w
        .knowledge()
        .read(id.clone(), ReadView::Full, CancellationToken::new())
        .await
        .unwrap();
    w.knowledge()
        .patch(
            PatchObject {
                id,
                expected_revision: old.revision,
                metadata_patch: Default::default(),
                body: Some("# 事务隔离性\n\n## 核心解释\n\n更新后的解释。".into()),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    rt.ui.editor.set("解释引用".into());
    rt.submit(rt.ui.editor.text.clone());
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    assert_eq!(rt.ui.view, View::Read);
    assert_eq!(rt.ui.unread, 1);
    assert_eq!(
        rt.ui.messages.iter().filter(|m| m.role == "Lumen").count(),
        1
    );
    let request = requests.lock().unwrap().last().unwrap().clone();
    let input = request
        .messages
        .iter()
        .filter_map(|m| {
            if let AgentMessage::User(parts) = m {
                Some(parts)
            } else {
                None
            }
        })
        .flatten()
        .map(|ContentPart::Text(t)| t.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(input.contains("更新后的解释"));
    assert!(!input.contains("旧的解释"));
    rt.ui.editor.set("保留草稿".into());
    rt.ui.focus = Focus::Input;
    rt.ui.document.as_mut().unwrap().scroll = 3;
    rt.ui.key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
    assert_eq!(rt.ui.view, View::Chat);
    assert_eq!(rt.ui.unread, 0);
    assert_eq!(rt.ui.editor.text, "保留草稿");
    rt.ui.key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
    assert_eq!(rt.ui.document.as_ref().unwrap().scroll, 3);
    rt.submit("/theme light".into());
    assert_eq!(rt.ui.theme, Theme::Light);
    assert_eq!(rt.ui.view, View::Read);
    rt.shutdown().await.unwrap();
    let sessions = dir.path().join("学习工作区/sessions");
    let segments = std::fs::read_dir(sessions)
        .unwrap()
        .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
        .map(|p| p.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect::<Vec<_>>();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].extension().unwrap(), "jsonl");
    assert!(std::fs::read_to_string(&segments[0])
        .unwrap()
        .contains("并发事务的解释"));
}

#[tokio::test]
async fn cancellation_keeps_partial_reply_and_restores_draft() {
    let (_dir, w, _) = fixture().await;
    let (mut rt, _) = runtime(w, true);
    rt.submit("待重试的问题".into());
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), rt.agent_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let partial = matches!(&event, AgentEvent::MessageDelta { .. })
            || matches!(&event,AgentEvent::SessionEvent{event,..}if matches!(event.as_ref(),AgentEvent::MessageDelta{..}));
        rt.ui.agent(event);
        if partial {
            break;
        }
    }
    rt.action(Action::Cancel);
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    assert_eq!(rt.ui.editor.text, "待重试的问题");
    assert!(rt.ui.messages.iter().any(|m| m.body == "并发事务的解释。"));
    assert!(!rt.ui.messages.iter().any(|m| m.role == "错误"));
    rt.shutdown().await.unwrap();
}

#[tokio::test]
async fn stale_selection_is_rejected_and_catalog_is_fully_paginated() {
    let (_dir, w, id) = fixture().await;
    let mut app = Application::new();
    app.workspace = Some(w.clone());
    let old = w
        .knowledge()
        .read(id.clone(), ReadView::Full, CancellationToken::new())
        .await
        .unwrap();
    let reference = Reference {
        id: id.clone(),
        title: "事务隔离性".into(),
        heading: None,
        selection: Some((old.revision.clone(), "旧的解释。".into())),
    };
    w.knowledge()
        .patch(
            PatchObject {
                id,
                expected_revision: old.revision,
                metadata_patch: Default::default(),
                body: Some("新正文".into()),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(referenced_input(
        &app,
        "问题".into(),
        Some(reference),
        CancellationToken::new()
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("文档已更新"));
    let changes = (0..105)
        .map(|i| ObjectChange::Create {
            object: KnowledgeObject {
                metadata: ObjectMetadata::new(ObjectKind::Concept, format!("概念 {i}")),
                body: String::new(),
            },
        })
        .collect();
    w.knowledge()
        .apply(ChangeSet { changes }, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        service::load_catalog(&w, CancellationToken::new())
            .await
            .unwrap()
            .len(),
        108
    );
    w.close(CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn context_commands_resume_durable_window_and_learning_evidence_without_duplicate_messages() {
    let (_dir, w, id) = fixture().await;
    let event = w
        .learning()
        .append(
            crate::knowledge::AppendEvent {
                event_kind: crate::knowledge::EventKind::Assessment,
                occurred_at: None,
                targets: vec![id.clone()],
                session_id: None,
                supersedes: None,
                data: serde_json::json!({"outcome":"partial"}),
                body: "assessment evidence".into(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let requests = Arc::new(Mutex::new(vec![]));
    let provider = Arc::new(TestProvider {
        requests: requests.clone(),
        wait: false,
    });
    let mut app = Application::new();
    app.workspace = Some(w.clone());
    app.provider = Some(provider.clone());
    let (tx, _rx) = mpsc::unbounded_channel();
    app.chat(
        format!("解释 {}", id.as_str()),
        CancellationToken::new(),
        tx.clone(),
    )
    .await
    .unwrap();
    let context: serde_json::Value = serde_json::from_str(
        &app.command(crate::commands::Command::Context, CancellationToken::new())
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(context["regions"]["C_task"].as_u64().unwrap() > 0);
    let session_id = context["session_id"].as_str().unwrap().to_string();
    app.chat("再举一个例子".into(), CancellationToken::new(), tx.clone())
        .await
        .unwrap();
    let transcript = w
        .sessions()
        .load(session_id.clone(), CancellationToken::new())
        .await
        .unwrap();
    let raw: Vec<_> = transcript
        .segments
        .iter()
        .flat_map(|s| &s.messages)
        .collect();
    assert_eq!(
        raw.iter()
            .filter(|m| matches!(m, AgentMessage::User(_)))
            .count(),
        2
    );
    assert_eq!(
        raw.iter()
            .filter(|m| matches!(m, AgentMessage::Assistant(_)))
            .count(),
        2
    );
    let old_request = requests.lock().unwrap().last().unwrap().messages.clone();
    assert!(serde_json::to_string(&old_request)
        .unwrap()
        .contains(&event.metadata.id));
    app.manager.as_mut().unwrap().close(None).unwrap();
    let mut resumed = Application::new();
    resumed.workspace = Some(w.clone());
    resumed.provider = Some(provider);
    resumed
        .command(
            crate::commands::Command::Resume(session_id),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    resumed
        .chat("继续".into(), CancellationToken::new(), tx)
        .await
        .unwrap();
    assert_eq!(
        &requests.lock().unwrap().last().unwrap().messages[..old_request.len()],
        &old_request
    );
    w.close(CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn switching_sessions_restores_history_draft_reading_and_rejects_stale_events() {
    let (_dir, w, id) = fixture().await;
    let (mut rt, _) = runtime(w.clone(), false);
    navigate(&mut rt).await;
    rt.submit("/new 第一课".into());
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    let first = rt.ui.session_identity.clone().unwrap();
    rt.submit("第一课问题".into());
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    rt.submit(format!("/open {}", id.as_str()));
    navigate(&mut rt).await;
    rt.ui.document.as_mut().unwrap().scroll = 2;
    rt.ui.editor.set("第一课草稿".into());
    rt.action(Action::Command(crate::commands::Command::NewTitled(
        "第二课".into(),
    )));
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    assert_ne!(rt.ui.session_identity.as_ref(), Some(&first));
    assert!(rt.ui.editor.text.is_empty());
    assert!(rt.ui.messages.is_empty());
    rt.submit("第二课问题".into());
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    let second = rt.ui.session_identity.clone().unwrap();
    rt.ui.editor.set("第二课草稿".into());
    rt.action(Action::Command(crate::commands::Command::Resume(
        first.session_id.clone(),
    )));
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    navigate(&mut rt).await;
    assert_eq!(rt.ui.editor.text, "第一课草稿");
    assert_eq!(rt.ui.document.as_ref().unwrap().scroll, 2);
    assert_eq!(rt.ui.view, View::Read);
    assert!(rt
        .ui
        .messages
        .iter()
        .any(|message| message.body.contains("第一课问题")));
    assert!(!rt
        .ui
        .messages
        .iter()
        .any(|message| message.body.contains("第二课问题")));
    let count = rt.ui.messages.len();
    rt.ui.agent(AgentEvent::SessionEvent {
        identity: second,
        run_id: "old".into(),
        activation: 0,
        event: Box::new(AgentEvent::MessageEnd(AgentMessage::Assistant(
            AssistantMessage {
                content: vec![ContentPart::Text("stale result".into())],
                tool_calls: vec![],
            },
        ))),
    });
    assert_eq!(rt.ui.messages.len(), count);
    rt.submit("/sessions".into());
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    assert!(rt.ui.session_panel);
    assert_eq!(rt.ui.session_choices.len(), 2);
    let mut terminal = Terminal::new(TestBackend::new(128, 40)).unwrap();
    terminal
        .draw(|frame| render::draw(frame, &mut rt.ui))
        .unwrap();
    assert!(screen(&terminal).contains("第一课"));
    rt.shutdown().await.unwrap();
}

#[tokio::test]
async fn paged_history_deduplicates_and_readonly_catalog_is_available_during_run() {
    let (dir, w, _) = fixture().await;
    let (mut rt, _) = runtime(w.clone(), true);
    navigate(&mut rt).await;
    rt.submit("/new paging".into());
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    // Import a legacy transcript with enough rows to require UI pagination.
    let legacy = dir.path().join("history.jsonl");
    let mut body = String::new();
    for index in 0..130 {
        body.push_str(
            &serde_json::to_string(&SessionRecord {
                message: if index % 2 == 0 {
                    AgentMessage::User(vec![ContentPart::Text(format!("question-{index}"))])
                } else {
                    AgentMessage::Assistant(AssistantMessage {
                        content: vec![ContentPart::Text(format!("answer-{index}"))],
                        tool_calls: vec![],
                    })
                },
            })
            .unwrap(),
        );
        body.push('\n');
    }
    std::fs::write(&legacy, body).unwrap();
    let id = rt
        .app
        .as_mut()
        .unwrap()
        .manager
        .as_mut()
        .unwrap()
        .import_legacy(&legacy)
        .await
        .unwrap()
        .remove(0);
    rt.action(Action::Command(crate::commands::Command::Resume(id)));
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    assert_eq!(rt.ui.messages.len(), 50);
    let cursor = rt.ui.history_before.clone().unwrap();
    rt.action(Action::Command(crate::commands::Command::Session(
        crate::commands::SessionCommand::History(Some(cursor)),
    )));
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    assert_eq!(rt.ui.messages.len(), 100);
    let count = rt.ui.messages.len();
    let page = rt
        .app
        .as_ref()
        .unwrap()
        .manager
        .as_ref()
        .unwrap()
        .history(
            &rt.ui.session_identity.as_ref().unwrap().session_id,
            None,
            50,
        )
        .await
        .unwrap();
    rt.ui.prepend_history(page);
    assert_eq!(rt.ui.messages.len(), count);
    rt.submit("new question".into());
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), rt.agent_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let partial = matches!(&event,AgentEvent::SessionEvent{event,..}if matches!(event.as_ref(),AgentEvent::MessageDelta{..}));
        rt.ui.agent(event);
        if partial {
            break;
        }
    }
    rt.action(Action::Command(crate::commands::Command::Sessions(
        String::new(),
    )));
    navigate(&mut rt).await;
    assert!(rt.operation.is_some());
    assert!(rt.ui.session_panel);
    rt.action(Action::Cancel);
    let result = rt.operation.take().unwrap().await;
    rt.completed(result).unwrap();
    rt.shutdown().await.unwrap();
}
