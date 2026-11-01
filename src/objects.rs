//! Write-once object storage for immutable raw payloads and attachments.
//!
//! The metadata store keeps object *keys*; this module owns the bytes. Writes use
//! `PutMode::Create`, so no code path can overwrite an existing payload
//! (constitution Principle II). Retention is the only path that deletes.

use crate::config::Config;
use crate::error::ApiError;
use futures_util::StreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::memory::InMemory;
use object_store::{ClientOptions, ObjectStore, PutMode, PutOptions, PutPayload, RetryConfig};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// A sentinel key used to probe reachability without touching real payloads.
const HEALTH_KEY: &str = "__scout/health";

#[derive(Debug)]
pub enum ObjectError {
    /// The key already exists. Raw payloads are write-once, so this is a bug or
    /// a replay that must not overwrite authoritative data.
    AlreadyExists(String),
    /// The key does not exist (or its retention window expired it).
    NotFound(String),
    /// The backend could not be reached.
    Unavailable(String),
}

impl fmt::Display for ObjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ObjectError::AlreadyExists(message)
            | ObjectError::NotFound(message)
            | ObjectError::Unavailable(message) => write!(f, "{message}"),
        }
    }
}

impl From<ObjectError> for ApiError {
    fn from(value: ObjectError) -> Self {
        match value {
            ObjectError::AlreadyExists(message) => ApiError::BadRequest(message),
            ObjectError::NotFound(message) => ApiError::NotFound(message),
            ObjectError::Unavailable(message) => ApiError::ServiceUnavailable(message),
        }
    }
}

/// The key layout is deterministic so retention can find a project's payloads.
pub fn raw_key(project_id: uuid::Uuid, event_id: uuid::Uuid) -> String {
    format!("raw/{project_id}/{event_id}")
}

/// Attachments are write-once too, keyed per event and position.
pub fn attachment_key(project_id: uuid::Uuid, event_id: uuid::Uuid, index: usize) -> String {
    format!("attachments/{project_id}/{event_id}/{index}")
}

/// Explicit S3 connection settings.
///
/// Kept separate from `Config` so the migration mode can address a source and a
/// destination store with the same code the server uses.
#[derive(Clone, Debug)]
pub struct S3Settings {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
}

impl S3Settings {
    pub fn new(
        endpoint: String,
        bucket: String,
        region: String,
        access_key: String,
        secret_key: String,
    ) -> Result<Self, String> {
        let settings = Self {
            endpoint,
            bucket,
            region,
            access_key,
            secret_key,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("endpoint", &self.endpoint),
            ("bucket", &self.bucket),
            ("region", &self.region),
            ("access key", &self.access_key),
            ("secret key", &self.secret_key),
        ] {
            if value.trim().is_empty() {
                return Err(format!("object storage {name} must be set and non-empty"));
            }
        }
        if !(self.endpoint.starts_with("http://") || self.endpoint.starts_with("https://")) {
            return Err("object storage endpoint must include http:// or https://".into());
        }
        Ok(())
    }

    /// Destination settings for the migration mode.
    pub fn from_runtime_lookup(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        Self::from_runtime_values(
            required(get, "SCOUT_S3_ENDPOINT")?,
            required(get, "SCOUT_S3_BUCKET")?,
            get,
        )
    }

    /// Destination settings from the values the runtime `Config` already holds.
    pub fn from_runtime_values(
        endpoint: String,
        bucket: String,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let region = get("AWS_REGION")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "us-east-1".to_string());
        Self::new(
            endpoint,
            bucket,
            region,
            required(get, "AWS_ACCESS_KEY_ID")?,
            required(get, "AWS_SECRET_ACCESS_KEY")?,
        )
    }

    /// Source settings for the migration mode.
    pub fn from_migration_source_lookup(
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        Self::new(
            required(get, "SCOUT_MIGRATE_SOURCE_ENDPOINT")?,
            required(get, "SCOUT_MIGRATE_SOURCE_BUCKET")?,
            required(get, "SCOUT_MIGRATE_SOURCE_REGION")?,
            required(get, "SCOUT_MIGRATE_SOURCE_ACCESS_KEY_ID")?,
            required(get, "SCOUT_MIGRATE_SOURCE_SECRET_ACCESS_KEY")?,
        )
    }

    /// Build the object store described by these settings.
    pub fn build(&self) -> Result<Objects, String> {
        self.validate()?;
        let retry = RetryConfig {
            max_retries: 2,
            retry_timeout: Duration::from_secs(20),
            ..RetryConfig::default()
        };
        let client = ClientOptions::new()
            .with_connect_timeout(Duration::from_secs(5))
            .with_timeout(Duration::from_secs(10));
        let store = AmazonS3Builder::new()
            .with_bucket_name(self.bucket.clone())
            .with_endpoint(self.endpoint.clone())
            .with_region(self.region.clone())
            .with_access_key_id(self.access_key.clone())
            .with_secret_access_key(self.secret_key.clone())
            .with_allow_http(self.endpoint.starts_with("http://"))
            .with_virtual_hosted_style_request(false)
            .with_client_options(client)
            .with_retry(retry)
            .build()
            .map_err(|error| format!("invalid object-store configuration: {error}"))?;
        Ok(Objects {
            inner: Arc::new(store),
            backend: "s3",
            target: Some((self.endpoint.clone(), self.bucket.clone())),
        })
    }
}

