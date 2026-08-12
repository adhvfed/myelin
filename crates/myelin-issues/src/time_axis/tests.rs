use super::*;
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region as BusRegion,
    Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::blob::FsBlobStore;
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

fn issue(key: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/issue/issue/{key}"))
}

fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
    (
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
    )
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: BusRegion("fr-par".into()),
        actor: Actor(viewer("p")),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

#[test]
fn cycle_membership_is_an_edge_not_containment() {
    let (store, minter) = store_and_minter();
    let cycle = time_axis_ref("acme", MembershipKind::Cycle, "C-7");
    let edge = MembershipEdge::new(MembershipKind::Cycle, issue("ENG-1"), cycle.clone());

    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("cycle_membership row inserted");
    let id = emit_membership_edge(&mut tx, &edge, true, None).unwrap();
    tx.commit().unwrap();

    assert_eq!(store.outbox_depth(), 1, "ONE mirror event per edge write");
    let row = store.row(&id).unwrap();
    assert_eq!(row.envelope.type_.0, events::CYCLE_ISSUE_ADDED);
    assert_ne!(
        row.envelope.type_.0,
        events::ISSUE_PARENT_CHANGED,
        "a cycle add is NOT a containment re-parent"
    );
    assert_eq!(row.envelope.payload["rel_class"], REL_CLASS_LIFECYCLE);
    assert_eq!(row.envelope.payload["rel"], "member_of_cycle");
    assert_eq!(row.envelope.payload["source"], cycle.0);
    assert_eq!(row.envelope.payload["target"], issue("ENG-1").0);
    assert!(!row.envelope.contains_personal_data);
}

#[test]
fn milestone_membership_is_the_same_edge_shape() {
    let (store, minter) = store_and_minter();
    let ms = time_axis_ref("acme", MembershipKind::Milestone, "v2.0");
    let edge = MembershipEdge::new(MembershipKind::Milestone, issue("ENG-2"), ms.clone());

    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("milestone membership row inserted");
    let id = emit_membership_edge(&mut tx, &edge, true, None).unwrap();
    tx.commit().unwrap();

    let row = store.row(&id).unwrap();
    assert_eq!(row.envelope.type_.0, events::MILESTONE_ISSUE_ADDED);
    assert_eq!(row.envelope.payload["rel"], "member_of_milestone");
    assert_eq!(row.envelope.payload["rel_class"], REL_CLASS_LIFECYCLE);
}

#[test]
fn membership_remove_emits_the_removed_mirror_on_the_same_aggregate() {
    let (store, minter) = store_and_minter();
    let cycle = time_axis_ref("acme", MembershipKind::Cycle, "C-7");
    let edge = MembershipEdge::new(MembershipKind::Cycle, issue("ENG-1"), cycle);

    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("membership added then removed");
    let add = emit_membership_edge(&mut tx, &edge, true, None).unwrap();
    let rem = emit_membership_edge(&mut tx, &edge, false, None).unwrap();
    tx.commit().unwrap();

    let add_row = store.row(&add).unwrap();
    let rem_row = store.row(&rem).unwrap();
    assert_eq!(add_row.envelope.type_.0, events::CYCLE_ISSUE_ADDED);
    assert_eq!(rem_row.envelope.type_.0, events::CYCLE_ISSUE_REMOVED);
    assert_eq!(add_row.envelope.aggregate, rem_row.envelope.aggregate);
}

#[test]
fn aborted_membership_write_emits_nothing() {
    let (store, minter) = store_and_minter();
    let cycle = time_axis_ref("acme", MembershipKind::Cycle, "C-7");
    let edge = MembershipEdge::new(MembershipKind::Cycle, issue("ENG-1"), cycle);

    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("membership row staged but the tx is dropped");
    let _ = emit_membership_edge(&mut tx, &edge, true, None).unwrap();
    drop(tx);

    assert_eq!(store.outbox_depth(), 0, "0 committed events on an abort");
    assert!(store.committed_rows().is_empty());
}

#[test]
fn rollover_carries_unfinished_issues_with_provenance() {
    let members = vec![
        (issue("ENG-1"), StateCategory::Started),
        (issue("ENG-2"), StateCategory::Unstarted),
        (issue("ENG-3"), StateCategory::Completed),
        (issue("ENG-4"), StateCategory::Cancelled),
    ];
    let carried = rollover_carry_over("acme", "C-7", "C-8", &members);

    assert_eq!(
        carried.len(),
        2,
        "only the two unfinished issues carry over"
    );
    for edge in &carried {
        assert!(edge.is_carried_over(), "every carried edge has provenance");
        assert_eq!(edge.kind, MembershipKind::Cycle);
        assert_eq!(
            edge.carried_over_from.as_ref().unwrap().0,
            "myelin://acme/issue/cycle/C-7"
        );
        assert_eq!(edge.axis.0, "myelin://acme/issue/cycle/C-8");
    }
    let issues: Vec<&str> = carried.iter().map(|e| e.issue.0.as_str()).collect();
    assert!(issues.contains(&"myelin://acme/issue/issue/ENG-1"));
    assert!(issues.contains(&"myelin://acme/issue/issue/ENG-2"));
}

