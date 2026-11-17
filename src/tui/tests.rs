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
            ui.workspace_path = Some("/tmp/test-knowledge".into());
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
                assert!(caret.y < g.input.bottom());
                assert!(screen(&terminal).contains("输入"));
                let tree = g.tree;
                assert!(
                    tree.width > 0,
                    "directory must remain visible at {width} columns"
                );
                assert_eq!(tree.right(), g.content.x);
                assert!(!tree.intersects(g.content));
                assert!(!tree.intersects(g.input));
                assert_eq!(g.content.bottom(), g.input.y);
                assert!(!g.content.intersects(g.input));
                assert_eq!(g.input.bottom(), g.footer.y - 1);
                if theme == Theme::Auto {
                    for cell in &terminal.backend().buffer().content {
                        assert_eq!(
                            cell.bg,
                            Color::Reset,
                            "auto must inherit the terminal background, including selections"
                        );
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
            assert!(screen(&terminal).contains("目录"));
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
    assert_eq!(ui.focus, Focus::Input);
    assert_eq!(ui.view, View::Chat);
    ui.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    assert_eq!(ui.focus, Focus::Tree);
    assert_eq!(ui.editor.text, "/theme auto ");
}

#[test]
fn slash_opens_commands_from_chat_and_f1_restores_input_focus() {
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.focus = Focus::Content;
    ui.completions_hidden = true;
    ui.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert_eq!(ui.focus, Focus::Input);
    assert_eq!(ui.editor.text, "/");
    assert!(!ui.candidates().is_empty());

    for focus in [Focus::Tree, Focus::Content] {
        ui.view = View::Read;
        ui.focus = focus;
        ui.completions_hidden = true;
        ui.key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert_eq!(ui.view, View::Chat);
        assert_eq!(ui.focus, Focus::Input);
        assert_eq!(ui.editor.text, "/");
        assert!(!ui.candidates().is_empty());
    }

    // Commands remain available while browsing the directory or a document.
    for (focus, view) in [(Focus::Tree, View::Chat), (Focus::Content, View::Read)] {
        ui.editor.take();
        ui.focus = focus;
        ui.view = view;
        ui.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(ui.focus, Focus::Input);
        assert_eq!(ui.editor.text, "/");
        assert!(!ui.candidates().is_empty());
    }
}

#[test]
fn complete_commands_execute_from_browsing_and_search_on_the_first_enter() {
    for focus in [Focus::Tree, Focus::Content, Focus::Search, Focus::Input] {
        for command in ["/quit", "/exit", "/chat", "/read", "/theme light"] {
            let mut rt = Runtime::new(Application::new(), Theme::Auto);
            rt.ui.focus = focus;
            rt.ui.view = View::Read;
            rt.ui.completions_hidden = true;
            rt.ui.search.set("旧的查找内容".into());
            for ch in command.chars() {
                rt.ui
                    .key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            }
            assert_eq!(rt.ui.focus, Focus::Input, "{focus:?}: {command}");
            assert_eq!(rt.ui.editor.text, command, "{focus:?}: {command}");
            let action = rt
                .ui
                .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .expect("one Enter must execute a fully typed command");
            rt.action(action);
            match command {
                "/quit" | "/exit" => assert!(rt.quitting),
                "/chat" => assert_eq!(rt.ui.view, View::Chat),
                "/read" => assert_eq!(rt.ui.view, View::Read),
                "/theme light" => assert_eq!(rt.ui.theme, Theme::Light),
                _ => unreachable!(),
            }
            assert!(rt.ui.editor.text.is_empty());
        }
    }
}

#[test]
fn pasting_a_command_from_browsing_or_search_waits_for_enter_then_executes() {
    for focus in [Focus::Tree, Focus::Content, Focus::Search, Focus::Input] {
        let mut rt = Runtime::new(Application::new(), Theme::Auto);
        rt.ui.focus = focus;
        rt.ui.view = View::Read;
        rt.ui.completions_hidden = true;
        rt.ui.paste("/quit");
        assert_eq!(rt.ui.focus, Focus::Input);
        assert_eq!(rt.ui.editor.text, "/quit");
        assert_eq!(rt.ui.candidates()[0].label, "/quit");
        assert!(!rt.quitting, "pasting must not execute commands");
        let action = rt
            .ui
            .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        rt.action(action);
        assert!(rt.quitting);
    }
}

#[test]
fn browse_find_shortcut_filters_without_becoming_a_command_or_losing_the_draft() {
    for (focus, view) in [(Focus::Tree, View::Chat), (Focus::Content, View::Read)] {
        let mut ui = Ui::new("mock".into(), Theme::Auto);
        ui.focus = focus;
        ui.view = view;
        ui.editor.set("尚未发送的草稿".into());
        ui.editor.left();
        let cursor = ui.editor.cursor;
        ui.key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert_eq!(ui.focus, Focus::Search);
        assert_eq!(ui.search_tree, focus == Focus::Tree);
        ui.key(KeyEvent::new(KeyCode::Char('知'), KeyModifiers::NONE));
        assert_eq!(ui.search.text, "知");
        if focus == Focus::Tree {
            assert_eq!(ui.tree.filter, "知");
        } else {
            assert_eq!(ui.reading_query, "知");
        }
        ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(ui.focus, focus);
        ui.key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(ui.focus, Focus::Input);
        assert_eq!(ui.editor.text, "尚未发送的草稿");
        assert_eq!(ui.editor.cursor, cursor);
    }
}

#[test]
fn slash_menu_keeps_a_visible_choice_above_the_editor_in_short_windows() {
    for (width, height) in [(24, 9), (40, 12), (60, 14), (100, 30)] {
        let mut ui = Ui::new("mock".into(), Theme::Auto);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
        let before = terminal.backend().buffer().clone();
        ui.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
        let g = render::geometry(before.area, &ui);
        let rendered = screen(&terminal);
        assert!(rendered.contains("命令"), "{width}x{height}: {rendered}");
        assert!(rendered.contains("› /"), "{width}x{height}: {rendered}");
        assert!(g.input.contains(terminal.get_cursor_position().unwrap()));
        for y in g.tree.y..g.tree.bottom() {
            for x in g.tree.x..g.tree.right() {
                assert_eq!(before[(x, y)], terminal.backend().buffer()[(x, y)]);
            }
        }
    }
}

#[test]
fn clicking_the_editor_restores_commands_without_losing_the_draft() {
    let mut rt = Runtime::new(Application::new(), Theme::Auto);
    rt.ui.view = View::Read;
    rt.ui.focus = Focus::Content;
    rt.ui.completions_hidden = true;
    rt.ui.editor.set("/the".into());
    let cursor = rt.ui.editor.cursor;
    let area = ratatui::layout::Rect::new(0, 0, 100, 30);
    let input = render::geometry(area, &rt.ui).input;
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: input.x + 3,
        row: input.y + 2,
        modifiers: KeyModifiers::NONE,
    };
    assert!(rt.mouse(click, area));
    assert_eq!(rt.ui.focus, Focus::Input);
    assert_eq!(rt.ui.view, View::Read);
    assert_eq!(rt.ui.editor.text, "/the");
    assert_eq!(rt.ui.editor.cursor, cursor);
    assert_eq!(rt.ui.candidates().len(), 3);
    rt.ui.session_panel = true;
    assert!(rt.ui.candidates().is_empty());
    rt.ui.focus = Focus::Content;
    assert!(!rt.mouse(click, area));
    assert_eq!(rt.ui.focus, Focus::Content);
}

