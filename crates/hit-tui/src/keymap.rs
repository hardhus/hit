//! Data-driven key bindings.
//!
//! Every user-facing action is a [`Command`] with a name, a description and
//! default keys. `[keys]` in the config replaces a command's keys. The help
//! overlay, the hint line and the command palette are all generated from
//! here, so they never drift from what the keys actually do.

use std::{collections::BTreeMap, fmt};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Where a command applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Global,
    List,
}

impl Scope {
    pub fn title(self) -> &'static str {
        match self {
            Self::Global => "Everywhere",
            Self::List => "Lists",
        }
    }
}

macro_rules! commands {
    ($( $variant:ident => $name:literal, $scope:ident, $desc:literal, [$($key:literal),*]; )*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Command {
            $($variant,)*
        }

        impl Command {
            pub const ALL: &'static [Command] = &[$(Command::$variant,)*];

            /// Config name (`[keys] <name> = [...]`).
            pub fn name(self) -> &'static str {
                match self { $(Command::$variant => $name,)* }
            }

            pub fn scope(self) -> Scope {
                match self { $(Command::$variant => Scope::$scope,)* }
            }

            pub fn description(self) -> &'static str {
                match self { $(Command::$variant => $desc,)* }
            }

            fn default_keys(self) -> &'static [&'static str] {
                match self { $(Command::$variant => &[$($key),*],)* }
            }
        }
    };
}

commands! {
    Quit         => "quit",          Global, "Quit hit",                         ["q"];
    Help         => "help",          Global, "Show all keys",                    ["?"];
    Palette      => "palette",       Global, "Command palette and jump",         [":", "ctrl+p"];
    NextTab      => "next_tab",      Global, "Next tab",                         ["tab", "L"];
    PrevTab      => "prev_tab",      Global, "Previous tab",                     ["shift+tab", "H"];
    Tab1         => "tab_1",         Global, "Go to Focus",                      ["1"];
    Tab2         => "tab_2",         Global, "Go to Inbox",                      ["2"];
    Tab3         => "tab_3",         Global, "Go to Pull requests",              ["3"];
    Tab4         => "tab_4",         Global, "Go to Issues",                     ["4"];
    Tab5         => "tab_5",         Global, "Go to Repositories",               ["5"];
    Refresh      => "refresh",       Global, "Refresh now",                      ["r"];
    CycleAccount => "cycle_account", Global, "Show one account / all accounts",  ["a"];
    Up           => "up",            List,   "Move up",                          ["k", "up"];
    Down         => "down",          List,   "Move down",                        ["j", "down"];
    Top          => "top",           List,   "Go to the first item",             ["g", "home"];
    Bottom       => "bottom",        List,   "Go to the last item",              ["G", "end"];
    PageUp       => "page_up",       List,   "Page up",                          ["ctrl+u", "pageup"];
    PageDown     => "page_down",     List,   "Page down",                        ["ctrl+d", "pagedown"];
    Open         => "open",          List,   "Open the selected item",           ["enter"];
    Back         => "back",          List,   "Back / close",                     ["esc"];
    Filter       => "filter",        List,   "Filter the list",                  ["/"];
    Browser      => "browser",       List,   "Open in the browser",              ["o"];
    NextGroup    => "next_group",    List,   "Jump to the next group",           ["J"];
    PrevGroup    => "prev_group",    List,   "Jump to the previous group",       ["K"];
    Sort         => "sort",          List,   "Newest / oldest first",            ["s"];
    ToggleGroups => "toggle_groups", List,   "Group / flat list",                ["v"];
    MarkRead     => "mark_read",     List,   "Mark notification read",           ["m"];
    MarkAllRead  => "mark_all_read", List,   "Mark all shown notifications read",["M"];
    ToggleUnread => "toggle_unread", List,   "Unread only / all notifications",  ["u"];
    Participating=> "participating", List,   "Only notifications for you",       ["p"];
    CycleReason  => "cycle_reason",  List,   "Filter by reason / switch view",   ["f"];
    NextFile     => "next_file",     List,   "Next file in a diff",              ["]"];
    PrevFile     => "prev_file",     List,   "Previous file in a diff",          ["["];
    NextHunk     => "next_hunk",     List,   "Next change in a diff",            ["}"];
    PrevHunk     => "prev_hunk",     List,   "Previous change in a diff",        ["{"];
    SplitDiff    => "split_diff",    List,   "Unified / side-by-side diff",      ["t"];
    ScrollLeft   => "scroll_left",   List,   "Scroll a diff left",               ["h", "left"];
    ScrollRight  => "scroll_right",  List,   "Scroll a diff right",              ["l", "right"];
}

/// One key with modifiers. Letters keep their case (`G` is shift+g).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    code: KeyCode,
    ctrl: bool,
    alt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown key {0:?}")]
pub struct BadKey(pub String);

