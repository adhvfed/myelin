//! # The minimal cache trait (Stage 1 / infra — NEW seam, noted in the report).
//!
//! No cache trait existed in the workspace before Stage 1 (the survey found BlobStore in
//! myelin-storage, BusTransport in myelin-events, the OLTP/outbox/ReBAC stores in
//! -storage/-substrate/-identity — but NO cache seam). Per the ground rules ("if a cache trait
//! does not yet exist, create a minimal one and note it") this is that minimal seam.
//!
//! It is deliberately tiny — `get` / `set` / `delete` over a per-tenant-keyed namespace, with a
//! TTL on writes — so the in-memory floor (unit tests) and the real Valkey/Redis backing (the
//! `fred` client, behind the `integration` feature) are a one-line swap, exactly like the
//! fs↔object BlobStore swap. The cache is a DERIVED, reconstructible tier: a miss is never an
//! error, and a wrong/stale value is bounded by the TTL — so the trait carries no durability
//! guarantee.
//!
//! Keys are namespaced by [`TenantId`] for the same isolation reason BlobStore is: one tenant
//! MUST NOT read another tenant's cached value. The backing key is `{tenant}:{key}`.

use myelin_tenancy::TenantId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A cache error. A MISS is NOT an error (it is `Ok(None)` from [`Cache::get`]); this type
/// models the backing store being unreachable (the real Valkey impl maps a connection error
/// here). The cache is best-effort, so callers typically treat an `Err` as a miss + log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheError(pub String);

impl core::fmt::Display for CacheError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "cache error: {}", self.0)
    }
}

impl std::error::Error for CacheError {}

/// The minimal cache seam. Per-tenant-keyed, TTL on write, miss-is-not-an-error.
///
/// The in-memory [`InMemoryCache`] is the unit/floor impl; the Valkey/Redis (`fred`) impl lands
/// behind the `integration` feature. Methods are sync over an owned handle (the real impl drives
/// the async client on a runtime internally) to match the BlobStore shape on this floor.
pub trait Cache: Send + Sync {
    /// Read the value cached under `key` in `tenant`'s namespace. `Ok(None)` is a clean MISS
    /// (including an expired entry); `Err` is the backing store being unreachable.
    fn get(&self, tenant: &TenantId, key: &str) -> Result<Option<Vec<u8>>, CacheError>;

    /// Cache `value` under `key` in `tenant`'s namespace for `ttl`. Overwrites any prior value.
    fn set(&self, tenant: &TenantId, key: &str, value: &[u8], ttl: Duration)
        -> Result<(), CacheError>;

    /// Invalidate `key` in `tenant`'s namespace (a no-op if absent).
    fn delete(&self, tenant: &TenantId, key: &str) -> Result<(), CacheError>;
}

/// The in-memory floor [`Cache`] (unit tests). A cloneable handle over shared state with a
/// monotonic-clock TTL — a `get` past the deadline reports a clean MISS and evicts lazily.
#[derive(Clone, Default)]
pub struct InMemoryCache {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
}

struct Entry {
    value: Vec<u8>,
    expires_at: Instant,
}

impl InMemoryCache {
    /// A fresh, empty cache.
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
        let mut map = self.inner.lock().expect("cache mutex poisoned");
        match map.get(&k) {
            Some(e) if e.expires_at > Instant::now() => Ok(Some(e.value.clone())),
            Some(_) => {
                // Lazily evict the expired entry; report a clean miss.
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
        let mut map = self.inner.lock().expect("cache mutex poisoned");
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
            .expect("cache mutex poisoned")
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
        c.set(&tenant("t1"), "k", b"v", Duration::from_secs(60)).unwrap();
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
        c.set(&tenant("t1"), "k", b"a", Duration::from_secs(60)).unwrap();
        // A DIFFERENT tenant must not read t1's value.
        assert_eq!(c.get(&tenant("t2"), "k").unwrap(), None);
    }

    #[test]
    fn expired_entry_is_a_miss() {
        let c = InMemoryCache::new();
        c.set(&tenant("t1"), "k", b"v", Duration::from_millis(0)).unwrap();
        // TTL of 0 means the deadline is already past on the next get.
        assert_eq!(c.get(&tenant("t1"), "k").unwrap(), None);
    }

    #[test]
    fn delete_invalidates() {
        let c = InMemoryCache::new();
        c.set(&tenant("t1"), "k", b"v", Duration::from_secs(60)).unwrap();
        c.delete(&tenant("t1"), "k").unwrap();
        assert_eq!(c.get(&tenant("t1"), "k").unwrap(), None);
    }
}
