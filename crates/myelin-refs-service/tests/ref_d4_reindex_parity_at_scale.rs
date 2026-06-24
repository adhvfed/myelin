//! # REF-D4 (full scale) — reindex-parity across the full five-producer corpus + BOTH TE-7 mirrors
//! (REF-P24 / P-455, M5)
//!
//! **Drill catalogue:** REF-D4 (the reindex-parity drill) at its **full-scale form** —
//! reference-graph.md drill ~349 + §7 **D-4 the scale variant**. This is the **world-scale promotion**
//! of the REF-P16 CI-variant reindex-parity drill (`cdc_5_8_reindex.rs`, a 3-edge corpus): the rebuilt
//! edge index byte-matches live across the FULL five-producer corpus (Git, Knowledge, CI, Chat,
//! Issues) **INCLUDING BOTH TE-7 lifecycle mirrors** (Knowledge `page_parent` + Issues
//! `issue_relation`).
//!
//! **Architecture:** reference-graph.md §4.7 (reindex-from-source — ONE code path for steady-state +
//! cold rebuild; the rebuilt index byte-matches live; on a TE-7 drift the typed table wins), §7 D-4
//! (the scale variant). **Contract-index:** row **5.8** (`reindex(scope)` at scale, never reads owner
//! DBs), row **1.8** (`reindex_parity` telemetry). **Doctrine:** EI-01 §3 (prove it at scale — the
//! byte-parity is DRILLED green across all five producers + both mirrors, not asserted in prose; name
//! the floor; never claim a green you did not earn).
//!
//! ## What this drill proves (the full-scale REF-D4 green)
//! Wipe the edge index, `reindex(scope)` ONLY from the reindex-from-source `*.snapshot` replay through
//! the SAME live consumer `handle` (no owner-DB backdoor), reconverge BOTH TE-7 mirrors to their typed
//! snapshots (typed wins) — and the rebuilt partition's **parity hash byte-matches the live
//! partition** across the WHOLE corpus. The `reindex_parity` telemetry (contract 1.8) fires `1`.
//!
//! ## The CI→full-scale promotion (the floor this prompt resolves)
//! This drill PROMOTES the REF-P16 CI-variant REF-D4 (`cdc_5_8_reindex.rs`, byte-parity over a 3-edge
//! corpus + a single synthetic TE-7 drift) to its at-scale form (the full five-producer corpus + BOTH
//! real mirrors at once). The CI floor named in `reindex.rs` ("the full-scale REF-D4 is R-M5 / REF-P24")
//! is RESOLVED here; the mutation-core (the reindex decision logic) is UNCHANGED and still holds — this
//! drill scales the corpus the frozen engine runs over (EI-01 §7, no parallel second reindexer).
//!
//! ## Floor named (the ONE legitimate remaining floor)
//! The **30× world-scale FLEET-hardware load** over the PgStore-backed edge index
//! ([`myelin_refs_service::WORLD_SCALE_FLEET_LOAD_FLOOR`]) is the ONE legitimate remaining floor. This
//! drill proves the byte-parity PROPERTY + both-mirror reconvergence over a deterministic scaled corpus
//! — the parity hash is a content-address, so identical bytes ⇒ identical hash at ANY scale; the
//! property does not change shape when real fleet hardware carries the full cardinality.
//!
//! Permanent-gate posture: re-run on every reindex/mirror-touching change; contributes to the master
//! M5→M6 boundary (REF-D4 at scale green across both TE-7 mirrors).

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

/// **THE full-scale REF-D4 PROOF (the dated green artifact the DoD names).** Wipe the edge index,
/// reindex from source, reconverge both TE-7 mirrors — the rebuilt index byte-matches live across the
/// full five-producer corpus + both mirrors. A scale large enough to span every producer namespace +
/// both mirror vocabularies (the fleet-hardware cardinality is the named floor).
#[test]
fn ref_d4_full_scale_reindex_parity_across_five_producers_and_both_mirrors() {
    // A scale that gives a substantial corpus across all five producers + both mirrors (the property is
    // scale-invariant — the parity hash is a content-address; the fleet cardinality is the floor).
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

/// **MANDATORY: the at-scale green is EARNED — the parity hash is sensitive to the corpus** (a
/// different corpus is a different hash). Proves the byte-parity is a real content-address, not a
/// vacuous constant.
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

/// The ONE legitimate remaining floor (the 30× world-scale fleet-hardware load) is NAMED — never
/// claimed as already-proven (EI-01 §3).
#[test]
fn world_scale_fleet_floor_is_named_not_claimed() {
    assert!(
        WORLD_SCALE_FLEET_LOAD_FLOOR.contains("fleet hardware")
            && WORLD_SCALE_FLEET_LOAD_FLOOR.contains("30x"),
        "the fleet-hardware floor is named as the ONE legitimate remaining floor"
    );
}
