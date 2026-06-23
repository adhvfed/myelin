//! # ISS-D4 — the create-storm human-key drill (ISS-P08 / P-374, M4-I1)
//!
//! **Drill:** `planning/05-refined-shared-systems-architecture/testing-strategy/
//! 01-whole-system-e2e-and-drill-catalogue.md` row ISS-D4 (create-storm on one hot prefix, N workers —
//! import + incident burst → **no duplicate key**, **monotonic per prefix**, gaps benign, **per-prefix
//! isolation**, the key == the stored canonical id). Owning architecture:
//! `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//! §4 (the Hi/Lo allocator) + §"Key allocation" row.
//!
//! ## The green artifact (measured, dated 2026-06-23)
//! This is the DEFAULT-BUILD leg of ISS-D4 — the storm over the in-process
//! [`myelin_issues::HiLoKeyAllocator`] + the in-memory `prefix_counter` model (the SAME atomic-reserve
//! semantics the live Postgres `UPDATE … RETURNING` gives, modelled by
//! [`myelin_issues::InMemoryPrefixCounter`]). It proves, under a concurrent N-worker storm on ONE hot
//! prefix:
//!
//! - **0 duplicate key** — the storm minted `WORKERS * PER_WORKER` DISTINCT canonical keys (a
//!   duplicate `<PROJECTKEY>-<seqno>` is a Tier-1 correctness failure: two issues sharing a canonical
//!   id is silent data corruption);
//! - **monotonic per prefix** — the seqnos for the hot prefix are exactly the contiguous `1..=total`
//!   (gap-free here because every reserved block is fully consumed; a leaked block on crash would be a
//!   benign gap, proven in `keys.rs::tests::gap_tolerant_…`);
//! - **per-prefix isolation** — a second prefix run concurrently has its OWN independent `1..=N`
//!   seqno space; the two never collide (a busy `ENG` does not slow / collide with `OPS`);
//! - **the key == the stored canonical id** — every minted key renders to its
//!   `myelin://<tenant>/issue/issue/<PROJECTKEY>-<seqno>` URN and `myelin_refs::parse` admits it (5.1).
//!
//! The LIVE-Postgres leg (the `prefix_counter` `UPDATE … RETURNING` atomic reserve under a real
//! concurrent storm) is `integration_iss_p08_key_storm.rs` — the dated green artifact against the dev
//! stack (registered red-until-proven in the scorecard, flipped green only with the real artifact).

use myelin_issues::{HiLoKeyAllocator, InMemoryPrefixCounter};
use myelin_tenancy::TenantId;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::thread;

const WORKERS: usize = 24;
const PER_WORKER: usize = 1000;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

/// **ISS-D4 (default-build leg): an import + incident-burst storm on ONE hot prefix → 0 dup key,
/// monotonic, per-prefix isolation, key == the stored canonical id.**
#[test]
fn create_storm_zero_dup_key_monotonic_isolated() {
    let allocator = Arc::new(HiLoKeyAllocator::new(InMemoryPrefixCounter::new()));

    // The hot prefix (ENG) AND a second prefix (OPS) run CONCURRENTLY — proving per-prefix isolation
    // (a busy ENG does not collide with OPS) while ENG takes the storm.
    let mut handles = Vec::new();
    for _ in 0..WORKERS {
        let a = Arc::clone(&allocator);
        handles.push(thread::spawn(move || {
            (0..PER_WORKER)
                .map(|_| a.allocate(&tenant(), "ENG").expect("allocate ENG").seqno)
                .collect::<Vec<u64>>()
        }));
    }
    // a concurrent OPS worker (the isolation half).
    let ops_alloc = Arc::clone(&allocator);
    let ops_handle = thread::spawn(move || {
        (0..PER_WORKER)
            .map(|_| {
                ops_alloc
                    .allocate(&tenant(), "OPS")
                    .expect("allocate OPS")
                    .seqno
            })
            .collect::<Vec<u64>>()
    });

    let mut eng: Vec<u64> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    let mut ops = ops_handle.join().unwrap();

    let total = WORKERS * PER_WORKER;
    assert_eq!(eng.len(), total, "the storm minted {total} ENG keys");

    // ── 0 duplicate key ──────────────────────────────────────────────────────────────────────────
    let distinct: BTreeSet<u64> = eng.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        total,
        "0 duplicate key under a {WORKERS}-worker storm ({total} distinct seqnos)"
    );

    // ── monotonic per prefix (contiguous 1..=total — every block fully consumed) ───────────────────
    eng.sort_unstable();
    assert_eq!(eng.first(), Some(&1), "the first ENG seqno is 1");
    assert_eq!(
        eng.last(),
        Some(&(total as u64)),
        "the last ENG seqno is {total}"
    );
    for (i, seq) in eng.iter().enumerate() {
        assert_eq!(*seq, (i + 1) as u64, "monotonic + gap-free 1..=total");
    }

    // ── per-prefix isolation: OPS has its OWN independent 1..=PER_WORKER space (no collision) ──────
    ops.sort_unstable();
    assert_eq!(ops.len(), PER_WORKER);
    let ops_distinct: BTreeSet<u64> = ops.iter().copied().collect();
    assert_eq!(
        ops_distinct.len(),
        PER_WORKER,
        "0 duplicate key on the isolated OPS prefix"
    );
    assert_eq!(
        ops.first(),
        Some(&1),
        "OPS starts at its OWN seqno 1 (isolated from ENG)"
    );
    assert_eq!(ops.last(), Some(&(PER_WORKER as u64)));

    // ── the key == the stored canonical id (5.1): a sampled minted key renders to a parseable URN ──
    let allocator2 = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let key = allocator2.allocate(&tenant(), "ENG").unwrap();
    let stored = key.issue_artifact_ref(&tenant());
    assert_eq!(stored.0, "myelin://acme/issue/issue/ENG-1");
    myelin_refs::parse(&stored.0).expect("the stored canonical key is the ArtifactRef <id> (5.1)");
}
