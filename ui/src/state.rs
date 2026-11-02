use crate::api::{self, Account, Issue, LogLine, Member, ProjectSummary};
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
    Tokens,
    Account,
    Orgs,
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
    /// The operator's one-time recovery codes, shown on the first sign-in.
    FirstSignInCodes,
    /// A freshly created or rotated ingest key, shown once.
    NewKey,
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
    pub search_has_more: bool,
    pub issue: Option<api::IssueDetailResponse>,
    pub members: Vec<Member>,
    pub settings: Option<api::SettingsResponse>,
    pub tokens: Vec<api::ApiToken>,
    /// The plaintext of a token created in this browser session; never persisted.
    pub new_token: Option<api::CreatedApiToken>,
    /// Recovery codes returned by the current sign-in, shown once and cleared on
    /// acknowledgement (FR-012, SC-017).
    pub pending_recovery_codes: Vec<String>,
    /// Accounts currently locked by failed sign-ins, for the operator console.
    pub locked_users: Vec<api::LockedUser>,
    /// The current project organisation's optional OIDC connection (FR-018).
    pub oidc: Option<api::OidcConfig>,
    /// Distinguishes "loaded and not configured" from "not loaded yet".
    pub oidc_loaded: bool,
    /// Deployment-level organisation list for the operator console.
    pub orgs: Vec<api::OrgSummary>,
    /// The setup token returned by the last organisation creation, shown once.
    pub last_setup_token: Option<(String, String)>,
    /// The invite token returned by the last invitation creation, shown once.
    pub last_invitation: Option<(String, String, String)>,
    /// A freshly created or rotated ingest key: (label, plaintext).
    pub new_key: Option<(String, String)>,
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
            search_has_more: false,
            issue: None,
            members: Vec::new(),
            settings: None,
            tokens: Vec::new(),
            new_token: None,
            pending_recovery_codes: Vec::new(),
            locked_users: Vec::new(),
            oidc: None,
            oidc_loaded: false,
            orgs: Vec::new(),
            last_setup_token: None,
            last_invitation: None,
            new_key: None,
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

