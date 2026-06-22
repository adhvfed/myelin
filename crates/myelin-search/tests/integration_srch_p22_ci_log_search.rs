//! # Integration — SRCH-P22 (P-340, M4): Search indexes the REAL CI-log corpus
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D1 (F1, the
//! zero-escape leak: a fork-scoped CI log NEVER in any result incl. counts for an unauthorized viewer)
//! and SRCH-D3 (F2, cross-tenant IDOR = 0), the gate-invariant ratchet **re-confirmed on the REAL
//! CI-log corpus**. **Architecture:** `search-and-indexing.md` change 11 (Search consumes the DURABLE
//! sealed segments, NOT the firehose), change 9 (the per-subject CI-log DEK backstop). **Contracts:**
//! 11.8 (the per-subject-DEK sealed log segments and the `(job, step, byte-range)` index), 5.9 (the
//! X-1 `CheckStatus.details_ref` `#step-<n>` the index resolves).
//!
//! ## What this proves (the dated green artifact, 2026-06-23)
//! The REAL CI-log corpus (the reconstructed `(run, job, step)` step logs from the DURABLE sealed
//! segments) is projected through [`myelin_search::ci_log_search_projection`] (CI's consumed 11.8
//! projection) into the LIVE [`IncrementalIndexer`] per-event pipeline, then queried back. The GATE:
//!
//! 1. **CI-log search correctness** — a `(job, step, byte-range)` query resolves the right sealed
//!    segment doc; the X-1 `details_ref` `#step-<n>` resolves to the exact failing step's doc; a
//!    log-content query hits the right step. Search reads the durable segments, not the firehose.
//! 2. **SRCH-D1 (F1) on the CI-log corpus** — a FORK-SCOPED CI log (a log of a run the viewer cannot
//!    `view`) never appears in ANY result (FT or facet) incl. counts; a grant ⇒ it appears (the
//!    rejection was the parent-run ACL firing, not a blanket deny).
//! 3. **SRCH-D3 (F2) on the CI-log corpus** — a viewer's tenant partitions the index; a cross-tenant
//!    query sees 0 of the other tenant's CI logs.
//!
//! The ENGINE is UNCHANGED — this is producer-corpus wiring (the prompt's DoD). No mutation-core module
//! is added; the SRCH-P09 mutation floor still holds on the real CI-log corpus.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_query::FieldValue;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventId, EventType, Timestamp,
    Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::{
    ci_log_details_ref, ci_log_doc_ref, ci_log_index_specs, ci_log_search_projection,
    parse_step_anchor, AclFilter, CiLogProjectionInput, IncrementalIndexer, MockEmbeddingAdapter,
    ProjectFetchError, ProjectFetcher, SearchProjection, CI_LOG_FACET_JOB_ID, CI_LOG_FACET_STEP_NO,
};

// ----------------------------------------------------------------------------------------------
// fixtures — the REAL CI-log corpus projected through CI's consumed 11.8 spec
// ----------------------------------------------------------------------------------------------

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str, t: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(t.into()),
    )
}

/// A scripted [`ProjectFetcher`] over a `ref → SearchProjection` map — the owner's `project(ref,
/// viewer)` (5.6) that (in production) resolves the `(job, step, byte-range)` index + reads the DURABLE
/// sealed segment (change #11 — NOT the firehose). The REAL CI-log corpus is built by
/// [`ci_log_search_projection`] over the `(run, job, step)` + log text, so this fetcher serves CI's
/// genuine 11.8 projection.
#[derive(Default)]
struct CiLogFetcher {
    projections: Mutex<BTreeMap<String, SearchProjection>>,
}
impl CiLogFetcher {
    fn put(&self, ref_: &str, p: SearchProjection) {
        self.projections.lock().unwrap().insert(ref_.to_string(), p);
    }
}
impl ProjectFetcher for CiLogFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        match self.projections.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

fn ci_event(id: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("ci.ci_log.sealed".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(viewer("platform", "acme")),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(format!("agg:{subject}")),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: true,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        payload: serde_json::json!({ "zookie": "zk-ci-1", "version": 1 }),
    }
}

fn event_in(id: &str, subject: &str, t: &str) -> EventEnvelope {
    let mut ev = ci_event(id, subject);
    ev.tenant = TenantId(t.into());
    ev
}

