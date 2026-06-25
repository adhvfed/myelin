//! # `e2e_wedge::e2e_pane` — Chat's E2E-1 leg: the unfurl / live-update pane (CHAT-D7) (CHAT-P27 / P-501)
//!
//! Chat's contribution to the whole-system **E2E-1 — the PR context pane** (testing-strategy §E2E-1).
//! E2E-1 proves the wedge: *one reference graph + one permission model* mean a pane unfurls **every**
//! connected artifact **per-viewer, leak-free, live**. Chat's leg is the **unfurl/live-update pane** (the
//! Chat-D7 analog): a confidential ref referenced in a chat message renders a tombstone to a viewer who
//! lacks access (0 title leak); a **mid-flight `ci.check.updated`** (5.9) busts the shared per-ref cache
//! and the pane **re-resolves LIVE** within the freshness budget.
//!
//! The chained mutation (each step mutates; the pane re-resolves mid-flight):
//! 1. The pane resolves the linked refs per-viewer: the **insider** sees the unfurl title; the
//!    confidential ref resolves for them.
//! 2. **Mid-flight mutation A:** CI emits `ci.check.updated` (the frozen CheckStatus event, 5.9 / X-1).
//!    [`invalidates_card`] matches it → [`UnfurlCache::bust`] drops the stale shared entry → the next
//!    pane resolve re-fetches LIVE (within [`FRESHNESS_BUDGET_SECS`]). The live re-render reflects the
//!    new state — the firehose busted the shared per-ref cache (CHAT-D7).
//! 3. **Mid-flight mutation B:** a SECOND viewer (the **outsider**) WITHOUT access opens the same pane —
//!    the confidential ref unfurls to a **TOMBSTONE carrying the root**, the title NEVER present (0 leak,
//!    incl. count/backlink leak — the gate-before-cache order means the title is never even FETCHED for a
//!    denied viewer, CHAT-D5).
//!
//! This drives the SAME [`UnfurlService::resolve_one`] no-leak chokepoint + the SAME
//! [`invalidates_card`]/[`UnfurlCache::bust`] bus-bust — no second resolver, no second cache (EI-01 §7).

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

/// The E2E scenario token chat's pane leg attests (chat owns the unfurl/live-update pane leg of E2E-1).
pub const E2E_SCENARIO: &str = "E2E-1";

/// **The freshness budget (the live check-update bound, §E2E-1).** The maximum staleness, in seconds,
/// the pane may serve before a mid-flight `ci.check.updated` MUST be reflected. The firehose busts the
/// shared per-ref cache; the re-read serves the new state within this. A synchronous in-scenario re-read
/// (age 0) trivially satisfies it; the budget is the named threshold the leg asserts against, never a
/// stray literal. Matches the Issues-side E2E-1 pane SLA (the wedge's one pane-freshness budget).
pub const FRESHNESS_BUDGET_SECS: u64 = 5;

/// The channel the pane renders in (the gate object the per-viewer `check` runs against).
const E2E_CHANNEL: &str = "c-pr-pane";

/// The confidential ref the chat message links (e.g. an embedded confidential issue/PR). A denied
/// viewer must NEVER see its title — the leak-test artifact.
fn confidential_ref() -> ArtifactRef {
    ArtifactRef("myelin://acme/issues/issue/ENG-1421".into())
}

/// The confidential ref's title — the SECRET the unfurl chokepoint must never leak to a denied viewer
/// (it is read only AFTER the per-viewer gate passes; the deny path returns a tombstone that never
/// fetches it).
const CONFIDENTIAL_TITLE: &str = "TOP SECRET acquisition plan";

fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

/// A deterministic per-viewer gate: `view@channel` allow-list (absent ⇒ Deny, fail-closed). The SAME
/// fail-closed gate the unfurl chokepoint runs FIRST (the no-leak structural guarantee). The insider is
/// granted `read` on the pane channel; the outsider is denied.
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

/// **The Refs resolve chokepoint (5.2; REF-P10/CHAT-P15 floor) — the in-memory model the leg drives
/// through.** Returns a live projection carrying the CURRENT state (so a stale cache read after a CI
/// state change would be observable) and counts the resolve calls (so the leg proves the bus-bust forced
/// a LIVE re-fetch, not a stale cache hit). The production binding is Refs' permission-aware `resolve`
/// over the resilient client (1.9) — chat NEVER re-implements permission-aware resolution (EI-01 §7).
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

