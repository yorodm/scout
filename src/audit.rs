use crate::models::AuditEntry;
use crate::store::Store;
use chrono::Utc;
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
    store.audit.push(AuditEntry {
        id: Uuid::new_v4(),
        org_id,
        project_id,
        actor: actor.to_string(),
        action: action.to_string(),
        target,
        at: Utc::now(),
    });
}
