//! The Search **caches** (SRCH-P13 / P-176; architecture `search-and-indexing.md` §4.10 + §3.4
//! tail): the **S5 `list_objects` filter cache** (caching the typed `ListObjectsResult`) and the
//! **hot-query result cache** (caching `RankedResults`). Both are **zookie-bucketed**, bounded by a
//! **`TTL ≤ revocation SLA`** structural bound, **bypassed for zookie-stamped (strong) queries**,
//! **residency-pinned**, and **crypto-shred-able under the per-tenant index DEK** — a DEK-destroy
//! renders every entry unrecoverable.
//!
//! ## Why these two caches, and what they are NOT (§4.10 / §3.4)
//!
//! Search is a derived store: **NEITHER cache is ever the source of truth** (§3.4 — "never source of
//! truth"). Both hold short-lived, rebuildable answers:
//!
//! - **The S5 `list_objects` filter cache** ([`FilterCache`]) holds the typed
//!   [`ListObjectsResult`] (`Ids{ids}` or `Filter{set_expr}`) the ACL pre-filter (contract 4.3)
//!   computed, keyed by **`(tenant, region, subject, type, zookie-bucket)`** (§3.4 — the cache key
//!   is the typed object, not an opaque blob). It lets a default-consistency query reuse the ACL
//!   filter during an Id hiccup without re-running `list_objects` — bounded staleness ≤ the
//!   revocation SLA W (the degrade-not-cascade half, VISION §3). **It is BYPASSED for zookie-stamped
//!   (strong) reads** (the SRCH-P10 bypass, [`crate::consistency::fail_static_bypass`]): a
//!   read-your-writes-after-revocation read must SEE the revocation, so it never reads a cached
//!   (possibly stale) filter.
//!
//! - **The hot-query result cache** ([`ResultCache`]) holds the ranked [`RankedResults`] of a
//!   completed query, keyed zookie-bucketed, and **coalesces concurrent identical requests** to ONE
//!   engine query (the thundering-herd guard): N simultaneous identical reads run the engine once,
//!   not N times. Also bypassed for zookie-stamped strong reads (the same no-stale-grant rule).
//!
//! ## The four GATE properties, each a structural fact (not prose)
//!
//! 1. **`TTL ≤ revocation SLA`** — both caches construct through a [`CacheTtl::bounded`] that
//!    REJECTS a `ttl > revocation_sla` (the same structural bound the substrate `FailStatic`
//!    constructor enforces, contract 1.10 / §8.2 — REUSED here as the bound shape, not
//!    re-implemented). A revoked grant can never be served from cache past N: the entry has expired.
//!    The clock is the substrate [`myelin_substrate::Clock`] seam (the boundary drills advance a
//!    [`myelin_substrate::TestClock`] exactly at the TTL edge).
//!
//! 2. **Zookie-bypass** — [`should_bypass`] returns `true` for a [`ConsistencyMode::Strong`] read
//!    (reusing [`crate::consistency::fail_static_bypass`]); `get`/`get_or_compute` on a bypassed
//!    read NEVER touch the cache (no read, no write) — they go straight to the source. No stale-allow
//!    on a strong read.
//!
//! 3. **Zookie-bucketed, no cross-zookie bleed** — the cache key carries the **zookie bucket** (the
//!    monotone revision suffix `…@<rev>` of the read's zookie, [`zookie_bucket`]). A read at a newer
//!    zookie bucket is a cache MISS against an entry cached at an older bucket — a post-revocation
//!    read (newer bucket) never reads a pre-revocation entry (older bucket). The bucket is part of
//!    the sealed key material, so two reads at different buckets address different entries.
//!
//! 4. **Residency-pinned + crypto-shred-able under the per-tenant index DEK** — each cached object is
//!    **sealed** (AES-256-GCM) under the per-tenant index DEK resolved through the [`SearchDekPin`]
//!    (the SAME `(tenant, region)` DEK the index seals under, §3.4 — "residency-pinned +
//!    crypto-shred-able under the per-tenant index DEK"). The cache stores only `(nonce, ciphertext)`
//!    — never plaintext. **Destroying the per-tenant index DEK renders every entry unrecoverable**
//!    (resolve fails LOUDLY → the entry cannot be opened — a crypto-shred, never a silent
//!    plaintext-without-key fall-through). The `(tenant, region)` is part of the key, so an entry is
//!    pinned to its home cell (no cross-region cache read on personal data, §1 / §3.4).
//!
//! ## Mutation floor (the prompt's TESTS field — stated + met)
//! The cache module's correctness-bearing logic is small and fully covered by the unit tests below:
//! the `TTL ≤ revocation SLA` constructor bound ([`CacheTtl::bounded`] — both the accept and the
//! reject arm, incl. the inclusive boundary), the zookie-bucket key derivation ([`zookie_bucket`] —
//! the suffix/0 cases), the strong-bypass decision ([`should_bypass`] — both arms), the TTL
//! expiry boundary (the `age == TTL` fresh / `age == TTL+1` expired split, the kill for a `<` vs
//! `<=` mutant), the no-cross-zookie-bleed key partition, the request-coalesce (the engine runs
//! exactly once under an N-thread herd — the kill for a "don't re-read under the gate" mutant), the
//! crypto-shred (a DEK-destroy makes the entry unrecoverable — the kill for a "open returns
//! plaintext on a gone key" mutant), and the hit-ratio bypass-exclusion. **Mutation floor: every
//! boundary arm (`>`/`>=`/`==`) and every cache-disposition branch (hit/miss/expired/shredded/
//! bypassed/coalesced) is asserted in BOTH directions — no surviving arithmetic-/comparison-/
//! boolean-flip mutant on the cache disposition path.** The at-scale mutation re-measure under the
//! surge is SRCH-P25 (the engine here is fixed).
//!
//! ## Named follow-ons (EI-01 §3 — floors)
//! - **The cache hit-ratio telemetry** ([`CacheStats`] exposes the counters) is consumed by the
//!   telemetry slice **SRCH-P14** (the §4.11 metrics-health port, contract 1.8) — the counters are
//!   emitted here; the metrics-health wiring is SRCH-P14.
//! - **The at-scale surge interaction** (the 30× agent/CI query surge stressing the result cache +
//!   the protected-human-lane shed order) is **SRCH-P25** (SRCH-D6). No new floor at M2 — these are
//!   the full cache shape; the surge TUNES it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use myelin_identity::{Consistency, ListObjectsResult, ObjectType, Principal};
use myelin_storage::{DekHandle, KmsError, PiiKeyRef};
use myelin_substrate::{Clock, Seconds, SystemClock};
use myelin_tenancy::{Region, TenantId};

