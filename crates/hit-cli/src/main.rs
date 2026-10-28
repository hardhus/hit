mod cli;
mod commands;
mod logging;

use std::process::ExitCode;

use clap::Parser;
use color_eyre::eyre::{Result, WrapErr};
use hit_config::Paths;

use crate::cli::{Cli, Command};

fn main() -> Result<ExitCode> {
    color_eyre::config::HookBuilder::default()
        .panic_section(format!(
            "This is a bug in hit. Please report it at {}/issues",
            env!("CARGO_PKG_REPOSITORY")
        ))
        .display_env_section(false)
        .display_location_section(cfg!(debug_assertions))
        .install()?;

    let cli = Cli::parse();

    if let Some(Command::Completions { shell }) = cli.command {
        commands::completions::run(shell);
        return Ok(ExitCode::SUCCESS);
    }

    let paths = Paths::resolve().wrap_err("could not determine hit's directories")?;
    let config_file = cli.config.clone().unwrap_or_else(|| paths.config_file());

    match &cli.command {
        None => {
            let level = cli.log_level.map(Into::into);
            commands::tui::run(&paths, &config_file, level)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Config { command }) => commands::config::run(command, &config_file),
        Some(Command::Paths) => {
            commands::paths::run(&paths, &config_file);
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Completions { .. }) => Ok(ExitCode::SUCCESS),
    }
}
