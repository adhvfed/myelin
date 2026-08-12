use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use myelin_events::ArtifactRef;
use myelin_storage::{Cache, NONCE_LEN};
use myelin_tenancy::{Region, TenantId};

use crate::dek::RefsDekPin;
use crate::invalidator::ProjectionCache;
use crate::resolve::{Projection, ProjectionCacheRead};

pub const R2_DEFAULT_TTL: Duration = Duration::from_secs(600);

pub const R2_KEY_PREFIX: &str = "refs:proj:";

#[derive(Clone)]
pub struct R2ProjectionCache {
    backing: Arc<dyn Cache>,
    dek: Arc<RefsDekPin>,
    ttl: Duration,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    fills: Arc<AtomicU64>,
}

impl R2ProjectionCache {
    pub const HIT_RATIO_SIGNAL: &'static str = crate::resolve::RESOLVE_CACHE_HIT_RATIO_SIGNAL;

    pub fn new(backing: Arc<dyn Cache>, dek: Arc<RefsDekPin>) -> R2ProjectionCache {
        R2ProjectionCache::with_ttl(backing, dek, R2_DEFAULT_TTL)
    }

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

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn cache_key(ref_: &ArtifactRef) -> String {
        format!("{R2_KEY_PREFIX}{}", ref_.0)
    }

    pub fn fill(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        projection: &Projection,
    ) -> Result<(), CacheFillError> {
        let key_ref = self
            .dek
            .reserve(tenant, region)
            .map_err(|e| CacheFillError(format!("reserve per-tenant DEK: {e:?}")))?;
        let dek = self
            .dek
            .resolve(&key_ref, region)
            .map_err(|e| CacheFillError(format!("resolve per-tenant DEK: {e:?}")))?;

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

    fn decode(&self, region: &Region, tenant: &TenantId, blob: &[u8]) -> Option<Projection> {
        if blob.len() < NONCE_LEN {
            return None;
        }
        let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(nonce_bytes);

        let key_ref = self.dek.reserve(tenant, region).ok()?;
        let dek = self.dek.resolve(&key_ref, region).ok()?;
        let plaintext = dek.open(&nonce, ct)?;
        serde_json::from_slice(&plaintext).ok()
    }

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

    pub fn counters(&self) -> (u64, u64, u64) {
        (
            self.hits.load(Ordering::SeqCst),
            self.misses.load(Ordering::SeqCst),
            self.fills.load(Ordering::SeqCst),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheFillError(pub String);

impl core::fmt::Display for CacheFillError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "R2 cache fill error: {}", self.0)
    }
}

impl std::error::Error for CacheFillError {}

impl ProjectionCacheRead for R2ProjectionCache {
    fn read(&self, tenant: &TenantId, region: &Region, ref_: &ArtifactRef) -> Option<Projection> {
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
                self.misses.fetch_add(1, Ordering::SeqCst);
                None
            }
        }
    }

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

impl ProjectionCache for R2ProjectionCache {
    fn invalidate(&self, tenant: &TenantId, _region: &Region, ref_: &ArtifactRef) {
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

        c.invalidate(&tenant(), &region(), &ref_);
        assert!(
            c.read(&tenant(), &region(), &ref_).is_none(),
            "after the bust the read MISSES → re-resolves (never the stale v1 title)"
        );
    }

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

        c.invalidate(&tenant(), &region(), &ref_);
        let after = c.read(&tenant(), &region(), &ref_);
        assert!(
            after.is_none(),
            "on erasure the cache re-resolves - never serves the stale title"
        );
    }

    #[test]
    fn bust_is_idempotent() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/issue/issue/E2".into());
        c.invalidate(&tenant(), &region(), &ref_);
        c.fill(&tenant(), &region(), &ref_, &projection(&ref_.0, "t"))
            .expect("fill");
        c.invalidate(&tenant(), &region(), &ref_);
        c.invalidate(&tenant(), &region(), &ref_);
        assert!(
            c.read(&tenant(), &region(), &ref_).is_none(),
            "idempotent bust → miss"
        );
    }

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

        assert!(
            dek.destroy_tenant_dek(&tenant(), &region()).unwrap(),
            "the per-tenant DEK is shredded"
        );
        let after = c.read(&tenant(), &region(), &ref_);
        assert!(
            after.is_none(),
            "a crypto-shredded title is unrecoverable - a MISS, never plaintext"
        );
    }

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
        assert_eq!(cache().ttl(), R2_DEFAULT_TTL);
        assert!(
            cache().ttl() > Duration::ZERO,
            "the default bound is a real non-zero TTL"
        );
    }

    #[test]
    fn a_truncated_blob_is_a_clean_miss() {
        let backing = Arc::new(InMemoryCache::new());
        let c = R2ProjectionCache::new(backing.clone(), pin());
        let ref_ = ArtifactRef("myelin://acme/issue/issue/trunc".into());
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
        assert!(
            c.read(&TenantId("b".into()), &region(), &ref_).is_none(),
            "tenant B never reads tenant A's cached projection"
        );
    }

    #[test]
    fn hit_ratio_is_true_division() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/issue/issue/r".into());
        c.fill(&tenant(), &region(), &ref_, &projection(&ref_.0, "t"))
            .expect("fill");
        c.read(&tenant(), &region(), &ref_);
        c.read(&tenant(), &region(), &ref_);
        c.read(
            &tenant(),
            &region(),
            &ArtifactRef("myelin://acme/issue/issue/absent".into()),
        );
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

    #[test]
    fn live_cache_plugs_into_the_ref_p7_invalidator() {
        let c = cache();
        let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        c.fill(&tenant(), &region(), &ref_, &projection(&ref_.0, "v1"))
            .expect("fill");

        use myelin_events::EventHandler;
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(c.clone()));
        let ev = lifecycle_event("01J-u", "issue.issue.updated", &ref_.0);
        assert_eq!(
            inv.handle(&ev, &mut myelin_events::HandlerTx::none()),
            myelin_events::HandleOutcome::Done,
            "the invalidator busts the live entry"
        );
        assert!(
            c.read(&tenant(), &region(), &ref_).is_none(),
            "the live entry was busted, not recorded"
        );
    }
}
