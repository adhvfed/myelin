use myelin_git::shed_clone::GitFrontDoorShed;
use myelin_identity::{
    DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef,
};
use myelin_substrate::shed::{RunClass, RunClassHeader, Surface};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

fn tenant(s: &str) -> TenantId {
    TenantId::from_token(s)
}

fn principal(tenant_slug: &str, kind: PrincipalKind) -> Principal {
    Principal::new(
        tenant(tenant_slug),
        Region("fr-par".into()),
        PrincipalId(format!("p-{tenant_slug}")),
        kind,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn agent_kind() -> PrincipalKind {
    PrincipalKind::Agent {
        runtime_ref: RuntimeRef("rt".into()),
        on_behalf_of: None,
    }
}

#[test]
fn cdc_1_11_git_front_door_reads_its_shed_budget_from_the_thresholds_file() {
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds.toml loads");
    let budget = thresholds
        .shed_budget(Surface::GitFrontDoor)
        .expect("the thresholds file carries a GitFrontDoor shed-budget row (OQ-K)");
    assert!(
        budget.per_tenant_in_flight_cap > 0,
        "the surface is bounded (§7.1)"
    );
    assert!(
        budget.human_lane_reservation > 0,
        "a human lane is reserved"
    );
    assert!(
        budget.retry_after_secs > 0,
        "the surface sheds with a Retry-After (clients honour it)"
    );

    GitFrontDoorShed::from_thresholds(&thresholds).expect("the front-door shed gate opens");
}

#[test]
fn cdc_1_11_git_shed_serves_the_human_and_sheds_the_agent_lane() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let budget = thresholds
        .shed_budget(Surface::GitFrontDoor)
        .expect("present");
    let mut gate = GitFrontDoorShed::from_thresholds(&thresholds).expect("open");

    let t = "acme";
    let a = principal(t, agent_kind());
    let h = principal(t, PrincipalKind::Human);

    let mut admitted = 0u32;
    let mut agent_shed = false;
    for _ in 0..(budget.per_tenant_in_flight_cap + 4) {
        match gate.admit_for(&a, None) {
            Ok(_) => admitted += 1,
            Err(rej) => {
                assert_eq!(rej.lane, RunClass::Agent, "the agent lane sheds");
                assert_eq!(
                    rej.retry_after_secs, budget.retry_after_secs,
                    "the shed carries the file's Retry-After"
                );
                agent_shed = true;
            }
        }
    }
    assert!(agent_shed, "an over-budget agent clone storm sheds");
    assert!(
        admitted <= budget.per_tenant_in_flight_cap - budget.human_lane_reservation,
        "the agent lane never consumes the reserved human slots"
    );

    assert_eq!(
        gate.admit_for(&h, None)
            .expect("the human is served while the agent sheds"),
        RunClass::Human
    );
    assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed");
    assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
}

#[test]
fn cdc_1_11_a_machine_principal_cannot_spoof_the_human_lane() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let mut gate = GitFrontDoorShed::from_thresholds(&thresholds).expect("open");

    let svc = principal("acme", PrincipalKind::Service);
    assert_eq!(
        gate.admit_for(&svc, None).expect("admitted"),
        RunClass::BatchCi
    );
    let svc2 = principal("acme", PrincipalKind::Service);
    assert_eq!(
        gate.admit_for(&svc2, Some(RunClassHeader::Speculative))
            .expect("admitted"),
        RunClass::Speculative,
        "the human lane is structurally unspoofable (no human header exists)"
    );
}
