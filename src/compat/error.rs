//! Error type for the Sentry-compatible surface.
//!
//! Every response uses Sentry's `{"detail": "<message>"}` envelope, which is
//! deliberately different from the native API's `{"error": ...}` shape.

use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use serde_json::json;
use std::fmt;

#[derive(Debug)]
pub enum CompatError {
    BadRequest(String),
    /// Missing, malformed, unknown, revoked, or disabled-owner token. The body is
    /// always identical so callers cannot distinguish the cases.
    Unauthorized,
    Forbidden(String),
    NotFound(String),
    Unsupported(String),
}

impl fmt::Display for CompatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompatError::BadRequest(message)
            | CompatError::Forbidden(message)
            | CompatError::NotFound(message)
            | CompatError::Unsupported(message) => write!(f, "{message}"),
            CompatError::Unauthorized => write!(f, "Invalid token"),
        }
    }
}

impl ResponseError for CompatError {
    fn status_code(&self) -> StatusCode {
        match self {
            CompatError::BadRequest(_) => StatusCode::BAD_REQUEST,
            CompatError::Unauthorized => StatusCode::UNAUTHORIZED,
            CompatError::Forbidden(_) => StatusCode::FORBIDDEN,
            CompatError::NotFound(_) => StatusCode::NOT_FOUND,
            CompatError::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
        }
    }

    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status_code()).json(json!({ "detail": self.to_string() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorized_body_is_uniform() {
        assert_eq!(CompatError::Unauthorized.to_string(), "Invalid token");
        assert_eq!(
            CompatError::Unauthorized.status_code(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn statuses_match_the_contract() {
        assert_eq!(
            CompatError::BadRequest("x".into()).status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            CompatError::Forbidden("x".into()).status_code(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            CompatError::NotFound("x".into()).status_code(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            CompatError::Unsupported("x".into()).status_code(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
}
