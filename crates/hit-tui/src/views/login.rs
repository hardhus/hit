//! Device-flow sign-in inside the TUI.

use std::{
    sync::{Arc, mpsc as std_mpsc},
    time::Duration,
};

use crossterm::event::KeyCode;
use hit_core::{
    Timestamp,
    auth::{DeviceCode, LoginFlow},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use crate::{Action, Ctx, DataEvent, Event, View, action::ForgeHandle, action::LoginEvent};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

enum State {
    Starting,
    Waiting {
        code: DeviceCode,
        copied: bool,
        opened: bool,
    },
    Failed(String),
}

pub struct LoginView {
    flow: Arc<dyn LoginFlow>,
    state: State,
    task: Option<JoinHandle<()>>,
}

impl LoginView {
    pub fn new(flow: Arc<dyn LoginFlow>) -> Self {
        Self {
            flow,
            state: State::Starting,
            task: None,
        }
    }

    fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for LoginView {
    fn drop(&mut self) {
        // Leaving the screen cancels the sign-in (and stops polling).
        self.cancel();
    }
}

/// Copies `text` on a dedicated thread that keeps owning the clipboard until
/// `stop` is dropped (on X11 the contents vanish with their owner). Returns
/// whether copying worked.
fn copy_to_clipboard(text: String) -> (bool, Option<std_mpsc::Sender<()>>) {
    let (result_tx, result_rx) = std_mpsc::channel();
    let (stop_tx, stop_rx) = std_mpsc::channel::<()>();
    let spawned = std::thread::Builder::new()
        .name("hit-clipboard".to_owned())
        .spawn(move || {
            let clipboard = arboard::Clipboard::new()
                .and_then(|mut c| c.set_text(text).map(|()| c))
                .ok();
            let _ = result_tx.send(clipboard.is_some());
            if clipboard.is_some() {
                let _ = stop_rx.recv_timeout(Duration::from_secs(15 * 60));
            }
        });
    if spawned.is_err() {
        return (false, None);
    }
    let copied = result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or(false);
    (copied, Some(stop_tx))
}

impl View for LoginView {
    fn mount(&mut self, ctx: &Ctx) {
        self.cancel();
        self.state = State::Starting;
        let flow = Arc::clone(&self.flow);
        let tx = ctx.sender();
        let send = move |event| {
            let _ = tx.send(event);
        };
        self.task = Some(tokio::spawn(async move {
            let pending = match flow.start().await {
                Ok(pending) => pending,
                Err(e) => {
                    send(Action::Data(DataEvent::Login(LoginEvent::Failed(
                        e.to_string(),
                    ))));
                    return;
                }
            };
            let code = pending.code().clone();
            let text = code.user_code.clone();
            let (copied, _keep_clipboard) =
                tokio::task::spawn_blocking(move || copy_to_clipboard(text))
                    .await
                    .unwrap_or((false, None));
            // The provider validated the URL (https, own host); re-check the
            // scheme before handing it to the OS.
            let opened = code.verification_uri.starts_with("https://")
                && open::that_detached(&code.verification_uri).is_ok();
            send(Action::Data(DataEvent::Login(LoginEvent::Code {
                code,
                copied,
                opened,
            })));
            match pending.finish().await {
                Ok(forge) => send(Action::AccountAdded(ForgeHandle(forge))),
                Err(e) => send(Action::Data(DataEvent::Login(LoginEvent::Failed(
                    e.to_string(),
                )))),
            }
        }));
    }

    fn handle_event(&mut self, event: &Event, _ctx: &Ctx) -> Option<Action> {
        match event {
            Event::Key(key) if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) => {
                self.cancel();
                Some(Action::Back)
            }
            // Swallow other keys so global ones don't act behind the dialog.
            Event::Key(_) => Some(Action::Redraw),
            _ => None,
        }
    }

    fn update(&mut self, action: &Action, _ctx: &Ctx) -> Option<Action> {
        if let Action::Data(DataEvent::Login(event)) = action {
            self.state = match event.clone() {
                LoginEvent::Code {
                    code,
                    copied,
                    opened,
                } => State::Waiting {
                    code,
                    copied,
                    opened,
                },
                LoginEvent::Failed(message) => State::Failed(message),
            };
        }
        None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let [dialog] = Layout::horizontal([Constraint::Max(72)])
            .flex(Flex::Center)
            .areas(area);
        let [dialog] = Layout::vertical([Constraint::Length(13)])
            .flex(Flex::Center)
            .areas(dialog);

        let host = self.flow.host().to_string();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.accent_alt))
            .style(t.base())
            .title(Span::styled(
                format!(" sign in to {host} "),
                Style::new().fg(t.accent_alt).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(" esc cancel ", Style::new().fg(t.muted)));
        let inner = block.inner(dialog);
        frame.render_widget(Clear, dialog);
        frame.render_widget(block, dialog);

        let spinner = if ctx.animations {
            SPINNER
                .get((ctx.time() * 12.0) as usize % SPINNER.len())
                .copied()
                .unwrap_or("·")
        } else {
            "·"
        };
        let muted = Style::new().fg(t.muted);
        let check = |done: bool, text: &'static str| {
            if done {
                Span::styled(format!("  ✓ {text}"), Style::new().fg(t.success))
            } else {
                Span::raw("")
            }
        };

        let lines: Vec<Line> = match &self.state {
            State::Starting => vec![
                Line::raw(""),
                Line::raw(""),
                Line::from(vec![
                    Span::styled(spinner, Style::new().fg(t.accent)),
                    Span::styled(format!(" contacting {host}…"), muted),
                ]),
            ],
            State::Waiting {
                code,
                copied,
                opened,
            } => {
                let spaced: String = code
                    .user_code
                    .chars()
                    .map(String::from)
                    .collect::<Vec<_>>()
                    .join(" ");
                let phase = if ctx.animations {
                    -ctx.time() * 0.2
                } else {
                    0.0
                };
                let left = remaining(code.expires_at);
                // The code itself never animates: it has to be read and typed.
                // Only the underline below it moves.
                let pill = format!("  {spaced}  ");
                let underline = "▀".repeat(pill.chars().count());
                vec![
                    Line::styled("your one-time code", muted),
                    Line::raw(""),
                    Line::from(Span::styled(
                        pill,
                        Style::new()
                            .fg(t.fg)
                            .bg(t.selection)
                            .add_modifier(Modifier::BOLD),
                    )),
                    t.gradient_line_owned(underline, 24, phase),
                    Line::from(vec![
                        Span::styled("1. ", muted),
                        Span::styled(code.verification_uri.clone(), Style::new().fg(t.accent)),
                        check(*opened, "opened"),
                    ]),
                    Line::from(vec![
                        Span::styled("2. ", muted),
                        Span::styled("enter the code", Style::new().fg(t.fg)),
                        check(*copied, "copied"),
                    ]),
                    Line::raw(""),
                    Line::from(vec![
                        Span::styled(spinner, Style::new().fg(t.accent_alt)),
                        Span::styled(format!(" waiting for approval · {left} left"), muted),
                    ]),
                ]
            }
            State::Failed(message) => vec![
                Line::raw(""),
                Line::from(vec![
                    Span::styled("! ", Style::new().fg(t.danger).add_modifier(Modifier::BOLD)),
                    Span::styled(message.clone(), Style::new().fg(t.danger)),
                ]),
                Line::raw(""),
                Line::styled("press esc to go back", muted),
            ],
        };
        frame.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            inner,
        );
    }

    fn is_animating(&self) -> bool {
        !matches!(self.state, State::Failed(_))
    }
}

