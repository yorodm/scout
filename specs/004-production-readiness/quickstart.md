# Quickstart: Validating Production Readiness

A validation guide for feature 004. It proves the scenarios in [spec.md](./spec.md) against the shipped
stack; it is not an implementation guide. Results belong in
`specs/004-production-readiness/evidence/` with the date and the host profile.

## Prerequisites

- Docker with Compose v2 on a clean host.
- For local development and tests: a Rust stable toolchain, `wasm32-unknown-unknown`, and Trunk.
- Two terminal shells: one for the stack, one for commands.

Reference deployment for performance evidence: 4 vCPU, 8 GB RAM, local SSD.

```sh
cd /path/to/scout
cp .env.example .env 2>/dev/null || true
# Set at least:
#   SCOUT_OPERATOR_EMAIL, SCOUT_OPERATOR_PASSWORD
#   SCOUT_S3_ACCESS_KEY, SCOUT_S3_SECRET_KEY, SCOUT_S3_DEFAULT_BUCKET=scout-raw
export SCOUT_OPERATOR_EMAIL=operator@scout.local
export SCOUT_OPERATOR_PASSWORD="$(openssl rand -base64 24)"
export SCOUT_S3_ACCESS_KEY="$(openssl rand -hex 16)"
export SCOUT_S3_SECRET_KEY="$(openssl rand -base64 32)"
export SCOUT_DEMO_MODE=false
```

Save the three generated values — the operator password and the object-store credentials are needed for
the rest of this guide.

## 1. Local quality gates (SC-012, SC-013)

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
(cd ui && cargo check --target wasm32-unknown-unknown && trunk build --release)
```

Expected: everything passes; the existing 001/002 suite is unchanged in count and outcome.

With a disposable PostgreSQL database available:

```sh
export SCOUT_TEST_DATABASE_URL=postgres://scout:scout@127.0.0.1:5432/scout_test
cargo test -- --ignored
```

Expected: migration, restart-durability, concurrent-start, and repository parity tests pass. Create the
database fresh before this run; the suite wipes the schema it uses.

## 2. Clean-host stack (SC-009, part 1)

```sh
docker compose up -d --build --force-recreate
docker compose ps
curl -fsS http://127.0.0.1:8080/api/health | jq
```

Expected: `postgres` and `garage` report healthy; `scout` becomes healthy without any manual database
step; health returns `200` with `"store":"postgres"`, `"durability":"durable"`, `"storage":"ok"`, and a
named `object_store`. The Scout log contains one operator-created message and no migration error.

## 3. Operator onboarding (US2/AC1, SC-005)

Start a timer, then:

1. Open `http://127.0.0.1:8080` and sign in with the operator credentials.
2. Create an organisation named `Acme` with initial admin `admin@acme.test`.
3. Copy the one-time initial-admin setup token.

Expected: under 5 minutes to this point, no `psql` or manual SQL. The token is shown once; reopening the
organisation list does not reveal it.

## 4. Initial admin, invitations, roles (US2/AC2–5, SC-006)

1. Open the setup link in a private window, set a password, and record the recovery codes shown once.
2. As the new admin, invite `triager@acme.test` with role `triager` and copy the invite link.
3. Accept it in another private window; confirm the account lands on the project with `triager` access.
4. Change that member's role to `viewer`; the same session loses triage actions within one minute.
5. Sign out and back in with the initial admin's password; recovery codes still work once each.

Expected: every change is immediate, no restart, and the audit trail shows organisation creation,
initial-admin issuance, invitation creation/acceptance, and role change.

## 5. Keys, settings, and demo confinement (US2/AC6–8, SC-014)

1. As project owner, create an ingest key, copy it once, then rotate it; the previous key is rejected on
   the next ingest.
2. Change the model endpoint, threshold, and retention; reload and confirm persistence.
3. With `SCOUT_DEMO_MODE=false`, confirm the dashboard offers only real sign-in.
4. Restart with `SCOUT_DEMO_MODE=true` while the real organisation exists, sign in with a demo shortcut,
   and confirm: only demo data is visible, the real project returns "not found", and
   `POST /api/demo/reset` leaves the real organisation, its project, and the operator intact.

## 6. Triage, search, and settings UI (US4, SC-008)

1. Ingest an event through the DSN and a log line; wait for them to appear.
2. Open the issue: timeline, frames, breadcrumbs, release, raw payload, and the suggestion card (when a
   model endpoint is configured) are visible.
3. Confirm or override the suggestion; the decision becomes authoritative and the suggestion stays
   visible.
4. Search for a term present in both an issue title and a log message; both appear in one result set.
5. Apply each filter (state, level, category, text, sort) and confirm ordering and empty states.
6. Stop Scout (`docker compose stop scout`) and load a view: the UI shows a retryable error, not a blank
   page.

## 7. Retention and restart durability (US1, SC-001, SC-009)

