//! The bottom hint line: key pills with short labels.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{Theme, keymap::Command, keymap::Keymap};

/// `(command, label)` pairs rendered as ` j/k  move   ↵  open …`. Commands
/// without keys are skipped.
pub fn line(items: &[(&[Command], &'static str)], keymap: &Keymap, theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    for (commands, label) in items {
        let keys: Vec<String> = commands
            .iter()
            .map(|c| keymap.short(*c))
            .filter(|k| !k.is_empty())
            .collect();
        if keys.is_empty() {
            continue;
        }
        spans.push(Span::styled(
            format!(" {} ", keys.join("/")),
            Style::new()
                .fg(theme.selection)
                .bg(theme.accent_alt)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {label}  "),
            Style::new().fg(theme.muted),
        ));
    }
    Line::from(spans)
}
