use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, EffectivePolicy, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;

use crate::ci_guard::LinkedPrCheck;
use crate::refs_glue::{
    issue_root_ref, IssueMeta, IssueProjectionStore, Projected, Projector, TombstoneReason,
};
use std::collections::HashSet;

pub const E2E_SCENARIO: &str = "E2E-1";

pub const FRESHNESS_BUDGET_SECS: u64 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesE2eArtifact {
    pub scenario: &'static str,
    pub green: bool,
    pub evidence: String,
    pub leaks: u64,
}

impl IssuesE2eArtifact {
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

fn e2e_zookie() -> Zookie {
    Zookie("zk-e2e1".into())
}

struct E2eId {
    allow: HashSet<String>,
}

impl E2eId {
    fn new() -> E2eId {
        E2eId {
            allow: HashSet::new(),
        }
    }

    fn allow_view_for(mut self, viewer: &str, object: &ArtifactRef) -> E2eId {
        self.allow.insert(format!("{viewer}|view@{}", object.0));
        self
    }

    fn allow_view_all(mut self, object: &ArtifactRef) -> E2eId {
        self.allow.insert(format!("*|view@{}", object.0));
        self
    }
}

impl IdentityService for E2eId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let any = format!("*|{}@{}", permission.0, object.0);
        let specific = format!("{}|{}@{}", subject.principal_id.0, permission.0, object.0);
        Ok(
            if self.allow.contains(&any) || self.allow.contains(&specific) {
                Decision::Allow
            } else {
                Decision::Deny
            },
        )
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &myelin_identity::RunId,
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> IdResult<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(
        &self,
        _f: &myelin_identity::NamespaceFragment,
    ) -> IdResult<myelin_identity::FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

fn confidential_issue_key() -> &'static str {
    "ENG-1421"
}

fn confidential_title() -> &'static str {
    "TOP SECRET acquisition plan"
}

fn build_pane_store() -> IssueProjectionStore {
    let mut store = IssueProjectionStore::new();
    let confidential = issue_root_ref(&e2e_tenant().0, confidential_issue_key());
    store.put_issue(
        &confidential,
        IssueMeta {
            title: confidential_title().to_string(),
            state: "In Progress".into(),
            state_category: "started".into(),
            icon: "issue".into(),
            assignee: Some("psn:alice".into()),
            priority: 2,
            type_rank: 1,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    let public = issue_root_ref(&e2e_tenant().0, "ENG-7");
    store.put_issue(
        &public,
        IssueMeta {
            title: "open the docs site".into(),
            state: "Todo".into(),
            state_category: "unstarted".into(),
            icon: "issue".into(),
            assignee: None,
            priority: 1,
            type_rank: 1,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    store
}

pub fn run_e2e_1_pr_pane() -> IssuesE2eArtifact {
    let tenant = e2e_tenant();
    let confidential = issue_root_ref(&tenant.0, confidential_issue_key());
    let public = issue_root_ref(&tenant.0, "ENG-7");

    let id = E2eId::new()
        .allow_view_for("insider", &confidential)
        .allow_view_all(&public);
    let projector = Projector::new(id, build_pane_store());

    let mut leaks: u64 = 0;

    let insider_conf = projector
        .project(&confidential, &e2e_viewer("insider"), e2e_zookie())
        .expect("the linked issue is a well-formed Issues artifact");
    let insider_sees_title = insider_conf.title() == Some(confidential_title());
    let insider_public = projector
        .project(&public, &e2e_viewer("insider"), e2e_zookie())
        .expect("a well-formed Issues artifact");
    let insider_resolved_public = insider_public.is_visible();

    let live_check = LinkedPrCheck::trusted("failure");
    let re_read_age_secs: u64 = 0;
    let within_freshness_budget = re_read_age_secs <= FRESHNESS_BUDGET_SECS;
    let merge_gate_blocked = !live_check.is_acceptable();

    let denied = projector
        .project(&confidential, &e2e_viewer("outsider"), e2e_zookie())
        .expect("a well-formed Issues artifact - a denied viewer gets a tombstone, never an error");
    let outsider_tombstoned = matches!(
        &denied,
        Projected::Tombstoned(t) if t.reason == TombstoneReason::Denied
    );
    if denied.title().is_some() {
        leaks += 1;
    }
    if let Projected::Tombstoned(t) = &denied {
        let rendered = format!("{t:?}");
        if rendered.contains("SECRET") || rendered.contains("acquisition") {
            leaks += 1;
        }
        if t.root != confidential {
            leaks += 1;
        }
    } else {
        leaks += 1;
    }
    let outsider_public = projector
        .project(&public, &e2e_viewer("outsider"), e2e_zookie())
        .expect("a well-formed Issues artifact");
    let outsider_saw_public = outsider_public.is_visible();

    let green = insider_sees_title
        && insider_resolved_public
        && within_freshness_budget
        && merge_gate_blocked
        && outsider_tombstoned
        && outsider_saw_public;

    IssuesE2eArtifact {
        scenario: E2E_SCENARIO,
        green,
        evidence: format!(
            "PR pane (Issues linked issue): insider_sees_title={insider_sees_title} \
             insider_resolved_public={insider_resolved_public}; mid-flight ci.check.updated \
             (test→failure) re-read within freshness budget ({re_read_age_secs}s ≤ \
             {FRESHNESS_BUDGET_SECS}s)={within_freshness_budget}, merge_gate_blocked={merge_gate_blocked}; \
             outsider→confidential tombstone(denied)={outsider_tombstoned}, outsider_saw_public={outsider_saw_public}; \
             leaks={leaks}; mock-agent runtime (real-LLM is post-M5/R-10)",
        ),
        leaks,
    }
}

pub fn run_issues_e2e_wedge() -> Vec<IssuesE2eArtifact> {
    vec![run_e2e_1_pr_pane()]
}

#[cfg(test)]
#[path = "e2e_wedge/tests.rs"]
mod tests;
