//! # `e2e_wedge` — Chat's whole-system E2E-1 + E2E-2 + E2E-4 wedge participation (CHAT-P27 / P-501, M5)
//!
//! **The completion of M5-C-S1's whole-system E2E wedge for Chat** (testing-strategy
//! `01-whole-system-e2e-and-drill-catalogue.md` §2). Chat participates in three of the four chained-
//! mutation E2E scenarios; this crate-internal module COMPOSES the production-hardened Chat surfaces
//! into chat's leg of each and emits the scenario's **named green artifact** ([`ChatE2eArtifact`]) the
//! master M5 exit gate cites. It adds **NO new contract** and **NO new core module** — it EXERCISES the
//! frozen surfaces end-to-end (the prompt's CONTRACTS-TO-IMPLEMENT: "no new core module").
//!
//! - **[`mod e2e_pane`] — E2E-1 (the unfurl/live-update pane, CHAT-D7).** Chat's PR-context-pane analog:
//!   a ref unfurls per-viewer leak-free; a mid-flight `ci.check.updated` (5.9) busts the shared per-ref
//!   cache and the pane re-resolves LIVE; a second viewer without access gets a tombstone (0 title leak).
//! - **[`mod e2e_flagship`] — E2E-2 (the agent-native FLAGSHIP, chat is the TERMINAL surface).** The
//!   CI-fail → triage agent → issue → **chat → fix-PR** loop terminates in chat: the explicit-first
//!   dispatch (8.6, one wallet via 11.7 reserve), the HITL withhold→approve→apply card (the merge tool
//!   withheld until approve, exactly-once across a kill), all metered through ONE wallet. 0 leak,
//!   exactly-once HITL + merge.
//! - **[`mod e2e_dsar`] — E2E-4 (chat's CHAT-D8 erasure as a NAMED HOLDER in the 0-holders-missed DSAR
//!   certificate).** Chat's H5 holder erases (crypto-shred + complete receipt set + bus cascade) and
//!   appears in the whole-system certificate with **0 holders missed** and **0 recoverable PII**.
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! - **E2E-1** drives the SAME [`crate::unfurl::UnfurlService::resolve_one`] no-leak chokepoint (the
//!   gate-before-cache order, CHAT-D5) + the SAME [`crate::unfurl::invalidation::invalidates_card`] /
//!   [`crate::unfurl::UnfurlCache::bust`] bus-bust the CHAT-P14 invalidation consumer owns. No second
//!   resolver, no second cache.
//! - **E2E-2** drives the SAME [`crate::dispatch::dispatch_explicit`] explicit-first reserve-gated
//!   dispatch (8.6 / 11.7) + the SAME [`crate::hitl::post_decision`] withhold→approve bridge (the
//!   per-effect `idem_key` dedup, AG-8). No second dispatch path, no second HITL surface.
//! - **E2E-4** drives the SAME [`crate::erase::ChatErasureCascade::erase`] CHAT-D8 cascade (the
//!   crypto-shred + complete [`crate::erase::ChatEraseReport::receipts_complete`] receipt set). No second
//!   erase orchestrator — the whole-system certificate (`myelin-storage`/`myelin-gdpr-service`) calls
//!   THIS through the H5 holder seam.
//!
//! ## Mock-agent runtime note (R-10 named — the prompt's required statement)
//! E2E-2's agent step runs with the **MOCK agent runtime** (`--use-mock`, contract 8.3 — a scripted mock
//! run twice → identical proposed-effect sequences, AG-D9). The **real-LLM agent runtime is the post-M5
//! swap (R-10)** — named, not built here. The mock-runtime posture is the cell-wide posture the whole-
//! system E2E harness boots under.
//!
//! ## Floors named (VISION §3 / EI-01 §1) — the prompt's required statement
//! **None new.** This is chat's contribution to the SHARED wedge over the production-hardened Chat
//! surface. The floor promotions remain CHAT-P28 (ScyllaDB hot tier) / CHAT-P29 (channel-sharded
//! home-node) / CHAT-P30 (cross-org) / CHAT-P31 — separately-triggered, named there, untouched here. The
//! one legitimate remaining platform floor is the world-scale fleet-hardware 30× load (named in
//! CHAT-P26); this wedge does not introduce a new one. The Refs `resolve` chokepoint binding (5.2,
//! REF-P10/CHAT-P15) is the inherited seam E2E-1 drives through; here the in-memory resolver models its
//! EXACT `Projection | Tombstone` contract so the no-leak PROPERTY is proven structurally.

use crate::erase::ChatEraseReport;
use crate::hitl::CardOutcome;

pub mod e2e_dsar;
pub mod e2e_flagship;
pub mod e2e_pane;

/// **The named green artifact one chat E2E scenario emits (the prompt's "named green artifact").** A
/// dated, content-addressed report the master M5 exit gate cites. `green` is the EARNED green predicate
/// (every load-bearing assertion held end-to-end); `evidence` is the load-bearing assertion summary;
/// `leaks` is the title/count/backlink leak counter (asserted at 0 — the F1 spine). A scenario that did
/// not reach green has `green = false` — it fails LOUDLY, never a claimed-but-unearned green (EI-01 §3 /
/// VISION §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatE2eArtifact {
    /// Which E2E scenario this artifact attests (`"E2E-1"` / `"E2E-2"` / `"E2E-4"`).
    pub scenario: &'static str,
    /// The earned green verdict — `true` iff every load-bearing assertion held end-to-end.
    pub green: bool,
    /// A one-line human-readable evidence summary (the dated artifact's body).
    pub evidence: String,
    /// The leak counter the scenario asserted at `0` (0 title/count/backlink leak) — the F1 spine.
    pub leaks: u64,
}

