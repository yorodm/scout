//! Operational counters (FR-025).
//!
//! Counters are plain atomics so the ingest hot path pays no extra round trip,
//! and `GET /api/metrics` renders a snapshot for the deployment operator. The
//! same numbers are emitted periodically as `INFO` lines for log-only setups.

use crate::error::ApiError;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Counters accumulated since process start.
#[derive(Debug)]
pub struct Metrics {
    started: Instant,
    ingest_accepted: AtomicU64,
    ingest_rejected_auth: AtomicU64,
    ingest_rejected_malformed: AtomicU64,
    ingest_rejected_oversized: AtomicU64,
    ingest_rejected_saturated: AtomicU64,
    ingest_rejected_storage: AtomicU64,
    ingest_rejected_other: AtomicU64,
    grouping_ok: AtomicU64,
    grouping_failed: AtomicU64,
    classifier_suggested: AtomicU64,
    classifier_no_suggestion: AtomicU64,
    classifier_failed: AtomicU64,
    retention_objects_deleted: AtomicU64,
    retention_logs_deleted: AtomicU64,
    raw_expired: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            ingest_accepted: AtomicU64::new(0),
            ingest_rejected_auth: AtomicU64::new(0),
            ingest_rejected_malformed: AtomicU64::new(0),
            ingest_rejected_oversized: AtomicU64::new(0),
            ingest_rejected_saturated: AtomicU64::new(0),
            ingest_rejected_storage: AtomicU64::new(0),
            ingest_rejected_other: AtomicU64::new(0),
            grouping_ok: AtomicU64::new(0),
            grouping_failed: AtomicU64::new(0),
            classifier_suggested: AtomicU64::new(0),
            classifier_no_suggestion: AtomicU64::new(0),
            classifier_failed: AtomicU64::new(0),
            retention_objects_deleted: AtomicU64::new(0),
            retention_logs_deleted: AtomicU64::new(0),
            raw_expired: AtomicU64::new(0),
        }
    }

    /// Record accepted items (events plus log lines) in one call.
    pub fn accepted(&self, count: u64) {
        self.ingest_accepted.fetch_add(count, Ordering::Relaxed);
    }

    /// Classify an ingest rejection by the response it produces.
    pub fn rejected(&self, error: &ApiError) {
        let counter = match error {
            ApiError::Unauthorized(_) | ApiError::Forbidden(_) => &self.ingest_rejected_auth,
            ApiError::BadRequest(_) | ApiError::NotFound(_) | ApiError::Conflict(_) => {
                &self.ingest_rejected_malformed
            }
            ApiError::PayloadTooLarge(_) => &self.ingest_rejected_oversized,
            ApiError::TooManyRequests { .. } => &self.ingest_rejected_saturated,
            ApiError::ServiceUnavailable(_) => &self.ingest_rejected_storage,
            ApiError::Gone(_) => &self.ingest_rejected_other,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn grouped(&self, ok: bool) {
        let counter = if ok {
            &self.grouping_ok
        } else {
            &self.grouping_failed
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn suggested(&self, outcome: SuggestionOutcome) {
        let counter = match outcome {
            SuggestionOutcome::Suggested => &self.classifier_suggested,
            SuggestionOutcome::NoSuggestion => &self.classifier_no_suggestion,
            SuggestionOutcome::Failed => &self.classifier_failed,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn retention(&self, objects: u64, logs: u64) {
        self.retention_objects_deleted
            .fetch_add(objects, Ordering::Relaxed);
        self.retention_logs_deleted
            .fetch_add(logs, Ordering::Relaxed);
        self.raw_expired.fetch_add(objects, Ordering::Relaxed);
    }

    /// JSON document served by `GET /api/metrics`.
    pub fn snapshot(&self, store_kind: &str) -> Value {
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        json!({
            "uptime_seconds": self.started.elapsed().as_secs(),
            "store": store_kind,
            "counters": {
                "ingest_accepted": read(&self.ingest_accepted),
                "ingest_rejected": {
                    "auth": read(&self.ingest_rejected_auth),
                    "malformed": read(&self.ingest_rejected_malformed),
                    "oversized": read(&self.ingest_rejected_oversized),
                    "saturated": read(&self.ingest_rejected_saturated),
                    "storage": read(&self.ingest_rejected_storage),
                    "other": read(&self.ingest_rejected_other),
                },
                "grouping_ok": read(&self.grouping_ok),
                "grouping_failed": read(&self.grouping_failed),
                "classifier_suggested": read(&self.classifier_suggested),
                "classifier_no_suggestion": read(&self.classifier_no_suggestion),
                "classifier_failed": read(&self.classifier_failed),
                "retention_objects_deleted": read(&self.retention_objects_deleted),
                "retention_logs_deleted": read(&self.retention_logs_deleted),
                "raw_expired": read(&self.raw_expired),
            }
        })
    }

    /// One-line summary for periodic `INFO` logging.
    pub fn log_line(&self, store_kind: &str) -> String {
        let snapshot = self.snapshot(store_kind);
        format!("operational counters: {snapshot}")
    }
}

/// How a classifier attempt ended (constitution I: failure is never fatal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionOutcome {
    Suggested,
    NoSuggestion,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejections_are_classified_by_response() {
        let metrics = Metrics::new();
        metrics.rejected(&ApiError::Unauthorized("x".into()));
        metrics.rejected(&ApiError::Forbidden("x".into()));
        metrics.rejected(&ApiError::BadRequest("x".into()));
        metrics.rejected(&ApiError::PayloadTooLarge("x".into()));
        metrics.rejected(&ApiError::TooManyRequests {
            message: "x".into(),
            retry_after_secs: 1,
        });
        metrics.rejected(&ApiError::ServiceUnavailable("x".into()));

        let snapshot = metrics.snapshot("memory");
        assert_eq!(snapshot["counters"]["ingest_rejected"]["auth"], 2);
        assert_eq!(snapshot["counters"]["ingest_rejected"]["malformed"], 1);
        assert_eq!(snapshot["counters"]["ingest_rejected"]["oversized"], 1);
        assert_eq!(snapshot["counters"]["ingest_rejected"]["saturated"], 1);
        assert_eq!(snapshot["counters"]["ingest_rejected"]["storage"], 1);
        assert_eq!(snapshot["store"], "memory");
    }

    #[test]
    fn accepted_grouping_classifier_and_retention_counters_accumulate() {
        let metrics = Metrics::new();
        metrics.accepted(3);
        metrics.grouped(true);
        metrics.grouped(false);
        metrics.suggested(SuggestionOutcome::Suggested);
        metrics.suggested(SuggestionOutcome::NoSuggestion);
        metrics.suggested(SuggestionOutcome::Failed);
        metrics.retention(4, 9);

        let snapshot = metrics.snapshot("postgres");
        assert_eq!(snapshot["counters"]["ingest_accepted"], 3);
        assert_eq!(snapshot["counters"]["grouping_ok"], 1);
        assert_eq!(snapshot["counters"]["grouping_failed"], 1);
        assert_eq!(snapshot["counters"]["classifier_suggested"], 1);
        assert_eq!(snapshot["counters"]["classifier_no_suggestion"], 1);
        assert_eq!(snapshot["counters"]["classifier_failed"], 1);
        assert_eq!(snapshot["counters"]["retention_objects_deleted"], 4);
        assert_eq!(snapshot["counters"]["retention_logs_deleted"], 9);
        assert_eq!(snapshot["counters"]["raw_expired"], 4);
        assert!(metrics
            .log_line("postgres")
            .contains("operational counters"));
    }
}