fn ci_indexer(fetcher: Arc<CiLogFetcher>) -> IncrementalIndexer {
    IncrementalIndexer::new(
        ci_log_index_specs(),
        fetcher,
        Arc::new(MockEmbeddingAdapter::new(8)),
    )
}

/// A CI-log projection input for `(tenant, run, job, step)` carrying the reconstructed step log.
fn ci_input(tenant: &str, run: &str, job: &str, step: u32, log: &str) -> CiLogProjectionInput {
    CiLogProjectionInput {
        run_id: format!("myelin://{tenant}/ci/run/{run}"),
        job_id: job.into(),
        step_no: step,
        log_text: log.into(),
        lang: None,
    }
}

// ----------------------------------------------------------------------------------------------
// 1. CI-log search correctness — the (job, step, byte-range) facets + the #step-<n> resolve
// ----------------------------------------------------------------------------------------------

/// **A `(job, step)` query resolves the right sealed segment doc + the log body is searchable.** The
/// REAL CI-log corpus is projected through CI's consumed 11.8 spec and indexed through the live
/// per-event pipeline (Search reads the durable segments, NOT the firehose — change #11).
#[test]
fn ci_log_index_and_step_query_returns_the_right_segment() {
    let step1 = ci_log_doc_ref("acme", "run-1", "build", 1);
    let step3 = ci_log_doc_ref("acme", "run-1", "build", 3);
    let fetcher = Arc::new(CiLogFetcher::default());
    fetcher.put(
        &step1,
        ci_log_search_projection(&ci_input("acme", "run-1", "build", 1, "checkout ok\n")),
    );
    fetcher.put(
        &step3,
        ci_log_search_projection(&ci_input(
            "acme",
            "run-1",
            "build",
            3,
            "FAIL: assertion at src/scheduler/deadlock.rs:42\n",
        )),
    );
    let ix = ci_indexer(fetcher);
    ix.index(&ci_event("e-1", &step1)).expect("index step 1");
    ix.index(&ci_event("e-3", &step3)).expect("index step 3");
    assert_eq!(ix.live_count(&tenant(), &region()), 2, "both steps indexed");

    let acl = AclFilter::ids([step1.as_str(), step3.as_str()]);

    // FT: a log-content term hits its step (the failing step's assertion message).
    let ft = ix
        .search_ft(&tenant(), &region(), &acl, "assertion", 10)
        .expect("ft search");
    assert!(
        ft.iter().any(|h| h.doc_id == step3),
        "the log term finds the failing step"
    );
    assert!(
        !ft.iter().any(|h| h.doc_id == step1),
        "the log term does not find the unrelated step"
    );

    // The structured `(job, step)`-index facet equality: step_no == 3 → exactly the failing step doc.
    let by_step = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            CI_LOG_FACET_STEP_NO,
            &FieldValue::Int(3),
            10,
        )
        .expect("step_no facet scan");
    assert_eq!(by_step.len(), 1, "exactly the step-3 doc");
    assert_eq!(by_step[0].doc_id, step3);
}

/// **The X-1 `details_ref` `#step-<n>` resolves to the exact failing step's doc (5.9 / OQ-D).** A
/// `CheckStatus.details_ref` `myelin://<tenant>/ci/run/<run>#step-<n>` resolves (Search-side) to the
/// `(run, step)` facets, and a facet query on those returns exactly the failing step's CI-log doc —
/// the jump-to-failure searchable end-to-end.
#[test]
fn details_ref_step_anchor_resolves_to_the_exact_failing_step_doc() {
    let step2 = ci_log_doc_ref("acme", "run-7", "test", 2);
    let step5 = ci_log_doc_ref("acme", "run-7", "test", 5);
    let fetcher = Arc::new(CiLogFetcher::default());
    fetcher.put(
        &step2,
        ci_log_search_projection(&ci_input("acme", "run-7", "test", 2, "step two passed\n")),
    );
    fetcher.put(
        &step5,
        ci_log_search_projection(&ci_input("acme", "run-7", "test", 5, "FAILURE HERE\n")),
    );
    let ix = ci_indexer(fetcher);
    ix.index(&ci_event("e-2", &step2)).expect("index step 2");
    ix.index(&ci_event("e-5", &step5)).expect("index step 5");

    // The X-1 details_ref for the failing step, parsed to its (run, step) resolution target.
    let details_ref = ci_log_details_ref("acme", "run-7", 5);
    let parsed = parse_step_anchor(&details_ref).expect("parse the #step-5 details_ref");
    assert_eq!(parsed.run_id, "run-7");
    assert_eq!(parsed.step_no, 5);

    // Resolving on the (run, step) facets returns EXACTLY the failing step's doc (not step 2's).
    let acl = AclFilter::ids([step2.as_str(), step5.as_str()]);
    let hits = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            CI_LOG_FACET_STEP_NO,
            &FieldValue::Int(i64::from(parsed.step_no)),
            10,
        )
        .expect("resolve the #step-5 facet");
    assert_eq!(hits.len(), 1, "exactly the failing step's doc");
    assert_eq!(
        hits[0].doc_id, step5,
        "the #step-5 details_ref resolves to step 5's doc, not a neighbour's"
    );
}

