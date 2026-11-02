# PostgreSQL identity adapter — 2026-11-02

- Commit: see `git log` for `store(postgres): durable identity adapter`
- Host: Linux 6.18.52 x86_64 development workspace (no Docker)
- PostgreSQL: 18.6, local cluster at `127.0.0.1:55432`, database `scout_test`
- Command:
  `SCOUT_TEST_DATABASE_URL=postgres://scout@127.0.0.1:55432/scout_test cargo test --bin scout -- --ignored --test-threads=1`
- Result: PASS — 4 passed, 0 failed

## What was proven

The ignored suite now includes the durable identity surface:

- `store::postgres::tests::postgres_identity_surface_round_trips`
  - users round-trip with an Argon2id hash and case-insensitive email
  - the single-operator partial index rejects a second operator
  - five consecutive failures lock the account; the operator unlock clears the
    lock and the counter
  - sessions honour the 12-hour idle limit and the absolute expiry, and
    `sweep_sessions` removes expired rows
  - recovery codes are issued revoke-then-issue in one transaction; reissuing
    invalidates the prior unused set and a used code cannot be reused
  - organisation slug allocation matches the in-memory rule (`acme-identity`,
    `acme-identity-2`); membership is upserted
  - invitations are stored by hash, report pending, and become non-pending after
    acceptance with the accepting account recorded
- `store::postgres::tests::postgres_health_is_bounded_and_reports_durable`
  - `SELECT 1` health check reports reachable, `kind()` is `postgres`, and the
    adapter is durable
- The two pre-existing ignored checks (`sc_007` latency and the blackholed
  object-store probe) still pass.

## Scope note

This validates the identity half of the storage seam (T022, T023). Project,
telemetry, operations SQL (T024–T026), the `Store` enum seam (T018/T019), and
handler conversion (T027–T032) remain; the full clean-host evidence run (T041,
T085, T088) still requires the compose stack.
