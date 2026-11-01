use gloo_net::http::{Request, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

// ------------------------------------------------------------------ types

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AccountMembership {
    pub role: String,
    pub project_name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Account {
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub memberships: Vec<AccountMembership>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DemoAccountsResponse {
    pub accounts: Vec<Account>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: User,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MeResponse {
    pub user: User,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ProjectSummary {
    pub id: String,
    pub dsn_id: String,
    pub slug: String,
    pub name: String,
    pub org_name: String,
    pub role: String,
    pub open_issues: u64,
    pub total_issues: u64,
    pub logs: u64,
    pub model_configured: bool,
    pub retention_days: u32,
    /// Plaintext key is never returned by list endpoints; this field is only
    /// populated in the browser session after creating or rotating a key.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub key_prefix: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ProjectsResponse {
    pub projects: Vec<ProjectSummary>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct StackFrame {
    pub filename: Option<String>,
    pub function: Option<String>,
    pub lineno: Option<i64>,
    pub colno: Option<i64>,
    pub in_app: Option<bool>,
    pub context_line: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Breadcrumb {
    pub timestamp: Option<String>,
    pub category: Option<String>,
    pub level: Option<String>,
    pub message: Option<String>,
    pub data: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Suggestion {
    pub category: String,
    pub confidence: f64,
    pub source: String,
    pub at: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Decision {
    pub category: String,
    pub by: String,
    pub at: String,
    pub overridden: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct StateChange {
    pub state: String,
    pub by: String,
    pub at: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Issue {
    pub id: String,
    pub project_id: String,
    pub fingerprint: String,
    pub title: String,
    pub culprit: Option<String>,
    pub level: String,
    pub first_seen: String,
    pub last_seen: String,
    pub count: u64,
    pub state: String,
    pub release: Option<String>,
    pub environment: Option<String>,
    #[serde(default)]
    pub tags: HashMap<String, Value>,
    #[serde(default)]
    pub frames: Vec<StackFrame>,
    #[serde(default)]
    pub breadcrumbs: Vec<Breadcrumb>,
    pub suggestion: Option<Suggestion>,
    pub decision: Option<Decision>,
    #[serde(default)]
    pub state_history: Vec<StateChange>,
    pub last_event_id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct EventMeta {
    pub id: String,
    pub sentry_event_id: String,
    pub received_at: String,
    pub size: usize,
    pub attachment_count: usize,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AttachmentMeta {
    pub filename: String,
    pub content_type: String,
    pub size: usize,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct IssueDetailResponse {
    pub issue: Issue,
    pub event: Option<EventMeta>,
    pub event_json: Option<Value>,
    #[serde(default)]
    pub attachments: Vec<AttachmentMeta>,
    pub raw_available: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct IssuesResponse {
    pub issues: Vec<Issue>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct IssueResponse {
    pub issue: Issue,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LogLine {
    pub id: String,
    pub project_id: String,
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub source: String,
    pub attributes: Value,
    pub received_at: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LogsResponse {
    pub logs: Vec<LogLine>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SearchResponse {
    pub issues: Vec<Issue>,
    pub logs: Vec<LogLine>,
    /// True when more than 200 matches existed and the response was truncated.
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Member {
    pub user_id: String,
    pub name: String,
    pub email: String,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MembersResponse {
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct IngestKey {
    pub id: String,
    pub project_id: String,
    pub label: String,
    pub key_prefix: String,
    pub created_at: String,
    pub revoked: bool,
}

/// Response to creating a key: the plaintext is present exactly once.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct KeyResponse {
    pub key: String,
    pub id: String,
    pub label: String,
    pub key_prefix: String,
    pub created_at: String,
    pub revoked: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Project {
    pub id: String,
    pub dsn_id: String,
    pub slug: String,
    pub name: String,
    pub org_id: String,
    pub retention_days: u32,
    pub model: Option<ModelConfig>,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SettingsResponse {
    pub project: Project,
    pub role: String,
    #[serde(default)]
    pub dsn_id: String,
    #[serde(default)]
    pub key_prefix: Option<String>,
    pub keys: Vec<IngestKey>,
    pub mock_model_endpoint: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct OkResponse {
    #[serde(default)]
    pub ok: bool,
}

// ------------------------------------------------------------------ requests

fn with_auth(request: RequestBuilder, token: Option<&str>) -> RequestBuilder {
    match token {
        Some(token) => request.header("Authorization", &format!("Bearer {token}")),
        None => request,
    }
}

fn error_message(text: &str, status: u16) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("Request failed ({status})"))
}

async fn read_response<T: DeserializeOwned>(
    response: gloo_net::http::Response,
) -> Result<T, String> {
    let status = response.status();
    if !(200..300).contains(&status) {
        let text = response.text().await.unwrap_or_default();
        return Err(error_message(&text, status));
    }
    response
        .json::<T>()
        .await
        .map_err(|error| error.to_string())
}

async fn send_builder<T: DeserializeOwned>(builder: RequestBuilder) -> Result<T, String> {
    let response = builder.send().await.map_err(|error| error.to_string())?;
    read_response(response).await
}

async fn send_request<T: DeserializeOwned>(request: Request) -> Result<T, String> {
    let response = request.send().await.map_err(|error| error.to_string())?;
    read_response(response).await
}

async fn get<T: DeserializeOwned>(path: &str, token: Option<&str>) -> Result<T, String> {
    send_builder(with_auth(Request::get(path), token)).await
}

async fn post<T: DeserializeOwned, B: Serialize>(
    path: &str,
    token: Option<&str>,
    body: &B,
) -> Result<T, String> {
    let builder = with_auth(Request::post(path), token);
    let request = builder.json(body).map_err(|error| error.to_string())?;
    send_request(request).await
}

async fn put<T: DeserializeOwned, B: Serialize>(
    path: &str,
    token: Option<&str>,
    body: &B,
) -> Result<T, String> {
    let builder = with_auth(Request::put(path), token);
    let request = builder.json(body).map_err(|error| error.to_string())?;
    send_request(request).await
}

async fn delete<T: DeserializeOwned>(path: &str, token: Option<&str>) -> Result<T, String> {
    send_builder(with_auth(Request::delete(path), token)).await
}

pub async fn get_text(path: &str, token: Option<&str>) -> Result<String, String> {
    let response = with_auth(Request::get(path), token)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(error_message(&text, status));
    }
    Ok(text)
}

// ------------------------------------------------------------------ session

pub async fn demo_accounts() -> Result<DemoAccountsResponse, String> {
    get("/api/demo/accounts", None).await
}

pub async fn login(email: &str, password: &str) -> Result<LoginResponse, String> {
    post(
        "/api/login",
        None,
        &serde_json::json!({ "email": email, "password": password }),
    )
    .await
}

pub async fn me(token: &str) -> Result<MeResponse, String> {
    get("/api/me", Some(token)).await
}

pub async fn logout(token: &str) -> Result<OkResponse, String> {
    post("/api/logout", Some(token), &serde_json::json!({})).await
}

// ------------------------------------------------------------------ data

pub struct IssueQuery<'a> {
    pub q: &'a str,
    pub state: &'a str,
    pub level: &'a str,
    pub sort: &'a str,
}

pub async fn projects(token: &str) -> Result<ProjectsResponse, String> {
    get("/api/projects", Some(token)).await
}

pub async fn issues(
    token: &str,
    project_id: &str,
    query: &IssueQuery<'_>,
) -> Result<IssuesResponse, String> {
    let path = format!(
        "/api/projects/{project_id}/issues?q={}&state={}&level={}&sort={}",
        crate::util::encode(query.q),
        crate::util::encode(query.state),
        crate::util::encode(query.level),
        crate::util::encode(query.sort)
    );
    get(&path, Some(token)).await
}

pub async fn issue(token: &str, issue_id: &str) -> Result<IssueDetailResponse, String> {
    get(&format!("/api/issues/{issue_id}"), Some(token)).await
}

pub async fn set_issue_state(
    token: &str,
    issue_id: &str,
    next_state: &str,
) -> Result<IssueResponse, String> {
    post(
        &format!("/api/issues/{issue_id}/state"),
        Some(token),
        &serde_json::json!({ "state": next_state }),
    )
    .await
}

pub async fn categorise(
    token: &str,
    issue_id: &str,
    action: &str,
    category: Option<&str>,
) -> Result<IssueResponse, String> {
    post(
        &format!("/api/issues/{issue_id}/categorisation"),
        Some(token),
        &serde_json::json!({ "action": action, "category": category }),
    )
    .await
}

pub async fn logs(
    token: &str,
    project_id: &str,
    q: &str,
    level: &str,
) -> Result<LogsResponse, String> {
    let path = format!(
        "/api/projects/{project_id}/logs?q={}&level={}&limit=300",
        crate::util::encode(q),
        crate::util::encode(level)
    );
    get(&path, Some(token)).await
}

pub async fn search(token: &str, project_id: &str, q: &str) -> Result<SearchResponse, String> {
    let path = format!(
        "/api/projects/{project_id}/search?q={}",
        crate::util::encode(q)
    );
    get(&path, Some(token)).await
}

pub async fn members(token: &str, project_id: &str) -> Result<MembersResponse, String> {
    get(&format!("/api/projects/{project_id}/members"), Some(token)).await
}

pub async fn add_member(
    token: &str,
    project_id: &str,
    email: &str,
    role: &str,
) -> Result<Value, String> {
    post(
        &format!("/api/projects/{project_id}/members"),
        Some(token),
        &serde_json::json!({ "email": email, "role": role }),
    )
    .await
}

pub async fn update_member(
    token: &str,
    project_id: &str,
    user_id: &str,
    role: &str,
) -> Result<Value, String> {
    put(
        &format!("/api/projects/{project_id}/members/{user_id}"),
        Some(token),
        &serde_json::json!({ "role": role }),
    )
    .await
}

pub async fn remove_member(token: &str, project_id: &str, user_id: &str) -> Result<Value, String> {
    delete(
        &format!("/api/projects/{project_id}/members/{user_id}"),
        Some(token),
    )
    .await
}

pub async fn create_key(token: &str, project_id: &str, label: &str) -> Result<KeyResponse, String> {
    post(
        &format!("/api/projects/{project_id}/keys"),
        Some(token),
        &serde_json::json!({ "label": label }),
    )
    .await
}

pub async fn revoke_key(token: &str, project_id: &str, key_id: &str) -> Result<Value, String> {
    delete(
        &format!("/api/projects/{project_id}/keys/{key_id}"),
        Some(token),
    )
    .await
}

pub async fn settings(token: &str, project_id: &str) -> Result<SettingsResponse, String> {
    get(&format!("/api/projects/{project_id}/settings"), Some(token)).await
}

pub async fn update_settings(
    token: &str,
    project_id: &str,
    retention_days: u32,
    model: Option<&ModelConfig>,
) -> Result<Value, String> {
    put(
        &format!("/api/projects/{project_id}/settings"),
        Some(token),
        &serde_json::json!({ "retention_days": retention_days, "model": model }),
    )
    .await
}

pub async fn outage() -> Result<bool, String> {
    let value: Value = get("/mock-jev/outage", None).await?;
    Ok(value.get("down").and_then(Value::as_bool).unwrap_or(false))
}

pub async fn set_outage(down: bool) -> Result<bool, String> {
    let value: Value = post(
        "/mock-jev/outage",
        None,
        &serde_json::json!({ "down": down }),
    )
    .await?;
    Ok(value.get("down").and_then(Value::as_bool).unwrap_or(down))
}

pub async fn reset_demo() -> Result<Value, String> {
    post("/api/demo/reset", None, &serde_json::json!({})).await
}

// ------------------------------------------------------------------ ingest

pub async fn send_envelope(dsn_id: &str, key: &str, payload: String) -> Result<Value, String> {
    let path = format!(
        "/api/{}/envelope/?sentry_key={}",
        crate::util::encode(dsn_id),
        crate::util::encode(key)
    );
    let request = Request::post(&path)
        .header("Content-Type", "application/x-sentry-envelope")
        .body(payload)
        .map_err(|error| error.to_string())?;
    send_request(request).await
}

pub async fn send_log(
    dsn_id: &str,
    key: &str,
    level: &str,
    message: &str,
    source: &str,
) -> Result<Value, String> {
    let path = format!(
        "/api/{}/logs/?sentry_key={}",
        crate::util::encode(dsn_id),
        crate::util::encode(key)
    );
    post(
        &path,
        None,
        &serde_json::json!({
            "logs": [{
                "timestamp": js_sys::Date::now() / 1000.0,
                "level": level,
                "message": message,
                "source": source,
                "attributes": { "service": source, "sent_by": "sycamore-ui" }
            }]
        }),
    )
    .await
}

// ------------------------------------------------------------------ api tokens

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ApiToken {
    pub id: String,
    pub label: String,
    pub token_prefix: String,
    pub created_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CreatedApiToken {
    pub token: String,
    pub id: String,
    pub label: String,
    pub token_prefix: String,
    pub created_at: String,
}

pub async fn api_tokens(token: &str) -> Result<Vec<ApiToken>, String> {
    get("/api/me/tokens", Some(token)).await
}

pub async fn create_api_token(token: &str, label: &str) -> Result<CreatedApiToken, String> {
    post("/api/me/tokens", Some(token), &json!({ "label": label })).await
}

pub async fn revoke_api_token(token: &str, id: &str) -> Result<(), String> {
    let response = with_auth(
        Request::delete(&format!("/api/me/tokens/{id}")),
        Some(token),
    )
    .send()
    .await
    .map_err(|error| error.to_string())?;
    let status = response.status();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(error_message(&text, status))
    }
}
