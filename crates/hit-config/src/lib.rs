//! Configuration model, lenient loading and platform directories for hit.

mod load;
mod model;
mod paths;

pub use load::{ConfigError, ConfigWarning, Loaded, Source, default_toml, init, load};
pub use model::{
    AuthConfig, CacheConfig, Config, FocusConfig, GeneralConfig, HostConfig, LogConfig, LogLevel,
    RefreshConfig, ThemeName, UiConfig,
};
pub use paths::{APP_NAME, Paths, PathsError};
