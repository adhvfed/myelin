//! # Integration — SRCH-P20 (P-338, M4): Search indexes the REAL Issues corpus
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D1 (F1 — the
//! zero-escape leak: a confidential issue NEVER in any result incl. counts) + SRCH-D3 (F2 —
//! cross-tenant IDOR = 0), the gate-invariant ratchet **re-confirmed on the REAL Issues corpus** (the
//! M2 drills re-run on each new producer corpus). **Architecture:** `search-and-indexing.md` §3.1 (the
//! FieldType-typed facets; the `order_key` LexoRank columnar fast-field for sort, byte-identical to
//! Issues'/Knowledge's), §4.6.1 (the GIN-scan custom-field path + the measured projection-feeder
//! promotion follow-on). **Contracts:** 6.3 (consume Issues' IndexSpec), 13.3 (the FieldType facets +
//! order_key).
//!
//! ## What this proves (the dated green artifact, 2026-06-23)
//! The REAL Issues corpus is projected through [`myelin_search::issue_search_projection`] (Issues'
//! consumed 6.3 projection) into the LIVE [`IncrementalIndexer`] per-event pipeline (project-fetch →
//! analyze → upsert), then queried back through the engine surface. The GATE:
//!
//! 1. **Issues indexing correctness** — a facet query returns the right issue (the typed columnar
//!    equality); results sort by `order_key` (the LexoRank columnar fast-field, ascending byte order);
//!    a board/custom-field query works via the GIN scan.
//! 2. **SRCH-D1 (F1) on the Issues corpus** — a CONFIDENTIAL issue never appears in ANY result (FT or
//!    structured facet) incl. counts, for an unauthorized viewer; a grant ⇒ it appears (the rejection
//!    was the ACL firing, not a blanket deny).
//! 3. **SRCH-D3 (F2) on the Issues corpus** — a viewer's tenant partitions the index; a cross-tenant
//!    query sees 0 of the other tenant's issues (the per-tenant index, partition-keyed).
//!
//! The ENGINE is UNCHANGED — this is producer-corpus wiring (the prompt's DoD). No mutation-core
//! module is added; the SRCH-P09 mutation floor still holds on the real Issues corpus (the SetExpr ACL
//! conjoin decision logic is the same one those drills mutation-test; here it runs on Issues content).
//!
//! ## Floor named
//! The GIN-indexed JSONB facet scan for the Issues board facets serves correctly here; the **measured
//! projection-feeder promotion** to a generated index (per facet at > 5% of view executions, OQ-C) is
//! the M5 follow-on **SRCH-P27** — promotion changes COST, never correctness. The Issues Tier-3
//! board-escalation valve is the sibling slice **SRCH-P21**.

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

// ----------------------------------------------------------------------------------------------
// fixtures — the REAL Issues corpus projected through Issues' consumed 6.3 spec
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
/// viewer)` (5.6). The REAL Issues corpus is built by [`issue_search_projection`] over the typed
/// issue inputs, so this fetcher serves Issues' genuine 6.3 projection (NOT a DB read).
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

/// Build the indexer over the REAL Issues spec (the `issue` declare_indexable shape).
fn issue_indexer(fetcher: Arc<IssueFetcher>) -> IncrementalIndexer {
    IncrementalIndexer::new(
        issue_index_specs(),
        fetcher,
        Arc::new(MockEmbeddingAdapter::new(8)),
    )
}

/// A typed issue projection input with the given board facets + body. The `rank` is the LexoRank
/// fractional index (stamped under the order_key convention by the builder).
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

// ----------------------------------------------------------------------------------------------
// 1. Issues indexing correctness — typed facets + order_key sort + the GIN scan
// ----------------------------------------------------------------------------------------------

/// **A facet query returns the right issue (the typed columnar equality), and the body is searchable.**
/// The REAL Issues corpus is projected through Issues' consumed 6.3 spec and indexed through the live
/// per-event pipeline.
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

    // FT: the body is searchable (a title/body term hits its issue).
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

    // The structured facet equality: state_category == started → exactly the started issue.
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

/// **Results sort by `order_key` (the LexoRank columnar fast-field), ascending byte order (§3.1).** A
/// structured facet scan over a shared facet returns the matching issues in their board rank order —
/// the columnar-fast-field-for-sort deliverable.
#[test]
fn issues_sort_by_order_key_columnar_fast_field() {
    // Three issues in the SAME state (so the facet scan returns all three), with deliberately
    // OUT-OF-ORDER insert order but DISTINCT LexoRank ranks. The scan must return them in rank order.
    let last = OrderKey::bisect(None, None); // "U"-ish midpoint
    let first = OrderKey::bisect(None, Some(&last)); // prepend before `last`
    let middle = OrderKey::bisect(Some(&first), Some(&last)); // strictly between
    assert!(
        first < middle && middle < last,
        "the three ranks are strictly ordered (LexoRank byte order)"
    );

    let r_first = "myelin://acme/issue/issue/ENG-100";
    let r_middle = "myelin://acme/issue/issue/ENG-101";
    let r_last = "myelin://acme/issue/issue/ENG-102";

    let fetcher = Arc::new(IssueFetcher::default());
    // Insert in a scrambled order (last, first, middle) — the SORT must come from the order_key, not
    // the insert order.
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

/// **A board/custom-field query works via the GIN scan (§4.6.1).** A structured equality on the
/// `priority` facet returns exactly the matching issues. The GIN scan serves correctly; the measured
/// projection-feeder promotion is the M5 follow-on (SRCH-P27).
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
    // The GIN-scan facet query: priority == P1 (1) → exactly the two P1 issues.
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

// ----------------------------------------------------------------------------------------------
// 2. SRCH-D1 (F1) — the zero-escape leak on the REAL Issues corpus (the confidential issue)
// ----------------------------------------------------------------------------------------------

/// **SRCH-D1 (F1) re-confirmed on the Issues corpus: a CONFIDENTIAL issue never appears in ANY result
/// (FT or structured facet) incl. counts, for an unauthorized viewer — and a grant makes it appear
/// (the rejection was the ACL firing, not a blanket deny).**
#[test]
fn srch_d1_confidential_issue_never_leaks() {
    let visible = "myelin://acme/issue/issue/ENG-50";
    let confidential = "myelin://acme/issue/issue/ENG-51";
    let fetcher = Arc::new(IssueFetcher::default());
    // BOTH issues carry the SAME rare term + the SAME facet — so a leak would be exposed by
    // FT/count/IDF inference OR by a facet-count leak.
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

    // The unauthorized viewer's reachable set is JUST the visible issue — the confidential issue is
    // NOT in it (an issue's reachability is its ReBAC `view` MINUS the `- confidential` set-difference;
    // here that resolves to the confidential issue being absent from the allow-set).
    let acl_unauth = AclFilter::ids([visible]);

    // FT: the shared rare term `zarquon` — only the visible issue surfaces; the confidential one never.
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

    // Structured facet: even on the SHARED state_category facet, the confidential issue never surfaces.
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

    // The chained grant: the viewer is now granted the confidential issue → it becomes visible.
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

// ----------------------------------------------------------------------------------------------
// 3. SRCH-D3 (F2) — cross-tenant IDOR = 0 on the REAL Issues corpus
// ----------------------------------------------------------------------------------------------

/// **SRCH-D3 (F2) re-confirmed on the Issues corpus: a viewer's tenant partitions the index — a query
/// against a DIFFERENT tenant's index sees 0 of this tenant's issues (the per-tenant index, §3.4).**
#[test]
fn srch_d3_cross_tenant_issues_do_not_leak() {
    // Two tenants index an issue under a COLLIDING doc-id namespace, so only the partition key
    // (tenant, region) keeps them apart — not a lucky id difference.
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

    // Positive control: acme's viewer querying acme's index sees acme's issue.
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

    // The cross-tenant attack: even with an allow-set NAMING the evil issue's doc-id, querying ACME's
    // partition returns 0 — the evil issue lives in a DIFFERENT (tenant, region) index entirely.
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

    // And the evil tenant's index, conversely, holds only evil's issue.
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

// ----------------------------------------------------------------------------------------------
// 4. The order_key facet uses Search's columnar-sort convention (the documented `rank` mapping)
// ----------------------------------------------------------------------------------------------

/// **The projection stamps the LexoRank rank under the `order_key` convention (NOT the producer's
/// `rank` board name).** This is the documented `rank`→`order_key` reconciliation: the engine sorts on
/// the one dedicated `order_key` columnar fast-field, so the consumed projection emits the rank under
/// that convention. The value/encoding/type are byte-identical LexoRank.
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
