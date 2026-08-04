use std::sync::Arc;

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::events::{RELATION_CREATED, RELATION_REMOVED, RELATION_SNAPSHOT};
use myelin_refs::{mint, strip_sub, sub_kind, ArtifactRef, Sub};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

use super::*;
use crate::edge_builder::{edge_id, EdgeProjection, RelClass};
use crate::ladder::resolve_sub_outcome;
use crate::resolve::{bounded_stale, ProjectOutcome, ResolveMode, ResolveService, TombstoneReason};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}
fn viewer(id: &str, t: &TenantId) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, t.clone())
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

fn relation_event(src: &str, tgt: &str, rel: &str, trigger: &str) -> IssueRelationEvent {
    IssueRelationEvent {
        source: IssueEdgeProducer::issue_root("acme", src),
        target: IssueEdgeProducer::issue_root("acme", tgt),
        rel: rel.into(),
        origin_event_id: format!("evt-{src}-{tgt}-{rel}"),
        origin_event_type: trigger.into(),
        origin_actor: "issue-pseudonym".into(),
        zookie: Some("zk-1".into()),
    }
}

#[test]
fn blocks_relation_mirror_projects_both_inverse_paired_lifecycle_edges() {
    let ev = relation_event("ENG-1", "ENG-2", "blocks", RELATION_CREATED);
    let rows = mirror_issue_relation(&tenant(), &ev).expect("recognised trigger + known rel");
    assert_eq!(rows.len(), 2, "blocks + the frozen inverse blocked_by edge");

    let fwd = rows
        .iter()
        .find(|r| r.rel == "blocks")
        .expect("a blocks edge");
    assert_eq!(fwd.source.0, "myelin://acme/issue/issue/ENG-1");
    assert_eq!(fwd.target.0, "myelin://acme/issue/issue/ENG-2");
    assert_eq!(
        fwd.rel_class,
        RelClass::Lifecycle,
        "a mirror edge is ALWAYS lifecycle-class"
    );

    let inv = rows
        .iter()
        .find(|r| r.rel == "blocked_by")
        .expect("the inverse blocked_by edge");
    assert_eq!(inv.source.0, "myelin://acme/issue/issue/ENG-2");
    assert_eq!(inv.target.0, "myelin://acme/issue/issue/ENG-1");
    assert_eq!(inv.rel_class, RelClass::Lifecycle);
}

#[test]
fn parent_relation_mirror_pairs_parent_to_child() {
    let ev = relation_event("PLAT-9", "ENG-1", "parent", RELATION_CREATED);
    let rows = mirror_issue_relation(&tenant(), &ev).expect("parent is a known rel");
    let rels: Vec<&str> = rows.iter().map(|r| r.rel.as_str()).collect();
    assert!(rels.contains(&"parent"), "the forward parent edge");
    assert!(rels.contains(&"child"), "the frozen inverse child edge");
}

#[test]
fn relates_relation_mirror_is_symmetric() {
    let ev = relation_event("ENG-1", "ENG-2", "relates", RELATION_CREATED);
    let rows = mirror_issue_relation(&tenant(), &ev).expect("relates is a known rel");
    assert_eq!(rows.len(), 2, "relates is mirrored from both ends");
    assert!(rows.iter().all(|r| r.rel == "relates"));
    let pairs: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.source.0.as_str(), r.target.0.as_str()))
        .collect();
    assert!(pairs.contains(&(
        "myelin://acme/issue/issue/ENG-1",
        "myelin://acme/issue/issue/ENG-2"
    )));
    assert!(pairs.contains(&(
        "myelin://acme/issue/issue/ENG-2",
        "myelin://acme/issue/issue/ENG-1"
    )));
}

#[test]
fn closes_relation_mirror_is_forward_only() {
    let ev = relation_event("ENG-1", "ENG-2", "closes", RELATION_CREATED);
    let rows = mirror_issue_relation(&tenant(), &ev).expect("closes is a known rel");
    assert_eq!(rows.len(), 1, "closes has no frozen inverse - forward only");
    assert_eq!(rows[0].rel, "closes");
    assert_eq!(rows[0].source.0, "myelin://acme/issue/issue/ENG-1");
}

#[test]
fn relation_mirror_accepts_removed_and_snapshot_triggers() {
    for trigger in [RELATION_REMOVED, RELATION_SNAPSHOT] {
        let ev = relation_event("ENG-1", "ENG-2", "depends_on", trigger);
        let rows = mirror_issue_relation(&tenant(), &ev)
            .unwrap_or_else(|e| panic!("`{trigger}` is a recognised trigger: {e:?}"));
        assert_eq!(rows.len(), 1, "depends_on is forward-only (None inverse)");
    }
}

