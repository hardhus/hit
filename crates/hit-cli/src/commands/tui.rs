use std::{path::Path, sync::Arc};

use color_eyre::{
    Section,
    eyre::{Result, WrapErr},
};
use hit_config::{LogLevel, Paths};
use hit_core::Forges;
use hit_forge_mock::{MockForge, MockOptions};
use hit_tui::Services;

use crate::{accounts, logging};

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
    let mut warnings: Vec<String> = loaded.warnings.iter().map(ToString::to_string).collect();

    // Accounts are read before the runtime starts: keyring access blocks.
    let services = if options.demo {
        // The demo never touches the cache: nothing real to keep.
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::default())));
        Services {
            forges,
            ..Services::default()
        }
    } else {
        let (forges, account_warnings) = accounts::forges(&loaded.config, paths);
        for warning in &account_warnings {
            tracing::warn!(%warning, "accounts");
        }
        warnings.extend(account_warnings);
        Services {
            forges,
            login: accounts::tui_login(&loaded.config, paths),
            cache: accounts::open_cache(&loaded.config, paths),
        }
    };
    tracing::info!(forges = ?services.forges, cache = services.cache.is_some(), demo = options.demo, "services");

    hit_tui::install_panic_hook();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("hit-worker")
        .build()
        .wrap_err("could not start the async runtime")?;

    let result = runtime.block_on(hit_tui::run(&loaded.config, warnings, services));
    tracing::info!(ok = result.is_ok(), "hit exiting");

    result
        .wrap_err("the terminal UI stopped with an error")
        .with_section(|| format!("logs: {}", paths.logs.display()))
}
