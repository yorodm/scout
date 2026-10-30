use crate::models::{Breadcrumb, StackFrame};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct Derived {
    pub fingerprint: String,
    pub title: String,
    pub message: String,
    pub culprit: Option<String>,
    pub frames: Vec<StackFrame>,
    pub breadcrumbs: Vec<Breadcrumb>,
    pub level: String,
    pub release: Option<String>,
    pub environment: Option<String>,
    pub tags: Value,
    pub exception_type: Option<String>,
}

/// Derive everything the issue list and detail view need from a raw Sentry event.
/// This is deliberately deterministic: the same stored event always yields the
/// same fingerprint, so grouping can be recomputed from stored payloads.
pub fn derive(event: &Value, project_id: Uuid) -> Derived {
    let chain = exception_chain(event);
    let frames = extract_frames(&chain);
    let breadcrumbs = extract_breadcrumbs(event);
    let tags = extract_tags(event);
    let message = extract_message(event, &chain);
    let title = extract_title(&chain, &message);
    let exception_type = chain
        .first()
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let culprit = frames
        .iter()
        .find(|f| f.in_app == Some(true))
        .or_else(|| frames.first())
        .map(describe_frame);

    let fingerprint = fingerprint(event, project_id, &chain, &frames, &message);

    Derived {
        fingerprint,
        title,
        message,
        culprit,
        frames,
        breadcrumbs,
        level: event
            .get("level")
            .and_then(Value::as_str)
            .unwrap_or("error")
            .to_string(),
        release: event
            .get("release")
            .and_then(Value::as_str)
            .map(str::to_string),
        environment: event
            .get("environment")
            .and_then(Value::as_str)
            .map(str::to_string),
        tags,
        exception_type,
    }
}

