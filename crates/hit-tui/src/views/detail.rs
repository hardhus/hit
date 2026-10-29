//! A full-screen look at one item. Richer detail (body, comments, diff)
//! arrives with the PR and Issue screens (F6/F7).

use hit_core::{
    AccountId,
    focus::{FocusItem, FocusSubject},
    model::Notification,
};
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Wrap},
};

use crate::{
    Action, Ctx, Event, View,
    action::Toast,
    anim::Moment,
    browser,
    keymap::{Command, Scope},
    screens::{notification_lines, preview_lines},
    ui::popup::Popup,
};

enum Content {
    Focus(FocusItem),
    Notification(Notification),
}

pub struct DetailView {
    account: AccountId,
    content: Content,
    scroll: u16,
    opened: Moment,
}

impl DetailView {
    fn new(account: AccountId, content: Content) -> Self {
        Self {
            account,
            content,
            scroll: 0,
            opened: Moment::now(),
        }
    }

    pub fn focus(account: AccountId, item: FocusItem) -> Self {
        Self::new(account, Content::Focus(item))
    }

    pub fn notification(account: AccountId, notification: Notification) -> Self {
        Self::new(account, Content::Notification(notification))
    }

    fn url(&self) -> String {
        match &self.content {
            Content::Focus(item) => match &item.subject {
                FocusSubject::PullRequest(pr) => pr.url.clone(),
                FocusSubject::Issue(issue) => issue.url.clone(),
            },
            Content::Notification(n) => n.subject.url.clone().unwrap_or_default(),
        }
    }

    fn title(&self) -> String {
        match &self.content {
            Content::Focus(item) => item.subject.reference().to_string(),
            Content::Notification(n) => match &n.subject.item {
                Some(item) => item.to_string(),
                None => n.repo.to_string(),
            },
        }
    }

    fn lines(&self, ctx: &Ctx) -> Vec<Line<'static>> {
        match &self.content {
            Content::Focus(item) => preview_lines(&self.account, item, ctx),
            Content::Notification(n) => notification_lines(&self.account, n, ctx),
        }
    }
}

impl View for DetailView {
    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let Event::Key(key) = event else {
            return None;
        };
        match ctx.keymap.lookup(key, &[Scope::List, Scope::Global])? {
            Command::Down => self.scroll = self.scroll.saturating_add(1),
            Command::Up => self.scroll = self.scroll.saturating_sub(1),
            Command::Top => self.scroll = 0,
            Command::Back | Command::Open => return Some(Action::Back),
            Command::Quit => return Some(Action::Quit),
            Command::Browser => {
                return Some(Action::Toast(match browser::open(&self.url()) {
                    Ok(()) => Toast::success(format!("opened {}", self.title())),
                    Err(toast) => toast,
                }));
            }
            _ => return None,
        }
        Some(Action::Redraw)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let title = self.title();
        let footer = format!(
            "{} scroll · {} browser · {} back",
            ctx.keymap.label(Command::Down),
            ctx.keymap.short(Command::Browser),
            ctx.keymap.short(Command::Back),
        );
        let inner = Popup {
            title: &title,
            footer: Some(&footer),
            width: area.width.saturating_sub(4).min(110),
            height: area.height.saturating_sub(2),
            border: ctx.theme.accent,
            theme: &ctx.theme,
            opened: ctx.animations.then_some(self.opened),
        }
        .render(frame, area);
        let content = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };
        let lines = self.lines(ctx);
        // Never scroll past the end: the last line stays at the bottom.
        let max_scroll = wrapped_height(&lines, content.width).saturating_sub(content.height);
        self.scroll = self.scroll.min(max_scroll);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            content,
        );
    }

    fn is_animating(&self) -> bool {
        Popup::is_animating(Some(self.opened))
    }
}

/// Rows `lines` take when wrapped to `width` (word wrapping can only add
/// rows, so this may undercount by a little on very long words; that only
/// stops scrolling slightly early, never past the end).
fn wrapped_height(lines: &[Line<'_>], width: u16) -> u16 {
    let width = usize::from(width.max(1));
    let rows: usize = lines.iter().map(|l| l.width().max(1).div_ceil(width)).sum();
    u16::try_from(rows).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_height_counts_wrapped_rows() {
        let lines = vec![Line::raw("a".repeat(25)), Line::raw(""), Line::raw("short")];
        assert_eq!(wrapped_height(&lines, 10), 3 + 1 + 1);
    }
}
