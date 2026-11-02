//! Changes single values in `config.toml`, leaving everything else in the
//! file (comments, order, keys hit does not know) as the user wrote it.

use std::{
    fs, io,
    path::{Path, PathBuf},
    process,
};

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::load::{ConfigError, default_toml};

/// A value for one config key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Str(String),
}

impl From<&SettingValue> for Value {
    fn from(value: &SettingValue) -> Self {
        match value {
            SettingValue::Bool(b) => Self::from(*b),
            SettingValue::Int(n) => Self::from(*n),
            SettingValue::Str(s) => Self::from(s.as_str()),
        }
    }
}

/// One change: `[section] key = value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEdit {
    pub section: &'static str,
    pub key: &'static str,
    pub value: SettingValue,
}

/// Applies `edits` to the file at `path`, creating it from the commented
/// default config if there is none. A file that does not parse is left alone:
/// it may hold the user's work, and "fixing" it could lose that.
pub fn apply_edits(path: &Path, edits: &[ConfigEdit]) -> Result<(), ConfigError> {
    let write_err = |source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => default_toml().to_owned(),
        Err(e) => return Err(write_err(e)),
    };
    let mut document: DocumentMut = text.parse().map_err(|_| ConfigError::Unparsable {
        path: path.to_path_buf(),
    })?;
    for edit in edits {
        set(&mut document, edit)?;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(write_err)?;
    }
    write_atomic(path, &document.to_string()).map_err(write_err)
}

fn set(document: &mut DocumentMut, edit: &ConfigEdit) -> Result<(), ConfigError> {
    let not_a_table = || ConfigError::NotATable {
        section: edit.section.to_owned(),
    };
    let table = document
        .as_table_mut()
        .entry(edit.section)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(not_a_table)?;
    let mut value = Value::from(&edit.value);
    match table.get_mut(edit.key) {
        Some(item) => {
            // Keeps a comment at the end of the line.
            if let Some(old) = item.as_value() {
                *value.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(value);
        }
        None => {
            table.insert(edit.key, Item::Value(value));
        }
    }
    Ok(())
}

/// Never leaves a half-written config behind. A symlinked config (dotfile
/// managers) stays a symlink: the file it points at is the one replaced.
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let temporary = temporary_path(&target);
    let written = fs::write(&temporary, text).and_then(|()| {
        if let Ok(metadata) = fs::metadata(&target) {
            fs::set_permissions(&temporary, metadata.permissions())?;
        }
        fs::rename(&temporary, &target)
    });
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

fn temporary_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", process::id()));
    target.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load::load, model::Config};

    fn edit(section: &'static str, key: &'static str, value: SettingValue) -> ConfigEdit {
        ConfigEdit {
            section,
            key,
            value,
        }
    }

    #[test]
    fn everything_else_in_the_file_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "# my notes\n[ui]\n# why I turned these off\nanimations = false # too busy\nmax_fps = 30\n\n[keys]\ndown = [\"j\", \"n\"]\n\n[future]\nunknown = 1\n";
        fs::write(&path, text).unwrap();

        apply_edits(&path, &[edit("ui", "animations", SettingValue::Bool(true))]).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            text.replace("animations = false", "animations = true")
        );
    }

    #[test]
    fn missing_keys_and_sections_are_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[ui]\nmax_fps = 30\n").unwrap();

        apply_edits(
            &path,
            &[
                edit("ui", "animations", SettingValue::Bool(false)),
                edit("inbox", "mark_read", SettingValue::Str("open".to_owned())),
                edit("focus", "mention_days", SettingValue::Int(7)),
            ],
        )
        .unwrap();

        let config = load(&path);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.config.ui.max_fps, 30);
        assert!(!config.config.ui.animations);
        assert_eq!(config.config.inbox.mark_read, crate::MarkRead::Open);
        assert_eq!(config.config.focus.mention_days, 7);
    }

    #[test]
    fn a_missing_file_starts_from_the_commented_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");

        apply_edits(
            &path,
            &[edit("ui", "animations", SettingValue::Bool(false))],
        )
        .unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# hit configuration"));
        assert!(text.contains("# Small animations"));
        let config = load(&path).config;
        assert!(!config.ui.animations);
        assert_eq!(config.focus, Config::default().focus);
    }

    #[test]
    fn a_new_file_only_pins_what_was_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        apply_edits(
            &path,
            &[edit("ui", "animations", SettingValue::Bool(false))],
        )
        .unwrap();

        let text = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        assert!(text.contains("[ui]\nanimations = false\n"), "{text}");
        assert!(text.contains("# focus_minutes = 5"), "{text}");
        let document: DocumentMut = text.parse().unwrap();
        let pinned: Vec<&str> = document
            .iter()
            .filter_map(|(_, section)| section.as_table())
            .flat_map(|table| table.iter().map(|(key, _)| key))
            .collect();
        assert_eq!(pinned, ["animations"]);
    }

    #[test]
    fn a_broken_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[ui\nnot toml at all").unwrap();

        let error = apply_edits(
            &path,
            &[edit("ui", "animations", SettingValue::Bool(false))],
        )
        .unwrap_err();

        assert!(matches!(error, ConfigError::Unparsable { .. }));
        assert_eq!(fs::read_to_string(&path).unwrap(), "[ui\nnot toml at all");
    }

    #[test]
    fn a_section_that_is_not_a_table_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "ui = 3\n").unwrap();

        let error = apply_edits(
            &path,
            &[edit("ui", "animations", SettingValue::Bool(false))],
        )
        .unwrap_err();

        assert!(matches!(error, ConfigError::NotATable { .. }));
        assert_eq!(fs::read_to_string(&path).unwrap(), "ui = 3\n");
    }

    #[test]
    fn no_temporary_file_stays_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        apply_edits(
            &path,
            &[edit("ui", "animations", SettingValue::Bool(false))],
        )
        .unwrap();
        let files: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(files.len(), 1);
    }
}
