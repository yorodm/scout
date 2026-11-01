use crate::error::ApiError;
use crate::models::{Attachment, Event, LogLine, Project};
use crate::state::AppState;
use crate::store::event_hex_id;
use crate::worker::Job;
use actix_web::{web, HttpRequest, HttpResponse};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use uuid::Uuid;

/// FR-007: one ingest request (and one attachment) may not exceed 10 MB.
pub const MAX_INGEST_BYTES: usize = 10 * 1024 * 1024;

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.route("/api/{project}/envelope", web::post().to(ingest_envelope))
        .route("/api/{project}/envelope/", web::post().to(ingest_envelope))
        .route("/api/{project}/store", web::post().to(ingest_store))
        .route("/api/{project}/store/", web::post().to(ingest_store))
        .route("/api/{project}/logs", web::post().to(ingest_logs))
        .route("/api/{project}/logs/", web::post().to(ingest_logs));
}

pub async fn ingest_envelope(
    req: HttpRequest,
    path: web::Path<String>,
    query: web::Query<HashMap<String, String>>,
    body: web::Bytes,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let _permit = tracked(&state, acquire_ingest_permit(&state))?;
    tracked(&state, state.ensure_storage())?;
    let project = tracked(
        &state,
        authenticate(&req, path.as_str(), &query, &state).await,
    )?;
    let bytes = tracked(&state, decode_body(body))?;
    let ParsedEnvelope {
        header,
        mut events,
        attachments,
        logs,
        ignored,
    } = tracked(&state, parse_envelope(&bytes).map_err(ApiError::BadRequest))?;

    // Sentry puts the event id in the envelope header as well as the event item;
    // fall back to the header when a single event omits it.
    if let Some(header_id) = header.get("event_id").and_then(Value::as_str) {
        if events.len() == 1 {
            if let Some(object) = events[0].as_object_mut() {
                object.entry("event_id").or_insert_with(|| json!(header_id));
            }
        }
    }

    let event_count = events.len();
    let log_count = logs.len();
    tracing::debug!(
        project_id = %project.id,
        project = %project.slug,
        events = event_count,
        logs = log_count,
        ignored,
        "envelope accepted"
    );

    let event_ids = tracked(
        &state,
        persist_events(&state, &project, events, &attachments, bytes).await,
    )?;
    let log_count = tracked(&state, persist_logs(&state, &project, logs).await)?;
    tracing::debug!(
        project_id = %project.id,
        event_ids = ?event_ids,
        logs = log_count,
        "envelope persisted"
    );

    Ok(HttpResponse::Ok().json(json!({
        "id": event_ids.first(),
        "event_ids": event_ids,
        "logs": log_count,
        "ignored": ignored,
    })))
}

pub async fn ingest_store(
    req: HttpRequest,
    path: web::Path<String>,
    query: web::Query<HashMap<String, String>>,
    body: web::Bytes,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let _permit = tracked(&state, acquire_ingest_permit(&state))?;
    tracked(&state, state.ensure_storage())?;
    let project = tracked(
        &state,
        authenticate(&req, path.as_str(), &query, &state).await,
    )?;
    let bytes = tracked(&state, decode_body(body))?;
    let event: Value = match serde_json::from_slice(&bytes) {
        Ok(event) => event,
        Err(error) => {
            return tracked(
                &state,
                Err(ApiError::BadRequest(format!("invalid event JSON: {error}"))),
            )
        }
    };
    if !event.is_object() {
        return tracked(
            &state,
            Err(ApiError::BadRequest("event must be a JSON object".into())),
        );
    }
    let ids = tracked(
        &state,
        persist_events(&state, &project, vec![event], &[], bytes).await,
    )?;
    tracing::debug!(project_id = %project.id, event_id = ?ids.first(), "event stored");
    Ok(HttpResponse::Ok().json(json!({ "id": ids.first() })))
}

