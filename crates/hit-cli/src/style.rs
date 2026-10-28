//! Jinx colors for plain CLI output. Disabled when stdout is not a terminal
//! or `NO_COLOR` is set.

use std::{fmt::Display, io::IsTerminal, sync::OnceLock};

use clap::builder::styling::{Effects, RgbColor, Style};

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
}

fn paint(text: impl Display, style: Style) -> String {
    if enabled() {
        format!("{}{text}{}", style.render(), style.render_reset())
    } else {
        text.to_string()
    }
}

pub fn accent(text: impl Display) -> String {
    paint(
        text,
        RgbColor(0x00, 0xB4, 0xFF)
            .on_default()
            .effects(Effects::BOLD),
    )
}

pub fn pink(text: impl Display) -> String {
    paint(
        text,
        RgbColor(0xFF, 0x2E, 0x97)
            .on_default()
            .effects(Effects::BOLD),
    )
}

pub fn ok(text: impl Display) -> String {
    paint(
        text,
        RgbColor(0x7C, 0xFF, 0x4F)
            .on_default()
            .effects(Effects::BOLD),
    )
}

pub fn warn(text: impl Display) -> String {
    paint(
        text,
        RgbColor(0xFF, 0xE1, 0x4D)
            .on_default()
            .effects(Effects::BOLD),
    )
}

pub fn error(text: impl Display) -> String {
    paint(
        text,
        RgbColor(0xFF, 0x4D, 0x6D)
            .on_default()
            .effects(Effects::BOLD),
    )
}

pub fn muted(text: impl Display) -> String {
    paint(text, RgbColor(0x8A, 0x7F, 0xB0).on_default())
}
