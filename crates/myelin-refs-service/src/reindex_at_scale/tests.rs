//! Unit tests for the full-scale REF-D4 corpus builder + the at-scale reindex-parity drill
//! (REF-P24 / P-455). These run on the default DB-free `cargo test --workspace`. The world-scale
//! REF-D4 drill-harness scenario (the named green artifact) lives in
//! `tests/ref_d4_reindex_parity_at_scale.rs`; here we prove the corpus shape + the at-scale property +
//! the counter-cases (a dropped mirror / a non-vacuous parity gate) over a small scale.

use super::*;
use myelin_events::{Actor, EmitContextBase, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
        caused_by: None,
    }
}

/// The corpus spans ALL FIVE producers + BOTH TE-7 mirrors, deterministically sized by `scale`.
#[test]
fn corpus_spans_five_producers_and_both_mirrors() {
    let scale = 8;
    let corpus = build_full_scale_corpus("acme", scale);
    // Five producers × `scale` reference edges each.
    assert_eq!(corpus.reference_count(), FIVE_PRODUCERS.len() * scale);
    // BOTH mirrors: `scale` page_parent + `scale` issue_relation events.
    assert_eq!(corpus.page_parent_snapshot.len(), scale);
    assert_eq!(corpus.issue_relation_snapshot.len(), scale);
    assert_eq!(corpus.mirror_event_count(), 2 * scale);

    // Every producer namespace is present in the reference log.
    for producer in FIVE_PRODUCERS {
        assert!(
            corpus
                .reference_edges
                .iter()
                .any(|e| e.aggregate.contains(&format!(":{producer}:"))),
            "producer {producer} must contribute reference edges"
        );
    }
    // Both inverse shapes are exercised on the issue_relation mirror (blocks paired + relates symmetric).
    assert!(corpus
        .issue_relation_snapshot
        .iter()
        .any(|e| e.rel == LifecycleRel::Blocks));
    assert!(corpus
        .issue_relation_snapshot
        .iter()
        .any(|e| e.rel == LifecycleRel::Relates));
}

/// The corpus is DETERMINISTIC — the same scale yields a byte-identical corpus (the cold==live floor).
#[test]
fn corpus_is_deterministic_for_a_given_scale() {
    let a = build_full_scale_corpus("acme", 5);
    let b = build_full_scale_corpus("acme", 5);
    assert_eq!(a.reference_edges, b.reference_edges);
    assert_eq!(a.page_parent_snapshot, b.page_parent_snapshot);
    assert_eq!(a.issue_relation_snapshot, b.issue_relation_snapshot);
}

/// **THE at-scale REF-D4 PROPERTY: wipe → reindex → byte-parity across the full five-producer corpus +
/// BOTH TE-7 mirrors.** The rebuilt index byte-matches live, the telemetry fires 1, both mirrors
/// reconverge.
#[test]
fn full_scale_reindex_byte_parity_across_five_producers_and_both_mirrors() {
    let corpus = build_full_scale_corpus("acme", 12);
    let report =
        run_full_scale_reindex_parity(&tenant(), &region(), &corpus, ctx_base()).expect("reindex");

    assert!(
        report.is_ref_d4_full_scale_green(),
        "REF-D4 full-scale must be GREEN: {report:?}"
    );
    assert!(report.parity_matched, "rebuilt index byte-matches live");
    assert_eq!(
        report.reindex_parity_signal, 1,
        "reindex_parity telemetry 1"
    );
    // Every reference edge across all five producers was rebuilt through the live consumer path.
    assert_eq!(report.reference_ingested, corpus.reference_count());
    // BOTH mirrors reconverged (each pair-projected at least its cardinality).
    assert!(
        report.page_parent_reprojected > 0,
        "page_parent reconverged"
    );
    assert!(
        report.issue_relation_reprojected > 0,
        "issue_relation reconverged"
    );
    assert!(report.parity_hash.starts_with("blake3:"), "parity hash");
}

/// The reindex_parity telemetry is asserted against the NAMED signal constant, never a literal
/// (EI-01 §3 observability).
#[test]
fn parity_telemetry_uses_the_named_signal_constant() {
    assert_eq!(
        RefsReindexer::REINDEX_PARITY_SIGNAL,
        "refs.reindex_parity",
        "the reindex_parity signal name is the contract-1.8 constant"
    );
}

/// **MANDATORY counter-case: the parity gate is NOT vacuous — a rebuild that DROPS one TE-7 mirror
/// flips the hash and reads RED.** Proves the at-scale green is earned (the byte-parity genuinely
/// covers both mirrors).
#[test]
fn dropping_the_issue_relation_mirror_flips_parity_red() {
    let corpus = build_full_scale_corpus("acme", 6);

    // The live index has BOTH mirrors.
    let live = RefsEdgeBuilder::new(EdgeProjection::new());
    let mut truth = RefsReindexSource::new();
    for edge in &corpus.reference_edges {
        truth.record(edge.clone());
        live.handle(&super::live_reference_event(
            &tenant(),
            &region(),
            edge,
            &ctx_base(),
        ), &mut myelin_events::HandlerTx::none());
    }
    for ev in &corpus.page_parent_snapshot {
        project_typed_event(live.projection(), &tenant(), &region(), ev).unwrap();
    }
    for ev in &corpus.issue_relation_snapshot {
        project_typed_event(live.projection(), &tenant(), &region(), ev).unwrap();
    }
    let live_snapshot = live.projection().clone();

    // The rebuild reconverges ONLY page_parent (the issue_relation mirror is DROPPED — the failure mode).
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let scope = SnapshotScope::new(REFS_OWNER_TOKEN, "edge:all");
    let mut outbox = OutboxStore::new();
    reindexer
        .reindex(&scope, None, &truth, &mut outbox, ctx_base())
        .expect("reference reindex");
    reindexer
        .reconverge_typed(
            &tenant(),
            &region(),
            &corpus.page_parent_snapshot,
            &corpus.page_parent_roots,
            "reindex-page-parent-only",
        )
        .expect("page_parent reconverge");
    // issue_relation deliberately NOT reconverged.

    let matched = reindexer.verify_parity(&live_snapshot, &tenant(), &region());
    assert!(
        !matched,
        "a rebuild missing the issue_relation mirror MUST flip parity RED (never a silent partial \
         rebuild)"
    );
    assert_eq!(
        reindexer.reindex_parity(),
        0,
        "the reindex_parity telemetry reads 0 on the dropped-mirror drift"
    );
}

/// The world-scale fleet-hardware floor is NAMED (the ONE legitimate remaining floor — EI-01 §3).
#[test]
fn world_scale_fleet_load_floor_is_named() {
    assert!(WORLD_SCALE_FLEET_LOAD_FLOOR.contains("fleet hardware"));
    assert!(WORLD_SCALE_FLEET_LOAD_FLOOR.contains("30x"));
}
