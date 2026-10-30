---
id: scout-goal
type: goal-and-requirements
status: active
title: Scout — self-hosted error tracking & log ingestion
---

# Scout

Self-hosted, Sentry-compatible error tracking and log ingestion.

## Overview

Scout is a self-hosted error tracker and log ingester written in Rust on Actix. A team points its
existing Sentry SDK at Scout instead of at Sentry: Scout accepts the Sentry-compatible payloads,
groups repeated errors into issues, and stores the raw events and logs it received in S3-compatible
storage with metadata in Postgres. It serves an internal UI, built with Sycamore and Tailwind CSS,
for triage, project management and access control, and it lets a project owner connect a decision
model that suggests how an incoming item should be categorised — as an issue, a log or a raw event —
for a triager to confirm. It is published under GPL-3.0 for anyone who wants their telemetry on their
own infrastructure.

## Problem Statement

A team that wants its telemetry on its own infrastructure has to run two stacks: a Sentry-compatible
error tracker for errors, and a log stack (Loki, Elasticsearch) for logs. That means two deployments,
two retention policies, two query languages, and — during an incident — two places to look, with
nothing tying a log line to the error that produced it. The self-hostable error trackers that exist
cover the error side only, so the split remains.

## Target Users

- **Operator** — deploys Scout, owns the database and object store, creates organisations and projects.
- **Project owner** — owns one project's settings, its ingest keys, who has access, and which decision
  model it uses.
- **Triager / developer on call** — reads grouped issues and logs, confirms how an item was categorised,
  resolves what is fixed.
- **Organisation admin** — grants and revokes member access and roles, and configures SSO.

## Jobs to Be Done

- When my service throws an error in production, I want the SDK output I already produce to land in
  my own infrastructure, so I can read the stack trace without shipping it to a vendor.
- When the same error fires a thousand times, I want it counted as one issue, so triage is one decision
  instead of a thousand.
- When an incoming payload is not obviously an error or a log, I want Scout to suggest what it is and
  let me confirm, so nothing is filed silently under the wrong kind.
- When a teammate joins, I want to give them a role on one project, so access is scoped without
  touching the deployment.
- When the decision model is slow or unreachable, I want ingest to keep accepting traffic, so no
  telemetry is lost waiting on a classifier.

## Key User Story

A developer changes one line — the DSN — in an existing service and deploys. An error fires. Within
seconds the issue appears in the project's issue list with its stack trace and a running count; the
developer opens it, sees the raw event with the SDK's breadcrumbs and release tag, accepts the
suggested categorisation, and resolves the issue.

## Goals

- **Accept SDK traffic unmodified**: an app whose DSN points at Scout delivers events with no other
  client change, including envelopes, breadcrumbs and release tags.
- **Group by identity**: repeated occurrences of the same error collapse into one issue with a count
  and first/last-seen.
- **Keep the original**: every issue resolves to the exact payload that was received.
- **Survive a classifier outage**: no accepted event is dropped or delayed because the decision model
  failed.
- **Scope access per project**: granting or revoking a person's access needs no redeploy.

## Non-Goals

- **A hosted offering** — Scout is self-hosted only.
- **Sentry read-API compatibility** — only ingestion is Sentry-compatible; tools that query the Sentry
  API cannot use Scout as a drop-in replacement.
- **Metrics and tracing** — Scout stores errors and logs, not spans or metric series.

## Success Metrics

- A fresh deployment with no prior data reaches its first stored issue within 10 minutes of starting
  the stack.
- 1,000 identical error events produce exactly one issue with count 1,000.
- Every stored issue opens to a raw event byte-identical to what the SDK sent, apart from envelope
  framing.
- With the decision model unreachable, ingest acceptance and storage are unaffected; items carry no
  suggestion rather than a wrong one.

## Capabilities

- **Sentry-compatible ingestion** — accepts Sentry envelopes (event, attachments where present), with
  breadcrumbs and release tags, per project ingest key.
- **Issue grouping** — fingerprints repeated errors into one issue with count, first seen and last seen.
- **Raw event storage** — the original payload of every event is kept in S3-compatible storage, with
  metadata and grouping in Postgres.
- **Log ingestion and search** — accepts and stores log lines and makes them searchable alongside issues.
- **Categorisation model** — a project owner connects a Jev-compatible decisions endpoint; it
  *suggests* whether an item is an issue, a log or a raw event, a triager confirms or overrides, and the
  item is stored either way.
- **Internal UI** — issue list and detail with the raw event, project management, member roles, invites,
  and access configuration.
- **Access control** — organisations containing projects, members with owner / admin / triager / viewer
  roles scoped to a project, per-project ingest keys, local accounts plus optional OIDC/SSO login.

## Non-Functional Requirements

- **Single deployment unit**: app + Postgres + S3-compatible store, runnable from one compose file.
- **Ingest never blocks on the decision model**: bounded timeout, item stored regardless of result.
- **Server-side authorisation**: every request, including ingest keys, is scoped to exactly one project.
- **Raw payloads immutable**: stored events are never rewritten; retention is configured per project.

## Technology

| Aspect | Choice | Rationale |
| --- | --- | --- |
| Backend | Rust, Actix | Sustained concurrent ingest; the stack the project is built on. |
| UI | Sycamore + Tailwind CSS | Rust-native frontend, one toolchain with the backend. |
| Metadata store | PostgreSQL | Projects, members, issue grouping and state. |
| Raw storage | S3-compatible object store (S3 / MinIO) | Payload volume kept out of the database. |
| Identity | Local accounts now, OIDC/SSO connectable | Own user machinery, SSO added without a rewrite. |
| Categorisation | Jev-compatible decisions endpoint, per project | Owner chooses the model and host; Scout sends only a summary. |
