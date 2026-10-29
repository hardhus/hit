//! Tabs inside the shell. A tab draws into the regions it is given and
//! never decides the layout (audit D-036).

mod focus;
mod inbox;
mod placeholder;
mod pulls;

use hit_core::{AccountId, ItemRef};
use ratatui::{Frame, layout::Rect, text::Span};

pub use focus::{FocusTab, pr_fields, preview_lines, spinner};
pub use inbox::{InboxTab, notification_lines};
pub use placeholder::Placeholder;
pub use pulls::{PullsTab, pr_icon, pr_status_spans};

use crate::{Action, Ctx, Event, keymap::Command};

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

    /// The tab is no longer visible.
    fn hidden(&mut self, ctx: &Ctx) {
        let _ = ctx;
    }
}
