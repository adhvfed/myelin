use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_query::{FieldValue, OrderKey};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventId, EventType, Timestamp,
    Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::{
    issue_index_specs, issue_search_projection, AclFilter, IncrementalIndexer,
    IssueProjectionInput, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
    SearchProjection, ISSUE_FACET_PRIORITY, ISSUE_FACET_STATE_CATEGORY, ORDER_KEY_FIELD,
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
struct IssueFetcher {
    projections: Mutex<BTreeMap<String, SearchProjection>>,
}
impl IssueFetcher {
    fn put(&self, ref_: &str, p: SearchProjection) {
        self.projections.lock().unwrap().insert(ref_.to_string(), p);
    }
}
impl ProjectFetcher for IssueFetcher {
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

fn issue_event(id: &str, type_: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_.into()),
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
        payload: serde_json::json!({ "zookie": "zk-iss-1", "version": 1 }),
    }
}

fn event_in(id: &str, type_: &str, subject: &str, t: &str) -> EventEnvelope {
    let mut ev = issue_event(id, type_, subject);
    ev.tenant = TenantId(t.into());
    ev
}

fn issue_indexer(fetcher: Arc<IssueFetcher>) -> IncrementalIndexer {
    IncrementalIndexer::new(
        issue_index_specs(),
        fetcher,
        Arc::new(MockEmbeddingAdapter::new(8)),
    )
}

#[allow(clippy::too_many_arguments)]
fn issue_input(
    body: &str,
    state: &str,
    priority: i64,
    assignee: &str,
    type_rank: i64,
    project: &str,
    rank: OrderKey,
) -> IssueProjectionInput {
    IssueProjectionInput {
        body: body.into(),
        state_category: Some(state.into()),
        priority: Some(priority),
        assignee: Some(assignee.into()),
        type_rank: Some(type_rank),
        project_id: Some(project.into()),
        cycle_id: None,
        rank: Some(rank),
        lang: Some("en".into()),
    }
}

#[test]
fn issues_index_and_facet_query_returns_the_right_issue() {
    let started = "myelin://acme/issue/issue/ENG-1";
    let backlog = "myelin://acme/issue/issue/ENG-2";
    let fetcher = Arc::new(IssueFetcher::default());
    fetcher.put(
        started,
        issue_search_projection(&issue_input(
            "scheduler deadlock at runtime",
            "started",
            2,
            "psn:alice",
            1,
            "myelin://acme/issue/project/ENG",
            OrderKey::bisect(None, None),
        )),
    );
    fetcher.put(
        backlog,
        issue_search_projection(&issue_input(
            "write the onboarding docs",
            "backlog",
            3,
            "psn:bob",
            1,
            "myelin://acme/issue/project/ENG",
            OrderKey::bisect(None, None),
        )),
    );
    let ix = issue_indexer(fetcher);

    ix.index(&issue_event("e-1", "issue.issue.created", started))
        .expect("index started issue");
    ix.index(&issue_event("e-2", "issue.issue.created", backlog))
        .expect("index backlog issue");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        2,
        "both issues are live"
    );

    let acl = AclFilter::ids([started, backlog]);

    let ft = ix
        .search_ft(&tenant(), &region(), &acl, "deadlock", 10)
        .expect("ft search");
    assert!(
        ft.iter().any(|h| h.doc_id == started),
        "the body term finds the started issue"
    );
    assert!(
        !ft.iter().any(|h| h.doc_id == backlog),
        "the body term does not find the unrelated backlog issue"
    );

    let by_state = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            ISSUE_FACET_STATE_CATEGORY,
            &FieldValue::Select("started".into()),
            10,
        )
        .expect("state_category facet scan");
    assert_eq!(by_state.len(), 1, "exactly the started issue");
    assert_eq!(by_state[0].doc_id, started);
}

