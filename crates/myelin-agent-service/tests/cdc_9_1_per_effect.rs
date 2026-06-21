//! # The consumer CDC for 9.1 (the per-effect resume `idem_key`) — the agent-fabric ↔ durable-engine
//! parity for batch / partial HITL approval (AG-P10 → P-222)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 9.1 (the
//! durable signal is idempotent on the per-effect `idem_key`; the per-effect RULE — `card_id` single,
//! `card_id:effect_idx` multi). Owning architecture: `agent-fabric.md` §5.3 **C4** (the resume
//! signal's idempotency key is per-effect) + `durable-workflow.md` §6.4 (the frozen rule + the
//! `wf_signal` PK). Drill: AG-D5 (the exactly-once leg — partial approval + double-click well-defined).
//!
//! ## What this CDC proves — the two sides produce the SAME per-effect key (parity, not duplication)
//!
//! The per-effect `idem_key` is computed on BOTH sides of the HITL loop:
//! - the **agent fabric** (`myelin_agent_service::per_effect_idem_key`, P-222) builds the key the
//!   resume signal CARRIES (the CONSUMER of 9.1);
//! - the **durable engine** (`myelin_flow::approval::per_effect_idem_key`, P-206) dedups the resume
//!   signal at the `wf_signal` PK on that key (the PROVIDER of 9.1).
//!
//! If the two derivations DISAGREED, a double-click would slip a second apply (the fabric's key would
//! miss the engine's buffered signal) or a partial approval would couple effects. This CDC pins the
//! contract: for EVERY card arity + effect index, the agent-fabric key == the durable-engine key —
//! exactly-once is true BY CONSTRUCTION, not by hope. The agent crate does NOT re-implement the
//! engine's dedup; it derives the same key the engine expects.

use myelin_agent_service::per_effect_idem_key as fabric_key;
use myelin_flow::approval::per_effect_idem_key as engine_key;

/// **PARITY (9.1): the agent-fabric per-effect key == the durable-engine per-effect key for every card
/// arity + effect index.** The single-effect degenerate case (the bare `card_id`) and the multi-effect
/// case (`card_id:idx`) both agree — so the resume signal the fabric builds hits the exact `wf_signal`
/// row the engine dedups (a double-click is one approval BY CONSTRUCTION).
#[test]
fn cdc_9_1_fabric_and_engine_per_effect_keys_agree() {
    let card_id = "card-7";
    // single-effect card → the bare card id (the degenerate per-effect case, §6.4).
    assert_eq!(
        fabric_key(card_id, 0, 1),
        engine_key(card_id, 0, 1),
        "single-effect: the agent fabric and the durable engine derive the SAME key (the bare card id)"
    );
    assert_eq!(fabric_key(card_id, 0, 1), "card-7");

    // multi-effect card → card_id:effect_idx, agreeing for EVERY effect index.
    for total in 2..=6usize {
        for idx in 0..total {
            assert_eq!(
                fabric_key(card_id, idx, total),
                engine_key(card_id, idx, total),
                "multi-effect (idx {idx} of {total}): the fabric and engine keys MUST agree (else a \
                 double-click slips a second apply / a partial approval couples effects)"
            );
        }
    }
    // spot-check the multi-effect shape so a renamed separator is caught loudly.
    assert_eq!(fabric_key(card_id, 0, 3), "card-7:0");
    assert_eq!(fabric_key(card_id, 1, 3), "card-7:1");
    assert_eq!(fabric_key(card_id, 2, 3), "card-7:2");
}

/// **The partial-approval scenario keys (9.1): approve 0 and 2, decline 1 — three DISTINCT per-effect
/// keys, identical on both sides.** Each effect's decision rides its OWN key, so the engine buffers
/// three independent signals and the fabric resumes each independently — the partial approval is
/// well-defined BY CONSTRUCTION (no coupling across the three effects).
#[test]
fn cdc_9_1_partial_approval_keys_are_three_independent_and_agree() {
    let card_id = "card-7";
    let total = 3;
    let keys: Vec<String> = (0..total).map(|idx| fabric_key(card_id, idx, total)).collect();
    // three DISTINCT keys (no two effects share a key — independence).
    assert_eq!(keys.len(), 3);
    assert_ne!(keys[0], keys[1]);
    assert_ne!(keys[1], keys[2]);
    assert_ne!(keys[0], keys[2]);
    // and each agrees with the durable engine (the PROVIDER dedups on exactly these).
    for (idx, k) in keys.iter().enumerate() {
        assert_eq!(*k, engine_key(card_id, idx, total), "partial-approval key {idx} agrees with the engine");
    }
}