#[test]
fn copy_uses_original_message_text_and_never_copies_the_draft() {
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.editor.set("尚未发送的草稿".into());
    assert!(ui.copy_text(false).is_err());
    assert!(ui.copy_text(true).is_err());
    ui.message("你", "解释数据库事务");
    let reply = "## 事务\n\n中文 e\u{301} 👩‍💻\n\n```sql\nBEGIN;\nCOMMIT;\n```";
    ui.message("Lumen", reply);
    ui.message("系统", "执行完毕");
    ui.message("Lumen", "");
    assert_eq!(ui.copy_text(false).unwrap(), reply);
    assert_eq!(
        ui.copy_text(true).unwrap(),
        format!("你:\n解释数据库事务\n\nLumen:\n{reply}\n\n系统:\n执行完毕")
    );
    assert_eq!(ui.editor.text, "尚未发送的草稿");

    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    assert_eq!(
        ui.copy_text(false).unwrap(),
        reply,
        "copy must not use wrapped terminal rows"
    );
    ui.messages
        .last_mut()
        .unwrap()
        .body
        .push_str("正在生成的回复");
    assert_eq!(ui.copy_text(false).unwrap(), "正在生成的回复");
    ui.close_session();
    assert!(
        ui.copy_text(true).is_err(),
        "closed sessions must not leak into copying"
    );
}

#[test]
fn copy_commands_execute_from_candidates_and_empty_replies_keep_the_input() {
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.editor.set("/cop".into());
    assert_eq!(ui.candidates()[0].label, "/copy");
    ui.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(matches!(
        ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(Action::Submit(text)) if text.trim() == "/copy all"
    ));
    let mut rt = Runtime::new(Application::new(), Theme::Auto);
    rt.ui.editor.set("/copy".into());
    rt.submit("/copy".into());
    assert!(rt.ui.error);
    assert_eq!(rt.ui.editor.text, "/copy");
    assert!(rt.ui.notice.as_ref().unwrap().contains("没有可复制"));
    assert!(rt.operation.is_none());
}

#[test]
fn command_candidates_enter_executes_and_tab_only_completes() {
    for confirm in [KeyCode::Enter, KeyCode::Tab] {
        let mut ui = Ui::new("mock".into(), Theme::Auto);
        ui.editor.set("/the".into());
        ui.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(ui.candidates()[ui.candidate].label, "/theme light");
        let action = ui.key(KeyEvent::new(confirm, KeyModifiers::NONE));
        if confirm == KeyCode::Enter {
            assert!(matches!(action, Some(Action::Submit(text)) if text.trim() == "/theme light"));
        } else {
            assert!(action.is_none());
            assert!(
                matches!(ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Some(Action::Submit(text)) if text.trim() == "/theme light")
            );
        }
        assert_eq!(ui.editor.text, "/theme light ");
        assert_eq!(ui.focus, Focus::Input);
        assert!(ui.candidates().is_empty());
    }
    for command in ["/session", "/sessions", "/model", "/models", "/sync"] {
        let mut ui = Ui::new("mock".into(), Theme::Auto);
        ui.editor.set(command.into());
        assert_eq!(ui.candidates()[0].label, command);
        assert!(
            matches!(ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Some(Action::Submit(text)) if text == command)
        );
    }
}

#[test]
fn commands_with_required_arguments_keep_input_until_the_arguments_are_supplied() {
    for (prefix, command, arguments) in [
        ("/workspace ini", "/workspace init", "\"/tmp/学习 空间\""),
        ("/sear", "/search", "事务隔离"),
        ("/provider k", "/provider key", "company"),
    ] {
        let mut ui = Ui::new("mock".into(), Theme::Auto);
        ui.editor.set(prefix.into());
        assert!(ui
            .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .is_none());
        assert_eq!(ui.editor.text, format!("{command} "));
        assert_eq!(ui.focus, Focus::Input);
        assert!(ui.notice.as_ref().is_some_and(|n| n.contains("参数")));
        assert!(ui.candidates().is_empty());
        ui.editor.insert(arguments);
        assert!(
            matches!(ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Some(Action::Submit(text)) if text == format!("{command} {arguments}"))
        );
    }
}

