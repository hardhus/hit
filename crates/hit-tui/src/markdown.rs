//! A small Markdown → styled lines renderer for descriptions and comments.
//!
//! Covers what PR and issue bodies use: headings, emphasis, inline and
//! fenced code, lists (with task boxes), quotes, links and rules. HTML
//! (e.g. `<!-- template comments -->`) is dropped.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::Theme;

struct Renderer<'t> {
    theme: &'t Theme,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    quote: usize,
    in_code: bool,
}

impl<'t> Renderer<'t> {
    fn new(theme: &'t Theme) -> Self {
        Self {
            theme,
            lines: Vec::new(),
            current: Vec::new(),
            styles: vec![Style::new().fg(theme.fg)],
            lists: Vec::new(),
            quote: 0,
            in_code: false,
        }
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        let next = f(self.style());
        self.styles.push(next);
    }

    fn prefix(&self) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for _ in 0..self.quote {
            spans.push(Span::styled("▎ ", Style::new().fg(self.theme.border)));
        }
        spans
    }

    fn flush(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let mut spans = self.prefix();
        spans.append(&mut self.current);
        self.lines.push(Line::from(spans));
    }

    fn blank(&mut self) {
        self.flush();
        if self.lines.last().is_some_and(|l| l.width() > 0) {
            self.lines.push(Line::raw(""));
        }
    }

    fn text(&mut self, text: &str) {
        if self.in_code {
            let code = Style::new().fg(self.theme.info);
            for line in text.lines() {
                let mut spans = self.prefix();
                spans.push(Span::styled("│ ", Style::new().fg(self.theme.border)));
                spans.push(Span::styled(line.to_owned(), code));
                self.lines.push(Line::from(spans));
            }
            return;
        }
        let style = self.style();
        self.current.push(Span::styled(text.to_owned(), style));
    }

    fn run(mut self, markdown: &str) -> Vec<Line<'static>> {
        let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        let t = self.theme;
        for event in Parser::new_ext(markdown, options) {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Heading { level, .. } => {
                        self.blank();
                        let color = if level == HeadingLevel::H1 {
                            t.accent_alt
                        } else {
                            t.accent
                        };
                        self.push_style(|_| Style::new().fg(color).add_modifier(Modifier::BOLD));
                    }
                    Tag::Paragraph => {}
                    Tag::BlockQuote(_) => {
                        self.blank();
                        self.quote += 1;
                        self.push_style(|s| s.fg(t.muted));
                    }
                    Tag::CodeBlock(_) => {
                        self.blank();
                        self.in_code = true;
                    }
                    Tag::List(start) => {
                        self.flush();
                        self.lists.push(start);
                    }
                    Tag::Item => {
                        self.flush();
                        let depth = self.lists.len().saturating_sub(1);
                        let marker = match self.lists.last_mut() {
                            Some(Some(n)) => {
                                let marker = format!("{n}. ");
                                *n += 1;
                                marker
                            }
                            _ => "• ".to_owned(),
                        };
                        self.current.push(Span::raw("  ".repeat(depth)));
                        self.current
                            .push(Span::styled(marker, Style::new().fg(t.accent_alt)));
                    }
                    Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
                    Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
                    Tag::Strikethrough => {
                        self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT));
                    }
                    Tag::Link { .. } => {
                        self.push_style(|s| s.fg(t.accent).add_modifier(Modifier::UNDERLINED));
                    }
                    Tag::Image { .. } => {
                        self.current
                            .push(Span::styled("[image] ", Style::new().fg(t.muted)));
                        self.push_style(|s| s.fg(t.muted));
                    }
                    _ => self.push_style(|s| s),
                },
                Event::End(tag) => match tag {
                    TagEnd::Paragraph | TagEnd::Heading(_) => {
                        if matches!(tag, TagEnd::Heading(_)) {
                            self.styles.pop();
                        }
                        self.blank();
                    }
                    TagEnd::BlockQuote(_) => {
                        self.flush();
                        self.quote = self.quote.saturating_sub(1);
                        self.styles.pop();
                        self.blank();
                    }
                    TagEnd::CodeBlock => {
                        self.in_code = false;
                        self.blank();
                    }
                    TagEnd::List(_) => {
                        self.flush();
                        self.lists.pop();
                        if self.lists.is_empty() {
                            self.blank();
                        }
                    }
                    TagEnd::Item => self.flush(),
                    _ => {
                        if self.styles.len() > 1 {
                            self.styles.pop();
                        }
                    }
                },
                Event::Text(text) => self.text(&text),
                Event::Code(code) => self
                    .current
                    .push(Span::styled(format!("`{code}`"), Style::new().fg(t.info))),
                Event::SoftBreak => self.current.push(Span::raw(" ")),
                Event::HardBreak => self.flush(),
                Event::Rule => {
                    self.blank();
                    self.lines
                        .push(Line::styled("─".repeat(40), Style::new().fg(t.border)));
                    self.lines.push(Line::raw(""));
                }
                Event::TaskListMarker(done) => self.current.push(if done {
                    Span::styled("☑ ", Style::new().fg(t.success))
                } else {
                    Span::styled("☐ ", Style::new().fg(t.muted))
                }),
                // HTML (PR template comments, details tags) is dropped.
                Event::Html(_) | Event::InlineHtml(_) => {}
                _ => {}
            }
        }
        self.flush();
        while self.lines.last().is_some_and(|l| l.width() == 0) {
            self.lines.pop();
        }
        self.lines
    }
}

pub fn render(markdown: &str, theme: &Theme) -> Vec<Line<'static>> {
    Renderer::new(theme).run(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn renders_common_markdown() {
        let theme = Theme::jinx(true, true);
        let md = "## Why\n\nUploads **fail** with `503`.\n\n<!-- template -->\n\n- [x] retry\n- [ ] test\n\n```rust\nlet a = 1;\n```\n\n> quoted";
        let lines = render(md, &theme);
        let text = text(&lines);
        assert_eq!(text[0], "Why");
        assert!(
            text.contains(&"Uploads fail with `503`.".to_owned()),
            "{text:?}"
        );
        assert!(text.iter().any(|l| l.contains("☑ retry")));
        assert!(text.iter().any(|l| l.contains("☐ test")));
        assert!(text.iter().any(|l| l.contains("│ let a = 1;")));
        assert!(text.iter().any(|l| l.contains("▎ quoted")));
        assert!(!text.iter().any(|l| l.contains("template")));
    }

    #[test]
    fn empty_body_is_empty() {
        assert!(render("", &Theme::jinx(true, true)).is_empty());
    }
}
