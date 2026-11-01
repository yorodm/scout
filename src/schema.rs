//! Startup schema management.
//!
//! This module is deliberately separate from [`crate::migrate`], which is the
//! object-storage migration subcommand from feature 003.
//!
//! Responsibilities (research R2, FR-003, FR-004):
//!
//! 1. Connect to PostgreSQL with a bounded retry so a starting database is
//!    tolerated inside the compose stack.
//! 2. Refuse to run when the stored schema is *newer* than this build, because
//!    an older build writing into a newer schema is how data gets corrupted.
//! 3. Apply pending migrations under SQLx's advisory lock, so several instances
//!    starting at once apply each change exactly once.
//! 4. Report actionable errors and never leave the caller serving requests.

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

/// Migrations are embedded so the single binary carries its own schema.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// How long to keep retrying the initial connection.
const CONNECT_ATTEMPTS: u32 = 30;
const CONNECT_DELAY: Duration = Duration::from_secs(1);

/// Result of comparing the stored schema with this build.
#[derive(Debug, PartialEq, Eq)]
pub enum VersionCheck {
    /// Nothing has been applied yet; the migrator will apply everything.
    Empty,
    /// The store is behind this build; pending migrations must run.
    Behind { stored: i64, embedded: i64 },
    /// The store is exactly at this build's version.
    Current { version: i64 },
    /// The store is ahead of this build.
    Newer { stored: i64, embedded: i64 },
}

impl VersionCheck {
    /// True when serving on this store is unsafe for this build.
    pub fn is_incompatible(&self) -> bool {
        matches!(self, VersionCheck::Newer { .. })
    }
}

/// Compare the stored schema version with the highest embedded migration.
pub fn classify(stored: Option<i64>, embedded: i64) -> VersionCheck {
    match stored {
        None => VersionCheck::Empty,
        Some(stored) if stored > embedded => VersionCheck::Newer { stored, embedded },
        Some(stored) if stored < embedded => VersionCheck::Behind { stored, embedded },
        Some(stored) => VersionCheck::Current { version: stored },
    }
}

/// The highest migration version this build knows about.
pub fn highest_embedded_version() -> i64 {
    MIGRATOR
        .migrations
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0)
}

/// Connect with a bounded retry. The message names the variable to fix without
/// echoing the connection string, which contains credentials.
pub async fn connect(database_url: &str) -> Result<PgPool, String> {
    let mut last_error = String::new();
    for attempt in 1..=CONNECT_ATTEMPTS {
        match PgPoolOptions::new()
            .max_connections(16)
            .acquire_timeout(Duration::from_secs(10))
            .connect(database_url)
            .await
        {
            Ok(pool) => return Ok(pool),
            Err(error) => {
                last_error = error.to_string();
                tracing::warn!(
                    attempt,
                    attempts = CONNECT_ATTEMPTS,
                    "database not reachable yet: {error}"
                );
                tokio::time::sleep(CONNECT_DELAY).await;
            }
        }
    }
    Err(format!(
        "could not connect to the metadata store at DATABASE_URL after {CONNECT_ATTEMPTS} attempts: \
{last_error}. Check that the PostgreSQL service is running and the credentials are correct."
    ))
}

/// Read the highest applied migration version, if the migration table exists.
pub async fn stored_version(pool: &PgPool) -> Result<Option<i64>, String> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = current_schema() AND table_name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| format!("could not inspect the metadata store schema: {error}"))?;

    if !exists {
        return Ok(None);
    }

    sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
        .map_err(|error| format!("could not read the applied schema version: {error}"))
}

/// Apply pending schema changes, refusing to touch a store that is ahead of
/// this build. Returns the check that was performed so startup can log it.
pub async fn prepare(pool: &PgPool) -> Result<VersionCheck, String> {
    let embedded = highest_embedded_version();
    let stored = stored_version(pool).await?;
    let check = classify(stored, embedded);

    if let VersionCheck::Newer { stored, embedded } = check {
        return Err(format!(
            "database schema version {stored} is newer than this build ({embedded}); \
deploy a matching build or restore a backup. This build will not serve requests against a \
newer schema."
        ));
    }

    MIGRATOR.run(pool).await.map_err(|error| {
        format!(
            "could not apply database schema changes: {error}. The application will not start \
until the schema is consistent."
        )
    })?;

    Ok(check)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_covers_every_ordering() {
        assert_eq!(classify(None, 8), VersionCheck::Empty);
        assert_eq!(
            classify(Some(9), 8),
            VersionCheck::Newer {
                stored: 9,
                embedded: 8
            }
        );
        assert_eq!(
            classify(Some(6), 8),
            VersionCheck::Behind {
                stored: 6,
                embedded: 8
            }
        );
        assert_eq!(classify(Some(8), 8), VersionCheck::Current { version: 8 });
    }

    #[test]
    fn only_a_newer_store_is_incompatible() {
        assert!(!classify(None, 8).is_incompatible());
        assert!(!classify(Some(3), 8).is_incompatible());
        assert!(!classify(Some(8), 8).is_incompatible());
        assert!(classify(Some(9), 8).is_incompatible());
    }

    #[test]
    fn the_embedded_set_covers_the_shipped_migrations() {
        // 0001 through 0008 are shipped by this feature.
        assert_eq!(highest_embedded_version(), 8);
    }
}
