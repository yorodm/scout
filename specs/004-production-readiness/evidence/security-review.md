# Security review — 2026-11-02

- Commit: `812bcf1`
- Host: Linux 6.18.52 x86_64 (development workspace)
- Reviewer: implementation pass against `.specify/memory/constitution.md` v1.1.0
- Result: PASS for the shipped in-memory implementation; the durable adapter remains pending (T013–T041)

## Scope

This review covers the code paths reachable in this repository: the Actix handlers, the ingest path,
the Sentry-compatible layer, the OIDC flow, the in-memory metadata adapter, and the browser client.
The PostgreSQL adapter (`src/store/postgres.rs`) and the startup migration engine wiring (T020, T022–T026)
are not yet implemented, so every at-rest claim below is made against the shipped schema and the
adapter that exists. The review must be repeated for the durable path before release.

## Constitution controls

| Control (constitution) | Where enforced | Evidence |
|---|---|---|
| Passwords stored only as Argon2id hashes | `src/auth.rs::hash_password` (Argon2 default parameters); `User::password_hash` is `skip_serializing` | `operator_changes_its_own_password_and_other_sessions_end` asserts the stored value starts with `$argon2`; `no_response_exposes_credential_material` scans responses |
| Session, invitation, recovery, API-token, and ingest-key secrets stored as hashes | All hashing flows through `src/auth.rs::hash_token` / `hash_password`; only hashes are inserted into the store | `recovery_codes_reset_the_password_and_reissue_revokes_old_codes`, `key_rotation_revokes_the_previous_key`, and `no_response_exposes_credential_material` |
| Plaintext credentials shown once | Invitation/setup tokens, recovery codes, ingest keys, and API tokens are returned only from the creating call; the browser keeps them in memory only | `operator_receives_recovery_codes_once_on_first_sign_in`, the one-time key modal, and `no_response_exposes_credential_material` |
| No credential material in any response | `User`, `IngestKey`, `RecoveryCode`, `OidcConnection`, and `OidcLoginFlow` skip secrets when serialized; OIDC flow `state`/`nonce`/`verifier` are `skip_serializing` | `no_response_exposes_credential_material`; `oidc_configuration_is_admin_only_and_fails_fast` asserts the client secret is absent from the response and redacted from `Debug` |
| Model summaries redacted and capped; raw payloads never sent to the model | `src/classifier.rs` builds a 2 KB summary from issue metadata; raw bytes are never passed | `suggestion_is_attached_from_the_model`, existing 001/002 suite |
| Operator seeded only when absent; startup never resets an existing password | `src/auth.rs::bootstrap_operator` inserts only when no operator row exists | `auth::tests::operator_bootstrap_is_idempotent_and_never_resets_the_password` |
| Every administrative action audited | `src/audit.rs` records organisation creation, setup-token issue, invitation accept/revoke, project creation, key create/rotate/revoke, member changes, settings, OIDC configure/remove, unlock, and password changes | `operator_provisions_an_org_and_its_initial_admin`, `oidc_configuration_is_admin_only_and_fails_fast`, `the_operator_lists_and_unlocks_a_locked_account` |
| API tokens inherit live roles and cannot exceed their owner | `src/compat/tokens.rs` re-derives `scopes_for(role, org_role)` on every request; no scope list is stored on the token | 002 compatibility suite |
| Project-scoped, server-side authorisation with no existence leak | `auth_project`, `auth_issue`-equivalent checks, `deny_project_access`, and the compatibility `authorize_*` helpers resolve one project and one role per request; denials are `404`/`403` | `viewer_cannot_reach_a_project_they_are_not_in`, `viewer_is_denied_settings_resolve_and_other_projects`, `the_operator_is_denied_every_project_scoped_route` |
| Operator authority is deployment-level only | `deny_project_access` rejects the operator on every project route even with a membership row; compatibility authorisation does the same | `the_operator_is_denied_every_project_scoped_route` |
| Demo confinement | `Store::is_demo_user` gates project reads, memberships, and compatibility authorisation; `Store::reset_demo` removes only demo-organisation rows | `demo_accounts_and_projects_are_confined_and_a_reset_preserves_real_data` |
| Storage failure returns retryable `503`, nothing acknowledged | Store/object failures map to `503` with `Retry-After`; the ingest path acknowledges nothing on failure | `unavailable_storage_returns_503_and_acknowledges_nothing`, `unreachable_object_store_returns_503_and_acknowledges_nothing` |
| Saturation returns `429` with `Retry-After`, oversized input `413` | `src/ingest.rs` in-flight gate and payload limits | `saturated_ingest_cap_returns_429_with_retry_after`, `oversized_envelope_is_rejected_before_anything_is_stored` |
| Five consecutive failures lock an account with a generic response | `src/api.rs::login` + `Store::record_failed_sign_in`; every locked attempt returns the same `401` body | `five_consecutive_failures_lock_the_account_for_every_sign_in` |
| Twelve-character minimum with no composition rules | `src/auth.rs::validate_password`, called on initial-admin setup, invitation acceptance, recovery, operator change, and password change | `short_passwords_are_rejected_and_leave_the_recovery_code_usable`, `operator_changes_its_own_password_and_other_sessions_end` |
| Sessions expire on absolute and idle limits; API tokens unaffected | `user_for_token` enforces both; compatibility tokens use a separate lookup | `an_idle_session_expires_and_a_recent_request_keeps_it_alive`, `a_session_expires_at_the_absolute_limit_despite_activity` |
| OIDC verified-email only; no account creation; no role escalation | `src/api.rs::oidc_callback` and `src/oidc.rs` | `oidc_verified_email_signs_in_and_uninvited_identities_are_denied` |
| OIDC flow single-use, 10-minute TTL, fails closed | `Store::take_oidc_flow` deletes before exchange; removed connections fail closed | `oidc_flows_are_single_use_and_fail_closed` |

## Secrets and logging

- OIDC errors are reduced to a short summary before logging; the client secret, authorization code,
  PKCE verifier, nonce, and ID token never enter a log line. `OidcConnection` and `OidcLoginFlow`
  implement `Debug` with redaction and skip secrets from serialization.
- The classifier key and OIDC secret are the only replayable values; both are excluded from
  serialization. Their at-rest exposure is recorded in the plan's Complexity Tracking.
- The bootstrap password is read from the environment, hashed once, and never logged. Startup logs
  name the operator email only.

## Open risks and limitations

1. **Durable adapter not yet shipped.** The `Store` seam (`T018`–`T039`) is pending; until it lands,
   the shipped build is in-memory. The schema in `migrations/0001`–`0008` is additive and
   credential-bearing columns are hash columns or skipped-by-serialization secrets, but the SQL
   adapter and its tests must be re-reviewed.
2. **No per-client-IP protection.** Lockout is per account; rate limiting in front of sign-in is the
   operator's reverse-proxy responsibility, stated in FR-032.
3. **Sessions are not ended by a lock.** This is deliberate (FR-032) and covered by the lockout test.
4. **Password change revokes other sessions only.** The session making the change stays valid, which
   is the documented behaviour.
5. **Demo mode is opt-in and unauthenticated by design.** Both demo routes are registered only when
   `SCOUT_DEMO_MODE=true`; enabling them on a deployment that also holds real data is documented as a
   demo-only configuration.
