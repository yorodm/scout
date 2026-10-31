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
        }
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
