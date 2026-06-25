//! # CI-P34 / P-494 — CI's slice of the whole-system E2E-2 agent-native FLAGSHIP
//!
//! **Prompt:** P-494 (CI-P34, M5 · CI-M5). **Drill (catalogue 01 §1.E2E-2 + §2.5):** the agent-native
//! flagship — *CI-fail → triage agent → issue → chat → fix-PR*. **CI's slice** is the part the CI
//! subsystem owns in the joint scenario (arch 02 §4): the structured `ci.run.failed` triage hook, the
//! AG-D4-gated runner the triage agent's compute runs on, the fix-PR check seam, the `ci.result` merge
//! wake (exactly once), and the balanced reserve/settle.
//!
//! **Owning architecture doc:**
//! `04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §4 (the structured `ci.run.failed` — the deliberate triage hook) + §3.1 (the pipeline body emits it);
//! `03-events-contracts-and-glue.md` §1.2 (ci.run.failed carries structured failure);
//! `04-views-cli-and-api.md` §1 (the agent-surfaced triage). **Reconciliation:** `00-reconciliation-
//! decisions.md` X-6 (the agent compute runs on CI's AG-D4-gated runner). **Contracts exercised (none
//! new):** 5.9 (the check seam, end-to-end), 9.4 (the `ci.result` merge wake), 11.7 (reserve/settle
//! balanced), 8.4 (the AG-D4-gated runner). **Doctrine:** EI-01 §4 (drive the WHOLE CI side end to end —
//! chain the at-least-once duplicate mid-flight, not a single handler), §3 (prove-it: structured hook;
//! AG-D4-gated; merge-count == 1; reserve == billed + refunded).
//!
//! ## What this drill proves (CI's E2E-2 slice, end-to-end, against a full cell with mock agents)
//! - **Structured `ci.run.failed`** carries which stage / which step / which test / a log-excerpt ref
//!   (references-not-payloads — a machine token + an `ArtifactRef`, never log bytes) — the deliberate
//!   triage hook the (mock) triage agent reads to file a precise issue.
//! - **The triage agent's compute runs AG-D4-gated** — a `JobSpec{ kind: Agent }` on CI's unified
//!   runner derives a fully-enforced hardening profile (egress default-deny + no NIC, caps dropped,
//!   no-new-privs, seccomp, pids ceiling, one-job-ephemeral) — no less sandboxed than untrusted CI code.
//! - **The fix-PR's CI greens** (`ci.check.updated{success}`, 5.9); the **merge-queue wakes EXACTLY
//!   ONCE** on `ci.result` (an at-least-once DUPLICATE under the same `idem_token` is absorbed by the
//!   `wf_signal` PK → merge-count == 1, 0 double-merge); **reserve/settle is BALANCED** (reserved ==
//!   billed + refunded, one cost event per metered unit, 0 in-flight interrupt).
//!
//! ## FLOOR (named, per the prompt)
//! This is CI's SLICE of the JOINT flagship — the full E2E-2 green requires every subsystem's slice
//! (Agent, Workflow, Issues, Chat, Git, Identity, Notif). The **Agent-Fabric leg** (the plan loop, the
//! HITL withhold→approve→apply ledger, the per-run re-mint) is **AG-P24 / P-480**
//! (`myelin-agent-service/tests/drills_ag_p24_e2e2_flagship.rs`); the durable park/resume **spine** is
//! **`myelin-flow`'s P-477** (`crates/myelin-flow/tests/drills_flow_e2e2_spine.rs`). The flagship runs
//! on the **MOCK runtime** (the real `LlmAgentRuntime` swap is **AG-P25, post-M5**). The ONE remaining
//! load floor is the world-scale fleet-hardware 30× drill (CI-P30). No code fix landed that owes a new
//! mutation/CDC floor; CI's slice greens deterministically here.

use myelin_ci_controlplane::e2e_flagship::{run_e2e2_ci_flagship_slice, E2E_FLAGSHIP_SCENARIO};

/// **CI-P34 / E2E-2 headline (CI's slice): structured triage hook + AG-D4-gated runner + green fix +
/// exactly-once merge wake + balanced reserve/settle — GREEN end-to-end.** The dated green artifact is
/// the scenario's `is_green()` AND 0 leak/double-merge. The evidence body names every load-bearing fact.
#[test]
fn ci_p34_e2e2_ci_flagship_slice_green_end_to_end() {
    let art = run_e2e2_ci_flagship_slice();
    assert_eq!(art.scenario, "E2E-2");
    assert_eq!(E2E_FLAGSHIP_SCENARIO, "E2E-2");
    assert_eq!(
        art.leaks, 0,
        "E2E-2 (CI slice): 0 leak / 0 double-merge across the whole CI side — {}",
        art.evidence
    );
    assert!(
        art.is_green(),
        "E2E-2 (CI slice) green not earned (the dated artifact): {} [seal={}]",
        art.evidence,
        art.seal
    );
    // The artifact is a citable content-address (the master M5 exit gate cites it by hash).
    assert!(art.seal.starts_with("blake3:"), "the artifact is sealed");
    // The evidence body names the load-bearing facts (the structured hook, the gated runner, the
    // exactly-once merge wake with merge-count == 1, the balanced reserve/settle).
    assert!(art.evidence.contains("structured ci.run.failed"));
    assert!(art.evidence.contains("AG-D4-gated"));
    assert!(art.evidence.contains("fix-PR CI greens=true"));
    assert!(art.evidence.contains("EXACTLY ONCE"));
    assert!(art.evidence.contains("merge-count=1"));
    assert!(art.evidence.contains("reserve/settle balanced"));
}
