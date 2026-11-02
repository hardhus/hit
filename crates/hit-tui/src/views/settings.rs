//! The settings screen. A change applies and is saved the moment it is made;
//! what can only apply on the next start says so next to its value.

mod items;

use std::time::Duration;

use hit_config::{Config, ConfigEdit, SettingValue};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    Action, Ctx, Event, View,
    action::{ConfigChange, Toast},
    anim::Moment,
    keymap::{Command, Scope, typed_char},
    sync,
    ui::{chips::bold, hints, popup::Popup},
};
use items::{Applies, Kind, SETTINGS, Setting};

const WIDE: u16 = 100;
const LABEL_WIDTH: usize = 34;
/// How long the screen stays open for the first frame's pop.
const OPENING: Duration = Duration::from_millis(130);

/// A selectable row: a setting, or something shown for reference only.
#[derive(Clone)]
enum Row {
    Setting(&'static Setting),
    Account {
        name: String,
        read_only: bool,
    },
    Key {
        description: &'static str,
        keys: String,
        name: &'static str,
    },
    ConfigFile,
}

/// A line of the list.
enum Slot {
    Header(&'static str),
    Row(Row),
}

#[derive(Debug, Clone, Copy)]
struct Asking {
    setting: &'static Setting,
    value: bool,
    opened: Moment,
}

impl std::fmt::Debug for Setting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.section, self.key)
    }
}

pub struct SettingsView {
    selected: usize,
    offset: usize,
    list_height: u16,
    asking: Option<Asking>,
    /// The last save problem, or the demo's reminder that nothing is saved.
    notice: Option<Toast>,
    writer: Option<UnboundedSender<ConfigEdit>>,
    opened: Moment,
}

impl SettingsView {
    pub fn new() -> Self {
        Self {
            selected: 0,
            offset: 0,
            list_height: 0,
            asking: None,
            notice: None,
            writer: None,
            opened: Moment::now(),
        }
    }

    fn slots(ctx: &Ctx) -> Vec<Slot> {
        let mut slots = Vec::new();
        let mut group = "";
        for setting in SETTINGS {
            if setting.group != group {
                group = setting.group;
                slots.push(Slot::Header(group));
            }
            slots.push(Slot::Row(Row::Setting(setting)));
        }
        slots.push(Slot::Header("Accounts"));
        for forge in ctx.forges.iter() {
            let account = forge.account();
            slots.push(Slot::Row(Row::Account {
                name: format!("@{} · {}", account.login, account.host),
                read_only: forge.is_read_only(),
            }));
        }
        slots.push(Slot::Header("Keys"));
        for &command in Command::ALL {
            slots.push(Slot::Row(Row::Key {
                description: command.description(),
                keys: ctx.keymap.label(command),
                name: command.name(),
            }));
        }
        slots.push(Slot::Header("File"));
        slots.push(Slot::Row(Row::ConfigFile));
        slots
    }

    fn rows(ctx: &Ctx) -> Vec<Row> {
        Self::slots(ctx)
            .into_iter()
            .filter_map(|slot| match slot {
                Slot::Row(row) => Some(row),
                Slot::Header(_) => None,
            })
            .collect()
    }

    fn selected_row(&self, ctx: &Ctx) -> Option<Row> {
        Self::rows(ctx).into_iter().nth(self.selected)
    }

    fn jump_group(&mut self, forward: bool, ctx: &Ctx) {
        let mut index = 0;
        let starts: Vec<usize> = Self::slots(ctx)
            .iter()
            .filter_map(|slot| match slot {
                Slot::Header(_) => Some(index),
                Slot::Row(_) => {
                    index += 1;
                    None
                }
            })
            .collect();
        let target = if forward {
            starts.iter().find(|&&start| start > self.selected)
        } else {
            starts.iter().rev().find(|&&start| start < self.selected)
        };
        if let Some(&target) = target {
            self.selected = target;
        }
    }

