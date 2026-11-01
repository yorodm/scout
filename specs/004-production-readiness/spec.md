# Feature Specification: Production Readiness — Durable Metadata and Remaining Product Surfaces

**Feature Branch**: `004-production-readiness`

**Created**: 2026-11-01

**Status**: Draft

**Input**: User description: "Complete all unresolved work from features 001 (self-hosted error tracking and log ingestion) and 002 (Sentry read-API compatibility): durable metadata persistence (including startup schema migrations), the remaining administration and authentication surfaces, optional OIDC/SSO, triage and search UI completion, and release-grade operational verification."

## Clarifications

### Session 2026-11-01 (inherited decisions)

- The durable metadata store is the production default; an in-memory store remains only for automated tests and explicit local development.
- The product keeps its database migrations and runs its migration engine at startup whenever migrations are available. Planning, sequencing, and rolling back upgrades belongs to the operator; this feature specifies no concurrency, ordering, or rollback guarantees for that process.
- Local accounts remain always available. OIDC/SSO is optional, configured per organisation, and never replaces local sign-in.
- No email is sent. Invitations, initial-admin setup, and recovery codes are one-time out-of-band values.
- The Sentry-compatible read surface remains opt-in and disabled by default; this feature adds no new compatibility endpoints.
- Demonstration data and the demo sign-in shortcuts remain explicitly opt-in and are not the production authentication path.

### Session 2026-11-01 (production readiness)

- Q: Should the in-memory metadata store remain selectable in a production build, or be compiled out of release artifacts? → A: Remain runtime-selectable through one explicit documented setting, with the durable store as the default and a prominent startup warning when the in-memory store is active.
- Q: When demo mode is enabled on a deployment that already contains real organisations, what should the demo sign-in shortcuts do? → A: Keep them usable but confine every demo session to a single seeded demo organisation, with no access to real organisations or projects.
- Q: Should the load check be a blocking CI gate on every change, or a versioned script whose documented results are committed as release evidence? → A: A versioned load-check script plus a documented procedure, with recorded results and any accepted shortfall committed as release evidence; not a per-change CI gate.
- Q: After the first organisation administrator exists, what access should the bootstrap operator account retain? → A: Deployment-level administration only (create organisations, issue initial-admin tokens); no access to project-scoped telemetry such as issues, events, logs, or ingest keys.

### Session 2026-11-01 (multi-instance operations)

- Q: If an operator runs more than one Scout instance against the same metadata store, must every instance serve the same up-to-date data, or is only safe concurrent startup required? → A: Every instance resolves metadata from the shared store on every request; no instance serves from an instance-local copy of metadata.
- Q: Who should be allowed to read Scout's operational counters? → A: The deployment operator only, through an authenticated session; the counters describe deployment-wide traffic and stay behind the deployment-level role.
- Q: How long should an invitation or initial-admin setup token stay usable before it expires? → A: Seven days for both invitation and initial-admin tokens, matching the existing invitation lifetime; expiry is reported explicitly.
- Q: Must the audit trail be browsable in the dashboard, or is durable storage enough for this feature? → A: Durable storage only; this feature adds no audit read endpoint or screen, and verification is through automated checks and direct database inspection.
- Q: When the metadata store is momentarily unavailable, should ingest fail immediately or retry briefly before returning the retryable failure? → A: Fail immediately with a retryable 503 and Retry-After; no internal retry or buffering, and the client retries.

### Session 2026-11-01 (operations)

- Q: How should a deployment operator change or recover the bootstrap password after the deployment is running? → A: The operator changes its own password from the dashboard, and one set of recovery codes is issued once on its first sign-in; recovery codes are the lockout path and require no database access.
- Q: Which clock decides whether an event or a log line has aged out of retention? → A: The server's receipt time decides for both events and log lines; client-supplied timestamps are display and ordering attributes only.
- Q: Must this feature deliver a tested backup and restore procedure, or only documentation describing one? → A: Documentation only — a complete operator-run backup and restore procedure with expected outcomes; no backup tooling or automated restore verification ships with this feature.
- Q: When an account is disabled or a member is removed, what should happen to sessions and API tokens it already holds? → A: Every existing session and API token stops working on that account's next request.
- Q: During an upgrade with several instances running, may an older instance keep serving while a newer one applies schema changes? → A: The product keeps its migration files and runs the migration engine at startup when pending migrations exist; the process around upgrades (planning, sequencing, rollback) is the operator's responsibility, and no concurrency or rollback guarantees are specified here.