#[test]
fn issues_sort_by_order_key_columnar_fast_field() {
    let last = OrderKey::bisect(None, None);
    let first = OrderKey::bisect(None, Some(&last));
    let middle = OrderKey::bisect(Some(&first), Some(&last));
    assert!(
        first < middle && middle < last,
        "the three ranks are strictly ordered (LexoRank byte order)"
    );

    let r_first = "myelin://acme/issue/issue/ENG-100";
    let r_middle = "myelin://acme/issue/issue/ENG-101";
    let r_last = "myelin://acme/issue/issue/ENG-102";

    let fetcher = Arc::new(IssueFetcher::default());
    fetcher.put(
        r_last,
        issue_search_projection(&issue_input(
            "task three",
            "started",
            2,
            "psn:alice",
            1,
            "myelin://acme/issue/project/ENG",
            last.clone(),
        )),
    );
    fetcher.put(
        r_first,
        issue_search_projection(&issue_input(
            "task one",
            "started",
            2,
            "psn:alice",
            1,
            "myelin://acme/issue/project/ENG",
            first.clone(),
        )),
    );
    fetcher.put(
        r_middle,
        issue_search_projection(&issue_input(
            "task two",
            "started",
            2,
            "psn:alice",
            1,
            "myelin://acme/issue/project/ENG",
            middle.clone(),
        )),
    );
    let ix = issue_indexer(fetcher);
    ix.index(&issue_event("e-last", "issue.issue.created", r_last))
        .expect("index last");
    ix.index(&issue_event("e-first", "issue.issue.created", r_first))
        .expect("index first");
    ix.index(&issue_event("e-mid", "issue.issue.created", r_middle))
        .expect("index middle");

    let acl = AclFilter::ids([r_first, r_middle, r_last]);
    let hits = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            ISSUE_FACET_STATE_CATEGORY,
            &FieldValue::Select("started".into()),
            10,
        )
        .expect("facet scan");
    let order: Vec<&str> = hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(
        order,
        vec![r_first, r_middle, r_last],
        "the board scan returns issues in order_key (LexoRank) ascending order, not insert order"
    );
}

#[test]
fn issues_board_facet_query_via_gin_scan() {
    let p1 = "myelin://acme/issue/issue/ENG-10";
    let p2 = "myelin://acme/issue/issue/ENG-11";
    let p3 = "myelin://acme/issue/issue/ENG-12";
    let fetcher = Arc::new(IssueFetcher::default());
    fetcher.put(
        p1,
        issue_search_projection(&issue_input(
            "urgent regression",
            "started",
            1,
            "psn:alice",
            0,
            "myelin://acme/issue/project/ENG",
            OrderKey::bisect(None, None),
        )),
    );
    fetcher.put(
        p2,
        issue_search_projection(&issue_input(
            "another urgent one",
            "started",
            1,
            "psn:bob",
            0,
            "myelin://acme/issue/project/ENG",
            OrderKey::bisect(None, None),
        )),
    );
    fetcher.put(
        p3,
        issue_search_projection(&issue_input(
            "low priority cleanup",
            "backlog",
            3,
            "psn:carol",
            0,
            "myelin://acme/issue/project/ENG",
            OrderKey::bisect(None, None),
        )),
    );
    let ix = issue_indexer(fetcher);
    ix.index(&issue_event("e-1", "issue.issue.created", p1))
        .expect("index p1");
    ix.index(&issue_event("e-2", "issue.issue.created", p2))
        .expect("index p2");
    ix.index(&issue_event("e-3", "issue.issue.created", p3))
        .expect("index p3");

    let acl = AclFilter::ids([p1, p2, p3]);
    let hits = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            ISSUE_FACET_PRIORITY,
            &FieldValue::Int(1),
            10,
        )
        .expect("priority GIN scan");
    assert_eq!(
        hits.len(),
        2,
        "exactly the two P1 issues (the GIN-scan facet)"
    );
    let ids: Vec<&str> = hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(ids.contains(&p1) && ids.contains(&p2));
    assert!(
        !ids.contains(&p3),
        "the P3 issue is not in the P1 facet result"
    );
}

