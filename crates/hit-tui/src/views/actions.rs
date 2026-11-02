//! The action menu (`x`): what can be done with this item right now.

use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use crate::{
    Action, Ctx, Event, View,
    action::Target,
    anim::Moment,
    keymap::typed_char,
    ui::{chips::bold, popup::Popup},
};

/// One thing the menu offers: the key, what it says, what it does.
struct Entry {
    key: char,
    label: &'static str,
    action: Action,
}

pub struct ActionMenu {
    target: Target,
    opened: Moment,
}

impl ActionMenu {
    pub fn new(target: Target) -> Self {
        Self {
            target,
            opened: Moment::now(),
        }
    }

    fn read_only(&self, ctx: &Ctx) -> bool {
        ctx.forges
            .by_account(&self.target.account)
            .is_some_and(|forge| forge.is_read_only())
    }

    fn entries(&self, ctx: &Ctx) -> Vec<Entry> {
        let can_act = ctx
            .forges
            .by_account(&self.target.account)
            .is_some_and(|forge| forge.actions().is_some());
        if !can_act || self.read_only(ctx) {
            return Vec::new();
        }
        vec![Entry {
            key: 'c',
            label: "comment",
            action: Action::Compose(self.target.clone()),
        }]
    }
}

impl View for ActionMenu {
    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let Event::Key(key) = event else {
            return None;
        };
        if key.code == KeyCode::Esc {
            return Some(Action::Back);
        }
        let pressed = typed_char(key);
        let chosen = self
            .entries(ctx)
            .into_iter()
            .find(|entry| Some(entry.key) == pressed);
        Some(match chosen {
            Some(entry) => entry.action,
            // Any other key closes it, so a stray press does nothing.
            None => Action::Back,
        })
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let entries = self.entries(ctx);
        let mut lines: Vec<Line> = entries
            .iter()
            .map(|entry| {
                Line::from(vec![
                    Span::styled(format!(" {}  ", entry.key), bold(t.accent)),
                    Span::styled(entry.label, Style::new().fg(t.fg)),
                ])
            })
            .collect();
        if lines.is_empty() {
            lines.push(Line::styled(
                if self.read_only(ctx) {
                    " This account is read-only: hit writes nothing with it. , opens settings."
                } else {
                    " Nothing can be done with this here."
                },
                Style::new().fg(t.muted),
            ));
        }
        let title = self.target.item.to_string();
        let height = u16::try_from(lines.len()).unwrap_or(1).saturating_add(2);
        let wide = entries.is_empty();
        let inner = Popup {
            title: &title,
            footer: Some("esc close"),
            width: if wide { 60 } else { 36 },
            height: if wide { height + 1 } else { height },
            border: t.accent_alt,
            theme: t,
            opened: ctx.animations.then_some(self.opened),
        }
        .render(frame, area);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn is_animating(&self) -> bool {
        Popup::is_animating(Some(self.opened))
    }

    fn is_overlay(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crossterm::event::{KeyEvent, KeyModifiers};
    use hit_core::{Forges, ItemRef, RepoRef};
    use hit_forge_mock::{DEMO_HOST, MockForge, MockOptions};

    use super::*;
    use crate::Services;

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn setup() -> (ActionMenu, Ctx) {
        let forge = MockForge::demo(MockOptions::instant());
        let account = hit_core::Forge::account(&forge).clone();
        let mut forges = Forges::new();
        forges.add(Arc::new(forge));
        let (ctx, _rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let repo = RepoRef::new(hit_core::Host::new(DEMO_HOST), "acme", "rocket");
        let target = Target {
            account,
            item: ItemRef::pull_request(repo, 142),
        };
        (ActionMenu::new(target), ctx)
    }

    #[test]
    fn a_letter_chooses_and_anything_else_closes() {
        let (mut menu, ctx) = setup();
        assert!(matches!(
            menu.handle_event(&key('c'), &ctx),
            Some(Action::Compose(_))
        ));
        assert!(matches!(
            menu.handle_event(&key('z'), &ctx),
            Some(Action::Back)
        ));
    }
}
