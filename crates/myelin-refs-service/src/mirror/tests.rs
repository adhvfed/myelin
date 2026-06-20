//! Unit tests for the TE-7 typed-edge mirror discipline (REF-P14 / P-163; contract 5.5). The
//! mutation-tested core: the vocabulary parse (reject unknown), the inverse pairing across the whole
//! lifecycle relation set, the `rel_class='lifecycle'` discipline, the both-directions projection, and
//! the drift-reconvergence (typed wins). The chained epic-tree + CDC tests live in
//! `tests/cdc_5_5_mirror.rs`.

use super::*;
use crate::edge_builder::RelClass;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn typed_event(source: &str, target: &str, rel: LifecycleRel) -> SyntheticTypedEvent {
    SyntheticTypedEvent {
        source: ArtifactRef(source.into()),
        target: ArtifactRef(target.into()),
        rel,
        origin_event: "01J-typed".into(),
        origin_actor: "p-opaque-1".into(),
        zookie: Some("zk-1".into()),
    }
}

// --- The frozen vocabulary (§3.3 / contract 5.5) ---

/// **The frozen lifecycle vocabulary is exactly the §3.3 set, and round-trips through parse.** Every
/// forward rel parses to itself; the inverse direction `child` parses; an unknown token is REJECTED
/// (REF-3 — never guessed).
#[test]
fn vocabulary_is_the_frozen_set_and_rejects_unknown() {
    for &rel in LifecycleRel::FORWARD_VOCABULARY {
        assert_eq!(LifecycleRel::parse(rel.as_str()), Some(rel), "{} round-trips", rel.as_str());
    }
    // the 7 frozen forward rels (the §3.3 set, exactly).
    assert_eq!(LifecycleRel::FORWARD_VOCABULARY.len(), 7, "the §3.3 vocabulary is 7 rels");
    // `child` is the inverse direction of `parent` (projected, parseable).
    assert_eq!(LifecycleRel::parse("child"), Some(LifecycleRel::Child));
    // an unknown lifecycle token is REJECTED (never guessed) — and a reference-class rel is NOT a
    // lifecycle rel (the two classes never alias).
    assert_eq!(LifecycleRel::parse("unblocks"), None, "unknown token rejected");
    assert_eq!(LifecycleRel::parse("mentions"), None, "a reference rel is not a lifecycle rel");
    assert_eq!(LifecycleRel::parse(""), None, "empty token rejected");
}

// --- The inverse pairing (§3.3 frozen: blocks↔blocked_by, parent↔child; relates symmetric) ---

/// **The inverse pairing is correct across the whole lifecycle relation set (§3.3).** The named pairs
/// `blocks↔blocked_by` and `parent↔child` are reciprocal; `relates` is symmetric; `closes`/
/// `depends_on`/`assigns` have NO frozen inverse (the floor). A mutant that mis-pairs is caught.
#[test]
fn inverse_pairing_is_correct_across_the_relation_set() {
    // the frozen reciprocal pairs.
    assert_eq!(LifecycleRel::Blocks.inverse(), Inverse::Paired(LifecycleRel::BlockedBy));
    assert_eq!(LifecycleRel::BlockedBy.inverse(), Inverse::Paired(LifecycleRel::Blocks));
    assert_eq!(LifecycleRel::Parent.inverse(), Inverse::Paired(LifecycleRel::Child));
    assert_eq!(LifecycleRel::Child.inverse(), Inverse::Paired(LifecycleRel::Parent));
    // reciprocity: the inverse of the inverse is the original (no asymmetric drift).
    for &rel in &[LifecycleRel::Blocks, LifecycleRel::BlockedBy, LifecycleRel::Parent, LifecycleRel::Child] {
        if let Inverse::Paired(inv) = rel.inverse() {
            assert_eq!(inv.inverse(), Inverse::Paired(rel), "{} ↔ inverse is reciprocal", rel.as_str());
        } else {
            panic!("{} must have a paired inverse", rel.as_str());
        }
    }
    // symmetric.
    assert_eq!(LifecycleRel::Relates.inverse(), Inverse::Symmetric);
    // the floor: directional rels with no frozen inverse token yet.
    assert_eq!(LifecycleRel::Closes.inverse(), Inverse::None);
    assert_eq!(LifecycleRel::DependsOn.inverse(), Inverse::None);
    assert_eq!(LifecycleRel::Assigns.inverse(), Inverse::None);
}

// --- The rel_class='lifecycle' mirror discipline + both-directions projection ---

