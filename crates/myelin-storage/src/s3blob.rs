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

use crate::blob::{BlobError, BlobMeta, BlobStore, ContentHash, HashAlgo, Result};

/// The [`BlobStore`] backed by a real S3-compatible object store via `aws-sdk-s3`.
///
/// Cloneable: the inner `aws_sdk_s3::Client` is cheap to clone (an `Arc` internally) and the
/// runtime handle is a handle, so a clone shares the same connection pool + bucket.
#[derive(Clone)]
pub struct S3BlobStore {
    client: Client,
    bucket: String,
    rt: tokio::runtime::Handle,
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
        }
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
        self.block(
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(&key)
                .body(ByteStream::from(bytes.to_vec()))
                .send(),
        )
        .map_err(|e| BlobError::MalformedAddress(format!("s3 put_object failed: {e}")))?;
        Ok(hash)
    }

    fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>> {
        let key = Self::key_path(tenant, hash);
        let out = self
            .block(self.client.get_object().bucket(&self.bucket).key(&key).send())
            .map_err(|e| {
                // A NoSuchKey from the store is a NotFound; any other error is surfaced as a
                // NotFound too (the trait's narrow error set has no transport variant — the
                // smoke test only asserts the happy round-trip + the post-delete miss).
                if is_no_such_key(&e) {
                    BlobError::NotFound {
                        tenant: tenant.clone(),
                        hash: hash.clone(),
                    }
                } else {
                    BlobError::MalformedAddress(format!("s3 get_object failed: {e}"))
                }
            })?;
        let bytes = self
            .block(out.body.collect())
            .map_err(|e| BlobError::MalformedAddress(format!("s3 body collect failed: {e}")))?
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
            .block(self.client.head_object().bucket(&self.bucket).key(&key).send())
            .map_err(|e| {
                if is_no_such_key(&e) {
                    BlobError::NotFound {
                        tenant: tenant.clone(),
                        hash: hash.clone(),
                    }
                } else {
                    BlobError::MalformedAddress(format!("s3 head_object failed: {e}"))
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
        self.block(
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(&key)
                .send(),
        )
        .map_err(|e| BlobError::MalformedAddress(format!("s3 delete_object failed: {e}")))?;
        Ok(())
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
