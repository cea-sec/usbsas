//! Error type shared by the HTTP handlers.
//!
//! Every variant maps to an HTTP status code through [`ResponseError`], so
//! handlers can simply use `?`. Internal errors are logged with their full
//! details but only a generic message is sent back to the client.

use actix_web::{HttpResponse, ResponseError, error::PayloadError, http::StatusCode};
use std::{fmt, io};

/// Errors returned by the API handlers.
#[derive(Debug)]
pub enum Error {
    /// The requested user, bundle or scan does not exist (404).
    NotFound(&'static str),
    /// Malformed request: invalid identifier, truncated body, ... (400).
    BadRequest(String),
    /// The uploaded body exceeds the configured maximum size (413).
    PayloadTooLarge(u64),
    /// The server is shutting down or clamd is unavailable (503).
    Unavailable(&'static str),
    /// Any other server-side failure (500).
    Internal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(what) => write!(f, "{what} not found"),
            Error::BadRequest(msg) => write!(f, "bad request: {msg}"),
            Error::PayloadTooLarge(max) => write!(f, "payload too large (max {max} bytes)"),
            Error::Unavailable(msg) => write!(f, "service unavailable: {msg}"),
            Error::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => Error::NotFound("file"),
            _ => Error::Internal(err.to_string()),
        }
    }
}

impl From<PayloadError> for Error {
    fn from(err: PayloadError) -> Self {
        Error::BadRequest(format!("error while receiving body: {err}"))
    }
}

impl ResponseError for Error {
    fn status_code(&self) -> StatusCode {
        match self {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::BadRequest(_) => StatusCode::BAD_REQUEST,
            Error::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Error::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let body = match self {
            Error::Internal(msg) => {
                log::error!("{msg}");
                "internal server error".to_string()
            }
            other => other.to_string(),
        };
        HttpResponse::build(self.status_code())
            .content_type("text/plain; charset=utf-8")
            .body(body)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