/// **A `blocks` event yields BOTH a `blocks` edge AND a `blocked_by` edge with the correct direction,
/// both `lifecycle`-class (the GATE: inverse-pairing correctness).** ENG-1 blocks ENG-2 ⇒
/// blocks(ENG-1→ENG-2) + blocked_by(ENG-2→ENG-1). A mutant that drops the inverse or swaps the class
/// is caught.
#[test]
fn blocks_event_yields_both_blocks_and_blocked_by_with_correct_direction() {
    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
    let rows = mirror_edges(&tenant(), &typed_event(eng1, eng2, LifecycleRel::Blocks));

    assert_eq!(rows.len(), 2, "a blocks event projects BOTH directions");
    let blocks = rows.iter().find(|r| r.rel == "blocks").expect("the forward blocks edge");
    let blocked_by = rows.iter().find(|r| r.rel == "blocked_by").expect("the inverse blocked_by edge");

    // direction: blocks runs ENG-1 → ENG-2; blocked_by runs ENG-2 → ENG-1 (endpoints swapped).
    assert_eq!(blocks.source.0, eng1);
    assert_eq!(blocks.target.0, eng2);
    assert_eq!(blocked_by.source.0, eng2, "inverse edge has the endpoints SWAPPED");
    assert_eq!(blocked_by.target.0, eng1);

    // THE discipline: BOTH are lifecycle-class (never reference).
    assert_eq!(blocks.rel_class, RelClass::Lifecycle);
    assert_eq!(blocked_by.rel_class, RelClass::Lifecycle);

    // the roots are the #sub-stripped parents; these refs carry no #sub so root == ref.
    assert_eq!(blocks.source_root.0, eng1);
    assert_eq!(blocked_by.source_root.0, eng2);

    // the two edges have DISTINCT deterministic ids (different rel/endpoints).
    assert_ne!(blocks.edge_id, blocked_by.edge_id);
}

/// **`parent` mirrors to `parent` + `child` (the §3.3 frozen pair); `relates` mirrors symmetrically;
/// the floor rels mirror forward-only.** Asserts the both-directions discipline across the inverse
/// shapes.
#[test]
fn parent_relates_and_floor_rels_mirror_correctly() {
    let a = "myelin://acme/knowledge/page/A";
    let b = "myelin://acme/knowledge/page/B";

    // parent ⇒ parent + child.
    let rows = mirror_edges(&tenant(), &typed_event(a, b, LifecycleRel::Parent));
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| r.rel == "parent" && r.source.0 == a && r.target.0 == b));
    assert!(rows.iter().any(|r| r.rel == "child" && r.source.0 == b && r.target.0 == a));

    // relates ⇒ relates + relates (same token, endpoints swapped — visible from both ends).
    let rows = mirror_edges(&tenant(), &typed_event(a, b, LifecycleRel::Relates));
    assert_eq!(rows.len(), 2, "symmetric relates is visible from both ends");
    assert!(rows.iter().all(|r| r.rel == "relates"));
    assert!(rows.iter().any(|r| r.source.0 == a && r.target.0 == b));
    assert!(rows.iter().any(|r| r.source.0 == b && r.target.0 == a));

    // the floor rels (closes/depends_on/assigns) mirror FORWARD-only (no invented inverse token).
    for rel in [LifecycleRel::Closes, LifecycleRel::DependsOn, LifecycleRel::Assigns] {
        let rows = mirror_edges(&tenant(), &typed_event(a, b, rel));
        assert_eq!(rows.len(), 1, "{} mirrors forward-only (floor: no inverse token)", rel.as_str());
        assert_eq!(rows[0].rel_class, RelClass::Lifecycle);
    }
}

/// **The full sub-precise URN is retained; the roots strip `#sub`.** A typed lifecycle event over a
/// sub-anchored artifact keeps the full URN in `source`/`target` and rolls up the roots.
#[test]
fn mirror_strips_sub_for_roots_keeps_full_urn() {
    let src = "myelin://acme/knowledge/page/7c2#block-9";
    let tgt = "myelin://acme/knowledge/page/abc#block-3";
    let rows = mirror_edges(&tenant(), &typed_event(src, tgt, LifecycleRel::Parent));
    let parent = rows.iter().find(|r| r.rel == "parent").unwrap();
    assert_eq!(parent.source.0, src, "full #sub URN retained");
    assert_eq!(parent.source_root.0, "myelin://acme/knowledge/page/7c2", "root strips #sub");
    assert_eq!(parent.target_root.0, "myelin://acme/knowledge/page/abc");
}

// --- project_typed_event idempotency ---

/// **Projecting a typed event upserts BOTH inverse edges idempotently.** Replaying the same synthetic
/// typed event leaves exactly TWO live edges (forward + inverse), not four.
#[test]
fn project_typed_event_is_idempotent_on_both_directions() {
    let proj = EdgeProjection::new();
    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
    let ev = typed_event(eng1, eng2, LifecycleRel::Blocks);

    project_typed_event(&proj, &tenant(), &region(), &ev).unwrap();
    // replay → still two live edges (idempotent on the deterministic edge_id).
    project_typed_event(&proj, &tenant(), &region(), &ev).unwrap();
    assert_eq!(proj.live_count(&tenant(), &region()), 2, "forward + inverse, idempotent");

    // the inverse edge is inbound to ENG-1 (the blocked_by edge ENG-2→ENG-1 has target_root ENG-1).
    let inbound = proj.inbound_live(&tenant(), &region(), &ArtifactRef(eng1.into()));
    assert!(
        inbound.iter().any(|r| r.rel == "blocked_by"),
        "the inverse blocked_by edge is inbound to ENG-1 (one-query reverse traversal)"
    );
}

