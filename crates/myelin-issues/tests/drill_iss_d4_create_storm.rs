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

#[test]
fn create_storm_zero_dup_key_monotonic_isolated() {
    let allocator = Arc::new(HiLoKeyAllocator::new(InMemoryPrefixCounter::new()));

    let mut handles = Vec::new();
    for _ in 0..WORKERS {
        let a = Arc::clone(&allocator);
        handles.push(thread::spawn(move || {
            (0..PER_WORKER)
                .map(|_| a.allocate(&tenant(), "ENG").expect("allocate ENG").seqno)
                .collect::<Vec<u64>>()
        }));
    }
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

    let distinct: BTreeSet<u64> = eng.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        total,
        "0 duplicate key under a {WORKERS}-worker storm ({total} distinct seqnos)"
    );

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

    let allocator2 = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let key = allocator2.allocate(&tenant(), "ENG").unwrap();
    let stored = key.issue_artifact_ref(&tenant());
    assert_eq!(stored.0, "myelin://acme/issue/issue/ENG-1");
    myelin_refs::parse(&stored.0).expect("the stored canonical key is the ArtifactRef <id> (5.1)");
}
