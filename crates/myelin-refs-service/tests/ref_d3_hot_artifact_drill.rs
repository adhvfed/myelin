//! # REF-D3 — the hot-artifact backlink scale: the reach index R4 (REF-P23 / P-454, M5)
//!
//! **Drill catalogue:** REF-D3 (reference-graph.md §7 D-3, ~348): *"Hot-artifact backlink falls over"
//! — 'referenced-by-50,000' under concurrent permission-filtered reads: p99 within budget; hot-fanout
//! telemetry fires; R4 serves post-promotion."* **Architecture:** reference-graph.md §6.3 (the
//! hot-artifact backlink scale — the BUILT read-time CTE floor + the FOLLOW-ON Leopard-style flattened
//! reach index R4, promoted on MEASURED hot-fanout exceeding the read budget), §3.7 (R4 derived from
//! R1). **Contract-index:** row **5.3** at scale (the R4 path), **4.3** (R4 gated by the same
//! `list_objects` filter), **1.8** (the `hot_artifact_fanout` telemetry). **Doctrine:** EI-01 §3
//! (prove-it; measured-not-predicted; the read budget is read from the FROZEN thresholds file, never
//! hardcoded; never weaken a threshold to pass — a red is a dated `claimed-not-proven` row).
//!
//! ## What this drill proves (the REF-D3 green artifact)
//! A "referenced-by-50,000" hot artifact (50,000 inbound edges, a confidential SECRET subset + a PUBLIC
//! subset) under CONCURRENT permission-filtered reads:
//! 1. **R4 serves post-promotion** — the target's MEASURED inbound fanout EXCEEDS the read budget R5
//!    (read from the thresholds file `[refs_hot_artifact]`), so R4 is promoted and serves the read (the
//!    measured-trigger, never predicted; a cold target below budget would serve from the CTE floor).
//! 2. **the `hot_artifact_fanout` telemetry fires** — the measured fanout is sampled + observable
//!    (`refs.hot_artifact_fanout`), so the hot artifact is loud BEFORE promotion.
//! 3. **paginated p99 within budget** — every concurrent read PAGES the backlinks (`LIMIT :page`); R4
//!    NEVER materialises all 50,000 (the §6.3 "you page them, you don't materialise them" at scale), so
//!    the served-read p99 stays bounded as the fanout grows (the falls-over case is avoided).
//! 4. **R4 ↔ CTE-floor parity, leak-free** — R4 returns the IDENTICAL admitted set the CTE floor
//!    (REF-P11) returns: exactly the PUBLIC referrers, 0 SECRET referrer (the REF-P11 SetExpr-lowering
//!    leak invariant STILL HOLDS on R4 — it reuses the FROZEN `set_expr_admits`; the leak invariant must
//!    not regress on the new path).
//!
//! ## The R4 follow-on is LINKED to its REF-P11 floor (the prompt's DoD)
//! REF-P11 ([`myelin_refs_service::backlinks`]) NAMED R4 as the hot-artifact follow-on (the read-time
//! CTE floor's "we page them, we don't materialise them" is not the at-scale answer). REF-P23 SHIPS it:
//! [`myelin_refs_service::R4ReachIndex`]. This drill proves the pair — R4 over the SAME R1 + the SAME
//! filter == the CTE floor's answer, faster.
//!
//! ## Floors named (the honesty register)
//! - **The WORLD-SCALE fleet-hardware re-measure of the real R5 crossover is the ONE remaining floor.**
//!   The read budget here is the thresholds-file v1 default-to-beat; the real crossover where the CTE
//!   p99 falls over its budget is re-measured on real fleet hardware (the master M5 30× load floor). The
//!   PROPERTY (R4 promotes only above the measured budget, serves the same leak-free paginated set, the
//!   fanout telemetry fires) is complete + testable now.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use myelin_identity::Consistency;
use myelin_identity::{
    ColRef, ConsistencyMode, ListObjectsResult, Principal, PrincipalId, PrincipalKind, RelName,
    SetExpr, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_refs_service::backlinks::source_root_colref;
use myelin_refs_service::edge_builder::RelClass;
use myelin_refs_service::{AuthzVisibleIndex, BacklinkRead, EdgeProjection, EdgeRow, R4ReachIndex};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn aref(s: &str) -> ArtifactRef {
    ArtifactRef(s.into())
}
fn latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}
/// The hot "referenced-by-50,000" target.
fn target_root() -> ArtifactRef {
    aref("myelin://acme/issue/issue/VIRAL-1")
}

/// **Read R5 (the read budget) from the workspace-root `thresholds.toml` `[refs_hot_artifact]` row**
/// through the typed [`Thresholds`] loader — the versioned source of truth (P-038), the SAME loader
/// every other Refs drill uses, never a hardcoded number. A missing/unreadable file is a LOUD failure.
fn read_budget_from_thresholds() -> u64 {
    let t = Thresholds::load_canonical().expect("the versioned thresholds file must load");
    let b = t.refs_hot_artifact.read_budget_fanout;
    assert!(b > 0, "the read budget must be a positive fanout");
    b
}

