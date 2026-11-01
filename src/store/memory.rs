use crate::models::*;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::OnceLock;
use uuid::Uuid;

/// Demo-only credential for the seeded accounts. Real accounts are created by
/// operator provisioning and invitation acceptance, which set their own password.
pub const DEMO_PASSWORD: &str = "scout-demo";

/// Consecutive failed sign-ins that lock an account (FR-032).
pub const MAX_FAILED_SIGN_INS: u32 = 5;

fn demo_password_hash() -> String {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| crate::auth::hash_password(DEMO_PASSWORD).expect("hash demo password"))
        .clone()
}

pub fn event_hex_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// In-memory metadata store. The prototype keeps the same shape as the
/// Postgres schema described in architecture.md, behind ordinary methods, so the
/// storage layer can be swapped without touching the ingest or API paths.
pub struct Store {
    pub orgs: HashMap<Uuid, Organisation>,
    pub org_members: HashMap<(Uuid, Uuid), OrgRole>,
    pub projects: HashMap<Uuid, Project>,
    pub projects_by_dsn: HashMap<String, Uuid>,
    pub keys: HashMap<Uuid, IngestKey>,
    pub users: HashMap<Uuid, User>,
    pub users_by_email: HashMap<String, Uuid>,
    pub memberships: HashMap<(Uuid, Uuid), Role>,
    pub sessions: HashMap<String, Session>,
    pub issues: HashMap<Uuid, Issue>,
    pub issues_by_fingerprint: HashMap<(Uuid, String), Uuid>,
    pub events: HashMap<Uuid, Event>,
    pub seen_event_ids: HashMap<(Uuid, String), Uuid>,
    pub logs: Vec<LogLine>,
    pub audit: Vec<AuditEntry>,
    pub invitations: HashMap<Uuid, Invitation>,
    pub invitations_by_token: HashMap<String, Uuid>,
    pub recovery_codes: Vec<RecoveryCode>,
    /// Consecutive failed sign-ins per user; reset on success or unlock.
    pub failed_sign_ins: HashMap<Uuid, u32>,
    /// Long-lived API tokens for the Sentry-compatible surface.
    pub api_tokens: HashMap<Uuid, ApiToken>,
    /// Token hash → token id, so authentication is a single map lookup.
    pub api_tokens_by_hash: HashMap<String, Uuid>,
}

impl Store {
    /// An empty metadata store for a real deployment. Demo data is only seeded
    /// when `SCOUT_DEMO_MODE` is explicitly enabled.
    pub fn empty() -> Self {
        Self {
            orgs: HashMap::new(),
            org_members: HashMap::new(),
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
            invitations: HashMap::new(),
            invitations_by_token: HashMap::new(),
            recovery_codes: Vec::new(),
            failed_sign_ins: HashMap::new(),
            api_tokens: HashMap::new(),
            api_tokens_by_hash: HashMap::new(),
        }
    }

    pub fn project_by_identifier(&self, identifier: &str) -> Option<&Project> {
        self.projects_by_dsn
            .get(identifier)
            .and_then(|id| self.projects.get(id))
    }

