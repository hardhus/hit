use std::{path::Path, sync::Arc};

use color_eyre::{
    Section,
    eyre::{Result, WrapErr},
};
use hit_config::{LogLevel, Paths};
use hit_core::Forges;
use hit_forge_mock::{MockForge, MockOptions};

use crate::logging;

pub struct Options<'a> {
    pub config_file: &'a Path,
    pub log_level: Option<LogLevel>,
    pub demo: bool,
}

pub fn run(paths: &Paths, options: &Options<'_>) -> Result<()> {
    paths
        .ensure()
        .wrap_err("could not create hit's directories")?;

    let loaded = hit_config::load(options.config_file);
    let _log_guard = logging::init(&paths.logs, &loaded.config.log, options.log_level)?;
    for warning in &loaded.warnings {
        tracing::warn!(%warning, file = %options.config_file.display(), "config");
    }

    hit_tui::install_panic_hook();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("hit-worker")
        .build()
        .wrap_err("could not start the async runtime")?;

    let forges = forges(options.demo);
    tracing::info!(?forges, demo = options.demo, "forges");

    let warnings = loaded.warnings.iter().map(ToString::to_string).collect();
    let result = runtime.block_on(hit_tui::run(&loaded.config, warnings, forges));
    tracing::info!(ok = result.is_ok(), "hit exiting");

    result
        .wrap_err("the terminal UI stopped with an error")
        .with_section(|| format!("logs: {}", paths.logs.display()))
}

/// Connected forges. Real accounts arrive with `hit auth` (F2).
fn forges(demo: bool) -> Forges {
    let mut forges = Forges::new();
    if demo {
        forges.add(Arc::new(MockForge::demo(MockOptions::default())));
    }
    forges
}
