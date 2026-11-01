use crate::error::ApiError;
use crate::models::*;
use crate::state::AppState;
use crate::store::Store;
use actix_web::{web, HttpRequest, HttpResponse};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.route("/api/health", web::get().to(health))
        .route("/api/metrics", web::get().to(metrics))
        .route("/api/operator/users", web::get().to(operator_users))
        .route(
            "/api/operator/users/{user_id}/unlock",
            web::post().to(unlock_account),
        )
        .route("/api/login", web::post().to(login))
        .route("/api/logout", web::post().to(logout))
        .route("/api/me", web::get().to(me))
        .route("/api/me/password", web::post().to(change_password))
        .route(
            "/api/me/recovery-codes",
            web::post().to(reissue_recovery_codes),
        )
        .route("/api/recover", web::post().to(recover))
        .route("/api/orgs", web::get().to(list_orgs))
        .route("/api/orgs", web::post().to(create_org))
        .route(
            "/api/orgs/{org_id}/invitations",
            web::post().to(create_invitation),
        )
        .route(
            "/api/orgs/{org_id}/invitations/{invitation_id}",
            web::delete().to(revoke_invitation),
        )
        .route(
            "/api/orgs/{org_id}/projects",
            web::post().to(create_project),
        )
        .route("/api/orgs/{org_id}/oidc", web::get().to(get_oidc))
        .route("/api/orgs/{org_id}/oidc", web::put().to(put_oidc))
        .route("/api/orgs/{org_id}/oidc", web::delete().to(delete_oidc))
        .route("/api/oidc/{org_slug}/start", web::get().to(oidc_start))
        .route("/api/oidc/callback", web::get().to(oidc_callback))
        .route(
            "/api/invitations/{token}",
            web::get().to(preview_invitation),
        )
        .route(
            "/api/invitations/{token}/accept",
            web::post().to(accept_invitation),
        )
        .route(
            "/api/projects/{project_id}/keys/rotate",
            web::post().to(rotate_key),
        )
        .route("/api/projects", web::get().to(list_projects))
        .route("/api/projects/{project_id}", web::get().to(get_project))
        .route(
            "/api/projects/{project_id}",
            web::delete().to(soft_delete_project),
        )
        .route(
            "/api/projects/{project_id}/issues",
            web::get().to(list_issues),
        )
        .route("/api/projects/{project_id}/logs", web::get().to(list_logs))
        .route("/api/projects/{project_id}/search", web::get().to(search))
        .route(
            "/api/projects/{project_id}/members",
            web::get().to(list_members),
        )
        .route(
            "/api/projects/{project_id}/members",
            web::post().to(add_member),
        )
        .route(
            "/api/projects/{project_id}/members/{user_id}",
            web::put().to(update_member),
        )
        .route(
            "/api/projects/{project_id}/members/{user_id}",
            web::delete().to(remove_member),
        )
        .route("/api/projects/{project_id}/keys", web::get().to(list_keys))
        .route(
            "/api/projects/{project_id}/keys",
            web::post().to(create_key),
        )
        .route(
            "/api/projects/{project_id}/keys/{key_id}",
            web::delete().to(revoke_key),
        )
        .route(
            "/api/projects/{project_id}/settings",
            web::get().to(get_settings),
        )
        .route(
            "/api/projects/{project_id}/settings",
            web::put().to(update_settings),
        )
        .route("/api/issues/{issue_id}", web::get().to(get_issue))
        .route(
            "/api/issues/{issue_id}/occurrences",
            web::get().to(list_occurrences),
        )
        .route("/api/issues/{issue_id}/raw", web::get().to(get_raw))
        .route(
            "/api/issues/{issue_id}/state",
            web::post().to(set_issue_state),
        )
        .route(
            "/api/issues/{issue_id}/categorisation",
            web::post().to(categorise),
        );
    crate::compat::tokens::config(cfg);
}

// --- authentication helpers ------------------------------------------------

