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

fn ci_input(tenant: &str, run: &str, job: &str, step: u32, log: &str) -> CiLogProjectionInput {
    CiLogProjectionInput {
        run_id: format!("myelin://{tenant}/ci/run/{run}"),
        job_id: job.into(),
        step_no: step,
        log_text: log.into(),
        lang: None,
    }
}

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

    let details_ref = ci_log_details_ref("acme", "run-7", 5);
    let parsed = parse_step_anchor(&details_ref).expect("parse the #step-5 details_ref");
    assert_eq!(parsed.run_id, "run-7");
    assert_eq!(parsed.step_no, 5);

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

#[test]
fn srch_d1_fork_scoped_ci_log_never_leaks() {
    let visible = ci_log_doc_ref("acme", "run-main", "build", 1);
    let fork_scoped = ci_log_doc_ref("acme", "run-fork", "build", 1);
    let fetcher = Arc::new(CiLogFetcher::default());
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

    let acl_unauth = AclFilter::ids([visible.as_str()]);

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

#[test]
fn srch_d3_cross_tenant_ci_logs_do_not_leak() {
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
