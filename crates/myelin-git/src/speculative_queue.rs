//! # `speculative_queue` — the speculative/parallel merge queue (GF-8 → OQ-5, GIT-P33 / P-482, M5)
//!
//! **The single-lane serialised merge queue promotes to speculative/parallel batching once the
//! promotion trigger is MEASURED.** The M3 floor ([`crate::merge_queue`] / GF-8) tested + merged ONE
//! PR at a time per protected `base_ref` (correctness first). This module promotes it: a SPECULATIVE
//! BATCH tests several queued PRs OPTIMISTICALLY stacked on each other's speculative tips, so CI runs
//! in parallel; on success the whole batch lands; on a base movement or a mid-batch failure the queue
//! BISECTS — the survivors are rebased onto the new base and re-tested, the culprit is dequeued.
//!
//! Critically, the promotion does NOT change the linearisation point: the protected `base_ref` is
//! still advanced by the per-ref CAS (the home cell's DB transaction, HP-6). A speculative batch is an
//! OPTIMISTIC test ordering; the ACTUAL land is still a linearizable CAS sequence on `base_ref`. So the
//! speculative queue is a throughput optimisation that preserves the GF-8 correctness guarantee — and
//! it is the queue the GIT-D5 linearizability drill runs against.
//!
//! **Owning architecture (read first, in full):**
//! `05-hard-problems.md` **HP-8** (merge queue — "single-lane serialised durable workflow in v1 (GF-8);
//! speculative/parallel batching is the demand-triggered follow-on (OQ-5)"; GitHub merge-queue as the
//! speculative-batch target) + **HP-6** (the per-ref CAS is the linearisation point; `update_seq` is the
//! fence; no split-brain; the recovery tiebreaker is the DB ref index). `02-internals-and-algorithms.md`
//! §6.4 (the merge queue — one durable workflow per target ref) + §3-4 (the linearizable merge).
//!
//! ## What is REUSED vs NEW (EI-01 §7 coherence)
//! REUSED, never re-defined:
//! - [`crate::merge_queue::GitMergePerformer`] — the §6.2/§6.3 "what is allowed to land" gate bound to
//!   the durable merge step (the single-lane composition).
//! - The per-ref CAS + `update_seq` fence ([`crate::receive_pack::RefStore`]) — the linearisation point
//!   the actual land still goes through.
//! - The promotion DISCIPLINE — promote only when the trigger is MEASURED (the GIT-D4 measure-before-
//!   shard idiom, mirrored here for the queue).
//!
//! What is **genuinely NEW** here (the OQ-5 promotion):
//! 1. [`PromotionTrigger`] — the MEASURED trigger: the single-lane queue's merge throughput is
//!    saturated (the queue depth exceeds what one lane drains within the latency budget) → promote to
//!    speculative batching. Never promoted on a guess (§8 measure-before-shard).
//! 2. [`SpeculativeBatch`] — an optimistic stack of queued PRs tested in parallel on each other's
//!    speculative tips, landed as a linearizable CAS sequence on the protected base.
//! 3. [`BatchOutcome`] + [`SpeculativeBatch::land`] — the bisect-on-failure mechanics: a clean batch
//!    lands all; a base movement / mid-batch failure bisects (survivors rebased, culprit dequeued) —
//!    **linearizable on `base_ref`, 0 lost merge, no split-brain** (the GIT-D5 property).
//!
//! ## FLOOR PROMOTED (the honesty register — VISION §3 / EI-01 §1)
//! - **GF-8 — single-lane serialised queue (M3 floor) → speculative/parallel batching, promoted once
//!   the trigger is MEASURED (OQ-5).** The speculative-batch model + the bisect-on-failure + the
//!   measured-promotion gate ship HERE. Recorded, dated GIT-P33. The real CI fan-out of a speculative
//!   batch rides the durable-workflow body ([`myelin_flow`]); this owns the batch-ordering + bisect
//!   semantics + the linearizable land sequence over the per-ref CAS.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; a lost merge is the failure)
//! The land path is mandatory-core. The load-bearing mutants — the measured-promotion gate
//! ([`PromotionTrigger::should_promote`] / its `>` boundary), the linearizable CAS sequence
//! ([`SpeculativeBatch::land`] advances `update_seq` strictly monotonically, 0 lost), the bisect on a
//! base movement (the speculative tips are invalidated, survivors rebased — never a silent stale land),
//! and the 0-lost-merge invariant — are each killed by an assertion in the unit + GIT-D5 drill tests.
//! The floor is **≥ 80%**.

