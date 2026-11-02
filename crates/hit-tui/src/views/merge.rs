//! Merging a pull request. It cannot be undone, so this is the one action
//! that always asks, and says what stands in the way.

use std::sync::Arc;

use crossterm::event::KeyCode;
use hit_core::{
    ForgeError,
    model::{MergeMethod, MergeOptions},
};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use crate::{
    Action, Ctx, DataEvent, Event, View,
    action::Target,
    anim::Moment,
    keymap::typed_char,
    screens::spinner,
    sync::{self, ItemOp},
    ui::{chips::bold, popup::Popup},
};

pub struct MergeView {
    target: Target,
    options: Option<MergeOptions>,
    method: Option<MergeMethod>,
    delete_branch: bool,
    /// Waiting for the forge; input is ignored so nothing is sent twice.
    merging: bool,
    error: Option<ForgeError>,
    opened: Moment,
}

impl MergeView {
    pub fn new(target: Target) -> Self {
        Self {
            target,
            options: None,
            method: None,
            delete_branch: false,
            merging: false,
            error: None,
            opened: Moment::now(),
        }
    }

    fn step_method(&mut self, forward: bool) {
        let (Some(options), Some(method)) = (&self.options, self.method) else {
            return;
        };
        let count = options.allowed.len();
        let Some(index) = options.allowed.iter().position(|m| *m == method) else {
            return;
        };
        let next = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        self.method = options.allowed.get(next).copied();
    }

    fn merge(&mut self, ctx: &Ctx) {
        let (Some(options), Some(method)) = (&self.options, self.method) else {
            return;
        };
        if options.blocked.is_some() {
            return;
        }
        let Some(forge) = ctx.forges.by_account(&self.target.account) else {
            return;
        };
        let delete_branch =
            (self.delete_branch && options.can_delete_branch).then(|| options.head_ref.clone());
        self.error = None;
        self.merging = true;
        sync::spawn_item_op(
            Arc::clone(forge),
            self.target.clone(),
            ItemOp::Merge {
                method,
                delete_branch,
            },
            ctx.sender(),
        );
    }
}

impl View for MergeView {
    fn mount(&mut self, ctx: &Ctx) {
        if let Some(forge) = ctx.forges.by_account(&self.target.account) {
            sync::spawn_merge_options(Arc::clone(forge), self.target.clone(), ctx.sender());
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let Event::Key(key) = event else {
            return None;
        };
        if self.merging {
            return Some(Action::Redraw);
        }
        match (key.code, typed_char(key)) {
            (KeyCode::Esc, _) => return Some(Action::Back),
            (_, Some('y')) => self.merge(ctx),
            (_, Some('d')) => {
                if self.options.as_ref().is_some_and(|o| o.can_delete_branch) {
                    self.delete_branch = !self.delete_branch;
                }
            }
            (KeyCode::Left, _) | (_, Some('h')) => self.step_method(false),
            (KeyCode::Right, _) | (_, Some('l')) => self.step_method(true),
            _ => {}
        }
        Some(Action::Redraw)
    }

    fn update(&mut self, action: &Action, _ctx: &Ctx) -> Option<Action> {
        let Action::Data(event) = action else {
            return None;
        };
        match event {
            DataEvent::MergeOptions { target, result } if *target == self.target => match result {
                Ok(options) => {
                    self.method = Some(options.default);
                    self.options = Some(options.clone());
                }
                Err(e) => self.error = Some(e.clone()),
            },
            DataEvent::ActionDone { target, result, .. }
                if *target == self.target && self.merging =>
            {
                self.merging = false;
                match result {
                    Ok(()) => return Some(Action::Back),
                    Err(e) => self.error = Some(e.clone()),
                }
            }
            _ => {}
        }
        None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let mut lines = Vec::new();
        match &self.options {
            None if self.error.is_none() => lines.push(Line::from(vec![
                Span::styled(spinner(ctx), bold(t.accent)),
                Span::styled(" checking what stands in the way…", muted),
            ])),
            None => {}
            Some(options) => {
                lines.push(Line::from(vec![
                    Span::styled(options.head_ref.clone(), bold(t.fg)),
                    Span::styled(" → ", muted),
                    Span::styled(options.base_ref.clone(), bold(t.fg)),
                ]));
                lines.push(Line::raw(""));
                let mut methods = Vec::new();
                for method in &options.allowed {
                    let chosen = Some(*method) == self.method;
                    methods.push(Span::styled(
                        format!("{} {}   ", if chosen { "◆" } else { "◇" }, method.label()),
                        if chosen { bold(t.accent) } else { muted },
                    ));
                }
                lines.push(Line::from(methods));
                if options.can_delete_branch {
                    lines.push(Line::from(Span::styled(
                        format!(
                            "{} delete the branch afterwards",
                            if self.delete_branch { "◆" } else { "◇" }
                        ),
                        if self.delete_branch {
                            bold(t.accent)
                        } else {
                            muted
                        },
                    )));
                }
                if let Some(blocked) = &options.blocked {
                    lines.push(Line::raw(""));
                    lines.push(Line::styled(
                        format!("✗ can't merge: {blocked}"),
                        bold(t.danger),
                    ));
                }
                if let Some(warning) = &options.warning {
                    lines.push(Line::raw(""));
                    lines.push(Line::styled(format!("! {warning}"), bold(t.warning)));
                }
            }
        }
        if let Some(error) = &self.error {
            lines.push(Line::raw(""));
            lines.push(Line::styled(error.to_string(), bold(t.danger)));
        }

        let can_merge = self
            .options
            .as_ref()
            .is_some_and(|options| options.blocked.is_none());
        let footer = if self.merging {
            "merging…"
        } else if can_merge {
            "y merge · h/l method · d branch · esc cancel"
        } else {
            "esc close"
        };
        let title = format!("merge {}", self.target.item);
        let inner = Popup {
            title: &title,
            footer: Some(footer),
            width: 68,
            height: u16::try_from(lines.len()).unwrap_or(4).saturating_add(4),
            border: t.warning,
            theme: t,
            opened: ctx.animations.then_some(self.opened),
        }
        .render(frame, area);
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            Rect {
                x: inner.x + 1,
                y: inner.y + 1,
                width: inner.width.saturating_sub(2),
                height: inner.height.saturating_sub(1),
            },
        );
    }

