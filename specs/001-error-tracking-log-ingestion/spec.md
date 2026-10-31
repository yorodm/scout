# Feature Specification: Scout — Self-Hosted Error Tracking & Log Ingestion

**Feature Branch**: `001-error-tracking-log-ingestion`

**Created**: 2026-10-30

**Status**: Draft

**Input**: User description: "Scout — self-hosted, Sentry-compatible error tracking and log ingestion" (source: `goal-and-requirements.md`)

## Clarifications

### Session 2026-10-31

- Q: How should a project invitation be delivered and accepted, given Scout is self-hosted and runs from a single compose file? (FR-028) → A: Admin generates a one-time invite link/token shared out of band; the invitee redeems it while signing in or creating a local account, with no email/SMTP dependency.
- Q: How should people authenticate to a local Scout account? (FR-030) → A: Email and password, hashed server-side, with accounts created only by invitation or an operator and no public self-sign-up.
- Q: What may Scout include in the summary it sends to a project's decision endpoint? (FR-014) → A: Derived metadata plus at most 2 KB of payload sample, with common secret and personal-data fields redacted; never the full raw payload.
- Q: At what confidence should a decision endpoint's answer stop being recorded as a suggestion? (FR-018) → A: Below a per-project configurable threshold that defaults to 0.5.
- Q: What is the maximum accepted size for an ingest request and for a single attachment? (FR-007) → A: 10 MB per request and 10 MB per attachment.

### Session 2026-10-31 (post-plan)

- Q: When a triager confirms or overrides an item to a category different from how it was stored, what should happen to the stored item? (FR-017) → A: The decision is authoritative metadata; the item stays where it is, and the decision (not the suggestion) determines how it is labelled and filtered.
- Q: How should a local account recover access when the person forgets their password, given Scout sends no email? (FR-030) → A: Single-use recovery codes generated at account creation and stored by the person.
- Q: How should issue and log search match a query? (FR-012) → A: Case-insensitive substring matching across the user-visible fields, newest first, with a fixed result cap.
- Q: Which issue states should the first slice support? (FR-023) → A: Open, resolved, and ignored.
- Q: Should Scout enforce ingest rate limits in the first slice? (FR-009) → A: A global in-flight concurrency cap; excess requests return 429 with Retry-After, configurable with a generous default.

### Session 2026-10-31 (operator provisioning)

- Q: How should the first organisation and its first administrator be created on a fresh deployment? (FR-025, FR-030) → A: Environment variables at startup seed the deployment operator account and its password; the operator then creates organisations on first boot.
- Q: What should happen to the operator account on later restarts when the bootstrap environment variables are still set? (FR-030) → A: Create the operator only if none exists; later starts ignore the environment variables and do not reset the password.
- Q: Besides the bootstrap account, should the deployment operator be able to create other accounts directly? (FR-030) → A: Only the initial organisation admin when creating an organisation; everyone else joins by invitation.
- Q: When the operator creates an organisation's initial admin, how should that person receive their sign-in credentials? (FR-028, FR-030) → A: Scout returns a one-time setup token shared out of band; the admin redeems it to set a password and receives recovery codes.

### Session 2026-10-31 (operator security)

- Q: When the operator creates an organisation and the initial admin's email already belongs to an existing account, what should Scout do? (FR-025, FR-030) → A: Bind the existing account only after it authenticates and consents; the setup token grants `org_admin` but does not change the password.
- Q: How should the deployment operator obtain recovery codes, given startup has no API response and secrets must not be logged? (FR-030) → A: Any authenticated account, including the operator, can generate and view a new set of single-use recovery codes once.
- Q: What should Scout do on a fresh deployment when no operator exists and the bootstrap email or password is missing or blank? (FR-030) → A: Refuse to start with an actionable error naming the missing bootstrap variables.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Provision a project and ingest unmodified SDK errors into grouped issues (Priority: P1)

An operator creates an organisation and a project and issues an ingest key. A developer changes only the connection setting (the DSN) in an existing service, deploys, and an error fires. Scout accepts the payload exactly as the SDK produced it, stores the original, groups the occurrence into an issue, and keeps a running count with first-seen and last-seen timestamps.

