//! Writing in the user's own editor.

use std::{
    env, fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

/// `$VISUAL`, then `$EDITOR`, then something every system has.
fn editor_command() -> String {
    env::var("VISUAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| env::var("EDITOR").ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "notepad".to_owned()
            } else {
                "vi".to_owned()
            }
        })
}

/// Program and arguments. The whole value may be a path with spaces in it
/// (`C:\Program Files\...`); otherwise it is split like `code --wait`.
fn split_command(command: &str) -> Vec<String> {
    let command = command.trim();
    if Path::new(command).is_file() {
        return vec![command.to_owned()];
    }
    command.split_whitespace().map(str::to_owned).collect()
}

/// Opens `text` in the user's editor and returns what they saved. The terminal
/// must already be handed over. On any problem the text is not lost: the caller
/// keeps what it had.
pub fn edit(text: &str) -> Result<String, String> {
    edit_with(&editor_command(), text)
}

fn edit_with(command: &str, text: &str) -> Result<String, String> {
    let parts = split_command(command);
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| "no editor is set".to_owned())?;
    // Private to the user on every system, and gone afterwards. The name makes
    // editors treat it as Markdown.
    let mut file = tempfile::Builder::new()
        .prefix("hit-comment-")
        .suffix(".md")
        .tempfile()
        .map_err(|e| format!("could not create a temporary file: {e}"))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.flush())
        .map_err(|e| format!("could not write a temporary file: {e}"))?;
    let status = Command::new(program)
        .args(args)
        .arg(file.path())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("could not start {program}: {e}"))?;
    if !status.success() {
        return Err(format!(
            "{program} exited with {status}; nothing was changed"
        ));
    }
    let saved = fs::read_to_string(file.path())
        .map_err(|e| format!("could not read what was saved: {e}"))?;
    Ok(saved.trim_end_matches(['\r', '\n']).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_split_unless_they_are_a_file() {
        assert_eq!(split_command("code --wait"), ["code", "--wait"]);
        assert_eq!(split_command("  vim  "), ["vim"]);
        assert!(split_command("   ").is_empty());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("my editor");
        fs::write(&path, "").unwrap();
        let spaced = path.to_str().unwrap();
        assert_eq!(split_command(spaced), [spaced]);
    }

    #[test]
    fn no_editor_and_a_missing_editor_are_errors() {
        assert!(edit_with("  ", "x").is_err());
        assert!(edit_with("hit-no-such-editor-anywhere", "x").is_err());
    }
}

// Two attributes: clippy only treats a plain `cfg(test)` as test code.
#[cfg(test)]
#[cfg(unix)]
mod unix_tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn script(dir: &Path, body: &str) -> String {
        let path = dir.join("fake-editor");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_owned()
    }

    #[test]
    fn what_the_editor_saves_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let editor = script(dir.path(), r#"printf 'edited %s\n\n' "$(cat "$1")" > "$1""#);
        assert_eq!(edit_with(&editor, "draft").unwrap(), "edited draft");
    }

    #[test]
    fn a_failing_editor_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let editor = script(dir.path(), "exit 3");
        assert!(edit_with(&editor, "draft").is_err());
    }
}
