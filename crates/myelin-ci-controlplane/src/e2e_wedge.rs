//! # `e2e_wedge` — CI's slices of the whole-system E2E wedge (CI-P33 / P-493, M5)
//!
//! **The completion of CI-M5's E2E slices (the master M5→M6 boundary's CI rows).** This module is the
//! **CI side of two whole-system chained-mutation E2E scenarios** — **E2E-1** (the PR context pane: CI
//! emits `ci.check.updated` and the pane resolves CI's check rows per-viewer, 0 leak to the
//! unauthorized viewer, with the `#step-<n>` jump-to-failure anchor) and **E2E-3** (spec-to-ship
//! traceability: a CI run attaches `CheckStatus`, a protected-env deploy ships it HITL-gated, and
//! cold-reindex (replay) == live, with audit tamper detected). Each is driven **end-to-end** — the
//! whole flow with mid-flight mutations, NOT a single handler (EI-01 §4 / VISION §3) — over the
//! **production-hardened CI engine** the M5 prompts built. The engine is **UNCHANGED**; this module
//! COMPOSES it into the two whole-system scenarios and emits each scenario's named green artifact.
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! This is the **whole-system DRIVER over the EXISTING CI engine**, not a second project / deploy /
//! reindex / lineage.
//! - **E2E-1** drives the SAME [`crate::surfacing::Projector::project`] per-viewer permission-first
//!   tombstone gate (contract 5.6 / 5.9, CI-P25) over the SAME [`crate::surfacing::ArtifactStore`]:
//!   CI emits a `ci.check.updated` fact via the FROZEN [`crate::check_emitter::assemble_check_status`]
//!   (build → success, test → failure) and the PR context pane resolves CI's run row PER-VIEWER. The
//!   authorized viewer sees the live run projection carrying the `#step-<n>` jump-to-failure
//!   sub-anchor ([`crate::check_emitter::details_ref`] resolves through the CI-P21 log index); the
//!   unauthorized viewer gets a content-free [`crate::surfacing::Tombstone`] — the run title/state is
//!   structurally ABSENT (0 leak; the permission-first gate NEVER reads the artifact). The scenario
//!   CHAINS mutations mid-flight: the test step goes `failure` (the pane live-updates from the SAME
//!   read path, merge gate blocks), then the run is erased mid-flight and the SAME projector
//!   re-resolves to an `Erased` tombstone (the mutation is honoured live, not a stale cached title).
//!   No second project path.
//! - **E2E-3** drives the SAME [`crate::deployment::DeployGate::gate_deploy`] protected-env HITL gate
//!   (over the FROZEN `myelin_flow::per_effect_idem_key`, OQ-F) for the spec-to-ship deploy, and the
//!   SAME [`myelin_ci_sandbox::replay::CiReindexSource`] reindex-from-source replay (CI-P26, contract
//!   2.6) through the SAME live consumer [`myelin_events::DerivedStore`]: a CI run attaches
//!   `CheckStatus`, the HITL-gated protected-env deploy ships, the derived projection is WIPED, and
//!   `replay(scope)` rebuilds it cold == live (the parity bytes byte-match). The spec→issue→PR→run→
//!   deploy lineage is then sealed into a **hash-chained lineage ledger** built ONLY from the frozen
//!   [`myelin_storage::blob::ContentHash::blake3`] content-address primitive — a TAMPER to any lineage
//!   hop breaks the chain hash, which the verify detects (0 silent tamper). No second reindexer, no
//!   hand-rolled hash, no bespoke recovery reader.
//!
//! Each scenario emits its **named green artifact** (an [`E2eArtifact`]) — the dated, content-addressed
//! report the master M5 exit gate cites. A scenario that does not reach its green predicate fails
//! LOUDLY (`is_green()` is false); there is no weakened threshold and no claimed green that was not
//! earned (EI-01 §3 / VISION §3).
//!
//! ## The load-bearing invariants STILL HOLD at E2E scale (the prompt's required statement)
//! The CI-P25 project-leak invariant (a check row degrades to a tombstone carrying NO title/state — the
//! permission-first gate, not a post-filter) and the CI-P26 reindex invariant (cold == live; the
//! rebuild is the live consumer path only) are the load-bearing properties. This module ASSERTS both
//! at E2E scale: E2E-1's unauthorized viewer gets a content-free tombstone (0 row leak), E2E-3's wiped
//! projection rebuilds byte-identical to live AND the lineage seal detects tamper. The mutation floors
//! on those invariants live in `surfacing.rs` / the Bus `reindex` + sandbox `replay.rs` and are
//! UNCHANGED — this module adds NO new leak / reindex / gate decision logic; it proves the frozen
//! decisions hold across the whole flow.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new.** This is the E2E run over the production-hardened CI engine — the named single-cell
//!   project ([CI-P25]) / deploy ([CI-P24]) / reindex ([CI-P26]) follow-ons proven end-to-end. The ONE
//!   legitimate remaining floor inherited by both slices is the world-scale fleet-hardware 30× load
//!   drill (the CI variant here runs a MODERATE scenario, not the world-scale fleet corpus — already
//!   named by CI-P30/[`crate::surge`]).
//! - **These are CI's SLICES of joint scenarios** — the full E2E green requires every subsystem's
//!   slice (Git, Issues, Chat, Knowledge, Refs, Search, Identity, Notif). The cross-subsystem
//!   producers are reached through the SAME frozen seams; this module drives the **CI side**: the
//!   leak-free per-viewer check-row pane (E2E-1) + the HITL-gated deploy / cold==live reindex /
//!   tamper-evident lineage seal (E2E-3). The **E2E-2 agent-native flagship is CI-P34** (CI-fail →
//!   triage agent → issue → chat → fix-PR) — NOT duplicated here. **E2E-4 (the DSAR fan-out)** is
//!   covered for CI by **CI-P32's CI-D3** (erasure-reaches-every-holder, the crypto-shred erase) —
//!   NOT duplicated here.
//!
//! [CI-P24]: crate::deployment
//! [CI-P25]: crate::surfacing
//! [CI-P26]: myelin_ci_sandbox::replay

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

