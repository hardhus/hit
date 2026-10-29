//! Lenient config loading.
//!
//! A broken config never stops hit from starting. Problems are collected as
//! [`ConfigWarning`]s and the affected values fall back to their defaults:
//! a syntax error resets the whole file, an invalid value resets only that key,
//! unknown keys are ignored and out-of-range numbers are clamped.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::de::DeserializeOwned;
use toml::{Table, Value};

use crate::model::{CacheConfig, Config, FocusConfig, LogConfig, RefreshConfig, UiConfig};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config file already exists at {0} (use --force to overwrite)")]
    AlreadyExists(PathBuf),
    #[error("could not write config file {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Defaults,
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigWarning {
    Unreadable {
        path: PathBuf,
        message: String,
    },
    Syntax {
        line: usize,
        column: usize,
        message: String,
    },
    NotATable {
        section: String,
    },
    UnknownSection {
        section: String,
    },
    UnknownKey {
        section: String,
        key: String,
    },
    InvalidValue {
        section: String,
        key: String,
        message: String,
    },
    OutOfRange {
        section: String,
        key: String,
        value: String,
        used: String,
    },
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, message } => {
                write!(
                    f,
                    "could not read {}: {message}; using defaults",
                    path.display()
                )
            }
            Self::Syntax {
                line,
                column,
                message,
            } => write!(
                f,
                "syntax error at line {line}, column {column}: {message}; using defaults"
            ),
            Self::NotATable { section } => {
                write!(f, "[{section}] must be a table; using defaults for it")
            }
            Self::UnknownSection { section } => write!(f, "unknown section [{section}]; ignored"),
            Self::UnknownKey { section, key } => write!(f, "unknown key {section}.{key}; ignored"),
            Self::InvalidValue {
                section,
                key,
                message,
            } => write!(f, "invalid {section}.{key}: {message}; using default"),
            Self::OutOfRange {
                section,
                key,
                value,
                used,
            } => write!(f, "{section}.{key} = {value} is out of range; using {used}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<ConfigWarning>,
    pub source: Source,
}

/// Loads the config at `path`. Never fails; see the module docs.
pub fn load(path: &Path) -> Loaded {
    let defaults = |warnings, source| Loaded {
        config: Config::default(),
        warnings,
        source,
    };

    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return defaults(Vec::new(), Source::Defaults);
        }
        Err(e) => {
            let warning = ConfigWarning::Unreadable {
                path: path.to_path_buf(),
                message: e.to_string(),
            };
            return defaults(vec![warning], Source::Defaults);
        }
    };

    let (config, warnings) = parse(&text);
    Loaded {
        config,
        warnings,
        source: Source::File(path.to_path_buf()),
    }
}

/// Parses config text leniently.
pub(crate) fn parse(text: &str) -> (Config, Vec<ConfigWarning>) {
    let mut warnings = Vec::new();

    let mut root = match text.parse::<Table>() {
        Ok(root) => root,
        Err(e) => {
            let (line, column) = e
                .span()
                .map(|span| line_col(text, span.start))
                .unwrap_or((1, 1));
            warnings.push(ConfigWarning::Syntax {
                line,
                column,
                message: e.message().to_owned(),
            });
            return (Config::default(), warnings);
        }
    };

    let mut config = Config {
        general: section(&mut root, "general", &mut warnings),
        ui: section(&mut root, "ui", &mut warnings),
        log: section(&mut root, "log", &mut warnings),
        auth: section(&mut root, "auth", &mut warnings),
        focus: section(&mut root, "focus", &mut warnings),
        refresh: section(&mut root, "refresh", &mut warnings),
        cache: section(&mut root, "cache", &mut warnings),
        keys: section(&mut root, "keys", &mut warnings),
        hosts: section(&mut root, "hosts", &mut warnings),
    };

    for unknown in root.keys() {
        warnings.push(ConfigWarning::UnknownSection {
            section: unknown.clone(),
        });
    }

    validate(&mut config, &mut warnings);
    (config, warnings)
}

