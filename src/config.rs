use std::env;

/// Which metadata adapter the process uses. `Postgres` is the production
/// default; `Memory` exists for automated tests and explicit local development
/// (research R7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    Postgres,
    Memory,
}

impl StoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StoreKind::Postgres => "postgres",
            StoreKind::Memory => "memory",
        }
    }
}

/// Environment-backed configuration. Values not wired into the current slice are
/// kept here so later phases read one source instead of scattered `env::var`
/// calls.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub self_url: String,
    pub ingest_max_in_flight: usize,
    pub retention_days: u32,
    pub confidence_threshold: f64,
    pub session_ttl_days: i64,
    /// Hours of inactivity after which a dashboard session expires (FR-033).
    /// Zero disables the idle limit, leaving only the absolute lifetime.
    pub session_idle_hours: i64,
    pub operator_email: Option<String>,
    pub operator_password: Option<String>,
    pub database_url: Option<String>,
    /// Raw `SCOUT_STORE` value; resolved together with `database_url` by
    /// [`Config::store_kind`].
    pub store: Option<String>,
    /// How often the retention sweep runs.
    pub retention_interval_secs: u64,
    pub s3_endpoint: Option<String>,
    pub s3_bucket: Option<String>,
    /// Enables seeded demo data and the prototype demo endpoints. Off by
    /// default so a real deployment never exposes known credentials or an
    /// unauthenticated reset.
    pub demo_mode: bool,
    /// Enables the Sentry-compatible `/api/0/` surface. Off by default because it
    /// exposes project data to long-lived tokens.
    pub sentry_compat_enabled: bool,
}

impl Config {
    pub fn from_env() -> Self {
        let port = env_parse("PORT", 8080);
        let self_url =
            env::var("SCOUT_SELF_URL").unwrap_or_else(|_| format!("http://127.0.0.1:{port}"));

        Self {
            port,
            self_url,
            ingest_max_in_flight: env_parse("SCOUT_INGEST_MAX_IN_FLIGHT", 256).max(1),
            retention_days: env_parse("SCOUT_RETENTION_DAYS", 30),
            confidence_threshold: env_parse::<f64>("SCOUT_CONFIDENCE_THRESHOLD", 0.5)
                .clamp(0.0, 1.0),
            session_ttl_days: env_parse("SCOUT_SESSION_TTL_DAYS", 7),
            session_idle_hours: env_parse("SCOUT_SESSION_IDLE_HOURS", 12).max(0),
            operator_email: non_empty("SCOUT_OPERATOR_EMAIL"),
            operator_password: non_empty("SCOUT_OPERATOR_PASSWORD"),
            database_url: non_empty("DATABASE_URL"),
            store: non_empty("SCOUT_STORE"),
            retention_interval_secs: env_parse::<u64>("SCOUT_RETENTION_INTERVAL_SECONDS", 3600)
                .max(1),
            s3_endpoint: non_empty("SCOUT_S3_ENDPOINT"),
            s3_bucket: non_empty("SCOUT_S3_BUCKET"),
            demo_mode: env_flag("SCOUT_DEMO_MODE"),
            sentry_compat_enabled: env_flag("SCOUT_SENTRY_API_COMPAT_ENABLED"),
        }
    }

    /// Resolve the metadata adapter. An explicit `SCOUT_STORE` wins; otherwise
    /// the durable adapter is selected whenever `DATABASE_URL` is present.
    pub fn store_kind(&self) -> Result<StoreKind, String> {
        let requested = self
            .store
            .as_deref()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());

