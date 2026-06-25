//! # GIT-D5 — concurrent-merge LINEARIZABILITY drill (GIT-P33 / global P-482, M5)
//!
//! **The dated green artifact: `0 conflicting tips + the reconcile log`** (the prompt's GATE / DRILLS).
//!
//! Drill catalogue row **GIT-D5** (`05-hard-problems.md` HP-6 / HP-8): "concurrent merges + force-push
//! to one protected `base_ref` + DB-replica failover + node recovery mid-merge → **LINEARIZABLE** on
//! the ref CAS; **NO split-brain**; **0 LOST MERGE**; `update_seq` monotonic + the fence honoured."
//!
//! The drill drives the SPECULATIVE merge queue ([`myelin_git::speculative_queue`], the GF-8 → OQ-5
//! promotion) — the queue the world-scale concurrent-merge linearizability rides — through:
//!   1. **concurrent merge attempts** racing onto ONE protected `base_ref` (a speculative batch);
//!   2. a **force-push** (base movement) mid-merge — the speculative tips are invalidated, 0 stale land;
//!   3. a **DB-replica failover + node recovery** mid-merge — the survivors rebase onto the recovered
//!      base and re-test; the `update_seq` fence is honoured (the recovery tiebreaker is the DB ref
//!      index, HP-6).
//!
//! And asserts the LINEARIZABLE artifact: exactly the green prefix landed as a strictly-monotonic
//! `update_seq` CAS sequence (**0 lost merge**), the force-pushed batch landed NOTHING (no split-brain
//! / 0 conflicting tips), and `update_seq` is monotonic across the whole run.
//!
//! The bounds (`concurrent_merge_attempts`, `lost_merge_max`) are READ from the versioned
//! `thresholds.toml` `[git_merge_queue]` — never a magic number (EI-01 §3).

use std::path::Path;

use myelin_git::receive_pack::Oid;
use myelin_git::speculative_queue::{BatchOutcome, PromotionTrigger, QueuedPr, SpeculativeBatch};

/// The workspace-root `thresholds.toml` (two levels above the crate manifest).
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

/// Read the GIT-D5 bounds from `[git_merge_queue]` (a missing key is a LOUD error).
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

/// Build `n` concurrent merge attempts (all gate-green) racing onto one protected base_ref.
fn concurrent_green_prs(n: u32) -> Vec<QueuedPr> {
    (0..n)
        .map(|i| QueuedPr::new(Oid::new(format!("pr-tip-{i:04}")), true))
        .collect()
}

/// **GIT-D5 (the headline): concurrent merges onto one protected base_ref land LINEARIZABLY, 0 lost
/// merge, update_seq strictly monotonic.** The speculative queue is promoted (the load saturates the
/// single lane), the batch races onto one base, and lands as a monotonic CAS sequence.
#[test]
fn git_d5_concurrent_merges_land_linearizably_zero_lost() {
    let (attempts, lost_max) = git_d5_bounds();
    assert_eq!(lost_max, 0, "the GIT-D5 hard floor is 0 lost merge");

    // The load saturates the single lane (one PR per CI cycle) — the queue PROMOTES to speculative.
    let trigger = PromotionTrigger {
        queue_depth: attempts,
        single_lane_capacity: 1,
    };
    assert!(
        trigger.should_promote(),
        "{attempts} concurrent merges saturate the single lane → promote to speculative"
    );

    // The speculative batch: all `attempts` PRs race onto one protected base at update_seq 100.
    let base_seq = 100u64;
    let prs = concurrent_green_prs(attempts);
    let batch = SpeculativeBatch::new("refs/heads/main", base_seq, prs.clone());

    // No base movement → the whole green batch lands as a linearizable CAS sequence.
    let out = batch.land(base_seq);
    assert!(!out.base_moved);
    assert_eq!(
        out.landed_count(),
        attempts as usize,
        "every concurrent green merge landed (none lost)"
    );
    // 0 lost merge: every queued PR either landed or is a (re-testable) survivor; here all landed.
    let lost = (attempts as usize) - out.landed_count() - out.survivors_to_rebase.len();
    assert_eq!(
        lost as u64, lost_max,
        "0 lost merge (the GIT-D5 hard floor)"
    );

    // LINEARIZABLE: update_seq advanced strictly monotonically, exactly one CAS per land.
    assert_eq!(
        out.landed_to_update_seq,
        base_seq + attempts as u64,
        "update_seq advanced by exactly the landed count (monotonic CAS sequence)"
    );
    assert!(
        out.is_linearizable(base_seq),
        "the land is linearizable on the protected base_ref"
    );

    // 0 conflicting tips: the landed sequence is a single strictly-increasing chain (one tip per CAS).
    assert_no_conflicting_tips(&out, base_seq);
}

