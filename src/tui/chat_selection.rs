use super::text;
use ratatui::{
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span, StyledGrapheme},
};
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ChatPosition {
    row: usize,
    column: usize,
}

#[derive(Default)]
pub(super) struct ChatSelection {
    area: Rect,
    offset: usize,
    lines: Vec<Line<'static>>,
    endpoints: Option<(ChatPosition, ChatPosition)>,
    dragging: bool,
}

impl ChatSelection {
    pub(super) fn set_viewport(&mut self, area: Rect, offset: usize, lines: Vec<Line<'static>>) {
        if area.width != self.area.width {
            self.clear();
        }
        self.area = area;
        self.offset = offset;
        self.lines = lines;
    }

    pub(super) fn clear(&mut self) -> bool {
        self.dragging = false;
        self.endpoints.take().is_some()
    }

    fn position(&self, point: Position) -> ChatPosition {
        let row = point
            .y
            .saturating_sub(self.area.y)
            .min(self.area.height.saturating_sub(1));
        let column = point
            .x
            .saturating_sub(self.area.x)
            .min(self.area.width.saturating_sub(1));
        ChatPosition {
            row: self.offset + usize::from(row),
            column: usize::from(column),
        }
    }

    pub(super) fn start(&mut self, point: Position) -> bool {
        let cleared = self.clear();
        let anchor = self.position(point);
        if !self.area.contains(point) || anchor.row >= self.lines.len() {
            return cleared;
        }
        self.endpoints = Some((anchor, anchor));
        self.dragging = true;
        true
    }

    pub(super) fn extend(&mut self, point: Position, released: bool) -> bool {
        if !self.dragging {
            return false;
        }
        let mut position = self.position(point);
        position.row = position.row.min(self.lines.len().saturating_sub(1));
        if let Some((_, cursor)) = &mut self.endpoints {
            *cursor = position;
        }
        self.dragging = !released;
        true
    }

    pub(super) fn shift_rows(&mut self, rows: usize) {
        if let Some((anchor, cursor)) = &mut self.endpoints {
            anchor.row += rows;
            cursor.row += rows;
        }
    }

    fn ordered(&self) -> Option<(ChatPosition, ChatPosition)> {
        let (anchor, cursor) = self.endpoints?;
        (anchor != cursor).then_some((anchor.min(cursor), anchor.max(cursor)))
    }

    pub(super) fn has_text_selection(&self) -> bool {
        self.ordered().is_some()
    }

    fn columns(&self, row: usize) -> Option<Range<usize>> {
        let (start, end) = self.ordered()?;
        if !(start.row..=end.row).contains(&row) {
            return None;
        }
        let left = if row == start.row { start.column } else { 0 };
        let right = if row == end.row {
            end.column + 1
        } else {
            usize::from(self.area.width)
        };
        Some(left..right)
    }

    pub(super) fn visible_lines(&self) -> Vec<Line<'static>> {
        self.lines
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(usize::from(self.area.height))
            .map(|(row, line)| {
                let Some(columns) = self.columns(row) else {
                    return line.clone();
                };
                let spans = graphemes_in_columns(line, columns)
                    .map(|(glyph, selected)| {
                        let style = if selected {
                            glyph.style.add_modifier(Modifier::REVERSED)
                        } else {
                            glyph.style
                        };
                        Span::styled(glyph.symbol.to_owned(), style)
                    })
                    .collect::<Vec<_>>();
                Line::from(spans).style(line.style)
            })
            .collect()
    }

    pub(super) fn copy_text(&self) -> Option<String> {
        let (start, end) = self.ordered()?;
        let mut output = String::new();
        for (row, line) in self
            .lines
            .iter()
            .enumerate()
            .skip(start.row)
            .take(end.row - start.row + 1)
        {
            if row != start.row {
                output.push('\n');
            }
            if let Some(columns) = self.columns(row) {
                for (glyph, selected) in graphemes_in_columns(line, columns) {
                    if selected {
                        output.push_str(glyph.symbol);
                    }
                }
            }
        }
        (!output.is_empty()).then_some(output)
    }
}

fn graphemes_in_columns<'a>(
    line: &'a Line<'a>,
    columns: Range<usize>,
) -> impl Iterator<Item = (StyledGrapheme<'a>, bool)> {
    let mut column = 0;
    line.styled_graphemes(Style::default()).map(move |glyph| {
        let end = column + text::width(glyph.symbol);
        let selected = column < columns.end && end > columns.start;
        column = end;
        (glyph, selected)
    })
}
