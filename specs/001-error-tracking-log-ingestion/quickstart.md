# Quickstart Validation: Scout

**Feature**: `001-error-tracking-log-ingestion` | **Date**: 2026-10-31

A runnable guide for proving the feature end to end. It links to
[data-model.md](./data-model.md) and the [contracts](./contracts/) instead of duplicating their rules.

## Prerequisites

- Rust 1.99 with the `wasm32-unknown-unknown` target
- Trunk 0.22 (for the Sycamore/Tailwind UI; no Node.js)
- Docker with Compose (for PostgreSQL and MinIO)
- `curl` and `jq` for the manual checks below

## Build and automated tests

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check

(cd ui && cargo check --target wasm32-unknown-unknown)
(cd ui && trunk build --release)
```

Expected: all unit and in-crate integration tests pass; clippy and formatting are clean; `ui/dist`
contains `index.html`, the JS bundle, the WASM module, and a compiled Tailwind stylesheet.

The integration suite in `src/tests.rs` already covers grouping, deduplication, raw byte identity,
key scoping, suggestions, classifier outage, resolve/reopen, roles, live role changes, logs, and
search. The scenarios below add the external-service wiring and the flows that need a browser.

## Start the stack

```sh
export SCOUT_OPERATOR_EMAIL=operator@example.com
export SCOUT_OPERATOR_PASSWORD='change-this-before-first-start'
docker compose up --build
```

Expected: `scout`, `postgres`, and `minio` become healthy. The UI is reachable at
`http://localhost:8080`, and `GET /api/health` returns `200` with `database: "ok"` and
`object_store: "ok"`.