**Why this priority**: This is the product's reason to exist. Without accepting existing SDK traffic unmodified and collapsing repeats into one issue, nothing else delivers value.

**Independent Test**: On an empty deployment, create a project with an ingest key, submit a valid error event 1,000 times through the SDK-compatible interface, and verify: all are accepted, exactly one issue exists with count 1,000, first-seen and last-seen are populated, and the original payload is retrievable.

**Acceptance Scenarios**:

1. **Given** a project with a valid ingest key, **When** an application sends a valid error event, **Then** the event is accepted, stored, and its original payload is retrievable.
2. **Given** the same error is delivered 1,000 times, **When** the events are processed, **Then** exactly one issue exists with count 1,000, the earliest occurrence recorded as first-seen, and the latest as last-seen.
3. **Given** an unknown or revoked ingest key, **When** an event is submitted, **Then** the request is rejected and no data belonging to another project is exposed.
4. **Given** a sender retries an event that carries the same event identifier, **When** both deliveries arrive, **Then** they count as a single occurrence.
5. **Given** the payload contains attachments, breadcrumbs, and a release tag, **When** the event is stored, **Then** all of them are preserved with the original payload.
6. **Given** storage is unavailable, **When** an event arrives, **Then** ingest reports a retryable failure rather than acknowledging an event it did not durably store.
7. **Given** a malformed or oversized payload, **When** it is submitted, **Then** it is rejected with an actionable error and nothing partial is persisted.

---

### User Story 2 - Triage grouped issues in the internal UI (Priority: P2)

A triager or developer on call opens a project's issue list, sees each issue's count and first/last-seen, filters and sorts to find what matters, opens an issue to read its stack trace, breadcrumbs, and release tag alongside the raw original event, and resolves it once fixed.

**Why this priority**: Grouping is only valuable if a person can act on the result; this story turns stored data into a triage workflow.

**Independent Test**: Seed a project with several issues, sign in as a project member, and verify the list shows counts and timestamps, filters and sorting work, the detail view shows the stack trace and the original payload, and resolve/reopen changes issue state.

**Acceptance Scenarios**:

1. **Given** a project with stored issues, **When** a member with access opens the issue list, **Then** each issue shows its count, first-seen, and last-seen and can be filtered, sorted, and searched.
2. **Given** an issue, **When** the member opens it, **Then** the stack trace, breadcrumbs, release tag, and the raw original event are shown.
3. **Given** a member without access to a project, **When** they attempt to view that project's issues, **Then** access is denied.
4. **Given an** open issue, **When** a triager resolves it, **Then** its state becomes resolved and the change is recorded; reopening restores it to open.
5. **Given** an open issue, **When** a triager ignores it, **Then** it is marked ignored and can be reopened; the change is recorded in its history.

---

### User Story 3 - Ingest and search logs alongside issues (Priority: P3)

A developer submits structured log lines for a project and later searches log lines and issues in one place, so an incident can be investigated without a second tool.

**Why this priority**: Unifying logs and errors is the differentiator described in the problem statement, but it depends on the project and ingest foundation from P1.

**Independent Test**: Submit structured log lines to a project, search for terms contained in them, and verify matching log lines and issues are returned together and are scoped to that project.

**Acceptance Scenarios**:

1. **Given** a project with a valid ingest key, **When** structured log lines are submitted, **Then** they are stored under that project with their timestamp, level, message, and source.
2. **Given** stored log lines and issues, **When** a member searches, **Then** matching results from both are returned in one result set.
3. **Given** a log submission with an unknown or revoked ingest key, **When** it arrives, **Then** it is rejected.
4. **Given** a user who lacks access to a project, **When** they search, **Then** that project's log lines are never returned.

---

### User Story 4 - Get a categorisation suggestion and confirm or override it (Priority: P4)

A project owner connects a decision endpoint that suggests whether an incoming item is an issue, a log, or a raw event. A triager confirms or overrides the suggestion. The item is stored either way, and a slow or unreachable model never slows or blocks ingest.

