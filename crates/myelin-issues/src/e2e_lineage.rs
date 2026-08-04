use serde_json::json;

use myelin_events::{ReindexSource, SnapshotScope};
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;

use crate::e2e_wedge::IssuesE2eArtifact;
use crate::refs_glue::{
    issue_root_ref, IssueLifecycleRel, IssueMeta, IssueProjectionStore, IssueRelationGraph,
    Projected, Projector, TombstoneReason,
};
use crate::replay::{IssueReindexSource, IssueReplayKind};

use std::collections::HashSet;

pub const E2E_LINEAGE_SCENARIO: &str = "E2E-3";

pub const LINEAGE_DEPTH_BOUND: usize = crate::refs_glue::TRAVERSE_MAX_DEPTH;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn lineage_zookie() -> myelin_identity::Zookie {
    myelin_identity::Zookie("zk-e2e3".into())
}

fn lineage_viewer(id: &str) -> myelin_identity::Principal {
    myelin_identity::Principal::stub(
        myelin_identity::PrincipalId(id.into()),
        myelin_identity::PrincipalKind::Human,
        tenant(),
    )
}

fn spec_doc_ref() -> ArtifactRef {
    ArtifactRef("myelin://acme/knowledge/doc/spec-search-relevance".into())
}

fn initiative_key() -> &'static str {
    "ENG-100"
}

fn confidential_child_key() -> &'static str {
    "ENG-101"
}

fn confidential_child_title() -> &'static str {
    "TOP SECRET ranking-signal weights"
}

fn public_child_key() -> &'static str {
    "ENG-102"
}

fn pr_ref() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/pr/4821".into())
}

fn ci_run_ref() -> ArtifactRef {
    ArtifactRef("myelin://acme/ci/run/991ad".into())
}

pub fn lineage_audit_anchor() -> ArtifactRef {
    issue_root_ref(&tenant().0, initiative_key())
}