fn exception_chain(event: &Value) -> Vec<Value> {
    event
        .pointer("/exception/values")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn extract_message(event: &Value, chain: &[Value]) -> String {
    if let Some(m) = event.get("message") {
        if let Some(s) = m.as_str() {
            if !s.is_empty() {
                return s.to_string();
            }
        }
        if let Some(s) = m.get("formatted").and_then(Value::as_str) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    if let Some(s) = event.pointer("/logentry/formatted").and_then(Value::as_str) {
        if !s.is_empty() {
            return s.to_string();
        }
    }
    if let Some(ex) = chain.first() {
        let ty = ex.get("type").and_then(Value::as_str).unwrap_or("Error");
        let val = ex.get("value").and_then(Value::as_str).unwrap_or("unknown");
        return format!("{ty}: {val}");
    }
    "Unknown error".to_string()
}

fn extract_title(chain: &[Value], message: &str) -> String {
    if let Some(ex) = chain.first() {
        let ty = ex.get("type").and_then(Value::as_str);
        let val = ex.get("value").and_then(Value::as_str);
        match (ty, val) {
            (Some(t), Some(v)) if !v.is_empty() => return format!("{t}: {v}"),
            (Some(t), _) => return t.to_string(),
            _ => {}
        }
    }
    message.to_string()
}

/// Sentry stores frames oldest-first; the UI shows the crash site first.
fn extract_frames(chain: &[Value]) -> Vec<StackFrame> {
    let mut frames = Vec::new();
    for exception in chain {
        if let Some(list) = exception
            .pointer("/stacktrace/frames")
            .and_then(Value::as_array)
        {
            for f in list {
                frames.push(StackFrame {
                    filename: f
                        .get("filename")
                        .or_else(|| f.get("abs_path"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    function: f
                        .get("function")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    lineno: f.get("lineno").and_then(Value::as_i64),
                    colno: f.get("colno").and_then(Value::as_i64),
                    in_app: f.get("in_app").and_then(Value::as_bool),
                    context_line: f
                        .get("context_line")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
        }
    }
    frames.reverse();
    frames.truncate(40);
    frames
}

fn describe_frame(f: &StackFrame) -> String {
    let function = f.function.as_deref().unwrap_or("<anonymous>");
    match (&f.filename, f.lineno) {
        (Some(file), Some(line)) => format!("{function} at {file}:{line}"),
        (Some(file), None) => format!("{function} at {file}"),
        (None, Some(line)) => format!("{function} at line {line}"),
        (None, None) => function.to_string(),
    }
}

fn extract_breadcrumbs(event: &Value) -> Vec<Breadcrumb> {
    let Some(list) = event
        .pointer("/breadcrumbs/values")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut out: Vec<Breadcrumb> = list
        .iter()
        .map(|b| Breadcrumb {
            timestamp: b.get("timestamp").map(value_to_string),
            category: b
                .get("category")
                .and_then(Value::as_str)
                .map(str::to_string),
            level: b.get("level").and_then(Value::as_str).map(str::to_string),
            message: b
                .get("message")
                .map(value_to_string)
                .filter(|s| !s.is_empty()),
            data: b.get("data").cloned(),
        })
        .collect();
    let keep = out.len().saturating_sub(20);
    out.drain(..keep);
    out
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn extract_tags(event: &Value) -> Value {
    let Some(tags) = event.get("tags") else {
        return json!({});
    };
    if let Some(obj) = tags.as_object() {
        return Value::Object(obj.clone());
    }
    if let Some(list) = tags.as_array() {
        let mut map = serde_json::Map::new();
        for pair in list {
            if let Some(arr) = pair.as_array() {
                if arr.len() == 2 {
                    if let Some(k) = arr[0].as_str() {
                        map.insert(k.to_string(), arr[1].clone());
                    }
                }
            }
        }
        return Value::Object(map);
    }
    json!({})
}

fn fingerprint(
    event: &Value,
    project_id: Uuid,
    chain: &[Value],
    frames: &[StackFrame],
    message: &str,
) -> String {
    let mut material = String::new();

    let explicit: Vec<String> = event
        .get("fingerprint")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if !explicit.is_empty() {
        material.push_str(&explicit.join("\u{1f}"));
    } else {
        for ex in chain {
            let ty = ex.get("type").and_then(Value::as_str).unwrap_or("");
            let val = ex.get("value").and_then(Value::as_str).unwrap_or("");
            material.push_str(ty);
            material.push('\u{1f}');
            material.push_str(val);
            material.push('\u{1e}');
        }
        let mut used = 0;
        for frame in frames {
            if used >= 5 {
                break;
            }
            if frame.in_app == Some(false) {
                continue;
            }
            material.push_str(frame.filename.as_deref().unwrap_or(""));
            material.push('\u{1f}');
            material.push_str(frame.function.as_deref().unwrap_or(""));
            material.push('\u{1f}');
            material.push_str(&frame.lineno.map(|n| n.to_string()).unwrap_or_default());
            material.push('\u{1e}');
            used += 1;
        }
    }

    if material.is_empty() {
        material = normalize(message);
    }

    let salted = format!("{project_id}\u{1d}{material}");
    let digest = Sha256::digest(salted.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn normalize(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut last_digit = false;
    let mut last_space = false;
    for ch in message.to_lowercase().chars() {
        if ch.is_ascii_digit() {
            if !last_digit {
                out.push('#');
            }
            last_digit = true;
            last_space = false;
        } else if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
            last_digit = false;
        } else {
            out.push(ch);
            last_digit = false;
            last_space = false;
        }
    }
    out.trim().to_string()
}

/// The compact summary that is allowed to cross the model boundary (max 2 KB).
pub fn summary(derived: &Derived, event: &Value, received_by: &str) -> Value {
    let compact = serde_json::to_string(event).unwrap_or_default();
    let sample: String = compact.chars().take(2048).collect();
    json!({
        "kind": "sentry_event",
        "message": derived.message,
        "title": derived.title,
        "exception_type": derived.exception_type,
        "level": derived.level,
        "release": derived.release,
        "received_by": received_by,
        "payload_sample": sample,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn event(msg: &str) -> Value {
        json!({
            "event_id": "a".repeat(32),
            "message": msg,
            "exception": {
                "values": [{
                    "type": "TypeError",
                    "value": msg,
                    "stacktrace": { "frames": [
                        {"filename": "src/app.ts", "function": "main", "lineno": 2, "in_app": true},
                        {"filename": "src/checkout.ts", "function": "pay", "lineno": 42, "in_app": true}
                    ]}
                }]
            }
        })
    }

    #[test]
    fn identical_events_share_a_fingerprint() {
        let project = Uuid::new_v4();
        let a = derive(&event("boom"), project);
        let b = derive(&event("boom"), project);
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.title, "TypeError: boom");
        assert_eq!(a.frames.len(), 2);
        // crash site first
        assert_eq!(a.frames[0].function.as_deref(), Some("pay"));
    }

    #[test]
    fn different_messages_produce_different_fingerprints() {
        let project = Uuid::new_v4();
        assert_ne!(
            derive(&event("boom"), project).fingerprint,
            derive(&event("bang"), project).fingerprint
        );
    }

    #[test]
    fn explicit_fingerprint_is_honoured() {
        let project = Uuid::new_v4();
        let mut e = event("boom");
        e["fingerprint"] = json!(["custom", "group"]);
        let mut e2 = event("something else");
        e2["fingerprint"] = json!(["custom", "group"]);
        assert_eq!(
            derive(&e, project).fingerprint,
            derive(&e2, project).fingerprint
        );
    }

    #[test]
    fn numbers_are_normalised_in_message_fallback() {
        assert_eq!(
            normalize("Timeout after 3000ms on port 8080"),
            "timeout after #ms on port #"
        );
    }
}
