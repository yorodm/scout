# Feature Specification: Production Readiness — Durable Metadata and Remaining Product Surfaces

**Feature Branch**: `004-production-readiness`

**Created**: 2026-11-01

**Status**: Draft

**Input**: User description: "Complete all unresolved work from features 001 (self-hosted error tracking and log ingestion) and 002 (Sentry read-API compatibility): durable metadata persistence, safe schema upgrades, the remaining administration and authentication surfaces, optional OIDC/SSO, triage and search UI completion, and release-grade operational verification."

## Clarifications

### Session 2026-11-01 (inherited decisions)

- The durable metadata store is the production default; an in-memory store remains only for automated tests and explicit local development.
- Schema changes apply automatically on startup, idempotently, and safely when several application instances start at once. A failed upgrade aborts startup with an actionable message; it never serves on a partially upgraded schema.
- Local accounts remain always available. OIDC/SSO is optional, configured per organisation, and never replaces local sign-in.
- No email is sent. Invitations, initial-admin setup, and recovery codes are one-time out-of-band values.
- The Sentry-compatible read surface remains opt-in and disabled by default; this feature adds no new compatibility endpoints.
- Demonstration data and the demo sign-in shortcuts remain explicitly opt-in and are not the production authentication path.

### Session 2026-11-01 (production readiness)

- Q: When an application build older than the stored schema starts against an already-upgraded metadata store, what should it do? → A: Refuse to start with an actionable "schema newer than application" error; document rollback as restoring a backup or deploying a matching build.
- Q: Should the in-memory metadata store remain selectable in a production build, or be compiled out of release artifacts? → A: Remain runtime-selectable through one explicit documented setting, with the durable store as the default and a prominent startup warning when the in-memory store is active.
- Q: When demo mode is enabled on a deployment that already contains real organisations, what should the demo sign-in shortcuts do? → A: Keep them usable but confine every demo session to a single seeded demo organisation, with no access to real organisations or projects.
- Q: Should the load check be a blocking CI gate on every change, or a versioned script whose documented results are committed as release evidence? → A: A versioned load-check script plus a documented procedure, with recorded results and any accepted shortfall committed as release evidence; not a per-change CI gate.
- Q: After the first organisation administrator exists, what access should the bootstrap operator account retain? → A: Deployment-level administration only (create organisations, issue initial-admin tokens); no access to project-scoped telemetry such as issues, events, logs, or ingest keys.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Keep every record across restarts and upgrades (Priority: P1)

An operator runs Scout from the shipped stack, creates organisations, projects, members, keys, issues, logs, and audit history, then restarts or upgrades the deployment. Every record is still there, counts and history are unchanged, and the next start applies any pending schema change automatically without operator database access.

**Why this priority**: The product's promise is durable telemetry. Losing metadata on restart makes every downstream workflow unreliable and blocks the Sentry compatibility surface from being verified against a real deployment.

**Independent Test**: On a fresh stack, create an organisation, project, member, key, issue, log line, and administrative action; restart the application; confirm every record, count, state change, and audit entry is present and the service reports healthy.

**Acceptance Scenarios**:

1. **Given** a deployment with stored organisations, projects, users, keys, issues, events, logs, and audit entries, **When** the application restarts, **Then** all records and derived counts are unchanged and remain queryable.
2. **Given** an empty metadata store, **When** the application starts, **Then** pending schema changes are applied automatically and the service becomes ready.
3. **Given** an already upgraded store, **When** the application starts again, **Then** no schema change is reapplied destructively and startup succeeds.
4. **Given** several application instances starting simultaneously, **When** schema changes are pending, **Then** exactly one applies them and all instances start safely.
5. **Given** the durable metadata store is unavailable, **When** health is queried, **Then** the service reports degraded and returns a retryable failure; ingest acknowledges nothing.
6. **Given** a schema upgrade fails, **When** startup runs, **Then** startup aborts with an actionable error and the service does not serve requests.
7. **Given** a store whose schema is newer than the running build, **When** startup runs, **Then** startup aborts with an actionable error naming the version mismatch and the service does not serve requests.

---

### User Story 2 - Administer the deployment from the dashboard (Priority: P2)

