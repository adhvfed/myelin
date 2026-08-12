use myelin_config::MyelinConfig;

use crate::blob::BlobStore;
#[cfg(any(test, feature = "test-support"))]
use crate::blob::FsBlobStore;
use crate::cache::{Cache, CacheError, InMemoryCache};
use crate::s3blob::S3BlobStore;
use crate::valkey::ValkeyCache;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    InMemory,
    Real,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendError {
    TestOnlyBlobStoreUnavailable,
}

impl core::fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TestOnlyBlobStoreUnavailable => formatter.write_str(
                "the in-memory blob store requires the `test-support` feature; production uses the real object store",
            ),
        }
    }
}

impl std::error::Error for BackendError {}

pub fn blob_store(
    backend: Backend,
    cfg: &MyelinConfig,
    rt: tokio::runtime::Handle,
) -> Result<Box<dyn BlobStore + Send + Sync>, BackendError> {
    match backend {
        #[cfg(any(test, feature = "test-support"))]
        Backend::InMemory => Ok(Box::new(FsBlobStore::new())),
        #[cfg(not(any(test, feature = "test-support")))]
        Backend::InMemory => Err(BackendError::TestOnlyBlobStoreUnavailable),
        Backend::Real => Ok(Box::new(S3BlobStore::connect(&cfg.s3, rt))),
    }
}

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
