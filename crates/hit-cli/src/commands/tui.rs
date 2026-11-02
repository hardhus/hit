use std::{path::Path, sync::Arc};

use color_eyre::{
    Section,
    eyre::{Result, WrapErr},
};
use hit_config::{Config, LogLevel, Paths};
use hit_core::{Forges, RepoRef};
use hit_forge_mock::{MockForge, MockOptions};
use hit_tui::Services;

use crate::{accounts, logging, repo};

pub struct Options<'a> {
    pub config_file: &'a Path,
    pub log_level: Option<LogLevel>,
    pub demo: bool,
    pub repo: Option<&'a str>,
}

/// The repository to open on: the one asked for, otherwise the current
/// folder's. Only hosts with a signed-in account count, so a folder on
/// another forge just opens the overview.
pub(crate) fn repo_to_open(
    options: &Options<'_>,
    config: &Config,
    forges: &Forges,
) -> Result<Option<RepoRef>, String> {
    let signed_in = |repo: &RepoRef| forges.iter().any(|f| f.account().host == repo.host);
    if let Some(flag) = options.repo {
        let Some(default_host) = forges.iter().next().map(|f| f.account().host.clone()) else {
            return Ok(None);
        };
        let repo = repo::parse_flag(flag, &default_host)
            .ok_or_else(|| format!("--repo: {flag:?} is not [HOST/]OWNER/NAME"))?;
        if !signed_in(&repo) {
            return Err(format!("--repo: no account on {}", repo.host));
        }
        return Ok(Some(repo));
    }
    // The demo's repositories are made up; a real folder never matches them.
    if options.demo || !config.general.repo_context {
        return Ok(None);
    }
    let Ok(cwd) = std::env::current_dir() else {
        return Ok(None);
    };
    Ok(repo::remotes_of(&cwd).into_iter().find(signed_in))
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
    let mut services = if options.demo {
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
            repo: None,
            repo_candidates: Vec::new(),
            config_file: Some(options.config_file.to_path_buf()),
        }
    };
    if services.forges.is_empty() && !options.demo && loaded.config.general.repo_context {
        services.repo_candidates = std::env::current_dir()
            .map(|cwd| repo::remotes_of(&cwd))
            .unwrap_or_default();
    }
    match repo_to_open(options, &loaded.config, &services.forges) {
        Ok(repo) => services.repo = repo,
        Err(warning) => {
            tracing::warn!(%warning, "repo");
            warnings.push(warning);
        }
    }
    tracing::info!(forges = ?services.forges, cache = services.cache.is_some(), demo = options.demo, repo = ?services.repo, "services");

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
