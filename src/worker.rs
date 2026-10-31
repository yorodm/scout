use crate::models::{Issue, IssueState, Project, StateChange};
use crate::store::Store;
use std::sync::Arc;
use tokio::sync::{mpsc::UnboundedReceiver, RwLock};
use uuid::Uuid;

pub enum Job {
    GroupEvent {
        job_id: Uuid,
        project_id: Uuid,
        event_id: Uuid,
    },
}

/// One in-process worker, mirroring the "claim a job, group, then classify"
/// pipeline from architecture.md. Ingest never waits for it.
pub fn spawn(store: Arc<RwLock<Store>>, mut rx: UnboundedReceiver<Job>, http: reqwest::Client) {
    actix_web::rt::spawn(async move {
        while let Some(job) = rx.recv().await {
            match job {
                Job::GroupEvent {
                    job_id,
                    project_id,
                    event_id,
                } => process_event(&store, &http, job_id, project_id, event_id).await,
            }
        }
    });
}

async fn process_event(
    store: &Arc<RwLock<Store>>,
    http: &reqwest::Client,
    job_id: Uuid,
    project_id: Uuid,
    event_id: Uuid,
) {
    let started = std::time::Instant::now();
    let Some((issue_id, derived, project, event, needs_suggestion)) =
        group(store, project_id, event_id).await
    else {
        tracing::warn!(job_id = %job_id, project_id = %project_id, event_id = %event_id, "grouping skipped: event not found");
        return;
    };
    tracing::debug!(
        job_id = %job_id,
        project_id = %project_id,
        event_id = %event_id,
        issue_id = %issue_id,
        elapsed_us = started.elapsed().as_micros() as u64,
        "event grouped"
    );

    // No model configured, a suggestion already exists, or a human already
    // decided: nothing to ask the model for.
    if project.model.is_none() || !needs_suggestion {
        return;
    }

    let received_by = format!("/api/{}/envelope/", project.dsn_id);
    let suggestion =
        crate::classifier::suggest(http, &project, &derived, &event, &received_by).await;
    tracing::debug!(
        job_id = %job_id,
        project_id = %project_id,
        issue_id = %issue_id,
        outcome = if suggestion.is_some() { "suggested" } else { "no_suggestion" },
        elapsed_ms = started.elapsed().as_millis() as u64,
        "classifier finished"
    );
    let Some(suggestion) = suggestion else {
        return;
    };

    let mut store = store.write().await;
    if let Some(issue) = store.issues.get_mut(&issue_id) {
        // FR-019: a late suggestion must not rewrite a human decision.
        if issue.decision.is_none() {
            issue.suggestion = Some(suggestion);
        }
    }
}

#[allow(clippy::type_complexity)]
async fn group(
    store: &Arc<RwLock<Store>>,
    project_id: Uuid,
    event_id: Uuid,
) -> Option<(
    Uuid,
    crate::grouping::Derived,
    Project,
    serde_json::Value,
    bool,
)> {
    let mut store = store.write().await;
    let event = store.events.get(&event_id)?.clone();
    let project = store.projects.get(&project_id)?.clone();
    let derived = crate::grouping::derive(&event.event, project_id);
    let now = event.received_at;

    let issue_id = match store
        .issues_by_fingerprint
        .get(&(project_id, derived.fingerprint.clone()))
        .copied()
    {
        Some(id) => {
            if let Some(issue) = store.issues.get_mut(&id) {
                issue.count = issue.count.saturating_add(1);
                issue.last_seen = issue.last_seen.max(now);
                issue.first_seen = issue.first_seen.min(now);
                issue.last_event_id = event.id;
                issue.title = derived.title.clone();
                if derived.culprit.is_some() {
                    issue.culprit = derived.culprit.clone();
                }
                issue.level = derived.level.clone();
                if derived.release.is_some() {
                    issue.release = derived.release.clone();
                }
                if derived.environment.is_some() {
                    issue.environment = derived.environment.clone();
                }
                // Later occurrences often carry less context; never let an
                // emptier event erase what an earlier one showed.
                if derived
                    .tags
                    .as_object()
                    .is_some_and(|tags| !tags.is_empty())
                {
                    issue.tags = derived.tags.clone();
                }
                if !derived.frames.is_empty() {
                    issue.frames = derived.frames.clone();
                }
                if !derived.breadcrumbs.is_empty() {
                    issue.breadcrumbs = derived.breadcrumbs.clone();
                }
            }
            id
        }
        None => {
            let id = Uuid::new_v4();
            let issue = Issue {
                id,
                project_id,
                fingerprint: derived.fingerprint.clone(),
                title: derived.title.clone(),
                culprit: derived.culprit.clone(),
                level: derived.level.clone(),
                first_seen: now,
                last_seen: now,
                count: 1,
                state: IssueState::Open,
                release: derived.release.clone(),
                environment: derived.environment.clone(),
                tags: derived.tags.clone(),
                frames: derived.frames.clone(),
                breadcrumbs: derived.breadcrumbs.clone(),
                suggestion: None,
                decision: None,
                state_history: vec![StateChange {
                    state: IssueState::Open,
                    by: "system".into(),
                    at: now,
                }],
                last_event_id: event.id,
            };
            store.issues.insert(id, issue);
            store
                .issues_by_fingerprint
                .insert((project_id, derived.fingerprint.clone()), id);
            id
        }
    };

    if let Some(stored) = store.events.get_mut(&event_id) {
        stored.issue_id = Some(issue_id);
    }
    let needs_suggestion = store
        .issues
        .get(&issue_id)
        .map(|i| i.suggestion.is_none() && i.decision.is_none())
        .unwrap_or(false);

    Some((
        issue_id,
        derived,
        project,
        event.event.clone(),
        needs_suggestion,
    ))
}
