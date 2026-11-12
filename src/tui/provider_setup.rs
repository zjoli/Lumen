//! Provider setup is separate from chat input: secrets never enter drafts or history.
use super::{input::Editor, render, text, theme::Theme};
use crate::provider::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

pub(super) enum ProviderAction {
    Add {
        name: String,
        connection: Box<ProviderConnection>,
        key: Option<String>,
    },
    Key {
        provider: String,
        key: String,
    },
    Auth {
        provider: String,
        auth: AuthConfig,
        key: Option<String>,
    },
    Logout(String),
    Select(String),
    Refresh(String),
    Model(RegisterModel),
}
pub(super) enum DialogEvent {
    None,
    Close,
    Quit,
    Open(Box<ProviderDialog>),
    Save(Box<ProviderAction>),
}
pub(super) enum ProviderDialog {
    Form(Form),
    Models(ModelPicker),
    Providers(ProviderPicker),
    Available(AvailablePicker),
}
pub(super) struct Form {
    kind: FormKind,
    fields: Vec<Field>,
    selected: usize,
    error: Option<String>,
}
enum FormKind {
    Add,
    Key(String),
    Auth(String),
    Model {
        provider: String,
        model_id: String,
        connection_key: String,
    },
}
struct Field {
    label: &'static str,
    editor: Editor,
    secret: bool,
    choices: Vec<&'static str>,
}
impl Field {
    fn new(label: &'static str, value: impl Into<String>) -> Self {
        let mut editor = Editor::default();
        editor.set(value.into());
        Self {
            label,
            editor,
            secret: false,
            choices: vec![],
        }
    }
    fn secret(label: &'static str) -> Self {
        Self {
            secret: true,
            ..Self::new(label, "")
        }
    }
    fn choice(label: &'static str, value: &str, choices: Vec<&'static str>) -> Self {
        Self {
            choices,
            ..Self::new(label, value)
        }
    }
    fn toggle(label: &'static str, value: &str) -> Self {
        Self {
            choices: vec!["是", "否"],
            ..Self::new(label, value)
        }
    }
}
pub(super) struct ModelPicker {
    catalog: ProviderModelCatalog,
    aliases: std::collections::BTreeMap<String, String>,
    filter: Editor,
    selected: usize,
}
impl ProviderDialog {
    pub fn providers(entries: Vec<ProviderEntry>, logout: bool) -> Self {
        Self::Providers(ProviderPicker {
            entries,
            logout,
            filter: Editor::default(),
            selected: 0,
        })
    }
    pub fn login(entry: &ProviderEntry) -> Self {
        if !entry.configured {
            return Self::connection_form(entry.id.clone(), entry.connection.clone());
        }
        let (mode, env) = auth_fields(&entry.connection.auth);
        Self::Form(Form {
            kind: FormKind::Auth(entry.id.clone()),
            fields: vec![
                Field::choice("认证方式", mode, vec!["API Key", "环境变量", "无认证"]),
                Field::new("环境变量名（选择环境变量时必填）", env),
                Field::secret("API Key（隐藏显示）"),
            ],
            selected: if mode == "API Key" { 2 } else { 0 },
            error: None,
        })
    }
    pub fn available(models: Vec<AvailableModel>, providers: Vec<String>) -> Self {
        Self::Available(AvailablePicker {
            models,
            providers,
            filter: Editor::default(),
            selected: 0,
        })
    }
    pub fn add(name: Option<String>, base_url: Option<String>) -> Self {
        let preset = name.as_deref().and_then(provider_preset);
        let mut connection = preset
            .map(|p| p.connection())
            .unwrap_or_else(|| compatible_connection(String::new(), AuthConfig::None));
        if let Some(url) = base_url {
            connection.base_url = url;
        }
        Self::connection_form(name.unwrap_or_default(), connection)
    }
    fn connection_form(name: String, connection: ProviderConnection) -> Self {
        let protocol = match connection.protocol {
            ProtocolKind::OpenaiResponses => "openai_responses",
            ProtocolKind::AnthropicMessages => "anthropic_messages",
            _ => "openai_chat",
        };
        let (mode, env) = auth_fields(&connection.auth);
        let preset = !name.is_empty() && !connection.base_url.is_empty();
        Self::Form(Form {
            kind: FormKind::Add,
            fields: vec![
                Field::new("供应商 ID", name),
                Field::new("Base URL（保留 /v1 等前缀）", connection.base_url),
                Field::choice(
                    "API 协议",
                    protocol,
                    vec!["openai_chat", "openai_responses", "anthropic_messages"],
                ),
                Field::choice("认证方式", mode, vec!["API Key", "环境变量", "无认证"]),
                Field::new("环境变量名（选择环境变量时必填）", env),
                Field::secret("API Key（隐藏显示）"),
            ],
            selected: if preset && mode == "API Key" { 5 } else { 0 },
            error: None,
        })
    }
    pub fn key(provider: String) -> Self {
        Self::Form(Form {
            kind: FormKind::Key(provider),
            fields: vec![Field::secret("新的 API Key（隐藏显示）")],
            selected: 0,
            error: None,
        })
    }
    pub fn models(catalog: ProviderModelCatalog, registered: Vec<AvailableModel>) -> Self {
        let aliases = registered
            .into_iter()
            .filter(|m| {
                m.provider == catalog.provider && m.connection_key == catalog.connection_key
            })
            .filter_map(|m| m.alias.map(|alias| (m.model.id, alias)))
            .collect();
        Self::Models(ModelPicker {
            catalog,
            aliases,
            filter: Editor::default(),
            selected: 0,
        })
    }
    fn model(catalog: &ProviderModelCatalog, model: &DiscoveredModel) -> Self {
        Self::Form(Form {
            kind: FormKind::Model {
                provider: catalog.provider.clone(),
                model_id: model.id.clone(),
                connection_key: catalog.connection_key.clone(),
            },
            fields: vec![
                Field::new("本机模型别名", format!("{}:{}", catalog.provider, model.id)),
                Field::new(
                    "上下文容量（tokens）",
                    model
                        .context_tokens
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                ),
                Field::new(
                    "本次使用的输出上限（tokens）",
                    model
                        .max_output_tokens
                        .map(|n| {
                            n.min(4096)
                                .min(model.context_tokens.unwrap_or(usize::MAX) / 8)
                                .max(1)
                                .to_string()
                        })
                        .unwrap_or_default(),
                ),
                Field::toggle("支持工具调用（请按模型实际能力确认）", "是"),
                Field::toggle("设为新会话默认模型", "否"),
            ],
            selected: if model.context_tokens.is_some() { 0 } else { 1 },
            error: None,
        })
    }
    pub fn paste(&mut self, value: &str) {
        match self {
            Self::Form(form) => {
                let field = &mut form.fields[form.selected];
                if field.choices.is_empty() {
                    field.editor.insert(value.trim_end_matches(['\r', '\n']));
                    form.error = None;
                }
            }
            Self::Models(picker) => {
                picker.filter.insert(value);
                picker.selected = 0;
            }
            Self::Providers(picker) => {
                picker.filter.insert(value);
                picker.selected = 0;
            }
            Self::Available(picker) => {
                picker.filter.insert(value);
                picker.selected = 0;
            }
        }
    }
    pub fn key_event(&mut self, key: KeyEvent) -> DialogEvent {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            return DialogEvent::Quit;
        }
        if key.code == KeyCode::Esc
            || key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
        {
            return DialogEvent::Close;
        }
        match self {
            Self::Providers(picker) => {
                let filtered = picker.filtered();
                let count = filtered.len() + usize::from(!picker.logout);
                if key.code == KeyCode::Enter {
                    if let Some(entry) = filtered.get(picker.selected) {
                        return if picker.logout {
                            DialogEvent::Save(Box::new(ProviderAction::Logout(entry.id.clone())))
                        } else {
                            DialogEvent::Open(Box::new(Self::login(entry)))
                        };
                    }
                    if !picker.logout {
                        return DialogEvent::Open(Box::new(Self::add(None, None)));
                    }
                }
                picker_key(&mut picker.filter, &mut picker.selected, count, key);
            }
            Self::Available(picker) => {
                let filtered = picker.filtered();
                let providers = picker.filtered_providers();
                let count = filtered.len() + providers.len();
                if key.code == KeyCode::Enter {
                    if let Some(entry) = filtered.get(picker.selected) {
                        if let Some(alias) = &entry.alias {
                            return DialogEvent::Save(Box::new(ProviderAction::Select(
                                alias.clone(),
                            )));
                        }
                        let catalog = ProviderModelCatalog {
                            provider: entry.provider.clone(),
                            connection_key: entry.connection_key.clone(),
                            models: vec![],
                        };
                        return DialogEvent::Open(Box::new(Self::model(&catalog, &entry.model)));
                    }
                    if let Some(provider) =
                        providers.get(picker.selected.saturating_sub(filtered.len()))
                    {
                        return DialogEvent::Save(Box::new(ProviderAction::Refresh(
                            (*provider).clone(),
                        )));
                    }
                }
                picker_key(&mut picker.filter, &mut picker.selected, count, key);
            }

            Self::Models(picker) => {
                let filtered = picker.filtered();
                match key.code {
                    KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
                    KeyCode::Down => {
                        picker.selected =
                            (picker.selected + 1).min(filtered.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some(model) = filtered.get(picker.selected) {
                            if let Some(alias) = picker.aliases.get(&model.id) {
                                return DialogEvent::Save(Box::new(ProviderAction::Select(
                                    alias.clone(),
                                )));
                            }
                            return DialogEvent::Open(Box::new(Self::model(
                                &picker.catalog,
                                model,
                            )));
                        }
                    }
                    KeyCode::Backspace => {
                        picker.filter.backspace();
                        picker.selected = 0;
                    }
                    KeyCode::Char(ch)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        picker.filter.insert(&ch.to_string());
                        picker.selected = 0;
                    }
                    _ => (),
                }
            }
            Self::Form(form) => {
                let save = key.code == KeyCode::Enter
                    && (form.selected == form.fields.len() - 1
                        || key.modifiers.contains(KeyModifiers::CONTROL));
                if save {
                    return match form.request() {
                        Ok(action) => DialogEvent::Save(Box::new(action)),
                        Err(error) => {
                            form.error = Some(error);
                            DialogEvent::None
                        }
                    };
                }
                match key.code {
                    KeyCode::Tab | KeyCode::Down | KeyCode::Enter => {
                        form.selected = (form.selected + 1) % form.fields.len()
                    }
                    KeyCode::BackTab | KeyCode::Up => {
                        form.selected = if form.selected == 0 {
                            form.fields.len() - 1
                        } else {
                            form.selected - 1
                        }
                    }
                    _ => {
                        let field = &mut form.fields[form.selected];
                        if !field.choices.is_empty() {
                            if matches!(
                                key.code,
                                KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                            ) {
                                let at = field
                                    .choices
                                    .iter()
                                    .position(|s| *s == field.editor.text)
                                    .unwrap_or(0);
                                let next = if key.code == KeyCode::Left {
                                    (at + field.choices.len() - 1) % field.choices.len()
                                } else {
                                    (at + 1) % field.choices.len()
                                };
                                field.editor.set(field.choices[next].into());
                            }
                        } else {
                            edit_key(&mut field.editor, key);
                        }
                        form.error = None;
                    }
                }
            }
        }
        DialogEvent::None
    }
}
impl ModelPicker {
    fn filtered(&self) -> Vec<&DiscoveredModel> {
        let q = self.filter.text.to_lowercase();
        self.catalog
            .models
            .iter()
            .filter(|m| {
                m.id.to_lowercase().contains(&q)
                    || m.name
                        .as_ref()
                        .is_some_and(|n| n.to_lowercase().contains(&q))
            })
            .collect()
    }
}
impl Form {
    fn request(&self) -> Result<ProviderAction, String> {
        let value = |i: usize| self.fields[i].editor.text.trim().to_string();
        match &self.kind {
            FormKind::Add => {
                let name = value(0);
                let protocol = match value(2).as_str() {
                    "openai_responses" => ProtocolKind::OpenaiResponses,
                    "anthropic_messages" => ProtocolKind::AnthropicMessages,
                    _ => ProtocolKind::OpenaiChat,
                };
                let (auth, key) = form_auth(&value(3), value(4), value(5))?;
                let connection = protocol_connection(protocol, value(1), auth);
                let cfg = ProviderConfig {
                    schema_version: 1,
                    default_model: String::new(),
                    providers: std::collections::BTreeMap::from([(
                        name.clone(),
                        connection.clone(),
                    )]),
                    models: Default::default(),
                };
                cfg.validate().map_err(|e| e.to_string())?;
                Ok(ProviderAction::Add {
                    name,
                    connection: Box::new(connection),
                    key,
                })
            }
            FormKind::Auth(provider) => {
                let (auth, key) = form_auth(&value(0), value(1), value(2))?;
                Ok(ProviderAction::Auth {
                    provider: provider.clone(),
                    auth,
                    key,
                })
            }
            FormKind::Key(provider) => {
                let key = value(0);
                if key.is_empty() {
                    return Err("请填写 API Key".into());
                }
                Ok(ProviderAction::Key {
                    provider: provider.clone(),
                    key,
                })
            }
            FormKind::Model {
                provider,
                model_id,
                connection_key,
            } => {
                let context_tokens = value(1)
                    .parse::<usize>()
                    .map_err(|_| "请填写模型的上下文容量".to_string())?;
                let output_tokens = value(2)
                    .parse::<usize>()
                    .map_err(|_| "请填写输出上限".to_string())?;
                if output_tokens == 0 || output_tokens >= context_tokens {
                    return Err("输出上限必须大于 0 且小于上下文容量".into());
                }
                Ok(ProviderAction::Model(RegisterModel {
                    provider: provider.clone(),
                    model_id: model_id.clone(),
                    alias: value(0),
                    context_tokens,
                    output_tokens,
                    tools: value(3) == "是",
                    make_default: value(4) == "是",
                    connection_key: Some(connection_key.clone()),
                }))
            }
        }
    }
}
fn edit_key(editor: &mut Editor, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('u') => {
                editor.take();
            }
            KeyCode::Char('a') => editor.home(),
            KeyCode::Char('e') => editor.end(),
            _ => (),
        }
        return;
    }
    match key.code {
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            editor.insert(&c.to_string())
        }
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Delete => editor.delete(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        _ => (),
    }
}
pub(super) fn draw(frame: &mut Frame, dialog: &ProviderDialog, theme: Theme, area: Rect) {
    let width = area.width.saturating_sub(2).min(82);
    let height = area.height.saturating_sub(2).min(20);
    if width < 12 || height < 5 {
        return;
    }
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    let title = match dialog {
        ProviderDialog::Form(form) => match &form.kind {
            FormKind::Add => "添加模型供应商".into(),
            FormKind::Key(p) => format!("修改 {p} 的 API Key"),
            FormKind::Auth(p) => format!("认证 · {p}"),
            FormKind::Model { model_id, .. } => format!("配置模型 {model_id}"),
        },
        ProviderDialog::Models(p) => format!("{} · 选择可用模型", p.catalog.provider),
        ProviderDialog::Providers(p) => if p.logout {
            "退出供应商认证"
        } else {
            "选择供应商 · /login"
        }
        .into(),
        ProviderDialog::Available(_) => "选择模型 · /model".into(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(render::span(title, theme.accent())))
        .style(theme.base())
        .border_style(theme.border(true));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let row = |offset: u16| {
        Rect::new(
            inner.x + 1,
            inner.y + offset,
            inner.width.saturating_sub(2),
            1,
        )
    };
    match dialog {
        ProviderDialog::Providers(picker) => {
            let mut labels: Vec<String> = picker
                .filtered()
                .iter()
                .map(|p| {
                    format!(
                        "{} · {} · {}",
                        p.label,
                        p.id,
                        if p.configured && p.authenticated {
                            "已配置 · 认证可用"
                        } else if p.configured {
                            "已配置 · 缺少凭据"
                        } else if p.authenticated {
                            "环境变量/免认证"
                        } else {
                            "未配置"
                        }
                    )
                })
                .collect();
            if !picker.logout {
                labels.push("+ 添加自定义供应商（选择 API 协议）".into());
            }
            draw_picker(
                frame,
                inner,
                &picker.filter,
                picker.selected,
                labels,
                theme,
                "输入筛选 · ↑↓ 选择 · Enter 继续 · Esc 取消",
            );
        }
        ProviderDialog::Available(picker) => {
            let mut labels: Vec<String> = picker
                .filtered()
                .iter()
                .map(|m| {
                    format!(
                        "{} / {}{}",
                        m.provider,
                        m.model.id,
                        m.alias
                            .as_ref()
                            .map(|a| format!(" · {a}"))
                            .unwrap_or_else(|| " · 待配置".into())
                    )
                })
                .collect();
            labels.extend(
                picker
                    .filtered_providers()
                    .iter()
                    .map(|p| format!("↻ {p} · 查询/刷新远端模型")),
            );
            draw_picker(
                frame,
                inner,
                &picker.filter,
                picker.selected,
                labels,
                theme,
                "输入筛选 · ↑↓ 选择 · Enter 确认 · Esc 返回",
            );
        }

        ProviderDialog::Form(form) => {
            let visible = inner.height.saturating_sub(4) as usize / 2;
            let start = form.selected.saturating_sub(visible.saturating_sub(1));
            for (i, field) in form.fields.iter().enumerate().skip(start).take(visible) {
                let y = ((i - start) * 2) as u16;
                frame.render_widget(
                    Paragraph::new(render::span(
                        format!(
                            "{} {}",
                            if i == form.selected { "›" } else { " " },
                            field.label
                        ),
                        if i == form.selected {
                            theme.accent()
                        } else {
                            theme.muted()
                        },
                    )),
                    row(y),
                );
                let display = if field.secret {
                    "*".repeat(field.editor.text.chars().count())
                } else {
                    field.editor.text.clone()
                };
                let mut editor = Editor::default();
                editor.set(display);
                editor.cursor = if field.secret {
                    field.editor.text[..field.editor.cursor].chars().count()
                } else {
                    field.editor.cursor
                };
                let (lines, (x, at)) =
                    editor.layout(row(y + 1).width.saturating_sub(1).max(1) as usize);
                frame.render_widget(
                    Paragraph::new(render::span(
                        lines.get(at).cloned().unwrap_or_default(),
                        theme.base(),
                    )),
                    row(y + 1),
                );
                if i == form.selected {
                    frame.set_cursor_position((row(y + 1).x + x as u16, row(y + 1).y));
                }
            }
            let note = if matches!(form.kind, FormKind::Model { .. }) {
                "接口未提供的容量需手工填写；请确认工具能力。"
            } else {
                "Key 仅保存在本机，不写入聊天或会话草稿。"
            };
            frame.render_widget(
                Paragraph::new(render::span(note, theme.muted())),
                row(inner.height - 3),
            );
            frame.render_widget(
                Paragraph::new(render::span(
                    form.error
                        .as_deref()
                        .unwrap_or("Tab/↑↓ 切换 · ←→/空格 选择 · Ctrl+Enter 保存"),
                    theme.accent(),
                )),
                row(inner.height - 2),
            );
            frame.render_widget(
                Paragraph::new(render::span(
                    "最后一项 Enter 保存 · Esc 取消",
                    theme.muted(),
                )),
                row(inner.height - 1),
            );
        }
        ProviderDialog::Models(picker) => {
            let cursor =
                text::width("筛选：") + text::width(&picker.filter.text[..picker.filter.cursor]);
            frame.set_cursor_position((
                row(0).x + (cursor as u16).min(row(0).width.saturating_sub(1)),
                row(0).y,
            ));
            frame.render_widget(
                Paragraph::new(render::span(
                    format!("筛选：{}", picker.filter.text),
                    theme.accent(),
                )),
                row(0),
            );
            let choices = picker.filtered();
            let visible = inner.height.saturating_sub(3) as usize;
            let start = picker.selected.saturating_sub(visible.saturating_sub(1));
            for (i, model) in choices.iter().enumerate().skip(start).take(visible) {
                let label = format!(
                    "{} {}{}",
                    if i == picker.selected { "›" } else { " " },
                    model.id,
                    model
                        .name
                        .as_ref()
                        .map(|s| format!(" · {s}"))
                        .unwrap_or_default()
                );
                frame.render_widget(
                    Paragraph::new(render::span(
                        text::truncate(&label, row(0).width as usize),
                        if i == picker.selected {
                            theme.selected()
                        } else {
                            theme.base()
                        },
                    )),
                    row((i - start + 1) as u16),
                );
            }
            if choices.is_empty() {
                frame.render_widget(
                    Paragraph::new(render::span(
                        "没有匹配的模型；Esc 返回，可手工添加。",
                        theme.muted(),
                    )),
                    row(1),
                );
            }
            frame.render_widget(
                Paragraph::new(render::span(
                    "输入筛选 · ↑↓ 选择 · Enter 配置模型 · Esc 返回",
                    theme.muted(),
                )),
                row(inner.height - 1),
            );
        }
    }
}

pub(super) struct ProviderPicker {
    entries: Vec<ProviderEntry>,
    logout: bool,
    filter: Editor,
    selected: usize,
}
impl ProviderPicker {
    fn filtered(&self) -> Vec<&ProviderEntry> {
        let q = self.filter.text.to_lowercase();
        self.entries
            .iter()
            .filter(|p| {
                (!self.logout || p.configured)
                    && format!("{} {}", p.id, p.label).to_lowercase().contains(&q)
            })
            .collect()
    }
}
pub(super) struct AvailablePicker {
    models: Vec<AvailableModel>,
    providers: Vec<String>,
    filter: Editor,
    selected: usize,
}
impl AvailablePicker {
    fn filtered_providers(&self) -> Vec<&String> {
        let q = self.filter.text.to_lowercase();
        self.providers
            .iter()
            .filter(|p| p.to_lowercase().contains(&q))
            .collect()
    }
    fn filtered(&self) -> Vec<&AvailableModel> {
        let q = self.filter.text.to_lowercase();
        self.models
            .iter()
            .filter(|m| {
                format!(
                    "{} {} {}",
                    m.provider,
                    m.model.id,
                    m.alias.as_deref().unwrap_or("")
                )
                .to_lowercase()
                .contains(&q)
            })
            .collect()
    }
}
fn auth_fields(auth: &AuthConfig) -> (&'static str, String) {
    match auth {
        AuthConfig::Env { variable } => (
            if std::env::var(variable).is_ok_and(|v| !v.trim().is_empty()) {
                "环境变量"
            } else {
                "API Key"
            },
            variable.clone(),
        ),
        AuthConfig::Local { .. } => ("API Key", String::new()),
        AuthConfig::None => ("无认证", String::new()),
    }
}
fn form_auth(
    mode: &str,
    variable: String,
    key: String,
) -> Result<(AuthConfig, Option<String>), String> {
    match mode {
        "API Key" if !key.is_empty() => Ok((AuthConfig::None, Some(key))),
        "API Key" => Err("请填写 API Key，或切换为环境变量认证".into()),
        "环境变量" if !variable.is_empty() => Ok((AuthConfig::Env { variable }, None)),
        "环境变量" => Err("请填写环境变量名".into()),
        _ => Ok((AuthConfig::None, None)),
    }
}
fn picker_key(filter: &mut Editor, selected: &mut usize, count: usize, key: KeyEvent) {
    match key.code {
        KeyCode::Up => *selected = selected.saturating_sub(1),
        KeyCode::Down => *selected = (*selected + 1).min(count.saturating_sub(1)),
        KeyCode::Enter => (),
        _ => {
            edit_key(filter, key);
            *selected = 0;
        }
    }
}
fn draw_picker(
    frame: &mut Frame,
    inner: Rect,
    filter: &Editor,
    selected: usize,
    labels: Vec<String>,
    theme: Theme,
    hint: &str,
) {
    let row = |y| Rect::new(inner.x + 1, inner.y + y, inner.width.saturating_sub(2), 1);
    frame.render_widget(
        Paragraph::new(render::span(
            format!("筛选：{}", filter.text),
            theme.accent(),
        )),
        row(0),
    );
    let cursor = text::width("筛选：") + text::width(&filter.text[..filter.cursor]);
    frame.set_cursor_position((
        row(0).x + (cursor as u16).min(row(0).width.saturating_sub(1)),
        row(0).y,
    ));
    let visible = inner.height.saturating_sub(3) as usize;
    let start = selected.saturating_sub(visible.saturating_sub(1));
    for (i, label) in labels.iter().enumerate().skip(start).take(visible) {
        frame.render_widget(
            Paragraph::new(render::span(
                text::truncate(
                    &format!("{} {label}", if selected == i { "›" } else { " " }),
                    row(0).width as usize,
                ),
                if selected == i {
                    theme.selected()
                } else {
                    theme.base()
                },
            )),
            row((i - start + 1) as u16),
        );
    }
    if labels.is_empty() {
        frame.render_widget(
            Paragraph::new(render::span("没有匹配项", theme.muted())),
            row(1),
        );
    }
    frame.render_widget(
        Paragraph::new(render::span(hint, theme.muted())),
        row(inner.height - 1),
    );
}