pub async fn ingest_logs(
    req: HttpRequest,
    path: web::Path<String>,
    query: web::Query<HashMap<String, String>>,
    body: web::Bytes,
    state: web::Data<AppState>,
) -> Result<HttpResponse, ApiError> {
    let _permit = tracked(&state, acquire_ingest_permit(&state))?;
    tracked(&state, state.ensure_storage())?;
    let project = tracked(
        &state,
        authenticate(&req, path.as_str(), &query, &state).await,
    )?;
    let bytes = tracked(&state, decode_body(body))?;
    let value = tracked(&state, parse_log_document(&bytes))?;
    let logs = match &value {
        Value::Array(items) => items.clone(),
        Value::Object(obj) => obj
            .get("logs")
            .or_else(|| obj.get("items"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| vec![value.clone()]),
        _ => vec![value.clone()],
    };
    let count = tracked(&state, persist_logs(&state, &project, logs).await)?;
    tracing::debug!(project_id = %project.id, accepted = count, "logs stored");
    Ok(HttpResponse::Ok().json(json!({ "accepted": count })))
}

fn acquire_ingest_permit(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state
        .ingest_gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::TooManyRequests {
            message: "ingest concurrency limit reached; retry shortly".into(),
            retry_after_secs: 1,
        })
}

/// Count an ingest rejection by the response class it produces (FR-025).
fn tracked<T>(state: &AppState, result: Result<T, ApiError>) -> Result<T, ApiError> {
    result.map_err(|error| {
        state.metrics.rejected(&error);
        error
    })
}

async fn authenticate(
    req: &HttpRequest,
    project_ref: &str,
    query: &HashMap<String, String>,
    state: &AppState,
) -> Result<Project, ApiError> {
    // Every key failure returns the same body, so a caller cannot learn whether
    // a key exists, was revoked, or belongs to another project.
    let key_str = extract_ingest_key(req, query).ok_or_else(invalid_key)?;
    let store = state.store.read().await;
    let project = store
        .project_by_identifier(project_ref)
        .cloned()
        .ok_or_else(|| ApiError::NotFound("project not found".into()))?;
    let key_hash = crate::auth::hash_token(&key_str);
    let key = store
        .keys
        .values()
        .find(|key| key.key_hash == key_hash)
        .ok_or_else(invalid_key)?;
    // A soft-deleted project rejects every key exactly like an invalid one.
    if key.revoked || key.project_id != project.id || project.deleted_at.is_some() {
        return Err(invalid_key());
    }
    Ok(project)
}

fn invalid_key() -> ApiError {
    ApiError::Unauthorized("invalid or missing ingest key".into())
}

fn extract_ingest_key(req: &HttpRequest, query: &HashMap<String, String>) -> Option<String> {
    for header in ["x-sentry-auth", "authorization"] {
        if let Some(value) = req.headers().get(header).and_then(|v| v.to_str().ok()) {
            let lower = value.to_ascii_lowercase();
            if lower.starts_with("sentry ") {
                if let Some(key) = parse_sentry_auth(&value[7..]).get("sentry_key") {
                    return Some(key.clone());
                }
            }
        }
    }
    if let Some(key) = query.get("sentry_key") {
        return Some(key.clone());
    }
    req.headers()
        .get("x-scout-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn parse_sentry_auth(value: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for part in value.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
        }
    }
    out
}

/// Actix's `Bytes` extractor already decompresses `Content-Encoding` bodies and
/// enforces `PayloadConfig`. This guard keeps the 10 MB contract explicit for
/// the handlers and covers any body that reaches them uncompressed.
fn decode_body(body: web::Bytes) -> Result<Vec<u8>, ApiError> {
    if body.len() > MAX_INGEST_BYTES {
        return Err(too_large());
    }
    Ok(body.to_vec())
}

fn too_large() -> ApiError {
    ApiError::PayloadTooLarge(format!(
        "payload exceeds the {} byte ingest limit",
        MAX_INGEST_BYTES
    ))
}

