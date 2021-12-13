use actix_web::error::PayloadError;
use actix_web::ResponseError;
use awc::error::SendRequestError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Invalid command Line")]
    CommandError,
    #[error("Illegal inherited environment variable - {0}")]
    EnvError(#[from] std::env::VarError),
    #[error("Unable to spawn child - {0}")]
    SpawnError(std::io::Error),
    #[error("Child exited too quickly")]
    AlreadyDiedError,
    #[error("Child exited with non-zero exit code - {0}")]
    NonZeroExitError(i32),
    #[cfg(unix)]
    #[error("Child terminated by signal - {0}")]
    ExitBySignalError(&'static str),
}

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("Unable to send request to upstream: {0}")]
    Send(#[from] SendRequestError),
    #[error("Unable to receive response from upstream: {0}")]
    Receive(#[from] PayloadError),
}

impl ResponseError for HttpError {}