pub fn is_operator(state: &AppState) -> bool {
    state.me.as_ref().is_some_and(|user| user.is_operator)
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

pub async fn load_accounts(store: Signal<AppState>) {
    // Demo shortcuts exist only when the demo endpoint is registered. In a
    // production deployment the request fails and no accounts are offered.
    match api::demo_accounts().await {
        Ok(data) => store.update(|state| state.accounts = data.accounts),
        Err(_) => store.update(|state| state.accounts.clear()),
    }
}

pub async fn login(store: Signal<AppState>, email: String, password: String) {
    match api::login(&email, &password).await {
        Ok(data) => {
            util::storage_set("scout_token", &data.token);
            let codes = data.recovery_codes.clone();
            store.update(|state| {
                state.token = Some(data.token.clone());
                state.me = Some(data.user.clone());
                state.pending_recovery_codes = codes.clone();
                state.modal = if codes.is_empty() {
                    None
                } else {
                    Some(ModalKind::FirstSignInCodes)
                };
            });
            load_projects(store).await;
            if util::current_hash().is_empty() {
                util::set_hash("#/issues");
            }
            sync_from_hash(store).await;
            toast(store, format!("Signed in as {}", data.user.name), "success");
            claim_pending_invitation(store).await;
        }
        Err(error) => toast(store, error, "error"),
    }
}

/// Accept an invitation the visitor held while they signed in. The token is
/// remembered across the sign-in so an existing account can claim it.
async fn claim_pending_invitation(store: Signal<AppState>) {
    let Some(invitation) = util::storage_get("scout_pending_invite") else {
        return;
    };
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::accept_invitation(&invitation, Some(&token), None, None).await {
        Ok(_) => {
            util::storage_remove("scout_pending_invite");
            load_projects(store).await;
            toast(store, "Invitation accepted", "success");
        }
        Err(error) => toast(store, error, "error"),
    }
}

/// Redeem an invitation as a new account, then enter the dashboard exactly as a
/// password sign-in would.
pub async fn accept_invitation_as_new(
    store: Signal<AppState>,
    invitation: String,
    name: String,
    password: String,
) {
    match api::accept_invitation(&invitation, None, Some(&name), Some(&password)).await {
        Ok(data) => {
            util::storage_set("scout_token", &data.token);
            util::storage_remove("scout_pending_invite");
            let codes = data.recovery_codes.clone();
            store.update(|state| {
                state.token = Some(data.token.clone());
                state.me = Some(data.user.clone());
                state.pending_recovery_codes = codes.clone();
                state.modal = if codes.is_empty() {
                    None
                } else {
                    Some(ModalKind::FirstSignInCodes)
                };
            });
            util::set_hash("#/issues");
            load_projects(store).await;
            sync_from_hash(store).await;
            toast(store, format!("Welcome, {}", data.user.name), "success");
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub fn acknowledge_recovery_codes(store: Signal<AppState>) {
    store.update(|state| {
        state.pending_recovery_codes.clear();
        state.modal = None;
    });
}

pub fn dismiss_new_key(store: Signal<AppState>) {
    store.update(|state| {
        state.new_key = None;
        state.modal = None;
    });
}

/// Handle the identity provider's `#oidc=<outcome>&token=<session>` landing.
/// Returns true when the fragment was an OIDC landing and has been consumed.
pub async fn apply_oidc_landing(store: Signal<AppState>) -> bool {
    let Some((outcome, token)) = util::oidc_landing(&util::current_hash()) else {
        return false;
    };
    // Strip the fragment before anything else so the token is not left in the
    // address bar or in history.
    util::set_hash("#/issues");

    if let Some(token) = token {
        util::storage_set("scout_token", &token);
        match api::me(&token).await {
            Ok(data) => {
                store.update(|state| {
                    state.token = Some(token);
                    state.me = Some(data.user);
                });
                load_projects(store).await;
                sync_from_hash(store).await;
                toast(store, "Signed in with your identity provider", "success");
                return true;
            }
            Err(_) => util::storage_remove("scout_token"),
        }
    }

    let message = match outcome.as_str() {
        "email_unverified" => "Your provider did not verify your email address.",
        "membership_unknown" => "No membership in this organisation matches that account.",
        "state_invalid" => "That sign-in link expired or was already used.",
        "provider_error" => "The identity provider could not complete sign-in.",
        _ => "Single sign-on did not complete.",
    };
    toast(store, message, "error");
    true
}

pub async fn change_password(
    store: Signal<AppState>,
    current_password: String,
    new_password: String,
) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::change_password(&token, &current_password, &new_password).await {
        Ok(_) => toast(store, "Password changed", "success"),
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn reissue_recovery_codes(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::reissue_recovery_codes(&token).await {
        Ok(data) => {
            store.update(|state| {
                state.pending_recovery_codes = data.recovery_codes;
                state.modal = Some(ModalKind::FirstSignInCodes);
            });
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn load_account(store: Signal<AppState>) {
    if !store.with(is_operator) {
        store.update(|state| state.locked_users.clear());
        return;
    }
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::locked_users(&token).await {
        Ok(data) => store.update(|state| state.locked_users = data.users),
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn unlock_account(store: Signal<AppState>, user_id: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::unlock_user(&token, &user_id).await {
        Ok(_) => {
            toast(store, "Account unlocked", "success");
            load_account(store).await;
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
        state.search_has_more = false;
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
    load_oidc(store).await;
}

/// The organisation the current project belongs to.
fn settings_org_id(state: &AppState) -> Option<String> {
    state
        .settings
        .as_ref()
        .map(|settings| settings.project.org_id.clone())
}

pub async fn load_oidc(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let Some(org_id) = store.with(settings_org_id) else {
        return;
    };
    match api::oidc_config(&token, &org_id).await {
        Ok(config) => store.update(|state| {
            state.oidc = config;
            state.oidc_loaded = true;
        }),
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn save_oidc(
    store: Signal<AppState>,
    issuer: String,
    client_id: String,
    client_secret: String,
    scopes: String,
) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let Some(org_id) = store.with(settings_org_id) else {
        return;
    };
    match api::put_oidc(
        &token,
        &org_id,
        &issuer,
        &client_id,
        &client_secret,
        &scopes,
    )
    .await
    {
        Ok(config) => {
            store.update(|state| {
                state.oidc = Some(config);
                state.oidc_loaded = true;
            });
            toast(store, "Identity provider saved", "success");
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn remove_oidc(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let Some(org_id) = store.with(settings_org_id) else {
        return;
    };
    match api::delete_oidc(&token, &org_id).await {
        Ok(_) => {
            store.update(|state| state.oidc = None);
            toast(store, "Identity provider removed", "success");
        }
        Err(error) => toast(store, error, "error"),
    }
}

// ------------------------------------------------------------------ organisations

pub async fn load_orgs(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::orgs(&token).await {
        Ok(data) => store.update(|state| state.orgs = data.organisations),
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn create_org(store: Signal<AppState>, name: String, initial_admin_email: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::create_org(&token, &name, &initial_admin_email).await {
        Ok(created) => {
            let org_name = created.organisation.name.clone();
            store.update(|state| {
                state.last_setup_token =
                    Some((org_name.clone(), created.initial_admin_setup_token));
            });
            load_orgs(store).await;
            toast(store, format!("Organisation {org_name} created"), "success");
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn create_invitation(store: Signal<AppState>, email: String, role: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let Some(org_id) = store.with(settings_org_id) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    if project_id.is_empty() {
        toast(store, "Select a project before inviting", "error");
        return;
    }
    match api::create_invitation(&token, &org_id, &email, &project_id, &role).await {
        Ok(created) => {
            let id = created
                .invitation
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            store.update(|state| {
                state.last_invitation = Some((email.clone(), created.token, id));
            });
            toast(store, format!("Invitation created for {email}"), "success");
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn revoke_invitation(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let Some(org_id) = store.with(settings_org_id) else {
        return;
    };
    let Some((_, _, invitation_id)) = store.with(|state| state.last_invitation.clone()) else {
        return;
    };
    match api::revoke_invitation(&token, &org_id, &invitation_id).await {
        Ok(_) => {
            store.update(|state| state.last_invitation = None);
            toast(store, "Invitation revoked", "success");
        }
        Err(error) => toast(store, error, "error"),
    }
}

// ------------------------------------------------------------------ api tokens

pub async fn load_tokens(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::api_tokens(&token).await {
        Ok(tokens) => store.update(|state| state.tokens = tokens),
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn create_token(store: Signal<AppState>, label: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::create_api_token(&token, &label).await {
        Ok(created) => {
            store.update(|state| state.new_token = Some(created));
            toast(
                store,
                "Token created — copy it now, it is shown once",
                "success",
            );
            load_tokens(store).await;
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn revoke_token(store: Signal<AppState>, id: String) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    match api::revoke_api_token(&token, &id).await {
        Ok(()) => {
            toast(store, "Token revoked", "success");
            load_tokens(store).await;
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub fn dismiss_new_token(store: Signal<AppState>) {
    store.update(|state| state.new_token = None);
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
            state.search_has_more = false;
        });
        return;
    }
    match api::search(&token, &project_id, &query).await {
        Ok(data) => store.update(|state| {
            state.search_issues = data.issues;
            state.search_logs = data.logs;
            state.search_has_more = data.has_more;
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
        ["tokens", ..] => (ViewKind::Tokens, None),
        ["account", ..] => (ViewKind::Account, None),
        ["orgs", ..] => (ViewKind::Orgs, None),
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
        ViewKind::Tokens => load_tokens(store).await,
        ViewKind::Account => load_account(store).await,
        ViewKind::Orgs => load_orgs(store).await,
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
                state.new_key = Some((prefix.clone(), plaintext.clone()));
                state.modal = Some(ModalKind::NewKey);
            });
            load_settings(store).await;
            load_projects(store).await;
        }
        Err(error) => toast(store, error, "error"),
    }
}

pub async fn rotate_key(store: Signal<AppState>) {
    let Some(token) = store.with(|state| state.token.clone()) else {
        return;
    };
    let project_id = store.with(|state| state.project_id.clone());
    match api::rotate_key(&token, &project_id, "Rotated").await {
        Ok(data) => {
            let plaintext = data.key.clone();
            let prefix: String = plaintext.chars().take(12).collect();
            store.update(|state| {
                for project in state.projects.iter_mut() {
                    if project.id == project_id {
                        project.key = Some(plaintext.clone());
                    }
                }
                state.new_key = Some((prefix, plaintext.clone()));
                state.modal = Some(ModalKind::NewKey);
            });
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