#[test]
fn relation_mirror_rejects_an_unrecognised_trigger() {
    let ev = relation_event("ENG-1", "ENG-2", "blocks", "issue.issue.created");
    let err = mirror_issue_relation(&tenant(), &ev).expect_err("not a relation trigger");
    assert_eq!(err, MirrorError::UnknownRel("issue.issue.created".into()));
}

#[test]
fn relation_mirror_rejects_an_unknown_rel_token() {
    let ev = relation_event("ENG-1", "ENG-2", "supersedes", RELATION_CREATED);
    let err = mirror_issue_relation(&tenant(), &ev).expect_err("supersedes is not a lifecycle rel");
    assert_eq!(err, MirrorError::UnknownRel("supersedes".into()));
}

#[test]
fn issue_relation_mirror_is_idempotent_on_replay() {
    let proj = EdgeProjection::new();
    let ev = relation_event("ENG-1", "ENG-2", "blocks", RELATION_CREATED);
    let ids1 = project_issue_relation(&proj, &tenant(), &region(), &ev).expect("project");
    let ids2 = project_issue_relation(&proj, &tenant(), &region(), &ev).expect("re-project");
    assert_eq!(ids1, ids2, "the same deterministic edge_id pair on replay");
    let target = IssueEdgeProducer::issue_root("acme", "ENG-2");
    let inbound = proj.inbound_live(&tenant(), &region(), &target);
    let blocks: Vec<_> = inbound.iter().filter(|r| r.rel == "blocks").collect();
    assert_eq!(
        blocks.len(),
        1,
        "idempotent - one blocks edge inbound to the target ENG-2"
    );
}

#[test]
fn issue_relation_reconverges_to_the_typed_table_typed_wins() {
    let proj = EdgeProjection::new();
    let t = tenant();
    let r = region();

    let drift = relation_event("ENG-1", "ENG-2", "blocks", RELATION_CREATED);
    project_issue_relation(&proj, &t, &r, &drift).expect("project drift");

    let truth = relation_event("ENG-1", "ENG-3", "blocks", RELATION_SNAPSHOT);
    let eng1 = IssueEdgeProducer::issue_root("acme", "ENG-1");
    let eng2 = IssueEdgeProducer::issue_root("acme", "ENG-2");
    let (reprojected, tombstoned) = reconverge_issue_relations(
        &proj,
        &t,
        &r,
        std::slice::from_ref(&truth),
        &[eng1.clone(), eng2.clone()],
        "evt-reindex-1",
    )
    .expect("reconverge");
    assert_eq!(
        reprojected, 2,
        "the typed truth's blocks+blocked_by pair re-projected"
    );
    assert!(
        tombstoned >= 1,
        "the drifted ENG-1→ENG-2 relation is tombstoned (typed wins)"
    );

    let stale_inbound = proj.inbound_live(&t, &r, &eng2);
    assert!(
        stale_inbound.iter().all(|r| r.rel != "blocks"),
        "the stale blocks inbound to ENG-2 is gone (typed table won)"
    );
    let eng1_inbound = proj.inbound_live(&t, &r, &eng1);
    let blocked_by_sources: Vec<&str> = eng1_inbound
        .iter()
        .filter(|r| r.rel == "blocked_by")
        .map(|r| r.source.0.as_str())
        .collect();
    assert_eq!(
        blocked_by_sources,
        vec!["myelin://acme/issue/issue/ENG-3"],
        "ENG-1 is now blocked_by ENG-3 only (the typed truth is live, the drift tombstoned)"
    );
}

#[test]
fn reconverge_rejects_a_malformed_snapshot_event() {
    let proj = EdgeProjection::new();
    let bad_rel = relation_event("ENG-1", "ENG-2", "supersedes", RELATION_SNAPSHOT);
    assert_eq!(
        reconverge_issue_relations(&proj, &tenant(), &region(), &[bad_rel], &[], "evt-x"),
        Err(MirrorError::UnknownRel("supersedes".into()))
    );
    let bad_trigger = relation_event("ENG-1", "ENG-2", "blocks", "issue.issue.created");
    assert_eq!(
        reconverge_issue_relations(&proj, &tenant(), &region(), &[bad_trigger], &[], "evt-y"),
        Err(MirrorError::UnknownRel("issue.issue.created".into()))
    );
}

