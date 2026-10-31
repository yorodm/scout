//! Write-once object storage for immutable raw payloads and attachments.
//!
//! The metadata store keeps object *keys*; this module owns the bytes. Writes use
//! `PutMode::Create`, so no code path can overwrite an existing payload
//! (constitution Principle II). Retention is the only path that deletes.

use crate::config::Config;
use crate::error::ApiError;
use object_store::aws::AmazonS3Builder;
use object_store::memory::InMemory;
use object_store::{ObjectStore, PutMode, PutOptions, PutPayload};
use std::fmt;
use std::sync::Arc;

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

pub struct Objects {
    inner: Arc<dyn ObjectStore>,
    backend: &'static str,
}

impl Objects {
    /// In-memory backend used by tests and by single-process runs without S3.
    pub fn memory() -> Self {
        Self {
            inner: Arc::new(InMemory::new()),
            backend: "memory",
        }
    }

    /// Build an S3/MinIO backend when `SCOUT_S3_ENDPOINT` and `SCOUT_S3_BUCKET`
    /// are set, otherwise fall back to memory.
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let (Some(endpoint), Some(bucket)) = (&config.s3_endpoint, &config.s3_bucket) else {
            return Ok(Self::memory());
        };
        let store = AmazonS3Builder::from_env()
            .with_bucket_name(bucket.clone())
            .with_endpoint(endpoint.clone())
            .with_allow_http(endpoint.starts_with("http://"))
            .build()
            .map_err(|e| format!("invalid object-store configuration: {e}"))?;
        Ok(Self {
            inner: Arc::new(store),
            backend: "s3",
        })
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

    /// Cheap reachability probe for `/api/health`. A `404` still proves the
    /// backend answered.
    pub async fn healthy(&self) -> bool {
        match self.inner.head(&HEALTH_KEY.into()).await {
            Ok(_) | Err(object_store::Error::NotFound { .. }) => true,
            Err(_) => false,
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
}
