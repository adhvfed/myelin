//! `ValkeyCache` — the [`Cache`](crate::cache::Cache) trait backed by REAL Valkey (the BSD
//! Redis fork) via the `fred` client.
//!
//! **Stage 2 / infra.** This is the real backing for the minimal cache seam the
//! [`crate::cache`] module created in Stage 1. It implements the EXACT `get/set/delete` shape
//! behind the existing [`Cache`] trait — it does NOT fork or redefine it (EI-01 §7 coherence).
//! The [`crate::cache::InMemoryCache`] floor remains the unit/default backing; this real
//! backing is config-selected (see [`crate::backend::CacheBackend`]) and compiled ONLY under
//! `--features integration`.
//!
//! ## Never a source of truth
//! The cache tier is DERIVED + reconstructible (storage.md §1.1): a miss is `Ok(None)`, never
//! an error, and a stale value is bounded by the TTL on every write. So `get` of an absent OR
//! expired key returns a clean miss; an unreachable Valkey maps to [`CacheError`] (the caller
//! treats it as a miss + logs). The same `{tenant}:{key}` per-tenant namespacing the
//! in-memory floor uses is preserved so one tenant never reads another's cached value.
//!
//! ## How a sync trait drives the async client
//! [`Cache`] is sync (it matches the in-memory floor). `fred` is async, so `ValkeyCache` holds
//! a `tokio::runtime::Handle` and drives each command with `block_in_place` + `block_on` — the
//! same bridge [`crate::s3blob::S3BlobStore`] uses.

use std::time::Duration;

use fred::prelude::*;
use myelin_tenancy::TenantId;

use crate::cache::{Cache, CacheError};

/// The [`Cache`] backed by a real Valkey/Redis server via `fred`.
///
/// Cloneable: the `fred::clients::Client` is an `Arc`-backed handle, so a clone shares the
/// same connection.
#[derive(Clone)]
pub struct ValkeyCache {
    client: fred::clients::Client,
    rt: tokio::runtime::Handle,
}

impl ValkeyCache {
    /// Connect to Valkey at `redis_url` (the myelin-config `REDIS_URL`) and wrap it as a
    /// [`Cache`]. `rt` is the runtime handle the sync trait methods drive the async client on.
    /// Returns a loud [`CacheError`] if the URL is unparseable or the initial connect fails
    /// (fail-fast at construction; a later command on a dropped connection is a per-call
    /// `CacheError` the caller treats as a miss).
    pub fn connect(redis_url: &str, rt: tokio::runtime::Handle) -> Result<ValkeyCache, CacheError> {
        let config =
            Config::from_url(redis_url).map_err(|e| CacheError(format!("bad REDIS_URL: {e}")))?;
        let client = Builder::from_config(config)
            .build()
            .map_err(|e| CacheError(format!("build valkey client: {e}")))?;
        tokio::task::block_in_place(|| rt.block_on(client.init()))
            .map_err(|e| CacheError(format!("connect to valkey: {e}")))?;
        Ok(ValkeyCache { client, rt })
    }

    /// The `{tenant}:{key}` namespaced backing key — identical to the in-memory floor's
    /// namespacing, so the per-tenant isolation property holds against the real server.
    fn ns_key(tenant: &TenantId, key: &str) -> String {
        format!("{}:{}", tenant.0, key)
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl Cache for ValkeyCache {
    fn get(&self, tenant: &TenantId, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let k = Self::ns_key(tenant, key);
        // A GET of an absent/expired key returns a Redis nil → `None` → a clean MISS (never an
        // error). Bytes are stored/read as a binary value so non-UTF-8 cache payloads survive.
        let got: Option<Vec<u8>> = self
            .block(self.client.get(&k))
            .map_err(|e| CacheError(format!("valkey GET: {e}")))?;
        Ok(got)
    }

    fn set(
        &self,
        tenant: &TenantId,
        key: &str,
        value: &[u8],
        ttl: Duration,
    ) -> Result<(), CacheError> {
        let k = Self::ns_key(tenant, key);
        // TTL on every write (the derived-tier staleness bound). Sub-second TTLs round up to at
        // least 1s (EX is whole seconds); a value is always written with an expiry so the cache
        // self-evicts — it is never an unbounded source of truth.
        let secs = ttl.as_secs().max(1) as i64;
        let _: () = self
            .block(self.client.set(
                &k,
                value.to_vec(),
                Some(Expiration::EX(secs)),
                None,
                false,
            ))
            .map_err(|e| CacheError(format!("valkey SET: {e}")))?;
        Ok(())
    }

    fn delete(&self, tenant: &TenantId, key: &str) -> Result<(), CacheError> {
        let k = Self::ns_key(tenant, key);
        // DEL of an absent key is a no-op success (matches the trait's "no-op if absent").
        let _: i64 = self
            .block(self.client.del(&k))
            .map_err(|e| CacheError(format!("valkey DEL: {e}")))?;
        Ok(())
    }
}
