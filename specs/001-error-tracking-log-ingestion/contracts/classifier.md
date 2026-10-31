# Contract: Categorisation Model (Jev-compatible)

**Feature**: `001-error-tracking-log-ingestion` | **Plan**: [../plan.md](../plan.md)

A project owner may connect a Jev-compatible decisions endpoint. The endpoint **suggests** a category;
it never stores, hides, or reclassifies an item on its own. Scout calls it after the item is durably
stored and grouped, off the ingest path.

## Connection settings

| Setting | Rule |
|---------|------|
| Endpoint | `http://` or `https://`, trailing slash trimmed |
| API key | Optional; sent as `Authorization: Bearer <key>`; stored as a secret reference |
| Model id | Non-empty, default `jev-latest` |
| Confidence threshold | `0..=1`, default `0.5` |

## Outbound request

```http
POST {endpoint}/v1/systemone
Authorization: Bearer <api key>          # only when configured
Content-Type: application/json
```

```json
{
  "model": "jev-latest",
  "state": {
    "kind": "sentry_event",
    "message": "Cannot read properties of undefined (reading 'currency')",
    "title": "TypeError: Cannot read properties of undefined (reading 'currency')",
    "exception_type": "TypeError",
    "level": "error",
    "release": "web@2.4.1",
    "environment": "production",
    "received_by": "/api/1/envelope/",
    "payload_sample": "{…redacted, at most 2048 characters…}"
  },
  "questions": {
    "category": {
      "type": "choice",
      "instructions": "Classify this incoming telemetry item for a human triager.",
      "criteria": {
        "issue": "A discrete error or exception that should be grouped for triage.",
        "log": "A log line or operational message.",
        "raw_event": "An opaque payload that should be kept but not grouped."
      }
    }
  }
}
```

### Summary construction

1. Start from derived fields: message, title, exception type, level, release, environment, receiving
   route.
2. Serialise the stored event compactly and truncate to **2,048 characters**.
3. Redact any object key whose lowercased name contains one of:
   `authorization`, `cookie`, `set-cookie`, `password`, `passwd`, `secret`, `token`, `api_key`,
   `apikey`, `access_token`, `refresh_token`, `ssn`, `credit_card`, `email`.
   Redaction replaces the value with `"[redacted]"` before serialisation, including values nested in
   objects and arrays.
4. Never send attachment bytes, and never send the full raw payload.

## Response

Scout accepts any of these shapes and reads the category from `choice`, `answer`, or `category`:

```json
{
  "choice": "issue",
  "confidence": 0.88,
  "model": "jev-latest",
  "reason": "exception with stack trace"
}
```

Category normalisation: `issue`/`error` → `issue`; `log`/`log_line` → `log`;
`raw_event`/`raw`/`event` → `raw_event`. Anything else is not a suggestion.

## Decision rules

| Condition | Result |
|-----------|--------|
| Connection not configured | No suggestion; item unchanged |
| Non-2xx response | No suggestion |
| Timeout (800 ms per attempt) | No suggestion |
| Unparseable body | No suggestion |
| Unknown or missing category | No suggestion |
| `confidence < confidence_threshold` | No suggestion |
| Valid category and confidence at or above threshold | Suggestion stored |
| A human decision already exists | Suggestion discarded |
| `first_presented_at` is set | Suggestion discarded (FR-019) |

**Retries**: at most one retry after a timeout, connection error, or `5xx`. `4xx` responses are not
retried. Total time spent is bounded and always off the ingest request path.

**Storage**: a stored suggestion records category, confidence, source (model id), and timestamp. It is
kept even after a human confirms or overrides, for audit.

**Failure is normal**: an item without a suggestion is fully usable; the UI shows "no suggestion" and
the triager can categorise manually. Ingest acceptance, storage, and list visibility are unaffected.

## Built-in mock (prototype and demos)

`POST /mock-jev/v1/systemone` returns the same response shape, choosing `log` when the summary
message contains `pool`, `log`, or `audit`, and `issue` otherwise. `POST /mock-jev/outage` with
`{ "down": true }` simulates an unreachable endpoint; `GET /mock-jev/outage` reports the current
state. The mock is for demonstrations and the integration suite; it is not part of the production
contract.

## Security notes

- The API key is never logged, returned by the API, or written to the suggestion record.
- The summary is constructed after redaction, so sensitive values are never serialised.
- Destinations must be `http(s)://`; operators are responsible for choosing a trusted endpoint.
- A project owner can disconnect the endpoint at any time; the setting takes effect for subsequent
  items without a redeploy.
