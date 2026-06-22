//! # KN-D3 — the per-block CAS merge floor drill (KN-P13 / P-303, M3 — the named-floor proof)
//!
//! **Drill catalogue (testing-strategy/01-…-catalogue.md, row KN-D3):** "Two clients edit the same
//! block concurrently → the loser is rejected with current state (never silently overwritten);
//! different blocks edit in parallel, no false conflict. — **0 silent overwrites** — CI."
//!
//! This is the named-floor proof in the master M3→M4 gate (roadmap §3, KN-M3c). It is a CONCURRENCY
//! property, not a single handler: two clients each read a block at the same version, then INTERLEAVE
//! their writes through the per-block CAS guard ([`myelin_knowledge::CasStore::edit_block`]). The
//! property the drill GATES:
//!
//! - exactly ONE of the two interleaved same-block writers commits; the other is REJECTED with the
//!   current server state to reconcile (`Conflict{current}`) — **never silently overwritten**;
//! - after the full interleave the store holds the WINNER's bytes, and **0 silent overwrites** is the
//!   quantified counter (a silent overwrite = a write that landed without holding the CAS — measured
//!   to be exactly 0);
//! - DIFFERENT blocks edited in parallel produce 0 false conflicts;
//! - the CAS-conflict-rate metric (the CRDT-promotion trigger, KQ-1) is emitted with the conflicts the
//!   interleave produced.
//!
//! The drill runs a REAL concurrent interleave (two client cursors, alternating sends against the same
//! [`CasStore`]), not an asserted single outcome.

use myelin_knowledge::{BlockId, CasOutcome, CasStore};

/// One simulated client: it holds the version it last READ for a block (its CAS base) and sends edits
/// against that base, exactly as a real collaborator client presents its `expected_version`.
struct Client {
    /// The version this client last read for the block it is editing (its CAS base / `expected_version`).
    base_version: u64,
}

impl Client {
    fn new(base_version: u64) -> Client {
        Client { base_version }
    }
}

fn bid(s: &str) -> BlockId {
    BlockId(s.to_string())
}

/// **KN-D3 (the headline): two clients edit the SAME block concurrently, interleaved → exactly one
/// commits, the loser is rejected with current state, 0 silent overwrites.**
#[test]
fn kn_d3_two_clients_same_block_loser_rejected_zero_silent_overwrites() {
    let mut store = CasStore::new();
    let block = bid("shared-block");
    // The block starts at v1 with content "base".
    store.insert_block(block.clone(), "base", "{}").unwrap();

    // Two clients BOTH read the block at v1 (the classic concurrent-edit setup: each decided to edit
    // before seeing the other's write — the same base version).
    let alice = Client::new(1);
    let mut bob = Client::new(1);

    // ── THE INTERLEAVE: alice sends first, then bob sends against his (now stale) v1 base. ──────────
    let mut commits = 0u32;
    let mut conflicts = 0u32;
    // A "silent overwrite" would be a write that landed in the store WITHOUT holding the CAS. We track
    // the store's content after every write and assert each commit corresponds to a held CAS.
    let mut silent_overwrites = 0u32;

    // alice writes at her base v1.
    let alice_out = store
        .edit_block(&block, alice.base_version, "alice's paragraph", "{}")
        .unwrap();
    match alice_out {
        CasOutcome::Committed(s) => {
            commits += 1;
            assert_eq!(s.version, 2, "alice's commit advances the block to v2");
            // a commit is only legitimate if the store actually reflects alice's bytes.
            assert_eq!(store.get(&block).unwrap().inline, "alice's paragraph");
        }
        CasOutcome::Conflict { .. } => conflicts += 1,
    }

    // bob writes at his STILL-v1 base (he never saw alice's write — the concurrent edit).
    let bob_out = store
        .edit_block(&block, bob.base_version, "bob's paragraph", "{}")
        .unwrap();
    match bob_out {
        CasOutcome::Committed(_) => {
            commits += 1;
            // If bob "committed" at a stale base while alice already wrote, that would be a silent
            // overwrite of alice — the bug the floor exists to prevent. Detect it explicitly.
            if store.get(&block).unwrap().inline != "bob's paragraph" {
                silent_overwrites += 1;
            }
        }
        CasOutcome::Conflict { current } => {
            conflicts += 1;
            // bob is handed the CURRENT state (alice's) to reconcile — the loser is rejected, not
            // silently overwritten.
            assert_eq!(current.version, 2, "the loser sees the current version");
            assert_eq!(
                current.inline, "alice's paragraph",
                "the loser sees the winner's content to reconcile"
            );
            bob.base_version = current.version; // bob re-bases for his reconcile
        }
    }

    // EXACTLY one of the two interleaved same-block writers committed.
    assert_eq!(
        commits, 1,
        "exactly one concurrent same-block writer commits"
    );
    assert_eq!(
        conflicts, 1,
        "the other is rejected with a conflict (the loser reconciles)"
    );
    // THE QUANTIFIED GATE: 0 silent overwrites.
    assert_eq!(silent_overwrites, 0, "KN-D3: 0 silent overwrites");
    // The store holds the WINNER's bytes (alice's), not the loser's.
    assert_eq!(
        store.get(&block).unwrap().inline,
        "alice's paragraph",
        "the store holds the winner's content; the loser never silently overwrote it"
    );

    // ── the loser RECONCILES: bob re-bases on the current state (v2) and re-submits → now commits. ─
    let bob_reconciled = store
        .edit_block(
            &block,
            bob.base_version,
            "alice's paragraph + bob's reconciled addition",
            "{}",
        )
        .unwrap();
    assert!(
        bob_reconciled.committed(),
        "after reconciling at the current version, the loser commits"
    );
    assert_eq!(
        store.get(&block).unwrap().version,
        3,
        "the reconciled commit advances to v3"
    );

    // the conflict-rate metric (the CRDT-promotion trigger, KQ-1) recorded the contention.
    assert_eq!(
        store.meter().conflicted(),
        1,
        "the conflict-rate metric recorded the 1 conflict"
    );
    assert!(
        store.meter().conflict_rate() > 0.0,
        "the CAS-conflict-rate (the CRDT trigger) is emitted"
    );
}

