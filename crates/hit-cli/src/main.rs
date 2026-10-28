mod accounts;
mod cli;
mod commands;
mod logging;
mod style;

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
            let options = commands::tui::Options {
                config_file: &config_file,
                log_level: cli.log_level.map(Into::into),
                demo: cli.demo,
            };
            commands::tui::run(&paths, &options)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Auth { command }) => {
            paths
                .ensure()
                .wrap_err("could not create hit's directories")?;
            let loaded = hit_config::load(&config_file);
            let _log_guard = logging::init(
                &paths.logs,
                &loaded.config.log,
                cli.log_level.map(Into::into),
            )?;
            for warning in &loaded.warnings {
                eprintln!("{} config: {warning}", style::warn("warning:"));
            }
            commands::auth::run(command, &loaded.config, &paths)
        }
        Some(Command::Cache { command }) => Ok(commands::cache::run(command, &paths)),
        Some(Command::Config { command }) => commands::config::run(command, &config_file),
        Some(Command::Paths) => {
            commands::paths::run(&paths, &config_file);
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Completions { .. }) => Ok(ExitCode::SUCCESS),
    }
}
