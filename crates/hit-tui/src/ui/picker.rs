//! A popup to pick several things from a list (labels, later authors or
//! repos): type to search, ↑/↓ to move, ↵ to pick or unpick.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    Theme,
    anim::Moment,
    keymap::typed_char,
    ui::{chips::bold, popup::Popup},
};

/// One choice. `key` identifies it, `label` is how it looks.
pub struct PickerItem {
    pub key: String,
    pub label: Span<'static>,
    pub count: usize,
    pub picked: bool,
}

/// What a key press in the picker means for its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerEvent {
    Toggle(String),
    ClearAll,
    Close,
    /// Moved or typed; just redraw.
    Changed,
    Ignored,
}

/// Open picker state. The owner keeps the items and what is picked.
#[derive(Debug, Clone)]
pub struct Picker {
    pub query: String,
    cursor: usize,
    opened: Moment,
}

impl Default for Picker {
    fn default() -> Self {
        Self::new()
    }
}

impl Picker {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            cursor: 0,
            opened: Moment::now(),
        }
    }

    /// The items matching the search, in the given order.
    pub fn matching<'a>(&self, items: &'a [PickerItem]) -> Vec<&'a PickerItem> {
        let needle = self.query.to_lowercase();
        items
            .iter()
            .filter(|i| needle.is_empty() || i.key.to_lowercase().contains(&needle))
            .collect()
    }

    pub fn input(&mut self, key: &KeyEvent, items: &[PickerItem]) -> PickerEvent {
        let len = self.matching(items).len();
        match key.code {
            KeyCode::Esc if !self.query.is_empty() => self.query.clear(),
            KeyCode::Esc => return PickerEvent::Close,
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down => self.cursor = (self.cursor + 1).min(len.saturating_sub(1)),
            KeyCode::Enter => {
                return self
                    .matching(items)
                    .get(self.cursor)
                    .map_or(PickerEvent::Ignored, |i| PickerEvent::Toggle(i.key.clone()));
            }
            KeyCode::Delete => return PickerEvent::ClearAll,
            KeyCode::Backspace => {
                self.query.pop();
            }
            _ => match typed_char(key) {
                Some(c) => {
                    self.query.push(c);
                    self.cursor = 0;
                }
                None => return PickerEvent::Ignored,
            },
        }
        self.cursor = self
            .cursor
            .min(self.matching(items).len().saturating_sub(1));
        PickerEvent::Changed
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        items: &[PickerItem],
        theme: &Theme,
        animations: bool,
    ) {
        let t = theme;
        let muted = Style::new().fg(t.muted);
        let matching = self.matching(items);
        let rows = (matching.len().max(1) as u16).min(14);
        let inner = Popup {
            title,
            footer: Some("type to search · ↑↓ move · ↵ pick · del clear all · esc done"),
            width: 64,
            height: rows + 4,
            border: t.accent_alt,
            theme: t,
            opened: animations.then_some(self.opened),
        }
        .render(frame, area);
        let inner = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };

        let mut lines = vec![
            Line::from(vec![
                Span::styled("⌕ ", bold(t.accent_alt)),
                Span::styled(self.query.clone(), Style::new().fg(t.fg)),
                Span::styled("▏", bold(t.accent_alt)),
            ]),
            Line::raw(""),
        ];
        if matching.is_empty() {
            lines.push(Line::styled("nothing matches", muted));
        }
        let visible = usize::from(inner.height.saturating_sub(2)).max(1);
        let first = self.cursor.saturating_sub(visible - 1);
        for (index, item) in matching.iter().enumerate().skip(first).take(visible) {
            let current = index == self.cursor;
            let mut line = Line::from(vec![
                Span::styled(if current { "▌" } else { " " }, bold(t.accent_alt)),
                Span::styled(
                    if item.picked { "☑ " } else { "☐ " },
                    if item.picked { bold(t.success) } else { muted },
                ),
                item.label.clone(),
                Span::styled(format!("  {}", item.count), muted),
            ]);
            if current {
                line = line.style(Style::new().bg(t.selection));
            }
            lines.push(line);
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }

    pub fn is_animating(&self) -> bool {
        Popup::is_animating(Some(self.opened))
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;

    fn items() -> Vec<PickerItem> {
        ["bug", "docs", "good first issue"]
            .into_iter()
            .map(|k| PickerItem {
                key: k.to_owned(),
                label: Span::raw(k),
                count: 1,
                picked: false,
            })
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn search_move_and_pick() {
        let items = items();
        let mut picker = Picker::new();
        assert_eq!(
            picker.input(&key(KeyCode::Down), &items),
            PickerEvent::Changed
        );
        assert_eq!(
            picker.input(&key(KeyCode::Enter), &items),
            PickerEvent::Toggle("docs".into())
        );
        for c in "good".chars() {
            picker.input(&key(KeyCode::Char(c)), &items);
        }
        assert_eq!(picker.matching(&items).len(), 1);
        assert_eq!(
            picker.input(&key(KeyCode::Enter), &items),
            PickerEvent::Toggle("good first issue".into())
        );
        // Esc clears the search first, then closes.
        assert_eq!(
            picker.input(&key(KeyCode::Esc), &items),
            PickerEvent::Changed
        );
        assert_eq!(picker.input(&key(KeyCode::Esc), &items), PickerEvent::Close);
        assert_eq!(
            picker.input(&key(KeyCode::Delete), &items),
            PickerEvent::ClearAll
        );
    }
}
