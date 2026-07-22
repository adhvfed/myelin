//! # `e2e_flagship` — CI's slice of the E2E-2 agent-native FLAGSHIP (CI-P34 / P-494, M5)
//!
//! **CI's slice of the whole-system E2E-2 flagship** — *CI-fail → triage agent → issue → chat →
//! fix-PR* (testing-strategy `01-whole-system-e2e-and-drill-catalogue.md` §1.E2E-2 + §2.5; VISION §1
//! — agents are first-class, work flows between tools). The **Agent-Fabric leg** (the real plan loop,
//! the plan-then-apply pipeline, the HITL withhold→approve→apply ledger, the per-run re-mint) is owned
//! by **AG-P24 / P-480** (`myelin-agent-service/tests/drills_ag_p24_e2e2_flagship.rs`); the durable
//! park/resume SPINE is **`myelin-flow`'s P-477**. **THIS module owns CI's SLICE** — the part the CI
//! subsystem is responsible for in the joint flagship (the CI-P34 prompt scope, arch 02 §4):
//!
//! 1. **The STRUCTURED `ci.run.failed` triage hook** — the deliberate agent-native input (arch §3.1 /
//!    §4): a failing run emits `ci.run.failed` carrying *which stage, which step, which test, a
//!    log-excerpt ref* ([`crate::ci_pipeline::structured_failure`]) — references-not-payloads, never
//!    log bytes. The (mock) triage agent reads THIS to file a precise issue, not the firehose.
//! 2. **The AG-D4-gated runner the triage agent's compute runs on** (X-6 / contract 8.4): the agent's
//!    `ToolHands::exec` IS a `JobSpec{ kind: Agent, .. }` launched on CI's UNIFIED runner under the
//!    SAME mandatory hardening the sandbox-escape gate (AG-D4) proves — the agent compute is no less
//!    sandboxed than untrusted CI code. We assert the agent job derives a fully-enforced
//!    [`HardeningProfile`] (egress default-deny, no metadata/control-plane reach, caps dropped, …).
//! 3. **The check seam end-to-end (5.9)** — the fix-PR's CI run greens: CI emits `ci.check.updated`
//!    (state=success) on the fix commit via the FROZEN [`assemble_check_status`] producer.
//! 4. **The `ci.result` merge wake (9.4)** — CI rolls up the fix-PR's green run into the `ci.result`
//!    SIGNAL via the FROZEN [`CiResultSignal`]; the merge-queue workflow wakes EXACTLY ONCE on the
//!    `idem_token` (an at-least-once double-delivery is absorbed by the `wf_signal` PK → merge-count
//!    == 1, 0 double-merge).
//! 5. **reserve/settle balanced (11.7)** — CI's run reserves at dispatch against the ONE wallet and
//!    settles its resource-seconds on completion; reserved == billed + refunded, one cost event per
//!    metered unit, 0 in-flight interrupt.
//!
//! Each is driven **end-to-end** — the whole CI side of the flow with the mid-flight mutation (the
//! at-least-once duplicate `ci.result`), NOT a single handler (EI-01 §4 / VISION §3). The engine /
//! producer seams are **UNCHANGED**; this module COMPOSES CI's frozen producer side into the flagship
//! and emits the scenario's named green [`E2eArtifact`].
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! - The structured failure is the FROZEN [`crate::ci_pipeline::structured_failure`] (the SAME hook the
//!   `ci.pipeline` body emits on `PipelineOutcome::Failed`) — no second failure language.
//! - The check seam is the FROZEN [`assemble_check_status`] / [`details_ref`] producer (CI-P18) — the
//!   SAME `ci.check.updated` shape Git's gate consumes (0 drift).
//! - The merge wake is the FROZEN [`CiResultSignal`] over the FROZEN `myelin_flow::SignalStore` (CI-P19)
//!   — the SAME `ci.result` rollup the merge-queue workflow parks on; idempotent on the `idem_token`.
//! - The AG-D4 gate is the FROZEN [`HardeningProfile::derive`] / [`HardeningProfile::assert_enforced`]
//!   over a `JobSpec{ kind: Agent }` (CI-P2/CI-P5) — the SAME profile the escape drill proves; no
//!   second hardening posture.
//! - The reserve/settle is the FROZEN `myelin_storage::reserve_settle::CostLedger` (11.7) — the SAME
//!   wallet ledger CI's metering settles against; no second bookend.
//!
//! ## FLOOR named (VISION §3 / EI-01 §1)
//! - **This is CI's SLICE of the joint flagship** — the FULL E2E-2 green requires every subsystem's
//!   slice (Agent, Workflow, Issues, Chat, Git, Identity, Notif). CI's slice (the structured failure
//!   hook + the AG-D4-gated runner + the check seam + the merge wake + the reserve/settle balance) is
//!   the deliverable here; the cross-subsystem orchestration is the whole-system M5 wedge. The
//!   **Agent-Fabric leg** (the plan loop, the HITL ledger, the re-mint) is **AG-P24 / P-480**; the
//!   durable park/resume **spine** is **`myelin-flow`'s P-477** — NOT duplicated here.
//! - The flagship runs on the **MOCK runtime** (VISION §3 — mock agents during development); the real
//!   `LlmAgentRuntime` swap is **AG-P25 (post-M5)**, gated on the safety drills this E2E proves.
//! - **None new in CI's slice.** The ONE legitimate remaining floor is the world-scale 30× fleet-
//!   hardware load drill (CI-P30 / [`crate::surge`]) — this scenario is a MODERATE single-cell run.

use std::collections::BTreeMap;

use myelin_ci_sandbox::hardening::HardeningProfile;
use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ResourceLimits,
    RunTokenCredential, TrustTier, WorkspaceSpec,
};
use myelin_flow::SignalStore;
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MinorUnits, RunId as LedgerRunId};
use myelin_tenancy::{Region, TenantId};

