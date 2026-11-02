//! The command palette: fuzzy search over commands, and jumps to items
//! (`#142`, `owner/repo#142`, `owner/repo`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hit_core::Host;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    Ctx,
    anim::Moment,
    keymap::{Command, Scope, typed_char},
    screens::JumpTarget,
    ui::{chips::bold, popup::Popup},
};

const MAX_ENTRIES: usize = 10;

#[derive(Debug, Clone)]
pub enum Entry {
    Command(Command),
    Jump(JumpTarget),
    /// Not loaded in hit: open it on the forge.
    Url {
        label: String,
        url: String,
    },
}

pub enum Outcome {
    Close,
    Run(Entry),
}

pub struct Palette {
    input: String,
    selected: usize,
    pub opened: Moment,
}

/// Commands worth offering in the palette (movement is not).
fn offered(command: Command) -> bool {
    !matches!(
        command,
        Command::Palette
            | Command::Up
            | Command::Down
            | Command::Top
            | Command::Bottom
            | Command::PageUp
            | Command::PageDown
            | Command::Back
    )
}

/// Subsequence match with bonuses for consecutive characters and word
/// starts. `None` when not all characters are found in order.
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut last: Option<usize> = None;
    let mut from = 0;
    for q in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let index = (from..text.len()).find(|&i| text.get(i) == Some(&q))?;
        score += 10;
        if last.is_some_and(|l| l + 1 == index) {
            score += 15;
        }
        let word_start = index == 0 || text.get(index - 1).is_some_and(|c| !c.is_alphanumeric());
        if word_start {
            score += 20;
        }
        score -= i32::try_from(index.saturating_sub(from))
            .unwrap_or(i32::MAX)
            .min(10);
        last = Some(index);
        from = index + 1;
    }
    Some(score)
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `owner/repo#n`, `owner/repo` or `#n`.
/// `(owner, name)` of a jump.
type RepoName = (String, String);

fn parse_jump(input: &str) -> Option<(Option<RepoName>, Option<u64>)> {
    let input = input.trim();
    let (repo, number) = match input.split_once('#') {
        Some((repo, n)) => (repo, Some(n.parse::<u64>().ok()?)),
        None => (input, None),
    };
    let repo = if repo.is_empty() {
        None
    } else {
        let (owner, name) = repo.split_once('/')?;
        if !valid_name(owner) || !valid_name(name) {
            return None;
        }
        Some((owner.to_owned(), name.to_owned()))
    };
    if repo.is_none() && number.is_none() {
        return None;
    }
    Some((repo, number))
}

