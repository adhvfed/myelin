//! # Chat's whole-system E2E wedge participation — E2E-1 + E2E-2 + E2E-4 (CHAT-P27 / P-501, M5)
//!
//! The crate-boundary assertion that chat's three E2E legs are green — the master M5 exit gate cites
//! E2E-1 (the unfurl/live-update pane), E2E-2 (the agent-native flagship terminating in chat), and E2E-4
//! (chat's CHAT-D8 erasure named in the 0-holders-missed DSAR certificate). This drives the PUBLIC
//! [`run_chat_e2e_wedge`] surface (the wedge harness's entry into chat's contribution) so a red leg is
//! visible at the boundary the whole-system harness composes — a red E2E gate is a dated scorecard row,
//! never a weakened assertion (a red here must NOT let M6 start).
//!
//! ## Contracts exercised (CONSUMED — chat's E2E legs, no local divergence)
//! - **1.8** the telemetry survival-signal set the E2E scenarios assert against (the leak counter at 0,
//!   the exactly-once HITL signal, the holder-coverage receipt set — the load-bearing assertions each
//!   artifact carries).
//! - **11.7** reserve/settle — metered through ONE wallet in E2E-2 (the explicit-first dispatch reserves
//!   at dispatch; the exhausted-wallet variant refuses-start; the reservation balances).
//! - **5.9** `ci.check.updated` — the E2E-1 unfurl bust (the frozen CheckStatus event busts the shared
//!   per-ref cache → the pane re-resolves live within the freshness budget).
//!
//! ## Mock-agent runtime (R-10 named)
//! E2E-2's agent step runs under the MOCK agent runtime (`--use-mock`, 8.3 — deterministic, AG-D9). The
//! real-LLM runtime is the post-M5 swap (R-10), named not built. Floors: none new (CHAT-P27 is chat's
//! contribution to the SHARED wedge; the floor promotions remain CHAT-P28/P29/P30/P31).

use myelin_chat::{run_chat_e2e_wedge, ChatE2eArtifact};

/// **The whole chat E2E wedge is green (E2E-1 + E2E-2 + E2E-4), 0 leak.** The three named artifacts the
/// master M5 exit gate cites.
#[test]
fn chat_e2e_wedge_three_legs_green() {
    let arts: Vec<ChatE2eArtifact> = run_chat_e2e_wedge();
    assert_eq!(arts.len(), 3, "E2E-1 + E2E-2 + E2E-4 — chat's three legs");

    let scenarios: Vec<&str> = arts.iter().map(|a| a.scenario).collect();
    assert_eq!(
        scenarios,
        vec!["E2E-1", "E2E-2", "E2E-4"],
        "chat crosses E2E-1/E2E-2/E2E-4"
    );

    for art in &arts {
        assert!(
            art.is_green(),
            "{} is green for chat's surface: {}",
            art.scenario,
            art.evidence
        );
        assert_eq!(
            art.leaks, 0,
            "{} 0 title/count/backlink leak (incl. 0 recoverable PII for E2E-4)",
            art.scenario
        );
    }
}

/// **E2E-2 is the FLAGSHIP and it terminates GREEN in chat (chat is the terminal surface).** The
/// exactly-once HITL + merge + 0 leak signals the prompt's E2E-2 gate names.
#[test]
fn chat_e2e_2_flagship_terminates_green_in_chat() {
    let arts = run_chat_e2e_wedge();
    let flagship = arts
        .iter()
        .find(|a| a.scenario == "E2E-2")
        .expect("the flagship leg exists");
    assert!(
        flagship.is_green(),
        "the flagship terminates green in chat (exactly-once HITL + merge, 0 leak): {}",
        flagship.evidence
    );
    // The evidence carries the load-bearing exactly-once + one-wallet signals (1.8 survival signals).
    assert!(flagship.evidence.contains("merge_applied_once=true"));
    assert!(flagship
        .evidence
        .contains("dispatched_through_one_wallet=true"));
    assert!(flagship.evidence.contains("double_click_deduped=true"));
}

/// **E2E-4: chat appears in the 0-holders-missed certificate with 0 holders missed + 0 recoverable
/// PII.** Chat's H5 holder leg is green within the whole-system DSAR fan-out.
#[test]
fn chat_e2e_4_dsar_holder_zero_holders_missed_zero_recoverable() {
    let arts = run_chat_e2e_wedge();
    let dsar = arts
        .iter()
        .find(|a| a.scenario == "E2E-4")
        .expect("the DSAR holder leg exists");
    assert!(
        dsar.is_green(),
        "chat's H5 holder is green: {}",
        dsar.evidence
    );
    assert_eq!(dsar.leaks, 0, "0 recoverable PII (hot + cold + backups)");
    assert!(dsar.evidence.contains("0 holders missed"));
}
