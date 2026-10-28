//! Terminal UI for hit.
//!
//! The core is intentionally thin so the UX can be reshaped freely:
//! a stack of [`View`]s, an [`Action`] channel and a render loop that only
//! draws when something changed or a view is animating.

mod action;
mod app;
mod error;
mod event;
mod terminal;
pub mod theme;
mod view;
pub mod views;

pub use action::{Action, DataEvent};
pub use app::{App, run};
pub use error::TuiError;
pub use event::Event;
pub use terminal::{install_panic_hook, restore_terminal};
pub use theme::Theme;
pub use view::{Ctx, View};
