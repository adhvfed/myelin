use myelin_events::{Actor, EmitContextBase, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    build_full_scale_corpus, run_full_scale_reindex_parity, FIVE_PRODUCERS,
    WORLD_SCALE_FLEET_LOAD_FLOOR,
};
use myelin_tenancy::{Region, TenantId};

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
fn ref_d4_full_scale_reindex_parity_across_five_producers_and_both_mirrors() {
    let scale = 250;
    let corpus = build_full_scale_corpus("acme", scale);
    assert_eq!(
        corpus.reference_count(),
        FIVE_PRODUCERS.len() * scale,
        "the corpus spans all five producers"
    );
    assert_eq!(
        corpus.mirror_event_count(),
        2 * scale,
        "the corpus carries BOTH TE-7 mirrors (page_parent + issue_relation)"
    );

    let report = run_full_scale_reindex_parity(&tenant(), &region(), &corpus, ctx_base())
        .expect("the full-scale reindex must complete (no poison / no malformed typed snapshot)");

    assert!(
        report.is_ref_d4_full_scale_green(),
        "REF-D4 at full scale MUST be GREEN: {report:?}"
    );
    assert!(
        report.parity_matched,
        "the rebuilt edge index byte-matches live across the full corpus + both mirrors (§4.7)"
    );
    assert_eq!(
        report.reindex_parity_signal, 1,
        "the reindex_parity telemetry (contract 1.8) fires 1 on the byte-match"
    );
    assert_eq!(
        report.reference_ingested,
        corpus.reference_count(),
        "every reference edge across all five producers was rebuilt through the live consumer path"
    );
    assert!(
        report.page_parent_reprojected > 0 && report.issue_relation_reprojected > 0,
        "BOTH TE-7 mirrors reconverged on the rebuild"
    );

    println!(
        "[P-455 REF-D4 FULL-SCALE GREEN 2026-06-24] {} (scale={scale}, parity_hash={})",
        report.summary(),
        report.parity_hash
    );
}

#[test]
fn parity_hash_distinguishes_different_scales() {
    let a = build_full_scale_corpus("acme", 10);
    let b = build_full_scale_corpus("acme", 20);
    let ra = run_full_scale_reindex_parity(&tenant(), &region(), &a, ctx_base()).expect("a");
    let rb = run_full_scale_reindex_parity(&tenant(), &region(), &b, ctx_base()).expect("b");
    assert!(ra.parity_matched && rb.parity_matched, "both rebuild green");
    assert_ne!(
        ra.parity_hash, rb.parity_hash,
        "a larger corpus yields a different parity hash (the hash is a real content-address)"
    );
}

#[test]
fn world_scale_fleet_floor_is_named_not_claimed() {
    assert!(
        WORLD_SCALE_FLEET_LOAD_FLOOR.contains("fleet hardware")
            && WORLD_SCALE_FLEET_LOAD_FLOOR.contains("30x"),
        "the fleet-hardware floor is named as the ONE legitimate remaining floor"
    );
}