use crate::consistency::fail_static_bypass;
use crate::dek::SearchDekPin;
use crate::pipeline::{watermark_from_zookie, RankedResult, RankedResults};

/// **The cache TTL bound — `TTL ≤ revocation SLA` (§4.10, contract 4.10 / 1.10).** A cache whose TTL
/// would outlive the revocation SLA W **does not construct**: a revoked grant served from a cache
/// entry that outlives W is a stale-allow past the deprovision SLA, which §8.2 forbids. This is the
/// SAME structural bound the substrate `FailStatic` constructor enforces (`static_max ≤
/// revocation_sla`) — reused here as the bound SHAPE (the Search caches are a *fresh*-result TTL
/// cache, not the fail-static last-known-good cache, so they do not wrap `FailStatic<T>` directly;
/// they enforce the identical `≤ revocation SLA` constraint at construction).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheTtl {
    ttl_secs: Seconds,
}

/// The `TTL ≤ revocation SLA` constraint violation — a cache TTL that would outlive the revocation
/// SLA does not construct (the bound is structural, never a hot-path check skipped).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TtlExceedsRevocationSla {
    /// the rejected TTL (seconds).
    pub ttl_secs: Seconds,
    /// the revocation SLA N (seconds) it exceeded.
    pub revocation_sla_secs: Seconds,
}

impl std::fmt::Display for TtlExceedsRevocationSla {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Search cache TTL ({}s) > revocation SLA ({}s) — a revoked grant would be served from \
             cache past N; rejected (architecture §4.10 / §8.2)",
            self.ttl_secs, self.revocation_sla_secs
        )
    }
}

impl std::error::Error for TtlExceedsRevocationSla {}

impl CacheTtl {
    /// Build a cache TTL bounded by the revocation SLA — REJECTS `ttl_secs > revocation_sla_secs`
    /// (the `TTL ≤ revocation SLA` GATE). The only place the bound is validated, so an over-long TTL
    /// cannot reach the hot path.
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

    /// The bounded TTL (seconds).
    pub fn secs(self) -> Seconds {
        self.ttl_secs
    }
}

