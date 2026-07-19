//! `S3BlobStore` — the [`BlobStore`](crate::blob::BlobStore) trait backed by a REAL
//! S3-compatible object store (RustFS in dev, Scaleway Object Storage in prod), via
//! `aws-sdk-s3` with a custom endpoint + path-style addressing.
//!
//! **Stage 2 / infra.** This is the object-store backing the [`crate::blob`] module's floor
//! note names ("the object-store (MinIO / Ceph RADOS) BlobStore is the M5 follow-on … a
//! one-line backing swap by the trait's design"). It implements the EXACT frozen
//! `put/get/head/delete` shape behind the existing [`BlobStore`] trait — it does NOT fork or
//! redefine the trait (EI-01 §7 coherence). The fs floor [`crate::blob::FsBlobStore`] remains
//! the unit/default backing; this real backing is config-selected (see
//! [`crate::backend::StorageBackend`]) and compiled ONLY under `--features integration`.
//!
//! ## How a sync trait drives the async SDK
//! [`BlobStore`] is a sync trait (it matches the fs floor's shape). `aws-sdk-s3` is async, so
//! `S3BlobStore` holds a `tokio::runtime::Handle` and drives each request to completion with
//! `Handle::block_in_place` + `block_on`. This is the same "the real impl drives the async
//! client on a runtime internally" pattern the [`crate::cache`] module note established. The
//! handle is supplied at construction so the caller owns the runtime (the harness's runtime in
//! prod; a test runtime in the smoke test).
//!
//! ## Content-addressing + per-tenant keyspace are preserved
//! The object KEY is the SAME per-tenant fan-out path the fs floor uses
//! (`<tenant>/<algo>/<aa>/<rest>`) — so the per-tenant isolation + per-tenant dedup semantics
//! of §3.2 hold against the real bucket (two tenants storing identical bytes get two distinct
//! keys → two stored objects). `put` hashes the PLAINTEXT (address-by-plaintext-hash), `get`
//! RE-HASHES the bytes it read and refuses to serve on a mismatch (the STOR-D7 0-silent-serve
//! integrity gate), exactly like the floor.

use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use myelin_config::S3Config;
use myelin_tenancy::TenantId;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::blob::{BlobDependencyError, BlobError, BlobMeta, BlobStore, ContentHash, HashAlgo, Result};

/// The [`BlobStore`] backed by a real S3-compatible object store via `aws-sdk-s3`.
///
/// Cloneable: the inner `aws_sdk_s3::Client` is cheap to clone (an `Arc` internally) and the
/// runtime handle is a handle, so a clone shares the same connection pool + bucket.
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

pub type S3ReadinessError = BlobDependencyError;

#[derive(Clone, Copy)]
struct S3ReadinessState {
    last_error: Option<S3ReadinessError>,
    next_probe: Instant,
}

impl S3BlobStore {
    /// Build an `aws_sdk_s3::Client` from the [`S3Config`] (custom endpoint + path-style +
    /// static dev creds) and wrap it as a [`BlobStore`]. `rt` is the runtime handle the sync
    /// trait methods drive the async SDK on.
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
            // Path-style addressing: http://endpoint/bucket/key (RustFS/Scaleway), NOT the
            // virtual-host http://bucket.endpoint/key form. The myelin-config dev default is
            // force_path_style = true.
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

