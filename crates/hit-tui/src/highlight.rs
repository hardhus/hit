//! Syntax highlighting for diffs, in hit's own colors.
//!
//! The syntect theme is built from the hit theme (keywords pink, strings
//! green, functions blue…), so code looks like the rest of the UI. Without
//! truecolor, lines are left plain.

use std::{str::FromStr, sync::OnceLock};

use hit_core::diff::{Diff, LineKind};
use ratatui::style::Color;
use syntect::{
    easy::HighlightLines,
    highlighting::{
        Color as SynColor, ScopeSelectors, StyleModifier, Theme as SynTheme, ThemeItem,
        ThemeSettings,
    },
    parsing::SyntaxSet,
};

use crate::Theme;

/// A run of text in one color.
pub type Segment = (Color, String);

/// A diff with every line split into colored segments:
/// `files[f].hunks[h][l]`, matching `diff.files[f].hunks[h].lines[l]`.
#[derive(Debug, Clone)]
pub struct HighlightedDiff {
    pub diff: Diff,
    pub files: Vec<FileSegments>,
}

#[derive(Debug, Clone, Default)]
pub struct FileSegments {
    pub hunks: Vec<Vec<Vec<Segment>>>,
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn rgb(color: Color) -> Option<SynColor> {
    match color {
        Color::Rgb(r, g, b) => Some(SynColor { r, g, b, a: 0xff }),
        _ => None,
    }
}

/// A syntect theme from hit's palette; `None` without truecolor.
fn syntax_theme(t: &Theme) -> Option<SynTheme> {
    let item = |selector: &str, color: Color| -> Option<ThemeItem> {
        Some(ThemeItem {
            scope: ScopeSelectors::from_str(selector).ok()?,
            style: StyleModifier {
                foreground: rgb(color),
                background: None,
                font_style: None,
            },
        })
    };
    let scopes = [
        ("comment, punctuation.definition.comment", t.muted),
        ("string, punctuation.definition.string", t.success),
        ("constant.numeric, constant.language, constant.character, constant.other", t.warning),
        ("keyword, storage.modifier, keyword.operator.word", t.accent_alt),
        ("storage.type, keyword.declaration", t.highlight),
        ("entity.name.function, support.function, meta.function-call entity.name", t.accent),
        ("entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, support.type, support.class", t.info),
        ("entity.name.tag", t.accent_alt),
        ("entity.other.attribute-name, variable.other.member", t.warning),
        ("markup.heading", t.accent_alt),
        ("variable.language, variable.parameter", t.highlight),
    ]
    .into_iter()
    .filter_map(|(s, c)| item(s, c))
    .collect();
    Some(SynTheme {
        settings: ThemeSettings {
            foreground: Some(rgb(t.fg)?),
            ..ThemeSettings::default()
        },
        scopes,
        ..SynTheme::default()
    })
}

fn plain(text: &str, color: Color) -> Vec<Segment> {
    vec![(color, text.to_owned())]
}

/// Highlights every line of `diff`. CPU-bound: run it off the UI thread.
pub fn highlight_diff(diff: Diff, theme: &Theme) -> HighlightedDiff {
    let synth = syntax_theme(theme);
    let set = syntaxes();
    let files = diff
        .files
        .iter()
        .map(|file| {
            let syntax = std::path::Path::new(file.path())
                .extension()
                .and_then(|e| e.to_str())
                .and_then(|ext| set.find_syntax_by_extension(ext));
            let mut highlighters = match (&synth, syntax) {
                (Some(theme), Some(syntax)) => Some((
                    HighlightLines::new(syntax, theme),
                    HighlightLines::new(syntax, theme),
                )),
                _ => None,
            };
            let mut paint = |text: &str, old: bool| -> Vec<Segment> {
                let Some((new_hl, old_hl)) = highlighters.as_mut() else {
                    return plain(text, theme.fg);
                };
                let hl = if old { old_hl } else { new_hl };
                let line = format!("{text}\n");
                match hl.highlight_line(&line, set) {
                    Ok(regions) => regions
                        .into_iter()
                        .map(|(style, piece)| {
                            let f = style.foreground;
                            (
                                Color::Rgb(f.r, f.g, f.b),
                                piece.trim_end_matches('\n').to_owned(),
                            )
                        })
                        .filter(|(_, piece)| !piece.is_empty())
                        .collect(),
                    Err(_) => plain(text, theme.fg),
                }
            };
            let hunks = file
                .hunks
                .iter()
                .map(|hunk| {
                    hunk.lines
                        .iter()
                        .map(|line| match line.kind {
                            LineKind::Context => {
                                // Keep both sides' parser state in step.
                                let _ = paint(&line.text, true);
                                paint(&line.text, false)
                            }
                            LineKind::Added => paint(&line.text, false),
                            LineKind::Removed => paint(&line.text, true),
                            LineKind::NoNewline => plain(&line.text, theme.muted),
                        })
                        .collect()
                })
                .collect();
            FileSegments { hunks }
        })
        .collect();
    HighlightedDiff { diff, files }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_code_gets_theme_colors() {
        let diff = Diff::parse_unified(
            "--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-let a = 1;\n+fn main() { let s = \"hi\"; }\n",
        )
        .unwrap();
        let theme = Theme::jinx(true, true);
        let highlighted = highlight_diff(diff, &theme);
        let added = &highlighted.files[0].hunks[0][1];
        let text: String = added.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(text, "fn main() { let s = \"hi\"; }");
        let colors: Vec<Color> = added.iter().map(|(c, _)| *c).collect();
        assert!(
            colors.contains(&theme.success),
            "strings are green: {added:?}"
        );
        assert!(colors.contains(&theme.highlight) || colors.contains(&theme.accent_alt));
    }

    #[test]
    fn unknown_files_and_16_colors_stay_plain() {
        let diff = Diff::parse_unified("--- a/x.zzz\n+++ b/x.zzz\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
        let highlighted = highlight_diff(diff.clone(), &Theme::jinx(true, true));
        assert_eq!(highlighted.files[0].hunks[0][1].len(), 1);
        let diff = Diff::parse_unified("--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
        let highlighted = highlight_diff(diff, &Theme::jinx(true, false));
        assert_eq!(highlighted.files[0].hunks[0][1].len(), 1);
    }
}
