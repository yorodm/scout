//! Projection of stored Scout issues into Sentry-shaped objects, plus tag-value
//! derivation. Nothing here mutates data.

use crate::models::{Issue, IssueState, Project};
use chrono::SecondsFormat;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize)]
pub struct SentryProject {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub platform: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SentryTag {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SentryIssue {
    pub id: String,
    pub short_id: String,
    pub title: String,
    pub culprit: String,
    pub level: String,
    pub status: String,
    pub status_details: Value,
    pub count: u64,
    pub user_count: u64,
    pub first_seen: String,
    pub last_seen: String,
    pub metadata: Value,
    pub permalink: String,
    pub project: SentryProject,
    pub tags: Vec<SentryTag>,
    pub annotations: Vec<Value>,
}

pub fn status_str(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "unresolved",
        IssueState::Resolved => "resolved",
        IssueState::Ignored => "ignored",
    }
}

pub fn status_from_str(value: &str) -> Option<IssueState> {
    match value {
        "unresolved" => Some(IssueState::Open),
        "resolved" => Some(IssueState::Resolved),
        "ignored" => Some(IssueState::Ignored),
        _ => None,
    }
}

/// Stable, readable Sentry short id: `PROJECT-<number>`.
pub fn short_id(project: &Project, issue: &Issue) -> String {
    format!("{}-{}", project.slug.to_ascii_uppercase(), issue.number)
}

fn timestamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Stringify a tag value the way Sentry clients expect. Objects, arrays, null,
/// and empty strings are not tag values.
pub fn tag_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

pub fn project_issue(issue: &Issue, project: &Project, self_url: &str) -> SentryIssue {
    let culprit = issue.culprit.clone().unwrap_or_default();
    let mut tags: Vec<SentryTag> = issue
        .tags
        .as_object()
        .map(|tags| {
            let mut entries: Vec<SentryTag> = tags
                .iter()
                .filter_map(|(key, value)| {
                    tag_string(value).map(|value| SentryTag {
                        key: key.clone(),
                        value,
                    })
                })
                .collect();
            entries.sort_by(|a, b| a.key.cmp(&b.key));
            entries
        })
        .unwrap_or_default();
    tags.shrink_to_fit();

    let status_details = match issue.state {
        IssueState::Ignored => json!({ "ignoreCount": 0 }),
        _ => json!({}),
    };

    SentryIssue {
        id: issue.id.to_string(),
        short_id: short_id(project, issue),
        title: issue.title.clone(),
        culprit: culprit.clone(),
        level: issue.level.clone(),
        status: status_str(issue.state).to_string(),
        status_details,
        count: issue.count,
        user_count: 0,
        first_seen: timestamp(issue.first_seen),
        last_seen: timestamp(issue.last_seen),
        metadata: json!({ "title": issue.title, "value": culprit }),
        permalink: format!("{}/#/issues/{}", self_url.trim_end_matches('/'), issue.id),
        project: SentryProject {
            id: project.id.to_string(),
            name: project.name.clone(),
            slug: project.slug.clone(),
            platform: None,
        },
        tags,
        annotations: Vec::new(),
    }
}

/// Distinct, ordered tag values across the supplied issues, filtered by a
/// case-insensitive contains match and capped at 1,000.
pub fn tag_values<'a, I>(issues: I, key: &str, contains: Option<&str>) -> Vec<String>
where
    I: Iterator<Item = &'a Issue>,
{
    let needle = contains
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let mut values: BTreeSet<String> = BTreeSet::new();
    for issue in issues {
        let Some(value) = issue.tags.get(key).and_then(tag_string) else {
            continue;
        };
        if let Some(needle) = &needle {
            if !value.to_ascii_lowercase().contains(needle) {
                continue;
            }
        }
        values.insert(value);
    }
    values.into_iter().take(1_000).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn project() -> Project {
        Project {
            id: Uuid::new_v4(),
            dsn_id: "1".into(),
            slug: "scout-web".into(),
            name: "scout-web".into(),
            org_id: Uuid::new_v4(),
            retention_days: 30,
            model: None,
            confidence_threshold: 0.5,
            deleted_at: None,
            deleted_by: None,
            created_at: Utc::now(),
        }
    }

    fn issue(project: &Project) -> Issue {
        Issue {
            id: Uuid::new_v4(),
            project_id: project.id,
            number: 17,
            fingerprint: "fp".into(),
            title: "TypeError".into(),
            culprit: Some("app/checkout.js".into()),
            level: "error".into(),
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            count: 42,
            state: IssueState::Open,
            release: Some("web@1.0.0".into()),
            environment: Some("production".into()),
            tags: json!({ "release": "web@1.0.0", "empty": "", "count": 3 }),
            frames: Vec::new(),
            breadcrumbs: Vec::new(),
            suggestion: None,
            decision: None,
            first_presented_at: None,
            state_history: Vec::new(),
            last_event_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn maps_status_both_ways() {
        for state in [IssueState::Open, IssueState::Resolved, IssueState::Ignored] {
            let text = status_str(state);
            assert_eq!(status_from_str(text), Some(state));
        }
        assert_eq!(status_from_str("open"), None);
    }

    #[test]
    fn projection_has_contract_fields() {
        let project = project();
        let issue = issue(&project);
        let projected = project_issue(&issue, &project, "https://scout.example/");
        assert_eq!(projected.short_id, "SCOUT-WEB-17");
        assert_eq!(projected.status, "unresolved");
        assert_eq!(projected.user_count, 0);
        assert!(projected.annotations.is_empty());
        assert_eq!(
            projected.permalink,
            format!("https://scout.example/#/issues/{}", issue.id)
        );
        assert!(projected.first_seen.ends_with('Z'));
        assert_eq!(projected.metadata["title"], "TypeError");
        assert_eq!(projected.status_details, json!({}));
        // Empty tag values are omitted, scalars are stringified.
        let keys: Vec<&str> = projected.tags.iter().map(|tag| tag.key.as_str()).collect();
        assert_eq!(keys, vec!["count", "release"]);
    }

    #[test]
    fn ignored_issues_carry_ignore_count() {
        let project = project();
        let mut issue = issue(&project);
        issue.state = IssueState::Ignored;
        let projected = project_issue(&issue, &project, "https://scout.example");
        assert_eq!(projected.status, "ignored");
        assert_eq!(projected.status_details, json!({ "ignoreCount": 0 }));
    }

    #[test]
    fn tag_values_are_distinct_sorted_and_filtered() {
        let project = project();
        let mut first = issue(&project);
        let mut second = issue(&project);
        first.tags = json!({ "release": "web@1.1.0" });
        second.tags = json!({ "release": "web@1.0.0" });
        let values = tag_values([first.clone(), second.clone()].iter(), "release", None);
        assert_eq!(values, vec!["web@1.0.0", "web@1.1.0"]);
        let filtered = tag_values([first, second].iter(), "release", Some("1.1"));
        assert_eq!(filtered, vec!["web@1.1.0"]);
        assert!(tag_values(std::iter::empty(), "release", None).is_empty());
    }
}
