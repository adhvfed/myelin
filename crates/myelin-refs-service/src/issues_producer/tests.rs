//! Unit + CDC tests for REF-P20 / P-336 — Refs projects the SECOND real TE-7 mirror (Issues
//! `issue_relation`) + resolves Issues' `field-`/`row-` sub-anchors + the `<PROJECTKEY>-<seqno>` key.
//!
//! These RE-CONFIRM the Refs invariants on a production-shaped Issues corpus: REF-D9 (the ladder on
//! REAL Issues field/row sub-anchors — an edited/deleted/moved field resolves OUTDATED/GONE/MOVED with
//! the root ALWAYS carried), the TE-7 SECOND-mirror reconvergence (an out-of-band `issue_relation` edit
//! plus a scoped reindex reconverges to the typed table — typed wins; supports ISS-D6), and the leak
//! invariant (a confidential issue's non-member is tombstoned, never leaked). The engine is UNCHANGED —
//! these prove the Issues WIRING drives the engine correctly, and that the `issue_relation` mirror is
//! the SECOND real caller of the REF-P14 mirror discipline over a real typed table, exercising the
//! WHOLE lifecycle vocabulary (`blocks↔blocked_by`, `parent↔child`, the symmetric `relates`, the
//! `None`-inverse `closes`/`depends_on`/`assigns`).
//!
//! Mutation floors (still hold on the Issues corpus): the REF-P14 mirror inverse-pairing + reconverge
//! typed-wins set arithmetic, and the REF-P15 ladder mutation-core are UNCHANGED — this prompt adds the
//! Issues owner + the `issue.relation.*` event mapping, both of which delegate INTO those
//! mutation-tested cores (no new mutation-core module; the engine is fixed at M2).

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
        status: "OPEN — LEGAL".into(),
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

/// Build a relation event for `(source-key, target-key, rel, trigger)`.
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

// ===========================================================================
// 5.5 — the SECOND real lifecycle mirror: Issues issue_relation (the WHOLE vocabulary)
// ===========================================================================

/// **The `issue_relation` mirror projects BOTH inverse-paired edges for a `blocks` relation
/// (`blocks→blocked_by`), both lifecycle-class (5.5 — the SECOND real mirror).** This is the SECOND
/// time the TE-7 mirror discipline runs over a real typed table; it reuses the ONE REF-P14
/// `mirror_edges`, so the inverse pairing (`blocks↔blocked_by`) is the frozen one — never re-invented.
#[test]
fn blocks_relation_mirror_projects_both_inverse_paired_lifecycle_edges() {
    let ev = relation_event("ENG-1", "ENG-2", "blocks", RELATION_CREATED);
    let rows = mirror_issue_relation(&tenant(), &ev).expect("recognised trigger + known rel");
    assert_eq!(rows.len(), 2, "blocks + the frozen inverse blocked_by edge");

    // The forward edge: ENG-1 → ENG-2, rel = blocks, lifecycle-class.
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

    // The inverse edge: ENG-2 → ENG-1, rel = blocked_by (the frozen §3.3 inverse), lifecycle.
    let inv = rows
        .iter()
        .find(|r| r.rel == "blocked_by")
        .expect("the inverse blocked_by edge");
    assert_eq!(inv.source.0, "myelin://acme/issue/issue/ENG-2");
    assert_eq!(inv.target.0, "myelin://acme/issue/issue/ENG-1");
    assert_eq!(inv.rel_class, RelClass::Lifecycle);
}

/// **The `parent` relation mirrors to the frozen `parent↔child` inverse pairing (5.5).** The SAME
/// pairing KN's `page_parent` exercised — proven here on the Issues hierarchy (`initiative → child`).
#[test]
fn parent_relation_mirror_pairs_parent_to_child() {
    let ev = relation_event("PLAT-9", "ENG-1", "parent", RELATION_CREATED);
    let rows = mirror_issue_relation(&tenant(), &ev).expect("parent is a known rel");
    let rels: Vec<&str> = rows.iter().map(|r| r.rel.as_str()).collect();
    assert!(rels.contains(&"parent"), "the forward parent edge");
    assert!(rels.contains(&"child"), "the frozen inverse child edge");
}

