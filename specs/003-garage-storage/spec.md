# Feature Specification: Garage Object Storage

**Feature Branch**: `003-garage-storage`

**Created**: 2026-11-01

**Status**: Draft

**Input**: User description: "We are migrating from minio to Garage (https://garagehq.deuxfleurs.fr/documentation/quick-start/) an open source alternative for s3 storage"

## Clarifications

### Session 2026-11-01

- Q: How should the migration command receive its source and destination settings? → A: Destination uses the deployment's existing object-store settings; the source uses dedicated `SCOUT_MIGRATE_SOURCE_*` variables.
- Q: Should the migration run inside the running service or as a dedicated entry point? → A: A dedicated entry point that runs only the migration, not the service.
- Q: How should the cutover handle events written while the migration runs? → A: Migration and ingestion are separate stages that never coordinate; the operator is responsible for scheduling service downtime.
- Q: Should the shipped compose stack configure Garage automatically or leave setup to the operator? → A: The compose stack runs Garage only; the documentation walks through the Garage CLI steps for layout, bucket, and access key.
- Q: What should the migration do when the destination already holds an object under the same key? → A: Skip it when the content already matches, overwrite it when it differs, and fail only when an object cannot be read.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Deploy Scout with Garage from the shipped stack (Priority: P1)

A self-hoster follows the project's quick start on a clean host. The single compose stack starts Scout
with Garage as its object store, the raw-payload bucket exists, and the first ingested event is stored
and retrievable. Nothing in the instructions asks them to install or configure MinIO.

**Why this priority**: The whole point of the change is that the default self-hosted deployment uses
Garage. If a fresh install still needs MinIO or undocumented steps, the migration has not happened.

**Independent Test**: On a clean machine with the documented commands only, bring the stack up, send
one envelope, open the issue's raw payload, and confirm the bytes match what was sent.

**Acceptance Scenarios**:

1. **Given** a clean host and the repository's quick start, **When** the operator starts the shipped
   stack and completes the documented Garage setup, **Then** Scout, the metadata database, and Garage
   are all running and Scout reports healthy.
2. **Given** that stack, **When** an event with an attachment is ingested, **Then** the raw payload and
   attachment are stored in the Garage bucket and are readable through Scout with identical bytes.
3. **Given** expired data, **When** the retention sweep runs, **Then** the raw payload and attachment
   objects are removed from Garage while issue metadata and triage history remain.
4. **Given** the repository, **When** a reader searches the quick start and compose files, **Then** no
   step requires MinIO and Garage is the documented object store.

---

### User Story 2 - Move an existing MinIO-backed deployment to Garage (Priority: P2)

An operator running an existing deployment wants to switch object storage without losing telemetry.
They copy the stored raw payloads and attachments to Garage, verify the copy, point Scout at Garage, and
keep the old data available until they are satisfied.

**Why this priority**: Existing self-hosters are the population most affected by the switch; an
unverified cutover risks permanently losing the evidence Scout exists to preserve.

**Independent Test**: Starting from a deployment populated under MinIO, follow the documented migration
procedure, switch the endpoint and credentials, and confirm every raw payload and attachment is
retrievable with identical bytes and that a repeat run changes nothing.

**Acceptance Scenarios**:

1. **Given** an existing deployment with stored raw payloads and attachments, **When** the operator
   runs Scout's migration command with the old store as its source and Garage as the destination,
   **Then** every object is present in Garage under the same key and with identical content, and the
   run reports zero mismatches.
2. **Given** a completed migration, **When** the operator points Scout at Garage and opens old issues,
   **Then** raw payloads and attachments remain retrievable and byte-identical.
3. **Given** an interrupted migration, **When** the operator runs the command again, **Then** objects
   whose destination copy already matches are skipped, the rest are completed without duplication or
   corruption, and the final verified counts are reported.
4. **Given** an object that cannot be read or does not match after copying, **When** the command runs,
   **Then** it exits non-zero, names that object, and leaves the source untouched.
5. **Given** a verified migration, **When** the operator retires the old object store, **Then** Scout
   continues to serve all existing data and new ingests normally.

---

### User Story 3 - Understand and recover from storage misconfiguration (Priority: P3)

An operator misconfigures Garage (missing bucket, wrong credentials, unreachable endpoint, or an
incompatible addressing mode). Scout refuses to acknowledge telemetry it cannot durably store and says
what to fix.

