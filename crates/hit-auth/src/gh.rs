//! Importing a token from the GitHub CLI (`hit auth login --from-gh`).
//!
//! Runs `gh auth token --hostname=<host>` directly (no shell). Only used when
//! the user asks for it; the output is never logged.

use std::{
    ffi::OsStr,
    io,
    process::{Command, Stdio},
};

use hit_core::Host;

use crate::Secret;

#[derive(Debug, thiserror::Error)]
pub enum GhError {
    #[error("the GitHub CLI (gh) is not installed or not on PATH")]
    NotInstalled,
    #[error("gh has no token for {host}: {message}")]
    NoToken { host: String, message: String },
    #[error("gh returned an invalid token")]
    Invalid,
    #[error("could not run gh")]
    Io(#[source] io::Error),
}

/// Reads the token `gh` has for `host`. `program` is normally `"gh"`.
pub fn token_from_gh(program: &OsStr, host: &Host) -> Result<Secret, GhError> {
    let output = Command::new(program)
        // `--hostname=` keeps a hostile host value from being read as a flag.
        .arg("auth")
        .arg("token")
        .arg(format!("--hostname={host}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => GhError::NotInstalled,
            _ => GhError::Io(e),
        })?;

    if !output.status.success() {
        // gh's error output is a human message and never contains the token.
        let message = String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or("unknown error")
            .trim()
            .chars()
            .take(200)
            .collect();
        return Err(GhError::NoToken {
            host: host.to_string(),
            message,
        });
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| GhError::Invalid)?;
    Secret::new(stdout).map_err(|_| GhError::Invalid)
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::*;

    fn fake_gh(dir: &std::path::Path, script: &str) -> std::path::PathBuf {
        let path = dir.join("gh");
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn reads_the_token_and_passes_the_host_safely() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(
            dir.path(),
            r#"[ "$1 $2 $3" = "auth token --hostname=github.com" ] && echo gho_abc || exit 3"#,
        );
        let token = token_from_gh(gh.as_os_str(), &Host::new("github.com")).unwrap();
        assert_eq!(token.expose(), "gho_abc");
    }

    #[test]
    fn reports_missing_token_and_missing_gh() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(dir.path(), "echo 'no oauth token found for x' >&2; exit 1");
        let err = token_from_gh(gh.as_os_str(), &Host::new("x")).unwrap_err();
        assert!(matches!(err, GhError::NoToken { .. }), "{err}");

        let missing = dir.path().join("nope");
        let err = token_from_gh(missing.as_os_str(), &Host::new("x")).unwrap_err();
        assert!(matches!(err, GhError::NotInstalled));
    }
}