    fn is_animating(&self) -> bool {
        Popup::is_animating(Some(self.opened)) || (self.options.is_none() && self.error.is_none())
    }

    fn is_overlay(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEvent, KeyModifiers};
    use hit_core::{Forge, Forges, ItemRef, RepoRef, model::PrState};
    use hit_forge_mock::{DEMO_HOST, MockForge, MockOptions};
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::Services;

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    async fn setup(
        repo: (&str, &str),
        number: u64,
    ) -> (MergeView, Ctx, UnboundedReceiver<Action>, Arc<MockForge>) {
        let forge = Arc::new(MockForge::demo(MockOptions::instant()));
        let mut forges = Forges::new();
        forges.add(Arc::clone(&forge) as Arc<dyn Forge>);
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let repo = RepoRef::new(hit_core::Host::new(DEMO_HOST), repo.0, repo.1);
        let mut view = MergeView::new(Target {
            account: forge.account().clone(),
            item: ItemRef::pull_request(repo, number),
        });
        view.mount(&ctx);
        while view.options.is_none() {
            let action = rx.recv().await.unwrap();
            view.update(&action, &ctx);
        }
        (view, ctx, rx, forge)
    }

    async fn state(forge: &MockForge, view: &MergeView) -> PrState {
        forge
            .pull_requests()
            .unwrap()
            .get(&view.target.item)
            .await
            .unwrap()
            .pr
            .state
    }

    #[tokio::test]
    async fn nothing_is_merged_until_y() {
        let (mut view, ctx, mut rx, forge) = setup(("you", "dotfiles"), 12).await;
        view.handle_event(&key('l'), &ctx);
        view.handle_event(&key('d'), &ctx);
        view.handle_event(&key('z'), &ctx);
        assert_eq!(state(&forge, &view).await, PrState::Open);

        view.handle_event(&key('y'), &ctx);
        assert!(view.merging);
        loop {
            let action = rx.recv().await.unwrap();
            if let Some(next) = view.update(&action, &ctx) {
                assert!(matches!(next, Action::Back));
                break;
            }
        }
        assert_eq!(state(&forge, &view).await, PrState::Merged);
    }

    #[tokio::test]
    async fn what_stands_in_the_way_is_said_and_y_does_nothing() {
        // Marked work in progress in the demo.
        let (mut view, ctx, _rx, forge) = setup(("acme", "rocket"), 139).await;
        assert!(view.options.as_ref().unwrap().blocked.is_some());
        view.handle_event(&key('y'), &ctx);
        assert!(!view.merging);
        assert_eq!(state(&forge, &view).await, PrState::Open);
    }

    #[tokio::test]
    async fn methods_cycle_through_what_the_repository_allows() {
        let (mut view, ctx, _rx, _forge) = setup(("you", "dotfiles"), 12).await;
        let allowed = view.options.as_ref().unwrap().allowed.clone();
        let start = view.method.unwrap();
        for _ in 0..allowed.len() {
            view.handle_event(&key('l'), &ctx);
        }
        assert_eq!(view.method, Some(start));
        view.handle_event(&key('h'), &ctx);
        assert_ne!(view.method, Some(start));
    }
}
