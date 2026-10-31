# Scout Constitution

## Core Principles

### I. Ingest Never Depends on Classification (NON-NEGOTIABLE)

Accepted telemetry MUST be durably stored and acknowledged without waiting for the decision model.
Classification is always best-effort and off the request path: a bounded call, at most one retry, and
any failure (unreachable, slow, malformed, low-confidence) yields **no suggestion**, never a dropped,
delayed, or wrongly classified accepted item.

**Rationale**: A classifier outage during an incident is exactly when telemetry matters most. The
core loop must remain deterministic even when every optional dependency is unavailable.

### II. Raw Payloads Are Immutable and Authoritative

The exact bytes received for an event MUST be stored write-once and MUST never be rewritten.
Grouping, suggestions, decisions, counts, and state history are derived metadata that can be
recomputed from the stored payload; they MUST NOT mutate it. Retention is the only deletion path for
raw payloads and log lines.

**Rationale**: Immutability makes triage auditable, supports re-grouping when rules change, and
guarantees that an issue always resolves to what the sender actually transmitted.

### III. Project-Scoped, Server-Side Authorisation

Every request MUST be authorised on the server against exactly one project, or against the
deployment-operator scope where the operation is deployment-level. Ingest keys are per project,
hashed at rest, and cannot read data. Dashboard sessions resolve a role per project. A request for
another project MUST be denied without leaking whether the resource exists.

**Rationale**: Scout is multi-tenant by organisation and project. Client-side checks and implicit
trust in the UI are insufficient for a self-hosted system that stores other teams' telemetry.

### IV. One Deployment Unit, Minimal Moving Parts

Scout MUST ship as one application binary plus PostgreSQL and S3-compatible object storage, startable
from one compose file. The core loop MUST NOT require a message broker, search cluster, or external
identity provider. The Sycamore UI MUST be built by Trunk, including Tailwind through Trunk's
built-in pipeline; Node.js is not part of the toolchain.

**Rationale**: Self-hosting is the delivery model. Every additional service raises the cost of
operating Scout and the number of things that can fail during an incident.

### V. Evidence Over Assumption: Test the Real Paths

Every functional requirement MUST have an executable check. Cross-cutting behaviour MUST be exercised
through the real Actix application with in-crate integration tests; unit tests belong beside the code
they cover. The backend MUST remain a single binary crate: no `lib.rs` split and no separate `tests/`
integration crate. PostgreSQL migrations MUST be exercised against a real PostgreSQL instance, and
object-store behaviour MUST be covered by a compose-based smoke test.

**Rationale**: A prototype is only credible when routing, authentication, ingest, grouping, and
classification are verified together. In-process tests keep the feedback loop fast without
sacrificing the real request path.

## Additional Constraints

### Security and Privacy

- Passwords, session tokens, invitation/setup tokens, recovery codes, and ingest keys MUST be stored
  only as strong hashes; plaintext values are shown once and never persisted or logged.
- Model summaries MUST be redacted and capped at 2 KB; full raw payloads MUST NOT cross the model
  boundary.
- The deployment operator MUST be seeded only when no operator exists; missing bootstrap values on a
  fresh deployment MUST fail startup with an actionable error. Later starts MUST NOT reset an existing
  operator password.
- Every administrative action (roles, keys, model, retention, organisation creation, initial-admin
  setup) MUST be recorded in the audit log.
- API tokens issued for the Sentry-compatible surface MUST be stored only as strong hashes, shown in
  plaintext once, and MUST inherit their owner's live project and organisation roles on every
  request. A token MUST NOT carry an independent scope list and MUST NOT grant any capability its
  owner does not currently hold.

### Performance and Reliability

- Ingest SHOULD sustain at least 500 events/s on a 4 vCPU deployment at p95 acknowledgement ≤150 ms
  for 10 KB events; 95% of accepted items SHOULD be visible in issue or log lists within 10 seconds.
- The global in-flight ingest cap defaults to 256 and returns `429` with `Retry-After` when saturated.
- Ingest requests and individual attachments are limited to 10 MB; oversized input is rejected with
  `413` and nothing partial is persisted.
- A storage failure MUST return a retryable `503`; an event MUST NOT be acknowledged before its raw
  payload and metadata are durably committed.

### Compatibility and Scope

- Scout supports Sentry **ingestion** and a bounded Sentry **read-API compatibility** surface: only
  the endpoints, parameters, response shapes, pagination, and error format ratified with feature 002
  in `specs/002-sentry-api-compatibility/spec.md`. That surface MUST be opt-in and disabled by
  default, MUST derive every response from existing organisation, project, issue, and role data with
  no parallel data model, and MUST report unsupported capability honestly with a documented empty
  collection or an explicit `501` rather than an invented result.
- Sentry metrics, tracing, SCIM, Teams, and any endpoint outside the ratified compatibility contract
  remain out of scope and MUST NOT be implied.
- The classification integration targets a Jev-compatible decisions endpoint per project; with no
  endpoint configured, classification is inert and ingest proceeds normally.
- Per-project retention defaults to 30 days and applies to raw payloads and log lines; issue metadata
  and triage history remain until the project is deleted.

## Development Workflow & Quality Gates

- Every change MUST pass `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test`.
- UI changes MUST pass `cargo check --target wasm32-unknown-unknown` and `trunk build --release` in
  `ui/`.
- New behaviour MUST update the relevant specification, plan, contracts, and quickstart scenario.
- Migrations MUST be idempotent and safe for concurrent starts; schema changes MUST be reversible or
  accompanied by an explicit migration plan.
- Security-sensitive changes (authentication, authorisation, ingest parsing, classifier boundary,
  secret handling) require explicit review against this constitution.
- Complexity that violates a principle MUST be justified in the plan's Complexity Tracking section or
  rejected. Unjustified exceptions are review blockers.

## Governance

This constitution supersedes conflicting local practices, comments, and conventions. When a conflict
is found, the constitution prevails until it is formally amended.

**Amendment procedure**

1. Propose the amendment in a pull request that states the change, its rationale, and any migration
   or compatibility impact.
2. Obtain approval from the project's maintainer(s).
3. Update this file, increment the version, and record the amendment date.
4. Update dependent specifications, plans, contracts, and guidance when the amendment changes them.

**Versioning policy**: semantic versioning. MAJOR for removal or redefinition of a principle in a
backward-incompatible way; MINOR for a new principle, section, or materially expanded guidance;
PATCH for clarifications, wording fixes, or non-semantic refinements.

**Compliance review**: every pull request and review MUST verify compliance with the Core Principles
and Additional Constraints. A change that cannot comply MUST be rejected or accompanied by an
accepted constitution amendment. Runtime guidance lives in `README.md`, `architecture.md`, and the
active feature's `spec.md`, `plan.md`, and `contracts/`.

**Version**: 1.1.0 | **Ratified**: 2026-10-31 | **Last Amended**: 2026-11-01