    /// Applies a new value: everywhere at once, then to the file.
    fn commit(&mut self, setting: &'static Setting, next: Config, ctx: &Ctx) -> Action {
        let reload = next.refresh != ctx.config.refresh
            || next.inbox.unread_only != ctx.config.inbox.unread_only;
        let edit = ConfigEdit {
            section: setting.section,
            key: setting.key,
            value: setting.value(&next),
        };
        match &self.writer {
            Some(writer) => {
                if writer.send(edit).is_err() {
                    self.notice = Some(Toast::error(
                        "couldn't save; the change lasts until you quit",
                    ));
                }
            }
            None => {
                self.notice = Some(Toast::info(
                    "demo: changes last until you quit, nothing is saved",
                ));
            }
        }
        Action::ConfigChanged(ConfigChange {
            config: Box::new(next),
            reload,
        })
    }

    fn set_toggle(&mut self, setting: &'static Setting, value: bool, ctx: &Ctx) -> Option<Action> {
        let Kind::Toggle { set, .. } = setting.kind else {
            return None;
        };
        if setting.confirm.is_some_and(|c| c.to == value) {
            self.asking = Some(Asking {
                setting,
                value,
                opened: Moment::now(),
            });
            return Some(Action::Redraw);
        }
        let mut next = ctx.config.clone();
        set(&mut next, value);
        Some(self.commit(setting, next, ctx))
    }

    /// Enter and space: flip a toggle, go to the next choice. Numbers need a
    /// direction.
    fn activate(&mut self, ctx: &Ctx) -> Option<Action> {
        let Row::Setting(setting) = self.selected_row(ctx)? else {
            return None;
        };
        match setting.kind {
            Kind::Toggle { get, .. } => self.set_toggle(setting, !get(&ctx.config), ctx),
            Kind::Choice { .. } => self.adjust(setting, 1, true, ctx),
            Kind::Number { .. } => None,
        }
    }

    /// `h`/`l`: a toggle goes off/on, a number and a choice step. Stops at the
    /// ends unless `wrap`.
    fn adjust(
        &mut self,
        setting: &'static Setting,
        direction: i64,
        wrap: bool,
        ctx: &Ctx,
    ) -> Option<Action> {
        let mut next = ctx.config.clone();
        match setting.kind {
            Kind::Toggle { get, .. } => {
                let value = direction > 0;
                if get(&ctx.config) == value {
                    return None;
                }
                return self.set_toggle(setting, value, ctx);
            }
            Kind::Number {
                get,
                set,
                min,
                max,
                step,
            } => {
                let current = get(&ctx.config);
                let value = current.saturating_add(step * direction).clamp(min, max);
                if value == current {
                    return None;
                }
                set(&mut next, value);
            }
            Kind::Choice { options, get, set } => {
                let count = options.len() as i64;
                let current = options
                    .iter()
                    .position(|o| *o == get(&ctx.config))
                    .unwrap_or(0) as i64;
                let target = current + direction;
                let target = if wrap {
                    target.rem_euclid(count)
                } else {
                    target.clamp(0, count - 1)
                };
                if target == current {
                    return None;
                }
                let option = options.get(usize::try_from(target).ok()?)?;
                set(&mut next, option);
            }
        }
        Some(self.commit(setting, next, ctx))
    }

    fn handle_key(&mut self, command: Command, ctx: &Ctx) -> Option<Action> {
        let count = Self::rows(ctx).len();
        let page = usize::from(self.list_height.max(2) / 2);
        match command {
            Command::Back | Command::Settings => return Some(Action::Back),
            Command::Quit => return Some(Action::Quit),
            Command::Down => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            Command::Up => self.selected = self.selected.saturating_sub(1),
            Command::Top => self.selected = 0,
            Command::Bottom => self.selected = count.saturating_sub(1),
            Command::PageDown => {
                self.selected = (self.selected + page).min(count.saturating_sub(1));
            }
            Command::PageUp => self.selected = self.selected.saturating_sub(page),
            Command::NextGroup => self.jump_group(true, ctx),
            Command::PrevGroup => self.jump_group(false, ctx),
            Command::Open => return self.activate(ctx).or(Some(Action::Redraw)),
            Command::ScrollLeft | Command::ScrollRight => {
                let direction = if command == Command::ScrollRight {
                    1
                } else {
                    -1
                };
                if let Some(Row::Setting(setting)) = self.selected_row(ctx) {
                    return self
                        .adjust(setting, direction, false, ctx)
                        .or(Some(Action::Redraw));
                }
            }
            _ => return None,
        }
        Some(Action::Redraw)
    }