async fn persist_events(
    state: &AppState,
    project: &Project,
    events: Vec<Value>,
    attachments: &[Attachment],
    raw: Vec<u8>,
) -> Result<Vec<String>, ApiError> {
    // Durability ordering (FR-005): write the immutable raw payload first, then
    // insert the event + grouping job, and only then acknowledge.
    state.ensure_storage()?;
    let received_at = Utc::now();
    let raw_size = raw.len() as u64;

    // Resolve retried deliveries before writing anything, so a duplicate does
    // not create a second object.
    let mut pending: Vec<(Uuid, String, Value, String)> = Vec::new();
    let mut ids = Vec::new();
    {
        let store = state.store.read().await;
        for event in events {
            let sentry_id = event
                .get("event_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(event_hex_id);
            if store
                .seen_event_ids
                .contains_key(&(project.id, sentry_id.clone()))
            {
                ids.push(sentry_id);
                continue;
            }
            let id = Uuid::new_v4();
            let key = crate::objects::raw_key(project.id, id);
            ids.push(sentry_id.clone());
            pending.push((id, sentry_id, event, key));
        }
    }

    // Write every raw object and attachment before touching metadata. Any failure
    // removes what this request already wrote and returns 503.
    let mut written: Vec<String> = Vec::new();
    let mut stored_attachments: Vec<Vec<Attachment>> = Vec::with_capacity(pending.len());
    for (event_id, _, _, raw_key) in &pending {
        if let Err(error) = state.objects.put_create(raw_key, raw.clone()).await {
            cleanup_objects(state, &written).await;
            return Err(error.into());
        }
        written.push(raw_key.clone());

        let mut per_event = Vec::with_capacity(attachments.len());
        for (index, attachment) in attachments.iter().enumerate() {
            let key = crate::objects::attachment_key(project.id, *event_id, index);
            if let Err(error) = state
                .objects
                .put_create(&key, attachment.data.clone())
                .await
            {
                cleanup_objects(state, &written).await;
                return Err(error.into());
            }
            written.push(key.clone());
            per_event.push(Attachment {
                filename: attachment.filename.clone(),
                content_type: attachment.content_type.clone(),
                size: attachment.size,
                object_key: Some(key),
                data: Vec::new(),
            });
        }
        stored_attachments.push(per_event);
    }

    let mut jobs = Vec::new();
    let mut orphans = Vec::new();
    let expired;
    {
        let mut store = state.store.write().await;
        for ((id, sentry_id, event, key), event_attachments) in
            pending.into_iter().zip(stored_attachments)
        {
            // A concurrent retry may have won the race; reuse its row and drop
            // the objects we just wrote rather than overwriting authoritative data.
            if store
                .seen_event_ids
                .contains_key(&(project.id, sentry_id.clone()))
            {
                orphans.push(key);
                orphans.extend(
                    event_attachments
                        .iter()
                        .filter_map(|attachment| attachment.object_key.clone()),
                );
                continue;
            }
            store.events.insert(
                id,
                Event {
                    id,
                    sentry_event_id: sentry_id.clone(),
                    project_id: project.id,
                    issue_id: None,
                    received_at,
                    raw_object_key: Some(key),
                    raw_size,
                    raw_expired_at: None,
                    attachment_count: event_attachments.len(),
                    event,
                    attachments: event_attachments,
                },
            );
            store
                .seen_event_ids
                .insert((project.id, sentry_id.clone()), id);
            jobs.push(Job::GroupEvent {
                job_id: Uuid::new_v4(),
                project_id: project.id,
                event_id: id,
            });
        }
        expired = store.apply_retention(project);
    }
    for key in orphans.into_iter().chain(expired) {
        let _ = state.objects.delete(&key).await;
    }
    for job in jobs {
        let _ = state.jobs.send(job);
    }
    state.metrics.accepted(ids.len() as u64);
    Ok(ids)
}

async fn cleanup_objects(state: &AppState, keys: &[String]) {
    for key in keys {
        let _ = state.objects.delete(key).await;
    }
}

async fn persist_logs(
    state: &AppState,
    project: &Project,
    logs: Vec<Value>,
) -> Result<usize, ApiError> {
    state.ensure_storage()?;
    let mut count = 0;
    let expired;
    {
        let mut store = state.store.write().await;
        let received_at = Utc::now();
        for value in logs {
            for item in log_values(&value) {
                if let Some(line) = to_log_line(project.id, &item, received_at) {
                    store.logs.push(line);
                    count += 1;
                }
            }
        }
        expired = store.apply_retention(project);
    }
    for key in expired {
        let _ = state.objects.delete(&key).await;
    }
    state.metrics.accepted(count as u64);
    Ok(count)
}

fn log_values(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items.clone(),
        Value::Object(obj) => obj
            .get("items")
            .or_else(|| obj.get("logs"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| vec![value.clone()]),
        _ => vec![value.clone()],
    }
}

fn to_log_line(project_id: Uuid, value: &Value, received_at: DateTime<Utc>) -> Option<LogLine> {
    let obj = value.as_object()?;
    let message = obj
        .get("message")
        .or_else(|| obj.get("body"))
        .map(value_to_string)
        .unwrap_or_default();
    if message.is_empty() {
        return None;
    }
    let timestamp = parse_time(obj.get("timestamp")).unwrap_or(received_at);
    let level = obj
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("info")
        .to_string();
    let source = obj
        .get("source")
        .and_then(Value::as_str)
        .or_else(|| obj.get("service").and_then(Value::as_str))
        .or_else(|| obj.get("logger").and_then(Value::as_str))
        .unwrap_or("ingest")
        .to_string();
    let attributes = obj
        .get("attributes")
        .map(normalize_attributes)
        .unwrap_or(Value::Null);
    Some(LogLine {
        id: Uuid::new_v4(),
        project_id,
        timestamp,
        level,
        message,
        source,
        attributes,
        received_at,
    })
}

fn normalize_attributes(value: &Value) -> Value {
    let Some(obj) = value.as_object() else {
        return value.clone();
    };
    let mut map = Map::new();
    for (key, value) in obj {
        let inner = value.get("value").cloned().unwrap_or_else(|| value.clone());
        map.insert(key.clone(), inner);
    }
    Value::Object(map)
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn parse_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;
    if let Some(seconds) = value.as_f64() {
        return from_seconds(seconds);
    }
    if let Some(text) = value.as_str() {
        if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
            return Some(dt.with_timezone(&Utc));
        }
        if let Ok(seconds) = text.parse::<f64>() {
            return from_seconds(seconds);
        }
    }
    None
}