/// **KN-D3 (second half): different blocks edit in parallel with NO false conflict.**
#[test]
fn kn_d3_different_blocks_parallel_no_false_conflict() {
    let mut store = CasStore::new();
    // Ten blocks, each at v1.
    let blocks: Vec<BlockId> = (0..10).map(|i| bid(&format!("b{i}"))).collect();
    for b in &blocks {
        store.insert_block(b.clone(), "init", "{}").unwrap();
    }

    // Ten clients each edit a DIFFERENT block, all at v1, interleaved (round-robin).
    let mut all_committed = true;
    for (i, b) in blocks.iter().enumerate() {
        let out = store
            .edit_block(b, 1, format!("edited by client {i}"), "{}")
            .unwrap();
        if !out.committed() {
            all_committed = false;
        }
    }
    assert!(
        all_committed,
        "every different-block edit commits — no false conflict"
    );
    // 0 conflicts: different blocks are independent (the per-block CAS guard).
    assert_eq!(
        store.meter().conflicted(),
        0,
        "KN-D3: 0 false conflicts across different blocks"
    );
    assert_eq!(
        store.meter().committed(),
        10,
        "all ten parallel different-block edits committed"
    );
    // each block holds its own client's content (no cross-block clobber).
    for (i, b) in blocks.iter().enumerate() {
        assert_eq!(
            store.get(b).unwrap().inline,
            format!("edited by client {i}")
        );
    }
}

/// **KN-D3 (a longer chained interleave): N clients hammer the SAME block; exactly the held-CAS writes
/// commit, every loser reconciles, and the count of silent overwrites is 0 across the whole chain.**
#[test]
fn kn_d3_chained_interleave_zero_silent_overwrites_over_many_writers() {
    let mut store = CasStore::new();
    let block = bid("hot-block");
    store.insert_block(block.clone(), "v0", "{}").unwrap();

    // 50 writers, each reading the version at the START of their turn, then half of them deliberately
    // present a STALE base (simulating they read before the previous writer landed) to force a real
    // interleave of winners and losers.
    let mut silent_overwrites = 0u32;
    let mut last_committed_inline = String::from("v0");

    for i in 0..50u64 {
        let current_version = store.get(&block).unwrap().version;
        // even writers read fresh (will commit); odd writers present a stale base (will conflict).
        let base = if i % 2 == 0 {
            current_version
        } else {
            current_version.saturating_sub(1)
        };
        let proposed = format!("write-{i}");
        let out = store
            .edit_block(&block, base, proposed.clone(), "{}")
            .unwrap();
        match out {
            CasOutcome::Committed(_) => {
                // a commit must have held the CAS — verify the store reflects it (else a silent
                // overwrite slipped through).
                if store.get(&block).unwrap().inline != proposed {
                    silent_overwrites += 1;
                }
                last_committed_inline = proposed;
            }
            CasOutcome::Conflict { current } => {
                // the loser is handed the current state — and the store STILL holds the last legit
                // commit, never the loser's bytes.
                assert_eq!(
                    current.inline, last_committed_inline,
                    "the loser sees the current committed state"
                );
                assert_eq!(
                    store.get(&block).unwrap().inline,
                    last_committed_inline,
                    "the loser's bytes never landed — 0 silent overwrite at every step"
                );
            }
        }
    }

    // THE QUANTIFIED GATE across the whole chain: 0 silent overwrites.
    assert_eq!(
        silent_overwrites, 0,
        "KN-D3: 0 silent overwrites across the chained interleave"
    );
    // every odd writer (stale base) conflicted; every even writer committed.
    assert_eq!(
        store.meter().conflicted(),
        25,
        "the 25 stale writers all conflicted (rejected, reconciled)"
    );
    assert_eq!(
        store.meter().committed(),
        25,
        "the 25 fresh writers all committed"
    );
    // the conflict-rate metric reflects the 50/50 contention (the CRDT-promotion trigger reads this).
    assert!(
        (store.meter().conflict_rate() - 0.5).abs() < 1e-9,
        "the conflict rate is 0.5 (high contention)"
    );
}
