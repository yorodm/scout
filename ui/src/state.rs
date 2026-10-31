use crate::api::{self, Account, AccountMembership, Issue, LogLine, Member, ProjectSummary};
use crate::util;
use gloo_timers::future::TimeoutFuture;
use serde_json::{json, Value};
use sycamore::prelude::*;
use wasm_bindgen_futures::spawn_local;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Issues,
    Issue,
    Logs,
    Search,
    Members,
    Settings,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filters {
    pub q: String,
    pub state: String,
    pub level: String,
    pub sort: String,
}

impl Default for Filters {
    fn default() -> Self {
        Filters {
            q: String::new(),
            state: "open".into(),
            level: String::new(),
            sort: "last_seen".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogFilters {
    pub q: String,
    pub level: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalKind {
    SendEvent,
    SendLog,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub message: String,
    pub kind: String,
    pub nonce: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    pub token: Option<String>,
    pub me: Option<api::User>,
    pub accounts: Vec<Account>,
    pub projects: Vec<ProjectSummary>,
    pub project_id: String,
    pub view: ViewKind,
    pub issue_id: Option<String>,
    pub issues: Vec<Issue>,
    pub logs: Vec<LogLine>,
    pub search_issues: Vec<Issue>,
    pub search_logs: Vec<LogLine>,
    pub issue: Option<api::IssueDetailResponse>,
    pub members: Vec<Member>,
    pub settings: Option<api::SettingsResponse>,
    pub filters: Filters,
    pub log_filters: LogFilters,
    pub search_query: String,
    pub outage: bool,
    pub modal: Option<ModalKind>,
    pub toast: Option<Toast>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            token: None,
            me: None,
            accounts: Vec::new(),
            projects: Vec::new(),
            project_id: util::storage_get("scout_project").unwrap_or_default(),
            view: ViewKind::Issues,
            issue_id: None,
            issues: Vec::new(),
            logs: Vec::new(),
            search_issues: Vec::new(),
            search_logs: Vec::new(),
            issue: None,
            members: Vec::new(),
            settings: None,
            filters: Filters::default(),
            log_filters: LogFilters::default(),
            search_query: String::new(),
            outage: false,
            modal: None,
            toast: None,
        }
    }
}

pub fn use_store() -> Signal<AppState> {
    use_context::<Signal<AppState>>()
}

pub fn project(state: &AppState) -> Option<&ProjectSummary> {
    state.projects.iter().find(|p| p.id == state.project_id)
}

pub fn project_name(state: &AppState) -> String {
    project(state).map(|p| p.name.clone()).unwrap_or_default()
}

pub fn current_role(state: &AppState) -> String {
    project(state).map(|p| p.role.clone()).unwrap_or_default()
}

pub fn can_manage(state: &AppState) -> bool {
    matches!(current_role(state).as_str(), "admin" | "owner")
}

pub fn can_triage(state: &AppState) -> bool {
    matches!(current_role(state).as_str(), "triager" | "admin" | "owner")
}

pub fn can_send(state: &AppState) -> bool {
    can_manage(state) && project(state).and_then(|p| p.key.as_ref()).is_some()
}

pub fn toast(store: Signal<AppState>, message: impl Into<String>, kind: &str) {
    let message = message.into();
    let nonce = js_sys::Math::random();
    store.update(|state| {
        state.toast = Some(Toast {
            message: message.clone(),
            kind: kind.into(),
            nonce,
        });
    });
    spawn_local(async move {
        TimeoutFuture::new(4200).await;
        store.update(|state| {
            if state.toast.as_ref().map(|toast| toast.nonce) == Some(nonce) {
                state.toast = None;
            }
        });
    });
}

pub fn open_modal(store: Signal<AppState>, kind: ModalKind) {
    store.update(|state| state.modal = Some(kind));
}

pub fn close_modal(store: Signal<AppState>) {
    store.update(|state| state.modal = None);
}

// ------------------------------------------------------------------ loading

pub fn fallback_accounts() -> Vec<Account> {
    vec![
        Account {
            name: "Dana Demo".into(),
            email: "demo@scout.local".into(),
            memberships: vec![
                AccountMembership {
                    role: "owner".into(),
                    project_name: "scout-web".into(),
                },
                AccountMembership {
                    role: "admin".into(),
                    project_name: "platform-api".into(),
                },
            ],
        },
        Account {
            name: "Toni Triager".into(),
            email: "triager@scout.local".into(),
            memberships: vec![AccountMembership {
                role: "triager".into(),
                project_name: "scout-web".into(),
            }],
        },
        Account {
            name: "Vic Viewer".into(),
            email: "viewer@scout.local".into(),
            memberships: vec![AccountMembership {
                role: "viewer".into(),
                project_name: "scout-web".into(),
            }],
        },
    ]
}

pub async fn load_accounts(store: Signal<AppState>) {
    match api::demo_accounts().await {
        Ok(data) => store.update(|state| state.accounts = data.accounts),
        Err(_) => store.update(|state| state.accounts = fallback_accounts()),
    }
}

pub async fn login(store: Signal<AppState>, email: String, password: String) {
    match api::login(&email, &password).await {
        Ok(data) => {
            util::storage_set("scout_token", &data.token);
            store.update(|state| {
                state.token = Some(data.token.clone());
                state.me = Some(data.user.clone());
            });
            load_projects(store).await;
            if util::current_hash().is_empty() {
                util::set_hash("#/issues");
            }
            sync_from_hash(store).await;
            toast(store, format!("Signed in as {}", data.user.name), "success");
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn logout(store: Signal<AppState>) {
    if let Some(token) = store.with(|state| state.token.clone()) {
        let _ = api::logout(&token).await;
    }
    util::storage_remove("scout_token");
    store.update(|state| {
        state.token = None;
        state.me = None;
        state.projects.clear();
        state.project_id.clear();
        state.issues.clear();
        state.logs.clear();
        state.search_issues.clear();
        state.search_logs.clear();
        state.issue = None;
        state.members.clear();
        state.settings = None;
        state.modal = None;
        state.issue_id = None;
        state.view = ViewKind::Issues;
    });
}

pub async fn load_projects(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::projects(&token).await {
        Ok(data) => {
            store.update(|state| {
                state.projects = data.projects;
                if !state
                    .projects
                    .iter()
                    .any(|project| project.id == state.project_id)
                {
                    state.project_id = state
                        .projects
                        .first()
                        .map(|project| project.id.clone())
                        .unwrap_or_default();
                }
            });
            let project_id = store.with(|state| state.project_id.clone());
            util::storage_set("scout_project", &project_id);
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn load_issues(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    if project_id.is_empty() {
        return;
    }
    let filters = store.with(|state| state.filters.clone());
    let query = api::IssueQuery {
        q: &filters.q,
        state: &filters.state,
        level: &filters.level,
        sort: &filters.sort,
    };
    match api::issues(&token, &project_id, &query).await {
        Ok(data) => store.update(|state| state.issues = data.issues),
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn load_issue(store: Signal<AppState>, issue_id: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::issue(&token, &issue_id).await {
        Ok(data) => store.update(|state| state.issue = Some(data)),
        Err(error) => {
            store.update(|state| state.issue = None);
            toast(store, error, "error");
        }
    }
}

pub async fn load_logs(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    if project_id.is_empty() {
        return;
    }
    let filters = store.with(|state| state.log_filters.clone());
    match api::logs(&token, &project_id, &filters.q, &filters.level).await {
        Ok(data) => store.update(|state| state.logs = data.logs),
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn load_members(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    if project_id.is_empty() {
        return;
    }
    match api::members(&token, &project_id).await {
        Ok(data) => store.update(|state| state.members = data.members),
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn load_outage(store: Signal<AppState>) {
    if let Ok(down) = api::outage().await {
        store.update(|state| state.outage = down);
    }
}

pub async fn load_settings(store: Signal<AppState>) {
    load_outage(store).await;
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    if project_id.is_empty() {
        return;
    }
    match api::settings(&token, &project_id).await {
        Ok(data) => store.update(|state| state.settings = Some(data)),
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn search(store: Signal<AppState>, query: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    store.update(|state| state.search_query = query.clone());
    if query.trim().is_empty() || project_id.is_empty() {
        store.update(|state| {
            state.search_issues.clear();
            state.search_logs.clear();
        });
        return;
    }
    match api::search(&token, &project_id, &query).await {
        Ok(data) => store.update(|state| {
            state.search_issues = data.issues;
            state.search_logs = data.logs;
        }),
        Err(error) => toast(store, error, "error"),
    }
}

// ------------------------------------------------------------------ routing

pub async fn sync_from_hash(store: Signal<AppState>) {
    if store.with(|state| state.me.is_none()) {
        return;
    }
    let hash = util::current_hash();
    let raw = hash.trim_start_matches('#').trim_start_matches('/');
    let parts: Vec<&str> = raw.split('/').filter(|part| !part.is_empty()).collect();
    let (view, issue_id) = match parts.as_slice() {
        ["issues", id, ..] => (ViewKind::Issue, Some((*id).to_string())),
        ["issues", ..] => (ViewKind::Issues, None),
        ["logs", ..] => (ViewKind::Logs, None),
        ["search", ..] => (ViewKind::Search, None),
        ["members", ..] => (ViewKind::Members, None),
        ["settings", ..] => (ViewKind::Settings, None),
        _ => (ViewKind::Issues, None),
    };

    store.update(|state| {
        state.view = view;
        state.issue_id = issue_id.clone();
        state.issue = None;
    });

    match view {
        ViewKind::Issues => load_issues(store).await,
        ViewKind::Issue => {
            if let Some(id) = issue_id {
                load_issue(store, id).await;
            }
        }
        ViewKind::Logs => load_logs(store).await,
        ViewKind::Search => {
            let query = store.with(|state| state.search_query.clone());
            if !query.is_empty() {
                search(store, query).await;
            }
        }
        ViewKind::Members => load_members(store).await,
        ViewKind::Settings => load_settings(store).await,
    }
}

pub fn navigate(store: Signal<AppState>, hash: &str) {
    if util::current_hash() == hash {
        spawn_local(sync_from_hash(store));
    } else {
        util::set_hash(hash);
    }
}

pub fn change_project(store: Signal<AppState>, project_id: String) {
    store.update(|state| {
        state.project_id = project_id;
        state.issues.clear();
        state.logs.clear();
        state.issue = None;
        state.members.clear();
        state.settings = None;
        state.search_issues.clear();
        state.search_logs.clear();
    });
    let project_id = store.with(|state| state.project_id.clone());
    util::storage_set("scout_project", &project_id);
    navigate(store, "#/issues");
}

pub fn poll_issues(store: Signal<AppState>) {
    spawn_local(async move {
        for _ in 0..5 {
            TimeoutFuture::new(400).await;
            load_issues(store).await;
        }
        load_projects(store).await;
    });
}

pub fn poll_logs(store: Signal<AppState>) {
    spawn_local(async move {
        for _ in 0..4 {
            TimeoutFuture::new(350).await;
            load_logs(store).await;
        }
        load_projects(store).await;
    });
}

// ------------------------------------------------------------------ mutations

pub async fn change_issue_state(store: Signal<AppState>, issue_id: String, next: &str) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::set_issue_state(&token, &issue_id, next).await {
        Ok(data) => {
            store.update(|state| {
                if let Some(detail) = state.issue.as_mut() {
                    detail.issue = data.issue.clone();
                }
            });
            load_projects(store).await;
            toast(
                store,
                if next == "resolved" {
                    "Issue resolved"
                } else {
                    "Issue reopened"
                },
                "success",
            );
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn categorise(
    store: Signal<AppState>,
    issue_id: String,
    action: &str,
    category: Option<String>,
) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::categorise(&token, &issue_id, action, category.as_deref()).await {
        Ok(data) => {
            store.update(|state| {
                if let Some(detail) = state.issue.as_mut() {
                    detail.issue = data.issue.clone();
                }
            });
            toast(
                store,
                if action == "confirm" {
                    "Suggestion confirmed"
                } else {
                    "Categorisation overridden"
                },
                "success",
            );
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn update_member(store: Signal<AppState>, user_id: String, role: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    match api::update_member(&token, &project_id, &user_id, &role).await {
        Ok(_) => {
            toast(store, "Member role updated", "success");
            load_members(store).await;
            load_projects(store).await;
        }
        Err(error) => {
            toast(store, error, "error");
            load_members(store).await;
        }
    }
}

pub async fn add_member(store: Signal<AppState>, email: String, role: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    match api::add_member(&token, &project_id, &email, &role).await {
        Ok(_) => {
            toast(store, "Member added", "success");
            load_members(store).await;
            load_projects(store).await;
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn remove_member(store: Signal<AppState>, user_id: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    match api::remove_member(&token, &project_id, &user_id).await {
        Ok(_) => {
            toast(store, "Member removed", "success");
            load_members(store).await;
            load_projects(store).await;
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn create_key(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    match api::create_key(&token, &project_id, "Created in prototype").await {
        Ok(data) => {
            // Keep the plaintext for this browser session so the demo send
            // actions work; the server never returns it again.
            let plaintext = data.key.clone();
            let prefix = data.key_prefix.clone();
            store.update(|state| {
                for project in state.projects.iter_mut() {
                    if project.id == project_id {
                        project.key = Some(plaintext.clone());
                    }
                }
            });
            toast(
                store,
                format!("Created key {prefix} — copy it now, it is shown once"),
                "success",
            );
            load_settings(store).await;
            load_projects(store).await;
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn revoke_key(store: Signal<AppState>, key_id: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    match api::revoke_key(&token, &project_id, &key_id).await {
        Ok(_) => {
            toast(store, "Key revoked", "success");
            load_settings(store).await;
            load_projects(store).await;
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn save_settings(
    store: Signal<AppState>,
    retention_days: u32,
    model: Option<api::ModelConfig>,
) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    match api::update_settings(&token, &project_id, retention_days, model.as_ref()).await {
        Ok(_) => {
            toast(store, "Settings saved", "success");
            load_settings(store).await;
            load_projects(store).await;
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn toggle_outage(store: Signal<AppState>) {
    let down = !store.with(|state| state.outage);
    match api::set_outage(down).await {
        Ok(down) => {
            store.update(|state| state.outage = down);
            toast(
                store,
                if down {
                    "Classifier outage simulated"
                } else {
                    "Classifier restored"
                },
                "success",
            );
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn reset_demo(store: Signal<AppState>) {
    let _ = api::reset_demo().await;
    util::storage_remove("scout_token");
    store.update(|state| {
        state.token = None;
        state.me = None;
    });
    util::reload();
}

// ------------------------------------------------------------------ ingest

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TestEventForm {
    pub event_type: String,
    pub message: String,
    pub level: String,
    pub release: String,
    pub count: u32,
    pub attachment: bool,
}

pub async fn send_test_events(
    store: Signal<AppState>,
    form: TestEventForm,
    progress: Signal<f64>,
) -> Result<(), String> {
    let target = store.with(|state| {
        project(state)
            .and_then(|project| project.key.clone().map(|key| (project.dsn_id.clone(), key)))
    });
    let Some((dsn_id, key)) = target else {
        return Err("You need an ingest key — sign in as an owner or admin.".into());
    };

    let total = form.count.clamp(1, 1000);
    for index in 0..total {
        let event_id = util::random_hex(16);
        let event = build_event_json(&event_id, &form);
        let payload = build_envelope(&event_id, &event, form.attachment);
        api::send_envelope(&dsn_id, &key, payload).await?;
        progress.set((index + 1) as f64 / total as f64);
    }
    Ok(())
}

pub async fn send_test_log(
    store: Signal<AppState>,
    message: String,
    level: String,
    source: String,
) -> Result<(), String> {
    let target = store.with(|state| {
        project(state)
            .and_then(|project| project.key.clone().map(|key| (project.dsn_id.clone(), key)))
    });
    let Some((dsn_id, key)) = target else {
        return Err("You need an ingest key — sign in as an owner or admin.".into());
    };
    api::send_log(&dsn_id, &key, &level, &message, &source).await?;
    Ok(())
}

fn build_event_json(event_id: &str, form: &TestEventForm) -> Value {
    let now = js_sys::Date::now();
    let fingerprint = format!(
        "{}|{}|src/lib/validate.ts|requireCurrency",
        form.event_type, form.message
    );
    json!({
        "event_id": event_id,
        "timestamp": now / 1000.0,
        "platform": "javascript",
        "level": form.level,
        "release": form.release,
        "environment": "production",
        "message": format!("{}: {}", form.event_type, form.message),
        "fingerprint": [fingerprint],
        "exception": {
            "values": [{
                "type": form.event_type,
                "value": form.message,
                "stacktrace": {
                    "frames": [
                        {
                            "filename": "src/checkout/index.ts",
                            "function": "handleCheckout",
                            "lineno": 57,
                            "colno": 5,
                            "in_app": true,
                            "context_line": "await payment.chargeCard(cart, discount);"
                        },
                        {
                            "filename": "src/checkout/payment.ts",
                            "function": "chargeCard",
                            "lineno": 88,
                            "colno": 9,
                            "in_app": true,
                            "context_line": "const amount = cart.total(currency);"
                        },
                        {
                            "filename": "src/lib/validate.ts",
                            "function": "requireCurrency",
                            "lineno": 19,
                            "colno": 7,
                            "in_app": true,
                            "context_line": "if (!value.currency) throw new TypeError(\"missing currency\");"
                        }
                    ]
                }
            }]
        },
        "breadcrumbs": {
            "values": [
                {
                    "timestamp": util::iso_from_millis(now - 6000.0),
                    "category": "console",
                    "level": "info",
                    "message": "applying discount SAVE10"
                },
                {
                    "timestamp": util::iso_from_millis(now - 3000.0),
                    "category": "http",
                    "level": "info",
                    "message": "POST /api/discounts 200",
                    "data": { "status": 200, "duration_ms": 84 }
                },
                {
                    "timestamp": util::iso_from_millis(now - 1000.0),
                    "category": "ui.click",
                    "level": "info",
                    "message": "button#pay"
                }
            ]
        },
        "tags": {
            "service": "checkout-web",
            "region": "eu-west-1",
            "source": "sycamore-prototype"
        },
        "user": { "id": "u_8241", "email": "oncall@acme.dev" },
        "sdk": { "name": "sentry.javascript.browser", "version": "8.42.0" }
    })
}

fn build_envelope(event_id: &str, event: &Value, attachment: bool) -> String {
    let body = serde_json::to_string(event).unwrap_or_default();
    let header = json!({
        "event_id": event_id,
        "sent_at": util::iso_from_millis(js_sys::Date::now())
    });
    let item = json!({ "type": "event", "length": body.len() });
    let mut payload = format!("{header}\n{item}\n{body}\n");
    if attachment {
        let attachment_text = "cart=8f21\ncurrency=undefined\nroute=/checkout\n";
        let item = json!({
            "type": "attachment",
            "length": attachment_text.len(),
            "filename": "checkout-context.txt",
            "content_type": "text/plain"
        });
        payload.push_str(&format!("{item}\n"));
        payload.push_str(attachment_text);
        payload.push('\n');
    }
    payload
}
