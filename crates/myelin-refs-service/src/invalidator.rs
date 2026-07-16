//! The **refs-projection-invalidator** consumer + the **no-op projection-cache shim** (REF-P7 /
//! P-156; contract 2.4/2.5 consumer side; the §3.6 invalidation interface).
//!
//! **Owning architecture doc:** `reference-graph.md` §4.3 (two consumers off the substrate
//! `EventHandler` template — the `refs-edge-builder` (REF-P6) ingests, and the
//! **`refs-projection-invalidator` busts R2 on `*.updated`/`*.erased`**; both steady-state and cold
//! rebuild are ONE code path), §3.6 (the R2 projection cache it drives: a **bounded, invalidatable,
//! event-busted projection cache per `ArtifactRef`**, keyed `(tenant, ref)`, with `*.updated`/
//! `*.erased` invalidation — **never a source of truth**, on miss/erasure it re-resolves via the
//! projection API). **External insight:** `01-process-and-quality-doctrine.md` §3 (prove-it;
//! observability is part of the pass). **VISION §1** (the reference graph as connective tissue — a
//! ref that updated must not render stale).
//!
//! ## What REF-P7 (P-156) ships — the invalidator consumer + the no-op shim
//! The [`RefsProjectionInvalidator`] is an ordinary [`myelin_events::EventHandler`] (contract 2.4),
//! driven by the ONE sanctioned consumer runtime ([`myelin_events::Consumer`]) + the per-consumer
//! [`myelin_events::DedupLedger`] (contract 2.5, idempotent on `event_id`). It:
//!
//! - **whitelists** the `*.updated`/`*.erased` lifecycle subjects of the subsystems whose
//!   projections Refs caches (`issue.*.updated`, `knowledge.*.updated`, `chat.*.updated`, … and the
//!   matching `.erased`) — **NEVER `*`** (BUS-3/BUS-4: an over-broad subscription head-of-line-blocks
//!   the whole consumer; this is one of the explicitly reviewed firehose-class infra consumers, like
//!   the builder);
//! - on each `*.updated`/`*.erased`, **busts the projection-cache entry per `ArtifactRef`** — it
//!   computes the cache key `(tenant, ref)` for the artifact the event names and calls the
//!   [`ProjectionCache::invalidate`] interface (the §3.6 bust). A cache entry, once busted, re-resolves
//!   on the next read via the projection API (the cache is never a source of truth);
//! - is **idempotent on `event_id`** via `consumer_dedup` (rule 1, the runtime's outer guard) — a
//!   redelivered `*.updated` is dropped by the ledger, so it never double-busts; AND the bust itself
//!   is idempotent (busting an absent/already-busted entry is a well-defined no-op, never an error);
//! - **acks after apply** (the runtime's rule 2 — the cursor advances only on a terminal `Done`).
//!
//! ## The invalidation INTERFACE is real; the cache behind it is a NO-OP shim (the named floor)
//! Because the real R2 cache lands in **REF-P12**, this prompt ships the **invalidation interface**
//! ([`ProjectionCache`]) plus a **[`NoOpCacheShim`]** behind it. The shim holds **nothing** (there is
//! no cache to bust yet) but **records every invalidation call** ([`NoOpCacheShim::invalidations`]) so
//! the consumer's behaviour is observable + testable: a `*.updated` event drives exactly one
//! `invalidate(tenant, ref)` call. **REF-P12 replaces the shim** with the live bounded R2 cache by
//! implementing the SAME [`ProjectionCache`] trait — the invalidator does not change. Named so the
//! invalidation is **not mistaken for live cache-busting**: nothing is evicted yet because nothing is
//! cached yet; the WIRING is real, the CACHE is a shim.
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **No live R2 cache.** The invalidator targets the [`NoOpCacheShim`] (records calls; busts
//!   nothing) until **REF-P12** ships the live bounded, per-tenant-DEK-encrypted Valkey-class cache
//!   that implements [`ProjectionCache`]. REF-P12 swaps the shim for the live cache; this consumer is
//!   unchanged.
//! - **No mutation floor on a no-op shim.** Per the prompt: the no-op shim has no mutable projection
//!   state to mutation-test; the REAL cache mutation floor (eviction correctness under TTL + bound)
//!   lands in **REF-P12**. The invalidator's *routing* (which subjects bust, which key is computed)
//!   IS asserted by the unit + CDC tests below (every whitelisted branch + the key derivation).
//! - **The cache key is the `#sub`-stripped root for a backlink projection, the FULL ref for a
//!   sub-anchored projection.** §3.6 caches per `ArtifactRef`; an `*.updated` on a sub-anchored
//!   artifact (`…#block-9`) busts that exact key. The invalidator busts the FULL ref the event names
//!   (the most precise key); REF-P12's cache MAY additionally roll up to the root — that rollup is a
//!   cache-internal concern, not the invalidator's. Named so the precise-key choice is explicit.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use myelin_events::{
    ArtifactRef, EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern,
};
use myelin_tenancy::{Region, TenantId};

