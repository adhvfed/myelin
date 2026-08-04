use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::events;
use myelin_issues::time_axis::{
    emit_membership_edge, rollover_carry_over, time_axis_ref, MembershipEdge, MembershipKind,
};
use myelin_issues::workflow::StateCategory;
use myelin_tenancy::TenantId;
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(Actor_principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:e2e".into())),
    }
}

#[allow(non_snake_case)]
fn Actor_principal() -> Principal {
    Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, tenant())
}

fn issue(key: &str) -> myelin_events::ArtifactRef {
    myelin_events::ArtifactRef(format!("myelin://acme/issue/issue/{key}"))
}

#[test]
fn add_then_rollover_preserves_carry_over_provenance_end_to_end() {
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let fresh = MembershipEdge::new(
        MembershipKind::Cycle,
        issue("ENG-1"),
        time_axis_ref("acme", MembershipKind::Cycle, "C-7"),
    );
    let mut tx = store.begin(minter.clone(), ctx_base());
    tx.stage_state_change("INSERT cycle_membership (C-7, ENG-1)");
    let add_id = emit_membership_edge(&mut tx, &fresh, true, None).unwrap();
    tx.commit().unwrap();

    let add_row = store.row(&add_id).unwrap();
    assert_eq!(add_row.envelope.type_.0, events::CYCLE_ISSUE_ADDED);
    assert!(
        add_row.envelope.payload["carried_over_from"].is_null(),
        "a fresh add has no carry-over provenance"
    );
    assert_eq!(add_row.envelope.payload["rel"], "member_of_cycle");
    assert_ne!(add_row.envelope.type_.0, events::ISSUE_PARENT_CHANGED);

    let carried = rollover_carry_over(
        "acme",
        "C-7",
        "C-8",
        &[(issue("ENG-1"), StateCategory::Started)],
    );
    assert_eq!(carried.len(), 1, "the one unfinished issue carries over");
    assert!(carried[0].is_carried_over());

    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("INSERT cycle_membership (C-8, ENG-1, carried_over_from=C-7)");
    let carry_id = emit_membership_edge(&mut tx, &carried[0], true, None).unwrap();
    tx.commit().unwrap();

    let carry_row = store.row(&carry_id).unwrap();
    assert_eq!(carry_row.envelope.type_.0, events::CYCLE_ISSUE_ADDED);
    assert_eq!(
        carry_row.envelope.payload["carried_over_from"], "myelin://acme/issue/cycle/C-7",
        "the destination membership names the SOURCE cycle"
    );
    assert_eq!(
        carry_row.envelope.payload["source"], "myelin://acme/issue/cycle/C-8",
        "the membership lands in the destination cycle"
    );
    assert_eq!(carry_row.envelope.payload["target"], issue("ENG-1").0);

    assert_eq!(store.outbox_depth(), 2);
}

#[test]
fn finished_issues_are_not_carried_over_on_rollover() {
    let carried = rollover_carry_over(
        "acme",
        "C-7",
        "C-8",
        &[
            (issue("ENG-1"), StateCategory::Started),
            (issue("ENG-2"), StateCategory::Completed),
        ],
    );
    assert_eq!(carried.len(), 1);
    assert_eq!(carried[0].issue.0, issue("ENG-1").0);
}
