use super::{input::Editor, markdown, text, theme::Theme, tree::Tree};
use crate::{
    agent::{AgentEvent, AgentMessage, ContentPart, StopReason},
    knowledge::*,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Chat,
    Read,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Content,
    Input,
    Search,
}
pub struct Document {
    pub object: ObjectView,
    pub path: Vec<ObjectId>,
    pub scroll: usize,
    pub rendered: markdown::Rendered,
    pub render_key: Option<(usize, Theme)>,
    pub link: Option<usize>,
    pub selection: Option<(usize, usize)>,
}
#[derive(Clone)]
pub struct Reference {
    pub id: ObjectId,
    pub title: String,
    pub heading: Option<String>,
    pub selection: Option<(Revision, String)>,
}
pub struct Message {
    pub role: &'static str,
    pub body: String,
}
pub enum Action {
    Command(crate::commands::Command),
    Submit(String),
    Open(ObjectId, Vec<ObjectId>),
    Attach(ObjectId),
    Reload,
    Cancel,
    Quit,
    Link(String),
}
pub enum Completion {
    Command(String),
    Object(ObjectId),
}
pub struct Candidate {
    pub label: String,
    pub hint: String,
    pub value: Completion,
}
pub const COMMANDS: &[(&str, &str)] = &[
    ("/chat", "显示会话"),
    ("/read", "返回文档"),
    ("/open", "打开知识标题或 ID"),
    ("/workspace init", "创建工作区"),
    ("/workspace open", "打开工作区"),
    ("/workspace clone", "克隆工作区"),
    ("/theme auto", "跟随终端主题"),
    ("/theme light", "浅色主题"),
    ("/theme dark", "深色主题"),
    ("/search", "检索知识"),
    ("/new", "新建会话"),
    ("/sessions", "查找并选择历史会话"),
    ("/session", "当前会话详情"),
    ("/session rename", "重命名当前会话"),
    ("/session archive", "归档会话"),
    ("/session unarchive", "取消归档"),
    ("/session close", "关闭当前会话"),
    ("/session import", "导入旧 JSONL 文件"),
    ("/session preview", "只读浏览旧 JSONL"),
    ("/clear", "清空对话显示"),
    ("/model", "查看或切换模型"),
    ("/status", "查看工作区状态"),
    ("/context", "查看上下文窗口与活跃 Skill"),
    ("/compact", "安排上下文压缩"),
    ("/skills", "查看可用 Skill"),
    ("/resume", "恢复工作区会话"),
    ("/commit", "提交工作区"),
    ("/sync", "同步"),
    ("/history", "版本历史"),
    ("/diff", "查看修改"),
    ("/restore", "恢复版本"),
    ("/index status", "索引状态"),
    ("/index refresh", "刷新索引"),
    ("/validate", "校验知识"),
    ("/stop", "停止当前任务"),
    ("/help", "帮助"),
    ("/quit", "退出"),
];
pub struct Ui {
    pub session_identity: Option<crate::session::SessionIdentity>,
    pub session_activation: u64,
    pub session_title: String,
    pub history_before: Option<String>,
    pub history_keys: std::collections::BTreeSet<String>,
    pub session_choices: Vec<crate::session::ManagedSessionInfo>,
    pub session_choice: usize,
    pub session_filter: String,
    pub session_panel: bool,
    pub session_next: Option<String>,
    pub session_query: String,
    pub theme: Theme,
    pub focus: Focus,
    pub view: View,
    pub tree: Tree,
    pub sidebar: bool,
    pub editor: Editor,
    pub search: Editor,
    pub search_tree: bool,
    pub reading_query: String,
    pub document: Option<Document>,
    pub positions: BTreeMap<ObjectId, usize>,
    pub reference: Option<Reference>,
    pub messages: Vec<Message>,
    pub streaming: Option<usize>,
    pub chat_scroll: usize,
    pub follow: bool,
    pub chat_links: Vec<markdown::Link>,
    pub chat_link: Option<usize>,
    pub unread: usize,
    pub workspace_name: String,
    pub workspace_path: Option<std::path::PathBuf>,
    pub model: String,
    pub demo: bool,
    pub busy: bool,
    pub loading: bool,
    pub activity: String,
    pub notice: Option<String>,
    pub error: bool,
    pub candidate: usize,
    pub completions_hidden: bool,
    pub quit_armed: bool,
    pub last_submission: Option<String>,
    pub viewport_height: usize,
    pub content_lines: usize,
    pub input_width: usize,
}
impl Ui {
    pub fn session_ui(&self) -> crate::session::SessionUiState {
        crate::session::SessionUiState {
            draft: self.editor.text.clone(),
            chat_scroll: self.chat_scroll,
            reading: self
                .document
                .as_ref()
                .map(|document| crate::session::ReadingPosition {
                    id: document.object.metadata.id.clone(),
                    revision: document.object.revision.0.clone(),
                    scroll: document.scroll,
                    heading: None,
                }),
            reference: self
                .reference
                .as_ref()
                .map(|reference| crate::session::ReferenceState {
                    id: reference.id.clone(),
                    title: reference.title.clone(),
                    heading: reference.heading.clone(),
                    selection: reference
                        .selection
                        .as_ref()
                        .map(|(revision, text)| (revision.0.clone(), text.clone())),
                }),
        }
    }
    pub fn close_session(&mut self) {
        self.session_identity = None;
        self.session_activation += 1;
        self.session_title.clear();
        self.history_before = None;
        self.history_keys.clear();
        self.messages.clear();
        self.streaming = None;
        self.reference = None;
        self.document = None;
        self.editor.set(String::new());
        self.chat_links.clear();
        self.chat_link = None;
        self.chat_scroll = 0;
    }
    pub fn activate_session(&mut self, view: &crate::session::SessionView) {
        self.close_session();
        self.session_identity = Some(view.info.identity.clone());
        self.session_activation = view.activation;
        self.session_title = view.info.metadata.title.clone();
        self.model = view.info.metadata.model.model.clone();
        self.history_before = view.history.before.clone();
        for message in &view.history.messages {
            self.history_keys
                .insert(crate::session::history_key(&message.record_ref));
            self.message(
                if message.role == "user" {
                    "你"
                } else {
                    "Lumen"
                },
                &message.text,
            );
        }
        self.editor.set(view.ui.draft.clone());
        self.chat_scroll = view.ui.chat_scroll;
        self.follow = self.chat_scroll == 0;
        self.unread = 0;
        self.reference = view.ui.reference.as_ref().map(|reference| Reference {
            id: reference.id.clone(),
            title: reference.title.clone(),
            heading: reference.heading.clone(),
            selection: reference
                .selection
                .as_ref()
                .map(|(revision, text)| (Revision(revision.clone()), text.clone())),
        });
        self.view = View::Chat;
        if !view.info.diagnostics.is_empty() {
            self.notify(view.info.diagnostics.join("; "), !view.writable);
        }
    }
    pub fn prepend_history(&mut self, page: crate::session::HistoryPage) {
        let mut messages = vec![];
        for message in page.messages {
            if self
                .history_keys
                .insert(crate::session::history_key(&message.record_ref))
            {
                messages.push(Message {
                    role: if message.role == "user" {
                        "你"
                    } else {
                        "Lumen"
                    },
                    body: text::clean(&message.text),
                });
            }
        }
        messages.append(&mut self.messages);
        self.messages = messages;
        self.history_before = page.before;
        self.follow = false;
    }
    pub fn new(model: String, theme: Theme) -> Self {
        Self {
            session_identity: None,
            session_activation: 0,
            session_title: String::new(),
            history_before: None,
            history_keys: Default::default(),
            session_choices: vec![],
            session_choice: 0,
            session_filter: String::new(),
            session_panel: false,
            session_next: None,
            session_query: String::new(),
            theme,
            focus: Focus::Input,
            view: View::Chat,
            tree: Tree::default(),
            sidebar: true,
            editor: Editor::default(),
            search: Editor::default(),
            search_tree: false,
            reading_query: String::new(),
            document: None,
            positions: BTreeMap::new(),
            reference: None,
            messages: vec![],
            streaming: None,
            chat_scroll: 0,
            follow: true,
            chat_links: vec![],
            chat_link: None,
            unread: 0,
            workspace_name: "未打开工作区".into(),
            workspace_path: None,
            model,
            demo: std::env::var("LUMEN_MODEL_ENDPOINT").is_err()
                || std::env::var("LUMEN_MODEL_KEY").is_err(),
            busy: false,
            loading: false,
            activity: String::new(),
            notice: None,
            error: false,
            candidate: 0,
            completions_hidden: false,
            quit_armed: false,
            last_submission: None,
            viewport_height: 1,
            content_lines: 0,
            input_width: 1,
        }
    }
    pub fn notify(&mut self, value: impl Into<String>, error: bool) {
        self.notice = Some(text::clean(&value.into()));
        self.error = error;
    }
    pub fn cycle(&mut self) {
        self.focus = match self.focus {
            Focus::Tree => Focus::Content,
            Focus::Content => Focus::Input,
            _ => {
                if self.sidebar {
                    Focus::Tree
                } else {
                    Focus::Content
                }
            }
        };
        self.completions_hidden = true;
    }
    pub fn chat(&mut self) {
        self.view = View::Chat;
        self.unread = 0;
    }
    pub fn open(&mut self, object: ObjectView, path: Vec<ObjectId>) {
        if let Some(old) = &self.document {
            self.positions
                .insert(old.object.metadata.id.clone(), old.scroll);
        }
        let scroll = self
            .positions
            .get(&object.metadata.id)
            .copied()
            .unwrap_or(0);
        self.document = Some(Document {
            object,
            path,
            scroll,
            rendered: markdown::Rendered::default(),
            render_key: None,
            link: None,
            selection: None,
        });
        self.view = View::Read;
        self.focus = Focus::Content;
        self.notice = None;
    }
    pub fn attach(&mut self) {
        if let Some(doc) = &self.document {
            let heading = doc
                .rendered
                .sections
                .iter()
                .rev()
                .find(|(line, _)| *line <= doc.scroll)
                .map(|(_, title)| title.clone())
                .filter(|t| t != &doc.object.metadata.title);
            self.reference = Some(Reference {
                id: doc.object.metadata.id.clone(),
                title: doc.object.metadata.title.clone(),
                heading,
                selection: doc.selection.map(|(a, b)| {
                    (
                        doc.object.revision.clone(),
                        doc.rendered.lines[a.min(b)..=a.max(b)]
                            .iter()
                            .map(|l| l.to_string())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                }),
            });
            self.focus = Focus::Input;
        }
    }
    pub fn message(&mut self, role: &'static str, body: impl Into<String>) {
        self.messages.push(Message {
            role,
            body: text::clean(&body.into()),
        });
    }
    pub fn agent(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::SessionActivated(view) => {
                if let Some(identity) = &self.session_identity {
                    if view.info.identity.store_id != identity.store_id
                        || view.activation < self.session_activation
                    {
                        return;
                    }
                }
                // Empty implicit sessions must not clear the just-submitted question or reader.
                if self.session_identity.is_none() && view.history.messages.is_empty() {
                    self.session_identity = Some(view.info.identity.clone());
                    self.session_activation = view.activation;
                    self.session_title = view.info.metadata.title.clone();
                } else if self.session_identity.as_ref() != Some(&view.info.identity)
                    || self.session_activation != view.activation
                {
                    let current = self.busy.then(|| self.last_submission.clone()).flatten();
                    self.activate_session(&view);
                    if let Some(input) = current {
                        self.message("你", input);
                    }
                }
            }
            AgentEvent::SessionEvent {
                identity,
                activation,
                event,
                ..
            } => {
                if self.session_identity.as_ref() == Some(&identity)
                    && self.session_activation == activation
                {
                    self.agent(*event);
                }
            }
            AgentEvent::MessageStart(AgentMessage::Assistant(_)) => self.streaming = None,
            AgentEvent::MessageDelta { delta } => {
                let i = if let Some(i) = self.streaming {
                    i
                } else {
                    self.message("Lumen", "");
                    let i = self.messages.len() - 1;
                    self.streaming = Some(i);
                    i
                };
                self.messages[i].body.push_str(&text::clean(&delta));
            }
            AgentEvent::MessageEnd(AgentMessage::Assistant(message)) => {
                let body = message
                    .content
                    .into_iter()
                    .map(|ContentPart::Text(t)| t)
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Some(i) = self.streaming.take() {
                    self.messages[i].body = text::clean(&body);
                } else if !body.is_empty() {
                    self.message("Lumen", &body);
                }
                if !body.is_empty() && self.view == View::Read {
                    self.unread += 1;
                }
            }
            AgentEvent::AgentStart | AgentEvent::TurnStart => self.activity = "正在思考…".into(),
            AgentEvent::ContextCompactionStart => self.activity = "正在压缩上下文…".into(),
            AgentEvent::ContextPrepared {
                input_tokens,
                input_budget,
                ..
            } => self.activity = format!("上下文约 {input_tokens}/{input_budget} tokens"),
            AgentEvent::ToolExecutionStart(call) => {
                self.activity = format!("正在执行 {}", text::clean(&call.name))
            }
            AgentEvent::ToolExecutionEnd(result) => {
                self.activity = if result.is_error {
                    "正在处理工具错误"
                } else {
                    "正在整理结果"
                }
                .into()
            }
            AgentEvent::AgentEnd {
                reason: StopReason::Aborted,
            } => self.notify("任务已停止，已收到的内容仍保留。", false),
            _ => (),
        }
    }
    pub fn candidates(&self) -> Vec<Candidate> {
        if self.focus != Focus::Input || self.completions_hidden {
            return vec![];
        }
        if let Some(q) = self.editor.token().1.strip_prefix('@') {
            let q = q.to_lowercase();
            return self
                .tree
                .objects
                .iter()
                .filter(|o| o.title.to_lowercase().contains(&q))
                .take(20)
                .map(|o| Candidate {
                    label: o.title.clone(),
                    hint: super::tree::code(o.kind).into(),
                    value: Completion::Object(o.id.clone()),
                })
                .collect();
        }
        let prefix = &self.editor.text[..self.editor.cursor];
        if prefix.starts_with('/') && !prefix.contains('\n') {
            return COMMANDS
                .iter()
                .filter(|(c, _)| c.starts_with(prefix))
                .map(|(c, h)| Candidate {
                    label: (*c).into(),
                    hint: (*h).into(),
                    value: Completion::Command((*c).into()),
                })
                .collect();
        }
        vec![]
    }
    fn complete(&mut self) -> Option<Action> {
        let choices = self.candidates();
        let choice = choices.get(self.candidate.min(choices.len().saturating_sub(1)))?;
        self.completions_hidden = true;
        match &choice.value {
            Completion::Command(c) => {
                self.editor.set(format!("{c} "));
                None
            }
            Completion::Object(id) => {
                self.editor.replace_token("");
                Some(Action::Attach(id.clone()))
            }
        }
    }
    pub fn scroll(&mut self, delta: isize) {
        if self.focus == Focus::Tree {
            self.tree.selected = self
                .tree
                .selected
                .saturating_add_signed(delta)
                .min(self.tree.rows().len().saturating_sub(1));
            return;
        }
        let max = self.content_lines.saturating_sub(self.viewport_height);
        if self.view == View::Chat {
            self.chat_scroll = self.chat_scroll.saturating_add_signed(delta).min(max);
            self.follow = self.chat_scroll == max;
        } else if let Some(doc) = &mut self.document {
            if let Some((_, cursor)) = &mut doc.selection {
                *cursor = cursor
                    .saturating_add_signed(delta)
                    .min(doc.rendered.lines.len().saturating_sub(1));
                if *cursor < doc.scroll {
                    doc.scroll = *cursor;
                }
                if *cursor >= doc.scroll + self.viewport_height {
                    doc.scroll = cursor.saturating_sub(self.viewport_height - 1);
                }
            } else {
                doc.scroll = doc.scroll.saturating_add_signed(delta).min(max);
            }
        }
    }
    fn find(&mut self, advance: bool) {
        if self.search_tree {
            self.tree.filter = self.search.text.clone();
            self.tree.selected = 0;
        } else {
            self.reading_query = self.search.text.to_lowercase();
            if self.reading_query.is_empty() {
                return;
            }
            if let Some(doc) = &mut self.document {
                let hits = doc
                    .rendered
                    .lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.to_string().to_lowercase().contains(&self.reading_query))
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>();
                if let Some(hit) = hits
                    .iter()
                    .copied()
                    .find(|i| *i >= doc.scroll + usize::from(advance))
                    .or(hits.first().copied())
                {
                    doc.scroll = hit;
                }
            }
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Option<Action> {
        if self.session_panel {
            let filtered: Vec<_> = self
                .session_choices
                .iter()
                .filter(|info| {
                    info.metadata
                        .title
                        .to_lowercase()
                        .contains(&self.session_filter.to_lowercase())
                        || info.identity.session_id.contains(&self.session_filter)
                })
                .collect();
            match key.code {
                KeyCode::PageDown => {
                    if let Some(cursor) = &self.session_next {
                        return Some(Action::Command(crate::commands::Command::SessionsNext {
                            query: self.session_query.clone(),
                            cursor: cursor.clone(),
                        }));
                    }
                }
                KeyCode::Esc => self.session_panel = false,
                KeyCode::Up => self.session_choice = self.session_choice.saturating_sub(1),
                KeyCode::Down => {
                    self.session_choice =
                        (self.session_choice + 1).min(filtered.len().saturating_sub(1))
                }
                KeyCode::Backspace => {
                    self.session_filter.pop();
                    self.session_choice = 0;
                }
                KeyCode::Char(ch) => {
                    self.session_filter.push(ch);
                    self.session_choice = 0;
                }
                KeyCode::Enter => {
                    if let Some(info) = filtered.get(self.session_choice) {
                        let command = if info.metadata.archived
                            || !matches!(
                                info.health,
                                crate::session::SessionHealth::Complete
                                    | crate::session::SessionHealth::Interrupted
                            ) {
                            "/session inspect"
                        } else {
                            "/resume"
                        };
                        let input = format!("{command} {}", info.identity.session_id);
                        self.session_panel = false;
                        return Some(Action::Submit(input));
                    }
                }
                _ => (),
            }
            return None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            match key.code {
                KeyCode::Char('q') => {
                    if !self.editor.text.is_empty() && !self.quit_armed {
                        self.quit_armed = true;
                        self.notify("仍有草稿，再按 Ctrl+Q 退出。", false);
                        return None;
                    }
                    return Some(Action::Quit);
                }
                KeyCode::Char('c') => {
                    return if self.busy {
                        Some(Action::Cancel)
                    } else {
                        self.editor.take();
                        None
                    }
                }
                KeyCode::Char('b') => {
                    self.sidebar = !self.sidebar;
                    if !self.sidebar && self.focus == Focus::Tree {
                        self.focus = Focus::Input;
                    }
                    return None;
                }
                KeyCode::Char('r') => {
                    self.reference = None;
                    return None;
                }
                _ => (),
            }
        }
        self.quit_armed = false;
        match key.code {
            KeyCode::F(1) => {
                self.sidebar = true;
                self.focus = Focus::Tree;
                return Some(Action::Reload);
            }
            KeyCode::F(2) => {
                self.view = View::Read;
                self.focus = Focus::Content;
                return None;
            }
            KeyCode::F(3) => {
                self.chat();
                self.focus = Focus::Content;
                return None;
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Input => Focus::Content,
                    Focus::Content if self.sidebar => Focus::Tree,
                    _ => Focus::Input,
                };
                self.completions_hidden = true;
                return None;
            }
            KeyCode::F(6) => {
                self.cycle();
                return None;
            }
            KeyCode::PageUp => {
                if self.view == View::Chat && self.chat_scroll == 0 && !self.busy {
                    if let Some(cursor) = &self.history_before {
                        return Some(Action::Command(crate::commands::Command::Session(
                            crate::commands::SessionCommand::History(Some(cursor.clone())),
                        )));
                    }
                }
                self.scroll(-(self.viewport_height as isize));
                return None;
            }
            KeyCode::PageDown => {
                self.scroll(self.viewport_height as isize);
                return None;
            }
            _ => (),
        }
        if self.focus == Focus::Search {
            match key.code {
                KeyCode::Esc => {
                    self.search.take();
                    self.find(false);
                    self.focus = if self.search_tree {
                        Focus::Tree
                    } else {
                        Focus::Content
                    };
                }
                KeyCode::Enter => {
                    self.focus = if self.search_tree {
                        Focus::Tree
                    } else {
                        Focus::Content
                    }
                }
                KeyCode::Backspace => {
                    self.search.backspace();
                    self.find(false);
                }
                KeyCode::Char(c) if !ctrl => {
                    self.search.insert(&c.to_string());
                    self.find(false);
                }
                _ => (),
            }
            return None;
        }
        if self.focus == Focus::Input {
            let choices = self.candidates().len();
            match key.code {
                KeyCode::Esc => {
                    self.focus = Focus::Content;
                    self.completions_hidden = true;
                }
                KeyCode::Tab if choices > 0 => return self.complete(),
                KeyCode::Tab => self.cycle(),
                KeyCode::Up if choices > 0 => self.candidate = self.candidate.saturating_sub(1),
                KeyCode::Down if choices > 0 => {
                    self.candidate = (self.candidate + 1).min(choices - 1)
                }
                KeyCode::Enter => return Some(Action::Submit(self.editor.text.clone())),
                KeyCode::Char('j') if ctrl => {
                    self.editor.insert("\n");
                    self.completions_hidden = true;
                }
                KeyCode::Char('a') if ctrl => self.editor.home(),
                KeyCode::Char('e') if ctrl => self.editor.end(),
                KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                    self.editor.insert(&c.to_string());
                    self.completions_hidden = false;
                    self.candidate = 0;
                }
                KeyCode::Backspace => {
                    self.editor.backspace();
                    self.completions_hidden = false;
                    self.candidate = 0;
                }
                KeyCode::Delete => self.editor.delete(),
                KeyCode::Left => self.editor.left(),
                KeyCode::Right => self.editor.right(),
                KeyCode::Home => self.editor.home(),
                KeyCode::End => self.editor.end(),
                KeyCode::Up => self.editor.vertical(false, self.input_width),
                KeyCode::Down => self.editor.vertical(true, self.input_width),
                _ => (),
            }
            return None;
        }
        match key.code {
            KeyCode::Char('i') => {
                self.focus = Focus::Input;
                self.completions_hidden = false;
            }
            KeyCode::Char('/') if self.focus == Focus::Tree || self.view == View::Read => {
                self.search_tree = self.focus == Focus::Tree;
                self.search.take();
                self.focus = Focus::Search;
            }
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll(1),
            KeyCode::Home => self.scroll(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.scroll(isize::MAX / 2),
            KeyCode::Left | KeyCode::Char('h') if self.focus == Focus::Tree => {
                self.tree.expand(false)
            }
            KeyCode::Right | KeyCode::Char('l') if self.focus == Focus::Tree => {
                self.tree.expand(true)
            }
            KeyCode::Enter if self.focus == Focus::Tree => {
                if let Some(row) = self.tree.rows().get(self.tree.selected) {
                    return Some(Action::Open(row.id.clone(), row.path.clone()));
                }
            }
            KeyCode::Esc if self.focus == Focus::Tree => {
                self.tree.filter.clear();
                self.tree.selected = 0;
            }
            KeyCode::Char('a') if self.focus == Focus::Content && self.view == View::Read => {
                self.attach()
            }
            KeyCode::Char('v') if self.focus == Focus::Content && self.view == View::Read => {
                if let Some(doc) = &mut self.document {
                    doc.selection = if doc.selection.is_some() || doc.rendered.lines.is_empty() {
                        None
                    } else {
                        Some((doc.scroll, doc.scroll))
                    };
                }
            }
            KeyCode::Char('n') if self.focus == Focus::Content && self.view == View::Read => {
                self.search_tree = false;
                self.find(true);
            }
            KeyCode::Tab if self.focus == Focus::Content && self.view == View::Read => {
                if let Some(doc) = &mut self.document {
                    if !doc.rendered.links.is_empty() {
                        let i = doc
                            .link
                            .map(|i| (i + 1) % doc.rendered.links.len())
                            .unwrap_or(0);
                        doc.link = Some(i);
                        doc.scroll = doc.rendered.links[i].line;
                    } else {
                        self.cycle();
                    }
                } else {
                    self.cycle();
                }
            }
            KeyCode::Tab
                if self.focus == Focus::Content
                    && self.view == View::Chat
                    && !self.chat_links.is_empty() =>
            {
                let i = self
                    .chat_link
                    .map(|i| (i + 1) % self.chat_links.len())
                    .unwrap_or(0);
                self.chat_link = Some(i);
                self.chat_scroll = self.chat_links[i].line;
                self.follow = false;
            }
            KeyCode::Tab => self.cycle(),
            KeyCode::Enter => {
                let link = if self.view == View::Chat {
                    self.chat_link.and_then(|i| self.chat_links.get(i))
                } else {
                    self.document
                        .as_ref()
                        .and_then(|d| d.link.and_then(|i| d.rendered.links.get(i)))
                };
                if let Some(link) = link {
                    return Some(Action::Link(link.target.clone()));
                }
            }
            KeyCode::Esc => {
                if let Some(doc) = &mut self.document {
                    doc.selection = None;
                }
                self.focus = Focus::Input;
            }
            KeyCode::Char('?') => {
                self.chat();
                self.message("帮助", crate::commands::help());
            }
            _ => (),
        }
        None
    }
}
