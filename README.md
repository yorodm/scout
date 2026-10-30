# Scout

Self-hosted, Sentry-compatible error tracking and log ingestion.

Scout accepts the payloads existing Sentry SDKs already send, groups repeated errors into issues, and
stores raw events and logs — on your infrastructure, not a vendor's. It serves its own UI for triage,
project management and access control, and lets a project owner connect a [Jev](https://jev-api.com/docs/api)-compatible
decisions endpoint that *suggests* how an incoming item should be categorised (issue, log or raw event);
a human confirms.

> **Status: runnable prototype.** The ingest path, grouping worker, classification worker, REST API and
> UI are implemented in Rust. The metadata store and raw-payload store are in-memory adapters: the
> boundaries match [architecture.md](architecture.md), but Postgres and S3/MinIO are not wired up yet.
> See [Prototype shortcuts](#prototype-shortcuts) for the honest list.

## Quick start

```sh
cargo run
# open http://localhost:8080
```

Requires only a Rust toolchain (edition 2021). No Node build, database, or object store is needed for
the prototype. The server seeds an organisation, two projects, three issues, log lines and three demo
accounts on startup.

```sh
curl http://localhost:8080/api/health
# {"service":"scout","status":"ok","version":"0.1.0"}
```

### Demo accounts

Sign in from the UI. There are no passwords in the prototype; the session token is in-memory and is
invalidated on restart.

| Account | `scout-web` | `platform-api` |
| --- | --- | --- |
| `demo@scout.local` | owner | admin |
| `triager@scout.local` | triager | — |
| `viewer@scout.local` | viewer | — |

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
| UI | No-build SPA (vanilla JS + handcrafted CSS) served by Actix | Port to Sycamore + Tailwind; same API |
| Sessions | In-memory bearer tokens, no passwords | Password hashing, cookies, OIDC/SSO provider |
| Invites | Direct membership by seeded email | Email invitations and acceptance tokens |
| Retention | Sweep on ingest; per-project day window | Scheduled sweeper + object-store lifecycle rules |
| Classifier | Built-in mock endpoint at `/mock-jev` | Any Jev-compatible endpoint per project |
| Scaling | One process, one worker | Postgres `FOR UPDATE SKIP LOCKED` job queue, multiple replicas |

Everything else follows the documented behaviour: ingest stores the original bytes before replying,
grouping and classification happen off the request path, classification has a bounded timeout and at
most one retry, and a classifier failure is a normal "no suggestion" state rather than an error.

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

## Tests

```sh
cargo test
```

Nine unit tests cover envelope parsing (length-prefixed and newline-delimited), truncation rejection,
Sentry auth parsing, log attribute normalisation, fingerprint stability, explicit fingerprints, and
message normalisation. One integration test parses `web/app.js` with a real JavaScript parser so a
syntax error cannot slip into the UI.

## Deployment

```sh
docker compose up --build
```

The compose file runs the prototype as one service. The production shape — `scout` + `postgres` +
`minio`, all from one compose file — is described in [architecture.md](architecture.md); those two
services are not part of the prototype yet.

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