        match requested.as_deref() {
            Some("postgres") => {
                if self.database_url.is_some() {
                    Ok(StoreKind::Postgres)
                } else {
                    Err("SCOUT_STORE=postgres requires DATABASE_URL (for example \
postgres://scout:scout@postgres:5432/scout)"
                        .into())
                }
            }
            Some("memory") => Ok(StoreKind::Memory),
            Some(other) => Err(format!(
                "SCOUT_STORE must be `postgres` or `memory` (got `{other}`)"
            )),
            None => Ok(if self.database_url.is_some() {
                StoreKind::Postgres
            } else {
                StoreKind::Memory
            }),
        }
    }

    /// A persistent deployment must not silently acknowledge events into memory.
    pub fn validate_storage(&self) -> Result<(), String> {
        if self.store_kind()? == StoreKind::Postgres
            && (self.s3_endpoint.is_none() || self.s3_bucket.is_none())
        {
            return Err(
                "the durable metadata store is configured but object storage is not: set \
SCOUT_S3_ENDPOINT and SCOUT_S3_BUCKET (both are required for durable raw payloads)"
                    .into(),
            );
        }
        Ok(())
    }
}

fn env_parse<T>(key: &str, default: T) -> T
where
    T: std::str::FromStr,
{
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn non_empty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_flag(key: &str) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(database_url: Option<&str>, endpoint: Option<&str>, bucket: Option<&str>) -> Config {
        Config {
            port: 8080,
            self_url: "http://127.0.0.1:1".into(),
            ingest_max_in_flight: 256,
            retention_days: 30,
            confidence_threshold: 0.5,
            session_ttl_days: 7,
            session_idle_hours: 12,
            operator_email: Some("ops@example.com".into()),
            operator_password: Some("secret".into()),
            database_url: database_url.map(str::to_string),
            store: None,
            retention_interval_secs: 3600,
            s3_endpoint: endpoint.map(str::to_string),
            s3_bucket: bucket.map(str::to_string),
            demo_mode: false,
            sentry_compat_enabled: false,
        }
    }

    fn with_store(mut config: Config, store: Option<&str>) -> Config {
        config.store = store.map(str::to_string);
        config
    }

    #[test]
    fn store_kind_prefers_an_explicit_setting_and_falls_back_to_database_url() {
        // Unset: DATABASE_URL decides.
        assert_eq!(
            config(
                Some("postgres://x"),
                Some("http://objects:3900"),
                Some("raw")
            )
            .store_kind()
            .expect("durable"),
            StoreKind::Postgres
        );
        assert_eq!(
            config(None, None, None).store_kind().expect("memory"),
            StoreKind::Memory
        );

        // Explicit `memory` wins even with a database URL present.
        assert_eq!(
            with_store(config(Some("postgres://x"), None, None), Some("memory"))
                .store_kind()
                .expect("memory"),
            StoreKind::Memory
        );

        // Explicit `postgres` without a URL is a startup error naming both values.
        let error = with_store(config(None, None, None), Some("postgres"))
            .store_kind()
            .expect_err("missing DATABASE_URL must fail");
        assert!(error.contains("SCOUT_STORE=postgres"), "error: {error}");
        assert!(error.contains("DATABASE_URL"), "error: {error}");

        // Case and surrounding whitespace are tolerated.
        assert_eq!(
            with_store(config(Some("postgres://x"), None, None), Some(" MEMORY "))
                .store_kind()
                .expect("memory"),
            StoreKind::Memory
        );

        // Anything else is rejected with the accepted values.
        let error = with_store(config(None, None, None), Some("sqlite"))
            .store_kind()
            .expect_err("unknown adapter must fail");
        assert!(
            error.contains("postgres") && error.contains("memory"),
            "error: {error}"
        );
    }

    #[test]
    fn explicit_memory_needs_no_object_store() {
        let config = with_store(config(None, None, None), Some("memory"));
        assert!(config.validate_storage().is_ok());
    }

    #[test]
    fn persistent_metadata_requires_object_storage() {
        let error = config(Some("postgres://x"), None, None)
            .validate_storage()
            .expect_err("missing object storage must fail startup");
        assert!(error.contains("SCOUT_S3_ENDPOINT"));
        assert!(error.contains("SCOUT_S3_BUCKET"));

        assert!(
            config(Some("postgres://x"), Some("http://objects:3900"), None)
                .validate_storage()
                .is_err()
        );
        assert!(config(
            Some("postgres://x"),
            Some("http://objects:3900"),
            Some("raw")
        )
        .validate_storage()
        .is_ok());
        // The in-memory/demo shape needs no object store.
        assert!(config(None, None, None).validate_storage().is_ok());
    }
}
