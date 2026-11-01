# Quickstart: Sentry Read-API Compatibility

Runnable validation for feature 002. These scenarios prove the feature end-to-end on a local
deployment; implementation details live in `tasks.md`, shapes live in
[`contracts/sentry-compat-api.md`](./contracts/sentry-compat-api.md) and
[`contracts/native-tokens-api.md`](./contracts/native-tokens-api.md).

## Prerequisites

- Rust stable toolchain and `trunk` (for the UI slice).
- The repository root as working directory.
- Port 8080 free.
- Docker (only for the migration parity check in §9).
- Demo data is convenient for these checks (it seeds organisations, projects, and issues).

## 1. Start with the compatibility surface enabled

```sh
export SCOUT_DEMO_MODE=true
export SCOUT_OPERATOR_EMAIL=operator@scout.local
export SCOUT_OPERATOR_PASSWORD=change-me-now
export SCOUT_SENTRY_API_COMPAT_ENABLED=true
cargo run
```

Expected: startup logs a warning for demo mode and the server listens on `http://127.0.0.1:8080`.

Verify the default-off behaviour in a second shell (restart without the flag or use a different
port):

```sh
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/api/0/projects/acme/web/issues/
```

With `SCOUT_SENTRY_API_COMPAT_ENABLED` unset the status is `404` and the body is
`{"detail":"Not Found"}`.

## 2. Create a token

Sign in as a seeded user, then create a token; the plaintext is returned once.

```sh
curl -s -X POST http://127.0.0.1:8080/api/login \
  -H 'content-type: application/json' \
  -d '{"email":"operator@scout.local","password":"change-me-now"}'
```

Take the returned session token, then:

```sh
curl -s -X POST http://127.0.0.1:8080/api/me/tokens \
  -H "authorization: Bearer <SESSION_TOKEN>" \
  -H 'content-type: application/json' \
  -d '{"label":"quickstart"}'
```

Expected: `201` with `token` (starts `scout_pat_`), `id`, `label`, `token_prefix`, `created_at`.
Re-running `GET /api/me/tokens` shows the same token by prefix without the plaintext.

## 3. Issue read and status triage (User Story 1)

```sh
curl -s http://127.0.0.1:8080/api/0/projects/<org>/<project>/issues/ \
  -H "authorization: Bearer <TOKEN>" -D - | head -40
```

Expected: a JSON array of Sentry issue objects (fields per the contract), a `Link` header with
`rel="next"`, `results="true"`, `cursor="…"`, and an `X-Hits` total. With no `query` parameter the
list defaults to unresolved issues, matching Sentry.

- Inspect any element: `shortId` reads `<PROJECT>-<n>`, `statusDetails` is `{}`, `userCount` is `0`,
  `annotations` is `[]`, timestamps end in `Z`, and `permalink` opens the Scout issue detail.
- Filter: `?query=is:resolved&sort=freq&limit=25` returns only resolved issues, highest count first,
  at most 25.
- Follow the `cursor` from the `Link` header until `results="false"`; count distinct `id` values and
  confirm none repeats.
- Resolve two issues:

  ```sh
  curl -s -X PUT 'http://127.0.0.1:8080/api/0/projects/<org>/<project>/issues/?id=<ID_A>&id=<ID_B>' \
    -H "authorization: Bearer <TOKEN>" -H 'content-type: application/json' \
    -d '{"status":"resolved"}'
  ```

  Then confirm in the UI (`#/issues/<id>`) that both are resolved and that the state history names
  the token owner.

## 4. Tag values (User Story 2)

```sh
curl -s http://127.0.0.1:8080/api/0/projects/<org>/<project>/tags/service/values/ \
  -H "authorization: Bearer <TOKEN>"
```

Expected: the distinct values that project's issues carry, ascending — for the seeded demo project
`service` yields `[{"name":"checkout-web"}]` and `region` yields `[{"name":"eu-west-1"}]`. With
`?query=check` only matching values are returned. The issue-scoped path
`/api/0/organizations/<org>/issues/<issue>/tags/service/values/` returns the values for that issue
only, and a key with no values returns `[]` (for example `release` on the demo project).

## 5. Scopes and denial paths (User Story 3)