fn bearer(req: &HttpRequest) -> Option<String> {
    req.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

pub(crate) async fn auth_user(
    req: &HttpRequest,
    state: &AppState,
) -> Result<(Uuid, String), ApiError> {
    let token =
        bearer(req).ok_or_else(|| ApiError::Unauthorized("missing session token".into()))?;
    let mut store = state.store.write().await;
    let user_id = store
        .user_for_token(&token, state.session_idle_hours)
        .ok_or_else(|| ApiError::Unauthorized("invalid or expired session".into()))?;
    // FR-014: disabling or removing an account ends its sessions and tokens on
    // the next request, not at the next expiry.
    let Some(user) = store.users.get(&user_id) else {
        return Err(ApiError::Unauthorized("invalid or expired session".into()));
    };
    if user.disabled_at.is_some() {
        return Err(ApiError::Unauthorized("this account is disabled".into()));
    }
    let email = user.email.clone();
    Ok((user_id, email))
}

async fn auth_project(
    req: &HttpRequest,
    state: &AppState,
    project_id: Uuid,
) -> Result<(Uuid, String, Role), ApiError> {
    let (user_id, email) = auth_user(req, state).await?;
    let store = state.store.read().await;
    deny_project_access(&store, user_id, project_id)?;
    if !project_is_active(&store, project_id) {
        return Err(ApiError::NotFound("project not found".into()));
    }
    let role = store
        .role_for(user_id, project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
    Ok((user_id, email, role))
}

/// The shared project-scoped access rule: the operator is denied outright
/// (FR-012, SC-015) and a demo session may only reach demo organisations
/// (FR-011, SC-014). Both resolve as not-found so existence never leaks.
fn deny_project_access(
    store: &crate::store::Store,
    user_id: Uuid,
    project_id: Uuid,
) -> Result<(), ApiError> {
    if store
        .users
        .get(&user_id)
        .is_some_and(|user| user.is_operator)
    {
        return Err(ApiError::NotFound("project not found".into()));
    }
    if store.is_demo_user(user_id) && !project_in_demo_org(store, project_id) {
        return Err(ApiError::NotFound("project not found".into()));
    }
    Ok(())
}

/// Whether the project belongs to an organisation marked as demo.
fn project_in_demo_org(store: &crate::store::Store, project_id: Uuid) -> bool {
    store
        .projects
        .get(&project_id)
        .and_then(|project| store.orgs.get(&project.org_id))
        .is_some_and(|org| org.is_demo)
}

/// A soft-deleted project is hidden from every normal read, but its rows remain.
fn project_is_active(store: &crate::store::Store, project_id: Uuid) -> bool {
    store
        .projects
        .get(&project_id)
        .is_some_and(|project| project.deleted_at.is_none())
}

fn issue_state_str(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "open",
        IssueState::Resolved => "resolved",
        IssueState::Ignored => "ignored",
    }
}

fn memberships_json(store: &crate::store::Store, user_id: Uuid) -> Vec<Value> {
    let demo_confined = store.is_demo_user(user_id);
    store
        .memberships
        .iter()
        .filter(|((uid, _), _)| *uid == user_id)
        .filter_map(|((_, project_id), role)| {
            let project = store.projects.get(project_id)?;
            // A demo session must not learn that a real project exists.
            if demo_confined
                && !store
                    .orgs
                    .get(&project.org_id)
                    .is_some_and(|org| org.is_demo)
            {
                return None;
            }
            Some(json!({
                "project_id": project_id,
                "project_name": project.name,
                "slug": project.slug,
                "role": role,
            }))
        })
        .collect()
}

// --- demo-only routes ------------------------------------------------------

/// Registered only when `SCOUT_DEMO_MODE` is enabled. These routes are not part
/// of the product contract: `demo_accounts` lists every seeded account and
/// `reset_demo` wipes the store without authentication.
pub fn demo_config(cfg: &mut web::ServiceConfig) {
    cfg.route("/api/demo/accounts", web::get().to(demo_accounts))
        .route("/api/demo/reset", web::post().to(reset_demo));
}

// --- session ---------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

fn invalid_credentials() -> ApiError {
    // The same response for an unknown email and a wrong password, so account
    // existence cannot be probed.
    ApiError::Unauthorized("invalid email or password".into())
}

pub async fn login(
    body: web::Json<LoginRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let mut store = state.store.write().await;
    // Expired rows are rejected on lookup; sweep them on each successful sign-in
    // so they do not accumulate between restarts.
    store.sweep_sessions();
    let user = store
        .user_by_email(&body.email)
        .cloned()
        .ok_or_else(invalid_credentials)?;
    if user.disabled_at.is_some() {
        return Err(ApiError::Forbidden("this account is disabled".into()));
    }
    // FR-032: a locked account looks exactly like a wrong password, even when
    // the password is correct, so the lock never reveals that the account exists.
    if user.locked_at.is_some() {
        return Err(invalid_credentials());
    }
    let Some(hash) = user.password_hash.as_deref() else {
        // An account that only authenticates through OIDC has no local password.
        return Err(invalid_credentials());
    };
    if !crate::auth::verify_password(&body.password, hash) {
        let locked = store.record_failed_sign_in(user.id);
        if locked {
            tracing::warn!(account = %user.email, "account locked after repeated failed sign-ins");
        }
        return Err(invalid_credentials());
    }
    store.clear_failed_sign_ins(user.id);
    let token = store.issue_session(user.id, state.session_ttl_days);
    // FR-012: the operator's recovery codes are issued once, at the first
    // successful sign-in, so a lost password is recoverable without database
    // access. The plaintext is returned exactly once and never stored.
    let recovery_codes = if user.is_operator && !store.has_recovery_codes(user.id) {
        let codes = crate::auth::issue_recovery_codes(&mut store, user.id, 10);
        crate::audit::record(
            &mut store,
            None,
            None,
            &user.email,
            "recovery_codes.issued",
            Some("first sign-in".into()),
        );
        codes
    } else {
        Vec::new()
    };
    let memberships = memberships_json(&store, user.id);
    Ok(HttpResponse::Ok().json(json!({
        "token": token,
        "user": user,
        "memberships": memberships,
        "recovery_codes": recovery_codes,
    })))
}

pub async fn logout(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    if let Some(token) = bearer(&req) {
        state.store.write().await.revoke_session(&token);
    }
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

pub async fn me(req: HttpRequest, state: web::Data<AppState>) -> Result<HttpResponse, ApiError> {
    let (user_id, _) = auth_user(&req, &state).await?;
    let store = state.store.read().await;
    let user = store
        .users
        .get(&user_id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    let memberships = memberships_json(&store, user_id);
    Ok(HttpResponse::Ok().json(json!({ "user": user, "memberships": memberships })))
}

pub async fn demo_accounts(state: web::Data<AppState>) -> Result<HttpResponse, ApiError> {
    let store = state.store.read().await;
    let mut accounts = Vec::new();
    for user in store.users.values().filter(|user| !user.is_operator) {
        // SC-014: only members of the seeded demo organisation are listed.
        if !store.is_demo_user(user.id) {
            continue;
        }
        let memberships = memberships_json(&store, user.id);
        accounts.push(json!({
            "email": user.email,
            "name": user.name,
            "memberships": memberships,
        }));
    }
    accounts.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(HttpResponse::Ok().json(json!({ "accounts": accounts })))
}

pub async fn reset_demo(state: web::Data<AppState>) -> HttpResponse {
    // A demo reset replaces only demo-organisation rows: real organisations,
    // their projects and records, live sessions, and the deployment operator are
    // preserved by `Store::reset_demo` (FR-011, SC-014).
    let (fresh, raws) = crate::store::seed(&state.self_url);
    for (key, data) in raws {
        if let Err(error) = state.objects.put_create(&key, data).await {
            return HttpResponse::InternalServerError()
                .json(json!({ "error": format!("could not reseed raw payload: {error}") }));
        }
    }
    state.store.write().await.reset_demo(fresh);
    crate::classifier::reset_outage();
    HttpResponse::Ok().json(json!({ "ok": true }))
}

pub async fn health(state: web::Data<AppState>) -> HttpResponse {
    let objects_ok = state.objects.healthy().await;
    let healthy = state.storage_ready() && objects_ok;
    let body = json!({
        "status": if healthy { "ok" } else { "degraded" },
        "service": "scout",
        "version": env!("CARGO_PKG_VERSION"),
        "storage": if state.storage_ready() { "ok" } else { "unavailable" },
        "object_store": if objects_ok {
            state.objects.backend()
        } else {
            "unavailable"
        },
    });
    if healthy {
        HttpResponse::Ok().json(body)
    } else {
        HttpResponse::ServiceUnavailable().json(body)
    }
}

// --- operational counters --------------------------------------------------

/// Operational counters for the deployment operator (FR-025). Scoped to the
/// operator because the numbers describe deployment-wide traffic.
pub async fn metrics(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    auth_operator(&req, &state).await?;
    Ok(HttpResponse::Ok().json(state.metrics.snapshot(state.store_kind)))
}

// --- operator account administration --------------------------------------

#[derive(Deserialize, Default)]
pub struct OperatorUserQuery {
    /// `?locked=true` lists only accounts locked by failed sign-ins.
    #[serde(default)]
    pub locked: Option<bool>,
}

/// Lists accounts for the operator console. Deployment-level only: it exposes
/// account emails and lock state, never project telemetry (FR-012).
pub async fn operator_users(
    req: HttpRequest,
    query: web::Query<OperatorUserQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    auth_operator(&req, &state).await?;
    let store = state.store.read().await;
    let mut accounts: Vec<User> = if query.locked.unwrap_or(false) {
        store.locked_users()
    } else {
        store.users.values().cloned().collect()
    };
    if !query.locked.unwrap_or(false) {
        accounts.sort_by(|a, b| a.email.cmp(&b.email));
    }
    let users: Vec<Value> = accounts
        .into_iter()
        .map(|user| {
            json!({
                "id": user.id,
                "email": user.email,
                "locked_at": user.locked_at,
            })
        })
        .collect();
    Ok(HttpResponse::Ok().json(json!({ "users": users })))
}

/// Unlocks a locked account and resets its failure count (FR-032, SC-023).
pub async fn unlock_account(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (_, email) = auth_operator(&req, &state).await?;
    let user_id = path.into_inner();
    let mut store = state.store.write().await;
    if !store.unlock_user(user_id) {
        return Err(ApiError::NotFound("account not found".into()));
    }
    let target = store
        .users
        .get(&user_id)
        .map(|user| user.email.clone())
        .unwrap_or_default();
    crate::audit::record(
        &mut store,
        None,
        None,
        &email,
        "user.unlocked",
        Some(target),
    );
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

// --- projects --------------------------------------------------------------

pub async fn list_projects(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (user_id, _) = auth_user(&req, &state).await?;
    let store = state.store.read().await;
    let demo_confined = store.is_demo_user(user_id);
    let mut projects = Vec::new();
    for ((member, project_id), role) in &store.memberships {
        if *member != user_id {
            continue;
        }
        let Some(project) = store.projects.get(project_id) else {
            continue;
        };
        if project.deleted_at.is_some() {
            continue;
        }
        // SC-014: a demo session sees only demo-organisation projects.
        if demo_confined && !project_in_demo_org(&store, *project_id) {
            continue;
        }
        let org_name = store
            .orgs
            .get(&project.org_id)
            .map(|o| o.name.clone())
            .unwrap_or_default();
        let open_issues = store
            .issues
            .values()
            .filter(|i| i.project_id == *project_id && i.state == IssueState::Open)
            .count();
        let total_issues = store
            .issues
            .values()
            .filter(|i| i.project_id == *project_id)
            .count();
        let logs = store
            .logs
            .iter()
            .filter(|l| l.project_id == *project_id)
            .count();
        let key_prefix = if role.can_manage() {
            store
                .active_key_for(*project_id)
                .map(|k| k.key_prefix.clone())
        } else {
            None
        };
        projects.push(json!({
            "id": project_id,
            "dsn_id": project.dsn_id,
            "slug": project.slug,
            "name": project.name,
            "org_name": org_name,
            "role": role,
            "open_issues": open_issues,
            "total_issues": total_issues,
            "logs": logs,
            "model_configured": project.model.is_some(),
            "retention_days": project.retention_days,
            "key_prefix": key_prefix,
        }));
    }
    projects.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(HttpResponse::Ok().json(json!({ "projects": projects })))
}

pub async fn get_project(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    let (_, _, role) = auth_project(&req, &state, project_id).await?;
    let store = state.store.read().await;
    let project = store
        .projects
        .get(&project_id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("project not found".into()))?;
    let key_prefix = if role.can_manage() {
        store
            .active_key_for(project_id)
            .map(|k| k.key_prefix.clone())
    } else {
        None
    };
    let (project, api_key_configured) = public_project(project);
    Ok(HttpResponse::Ok().json(json!({
        "project": project,
        "role": role,
        "key_prefix": key_prefix,
        "model_api_key_configured": api_key_configured,
    })))
}

/// FR-036: owner-only soft-delete. Rows, objects, and logs are retained and stay
/// subject to retention; the project disappears from normal reads and ingest.
pub async fn soft_delete_project(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    let (user_id, email, role) = auth_project(&req, &state, project_id).await?;
    if !role.is_owner() {
        return Err(ApiError::Forbidden(
            "only the project owner can delete it".into(),
        ));
    }
    let mut store = state.store.write().await;
    {
        let project = store
            .projects
            .get_mut(&project_id)
            .ok_or_else(|| ApiError::NotFound("project not found".into()))?;
        project.deleted_at = Some(Utc::now());
        project.deleted_by = Some(user_id);
    }
    crate::audit::record(
        &mut store,
        None,
        Some(project_id),
        &email,
        "project.soft_deleted",
        None,
    );
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

// --- organisations, invitations, and provisioning --------------------------

async fn auth_operator(req: &HttpRequest, state: &AppState) -> Result<(Uuid, String), ApiError> {
    let (user_id, email) = auth_user(req, state).await?;
    let store = state.store.read().await;
    if !store
        .users
        .get(&user_id)
        .is_some_and(|user| user.is_operator)
    {
        return Err(ApiError::Forbidden(
            "deployment operator role is required".into(),
        ));
    }
    Ok((user_id, email))
}

async fn auth_org_admin(
    req: &HttpRequest,
    state: &AppState,
    org_id: Uuid,
) -> Result<(Uuid, String), ApiError> {
    let (user_id, email) = auth_user(req, state).await?;
    let store = state.store.read().await;
    // FR-012: deployment-level only; organisation administration stays with the
    // organisation's own members.
    if store
        .users
        .get(&user_id)
        .is_some_and(|user| user.is_operator)
    {
        return Err(ApiError::Forbidden(
            "organisation administration belongs to its members".into(),
        ));
    }
    if store.org_role_for(user_id, org_id) != Some(OrgRole::Admin) {
        return Err(ApiError::Forbidden(
            "organisation admin role is required".into(),
        ));
    }
    Ok((user_id, email))
}

fn issue_invitation(
    store: &mut Store,
    org_id: Uuid,
    kind: InvitationKind,
    project_id: Option<Uuid>,
    email: &str,
    role: Option<Role>,
    created_by: Uuid,
) -> (Uuid, String) {
    let token = crate::auth::new_token();
    let token_hash = crate::auth::hash_token(&token);
    let id = Uuid::new_v4();
    store.invitations.insert(
        id,
        Invitation {
            id,
            org_id,
            kind,
            project_id,
            email: email.to_string(),
            role,
            token_hash: token_hash.clone(),
            created_by,
            expires_at: Utc::now() + chrono::Duration::days(7),
            accepted_at: None,
            accepted_user_id: None,
            revoked_at: None,
        },
    );
    store.invitations_by_token.insert(token_hash, id);
    (id, token)
}

fn invitation_json(store: &Store, invitation: &Invitation) -> Value {
    let organisation = store
        .orgs
        .get(&invitation.org_id)
        .map(|org| org.name.clone())
        .unwrap_or_default();
    let project = invitation
        .project_id
        .and_then(|project_id| store.projects.get(&project_id))
        .map(|project| project.name.clone());
    json!({
        "id": invitation.id,
        "kind": invitation.kind,
        "email": invitation.email,
        "organisation": organisation,
        "project": project,
        "role": invitation.role,
        "expires_at": invitation.expires_at,
        "accepted_at": invitation.accepted_at,
        "revoked_at": invitation.revoked_at,
    })
}

#[derive(Deserialize)]
pub struct CreateOrgRequest {
    pub name: String,
    pub initial_admin_email: String,
}

pub async fn list_orgs(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    auth_operator(&req, &state).await?;
    let store = state.store.read().await;
    let mut organisations: Vec<Value> = store
        .orgs
        .values()
        .map(|org| {
            json!({
                "id": org.id,
                "name": org.name,
                "slug": org.slug,
                "is_demo": org.is_demo,
                "created_by": org.created_by,
                "created_at": org.created_at,
            })
        })
        .collect();
    organisations.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(HttpResponse::Ok().json(json!({ "organisations": organisations })))
}

pub async fn create_org(
    req: HttpRequest,
    body: web::Json<CreateOrgRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (operator_id, email) = auth_operator(&req, &state).await?;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::BadRequest(
            "organisation name must be 1-120 characters".into(),
        ));
    }
    let admin_email = body.initial_admin_email.trim().to_ascii_lowercase();
    if !admin_email.contains('@') {
        return Err(ApiError::BadRequest(
            "initial_admin_email must be a valid email address".into(),
        ));
    }
    let mut store = state.store.write().await;
    let org_id = Uuid::new_v4();
    let slug = store.unique_org_slug(name);
    let organisation = Organisation {
        id: org_id,
        name: name.to_string(),
        slug,
        sso_provider: None,
        created_by: Some(operator_id),
        is_demo: false,
        created_at: Utc::now(),
    };
    store.orgs.insert(org_id, organisation.clone());
    let (_, token) = issue_invitation(
        &mut store,
        org_id,
        InvitationKind::OrgAdminSetup,
        None,
        &admin_email,
        None,
        operator_id,
    );
    crate::audit::record(
        &mut store,
        Some(org_id),
        None,
        &email,
        "organisation.created",
        Some(name.to_string()),
    );
    crate::audit::record(
        &mut store,
        Some(org_id),
        None,
        &email,
        "org_admin_setup.issued",
        Some(admin_email),
    );
    Ok(HttpResponse::Ok().json(json!({
        "organisation": organisation,
        "initial_admin_setup_token": token,
    })))
}

pub async fn preview_invitation(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let token = path.into_inner();
    let store = state.store.read().await;
    let invitation = store
        .invitation_by_token(&token)
        .ok_or_else(|| ApiError::NotFound("invitation not found".into()))?;
    if !invitation.is_pending() {
        return Err(ApiError::Gone(
            "invitation is expired, revoked, or already accepted".into(),
        ));
    }
    let view = invitation_json(&store, invitation);
    Ok(HttpResponse::Ok().json(json!({
        "email": view["email"],
        "organisation": view["organisation"],
        "project": view["project"],
        "role": view["role"],
        "expires_at": view["expires_at"],
    })))
}

#[derive(Deserialize)]
pub struct AcceptInvitationRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

pub async fn accept_invitation(
    req: HttpRequest,
    path: web::Path<String>,
    body: web::Json<AcceptInvitationRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let token = path.into_inner();
    let mut store = state.store.write().await;
    let invitation = store
        .invitation_by_token(&token)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("invitation not found".into()))?;
    if !invitation.is_pending() {
        return Err(ApiError::Gone(
            "invitation is expired, revoked, or already accepted".into(),
        ));
    }

    let email = invitation.email.to_ascii_lowercase();
    let existing = store.user_by_email(&email).map(|user| user.id);
    let session_user =
        bearer(&req).and_then(|token| store.user_for_token(&token, state.session_idle_hours));

    let (user_id, issued_token, recovery_codes) = match existing {
        Some(existing_id) => {
            if session_user != Some(existing_id) {
                return Err(ApiError::Unauthorized(
                    "sign in to accept this invitation".into(),
                ));
            }
            if body.password.is_some() {
                return Err(ApiError::BadRequest(
                    "an existing account cannot set a password here".into(),
                ));
            }
            (existing_id, None, Vec::new())
        }
        None => {
            let password = body.password.as_deref().ok_or_else(|| {
                ApiError::BadRequest("password is required for a new account".into())
            })?;
            let name = body
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| email.split('@').next().unwrap_or("Member"));
            let user = crate::auth::create_local_account(&mut store, &email, name, password)
                .map_err(ApiError::BadRequest)?;
            let token = store.issue_session(user.id, state.session_ttl_days);
            let codes = crate::auth::issue_recovery_codes(&mut store, user.id, 10);
            (user.id, Some(token), codes)
        }
    };

    match invitation.kind {
        InvitationKind::OrgAdminSetup => {
            store
                .org_members
                .insert((user_id, invitation.org_id), OrgRole::Admin);
        }
        InvitationKind::ProjectInvite => {
            let project_id = invitation
                .project_id
                .ok_or_else(|| ApiError::BadRequest("invitation has no project".into()))?;
            let role = invitation
                .role
                .ok_or_else(|| ApiError::BadRequest("invitation has no role".into()))?;
            store.memberships.insert((user_id, project_id), role);
            store
                .org_members
                .entry((user_id, invitation.org_id))
                .or_insert(OrgRole::Member);
        }
    }

    if let Some(stored) = store.invitations.get_mut(&invitation.id) {
        stored.accepted_at = Some(Utc::now());
        stored.accepted_user_id = Some(user_id);
    }
    let actor = store
        .users
        .get(&user_id)
        .map(|user| user.email.clone())
        .unwrap_or_default();
    crate::audit::record(
        &mut store,
        Some(invitation.org_id),
        invitation.project_id,
        &actor,
        "invitation.accepted",
        Some(invitation.id.to_string()),
    );

    let user = store.users.get(&user_id).cloned();
    let memberships = memberships_json(&store, user_id);
    Ok(HttpResponse::Ok().json(json!({
        "accepted": true,
        "token": issued_token,
        "user": user,
        "memberships": memberships,
        "recovery_codes": recovery_codes,
    })))
}

#[derive(Deserialize)]
pub struct CreateInvitationRequest {
    pub email: String,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub role: Option<Role>,
}

pub async fn create_invitation(
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<CreateInvitationRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let org_id = path.into_inner();
    let (actor_id, email) = auth_org_admin(&req, &state, org_id).await?;
    let invite_email = body.email.trim().to_ascii_lowercase();
    if !invite_email.contains('@') {
        return Err(ApiError::BadRequest("email must be a valid address".into()));
    }
    let project_id = body
        .project_id
        .ok_or_else(|| ApiError::BadRequest("project_id is required".into()))?;
    let role = body
        .role
        .ok_or_else(|| ApiError::BadRequest("role is required".into()))?;

    let mut store = state.store.write().await;
    {
        let project = store
            .projects
            .get(&project_id)
            .ok_or_else(|| ApiError::NotFound("project not found".into()))?;
        if project.org_id != org_id || project.deleted_at.is_some() {
            return Err(ApiError::BadRequest(
                "project does not belong to this organisation".into(),
            ));
        }
    }
    let (id, token) = issue_invitation(
        &mut store,
        org_id,
        InvitationKind::ProjectInvite,
        Some(project_id),
        &invite_email,
        Some(role),
        actor_id,
    );
    let invitation = store.invitations.get(&id).cloned().expect("just inserted");
    let view = invitation_json(&store, &invitation);
    crate::audit::record(
        &mut store,
        Some(org_id),
        Some(project_id),
        &email,
        "invitation.issued",
        Some(invite_email),
    );
    Ok(HttpResponse::Ok().json(json!({ "invitation": view, "token": token })))
}

pub async fn revoke_invitation(
    req: HttpRequest,
    path: web::Path<(Uuid, Uuid)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (org_id, invitation_id) = path.into_inner();
    let (_, email) = auth_org_admin(&req, &state, org_id).await?;
    let mut store = state.store.write().await;
    let project_id = {
        let invitation = store
            .invitations
            .get_mut(&invitation_id)
            .ok_or_else(|| ApiError::NotFound("invitation not found".into()))?;
        if invitation.org_id != org_id {
            return Err(ApiError::NotFound("invitation not found".into()));
        }
        if !invitation.is_pending() {
            return Err(ApiError::Gone(
                "invitation is expired, revoked, or already accepted".into(),
            ));
        }
        invitation.revoked_at = Some(Utc::now());
        invitation.project_id
    };
    crate::audit::record(
        &mut store,
        Some(org_id),
        project_id,
        &email,
        "invitation.revoked",
        Some(invitation_id.to_string()),
    );
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

// --- organisation OIDC (FR-018) --------------------------------------------

/// Organisation admins manage the connection; the deployment operator may do so
/// for any organisation (contracts/oidc.md).
async fn auth_org_admin_or_operator(
    req: &HttpRequest,
    state: &AppState,
    org_id: Uuid,
) -> Result<(Uuid, String), ApiError> {
    let (user_id, email) = auth_user(req, state).await?;
    let store = state.store.read().await;
    if !store.orgs.contains_key(&org_id) {
        return Err(ApiError::NotFound("organisation not found".into()));
    }
    let is_operator = store
        .users
        .get(&user_id)
        .is_some_and(|user| user.is_operator);
    if is_operator || store.org_role_for(user_id, org_id) == Some(OrgRole::Admin) {
        Ok((user_id, email))
    } else {
        Err(ApiError::Forbidden(
            "organisation admin role is required".into(),
        ))
    }
}

fn oidc_json(connection: &OidcConnection) -> Value {
    json!({
        "configured": true,
        "issuer": connection.issuer,
        "client_id": connection.client_id,
        "scopes": connection.scopes,
        "configured_at": connection.created_at,
    })
}

pub async fn get_oidc(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let org_id = path.into_inner();
    auth_org_admin_or_operator(&req, &state, org_id).await?;
    let store = state.store.read().await;
    let connection = store
        .oidc_connection(org_id)
        .ok_or_else(|| ApiError::NotFound("no OIDC connection configured".into()))?;
    Ok(HttpResponse::Ok().json(oidc_json(connection)))
}

#[derive(Deserialize)]
pub struct OidcConfigRequest {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub scopes: Option<String>,
}

pub async fn put_oidc(
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<OidcConfigRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let org_id = path.into_inner();
    let (_, email) = auth_org_admin_or_operator(&req, &state, org_id).await?;

    let issuer = body.issuer.trim().trim_end_matches('/').to_string();
    if !crate::oidc::valid_issuer(&issuer) {
        return Err(ApiError::BadRequest(
            "issuer must be an absolute http(s) URL".into(),
        ));
    }
    let client_id = body.client_id.trim().to_string();
    if client_id.is_empty() {
        return Err(ApiError::BadRequest("client_id is required".into()));
    }
    if body.client_secret.is_empty() {
        return Err(ApiError::BadRequest("client_secret is required".into()));
    }
    let scopes =
        crate::oidc::normalise_scopes(body.scopes.as_deref()).map_err(ApiError::BadRequest)?;

    // Validate discovery synchronously so an unreachable or malformed provider
    // fails here and changes nothing.
    crate::oidc::discover(&issuer)
        .await
        .map_err(|error| ApiError::BadRequest(format!("provider discovery failed: {error}")))?;

    let now = Utc::now();
    let mut store = state.store.write().await;
    store.set_oidc_connection(OidcConnection {
        org_id,
        issuer: issuer.clone(),
        client_id,
        client_secret: body.client_secret.clone(),
        scopes,
        created_at: now,
        updated_at: now,
    });
    crate::audit::record(
        &mut store,
        Some(org_id),
        None,
        &email,
        "oidc.configured",
        Some(issuer),
    );
    let connection = store.oidc_connection(org_id).expect("just stored");
    Ok(HttpResponse::Ok().json(oidc_json(connection)))
}

pub async fn delete_oidc(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let org_id = path.into_inner();
    let (_, email) = auth_org_admin_or_operator(&req, &state, org_id).await?;
    let mut store = state.store.write().await;
    if !store.remove_oidc_connection(org_id) {
        return Err(ApiError::NotFound("no OIDC connection configured".into()));
    }
    crate::audit::record(&mut store, Some(org_id), None, &email, "oidc.removed", None);
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
pub struct OidcCallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// The fixed landing target. The fragment is deliberate: browsers never send it
/// to a server, and the UI strips it after storing the session.
fn oidc_landing(state: &AppState, outcome: &str, token: Option<&str>) -> HttpResponse {
    let mut url = format!("{}/#oidc={outcome}", state.self_url.trim_end_matches('/'));
    if let Some(token) = token {
        url.push_str("&token=");
        url.push_str(token);
    }
    HttpResponse::Found()
        .insert_header(("Location", url))
        .finish()
}

pub async fn oidc_start(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let slug = path.into_inner();
    let (org_id, connection) = {
        let store = state.store.read().await;
        let org = store
            .orgs
            .values()
            .find(|org| org.slug == slug)
            .ok_or_else(|| ApiError::NotFound("organisation not found".into()))?;
        let connection = store
            .oidc_connection(org.id)
            .cloned()
            .ok_or_else(|| ApiError::NotFound("no OIDC connection configured".into()))?;
        (org.id, connection)
    };

    let started = crate::oidc::start(&connection, &state.self_url)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "OIDC start failed");
            ApiError::ServiceUnavailable("the identity provider is unavailable".into())
        })?;

    let now = Utc::now();
    state.store.write().await.insert_oidc_flow(OidcLoginFlow {
        state: started.state,
        org_id,
        nonce: started.nonce,
        pkce_verifier: started.pkce_verifier,
        return_to: None,
        created_at: now,
        expires_at: now + chrono::Duration::minutes(10),
    });
    Ok(HttpResponse::Found()
        .insert_header(("Location", started.authorize_url))
        .finish())
}