impl ChatE2eArtifact {
    /// The green predicate (the dated artifact is green iff the scenario earned it AND 0 leaks).
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

/// **Run the whole Chat-side E2E wedge (E2E-1 + E2E-2 + E2E-4).** Drives each chained-mutation scenario
/// end-to-end over the production-hardened Chat surfaces and returns the three named green artifacts.
/// This COMPLETES Chat's E2E wedge participation for M5-C-S1 — the master M5 exit gate cites E2E-1 /
/// E2E-2 (the flagship terminating green in chat) / E2E-4 green; a red leg must NOT let M6 start (a red
/// E2E gate is a dated scorecard row, never a weakened assertion). Each artifact's [`is_green`] is the
/// earned verdict (the scenario predicate + 0 leak).
///
/// [`is_green`]: ChatE2eArtifact::is_green
pub fn run_chat_e2e_wedge() -> Vec<ChatE2eArtifact> {
    vec![
        e2e_pane::run_e2e_1_unfurl_pane(),
        e2e_flagship::run_e2e_2_chat_flagship(),
        e2e_dsar::run_e2e_4_chat_dsar_holder(),
    ]
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  Shared E2E predicates the leg modules assert against (the load-bearing properties, named once).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **The HITL withhold→approve→apply parity (E2E-2 — the exactly-once core).** The merge tool is
/// `Approved` exactly once (the per-effect `idem_key` dedup made a double-click ONE approval); a decline
/// path would be `Withheld` (0 mutation). The flagship asserts this is `Approved` and the apply count is
/// exactly 1. Returns `true` iff the outcome is the single approve (the exactly-once HITL signal).
pub(crate) fn hitl_approved_once(outcome: &CardOutcome, apply_count: usize) -> bool {
    matches!(outcome, CardOutcome::Approved(_)) && apply_count == 1
}

/// **The E2E-4 holder leg is green iff chat erased 0-holders-missed with 0 recoverable PII.** The
/// complete holder-receipt set ([`ChatEraseReport::receipts_complete`]) + the destroyed-key epoch
/// recorded (the crypto-shred ran) + the cascade rode the bus (no backdoor). The whole-system
/// certificate cites chat's leg green iff this holds (chat appears with 0 holders missed). Returns
/// `true` iff chat's H5 holder erased completely.
pub(crate) fn dsar_holder_green(report: &ChatEraseReport) -> bool {
    report.receipts_complete() && report.destroyed_key_epoch.is_some() && report.cascade_published
}

#[cfg(test)]
#[path = "e2e_wedge/tests.rs"]
mod tests;