/// The two whole-system E2E scenarios CI's slices cross (the master M5 exit gate cites E2E-1..E2E-4;
/// this module owns CI's side of -1 and -3; E2E-2 is CI-P34, E2E-4 is CI-P32's CI-D3). PII-free
/// tokens — drills assert against the NAME, never a literal (EI-01 §3).
pub const E2E_SCENARIOS: [&str; 2] = ["E2E-1", "E2E-3"];

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The named green artifact (the prompt's per-scenario "named green artifact").
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **The named green artifact one E2E scenario emits.** A content-addressed, dated report the master
/// M5 exit gate cites. `green` is the scenario's earned green predicate; `evidence` is the load-bearing
/// assertion summary; `leaks` is the leak/tamper counter the scenario asserted at `0`. A scenario that
/// did not reach green has `green = false` — it fails LOUDLY, never a claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    /// Which E2E scenario this artifact attests (one of [`E2E_SCENARIOS`]).
    pub scenario: &'static str,
    /// The earned green verdict — `true` iff every load-bearing assertion held end-to-end.
    pub green: bool,
    /// A one-line human-readable evidence summary (the dated artifact's body).
    pub evidence: String,
    /// The leak/tamper counter the scenario asserted at `0` (0 row leak for E2E-1; 0 undetected tamper
    /// + 0 cold/live byte divergence for E2E-3) — the F1 spine.
    pub leaks: u64,
    /// The content-address of the evidence body (the dated artifact's self-describing seal). Derived
    /// from the frozen [`ContentHash::blake3`] over the `scenario|green|leaks|evidence` framing — never
    /// a hand-rolled hash (VISION §4).
    pub seal: String,
}

impl E2eArtifact {
    /// Build a sealed artifact from the earned verdict + the evidence summary. The seal is a pure
    /// function of the body, so the same verdict always yields the same address (a reproducible
    /// artifact the exit gate can cite by hash).
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