/// Consume a pending invitation exactly like a local acceptance, granting only
/// the role it already carries.
fn consume_oidc_invitation(store: &mut Store, invitation: &Invitation, user_id: Uuid, email: &str) {
    match invitation.kind {
        InvitationKind::OrgAdminSetup => {
            store
                .org_members
                .insert((user_id, invitation.org_id), OrgRole::Admin);
        }
        InvitationKind::ProjectInvite => {
            if let (Some(project_id), Some(role)) = (invitation.project_id, invitation.role) {
                store.memberships.insert((user_id, project_id), role);
            }
            store
                .org_members
                .entry((user_id, invitation.org_id))
                .or_insert(OrgRole::Member);
        }
    }
    if let Some(stored) = store.invitations.get_mut(&invitation.id) {
        stored.accepted_at = Some(Utc::now());
        stored.accepted_user_id = Some(user_id);
    }
    crate::audit::record(
        store,
        Some(invitation.org_id),
        invitation.project_id,
        email,
        "invitation.accepted",
        Some(invitation.id.to_string()),
    );
}

pub async fn oidc_callback(
    query: web::Query<OidcCallbackQuery>,
    state: web::Data<AppState>,
) -> HttpResponse {
    let Some(state_param) = query.state.clone().filter(|value| !value.is_empty()) else {
        return oidc_landing(&state, "state_invalid", None);
    };
    // Single use: the row is removed before the code is exchanged.
    let flow = state.store.write().await.take_oidc_flow(&state_param);
    let Some(flow) = flow else {
        return oidc_landing(&state, "state_invalid", None);
    };

    let connection = {
        let store = state.store.read().await;
        store.oidc_connection(flow.org_id).cloned()
    };
    let Some(connection) = connection else {
        return oidc_landing(&state, "provider_error", None);
    };
    if query.error.is_some() {
        return oidc_landing(&state, "provider_error", None);
    }
    let Some(code) = query.code.clone().filter(|value| !value.is_empty()) else {
        return oidc_landing(&state, "provider_error", None);
    };

    let identity = match crate::oidc::finish(&connection, &state.self_url, &flow, &code).await {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(%error, "OIDC token exchange or ID-token validation failed");
            return oidc_landing(&state, "provider_error", None);
        }
    };
    tracing::debug!(subject = %identity.subject, "OIDC identity verified");

    if !identity.email_verified {
        return oidc_landing(&state, "email_unverified", None);
    }
    let Some(email) = identity
        .email
        .map(|email| email.trim().to_ascii_lowercase())
        .filter(|email| !email.is_empty())
    else {
        return oidc_landing(&state, "email_unverified", None);
    };

    let mut store = state.store.write().await;
    let Some(user) = store.user_by_email(&email).cloned() else {
        // No account is created by provider sign-in (US3/AC2).
        return oidc_landing(&state, "membership_unknown", None);
    };
    if user.disabled_at.is_some() {
        return oidc_landing(&state, "membership_unknown", None);
    }
    if store.org_role_for(user.id, flow.org_id).is_none() {
        // An invitation in this organisation also matches, and is consumed with
        // exactly the role it already carries.
        let pending = store
            .invitations
            .values()
            .find(|invitation| {
                invitation.org_id == flow.org_id
                    && invitation.is_pending()
                    && invitation.email.eq_ignore_ascii_case(&email)
            })
            .cloned();
        let Some(invitation) = pending else {
            return oidc_landing(&state, "membership_unknown", None);
        };
        consume_oidc_invitation(&mut store, &invitation, user.id, &email);
    }

    let token = store.issue_session(user.id, state.session_ttl_days);
    drop(store);
    oidc_landing(&state, "signed_in", Some(&token))
}

