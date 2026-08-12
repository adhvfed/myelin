use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use myelin_config::S3Config;
use myelin_tenancy::TenantId;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

use crate::blob::{
    BlobDependencyError, BlobError, BlobMeta, BlobStore, ContentHash, HashAlgo, Result,
};

#[derive(Clone)]
pub struct S3BlobStore {
    client: Client,
    bucket: String,
    rt: tokio::runtime::Handle,
    config_valid: bool,
    readiness: Arc<Mutex<S3ReadinessState>>,
}

const READINESS_TENANT: &str = ".myelin-readiness";
const READINESS_MARKER_BYTES: &[u8] = b"myelin-blob-read-write-v1";
const HEALTHY_PROBE_INTERVAL: Duration = Duration::from_secs(30);
const UNHEALTHY_PROBE_INTERVAL: Duration = Duration::from_secs(1);

async fn verify_readiness_marker(
    body: ByteStream,
    content_length: Option<i64>,
) -> std::result::Result<(), S3ReadinessError> {
    if content_length.is_some_and(|length| length != READINESS_MARKER_BYTES.len() as i64) {
        return Err(S3ReadinessError::Transient);
    }
    let mut reader = body
        .into_async_read()
        .take((READINESS_MARKER_BYTES.len() + 1) as u64);
    let mut bytes = Vec::with_capacity(READINESS_MARKER_BYTES.len() + 1);
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| S3ReadinessError::Transient)?;
    if bytes != READINESS_MARKER_BYTES {
        return Err(S3ReadinessError::Transient);
    }
    Ok(())
}

pub type S3ReadinessError = BlobDependencyError;

#[derive(Clone, Copy)]
struct S3ReadinessState {
    last_error: Option<S3ReadinessError>,
    next_probe: Instant,
}