fn remaining(expires_at: Timestamp) -> String {
    let secs = expires_at.duration_since(Timestamp::now()).as_secs().max(0);
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use hit_core::{
        Host,
        auth::{LoginError, PendingLogin},
    };
    use ratatui::{Terminal, backend::TestBackend};
    use tokio::sync::mpsc;

    use super::*;

    struct FakeFlow(Host);

    #[async_trait]
    impl LoginFlow for FakeFlow {
        fn host(&self) -> &Host {
            &self.0
        }
        async fn start(&self) -> Result<Box<dyn PendingLogin>, LoginError> {
            Err(LoginError::Unavailable("offline".into()))
        }
    }

    fn render(view: &mut LoginView, ctx: &Ctx) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 20)).unwrap();
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

    fn ctx() -> (Ctx, mpsc::UnboundedReceiver<Action>) {
        Ctx::for_tests(crate::Services::default())
    }

    #[test]
    fn shows_code_link_and_progress() {
        let (ctx, _rx) = ctx();
        let mut view = LoginView::new(Arc::new(FakeFlow(Host::new("github.com"))));
        view.update(
            &Action::Data(DataEvent::Login(LoginEvent::Code {
                code: DeviceCode {
                    host: Host::new("github.com"),
                    user_code: "ABCD-1234".into(),
                    verification_uri: "https://github.com/login/device".into(),
                    expires_at: Timestamp::now()
                        .checked_add(jiff::SignedDuration::from_mins(10))
                        .unwrap(),
                },
                copied: true,
                opened: false,
            })),
            &ctx,
        );
        let screen = render(&mut view, &ctx);
        assert!(screen.contains("sign in to github.com"));
        assert!(screen.contains("A B C D - 1 2 3 4"));
        assert!(screen.contains("https://github.com/login/device"));
        assert!(screen.contains("✓ copied"));
        assert!(!screen.contains("✓ opened"));
        assert!(screen.contains("waiting for approval"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn start_failure_is_shown() {
        let (ctx, mut rx) = ctx();
        let mut view = LoginView::new(Arc::new(FakeFlow(Host::new("github.com"))));
        view.mount(&ctx);
        let action = rx.recv().await.unwrap();
        view.update(&action, &ctx);
        let screen = render(&mut view, &ctx);
        assert!(screen.contains("offline"), "{screen}");
        assert!(!view.is_animating());
    }
}