// --- The TE-7 drift reconvergence — typed wins (drill REF-D4 TE-7 half) ---

/// **A synthetic drift reconverges to the typed table (typed wins).** Seed a drifted projection (a
/// stale `blocks` edge the typed table no longer backs); reconverge against the authoritative typed
/// snapshot; assert the drift is tombstoned and the typed truth is live. The GATE: typed wins.
#[test]
fn drift_reconverges_to_the_typed_table_typed_wins() {
    let proj = EdgeProjection::new();
    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
    let eng3 = "myelin://acme/issue/issue/ENG-3";

    // DRIFT: a stale lifecycle edge says ENG-3 blocks ENG-2 (the typed table no longer agrees).
    project_typed_event(&proj, &tenant(), &region(), &typed_event(eng3, eng2, LifecycleRel::Blocks))
        .unwrap();
    // and a reference-class edge inbound to ENG-2 (must NOT be touched by reconvergence).
    proj.upsert(
        &tenant(),
        &region(),
        EdgeRow {
            edge_id: edge_id(&tenant(), "myelin://acme/chat/message/m1", eng2, "mentions"),
            source: ArtifactRef("myelin://acme/chat/message/m1".into()),
            source_root: ArtifactRef("myelin://acme/chat/message/m1".into()),
            target: ArtifactRef(eng2.into()),
            target_root: ArtifactRef(eng2.into()),
            rel: "mentions".into(),
            rel_class: RelClass::Reference,
            origin_event: "01J-ref".into(),
            origin_actor: "p-opaque-2".into(),
            zookie: None,
            tombstoned: false,
        },
    );

    // THE TYPED TRUTH for the scope (target ENG-2): ENG-1 blocks ENG-2 (NOT ENG-3).
    let snapshot = vec![typed_event(eng1, eng2, LifecycleRel::Blocks)];
    let covered = vec![ArtifactRef(eng2.into())];
    let (reprojected, tombstoned) =
        reconverge(&proj, &tenant(), &region(), &snapshot, &covered, "01J-reindex").unwrap();

    assert_eq!(reprojected, 2, "the typed snapshot re-projects both directions");
    assert_eq!(tombstoned, 1, "the drifted ENG-3 blocks ENG-2 edge is tombstoned (typed wins)");

    // the drift is gone; the typed truth is live; the reference edge is UNTOUCHED.
    let inbound = proj.inbound_live(&tenant(), &region(), &ArtifactRef(eng2.into()));
    assert!(
        inbound.iter().any(|r| r.rel == "blocks" && r.source.0 == eng1),
        "the typed truth (ENG-1 blocks ENG-2) is live"
    );
    assert!(
        !inbound.iter().any(|r| r.rel == "blocks" && r.source.0 == eng3),
        "the drifted ENG-3 edge is tombstoned (typed table wins)"
    );
    assert!(
        inbound.iter().any(|r| r.rel == "mentions"),
        "the reference-class edge is Refs-authoritative — NOT touched by reconvergence"
    );
}

/// **Reconvergence is scoped: a lifecycle edge OUTSIDE the covered roots is left alone.** A scoped
/// reindex re-emits a bounded scope; an edge the snapshot did not cover is not drift.
#[test]
fn reconverge_leaves_out_of_scope_edges_alone() {
    let proj = EdgeProjection::new();
    let a = "myelin://acme/issue/issue/A";
    let b = "myelin://acme/issue/issue/B";
    let x = "myelin://acme/issue/issue/X";
    let y = "myelin://acme/issue/issue/Y";

    // an edge inbound to B (in scope) and one inbound to Y (out of scope).
    project_typed_event(&proj, &tenant(), &region(), &typed_event(a, b, LifecycleRel::Blocks)).unwrap();
    project_typed_event(&proj, &tenant(), &region(), &typed_event(x, y, LifecycleRel::Blocks)).unwrap();

    // reconverge a snapshot that re-emits A blocks B, covering ONLY root B (not Y).
    let snapshot = vec![typed_event(a, b, LifecycleRel::Blocks)];
    let covered = vec![ArtifactRef(b.into())];
    let (_, tombstoned) =
        reconverge(&proj, &tenant(), &region(), &snapshot, &covered, "01J-reindex").unwrap();

    assert_eq!(tombstoned, 0, "no drift in the covered scope");
    // the out-of-scope X→Y blocks edge is untouched (its inverse blocked_by is inbound to X).
    let inbound_y = proj.inbound_live(&tenant(), &region(), &ArtifactRef(y.into()));
    assert!(inbound_y.iter().any(|r| r.rel == "blocks"), "out-of-scope edge left alone");
}
