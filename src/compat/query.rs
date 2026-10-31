//! Sentry search-grammar subset, sort modes, and keyset positioning.

use crate::compat::cursor::Cursor;
use crate::compat::projection::tag_string;
use crate::models::{Issue, IssueState};
use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Date,
    New,
    Freq,
    User,
    Trends,
    Recommended,
}

impl Sort {
    pub fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or("date") {
            "new" => Sort::New,
            "freq" => Sort::Freq,
            "user" => Sort::User,
            "trends" => Sort::Trends,
            "recommended" => Sort::Recommended,
            _ => Sort::Date,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Sort::Date => "date",
            Sort::New => "new",
            Sort::Freq => "freq",
            Sort::User => "user",
            Sort::Trends => "trends",
            Sort::Recommended => "recommended",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub states: Option<Vec<IssueState>>,
    pub level: Option<String>,
    pub release: Option<String>,
    pub environment: Option<String>,
    pub tags: Vec<(String, String)>,
    pub text: Vec<String>,
    pub ids: Vec<Uuid>,
    pub hashes: Vec<String>,
}

const UNSUPPORTED_OPERATORS: &[&str] = &[
    "assigned",
    "bookmarks",
    "first-release",
    "last-release",
    "has",
    "age",
    "timestamp",
    "timesSeen",
    "user",
    "device",
    "os",
    "browser",
    "url",
    "transaction",
    "tag",
    "geo",
    "session.status",
];

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in input.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Parse the supported subset. Unknown `is:` values and unsupported operators are
/// ignored, never rejected.
pub fn parse_filter(query: Option<&str>, ids: &[String], hashes: &[String]) -> Filter {
    let mut filter = Filter {
        ids: ids
            .iter()
            .filter_map(|id| Uuid::parse_str(id).ok())
            .collect(),
        hashes: hashes.to_vec(),
        ..Filter::default()
    };
    let mut states: Vec<IssueState> = Vec::new();
    let mut saw_is = false;

    if let Some(text) = query {
        for token in tokenize(text) {
            let Some((key, value)) = token.split_once(':') else {
                let needle = token.trim().to_ascii_lowercase();
                if !needle.is_empty() {
                    filter.text.push(needle);
                }
                continue;
            };
            match key {
                "is" => match value {
                    "unresolved" => {
                        saw_is = true;
                        states.push(IssueState::Open);
                    }
                    "resolved" => {
                        saw_is = true;
                        states.push(IssueState::Resolved);
                    }
                    "ignored" => {
                        saw_is = true;
                        states.push(IssueState::Ignored);
                    }
                    _ => {}
                },
                "level" => filter.level = Some(value.to_string()),
                "release" => filter.release = Some(value.to_string()),
                "environment" => filter.environment = Some(value.to_string()),
                other if UNSUPPORTED_OPERATORS.contains(&other) => {}
                _ => filter.tags.push((key.to_string(), value.to_string())),
            }
        }
    }

    filter.states = Some(if saw_is {
        states
    } else {
        vec![IssueState::Open]
    });
    filter
}

pub fn matches(issue: &Issue, filter: &Filter) -> bool {
    if !filter.ids.is_empty() && !filter.ids.contains(&issue.id) {
        return false;
    }
    if !filter.hashes.is_empty() && !filter.hashes.contains(&issue.fingerprint) {
        return false;
    }
    if let Some(states) = &filter.states {
        if !states.contains(&issue.state) {
            return false;
        }
    }
    if let Some(level) = &filter.level {
        if !issue.level.eq_ignore_ascii_case(level) {
            return false;
        }
    }
    if let Some(release) = &filter.release {
        if issue.release.as_deref() != Some(release.as_str()) {
            return false;
        }
    }
    if let Some(environment) = &filter.environment {
        if issue.environment.as_deref() != Some(environment.as_str()) {
            return false;
        }
    }
    for (key, value) in &filter.tags {
        match issue.tags.get(key).and_then(tag_string) {
            Some(actual) if actual == *value => {}
            _ => return false,
        }
    }
    if !filter.text.is_empty() {
        let title = issue.title.to_ascii_lowercase();
        let culprit = issue.culprit.as_deref().unwrap_or("").to_ascii_lowercase();
        let fingerprint = issue.fingerprint.to_ascii_lowercase();
        for needle in &filter.text {
            if !title.contains(needle) && !culprit.contains(needle) && !fingerprint.contains(needle)
            {
                return false;
            }
        }
    }
    true
}

fn millis(at: DateTime<Utc>) -> i64 {
    at.timestamp_millis()
}

/// Numeric sort key: milliseconds for time-based sorts, event count otherwise.
pub fn sort_value(sort: Sort, issue: &Issue) -> i64 {
    match sort {
        Sort::Date | Sort::Trends | Sort::Recommended => millis(issue.last_seen),
        Sort::New => millis(issue.first_seen),
        Sort::Freq | Sort::User => issue.count as i64,
    }
}

/// Descending by sort value, ascending by id as a deterministic tiebreak.
pub fn compare(sort: Sort, a: &Issue, b: &Issue) -> Ordering {
    sort_value(sort, b)
        .cmp(&sort_value(sort, a))
        .then_with(|| a.id.cmp(&b.id))
}

/// True when the issue comes strictly after the cursor in the current ordering.
pub fn is_after_cursor(issue: &Issue, sort: Sort, cursor: &Cursor) -> bool {
    let Ok(cursor_value) = cursor.k.parse::<i64>() else {
        return false;
    };
    let cursor_id = Uuid::parse_str(&cursor.i).unwrap_or(Uuid::nil());
    let value = sort_value(sort, issue);
    value < cursor_value || (value == cursor_value && issue.id > cursor_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use serde_json::json;

    fn issue(number: u64, state: IssueState, count: u64) -> Issue {
        Issue {
            id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            number,
            fingerprint: format!("fp-{number}"),
            title: "Checkout failed".into(),
            culprit: Some("app/checkout.js".into()),
            level: "error".into(),
            first_seen: Utc::now() - Duration::days(5),
            last_seen: Utc::now() - Duration::hours(number as i64),
            count,
            state,
            release: Some("web@1.0.0".into()),
            environment: Some("production".into()),
            tags: json!({ "release": "web@1.0.0", "region": "eu" }),
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
    fn defaults_to_unresolved() {
        let filter = parse_filter(None, &[], &[]);
        assert_eq!(filter.states, Some(vec![IssueState::Open]));
        assert!(matches(&issue(1, IssueState::Open, 1), &filter));
        assert!(!matches(&issue(2, IssueState::Resolved, 1), &filter));
    }

    #[test]
    fn default_applies_with_ids_and_hashes() {
        let filter = parse_filter(None, &["not-a-uuid".into()], &["fp-1".into()]);
        assert_eq!(filter.states, Some(vec![IssueState::Open]));
        assert!(matches(&issue(1, IssueState::Open, 1), &filter));
    }

    #[test]
    fn supports_supported_operators_and_ignores_the_rest() {
        let issue = issue(1, IssueState::Resolved, 3);
        let filter = parse_filter(
            Some("is:resolved level:error release:web@1.0.0 environment:production region:eu assigned:me"),
            &[],
            &[],
        );
        assert!(matches(&issue, &filter), "assigned:me must be ignored");
        let unknown_is = parse_filter(Some("is:assigned"), &[], &[]);
        assert_eq!(unknown_is.states, Some(vec![IssueState::Open]));
    }

    #[test]
    fn free_text_searches_title_culprit_and_fingerprint() {
        let issue = issue(1, IssueState::Open, 1);
        assert!(matches(&issue, &parse_filter(Some("checkout"), &[], &[])));
        assert!(matches(&issue, &parse_filter(Some("fp-1"), &[], &[])));
        assert!(!matches(&issue, &parse_filter(Some("billing"), &[], &[])));
    }

    #[test]
    fn sort_orders_and_cursor_position() {
        let first = issue(1, IssueState::Open, 5);
        let second = issue(2, IssueState::Open, 9);
        assert_eq!(compare(Sort::Freq, &first, &second), Ordering::Greater);
        assert_eq!(compare(Sort::Date, &first, &second), Ordering::Less);

        let cursor = Cursor {
            v: 1,
            q: "q".into(),
            s: "freq".into(),
            k: sort_value(Sort::Freq, &second).to_string(),
            i: second.id.to_string(),
        };
        assert!(is_after_cursor(&first, Sort::Freq, &cursor));
        assert!(!is_after_cursor(&second, Sort::Freq, &cursor));
    }
}
