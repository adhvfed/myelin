use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    SetExpr, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::membership::{channel_object, permissions};

pub mod gate;
pub mod invalidation;

pub use invalidation::{
    erasure_safe_rerender, invalidates_card, CardUpdatePush, LiveUnfurlInvalidator,
    UnfurlInvalidator, DEFAULT_CACHE_TTL_SECONDS, UNFURL_INVALIDATION_SUBJECTS,
};

pub use gate::{
    lower_over_unfurl_candidate, unfurl_candidate_colref, AuthzJoin, AuthzVisibleIndex, BoundParam,
    FilterMode, LoweredFilter,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub title: String,
    pub state: String,
    pub icon: String,
    pub sub_anchor: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    Denied,
    Gone,
    Erased,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    pub root: ArtifactRef,
    pub reason: TombstoneReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LadderOutcome {
    Live(Projection),
    Moved(Projection),
    Outdated(Projection),
    Gone(Tombstone),
    Erased(Tombstone),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Card {
    Live {
        projection: Projection,
        moved: bool,
        outdated: bool,
    },
    Tombstone(Tombstone),
}

impl Card {
    pub fn is_tombstone(&self) -> bool {
        matches!(self, Card::Tombstone(_))
    }

    pub fn exposed_title(&self) -> Option<&str> {
        match self {
            Card::Live { projection, .. } => Some(&projection.title),
            Card::Tombstone(_) => None,
        }
    }
}

pub trait RefsResolvePort {
    fn resolve(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        at: &Consistency,
    ) -> LadderOutcome;
}

#[derive(Clone, Default)]
pub struct UnfurlCache {
    entries: Arc<Mutex<HashMap<String, Projection>>>,
}

impl UnfurlCache {
    pub fn new() -> UnfurlCache {
        UnfurlCache::default()
    }

    pub fn get(&self, ref_: &ArtifactRef) -> Option<Projection> {
        self.entries.lock().unwrap().get(&ref_.0).cloned()
    }

    pub fn put(&self, ref_: &ArtifactRef, projection: Projection) -> Option<Projection> {
        self.entries
            .lock()
            .unwrap()
            .insert(ref_.0.clone(), projection)
    }

    pub fn bust(&self, ref_: &ArtifactRef) -> bool {
        self.entries.lock().unwrap().remove(&ref_.0).is_some()
    }

    pub fn entry_count(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn contains(&self, ref_: &ArtifactRef) -> bool {
        self.entries.lock().unwrap().contains_key(&ref_.0)
    }

    pub fn clear(&self) -> usize {
        let mut entries = self.entries.lock().unwrap();
        let n = entries.len();
        entries.clear();
        n
    }
}

#[derive(Clone, Debug)]
pub struct UnfurlCandidate {
    pub ref_: ArtifactRef,
    pub channel_id: Option<String>,
}

pub struct UnfurlService<I: IdentityService, R: RefsResolvePort> {
    id: I,
    resolver: R,
    cache: UnfurlCache,
}

impl<I: IdentityService, R: RefsResolvePort> UnfurlService<I, R> {
    pub fn new(id: I, resolver: R) -> UnfurlService<I, R> {
        UnfurlService {
            id,
            resolver,
            cache: UnfurlCache::new(),
        }
    }

    pub fn with_cache(id: I, resolver: R, cache: UnfurlCache) -> UnfurlService<I, R> {
        UnfurlService {
            id,
            resolver,
            cache,
        }
    }

    pub fn cache(&self) -> &UnfurlCache {
        &self.cache
    }

    pub fn resolver(&self) -> &R {
        &self.resolver
    }

    pub fn resolve_one(&self, candidate: &UnfurlCandidate, viewer: &Principal) -> Card {
        let tenant = TenantId(viewer.tenant.0.clone());
        let region = Region(viewer.region.0.clone());

        let (object, at) = self.gate_object(candidate);
        let decision = self.id.check(
            viewer,
            &Permission(permissions::READ.to_string()),
            &object,
            &at,
            None,
        );
        match decision {
            Ok(Decision::Allow) => {}
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Card::Tombstone(Tombstone {
                    root: myelin_refs::strip_sub(&candidate.ref_),
                    reason: TombstoneReason::Denied,
                });
            }
        }

        if let Some(projection) = self.cache.get(&candidate.ref_) {
            return Card::Live {
                projection,
                moved: false,
                outdated: false,
            };
        }

        let outcome = self
            .resolver
            .resolve(&tenant, &region, &candidate.ref_, viewer, &at);
        self.outcome_to_card(&candidate.ref_, outcome)
    }

    pub fn unfurl_viewport(&self, viewport: &[UnfurlCandidate], viewer: &Principal) -> Vec<Card> {
        viewport
            .iter()
            .map(|c| self.resolve_one(c, viewer))
            .collect()
    }

    fn gate_object(
        &self,
        candidate: &UnfurlCandidate,
    ) -> (myelin_tenancy::ArtifactRef, Consistency) {
        match &candidate.channel_id {
            Some(channel_id) => (
                myelin_tenancy::ArtifactRef(channel_object(channel_id)),
                Consistency {
                    at_least: Zookie(String::new()),
                    mode: ConsistencyMode::Strong,
                },
            ),
            None => (
                myelin_tenancy::ArtifactRef(candidate.ref_.0.clone()),
                Consistency {
                    at_least: Zookie(String::new()),
                    mode: ConsistencyMode::Strong,
                },
            ),
        }
    }

    fn outcome_to_card(&self, ref_: &ArtifactRef, outcome: LadderOutcome) -> Card {
        match outcome {
            LadderOutcome::Live(projection) => {
                self.cache.put(ref_, projection.clone());
                Card::Live {
                    projection,
                    moved: false,
                    outdated: false,
                }
            }
            LadderOutcome::Moved(projection) => {
                self.cache.put(ref_, projection.clone());
                Card::Live {
                    projection,
                    moved: true,
                    outdated: false,
                }
            }
            LadderOutcome::Outdated(projection) => {
                self.cache.put(ref_, projection.clone());
                Card::Live {
                    projection,
                    moved: false,
                    outdated: true,
                }
            }
            LadderOutcome::Gone(tombstone) | LadderOutcome::Erased(tombstone) => {
                Card::Tombstone(tombstone)
            }
        }
    }
}

pub fn precompute_visibility_class(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    lower_over_unfurl_candidate(set_expr, viewer)
}

pub fn filter_candidates_by_class(
    index: &AuthzVisibleIndex,
    tenant: &TenantId,
    region: &Region,
    viewer: &Principal,
    lowered: &LoweredFilter,
    candidates: &[ObjectId],
) -> Vec<ObjectId> {
    index.evaluate(tenant, region, viewer, lowered, candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    use myelin_identity::{AuthzError, ListObjectsResult, Precondition};
    use myelin_identity::{
        Credential, EffectivePolicy, FailStaticBound, FragmentAdmit, NamespaceFragment, ObjectType,
        PrincipalId, PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId,
        RunToken, SubjectTree, TupleDelta,
    };

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }
    fn confidential_ref() -> ArtifactRef {
        ArtifactRef("myelin://acme/chat/channel/board-secret".into())
    }
    const SECRET_TITLE: &str = "#board-leadership-comp";

    #[derive(Default)]
    struct GateId {
        allow: StdMutex<Vec<(String, String)>>,
    }
    impl GateId {
        fn allow(&self, subject: &str, object: &str) {
            self.allow
                .lock()
                .unwrap()
                .push((subject.into(), object.into()));
        }
    }
    impl IdentityService for GateId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn check(
            &self,
            subject: &Principal,
            _permission: &Permission,
            object: &myelin_tenancy::ArtifactRef,
            _at: &Consistency,
            _caveat: Option<&myelin_identity::CaveatContext>,
        ) -> IdResult<Decision> {
            let allowed = self
                .allow
                .lock()
                .unwrap()
                .iter()
                .any(|(s, o)| s == &subject.principal_id.0 && o == &object.0);
            Ok(if allowed {
                Decision::Allow
            } else {
                Decision::Deny
            })
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _at: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _at: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &FailStaticBound,
        ) -> IdResult<RunToken> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
            Ok(FragmentAdmit::Admitted {
                fragment_id: "test".into(),
            })
        }
    }

    #[derive(Default)]
    struct SyntheticResolver {
        outcome: StdMutex<Option<LadderOutcome>>,
        calls: StdMutex<usize>,
    }
    impl SyntheticResolver {
        fn live() -> SyntheticResolver {
            let r = SyntheticResolver::default();
            *r.outcome.lock().unwrap() = Some(LadderOutcome::Live(Projection {
                title: SECRET_TITLE.into(),
                state: "active".into(),
                icon: "channel".into(),
                sub_anchor: None,
            }));
            r
        }
        fn set(&self, o: LadderOutcome) {
            *self.outcome.lock().unwrap() = Some(o);
        }
        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }
    impl RefsResolvePort for SyntheticResolver {
        fn resolve(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            _ref_: &ArtifactRef,
            _viewer: &Principal,
            _at: &Consistency,
        ) -> LadderOutcome {
            *self.calls.lock().unwrap() += 1;
            self.outcome.lock().unwrap().clone().expect("outcome set")
        }
    }

    fn channel_candidate() -> UnfurlCandidate {
        UnfurlCandidate {
            ref_: confidential_ref(),
            channel_id: Some("board-secret".into()),
        }
    }

    #[test]
    fn denied_viewer_tombstones_title_never_fetched() {
        let id = GateId::default();
        let resolver = SyntheticResolver::live();
        let svc = UnfurlService::new(id, resolver);

        let card = svc.resolve_one(&channel_candidate(), &viewer("intruder"));
        assert!(card.is_tombstone(), "a denied viewer sees a tombstone");
        assert_eq!(
            card.exposed_title(),
            None,
            "0 title leak - no title exposed"
        );
        assert_eq!(
            svc.resolver.calls(),
            0,
            "the resolver is unreached for a denied viewer"
        );
        assert!(!svc.cache().contains(&confidential_ref()));
    }

    #[test]
    fn one_cache_entry_per_ref_never_per_viewer() {
        let id = GateId::default();
        let object = channel_object("board-secret");
        id.allow("alice", &object);
        id.allow("bob", &object);
        id.allow("carol", &object);
        let resolver = SyntheticResolver::live();
        let svc = UnfurlService::new(id, resolver);

        for who in ["alice", "bob", "carol"] {
            let card = svc.resolve_one(&channel_candidate(), &viewer(who));
            assert_eq!(
                card.exposed_title(),
                Some(SECRET_TITLE),
                "{who} sees the shared title"
            );
        }
        assert_eq!(
            svc.cache().entry_count(),
            1,
            "exactly one cache entry per ref"
        );
        assert_eq!(
            svc.resolver.calls(),
            1,
            "resolve once, cache serves the rest"
        );
    }

    #[test]
    fn chained_member_then_revoke_then_tombstone_zero_leak() {
        let id = GateId::default();
        let object = channel_object("board-secret");
        id.allow("dave", &object);
        let resolver = SyntheticResolver::live();
        let svc = UnfurlService::new(id, resolver);

        let before = svc.resolve_one(&channel_candidate(), &viewer("dave"));
        assert_eq!(before.exposed_title(), Some(SECRET_TITLE));

        svc.id.allow.lock().unwrap().clear();

        let after = svc.resolve_one(&channel_candidate(), &viewer("dave"));
        assert!(after.is_tombstone(), "post-revoke the card is a tombstone");
        assert_eq!(after.exposed_title(), None, "0 leak post-revoke");
    }

    #[test]
    fn ladder_outcomes_map_to_cards() {
        let object = channel_object("board-secret");

        let id = GateId::default();
        id.allow("e", &object);
        let svc = UnfurlService::new(id, SyntheticResolver::live());
        let card = svc.resolve_one(&channel_candidate(), &viewer("e"));
        assert!(matches!(
            card,
            Card::Live {
                moved: false,
                outdated: false,
                ..
            }
        ));

        let id2 = GateId::default();
        id2.allow("f", &object);
        let resolver2 = SyntheticResolver::default();
        resolver2.set(LadderOutcome::Gone(Tombstone {
            root: confidential_ref(),
            reason: TombstoneReason::Gone,
        }));
        let svc2 = UnfurlService::new(id2, resolver2);
        match svc2.resolve_one(&channel_candidate(), &viewer("f")) {
            Card::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::Gone),
            other => panic!("expected Gone tombstone, got {other:?}"),
        }
        assert!(!svc2.cache().contains(&confidential_ref()));

        let id3 = GateId::default();
        id3.allow("g", &object);
        let resolver3 = SyntheticResolver::default();
        resolver3.set(LadderOutcome::Erased(Tombstone {
            root: confidential_ref(),
            reason: TombstoneReason::Erased,
        }));
        let svc3 = UnfurlService::new(id3, resolver3);
        match svc3.resolve_one(&channel_candidate(), &viewer("g")) {
            Card::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::Erased),
            other => panic!("expected Erased tombstone, got {other:?}"),
        }
    }

    #[test]
    fn unfurl_viewport_resolves_only_the_slice() {
        let id = GateId::default();
        let object = channel_object("board-secret");
        id.allow("h", &object);
        let svc = UnfurlService::new(id, SyntheticResolver::live());
        let viewport = vec![channel_candidate(), channel_candidate()];
        let cards = svc.unfurl_viewport(&viewport, &viewer("h"));
        assert_eq!(cards.len(), 2);
        assert!(cards
            .iter()
            .all(|c| c.exposed_title() == Some(SECRET_TITLE)));
        assert_eq!(svc.cache().entry_count(), 1);
        assert_eq!(svc.resolver.calls(), 1);
    }

    #[test]
    fn cache_bust_drops_the_shared_entry() {
        let id = GateId::default();
        let object = channel_object("board-secret");
        id.allow("i", &object);
        let svc = UnfurlService::new(id, SyntheticResolver::live());
        svc.resolve_one(&channel_candidate(), &viewer("i"));
        assert_eq!(svc.cache().entry_count(), 1);
        assert!(
            svc.cache().bust(&confidential_ref()),
            "the entry was busted"
        );
        assert_eq!(svc.cache().entry_count(), 0);
        svc.resolve_one(&channel_candidate(), &viewer("i"));
        assert_eq!(svc.resolver.calls(), 2);
    }
}
