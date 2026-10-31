use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use std::fmt;

#[derive(Debug)]
pub enum ApiError {
    Unauthorized(String),
    Forbidden(String),
    NotFound(String),
    BadRequest(String),
    Conflict(String),
    Gone(String),
    PayloadTooLarge(String),
    TooManyRequests {
        message: String,
        retry_after_secs: u64,
    },
    ServiceUnavailable(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized(message)
            | ApiError::Forbidden(message)
            | ApiError::NotFound(message)
            | ApiError::BadRequest(message)
            | ApiError::Conflict(message)
            | ApiError::Gone(message)
            | ApiError::PayloadTooLarge(message)
            | ApiError::ServiceUnavailable(message) => write!(f, "{message}"),
            ApiError::TooManyRequests { message, .. } => write!(f, "{message}"),
        }
    }
}

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::Gone(_) => StatusCode::GONE,
            ApiError::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS,
            ApiError::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let mut builder = HttpResponse::build(self.status_code());
        if let ApiError::TooManyRequests {
            retry_after_secs, ..
        } = self
        {
            builder.insert_header(("Retry-After", retry_after_secs.to_string()));
        }
        builder.json(serde_json::json!({ "error": self.to_string() }))
    }
}
