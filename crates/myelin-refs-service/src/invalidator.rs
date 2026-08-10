use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use myelin_events::{
    ArtifactRef, EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern,
};
use myelin_tenancy::{Region, TenantId};

pub const INVALIDATOR_CONSUMER: &str = "refs-projection-invalidator";

pub const INVALIDATOR_SUBJECT_PREFIXES: &[&str] =
    &["issue.", "knowledge.", "chat.", "git.", "refs.edge."];

pub static INVALIDATOR_SUBJECTS: &[SubjectPattern] = &[];

pub trait ProjectionCache: Send + Sync {
    fn invalidate(&self, tenant: &TenantId, region: &Region, ref_: &ArtifactRef);
}

#[derive(Clone, Default)]
pub struct NoOpCacheShim {
    calls: Arc<Mutex<Vec<InvalidationCall>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidationCall {
    pub tenant: TenantId,
    pub region: Region,
    pub ref_: ArtifactRef,
}

impl NoOpCacheShim {
    pub fn new() -> NoOpCacheShim {
        NoOpCacheShim::default()
    }

    pub fn invalidations(&self) -> Vec<InvalidationCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

impl ProjectionCache for NoOpCacheShim {
    fn invalidate(&self, tenant: &TenantId, region: &Region, ref_: &ArtifactRef) {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidateError(pub String);

#[derive(Clone)]
pub struct RefsProjectionInvalidator {
    cache: Arc<dyn ProjectionCache>,
    busted: Arc<AtomicU64>,
}

impl RefsProjectionInvalidator {
    pub const INVALIDATIONS_SIGNAL: &'static str = "refs.invalidations";

    pub fn new(cache: impl ProjectionCache + 'static) -> RefsProjectionInvalidator {
        RefsProjectionInvalidator {
            cache: Arc::new(cache),
            busted: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_cache(cache: Arc<dyn ProjectionCache>) -> RefsProjectionInvalidator {
        RefsProjectionInvalidator {
            cache,
            busted: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn invalidation_count(&self) -> u64 {
        self.busted.load(Ordering::SeqCst)
    }

    pub fn is_invalidating(type_: &str) -> bool {
        let name = type_.rsplit('.').next().unwrap_or("");
        matches!(name, "updated" | "erased")
    }

    pub fn invalidate_for(&self, ev: &EventEnvelope) -> Result<(), InvalidateError> {
        if !Self::is_invalidating(ev.type_.0.as_str()) {
            return Ok(());
        }

        let ref_ = self.artifact_ref(ev).ok_or_else(|| {
            InvalidateError(format!(
                "{} names no ArtifactRef to invalidate (no envelope subject, no payload ref)",
                ev.type_.0
            ))
        })?;

        self.cache.invalidate(&ev.tenant, &ev.region, &ref_);
        self.busted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

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
    fn subjects(&self) -> &'static [SubjectPattern] {
        INVALIDATOR_SUBJECTS
    }

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

    #[test]
    fn erased_event_busts_the_cache() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let ref_ = "myelin://acme/issue/issue/ENG-1";
        let ev = lifecycle_event("01J-e1", "issue.issue.erased", ref_);
        assert_eq!(
            inv.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            shim.call_count(),
            1,
            "the erased artifact's cache entry is busted"
        );
        assert_eq!(shim.invalidations()[0].ref_.0, ref_);
    }

    #[test]
    fn sub_anchored_update_busts_the_full_ref() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let ref_ = "myelin://acme/knowledge/page/7c2#block-9";
        let ev = lifecycle_event("01J-u2", "knowledge.page.updated", ref_);
        assert_eq!(
            inv.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            shim.invalidations()[0].ref_.0,
            ref_,
            "the FULL #sub ref is the precise bust key"
        );
    }

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

    #[test]
    fn malformed_updated_without_ref_is_a_poison() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let mut ev = lifecycle_event("01J-bad", "knowledge.page.updated", "");
        ev.subject = ArtifactRef(String::new());
        ev.payload = serde_json::json!({ "title": "x" });
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

    #[test]
    fn payload_ref_fallback_busts_when_subject_empty() {
        let shim = NoOpCacheShim::new();
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        let mut ev = lifecycle_event("01J-u3", "chat.message.updated", "");
        ev.subject = ArtifactRef(String::new());
        ev.payload = serde_json::json!({ "ref": "myelin://acme/chat/message/m1" });
        assert_eq!(
            inv.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            shim.invalidations()[0].ref_.0,
            "myelin://acme/chat/message/m1"
        );
    }

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
            "the second bust is the OTHER tenant - keys never cross"
        );
    }

    #[test]
    fn shim_records_calls_holds_no_entries() {
        let shim = NoOpCacheShim::new();
        assert_eq!(shim.call_count(), 0, "a fresh shim has recorded nothing");
        let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
        inv.handle(
            &lifecycle_event(
                "01J-u",
                "issue.issue.updated",
                "myelin://acme/issue/issue/E1",
            ),
            &mut myelin_events::HandlerTx::none(),
        );
        assert_eq!(
            shim.call_count(),
            1,
            "the call was recorded (the WIRING is real)"
        );
    }
}
