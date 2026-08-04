use myelin_identity::Principal;
use myelin_storage::blob::{BlobError, ContentHash};
use myelin_storage::cdn::CdnCloneClass;
use myelin_substrate::shed::{
    RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

pub struct GitFrontDoorShed {
    lane: ShedLane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShedRejection {
    pub lane: RunClass,
    pub retry_after_secs: u64,
}

impl GitFrontDoorShed {
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<GitFrontDoorShed, String> {
        let budget = thresholds
            .shed_budget(Surface::GitFrontDoor)
            .map_err(|e| format!("Git front-door shed budget unavailable: {e}"))?;
        Ok(GitFrontDoorShed {
            lane: ShedLane::with_budget(Surface::GitFrontDoor, budget),
        })
    }

    pub fn with_budget(budget: SurfaceBudget) -> GitFrontDoorShed {
        GitFrontDoorShed {
            lane: ShedLane::with_budget(Surface::GitFrontDoor, budget),
        }
    }

    pub fn admit_for(
        &mut self,
        principal: &Principal,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, ShedRejection> {
        let class = RunClass::derive(&principal.kind, header);
        self.admit_class(&principal.tenant, class).map(|()| class)
    }

    pub fn admit_class(&mut self, tenant: &TenantId, class: RunClass) -> Result<(), ShedRejection> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(ShedRejection {
                lane: class,
                retry_after_secs,
            }),
        }
    }

    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        self.lane.release(tenant, class);
    }

    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleUri {
    pub tenant: TenantId,
    pub content_hash: ContentHash,
}

pub struct BundleUriClone<'a> {
    cdn: CdnCloneClass<'a>,
}

impl<'a> BundleUriClone<'a> {
    pub fn new(cdn: CdnCloneClass<'a>) -> BundleUriClone<'a> {
        BundleUriClone { cdn }
    }

    pub fn publish_bundle(&self, bundle_bytes: &[u8]) -> Result<BundleUri, BlobError> {
        let content_hash = self.cdn.publish_bundle(bundle_bytes)?;
        Ok(BundleUri {
            tenant: self.cdn.tenant().clone(),
            content_hash,
        })
    }

    pub fn clone_via_bundle_uri(&self, uri: &BundleUri) -> Result<Vec<u8>, BundleCloneError> {
        if &uri.tenant != self.cdn.tenant() {
            return Err(BundleCloneError::CrossTenant {
                uri_tenant: uri.tenant.as_str().to_string(),
                class_tenant: self.cdn.tenant().as_str().to_string(),
            });
        }
        self.cdn
            .bundle(&uri.content_hash)
            .map_err(|e| BundleCloneError::Fetch {
                detail: e.to_string(),
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleCloneError {
    Fetch {
        detail: String,
    },
    CrossTenant {
        uri_tenant: String,
        class_tenant: String,
    },
}

impl std::fmt::Display for BundleCloneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BundleCloneError::Fetch { detail } => write!(
                f,
                "bundle-URI clone REFUSED - the bundle failed the content-address verify ({detail}); \
                 the content-address is the cache-validity check (0 silent serve)"
            ),
            BundleCloneError::CrossTenant {
                uri_tenant,
                class_tenant,
            } => write!(
                f,
                "bundle-URI clone REFUSED - URI tenant `{uri_tenant}` ≠ serving-class tenant \
                 `{class_tenant}` (the bundle keyspace is per-tenant)"
            ),
        }
    }
}

impl std::error::Error for BundleCloneError {}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef};
    use myelin_storage::blob::FsBlobStore;
    use myelin_tenancy::Region;

