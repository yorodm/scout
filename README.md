# Scout

Self-hosted, Sentry-compatible error tracking and log ingestion.

Scout accepts the payloads existing Sentry SDKs already send, groups repeated errors into issues, and
stores raw events and logs — on your infrastructure, not a vendor's. It serves its own UI for triage,
project management and access control, and lets a project owner connect a [Jev](https://jev-api.com/docs/api)-compatible
decisions endpoint that *suggests* how an incoming item should be categorised (issue, log or raw event);
a human confirms.

> **Status: spec stage.** No implementation yet. The documents below are the source of truth.

## Documentation

| Document | Covers |
| --- | --- |
| [goal-and-requirements.md](goal-and-requirements.md) | What Scout is and why: problem, users, goals, non-goals, capabilities, metrics |
| [architecture.md](architecture.md) | Components, ingest path, data ownership, grouping, authorisation, deployment |

## Stack

Rust + Actix (API, UI delivery, background workers) · Sycamore + Tailwind CSS (UI) · PostgreSQL
(metadata) · S3-compatible object storage such as MinIO (raw payloads) · Jev-compatible decisions
endpoint (categorisation suggestions).

## Design commitments

- **Store first, classify later.** Ingest writes the raw payload and the event row, then replies.
  Grouping and categorisation happen off the request path.
- **The model suggests; a human decides.** A decision-model suggestion never gates storage, visibility
  or triage, and a classifier outage leaves items unclassified rather than misclassified. Only a
  summary — never the whole stored event — is sent to the model.
- **The raw payload is the source of truth.** Grouping and suggestions are derived from it and can be
  recomputed.
- **Authorisation is server-side.** Orgs contain projects; roles (owner, admin, triager, viewer) are
  scoped to a project; ingest keys can write but never read.
- **Cheap to self-host.** Three services — `scout`, `postgres`, S3/MinIO — no broker, no search cluster.

## Running it

Nothing to run yet. A first deployment will be a single compose file:

```sh
git clone <this repo> && cd scout
docker compose up
```

## License

[GPL-3.0](LICENSE).
