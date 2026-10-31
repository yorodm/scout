//! In-crate integration tests.
//!
//! These drive the same `App` wiring as `main.rs` through `actix_web::test`, so
//! routing, auth, ingest, grouping, classification and the API all run together
//! without a browser, `curl`, or a library target.

use crate::state::AppState;
use crate::worker::Job;
use actix_http::Request;
use actix_web::body::{BoxBody, MessageBody};
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::test;
use actix_web::{web, App, Error, HttpResponse, HttpServer};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock, Semaphore};
use uuid::Uuid;

// --- harness ---------------------------------------------------------------

/// A mock Jev-compatible decisions endpoint bound to an ephemeral loopback
/// port. Each test gets its own server; there is no shared classifier state.
fn start_mock_model() -> String {
    let server =
        HttpServer::new(|| App::new().route("/mock-jev/v1/systemone", web::post().to(mock_model)))
            .bind(("127.0.0.1", 0))
            .expect("bind mock model server");
    let addr = server.addrs()[0];
    actix_web::rt::spawn(server.run());
    format!("http://{addr}")
}

async fn mock_model(body: web::Json<Value>) -> HttpResponse {
    let message = body
        .pointer("/state/message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if message.contains("malformed response") {
        return HttpResponse::Ok().body("not json");
    }
    if message.contains("low confidence") {
        return HttpResponse::Ok().json(json!({ "choice": "issue", "confidence": 0.1 }));
    }
    if message.contains("unknown category") {
        return HttpResponse::Ok().json(json!({ "choice": "billing", "confidence": 0.9 }));
    }
    let category = if message.contains("log") || message.contains("pool") {
        "log"
    } else {
        "issue"
    };
    HttpResponse::Ok().json(json!({ "choice": category, "confidence": 0.9 }))
}

async fn state_for(self_url: &str, max_in_flight: usize) -> web::Data<AppState> {
    let objects = Arc::new(crate::objects::Objects::memory());
    let (seeded, raws) = crate::store::seed(self_url);
    for (key, data) in raws {
        objects
            .put_create(&key, data)
            .await
            .expect("seed raw payload");
    }
    let store = Arc::new(RwLock::new(seeded));
    let (jobs, rx) = mpsc::unbounded_channel::<Job>();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("build test http client");
    crate::worker::spawn(store.clone(), rx, http);
    web::Data::new(AppState {
        store,
        jobs,
        self_url: self_url.to_string(),
        ingest_gate: Arc::new(Semaphore::new(max_in_flight.max(1))),
        session_ttl_days: 7,
        objects,
        storage_ready: Arc::new(AtomicBool::new(true)),
    })
}

/// Mirrors `main.rs` wiring, minus the static-file catch-all (which would make
/// tests depend on the Trunk build output).
async fn init_app(
    self_url: &str,
) -> impl Service<Request, Response = ServiceResponse<BoxBody>, Error = Error> {
    init_app_with_state(state_for(self_url, 256).await).await
}

async fn init_app_with_state(
    state: web::Data<AppState>,
) -> impl Service<Request, Response = ServiceResponse<BoxBody>, Error = Error> {
    test::init_service(
        App::new()
            .app_data(state)
            .app_data(web::PayloadConfig::new(10 * 1024 * 1024))
            .app_data(web::JsonConfig::default().limit(10 * 1024 * 1024))
            .configure(crate::api::config)
            .configure(crate::ingest::config)
            .route(
                "/mock-jev/v1/systemone",
                web::post().to(crate::classifier::mock_systemone),
            )
            .route(
                "/mock-jev/outage",
                web::get().to(crate::classifier::get_outage),
            )
            .route(
                "/mock-jev/outage",
                web::post().to(crate::classifier::set_outage),
            ),
    )
    .await
}

async fn call<S, B>(app: &S, req: Request) -> ServiceResponse<B>
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    test::call_service(app, req).await
}

async fn call_json<S, B>(app: &S, req: Request) -> (StatusCode, Value)
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    let res = call(app, req).await;
    let status = res.status();
    let body = test::read_body(res).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn get(uri: &str, token: &str) -> Request {
    test::TestRequest::get()
        .uri(uri)
        .insert_header(("Authorization", format!("Bearer {token}")))
        .to_request()
}

fn post_json(uri: &str, token: &str, body: Value) -> Request {
    test::TestRequest::post()
        .uri(uri)
        .insert_header(("Authorization", format!("Bearer {token}")))
        .set_json(body)
        .to_request()
}

fn put_json(uri: &str, token: &str, body: Value) -> Request {
    test::TestRequest::put()
        .uri(uri)
        .insert_header(("Authorization", format!("Bearer {token}")))
        .set_json(body)
        .to_request()
}

fn delete(uri: &str, token: &str) -> Request {
    test::TestRequest::delete()
        .uri(uri)
        .insert_header(("Authorization", format!("Bearer {token}")))
        .to_request()
}

fn ingest_request(dsn: &str, key: &str, content_type: &str, body: Vec<u8>, kind: &str) -> Request {
    test::TestRequest::post()
        .uri(&format!("/api/{dsn}/{kind}"))
        .insert_header(("X-Scout-Key", key))
        .insert_header(("Content-Type", content_type))
        .set_payload(body)
        .to_request()
}

async fn login<S, B>(app: &S, email: &str) -> Value
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    let req = test::TestRequest::post()
        .uri("/api/login")
        .set_json(json!({ "email": email, "password": crate::store::DEMO_PASSWORD }))
        .to_request();
    let (status, body) = call_json(app, req).await;
    assert_eq!(status, StatusCode::OK, "login for {email} failed: {body}");
    body
}