impl Key {
    pub fn parse(spec: &str) -> Result<Self, BadKey> {
        let bad = || BadKey(spec.to_owned());
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut rest = spec.trim();
        loop {
            let lower = rest.to_ascii_lowercase();
            if let Some(r) = lower.strip_prefix("ctrl+") {
                ctrl = true;
                rest = &rest[rest.len() - r.len()..];
            } else if let Some(r) = lower.strip_prefix("alt+") {
                alt = true;
                rest = &rest[rest.len() - r.len()..];
            } else if let Some(r) = lower.strip_prefix("shift+") {
                shift = true;
                rest = &rest[rest.len() - r.len()..];
            } else {
                break;
            }
        }
        let code = match rest.to_ascii_lowercase().as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" if shift => KeyCode::BackTab,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            f if f.len() > 1 && f.starts_with('f') => {
                let n: u8 = f[1..].parse().map_err(|_| bad())?;
                if !(1..=12).contains(&n) {
                    return Err(bad());
                }
                KeyCode::F(n)
            }
            _ => {
                let mut chars = rest.chars();
                let (Some(c), None) = (chars.next(), chars.next()) else {
                    return Err(bad());
                };
                if c.is_control() {
                    return Err(bad());
                }
                let c = if shift { c.to_ascii_uppercase() } else { c };
                // Ctrl combos arrive lower-case.
                KeyCode::Char(if ctrl { c.to_ascii_lowercase() } else { c })
            }
        };
        Ok(Self { code, ctrl, alt })
    }

    pub fn from_event(event: &KeyEvent) -> Self {
        let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
        let alt = event.modifiers.contains(KeyModifiers::ALT);
        // AltGr arrives as ctrl+alt on Windows; the character it produces
        // (`#`, `@`, `{` on many layouts) is what the user typed.
        if is_altgr_char(event) {
            return Self {
                code: event.code,
                ctrl: false,
                alt: false,
            };
        }
        Self {
            code: event.code,
            ctrl,
            alt,
        }
    }
}

/// A printable character typed with AltGr (reported as ctrl+alt).
pub fn is_altgr_char(event: &KeyEvent) -> bool {
    let altgr = event
        .modifiers
        .contains(KeyModifiers::CONTROL | KeyModifiers::ALT);
    altgr && matches!(event.code, KeyCode::Char(c) if !c.is_control())
}

/// A printable character the user typed (plain, shifted or with AltGr),
/// as opposed to a ctrl/alt shortcut.
pub fn typed_char(event: &KeyEvent) -> Option<char> {
    let KeyCode::Char(c) = event.code else {
        return None;
    };
    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
    let alt = event.modifiers.contains(KeyModifiers::ALT);
    (!(ctrl || alt) || is_altgr_char(event)).then_some(c)
}