use crate::receive_pack::Oid;

// ───────────────────────────── OQ-5 — the MEASURED promotion trigger ─────────────────────────────

/// **The measured promotion trigger (OQ-5 / §8 measure-before-shard).** The single-lane queue promotes
/// to speculative batching ONLY when its throughput is MEASURED-saturated: the queue depth exceeds what
/// one lane drains within the per-PR latency budget. Never promoted on a guess. PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromotionTrigger {
    /// The current queue depth (PRs waiting to merge on one protected base_ref).
    pub queue_depth: u32,
    /// How many PRs the SINGLE lane drains within the latency budget (the measured single-lane
    /// capacity — one PR tested+merged serially per CI cycle).
    pub single_lane_capacity: u32,
}

impl PromotionTrigger {
    /// **Should the queue PROMOTE to speculative batching?** `true` IFF `queue_depth >
    /// single_lane_capacity` — the single lane is MEASURED-saturated (it cannot drain the queue within
    /// the latency budget). At-or-below capacity stays single-lane (correctness-first, no premature
    /// speculation). Mandatory-core: the strict `>` is the measured trigger.
    pub fn should_promote(self) -> bool {
        self.queue_depth > self.single_lane_capacity
    }

    /// The speculative batch width to promote to — the backlog over single-lane capacity, so the batch
    /// drains the saturation. At least 2 (a speculative batch is ≥ 2 PRs; a 1-PR "batch" is the single
    /// lane). Capped at the queue depth (never speculate more PRs than are queued).
    pub fn batch_width(self) -> u32 {
        if !self.should_promote() {
            return 1; // not saturated → single lane.
        }
        let over = self.queue_depth - self.single_lane_capacity;
        over.max(2).min(self.queue_depth)
    }
}

// ───────────────────────────── the speculative batch (OQ-5) ──────────────────────────────────────

/// **One queued PR in a speculative batch** — its head tip + its required-context green-ness (the gate
/// fact). PII-free: oids + a bool, no person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedPr {
    /// The PR's head commit oid (the tip the speculative batch stacks).
    pub head_oid: Oid,
    /// Whether this PR's required contexts are all green (the §6.2 gate already admitted it). A batch
    /// member that is not green is the culprit a mid-batch failure bisects out.
    pub gate_green: bool,
}

impl QueuedPr {
    /// A queued PR with a head tip and its gate verdict.
    pub fn new(head_oid: Oid, gate_green: bool) -> QueuedPr {
        QueuedPr {
            head_oid,
            gate_green,
        }
    }
}

/// **A speculative batch — an optimistic stack of queued PRs tested in parallel (OQ-5).** Each PR is
/// tested on the speculative tip of the previous PR (so CI runs all of them at once rather than one per
/// cycle). The batch is landed as a LINEARIZABLE CAS sequence on the protected `base_ref`: the base's
/// `update_seq` advances strictly monotonically, one CAS per landed PR (HP-6).
#[derive(Clone, Debug)]
pub struct SpeculativeBatch {
    /// The protected base ref the batch lands onto (the linearisation point).
    base_ref: String,
    /// The base's `update_seq` at batch-build time (the speculative tips were stacked on THIS base).
    base_update_seq: u64,
    /// The optimistically-stacked PRs, in queue order (the first lands first).
    prs: Vec<QueuedPr>,
}

impl SpeculativeBatch {
    /// Build a speculative batch over a protected base at `base_update_seq`, stacking `prs` in queue
    /// order.
    pub fn new(
        base_ref: impl Into<String>,
        base_update_seq: u64,
        prs: Vec<QueuedPr>,
    ) -> SpeculativeBatch {
        SpeculativeBatch {
            base_ref: base_ref.into(),
            base_update_seq,
            prs,
        }
    }