async fn project_id<S, B>(app: &S, token: &str, name: &str) -> Uuid
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    let (status, body) = call_json(app, get("/api/projects", token)).await;
    assert_eq!(status, StatusCode::OK, "project list failed: {body}");
    body["projects"]
        .as_array()
        .and_then(|projects| projects.iter().find(|p| p["name"] == name))
        .and_then(|p| p["id"].as_str())
        .map(|id| Uuid::parse_str(id).expect("project id is a uuid"))
        .unwrap_or_else(|| panic!("no project named {name}: {body}"))
}

async fn issues_for<S, B>(app: &S, token: &str, project: Uuid) -> Vec<Value>
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    let (status, body) =
        call_json(app, get(&format!("/api/projects/{project}/issues"), token)).await;
    assert_eq!(status, StatusCode::OK, "issue list failed: {body}");
    body["issues"].as_array().cloned().unwrap_or_default()
}

/// Grouping runs on the shared worker, so poll until the issue reaches the
/// expected count instead of assuming ingest and grouping are synchronous.
async fn wait_for_issue_count<S, B>(
    app: &S,
    token: &str,
    project: Uuid,
    title: &str,
    expected: u64,
) -> Value
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    for _ in 0..250 {
        let issues = issues_for(app, token, project).await;
        if let Some(issue) = issues.iter().find(|i| i["title"] == title) {
            if issue["count"].as_u64().unwrap_or(0) >= expected {
                return issue.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for issue {title:?} to reach count {expected}");
}

/// Poll the issue list (which does not present the issue) until the classifier
/// answer lands. Opening the detail would freeze suggestions by design (FR-019).
async fn wait_for_suggestion<S, B>(app: &S, token: &str, project: Uuid, issue_id: Uuid) -> Value
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    let target = issue_id.to_string();
    for _ in 0..250 {
        let issues = issues_for(app, token, project).await;
        if let Some(issue) = issues
            .iter()
            .find(|issue| issue["id"].as_str() == Some(target.as_str()))
        {
            if issue["suggestion"].is_object() {
                return issue.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for a classifier suggestion");
}

async fn first_open_issue<S, B>(app: &S, token: &str, project: Uuid) -> Value
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    issues_for(app, token, project)
        .await
        .into_iter()
        .find(|i| i["state"] == "open")
        .expect("a seeded open issue")
}

// --- fixtures --------------------------------------------------------------

fn event(event_id: &str, message: &str, fingerprint: &[&str]) -> Value {
    json!({
        "event_id": event_id,
        "timestamp": chrono::Utc::now().timestamp(),
        "platform": "rust",
        "level": "error",
        "release": "integration@1.0.0",
        "environment": "test",
        "message": message,
        "fingerprint": fingerprint,
        "exception": {
            "values": [{
                "type": "IntegrationError",
                "value": message,
                "stacktrace": {"frames": [{
                    "filename": "src/tests.rs",
                    "function": "run",
                    "lineno": 1,
                    "in_app": true
                }]}
            }]
        },
        "breadcrumbs": {"values": [{
            "category": "test",
            "level": "info",
            "message": "integration breadcrumb"
        }]},
        "tags": {"service": "integration-test"}
    })
}

fn envelope(events: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(
        json!({ "event_id": events[0]["event_id"] })
            .to_string()
            .as_bytes(),
    );
    out.push(b'\n');
    for event in events {
        out.extend_from_slice(br#"{"type":"event"}"#);
        out.push(b'\n');
        out.extend_from_slice(event.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

fn hex_id(value: usize) -> String {
    format!("{value:032x}")
}

// --- ingest, grouping, storage --------------------------------------------

#[actix_web::test]
async fn ingest_groups_identical_events_into_one_issue() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "platform-api").await;

    let events = vec![
        event(&hex_id(1), "grouping check", &["rust-group"]),
        event(&hex_id(2), "grouping check", &["rust-group"]),
        event(&hex_id(3), "grouping check", &["rust-group"]),
    ];
    let req = ingest_request(
        "2",
        "scout_platform_key",
        "application/x-sentry-envelope",
        envelope(&events),
        "envelope",
    );
    let (status, body) = call_json(&app, req).await;
    assert_eq!(status, StatusCode::OK, "ingest failed: {body}");
    assert_eq!(body["event_ids"].as_array().unwrap().len(), 3);

    let issue =
        wait_for_issue_count(&app, token, project, "IntegrationError: grouping check", 3).await;
    assert_eq!(issue["count"].as_u64(), Some(3));
    assert!(!issue["fingerprint"].as_str().unwrap_or("").is_empty());
    assert!(issue["first_seen"].as_str().unwrap() <= issue["last_seen"].as_str().unwrap());
    assert_eq!(issue["breadcrumbs"].as_array().unwrap().len(), 1);
    assert_eq!(issue["tags"]["service"], "integration-test");
}

#[actix_web::test]
async fn thousand_identical_events_collapse_to_one_issue() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "platform-api").await;

    let events: Vec<Value> = (0..1_000)
        .map(|i| event(&hex_id(i), "thousand check", &["thousand-group"]))
        .collect();
    let req = ingest_request(
        "2",
        "scout_platform_key",
        "application/x-sentry-envelope",
        envelope(&events),
        "envelope",
    );
    let (status, body) = call_json(&app, req).await;
    assert_eq!(status, StatusCode::OK, "ingest failed: {body}");

    let issue = wait_for_issue_count(
        &app,
        token,
        project,
        "IntegrationError: thousand check",
        1_000,
    )
    .await;
    assert_eq!(issue["count"].as_u64(), Some(1_000));
}

#[actix_web::test]
async fn ingest_rejects_unknown_and_mismatched_keys() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let body = envelope(&[event(&hex_id(4), "key check", &["key-group"])]);

    let (status, unknown) = call_json(
        &app,
        ingest_request(
            "1",
            "not_a_real_key",
            "application/x-sentry-envelope",
            body.clone(),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The platform-api key must not be usable against scout-web.
    let (status, wrong_project) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_platform_key",
            "application/x-sentry-envelope",
            body.clone(),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A revoked key is rejected the same way, with no extra information.
    state
        .store
        .write()
        .await
        .keys
        .get_mut("scout_demo_key")
        .expect("seeded demo key")
        .revoked = true;
    let (status, revoked) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/x-sentry-envelope",
            body,
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    assert_eq!(
        unknown, wrong_project,
        "key failures must not be distinguishable"
    );
    assert_eq!(unknown, revoked, "key failures must not be distinguishable");
    assert_eq!(unknown["error"], "invalid or missing ingest key");
}

#[actix_web::test]
async fn retried_event_id_is_counted_once() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "platform-api").await;

    let payload = event(&hex_id(5), "retry check", &["retry-group"])
        .to_string()
        .into_bytes();
    for _ in 0..2 {
        let (status, body) = call_json(
            &app,
            ingest_request(
                "2",
                "scout_platform_key",
                "application/json",
                payload.clone(),
                "store",
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "store failed: {body}");
    }

    let issue =
        wait_for_issue_count(&app, token, project, "IntegrationError: retry check", 1).await;
    assert_eq!(issue["count"].as_u64(), Some(1));

    // Give any accidental second job time to land before asserting again.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let issue = first_issue_by_title(&app, token, project, "IntegrationError: retry check").await;
    assert_eq!(issue["count"].as_u64(), Some(1));
}

async fn first_issue_by_title<S, B>(app: &S, token: &str, project: Uuid, title: &str) -> Value
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    issues_for(app, token, project)
        .await
        .into_iter()
        .find(|i| i["title"] == title)
        .unwrap_or_else(|| panic!("no issue titled {title:?}"))
}

#[actix_web::test]
async fn raw_payload_is_byte_identical() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "platform-api").await;

    let payload = event(&hex_id(6), "raw check", &["raw-group"])
        .to_string()
        .into_bytes();
    let (status, body) = call_json(
        &app,
        ingest_request(
            "2",
            "scout_platform_key",
            "application/json",
            payload.clone(),
            "store",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "store failed: {body}");

    let issue = wait_for_issue_count(&app, token, project, "IntegrationError: raw check", 1).await;
    let issue_id = issue["id"].as_str().unwrap();

    let res = call(&app, get(&format!("/api/issues/{issue_id}/raw"), token)).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("X-Scout-Event-Id").unwrap(),
        hex_id(6).as_str()
    );
    let raw = test::read_body(res).await;
    assert_eq!(&raw[..], &payload[..], "stored payload was rewritten");
}

