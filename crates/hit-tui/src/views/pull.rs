//! A pull request, full screen: summary on top, then Conversation, Files
//! (diff) and Checks.

use std::{sync::Arc, time::Duration};

use hit_core::{
    AccountId, ForgeError, ItemRef,
    diff::{FileStatus, LineKind},
    model::{CheckState, PullRequest, PullRequestDetail, ReviewState},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use crate::{
    Action, Ctx, DataEvent, Event, View,
    action::Toast,
    anim::{Moment, Tween},
    browser,
    highlight::HighlightedDiff,
    keymap::{Command, Scope},
    markdown,
    screens::{pr_fields, pr_icon},
    sync::{self, Loaded},
    ui::{
        chips::{self, bold},
        tabs::{TabBar, TabSpec},
        wrapped_height,
    },
};

const GLIDE: Duration = Duration::from_millis(200);
const FADE: Duration = Duration::from_millis(120);
/// Width at which the side-by-side diff is allowed.
const SPLIT_MIN_WIDTH: u16 = 140;
/// Line numbers and sign in front of the code: `1234 1234 + `.
const UNIFIED_GUTTER: usize = 12;
/// One line number in front of each side of a split diff: `1234 `.
const SPLIT_GUTTER: usize = 5;
/// Columns moved by one `h` / `l`.
const HSCROLL_STEP: u16 = 8;

/// The highlighted code of one line with its first `skip` columns cut off.
fn code_spans(segments: &[(Color, String)], skip: u16) -> Vec<Span<'static>> {
    let mut skip = usize::from(skip);
    let mut spans = Vec::new();
    for (color, text) in segments {
        let len = text.chars().count();
        if skip >= len {
            skip -= len;
            continue;
        }
        let text: String = text.chars().skip(skip).collect();
        skip = 0;
        spans.push(Span::styled(text, Style::new().fg(*color)));
    }
    spans
}

/// The widest line of code in one file, in columns.
fn code_width(diff: &HighlightedDiff, file: usize) -> u16 {
    diff.files
        .get(file)
        .into_iter()
        .flat_map(|f| f.hunks.iter().flatten())
        .map(|segs| segs.iter().map(|(_, t)| t.chars().count()).sum::<usize>())
        .max()
        .map_or(0, |w| u16::try_from(w).unwrap_or(u16::MAX))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Conversation,
    Files,
    Checks,
}

const PANES: [Pane; 3] = [Pane::Conversation, Pane::Files, Pane::Checks];

pub struct PullView {
    account: AccountId,
    item: ItemRef,
    summary: Option<PullRequest>,
    detail: Option<Loaded<Arc<PullRequestDetail>>>,
    detail_error: Option<ForgeError>,
    diff: Option<Result<Arc<HighlightedDiff>, ForgeError>>,
    task: Option<JoinHandle<()>>,
    pane: usize,
    underline: (Tween, Tween),
    positions: Vec<(u16, u16)>,
    switched: Moment,
    opened: Moment,
    conversation_scroll: u16,
    file: usize,
    diff_scroll: u16,
    /// Rendered-line offsets of each hunk header in the current file.
    hunk_offsets: Vec<u16>,
    /// Columns of code scrolled off to the left; both split sides share it.
    diff_hscroll: u16,
    /// `{` stepped back into the previous file: land on its last change once
    /// that file's offsets are known.
    to_last_hunk: bool,
    /// The change `}` / `{` last landed on. Short files can't scroll a later
    /// change to the top, so the scroll position alone can't say where you
    /// are; plain scrolling clears it.
    hunk: Option<usize>,
    split: bool,
    check: usize,
}

impl PullView {
    pub fn new(account: AccountId, item: ItemRef, summary: Option<PullRequest>) -> Self {
        Self {
            account,
            item,
            summary,
            detail: None,
            detail_error: None,
            diff: None,
            task: None,
            pane: 0,
            underline: (Tween::settled(f32::NAN), Tween::settled(0.0)),
            positions: Vec::new(),
            switched: Moment::now(),
            opened: Moment::now(),
            conversation_scroll: 0,
            file: 0,
            diff_scroll: 0,
            hunk_offsets: Vec::new(),
            diff_hscroll: 0,
            to_last_hunk: false,
            hunk: None,
            split: false,
            check: 0,
        }
    }

    fn pr(&self) -> Option<&PullRequest> {
        self.detail
            .as_ref()
            .map(|d| &d.value.pr)
            .or(self.summary.as_ref())
    }

    fn pane(&self) -> Pane {
        PANES.get(self.pane).copied().unwrap_or(Pane::Conversation)
    }

