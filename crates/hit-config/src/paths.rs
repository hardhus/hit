//! Platform directories.
//!
//! Linux and macOS follow the XDG base directory spec. Windows uses the roaming
//! `%APPDATA%` for config and `%LOCALAPPDATA%` for everything else. Every
//! directory can be overridden with a `HIT_*_DIR` environment variable.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

pub const APP_NAME: &str = "hit";
const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    #[error("could not determine the home directory")]
    NoHome,
    #[error("could not determine the {0} folder")]
    NoKnownFolder(&'static str),
    #[error("could not create directory {path}")]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
    pub logs: PathBuf,
}

/// The four base directories before overrides are applied.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Base {
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
    cache: PathBuf,
}

impl Paths {
    /// Resolves directories from the real environment.
    pub fn resolve() -> Result<Self, PathsError> {
        Self::resolve_with(&|key| std::env::var_os(key))
    }

    /// Resolves directories using `env` to read environment variables.
    pub fn resolve_with(env: &dyn Fn(&str) -> Option<OsString>) -> Result<Self, PathsError> {
        let base = platform_base(env)?;
        Ok(Self::from_base(base, env))
    }

    fn from_base(base: Base, env: &dyn Fn(&str) -> Option<OsString>) -> Self {
        let pick = |key: &str, default: PathBuf| absolute_var(env, key).unwrap_or(default);
        let state = pick("HIT_STATE_DIR", base.state);
        Self {
            config: pick("HIT_CONFIG_DIR", base.config),
            data: pick("HIT_DATA_DIR", base.data),
            logs: state.join("logs"),
            state,
            cache: pick("HIT_CACHE_DIR", base.cache),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join(CONFIG_FILE)
    }

    /// Creates every directory that does not exist yet.
    pub fn ensure(&self) -> Result<(), PathsError> {
        for dir in [
            &self.config,
            &self.data,
            &self.state,
            &self.cache,
            &self.logs,
        ] {
            create_dir(dir)?;
        }
        Ok(())
    }
}

fn create_dir(path: &Path) -> Result<(), PathsError> {
    fs::create_dir_all(path).map_err(|source| PathsError::Create {
        path: path.to_path_buf(),
        source,
    })
}

/// Reads an environment variable as a path, ignoring empty and relative values
/// as the XDG spec requires.
fn absolute_var(env: &dyn Fn(&str) -> Option<OsString>, key: &str) -> Option<PathBuf> {
    let value = env(key)?;
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

#[cfg(not(windows))]
fn platform_base(env: &dyn Fn(&str) -> Option<OsString>) -> Result<Base, PathsError> {
    let home = absolute_var(env, "HOME")
        .or_else(dirs::home_dir)
        .ok_or(PathsError::NoHome)?;
    Ok(xdg_base(env, &home))
}

#[cfg(windows)]
fn platform_base(_env: &dyn Fn(&str) -> Option<OsString>) -> Result<Base, PathsError> {
    let roaming = dirs::config_dir().ok_or(PathsError::NoKnownFolder("AppData\\Roaming"))?;
    let local = dirs::data_local_dir().ok_or(PathsError::NoKnownFolder("AppData\\Local"))?;
    Ok(windows_base(&roaming, &local))
}

#[cfg_attr(windows, allow(dead_code))]
fn xdg_base(env: &dyn Fn(&str) -> Option<OsString>, home: &Path) -> Base {
    let xdg = |key: &str, fallback: &str| {
        absolute_var(env, key)
            .unwrap_or_else(|| home.join(fallback))
            .join(APP_NAME)
    };
    Base {
        config: xdg("XDG_CONFIG_HOME", ".config"),
        data: xdg("XDG_DATA_HOME", ".local/share"),
        state: xdg("XDG_STATE_HOME", ".local/state"),
        cache: xdg("XDG_CACHE_HOME", ".cache"),
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn windows_base(roaming: &Path, local: &Path) -> Base {
    let local = local.join(APP_NAME);
    Base {
        config: roaming.join(APP_NAME),
        data: local.join("data"),
        state: local.join("state"),
        cache: local.join("cache"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\root")
        } else {
            PathBuf::from("/root")
        }
    }

    fn env_from(vars: &[(&str, PathBuf)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let map: HashMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone().into_os_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn xdg_defaults_under_home() {
        let home = root().join("home");
        let base = xdg_base(&env_from(&[]), &home);
        assert_eq!(base.config, home.join(".config").join("hit"));
        assert_eq!(base.data, home.join(".local/share").join("hit"));
        assert_eq!(base.state, home.join(".local/state").join("hit"));
        assert_eq!(base.cache, home.join(".cache").join("hit"));
    }

    #[test]
    fn xdg_vars_are_respected() {
        let cfg = root().join("cfg");
        let base = xdg_base(&env_from(&[("XDG_CONFIG_HOME", cfg.clone())]), &root());
        assert_eq!(base.config, cfg.join("hit"));
    }

    #[test]
    fn relative_xdg_vars_are_ignored() {
        let home = root().join("home");
        let base = xdg_base(&env_from(&[("XDG_CACHE_HOME", "rel/cache".into())]), &home);
        assert_eq!(base.cache, home.join(".cache").join("hit"));
    }

    #[test]
    fn windows_layout() {
        let roaming = root().join("Roaming");
        let local = root().join("Local");
        let base = windows_base(&roaming, &local);
        assert_eq!(base.config, roaming.join("hit"));
        assert_eq!(base.data, local.join("hit").join("data"));
        assert_eq!(base.state, local.join("hit").join("state"));
        assert_eq!(base.cache, local.join("hit").join("cache"));
    }

    #[test]
    fn hit_overrides_win_and_logs_follow_state() {
        let base = xdg_base(&env_from(&[]), &root());
        let state = root().join("my-state");
        let env = env_from(&[("HIT_STATE_DIR", state.clone())]);
        let paths = Paths::from_base(base.clone(), &env);
        assert_eq!(paths.state, state);
        assert_eq!(paths.logs, state.join("logs"));
        assert_eq!(paths.config, base.config);
        assert_eq!(paths.config_file(), base.config.join("config.toml"));
    }
}
