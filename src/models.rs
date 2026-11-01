use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Organisation {
    pub id: Uuid,
    pub name: String,
    /// Stable, URL-safe identifier used by the Sentry-compatible paths. Derived
    /// from the name at creation and never recomputed afterwards.
    #[serde(default)]
    pub slug: String,
    pub sso_provider: Option<String>,
    /// The deployment operator that created the organisation.
    #[serde(default)]
    pub created_by: Option<Uuid>,
    /// True for the seeded demonstration organisation. Demo sessions are scoped
    /// to it and a demo reset only replaces its rows (FR-011, SC-014).
    #[serde(default)]
    pub is_demo: bool,
    #[serde(default)]
    pub created_at: DateTime<Utc>,
}

/// Derive the stable URL slug for an organisation name: lowercase ASCII
/// alphanumerics joined by single hyphens, trimmed and capped at 48 characters.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    if out.len() > 48 {
        out.truncate(48);
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        "org".to_string()
    } else {
        out
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrgRole {
    Admin,
    Member,
}

impl std::fmt::Display for OrgRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrgRole::Admin => write!(f, "org_admin"),
            OrgRole::Member => write!(f, "org_member"),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvitationKind {
    ProjectInvite,
    OrgAdminSetup,
}

impl std::fmt::Display for InvitationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InvitationKind::ProjectInvite => write!(f, "project_invite"),
            InvitationKind::OrgAdminSetup => write!(f, "org_admin_setup"),
        }
    }
}

/// A one-time invitation. Only the SHA-256 of the token is stored.
#[derive(Clone, Debug)]
pub struct Invitation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub kind: InvitationKind,
    pub project_id: Option<Uuid>,
    pub email: String,
    pub role: Option<Role>,
    /// Kept alongside the `invitations_by_token` key for PostgreSQL parity.
    #[allow(dead_code)]
    pub token_hash: String,
    #[allow(dead_code)]
    pub created_by: Uuid,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub accepted_user_id: Option<Uuid>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl Invitation {
    pub fn is_pending(&self) -> bool {
        self.accepted_at.is_none() && self.revoked_at.is_none() && self.expires_at > Utc::now()
    }
}

