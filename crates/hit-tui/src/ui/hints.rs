//! The bottom hint line: key pills with short labels.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{Theme, keymap::Command, keymap::Keymap};

/// `(command, label)` pairs rendered as ` j/k  move   ↵  open …`. Commands
/// without keys are skipped. What does not fit in `width` is left out from the
/// end, except the last item, which tells the way to everything else.
pub fn line(
    items: &[(&[Command], &'static str)],
    keymap: &Keymap,
    theme: &Theme,
    width: u16,
) -> Line<'static> {
    let mut hints: Vec<[Span<'static>; 2]> = Vec::new();
    for (commands, label) in items {
        let keys: Vec<String> = commands
            .iter()
            .map(|c| keymap.short(*c))
            .filter(|k| !k.is_empty())
            .collect();
        if keys.is_empty() {
            continue;
        }
        hints.push([
            Span::styled(
                format!(" {} ", keys.join("/")),
                Style::new()
                    .fg(theme.selection)
                    .bg(theme.accent_alt)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {label}  "), Style::new().fg(theme.muted)),
        ]);
    }
    let size = |hint: &[Span<'static>; 2]| hint[0].width() + hint[1].width();
    let last = hints.pop();
    let mut room = usize::from(width).saturating_sub(last.as_ref().map_or(0, size));
    let mut spans = Vec::new();
    for hint in hints {
        if size(&hint) > room {
            break;
        }
        room -= size(&hint);
        spans.extend(hint);
    }
    spans.extend(last.into_iter().flatten());
    Line::from(spans)
}