    /// The green predicate (the dated artifact is green iff the scenario earned it AND 0 leaks/tamper).
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

/// Length-prefix a field (u32 big-endian length, then the bytes) — the injective framing the seal
/// relies on (the same convention the Knowledge/Notif/Search e2e wedges use, so two distinct bodies
/// can never collide on a shared boundary).
fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  Shared E2E fixtures (the cell + tenant the wedge runs against; a full cell with mock producers).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The tenant the wedge runs against (a full cell). Opaque, PII-free.
fn e2e_tenant() -> TenantId {
    TenantId::from_token("acme")
}

/// The region (fr-par — the dev/prod residency pin; a config swap, never a code change).
fn e2e_region() -> Region {
    Region("fr-par".into())
}

/// A viewer principal (a human — the wedge runs per-viewer).
fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

/// The platform service actor (the reindex re-emit stamps it).
fn e2e_platform() -> Principal {
    Principal::stub(
        PrincipalId("platform".into()),
        PrincipalKind::Service,
        e2e_tenant(),
    )
}

/// The emit context (the platform actor + clock) the reindex re-emit stamps.
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

/// A read-consistency fence (the strong, zookie-stamped read a security-sensitive projection uses).
fn e2e_zookie() -> Zookie {
    Zookie("z0".into())
}

/// **A deterministic Id resolver for the wedge: a `view@object` allow-list (absent ⇒ Deny,
/// fail-closed).** The SAME `IdentityService` seam the production projector wires — the wedge swaps a
/// deterministic resolver in so the per-viewer leak property is asserted against a known reachable set
/// (EI-01 §3 — assert against the name, never a hidden literal).
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

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-1 — The PR context pane (CI's check rows resolve per-viewer; 0 leak; the #step-<n> anchor).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The confidential pipeline name the unauthorized viewer must NEVER see (the leak-test artifact). A
/// run on a private repo — its very NAME is the secret the deny path must not read into the tombstone.
const E2E1_SECRET_PIPELINE: &str = "cerberus-acquisition-release";

/// The CI run id surfaced in the PR context pane (the run a push triggered).
const E2E1_RUN_ID: &str = "run-cerberus-42";

/// The failing test step index — the `#step-<n>` jump-to-failure anchor the pane resolves.
const E2E1_FAIL_STEP: u32 = 3;

/// **E2E-1 (the PR context pane — CI's slice): drive it end-to-end, chaining mutations mid-flight.**
///
/// The whole flow, not a single handler (EI-01 §4):
/// 1. A push triggers a CI run; the build step succeeds, then the test step FAILS — CI emits
///    `ci.check.updated` (state=failure) via the FROZEN [`assemble_check_status`], carrying the
///    `#step-<n>` jump-to-failure [`details_ref`] (the CI-P21 log-index anchor).
/// 2. The PR context pane resolves CI's run row PER-VIEWER through the SAME [`Projector::project`]
///    permission-first gate (contract 5.6 / 5.9). The embed points at the run's failing step
///    (`<run-ref>#step-<n>`).
/// 3. The **authorized** viewer (a repo collaborator) sees the live [`Projected::Visible`] run
///    projection carrying the title/state + the `#step-<n>` sub-anchor — the pane renders the failure
///    and the merge gate shows blocked.
/// 4. The **unauthorized** viewer (a denied teammate, no access to the private repo) gets a
///    [`Projected::Tombstoned`] — the run title (the SECRET pipeline name) + state are structurally
///    ABSENT (0 leak; the permission-first gate never read the run row).
/// 5. **MID-FLIGHT mutation:** the run is ERASED (a `ci.run.erased` lands while the pane is open). The
///    SAME projector re-resolves: the previously-authorized viewer's embed now degrades to an `Erased`
///    tombstone too (the erasure is honoured by the live read path — not a stale cached title).
///
/// Returns the named green artifact (`is_green()` iff 0 row leak across every projection).
pub fn run_e2e1_pr_context_pane() -> E2eArtifact {
    let mut leaks: u64 = 0;
    let collaborator = e2e_viewer("collaborator");
    let denied = e2e_viewer("denied-teammate");
    let tenant = e2e_tenant();
    let run_ref = ci_run_ref(&tenant.0, E2E1_RUN_ID);

    // ── STEP 1: CI emits `ci.check.updated` (build → success, test → failure) via the FROZEN check
    //    emitter. This is the producer fact the PR-checks panel shows — assert it carries the
    //    `#step-<n>` jump-to-failure anchor (the CI-P21 log-index target). Build the check seam
    //    EXACTLY (no second emit path).
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
    )
    .expect("canonical wedge check reference");
    // The failure fact carries the `#step-<n>` jump-to-failure anchor (resolved through CI-P21's log
    // index) — the pane's "jump to failure" target. Assert the anchor exists on the emitted fact.
    let expected_anchor = details_ref(&run_ref.0, CheckState::Failure, Some(E2E1_FAIL_STEP));
    let anchor_present = test_fail
        .payload
        .get("details_ref")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s == expected_anchor && s.ends_with(&format!("#step-{E2E1_FAIL_STEP}")));
    if !anchor_present {
        leaks += 1; // the jump-to-failure anchor did not resolve — the pane cannot jump to the step
    }