**Why this priority**: The suggestion improves classification quality, but it is an enhancement over deterministic ingest, so it ranks below the core tracking workflow.

**Independent Test**: Connect a controllable decision endpoint, submit an ambiguous item, verify a suggestion is attached; make the endpoint unreachable and verify items are still accepted and stored with no suggestion.

**Acceptance Scenarios**:

1. **Given** a project owner has connected a compatible decisions endpoint, **When** an ambiguous item arrives, **Then** Scout sends only a redacted, size-bounded summary of the item and records the returned suggested category against it.
2. **Given** an item with a suggestion, **When** a triager confirms or overrides it, **Then** the human decision is authoritative, is stored with the item, determines how the item is labelled and filtered, and the original suggestion remains recorded; the stored item itself is not moved, regrouped, or deleted.
3. **Given** the decision endpoint is unreachable or exceeds the allowed wait, **When** an item arrives, **Then** the item is stored within the normal ingest bound carrying no suggestion.
4. **Given** the decision endpoint returns malformed, unparseable, or low-confidence output, **When** the item arrives, **Then** the item is stored with no suggestion.
5. **Given** the decision endpoint is down, **When** normal traffic continues, **Then** no accepted item is dropped or delayed beyond the normal ingest bound.

---

### User Story 5 - Administer organisations, members, roles, invites, and SSO (Priority: P5)

An organisation admin creates projects, invites people, and grants or revokes a role on a specific project. A project owner manages that project's ingest keys and settings. Organisations may optionally connect OIDC/SSO while local accounts keep working.

**Why this priority**: Access control and administration are required for team use and are stated non-functional requirements, but the core tracking loop must exist first.

**Independent Test**: Create an organisation and two projects, invite two people with different project-scoped roles, and verify each person sees and can act on only what their role allows; revoke one and verify access disappears without redeploying.

**Acceptance Scenarios**:

1. **Given** an organisation, **When** an admin creates a project, **Then** the project belongs to that organisation and has its own ingest keys.
2. **Given** a member, **When** an admin grants them a role on one project, **Then** their access is limited to that project and that role.
3. **Given** a role is granted or revoked, **When** the change is saved, **Then** it takes effect without redeploying or restarting the deployment.
4. **Given** an invitation link created by an admin, **When** the invited person redeems it while signing in or creating a local account, **Then** they become a member with the role assigned in the invitation; no email is sent by Scout.
5. **Given** an organisation configured for OIDC/SSO, **When** a person signs in through the provider, **Then** they are authenticated and matched to a member; local accounts remain usable.
6. **Given** a viewer role, **When** the member attempts to resolve items, confirm categorisations, or change settings, **Then** the action is denied.
7. **Given** a project owner, **When** they rotate or revoke an ingest key, **Then** the previous key stops being accepted.
8. **Given** a fresh deployment with the operator environment variables set, **When** the operator signs in, **Then** the operator can create the first organisation and its initial organisation admin, and receive a one-time setup token for that admin. If the email already belongs to an account, that person authenticates and consents before receiving `org_admin`, and their password is unchanged.

---

### User Story 6 - Deploy as one self-hosted unit with per-project retention (Priority: P6)

An operator starts the whole stack from one compose file on their own infrastructure, configures how long each project keeps raw payloads and logs, and relies on stored raw payloads never being rewritten.

**Why this priority**: Self-hosting is the delivery model, but it packages capabilities that must exist first.

**Independent Test**: On a host with no prior data, start the stack from the single compose file, verify the UI is reachable and a first issue is stored within 10 minutes; set a short retention period and verify aged data is removed.

**Acceptance Scenarios**:

1. **Given** a fresh host with no prior data, **When** the operator starts the stack from one compose file, **Then** the application, metadata store, and object storage start and the UI is reachable.
2. **Given** a fresh deployment, **When** the first event is submitted, **Then** a stored issue is visible within 10 minutes of the stack starting.
3. **Given** a per-project retention setting, **When** raw payloads or log lines age past it, **Then** the expired data is removed and no longer retrievable or searchable.
4. **Given** a stored event, **When** any operation reads or reprocesses it, **Then** the stored raw payload is never rewritten.
5. **Given** a fresh host with no operator and missing bootstrap variables, **When** the stack starts, **Then** startup fails with an actionable error naming the missing values and no partial operator is created.

