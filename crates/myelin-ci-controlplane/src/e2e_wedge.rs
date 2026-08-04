use std::collections::{HashMap, HashSet};

use myelin_ci_sandbox::replay::{CiReindexSource, CiReplayKind};
use myelin_events::{
    reindex, Actor, ArtifactRef, CorrelationId, DerivedStore, EmitContextBase, EventEnvelope,
    OutboxStore, Region, ReindexSource, SnapshotDraft, SnapshotScope, TenantId, Timestamp,
};
use myelin_flow::ApprovalDecision;
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_storage::blob::ContentHash;

use crate::check_emitter::{
    assemble_check_status, details_ref, CheckEmitContext, CheckProvider, CheckState, CostPosture,
    TrustTier,
};
use crate::deployment::{DeployGate, DeployGateOutcome};
use crate::surfacing::{
    ci_run_ref, ArtifactStore, Projected, Projector, RunMeta, TombstoneReason, VIEW,
};

pub const E2E_SCENARIOS: [&str; 2] = ["E2E-1", "E2E-3"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    pub scenario: &'static str,
    pub green: bool,
    pub evidence: String,
    pub leaks: u64,
    pub seal: String,
}

impl E2eArtifact {
    pub(crate) fn sealed(
        scenario: &'static str,
        green: bool,
        leaks: u64,
        evidence: impl Into<String>,
    ) -> Self {
        let evidence = evidence.into();
        let mut body = Vec::new();
        push_lp(&mut body, scenario.as_bytes());
        push_lp(&mut body, &[u8::from(green)]);
        push_lp(&mut body, &leaks.to_be_bytes());
        push_lp(&mut body, evidence.as_bytes());
        let seal = ContentHash::blake3(&body).to_multihash_string();
        E2eArtifact {
            scenario,
            green,
            evidence,
            leaks,
            seal,
        }
    }

    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn e2e_tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn e2e_region() -> Region {
    Region("fr-par".into())
}

fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

fn e2e_platform() -> Principal {
    Principal::stub(
        PrincipalId("platform".into()),
        PrincipalKind::Service,
        e2e_tenant(),
    )
}

fn e2e_ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: e2e_tenant(),
        region: e2e_region(),
        actor: Actor(e2e_platform()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        caused_by: None,
    }
}

fn e2e_zookie() -> Zookie {
    Zookie("z0".into())
}

struct WedgeId {
    allow: HashSet<String>,
}

impl WedgeId {
    fn new() -> Self {
        Self {
            allow: HashSet::new(),
        }
    }
    fn allow_view(mut self, viewer: &Principal, object: &ArtifactRef) -> Self {
        self.allow
            .insert(format!("{}|{}@{}", viewer.principal_id.0, VIEW, object.0));
        self
    }
}