/// A single-use recovery code; only its SHA-256 is stored.
#[derive(Clone, Debug)]
pub struct RecoveryCode {
    #[allow(dead_code)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub code_hash: String,
    #[allow(dead_code)]
    pub created_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

fn default_confidence_threshold() -> f64 {
    0.5
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub model: String,
    /// Suggestions below this confidence are discarded.
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    /// The numeric id a Sentry DSN path points at, e.g. `/api/1/envelope/`.
    pub dsn_id: String,
    pub slug: String,
    pub name: String,
    pub org_id: Uuid,
    pub retention_days: u32,
    pub model: Option<ModelConfig>,
    /// Authoritative suggestion threshold for the project; new projects use
    /// `SCOUT_CONFIDENCE_THRESHOLD` and settings updates carry it through.
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
    /// Set when an owner soft-deletes the project. Rows, objects, and logs stay
    /// and remain subject to retention.
    #[serde(default)]
    pub deleted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub deleted_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IngestKey {
    pub id: Uuid,
    pub project_id: Uuid,
    pub label: String,
    /// SHA-256 of the plaintext key. Never serialised; the plaintext is shown
    /// once at creation/rotation and is not stored.
    #[serde(default, skip_serializing)]
    pub key_hash: String,
    /// First characters of the plaintext key, for identification in the UI.
    pub key_prefix: String,
    pub created_at: DateTime<Utc>,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    /// Argon2id PHC string. Never serialised: API responses must not expose it.
    #[serde(default, skip_serializing)]
    pub password_hash: Option<String>,
    /// The single deployment-level principal seeded from the environment.
    pub is_operator: bool,
    /// A disabled account cannot sign in or start new sessions.
    #[serde(default)]
    pub disabled_at: Option<DateTime<Utc>>,
}

/// An authenticated session. Only the SHA-256 of the opaque token is stored, and
/// the key in `Store::sessions` is that hash.
#[derive(Clone, Debug)]
pub struct Session {
    pub user_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

/// A long-lived API token for the Sentry-compatible surface. Only the SHA-256 of
/// the plaintext is stored; permissions come from the owner's live roles.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub label: String,
    #[serde(default, skip_serializing)]
    pub token_hash: String,
    pub token_prefix: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
}

impl ApiToken {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Viewer,
    Triager,
    Admin,
    Owner,
}

impl Role {
    pub fn can_triage(self) -> bool {
        self >= Role::Triager
    }

    pub fn can_manage(self) -> bool {
        self >= Role::Admin
    }

    pub fn is_owner(self) -> bool {
        self == Role::Owner
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Role::Viewer => "viewer",
            Role::Triager => "triager",
            Role::Admin => "admin",
            Role::Owner => "owner",
        };
        write!(f, "{s}")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StackFrame {
    pub filename: Option<String>,
    pub function: Option<String>,
    pub lineno: Option<i64>,
    pub colno: Option<i64>,
    pub in_app: Option<bool>,
    pub context_line: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Breadcrumb {
    pub timestamp: Option<String>,
    pub category: Option<String>,
    pub level: Option<String>,
    pub message: Option<String>,
    pub data: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub category: String,
    pub confidence: f64,
    pub source: String,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CategorisationDecision {
    pub category: String,
    pub by: String,
    pub at: DateTime<Utc>,
    pub overridden: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    #[default]
    Open,
    Resolved,
    Ignored,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateChange {
    pub state: IssueState,
    pub by: String,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Issue {
    pub id: Uuid,
    pub project_id: Uuid,
    /// Per-project monotonic sequence assigned at creation. Immutable and unique
    /// within the project; used for `shortId`.
    #[serde(default)]
    pub number: u64,
    pub fingerprint: String,
    pub title: String,
    pub culprit: Option<String>,
    pub level: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub count: u64,
    pub state: IssueState,
    pub release: Option<String>,
    pub environment: Option<String>,
    pub tags: Value,
    pub frames: Vec<StackFrame>,
    pub breadcrumbs: Vec<Breadcrumb>,
    pub suggestion: Option<Suggestion>,
    pub decision: Option<CategorisationDecision>,
    /// Set the first time a member opens the issue detail. A late classifier
    /// answer is discarded once this is set (FR-019).
    #[serde(default)]
    pub first_presented_at: Option<DateTime<Utc>>,
    pub state_history: Vec<StateChange>,
    pub last_event_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    pub size: u64,
    /// Write-once object key, set once the bytes are persisted.
    pub object_key: Option<String>,
    /// Parsed bytes; cleared after they are written to object storage.
    pub data: Vec<u8>,
}

impl Attachment {
    pub fn new(filename: String, content_type: String, data: Vec<u8>) -> Self {
        let size = data.len() as u64;
        Self {
            filename,
            content_type,
            size,
            object_key: None,
            data,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub id: Uuid,
    pub sentry_event_id: String,
    pub project_id: Uuid,
    pub issue_id: Option<Uuid>,
    pub received_at: DateTime<Utc>,
    /// Write-once object key holding the exact (decompressed) bytes received.
    /// Null once retention has removed the payload.
    pub raw_object_key: Option<String>,
    pub raw_size: u64,
    pub raw_expired_at: Option<DateTime<Utc>>,
    /// Historical attachment count; attachments themselves expire with the raw
    /// payload.
    pub attachment_count: usize,
    pub event: Value,
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EventMeta {
    pub id: Uuid,
    pub sentry_event_id: String,
    pub received_at: DateTime<Utc>,
    pub size: usize,
    pub attachment_count: usize,
}

impl From<&Event> for EventMeta {
    fn from(e: &Event) -> Self {
        EventMeta {
            id: e.id,
            sentry_event_id: e.sentry_event_id.clone(),
            received_at: e.received_at,
            size: e.raw_size as usize,
            attachment_count: e.attachment_count,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogLine {
    pub id: Uuid,
    pub project_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub message: String,
    pub source: String,
    pub attributes: Value,
    pub received_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub actor: String,
    pub action: String,
    pub target: Option<String>,
    /// Structured context for the action (for example the previous role).
    #[serde(default)]
    pub metadata: Value,
    pub at: DateTime<Utc>,
}

/// An organisation's optional OpenID Connect connection (FR-018).
///
/// `client_secret` must be replayed to the provider during the token exchange, so
/// it cannot be hashed. It is skipped from serialization and redacted from
/// `Debug` output; it must never appear in a response or a log line.
#[derive(Clone, Serialize, Deserialize)]
pub struct OidcConnection {
    pub org_id: Uuid,
    pub issuer: String,
    pub client_id: String,
    #[serde(default, skip_serializing)]
    pub client_secret: String,
    pub scopes: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for OidcConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcConnection")
            .field("org_id", &self.org_id)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// One in-progress OIDC sign-in. Single use, 10-minute TTL, never exposed.
#[derive(Clone, Serialize, Deserialize)]
pub struct OidcLoginFlow {
    pub state: String,
    pub org_id: Uuid,
    pub nonce: String,
    pub pkce_verifier: String,
    pub return_to: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for OidcLoginFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcLoginFlow")
            .field("state", &"<redacted>")
            .field("org_id", &self.org_id)
            .field("nonce", &"<redacted>")
            .field("pkce_verifier", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}