---

### Edge Cases

- **Duplicate delivery**: A sender retries the same event because an acknowledgement was lost. Events carrying the same event identifier count once, so retries do not inflate issue counts.
- **Payload with no fingerprint**: An event arrives without an explicit grouping identifier; it is grouped by a stable default identity derived from the error itself rather than creating a new issue per occurrence.
- **Unsupported item types**: A Sentry payload contains types Scout does not track (for example performance spans or metric series); these are not filed as errors or issues and the rest of the payload is handled normally.
- **Classifier returns an unexpected category**: The endpoint returns a value outside issue/log/raw event; the item is stored with no suggestion.
- **Classifier exceeds the wait budget**: Ingest proceeds and stores the item without a suggestion instead of waiting.
- **Storage write fails mid-ingest**: The request reports a retryable failure so the sender can retry; no half-written event is acknowledged.
- **Key revoked between deliveries**: A key revoked mid-stream stops being accepted immediately; already-stored events are unaffected.
- **Ingest concurrency saturated**: When the global in-flight cap is reached, excess ingest requests return `429` with `Retry-After`; already-accepted events are unaffected, and SDK retries can recover.
- **Cross-project access attempt**: A signed-in member requests another project's issue, log, or raw payload; the request is denied and no existence information leaks.
- **Retention expires an event referenced by an issue**: The raw payload is removed on schedule while the issue's metadata remains; the detail view indicates the raw payload has expired.
- **Reviewing invitation with no organisation admin**: An organisation created by the operator always has an owner before invitations are issued.
- **Bootstrap variables left set**: On later starts the operator is not recreated and the stored password is not reset.
- **Initial admin email already has an account**: The setup token grants `org_admin` only after that account authenticates and consents, and it never resets the password.
- **Recovery codes lost or exhausted**: Any authenticated account can generate a new set of single-use codes; prior unused codes are invalidated and the new set is shown once.
- **Bootstrap variables missing on a fresh deployment**: Startup fails with an actionable error naming the missing values; no partial operator is created.
- **Very large attachment**: An attachment greater than 10 MB is rejected with an actionable error rather than silently truncated.
- **Model connected but returns after the item was already shown**: A late suggestion is discarded rather than retroactively altering the stored item without human confirmation.

## Requirements *(mandatory)*

### Functional Requirements

**Ingestion and grouping**

- **FR-001**: System MUST accept Sentry-compatible event submissions from unmodified SDKs using only the project's connection setting (DSN) and ingest key.
- **FR-002**: System MUST preserve and expose the original received payload, including attachments, breadcrumbs, and release tags, without rewriting it.
- **FR-003**: System MUST authenticate every ingest request against exactly one project's ingest key and MUST reject unknown or revoked keys without disclosing other projects' data.
- **FR-004**: System MUST assign each accepted event to exactly one issue within its project using a stable grouping identity, using the sender-supplied fingerprint when present and a stable derived identity otherwise.
- **FR-005**: System MUST maintain per-issue occurrence count, first-seen, and last-seen.
- **FR-006**: System MUST treat retried deliveries carrying the same event identifier as a single occurrence.
- **FR-007**: System MUST reject malformed or unparseable payloads, and any request or single attachment larger than 10 MB, with an actionable error, and MUST NOT persist partial data.
- **FR-008**: System MUST acknowledge an ingest request only after the original payload and its metadata are durably stored; on storage failure it MUST return a retryable failure.
- **FR-009**: System MUST accept and store events within a bounded time regardless of the categorisation model's availability.

**Logs**

- **FR-010**: System MUST accept structured log lines through a documented project-scoped interface authenticated by the project's ingest key.
- **FR-011**: System MUST store each log line with at least its timestamp, level, message, source, and project scope.
- **FR-012**: System MUST allow log lines and issues to be searched together in one result set, scoped to projects the requester may access. Search MUST use case-insensitive substring matching over issue title/culprit/fingerprint and log message/source/level/attributes, order results newest first, and cap each result set at 50.

