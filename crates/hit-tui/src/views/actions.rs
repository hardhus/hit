//! The action menu (`x`): what can be done with this item right now.

use crossterm::event::KeyCode;
use hit_core::{
    ItemKind,
    model::{CheckState, CloseReason, Issue, PrState, PullRequest, ReviewVerdict},
};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use crate::{
    Action, Ctx, Event, View,
    action::{Purpose, Target},
    anim::Moment,
    keymap::typed_char,
    sync::ItemOp,
    ui::{chips::bold, popup::Popup},
};

/// What is known about the item when the menu opens. A notification knows
/// none of it; then everything is offered and the forge has the last word.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    pub open: Option<bool>,
    pub merged: bool,
    pub mine: bool,
    pub checks_failing: bool,
}

impl Facts {
    pub fn of_pull(pr: &PullRequest, me: &str) -> Self {
        Self {
            open: Some(pr.is_open()),
            merged: pr.state == PrState::Merged,
            mine: pr.author.login == me,
            checks_failing: pr.checks.is_some_and(|c| c.state == CheckState::Failure),
        }
    }

    pub fn of_issue(issue: &Issue, me: &str) -> Self {
        Self {
            open: Some(issue.is_open()),
            merged: false,
            mine: issue.author.login == me,
            checks_failing: false,
        }
    }
}

/// One thing the menu offers: the key, what it says, what it does.
struct Entry {
    key: char,
    label: &'static str,
    action: Action,
}

pub struct ActionMenu {
    target: Target,
    facts: Facts,
    opened: Moment,
}

impl ActionMenu {
    pub fn new(target: Target, facts: Facts) -> Self {
        Self {
            target,
            facts,
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
        let target = &self.target;
        let facts = self.facts;
        let pull = target.item.kind == ItemKind::PullRequest;
        let open = facts.open != Some(false);
        let closed = facts.open != Some(true) && !facts.merged;
        let compose = |purpose| Action::Compose(target.clone(), purpose);
        let perform = |op| Action::Perform(target.clone(), op);

        let mut entries = vec![Entry {
            key: 'c',
            label: "comment",
            action: compose(Purpose::Comment),
        }];
        if pull && open {
            if !facts.mine {
                entries.push(Entry {
                    key: 'a',
                    label: "approve",
                    action: compose(Purpose::Review(ReviewVerdict::Approve)),
                });
                entries.push(Entry {
                    key: 'r',
                    label: "request changes",
                    action: compose(Purpose::Review(ReviewVerdict::RequestChanges)),
                });
            }
            entries.push(Entry {
                key: 'm',
                label: "merge…",
                action: Action::OpenMerge(target.clone()),
            });
            if facts.checks_failing || facts.open.is_none() {
                entries.push(Entry {
                    key: 't',
                    label: "run failed checks again",
                    action: perform(ItemOp::RerunFailed),
                });
            }
        }
        if open {
            entries.push(Entry {
                key: 'k',
                label: "close",
                action: perform(ItemOp::SetOpen {
                    open: false,
                    reason: (!pull).then_some(CloseReason::Completed),
                }),
            });
            if !pull {
                entries.push(Entry {
                    key: 'n',
                    label: "close as not planned",
                    action: perform(ItemOp::SetOpen {
                        open: false,
                        reason: Some(CloseReason::NotPlanned),
                    }),
                });
            }
        }
        if closed {
            entries.push(Entry {
                key: 'o',
                label: "reopen",
                action: perform(ItemOp::SetOpen {
                    open: true,
                    reason: None,
                }),
            });
        }
        entries
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
                    " Read-only account: hit writes nothing with it. Settings: ,"
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

    fn setup(pull: bool, facts: Facts) -> (ActionMenu, Ctx) {
        let forge = MockForge::demo(MockOptions::instant());
        let account = hit_core::Forge::account(&forge).clone();
        let mut forges = Forges::new();
        forges.add(Arc::new(forge));
        let (ctx, _rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let repo = RepoRef::new(hit_core::Host::new(DEMO_HOST), "acme", "rocket");
        let item = if pull {
            ItemRef::pull_request(repo, 142)
        } else {
            ItemRef::issue(repo, 57)
        };
        (ActionMenu::new(Target { account, item }, facts), ctx)
    }

    fn keys(menu: &ActionMenu, ctx: &Ctx) -> String {
        menu.entries(ctx).iter().map(|e| e.key).collect()
    }

    const OPEN: Facts = Facts {
        open: Some(true),
        merged: false,
        mine: false,
        checks_failing: false,
    };

    #[test]
    fn a_letter_chooses_and_anything_else_closes() {
        let (mut menu, ctx) = setup(true, OPEN);
        assert!(matches!(
            menu.handle_event(&key('c'), &ctx),
            Some(Action::Compose(_, Purpose::Comment))
        ));
        assert!(matches!(
            menu.handle_event(&key('z'), &ctx),
            Some(Action::Back)
        ));
    }

    #[test]
    fn it_offers_what_makes_sense_for_the_item() {
        let of = |pull, facts| {
            let (menu, ctx) = setup(pull, facts);
            keys(&menu, &ctx)
        };
        assert_eq!(of(true, OPEN), "carmk", "someone else's open pull request");
        assert_eq!(
            of(
                true,
                Facts {
                    mine: true,
                    checks_failing: true,
                    ..OPEN
                }
            ),
            "cmtk",
            "your own: no reviewing it, and it needs its checks run"
        );
        assert_eq!(
            of(
                true,
                Facts {
                    open: Some(false),
                    merged: true,
                    ..OPEN
                }
            ),
            "c",
            "merged: nothing left but talking"
        );
        assert_eq!(
            of(
                true,
                Facts {
                    open: Some(false),
                    ..OPEN
                }
            ),
            "co",
            "closed"
        );
        assert_eq!(of(false, OPEN), "ckn", "open issue");
        assert_eq!(
            of(
                false,
                Facts {
                    open: Some(false),
                    ..OPEN
                }
            ),
            "co",
            "closed issue"
        );
        assert_eq!(
            of(true, Facts::default()),
            "carmtko",
            "known nothing: everything, the forge decides"
        );
    }
}
