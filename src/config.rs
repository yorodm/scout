use std::env;

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
    pub operator_email: Option<String>,
    pub operator_password: Option<String>,
    pub database_url: Option<String>,
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
            operator_email: non_empty("SCOUT_OPERATOR_EMAIL"),
            operator_password: non_empty("SCOUT_OPERATOR_PASSWORD"),
            database_url: non_empty("DATABASE_URL"),
            s3_endpoint: non_empty("SCOUT_S3_ENDPOINT"),
            s3_bucket: non_empty("SCOUT_S3_BUCKET"),
            demo_mode: env_flag("SCOUT_DEMO_MODE"),
            sentry_compat_enabled: env_flag("SCOUT_SENTRY_API_COMPAT_ENABLED"),
        }
    }

    /// A persistent deployment must not silently acknowledge events into memory.
    pub fn validate_storage(&self) -> Result<(), String> {
        if self.database_url.is_some() && (self.s3_endpoint.is_none() || self.s3_bucket.is_none()) {
            return Err(
                "DATABASE_URL is set but object storage is not configured: set \
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
            operator_email: Some("ops@example.com".into()),
            operator_password: Some("secret".into()),
            database_url: database_url.map(str::to_string),
            s3_endpoint: endpoint.map(str::to_string),
            s3_bucket: bucket.map(str::to_string),
            demo_mode: false,
            sentry_compat_enabled: false,
        }
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
