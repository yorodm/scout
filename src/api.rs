use crate::error::ApiError;
use crate::models::*;
use crate::state::AppState;
use actix_web::{web, HttpRequest, HttpResponse};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.route("/api/health", web::get().to(health))
        .route("/api/demo/accounts", web::get().to(demo_accounts))
        .route("/api/demo/reset", web::post().to(reset_demo))
        .route("/api/login", web::post().to(login))
        .route("/api/logout", web::post().to(logout))
        .route("/api/me", web::get().to(me))
        .route("/api/projects", web::get().to(list_projects))
        .route("/api/projects/{project_id}", web::get().to(get_project))
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
        .route("/api/issues/{issue_id}/raw", web::get().to(get_raw))
        .route(
            "/api/issues/{issue_id}/state",
            web::post().to(set_issue_state),
        )
        .route(
            "/api/issues/{issue_id}/categorisation",
            web::post().to(categorise),
        );
}

// --- authentication helpers ------------------------------------------------

fn bearer(req: &HttpRequest) -> Option<String> {
    req.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

async fn auth_user(req: &HttpRequest, state: &AppState) -> Result<(Uuid, String), ApiError> {
    let token =
        bearer(req).ok_or_else(|| ApiError::Unauthorized("missing session token".into()))?;
    let store = state.store.read().await;
    let user_id = store
        .user_for_token(&token)
        .ok_or_else(|| ApiError::Unauthorized("invalid or expired session".into()))?;
    let email = store
        .users
        .get(&user_id)
        .map(|u| u.email.clone())
        .unwrap_or_default();
    Ok((user_id, email))
}

async fn auth_project(
    req: &HttpRequest,
    state: &AppState,
    project_id: Uuid,
) -> Result<(Uuid, String, Role), ApiError> {
    let (user_id, email) = auth_user(req, state).await?;
    let store = state.store.read().await;
    let role = store
        .role_for(user_id, project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
    Ok((user_id, email, role))
}

fn issue_state_str(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "open",
        IssueState::Resolved => "resolved",
        IssueState::Ignored => "ignored",
    }
}

fn memberships_json(store: &crate::store::Store, user_id: Uuid) -> Vec<Value> {
    store
        .memberships
        .iter()
        .filter(|((uid, _), _)| *uid == user_id)
        .filter_map(|((_, project_id), role)| {
            let project = store.projects.get(project_id)?;
            Some(json!({
                "project_id": project_id,
                "project_name": project.name,
                "slug": project.slug,
                "role": role,
            }))
        })
        .collect()
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
    let user = store
        .user_by_email(&body.email)
        .cloned()
        .ok_or_else(invalid_credentials)?;
    let Some(hash) = user.password_hash.as_deref() else {
        // An account that only authenticates through OIDC has no local password.
        return Err(invalid_credentials());
    };
    if !crate::auth::verify_password(&body.password, hash) {
        return Err(invalid_credentials());
    }
    let token = store.issue_session(user.id, state.session_ttl_days);
    let memberships = memberships_json(&store, user.id);
    Ok(HttpResponse::Ok().json(json!({
        "token": token,
        "user": user,
        "memberships": memberships,
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
    // A demo reset must not delete the deployment operator: it cannot be
    // recreated without a restart and its password must never be reset here.
    let operator = {
        let store = state.store.read().await;
        store.users.values().find(|user| user.is_operator).cloned()
    };
    let (mut fresh, raws) = crate::store::seed(&state.self_url);
    if let Some(operator) = operator {
        fresh
            .users_by_email
            .insert(operator.email.to_ascii_lowercase(), operator.id);
        fresh.users.insert(operator.id, operator);
    }
    for (key, data) in raws {
        if let Err(error) = state.objects.put_create(&key, data).await {
            return HttpResponse::InternalServerError()
                .json(json!({ "error": format!("could not reseed raw payload: {error}") }));
        }
    }
    *state.store.write().await = fresh;
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

// --- projects --------------------------------------------------------------

pub async fn list_projects(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let (user_id, _) = auth_user(&req, &state).await?;
    let store = state.store.read().await;
    let mut projects = Vec::new();
    for ((member, project_id), role) in &store.memberships {
        if *member != user_id {
            continue;
        }
        let Some(project) = store.projects.get(project_id) else {
            continue;
        };
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
        let key = if role.can_manage() {
            store.active_key_for(*project_id).map(|k| k.key.clone())
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
            "key": key,
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
    let key = if role.can_manage() {
        store.active_key_for(project_id).map(|k| k.key.clone())
    } else {
        None
    };
    Ok(HttpResponse::Ok().json(json!({
        "project": project,
        "role": role,
        "key": key,
    })))
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

    let mut issues: Vec<Issue> = store
        .issues
        .values()
        .filter(|i| i.project_id == project_id)
        .filter(|i| state_filter.is_none_or(|s| issue_state_str(i.state) == s))
        .filter(|i| level_filter.is_none_or(|l| i.level.eq_ignore_ascii_case(l)))
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
    let store = state.store.read().await;
    let issue = store
        .issues
        .get(&issue_id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("issue not found".into()))?;
    store
        .role_for(user_id, issue.project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
    let event = store.events.get(&issue.last_event_id);
    let meta = event.map(EventMeta::from);
    let event_json = event.map(|e| e.event.clone());
    let attachments: Vec<Value> = event
        .map(|e| {
            e.attachments
                .iter()
                .map(|a| {
                    json!({
                        "filename": a.filename,
                        "content_type": a.content_type,
                        "size": a.data.len(),
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
        "raw_available": event.is_some(),
    })))
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
    store
        .role_for(user_id, issue.project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
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
    let role = store
        .role_for(user_id, project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
    if !role.can_triage() {
        return Err(ApiError::Forbidden(
            "triager role or above is required".into(),
        ));
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
    let role = store
        .role_for(user_id, project_id)
        .ok_or_else(|| ApiError::Forbidden("no access to this project".into()))?;
    if !role.can_triage() {
        return Err(ApiError::Forbidden(
            "triager role or above is required".into(),
        ));
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
    let project_id = path.into_inner();
    auth_project(&req, &state, project_id).await?;
    let store = state.store.read().await;
    let needle = query.q.as_deref().unwrap_or("").trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(HttpResponse::Ok().json(json!({ "issues": [], "logs": [] })));
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
        })
        .cloned()
        .collect();
    issues.sort_by_key(|i| std::cmp::Reverse(i.last_seen));
    issues.truncate(50);

    let mut logs: Vec<LogLine> = store
        .logs
        .iter()
        .filter(|l| l.project_id == project_id)
        .filter(|l| {
            l.message.to_ascii_lowercase().contains(&needle)
                || l.source.to_ascii_lowercase().contains(&needle)
                || l.attributes
                    .to_string()
                    .to_ascii_lowercase()
                    .contains(&needle)
        })
        .cloned()
        .collect();
    logs.sort_by_key(|l| std::cmp::Reverse(l.timestamp));
    logs.truncate(50);

    Ok(HttpResponse::Ok().json(json!({ "issues": issues, "logs": logs })))
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
    crate::audit::record(
        &mut store,
        None,
        Some(project_id),
        &email,
        "member.role_changed",
        Some(user_id.to_string()),
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
    let key = IngestKey {
        id: Uuid::new_v4(),
        project_id,
        label: body
            .label
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "New key".into()),
        key: format!("scout_{}", Uuid::new_v4().simple()),
        created_at: Utc::now(),
        revoked: false,
    };
    let mut store = state.store.write().await;
    store.keys.insert(key.key.clone(), key.clone());
    crate::audit::record(
        &mut store,
        None,
        Some(project_id),
        &email,
        "key.created",
        Some(key.label.clone()),
    );
    Ok(HttpResponse::Ok().json(json!({ "key": key })))
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
    let host = req.connection_info().host().to_string();
    let store = state.store.read().await;
    let project = store
        .projects
        .get(&project_id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("project not found".into()))?;
    let key = store.active_key_for(project_id).map(|k| k.key.clone());
    let dsn = key.map(|k| format!("http://{k}@{host}/{}", project.dsn_id));
    let mut keys: Vec<IngestKey> = store
        .keys
        .values()
        .filter(|k| k.project_id == project_id)
        .cloned()
        .collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.created_at));
    Ok(HttpResponse::Ok().json(json!({
        "project": project,
        "role": role,
        "dsn": dsn,
        "keys": keys,
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
            Some(ModelConfig {
                endpoint: model.endpoint.trim().trim_end_matches('/').to_string(),
                api_key: model.api_key.clone().filter(|k| !k.trim().is_empty()),
                model: model.model.trim().to_string(),
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