**Categorisation**

- **FR-013**: A project owner MUST be able to connect, update, and disconnect a Jev-compatible decisions endpoint per project, including its suggestion-confidence threshold.
- **FR-014**: System MUST send only a bounded summary of an item to the decisions endpoint — derived metadata plus at most 2 KB of payload sample, with common secret and personal-data fields redacted — and MUST NOT send the full raw payload.
- **FR-015**: System MUST record the suggested category returned by the endpoint (issue, log, or raw event) against the item as a suggestion, not as an imposed outcome, and a suggestion MUST NOT move or re-file the item.
- **FR-016**: System MUST store every accepted item regardless of whether a suggestion was produced.
- **FR-017**: A triager MUST be able to confirm or override a suggestion, and the human decision MUST be authoritative and recorded while the original suggestion remains auditable. The decision is authoritative metadata for labelling and filtering and MUST NOT move, regroup, or delete the stored event or issue.
- **FR-018**: System MUST treat an unreachable, timing-out, erroring, malformed, or low-confidence endpoint as producing no suggestion, where low-confidence means below the project's configured threshold (default 0.5), and MUST NOT delay ingest beyond the defined wait budget.
- **FR-019**: System MUST discard a suggestion that arrives after the item has been presented for triage rather than changing the stored item without human confirmation.

**Triage UI**

- **FR-020**: Authenticated members MUST see only projects they are authorised for.
- **FR-021**: Users MUST be able to view a project's issue list with count, first-seen, last-seen, and filtering, sorting, and search, including filtering by the human categorisation decision.
- **FR-022**: Users MUST be able to open an issue and see its stack trace, breadcrumbs, release tag, occurrence timeline, and the original raw event.
- **FR-023**: Triagers MUST be able to resolve, reopen, and ignore issues, and each state change MUST be recorded with who made it and when. The supported states are open, resolved, and ignored.
- **FR-024**: The UI MUST indicate when an issue's raw payload has expired under retention.

**Administration and access control**

- **FR-025**: System MUST support organisations containing multiple projects. A deployment operator account seeded from environment variables at startup MUST be able to create organisations and create each organisation's initial admin. If the initial admin's email already belongs to an account, the role MUST be granted only after that account authenticates and consents, and its password MUST NOT be changed.
- **FR-026**: System MUST support members assigned one of owner, admin, triager, or viewer, scoped to a specific project.
- **FR-027**: Owners and admins MUST be able to grant and revoke member roles, and changes MUST take effect without redeploying.
- **FR-028**: System MUST support invitations that carry a project role and apply it when accepted; an admin MUST generate a one-time invite link/token that is shared out of band and redeemed while the invitee signs in or creates a local account, with no email/SMTP dependency. The same one-time token mechanism MUST establish an organisation's initial admin, with the token returned to the operator for out-of-band delivery. For an existing account, the token MUST require authentication and consent and MUST NOT change the password.
- **FR-029**: Project owners MUST be able to create, rotate, and revoke per-project ingest keys.
- **FR-030**: System MUST support local accounts and optional OIDC/SSO sign-in per organisation. Local accounts MUST authenticate with email and password, with passwords hashed server-side, and MUST be created only by an invitation or as the initial organisation admin created by the deployment operator (no public self-sign-up); the operator MUST NOT create other accounts. If the initial admin's email already belongs to an account, the account is bound only after it authenticates and consents, and its password is unchanged. At account creation the system MUST issue single-use recovery codes, stored hashed, that allow a forgotten password to be reset without email. Any authenticated account, including the deployment operator, MUST be able to generate a new set of single-use recovery codes; the new set is returned once, prior unused codes are invalidated, and only hashes are stored. The initial organisation admin MUST receive a one-time setup token, set a password through it when the account is new, and receive recovery codes. The deployment operator account MUST be seeded from environment variables at startup, with its initial password taken from the environment; seeding MUST be idempotent, so later starts MUST NOT reset an existing operator's password. If no operator exists and either bootstrap value is missing or blank, startup MUST fail with an actionable error naming the missing values.
- **FR-031**: System MUST authorise every request server-side against the requesting member's project scope, including ingest-key requests.