    fn tenant(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn human(tenant_slug: &str) -> Principal {
        Principal::new(
            tenant(tenant_slug),
            Region("fr-par".into()),
            PrincipalId(format!("h-{tenant_slug}")),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn agent(tenant_slug: &str) -> Principal {
        Principal::new(
            tenant(tenant_slug),
            Region("fr-par".into()),
            PrincipalId(format!("a-{tenant_slug}")),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: None,
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn small_budget() -> SurfaceBudget {
        SurfaceBudget {
            per_tenant_in_flight_cap: 6,
            human_lane_reservation: 2,
            retry_after_secs: 5,
        }
    }

    #[test]
    fn the_git_front_door_shed_budget_is_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let gate =
            GitFrontDoorShed::from_thresholds(&thresholds).expect("GitFrontDoor budget present");
        let file_budget = thresholds
            .shed_budget(Surface::GitFrontDoor)
            .expect("present");
        assert!(file_budget.per_tenant_in_flight_cap > 0, "bounded (§7.1)");
        assert!(
            file_budget.human_lane_reservation > 0,
            "the human lane is reserved"
        );
        assert_eq!(gate.lane.surface(), Surface::GitFrontDoor);
    }

    #[test]
    fn shed_order_serves_the_human_while_the_agent_lane_sheds() {
        let mut gate = GitFrontDoorShed::with_budget(small_budget());
        let a = agent("acme");
        let h = human("acme");

        for _ in 0..4 {
            assert!(
                gate.admit_for(&a, None).is_ok(),
                "agent fetch admitted while under the non-human budget"
            );
        }
        let shed = gate
            .admit_for(&a, None)
            .expect_err("the agent clone storm sheds");
        assert_eq!(shed.lane, RunClass::Agent);
        assert_eq!(
            shed.retry_after_secs, 5,
            "the shed carries a Retry-After (clients honour it)"
        );

        assert_eq!(
            gate.admit_for(&h, None)
                .expect("the human is served while the agent sheds"),
            RunClass::Human
        );

        assert_eq!(
            gate.shed_count(RunClass::Human),
            0,
            "human lane: 0 shed (served)"
        );
        assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
    }

    #[test]
    fn shed_priority_is_speculative_then_batch_then_agent_then_human() {
        let mut gate = GitFrontDoorShed::with_budget(small_budget());
        let t = tenant("acme");
        for _ in 0..2 {
            gate.admit_class(&t, RunClass::Agent)
                .expect("agent admitted");
        }
        assert!(
            gate.admit_class(&t, RunClass::Speculative).is_err(),
            "speculative sheds first"
        );
        gate.admit_class(&t, RunClass::BatchCi)
            .expect("batch admitted");
        assert!(
            gate.admit_class(&t, RunClass::BatchCi).is_err(),
            "batch/ci sheds next"
        );
        gate.admit_class(&t, RunClass::Agent)
            .expect("agent admitted");
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "agent sheds before the human"
        );
        gate.admit_class(&t, RunClass::Human)
            .expect("human served - shed last");

        assert_eq!(gate.shed_count(RunClass::Speculative), 1);
        assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
        assert_eq!(gate.shed_count(RunClass::Agent), 1);
        assert_eq!(
            gate.shed_count(RunClass::Human),
            0,
            "the human lane is never shed here"
        );
    }

    #[test]
    fn one_tenants_storm_never_sheds_anothers_human() {
        let mut gate = GitFrontDoorShed::with_budget(small_budget());
        let noisy = agent("noisy");
        let quiet_human = human("quiet");

        for _ in 0..4 {
            gate.admit_for(&noisy, None).expect("noisy agent admitted");
        }
        assert!(
            gate.admit_for(&noisy, None).is_err(),
            "noisy agent lane sheds"
        );

        assert_eq!(
            gate.in_flight(&tenant("noisy")),
            4,
            "the noisy tenant has 4 in-flight machine fetches"
        );
        assert_eq!(
            gate.in_flight(&tenant("quiet")),
            0,
            "the quiet tenant's budget is independent"
        );
        assert_eq!(
            gate.admit_for(&quiet_human, None)
                .expect("the quiet human is served"),
            RunClass::Human,
            "the noisy clone storm must NEVER shed another tenant's human",
        );
    }

    #[test]
    fn a_machine_principal_cannot_spoof_the_human_lane() {
        let mut gate = GitFrontDoorShed::with_budget(small_budget());
        let a = agent("acme");
        assert_eq!(gate.admit_for(&a, None).expect("admitted"), RunClass::Agent);
        let h = human("acme");
        assert_eq!(
            gate.admit_for(&h, Some(RunClassHeader::Speculative))
                .expect("admitted"),
            RunClass::Speculative,
            "a human-issued prefetch may down-class itself (sheds earlier)",
        );
    }

    #[test]
    fn release_frees_a_slot_after_the_storm() {
        let mut gate = GitFrontDoorShed::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        });
        let t = tenant("acme");
        gate.admit_class(&t, RunClass::Agent).expect("admitted");
        gate.admit_class(&t, RunClass::Agent).expect("admitted");
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "agent sheds at cap-reserved"
        );
        gate.release(&t, RunClass::Agent);
        gate.admit_class(&t, RunClass::Agent)
            .expect("a released slot is reusable");
    }