On first start, the deployment operator is created from `SCOUT_OPERATOR_EMAIL` and
`SCOUT_OPERATOR_PASSWORD` if no operator exists. Later starts ignore these variables and do not reset
the password. For local exploration, the prototype also seeds demo, triager, and viewer accounts on
the `scout-web` project. Production deployments create accounts through the initial-admin setup token
and invitations; see [contracts/api.md](./contracts/api.md#authentication).

## Scenario 0 — Bootstrap and first organisation (SC-011)

1. Set the operator environment variables and start the stack from empty PostgreSQL and MinIO.
2. Sign in as the operator:

```sh
curl -sS -X POST 'http://localhost:8080/api/login' \
  -H 'Content-Type: application/json' \
  -d '{"email":"operator@example.com","password":"change-this-before-first-start"}'
```

3. Create an organisation and its initial admin:

```sh
curl -sS -X POST 'http://localhost:8080/api/orgs' \
  -H "Authorization: Bearer <operator-token>" \
  -H 'Content-Type: application/json' \
  -d '{"name":"Acme","initial_admin_email":"admin@example.com"}'
```

4. Share the returned `initial_admin_setup_token` out of band. Redeem it to set the admin password:

```sh
curl -sS -X POST 'http://localhost:8080/api/invitations/<setup-token>/accept' \
  -H 'Content-Type: application/json' \
  -d '{"name":"Acme Admin","password":"admin-password"}'
```

5. Sign in as `admin@example.com`; the response includes `org_admin` access and `recovery_codes`.
6. Restart the stack with the same operator variables and sign in with the **changed** operator
   password (if changed). The restart must not reset it.

Expected: the operator can create the organisation and obtain a one-time setup token; the token works
once; the initial admin can sign in, create a project, and issue its first ingest key. Reusing or
expiring the setup token returns `410` and grants nothing. The operator cannot create any other
accounts.

## Scenario 1 — Fresh deployment to first issue (SC-001)

1. Start from an empty database and object store.
2. Send the sample envelope:

```sh
curl -sS -X POST 'http://localhost:8080/api/1/envelope/?sentry_key=scout_demo_key' \
  -H 'Content-Type: application/x-sentry-envelope' \
  --data-binary @examples/sample-envelope.txt
```

3. Open the project's issue list.

Expected: within 10 minutes of the stack starting, the project shows one new issue titled
`SmokeError: grouping check` with count 3, populated first/last-seen, and an attachment count of 1.
The response from step 2 includes three event ids. Resolve, reopen, and ignore the issue in turn; each
transition appears in its history with the actor and time.

## Scenario 2 — Grouping at scale (SC-002, SC-009)

Send 1,000 distinct event ids that share a fingerprint, then reload the issue list.

```sh
for i in $(seq 1 1000); do
  curl -sS -o /dev/null -X POST 'http://localhost:8080/api/1/store/?sentry_key=scout_demo_key' \
    -H 'Content-Type: application/json' \
    -d "{\"event_id\":\"$(printf '%032x' "$i")\",\"message\":\"ScaleError: same identity\",\"fingerprint\":[\"quickstart-scale\"],\"exception\":{\"values\":[{\"type\":\"ScaleError\",\"value\":\"same identity\"}]}}"
done
```

Expected: exactly one issue with count 1,000. Re-sending any one of those event ids leaves the count
unchanged.

## Scenario 3 — Raw payload identity (SC-003)

1. Submit a single event through `POST /api/1/store/` and keep the exact request body in a file.
2. Open the issue and download the raw payload, or:

```sh
curl -sS -H "Authorization: Bearer <token>" \
  'http://localhost:8080/api/issues/<issue-id>/raw' -o received.bin
cmp request.json received.bin
```

Expected: `cmp` reports no difference. The `X-Scout-Event-Id` response header matches the submitted
event id. After retention expires the payload, the same request returns `404` and the issue detail
shows `raw_available: false`.

## Scenario 4 — Logs and unified search

```sh
curl -sS -X POST 'http://localhost:8080/api/1/logs/?sentry_key=scout_demo_key' \
  -H 'Content-Type: application/json' \
  -d '{"logs":[{"level":"warning","message":"pool saturation observed on orders","source":"orders-api","attributes":{"pool":"primary","waiting":48}}]}'
```

Expected: `{"accepted":1}`. `GET /api/projects/{id}/logs?q=pool` returns the line, and
`GET /api/projects/{id}/search?q=pool` returns it alongside any matching issues.

## Scenario 5 — Suggestion, confirmation, override

The seeded `scout-web` project points at the built-in mock endpoint.

1. Send a new event and open its issue.
2. Observe the suggestion (the mock returns `issue` for a normal error).
3. Confirm it, then override it to `log`.

Expected: the suggestion appears within the bounded wait; confirming sets a human decision with
`overridden: false`; overriding sets `category: "log"` with `overridden: true`; the original
suggestion remains visible. No item is ever hidden or reclassified automatically, and the stored
event, grouping, count, and raw payload are unchanged. The issue list's `category=log` filter returns
the issue after the override.

## Scenario 6 — Classifier outage (SC-004)

1. `POST /mock-jev/outage` with `{"down": true}`, or point the project at an unreachable endpoint in
   Settings.
2. Send a new event.

Expected: ingest returns `200` promptly, the issue is stored and visible, and it carries no
suggestion. No accepted item is dropped or delayed beyond the normal ingest bound. Traffic continues
normally while the endpoint is down.

## Scenario 7 — Roles and live access changes (SC-006, SC-007)

1. Sign in as the viewer and open `scout-web`: issue list and raw payloads are readable.
2. Attempt to resolve an issue or open Settings: both return `403`.
3. As the owner, change the viewer's role to triager through
   `PUT /api/projects/{id}/members/{user_id}`.
4. Using the viewer's existing session, resolve an issue.

Expected: step 2 is denied; step 4 succeeds without signing in again or restarting anything. A
viewer requesting another project's issues, logs, or raw payload gets `403`, and no existence
information is returned. Demoting or removing the last owner returns `400`.

## Scenario 8 — Invitation and password sign-in

1. As an organisation admin, create an invitation for a new email with the `triager` role.
2. Redeem the returned one-time token with a name and password.
3. Sign in with the new email and password.

Expected: the token works once and creates the account with the project role; reuse, expiry, or
revocation returns `410` and grants nothing. Passwords are never echoed, and no email is sent. The
creation response includes single-use recovery codes; redeeming one at `POST /api/recover` sets a new
password without email and cannot be reused.

## Scenario 9 — Retention

1. Set the project's retention to 1 day.
2. Insert a log line with a timestamp older than 24 hours (or wait for the sweep).
3. Request the affected issue's raw payload.

Expected: the old log line is gone from list and search results; the raw payload returns `404`; the
issue metadata, count, and history remain visible. No stored payload that is still within retention
is ever rewritten.

## Scenario 10 — Performance sanity (SC-005)

Drive at least 500 events/s for one minute against a 4 vCPU deployment and record:

- ingest acceptance rate and p95 acknowledgement latency
- time from acceptance to visibility in the issue list

Expected: no dropped accepted events, p95 acknowledgement under 150 ms for 10 KB events, and 95% of
items visible within 10 seconds. This is the planning target from
[research.md](./research.md#r11-performance-sizing), not a spec-mandated ceiling.

## Troubleshooting

| Symptom | Check |
|---------|-------|
| `401` on ingest | key is revoked, belongs to another project, or the project reference is wrong |
| `429` on ingest | the global in-flight cap is saturated; retry after the `Retry-After` interval |
| `404` on raw payload | retention removed it; the issue detail should say `raw_available: false` |
| Suggestion missing | endpoint unreachable, returned an unknown category, or confidence is below the threshold (default 0.5) |
| UI shows the "not built" page | run `trunk build --release` in `ui/` and restart, or set `SCOUT_UI_DIR` |
| Health reports a failing dependency | PostgreSQL/MinIO credentials or readiness in `docker-compose.yml` |