    fn value_spans(setting: &Setting, selected: bool, ctx: &Ctx) -> Vec<Span<'static>> {
        let t = &ctx.theme;
        match setting.kind {
            Kind::Toggle { get, .. } => {
                if get(&ctx.config) {
                    vec![Span::styled("● on", bold(t.success))]
                } else {
                    vec![Span::styled("○ off", Style::new().fg(t.muted))]
                }
            }
            Kind::Number { .. } | Kind::Choice { .. } => {
                let value = setting.shown(&ctx.config);
                if selected {
                    vec![
                        Span::styled("‹ ", bold(t.accent_alt)),
                        Span::styled(value, bold(t.accent)),
                        Span::styled(" ›", bold(t.accent_alt)),
                    ]
                } else {
                    vec![Span::styled(value, bold(t.accent))]
                }
            }
        }
    }

    /// Changed since hit started but only applies on the next start.
    fn waiting_for_restart(setting: &Setting, ctx: &Ctx) -> bool {
        let changed = setting.value(&ctx.config) != setting.value(&ctx.started_config);
        match setting.applies {
            Applies::Now => false,
            Applies::NextStart => changed,
            Applies::OnNowOffNextStart => {
                changed && setting.value(&ctx.config) == SettingValue::Bool(false)
            }
        }
    }

    fn row_line(row: &Row, selected: bool, ctx: &Ctx) -> Line<'static> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let mut spans = vec![Span::styled(
            if selected { "▌ " } else { "  " },
            bold(t.accent_alt),
        )];
        match row {
            Row::Setting(setting) => {
                spans.push(Span::styled(
                    format!("{:<LABEL_WIDTH$}", setting.label),
                    Style::new().fg(t.fg),
                ));
                spans.extend(Self::value_spans(setting, selected, ctx));
                if Self::waiting_for_restart(setting, ctx) {
                    spans.push(Span::styled("  ↻ next start", bold(t.warning)));
                }
            }
            Row::Account { name, read_only } => {
                spans.push(Span::styled(name.clone(), Style::new().fg(t.fg)));
                if *read_only {
                    spans.push(Span::styled("  read-only", bold(t.warning)));
                }
            }
            Row::Key {
                description, keys, ..
            } => {
                spans.push(Span::styled(
                    format!("{description:<LABEL_WIDTH$}"),
                    Style::new().fg(t.fg),
                ));
                spans.push(Span::styled(keys.clone(), bold(t.accent)));
            }
            Row::ConfigFile => {
                let path = ctx.config_file.as_ref().map_or_else(
                    || "not saved in the demo".to_owned(),
                    |path| path.display().to_string(),
                );
                spans.push(Span::styled(path, muted));
            }
        }
        let line = Line::from(spans);
        if selected {
            line.style(Style::new().bg(t.selection))
        } else {
            line
        }
    }

    fn render_list(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        self.list_height = area.height;
        let mut lines = Vec::new();
        let mut selected_line = 0;
        let mut index = 0;
        for slot in Self::slots(ctx) {
            match slot {
                Slot::Header(title) => {
                    if !lines.is_empty() {
                        lines.push(Line::raw(""));
                    }
                    lines.push(Line::from(Span::styled(
                        format!(" {title}"),
                        bold(t.accent_alt),
                    )));
                }
                Slot::Row(row) => {
                    if index == self.selected {
                        selected_line = lines.len();
                    }
                    lines.push(Self::row_line(&row, index == self.selected, ctx));
                    index += 1;
                }
            }
        }
        let height = usize::from(area.height);
        // Keeps the group title in view when moving up onto a group's first row.
        if selected_line < self.offset + 1 {
            self.offset = selected_line.saturating_sub(1);
        } else if selected_line >= self.offset + height {
            self.offset = selected_line + 1 - height;
        }
        self.offset = self.offset.min(lines.len().saturating_sub(height));
        let visible: Vec<Line> = lines.into_iter().skip(self.offset).take(height).collect();
        frame.render_widget(Paragraph::new(visible), area);
    }

    fn help_lines(&self, ctx: &Ctx) -> Vec<Line<'static>> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let text = Style::new().fg(t.fg);
        let Some(row) = self.selected_row(ctx) else {
            return Vec::new();
        };
        match row {
            Row::Setting(setting) => {
                let defaults = Config::default();
                let mut lines = vec![
                    Line::from(Span::styled(setting.label, bold(t.accent))),
                    Line::from(Span::styled(setting.help, text)),
                    Line::raw(""),
                    Line::from(Span::styled(
                        format!("default  {}", setting.shown(&defaults)),
                        muted,
                    )),
                    Line::from(Span::styled(
                        match setting.applies {
                            Applies::Now => "applies  right away",
                            Applies::NextStart => "applies  the next time hit starts",
                            Applies::OnNowOffNextStart => {
                                "applies  on: right away · off: next start"
                            }
                        },
                        muted,
                    )),
                    Line::from(Span::styled(
                        format!("config   [{}] {}", setting.section, setting.key),
                        muted,
                    )),
                ];
                if let Kind::Number { min, max, .. } = setting.kind {
                    lines.push(Line::from(Span::styled(
                        format!("range    {min} to {max}"),
                        muted,
                    )));
                }
                lines
            }
            Row::Account { name, read_only } => vec![
                Line::from(Span::styled(name, bold(t.accent))),
                Line::from(Span::styled(
                    if read_only {
                        "This account will not write anything."
                    } else {
                        "hit can act with this account as far as its token allows."
                    },
                    text,
                )),
                Line::raw(""),
                Line::from(Span::styled(
                    "Sign in and out with `hit auth login` and `hit auth logout`.",
                    muted,
                )),
            ],
            Row::Key { name, .. } => vec![
                Line::from(Span::styled(
                    "Keys are changed in config.toml",
                    bold(t.accent),
                )),
                Line::from(Span::styled(format!("[keys] {name} = [\"…\"]"), text)),
                Line::raw(""),
                Line::from(Span::styled(
                    "Replaces this command's keys; hit warns about clashes on start.",
                    muted,
                )),
            ],
            Row::ConfigFile => vec![
                Line::from(Span::styled("config.toml", bold(t.accent))),
                Line::from(Span::styled(
                    "Changes here keep your comments and everything else in the file.",
                    text,
                )),
                Line::raw(""),
                Line::from(Span::styled(
                    "`hit config check` reports problems after editing it by hand.",
                    muted,
                )),
            ],
        }
    }

    fn render_asking(asking: &Asking, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let question = asking
            .setting
            .confirm
            .map_or("Are you sure?", |c| c.question);
        let inner = Popup {
            title: asking.setting.label,
            footer: Some("y yes · anything else cancels"),
            width: 64,
            height: 6,
            border: t.warning,
            theme: t,
            opened: ctx.animations.then_some(asking.opened),
        }
        .render(frame, area);
        frame.render_widget(
            Paragraph::new(Span::styled(question, Style::new().fg(t.fg))).wrap(Wrap { trim: true }),
            Rect {
                x: inner.x + 1,
                y: inner.y + 1,
                width: inner.width.saturating_sub(2),
                height: inner.height.saturating_sub(1),
            },
        );
    }
}