**Operations and data lifecycle**

- **FR-032**: System MUST be deployable as a single unit comprising the application, a metadata store, and an object store, startable from one compose file.
- **FR-033**: System MUST support per-project retention periods for raw payloads and log lines and MUST delete data that exceeds them.
- **FR-034**: System MUST never rewrite a stored raw payload.
- **FR-035**: System MUST bound concurrent ingest work with a global in-flight concurrency cap. Requests above the cap MUST be rejected with `429` and a `Retry-After` header instead of queuing without bound, and the cap MUST be configurable with a generous default.

### Key Entities *(include if feature involves data)*

- **Organisation**: Top-level tenancy boundary; contains projects, members, and an optional SSO configuration.
- **Project**: A unit of tracking inside an organisation; owns issues, log lines, ingest keys, retention settings, and an optional decision-model connection.
- **Member**: A person's identity within an organisation, with one or more project-scoped role assignments.
- **Project Role Assignment**: The role (owner, admin, triager, viewer) a member holds on a specific project.
- **Invitation**: A pending offer of membership and a project role, addressed to a person and redeemed through a one-time link/token shared out of band; it does not depend on email delivery. The same token mechanics establish an organisation's initial admin.
- **Ingest Key**: A project-scoped credential that authenticates incoming events and log lines; can be rotated and revoked.
- **Issue**: A grouped identity for repeated occurrences within a project, carrying count, first-seen, last-seen, and triage state (open, resolved, or ignored).
- **Event / Occurrence**: One received submission, linked to exactly one issue, with its received timestamp and identifier.
- **Raw Payload**: The immutable original bytes of a received event, including attachments, subject to retention.
- **Log Line**: A structured log record stored under a project, with timestamp, level, message, and source.
- **Categorisation Suggestion**: The category proposed by the decision endpoint for an item, with the confidence and time received, retained for audit alongside any human decision.
- **Categorisation Decision**: The human's authoritative category for an item, recorded with the actor and time; it overrides the suggestion for labelling and filtering without moving the stored item.
- **Recovery Code**: A single-use code issued when a local account is created, stored hashed, and used to reset a forgotten password without email. An authenticated account can reissue the set, invalidating prior unused codes.
- **Deployment Operator**: The bootstrap account seeded from environment variables at startup, only when no operator exists; it is not scoped to an organisation and can create organisations and their initial organisation admin.
- **Decision Model Connection**: A project's link to a Jev-compatible endpoint, including its location, credential reference, and suggestion-confidence threshold (default 0.5); can be disconnected.
- **Retention Policy**: A per-project period after which raw payloads and log lines expire.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A fresh deployment with no prior data reaches its first stored issue within 10 minutes of the stack starting.
- **SC-002**: 1,000 identical error events produce exactly one issue with a count of 1,000.
- **SC-003**: Every stored issue opens to a raw event that is byte-identical to what the sender transmitted, apart from transport framing.
- **SC-004**: With the decision model unreachable, ingest acceptance and storage are unaffected, and items carry no suggestion rather than a wrong one.
- **SC-005**: 95% of valid accepted items become visible in their project's issue or log list within 10 seconds of receipt.
- **SC-006**: Granting or revoking a member's project access takes effect within one minute without a redeploy or restart.
- **SC-007**: 100% of attempts to read another project's issues, logs, or raw payloads by a member without access are denied.
- **SC-008**: A developer can go from the first received event to a resolved issue — viewing the raw event and confirming categorisation — in under 2 minutes.
- **SC-009**: Retried deliveries of the same event never increase an issue's count beyond the number of distinct events received.
- **SC-010**: When the global ingest concurrency cap is saturated, every excess request receives `429` with `Retry-After`, and no already-accepted event is lost or delayed beyond the normal ingest bound.
- **SC-011**: A fresh deployment with the operator environment variables set reaches a sign-in-capable operator account, and the operator can create its first organisation and obtain a one-time setup token for its initial organisation admin without direct database access.
- **SC-012**: A fresh deployment with no operator and missing bootstrap variables fails to start with an error that names the missing values.

