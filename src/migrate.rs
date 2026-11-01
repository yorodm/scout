//! Dedicated migration entry point (`scout migrate-objects`).
//!
//! Copies raw payloads and attachments from a source object store into the
//! destination configured for the runtime, verifying content and reporting
//! counts. It starts no HTTP server, ingest, workers, or retention, and it never
//! writes to the source store.

use crate::objects::{ObjectError, Objects, S3Settings};
use sha2::{Digest, Sha256};
use std::time::Instant;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// The subcommand that selects this mode.
pub const MODE: &str = "migrate-objects";

/// Bounded transfer concurrency; listing stays sequential.
const CONCURRENCY: usize = 8;

#[derive(Clone, Debug)]
pub struct MigrationSettings {
    pub source: S3Settings,
    pub destination: S3Settings,
}

impl MigrationSettings {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(&|name| std::env::var(name).ok())
    }

    /// Split out from `from_env` so tests can supply values without mutating the
    /// process environment.
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        Ok(Self {
            source: S3Settings::from_migration_source_lookup(get)?,
            destination: S3Settings::from_runtime_lookup(get)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub found: usize,
    pub copied: usize,
    pub skipped: usize,
    pub verified: usize,
    pub mismatched: usize,
    pub failed: usize,
}

impl Summary {
    pub fn exit_code(&self) -> i32 {
        if self.failed == 0 && self.found == self.verified {
            0
        } else {
            1
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Skipped,
    CopiedNew,
    CopiedOverwrite,
    Mismatch,
    Unreadable,
}

/// Entry point used by `main`. Returns the process exit code.
pub async fn run_from_env() -> Result<i32, String> {
    let settings = MigrationSettings::from_env()?;
    let source = settings.source.build()?;
    let destination = settings.destination.build()?;
    let started = Instant::now();
    let summary = run(&source, &destination).await;
    tracing::info!(
        found = summary.found,
        copied = summary.copied,
        skipped = summary.skipped,
        verified = summary.verified,
        mismatched = summary.mismatched,
        failed = summary.failed,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "migrate: summary"
    );
    Ok(summary.exit_code())
}

/// Copy every object from `source` into `destination`, returning the counts.
pub async fn run(source: &Objects, destination: &Objects) -> Summary {
    let mut summary = Summary::default();
    let objects = match source.list_objects().await {
        Ok(objects) => objects,
        Err(error) => {
            tracing::error!(error = %error, "migrate: listing failed");
            summary.failed += 1;
            return summary;
        }
    };

    let semaphore = std::sync::Arc::new(Semaphore::new(CONCURRENCY));
    let mut tasks: JoinSet<(String, Outcome)> = JoinSet::new();

    for (key, size) in objects {
        // Sentinels from a crashed startup probe are not payloads.
        if key.starts_with("__scout/") {
            continue;
        }
        summary.found += 1;
        tracing::info!(key = %key, size, "migrate: listed");
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("migration semaphore is open");
        let source = source.clone();
        let destination = destination.clone();
        let task_key = key.clone();
        tasks.spawn(async move {
            let _permit = permit;
            let outcome = copy_one(&source, &destination, &key).await;
            (task_key, outcome)
        });
        while let Some(joined) = tasks.try_join_next() {
            record_joined(joined, &mut summary);
        }
    }

    while let Some(joined) = tasks.join_next().await {
        record_joined(joined, &mut summary);
    }
    summary
}

fn record_joined(joined: Result<(String, Outcome), tokio::task::JoinError>, summary: &mut Summary) {
    match joined {
        Ok((key, outcome)) => record(summary, &key, outcome),
        Err(error) => {
            summary.failed += 1;
            tracing::error!(error = %error, "migrate: task failed");
        }
    }
}

fn record(summary: &mut Summary, key: &str, outcome: Outcome) {
    match outcome {
        Outcome::Skipped => {
            summary.skipped += 1;
            summary.verified += 1;
            tracing::info!(key = %key, outcome = "skipped_identical", "migrate: verified");
        }
        Outcome::CopiedNew => {
            summary.copied += 1;
            summary.verified += 1;
            tracing::info!(key = %key, outcome = "copied_new", "migrate: verified");
        }
        Outcome::CopiedOverwrite => {
            summary.copied += 1;
            summary.verified += 1;
            tracing::info!(key = %key, outcome = "copied_overwrite", "migrate: verified");
        }
        Outcome::Mismatch => {
            summary.failed += 1;
            summary.mismatched += 1;
            tracing::warn!(
                key = %key,
                outcome = "copy_failed",
                reason = "destination verification mismatch",
                "migrate: failed"
            );
        }
        Outcome::Unreadable => {
            summary.failed += 1;
            tracing::warn!(key = %key, outcome = "unreadable", "migrate: failed");
        }
    }
}

async fn copy_one(source: &Objects, destination: &Objects, key: &str) -> Outcome {
    let bytes = match source.get(key).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(key = %key, error = %error, "migrate: source unreadable");
            return Outcome::Unreadable;
        }
    };
    let source_hash = hash(&bytes);

    let existing_size = match destination.head_size(key).await {
        Ok(size) => size,
        Err(error) => {
            tracing::warn!(key = %key, error = %error, "migrate: destination unreadable");
            return Outcome::Unreadable;
        }
    };

    let outcome = match existing_size {
        // Same size: compare content before deciding.
        Some(size) if size == bytes.len() as u64 => match destination.get(key).await {
            Ok(existing) if hash(&existing) == source_hash => return Outcome::Skipped,
            Ok(_) => match write_over(destination, key, &bytes).await {
                Some(outcome) => outcome,
                None => return Outcome::Unreadable,
            },
            Err(error) => {
                tracing::warn!(key = %key, error = %error, "migrate: destination unreadable");
                return Outcome::Unreadable;
            }
        },
        // Different size: content cannot match.
        Some(_) => match write_over(destination, key, &bytes).await {
            Some(outcome) => outcome,
            None => return Outcome::Unreadable,
        },
        None => match destination.put_create(key, bytes.clone()).await {
            Ok(()) => Outcome::CopiedNew,
            Err(ObjectError::AlreadyExists(_)) => {
                // A concurrent run won the create; repair it if it differs.
                match write_over(destination, key, &bytes).await {
                    Some(outcome) => outcome,
                    None => return Outcome::Unreadable,
                }
            }
            Err(error) => {
                tracing::warn!(key = %key, error = %error, "migrate: write failed");
                return Outcome::Unreadable;
            }
        },
    };

    match destination.get(key).await {
        Ok(stored) if hash(&stored) == source_hash => outcome,
        Ok(_) => Outcome::Mismatch,
        Err(error) => {
            tracing::warn!(key = %key, error = %error, "migrate: verification read failed");
            Outcome::Unreadable
        }
    }
}

async fn write_over(destination: &Objects, key: &str, bytes: &[u8]) -> Option<Outcome> {
    match destination.put_overwrite(key, bytes.to_vec()).await {
        Ok(()) => Some(Outcome::CopiedOverwrite),
        Err(error) => {
            tracing::warn!(key = %key, error = %error, "migrate: overwrite failed");
            None
        }
    }
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store(entries: &[(&str, &[u8])]) -> Objects {
        let objects = Objects::memory();
        for (key, data) in entries {
            objects.put_create(key, data.to_vec()).await.unwrap();
        }
        objects
    }

    fn sorted(mut entries: Vec<(String, u64)>) -> Vec<(String, u64)> {
        entries.sort();
        entries
    }

    #[actix_web::test]
    async fn copies_every_object_and_reports_counts() {
        let source = store(&[("raw/p-a/e1", b"one"), ("attachments/p-a/e1/0", b"two")]).await;
        let destination = Objects::memory();

        let summary = run(&source, &destination).await;

        assert_eq!(
            summary,
            Summary {
                found: 2,
                copied: 2,
                skipped: 0,
                verified: 2,
                mismatched: 0,
                failed: 0,
            }
        );
        assert_eq!(summary.exit_code(), 0);
        assert_eq!(destination.get("raw/p-a/e1").await.unwrap(), b"one");
        assert_eq!(
            destination.get("attachments/p-a/e1/0").await.unwrap(),
            b"two"
        );
    }

    #[actix_web::test]
    async fn rerun_skips_identical_objects() {
        let source = store(&[("raw/p-a/e1", b"one"), ("raw/p-b/e2", b"two")]).await;
        let destination = Objects::memory();
        let _ = run(&source, &destination).await;

        let second = run(&source, &destination).await;

        assert_eq!(
            second,
            Summary {
                found: 2,
                copied: 0,
                skipped: 2,
                verified: 2,
                mismatched: 0,
                failed: 0,
            }
        );
        assert_eq!(second.exit_code(), 0);
    }

    #[actix_web::test]
    async fn differing_destination_is_overwritten_and_verified() {
        let source = store(&[("raw/p-a/e1", b"authoritative")]).await;
        let destination = store(&[("raw/p-a/e1", b"corrupted")]).await;

        let summary = run(&source, &destination).await;

        assert_eq!(summary.copied, 1);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.failed, 0);
        assert_eq!(
            destination.get("raw/p-a/e1").await.unwrap(),
            b"authoritative"
        );
    }

    #[actix_web::test]
    async fn migration_leaves_the_source_unchanged_and_preserves_the_key_set() {
        let source = store(&[
            ("raw/p-a/e1", b"source-one"),
            ("attachments/p-a/e1/0", b"source-two"),
        ])
        .await;
        let destination = store(&[("raw/p-a/e1", b"corrupted")]).await;

        let before = sorted(source.list_objects().await.unwrap());
        let summary = run(&source, &destination).await;
        assert_eq!(summary.copied, 2, "one overwrite plus one new object");
        assert_eq!(summary.failed, 0);

        assert_eq!(
            sorted(source.list_objects().await.unwrap()),
            before,
            "the migration must not add or remove source keys"
        );
        assert_eq!(
            source.get("raw/p-a/e1").await.unwrap(),
            b"source-one",
            "source bytes are authoritative and untouched"
        );
        assert_eq!(
            source.get("attachments/p-a/e1/0").await.unwrap(),
            b"source-two"
        );

        let source_keys: Vec<String> = before.iter().map(|(key, _)| key.clone()).collect();
        let destination_keys: Vec<String> = sorted(destination.list_objects().await.unwrap())
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(
            destination_keys, source_keys,
            "the destination must hold exactly the source key set"
        );

        // An unreadable source key is reported without changing the store.
        assert_eq!(
            copy_one(&source, &destination, "raw/missing").await,
            Outcome::Unreadable
        );
        assert_eq!(
            sorted(source.list_objects().await.unwrap()),
            before,
            "a failed object must leave the source untouched"
        );
    }

    #[actix_web::test]
    async fn unreadable_source_is_reported_without_aborting() {
        let source = store(&[("raw/p-a/e1", b"one")]).await;
        let destination = Objects::memory();

        let outcome = copy_one(&source, &destination, "raw/missing").await;

        assert_eq!(outcome, Outcome::Unreadable);
    }

    #[actix_web::test]
    async fn sentinel_keys_are_skipped() {
        let source = store(&[("__scout/health/abc", b"ok"), ("raw/p-a/e1", b"one")]).await;
        let destination = Objects::memory();

        let summary = run(&source, &destination).await;

        assert_eq!(summary.found, 1);
        assert!(destination
            .head_size("__scout/health/abc")
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn missing_source_variables_are_named_without_leaking_secrets() {
        let base = [
            ("SCOUT_MIGRATE_SOURCE_ENDPOINT", "http://garage:3900"),
            ("SCOUT_MIGRATE_SOURCE_BUCKET", "old-raw"),
            ("SCOUT_MIGRATE_SOURCE_REGION", "garage"),
            ("SCOUT_MIGRATE_SOURCE_ACCESS_KEY_ID", "source-key"),
            (
                "SCOUT_MIGRATE_SOURCE_SECRET_ACCESS_KEY",
                "super-secret-value",
            ),
            ("SCOUT_S3_ENDPOINT", "http://garage:3900"),
            ("SCOUT_S3_BUCKET", "scout-raw"),
            ("AWS_REGION", "garage"),
            ("AWS_ACCESS_KEY_ID", "dest-key"),
            ("AWS_SECRET_ACCESS_KEY", "dest-secret-value"),
        ];
        for (missing, _) in base
            .iter()
            .filter(|(name, _)| name.starts_with("SCOUT_MIGRATE"))
        {
            let get = |name: &str| -> Option<String> {
                base.iter()
                    .find(|(key, _)| *key == name)
                    .filter(|(key, _)| key != missing)
                    .map(|(_, value)| (*value).to_string())
            };
            let error = MigrationSettings::from_lookup(&get).expect_err("must fail");
            assert!(error.contains(missing), "{missing}: {error}");
            assert!(!error.contains("super-secret-value"), "{error}");
        }
    }

    #[test]
    fn exit_code_requires_every_object_verified() {
        let summary = Summary {
            found: 3,
            copied: 2,
            skipped: 0,
            verified: 2,
            mismatched: 1,
            failed: 1,
        };
        assert_eq!(summary.exit_code(), 1);

        let complete = Summary {
            found: 3,
            copied: 2,
            skipped: 1,
            verified: 3,
            mismatched: 0,
            failed: 0,
        };
        assert_eq!(complete.exit_code(), 0);
    }

    #[test]
    fn docs_describe_stages_without_external_tooling_or_minio_setup() {
        let readme = include_str!("../README.md");
        let architecture = include_str!("../architecture.md");
        let compose = include_str!("../docker-compose.yml");
        let readme_lower = readme.to_lowercase();

        assert!(
            readme_lower.contains("separate operational stages"),
            "README must state that migration and ingestion are separate stages"
        );
        assert!(
            readme_lower.contains("downtime"),
            "README must state that the operator schedules downtime"
        );
        assert!(
            readme_lower.contains("scout migrate-objects"),
            "README must document the migration command"
        );
        for tool in ["mc mirror", "aws s3 sync", "rclone", "s3cmd"] {
            assert!(
                !readme_lower.contains(tool),
                "README must not require external object-storage tooling ({tool})"
            );
        }

        assert!(
            !compose.to_lowercase().contains("minio"),
            "the shipped compose file must not require MinIO"
        );
        for doc in [readme, architecture] {
            let doc = doc.to_lowercase();
            assert!(
                !doc.contains("minio-init"),
                "MinIO setup must not be required"
            );
            assert!(
                !doc.contains("install minio"),
                "MinIO setup must not be required"
            );
        }
    }

    #[test]
    fn migration_settings_read_only_storage_variables() {
        use std::cell::RefCell;

        let seen = RefCell::new(Vec::new());
        let get = |name: &str| -> Option<String> {
            seen.borrow_mut().push(name.to_string());
            match name {
                "SCOUT_MIGRATE_SOURCE_ENDPOINT" => Some("http://source:9000".into()),
                "SCOUT_MIGRATE_SOURCE_BUCKET" => Some("old-raw".into()),
                "SCOUT_MIGRATE_SOURCE_ACCESS_KEY_ID" => Some("source-key".into()),
                "SCOUT_MIGRATE_SOURCE_SECRET_ACCESS_KEY" => Some("source-secret".into()),
                "SCOUT_MIGRATE_SOURCE_REGION" => Some("us-east-1".into()),
                "SCOUT_S3_ENDPOINT" => Some("http://garage:3900".into()),
                "SCOUT_S3_BUCKET" => Some("scout-raw".into()),
                "AWS_ACCESS_KEY_ID" => Some("dest-key".into()),
                "AWS_SECRET_ACCESS_KEY" => Some("dest-secret".into()),
                "AWS_REGION" => Some("garage".into()),
                _ => None,
            }
        };

        MigrationSettings::from_lookup(&get).expect("complete storage settings");
        let seen = seen.into_inner();
        assert!(
            !seen.iter().any(|name| name == "SCOUT_SELF_URL"),
            "{seen:?}"
        );
        for name in &seen {
            assert!(
                name.starts_with("SCOUT_S3_")
                    || name.starts_with("SCOUT_MIGRATE_SOURCE_")
                    || name.starts_with("AWS_"),
                "FR-017: migration settings must not read the service variable {name}"
            );
        }
    }
}