impl Default for SettingsView {
    fn default() -> Self {
        Self::new()
    }
}

impl View for SettingsView {
    fn mount(&mut self, ctx: &Ctx) {
        self.writer = ctx
            .config_file
            .clone()
            .map(|path| sync::spawn_config_writer(path, ctx.sender()));
    }

    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let Event::Key(key) = event else {
            return None;
        };
        if let Some(asking) = self.asking.take() {
            if typed_char(key) == Some('y')
                && let Kind::Toggle { set, .. } = asking.setting.kind
            {
                let mut next = ctx.config.clone();
                set(&mut next, asking.value);
                return Some(self.commit(asking.setting, next, ctx));
            }
            return Some(Action::Redraw);
        }
        if key.code == crossterm::event::KeyCode::Char(' ') {
            return self.activate(ctx).or(Some(Action::Redraw));
        }
        let command = ctx.keymap.lookup(key, &[Scope::List, Scope::Global])?;
        self.handle_key(command, ctx)
    }

    fn update(&mut self, action: &Action, _ctx: &Ctx) -> Option<Action> {
        if let Action::Toast(toast) = action {
            self.notice = Some(toast.clone());
        }
        None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let [title, body, status, hint] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);

        let title_line = Rect { height: 1, ..title };
        frame.render_widget(
            Paragraph::new(Span::styled(
                " settings",
                bold(t.accent_alt).add_modifier(Modifier::BOLD),
            )),
            title_line,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                if ctx.config_file.is_some() {
                    "saved as you go "
                } else {
                    "demo · not saved "
                },
                Style::new().fg(t.muted),
            ))
            .alignment(Alignment::Right),
            title_line,
        );

        if body.width >= WIDE {
            let [list, help] =
                Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)]).areas(body);
            self.render_list(frame, list, ctx);
            let divider = Rect { width: 1, ..help };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled("│", Style::new().fg(t.border));
                    usize::from(divider.height)
                ]),
                divider,
            );
            frame.render_widget(
                Paragraph::new(self.help_lines(ctx)).wrap(Wrap { trim: false }),
                Rect {
                    x: help.x + 2,
                    width: help.width.saturating_sub(3),
                    ..help
                },
            );
        } else {
            let [list, help] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(5)]).areas(body);
            self.render_list(frame, list, ctx);
            frame.render_widget(
                Paragraph::new(self.help_lines(ctx)).wrap(Wrap { trim: false }),
                Rect {
                    x: help.x + 1,
                    width: help.width.saturating_sub(2),
                    ..help
                },
            );
        }

        if let Some(notice) = &self.notice {
            use crate::action::ToastKind;
            let color = match notice.kind {
                ToastKind::Error => t.danger,
                ToastKind::Warning => t.warning,
                ToastKind::Success => t.success,
                ToastKind::Info => t.muted,
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!(" {}", notice.message),
                    Style::new().fg(color),
                )),
                status,
            );
        }
        let items: [(&[Command], &'static str); 5] = [
            (&[Command::Down, Command::Up], "move"),
            (&[Command::Open], "change"),
            (&[Command::ScrollLeft, Command::ScrollRight], "adjust"),
            (&[Command::NextGroup, Command::PrevGroup], "groups"),
            (&[Command::Back], "close"),
        ];
        frame.render_widget(
            Paragraph::new(hints::line(
                &items,
                &ctx.keymap,
                t,
                hint.width.saturating_sub(1),
            )),
            Rect {
                x: hint.x + 1,
                width: hint.width.saturating_sub(1),
                ..hint
            },
        );

        if let Some(asking) = &self.asking {
            Self::render_asking(asking, frame, area, ctx);
        }
    }

    fn is_animating(&self) -> bool {
        self.opened.within(OPENING)
            || self
                .asking
                .is_some_and(|a| Popup::is_animating(Some(a.opened)))
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use hit_config::apply_edits;
    use hit_core::Forges;
    use hit_forge_mock::{MockForge, MockOptions};
    use ratatui::{Terminal, backend::TestBackend};
    use std::sync::Arc;

    use super::*;
    use crate::Services;

    fn ctx() -> Ctx {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        Ctx::for_tests(Services {
            forges,
            ..Services::default()
        })
        .0
    }

    fn setting(section: &str, key: &str) -> &'static Setting {
        SETTINGS
            .iter()
            .find(|s| s.section == section && s.key == key)
            .unwrap()
    }

    fn select(view: &mut SettingsView, wanted: &'static Setting, ctx: &Ctx) {
        view.selected = SettingsView::rows(ctx)
            .iter()
            .position(|row| matches!(row, Row::Setting(s) if std::ptr::eq(*s, wanted)))
            .unwrap();
    }

    fn apply(action: Option<Action>, ctx: &mut Ctx) {
        if let Some(Action::ConfigChanged(change)) = action {
            ctx.config = *change.config;
        }
    }

    fn press(view: &mut SettingsView, c: char, ctx: &Ctx) -> Option<Action> {
        view.handle_event(
            &Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
            ctx,
        )
    }

    /// A value for `setting` other than the default.
    fn changed(setting: &Setting, config: &mut Config) {
        match setting.kind {
            Kind::Toggle { get, set } => {
                let value = !get(config);
                set(config, value);
            }
            Kind::Number {
                get, set, min, max, ..
            } => {
                let value = if get(config) == min { max } else { min };
                set(config, value);
            }
            Kind::Choice { options, get, set } => {
                let current = get(config);
                if let Some(other) = options.iter().find(|o| **o != current) {
                    set(config, other);
                }
            }
        }
    }

    #[test]
    fn every_setting_reaches_the_file_and_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut expected = Config::default();
        for setting in SETTINGS {
            changed(setting, &mut expected);
        }
        let edits: Vec<ConfigEdit> = SETTINGS
            .iter()
            .map(|setting| ConfigEdit {
                section: setting.section,
                key: setting.key,
                value: setting.value(&expected),
            })
            .collect();
        apply_edits(&path, &edits).unwrap();

        let loaded = hit_config::load(&path);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(loaded.config, expected);
    }

    #[test]
    fn every_setting_changes_on_its_own() {
        for setting in SETTINGS {
            if matches!(setting.kind, Kind::Choice { options, .. } if options.len() == 1) {
                continue;
            }
            let mut config = Config::default();
            changed(setting, &mut config);
            assert_ne!(config, Config::default(), "{setting:?}");
            for other in SETTINGS.iter().filter(|s| !std::ptr::eq(*s, setting)) {
                assert_eq!(
                    other.value(&config),
                    other.value(&Config::default()),
                    "{setting:?} also changed {other:?}"
                );
            }
        }
    }

    #[test]
    fn a_toggle_flips_and_applies() {
        let mut ctx = ctx();
        let mut view = SettingsView::new();
        select(&mut view, setting("ui", "animations"), &ctx);
        let before = ctx.config.ui.animations;
        let action = press(&mut view, ' ', &ctx);
        apply(action, &mut ctx);
        assert_eq!(ctx.config.ui.animations, !before);
    }

    #[test]
    fn numbers_stop_at_their_limits() {
        let mut ctx = ctx();
        let mut view = SettingsView::new();
        select(&mut view, setting("focus", "mention_days"), &ctx);
        ctx.config.focus.mention_days = 365;
        assert!(matches!(press(&mut view, 'l', &ctx), Some(Action::Redraw)));
        let action = press(&mut view, 'h', &ctx);
        apply(action, &mut ctx);
        assert_eq!(ctx.config.focus.mention_days, 364);
    }

    #[test]
    fn enter_cycles_a_choice_and_wraps() {
        let mut ctx = ctx();
        let mut view = SettingsView::new();
        select(&mut view, setting("inbox", "mark_read"), &ctx);
        let mut seen = Vec::new();
        for _ in 0..3 {
            let action = press(&mut view, ' ', &ctx);
            apply(action, &mut ctx);
            seen.push(ctx.config.inbox.mark_read);
        }
        use hit_config::MarkRead::{Leave, Manual, Open};
        assert_eq!(seen, [Open, Leave, Manual]);
    }

    #[test]
    fn turning_read_only_on_needs_no_confirmation() {
        let mut ctx = ctx();
        let mut view = SettingsView::new();
        select(&mut view, setting("auth", "read_only"), &ctx);
        let action = press(&mut view, ' ', &ctx);
        assert!(view.asking.is_none());
        apply(action, &mut ctx);
        assert!(ctx.config.auth.read_only);
    }

    #[test]
    fn turning_read_only_off_asks_first() {
        let mut ctx = ctx();
        ctx.config.auth.read_only = true;
        let mut view = SettingsView::new();
        select(&mut view, setting("auth", "read_only"), &ctx);

        assert!(matches!(press(&mut view, ' ', &ctx), Some(Action::Redraw)));
        assert!(view.asking.is_some());
        assert!(matches!(press(&mut view, 'n', &ctx), Some(Action::Redraw)));
        assert!(view.asking.is_none());
        assert!(ctx.config.auth.read_only, "cancelled: stays on");

        let _ = press(&mut view, ' ', &ctx);
        let action = press(&mut view, 'y', &ctx);
        apply(action, &mut ctx);
        assert!(!ctx.config.auth.read_only);
    }

    #[test]
    fn allowing_mark_read_asks_first() {
        let ctx = ctx();
        let mut view = SettingsView::new();
        select(&mut view, setting("auth", "allow_mark_read"), &ctx);
        let _ = press(&mut view, 'l', &ctx);
        assert!(view.asking.is_some());
    }

    #[test]
    fn what_waits_for_a_restart_says_so_until_it_is_undone() {
        let mut ctx = ctx();
        let mentions = setting("focus", "mentions");
        let animations = setting("ui", "animations");
        assert!(!SettingsView::waiting_for_restart(mentions, &ctx));
        ctx.config.focus.mentions = false;
        ctx.config.ui.animations = false;
        assert!(SettingsView::waiting_for_restart(mentions, &ctx));
        assert!(!SettingsView::waiting_for_restart(animations, &ctx));
        ctx.config.focus.mentions = true;
        assert!(!SettingsView::waiting_for_restart(mentions, &ctx));
    }

    #[test]
    fn read_only_only_waits_for_a_restart_when_turned_off() {
        let read_only = setting("auth", "read_only");
        let mut ctx = ctx();
        ctx.config.auth.read_only = true;
        assert!(
            !SettingsView::waiting_for_restart(read_only, &ctx),
            "on works right away"
        );
        ctx.started_config.auth.read_only = true;
        ctx.config.auth.read_only = false;
        assert!(SettingsView::waiting_for_restart(read_only, &ctx));
    }

    #[test]
    fn changing_how_often_data_refreshes_restarts_the_loops() {
        let ctx = ctx();
        let mut view = SettingsView::new();
        select(&mut view, setting("refresh", "notifications"), &ctx);
        let Some(Action::ConfigChanged(change)) = press(&mut view, ' ', &ctx) else {
            panic!("expected a change");
        };
        assert!(change.reload);
        select(&mut view, setting("ui", "animations"), &ctx);
        let Some(Action::ConfigChanged(change)) = press(&mut view, ' ', &ctx) else {
            panic!("expected a change");
        };
        assert!(!change.reload);
    }

    #[test]
    fn groups_jump_to_their_first_row() {
        let ctx = ctx();
        let mut view = SettingsView::new();
        view.handle_key(Command::NextGroup, &ctx);
        let rows = SettingsView::rows(&ctx);
        assert!(matches!(
            rows[view.selected],
            Row::Setting(s) if s.group == "Inbox"
        ));
        view.handle_key(Command::PrevGroup, &ctx);
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn the_demo_says_nothing_is_saved() {
        let ctx = ctx();
        let mut view = SettingsView::new();
        select(&mut view, setting("ui", "animations"), &ctx);
        let _ = press(&mut view, ' ', &ctx);
        assert!(view.notice.is_some());
    }

    #[test]
    fn keys_are_only_shown() {
        let ctx = ctx();
        let mut view = SettingsView::new();
        view.selected = SettingsView::rows(&ctx)
            .iter()
            .position(|row| matches!(row, Row::Key { .. }))
            .unwrap();
        assert!(matches!(press(&mut view, ' ', &ctx), Some(Action::Redraw)));
    }

    fn screen(view: &mut SettingsView, ctx: &Ctx, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| view.render(frame, frame.area(), ctx))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn it_draws_at_any_size_and_keeps_the_selection_visible() {
        let ctx = ctx();
        let mut view = SettingsView::new();
        for (width, height) in [(160, 50), (80, 24), (40, 10), (10, 3)] {
            view.selected = 0;
            let _ = screen(&mut view, &ctx, width, height);
            view.selected = SettingsView::rows(&ctx).len() - 1;
            let _ = screen(&mut view, &ctx, width, height);
        }
        view.selected = 0;
        assert!(screen(&mut view, &ctx, 120, 30).contains("Reviews asked of you"));
        select(&mut view, setting("log", "level"), &ctx);
        assert!(screen(&mut view, &ctx, 120, 30).contains("Level"));
    }

    #[test]
    fn the_value_written_matches_the_kind() {
        let config = Config::default();
        assert_eq!(
            setting("ui", "animations").value(&config),
            SettingValue::Bool(true)
        );
        assert_eq!(
            setting("focus", "mention_days").value(&config),
            SettingValue::Int(14)
        );
        assert_eq!(
            setting("inbox", "mark_read").value(&config),
            SettingValue::Str("manual".to_owned())
        );
    }
}
