//! Sentry read-API compatibility surface.
//!
//! Everything under `/api/0/` lives here so its error envelope, pagination, and
//! authentication cannot affect the native `/api` handlers.

pub mod auth;
pub mod cursor;
pub mod error;
pub mod handlers;
pub mod projection;
pub mod query;
pub mod tokens;

use actix_web::error::QueryPayloadError;
use actix_web::{http::header::ContentType, web, HttpRequest, HttpResponse};
use serde_json::json;

/// Convert query-string decoding failures into the compatibility envelope so no
/// extractor error escapes as a non-JSON response (FR-021).
fn query_error(error: QueryPayloadError, _req: &HttpRequest) -> actix_web::Error {
    let body = json!({ "detail": format!("invalid query string: {error}") }).to_string();
    let response = HttpResponse::BadRequest()
        .content_type(ContentType::json())
        .body(body);
    actix_web::error::InternalError::from_response(error, response).into()
}

/// Register the compatibility routes. The scope is always registered; when the
/// feature is disabled each handler answers `404` before authentication.
pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/0")
            .app_data(web::QueryConfig::default().error_handler(query_error))
            // Issues
            .route(
                "/projects/{org}/{project}/issues",
                web::get().to(handlers::list_issues),
            )
            .route(
                "/projects/{org}/{project}/issues/",
                web::get().to(handlers::list_issues),
            )
            .route(
                "/projects/{org}/{project}/issues",
                web::put().to(handlers::mutate_issues),
            )
            .route(
                "/projects/{org}/{project}/issues/",
                web::put().to(handlers::mutate_issues),
            )
            .route(
                "/projects/{org}/{project}/issues",
                web::delete().to(handlers::delete_issues),
            )
            .route(
                "/projects/{org}/{project}/issues/",
                web::delete().to(handlers::delete_issues),
            )
            // Tag values
            .route(
                "/projects/{org}/{project}/tags/{key}/values",
                web::get().to(handlers::project_tag_values),
            )
            .route(
                "/projects/{org}/{project}/tags/{key}/values/",
                web::get().to(handlers::project_tag_values),
            )
            .route(
                "/organizations/{org}/issues/{issue}/tags/{key}/values",
                web::get().to(handlers::issue_tag_values),
            )
            .route(
                "/organizations/{org}/issues/{issue}/tags/{key}/values/",
                web::get().to(handlers::issue_tag_values),
            )
            // Releases
            .route(
                "/organizations/{org}/releases/{version}/commitfiles",
                web::get().to(handlers::release_commit_files),
            )
            .route(
                "/organizations/{org}/releases/{version}/commitfiles/",
                web::get().to(handlers::release_commit_files),
            )
            // Service hooks (list + collection mutations)
            .route(
                "/projects/{org}/{project}/servicehooks",
                web::get().to(handlers::service_hooks),
            )
            .route(
                "/projects/{org}/{project}/servicehooks/",
                web::get().to(handlers::service_hooks),
            )
            .route(
                "/projects/{org}/{project}/servicehooks",
                web::post().to(handlers::service_hook_mutate),
            )
            .route(
                "/projects/{org}/{project}/servicehooks/",
                web::post().to(handlers::service_hook_mutate),
            )
            // Service hook detail
            .route(
                "/projects/{org}/{project}/servicehooks/{hook}",
                web::get().to(handlers::service_hook_detail),
            )
            .route(
                "/projects/{org}/{project}/servicehooks/{hook}/",
                web::get().to(handlers::service_hook_detail),
            )
            .route(
                "/projects/{org}/{project}/servicehooks/{hook}",
                web::delete().to(handlers::service_hook_detail_mutate),
            )
            .route(
                "/projects/{org}/{project}/servicehooks/{hook}/",
                web::delete().to(handlers::service_hook_detail_mutate),
            )
            // Legacy user feedback
            .route(
                "/projects/{org}/{project}/users",
                web::get().to(handlers::user_feedback),
            )
            .route(
                "/projects/{org}/{project}/users/",
                web::get().to(handlers::user_feedback),
            )
            .route(
                "/projects/{org}/{project}/users",
                web::post().to(handlers::user_feedback_create),
            )
            .route(
                "/projects/{org}/{project}/users/",
                web::post().to(handlers::user_feedback_create),
            )
            // Spike protection
            .route(
                "/projects/{org}/{project}/spike-protection",
                web::get().to(handlers::spike_protection),
            )
            .route(
                "/projects/{org}/{project}/spike-protection/",
                web::get().to(handlers::spike_protection),
            )
            .route(
                "/projects/{org}/{project}/spike-protection",
                web::put().to(handlers::spike_protection_mutate),
            )
            .route(
                "/projects/{org}/{project}/spike-protection/",
                web::put().to(handlers::spike_protection_mutate),
            )
            .route(
                "/projects/{org}/{project}/spike-protection",
                web::post().to(handlers::spike_protection_mutate),
            )
            .route(
                "/projects/{org}/{project}/spike-protection/",
                web::post().to(handlers::spike_protection_mutate),
            )
            // Sentry apps
            .route(
                "/organizations/{org}/sentry-app-installations",
                web::get().to(handlers::app_installations),
            )
            .route(
                "/organizations/{org}/sentry-app-installations/",
                web::get().to(handlers::app_installations),
            )
            .route(
                "/organizations/{org}/sentry-app-installations",
                web::post().to(handlers::app_installations_mutate),
            )
            .route(
                "/organizations/{org}/sentry-app-installations/",
                web::post().to(handlers::app_installations_mutate),
            )
            .route(
                "/organizations/{org}/sentry-app-installations/{installation}/external-issues",
                web::post().to(handlers::external_issues_mutate),
            )
            .route(
                "/organizations/{org}/sentry-app-installations/{installation}/external-issues/",
                web::post().to(handlers::external_issues_mutate),
            )
            .route(
                "/organizations/{org}/sentry-app-installations/{installation}/external-issues/{external_issue}",
                web::delete().to(handlers::external_issue_delete),
            )
            .route(
                "/organizations/{org}/sentry-app-installations/{installation}/external-issues/{external_issue}/",
                web::delete().to(handlers::external_issue_delete),
            ),
    );
}
