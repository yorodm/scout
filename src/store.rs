use crate::models::*;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

pub fn event_hex_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// In-memory metadata store. The prototype keeps the same shape as the
/// Postgres schema described in architecture.md, behind ordinary methods, so the
/// storage layer can be swapped without touching the ingest or API paths.
pub struct Store {
    pub orgs: HashMap<Uuid, Organisation>,
    pub projects: HashMap<Uuid, Project>,
    pub projects_by_dsn: HashMap<String, Uuid>,
    pub keys: HashMap<String, IngestKey>,
    pub users: HashMap<Uuid, User>,
    pub users_by_email: HashMap<String, Uuid>,
    pub memberships: HashMap<(Uuid, Uuid), Role>,
    pub sessions: HashMap<String, Uuid>,
    pub issues: HashMap<Uuid, Issue>,
    pub issues_by_fingerprint: HashMap<(Uuid, String), Uuid>,
    pub events: HashMap<Uuid, Event>,
    pub seen_event_ids: HashMap<(Uuid, String), Uuid>,
    pub logs: Vec<LogLine>,
    pub audit: Vec<AuditEntry>,
}

impl Store {
    pub fn project_by_identifier(&self, identifier: &str) -> Option<&Project> {
        self.projects_by_dsn
            .get(identifier)
            .and_then(|id| self.projects.get(id))
    }

    pub fn active_key_for(&self, project_id: Uuid) -> Option<&IngestKey> {
        self.keys
            .values()
            .filter(|k| k.project_id == project_id && !k.revoked)
            .min_by_key(|k| k.created_at)
    }

    pub fn role_for(&self, user_id: Uuid, project_id: Uuid) -> Option<Role> {
        self.memberships.get(&(user_id, project_id)).copied()
    }

    pub fn user_for_token(&self, token: &str) -> Option<Uuid> {
        self.sessions.get(token).copied()
    }

    pub fn user_by_email(&self, email: &str) -> Option<&User> {
        self.users_by_email
            .get(&email.to_ascii_lowercase())
            .and_then(|id| self.users.get(id))
    }

    /// Remove raw events and log lines that exceed the project's retention
    /// window. Issues and their counters stay; their raw payload simply becomes
    /// unavailable, which the UI reports. Returns the object keys that the
    /// caller must delete from object storage.
    pub fn apply_retention(&mut self, project: &Project) -> Vec<String> {
        let cutoff = Utc::now() - Duration::days(project.retention_days as i64);
        let expired: Vec<Uuid> = self
            .events
            .values()
            .filter(|e| e.project_id == project.id && e.received_at < cutoff)
            .map(|e| e.id)
            .collect();
        let mut keys = Vec::new();
        for id in expired {
            if let Some(event) = self.events.remove(&id) {
                self.seen_event_ids
                    .remove(&(event.project_id, event.sentry_event_id));
                if let Some(key) = event.raw_object_key {
                    keys.push(key);
                }
            }
        }
        self.logs
            .retain(|l| !(l.project_id == project.id && l.timestamp < cutoff));
        keys
    }
}