    fn switch(&mut self, index: usize, ctx: &Ctx) {
        if index >= PANES.len() || index == self.pane {
            return;
        }
        self.pane = index;
        self.switched = Moment::now();
        if let Some(&(x, w)) = self.positions.get(index) {
            let duration = if ctx.animations {
                GLIDE
            } else {
                Duration::ZERO
            };
            self.underline.0.retarget(f32::from(x), duration);
            self.underline.1.retarget(f32::from(w), duration);
        }
    }

    fn files(&self) -> Option<&HighlightedDiff> {
        match &self.diff {
            Some(Ok(diff)) => Some(diff),
            _ => None,
        }
    }

    fn checks_len(&self) -> usize {
        self.detail.as_ref().map_or(0, |d| d.value.checks.len())
    }

    fn header(&self, ctx: &Ctx) -> Vec<Line<'static>> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let Some(pr) = self.pr() else {
            return vec![Line::from(vec![
                Span::styled(crate::screens::spinner(ctx), bold(t.accent)),
                Span::styled(format!(" loading {}…", self.item), muted),
            ])];
        };
        let mut meta = vec![
            pr_icon(pr, ctx),
            Span::styled(format!(" {}", pr.reference), bold(t.accent)),
            Span::styled(
                format!(
                    "  @{} wants to merge {} → {}",
                    pr.author.login, pr.head_ref, pr.base_ref
                ),
                muted,
            ),
        ];
        if ctx.multi_account() {
            meta.push(Span::styled(
                format!("  @{}", self.account.login),
                bold(ctx.account_color(&self.account)),
            ));
        }
        let mut status = crate::screens::pr_status_spans(pr, ctx);
        status.insert(0, Span::raw(""));
        vec![
            Line::from(Span::styled(pr.title.clone(), bold(t.fg))),
            Line::from(meta),
            Line::from(status),
        ]
    }

    fn conversation_lines(&self, ctx: &Ctx) -> Vec<Line<'static>> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let Some(detail) = &self.detail else {
            return match &self.detail_error {
                Some(e) => vec![Line::styled(
                    format!("couldn't load: {e}"),
                    Style::new().fg(t.danger),
                )],
                None => vec![Line::from(vec![
                    Span::styled(crate::screens::spinner(ctx), bold(t.accent)),
                    Span::styled(" loading the conversation…", muted),
                ])],
            };
        };
        let detail = &detail.value;
        let mut lines = Vec::new();
        let body = markdown::render(&detail.body, t);
        if body.is_empty() {
            lines.push(Line::styled("No description provided.", muted));
        } else {
            lines.extend(body);
        }
        lines.push(Line::raw(""));

        // Comments and reviews, oldest first.
        enum Event<'a> {
            Comment(&'a hit_core::model::Comment),
            Review(&'a hit_core::model::Review),
        }
        let mut events: Vec<(hit_core::Timestamp, Event)> = detail
            .comments
            .iter()
            .map(|c| (c.created_at, Event::Comment(c)))
            .chain(
                detail
                    .reviews
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

    fn render_conversation(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let mut lines = self.conversation_lines(ctx);
        if area.width >= 120
            && let Some(pr) = self.pr()
        {
            // Wide: the PR's fields to the right of the conversation.
            let [main, side] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Length(44)]).areas(area);
            let mut side_lines = pr_fields(pr, ctx);
            side_lines.insert(
                0,
                Line::from(Span::styled("Details", bold(ctx.theme.accent_alt))),
            );
            frame.render_widget(
                Paragraph::new(side_lines).wrap(Wrap { trim: true }).block(
                    Block::new()
                        .borders(ratatui::widgets::Borders::LEFT)
                        .border_style(Style::new().fg(ctx.theme.border))
                        .padding(ratatui::widgets::Padding::left(1)),
                ),
                side,
            );
            self.render_scrolled(frame, main, &mut lines, ctx, true);
        } else {
            self.render_scrolled(frame, area, &mut lines, ctx, true);
        }
    }

    fn render_scrolled(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        lines: &mut Vec<Line<'static>>,
        ctx: &Ctx,
        wrap: bool,
    ) {
        let area = Rect {
            width: area.width.saturating_sub(1),
            ..area
        };
        let max = wrapped_height(lines, area.width).saturating_sub(area.height);
        self.conversation_scroll = self.conversation_scroll.min(max);
        let mut paragraph =
            Paragraph::new(std::mem::take(lines)).scroll((self.conversation_scroll, 0));
        if wrap {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        if self.switched.within(FADE) && ctx.animations {
            paragraph = paragraph.style(Style::new().add_modifier(Modifier::DIM));
        }
        frame.render_widget(paragraph, area);
    }

    fn file_icon(status: FileStatus, ctx: &Ctx) -> Span<'static> {
        let t = &ctx.theme;
        match status {
            FileStatus::Added => Span::styled("+", bold(t.success)),
            FileStatus::Deleted => Span::styled("−", bold(t.danger)),
            FileStatus::Modified => Span::styled("~", bold(t.warning)),
            FileStatus::Renamed => Span::styled("→", bold(t.info)),
        }
    }

    /// `▶` in front of the change `}` / `{` is on.
    fn hunk_marker(current: bool, ctx: &Ctx) -> Span<'static> {
        if current {
            Span::styled("▶", bold(ctx.theme.accent_alt))
        } else {
            Span::raw(" ")
        }
    }

    fn line_bg(kind: LineKind, ctx: &Ctx) -> Option<Color> {
        if !matches!(ctx.theme.fg, Color::Rgb(..)) {
            return None;
        }
        match kind {
            LineKind::Added => Some(Color::Rgb(0x12, 0x3a, 0x26)),
            LineKind::Removed => Some(Color::Rgb(0x45, 0x14, 0x28)),
            _ => None,
        }
    }

    /// Unified diff lines for the current file, and the hunk header offsets.
    fn unified_lines(&self, diff: &HighlightedDiff, ctx: &Ctx) -> (Vec<Line<'static>>, Vec<u16>) {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let mut lines = Vec::new();
        let mut offsets = Vec::new();
        let (Some(file), Some(segments)) =
            (diff.diff.files.get(self.file), diff.files.get(self.file))
        else {
            return (lines, offsets);
        };
        if file.binary {
            lines.push(Line::styled("binary file, not shown", muted));
        } else if file.hunks.is_empty() {
            lines.push(Line::styled("renamed without content changes", muted));
        }
        let current = self.current_hunk();
        for (h, hunk) in file.hunks.iter().enumerate() {
            offsets.push(u16::try_from(lines.len()).unwrap_or(u16::MAX));
            lines.push(
                Line::from(vec![
                    Self::hunk_marker(current == Some(h), ctx),
                    Span::styled(
                        format!(
                            " @@ -{},{} +{},{} @@ ",
                            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
                        ),
                        bold(t.info),
                    ),
                    Span::styled(hunk.section.clone(), muted),
                ])
                .style(Style::new().bg(t.selection)),
            );
            for (l, line) in hunk.lines.iter().enumerate() {
                let num =
                    |n: Option<u32>| n.map_or_else(|| "    ".to_owned(), |n| format!("{n:>4}"));
                let (sign, sign_color) = match line.kind {
                    LineKind::Added => ("+", t.success),
                    LineKind::Removed => ("-", t.danger),
                    LineKind::Context => (" ", t.muted),
                    LineKind::NoNewline => ("\\", t.muted),
                };
                let mut spans = vec![
                    Span::styled(num(line.old_no), muted),
                    Span::styled(" ", muted),
                    Span::styled(num(line.new_no), muted),
                    Span::styled(format!(" {sign} "), bold(sign_color)),
                ];
                if let Some(segs) = segments.hunks.get(h).and_then(|hs| hs.get(l)) {
                    spans.extend(code_spans(segs, self.diff_hscroll));
                }
                let mut rendered = Line::from(spans);
                if let Some(bg) = Self::line_bg(line.kind, ctx) {
                    rendered = rendered.style(Style::new().bg(bg));
                }
                lines.push(rendered);
            }
        }
        (lines, offsets)
    }

    /// Side-by-side: removed lines on the left, added on the right, context on
    /// both. Returns (left, right, hunk offsets).
    fn split_lines(
        &self,
        diff: &HighlightedDiff,
        ctx: &Ctx,
    ) -> (Vec<Line<'static>>, Vec<Line<'static>>, Vec<u16>) {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let (mut left, mut right, mut offsets) = (Vec::new(), Vec::new(), Vec::new());
        let (Some(file), Some(segments)) =
            (diff.diff.files.get(self.file), diff.files.get(self.file))
        else {
            return (left, right, offsets);
        };
        let side = |no: Option<u32>, kind: LineKind, segs: &[(Color, String)]| {
            let mut spans = vec![Span::styled(
                no.map_or_else(|| " ".repeat(SPLIT_GUTTER), |n| format!("{n:>4} ")),
                muted,
            )];
            spans.extend(code_spans(segs, self.diff_hscroll));
            let mut line = Line::from(spans);
            if let Some(bg) = Self::line_bg(kind, ctx) {
                line = line.style(Style::new().bg(bg));
            }
            line
        };
        let current = self.current_hunk();
        for (h, hunk) in file.hunks.iter().enumerate() {
            offsets.push(u16::try_from(left.len()).unwrap_or(u16::MAX));
            let header = Line::from(vec![
                Self::hunk_marker(current == Some(h), ctx),
                Span::styled(format!(" @@ {} ", hunk.section), bold(t.info)),
            ])
            .style(Style::new().bg(t.selection));
            left.push(header.clone());
            right.push(header);
            let segs = |l: usize| -> &[(Color, String)] {
                segments
                    .hunks
                    .get(h)
                    .and_then(|hs| hs.get(l))
                    .map_or(&[], Vec::as_slice)
            };
            let mut removed = Vec::new();
            let mut added = Vec::new();
            let flush = |removed: &mut Vec<usize>,
                         added: &mut Vec<usize>,
                         left: &mut Vec<Line<'static>>,
                         right: &mut Vec<Line<'static>>| {
                for i in 0..removed.len().max(added.len()) {
                    left.push(match removed.get(i) {
                        Some(&l) => side(hunk.lines[l].old_no, LineKind::Removed, segs(l)),
                        None => Line::raw(""),
                    });
                    right.push(match added.get(i) {
                        Some(&l) => side(hunk.lines[l].new_no, LineKind::Added, segs(l)),
                        None => Line::raw(""),
                    });
                }
                removed.clear();
                added.clear();
            };
            for (l, line) in hunk.lines.iter().enumerate() {
                match line.kind {
                    LineKind::Removed => removed.push(l),
                    LineKind::Added => added.push(l),
                    LineKind::Context => {
                        flush(&mut removed, &mut added, &mut left, &mut right);
                        left.push(side(line.old_no, LineKind::Context, segs(l)));
                        right.push(side(line.new_no, LineKind::Context, segs(l)));
                    }
                    LineKind::NoNewline => {}
                }
            }
            flush(&mut removed, &mut added, &mut left, &mut right);
        }
        (left, right, offsets)
    }

    fn render_files(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let Some(result) = &self.diff else {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(crate::screens::spinner(ctx), bold(t.accent)),
                    Span::styled(" loading the diff…", muted),
                ])),
                area,
            );
            return;
        };
        let diff = match result {
            Ok(diff) => Arc::clone(diff),
            Err(e) => {
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        format!("couldn't load the diff: {e}"),
                        Style::new().fg(t.danger),
                    )),
                    area,
                );
                return;
            }
        };
        if diff.diff.files.is_empty() {
            frame.render_widget(Paragraph::new(Line::styled("no changes", muted)), area);
            return;
        }
        self.file = self.file.min(diff.diff.files.len() - 1);

        let tree_width = (area.width / 3).clamp(24, 44);
        let [tree, _, body] = Layout::horizontal([
            Constraint::Length(tree_width),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);

        // File tree.
        let mut tree_lines = Vec::new();
        for (index, file) in diff.diff.files.iter().enumerate() {
            let selected = index == self.file;
            let mut top = Line::from(vec![
                Span::styled(if selected { "▌" } else { " " }, bold(t.accent_alt)),
                Self::file_icon(file.status, ctx),
                Span::raw(" "),
                Span::styled(
                    file.path().to_owned(),
                    if selected {
                        bold(t.fg)
                    } else {
                        Style::new().fg(t.fg)
                    },
                ),
            ]);
            let mut bottom = Line::from(vec![
                Span::raw("   "),
                Span::styled(format!("+{}", file.additions), Style::new().fg(t.success)),
                Span::styled(format!(" −{}", file.deletions), Style::new().fg(t.danger)),
            ]);
            if selected {
                top = top.style(Style::new().bg(t.selection));
                bottom = bottom.style(Style::new().bg(t.selection));
            }
            tree_lines.push(top);
            tree_lines.push(bottom);
        }
        let tree_scroll = (self.file as u16 * 2).saturating_sub(tree.height.saturating_sub(2));
        frame.render_widget(Paragraph::new(tree_lines).scroll((tree_scroll, 0)), tree);
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled("│", Style::new().fg(t.border));
                usize::from(body.height)
            ]),
            Rect {
                x: tree.right(),
                width: 1,
                ..tree
            },
        );

        // The diff of the selected file, under a line saying where you are.
        let [title, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(body);
        let split = self.split && body.width >= SPLIT_MIN_WIDTH - tree_width;
        let fade = self.switched.within(FADE) && ctx.animations;
        let [l, _, r] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(body);
        // Only scroll sideways as far as the widest line needs.
        let visible = if split {
            l.width.saturating_sub(SPLIT_GUTTER as u16)
        } else {
            body.width.saturating_sub(UNIFIED_GUTTER as u16)
        };
        self.diff_hscroll = self
            .diff_hscroll
            .min(code_width(&diff, self.file).saturating_sub(visible));
        if split {
            let (left, right, offsets) = self.split_lines(&diff, ctx);
            self.set_offsets(offsets);
            let max = (left.len() as u16).saturating_sub(body.height);
            self.diff_scroll = self.diff_scroll.min(max);
            for (lines, rect) in [(left, l), (right, r)] {
                let mut p = Paragraph::new(lines).scroll((self.diff_scroll, 0));
                if fade {
                    p = p.style(Style::new().add_modifier(Modifier::DIM));
                }
                frame.render_widget(p, rect);
            }
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled("│", Style::new().fg(t.border));
                    usize::from(body.height)
                ]),
                Rect {
                    x: l.right(),
                    width: 1,
                    ..body
                },
            );
        } else {
            let (lines, offsets) = self.unified_lines(&diff, ctx);
            self.set_offsets(offsets);
            let max = (lines.len() as u16).saturating_sub(body.height);
            self.diff_scroll = self.diff_scroll.min(max);
            let mut p = Paragraph::new(lines).scroll((self.diff_scroll, 0));
            if fade {
                p = p.style(Style::new().add_modifier(Modifier::DIM));
            }
            frame.render_widget(p, body);
        }
        frame.render_widget(Paragraph::new(self.diff_title(&diff, ctx)), title);
    }

    fn set_offsets(&mut self, offsets: Vec<u16>) {
        if std::mem::take(&mut self.to_last_hunk)
            && let Some(&last) = offsets.last()
        {
            self.diff_scroll = last;
            self.hunk = Some(offsets.len() - 1);
        }
        self.hunk_offsets = offsets;
    }

    /// The change at the top of the diff, by scroll position.
    fn hunk_at_scroll(&self) -> Option<usize> {
        self.hunk_offsets
            .iter()
            .rposition(|&o| o <= self.diff_scroll)
    }

    /// The change the top of the diff is in (0-based), if the file has any.
    fn current_hunk(&self) -> Option<usize> {
        if self.hunk_offsets.is_empty() {
            return None;
        }
        self.hunk
            .filter(|&h| h < self.hunk_offsets.len())
            .or_else(|| self.hunk_at_scroll())
            .or(Some(0))
    }

    /// `path  file 1/3  ·  change 1/2  ·  ⇠ col 9`
    fn diff_title(&self, diff: &HighlightedDiff, ctx: &Ctx) -> Line<'static> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let Some(file) = diff.diff.files.get(self.file) else {
            return Line::raw("");
        };
        let mut spans = vec![
            Span::styled(file.path().to_owned(), bold(t.fg)),
            Span::styled(
                format!("  file {}/{}", self.file + 1, diff.diff.files.len()),
                muted,
            ),
        ];
        if let Some(hunk) = self.current_hunk() {
            spans.push(Span::styled("  ·  ", muted));
            spans.push(Span::styled(
                format!("change {}/{}", hunk + 1, self.hunk_offsets.len()),
                bold(t.info),
            ));
        }
        if self.diff_hscroll > 0 {
            spans.push(Span::styled("  ·  ", muted));
            spans.push(Span::styled(
                format!("⇠ col {}", self.diff_hscroll + 1),
                bold(t.warning),
            ));
        }
        Line::from(spans)
    }

    fn render_checks(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let Some(detail) = &self.detail else {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(crate::screens::spinner(ctx), bold(t.accent)),
                    Span::styled(" loading checks…", muted),
                ])),
                area,
            );
            return;
        };
        let checks = &detail.value.checks;
        if checks.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::styled("no checks on this pull request", muted)),
                area,
            );
            return;
        }
        self.check = self.check.min(checks.len() - 1);
        let mut lines = Vec::new();
        if let Some(summary) = &detail.value.pr.checks {
            let mut spans = vec![Span::styled("summary   ", muted)];
            spans.extend(chips::checks_spans(summary, t));
            lines.push(Line::from(spans));
            lines.push(Line::raw(""));
        }
        for (index, check) in checks.iter().enumerate() {
            let (icon, color, word) = match check.state {
                CheckState::Success => ("✓", t.success, "passed"),
                CheckState::Failure => ("✗", t.danger, "failed"),
                CheckState::Pending => ("◐", t.warning, "running"),
                CheckState::Neutral => ("·", t.muted, "skipped"),
            };
            let mut line = Line::from(vec![
                Span::styled(
                    if index == self.check { "▌ " } else { "  " },
                    bold(t.accent_alt),
                ),
                Span::styled(format!("{icon} "), bold(color)),
                Span::styled(check.name.clone(), bold(t.fg)),
                Span::styled(format!("  {word}"), Style::new().fg(color)),
                Span::styled(
                    if check.url.is_some() && index == self.check {
                        "   o opens the log"
                    } else {
                        ""
                    },
                    muted,
                ),
            ]);
            if index == self.check {
                line = line.style(Style::new().bg(t.selection));
            }
            lines.push(line);
        }
        let offset = if lines.len() > checks.len() { 2 } else { 0 };
        let scroll = ((self.check + offset) as u16).saturating_sub(area.height.saturating_sub(1));
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
    }

    fn hints(&self, ctx: &Ctx) -> Line<'static> {
        let mut items: Vec<(&[Command], &'static str)> = vec![(&[Command::NextTab], "tabs")];
        match self.pane() {
            Pane::Conversation => items.push((&[Command::Down, Command::Up], "scroll")),
            Pane::Files => {
                items.push((&[Command::NextFile, Command::PrevFile], "file"));
                items.push((&[Command::NextHunk, Command::PrevHunk], "change"));
                items.push((&[Command::Down, Command::Up], "scroll"));
                items.push((&[Command::ScrollLeft, Command::ScrollRight], "sideways"));
                items.push((&[Command::SplitDiff], "split"));
            }
            Pane::Checks => items.push((&[Command::Down, Command::Up], "select")),
        }
        items.push((&[Command::Browser], "browser"));
        items.push((&[Command::Refresh], "refresh"));
        items.push((&[Command::Back], "back"));
        crate::ui::hints::line(&items, &ctx.keymap, &ctx.theme)
    }

    fn scroll(&mut self, delta: i32) {
        let apply = |v: u16| -> u16 {
            if delta < 0 {
                v.saturating_sub(delta.unsigned_abs() as u16)
            } else {
                v.saturating_add(delta as u16)
            }
        };
        match self.pane() {
            Pane::Conversation => self.conversation_scroll = apply(self.conversation_scroll),
            Pane::Files => {
                self.diff_scroll = apply(self.diff_scroll);
                self.hunk = None;
            }
            Pane::Checks => {
                let len = self.checks_len();
                if len > 0 {
                    self.check =
                        (self.check as i32 + delta.signum()).clamp(0, len as i32 - 1) as usize;
                }
            }
        }
    }

    fn select_file(&mut self, next: bool) {
        let Some(diff) = self.files() else {
            return;
        };
        let len = diff.diff.files.len();
        if len == 0 {
            return;
        }
        self.file = if next {
            (self.file + 1).min(len - 1)
        } else {
            self.file.saturating_sub(1)
        };
        self.diff_scroll = 0;
        self.diff_hscroll = 0;
        self.hunk = None;
        self.switched = Moment::now();
    }

    fn jump_hunk(&mut self, next: bool) {
        let at_scroll = self.hunk_at_scroll();
        let target = if next {
            match self.hunk.or(at_scroll) {
                Some(h) => h + 1,
                None => 0,
            }
        } else {
            match (self.hunk, at_scroll) {
                // Scrolled into the middle of a change: back to its start.
                (None, Some(h))
                    if self
                        .hunk_offsets
                        .get(h)
                        .is_some_and(|&o| o < self.diff_scroll) =>
                {
                    h
                }
                (current, at_scroll) => {
                    match current.or(at_scroll).and_then(|h| h.checked_sub(1)) {
                        Some(h) => h,
                        None => usize::MAX,
                    }
                }
            }
        };
        if let Some(&offset) = self.hunk_offsets.get(target) {
            self.diff_scroll = offset;
            self.hunk = Some(target);
            return;
        }
        // Past the last (or before the first) change: on to the next file's
        // first change, or back to the previous file's last one.
        let before = self.file;
        self.select_file(next);
        if !next && self.file != before {
            self.to_last_hunk = true;
        }
    }
}

