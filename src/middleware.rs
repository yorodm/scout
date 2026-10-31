//! Request-scoped observability middleware.

use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::header::{HeaderName, HeaderValue};
use actix_web::middleware::Next;
use actix_web::Error;
use std::time::Instant;
use uuid::Uuid;

/// Attach a request id (echoing a caller-supplied one when present) and log the
/// outcome with its latency. Handlers add project, event, and job ids to their
/// own events, so one id ties the whole path together.
pub async fn request_id<B: MessageBody + 'static>(
    req: ServiceRequest,
    next: Next<B>,
) -> Result<ServiceResponse<B>, Error> {
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let method = req.method().clone();
    let path = req.path().to_string();
    let started = Instant::now();

    let mut response = next.call(req).await?;

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), value);
    }
    tracing::debug!(
        request_id = %request_id,
        method = %method,
        path = %path,
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "request completed"
    );
    Ok(response)
}
