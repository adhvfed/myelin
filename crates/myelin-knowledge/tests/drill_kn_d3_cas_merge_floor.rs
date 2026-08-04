use myelin_knowledge::{BlockId, CasOutcome, CasStore};

struct Client {
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

#[test]
fn kn_d3_two_clients_same_block_loser_rejected_zero_silent_overwrites() {
    let mut store = CasStore::new();
    let block = bid("shared-block");
    store.insert_block(block.clone(), "base", "{}").unwrap();

    let alice = Client::new(1);
    let mut bob = Client::new(1);

    let mut commits = 0u32;
    let mut conflicts = 0u32;
    let mut silent_overwrites = 0u32;

    let alice_out = store
        .edit_block(&block, alice.base_version, "alice's paragraph", "{}")
        .unwrap();
    match alice_out {
        CasOutcome::Committed(s) => {
            commits += 1;
            assert_eq!(s.version, 2, "alice's commit advances the block to v2");
            assert_eq!(store.get(&block).unwrap().inline, "alice's paragraph");
        }
        CasOutcome::Conflict { .. } => conflicts += 1,
    }

    let bob_out = store
        .edit_block(&block, bob.base_version, "bob's paragraph", "{}")
        .unwrap();
    match bob_out {
        CasOutcome::Committed(_) => {
            commits += 1;
            if store.get(&block).unwrap().inline != "bob's paragraph" {
                silent_overwrites += 1;
            }
        }
        CasOutcome::Conflict { current } => {
            conflicts += 1;
            assert_eq!(current.version, 2, "the loser sees the current version");
            assert_eq!(
                current.inline, "alice's paragraph",
                "the loser sees the winner's content to reconcile"
            );
            bob.base_version = current.version;
        }
    }

    assert_eq!(
        commits, 1,
        "exactly one concurrent same-block writer commits"
    );
    assert_eq!(
        conflicts, 1,
        "the other is rejected with a conflict (the loser reconciles)"
    );
    assert_eq!(silent_overwrites, 0, "KN-D3: 0 silent overwrites");
    assert_eq!(
        store.get(&block).unwrap().inline,
        "alice's paragraph",
        "the store holds the winner's content; the loser never silently overwrote it"
    );

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

#[test]
fn kn_d3_different_blocks_parallel_no_false_conflict() {
    let mut store = CasStore::new();
    let blocks: Vec<BlockId> = (0..10).map(|i| bid(&format!("b{i}"))).collect();
    for b in &blocks {
        store.insert_block(b.clone(), "init", "{}").unwrap();
    }

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
        "every different-block edit commits - no false conflict"
    );
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
    for (i, b) in blocks.iter().enumerate() {
        assert_eq!(
            store.get(b).unwrap().inline,
            format!("edited by client {i}")
        );
    }
}

#[test]
fn kn_d3_chained_interleave_zero_silent_overwrites_over_many_writers() {
    let mut store = CasStore::new();
    let block = bid("hot-block");
    store.insert_block(block.clone(), "v0", "{}").unwrap();

    let mut silent_overwrites = 0u32;
    let mut last_committed_inline = String::from("v0");

    for i in 0..50u64 {
        let current_version = store.get(&block).unwrap().version;
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
                if store.get(&block).unwrap().inline != proposed {
                    silent_overwrites += 1;
                }
                last_committed_inline = proposed;
            }
            CasOutcome::Conflict { current } => {
                assert_eq!(
                    current.inline, last_committed_inline,
                    "the loser sees the current committed state"
                );
                assert_eq!(
                    store.get(&block).unwrap().inline,
                    last_committed_inline,
                    "the loser's bytes never landed - 0 silent overwrite at every step"
                );
            }
        }
    }

    assert_eq!(
        silent_overwrites, 0,
        "KN-D3: 0 silent overwrites across the chained interleave"
    );
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
    assert!(
        (store.meter().conflict_rate() - 0.5).abs() < 1e-9,
        "the conflict rate is 0.5 (high contention)"
    );
}