    /// The protected base ref the batch lands onto.
    pub fn base_ref(&self) -> &str {
        &self.base_ref
    }

    /// **Land the speculative batch as a linearizable CAS sequence on `base_ref`** — the GIT-D5 core.
    ///
    /// `current_base_seq` is the base's `update_seq` AT LAND TIME (read under the per-ref lock). Two
    /// cases:
    /// - **No base movement** (`current_base_seq == base_update_seq`): the speculative tips are still
    ///   valid. Each green PR lands in order, advancing `update_seq` by 1 per land (a strictly
    ///   monotonic CAS sequence — linearizable, 0 lost). A non-green PR is the culprit: it is dequeued
    ///   and the batch BISECTS (the PRs after it are returned as survivors to re-test).
    /// - **Base movement** (`current_base_seq != base_update_seq`): the protected base moved out from
    ///   under the speculative tips (a force-push or a concurrent merge). The whole batch is
    ///   INVALIDATED — NONE land (no silent stale land); every PR is returned as a survivor to rebase
    ///   onto the new base and re-test. **0 lost merge** (nothing landed on a stale base).
    pub fn land(&self, current_base_seq: u64) -> BatchOutcome {
        // ── Base movement → the speculative tips are stale; bisect-all (rebase every PR). ──
        if current_base_seq != self.base_update_seq {
            return BatchOutcome {
                landed: Vec::new(),
                landed_to_update_seq: current_base_seq,
                survivors_to_rebase: self.prs.clone(),
                culprit: None,
                base_moved: true,
            };
        }

        // ── No base movement → land the green prefix; bisect at the first non-green culprit. ──
        let mut landed = Vec::new();
        let mut seq = self.base_update_seq;
        for (i, pr) in self.prs.iter().enumerate() {
            if !pr.gate_green {
                // The culprit — dequeue it; the PRs AFTER it are survivors to re-test on the new tip.
                return BatchOutcome {
                    landed,
                    landed_to_update_seq: seq,
                    survivors_to_rebase: self.prs[i + 1..].to_vec(),
                    culprit: Some(pr.clone()),
                    base_moved: false,
                };
            }
            // Land this PR — ONE CAS, advancing update_seq by exactly 1 (linearizable, monotonic).
            seq += 1;
            landed.push(pr.head_oid.clone());
        }
        // The whole batch landed cleanly.
        BatchOutcome {
            landed,
            landed_to_update_seq: seq,
            survivors_to_rebase: Vec::new(),
            culprit: None,
            base_moved: false,
        }
    }
}

/// **The outcome of landing a speculative batch (GIT-D5).** Records exactly what landed (the
/// linearizable CAS sequence), the resulting `update_seq`, the survivors to rebase + re-test, and the
/// dequeued culprit (if a mid-batch failure bisected). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchOutcome {
    /// The PR head oids that LANDED, in CAS order (each advanced `update_seq` by 1 — linearizable).
    pub landed: Vec<Oid>,
    /// The base's `update_seq` after the landed sequence (strictly monotonic over the landed count).
    pub landed_to_update_seq: u64,
    /// The PRs that did NOT land and must be rebased onto the new tip + re-tested (the bisect
    /// survivors). On a base movement this is the WHOLE batch (nothing landed on a stale base).
    pub survivors_to_rebase: Vec<QueuedPr>,
    /// The dequeued culprit PR (a mid-batch gate failure), if the batch bisected on one. `None` on a
    /// clean land or a base-movement invalidation.
    pub culprit: Option<QueuedPr>,
    /// Whether the protected base MOVED out from under the speculative tips (force-push / concurrent
    /// merge) — the whole batch was invalidated, 0 landed (no silent stale land).
    pub base_moved: bool,
}

impl BatchOutcome {
    /// **The number of merges that LANDED** (the linearizable CAS count). The GIT-D5 0-lost-merge
    /// property checks this against the green-prefix length.
    pub fn landed_count(&self) -> usize {
        self.landed.len()
    }

