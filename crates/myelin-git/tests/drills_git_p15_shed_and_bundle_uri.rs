use myelin_git::shed_clone::{BundleUriClone, GitFrontDoorShed};
use myelin_identity::{
    DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef,
};
use myelin_storage::blob::FsBlobStore;
use myelin_storage::cdn::CdnCloneClass;
use myelin_substrate::shed::{RunClass, Surface};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

fn tenant(s: &str) -> TenantId {
    TenantId::from_token(s)
}

fn principal(tenant_slug: &str, kind: PrincipalKind) -> Principal {
    Principal::new(
        tenant(tenant_slug),
        Region("fr-par".into()),
        PrincipalId(format!("p-{tenant_slug}-{}", kind_tag(&kind))),
        kind,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn kind_tag(kind: &PrincipalKind) -> &'static str {
    match kind {
        PrincipalKind::Human => "h",
        PrincipalKind::Agent { .. } => "a",
        PrincipalKind::Service => "s",
    }
}

fn agent_kind() -> PrincipalKind {
    PrincipalKind::Agent {
        runtime_ref: RuntimeRef("rt".into()),
        on_behalf_of: None,
    }
}

#[test]
fn git_p15_storm_protects_the_human_then_serves_the_accelerated_bundle_uri_clone() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let budget = thresholds
        .shed_budget(Surface::GitFrontDoor)
        .expect("GitFrontDoor budget present");
    let mut shed = GitFrontDoorShed::from_thresholds(&thresholds).expect("open the shed gate");

    let t = "acme";
    let human = principal(t, PrincipalKind::Human);

    let mut agent_admitted = 0u32;
    let mut agent_shed = 0u32;
    let mut ci_shed = 0u32;
    for i in 0..(budget.per_tenant_in_flight_cap * 2) {
        if i % 2 == 0 {
            let a = principal(t, agent_kind());
            match shed.admit_for(&a, None) {
                Ok(_) => agent_admitted += 1,
                Err(rej) => {
                    assert_eq!(rej.lane, RunClass::Agent);
                    agent_shed += 1;
                }
            }
        } else {
            let svc = principal(t, PrincipalKind::Service);
            if let Err(rej) = shed.admit_for(&svc, None) {
                assert_eq!(rej.lane, RunClass::BatchCi, "the CI/batch lane sheds");
                ci_shed += 1;
            }
        }
    }
    assert!(agent_shed >= 1, "the agent lane shed under the storm");
    assert!(ci_shed >= 1, "the CI/batch lane shed under the storm");
    assert!(
        agent_admitted <= budget.per_tenant_in_flight_cap - budget.human_lane_reservation,
        "the machine lanes never take the reserved human slots"
    );

    let human_class = shed
        .admit_for(&human, None)
        .expect("the human's interactive fetch is served while the storm sheds the machine lanes");
    assert_eq!(human_class, RunClass::Human);

    assert_eq!(
        shed.shed_count(RunClass::Human),
        0,
        "human lane: 0 shed (served under the storm)"
    );
    assert!(shed.shed_count(RunClass::Agent) >= 1, "agent lane: shed");
    assert!(
        shed.shed_count(RunClass::BatchCi) >= 1,
        "CI/batch lane: shed"
    );

    let store = FsBlobStore::new();
    let cdn = CdnCloneClass::over(tenant(t), Region::new("fr-par"), true, &store);
    let bundle = BundleUriClone::new(cdn);

    let repo_bundle_bytes = b"PACK\0acme/widgets-clone-bundle@deadbeefcafe";
    let uri = bundle
        .publish_bundle(repo_bundle_bytes)
        .expect("serving tier publishes the bundle");
    let cloned = bundle
        .clone_via_bundle_uri(&uri)
        .expect("the bundle-URI clone round-trips a valid clone");
    assert_eq!(
        cloned, repo_bundle_bytes,
        "the accelerated-clone floor holds - the bundle-URI clone is valid + content-address-verified"
    );

    shed.release(&tenant(t), RunClass::Human);
}