    /// The seeded demonstration organisation, when the store contains one.
    pub fn demo_org_id(&self) -> Option<Uuid> {
        self.orgs.values().find(|org| org.is_demo).map(|org| org.id)
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

    pub fn org_role_for(&self, user_id: Uuid, org_id: Uuid) -> Option<OrgRole> {
        self.org_members.get(&(user_id, org_id)).copied()
    }

    pub fn invitation_by_token(&self, token: &str) -> Option<&Invitation> {
        self.invitations_by_token
            .get(&crate::auth::hash_token(token))
            .and_then(|id| self.invitations.get(id))
    }

    pub fn user_for_token(&self, token: &str) -> Option<Uuid> {
        let session = self.sessions.get(&crate::auth::hash_token(token))?;
        if session.expires_at <= Utc::now() {
            return None;
        }
        Some(session.user_id)
    }

    /// Issue a hashed session token. The plaintext token is returned once and is
    /// the only thing the caller can ever present.
    pub fn issue_session(&mut self, user_id: Uuid, ttl_days: i64) -> String {
        let token = crate::auth::new_token();
        let now = Utc::now();
        self.sessions.insert(
            crate::auth::hash_token(&token),
            Session {
                user_id,
                expires_at: now + Duration::days(ttl_days),
            },
        );
        token
    }

    pub fn revoke_session(&mut self, token: &str) {
        self.sessions.remove(&crate::auth::hash_token(token));
    }

    /// Remove expired sessions. Returns the number removed.
    pub fn sweep_sessions(&mut self) -> usize {
        let now = Utc::now();
        let before = self.sessions.len();
        self.sessions.retain(|_, session| session.expires_at > now);
        before - self.sessions.len()
    }

    pub fn user_by_email(&self, email: &str) -> Option<&User> {
        self.users_by_email
            .get(&email.to_ascii_lowercase())
            .and_then(|id| self.users.get(id))
    }

    /// Count a failed sign-in. Returns true when this failure locked the account.
    pub fn record_failed_sign_in(&mut self, user_id: Uuid) -> bool {
        let count = self.failed_sign_ins.entry(user_id).or_insert(0);
        *count = count.saturating_add(1);
        if *count < MAX_FAILED_SIGN_INS {
            return false;
        }
        if let Some(user) = self.users.get_mut(&user_id) {
            user.locked_at = Some(Utc::now());
        }
        true
    }

    /// Clear the consecutive-failure count after a successful sign-in.
    pub fn clear_failed_sign_ins(&mut self, user_id: Uuid) {
        self.failed_sign_ins.remove(&user_id);
    }

    /// Operator unlock: clears the lock and the failure count.
    pub fn unlock_user(&mut self, user_id: Uuid) -> bool {
        let Some(user) = self.users.get_mut(&user_id) else {
            return false;
        };
        user.locked_at = None;
        self.failed_sign_ins.remove(&user_id);
        true
    }

    /// Locked accounts, newest lock first, for the operator console.
    pub fn locked_users(&self) -> Vec<User> {
        let mut users: Vec<User> = self
            .users
            .values()
            .filter(|user| user.locked_at.is_some())
            .cloned()
            .collect();
        users.sort_by(|a, b| b.locked_at.cmp(&a.locked_at));
        users
    }

    /// Expire raw payloads and log lines that exceed the project's retention
    /// window. The event row, its issue link, and issue counters stay so triage
    /// history remains; only the raw reference, attachments, and object bytes go.
    /// Returns the object keys the caller must delete.
    pub fn apply_retention(&mut self, project: &Project) -> Vec<String> {
        let cutoff = Utc::now() - Duration::days(project.retention_days as i64);
        let now = Utc::now();
        let expired: Vec<Uuid> = self
            .events
            .values()
            .filter(|e| {
                e.project_id == project.id && e.received_at < cutoff && e.raw_object_key.is_some()
            })
            .map(|e| e.id)
            .collect();
        let mut keys = Vec::new();
        for id in expired {
            if let Some(event) = self.events.get_mut(&id) {
                keys.extend(
                    event
                        .attachments
                        .iter()
                        .filter_map(|attachment| attachment.object_key.clone()),
                );
                if let Some(key) = event.raw_object_key.take() {
                    keys.push(key);
                }
                event.attachments.clear();
                event.raw_expired_at = Some(now);
            }
        }
        self.logs
            .retain(|l| !(l.project_id == project.id && l.timestamp < cutoff));
        keys
    }

    /// Allocate a stable slug for a new organisation, appending `-2`, `-3`, … when
    /// the derived slug is already taken (case-insensitive).
    pub fn unique_org_slug(&self, name: &str) -> String {
        let base = slugify(name);
        let taken: Vec<String> = self
            .orgs
            .values()
            .map(|org| org.slug.to_ascii_lowercase())
            .collect();
        if !taken.iter().any(|slug| slug == &base) {
            return base;
        }
        for suffix in 2..10_000u32 {
            let candidate = format!("{base}-{suffix}");
            if !taken.iter().any(|slug| slug == &candidate) {
                return candidate;
            }
        }
        format!("{base}-{}", Uuid::new_v4().simple())
    }

    /// Fill in slugs for organisations created before slugs existed. Returns the
    /// number of organisations updated.
    pub fn ensure_org_slugs(&mut self) -> usize {
        let pending: Vec<(Uuid, String)> = self
            .orgs
            .values()
            .filter(|org| org.slug.is_empty())
            .map(|org| (org.id, org.name.clone()))
            .collect();
        let mut updated = 0;
        for (id, name) in pending {
            let slug = self.unique_org_slug(&name);
            if let Some(org) = self.orgs.get_mut(&id) {
                org.slug = slug;
                updated += 1;
            }
        }
        updated
    }

    /// Next per-project issue number. Numbers are assigned once at creation and
    /// never reused.
    pub fn next_issue_number(&self, project_id: Uuid) -> u64 {
        self.issues
            .values()
            .filter(|issue| issue.project_id == project_id)
            .map(|issue| issue.number)
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Issue a hashed API token. The plaintext is returned once and never stored.
    pub fn create_api_token(&mut self, user_id: Uuid, label: &str) -> (String, ApiToken) {
        let plaintext = format!("scout_pat_{}", crate::auth::new_token());
        let token = ApiToken {
            id: Uuid::new_v4(),
            user_id,
            label: label.to_string(),
            token_hash: crate::auth::hash_token(&plaintext),
            token_prefix: crate::auth::token_prefix(&plaintext, 14),
            created_at: Utc::now(),
            last_used_at: None,
            revoked_at: None,
        };
        self.api_tokens_by_hash
            .insert(token.token_hash.clone(), token.id);
        self.api_tokens.insert(token.id, token.clone());
        (plaintext, token)
    }

    /// Resolve a presented plaintext token. Revoked tokens resolve to `None`.
    pub fn active_api_token(&self, plaintext: &str) -> Option<&ApiToken> {
        let id = self
            .api_tokens_by_hash
            .get(&crate::auth::hash_token(plaintext))?;
        self.api_tokens.get(id).filter(|token| token.is_active())
    }

    pub fn api_tokens_for_user(&self, user_id: Uuid) -> Vec<ApiToken> {
        let mut tokens: Vec<ApiToken> = self
            .api_tokens
            .values()
            .filter(|token| token.user_id == user_id)
            .cloned()
            .collect();
        tokens.sort_by_key(|token| std::cmp::Reverse(token.created_at));
        tokens
    }

    /// Tokens owned by users who belong to the organisation, with their owner's
    /// email, newest first. Metadata only.
    pub fn api_tokens_for_org(&self, org_id: Uuid) -> Vec<(String, ApiToken)> {
        let project_ids: std::collections::HashSet<Uuid> = self
            .projects
            .values()
            .filter(|project| project.org_id == org_id)
            .map(|project| project.id)
            .collect();
        let mut members: std::collections::HashSet<Uuid> = self
            .org_members
            .keys()
            .filter(|(_, member_org)| *member_org == org_id)
            .map(|(user, _)| *user)
            .collect();
        for (user, project) in self.memberships.keys() {
            if project_ids.contains(project) {
                members.insert(*user);
            }
        }
        let mut rows: Vec<(String, ApiToken)> = self
            .api_tokens
            .values()
            .filter(|token| members.contains(&token.user_id))
            .map(|token| {
                let email = self
                    .users
                    .get(&token.user_id)
                    .map(|user| user.email.clone())
                    .unwrap_or_default();
                (email, token.clone())
            })
            .collect();
        rows.sort_by_key(|(_, token)| std::cmp::Reverse(token.created_at));
        rows
    }

    /// Revoke a token. Returns false when the id is unknown.
    pub fn revoke_api_token(&mut self, token_id: Uuid) -> bool {
        match self.api_tokens.get_mut(&token_id) {
            Some(token) => {
                if token.revoked_at.is_none() {
                    token.revoked_at = Some(Utc::now());
                }
                true
            }
            None => false,
        }
    }

    /// Record a successful use, at most once per hour.
    pub fn touch_api_token(&mut self, token_id: Uuid) {
        if let Some(token) = self.api_tokens.get_mut(&token_id) {
            let stale = token
                .last_used_at
                .is_none_or(|at| at < Utc::now() - Duration::hours(1));
            if stale {
                token.last_used_at = Some(Utc::now());
            }
        }
    }
}

/// Build the demo store. Raw payloads are returned separately so the caller can
/// write them to object storage (which is async) before serving requests.
pub fn seed(self_url: &str) -> (Store, Vec<(String, Vec<u8>)>) {
    let mut raws = Vec::new();
    let mut store = Store::empty();
    let now = Utc::now();

    let org_id = Uuid::new_v4();
    store.orgs.insert(
        org_id,
        Organisation {
            id: org_id,
            name: "Acme".into(),
            slug: "acme".into(),
            sso_provider: None,
            created_by: None,
            is_demo: true,
            created_at: now - Duration::days(60),
        },
    );

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
            confidence_threshold: 0.5,
        }),
        confidence_threshold: 0.5,
        deleted_at: None,
        deleted_by: None,
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
        confidence_threshold: 0.5,
        deleted_at: None,
        deleted_by: None,
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
        let key_hash = crate::auth::hash_token(key);
        let ingest_key = IngestKey {
            id: Uuid::new_v4(),
            project_id,
            label: label.into(),
            key_hash,
            key_prefix: crate::auth::token_prefix(key, 12),
            created_at: now - Duration::days(40),
            revoked: false,
        };
        store.keys.insert(ingest_key.id, ingest_key);
    }

    let demo_hash = demo_password_hash();
    let demo = User {
        id: Uuid::new_v4(),
        email: "demo@scout.local".into(),
        name: "Dana Demo".into(),
        password_hash: Some(demo_hash.clone()),
        is_operator: false,
        disabled_at: None,
        locked_at: None,
    };
    let triager = User {
        id: Uuid::new_v4(),
        email: "triager@scout.local".into(),
        name: "Toni Triager".into(),
        password_hash: Some(demo_hash.clone()),
        is_operator: false,
        disabled_at: None,
        locked_at: None,
    };
    let viewer = User {
        id: Uuid::new_v4(),
        email: "viewer@scout.local".into(),
        name: "Vic Viewer".into(),
        password_hash: Some(demo_hash),
        is_operator: false,
        disabled_at: None,
        locked_at: None,
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
    // The demo owner also administers the demo organisation, so the
    // provisioning endpoints are usable in demo mode.
    store.org_members.insert((demo.id, org_id), OrgRole::Admin);

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
        vec![Attachment::new(
            "checkout-context.txt".into(),
            "text/plain".into(),
            b"cart=8f21\ncurrency=undefined\nroute=/checkout\n".to_vec(),
        )],
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

    // Attachment bytes live in object storage too; the event keeps metadata only.
    let mut stored_attachments = Vec::with_capacity(attachments.len());
    for (index, attachment) in attachments.into_iter().enumerate() {
        let key = crate::objects::attachment_key(project.id, event_id, index);
        raw_objects.push((key.clone(), attachment.data.clone()));
        stored_attachments.push(crate::models::Attachment {
            filename: attachment.filename,
            content_type: attachment.content_type,
            size: attachment.size,
            object_key: Some(key),
            data: Vec::new(),
        });
    }

    let issue = Issue {
        id: issue_id,
        project_id: project.id,
        number: store.next_issue_number(project.id),
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
        first_presented_at: None,
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
            attachment_count: stored_attachments.len(),
            event,
            attachments: stored_attachments,
        },
    );
    store
        .seen_event_ids
        .insert((project.id, sentry_event_id), event_id);
    issue_id
}