### Session 2026-11-01 (security posture)

- Q: What minimum requirements should a password meet when someone sets or changes it? → A: At least 12 characters, no composition rules, rejected with an actionable message.
- Q: How should Scout respond to repeated failed sign-in attempts against the same account? → A: Five consecutive failures lock the account regardless of elapsed time; it stays locked until the deployment operator unlocks it, while every rejection stays generic. Client-IP protection is left to the operator's reverse proxy.
- Q: How long should a dashboard session stay valid before the person must sign in again? → A: Seven-day absolute maximum plus a 12-hour idle timeout, both configurable.
- Q: What should count as activity that keeps a dashboard session alive? → A: Any authenticated request carrying the session; compatibility API tokens do not extend a session.
- Q: How many results should a unified issues-and-logs search return when far more match? → A: At most 200 combined results, newest first, with a `has_more` indicator and no pagination.
- Q: Who is allowed to create a new project inside an organisation? → A: Only the deployment operator creates projects and designates an existing organisation member as the project's initial owner; administrators manage the project afterwards.
- Q: When the operator creates a project, should the response include that project's ingest key and DSN? → A: No. Creation returns project metadata only; the named owner creates the first ingest key afterwards and receives the full DSN then.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Keep every record across restarts (Priority: P1)

An operator runs Scout from the shipped stack, creates organisations, projects, members, keys, issues, logs, and audit history, then restarts the deployment. Every record is still there and counts and history are unchanged.

**Why this priority**: The product's promise is durable telemetry. Losing metadata on restart makes every downstream workflow unreliable and blocks the Sentry compatibility surface from being verified against a real deployment.

**Independent Test**: On a fresh stack, create an organisation, project, member, key, issue, log line, and administrative action; restart the application; confirm every record, count, state change, and audit entry is present and the service reports healthy.

**Acceptance Scenarios**:

1. **Given** a deployment with stored organisations, projects, users, keys, issues, events, logs, and audit entries, **When** the application restarts, **Then** all records and derived counts are unchanged and remain queryable.
2. **Given** migrations the deployment has not applied yet, **When** the application starts, **Then** the migration engine applies them and only then does the service report ready.
3. **Given** the durable metadata store is unavailable, **When** health is queried, **Then** the service reports degraded and returns a retryable failure; ingest acknowledges nothing.

---

### User Story 2 - Administer the deployment from the dashboard (Priority: P2)

The deployment operator signs in with the bootstrap credentials, creates an organisation and its first administrator, and receives a one-time setup token. The operator also creates the organisation's projects, naming an existing member as each project's initial owner. Administrators then invite members by link, grant or revoke project roles, create and rotate ingest keys, and change model and retention settings — all without direct database access or a redeploy.

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
10. **Given** the operator, **When** they create a project and name an existing organisation member as its initial owner, **Then** that member owns the project and administrators manage its members, keys, settings, and soft deletion; a non-operator attempt to create a project is refused.
11. **Given** the operator signs in for the first time, **When** the console loads, **Then** a single set of recovery codes is shown once and must be acknowledged; subsequent sign-ins do not re-show them.
12. **Given** an account locked by repeated failed sign-ins, **When** the operator unlocks it, **Then** the person signs in with the correct password, the failure count is reset, and until that unlock every attempt returns the generic invalid-credentials response.

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

1. **Given** a clean host, **When** the operator follows the documented setup path, including whatever metadata-store preparation the documentation requires, **Then** the application, metadata store, and object store become healthy.
2. **Given a running stack, **When** the smoke path ingests an event and a log, retrieves the raw payload, runs retention, restarts the stack, and re-reads the data, **Then** every step succeeds and the data is unchanged.
3. **Given the documented load check, **When** it runs on the reference deployment, **Then** it records sustained ingest rate, acknowledgement latency, and visibility latency against the specified targets.
4. **Given the compatibility surface enabled, **When** the documented compatibility paths are exercised, **Then** every path returns a documented response once the startup migrations have completed.
5. **Given the final change, **When** the quality gates run, **Then** formatting, linting, backend tests, and the UI build pass.

