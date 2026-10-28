use std::io;

#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    #[error("could not set up the terminal")]
    Setup(#[source] io::Error),
    #[error("could not restore the terminal")]
    Restore(#[source] io::Error),
    #[error("could not draw to the terminal")]
    Draw(#[source] io::Error),
    #[error("terminal input stream closed unexpectedly")]
    InputClosed,
    #[error("hit stopped after an internal panic (see the report above and the log file)")]
    Panicked,
}
