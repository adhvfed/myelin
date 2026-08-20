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

#[test]
fn corpus_spans_five_producers_and_both_mirrors() {
    let scale = 8;
    let corpus = build_full_scale_corpus("acme", scale);
    assert_eq!(corpus.reference_count(), FIVE_PRODUCERS.len() * scale);
    assert_eq!(corpus.page_parent_snapshot.len(), scale);
    assert_eq!(corpus.issue_relation_snapshot.len(), scale);
    assert_eq!(corpus.mirror_event_count(), 2 * scale);

    for producer in FIVE_PRODUCERS {
        assert!(
            corpus
                .reference_edges
                .iter()
                .any(|e| e.aggregate.contains(&format!(":{producer}:"))),
            "producer {producer} must contribute reference edges"
        );
    }
    assert!(corpus
        .issue_relation_snapshot
        .iter()
        .any(|e| e.rel == LifecycleRel::Blocks));
    assert!(corpus
        .issue_relation_snapshot
        .iter()
        .any(|e| e.rel == LifecycleRel::Relates));
}

#[test]
fn corpus_is_deterministic_for_a_given_scale() {
    let a = build_full_scale_corpus("acme", 5);
    let b = build_full_scale_corpus("acme", 5);
    assert_eq!(a.reference_edges, b.reference_edges);
    assert_eq!(a.page_parent_snapshot, b.page_parent_snapshot);
    assert_eq!(a.issue_relation_snapshot, b.issue_relation_snapshot);
}

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
    assert_eq!(report.reference_ingested, corpus.reference_count());
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

#[test]
fn parity_telemetry_uses_the_named_signal_constant() {
    assert_eq!(
        RefsReindexer::REINDEX_PARITY_SIGNAL,
        "refs.reindex_parity",
        "the reindex_parity signal name is the contract-1.8 constant"
    );
}

#[test]
fn dropping_the_issue_relation_mirror_flips_parity_red() {
    let corpus = build_full_scale_corpus("acme", 6);

    let live = RefsEdgeBuilder::new(EdgeProjection::new());
    let mut truth = RefsReindexSource::new();
    for edge in &corpus.reference_edges {
        truth.record(edge.clone());
        live.handle(
            &super::live_reference_event(&tenant(), &region(), edge, &ctx_base()),
            &mut myelin_events::HandlerTx::none(),
        );
    }
    for ev in &corpus.page_parent_snapshot {
        project_typed_event(live.projection(), &tenant(), &region(), ev).unwrap();
    }
    for ev in &corpus.issue_relation_snapshot {
        project_typed_event(live.projection(), &tenant(), &region(), ev).unwrap();
    }
    let live_snapshot = live.projection().clone();

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
