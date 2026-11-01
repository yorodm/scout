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
    state_for_with(self_url, max_in_flight, true).await
}

async fn state_for_with(
    self_url: &str,
    max_in_flight: usize,
    compat_enabled: bool,
) -> web::Data<AppState> {
    let objects = crate::objects::Objects::memory();
    let (seeded, raws) = crate::store::seed(self_url);
    for (key, data) in raws {
        objects
            .put_create(&key, data)
            .await
            .expect("seed raw payload");
    }
    state_with_objects(self_url, max_in_flight, compat_enabled, objects, seeded).await
}

/// Build state around a caller-provided object store so the storage-failure path
/// can be exercised against an unreachable endpoint, not just the metadata flag.
async fn state_with_objects(
    self_url: &str,
    max_in_flight: usize,
    compat_enabled: bool,
    objects: crate::objects::Objects,
    seeded: crate::store::Store,
) -> web::Data<AppState> {
    let store = Arc::new(RwLock::new(seeded));
    let (jobs, rx) = mpsc::unbounded_channel::<Job>();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("build test http client");
    crate::worker::spawn(
        store.clone(),
        rx,
        http,
        Arc::new(crate::metrics::Metrics::new()),
    );
    web::Data::new(AppState {
        store,
        jobs,
        self_url: self_url.to_string(),
        ingest_gate: Arc::new(Semaphore::new(max_in_flight.max(1))),
        session_ttl_days: 7,
        session_idle_hours: 12,
        retention_days: 30,
        confidence_threshold: 0.5,
        objects: Arc::new(objects),
        storage_ready: Arc::new(AtomicBool::new(true)),
        compat_enabled,
        metrics: Arc::new(crate::metrics::Metrics::new()),
        store_kind: "memory",
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
            .configure(crate::compat::config)
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

/// Mirrors a deployment with `SCOUT_DEMO_MODE=true`.
async fn init_app_with_demo(
    state: web::Data<AppState>,
) -> impl Service<Request, Response = ServiceResponse<BoxBody>, Error = Error> {
    test::init_service(
        App::new()
            .app_data(state)
            .configure(crate::api::config)
            .configure(crate::api::demo_config)
            .configure(crate::ingest::config),
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

fn get_public(uri: &str) -> Request {
    test::TestRequest::get().uri(uri).to_request()
}

fn post_public(uri: &str, body: Value) -> Request {
    test::TestRequest::post()
        .uri(uri)
        .set_json(body)
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
    let (status, body) = login_with(app, email, crate::store::DEMO_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "login for {email} failed: {body}");
    body
}

async fn login_with<S, B>(app: &S, email: &str, password: &str) -> (StatusCode, Value)
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    let req = test::TestRequest::post()
        .uri("/api/login")
        .set_json(json!({ "email": email, "password": password }))
        .to_request();
    call_json(app, req).await
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
        .values_mut()
        .find(|key| key.key_hash == crate::auth::hash_token("scout_demo_key"))
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
    let key = body["key"].as_str().unwrap().to_string();
    let key_id = body["id"].as_str().unwrap().to_string();
    assert!(key.starts_with("scout_"));
    assert_eq!(body["key_prefix"], key.chars().take(12).collect::<String>());
    assert!(
        body.get("key_hash").is_none(),
        "hashes must never be returned"
    );

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
async fn unreachable_object_store_returns_503_and_acknowledges_nothing() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("loopback address").port();
    drop(listener);
    let objects = crate::objects::S3Settings::new(
        format!("http://127.0.0.1:{port}"),
        "scout-raw".into(),
        "garage".into(),
        "probe-key".into(),
        "probe-secret".into(),
    )
    .expect("settings")
    .build()
    .expect("build store");

    let (seeded, _) = crate::store::seed("http://127.0.0.1:1");
    let state = state_with_objects("http://127.0.0.1:1", 256, true, objects, seeded).await;
    let app = init_app_with_state(state.clone()).await;
    let before = state.store.read().await.events.len();

    let req = ingest_request(
        "2",
        "scout_platform_key",
        "application/x-sentry-envelope",
        envelope(&[event(&hex_id(92), "garage unreachable", &["garage-group"])]),
        "envelope",
    );
    let (status, body) = call_json(&app, req).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {body}");
    assert_eq!(
        state.store.read().await.events.len(),
        before,
        "an unstored event must not be acknowledged"
    );

    let (health, health_body) = call_json(
        &app,
        test::TestRequest::get().uri("/api/health").to_request(),
    )
    .await;
    assert_eq!(health, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(health_body["status"], "degraded");
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
        55,
        "unified search returns every match below the combined cap"
    );
    assert_eq!(body["has_more"], false);
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

// --- convergence: safety, scoping, and storage ------------------------------

#[actix_web::test]
async fn demo_accounts_and_projects_are_confined_and_a_reset_preserves_real_data() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_demo(state.clone()).await;
    bootstrap_operator(&state).await;
    let (_, operator) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    let operator_token = operator["token"].as_str().expect("operator session");

    // A real organisation and project, alongside the seeded demo organisation.
    let (status, org_body) = call_json(
        &app,
        post_json(
            "/api/orgs",
            operator_token,
            json!({ "name": "Real Co", "initial_admin_email": "admin@real.test" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create org failed: {org_body}");
    let real_org = org_body["organisation"]["id"].as_str().unwrap().to_string();
    let setup_token = org_body["initial_admin_setup_token"].as_str().unwrap();
    let (_, accepted) = call_json(
        &app,
        post_public(
            &format!("/api/invitations/{setup_token}/accept"),
            json!({ "name": "Real Admin", "password": "real-admin-pw-1" }),
        ),
    )
    .await;
    let real_admin_token = accepted["token"].as_str().expect("real admin session");
    let (status, project_body) = call_json(
        &app,
        post_json(
            &format!("/api/orgs/{real_org}/projects"),
            operator_token,
            json!({
                "name": "real-app",
                "slug": "real-app",
                "initial_owner_email": "admin@real.test",
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create project failed: {project_body}"
    );
    let real_project = project_body["project"]["id"].as_str().unwrap().to_string();

    // T048: the organisation list exposes the slug and the demo marker.
    let (status, orgs) = call_json(&app, get("/api/orgs", operator_token)).await;
    assert_eq!(status, StatusCode::OK);
    let listed = orgs["organisations"].as_array().expect("organisations");
    let real = listed.iter().find(|org| org["name"] == "Real Co").unwrap();
    assert_eq!(real["is_demo"], false);
    assert_eq!(real["slug"], "real-co");
    let demo = listed.iter().find(|org| org["name"] == "Acme").unwrap();
    assert_eq!(demo["is_demo"], true);
    assert_eq!(demo["slug"], "acme");

    // SC-014: demo accounts list only demo-organisation members.
    let (status, accounts) = call_json(&app, get_public("/api/demo/accounts")).await;
    assert_eq!(status, StatusCode::OK);
    let emails: Vec<&str> = accounts["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|account| account["email"].as_str().unwrap())
        .collect();
    assert!(emails.contains(&"demo@scout.local"));
    assert!(!emails.contains(&"admin@real.test"));
    assert!(!emails.contains(&"ops@test.local"));

    // A demo session lists only demo projects and cannot reach the real one.
    let demo_auth = login(&app, "demo@scout.local").await;
    let demo_token = demo_auth["token"].as_str().unwrap();
    let (status, projects) = call_json(&app, get("/api/projects", demo_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|project| project["id"] != real_project.as_str()));
    assert!(demo_auth["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .all(|membership| membership["project_id"] != real_project.as_str()));
    let (status, _) = call_json(
        &app,
        get(&format!("/api/projects/{real_project}/issues"), demo_token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a demo session must not learn that the real project exists"
    );

    // A reset replaces demo rows only; real rows and the operator survive.
    let (status, _) = call_json(&app, post_public("/api/demo/reset", json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    {
        let store = state.store.read().await;
        assert!(store
            .orgs
            .values()
            .any(|org| org.id.to_string() == real_org));
        assert!(store
            .projects
            .values()
            .any(|project| project.id.to_string() == real_project));
        assert!(store.users.values().any(|user| user.is_operator));
        assert!(store
            .users
            .values()
            .any(|user| user.email == "admin@real.test"));
        assert!(store.orgs.values().any(|org| org.is_demo));
    }
    // The real admin still reads its project after the reset.
    let (status, _) = call_json(
        &app,
        get(
            &format!("/api/projects/{real_project}/issues"),
            real_admin_token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[actix_web::test]
async fn the_operator_is_denied_every_project_scoped_route() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    bootstrap_operator(&state).await;

    // Give the operator an owner membership on purpose: the denial must come from
    // the operator's deployment-level scope, not from a missing membership row.
    let (project_id, issue_id) = {
        let store = state.store.read().await;
        let project = store.projects.values().next().expect("a seeded project");
        let issue = store
            .issues
            .values()
            .find(|issue| issue.project_id == project.id)
            .expect("a seeded issue");
        (project.id, issue.id)
    };
    {
        let mut store = state.store.write().await;
        let operator_id = store
            .users
            .values()
            .find(|user| user.is_operator)
            .expect("the operator")
            .id;
        store
            .memberships
            .insert((operator_id, project_id), crate::models::Role::Owner);
    }

    let (_, auth) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    let token = auth["token"].as_str().expect("operator session");

    for path in [
        format!("/api/projects/{project_id}"),
        format!("/api/projects/{project_id}/issues"),
        format!("/api/projects/{project_id}/logs"),
        format!("/api/projects/{project_id}/search?q=error"),
        format!("/api/projects/{project_id}/keys"),
        format!("/api/projects/{project_id}/settings"),
        format!("/api/projects/{project_id}/members"),
        format!("/api/issues/{issue_id}"),
        format!("/api/issues/{issue_id}/occurrences"),
        format!("/api/issues/{issue_id}/raw"),
    ] {
        let (status, body) = call_json(&app, get(&path, token)).await;
        assert!(
            matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
            "{path} answered {status}: {body}"
        );
    }

    // Writing a credential is denied just as hard as reading telemetry.
    let (status, _) = call_json(
        &app,
        post_json(
            &format!("/api/projects/{project_id}/keys"),
            token,
            json!({ "label": "operator" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Deployment-level routes keep working.
    let (status, body) = call_json(
        &app,
        post_json(
            "/api/orgs",
            token,
            json!({ "name": "Ops Co", "initial_admin_email": "admin@ops.test" }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "operator provisioning broke: {body}"
    );
    let (status, _) = call_json(&app, get("/api/metrics", token)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call_json(&app, get("/api/operator/users?locked=true", token)).await;
    assert_eq!(status, StatusCode::OK);
}

#[actix_web::test]
async fn demo_routes_are_opt_in_only() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;

    // Default (production-shaped) wiring must not expose the demo endpoints.
    for req in [
        test::TestRequest::get()
            .uri("/api/demo/accounts")
            .to_request(),
        test::TestRequest::post()
            .uri("/api/demo/reset")
            .to_request(),
    ] {
        let (status, _) = call_json(&app, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    let demo = init_app_with_demo(state).await;
    let (status, body) = call_json(
        &demo,
        test::TestRequest::get()
            .uri("/api/demo/accounts")
            .to_request(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body["accounts"].as_array().is_some());
    let (status, _) = call_json(
        &demo,
        test::TestRequest::post()
            .uri("/api/demo/reset")
            .to_request(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[actix_web::test]
async fn search_matches_issue_fingerprint_and_log_level() {
    let app = init_app("http://127.0.0.1:1").await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;

    let issues = issues_for(&app, token, project).await;
    let fingerprint = issues[0]["fingerprint"].as_str().unwrap().to_string();
    let (status, body) = call_json(
        &app,
        get(
            &format!("/api/projects/{project}/search?q={fingerprint}"),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["fingerprint"] == fingerprint),
        "FR-012 search must match the issue fingerprint"
    );

    let (status, body) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/json",
            json!({ "message": "level search marker", "level": "critical", "source": "probe" })
                .to_string()
                .into_bytes(),
            "logs",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "log ingest failed: {body}");

    let (status, body) = call_json(
        &app,
        get(&format!("/api/projects/{project}/search?q=critical"), token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|log| log["level"] == "critical"),
        "FR-012 search must match the log level"
    );
}

#[actix_web::test]
async fn expired_sessions_are_swept_on_login() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;

    let expired = crate::auth::new_token();
    let expired_hash = crate::auth::hash_token(&expired);
    state.store.write().await.sessions.insert(
        expired_hash.clone(),
        crate::models::Session {
            user_id: Uuid::new_v4(),
            expires_at: chrono::Utc::now() - chrono::Duration::seconds(1),
            last_seen_at: chrono::Utc::now() - chrono::Duration::seconds(2),
        },
    );
    assert!(state
        .store
        .read()
        .await
        .sessions
        .contains_key(&expired_hash));

    login(&app, "demo@scout.local").await;

    assert!(
        !state
            .store
            .read()
            .await
            .sessions
            .contains_key(&expired_hash),
        "an expired session must be removed on the next sign-in"
    );
}

#[actix_web::test]
async fn attachment_bytes_are_written_once_to_object_storage() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "platform-api").await;

    let payload = b"panic log body";
    let mut raw = Vec::new();
    raw.extend_from_slice(json!({ "event_id": hex_id(95) }).to_string().as_bytes());
    raw.push(b'\n');
    raw.extend_from_slice(br#"{"type":"event"}"#);
    raw.push(b'\n');
    raw.extend_from_slice(
        event(
            &hex_id(95),
            "attachment object check",
            &["attachment-object"],
        )
        .to_string()
        .as_bytes(),
    );
    raw.push(b'\n');
    raw.extend_from_slice(
        format!(
            r#"{{"type":"attachment","length":{},"filename":"panic.log","content_type":"text/plain"}}"#,
            payload.len()
        )
        .as_bytes(),
    );
    raw.push(b'\n');
    raw.extend_from_slice(payload);
    raw.push(b'\n');

    let (status, body) = call_json(
        &app,
        ingest_request(
            "2",
            "scout_platform_key",
            "application/x-sentry-envelope",
            raw,
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ingest failed: {body}");

    let issue = wait_for_issue_count(
        &app,
        token,
        project,
        "IntegrationError: attachment object check",
        1,
    )
    .await;
    let last_event = Uuid::parse_str(issue["last_event_id"].as_str().unwrap()).unwrap();

    let attachment = {
        let store = state.store.read().await;
        let event = store.events.get(&last_event).expect("the stored event");
        assert_eq!(event.attachments.len(), 1);
        let attachment = &event.attachments[0];
        assert!(
            attachment.data.is_empty(),
            "bytes belong to the object store, not the event"
        );
        assert_eq!(attachment.size, payload.len() as u64);
        attachment.clone()
    };
    let key = attachment.object_key.expect("an object key");
    assert_eq!(
        state.objects.get(&key).await.unwrap(),
        payload.to_vec(),
        "the object store holds the exact attachment bytes"
    );

    // Raw payloads are write-once: a second write to the same key must fail and
    // must not change the stored bytes.
    let raw_key = state.store.read().await.events[&last_event]
        .raw_object_key
        .clone()
        .expect("a raw object");
    let original = state.objects.get(&raw_key).await.unwrap();
    assert!(matches!(
        state.objects.put_create(&raw_key, b"forged".to_vec()).await,
        Err(crate::objects::ObjectError::AlreadyExists(_))
    ));
    assert_eq!(
        state.objects.get(&raw_key).await.unwrap(),
        original,
        "an existing raw payload can never be rewritten"
    );
}

// --- retention --------------------------------------------------------------

#[actix_web::test]
async fn retention_expires_raw_payloads_and_keeps_issue_metadata() {
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
    let issue_id = Uuid::parse_str(issue["id"].as_str().unwrap()).unwrap();
    let event_id = Uuid::parse_str(issue["last_event_id"].as_str().unwrap()).unwrap();

    let (raw_key, attachment_key) = {
        let store = state.store.read().await;
        let event = store.events.get(&event_id).expect("the seeded event");
        assert_eq!(event.attachment_count, 1);
        (
            event.raw_object_key.clone().expect("a raw object"),
            event.attachments[0]
                .object_key
                .clone()
                .expect("an attachment object"),
        )
    };
    assert!(state.objects.get(&raw_key).await.is_ok());
    assert!(state.objects.get(&attachment_key).await.is_ok());

    // Age the event past the 30-day window and run the same sweep the worker runs.
    let keys = {
        let mut store = state.store.write().await;
        let project_model = store.projects.get(&project).cloned().unwrap();
        store.events.get_mut(&event_id).unwrap().received_at =
            chrono::Utc::now() - chrono::Duration::days(40);
        store.apply_retention(&project_model)
    };
    assert!(keys.contains(&raw_key));
    assert!(keys.contains(&attachment_key));
    for key in &keys {
        let _ = state.objects.delete(key).await;
    }

    {
        let store = state.store.read().await;
        let event = store
            .events
            .get(&event_id)
            .expect("event metadata survives retention");
        assert!(event.raw_object_key.is_none());
        assert!(event.raw_expired_at.is_some());
        assert!(event.attachments.is_empty());
        assert_eq!(event.attachment_count, 1, "the historical count is kept");
        assert_eq!(store.issues[&issue_id].count, 1284, "issue counters stay");
    }
    assert!(matches!(
        state.objects.get(&raw_key).await,
        Err(crate::objects::ObjectError::NotFound(_))
    ));

    let (status, detail) = call_json(&app, get(&format!("/api/issues/{issue_id}"), token)).await;
    assert_eq!(status, StatusCode::OK, "detail stays readable: {detail}");
    assert_eq!(detail["raw_available"], false);
    assert_eq!(detail["event"]["attachment_count"], 1);
    assert!(detail["attachments"].as_array().unwrap().is_empty());

    let (status, body) = call_json(&app, get(&format!("/api/issues/{issue_id}/raw"), token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap_or("").contains("expired"));

    // A retried delivery after expiry is still deduplicated.
    let sentry_id = state.store.read().await.events[&event_id]
        .sentry_event_id
        .clone();
    let (status, body) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/x-sentry-envelope",
            envelope(&[event(&sentry_id, "retention retry", &["retention-group"])]),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "retry failed: {body}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        state.store.read().await.issues[&issue_id].count,
        1284,
        "a retry after expiry must not inflate the count"
    );
}

#[actix_web::test]
async fn retention_is_per_project_and_covers_logs() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let now = chrono::Utc::now();
    let (web, platform, web_event, platform_event) = {
        let store = state.store.read().await;
        let web = store
            .projects
            .values()
            .find(|project| project.name == "scout-web")
            .unwrap()
            .id;
        let platform = store
            .projects
            .values()
            .find(|project| project.name == "platform-api")
            .unwrap()
            .id;
        let web_event = store
            .issues
            .values()
            .find(|issue| issue.project_id == web && issue.title.starts_with("TypeError"))
            .unwrap()
            .last_event_id;
        let platform_event = store
            .issues
            .values()
            .find(|issue| issue.project_id == platform)
            .unwrap()
            .last_event_id;
        (web, platform, web_event, platform_event)
    };

    let (web_log, platform_log) = (Uuid::new_v4(), Uuid::new_v4());
    {
        let mut store = state.store.write().await;
        for event_id in [web_event, platform_event] {
            store.events.get_mut(&event_id).unwrap().received_at = now - chrono::Duration::days(20);
        }
        for (id, project_id, age, message) in [
            (web_log, web, 40, "web retention log"),
            (platform_log, platform, 20, "platform retention log"),
        ] {
            store.logs.push(crate::models::LogLine {
                id,
                project_id,
                timestamp: now - chrono::Duration::days(age),
                level: "info".into(),
                message: message.into(),
                source: "retention-test".into(),
                attributes: json!({}),
                received_at: now - chrono::Duration::days(age),
            });
        }
    }

    // scout-web retains for 30 days: 20 days is inside, the 40-day log expires.
    let web_keys = {
        let mut store = state.store.write().await;
        let project = store.projects.get(&web).cloned().unwrap();
        store.apply_retention(&project)
    };
    assert!(
        web_keys.is_empty(),
        "the 20-day event is still inside retention"
    );
    {
        let store = state.store.read().await;
        assert!(store.events[&web_event].raw_object_key.is_some());
        assert!(!store.logs.iter().any(|log| log.id == web_log));
        assert!(store.logs.iter().any(|log| log.id == platform_log));
    }

    // platform-api retains for 14 days: its 20-day event and log expire now.
    let platform_keys = {
        let mut store = state.store.write().await;
        let project = store.projects.get(&platform).cloned().unwrap();
        store.apply_retention(&project)
    };
    assert!(!platform_keys.is_empty());
    {
        let store = state.store.read().await;
        assert!(store.events[&platform_event].raw_object_key.is_none());
        assert!(store.events[&platform_event].raw_expired_at.is_some());
        assert!(store.events[&web_event].raw_object_key.is_some());
        assert!(!store.logs.iter().any(|log| log.id == platform_log));
    }
}

/// FR-010: retention is measured from the server's receipt time. A client clock
/// that is wildly wrong can neither keep an old record alive nor expire a new one.
#[actix_web::test]
async fn retention_follows_server_receipt_time_not_client_clocks() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let now = chrono::Utc::now();
    let project = {
        let store = state.store.read().await;
        store
            .projects
            .values()
            .find(|project| project.name == "scout-web")
            .cloned()
            .expect("seeded project")
    };

    let (past_client, future_client, fresh_client) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (past_event, future_event) = (Uuid::new_v4(), Uuid::new_v4());
    {
        let mut store = state.store.write().await;
        for (id, message, timestamp, received_at) in [
            // Claimed to be old, but received now: it stays.
            (
                past_client,
                "slow client clock",
                now - chrono::Duration::days(400),
                now,
            ),
            // Claimed to be in the future, but received long ago: it goes.
            (
                future_client,
                "fast client clock",
                now + chrono::Duration::days(400),
                now - chrono::Duration::days(40),
            ),
            (fresh_client, "honest clock", now, now),
        ] {
            store.logs.push(crate::models::LogLine {
                id,
                project_id: project.id,
                timestamp,
                level: "info".into(),
                message: message.into(),
                source: "clock-test".into(),
                attributes: json!({}),
                received_at,
            });
        }
        for (id, sentry_event_id, received_at) in [
            (past_event, "past-client-event", now),
            (
                future_event,
                "future-client-event",
                now - chrono::Duration::days(40),
            ),
        ] {
            store.events.insert(
                id,
                crate::models::Event {
                    id,
                    sentry_event_id: sentry_event_id.into(),
                    project_id: project.id,
                    issue_id: None,
                    received_at,
                    raw_object_key: Some(format!("raw/{sentry_event_id}")),
                    raw_size: 1,
                    raw_expired_at: None,
                    attachment_count: 0,
                    event: json!({ "timestamp": now + chrono::Duration::days(400) }),
                    attachments: Vec::new(),
                },
            );
        }
    }

    {
        let mut store = state.store.write().await;
        let project = store.projects.get(&project.id).cloned().unwrap();
        store.apply_retention(&project);
    }

    let store = state.store.read().await;
    assert!(
        store.logs.iter().any(|log| log.id == past_client),
        "a client timestamp in the past must not expire a newly received log"
    );
    assert!(
        !store.logs.iter().any(|log| log.id == future_client),
        "a client timestamp in the future must not keep an old log alive"
    );
    assert!(store.logs.iter().any(|log| log.id == fresh_client));
    assert!(
        store.events[&past_event].raw_object_key.is_some(),
        "a newly received event survives regardless of its client timestamp"
    );
    assert!(
        store.events[&future_event].raw_object_key.is_none(),
        "an old event expires even when its client timestamp is in the future"
    );
}

// --- project soft-delete ----------------------------------------------------

#[actix_web::test]
async fn soft_deleted_project_is_hidden_and_rejects_ingest() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().unwrap();
    let project = project_id(&app, token, "scout-web").await;
    let issue = first_open_issue(&app, token, project).await;
    let issue_id = issue["id"].as_str().unwrap().to_string();
    let event_id = Uuid::parse_str(issue["last_event_id"].as_str().unwrap()).unwrap();

    let (raw_key, events_before, issues_before, logs_before) = {
        let store = state.store.read().await;
        let raw_key = store.events[&event_id]
            .raw_object_key
            .clone()
            .expect("a raw object");
        (
            raw_key,
            store.events.len(),
            store.issues.len(),
            store.logs.len(),
        )
    };

    // A triager cannot delete a project.
    let triager = login(&app, "triager@scout.local").await;
    let triager_token = triager["token"].as_str().unwrap();
    let (status, _) = call_json(
        &app,
        delete(&format!("/api/projects/{project}"), triager_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = call_json(&app, delete(&format!("/api/projects/{project}"), token)).await;
    assert_eq!(status, StatusCode::OK, "delete failed: {body}");

    // Hidden from every normal read.
    let (status, body) = call_json(&app, get("/api/projects", token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|candidate| candidate["id"] == project.to_string()),
        "a soft-deleted project must disappear from the project list"
    );
    for uri in [
        format!("/api/projects/{project}"),
        format!("/api/projects/{project}/settings"),
        format!("/api/projects/{project}/issues"),
        format!("/api/issues/{issue_id}"),
    ] {
        let (status, _) = call_json(&app, get(&uri, token)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri} must be hidden");
    }

    // Ingest is rejected with the same body as any other bad key.
    let (status, body) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/x-sentry-envelope",
            envelope(&[event(&hex_id(97), "deleted project", &["deleted-group"])]),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "invalid or missing ingest key");

    // Nothing was deleted; the data stays subject to retention.
    {
        let store = state.store.read().await;
        assert!(store.projects[&project].deleted_at.is_some());
        assert_eq!(store.events.len(), events_before);
        assert_eq!(store.issues.len(), issues_before);
        assert_eq!(store.logs.len(), logs_before);
        assert!(store.audit.iter().any(
            |entry| entry.action == "project.soft_deleted" && entry.project_id == Some(project)
        ));
    }
    assert!(state.objects.get(&raw_key).await.is_ok());

    let keys = {
        let mut store = state.store.write().await;
        let deleted = store.projects[&project].clone();
        store.events.get_mut(&event_id).unwrap().received_at =
            chrono::Utc::now() - chrono::Duration::days(40);
        store.apply_retention(&deleted)
    };
    assert!(
        keys.contains(&raw_key),
        "retention still applies to a soft-deleted project"
    );

    // Deleting again is a 404, not a second audit row.
    let (status, _) = call_json(&app, delete(&format!("/api/projects/{project}"), token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- provisioning, invitations, and recovery codes --------------------------

const OPERATOR_PASSWORD: &str = "operator-secret";

fn operator_config() -> crate::config::Config {
    crate::config::Config {
        port: 8080,
        self_url: "http://127.0.0.1:1".into(),
        ingest_max_in_flight: 256,
        retention_days: 30,
        confidence_threshold: 0.5,
        session_ttl_days: 7,
        session_idle_hours: 12,
        operator_email: Some("ops@test.local".into()),
        operator_password: Some(OPERATOR_PASSWORD.into()),
        database_url: None,
        store: None,
        retention_interval_secs: 3600,
        s3_endpoint: None,
        s3_bucket: None,
        demo_mode: false,
        sentry_compat_enabled: false,
    }
}

async fn bootstrap_operator(state: &web::Data<AppState>) {
    let mut store = state.store.write().await;
    crate::auth::bootstrap_operator(&mut store, &operator_config()).expect("bootstrap operator");
}

#[actix_web::test]
async fn operator_provisions_an_org_and_its_initial_admin() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    bootstrap_operator(&state).await;

    let (status, auth) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "operator login failed: {auth}");
    assert_eq!(auth["user"]["is_operator"], true);
    let operator_token = auth["token"].as_str().unwrap();

    let (status, body) = call_json(
        &app,
        post_json(
            "/api/orgs",
            operator_token,
            json!({ "name": "Beta", "initial_admin_email": "admin@beta.test" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create org failed: {body}");
    let org_id = body["organisation"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        body["organisation"]["created_by"], auth["user"]["id"],
        "the creating operator must be recorded"
    );
    let setup_token = body["initial_admin_setup_token"]
        .as_str()
        .expect("a setup token")
        .to_string();

    let (status, preview) =
        call_json(&app, get_public(&format!("/api/invitations/{setup_token}"))).await;
    assert_eq!(status, StatusCode::OK, "preview failed: {preview}");
    assert_eq!(preview["organisation"], "Beta");
    assert_eq!(preview["email"], "admin@beta.test");

    let (status, accepted) = call_json(
        &app,
        post_public(
            &format!("/api/invitations/{setup_token}/accept"),
            json!({ "name": "Beta Admin", "password": "beta-admin-pw-1" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "accept failed: {accepted}");
    assert_eq!(accepted["user"]["email"], "admin@beta.test");
    assert_eq!(accepted["recovery_codes"].as_array().unwrap().len(), 10);
    let admin_token = accepted["token"]
        .as_str()
        .expect("a session for the new admin")
        .to_string();

    // Setup tokens are single use.
    let (status, _) = call_json(
        &app,
        post_public(
            &format!("/api/invitations/{setup_token}/accept"),
            json!({ "name": "Again", "password": "again-admin-pw-1" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);

    // The initial admin signs in; the operator still creates the first project.
    let (status, _) = login_with(&app, "admin@beta.test", "beta-admin-pw-1").await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/orgs/{org_id}/projects"),
            operator_token,
            json!({
                "name": "billing",
                "slug": "billing",
                "initial_owner_email": "admin@beta.test",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create project failed: {body}");
    assert_eq!(body["project"]["retention_days"], 30);
    assert_eq!(body["project"]["confidence_threshold"], 0.5);
    // FR-012: the operator never receives an ingest credential.
    assert!(body.get("key").is_none(), "creation returned a key: {body}");
    assert!(body.get("dsn").is_none(), "creation returned a dsn: {body}");
    let project_id = Uuid::parse_str(body["project"]["id"].as_str().unwrap()).unwrap();
    let dsn = body["project"]["dsn_id"].as_str().unwrap().to_string();

    // The named owner creates the first key and is the only holder of it.
    let (status, key_body) = call_json(
        &app,
        post_json(
            &format!("/api/projects/{project_id}/keys"),
            &admin_token,
            json!({ "label": "Default" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create key failed: {key_body}");
    let key = key_body["key"].as_str().unwrap().to_string();

    // The freshly created key ingests unmodified Sentry traffic.
    let (status, body) = call_json(
        &app,
        ingest_request(
            &dsn,
            &key,
            "application/x-sentry-envelope",
            envelope(&[event(
                &hex_id(98),
                "provisioned path",
                &["provisioned-group"],
            )]),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ingest failed: {body}");
    wait_for_issue_count(
        &app,
        &admin_token,
        project_id,
        "IntegrationError: provisioned path",
        1,
    )
    .await;

    // Only the operator may create organisations.
    let (status, _) = call_json(
        &app,
        post_json(
            "/api/orgs",
            &admin_token,
            json!({ "name": "Nope", "initial_admin_email": "x@y.z" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // FR-034: organisation administrators cannot create projects either.
    let (status, _) = call_json(
        &app,
        post_json(
            &format!("/api/orgs/{org_id}/projects"),
            &admin_token,
            json!({
                "name": "nope",
                "slug": "nope",
                "initial_owner_email": "admin@beta.test",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The named owner must already belong to the organisation.
    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/orgs/{org_id}/projects"),
            operator_token,
            json!({
                "name": "stray",
                "slug": "stray",
                "initial_owner_email": "ops@test.local",
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "non-member owner: {body}");

    // The owner must be named at all.
    let (status, _) = call_json(
        &app,
        post_json(
            &format!("/api/orgs/{org_id}/projects"),
            operator_token,
            json!({ "name": "stray", "slug": "stray" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let store = state.store.read().await;
    assert!(
        !store
            .projects
            .values()
            .any(|project| project.slug == "stray"),
        "a rejected creation must leave nothing behind"
    );
    let owner = store
        .user_by_email("admin@beta.test")
        .map(|user| user.id)
        .expect("the initial owner");
    assert_eq!(
        store.role_for(owner, project_id),
        Some(crate::models::Role::Owner)
    );
    for action in [
        "organisation.created",
        "org_admin_setup.issued",
        "invitation.accepted",
        "project.created",
    ] {
        assert!(
            store.audit.iter().any(|entry| entry.action == action),
            "missing audit row {action}"
        );
    }
}

#[actix_web::test]
async fn existing_account_binds_org_admin_with_a_session_and_keeps_its_password() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    bootstrap_operator(&state).await;
    let (_, operator) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    let operator_token = operator["token"].as_str().unwrap();
    let (status, body) = call_json(
        &app,
        post_json(
            "/api/orgs",
            operator_token,
            json!({ "name": "Gamma", "initial_admin_email": "demo@scout.local" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create org failed: {body}");
    let org_id = Uuid::parse_str(body["organisation"]["id"].as_str().unwrap()).unwrap();
    let setup_token = body["initial_admin_setup_token"]
        .as_str()
        .unwrap()
        .to_string();
    let accept_uri = format!("/api/invitations/{setup_token}/accept");

    let (status, _) = call_json(
        &app,
        post_public(&accept_uri, json!({ "password": "stolen" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an existing account must sign in first"
    );

    let demo = login(&app, "demo@scout.local").await;
    let demo_token = demo["token"].as_str().unwrap();
    let demo_id = Uuid::parse_str(demo["user"]["id"].as_str().unwrap()).unwrap();

    let (status, _) = call_json(
        &app,
        post_json(&accept_uri, demo_token, json!({ "password": "changed" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = call_json(&app, post_json(&accept_uri, demo_token, json!({}))).await;
    assert_eq!(status, StatusCode::OK, "accept failed: {body}");
    assert!(
        body["recovery_codes"].as_array().unwrap().is_empty(),
        "an existing account gets no new recovery-code set"
    );
    assert_eq!(
        state.store.read().await.org_role_for(demo_id, org_id),
        Some(crate::models::OrgRole::Admin)
    );
    let (status, _) = login_with(&app, "demo@scout.local", crate::store::DEMO_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "the password must be unchanged");
}

#[actix_web::test]
async fn project_invitations_preview_accept_and_revoke() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let demo = login(&app, "demo@scout.local").await;
    let token = demo["token"].as_str().unwrap();
    let web = project_id(&app, token, "scout-web").await;
    let org_id = state.store.read().await.projects[&web].org_id;

    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/orgs/{org_id}/invitations"),
            token,
            json!({ "email": "invitee@test.local", "project_id": web, "role": "triager" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "invite failed: {body}");
    let invite_token = body["token"].as_str().unwrap().to_string();

    let (status, preview) = call_json(
        &app,
        get_public(&format!("/api/invitations/{invite_token}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "preview failed: {preview}");
    assert_eq!(preview["role"], "triager");
    assert_eq!(preview["project"], "scout-web");

    let (status, accepted) = call_json(
        &app,
        post_public(
            &format!("/api/invitations/{invite_token}/accept"),
            json!({ "name": "In Vitee", "password": "invitee-pw-12345" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "accept failed: {accepted}");
    let invitee_token = accepted["token"].as_str().unwrap();
    let (status, list) = call_json(&app, get("/api/projects", invitee_token)).await;
    assert_eq!(status, StatusCode::OK);
    let project = list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|project| project["id"] == web.to_string())
        .expect("the invited project");
    assert_eq!(project["role"], "triager");

    // Reuse is rejected.
    let (status, _) = call_json(
        &app,
        post_public(
            &format!("/api/invitations/{invite_token}/accept"),
            json!({ "name": "Again", "password": "again-admin-pw-1" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);

    // Revocation invalidates a pending invitation.
    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/orgs/{org_id}/invitations"),
            token,
            json!({ "email": "revoked@test.local", "project_id": web, "role": "viewer" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "invite failed: {body}");
    let revoked_token = body["token"].as_str().unwrap().to_string();
    let invitation_id = body["invitation"]["id"].as_str().unwrap().to_string();
    let (status, _) = call_json(
        &app,
        delete(
            &format!("/api/orgs/{org_id}/invitations/{invitation_id}"),
            token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call_json(
        &app,
        get_public(&format!("/api/invitations/{revoked_token}")),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
}

#[actix_web::test]
async fn recovery_codes_reset_the_password_and_reissue_revokes_old_codes() {
    let app = init_app("http://127.0.0.1:1").await;
    let demo = login(&app, "demo@scout.local").await;
    let token = demo["token"].as_str().unwrap();

    let (status, body) =
        call_json(&app, post_json("/api/me/recovery-codes", token, json!({}))).await;
    assert_eq!(status, StatusCode::OK, "reissue failed: {body}");
    let codes: Vec<String> = body["recovery_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|code| code.as_str().unwrap().to_string())
        .collect();
    assert_eq!(codes.len(), 10);

    let (status, _) = call_json(
        &app,
        post_public(
            "/api/recover",
            json!({ "email": "demo@scout.local", "recovery_code": codes[0], "new_password": "reset-pw-123456" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = login_with(&app, "demo@scout.local", crate::store::DEMO_PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the old password no longer works"
    );
    let (status, auth) = login_with(&app, "demo@scout.local", "reset-pw-123456").await;
    assert_eq!(status, StatusCode::OK);

    // A used code cannot be reused.
    let (status, _) = call_json(
        &app,
        post_public(
            "/api/recover",
            json!({ "email": "demo@scout.local", "recovery_code": codes[0], "new_password": "again-admin-pw-1" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // An unknown code and an unknown email fail the same way.
    for body in [
        json!({ "email": "demo@scout.local", "recovery_code": "AAAAA-BBBBB", "new_password": "nope-pw-1234567" }),
        json!({ "email": "nobody@test.local", "recovery_code": codes[1], "new_password": "nope-pw-1234567" }),
    ] {
        let (status, _) = call_json(&app, post_public("/api/recover", body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // Reissuing revokes every remaining unused code.
    let token = auth["token"].as_str().unwrap();
    let (status, body) =
        call_json(&app, post_json("/api/me/recovery-codes", token, json!({}))).await;
    assert_eq!(status, StatusCode::OK, "reissue failed: {body}");
    let new_codes: Vec<String> = body["recovery_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|code| code.as_str().unwrap().to_string())
        .collect();
    let (status, _) = call_json(
        &app,
        post_public(
            "/api/recover",
            json!({ "email": "demo@scout.local", "recovery_code": codes[1], "new_password": "third-pw-123456" }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the previous unused set is invalidated"
    );
    let (status, _) = call_json(
        &app,
        post_public(
            "/api/recover",
            json!({ "email": "demo@scout.local", "recovery_code": new_codes[0], "new_password": "third-pw-123456" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[actix_web::test]
async fn operator_changes_its_own_password_and_other_sessions_end() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    bootstrap_operator(&state).await;

    let (_, first) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    let first_token = first["token"].as_str().unwrap().to_string();
    let (_, second) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    let second_token = second["token"].as_str().unwrap().to_string();

    // A value below the minimum is rejected with the minimum named, before the
    // stored credential is touched.
    let (status, body) = call_json(
        &app,
        post_json(
            "/api/me/password",
            &first_token,
            json!({ "current_password": OPERATOR_PASSWORD, "new_password": "short-pw" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("12"),
        "the message must name the minimum: {body}"
    );

    // The current password is required.
    let (status, _) = call_json(
        &app,
        post_json(
            "/api/me/password",
            &first_token,
            json!({ "current_password": "wrong-password", "new_password": "operator-secret-2" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a rejected change leaves the old password usable"
    );

    let new_password = "operator-secret-2";
    let (status, body) = call_json(
        &app,
        post_json(
            "/api/me/password",
            &first_token,
            json!({ "current_password": OPERATOR_PASSWORD, "new_password": new_password }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "change failed: {body}");

    // The calling session keeps working; the other one ends with it.
    let (status, _) = call_json(&app, get("/api/me", &first_token)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call_json(&app, get("/api/me", &second_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Only the new password signs in.
    let (status, _) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, auth) = login_with(&app, "ops@test.local", new_password).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        auth["user"].get("password_hash").is_none(),
        "no response may contain the hash: {auth}"
    );

    let store = state.store.read().await;
    assert!(
        store
            .audit
            .iter()
            .any(|entry| entry.action == "account.password_changed"),
        "the change must be audited"
    );
    let operator_hash = store
        .user_by_email("ops@test.local")
        .and_then(|user| user.password_hash.clone())
        .expect("an operator password hash");
    assert!(operator_hash.starts_with("$argon2"), "stored as Argon2id");
}

#[actix_web::test]
async fn operator_receives_recovery_codes_once_on_first_sign_in() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    bootstrap_operator(&state).await;

    let (status, first) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);
    let codes: Vec<String> = first["recovery_codes"]
        .as_array()
        .expect("codes on the first sign-in")
        .iter()
        .map(|code| code.as_str().unwrap().to_string())
        .collect();
    assert_eq!(codes.len(), 10);

    // The same set is not shown again.
    let (status, second) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["recovery_codes"], json!([]));

    // A non-operator never gets a set from signing in.
    let (_, member) = login_with(&app, "demo@scout.local", crate::store::DEMO_PASSWORD).await;
    assert_eq!(member["recovery_codes"], json!([]));

    // Only hashes are stored, and the codes actually recover the account.
    {
        let store = state.store.read().await;
        let rows: Vec<_> = store
            .recovery_codes
            .iter()
            .filter(|code| {
                code.user_id
                    == second["user"]["id"]
                        .as_str()
                        .unwrap()
                        .parse::<Uuid>()
                        .unwrap()
            })
            .collect();
        assert_eq!(rows.len(), 10);
        for row in rows {
            assert!(!codes.contains(&row.code_hash), "plaintext is never stored");
        }
    }
    let (status, _) = call_json(
        &app,
        post_public(
            "/api/recover",
            json!({ "email": "ops@test.local", "recovery_code": codes[0], "new_password": "operator-recovered" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = login_with(&app, "ops@test.local", "operator-recovered").await;
    assert_eq!(status, StatusCode::OK, "a recovery code restores access");
}

#[actix_web::test]
async fn key_rotation_revokes_the_previous_key() {
    let app = init_app("http://127.0.0.1:1").await;
    let demo = login(&app, "demo@scout.local").await;
    let token = demo["token"].as_str().unwrap();
    let web = project_id(&app, token, "scout-web").await;

    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/projects/{web}/keys/rotate"),
            token,
            json!({ "label": "ci" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "rotation failed: {body}");
    let new_key = body["key"].as_str().unwrap().to_string();

    let (status, _) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/x-sentry-envelope",
            envelope(&[event(&hex_id(99), "rotated old", &["rotate-old"])]),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old key is revoked");

    let (status, body) = call_json(
        &app,
        ingest_request(
            "1",
            &new_key,
            "application/x-sentry-envelope",
            envelope(&[event(&hex_id(100), "rotated new", &["rotate-new"])]),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the new key works: {body}");
}

// --- additional convergence items -------------------------------------------

#[actix_web::test]
async fn api_never_exposes_ingest_key_plaintext() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let demo = login(&app, "demo@scout.local").await;
    let token = demo["token"].as_str().unwrap();
    let web = project_id(&app, token, "scout-web").await;

    for uri in [
        "/api/projects".to_string(),
        format!("/api/projects/{web}"),
        format!("/api/projects/{web}/settings"),
        format!("/api/projects/{web}/keys"),
    ] {
        let (status, body) = call_json(&app, get(&uri, token)).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let text = body.to_string();
        assert!(
            !text.contains("scout_demo_key") && !text.contains("scout_platform_key"),
            "{uri} leaked a plaintext ingest key: {body}"
        );
        assert!(
            !text.contains("key_hash"),
            "{uri} leaked a key hash: {body}"
        );
    }

    let (status, settings) =
        call_json(&app, get(&format!("/api/projects/{web}/settings"), token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        settings["key_prefix"]
            .as_str()
            .unwrap_or("")
            .starts_with("scout_"),
        "the prefix remains available for identification: {settings}"
    );

    // Seeded keys still authenticate ingest through the hash lookup.
    let (status, body) = call_json(
        &app,
        ingest_request(
            "1",
            "scout_demo_key",
            "application/x-sentry-envelope",
            envelope(&[event(&hex_id(120), "hashed key works", &["hashed-key"])]),
            "envelope",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "hashed key lookup failed: {body}");

    // Creation reveals the plaintext once and never the hash.
    let (status, created) = call_json(
        &app,
        post_json(
            &format!("/api/projects/{web}/keys"),
            token,
            json!({ "label": "ci" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create key failed: {created}");
    let plaintext = created["key"].as_str().expect("plaintext once").to_string();
    assert!(plaintext.starts_with("scout_"));
    assert!(created.get("key_hash").is_none());

    let (status, listed) = call_json(&app, get(&format!("/api/projects/{web}/keys"), token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !listed.to_string().contains(&plaintext),
        "the plaintext must never be retrievable again: {listed}"
    );
    assert!(listed["keys"]
        .as_array()
        .unwrap()
        .iter()
        .any(|key| key["key_prefix"] == created["key_prefix"]));
}

#[actix_web::test]
async fn disabled_account_cannot_sign_in() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;

    {
        let mut store = state.store.write().await;
        let user_id = store
            .user_by_email("demo@scout.local")
            .expect("demo account")
            .id;
        store.users.get_mut(&user_id).unwrap().disabled_at = Some(chrono::Utc::now());
    }

    let (status, body) = login_with(&app, "demo@scout.local", crate::store::DEMO_PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
    assert!(body["error"].as_str().unwrap_or("").contains("disabled"));

    {
        let mut store = state.store.write().await;
        let user_id = store
            .user_by_email("demo@scout.local")
            .expect("demo account")
            .id;
        store.users.get_mut(&user_id).unwrap().disabled_at = None;
    }
    let (status, _) = login_with(&app, "demo@scout.local", crate::store::DEMO_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);
}

#[actix_web::test]
async fn retention_sweep_also_removes_expired_sessions() {
    let state = state_for("http://127.0.0.1:1", 256).await;

    let expired = crate::auth::new_token();
    let expired_hash = crate::auth::hash_token(&expired);
    state.store.write().await.sessions.insert(
        expired_hash.clone(),
        crate::models::Session {
            user_id: Uuid::new_v4(),
            expires_at: chrono::Utc::now() - chrono::Duration::seconds(1),
            last_seen_at: chrono::Utc::now() - chrono::Duration::seconds(2),
        },
    );

    let (sessions_removed, _objects_removed, _logs_removed) =
        crate::worker::sweep_once(&state.store, &state.objects).await;

    assert_eq!(sessions_removed, 1);
    assert!(!state
        .store
        .read()
        .await
        .sessions
        .contains_key(&expired_hash));
}

// --- Sentry read-API compatibility -----------------------------------------

const COMPAT_ORG: &str = "acme";
const COMPAT_PROJECT: &str = "scout-web";

async fn compat_call<S, B>(
    app: &S,
    req: Request,
) -> (StatusCode, Value, actix_web::http::header::HeaderMap)
where
    S: Service<Request, Response = ServiceResponse<B>, Error = Error>,
    B: MessageBody,
{
    let res = call(app, req).await;
    let status = res.status();
    let headers = res.headers().clone();
    let body = test::read_body(res).await;
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, value, headers)
}

async fn compat_token(state: &web::Data<AppState>, email: &str) -> String {
    let mut store = state.store.write().await;
    let user_id = store
        .users
        .values()
        .find(|user| user.email == email)
        .map(|user| user.id)
        .unwrap_or_else(|| panic!("seeded user {email} is missing"));
    let (plaintext, _) = store.create_api_token(user_id, "test token");
    plaintext
}

fn issues_uri(project: &str) -> String {
    format!("/api/0/projects/{COMPAT_ORG}/{project}/issues/")
}

fn header_value<'a>(headers: &'a actix_web::http::header::HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

fn link_cursor(link: &str) -> Option<String> {
    for part in link.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("cursor=") {
            return Some(rest.trim_matches('"').to_string());
        }
    }
    None
}

/// Insert synthetic issues directly into the store so pagination tests do not
/// depend on ingest timing.
async fn seed_synthetic_issues(state: &web::Data<AppState>, count: usize, tag: &str) -> Vec<Uuid> {
    let mut store = state.store.write().await;
    let project_id = store
        .projects
        .values()
        .find(|project| project.slug == COMPAT_PROJECT)
        .map(|project| project.id)
        .expect("seeded project is missing");
    let base = chrono::Utc::now();
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let id = Uuid::new_v4();
        let number = store.next_issue_number(project_id);
        let issue = crate::models::Issue {
            id,
            project_id,
            number,
            fingerprint: format!("synthetic-{tag}-{index}"),
            title: format!("synthetic issue {index}"),
            culprit: Some("src/synthetic.rs".into()),
            level: "error".into(),
            first_seen: base - chrono::Duration::seconds(index as i64 + 10),
            last_seen: base - chrono::Duration::seconds(index as i64),
            count: index as u64 + 1,
            state: crate::models::IssueState::Open,
            release: Some("web@1.0.0".into()),
            environment: Some("production".into()),
            tags: json!({ "synthetic": tag, "release": "web@1.0.0" }),
            frames: Vec::new(),
            breadcrumbs: Vec::new(),
            suggestion: None,
            decision: None,
            first_presented_at: None,
            state_history: Vec::new(),
            last_event_id: Uuid::new_v4(),
        };
        store.issues.insert(id, issue);
        ids.push(id);
    }
    ids
}

#[actix_web::test]
async fn compat_issue_list_is_sentry_shaped_and_defaults_to_unresolved() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;

    let (status, body, headers) = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), &token)).await;
    assert_eq!(status, StatusCode::OK, "issue list failed: {body}");
    let issues = body.as_array().expect("array body");
    assert!(!issues.is_empty(), "demo data has issues");
    for issue in issues {
        assert_eq!(
            issue["status"], "unresolved",
            "default query is is:unresolved"
        );
        assert_eq!(issue["userCount"], 0);
        assert_eq!(issue["annotations"], json!([]));
        assert_eq!(issue["statusDetails"], json!({}));
        assert!(issue["shortId"]
            .as_str()
            .unwrap_or_default()
            .starts_with("SCOUT-WEB-"));
        assert!(issue["permalink"]
            .as_str()
            .unwrap_or_default()
            .contains("#/issues/"));
        assert!(issue["firstSeen"]
            .as_str()
            .unwrap_or_default()
            .ends_with('Z'));
        assert!(issue["metadata"]["title"].is_string());
        assert!(issue["project"]["slug"].is_string());
    }
    let hits: usize = header_value(&headers, "x-hits")
        .parse()
        .expect("X-Hits header");
    assert!(hits >= issues.len());
    let link = header_value(&headers, "link");
    assert!(link.contains("rel=\"next\""), "Link header: {link}");
    assert!(link.contains("results=\""));
}

#[actix_web::test]
async fn compat_status_mutation_updates_history_and_rejects_unsupported_fields() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;
    let viewer = login(&app, "demo@scout.local").await;
    let session = viewer["token"].as_str().unwrap();
    let project = project_id(&app, session, "scout-web").await;

    let (status, body, _) = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), &token)).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<String> = body
        .as_array()
        .expect("array")
        .iter()
        .take(2)
        .map(|issue| issue["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2);

    let uri = format!(
        "/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}/issues/?id={}&id={}",
        ids[0], ids[1]
    );
    let (status, body, _) = compat_call(
        &app,
        put_json(&uri, &token, json!({ "status": "resolved" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "mutation failed: {body}");
    let updated = body.as_array().expect("array");
    assert_eq!(updated.len(), 2);
    for issue in updated {
        assert_eq!(issue["status"], "resolved");
    }

    // The native API sees the new state and the token owner as the actor.
    let native = issues_for(&app, session, project).await;
    for id in &ids {
        let issue = native
            .iter()
            .find(|issue| issue["id"].as_str() == Some(id.as_str()))
            .expect("issue still listed");
        assert_eq!(issue["state"], "resolved");
        let last = issue["state_history"].as_array().expect("history");
        assert_eq!(last.last().unwrap()["by"], "demo@scout.local");
    }

    let (status, body, _) = compat_call(
        &app,
        put_json(
            &issues_uri(COMPAT_PROJECT),
            &token,
            json!({ "assignedTo": "me" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("assignedTo"));

    let (status, _, _) = compat_call(
        &app,
        put_json(&issues_uri(COMPAT_PROJECT), &token, json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body, _) = compat_call(&app, delete(&issues_uri(COMPAT_PROJECT), &token)).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(body["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("preserves"));
}

#[actix_web::test]
async fn compat_pagination_is_exactly_once_and_query_bound() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;
    seed_synthetic_issues(&state, 1_000, "pager").await;

    let base =
        format!("/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}/issues/?query=synthetic:pager");
    let (status, first, headers) = compat_call(&app, get(&base, &token)).await;
    assert_eq!(status, StatusCode::OK);
    let mut seen: Vec<String> = first
        .as_array()
        .expect("array")
        .iter()
        .map(|issue| issue["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(seen.len(), 100, "default page size");
    let hits: usize = header_value(&headers, "x-hits").parse().unwrap();
    assert_eq!(hits, 1_000);

    let mut cursor = link_cursor(header_value(&headers, "link"));
    while let Some(value) = cursor {
        let uri = format!("{base}&cursor={value}");
        let (status, page, headers) = compat_call(&app, get(&uri, &token)).await;
        assert_eq!(status, StatusCode::OK);
        let items = page.as_array().expect("array");
        assert!(items.len() <= 100);
        seen.extend(
            items
                .iter()
                .map(|issue| issue["id"].as_str().unwrap().to_string()),
        );
        cursor = link_cursor(header_value(&headers, "link"));
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 1_000, "every issue exactly once");

    // A cursor replayed against a different query is rejected.
    let (status, _, headers) = compat_call(&app, get(&base, &token)).await;
    assert_eq!(status, StatusCode::OK);
    let cursor = link_cursor(header_value(&headers, "link")).expect("next cursor");
    let other = format!("{base}&sort=freq&cursor={cursor}");
    let (status, body, _) = compat_call(&app, get(&other, &token)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!body["detail"].as_str().unwrap_or_default().is_empty());

    // limit is clamped, never an error; zero and negatives use the default.
    for (limit, expected) in [("0", 100), ("-3", 100), ("500", 100), ("abc", 100)] {
        let uri = format!("{base}&limit={limit}");
        let (status, page, _) = compat_call(&app, get(&uri, &token)).await;
        assert_eq!(status, StatusCode::OK, "limit={limit}");
        assert_eq!(page.as_array().unwrap().len(), expected, "limit={limit}");
    }
}

#[actix_web::test]
async fn compat_tag_values_filter_paginate_and_scope_to_an_issue() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;
    let ids = seed_synthetic_issues(&state, 2, "tags").await;
    {
        let mut store = state.store.write().await;
        let mut extra = store.issues.get(&ids[0]).cloned().expect("seeded issue");
        extra.id = Uuid::new_v4();
        extra.number = store.next_issue_number(extra.project_id);
        extra.fingerprint = "synthetic-tags-extra".into();
        extra.tags = json!({ "release": "web@1.1.0" });
        store.issues.insert(extra.id, extra);
    }

    let uri = format!("/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}/tags/release/values/");
    let (status, body, headers) = compat_call(&app, get(&uri, &token)).await;
    assert_eq!(status, StatusCode::OK, "tag values failed: {body}");
    let values: Vec<String> = body
        .as_array()
        .expect("array")
        .iter()
        .map(|value| value["name"].as_str().unwrap().to_string())
        .collect();
    assert!(values.contains(&"web@1.0.0".to_string()));
    assert!(values.contains(&"web@1.1.0".to_string()));
    let mut sorted = values.clone();
    sorted.sort();
    assert_eq!(values, sorted);

    let (status, filtered, _) = compat_call(&app, get(&format!("{uri}?query=1.1"), &token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filtered, json!([{ "name": "web@1.1.0" }]));

    let (status, missing, _) = compat_call(
        &app,
        get(
            &format!("/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}/tags/nope/values/"),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(missing, json!([]));
    assert!(headers.contains_key("link"));

    let issue_uri = format!(
        "/api/0/organizations/{COMPAT_ORG}/issues/{}/tags/release/values/",
        ids[0]
    );
    let (status, issue_values, _) = compat_call(&app, get(&issue_uri, &token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(issue_values, json!([{ "name": "web@1.0.0" }]));

    let (status, _, _) = compat_call(
        &app,
        get(
            &format!(
                "/api/0/organizations/{COMPAT_ORG}/issues/{}/tags/release/values/",
                Uuid::new_v4()
            ),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[actix_web::test]
async fn compat_tokens_are_hashed_shown_once_and_revocable() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let session = auth["token"].as_str().unwrap();

    let (status, created) = call_json(
        &app,
        post_json(
            "/api/me/tokens",
            session,
            json!({ "label": "release-dashboard" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    let plaintext = created["token"]
        .as_str()
        .expect("plaintext token")
        .to_string();
    assert!(plaintext.starts_with("scout_pat_"));
    assert_eq!(
        created["token_prefix"].as_str().unwrap().chars().count(),
        14
    );
    assert!(created.get("token_hash").is_none());
    let token_id = created["id"].as_str().unwrap().to_string();

    let (status, listed) = call_json(&app, get("/api/me/tokens", session)).await;
    assert_eq!(status, StatusCode::OK);
    let row = listed
        .as_array()
        .expect("array")
        .iter()
        .find(|row| row["id"].as_str() == Some(token_id.as_str()))
        .expect("new token listed");
    assert!(row.get("token_hash").is_none());
    assert!(row.get("token").is_none());

    let (status, _) = call_json(
        &app,
        post_json("/api/me/tokens", session, json!({ "label": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The token works against /api/0 and stops working after revocation.
    let (status, _, _) = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), &plaintext)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call_json(&app, delete(&format!("/api/me/tokens/{token_id}"), session)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call_json(&app, delete(&format!("/api/me/tokens/{token_id}"), session)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "revocation is idempotent");
    let (status, body, _) = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), &plaintext)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["detail"], "Invalid token");

    let audit = state.store.read().await.audit.clone();
    assert!(audit.iter().any(|entry| entry.action == "token.create"));
    assert!(audit.iter().any(|entry| entry.action == "token.revoke"));

    // Another user cannot revoke someone else's token.
    let viewer = login(&app, "viewer@scout.local").await;
    let viewer_session = viewer["token"].as_str().unwrap();
    let (status, _) = call_json(
        &app,
        delete(&format!("/api/me/tokens/{token_id}"), viewer_session),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[actix_web::test]
async fn compat_organisation_admins_can_list_and_revoke_member_tokens() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let org_id = {
        let store = state.store.read().await;
        store
            .orgs
            .values()
            .find(|org| org.slug == COMPAT_ORG)
            .map(|org| org.id)
            .expect("seeded org")
    };
    let member_token = compat_token(&state, "viewer@scout.local").await;

    let admin = login(&app, "demo@scout.local").await;
    let admin_session = admin["token"].as_str().unwrap();
    let (status, listed) = call_json(
        &app,
        get(&format!("/api/orgs/{org_id}/tokens"), admin_session),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "org token list failed: {listed}");
    let rows = listed.as_array().expect("array");
    assert!(rows
        .iter()
        .any(|row| row["user_email"] == "viewer@scout.local"));
    assert!(rows.iter().all(|row| row.get("token_hash").is_none()));
    let token_id = rows
        .iter()
        .find(|row| row["user_email"] == "viewer@scout.local")
        .and_then(|row| row["id"].as_str())
        .expect("member token id")
        .to_string();

    let (status, _) = call_json(
        &app,
        delete(
            &format!("/api/orgs/{org_id}/tokens/{token_id}"),
            admin_session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), &member_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let member = login(&app, "viewer@scout.local").await;
    let member_session = member["token"].as_str().unwrap();
    let (status, _) = call_json(
        &app,
        get(&format!("/api/orgs/{org_id}/tokens"), member_session),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[actix_web::test]
async fn compat_scopes_follow_live_roles_and_fail_uniformly() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let viewer = compat_token(&state, "viewer@scout.local").await;
    let triager = compat_token(&state, "triager@scout.local").await;
    let owner = compat_token(&state, "demo@scout.local").await;

    // Missing, malformed, unknown tokens all look the same.
    let missing = compat_call(
        &app,
        test::TestRequest::get()
            .uri(&issues_uri(COMPAT_PROJECT))
            .to_request(),
    )
    .await;
    let garbage = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), "garbage")).await;
    let unknown = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), "scout_pat_unknown")).await;
    for (status, body) in [
        (missing.0, missing.1),
        (garbage.0, garbage.1),
        (unknown.0, unknown.1),
    ] {
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["detail"], "Invalid token");
    }

    let (status, _, _) = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), &viewer)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = compat_call(
        &app,
        put_json(
            &issues_uri(COMPAT_PROJECT),
            &viewer,
            json!({ "status": "resolved" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, _) = compat_call(
        &app,
        put_json(
            &issues_uri(COMPAT_PROJECT),
            &triager,
            json!({ "status": "ignored" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "triagers may write events");

    // Removing the owner from the project takes effect on the next request.
    {
        let mut store = state.store.write().await;
        let project_id = store
            .projects
            .values()
            .find(|project| project.slug == COMPAT_PROJECT)
            .map(|project| project.id)
            .unwrap();
        let owner_id = store
            .users
            .values()
            .find(|user| user.email == "demo@scout.local")
            .map(|user| user.id)
            .unwrap();
        store.memberships.remove(&(owner_id, project_id));
    }
    let (status, _, _) = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), &owner)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no existence leak");

    // A viewer of one project cannot see another project in the same org.
    let (status, _, _) = compat_call(&app, get(&issues_uri("platform-api"), &viewer)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[actix_web::test]
async fn compat_unsupported_subsystems_answer_honestly() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;
    let before = state.store.read().await.issues.len();
    let base = format!("/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}");
    let release = {
        let store = state.store.read().await;
        store
            .issues
            .values()
            .filter_map(|issue| issue.release.clone())
            .next()
            .unwrap_or_else(|| "web@1.0.0".into())
    };

    let (status, body, _) = compat_call(
        &app,
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/releases/{release}/commitfiles/"),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
    let (status, _, _) = compat_call(
        &app,
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/releases/never/commitfiles/"),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for path in ["hooks/", "user-feedback/"] {
        let (status, body, _) = compat_call(&app, get(&format!("{base}/{path}"), &token)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body, json!([]), "{path}");
    }
    let (status, _, _) = compat_call(&app, get(&format!("{base}/hooks/hook-1/"), &token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = compat_call(
        &app,
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/spike-protections/"),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for (req, label) in [
        (
            post_json(&format!("{base}/hooks/"), &token, json!({})),
            "service hooks",
        ),
        (
            put_json(&format!("{base}/hooks/hook-1/"), &token, json!({})),
            "service hooks",
        ),
        (
            post_json(&format!("{base}/user-feedback/"), &token, json!({})),
            "user reports",
        ),
        (
            put_json(
                &format!("/api/0/organizations/{COMPAT_ORG}/spike-protections/"),
                &token,
                json!({}),
            ),
            "spike protection",
        ),
        (
            delete(
                &format!("/api/0/organizations/{COMPAT_ORG}/spike-protections/"),
                &token,
            ),
            "spike protection",
        ),
        (
            post_json(
                "/api/0/sentry-app-installations/inst-1/external-issues/",
                &token,
                json!({}),
            ),
            "external issues",
        ),
        (
            delete(
                "/api/0/sentry-app-installations/inst-1/external-issues/ext-1/",
                &token,
            ),
            "external issues",
        ),
    ] {
        let (status, body, _) = compat_call(&app, req).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{label}");
        assert!(
            body["detail"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .contains("not supported"),
            "{label}: {body}"
        );
    }

    let (status, body, _) = compat_call(
        &app,
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/sentry-app-installations/"),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));

    let after = state.store.read().await.issues.len();
    assert_eq!(before, after, "unsupported paths never mutate data");
}

#[actix_web::test]
async fn compat_surface_is_off_by_default() {
    let state = state_for_with("https://scout.example", 256, false).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;
    let base = format!("/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}");
    let issue = Uuid::new_v4();
    let requests = vec![
        get(&format!("{base}/issues/"), &token),
        put_json(
            &format!("{base}/issues/"),
            &token,
            json!({ "status": "resolved" }),
        ),
        delete(&format!("{base}/issues/"), &token),
        get(&format!("{base}/tags/release/values/"), &token),
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/issues/{issue}/tags/release/values/"),
            &token,
        ),
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/releases/web@1.0.0/commitfiles/"),
            &token,
        ),
        get(&format!("{base}/hooks/"), &token),
        get(&format!("{base}/hooks/hook-1/"), &token),
        put_json(&format!("{base}/hooks/hook-1/"), &token, json!({})),
        get(&format!("{base}/user-feedback/"), &token),
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/spike-protections/"),
            &token,
        ),
        delete(
            &format!("/api/0/organizations/{COMPAT_ORG}/spike-protections/"),
            &token,
        ),
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/sentry-app-installations/"),
            &token,
        ),
        post_json(
            "/api/0/sentry-app-installations/inst-1/external-issues/",
            &token,
            json!({}),
        ),
        delete(
            "/api/0/sentry-app-installations/inst-1/external-issues/ext-1/",
            &token,
        ),
    ];
    for req in requests {
        let (status, body, _) = compat_call(&app, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["detail"], "Not Found");
    }
}

#[actix_web::test]
async fn compat_every_documented_path_answers_with_json() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let demo = compat_token(&state, "demo@scout.local").await;
    let viewer = compat_token(&state, "viewer@scout.local").await;
    let base = format!("/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}");
    let (status, listed, _) = compat_call(&app, get(&format!("{base}/issues/"), &viewer)).await;
    assert_eq!(status, StatusCode::OK);
    let issue_id = listed
        .as_array()
        .and_then(|issues| issues.first())
        .and_then(|issue| issue["id"].as_str())
        .expect("seeded issue")
        .to_string();
    let release = {
        let store = state.store.read().await;
        store
            .issues
            .values()
            .filter_map(|issue| issue.release.clone())
            .next()
            .unwrap_or_else(|| "web@1.0.0".into())
    };

    let requests: Vec<Request> = vec![
        get(&format!("{base}/issues/"), &viewer),
        put_json(
            &format!("{base}/issues/"),
            &viewer,
            json!({ "status": "resolved" }),
        ),
        delete(&format!("{base}/issues/"), &viewer),
        get(&format!("{base}/tags/release/values/"), &viewer),
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/issues/{issue_id}/tags/release/values/"),
            &viewer,
        ),
        get(&format!("{base}/hooks/"), &viewer),
        get(&format!("{base}/hooks/hook-1/"), &viewer),
        put_json(&format!("{base}/hooks/hook-1/"), &viewer, json!({})),
        get(&format!("{base}/user-feedback/"), &viewer),
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/releases/{release}/commitfiles/"),
            &demo,
        ),
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/spike-protections/"),
            &viewer,
        ),
        delete(
            &format!("/api/0/organizations/{COMPAT_ORG}/spike-protections/"),
            &demo,
        ),
        get(
            &format!("/api/0/organizations/{COMPAT_ORG}/sentry-app-installations/"),
            &demo,
        ),
        post_json(
            "/api/0/sentry-app-installations/inst-1/external-issues/",
            &demo,
            json!({}),
        ),
        delete(
            "/api/0/sentry-app-installations/inst-1/external-issues/ext-1/",
            &demo,
        ),
    ];
    for req in requests {
        let res = call(&app, req).await;
        let status = res.status();
        assert!(
            !status.is_server_error() || status == StatusCode::NOT_IMPLEMENTED,
            "documented path returned {status}"
        );
        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            content_type.contains("application/json"),
            "non-JSON response ({content_type}) for {status}"
        );
        let body = test::read_body(res).await;
        let value: Value = serde_json::from_slice(&body).expect("JSON body");
        if status.is_client_error() || status == StatusCode::NOT_IMPLEMENTED {
            assert!(
                value["detail"].is_string(),
                "error body must use detail: {value}"
            );
        }
    }
}

#[actix_web::test]
async fn compat_does_not_change_the_native_surface() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let session = auth["token"].as_str().unwrap();
    let (status, body) = call_json(&app, get("/api/projects", session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["projects"].is_array());
    let (status, _) = call_json(&app, get("/api/health", "")).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::UNAUTHORIZED,
        "native health must not be affected"
    );
}

/// SC-007 benchmark. Ignored by default; run with
/// `cargo test --release -- --ignored sc_007`.
#[actix_web::test]
#[ignore]
async fn sc_007_issue_list_and_tag_values_stay_under_500ms() {
    use std::time::Instant;
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;
    seed_synthetic_issues(&state, 1_000, "perf").await;

    let mut samples = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let (status, _, _) = compat_call(
            &app,
            get(
                &format!(
                    "/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}/issues/?query=synthetic:perf"
                ),
                &token,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = compat_call(
            &app,
            get(
                &format!("/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}/tags/release/values/"),
                &token,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        samples.push(started.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() * 95) / 100];
    println!("SC-007 p95 for issue list + tag values: {p95:?}");
    if !cfg!(debug_assertions) {
        assert!(
            p95 < Duration::from_millis(500),
            "p95 {p95:?} exceeds the 500 ms budget"
        );
    }
}

#[actix_web::test]
async fn compat_invalid_ids_match_nothing() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;

    // A malformed id must not degrade into an unrestricted list.
    let (status, body, headers) = compat_call(
        &app,
        get(
            &format!("{}?id=not-a-uuid", issues_uri(COMPAT_PROJECT)),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
    assert_eq!(header_value(&headers, "x-hits"), "0");

    // Mixed valid and invalid ids keep only the valid ones.
    let (status, listed, _) = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), &token)).await;
    assert_eq!(status, StatusCode::OK);
    let valid = listed
        .as_array()
        .and_then(|issues| issues.first())
        .and_then(|issue| issue["id"].as_str())
        .expect("seeded issue")
        .to_string();
    let (status, filtered, _) = compat_call(
        &app,
        get(
            &format!("{}?id={valid}&id=garbage", issues_uri(COMPAT_PROJECT)),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let filtered = filtered.as_array().expect("array");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0]["id"].as_str(), Some(valid.as_str()));

    // A mutation addressed only with malformed ids changes nothing.
    let before = state
        .store
        .read()
        .await
        .issues
        .values()
        .filter(|issue| issue.state == crate::models::IssueState::Open)
        .count();
    let (status, body, _) = compat_call(
        &app,
        put_json(
            &format!("{}?id=garbage", issues_uri(COMPAT_PROJECT)),
            &token,
            json!({ "status": "resolved" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
    let after = state
        .store
        .read()
        .await
        .issues
        .values()
        .filter(|issue| issue.state == crate::models::IssueState::Open)
        .count();
    assert_eq!(before, after, "no issue may change state");
}

#[actix_web::test]
async fn compat_sorts_hashes_and_ignored_parameters() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;
    let ids = seed_synthetic_issues(&state, 2, "sorts").await;
    let (first_seen_a, last_seen_a) = {
        let mut store = state.store.write().await;
        let base = chrono::Utc::now();
        let mut a = store.issues.get(&ids[0]).cloned().expect("issue a");
        a.count = 5;
        a.last_seen = base - chrono::Duration::hours(1);
        a.first_seen = base - chrono::Duration::hours(3);
        a.fingerprint = "hash-a".into();
        a.title = "issue A".into();
        let (first_a, last_a) = (a.first_seen, a.last_seen);
        store.issues.insert(a.id, a);
        let mut b = store.issues.get(&ids[1]).cloned().expect("issue b");
        b.count = 9;
        b.last_seen = base - chrono::Duration::hours(2);
        b.first_seen = base - chrono::Duration::hours(1);
        b.fingerprint = "hash-b".into();
        b.title = "issue B".into();
        store.issues.insert(b.id, b);
        (first_a, last_a)
    };
    let _ = (first_seen_a, last_seen_a);

    let base_uri =
        format!("/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}/issues/?query=synthetic:sorts");
    for (sort, expected_title) in [
        ("date", "issue A"),
        ("new", "issue B"),
        ("freq", "issue B"),
        ("user", "issue B"),
        ("trends", "issue A"),
        ("recommended", "issue A"),
    ] {
        let uri = format!("{base_uri}&sort={sort}");
        let (status, body, _) = compat_call(&app, get(&uri, &token)).await;
        assert_eq!(status, StatusCode::OK, "sort={sort}");
        let first = body
            .as_array()
            .and_then(|issues| issues.first())
            .expect("at least one issue");
        assert_eq!(first["title"], expected_title, "sort={sort}");
    }

    // hashes filters on the stored fingerprint; unknown hashes yield an empty page.
    let (status, body, _) =
        compat_call(&app, get(&format!("{base_uri}&hashes=hash-a"), &token)).await;
    assert_eq!(status, StatusCode::OK);
    let body = body.as_array().expect("array");
    assert_eq!(body.len(), 1);
    assert_eq!(body[0]["title"], "issue A");
    let (status, body, _) =
        compat_call(&app, get(&format!("{base_uri}&hashes=nope"), &token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));

    // statsPeriod and shortIdLookup are accepted and ignored.
    let (status, tolerated, headers) = compat_call(
        &app,
        get(
            &format!("{base_uri}&statsPeriod=14d&shortIdLookup=0"),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tolerated.as_array().unwrap().len(), 2);
    assert_eq!(header_value(&headers, "x-hits"), "2");
}

#[actix_web::test]
async fn compat_tag_values_are_capped_at_one_thousand() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;
    let ids = seed_synthetic_issues(&state, 1, "bulk").await;
    {
        let mut store = state.store.write().await;
        let template = store.issues.get(&ids[0]).cloned().expect("seeded issue");
        for index in 0..1_005usize {
            let mut issue = template.clone();
            issue.id = Uuid::new_v4();
            issue.number = store.next_issue_number(issue.project_id);
            issue.tags = json!({ "bulk": format!("value-{index:04}") });
            store.issues.insert(issue.id, issue);
        }
    }
    let uri = format!("/api/0/projects/{COMPAT_ORG}/{COMPAT_PROJECT}/tags/bulk/values/?limit=100");
    let mut cursor: Option<String> = None;
    let mut values: Vec<String> = Vec::new();
    loop {
        let request_uri = match &cursor {
            Some(value) => format!("{uri}&cursor={value}"),
            None => uri.clone(),
        };
        let (status, body, headers) = compat_call(&app, get(&request_uri, &token)).await;
        assert_eq!(status, StatusCode::OK);
        values.extend(
            body.as_array()
                .expect("array")
                .iter()
                .map(|value| value["name"].as_str().unwrap().to_string()),
        );
        cursor = link_cursor(header_value(&headers, "link"));
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(values.len(), 1_000, "the distinct set is capped at 1,000");
    let mut unique = values.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 1_000);
}

#[actix_web::test]
async fn compat_disabled_owner_token_is_rejected() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "viewer@scout.local").await;
    {
        let mut store = state.store.write().await;
        let user = store
            .users
            .values_mut()
            .find(|user| user.email == "viewer@scout.local")
            .expect("seeded viewer");
        user.disabled_at = Some(chrono::Utc::now());
    }
    let (status, body, _) = compat_call(&app, get(&issues_uri(COMPAT_PROJECT), &token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["detail"], "Invalid token");
}

#[actix_web::test]
async fn compat_query_decode_errors_use_the_detail_envelope() {
    let state = state_for("https://scout.example", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let token = compat_token(&state, "demo@scout.local").await;
    // Whatever the query decoder does with these bytes, the response must stay
    // JSON: either a served page or the compatibility detail envelope.
    for query in ["limit=%FF", "limit=%", "limit=%GG", "%FF=1", "cursor=%FF"] {
        let uri = format!("{}?{}", issues_uri(COMPAT_PROJECT), query);
        let res = call(&app, get(&uri, &token)).await;
        let status = res.status();
        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = test::read_body(res).await;
        assert!(
            content_type.contains("application/json"),
            "{query} -> {status} ({content_type})"
        );
        let value: Value = serde_json::from_slice(&body).expect("JSON body");
        if status == StatusCode::BAD_REQUEST {
            assert!(value["detail"].is_string(), "{query}: {value}");
        } else {
            assert_eq!(status, StatusCode::OK, "{query}");
        }
    }
}

/// FR-016: the migration mode must not be able to start the HTTP server,
/// workers, or retention. Scanning the production half of `migrate.rs` makes
/// the guarantee executable without launching a process.
#[actix_web::test]
async fn migrate_module_has_no_service_wiring() {
    let source = include_str!("migrate.rs");
    let production = source
        .split("#[cfg(test)]")
        .next()
        .expect("migrate.rs has a production section");
    for forbidden in ["actix_web::", "crate::worker", "crate::config"] {
        assert!(
            !production.contains(forbidden),
            "FR-016: src/migrate.rs must not reference {forbidden}"
        );
    }
}

// --- operational counters --------------------------------------------------

#[actix_web::test]
async fn metrics_endpoint_is_operator_only() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    bootstrap_operator(&state).await;

    let (status, _) = call_json(&app, get_public("/api/metrics")).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "anonymous callers are rejected"
    );

    let member = login(&app, "demo@scout.local").await;
    let member_token = member["token"].as_str().expect("member session");
    let (status, _) = call_json(&app, get("/api/metrics", member_token)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "members cannot read deployment counters"
    );

    let (status, auth) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "operator login failed: {auth}");
    let operator_token = auth["token"].as_str().expect("operator session");
    let (status, body) = call_json(&app, get("/api/metrics", operator_token)).await;
    assert_eq!(status, StatusCode::OK, "metrics failed: {body}");
    assert_eq!(body["store"], "memory");
    assert!(body["counters"]["ingest_accepted"].is_u64());
    assert!(body["counters"]["retention_objects_deleted"].is_u64());
}

#[actix_web::test]
async fn accepted_ingest_and_rejections_are_counted() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    bootstrap_operator(&state).await;

    let accepted = ingest_request(
        "1",
        "scout_demo_key",
        "application/x-sentry-envelope",
        envelope(&[event(&hex_id(9001), "metrics check", &["metrics-group"])]),
        "envelope",
    );
    let (status, body) = call_json(&app, accepted).await;
    assert_eq!(status, StatusCode::OK, "ingest failed: {body}");

    let rejected = ingest_request(
        "1",
        "not-a-real-key",
        "application/x-sentry-envelope",
        envelope(&[event(&hex_id(9002), "rejected", &["metrics-group"])]),
        "envelope",
    );
    let (status, _) = call_json(&app, rejected).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (_, auth) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    let token = auth["token"].as_str().expect("operator session");
    let (status, metrics) = call_json(&app, get("/api/metrics", token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        metrics["counters"]["ingest_accepted"].as_u64().unwrap_or(0) >= 1,
        "accepted ingest must be counted: {metrics}"
    );
    assert_eq!(metrics["counters"]["ingest_rejected"]["auth"], 1);
}

// --- account lockout (FR-032, SC-023) --------------------------------------

#[actix_web::test]
async fn five_consecutive_failures_lock_the_account_for_every_sign_in() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;

    for _ in 0..4 {
        let (status, body) = login_with(&app, "demo@scout.local", "wrong-password").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "invalid email or password");
    }
    // A success before the threshold resets the consecutive count.
    let (status, _) = login_with(&app, "demo@scout.local", crate::store::DEMO_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "sign-in must work before the lock");
    {
        let store = state.store.read().await;
        let user = store
            .user_by_email("demo@scout.local")
            .expect("the demo account");
        assert_eq!(user.failed_sign_in_count, 0, "a success resets the count");
        assert!(user.locked_at.is_none());
    }

    for _ in 0..5 {
        let (status, _) = login_with(&app, "demo@scout.local", "wrong-password").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    {
        let store = state.store.read().await;
        let user = store
            .user_by_email("demo@scout.local")
            .expect("the demo account");
        assert_eq!(
            user.failed_sign_in_count,
            crate::store::MAX_FAILED_SIGN_INS,
            "the count lives on the account"
        );
        assert!(user.locked_at.is_some(), "the lock lives on the account");
    }

    // The correct password is refused with the same generic body while locked.
    let (status, body) = login_with(&app, "demo@scout.local", crate::store::DEMO_PASSWORD).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        body["error"], "invalid email or password",
        "a locked account must not reveal that it is locked"
    );
}

#[actix_web::test]
async fn the_operator_lists_and_unlocks_a_locked_account() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    bootstrap_operator(&state).await;

    for _ in 0..5 {
        let (status, _) = login_with(&app, "demo@scout.local", "wrong-password").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // A member cannot read or change lock state.
    let triager = login(&app, "triager@scout.local").await;
    let triager_token = triager["token"].as_str().expect("triager session");
    let (status, _) = call_json(&app, get("/api/operator/users?locked=true", triager_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, auth) = login_with(&app, "ops@test.local", OPERATOR_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "operator login failed: {auth}");
    let operator_token = auth["token"].as_str().expect("operator session");

    let (status, body) =
        call_json(&app, get("/api/operator/users?locked=true", operator_token)).await;
    assert_eq!(status, StatusCode::OK);
    let locked = body["users"]
        .as_array()
        .expect("a users array")
        .iter()
        .find(|user| user["email"] == "demo@scout.local")
        .expect("the locked account is listed")
        .clone();
    let user_id = locked["id"].as_str().expect("account id");
    assert!(locked["locked_at"].is_string(), "lock time is reported");

    // A non-operator cannot unlock either.
    let (status, _) = call_json(
        &app,
        post_json(
            &format!("/api/operator/users/{user_id}/unlock"),
            triager_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = call_json(
        &app,
        post_json(
            &format!("/api/operator/users/{user_id}/unlock"),
            operator_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unlock failed: {body}");
    assert_eq!(body["ok"], true);

    // Unlocking itself clears both the lock and the counter, before any sign-in.
    {
        let store = state.store.read().await;
        let user = store
            .user_by_email("demo@scout.local")
            .expect("the demo account");
        assert!(user.locked_at.is_none());
        assert_eq!(user.failed_sign_in_count, 0);
    }

    let (status, _) = login_with(&app, "demo@scout.local", crate::store::DEMO_PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the account signs in after an unlock"
    );

    // Unlocking an unknown account is a 404.
    let (status, _) = call_json(
        &app,
        post_json(
            &format!("/api/operator/users/{}/unlock", Uuid::new_v4()),
            operator_token,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- session lifetime (FR-033, SC-024) -------------------------------------

#[actix_web::test]
async fn an_idle_session_expires_and_a_recent_request_keeps_it_alive() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().expect("session token").to_string();
    let hash = crate::auth::hash_token(&token);

    // A request refreshes the session's last activity.
    let (status, _) = call_json(&app, get("/api/me", &token)).await;
    assert_eq!(status, StatusCode::OK);

    // Backdate the session beyond the configured 12-hour idle window.
    {
        let mut store = state.store.write().await;
        let session = store.sessions.get_mut(&hash).expect("stored session");
        session.last_seen_at = chrono::Utc::now() - chrono::Duration::hours(13);
    }

    let (status, body) = call_json(&app, get("/api/me", &token)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an idle session must be rejected: {body}"
    );
    assert!(
        state.store.read().await.sessions.get(&hash).is_none(),
        "the expired session is removed"
    );
}

// --- unified search cap (FR-021, SC-025) -----------------------------------

#[actix_web::test]
async fn search_caps_at_two_hundred_newest_matches_and_reports_more() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().expect("session").to_string();
    let project = project_id(&app, &token, "scout-web").await;

    let items: Vec<Value> = (0..205)
        .map(|index| json!({ "message": format!("capcheck line {index}"), "level": "info" }))
        .collect();
    let body = json!({ "logs": items }).to_string();
    let req = ingest_request(
        "1",
        "scout_demo_key",
        "application/json",
        body.into_bytes(),
        "logs",
    );
    let (status, ingested) = call_json(&app, req).await;
    assert_eq!(status, StatusCode::OK, "log ingest failed: {ingested}");
    assert_eq!(ingested["accepted"].as_u64(), Some(205));

    let (status, body) = call_json(
        &app,
        get(
            &format!("/api/projects/{project}/search?q=capcheck"),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "search failed: {body}");
    let issues = body["issues"].as_array().expect("issues array").len();
    let logs = body["logs"].as_array().expect("logs array").len();
    assert_eq!(issues + logs, 200, "the combined cap is 200");
    assert_eq!(body["has_more"], true, "truncation is reported");

    let (status, body) = call_json(
        &app,
        get(
            &format!("/api/projects/{project}/search?q=nothing-matches-this"),
            &token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["has_more"], false);
    assert_eq!(body["issues"].as_array().unwrap().len(), 0);
    assert_eq!(body["logs"].as_array().unwrap().len(), 0);
}

// --- disabling and removal end access immediately (FR-014, SC-020) ---------

#[actix_web::test]
async fn a_disabled_or_removed_account_loses_its_existing_session() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().expect("session").to_string();
    let user_id: Uuid = auth["user"]["id"]
        .as_str()
        .expect("user id")
        .parse()
        .expect("uuid");

    let (status, _) = call_json(&app, get("/api/me", &token)).await;
    assert_eq!(status, StatusCode::OK);

    // Disabling the account rejects the very next request.
    {
        let mut store = state.store.write().await;
        let user = store.users.get_mut(&user_id).expect("account");
        user.disabled_at = Some(chrono::Utc::now());
    }
    let (status, body) = call_json(&app, get("/api/me", &token)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a disabled account must lose its session: {body}"
    );

    // Re-enabling restores it, and removing the account rejects it again.
    {
        let mut store = state.store.write().await;
        let user = store.users.get_mut(&user_id).expect("account");
        user.disabled_at = None;
    }
    let (status, _) = call_json(&app, get("/api/me", &token)).await;
    assert_eq!(status, StatusCode::OK);

    {
        let mut store = state.store.write().await;
        store.users.remove(&user_id);
        store.users_by_email.remove("demo@scout.local");
    }
    let (status, _) = call_json(&app, get("/api/me", &token)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a removed account must lose its session"
    );
}

// --- password minimum (FR-011, SC-022) -------------------------------------

#[actix_web::test]
async fn short_passwords_are_rejected_and_leave_the_recovery_code_usable() {
    let state = state_for("http://127.0.0.1:1", 256).await;
    let app = init_app_with_state(state.clone()).await;
    let auth = login(&app, "demo@scout.local").await;
    let token = auth["token"].as_str().expect("session").to_string();

    let (status, body) =
        call_json(&app, post_json("/api/me/recovery-codes", &token, json!({}))).await;
    assert_eq!(status, StatusCode::OK, "reissue failed: {body}");
    let code = body["recovery_codes"]
        .as_array()
        .and_then(|codes| codes.first())
        .and_then(|code| code.as_str())
        .expect("a recovery code")
        .to_string();

    let (status, body) = call_json(
        &app,
        post_public(
            "/api/recover",
            json!({ "email": "demo@scout.local", "recovery_code": code, "new_password": "too-short" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"].as_str().unwrap_or("").contains("12"),
        "the message must name the minimum: {body}"
    );

    // A rejected weak password must not consume the code.
    let (status, body) = call_json(
        &app,
        post_public(
            "/api/recover",
            json!({ "email": "demo@scout.local", "recovery_code": code, "new_password": "long-enough-password" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the code stays usable: {body}");
}
