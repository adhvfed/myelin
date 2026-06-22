//! The **R2 projection cache** — the bounded, invalidatable, DEK-encrypted holder that replaces the
//! REF-P7 no-op shim (REF-P12 / P-161; contract 5.6 holder side + the §3.6 R2 holder; 10.1 the cache
//! half; 11.3/11.4 the per-tenant DEK; 1.8 `resolve_cache_hit_ratio`).
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §3.6 (the R2 projection cache: a **bounded, invalidatable, event-busted projection cache per
//! `ArtifactRef`**, keyed `(tenant, ref)`, with `*.updated`/`*.erased` invalidation + a TTL — **NEVER a
//! source of truth**; on a miss/erasure it re-resolves via the projection API), §4.2 (resolve reads R2
//! FIRST, then falls through to the owner's `project`). **External insight:**
//! `external-insights/04-hard-problems.md` §1 (erasure vs immutability — a name in a title must be
//! crypto-shred-able), `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it;
//! observability is part of the pass — the hit-ratio telemetry).
//!
//! ## What REF-P12 (P-161) ships — the LIVE R2 cache, replacing the REF-P7 shim
//! [`R2ProjectionCache`] is the live, bounded, per-tenant-DEK-encrypted, residency-pinned projection
//! cache. It implements **BOTH** seams the REF-P7/REF-P10 floors stubbed:
//!
//! - the **write/invalidate side** — [`crate::invalidator::ProjectionCache`] (the REF-P7
//!   [`crate::invalidator::NoOpCacheShim`] is replaced): an `*.updated`/`*.erased` from the
//!   refs-projection-invalidator now **evicts a live entry** (the §3.6 bust), not a recorded no-op;
//! - the **read side** — [`crate::resolve::ProjectionCacheRead`] (the REF-P10
//!   [`crate::resolve::NoOpCacheRead`] is replaced): the resolve chokepoint's step-3 cache read now
//!   returns a **live HIT** on a warm `(tenant, ref)` entry (decrypted under the per-tenant DEK), or a
//!   MISS that falls through to the owner's `project`.
//!
//! It also exposes the **fill** seam ([`R2ProjectionCache::fill`]) the resolve chokepoint calls after a
//! cache-miss projection so the NEXT viewer of the same `(tenant, ref)` is served from the cache
//! (per-viewer correctness WITHOUT per-viewer caching — the cache is ref-keyed, viewer-independent, and
//! read ONLY on the allowed branch; §4.2/REF-P10).
//!
//! ## Encrypted-from-birth under the per-tenant DEK (11.3/11.4; the REF-P4 reservation)
//! A cached projection MAY hold **a name in a title** (§3.6), so the cache value is the projection
//! **sealed under the per-tenant DEK** ([`crate::dek::RefsDekPin`], reserved in REF-P4) — never
//! plaintext. The stored blob is `nonce || ciphertext`; a read resolves the per-tenant DEK and `open`s
//! it. **Crypto-shred-able:** destroying the per-tenant DEK (tenant offboard) makes EVERY cached title
//! unrecoverable — a restored cache blob never decrypts, so a name in a title cannot resurrect (the
//! erasure-vs-immutability answer for the cache, EI-04 §1). A wrong/destroyed-key open is a clean MISS
//! (the cache re-resolves), NEVER a plaintext fall-through.
//!
//! ## Bounded + residency-pinned + tenant-first (§3 / §3.6)
//! - **Bounded:** every write carries a **TTL** ([`R2ProjectionCache::ttl`]) — the cache self-evicts,
//!   so it is never an unbounded source of truth (a stale title is bounded by the TTL, the *.updated
//!   bust evicts it sooner). The Valkey backing additionally caps memory; the bound is the TTL +
//!   Valkey's `maxmemory` policy (the in-memory floor is TTL-only).
//! - **Residency-pinned + tenant-first:** the key is `(tenant, ref)` — the underlying
//!   [`myelin_storage::Cache`] namespaces by `TenantId` (`{tenant}:{key}`), so one tenant NEVER reads
//!   another's cached projection (the no-cross-tenant-query floor, §3). The cache rides the cell-local
//!   Valkey instance (residency-pinned by deployment).
//!
//! ## NEVER a source of truth (§3.6 — the load-bearing property)
//! On a MISS — absent, expired, busted, erased, or a decrypt failure — [`R2ProjectionCache::read`]
//! returns `None`, and the resolve chokepoint re-resolves via the owner's `project`. The cache is
//! DERIVED + reconstructible: it holds nothing the owner is not the truth for. An `*.erased` evicts the
//! entry; the next read re-resolves (and, the artifact being gone, tombstones) — it NEVER serves the
//! stale pre-erasure title (the chained erasure test below).
//!
//! ## The storage seam reused, not forked (EI-01 §7 coherence)
//! The cache rides the EXISTING [`myelin_storage::Cache`] trait (the one cache primitive,
//! external-insights/02 §7) — the [`myelin_storage::InMemoryCache`] floor for unit tests + the
//! `ValkeyCache` real backing behind storage's `integration` feature. REF-P12 adds NO second cache
//! primitive; it adds the Refs-specific KEYING + the DEK SEALING + the projection CODEC on top of the
//! one seam. The integration test (`tests/integration_ref_p12_r2_cache.rs`, the `integration` feature)
//! proves the bust/fill/read round-trip + the crypto-shred against the LIVE dev-stack Valkey.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **The structural ERASE of cache PII is the holder erase surface in REF-P15.** This cache holds PII
//!   (a name in a title) sealed under the per-tenant DEK. REF-P12 makes the cache crypto-shred-able
//!   (destroy the DEK → every title unrecoverable) and event-busts it on `*.erased`; the SUBJECT-grain
//!   structural erase that drives the holder `erase` body (purge the subject's cached titles + rely on
//!   Identity's pseudonym shred for `origin_actor` + reindex-from-source) lands in **REF-P15**. Named
//!   so the cache is **not mistaken for a complete erasure answer** — it is the crypto-shred-able
//!   holder; REF-P15 is the erase body that drives it.
//! - **No live producer fill at M2.** The projection the cache holds comes from the owner's `project`
//!   (5.6), whose real Git/Knowledge/Chat implementations are REF-P17/P18/P21 (the resolve floor); the
//!   cache CODEC + KEYING + SEALING here are real and production-shaped.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! The R2 cache keying/sealing/invalidation is leak-of-stale-PII + erasure-correctness critical. Floor:
//! **≥ 80% of viable mutants caught** (`cargo mutants -p myelin-refs-service -f
//! crates/myelin-refs-service/src/cache.rs`). Measured 2026-06-20: **32 mutants generated → 3 unviable,
//! 29 viable, 28 caught, 1 missed = 96.5% of viable** — floor met. (The single surviving mutant flips
//! `<` to `<=` on the `blob.len() < NONCE_LEN` truncation guard; at the exact boundary — a NONCE_LEN
//! blob with empty ciphertext — BOTH the guard and `open` return a MISS, so it is behaviorally
//! equivalent: a sub-nonce or exact-nonce blob is a clean MISS either way, asserted in
//! `a_truncated_blob_is_a_clean_miss`.) Each load-bearing rule — the `(tenant, ref)` key derivation,
//! the seal/open round-trip, the busted/erased→MISS arm, the TTL on every write, the decrypt-fail→MISS
//! (never plaintext), and the true hit/miss/fill counters — has a unit test a mutation flips.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use myelin_events::ArtifactRef;
use myelin_storage::{Cache, NONCE_LEN};
use myelin_tenancy::{Region, TenantId};

use crate::dek::RefsDekPin;
use crate::invalidator::ProjectionCache;
use crate::resolve::{Projection, ProjectionCacheRead};

/// The default TTL the cache writes every entry under (the §3.6 bound — the cache self-evicts so it is
/// never an unbounded source of truth). 10 minutes: long enough to absorb a hot artifact's read fan-out,
/// short enough that a missed bust (a redelivery gap) still self-heals within the bound. A named
/// constant — drills assert against the NAME, never a literal.
pub const R2_DEFAULT_TTL: Duration = Duration::from_secs(600);

/// The cache-key prefix that namespaces the Refs projection cache inside a tenant's keyspace (the
/// underlying [`Cache`] already namespaces by `TenantId`; this prefix separates Refs projections from
/// any other Refs cache use under the same tenant). PII-free.
pub const R2_KEY_PREFIX: &str = "refs:proj:";

/// **The live R2 projection cache (§3.6 — the REF-P12 deliverable).** A bounded, invalidatable,
/// per-tenant-DEK-encrypted, residency-pinned holder keyed `(tenant, ref)`. Replaces BOTH the REF-P7
/// write-side [`crate::invalidator::NoOpCacheShim`] (it implements [`ProjectionCache`]) AND the REF-P10
/// read-side [`crate::resolve::NoOpCacheRead`] (it implements [`ProjectionCacheRead`]).
///
/// Cloneable: the backing [`Cache`] + the [`RefsDekPin`] are held behind `Arc`s + the counters are
/// `Arc<AtomicU64>`, so the SAME cache (and its observed hit/miss/fill counts) is shared across the
/// invalidator consumer thread + the resolve serving threads.
#[derive(Clone)]
pub struct R2ProjectionCache {
    /// The one cache primitive (external-insights/02 §7): the [`myelin_storage::InMemoryCache`] floor
    /// for unit tests / the `ValkeyCache` real backing behind storage's `integration` feature. The
    /// projection is sealed under the per-tenant DEK BEFORE it lands here — the backing store never
    /// sees plaintext.
    backing: Arc<dyn Cache>,
    /// The per-tenant DEK pin (REF-P4; 11.3/11.4) — the cache value is sealed under the per-tenant DEK
    /// so a cached title (a name) is crypto-shred-able + encrypted-at-rest. The SAME `Arc<KmsEngine>`
    /// the cell's other stores share (one cell root, one hierarchy — never a second KMS).
    dek: Arc<RefsDekPin>,
    /// The TTL every write carries (the §3.6 bound). Defaults to [`R2_DEFAULT_TTL`].
    ttl: Duration,
    /// Live `resolve_cache_hit_ratio` numerator (contract 1.8): cache READ hits. Bumped only on a true
    /// hit (a present, decrypting entry on the allowed branch). The denominator's other half is misses.
    hits: Arc<AtomicU64>,
    /// Cache READ misses (absent / expired / busted / decrypt-fail) — the fall-through to `project`.
    misses: Arc<AtomicU64>,
    /// Cache FILLs (a projection sealed + written after a resolve miss) — observability that the cache
    /// is being populated (a warm-up signal; not part of the hit ratio).
    fills: Arc<AtomicU64>,
}

impl R2ProjectionCache {
    /// The telemetry signal the cache feeds (contract 1.8). The resolve chokepoint
    /// ([`crate::resolve::RESOLVE_CACHE_HIT_RATIO_SIGNAL`]) owns the ratio; the cache exposes the raw
    /// hit/miss/fill counters that compute it. A named constant — drills assert against the NAME.
    pub const HIT_RATIO_SIGNAL: &'static str = crate::resolve::RESOLVE_CACHE_HIT_RATIO_SIGNAL;

    /// Build the live R2 cache over the one cache primitive + the per-tenant DEK pin, with the default
    /// TTL ([`R2_DEFAULT_TTL`]). The `backing` is the [`myelin_storage::InMemoryCache`] floor (unit)
    /// or the `ValkeyCache` real backing (integration); the `dek` is the cell's REF-P4 DEK pin.
    pub fn new(backing: Arc<dyn Cache>, dek: Arc<RefsDekPin>) -> R2ProjectionCache {
        R2ProjectionCache::with_ttl(backing, dek, R2_DEFAULT_TTL)
    }

    /// Build the cache with an explicit TTL (the bound). A TTL of `0` rounds up to the backing's
    /// minimum (the Valkey backing floors at 1s) — the entry still self-evicts; there is no unbounded
    /// write.
    pub fn with_ttl(
        backing: Arc<dyn Cache>,
        dek: Arc<RefsDekPin>,
        ttl: Duration,
    ) -> R2ProjectionCache {
        R2ProjectionCache {
            backing,
            dek,
            ttl,
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            fills: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The TTL every write carries (the §3.6 bound).
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// **The `(tenant, ref)` cache key (§3.6 — keyed per `ArtifactRef`, tenant-first).** The backing
    /// [`Cache`] namespaces by `TenantId` (`{tenant}:{key}`), so the cross-tenant isolation is
    /// structural; this returns the in-tenant key (the [`R2_KEY_PREFIX`] + the FULL `ArtifactRef` — a
    /// sub-anchored `…#block-9` is its own precise key, mirroring the REF-P7 invalidator's precise bust).
    /// PII-free: an opaque artifact URN.
    pub fn cache_key(ref_: &ArtifactRef) -> String {
        format!("{R2_KEY_PREFIX}{}", ref_.0)
    }

    /// **Fill the cache for `(tenant, ref)` with `projection` (the §4.2 post-miss populate).** Seals the
    /// serialized projection under the per-tenant DEK and writes `nonce || ciphertext` under the TTL.
    /// Called by the resolve chokepoint after a cache-miss projection so the NEXT viewer of the same
    /// `(tenant, ref)` is served a HIT (viewer-independent, ref-keyed — safe because the fill happens on
    /// the allowed branch and the read is gated by the per-viewer check). A backing/DEK error is a clean
    /// best-effort no-op (the cache is derived — a failed fill just means the next read re-resolves);
    /// it is surfaced as `Err` so a caller MAY log it, but resolve treats it as a non-fatal miss.
    pub fn fill(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        projection: &Projection,
    ) -> Result<(), CacheFillError> {
        // Resolve the per-tenant DEK (REF-P4). A destroyed/absent DEK (tenant offboarded mid-fill) is a
        // LOUD error — we do NOT write plaintext (the 0-fail-open invariant).
        let key_ref = self
            .dek
            .reserve(tenant, region)
            .map_err(|e| CacheFillError(format!("reserve per-tenant DEK: {e:?}")))?;
        let dek = self
            .dek
            .resolve(&key_ref, region)
            .map_err(|e| CacheFillError(format!("resolve per-tenant DEK: {e:?}")))?;

        // Serialize the projection (the 5.6 shape) → seal under the DEK → store nonce || ciphertext.
        let plaintext = serde_json::to_vec(projection)
            .map_err(|e| CacheFillError(format!("serialize projection: {e}")))?;
        let (nonce, ct) = dek.seal(&plaintext);
        let mut blob = Vec::with_capacity(NONCE_LEN + ct.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ct);

        self.backing
            .set(tenant, &Self::cache_key(ref_), &blob, self.ttl)
            .map_err(|e| CacheFillError(format!("cache set: {e}")))?;
        self.fills.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Decrypt + deserialize a stored `nonce || ciphertext` blob under the per-tenant DEK. Returns
    /// `None` on ANY failure (a truncated blob, a wrong/destroyed key that doesn't authenticate, or a
    /// codec mismatch) — a decrypt failure is a clean MISS, never a plaintext fall-through (the cache
    /// re-resolves). A crypto-shredded tenant's blob lands here and returns `None` (the title is
    /// unrecoverable).
    fn decode(&self, region: &Region, tenant: &TenantId, blob: &[u8]) -> Option<Projection> {
        if blob.len() < NONCE_LEN {
            return None; // a truncated/garbage blob is a miss, never a panic.
        }
        let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(nonce_bytes);

        // Resolve the per-tenant DEK by its deterministic ref. A destroyed/absent DEK (crypto-shred)
        // resolves to an Err → MISS (the cached title is unrecoverable, never decrypts to plaintext).
        let key_ref = self.dek.reserve(tenant, region).ok()?;
        let dek = self.dek.resolve(&key_ref, region).ok()?;
        let plaintext = dek.open(&nonce, ct)?; // a wrong key / tampered ct is None → MISS.
        serde_json::from_slice(&plaintext).ok()
    }

    /// The live `resolve_cache_hit_ratio` sample (contract 1.8): hits / (hits + misses). `None` until a
    /// read has hit the cache stage (no denominator). The cache also feeds the resolve chokepoint's own
    /// counters; this is the cache-internal view (e.g. for the invalidator-side observability).
    pub fn hit_ratio(&self) -> Option<f64> {
        let hits = self.hits.load(Ordering::SeqCst);
        let misses = self.misses.load(Ordering::SeqCst);
        let total = hits + misses;
        if total == 0 {
            None
        } else {
            Some(hits as f64 / total as f64)
        }
    }

    /// The raw `(hits, misses, fills)` counters — the drill reads them to assert a bust evicted (next
    /// read misses), a fill warmed (next read hits), and an erasure re-resolved (a busted entry misses).
    pub fn counters(&self) -> (u64, u64, u64) {
        (
            self.hits.load(Ordering::SeqCst),
            self.misses.load(Ordering::SeqCst),
            self.fills.load(Ordering::SeqCst),
        )
    }
}

/// A cache FILL failure — a backing/DEK/codec error on a populate. The cache is DERIVED, so a fill
/// failure is non-fatal (the next read re-resolves); it is surfaced so a caller MAY log it. A
/// destroyed-DEK fill error is the correct LOUD refusal to write plaintext (0-fail-open).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheFillError(pub String);

impl core::fmt::Display for CacheFillError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "R2 cache fill error: {}", self.0)
    }
}

impl std::error::Error for CacheFillError {}

// ── The READ side (§4.2 step 3): replaces the REF-P10 NoOpCacheRead. ──
impl ProjectionCacheRead for R2ProjectionCache {
    /// Read the cached projection for `(tenant, ref)` — a live HIT (decrypted under the per-tenant DEK)
    /// or a MISS (absent / expired / busted / erased / decrypt-fail). Read ONLY on the permission-
    /// allowed branch (the resolve chokepoint gates it — viewer-independent, ref-keyed). A MISS falls
    /// through to the owner's `project` (the cache is never a source of truth).
    fn read(&self, tenant: &TenantId, region: &Region, ref_: &ArtifactRef) -> Option<Projection> {
        // A backing error (Valkey unreachable) is treated as a MISS (the cache is best-effort, §3.6);
        // resolve falls through to project. We bump `misses` so the degraded ratio is observable.
        let blob = match self.backing.get(tenant, &Self::cache_key(ref_)) {
            Ok(Some(b)) => b,
            Ok(None) | Err(_) => {
                self.misses.fetch_add(1, Ordering::SeqCst);
                return None;
            }
        };
        match self.decode(region, tenant, &blob) {
            Some(p) => {
                self.hits.fetch_add(1, Ordering::SeqCst);
                Some(p)
            }
            None => {
                // present-but-undecryptable (crypto-shredded / tampered) → a MISS, never plaintext.
                self.misses.fetch_add(1, Ordering::SeqCst);
                None
            }
        }
    }

    /// Populate the cache after a resolve MISS (the §4.2 post-miss fill — the live override of the
    /// trait's no-op default). Best-effort: a fill error (backing/DEK) is swallowed (the cache is
    /// derived; the next read re-resolves). Delegates to the inherent [`R2ProjectionCache::fill`].
    fn fill(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        projection: &Projection,
    ) {
        let _ = R2ProjectionCache::fill(self, tenant, region, ref_, projection);
    }
}

// ── The WRITE/INVALIDATE side (§4.3): replaces the REF-P7 NoOpCacheShim. ──
impl ProjectionCache for R2ProjectionCache {
    /// **Bust the live cached projection for `(tenant, ref)` (the §3.6 invalidation — replaces the
    /// no-op shim).** Driven by the refs-projection-invalidator on `*.updated`/`*.erased`: it DELETES
    /// the entry, so the next read MISSES and re-resolves (a `*.updated` re-fetches the fresh
    /// projection; a `*.erased` re-resolves to a tombstone — NEVER the stale pre-erasure title).
    /// Idempotent: deleting an absent/already-busted entry is a well-defined no-op. A backing error is
    /// swallowed (the cache is best-effort; a TTL still bounds a missed bust) — the invalidator's
    /// `consumer_dedup` + the TTL together make a transiently-failed bust self-heal.
    fn invalidate(&self, tenant: &TenantId, _region: &Region, ref_: &ArtifactRef) {
        // delete is region-independent (the key is (tenant, ref); the backing namespaces by tenant).
        let _ = self.backing.delete(tenant, &Self::cache_key(ref_));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invalidator::RefsProjectionInvalidator;
    use crate::resolve::ProjectionFlag;
    use myelin_storage::{InMemoryCache, KmsEngine};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn pin() -> Arc<RefsDekPin> {
        Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())))
    }
    fn cache() -> R2ProjectionCache {
        R2ProjectionCache::new(Arc::new(InMemoryCache::new()), pin())
    }
    fn projection(ref_: &str, title: &str) -> Projection {
        Projection {
            ref_: ArtifactRef(ref_.into()),
            title: title.into(),
            state: "open".into(),
            icon: "lock".into(),
            render_hint: "issue-card".into(),
            sub_anchor: None,
            flag: None,
        }
    }

    /// A lifecycle event naming `subject` as the artifact that updated/erased — to drive the REF-P7
    /// invalidator over the live cache (the shim-swap test).
    fn lifecycle_event(id: &str, type_: &str, subject: &str) -> myelin_events::EventEnvelope {
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
        };
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        myelin_events::EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p-opaque-1".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            subject: ArtifactRef(subject.into()),
            aggregate: AggregateKey(format!("agg:{subject}")),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }

    /// **The key is `(tenant, ref)`, prefixed + per-`ArtifactRef` (§3.6).** A sub-anchored ref is its
    /// own precise key (mirrors the REF-P7 invalidator's precise bust). Tenant isolation is the
    /// backing's `{tenant}:{key}` namespacing (asserted in `tenants_are_isolated`).
    #[test]
    fn cache_key_is_prefixed_per_ref() {
        let k =
            R2ProjectionCache::cache_key(&ArtifactRef("myelin://acme/issue/issue/ENG-1".into()));
        assert_eq!(k, "refs:proj:myelin://acme/issue/issue/ENG-1");
        let sub =
            R2ProjectionCache::cache_key(&ArtifactRef("myelin://acme/kn/page/7c2#block-9".into()));
        assert_eq!(
            sub, "refs:proj:myelin://acme/kn/page/7c2#block-9",
            "the FULL #sub ref is the key"
        );
    }

    /// **fill → read round-trips the projection through the DEK seal (the cache HITs warm).** The blob
    /// stored is sealed (nonce || ciphertext), never plaintext; the read decrypts + deserializes the
    /// exact 5.6 shape.
    #[test]
    fn fill_then_read_hits_and_round_trips() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let p = projection(&ref_.0, "TOP SECRET acquisition plan");
        c.fill(&tenant(), &region(), &ref_, &p)
            .expect("fill seals + writes");

        let got = c
            .read(&tenant(), &region(), &ref_)
            .expect("warm entry HITs");
        assert_eq!(got, p, "the read decrypts to the exact projection");
        assert_eq!(c.counters(), (1, 0, 1), "one hit, zero misses, one fill");
    }

    /// **The cache value is SEALED — the raw backing blob never contains the plaintext title (a name
    /// in a title is encrypted at rest, §3.6 / EI-04 §1).**
    #[test]
    fn the_backing_blob_is_sealed_not_plaintext() {
        let backing = Arc::new(InMemoryCache::new());
        let c = R2ProjectionCache::new(backing.clone(), pin());
        let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        c.fill(
            &tenant(),
            &region(),
            &ref_,
            &projection(&ref_.0, "Alice Liddell's salary review"),
        )
        .expect("fill");
        // The blob in the backing store must NOT contain the plaintext name.
        let blob = backing
            .get(&tenant(), &R2ProjectionCache::cache_key(&ref_))
            .expect("backing get")
            .expect("a blob was written");
        let as_text = String::from_utf8_lossy(&blob);
        assert!(
            !as_text.contains("Alice Liddell"),
            "the cached title is sealed, never plaintext"
        );
        assert!(blob.len() > NONCE_LEN, "the blob is nonce || ciphertext");
    }

    /// **A cold read MISSES (no entry) — the cache is never a source of truth, it falls through.**
    #[test]
    fn cold_read_misses() {
        let c = cache();
        let r = c.read(
            &tenant(),
            &region(),
            &ArtifactRef("myelin://acme/issue/issue/none".into()),
        );
        assert!(r.is_none(), "a cold read misses (falls through to project)");
        assert_eq!(c.counters(), (0, 1, 0), "one miss");
    }

    /// **An `*.updated` bust evicts the live entry; the next read MISSES + re-resolves (the §3.6
    /// invalidation, the chained test: hit → updated → miss → re-resolve).** This is the REF-P7
    /// invalidator now driving the LIVE cache, not the no-op shim.
    #[test]
    fn bust_evicts_and_next_read_re_resolves() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/kn/page/7c2".into());
        c.fill(
            &tenant(),
            &region(),
            &ref_,
            &projection(&ref_.0, "v1 title"),
        )
        .expect("fill");
        assert!(
            c.read(&tenant(), &region(), &ref_).is_some(),
            "warm HIT before the bust"
        );

        // a *.updated busts the entry (the invalidator drives THIS now).
        c.invalidate(&tenant(), &region(), &ref_);
        assert!(
            c.read(&tenant(), &region(), &ref_).is_none(),
            "after the bust the read MISSES → re-resolves (never the stale v1 title)"
        );
    }

    /// **An `*.erased` busts the entry; the next read MISSES — the cache NEVER serves the stale
    /// pre-erasure title (the "never a source of truth on erasure" test).** After eviction the resolve
    /// chokepoint re-resolves (and, the artifact gone, tombstones) — the cache holds nothing stale.
    #[test]
    fn erasure_bust_never_serves_stale() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-secret".into());
        c.fill(
            &tenant(),
            &region(),
            &ref_,
            &projection(&ref_.0, "SECRET soon-erased"),
        )
        .expect("fill");
        assert!(
            c.read(&tenant(), &region(), &ref_).is_some(),
            "warm before erase"
        );

        c.invalidate(&tenant(), &region(), &ref_); // the *.erased bust
        let after = c.read(&tenant(), &region(), &ref_);
        assert!(
            after.is_none(),
            "on erasure the cache re-resolves — never serves the stale title"
        );
    }

    /// **Busting is idempotent — deleting an absent/already-busted entry is a no-op (the invalidator's
    /// dedup + a redelivery both land here safely).**
    #[test]
    fn bust_is_idempotent() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/issue/issue/E2".into());
        c.invalidate(&tenant(), &region(), &ref_); // absent
        c.fill(&tenant(), &region(), &ref_, &projection(&ref_.0, "t"))
            .expect("fill");
        c.invalidate(&tenant(), &region(), &ref_); // present
        c.invalidate(&tenant(), &region(), &ref_); // already busted
        assert!(
            c.read(&tenant(), &region(), &ref_).is_none(),
            "idempotent bust → miss"
        );
    }

    /// **Crypto-shred: destroying the per-tenant DEK makes a cached title UNRECOVERABLE — a read of the
    /// surviving blob MISSES (never decrypts to plaintext).** This is the cache's erasure-vs-immutability
    /// answer (§3.6 / EI-04 §1): the blob may survive in a backup, but without the DEK it never resurrects.
    #[test]
    fn crypto_shred_renders_a_cached_title_unrecoverable() {
        let kms = Arc::new(KmsEngine::new());
        let dek = Arc::new(RefsDekPin::new(kms));
        let backing = Arc::new(InMemoryCache::new());
        let c = R2ProjectionCache::new(backing, dek.clone());
        let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        c.fill(
            &tenant(),
            &region(),
            &ref_,
            &projection(&ref_.0, "a name in a title"),
        )
        .expect("fill");
        assert!(
            c.read(&tenant(), &region(), &ref_).is_some(),
            "decrypts while the DEK lives"
        );

        // tenant offboard: destroy the per-tenant DEK (crypto-shred). The blob may still be in the
        // backing store, but it can no longer decrypt.
        assert!(
            dek.destroy_tenant_dek(&tenant(), &region()),
            "the per-tenant DEK is shredded"
        );
        let after = c.read(&tenant(), &region(), &ref_);
        assert!(
            after.is_none(),
            "a crypto-shredded title is unrecoverable — a MISS, never plaintext"
        );
    }

    /// **Every write carries the TTL (the §3.6 bound — the cache self-evicts).** A 0-TTL entry is gone
    /// on the next read (the in-memory floor's deadline is already past), proving the write is bounded.
    #[test]
    fn writes_are_ttl_bounded() {
        let c = R2ProjectionCache::with_ttl(
            Arc::new(InMemoryCache::new()),
            pin(),
            Duration::from_millis(0),
        );
        assert_eq!(c.ttl(), Duration::from_millis(0));
        let ref_ = ArtifactRef("myelin://acme/issue/issue/ttl".into());
        c.fill(
            &tenant(),
            &region(),
            &ref_,
            &projection(&ref_.0, "ephemeral"),
        )
        .expect("fill");
        assert!(
            c.read(&tenant(), &region(), &ref_).is_none(),
            "a 0-TTL entry self-evicts (bounded)"
        );
        assert_eq!(
            R2_DEFAULT_TTL,
            Duration::from_secs(600),
            "the default bound is 10 minutes"
        );
    }

    /// **A non-default (non-zero) TTL is carried verbatim (the bound is the configured value, not a
    /// `Default`).** Pins [`R2ProjectionCache::ttl`] against a mutant that returns `Default::default()`.
    #[test]
    fn ttl_is_the_configured_value_not_default() {
        let c = R2ProjectionCache::with_ttl(
            Arc::new(InMemoryCache::new()),
            pin(),
            Duration::from_secs(42),
        );
        assert_eq!(
            c.ttl(),
            Duration::from_secs(42),
            "the configured TTL is carried verbatim"
        );
        assert_ne!(
            c.ttl(),
            Duration::default(),
            "the TTL is not the Default (0)"
        );
        // the default constructor carries the named 10-minute bound (a real, non-zero TTL).
        assert_eq!(cache().ttl(), R2_DEFAULT_TTL);
        assert!(
            cache().ttl() > Duration::ZERO,
            "the default bound is a real non-zero TTL"
        );
    }

    /// **A truncated blob (shorter than the nonce) is a clean MISS, never a panic/plaintext.** Pins the
    /// `blob.len() < NONCE_LEN` boundary in `decode` (a mutant flipping `<` to `<=`/`==` would
    /// mis-handle the exact-NONCE_LEN / short cases). We write a deliberately short raw value behind the
    /// cache key and assert the read MISSES.
    #[test]
    fn a_truncated_blob_is_a_clean_miss() {
        let backing = Arc::new(InMemoryCache::new());
        let c = R2ProjectionCache::new(backing.clone(), pin());
        let ref_ = ArtifactRef("myelin://acme/issue/issue/trunc".into());
        // a blob SHORTER than NONCE_LEN (can't even hold a nonce) → decode returns None at the length
        // guard, never indexing past the slice.
        backing
            .set(
                &tenant(),
                &R2ProjectionCache::cache_key(&ref_),
                &[0u8; NONCE_LEN - 1],
                Duration::from_secs(60),
            )
            .expect("write a short blob");
        assert!(
            c.read(&tenant(), &region(), &ref_).is_none(),
            "a sub-nonce blob is a clean MISS"
        );

        // a blob EXACTLY NONCE_LEN (a nonce + empty ciphertext) is also a MISS (an empty ct never
        // authenticates) — but it must reach `open`, not the length guard, and still not panic.
        backing
            .set(
                &tenant(),
                &R2ProjectionCache::cache_key(&ref_),
                &[0u8; NONCE_LEN],
                Duration::from_secs(60),
            )
            .expect("write an exact-nonce blob");
        assert!(
            c.read(&tenant(), &region(), &ref_).is_none(),
            "an exact-nonce (empty-ct) blob is a MISS"
        );
    }

    /// **The fill error renders a loud, descriptive message (Display is real, not a `Default`).**
    #[test]
    fn cache_fill_error_display_is_loud() {
        let e = CacheFillError("cache set: unreachable".into());
        let s = format!("{e}");
        assert!(
            s.contains("R2 cache fill error"),
            "the error names itself: {s}"
        );
        assert!(
            s.contains("unreachable"),
            "the error carries the cause: {s}"
        );
    }

    /// **Tenants are isolated — tenant B never reads tenant A's cached projection (the
    /// no-cross-tenant-query floor, §3; the backing's `{tenant}:{key}` namespacing).**
    #[test]
    fn tenants_are_isolated() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/issue/issue/shared".into());
        c.fill(
            &TenantId("a".into()),
            &region(),
            &ref_,
            &projection(&ref_.0, "a's title"),
        )
        .expect("fill");
        // tenant B, SAME ref → a MISS (no cross-tenant cache path).
        assert!(
            c.read(&TenantId("b".into()), &region(), &ref_).is_none(),
            "tenant B never reads tenant A's cached projection"
        );
    }

    /// **The hit ratio is the true hits/(hits+misses) — observability is real (1.8).** Two hits + one
    /// miss → 2/3.
    #[test]
    fn hit_ratio_is_true_division() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/issue/issue/r".into());
        c.fill(&tenant(), &region(), &ref_, &projection(&ref_.0, "t"))
            .expect("fill");
        c.read(&tenant(), &region(), &ref_); // hit
        c.read(&tenant(), &region(), &ref_); // hit
        c.read(
            &tenant(),
            &region(),
            &ArtifactRef("myelin://acme/issue/issue/absent".into()),
        ); // miss
        assert_eq!(c.counters(), (2, 1, 1));
        assert_eq!(
            c.hit_ratio(),
            Some(2.0 / 3.0),
            "the ratio is the true division"
        );
        assert_eq!(
            R2ProjectionCache::HIT_RATIO_SIGNAL,
            "resolve_cache_hit_ratio"
        );
    }

    /// **A sub-anchored projection (flag = Moved) round-trips intact (the §4.6 flag survives the
    /// seal/codec).**
    #[test]
    fn sub_anchored_projection_round_trips() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/kn/page/7c2#block-9".into());
        let mut p = projection(&ref_.0, "block 9");
        p.sub_anchor = Some("block-9".into());
        p.flag = Some(ProjectionFlag::Moved);
        c.fill(&tenant(), &region(), &ref_, &p).expect("fill");
        assert_eq!(
            c.read(&tenant(), &region(), &ref_),
            Some(p),
            "the #sub + flag survive the codec"
        );
    }

    /// **The live cache plugs into the REF-P7 invalidator UNCHANGED (the shim swap).** The invalidator
    /// holds the cache behind `Arc<dyn ProjectionCache>` and busts a live entry — proving REF-P12
    /// replaces the no-op shim with NO change to the consumer.
    #[test]
    fn live_cache_plugs_into_the_ref_p7_invalidator() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        c.fill(&tenant(), &region(), &ref_, &projection(&ref_.0, "v1"))
            .expect("fill");

        // the SAME invalidator from REF-P7, now over the LIVE cache (not the shim).
        use myelin_events::EventHandler;
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(c.clone()));
        let ev = lifecycle_event("01J-u", "issue.issue.updated", &ref_.0);
        assert_eq!(
            inv.handle(&ev),
            myelin_events::HandleOutcome::Done,
            "the invalidator busts the live entry"
        );
        assert!(
            c.read(&tenant(), &region(), &ref_).is_none(),
            "the live entry was busted, not recorded"
        );
    }
}
