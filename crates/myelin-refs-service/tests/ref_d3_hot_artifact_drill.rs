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
fn target_root() -> ArtifactRef {
    aref("myelin://acme/issue/issue/VIRAL-1")
}

fn read_budget_from_thresholds() -> u64 {
    let t = Thresholds::load_canonical().expect("the versioned thresholds file must load");
    let b = t.refs_hot_artifact.read_budget_fanout;
    assert!(b > 0, "the read budget must be a positive fanout");
    b
}

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

fn p99(mut samples: Vec<Duration>) -> Duration {
    assert!(!samples.is_empty());
    samples.sort();
    let idx = ((samples.len() as f64 * 0.99).ceil() as usize).saturating_sub(1);
    samples[idx.min(samples.len() - 1)]
}

#[test]
fn ref_d3_hot_artifact_r4_serves_post_promotion_paginated_leak_free() {
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

    let authz = AuthzVisibleIndex::new();
    let v = viewer("viewer-d3");
    let list_objects = public_only_filter(&authz, &v, n_public);
    let r4 = R4ReachIndex::new(authz, read_budget);
    r4.rebuild_from_r1(&r1, &tenant(), &region(), &target_root());

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

    assert_eq!(
        r4.last_fanout_sample(),
        total as u64,
        "the hot_artifact_fanout telemetry sampled the measured fanout"
    );
    assert_eq!(
        R4ReachIndex::HOT_ARTIFACT_FANOUT_SIGNAL,
        "refs.hot_artifact_fanout"
    );

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

    let budget = Duration::from_millis(50);
    assert!(
        measured_p99 < budget,
        "the paginated R4 read p99 ({measured_p99:?}) must stay within budget ({budget:?}) - \
         R4 pages the 50,000 backlinks, it does not materialise them (REF-D3 falls-over avoided)"
    );

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

#[test]
fn ref_d3_a_below_budget_target_is_not_promoted() {
    let read_budget = read_budget_from_thresholds();
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

#[test]
fn r4_lowers_over_the_same_source_root_column_as_the_cte_floor() {
    let col: ColRef = source_root_colref();
    assert_eq!(col.table, "edge");
    assert_eq!(col.column, "source_root");
}