#[test]
fn the_lineage_is_one_traverse() {
    let proj = EdgeProjection::new();
    let t = tenant();
    let r = region();
    let parent_ev = IssueRelationEvent {
        source: IssueEdgeProducer::initiative_root("acme", "PLAT-9"),
        target: IssueEdgeProducer::issue_root("acme", "ENG-1"),
        rel: "parent".into(),
        origin_event_id: "evt-parent".into(),
        origin_event_type: RELATION_CREATED.into(),
        origin_actor: "issue-pseudonym".into(),
        zookie: Some("zk-1".into()),
    };
    project_issue_relation(&proj, &t, &r, &parent_ev).expect("project parent");
    project_issue_relation(
        &proj,
        &t,
        &r,
        &relation_event("ENG-1", "ENG-2", "blocks", RELATION_CREATED),
    )
    .expect("project blocks");

    let plat9 = IssueEdgeProducer::initiative_root("acme", "PLAT-9");
    let eng1 = IssueEdgeProducer::issue_root("acme", "ENG-1");
    let from_plat9 = proj.outbound_live(&t, &r, &plat9);
    assert!(
        from_plat9
            .iter()
            .any(|e| e.rel == "parent" && e.target.0 == "myelin://acme/issue/issue/ENG-1"),
        "PLAT-9 → ENG-1 (parent) is one hop"
    );
    let from_eng1 = proj.outbound_live(&t, &r, &eng1);
    assert!(
        from_eng1
            .iter()
            .any(|e| e.rel == "blocks" && e.target.0 == "myelin://acme/issue/issue/ENG-2"),
        "ENG-1 → ENG-2 (blocks) is the next hop - one traverse, not a five-way fan-out"
    );
}

fn field_ref(key: &str, field_id: &str) -> ArtifactRef {
    let root = IssueEdgeProducer::issue_root("acme", key);
    mint(&root, Sub::Field(field_id.into())).expect("grammatical field-<id> mint")
}

fn row_ref(key: &str, row_id: &str) -> ArtifactRef {
    let root = IssueEdgeProducer::issue_root("acme", key);
    mint(&root, Sub::Row(row_id.into())).expect("grammatical row-<id> mint")
}

#[test]
fn stable_issue_field_resolves_live() {
    let owner = IssueOwner::new();
    let ref_ = field_ref("ENG-1", "status");
    owner.record_anchor(&ref_, IssueAnchorState::Live);
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, None, "a stable field is a clean LIVE"),
        other => panic!("expected LIVE, got {other:?}"),
    }
}

#[test]
fn edited_issue_field_resolves_outdated() {
    let owner = IssueOwner::new();
    let ref_ = field_ref("ENG-1", "priority");
    owner.record_anchor(&ref_, IssueAnchorState::Edited);
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => {
            assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Outdated));
            assert!(p.sub_anchor.is_some(), "the root is carried, never a 404");
        }
        other => panic!("expected OUTDATED Live, got {other:?}"),
    }
}

#[test]
fn moved_issue_field_resolves_moved() {
    let owner = IssueOwner::new();
    let ref_ = field_ref("ENG-1", "estimate");
    owner.record_anchor(&ref_, IssueAnchorState::Moved);
    match resolve_sub_outcome(&owner, &ref_) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, Some(crate::resolve::ProjectionFlag::Moved)),
        other => panic!("expected MOVED Live, got {other:?}"),
    }
}

#[test]
fn deleted_issue_field_tombstones_carrying_the_root() {
    let owner = IssueOwner::new();
    let ref_ = field_ref("ENG-1", "removed-field");
    owner.record_anchor(&ref_, IssueAnchorState::Deleted);

    let svc = issue_resolve_service(&owner);
    let v = viewer("insider", &tenant());
    owner.grant_view(&tenant(), &region(), &v, &strip_sub(&ref_));
    let res = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &strip_sub(&ref_),
        &v,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(res.is_tombstone(), "deleted field → tombstone");
    assert_eq!(res.tombstone_reason(), Some(TombstoneReason::SubGone));
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(t.root, strip_sub(&ref_));
        assert_eq!(t.root.0, "myelin://acme/issue/issue/ENG-1");
    }
}

#[test]
fn issue_row_anchor_degrades_through_the_ladder() {
    let owner = IssueOwner::new();
    let live_row = row_ref("ENG-1", "r1");
    let dead_row = row_ref("ENG-1", "r2");
    owner.record_anchor(&live_row, IssueAnchorState::Live);
    owner.record_anchor(&dead_row, IssueAnchorState::Deleted);
    match resolve_sub_outcome(&owner, &live_row) {
        ProjectOutcome::Live(p) => assert_eq!(p.flag, None),
        other => panic!("live row → LIVE, got {other:?}"),
    }
    assert_eq!(
        resolve_sub_outcome(&owner, &dead_row),
        ProjectOutcome::SubGone
    );
}