/// **A symmetric `relates` relation mirrors to itself with the endpoints swapped (5.5).** `relates` is
/// its own inverse (§3.3) — both edges carry `rel = relates`, visible from both ends.
#[test]
fn relates_relation_mirror_is_symmetric() {
    let ev = relation_event("ENG-1", "ENG-2", "relates", RELATION_CREATED);
    let rows = mirror_issue_relation(&tenant(), &ev).expect("relates is a known rel");
    assert_eq!(rows.len(), 2, "relates is mirrored from both ends");
    // BOTH edges carry rel = relates (symmetric), endpoints swapped.
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

/// **A directional `closes` relation (no frozen inverse token yet — the FLOOR) mirrors the FORWARD edge
/// only (5.5).** `closes`/`depends_on`/`assigns` are [`Inverse::None`] — the forward edge is mirrored,
/// the inverse token is NOT invented (REF-3 — the owning subsystem's mint). Exactly the M2 discipline,
/// now over a real Issues table.
#[test]
fn closes_relation_mirror_is_forward_only() {
    let ev = relation_event("ENG-1", "ENG-2", "closes", RELATION_CREATED);
    let rows = mirror_issue_relation(&tenant(), &ev).expect("closes is a known rel");
    assert_eq!(rows.len(), 1, "closes has no frozen inverse — forward only");
    assert_eq!(rows[0].rel, "closes");
    assert_eq!(rows[0].source.0, "myelin://acme/issue/issue/ENG-1");
}

/// **A relation off a `RELATION_REMOVED` / `RELATION_SNAPSHOT` trigger is mirrored (5.5).** Created,
/// removed, and the reindex snapshot are all recognised mirror triggers — both axes drive the mirror.
#[test]
fn relation_mirror_accepts_removed_and_snapshot_triggers() {
    for trigger in [RELATION_REMOVED, RELATION_SNAPSHOT] {
        let ev = relation_event("ENG-1", "ENG-2", "depends_on", trigger);
        let rows = mirror_issue_relation(&tenant(), &ev)
            .unwrap_or_else(|e| panic!("`{trigger}` is a recognised trigger: {e:?}"));
        assert_eq!(rows.len(), 1, "depends_on is forward-only (None inverse)");
    }
}

/// **A relation off an UNRECOGNISED trigger is REJECTED, never mirrored on a guess (REF-3).** The
/// mirror never invents a relation off a token outside the frozen Issues lifecycle-trigger set.
#[test]
fn relation_mirror_rejects_an_unrecognised_trigger() {
    let ev = relation_event("ENG-1", "ENG-2", "blocks", "issue.issue.created");
    let err = mirror_issue_relation(&tenant(), &ev).expect_err("not a relation trigger");
    assert_eq!(err, MirrorError::UnknownRel("issue.issue.created".into()));
}

/// **A relation carrying a token OUTSIDE the frozen lifecycle vocabulary is REJECTED (REF-3).** An
/// unknown rel (`supersedes`) has no mirror semantics — rejected LOUDLY, never guessed.
#[test]
fn relation_mirror_rejects_an_unknown_rel_token() {
    let ev = relation_event("ENG-1", "ENG-2", "supersedes", RELATION_CREATED);
    let err = mirror_issue_relation(&tenant(), &ev).expect_err("supersedes is not a lifecycle rel");
    assert_eq!(err, MirrorError::UnknownRel("supersedes".into()));
}

/// **The `issue_relation` mirror is idempotent — a replay is ONE edge pair, not duplicates (5.5).** The
/// deterministic edge_id makes a re-projected event upsert in place.
#[test]
fn issue_relation_mirror_is_idempotent_on_replay() {
    let proj = EdgeProjection::new();
    let ev = relation_event("ENG-1", "ENG-2", "blocks", RELATION_CREATED);
    let ids1 = project_issue_relation(&proj, &tenant(), &region(), &ev).expect("project");
    let ids2 = project_issue_relation(&proj, &tenant(), &region(), &ev).expect("re-project");
    assert_eq!(ids1, ids2, "the same deterministic edge_id pair on replay");
    // The live inbound set for the target (ENG-2) is exactly the one forward `blocks` edge (no
    // duplicate). The inverse `blocked_by` edge has its TARGET at ENG-1 (endpoints swapped).
    let target = IssueEdgeProducer::issue_root("acme", "ENG-2");
    let inbound = proj.inbound_live(&tenant(), &region(), &target);
    let blocks: Vec<_> = inbound.iter().filter(|r| r.rel == "blocks").collect();
    assert_eq!(
        blocks.len(),
        1,
        "idempotent — one blocks edge inbound to the target ENG-2"
    );
}

/// **THE TE-7 SECOND-mirror reconvergence — an out-of-band `issue_relation` edit + a scoped reindex
/// reconverges to the typed table (the typed table WINS; supports ISS-D6).** A stale `blocks` edge is
/// in the projection (drift — the row was edited out of band); the authoritative `issue_relation`
/// snapshot no longer carries it (ENG-1 now `blocks` ENG-3, not ENG-2); a scoped reindex reconverges —
/// the stale edge is tombstoned, the typed truth becomes live.
#[test]
fn issue_relation_reconverges_to_the_typed_table_typed_wins() {
    let proj = EdgeProjection::new();
    let t = tenant();
    let r = region();

    // The DRIFTED state: the projection still has ENG-1 `blocks` ENG-2 (a stale row).
    let drift = relation_event("ENG-1", "ENG-2", "blocks", RELATION_CREATED);
    project_issue_relation(&proj, &t, &r, &drift).expect("project drift");

    // The AUTHORITATIVE typed snapshot (what a scoped reindex re-emits): ENG-1 now `blocks` ENG-3.
    // The drifted pair is the forward `blocks` ENG-1→ENG-2 (inbound to ENG-2) + the inverse
    // `blocked_by` ENG-2→ENG-1 (inbound to ENG-1). To tombstone BOTH drifted legs the covered roots
    // are {ENG-1, ENG-2} — the roots the snapshot's inverse-paired edges land inbound to.
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

    // The live inbound `blocks` of ENG-2 is now EMPTY (the stale forward edge was tombstoned).
    let stale_inbound = proj.inbound_live(&t, &r, &eng2);
    assert!(
        stale_inbound.iter().all(|r| r.rel != "blocks"),
        "the stale blocks inbound to ENG-2 is gone (typed table won)"
    );
    // The live inbound `blocked_by` of ENG-1 is now EXACTLY the new truth (ENG-3→ENG-1), the stale
    // ENG-2→ENG-1 inverse leg tombstoned.
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

/// **reconverge rejects a non-trigger / unknown-rel event in the typed snapshot BEFORE mutating the
/// projection (REF-3).** A malformed snapshot is a LOUD rejection — the projection is not
/// half-reconverged.
#[test]
fn reconverge_rejects_a_malformed_snapshot_event() {
    let proj = EdgeProjection::new();
    // unknown rel
    let bad_rel = relation_event("ENG-1", "ENG-2", "supersedes", RELATION_SNAPSHOT);
    assert_eq!(
        reconverge_issue_relations(&proj, &tenant(), &region(), &[bad_rel], &[], "evt-x"),
        Err(MirrorError::UnknownRel("supersedes".into()))
    );
    // unrecognised trigger
    let bad_trigger = relation_event("ENG-1", "ENG-2", "blocks", "issue.issue.created");
    assert_eq!(
        reconverge_issue_relations(&proj, &tenant(), &region(), &[bad_trigger], &[], "evt-y"),
        Err(MirrorError::UnknownRel("issue.issue.created".into()))
    );
}

/// **The spec-to-ship lineage is ONE Refs traverse, not a five-way fan-out (§4.5).** An
/// `initiative parent ENG-1` + `ENG-1 blocks ENG-2` chain, projected, lets a single inbound/outbound
/// walk follow the lifecycle edges in one query — the whole point of the second mirror.
#[test]
fn the_lineage_is_one_traverse() {
    let proj = EdgeProjection::new();
    let t = tenant();
    let r = region();
    // initiative PLAT-9 is the parent of ENG-1; ENG-1 blocks ENG-2.
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

    // ONE outbound walk from PLAT-9 reaches ENG-1 (parent), and from ENG-1 reaches ENG-2 (blocks) —
    // the lineage is traversable as lifecycle-class edges in a single Refs query (no fan-out).
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
        "ENG-1 → ENG-2 (blocks) is the next hop — one traverse, not a five-way fan-out"
    );
}

// ===========================================================================
// REF-D9 — the ladder on REAL Issues field-/row- sub-anchors (5.6/5.7)
// ===========================================================================

/// Helper: mint an Issues field sub-anchor on an issue root.
fn field_ref(key: &str, field_id: &str) -> ArtifactRef {
    let root = IssueEdgeProducer::issue_root("acme", key);
    mint(&root, Sub::Field(field_id.into())).expect("grammatical field-<id> mint")
}

/// Helper: mint an Issues row sub-anchor on an issue root.
fn row_ref(key: &str, row_id: &str) -> ArtifactRef {
    let root = IssueEdgeProducer::issue_root("acme", key);
    mint(&root, Sub::Row(row_id.into())).expect("grammatical row-<id> mint")
}

/// **A stable Issues field resolves LIVE, no flag (REF-D9 happy path).**
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

/// **An EDITED Issues field resolves OUTDATED, the root carried (REF-D9).** A field id is a STABLE
/// opaque id — an edit changes the value but keeps the id, so the embed resolves OUTDATED, NOT GONE.
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

/// **A MOVED Issues field (re-ordered in the scheme) resolves MOVED — the id is immutable (REF-D9).**
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

/// **A DELETED Issues field/row resolves to a sub-gone tombstone that carries the root (REF-D9 — 0
/// dangling embed, 0 hard 404).** The field/row was deleted → GONE; the chokepoint tombstones it
/// carrying the `#sub`-stripped ISSUE root (the embed shows the parent issue).
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

/// **An Issues row anchor degrades through the SAME ladder (REF-D9).** A deleted row is GONE, a live
/// row LIVE — the `row-<id>` kind shares the one ladder vocabulary.
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

/// **An unscripted Issues field anchor is defensively GONE, never a guessed LIVE (REF-3).** A real
/// owner always has the mint-time state; an anchor it never recorded resolves GONE, never fabricated.
#[test]
fn unscripted_issue_field_anchor_is_gone_not_a_leak() {
    let owner = IssueOwner::new();
    let ref_ = field_ref("ENG-1", "never-recorded");
    assert_eq!(resolve_sub_outcome(&owner, &ref_), ProjectOutcome::SubGone);
}

/// **An ERASED Issues field (crypto-shred of the issue DEK, contract 2.7) is an erased tombstone.**
#[test]
fn erased_issue_field_is_an_erased_tombstone() {
    let owner = IssueOwner::new();
    let ref_ = field_ref("ENG-1", "erased-field");
    owner.record_anchor(&ref_, IssueAnchorState::Erased);
    assert_eq!(resolve_sub_outcome(&owner, &ref_), ProjectOutcome::Erased);
}

/// **The bare Issues root resolves LIVE (the issue itself, no sub-anchor).**
#[test]
fn a_bare_issue_root_is_live() {
    let owner = IssueOwner::new();
    let root = IssueEdgeProducer::issue_root("acme", "ENG-1");
    assert!(matches!(
        resolve_sub_outcome(&owner, &root),
        ProjectOutcome::Live(_)
    ));
}

// ===========================================================================
// REF-D1 — leak invariant re-confirmed on the Issues corpus (confidential issue)
// ===========================================================================

/// **REF-D1 (leak) on the Issues corpus: a DENIED viewer of a confidential issue is tombstoned, never
/// leaked.** A confidential issue's content does NOT leak through an unfurl to a non-member (default-
/// deny). The tombstone is structurally incapable of carrying the issue title (the leak invariant;
/// supports the Issues confidential-exclusion fragment, REF-P322).
#[test]
fn ref_d1_denied_viewer_of_a_confidential_issue_is_tombstoned() {
    let owner = IssueOwner::new();
    let issue = IssueEdgeProducer::issue_root("acme", "SEC-99");
    let outsider = viewer("non-member", &tenant());
    // NO grant_view for the outsider (default-deny) — the confidential-exclusion leak invariant.
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
    // A granted member resolves LIVE.
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

// ===========================================================================
// REF-D4 — reindex-parity (DB-free CDC half) over an Issues corpus incl. the second mirror
// ===========================================================================

/// **REF-D4 (reindex-parity, DB-free CDC half) over an Issues corpus INCL. the `issue_relation`
/// mirror.** An Issues corpus (relation lifecycle pairs across the whole vocabulary) is built LIVE, the
/// partition is WIPED, then rebuilt ONLY by re-driving the SAME `issue_relation` projection (the
/// deterministic edge_id — the production logic, no Issues-DB backdoor) → byte-parity (cold == live).
/// The live-Postgres proof is the integration test.
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

// ----- a small resolve-service harness over the Issues owner -----

/// Build a [`ResolveService`] over the Issues owner (the engine is unchanged — the Issues owner is the
/// only new wiring). [`IssueOwner`] is `Clone` (Arc-shared interior), so a clone the service holds
/// shares the SAME recorded state the test arms.
fn issue_resolve_service(owner: &IssueOwner) -> ResolveService {
    ResolveService::new(
        authz(),
        Arc::new(crate::resolve::NoOpCacheRead),
        Arc::new(owner.clone()),
        cell(),
    )
}

/// Sanity: an Issues field ref classifies to the Field sub-kind through the ONE grammar (5.7); the
/// `<PROJECTKEY>-<seqno>` key is the stored root (C-3).
#[test]
fn issue_field_ref_classifies_through_the_one_grammar() {
    let ref_ = field_ref("ENG-1421", "status");
    assert_eq!(sub_kind(&ref_), Some(Sub::Field("status".into())));
    assert_eq!(strip_sub(&ref_).0, "myelin://acme/issue/issue/ENG-1421");
    assert_eq!(ISSUE_OWNER_TOKEN, "issue");
}

/// **An edge id is tenant-scoped — the SAME (source, target, rel) in two tenants is two distinct edges
/// (no cross-tenant collision).** Confirms the mirror's edge_id derivation carries the tenant.
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
