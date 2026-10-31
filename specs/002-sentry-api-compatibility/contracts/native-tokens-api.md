# Contract: API Token Management (native API + dashboard)

**Feature**: 002-sentry-api-compatibility | **Status**: design contract for implementation

These endpoints live under Scout's native `/api` surface and use the existing `ApiError` envelope
(`{"error": "..."}`) and session authentication (`Authorization: Bearer <session token>` or the
session cookie the UI uses). They do not change any existing native endpoint.

## Token representation

- Plaintext format: `scout_pat_` followed by 43 base64url characters (32 random bytes, unpadded).
- Stored: SHA-256 hex digest plus a 14-character prefix. The plaintext is returned only by the create
  call.
- No scopes, no expiry. Permissions are the owner's live roles at request time.

## `GET /api/me/tokens`

Lists the signed-in user's tokens, newest first. Never returns `token_hash` or the plaintext.

```json
[
  {
    "id": "b1d9f0a2-3c4e-4f5a-8b6c-7d8e9f0a1b2c",
    "label": "release-dashboard",
    "token_prefix": "scout_pat_a1b2",
    "created_at": "2026-11-01T09:00:00Z",
    "last_used_at": "2026-11-01T10:12:03Z",
    "revoked_at": null
  }
]
```

## `POST /api/me/tokens`

```json
{ "label": "release-dashboard" }
```

| Case | Result |
|---|---|
| Valid label (1–100 characters after trim) | `201` with `{ "token": "scout_pat_…", ...metadata }` — the only time the plaintext is ever returned |
| Missing, empty, or over-long label | `400` naming the field |
| Not signed in | `401` |

The response body is the same metadata object as above plus a `token` field. A `token.create` audit
row records the token id and label (never the secret).

## `DELETE /api/me/tokens/{token_id}`

- Own token → `204`, `revoked_at` set; already-revoked tokens stay `204` (idempotent).
- Someone else's token or unknown id → `404` (no existence leak).
- Writes a `token.revoke` audit row with the acting user.

## Organisation admin endpoints

For containment when a teammate's token leaks. Restricted to `OrgRole::Admin`; other members get
`403`. Metadata only.

### `GET /api/orgs/{org_id}/tokens`

Returns the tokens of users who hold a role in any project of the organisation, each entry carrying
`user_id` and `user_email` in addition to the metadata fields above.

### `DELETE /api/orgs/{org_id}/tokens/{token_id}`

Revokes a token belonging to a member of that organisation: `204`, or `404` when the token's owner is
not in the organisation. Writes a `token.revoke` audit row naming both the actor and the owner.

## Dashboard screen contract

Route: `#/tokens`, reachable from the account/settings navigation.

| Element | Behaviour |
|---|---|
| Token list | Label, prefix, created, last used, state. Revoked tokens remain listed and marked. |
| Create form | Single label field; submit creates the token. |
| One-time reveal | The plaintext appears once in a panel with a copy button and a warning that it will not be shown again; dismissing it clears the value from UI state. |
| Revoke action | Confirmation prompt; on success the row switches to revoked without a reload. |
| Empty state | Explains that no tokens exist and that a Sentry client needs one to call `/api/0/`. |
| Errors | Native `{"error": ...}` message rendered inline; no plaintext or hash is ever displayed. |

Accessibility: the create form and revoke controls are keyboard-operable, the one-time reveal is
announced, and the copy button reports success through visible text (not colour alone).

## Compatibility expectations

- SC-008: existing `/api` routes, payloads, and screens are untouched; this screen is the only UI
  addition.
- SC-009: an administrator can create, copy once, and revoke a token from this screen in under a
  minute without hand-calling the API.
