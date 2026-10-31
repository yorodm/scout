# Contract: Ingest API

**Feature**: `001-error-tracking-log-ingestion` | **Plan**: [../plan.md](../plan.md)

Scout accepts Sentry-compatible telemetry and structured logs. `{project}` is either the project's
`dsn_id` (the numeric DSN path segment) or its slug.

Base URL: `SCOUT_URL` (for example `http://localhost:8080`).

## Authentication

Every ingest request must present exactly one project's ingest key, in any of these forms:

| Presentation | Example |
|--------------|---------|
| Query parameter | `?sentry_key=scout_…` |
| Sentry header | `X-Sentry-Auth: Sentry sentry_version=7, sentry_key=scout_…` |
| Authorization header | `Authorization: Sentry sentry_version=7, sentry_key=scout_…` |
| Scout header | `X-Scout-Key: scout_…` |

The key is hashed and compared against `ingest_keys.key_hash`. Unknown, revoked, and other-project
keys all return `401` with the same body so no project existence information leaks.

## Common limits and behaviour

- Maximum request body: **10 MB**. Maximum single attachment: **10 MB**. Larger input returns `413`
  and nothing partial is persisted.
- `Content-Encoding: gzip` is accepted and decompressed before parsing. Any other encoding returns
  `400`.
- Acknowledgement happens only after the raw object, event metadata, and grouping job are durably
  committed. A storage failure returns `503` with a retryable error.
- A global in-flight concurrency cap bounds ingest work. Requests above the cap return `429` with a
  `Retry-After` header instead of queuing without bound; the cap is configurable with a generous
  default.
- Events are deduplicated per project by `sentry_event_id`; a retried delivery returns the previously
  accepted identifier and does not increment the issue count.
- Unknown envelope item types (for example spans or metrics) are counted in `ignored` and do not fail
  the request.

## `POST /api/{project}/envelope/`

Accepts a Sentry envelope. Trailing slash is optional.

**Headers**

```http
Content-Type: application/x-sentry-envelope
Content-Encoding: gzip            # optional
X-Sentry-Auth: Sentry sentry_version=7, sentry_key=scout_…
```

**Body**

An envelope header line, followed by zero or more items. Items may be newline-delimited or
length-prefixed (`{"type":"event","length":123}` followed by exactly that many bytes). Supported item
types:

| Type | Payload | Handling |
|------|---------|----------|
| `event` | JSON object | Stored, grouped, optionally classified |
| `attachment` | arbitrary bytes | Stored with filename and content type |
| `log` | JSON object | Stored as a log line |
| other | any | Counted as ignored |

If the envelope header carries an `event_id` and exactly one event item omits one, the header value is
used. Otherwise a new event id is generated.

**Response `200`**

```json
{
  "id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "event_ids": ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
  "logs": 0,
  "ignored": 1
}
```

**Errors**

| Status | When |
|--------|------|
| `400` | Malformed envelope, invalid JSON item, truncated length, or unsupported encoding |
| `401` | Missing, unknown, revoked, or other-project ingest key |
| `404` | Project reference not found |
| `413` | Request or attachment exceeds 10 MB |
| `429` | Global ingest concurrency cap saturated; honor `Retry-After` |
| `503` | Raw storage or metadata commit failed; safe to retry |

## `POST /api/{project}/store/`

Legacy Sentry event submission. The body is a single JSON event object.

**Response `200`**

```json
{ "id": "dddddddddddddddddddddddddddddddd" }
```

Errors follow the same table as the envelope endpoint. Malformed JSON is `400`.

## `POST /api/{project}/logs/`

Accepts structured log lines as a JSON object, a JSON array, an object containing `logs` or `items`,
or newline-delimited JSON.

Each accepted line needs at least a `message` (or `body`). Optional fields are `timestamp` (RFC 3339
or epoch seconds/milliseconds), `level` (default `info`), `source` (falls back to `service`, `logger`,
then `ingest`), and `attributes` (either a plain object or a `{key: {value: …}}` object).

**Response `200`**

```json
{ "accepted": 2 }
```

**Errors**: `400` for invalid or empty JSON, `401` for a bad key, `404` for an unknown project.

## Durability and ordering

1. Authenticate the key against the project.
2. Decompress and parse the body.
3. Write raw objects (event payload and attachments) with create-only semantics.
4. In one transaction: insert the event row, insert the grouping job, and apply retention for the
   project.
5. Return `200`.

No grouping, classification, or search work happens before the response.