#[derive(Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    pub slug: String,
    /// An existing member of the owning organisation who becomes the project's
    /// first owner (FR-034). The operator names the owner but never receives a
    /// credential from this call (FR-012).
    #[serde(default)]
    pub initial_owner_email: Option<String>,
}

fn next_dsn_id(store: &Store) -> String {
    let next = store
        .projects
        .values()
        .filter_map(|project| project.dsn_id.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    next.to_string()
}

pub async fn create_project(
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<CreateProjectRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let org_id = path.into_inner();
    // FR-034: only the deployment operator creates projects. Organisation
    // administrators keep every other project-level capability.
    let (_, email) = auth_operator(&req, &state).await?;
    let name = body.name.trim();
    let slug = body.slug.trim().to_ascii_lowercase();
    if name.is_empty() || slug.is_empty() {
        return Err(ApiError::BadRequest(
            "project name and slug are required".into(),
        ));
    }
    let owner_email = body
        .initial_owner_email
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if owner_email.is_empty() {
        return Err(ApiError::BadRequest(
            "initial_owner_email is required".into(),
        ));
    }
    if !slug
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(ApiError::BadRequest(
            "slug may only contain letters, digits, hyphens, and underscores".into(),
        ));
    }
    let mut store = state.store.write().await;
    if !store.orgs.contains_key(&org_id) {
        return Err(ApiError::NotFound("organisation not found".into()));
    }
    // FR-034: the initial owner must already belong to the organisation; nothing
    // is created when the name does not resolve to a member.
    let owner_id = match store
        .user_by_email(owner_email)
        .map(|user| user.id)
        .filter(|user_id| store.org_role_for(*user_id, org_id).is_some())
    {
        Some(user_id) => user_id,
        None => {
            return Err(ApiError::BadRequest(
                "initial_owner_email must name an existing member of this organisation".into(),
            ));
        }
    };
    if store.projects.values().any(|project| {
        project.org_id == org_id && project.slug == slug && project.deleted_at.is_none()
    }) {
        return Err(ApiError::Conflict(
            "a project with that slug already exists".into(),
        ));
    }
    let now = Utc::now();
    let dsn_id = next_dsn_id(&store);
    let project = Project {
        id: Uuid::new_v4(),
        dsn_id: dsn_id.clone(),
        slug: slug.clone(),
        name: name.to_string(),
        org_id,
        retention_days: state.retention_days,
        model: None,
        confidence_threshold: state.confidence_threshold,
        deleted_at: None,
        deleted_by: None,
        created_at: now,
    };
    store.projects.insert(project.id, project.clone());
    store.projects_by_dsn.insert(dsn_id.clone(), project.id);
    store.projects_by_dsn.insert(slug, project.id);
    store
        .memberships
        .insert((owner_id, project.id), Role::Owner);
    let (view, _) = public_project(project.clone());
    crate::audit::record(
        &mut store,
        Some(org_id),
        Some(project.id),
        &email,
        "project.created",
        Some(format!("{} (owner {owner_email})", project.name)),
    );
    // FR-012: the operator never receives an ingest credential. The named owner
    // signs in and creates the first key with `POST /api/projects/{id}/keys`.
    Ok(HttpResponse::Ok().json(json!({ "project": view })))
}