fn from_seconds(seconds: f64) -> Option<DateTime<Utc>> {
    let seconds = if seconds > 1e12 {
        seconds / 1000.0
    } else {
        seconds
    };
    let whole = seconds.trunc() as i64;
    let nanos = (seconds.fract().abs() * 1_000_000_000.0) as u32;
    Utc.timestamp_opt(whole, nanos).single()
}

fn parse_log_document(bytes: &[u8]) -> Result<Value, ApiError> {
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        return Ok(value);
    }
    let mut values = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Value>(line) {
            Ok(value) => values.push(value),
            Err(e) => {
                return Err(ApiError::BadRequest(format!(
                    "invalid log JSON (expected JSON or NDJSON): {e}"
                )))
            }
        }
    }
    if values.is_empty() {
        return Err(ApiError::BadRequest("empty log body".into()));
    }
    Ok(Value::Array(values))
}

pub struct ParsedEnvelope {
    pub header: Value,
    pub events: Vec<Value>,
    pub attachments: Vec<Attachment>,
    pub logs: Vec<Value>,
    pub ignored: usize,
}

pub fn parse_envelope(input: &[u8]) -> Result<ParsedEnvelope, String> {
    let mut pos = 0usize;
    let header_line = read_line(input, &mut pos).ok_or_else(|| "empty envelope".to_string())?;
    let header: Value =
        serde_json::from_slice(header_line).map_err(|e| format!("invalid envelope header: {e}"))?;
    if !header.is_object() {
        return Err("envelope header must be a JSON object".into());
    }

    let mut out = ParsedEnvelope {
        header,
        events: Vec::new(),
        attachments: Vec::new(),
        logs: Vec::new(),
        ignored: 0,
    };

    while pos < input.len() {
        if input[pos] == b'\n' {
            pos += 1;
            continue;
        }
        if input[pos] == b'\r' && input.get(pos + 1) == Some(&b'\n') {
            pos += 2;
            continue;
        }
        let item_line = read_line(input, &mut pos)
            .ok_or_else(|| "truncated envelope item header".to_string())?;
        if item_line.is_empty() {
            continue;
        }
        let item: Value = serde_json::from_slice(item_line)
            .map_err(|e| format!("invalid envelope item header: {e}"))?;
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        let payload: Vec<u8> = if let Some(len) = item.get("length").and_then(Value::as_u64) {
            let len = len as usize;
            if pos + len > input.len() {
                return Err("envelope item length exceeds body".into());
            }
            let payload = input[pos..pos + len].to_vec();
            pos += len;
            if input.get(pos) == Some(&b'\n') {
                pos += 1;
            } else if input.get(pos) == Some(&b'\r') && input.get(pos + 1) == Some(&b'\n') {
                pos += 2;
            }
            payload
        } else {
            read_line(input, &mut pos)
                .ok_or_else(|| "truncated envelope item payload".to_string())?
                .to_vec()
        };

        match item_type {
            "event" => {
                let event: Value = serde_json::from_slice(&payload)
                    .map_err(|e| format!("invalid event payload: {e}"))?;
                out.events.push(event);
            }
            "attachment" => {
                out.attachments.push(Attachment::new(
                    item.get("filename")
                        .and_then(Value::as_str)
                        .unwrap_or("attachment")
                        .to_string(),
                    item.get("content_type")
                        .and_then(Value::as_str)
                        .unwrap_or("application/octet-stream")
                        .to_string(),
                    payload,
                ));
            }
            "log" => {
                let log: Value = serde_json::from_slice(&payload)
                    .map_err(|e| format!("invalid log payload: {e}"))?;
                out.logs.push(log);
            }
            _ => out.ignored += 1,
        }
    }

    Ok(out)
}