The deployment operator signs in with the bootstrap credentials, creates an organisation and its first administrator, and receives a one-time setup token. Administrators then invite members by link, grant or revoke project roles, create and rotate ingest keys, and change model and retention settings — all without direct database access or a redeploy.

**Why this priority**: The backend administration exists, but the production path is incomplete while the dashboard only offers demo sign-in and cannot perform these workflows.

**Independent Test**: With no prior data, sign in as the operator, create an organisation and initial admin, accept the setup token as a new account, invite a second member, change their project role, rotate an ingest key, and verify each change is visible immediately.

**Acceptance Scenarios**:

1. **Given** a fresh deployment with bootstrap values, **When** the operator signs in, **Then** the dashboard offers organisation creation and returns a one-time initial-admin setup token.
2. **Given** an initial-admin setup token, **When** the recipient redeems it as a new account, **Then** they set a password, receive recovery codes once, and gain the intended organisation admin role.
3. **Given** an existing account, **When** it accepts an initial-admin token, **Then** it signs in first, accepts with its session, keeps its password, and gains the role without a password in the request.
4. **Given an organisation admin, **When** they invite a member and assign a project role, **Then** the invite link is usable out of band and the role applies on acceptance.
5. **Given an admin, **When** they change or revoke a member's role, **Then** access changes without restarting or redeploying.
6. **Given a project owner, **When** they create, rotate, or revoke an ingest key, **Then** the dashboard shows the new state and the previous key stops working.
7. **Given an authenticated account, **When** they request new recovery codes, **Then** the new set is shown once, prior unused codes stop working, and no plaintext is stored.
8. **Given** demo mode is disabled, **When** an unauthenticated person opens the dashboard, **Then** only the real email-and-password sign-in is offered.
9. **Given** demo mode is enabled on a deployment with real organisations, **When** a person uses a demo shortcut, **Then** every request is scoped to the demo organisation and no real organisation or project is reachable.

---

### User Story 3 - Sign in with an organisation's identity provider (Priority: P3)

An organisation connects an OIDC/SSO provider. A member signs in through it, is matched to their existing membership by verified email, and lands in the same projects and roles; local accounts keep working.

**Why this priority**: SSO is a stated requirement for team use, but it is optional and does not block the durable core or the local administration path.

**Independent Test**: Configure a mock OIDC provider for an organisation, sign in with a verified-email identity that matches a member, confirm the expected project access, and confirm an uninvited identity is rejected; then sign in locally with the same account.

**Acceptance Scenarios**:

1. **Given an organisation with OIDC configured, **When** a member completes the provider flow with a verified email, **Then** they are signed in as their existing membership without changing local credentials.
2. **Given an OIDC identity whose verified email has no invitation or membership, **When** sign-in completes, **Then** access is denied and no account is created.
3. **Given an organisation without OIDC, **When** a person signs in locally, **Then** behaviour is unchanged.
4. **Given an administrator, **When** they configure or remove the organisation's OIDC connection, **Then** the change takes effect without redeploying and local sign-in remains available.

---

### User Story 4 - Finish the triage, search, and settings experience (Priority: P4)

A triager opens an issue and sees the occurrence timeline, stack trace, breadcrumbs, release, raw payload (or an explicit expired state), and the categorisation suggestion or decision. Logs and issues can be filtered and searched in one place, and a project owner can manage the decision model and retention settings from the dashboard.

**Why this priority**: Most backend behaviour already exists; completing these views turns it into a usable product and closes the remaining UI tasks.

**Independent Test**: Seed a project with issues, occurrences, logs, and a suggestion; verify the issue timeline, raw expired state, filters, unified search, suggestion confirmation, and the model/retention form all work in the UI.

**Acceptance Scenarios**:

1. **Given an issue with several occurrences, **When** the triager opens it, **Then** the occurrence timeline, stack trace, breadcrumbs, release, and raw payload are shown.
2. **Given an issue whose raw payload expired, **When** it is opened, **Then** the view states that the payload expired and does not show stale content.
3. **Given stored issues and logs, **When** a member searches or filters, **Then** both kinds appear in one result set with the documented filters and ordering.
4. **Given an issue with a suggestion, **When** a triager confirms or overrides it, **Then** the decision is shown as authoritative and the original suggestion remains visible.
5. **Given a project owner, **When** they update the decision endpoint, threshold, or retention period, **Then** the change is saved and applies to subsequent items.
6. **Given a slow or failed request, **When** a view loads, **Then** loading, empty, and actionable error states are shown.

