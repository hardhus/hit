//! Writing a comment: a box, a Markdown preview and a way out to the user's
//! own editor. What was written is never lost by closing or by a failed send.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyModifiers};
use hit_config::ComposeMode;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::Line,
    widgets::{Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use hit_core::model::ReviewVerdict;

use crate::{
    Action, Ctx, DataEvent, Event, View,
    action::{Purpose, Target},
    anim::Moment,
    keymap::typed_char,
    markdown,
    sync::{self, ItemOp},
    ui::{editor::Editor, popup::Popup},
};

pub struct ComposeView {
    target: Target,
    purpose: Purpose,
    editor: Editor,
    preview: bool,
    preview_scroll: u16,
    /// Waiting for the forge; input is ignored so nothing is sent twice.
    sending: bool,
    error: Option<String>,
    task: Option<JoinHandle<()>>,
    opened: Moment,
}

impl Drop for ComposeView {
    fn drop(&mut self) {
        // A send already on its way is finished: leaving does not unsend it.
        drop(self.task.take());
    }
}

impl ComposeView {
    pub fn new(target: Target, purpose: Purpose, draft: &str) -> Self {
        Self {
            target,
            purpose,
            editor: Editor::new(draft),
            preview: false,
            preview_scroll: 0,
            sending: false,
            error: None,
            task: None,
            opened: Moment::now(),
        }
    }

    fn send(&mut self, ctx: &Ctx) {
        // An approval says enough by itself; everything else needs words.
        let may_be_blank = self.purpose == Purpose::Review(ReviewVerdict::Approve);
        if self.editor.is_blank() && !may_be_blank {
            self.error = Some(match self.purpose {
                Purpose::Comment => "nothing to send yet".to_owned(),
                Purpose::Review(_) => "say what should change".to_owned(),
            });
            return;
        }
        let Some(forge) = ctx.forges.by_account(&self.target.account) else {
            self.error = Some("this account is no longer signed in".to_owned());
            return;
        };
        self.error = None;
        self.sending = true;
        let text = self.editor.text();
        let op = match self.purpose {
            Purpose::Comment => ItemOp::Comment(text),
            Purpose::Review(verdict) => ItemOp::Review(verdict, text),
        };
        self.task = Some(sync::spawn_item_op(
            Arc::clone(forge),
            self.target.clone(),
            op,
            ctx.sender(),
        ));
    }

    fn close(&self) -> Action {
        Action::CloseCompose {
            target: self.target.clone(),
            draft: self.editor.text(),
        }
    }
}

impl View for ComposeView {
    fn mount(&mut self, ctx: &Ctx) {
        if ctx.config.general.compose == ComposeMode::Editor {
            let _ = ctx.sender().send(Action::EditInEditor(self.editor.text()));
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        if self.sending {
            return Some(Action::Redraw);
        }
        if let Event::Paste(text) = event {
            self.editor.insert_str(text);
            return Some(Action::Redraw);
        }
        let Event::Key(key) = event else {
            return None;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Some(self.close()),
            KeyCode::Char('s') if ctrl => self.send(ctx),
            KeyCode::Char('e') if ctrl => {
                return Some(Action::EditInEditor(self.editor.text()));
            }
            KeyCode::Tab => {
                self.preview = !self.preview;
                self.preview_scroll = 0;
            }
            KeyCode::Up if self.preview => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
            }
            KeyCode::Down if self.preview => {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
            }
            // Text only goes in where it can be seen.
            _ if self.preview => {}
            KeyCode::Enter => self.editor.newline(),
            KeyCode::Backspace => self.editor.backspace(),
            KeyCode::Delete => self.editor.delete(),
            KeyCode::Left => self.editor.left(),
            KeyCode::Right => self.editor.right(),
            KeyCode::Up => self.editor.up(),
            KeyCode::Down => self.editor.down(),
            KeyCode::Home => self.editor.home(),
            KeyCode::End => self.editor.end(),
            _ => {
                if let Some(c) = typed_char(key) {
                    self.editor.insert_char(c);
                }
            }
        }
        Some(Action::Redraw)
    }

    fn update(&mut self, action: &Action, _ctx: &Ctx) -> Option<Action> {
        match action {
            Action::Data(DataEvent::ActionDone { target, result, .. })
                if *target == self.target && self.sending =>
            {
                self.sending = false;
                match result {
                    Ok(()) => {
                        return Some(Action::CloseCompose {
                            target: self.target.clone(),
                            draft: String::new(),
                        });
                    }
                    Err(e) => self.error = Some(format!("couldn't send: {e}")),
                }
            }
            Action::EditorDone(Ok(text)) => {
                self.editor = Editor::new(text);
                self.error = None;
            }
            Action::EditorDone(Err(message)) => self.error = Some(message.clone()),
            _ => {}
        }
        None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let title = match self.purpose {
            Purpose::Comment => format!("comment on {}", self.target.item),
            Purpose::Review(ReviewVerdict::Approve) => {
                format!("approve {} · a comment is optional", self.target.item)
            }
            Purpose::Review(ReviewVerdict::RequestChanges) => {
                format!("request changes on {}", self.target.item)
            }
        };
        let footer = if self.sending {
            "sending…"
        } else if self.preview {
            "tab back to writing · ctrl+s send · esc keep and close"
        } else {
            "ctrl+s send · tab preview · ctrl+e editor · esc keep and close"
        };
        let inner = Popup {
            title: &title,
            footer: Some(footer),
            width: area.width.saturating_sub(4).min(100),
            height: area.height.saturating_sub(2).min(24),
            border: t.accent,
            theme: t,
            opened: ctx.animations.then_some(self.opened),
        }
        .render(frame, area);
        let inner = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };
        let [body, status] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
        if self.preview {
            let mut lines = markdown::render(&self.editor.text(), t);
            if lines.is_empty() {
                lines.push(Line::styled("nothing yet", Style::new().fg(t.muted)));
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((self.preview_scroll, 0)),
                body,
            );
        } else {
            self.editor.render(frame, body, Style::new().fg(t.fg));
        }
        if let Some(error) = &self.error {
            frame.render_widget(
                Paragraph::new(Line::styled(error.clone(), Style::new().fg(t.danger))),
                status,
            );
        }
    }

    fn is_animating(&self) -> bool {
        Popup::is_animating(Some(self.opened))
    }

    fn is_overlay(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyEvent;
    use hit_core::{Forge, Forges, ItemRef, RepoRef};
    use hit_forge_mock::{DEMO_HOST, MockForge, MockOptions};
    use ratatui::{Terminal, backend::TestBackend};
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::Services;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn setup(
        options: MockOptions,
    ) -> (ComposeView, Ctx, UnboundedReceiver<Action>, Arc<MockForge>) {
        let forge = Arc::new(MockForge::demo(options));
        let mut forges = Forges::new();
        forges.add(Arc::clone(&forge) as Arc<dyn Forge>);
        let (ctx, rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let repo = RepoRef::new(hit_core::Host::new(DEMO_HOST), "acme", "rocket");
        let target = Target {
            account: forge.account().clone(),
            item: ItemRef::pull_request(repo, 142),
        };
        (
            ComposeView::new(target, Purpose::Comment, ""),
            ctx,
            rx,
            forge,
        )
    }

    fn type_text(view: &mut ComposeView, text: &str, ctx: &Ctx) {
        for c in text.chars() {
            view.handle_event(&key(KeyCode::Char(c)), ctx);
        }
    }

    async fn done(rx: &mut UnboundedReceiver<Action>) -> Action {
        loop {
            let action = rx.recv().await.unwrap();
            if matches!(action, Action::Data(DataEvent::ActionDone { .. })) {
                return action;
            }
        }
    }

    #[tokio::test]
    async fn a_comment_reaches_the_forge_and_closes_the_box() {
        let (mut view, ctx, mut rx, forge) = setup(MockOptions::instant());
        type_text(&mut view, "looks good", &ctx);
        view.handle_event(&ctrl('s'), &ctx);
        assert!(view.sending);

        // Nothing is typed or sent again while it is on its way.
        type_text(&mut view, "!!", &ctx);
        assert_eq!(view.editor.text(), "looks good");

        let action = done(&mut rx).await;
        let next = view.update(&action, &ctx);
        assert!(matches!(
            next,
            Some(Action::CloseCompose { draft, .. }) if draft.is_empty()
        ));
        let detail = forge
            .pull_requests()
            .unwrap()
            .get(&view.target.item)
            .await
            .unwrap();
        assert_eq!(detail.comments.last().unwrap().body, "looks good");
    }

    #[tokio::test]
    async fn a_failed_send_keeps_the_text_and_says_why() {
        let (mut view, ctx, mut rx, _forge) = setup(MockOptions {
            failure_rate: 1.0,
            ..MockOptions::instant()
        });
        type_text(&mut view, "important words", &ctx);
        view.handle_event(&ctrl('s'), &ctx);
        let action = done(&mut rx).await;
        assert!(view.update(&action, &ctx).is_none());
        assert_eq!(view.editor.text(), "important words");
        assert!(view.error.as_deref().unwrap().starts_with("couldn't send"));
        assert!(!view.sending, "it can be tried again");
    }

    #[tokio::test]
    async fn an_approval_may_be_blank_and_a_request_for_changes_may_not() {
        let (view, ctx, mut rx, forge) = setup(MockOptions::instant());
        let repo = view.target.item.repo.clone();
        let target = Target {
            account: view.target.account.clone(),
            item: ItemRef::pull_request(repo, 145),
        };

        let mut changes = ComposeView::new(
            target.clone(),
            Purpose::Review(ReviewVerdict::RequestChanges),
            "",
        );
        changes.handle_event(&ctrl('s'), &ctx);
        assert!(!changes.sending);
        assert!(changes.error.is_some());

        let mut approve =
            ComposeView::new(target.clone(), Purpose::Review(ReviewVerdict::Approve), "");
        approve.handle_event(&ctrl('s'), &ctx);
        assert!(approve.sending);
        let action = done(&mut rx).await;
        assert!(approve.update(&action, &ctx).is_some());
        let detail = forge
            .pull_requests()
            .unwrap()
            .get(&target.item)
            .await
            .unwrap();
        assert_eq!(
            detail.reviews.last().unwrap().state,
            hit_core::model::ReviewState::Approved
        );
    }

    #[tokio::test]
    async fn the_forge_s_refusal_is_shown_as_it_is() {
        // #142 is the demo user's own.
        let (mut view, ctx, mut rx, _forge) = setup(MockOptions::instant());
        view.purpose = Purpose::Review(ReviewVerdict::Approve);
        view.handle_event(&ctrl('s'), &ctx);
        let action = done(&mut rx).await;
        assert!(view.update(&action, &ctx).is_none());
        assert!(
            view.error
                .as_deref()
                .unwrap()
                .contains("your own pull request"),
            "{:?}",
            view.error
        );
    }

    #[tokio::test]
    async fn nothing_blank_is_sent() {
        let (mut view, ctx, _rx, forge) = setup(MockOptions::instant());
        let before = forge
            .pull_requests()
            .unwrap()
            .get(&view.target.item)
            .await
            .unwrap()
            .comments
            .len();
        type_text(&mut view, "   ", &ctx);
        view.handle_event(&ctrl('s'), &ctx);
        assert!(!view.sending);
        assert!(view.error.is_some());
        let after = forge
            .pull_requests()
            .unwrap()
            .get(&view.target.item)
            .await
            .unwrap()
            .comments
            .len();
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn closing_keeps_the_draft() {
        let (mut view, ctx, _rx, _forge) = setup(MockOptions::instant());
        type_text(&mut view, "half a thought", &ctx);
        let close = view.handle_event(&key(KeyCode::Esc), &ctx);
        assert!(matches!(
            close,
            Some(Action::CloseCompose { draft, .. }) if draft == "half a thought"
        ));
    }

    #[tokio::test]
    async fn the_editor_takes_over_and_gives_back() {
        let (mut view, ctx, _rx, _forge) = setup(MockOptions::instant());
        type_text(&mut view, "draft", &ctx);
        assert!(matches!(
            view.handle_event(&ctrl('e'), &ctx),
            Some(Action::EditInEditor(text)) if text == "draft"
        ));
        view.update(&Action::EditorDone(Ok("from the editor".to_owned())), &ctx);
        assert_eq!(view.editor.text(), "from the editor");

        view.update(&Action::EditorDone(Err("no editor".to_owned())), &ctx);
        assert_eq!(view.editor.text(), "from the editor", "still there");
        assert_eq!(view.error.as_deref(), Some("no editor"));
    }

    #[tokio::test]
    async fn the_preview_shows_markdown_and_typing_does_not_go_in_blind() {
        let (mut view, ctx, _rx, _forge) = setup(MockOptions::instant());
        type_text(&mut view, "**bold**", &ctx);
        view.handle_event(&key(KeyCode::Tab), &ctx);
        type_text(&mut view, "x", &ctx);
        assert_eq!(view.editor.text(), "**bold**");

        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal
            .draw(|frame| view.render(frame, frame.area(), &ctx))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("bold") && !text.contains("**bold**"),
            "{text}"
        );
    }
}