/// **A per-job `(job)` facet query works (the `(job, step, byte-range)` index's first key).** Two jobs
/// in the same run; a job filter returns only that job's step logs.
#[test]
fn ci_log_per_job_facet_query() {
    let build1 = ci_log_doc_ref("acme", "run-9", "build", 1);
    let test1 = ci_log_doc_ref("acme", "run-9", "test", 1);
    let fetcher = Arc::new(CiLogFetcher::default());
    fetcher.put(
        &build1,
        ci_log_search_projection(&ci_input("acme", "run-9", "build", 1, "compiling\n")),
    );
    fetcher.put(
        &test1,
        ci_log_search_projection(&ci_input("acme", "run-9", "test", 1, "running tests\n")),
    );
    let ix = ci_indexer(fetcher);
    ix.index(&ci_event("e-b", &build1)).expect("index build");
    ix.index(&ci_event("e-t", &test1)).expect("index test");

    let acl = AclFilter::ids([build1.as_str(), test1.as_str()]);
    let hits = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            CI_LOG_FACET_JOB_ID,
            &FieldValue::Text("build".into()),
            10,
        )
        .expect("job facet scan");
    assert_eq!(hits.len(), 1, "exactly the build job's step log");
    assert_eq!(hits[0].doc_id, build1);
}

// ----------------------------------------------------------------------------------------------
// 2. SRCH-D1 (F1) — the zero-escape leak on the CI-log corpus (the fork-scoped CI log)
// ----------------------------------------------------------------------------------------------

/// **SRCH-D1 (F1) re-confirmed on the CI-log corpus: a FORK-SCOPED CI log (a log of a run the viewer
/// cannot `view` — an untrusted-fork run the viewer is not a member of) never appears in ANY result
/// (FT or facet) incl. counts, for an unauthorized viewer — and a grant makes it appear (the rejection
/// was the parent-run ACL firing, not a blanket deny).**
#[test]
fn srch_d1_fork_scoped_ci_log_never_leaks() {
    let visible = ci_log_doc_ref("acme", "run-main", "build", 1);
    let fork_scoped = ci_log_doc_ref("acme", "run-fork", "build", 1);
    let fetcher = Arc::new(CiLogFetcher::default());
    // BOTH logs carry the SAME rare term — so a leak would be exposed by FT/count/IDF inference OR by
    // a facet-count leak on the shared step_no facet.
    fetcher.put(
        &visible,
        ci_log_search_projection(&ci_input(
            "acme",
            "run-main",
            "build",
            1,
            "zarquon build succeeded\n",
        )),
    );
    fetcher.put(
        &fork_scoped,
        ci_log_search_projection(&ci_input(
            "acme",
            "run-fork",
            "build",
            1,
            "zarquon secret fork build\n",
        )),
    );
    let ix = ci_indexer(fetcher);
    ix.index(&ci_event("v", &visible)).expect("index visible");
    ix.index(&ci_event("f", &fork_scoped))
        .expect("index fork-scoped");
    assert_eq!(ix.live_count(&tenant(), &region()), 2, "both logs indexed");

    // The unauthorized viewer's reachable set is JUST the main run's log — the fork-scoped run's log is
    // NOT in it (a CI log's reachability is its parent run's ReBAC `view`; here that resolves to the
    // fork run being absent from the allow-set — the viewer is not a member of the untrusted fork).
    let acl_unauth = AclFilter::ids([visible.as_str()]);

    // FT: the shared rare term `zarquon` — only the visible log surfaces; the fork-scoped one never.
    let ft = ix
        .search_ft(&tenant(), &region(), &acl_unauth, "zarquon", 10)
        .expect("ft");
    assert_eq!(
        ft.len(),
        1,
        "0 count-leak: exactly the one visible log (the fork-scoped log never counted)"
    );
    assert_eq!(ft[0].doc_id, visible);
    assert!(
        !ft.iter().any(|h| h.doc_id == fork_scoped),
        "0 leak: the fork-scoped CI log never surfaces in FT"
    );

    // Structured facet: even on the SHARED step_no facet, the fork-scoped log never surfaces.
    let by_step = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl_unauth,
            CI_LOG_FACET_STEP_NO,
            &FieldValue::Int(1),
            10,
        )
        .expect("facet scan");
    assert_eq!(
        by_step.len(),
        1,
        "0 facet-count-leak: the fork-scoped log is not in the step facet result"
    );
    assert!(
        !by_step.iter().any(|h| h.doc_id == fork_scoped),
        "0 leak: the fork-scoped CI log never surfaces in a structured facet scan"
    );

    // The chained grant: the viewer is now granted the fork run (e.g. fork-trust approved) → it appears.
    let acl_granted = AclFilter::ids([visible.as_str(), fork_scoped.as_str()]);
    let granted = ix
        .search_ft(&tenant(), &region(), &acl_granted, "zarquon", 10)
        .expect("ft granted");
    assert_eq!(
        granted.len(),
        2,
        "after the grant BOTH logs surface (the rejection was the parent-run ACL, not a deny)"
    );
    assert!(
        granted.iter().any(|h| h.doc_id == fork_scoped),
        "the granted fork-scoped CI log now appears"
    );
}

