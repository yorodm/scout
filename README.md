# Scout

Self-hosted, Sentry-compatible error tracking and log ingestion.

Scout accepts the payloads existing Sentry SDKs already send, groups repeated errors into issues, and
stores raw events and logs — on your infrastructure, not a vendor's. It serves a Sycamore UI for
triage, project management and access control, and lets a project owner connect a
[Jev](https://jev-api.com/docs/api)-compatible decisions endpoint that *suggests* how an incoming item
should be categorised (issue, log or raw event); a human confirms.

> **Status: runnable prototype.** The ingest path, grouping worker, classification worker, REST API and
> Sycamore/Tailwind UI are implemented in Rust. The metadata store and raw-payload store are in-memory
> adapters: the boundaries match [architecture.md](architecture.md), but Postgres and S3/MinIO are not
> wired up yet. See [Prototype shortcuts](#prototype-shortcuts) for the honest list.

## Stack

- **Backend** — Rust + Actix Web: ingest API, project API, background worker.
- **UI** — Sycamore 0.9 (CSR), compiled to WebAssembly with Trunk; Tailwind CSS v4 compiled by
  Trunk's built-in `tailwind-css` pipeline. **No Node or npm is involved.**
- **Prototype storage** — in-memory metadata and raw payloads.
- **Target storage** — PostgreSQL (metadata) and S3/MinIO (raw payloads), behind the same store
  boundaries.

## Prerequisites

- A Rust toolchain.
- The `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- [Trunk](https://trunkrs.dev/): use a prebuilt release binary or `cargo install trunk --locked`.
  Trunk downloads the matching `wasm-bindgen` and Tailwind binaries on first build.

No Node.js is required or used.

## Quick start

Build the UI, then run the single Actix binary:

```sh
rustup target add wasm32-unknown-unknown
# Install Trunk once (prebuilt release is fastest).
(cd ui && trunk build --release)
export SCOUT_OPERATOR_EMAIL=operator@scout.local
export SCOUT_OPERATOR_PASSWORD=change-me-now
export SCOUT_DEMO_MODE=true
cargo run
# open http://localhost:8080
```

Scout refuses to start when no deployment operator exists and either bootstrap value is missing or
blank. The operator is seeded once with an Argon2id password hash; later starts never reset it. Set
`SCOUT_SESSION_TTL_DAYS`, `SCOUT_RETENTION_DAYS`, `SCOUT_CONFIDENCE_THRESHOLD`, and
`SCOUT_INGEST_MAX_IN_FLIGHT` to override their defaults (7, 30, 0.5, and 256).
`SCOUT_SENTRY_API_COMPAT_ENABLED=true` turns on the opt-in Sentry read-API surface described below;
it is off by default.

`SCOUT_DEMO_MODE=true` seeds the demo organisation, accounts, and keys and enables the
`/api/demo/*` prototype routes, including an unauthenticated store reset. Leave it unset (the
default) for anything reachable by real users; a fresh deployment then starts with an empty store and
only the operator account.

Setting `DATABASE_URL` also requires `SCOUT_S3_ENDPOINT` and `SCOUT_S3_BUCKET`; startup fails with an
actionable error rather than silently keeping raw payloads in memory.

`cargo run` serves the API and the contents of `ui/dist`. If `ui/dist/index.html` is missing, the
server returns a page explaining how to build it instead of failing.

### Live development

Run the backend and Trunk's dev server side by side:

```sh
# terminal 1
cargo run                     # API on :8080

# terminal 2
cd ui && trunk serve          # UI on :8081, proxies /api and /mock-jev to :8080
# open http://localhost:8081
```

Trunk rebuilds the WASM bundle and Tailwind CSS on change. The proxy configuration lives in
`ui/Trunk.toml`.

```sh
curl http://localhost:8080/api/health
# {"object_store":"memory","service":"scout","status":"ok","storage":"ok","version":"0.1.0"}
```

Health returns `503` with `"status":"degraded"` when metadata storage or object storage is
unreachable.

### Demo accounts

With `SCOUT_DEMO_MODE=true`, seeded demo accounts share the password `scout-demo`. Sign-in returns an
opaque bearer token; only its SHA-256 hash is stored, and it expires after `SCOUT_SESSION_TTL_DAYS`
(7 by default). Signing out revokes the token.

| Account | `scout-web` | `platform-api` |
| --- | --- | --- |
| `demo@scout.local` | owner | admin |
| `triager@scout.local` | triager | — |
| `viewer@scout.local` | viewer | — |

The deployment operator (`SCOUT_OPERATOR_EMAIL`) is a separate, deployment-level principal: it can
create organisations but cannot read a project's issues, logs, or raw payloads.

### Seeded ingest keys

| Project | DSN id | Key |
| --- | --- | --- |
| `scout-web` | `1` | `scout_demo_key` |
| `platform-api` | `2` | `scout_platform_key` |

The DSN to give an SDK is `http://<key>@<host>:8080/<dsn-id>`, for example
`http://scout_demo_key@localhost:8080/1`.

## Try the flows

### 1. Send a real Sentry envelope

```sh
curl -X POST 'http://localhost:8080/api/1/envelope/?sentry_key=scout_demo_key' \
  -H 'Content-Type: application/x-sentry-envelope' \
  --data-binary @examples/sample-envelope.txt
```

The sample carries three event items with the same explicit fingerprint and one attachment. The ingest
worker replies immediately; the background worker then produces **one issue with count 3**. Open it in
the UI to see the stack trace, breadcrumbs, tags, attachment metadata, and the exact bytes received.

Point an actual Sentry SDK at the DSN above — only the DSN changes.

The legacy endpoint is also supported:

```sh
curl -X POST 'http://localhost:8080/api/1/store/?sentry_key=scout_demo_key' \
  -H 'Content-Type: application/json' \
  -d '{"event_id":"dddddddddddddddddddddddddddddddd","message":"LegacyError: stored","level":"error","fingerprint":["legacy"],"exception":{"values":[{"type":"LegacyError","value":"stored"}]}}'
```

### 2. Demonstrate grouping at scale

In the UI, **Send test event** → set occurrences to `1000` → send. Scout posts 1,000 distinct event
ids that share a fingerprint; the issue list ends with exactly one issue and count `1,000`.

### 3. Watch the classifier suggest, then confirm or override

The seeded `scout-web` project points at the built-in mock Jev endpoint. New items get a suggestion
within the bounded wait (under a second). Open an issue and use **Confirm** or **Override**. The human
decision is stored alongside the original suggestion.

To prove ingest is unaffected by an outage: **Settings → Simulate classifier outage**, then send an
event. Ingest accepts and stores it; the issue carries **no suggestion**. Restore the classifier and
send another event to see a suggestion again.

### 4. Logs and unified search

```sh
curl -X POST 'http://localhost:8080/api/1/logs/?sentry_key=scout_demo_key' \
  -H 'Content-Type: application/json' \
  -d '{"logs":[{"level":"warning","message":"pool saturation observed on orders","source":"orders-api","attributes":{"pool":"primary","waiting":48}}]}'
```

Use **Logs** to filter by level and text, or **Search** to query issues and log lines together.

### 5. Roles are enforced server-side

Sign in as `viewer@scout.local`: the project list contains only `scout-web`, settings are hidden and
rejected with `403`, and resolve/confirm actions are hidden and rejected with `403`. Sign in as
`triager@scout.local` to resolve and confirm but still be denied project settings. Role changes made on
**Members** take effect on the next request, without a restart.

## Prototype shortcuts

These are deliberate and visible, not silent gaps:

| Area | Prototype | Next step |
| --- | --- | --- |
| Metadata | In-memory `Store` behind ordinary methods | Postgres adapter + migrations |
| Raw payloads | In-memory `Event.raw`, append-only semantics | S3/MinIO adapter; same immutable key/value contract |
| UI | Sycamore 0.9 CSR + Tailwind CSS, built by Trunk | Add SSR/hydration if needed; refine Tailwind component layer |
| Sessions | Hashed opaque tokens with expiry; password sign-in | Cookies, OIDC/SSO provider |
| Invites | Direct membership by seeded email | Email invitations and acceptance tokens |
| Retention | Sweep on ingest; per-project day window | Scheduled sweeper + object-store lifecycle rules |
| Classifier | Built-in mock endpoint at `/mock-jev` | Any Jev-compatible endpoint per project |
| Scaling | One process, one worker | Postgres `FOR UPDATE SKIP LOCKED` job queue, multiple replicas |

Everything else follows the documented behaviour: ingest stores the original bytes before replying,
grouping and classification happen off the request path, classification has a bounded timeout and at
most one retry, and a classifier failure is a normal "no suggestion" state rather than an error.

## UI layout

```
ui/
  index.html              Trunk entry point (rust + tailwind-css assets)
  Trunk.toml              build/serve config and /api, /mock-jev proxies
  styles/tailwind.css     Tailwind entry; imports the design system
  styles/design-system.css  design tokens and component classes (processed by Tailwind)
  src/main.rs             mounts the Sycamore App
  src/api.rs              typed fetch client and response models
  src/state.rs            reactive store, routing, actions
  src/util.rs             formatting and browser helpers
  src/components/         app shell, login, issues, logs, search, members,
                          settings, tokens, modals, toasts
```

The backend serves `ui/dist` (override with `SCOUT_UI_DIR`). Ingestion and the REST API are unchanged
by the UI choice.

## API surface

| Method & path | Purpose |
| --- | --- |
| `POST /api/{project}/envelope/` | Sentry envelope ingest (event, attachment, log items) |
| `POST /api/{project}/store/` | Legacy single-event ingest |
| `POST /api/{project}/logs/` | Structured log lines (JSON or NDJSON) |
| `GET /api/projects` | Projects the session can access |
| `GET /api/projects/{id}/issues` | Issue list with filters and sorting |
| `GET /api/projects/{id}/logs` | Log search |
| `GET /api/projects/{id}/search` | Issues and logs in one result |
| `GET /api/issues/{id}` | Issue detail + latest event + attachment metadata |
| `GET /api/issues/{id}/raw` | Exact stored bytes |
| `POST /api/issues/{id}/state` | Resolve / reopen (triager+) |
| `POST /api/issues/{id}/categorisation` | Confirm / override suggestion (triager+) |
| `GET/POST /api/projects/{id}/members` | List / add project-scoped members |
| `PUT/DELETE /api/projects/{id}/members/{user_id}` | Change role / remove a member (admin+) |
| `GET/POST /api/projects/{id}/keys` | List / create per-project ingest keys (admin+) |
| `DELETE /api/projects/{id}/keys/{key_id}` | Revoke an ingest key (admin+) |
| `GET/PUT /api/projects/{id}/settings` | Retention and decision model (admin+) |
| `POST /mock-jev/v1/systemone` | Built-in mock decisions endpoint |
| `GET/POST /mock-jev/outage` | Toggle the simulated classifier outage |

Ingest authenticates with `X-Sentry-Auth`, `Authorization: Sentry ...`, or `?sentry_key=`. Every
dashboard request requires a bearer session token and is authorised against the project before data
is touched.

### Token management

| Method & path | Purpose |
| --- | --- |
| `GET /api/me/tokens` | List your tokens (metadata only, never the secret) |
| `POST /api/me/tokens` | Create a token; the plaintext is returned once |
| `DELETE /api/me/tokens/{id}` | Revoke a token immediately and idempotently |
| `GET /api/orgs/{id}/tokens` | Organisation admins list members' tokens (metadata only) |
| `DELETE /api/orgs/{id}/tokens/{id}` | Organisation admins revoke a member's token |

The dashboard exposes the same flow at `#/tokens`.

### Sentry read-API compatibility (opt-in)

With `SCOUT_SENTRY_API_COMPAT_ENABLED=true`, the routes below are served under `/api/0/` and
authenticate with `Authorization: Bearer scout_pat_…`. They are disabled by default and answer `404`
when off. Scopes come from the token owner's live project and organisation roles.

| Path | Behaviour |
| --- | --- |
| `GET/PUT/DELETE /api/0/projects/{org}/{project}/issues/` | List, filter, sort, paginate; status mutation; `DELETE` is `501` |
| `GET /api/0/projects/{org}/{project}/tags/{key}/values/` | Distinct project tag values |
| `GET /api/0/organizations/{org}/issues/{issue}/tags/{key}/values/` | Distinct values for one issue |
| `GET /api/0/organizations/{org}/releases/{version}/commitfiles/` | `[]` for a known release, `404` otherwise |
| `GET /api/0/projects/{org}/{project}/servicehooks[/{hook}]/` | Empty list / `404` detail; mutations `501` |
| `GET /api/0/projects/{org}/{project}/users/` | Empty list; feedback submission `501` |
| `GET /api/0/projects/{org}/{project}/spike-protection/` | `404`; mutations `501` |
| `GET /api/0/organizations/{org}/sentry-app-installations/` | Empty list; mutations `501` |
| `POST/DELETE .../external-issues/...` | `501` |

Errors use Sentry's `{"detail": …}` envelope. Sentry metrics, tracing, SCIM, Teams, and any path
outside this table remain out of scope and are not implied by the compatibility surface. There is no
read-API rate limiting in this slice; revocation is the containment mechanism.

## Tests and checks

```sh
# Backend unit + integration tests (in-process Actix app)
cargo test

# UI type-check for the WASM target
(cd ui && cargo check --target wasm32-unknown-unknown)

# UI production build (Sycamore + Tailwind via Trunk)
(cd ui && trunk build --release)
```

The backend suite runs in-process with `actix_web::test` — no running server, browser or `curl`
required. Unit tests cover envelope parsing (length-prefixed and newline-delimited), truncation
rejection, Sentry auth parsing, log attribute normalisation, fingerprint stability, explicit
fingerprints, and message normalisation. Integration tests in `src/tests.rs` drive the same App
wiring as `main.rs` and cover:

- ingest and grouping — 3 identical events become one issue with count 3, and 1,000 events become
  one issue with count 1,000
- key scoping — an unknown key, a key for another project, and a revoked key all return `401`
- de-duplication of a retried event id
- byte-identical retrieval of the stored raw payload
- classifier suggestions, confirm/override, and an unreachable classifier leaving items without a
  suggestion while ingest still succeeds
- resolve/reopen state history
- role enforcement (viewer, triager, last-owner protection) and a role change taking effect without
  a restart
- log ingestion/search and unified search across issues and logs

`examples/sample-envelope.txt` remains a manual aid for trying a real Sentry envelope with `curl` or
an SDK; the integration suite builds its own fixtures.

## Deployment

```sh
docker compose up --build
```

The Docker build compiles the backend and the Sycamore/Tailwind UI in separate stages, then runs one
`scout` container with both the API and the static UI bundle. The production shape also adds
`postgres` and `minio`, all from one compose file, as described in
[architecture.md](architecture.md); those two services are not part of the prototype yet.

## Documentation

| Document | Covers |
| --- | --- |
| [goal-and-requirements.md](goal-and-requirements.md) | What Scout is and why: problem, users, goals, non-goals, capabilities, metrics |
| [architecture.md](architecture.md) | Components, ingest path, data ownership, grouping, authorisation, deployment |
| [specs/001-error-tracking-log-ingestion/spec.md](specs/001-error-tracking-log-ingestion/spec.md) | The feature specification the prototype implements |

## Design commitments

- **Store first, classify later.** Ingest writes the original payload and metadata, then replies.
  Grouping and categorisation happen off the request path.
- **The model suggests; a human decides.** A suggestion never gates storage, visibility or triage, and
  a classifier outage leaves items unclassified rather than misclassified. Only a summary — never the
  whole stored event — is sent to the model.
- **The raw payload is the source of truth.** Grouping and suggestions are derived from it.
- **Authorisation is server-side.** Orgs contain projects; roles (owner, admin, triager, viewer) are
  scoped to a project; ingest keys can write but never read.
- **Cheap to self-host.** Three services in production — `scout`, Postgres, S3/MinIO — no broker, no
  search cluster.

## License

[GPL-3.0](LICENSE).