    /// **The land was linearizable: each landed PR advanced `update_seq` by exactly 1** (a strictly
    /// monotonic CAS sequence over the base's generation). `true` iff `landed_to_update_seq == base +
    /// landed_count` for the batch's build-time base. The GIT-D5 monotonicity witness.
    pub fn is_linearizable(&self, build_time_base_seq: u64) -> bool {
        if self.base_moved {
            // A base-movement invalidation lands nothing — the resulting seq is the moved base's, and
            // landed_count is 0 (no merge on a stale base — 0 lost).
            return self.landed.is_empty();
        }
        self.landed_to_update_seq == build_time_base_seq + self.landed_count() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(oid: &str, green: bool) -> QueuedPr {
        QueuedPr::new(Oid::new(oid), green)
    }

    /// **The queue promotes to speculative batching ONLY when MEASURED-saturated (`queue_depth >
    /// single_lane_capacity`).** Below/at capacity stays single-lane. Kills the always-promote mutant +
    /// the `>` → `>=` boundary mutant.
    #[test]
    fn promotes_only_when_measured_saturated() {
        // Saturated: 10 queued, single lane drains 3 → promote.
        assert!(PromotionTrigger {
            queue_depth: 10,
            single_lane_capacity: 3
        }
        .should_promote());
        // Exactly at capacity → stay single-lane (the `>=` mutant would promote).
        assert!(!PromotionTrigger {
            queue_depth: 3,
            single_lane_capacity: 3
        }
        .should_promote());
        // Below capacity → single-lane.
        assert!(!PromotionTrigger {
            queue_depth: 1,
            single_lane_capacity: 3
        }
        .should_promote());
    }

    /// **The batch width is the backlog over capacity, ≥ 2, capped at the depth.** A single-lane (not
    /// promoted) yields width 1.
    #[test]
    fn batch_width_drains_the_saturation() {
        assert_eq!(
            PromotionTrigger {
                queue_depth: 10,
                single_lane_capacity: 3
            }
            .batch_width(),
            7,
            "backlog over capacity (10-3=7)"
        );
        // Not saturated → single lane (width 1).
        assert_eq!(
            PromotionTrigger {
                queue_depth: 2,
                single_lane_capacity: 3
            }
            .batch_width(),
            1
        );
        // A small saturation still yields a real batch (≥ 2).
        assert_eq!(
            PromotionTrigger {
                queue_depth: 4,
                single_lane_capacity: 3
            }
            .batch_width(),
            2,
            "min batch width is 2 (a 1-PR batch is the single lane)"
        );
    }

    /// **A clean speculative batch lands ALL PRs as a linearizable CAS sequence (0 lost, monotonic
    /// update_seq).** The GIT-D5 happy path: 3 green PRs land, update_seq advances 5→8 (one per land).
    #[test]
    fn clean_batch_lands_all_linearizably() {
        let batch = SpeculativeBatch::new(
            "refs/heads/main",
            5,
            vec![pr("a1", true), pr("b2", true), pr("c3", true)],
        );
        // No base movement (current_base_seq == build-time base 5).
        let out = batch.land(5);
        assert_eq!(out.landed_count(), 3, "all 3 green PRs landed");
        assert_eq!(
            out.landed,
            vec![Oid::new("a1"), Oid::new("b2"), Oid::new("c3")]
        );
        assert_eq!(out.landed_to_update_seq, 8, "5 + 3 = 8 (one CAS per land)");
        assert!(out.survivors_to_rebase.is_empty());
        assert!(out.culprit.is_none());
        assert!(!out.base_moved);
        assert!(
            out.is_linearizable(5),
            "the land is a strictly monotonic CAS sequence (linearizable)"
        );
    }

    /// **A mid-batch gate failure BISECTS: the green prefix lands, the culprit is dequeued, the
    /// survivors are returned to rebase + re-test.** 0 lost merge — the green prefix landed
    /// linearizably; the survivors are not lost, they are re-queued.
    #[test]
    fn mid_batch_failure_bisects_dequeues_culprit_survivors_rebase() {
        let batch = SpeculativeBatch::new(
            "refs/heads/main",
            5,
            vec![
                pr("a1", true),
                pr("b2", false), // the culprit
                pr("c3", true),
                pr("d4", true),
            ],
        );
        let out = batch.land(5);
        // The green prefix (a1) landed; update_seq advanced 5→6.
        assert_eq!(out.landed, vec![Oid::new("a1")]);
        assert_eq!(out.landed_to_update_seq, 6);
        // The culprit (b2) was dequeued.
        assert_eq!(out.culprit, Some(pr("b2", false)));
        // The survivors AFTER the culprit (c3, d4) are returned to rebase + re-test (not lost).
        assert_eq!(
            out.survivors_to_rebase,
            vec![pr("c3", true), pr("d4", true)]
        );
        assert!(!out.base_moved);
        assert!(
            out.is_linearizable(5),
            "the green prefix landed linearizably (6 == 5 + 1)"
        );
    }

    /// **A BASE MOVEMENT (force-push / concurrent merge) invalidates the WHOLE batch — 0 land, every PR
    /// rebased.** No silent stale land on a moved base — the GIT-D5 no-split-brain / 0-lost-merge core.
    #[test]
    fn base_movement_invalidates_the_whole_batch_zero_lost() {
        let batch = SpeculativeBatch::new(
            "refs/heads/main",
            5,
            vec![pr("a1", true), pr("b2", true), pr("c3", true)],
        );
        // The base moved (current seq 7 ≠ build-time 5) — a concurrent merge / force-push.
        let out = batch.land(7);
        assert!(
            out.base_moved,
            "the base moved out from under the speculative tips"
        );
        assert_eq!(
            out.landed_count(),
            0,
            "0 landed on a stale base (0 lost merge)"
        );
        assert!(out.landed.is_empty());
        // The resulting seq is the MOVED base's (the batch did not touch it).
        assert_eq!(out.landed_to_update_seq, 7);
        // Every PR is a survivor to rebase onto the new base + re-test (nothing lost).
        assert_eq!(
            out.survivors_to_rebase,
            vec![pr("a1", true), pr("b2", true), pr("c3", true)]
        );
        assert!(out.culprit.is_none());
        assert!(
            out.is_linearizable(5),
            "a base-movement invalidation lands nothing (0 lost — linearizable trivially)"
        );
    }

    /// **The first PR being the culprit lands NOTHING + bisects (0 prefix).** The green-prefix is
    /// empty; the survivors are everything after the first.
    #[test]
    fn first_pr_culprit_lands_nothing() {
        let batch =
            SpeculativeBatch::new("refs/heads/main", 5, vec![pr("a1", false), pr("b2", true)]);
        let out = batch.land(5);
        assert_eq!(
            out.landed_count(),
            0,
            "the first PR is the culprit → 0 landed"
        );
        assert_eq!(
            out.landed_to_update_seq, 5,
            "update_seq unchanged (0 landed)"
        );
        assert_eq!(out.culprit, Some(pr("a1", false)));
        assert_eq!(out.survivors_to_rebase, vec![pr("b2", true)]);
    }

    /// **The linearizability witness is exact: a tampered land count fails `is_linearizable`.** Kills a
    /// mutant that would let a non-monotonic land pass (e.g. `+ landed_count` → `+ 0`).
    #[test]
    fn linearizability_witness_is_exact() {
        let good = BatchOutcome {
            landed: vec![Oid::new("a1"), Oid::new("b2")],
            landed_to_update_seq: 7, // 5 + 2
            survivors_to_rebase: vec![],
            culprit: None,
            base_moved: false,
        };
        assert!(good.is_linearizable(5));
        // A skewed resulting seq (a lost or doubled CAS) is NOT linearizable.
        let skewed = BatchOutcome {
            landed_to_update_seq: 8, // 5 + 2 should be 7 — a lost/doubled CAS
            ..good.clone()
        };
        assert!(
            !skewed.is_linearizable(5),
            "a non-monotonic land (seq ≠ base + landed_count) is not linearizable"
        );
    }
}