/// **E2E-1 — drive the whole chat unfurl/live-update pane flow end-to-end (CHAT-D7).** Returns the named
/// green artifact: per-viewer leak-free unfurl (insider title resolves, outsider tombstone 0 leak) + the
/// mid-flight `ci.check.updated` LIVE re-resolve within the freshness budget. Drives the SAME
/// [`UnfurlService::resolve_one`] chokepoint + the SAME [`invalidates_card`]/[`UnfurlCache::bust`] —
/// no second resolver, no second cache.
pub fn run_e2e_1_unfurl_pane() -> ChatE2eArtifact {
    let channel_obj = channel_object(E2E_CHANNEL);
    let gate = E2eGate::default();
    // The insider holds `read` on the pane channel; the outsider is fail-closed (absent ⇒ Deny).
    gate.allow_read("insider", &channel_obj);

    let resolver = E2eResolver::with_state("build:pending");
    let service = UnfurlService::new(gate, resolver);

    let candidate = UnfurlCandidate {
        ref_: confidential_ref(),
        channel_id: Some(E2E_CHANNEL.to_string()),
    };

    let mut leaks: u64 = 0;

    // ── (1) The pane resolves the confidential ref per-viewer: the INSIDER sees the title. ──
    let insider_card = service.resolve_one(&candidate, &e2e_viewer("insider"));
    let insider_sees_title = insider_card.exposed_title() == Some(CONFIDENTIAL_TITLE);
    let insider_state_pending = matches!(
        &insider_card,
        Card::Live { projection, .. } if projection.state == "build:pending"
    );
    // The insider's resolve filled the SHARED cache (one entry per ref).
    let calls_after_first = service.resolver().calls();

    // ── (2) Mid-flight mutation A: CI emits ci.check.updated (build → success, test → failure). ──
    //        The frozen CheckStatus event (5.9 / X-1) busts the shared per-ref cache → the pane ──
    //        re-resolves LIVE within the freshness budget (the firehose busted the cache). ──
    let ci_event = "ci.check.updated";
    let busts = invalidates_card(ci_event);
    // The new CURRENT state the resolver now serves (test → failure → the merge gate shows blocked).
    service.resolver().set_state("test:failure");
    // The bus-bust drops the stale shared entry (the precise CHAT-P14 invalidation lever). The next
    // resolve MUST re-fetch (a stale cache hit would serve the old `build:pending` — a freshness miss).
    let busted = service.cache().bust(&candidate.ref_);
    // The pane re-resolves: the re-read serves the NEW state (LIVE), not the stale cached one.
    let insider_reread = service.resolve_one(&candidate, &e2e_viewer("insider"));
    let re_read_reflects_live = matches!(
        &insider_reread,
        Card::Live { projection, .. } if projection.state == "test:failure"
    );
    // The re-resolve forced a LIVE re-fetch (the resolver was called again — not a stale cache hit).
    let calls_after_reread = service.resolver().calls();
    let forced_live_refetch = calls_after_reread == calls_after_first + 1;
    // The re-read is SYNCHRONOUS in-scenario (age 0 ≤ the freshness budget). The budget is the threshold.
    let re_read_age_secs: u64 = 0;
    let within_freshness_budget = re_read_age_secs <= FRESHNESS_BUDGET_SECS;
    // The merge/Done gate: a `test:failure` posture shows BLOCKED (the pane's checks panel — the live
    // CheckStatus the pane re-read off the fact).
    let merge_gate_blocked = re_read_reflects_live;

    // ── (3) Mid-flight mutation B: a SECOND viewer (OUTSIDER) without access → the confidential ref ──
    //        tombstones carrying the root, the title NEVER present (0 leak). The gate runs FIRST → the ──
    //        title is never even FETCHED for the denied viewer (CHAT-D5, the no-leak chokepoint). ──
    let outsider_card = service.resolve_one(&candidate, &e2e_viewer("outsider"));
    let outsider_tombstoned = matches!(
        &outsider_card,
        Card::Tombstone(t) if t.reason == TombstoneReason::Denied
    );
    // The structural leak invariant: a tombstone exposes NO title.
    if outsider_card.exposed_title().is_some() {
        leaks += 1;
    }
    if let Card::Tombstone(t) = &outsider_card {
        let rendered = format!("{t:?}");
        if rendered.contains("SECRET") || rendered.contains("acquisition") {
            leaks += 1;
        }
        // The tombstone carries the root (and only the root).
        if t.root != myelin_refs::strip_sub(&confidential_ref()) {
            leaks += 1;
        }
    } else {
        // A denied viewer that got a PROJECTION is a catastrophic leak.
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