**Why this priority**: A storage switch is exactly when credentials and buckets are wrong. Silent loss
of accepted events is the worst possible failure for an error tracker.

**Independent Test**: Point Scout at Garage with each misconfiguration in turn and confirm startup or
ingest fails with an actionable message, and that no event was acknowledged without being stored.

**Acceptance Scenarios**:

1. **Given** a configured endpoint and bucket that does not exist, **When** Scout starts, **Then** it
   fails with a message naming the bucket and the likely cause.
2. **Given** valid Garage configuration but invalid credentials, **When** an event is ingested, **Then**
   the request fails with a retryable response and nothing is acknowledged.
3. **Given** a working configuration, **When** Garage becomes unreachable, **Then** ingest returns the
   existing retryable storage failure rather than accepting and dropping the event.

---

### User Story 4 - Keep working with any S3-compatible provider (Priority: P4)

An operator already running Scout against another S3-compatible provider (including MinIO) is not
forced onto Garage. The change alters the default and the documentation, not the storage contract.

**Why this priority**: The product promise is "S3-compatible object storage", not one vendor. Forcing a
provider would break existing deployments and narrow the self-hosting story.

**Independent Test**: Run the same ingest/retrieval/retention checks against a non-Garage
S3-compatible endpoint and confirm identical behaviour.

**Acceptance Scenarios**:

1. **Given** an S3-compatible endpoint other than Garage, **When** Scout is configured with its
   endpoint, bucket, and credentials, **Then** ingest, raw retrieval, attachments, and retention work.
2. **Given** a deployment switching between S3-compatible providers, **When** the stored objects are
   copied, **Then** no application or schema change is required.

### Edge Cases

- The bucket is missing, private in an unexpected way, or the key lacks read/write/delete permission.
- The configured region, endpoint style, or signature expectations do not match what the object store
  accepts.
- Garage is running but its cluster layout or access key has not been configured yet.
- Objects were written by a different provider and are read back after the switch.
- The destination already contains an object under the same key: identical content is skipped, different
  content is overwritten, and an object that cannot be read fails the run.
- A migration is interrupted midway and re-run.
- Ingest is still running during the migration because downtime was not scheduled; the documented
  procedure warns that this leaves late events on the old store, and the tool does not prevent it.
- An object exceeds the ingest size limit, or an attachment is empty.
- Retention runs before, during, or after the migration.
- The object store is temporarily unavailable while Scout is serving reads.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The shipped single-compose deployment MUST run Scout with Garage as its object store in
  place of MinIO, using the same one-command start.
- **FR-002**: A fresh deployment MUST be given the raw-payload bucket and access credentials through
  explicit, documented Garage CLI steps (cluster layout, bucket creation, access key) so that no
  undocumented manual step is required before the first ingest; the shipped stack MUST NOT rely on an
  automatic init container for this.
- **FR-003**: Object storage MUST remain configured through the existing endpoint, bucket, credential,
  and region settings; adopting Garage MUST NOT require new application configuration concepts.
- **FR-004**: Switching object storage providers MUST NOT change object keys, object format, or the
  metadata that references objects, so previously stored data stays addressable.
- **FR-005**: Scout MUST verify the configured object store before accepting telemetry: missing bucket,
  invalid credentials, and unreachable endpoints MUST fail startup or the ingest request with an
  actionable message, and an event MUST NOT be acknowledged unless its payload was durably stored.
- **FR-006**: Ingest, raw-payload retrieval, attachment retrieval, and retention deletion MUST behave
  identically on Garage as they do today.
- **FR-007**: An existing deployment's stored objects MUST be migratable to Garage through a documented,
  repeatable procedure that verifies object presence and content identity and is safe to re-run.
- **FR-008**: The migration procedure MUST NOT delete source data; the old object store remains intact
  and usable until the operator explicitly retires it.
- **FR-009**: Documentation MUST state that migration and ingestion are separate operational stages,
  that the operator schedules service downtime for the migration, and that Scout does not coordinate
  the two.
- **FR-017**: The migration entry point MUST NOT contact, detect, wait for, or otherwise depend on the
  running service; it operates solely on the two object stores, and scheduling a quiesced source is the
  operator's responsibility.
- **FR-010**: Documentation MUST replace MinIO with Garage in the quick start, deployment notes, and
  environment examples, including the Garage-specific setup expectations an operator must satisfy.
- **FR-011**: Scout MUST keep working with any S3-compatible object store; the change is the default
  provider and its documentation, not a Garage-only dependency.