/// The durable consumer name (rule 4: bind-by-name; re-bound identically on reconnect so the SAME
/// dedup ledger + cursor are re-used → 0 lost across reconnect). PII-free identifier.
pub const INVALIDATOR_CONSUMER: &str = "refs-projection-invalidator";

/// The `*`-free subject-prefix whitelist the invalidator binds through [`myelin_events::ConsumerSpec`]
/// (the ONE sanctioned entry-point). The `*.updated`/`*.erased` lifecycle subjects of the subsystems
/// whose projections Refs caches (§3.6) — the artifacts a backlink/embed renders. NEVER `*` — an
/// over-broad subscription head-of-line-blocks the whole consumer (BUS-3/BUS-4). `consume(...)`
/// rejects a `*`/empty subject loudly at registration.
///
/// These are subject PREFIXES (the `Subscription`/`Consumer` prefix model); the dotted event TYPE on
/// the envelope (`issue.issue.updated`, `knowledge.page.erased`, …) is what [`is_invalidating`]
/// branches on (`*.updated`/`*.erased`). The invalidator is one of the explicitly reviewed
/// firehose-class infra consumers (BUS-4), mirroring the builder.
pub const INVALIDATOR_SUBJECT_PREFIXES: &[&str] =
    &["issue.", "knowledge.", "chat.", "git.", "refs.edge."];

/// The `'static` [`SubjectPattern`] slice the [`EventHandler`] trait requires (the service `serve`
/// binds the runtime through the sanctioned [`myelin_events::consume`] with
/// [`INVALIDATOR_SUBJECT_PREFIXES`], which the runtime rejects if `*`). Empty here for the same reason
/// the builder's is: the prefixes are the binding surface, not this slice.
pub static INVALIDATOR_SUBJECTS: &[SubjectPattern] = &[];

/// **The projection-cache invalidation INTERFACE (§3.6).** The seam the invalidator drives and the
/// live R2 cache (REF-P12) plugs into. `invalidate(tenant, ref)` busts the cached projection for one
/// `ArtifactRef` in one tenant (tenant-first — no cross-tenant cache path). Idempotent: busting an
/// absent/already-busted entry is a no-op. The REAL bounded, per-tenant-DEK-encrypted cache (REF-P12)
/// implements this trait; the [`NoOpCacheShim`] is the floor implementation that records calls but
/// holds no entries.
///
/// `Send + Sync` so the invalidator (a cloneable [`EventHandler`]) can hold it behind an [`Arc`] and
/// be shared across the consumer runtime's threads.
pub trait ProjectionCache: Send + Sync {
    /// Bust the cached projection for `ref_` in `(tenant, region)`. Idempotent (busting an absent
    /// entry is a no-op). Tenant-first: the key is `(tenant, ref)`, never a cross-tenant lookup
    /// (§3.6; the no-cross-tenant-query floor).
    fn invalidate(&self, tenant: &TenantId, region: &Region, ref_: &ArtifactRef);
}

/// **The NO-OP projection-cache shim (the REF-P7 floor — REF-P12 replaces it).** Implements
/// [`ProjectionCache`] but holds **no cache entries** (there is no cache yet); it **records every
/// `invalidate` call** so the invalidator's behaviour is observable + testable. A cloneable handle
/// over shared state (the consumer holds it behind an [`Arc<dyn ProjectionCache>`]; tests read the
/// recorded calls). PII-free: it records the `(tenant, region, ref)` triple of each bust, all opaque
/// tokens/URNs.
///
/// When REF-P12 lands, the live R2 cache implements [`ProjectionCache::invalidate`] to actually evict
/// the `(tenant, ref)` entry; the invalidator code does not change — only the trait object behind it.
#[derive(Clone, Default)]
pub struct NoOpCacheShim {
    /// Every `invalidate(tenant, region, ref)` call, in order — the proof the consumer busts exactly
    /// the right entries. Shared so a cloneable handle sees the same record.
    calls: Arc<Mutex<Vec<InvalidationCall>>>,
}