fn required(get: &dyn Fn(&str) -> Option<String>, name: &str) -> Result<String, String> {
    get(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} must be set and non-empty"))
}

#[derive(Clone)]
pub struct Objects {
    inner: Arc<dyn ObjectStore>,
    backend: &'static str,
    /// Endpoint and bucket, kept only so startup-probe failures can name them.
    /// `None` for the in-memory backend.
    target: Option<(String, String)>,
}

impl Objects {
    /// In-memory backend used by tests and by single-process runs without S3.
    pub fn memory() -> Self {
        Self {
            inner: Arc::new(InMemory::new()),
            backend: "memory",
            target: None,
        }
    }

    /// Build an S3 backend when `SCOUT_S3_ENDPOINT` and `SCOUT_S3_BUCKET` are set,
    /// otherwise fall back to memory. Credentials and region come from the
    /// environment (`AWS_*`), as before.
    pub fn from_config(config: &Config) -> Result<Self, String> {
        match (&config.s3_endpoint, &config.s3_bucket) {
            (Some(endpoint), Some(bucket)) => {
                S3Settings::from_runtime_values(endpoint.clone(), bucket.clone(), &|name| {
                    std::env::var(name).ok()
                })?
                .build()
            }
            _ => Ok(Self::memory()),
        }
    }

    pub fn backend(&self) -> &'static str {
        self.backend
    }

    /// Store bytes under `key`, failing if the key already exists.
    pub async fn put_create(&self, key: &str, data: Vec<u8>) -> Result<(), ObjectError> {
        let options = PutOptions {
            mode: PutMode::Create,
            ..PutOptions::default()
        };
        self.inner
            .put_opts(&key.into(), PutPayload::from(data), options)
            .await
            .map(|_| ())
            .map_err(|e| map_error(key, e))
    }

    pub async fn get(&self, key: &str) -> Result<Vec<u8>, ObjectError> {
        let result = self
            .inner
            .get(&key.into())
            .await
            .map_err(|e| map_error(key, e))?;
        result
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| ObjectError::Unavailable(format!("read {key} failed: {e}")))
    }

    pub async fn delete(&self, key: &str) -> Result<(), ObjectError> {
        self.inner
            .delete(&key.into())
            .await
            .map_err(|e| map_error(key, e))
    }

    /// Every object in the store as `(key, size)`. Used by the migration mode.
    pub async fn list_objects(&self) -> Result<Vec<(String, u64)>, ObjectError> {
        let mut stream = self.inner.list(None);
        let mut objects = Vec::new();
        while let Some(item) = stream.next().await {
            let meta = item.map_err(|error| {
                ObjectError::Unavailable(format!("listing objects failed: {error}"))
            })?;
            objects.push((meta.location.to_string(), meta.size));
        }
        Ok(objects)
    }

    /// Size of an object, or `None` when the key does not exist.
    pub async fn head_size(&self, key: &str) -> Result<Option<u64>, ObjectError> {
        match self.inner.head(&key.into()).await {
            Ok(meta) => Ok(Some(meta.size)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(map_error(key, error)),
        }
    }

    /// Overwrite an object. Only the migration mode uses this; the request path
    /// stays write-once (`put_create`).
    pub async fn put_overwrite(&self, key: &str, data: Vec<u8>) -> Result<(), ObjectError> {
        let options = PutOptions {
            mode: PutMode::Overwrite,
            ..PutOptions::default()
        };
        self.inner
            .put_opts(&key.into(), PutPayload::from(data), options)
            .await
            .map(|_| ())
            .map_err(|e| map_error(key, e))
    }

    /// Prove the backend accepts writes before the server starts. Uses a unique
    /// sentinel key and removes it immediately; memory backends always pass.
    pub async fn verify_writable(&self) -> Result<(), ObjectError> {
        if self.backend == "memory" {
            return Ok(());
        }
        let key = format!("{HEALTH_KEY}/{}", uuid::Uuid::new_v4());
        self.put_create(&key, b"ok".to_vec())
            .await
            .map_err(|error| self.probe_error("write", error))?;
        self.delete(&key)
            .await
            .map_err(|error| self.probe_error("delete", error))
    }

    /// Add the endpoint, bucket, and a likely cause to a probe failure so a
    /// misconfigured deployment fails with something an operator can act on.
    fn probe_error(&self, step: &str, error: ObjectError) -> ObjectError {
        let (endpoint, bucket) = match &self.target {
            Some((endpoint, bucket)) => (endpoint.as_str(), bucket.as_str()),
            None => ("<unconfigured>", "<unconfigured>"),
        };
        let message = format!(
            "object storage {step} check failed for bucket {bucket} at {endpoint}: {}. {}",
            cause(&error),
            remedy(&error)
        );
        match error {
            ObjectError::AlreadyExists(_) => ObjectError::AlreadyExists(message),
            ObjectError::NotFound(_) => ObjectError::NotFound(message),
            ObjectError::Unavailable(_) => ObjectError::Unavailable(message),
        }
    }

    /// Cheap reachability probe for `/api/health`. A `404` still proves the
    /// backend answered.
    pub async fn healthy(&self) -> bool {
        match self.inner.head(&HEALTH_KEY.into()).await {
            Ok(_) | Err(object_store::Error::NotFound { .. }) => true,
            Err(_) => false,
        }
    }
}