impl Key {}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("ctrl+")?;
        }
        if self.alt {
            f.write_str("alt+")?;
        }
        match self.code {
            KeyCode::Enter => f.write_str("↵"),
            KeyCode::Esc => f.write_str("esc"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::BackTab => f.write_str("shift+tab"),
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Backspace => f.write_str("⌫"),
            KeyCode::Delete => f.write_str("del"),
            KeyCode::Up => f.write_str("↑"),
            KeyCode::Down => f.write_str("↓"),
            KeyCode::Left => f.write_str("←"),
            KeyCode::Right => f.write_str("→"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::PageUp => f.write_str("pgup"),
            KeyCode::PageDown => f.write_str("pgdn"),
            KeyCode::F(n) => write!(f, "f{n}"),
            KeyCode::Char(c) => write!(f, "{c}"),
            other => write!(f, "{other:?}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: BTreeMap<Command, Vec<Key>>,
}

impl Default for Keymap {
    fn default() -> Self {
        let bindings = Command::ALL
            .iter()
            .map(|&cmd| {
                let keys = cmd
                    .default_keys()
                    .iter()
                    .filter_map(|k| Key::parse(k).ok())
                    .collect();
                (cmd, keys)
            })
            .collect();
        Self { bindings }
    }
}

impl Keymap {
    /// Defaults with `[keys]` overrides applied. Problems become warnings;
    /// the affected command keeps its defaults.
    pub fn from_config(overrides: &BTreeMap<String, Vec<String>>) -> (Self, Vec<String>) {
        let mut map = Self::default();
        let mut warnings = Vec::new();
        for (name, specs) in overrides {
            let Some(&cmd) = Command::ALL.iter().find(|c| c.name() == name) else {
                warnings.push(format!("keys: unknown command {name:?}"));
                continue;
            };
            let parsed: Result<Vec<Key>, BadKey> = specs.iter().map(|s| Key::parse(s)).collect();
            match parsed {
                Ok(keys) if !keys.is_empty() => {
                    map.bindings.insert(cmd, keys);
                }
                Ok(_) => warnings.push(format!("keys: {name} has no keys; keeping the defaults")),
                Err(e) => warnings.push(format!("keys: {name}: {e}; keeping the defaults")),
            }
        }
        for conflict in map.conflicts() {
            warnings.push(conflict);
        }
        (map, warnings)
    }

    /// Keys bound to two commands that can be active at the same time.
    fn conflicts(&self) -> Vec<String> {
        let mut seen: BTreeMap<String, Command> = BTreeMap::new();
        let mut out = Vec::new();
        for (&cmd, keys) in &self.bindings {
            for key in keys {
                let id = key.to_string();
                if let Some(&other) = seen.get(&id) {
                    out.push(format!(
                        "keys: {id} is bound to both {} and {}",
                        other.name(),
                        cmd.name()
                    ));
                } else {
                    seen.insert(id, cmd);
                }
            }
        }
        out
    }

    /// The command for `event` among the given scopes.
    pub fn lookup(&self, event: &KeyEvent, scopes: &[Scope]) -> Option<Command> {
        let key = Key::from_event(event);
        self.bindings
            .iter()
            .find(|(cmd, keys)| scopes.contains(&cmd.scope()) && keys.contains(&key))
            .map(|(&cmd, _)| cmd)
    }

    pub fn keys(&self, cmd: Command) -> &[Key] {
        self.bindings.get(&cmd).map_or(&[], Vec::as_slice)
    }

    /// "j/↓" style label for hints and help.
    pub fn label(&self, cmd: Command) -> String {
        self.keys(cmd)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("/")
    }

    /// The first key only, for compact hints.
    pub fn short(&self, cmd: Command) -> String {
        self.keys(cmd)
            .first()
            .map(ToString::to_string)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn parses_specs() {
        let k = |s| Key::parse(s).unwrap();
        assert_eq!(
            k("ctrl+p"),
            Key {
                code: KeyCode::Char('p'),
                ctrl: true,
                alt: false
            }
        );
        assert_eq!(k("Ctrl+P"), k("ctrl+p"));
        assert_eq!(k("G").code, KeyCode::Char('G'));
        assert_eq!(k("shift+g").code, KeyCode::Char('G'));
        assert_eq!(k("shift+tab").code, KeyCode::BackTab);
        assert_eq!(k("f5").code, KeyCode::F(5));
        assert_eq!(k("enter").to_string(), "↵");
        for bad in ["", "foo", "ctrl+", "f13", "\u{7}"] {
            assert!(Key::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn defaults_are_vim_style_and_conflict_free() {
        let (map, warnings) = Keymap::from_config(&BTreeMap::new());
        assert!(warnings.is_empty(), "{warnings:?}");
        let scopes = [Scope::Global, Scope::List];
        let lookup = |code, m| map.lookup(&press(code, m), &scopes);
        assert_eq!(
            lookup(KeyCode::Char('j'), KeyModifiers::NONE),
            Some(Command::Down)
        );
        assert_eq!(
            lookup(KeyCode::Down, KeyModifiers::NONE),
            Some(Command::Down)
        );
        assert_eq!(
            lookup(KeyCode::Char('G'), KeyModifiers::SHIFT),
            Some(Command::Bottom)
        );
        assert_eq!(
            lookup(KeyCode::Char('p'), KeyModifiers::CONTROL),
            Some(Command::Palette)
        );
        assert_eq!(
            lookup(KeyCode::BackTab, KeyModifiers::SHIFT),
            Some(Command::PrevTab)
        );
        assert_eq!(lookup(KeyCode::Char('x'), KeyModifiers::NONE), None);
        // List keys are inactive outside lists.
        assert_eq!(
            map.lookup(
                &press(KeyCode::Char('j'), KeyModifiers::NONE),
                &[Scope::Global]
            ),
            None
        );
        assert_eq!(map.label(Command::Down), "j/↓");
    }

    #[test]
    fn altgr_characters_are_typed_not_shortcuts() {
        let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;
        let hash = press(KeyCode::Char('#'), altgr);
        assert_eq!(typed_char(&hash), Some('#'));
        assert_eq!(Key::from_event(&hash), Key::parse("#").unwrap());
        assert_eq!(
            typed_char(&press(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            typed_char(&press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some('G')
        );
    }

    #[test]
    fn overrides_replace_defaults_and_report_problems() {
        let mut overrides = BTreeMap::new();
        overrides.insert("down".to_owned(), vec!["n".to_owned()]);
        overrides.insert("nonsense".to_owned(), vec!["x".to_owned()]);
        overrides.insert("up".to_owned(), vec!["notakey".to_owned()]);
        overrides.insert("refresh".to_owned(), vec!["q".to_owned()]);
        let (map, warnings) = Keymap::from_config(&overrides);
        let scopes = [Scope::Global, Scope::List];
        assert_eq!(
            map.lookup(&press(KeyCode::Char('n'), KeyModifiers::NONE), &scopes),
            Some(Command::Down)
        );
        assert_eq!(
            map.lookup(&press(KeyCode::Char('j'), KeyModifiers::NONE), &scopes),
            None
        );
        // A bad spec keeps the defaults.
        assert_eq!(map.label(Command::Up), "k/↑");
        assert!(warnings.iter().any(|w| w.contains("nonsense")));
        assert!(warnings.iter().any(|w| w.contains("notakey")));
        assert!(warnings.iter().any(|w| w.contains("bound to both")));
    }
}