## Assumptions

- **Log interface**: The goal document does not define how log lines arrive. Scout is assumed to expose a simple project-scoped structured-log interface authenticated by the ingest key; where a SDK emits log items in a Sentry-compatible payload, those are accepted through the same route.
- **Role capabilities**: The document names the four roles but not their exact permissions. The default is: *owner* — everything on the project, including deletion, keys, model connection, and member management; *admin* — project settings and membership except removing owners; *triager* — read, confirm/override categorisation, resolve/reopen; *viewer* — read only.
- **Default retention**: 30 days for raw payloads and log lines, configurable per project. Issue metadata (counts, state, triage history) is kept until the project is deleted; only raw payloads and log lines expire.
- **Grouping identity**: The sender's fingerprint is used when supplied; otherwise a stable derived identity based on the error type and location is used, so repeated occurrences group rather than multiply.
- **Idempotency**: Sentry-compatible events carry an event identifier; retries with the same identifier are deduplicated per project. Events without an identifier are not deduplicated.
- **Storage failure behaviour**: Ingest returns a retryable failure rather than acknowledging an event that was not durably stored, so SDK retry behaviour protects against loss.
- **Multiple organisations**: A single deployment can host multiple organisations, each with its own projects, members, and optional SSO configuration.
- **Identity**: Local accounts are always available. They sign in with email and password, and passwords are hashed server-side; accounts are created only by invitation or as the organisation's initial admin created by the deployment operator, with no public self-sign-up. Each account receives single-use recovery codes at creation for password recovery without email, and any authenticated account can reissue its codes (invalidating prior unused codes). OIDC/SSO is optional and configured per organisation; no assumption is made about a specific provider.
- **Operator provisioning**: At startup, Scout reads the operator email and password from environment variables and establishes the deployment operator account if none exists. If no operator exists and either value is missing or blank, startup fails with an actionable error rather than starting without an operator. If an operator already exists, the environment values are ignored and the stored password is not reset. The operator can create organisations and each organisation's initial admin; all other accounts join by invitation. The operator supplies the initial admin's email and receives a one-time setup token to share out of band; for a new account, redeeming it sets the password and issues recovery codes. For an existing account, the person authenticates and consents, receives `org_admin` and recovery codes, and keeps their password.
- **Suggested wait budget**: The categorisation wait budget is short enough not to be perceptible to senders (target: well under one second) and is configurable per project.
- **Suggestion confidence threshold**: Each project has a configurable threshold, defaulting to 0.5; a suggestion below it is discarded rather than shown to a triager.
- **Categorisation decisions do not re-file**: Confirming or overriding a category records an authoritative label; the event, its issue grouping, its count, and its raw payload remain unchanged.
- **Search semantics**: Unified search is case-insensitive substring matching over the fields in FR-012, ordered newest first and capped at 50 results per item kind. Full-text ranking and structured query syntax are out of scope for the first slice.
- **Ingest concurrency cap**: A single global cap protects the deployment. Excess ingest requests are rejected with `429` and `Retry-After`; the cap is configurable with a generous default and is not a per-project quota.
- **Non-goals carried forward**: Scout is self-hosted only, provides no Sentry read-API compatibility (ingestion only), and does not store metrics or traces.
- **Scope**: This specification covers the first complete product slice described in the goal document; it does not include hosted operations, billing, or mobile clients.

## Dependencies

- A metadata store and an S3-compatible object store are prerequisites for operating a deployment; they are supplied by the operator as part of the single-deployment unit.
- Sentry SDKs are expected to produce the payloads described by the Sentry ingestion protocol; Scout does not modify or patch them.
- The categorisation feature depends on a Jev-compatible decisions endpoint chosen and hosted by the project owner; when none is configured, the feature is inert and ingest proceeds normally.
- OIDC/SSO depends on an identity provider supplied by the operator's organisation.
