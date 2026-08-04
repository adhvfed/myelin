use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SignalName {
    RequestRate,
    RequestErrors,
    RequestDuration,
    PoolSaturation,
    ConsumerLag,
    OutboxDepth,
    DeadLetterCount,
    BreakerState,
    FailStaticRatio,
    FailStaticStalenessSecs,
    ShedCount,
    CausalDepthFirings,
    DispatchPoolDrops,
    CrossTenantCount,
    FirehoseFrameLag,
    ResyncRequiredCount,
    Readiness,
    LivenessRestartCount,
    RestoreCrossSeamMismatch,
    RestoreRpoSecs,
    RestoreRtoSecs,
    DedupCollapseRatio,
    MigrationLockWaitP99Ms,
    MigrationErroredWrites,
    MigrationDowntimeMs,
}

impl SignalName {
    pub const ALL: [SignalName; 25] = [
        SignalName::RequestRate,
        SignalName::RequestErrors,
        SignalName::RequestDuration,
        SignalName::PoolSaturation,
        SignalName::ConsumerLag,
        SignalName::OutboxDepth,
        SignalName::DeadLetterCount,
        SignalName::BreakerState,
        SignalName::FailStaticRatio,
        SignalName::FailStaticStalenessSecs,
        SignalName::ShedCount,
        SignalName::CausalDepthFirings,
        SignalName::DispatchPoolDrops,
        SignalName::CrossTenantCount,
        SignalName::FirehoseFrameLag,
        SignalName::ResyncRequiredCount,
        SignalName::Readiness,
        SignalName::LivenessRestartCount,
        SignalName::RestoreCrossSeamMismatch,
        SignalName::RestoreRpoSecs,
        SignalName::RestoreRtoSecs,
        SignalName::DedupCollapseRatio,
        SignalName::MigrationLockWaitP99Ms,
        SignalName::MigrationErroredWrites,
        SignalName::MigrationDowntimeMs,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Label {
    pub key: String,
    pub value: String,
}

impl Label {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Label {
        Label {
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Predicate {
    Eq(i64),
    Lte(i64),
    Gte(i64),
    Lt(i64),
    Gt(i64),
    InRange(i64, i64),
    #[doc(hidden)]
    AlwaysTrue,
}

impl Predicate {
    fn is_satisfied_by(&self, value: i64) -> bool {
        match *self {
            Predicate::Eq(n) => value == n,
            Predicate::Lte(n) => value <= n,
            Predicate::Gte(n) => value >= n,
            Predicate::Lt(n) => value < n,
            Predicate::Gt(n) => value > n,
            Predicate::InRange(lo, hi) => value >= lo && value <= hi,
            Predicate::AlwaysTrue => false,
        }
    }

    fn is_vacuous(&self) -> bool {
        matches!(self, Predicate::AlwaysTrue)
    }
}

#[must_use = "a telemetry assertion verdict must be checked - a dropped red is a swallowed failure (EI-01 §3)"]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Assertion {
    Green {
        signal: AssertedSignal,
        predicate: Predicate,
        observed: i64,
    },
    Red {
        signal: AssertedSignal,
        predicate: Predicate,
        observed: Option<i64>,
    },
    Rejected {
        signal: AssertedSignal,
        reason: RejectReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    VacuousPredicate,
    LabelShapeMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertedSignal {
    pub name: SignalName,
    pub labels: Vec<Label>,
}

impl AssertedSignal {
    fn scalar(name: SignalName) -> AssertedSignal {
        AssertedSignal {
            name,
            labels: Vec::new(),
        }
    }
}

impl Assertion {
    pub fn is_green(&self) -> bool {
        matches!(self, Assertion::Green { .. })
    }

    fn rejected(signal: AssertedSignal, reason: RejectReason) -> Assertion {
        Assertion::Rejected { signal, reason }
    }

    pub fn expect_green(self) {
        match self {
            Assertion::Green { .. } => {}
            Assertion::Red {
                signal,
                predicate,
                observed,
            } => panic!(
                "DRILL RED: signal {:?}{} did not satisfy {:?} (observed {}) - \
                 the property is broken; fix the deliverable, do NOT weaken the predicate (EI-01 §3)",
                signal.name,
                fmt_labels(&signal.labels),
                predicate,
                match observed {
                    Some(v) => v.to_string(),
                    None => "ABSENT (signal not emitted - you cannot operate what you cannot see)"
                        .to_string(),
                },
            ),
            Assertion::Rejected { signal, reason } => panic!(
                "DRILL REJECTED: signal {:?}{} - {:?}; an assertion that asserts nothing is not a pass (EI-01 §3)",
                signal.name,
                fmt_labels(&signal.labels),
                reason,
            ),
        }
    }
}

fn fmt_labels(labels: &[Label]) -> String {
    if labels.is_empty() {
        String::new()
    } else {
        let inner: Vec<String> = labels
            .iter()
            .map(|l| format!("{}={}", l.key, l.value))
            .collect();
        format!("{{{}}}", inner.join(", "))
    }
}

#[derive(Default, Debug, Clone)]
pub struct SignalSource {
    scalars: HashMap<SignalName, i64>,
    labelled: HashMap<(SignalName, Vec<Label>), i64>,
}

impl SignalSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_scalar(&mut self, name: SignalName, value: i64) {
        self.scalars.insert(name, value);
    }

    pub fn set_labelled(&mut self, name: SignalName, labels: Vec<Label>, value: i64) {
        let mut labels = labels;
        labels.sort();
        self.labelled.insert((name, labels), value);
    }

    pub fn scalar(&self, name: SignalName) -> Option<i64> {
        self.scalars.get(&name).copied()
    }

    pub fn labelled(&self, name: SignalName, labels: &[Label]) -> Option<i64> {
        let mut labels = labels.to_vec();
        labels.sort();
        self.labelled.get(&(name, labels)).copied()
    }

    pub fn assert_signal(&self, name: SignalName, predicate: Predicate) -> Assertion {
        let signal = AssertedSignal::scalar(name);
        if predicate.is_vacuous() {
            return Assertion::rejected(signal, RejectReason::VacuousPredicate);
        }
        match self.scalar(name) {
            None => Assertion::Red {
                signal,
                predicate,
                observed: None,
            },
            Some(observed) => {
                if predicate.is_satisfied_by(observed) {
                    Assertion::Green {
                        signal,
                        predicate,
                        observed,
                    }
                } else {
                    Assertion::Red {
                        signal,
                        predicate,
                        observed: Some(observed),
                    }
                }
            }
        }
    }

    pub fn assert_labelled(
        &self,
        name: SignalName,
        labels: Vec<Label>,
        predicate: Predicate,
    ) -> Assertion {
        let mut labels = labels;
        labels.sort();
        let signal = AssertedSignal {
            name,
            labels: labels.clone(),
        };
        if predicate.is_vacuous() {
            return Assertion::rejected(signal, RejectReason::VacuousPredicate);
        }
        match self.labelled(name, &labels) {
            None => Assertion::Red {
                signal,
                predicate,
                observed: None,
            },
            Some(observed) => {
                if predicate.is_satisfied_by(observed) {
                    Assertion::Green {
                        signal,
                        predicate,
                        observed,
                    }
                } else {
                    Assertion::Red {
                        signal,
                        predicate,
                        observed: Some(observed),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_signal_reads_green_on_satisfied_predicate() {
        let mut src = SignalSource::new();
        src.set_scalar(SignalName::OutboxDepth, 0);
        let verdict = src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0));
        assert!(verdict.is_green());
        verdict.expect_green();
    }

    #[test]
    fn assert_signal_fails_loudly_on_red_signal() {
        let mut src = SignalSource::new();
        src.set_scalar(SignalName::OutboxDepth, 7);
        let verdict = src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0));
        assert!(!verdict.is_green(), "a non-zero outbox depth must read RED");
        match &verdict {
            Assertion::Red { observed, .. } => assert_eq!(*observed, Some(7)),
            other => panic!("expected Red, got {other:?}"),
        }
    }

    #[test]
    #[should_panic(expected = "DRILL RED")]
    fn expect_green_panics_on_red() {
        let mut src = SignalSource::new();
        src.set_scalar(SignalName::OutboxDepth, 3);
        src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
            .expect_green();
    }

    #[test]
    fn absent_signal_reads_red_not_green() {
        let src = SignalSource::new();
        let verdict = src.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0));
        assert!(!verdict.is_green(), "an absent signal must NOT read green");
        match verdict {
            Assertion::Red { observed: None, .. } => {}
            other => panic!("expected Red{{observed: None}}, got {other:?}"),
        }
    }

    #[test]
    fn inverted_assertion_is_rejected_not_green() {
        let mut src = SignalSource::new();
        src.set_scalar(SignalName::OutboxDepth, 0);
        let verdict = src.assert_signal(SignalName::OutboxDepth, Predicate::AlwaysTrue);
        assert!(
            !verdict.is_green(),
            "a vacuous assertion must NOT read green"
        );
        match verdict {
            Assertion::Rejected {
                reason: RejectReason::VacuousPredicate,
                ..
            } => {}
            other => panic!("expected Rejected{{VacuousPredicate}}, got {other:?}"),
        }
    }

    #[test]
    #[should_panic(expected = "DRILL REJECTED")]
    fn expect_green_panics_on_rejected() {
        let src = SignalSource::new();
        src.assert_signal(SignalName::OutboxDepth, Predicate::AlwaysTrue)
            .expect_green();
    }

    #[test]
    fn labelled_signals_read_per_principal_kind_and_tenant() {
        let mut src = SignalSource::new();
        let human = vec![Label::new("kind", "human"), Label::new("tenant", "acme")];
        let agent_lane = vec![Label::new("lane", "agent")];
        src.set_labelled(SignalName::RequestErrors, human.clone(), 0);
        src.set_labelled(SignalName::ShedCount, agent_lane.clone(), 1500);

        src.assert_labelled(SignalName::RequestErrors, human, Predicate::Eq(0))
            .expect_green();
        src.assert_labelled(SignalName::ShedCount, agent_lane, Predicate::Gte(1))
            .expect_green();
    }

    #[test]
    fn labelled_read_is_order_independent() {
        let mut src = SignalSource::new();
        src.set_labelled(
            SignalName::RequestErrors,
            vec![Label::new("tenant", "acme"), Label::new("kind", "human")],
            0,
        );
        src.assert_labelled(
            SignalName::RequestErrors,
            vec![Label::new("kind", "human"), Label::new("tenant", "acme")],
            Predicate::Eq(0),
        )
        .expect_green();
    }

    #[test]
    fn predicate_vocabulary_covers_the_bounds() {
        let mut src = SignalSource::new();
        src.set_scalar(SignalName::FailStaticStalenessSecs, 30);
        src.set_scalar(SignalName::CausalDepthFirings, 1);
        src.set_scalar(SignalName::CrossTenantCount, 0);

        src.assert_signal(SignalName::FailStaticStalenessSecs, Predicate::Lte(60))
            .expect_green();
        src.assert_signal(SignalName::CausalDepthFirings, Predicate::Gte(1))
            .expect_green();
        src.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .expect_green();
        src.set_scalar(SignalName::FailStaticStalenessSecs, 120);
        assert!(!src
            .assert_signal(SignalName::FailStaticStalenessSecs, Predicate::Lte(60))
            .is_green());
        src.set_scalar(SignalName::FailStaticRatio, 95);
        src.assert_signal(SignalName::FailStaticRatio, Predicate::InRange(90, 100))
            .expect_green();
    }

    #[test]
    fn covers_the_full_contract_1_8_signal_set() {
        let mut uniq = SignalName::ALL.to_vec();
        let n = uniq.len();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), n, "every contract-1.8 signal name is distinct");
        assert_eq!(
            n, 25,
            "the §10.2 set + the restore-verify triplet + the Notif dedup-collapse-ratio + the \
             online-migration-under-load triplet (lock-wait p99 + errored-writes + downtime, SUB-D10 / \
             contract 1.5) is covered exhaustively"
        );
    }
}