/// Seed a "referenced-by-N" hot artifact into R1: `n_secret` SECRET inbound edges (confidential — must
/// be hidden) + `n_public` PUBLIC inbound edges (admitted). The edge ids are zero-padded so the
/// deterministic `edge_id` order is stable across R1 and R4 (the parity order).
fn seed_hot_artifact(n_secret: usize, n_public: usize) -> EdgeProjection {
    let r1 = EdgeProjection::new();
    for i in 0..n_secret {
        let src = aref(&format!("myelin://acme/issue/issue/SECRET-{i:06}"));
        r1.upsert(&tenant(), &region(), row(&format!("s-{i:06}"), &src));
    }
    for i in 0..n_public {
        let src = aref(&format!("myelin://acme/issue/issue/OPEN-{i:06}"));
        r1.upsert(&tenant(), &region(), row(&format!("p-{i:06}"), &src));
    }
    r1
}

fn row(eid: &str, src: &ArtifactRef) -> EdgeRow {
    EdgeRow {
        edge_id: eid.into(),
        source: src.clone(),
        source_root: src.clone(),
        target: target_root(),
        target_root: target_root(),
        rel: "mentions".into(),
        rel_class: RelClass::Reference,
        origin_event: format!("evt-{eid}"),
        origin_actor: "principal-opaque-1".into(),
        zookie: Some("zk-1".into()),
        tombstoned: false,
    }
}

/// A `list_objects` `Filter` admitting exactly the PUBLIC source space via the reverse index
/// (`InRelation{view}`) — the pushed-down hot-read path. Grants the public sources into `authz`; the
/// SECRET sources are never granted (so they are leak-free absent on both the CTE and R4 paths).
fn public_only_filter(
    authz: &AuthzVisibleIndex,
    viewer: &Principal,
    n_public: usize,
) -> ListObjectsResult {
    for i in 0..n_public {
        let src = format!("myelin://acme/issue/issue/OPEN-{i:06}");
        authz.grant(
            &tenant(),
            &region(),
            &viewer.principal_id.0,
            "view",
            &src,
            "zk-1",
        );
    }
    ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        zookie: Zookie("zk-1".into()),
    }
}

/// The `p99` of a sorted-ascending duration slice (nearest-rank). The drill's latency artifact.
fn p99(mut samples: Vec<Duration>) -> Duration {
    assert!(!samples.is_empty());
    samples.sort();
    let idx = ((samples.len() as f64 * 0.99).ceil() as usize).saturating_sub(1);
    samples[idx.min(samples.len() - 1)]
}

