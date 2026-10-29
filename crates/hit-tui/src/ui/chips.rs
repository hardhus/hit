//! Small colored spans: reasons, labels, accounts, states.

use hit_core::{
    AccountId,
    focus::{FocusReason, FocusSubject},
    model::{CheckState, CheckSummary, Label, PrState},
};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

use crate::{Ctx, Theme};

pub fn bold(color: Color) -> Style {
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

/// Icon, text and color for a focus reason.
pub fn reason(reason: FocusReason, theme: &Theme) -> (&'static str, &'static str, Color) {
    match reason {
        FocusReason::ReviewRequested => ("●", "review requested", theme.accent),
        FocusReason::ChecksFailing => ("✗", "CI failing", theme.danger),
        FocusReason::ChangesRequested => ("◐", "changes requested", theme.warning),
        FocusReason::ReadyToMerge => ("✓", "ready to merge", theme.success),
        FocusReason::Mentioned => ("@", "mentioned", theme.info),
        FocusReason::Assigned => ("◆", "assigned", theme.highlight),
    }
}

pub fn reason_span(r: FocusReason, theme: &Theme) -> Span<'static> {
    let (icon, text, color) = reason(r, theme);
    Span::styled(format!("{icon} {text}"), bold(color))
}

/// The most important reason's color, used for the row accent bar.
pub fn reasons_color(reasons: &[FocusReason], theme: &Theme) -> Color {
    reasons
        .first()
        .map_or(theme.accent, |r| reason(*r, theme).2)
}

/// A filled label with readable text on top of the label's own color.
pub fn label_span(label: &Label, theme: &Theme) -> Span<'static> {
    match label.color {
        Some(rgb) => {
            let luminance =
                0.2126 * f32::from(rgb.0) + 0.7152 * f32::from(rgb.1) + 0.0722 * f32::from(rgb.2);
            let text = if luminance > 140.0 {
                Color::Rgb(0x14, 0x0f, 0x24)
            } else {
                Color::Rgb(0xff, 0xff, 0xff)
            };
            Span::styled(
                format!(" {} ", label.name),
                Style::new().fg(text).bg(Color::Rgb(rgb.0, rgb.1, rgb.2)),
            )
        }
        _ => Span::styled(format!(" {} ", label.name), Style::new().fg(theme.muted)),
    }
}

/// `▍` in the account's color, shown when several accounts are connected.
pub fn account_badge(account: &AccountId, ctx: &Ctx) -> Option<Span<'static>> {
    ctx.multi_account()
        .then(|| Span::styled("▍", Style::new().fg(ctx.account_color(account))))
}

/// Icon for a pull request (by state) or issue.
pub fn subject_icon(subject: &FocusSubject, theme: &Theme) -> Span<'static> {
    match subject {
        FocusSubject::PullRequest(pr) => {
            let color = match pr.state {
                PrState::Open if pr.is_draft() => theme.muted,
                PrState::Open => theme.success,
                PrState::Merged => theme.highlight,
                PrState::Closed => theme.danger,
            };
            Span::styled("◈", bold(color))
        }
        FocusSubject::Issue(issue) => {
            let color = if issue.is_open() {
                theme.success
            } else {
                theme.highlight
            };
            Span::styled("○", bold(color))
        }
    }
}

/// `✓ 3  ✗ 1  ◐ 2` check counts.
pub fn checks_spans(checks: &CheckSummary, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut push = |n: u32, icon: &str, color: Color| {
        if n > 0 {
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(format!("{icon} {n}"), bold(color)));
        }
    };
    push(checks.passed, "✓", theme.success);
    push(checks.failed, "✗", theme.danger);
    push(checks.pending, "◐", theme.warning);
    if spans.is_empty() {
        let (icon, color) = match checks.state {
            CheckState::Success => ("✓ passing", theme.success),
            CheckState::Failure => ("✗ failing", theme.danger),
            CheckState::Pending => ("◐ running", theme.warning),
            CheckState::Neutral => ("· no result", theme.muted),
        };
        spans.push(Span::styled(icon, bold(color)));
    }
    spans
}

#[cfg(test)]
mod tests {
    use hit_core::model::Rgb;

    use super::*;

    #[test]
    fn label_text_is_readable() {
        let theme = Theme::jinx(true, true);
        let light = label_span(&Label::new("docs", Some(Rgb(0xff, 0xe1, 0x4d))), &theme);
        let dark = label_span(&Label::new("bug", Some(Rgb(0x20, 0x10, 0x40))), &theme);
        assert_eq!(light.style.fg, Some(Color::Rgb(0x14, 0x0f, 0x24)));
        assert_eq!(dark.style.fg, Some(Color::Rgb(0xff, 0xff, 0xff)));
    }
}