### Edge Cases

- The metadata store is unavailable during startup, during ingest, or during an administrative action; the service must fail closed, acknowledge nothing it did not store, and fail ingest immediately with a retry hint rather than retrying internally.
- A startup migration fails; the service must not serve requests until the operator resolves it, and the application must not attempt to revert applied migrations on its own.
- Several instances serve traffic while an administrator changes a role or revokes a key through one of them; every other instance must honour the change on its next request.
- The bootstrap operator requests project-scoped telemetry; the request must be denied without revealing whether the project, issue, or log exists.
- A deployment is started with the in-memory store selected by mistake; the operator must be able to tell from the startup output and health signal that records will not survive a restart.
- An operator bootstrap value is missing while no operator exists; startup must name the missing value and create no partial account.
- The operator loses the bootstrap password before its first sign-in, so no recovery codes have been issued yet; the documented offline procedure is the only path and startup must never reset an existing operator password.
- An invitation or initial-admin token is expired, already used, or redeemed by the wrong account; an expired token (seven days after issue) is reported distinctly from an unknown one, and a token redeemed by the wrong account is refused without consuming it.
- A recovery code is reused or a new set is issued; only the newest set works.
- A password shorter than 12 characters is submitted through any path that sets one; it is rejected with a message naming the minimum, and no partial account or session is created.
- An account receives repeated failed sign-ins from an automated client; after five consecutive failures it locks, every later attempt receives the generic invalid-credentials response even with the correct password, and it stays locked until the operator unlocks it; any successful sign-in resets the count.
- A dashboard session sits idle past the configured window or ages past the absolute maximum; its next request is rejected and the person signs in again, while a compatibility API token held by the same account keeps working.
- A member's role is revoked while their browser session or compatibility token is active; the same session and token lose the revoked capability immediately.
- An account is disabled or removed while it holds a session and several compatibility tokens; the next request from any of them is rejected, including for routes the account could reach a moment earlier.
- An OIDC provider returns an unverified email, an unknown member, or an unavailable endpoint.
- An issue's raw payload expires while its detail view is open.
- A search query matches nothing, exceeds the result cap, or uses an unsupported filter; exceeding the cap returns the 200 newest matches with an indication that more exist.
- Retention runs while a member reads data.
- A client sends log lines or events with a timestamp far in the past or future; retention must still follow the server receipt time, and display must not misrepresent the age of the data.
- A compatibility request arrives while the compatibility surface is disabled.
- A non-operator account attempts to create a project, or the operator names an initial owner who is not a member of the owning organisation; the request is refused and no project is created.
- The operator creates a project and inspects the response; it contains no ingest key and no DSN, so the credential reaches only the owner who creates the first key.
- The demo mode is enabled on a deployment that also has real accounts; demo sessions must stay confined to the demo organisation and must never resolve a real project, issue, log, or key.

## Requirements *(mandatory)*

### Functional Requirements

**Durability**

