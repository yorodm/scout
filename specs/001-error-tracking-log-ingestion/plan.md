# Implementation Plan: Scout — Self-Hosted Error Tracking & Log Ingestion

**Branch**: `001-error-tracking-log-ingestion` | **Date**: 2026-10-31 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/001-error-tracking-log-ingestion/spec.md`

## Summary

Scout is a single Rust deployment that accepts Sentry-compatible telemetry unmodified, stores the raw
payload in S3-compatible object storage, groups repeated errors into issues in PostgreSQL, ingests
searchable log lines, and serves a Sycamore UI for triage and administration. A project's optional
Jev-compatible decisions endpoint only *suggests* a category after storage; ingest never waits on it.

The repository already contains a working prototype: Actix API, Sycamore/Tailwind UI, in-memory
metadata store, and an in-process grouping/classification worker. This plan takes that prototype to
the specified product: PostgreSQL and object storage replace the in-memory store, a deployment
operator is bootstrapped from environment variables (failing fast when bootstrap values are missing)
and creates organisations plus each organisation's initial admin, local accounts gain password
authentication, token-based invitations, and single-use recovery codes that any authenticated account
can reissue, existing accounts are bound to an initial-admin role only after authenticating and
consenting, the classifier applies redaction and a per-project confidence threshold, triage gains the
`ignored` state and decision-based filtering, ingest gains a configurable global backpressure cap, and
durability/authorisation rules become explicit.

## Technical Context

**Language/Version**: Rust 1.99 (edition 2021, stable toolchain); TypeScript is not used.

**Primary Dependencies**: Actix Web 4.18 and Actix Files; Tokio; Serde/`serde_json`; `uuid`; `chrono`;
`sha2`; `flate2`; `reqwest` (rustls); `tracing`; `sqlx` 0.8 (PostgreSQL, migrations, runtime-tokio-rustls);
`object_store` (S3/MinIO backend); `argon2` (Argon2id); `openidconnect` (OIDC); Sycamore 0.9,
`wasm-bindgen`, `gloo-net`, `web-sys` for the UI. Trunk 0.22 builds the UI, including Tailwind through
Trunk's built-in `tailwind-css` pipeline (no Node.js toolchain).

**Storage**: PostgreSQL for organisations, projects, users (including the deployment-operator flag),
roles, invitations and initial-admin setup tokens, sessions, recovery codes, ingest-key hashes,
issues, event metadata, suggestions, decisions, log lines, and jobs. S3-compatible object storage
(AWS S3 or MinIO) for immutable raw payloads and attachments. The in-memory `Store` remains as a test
adapter so the in-crate integration suite runs without external services.

**Testing**: `cargo test` — unit tests plus in-crate integration tests using `actix_web::test`, per the
project constraint that the backend must not be split into a library crate. No `tests/` integration
crate. SQL migrations are validated by running them against a disposable PostgreSQL instance in the
compose-based end-to-end check.

**Target Platform**: Linux x86_64 server in a container (the single deployment unit); browser WASM
(`wasm32-unknown-unknown`) for the UI.

**Project Type**: Web service + WASM single-page application in one Cargo workspace: the `scout` binary
and the `scout-ui` workspace member.

**Performance Goals**: Sustained ingest of at least 500 events/s on a 4 vCPU instance for 10 KB events
at p95 acknowledgement under 150 ms (raw write plus transaction), 1,000 events/s peak; 95% of accepted
items visible in issue/log lists within 10 seconds (SC-005); classifier work entirely off the request
path with a bounded 800 ms attempt (at most one retry).

**Constraints**: 10 MB per ingest request and per attachment (FR-007); no ingest dependency on the
classifier (FR-009, FR-018); a global in-flight ingest concurrency cap (default 256) returning `429`
with `Retry-After` (FR-035); raw payloads are write-once and never rewritten (FR-034); one compose
file for app, PostgreSQL, and object storage (FR-032); the deployment operator is seeded from
environment variables only when none exists, and startup fails with an actionable error when no
operator exists and bootstrap values are missing (FR-030); an existing account is bound to an
initial-admin role only after authentication and consent, without a password change (FR-025,
FR-030); no Node.js in the build; UI served by the same binary; migrations idempotent and safe for
concurrent starts.

**Scale/Scope**: Planning target for one deployment: up to 50 projects, up to 1,000 members, 30-day
default retention, 50 million stored events per month. These are sizing assumptions, not new product
requirements.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

**Gate result: PASS (no enforceable gates).**

`.specify/memory/constitution.md` is still the unmodified template: principle names and descriptions
are placeholders (`[PRINCIPLE_1_NAME]`, `[PRINCIPLE_1_DESCRIPTION]`, …), and no version, ratification
date, or governance rules have been recorded. There are no MUST/SHOULD statements to test this plan
against, and placeholder text is deliberately not treated as a gate.

Recommended follow-up outside this command: run `/speckit-constitution` to ratify the project
principles, then re-run the Constitution Check before implementation begins.

## Project Structure

### Documentation (this feature)

```text
specs/001-error-tracking-log-ingestion/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command)
│   ├── ingest.md
│   ├── api.md
│   └── classifier.md
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created here)
```

### Source Code (repository root)

```text
Cargo.toml                  # workspace + `scout` binary crate
migrations/                 # sqlx PostgreSQL migrations (added with persistence work)
src/
├── main.rs                 # bootstrap, operator seeding, app wiring, static UI fallback
├── config.rs               # environment configuration and defaults
├── api.rs                  # dashboard API (operator, orgs, sessions, issues, members, keys, settings)
├── auth.rs                 # Argon2id, sessions, invitations, setup tokens, recovery codes, OIDC
├── audit.rs                # admin action audit helper
├── ingest.rs               # Sentry envelopes, legacy store, log ingestion, dedupe
├── grouping.rs             # deterministic fingerprinting and derived issue fields
├── classifier.rs           # Jev client, summary redaction, confidence threshold
├── worker.rs               # in-process job claims and grouping/classification pipeline
├── objects.rs              # write-once object-store adapter (S3 / MinIO)
├── store.rs                # metadata-store trait and shared queries
├── store_memory.rs         # in-memory adapter and seed fixtures (tests/default)
├── store_postgres.rs       # PostgreSQL adapter
├── models.rs               # domain types and enums
├── state.rs                # AppState (store, jobs, self URL)
├── error.rs                # ApiError → HTTP mapping
├── ui.rs                   # serves ui/dist, with an instructional fallback
└── tests.rs                # #[cfg(test)] in-crate integration suite
ui/
├── Cargo.toml              # `scout-ui` workspace member (cdylib)
├── Trunk.toml
├── index.html
├── tailwind.css
└── src/                    # Sycamore components, API client, routing
examples/
└── sample-envelope.txt     # manual SDK-shaped envelope
docker-compose.yml          # app + postgres + minio
Dockerfile                  # multi-stage: Rust backend, Trunk UI, minimal runtime image
```

**Structure Decision**: Single Rust workspace with a server binary and one WASM UI member. This keeps
the "one toolchain, one deployment unit" constraint and avoids the library split the project has
explicitly ruled out. Persistence is introduced behind the existing store boundary; the in-memory
adapter stays for tests so the in-crate suite remains dependency-free.

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No constitution violations. No complexity exceptions are required.

## Phase 0 Research

See [research.md](./research.md). All technical unknowns are resolved there; there are no remaining
`NEEDS CLARIFICATION` items.

## Phase 1 Design

- [data-model.md](./data-model.md) — entities, relationships, validation rules, state transitions.
- [contracts/ingest.md](./contracts/ingest.md) — Sentry envelope/store/log interfaces.
- [contracts/api.md](./contracts/api.md) — dashboard, auth, invitation, and administration interfaces.
- [contracts/classifier.md](./contracts/classifier.md) — Jev-compatible decisions interface and privacy
  boundary.
- [quickstart.md](./quickstart.md) — runnable end-to-end validation scenarios.

### Post-Design Constitution Re-check

**Gate result: PASS.** The design still introduces no conflict because no ratified principles exist.
The single-binary, three-service deployment, least-privilege operator provisioning, project-scoped
authorisation, immutable raw payloads, and non-blocking classifier all remain within the spec's stated
constraints.

## Implementation Sequencing (input for /speckit-tasks)

1. **Persistence foundation**: split `store.rs` behind a trait, add `sqlx` migrations for every entity,
   implement the PostgreSQL adapter, keep the in-memory adapter for tests.
2. **Raw storage**: implement the `object_store` adapter with write-once keys and retention deletion;
   wire ingest and raw retrieval through it.
3. **Durability semantics**: write raw → insert event + job in one transaction → acknowledge; add the
   job table and `FOR UPDATE SKIP LOCKED` claiming; add the configurable global in-flight ingest cap
   with `429` + `Retry-After`; keep the prototype's in-process channel as the test-only default.
4. **Authentication and provisioning**: Argon2id password hashing, opaque session tokens stored
   hashed with expiry, invite tokens redeemed at account creation, single-use recovery codes for
   password reset plus authenticated reissue, the idempotent environment-seeded deployment operator
   with fail-fast missing-value handling, one-time setup tokens for an organisation's initial admin
   (binding an existing account only after authentication and consent, without changing its
   password), and optional OIDC account linking by verified email.
5. **Classifier hardening**: redact sensitive fields before building the ≤2 KB summary; apply the
   per-project confidence threshold (default 0.5); enforce the 800 ms budget and single retry.
6. **API and UI alignment**: operator organisation creation and initial-admin setup screens, settings
   for threshold and retention, invitation screens, recovery-code redemption, member/role management,
   the `ignored` state, decision-based issue filtering, raw-expired state, and log/search views using
   the defined substring-matching rules.
7. **Verification**: extend `src/tests.rs` for every acceptance scenario, including `ignored` state
   transitions, decision-based filtering, and the global concurrency cap (SC-010); add a compose-based
   smoke script for PostgreSQL/object-store wiring; and run the quickstart.
