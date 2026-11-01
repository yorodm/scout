use crate::metrics::Metrics;
use crate::objects::Objects;
use crate::store::Store;
use crate::worker::Job;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{mpsc::UnboundedSender, RwLock, Semaphore};

pub struct AppState {
    pub store: Arc<RwLock<Store>>,
    pub jobs: UnboundedSender<Job>,
    /// Used to point the demo project at the built-in mock decisions endpoint.
    pub self_url: String,
    /// Global in-flight bound for ingest requests. Saturation returns 429.
    pub ingest_gate: Arc<Semaphore>,
    /// Lifetime of an issued session token.
    pub session_ttl_days: i64,
    /// Defaults applied when a project is created through the API.
    pub retention_days: u32,
    pub confidence_threshold: f64,
    /// Immutable raw payloads (write-once). Retention is the only deleter.
    pub objects: Arc<Objects>,
    /// Whether the metadata store can accept writes. `503` is returned while
    /// this is false; the PostgreSQL adapter drives it from its pool health and
    /// the in-crate tests flip it to exercise the failure path.
    pub storage_ready: Arc<AtomicBool>,
    /// Whether the Sentry-compatible `/api/0/` surface is enabled.
    pub compat_enabled: bool,
    /// Operational counters (FR-025).
    pub metrics: Arc<Metrics>,
    /// Which metadata adapter is actually serving requests: `memory` until the
    /// PostgreSQL adapter is wired in, `postgres` after that.
    pub store_kind: &'static str,
}

impl AppState {
    pub fn storage_ready(&self) -> bool {
        self.storage_ready
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Fail ingest before anything is written when the store is unavailable.
    pub fn ensure_storage(&self) -> Result<(), crate::error::ApiError> {
        if self.storage_ready() {
            Ok(())
        } else {
            Err(crate::error::ApiError::ServiceUnavailable(
                "metadata storage is unavailable; retry shortly".into(),
            ))
        }
    }
}
