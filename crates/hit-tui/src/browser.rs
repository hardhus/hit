//! Opening links in the user's browser. Only https URLs are opened; anything
//! else is refused rather than handed to the OS.

use crate::action::Toast;

pub fn open(url: &str) -> Result<(), Toast> {
    if !url.starts_with("https://") || url.chars().any(char::is_whitespace) {
        return Err(Toast::error(format!("refusing to open {url:?}")));
    }
    open::that_detached(url).map_err(|e| Toast::error(format!("could not open the browser: {e}")))
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
}
