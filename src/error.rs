use actix_web::error::PayloadError;
use actix_web::ResponseError;
use awc::error::SendRequestError;
use awc::http::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("Invalid command Line")]
    Command,
    #[error("Unable to spawn child - {0}")]
    Spawn(std::io::Error),
    #[error("Child exited too quickly")]
    AlreadyDied,
    #[error("Child exited with non-zero exit code - {0}")]
    NonZeroExit(i32),
    #[cfg(unix)]
    #[error("Child terminated by signal - {0}")]
    ExitBySignal(&'static str),
}

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("Unable to send request to upstream - {0}")]
    Send(#[from] SendRequestError),
    #[error("Unable to receive response from upstream - {0}")]
    Receive(#[from] PayloadError),
}

impl ResponseError for HttpError {}

#[derive(Debug, Error)]
pub enum HealthError {
    #[error("Unable to send request to upstream - {0}")]
    Send(SendRequestError),
    #[error("Non-success response status code - {0}")]
    Status(StatusCode),
}