impl IdentityService for WedgeId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("wedge: authenticate n/a"))
    }
    fn check(
        &self,
        s: &Principal,
        p: &Permission,
        o: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(
            if self
                .allow
                .contains(&format!("{}|{}@{}", s.principal_id.0, p.0, o.0))
            {
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
        Err(AuthzError::NotYetImplemented("wedge: list_objects n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("wedge: list_subjects n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("wedge: explain n/a"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("wedge: delegation n/a"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("wedge: write_tuples n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("wedge: mint_run_token n/a"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("wedge: revoke n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented(
            "wedge: resolve_pseudonym n/a",
        ))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("wedge: erase n/a"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("wedge: admit_fragment n/a"))
    }
}

const E2E1_SECRET_PIPELINE: &str = "cerberus-acquisition-release";

const E2E1_RUN_ID: &str = "run-cerberus-42";

const E2E1_FAIL_STEP: u32 = 3;

pub fn run_e2e1_pr_context_pane() -> E2eArtifact {
    let mut leaks: u64 = 0;
    let collaborator = e2e_viewer("collaborator");
    let denied = e2e_viewer("denied-teammate");
    let tenant = e2e_tenant();
    let run_ref = ci_run_ref(&tenant.0, E2E1_RUN_ID);

    let emit_ctx = CheckEmitContext {
        tenant: tenant.0.clone(),
        repo: format!("myelin://{}/git/repo/cerberus", tenant.0),
        commit_oid: "deadbeefcafe".to_string(),
        run_ref: run_ref.0.clone(),
        run_attempt: 1,
        trust_tier: TrustTier::Trusted,
        started_at: "2026-06-25T00:00:00Z".to_string(),
        completed_at: Some("2026-06-25T00:01:00Z".to_string()),
    };
    let _build_ok = assemble_check_status(
        &emit_ctx,
        CheckProvider::Ci,
        "build",
        CheckState::Success,
        true,
        CostPosture::Settled,
        None,
    );
    let test_fail = assemble_check_status(
        &emit_ctx,
        CheckProvider::Ci,
        "test",
        CheckState::Failure,
        true,
        CostPosture::Settled,
        Some(E2E1_FAIL_STEP),
    );
    let expected_anchor = details_ref(&run_ref.0, CheckState::Failure, Some(E2E1_FAIL_STEP));
    let anchor_present = test_fail
        .payload
        .get("details_ref")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s == expected_anchor && s.ends_with(&format!("#step-{E2E1_FAIL_STEP}")));
    if !anchor_present {
        leaks += 1;
    }

    let mut store = ArtifactStore::new();
    store.put_run(
        &run_ref,
        RunMeta {
            number: 42,
            pipeline: E2E1_SECRET_PIPELINE.to_string(),
            state: "failed".to_string(),
            dag_summary: "1/2 stages green".to_string(),
            failed_step: Some(E2E1_FAIL_STEP as u64),
            duration_secs: Some(60),
        },
    );
    let id = WedgeId::new().allow_view(&collaborator, &run_ref);
    let projector = Projector::new(id, store);
    let embed = ArtifactRef(format!("{}#step-{E2E1_FAIL_STEP}", run_ref.0));

    let collab_view = projector
        .project(&embed, &collaborator, e2e_zookie())
        .expect("collaborator projection");
    let collab_sees_run = match &collab_view {
        Projected::Visible(p) => {
            p.state == "failed"
                && p.title.contains(E2E1_SECRET_PIPELINE)
                && p.sub_anchor
                    .as_ref()
                    .is_some_and(|a| a.kind == "step" && a.step == E2E1_FAIL_STEP as u64)
        }
        Projected::Tombstoned(_) => false,
    };
    let merge_blocked = test_fail
        .payload
        .get("state")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s == "failure");

    let denied_view = projector
        .project(&embed, &denied, e2e_zookie())
        .expect("denied projection");
    match &denied_view {
        Projected::Tombstoned(t) => {
            if t.reason != TombstoneReason::Unauthorized {
                leaks += 1;
            }
            if t.display_text().contains("cerberus") || t.display_text().contains("acquisition") {
                leaks += 1;
            }
            let rendered = format!("{denied_view:?}");
            if rendered.contains("cerberus") || rendered.contains("acquisition") {
                leaks += 1;
            }
            if denied_view.title().is_some() {
                leaks += 1;
            }
        }
        Projected::Visible(_) => {
            leaks += 1;
        }
    }

    let mut store2 = ArtifactStore::new();
    store2.put_run(
        &run_ref,
        RunMeta {
            number: 42,
            pipeline: E2E1_SECRET_PIPELINE.to_string(),
            state: "failed".to_string(),
            dag_summary: "1/2 stages green".to_string(),
            failed_step: Some(E2E1_FAIL_STEP as u64),
            duration_secs: Some(60),
        },
    );
    store2.mark_erased(&run_ref);
    let id2 = WedgeId::new().allow_view(&collaborator, &run_ref);
    let projector2 = Projector::new(id2, store2);
    let collab_after_erase = projector2
        .project(&embed, &collaborator, e2e_zookie())
        .expect("collaborator projection after erase");
    let erasure_honoured_live = match &collab_after_erase {
        Projected::Tombstoned(t) => t.reason == TombstoneReason::Erased,
        Projected::Visible(_) => {
            leaks += 1;
            false
        }
    };
    let rendered_after = format!("{collab_after_erase:?}");
    if rendered_after.contains("cerberus") || rendered_after.contains("acquisition") {
        leaks += 1;
    }

    let green = anchor_present
        && collab_sees_run
        && merge_blocked
        && erasure_honoured_live
        && matches!(denied_view, Projected::Tombstoned(_));
    E2eArtifact::sealed(
        "E2E-1",
        green,
        leaks,
        format!(
            "PR-context-pane: ci.check.updated (build→success,test→failure) emitted with #step-{} \
             anchor; collaborator run-row resolves live (state=failed, merge blocked); denied viewer \
             → content-free tombstone ({} row leaks); mid-flight erase honoured live → run embed \
             degrades to Erased tombstone",
            E2E1_FAIL_STEP, leaks
        ),
    )
}

fn e2e3_spec_ref() -> String {
    "myelin://acme/knowledge/page/spec-payments-v2".to_string()
}

fn e2e3_issue_ref() -> String {
    "myelin://acme/issue/issue/PAY-1".to_string()
}

fn e2e3_pr_ref() -> String {
    "myelin://acme/git/pr/PR-7".to_string()
}

const E2E3_RUN_ID: &str = "run-payments-ship";

const E2E3_DEPLOY_CARD: &str = "deploy-prod-payments-v2";

fn e2e3_ci_source() -> CiReindexSource {
    let mut src = CiReindexSource::new();
    let run_ref = ci_run_ref("acme", E2E3_RUN_ID);
    src.upsert(
        CiReplayKind::Run,
        &run_ref.0,
        1,
        &run_ref.0,
        serde_json::json!({
            "overall": "success",
            "commit": "feedface",
            "pr": e2e3_pr_ref(),
        }),
    );
    src
}

fn e2e3_snapshot_envelope(draft: &SnapshotDraft) -> EventEnvelope {
    EventEnvelope {
        event_id: draft.event_id(),
        type_: draft.type_.clone(),
        schema_ver: 1,
        tenant: e2e_tenant(),
        region: e2e_region(),
        actor: Actor(e2e_platform()),
        subject: draft.subject.clone(),
        aggregate: draft.aggregate.clone(),
        causation_id: None,
        correlation_id: CorrelationId(draft.event_id().0),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        payload: draft.payload.clone(),
    }
}

#[derive(Clone, Debug)]
struct LineageHop {
    source: String,
    target: String,
    rel: String,
    seal: String,
}

fn seal_lineage(hops: &[(String, String, String)]) -> Vec<LineageHop> {
    let mut prev = String::from("genesis");
    let mut out = Vec::new();
    for (source, target, rel) in hops {
        let mut body = Vec::new();
        push_lp(&mut body, prev.as_bytes());
        push_lp(&mut body, source.as_bytes());
        push_lp(&mut body, target.as_bytes());
        push_lp(&mut body, rel.as_bytes());
        let seal = ContentHash::blake3(&body).to_multihash_string();
        out.push(LineageHop {
            source: source.clone(),
            target: target.clone(),
            rel: rel.clone(),
            seal: seal.clone(),
        });
        prev = seal;
    }
    out
}

fn verify_lineage(hops: &[LineageHop]) -> bool {
    let mut prev = String::from("genesis");
    for hop in hops {
        let mut body = Vec::new();
        push_lp(&mut body, prev.as_bytes());
        push_lp(&mut body, hop.source.as_bytes());
        push_lp(&mut body, hop.target.as_bytes());
        push_lp(&mut body, hop.rel.as_bytes());
        let expect = ContentHash::blake3(&body).to_multihash_string();
        if expect != hop.seal {
            return false;
        }
        prev = hop.seal.clone();
    }
    true
}

fn e2e3_lineage_hops() -> Vec<(String, String, String)> {
    let spec = e2e3_spec_ref();
    let issue = e2e3_issue_ref();
    let pr = e2e3_pr_ref();
    let run = ci_run_ref("acme", E2E3_RUN_ID).0;
    let deploy = format!("myelin://acme/ci/deployment/{E2E3_DEPLOY_CARD}");
    vec![
        (spec, issue.clone(), "decomposes".to_string()),
        (issue, pr.clone(), "closes".to_string()),
        (pr.clone(), run.clone(), "checked_by".to_string()),
        (run, deploy, "ships_via".to_string()),
    ]
}

pub fn run_e2e3_spec_to_ship_lineage() -> E2eArtifact {
    let mut leaks: u64 = 0;
    let hops = e2e3_lineage_hops();

    let spec = e2e3_spec_ref();
    let deploy = format!("myelin://acme/ci/deployment/{E2E3_DEPLOY_CARD}");
    let mut frontier = vec![spec.clone()];
    let mut reached: HashSet<String> = HashSet::new();
    while let Some(node) = frontier.pop() {
        for (s, t, _r) in &hops {
            if *s == node && reached.insert(t.clone()) {
                frontier.push(t.clone());
            }
        }
    }
    let run_ref = ci_run_ref("acme", E2E3_RUN_ID).0;
    let lineage_traceable = reached.contains(&e2e3_issue_ref())
        && reached.contains(&e2e3_pr_ref())
        && reached.contains(&run_ref)
        && reached.contains(&deploy);
    if !lineage_traceable {
        leaks += 1;
    }

    let mut applied: HashMap<String, String> = HashMap::new();
    let mut deploy_runs: u64 = 0;
    let withheld = DeployGate::gate_deploy(
        E2E3_DEPLOY_CARD,
        0,
        1,
        ApprovalDecision::Decline,
        &mut applied,
        || {
            deploy_runs += 1;
            E2E3_DEPLOY_CARD.to_string()
        },
    );
    if !matches!(withheld, DeployGateOutcome::Withheld(_)) || deploy_runs != 0 {
        leaks += 1;
    }
    let approved = DeployGate::gate_deploy(
        E2E3_DEPLOY_CARD,
        0,
        1,
        ApprovalDecision::Approve,
        &mut applied,
        || {
            deploy_runs += 1;
            E2E3_DEPLOY_CARD.to_string()
        },
    );
    let approved_again = DeployGate::gate_deploy(
        E2E3_DEPLOY_CARD,
        0,
        1,
        ApprovalDecision::Approve,
        &mut applied,
        || {
            deploy_runs += 1;
            E2E3_DEPLOY_CARD.to_string()
        },
    );
    let hitl_ships_exactly_once =
        approved.is_applied() && approved_again.is_applied() && deploy_runs == 1;
    if !hitl_ships_exactly_once {
        leaks += 1;
    }

    let source = e2e3_ci_source();
    let scope = SnapshotScope::new("ci", "run:all");
    let mut live = DerivedStore::new();
    for draft in source.replay(&scope, None) {
        live.ingest(&e2e3_snapshot_envelope(&draft));
    }
    let sources: &[&dyn ReindexSource] = &[&source];
    let mut outbox = OutboxStore::new();
    reindex(&scope, None, sources, &mut outbox, e2e_ctx_base()).expect("reindex replay");
    let mut cold = DerivedStore::new();
    assert!(cold.is_empty(), "the derived store is wiped before rebuild");
    for draft in source.replay(&scope, None) {
        let row = outbox.row(&draft.event_id()).expect("snapshot row present");
        cold.ingest(&row.envelope);
    }
    let cold_equals_live = cold.len() == live.len() && cold.parity_bytes() == live.parity_bytes();
    if !cold_equals_live {
        leaks += 1;
    }

    let honest = seal_lineage(&hops);
    let honest_verifies = verify_lineage(&honest);
    if !honest_verifies {
        leaks += 1;
    }
    let mut tampered = honest.clone();
    if let Some(last) = tampered.last_mut() {
        last.source = "myelin://acme/ci/run/run-FORGED".to_string();
    }
    let tamper_detected = !verify_lineage(&tampered);
    if !tamper_detected {
        leaks += 1;
    }

    let green = lineage_traceable
        && hitl_ships_exactly_once
        && cold_equals_live
        && honest_verifies
        && tamper_detected;
    E2eArtifact::sealed(
        "E2E-3",
        green,
        leaks,
        format!(
            "spec→issue→PR→run→deploy lineage traceable={lineage_traceable}; \
             HITL-gated deploy: decline-withheld + approve-ships-exactly-once={hitl_ships_exactly_once}; \
             cold-reindex==live={cold_equals_live} (parity bytes byte-match); \
             audit honest-verifies={honest_verifies}, tamper-detected={tamper_detected}"
        ),
    )
}

pub fn run_ci_e2e_slices() -> Vec<E2eArtifact> {
    vec![run_e2e1_pr_context_pane(), run_e2e3_spec_to_ship_lineage()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2e1_pr_context_pane_zero_row_leak() {
        let art = run_e2e1_pr_context_pane();
        assert_eq!(art.scenario, "E2E-1");
        assert_eq!(art.leaks, 0, "0 row leak across every projection: {art:?}");
        assert!(art.is_green(), "E2E-1 green not earned: {art:?}");
        assert!(art.seal.starts_with("blake3:"));
    }

    #[test]
    fn e2e3_spec_to_ship_cold_equals_live_and_tamper_detected() {
        let art = run_e2e3_spec_to_ship_lineage();
        assert_eq!(art.scenario, "E2E-3");
        assert_eq!(art.leaks, 0, "0 divergence/undetected-tamper: {art:?}");
        assert!(art.is_green(), "E2E-3 green not earned: {art:?}");
        assert!(art.seal.starts_with("blake3:"));
    }

    #[test]
    fn both_slices_green_and_distinctly_sealed() {
        let arts = run_ci_e2e_slices();
        assert_eq!(arts.len(), 2);
        assert!(arts.iter().all(|a| a.is_green()));
        assert_ne!(arts[0].seal, arts[1].seal);
        assert_eq!(E2E_SCENARIOS, ["E2E-1", "E2E-3"]);
    }

    #[test]
    fn e2e1_unauthorized_projection_carries_no_run_fragment() {
        let denied = e2e_viewer("nobody");
        let run_ref = ci_run_ref("acme", E2E1_RUN_ID);
        let embed = ArtifactRef(format!("{}#step-{E2E1_FAIL_STEP}", run_ref.0));
        let mut store = ArtifactStore::new();
        store.put_run(
            &run_ref,
            RunMeta {
                number: 42,
                pipeline: E2E1_SECRET_PIPELINE.to_string(),
                state: "failed".to_string(),
                dag_summary: "1/2 stages green".to_string(),
                failed_step: Some(E2E1_FAIL_STEP as u64),
                duration_secs: Some(60),
            },
        );
        let projector = Projector::new(WedgeId::new(), store);
        let view = projector.project(&embed, &denied, e2e_zookie()).unwrap();
        assert!(matches!(view, Projected::Tombstoned(_)));
        assert!(view.title().is_none(), "a tombstone has no title");
        let rendered = format!("{view:?}");
        assert!(!rendered.contains("cerberus"));
        assert!(!rendered.contains("acquisition"));
    }

    #[test]
    fn e2e3_verify_catches_a_reordered_chain() {
        let hops = e2e3_lineage_hops();
        let mut sealed = seal_lineage(&hops);
        assert!(verify_lineage(&sealed));
        sealed.swap(1, 2);
        assert!(
            !verify_lineage(&sealed),
            "a reordered chain must fail verify"
        );
    }

    #[test]
    fn e2e3_hitl_decline_withholds_zero_mutation() {
        let mut applied: HashMap<String, String> = HashMap::new();
        let mut runs = 0u64;
        let out = DeployGate::gate_deploy(
            "card",
            0,
            1,
            ApprovalDecision::Decline,
            &mut applied,
            || {
                runs += 1;
                "dep".to_string()
            },
        );
        assert!(matches!(out, DeployGateOutcome::Withheld(_)));
        assert_eq!(runs, 0, "a declined deploy must not mutate");
    }

    #[test]
    fn e2e_artifact_seal_is_deterministic() {
        let a = E2eArtifact::sealed("E2E-1", true, 0, "same body");
        let b = E2eArtifact::sealed("E2E-1", true, 0, "same body");
        assert_eq!(a.seal, b.seal, "the seal is a pure function of the body");
        let c = E2eArtifact::sealed("E2E-1", true, 1, "same body");
        assert_ne!(a.seal, c.seal, "a different leak count seals differently");
    }
}