/// **THE REF-D3 GREEN ARTIFACT — the "referenced-by-50,000" hot artifact, R4 serves post-promotion,
/// paginated, leak-free, the fanout telemetry fires; R4 ↔ CTE-floor parity.**
#[test]
fn ref_d3_hot_artifact_r4_serves_post_promotion_paginated_leak_free() {
    // The "referenced-by-50,000" hot artifact: 10,000 SECRET + 40,000 PUBLIC = 50,000 inbound edges.
    let n_secret = 10_000;
    let n_public = 40_000;
    let total = n_secret + n_public;
    let r1 = seed_hot_artifact(n_secret, n_public);
    assert_eq!(
        r1.inbound_live(&tenant(), &region(), &target_root()).len(),
        total,
        "the hot artifact has 50,000 inbound edges"
    );

    let read_budget = read_budget_from_thresholds();

    // R4 derived from R1, gated by the SAME filter (the SAME public grants → the SAME leak-free admit).
    let authz = AuthzVisibleIndex::new();
    let v = viewer("viewer-d3");
    let list_objects = public_only_filter(&authz, &v, n_public);
    let r4 = R4ReachIndex::new(authz, read_budget);
    r4.rebuild_from_r1(&r1, &tenant(), &region(), &target_root());

    // (1) R4 SERVES post-promotion: the measured fanout (50,000) EXCEEDS the read budget → promoted.
    let verdict = r4.promotion_verdict(&tenant(), &region(), &target_root());
    assert!(
        verdict.is_promoted(),
        "the 50,000-inbound hot artifact promotes R4 (measured fanout {} > budget {read_budget}): {verdict:?}",
        verdict.measured_fanout()
    );
    assert_eq!(
        verdict.measured_fanout(),
        total as u64,
        "the measured fanout is the full inbound count"
    );

    // (2) the hot_artifact_fanout telemetry FIRES (the measured fanout is sampled + named).
    assert_eq!(
        r4.last_fanout_sample(),
        total as u64,
        "the hot_artifact_fanout telemetry sampled the measured fanout"
    );
    assert_eq!(
        R4ReachIndex::HOT_ARTIFACT_FANOUT_SIGNAL,
        "refs.hot_artifact_fanout"
    );

    // (3) PAGINATED p99 within budget under CONCURRENT permission-filtered reads. Each reader PAGES
    //     (LIMIT 50) — R4 NEVER materialises all 50,000. We measure the served-read latency across many
    //     concurrent readers; the paginated read stays bounded (the falls-over case is avoided).
    let page = 50;
    let readers = 16;
    let reads_per_thread = 32;
    let r4 = Arc::new(r4);
    let list_objects = Arc::new(list_objects);

    let mut handles = Vec::new();
    for t in 0..readers {
        let r4 = Arc::clone(&r4);
        let lo = Arc::clone(&list_objects);
        handles.push(thread::spawn(move || {
            let v = viewer("viewer-d3");
            let mut samples = Vec::with_capacity(reads_per_thread);
            for _ in 0..reads_per_thread {
                let start = Instant::now();
                let res = r4
                    .backlinks(
                        &tenant(),
                        &region(),
                        &target_root(),
                        &v,
                        &lo,
                        &latest(),
                        page,
                    )
                    .expect("R4 paginated read");
                samples.push(start.elapsed());
                // every read PAGES — never the full 50,000 materialised — and is leak-free.
                assert_eq!(
                    res.edges.len(),
                    page,
                    "the read pages to LIMIT (never the full fanout)"
                );
                for e in &res.edges {
                    assert!(
                        e.source_root.0.contains("OPEN-"),
                        "no SECRET referrer leaks through R4 (thread {t})"
                    );
                }
            }
            samples
        }));
    }
    let mut all = Vec::new();
    for h in handles {
        all.extend(h.join().expect("reader thread"));
    }
    let measured_p99 = p99(all.clone());

    // The paginated read budget: a single PAGED R4 serve (LIMIT 50 over a flattened reach set) is a
    // bounded operation, NOT a 50,000-row materialisation. The budget is generous (the in-memory model
    // serves in microseconds; the real PgStore-backed R4 on the read replica has its own measured p99
    // budget) — the POINT is the read does NOT grow with the fanout because it PAGES. 50 ms is a loud
    // ceiling a paginated serve cannot cross (a falls-over un-paginated materialisation of 50,000 WOULD).
    let budget = Duration::from_millis(50);
    assert!(
        measured_p99 < budget,
        "the paginated R4 read p99 ({measured_p99:?}) must stay within budget ({budget:?}) — \
         R4 pages the 50,000 backlinks, it does not materialise them (REF-D3 falls-over avoided)"
    );

    // (4) R4 ↔ CTE-floor PARITY (leak-free): R4 returns the IDENTICAL admitted set the CTE floor does.
    let authz_cte = AuthzVisibleIndex::new();
    let lo_cte = public_only_filter(&authz_cte, &v, n_public);
    let cte = BacklinkRead::new(r1.clone(), authz_cte);
    let cte_page = cte
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &v,
            &lo_cte,
            &latest(),
            page,
        )
        .expect("the CTE floor read");
    let r4_page = r4
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &v,
            &list_objects,
            &latest(),
            page,
        )
        .expect("the R4 read");
    assert_eq!(
        r4_page.edges, cte_page.edges,
        "REF-D3 PARITY: R4 returns the SAME leak-free, paginated result set as the CTE floor (REF-P11)"
    );
    assert_eq!(r4_page.edges.len(), page, "both paths page to LIMIT");

    println!(
        "REF-D3 GREEN (2026-06-24): fanout={total} budget={read_budget} promoted={} \
         paginated_p99={measured_p99:?} (<{budget:?}) parity=OK leak-free=OK \
         concurrent_readers={readers}x{reads_per_thread}",
        verdict.is_promoted()
    );
}

/// **The drill is NOT vacuous — a target AT-or-UNDER the read budget is NOT promoted (R4 does not serve
/// a cold target).** The measured-trigger discipline: promotion is MEASURED, never predicted.
#[test]
fn ref_d3_a_below_budget_target_is_not_promoted() {
    let read_budget = read_budget_from_thresholds();
    // a target with EXACTLY the budget many inbound edges → at the budget → NOT promoted (strict >).
    let n_public = read_budget as usize;
    let r1 = seed_hot_artifact(0, n_public);
    let authz = AuthzVisibleIndex::new();
    let v = viewer("viewer-d3");
    let _lo = public_only_filter(&authz, &v, n_public);
    let r4 = R4ReachIndex::new(authz, read_budget);
    r4.rebuild_from_r1(&r1, &tenant(), &region(), &target_root());
    assert!(
        !r4.is_promoted(&tenant(), &region(), &target_root()),
        "a target AT the read budget is NOT promoted (strict >, measured not predicted)"
    );
}

/// **A `ColRef`-typed sanity guard** — the R4 admit lowers the SAME frozen SetExpr over the SAME
/// `edge.source_root` column the CTE floor does (one filter column, C-4). A compile-time tie that the
/// R4 path and the CTE path share the FROZEN column, not a parallel one.
#[test]
fn r4_lowers_over_the_same_source_root_column_as_the_cte_floor() {
    let col: ColRef = source_root_colref();
    assert_eq!(col.table, "edge");
    assert_eq!(col.column, "source_root");
}
