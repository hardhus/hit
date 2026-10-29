//! A two-line item list with selection, scrolling and a staggered slide-in
//! when it appears.

use std::time::Duration;

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use crate::{Theme, anim};

/// Rows are two lines plus a spacer.
const ROW_HEIGHT: u16 = 3;
/// Delay between rows sliding in, and how long each takes.
const STAGGER: Duration = Duration::from_millis(14);
const SLIDE: Duration = Duration::from_millis(160);
/// Columns a row travels while sliding in.
const SLIDE_COLUMNS: f32 = 6.0;

#[derive(Debug, Clone)]
pub struct Row {
    pub top: Line<'static>,
    pub bottom: Line<'static>,
    /// Color of the selection bar for this row.
    pub accent: Color,
}

/// Selection and scroll position. Kept by the screen, so it survives data
/// refreshes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListState {
    pub selected: usize,
    offset: usize,
}

impl ListState {
    pub fn select(&mut self, index: usize, len: usize) {
        self.selected = index.min(len.saturating_sub(1));
    }

    pub fn move_by(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected.saturating_add_signed(delta);
        self.selected = next.min(len - 1);
    }

    pub fn top(&mut self) {
        self.selected = 0;
    }

    pub fn bottom(&mut self, len: usize) {
        self.selected = len.saturating_sub(1);
    }

    /// Rows that fit in `height`.
    pub fn page(height: u16) -> usize {
        usize::from((height / ROW_HEIGHT).max(1))
    }

    fn scroll_into_view(&mut self, visible: usize) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + visible {
            self.offset = self.selected + 1 - visible;
        }
    }
}

/// Props for drawing a list.
pub struct ItemList<'a> {
    pub rows: &'a [Row],
    pub theme: &'a Theme,
    /// When the list appeared; rows slide in from it. `None` draws them in place.
    pub appeared: Option<anim::Moment>,
    /// Where the highlight is drawn, as a (fractional) item index; lets it
    /// glide between rows. `None` draws it on the selected row.
    pub cursor: Option<f32>,
}

impl ItemList<'_> {
    pub fn render(&self, frame: &mut Frame, area: Rect, state: &mut ListState) {
        let visible = ListState::page(area.height);
        state.select(state.selected, self.rows.len());
        state.scroll_into_view(visible);

        let buf = frame.buffer_mut();
        self.render_highlight(buf, area, state, visible);
        let rows = self
            .rows
            .iter()
            .enumerate()
            .skip(state.offset)
            .take(visible);
        for (slot, (_, row)) in rows.enumerate() {
            let y = area.y + slot as u16 * ROW_HEIGHT;
            let row_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 2.min(area.bottom().saturating_sub(y)),
            };
            let progress = self.appeared.map_or(1.0, |at| {
                anim::ease_out(at.progress(STAGGER * slot as u32, SLIDE))
            });
            self.render_row(buf, row_area, row, progress);
        }
    }

    /// The selection background and bar, possibly between two rows.
    fn render_highlight(&self, buf: &mut Buffer, area: Rect, state: &ListState, visible: usize) {
        if self.rows.is_empty() {
            return;
        }
        let max_slot = (visible.min(self.rows.len() - state.offset) as f32 - 1.0).max(0.0);
        let index = self.cursor.unwrap_or(state.selected as f32);
        let slot = (index - state.offset as f32).clamp(0.0, max_slot);
        let y = area.y + (slot * f32::from(ROW_HEIGHT)).round() as u16;
        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 2.min(area.bottom().saturating_sub(y)),
        };
        let accent = self
            .rows
            .get(state.selected)
            .map_or(self.theme.accent, |r| r.accent);
        buf.set_style(rect, Style::new().bg(self.theme.selection));
        for y in rect.y..rect.bottom() {
            if let Some(cell) = buf.cell_mut((rect.x, y)) {
                cell.set_symbol("▌").set_fg(accent);
            }
        }
    }

    fn render_row(&self, buf: &mut Buffer, area: Rect, row: &Row, progress: f32) {
        let shift = ((1.0 - progress) * SLIDE_COLUMNS).round() as u16;
        let text = Rect {
            x: area.x + 2 + shift,
            width: area.width.saturating_sub(3 + shift),
            ..area
        };
        let mut paragraph = Paragraph::new(vec![row.top.clone(), row.bottom.clone()]);
        if progress < 1.0 {
            paragraph = paragraph.style(Style::new().add_modifier(Modifier::DIM));
        }
        paragraph.render(text, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_stays_in_bounds() {
        let mut state = ListState::default();
        state.move_by(-3, 5);
        assert_eq!(state.selected, 0);
        state.move_by(10, 5);
        assert_eq!(state.selected, 4);
        state.select(9, 3);
        assert_eq!(state.selected, 2);
        state.move_by(1, 0);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn scrolls_to_keep_selection_visible() {
        let mut state = ListState::default();
        state.select(7, 10);
        state.scroll_into_view(3);
        assert_eq!(state.offset, 5);
        state.top();
        state.scroll_into_view(3);
        assert_eq!(state.offset, 0);
    }
}
