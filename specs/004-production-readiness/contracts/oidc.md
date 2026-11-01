# Contract: Organisation OIDC Sign-In

Optional per-organisation OpenID Connect. Local email-and-password sign-in is always available and is
never replaced, disabled, or redirected away from (FR-018, US3).

## Configuration (organisation admin)

| Route | Request | Success response |
|---|---|---|
| `GET /api/orgs/{org_id}/oidc` | — | `200` `{configured, issuer, client_id, scopes, configured_at}`; `404` when the organisation has no connection |
| `PUT /api/orgs/{org_id}/oidc` | `{issuer, client_id, client_secret, scopes?}` | `200` with the same shape as `GET` (secret omitted) |
| `DELETE /api/orgs/{org_id}/oidc` | — | `200 {"ok": true}`; `404` when nothing is configured |

Rules:

- `issuer` must be an absolute `http(s)` URL. At `PUT`, discovery
  (`/.well-known/openid-configuration`) is fetched with a 5 s timeout; failure returns
  `400 {"error":"provider discovery failed: <summary>"}` and changes nothing.
- `scopes` defaults to `openid email profile` and must include `openid` and `email`.
- `client_secret` is write-only: it is stored, replayed to the provider, and never returned or logged.
- Only organisation admins (or the deployment operator for that organisation) may read or change it.
  Every change is audited with actor, organisation, and action.

## Sign-in flow

```text
browser ──GET /api/oidc/{org_slug}/start──► 302 provider /authorize
                                             (?response_type=code, client_id, redirect_uri,
                                              scope, state, nonce, code_challenge=S256)
provider ──GET /api/oidc/callback?code&state──► Scout
                                              validate state → token exchange with PKCE verifier
                                              → verify ID token (iss, aud, exp, nonce)
                                              → match verified email to a membership
                                              → issue a normal session
                                              → 302 to SCOUT_SELF_URL/#oidc=<outcome>&token=<session>
```

| Route | Notes |
|---|---|
| `GET /api/oidc/{org_slug}/start` | `404` unknown slug, `404` no connection configured, `503` provider unreachable, `302` otherwise |
| `GET /api/oidc/callback` | Always redirects to `SCOUT_SELF_URL/#oidc=<outcome>&token=<session>`; `token` is present only on success |

Outcome codes (fixed set, no detail about which check failed or whether an email exists):

| Code | Meaning |
|---|---|
| `signed_in` | Verified email matched a membership in the organisation; session issued |
| `state_invalid` | Unknown, expired, or already consumed `state` |
| `email_unverified` | Identity's email is absent or not verified |
| `membership_unknown` | Verified email has no membership or invitation in this organisation |
| `provider_error` | Discovery, token exchange, or ID-token validation failed |

The fragment is used deliberately: it is not sent to any server and the UI removes it after storing the
session, mirroring the existing Bearer-session model.

## Matching rules

1. Only the ID token's `email` claim, and only when `email_verified` is `true`, is used for matching.
   `sub` is recorded for diagnostics but never used to link accounts.
2. The match is an existing `project_members`/`org_members` row scoped to **that** organisation. An
   invitation in `pending` state also matches, and is consumed exactly like a local acceptance.
3. No user is created, no password is set, no role is granted beyond what the membership or invitation
   already carries (US3/AC2).
4. The sign-in produces the same session token as local sign-in, so all downstream authorisation is
   unchanged.

## Failure and concurrency rules

- A flow row is single use: the callback deletes it before exchanging the code, so a replayed callback
  fails as `state_invalid`.
- Flow rows expire after 10 minutes and are swept with sessions.
- A connection removed while a flow is in progress fails that flow with `provider_error`; it never
  falls back to local credentials.
- An unresolvable issuer, missing metadata, or an unsupported algorithm fails closed; there is no
  "accept anyway" path.
- The client secret, authorization code, PKCE verifier, nonce, and ID token are never logged.
