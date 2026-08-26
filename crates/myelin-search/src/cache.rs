use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use myelin_identity::{Consistency, ListObjectsResult, ObjectType, Principal};
use myelin_storage::{DekHandle, KmsError, PiiKeyRef};
use myelin_substrate::{Clock, MonotonicClock, Seconds};
use myelin_tenancy::{Region, TenantId};

use crate::consistency::fail_static_bypass;
use crate::dek::SearchDekPin;
use crate::pipeline::{watermark_from_zookie, RankedResult, RankedResults};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheTtl {
    ttl_secs: Seconds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TtlExceedsRevocationSla {
    pub ttl_secs: Seconds,
    pub revocation_sla_secs: Seconds,
}

impl std::fmt::Display for TtlExceedsRevocationSla {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Search cache TTL ({}s) > revocation SLA ({}s) - a revoked grant would be served from \
             cache past N; rejected (architecture §4.10 / §8.2)",
            self.ttl_secs, self.revocation_sla_secs
        )
    }
}

impl std::error::Error for TtlExceedsRevocationSla {}

impl CacheTtl {
    pub fn bounded(
        ttl_secs: Seconds,
        revocation_sla_secs: Seconds,
    ) -> Result<CacheTtl, TtlExceedsRevocationSla> {
        if ttl_secs > revocation_sla_secs {
            return Err(TtlExceedsRevocationSla {
                ttl_secs,
                revocation_sla_secs,
            });
        }
        Ok(CacheTtl { ttl_secs })
    }

    pub fn secs(self) -> Seconds {
        self.ttl_secs
    }
}

pub fn zookie_bucket(zookie: &str) -> u64 {
    watermark_from_zookie(zookie).0
}

pub fn should_bypass(at: &Consistency) -> bool {
    fail_static_bypass(at)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FilterKey {
    tenant: String,
    region: String,
    subject: String,
    object_type: String,
    zookie_bucket: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ResultKey {
    tenant: String,
    region: String,
    subject: String,
    query_hash: u64,
    zookie_bucket: u64,
}

struct SealedEntry {
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
    cached_at_secs: u64,
}

impl SealedEntry {
    fn is_fresh_at(&self, now_secs: u64, ttl_secs: Seconds) -> bool {
        now_secs
            .checked_sub(self.cached_at_secs)
            .is_some_and(|age| age <= ttl_secs)
    }
}

#[derive(Debug, Default)]
pub struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
    bypasses: AtomicU64,
    expired: AtomicU64,
    shredded: AtomicU64,
    coalesced: AtomicU64,
}

impl CacheStats {
    pub fn new() -> CacheStats {
        CacheStats::default()
    }

    fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }
    fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }
    fn record_bypass(&self) {
        self.bypasses.fetch_add(1, Ordering::Relaxed);
    }
    fn record_expired(&self) {
        self.expired.fetch_add(1, Ordering::Relaxed);
    }
    fn record_shredded(&self) {
        self.shredded.fetch_add(1, Ordering::Relaxed);
    }
    fn record_coalesced(&self) {
        self.coalesced.fetch_add(1, Ordering::Relaxed);
    }

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
    pub fn bypasses(&self) -> u64 {
        self.bypasses.load(Ordering::Relaxed)
    }
    pub fn expired(&self) -> u64 {
        self.expired.load(Ordering::Relaxed)
    }
    pub fn shredded(&self) -> u64 {
        self.shredded.load(Ordering::Relaxed)
    }
    pub fn coalesced(&self) -> u64 {
        self.coalesced.load(Ordering::Relaxed)
    }

    pub fn hit_ratio_pct(&self) -> Option<u64> {
        let denom = self.hits().saturating_add(self.misses());
        (self.hits().saturating_mul(100)).checked_div(denom)
    }
}

fn seal_under_dek(
    dek: &SearchDekPin,
    key_ref: &PiiKeyRef,
    region: &Region,
    plaintext: &[u8],
) -> Result<([u8; 12], Vec<u8>), KmsError> {
    let handle: DekHandle = dek.resolve(key_ref, region)?;
    Ok(handle.seal(plaintext))
}