impl Palette {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            selected: 0,
            opened: Moment::now(),
        }
    }

    /// Matching entries, best first.
    pub fn entries(&self, targets: &[JumpTarget], ctx: &Ctx) -> Vec<Entry> {
        let mut scored: Vec<(i32, Entry)> = Vec::new();

        if let Some((repo, number)) = parse_jump(&self.input) {
            let matches = targets.iter().filter(|t| {
                number.is_none_or(|n| t.item.number == n)
                    && repo.as_ref().is_none_or(|(owner, name)| {
                        t.item.repo.owner.eq_ignore_ascii_case(owner)
                            && t.item.repo.name.eq_ignore_ascii_case(name)
                    })
            });
            for target in matches {
                scored.push((1000, Entry::Jump(target.clone())));
            }
            if let (Some((owner, name)), true) = (&repo, scored.is_empty()) {
                let host = ctx
                    .active_forges()
                    .next()
                    .map(|f| f.account().host.clone())
                    .unwrap_or_else(|| Host::new("github.com"));
                let (label, url) = match number {
                    Some(n) => (
                        format!("open {owner}/{name}#{n} in the browser"),
                        format!("https://{host}/{owner}/{name}/issues/{n}"),
                    ),
                    None => (
                        format!("open {owner}/{name} in the browser"),
                        format!("https://{host}/{owner}/{name}"),
                    ),
                };
                scored.push((900, Entry::Url { label, url }));
            }
        }

        for &command in Command::ALL.iter().filter(|c| offered(**c)) {
            let text = format!("{} {}", command.description(), command.name());
            if let Some(score) = fuzzy(&self.input, &text) {
                scored.push((score, Entry::Command(command)));
            }
        }
        if !self.input.trim().is_empty() && parse_jump(&self.input).is_none() {
            for target in targets {
                let text = format!("{} {}", target.item, target.title);
                if let Some(score) = fuzzy(&self.input, &text) {
                    scored.push((score - 5, Entry::Jump(target.clone())));
                }
            }
        }
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored
            .into_iter()
            .map(|(_, e)| e)
            .take(MAX_ENTRIES)
            .collect()
    }

    pub fn handle(&mut self, key: &KeyEvent, entries: &[Entry]) -> Option<Outcome> {
        // Typed characters first: AltGr (`#` on many layouts) reports
        // ctrl+alt and must not be mistaken for a ctrl shortcut.
        if let Some(c) = typed_char(key) {
            self.input.push(c);
            self.selected = 0;
            return None;
        }
        match key.code {
            KeyCode::Esc => return Some(Outcome::Close),
            KeyCode::Enter => {
                return Some(match entries.get(self.selected) {
                    Some(entry) => Outcome::Run(entry.clone()),
                    None => Outcome::Close,
                });
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(entries.len().saturating_sub(1));
            }
            KeyCode::Char('p' | 'k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Char('n' | 'j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.selected = (self.selected + 1).min(entries.len().saturating_sub(1));
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.selected = 0;
            }
            _ => {}
        }
        None
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, entries: &[Entry], ctx: &Ctx) {
        let t = &ctx.theme;
        let height = (entries.len() as u16 + 4).max(6);
        let inner = Popup {
            title: "command palette",
            footer: Some("↑↓ choose · ↵ run · esc close · try #142 or owner/repo"),
            width: 78,
            height,
            border: t.accent_alt,
            theme: t,
            opened: ctx.animations.then_some(self.opened),
        }
        .render(frame, area);

        let mut lines = vec![
            Line::from(vec![
                Span::styled(" › ", bold(t.accent_alt)),
                Span::styled(self.input.clone(), Style::new().fg(t.fg)),
                Span::styled("▏", bold(t.accent_alt)),
            ]),
            Line::raw(""),
        ];
        let width = usize::from(inner.width);
        for (index, entry) in entries.iter().enumerate() {
            let selected = index == self.selected;
            let (icon, title, right, color) = match entry {
                Entry::Command(command) => (
                    if command.scope() == Scope::Global {
                        "⌘"
                    } else {
                        "›"
                    },
                    command.description().to_owned(),
                    ctx.keymap.label(*command),
                    t.accent,
                ),
                Entry::Jump(target) => (
                    "◈",
                    format!("{}  {}", target.item, target.title),
                    format!("@{}", target.account.login),
                    t.success,
                ),
                Entry::Url { label, .. } => ("↗", label.clone(), "browser".to_owned(), t.info),
            };
            let left = format!(" {icon} {title}");
            let pad = width
                .saturating_sub(left.chars().count() + right.chars().count() + 2)
                .max(1);
            let mut line = Line::from(vec![
                Span::styled(format!(" {icon} "), bold(color)),
                Span::styled(
                    title,
                    if selected {
                        bold(t.fg)
                    } else {
                        Style::new().fg(t.fg)
                    },
                ),
                Span::raw(" ".repeat(pad)),
                Span::styled(right, Style::new().fg(t.muted)),
            ]);
            if selected {
                line = line.style(Style::new().bg(t.selection).add_modifier(Modifier::BOLD));
            }
            lines.push(line);
        }
        if entries.is_empty() {
            lines.push(Line::styled("   no matches", Style::new().fg(t.muted)));
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_word_starts_and_runs() {
        assert!(fuzzy("rf", "Refresh now").is_some());
        assert!(fuzzy("xyz", "Refresh now").is_none());
        let start = fuzzy("open", "Open in the browser").unwrap();
        let scattered = fuzzy("open", "ToP Every Nothing").unwrap_or(i32::MIN);
        assert!(start > scattered);
    }

    #[test]
    fn parses_jumps() {
        assert_eq!(parse_jump("#142"), Some((None, Some(142))));
        assert_eq!(
            parse_jump("acme/rocket#7"),
            Some((Some(("acme".into(), "rocket".into())), Some(7)))
        );
        assert_eq!(
            parse_jump("acme/rocket"),
            Some((Some(("acme".into(), "rocket".into())), None))
        );
        for bad in ["refresh", "#x", "a/b/c", "a b/c", "../x#1", "/x"] {
            assert_eq!(parse_jump(bad), None, "{bad}");
        }
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn altgr_hash_is_typed() {
        let mut palette = Palette::new();
        let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;
        palette.handle(&KeyEvent::new(KeyCode::Char('#'), altgr), &[]);
        palette.handle(&KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE), &[]);
        assert_eq!(palette.input, "#1");
        // A real ctrl shortcut does not type.
        palette.handle(
            &KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            &[],
        );
        assert_eq!(palette.input, "#1");
    }
}
