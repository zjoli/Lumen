use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn clean(value: &str) -> String {
    value
        .replace('\t', "    ")
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}
pub fn width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}
pub fn truncate(value: &str, max: usize) -> String {
    if width(value) <= max {
        return value.into();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    for g in value.graphemes(true) {
        if width(&out) + width(g) + 1 > max {
            break;
        }
        out.push_str(g);
    }
    out.push('…');
    out
}
pub fn wrap(line: Line<'static>, max: usize) -> Vec<Line<'static>> {
    let max = max.max(1);
    let mut out = vec![];
    let mut current = vec![];
    let mut used = 0;
    for span in line.spans {
        for word in span.content.split_word_bounds() {
            if used > 0 && used + width(word) > max {
                out.push(Line::from(std::mem::take(&mut current)).style(line.style));
                used = 0;
                if word.chars().all(char::is_whitespace) {
                    continue;
                }
            }
            for g in word.graphemes(true) {
                let n = width(g);
                if n > max {
                    continue;
                }
                if used + n > max {
                    out.push(Line::from(std::mem::take(&mut current)).style(line.style));
                    used = 0;
                }
                current.push(Span::styled(g.to_owned(), span.style));
                used += n;
            }
        }
    }
    out.push(Line::from(current).style(line.style));
    out
}
