//! Contract 1.11 CDC pair — git's **consumer half** of the protected-human-lane shed order
//! (GIT-P15 / global P-276, M3-G2).
//!
//! The substrate-side CDC (`myelin-substrate/tests/cdc_1_11_shed_order.rs`, P-035) owns the shed
//! lane / run-class / budget-table PROVIDER. GIT-P15 wires the REAL git consumer — the
//! [`myelin_git::shed_clone::GitFrontDoorShed`] over the new `Surface::GitFrontDoor` budget read from
//! the thresholds file — so this is the consumer-driven contract test with the ACTUAL consumer type:
//!
//! - **PROVIDER:** `myelin-substrate` — [`myelin_substrate::shed::ShedLane`] (the shed order
//!   `speculative → batch/CI → agent → human-last`, `429 + Retry-After`, per-tenant) + the
//!   `Surface::GitFrontDoor` budget row exposed through [`myelin_substrate::thresholds`].
//! - **CONSUMER:** `myelin-git` — the [`myelin_git::shed_clone::GitFrontDoorShed`] front-door gate.
//!
//! The load-bearing contract this pins: the Git front door reads its per-surface budget FROM THE
//! THRESHOLDS FILE (never a hardcoded number), derives the run-class from the verified principal
//! (a machine principal can never up-class to the protected human lane), and applies the shed order
//! with `429 + Retry-After` on the shed lane. If the provider's shed shapes drift, this stops
//! compiling/passing.

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

/// **CDC: the git front-door shed gate reads its budget from the thresholds file (the contract).** A
/// missing `GitFrontDoor` shed-budget row would be a loud error — the gate opens against the file's
/// row, never a guessed default.
#[test]
fn cdc_1_11_git_front_door_reads_its_shed_budget_from_the_thresholds_file() {
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds.toml loads");
    // the GitFrontDoor surface budget is present in the file (the new OQ-K per-surface floor).
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

    // the consumer opens against the file's budget.
    GitFrontDoorShed::from_thresholds(&thresholds).expect("the front-door shed gate opens");
}

/// **CDC: the shed order serves the human while the agent lane sheds with 429 + Retry-After (the
/// contract behaviour the git consumer relies on).** Run against the REAL thresholds-file budget so
/// the consumer-driven test exercises the production budget path end-to-end (the storm is sized to
/// the file's cap).
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

    // fill the entire per-tenant cap with agent fetches so the agent lane is forced to shed.
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
    // the agent lane filled only the non-reserved budget (it never took the human's reserved slots).
    assert!(
        admitted <= budget.per_tenant_in_flight_cap - budget.human_lane_reservation,
        "the agent lane never consumes the reserved human slots"
    );

    // THE CONTRACT: the human's interactive fetch is STILL SERVED (the protected lane).
    assert_eq!(
        gate.admit_for(&h, None)
            .expect("the human is served while the agent sheds"),
        RunClass::Human
    );
    // the green-artifact signal: human lane 0 shed, agent lane sheds.
    assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed");
    assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
}

/// **CDC: a machine principal can never up-class to the human lane (the structural-unspoofability
/// contract).** A header may only DOWN-class (a human-issued prefetch); there is no human header.
#[test]
fn cdc_1_11_a_machine_principal_cannot_spoof_the_human_lane() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let mut gate = GitFrontDoorShed::from_thresholds(&thresholds).expect("open");

    // a service principal derives the batch/ci lane (no header).
    let svc = principal("acme", PrincipalKind::Service);
    assert_eq!(
        gate.admit_for(&svc, None).expect("admitted"),
        RunClass::BatchCi
    );
    // a header may only down-class — a service declaring speculative is speculative, never human.
    let svc2 = principal("acme", PrincipalKind::Service);
    assert_eq!(
        gate.admit_for(&svc2, Some(RunClassHeader::Speculative))
            .expect("admitted"),
        RunClass::Speculative,
        "the human lane is structurally unspoofable (no human header exists)"
    );
}