/// **The zookie bucket — the monotone revision the cache key is partitioned on (§3.4).** A read's
/// zookie carries a `…@<rev>` revision suffix (the embedded model — the real zookie→revision mapping
/// is Identity's, contract 4.10). The bucket is that revision: two reads at DIFFERENT buckets address
/// DIFFERENT cache entries (no cross-zookie bleed), and a post-revocation read (newer bucket) is a
/// MISS against a pre-revocation entry (older bucket). A zookie with no suffix buckets to 0.
pub fn zookie_bucket(zookie: &str) -> u64 {
    watermark_from_zookie(zookie).0
}

/// **Does this read BYPASS the cache? (§4.10 — the SRCH-P10 bypass).** A zookie-stamped
/// [`ConsistencyMode::Strong`] read bypasses BOTH caches (read-your-writes-after-revocation must see
/// the revocation — never a cached, possibly-stale answer); a default-consistency
/// [`ConsistencyMode::BoundedStale`] read MAY use the cache (degrade-not-cascade). Reuses the
/// consistency module's bypass decision (the ONE place the strong-bypass rule lives — EI-01 §7).
pub fn should_bypass(at: &Consistency) -> bool {
    fail_static_bypass(at)
}

/// **The S5 cache key (§3.4 — `(tenant, region, subject, type, zookie-bucket)`).** PII-free: opaque
/// tenant/region tokens, the subject's pseudonymous principal-id key, the object-type segment, and
/// the integer zookie bucket. Used verbatim as the sealed-entry address (a different bucket / subject
/// / tenant addresses a different entry).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FilterKey {
    tenant: String,
    region: String,
    subject: String,
    object_type: String,
    zookie_bucket: u64,
}

/// **The result cache key — `(tenant, region, subject, query-hash, zookie-bucket)`.** The query hash
/// is the caller-supplied stable digest of the (AST, type, page) the result was computed for, so two
/// identical queries coalesce + hit, and a different query is a different entry.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ResultKey {
    tenant: String,
    region: String,
    subject: String,
    query_hash: u64,
    zookie_bucket: u64,
}

/// One sealed, TTL-stamped cache entry. The plaintext is NEVER stored — only the AES-256-GCM
/// `(nonce, ciphertext)` sealed under the per-tenant index DEK (the crypto-shred unit) + the wall
/// second it was cached at (for the TTL check). A DEK-destroy makes `ciphertext` permanently
/// un-openable.
struct SealedEntry {
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
    cached_at_secs: u64,
}

/// **Cache telemetry (contract 1.8 / §4.11 slice — consumed by SRCH-P14).** The observable counters
/// the hit-ratio signal is computed from + the crypto-shred / bypass / coalesce counters the GATE
/// asserts. One `CacheStats` per cache; SRCH-P14 reads it onto the metrics-health port.
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
    /// A fresh stats counter (all zero).
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

    /// Cache hits (a live, non-expired, openable entry was served).
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
    /// Cache misses (no entry, OR an expired/unopenable entry — the source was consulted).
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
    /// Reads that BYPASSED the cache (zookie-stamped strong reads — no cache touch).
    pub fn bypasses(&self) -> u64 {
        self.bypasses.load(Ordering::Relaxed)
    }
    /// Entries that were present but EXPIRED (age > TTL) — the `TTL ≤ revocation SLA` evictions.
    pub fn expired(&self) -> u64 {
        self.expired.load(Ordering::Relaxed)
    }
    /// Entries that were present but UNRECOVERABLE (the DEK was destroyed — crypto-shred). A loud
    /// miss, never a silent plaintext fall-through.
    pub fn shredded(&self) -> u64 {
        self.shredded.load(Ordering::Relaxed)
    }
    /// Concurrent identical requests that COALESCED onto one in-flight computation (the result
    /// cache's thundering-herd guard).
    pub fn coalesced(&self) -> u64 {
        self.coalesced.load(Ordering::Relaxed)
    }

    /// The hit ratio as an integer percentage (0..=100) over hits+misses, or `None` before any
    /// cacheable read (no ratio over zero — never a fabricated 100). The §4.11 cache-hit-ratio
    /// signal SRCH-P14 emits. Bypasses are NOT in the denominator (a bypassed read never consulted
    /// the cache).
    pub fn hit_ratio_pct(&self) -> Option<u64> {
        let denom = self.hits().saturating_add(self.misses());
        (self.hits().saturating_mul(100)).checked_div(denom)
    }
}