```sh
# Age one project's rows honestly, inside the shipped database.
docker compose exec -T postgres psql -U scout -d scout -c \
  "UPDATE events SET received_at = now() - interval '40 days';"
docker compose exec -T postgres psql -U scout -d scout -c \
  "UPDATE log_lines SET timestamp = now() - interval '40 days';"
```

Restart Scout with `SCOUT_RETENTION_INTERVAL_SECONDS=2`, wait two intervals, then:

1. Reopen the issue: the raw payload reports the explicit expired state; the issue, counters, and state
   history remain.
2. Confirm the aged log line is gone while newer lines remain.
3. `docker compose restart scout` and re-read the same issue: title, counts, release, decision, and
   history are unchanged.

Expected: every record created before the restart is present afterwards, and nothing is acknowledged
that was not stored.

## 8. Schema behaviour (SC-002, SC-003, SC-011)

```sh
# Idempotent restart: no migration is applied twice, startup succeeds.
docker compose restart scout && docker compose logs --tail=20 scout

# Concurrent starts against one store. `run` does not publish host ports, so
# three instances can share the same service definition.
for n in 1 2 3; do
  docker compose run -d --no-deps --name "scout-start-$n" scout
done
sleep 20
docker logs scout-start-1 2>&1 | tail -5
docker logs scout-start-2 2>&1 | tail -5
docker logs scout-start-3 2>&1 | tail -5
docker compose exec -T postgres psql -U scout -d scout \
  -c "SELECT count(*) AS applied FROM _sqlx_migrations;"
docker compose rm -sf scout-start-1 scout-start-2 scout-start-3
```

Expected: exactly one instance applies pending changes, all three log a successful start with no
migration error, no duplicate or partial schema change exists, and `_sqlx_migrations` lists one row per
migration.

Version skew: run the previous image against the upgraded database (`docker compose run --rm scout` with
the older tag). Expected: startup aborts with a message naming the stored version and the build's
maximum, and no HTTP listener is opened.

## 9. Optional OIDC (US3, SC-007)

Use any OIDC test provider (for example a local `oauth2-proxy` or Keycloak) with a user whose verified
email matches an invited member:

1. As organisation admin, save issuer, client id, and client secret; the connection is listed.
2. Start provider sign-in; expect a redirect back to the dashboard signed in as that member's role.
3. Try an identity with an unverified email, and one with a verified email that has no membership.
   Both are denied with no account created.
4. Sign in locally with the same member's password; it still works.
5. Remove the connection; provider sign-in stops and local sign-in is unaffected.

## 10. Load check (SC-010)

```sh
./scripts/load-ingest.sh --rate 500 --duration 60 --size 10240
```

Expected: the script prints accepted events/s, p95 acknowledgement latency, and 95% time-to-visibility,
and writes a dated result file under `specs/004-production-readiness/evidence/`. Targets: ≥500 accepted
events/s, p95 ≤150 ms, 95% visible within 10 s. A shortfall must be recorded with the host profile and an
explicit acceptance note.

## 11. Sentry compatibility surface (SC-011)

```sh
export SCOUT_SENTRY_API_COMPAT_ENABLED=true
docker compose up -d --force-recreate scout
./scripts/compose-smoke.sh --compat-only
```

Expected: every documented path in
[feature 002's contract](../../002-sentry-api-compatibility/contracts/sentry-compat-api.md) returns its
documented response; the compatibility schema additions are present, re-applying them changes nothing,
and organisation slug and per-project issue-number uniqueness hold.

## 12. Failure modes (SC-004, FR-004, FR-005)

| Action | Expected |
|---|---|
| `docker compose stop postgres` | `/api/health` → `503`, `"storage":"unavailable"`; ingest returns `503` with `Retry-After`; nothing is acknowledged |
| `docker compose stop garage` | `/api/health` → `503`, `"object_store":"unavailable"`; ingest returns retryable `503` |
| Start Scout with `SCOUT_STORE=memory` | Startup logs a durability warning and `/api/health` reports `"durability":"ephemeral"` |
| Corrupt a migration (or apply a newer one out of band) | Startup aborts with an actionable message and binds no port |

## Evidence checklist

- [ ] Quality gates pass (section 1)
- [ ] Health reports durable storage and both dependencies (section 2)
- [ ] Onboarding under five minutes with a one-time token (section 3)
- [ ] Roles and invitations effective immediately (section 4)
- [ ] Demo session confined; reset preserves real data (section 5)
- [ ] Issue detail, raw-expired state, search, suggestion/decision, settings (section 6)
- [ ] Retention expires raw data while triage history survives; restart preserves everything (section 7)
- [ ] Migrations idempotent and concurrency-safe; version skew aborts (section 8)
- [ ] OIDC verified-email sign-in, denial, and local fallback (section 9)
- [ ] Load check within targets or recorded shortfall (section 10)
- [ ] Sentry compatibility paths and schema idempotency (section 11)
- [ ] Degraded health and retryable ingest on dependency loss (section 12)
