//! Terminal UI for hit.
//!
//! The core is intentionally thin so the UX can be reshaped freely:
//! a stack of [`View`]s, an [`Action`] channel and a render loop that only
//! draws when something changed or a view is animating.

mod action;
mod anim;
mod app;
mod browser;
mod error;
mod event;
pub mod keymap;
pub mod layout;
mod screens;
mod shell;
mod sync;
mod terminal;
pub mod theme;
mod ui;
mod view;
pub mod views;

pub use action::{Action, DataEvent, Toast};
pub use app::{App, run};
pub use error::TuiError;
pub use event::Event;
pub use sync::Loaded;
pub use terminal::{install_panic_hook, restore_terminal};
pub use theme::Theme;
pub use view::{Ctx, Services, View};
