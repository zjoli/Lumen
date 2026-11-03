use super::{text, theme::Theme};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

#[derive(Clone)]
pub struct Link {
    pub label: String,
    pub target: String,
    pub line: usize,
}
#[derive(Default)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<Link>,
    pub sections: Vec<(usize, String)>,
}
struct Writer {
    out: Rendered,
    spans: Vec<Span<'static>>,
    style: Style,
    stack: Vec<Style>,
    quote: usize,
    code: bool,
    lists: Vec<Option<u64>>,
    link: Option<usize>,
    heading: Option<(usize, String)>,
    width: usize,
    theme: Theme,
}
impl Writer {
    fn push(&mut self, value: &str) {
        if self.spans.is_empty() && self.quote > 0 {
            self.spans
                .push(Span::styled("│ ".repeat(self.quote), self.theme.accent()));
        }
        if let Some(i) = self.link {
            self.out.links[i].label.push_str(value);
        }
        if let Some((_, title)) = &mut self.heading {
            title.push_str(value);
        }
        self.spans.push(Span::styled(value.to_owned(), self.style));
    }
    fn flush(&mut self) {
        if !self.spans.is_empty() {
            self.out.lines.extend(text::wrap(
                Line::from(std::mem::take(&mut self.spans)),
                self.width,
            ));
        }
    }
    fn blank(&mut self) {
        self.flush();
        if self.out.lines.last().is_some_and(|l| !l.spans.is_empty()) {
            self.out.lines.push(Line::default());
        }
    }
    fn style(&mut self, style: Style) {
        self.stack.push(self.style);
        self.style = self.style.patch(style);
    }
    fn restore(&mut self) {
        self.style = self.stack.pop().unwrap_or(self.theme.base());
    }
}
pub fn render(source: &str, width: usize, theme: Theme) -> Rendered {
    let source = text::clean(source);
    let mut w = Writer {
        out: Rendered::default(),
        spans: vec![],
        style: theme.base(),
        stack: vec![],
        quote: 0,
        code: false,
        lists: vec![],
        link: None,
        heading: None,
        width: width.max(1),
        theme,
    };
    for event in Parser::new_ext(
        &source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    ) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                w.blank();
                w.heading = Some((w.out.lines.len(), String::new()));
                w.style(if level == HeadingLevel::H1 {
                    theme.base().add_modifier(Modifier::BOLD)
                } else {
                    theme.accent().add_modifier(Modifier::BOLD)
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                w.blank();
                w.restore();
                if let Some(h) = w.heading.take() {
                    w.out.sections.push(h);
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                w.blank();
                w.quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                w.blank();
                w.quote = w.quote.saturating_sub(1);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                w.blank();
                w.code = true;
                let lang = match kind {
                    CodeBlockKind::Fenced(s) => s.to_string(),
                    _ => String::new(),
                };
                w.out
                    .lines
                    .push(Line::from(Span::styled(format!("  {lang}"), theme.muted())));
                w.style(theme.accent());
            }
            Event::End(TagEnd::CodeBlock) => {
                w.flush();
                w.code = false;
                w.restore();
                w.blank();
            }
            Event::Text(value) if w.code => {
                for line in value.split_inclusive('\n') {
                    w.push("  ");
                    w.push(line.trim_end_matches('\n'));
                    w.flush();
                }
            }
            Event::Text(value) | Event::Html(value) | Event::InlineHtml(value) => w.push(&value),
            Event::Start(Tag::Strong) => w.style(Style::new().add_modifier(Modifier::BOLD)),
            Event::Start(Tag::Emphasis) => w.style(Style::new().add_modifier(Modifier::ITALIC)),
            Event::Start(Tag::Strikethrough) => {
                w.style(Style::new().add_modifier(Modifier::CROSSED_OUT))
            }
            Event::End(TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough) => w.restore(),
            Event::Code(value) => {
                w.style(theme.accent());
                w.push(&value);
                w.restore();
            }
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                w.link = Some(w.out.links.len());
                w.out.links.push(Link {
                    label: String::new(),
                    target: dest_url.into_string(),
                    line: w.out.lines.len(),
                });
                w.style(theme.link());
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                w.link = None;
                w.push(&format!(" [{}]", w.out.links.len()));
                w.restore();
            }
            Event::Start(Tag::List(n)) => {
                w.flush();
                w.lists.push(n);
            }
            Event::End(TagEnd::List(_)) => {
                w.flush();
                w.lists.pop();
                if w.lists.is_empty() {
                    w.blank();
                }
            }
            Event::Start(Tag::Item) => {
                w.flush();
                let indent = "  ".repeat(w.lists.len().saturating_sub(1));
                let bullet = if let Some(Some(n)) = w.lists.last_mut() {
                    let s = format!("{n}. ");
                    *n += 1;
                    s
                } else {
                    "• ".into()
                };
                w.push(&format!("{indent}{bullet}"));
            }
            Event::End(TagEnd::Item) => w.flush(),
            Event::End(TagEnd::Paragraph) => {
                if w.lists.is_empty() && w.quote == 0 {
                    w.blank();
                } else {
                    w.flush();
                }
            }
            Event::SoftBreak => w.push(" "),
            Event::HardBreak => w.flush(),
            Event::TaskListMarker(done) => w.push(if done { "[x] " } else { "[ ] " }),
            Event::Rule => {
                w.blank();
                w.out
                    .lines
                    .push(Line::from(Span::styled("─".repeat(w.width), theme.muted())));
                w.blank();
            }
            Event::Start(Tag::TableHead) => {
                w.flush();
                w.style(theme.accent().add_modifier(Modifier::BOLD));
            }
            Event::End(TagEnd::TableHead) => {
                w.flush();
                w.restore();
            }
            Event::Start(Tag::TableRow) => w.flush(),
            Event::End(TagEnd::TableRow | TagEnd::Table) => w.blank(),
            Event::End(TagEnd::TableCell) => w.push(" │ "),
            _ => (),
        }
    }
    w.flush();
    while w.out.lines.last().is_some_and(|l| l.spans.is_empty()) {
        w.out.lines.pop();
    }
    w.out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headings_keep_terminal_size_and_markdown_links_are_navigable() {
        let out = render(
            "# 标题\n\n## 核心\n\n**中文**[来源](lumen:id)\n\n> 引用\n\n```sql\nSELECT 中文;\n```",
            18,
            Theme::Auto,
        );
        assert_eq!(out.links[0].target, "lumen:id");
        assert_eq!(out.links[0].label, "来源");
        assert_eq!(out.sections[1].1, "核心");
        assert!(out.lines.iter().all(|l| l.width() <= 18));
        let plain = out
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plain.contains("│ 引用"));
        assert!(!plain.contains("**"));
        assert!(plain.contains("SELECT 中文;"));
    }
}