#[derive(Deserialize, Default)]
pub struct RotateKeyRequest {
    #[serde(default)]
    pub label: Option<String>,
}

pub async fn rotate_key(
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<RotateKeyRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    let (_, email, role) = auth_project(&req, &state, project_id).await?;
    if !role.can_manage() {
        return Err(ApiError::Forbidden(
            "admin role or above is required".into(),
        ));
    }
    let label = body
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .unwrap_or("Rotated")
        .to_string();
    let mut store = state.store.write().await;
    for key in store.keys.values_mut() {
        if key.project_id == project_id && !key.revoked {
            key.revoked = true;
        }
    }
    let plaintext = format!("scout_{}", crate::auth::new_token());
    let key_hash = crate::auth::hash_token(&plaintext);
    let key_id = Uuid::new_v4();
    store.keys.insert(
        key_id,
        IngestKey {
            id: key_id,
            project_id,
            label,
            key_hash,
            key_prefix: crate::auth::token_prefix(&plaintext, 12),
            created_at: Utc::now(),
            revoked: false,
        },
    );
    crate::audit::record(
        &mut store,
        None,
        Some(project_id),
        &email,
        "key.rotated",
        None,
    );
    Ok(HttpResponse::Ok().json(json!({ "key": plaintext, "ok": true })))
}

#[derive(Deserialize)]
pub struct RecoverRequest {
    pub email: String,
    pub recovery_code: String,
    pub new_password: String,
}