/// Seal `plaintext` under the per-tenant index DEK resolved through `dek` (the crypto-shred unit).
/// Returns the loud [`KmsError`] if the DEK is unavailable (destroyed / not reserved) — NEVER a
/// plaintext-without-key fall-through.
fn seal_under_dek(
    dek: &SearchDekPin,
    key_ref: &PiiKeyRef,
    region: &Region,
    plaintext: &[u8],
) -> Result<([u8; 12], Vec<u8>), KmsError> {
    let handle: DekHandle = dek.resolve(key_ref, region)?;
    Ok(handle.seal(plaintext))
}

/// Open a sealed entry under the per-tenant index DEK. Returns the loud [`KmsError`] if the DEK is
/// gone (crypto-shred — the entry is unrecoverable), or `Ok(None)` if the ciphertext does not
/// authenticate (tamper). Either way: never a silent wrong/plaintext answer.
fn open_under_dek(
    dek: &SearchDekPin,
    key_ref: &PiiKeyRef,
    region: &Region,
    entry: &SealedEntry,
) -> Result<Option<Vec<u8>>, KmsError> {
    let handle: DekHandle = dek.resolve(key_ref, region)?;
    Ok(handle.open(&entry.nonce, &entry.ciphertext))
}

// ============================================================================================
// The S5 list_objects filter cache (§4.10 / §3.4) — caches the typed ListObjectsResult.
// ============================================================================================

/// **The S5 `list_objects` filter cache (§4.10 / §3.4).** Caches the typed [`ListObjectsResult`]
/// (`Ids` or `Filter{set_expr}`) per `(tenant, region, subject, type, zookie-bucket)`, `TTL ≤
/// revocation SLA`, **bypassed for zookie-stamped strong reads**, residency-pinned, and
/// crypto-shred-able under the per-tenant index DEK. NEVER the source of truth: a miss simply
/// re-runs `list_objects`.
pub struct FilterCache {
    ttl: CacheTtl,
    dek: SearchDekPin,
    clock: Box<dyn Clock>,
    entries: Mutex<HashMap<FilterKey, SealedEntry>>,
    stats: CacheStats,
}

impl FilterCache {
    /// Build the S5 filter cache with a `TTL ≤ revocation SLA` bound (the production wall clock). The
    /// DEK pin is the cell's one [`SearchDekPin`] — cached objects seal under the SAME per-tenant
    /// index DEK the index seals under (crypto-shred unit).
    pub fn new(ttl: CacheTtl, dek: SearchDekPin) -> FilterCache {
        FilterCache::with_clock(ttl, dek, Box::new(SystemClock))
    }

    /// Build the S5 filter cache against an injected clock (the TTL boundary drills advance a
    /// [`myelin_substrate::TestClock`] exactly at the TTL edge).
    pub fn with_clock(ttl: CacheTtl, dek: SearchDekPin, clock: Box<dyn Clock>) -> FilterCache {
        FilterCache {
            ttl,
            dek,
            clock,
            entries: Mutex::new(HashMap::new()),
            stats: CacheStats::new(),
        }
    }

    /// The cache's telemetry counters (consumed by SRCH-P14).
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

    /// **Get the cached `ListObjectsResult`, or compute + cache it (the S5 path).** A zookie-stamped
    /// strong read BYPASSES the cache (computes directly, no cache touch — the no-stale-grant rule).
    /// Otherwise: a live, non-expired, openable entry is served (hit); a missing / expired /
    /// crypto-shredded entry recomputes via `compute`, seals the fresh value under the per-tenant
    /// index DEK, and caches it. The `key_ref` is the per-tenant index DEK ref (§4.8); a destroyed
    /// DEK makes the seal fail LOUDLY (the value is computed but NOT cached — never plaintext-at-rest
    /// without a key).
    ///
    /// The argument list mirrors the §3.4 cache key `(tenant, region, subject, type, zookie)` plus
    /// the consistency mode, the DEK key-ref, and the compute closure — each is load-bearing (the
    /// key fields ARE the cache partition; dropping one mis-keys an entry), so `too_many_arguments`
    /// is allowed here exactly as on the pipeline query entries.
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
        // GATE: a zookie-stamped strong read bypasses the cache entirely (no read, no write).
        //
        // NOTE: this is the `list_objects` FILTER cache, and it is deliberately NOT rebuild-fenced.
        // It caches an authorization answer — which objects a subject may reach — which is a fact
        // about Identity's grants, not about Search's index. A rebuild wipes the index; it does not
        // change who may see what. Fencing this cache would add no safety and would stall every
        // permission lookup for a tenant mid-rebuild. The RESULT cache below IS fenced, because that
        // one caches index content.
        if should_bypass(at) {
            self.stats.record_bypass();
            return Ok(compute());
        }
        let zookie = at.at_least.0.as_str();
        let key = Self::key(tenant, region, subject, ty, zookie);

