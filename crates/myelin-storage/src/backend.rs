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

pub fn blob_store(
    backend: Backend,
    cfg: &MyelinConfig,
    rt: tokio::runtime::Handle,
) -> Box<dyn BlobStore + Send + Sync> {
    match backend {
        #[cfg(any(test, feature = "test-support"))]
        Backend::InMemory => Box::new(FsBlobStore::new()),
        #[cfg(not(any(test, feature = "test-support")))]
        Backend::InMemory => panic!(
            "backend::blob_store(Backend::InMemory) requires the `test-support` feature - the fs \
             FsBlobStore floor is a test double; production uses Backend::Real (S3BlobStore)"
        ),
        Backend::Real => Box::new(S3BlobStore::connect(&cfg.s3, rt)),
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
