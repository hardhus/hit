//! A small multi-line text editor for comments: soft-wrapped, with a cursor
//! that keeps its place on screen.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

fn char_width(c: char) -> usize {
    Span::raw(c.to_string()).width()
}

#[derive(Debug, Clone)]
pub struct Editor {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    /// First visual row on screen.
    scroll: usize,
}

impl Editor {
    pub fn new(text: &str) -> Self {
        let mut editor = Self {
            lines: vec![Vec::new()],
            row: 0,
            col: 0,
            scroll: 0,
        };
        editor.insert_str(text);
        editor
    }

    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn is_blank(&self) -> bool {
        self.lines
            .iter()
            .all(|line| line.iter().all(|c| c.is_whitespace()))
    }

    fn line_mut(&mut self) -> Option<&mut Vec<char>> {
        self.lines.get_mut(self.row)
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines.get(row).map_or(0, Vec::len)
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            self.newline();
            return;
        }
        // Tabs and other control characters have no place in a text box.
        let c = if c == '\t' { ' ' } else { c };
        if c.is_control() {
            return;
        }
        let col = self.col;
        if let Some(line) = self.line_mut() {
            line.insert(col, c);
            self.col += 1;
        }
    }

    /// Pasted text, with any kind of line ending.
    pub fn insert_str(&mut self, text: &str) {
        for c in text.replace("\r\n", "\n").replace('\r', "\n").chars() {
            self.insert_char(c);
        }
    }

    pub fn newline(&mut self) {
        let col = self.col;
        let rest = self
            .line_mut()
            .map(|line| line.split_off(col))
            .unwrap_or_default();
        self.row += 1;
        self.col = 0;
        self.lines.insert(self.row, rest);
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            let col = self.col;
            if let Some(line) = self.line_mut() {
                line.remove(col);
            }
        } else if self.row > 0 {
            let current = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len(self.row);
            if let Some(line) = self.line_mut() {
                line.extend(current);
            }
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.line_len(self.row) {
            let col = self.col;
            if let Some(line) = self.line_mut() {
                line.remove(col);
            }
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            if let Some(line) = self.line_mut() {
                line.extend(next);
            }
        }
    }

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.line_len(self.row);
        }
    }

    pub fn right(&mut self) {
        if self.col < self.line_len(self.row) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.line_len(self.row));
        }
    }

    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.line_len(self.row));
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = self.line_len(self.row);
    }

    /// Visual rows for `width`, and where the cursor is among them.
    fn wrapped(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut rows = Vec::new();
        let mut cursor = (0, 0);
        for (row, line) in self.lines.iter().enumerate() {
            let mut current = String::new();
            let mut used = 0;
            for (col, &c) in line.iter().enumerate() {
                let size = char_width(c);
                if used + size > width {
                    rows.push(std::mem::take(&mut current));
                    used = 0;
                }
                if row == self.row && col == self.col {
                    cursor = (rows.len(), used);
                }
                current.push(c);
                used += size;
            }
            if row == self.row && self.col >= line.len() {
                // At the end of a full row the cursor starts the next one.
                if used >= width {
                    rows.push(std::mem::take(&mut current));
                    used = 0;
                }
                cursor = (rows.len(), used);
            }
            rows.push(current);
        }
        (rows, cursor)
    }

    /// Draws the text and puts the terminal's cursor in its place.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, style: Style) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let (rows, (cursor_row, cursor_col)) = self.wrapped(usize::from(area.width));
        let height = usize::from(area.height);
        if cursor_row < self.scroll {
            self.scroll = cursor_row;
        } else if cursor_row >= self.scroll + height {
            self.scroll = cursor_row + 1 - height;
        }
        let lines: Vec<Line> = rows
            .into_iter()
            .skip(self.scroll)
            .take(height)
            .map(|row| Line::styled(row, style))
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
        let x = area.x + u16::try_from(cursor_col).unwrap_or(0);
        let y = area.y + u16::try_from(cursor_row - self.scroll).unwrap_or(0);
        frame.set_cursor_position((x, y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_moving() {
        let mut editor = Editor::new("");
        for c in "helo".chars() {
            editor.insert_char(c);
        }
        editor.left();
        editor.insert_char('l');
        assert_eq!(editor.text(), "hello");
        editor.end();
        editor.newline();
        editor.insert_str("world");
        assert_eq!(editor.text(), "hello\nworld");
        editor.up();
        editor.home();
        editor.delete();
        assert_eq!(editor.text(), "ello\nworld");
    }

    #[test]
    fn lines_join_and_split() {
        let mut editor = Editor::new("ab\ncd");
        editor.up();
        editor.home();
        editor.down();
        editor.home();
        editor.backspace();
        assert_eq!(editor.text(), "abcd");
        editor.newline();
        assert_eq!(editor.text(), "ab\ncd");
        editor.up();
        editor.end();
        editor.delete();
        assert_eq!(editor.text(), "abcd");
    }

    #[test]
    fn it_stops_at_the_edges() {
        let mut editor = Editor::new("");
        editor.backspace();
        editor.delete();
        editor.left();
        editor.right();
        editor.up();
        editor.down();
        assert_eq!(editor.text(), "");
        assert!(editor.is_blank());
    }

    #[test]
    fn a_paste_keeps_its_lines_and_loses_control_characters() {
        let editor = Editor::new("one\r\ntwo\rthree\x1b[31m\tfour");
        assert_eq!(editor.text(), "one\ntwo\nthree[31m four");
    }

    #[test]
    fn the_cursor_follows_wrapped_and_wide_text() {
        let editor = Editor::new("abcdef");
        let (rows, cursor) = editor.wrapped(4);
        assert_eq!(rows, ["abcd", "ef"]);
        assert_eq!(cursor, (1, 2));

        let editor = Editor::new("abcd");
        let (rows, cursor) = editor.wrapped(4);
        assert_eq!(rows, ["abcd", ""]);
        assert_eq!(cursor, (1, 0));

        // Wide characters never split across the edge.
        let editor = Editor::new("日本語");
        let (rows, cursor) = editor.wrapped(5);
        assert_eq!(rows, ["日本", "語"]);
        assert_eq!(cursor, (1, 2));
    }
}
