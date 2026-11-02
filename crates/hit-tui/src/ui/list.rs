//! A list of two-line items with optional group headers, selection,
//! scrolling, a gliding highlight and a staggered slide-in.

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

/// Items are two lines plus a spacer; headers one line.
const ITEM_HEIGHT: u16 = 3;
const HEADER_HEIGHT: u16 = 1;
/// Delay between rows sliding in, and how long each takes.
const STAGGER: Duration = Duration::from_millis(14);
const SLIDE: Duration = Duration::from_millis(160);
/// Columns a row travels while sliding in.
const SLIDE_COLUMNS: f32 = 6.0;

#[derive(Debug, Clone)]
pub enum Row {
    Item {
        top: Line<'static>,
        bottom: Line<'static>,
        /// Color of the selection bar for this row.
        accent: Color,
        /// Drawn faded (e.g. a notification just marked read).
        dimmed: bool,
    },
    /// A group title; never selected.
    Header(Line<'static>),
}

impl Row {
    pub fn item(top: Line<'static>, bottom: Line<'static>, accent: Color) -> Self {
        Self::Item {
            top,
            bottom,
            accent,
            dimmed: false,
        }
    }

    fn height(&self) -> u16 {
        match self {
            Self::Item { .. } => ITEM_HEIGHT,
            Self::Header(_) => HEADER_HEIGHT,
        }
    }

    pub fn is_item(&self) -> bool {
        matches!(self, Self::Item { .. })
    }
}

/// Selection (a row index, always on an item) and scroll position. Kept by
/// the screen, so it survives data refreshes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListState {
    pub selected: usize,
    offset: usize,
}

/// The nearest item at or after `from` going in `step` direction.
fn item_from(rows: &[Row], from: usize, step: isize) -> Option<usize> {
    let mut index = from as isize;
    while index >= 0 && (index as usize) < rows.len() {
        if rows.get(index as usize).is_some_and(Row::is_item) {
            return Some(index as usize);
        }
        index += step;
    }
    None
}

impl ListState {
    /// Keeps the selection on an item and in bounds.
    pub fn fix(&mut self, rows: &[Row]) {
        if rows.is_empty() {
            self.selected = 0;
            return;
        }
        let at = self.selected.min(rows.len() - 1);
        self.selected = item_from(rows, at, 1)
            .or_else(|| item_from(rows, at, -1))
            .unwrap_or(0);
    }

    /// Moves `delta` items (headers are skipped).
    pub fn move_by(&mut self, delta: isize, rows: &[Row]) {
        self.fix(rows);
        let step = delta.signum();
        for _ in 0..delta.unsigned_abs() {
            let next = (self.selected as isize + step).max(0) as usize;
            match item_from(rows, next, step) {
                Some(index) if index != self.selected => self.selected = index,
                _ => break,
            }
        }
    }

    pub fn top(&mut self, rows: &[Row]) {
        self.selected = item_from(rows, 0, 1).unwrap_or(0);
    }

    pub fn bottom(&mut self, rows: &[Row]) {
        self.selected = item_from(rows, rows.len().saturating_sub(1), -1).unwrap_or(0);
    }

    /// The first item of the next (or previous) group.
    pub fn jump_group(&mut self, forward: bool, rows: &[Row]) {
        let headers: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.is_item())
            .map(|(i, _)| i)
            .collect();
        let target = if forward {
            headers.iter().find(|&&h| h > self.selected)
        } else {
            // The header of the current group, then the one before it.
            let current = headers.iter().rev().find(|&&h| h < self.selected);
            current.and_then(|c| headers.iter().rev().find(|&&h| h < *c))
        };
        // No group in that direction (or a flat list): go to the end, the
        // same way in both directions.
        match target.and_then(|&h| item_from(rows, h, 1)) {
            Some(index) => self.selected = index,
            None if forward => self.bottom(rows),
            None => self.top(rows),
        }
    }

    /// Items that fit in `height`, for paging.
    pub fn page(height: u16) -> usize {
        usize::from((height / ITEM_HEIGHT).max(1))
    }

    /// Scrolls so the selection (and its group header, if just above) shows.
    fn scroll_into_view(&mut self, rows: &[Row], height: u16) {
        let anchor =
            if self.selected > 0 && rows.get(self.selected - 1).is_some_and(|r| !r.is_item()) {
                self.selected - 1
            } else {
                self.selected
            };
        if anchor < self.offset {
            self.offset = anchor;
        }
        loop {
            let used: u16 = rows
                .iter()
                .skip(self.offset)
                .take(self.selected + 1 - self.offset)
                .map(Row::height)
                .sum();
            if used <= height || self.offset >= self.selected {
                break;
            }
            self.offset += 1;
        }
    }
}

/// Props for drawing a list.
pub struct ItemList<'a> {
    pub rows: &'a [Row],
    pub theme: &'a Theme,
    /// When the list appeared; rows slide in from it. `None` draws them in place.
    pub appeared: Option<anim::Moment>,
    /// Where the highlight is drawn, as a (fractional) row index; lets it
    /// glide between rows. `None` draws it on the selected row.
    pub cursor: Option<f32>,
}

