//! The config-selection seam — real-vs-in-memory storage/cache backends from [`MyelinConfig`].
//!
//! **Stage 2 / infra.** This is the dev<->prod CONFIG SWAP made concrete for the storage tier:
//! the SAME code asks for a [`BlobStore`](crate::blob::BlobStore) /
//! [`Cache`](crate::cache::Cache); whether it gets the in-memory floor or the real
//! RustFS/Valkey backing is a CONFIG choice, not a code change. The selection enum carries
//! ONLY the choice; the endpoints come from [`MyelinConfig`] (the env-driven layer).
//!
//! Compiled only under `--features integration` (the real backings live there). The default
//! build keeps the in-memory floors directly via [`crate::blob::FsBlobStore`] /
//! [`crate::cache::InMemoryCache`].

use myelin_config::MyelinConfig;

use crate::blob::BlobStore;
// MR-009b W7.3 — the fs `FsBlobStore` floor is `test-support`-gated; the `Backend::InMemory` blob
// arm below (a test/dev convenience — production uses `Backend::Real` → `S3BlobStore`) is gated with
// it. There are zero production callers of `blob_store(Backend::InMemory, …)`.
#[cfg(any(test, feature = "test-support"))]
use crate::blob::FsBlobStore;
use crate::cache::{Cache, CacheError, InMemoryCache};
use crate::s3blob::S3BlobStore;
use crate::valkey::ValkeyCache;

/// Which backing a storage seam should use — the config-selected choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// The in-memory/fs floor (unit tests, ephemeral dev).
    InMemory,
    /// The real backend (Postgres / RustFS / Valkey) at the [`MyelinConfig`] endpoints.
    Real,
}

/// Build the [`BlobStore`] the config selects: the real [`S3BlobStore`] (RustFS/Scaleway via
/// the `cfg.s3` endpoint) for [`Backend::Real`], or the [`FsBlobStore`] floor for
/// [`Backend::InMemory`]. The return is a boxed trait object so the CALLER is identical for
/// both — the swap is entirely in this seam.
pub fn blob_store(
    backend: Backend,
    cfg: &MyelinConfig,
    rt: tokio::runtime::Handle,
) -> Box<dyn BlobStore + Send + Sync> {
    match backend {
        // MR-009b W7.3 — the fs floor is a `test-support`-gated test double. In a production build
        // (no `test-support`) this arm is unreachable — production wires `Backend::Real` only
        // (`SubstrateProvider::blob_store` hardcodes `Backend::Real`) — so it fails LOUD rather than
        // silently returning an in-memory store as a system of record.
        #[cfg(any(test, feature = "test-support"))]
        Backend::InMemory => Box::new(FsBlobStore::new()),
        #[cfg(not(any(test, feature = "test-support")))]
        Backend::InMemory => panic!(
            "backend::blob_store(Backend::InMemory) requires the `test-support` feature — the fs \
             FsBlobStore floor is a test double; production uses Backend::Real (S3BlobStore)"
        ),
        Backend::Real => Box::new(S3BlobStore::connect(&cfg.s3, rt)),
    }
}

/// Build the [`Cache`] the config selects: the real [`ValkeyCache`] (via `cfg.redis_url`) for
/// [`Backend::Real`], or the [`InMemoryCache`] floor for [`Backend::InMemory`]. Same boxed-seam
/// shape as [`blob_store`] so the caller never branches.
pub fn cache(
    backend: Backend,
    cfg: &MyelinConfig,
    rt: tokio::runtime::Handle,
) -> Result<Box<dyn Cache>, CacheError> {
    match backend {
        Backend::InMemory => Ok(Box::new(InMemoryCache::new())),
        Backend::Real => Ok(Box::new(ValkeyCache::connect(&cfg.redis_url, rt)?)),
    }
}
