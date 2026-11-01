//! Handlers for the 11 documented `/api/0/` paths.
//!
//! Every handler starts with the same sequence: disabled gate, bearer
//! authentication, resource resolution (`404` without leaking existence), then
//! the scope check (`403`). Responses never fabricate data.

use crate::compat::auth::{
    authenticate, authorize_org, authorize_project, resolve_org, scopes_for, CompatAuth, Scope,
};
use crate::compat::cursor::{self, Cursor};
use crate::compat::error::CompatError;
use crate::compat::projection;
use crate::compat::query::{self, Sort};
use crate::models::{Issue, Organisation, Project, StateChange};
use crate::state::AppState;
use actix_web::{web, HttpRequest, HttpResponse};
use chrono::Utc;
use serde_json::{json, Value};
use std::cmp::min;
use uuid::Uuid;

type Params = Vec<(String, String)>;

pub(crate) fn ensure_enabled(state: &AppState) -> Result<(), CompatError> {
    if state.compat_enabled {
        Ok(())
    } else {
        Err(CompatError::NotFound("Not Found".into()))
    }
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn all<'a>(params: &'a [(String, String)], key: &str) -> Vec<&'a str> {
    params
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .collect()
}

fn parse_limit(params: &[(String, String)]) -> usize {
    match param(params, "limit").and_then(|raw| raw.trim().parse::<i64>().ok()) {
        Some(value) if value > 0 => value.min(100) as usize,
        // Absent, non-numeric, zero, and negative values fall back to the default.
        _ => 100,
    }
}

fn values_page(
    values: &[String],
    params: &[(String, String)],
    fingerprint: &str,
) -> Result<(Vec<Value>, Option<Cursor>), CompatError> {
    let limit = parse_limit(params);
    let start = match param(params, "cursor") {
        Some(raw) => {
            let cursor = cursor::decode(raw)?;
            if cursor.q != fingerprint {
                return Err(CompatError::BadRequest(
                    "cursor does not match this query".into(),
                ));
            }
            values
                .iter()
                .position(|value| value > &cursor.k)
                .unwrap_or(values.len())
        }
        None => 0,
    };
    let end = min(start + limit, values.len());
    let page: Vec<Value> = values[start..end]
        .iter()
        .map(|value| json!({ "name": value }))
        .collect();
    let next = if end < values.len() {
        let last = values[end - 1].clone();
        Some(Cursor {
            v: 1,
            q: fingerprint.to_string(),
            s: "value".into(),
            k: last,
            i: String::new(),
        })
    } else {
        None
    };
    Ok((page, next))
}

fn link_header(
    self_url: &str,
    path: &str,
    params: &[(String, String)],
    next: Option<&Cursor>,
) -> String {
    let mut pairs: Vec<String> = params
        .iter()
        .filter(|(name, _)| name != "cursor")
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    let base = format!("{}{}", self_url.trim_end_matches('/'), path);
    match next {
        Some(cursor) => {
            let encoded = cursor::encode(cursor);
            pairs.push(format!("cursor={encoded}"));
            format!(
                "<{base}?{}>; rel=\"next\"; results=\"true\"; cursor=\"{encoded}\"",
                pairs.join("&")
            )
        }
        None => {
            let suffix = if pairs.is_empty() {
                String::new()
            } else {
                format!("?{}", pairs.join("&"))
            };
            format!("<{base}{suffix}>; rel=\"next\"; results=\"false\"")
        }
    }
}

fn page_response<B: serde::Serialize>(
    body: B,
    hits: usize,
    path: &str,
    params: &[(String, String)],
    next: Option<&Cursor>,
    state: &AppState,
) -> HttpResponse {
    let mut response = HttpResponse::Ok();
    response.insert_header(("X-Hits", hits.to_string()));
    response.insert_header(("Link", link_header(&state.self_url, path, params, next)));
    response.json(body)
}

// --- issues ----------------------------------------------------------------

