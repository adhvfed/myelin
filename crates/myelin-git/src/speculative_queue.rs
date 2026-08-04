use crate::receive_pack::Oid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromotionTrigger {
    pub queue_depth: u32,
    pub single_lane_capacity: u32,
}

impl PromotionTrigger {
    pub fn should_promote(self) -> bool {
        self.queue_depth > self.single_lane_capacity
    }

    pub fn batch_width(self) -> u32 {
        if !self.should_promote() {
            return 1;
        }
        let over = self.queue_depth - self.single_lane_capacity;
        over.max(2).min(self.queue_depth)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedPr {
    pub head_oid: Oid,
    pub gate_green: bool,
}

impl QueuedPr {
    pub fn new(head_oid: Oid, gate_green: bool) -> QueuedPr {
        QueuedPr {
            head_oid,
            gate_green,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpeculativeBatch {
    base_ref: String,
    base_update_seq: u64,
    prs: Vec<QueuedPr>,
}

impl SpeculativeBatch {
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

    pub fn base_ref(&self) -> &str {
        &self.base_ref
    }

    pub fn land(&self, current_base_seq: u64) -> BatchOutcome {
        if current_base_seq != self.base_update_seq {
            return BatchOutcome {
                landed: Vec::new(),
                landed_to_update_seq: current_base_seq,
                survivors_to_rebase: self.prs.clone(),
                culprit: None,
                base_moved: true,
            };
        }

        let mut landed = Vec::new();
        let mut seq = self.base_update_seq;
        for (i, pr) in self.prs.iter().enumerate() {
            if !pr.gate_green {
                return BatchOutcome {
                    landed,
                    landed_to_update_seq: seq,
                    survivors_to_rebase: self.prs[i + 1..].to_vec(),
                    culprit: Some(pr.clone()),
                    base_moved: false,
                };
            }
            seq += 1;
            landed.push(pr.head_oid.clone());
        }
        BatchOutcome {
            landed,
            landed_to_update_seq: seq,
            survivors_to_rebase: Vec::new(),
            culprit: None,
            base_moved: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchOutcome {
    pub landed: Vec<Oid>,
    pub landed_to_update_seq: u64,
    pub survivors_to_rebase: Vec<QueuedPr>,
    pub culprit: Option<QueuedPr>,
    pub base_moved: bool,
}

impl BatchOutcome {
    pub fn landed_count(&self) -> usize {
        self.landed.len()
    }

    pub fn is_linearizable(&self, build_time_base_seq: u64) -> bool {
        if self.base_moved {
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

    #[test]
    fn promotes_only_when_measured_saturated() {
        assert!(PromotionTrigger {
            queue_depth: 10,
            single_lane_capacity: 3
        }
        .should_promote());
        assert!(!PromotionTrigger {
            queue_depth: 3,
            single_lane_capacity: 3
        }
        .should_promote());
        assert!(!PromotionTrigger {
            queue_depth: 1,
            single_lane_capacity: 3
        }
        .should_promote());
    }

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
        assert_eq!(
            PromotionTrigger {
                queue_depth: 2,
                single_lane_capacity: 3
            }
            .batch_width(),
            1
        );
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

    #[test]
    fn clean_batch_lands_all_linearizably() {
        let batch = SpeculativeBatch::new(
            "refs/heads/main",
            5,
            vec![pr("a1", true), pr("b2", true), pr("c3", true)],
        );
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

    #[test]
    fn mid_batch_failure_bisects_dequeues_culprit_survivors_rebase() {
        let batch = SpeculativeBatch::new(
            "refs/heads/main",
            5,
            vec![
                pr("a1", true),
                pr("b2", false),
                pr("c3", true),
                pr("d4", true),
            ],
        );
        let out = batch.land(5);
        assert_eq!(out.landed, vec![Oid::new("a1")]);
        assert_eq!(out.landed_to_update_seq, 6);
        assert_eq!(out.culprit, Some(pr("b2", false)));
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

    #[test]
    fn base_movement_invalidates_the_whole_batch_zero_lost() {
        let batch = SpeculativeBatch::new(
            "refs/heads/main",
            5,
            vec![pr("a1", true), pr("b2", true), pr("c3", true)],
        );
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
        assert_eq!(out.landed_to_update_seq, 7);
        assert_eq!(
            out.survivors_to_rebase,
            vec![pr("a1", true), pr("b2", true), pr("c3", true)]
        );
        assert!(out.culprit.is_none());
        assert!(
            out.is_linearizable(5),
            "a base-movement invalidation lands nothing (0 lost - linearizable trivially)"
        );
    }

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

    #[test]
    fn linearizability_witness_is_exact() {
        let good = BatchOutcome {
            landed: vec![Oid::new("a1"), Oid::new("b2")],
            landed_to_update_seq: 7,
            survivors_to_rebase: vec![],
            culprit: None,
            base_moved: false,
        };
        assert!(good.is_linearizable(5));
        let skewed = BatchOutcome {
            landed_to_update_seq: 8,
            ..good.clone()
        };
        assert!(
            !skewed.is_linearizable(5),
            "a non-monotonic land (seq ≠ base + landed_count) is not linearizable"
        );
    }
}