/// Deserializes one section key by key so a single bad value only resets
/// itself instead of the whole section.
fn section<T>(root: &mut Table, name: &str, warnings: &mut Vec<ConfigWarning>) -> T
where
    T: DeserializeOwned + Default,
{
    let Some(value) = root.remove(name) else {
        return T::default();
    };
    let Value::Table(mut table) = value else {
        warnings.push(ConfigWarning::NotATable {
            section: name.to_owned(),
        });
        return T::default();
    };

    let invalid: Vec<(String, String)> = table
        .iter()
        .filter_map(|(key, value)| {
            let mut single = Table::new();
            single.insert(key.clone(), value.clone());
            T::deserialize(Value::Table(single))
                .err()
                .map(|e| (key.clone(), e.message().to_owned()))
        })
        .collect();
    for (key, message) in invalid {
        table.remove(&key);
        warnings.push(ConfigWarning::InvalidValue {
            section: name.to_owned(),
            key,
            message,
        });
    }

    let mut unknown = Vec::new();
    let parsed = serde_ignored::deserialize(Value::Table(table), |path| {
        unknown.push(path.to_string());
    });
    for key in unknown {
        warnings.push(ConfigWarning::UnknownKey {
            section: name.to_owned(),
            key,
        });
    }

    // Every remaining key deserialized on its own, so this only fails on
    // cross-field problems; fall back to defaults rather than give up.
    parsed.unwrap_or_else(|e: toml::de::Error| {
        warnings.push(ConfigWarning::InvalidValue {
            section: name.to_owned(),
            key: "*".to_owned(),
            message: e.message().to_owned(),
        });
        T::default()
    })
}

fn validate(config: &mut Config, warnings: &mut Vec<ConfigWarning>) {
    let (lo, hi) = UiConfig::FPS_RANGE;
    clamp(&mut config.ui.max_fps, lo, hi, "ui", "max_fps", warnings);

    let (lo, hi) = LogConfig::RETENTION_RANGE;
    clamp(
        &mut config.log.retention_days,
        lo,
        hi,
        "log",
        "retention_days",
        warnings,
    );

    let (lo, hi) = RefreshConfig::FOCUS_MINUTES_RANGE;
    clamp(
        &mut config.refresh.focus_minutes,
        lo,
        hi,
        "refresh",
        "focus_minutes",
        warnings,
    );

    let (lo, hi) = CacheConfig::MAX_AGE_RANGE;
    clamp(
        &mut config.cache.max_age_days,
        lo,
        hi,
        "cache",
        "max_age_days",
        warnings,
    );

    let (lo, hi) = FocusConfig::MENTION_DAYS_RANGE;
    clamp(
        &mut config.focus.mention_days,
        lo,
        hi,
        "focus",
        "mention_days",
        warnings,
    );

    let (lo, hi) = LogConfig::TOTAL_MB_RANGE;
    clamp(
        &mut config.log.max_total_mb,
        lo,
        hi,
        "log",
        "max_total_mb",
        warnings,
    );
}

fn clamp<T>(
    value: &mut T,
    lo: T,
    hi: T,
    section: &str,
    key: &str,
    warnings: &mut Vec<ConfigWarning>,
) where
    T: PartialOrd + Copy + fmt::Display,
{
    let clamped = if *value < lo {
        lo
    } else if *value > hi {
        hi
    } else {
        return;
    };
    warnings.push(ConfigWarning::OutOfRange {
        section: section.to_owned(),
        key: key.to_owned(),
        value: value.to_string(),
        used: clamped.to_string(),
    });
    *value = clamped;
}

/// 1-based line and column of a byte offset.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |last| last.chars().count())
        + 1;
    (line, column)
}

