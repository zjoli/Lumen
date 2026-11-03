use super::text::{clean, width};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub struct Editor {
    pub text: String,
    /// UTF-8 byte offset on a grapheme boundary, never a character count.
    pub cursor: usize,
}

impl Editor {
    pub fn set(&mut self, value: String) {
        self.text = clean(&value);
        self.cursor = self.text.len();
    }
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }
    fn boundary(&mut self) {
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(self.text.len()))
            .find(|i| *i >= self.cursor)
            .unwrap_or(self.text.len());
    }
    pub fn insert(&mut self, value: &str) {
        let value = clean(&value.replace("\r\n", "\n").replace('\r', "\n"));
        self.text.insert_str(self.cursor, &value);
        self.cursor += value.len();
        self.boundary();
    }
    pub fn left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }
    pub fn right(&mut self) {
        self.cursor += self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map(str::len)
            .unwrap_or(0);
    }
    pub fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.text.replace_range(self.cursor..end, "");
        self.boundary();
    }
    pub fn delete(&mut self) {
        let start = self.cursor;
        self.right();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.boundary();
    }
    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
    }
    pub fn end(&mut self) {
        self.cursor += self.text[self.cursor..]
            .find('\n')
            .unwrap_or(self.text.len() - self.cursor);
    }
    pub fn token(&self) -> (usize, &str) {
        let start = self.text[..self.cursor]
            .char_indices()
            .rfind(|(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        (start, &self.text[start..self.cursor])
    }
    pub fn replace_token(&mut self, value: &str) {
        let start = self.token().0;
        self.text.replace_range(start..self.cursor, value);
        self.cursor = start + value.len();
        self.boundary();
    }
    fn visual(&self, max: usize) -> (Vec<String>, Vec<(usize, usize, usize)>) {
        let max = max.max(1);
        let mut lines = vec![String::new()];
        let mut positions = vec![];
        let mut col = 0;
        for (i, g) in self.text.grapheme_indices(true) {
            if g != "\n" && col + width(g) > max {
                lines.push(String::new());
                col = 0;
            }
            positions.push((i, col, lines.len() - 1));
            if g == "\n" {
                lines.push(String::new());
                col = 0;
            } else {
                lines.last_mut().unwrap().push_str(g);
                col += width(g);
            }
        }
        if col >= max {
            lines.push(String::new());
            col = 0;
        }
        positions.push((self.text.len(), col, lines.len() - 1));
        (lines, positions)
    }
    pub fn layout(&self, max: usize) -> (Vec<String>, (usize, usize)) {
        let (lines, positions) = self.visual(max);
        let (_, x, y) = positions
            .iter()
            .find(|(i, _, _)| *i == self.cursor)
            .copied()
            .unwrap_or((0, 0, 0));
        (lines, (x, y))
    }
    pub fn vertical(&mut self, down: bool, max: usize) {
        let (_, positions) = self.visual(max);
        let Some((_, x, y)) = positions
            .iter()
            .find(|(i, _, _)| *i == self.cursor)
            .copied()
        else {
            return;
        };
        let next = if down {
            y + 1
        } else if y > 0 {
            y - 1
        } else {
            return;
        };
        if let Some((i, _, _)) = positions
            .iter()
            .filter(|(_, _, r)| *r == next)
            .min_by_key(|(_, c, _)| c.abs_diff(x))
        {
            self.cursor = *i;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caret_tracks_cjk_combining_marks_wrapping_and_navigation() {
        let mut e = Editor::default();
        e.insert("中文e\u{301}👩‍💻");
        e.backspace();
        assert_eq!(e.text, "中文e\u{301}");
        e.left();
        e.delete();
        assert_eq!(e.text, "中文");
        assert_eq!(e.layout(4).1, (0, 1));
        e.insert("换行");
        e.vertical(false, 4);
        assert_eq!(e.cursor, "中文".len());
        e.insert("\n");
        assert!(e.text.contains('\n'));
    }
    #[test]
    fn pasted_control_codes_are_not_terminal_commands() {
        let mut e = Editor::default();
        e.insert("中\r\n文\x1b\x07");
        assert_eq!(e.text, "中\n文");
        assert_eq!(e.layout(4).1, (2, 1));
        e.set("问题　@概念".into());
        assert_eq!(e.token().1, "@概念");
    }
}