#[test]
fn a_single_enter_on_a_command_prefix_applies_the_selected_theme() {
    let mut rt = Runtime::new(Application::new(), Theme::Auto);
    rt.ui.editor.set("/the".into());
    rt.ui.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let action = rt
        .ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .expect("selected command executes on the first Enter");
    rt.action(action);
    assert_eq!(rt.ui.theme, Theme::Light);
    assert!(rt.ui.editor.text.is_empty());
    assert!(rt.operation.is_none());
}

#[test]
fn command_list_scrolls_to_the_selected_entry_and_escape_keeps_the_draft() {
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.editor.set("/".into());
    for _ in 0..12 {
        ui.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let selected = ui.candidates()[ui.candidate].label.clone();
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    assert!(screen(&terminal).contains(&format!("› {selected}")));
    assert!(
        matches!(ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Some(Action::Submit(text)) if text.trim() == selected)
    );
    assert_eq!(ui.editor.text, format!("{selected} "));

    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.editor.set("/the".into());
    ui.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(ui.candidates().is_empty());
    assert_eq!(ui.editor.text, "/the");
    assert_eq!(ui.focus, Focus::Input);
    ui.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(ui.focus, Focus::Content);
}

#[test]
fn completing_a_command_in_the_middle_preserves_arguments_and_unicode_boundaries() {
    for (input, cursor, choice, expected) in [
        (
            "/workspace init \"/tmp/学习 空间\"",
            3,
            "/workspace open",
            "/workspace open \"/tmp/学习 空间\"",
        ),
        ("/mod research", 4, "/model", "/model research"),
        ("/mo　研究模型", 3, "/model", "/model　研究模型"),
    ] {
        let mut ui = Ui::new("mock".into(), Theme::Auto);
        ui.editor.set(input.into());
        ui.editor.cursor = cursor;
        let index = ui
            .candidates()
            .iter()
            .position(|c| c.label == choice)
            .unwrap();
        for _ in 0..index {
            ui.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        assert!(
            matches!(ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Some(Action::Submit(text)) if text == expected)
        );
        assert_eq!(ui.editor.text, expected);
        assert!(ui.editor.text.is_char_boundary(ui.editor.cursor));
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    }
}

#[test]
fn enter_confirms_a_selected_knowledge_reference_without_submitting_the_question() {
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    let first = ObjectMetadata::new(ObjectKind::Concept, "事务原子性");
    let second = ObjectMetadata::new(ObjectKind::Concept, "事务隔离性");
    let selected = second.id.clone();
    ui.tree.objects = vec![first, second];
    ui.editor.set("请解释 @事务".into());
    ui.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(
        matches!(ui.key(KeyEvent::new(KeyCode::Enter,KeyModifiers::NONE)),Some(Action::Attach(id)) if id==selected)
    );
    assert_eq!(ui.editor.text, "请解释 ");
    assert_eq!(ui.focus, Focus::Input);
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
    ) -> Result<ModelEventStream, ProviderError> {
        self.requests.lock().unwrap().push(request);
        let wait = self.wait;
        Ok(Box::pin(async_stream::stream! {
            yield Ok(ModelEvent::Started); yield Ok(ModelEvent::TextDelta("并发事务的解释。".into()));
            if wait { std::future::pending::<()>().await; }
            yield Ok(ModelEvent::completed(AssistantMessage { content:vec![ContentPart::Text("并发事务的解释。".into())],tool_calls:vec![] , ..Default::default()}));
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
    rt.ui.key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
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
                ..Default::default()
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
                        ..Default::default()
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
    rt.ui.editor.set("保留这份草稿".into());
    rt.ui.editor.left();
    let draft_cursor = rt.ui.editor.cursor;
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| render::draw(f, &mut rt.ui)).unwrap();
    // PageUp works from the editor and loads older records without submitting a command.
    for _ in 0..30 {
        if let Some(action) = rt
            .ui
            .key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
        {
            rt.action(action);
        }
        if rt.ui.history_loading {
            break;
        }
        terminal.draw(|f| render::draw(f, &mut rt.ui)).unwrap();
    }
    assert!(rt.ui.history_loading);
    assert!(rt.operation.is_none());
    terminal.draw(|f| render::draw(f, &mut rt.ui)).unwrap();
    let area = terminal.backend().buffer().area;
    let pane = render::geometry(area, &rt.ui).content;
    let visible_before = screen(&terminal)
        .lines()
        .skip(pane.y as usize + 2)
        .take(6)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let pending = rt.history_pending.clone();
    let requests = rt.jobs.len();
    assert!(rt.mouse(
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: pane.x + 3,
            row: pane.y + 3,
            modifiers: KeyModifiers::NONE
        },
        area
    ));
    assert_eq!(rt.history_pending, pending);
    assert_eq!(
        rt.jobs.len(),
        requests,
        "repeated wheel events must not duplicate a pending read"
    );
    while rt.ui.history_loading {
        navigate(&mut rt).await;
    }
    assert_eq!(rt.ui.messages.len(), 100);
    assert_eq!(rt.ui.editor.text, "保留这份草稿");
    assert_eq!(rt.ui.editor.cursor, draft_cursor);
    assert_eq!(rt.ui.focus, Focus::Input);
    terminal.draw(|f| render::draw(f, &mut rt.ui)).unwrap();
    let visible_after = screen(&terminal)
        .lines()
        .skip(pane.y as usize + 2)
        .take(6)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        visible_before, visible_after,
        "prepending a page must preserve the visible conversation"
    );
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
    let oldest = rt.ui.history_before.clone();
    rt.ui.prepend_history(page);
    assert_eq!(rt.ui.messages.len(), count);
    assert_eq!(rt.ui.history_before, oldest);
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
    rt.ui.editor.set("生成过程中的草稿".into());
    rt.ui.follow = false;
    rt.ui.chat_scroll = 0;
    terminal.draw(|f| render::draw(f, &mut rt.ui)).unwrap();
    rt.mouse(
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: pane.x + 3,
            row: pane.y + 3,
            modifiers: KeyModifiers::NONE,
        },
        area,
    );
    assert!(rt.ui.history_loading);
    let button = rt.ui.latest_button.unwrap();
    rt.mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: button.x + 1,
            row: button.y,
            modifiers: KeyModifiers::NONE,
        },
        area,
    );
    assert!(rt.ui.follow);
    while rt.ui.history_loading {
        navigate(&mut rt).await;
    }
    assert!(rt.operation.is_some());
    assert!(rt.ui.busy);
    assert!(rt.ui.history_before.is_none());
    assert_eq!(rt.ui.editor.text, "生成过程中的草稿");
    terminal.draw(|f| render::draw(f, &mut rt.ui)).unwrap();
    assert!(
        rt.ui.follow,
        "a pending history response must not undo return-to-latest"
    );
    assert!(rt.ui.latest_button.is_none());

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

