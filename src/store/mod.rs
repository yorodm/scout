//! Metadata storage seam.
//!
//! The product ships with two adapters behind one repository surface:
//!
//! - [`memory::MemoryStore`] — the in-process maps used by automated tests and
//!   by an explicit `SCOUT_STORE=memory` local-development run.
//! - `postgres::PostgresStore` — the durable, authoritative store.
//!
//! `Store` (defined here) is the type handlers, ingest, the worker, and the
//! Sentry compatibility layer talk to. Keeping one surface is what prevents the
//! test path and the durable path from drifting apart.

pub mod memory;
pub mod postgres;

pub use memory::*;