- A viewer's token lists issues successfully and receives `403` on `PUT .../issues/`.
- A token whose owner is removed from the project receives `404` for that project on the next call
  (no reissue needed).
- `Authorization: Bearer not-a-token` returns `401` with the same body as a revoked or unknown token.
- Revoke from the dashboard (`#/tokens`) or `DELETE /api/me/tokens/<id>`; the next `/api/0/` call with
  that token returns `401`.
- A token used against `/api/0/` while the surface is disabled returns `404`, not `401`.

## 6. Honest non-support (User Story 4)

```sh
curl -s -o /dev/null -w '%{http_code}\n' -X DELETE \
  http://127.0.0.1:8080/api/0/projects/<org>/<project>/issues/ \
  -H "authorization: Bearer <TOKEN>"
```

Expected: `501` with a `detail` citing preserved history and raw payloads; the issue list is
unchanged. Similarly:

- release commit files → `[]` for a known version, `404` for an unknown one;
- service hooks, feedback, app installations → `[]`; detail hook → `404`;
- service-hook creation, feedback submission, spike-protection change, external-issue creation →
  `501` naming the feature;
- every error body matches `{"detail": "…"}` and no response is HTML.

## 7. Dashboard token screen (SC-009)

Open `#/tokens` while signed in: create a token, copy it from the one-time panel, dismiss the panel,
reload the page and confirm the token is listed by prefix only, then revoke it. The whole flow should
take under a minute and never reveal the plaintext again.

## 8. Automated checks

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo check --target wasm32-unknown-unknown --manifest-path ui/Cargo.toml
trunk build --release --manifest-path ui/Trunk.toml
# SC-007 benchmark (ignored by default; record the p95 here after running it)
cargo test --release -- --ignored sc_007
```

- Measured p95 (1,000-issue list / tag values): 10.8 ms on 2026-10-31 (release build, in-crate benchmark).

Expected: all pass. The test suite covers projection, authentication and scope denial, list
filtering/sorting, exactly-once pagination over 1,000 issues, mutations and history, tag values,
unsupported responses, the disabled toggle, and token management (see R14 in
[`research.md`](./research.md)).

## 9. Migration parity

On a host with Docker available, apply and re-apply the PostgreSQL migration added by this feature:

```sh
docker compose up -d postgres
# apply migrations/0007_sentry_compat.sql, then run it a second time
# verify: organisations.slug is unique case-insensitively; issues.number is unique per project
# rollback: drop api_tokens, the issue number index/column, and the slug index/column
```

Expected: both applications succeed, the second is a no-op, and the rollback leaves the original
schema. This is the constitution V gate for migrations.

## 10. Regression check (SC-008)

In the same run, exercise the native surface and confirm nothing changed:

- `POST /api/{project}/envelope/` with a seeded ingest key still accepts an event.
- `GET /api/projects/{project_id}/issues` and the existing UI screens behave as before.
- The new token screen is the only UI addition; login, issues, logs, search, members, and settings
  are unchanged.

## Live validation record (2026-10-31)

Run against `cargo run` with `SCOUT_DEMO_MODE=true` and `SCOUT_SENTRY_API_COMPAT_ENABLED=true`,
using the demo organisation `acme` and project `scout-web`:

| Step | Result |
|---|---|
| `POST /api/me/tokens` | `201` in 2.1 ms; prefix `scout_pat_K0Y0`, plaintext returned once |
| `GET /api/0/.../issues/` | `200` in 6.5 ms; `SCOUTWEB-1`, `unresolved`, `userCount` 0, `statusDetails` `{}`; `Link` with `results="false"`, `X-Hits: 1` |
| `PUT .../issues/?id=<id>` | `200` in 6.9 ms; `status` became `resolved` and the native API/UI reflect it immediately (SC-006) |
| `GET .../tags/service/values/` | `200` with the project's distinct values |
| `DELETE .../issues/` | `501` with the preserved-history `detail` |
| `DELETE /api/me/tokens/<id>` | `204`; the next `/api/0/` call returned `401` `{"detail":"Invalid token"}` |

The create → copy → revoke sequence completed over HTTP in well under a second. SC-009's one-minute
human timing in the browser was not measured; the `#/tokens` screen implements the same flow with a
one-time copy panel and a confirmation prompt.
