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

use crate::blob::{BlobStore, FsBlobStore};
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
        Backend::InMemory => Box::new(FsBlobStore::new()),
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