impl Drop for PullView {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl View for PullView {
    fn mount(&mut self, ctx: &Ctx) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let Some(forge) = ctx.forges.by_account(&self.account) else {
            self.detail_error = Some(ForgeError::NotFound);
            return;
        };
        self.task = Some(sync::spawn_pull(
            Arc::clone(forge),
            ctx.cache.clone(),
            self.item.clone(),
            ctx.theme.clone(),
            ctx.sender(),
        ));
    }

    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let Event::Key(key) = event else {
            return None;
        };
        let page = 15;
        match ctx.keymap.lookup(key, &[Scope::List, Scope::Global])? {
            Command::Back => return Some(Action::Back),
            Command::Quit => return Some(Action::Quit),
            Command::Tab1 => self.switch(0, ctx),
            Command::Tab2 => self.switch(1, ctx),
            Command::Tab3 => self.switch(2, ctx),
            Command::NextTab => self.switch((self.pane + 1) % PANES.len(), ctx),
            Command::PrevTab => self.switch((self.pane + PANES.len() - 1) % PANES.len(), ctx),
            Command::Down => self.scroll(1),
            Command::Up => self.scroll(-1),
            Command::PageDown => self.scroll(page),
            Command::PageUp => self.scroll(-page),
            Command::Top => match self.pane() {
                Pane::Conversation => self.conversation_scroll = 0,
                Pane::Files => (self.diff_scroll, self.hunk) = (0, None),
                Pane::Checks => self.check = 0,
            },
            Command::Bottom => match self.pane() {
                Pane::Conversation => self.conversation_scroll = u16::MAX,
                Pane::Files => (self.diff_scroll, self.hunk) = (u16::MAX, None),
                Pane::Checks => self.check = self.checks_len().saturating_sub(1),
            },
            Command::ScrollLeft if self.pane() == Pane::Files => {
                self.diff_hscroll = self.diff_hscroll.saturating_sub(HSCROLL_STEP);
            }
            Command::ScrollRight if self.pane() == Pane::Files => {
                self.diff_hscroll = self.diff_hscroll.saturating_add(HSCROLL_STEP);
            }
            Command::NextFile => self.select_file(true),
            Command::PrevFile => self.select_file(false),
            Command::NextHunk => self.jump_hunk(true),
            Command::PrevHunk => self.jump_hunk(false),
            Command::SplitDiff => {
                self.split = !self.split;
                return Some(Action::Toast(Toast::info(if self.split {
                    "side by side (needs a wide terminal)"
                } else {
                    "unified diff"
                })));
            }
            Command::Open if self.pane() == Pane::Checks => return self.open_check(),
            Command::Browser => {
                if self.pane() == Pane::Checks {
                    return self.open_check();
                }
                let url = self.pr().map(|pr| pr.url.clone())?;
                return Some(Action::Toast(match browser::open(&url) {
                    Ok(()) => Toast::success(format!("opened {}", self.item)),
                    Err(toast) => toast,
                }));
            }
            Command::Refresh => {
                self.mount(ctx);
                return Some(Action::Toast(Toast::info("refreshing…")));
            }
            _ => return None,
        }
        Some(Action::Redraw)
    }

    fn update(&mut self, action: &Action, _ctx: &Ctx) -> Option<Action> {
        match action {
            Action::Data(DataEvent::PullDetail { item, result }) if *item == self.item => {
                match result {
                    Ok(loaded) => {
                        self.detail = Some(loaded.clone());
                        self.detail_error = None;
                    }
                    Err(e) => self.detail_error = Some(e.clone()),
                }
            }
            Action::Data(DataEvent::PullDiff { item, result }) if *item == self.item => {
                self.diff = Some(result.clone());
            }
            _ => {}
        }
        None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.accent))
            .style(t.base())
            .title(Span::styled(format!(" {} ", self.item), bold(t.accent)));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let inner = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };

        let [header, tabs, body, hints] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        frame.render_widget(
            Paragraph::new(self.header(ctx)).wrap(Wrap { trim: true }),
            header,
        );

        let files = self.files().map(|d| d.diff.files.len());
        let specs = [
            TabSpec {
                title: "Conversation",
                count: self
                    .detail
                    .as_ref()
                    .map(|d| d.value.comments.len() + d.value.reviews.len()),
            },
            TabSpec {
                title: "Files",
                count: files,
            },
            TabSpec {
                title: "Checks",
                count: Some(self.checks_len()),
            },
        ];
        let underline = (self.underline.0.value(), self.underline.1.value());
        let positions = TabBar {
            tabs: &specs,
            active: self.pane,
            theme: t,
            underline: if underline.0.is_nan() {
                (0.0, 0.0)
            } else {
                underline
            },
            logo: false,
            phase: if ctx.animations {
                ctx.time() * 0.1
            } else {
                0.0
            },
            right: Line::raw(""),
        }
        .render(frame, tabs);
        if let Some(&(x, w)) = positions.get(self.pane)
            && (self.underline.0.target().is_nan()
                || (!self.underline.0.is_running()
                    && (self.underline.0.target() != f32::from(x)
                        || self.underline.1.target() != f32::from(w))))
        {
            self.underline = (Tween::settled(f32::from(x)), Tween::settled(f32::from(w)));
        }
        self.positions = positions;

        let body = Rect {
            y: body.y + 1,
            height: body.height.saturating_sub(1),
            ..body
        };
        match self.pane() {
            Pane::Conversation => self.render_conversation(frame, body, ctx),
            Pane::Files => self.render_files(frame, body, ctx),
            Pane::Checks => self.render_checks(frame, body, ctx),
        }
        frame.render_widget(Paragraph::new(self.hints(ctx)), hints);
    }

    fn is_animating(&self) -> bool {
        self.underline.0.is_running()
            || self.switched.within(FADE)
            || self.opened.within(FADE)
            || self.detail.is_none()
            || self.diff.is_none()
    }
}