    fn eu_cdn<'a>(store: &'a FsBlobStore, t: &str) -> CdnCloneClass<'a> {
        CdnCloneClass::over(tenant(t), Region::new("fr-par"), true, store)
    }

    #[test]
    fn a_bundle_uri_clone_round_trips_a_valid_clone() {
        let store = FsBlobStore::new();
        let path = BundleUriClone::new(eu_cdn(&store, "acme"));

        let bundle_bytes = b"PACK\0clone-bundle-of-hot-repo@deadbeef";
        let uri = path
            .publish_bundle(bundle_bytes)
            .expect("publish bundle → bundle-URI");
        assert_eq!(uri.content_hash, ContentHash::blake3(bundle_bytes));
        assert_eq!(uri.tenant, tenant("acme"));

        let cloned = path
            .clone_via_bundle_uri(&uri)
            .expect("clone via bundle-URI");
        assert_eq!(
            cloned, bundle_bytes,
            "the bundle-URI clone round-trips the exact repo bytes"
        );
    }

    #[test]
    fn a_tampered_bundle_is_refused_zero_silent_serve() {
        let store = FsBlobStore::new();
        let path = BundleUriClone::new(eu_cdn(&store, "acme"));
        let uri = path.publish_bundle(b"valid-clone-bundle").expect("publish");

        assert!(
            store.corrupt_for_drill(&tenant("acme"), &uri.content_hash),
            "bundle present"
        );
        let err = path
            .clone_via_bundle_uri(&uri)
            .expect_err("a tampered bundle MUST be refused");
        assert!(
            matches!(err, BundleCloneError::Fetch { .. }),
            "0 silent serve: {err}"
        );
    }

    #[test]
    fn a_cross_tenant_bundle_uri_is_refused() {
        let store = FsBlobStore::new();
        let path = BundleUriClone::new(eu_cdn(&store, "acme"));
        let foreign = BundleUri {
            tenant: tenant("globex"),
            content_hash: ContentHash::blake3(b"whatever"),
        };
        let err = path
            .clone_via_bundle_uri(&foreign)
            .expect_err("a foreign-tenant URI is refused");
        assert!(matches!(err, BundleCloneError::CrossTenant { .. }), "{err}");
    }

    #[test]
    fn bundle_clone_error_display_is_distinct() {
        let fetch = BundleCloneError::Fetch { detail: "x".into() };
        let xtenant = BundleCloneError::CrossTenant {
            uri_tenant: "globex".into(),
            class_tenant: "acme".into(),
        };
        let s1 = fetch.to_string();
        let s2 = xtenant.to_string();
        assert!(s1.contains("content-address") && !s1.is_empty());
        assert!(s2.contains("per-tenant") && !s2.is_empty());
        assert_ne!(s1, s2);
    }
}