/// Build the demo store. Raw payloads are returned separately so the caller can
/// write them to object storage (which is async) before serving requests.
pub fn seed(self_url: &str) -> (Store, Vec<(String, Vec<u8>)>) {
    let mut raws = Vec::new();
    let mut store = Store {
        orgs: HashMap::new(),
        projects: HashMap::new(),
        projects_by_dsn: HashMap::new(),
        keys: HashMap::new(),
        users: HashMap::new(),
        users_by_email: HashMap::new(),
        memberships: HashMap::new(),
        sessions: HashMap::new(),
        issues: HashMap::new(),
        issues_by_fingerprint: HashMap::new(),
        events: HashMap::new(),
        seen_event_ids: HashMap::new(),
        logs: Vec::new(),
        audit: Vec::new(),
    };

    let org_id = Uuid::new_v4();
    store.orgs.insert(
        org_id,
        Organisation {
            id: org_id,
            name: "Acme".into(),
            sso_provider: None,
        },
    );

    let now = Utc::now();

    let web_id = Uuid::new_v4();
    let web = Project {
        id: web_id,
        dsn_id: "1".into(),
        slug: "scout-web".into(),
        name: "scout-web".into(),
        org_id,
        retention_days: 30,
        model: Some(ModelConfig {
            endpoint: format!("{}/mock-jev", self_url.trim_end_matches('/')),
            api_key: None,
            model: "jev-latest".into(),
        }),
        created_at: now - Duration::days(46),
    };
    let platform_id = Uuid::new_v4();
    let platform = Project {
        id: platform_id,
        dsn_id: "2".into(),
        slug: "platform-api".into(),
        name: "platform-api".into(),
        org_id,
        retention_days: 14,
        model: None,
        created_at: now - Duration::days(21),
    };
    for project in [&web, &platform] {
        store.projects.insert(project.id, project.clone());
        store
            .projects_by_dsn
            .insert(project.dsn_id.clone(), project.id);
        store
            .projects_by_dsn
            .insert(project.slug.clone(), project.id);
    }

    for (project_id, key, label) in [
        (web_id, "scout_demo_key", "Default"),
        (platform_id, "scout_platform_key", "Default"),
    ] {
        let ingest_key = IngestKey {
            id: Uuid::new_v4(),
            project_id,
            label: label.into(),
            key: key.into(),
            created_at: now - Duration::days(40),
            revoked: false,
        };
        store.keys.insert(key.into(), ingest_key);
    }

    let demo = User {
        id: Uuid::new_v4(),
        email: "demo@scout.local".into(),
        name: "Dana Demo".into(),
    };
    let triager = User {
        id: Uuid::new_v4(),
        email: "triager@scout.local".into(),
        name: "Toni Triager".into(),
    };
    let viewer = User {
        id: Uuid::new_v4(),
        email: "viewer@scout.local".into(),
        name: "Vic Viewer".into(),
    };
    for user in [&demo, &triager, &viewer] {
        store.users.insert(user.id, user.clone());
        store
            .users_by_email
            .insert(user.email.to_ascii_lowercase(), user.id);
    }
    store.memberships.insert((demo.id, web_id), Role::Owner);
    store
        .memberships
        .insert((demo.id, platform_id), Role::Admin);
    store
        .memberships
        .insert((triager.id, web_id), Role::Triager);
    store.memberships.insert((viewer.id, web_id), Role::Viewer);

    // --- seed issues and their original payloads ---------------------------
    let checkout_event = event_json(
        &event_hex_id(),
        "TypeError",
        "Cannot read properties of undefined (reading 'currency')",
        "error",
        "web@2.4.1",
        "production",
        json!([
            {"filename":"src/checkout/index.ts","function":"handleCheckout","lineno":57,"colno":5,"in_app":true,"context_line":"await payment.chargeCard(cart, discount);"},
            {"filename":"src/checkout/payment.ts","function":"chargeCard","lineno":88,"colno":9,"in_app":true,"context_line":"const amount = cart.total(currency);"},
            {"filename":"src/checkout/cart.ts","function":"applyDiscount","lineno":142,"colno":13,"in_app":true,"context_line":"const currency = discount.currency.toLowerCase();"},
            {"filename":"src/lib/validate.ts","function":"requireCurrency","lineno":19,"colno":7,"in_app":true,"context_line":"if (!value.currency) throw new TypeError(\"missing currency\");"}
        ]),
        json!([
            {"timestamp": (now - Duration::seconds(9)).to_rfc3339(),"category":"console","level":"info","message":"Applying discount code SAVE10"},
            {"timestamp": (now - Duration::seconds(7)).to_rfc3339(),"category":"http","level":"info","message":"POST /api/discounts 200","data":{"status":200,"duration_ms":84}},
            {"timestamp": (now - Duration::seconds(5)).to_rfc3339(),"category":"navigation","level":"info","message":"/cart -> /checkout"},
            {"timestamp": (now - Duration::seconds(3)).to_rfc3339(),"category":"ui.click","level":"info","message":"button#pay"}
        ]),
        json!({"service":"checkout-web","region":"eu-west-1","runtime":"node 22","transaction":"/checkout"}),
        now - Duration::minutes(2),
    );
    seed_issue(
        &mut store,
        &mut raws,
        &web,
        checkout_event,
        1284,
        now - Duration::hours(3),
        now - Duration::minutes(2),
        Some(Suggestion {
            category: "issue".into(),
            confidence: 0.91,
            source: "jev-latest".into(),
            at: now - Duration::minutes(2),
        }),
        None,
        vec![StateChange {
            state: IssueState::Open,
            by: "system".into(),
            at: now - Duration::hours(3),
        }],
        vec![Attachment {
            filename: "checkout-context.txt".into(),
            content_type: "text/plain".into(),
            data: b"cart=8f21\ncurrency=undefined\nroute=/checkout\n".to_vec(),
        }],
    );

    let pool_event = event_json(
        &event_hex_id(),
        "PoolExhaustedError",
        "timed out waiting for a database connection (30s)",
        "fatal",
        "api@1.18.0",
        "production",
        json!([
            {"filename":"src/api/orders.rs","function":"create_order","lineno":128,"colno":5,"in_app":true,"context_line":"let conn = pool.acquire().await?;"},
            {"filename":"src/db/pool.rs","function":"acquire","lineno":71,"colno":9,"in_app":true,"context_line":"tokio::time::timeout(self.timeout, self.inner.acquire()).await??"},
            {"filename":"src/db/pool.rs","function":"checkout","lineno":96,"colno":13,"in_app":true,"context_line":"return Err(PoolExhaustedError::new(self.active, self.waiting));"}
        ]),
        json!([
            {"timestamp": (now - Duration::seconds(905)).to_rfc3339(),"category":"log","level":"warning","message":"pool size=20 active=20 waiting=48"},
            {"timestamp": (now - Duration::seconds(890)).to_rfc3339(),"category":"query","level":"info","message":"SELECT id, total FROM orders WHERE status = $1","data":{"duration_ms":30012}}
        ]),
        json!({"service":"orders-api","region":"eu-central-1","runtime":"rust 1.99","transaction":"POST /orders"}),
        now - Duration::minutes(8),
    );
    seed_issue(
        &mut store,
        &mut raws,
        &web,
        pool_event,
        37,
        now - Duration::hours(2),
        now - Duration::minutes(8),
        Some(Suggestion {
            category: "log".into(),
            confidence: 0.64,
            source: "jev-latest".into(),
            at: now - Duration::minutes(8),
        }),
        None,
        vec![StateChange {
            state: IssueState::Open,
            by: "system".into(),
            at: now - Duration::hours(2),
        }],
        vec![],
    );

    let webhook_event = event_json(
        &event_hex_id(),
        "WebhookDeliveryFailed",
        "503 from inventory-service",
        "warning",
        "api@1.18.0",
        "production",
        json!([
            {"filename":"src/webhooks/dispatch.rs","function":"dispatch","lineno":44,"colno":5,"in_app":true,"context_line":"let response = client.post(url).json(&payload).send().await?;"},
            {"filename":"src/webhooks/inventory.rs","function":"send","lineno":83,"colno":9,"in_app":true,"context_line":"if !response.status().is_success() { return Err(DeliveryFailed::from(response)); }"}
        ]),
        json!([
            {"timestamp": (now - Duration::seconds(3620)).to_rfc3339(),"category":"http","level":"warning","message":"POST https://inventory.internal/webhooks -> 503","data":{"attempt":3,"retry_in_ms":2000}}
        ]),
        json!({"service":"orders-api","region":"eu-central-1","webhook":"inventory-service"}),
        now - Duration::hours(1),
    );
    seed_issue(
        &mut store,
        &mut raws,
        &web,
        webhook_event,
        12,
        now - Duration::days(1),
        now - Duration::hours(1),
        Some(Suggestion {
            category: "issue".into(),
            confidence: 0.72,
            source: "jev-latest".into(),
            at: now - Duration::hours(1),
        }),
        Some(CategorisationDecision {
            category: "issue".into(),
            by: "demo@scout.local".into(),
            at: now - Duration::minutes(50),
            overridden: false,
        }),
        vec![
            StateChange {
                state: IssueState::Open,
                by: "system".into(),
                at: now - Duration::days(1),
            },
            StateChange {
                state: IssueState::Resolved,
                by: "demo@scout.local".into(),
                at: now - Duration::minutes(50),
            },
        ],
        vec![],
    );

    let parse_event = event_json(
        &event_hex_id(),
        "SyntaxError",
        "Unexpected token '<' in JSON at position 0",
        "error",
        "platform@0.9.3",
        "staging",
        json!([
            {"filename":"src/handlers/webhook.rs","function":"handle","lineno":22,"colno":5,"in_app":true,"context_line":"let body: Value = serde_json::from_slice(&bytes)?;"},
            {"filename":"src/parse.rs","function":"from_slice","lineno":11,"colno":9,"in_app":true,"context_line":"return Err(SyntaxError::at(position));"}
        ]),
        json!([
            {"timestamp": (now - Duration::minutes(21)).to_rfc3339(),"category":"http","level":"info","message":"POST /hooks/stripe 502","data":{"status":502}}
        ]),
        json!({"service":"platform-api","region":"us-east-1","runtime":"rust 1.99"}),
        now - Duration::minutes(20),
    );
    seed_issue(
        &mut store,
        &mut raws,
        &platform,
        parse_event,
        5,
        now - Duration::hours(6),
        now - Duration::minutes(20),
        None,
        None,
        vec![StateChange {
            state: IssueState::Open,
            by: "system".into(),
            at: now - Duration::hours(6),
        }],
        vec![],
    );

    // --- seed log lines -----------------------------------------------------
    let log = |project_id: Uuid,
               ago: Duration,
               level: &str,
               message: &str,
               source: &str,
               attributes: Value| LogLine {
        id: Uuid::new_v4(),
        project_id,
        timestamp: now - ago,
        level: level.into(),
        message: message.into(),
        source: source.into(),
        attributes,
        received_at: now - ago,
    };
    store.logs.extend([
        log(web_id, Duration::minutes(2), "error", "checkout failed for cart 8f21: TypeError: Cannot read properties of undefined (reading 'currency')", "checkout-web", json!({"route":"/checkout","user":"u_8241"})),
        log(web_id, Duration::minutes(3), "info", "applied discount SAVE10 to cart 8f21", "checkout-web", json!({"route":"/checkout","code":"SAVE10"})),
        log(web_id, Duration::minutes(8), "error", "database pool exhausted after 30s; 48 waiters", "orders-api", json!({"pool_size":20,"active":20,"waiting":48})),
        log(web_id, Duration::minutes(9), "warning", "retrying webhook delivery to inventory-service (attempt 3/5)", "orders-api", json!({"attempt":3,"max_attempts":5})),
        log(web_id, Duration::hours(1), "info", "deployment web@2.4.1 rolled out to production", "deploy-bot", json!({"release":"web@2.4.1"})),
        log(web_id, Duration::minutes(65), "warning", "inventory-service responded 503 to webhook", "orders-api", json!({"status":503,"service":"inventory-service"})),
        log(web_id, Duration::minutes(70), "info", "health check ok", "checkout-web", json!({"path":"/healthz"})),
        log(platform_id, Duration::minutes(20), "info", "GET /v1/health 200 in 4ms", "platform-api", json!({"status":200,"duration_ms":4})),
        log(platform_id, Duration::minutes(21), "warning", "upstream latency above threshold: p95=820ms", "platform-api", json!({"p95_ms":820,"threshold_ms":500})),
    ]);

    (store, raws)
}