impl PullView {
    fn open_check(&self) -> Option<Action> {
        let check = self.detail.as_ref()?.value.checks.get(self.check)?;
        let url = check.url.as_deref()?;
        Some(Action::Toast(match browser::open(url) {
            Ok(()) => Toast::success(format!("opened {}", check.name)),
            Err(toast) => toast,
        }))
    }
}

#[cfg(test)]
mod tests {
    use hit_core::{Forges, Host, RepoRef};
    use hit_forge_mock::{MockForge, MockOptions};
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::Services;

    fn render(view: &mut PullView, ctx: &Ctx, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
        terminal
            .draw(|frame| view.render(frame, frame.area(), ctx))
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn loaded() -> (PullView, Ctx) {
        let forge = Arc::new(MockForge::demo(MockOptions::instant()));
        let account = hit_core::Forge::account(forge.as_ref()).clone();
        let mut forges = Forges::new();
        forges.add(forge);
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let item =
            ItemRef::pull_request(RepoRef::new(Host::new("demo.hit"), "acme", "rocket"), 142);
        let mut view = PullView::new(account, item, None);
        view.mount(&ctx);
        while view.detail.is_none() || view.diff.is_none() {
            let action = rx.recv().await.unwrap();
            view.update(&action, &ctx);
        }
        (view, ctx)
    }

    #[tokio::test]
    async fn conversation_files_and_checks() {
        let (mut view, ctx) = loaded().await;
        let screen = render(&mut view, &ctx, 150);
        assert!(screen.contains("Retry flaky uploads"), "{screen}");
        assert!(screen.contains("Conversation"));
        assert!(
            screen.contains("exponential backoff"),
            "markdown body: {screen}"
        );
        assert!(screen.contains("@mira commented"));

        view.switch(1, &ctx);
        let screen = render(&mut view, &ctx, 150);
        assert!(screen.contains("src/upload.rs"), "{screen}");
        assert!(screen.contains("MAX_RETRIES"));
        assert!(screen.contains("@@ -1,14 +1,19 @@"));

        view.select_file(true);
        let screen = render(&mut view, &ctx, 150);
        assert!(screen.contains("retries_transient_errors"), "{screen}");

        view.split = true;
        view.select_file(false);
        let screen = render(&mut view, &ctx, 200);
        assert!(screen.contains("MAX_RETRIES: u32 = 3"), "{screen}");
        assert!(screen.contains("MAX_RETRIES: u32 = 5"));

        view.switch(2, &ctx);
        let screen = render(&mut view, &ctx, 150);
        assert!(
            screen.contains("test") && screen.contains("failed"),
            "{screen}"
        );
    }

    #[tokio::test]
    async fn hunk_jumps_walk_through_files_both_ways() {
        let (mut view, ctx) = loaded().await;
        view.switch(1, &ctx);
        let screen = render(&mut view, &ctx, 150);
        assert!(screen.contains("change 1/2"), "{screen}");

        view.jump_hunk(true);
        let screen = render(&mut view, &ctx, 150);
        assert!(screen.contains("change 2/2"), "{screen}");

        view.jump_hunk(true);
        render(&mut view, &ctx, 150);
        assert_eq!((view.file, view.diff_scroll), (1, 0), "on to the next file");

        // Back: from the second file's first change to the first file's last.
        view.jump_hunk(false);
        let screen = render(&mut view, &ctx, 150);
        assert_eq!(view.file, 0);
        assert!(screen.contains("change 2/2"), "{screen}");

        view.jump_hunk(false);
        let screen = render(&mut view, &ctx, 150);
        assert!(screen.contains("change 1/2"), "{screen}");
    }

    #[tokio::test]
    async fn sideways_scroll_moves_both_sides_and_stops_at_the_longest_line() {
        let (mut view, ctx) = loaded().await;
        view.switch(1, &ctx);
        view.split = true;
        view.diff_scroll = u16::MAX;
        let screen = render(&mut view, &ctx, 200);
        assert!(
            screen.contains("const MAX") || screen.contains("matches!"),
            "{screen}"
        );

        view.diff_hscroll = 8;
        let screen = render(&mut view, &ctx, 200);
        assert!(screen.contains("⇠ col 9"), "{screen}");
        // Both sides lost their first 8 columns of `        matches!(…`.
        assert_eq!(screen.matches("matches!(self").count(), 2, "{screen}");

        view.diff_hscroll = u16::MAX;
        render(&mut view, &ctx, 200);
        let max = code_width(view.files().unwrap(), 0);
        assert!(view.diff_hscroll < max, "clamped: {}", view.diff_hscroll);
        assert!(view.diff_hscroll > 0);

        view.select_file(true);
        assert_eq!(view.diff_hscroll, 0, "a new file starts at the left edge");
    }

    #[test]
    fn code_spans_cut_across_segments() {
        let segs = vec![
            (Color::Red, "abc".to_owned()),
            (Color::Blue, "defg".to_owned()),
        ];
        let text = |skip| {
            code_spans(&segs, skip)
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        assert_eq!(text(0), "abcdefg");
        assert_eq!(text(2), "cdefg");
        assert_eq!(text(3), "defg");
        assert_eq!(text(9), "");
    }

    #[tokio::test]
    async fn scrolling_stops_at_the_end() {
        let (mut view, ctx) = loaded().await;
        view.switch(1, &ctx);
        view.diff_scroll = u16::MAX;
        render(&mut view, &ctx, 150);
        assert!(view.diff_scroll < 100, "clamped to the content");
    }
}
