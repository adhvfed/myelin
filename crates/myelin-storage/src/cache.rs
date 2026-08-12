use myelin_tenancy::TenantId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheError(pub String);

impl core::fmt::Display for CacheError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "cache error: {}", self.0)
    }
}

impl std::error::Error for CacheError {}

pub trait Cache: Send + Sync {
    fn get(&self, tenant: &TenantId, key: &str) -> Result<Option<Vec<u8>>, CacheError>;

    fn set(
        &self,
        tenant: &TenantId,
        key: &str,
        value: &[u8],
        ttl: Duration,
    ) -> Result<(), CacheError>;

    fn delete(&self, tenant: &TenantId, key: &str) -> Result<(), CacheError>;
}

#[derive(Clone, Default)]
pub struct InMemoryCache {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
}

struct Entry {
    value: Vec<u8>,
    expires_at: Instant,
}

impl InMemoryCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn ns_key(tenant: &TenantId, key: &str) -> String {
        format!("{}:{}", tenant.0, key)
    }
}

impl Cache for InMemoryCache {
    fn get(&self, tenant: &TenantId, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let k = Self::ns_key(tenant, key);
        let mut map = self
            .inner
            .lock()
            .map_err(|_| CacheError("in-memory cache state is unavailable".into()))?;
        match map.get(&k) {
            Some(e) if e.expires_at > Instant::now() => Ok(Some(e.value.clone())),
            Some(_) => {
                map.remove(&k);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    fn set(
        &self,
        tenant: &TenantId,
        key: &str,
        value: &[u8],
        ttl: Duration,
    ) -> Result<(), CacheError> {
        let k = Self::ns_key(tenant, key);
        let mut map = self
            .inner
            .lock()
            .map_err(|_| CacheError("in-memory cache state is unavailable".into()))?;
        map.insert(
            k,
            Entry {
                value: value.to_vec(),
                expires_at: Instant::now() + ttl,
            },
        );
        Ok(())
    }

    fn delete(&self, tenant: &TenantId, key: &str) -> Result<(), CacheError> {
        let k = Self::ns_key(tenant, key);
        self.inner
            .lock()
            .map_err(|_| CacheError("in-memory cache state is unavailable".into()))?
            .remove(&k);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    #[test]
    fn set_then_get_hits() {
        let c = InMemoryCache::new();
        c.set(&tenant("t1"), "k", b"v", Duration::from_secs(60))
            .unwrap();
        assert_eq!(c.get(&tenant("t1"), "k").unwrap(), Some(b"v".to_vec()));
    }

    #[test]
    fn miss_is_not_an_error() {
        let c = InMemoryCache::new();
        assert_eq!(c.get(&tenant("t1"), "absent").unwrap(), None);
    }

    #[test]
    fn tenants_are_isolated() {
        let c = InMemoryCache::new();
        c.set(&tenant("t1"), "k", b"a", Duration::from_secs(60))
            .unwrap();
        assert_eq!(c.get(&tenant("t2"), "k").unwrap(), None);
    }

    #[test]
    fn expired_entry_is_a_miss() {
        let c = InMemoryCache::new();
        c.set(&tenant("t1"), "k", b"v", Duration::from_millis(0))
            .unwrap();
        assert_eq!(c.get(&tenant("t1"), "k").unwrap(), None);
    }

    #[test]
    fn delete_invalidates() {
        let c = InMemoryCache::new();
        c.set(&tenant("t1"), "k", b"v", Duration::from_secs(60))
            .unwrap();
        c.delete(&tenant("t1"), "k").unwrap();
        assert_eq!(c.get(&tenant("t1"), "k").unwrap(), None);
    }

    #[test]
    fn poisoned_cache_state_is_a_typed_error() {
        let cache = InMemoryCache::new();
        let state = cache.inner.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = state.lock().unwrap();
            panic!("poison the test cache");
        });

        assert_eq!(
            cache.get(&tenant("t1"), "k").unwrap_err(),
            CacheError("in-memory cache state is unavailable".into()),
        );
    }
}