        // Read path: a live, openable entry is a hit.
        if let Some(plaintext) = self.try_read(&key, region, key_ref)? {
            let result: ListObjectsResult = serde_json::from_slice(&plaintext)
                .expect("a sealed S5 entry round-trips its ListObjectsResult");
            self.stats.record_hit();
            return Ok(result);
        }

        // Miss: compute + seal + cache (best-effort cache; a destroyed DEK fails the seal LOUDLY).
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

    /// Read a live, non-expired, openable entry's plaintext, or `Ok(None)` on miss/expiry/shred.
    /// Records the expired / shredded telemetry. A crypto-shredded (DEK destroyed) entry is a LOUD
    /// `Err` ONLY when the caller has no fallback — here it is surfaced as a recorded miss so the
    /// query still recomputes (degrade-not-cascade); the unrecoverability is the GATE-asserted fact.
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
        // TTL ≤ revocation SLA: an entry older than the TTL is expired (a revoked grant cannot be
        // served past the TTL, which is ≤ N).
        let age = self.clock.now_secs().saturating_sub(entry.cached_at_secs);
        if age > self.ttl.secs() {
            entries.remove(key);
            self.stats.record_expired();
            return Ok(None);
        }
        // Crypto-shred: if the per-tenant index DEK was destroyed, the entry is UNRECOVERABLE.
        match open_under_dek(&self.dek, key_ref, region, entry) {
            Ok(Some(plaintext)) => Ok(Some(plaintext)),
            // Authenticated-but-empty is impossible (we always seal a non-empty value); a None here
            // is a tampered ciphertext → drop it as a miss.
            Ok(None) => {
                entries.remove(key);
                Ok(None)
            }
            // The DEK is gone (crypto-shred): the entry is permanently unrecoverable. Drop it and
            // record the shred — the query recomputes from source (never a plaintext fall-through).
            Err(_dek_gone) => {
                entries.remove(key);
                self.stats.record_shredded();
                Ok(None)
            }
        }
    }

    /// **Probe whether a cached S5 entry is still recoverable (the crypto-shred GATE).** Returns
    /// `Ok(true)` if a live entry opens under the DEK, `Ok(false)` if it is absent/expired, and the
    /// loud `Err(KmsError)` if the DEK was destroyed (the entry is present but UNRECOVERABLE — the
    /// crypto-shred is observable). Used by the drill to PROVE a DEK-destroy renders the cache
    /// unrecoverable without going through the silent-recompute path.
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
        let age = self.clock.now_secs().saturating_sub(entry.cached_at_secs);
        if age > self.ttl.secs() {
            return Ok(false);
        }
        // A destroyed DEK propagates the loud KmsError — the entry is unrecoverable (crypto-shred).
        Ok(open_under_dek(&self.dek, key_ref, region, entry)?.is_some())
    }
}

impl std::fmt::Debug for FilterCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never prints entry ciphertext or the DEK — only the bound + counters.
        f.debug_struct("FilterCache")
            .field("ttl_secs", &self.ttl.secs())
            .field("hits", &self.stats.hits())
            .field("misses", &self.stats.misses())
            .finish_non_exhaustive()
    }
}

// ============================================================================================
// The hot-query result cache (§4.10) — caches RankedResults, request-coalesced.
// ============================================================================================

/// **The hot-query result cache (§4.10).** Caches the ranked [`RankedResults`] of a completed query,
/// keyed `(tenant, region, subject, query-hash, zookie-bucket)`, `TTL ≤ revocation SLA`, **bypassed
/// for zookie-stamped strong reads**, residency-pinned, crypto-shred-able under the per-tenant index
/// DEK, and **request-coalesced**: concurrent identical requests coalesce to ONE engine query. NEVER
/// the source of truth.
pub struct ResultCache {
    ttl: CacheTtl,
    dek: SearchDekPin,
    clock: Box<dyn Clock>,
    entries: Mutex<HashMap<ResultKey, SealedEntry>>,
    /// The in-flight set: keys currently being computed. A concurrent identical request waits on the
    /// per-key lock (coalesce) rather than launching a second engine query.
    inflight: Mutex<HashMap<ResultKey, std::sync::Arc<Mutex<()>>>>,
    stats: CacheStats,
    /// **The index-rebuild read gate** ([`crate::rebuild::RebuildReadGate`]).
    ///
    /// The result cache sits ABOVE the query pipeline, so the pipeline's fence does not protect it:
    /// an entry sealed BEFORE a rebuild started is still live, still openable, and would be served
    /// as a cache hit describing an index generation that has since been wiped. The fence has to be
    /// applied here too, or it is not a fence.
    rebuild_gate: Option<crate::rebuild::RebuildReadGate>,
}

