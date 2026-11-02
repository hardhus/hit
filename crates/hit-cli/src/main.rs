mod accounts;
mod cli;
mod commands;
mod logging;
mod repo;
mod style;

use std::process::ExitCode;

use clap::Parser;
use color_eyre::eyre::{Result, WrapErr};
use hit_config::Paths;

use crate::cli::{Cli, Command};

/// `hit ... | head` closes the pipe while hit is still printing, and `println!`
/// panics about it. Nothing is wrong: the reader has what it wanted.
fn quiet_when_nobody_reads() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        // The text after the code is in the system's language.
        let closed = ["os error 32)", "os error 109)", "os error 232)"]
            .iter()
            .any(|code| message.contains(code));
        if message.starts_with("failed printing to stdout") && closed {
            std::process::exit(0);
        }
        report(info);
    }));
}

fn main() -> Result<ExitCode> {
    color_eyre::config::HookBuilder::default()
        .panic_section(format!(
            "This is a bug in hit. Please report it at {}/issues",
            env!("CARGO_PKG_REPOSITORY")
        ))
        .display_env_section(false)
        .display_location_section(cfg!(debug_assertions))
        .install()?;

    quiet_when_nobody_reads();

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
                repo: cli.repo.as_deref(),
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
        Some(Command::Work(command)) => {
            let options = commands::work::Options {
                config_file: &config_file,
                log_level: cli.log_level.map(Into::into),
                demo: cli.demo,
                repo: cli.repo.as_deref(),
                account: cli.account.as_deref(),
                json: cli.json,
            };
            match commands::work::run(command, &paths, &options) {
                Ok(code) => Ok(code),
                // A refusal is an answer here, not a crash report.
                Err(e) => {
                    eprintln!("{} {e}", style::error("error:"));
                    Ok(ExitCode::FAILURE)
                }
            }
        }
    }
}