fn open_under_dek(
    dek: &SearchDekPin,
    key_ref: &PiiKeyRef,
    region: &Region,
    entry: &SealedEntry,
) -> Result<Option<Vec<u8>>, KmsError> {
    let handle: DekHandle = dek.resolve(key_ref, region)?;
    Ok(handle.open(&entry.nonce, &entry.ciphertext))
}

pub struct FilterCache {
    ttl: CacheTtl,
    dek: SearchDekPin,
    clock: Box<dyn Clock>,
    entries: Mutex<HashMap<FilterKey, SealedEntry>>,
    stats: CacheStats,
}

impl FilterCache {
    pub fn new(ttl: CacheTtl, dek: SearchDekPin) -> FilterCache {
        FilterCache::with_clock(ttl, dek, Box::new(MonotonicClock::default()))
    }

    pub fn with_clock(ttl: CacheTtl, dek: SearchDekPin, clock: Box<dyn Clock>) -> FilterCache {
        FilterCache {
            ttl,
            dek,
            clock,
            entries: Mutex::new(HashMap::new()),
            stats: CacheStats::new(),
        }
    }

    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    fn key(
        tenant: &TenantId,
        region: &Region,
        subject: &Principal,
        ty: &ObjectType,
        zookie: &str,
    ) -> FilterKey {
        FilterKey {
            tenant: tenant.as_str().to_string(),
            region: region.0.clone(),
            subject: subject.principal_id.0.clone(),
            object_type: ty.0.clone(),
            zookie_bucket: zookie_bucket(zookie),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_or_compute(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &Principal,
        ty: &ObjectType,
        at: &Consistency,
        key_ref: &PiiKeyRef,
        compute: impl FnOnce() -> ListObjectsResult,
    ) -> Result<ListObjectsResult, KmsError> {
        if should_bypass(at) {
            self.stats.record_bypass();
            return Ok(compute());
        }
        let zookie = at.at_least.0.as_str();
        let key = Self::key(tenant, region, subject, ty, zookie);

        if let Some(plaintext) = self.try_read(&key, region, key_ref)? {
            let result: ListObjectsResult = serde_json::from_slice(&plaintext)
                .expect("a sealed S5 entry round-trips its ListObjectsResult");
            self.stats.record_hit();
            return Ok(result);
        }

        self.stats.record_miss();
        let result = compute();
        let plaintext = serde_json::to_vec(&result).expect("ListObjectsResult serialises");
        let (nonce, ciphertext) = seal_under_dek(&self.dek, key_ref, region, &plaintext)?;
        let now = self.clock.now_secs();
        self.entries
            .lock()
            .expect("S5 filter cache poisoned")
            .insert(
                key,
                SealedEntry {
                    nonce,
                    ciphertext,
                    cached_at_secs: now,
                },
            );
        Ok(result)
    }

    fn try_read(
        &self,
        key: &FilterKey,
        region: &Region,
        key_ref: &PiiKeyRef,
    ) -> Result<Option<Vec<u8>>, KmsError> {
        let mut entries = self.entries.lock().expect("S5 filter cache poisoned");
        let Some(entry) = entries.get(key) else {
            return Ok(None);
        };
        if !entry.is_fresh_at(self.clock.now_secs(), self.ttl.secs()) {
            entries.remove(key);
            self.stats.record_expired();
            return Ok(None);
        }
        match open_under_dek(&self.dek, key_ref, region, entry) {
            Ok(Some(plaintext)) => Ok(Some(plaintext)),
            Ok(None) => {
                entries.remove(key);
                Ok(None)
            }
            Err(_dek_gone) => {
                entries.remove(key);
                self.stats.record_shredded();
                Ok(None)
            }
        }
    }

    pub fn probe_recoverable(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &Principal,
        ty: &ObjectType,
        zookie: &str,
        key_ref: &PiiKeyRef,
    ) -> Result<bool, KmsError> {
        let key = Self::key(tenant, region, subject, ty, zookie);
        let entries = self.entries.lock().expect("S5 filter cache poisoned");
        let Some(entry) = entries.get(&key) else {
            return Ok(false);
        };
        if !entry.is_fresh_at(self.clock.now_secs(), self.ttl.secs()) {
            return Ok(false);
        }
        Ok(open_under_dek(&self.dek, key_ref, region, entry)?.is_some())
    }
}

impl std::fmt::Debug for FilterCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterCache")
            .field("ttl_secs", &self.ttl.secs())
            .field("hits", &self.stats.hits())
            .field("misses", &self.stats.misses())
            .finish_non_exhaustive()
    }
}

