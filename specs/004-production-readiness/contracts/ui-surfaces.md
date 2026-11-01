# Contract: Dashboard Surfaces

The dashboard is the existing Sycamore/Trunk application in `ui/`. This contract lists the screens this
feature must complete, the calls each one uses, and the states each must show. Views not listed keep
their current behaviour. Every screen must render loading, empty, and actionable error states from
`ui/src/components/common.rs` (FR-020).

## Access model in the UI

| Caller | Sees |
|---|---|
| Unauthenticated | Real sign-in, recovery, invitation acceptance. Demo shortcuts only when demo mode is enabled **and** `GET /api/demo/accounts` succeeds |
| Deployment operator | Operator console: organisations list, organisation creation with the one-time setup token |
| Organisation admin | Members/invitations for their organisation, OIDC configuration, project creation |
| Project owner | Keys, settings (model, threshold, retention), soft delete |
| Project admin/triager/viewer | Issues, logs, unified search; triage controls for triager and above |

A demo session shows a persistent banner ("Demo data — changes are discarded") and only demo
organisation data.

## Screens

### 1. Sign-in (`components/overlays.rs`)

- Real email/password form → `POST /api/login`; stores the Bearer session as today.
- Links to recovery and to invitation acceptance via the token in the URL.
- Demo shortcuts button visible only when the demo-accounts call succeeds; hidden when demo mode is off.
- Errors: `401` invalid credentials, `429` saturation with `Retry-After`, store-unavailable `503`
  rendered as a retryable message.

### 2. Recovery (`components/overlays.rs`)

- `POST /api/recover` with email, recovery code, and new password, then sign in with the new password.
- One failure message for every rejection (no account enumeration).

### 3. Invitation acceptance (`components/overlays.rs`)

- `GET /api/invitations/{token}` renders organisation/project, role, and expiry.
- New account: name + password → `POST /api/invitations/{token}/accept`, then the returned recovery
  codes are shown once with an explicit "I have saved these" confirmation.
- Existing account: prompts sign-in first, then accepts with the session and no password field.
- Expired/used/revoked: explicit state, no form.

### 4. Operator console (`components/other.rs`, entry in `app.rs`)

- `GET /api/orgs` list with name, slug, `is_demo` badge.
- Create organisation form (`name`, `initial_admin_email`) → `POST /api/orgs`, then the
  `initial_admin_setup_token` is shown once in a copyable block with a warning that it is not
  retrievable.
- Never shows issues, events, logs, keys, or other tenant data.

### 5. Members (`components/other.rs`)

- `GET /api/projects/{id}/members`; role changes via `PUT`, removal via `DELETE`, additions via
  `POST /api/projects/{id}/members`.
- Organisation invitation: `POST /api/orgs/{id}/invitations`, which returns the invite link once;
  revoke with `DELETE /api/orgs/{id}/invitations/{invitation_id}`.
- Last-owner protection message when the server refuses (`409`).

### 6. Keys (`components/other.rs`)

- `GET/POST /api/projects/{id}/keys`, `DELETE …/keys/{key_id}`, `POST …/keys/rotate`.
- New and rotated keys are shown once with a copy action; afterwards only the prefix is listed.
- Revoking a key updates the list without a reload.

### 7. Settings (`components/other.rs`)

- `GET/PUT /api/projects/{id}/settings` for model endpoint, model name, API key, confidence threshold,
  and retention period; validation errors are rendered inline.
- OIDC section: `GET/PUT/DELETE /api/orgs/{org_id}/oidc` with issuer, client id, write-only client
  secret, and scopes; removal states that local sign-in is unaffected.

### 8. Issues list (`components/issues.rs`)

- `GET /api/projects/{id}/issues` with `state`, `level`, `category`, `q`, and `sort` controls.
- Multi-select filters, active-filter summary, "no issues match" empty state, retryable error state.

### 9. Issue detail (`components/issues.rs`)

- Header: title, culprit, level, state, count, first/last seen, release, environment.
- Occurrence timeline from `GET /api/issues/{id}/occurrences`, newest first, with event ids and sizes.
- Stack frames (in-app highlighted) and breadcrumbs.
- Raw payload from `GET /api/issues/{id}/raw`, rendered read-only; when it expired, an explicit
  "Raw payload expired" panel replaces the content and no stale body is shown.
- Suggestion card (category, confidence, source, time) beside the authoritative decision; confirm and
  override actions call `POST /api/issues/{id}/categorisation` and refresh without moving the issue.
- Resolve / reopen / ignore actions call `POST /api/issues/{id}/state`.

### 10. Logs and unified search (`components/other.rs`)

- Logs: `GET /api/projects/{id}/logs` with level and text filters.
- Search: `GET /api/projects/{id}/search` renders issues and logs in one ordered result set, with the
  documented filter and result cap.
- Empty state distinguishes "no data yet" from "no matches for this filter".

### 11. API tokens (`components/tokens.rs`)

Unchanged; must remain reachable from the same navigation entry.

## Cross-cutting requirements

- Navigation is role-aware: operator console only for the operator, organisation admin entry only for
  org admins, project settings only for owners/admins.
- Any `401` clears the session and returns to sign-in; any `403` shows a permission message instead of
  an empty list.
- No screen renders a credential hash, OIDC client secret, or model API key.
- The UI production build (`trunk build --release`) and `cargo check --target wasm32-unknown-unknown`
  must pass with no new dependency.
