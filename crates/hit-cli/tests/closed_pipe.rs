//! `hit ... | head` closes the pipe while hit is still writing. That is how the
//! command line is used, not a bug to report.

#![allow(clippy::unwrap_used)] // A test: failing loudly is the point.

use std::process::{Command, Stdio};

fn run_with_closed_stdout(args: &[&str]) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hit"))
        .args(args)
        .env("HIT_CONFIG_DIR", dir.path().join("config"))
        .env("HIT_DATA_DIR", dir.path().join("data"))
        .env("HIT_STATE_DIR", dir.path().join("state"))
        .env("HIT_CACHE_DIR", dir.path().join("cache"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Nobody will read this.
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn a_closed_pipe_is_not_a_crash() {
    for args in [
        &["--demo", "repos"][..],
        &["--demo", "focus", "--json"],
        &["--demo", "--repo", "acme/rocket", "pr", "list"],
        &["paths"],
    ] {
        let (ok, stderr) = run_with_closed_stdout(args);
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
        assert!(ok, "{args:?}: {stderr}");
    }
}
