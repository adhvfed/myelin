use std::sync::Arc;

use myelin_events::ArtifactRef;
use myelin_identity::{Decision, Permission, Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

use myelin_refs_service::{
    bounded_stale, NoOpCacheRead, OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome,
    Resolution, ResolveMode, ResolveService, TombstoneReason,
};

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

struct ProviderOwner {
    allowed: Vec<String>,
    outcome: ProjectOutcome,
}
impl ProjectApi for ProviderOwner {
    fn check_view(
        &self,
        _t: &TenantId,
        _r: &Region,
        _o: &ArtifactRef,
        viewer: &Principal,
        _p: &Permission,
    ) -> Result<Decision, ProjectApiError> {
        if self.allowed.iter().any(|a| a == &viewer.principal_id.0) {
            Ok(Decision::Allow)
        } else {
            Ok(Decision::Deny)
        }
    }
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        _rf: &ArtifactRef,
        _v: &Principal,
        _m: ResolveMode,
    ) -> Result<ProjectOutcome, ProjectApiError> {
        Ok(self.outcome.clone())
    }
}

fn provider(allowed: Vec<&str>, outcome: ProjectOutcome) -> ResolveService {
    let owner = ProviderOwner {
        allowed: allowed.into_iter().map(String::from).collect(),
        outcome,
    };
    ResolveService::new(
        Arc::new(FailStaticAuthz::try_new(300, &threshold()).expect("valid bound")),
        Arc::new(NoOpCacheRead),
        Arc::new(owner),
        cell(),
    )
}

fn render_unfurl(r: &Resolution) -> String {
    match r {
        Resolution::Projection(p) => {
            format!("[{}] {} ({})", p.icon, p.title, p.state)
        }
        Resolution::Tombstone(t) => {
            format!("referenced {} (unavailable: {:?})", t.root.0, t.reason)
        }
    }
}

fn live_secret() -> ProjectOutcome {
    ProjectOutcome::Live(OwnerProjection {
        title: "TOP SECRET acquisition plan".into(),
        state: "open".into(),
        icon: "lock".into(),
        render_hint: "issue-card".into(),
        sub_anchor: None,
        flag: None,
    })
}

#[test]
fn cdc_5_2_allowed_viewer_resolves_to_a_projection() {
    let svc = provider(vec!["insider"], live_secret());
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-secret".into());
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
        r.is_projection(),
        "PROVIDER: an allowed viewer resolves to a Projection"
    );
    let rendered = render_unfurl(&r);
    assert_eq!(rendered, "[lock] TOP SECRET acquisition plan (open)");
}

#[test]
fn cdc_5_2_denied_viewer_resolves_to_a_tombstone_with_zero_content() {
    let svc = provider(vec!["insider"], live_secret());
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-secret".into());
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
        "PROVIDER: a denied viewer resolves to a Tombstone"
    );
    assert_eq!(r.tombstone_reason(), Some(TombstoneReason::Denied));
    let rendered = render_unfurl(&r);
    assert!(
        !rendered.contains("SECRET") && !rendered.contains("acquisition"),
        "CONSUMER LEAK INVARIANT: the denied unfurl must not contain the secret title, got `{rendered}`"
    );
    assert!(
        rendered.contains("myelin://acme/issue/issue/ENG-secret"),
        "the tombstone carries the root"
    );
}

#[test]
fn cdc_5_2_sub_ladder_reasons_are_contracted() {
    for (outcome, want) in [
        (ProjectOutcome::RootGone, TombstoneReason::RootGone),
        (ProjectOutcome::SubGone, TombstoneReason::SubGone),
        (ProjectOutcome::Erased, TombstoneReason::Erased),
    ] {
        let svc = provider(vec!["insider"], outcome.clone());
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
            "PROVIDER: outcome {outcome:?} → {want:?}"
        );
        let rendered = render_unfurl(&r);
        assert!(rendered.starts_with("referenced myelin://acme/knowledge/page/7c2"));
    }
}

#[test]
fn cdc_5_2_consumer_trusts_the_chokepoint_total_match() {
    let svc = provider(vec!["insider"], live_secret());
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-secret".into());
    let root = ref_.clone();
    for who in ["insider", "intruder"] {
        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer(who),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        let _rendered: String = match r {
            Resolution::Projection(p) => p.title,
            Resolution::Tombstone(t) => format!("{:?}", t.reason),
        };
    }
}
