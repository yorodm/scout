use crate::grouping::Derived;
use crate::models::{Project, Suggestion};
use actix_web::{web, HttpResponse};
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Lets the demo UI simulate a classifier outage without touching config.
static OUTAGE: AtomicBool = AtomicBool::new(false);

/// Ask a project's Jev-compatible endpoint to suggest a category.
///
/// This is best-effort by design: a bounded timeout, at most one retry, and any
/// failure simply yields no suggestion. It never mutates stored data and never
/// sits on the ingest path.
pub async fn suggest(
    http: &reqwest::Client,
    project: &Project,
    derived: &Derived,
    event: &Value,
    received_by: &str,
) -> Option<Suggestion> {
    let config = project.model.as_ref()?;
    let url = format!("{}/v1/systemone", config.endpoint.trim_end_matches('/'));
    let summary = crate::grouping::summary(derived, event, received_by);
    let request = json!({
        "model": config.model,
        "state": summary,
        "questions": {
            "category": {
                "type": "choice",
                "instructions": "Classify this incoming telemetry item for a human triager.",
                "criteria": {
                    "issue": "A discrete error or exception that should be grouped for triage.",
                    "log": "A log line or operational message.",
                    "raw_event": "An opaque payload that should be kept but not grouped."
                }
            }
        }
    });

    for _attempt in 0..2 {
        let mut builder = http.post(&url).json(&request);
        if let Some(key) = &config.api_key {
            builder = builder.bearer_auth(key);
        }
        let response = match tokio::time::timeout(Duration::from_millis(800), builder.send()).await
        {
            Ok(Ok(response)) => response,
            _ => continue,
        };
        if !response.status().is_success() {
            // A rejected request is not worth retrying; only 5xx and transport
            // failures get the single retry.
            if response.status().is_client_error() {
                return None;
            }
            continue;
        }
        let body = match response.json::<Value>().await {
            Ok(body) => body,
            Err(_) => return None,
        };
        let raw = body
            .get("choice")
            .or_else(|| body.get("answer"))
            .or_else(|| body.get("category"))
            .and_then(Value::as_str)?;
        let category = normalize_category(raw)?;
        let confidence = body
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.5)
            .clamp(0.0, 1.0);
        // Below the project's threshold is treated exactly like no answer.
        if confidence < project.confidence_threshold {
            return None;
        }
        return Some(Suggestion {
            category,
            confidence,
            source: config.model.clone(),
            at: Utc::now(),
        });
    }
    None
}

fn normalize_category(raw: &str) -> Option<String> {
    match raw
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-'], "_")
        .as_str()
    {
        "issue" | "error" => Some("issue".into()),
        "log" | "log_line" => Some("log".into()),
        "raw_event" | "raw" | "event" => Some("raw_event".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_known_categories_and_rejects_the_rest() {
        assert_eq!(normalize_category("error").as_deref(), Some("issue"));
        assert_eq!(normalize_category(" Log-Line ").as_deref(), Some("log"));
        assert_eq!(normalize_category("raw").as_deref(), Some("raw_event"));
        assert_eq!(normalize_category("billing"), None);
        assert_eq!(normalize_category(""), None);
    }
}

/// A deliberately small Jev-compatible endpoint so the prototype demonstrates
/// the full suggestion -> confirmation loop without an external model.
pub async fn mock_systemone(body: web::Json<Value>) -> HttpResponse {
    if OUTAGE.load(Ordering::Relaxed) {
        return HttpResponse::ServiceUnavailable()
            .json(json!({"error": "simulated classifier outage"}));
    }
    let state = body.pointer("/state").cloned().unwrap_or(Value::Null);
    let message = state
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let (choice, confidence) =
        if message.contains("pool") || message.contains("log") || message.contains("audit") {
            ("log", 0.64)
        } else if message.contains("unknown item") || message.contains("raw event") {
            ("raw_event", 0.69)
        } else {
            ("issue", 0.88)
        };
    HttpResponse::Ok().json(json!({
        "choice": choice,
        "confidence": confidence,
        "model": "jev-latest",
        "reason": "prototype heuristic"
    }))
}

#[derive(serde::Deserialize)]
pub struct OutageRequest {
    pub down: bool,
}

pub async fn set_outage(body: web::Json<OutageRequest>) -> HttpResponse {
    OUTAGE.store(body.down, Ordering::Relaxed);
    HttpResponse::Ok().json(json!({ "down": body.down }))
}

pub async fn get_outage() -> HttpResponse {
    HttpResponse::Ok().json(json!({ "down": OUTAGE.load(Ordering::Relaxed) }))
}

pub fn reset_outage() {
    OUTAGE.store(false, Ordering::Relaxed);
}