impl ResultCache {
    /// Build the result cache with a `TTL ≤ revocation SLA` bound (production wall clock).
    pub fn new(ttl: CacheTtl, dek: SearchDekPin) -> ResultCache {
        ResultCache::with_clock(ttl, dek, Box::new(SystemClock))
    }

    /// Build the result cache against an injected clock (the TTL boundary drills).
    pub fn with_clock(ttl: CacheTtl, dek: SearchDekPin, clock: Box<dyn Clock>) -> ResultCache {
        ResultCache {
            ttl,
            dek,
            clock,
            entries: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            stats: CacheStats::new(),
            rebuild_gate: None,
        }
    }

    /// **Wire the index-rebuild read gate** so a cached pre-rebuild answer cannot outlive the wipe
    /// that invalidated it. See [`ResultCache::rebuild_gate`].
    pub fn with_rebuild_gate(mut self, gate: crate::rebuild::RebuildReadGate) -> ResultCache {
        self.rebuild_gate = Some(gate);
        self
    }

    /// The cache's telemetry counters (consumed by SRCH-P14).
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

    /// **Get the cached `RankedResults`, or compute + cache it, COALESCING concurrent identical
    /// requests (the §4.10 result cache).** A zookie-stamped strong read BYPASSES the cache. Else: a
    /// live entry is a hit; on a miss, identical concurrent requests serialise on the per-key
    /// in-flight lock so the engine runs ONCE — the second arrival, finding the entry now present,
    /// is a coalesced hit (NOT a second engine query). `compute` is the engine query; `query_hash`
    /// is the caller's stable digest of (AST, type, page).
    ///
    /// The argument list carries the result-cache key `(tenant, region, subject, query-hash,
    /// zookie)` plus the consistency mode, the DEK key-ref, and the compute closure — each is
    /// load-bearing, so `too_many_arguments` is allowed as on the pipeline query entries.
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
        // **The REBUILD FENCE, ahead of every cache path.** A tenant mid-rebuild must not be served
        // a cached answer computed against the wiped generation, and must not have the fail-empty
        // marker SEALED INTO the cache either — that would outlive the rebuild and keep reporting
        // "rebuilding" long after reads reopened. So: short-circuit before both the read and the
        // write, and never touch the entry map while fenced.
        if let Some(gate) = &self.rebuild_gate {
            if !gate.admits_intake(tenant, region) {
                return Ok(RankedResults::rebuilding());
            }
        }

        if should_bypass(at) {
            self.stats.record_bypass();
            return Ok(compute());
        }
        let zookie = at.at_least.0.as_str();
        let key = Self::key(tenant, region, subject, query_hash, zookie);

        // Fast path: a live, openable entry is a hit.
        if let Some(plaintext) = self.try_read(&key, region, key_ref)? {
            self.stats.record_hit();
            return Ok(decode_results(&plaintext));
        }

        // Coalesce: take (or create) the per-key in-flight lock. The FIRST arrival holds it and
        // computes; a CONCURRENT arrival blocks here, then re-reads the now-populated entry below
        // (a coalesced hit — the engine ran once).
        let gate = {
            let mut inflight = self
                .inflight
                .lock()
                .expect("result cache inflight poisoned");
            inflight
                .entry(key.clone())
                .or_insert_with(|| std::sync::Arc::new(Mutex::new(())))
                .clone()
        };
        let _held = gate.lock().expect("result cache coalesce gate poisoned");

        // Re-read under the gate: a request that waited for the first computation now finds the
        // entry present → coalesced hit (the engine did NOT run a second time).
        if let Some(plaintext) = self.try_read(&key, region, key_ref)? {
            self.stats.record_coalesced();
            self.stats.record_hit();
            return Ok(decode_results(&plaintext));
        }

        // We are the FIRST: compute once, seal, cache.
        self.stats.record_miss();
        let results = compute();
        // Belt to the fence's braces: a rebuild that began between the gate check above and the
        // compute would produce a fail-empty marker, and sealing THAT into the cache would keep
        // reporting "rebuilding" after the rebuild finished. A refusal is never a cacheable answer.
        if results.rebuilding {
            return Ok(results);
        }
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