pub async fn list_issues(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    params: web::Query<Params>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, project_ref) = path.into_inner();
    let auth = authenticate(&req, &state).await?;
    let (org, project, _) =
        authorize_project(&state, &auth, &org_ref, &project_ref, Scope::EventRead).await?;

    let params = params.into_inner();
    let query_text = param(&params, "query").unwrap_or_default();
    let sort = Sort::parse(param(&params, "sort"));
    let ids: Vec<String> = all(&params, "id").into_iter().map(str::to_string).collect();
    let hashes: Vec<String> = all(&params, "hashes")
        .into_iter()
        .map(str::to_string)
        .collect();
    let filter = query::parse_filter(Some(query_text), &ids, &hashes);
    let limit = parse_limit(&params);
    // Supplied ids that are not valid identifiers match nothing; they must never
    // fall back to an unrestricted query (FR-009).
    let ids_match_nothing = !ids.is_empty() && filter.ids.is_empty();
    let fingerprint = cursor::fingerprint(&[
        query_text,
        sort.as_str(),
        &ids.join(","),
        &hashes.join(","),
        &org.id.to_string(),
        &project.id.to_string(),
    ]);

    let mut issues: Vec<Issue> = if ids_match_nothing {
        Vec::new()
    } else {
        let store = state.store.read().await;
        store
            .issues
            .values()
            .filter(|issue| issue.project_id == project.id)
            .filter(|issue| query::matches(issue, &filter))
            .cloned()
            .collect()
    };
    issues.sort_by(|a, b| query::compare(sort, a, b));
    let hits = issues.len();

    if let Some(raw) = param(&params, "cursor") {
        let decoded = cursor::decode(raw)?;
        if decoded.q != fingerprint || decoded.s != sort.as_str() {
            return Err(CompatError::BadRequest(
                "cursor does not match this query".into(),
            ));
        }
        issues.retain(|issue| query::is_after_cursor(issue, sort, &decoded));
    }

    let next = if issues.len() > limit {
        issues.get(limit - 1).map(|last| Cursor {
            v: 1,
            q: fingerprint.clone(),
            s: sort.as_str().into(),
            k: query::sort_value(sort, last).to_string(),
            i: last.id.to_string(),
        })
    } else {
        None
    };
    let body: Vec<projection::SentryIssue> = issues
        .iter()
        .take(limit)
        .map(|issue| projection::project_issue(issue, &project, &state.self_url))
        .collect();
    Ok(page_response(
        body,
        hits,
        req.path(),
        &params,
        next.as_ref(),
        &state,
    ))
}

pub async fn mutate_issues(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    params: web::Query<Params>,
    body: web::Bytes,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, project_ref) = path.into_inner();
    let auth = authenticate(&req, &state).await?;
    let (_, project, _) =
        authorize_project(&state, &auth, &org_ref, &project_ref, Scope::EventWrite).await?;
    let params = params.into_inner();

    let value: Value = serde_json::from_slice(&body)
        .map_err(|_| CompatError::BadRequest("invalid JSON body".into()))?;
    let object = value
        .as_object()
        .ok_or_else(|| CompatError::BadRequest("body must be a JSON object".into()))?;
    if let Some(key) = object.keys().find(|key| key.as_str() != "status") {
        return Err(CompatError::BadRequest(format!(
            "unsupported attribute: {key}"
        )));
    }
    let status_text = object
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| CompatError::BadRequest("status is required".into()))?;
    let new_state = projection::status_from_str(status_text)
        .ok_or_else(|| CompatError::BadRequest(format!("unsupported status: {status_text}")))?;

    let ids: Vec<String> = all(&params, "id").into_iter().map(str::to_string).collect();
    let mut filter = query::parse_filter(param(&params, "query"), &ids, &[]);
    if !ids.is_empty() {
        // Explicit ids are addressed regardless of their current state.
        filter.states = None;
        // Invalid identifiers match nothing rather than every issue (FR-013).
        if filter.ids.is_empty() {
            return Ok(HttpResponse::Ok().json(Vec::<projection::SentryIssue>::new()));
        }
    }

    let now = Utc::now();
    let updated: Vec<Issue> = {
        let mut store = state.store.write().await;
        let mut changed = Vec::new();
        for issue in store.issues.values_mut() {
            if issue.project_id != project.id || !query::matches(issue, &filter) {
                continue;
            }
            if issue.state != new_state {
                issue.state = new_state;
                issue.state_history.push(StateChange {
                    state: new_state,
                    by: auth.email.clone(),
                    at: now,
                });
            }
            changed.push(issue.id);
        }
        changed
            .into_iter()
            .filter_map(|id| store.issues.get(&id).cloned())
            .collect()
    };
    let body: Vec<projection::SentryIssue> = updated
        .iter()
        .map(|issue| projection::project_issue(issue, &project, &state.self_url))
        .collect();
    Ok(HttpResponse::Ok().json(body))
}