#[test]
fn carry_over_provenance_rides_the_mirror_event() {
    let (store, minter) = store_and_minter();

    let mut tx = store.begin(minter.clone(), ctx_base());
    tx.stage_state_change("add ENG-1 to C-7");
    let fresh = MembershipEdge::new(
        MembershipKind::Cycle,
        issue("ENG-1"),
        time_axis_ref("acme", MembershipKind::Cycle, "C-7"),
    );
    let add_id = emit_membership_edge(&mut tx, &fresh, true, None).unwrap();
    tx.commit().unwrap();
    assert!(
        store.row(&add_id).unwrap().envelope.payload["carried_over_from"].is_null(),
        "a fresh add carries no provenance"
    );

    let carried = rollover_carry_over(
        "acme",
        "C-7",
        "C-8",
        &[(issue("ENG-1"), StateCategory::Started)],
    );
    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("carry ENG-1 over to C-8");
    let carry_id = emit_membership_edge(&mut tx, &carried[0], true, None).unwrap();
    tx.commit().unwrap();

    let row = store.row(&carry_id).unwrap();
    assert_eq!(
        row.envelope.payload["carried_over_from"],
        "myelin://acme/issue/cycle/C-7"
    );
    assert_eq!(
        row.envelope.payload["source"],
        "myelin://acme/issue/cycle/C-8"
    );
}

#[test]
fn cfd_band_tallies_all_four_fixed_categories() {
    let cycle = time_axis_ref("acme", MembershipKind::Cycle, "C-7");
    let members = vec![
        StateCategory::Unstarted,
        StateCategory::Unstarted,
        StateCategory::Started,
        StateCategory::Completed,
        StateCategory::Cancelled,
    ];
    let band = CfdBand::tally(&cycle, "2026-06-23T00:00:00Z", &members);
    assert_eq!(band.unstarted, 2);
    assert_eq!(band.started, 1);
    assert_eq!(band.completed, 1);
    assert_eq!(band.cancelled, 1);
    assert_eq!(
        band.total(),
        5,
        "no member dropped - the sum equals the count"
    );
}

#[test]
fn attachment_row_holds_zero_bytes_and_a_dek_pointer() {
    let blob = FsBlobStore::new();
    let bytes = b"a screenshot's PNG bytes that must NEVER touch the OLTP row";
    let subject = SubjectId("u42".into());

    let pointer = attach(&blob, &tenant(), &subject, 3, "fr-par", "image/png", bytes).unwrap();

    assert_eq!(
        pointer.row_byte_count(),
        0,
        "the OLTP row holds 0 attachment bytes"
    );
    assert_eq!(pointer.blob_ref, ContentHash::blake3(bytes));
    assert!(pointer
        .blob_ref
        .to_multihash_string()
        .starts_with("blake3:"));
    assert_eq!(pointer.pii_key_ref, "kms://acme/3/subject:u42");
    assert_eq!(pointer.size_bytes, bytes.len() as u64);
    assert_eq!(pointer.region, "fr-par");
    assert_eq!(pointer.content_type, "image/png");

    let fetched = pointer.fetch_bytes(&blob, &tenant()).unwrap();
    assert_eq!(fetched, bytes, "the bytes round-trip from the blob tier");
}

#[test]
fn attachment_event_carries_the_pointer_never_the_bytes() {
    let (store, minter) = store_and_minter();
    let blob = FsBlobStore::new();
    let bytes = b"PII-bearing attachment bytes";
    let subject = SubjectId("u42".into());
    let pointer = attach(&blob, &tenant(), &subject, 3, "fr-par", "text/plain", bytes).unwrap();

    let issue_ref = issue("ENG-9");
    let agg = myelin_events::AggregateKey("issue:eng:ENG-9".into());
    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("attachment pointer row inserted");
    let id = emit_attachment(&mut tx, &issue_ref, agg, &pointer, true, None).unwrap();
    tx.commit().unwrap();

    let row = store.row(&id).unwrap();
    assert_eq!(row.envelope.type_.0, events::ATTACHMENT_ADDED);
    assert_eq!(
        row.envelope.payload["blob_ref"],
        pointer.blob_ref.to_multihash_string()
    );
    assert_eq!(
        row.envelope.payload["pii_key_ref"],
        "kms://acme/3/subject:u42"
    );
    assert!(row.envelope.contains_personal_data);
    assert_eq!(
        row.envelope.pii_key_ref.as_ref().unwrap().0,
        "kms://acme/3/subject:u42"
    );
    let payload_str = row.envelope.payload.to_string();
    assert!(
        !payload_str.contains("PII-bearing attachment bytes"),
        "the attachment bytes must never appear in the event payload"
    );
}

#[test]
fn identical_attachment_bytes_dedup_to_one_address() {
    let blob = FsBlobStore::new();
    let subject = SubjectId("u42".into());
    let bytes = b"the same bytes twice";
    let p1 = attach(
        &blob,
        &tenant(),
        &subject,
        1,
        "fr-par",
        "application/octet-stream",
        bytes,
    )
    .unwrap();
    let p2 = attach(
        &blob,
        &tenant(),
        &subject,
        1,
        "fr-par",
        "application/octet-stream",
        bytes,
    )
    .unwrap();
    assert_eq!(p1.blob_ref, p2.blob_ref, "content-addressed: one address");
}

#[test]
fn the_new_tokens_are_registered_and_grammatical() {
    events::register_issue_tokens().expect("every issue token parses the bus grammar");
    for tok in [
        events::MILESTONE_ISSUE_ADDED,
        events::MILESTONE_ISSUE_REMOVED,
        events::ATTACHMENT_ADDED,
        events::ATTACHMENT_REMOVED,
    ] {
        assert!(
            events::ISSUE_EVENT_TOKENS.contains(&tok),
            "token {tok} is in the registered set"
        );
    }
}
