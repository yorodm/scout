use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Organisation {
    pub id: Uuid,
    pub name: String,
    pub sso_provider: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub model: String,
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
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IngestKey {
    pub id: Uuid,
    pub project_id: Uuid,
    pub label: String,
    pub key: String,
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
}

/// An authenticated session. Only the SHA-256 of the opaque token is stored, and
/// the key in `Store::sessions` is that hash.
#[derive(Clone, Debug)]
pub struct Session {
    pub user_id: Uuid,
    pub expires_at: DateTime<Utc>,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub state_history: Vec<StateChange>,
    pub last_event_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
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
            attachment_count: e.attachments.len(),
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
    pub at: DateTime<Utc>,
}
