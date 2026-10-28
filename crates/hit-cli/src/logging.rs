//! Daily log files with retention by age and total size.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use color_eyre::eyre::{Result, WrapErr};
use hit_config::{LogConfig, LogLevel};
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const PREFIX: &str = "hit";
const SUFFIX: &str = "log";
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Starts file logging. Keep the returned guard alive until exit so buffered
/// lines are flushed.
pub fn init(dir: &Path, config: &LogConfig, cli_level: Option<LogLevel>) -> Result<WorkerGuard> {
    let pruned = prune(
        dir,
        config.retention_days,
        config.max_total_mb.saturating_mul(1024 * 1024),
        SystemTime::now(),
    );

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(PREFIX)
        .filename_suffix(SUFFIX)
        .build(dir)
        .wrap_err_with(|| format!("could not open log directory {}", dir.display()))?;
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let level = cli_level.unwrap_or(config.level);
    let filter =
        EnvFilter::try_from_env("HIT_LOG").unwrap_or_else(|_| EnvFilter::new(level.as_str()));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(writer).with_ansi(false))
        .try_init()
        .wrap_err("could not install the log subscriber")?;

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "hit starting");
    for path in &pruned.removed {
        tracing::debug!(path = %path.display(), "removed old log file");
    }
    for (path, error) in &pruned.failed {
        tracing::warn!(path = %path.display(), %error, "could not remove old log file");
    }
    Ok(guard)
}

#[derive(Debug, Default)]
pub struct Pruned {
    pub removed: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, io::Error)>,
}

/// Deletes `hit.*.log` files older than `retention_days`, then the oldest
/// remaining ones until the total size fits `max_total_bytes`. The newest file
/// is always kept. Other files in `dir` are never touched.
pub fn prune(dir: &Path, retention_days: u32, max_total_bytes: u64, now: SystemTime) -> Pruned {
    let mut pruned = Pruned::default();
    let Ok(entries) = fs::read_dir(dir) else {
        return pruned;
    };

    let mut logs: Vec<(PathBuf, SystemTime, u64)> = entries
        .filter_map(Result::ok)
        .filter(|entry| is_log_file(&entry.file_name().to_string_lossy()))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file()
                .then(|| (entry.path(), meta.modified().unwrap_or(now), meta.len()))
        })
        .collect();
    // Oldest first.
    logs.sort_by_key(|(_, modified, _)| *modified);

    let max_age = DAY.saturating_mul(retention_days);
    let remove = |path: &Path, pruned: &mut Pruned| match fs::remove_file(path) {
        Ok(()) => pruned.removed.push(path.to_path_buf()),
        Err(e) => pruned.failed.push((path.to_path_buf(), e)),
    };

    let newest = logs.len().saturating_sub(1);
    let mut kept = Vec::new();
    for (index, (path, modified, size)) in logs.into_iter().enumerate() {
        let age = now.duration_since(modified).unwrap_or_default();
        if index != newest && age > max_age {
            remove(&path, &mut pruned);
        } else {
            kept.push((path, size));
        }
    }

    let mut total: u64 = kept.iter().map(|(_, size)| size).sum();
    let newest = kept.len().saturating_sub(1);
    for (path, size) in kept.into_iter().take(newest) {
        if total <= max_total_bytes {
            break;
        }
        remove(&path, &mut pruned);
        total = total.saturating_sub(size);
    }
    pruned
}

fn is_log_file(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_suffix(SUFFIX))
        .is_some_and(|date| date.ends_with('.') && date.len() > 1)
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;

    fn log(dir: &Path, name: &str, bytes: usize, age_days: u64, now: SystemTime) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, vec![b'x'; bytes]).unwrap();
        let modified = now - DAY * u32::try_from(age_days).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        path
    }

    #[test]
    fn recognizes_only_log_files() {
        assert!(is_log_file("hit.2026-10-28.log"));
        assert!(!is_log_file("hit.log"));
        assert!(!is_log_file("other.2026-10-28.log"));
        assert!(!is_log_file("hit.2026-10-28.txt"));
    }

    #[test]
    fn removes_files_older_than_retention() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let old = log(dir.path(), "hit.2026-01-01.log", 10, 30, now);
        let recent = log(dir.path(), "hit.2026-10-27.log", 10, 1, now);
        let other = log(dir.path(), "notes.txt", 10, 90, now);

        let pruned = prune(dir.path(), 14, u64::MAX, now);

        assert_eq!(pruned.removed, vec![old.clone()]);
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(other.exists());
    }

    #[test]
    fn removes_oldest_until_size_fits_but_keeps_newest() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let a = log(dir.path(), "hit.a.log", 100, 3, now);
        let b = log(dir.path(), "hit.b.log", 100, 2, now);
        let c = log(dir.path(), "hit.c.log", 500, 0, now);

        prune(dir.path(), 14, 150, now);

        assert!(!a.exists());
        assert!(!b.exists());
        assert!(
            c.exists(),
            "newest file must survive even when over the limit"
        );
    }

    #[test]
    fn missing_directory_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        let pruned = prune(&dir.path().join("nope"), 1, 1, SystemTime::now());
        assert!(pruned.removed.is_empty() && pruned.failed.is_empty());
    }
}