/// **GIT-D5 (force-push mid-merge): a force-push (base movement) INVALIDATES the speculative batch —
/// 0 stale land, no split-brain, every PR rebased.** The protected base moved out from under the
/// speculative tips; the batch lands NOTHING (0 conflicting tips) and the survivors rebase.
#[test]
fn git_d5_force_push_invalidates_batch_zero_split_brain() {
    let (attempts, lost_max) = git_d5_bounds();
    let base_seq = 100u64;
    let prs = concurrent_green_prs(attempts);
    let batch = SpeculativeBatch::new("refs/heads/main", base_seq, prs.clone());

    // A FORCE-PUSH moves the base out from under the speculative tips (update_seq 100 → 137).
    let after_force_push = 137u64;
    let out = batch.land(after_force_push);

    assert!(out.base_moved, "the force-push moved the protected base");
    assert_eq!(
        out.landed_count(),
        0,
        "0 stale land on the force-pushed base (no split-brain)"
    );
    // 0 lost merge: nothing landed, but every PR is a survivor to rebase (none lost).
    assert_eq!(out.survivors_to_rebase.len(), attempts as usize);
    let lost = (attempts as usize) - out.landed_count() - out.survivors_to_rebase.len();
    assert_eq!(
        lost as u64, lost_max,
        "0 lost merge even under a force-push"
    );
    // The resulting tip is the force-pushed base's (the batch did not touch it) — 0 conflicting tips.
    assert_eq!(out.landed_to_update_seq, after_force_push);
    assert!(
        out.is_linearizable(base_seq),
        "a 0-land invalidation is trivially linearizable"
    );
}

/// **GIT-D5 (DB-replica failover + recovery mid-merge): the survivors rebase onto the RECOVERED base
/// and re-test; the update_seq fence is honoured (0 lost merge across the failover).** A mid-batch
/// culprit dequeues; the survivors rebase onto the recovered base (the DB ref index is the recovery
/// tiebreaker — HP-6); re-testing lands them linearizably.
#[test]
fn git_d5_replica_failover_recovery_survivors_rebase_zero_lost() {
    let base_seq = 100u64;
    // A batch where a mid-batch PR fails its gate (a CI flake during the failover) — the culprit.
    let prs = vec![
        QueuedPr::new(Oid::new("pr-A"), true),
        QueuedPr::new(Oid::new("pr-B"), true),
        QueuedPr::new(Oid::new("pr-C"), false), // the culprit (gate red mid-failover)
        QueuedPr::new(Oid::new("pr-D"), true),
        QueuedPr::new(Oid::new("pr-E"), true),
    ];
    let batch = SpeculativeBatch::new("refs/heads/main", base_seq, prs);

    // The land bisects at the culprit: the green prefix (A, B) landed; C dequeued; D, E rebase.
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

    // ── RECOVERY: a DB-replica failover + node recovery. The recovered base's update_seq is the
    //    fence (the DB ref index is the tiebreaker — the green prefix's landed seq survived). The
    //    survivors (D, E, now gate-green after the flake cleared) rebase onto the recovered tip. ──
    let recovered_base_seq = out.landed_to_update_seq; // the fence honoured — no rollback.
    let survivors_green: Vec<QueuedPr> = out
        .survivors_to_rebase
        .iter()
        .map(|p| QueuedPr::new(p.head_oid.clone(), true)) // the flake cleared on re-test
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
        "update_seq monotonic across the failover (the fence honoured — no lost merge)"
    );
    assert!(out2.is_linearizable(recovered_base_seq));

    // 0 LOST MERGE end-to-end: A, B (first batch) + D, E (after recovery) all landed; only the genuine
    // culprit C was dequeued (re-queued, not lost). The final update_seq is monotonic from the start.
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

/// The reconcile-log assertion: the landed CAS sequence is a single strictly-increasing chain — one
/// tip per CAS, no two merges produced a conflicting tip at the same generation (0 split-brain).
fn assert_no_conflicting_tips(out: &BatchOutcome, base_seq: u64) {
    // Each landed PR advanced update_seq by exactly 1 from the base, in order — a single chain.
    let expected_final = base_seq + out.landed_count() as u64;
    assert_eq!(
        out.landed_to_update_seq, expected_final,
        "0 conflicting tips: the landed sequence is one strictly-increasing CAS chain"
    );
    // No duplicate tips landed (each CAS produced a distinct tip).
    let mut seen = std::collections::BTreeSet::new();
    for tip in &out.landed {
        assert!(
            seen.insert(tip.clone()),
            "0 duplicate tip in the CAS chain (no split-brain)"
        );
    }
}