---

### User Story 5 - Prove the deployment is release-ready (Priority: P5)

An operator from a clean host starts the shipped stack, runs one documented smoke path, ingests representative traffic, and sees measurable evidence that durability, visibility, retention, and the compatibility surface behave as specified.

**Why this priority**: The feature is only complete when the production path has repeatable evidence, not just passing in-process tests.

**Independent Test**: On a clean host, run the documented smoke path and load check; confirm the stack becomes healthy, data survives a restart, retention removes aged data, the Sentry compatibility surface answers every documented path, and the quality gates pass.

**Acceptance Scenarios**:

1. **Given a clean host, **When** the operator follows the documented startup path, **Then** the application, metadata store, and object store become healthy without manual database access.
2. **Given a running stack, **When** the smoke path ingests an event and a log, retrieves the raw payload, runs retention, restarts the stack, and re-reads the data, **Then** every step succeeds and the data is unchanged.
3. **Given the documented load check, **When** it runs on the reference deployment, **Then** it records sustained ingest rate, acknowledgement latency, and visibility latency against the specified targets.
4. **Given the compatibility surface enabled, **When** the documented compatibility paths are exercised, **Then** every path returns a documented response and the compatibility schema upgrade has been applied and re-applied idempotently.
5. **Given the final change, **When** the quality gates run, **Then** formatting, linting, backend tests, and the UI build pass.

### Edge Cases

- The metadata store is unavailable during startup, during ingest, or during an administrative action; the service must fail closed and acknowledge nothing it did not store.
- A schema upgrade is interrupted midway; the next start must not serve on a partial schema and must be safe to retry.
- Two instances start concurrently against the same store with pending changes.
- A build older than the stored schema starts against an already-upgraded store, for example after a rollback.
- The bootstrap operator requests project-scoped telemetry; the request must be denied without revealing whether the project, issue, or log exists.
- A deployment is started with the in-memory store selected by mistake; the operator must be able to tell from the startup output and health signal that records will not survive a restart.
- An operator bootstrap value is missing while no operator exists; startup must name the missing value and create no partial account.
- An invitation or initial-admin token is expired, already used, or redeemed by the wrong account.
- A recovery code is reused or a new set is issued; only the newest set works.
- A member's role is revoked while their browser session or compatibility token is active.
- An OIDC provider returns an unverified email, an unknown member, or an unavailable endpoint.
- An issue's raw payload expires while its detail view is open.
- A search query matches nothing, exceeds the result cap, or uses an unsupported filter.
- Retention runs while a member reads data or while a migration is being applied.
- A compatibility request arrives while the compatibility surface is disabled or while its schema upgrade has not run.
- The demo mode is enabled on a deployment that also has real accounts; demo sessions must stay confined to the demo organisation and must never resolve a real project, issue, log, or key.

## Requirements *(mandatory)*

### Functional Requirements

**Durability and schema safety**

- **FR-001**: The system MUST persist all metadata needed to operate the product — organisations, projects, members and roles, users, sessions, recovery codes, invitations, ingest keys, issues, events, attachments metadata, log lines, suggestions, decisions, audit entries, and background jobs — so that every record survives an application restart.
- **FR-002**: The durable metadata store MUST be the production default; the in-memory store MUST remain available only for automated tests or an explicit local-development choice, selected through one documented setting that defaults to durable. When the in-memory store is active, startup MUST warn prominently that records will not survive a restart.
- **FR-003**: Pending schema changes MUST be applied automatically at startup, idempotently, and safely when several instances start at once; a successful deployment MUST require no manual database step.
- **FR-004**: A schema upgrade failure MUST abort startup with an actionable error, and the service MUST NOT serve requests against a partially upgraded schema. A store whose schema is newer than the running build MUST also abort startup with an actionable version-mismatch error; rollback is an operator action (restore a backup or deploy a matching build), never automatic schema reversal.
- **FR-005**: The health signal MUST reflect the durability dependency as well as object storage, returning a retryable failure and a degraded status when either is unavailable.
- **FR-006**: Background work MUST be claimed durably so that a restart neither loses queued work nor processes the same item twice.
- **FR-007**: Every administrative action already required to be auditable MUST be stored durably with actor, target, action, and time.
- **FR-008**: Ingest-key lookup, revocation, and last-used tracking MUST be durable and effective immediately across restarts.
- **FR-009**: The existing durability rule MUST hold on the durable store: an event is acknowledged only after its raw payload and metadata are stored, and a storage failure returns a retryable response.
- **FR-010**: Retention, soft deletion, and the raw-payload immutability guarantee MUST behave identically on the durable store.

