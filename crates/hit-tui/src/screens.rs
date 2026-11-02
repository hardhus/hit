//! Tabs inside the shell. A tab draws into the regions it is given and
//! never decides the layout (audit D-036).

mod focus;
mod inbox;
mod issues;
mod pulls;
mod repos;

use std::collections::HashMap;

use hit_core::{AccountId, ItemRef, focus::FocusReason};
use ratatui::{Frame, layout::Rect, text::Span};

pub use focus::{FocusTab, issue_fields, pr_fields, spinner};
pub use inbox::{InboxTab, notification_lines};
pub use issues::IssuesTab;
pub use pulls::{PullsTab, pr_icon, pr_status_spans};
pub use repos::ReposTab;

use crate::{Action, Ctx, DataEvent, Event, keymap::Command};

/// What Action Required says about each item, for the repository views: the
/// forge's searches know things an item's own fields do not (mentions, team
/// reviews), and asking twice would only cost requests.
#[derive(Default)]
pub struct FocusAsks(HashMap<AccountId, HashMap<ItemRef, Vec<FocusReason>>>);

impl FocusAsks {
    pub fn update(&mut self, action: &Action, ctx: &Ctx) {
        if let Action::Data(DataEvent::Focus {
            account,
            scope,
            result: Ok(loaded),
        }) = action
            && *scope == ctx.scope
        {
            self.0.insert(
                account.clone(),
                loaded
                    .value
                    .iter()
                    .map(|item| (item.subject.reference().clone(), item.reasons.clone()))
                    .collect(),
            );
        }
    }

    pub fn reasons(&self, account: &AccountId, item: &ItemRef) -> &[FocusReason] {
        self.0
            .get(account)
            .and_then(|items| items.get(item))
            .map_or(&[], Vec::as_slice)
    }
}

/// Where a tab may draw.
#[derive(Debug, Clone, Copy)]
pub struct TabArea {
    pub list: Rect,
    pub preview: Option<Rect>,
}

/// Something the palette can jump to.
#[derive(Debug, Clone)]
pub struct JumpTarget {
    pub account: AccountId,
    pub item: ItemRef,
    pub title: String,
}

pub trait Tab {
    fn title(&self) -> &'static str;

    /// Shown next to the title in the tab bar.
    fn count(&self, ctx: &Ctx) -> Option<usize> {
        let _ = ctx;
        None
    }

    /// Start (or restart) loading. Called when the shell mounts and on refresh.
    fn mount(&mut self, ctx: &Ctx) {
        let _ = ctx;
    }

    /// A key command while this tab is active. `None` lets the shell handle it.
    fn command(&mut self, command: Command, ctx: &Ctx) -> Option<Action> {
        let _ = (command, ctx);
        None
    }

    /// Whether the tab is taking raw input (e.g. typing a filter).
    fn captures_input(&self) -> bool {
        false
    }

    /// Raw input while [`Self::captures_input`] is true.
    fn input(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let _ = (event, ctx);
        None
    }

    fn update(&mut self, action: &Action, ctx: &Ctx) {
        let _ = (action, ctx);
    }

    /// The app switched to another repository or back to all of them: what
    /// is loaded no longer applies.
    fn scope_changed(&mut self, ctx: &Ctx) {
        let _ = ctx;
    }

    fn render(&mut self, frame: &mut Frame, area: TabArea, ctx: &Ctx);

    /// Left part of the status line (freshness, counts).
    fn status(&self, ctx: &Ctx) -> Vec<Span<'static>> {
        let _ = ctx;
        Vec::new()
    }

    /// Commands worth showing in the hint line, with labels.
    fn hints(&self) -> &'static [(&'static [Command], &'static str)] {
        &[]
    }

    fn jump_targets(&self) -> Vec<JumpTarget> {
        Vec::new()
    }

    /// Select `item` if this tab shows it.
    fn reveal(&mut self, account: &AccountId, item: &ItemRef) -> bool {
        let _ = (account, item);
        false
    }

    fn is_animating(&self) -> bool {
        false
    }

    /// The tab became visible (for entrance animations).
    fn shown(&mut self) {}

    /// The user came to this tab: the time to load what only it needs, or to
    /// catch up if what it has is old.
    fn activated(&mut self, ctx: &Ctx) {
        let _ = ctx;
    }

    /// The tab is no longer visible.
    fn hidden(&mut self, ctx: &Ctx) {
        let _ = ctx;
    }
}
