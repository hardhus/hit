//! Configuration model, lenient loading and platform directories for hit.

mod load;
mod model;
mod paths;

pub use load::{ConfigError, ConfigWarning, Loaded, Source, default_toml, init, load};
pub use model::{
    AuthConfig, Config, GeneralConfig, HostConfig, LogConfig, LogLevel, ThemeName, UiConfig,
};
pub use paths::{APP_NAME, Paths, PathsError};
