use std::path::PathBuf;

use clap::{
    Parser, Subcommand, ValueEnum,
    builder::styling::{Effects, RgbColor, Styles},
};
use hit_config::LogLevel;

const ABOUT: &str = "GitHub from your terminal, focus first.";

const LONG_ABOUT: &str = "\
hit is a keyboard-driven terminal UI for GitHub.

Open it first thing in the morning: it shows what needs you right now:
review requests, your pull requests with failing checks or requested
changes, mentions and assigned issues. Then the rest of your inbox.

Run `hit` with no arguments to open the TUI.";

const AFTER_HELP: &str = "\
Examples:
  hit                       open the TUI
  hit --demo                try hit with sample data, no account needed
  hit paths                 show config, data, state, cache and log directories
  hit config init           write a commented default config.toml
  hit config check          validate config.toml and list warnings
  hit completions zsh       print shell completions

Environment:
  HIT_LOG                   log filter (e.g. debug, hit_tui=trace)
  HIT_CONFIG                config file to use instead of the default
  HIT_DEMO                  same as --demo
  HIT_CONFIG_DIR, HIT_DATA_DIR, HIT_STATE_DIR, HIT_CACHE_DIR
                            override individual directories";

#[derive(Debug, Parser)]
#[command(
    name = "hit",
    version,
    about = ABOUT,
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP,
    styles = styles(),
)]
pub struct Cli {
    /// Config file to use instead of the default location.
    #[arg(long, global = true, value_name = "FILE", env = "HIT_CONFIG")]
    pub config: Option<PathBuf>,

    /// Log level for this run (overrides the config; HIT_LOG overrides both).
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log_level: Option<LevelArg>,

    /// Explore hit with realistic sample data; no account needed and nothing
    /// you do leaves your machine.
    #[arg(long, env = "HIT_DEMO")]
    pub demo: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect and manage config.toml.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Show the directories hit uses.
    Paths,
    /// Print shell completions to stdout.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the config file path.
    Path,
    /// Validate the config file; exits with 1 if there are warnings.
    Check,
    /// Write a commented default config file.
    Init {
        /// Overwrite an existing config file.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LevelArg {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<LevelArg> for LogLevel {
    fn from(level: LevelArg) -> Self {
        match level {
            LevelArg::Off => Self::Off,
            LevelArg::Error => Self::Error,
            LevelArg::Warn => Self::Warn,
            LevelArg::Info => Self::Info,
            LevelArg::Debug => Self::Debug,
            LevelArg::Trace => Self::Trace,
        }
    }
}

/// Help colors from the Jinx palette.
fn styles() -> Styles {
    let pink = RgbColor(0xFF, 0x2E, 0x97);
    let blue = RgbColor(0x00, 0xB4, 0xFF);
    let cyan = RgbColor(0x2D, 0xE2, 0xE6);
    let yellow = RgbColor(0xFF, 0xE1, 0x4D);
    let red = RgbColor(0xFF, 0x4D, 0x6D);
    let purple = RgbColor(0x9D, 0x4E, 0xDD);
    Styles::styled()
        .header(pink.on_default().effects(Effects::BOLD))
        .usage(pink.on_default().effects(Effects::BOLD))
        .literal(blue.on_default().effects(Effects::BOLD))
        .placeholder(cyan.on_default())
        .valid(cyan.on_default())
        .invalid(yellow.on_default().effects(Effects::BOLD))
        .error(red.on_default().effects(Effects::BOLD))
        .context(purple.on_default())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }
}