- **FR-001**: The system MUST persist all metadata needed to operate the product — organisations, projects, members and roles, users, sessions, recovery codes, invitations, ingest keys, issues, events, attachments metadata, log lines, suggestions, decisions, audit entries, and background jobs — so that every record survives an application restart.
- **FR-002**: The durable metadata store MUST be the production default; the in-memory store MUST remain available only for automated tests or an explicit local-development choice, selected through one documented setting that defaults to durable. When the in-memory store is active, startup MUST warn prominently that records will not survive a restart.
- **FR-003**: The application MUST run its migration engine at startup whenever migrations are available, applying pending migrations before the service reports ready. Planning upgrades, sequencing instances, and preparing rollback remain the operator's responsibility; the application MUST NOT attempt to revert applied migrations on its own.
- **FR-005**: The health signal MUST reflect the durability dependency as well as object storage, returning a retryable failure and a degraded status when either is unavailable.
- **FR-006**: Background work MUST be claimed durably so that a restart neither loses queued work nor processes the same item twice.
- **FR-007**: Every administrative action already required to be auditable MUST be stored durably with actor, target, action, and time. This feature guarantees retention and completeness only; it adds no endpoint or dashboard view for browsing the audit trail.
- **FR-008**: Ingest-key lookup, revocation, and last-used tracking MUST be durable and effective immediately across restarts.
- **FR-009**: The existing durability rule MUST hold on the durable store: an event is acknowledged only after its raw payload and metadata are stored, and a storage failure returns a retryable response. A store failure MUST be returned immediately with a retryable status and a retry hint; the application MUST NOT retry internally, buffer in memory, or acknowledge anything it did not store.
- **FR-010**: Retention, soft deletion, and the raw-payload immutability guarantee MUST behave identically on the durable store. Retention age MUST be measured from the server's receipt time for both events and log lines; a client-supplied timestamp MUST NOT cause data to be deleted early or kept beyond the project window.
- **FR-031**: Every running instance MUST resolve organisations, projects, memberships, roles, sessions, ingest keys, and API tokens from the shared durable store on each request. No instance may serve from an instance-local copy of that metadata, so a change made through one instance is visible through every other without a restart.
- **FR-032**: Five consecutive failed sign-in attempts for the same account MUST lock that account, regardless of the time between attempts. A successful sign-in resets the count. A locked account MUST reject every sign-in attempt with the same generic response used for invalid credentials, regardless of whether the password is correct, so the lock never reveals that the account exists. Only the deployment operator can unlock an account, and unlocking resets the count. The operator MUST be able to list locked accounts in order to find them. Existing sessions are not ended by a lock, and per-client-IP protection is explicitly the operator's reverse proxy responsibility.
- **FR-033**: A dashboard session MUST expire after an absolute maximum of seven days and after 12 hours without activity, both configurable, and MUST be rejected on its next request once expired. Activity means any authenticated request carrying the session. Long-lived API tokens issued for the compatibility surface are unaffected by session timeouts and do not extend a session; they follow their own revocation rules.
- **FR-034**: Only the deployment operator MUST be able to create a project. Creation MUST name the owning organisation and designate an existing member of that organisation as the project's initial owner, and MUST return project metadata only. Organisation administrators and members MUST NOT create projects, and administrators manage membership, keys, settings, and soft deletion through the roles they already hold.

**Administration and identity**

- **FR-011**: The dashboard MUST provide real password sign-in, recovery-code reset, and invitation acceptance, with demo shortcuts available only when demo mode is explicitly enabled, and a demo session MUST be confined to a seeded demo organisation with no access to real organisations or projects. Every path that sets a password (initial-admin setup, invitation acceptance, recovery, operator change) MUST require at least 12 characters, apply no composition rules, and reject a shorter value with an actionable message.
- **FR-012**: The deployment operator MUST be able to create an organisation and its initial administrator from the dashboard and receive a one-time setup token for out-of-band delivery. The operator's authority MUST remain deployment-level: it MUST NOT read project-scoped telemetry such as issues, events, logs, attachments, or ingest keys. Creating a project is a deployment-level action allowed to the operator; reading that project's telemetry is not, and the creation response MUST contain project metadata only — no ingest key and no DSN. The operator MUST also be able to change its own password, MUST receive one set of recovery codes the first time it signs in so a lost password is recoverable without database access, and MUST be able to unlock an account locked by repeated failed sign-ins.
- **FR-013**: An initial-admin token MUST work for both a new account (set password, receive recovery codes) and an existing account (sign in first, accept with a session, keep the password, receive the role). Every invitation and initial-admin token MUST expire seven days after issue and MUST report an expired token explicitly rather than failing as if it were unknown.
- **FR-014**: Administrators MUST be able to invite members, grant or revoke project roles, and see the effect immediately without a restart. Disabling an account or removing a member MUST invalidate every session and API token that account holds, effective on its next request.
- **FR-015**: Project owners MUST be able to create, rotate, and revoke ingest keys from the dashboard.
- **FR-016**: Project owners MUST be able to manage the decision endpoint, confidence threshold, and retention period from the dashboard.
- **FR-017**: Any authenticated account MUST be able to reissue recovery codes once, invalidating prior unused codes, without storing plaintext.
- **FR-018**: An organisation MUST optionally configure OIDC/SSO; provider sign-in MUST match a verified email to an existing membership, reject uninvited identities, and leave local sign-in available.

**Triage, logs, search, and settings**