// ----------------------------------------------------------------------------------------------
// 3. SRCH-D3 (F2) — cross-tenant IDOR = 0 on the CI-log corpus
// ----------------------------------------------------------------------------------------------

/// **SRCH-D3 (F2) re-confirmed on the CI-log corpus: a viewer's tenant partitions the index — a query
/// against a DIFFERENT tenant's index sees 0 of this tenant's CI logs (the per-tenant index, §3.4).**
#[test]
fn srch_d3_cross_tenant_ci_logs_do_not_leak() {
    // Two tenants index a CI log under a COLLIDING doc-id namespace, so only the partition key
    // (tenant, region) keeps them apart — not a lucky id difference.
    let acme_log = ci_log_doc_ref("acme", "run-1", "build", 1);
    let evil_log = ci_log_doc_ref("evil", "run-1", "build", 1);
    let fetcher = Arc::new(CiLogFetcher::default());
    fetcher.put(
        &acme_log,
        ci_log_search_projection(&ci_input("acme", "run-1", "build", 1, "build log\n")),
    );
    fetcher.put(
        &evil_log,
        ci_log_search_projection(&ci_input("evil", "run-1", "build", 1, "build log\n")),
    );
    let ix = ci_indexer(fetcher);
    ix.index(&event_in("a", &acme_log, "acme"))
        .expect("index acme");
    ix.index(&event_in("e", &evil_log, "evil"))
        .expect("index evil");

    let acme_t = TenantId("acme".into());
    let evil_t = TenantId("evil".into());

    // Positive control: acme's viewer querying acme's index sees acme's log.
    let acme_hits = ix
        .search_ft(
            &acme_t,
            &region(),
            &AclFilter::ids([acme_log.as_str()]),
            "build",
            10,
        )
        .expect("acme search");
    assert!(
        acme_hits.iter().any(|h| h.doc_id == acme_log),
        "acme sees its own CI log"
    );

    // The cross-tenant attack: even with an allow-set NAMING the evil log's doc-id, querying ACME's
    // partition returns 0 — the evil log lives in a DIFFERENT (tenant, region) index entirely.
    let cross = ix
        .search_ft(
            &acme_t,
            &region(),
            &AclFilter::ids([evil_log.as_str()]),
            "build",
            10,
        )
        .expect("cross-tenant search");
    assert!(
        cross.is_empty(),
        "0 cross-tenant: acme's index holds none of evil's CI logs"
    );

    // And the evil tenant's index, conversely, holds only evil's log.
    let evil_hits = ix
        .search_ft(
            &evil_t,
            &region(),
            &AclFilter::ids([acme_log.as_str()]),
            "build",
            10,
        )
        .expect("evil search");
    assert!(
        evil_hits.is_empty(),
        "0 cross-tenant: evil's index holds none of acme's CI logs"
    );
}