- **FR-012**: Scout MUST provide a first-class migration command that copies every raw payload and
  attachment from a specified source object store into the destination Garage store, verifies content
  identity for each object, and reports counts of found, copied, skipped, verified, and mismatched
  objects.
- **FR-013**: The migration command MUST be safe to re-run and resumable: an object whose destination
  copy already matches is skipped without rewriting it, an object whose copies differ is overwritten,
  and an interrupted run changes nothing that a repeat run cannot complete. A completed run reports the
  same verified counts.
- **FR-014**: The migration command MUST NOT modify or delete source objects, MUST take the destination
  from the deployment's existing object-store configuration and the source from a separate, explicitly
  named source configuration so the running deployment keeps using its current store until cutover, and
  MUST exit non-zero when any object fails verification while naming the affected object.
- **FR-015**: Migration MUST require no external object-storage tooling beyond Scout itself, so an
  operator needs only the binary, its configuration, and network access to both stores.
- **FR-016**: Migration MUST run from a dedicated entry point of the Scout binary that performs only
  the migration: it MUST NOT start the HTTP server, ingest, workers, or retention, and it MUST exit
  when the run finishes so an operator can script around it.

### Key Entities *(include if feature involves data)*

- **Object Store Configuration**: The endpoint, bucket, credentials, and region that identify where raw
  payloads and attachments live.
- **Raw Payload Object**: The write-once bytes of one accepted event, addressed by a stable key that
  must survive a provider change.
- **Attachment Object**: The write-once bytes of one event attachment, addressed by a stable key.
- **Migration Run**: A repeatable copy-and-verify pass from a source object store to Garage, with
  counts of found, copied, skipped (already identical), verified, and mismatched objects.
- **Destination Conflict**: An object key that already exists in Garage; identical content is skipped,
  different content is overwritten, and unreadable objects fail the run because verification is
  impossible.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A person following only the documented quick start reaches a working deployment with
  Garage and stores a first event in under 15 minutes on a clean host.
- **SC-002**: 100% of sampled raw payloads and attachments are byte-identical after storage and after a
  provider switch.
- **SC-003**: A migration of a populated deployment reports 100% of source objects verified and leaves
  zero acknowledged events unreadable after cutover.
- **SC-004**: Re-running a completed migration changes nothing and reports the same verified counts.
- **SC-005**: Every misconfiguration tested (missing bucket, bad credentials, unreachable endpoint)
  fails within 30 seconds with a message that names the setting at fault, and no event is acknowledged.
- **SC-006**: Switching providers requires no application code, schema, or object-key change — only
  configuration values.
- **SC-007**: The existing ingest, raw-retrieval, attachment, and retention checks pass unchanged against
  a non-Garage S3-compatible endpoint.
- **SC-008**: The migration command exits zero only when 100% of source objects verify, and a single
  deliberately corrupted object makes it exit non-zero while naming that object.

## Assumptions

- Garage's S3 gateway is compatible with the S3 operations Scout already uses (put, get, head, delete)
  and supports the required signature and addressing options.
- The operator is responsible for Garage's cluster layout and access-key provisioning, guided by
  step-by-step Garage CLI instructions in the documentation; the shipped compose file runs a single
  Garage node and does not configure it. Production layout and replication choices stay with the
  operator.
- The existing environment settings for endpoint, bucket, credentials, and region are reused unchanged.
- MinIO is removed from the default stack and documentation, but remains supported as one of the
  S3-compatible providers an operator may choose.
- Object keys, object formats, retention semantics, and the metadata model do not change; this feature
  moves bytes between stores, it does not reshape them.
- Scout does not dual-write to two object stores. Migration and ingestion are separate stages that
  never coordinate; the operator schedules the downtime window and decides when to switch the
  destination settings.
- Migration is performed by Scout's own command; no external object-storage tooling is required or
  assumed. That command is a dedicated entry point of the same binary (a migration mode), which keeps
  the single-deployment-unit constraint while running no part of the service.
- The migration command reads the destination from the deployment's existing object-store settings and
  the source from `SCOUT_MIGRATE_SOURCE_*` variables (endpoint, bucket, credentials, region), so secrets
  stay in the environment and the running server is untouched until cutover.
- The constitution's deployment requirement is "S3-compatible object storage", which Garage satisfies;
  no constitutional amendment is required.
- Out of scope: changing the database, changing the ingest protocol, and adopting Garage-specific
  features that are not exposed through the S3 interface.