use crate::check_emitter::{
    assemble_check_status, details_ref, CheckEmitContext, CheckProvider, CheckState, CostPosture,
    TrustTier as CheckTrustTier,
};
use crate::ci_pipeline::structured_failure;
use crate::ci_result_signal::{CiResultSignal, RollupDelivery};
use crate::e2e_wedge::E2eArtifact;

/// The E2E scenario this module owns (CI's slice of the agent-native flagship). PII-free token — the
/// drills assert against the NAME, never a literal (EI-01 §3).
pub const E2E_FLAGSHIP_SCENARIO: &str = "E2E-2";

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  The scenario fixtures (a full cell with mock agents; the CI-fail → fix-PR flow's CI hops).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The tenant the flagship runs against (a full cell). Opaque, PII-free.
fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

/// The region (fr-par — the dev/prod residency pin; a config swap, never a code change).
fn region() -> Region {
    Region("fr-par".into())
}

/// The repo the failing-then-fixed run runs against.
const REPO: &str = "myelin://acme/git/repo/payments";

/// The commit that FAILED CI (the push that woke the triage agent).
const FAIL_COMMIT: &str = "f00dcafe";

/// The fix-PR's commit that GREENS CI (the agent's fix-PR, re-run after approval+merge).
const FIX_COMMIT: &str = "900dbeef";

/// The failing CI run ref.
const FAIL_RUN: &str = "myelin://acme/ci/run/run-payments-fail";

/// The fix-PR's CI run ref (the green run).
const FIX_RUN: &str = "myelin://acme/ci/run/run-payments-fix";

/// The required check context the merge gate keys on.
const CONTEXT: &str = "build-and-test";

/// The failing stage / step / test the structured triage hook carries (which step, which test).
const FAILED_STAGE: &str = "test";
const FAILED_STEP: u32 = 3;
const FAILED_TEST: &str = "myelin_payments::charge::test_refund_idempotent";

/// The merge-attempt idem token the merge queue minted (the no-coordination dedup key the `ci.result`
/// rollup echoes — a double-delivery wakes the merge queue ONCE, OQ-F).
const MERGE_IDEM_TOKEN: &str = "merge-attempt:payments:42";

/// The merge-queue run id the `ci.result` signal is buffered for (the run the merge-queue durable
/// workflow drives — CI delivers INTO its wait, never merges itself).
const MERGE_QUEUE_RUN: &str = "run:merge-queue:payments";

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  STEP 2 fixture — the AG-D4-gated runner the triage agent's compute runs on (kind=Agent, 8.4 / X-6).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// Build the `JobSpec{ kind: Agent }` the triage agent's compute (`ToolHands::exec`) runs as on CI's
/// UNIFIED runner (X-6 / contract 8.4). The SAME digest-pinned, hardened, default-deny-egress spec
/// untrusted CI code runs as — the agent's compute is no less sandboxed than a fork PR's build. The
/// egress allowlist is EMPTY (the triage agent reads via the in-boundary tools, not raw network) so
/// the guest gets no NIC at all (the strongest default-deny).
fn triage_agent_job() -> JobSpec {
    JobSpec::new(
        JobKind::Agent,
        ImageRef::pinned(
            "registry.myelin.internal/agent-runtime@sha256:\
             0000000000000000000000000000000000000000000000000000000000000001",
        )
        .expect("the agent runtime image is digest-pinned (CI-1, fail-closed)"),
        vec![
            "/usr/bin/triage-agent".to_string(),
            "--use-mock".to_string(),
        ],
        vec![],
        vec![],
        // Default-deny egress, EMPTY allowlist → no NIC (the agent reads via in-boundary tools).
        EgressPolicy { allow: vec![] },
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 512 * 1024 * 1024,
            disk_bytes: 256 * 1024 * 1024,
            pids_max: 256,
            timeout_secs: 300,
        },
        WorkspaceSpec::default(),
        // The agent's compute is a trusted member-driven triage run (a member push triggered the CI
        // fail) — but the hardening posture is IDENTICAL regardless of tier (the profile is fixed).
        TrustTier::Trusted,
        RunTokenCredential::new("triage-agent-bearer", "jti:triage-agent:run-triage", 300)
            .expect("static flagship credential is valid"),
        MeterTarget {
            reserve_id: "reserve:run-triage".to_string(),
        },
        IdemToken("idem:triage-agent:run-triage".to_string()),
    )
    .expect("the triage agent job is a valid hardened JobSpec (digest-pinned, pids_max, timeout)")
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  The flagship — CI's slice driven end-to-end.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **CI's slice of E2E-2 (the agent-native flagship): drive it end-to-end, chaining the mid-flight
/// at-least-once duplicate.**
///
/// The whole CI side of the flow, not a single handler (EI-01 §4):
/// 1. **CI fails with a STRUCTURED `ci.run.failed`** — the triage hook carries which stage, which step,
///    which test, a log-excerpt ref ([`structured_failure`]). The (mock) triage agent reads THESE
///    keys to file a precise issue. PII-free (machine tokens + an `ArtifactRef`, never log bytes); the
///    `#step-<n>` jump-to-failure anchor resolves through the log index ([`details_ref`]).
/// 2. **The triage agent's compute runs AG-D4-gated** — its `ToolHands::exec` is a `JobSpec{ kind:
///    Agent }` on CI's unified runner; the derived [`HardeningProfile`] is fully enforced (egress
///    default-deny, no metadata/control-plane reach, caps dropped, no-new-privs, seccomp, pids ceiling,
///    one-job-ephemeral). The agent compute is no less sandboxed than untrusted CI code (X-6).
/// 3. **The fix-PR's CI greens** — CI emits `ci.check.updated` (state=success) on the fix commit via
///    the FROZEN [`assemble_check_status`] producer (the SAME shape Git's gate consumes).
/// 4. **The merge-queue wakes EXACTLY ONCE on `ci.result`** — CI rolls up the green run into the
///    `ci.result` signal ([`CiResultSignal`]); the FIRST delivery WAKES the merge queue, an
///    at-least-once DUPLICATE (the mid-flight mutation) is ABSORBED by the `wf_signal` PK → merge-count
///    == 1 (0 double-merge).
/// 5. **reserve/settle balanced** — CI's run reserves at dispatch and settles on completion; reserved
///    == billed + refunded, one cost event per metered unit, 0 in-flight interrupt.
///
/// Returns the named green artifact (`is_green()` iff the triage hook is structured AND the runner is
/// AG-D4-gated AND the fix greens AND the merge wakes exactly once AND reserve/settle balances).
pub fn run_e2e2_ci_flagship_slice() -> E2eArtifact {
    let mut leaks: u64 = 0;

    // ── STEP 1: the STRUCTURED ci.run.failed triage hook (which stage / step / test / log excerpt). ──
    // The log excerpt is a REFERENCE into CI's log tier at the failing step — never inline bytes.
    let log_excerpt_ref = format!("{FAIL_RUN}#log/step-{FAILED_STEP}");
    let failure = structured_failure(
        FAILED_STAGE,
        Some(FAILED_STEP),
        Some(FAILED_TEST),
        Some(&log_excerpt_ref),
    );
    let failure_payload = failure.to_payload();
    // The triage agent reads the structured keys (which stage, which step, which test, the log ref) —
    // assert each is present and PII-free (a machine token / an ArtifactRef, never a free-text body).
    let triage_hook_structured = failure_payload.get("failed_stage").and_then(|v| v.as_str())
        == Some(FAILED_STAGE)
        && failure_payload.get("failed_step").and_then(|v| v.as_u64()) == Some(FAILED_STEP as u64)
        && failure_payload.get("failed_test").and_then(|v| v.as_str()) == Some(FAILED_TEST)
        && failure_payload
            .get("log_excerpt_ref")
            .and_then(|v| v.as_str())
            == Some(log_excerpt_ref.as_str());
    if !triage_hook_structured {
        leaks += 1; // the triage hook is not structured — the agent cannot triage precisely
    }
    // The failing run also emits `ci.check.updated{failure}` on the FAIL commit (5.9) — the producer
    // fact the PR-checks panel shows + the agent triages alongside the structured failure. It carries
    // the `#step-<n>` jump-to-failure anchor (references-not-payloads: an ArtifactRef sub-anchor into
    // the log tier, NOT log bytes). Build it through the FROZEN producer (no second emit path).
    let fail_ctx = CheckEmitContext {
        tenant: tenant().0,
        repo: REPO.to_string(),
        commit_oid: FAIL_COMMIT.to_string(),
        run_ref: FAIL_RUN.to_string(),
        run_attempt: 1,
        trust_tier: CheckTrustTier::Trusted,
        started_at: "2026-06-25T00:00:00Z".to_string(),
        completed_at: Some("2026-06-25T00:01:00Z".to_string()),
    };
    let fail_check = assemble_check_status(
        &fail_ctx,
        CheckProvider::Ci,
        CONTEXT,
        CheckState::Failure,
        true,
        CostPosture::Settled,
        Some(FAILED_STEP),
    );
    let expected_anchor = details_ref(FAIL_RUN, CheckState::Failure, Some(FAILED_STEP));
    let fail_check_state =
        fail_check.payload.get("state").and_then(|v| v.as_str()) == Some("failure");
    let anchor_resolves =
        expected_anchor.ends_with(&format!("#step-{FAILED_STEP}")) && fail_check_state;
    if !anchor_resolves {
        leaks += 1; // the failing check / jump-to-failure anchor did not resolve
    }
    // The structured failure carries NO inline log bytes (the bus stays small + leak-free, ADR-04.5).
    let no_log_bytes_on_bus = failure_payload
        .get("log_excerpt_ref")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.starts_with("myelin://") && !s.contains('\n'));
    if !no_log_bytes_on_bus {
        leaks += 1; // a log body leaked onto the bus event (references-not-payloads breached)
    }

    // ── STEP 2: the triage agent's compute runs on CI's AG-D4-GATED runner (kind=Agent, 8.4 / X-6). ──
    let agent_job = triage_agent_job();
    let runner_is_agent_kind = agent_job.kind == JobKind::Agent;
    let profile = HardeningProfile::derive(&agent_job);
    let ag_d4_gated = profile.assert_enforced().is_ok()
        && profile.egress_default_deny
        && !profile.network_device // empty allowlist → no NIC (the strongest default-deny)
        && profile.drop_all_caps
        && profile.no_new_privileges
        && profile.seccomp
        && profile.read_only_root
        && profile.ephemeral_one_job
        && profile.pids_max > 0;
    if !runner_is_agent_kind || !ag_d4_gated {
        leaks += 1; // the agent compute is not AG-D4-gated — it could escape the sandbox
    }

    // ── STEP 3: the fix-PR's CI GREENS — ci.check.updated{success} on the fix commit (5.9). ──────────
    let fix_ctx = CheckEmitContext {
        tenant: tenant().0,
        repo: REPO.to_string(),
        commit_oid: FIX_COMMIT.to_string(),
        run_ref: FIX_RUN.to_string(),
        run_attempt: 2, // the fix-PR re-run bumps the monotonic attempt (Git's last-writer-wins key).
        trust_tier: CheckTrustTier::Trusted,
        started_at: "2026-06-25T00:00:00Z".to_string(),
        completed_at: Some("2026-06-25T00:02:00Z".to_string()),
    };
    let green_check = assemble_check_status(
        &fix_ctx,
        CheckProvider::Ci,
        CONTEXT,
        CheckState::Success,
        true,
        CostPosture::Settled,
        None,
    );
    let fix_greens = green_check
        .payload
        .get("state")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s == "success");
    if !fix_greens {
        leaks += 1; // the fix-PR's CI did not green — the merge gate would never open
    }

    // ── STEP 4: the merge-queue wakes EXACTLY ONCE on ci.result (9.4 / X-1). The mid-flight mutation
    //    is an at-least-once DUPLICATE delivery — it MUST be absorbed (merge-count == 1, 0 double-merge).
    let signals = SignalStore::new();
    let rollup = CiResultSignal::new(&signals, tenant(), region(), MERGE_QUEUE_RUN);
    // The fix run's per-context verdict: the required context greened.
    let current: BTreeMap<String, bool> = [(CONTEXT.to_string(), true)].into_iter().collect();
    let required = vec![CONTEXT.to_string()];
    // FIRST delivery — the merge-queue workflow WAKES.
    let first = rollup.signal_ci_result(FIX_COMMIT, &current, &required, MERGE_IDEM_TOKEN);
    // MID-FLIGHT: the at-least-once bus re-delivers "ci.result" (the runner delivered done twice). The
    // SAME idem_token → the wf_signal PK ABSORBS it (one buffered row → one wake, never a second merge).
    let duplicate = rollup.signal_ci_result(FIX_COMMIT, &current, &required, MERGE_IDEM_TOKEN);
    let merge_wakes_exactly_once =
        first == RollupDelivery::Woke && duplicate == RollupDelivery::Duplicate;
    if !merge_wakes_exactly_once {
        leaks += 1; // the merge queue woke twice (or not at all) — a double-merge risk
    }
    // The rollup verdict is `success` (the merge queue merges iff the rollup is green).
    let rollup_verdict = rollup.rollup(FIX_COMMIT, &current, &required, MERGE_IDEM_TOKEN);
    let rollup_is_success = CiResultSignal::is_success(&rollup_verdict);
    if !rollup_is_success {
        leaks += 1; // the green run did not roll up to success — the merge would not proceed
    }
    // merge-count == 1: exactly ONE buffered ci.result signal under the idem_token (one wake → one
    // merge). The duplicate added NO second buffered row.
    let merge_count = u64::from(
        signals
            .get(
                &tenant(),
                MERGE_QUEUE_RUN,
                myelin_flow::CI_RESULT_SIGNAL,
                MERGE_IDEM_TOKEN,
            )
            .is_some(),
    );
    if merge_count != 1 {
        leaks += 1; // merge-count != 1 — the exactly-once merge invariant broke
    }

    // ── STEP 5: reserve/settle BALANCED (11.7). The CI run reserves its estimate at dispatch against
    //    the ONE wallet, begins flight, and settles its resource-seconds on completion; reserved ==
    //    billed + refunded, one cost event per metered unit, 0 in-flight interrupt.
    let mut ledger = CostLedger::new();
    let run = LedgerRunId::new(FIX_RUN);
    const ESTIMATE: u64 = 20;
    const WALLET: u64 = 100;
    let reservation = ledger
        .reserve(
            tenant(),
            run.clone(),
            MinorUnits(ESTIMATE),
            MinorUnits(WALLET),
        )
        .expect("a funded wallet reserves the CI run at dispatch (no balance → no run)");
    let reserved_estimate = reservation.reserved == MinorUnits(ESTIMATE);
    ledger
        .begin(&tenant(), &run)
        .expect("the reserved run begins flight (the reservation's only exit is settle)");
    // The run's metered resource-seconds (the actual bill: cpu + a build artifact unit < the estimate).
    let units = vec![
        MeteredUnit {
            unit: "ci.cpu_second",
            wholesale: MinorUnits(8),
            markup: MinorUnits(2),
        },
        MeteredUnit {
            unit: "ci.artifact_byte",
            wholesale: MinorUnits(3),
            markup: MinorUnits(1),
        },
    ];
    let settle = ledger
        .settle(&tenant(), &run, &units)
        .expect("the in-flight CI run settles on completion");
    let billed = settle.billed_total.0;
    let refunded = settle.refunded.0;
    // billed == cpu(10) + artifact(4) == 14; reserved == 20; refund == 6 → reserved == billed+refunded.
    let reserve_settle_balanced = reserved_estimate
        && billed == 14
        && billed + refunded == ESTIMATE
        && ledger.cost_events_for(&tenant(), &run).len() == units.len() // one cost event per metered unit
        && ledger.inflight_interrupt_count() == 0; // never interrupts in-flight (11.7)
    if !reserve_settle_balanced {
        leaks += 1; // the wallet did not conserve — reserve/settle is not balanced
    }

    let green = triage_hook_structured
        && anchor_resolves
        && no_log_bytes_on_bus
        && runner_is_agent_kind
        && ag_d4_gated
        && fix_greens
        && merge_wakes_exactly_once
        && rollup_is_success
        && merge_count == 1
        && reserve_settle_balanced;
    E2eArtifact::sealed(
        E2E_FLAGSHIP_SCENARIO,
        green,
        leaks,
        format!(
            "CI-fail→triage→issue→chat→fix-PR (CI slice): structured ci.run.failed \
             (stage={FAILED_STAGE},step={FAILED_STEP},test={FAILED_TEST},log-ref) \
             structured={triage_hook_structured}; triage-agent compute AG-D4-gated \
             (kind=Agent,no-NIC,caps-dropped,seccomp)={ag_d4_gated}; fix-PR CI greens={fix_greens}; \
             merge-queue wakes EXACTLY ONCE on ci.result (dup absorbed)={merge_wakes_exactly_once}, \
             merge-count={merge_count}; reserve/settle balanced (reserved {ESTIMATE} == billed \
             {billed} + refunded {refunded})={reserve_settle_balanced}"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **CI-P34 / E2E-2 (CI's slice) — GREEN end-to-end.** Every load-bearing property holds: the
    /// structured triage hook, the AG-D4-gated runner, the green fix check, the exactly-once merge
    /// wake (merge-count == 1), and the balanced reserve/settle.
    #[test]
    fn e2e2_ci_flagship_slice_green_end_to_end() {
        let art = run_e2e2_ci_flagship_slice();
        assert_eq!(art.scenario, "E2E-2");
        assert_eq!(
            art.leaks, 0,
            "0 leak/double-merge across CI's flagship slice: {art:?}"
        );
        assert!(art.is_green(), "E2E-2 (CI slice) green not earned: {art:?}");
        // The artifact is sealed (a citable content-address the master M5 exit gate cites by hash).
        assert!(art.seal.starts_with("blake3:"));
    }

    /// The triage hook is STRUCTURED (which stage / step / test / log-excerpt ref) — the deliberate
    /// agent-native input (arch §4). A bare stage-only hook would NOT let the agent file a precise
    /// issue. references-not-payloads: the log excerpt is an `ArtifactRef`, never inline bytes.
    #[test]
    fn structured_ci_run_failed_carries_step_test_and_log_ref() {
        let log_ref = format!("{FAIL_RUN}#log/step-{FAILED_STEP}");
        let failure = structured_failure(
            FAILED_STAGE,
            Some(FAILED_STEP),
            Some(FAILED_TEST),
            Some(&log_ref),
        );
        let p = failure.to_payload();
        assert_eq!(
            p.get("failed_stage").and_then(|v| v.as_str()),
            Some(FAILED_STAGE)
        );
        assert_eq!(
            p.get("failed_step").and_then(|v| v.as_u64()),
            Some(FAILED_STEP as u64)
        );
        assert_eq!(
            p.get("failed_test").and_then(|v| v.as_str()),
            Some(FAILED_TEST)
        );
        assert_eq!(
            p.get("log_excerpt_ref").and_then(|v| v.as_str()),
            Some(log_ref.as_str())
        );
        // references-not-payloads: a POINTER into the log tier, never the bytes (no newline body).
        assert!(log_ref.starts_with("myelin://") && !log_ref.contains('\n'));
        // A bare stage-only hook is the floor (absent detail is an absent key, never a `null`).
        let bare = structured_failure(FAILED_STAGE, None, None, None).to_payload();
        assert_eq!(bare.as_object().map(|o| o.len()), Some(1));
        assert!(
            bare.get("failed_step").is_none(),
            "absent detail is an absent key, not null"
        );
    }

    /// The triage agent's compute runs on the SAME AG-D4-gated runner as untrusted CI code: a
    /// `JobSpec{ kind: Agent }` derives a fully-enforced hardening profile (X-6 / contract 8.4).
    #[test]
    fn triage_agent_compute_is_ag_d4_gated() {
        let job = triage_agent_job();
        assert_eq!(
            job.kind,
            JobKind::Agent,
            "the agent compute is a kind=Agent job"
        );
        let profile = HardeningProfile::derive(&job);
        assert!(
            profile.assert_enforced().is_ok(),
            "the agent job's hardening profile is fully enforced: {:?}",
            profile.assert_enforced()
        );
        // Empty allowlist → NO NIC at all (the strongest default-deny; no metadata/control-plane reach).
        assert!(
            !profile.network_device,
            "the agent guest gets no NIC (egress closed at the device)"
        );
        assert!(profile.drop_all_caps && profile.no_new_privileges && profile.seccomp);
        assert!(
            profile.ephemeral_one_job,
            "one-job-per-sandbox, never reused"
        );
    }

    /// The merge-queue wakes EXACTLY ONCE on `ci.result`: the FIRST delivery wakes; an at-least-once
    /// DUPLICATE under the same `idem_token` is absorbed by the `wf_signal` PK (merge-count == 1).
    #[test]
    fn merge_queue_wakes_exactly_once_on_ci_result() {
        let signals = SignalStore::new();
        let rollup = CiResultSignal::new(&signals, tenant(), region(), MERGE_QUEUE_RUN);
        let current: BTreeMap<String, bool> = [(CONTEXT.to_string(), true)].into_iter().collect();
        let required = vec![CONTEXT.to_string()];
        let first = rollup.signal_ci_result(FIX_COMMIT, &current, &required, MERGE_IDEM_TOKEN);
        let dup = rollup.signal_ci_result(FIX_COMMIT, &current, &required, MERGE_IDEM_TOKEN);
        assert_eq!(
            first,
            RollupDelivery::Woke,
            "the first delivery wakes the merge queue"
        );
        assert_eq!(
            dup,
            RollupDelivery::Duplicate,
            "the duplicate is absorbed (0 double-merge)"
        );
        // Exactly one buffered ci.result signal under the idem_token (merge-count == 1).
        assert!(signals
            .get(
                &tenant(),
                MERGE_QUEUE_RUN,
                myelin_flow::CI_RESULT_SIGNAL,
                MERGE_IDEM_TOKEN
            )
            .is_some());
    }

    /// reserve/settle is BALANCED: reserved == billed + refunded, one cost event per metered unit,
    /// 0 in-flight interrupt (11.7). A focused re-assert of the wallet-conservation crux.
    #[test]
    fn reserve_settle_balanced_in_isolation() {
        let mut ledger = CostLedger::new();
        let run = LedgerRunId::new(FIX_RUN);
        let r = ledger
            .reserve(tenant(), run.clone(), MinorUnits(20), MinorUnits(100))
            .unwrap();
        assert_eq!(r.reserved, MinorUnits(20));
        ledger.begin(&tenant(), &run).unwrap();
        let units = vec![
            MeteredUnit {
                unit: "ci.cpu_second",
                wholesale: MinorUnits(8),
                markup: MinorUnits(2),
            },
            MeteredUnit {
                unit: "ci.artifact_byte",
                wholesale: MinorUnits(3),
                markup: MinorUnits(1),
            },
        ];
        let s = ledger.settle(&tenant(), &run, &units).unwrap();
        assert_eq!(s.billed_total.0, 14);
        assert_eq!(
            s.billed_total.0 + s.refunded.0,
            20,
            "reserved == billed + refunded"
        );
        assert_eq!(
            ledger.cost_events_for(&tenant(), &run).len(),
            2,
            "one cost event per unit"
        );
        assert_eq!(
            ledger.inflight_interrupt_count(),
            0,
            "0 in-flight interrupt"
        );
    }
}