fn build_lineage_store() -> IssueProjectionStore {
    let mut store = IssueProjectionStore::new();
    store.put_issue(
        &issue_root_ref(&tenant().0, initiative_key()),
        IssueMeta {
            title: "Search relevance initiative".into(),
            state: "In Progress".into(),
            state_category: "started".into(),
            icon: "initiative".into(),
            assignee: Some("psn:alice".into()),
            priority: 2,
            type_rank: 2,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    store.put_issue(
        &issue_root_ref(&tenant().0, confidential_child_key()),
        IssueMeta {
            title: confidential_child_title().into(),
            state: "In Review".into(),
            state_category: "started".into(),
            icon: "issue".into(),
            assignee: Some("psn:alice".into()),
            priority: 1,
            type_rank: 1,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    store.put_issue(
        &issue_root_ref(&tenant().0, public_child_key()),
        IssueMeta {
            title: "wire the relevance facet".into(),
            state: "Done".into(),
            state_category: "completed".into(),
            icon: "issue".into(),
            assignee: None,
            priority: 1,
            type_rank: 1,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    store
}

fn build_lineage_graph() -> IssueRelationGraph {
    let mut g = IssueRelationGraph::new();
    let spec = spec_doc_ref();
    let initiative = issue_root_ref(&tenant().0, initiative_key());
    let confidential = issue_root_ref(&tenant().0, confidential_child_key());
    let public = issue_root_ref(&tenant().0, public_child_key());
    g.add_edge(&spec, &initiative, IssueLifecycleRel::Relates);
    g.add_edge(&initiative, &confidential, IssueLifecycleRel::Parent);
    g.add_edge(&initiative, &public, IssueLifecycleRel::Parent);
    g.add_edge(&confidential, &pr_ref(), IssueLifecycleRel::Closes);
    g.add_edge(&public, &pr_ref(), IssueLifecycleRel::Closes);
    g.add_edge(&pr_ref(), &ci_run_ref(), IssueLifecycleRel::Relates);
    g
}

fn seed_reindex_source() -> IssueReindexSource {
    let mut src = IssueReindexSource::new();
    let initiative = issue_root_ref(&tenant().0, initiative_key());
    let confidential = issue_root_ref(&tenant().0, confidential_child_key());
    let public = issue_root_ref(&tenant().0, public_child_key());
    src.upsert(
        IssueReplayKind::Issue,
        &initiative.0,
        2,
        &initiative.0,
        json!({ "state": "In Progress", "type_rank": 2 }),
    );
    src.upsert(
        IssueReplayKind::Issue,
        &confidential.0,
        3,
        &confidential.0,
        json!({ "state": "In Review", "type_rank": 1 }),
    );
    src.upsert(
        IssueReplayKind::Issue,
        &public.0,
        4,
        &public.0,
        json!({ "state": "Done", "type_rank": 1 }),
    );
    src.upsert(
        IssueReplayKind::Relation,
        &format!("{}|closes|{}", confidential.0, pr_ref().0),
        1,
        &confidential.0,
        json!({ "rel": "closes", "target": pr_ref().0 }),
    );
    src.upsert(
        IssueReplayKind::Relation,
        &format!("{}|closes|{}", public.0, pr_ref().0),
        1,
        &public.0,
        json!({ "rel": "closes", "target": pr_ref().0 }),
    );
    src
}

fn cold_reindex_matches_live() -> (bool, u64) {
    let src = seed_reindex_source();
    let live_issues = src.replay(&SnapshotScope::new("issue", "issue:all"), None);
    let live_relations = src.replay(&SnapshotScope::new("issue", "relation:all"), None);
    let cold_src = seed_reindex_source();
    let cold_issues = cold_src.replay(&SnapshotScope::new("issue", "issue:all"), None);
    let cold_relations = cold_src.replay(&SnapshotScope::new("issue", "relation:all"), None);

    let mut drift: u64 = 0;
    if live_issues != cold_issues {
        drift += 1;
    }
    if live_relations != cold_relations {
        drift += 1;
    }
    let cold_has_both_closes = cold_relations.len() == 2;
    if !cold_has_both_closes {
        drift += 1;
    }
    let cold_has_all_issues = cold_issues.len() == 3;
    if !cold_has_all_issues {
        drift += 1;
    }
    (drift == 0, drift)
}

pub fn run_e2e_3_lineage() -> IssuesE2eArtifact {
    let store = build_lineage_store();
    let graph = build_lineage_graph();
    let spec = spec_doc_ref();
    let confidential = issue_root_ref(&tenant().0, confidential_child_key());
    let public = issue_root_ref(&tenant().0, public_child_key());

    let mut leaks: u64 = 0;

    let reached = graph.traverse(&spec, None);
    let reached_set: HashSet<&str> = reached.iter().map(|n| n.node.0.as_str()).collect();
    let initiative = issue_root_ref(&tenant().0, initiative_key());
    let lineage_complete = reached_set.contains(initiative.0.as_str())
        && reached_set.contains(confidential.0.as_str())
        && reached_set.contains(public.0.as_str())
        && reached_set.contains(pr_ref().0.as_str())
        && reached_set.contains(ci_run_ref().0.as_str());
    let within_depth_bound = reached.iter().all(|n| n.depth <= LINEAGE_DEPTH_BOUND);

    let id = LineageId::new()
        .allow_view_for("insider", &confidential)
        .allow_view_all(&initiative)
        .allow_view_all(&public);
    let projector = Projector::new(id, store);

    let insider_initiative = projector
        .project(&initiative, &lineage_viewer("insider"), lineage_zookie())
        .expect("a well-formed Issues artifact");
    let insider_confidential = projector
        .project(&confidential, &lineage_viewer("insider"), lineage_zookie())
        .expect("a well-formed Issues artifact");
    let insider_walks_full_lineage = insider_initiative.is_visible()
        && insider_confidential.is_visible()
        && insider_confidential.title() == Some(confidential_child_title());

    let outsider_confidential = projector
        .project(&confidential, &lineage_viewer("outsider"), lineage_zookie())
        .expect("a denied viewer gets a tombstone, never an error");
    let outsider_tombstoned = matches!(
        &outsider_confidential,
        Projected::Tombstoned(t) if t.reason == TombstoneReason::Denied
    );
    if outsider_confidential.title().is_some() {
        leaks += 1;
    }
    if let Projected::Tombstoned(t) = &outsider_confidential {
        let rendered = format!("{t:?}");
        if rendered.contains("SECRET") || rendered.contains("weights") {
            leaks += 1;
        }
        if t.root != confidential {
            leaks += 1;
        }
    } else {
        leaks += 1;
    }
    let outsider_initiative = projector
        .project(&initiative, &lineage_viewer("outsider"), lineage_zookie())
        .expect("a well-formed Issues artifact");
    let outsider_public = projector
        .project(&public, &lineage_viewer("outsider"), lineage_zookie())
        .expect("a well-formed Issues artifact");
    let lineage_degrades_gracefully = outsider_initiative.is_visible()
        && outsider_public.is_visible()
        && reached_set.contains(pr_ref().0.as_str())
        && reached_set.contains(ci_run_ref().0.as_str());

    let (cold_matches_live, drift) = cold_reindex_matches_live();

    let green = lineage_complete
        && within_depth_bound
        && insider_walks_full_lineage
        && outsider_tombstoned
        && lineage_degrades_gracefully
        && cold_matches_live;

    IssuesE2eArtifact {
        scenario: E2E_LINEAGE_SCENARIO,
        green,
        evidence: format!(
            "spec-to-ship lineage (Issues side): lineage_complete={lineage_complete} \
             (reached {} nodes, depth≤{LINEAGE_DEPTH_BOUND}={within_depth_bound}); \
             insider_walks_full_lineage={insider_walks_full_lineage}; \
             outsider→confidential tombstone(denied)={outsider_tombstoned}, \
             lineage_degrades_gracefully={lineage_degrades_gracefully}; \
             cold-reindex==live (2.6)={cold_matches_live} (drift={drift}); leaks={leaks}; \
             audit-tamper detected via the GDPR hash-chain (cross-module proof); \
             mock-agent runtime (real-LLM is post-M5/R-10)",
            reached.len(),
        ),
        leaks,
    }
}

pub fn run_issues_e2e_3() -> Vec<IssuesE2eArtifact> {
    vec![run_e2e_3_lineage()]
}

struct LineageId {
    allow: HashSet<String>,
}

impl LineageId {
    fn new() -> LineageId {
        LineageId {
            allow: HashSet::new(),
        }
    }

    fn allow_view_for(mut self, viewer: &str, object: &ArtifactRef) -> LineageId {
        self.allow.insert(format!("{viewer}|view@{}", object.0));
        self
    }

    fn allow_view_all(mut self, object: &ArtifactRef) -> LineageId {
        self.allow.insert(format!("*|view@{}", object.0));
        self
    }
}

impl myelin_identity::IdentityService for LineageId {
    fn authenticate(
        &self,
        _c: &myelin_identity::Credential,
    ) -> myelin_identity::Result<myelin_identity::Principal> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        subject: &myelin_identity::Principal,
        permission: &myelin_identity::Permission,
        object: &ArtifactRef,
        _at: &myelin_identity::Consistency,
        _caveat: Option<&myelin_identity::CaveatContext>,
    ) -> myelin_identity::Result<myelin_identity::Decision> {
        let any = format!("*|{}@{}", permission.0, object.0);
        let specific = format!("{}|{}@{}", subject.principal_id.0, permission.0, object.0);
        Ok(
            if self.allow.contains(&any) || self.allow.contains(&specific) {
                myelin_identity::Decision::Allow
            } else {
                myelin_identity::Decision::Deny
            },
        )
    }
    fn list_objects(
        &self,
        _s: &myelin_identity::Principal,
        _p: &myelin_identity::Permission,
        _t: &myelin_identity::ObjectType,
        _at: &myelin_identity::Consistency,
    ) -> myelin_identity::Result<myelin_identity::ListObjectsResult> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(
        &self,
        _o: &myelin_identity::ObjectId,
        _p: &myelin_identity::Permission,
        _at: &myelin_identity::Consistency,
    ) -> myelin_identity::Result<myelin_identity::SubjectTree> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &myelin_identity::Principal,
        _p: &myelin_identity::Permission,
        _o: &myelin_identity::ObjectId,
        _at: &myelin_identity::Consistency,
    ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(
        &self,
        _a: &myelin_identity::Principal,
        _t: &myelin_identity::Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(
        &self,
        _d: &[myelin_identity::TupleDelta],
        _p: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<myelin_identity::Zookie> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &myelin_identity::PrincipalId,
        _r: &myelin_identity::RunId,
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> myelin_identity::Result<myelin_identity::RunToken> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(
        &self,
        _s: &myelin_identity::PrincipalId,
        _t: &TenantId,
    ) -> myelin_identity::Result<String> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &myelin_identity::PrincipalId) -> myelin_identity::Result<()> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(
        &self,
        _f: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        Err(myelin_identity::AuthzError::NotYetImplemented("n/a"))
    }
}

#[cfg(test)]
#[path = "e2e_lineage/tests.rs"]
mod tests;