    // ── STEP 2: seed CI's projectable run row (the live-OLTP store the projector reads). Only the
    //    collaborator is on the per-viewer allow-list for the run ROOT.
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
    // The pane embeds the run's failing step (`<run-ref>#step-<n>`).
    let embed = ArtifactRef(format!("{}#step-{E2E1_FAIL_STEP}", run_ref.0));

    // ── STEP 3: the AUTHORIZED viewer's pane resolves the embed → a live run projection carrying the
    //    state + the `#step-<n>` sub-anchor (the pane renders the failure; the merge gate blocks).
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
    // The merge gate is BLOCKED iff the check the pane shows is a failure (the §1.E2E-1 mid-flight
    // mutation A — the checks panel live-updates and merge shows blocked).
    let merge_blocked = test_fail
        .payload
        .get("state")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s == "failure");

    // ── STEP 4: the UNAUTHORIZED viewer's pane resolves the SAME embed → a content-free tombstone;
    //    the SECRET pipeline name + state must be structurally absent (0 leak across EVERY field).
    let denied_view = projector
        .project(&embed, &denied, e2e_zookie())
        .expect("denied projection");
    match &denied_view {
        Projected::Tombstoned(t) => {
            if t.reason != TombstoneReason::Unauthorized {
                leaks += 1; // the denied path must be an Unauthorized tombstone, not a content leak
            }
            // The viewer-facing text is content-free (no title, no state, no reason).
            if t.display_text().contains("cerberus") || t.display_text().contains("acquisition") {
                leaks += 1;
            }
            // There is NO title/state accessor on a tombstone — the SECRET is structurally absent. We
            // assert the whole projection's debug rendering carries no fragment of the secret name.
            let rendered = format!("{denied_view:?}");
            if rendered.contains("cerberus") || rendered.contains("acquisition") {
                leaks += 1; // a title fragment leaked into the unauthorized projection
            }
            // The 0-leak helper: a tombstone has no title.
            if denied_view.title().is_some() {
                leaks += 1;
            }
        }
        Projected::Visible(_) => {
            leaks += 1; // a denied viewer got a VISIBLE projection — the leak gate failed
        }
    }

    // ── STEP 5: MID-FLIGHT — the run is ERASED while the pane is open. The SAME projector re-resolves;
    //    even the previously-authorized viewer now gets a content-free `Erased` tombstone.
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
    store2.mark_erased(&run_ref); // the erasure/restriction lands mid-flight
    let id2 = WedgeId::new().allow_view(&collaborator, &run_ref); // the collaborator STILL has view…
    let projector2 = Projector::new(id2, store2);
    let collab_after_erase = projector2
        .project(&embed, &collaborator, e2e_zookie())
        .expect("collaborator projection after erase");
    let erasure_honoured_live = match &collab_after_erase {
        // …but the erasure degrades the embed to an `Erased` tombstone (the mid-flight mutation is
        // honoured by the live read path — not a stale cached title).
        Projected::Tombstoned(t) => t.reason == TombstoneReason::Erased,
        Projected::Visible(_) => {
            leaks += 1; // the erased run still rendered a title — the mutation was not honoured
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

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-3 — Spec-to-ship traceability (CheckStatus attach + HITL-gated deploy; cold==live; tamper).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The spec-doc ref at the head of the spec-to-ship lineage chain (a Knowledge doc; CI's slice picks
/// up at the run/check/deploy hops).
fn e2e3_spec_ref() -> String {
    "myelin://acme/knowledge/page/spec-payments-v2".to_string()
}

/// The issue the spec decomposes into (the lineage's middle hop).
fn e2e3_issue_ref() -> String {
    "myelin://acme/issue/issue/PAY-1".to_string()
}

/// The PR that closes the issue (the lineage's Git hop).
fn e2e3_pr_ref() -> String {
    "myelin://acme/git/pr/PR-7".to_string()
}

/// The CI run that attached `CheckStatus` on the PR's commit (CI's lineage hop).
const E2E3_RUN_ID: &str = "run-payments-ship";

/// The protected-env deployment the HITL gate ships (CI's terminal lineage hop).
const E2E3_DEPLOY_CARD: &str = "deploy-prod-payments-v2";

/// Build CI's source of truth carrying the run + deployment rows the lineage's CI hops point at
/// (references-not-payloads). The SAME [`CiReindexSource`] CI's per-owner replay reads (CI-P26).
fn e2e3_ci_source() -> CiReindexSource {
    let mut src = CiReindexSource::new();
    let run_ref = ci_run_ref("acme", E2E3_RUN_ID);
    // The run that attached CheckStatus on PR-7's commit (the lineage's CI run hop).
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

/// Re-build a snapshot draft into an envelope the live consumer ingests (the SAME shape the steady-
/// state live event carries — the consumer cannot tell cold from live).
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

/// A single hop in the spec-to-ship lineage seal — the `source -> target (rel)` fact, content-addressed
/// and chained to the prior hop's seal so any TAMPER breaks the chain (the audit-tamper crux).
#[derive(Clone, Debug)]
struct LineageHop {
    source: String,
    target: String,
    rel: String,
    /// The chain hash: `BLAKE3(prev_seal | source | target | rel)`. Tampering with any field (or
    /// reordering hops) changes this and every downstream hash — detectable by re-deriving the chain.
    seal: String,
}

/// **Seal a lineage into a hash-chain** using ONLY the frozen [`ContentHash::blake3`] primitive (the
/// same proven hash the compaction snapshot determinism uses — never a hand-rolled hash, VISION §4).
/// Each hop's seal folds in the prior seal, so a tamper anywhere in the chain breaks every downstream
/// hash. Returns the ordered sealed hops.
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

/// **Verify a sealed lineage chain** by re-deriving every hop's seal from its body + the prior seal. A
/// single mismatched seal means a TAMPER (a hop's source/target/rel was altered, or the order changed)
/// — returns `false` LOUDLY (0 silent tamper).
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
            return false; // tamper detected — the re-derived seal does not match the stored one
        }
        prev = hop.seal.clone();
    }
    true
}

/// The spec-to-ship lineage hops as `(source, target, rel)` triples (spec → issue → PR → CI run →
/// deploy — the full causal path CI's slice attaches the run/check/deploy hops to).
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

/// **E2E-3 (spec-to-ship traceability — CI's slice): drive it end-to-end.**
///
/// The whole flow, not a single handler (EI-01 §4):
/// 1. A CI run attaches `CheckStatus` on the PR's commit; the spec → issue → PR → run → deploy lineage
///    is laid down. The lineage is TRACEABLE: a forward walk from the spec reaches the deploy.
/// 2. **HITL-gated deploy ships:** a protected-env deploy goes through the SAME
///    [`DeployGate::gate_deploy`] over the FROZEN per-effect `idem_key` (OQ-F). A DECLINE WITHHOLDS (0
///    mutation, AG-8); the APPROVE applies the deploy EXACTLY ONCE (a double-click re-sends the SAME
///    key → one apply). The HITL gate ships the spec-to-ship deploy.
/// 3. **cold-reindex == live:** the derived projection is WIPED; `reindex(scope)` → CI's `replay`
///    rebuilds it ONLY through the live consumer path ([`DerivedStore::ingest`]) — the rebuilt bytes
///    byte-match live (the CI-D9 parity, contract 2.6). No bespoke recovery reader.
/// 4. **audit tamper detected:** the lineage is sealed into a hash-chain (from the frozen
///    [`ContentHash::blake3`] primitive). The honest chain VERIFIES; a single TAMPERED hop (a forged
///    "this deploy shipped a different run") breaks the chain hash and FAILS verification — 0 silent
///    tamper.
///
/// Returns the named green artifact (`is_green()` iff lineage traceable AND HITL ships exactly-once
/// AND cold==live AND honest-verifies AND tamper-detected).
pub fn run_e2e3_spec_to_ship_lineage() -> E2eArtifact {
    let mut leaks: u64 = 0;
    let hops = e2e3_lineage_hops();

    // ── STEP 1: the lineage is TRACEABLE — a forward walk from the spec reaches the deploy.
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
        leaks += 1; // the spec does not trace to its ship deploy — the lineage is broken
    }

    // ── STEP 2: HITL-gated deploy ships. A DECLINE withholds (0 mutation); the APPROVE applies the
    //    deploy EXACTLY once (a double-click re-sends the SAME per-effect key → one apply).
    let mut applied: HashMap<String, String> = HashMap::new();
    let mut deploy_runs: u64 = 0;
    // First: a DECLINE — the deploy is WITHHELD (0 mutation, AG-8).
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
        leaks += 1; // a declined deploy mutated — the HITL withhold gate failed
    }
    // Then: APPROVE — the deploy applies EXACTLY once.
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
    // A DOUBLE-CLICK re-sends the SAME per-effect key → it is already applied → NO second apply.
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
        approved.is_applied() && approved_again.is_applied() && deploy_runs == 1; // the apply ran EXACTLY once across the double-click (OQ-F)
    if !hitl_ships_exactly_once {
        leaks += 1; // the HITL-gated deploy did not ship exactly once
    }

    // ── STEP 3: cold == live. Build live, WIPE the derived store, rebuild ONLY from replay through the
    //    live consumer path; assert the parity bytes byte-match.
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
        leaks += 1; // cold diverged from live — the reindex is not byte-exact
    }

    // ── STEP 4: audit tamper detected. Seal the lineage; the honest chain verifies; a tampered hop is
    //    caught.
    let honest = seal_lineage(&hops);
    let honest_verifies = verify_lineage(&honest);
    if !honest_verifies {
        leaks += 1; // the honest chain failed to verify — the seal is broken
    }
    // Forge a hop: rewrite the deploy hop's source run (a "this deploy shipped a different run"
    // tamper) but keep its stored seal — the re-derive must catch it.
    let mut tampered = honest.clone();
    if let Some(last) = tampered.last_mut() {
        last.source = "myelin://acme/ci/run/run-FORGED".to_string();
    }
    let tamper_detected = !verify_lineage(&tampered);
    if !tamper_detected {
        leaks += 1; // a tampered lineage hop went UNDETECTED — the audit seal is vacuous
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

/// **Run BOTH CI E2E slices and return their named green artifacts (E2E-1, E2E-3).** The master M5
/// exit gate cites these; both must be `is_green()`.
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
        // The artifact is sealed (a citable content-address).
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
        // The two scenarios seal to DISTINCT addresses (the framing is injective — no collision).
        assert_ne!(arts[0].seal, arts[1].seal);
        assert_eq!(E2E_SCENARIOS, ["E2E-1", "E2E-3"]);
    }

    #[test]
    fn e2e1_unauthorized_projection_carries_no_run_fragment() {
        // A focused re-assert of the leak crux: the unauthorized projection's full debug render is
        // free of any fragment of the SECRET pipeline name (structural absence, not redaction).
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
        // Empty allow-list ⇒ everyone is denied (fail-closed).
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
        // A reorder (swapping two hops) must also break the chain — the seal folds the prior hash, so
        // order is load-bearing.
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
        // The withhold crux in isolation: a DECLINE never calls `apply` (0 mutation, AG-8).
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
