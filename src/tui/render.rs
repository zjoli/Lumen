use super::{
    markdown,
    state::{Focus, Ui, View},
    text,
    theme::Theme,
    tree,
};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph},
    Frame,
};

pub fn span(value: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text::clean(&value.into()), style)
}
fn panel(theme: Theme, title: String, focus: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.border(focus))
        .title(Line::from(span(
            format!(" {title} "),
            if focus {
                theme.accent().bold()
            } else {
                theme.base()
            },
        )))
        .padding(Padding::horizontal(1))
        .style(theme.base())
}
#[derive(Clone, Copy)]
pub struct Geometry {
    pub tree: Option<Rect>,
    pub content: Rect,
    pub input: Rect,
    pub narrow_tree: bool,
}
pub fn geometry(area: Rect, ui: &Ui) -> Geometry {
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(2),
    );
    let tree_focus = ui.focus == Focus::Tree || (ui.focus == Focus::Search && ui.search_tree);
    let visible = ui.sidebar && (area.width >= 88 || tree_focus && area.width >= 64);
    let tree_width = if visible {
        if area.width >= 100 {
            28
        } else {
            24
        }
    } else {
        0
    };
    let gap = u16::from(visible);
    let right = Rect::new(
        body.x + tree_width + gap,
        body.y,
        body.width.saturating_sub(tree_width + gap),
        body.height,
    );
    let (rows, _) = ui
        .editor
        .layout(right.width.saturating_sub(6).max(1) as usize);
    let wanted = if area.height >= 24 { 7 } else { 5 };
    let input_height =
        (wanted + u16::from(ui.reference.is_some()) + rows.len().saturating_sub(2).min(3) as u16)
            .min(body.height.saturating_sub(4))
            .max(3)
            .min(body.height);
    Geometry {
        tree: visible.then_some(Rect::new(body.x, body.y, tree_width, body.height)),
        content: Rect::new(
            right.x,
            right.y,
            right.width,
            right.height.saturating_sub(input_height),
        ),
        input: Rect::new(
            right.x,
            right.bottom() - input_height,
            right.width,
            input_height,
        ),
        narrow_tree: !visible && tree_focus,
    }
}
pub fn draw(frame: &mut Frame, ui: &mut Ui) {
    let area = frame.area();
    let theme = ui.theme;
    frame.render_widget(Block::default().style(theme.base()), area);
    if area.width < 24 || area.height < 9 {
        frame.render_widget(
            Paragraph::new("请扩大终端\nCtrl+Q 退出").style(theme.base()),
            area,
        );
        return;
    }
    let g = geometry(area, ui);
    let header = Line::from(vec![
        span(" Lumen ", theme.accent().bold()),
        span(
            format!(
                "· {}",
                text::truncate(
                    &text::clean(&ui.workspace_name),
                    area.width.saturating_sub(32) as usize
                )
            ),
            theme.base(),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(header),
        Rect::new(area.x, area.y, area.width, 1),
    );
    if area.width >= 72 {
        let status = if ui.demo { "演示模式" } else { &ui.model };
        frame.render_widget(
            Paragraph::new(
                Line::from(span(text::truncate(status, 24), theme.muted())).right_aligned(),
            ),
            Rect::new(area.right() - 26, area.y, 24, 1),
        );
    }
    if let Some(tree) = g.tree {
        directory(frame, ui, tree);
    }
    if g.narrow_tree {
        directory(frame, ui, g.content);
    } else {
        content(frame, ui, g.content);
    }
    composer(frame, ui, g.input);
    completions(frame, ui, g.content);
    let focus = match ui.focus {
        Focus::Tree => "目录",
        Focus::Content => "内容",
        Focus::Input => "输入",
        Focus::Search => "查找",
    };
    let status = if let Some(notice) = &ui.notice {
        notice.clone()
    } else if ui.view == View::Read && ui.unread > 0 {
        format!("有 {} 条新回复 · /chat 查看", ui.unread)
    } else {
        format!("焦点:{focus}  F6 切换  F1 目录  F2 阅读  F3 对话  Ctrl+Q 退出")
    };
    frame.render_widget(
        Paragraph::new(Line::from(span(
            text::truncate(&status, area.width.saturating_sub(2) as usize),
            if ui.error && ui.notice.is_some() {
                theme.error()
            } else {
                theme.muted()
            },
        ))),
        Rect::new(
            area.x + 1,
            area.bottom() - 1,
            area.width.saturating_sub(2),
            1,
        ),
    );
}
fn directory(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let theme = ui.theme;
    let block = panel(
        theme,
        "知识目录".into(),
        ui.focus == Focus::Tree || ui.focus == Focus::Search && ui.search_tree,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let rows = ui.tree.rows();
    if rows.is_empty() {
        let value = if ui.loading {
            "正在读取…"
        } else if ui.workspace_path.is_none() {
            "尚未打开工作区\n\n在右下输入\n/workspace open <路径>\n或 init 创建工作区"
        } else if ui.tree.objects.is_empty() {
            "工作区暂无知识\n\n导入 Markdown 后\n按 F1 刷新目录"
        } else {
            "没有匹配项\n按 / 重新筛选\nEsc 清除筛选"
        };
        frame.render_widget(Paragraph::new(value).style(theme.base()), inner);
        return;
    }
    ui.tree.selected = ui.tree.selected.min(rows.len() - 1);
    let height = inner.height as usize;
    let offset = ui.tree.selected.saturating_sub(height - 1);
    let lines = rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(i, row)| {
            let object = ui.tree.get(&row.id).unwrap();
            let depth = row.depth.min(inner.width.saturating_sub(10) as usize / 2);
            let prefix = format!(
                "{}{} ",
                "  ".repeat(depth),
                if row.branch {
                    if row.open {
                        "▾"
                    } else {
                        "▸"
                    }
                } else {
                    "·"
                }
            );
            let title = text::truncate(
                &text::clean(&object.title),
                inner.width.saturating_sub(text::width(&prefix) as u16 + 4) as usize,
            );
            let style = if i == ui.tree.selected {
                theme.selected()
            } else {
                theme.base()
            };
            Line::from(vec![
                span(prefix, style),
                span(format!("[{}] ", tree::code(object.kind)), style),
                span(title, style),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
}
fn content(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let theme = ui.theme;
    let title = match ui.view {
        View::Chat => "对话 · 当前会话".into(),
        View::Read => format!(
            "阅读 · {}",
            ui.document
                .as_ref()
                .map(|d| text::clean(&d.object.metadata.title))
                .unwrap_or_else(|| "尚未打开文档".into())
        ),
    };
    let mut block = panel(
        theme,
        text::truncate(&title, area.width.saturating_sub(6) as usize),
        ui.focus == Focus::Content || ui.focus == Focus::Search && !ui.search_tree,
    );
    if ui.view == View::Read && ui.unread > 0 && area.width >= 52 {
        block = block.title_bottom(
            Line::from(span(
                format!(" 新回复 {} · /chat ", ui.unread),
                theme.accent(),
            ))
            .right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    ui.viewport_height = inner.height.max(1) as usize;
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let lines = if ui.view == View::Read {
        reading(ui, inner.width as usize)
    } else {
        conversation(ui, inner.width as usize)
    };
    ui.content_lines = lines.len();
    let max = lines.len().saturating_sub(inner.height as usize);
    let offset = if ui.view == View::Read {
        if let Some(doc) = &mut ui.document {
            doc.scroll = doc.scroll.min(max);
            doc.scroll
        } else {
            0
        }
    } else {
        ui.chat_scroll = if ui.follow {
            max
        } else {
            ui.chat_scroll.min(max)
        };
        ui.chat_scroll
    };
    let visible = lines
        .into_iter()
        .skip(offset)
        .take(inner.height as usize)
        .enumerate()
        .map(|(i, mut line)| {
            if ui.view == View::Read {
                if !ui.reading_query.is_empty()
                    && line.to_string().to_lowercase().contains(&ui.reading_query)
                {
                    line.style = line.style.add_modifier(Modifier::REVERSED);
                }
                if ui
                    .document
                    .as_ref()
                    .and_then(|d| d.selection)
                    .is_some_and(|(a, b)| (a.min(b)..=a.max(b)).contains(&(i + offset)))
                {
                    line.style = line.style.add_modifier(Modifier::REVERSED);
                }
            }
            line
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(visible), inner);
    let selected = if ui.view == View::Read {
        ui.document
            .as_ref()
            .and_then(|d| d.link.and_then(|i| d.rendered.links.get(i)))
    } else {
        ui.chat_link.and_then(|i| ui.chat_links.get(i))
    };
    if let Some(link) = selected {
        frame.render_widget(
            Paragraph::new(Line::from(span(
                text::truncate(
                    &format!("链接: {} · Enter 打开", link.label),
                    inner.width as usize,
                ),
                theme.selected(),
            ))),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}
fn reading(ui: &mut Ui, width: usize) -> Vec<Line<'static>> {
    let Some(doc) = &mut ui.document else {
        return markdown::render("在左侧选择知识并按 Enter 打开。\n\n也可以输入 `/open <标题或 ID>`。\n\n`/chat` 显示会话，`/read` 返回文档。",width,ui.theme).lines;
    };
    let key = (width, ui.theme);
    if doc.render_key != Some(key) {
        let fraction = if doc.rendered.lines.is_empty() {
            0.0
        } else {
            doc.scroll as f64 / doc.rendered.lines.len() as f64
        };
        let mut rendered = markdown::render(&doc.object.content, width, ui.theme);
        if !doc.object.content.trim_start().starts_with("# ") {
            let mut heading = text::wrap(
                Line::from(span(&doc.object.metadata.title, ui.theme.base().bold())),
                width,
            );
            heading.push(Line::default());
            let n = heading.len();
            for link in &mut rendered.links {
                link.line += n;
            }
            for section in &mut rendered.sections {
                section.0 += n;
            }
            heading.append(&mut rendered.lines);
            rendered.lines = heading;
        }
        if doc.render_key.is_some() {
            doc.scroll = (fraction * rendered.lines.len() as f64) as usize;
        }
        doc.render_key = Some(key);
        doc.rendered = rendered;
        doc.selection = None;
    }
    doc.rendered.lines.clone()
}
fn conversation(ui: &mut Ui, width: usize) -> Vec<Line<'static>> {
    ui.chat_links.clear();
    if ui.messages.is_empty() {
        ui.chat_link = None;
        return markdown::render("欢迎使用 **Lumen**\n\n在下方输入问题，开始学习。\n\n- `/workspace init <路径>` 创建知识库\n- `/workspace open <路径>` 打开知识库\n- `/help` 查看命令与快捷键\n\n左侧浏览知识，右上阅读或查看对话，右下输入。",width,ui.theme).lines;
    }
    let mut lines = vec![];
    for message in &ui.messages {
        lines.push(Line::from(span(
            if message.role == "你" {
                "› 你"
            } else {
                message.role
            },
            if message.role == "错误" {
                ui.theme.error().bold()
            } else {
                ui.theme.accent().bold()
            },
        )));
        if message.role == "Lumen" {
            let rendered = markdown::render(&message.body, width, ui.theme);
            ui.chat_links
                .extend(rendered.links.into_iter().map(|mut link| {
                    link.line += lines.len();
                    link
                }));
            lines.extend(rendered.lines);
        } else {
            for value in message.body.lines() {
                lines.extend(text::wrap(
                    Line::from(span(
                        value,
                        if message.role == "错误" {
                            ui.theme.error()
                        } else {
                            ui.theme.base()
                        },
                    )),
                    width,
                ));
            }
        }
        lines.push(Line::default());
    }
    lines
}
fn composer(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let theme = ui.theme;
    let search = ui.focus == Focus::Search;
    let title = if search {
        "查找"
    } else if ui.focus == Focus::Input {
        "Agent · 输入"
    } else {
        "Agent · i 输入"
    };
    let mut block = panel(theme, title.into(), search || ui.focus == Focus::Input);
    if ui.busy && area.width >= 44 {
        block = block.title_top(
            Line::from(span(
                format!(
                    " {} ",
                    text::truncate(&ui.activity, area.width.saturating_sub(24) as usize)
                ),
                theme.accent(),
            ))
            .right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 3 || inner.height == 0 {
        return;
    }
    let mut y = inner.y;
    if let Some(reference) = ui
        .reference
        .as_ref()
        .filter(|_| !search && inner.height >= 3)
    {
        let value = format!(
            "引用: {}{}  × Ctrl+R",
            reference.title,
            reference
                .heading
                .as_ref()
                .map(|h| format!(" / {h}"))
                .unwrap_or_default()
        );
        frame.render_widget(
            Paragraph::new(Line::from(span(
                text::truncate(&text::clean(&value), inner.width as usize),
                theme.accent(),
            ))),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
    }
    let input_height = inner
        .bottom()
        .saturating_sub(y + u16::from(inner.height >= 2));
    let width = inner.width.saturating_sub(2).max(1) as usize;
    ui.input_width = width;
    let editor = if search { &ui.search } else { &ui.editor };
    let (rows, (x, row)) = editor.layout(width);
    let offset = row.saturating_sub(input_height.saturating_sub(1) as usize);
    let mut lines = vec![];
    for (i, value) in rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(input_height as usize)
    {
        let placeholder = editor.text.is_empty();
        let value = if placeholder {
            if search {
                "输入查找内容…"
            } else {
                "输入问题或 / 命令…"
            }
        } else {
            value
        };
        lines.push(Line::from(vec![
            span(if i == 0 { "› " } else { "  " }, theme.accent()),
            span(
                value,
                if placeholder {
                    theme.muted()
                } else {
                    theme.base()
                },
            ),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(inner.x, y, inner.width, input_height),
    );
    // Native terminal cursor provides both a visible insertion mark and the IME anchor.
    if (ui.focus == Focus::Input || search) && input_height > 0 {
        frame.set_cursor_position((
            inner.x + 2 + (x as u16).min(inner.width.saturating_sub(3)),
            y + (row - offset) as u16,
        ));
    }
    if inner.height >= 2 {
        let hint = if search {
            "Enter 确认 · Esc 清除"
        } else if ui.focus == Focus::Input {
            "Enter 发送 · Ctrl+J 换行 · Esc 浏览"
        } else {
            "i 输入 · F6 切换区域"
        };
        frame.render_widget(
            Paragraph::new(Line::from(span(
                text::truncate(hint, inner.width as usize),
                theme.muted(),
            ))),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}
fn completions(frame: &mut Frame, ui: &Ui, content: Rect) {
    let choices = ui.candidates();
    if choices.is_empty() || content.height < 4 {
        return;
    }
    let height = (choices.len().min(5) as u16 + 2).min(content.height);
    let area = Rect::new(
        content.x + 1,
        content.bottom() - height,
        content.width.saturating_sub(2).min(70),
        height,
    );
    frame.render_widget(Clear, area);
    let block = panel(ui.theme, "↑↓ 选择 · Tab 确认".into(), true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let selected = ui.candidate.min(choices.len() - 1);
    let offset = selected.saturating_sub(inner.height.saturating_sub(1) as usize);
    let lines = choices
        .iter()
        .enumerate()
        .skip(offset)
        .take(inner.height as usize)
        .map(|(i, c)| {
            Line::from(span(
                text::truncate(&format!("{}  {}", c.label, c.hint), inner.width as usize),
                if i == selected {
                    ui.theme.selected()
                } else {
                    ui.theme.base()
                },
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
}
