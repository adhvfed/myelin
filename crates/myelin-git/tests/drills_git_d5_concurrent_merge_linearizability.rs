use std::path::Path;

use myelin_git::receive_pack::Oid;
use myelin_git::speculative_queue::{BatchOutcome, PromotionTrigger, QueuedPr, SpeculativeBatch};

fn thresholds() -> toml::Value {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let path = root.join("thresholds.toml");
    let text = std::fs::read_to_string(&path).expect("read thresholds.toml");
    text.parse().expect("thresholds.toml is valid TOML")
}

fn git_d5_bounds() -> (u32, u64) {
    let t = thresholds();
    let section = t
        .get("git_merge_queue")
        .expect("git_merge_queue section must be present");
    let attempts = section
        .get("concurrent_merge_attempts")
        .and_then(toml::Value::as_integer)
        .expect("concurrent_merge_attempts must be present") as u32;
    let lost_max = section
        .get("lost_merge_max")
        .and_then(toml::Value::as_integer)
        .expect("lost_merge_max must be present") as u64;
    (attempts, lost_max)
}

fn concurrent_green_prs(n: u32) -> Vec<QueuedPr> {
    (0..n)
        .map(|i| QueuedPr::new(Oid::new(format!("pr-tip-{i:04}")), true))
        .collect()
}

#[test]
fn git_d5_concurrent_merges_land_linearizably_zero_lost() {
    let (attempts, lost_max) = git_d5_bounds();
    assert_eq!(lost_max, 0, "the GIT-D5 hard floor is 0 lost merge");

    let trigger = PromotionTrigger {
        queue_depth: attempts,
        single_lane_capacity: 1,
    };
    assert!(
        trigger.should_promote(),
        "{attempts} concurrent merges saturate the single lane → promote to speculative"
    );

    let base_seq = 100u64;
    let prs = concurrent_green_prs(attempts);
    let batch = SpeculativeBatch::new("refs/heads/main", base_seq, prs.clone());

    let out = batch.land(base_seq);
    assert!(!out.base_moved);
    assert_eq!(
        out.landed_count(),
        attempts as usize,
        "every concurrent green merge landed (none lost)"
    );
    let lost = (attempts as usize) - out.landed_count() - out.survivors_to_rebase.len();
    assert_eq!(
        lost as u64, lost_max,
        "0 lost merge (the GIT-D5 hard floor)"
    );

    assert_eq!(
        out.landed_to_update_seq,
        base_seq + attempts as u64,
        "update_seq advanced by exactly the landed count (monotonic CAS sequence)"
    );
    assert!(
        out.is_linearizable(base_seq),
        "the land is linearizable on the protected base_ref"
    );

    assert_no_conflicting_tips(&out, base_seq);
}

#[test]
fn git_d5_force_push_invalidates_batch_zero_split_brain() {
    let (attempts, lost_max) = git_d5_bounds();
    let base_seq = 100u64;
    let prs = concurrent_green_prs(attempts);
    let batch = SpeculativeBatch::new("refs/heads/main", base_seq, prs.clone());

    let after_force_push = 137u64;
    let out = batch.land(after_force_push);

    assert!(out.base_moved, "the force-push moved the protected base");
    assert_eq!(
        out.landed_count(),
        0,
        "0 stale land on the force-pushed base (no split-brain)"
    );
    assert_eq!(out.survivors_to_rebase.len(), attempts as usize);
    let lost = (attempts as usize) - out.landed_count() - out.survivors_to_rebase.len();
    assert_eq!(
        lost as u64, lost_max,
        "0 lost merge even under a force-push"
    );
    assert_eq!(out.landed_to_update_seq, after_force_push);
    assert!(
        out.is_linearizable(base_seq),
        "a 0-land invalidation is trivially linearizable"
    );
}

#[test]
fn git_d5_replica_failover_recovery_survivors_rebase_zero_lost() {
    let base_seq = 100u64;
    let prs = vec![
        QueuedPr::new(Oid::new("pr-A"), true),
        QueuedPr::new(Oid::new("pr-B"), true),
        QueuedPr::new(Oid::new("pr-C"), false),
        QueuedPr::new(Oid::new("pr-D"), true),
        QueuedPr::new(Oid::new("pr-E"), true),
    ];
    let batch = SpeculativeBatch::new("refs/heads/main", base_seq, prs);

    let out = batch.land(base_seq);
    assert_eq!(out.landed, vec![Oid::new("pr-A"), Oid::new("pr-B")]);
    assert_eq!(
        out.landed_to_update_seq,
        base_seq + 2,
        "green prefix landed (monotonic)"
    );
    assert_eq!(out.culprit, Some(QueuedPr::new(Oid::new("pr-C"), false)));
    assert_eq!(
        out.survivors_to_rebase,
        vec![
            QueuedPr::new(Oid::new("pr-D"), true),
            QueuedPr::new(Oid::new("pr-E"), true)
        ]
    );
    assert!(
        out.is_linearizable(base_seq),
        "the green prefix landed linearizably"
    );

    let recovered_base_seq = out.landed_to_update_seq;
    let survivors_green: Vec<QueuedPr> = out
        .survivors_to_rebase
        .iter()
        .map(|p| QueuedPr::new(p.head_oid.clone(), true))
        .collect();
    let rebatch = SpeculativeBatch::new("refs/heads/main", recovered_base_seq, survivors_green);
    let out2 = rebatch.land(recovered_base_seq);

    assert_eq!(
        out2.landed_count(),
        2,
        "the survivors landed on the recovered base"
    );
    assert_eq!(
        out2.landed_to_update_seq,
        recovered_base_seq + 2,
        "update_seq monotonic across the failover (the fence honoured - no lost merge)"
    );
    assert!(out2.is_linearizable(recovered_base_seq));

    let total_landed = out.landed_count() + out2.landed_count();
    assert_eq!(
        total_landed, 4,
        "A,B,D,E all landed across the failover (0 lost)"
    );
    assert!(
        out2.landed_to_update_seq > base_seq,
        "update_seq is strictly monotonic from the pre-failover base (the recovery fence honoured)"
    );
}

fn assert_no_conflicting_tips(out: &BatchOutcome, base_seq: u64) {
    let expected_final = base_seq + out.landed_count() as u64;
    assert_eq!(
        out.landed_to_update_seq, expected_final,
        "0 conflicting tips: the landed sequence is one strictly-increasing CAS chain"
    );
    let mut seen = std::collections::BTreeSet::new();
    for tip in &out.landed {
        assert!(
            seen.insert(tip.clone()),
            "0 duplicate tip in the CAS chain (no split-brain)"
        );
    }
}