#[allow(clippy::too_many_arguments)]
fn event_json(
    event_id: &str,
    exception_type: &str,
    value: &str,
    level: &str,
    release: &str,
    environment: &str,
    frames: Value,
    breadcrumbs: Value,
    tags: Value,
    timestamp: chrono::DateTime<Utc>,
) -> Value {
    json!({
        "event_id": event_id,
        "timestamp": timestamp.timestamp(),
        "platform": "javascript",
        "level": level,
        "release": release,
        "environment": environment,
        "message": format!("{exception_type}: {value}"),
        "exception": {
            "values": [{
                "type": exception_type,
                "value": value,
                "stacktrace": { "frames": frames }
            }]
        },
        "breadcrumbs": { "values": breadcrumbs },
        "tags": tags,
        "user": { "id": "u_8241", "email": "oncall@acme.dev" },
        "sdk": { "name": "sentry.javascript.node", "version": "8.42.0" }
    })
}

#[allow(clippy::too_many_arguments)]
fn seed_issue(
    store: &mut Store,
    raw_objects: &mut Vec<(String, Vec<u8>)>,
    project: &Project,
    event: Value,
    count: u64,
    first_seen: chrono::DateTime<Utc>,
    last_seen: chrono::DateTime<Utc>,
    suggestion: Option<Suggestion>,
    decision: Option<CategorisationDecision>,
    state_history: Vec<StateChange>,
    attachments: Vec<Attachment>,
) -> Uuid {
    let derived = crate::grouping::derive(&event, project.id);
    let sentry_event_id = event
        .get("event_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(event_hex_id);
    let raw = serde_json::to_vec_pretty(&event).unwrap_or_default();
    let issue_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let raw_object_key = crate::objects::raw_key(project.id, event_id);
    raw_objects.push((raw_object_key.clone(), raw.clone()));

    let issue = Issue {
        id: issue_id,
        project_id: project.id,
        fingerprint: derived.fingerprint.clone(),
        title: derived.title,
        culprit: derived.culprit,
        level: derived.level,
        first_seen,
        last_seen,
        count,
        state: state_history
            .last()
            .map(|c| c.state)
            .unwrap_or(IssueState::Open),
        release: derived.release,
        environment: derived.environment,
        tags: derived.tags,
        frames: derived.frames,
        breadcrumbs: derived.breadcrumbs,
        suggestion,
        decision,
        state_history,
        last_event_id: event_id,
    };

    store
        .issues_by_fingerprint
        .insert((project.id, derived.fingerprint), issue_id);
    store.issues.insert(issue_id, issue);
    store.events.insert(
        event_id,
        Event {
            id: event_id,
            sentry_event_id: sentry_event_id.clone(),
            project_id: project.id,
            issue_id: Some(issue_id),
            received_at: last_seen,
            raw_object_key: Some(raw_object_key),
            raw_size: raw.len() as u64,
            raw_expired_at: None,
            event,
            attachments,
        },
    );
    store
        .seen_event_ids
        .insert((project.id, sentry_event_id), event_id);
    issue_id
}
