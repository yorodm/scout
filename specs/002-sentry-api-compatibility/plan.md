# Implementation Plan: Sentry Read-API Compatibility

**Branch**: `spec/sentry_compatibility` | **Date**: 2026-11-01 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/002-sentry-api-compatibility/spec.md`

## Summary

Add an opt-in `/api/0/` surface that serves the 11 paths in the referenced Sentry OpenAPI document
from Scout's existing organisation, project, issue, role, and tag data, plus long-lived bearer API
tokens that inherit their owner's live roles. The compatibility layer is a new `src/compat/` module
that projects stored issues into Sentry's issue shape, parses Sentry's search grammar and sort
parameters, paginates with query-bound opaque cursors (`Link` + `X-Hits`), and answers every error
with `{"detail": ...}`. Endpoints for Sentry subsystems Scout does not own are answered honestly:
empty collections where that is truthful, `501` otherwise. The only UI addition is an API-token
screen. Nothing in the existing `/api`, ingest, or UI surface changes.

## Technical Context

**Language/Version**: Rust 2021 edition, stable toolchain (`rustc` currently in the workspace)

**Primary Dependencies**: Actix Web 4, `serde`/`serde_json`, `chrono`, `uuid`, `sha2`, `base64`
(all already direct dependencies); Sycamore + Trunk for the UI. **No new crates.**

**Storage**: In-process `Store` behind `Arc<RwLock<_>>` for this slice, mirroring the PostgreSQL
schema. Migration `0007_sentry_compat.sql` adds the same columns/tables for the PostgreSQL adapter
so the two representations cannot drift. Raw payloads are untouched.

**Testing**: `cargo test` with in-crate `#[actix_web::test]` integration tests in `src/tests.rs`
and `#[cfg(test)]` modules beside the new code (constitution principle V).

**Target Platform**: Linux server, single binary; WASM UI served from `ui/dist`.

**Project Type**: Web service + WASM frontend in one cargo workspace, single binary crate.

**Performance Goals**: 95% of issue-list and tag-value requests for a 1,000-issue project ≤500 ms
(SC-007); status changes visible in the native API/UI within 1 s (SC-006).

**Constraints**: Compatibility surface off by default and gated by configuration; no read-API rate
limiting in this slice; responses are JSON with ISO-8601 UTC timestamps and string ids; all errors
use the `detail` envelope; `/api/0/` must never return an HTML error page.

