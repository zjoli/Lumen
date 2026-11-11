use super::{
    markdown,
    state::{Focus, Ui, View},
    text,
    theme::Theme,
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
        .border_type(BorderType::Plain)
        .border_style(theme.border(false))
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
fn section(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    title: &str,
    detail: &str,
    focused: bool,
    separator: bool,
) -> Rect {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme.border(false))
        .style(theme.base());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width < 2 {
        return Rect::new(inner.x, inner.y, 0, 0);
    }
    let header = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(Block::default().style(theme.surface()), header);
    let detail = text::truncate(
        &text::clean(detail),
        inner.width.saturating_sub(16) as usize,
    );
    let detail_width = text::width(&detail) as u16;
    let title_width = inner.width.saturating_sub(detail_width + 3);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            span(
                if focused { "▎" } else { " " },
                theme.accent().bg(theme.surface().bg.unwrap()),
            ),
            span(
                text::truncate(&text::clean(title), title_width.saturating_sub(1) as usize),
                theme.surface().bold(),
            ),
        ])),
        header,
    );
    if detail_width > 0 {
        frame.render_widget(
            Paragraph::new(span(
                detail,
                theme
                    .muted()
                    .patch(Style::new().bg(theme.surface().bg.unwrap())),
            )),
            Rect::new(inner.right() - detail_width - 1, inner.y, detail_width, 1),
        );
    }
    if separator && inner.height >= 3 {
        frame.render_widget(
            Paragraph::new("─".repeat(inner.width as usize)).style(theme.border(false)),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
        for (x, symbol) in [(area.x, "├"), (area.right() - 1, "┤")] {
            frame.buffer_mut()[(x, inner.y + 1)]
                .set_symbol(symbol)
                .set_style(theme.border(false));
        }
        Rect::new(
            inner.x + 1,
            inner.y + 2,
            inner.width.saturating_sub(2),
            inner.height - 2,
        )
    } else {
        Rect::new(
            inner.x + 1,
            inner.y + 1,
            inner.width.saturating_sub(2),
            inner.height - 1,
        )
    }
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
    let tree_width: u16 = if visible {
        if area.width >= 100 {
            (area.width / 4).clamp(28, 40)
        } else {
            24
        }
    } else {
        0
    };
    // Adjacent panels share one border cell instead of drawing separate boxes.
    let right_offset = tree_width.saturating_sub(u16::from(visible));
    let right = Rect::new(
        body.x + right_offset,
        body.y,
        body.width.saturating_sub(right_offset),
        body.height,
    );
    let (rows, _) = ui
        .editor
        .layout(right.width.saturating_sub(10).max(1) as usize);
    let wanted = if area.height >= 24 { 8 } else { 6 };
    let input_height =
        (wanted + u16::from(ui.reference.is_some()) + rows.len().saturating_sub(2).min(3) as u16)
            .min(body.height.saturating_sub(4))
            .max(5)
            .min(body.height);
    Geometry {
        tree: visible.then_some(Rect::new(body.x, body.y, tree_width, body.height)),
        content: Rect::new(
            right.x,
            right.y,
            right.width,
            right.height.saturating_sub(input_height) + 1,
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
        span(" ▤ ", theme.accent()),
        span("Lumen", theme.base().bold()),
        span("  /  ", theme.border(false)),
        span(
            text::truncate(
                &text::clean(&ui.workspace_name),
                area.width
                    .saturating_sub(if area.width >= 72 { 52 } else { 14 })
                    as usize,
            ),
            theme.muted(),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(header),
        Rect::new(area.x, area.y, area.width, 1),
    );
    if area.width >= 72 {
        let status = if ui.busy {
            "生成中"
        } else if ui.model == "mock" {
            "演示模式"
        } else {
            "就绪"
        };
        let status = format!(
            "{}  ·  {status}",
            text::truncate(&text::clean(&ui.model), 22)
        );
        frame.render_widget(
            Paragraph::new(Line::from(span(status, theme.muted())).right_aligned()),
            Rect::new(area.right() - 38, area.y, 36, 1),
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
    dividers(frame, g, theme);
    completions(frame, ui, g.content);
    if ui.session_panel {
        session_panel(frame, ui, area);
    }
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
        if ui.focus == Focus::Input {
            "Enter 发送  Ctrl+J 换行  ·  F1 目录  F2 阅读  F3 对话  ·  F6 切换  Ctrl+Q 退出".into()
        } else if ui.focus == Focus::Search {
            "Enter 确认  Esc 清除  ·  F6 切换  Ctrl+Q 退出".into()
        } else {
            format!("{focus}  ·  i 输入  F6 切换  ·  F1 目录  F2 阅读  F3 对话  Ctrl+Q 退出")
        }
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
fn session_panel(frame: &mut Frame, ui: &Ui, area: Rect) {
    let width = area.width.saturating_sub(4).min(100);
    let height = area.height.saturating_sub(4).min(20);
    if width < 10 || height < 4 {
        return;
    }
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = panel(
        ui.theme,
        "会话 · 输入过滤 · Enter 打开 · PgDn 下一页 · Esc 返回".into(),
        true,
    );
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    let filtered: Vec<_> = ui
        .session_choices
        .iter()
        .filter(|info| {
            info.metadata
                .title
                .to_lowercase()
                .contains(&ui.session_filter.to_lowercase())
                || info.identity.session_id.contains(&ui.session_filter)
        })
        .collect();
    let visible = inner.height.saturating_sub(1) as usize;
    let start = ui.session_choice.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![Line::from(span(
        format!("过滤: {}", ui.session_filter),
        ui.theme.accent(),
    ))];
    for (index, info) in filtered.iter().enumerate().skip(start).take(visible) {
        lines.push(Line::from(span(
            format!(
                "{} {}{} · {} · {:?} · {} · {}",
                if index == ui.session_choice {
                    "›"
                } else {
                    " "
                },
                info.metadata.title,
                if info.metadata.archived {
                    " [已归档]"
                } else {
                    ""
                },
                info.metadata.model.model,
                info.health,
                info.updated_at,
                info.identity
                    .session_id
                    .chars()
                    .rev()
                    .take(8)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            ),
            if index == ui.session_choice {
                ui.theme.accent()
            } else {
                ui.theme.base()
            },
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
fn dividers(frame: &mut Frame, g: Geometry, theme: Theme) {
    // Restore junctions after the overlapping panel borders have been drawn.
    let x = g.content.x;
    let y = g.input.y;
    let mut junctions = vec![(x, y, "├"), (g.content.right() - 1, y, "┤")];
    if g.tree.is_some() {
        junctions.extend([(x, g.content.y, "┬"), (x, g.input.bottom() - 1, "┴")]);
        if g.content.height >= 5 {
            junctions.push((x, g.content.y + 2, "┼"));
        }
    }
    for (x, y, symbol) in junctions {
        frame.buffer_mut()[(x, y)]
            .set_symbol(symbol)
            .set_style(theme.border(false));
    }
}
fn directory(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let theme = ui.theme;
    let mut inner = section(
        frame,
        area,
        theme,
        "知识目录",
        if ui.tree.filter.is_empty() {
            "/ 搜索"
        } else {
            "筛选中"
        },
        ui.focus == Focus::Tree || ui.focus == Focus::Search && ui.search_tree,
        true,
    );
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if inner.height >= 12 {
        let summary_y = inner.bottom() - 3;
        frame.render_widget(
            Paragraph::new("─".repeat(inner.width as usize)).style(theme.border(false)),
            Rect::new(inner.x, summary_y, inner.width, 1),
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(span(
                    format!("{} 项知识", ui.tree.objects.len()),
                    theme.muted(),
                )),
                Line::from(span("Enter 打开 · / 筛选", theme.muted())),
            ]),
            Rect::new(inner.x, summary_y + 1, inner.width, 2),
        );
        inner.height -= 4;
    }
    let rows = ui.tree.rows();
    if rows.is_empty() {
        let value = if ui.loading {
            "正在读取…"
        } else if ui.workspace_path.is_none() {
            "暂无知识库\n\n打开后在此浏览目录"
        } else if ui.tree.objects.is_empty() {
            "工作区暂无知识\n\n导入 Markdown 后\n按 F1 刷新目录"
        } else {
            "没有匹配项\n按 / 重新筛选\nEsc 清除筛选"
        };
        frame.render_widget(Paragraph::new(value).style(theme.muted()), inner);
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
                    " "
                }
            );
            let title = text::truncate(
                &text::clean(&object.title),
                inner.width.saturating_sub(text::width(&prefix) as u16 + 2) as usize,
            );
            let style = if i == ui.tree.selected {
                theme.selected()
            } else {
                theme.base()
            };
            Line::from(vec![
                span(prefix, style),
                span(
                    if matches!(
                        object.kind,
                        crate::knowledge::ObjectKind::Domain | crate::knowledge::ObjectKind::Topic
                    ) {
                        "▱ "
                    } else {
                        "· "
                    },
                    style,
                ),
                span(title, style),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
}
fn content(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let theme = ui.theme;
    let title = match ui.view {
        View::Chat => "对话 / 当前会话".into(),
        View::Read => ui
            .document
            .as_ref()
            .map(|d| {
                let mut titles = d
                    .path
                    .iter()
                    .filter_map(|id| ui.tree.get(id))
                    .filter(|o| o.id != d.object.metadata.id)
                    .map(|o| text::clean(&o.title))
                    .collect::<Vec<_>>();
                titles.push(text::clean(&d.object.metadata.title));
                titles.join(" / ")
            })
            .unwrap_or_else(|| "阅读 / 选择一篇知识".into()),
    };
    let detail = if ui.view == View::Read && ui.unread > 0 {
        format!("新回复 {} · /chat", ui.unread)
    } else if ui.view == View::Read {
        "Markdown".into()
    } else {
        format!("{} 条消息", ui.messages.len())
    };
    let inner = section(
        frame,
        area,
        theme,
        &title,
        &detail,
        ui.focus == Focus::Content || ui.focus == Focus::Search && !ui.search_tree,
        true,
    );
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
        let welcome = if ui.workspace_path.is_none() {
            "# 让知识彼此连接\n\n打开知识库，开始阅读与对话。\n\n**打开已有知识库**\n`/workspace open <路径>`\n\n**创建新的知识库**\n`/workspace init <路径>`\n\n输入 `/help` 查看更多命令。"
        } else {
            "# 从一个问题开始\n\n在左侧选择知识阅读，或在下方与 Agent 对话。\n\n`/open <标题>` 打开知识   `/help` 查看命令"
        };
        return markdown::render(welcome, width, ui.theme).lines;
    }
    let mut lines = vec![];
    for message in &ui.messages {
        lines.push(Line::from(span(
            if message.role == "你" {
                "› 你"
            } else if message.role == "Lumen" {
                "✦ Lumen"
            } else {
                message.role
            },
            if message.role == "错误" {
                ui.theme.error().bold()
            } else if message.role == "Lumen" {
                ui.theme.violet().bold()
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
    } else {
        "✦ Agent · 知识问答"
    };
    let detail = if search {
        "Enter 确认".into()
    } else if ui.busy {
        text::clean(&ui.activity)
    } else if ui.reference.is_some() {
        "引用 1 · Ctrl+R 移除".into()
    } else {
        "@ 引用知识".into()
    };
    let body = section(
        frame,
        area,
        theme,
        title,
        &detail,
        search || ui.focus == Focus::Input,
        false,
    );
    if body.width < 3 || body.height == 0 {
        return;
    }
    let mut edit_area = body;
    if let Some(reference) = ui
        .reference
        .as_ref()
        .filter(|_| !search && body.height >= 4)
    {
        let value = format!(
            "◇ {}{}",
            reference.title,
            reference
                .heading
                .as_ref()
                .map(|h| format!(" / {h}"))
                .unwrap_or_default()
        );
        frame.render_widget(
            Paragraph::new(span(
                text::truncate(&text::clean(&value), body.width as usize),
                theme.violet(),
            )),
            Rect::new(body.x, body.y, body.width, 1),
        );
        edit_area.y += 1;
        edit_area.height -= 1;
    }
    // Keep a little breathing room above a compact, terminal-native input field.
    if edit_area.height >= 4 {
        edit_area.y += 1;
        edit_area.height -= 1;
    }
    let inner = if edit_area.height >= 3 {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme.border(ui.focus == Focus::Input || search))
            .padding(Padding::horizontal(1))
            .style(theme.base());
        let inner = block.inner(edit_area);
        frame.render_widget(block, edit_area);
        inner
    } else {
        edit_area
    };
    let width = inner.width.saturating_sub(2).max(1) as usize;
    ui.input_width = width;
    let editor = if search { &ui.search } else { &ui.editor };
    let (rows, (x, row)) = editor.layout(width);
    let offset = row.saturating_sub(inner.height.saturating_sub(1) as usize);
    let mut lines = vec![];
    for (i, value) in rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(inner.height as usize)
    {
        let placeholder = editor.text.is_empty();
        let value = if placeholder {
            if search {
                "输入查找内容…"
            } else {
                "向 Agent 提问，或输入 / 命令…"
            }
        } else {
            value
        };
        lines.push(Line::from(vec![
            span(if i == 0 { "› " } else { "  " }, theme.violet()),
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
    frame.render_widget(Paragraph::new(lines), inner);
    // The real caret anchors the IME and follows grapheme-aware visual wrapping.
    if (ui.focus == Focus::Input || search) && inner.height > 0 {
        frame.set_cursor_position((
            inner.x + 2 + (x as u16).min(inner.width.saturating_sub(3)),
            inner.y + (row - offset) as u16,
        ));
    }
}
fn completions(frame: &mut Frame, ui: &Ui, content: Rect) {
    let choices = ui.candidates();
    if choices.is_empty() || content.height < 4 {
        return;
    }
    let height = (choices.len().min(5) as u16 + 2).min(content.height - 2);
    let area = Rect::new(
        content.x + 1,
        content.bottom() - 1 - height,
        content.width.saturating_sub(2).min(70),
        height,
    );
    frame.render_widget(Clear, area);
    let block = panel(ui.theme, "↑↓ 选择 · Enter/Tab 确认".into(), true);
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
                text::truncate(
                    &format!(
                        "{} {}  {}",
                        if i == selected { "›" } else { " " },
                        c.label,
                        c.hint
                    ),
                    inner.width as usize,
                ),
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
