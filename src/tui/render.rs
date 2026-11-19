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
fn panel(theme: Theme, title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme.border(false))
        .title(Line::from(span(format!(" {title} "), theme.base().bold())))
        .padding(Padding::horizontal(1))
        .style(theme.base())
}
/// The same frame and spacing are used by every modal selector and form.
pub(super) fn modal(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    title: &str,
    width: u16,
    height: u16,
) -> Option<Rect> {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    if width < 12 || height < 5 {
        return None;
    }
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = panel(
        theme,
        &text::truncate(title, width.saturating_sub(6) as usize),
    );
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    Some(inner)
}
pub(super) fn filter_input(
    frame: &mut Frame,
    area: Rect,
    value: &str,
    cursor: usize,
    theme: Theme,
) {
    let mut editor = super::input::Editor::default();
    editor.set(value.into());
    editor.cursor = cursor;
    let prefix = "搜索  ";
    let prefix_width = text::width(prefix) as u16;
    let (lines, (x, y)) =
        editor.layout(area.width.saturating_sub(prefix_width + 1).max(1) as usize);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            span(prefix, theme.muted()),
            span(lines.get(y).cloned().unwrap_or_default(), theme.base()),
        ])),
        area,
    );
    frame.set_cursor_position((
        area.x + (prefix_width + x as u16).min(area.width.saturating_sub(1)),
        area.y,
    ));
}
fn section(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    title: &str,
    detail: &str,
    focused: bool,
) -> Rect {
    let inner = Rect::new(
        area.x + 1,
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );
    if inner.height == 0 || inner.width == 0 {
        return inner;
    }
    let detail = if inner.width >= 16 {
        text::truncate(&text::clean(detail), inner.width as usize / 3)
    } else {
        String::new()
    };
    let detail_width = text::width(&detail) as u16;
    let title_width = inner.width.saturating_sub(if detail_width > 0 {
        detail_width + 2
    } else {
        0
    });
    frame.render_widget(
        Paragraph::new(span(
            text::truncate(title, title_width as usize),
            if focused {
                theme.accent().bold()
            } else {
                theme.base().bold()
            },
        )),
        Rect::new(inner.x, inner.y, title_width, 1),
    );
    if detail_width > 0 {
        frame.render_widget(
            Paragraph::new(span(detail, theme.muted())),
            Rect::new(inner.right() - detail_width, inner.y, detail_width, 1),
        );
    }
    let offset = inner.height.min(2);
    Rect::new(
        inner.x,
        inner.y + offset,
        inner.width,
        inner.height - offset,
    )
}
#[derive(Clone, Copy)]
pub struct Geometry {
    pub header: Rect,
    pub footer: Rect,
    pub body: Rect,
    pub tree: Rect,
    pub content: Rect,
    pub input: Rect,
}
fn editor_width(width: u16) -> usize {
    width.saturating_sub(7).max(1) as usize
}
pub fn geometry(area: Rect, ui: &Ui) -> Geometry {
    let inner = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let header = Rect::new(inner.x, inner.y, inner.width, inner.height.min(1));
    let footer = Rect::new(
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        inner.height.min(1),
    );
    let body = Rect::new(
        inner.x,
        inner.y + inner.height.min(2),
        inner.width,
        inner.height.saturating_sub(4),
    );
    let tree_width = ((u32::from(area.width) * 23 / 100) as u16)
        .clamp(24, 34)
        .min(body.width / 3)
        .min(body.width.saturating_sub(16));
    let right = Rect::new(
        body.x + tree_width,
        body.y,
        body.width.saturating_sub(tree_width),
        body.height,
    );
    let editor = if ui.focus == Focus::Search {
        &ui.search
    } else {
        &ui.editor
    };
    let (rows, _) = editor.layout(editor_width(right.width));
    let input_height = (3 + rows.len().clamp(1, 5) as u16 + u16::from(ui.reference.is_some()))
        .min(body.height.saturating_sub(3))
        .max(if body.height < 5 { 2 } else { 3 })
        .min(body.height);
    Geometry {
        header,
        footer,
        body,
        tree: Rect::new(body.x, body.y, tree_width, body.height),
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
    }
}
fn horizontal(frame: &mut Frame, theme: Theme, x: u16, y: u16, width: u16) {
    frame.render_widget(
        Paragraph::new(span("─".repeat(width as usize), theme.border(false))),
        Rect::new(x, y, width, 1),
    );
}
fn junction(frame: &mut Frame, theme: Theme, x: u16, y: u16, symbol: &str) {
    if frame.area().contains((x, y).into()) {
        frame.buffer_mut()[(x, y)]
            .set_symbol(symbol)
            .set_style(theme.border(false));
    }
}
pub fn draw(frame: &mut Frame, ui: &mut Ui) {
    let area = frame.area();
    let theme = ui.theme;
    ui.latest_button = None;
    frame.render_widget(Block::default().style(theme.base()), area);
    if area.width < 24 || area.height < 9 {
        ui.chat_selection = Default::default();
        frame.render_widget(
            Paragraph::new("请扩大终端\nCtrl+Q 退出").style(theme.base()),
            area,
        );
        return;
    }
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .style(theme.base())
            .border_style(theme.border(false)),
        area,
    );
    let g = geometry(area, ui);
    header(frame, ui, g.header);
    horizontal(frame, theme, g.body.x, g.header.bottom(), g.body.width);
    horizontal(frame, theme, g.body.x, g.footer.y - 1, g.body.width);
    for y in [g.header.bottom(), g.footer.y - 1] {
        junction(frame, theme, area.x, y, "├");
        junction(frame, theme, area.right() - 1, y, "┤");
    }
    directory(frame, ui, g.tree);
    content(frame, ui, g.content);
    composer(frame, ui, g.input);
    let tree = g.tree;
    frame.render_widget(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(theme.border(false)),
        tree,
    );
    junction(frame, theme, tree.right() - 1, g.header.bottom(), "┬");
    junction(frame, theme, tree.right() - 1, g.footer.y - 1, "┴");
    let left = g.content.x - 1;
    if g.content.height >= 2 {
        junction(frame, theme, left, g.content.y + 1, "├");
        junction(frame, theme, area.right() - 1, g.content.y + 1, "┤");
    }
    if g.input.height > 0 {
        junction(frame, theme, left, g.input.y, "├");
        junction(frame, theme, area.right() - 1, g.input.y, "┤");
    }
    completions(frame, ui, g);
    footer(frame, ui, g.footer);
    if ui.session_panel {
        session_panel(frame, ui, area);
    }
    if let Some(dialog) = &ui.provider_dialog {
        super::provider_setup::draw(frame, dialog, ui.theme, area);
    }
}
fn header(frame: &mut Frame, ui: &Ui, area: Rect) {
    let theme = ui.theme;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            span(" Lumen", theme.base().bold()),
            span(format!("  v{}", env!("CARGO_PKG_VERSION")), theme.muted()),
        ])),
        area,
    );
    let scope = if ui.workspace_path.is_some() {
        &ui.workspace_name
    } else {
        "独立会话"
    };
    if area.width >= 76 {
        let segment = area.width / 3;
        frame.render_widget(
            Paragraph::new(span(
                text::truncate(scope, segment.saturating_sub(2) as usize),
                theme.muted(),
            ))
            .centered(),
            Rect::new(area.x + segment, area.y, segment, 1),
        );
        let title = if ui.session_title.is_empty() {
            "新会话"
        } else {
            &ui.session_title
        };
        frame.render_widget(
            Paragraph::new(span(
                text::truncate(title, segment.saturating_sub(2) as usize),
                theme.base(),
            ))
            .right_aligned(),
            Rect::new(area.right() - segment, area.y, segment - 1, 1),
        );
    } else if area.width >= 42 {
        frame.render_widget(
            Paragraph::new(span(
                text::truncate(scope, area.width.saturating_sub(23) as usize),
                theme.muted(),
            ))
            .right_aligned(),
            Rect::new(area.x + 22, area.y, area.width - 23, 1),
        );
    }
}
fn footer(frame: &mut Frame, ui: &Ui, row: Rect) {
    let theme = ui.theme;
    let row = Rect::new(row.x + 1, row.y, row.width.saturating_sub(2), row.height);
    let state = if let Some(notice) = &ui.notice {
        notice.clone()
    } else if ui.view == View::Chat && ui.chat_selection.has_text_selection() {
        "Ctrl+C 复制选区 · Esc 取消选中".into()
    } else if ui.history_loading {
        "加载更早对话…".into()
    } else if ui.busy {
        format!(
            "{} · Ctrl+C 停止",
            if ui.activity.is_empty() {
                "处理中…"
            } else {
                &ui.activity
            }
        )
    } else if ui.view == View::Read && ui.unread > 0 {
        format!("{} 条新回复 · F1 查看", ui.unread)
    } else if ui.model == "mock" {
        "演示模式".into()
    } else {
        "就绪".into()
    };
    let model = text::truncate(&text::clean(&ui.model), row.width as usize / 3);
    let model_width = text::width(&model) as u16;
    let state = text::truncate(&state, row.width.saturating_sub(model_width + 2) as usize);
    let state_width = text::width(&state) as u16;
    frame.render_widget(
        Paragraph::new(span(
            state,
            if ui.error && ui.notice.is_some() {
                theme.error()
            } else {
                theme.muted()
            },
        )),
        row,
    );
    frame.render_widget(
        Paragraph::new(span(model, theme.muted())).right_aligned(),
        Rect::new(row.right() - model_width, row.y, model_width, 1),
    );
    if ui.notice.is_none() && !ui.busy && ui.provider_dialog.is_none() && !ui.session_panel {
        let hint = if ui.focus == Focus::Search {
            "Enter 确认 · Esc 返回 · / 命令"
        } else if !ui.candidates().is_empty() {
            if ui.editor.token().1.starts_with('@') {
                "↑↓ 选择 · Enter/Tab 引用 · Esc 取消"
            } else {
                "↑↓ 选择 · Enter 执行 · Tab 补全"
            }
        } else if ui.focus == Focus::Tree {
            "↑↓ 选择 · Enter 打开 · f 筛选 · / 命令"
        } else if ui.focus == Focus::Content && ui.view == View::Read {
            "a 引用 · f 查找 · / 命令"
        } else {
            "F1 会话 · F2 阅读 · Ctrl+B 目录 · / 命令"
        };
        let available = row.width.saturating_sub(state_width + model_width + 4);
        if text::width(hint) <= available as usize {
            frame.render_widget(
                Paragraph::new(span(hint, theme.muted())).centered(),
                Rect::new(row.x + state_width + 2, row.y, available, 1),
            );
        }
    }
}
fn session_panel(frame: &mut Frame, ui: &Ui, area: Rect) {
    let title = if ui.session_panel_for_resume {
        "恢复会话"
    } else {
        "会话"
    };
    let Some(inner) = modal(frame, area, ui.theme, title, 88, 20) else {
        return;
    };
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
    let count = filtered.len();
    let mut lines = vec![Line::default(), Line::default()];
    let visible = inner.height.saturating_sub(3) as usize / 2;
    let start = ui.session_choice.saturating_sub(visible.saturating_sub(1));
    for (index, info) in filtered.iter().enumerate().skip(start).take(visible) {
        let selected = index == ui.session_choice;
        let style = if selected {
            ui.theme.selected()
        } else {
            ui.theme.base()
        };
        let health = match info.health {
            crate::session::SessionHealth::Complete => "",
            crate::session::SessionHealth::Interrupted => "上次运行中断 · ",
            crate::session::SessionHealth::Corrupt => "记录损坏 · 只读 · ",
            crate::session::SessionHealth::Diverged => "记录分叉 · 只读 · ",
            crate::session::SessionHealth::Unsupported => "版本不兼容 · 只读 · ",
        };
        lines.push(
            Line::from(span(
                text::truncate(
                    &format!(
                        "{} {}{}",
                        if selected { "›" } else { " " },
                        info.metadata.title,
                        if info.metadata.archived {
                            " · 已归档"
                        } else {
                            ""
                        }
                    ),
                    inner.width as usize,
                ),
                style,
            ))
            .style(style),
        );
        lines.push(Line::from(span(
            text::truncate(
                &format!(
                    "  {health}{} · {}",
                    text::truncate(&info.metadata.model.model, inner.width as usize / 3),
                    info.updated_at
                ),
                inner.width as usize,
            ),
            ui.theme.muted(),
        )));
    }
    if count == 0 {
        let message = if ui.session_panel_for_resume && ui.session_filter.is_empty() {
            "没有可恢复的会话"
        } else {
            "没有匹配的会话"
        };
        lines.push(Line::from(span(message, ui.theme.muted())));
    }
    frame.render_widget(Paragraph::new(lines), inner);
    let hint = if ui.session_next.is_some() {
        "↑↓ 选择 · Enter 打开 · PgDn 下一页 · Esc 返回"
    } else {
        "↑↓ 选择 · Enter 打开 · Esc 返回"
    };
    frame.render_widget(
        Paragraph::new(span(
            text::truncate(hint, inner.width as usize),
            ui.theme.muted(),
        )),
        Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
    );
    filter_input(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        &ui.session_filter,
        ui.session_filter.len(),
        ui.theme,
    );
}
fn directory(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let theme = ui.theme;
    let inner = section(
        frame,
        area,
        theme,
        if area.width < 12 {
            "目录"
        } else {
            "知识目录"
        },
        &if ui.tree.filter.is_empty() {
            format!("{} 项", ui.tree.objects.len())
        } else {
            "筛选中".into()
        },
        ui.focus == Focus::Tree || ui.focus == Focus::Search && ui.search_tree,
    );
    if inner.width == 0 || inner.height == 0 {
        ui.chat_selection = Default::default();
        return;
    }
    let rows = ui.tree.rows();
    if rows.is_empty() {
        let value = if ui.loading {
            "正在读取…"
        } else if ui.workspace_path.is_none() {
            "目录需要工作区\n\n/workspace open <路径>"
        } else if ui.tree.objects.is_empty() {
            "工作区暂无知识"
        } else {
            "没有匹配项"
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
            Line::from(vec![span(prefix, style), span(title, style)]).style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
}
fn content_header(frame: &mut Frame, ui: &Ui, area: Rect, title: &str, detail: &str) -> Rect {
    if area.width < 5 || area.height < 2 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let theme = ui.theme;
    let tab = |view| {
        if ui.view == view {
            theme.accent().bold().add_modifier(Modifier::UNDERLINED)
        } else {
            theme.muted()
        }
    };
    let tabs = Line::from(vec![
        span(" 会话 ", tab(View::Chat)),
        span("│", theme.border(false)),
        span(" 阅读 ", tab(View::Read)),
        span("│", theme.border(false)),
    ]);
    let tabs_width = tabs.width() as u16;
    frame.render_widget(
        Paragraph::new(tabs),
        Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), 1),
    );
    let available = area.width.saturating_sub(tabs_width + 4);
    let detail = if ui.view == View::Read && ui.unread > 0 && available > 30 {
        text::truncate(detail, available as usize / 3)
    } else {
        String::new()
    };
    let detail_width = text::width(&detail) as u16;
    let title_width = available.saturating_sub(if detail_width > 0 {
        detail_width + 2
    } else {
        0
    });
    frame.render_widget(
        Paragraph::new(span(
            text::truncate(title, title_width as usize),
            theme.base().bold(),
        )),
        Rect::new(area.x + tabs_width + 2, area.y, title_width, 1),
    );
    if detail_width > 0 {
        frame.render_widget(
            Paragraph::new(span(detail, theme.muted())),
            Rect::new(area.right() - detail_width - 1, area.y, detail_width, 1),
        );
    }
    horizontal(frame, theme, area.x, area.y + 1, area.width);
    Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height - 2,
    )
}
fn content(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let theme = ui.theme;
    let title = match ui.view {
        View::Chat => {
            if ui.session_title.is_empty() {
                "新会话".into()
            } else {
                text::clean(&ui.session_title)
            }
        }
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
            .unwrap_or_else(|| "阅读".into()),
    };
    let detail = if ui.view == View::Read && ui.unread > 0 {
        format!("新回复 {} · /chat", ui.unread)
    } else if ui.view == View::Read {
        "Markdown".into()
    } else {
        format!("{} 条消息", ui.messages.len())
    };
    let inner = content_header(frame, ui, area, &title, &detail);
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
    let visible = if ui.view == View::Chat && !ui.messages.is_empty() {
        ui.chat_selection.set_viewport(inner, offset, lines);
        ui.chat_selection.visible_lines()
    } else {
        ui.chat_selection = Default::default();
        lines
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
            .collect::<Vec<_>>()
    };
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
    if ui.view == View::Chat
        && !ui.follow
        && offset < max
        && ui.provider_dialog.is_none()
        && !ui.session_panel
    {
        let label = " ↓ 回到最新 ";
        let width = text::width(label) as u16;
        if inner.width >= width && inner.height > 0 {
            let button = Rect::new(inner.right() - width, inner.bottom() - 1, width, 1);
            frame.render_widget(Paragraph::new(span(label, theme.selected())), button);
            ui.latest_button = Some(button);
        }
    }
}
fn reading(ui: &mut Ui, width: usize) -> Vec<Line<'static>> {
    let Some(doc) = &mut ui.document else {
        return text::wrap(
            Line::from(span("未打开文档 · /open <标题或 ID>", ui.theme.muted())),
            width,
        );
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
        let mut lines = text::wrap(
            Line::from(span("输入消息开始对话。", ui.theme.muted())),
            width,
        );
        lines.push(Line::default());
        lines.extend(text::wrap(
            Line::from(span(
                if ui.model == "mock" {
                    "/login 配置供应商 · /model 选择模型 · /help 帮助"
                } else {
                    "/model 选择模型 · /sessions 历史会话 · /help 帮助"
                },
                ui.theme.muted(),
            )),
            width,
        ));
        return lines;
    }
    let mut lines = vec![];
    let prepended = ui.prepended_messages.min(ui.messages.len());
    let mut added_lines = 0;
    for (index, message) in ui.messages.iter().enumerate() {
        if index == prepended {
            added_lines = lines.len();
        }
        let user = message.role == "你";
        let style = if message.role == "错误" {
            ui.theme.error()
        } else if user {
            ui.theme.surface()
        } else {
            ui.theme.base()
        };
        lines.push(
            Line::from(span(
                if user { "用户" } else { message.role },
                if message.role == "Lumen" || message.role == "错误" || user {
                    style.bold()
                } else {
                    ui.theme.muted()
                },
            ))
            .style(style),
        );
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
                            style
                        },
                    ))
                    .style(style),
                    width,
                ));
            }
        }
        lines.push(Line::default());
    }
    if prepended > 0 {
        let added_lines = if prepended == ui.messages.len() {
            lines.len()
        } else {
            added_lines
        };
        ui.chat_selection.shift_rows(added_lines);
        if !ui.follow {
            ui.chat_scroll = ui.chat_scroll.saturating_add(added_lines);
        }
        ui.prepended_messages = 0;
    }
    lines
}
fn composer(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let theme = ui.theme;
    let search = ui.focus == Focus::Search;
    let focused = ui.focus == Focus::Input || search;
    let compact = area.height < 4;
    let title = if search { "查找" } else { "输入" };
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(theme.border(false))
        .style(theme.base())
        .padding(Padding::horizontal(2));
    let block = if compact {
        block.title(Line::from(span(
            format!(" {title} "),
            if focused {
                theme.accent()
            } else {
                theme.muted()
            },
        )))
    } else {
        block
    };
    let mut inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 3 || inner.height == 0 {
        return;
    }
    if !compact {
        frame.render_widget(
            Paragraph::new(span(
                title,
                if focused {
                    theme.accent()
                } else {
                    theme.muted()
                },
            )),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        inner.y += 1;
        inner.height -= 1;
    }
    if let Some(reference) = ui
        .reference
        .as_ref()
        .filter(|_| !search && inner.height >= 2)
    {
        let value = format!(
            "引用  {}{}",
            reference.title,
            reference
                .heading
                .as_ref()
                .map(|h| format!(" / {h}"))
                .unwrap_or_default()
        );
        frame.render_widget(
            Paragraph::new(span(
                text::truncate(&text::clean(&value), inner.width as usize),
                theme.muted(),
            )),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        inner.y += 1;
        inner.height -= 1;
    }
    let width = editor_width(area.width);
    ui.input_width = width;
    let editor = if search { &ui.search } else { &ui.editor };
    let (rows, (x, row)) = editor.layout(width);
    let offset = row.saturating_sub(inner.height.saturating_sub(1) as usize);
    let lines = rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(inner.height as usize)
        .map(|(i, value)| {
            Line::from(vec![
                span(
                    if i == 0 { "› " } else { "  " },
                    if focused {
                        theme.accent()
                    } else {
                        theme.muted()
                    },
                ),
                span(value, theme.base()),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
    if focused {
        frame.set_cursor_position((
            inner.x + 2 + (x as u16).min(inner.width.saturating_sub(3)),
            inner.y + (row - offset) as u16,
        ));
    }
}
fn completions(frame: &mut Frame, ui: &Ui, geometry: Geometry) {
    let choices = ui.candidates();
    // Small windows may need the menu to cover the content header. Keep it
    // above the editor, inside the right pane, with at least one visible row.
    let available = geometry.input.y.saturating_sub(frame.area().y + 1);
    if choices.is_empty() || available < 3 {
        return;
    }
    let height = (choices.len().min(5) as u16 + 2).min(available);
    let area = Rect::new(
        geometry.content.x + 1,
        geometry.input.y - height,
        geometry.content.width.saturating_sub(2).min(70),
        height,
    );
    frame.render_widget(Clear, area);
    let block = panel(
        ui.theme,
        if ui.editor.token().1.starts_with('@') {
            "引用"
        } else {
            "命令"
        },
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let selected = ui.candidate.min(choices.len() - 1);
    let offset = selected.saturating_sub(inner.height.saturating_sub(1) as usize);
    let label_width = choices
        .iter()
        .map(|c| text::width(&c.label))
        .max()
        .unwrap_or(0)
        .min(inner.width.saturating_sub(4) as usize)
        .min(inner.width as usize * 2 / 3);
    let lines = choices
        .iter()
        .enumerate()
        .skip(offset)
        .take(inner.height as usize)
        .map(|(i, c)| {
            let style = if i == selected {
                ui.theme.selected()
            } else {
                ui.theme.base()
            };
            let label = text::truncate(&c.label, label_width);
            let padding = " ".repeat(label_width.saturating_sub(text::width(&label)) + 2);
            let hint = text::truncate(
                &c.hint,
                inner.width.saturating_sub(label_width as u16 + 4) as usize,
            );
            Line::from(vec![
                span(
                    format!("{} {label}{padding}", if i == selected { "›" } else { " " }),
                    style,
                ),
                span(
                    hint,
                    if i == selected {
                        style
                    } else {
                        ui.theme.muted()
                    },
                ),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
}