#[test]
fn unscripted_issue_field_anchor_is_gone_not_a_leak() {
    let owner = IssueOwner::new();
    let ref_ = field_ref("ENG-1", "never-recorded");
    assert_eq!(resolve_sub_outcome(&owner, &ref_), ProjectOutcome::SubGone);
}

#[test]
fn erased_issue_field_is_an_erased_tombstone() {
    let owner = IssueOwner::new();
    let ref_ = field_ref("ENG-1", "erased-field");
    owner.record_anchor(&ref_, IssueAnchorState::Erased);
    assert_eq!(resolve_sub_outcome(&owner, &ref_), ProjectOutcome::Erased);
}

#[test]
fn a_bare_issue_root_is_live() {
    let owner = IssueOwner::new();
    let root = IssueEdgeProducer::issue_root("acme", "ENG-1");
    assert!(matches!(
        resolve_sub_outcome(&owner, &root),
        ProjectOutcome::Live(_)
    ));
}

#[test]
fn ref_d1_denied_viewer_of_a_confidential_issue_is_tombstoned() {
    let owner = IssueOwner::new();
    let issue = IssueEdgeProducer::issue_root("acme", "SEC-99");
    let outsider = viewer("non-member", &tenant());
    let svc = issue_resolve_service(&owner);
    let res = svc.resolve(
        &tenant(),
        &region(),
        &issue,
        &issue,
        &outsider,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(res.is_tombstone(), "a non-member is tombstoned");
    assert_eq!(res.tombstone_reason(), Some(TombstoneReason::Denied));
    if let crate::resolve::Resolution::Tombstone(t) = res {
        assert_eq!(
            t.root, issue,
            "the tombstone carries only the root, never the title"
        );
    }
    let member = viewer("member", &tenant());
    owner.grant_view(&tenant(), &region(), &member, &issue);
    let res = svc.resolve(
        &tenant(),
        &region(),
        &issue,
        &issue,
        &member,
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(!res.is_tombstone(), "a member resolves LIVE");
}

#[test]
fn ref_d4_issue_corpus_reindex_byte_parity_incl_relation_mirror() {
    let t = tenant();
    let r = region();
    let corpus = [
        relation_event("ENG-1", "ENG-2", "blocks", RELATION_CREATED),
        relation_event("PLAT-9", "ENG-1", "parent", RELATION_CREATED),
        relation_event("ENG-3", "ENG-4", "relates", RELATION_CREATED),
        relation_event("ENG-5", "ENG-6", "closes", RELATION_CREATED),
    ];
    let build = || {
        let edges = EdgeProjection::new();
        for ev in &corpus {
            project_issue_relation(&edges, &t, &r, ev).expect("project issue_relation");
        }
        edges
    };
    let live = build();
    let live_hash = live.parity_hash(&t, &r);
    live.wipe_partition(&t, &r);
    assert_eq!(live.live_count(&t, &r), 0, "partition wiped");
    let cold = build();
    assert_eq!(
        cold.parity_hash(&t, &r),
        live_hash,
        "cold == live (byte-identical Issues-corpus reindex parity, incl. the issue_relation mirror)"
    );
}

fn issue_resolve_service(owner: &IssueOwner) -> ResolveService {
    ResolveService::new(
        authz(),
        Arc::new(crate::resolve::NoOpCacheRead),
        Arc::new(owner.clone()),
        cell(),
    )
}

#[test]
fn issue_field_ref_classifies_through_the_one_grammar() {
    let ref_ = field_ref("ENG-1421", "status");
    assert_eq!(sub_kind(&ref_), Some(Sub::Field("status".into())));
    assert_eq!(strip_sub(&ref_).0, "myelin://acme/issue/issue/ENG-1421");
    assert_eq!(ISSUE_OWNER_TOKEN, "issue");
}

#[test]
fn issue_relation_edge_id_is_tenant_scoped() {
    let a = edge_id(
        &TenantId("tenantA".into()),
        "myelin://tenantA/issue/issue/ENG-1",
        "myelin://tenantA/issue/issue/ENG-2",
        "blocks",
    );
    let b = edge_id(
        &TenantId("tenantB".into()),
        "myelin://tenantB/issue/issue/ENG-1",
        "myelin://tenantB/issue/issue/ENG-2",
        "blocks",
    );
    assert_ne!(
        a, b,
        "edge ids are tenant-scoped (no cross-tenant collision)"
    );
}