pub async fn recover(
    body: web::Json<RecoverRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let mut store = state.store.write().await;
    // The password policy is actionable, so it is reported as a 400 before the
    // code is touched; an unknown or expired code keeps the generic 401.
    crate::auth::validate_password(&body.new_password).map_err(ApiError::BadRequest)?;
    let user_id = crate::auth::redeem_recovery_code(
        &mut store,
        &body.email,
        &body.recovery_code,
        &body.new_password,
    )
    .map_err(|_| ApiError::Unauthorized("invalid email or recovery code".into()))?;
    // A password reset invalidates existing sessions.
    store
        .sessions
        .retain(|_, session| session.user_id != user_id);
    let actor = store
        .users
        .get(&user_id)
        .map(|user| user.email.clone())
        .unwrap_or_default();
    crate::audit::record(
        &mut store,
        None,
        None,
        &actor,
        "account.password_reset",
        None,
    );
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Lets an authenticated account change its own password (FR-012, SC-017). The
/// current password is required, the 12-character minimum is reported before the
/// stored credential is touched, and every other session for the account ends.
pub async fn change_password(
    req: HttpRequest,
    body: web::Json<ChangePasswordRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (user_id, email) = auth_user(&req, &state).await?;
    // The policy is actionable, so it is reported before anything is verified or
    // written; a rejected value leaves the old password usable.
    crate::auth::validate_password(&body.new_password).map_err(ApiError::BadRequest)?;
    let mut store = state.store.write().await;
    let Some(hash) = store
        .users
        .get(&user_id)
        .and_then(|user| user.password_hash.clone())
    else {
        return Err(ApiError::BadRequest(
            "this account has no local password".into(),
        ));
    };
    if !crate::auth::verify_password(&body.current_password, &hash) {
        return Err(ApiError::BadRequest(
            "the current password is incorrect".into(),
        ));
    }
    let new_hash = crate::auth::hash_password(&body.new_password).map_err(|error| {
        ApiError::ServiceUnavailable(format!("could not store the password: {error}"))
    })?;
    if let Some(user) = store.users.get_mut(&user_id) {
        user.password_hash = Some(new_hash);
    }
    // A changed credential ends every other session. The caller keeps the session
    // that made the change, so the dashboard does not force a re-sign-in.
    let keep = bearer(&req).map(|token| crate::auth::hash_token(&token));
    store.revoke_user_sessions_except(user_id, keep.as_deref());
    crate::audit::record(
        &mut store,
        None,
        None,
        &email,
        "account.password_changed",
        None,
    );
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

pub async fn reissue_recovery_codes(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (user_id, email) = auth_user(&req, &state).await?;
    let mut store = state.store.write().await;
    let codes = crate::auth::issue_recovery_codes(&mut store, user_id, 10);
    crate::audit::record(
        &mut store,
        None,
        None,
        &email,
        "recovery_codes.reissued",
        None,
    );
    Ok(HttpResponse::Ok().json(json!({ "recovery_codes": codes })))
}

// --- issues ----------------------------------------------------------------

#[derive(Deserialize)]
pub struct IssueQuery {
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    /// Matches the human decision when one exists; suggestions do not count.
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
}

pub async fn list_issues(
    req: HttpRequest,
    path: web::Path<Uuid>,
    query: web::Query<IssueQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    auth_project(&req, &state, project_id).await?;
    let store = state.store.read().await;
    let needle = query.q.as_deref().unwrap_or("").trim().to_ascii_lowercase();
    let state_filter = query.state.as_deref().filter(|s| !s.is_empty());
    let level_filter = query.level.as_deref().filter(|s| !s.is_empty());
    let category_filter = query.category.as_deref().filter(|s| !s.is_empty());

    let mut issues: Vec<Issue> = store
        .issues
        .values()
        .filter(|i| i.project_id == project_id)
        .filter(|i| state_filter.is_none_or(|s| issue_state_str(i.state) == s))
        .filter(|i| level_filter.is_none_or(|l| i.level.eq_ignore_ascii_case(l)))
        .filter(|i| {
            category_filter.is_none_or(|category| {
                i.decision
                    .as_ref()
                    .is_some_and(|decision| decision.category == category)
            })
        })
        .filter(|i| {
            needle.is_empty()
                || i.title.to_ascii_lowercase().contains(&needle)
                || i.culprit
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .contains(&needle)
                || i.fingerprint.contains(&needle)
        })
        .cloned()
        .collect();

    match query.sort.as_deref().unwrap_or("last_seen") {
        "count" => issues.sort_by_key(|i| std::cmp::Reverse(i.count)),
        "first_seen" => issues.sort_by_key(|i| std::cmp::Reverse(i.first_seen)),
        _ => issues.sort_by_key(|i| std::cmp::Reverse(i.last_seen)),
    }

    Ok(HttpResponse::Ok().json(json!({ "issues": issues })))
}

pub async fn get_issue(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let issue_id = path.into_inner();
    let (user_id, _) = auth_user(&req, &state).await?;
    let mut store = state.store.write().await;
    let mut issue = store
        .issues
        .get(&issue_id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;
    deny_project_access(&store, user_id, issue.project_id)?;
    store
        .role_for(user_id, issue.project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
    if !project_is_active(&store, issue.project_id) {
        return Err(ApiError::NotFound("project not found".into()));
    }
    // FR-019: the first presentation freezes late classifier suggestions. The
    // timestamp is written once and never advanced by later reads.
    if issue.first_presented_at.is_none() {
        let now = Utc::now();
        issue.first_presented_at = Some(now);
        if let Some(stored) = store.issues.get_mut(&issue_id) {
            stored.first_presented_at = Some(now);
        }
    }
    let event = store.events.get(&issue.last_event_id);
    let meta = event.map(EventMeta::from);
    let event_json = event.map(|e| e.event.clone());
    let raw_available =
        event.is_some_and(|e| e.raw_object_key.is_some() && e.raw_expired_at.is_none());
    let attachments: Vec<Value> = event
        .map(|e| {
            e.attachments
                .iter()
                .map(|a| {
                    json!({
                        "filename": a.filename,
                        "content_type": a.content_type,
                        "size": a.size,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(HttpResponse::Ok().json(json!({
        "issue": issue,
        "event": meta,
        "event_json": event_json,
        "attachments": attachments,
        "raw_available": raw_available,
    })))
}

pub async fn list_occurrences(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let issue_id = path.into_inner();
    let (user_id, _) = auth_user(&req, &state).await?;
    let store = state.store.read().await;
    let issue = store
        .issues
        .get(&issue_id)
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;
    deny_project_access(&store, user_id, issue.project_id)?;
    store
        .role_for(user_id, issue.project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
    if !project_is_active(&store, issue.project_id) {
        return Err(ApiError::NotFound("project not found".into()));
    }

    let mut occurrences: Vec<Value> = store
        .events
        .values()
        .filter(|event| event.issue_id == Some(issue_id))
        .map(|event| {
            json!({
                "id": event.id,
                "sentry_event_id": event.sentry_event_id,
                "received_at": event.received_at,
                "size": event.raw_size,
                "attachment_count": event.attachment_count,
                "raw_available": event.raw_object_key.is_some()
                    && event.raw_expired_at.is_none(),
            })
        })
        .collect();
    occurrences.sort_by(|a, b| b["received_at"].as_str().cmp(&a["received_at"].as_str()));
    Ok(HttpResponse::Ok().json(json!({ "occurrences": occurrences })))
}

pub async fn get_raw(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let issue_id = path.into_inner();
    let (user_id, _) = auth_user(&req, &state).await?;
    let store = state.store.read().await;
    let issue = store
        .issues
        .get(&issue_id)
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;
    deny_project_access(&store, user_id, issue.project_id)?;
    store
        .role_for(user_id, issue.project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
    if !project_is_active(&store, issue.project_id) {
        return Err(ApiError::NotFound("project not found".into()));
    }
    let event = store
        .events
        .get(&issue.last_event_id)
        .ok_or_else(|| ApiError::NotFound("raw payload has expired under retention".into()))?;
    let sentry_event_id = event.sentry_event_id.clone();
    let key = event
        .raw_object_key
        .clone()
        .filter(|_| event.raw_expired_at.is_none())
        .ok_or_else(|| ApiError::NotFound("raw payload has expired under retention".into()))?;
    drop(store);
    let bytes = state.objects.get(&key).await?;
    Ok(HttpResponse::Ok()
        .content_type("application/octet-stream")
        .insert_header(("X-Scout-Event-Id", sentry_event_id))
        .body(bytes))
}

#[derive(Deserialize)]
pub struct StateRequest {
    pub state: IssueState,
}

pub async fn set_issue_state(
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<StateRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let issue_id = path.into_inner();
    let (user_id, email) = auth_user(&req, &state).await?;
    let mut store = state.store.write().await;
    let project_id = store
        .issues
        .get(&issue_id)
        .map(|i| i.project_id)
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;
    deny_project_access(&store, user_id, project_id)?;
    let role = store
        .role_for(user_id, project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
    if !role.can_triage() {
        return Err(ApiError::Forbidden(
            "triager role or above is required".into(),
        ));
    }
    if !project_is_active(&store, project_id) {
        return Err(ApiError::NotFound("project not found".into()));
    }
    let issue = store.issues.get_mut(&issue_id).expect("checked above");
    issue.state = body.state;
    issue.state_history.push(StateChange {
        state: body.state,
        by: email,
        at: Utc::now(),
    });
    Ok(HttpResponse::Ok().json(json!({ "issue": issue })))
}

#[derive(Deserialize)]
pub struct CategoriseRequest {
    pub action: String,
    #[serde(default)]
    pub category: Option<String>,
}

pub async fn categorise(
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<CategoriseRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let issue_id = path.into_inner();
    let (user_id, email) = auth_user(&req, &state).await?;
    let mut store = state.store.write().await;
    let project_id = store
        .issues
        .get(&issue_id)
        .map(|i| i.project_id)
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;
    deny_project_access(&store, user_id, project_id)?;
    let role = store
        .role_for(user_id, project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
    if !role.can_triage() {
        return Err(ApiError::Forbidden(
            "triager role or above is required".into(),
        ));
    }
    if !project_is_active(&store, project_id) {
        return Err(ApiError::NotFound("project not found".into()));
    }

    let issue = store.issues.get_mut(&issue_id).expect("checked above");
    let category = match body.action.as_str() {
        "confirm" => issue
            .suggestion
            .as_ref()
            .map(|s| s.category.clone())
            .ok_or_else(|| ApiError::BadRequest("there is no suggestion to confirm".into()))?,
        "override" => {
            let category = body
                .category
                .clone()
                .ok_or_else(|| ApiError::BadRequest("category is required to override".into()))?;
            validate_category(&category)?;
            category
        }
        _ => {
            return Err(ApiError::BadRequest(
                "action must be 'confirm' or 'override'".into(),
            ))
        }
    };
    issue.decision = Some(CategorisationDecision {
        category,
        by: email,
        at: Utc::now(),
        overridden: body.action == "override",
    });
    Ok(HttpResponse::Ok().json(json!({ "issue": issue })))
}

fn owner_count(store: &crate::store::Store, project_id: Uuid) -> usize {
    store
        .memberships
        .iter()
        .filter(|((_, pid), role)| *pid == project_id && role.is_owner())
        .count()
}

fn validate_category(category: &str) -> Result<(), ApiError> {
    match category {
        "issue" | "log" | "raw_event" => Ok(()),
        _ => Err(ApiError::BadRequest(
            "category must be issue, log, or raw_event".into(),
        )),
    }
}

// --- logs and search -------------------------------------------------------

#[derive(Deserialize)]
pub struct LogQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

pub async fn list_logs(
    req: HttpRequest,
    path: web::Path<Uuid>,
    query: web::Query<LogQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    auth_project(&req, &state, project_id).await?;
    let store = state.store.read().await;
    let needle = query.q.as_deref().unwrap_or("").trim().to_ascii_lowercase();
    let level_filter = query.level.as_deref().filter(|s| !s.is_empty());
    let limit = query.limit.unwrap_or(200).min(1000);

    let mut logs: Vec<LogLine> = store
        .logs
        .iter()
        .filter(|l| l.project_id == project_id)
        .filter(|l| level_filter.is_none_or(|level| l.level.eq_ignore_ascii_case(level)))
        .filter(|l| {
            needle.is_empty()
                || l.message.to_ascii_lowercase().contains(&needle)
                || l.source.to_ascii_lowercase().contains(&needle)
                || l.level.to_ascii_lowercase().contains(&needle)
                || l.attributes
                    .to_string()
                    .to_ascii_lowercase()
                    .contains(&needle)
        })
        .cloned()
        .collect();
    logs.sort_by_key(|l| std::cmp::Reverse(l.timestamp));
    logs.truncate(limit);
    Ok(HttpResponse::Ok().json(json!({ "logs": logs })))
}

pub async fn search(
    req: HttpRequest,
    path: web::Path<Uuid>,
    query: web::Query<LogQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    /// Combined cap across issues and logs (FR-021, SC-025).
    const SEARCH_RESULT_CAP: usize = 200;
    let project_id = path.into_inner();
    auth_project(&req, &state, project_id).await?;
    let store = state.store.read().await;
    let needle = query.q.as_deref().unwrap_or("").trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(HttpResponse::Ok().json(json!({ "issues": [], "logs": [], "has_more": false })));
    }

    let mut issues: Vec<Issue> = store
        .issues
        .values()
        .filter(|i| i.project_id == project_id)
        .filter(|i| {
            i.title.to_ascii_lowercase().contains(&needle)
                || i.culprit
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .contains(&needle)
                || i.release
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .contains(&needle)
                // FR-012 includes the grouping identity in issue search.
                || i.fingerprint.to_ascii_lowercase().contains(&needle)
        })
        .cloned()
        .collect();
    issues.sort_by_key(|i| std::cmp::Reverse(i.last_seen));

    let mut logs: Vec<LogLine> = store
        .logs
        .iter()
        .filter(|l| l.project_id == project_id)
        .filter(|l| {
            l.message.to_ascii_lowercase().contains(&needle)
                || l.source.to_ascii_lowercase().contains(&needle)
                // FR-012 includes level and attributes.
                || l.level.to_ascii_lowercase().contains(&needle)
                || l.attributes
                    .to_string()
                    .to_ascii_lowercase()
                    .contains(&needle)
        })
        .cloned()
        .collect();
    logs.sort_by_key(|l| std::cmp::Reverse(l.timestamp));

    // Take the newest 200 matches across both kinds, then report whether
    // anything was left out (FR-021, SC-025).
    let mut selected_issues = Vec::new();
    let mut selected_logs = Vec::new();
    let (mut issue_index, mut log_index) = (0usize, 0usize);
    while selected_issues.len() + selected_logs.len() < SEARCH_RESULT_CAP {
        let take_issue = match (issues.get(issue_index), logs.get(log_index)) {
            (Some(issue), Some(log)) => issue.last_seen >= log.timestamp,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_issue {
            selected_issues.push(issues[issue_index].clone());
            issue_index += 1;
        } else {
            selected_logs.push(logs[log_index].clone());
            log_index += 1;
        }
    }
    let has_more = issue_index < issues.len() || log_index < logs.len();

    Ok(HttpResponse::Ok().json(json!({
        "issues": selected_issues,
        "logs": selected_logs,
        "has_more": has_more,
    })))
}

// --- members ---------------------------------------------------------------

pub async fn list_members(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    auth_project(&req, &state, project_id).await?;
    let store = state.store.read().await;
    let mut members: Vec<Value> = store
        .memberships
        .iter()
        .filter(|((_, pid), _)| *pid == project_id)
        .filter_map(|((user_id, _), role)| {
            let user = store.users.get(user_id)?;
            Some(json!({
                "user_id": user_id,
                "name": user.name,
                "email": user.email,
                "role": role,
            }))
        })
        .collect();
    members.sort_by(|a, b| {
        b["role"]
            .as_str()
            .cmp(&a["role"].as_str())
            .then(a["name"].as_str().cmp(&b["name"].as_str()))
    });
    Ok(HttpResponse::Ok().json(json!({ "members": members })))
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    pub email: String,
    pub role: Role,
}

#[derive(Deserialize)]
pub struct UpdateMemberRequest {
    pub role: Role,
}

pub async fn add_member(
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<AddMemberRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    let (_, email, actor_role) = auth_project(&req, &state, project_id).await?;
    if !actor_role.can_manage() {
        return Err(ApiError::Forbidden(
            "admin role or above is required".into(),
        ));
    }
    if body.role.is_owner() && !actor_role.is_owner() {
        return Err(ApiError::Forbidden(
            "only an owner can grant the owner role".into(),
        ));
    }
    let mut store = state.store.write().await;
    let user = store
        .user_by_email(&body.email)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("no account with that email".into()))?;
    store.memberships.insert((user.id, project_id), body.role);
    crate::audit::record(
        &mut store,
        None,
        Some(project_id),
        &email,
        "member.added",
        Some(user.email.clone()),
    );
    Ok(HttpResponse::Ok().json(json!({
        "user_id": user.id,
        "name": user.name,
        "email": user.email,
        "role": body.role,
    })))
}

pub async fn update_member(
    req: HttpRequest,
    path: web::Path<(Uuid, Uuid)>,
    body: web::Json<UpdateMemberRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (project_id, user_id) = path.into_inner();
    let (_, email, actor_role) = auth_project(&req, &state, project_id).await?;
    if !actor_role.can_manage() {
        return Err(ApiError::Forbidden(
            "admin role or above is required".into(),
        ));
    }
    let mut store = state.store.write().await;
    let current = store
        .role_for(user_id, project_id)
        .ok_or_else(|| ApiError::NotFound("member not found".into()))?;
    if (current.is_owner() || body.role.is_owner()) && !actor_role.is_owner() {
        return Err(ApiError::Forbidden(
            "only an owner can change owner roles".into(),
        ));
    }
    if current.is_owner() && !body.role.is_owner() && owner_count(&store, project_id) <= 1 {
        return Err(ApiError::BadRequest(
            "cannot demote the last owner of a project".into(),
        ));
    }
    store.memberships.insert((user_id, project_id), body.role);
    crate::audit::record_with(
        &mut store,
        None,
        Some(project_id),
        &email,
        "member.role_changed",
        Some(user_id.to_string()),
        json!({ "previous_role": current.to_string(), "role": body.role.to_string() }),
    );
    Ok(HttpResponse::Ok().json(json!({ "ok": true, "role": body.role })))
}

pub async fn remove_member(
    req: HttpRequest,
    path: web::Path<(Uuid, Uuid)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (project_id, user_id) = path.into_inner();
    let (_, email, actor_role) = auth_project(&req, &state, project_id).await?;
    if !actor_role.can_manage() {
        return Err(ApiError::Forbidden(
            "admin role or above is required".into(),
        ));
    }
    let mut store = state.store.write().await;
    let current = store
        .role_for(user_id, project_id)
        .ok_or_else(|| ApiError::NotFound("member not found".into()))?;
    if current.is_owner() && !actor_role.is_owner() {
        return Err(ApiError::Forbidden(
            "only an owner can remove an owner".into(),
        ));
    }
    if current.is_owner() && owner_count(&store, project_id) <= 1 {
        return Err(ApiError::BadRequest(
            "cannot remove the last owner of a project".into(),
        ));
    }
    store.memberships.remove(&(user_id, project_id));
    crate::audit::record(
        &mut store,
        None,
        Some(project_id),
        &email,
        "member.removed",
        Some(user_id.to_string()),
    );
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

// --- ingest keys -----------------------------------------------------------

pub async fn list_keys(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    let (_, _, role) = auth_project(&req, &state, project_id).await?;
    if !role.can_manage() {
        return Err(ApiError::Forbidden(
            "admin role or above is required".into(),
        ));
    }
    let store = state.store.read().await;
    let mut keys: Vec<IngestKey> = store
        .keys
        .values()
        .filter(|k| k.project_id == project_id)
        .cloned()
        .collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.created_at));
    Ok(HttpResponse::Ok().json(json!({ "keys": keys })))
}

#[derive(Deserialize)]
pub struct KeyRequest {
    #[serde(default)]
    pub label: Option<String>,
}

pub async fn create_key(
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<KeyRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    let (_, email, role) = auth_project(&req, &state, project_id).await?;
    if !role.can_manage() {
        return Err(ApiError::Forbidden(
            "admin role or above is required".into(),
        ));
    }
    let plaintext = format!("scout_{}", Uuid::new_v4().simple());
    let key_hash = crate::auth::hash_token(&plaintext);
    let key_prefix = crate::auth::token_prefix(&plaintext, 12);
    let key = IngestKey {
        id: Uuid::new_v4(),
        project_id,
        label: body
            .label
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "New key".into()),
        key_hash: key_hash.clone(),
        key_prefix: key_prefix.clone(),
        created_at: Utc::now(),
        revoked: false,
    };
    let mut store = state.store.write().await;
    store.keys.insert(key.id, key.clone());
    crate::audit::record(
        &mut store,
        None,
        Some(project_id),
        &email,
        "key.created",
        Some(key.label.clone()),
    );
    // The plaintext is returned exactly once; every other response exposes only
    // the prefix and never the hash.
    Ok(HttpResponse::Ok().json(json!({
        "key": plaintext,
        "id": key.id,
        "label": key.label,
        "key_prefix": key_prefix,
        "created_at": key.created_at,
        "revoked": false,
    })))
}

pub async fn revoke_key(
    req: HttpRequest,
    path: web::Path<(Uuid, Uuid)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (project_id, key_id) = path.into_inner();
    let (_, email, role) = auth_project(&req, &state, project_id).await?;
    if !role.can_manage() {
        return Err(ApiError::Forbidden(
            "admin role or above is required".into(),
        ));
    }
    let mut store = state.store.write().await;
    let key = store
        .keys
        .values_mut()
        .find(|k| k.id == key_id && k.project_id == project_id)
        .ok_or_else(|| ApiError::NotFound("key not found".into()))?;
    let label = key.label.clone();
    key.revoked = true;
    crate::audit::record(
        &mut store,
        None,
        Some(project_id),
        &email,
        "key.revoked",
        Some(label),
    );
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

// --- settings --------------------------------------------------------------

/// Project JSON safe to return to a client: secrets never leave the process.
fn public_project(mut project: Project) -> (Value, bool) {
    let api_key_configured = project
        .model
        .as_ref()
        .and_then(|model| model.api_key.as_ref())
        .is_some();
    if let Some(model) = project.model.as_mut() {
        model.api_key = None;
    }
    (
        serde_json::to_value(project).unwrap_or(Value::Null),
        api_key_configured,
    )
}

pub async fn get_settings(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    let (_, _, role) = auth_project(&req, &state, project_id).await?;
    if !role.can_manage() {
        return Err(ApiError::Forbidden(
            "admin role or above is required".into(),
        ));
    }
    let store = state.store.read().await;
    let project = store
        .projects
        .get(&project_id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("project not found".into()))?;
    let key_prefix = store
        .active_key_for(project_id)
        .map(|k| k.key_prefix.clone());
    let mut keys: Vec<IngestKey> = store
        .keys
        .values()
        .filter(|k| k.project_id == project_id)
        .cloned()
        .collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.created_at));
    let dsn_id = project.dsn_id.clone();
    let (project, api_key_configured) = public_project(project);
    Ok(HttpResponse::Ok().json(json!({
        "project": project,
        "role": role,
        "dsn_id": dsn_id,
        "key_prefix": key_prefix,
        "keys": keys,
        "model_api_key_configured": api_key_configured,
        "mock_model_endpoint": format!("{}/mock-jev", state.self_url.trim_end_matches('/')),
    })))
}

#[derive(Deserialize)]
pub struct SettingsRequest {
    pub retention_days: u32,
    #[serde(default)]
    pub model: Option<ModelConfig>,
}

pub async fn update_settings(
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<SettingsRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let project_id = path.into_inner();
    let (_, email, role) = auth_project(&req, &state, project_id).await?;
    if !role.can_manage() {
        return Err(ApiError::Forbidden(
            "admin role or above is required".into(),
        ));
    }
    if !(1..=3650).contains(&body.retention_days) {
        return Err(ApiError::BadRequest(
            "retention_days must be between 1 and 3650".into(),
        ));
    }
    let model = match &body.model {
        Some(model) => {
            if !(model.endpoint.starts_with("http://") || model.endpoint.starts_with("https://")) {
                return Err(ApiError::BadRequest(
                    "model endpoint must start with http:// or https://".into(),
                ));
            }
            if model.model.trim().is_empty() {
                return Err(ApiError::BadRequest("model id cannot be empty".into()));
            }
            if !(0.0..=1.0).contains(&model.confidence_threshold) {
                return Err(ApiError::BadRequest(
                    "confidence_threshold must be between 0 and 1".into(),
                ));
            }
            Some(ModelConfig {
                endpoint: model.endpoint.trim().trim_end_matches('/').to_string(),
                api_key: model.api_key.clone().filter(|k| !k.trim().is_empty()),
                model: model.model.trim().to_string(),
                confidence_threshold: model.confidence_threshold,
            })
        }
        None => None,
    };
    let mut store = state.store.write().await;
    let project = store
        .projects
        .get_mut(&project_id)
        .ok_or_else(|| ApiError::NotFound("project not found".into()))?;
    project.retention_days = body.retention_days;
    if let Some(model) = &model {
        project.confidence_threshold = model.confidence_threshold;
    }
    project.model = model;
    crate::audit::record(
        &mut store,
        None,
        Some(project_id),
        &email,
        "settings.updated",
        None,
    );
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}