- **FR-019**: An issue detail MUST show the occurrence timeline, stack trace, breadcrumbs, release, raw payload, and an explicit expired state when retention removed it.
- **FR-020**: Issue views MUST show actionable loading, empty, and error states, and MUST offer the documented filters, sorting, and search.
- **FR-021**: Logs and issues MUST be searchable together with the documented filters, ordering, and result cap, scoped to accessible projects. The cap MUST be 200 combined results, newest first, and a response that truncates MUST indicate that more matches exist rather than silently dropping them.
- **FR-022**: A suggestion MUST be visible next to the human decision, and confirming or overriding it MUST update the authoritative label without moving the stored event or issue.

**Release verification and operations**

- **FR-023**: The repository MUST provide one documented smoke path that, from a clean stack, starts the services, waits for health, ingests an event and a log, retrieves the raw payload, exercises retention, restarts the application, and verifies the data survived.
- **FR-024**: The repository MUST provide a repeatable load check as a versioned script with a documented procedure that records sustained ingest rate, acknowledgement latency, and acceptance-to-visibility latency against the specified targets, and the recorded measurements (including any accepted shortfall) MUST be committed as release evidence. The load check is not a per-change gate.
- **FR-025**: The application MUST expose operational counters or equivalent signals for accepted and rejected ingests, grouping outcomes, classifier outcomes, and retention deletions. They MUST be readable only by the deployment operator through an authenticated session; no unauthenticated or member-scoped access is permitted.
- **FR-026**: Client-facing errors for malformed input, authentication, authorisation, missing resources, oversized payloads, saturation, and storage failures MUST be actionable and MUST NOT leak project existence or secrets.
- **FR-027**: The deployment documentation and quickstart MUST match the shipped stack and implemented routes, including the metadata store, the migrations the build ships, the object store, bootstrap variables, and every verification scenario. The documentation MUST also describe an operator-run backup and restore procedure that names every artifact to preserve, the restore order, and the checks that confirm a successful restore.
- **FR-029**: Every existing behaviour and automated check from features 001 and 002 MUST continue to pass unchanged.
- **FR-030**: The change MUST pass formatting, linting, backend tests, and the UI build before it is considered complete.

### Key Entities

