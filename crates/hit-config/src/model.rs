//! Configuration model. Every field has a default so a partial (or empty)
//! `config.toml` is always valid.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub ui: UiConfig,
    pub log: LogConfig,
    pub auth: AuthConfig,
    /// Per-host settings, keyed by host name (e.g. `ghe.example.com`).
    pub hosts: BTreeMap<String, HostConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Never write with any account, regardless of the token's permissions.
    pub read_only: bool,
    /// In read-only mode, still allow marking notifications read. Off by
    /// default: read-only means hit changes nothing.
    pub allow_mark_read: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    /// Client id of an OAuth app registered on this host (GitHub Enterprise),
    /// enabling device-flow sign-in there.
    pub oauth_client_id: Option<String>,
}

/// Reserved for app-wide behaviour (startup view, default account, …).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Leave the terminal background visible.
    pub transparent: bool,
    pub theme: ThemeName,
    pub animations: bool,
    /// Frame cap while something is animating. Idle screens are not redrawn.
    pub max_fps: u16,
}

impl UiConfig {
    pub const FPS_RANGE: (u16, u16) = (1, 240);
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            transparent: true,
            theme: ThemeName::default(),
            animations: true,
            max_fps: 60,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    #[default]
    Jinx,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    pub level: LogLevel,
    /// Log files older than this are deleted on startup.
    pub retention_days: u32,
    /// Oldest log files are deleted on startup until the total fits.
    pub max_total_mb: u64,
}

impl LogConfig {
    pub const RETENTION_RANGE: (u32, u32) = (1, 3650);
    pub const TOTAL_MB_RANGE: (u64, u64) = (1, 10_240);
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::default(),
            retention_days: 14,
            max_total_mb: 50,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