#[tokio::test]
async fn provider_commands_select_alias_without_credentials_and_preserve_selection_on_error() {
    let (_dir, w, _) = fixture().await;
    let value = serde_json::json!({"schema_version":1,"default_model":"research","providers":{"company":{"protocol":"openai_responses","base_url":"https://example.invalid/v1","state_scope":"account","auth":{"kind":"env","variable":format!("LUMEN_ABSENT_{}",uuid::Uuid::now_v7().simple())}}},"models":{"research":{"provider":"company","model_id":"actual-id","capabilities":{"context_tokens":64000,"max_output_tokens":4096,"tools":true}}}});
    std::fs::write(
        w.root().join(".lumen/providers.yaml"),
        serde_yaml_ng::to_string(&value).unwrap(),
    )
    .unwrap();
    let mut app = Application::new();
    app.workspace = Some(w.clone());
    app.command(Command::New, CancellationToken::new())
        .await
        .unwrap();
    let active = app.manager.as_ref().unwrap().active().unwrap();
    assert_eq!(active.metadata().0.model.model, "research");
    assert!(app
        .command(Command::Models, CancellationToken::new())
        .await
        .unwrap()
        .contains("research"));
    assert!(app
        .command(Command::Providers, CancellationToken::new())
        .await
        .unwrap()
        .contains("company"));
    let revision = active.metadata().1;
    assert!(app
        .command(
            Command::Model(Some("missing".into())),
            CancellationToken::new()
        )
        .await
        .is_err());
    assert_eq!(active.metadata().1, revision);
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    assert!(app
        .chat("hello".into(), CancellationToken::new(), tx)
        .await
        .is_err());
    assert!(active
        .runtime()
        .unwrap()
        .snapshot()
        .await
        .messages
        .is_empty());
    assert_eq!(active.metadata().0.model.model, "research");
    app.command(
        Command::Model(Some("mock".into())),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    app.chat("demo".into(), CancellationToken::new(), tx)
        .await
        .unwrap();
    assert!(active
        .runtime()
        .unwrap()
        .snapshot()
        .await
        .messages
        .iter()
        .any(|m| matches!(m, AgentMessage::Assistant(_))));
    assert!(service::session_read_command(&Command::Models));
    assert!(service::read_session_command(
        app.manager.as_ref().unwrap().repository().clone(),
        None,
        Command::Providers
    )
    .await
    .is_ok());
    app.command(Command::New, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        app.manager
            .as_ref()
            .unwrap()
            .active()
            .unwrap()
            .metadata()
            .0
            .model
            .model,
        "research"
    );
    app.manager.as_mut().unwrap().close(None).unwrap();
    w.close(CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn study_commands_manage_routes_and_tutor_through_the_normal_session() {
    let (_dir, w, _) = fixture().await;
    let requests = Arc::new(Mutex::new(vec![]));
    let mut app = Application::new();
    app.workspace = Some(w.clone());
    app.provider = Some(Arc::new(TestProvider {
        requests: requests.clone(),
        wait: false,
    }));
    for input in [
        "/goal create '学习事务' '能解释并发异常'",
        "/goal set 学习事务 baseline '了解 SQL'",
        "/goal set 学习事务 weekly_minutes 240",
        "/plan add 学习事务 30 原子性 '解释回滚与提交'",
        "/plan add 学习事务 45 隔离性 '解释隔离级别与并发异常'",
    ] {
        app.command(commands::parse(input).unwrap(), CancellationToken::new())
            .await
            .unwrap();
    }
    let study = crate::study::StudyService(w.clone());
    let goal = study
        .resolve("学习事务", CancellationToken::new())
        .await
        .unwrap();
    let progress = study
        .progress(goal.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(progress.total_minutes, 75);
    let first = progress.units[0].id.clone();
    let second = progress.units[1].id.clone();
    for input in [
        format!("/plan move 学习事务 {second} 1"),
        format!("/plan done {second} 已完成练习"),
        "/review 学习事务 原子性需要更多例子".into(),
    ] {
        app.command(commands::parse(&input).unwrap(), CancellationToken::new())
            .await
            .unwrap();
    }
    let report = app
        .command(
            commands::parse("/progress 学习事务").unwrap(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(report.contains("1/2"));
    assert!(report.contains(&format!("/study {first}")));
    let progress = study
        .progress(goal, CancellationToken::new())
        .await
        .unwrap();
    assert!(progress.recent_reviews[0].body.contains("更多例子"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.dispatch_interactive(
        Command::Study(first.to_string()),
        CancellationToken::new(),
        tx,
    )
    .await
    .unwrap();
    assert!(rx.try_recv().is_ok());
    {
        let requests = requests.lock().unwrap();
        assert!(requests.iter().flat_map(|r|&r.messages).any(|m|matches!(m,AgentMessage::User(parts) if parts.iter().any(|ContentPart::Text(t)|t.contains("诊断问题") && t.contains("解释回滚与提交")))));
        assert!(requests
            .iter()
            .flat_map(|r| &r.tools)
            .any(|t| t.name == "plan_apply"));
    }
    assert!(app.manager.as_ref().unwrap().active().is_some());
    app.manager.as_mut().unwrap().close(None).unwrap();
    w.close(CancellationToken::new()).await.unwrap();
}

#[test]
fn model_status_tracks_the_selected_model_instead_of_legacy_environment_variables() {
    let mut ui = Ui::new("research".into(), Theme::Auto);
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    assert!(screen(&terminal).contains("就绪"));
    assert!(!screen(&terminal).contains("演示模式"));
    ui.model = "mock".into();
    terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    assert!(screen(&terminal).contains("演示模式"));
}

#[test]
fn provider_api_key_is_masked_and_never_enters_chat_or_session_drafts() {
    let secret = "test-only-sensitive-provider-key";
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.editor.set("保留原来的草稿".into());
    ui.provider_dialog = Some(provider_setup::ProviderDialog::add(
        Some("openrouter".into()),
        None,
    ));
    ui.provider_dialog.as_mut().unwrap().paste(secret);
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    assert!(screen(&terminal).contains("*****"));
    assert!(!screen(&terminal).contains(secret));
    assert!(!serde_json::to_string(&ui.session_ui())
        .unwrap()
        .contains(secret));
    assert_eq!(ui.editor.text, "保留原来的草稿");
    assert!(ui.messages.is_empty());
    let action = ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(
        matches!(action,Action::Provider(action) if matches!(*action,provider_setup::ProviderAction::Add {key:Some(ref key),..} if key==secret))
    );
    assert!(ui.provider_dialog.is_none());
    assert_eq!(ui.editor.text, "保留原来的草稿");
    ui.provider_dialog = Some(provider_setup::ProviderDialog::key("deepseek".into()));
    ui.provider_dialog.as_mut().unwrap().paste(secret);
    assert!(ui
        .key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .is_none());
    assert!(ui.provider_dialog.is_none());
    assert!(!serde_json::to_string(&ui.session_ui())
        .unwrap()
        .contains(secret));
}

#[test]
fn discovered_models_can_be_filtered_selected_and_configured_without_guessed_limits() {
    let catalog = crate::provider::ProviderModelCatalog {
        provider: "custom".into(),
        connection_key: "revision".into(),
        models: vec![
            crate::provider::DiscoveredModel {
                id: "first".into(),
                name: None,
                context_tokens: None,
                max_output_tokens: None,
            },
            crate::provider::DiscoveredModel {
                id: "second".into(),
                name: None,
                context_tokens: None,
                max_output_tokens: None,
            },
        ],
    };
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.provider_dialog = Some(provider_setup::ProviderDialog::models(catalog, vec![]));
    ui.provider_dialog.as_mut().unwrap().paste("second");
    assert!(ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .is_none());
    assert!(ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .is_none());
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    assert!(screen(&terminal).contains("请填写模型的上下文容量"));
    ui.provider_dialog.as_mut().unwrap().paste("64000");
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    ui.provider_dialog.as_mut().unwrap().paste("4096");
    let action = ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .unwrap();
    assert!(
        matches!(action,Action::Provider(action) if matches!(*action,provider_setup::ProviderAction::Model(ref request) if request.model_id=="second" && request.context_tokens==64000 && request.output_tokens==4096))
    );
}

struct DiscoveryHttp;
#[async_trait::async_trait]
impl crate::provider::HttpTransport for DiscoveryHttp {
    async fn send(
        &self,
        _: crate::provider::HttpRequest,
    ) -> Result<crate::provider::HttpResponse, ProviderError> {
        Err(ProviderError::protocol("discovery must not send chat"))
    }
    async fn get(
        &self,
        request: crate::provider::HttpRequest,
    ) -> Result<crate::provider::HttpResponse, ProviderError> {
        assert!(request.url.ends_with("/v1/models"));
        assert!(request
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer test-ui-key"));
        Ok(crate::provider::HttpResponse {status:200,headers:std::collections::BTreeMap::from([("content-type".into(),"application/json".into())]),body:Box::pin(futures_util::stream::iter(vec![Ok(serde_json::to_vec(&serde_json::json!({"data":[{"id":"chat-model","context_window":64000,"max_output_tokens":8192}]})).unwrap())]))})
    }
}
#[tokio::test]
async fn provider_form_actions_save_discover_and_select_a_model_for_new_sessions() {
    let (_dir, w, _) = fixture().await;
    let mut app = Application::new();
    app.workspace = Some(w.clone());
    app.ensure_manager().await.unwrap();
    app.providers = Some(Arc::new(crate::provider::ProviderRegistry::with_transport(
        w.root(),
        Arc::new(DiscoveryHttp),
    )));
    let result = app
        .configure_provider(
            provider_setup::ProviderAction::Add {
                name: "gateway".into(),
                connection: Box::new(crate::provider::compatible_connection(
                    "https://gateway.test/v1".into(),
                    crate::provider::AuthConfig::None,
                )),
                key: Some("test-ui-key".into()),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(result, CommandOutcome::Text(_)));
    let result = app
        .dispatch(
            Command::Provider(crate::commands::ProviderCommand::Models("gateway".into())),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let catalog = match result {
        CommandOutcome::ProviderModels(catalog) => catalog,
        _ => panic!("expected model picker"),
    };
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.provider_dialog = Some(provider_setup::ProviderDialog::models(*catalog, vec![]));
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let action = match ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .unwrap()
    {
        Action::Provider(action) => action,
        _ => panic!("expected model save"),
    };
    app.configure_provider(*action, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(app.model, "gateway:chat-model");
    app.command(Command::New, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        app.manager
            .as_ref()
            .unwrap()
            .active()
            .unwrap()
            .metadata()
            .0
            .model
            .model,
        "gateway:chat-model"
    );
    let config = std::fs::read_to_string(w.root().join(".lumen/providers.yaml")).unwrap();
    assert!(!config.contains("test-ui-key"));
    assert!(config.contains("gateway:chat-model"));
    app.manager.as_mut().unwrap().close(None).unwrap();
    w.close(CancellationToken::new()).await.unwrap();
}

#[test]
fn login_picker_filters_presets_and_configured_login_preserves_custom_routes() {
    use crate::provider::*;
    let dir = tempfile::tempdir().unwrap();
    let registry = ProviderRegistry::new(dir.path());
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.provider_dialog = Some(provider_setup::ProviderDialog::providers(
        registry.provider_catalog().unwrap(),
        false,
    ));
    ui.provider_dialog.as_mut().unwrap().paste("anthropic");
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    ui.provider_dialog
        .as_mut()
        .unwrap()
        .paste("masked-anthropic-test-key");
    let action = ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .unwrap();
    assert!(
        matches!(action, Action::Provider(action) if matches!(*action,
        provider_setup::ProviderAction::Add {ref name, ref connection, key: Some(ref key)}
        if name == "anthropic" && connection.protocol == ProtocolKind::AnthropicMessages && key == "masked-anthropic-test-key"))
    );
    registry
        .add_provider(
            "openai".into(),
            compatible_connection("https://company.test/v1".into(), AuthConfig::None),
            Some("previous-key"),
        )
        .unwrap();
    let entry = registry
        .provider_catalog()
        .unwrap()
        .into_iter()
        .find(|p| p.id == "openai")
        .unwrap();
    ui.provider_dialog = Some(provider_setup::ProviderDialog::login(&entry));
    ui.provider_dialog
        .as_mut()
        .unwrap()
        .paste("replacement-key");
    let action = ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .unwrap();
    assert!(
        matches!(action, Action::Provider(action) if matches!(*action,
        provider_setup::ProviderAction::Auth {ref provider, key: Some(ref key), ..}
        if provider == "openai" && key == "replacement-key"))
    );
}

#[test]
fn custom_provider_form_selects_protocol_and_explicit_authentication() {
    use crate::provider::*;
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    ui.provider_dialog = Some(provider_setup::ProviderDialog::add(None, None));
    ui.provider_dialog.as_mut().unwrap().paste("company");
    ui.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    ui.provider_dialog
        .as_mut()
        .unwrap()
        .paste("https://company.test/v1");
    ui.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    ui.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)); // Responses
    ui.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    ui.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)); // None -> Env
    ui.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    ui.provider_dialog
        .as_mut()
        .unwrap()
        .paste("COMPANY_API_KEY");
    let action = ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .unwrap();
    assert!(
        matches!(action, Action::Provider(action) if matches!(*action,
        provider_setup::ProviderAction::Add {ref connection, key: None, ..}
        if connection.protocol == ProtocolKind::OpenaiResponses && matches!(&connection.auth, AuthConfig::Env {variable} if variable == "COMPANY_API_KEY")))
    );
}

#[test]
fn model_picker_selects_registered_aliases_directly_and_can_refresh_a_provider() {
    use crate::provider::*;
    let mut ui = Ui::new("mock".into(), Theme::Auto);
    let models = vec![AvailableModel {
        provider: "company".into(),
        alias: Some("research".into()),
        model: DiscoveredModel {
            id: "model-a".into(),
            name: None,
            context_tokens: Some(64000),
            max_output_tokens: Some(4096),
        },
        connection_key: "test".into(),
    }];
    ui.provider_dialog = Some(provider_setup::ProviderDialog::available(
        models,
        vec!["company".into()],
    ));
    ui.provider_dialog.as_mut().unwrap().paste("research");
    let action = ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(
        matches!(action, Action::Provider(action) if matches!(*action, provider_setup::ProviderAction::Select(ref alias) if alias == "research"))
    );
    ui.provider_dialog = Some(provider_setup::ProviderDialog::available(
        vec![],
        vec!["company".into()],
    ));
    let action = ui
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(
        matches!(action, Action::Provider(action) if matches!(*action, provider_setup::ProviderAction::Refresh(ref p) if p == "company"))
    );
}

#[tokio::test]
async fn login_and_model_commands_expose_menus_and_auth_save_works_offline() {
    let (_dir, w, _) = fixture().await;
    let mut app = Application::new();
    app.workspace = Some(w.clone());
    let menu = app
        .dispatch(Command::Login(None), CancellationToken::new())
        .await
        .unwrap();
    assert!(
        matches!(menu, CommandOutcome::ProviderMenu(ref entries, None, false) if entries.iter().any(|e| e.id == "openai") && entries.iter().any(|e| e.id == "anthropic"))
    );
    assert!(app
        .dispatch(
            Command::Login(Some("unknown".into())),
            CancellationToken::new()
        )
        .await
        .is_err());
    // This transport never completes GET. Saving authentication must finish without querying it.
    struct Offline;
    #[async_trait::async_trait]
    impl crate::provider::HttpTransport for Offline {
        async fn send(
            &self,
            _: crate::provider::HttpRequest,
        ) -> Result<crate::provider::HttpResponse, ProviderError> {
            panic!("auth save must not POST")
        }
        async fn get(
            &self,
            _: crate::provider::HttpRequest,
        ) -> Result<crate::provider::HttpResponse, ProviderError> {
            panic!("auth save must not GET")
        }
    }
    app.providers = Some(Arc::new(crate::provider::ProviderRegistry::with_transport(
        w.root(),
        Arc::new(Offline),
    )));
    app.configure_provider(
        provider_setup::ProviderAction::Add {
            name: "company".into(),
            connection: Box::new(crate::provider::compatible_connection(
                "https://company.test/v1".into(),
                crate::provider::AuthConfig::None,
            )),
            key: Some("offline-test-key".into()),
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        matches!(app.dispatch(Command::Model(None), CancellationToken::new()).await.unwrap(), CommandOutcome::AvailableModels(ref models, ref providers) if models.is_empty() && providers == &["company"])
    );
    app.dispatch(
        Command::Logout(Some("company".into())),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        matches!(app.dispatch(Command::Model(None), CancellationToken::new()).await.unwrap(), CommandOutcome::AvailableModels(ref models, ref providers) if models.is_empty() && providers.is_empty())
    );
    assert!(
        !std::fs::read_to_string(w.root().join(".lumen/provider-credentials.json"))
            .unwrap()
            .contains("offline-test-key")
    );
    app.manager.as_mut().unwrap().close(None).unwrap();
    w.close(CancellationToken::new()).await.unwrap();
}

#[test]
fn directory_stays_visible_across_view_switches_and_workspace_changes() {
    let mut ui = Ui::new("research-model".into(), Theme::Dark);
    ui.session_title = "事务与并发控制".into();
    ui.editor.set("请解释数据库的隔离级别。".into());
    let area = ratatui::layout::Rect::new(0, 0, 120, 32);
    let standalone = render::geometry(area, &ui);
    assert!(standalone.tree.width > 0);
    assert_eq!(standalone.input.height, 4);
    ui.key(KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE));
    assert_eq!(ui.focus, Focus::Tree);
    ui.key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
    assert_eq!(ui.view, View::Read);
    assert_eq!(ui.focus, Focus::Content);
    assert_eq!(render::geometry(area, &ui).tree, standalone.tree);
    ui.key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert_eq!(ui.view, View::Chat);
    assert_eq!(render::geometry(area, &ui).tree, standalone.tree);
    for _ in 0..2 {
        ui.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(ui.focus, Focus::Tree);
        assert_eq!(render::geometry(area, &ui).tree, standalone.tree);
    }
    ui.workspace_path = Some("/tmp/learning".into());
    ui.workspace_name = "learning".into();
    let workspace = render::geometry(area, &ui);
    assert_eq!(workspace.tree, standalone.tree);
    assert_eq!(ui.editor.text, "请解释数据库的隔离级别。");
    ui.editor.insert("\n请给出一个例子。\n说明适用场景。");
    let multiline = render::geometry(area, &ui);
    assert!(multiline.input.height > workspace.input.height);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    ui.focus = Focus::Input;
    ui.notify("连接参数有误，请检查配置。", true);
    terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
    let output = screen(&terminal);
    assert!(output.contains("事务与并发控制"));
    assert!(output.contains("连接参数有误，请检查配置。"));
    assert!(output.contains("research-model"));
    assert!(multiline
        .input
        .contains(terminal.get_cursor_position().unwrap()));
}

#[test]
fn dialog_focus_survives_narrow_windows_and_long_cjk_search_without_exposing_secrets() {
    for (width, height) in [(24, 9), (40, 12), (88, 24), (120, 36)] {
        let mut ui = Ui::new("mock".into(), Theme::Auto);
        ui.provider_dialog = Some(provider_setup::ProviderDialog::key("company".into()));
        ui.provider_dialog
            .as_mut()
            .unwrap()
            .paste("private-test-key");
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
        assert!(!screen(&terminal).contains("private-test-key"));
        assert!(screen(&terminal).contains("*****"));
        let caret = terminal.get_cursor_position().unwrap();
        assert!(caret.x < width && caret.y < height - 2);
        ui.provider_dialog = Some(provider_setup::ProviderDialog::providers(vec![], false));
        ui.provider_dialog
            .as_mut()
            .unwrap()
            .paste(&"供应商搜索".repeat(30));
        terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
        let caret = terminal.get_cursor_position().unwrap();
        assert!(caret.x < width && caret.y < height - 2);
        // The selector owns focus; confirmation must open the custom form, not submit chat.
        assert!(ui
            .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .is_none());
        assert!(ui.messages.is_empty());
        assert!(ui.provider_dialog.is_some());
    }
}

#[test]
fn automatic_workbench_uses_terminal_colors_for_content_selection_and_dialogs() {
    use ratatui::style::Modifier;
    assert_eq!(Theme::default(), Theme::Auto);
    assert_eq!(Theme::Auto.selected().fg, Some(Color::Reset));
    assert_eq!(Theme::Auto.selected().bg, Some(Color::Reset));
    assert!(Theme::Auto
        .selected()
        .add_modifier
        .contains(Modifier::REVERSED));
    let mut ui = Ui::new("company:chat".into(), Theme::Auto);
    ui.workspace_path = Some("/tmp/palette-workspace".into());
    ui.workspace_name = "学习笔记".into();
    ui.tree.objects = vec![ObjectMetadata::new(ObjectKind::Concept, "事务隔离")];
    ui.message("你", "如何理解事务隔离？");
    ui.message("Lumen", "## 事务隔离\n\n可以先查看[参考资料](https://example.test)。\n\n```sql\nBEGIN;\nCOMMIT;\n```");
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    for popup in 0..4 {
        ui.provider_dialog = match popup {
            1 => Some(provider_setup::ProviderDialog::providers(vec![], false)),
            2 => Some(provider_setup::ProviderDialog::key("company".into())),
            _ => None,
        };
        ui.session_panel = popup == 3;
        terminal.draw(|f| render::draw(f, &mut ui)).unwrap();
        let buffer = terminal.backend().buffer();
        assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
        assert!(buffer
            .content
            .iter()
            .all(|cell| !matches!(cell.fg, Color::Rgb(..) | Color::Indexed(_))));
        if popup == 0 {
            let tree = render::geometry(buffer.area, &ui).tree;
            assert!((tree.y..tree.bottom()).any(|y| buffer[(tree.x + 1, y)]
                .modifier
                .contains(Modifier::REVERSED)));
        }
    }
}

#[test]
fn mouse_scroll_preserves_drafts_and_latest_click_resumes_following_streamed_replies() {
    let mut rt = Runtime::new(Application::new(), Theme::Auto);
    rt.ui.workspace_path = Some("/tmp/scroll-workspace".into());
    rt.ui.tree.objects = (0..5)
        .map(|i| ObjectMetadata::new(ObjectKind::Concept, format!("知识 {i}")))
        .collect();
    for i in 0..80 {
        rt.ui.message("Lumen", format!("第 {i} 条历史回复。"));
    }
    rt.ui.editor.set("尚未发送的中文草稿".into());
    rt.ui.editor.left();
    let draft = rt.ui.editor.text.clone();
    let cursor = rt.ui.editor.cursor;
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    terminal.draw(|f| render::draw(f, &mut rt.ui)).unwrap();
    let area = terminal.backend().buffer().area;
    let geometry = render::geometry(area, &rt.ui);
    let event = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let latest = rt.ui.chat_scroll;
    assert!(rt.mouse(
        event(
            MouseEventKind::ScrollUp,
            geometry.content.x + 3,
            geometry.content.y + 3
        ),
        area
    ));
    assert!(rt.ui.chat_scroll < latest);
    assert!(!rt.ui.follow);
    let reading_position = rt.ui.chat_scroll;
    rt.ui.agent(AgentEvent::MessageDelta {
        delta: "新回复仍在继续生成。".into(),
    });
    terminal.draw(|f| render::draw(f, &mut rt.ui)).unwrap();
    assert_eq!(rt.ui.chat_scroll, reading_position);
    assert_eq!(rt.ui.focus, Focus::Input);
    assert_eq!(rt.ui.editor.text, draft);
    assert_eq!(rt.ui.editor.cursor, cursor);
    assert!(rt.mouse(
        event(
            MouseEventKind::ScrollDown,
            geometry.content.x + 3,
            geometry.content.y + 3
        ),
        area
    ));
    assert_eq!(rt.ui.chat_scroll, reading_position + 3);
    assert!(rt.mouse(
        event(
            MouseEventKind::ScrollUp,
            geometry.content.x + 3,
            geometry.content.y + 3
        ),
        area
    ));
    assert_eq!(rt.ui.chat_scroll, reading_position);
    let button = rt
        .ui
        .latest_button
        .expect("history view offers a return-to-latest control");
    assert!(screen(&terminal).contains("回到最新"));
    assert!(rt.mouse(
        event(
            MouseEventKind::Down(MouseButton::Left),
            button.x + 1,
            button.y
        ),
        area
    ));
    terminal.draw(|f| render::draw(f, &mut rt.ui)).unwrap();
    assert!(rt.ui.follow);
    assert!(rt.ui.latest_button.is_none());
    assert_eq!(
        rt.ui.chat_scroll,
        rt.ui.content_lines - rt.ui.viewport_height
    );
    assert!(screen(&terminal).contains("新回复仍在继续生成。"));
    assert_eq!(rt.ui.editor.text, draft);
    assert_eq!(rt.ui.editor.cursor, cursor);
    let old_scroll = rt.ui.chat_scroll;
    assert!(!rt.mouse(
        event(
            MouseEventKind::ScrollUp,
            geometry.input.x + 3,
            geometry.input.y + 1
        ),
        area
    ));
    assert_eq!(rt.ui.chat_scroll, old_scroll);
    let tree = geometry.tree;
    assert!(rt.mouse(
        event(MouseEventKind::ScrollDown, tree.x + 2, tree.y + 2),
        area
    ));
    assert!(rt.ui.tree.selected > 0);
    assert_eq!(rt.ui.chat_scroll, old_scroll);
    assert_eq!(rt.ui.focus, Focus::Input);
    rt.ui
        .key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert!(!rt.ui.follow);
    rt.ui
        .key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
    assert!(rt.ui.follow);
    assert_eq!(rt.ui.editor.text, draft);
    rt.ui.provider_dialog = Some(provider_setup::ProviderDialog::key("company".into()));
    assert!(!rt.mouse(
        event(
            MouseEventKind::ScrollUp,
            geometry.content.x + 3,
            geometry.content.y + 3
        ),
        area
    ));
    assert!(
        rt.ui.follow,
        "modal dialogs must block scrolling the conversation behind them"
    );
}

#[test]
fn late_history_pages_cannot_change_the_newly_activated_session() {
    let mut rt = Runtime::new(Application::new(), Theme::Auto);
    let old = HistoryRequest {
        epoch: rt.epoch,
        identity: crate::session::SessionIdentity {
            store_id: "store".into(),
            session_id: "old".into(),
        },
        activation: 1,
        before: "old-cursor".into(),
    };
    let current = HistoryRequest {
        identity: crate::session::SessionIdentity {
            store_id: "store".into(),
            session_id: "current".into(),
        },
        activation: 2,
        before: "current-cursor".into(),
        ..old.clone()
    };
    rt.ui.session_identity = Some(current.identity.clone());
    rt.ui.session_activation = current.activation;
    rt.ui.history_before = Some(current.before.clone());
    rt.ui.editor.set("新会话的草稿".into());
    rt.ui.message("Lumen", "新会话的内容");
    rt.ui.history_loading = true;
    rt.history_pending = Some(current.clone());
    rt.navigation(Navigation::History {
        request: old,
        result: Ok(crate::session::HistoryPage {
            messages: vec![],
            before: None,
        }),
    });
    assert_eq!(rt.history_pending, Some(current));
    assert!(rt.ui.history_loading);
    assert_eq!(rt.ui.history_before.as_deref(), Some("current-cursor"));
    assert_eq!(rt.ui.messages.len(), 1);
    assert_eq!(rt.ui.editor.text, "新会话的草稿");
}