- **Durable Metadata Record**: Any organisation, project, user, membership, role, session, recovery code, invitation, ingest key, issue, event, attachment record, log line, suggestion, decision, audit entry, or job that must survive a restart.
- **Schema Version**: The record of which migrations a deployment has applied; the application applies pending ones at startup, while planning and sequencing upgrades belongs to the operator.
- **Bootstrap Operator**: The deployment-level account seeded from environment values only when none exists; the entry point for creating the first organisation and administrator, and thereafter limited to deployment-level administration with no access to project-scoped telemetry.
- **One-Time Token**: An invitation, initial-admin setup token, or recovery code that is shown once, stored only as a hash, and usable once. Invitations and initial-admin setup tokens expire seven days after issue; recovery codes remain valid until used or replaced by a newer set.
- **OIDC Connection**: An organisation's optional provider configuration, including the verified-email rule used to match members.
- **Operational Evidence**: The recorded smoke-path result, load measurements, and quality-gate results that prove the deployment is release-ready.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of organisations, projects, members, roles, keys, issues, events, logs, suggestions, decisions, audit entries, and queued jobs created before a restart are present and unchanged after it.
- **SC-004**: When the metadata store or object store is unavailable, 100% of health checks report degraded and 100% of ingest requests fail retryably without acknowledging an unstored event, each returned without an internal retry or in-memory buffering.
- **SC-005**: An operator can go from a fresh deployment to a created organisation and initial-admin setup token in under 5 minutes without direct database access.
- **SC-006**: A member invited with a project role gains exactly that role within one minute, and a revoked member loses access within one minute, without a restart.
- **SC-007**: An OIDC sign-in with a verified, invited email succeeds; an uninvited verified email is denied; local sign-in works before and after configuring the provider.
- **SC-008**: The issue timeline, raw-expired state, unified search, suggestion/decision view, and model/retention form are all reachable and usable in the dashboard.
- **SC-009**: The documented clean-host smoke path completes successfully and demonstrates survival across a restart.
- **SC-010**: The load check records at least 500 accepted events/s at p95 acknowledgement ≤150 ms for 10 KB events and 95% visibility within 10 seconds, or records a documented, accepted shortfall; either outcome is committed as release evidence produced by the versioned script.
- **SC-011**: All documented Sentry compatibility paths return their documented response on a deployment whose schema matches the build.
- **SC-012**: 100% of the existing 001 and 002 automated checks continue to pass.
- **SC-013**: Formatting, linting, backend tests, and the UI production build pass in the final gate.
- **SC-014**: With demo mode enabled alongside real organisations, 100% of demo-session requests are scoped to the demo organisation and 0 real projects, issues, logs, or keys are readable from them.
- **SC-015**: 100% of project-scoped requests made with the bootstrap operator account are denied without disclosing resource existence, while every deployment-level administrative action it needs still succeeds.
- **SC-016**: With three instances serving one durable store, a role change, key revocation, or new issue created through any instance is observed through all other instances on their next request; 0 requests are served from instance-local metadata.
- **SC-017**: The operator can change its password from the dashboard and recover access with a recovery code; after a reissue, 0 prior unused codes work, and 0 plaintext codes are stored or shown a second time.
- **SC-018**: Retention deletes exactly the events and log lines whose server receipt time is older than the project window; 0 rows are deleted early or retained past the window because of a client-supplied timestamp.
- **SC-019**: The documented backup and restore procedure names every artifact, the restore order, and its verification steps, and following it restores all organisations, projects, issues, logs, and raw payloads on a clean host.
- **SC-020**: 0 requests succeed with a session or API token belonging to a disabled account or removed member on the first request after the change.
- **SC-021**: A deployment with pending migrations applies them during startup and only then reports ready; 0 requests are served before the migration engine completes, and 0 applied migrations are reverted automatically.
- **SC-022**: 100% of password-setting paths reject values shorter than 12 characters with a message stating the minimum, and every stored password remains an Argon2id hash.
- **SC-023**: 5 consecutive failed sign-ins for one account lock it; 100% of subsequent sign-in attempts return the generic invalid-credentials response, with 0 responses revealing that the account exists or that it is locked, and the account resumes only after the deployment operator unlocks it.
- **SC-024**: 100% of sessions idle beyond the configured window, where activity is any authenticated request carrying the session, or older than the absolute maximum are rejected on their next request, and 0 compatibility API tokens are invalidated by, or extend, a session timeout.
- **SC-025**: A search matching more than 200 items returns exactly the 200 newest with `has_more` set, and 0 results come from projects the caller cannot access.
- **SC-026**: 100% of project-creation requests succeed only for the deployment operator, every created project has an initial owner who already belongs to the owning organisation, and 0 creation responses contain an ingest key or DSN.

## Assumptions

- Features 001 and 002 remain the authoritative statement of product behaviour; this feature completes their unresolved work and does not add new product scope.
- The shipped single-compose deployment remains the delivery model, with Garage as the default object store from feature 003.
- The durable metadata store is available in the shipped stack.
- The operator plans and sequences upgrades; the application applies pending migrations at startup when the build ships them.
- The shipped stack never selects the in-memory store; the in-memory option exists only for automated tests and deliberate local development.
- Local accounts are always available; OIDC is optional and configured by an organisation administrator.
- No email delivery is introduced; all invitations, setup tokens, and recovery codes are delivered out of band.
- Demo mode remains opt-in and is never the production authentication path.
- The Sentry-compatible surface remains opt-in and limited to the contract ratified by feature 002.
- Hard deletion of projects or issues, metrics, traces, and Sentry subsystems outside feature 002 remain out of scope.
- Browsing the audit trail through the dashboard is out of scope; the feature guarantees that entries are stored completely and durably, verified by automated checks and direct inspection.
- Backup and restore are documented operator procedures: the feature ships no backup tooling and no automated restore verification, and orphaned-object reconciliation after a restore remains the operator's responsibility.
- The reference deployment for performance evidence is the documented 4 vCPU self-hosted deployment.

## Dependencies

- Feature 001 (self-hosted error tracking and log ingestion) and feature 002 (Sentry read-API compatibility) specifications and plans.
- The project constitution, currently v1.1.0, including the durability, authorisation, and compatibility constraints.
- The shipped compose stack, including the object store and metadata-store services.
- OIDC/SSO depends on a provider supplied and configured by the operator's organisation.