pub struct ResultCache {
    ttl: CacheTtl,
    dek: SearchDekPin,
    clock: Box<dyn Clock>,
    entries: Mutex<HashMap<ResultKey, SealedEntry>>,
    inflight: Mutex<HashMap<ResultKey, Arc<Mutex<()>>>>,
    stats: CacheStats,
}

impl ResultCache {
    pub fn new(ttl: CacheTtl, dek: SearchDekPin) -> ResultCache {
        ResultCache::with_clock(ttl, dek, Box::new(MonotonicClock::default()))
    }

    pub fn with_clock(ttl: CacheTtl, dek: SearchDekPin, clock: Box<dyn Clock>) -> ResultCache {
        ResultCache {
            ttl,
            dek,
            clock,
            entries: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            stats: CacheStats::new(),
        }
    }

    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    fn key(
        tenant: &TenantId,
        region: &Region,
        subject: &Principal,
        query_hash: u64,
        zookie: &str,
    ) -> ResultKey {
        ResultKey {
            tenant: tenant.as_str().to_string(),
            region: region.0.clone(),
            subject: subject.principal_id.0.clone(),
            query_hash,
            zookie_bucket: zookie_bucket(zookie),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_or_compute(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &Principal,
        query_hash: u64,
        at: &Consistency,
        key_ref: &PiiKeyRef,
        compute: impl FnOnce() -> RankedResults,
    ) -> Result<RankedResults, KmsError> {
        if should_bypass(at) {
            self.stats.record_bypass();
            return Ok(compute());
        }
        let zookie = at.at_least.0.as_str();
        let key = Self::key(tenant, region, subject, query_hash, zookie);

        if let Some(plaintext) = self.try_read(&key, region, key_ref)? {
            self.stats.record_hit();
            return Ok(decode_results(&plaintext));
        }

        let computation = CoalescedComputation::join(&self.inflight, key.clone());
        let _held = computation.wait();

        if let Some(plaintext) = self.try_read(&key, region, key_ref)? {
            self.stats.record_coalesced();
            self.stats.record_hit();
            return Ok(decode_results(&plaintext));
        }

        self.stats.record_miss();
        let results = compute();
        let plaintext = encode_results(&results);
        let (nonce, ciphertext) = seal_under_dek(&self.dek, key_ref, region, &plaintext)?;
        let now = self.clock.now_secs();
        self.entries.lock().expect("result cache poisoned").insert(
            key.clone(),
            SealedEntry {
                nonce,
                ciphertext,
                cached_at_secs: now,
            },
        );

        Ok(results)
    }

    fn try_read(
        &self,
        key: &ResultKey,
        region: &Region,
        key_ref: &PiiKeyRef,
    ) -> Result<Option<Vec<u8>>, KmsError> {
        let mut entries = self.entries.lock().expect("result cache poisoned");
        let Some(entry) = entries.get(key) else {
            return Ok(None);
        };
        if !entry.is_fresh_at(self.clock.now_secs(), self.ttl.secs()) {
            entries.remove(key);
            self.stats.record_expired();
            return Ok(None);
        }
        match open_under_dek(&self.dek, key_ref, region, entry) {
            Ok(Some(plaintext)) => Ok(Some(plaintext)),
            Ok(None) => {
                entries.remove(key);
                Ok(None)
            }
            Err(_dek_gone) => {
                entries.remove(key);
                self.stats.record_shredded();
                Ok(None)
            }
        }
    }

    pub fn probe_recoverable(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &Principal,
        query_hash: u64,
        zookie: &str,
        key_ref: &PiiKeyRef,
    ) -> Result<bool, KmsError> {
        let key = Self::key(tenant, region, subject, query_hash, zookie);
        let entries = self.entries.lock().expect("result cache poisoned");
        let Some(entry) = entries.get(&key) else {
            return Ok(false);
        };
        if !entry.is_fresh_at(self.clock.now_secs(), self.ttl.secs()) {
            return Ok(false);
        }
        Ok(open_under_dek(&self.dek, key_ref, region, entry)?.is_some())
    }
}

struct CoalescedComputation<'a> {
    registry: &'a Mutex<HashMap<ResultKey, Arc<Mutex<()>>>>,
    key: ResultKey,
    gate: Arc<Mutex<()>>,
}

impl<'a> CoalescedComputation<'a> {
    fn join(registry: &'a Mutex<HashMap<ResultKey, Arc<Mutex<()>>>>, key: ResultKey) -> Self {
        let gate = registry
            .lock()
            .expect("result cache inflight poisoned")
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        Self {
            registry,
            key,
            gate,
        }
    }