**Administration and identity**

- **FR-011**: The dashboard MUST provide real password sign-in, recovery-code reset, and invitation acceptance, with demo shortcuts available only when demo mode is explicitly enabled, and a demo session MUST be confined to a seeded demo organisation with no access to real organisations or projects.
- **FR-012**: The deployment operator MUST be able to create an organisation and its initial administrator from the dashboard and receive a one-time setup token for out-of-band delivery. The operator's authority MUST remain deployment-level: it MUST NOT read project-scoped telemetry such as issues, events, logs, attachments, or ingest keys.
- **FR-013**: An initial-admin token MUST work for both a new account (set password, receive recovery codes) and an existing account (sign in first, accept with a session, keep the password, receive the role).
- **FR-014**: Administrators MUST be able to invite members, grant or revoke project roles, and see the effect immediately without a restart.
- **FR-015**: Project owners MUST be able to create, rotate, and revoke ingest keys from the dashboard.
- **FR-016**: Project owners MUST be able to manage the decision endpoint, confidence threshold, and retention period from the dashboard.
- **FR-017**: Any authenticated account MUST be able to reissue recovery codes once, invalidating prior unused codes, without storing plaintext.
- **FR-018**: An organisation MUST optionally configure OIDC/SSO; provider sign-in MUST match a verified email to an existing membership, reject uninvited identities, and leave local sign-in available.

**Triage, logs, search, and settings**

- **FR-019**: An issue detail MUST show the occurrence timeline, stack trace, breadcrumbs, release, raw payload, and an explicit expired state when retention removed it.
- **FR-020**: Issue views MUST show actionable loading, empty, and error states, and MUST offer the documented filters, sorting, and search.
- **FR-021**: Logs and issues MUST be searchable together with the documented filters, ordering, and result cap, scoped to accessible projects.
- **FR-022**: A suggestion MUST be visible next to the human decision, and confirming or overriding it MUST update the authoritative label without moving the stored event or issue.

**Release verification and operations**

- **FR-023**: The repository MUST provide one documented smoke path that, from a clean stack, starts the services, waits for health, ingests an event and a log, retrieves the raw payload, exercises retention, restarts the application, and verifies the data survived.
- **FR-024**: The repository MUST provide a repeatable load check as a versioned script with a documented procedure that records sustained ingest rate, acknowledgement latency, and acceptance-to-visibility latency against the specified targets, and the recorded measurements (including any accepted shortfall) MUST be committed as release evidence. The load check is not a per-change gate.
- **FR-025**: The application MUST expose operational counters or equivalent signals for accepted and rejected ingests, grouping outcomes, classifier outcomes, and retention deletions.
- **FR-026**: Client-facing errors for malformed input, authentication, authorisation, missing resources, oversized payloads, saturation, and storage failures MUST be actionable and MUST NOT leak project existence or secrets.
- **FR-027**: The deployment documentation and quickstart MUST match the shipped stack and implemented routes, including the metadata store, object store, bootstrap variables, and every verification scenario.
- **FR-028**: The Sentry-compatible compatibility schema additions MUST apply cleanly to an existing durable deployment, be idempotent on re-application, and preserve organisation slug and issue-number uniqueness.
- **FR-029**: Every existing behaviour and automated check from features 001 and 002 MUST continue to pass unchanged.
- **FR-030**: The change MUST pass formatting, linting, backend tests, and the UI build before it is considered complete.

### Key Entities

