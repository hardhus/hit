//! Opening links in the user's browser. Only https URLs are opened; anything
//! else is refused rather than handed to the OS.
//!
//! Links come from outside: a check's address is whatever its CI service says,
//! and on Windows opening one goes through `cmd`. So only the characters a URL
//! is made of get through, without quotes or anything else a shell acts on.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::action::Toast;

/// Running without a person at the keyboard: say what would open instead.
static REHEARSING: AtomicBool = AtomicBool::new(false);

pub fn rehearse() {
    REHEARSING.store(true, Ordering::Relaxed);
}

pub fn open(url: &str) -> Result<(), Toast> {
    if !is_safe(url) {
        return Err(Toast::error(format!("refusing to open {url:?}")));
    }
    if REHEARSING.load(Ordering::Relaxed) {
        return Err(Toast::info(format!("would open {url}")));
    }
    open::that_detached(url).map_err(|e| Toast::error(format!("could not open the browser: {e}")))
}

fn is_safe(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    !rest.is_empty()
        && !rest.starts_with(['/', '@', ':'])
        && url.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '-' | '.' | '_' | '~' | ':' | '/' | '?' | '#' | '[' | ']')
                || matches!(c, '@' | '!' | '$' | '&' | '(' | ')' | '*' | '+' | ',' | ';')
                || matches!(c, '=' | '%')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_non_https() {
        for bad in [
            "http://x",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://a b",
        ] {
            assert!(open(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn nothing_a_shell_acts_on_gets_through() {
        for bad in [
            "https://x\"&calc&\"",
            "https://x\"",
            "https://x'y",
            "https://x`id`",
            "https://x|calc",
            "https://x<y",
            "https://x>y",
            "https://x^y",
            "https://x\\y",
            "https://x\ny",
            "https://x\u{202e}y",
            "https://",
            "https:///x",
            "https://@x",
            "HTTP://x",
        ] {
            assert!(!is_safe(bad), "{bad:?}");
        }
    }

    #[test]
    fn ordinary_links_open() {
        for good in [
            "https://github.com/acme/rocket/pull/7",
            "https://github.com/acme/rocket/actions/runs/1/job/2?pr=7#step:3:1",
            "https://ci.example.com:8443/build/a%20b?x=1&y=2",
        ] {
            assert!(is_safe(good), "{good}");
        }
    }
}