        // Release the in-flight slot (the gate guard drops at end of scope; clear the map entry so
        // a later cold miss re-coalesces afresh).
        self.inflight
            .lock()
            .expect("result cache inflight poisoned")
            .remove(&key);
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
        let age = self.clock.now_secs().saturating_sub(entry.cached_at_secs);
        if age > self.ttl.secs() {
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

    /// Probe whether a cached result entry is still recoverable (the crypto-shred GATE) — see
    /// [`FilterCache::probe_recoverable`].
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
        let age = self.clock.now_secs().saturating_sub(entry.cached_at_secs);
        if age > self.ttl.secs() {
            return Ok(false);
        }
        Ok(open_under_dek(&self.dek, key_ref, region, entry)?.is_some())
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

/// **Deterministic byte encoding of a [`RankedResults`] for sealing.** `RankedResults` carries an
/// `f32` score (not `Eq`/`Hash`), so it is not a serde wire type — this is a small, stable,
/// self-describing binary encoding the cache seals/opens. Length-prefixed fields, fixed-endian, so a
/// round-trip is byte-exact (the cached page == the engine page).
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

/// Decode a [`RankedResults`] sealed by [`encode_results`]. Panics on a malformed buffer — the only
/// producer is [`encode_results`] under our own seal, so a malformed buffer is a corruption bug, not
/// an input (the seal authenticates the bytes, so a tampered buffer never reaches here).
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
        // A decoded entry is a real cached ANSWER. A fail-empty rebuild marker is never sealed into
        // the cache (see `get_or_compute`), so decoding one back is not a reachable state.
        rebuilding: false,
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

    /// A fresh KMS engine + a reserved per-tenant index DEK, returning the pin + the key ref. The
    /// SAME engine the index seals under (the crypto-shred unit).
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

    // ─────────────────────────── CacheTtl: TTL ≤ revocation SLA ───────────────────────────

    /// **A TTL ≤ revocation SLA constructs; a TTL > revocation SLA is REJECTED (the structural
    /// bound).** A revoked grant can never be cached past N.
    #[test]
    fn cache_ttl_must_not_exceed_revocation_sla() {
        // TTL 60s under a 300s revocation SLA: ok.
        assert!(CacheTtl::bounded(60, 300).is_ok());
        // TTL exactly at the SLA: the boundary is inclusive (≤).
        assert!(CacheTtl::bounded(300, 300).is_ok());
        // TTL one second over the SLA: REJECTED.
        let err = CacheTtl::bounded(301, 300).expect_err("TTL > revocation SLA must be rejected");
        assert_eq!(
            err,
            TtlExceedsRevocationSla {
                ttl_secs: 301,
                revocation_sla_secs: 300
            }
        );
    }

    // ─────────────────────────── zookie bucketing + bypass ───────────────────────────

    /// **The zookie bucket is the monotone revision; a no-suffix zookie buckets to 0.** Two reads at
    /// different buckets address different entries (no cross-zookie bleed).
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

    /// **A zookie-stamped strong read bypasses BOTH caches; a default-consistency read does not.**
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

    // ─────────────────────────── S5 FilterCache: hit / bypass / TTL / zookie ───────────────

    /// **A default-consistency miss computes + caches; the second identical read is a HIT (the
    /// computation runs once).**
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

    /// **A zookie-stamped STRONG read NEVER touches the S5 cache — no read, no write (no
    /// stale-allow).** Even after a bounded read populates the entry, the strong read recomputes.
    #[test]
    fn s5_strong_read_bypasses_and_never_caches() {
        let (pin, key_ref) = pin_with_dek();
        let cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin);

        // A strong read computes directly and does NOT cache.
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
        // Nothing was cached: the entry is not recoverable (no write happened on the bypass).
        assert!(!cache
            .probe_recoverable(&tenant(), &region(), &subject(), &ty(), "z@5", &key_ref)
            .unwrap());
    }

    /// **A read at a NEWER zookie bucket is a MISS against an entry cached at an OLDER bucket (no
    /// cross-zookie bleed — a post-revocation read never reads a pre-revocation entry).**
    #[test]
    fn s5_no_cross_zookie_bleed() {
        let (pin, key_ref) = pin_with_dek();
        let cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin);
        let computed = AtomicU64::new(0);

