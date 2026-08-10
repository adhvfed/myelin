use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::ArtifactRef;
use myelin_identity::{Consistency, ConsistencyMode, Decision, Permission, Principal, Zookie};
use myelin_substrate::{AuthzDecision, FailStaticAuthz, ServeError};
use myelin_tenancy::{CellId, CrossCellPointer, Region, TenantId};

pub const VIEW_PERMISSION: &str = "view";

pub const RESOLVE_CACHE_HIT_RATIO_SIGNAL: &str = "resolve_cache_hit_ratio";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveMode {
    Live,
    Display,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Projection {
    pub ref_: ArtifactRef,
    pub title: String,
    pub state: String,
    pub icon: String,
    pub render_hint: String,
    pub sub_anchor: Option<String>,
    pub flag: Option<ProjectionFlag>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectionFlag {
    Moved,
    Outdated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    pub root: ArtifactRef,
    pub reason: TombstoneReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    Denied,
    RootGone,
    SubGone,
    Erased,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Projection(Projection),
    Tombstone(Tombstone),
}

impl Resolution {
    pub fn is_projection(&self) -> bool {
        matches!(self, Resolution::Projection(_))
    }

    pub fn is_tombstone(&self) -> bool {
        matches!(self, Resolution::Tombstone(_))
    }

    pub fn tombstone_reason(&self) -> Option<TombstoneReason> {
        match self {
            Resolution::Tombstone(t) => Some(t.reason),
            Resolution::Projection(_) => None,
        }
    }
}

pub trait ProjectApi: Send + Sync {
    fn check_view(
        &self,
        tenant: &TenantId,
        region: &Region,
        object: &ArtifactRef,
        viewer: &Principal,
        permission: &Permission,
    ) -> Result<Decision, ProjectApiError>;

    fn project(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        mode: ResolveMode,
    ) -> Result<ProjectOutcome, ProjectApiError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectOutcome {
    Live(OwnerProjection),
    RootGone,
    SubGone,
    Erased,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerProjection {
    pub title: String,
    pub state: String,
    pub icon: String,
    pub render_hint: String,
    pub sub_anchor: Option<String>,
    pub flag: Option<ProjectionFlag>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectApiError {
    Unavailable(String),
    BadRequest(String),
}

pub trait ProjectionCacheRead: Send + Sync {
    fn read(&self, tenant: &TenantId, region: &Region, ref_: &ArtifactRef) -> Option<Projection>;

    fn fill(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        _ref_: &ArtifactRef,
        _projection: &Projection,
    ) {
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoOpCacheRead;

impl ProjectionCacheRead for NoOpCacheRead {
    fn read(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        _ref_: &ArtifactRef,
    ) -> Option<Projection> {
        None
    }
}

pub use myelin_substrate::AuthzServed;

pub struct ResolveService {
    authz: Arc<FailStaticAuthz>,
    cache: Arc<dyn ProjectionCacheRead>,
    owner: Arc<dyn ProjectApi>,
    home_cell: CellId,
    cache_hits: Arc<AtomicU64>,
    cache_misses: Arc<AtomicU64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrossCellDisposition {
    Home,
    Foreign(CellId),
}

impl ResolveService {
    pub fn new(
        authz: Arc<FailStaticAuthz>,
        cache: Arc<dyn ProjectionCacheRead>,
        owner: Arc<dyn ProjectApi>,
        home_cell: CellId,
    ) -> ResolveService {
        ResolveService {
            authz,
            cache,
            owner,
            home_cell,
            cache_hits: Arc::new(AtomicU64::new(0)),
            cache_misses: Arc::new(AtomicU64::new(0)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        root: &ArtifactRef,
        viewer: &Principal,
        mode: ResolveMode,
        at: &Consistency,
        subject_revoked: bool,
    ) -> Resolution {
        let (resolution, _served) = self.resolve_observed(
            tenant,
            region,
            ref_,
            root,
            viewer,
            mode,
            at,
            subject_revoked,
        );
        resolution
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resolve_observed(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        root: &ArtifactRef,
        viewer: &Principal,
        mode: ResolveMode,
        at: &Consistency,
        subject_revoked: bool,
    ) -> (Resolution, AuthzServed) {
        let key = myelin_substrate::encode_authz_key(&[
            &tenant.0,
            &region.0,
            &viewer.principal_id.0,
            VIEW_PERMISSION,
            &root.0,
        ]);
        let perm = Permission(VIEW_PERMISSION.to_string());
        let decision: AuthzDecision = self.authz.serve(key, at, subject_revoked, || {
            self.owner
                .check_view(tenant, region, root, viewer, &perm)
                .map_err(|e| ServeError(format!("identity check hiccup: {e:?}")))
        });

        if !matches!(decision.decision, Decision::Allow) {
            return (
                Resolution::Tombstone(Tombstone {
                    root: root.clone(),
                    reason: TombstoneReason::Denied,
                }),
                decision.served,
            );
        }

        if let Some(cached) = self.cache.read(tenant, region, ref_) {
            self.cache_hits.fetch_add(1, Ordering::SeqCst);
            return (Resolution::Projection(cached), decision.served);
        }
        self.cache_misses.fetch_add(1, Ordering::SeqCst);

        let resolution = match self.owner.project(tenant, region, ref_, viewer, mode) {
            Ok(ProjectOutcome::Live(op)) => {
                let projection = Projection {
                    ref_: ref_.clone(),
                    title: op.title,
                    state: op.state,
                    icon: op.icon,
                    render_hint: op.render_hint,
                    sub_anchor: op.sub_anchor,
                    flag: op.flag,
                };
                self.cache.fill(tenant, region, ref_, &projection);
                Resolution::Projection(projection)
            }
            Ok(ProjectOutcome::RootGone) => Resolution::Tombstone(Tombstone {
                root: root.clone(),
                reason: TombstoneReason::RootGone,
            }),
            Ok(ProjectOutcome::SubGone) => Resolution::Tombstone(Tombstone {
                root: root.clone(),
                reason: TombstoneReason::SubGone,
            }),
            Ok(ProjectOutcome::Erased) => Resolution::Tombstone(Tombstone {
                root: root.clone(),
                reason: TombstoneReason::Erased,
            }),
            Err(_unavailable) => Resolution::Tombstone(Tombstone {
                root: root.clone(),
                reason: TombstoneReason::RootGone,
            }),
        };
        (resolution, decision.served)
    }

    pub fn subscribe_subjects(ref_: &ArtifactRef) -> Vec<String> {
        let subsystem = ref_
            .0
            .strip_prefix("myelin://")
            .and_then(|rest| rest.split('/').nth(1))
            .unwrap_or("unknown");
        vec![
            format!("{subsystem}.updated"),
            format!("{subsystem}.erased"),
        ]
    }

    pub fn cross_cell_disposition(&self, target_cell: &CellId) -> CrossCellDisposition {
        if target_cell == &self.home_cell {
            CrossCellDisposition::Home
        } else {
            CrossCellDisposition::Foreign(target_cell.clone())
        }
    }

    pub fn disposition_of_pointer(&self, ptr: &CrossCellPointer) -> CrossCellDisposition {
        self.cross_cell_disposition(ptr.home_cell())
    }

    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let hits = self.cache_hits.load(Ordering::SeqCst);
        let misses = self.cache_misses.load(Ordering::SeqCst);
        let total = hits + misses;
        if total == 0 {
            None
        } else {
            Some(hits as f64 / total as f64)
        }
    }

    pub fn cache_counters(&self) -> (u64, u64) {
        (
            self.cache_hits.load(Ordering::SeqCst),
            self.cache_misses.load(Ordering::SeqCst),
        )
    }

    pub fn fail_static_signals(&self) -> myelin_substrate::FailStaticSignals {
        self.authz.signals()
    }
}

pub fn bounded_stale() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}

pub fn strong_read(zookie: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zookie.into()),
        mode: ConsistencyMode::Strong,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_substrate::FailStaticThreshold;
    use std::sync::Mutex;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn cell() -> CellId {
        CellId::from_token("cell-fr-par-1")
    }
    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }
    fn confidential_issue() -> ArtifactRef {
        ArtifactRef("myelin://acme/issue/issue/ENG-secret".into())
    }

    fn threshold() -> FailStaticThreshold {
        FailStaticThreshold {
            status: "OPEN - LEGAL".into(),
            owner: "DPO / Legal".into(),
            static_max_secs: None,
            static_max_default_secs: 300,
            agent_token_ttl_secs: 60,
            constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
        }
    }
    fn authz() -> Arc<FailStaticAuthz> {
        Arc::new(FailStaticAuthz::try_new(300, &threshold()).expect("valid bound"))
    }

    #[derive(Default)]
    struct SyntheticOwner {
        allowed: Mutex<Vec<String>>,
        outcome: Mutex<Option<ProjectOutcome>>,
        check_hiccup: Mutex<bool>,
        project_calls: Mutex<u64>,
    }

    impl SyntheticOwner {
        fn allow(&self, viewer_id: &str) {
            self.allowed.lock().unwrap().push(viewer_id.into());
        }
        fn set_outcome(&self, o: ProjectOutcome) {
            *self.outcome.lock().unwrap() = Some(o);
        }
        fn force_check_hiccup(&self) {
            *self.check_hiccup.lock().unwrap() = true;
        }
        fn project_call_count(&self) -> u64 {
            *self.project_calls.lock().unwrap()
        }
        fn secret_projection() -> OwnerProjection {
            OwnerProjection {
                title: "TOP SECRET acquisition plan".into(),
                state: "open".into(),
                icon: "lock".into(),
                render_hint: "issue-card".into(),
                sub_anchor: None,
                flag: None,
            }
        }
    }

    impl ProjectApi for SyntheticOwner {
        fn check_view(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            _object: &ArtifactRef,
            viewer: &Principal,
            _permission: &Permission,
        ) -> Result<Decision, ProjectApiError> {
            if *self.check_hiccup.lock().unwrap() {
                return Err(ProjectApiError::Unavailable("identity hiccup".into()));
            }
            let allowed = self.allowed.lock().unwrap();
            if allowed.iter().any(|a| a == &viewer.principal_id.0) {
                Ok(Decision::Allow)
            } else {
                Ok(Decision::Deny)
            }
        }

        fn project(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            _ref_: &ArtifactRef,
            _viewer: &Principal,
            _mode: ResolveMode,
        ) -> Result<ProjectOutcome, ProjectApiError> {
            *self.project_calls.lock().unwrap() += 1;
            Ok(self
                .outcome
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| ProjectOutcome::Live(SyntheticOwner::secret_projection())))
        }
    }

    #[derive(Default)]
    struct MapCacheRead {
        entries: Mutex<Vec<(String, Projection)>>,
    }
    impl MapCacheRead {
        fn put(&self, ref_: &str, p: Projection) {
            self.entries.lock().unwrap().push((ref_.into(), p));
        }
    }
    impl ProjectionCacheRead for MapCacheRead {
        fn read(&self, _t: &TenantId, _r: &Region, ref_: &ArtifactRef) -> Option<Projection> {
            self.entries
                .lock()
                .unwrap()
                .iter()
                .find(|(k, _)| k == &ref_.0)
                .map(|(_, p)| p.clone())
        }
    }

    fn service(owner: Arc<SyntheticOwner>) -> ResolveService {
        ResolveService::new(authz(), Arc::new(NoOpCacheRead), owner, cell())
    }

    #[test]
    fn denied_viewer_gets_tombstone_carrying_no_content_zero_leak() {
        let owner = Arc::new(SyntheticOwner::default());
        let svc = service(owner.clone());
        let ref_ = confidential_issue();
        let root = ref_.clone();

        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("intruder"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );

        assert!(
            r.is_tombstone(),
            "a denied viewer gets a tombstone, never a projection"
        );
        assert_eq!(r.tombstone_reason(), Some(TombstoneReason::Denied));
        if let Resolution::Tombstone(t) = &r {
            assert_eq!(
                t.root, root,
                "the tombstone carries the root (and only the root)"
            );
            let rendered = format!("{t:?}");
            assert!(
                !rendered.contains("SECRET") && !rendered.contains("acquisition"),
                "0 leak: the secret title must not appear in the tombstone, got `{rendered}`"
            );
        }
        assert_eq!(
            owner.project_call_count(),
            0,
            "a denied viewer never reaches project"
        );
        assert_eq!(
            svc.cache_counters(),
            (0, 0),
            "a denied resolve never touches the cache"
        );
    }

    #[test]
    fn allowed_viewer_gets_projection_from_project() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        let svc = service(owner.clone());
        let ref_ = confidential_issue();
        let root = ref_.clone();

        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );

        assert!(r.is_projection(), "an allowed viewer gets a projection");
        if let Resolution::Projection(p) = &r {
            assert_eq!(
                p.title, "TOP SECRET acquisition plan",
                "the allowed viewer sees the title"
            );
            assert_eq!(p.ref_, ref_, "the projection carries the resolved ref");
            assert_eq!(p.state, "open");
        }
        assert_eq!(
            owner.project_call_count(),
            1,
            "the allowed viewer reached project once"
        );
        assert_eq!(
            svc.cache_counters(),
            (0, 1),
            "an allowed miss falls through to project"
        );
    }

    #[test]
    fn two_viewers_share_one_ref_keyed_cache_without_leaking() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        let ref_ = confidential_issue();
        let root = ref_.clone();

        let cache = Arc::new(MapCacheRead::default());
        cache.put(
            &ref_.0,
            Projection {
                ref_: ref_.clone(),
                title: "TOP SECRET acquisition plan".into(),
                state: "open".into(),
                icon: "lock".into(),
                render_hint: "issue-card".into(),
                sub_anchor: None,
                flag: None,
            },
        );
        let svc = ResolveService::new(authz(), cache, owner.clone(), cell());

        let permitted = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &strong_read("z1"),
            false,
        );
        assert!(
            permitted.is_projection(),
            "the permitted viewer is served the cached projection"
        );

        let denied = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("intruder"),
            ResolveMode::Live,
            &strong_read("z1"),
            false,
        );
        assert!(
            denied.is_tombstone(),
            "the denied viewer is tombstoned even though the ref is cached"
        );
        assert_eq!(denied.tombstone_reason(), Some(TombstoneReason::Denied));

        assert_eq!(
            svc.cache_counters(),
            (1, 0),
            "one shared-cache hit (permitted); denied never read"
        );
        assert_eq!(
            owner.project_call_count(),
            0,
            "the shared cache served the permitted viewer"
        );
    }

    #[test]
    fn id_hiccup_degrades_to_tombstone_never_cascades_or_leaks() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        owner.force_check_hiccup();
        let svc = service(owner.clone());
        let ref_ = confidential_issue();
        let root = ref_.clone();

        let (r, served) = svc.resolve_observed(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        assert!(
            r.is_tombstone(),
            "an Id hiccup degrades to a tombstone (fail-closed), never a leak"
        );
        assert_eq!(r.tombstone_reason(), Some(TombstoneReason::Denied));
        assert_eq!(
            served,
            AuthzServed::Closed,
            "the fail-static branch is Closed (degraded, not cascade)"
        );
        assert_eq!(
            svc.fail_static_signals().closed,
            1,
            "the fail-static ratio telemetry fires"
        );
        assert_eq!(
            owner.project_call_count(),
            0,
            "a fail-closed gate never reaches project"
        );
    }

    #[test]
    fn strong_read_bypasses_cache_and_fails_closed_on_hiccup() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        owner.force_check_hiccup();
        let svc = service(owner);
        let ref_ = confidential_issue();
        let root = ref_.clone();
        let (r, served) = svc.resolve_observed(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &strong_read("z9"),
            false,
        );
        assert!(
            r.is_tombstone(),
            "a strong read fails closed on a hiccup → tombstone"
        );
        assert_eq!(
            served,
            AuthzServed::BypassClosed,
            "the strong read bypassed the cache, failed closed"
        );
    }

    #[test]
    fn revoked_subject_is_tombstoned_even_if_allowed() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        let svc = service(owner.clone());
        let ref_ = confidential_issue();
        let root = ref_.clone();
        let (r, served) = svc.resolve_observed(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            true,
        );
        assert!(
            r.is_tombstone(),
            "a revoked subject is tombstoned even though otherwise allowed"
        );
        assert_eq!(
            served,
            AuthzServed::Revoked,
            "the revoke is enforced before the cache/gate"
        );
        assert_eq!(
            owner.project_call_count(),
            0,
            "a revoked viewer never reaches project"
        );
    }

    #[test]
    fn moved_sub_renders_a_flagged_projection() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        owner.set_outcome(ProjectOutcome::Live(OwnerProjection {
            title: "doc".into(),
            state: "live".into(),
            icon: "page".into(),
            render_hint: "embed".into(),
            sub_anchor: Some("L42-L88".into()),
            flag: Some(ProjectionFlag::Moved),
        }));
        let svc = service(owner);
        let ref_ = ArtifactRef("myelin://acme/git/ref/main#L42-L88".into());
        let root = ArtifactRef("myelin://acme/git/ref/main".into());
        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        match r {
            Resolution::Projection(p) => assert_eq!(p.flag, Some(ProjectionFlag::Moved)),
            other => panic!("a moved sub must render a flagged projection, got {other:?}"),
        }
    }

    #[test]
    fn sub_ladder_maps_outcomes_onto_tombstone_reasons() {
        for (outcome, want) in [
            (ProjectOutcome::RootGone, TombstoneReason::RootGone),
            (ProjectOutcome::SubGone, TombstoneReason::SubGone),
            (ProjectOutcome::Erased, TombstoneReason::Erased),
        ] {
            let owner = Arc::new(SyntheticOwner::default());
            owner.allow("insider");
            owner.set_outcome(outcome.clone());
            let svc = service(owner);
            let ref_ = ArtifactRef("myelin://acme/knowledge/page/7c2#b9".into());
            let root = ArtifactRef("myelin://acme/knowledge/page/7c2".into());
            let r = svc.resolve(
                &tenant(),
                &region(),
                &ref_,
                &root,
                &viewer("insider"),
                ResolveMode::Live,
                &bounded_stale(),
                false,
            );
            assert_eq!(
                r.tombstone_reason(),
                Some(want),
                "outcome {outcome:?} → {want:?}"
            );
            if let Resolution::Tombstone(t) = &r {
                assert_eq!(t.root, root, "the {want:?} tombstone carries the root");
            }
        }
    }

    #[test]
    fn owner_project_hiccup_degrades_to_tombstone_no_fabrication() {
        struct HiccupOwner;
        impl ProjectApi for HiccupOwner {
            fn check_view(
                &self,
                _t: &TenantId,
                _r: &Region,
                _o: &ArtifactRef,
                _v: &Principal,
                _p: &Permission,
            ) -> Result<Decision, ProjectApiError> {
                Ok(Decision::Allow)
            }
            fn project(
                &self,
                _t: &TenantId,
                _r: &Region,
                _rf: &ArtifactRef,
                _v: &Principal,
                _m: ResolveMode,
            ) -> Result<ProjectOutcome, ProjectApiError> {
                Err(ProjectApiError::Unavailable("owner down".into()))
            }
        }
        let svc = ResolveService::new(
            authz(),
            Arc::new(NoOpCacheRead),
            Arc::new(HiccupOwner),
            cell(),
        );
        let ref_ = confidential_issue();
        let root = ref_.clone();
        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        assert!(
            r.is_tombstone(),
            "an owner project hiccup degrades to a tombstone, never a fabrication"
        );
    }

    #[test]
    fn resolve_cache_hit_ratio_telemetry_is_emitted() {
        assert_eq!(
            RESOLVE_CACHE_HIT_RATIO_SIGNAL, "resolve_cache_hit_ratio",
            "the 1.8 signal name"
        );
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        let svc = service(owner);
        let ref_ = confidential_issue();
        let root = ref_.clone();
        assert_eq!(
            svc.cache_hit_ratio(),
            None,
            "no denominator before any allowed resolve"
        );
        let _ = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        assert_eq!(
            svc.cache_hit_ratio(),
            Some(0.0),
            "the no-op shim always misses → ratio 0.0"
        );
        assert_eq!(svc.cache_counters(), (0, 1));
    }

    #[test]
    fn cache_hit_ratio_is_a_true_division_not_modulo_or_multiply() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        let ref_ = confidential_issue();
        let root = ref_.clone();
        let cache = Arc::new(MapCacheRead::default());
        cache.put(
            &ref_.0,
            Projection {
                ref_: ref_.clone(),
                title: "t".into(),
                state: "s".into(),
                icon: "i".into(),
                render_hint: "h".into(),
                sub_anchor: None,
                flag: None,
            },
        );
        let svc = ResolveService::new(authz(), cache, owner, cell());
        for _ in 0..3 {
            let _ = svc.resolve(
                &tenant(),
                &region(),
                &ref_,
                &root,
                &viewer("insider"),
                ResolveMode::Live,
                &bounded_stale(),
                false,
            );
        }
        let other = ArtifactRef("myelin://acme/issue/issue/ENG-other".into());
        let _ = svc.resolve(
            &tenant(),
            &region(),
            &other,
            &other,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        assert_eq!(svc.cache_counters(), (3, 1), "3 hits, 1 miss");
        assert_eq!(
            svc.cache_hit_ratio(),
            Some(0.75),
            "the ratio is hits/(hits+misses), a real division"
        );
    }

    #[test]
    fn subscribe_subjects_are_precise_never_a_firehose() {
        let subs = ResolveService::subscribe_subjects(&confidential_issue());
        assert_eq!(
            subs,
            vec!["issue.updated".to_string(), "issue.erased".to_string()]
        );
        for s in &subs {
            assert!(!s.contains('*'), "never a `*` subscription (BUS-4): {s}");
        }
    }

    #[test]
    fn cross_cell_resolution_is_pinned_cell_local() {
        let svc = service(Arc::new(SyntheticOwner::default()));
        assert_eq!(
            svc.cross_cell_disposition(&cell()),
            CrossCellDisposition::Home,
            "a target homed in this cell resolves locally"
        );
        let foreign = CellId::from_token("cell-us-east-1");
        assert_eq!(
            svc.cross_cell_disposition(&foreign),
            CrossCellDisposition::Foreign(foreign.clone()),
            "a foreign target is dispatched to its home cell (only the projection/tombstone crosses)"
        );
        assert_eq!(svc.home_cell(), &cell());
    }

    #[test]
    fn frozen_cross_cell_pointer_drives_disposition() {
        use myelin_tenancy::{ArtifactType, CorrelationId, OpaqueSubjectId};
        let svc = service(Arc::new(SyntheticOwner::default()));
        let foreign = CellId::from_token("cell-us-east-1");
        let ptr = CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef("myelin://acme/issue/issue/42".into())),
            ArtifactType::Issue,
            CorrelationId("01J0CORR".into()),
            foreign.clone(),
        );
        assert_eq!(
            svc.disposition_of_pointer(&ptr),
            CrossCellDisposition::Foreign(foreign),
            "the home cell on the frozen pointer is authoritative"
        );
    }

    #[test]
    fn resolution_classifiers_are_exact() {
        let proj = Resolution::Projection(Projection {
            ref_: confidential_issue(),
            title: "t".into(),
            state: "s".into(),
            icon: "i".into(),
            render_hint: "h".into(),
            sub_anchor: None,
            flag: None,
        });
        assert!(proj.is_projection() && !proj.is_tombstone() && proj.tombstone_reason().is_none());
        let tomb = Resolution::Tombstone(Tombstone {
            root: confidential_issue(),
            reason: TombstoneReason::Denied,
        });
        assert!(tomb.is_tombstone() && !tomb.is_projection());
        assert_eq!(tomb.tombstone_reason(), Some(TombstoneReason::Denied));
    }

    #[test]
    fn cache_is_read_only_after_the_gate_passes() {
        let owner = Arc::new(SyntheticOwner::default());
        let svc = service(owner);
        let ref_ = confidential_issue();
        let root = ref_.clone();
        let _ = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("intruder"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        assert_eq!(
            svc.cache_counters(),
            (0, 0),
            "a denied resolve must NOT read the cache (no leak path)"
        );
    }
}