impl ItemList<'_> {
    pub fn render(&self, frame: &mut Frame, area: Rect, state: &mut ListState) {
        state.fix(self.rows);
        state.scroll_into_view(self.rows, area.height);

        // Row tops relative to the list, from the scroll offset on.
        let mut tops = Vec::new();
        let mut y = 0u16;
        for row in self.rows.iter().skip(state.offset) {
            if y >= area.height {
                break;
            }
            tops.push(y);
            y = y.saturating_add(row.height());
        }

        let buf = frame.buffer_mut();
        self.render_highlight(buf, area, state, &tops);
        for (slot, (row, top)) in self.rows.iter().skip(state.offset).zip(&tops).enumerate() {
            let y = area.y + top;
            let row_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: row.height().min(2).min(area.bottom().saturating_sub(y)),
            };
            let progress = self.appeared.map_or(1.0, |at| {
                anim::ease_out(at.progress(STAGGER * slot as u32, SLIDE))
            });
            self.render_row(buf, row_area, row, progress);
        }
    }

    /// The selection background and bar, possibly between two rows.
    fn render_highlight(&self, buf: &mut Buffer, area: Rect, state: &ListState, tops: &[u16]) {
        let Some(Row::Item { accent, .. }) = self.rows.get(state.selected) else {
            return;
        };
        let top_of = |index: f32| -> Option<f32> {
            let slot = index.floor() as isize - state.offset as isize;
            let base = tops.get(usize::try_from(slot).ok()?)?;
            let next = tops
                .get(usize::try_from(slot + 1).ok()?)
                .copied()
                .unwrap_or(*base);
            Some(f32::from(*base) + (f32::from(next) - f32::from(*base)) * index.fract())
        };
        let index = self.cursor.unwrap_or(state.selected as f32);
        let top = top_of(index)
            .or_else(|| top_of(state.selected as f32))
            .unwrap_or(0.0);
        let y = area.y + top.round() as u16;
        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 2.min(area.bottom().saturating_sub(y)),
        };
        buf.set_style(rect, Style::new().bg(self.theme.selection));
        for y in rect.y..rect.bottom() {
            if let Some(cell) = buf.cell_mut((rect.x, y)) {
                cell.set_symbol("▌").set_fg(*accent);
            }
        }
    }

    fn render_row(&self, buf: &mut Buffer, area: Rect, row: &Row, progress: f32) {
        let shift = ((1.0 - progress) * SLIDE_COLUMNS).round() as u16;
        let (lines, dimmed, indent) = match row {
            Row::Item {
                top,
                bottom,
                dimmed,
                ..
            } => (vec![top.clone(), bottom.clone()], *dimmed, 2),
            Row::Header(line) => (vec![line.clone()], false, 0),
        };
        let text = Rect {
            x: area.x + indent + shift,
            width: area.width.saturating_sub(indent + 1 + shift),
            ..area
        };
        let mut paragraph = Paragraph::new(lines);
        if progress < 1.0 || dimmed {
            paragraph = paragraph.style(Style::new().add_modifier(Modifier::DIM));
        }
        paragraph.render(text, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(spec: &str) -> Vec<Row> {
        spec.chars()
            .map(|c| match c {
                'h' => Row::Header(Line::raw("h")),
                _ => Row::item(Line::raw("t"), Line::raw("b"), Color::Reset),
            })
            .collect()
    }

    #[test]
    fn selection_skips_headers() {
        let rows = rows("hiihii");
        let mut state = ListState::default();
        state.fix(&rows);
        assert_eq!(state.selected, 1);
        state.move_by(1, &rows);
        assert_eq!(state.selected, 2);
        state.move_by(1, &rows);
        assert_eq!(state.selected, 4, "header at 3 is skipped");
        state.move_by(10, &rows);
        assert_eq!(state.selected, 5);
        state.move_by(-10, &rows);
        assert_eq!(state.selected, 1);
        state.bottom(&rows);
        assert_eq!(state.selected, 5);
    }

    #[test]
    fn group_jumps() {
        let rows = rows("hiihiihi");
        let mut state = ListState::default();
        state.top(&rows);
        state.jump_group(true, &rows);
        assert_eq!(state.selected, 4);
        state.jump_group(true, &rows);
        assert_eq!(state.selected, 7);
        state.jump_group(true, &rows);
        assert_eq!(state.selected, 7, "stays on the last group");
        state.jump_group(false, &rows);
        assert_eq!(state.selected, 4);
        state.jump_group(false, &rows);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn group_jumps_in_a_flat_list_go_to_the_ends() {
        let rows = rows("iiiii");
        let mut state = ListState::default();
        state.move_by(2, &rows);
        state.jump_group(true, &rows);
        assert_eq!(state.selected, 4);
        state.jump_group(false, &rows);
        assert_eq!(state.selected, 0);

        // Grouped: J from the last group lands on the last item.
        let rows = super::tests::rows("hiihiii");
        let mut state = ListState::default();
        state.top(&rows);
        state.jump_group(true, &rows);
        assert_eq!(state.selected, 4);
        state.jump_group(true, &rows);
        assert_eq!(state.selected, 6);
    }

    #[test]
    fn scrolls_to_keep_selection_and_its_header_visible() {
        let rows = rows("hiiiiiiiii");
        let mut state = ListState {
            selected: 9,
            ..ListState::default()
        };
        state.scroll_into_view(&rows, 9);
        assert!(state.offset > 0);
        state.top(&rows);
        state.scroll_into_view(&rows, 9);
        assert_eq!(state.offset, 0, "the header above the first item shows");
        state.move_by(1, &[]);
        assert_eq!(state.selected, 0);
    }
}
