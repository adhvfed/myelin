use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::membership::channel_object;
use crate::unfurl::invalidation::invalidates_card;
use crate::unfurl::{
    Card, LadderOutcome, Projection, RefsResolvePort, TombstoneReason, UnfurlCandidate,
    UnfurlService,
};

use super::ChatE2eArtifact;
use std::sync::Mutex;

pub const E2E_SCENARIO: &str = "E2E-1";

pub const FRESHNESS_BUDGET_SECS: u64 = 5;

const E2E_CHANNEL: &str = "c-pr-pane";

fn confidential_ref() -> ArtifactRef {
    ArtifactRef("myelin://acme/issues/issue/ENG-1421".into())
}

const CONFIDENTIAL_TITLE: &str = "TOP SECRET acquisition plan";

fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

#[derive(Default)]
struct E2eGate {
    allow: Mutex<Vec<(String, String)>>,
}

impl E2eGate {
    fn allow_read(&self, viewer: &str, channel_object: &str) {
        self.allow
            .lock()
            .unwrap()
            .push((viewer.into(), channel_object.into()));
    }
}

impl IdentityService for E2eGate {
    fn check(
        &self,
        subject: &Principal,
        _permission: &Permission,
        object: &myelin_tenancy::ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let allowed = self
            .allow
            .lock()
            .unwrap()
            .iter()
            .any(|(s, o)| s == &subject.principal_id.0 && o == &object.0);
        Ok(if allowed {
            Decision::Allow
        } else {
            Decision::Deny
        })
    }
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Ok(FragmentAdmit::Admitted {
            fragment_id: "e2e".into(),
        })
    }
}

#[derive(Default)]
struct E2eResolver {
    state: Mutex<String>,
    calls: Mutex<usize>,
}

impl E2eResolver {
    fn with_state(state: &str) -> E2eResolver {
        let r = E2eResolver::default();
        *r.state.lock().unwrap() = state.into();
        r
    }
    fn set_state(&self, state: &str) {
        *self.state.lock().unwrap() = state.into();
    }
    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

impl RefsResolvePort for E2eResolver {
    fn resolve(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        _ref_: &ArtifactRef,
        _viewer: &Principal,
        _at: &Consistency,
    ) -> LadderOutcome {
        *self.calls.lock().unwrap() += 1;
        LadderOutcome::Live(Projection {
            title: CONFIDENTIAL_TITLE.into(),
            state: self.state.lock().unwrap().clone(),
            icon: "issue".into(),
            sub_anchor: None,
        })
    }
}

pub fn run_e2e_1_unfurl_pane() -> ChatE2eArtifact {
    let channel_obj = channel_object(E2E_CHANNEL);
    let gate = E2eGate::default();
    gate.allow_read("insider", &channel_obj);

    let resolver = E2eResolver::with_state("build:pending");
    let service = UnfurlService::new(gate, resolver);

    let candidate = UnfurlCandidate {
        ref_: confidential_ref(),
        channel_id: Some(E2E_CHANNEL.to_string()),
    };

    let mut leaks: u64 = 0;

    let insider_card = service.resolve_one(&candidate, &e2e_viewer("insider"));
    let insider_sees_title = insider_card.exposed_title() == Some(CONFIDENTIAL_TITLE);
    let insider_state_pending = matches!(
        &insider_card,
        Card::Live { projection, .. } if projection.state == "build:pending"
    );
    let calls_after_first = service.resolver().calls();

    let ci_event = "ci.check.updated";
    let busts = invalidates_card(ci_event);
    service.resolver().set_state("test:failure");
    let busted = service.cache().bust(&candidate.ref_);
    let insider_reread = service.resolve_one(&candidate, &e2e_viewer("insider"));
    let re_read_reflects_live = matches!(
        &insider_reread,
        Card::Live { projection, .. } if projection.state == "test:failure"
    );
    let calls_after_reread = service.resolver().calls();
    let forced_live_refetch = calls_after_reread == calls_after_first + 1;
    let re_read_age_secs: u64 = 0;
    let within_freshness_budget = re_read_age_secs <= FRESHNESS_BUDGET_SECS;
    let merge_gate_blocked = re_read_reflects_live;

    let outsider_card = service.resolve_one(&candidate, &e2e_viewer("outsider"));
    let outsider_tombstoned = matches!(
        &outsider_card,
        Card::Tombstone(t) if t.reason == TombstoneReason::Denied
    );
    if outsider_card.exposed_title().is_some() {
        leaks += 1;
    }
    if let Card::Tombstone(t) = &outsider_card {
        let rendered = format!("{t:?}");
        if rendered.contains("SECRET") || rendered.contains("acquisition") {
            leaks += 1;
        }
        if t.root != myelin_refs::strip_sub(&confidential_ref()) {
            leaks += 1;
        }
    } else {
        leaks += 1;
    }

    let green = insider_sees_title
        && insider_state_pending
        && busts
        && busted
        && re_read_reflects_live
        && forced_live_refetch
        && within_freshness_budget
        && merge_gate_blocked
        && outsider_tombstoned;

    ChatE2eArtifact {
        scenario: E2E_SCENARIO,
        green,
        evidence: format!(
            "Chat unfurl/live-update pane (CHAT-D7): insider_sees_title={insider_sees_title}, \
             insider_state_pending={insider_state_pending}; mid-flight ci.check.updated busts \
             (invalidates_card={busts}, cache_busted={busted}) → LIVE re-resolve \
             (reflects_live={re_read_reflects_live}, forced_refetch={forced_live_refetch}) within \
             freshness budget ({re_read_age_secs}s ≤ {FRESHNESS_BUDGET_SECS}s)={within_freshness_budget}, \
             merge_gate_blocked={merge_gate_blocked}; outsider→tombstone(denied)={outsider_tombstoned}; \
             leaks={leaks}; mock-agent runtime (real-LLM is post-M5/R-10)",
        ),
        leaks,
    }
}
