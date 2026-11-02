//! A conversation: markdown description, then comments and reviews in time
//! order. Shared by the pull request and issue screens.

use hit_core::{
    ForgeError, Timestamp,
    model::{Comment, Review, ReviewState, ReviewThread},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Wrap},
};

use crate::{
    Ctx,
    keymap::Command,
    markdown,
    screens::spinner,
    sync,
    ui::{chips::bold, wrapped_height},
};

/// Width from which the item's fields sit to the right of the conversation.
const SIDE_MIN_WIDTH: u16 = 120;
const SIDE_WIDTH: u16 = 44;

/// What to show while the conversation isn't there yet.
pub fn placeholder(error: Option<&ForgeError>, ctx: &Ctx) -> Vec<Line<'static>> {
    let t = &ctx.theme;
    match error {
        Some(e) => vec![Line::styled(
            format!("couldn't load: {e}"),
            Style::new().fg(t.danger),
        )],
        None => vec![Line::from(vec![
            Span::styled(spinner(ctx), bold(t.accent)),
            Span::styled(" loading the conversation…", Style::new().fg(t.muted)),
        ])],
    }
}

/// Comments on lines of the diff: what is still open in full, what is
/// resolved in one line.
fn review_thread_lines(threads: &[ReviewThread], ctx: &Ctx) -> Vec<Line<'static>> {
    let t = &ctx.theme;
    let muted = Style::new().fg(t.muted);
    let mut lines = Vec::new();
    let open = threads.iter().filter(|thread| !thread.resolved).count();
    lines.push(Line::from(vec![
        Span::styled("Review comments", bold(t.accent_alt)),
        Span::styled(format!("  {open} open"), muted),
    ]));
    let place = |thread: &ReviewThread| match thread.line {
        Some(line) => format!("{}:{line}", thread.path),
        None => thread.path.clone(),
    };
    for thread in threads.iter().filter(|thread| !thread.resolved) {
        let mut head = vec![
            Span::styled("◐ ", bold(t.warning)),
            Span::styled(place(thread), bold(t.fg)),
        ];
        if thread.outdated {
            head.push(Span::styled("  outdated", muted));
        }
        lines.push(Line::from(head));
        for comment in &thread.comments {
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::new().fg(t.border)),
                Span::styled(format!("@{}", comment.author.login), bold(t.fg)),
                Span::styled(format!(" · {}", sync::ago(comment.created_at)), muted),
            ]));
            for line in markdown::render(&comment.body, t) {
                let mut spans = vec![Span::styled("│ ", Style::new().fg(t.border))];
                spans.extend(line.spans);
                lines.push(Line::from(spans));
            }
        }
    }
    for thread in threads.iter().filter(|thread| thread.resolved) {
        lines.push(Line::from(vec![
            Span::styled("✓ ", bold(t.success)),
            Span::styled(format!("{} · resolved", place(thread)), muted),
        ]));
    }
    lines.push(Line::raw(""));
    lines
}

/// The description, comments on lines, then comments and reviews, oldest
/// first. `earlier` comments did not fit and are on the forge.
pub fn lines(
    body: &str,
    comments: &[Comment],
    earlier: u32,
    reviews: &[Review],
    review_threads: &[ReviewThread],
    ctx: &Ctx,
) -> Vec<Line<'static>> {
    let t = &ctx.theme;
    let muted = Style::new().fg(t.muted);
    let mut lines = Vec::new();
    let description = markdown::render(body, t);
    if description.is_empty() {
        lines.push(Line::styled("No description provided.", muted));
    } else {
        lines.extend(description);
    }
    lines.push(Line::raw(""));
    if !review_threads.is_empty() {
        lines.extend(review_thread_lines(review_threads, ctx));
    }
    if earlier > 0 {
        lines.push(Line::styled(
            format!(
                "··· {earlier} earlier comments · {} opens them in the browser",
                ctx.keymap.short(Command::Browser)
            ),
            muted,
        ));
        lines.push(Line::raw(""));
    }

    enum Event<'a> {
        Comment(&'a Comment),
        Review(&'a Review),
    }
    let mut events: Vec<(Timestamp, Event)> = comments
        .iter()
        .map(|c| (c.created_at, Event::Comment(c)))
        .chain(
            reviews
                .iter()
                .filter_map(|r| r.submitted_at.map(|at| (at, Event::Review(r)))),
        )
        .collect();
    events.sort_by_key(|(at, _)| *at);

    if events.is_empty() {
        lines.push(Line::styled("No comments yet.", muted));
    }
    for (at, event) in events {
        let (author, verb, color, body) = match event {
            Event::Comment(c) => (&c.author.login, "commented", t.accent, c.body.as_str()),
            Event::Review(r) => {
                let (verb, color) = match r.state {
                    ReviewState::Approved => ("approved", t.success),
                    ReviewState::ChangesRequested => ("requested changes", t.warning),
                    ReviewState::Commented => ("reviewed", t.info),
                    ReviewState::Dismissed => ("review dismissed", t.muted),
                    ReviewState::Pending => ("pending review", t.muted),
                };
                (&r.author.login, verb, color, r.body.as_str())
            }
        };
        lines.push(Line::from(vec![
            Span::styled("● ", bold(color)),
            Span::styled(format!("@{author}"), bold(t.fg)),
            Span::styled(format!(" {verb}"), bold(color)),
            Span::styled(format!(" · {}", sync::ago(at)), muted),
        ]));
        for line in markdown::render(body, t) {
            let mut spans = vec![Span::styled("│ ", Style::new().fg(t.border))];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
        lines.push(Line::raw(""));
    }
    lines
}

/// Draws a scrollable conversation; on wide screens `side` (the item's
/// fields) goes to the right. Scrolling stops at the last line.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    lines: Vec<Line<'static>>,
    side: Option<Vec<Line<'static>>>,
    scroll: &mut u16,
    dim: bool,
    ctx: &Ctx,
) {
    let t = &ctx.theme;
    let main = match side {
        Some(mut side) if area.width >= SIDE_MIN_WIDTH => {
            let [main, side_area] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Length(SIDE_WIDTH)])
                    .areas(area);
            side.insert(0, Line::from(Span::styled("Details", bold(t.accent_alt))));
            frame.render_widget(
                Paragraph::new(side).wrap(Wrap { trim: true }).block(
                    Block::new()
                        .borders(Borders::LEFT)
                        .border_style(Style::new().fg(t.border))
                        .padding(Padding::left(1)),
                ),
                side_area,
            );
            main
        }
        _ => area,
    };
    let main = Rect {
        width: main.width.saturating_sub(1),
        ..main
    };
    let max = wrapped_height(&lines, main.width).saturating_sub(main.height);
    *scroll = (*scroll).min(max);
    let mut paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((*scroll, 0));
    if dim {
        paragraph = paragraph.style(Style::new().add_modifier(Modifier::DIM));
    }
    frame.render_widget(paragraph, main);
}