impl S3BlobStore {
    pub fn connect(cfg: &S3Config, rt: tokio::runtime::Handle) -> S3BlobStore {
        let creds = aws_sdk_s3::config::Credentials::new(
            cfg.access_key.clone(),
            cfg.secret_key.clone(),
            None,
            None,
            "myelin-s3blobstore",
        );
        let conf = aws_sdk_s3::config::Builder::new()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(cfg.region.clone()))
            .endpoint_url(cfg.endpoint.clone())
            .force_path_style(cfg.force_path_style)
            .credentials_provider(creds)
            .build();
        S3BlobStore {
            client: Client::from_conf(conf),
            bucket: cfg.bucket.clone(),
            rt,
            config_valid: s3_config_is_valid(cfg),
            readiness: Arc::new(Mutex::new(S3ReadinessState {
                last_error: Some(S3ReadinessError::Transient),
                next_probe: Instant::now(),
            })),
        }
    }

    pub fn preflight(&self) -> std::result::Result<(), S3ReadinessError> {
        if !self.config_valid {
            let result = Err(S3ReadinessError::PermanentConfig);
            self.record_preflight(result);
            return result;
        }
        let marker_key = Self::key_path(
            &TenantId(READINESS_TENANT.into()),
            &ContentHash::blake3(READINESS_MARKER_BYTES),
        );
        let result = self.block(async {
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(&marker_key)
                .body(ByteStream::from_static(READINESS_MARKER_BYTES))
                .send()
                .await
                .map_err(|error| classify_sdk_error(&error, PreflightOperation::Put))?;
            let output = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(&marker_key)
                .send()
                .await
                .map_err(|error| classify_sdk_error(&error, PreflightOperation::Get))?;
            let content_length = output.content_length();
            verify_readiness_marker(output.body, content_length).await
        });
        self.record_preflight(result);
        result
    }

    pub fn readiness(&self) -> std::result::Result<(), S3ReadinessError> {
        let state = *self
            .readiness
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(error @ (S3ReadinessError::PermanentConfig | S3ReadinessError::PermanentAuth)) =
            state.last_error
        {
            return Err(error);
        }
        if Instant::now() < state.next_probe {
            return state.last_error.map_or(Ok(()), Err);
        }
        self.preflight()
    }

    fn record_preflight(&self, result: std::result::Result<(), S3ReadinessError>) {
        let mut state = self
            .readiness
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.last_error = result.err();
        state.next_probe = Instant::now()
            + if state.last_error.is_none() {
                HEALTHY_PROBE_INTERVAL
            } else {
                UNHEALTHY_PROBE_INTERVAL
            };
    }

    fn mark_runtime_failure(&self, kind: S3ReadinessError) {
        let mut state = self
            .readiness
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.last_error = Some(kind);
        state.next_probe = Instant::now();
    }

    fn key_path(tenant: &TenantId, hash: &ContentHash) -> String {
        let digest = &hash.digest_hex;
        let (fan, rest) = if digest.len() >= 2 {
            digest.split_at(2)
        } else {
            (digest.as_str(), "")
        };
        format!("{}/{}/{}/{}", tenant.0, hash.algo.tag(), fan, rest)
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl BlobStore for S3BlobStore {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash> {
        let hash = ContentHash::blake3(bytes);
        let key = Self::key_path(tenant, &hash);
        if let Err(error) = self.block(
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(&key)
                .body(ByteStream::from(bytes.to_vec()))
                .send(),
        ) {
            let kind = classify_sdk_error(&error, PreflightOperation::Put);
            self.mark_runtime_failure(kind);
            return Err(BlobError::Backend(kind));
        }
        Ok(hash)
    }

    fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>> {
        let key = Self::key_path(tenant, hash);
        let out = self
            .block(
                self.client
                    .get_object()
                    .bucket(&self.bucket)
                    .key(&key)
                    .send(),
            )
            .map_err(|e| {
                if is_no_such_key(&e) {
                    BlobError::NotFound {
                        tenant: tenant.clone(),
                        hash: hash.clone(),
                    }
                } else {
                    let kind = classify_sdk_error(&e, PreflightOperation::Get);
                    self.mark_runtime_failure(kind);
                    BlobError::Backend(kind)
                }
            })?;
        let bytes = self
            .block(out.body.collect())
            .map_err(|_| {
                self.mark_runtime_failure(S3ReadinessError::Transient);
                BlobError::Backend(S3ReadinessError::Transient)
            })?
            .into_bytes()
            .to_vec();

        let actual = match hash.algo {
            HashAlgo::Blake3 => ContentHash::blake3(&bytes),
            HashAlgo::Sha256 => return Err(BlobError::AlgoNotVerifiable(HashAlgo::Sha256)),
        };
        if actual != *hash {
            return Err(BlobError::IntegrityFail {
                requested: hash.clone(),
                actual,
            });
        }
        Ok(bytes)
    }

    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> Result<BlobMeta> {
        let key = Self::key_path(tenant, hash);
        let out = self
            .block(
                self.client
                    .head_object()
                    .bucket(&self.bucket)
                    .key(&key)
                    .send(),
            )
            .map_err(|e| {
                if is_no_such_key(&e) {
                    BlobError::NotFound {
                        tenant: tenant.clone(),
                        hash: hash.clone(),
                    }
                } else {
                    let kind = classify_sdk_error(&e, PreflightOperation::Get);
                    self.mark_runtime_failure(kind);
                    BlobError::Backend(kind)
                }
            })?;
        let stored_len = stored_len_from_content_length(out.content_length()).ok_or_else(|| {
            self.mark_runtime_failure(S3ReadinessError::Transient);
            BlobError::Backend(S3ReadinessError::Transient)
        })?;
        Ok(BlobMeta {
            hash: hash.clone(),
            stored_len,
        })
    }

    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<()> {
        let key = Self::key_path(tenant, hash);
        if let Err(error) = self.block(
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(&key)
                .send(),
        ) {
            let kind = classify_sdk_error(&error, PreflightOperation::Put);
            self.mark_runtime_failure(kind);
            return Err(BlobError::Backend(kind));
        }
        Ok(())
    }
}

fn stored_len_from_content_length(content_length: Option<i64>) -> Option<usize> {
    usize::try_from(content_length?).ok()
}

fn s3_config_is_valid(cfg: &S3Config) -> bool {
    let endpoint = cfg.endpoint.trim();
    let authority = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default();
    let bucket = cfg.bucket.as_bytes();
    !authority.is_empty()
        && !authority.contains([' ', '@', '#', '?'])
        && !cfg.region.is_empty()
        && cfg
            .region
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !cfg.access_key.trim().is_empty()
        && !cfg.secret_key.trim().is_empty()
        && (3..=63).contains(&bucket.len())
        && bucket.first().is_some_and(u8::is_ascii_alphanumeric)
        && bucket.last().is_some_and(u8::is_ascii_alphanumeric)
        && bucket.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        && !cfg.bucket.contains("..")
        && !cfg.bucket.contains(".-")
        && !cfg.bucket.contains("-.")
}

#[derive(Clone, Copy)]
enum PreflightOperation {
    Put,
    Get,
}

#[derive(Clone, Copy)]
enum PreflightFailure {
    Construction,
    Service(u16),
    Other,
}

fn classify_sdk_error<E>(
    error: &aws_sdk_s3::error::SdkError<E>,
    operation: PreflightOperation,
) -> S3ReadinessError {
    let failure = match error {
        aws_sdk_s3::error::SdkError::ConstructionFailure(_) => PreflightFailure::Construction,
        aws_sdk_s3::error::SdkError::ServiceError(service) => {
            PreflightFailure::Service(service.raw().status().as_u16())
        }
        _ => PreflightFailure::Other,
    };
    classify_preflight_failure(failure, operation)
}

fn classify_preflight_failure(
    failure: PreflightFailure,
    operation: PreflightOperation,
) -> S3ReadinessError {
    match failure {
        PreflightFailure::Construction => S3ReadinessError::PermanentConfig,
        PreflightFailure::Other => S3ReadinessError::Transient,
        PreflightFailure::Service(status) => match status {
            401 | 403 => S3ReadinessError::PermanentAuth,
            301 | 307 | 400 => S3ReadinessError::PermanentConfig,
            404 if matches!(operation, PreflightOperation::Put) => {
                S3ReadinessError::PermanentConfig
            }
            404 | 408 | 409 | 425 | 429 => S3ReadinessError::Transient,
            400..=499 => S3ReadinessError::PermanentConfig,
            _ => S3ReadinessError::Transient,
        },
    }
}

fn is_no_such_key<E: std::fmt::Debug>(e: &aws_sdk_s3::error::SdkError<E>) -> bool {
    if let aws_sdk_s3::error::SdkError::ServiceError(svc) = e {
        let raw = svc.raw();
        return raw.status().as_u16() == 404;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> S3Config {
        S3Config {
            endpoint: "https://object.example.invalid".into(),
            region: "fr-par".into(),
            access_key: "ACCESS_SENTINEL".into(),
            secret_key: "SECRET_SENTINEL".into(),
            bucket: "myelin-prod".into(),
            force_path_style: true,
        }
    }

    #[test]
    fn marker_is_stable_reserved_and_uses_normal_cas_key_shape() {
        let key = S3BlobStore::key_path(
            &TenantId(READINESS_TENANT.into()),
            &ContentHash::blake3(READINESS_MARKER_BYTES),
        );
        assert!(key.starts_with(".myelin-readiness/blake3/"));
        assert_eq!(
            key,
            S3BlobStore::key_path(
                &TenantId(READINESS_TENANT.into()),
                &ContentHash::blake3(READINESS_MARKER_BYTES),
            )
        );
    }

    #[tokio::test]
    async fn readiness_marker_body_must_match_exactly_with_or_without_length() {
        assert_eq!(
            verify_readiness_marker(
                ByteStream::from_static(READINESS_MARKER_BYTES),
                Some(READINESS_MARKER_BYTES.len() as i64),
            )
            .await,
            Ok(()),
        );
        assert_eq!(
            verify_readiness_marker(ByteStream::from_static(READINESS_MARKER_BYTES), None).await,
            Ok(()),
        );

        for (body, declared) in [
            (
                ByteStream::from_static(b"myelin-blob-read-write-v1-extra"),
                None,
            ),
            (
                ByteStream::from_static(b"myelin-blob-read-write-v1-extra"),
                Some(READINESS_MARKER_BYTES.len() as i64),
            ),
            (ByteStream::from_static(b"short"), None),
            (
                ByteStream::from_static(READINESS_MARKER_BYTES),
                Some((READINESS_MARKER_BYTES.len() + 1) as i64),
            ),
        ] {
            assert_eq!(
                verify_readiness_marker(body, declared).await,
                Err(S3ReadinessError::Transient),
            );
        }
    }

    #[test]
    fn malformed_authority_configuration_is_rejected() {
        assert!(s3_config_is_valid(&config()));
        for invalid in [
            S3Config {
                endpoint: "not-a-url".into(),
                ..config()
            },
            S3Config {
                bucket: "UPPERCASE".into(),
                ..config()
            },
            S3Config {
                region: "bad region".into(),
                ..config()
            },
            S3Config {
                secret_key: String::new(),
                ..config()
            },
        ] {
            assert!(!s3_config_is_valid(&invalid));
        }
    }

    #[test]
    fn content_length_validation_preserves_real_zero_and_rejects_invalid_metadata() {
        assert_eq!(stored_len_from_content_length(Some(0)), Some(0));
        assert_eq!(stored_len_from_content_length(Some(42)), Some(42));
        assert_eq!(stored_len_from_content_length(None), None);
        assert_eq!(stored_len_from_content_length(Some(-1)), None);
    }

    #[test]
    fn dependency_error_rendering_contains_no_authority_detail() {
        let rendered = [
            S3ReadinessError::PermanentConfig,
            S3ReadinessError::PermanentAuth,
            S3ReadinessError::Transient,
        ]
        .map(|error| error.to_string())
        .join(" ");
        for forbidden in [
            "ACCESS_SENTINEL",
            "SECRET_SENTINEL",
            "object.example.invalid",
            "myelin-prod",
            READINESS_TENANT,
        ] {
            assert!(!rendered.contains(forbidden));
        }
    }

    #[test]
    fn readiness_classifier_is_operation_aware_and_fail_closed() {
        use PreflightOperation::{Get, Put};
        let cases = [
            (
                PreflightFailure::Construction,
                Put,
                S3ReadinessError::PermanentConfig,
            ),
            (
                PreflightFailure::Service(401),
                Put,
                S3ReadinessError::PermanentAuth,
            ),
            (
                PreflightFailure::Service(403),
                Get,
                S3ReadinessError::PermanentAuth,
            ),
            (
                PreflightFailure::Service(404),
                Put,
                S3ReadinessError::PermanentConfig,
            ),
            (
                PreflightFailure::Service(404),
                Get,
                S3ReadinessError::Transient,
            ),
            (
                PreflightFailure::Service(408),
                Put,
                S3ReadinessError::Transient,
            ),
            (
                PreflightFailure::Service(429),
                Get,
                S3ReadinessError::Transient,
            ),
            (
                PreflightFailure::Service(503),
                Put,
                S3ReadinessError::Transient,
            ),
            (
                PreflightFailure::Service(422),
                Get,
                S3ReadinessError::PermanentConfig,
            ),
            (PreflightFailure::Other, Get, S3ReadinessError::Transient),
        ];
        for (failure, operation, expected) in cases {
            assert_eq!(classify_preflight_failure(failure, operation), expected);
        }
    }
}