#[test]
fn srch_d1_confidential_issue_never_leaks() {
    let visible = "myelin://acme/issue/issue/ENG-50";
    let confidential = "myelin://acme/issue/issue/ENG-51";
    let fetcher = Arc::new(IssueFetcher::default());
    fetcher.put(
        visible,
        issue_search_projection(&issue_input(
            "public zarquon rollout plan",
            "started",
            2,
            "psn:alice",
            1,
            "myelin://acme/issue/project/ENG",
            OrderKey::bisect(None, None),
        )),
    );
    fetcher.put(
        confidential,
        issue_search_projection(&issue_input(
            "classified zarquon acquisition plan",
            "started",
            2,
            "psn:alice",
            1,
            "myelin://acme/issue/project/ENG",
            OrderKey::bisect(None, None),
        )),
    );
    let ix = issue_indexer(fetcher);
    ix.index(&issue_event("v", "issue.issue.created", visible))
        .expect("index visible");
    ix.index(&issue_event("c", "issue.issue.created", confidential))
        .expect("index confidential");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        2,
        "both issues are indexed"
    );

    let acl_unauth = AclFilter::ids([visible]);

    let ft = ix
        .search_ft(&tenant(), &region(), &acl_unauth, "zarquon", 10)
        .expect("ft");
    assert_eq!(
        ft.len(),
        1,
        "0 count-leak: exactly the one visible issue (the confidential issue never counted)"
    );
    assert_eq!(ft[0].doc_id, visible);
    assert!(
        !ft.iter().any(|h| h.doc_id == confidential),
        "0 leak: the confidential issue never surfaces in FT"
    );

    let by_state = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl_unauth,
            ISSUE_FACET_STATE_CATEGORY,
            &FieldValue::Select("started".into()),
            10,
        )
        .expect("facet scan");
    assert_eq!(
        by_state.len(),
        1,
        "0 facet-count-leak: the confidential issue is not in the board facet result"
    );
    assert!(
        !by_state.iter().any(|h| h.doc_id == confidential),
        "0 leak: the confidential issue never surfaces in a structured facet scan"
    );

    let acl_granted = AclFilter::ids([visible, confidential]);
    let granted = ix
        .search_ft(&tenant(), &region(), &acl_granted, "zarquon", 10)
        .expect("ft granted");
    assert_eq!(
        granted.len(),
        2,
        "after the grant BOTH issues surface (the rejection was the ACL, not a deny)"
    );
    assert!(
        granted.iter().any(|h| h.doc_id == confidential),
        "the granted confidential issue now appears"
    );
}

#[test]
fn srch_d3_cross_tenant_issues_do_not_leak() {
    let acme_issue = "myelin://acme/issue/issue/ENG-1";
    let evil_issue = "myelin://evil/issue/issue/ENG-1";
    let fetcher = Arc::new(IssueFetcher::default());
    fetcher.put(
        acme_issue,
        issue_search_projection(&issue_input(
            "scheduler work",
            "started",
            2,
            "psn:alice",
            1,
            "myelin://acme/issue/project/ENG",
            OrderKey::bisect(None, None),
        )),
    );
    fetcher.put(
        evil_issue,
        issue_search_projection(&issue_input(
            "scheduler work",
            "started",
            2,
            "psn:mallory",
            1,
            "myelin://evil/issue/project/ENG",
            OrderKey::bisect(None, None),
        )),
    );
    let ix = issue_indexer(fetcher);
    ix.index(&event_in("a", "issue.issue.created", acme_issue, "acme"))
        .expect("index acme");
    ix.index(&event_in("e", "issue.issue.created", evil_issue, "evil"))
        .expect("index evil");

    let acme_t = TenantId("acme".into());
    let evil_t = TenantId("evil".into());

    let acme_hits = ix
        .search_ft(
            &acme_t,
            &region(),
            &AclFilter::ids([acme_issue]),
            "scheduler",
            10,
        )
        .expect("acme search");
    assert!(
        acme_hits.iter().any(|h| h.doc_id == acme_issue),
        "acme sees its own issue"
    );

    let cross = ix
        .search_ft(
            &acme_t,
            &region(),
            &AclFilter::ids([evil_issue]),
            "scheduler",
            10,
        )
        .expect("cross-tenant search");
    assert!(
        cross.is_empty(),
        "0 cross-tenant: acme's index holds none of evil's issues"
    );

    let evil_hits = ix
        .search_ft(
            &evil_t,
            &region(),
            &AclFilter::ids([acme_issue]),
            "scheduler",
            10,
        )
        .expect("evil search");
    assert!(
        evil_hits.is_empty(),
        "0 cross-tenant: evil's index holds none of acme's issues"
    );
}

#[test]
fn issue_projection_emits_rank_under_order_key_convention() {
    let rank = OrderKey::bisect(None, None);
    let p = issue_search_projection(&issue_input(
        "anything",
        "started",
        2,
        "psn:alice",
        1,
        "myelin://acme/issue/project/ENG",
        rank.clone(),
    ));
    assert_eq!(
        p.fields.get(ORDER_KEY_FIELD),
        Some(&FieldValue::OrderKey(rank)),
        "the rank is stamped under the order_key columnar-sort convention"
    );
    assert!(
        !p.fields.contains_key("rank"),
        "the projection never stamps a `rank`-named facet the engine would not sort on"
    );
}