// --- classification --------------------------------------------------------

#[actix_web::test]
async fn suggestion_is_attached_from_the_model() {
    let self_url = start_mock_model();
    let app = init_app(&self_url).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    let req = ingest_request(
        "1",
        "scout_demo_key",
        "application/x-sentry-envelope",
        envelope(&[event(&hex_id(7), "suggest me", &["suggest-group"])]),
        "envelope",
    );
    let (status, body) = call_json(&app, req).await;
    assert_eq!(status, StatusCode::OK, "ingest failed: {body}");

    let issue = wait_for_issue_count(&app, token, project, "IntegrationError: suggest me", 1).await;
    let issue = wait_for_suggestion(
        &app,
        token,
        project,
        issue["id"].as_str().unwrap().parse().unwrap(),
    )
    .await;
    assert_eq!(issue["suggestion"]["category"], "issue");
    assert!(issue["suggestion"]["confidence"].as_f64().unwrap() > 0.0);
}

#[actix_web::test]
async fn unreachable_classifier_leaves_items_without_a_suggestion() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    // Point the project at a port nothing listens on: ingest must still accept.
    let (status, body) = call_json(
        &app,
        put_json(
            &format!("/api/projects/{project}/settings"),
            token,
            json!({
                "retention_days": 30,
                "model": {"endpoint": "http://127.0.0.1:1", "model": "jev-latest"}
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "settings failed: {body}");

    let req = ingest_request(
        "1",
        "scout_demo_key",
        "application/x-sentry-envelope",
        envelope(&[event(&hex_id(8), "outage check", &["outage-group"])]),
        "envelope",
    );
    let (status, body) = call_json(&app, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "ingest blocked on classifier: {body}"
    );

    let issue =
        wait_for_issue_count(&app, token, project, "IntegrationError: outage check", 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let (status, detail) = call_json(
        &app,
        get(
            &format!("/api/issues/{}", issue["id"].as_str().unwrap()),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(detail["issue"]["suggestion"].is_null());
    assert_eq!(detail["raw_available"], true);
}

#[actix_web::test]
async fn confirm_and_override_categorisation() {
    let self_url = start_mock_model();
    let app = init_app(&self_url).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    let req = ingest_request(
        "1",
        "scout_demo_key",
        "application/x-sentry-envelope",
        envelope(&[event(
            &hex_id(9),
            "categorisation check",
            &["categorise-group"],
        )]),
        "envelope",
    );
    let (status, body) = call_json(&app, req).await;
    assert_eq!(status, StatusCode::OK, "ingest failed: {body}");

    let issue = wait_for_issue_count(
        &app,
        token,
        project,
        "IntegrationError: categorisation check",
        1,
    )
    .await;
    let issue_id = issue["id"].as_str().unwrap().to_string();
    let issue = wait_for_suggestion(&app, token, project, issue_id.parse().unwrap()).await;
    assert_eq!(issue["suggestion"]["category"], "issue");

    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{issue_id}/categorisation"),
            token,
            json!({ "action": "confirm" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "confirm failed: {body}");
    assert_eq!(body["issue"]["decision"]["category"], "issue");
    assert_eq!(body["issue"]["decision"]["overridden"], false);
    assert_eq!(body["issue"]["decision"]["by"], "demo@scout.local");

    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{issue_id}/categorisation"),
            token,
            json!({ "action": "override", "category": "log" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "override failed: {body}");
    assert_eq!(body["issue"]["decision"]["category"], "log");
    assert_eq!(body["issue"]["decision"]["overridden"], true);

    // Invalid categories are rejected rather than stored.
    let (status, _) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{issue_id}/categorisation"),
            token,
            json!({ "action": "override", "category": "banana" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// --- triage and access control --------------------------------------------

#[actix_web::test]
async fn resolve_and_reopen_issue_records_history() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;
    let issue = first_open_issue(&app, token, project).await;
    let issue_id = issue["id"].as_str().unwrap();

    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{issue_id}/state"),
            token,
            json!({ "state": "resolved" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resolve failed: {body}");
    assert_eq!(body["issue"]["state"], "resolved");
    let history = body["issue"]["state_history"].as_array().unwrap();
    let last = history.last().unwrap();
    assert_eq!(last["state"], "resolved");
    assert_eq!(last["by"], "demo@scout.local");

    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{issue_id}/state"),
            token,
            json!({ "state": "open" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reopen failed: {body}");
    assert_eq!(body["issue"]["state"], "open");
    assert_eq!(
        body["issue"]["state_history"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["state"],
        "open"
    );

    // FR-021: ignored is a real state and must be recorded like the others.
    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{issue_id}/state"),
            token,
            json!({ "state": "ignored" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ignore failed: {body}");
    assert_eq!(body["issue"]["state"], "ignored");
    assert_eq!(
        body["issue"]["state_history"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["state"],
        "ignored"
    );

    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{issue_id}/state"),
            token,
            json!({ "state": "open" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reopen from ignored failed: {body}");
    assert_eq!(body["issue"]["state"], "open");
    assert_eq!(body["issue"]["state_history"].as_array().unwrap().len(), 5);
}

#[actix_web::test]
async fn viewer_is_denied_settings_resolve_and_other_projects() {
    let app = init_app("http://127.0.0.1:1").await;
    let viewer = login(&app, "viewer@scout.local").await;
    let token = viewer["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;
    let issue = first_open_issue(&app, token, project).await;
    let issue_id = issue["id"].as_str().unwrap();

    let (status, _) = call_json(
        &app,
        get(&format!("/api/projects/{project}/settings"), token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{issue_id}/state"),
            token,
            json!({ "state": "resolved" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[actix_web::test]
async fn viewer_cannot_reach_a_project_they_are_not_in() {
    let app = init_app("http://127.0.0.1:1").await;
    let viewer = login(&app, "viewer@scout.local").await;
    let token = viewer["token"].as_str().unwrap();
    let web = project_id(&app, token, "scout-web").await;
    let web_id = web.to_string();

    // viewer has no membership on platform-api, so resolve its id through an
    // account that can see it and then probe with the viewer token.
    let demo = login(&app, "demo@scout.local").await;
    let demo_token = demo["token"].as_str().unwrap();
    let platform = project_id(&app, demo_token, "platform-api").await;

    let (status, _) = call_json(
        &app,
        get(&format!("/api/projects/{platform}/issues"), token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Detail and raw are project-scoped too.
    let platform_issue = first_open_issue(&app, demo_token, platform).await;
    let platform_issue_id = platform_issue["id"].as_str().unwrap();
    let (status, _) = call_json(
        &app,
        get(&format!("/api/issues/{platform_issue_id}"), token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call_json(
        &app,
        get(&format!("/api/issues/{platform_issue_id}/raw"), token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Sanity check: the same call works for a member.
    let (status, _) = call_json(&app, get(&format!("/api/projects/{web_id}/issues"), token)).await;
    assert_eq!(status, StatusCode::OK);
}

#[actix_web::test]
async fn triager_can_resolve_but_not_change_settings() {
    let app = init_app("http://127.0.0.1:1").await;
    let triager = login(&app, "triager@scout.local").await;
    let token = triager["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;
    let issue = first_open_issue(&app, token, project).await;

    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{}/state", issue["id"].as_str().unwrap()),
            token,
            json!({ "state": "resolved" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "triager resolve failed: {body}");

    let (status, _) = call_json(
        &app,
        get(&format!("/api/projects/{project}/settings"), token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[actix_web::test]
async fn last_owner_cannot_be_demoted_or_removed() {
    let app = init_app("http://127.0.0.1:1").await;
    let demo = login(&app, "demo@scout.local").await;
    let token = demo["token"].as_str().unwrap();
    let owner_id = demo["user"]["id"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    let (status, body) = call_json(
        &app,
        put_json(
            &format!("/api/projects/{project}/members/{owner_id}"),
            token,
            json!({ "role": "viewer" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "demote returned: {body}");

    let (status, body) = call_json(
        &app,
        delete(
            &format!("/api/projects/{project}/members/{owner_id}"),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "remove returned: {body}");
}

#[actix_web::test]
async fn role_change_takes_effect_without_a_restart() {
    let app = init_app("http://127.0.0.1:1").await;
    let demo = login(&app, "demo@scout.local").await;
    let admin_token = demo["token"].as_str().unwrap();
    let project = project_id(&app, admin_token, "scout-web").await;
    let viewer = login(&app, "viewer@scout.local").await;
    let viewer_token = viewer["token"].as_str().unwrap();
    let viewer_id = viewer["user"]["id"].as_str().unwrap();

    let issue = first_open_issue(&app, viewer_token, project).await;
    let issue_id = issue["id"].as_str().unwrap();

    // Before the grant, the viewer cannot triage.
    let (status, _) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{issue_id}/state"),
            viewer_token,
            json!({ "state": "resolved" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = call_json(
        &app,
        put_json(
            &format!("/api/projects/{project}/members/{viewer_id}"),
            admin_token,
            json!({ "role": "triager" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "role update failed: {body}");

    // The existing session picks up the new role immediately.
    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/issues/{issue_id}/state"),
            viewer_token,
            json!({ "state": "resolved" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "new role not applied: {body}");
}

#[actix_web::test]
async fn revoked_key_cannot_ingest() {
    let app = init_app("http://127.0.0.1:1").await;
    let demo = login(&app, "demo@scout.local").await;
    let token = demo["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/projects/{project}/keys"),
            token,
            json!({ "label": "ci" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "key creation failed: {body}");
    let key = body["key"]["key"].as_str().unwrap().to_string();
    let key_id = body["key"]["id"].as_str().unwrap().to_string();
    assert!(key.starts_with("scout_"));

    let (status, body) = call_json(
        &app,
        delete(&format!("/api/projects/{project}/keys/{key_id}"), token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "revoke failed: {body}");

    let (status, _) = call_json(
        &app,
        ingest_request(
            "1",
            &key,
            "application/x-sentry-envelope",
            envelope(&[event(&hex_id(10), "revoked key", &["revoked-group"])]),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// --- logs and search -------------------------------------------------------

#[actix_web::test]
async fn logs_are_ingested_and_searchable() {
    let app = init_app("http://127.0.0.1:1").await;
    let demo = login(&app, "demo@scout.local").await;
    let token = demo["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    let log = json!({
        "logs": [{
            "level": "warning",
            "message": "rust integration log 4821",
            "source": "tests",
            "timestamp": chrono::Utc::now().timestamp(),
            "attributes": {"suite": "integration"}
        }]
    });
    let (status, body) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/json",
            log.to_string().into_bytes(),
            "logs",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "log ingest failed: {body}");
    assert_eq!(body["accepted"], 1);

    let (status, body) = call_json(
        &app,
        get(&format!("/api/projects/{project}/logs?q=4821"), token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let logs = body["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["message"], "rust integration log 4821");
    assert_eq!(logs[0]["source"], "tests");
    assert_eq!(logs[0]["attributes"]["suite"], "integration");
}

#[actix_web::test]
async fn search_returns_issues_and_logs_together() {
    let app = init_app("http://127.0.0.1:1").await;
    let demo = login(&app, "demo@scout.local").await;
    let token = demo["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    let req = ingest_request(
        "1",
        "scout_demo_key",
        "application/x-sentry-envelope",
        envelope(&[event(&hex_id(11), "searchable failure", &["search-group"])]),
        "envelope",
    );
    let (status, body) = call_json(&app, req).await;
    assert_eq!(status, StatusCode::OK, "ingest failed: {body}");

    let log = json!({
        "level": "error",
        "message": "searchable failure observed",
        "source": "tests",
        "timestamp": chrono::Utc::now().timestamp()
    });
    let (status, _) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/json",
            log.to_string().into_bytes(),
            "logs",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let issue = wait_for_issue_count(
        &app,
        token,
        project,
        "IntegrationError: searchable failure",
        1,
    )
    .await;
    assert!(issue["id"].is_string());

    let (status, body) = call_json(
        &app,
        get(
            &format!("/api/projects/{project}/search?q=searchable"),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["title"] == "IntegrationError: searchable failure"));
    assert!(body["logs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["message"] == "searchable failure observed"));
}

// --- ingest limits, durability, backpressure -------------------------------

#[actix_web::test]
async fn oversized_envelope_is_rejected_before_anything_is_stored() {
    use std::io::Write;

    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let before = state.store.read().await.events.len();

    // A gzip body that is small on the wire but expands past 10 MB must still
    // be rejected: the limit applies to the decompressed payload.
    let oversized = event(
        &hex_id(90),
        &"x".repeat(crate::ingest::MAX_INGEST_BYTES + 1),
        &["oversized-group"],
    );
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&envelope(&[oversized])).unwrap();
    let compressed = encoder.finish().unwrap();
    assert!(compressed.len() < crate::ingest::MAX_INGEST_BYTES);

    let req = test::TestRequest::post()
        .uri("/api/2/envelope")
        .insert_header(("X-Scout-Key", "scout_platform_key"))
        .insert_header(("Content-Encoding", "gzip"))
        .insert_header(("Content-Type", "application/x-sentry-envelope"))
        .set_payload(compressed)
        .to_request();
    let res = call(&app, req).await;
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = test::read_body(res).await;
    assert!(!body.is_empty(), "413 responses should explain the limit");

    assert_eq!(
        state.store.read().await.events.len(),
        before,
        "a rejected request must not create an event"
    );
}

#[actix_web::test]
async fn unavailable_storage_returns_503_and_acknowledges_nothing() {
    use std::sync::atomic::Ordering;

    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let before = state.store.read().await.events.len();

    state.storage_ready.store(false, Ordering::Relaxed);
    let req = ingest_request(
        "2",
        "scout_platform_key",
        "application/x-sentry-envelope",
        envelope(&[event(&hex_id(91), "storage down", &["storage-group"])]),
        "envelope",
    );
    let (status, body) = call_json(&app, req).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {body}");
    assert_eq!(state.store.read().await.events.len(), before);

    let (health, health_body) = call_json(
        &app,
        test::TestRequest::get().uri("/api/health").to_request(),
    )
    .await;
    assert_eq!(health, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(health_body["status"], "degraded");

    state.storage_ready.store(true, Ordering::Relaxed);
    let (health, _) = call_json(
        &app,
        test::TestRequest::get().uri("/api/health").to_request(),
    )
    .await;
    assert_eq!(health, StatusCode::OK);
}

#[actix_web::test]
async fn saturated_ingest_cap_returns_429_with_retry_after() {
    let state = state_for("http://127.0.0.1:1", 1).await;
    let app = init_app_with_state(state.clone()).await;

    // Hold the only permit so the next request cannot acquire one.
    let held = state
        .ingest_gate
        .clone()
        .acquire_owned()
        .await
        .expect("acquire the sole ingest permit");

    let body = envelope(&[event(&hex_id(92), "cap check", &["cap-group"])]);
    let req = ingest_request(
        "2",
        "scout_platform_key",
        "application/x-sentry-envelope",
        body,
        "envelope",
    );
    let res = call(&app, req).await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.headers().get("retry-after").unwrap(), "1");

    drop(held);

    let req = ingest_request(
        "2",
        "scout_platform_key",
        "application/x-sentry-envelope",
        envelope(&[event(&hex_id(93), "cap check", &["cap-group"])]),
        "envelope",
    );
    let (status, body) = call_json(&app, req).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
}

#[actix_web::test]
async fn attachments_and_release_survive_grouping() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "platform-api").await;

    let event_id = hex_id(94);
    let payload = b"thread panicked at src/main.rs:42";
    let mut raw = Vec::new();
    raw.extend_from_slice(json!({ "event_id": event_id }).to_string().as_bytes());
    raw.push(b'\n');
    raw.extend_from_slice(br#"{"type":"event"}"#);
    raw.push(b'\n');
    raw.extend_from_slice(
        event(&event_id, "attachment check", &["attach-group"])
            .to_string()
            .as_bytes(),
    );
    raw.push(b'\n');
    raw.extend_from_slice(
        format!(
            r#"{{"type":"attachment","length":{},"filename":"crash.log","content_type":"text/plain"}}"#,
            payload.len()
        )
        .as_bytes(),
    );
    raw.push(b'\n');
    raw.extend_from_slice(payload);
    raw.push(b'\n');

    let req = ingest_request(
        "2",
        "scout_platform_key",
        "application/x-sentry-envelope",
        raw,
        "envelope",
    );
    let (status, body) = call_json(&app, req).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let issue = wait_for_issue_count(
        &app,
        token,
        project,
        "IntegrationError: attachment check",
        1,
    )
    .await;
    assert_eq!(issue["release"], "integration@1.0.0");
    assert_eq!(issue["breadcrumbs"].as_array().unwrap().len(), 1);

    let issue_id = issue["id"].as_str().unwrap();
    let (status, detail) = call_json(&app, get(&format!("/api/issues/{issue_id}"), token)).await;
    assert_eq!(status, StatusCode::OK, "detail: {detail}");
    assert_eq!(detail["attachments"][0]["filename"], "crash.log");
    assert_eq!(detail["attachments"][0]["size"], payload.len());
    assert_eq!(detail["event"]["attachment_count"], 1);
}

// --- sessions and credentials ---------------------------------------------

#[actix_web::test]
async fn login_requires_a_valid_password_and_logout_revokes_the_session() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state).await;

    let wrong_password = test::TestRequest::post()
        .uri("/api/login")
        .set_json(json!({ "email": "demo@scout.local", "password": "not-the-password" }))
        .to_request();
    let (status, body) = call_json(&app, wrong_password).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let unknown_email = test::TestRequest::post()
        .uri("/api/login")
        .set_json(json!({ "email": "nobody@scout.local", "password": "not-the-password" }))
        .to_request();
    let (status, unknown_body) = call_json(&app, unknown_email).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        body, unknown_body,
        "a wrong password and an unknown email must be indistinguishable"
    );

    let auth = login(&app, "demo@scout.local").await;
    assert!(
        auth["user"].get("password_hash").is_none(),
        "password hashes must never be serialised: {auth}"
    );
    assert_eq!(auth["user"]["is_operator"], false);
    let token = auth["token"].as_str().unwrap().to_string();

    let (status, _) = call_json(&app, get("/api/me", &token)).await;
    assert_eq!(status, StatusCode::OK);

    let logout = test::TestRequest::post()
        .uri("/api/logout")
        .insert_header(("Authorization", format!("Bearer {token}")))
        .to_request();
    let (status, _) = call_json(&app, logout).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call_json(&app, get("/api/me", &token)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "logout must revoke the token"
    );
}

#[actix_web::test]
async fn expired_sessions_are_rejected() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "triager@scout.local").await;
    let token = auth["token"].as_str().unwrap().to_string();

    {
        let mut store = state.store.write().await;
        let session = store
            .sessions
            .get_mut(&crate::auth::hash_token(&token))
            .expect("the session is stored under its hash");
        session.expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
    }

    let (status, _) = call_json(&app, get("/api/me", &token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// --- issue list, detail, and occurrences -----------------------------------

#[actix_web::test]
async fn issue_list_filters_by_category_level_and_sorts() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    // Only the webhook issue carries a human decision, so `category` must not
    // match issues that merely have a suggestion.
    let (status, body) = call_json(
        &app,
        get(
            &format!("/api/projects/{project}/issues?category=issue"),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "filtered list failed: {body}");
    let issues = body["issues"].as_array().unwrap();
    assert!(!issues.is_empty());
    for issue in issues {
        assert_eq!(issue["decision"]["category"], "issue");
    }
    assert!(issues
        .iter()
        .any(|issue| issue["title"] == "WebhookDeliveryFailed: 503 from inventory-service"));
    assert!(
        !issues.iter().any(|issue| issue["title"]
            == "TypeError: Cannot read properties of undefined (reading 'currency')"),
        "a suggestion is not a decision"
    );

    let (status, body) = call_json(
        &app,
        get(
            &format!("/api/projects/{project}/issues?level=fatal"),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let issues = body["issues"].as_array().unwrap();
    assert!(issues.iter().all(|issue| issue["level"] == "fatal"));
    assert!(issues.iter().any(|issue| issue["title"]
        == "PoolExhaustedError: timed out waiting for a database connection (30s)"));

    let (status, body) = call_json(
        &app,
        get(&format!("/api/projects/{project}/issues?sort=count"), token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let counts: Vec<u64> = body["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|issue| issue["count"].as_u64())
        .collect();
    assert!(
        counts.windows(2).all(|pair| pair[0] >= pair[1]),
        "count sort must be descending: {counts:?}"
    );
}

#[actix_web::test]
async fn issue_detail_freezes_first_presented_at_and_reports_raw_availability() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;
    let issue = issues_for(&app, token, project)
        .await
        .into_iter()
        .find(|issue| {
            issue["title"] == "TypeError: Cannot read properties of undefined (reading 'currency')"
        })
        .expect("seeded checkout issue");
    let issue_id = issue["id"].as_str().unwrap();
    assert!(
        issue["first_presented_at"].is_null(),
        "listing must not present the issue"
    );

    let (status, first) = call_json(&app, get(&format!("/api/issues/{issue_id}"), token)).await;
    assert_eq!(status, StatusCode::OK, "detail failed: {first}");
    let presented = first["issue"]["first_presented_at"]
        .as_str()
        .expect("the first read sets first_presented_at")
        .to_string();
    assert_eq!(first["raw_available"], true);

    tokio::time::sleep(Duration::from_millis(10)).await;
    let (status, second) = call_json(&app, get(&format!("/api/issues/{issue_id}"), token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        second["issue"]["first_presented_at"].as_str(),
        Some(presented.as_str()),
        "later reads must not advance the timestamp"
    );
}

#[actix_web::test]
async fn occurrences_are_newest_first_and_project_scoped() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let web = project_id(&app, token, "scout-web").await;
    let platform = project_id(&app, token, "platform-api").await;

    let issue = issues_for(&app, token, web)
        .await
        .into_iter()
        .find(|issue| {
            issue["title"] == "TypeError: Cannot read properties of undefined (reading 'currency')"
        })
        .expect("seeded checkout issue");
    let issue_id = Uuid::parse_str(issue["id"].as_str().unwrap()).unwrap();

    // Add two controlled occurrences so ordering does not depend on timing.
    {
        let mut store = state.store.write().await;
        let base = store
            .events
            .values()
            .find(|event| event.issue_id == Some(issue_id))
            .cloned()
            .expect("the seeded event is linked to the issue");
        for (offset, sentry_id) in [(30, hex_id(70)), (5, hex_id(71))] {
            let mut event = base.clone();
            event.id = Uuid::new_v4();
            event.sentry_event_id = sentry_id;
            event.received_at = chrono::Utc::now() - chrono::Duration::seconds(offset);
            store.events.insert(event.id, event);
        }
    }

    let (status, body) = call_json(
        &app,
        get(&format!("/api/issues/{issue_id}/occurrences"), token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "occurrences failed: {body}");
    let occurrences = body["occurrences"].as_array().unwrap();
    assert_eq!(occurrences.len(), 3);
    let times: Vec<String> = occurrences
        .iter()
        .map(|occurrence| occurrence["received_at"].as_str().unwrap().to_string())
        .collect();
    assert!(
        times.windows(2).all(|pair| pair[0] >= pair[1]),
        "occurrences must be newest first: {times:?}"
    );

    // A viewer of scout-web must not read another project's occurrences.
    let viewer = login(&app, "viewer@scout.local").await;
    let viewer_token = viewer["token"].as_str().unwrap();
    let platform_issue = first_open_issue(&app, token, platform).await;
    let (status, _) = call_json(
        &app,
        get(
            &format!(
                "/api/issues/{}/occurrences",
                platform_issue["id"].as_str().unwrap()
            ),
            viewer_token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[actix_web::test]
async fn expired_raw_payload_is_reported_without_failing_the_issue() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;
    let issue = issues_for(&app, token, project)
        .await
        .into_iter()
        .find(|issue| {
            issue["title"] == "TypeError: Cannot read properties of undefined (reading 'currency')"
        })
        .expect("seeded checkout issue");
    let issue_id = issue["id"].as_str().unwrap().to_string();

    {
        let mut store = state.store.write().await;
        let last_event = Uuid::parse_str(issue["last_event_id"].as_str().unwrap()).unwrap();
        let event = store.events.get_mut(&last_event).expect("the last event");
        event.raw_object_key = None;
        event.raw_expired_at = Some(chrono::Utc::now());
    }

    let (status, detail) = call_json(&app, get(&format!("/api/issues/{issue_id}"), token)).await;
    assert_eq!(status, StatusCode::OK, "metadata stays readable: {detail}");
    assert_eq!(detail["raw_available"], false);

    let (status, body) = call_json(&app, get(&format!("/api/issues/{issue_id}/raw"), token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap_or("").contains("expired"));
}

// --- logs and unified search ------------------------------------------------

#[actix_web::test]
async fn logs_accept_json_ndjson_and_envelope_items() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();

    let ndjson = b"{\"message\":\"ndjson marker one\",\"level\":\"warning\"}\n{\"message\":\"ndjson marker two\"}\n".to_vec();
    let (status, body) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/x-ndjson",
            ndjson,
            "logs",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ndjson ingest failed: {body}");
    assert_eq!(body["accepted"], 2);

    let mut raw = Vec::new();
    raw.extend_from_slice(br#"{"event_id":"abc"}"#);
    raw.push(b'\n');
    raw.extend_from_slice(br#"{"type":"log"}"#);
    raw.push(b'\n');
    raw.extend_from_slice(
        json!({ "message": "envelope log marker", "level": "error" })
            .to_string()
            .as_bytes(),
    );
    raw.push(b'\n');
    // Unknown item types are counted, not rejected.
    raw.extend_from_slice(br#"{"type":"span"}"#);
    raw.push(b'\n');
    raw.extend_from_slice(br#"{"span_id":"abc"}"#);
    raw.push(b'\n');

    let (status, body) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/x-sentry-envelope",
            raw,
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "envelope ingest failed: {body}");
    assert_eq!(body["logs"], 1);
    assert_eq!(body["ignored"], 1);

    let project = project_id(&app, token, "scout-web").await;
    let (status, body) = call_json(
        &app,
        get(
            &format!("/api/projects/{project}/logs?q=envelope+log+marker"),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let logs = body["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 1, "envelope logs share the normal log path");
    assert_eq!(logs[0]["level"], "error");
}

#[actix_web::test]
async fn logs_are_newest_first_capped_and_project_scoped() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let web = project_id(&app, token, "scout-web").await;
    let platform = project_id(&app, token, "platform-api").await;

    // Ingest the same term into two projects; search must stay scoped.
    for (dsn, key, source) in [
        ("1", "scout_demo_key", "web-isolation"),
        ("2", "scout_platform_key", "platform-isolation"),
    ] {
        let (status, body) = call_json(
            &app,
            ingest_request(
                dsn,
                key,
                "application/json",
                json!({ "message": "isolation marker", "source": source })
                    .to_string()
                    .into_bytes(),
                "logs",
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "log ingest failed: {body}");
    }

    for (project, expected_source) in [(web, "web-isolation"), (platform, "platform-isolation")] {
        let (status, body) = call_json(
            &app,
            get(
                &format!("/api/projects/{project}/search?q=isolation+marker"),
                token,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let logs = body["logs"].as_array().unwrap();
        assert_eq!(logs.len(), 1, "search must not cross project boundaries");
        assert_eq!(logs[0]["source"], expected_source);
    }

    // Freshest-first ordering and the limit are both enforced.
    {
        let mut store = state.store.write().await;
        let now = chrono::Utc::now();
        for index in 0..55 {
            store.logs.push(crate::models::LogLine {
                id: Uuid::new_v4(),
                project_id: web,
                timestamp: now - chrono::Duration::seconds(index),
                level: "info".into(),
                message: format!("cap marker {index}"),
                source: "cap-test".into(),
                attributes: json!({ "index": index }),
                received_at: now,
            });
        }
    }

    let (status, body) = call_json(
        &app,
        get(
            &format!("/api/projects/{web}/logs?q=cap+marker&limit=10"),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let logs = body["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 10);
    let timestamps: Vec<String> = logs
        .iter()
        .map(|log| log["timestamp"].as_str().unwrap().to_string())
        .collect();
    assert!(
        timestamps.windows(2).all(|pair| pair[0] >= pair[1]),
        "logs must be newest first: {timestamps:?}"
    );

    let (status, body) = call_json(
        &app,
        get(&format!("/api/projects/{web}/search?q=cap+marker"), token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["logs"].as_array().unwrap().len(),
        50,
        "unified search caps each kind"
    );
}

// --- classifier thresholds and malformed answers ---------------------------

#[actix_web::test]
async fn below_threshold_suggestions_are_discarded() {
    let self_url = start_mock_model();
    let app = init_app(&self_url).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    let (status, body) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/x-sentry-envelope",
            envelope(&[event(
                &hex_id(80),
                "low confidence case",
                &["low-confidence-group"],
            )]),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ingest failed: {body}");

    let issue = wait_for_issue_count(
        &app,
        token,
        project,
        "IntegrationError: low confidence case",
        1,
    )
    .await;
    let issue_id = issue["id"].as_str().unwrap().to_string();

    // The mock answers immediately; give the worker ample time to discard it.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let issue = issues_for(&app, token, project)
        .await
        .into_iter()
        .find(|candidate| candidate["id"] == issue_id.as_str())
        .expect("the ingested issue");
    assert!(
        issue["suggestion"].is_null(),
        "a below-threshold answer must not be stored: {issue}"
    );
}

#[actix_web::test]
async fn unknown_or_malformed_model_answers_leave_no_suggestion() {
    let self_url = start_mock_model();
    let app = init_app(&self_url).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    let (status, body) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/x-sentry-envelope",
            envelope(&[
                event(&hex_id(81), "unknown category case", &["unknown-category"]),
                event(
                    &hex_id(82),
                    "malformed response case",
                    &["malformed-response"],
                ),
            ]),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ingest failed: {body}");

    for title in [
        "IntegrationError: unknown category case",
        "IntegrationError: malformed response case",
    ] {
        wait_for_issue_count(&app, token, project, title, 1).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;

    let issues = issues_for(&app, token, project).await;
    for title in [
        "IntegrationError: unknown category case",
        "IntegrationError: malformed response case",
    ] {
        let issue = issues
            .iter()
            .find(|issue| issue["title"] == title)
            .expect("the ingested issue");
        assert!(
            issue["suggestion"].is_null(),
            "{title} must not gain a suggestion"
        );
    }
}

#[actix_web::test]
async fn settings_never_return_the_model_api_key() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    let (status, body) = call_json(
        &app,
        put_json(
            &format!("/api/projects/{project}/settings"),
            token,
            json!({
                "retention_days": 30,
                "model": {
                    "endpoint": "https://models.example.com/v1",
                    "model": "jev-latest",
                    "api_key": "super-secret-token",
                    "confidence_threshold": 0.7
                }
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "settings update failed: {body}");

    for uri in [
        format!("/api/projects/{project}/settings"),
        format!("/api/projects/{project}"),
    ] {
        let (status, body) = call_json(&app, get(&uri, token)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !body.to_string().contains("super-secret-token"),
            "the api key must never be returned: {body}"
        );
    }

    let (status, settings) = call_json(
        &app,
        get(&format!("/api/projects/{project}/settings"), token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["model_api_key_configured"], true);
    assert_eq!(settings["project"]["model"]["confidence_threshold"], 0.7);

    let (status, body) = call_json(
        &app,
        put_json(
            &format!("/api/projects/{project}/settings"),
            token,
            json!({
                "retention_days": 30,
                "model": {
                    "endpoint": "https://models.example.com/v1",
                    "model": "jev-latest",
                    "confidence_threshold": 1.5
                }
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}
