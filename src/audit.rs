use crate::models::AuditEntry;
use crate::store::Store;
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

/// Record a privileged action. The constitution requires every administrative
/// action to be auditable, so callers pass the actor and a stable action name.
pub fn record(
    store: &mut Store,
    org_id: Option<Uuid>,
    project_id: Option<Uuid>,
    actor: &str,
    action: &str,
    target: Option<String>,
) {
    record_with(store, org_id, project_id, actor, action, target, json!({}));
}

/// Record an action with structured context (for example the previous role).
pub fn record_with(
    store: &mut Store,
    org_id: Option<Uuid>,
    project_id: Option<Uuid>,
    actor: &str,
    action: &str,
    target: Option<String>,
    metadata: Value,
) {
    store.audit.push(AuditEntry {
        id: Uuid::new_v4(),
        org_id,
        project_id,
        actor: actor.to_string(),
        action: action.to_string(),
        target,
        metadata,
        at: Utc::now(),
    });
}