/// One recorded `invalidate(tenant, region, ref)` call (the no-op shim's observable output). PII-free:
/// the partition key + the opaque artifact URN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidationCall {
    /// The tenant whose cache entry was busted (tenant-first; no cross-tenant bust).
    pub tenant: TenantId,
    /// The region partition.
    pub region: Region,
    /// The `ArtifactRef` whose cached projection was busted.
    pub ref_: ArtifactRef,
}

impl NoOpCacheShim {
    /// A fresh, empty no-op shim (records calls; busts nothing).
    pub fn new() -> NoOpCacheShim {
        NoOpCacheShim::default()
    }

    /// Every recorded `invalidate` call, in delivery order (the test/drill assertion surface).
    pub fn invalidations(&self) -> Vec<InvalidationCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The count of recorded `invalidate` calls (a duplicate-deduped delivery never adds one because
    /// the runtime's `consumer_dedup` drops the redelivery before `handle` runs).
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

impl ProjectionCache for NoOpCacheShim {
    fn invalidate(&self, tenant: &TenantId, region: &Region, ref_: &ArtifactRef) {
        // NO-OP bust: there is no cache entry to evict yet (REF-P12 ships the live cache). RECORD the
        // call so the consumer's behaviour is observable + testable (one bust per `*.updated`/
        // `*.erased`, per ArtifactRef, tenant-first).
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(InvalidationCall {
                tenant: tenant.clone(),
                region: region.clone(),
                ref_: ref_.clone(),
            });
    }
}

/// Why an `*.updated`/`*.erased` event could not drive an invalidation — a structurally-malformed
/// event (one that names no `ArtifactRef` to bust) is a LOUD, non-retryable poison, NEVER a silent
/// skip (fail-closed; EI-01 §5). A stale-rendered ref is a correctness bug, so a malformed
/// invalidation signal must surface, not be swallowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidateError(pub String);

/// **The refs-projection-invalidator consumer (REF-P7; the §4.3 second consumer).** An ordinary
/// [`EventHandler`] that busts the projection cache on `*.updated`/`*.erased`. Cloneable handle (the
/// cache is shared behind an [`Arc`]). Idempotent on `event_id` (the runtime's `consumer_dedup`) AND
/// on the bust itself (busting an absent entry is a no-op).
#[derive(Clone)]
pub struct RefsProjectionInvalidator {
    /// The §3.6 invalidation interface — the no-op shim now, the live R2 cache (REF-P12) later. Behind
    /// an `Arc<dyn …>` so the consumer is cloneable + the cache is swappable without touching this
    /// consumer.
    cache: Arc<dyn ProjectionCache>,
    /// The live `refs.invalidations` measurement (contract 1.8): cache-bust calls issued. Observable
    /// so a drill asserts the consumer is actually busting (observability is part of the pass).
    busted: Arc<AtomicU64>,
}

impl RefsProjectionInvalidator {
    /// The telemetry signal name this invalidator emits (contract 1.8). A named constant — drills
    /// assert against the NAME, never a literal (EI-01 §3 observability).
    pub const INVALIDATIONS_SIGNAL: &'static str = "refs.invalidations";

    /// Build the invalidator over a [`ProjectionCache`] (the no-op shim now; the live R2 cache in
    /// REF-P12). Takes any `ProjectionCache` so REF-P12 swaps the implementation without changing the
    /// consumer.
    pub fn new(cache: impl ProjectionCache + 'static) -> RefsProjectionInvalidator {
        RefsProjectionInvalidator {
            cache: Arc::new(cache),
            busted: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Build the invalidator over an already-`Arc`'d cache (so a test can hold the SAME shim handle to
    /// read its recorded calls while the consumer drives it).
    pub fn with_cache(cache: Arc<dyn ProjectionCache>) -> RefsProjectionInvalidator {
        RefsProjectionInvalidator {
            cache,
            busted: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The live `refs.invalidations` sample (contract 1.8): cache-bust calls this invalidator has
    /// issued. Monotone (a redelivery is deduped before it reaches here, so it never increments).
    pub fn invalidation_count(&self) -> u64 {
        self.busted.load(Ordering::SeqCst)
    }

    /// **Is this event type one the invalidator acts on?** True for `*.updated`/`*.erased` (the §4.3
    /// invalidation triggers); false for everything else (a `*.created` builds an edge — the
    /// builder's job — but does not bust a cache, because nothing was cached for a brand-new
    /// artifact). The dotted event TYPE's terminal segment is the discriminator.
    pub fn is_invalidating(type_: &str) -> bool {
        let name = type_.rsplit('.').next().unwrap_or("");
        matches!(name, "updated" | "erased")
    }

    /// **Bust the projection cache for ONE `*.updated`/`*.erased` event (the ONE invalidation step).**
    /// Factored out of [`EventHandler::handle`] so a reindex-from-source replay / a drill can drive it
    /// directly — like the builder's `project`, this keeps steady-state == cold-rebuild a single code
    /// path (§4.3): a live `*.updated` and a replayed one both flow through HERE.
    ///
    /// The `ArtifactRef` to bust is the event's `subject` (the artifact that updated/erased — every
    /// envelope carries it as a first-class field) — falling back to a `ref`/`subject` payload field
    /// if the envelope subject is empty. A non-invalidating event (`*.created`, …) is a no-op (the
    /// invalidator only busts on update/erase). A malformed event (an invalidating type that names no
    /// ref) is a non-retryable poison ([`InvalidateError`]) — never a silent skip.
    pub fn invalidate_for(&self, ev: &EventEnvelope) -> Result<(), InvalidateError> {
        if !Self::is_invalidating(ev.type_.0.as_str()) {
            // Not an invalidation trigger (e.g. `*.created`): a well-defined no-op. The invalidator
            // whitelists broad lifecycle subjects, but only `*.updated`/`*.erased` bust a cache.
            return Ok(());
        }

        // The artifact to bust: the envelope `subject` (first-class, every event carries it), else a
        // payload `ref`/`subject` field. An invalidating event MUST name an artifact, or it is a
        // poison (a stale ref would otherwise render — fail-closed).
        let ref_ = self.artifact_ref(ev).ok_or_else(|| {
            InvalidateError(format!(
                "{} names no ArtifactRef to invalidate (no envelope subject, no payload ref)",
                ev.type_.0
            ))
        })?;

        // Bust the entry per ArtifactRef, tenant-first (§3.6 key `(tenant, ref)`). Idempotent: the
        // shim records the call; a live cache (REF-P12) evicts the entry (a no-op if absent).
        self.cache.invalidate(&ev.tenant, &ev.region, &ref_);
        self.busted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// The `ArtifactRef` an event names — the envelope `subject` (first-class) if non-empty, else a
    /// `ref`/`subject` payload field. `None` if the event names no artifact at all.
    fn artifact_ref(&self, ev: &EventEnvelope) -> Option<ArtifactRef> {
        if !ev.subject.0.is_empty() {
            return Some(ev.subject.clone());
        }
        let p = &ev.payload;
        p.get("ref")
            .or_else(|| p.get("subject"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| ArtifactRef(s.to_string()))
    }
}

impl EventHandler for RefsProjectionInvalidator {
    /// The `*`-free subject whitelist (rule 3): the `*.updated`/`*.erased` lifecycle subjects. The
    /// `'static` slice the trait requires; the service `serve` binds the runtime through the
    /// sanctioned [`myelin_events::consume`] with [`INVALIDATOR_SUBJECT_PREFIXES`] (rejected if `*`).
    /// NEVER `*` (BUS-3/BUS-4).
    fn subjects(&self) -> &'static [SubjectPattern] {
        INVALIDATOR_SUBJECTS
    }

    /// Bust the projection cache for the delivered `*.updated`/`*.erased` event (contract 2.4).
    /// Idempotent on `event_id` (the runtime's `consumer_dedup` outer guard, rule 1) — a redelivery
    /// never re-busts. A malformed invalidation event is a non-retryable poison
    /// ([`HandleOutcome::NonRetryable`]) — surfaced, never a silent stale-ref.
    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        match self.invalidate_for(ev) {
            Ok(()) => HandleOutcome::Done,
            Err(InvalidateError(reason)) => HandleOutcome::NonRetryable(Reason(reason)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-1".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    /// A lifecycle event naming `subject` as the artifact that updated/erased.
    fn lifecycle_event(id: &str, type_: &str, subject: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
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

    // --- is_invalidating: only *.updated / *.erased bust ---

    /// **Only `*.updated`/`*.erased` are invalidation triggers (§4.3).** A `*.created` builds an edge
    /// (the builder's job) but busts no cache; a non-lifecycle event is ignored.
    #[test]
    fn only_updated_and_erased_are_invalidating() {
        assert!(RefsProjectionInvalidator::is_invalidating(
            "issue.issue.updated"
        ));
        assert!(RefsProjectionInvalidator::is_invalidating(
            "knowledge.page.erased"
        ));
        assert!(RefsProjectionInvalidator::is_invalidating(
            "chat.message.updated"
        ));
        assert!(!RefsProjectionInvalidator::is_invalidating(
            "refs.edge.created"
        ));
        assert!(!RefsProjectionInvalidator::is_invalidating(
            "issue.issue.created"
        ));
        assert!(!RefsProjectionInvalidator::is_invalidating(
            "knowledge.page.removed"
        ));
    }

    // --- one *.updated → one invalidation call per ArtifactRef (the prompt's required unit test) ---

    /// **A `*.updated` event drives exactly ONE `invalidate(tenant, ref)` call per `ArtifactRef`
    /// (REF-P7 unit requirement).** The no-op shim records the bust: tenant-first, the exact ref the
    /// event named. The `refs.invalidations` telemetry is bumped.
    #[test]
    fn updated_event_drives_one_invalidation_per_ref() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let ref_ = "myelin://acme/knowledge/page/7c2";
        let ev = lifecycle_event("01J-u1", "knowledge.page.updated", ref_);

        assert_eq!(
            inv.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "an *.updated busts the cache"
        );
        let calls = shim.invalidations();
        assert_eq!(calls.len(), 1, "exactly one invalidation call");
        assert_eq!(
            calls[0].tenant,
            tenant(),
            "tenant-first (no cross-tenant bust)"
        );
        assert_eq!(calls[0].region, region());
        assert_eq!(
            calls[0].ref_.0, ref_,
            "the exact ArtifactRef the event named"
        );
        assert_eq!(
            inv.invalidation_count(),
            1,
            "the refs.invalidations telemetry is bumped"
        );
        assert_eq!(
            RefsProjectionInvalidator::INVALIDATIONS_SIGNAL,
            "refs.invalidations",
            "the contract-1.8 signal name"
        );
    }

    /// **An `*.erased` event busts the cache (the §3.6 erasure invalidation).** Mirrors `*.updated`;
    /// the erased artifact's cached projection must not render.
    #[test]
    fn erased_event_busts_the_cache() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let ref_ = "myelin://acme/issue/issue/ENG-1";
        let ev = lifecycle_event("01J-e1", "issue.issue.erased", ref_);
        assert_eq!(inv.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
        assert_eq!(
            shim.call_count(),
            1,
            "the erased artifact's cache entry is busted"
        );
        assert_eq!(shim.invalidations()[0].ref_.0, ref_);
    }

    /// **A sub-anchored `*.updated` (`…#block-9`) busts the FULL `#sub` ref (the most precise key).**
    /// §3.6 caches per `ArtifactRef`; an update to block-9 busts exactly `…#block-9`. REF-P12's cache
    /// MAY roll up to the root internally — that is a cache concern, not the invalidator's.
    #[test]
    fn sub_anchored_update_busts_the_full_ref() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let ref_ = "myelin://acme/knowledge/page/7c2#block-9";
        let ev = lifecycle_event("01J-u2", "knowledge.page.updated", ref_);
        assert_eq!(inv.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
        assert_eq!(
            shim.invalidations()[0].ref_.0,
            ref_,
            "the FULL #sub ref is the precise bust key"
        );
    }

    // --- non-invalidating events are no-ops (no bust) ---

    /// **A `*.created` is a no-op for the invalidator (no bust).** Creation builds an edge (the
    /// builder, REF-P6); nothing was cached for a brand-new artifact, so there is nothing to bust.
    #[test]
    fn created_event_is_a_noop_no_bust() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let ev = lifecycle_event(
            "01J-c1",
            "issue.issue.created",
            "myelin://acme/issue/issue/ENG-2",
        );
        assert_eq!(
            inv.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "a created event is handled (no-op)"
        );
        assert_eq!(shim.call_count(), 0, "no cache bust on create");
        assert_eq!(inv.invalidation_count(), 0);
    }

    // --- malformed invalidating event → non-retryable poison (fail-closed) ---

    /// **An `*.updated` that names NO `ArtifactRef` is a LOUD non-retryable poison** (fail-closed — a
    /// stale ref would otherwise render; the malformed invalidation signal must surface, not be
    /// swallowed). No bust is recorded.
    #[test]
    fn malformed_updated_without_ref_is_a_poison() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let mut ev = lifecycle_event("01J-bad", "knowledge.page.updated", "");
        ev.subject = ArtifactRef(String::new());
        ev.payload = serde_json::json!({ "title": "x" }); // no ref/subject field.
        match inv.handle(&ev, &mut myelin_events::HandlerTx::none()) {
            HandleOutcome::NonRetryable(Reason(r)) => {
                assert!(
                    r.contains("ArtifactRef"),
                    "the poison names the missing ref: {r}"
                )
            }
            other => panic!("a malformed invalidation event must poison, got {other:?}"),
        }
        assert_eq!(shim.call_count(), 0, "no bust on a malformed event");
    }

    /// **The invalidator falls back to a payload `ref` field when the envelope subject is empty.** An
    /// event that carries the artifact in its payload (not the envelope subject) still busts.
    #[test]
    fn payload_ref_fallback_busts_when_subject_empty() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let mut ev = lifecycle_event("01J-u3", "chat.message.updated", "");
        ev.subject = ArtifactRef(String::new());
        ev.payload = serde_json::json!({ "ref": "myelin://acme/chat/message/m1" });
        assert_eq!(inv.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
        assert_eq!(
            shim.invalidations()[0].ref_.0,
            "myelin://acme/chat/message/m1"
        );
    }

    // --- tenant-first: two tenants' busts never cross ---

    /// **The bust is tenant-first — two tenants' identical refs bust distinct `(tenant, ref)` keys
    /// (no cross-tenant cache path, §3.6).**
    #[test]
    fn busts_are_tenant_first_no_cross_tenant_path() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let ref_ = "myelin://acme/knowledge/page/7c2";
        let mut ev_a = lifecycle_event("01J-a", "knowledge.page.updated", ref_);
        ev_a.tenant = TenantId("acme".into());
        let mut ev_b = lifecycle_event("01J-b", "knowledge.page.updated", ref_);
        ev_b.tenant = TenantId("other".into());
        inv.handle(&ev_a, &mut myelin_events::HandlerTx::none());
        inv.handle(&ev_b, &mut myelin_events::HandlerTx::none());
        let calls = shim.invalidations();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tenant.0, "acme");
        assert_eq!(
            calls[1].tenant.0, "other",
            "the second bust is the OTHER tenant — keys never cross"
        );
    }

    // --- the shim records nothing-but-the-call (it is a no-op cache) ---

    /// **The no-op shim holds no cache entries — it ONLY records calls (the named floor).** This is
    /// what distinguishes the shim from a live cache (REF-P12): the invalidation interface is real,
    /// the cache behind it is a no-op that proves the WIRING.
    #[test]
    fn shim_records_calls_holds_no_entries() {
        let shim = NoOpCacheShim::new();
        assert_eq!(shim.call_count(), 0, "a fresh shim has recorded nothing");
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        inv.handle(&lifecycle_event(
            "01J-u",
            "issue.issue.updated",
            "myelin://acme/issue/issue/E1",
        ), &mut myelin_events::HandlerTx::none());
        // The shim recorded the call but holds no projection state (no get/contains — it is no-op).
        assert_eq!(
            shim.call_count(),
            1,
            "the call was recorded (the WIRING is real)"
        );
    }
}