pub async fn delete_issues(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, project_ref) = path.into_inner();
    let auth = authenticate(&req, &state).await?;
    authorize_project(&state, &auth, &org_ref, &project_ref, Scope::EventWrite).await?;
    Err(CompatError::Unsupported(
        "Deleting issues is not supported: Scout preserves issue history and raw payloads.".into(),
    ))
}

// --- tag values ------------------------------------------------------------

pub async fn project_tag_values(
    req: HttpRequest,
    path: web::Path<(String, String, String)>,
    params: web::Query<Params>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, project_ref, key) = path.into_inner();
    let auth = authenticate(&req, &state).await?;
    let (org, project, _) =
        authorize_project(&state, &auth, &org_ref, &project_ref, Scope::EventRead).await?;
    let params = params.into_inner();
    let query_text = param(&params, "query").unwrap_or_default();
    let values = {
        let store = state.store.read().await;
        projection::tag_values(
            store
                .issues
                .values()
                .filter(|issue| issue.project_id == project.id),
            &key,
            Some(query_text),
        )
    };
    let fingerprint = cursor::fingerprint(&[
        &key,
        query_text,
        "value",
        &org.id.to_string(),
        &project.id.to_string(),
    ]);
    let (page, next) = values_page(&values, &params, &fingerprint)?;
    let mut response = HttpResponse::Ok();
    response.insert_header((
        "Link",
        link_header(&state.self_url, req.path(), &params, next.as_ref()),
    ));
    Ok(response.json(page))
}

pub async fn issue_tag_values(
    req: HttpRequest,
    path: web::Path<(String, String, String)>,
    params: web::Query<Params>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, issue_ref, key) = path.into_inner();
    let auth = authenticate(&req, &state).await?;
    let params = params.into_inner();
    let query_text = param(&params, "query").unwrap_or_default();

    let (org_id, issue) = {
        let store = state.store.read().await;
        let org = resolve_org(&store, &org_ref)
            .ok_or_else(|| CompatError::NotFound("organisation not found".into()))?;
        let issue = Uuid::parse_str(&issue_ref)
            .ok()
            .and_then(|id| store.issues.get(&id))
            .ok_or_else(|| CompatError::NotFound("issue not found".into()))?;
        let project = store
            .projects
            .get(&issue.project_id)
            .filter(|project| project.org_id == org.id)
            .filter(|project| project.deleted_at.is_none())
            .ok_or_else(|| CompatError::NotFound("issue not found".into()))?;
        let role = store
            .role_for(auth.user_id, project.id)
            .ok_or_else(|| CompatError::NotFound("issue not found".into()))?;
        let scopes = scopes_for(role, store.org_role_for(auth.user_id, org.id));
        auth.require(&scopes, Scope::EventRead)?;
        (org.id, issue.clone())
    };

    let values = projection::tag_values(std::iter::once(&issue), &key, Some(query_text));
    let fingerprint = cursor::fingerprint(&[
        &key,
        query_text,
        "value",
        &org_id.to_string(),
        &issue.id.to_string(),
    ]);
    let (page, next) = values_page(&values, &params, &fingerprint)?;
    let mut response = HttpResponse::Ok();
    response.insert_header((
        "Link",
        link_header(&state.self_url, req.path(), &params, next.as_ref()),
    ));
    Ok(response.json(page))
}

// --- releases --------------------------------------------------------------

pub async fn release_commit_files(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, version) = path.into_inner();
    let auth = authenticate(&req, &state).await?;

    let (roles, present) = {
        let store = state.store.read().await;
        let org = resolve_org(&store, &org_ref)
            .ok_or_else(|| CompatError::NotFound("organisation not found".into()))?
            .clone();
        let roles: Vec<(Project, crate::models::Role)> = store
            .projects
            .values()
            .filter(|project| project.org_id == org.id && project.deleted_at.is_none())
            .filter_map(|project| {
                store
                    .role_for(auth.user_id, project.id)
                    .map(|role| (project.clone(), role))
            })
            .collect();
        if roles.is_empty() {
            return Err(CompatError::NotFound("organisation not found".into()));
        }
        let scopes: Vec<std::collections::HashSet<Scope>> = roles
            .iter()
            .map(|(_, role)| scopes_for(*role, store.org_role_for(auth.user_id, org.id)))
            .collect();
        if !scopes
            .iter()
            .any(|scopes| scopes.contains(&Scope::ProjectReleases))
        {
            return Err(CompatError::Forbidden(
                "missing required scope project:releases".into(),
            ));
        }
        let present = roles.iter().any(|(project, _)| {
            store.issues.values().any(|issue| {
                issue.project_id == project.id && issue.release.as_deref() == Some(version.as_str())
            })
        });
        (roles, present)
    };
    let _ = roles;
    if present {
        Ok(HttpResponse::Ok().json(Vec::<Value>::new()))
    } else {
        Err(CompatError::NotFound("release not found".into()))
    }
}