fn read_line<'a>(input: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    if *pos >= input.len() {
        return None;
    }
    let start = *pos;
    let mut end = start;
    while end < input.len() && input[end] != b'\n' {
        end += 1;
    }
    *pos = if end < input.len() { end + 1 } else { end };
    let mut slice = &input[start..end];
    if slice.last() == Some(&b'\r') {
        slice = &slice[..slice.len() - 1];
    }
    Some(slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(with_length: bool) -> Vec<u8> {
        let event = json!({
            "event_id": "abc123",
            "message": "boom",
            "level": "error"
        });
        let event_body = serde_json::to_vec(&event).unwrap();
        let header = r#"{"event_id":"abc123","sent_at":"2026-10-30T12:00:00Z"}"#;
        let item = if with_length {
            format!(r#"{{"type":"event","length":{}}}"#, event_body.len())
        } else {
            r#"{"type":"event"}"#.to_string()
        };
        let attachment = b"hello attachment";
        let att_header = format!(
            r#"{{"type":"attachment","length":{},"filename":"note.txt","content_type":"text/plain"}}"#,
            attachment.len()
        );
        let mut out = Vec::new();
        out.extend_from_slice(header.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(item.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(&event_body);
        out.push(b'\n');
        out.extend_from_slice(att_header.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(attachment);
        out.push(b'\n');
        out
    }

    #[test]
    fn parses_length_prefixed_envelope() {
        let parsed = parse_envelope(&envelope(true)).unwrap();
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].filename, "note.txt");
        assert_eq!(parsed.events[0]["message"], "boom");
    }

    #[test]
    fn parses_newline_delimited_event() {
        let parsed = parse_envelope(&envelope(false)).unwrap();
        assert_eq!(parsed.events.len(), 1);
    }

    #[test]
    fn rejects_truncated_length() {
        let body = b"{\"event_id\":\"x\"}\n{\"type\":\"event\",\"length\":999}\n{}";
        assert!(parse_envelope(body).is_err());
    }

    #[test]
    fn sentry_auth_parsing() {
        let map =
            parse_sentry_auth("sentry_version=7, sentry_key=\"abc\", sentry_client=raven-js/3.0");
        assert_eq!(map.get("sentry_key").map(String::as_str), Some("abc"));
        assert_eq!(map.get("sentry_version").map(String::as_str), Some("7"));
    }

    #[test]
    fn rejects_bodies_over_the_ingest_limit() {
        let oversize = web::Bytes::from(vec![b'x'; MAX_INGEST_BYTES + 1]);
        match decode_body(oversize) {
            Err(ApiError::PayloadTooLarge(message)) => assert!(message.contains("10485760")),
            other => panic!("expected a payload-too-large error, got {other:?}"),
        }
        let ok = web::Bytes::from_static(b"small");
        assert_eq!(decode_body(ok).unwrap(), b"small");
    }

    #[test]
    fn normalises_log_timestamp_and_level() {
        let value = json!({
            "message": "epoch millis",
            "timestamp": 1_700_000_000_000u64
        });
        let line = to_log_line(Uuid::nil(), &value, Utc::now()).expect("a log line");
        assert_eq!(line.level, "info", "level defaults to info");
        assert_eq!(line.source, "ingest", "source falls back to ingest");
        assert_eq!(line.timestamp.timestamp(), 1_700_000_000);
    }

    #[test]
    fn normalises_sentry_log_attributes() {
        let attrs = json!({
            "service": {"value": "checkout", "type": "string"},
            "duration_ms": {"value": 42, "type": "number"}
        });
        let normalized = normalize_attributes(&attrs);
        assert_eq!(normalized["service"], "checkout");
        assert_eq!(normalized["duration_ms"], 42);
    }
}