- **Durable Metadata Record**: Any organisation, project, user, membership, role, session, recovery code, invitation, ingest key, issue, event, attachment record, log line, suggestion, decision, audit entry, or job that must survive a restart.
- **Schema Version**: The record of which structural changes a deployment has applied, used to make upgrades idempotent and concurrency-safe.
- **Bootstrap Operator**: The deployment-level account seeded from environment values only when none exists; the entry point for creating the first organisation and administrator, and thereafter limited to deployment-level administration with no access to project-scoped telemetry.
- **One-Time Token**: An invitation, initial-admin setup token, or recovery code that is shown once, stored only as a hash, and usable once.
- **OIDC Connection**: An organisation's optional provider configuration, including the verified-email rule used to match members.
- **Operational Evidence**: The recorded smoke-path result, load measurements, and quality-gate results that prove the deployment is release-ready.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of organisations, projects, members, roles, keys, issues, events, logs, suggestions, decisions, audit entries, and queued jobs created before a restart are present and unchanged after it.
- **SC-002**: A deployment starting against an empty store applies every pending schema change automatically; restarting it reapplies nothing destructively; a build older than the stored schema refuses to start with an actionable version-mismatch error.
- **SC-003**: Three instances starting simultaneously against the same store with pending changes all become healthy, with no duplicate or partially applied change.
- **SC-004**: When the metadata store or object store is unavailable, 100% of health checks report degraded and 100% of ingest requests fail retryably without acknowledging an unstored event.
- **SC-005**: An operator can go from a fresh deployment to a created organisation and initial-admin setup token in under 5 minutes without direct database access.
- **SC-006**: A member invited with a project role gains exactly that role within one minute, and a revoked member loses access within one minute, without a restart.
- **SC-007**: An OIDC sign-in with a verified, invited email succeeds; an uninvited verified email is denied; local sign-in works before and after configuring the provider.
- **SC-008**: The issue timeline, raw-expired state, unified search, suggestion/decision view, and model/retention form are all reachable and usable in the dashboard.
- **SC-009**: The documented clean-host smoke path completes successfully and demonstrates survival across a restart.
- **SC-010**: The load check records at least 500 accepted events/s at p95 acknowledgement ≤150 ms for 10 KB events and 95% visibility within 10 seconds, or records a documented, accepted shortfall; either outcome is committed as release evidence produced by the versioned script.
- **SC-011**: All documented Sentry compatibility paths return their documented response after the compatibility schema is applied and re-applied idempotently.
- **SC-012**: 100% of the existing 001 and 002 automated checks continue to pass.
- **SC-013**: Formatting, linting, backend tests, and the UI production build pass in the final gate.
- **SC-014**: With demo mode enabled alongside real organisations, 100% of demo-session requests are scoped to the demo organisation and 0 real projects, issues, logs, or keys are readable from them.
- **SC-015**: 100% of project-scoped requests made with the bootstrap operator account are denied without disclosing resource existence, while every deployment-level administrative action it needs still succeeds.

## Assumptions

- Features 001 and 002 remain the authoritative statement of product behaviour; this feature completes their unresolved work and does not add new product scope.
- The shipped single-compose deployment remains the delivery model, with Garage as the default object store from feature 003.
- The durable metadata store is available in the shipped stack; the operator does not need to run schema commands by hand.
- The shipped stack never selects the in-memory store; the in-memory option exists only for automated tests and deliberate local development.
- Local accounts are always available; OIDC is optional and configured by an organisation administrator.
- No email delivery is introduced; all invitations, setup tokens, and recovery codes are delivered out of band.
- Demo mode remains opt-in and is never the production authentication path.
- The Sentry-compatible surface remains opt-in and limited to the contract ratified by feature 002.
- Hard deletion of projects or issues, metrics, traces, and Sentry subsystems outside feature 002 remain out of scope.
- The reference deployment for performance evidence is the documented 4 vCPU self-hosted deployment.

## Dependencies

- Feature 001 (self-hosted error tracking and log ingestion) and feature 002 (Sentry read-API compatibility) specifications and plans.
- The project constitution, currently v1.1.0, including the durability, authorisation, and compatibility constraints.
- The existing schema migration set and the shipped compose stack, including the object store and metadata-store services.
- OIDC/SSO depends on a provider supplied and configured by the operator's organisation.