**Scale/Scope**: 11 documented paths, ~1,000 issues per project, 1,000 tag values per key, a small
number of tokens per user. `X-Hits` totals are computed by scanning the project's issue map, which
is O(project issues) and bounded by the scale above.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Requirement | Status | How this plan complies |
|---|---|---|
| I. Ingest never depends on classification (NON-NEGOTIABLE) | PASS | The compatibility layer is read-only over stored data. It adds no work to ingest and cannot block or delay acknowledgement. |
| II. Raw payloads immutable and authoritative | PASS | No handler writes payloads or objects. `DELETE .../issues/` returns `501` (FR-015) precisely to preserve history and raw data. |
| III. Project-scoped, server-side authorisation | PASS | Every `/api/0/` request resolves exactly one project server-side, derives Sentry scopes from the token owner's live role, returns `403` for insufficient scope and `404` for inaccessible resources without leaking existence. |
| IV. One deployment unit, minimal moving parts | PASS | New module inside the existing binary; no broker, search cluster, or IdP. No new dependency. UI stays Sycamore/Trunk. |
| V. Evidence over assumption: test the real paths | PASS | Contract and behaviour tests drive the real Actix app with `#[actix_web::test]`; no `tests/` crate, no `lib.rs` split. PostgreSQL parity is covered by the migration plus the existing compose smoke path. |
| Security and Privacy constraints | PASS | Tokens are SHA-256 hashed, plaintext shown once, prefix stored for identification; no scope list stored; revocation is immediate; no token material is logged. |
| Performance and Reliability constraints | PASS | Read-only handlers avoid ingest paths; the 10 MB JSON limit and existing error mapping are reused. No `503`/`429` behaviour changes. |
| Compatibility and Scope constraints (v1.1.0) | PASS | Surface is limited to the 11-path contract, opt-in and default-off, derived from existing data with no parallel model, and answers unsupported capability honestly. Metrics, tracing, SCIM, Teams, and other endpoints stay unimplemented. |
| Development Workflow & Quality Gates | PASS | Work is split into slices that each pass `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, plus UI clippy and `trunk build --release` for the token screen. |
| Complexity Tracking | PASS | No principle violations. The one added concept (`Issue.number`) exists to make `shortId` stable and cursors keyset-based; see Complexity Tracking. |

**Gate result: PASS.** No unjustified violations; planning may proceed.

## Project Structure

### Documentation (this feature)

```text
specs/002-sentry-api-compatibility/
├── plan.md              # This file
├── research.md          # Phase 0 decisions
├── data-model.md        # Phase 1 entities, invariants, migration
├── quickstart.md        # Phase 1 runnable validation guide
├── contracts/
│   ├── sentry-compat-api.md   # The 11 documented /api/0/ paths
│   └── native-tokens-api.md   # Token management + dashboard screen contract
└── tasks.md             # Phase 2 output (/speckit-tasks — not created here)
```

### Source Code (repository root)

```text
src/
├── compat/                  # NEW: all Sentry-compatibility code
│   ├── mod.rs               # `/api/0` scope, disabled-gate, route table
│   ├── auth.rs              # Bearer token → owner → project → scope set
│   ├── error.rs             # CompatError → {"detail": ...}
│   ├── cursor.rs            # Opaque, query-bound keyset cursor
│   ├── projection.rs        # Issue → Sentry issue object
│   ├── query.rs             # Sentry search grammar, sort mapping
│   ├── handlers.rs          # The 11 documented paths
│   └── tokens.rs            # Native token endpoints (create/list/revoke)
├── models.rs (modify)       # + ApiToken, Organisation.slug, Issue.number
├── store.rs (modify)        # + token index, slug allocation, issue numbering
├── config.rs (modify)       # + sentry_compat_enabled (default false)
├── state.rs (modify)        # + compat_enabled flag for handlers/middleware
├── main.rs (modify)         # module wiring, AppState fields
├── api.rs (modify)          # registers native token endpoints only
└── tests.rs (modify)        # in-crate integration coverage for the new surface

migrations/
└── 0007_sentry_compat.sql   # api_tokens table, organisations.slug, issues.number

ui/src/
├── components/tokens.rs     # NEW: API-token screen
├── components/app.rs        # navigation entry
├── components/mod.rs        # module export
└── state.rs                 # token list/create/revoke calls + state
```

**Structure Decision**: Single cargo binary crate, matching the existing layout. The compatibility
surface is isolated in `src/compat/` so `/api/0/` semantics (error envelope, pagination, scopes) cannot
leak into the native `/api` handlers, which keep `ApiError` and `{"error": ...}` unchanged.

## Post-Design Constitution Re-check

Re-evaluated after `research.md`, `data-model.md`, `contracts/`, and `quickstart.md` were written:

- **I**: No design element touches ingest. **PASS**
- **II**: `Issue.number` is derived metadata, not payload data; `DELETE issues` remains `501` so
  history and raw payloads cannot be removed through this surface. **PASS**
- **III**: Every contract entry states the scope check and the `403`/`404` split; cursors carry no
  authority. **PASS**
- **IV**: All code lands in the existing binary and one WASM UI; no dependency or service added.
  **PASS**
- **V**: R14 maps every acceptance scenario to an in-crate `#[actix_web::test]` through the real app.
  **PASS**
- **Security**: Tokens hashed, shown once, prefix-only in listings, audit rows on create/revoke,
  no secret in logs. **PASS**
- **Compatibility and Scope**: The contract stays within the 11 documented paths; unsupported
  subsystems are `[]`/`404`/`501`, never fabricated. **PASS**
- **Migrations**: `0007_sentry_compat.sql` is idempotent and reversible (drop the added index, column,
  and table). **PASS**

**Post-design result: PASS.** No new violations, no Complexity Tracking entries beyond the one
recorded `Issue.number` decision.

## Complexity Tracking

No constitution violations. One design addition is recorded here because it changes an existing
entity:

| Addition | Why Needed | Simpler Alternative Rejected Because |
|---|---|---|
| `Issue.number` (per-project monotonic sequence) | `shortId` must be stable across requests and cursors need a deterministic keyset tiebreak | Deriving `shortId` from the UUID is unstable to display and does not give Sentry-like readable ids; offset pagination shifts when new issues arrive, breaking SC-005's exactly-once guarantee |