    /// Prove exact object authority with a stable, non-PII, normal-CAS-shaped marker. The marker is
    /// retained, so this requires PUT+GET only (never ListBuckets, HeadBucket, or DeleteObject).
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
            self.client.put_object()
                .bucket(&self.bucket)
                .key(&marker_key)
                .body(ByteStream::from_static(READINESS_MARKER_BYTES))
                .send().await
                .map_err(|error| classify_sdk_error(&error, PreflightOperation::Put))?;
            let output = self.client.get_object()
                .bucket(&self.bucket)
                .key(&marker_key)
                .send().await
                .map_err(|error| classify_sdk_error(&error, PreflightOperation::Get))?;
            let bytes = output.body.collect().await
                .map_err(|_| S3ReadinessError::Transient)?
                .into_bytes();
            if bytes.as_ref() != READINESS_MARKER_BYTES {
                return Err(S3ReadinessError::Transient);
            }
            Ok(())
        });
        self.record_preflight(result);
        result
    }

    /// Read cached health between bounded probes. Transient failures retry at most once per second;
    /// static credential/config failures remain latched until process restart.
    pub fn readiness(&self) -> std::result::Result<(), S3ReadinessError> {
        let state = *self.readiness.lock().unwrap_or_else(|error| error.into_inner());
        if matches!(state.last_error, Some(S3ReadinessError::PermanentConfig | S3ReadinessError::PermanentAuth)) {
            return Err(state.last_error.expect("matched a permanent readiness error"));
        }
        if Instant::now() < state.next_probe {
            return state.last_error.map_or(Ok(()), Err);
        }
        self.preflight()
    }

    fn record_preflight(&self, result: std::result::Result<(), S3ReadinessError>) {
        let mut state = self.readiness.lock().unwrap_or_else(|error| error.into_inner());
        state.last_error = result.err();
        state.next_probe = Instant::now() + if state.last_error.is_none() {
            HEALTHY_PROBE_INTERVAL
        } else {
            UNHEALTHY_PROBE_INTERVAL
        };
    }

    fn mark_runtime_failure(&self, kind: S3ReadinessError) {
        let mut state = self.readiness.lock().unwrap_or_else(|error| error.into_inner());
        state.last_error = Some(kind);
        state.next_probe = Instant::now();
    }

    /// The per-tenant object key for an address: `<tenant>/<algo>/<aa>/<rest>` — the SAME
    /// Git-style two-char fan-out the fs floor uses, so the per-tenant keyspace isolation +
    /// per-tenant dedup of §3.2 hold identically against the real bucket.
    fn key_path(tenant: &TenantId, hash: &ContentHash) -> String {
        let digest = &hash.digest_hex;
        let (fan, rest) = if digest.len() >= 2 {
            digest.split_at(2)
        } else {
            (digest.as_str(), "")
        };
        format!("{}/{}/{}/{}", tenant.0, hash.algo.tag(), fan, rest)
    }

    /// Run an async S3 op to completion on the owned runtime handle (the sync↔async bridge).
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl BlobStore for S3BlobStore {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash> {
        // Hash-on-write: the content address is the BLAKE3 hash of the PLAINTEXT (stable across
        // the encryption/rotation P-ST-08 wires). Idempotent within a tenant: re-putting the
        // same bytes writes the same key (S3 PUT is an overwrite, so the object stays single).
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
                // A NoSuchKey from the store is a NotFound; every other failure is classified as
                // a redacted dependency error and updates the shared readiness state.
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

        // Re-hash-on-read integrity (STOR-D7, 0 silent serve): the bytes we read MUST re-hash to
        // the requested address, else the serve is REFUSED. We re-hash under the address's own
        // algorithm tag (self-describing multihash), never a global config.
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
        let stored_len = out.content_length().unwrap_or(0).max(0) as usize;
        Ok(BlobMeta {
            hash: hash.clone(),
            stored_len,
        })
    }

    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<()> {
        let key = Self::key_path(tenant, hash);
        // S3 DELETE is idempotent (deleting an absent key is a success), matching the trait's
        // "no-op if absent" delete shape.
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

fn s3_config_is_valid(cfg: &S3Config) -> bool {
    let endpoint = cfg.endpoint.trim();
    let authority = endpoint.strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default();
    let bucket = cfg.bucket.as_bytes();
    !authority.is_empty()
        && !authority.contains([' ', '@', '#', '?'])
        && !cfg.region.is_empty()
        && cfg.region.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !cfg.access_key.trim().is_empty()
        && !cfg.secret_key.trim().is_empty()
        && (3..=63).contains(&bucket.len())
        && bucket.first().is_some_and(u8::is_ascii_alphanumeric)
        && bucket.last().is_some_and(u8::is_ascii_alphanumeric)
        && bucket.iter().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.'))
        && !cfg.bucket.contains("..")
        && !cfg.bucket.contains(".-")
        && !cfg.bucket.contains("-.")
}

#[derive(Clone, Copy)]
enum PreflightOperation { Put, Get }

#[derive(Clone, Copy)]
enum PreflightFailure { Construction, Service(u16), Other }

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
    let PreflightFailure::Service(status) = failure else {
        return match failure {
            PreflightFailure::Construction => S3ReadinessError::PermanentConfig,
            PreflightFailure::Other => S3ReadinessError::Transient,
            PreflightFailure::Service(_) => unreachable!(),
        };
    };
    match status {
        401 | 403 => S3ReadinessError::PermanentAuth,
        301 | 307 | 400 => S3ReadinessError::PermanentConfig,
        404 if matches!(operation, PreflightOperation::Put) => S3ReadinessError::PermanentConfig,
        404 | 408 | 409 | 425 | 429 => S3ReadinessError::Transient,
        400..=499 => S3ReadinessError::PermanentConfig,
        _ => S3ReadinessError::Transient,
    }
}

/// Whether an `aws-sdk-s3` error is a "no such key" (the object is absent) — the signal we map
/// to [`BlobError::NotFound`]. We sniff the service error rather than match an exact typed
/// variant so it works uniformly across get/head (head returns a generic `NotFound`).
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
        assert_eq!(key, S3BlobStore::key_path(
            &TenantId(READINESS_TENANT.into()),
            &ContentHash::blake3(READINESS_MARKER_BYTES),
        ));
    }

    #[test]
    fn malformed_authority_configuration_is_rejected() {
        assert!(s3_config_is_valid(&config()));
        for invalid in [
            S3Config { endpoint: "not-a-url".into(), ..config() },
            S3Config { bucket: "UPPERCASE".into(), ..config() },
            S3Config { region: "bad region".into(), ..config() },
            S3Config { secret_key: String::new(), ..config() },
        ] {
            assert!(!s3_config_is_valid(&invalid));
        }
    }

    #[test]
    fn dependency_error_rendering_contains_no_authority_detail() {
        let rendered = [S3ReadinessError::PermanentConfig, S3ReadinessError::PermanentAuth, S3ReadinessError::Transient]
            .map(|error| error.to_string()).join(" ");
        for forbidden in ["ACCESS_SENTINEL", "SECRET_SENTINEL", "object.example.invalid", "myelin-prod", READINESS_TENANT] {
            assert!(!rendered.contains(forbidden));
        }
    }

    #[test]
    fn readiness_classifier_is_operation_aware_and_fail_closed() {
        use PreflightOperation::{Get, Put};
        let cases = [
            (PreflightFailure::Construction, Put, S3ReadinessError::PermanentConfig),
            (PreflightFailure::Service(401), Put, S3ReadinessError::PermanentAuth),
            (PreflightFailure::Service(403), Get, S3ReadinessError::PermanentAuth),
            (PreflightFailure::Service(404), Put, S3ReadinessError::PermanentConfig),
            (PreflightFailure::Service(404), Get, S3ReadinessError::Transient),
            (PreflightFailure::Service(408), Put, S3ReadinessError::Transient),
            (PreflightFailure::Service(429), Get, S3ReadinessError::Transient),
            (PreflightFailure::Service(503), Put, S3ReadinessError::Transient),
            (PreflightFailure::Service(422), Get, S3ReadinessError::PermanentConfig),
            (PreflightFailure::Other, Get, S3ReadinessError::Transient),
        ];
        for (failure, operation, expected) in cases {
            assert_eq!(classify_preflight_failure(failure, operation), expected);
        }
    }
}