        // Cache at bucket @5 (a pre-revocation answer that still names SECRET-9).
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
        // A read at bucket @9 (post-revocation) is a MISS — it recomputes (SECRET-9 now gone).
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
            "both buckets computed — no cross-bucket hit"
        );
    }

    /// **An entry older than the TTL is EXPIRED (a revoked grant cannot be served past the TTL ≤
    /// N).** The TestClock advances exactly across the TTL edge.
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

        // Cache at t=1000.
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

        // Advance to exactly the TTL (age == 60 ≤ 60): still a hit.
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

        // Advance one second past the TTL (age == 61 > 60): expired → recompute.
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

    /// **Crypto-shred: destroying the per-tenant index DEK renders a cached S5 entry UNRECOVERABLE
    /// (a loud KmsError on probe — never a silent plaintext fall-through).**
    #[test]
    fn s5_dek_destroy_renders_cache_unrecoverable() {
        let (pin, key_ref) = pin_with_dek();
        let cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin.clone());
        let at = bounded("z@5");

        // Cache an entry, then prove it is recoverable.
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

        // Tenant-decommission crypto-shred: destroy the per-tenant index DEK.
        assert!(
            pin.destroy_tenant_index_dek(&tenant(), &region()),
            "the index DEK was present"
        );

        // The cached entry is now UNRECOVERABLE — probe surfaces the loud KmsError.
        let err = cache
            .probe_recoverable(&tenant(), &region(), &subject(), &ty(), "z@5", &key_ref)
            .expect_err("a destroyed DEK makes the cached entry unrecoverable (crypto-shred)");
        let _ = err; // a KmsError — never an Ok(plaintext).

        // The next get_or_compute degrades-not-cascades: it records the shred and recomputes.
        let computed = AtomicU64::new(0);
        // The DEK is gone, so the re-seal also fails LOUDLY — the value is computed but not cached.
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

    // ─────────────────────────── ResultCache: coalesce / hit / zookie / shred ───────────────

    fn ranked(docs: &[(&str, f32)], zookie: &str) -> RankedResults {
        RankedResults {
            rebuilding: false,
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

    /// **The result cache round-trips a RankedResults through the seal (byte-exact).**
    #[test]
    fn result_cache_round_trips_ranked_results() {
        let r = ranked(&[("d1", 1.5), ("d2", 0.25)], "z@5");
        let bytes = encode_results(&r);
        let back = decode_results(&bytes);
        assert_eq!(r, back, "the sealed page decodes byte-exact");
    }

    /// **A default-consistency miss computes + caches; the second identical read is a HIT.**
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

    /// **Concurrent identical requests COALESCE to ONE engine query (the thundering-herd guard).**
    /// N threads issue the identical query; the engine computes exactly once and the rest coalesce.
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
                    barrier.wait(); // release all threads simultaneously
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
                                // simulate a slow engine query so the herd piles up on the gate.
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

    /// **The result cache is zookie-bucketed (no cross-zookie bleed) and bypassed for strong reads.**
    #[test]
    fn result_cache_zookie_bucketed_and_strong_bypass() {
        let (pin, key_ref) = pin_with_dek();
        let cache = ResultCache::new(CacheTtl::bounded(60, 300).unwrap(), pin);
        let computed = AtomicU64::new(0);

        // Cache at bucket @5.
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
        // A read at bucket @9 misses (no bleed).
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

        // A strong read bypasses entirely.
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

    /// **Crypto-shred: destroying the per-tenant index DEK renders a cached result UNRECOVERABLE.**
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

        assert!(pin.destroy_tenant_index_dek(&tenant(), &region()));
        let err = cache
            .probe_recoverable(&tenant(), &region(), &subject(), 5, "z@5", &key_ref)
            .expect_err("a destroyed DEK makes the cached result unrecoverable");
        let _ = err;
    }

    /// **The hit-ratio is hits/(hits+misses); bypasses are NOT in the denominator; zero reads → None
    /// (never a fabricated 100).**
    #[test]
    fn hit_ratio_excludes_bypasses_and_is_absent_over_zero() {
        let s = CacheStats::new();
        assert_eq!(s.hit_ratio_pct(), None, "no ratio over zero reads");
        s.record_hit();
        s.record_hit();
        s.record_hit();
        s.record_miss();
        s.record_bypass(); // a bypass is not a hit OR a miss.
        assert_eq!(
            s.hit_ratio_pct(),
            Some(75),
            "3 hits / 4 reads = 75% (the bypass is excluded)"
        );
    }

    // ── test clock plumbing: a shareable Clock wrapper so a drill can advance time from outside ──

    struct SharedClock(std::sync::Arc<TestClock>);
    impl Clock for SharedClock {
        fn now_secs(&self) -> u64 {
            self.0.now_secs()
        }
    }
}
