//! Native token management endpoints and their dashboard contract.
//!
//! These live under the normal `/api` surface and keep its `{"error": ...}`
//! envelope. The Sentry-compatible surface only ever *uses* tokens created here.

use crate::error::ApiError;
use crate::models::{ApiToken, OrgRole};
use crate::state::AppState;
use actix_web::{web, HttpRequest, HttpResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.route("/api/me/tokens", web::get().to(list_own))
        .route("/api/me/tokens", web::post().to(create_own))
        .route("/api/me/tokens/{token_id}", web::delete().to(revoke_own))
        .route("/api/orgs/{org_id}/tokens", web::get().to(list_org))
        .route(
            "/api/orgs/{org_id}/tokens/{token_id}",
            web::delete().to(revoke_org),
        );
}

#[derive(Deserialize)]
pub struct CreateTokenRequest {
    pub label: String,
}

#[derive(Serialize)]
pub struct TokenView {
    pub id: Uuid,
    pub label: String,
    pub token_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<&ApiToken> for TokenView {
    fn from(token: &ApiToken) -> Self {
        Self {
            id: token.id,
            label: token.label.clone(),
            token_prefix: token.token_prefix.clone(),
            created_at: token.created_at,
            last_used_at: token.last_used_at,
            revoked_at: token.revoked_at,
        }
    }
}

#[derive(Serialize)]
pub struct CreatedToken {
    pub token: String,
    #[serde(flatten)]
    pub metadata: TokenView,
}

#[derive(Serialize)]
pub struct OrgTokenView {
    pub user_id: Uuid,
    pub user_email: String,
    #[serde(flatten)]
    pub token: TokenView,
}

fn valid_label(label: &str) -> Result<String, ApiError> {
    let trimmed = label.trim();
    let length = trimmed.chars().count();
    if length == 0 || length > 100 {
        return Err(ApiError::BadRequest(
            "label must be 1-100 characters".into(),
        ));
    }
    Ok(trimmed.to_string())
}

pub async fn list_own(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (user_id, _) = crate::api::auth_user(&req, &state).await?;
    let store = state.store.read().await;
    let tokens: Vec<TokenView> = store
        .api_tokens_for_user(user_id)
        .iter()
        .map(TokenView::from)
        .collect();
    Ok(HttpResponse::Ok().json(tokens))
}

pub async fn create_own(
    req: HttpRequest,
    body: web::Json<CreateTokenRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (user_id, email) = crate::api::auth_user(&req, &state).await?;
    let label = valid_label(&body.label)?;
    let mut store = state.store.write().await;
    let (plaintext, token) = store.create_api_token(user_id, &label);
    crate::audit::record(
        &mut store,
        None,
        None,
        &email,
        "token.create",
        Some(token.id.to_string()),
    );
    Ok(HttpResponse::Created().json(CreatedToken {
        token: plaintext,
        metadata: TokenView::from(&token),
    }))
}

pub async fn revoke_own(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (user_id, email) = crate::api::auth_user(&req, &state).await?;
    let token_id = Uuid::parse_str(&path.into_inner())
        .map_err(|_| ApiError::NotFound("token not found".into()))?;
    let mut store = state.store.write().await;
    let owned = store
        .api_tokens
        .get(&token_id)
        .is_some_and(|token| token.user_id == user_id);
    if !owned {
        return Err(ApiError::NotFound("token not found".into()));
    }
    store.revoke_api_token(token_id);
    crate::audit::record(
        &mut store,
        None,
        None,
        &email,
        "token.revoke",
        Some(token_id.to_string()),
    );
    Ok(HttpResponse::NoContent().finish())
}

pub async fn list_org(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (user_id, _) = crate::api::auth_user(&req, &state).await?;
    let org_id = path.into_inner();
    let store = state.store.read().await;
    require_org_admin(&store, user_id, org_id)?;
    let tokens: Vec<OrgTokenView> = store
        .api_tokens_for_org(org_id)
        .into_iter()
        .map(|(email, token)| OrgTokenView {
            user_id: token.user_id,
            user_email: email,
            token: TokenView::from(&token),
        })
        .collect();
    Ok(HttpResponse::Ok().json(tokens))
}

pub async fn revoke_org(
    req: HttpRequest,
    path: web::Path<(Uuid, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (user_id, email) = crate::api::auth_user(&req, &state).await?;
    let (org_id, token_ref) = path.into_inner();
    let token_id =
        Uuid::parse_str(&token_ref).map_err(|_| ApiError::NotFound("token not found".into()))?;
    let mut store = state.store.write().await;
    require_org_admin(&store, user_id, org_id)?;
    let belongs = store
        .api_tokens_for_org(org_id)
        .iter()
        .any(|(_, token)| token.id == token_id);
    if !belongs {
        return Err(ApiError::NotFound("token not found".into()));
    }
    store.revoke_api_token(token_id);
    crate::audit::record(
        &mut store,
        Some(org_id),
        None,
        &email,
        "token.revoke",
        Some(token_id.to_string()),
    );
    Ok(HttpResponse::NoContent().finish())
}

fn require_org_admin(
    store: &crate::store::Store,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<(), ApiError> {
    if !store.orgs.contains_key(&org_id) {
        return Err(ApiError::NotFound("organisation not found".into()));
    }
    match store.org_role_for(user_id, org_id) {
        Some(OrgRole::Admin) => Ok(()),
        _ => Err(ApiError::Forbidden("organisation admin required".into())),
    }
}
