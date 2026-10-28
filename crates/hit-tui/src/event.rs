//! Terminal input, read on a background task.

use crossterm::event::{Event as CtEvent, EventStream, KeyEvent, KeyEventKind};
use futures::StreamExt;
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    Paste(String),
    Resize(u16, u16),
    FocusGained,
    FocusLost,
}

pub(crate) struct EventSource {
    rx: mpsc::UnboundedReceiver<Event>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl EventSource {
    pub(crate) fn spawn() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(read_loop(tx, cancel.clone()));
        Self { rx, cancel, task }
    }

    /// Next input event; `None` once the input stream has ended.
    pub(crate) async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }

    pub(crate) async fn stop(self) {
        self.cancel.cancel();
        if let Err(e) = self.task.await {
            tracing::warn!(error = %e, "input task did not finish cleanly");
        }
    }
}

async fn read_loop(tx: mpsc::UnboundedSender<Event>, cancel: CancellationToken) {
    let mut stream = EventStream::new();
    loop {
        let raw = tokio::select! {
            () = cancel.cancelled() => break,
            raw = stream.next() => raw,
        };
        let event = match raw {
            // Windows reports key releases too; only presses (and repeats) matter.
            Some(Ok(CtEvent::Key(key))) if key.kind != KeyEventKind::Release => Event::Key(key),
            Some(Ok(CtEvent::Paste(text))) => Event::Paste(text),
            Some(Ok(CtEvent::Resize(w, h))) => Event::Resize(w, h),
            Some(Ok(CtEvent::FocusGained)) => Event::FocusGained,
            Some(Ok(CtEvent::FocusLost)) => Event::FocusLost,
            Some(Ok(_)) => continue,
            Some(Err(e)) => {
                tracing::error!(error = %e, "reading terminal input failed");
                break;
            }
            None => break,
        };
        if tx.send(event).is_err() {
            break;
        }
    }
}
