//! Finds which repository the current folder belongs to by reading its git
//! config. Nothing is run and nothing is written; a folder we cannot make
//! sense of simply is not a repository.

use std::{
    fs,
    path::{Path, PathBuf},
};

use hit_core::{Host, RepoRef};

/// The repositories behind `start`'s remotes, most relevant first: `upstream`
/// (where a fork's pull requests live), then `origin`, then the rest.
pub fn remotes_of(start: &Path) -> Vec<RepoRef> {
    let Some(config) = git_config(start) else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(config) else {
        return Vec::new();
    };
    let mut remotes = parse_remotes(&text);
    remotes.sort_by_key(|(name, _)| match name.as_str() {
        "upstream" => 0,
        "origin" => 1,
        _ => 2,
    });
    remotes
        .iter()
        .filter_map(|(_, url)| parse_remote_url(url))
        .collect()
}

/// The config file of the repository containing `start`. Worktrees and
/// submodules have a `.git` file pointing at the real directory.
fn git_config(start: &Path) -> Option<PathBuf> {
    let dot_git = start
        .ancestors()
        .map(|dir| dir.join(".git"))
        .find(|path| path.exists())?;
    if dot_git.is_dir() {
        return Some(dot_git.join("config"));
    }
    let text = fs::read_to_string(&dot_git).ok()?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    let git_dir = dot_git.parent()?.join(target);
    // A linked worktree shares the main repository's config.
    match fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => Some(git_dir.join(common.trim()).join("config")),
        Err(_) => Some(git_dir.join("config")),
    }
}

/// `(name, url)` of each `[remote "name"]` section, in file order.
fn parse_remotes(config: &str) -> Vec<(String, String)> {
    let mut remotes = Vec::new();
    let mut current: Option<&str> = None;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            current = line
                .strip_prefix("[remote \"")
                .and_then(|rest| rest.strip_suffix("\"]"));
        } else if let (Some(name), Some((key, value))) = (current, line.split_once('='))
            && key.trim().eq_ignore_ascii_case("url")
            // A remote can have several URLs; the first is the one fetched.
            && !remotes.iter().any(|(n, _): &(String, String)| n == name)
        {
            remotes.push((name.to_owned(), value.trim().to_owned()));
        }
    }
    remotes
}

/// Understands `https://host/owner/name.git`, `ssh://git@host:22/owner/name`
/// and `git@host:owner/name.git`. Local paths and anything else give `None`.
fn parse_remote_url(url: &str) -> Option<RepoRef> {
    let (authority, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git") {
            return None;
        }
        rest.split_once('/')?
    } else {
        // scp-like; a `/` before the `:` means a local path.
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') || authority.contains('\\') || authority.len() == 1 {
            return None;
        }
        (authority, path)
    };
    // Credentials can be embedded in a URL; they stop here.
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;
    // GitHub's SSH-over-443 endpoint is still GitHub.
    let host = if host.eq_ignore_ascii_case("ssh.github.com") {
        "github.com"
    } else {
        host
    };
    let clean = |text: &str| {
        !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/'))
    };
    if !clean(host) || !clean(path) || host.contains('/') {
        return None;
    }
    RepoRef::parse(Host::new(host), path).ok()
}

/// `[HOST/]OWNER/NAME` from the command line; `default_host` when no host is
/// given. The first of three or more segments is a host if it has a dot.
pub fn parse_flag(flag: &str, default_host: &Host) -> Option<RepoRef> {
    let flag = flag.trim().trim_matches('/');
    match flag.split_once('/') {
        Some((host, path)) if host.contains('.') && path.contains('/') => {
            RepoRef::parse(Host::new(host), path).ok()
        }
        _ => RepoRef::parse(default_host.clone(), flag).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(url: &str) -> Option<String> {
        parse_remote_url(url).map(|r| format!("{}/{r}", r.host))
    }

    #[test]
    fn remote_url_forms() {
        for url in [
            "https://github.com/acme/rocket.git",
            "https://github.com/acme/rocket",
            "https://user:s3cret@github.com/acme/rocket.git",
            "ssh://git@github.com/acme/rocket.git",
            "ssh://git@ssh.github.com:443/acme/rocket.git",
            "git@github.com:acme/rocket.git",
            "git@GitHub.com:acme/rocket",
        ] {
            assert_eq!(
                parsed(url).as_deref(),
                Some("github.com/acme/rocket"),
                "{url}"
            );
        }
        assert_eq!(
            parsed("git@gitlab.example.com:group/sub/project.git").as_deref(),
            Some("gitlab.example.com/group/sub/project")
        );
    }

    #[test]
    fn things_that_are_not_remotes() {
        for url in [
            "../other/repo",
            "/srv/git/repo.git",
            "C:\\src\\repo",
            "C:/src/repo",
            "file:///srv/git/repo.git",
            "https://github.com/justowner",
            "https://github.com/acme/ro\u{1b}[31mcket",
        ] {
            assert_eq!(parsed(url), None, "{url}");
        }
    }

    #[test]
    fn remote_sections() {
        let config = "[core]\n\turl = nope\n[remote \"origin\"]\n\turl = a\n\turl = b\n\tfetch = x\n[branch \"main\"]\n\tremote = origin\n[remote \"upstream\"]\n\tURL = c\n";
        assert_eq!(
            parse_remotes(config),
            [
                ("origin".to_owned(), "a".to_owned()),
                ("upstream".to_owned(), "c".to_owned())
            ]
        );
    }

    fn write_config(git_dir: &Path, remotes: &str) {
        fs::create_dir_all(git_dir).unwrap();
        fs::write(git_dir.join("config"), remotes).unwrap();
    }

    const FORK: &str = "[remote \"origin\"]\n\turl = git@github.com:me/rocket.git\n[remote \"upstream\"]\n\turl = https://github.com/acme/rocket.git\n";

    #[test]
    fn upstream_wins_and_subfolders_work() {
        let dir = tempfile::tempdir().unwrap();
        write_config(&dir.path().join(".git"), FORK);
        let deep = dir.path().join("src/deep");
        fs::create_dir_all(&deep).unwrap();
        let found: Vec<String> = remotes_of(&deep).iter().map(ToString::to_string).collect();
        assert_eq!(found, ["acme/rocket", "me/rocket"]);
    }

    #[test]
    fn linked_worktrees_use_the_main_config() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main/.git");
        write_config(&main, FORK);
        let linked = main.join("worktrees/feature");
        fs::create_dir_all(&linked).unwrap();
        fs::write(linked.join("commondir"), "../..\n").unwrap();
        let tree = dir.path().join("feature");
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join(".git"), format!("gitdir: {}\n", linked.display())).unwrap();
        assert_eq!(remotes_of(&tree).len(), 2);
    }

    #[test]
    fn a_plain_folder_has_no_remotes() {
        let dir = tempfile::tempdir().unwrap();
        // Only meaningful when the temp dir is not inside a repository.
        if dir
            .path()
            .ancestors()
            .skip(1)
            .all(|p| !p.join(".git").exists())
        {
            assert!(remotes_of(dir.path()).is_empty());
        }
    }

    #[test]
    fn repo_flag() {
        let default = Host::new("github.com");
        let flag = |text| parse_flag(text, &default).map(|r| format!("{}/{r}", r.host));
        assert_eq!(
            flag("acme/rocket").as_deref(),
            Some("github.com/acme/rocket")
        );
        assert_eq!(
            flag("ghe.example.com/acme/rocket").as_deref(),
            Some("ghe.example.com/acme/rocket")
        );
        assert_eq!(flag("rocket"), None);
    }
}