    fn wait(&self) -> MutexGuard<'_, ()> {
        self.gate
            .lock()
            .expect("result cache coalesce gate poisoned")
    }
}

impl Drop for CoalescedComputation<'_> {
    fn drop(&mut self) {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let joined_gate_is_current = registry
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.gate));
        if joined_gate_is_current {
            registry.remove(&self.key);
        }
    }
}

impl std::fmt::Debug for ResultCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultCache")
            .field("ttl_secs", &self.ttl.secs())
            .field("hits", &self.stats.hits())
            .field("coalesced", &self.stats.coalesced())
            .finish_non_exhaustive()
    }
}

fn encode_results(r: &RankedResults) -> Vec<u8> {
    let mut out = Vec::new();
    fn put_str(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    put_str(&mut out, &r.zookie);
    out.extend_from_slice(&(r.hits.len() as u32).to_le_bytes());
    for h in &r.hits {
        put_str(&mut out, &h.doc_id);
        out.extend_from_slice(&h.score.to_le_bytes());
    }
    out.extend_from_slice(&(r.post_fetch_fields.len() as u32).to_le_bytes());
    for f in &r.post_fetch_fields {
        put_str(&mut out, f);
    }
    out
}

fn decode_results(bytes: &[u8]) -> RankedResults {
    let mut cur = 0usize;
    fn take_str(bytes: &[u8], cur: &mut usize) -> String {
        let len = u32::from_le_bytes(bytes[*cur..*cur + 4].try_into().expect("len")) as usize;
        *cur += 4;
        let s = String::from_utf8(bytes[*cur..*cur + len].to_vec()).expect("utf8");
        *cur += len;
        s
    }
    fn take_u32(bytes: &[u8], cur: &mut usize) -> usize {
        let n = u32::from_le_bytes(bytes[*cur..*cur + 4].try_into().expect("u32")) as usize;
        *cur += 4;
        n
    }
    let zookie = take_str(bytes, &mut cur);
    let n_hits = take_u32(bytes, &mut cur);
    let mut hits = Vec::with_capacity(n_hits);
    for _ in 0..n_hits {
        let doc_id = take_str(bytes, &mut cur);
        let score = f32::from_le_bytes(bytes[cur..cur + 4].try_into().expect("score"));
        cur += 4;
        hits.push(RankedResult { doc_id, score });
    }
    let n_fields = take_u32(bytes, &mut cur);
    let mut post_fetch_fields = Vec::with_capacity(n_fields);
    for _ in 0..n_fields {
        post_fetch_fields.push(take_str(bytes, &mut cur));
    }
    RankedResults {
        hits,
        zookie,
        post_fetch_fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{ConsistencyMode, PrincipalId, PrincipalKind, Zookie};
    use myelin_storage::KmsEngine;
    use myelin_substrate::TestClock;
    use std::sync::Arc;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn subject() -> Principal {
        Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }
    fn ty() -> ObjectType {
        ObjectType("issue".into())
    }
    fn strong(zookie: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zookie.into()),
            mode: ConsistencyMode::Strong,
        }
    }
    fn bounded(zookie: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zookie.into()),
            mode: ConsistencyMode::BoundedStale,
        }
    }

    fn pin_with_dek() -> (SearchDekPin, PiiKeyRef) {
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin
            .reserve(&tenant(), &region())
            .expect("reserve index DEK");
        (pin, key_ref)
    }

    fn ids_result(ids: &[&str], zookie: &str) -> ListObjectsResult {
        ListObjectsResult::Ids {
            ids: ids
                .iter()
                .map(|s| myelin_identity::ObjectId((*s).into()))
                .collect(),
            zookie: Zookie(zookie.into()),
        }
    }

    #[test]
    fn cache_ttl_must_not_exceed_revocation_sla() {
        assert!(CacheTtl::bounded(60, 300).is_ok());
        assert!(CacheTtl::bounded(300, 300).is_ok());
        let err = CacheTtl::bounded(301, 300).expect_err("TTL > revocation SLA must be rejected");
        assert_eq!(
            err,
            TtlExceedsRevocationSla {
                ttl_secs: 301,
                revocation_sla_secs: 300
            }
        );
    }

    #[test]
    fn zookie_bucket_is_the_revision_suffix() {
        assert_eq!(zookie_bucket("z@5"), 5);
        assert_eq!(zookie_bucket("z@9"), 9);
        assert_eq!(zookie_bucket("z-no-suffix"), 0);
        assert_ne!(
            zookie_bucket("z@5"),
            zookie_bucket("z@9"),
            "different buckets, different entries"
        );
    }

    #[test]
    fn strong_reads_bypass_the_cache() {
        assert!(
            should_bypass(&strong("z@7")),
            "a strong read bypasses the cache"
        );
        assert!(
            !should_bypass(&bounded("z@7")),
            "a bounded read may use the cache"
        );
    }

    #[test]
    fn s5_caches_and_hits_on_default_consistency() {
        let (pin, key_ref) = pin_with_dek();
        let cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin);
        let computed = AtomicU64::new(0);
        let run = || {
            computed.fetch_add(1, Ordering::SeqCst);
            ids_result(&["d1", "d2"], "z@5")
        };

        let at = bounded("z@5");
        let r1 = cache
            .get_or_compute(&tenant(), &region(), &subject(), &ty(), &at, &key_ref, run)
            .unwrap();
        let r2 = cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                &ty(),
                &at,
                &key_ref,
                || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    ids_result(&["UNEXPECTED"], "z@5")
                },
            )
            .unwrap();
        assert_eq!(r1, r2, "the cached result is byte-identical");
        assert_eq!(
            computed.load(Ordering::SeqCst),
            1,
            "computed exactly once (the second was a hit)"
        );
        assert_eq!(cache.stats().hits(), 1);
        assert_eq!(cache.stats().misses(), 1);
    }

    #[test]
    fn s5_strong_read_bypasses_and_never_caches() {
        let (pin, key_ref) = pin_with_dek();
        let cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin);

        let r = cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                &ty(),
                &strong("z@5"),
                &key_ref,
                || ids_result(&["fresh"], "z@5"),
            )
            .unwrap();
        assert_eq!(r, ids_result(&["fresh"], "z@5"));
        assert_eq!(cache.stats().bypasses(), 1);
        assert!(!cache
            .probe_recoverable(&tenant(), &region(), &subject(), &ty(), "z@5", &key_ref)
            .unwrap());
    }

    #[test]
    fn s5_no_cross_zookie_bleed() {
        let (pin, key_ref) = pin_with_dek();
        let cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin);
        let computed = AtomicU64::new(0);

        cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                &ty(),
                &bounded("z@5"),
                &key_ref,
                || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    ids_result(&["PUB-1", "SECRET-9"], "z@5")
                },
            )
            .unwrap();
        let r = cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                &ty(),
                &bounded("z@9"),
                &key_ref,
                || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    ids_result(&["PUB-1"], "z@9")
                },
            )
            .unwrap();
        assert_eq!(
            r,
            ids_result(&["PUB-1"], "z@9"),
            "the newer bucket recomputed (no bleed)"
        );
        assert_eq!(
            computed.load(Ordering::SeqCst),
            2,
            "both buckets computed - no cross-bucket hit"
        );
    }

    #[test]
    fn s5_entry_expires_at_the_ttl_boundary() {
        let (pin, key_ref) = pin_with_dek();
        let clock = std::sync::Arc::new(TestClock::at(1_000));
        let cache = FilterCache::with_clock(
            CacheTtl::bounded(60, 300).unwrap(),
            pin,
            Box::new(SharedClock(clock.clone())),
        );
        let at = bounded("z@5");

        cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                &ty(),
                &at,
                &key_ref,
                || ids_result(&["d1"], "z@5"),
            )
            .unwrap();
        assert!(cache
            .probe_recoverable(&tenant(), &region(), &subject(), &ty(), "z@5", &key_ref)
            .unwrap());

        clock.advance(60);
        let computed = AtomicU64::new(0);
        cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                &ty(),
                &at,
                &key_ref,
                || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    ids_result(&["UNEXPECTED"], "z@5")
                },
            )
            .unwrap();
        assert_eq!(
            computed.load(Ordering::SeqCst),
            0,
            "age == TTL is still fresh (inclusive)"
        );

        clock.advance(1);
        cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                &ty(),
                &at,
                &key_ref,
                || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    ids_result(&["d1-fresh"], "z@5")
                },
            )
            .unwrap();
        assert_eq!(
            computed.load(Ordering::SeqCst),
            1,
            "expired past the TTL → recomputed"
        );
        assert_eq!(cache.stats().expired(), 1, "the expiry is recorded");
    }

    #[test]
    fn s5_clock_rollback_expires_visibility_instead_of_extending_it() {
        let (pin, key_ref) = pin_with_dek();
        let clock = Arc::new(TestClock::at(1_000));
        let cache = FilterCache::with_clock(
            CacheTtl::bounded(60, 300).unwrap(),
            pin,
            Box::new(SharedClock(clock.clone())),
        );
        let at = bounded("z@5");

        cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                &ty(),
                &at,
                &key_ref,
                || ids_result(&["previously-visible"], "z@5"),
            )
            .unwrap();
        clock.set(999);

        let refreshed = cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                &ty(),
                &at,
                &key_ref,
                || ids_result(&["visible-now"], "z@5"),
            )
            .unwrap();

        assert_eq!(refreshed, ids_result(&["visible-now"], "z@5"));
        assert_eq!(
            cache.stats().expired(),
            1,
            "an impossible cache timeline is observable as expiry"
        );
    }

    #[test]
    fn s5_dek_destroy_renders_cache_unrecoverable() {
        let (pin, key_ref) = pin_with_dek();
        let cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin.clone());
        let at = bounded("z@5");

        cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                &ty(),
                &at,
                &key_ref,
                || ids_result(&["d1"], "z@5"),
            )
            .unwrap();
        assert!(cache
            .probe_recoverable(&tenant(), &region(), &subject(), &ty(), "z@5", &key_ref)
            .unwrap());

        assert!(
            pin.destroy_tenant_index_dek(&tenant(), &region()).unwrap(),
            "the index DEK was present"
        );

        let err = cache
            .probe_recoverable(&tenant(), &region(), &subject(), &ty(), "z@5", &key_ref)
            .expect_err("a destroyed DEK makes the cached entry unrecoverable (crypto-shred)");
        let _ = err;

        let computed = AtomicU64::new(0);
        let recompute = cache.get_or_compute(
            &tenant(),
            &region(),
            &subject(),
            &ty(),
            &at,
            &key_ref,
            || {
                computed.fetch_add(1, Ordering::SeqCst);
                ids_result(&["d1"], "z@5")
            },
        );
        assert!(
            recompute.is_err(),
            "a destroyed DEK makes the re-seal fail loudly (no plaintext at rest)"
        );
        assert_eq!(
            computed.load(Ordering::SeqCst),
            1,
            "the value was still computed from source"
        );
        assert!(
            cache.stats().shredded() >= 1,
            "the crypto-shred is recorded"
        );
    }

    fn ranked(docs: &[(&str, f32)], zookie: &str) -> RankedResults {
        RankedResults {
            hits: docs
                .iter()
                .map(|(d, s)| RankedResult {
                    doc_id: (*d).into(),
                    score: *s,
                })
                .collect(),
            zookie: zookie.into(),
            post_fetch_fields: vec!["rollup:total".into()],
        }
    }

    #[test]
    fn result_cache_round_trips_ranked_results() {
        let r = ranked(&[("d1", 1.5), ("d2", 0.25)], "z@5");
        let bytes = encode_results(&r);
        let back = decode_results(&bytes);
        assert_eq!(r, back, "the sealed page decodes byte-exact");
    }

    #[test]
    fn result_cache_caches_and_hits() {
        let (pin, key_ref) = pin_with_dek();
        let cache = ResultCache::new(CacheTtl::bounded(60, 300).unwrap(), pin);
        let computed = AtomicU64::new(0);
        let at = bounded("z@5");

        let r1 = cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                0xABCD,
                &at,
                &key_ref,
                || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    ranked(&[("d1", 2.0)], "z@5")
                },
            )
            .unwrap();
        let r2 = cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                0xABCD,
                &at,
                &key_ref,
                || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    ranked(&[("UNEXPECTED", 9.0)], "z@5")
                },
            )
            .unwrap();
        assert_eq!(r1, r2);
        assert_eq!(computed.load(Ordering::SeqCst), 1, "the engine ran once");
        assert_eq!(cache.stats().hits(), 1);
    }

    #[test]
    fn result_clock_rollback_recomputes_instead_of_serving_old_hits() {
        let (pin, key_ref) = pin_with_dek();
        let clock = Arc::new(TestClock::at(1_000));
        let cache = ResultCache::with_clock(
            CacheTtl::bounded(60, 300).unwrap(),
            pin,
            Box::new(SharedClock(clock.clone())),
        );
        let at = bounded("z@5");

        cache
            .get_or_compute(&tenant(), &region(), &subject(), 7, &at, &key_ref, || {
                ranked(&[("previously-visible", 1.0)], "z@5")
            })
            .unwrap();
        clock.set(999);

        let refreshed = cache
            .get_or_compute(&tenant(), &region(), &subject(), 7, &at, &key_ref, || {
                ranked(&[("visible-now", 1.0)], "z@5")
            })
            .unwrap();

        assert_eq!(refreshed, ranked(&[("visible-now", 1.0)], "z@5"));
        assert_eq!(cache.stats().expired(), 1);
    }

    #[test]
    fn failed_result_seals_release_every_inflight_query() {
        let (pin, key_ref) = pin_with_dek();
        let cache = ResultCache::new(CacheTtl::bounded(60, 300).unwrap(), pin.clone());
        assert!(pin.destroy_tenant_index_dek(&tenant(), &region()).unwrap());

        for query_hash in 0..32 {
            let result = cache.get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                query_hash,
                &bounded("z@5"),
                &key_ref,
                || ranked(&[("cannot-be-sealed", 1.0)], "z@5"),
            );
            assert!(result.is_err(), "a shredded cache DEK fails loudly");
        }

        assert!(
            cache
                .inflight
                .lock()
                .expect("result cache inflight poisoned")
                .is_empty(),
            "failed user queries must not accumulate permanent coalescing entries"
        );
    }

    #[test]
    fn result_cache_coalesces_concurrent_identical_requests() {
        use std::sync::Barrier;
        let (pin, key_ref) = pin_with_dek();
        let cache = Arc::new(ResultCache::new(CacheTtl::bounded(60, 300).unwrap(), pin));
        let engine_runs = Arc::new(AtomicU64::new(0));
        let n = 8;
        let barrier = Arc::new(Barrier::new(n));

        std::thread::scope(|scope| {
            for _ in 0..n {
                let cache = cache.clone();
                let engine_runs = engine_runs.clone();
                let barrier = barrier.clone();
                let key_ref = key_ref.clone();
                scope.spawn(move || {
                    barrier.wait();
                    let at = bounded("z@5");
                    cache
                        .get_or_compute(
                            &tenant(),
                            &region(),
                            &subject(),
                            0x1234,
                            &at,
                            &key_ref,
                            || {
                                engine_runs.fetch_add(1, Ordering::SeqCst);
                                std::thread::sleep(std::time::Duration::from_millis(20));
                                ranked(&[("d1", 1.0)], "z@5")
                            },
                        )
                        .unwrap();
                });
            }
        });

        assert_eq!(
            engine_runs.load(Ordering::SeqCst),
            1,
            "the engine ran exactly once for N concurrent identical requests (coalesced)"
        );
        assert!(
            cache.stats().coalesced() >= 1,
            "at least one request coalesced onto the first"
        );
    }

    #[test]
    fn result_cache_zookie_bucketed_and_strong_bypass() {
        let (pin, key_ref) = pin_with_dek();
        let cache = ResultCache::new(CacheTtl::bounded(60, 300).unwrap(), pin);
        let computed = AtomicU64::new(0);

        cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                7,
                &bounded("z@5"),
                &key_ref,
                || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    ranked(&[("d1", 1.0)], "z@5")
                },
            )
            .unwrap();
        cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                7,
                &bounded("z@9"),
                &key_ref,
                || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    ranked(&[("d1", 1.0)], "z@9")
                },
            )
            .unwrap();
        assert_eq!(
            computed.load(Ordering::SeqCst),
            2,
            "different buckets, different entries"
        );

        cache
            .get_or_compute(
                &tenant(),
                &region(),
                &subject(),
                7,
                &strong("z@5"),
                &key_ref,
                || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    ranked(&[("d1", 1.0)], "z@5")
                },
            )
            .unwrap();
        assert_eq!(cache.stats().bypasses(), 1, "the strong read bypassed");
    }

    #[test]
    fn result_cache_dek_destroy_renders_unrecoverable() {
        let (pin, key_ref) = pin_with_dek();
        let cache = ResultCache::new(CacheTtl::bounded(60, 300).unwrap(), pin.clone());
        let at = bounded("z@5");

        cache
            .get_or_compute(&tenant(), &region(), &subject(), 5, &at, &key_ref, || {
                ranked(&[("d1", 1.0)], "z@5")
            })
            .unwrap();
        assert!(cache
            .probe_recoverable(&tenant(), &region(), &subject(), 5, "z@5", &key_ref)
            .unwrap());

        assert!(pin.destroy_tenant_index_dek(&tenant(), &region()).unwrap());
        let err = cache
            .probe_recoverable(&tenant(), &region(), &subject(), 5, "z@5", &key_ref)
            .expect_err("a destroyed DEK makes the cached result unrecoverable");
        let _ = err;
    }

    #[test]
    fn hit_ratio_excludes_bypasses_and_is_absent_over_zero() {
        let s = CacheStats::new();
        assert_eq!(s.hit_ratio_pct(), None, "no ratio over zero reads");
        s.record_hit();
        s.record_hit();
        s.record_hit();
        s.record_miss();
        s.record_bypass();
        assert_eq!(
            s.hit_ratio_pct(),
            Some(75),
            "3 hits / 4 reads = 75% (the bypass is excluded)"
        );
    }

    struct SharedClock(std::sync::Arc<TestClock>);
    impl Clock for SharedClock {
        fn now_secs(&self) -> u64 {
            self.0.now_secs()
        }
    }
}