/// The commented default config written by `hit config init`.
pub fn default_toml() -> &'static str {
    include_str!("default_config.toml")
}

/// Writes the default config to `path`, creating parent directories.
pub fn init(path: &Path, force: bool) -> Result<(), ConfigError> {
    if path.exists() && !force {
        return Err(ConfigError::AlreadyExists(path.to_path_buf()));
    }
    let write_err = |source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(write_err)?;
    }
    fs::write(path, default_toml()).map_err(write_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LogLevel, ThemeName};

    #[test]
    fn empty_text_gives_defaults() {
        let (config, warnings) = parse("");
        assert_eq!(config, Config::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn default_template_matches_defaults() {
        let (config, warnings) = parse(default_toml());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn syntax_error_resets_everything_with_position() {
        let (config, warnings) = parse("[ui]\ntransparent = false\nmax_fps = = 3\n");
        assert_eq!(config, Config::default());
        let [ConfigWarning::Syntax { line, .. }] = warnings.as_slice() else {
            panic!("expected one syntax warning, got {warnings:?}");
        };
        assert_eq!(*line, 3);
    }

    #[test]
    fn invalid_value_only_resets_that_key() {
        let text = r#"
            [ui]
            transparent = false
            max_fps = "fast"

            [log]
            level = "loud"
            retention_days = 3
        "#;
        let (config, warnings) = parse(text);
        assert!(!config.ui.transparent);
        assert_eq!(config.ui.max_fps, UiConfig::default().max_fps);
        assert_eq!(config.log.level, LogLevel::Info);
        assert_eq!(config.log.retention_days, 3);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .all(|w| matches!(w, ConfigWarning::InvalidValue { .. }))
        );
    }

    #[test]
    fn unknown_keys_and_sections_are_reported() {
        let text = "[ui]\ncolour = 1\ntheme = \"jinx\"\n[nope]\na = 1\n";
        let (config, warnings) = parse(text);
        assert_eq!(config.ui.theme, ThemeName::Jinx);
        assert!(warnings.contains(&ConfigWarning::UnknownKey {
            section: "ui".into(),
            key: "colour".into()
        }));
        assert!(warnings.contains(&ConfigWarning::UnknownSection {
            section: "nope".into()
        }));
    }

    #[test]
    fn non_table_section_falls_back() {
        let (config, warnings) = parse("ui = 3\n");
        assert_eq!(config.ui, UiConfig::default());
        assert_eq!(
            warnings,
            vec![ConfigWarning::NotATable {
                section: "ui".into()
            }]
        );
    }

    #[test]
    fn auth_and_hosts_sections() {
        let text = r#"
            [auth]
            read_only = true

            [hosts."ghe.example.com"]
            oauth_client_id = "Ov23abc"

            [hosts."broken.example.com"]
            oauth_client_id = 42
        "#;
        let (config, warnings) = parse(text);
        assert!(config.auth.read_only);
        assert!(!config.auth.allow_mark_read);
        assert_eq!(
            config.hosts["ghe.example.com"].oauth_client_id.as_deref(),
            Some("Ov23abc")
        );
        assert!(!config.hosts.contains_key("broken.example.com"));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn out_of_range_is_clamped() {
        let (config, warnings) = parse("[ui]\nmax_fps = 1000\n[log]\nretention_days = 0\n");
        assert_eq!(config.ui.max_fps, UiConfig::FPS_RANGE.1);
        assert_eq!(config.log.retention_days, LogConfig::RETENTION_RANGE.0);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn missing_file_is_defaults_without_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(&dir.path().join("config.toml"));
        assert_eq!(loaded.source, Source::Defaults);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn init_refuses_to_overwrite_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        init(&path, false).unwrap();
        assert!(matches!(
            init(&path, false),
            Err(ConfigError::AlreadyExists(_))
        ));
        init(&path, true).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.source, Source::File(path));
        assert!(loaded.warnings.is_empty());
    }
}