fn cause(error: &ObjectError) -> &str {
    match error {
        ObjectError::AlreadyExists(message)
        | ObjectError::NotFound(message)
        | ObjectError::Unavailable(message) => message,
    }
}

/// Classify the failure from what the store told us. The variants are the
/// contract; the string markers only refine the remedy for a transport error
/// that a provider answered with an authorisation status.
fn remedy(error: &ObjectError) -> &'static str {
    match error {
        ObjectError::NotFound(_) => {
            "Create the bucket and grant the access key read, write, and delete permission."
        }
        ObjectError::Unavailable(message) => {
            let lower = message.to_ascii_lowercase();
            let rejected = [
                "access denied",
                "forbidden",
                "invalidaccesskeyid",
                "signaturedoesnotmatch",
                "credential",
                "status 401",
                "status 403",
            ]
            .iter()
            .any(|marker| lower.contains(marker));
            if rejected {
                "The object store rejected the credentials; check the access key and secret."
            } else {
                "Check that the endpoint is reachable, that the object store is running, and that the bucket exists."
            }
        }
        ObjectError::AlreadyExists(_) => {
            "Retry the check; the probe key unexpectedly already exists."
        }
    }
}

fn map_error(key: &str, error: object_store::Error) -> ObjectError {
    match error {
        object_store::Error::AlreadyExists { .. } => {
            ObjectError::AlreadyExists(format!("object {key} already exists and is write-once"))
        }
        object_store::Error::NotFound { .. } => {
            ObjectError::NotFound(format!("object {key} is not available"))
        }
        other => ObjectError::Unavailable(format!("object store unavailable: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[actix_web::test]
    async fn write_once_get_and_delete() {
        let objects = Objects::memory();
        let key = raw_key(uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        objects.put_create(&key, b"first".to_vec()).await.unwrap();
        assert_eq!(objects.get(&key).await.unwrap(), b"first");

        // Constitution Principle II: a second write to the same key must fail.
        match objects.put_create(&key, b"second".to_vec()).await {
            Err(ObjectError::AlreadyExists(_)) => {}
            other => panic!("expected a write-once conflict, got {other:?}"),
        }
        assert_eq!(objects.get(&key).await.unwrap(), b"first");

        objects.delete(&key).await.unwrap();
        assert!(matches!(
            objects.get(&key).await,
            Err(ObjectError::NotFound(_))
        ));
        assert!(objects.healthy().await);
    }

    fn lookup<'a>(entries: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            entries
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn settings_require_every_value_and_a_scheme() {
        assert!(S3Settings::new(
            "http://garage:3900".into(),
            "bucket".into(),
            "garage".into(),
            "key".into(),
            "secret".into(),
        )
        .is_ok());

        assert!(S3Settings::new(
            "http://garage:3900".into(),
            "bucket".into(),
            "garage".into(),
            "key".into(),
            "  ".into(),
        )
        .is_err());

        assert!(S3Settings::new(
            "garage:3900".into(),
            "bucket".into(),
            "garage".into(),
            "key".into(),
            "secret".into(),
        )
        .is_err());
    }

    #[test]
    fn runtime_settings_read_aws_environment() {
        let get = lookup(&[
            ("SCOUT_S3_ENDPOINT", "http://garage:3900"),
            ("SCOUT_S3_BUCKET", "scout-raw"),
            ("AWS_REGION", "garage"),
            ("AWS_ACCESS_KEY_ID", "key-id"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
        ]);
        let settings = S3Settings::from_runtime_lookup(&get).expect("settings");
        assert_eq!(settings.region, "garage");
        assert_eq!(settings.bucket, "scout-raw");

        let missing = lookup(&[("SCOUT_S3_ENDPOINT", "http://garage:3900")]);
        let error = S3Settings::from_runtime_lookup(&missing).expect_err("must fail");
        assert!(error.contains("SCOUT_S3_BUCKET"), "{error}");
    }

    #[actix_web::test]
    async fn memory_helpers_list_head_overwrite_and_verify() {
        let objects = Objects::memory();
        objects.verify_writable().await.expect("memory is writable");
        objects
            .put_create("raw/p/e", b"one".to_vec())
            .await
            .unwrap();
        assert_eq!(objects.head_size("raw/p/e").await.unwrap(), Some(3));
        assert_eq!(objects.head_size("missing").await.unwrap(), None);
        assert_eq!(
            objects.list_objects().await.unwrap(),
            vec![("raw/p/e".to_string(), 3)]
        );
        objects
            .put_overwrite("raw/p/e", b"two".to_vec())
            .await
            .unwrap();
        assert_eq!(objects.get("raw/p/e").await.unwrap(), b"two");
    }

    fn closed_endpoint() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("loopback address").port();
        drop(listener);
        format!("http://127.0.0.1:{port}")
    }

    #[actix_web::test]
    async fn probe_failure_names_endpoint_and_bucket_without_the_secret() {
        let endpoint = closed_endpoint();
        let objects = S3Settings::new(
            endpoint.clone(),
            "missing-bucket".into(),
            "garage".into(),
            "probe-key".into(),
            "super-secret-value".into(),
        )
        .expect("settings")
        .build()
        .expect("build store");

        let started = std::time::Instant::now();
        let error = objects
            .verify_writable()
            .await
            .expect_err("a closed endpoint must fail the probe");
        let message = error.to_string();

        assert!(message.contains(&endpoint), "{message}");
        assert!(message.contains("missing-bucket"), "{message}");
        assert!(!message.contains("super-secret-value"), "{message}");
        assert!(message.contains("reachable"), "{message}");
        assert!(
            started.elapsed() <= Duration::from_secs(30),
            "the probe must fail within 30 s, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn probe_messages_distinguish_missing_bucket_and_rejected_credentials() {
        let endpoint = "https://s3.example.test";
        let objects = S3Settings::new(
            endpoint.into(),
            "scout-raw".into(),
            "garage".into(),
            "probe-key".into(),
            "super-secret-value".into(),
        )
        .expect("settings")
        .build()
        .expect("build store");

        // A provider answers NoSuchBucket with 404, which object_store maps to
        // NotFound; the remedy must name the bucket, not the endpoint's health.
        let missing = objects
            .probe_error("write", ObjectError::NotFound("object not found".into()))
            .to_string();
        assert!(missing.contains(endpoint), "{missing}");
        assert!(missing.contains("scout-raw"), "{missing}");
        assert!(missing.contains("Create the bucket"), "{missing}");
        assert!(!missing.contains("super-secret-value"), "{missing}");

        // object_store surfaces a 403 as PermissionDenied whose text carries the
        // status line, so the credentials marker still classifies correctly.
        let rejected = objects
            .probe_error(
                "write",
                ObjectError::Unavailable(
                    "object store unavailable: The operation lacked the necessary privileges \
                     for path __scout/health: Server returned non-2xx status code: 403 Forbidden: \
                     AccessDenied"
                        .into(),
                ),
            )
            .to_string();
        assert!(rejected.contains(endpoint), "{rejected}");
        assert!(rejected.contains("scout-raw"), "{rejected}");
        assert!(rejected.contains("rejected the credentials"), "{rejected}");
        assert!(!rejected.contains("super-secret-value"), "{rejected}");
    }

    #[test]
    fn non_garage_endpoint_builds_from_configuration_alone() {
        // Provider neutrality: only endpoint, region, bucket, and credentials
        // decide how the store is built; no code path inspects the provider name.
        let settings = S3Settings::new(
            "https://s3.eu-central-1.amazonaws.com".into(),
            "scout-raw".into(),
            "eu-central-1".into(),
            "AKIAEXAMPLE".into(),
            "secret".into(),
        )
        .expect("settings");
        assert_eq!(settings.region, "eu-central-1");

        let objects = settings.build().expect("non-Garage endpoints build");
        assert_eq!(objects.backend(), "s3");
        assert_eq!(Objects::memory().backend(), "memory");
    }
}
