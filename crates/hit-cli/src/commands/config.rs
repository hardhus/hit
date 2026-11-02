use std::{path::Path, process::ExitCode};

use color_eyre::eyre::{Result, WrapErr};
use hit_config::Source;

use crate::cli::ConfigCommand;

pub fn run(command: &ConfigCommand, config_file: &Path) -> Result<ExitCode> {
    match command {
        ConfigCommand::Path => {
            println!("{}", config_file.display());
            Ok(ExitCode::SUCCESS)
        }
        ConfigCommand::Check => Ok(check(config_file)),
        ConfigCommand::Init { force } => {
            hit_config::init(config_file, *force).wrap_err("could not write the default config")?;
            println!("wrote {}", config_file.display());
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn check(config_file: &Path) -> ExitCode {
    let loaded = hit_config::load(config_file);
    if loaded.source == Source::Defaults && loaded.warnings.is_empty() {
        println!(
            "no config file at {}; using defaults (create one with `hit config init`)",
            config_file.display()
        );
        return ExitCode::SUCCESS;
    }
    if loaded.warnings.is_empty() {
        println!("{}: ok", config_file.display());
        return ExitCode::SUCCESS;
    }
    eprintln!("{}:", config_file.display());
    for warning in &loaded.warnings {
        eprintln!("  warning: {warning}");
    }
    ExitCode::FAILURE
}