// --- unsupported subsystems ------------------------------------------------

async fn authorize_project_read(
    req: &HttpRequest,
    state: &AppState,
    org_ref: &str,
    project_ref: &str,
) -> Result<(Organisation, Project), CompatError> {
    let auth: CompatAuth = authenticate(req, state).await?;
    let (org, project, _) =
        authorize_project(state, &auth, org_ref, project_ref, Scope::ProjectRead).await?;
    Ok((org, project))
}

pub async fn service_hooks(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, project_ref) = path.into_inner();
    authorize_project_read(&req, &state, &org_ref, &project_ref).await?;
    Ok(HttpResponse::Ok().json(Vec::<Value>::new()))
}

pub async fn service_hook_detail(
    req: HttpRequest,
    path: web::Path<(String, String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, project_ref, _hook) = path.into_inner();
    authorize_project_read(&req, &state, &org_ref, &project_ref).await?;
    Err(CompatError::NotFound("service hook not found".into()))
}

pub async fn service_hook_mutate(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, project_ref) = path.into_inner();
    authorize_project_read(&req, &state, &org_ref, &project_ref).await?;
    Err(CompatError::Unsupported(
        "Service hooks are not supported by Scout.".into(),
    ))
}

pub async fn service_hook_detail_mutate(
    req: HttpRequest,
    path: web::Path<(String, String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, project_ref, _hook) = path.into_inner();
    authorize_project_read(&req, &state, &org_ref, &project_ref).await?;
    Err(CompatError::Unsupported(
        "Service hooks are not supported by Scout.".into(),
    ))
}

pub async fn user_feedback(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, project_ref) = path.into_inner();
    authorize_project_read(&req, &state, &org_ref, &project_ref).await?;
    Ok(HttpResponse::Ok().json(Vec::<Value>::new()))
}

pub async fn user_feedback_create(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (org_ref, project_ref) = path.into_inner();
    authorize_project_read(&req, &state, &org_ref, &project_ref).await?;
    Err(CompatError::Unsupported(
        "Legacy user reports are not supported by Scout.".into(),
    ))
}

pub async fn spike_protections(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let org_ref = path.into_inner();
    let auth = authenticate(&req, &state).await?;
    authorize_org(&state, &auth, &org_ref, Scope::OrgRead).await?;
    Err(CompatError::NotFound("spike protection not found".into()))
}

pub async fn spike_protections_mutate(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let org_ref = path.into_inner();
    let auth = authenticate(&req, &state).await?;
    authorize_org(&state, &auth, &org_ref, Scope::OrgRead).await?;
    Err(CompatError::Unsupported(
        "Spike protection is not supported by Scout.".into(),
    ))
}

pub async fn app_installations(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let org_ref = path.into_inner();
    let auth = authenticate(&req, &state).await?;
    authorize_org(&state, &auth, &org_ref, Scope::OrgIntegrations).await?;
    Ok(HttpResponse::Ok().json(Vec::<Value>::new()))
}

pub async fn app_installations_mutate(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let org_ref = path.into_inner();
    let auth = authenticate(&req, &state).await?;
    authorize_org(&state, &auth, &org_ref, Scope::OrgRead).await?;
    Err(CompatError::Unsupported(
        "Sentry app installations are not supported by Scout.".into(),
    ))
}

pub async fn external_issues_mutate(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let _installation = path.into_inner();
    authenticate(&req, &state).await?;
    Err(CompatError::Unsupported(
        "External issues are not supported by Scout.".into(),
    ))
}

pub async fn external_issue_delete(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, CompatError> {
    ensure_enabled(&state)?;
    let (_installation, _external) = path.into_inner();
    authenticate(&req, &state).await?;
    Err(CompatError::Unsupported(
        "External issues are not supported by Scout.".into(),
    ))
}
