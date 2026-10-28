use std::path::Path;

use color_eyre::{
    Section,
    eyre::{Result, WrapErr},
};
use hit_config::{LogLevel, Paths};

use crate::logging;

pub fn run(paths: &Paths, config_file: &Path, cli_level: Option<LogLevel>) -> Result<()> {
    paths
        .ensure()
        .wrap_err("could not create hit's directories")?;

    let loaded = hit_config::load(config_file);
    let _log_guard = logging::init(&paths.logs, &loaded.config.log, cli_level)?;
    for warning in &loaded.warnings {
        tracing::warn!(%warning, file = %config_file.display(), "config");
    }

    hit_tui::install_panic_hook();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("hit-worker")
        .build()
        .wrap_err("could not start the async runtime")?;

    let warnings = loaded.warnings.iter().map(ToString::to_string).collect();
    let result = runtime.block_on(hit_tui::run(&loaded.config, warnings));
    tracing::info!(ok = result.is_ok(), "hit exiting");

    result
        .wrap_err("the terminal UI stopped with an error")
        .with_section(|| format!("logs: {}", paths.logs.display()))
}
