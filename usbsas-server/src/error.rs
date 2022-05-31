use actix_web::{error::ResponseError, HttpResponse};
use err_derive::Error;
use log::error;
use std::io;

#[derive(Debug, Error)]
pub(crate) enum ServiceError {
    #[error(display = "Internal Server Error")]
    InternalServerError,

    #[error(display = "BadRequest: {}", _0)]
    BadRequest(String),

    #[error(display = "Unauthorized")]
    Unauthorized,
}

#[derive(Debug)]
pub(crate) enum AuthentError {
    NotEnoughBytes,
    BadHmac,
}

// impl ResponseError trait allows to convert our errors into http responses with appropriate data
impl ResponseError for ServiceError {
    fn error_response(&self) -> HttpResponse {
        match *self {
            ServiceError::InternalServerError => {
                HttpResponse::InternalServerError().json("Internal Server Error, Please try later")
            }
            ServiceError::BadRequest(ref message) => HttpResponse::BadRequest().json(message),
            ServiceError::Unauthorized => HttpResponse::Unauthorized().json("Unauthorized"),
        }
    }
}

impl From<base64::DecodeError> for ServiceError {
    fn from(error: base64::DecodeError) -> ServiceError {
        ServiceError::BadRequest(format!(
            "Input data error: unable to decode base64: {:?}",
            error
        ))
    }
}

impl From<toml::de::Error> for ServiceError {
    fn from(_error: toml::de::Error) -> ServiceError {
        dbg!(_error);
        ServiceError::InternalServerError
    }
}

impl<T> From<std::sync::PoisonError<T>> for ServiceError {
    fn from(_error: std::sync::PoisonError<T>) -> ServiceError {
        dbg!(_error);
        ServiceError::InternalServerError
    }
}

impl From<io::Error> for ServiceError {
    fn from(_error: io::Error) -> ServiceError {
        dbg!(_error);
        ServiceError::InternalServerError
    }
}

impl From<serde_json::Error> for ServiceError {
    fn from(_error: serde_json::Error) -> ServiceError {
        dbg!(_error);
        ServiceError::InternalServerError
    }
}

impl From<nix::Error> for ServiceError {
    fn from(_error: nix::Error) -> ServiceError {
        dbg!(_error);
        ServiceError::InternalServerError
    }
}

impl From<AuthentError> for ServiceError {
    fn from(error: AuthentError) -> ServiceError {
        match error {
            AuthentError::NotEnoughBytes => {
                ServiceError::BadRequest("Not enough bytes".to_string())
            }
            AuthentError::BadHmac => ServiceError::Unauthorized,
        }
    }
}

impl From<std::string::FromUtf8Error> for ServiceError {
    fn from(error: std::string::FromUtf8Error) -> ServiceError {
        error!("{}", error);
        ServiceError::InternalServerError
    }
}

impl From<tempfile::PersistError> for ServiceError {
    fn from(error: tempfile::PersistError) -> ServiceError {
        error!("{}", error);
        ServiceError::InternalServerError
    }
}

impl From<usbsas_process::Error> for ServiceError {
    fn from(_error: usbsas_process::Error) -> ServiceError {
        dbg!(_error);
        ServiceError::InternalServerError
    }
}

impl From<hmac::digest::InvalidLength> for ServiceError {
    fn from(_error: hmac::digest::InvalidLength) -> ServiceError {
        dbg!(_error);
        ServiceError::InternalServerError
    }
}
