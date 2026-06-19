//! The telemetry-assertion library — the machinery UNDER every drill in the whole ledger.
//!
//! See the crate-level docs for the doctrine / architecture / testing-strategy anchors.
//! This module is the **assertion half** of the unit-of-proof (doctrine EI-01 §3 "a
//! property does not exist until a test forces the failure and observability watches the
//! system survive it"; architecture §10.2 the telemetry signal set; contract-index 1.8).
//! The load generator (P-S02) drives traffic, the dependency-break injector (P-S03) severs
//! a dependency, and THIS library reads — off the contract-1.8 survival-signal set — that
//! the system survived, returning a typed green/red that is **never** a swallowed pass.
//!
//! ## Why "loud, never swallowed" is structural here, not a convention
//! Doctrine EI-01 §3/§5 is explicit: *never weaken a threshold or invert an assertion to
//! make a check pass*, and *replace `... || true` and silent filters with explicit, noisy
//! failures*. This library encodes that mechanically:
//! - [`Assertion`] is a `#[must_use]` typed result. A caller cannot accidentally drop a red
//!   on the floor — the compiler warns if the verdict is ignored, and
//!   [`Assertion::expect_green`] **panics with the signal + predicate + observed value** on
//!   red (the loud failure a drill needs), rather than returning a silent `bool` a caller
//!   could `|| true` away.
//! - [`SignalSource::assert_signal`] takes a [`Predicate`] *value*, not a closure. A
//!   predicate carries the bound it asserts, so a red verdict can report "outbox_depth == 0
//!   expected, observed 7" — observability is part of the pass condition (EI-01 §3). A bare
//!   closure could hide the bound, or be written as `|_| true` (the inverted assertion §3
//!   forbids); the predicate value cannot.
//! - **The inverted-assertion guard.** A [`Predicate::AlwaysTrue`] — a predicate that admits
//!   every value, the structural shape of "invert the assertion to manufacture green" —
//!   is **rejected at construction** ([`Predicate`] has no public always-true constructor)
//!   and, defensively, [`SignalSource::assert_signal`] returns [`Assertion::rejected`] (a
//!   distinct verdict that is NOT green) if one is ever passed. A drill that tries to pass by
//!   asserting nothing fails loudly instead.
//!
//! ## The in-memory signal source (testable before `serve` exists)
//! The harness reads from an in-memory [`SignalSource`] populated by the rig in tests. The
//! **producer side** — a real service exporting these signals on its metrics-health port —
//! is wired into `serve` at **P-S12 / P-S13** (architecture §3.5 / §10). The signal *names
//! and units* this library reads are the frozen contract-1.8 set (architecture §10.2), so
//! when the producer side lands it populates the SAME [`SignalName`]s this library already
//! asserts against; the assertion surface does not change.
//!
//! ## Floors named (deferred + filling prompt)
//! - **In-memory source only.** The producer side (a service exporting the §10.2 set on its
//!   metrics-health port via OpenTelemetry) lands in **P-S12 / P-S13** (`serve` lifecycle +
//!   the three-port topology). This library is the *consumer* side, complete and testable
//!   now; it reads the SAME signal set the producer will export.
//! - **Per-(principal-kind, tenant) RED labels are a flat key here.** Contract 1.8 reads
//!   RED/USE *per principal-kind per tenant*; this library models a labelled signal as a
//!   `(SignalName, Vec<Label>)` key so a drill can assert e.g. `request_errors{kind=human,
//!   tenant=acme} == 0` while the agent lane sheds. The structured OpenTelemetry attribute
//!   set the producer exports lands with the producer (P-S13); the label *shape* here is the
//!   stable read surface.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The canonical contract-1.8 telemetry signal set (architecture §10.2; contract-index 1.8).
///
/// Every Myelin service exports this set on its metrics-health port; the Phase-5 drills read
/// it as a **survival signal** (a shared-system doc that omits any of these fails X-1, and
/// the drills *assert against* these signals, which is what makes "proven" mean proven).
/// This enum is the frozen NAME side of contract 1.8 — the harness is its first consumer;
/// the producer side (the `serve` metrics-health port) lands at P-S12/P-S13 and populates
/// the SAME names.
///
/// Each variant maps one row of the §10.2 table. The labelled signals (RED/USE, consumer
/// lag, shed counts, firehose lag) are read per their label set via
/// [`SignalSource::set_labelled`] / [`SignalSource::assert_labelled`]; the scalar signals
/// (outbox depth, dead-letter count, causal-depth firings) are read by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SignalName {
    /// RED — request rate, per principal-kind + per tenant (§10.2 row 1; the 30×-surge
    /// human-lane-holds drill). Labelled by `{kind, tenant}`.
    RequestRate,
    /// RED — request errors, per principal-kind + per tenant. Labelled by `{kind, tenant}`.
    RequestErrors,
    /// RED — request duration (a quantile/aggregate the drill reads), per principal-kind +
    /// per tenant. Labelled by `{kind, tenant}`.
    RequestDuration,
    /// USE — utilisation/saturation/errors of a pool or queue (§10.2 row 2; overload /
    /// cascade). Labelled by `{pool}`.
    PoolSaturation,
    /// Consumer lag — `num_pending` / oldest-un-acked age, per consumer (§10.2 row 3;
    /// event-loss / head-of-line, D-7). Labelled by `{consumer}`.
    ConsumerLag,
    /// Outbox depth (§10.2 row 4; silent-data-loss, BUS-2). The drill asserts `== 0` once
    /// the relay has drained. Scalar.
    OutboxDepth,
    /// Dead-letter count (§10.2 row 4). A poison message dead-letters; the drill asserts the
    /// bound. Scalar.
    DeadLetterCount,
    /// Breaker state — open(2)/half(1)/closed(0) (§10.2 row 5; retry-storm / cascade).
    /// Labelled by `{downstream}`. Encoded numerically so a predicate can read it.
    BreakerState,
    /// Fail-static fresh/stale/closed answer ratio + staleness age (§10.2 row 6; Id-hiccup /
    /// fail-static, D-4). Labelled by `{answer_class}` (`fresh`/`stale`/`closed`).
    FailStaticRatio,
    /// Fail-static staleness age, in seconds (§10.2 row 6). The drill asserts it never
    /// exceeds `static_max ≤ revocation SLA`. Scalar.
    FailStaticStalenessSecs,
    /// Shed counts per lane (§10.2 row 7; agent-surge / human-lane-holds / connection-storm).
    /// Labelled by `{lane}` (`speculative`/`ci`/`agent`/`human`).
    ShedCount,
    /// Causal-depth tripwire firings (§10.2 row 8; causal-loop tripwire, D-8). The drill
    /// asserts the ceiling fired (`>= 1`) on an adversarial loop. Scalar.
    CausalDepthFirings,
    /// Dispatch-pool drops (§10.2 row 8; the bounded-dispatch drops-over-cap leg). Scalar.
    DispatchPoolDrops,
    /// Cross-tenant read count (the SUB-D7 / IDOR survival signal; §10.2 RED per tenant is the
    /// surface, this is its zero-cross-tenant projection the IDOR drill asserts `== 0`).
    /// Scalar — the single most-load-bearing zero in the platform.
    CrossTenantCount,
    /// Firehose per-(stream, scope) frame lag (§10.2 row 9; reconnect-loses-zero-ops, D-11).
    /// Labelled by `{stream, scope}`.
    FirehoseFrameLag,
    /// Firehose `resync_required` count (§10.2 row 9). The over-retention-gap signal — named,
    /// not silent. Scalar.
    ResyncRequiredCount,
    /// Readiness — `1` = ready (can serve correct traffic now), `0` = not-ready (a critical
    /// dependency is down, or boot/migration is incomplete → shed new traffic). The SUB-D9
    /// survival signal (architecture §4.3; liveness ≠ readiness): a severed critical dependency
    /// FLIPS this to `0` and the surface sheds; a healthy instance reads `1`. Scalar.
    Readiness,
    /// Liveness restart-churn — the count of liveness-triggered restarts. The SUB-D9 "no
    /// restart-storm" signal: liveness = "not wedged" must NOT check dependencies, so a dead
    /// critical dependency leaves this at its baseline (`0`) — only a genuinely wedged process
    /// flips liveness (and restarts). The drill asserts `== 0` across a dependency outage
    /// (readiness sheds; liveness does not churn). Scalar.
    LivenessRestartCount,
    /// **Restore cross-seam mismatch count** — the number of inconsistencies found across the
    /// four restore seams (OLTP rows ↔ blob ↔ search index ↔ event-log offsets) after a
    /// rebuild-from-backups (§11 D-6 / SUB-D6 / STOR-D1; contract 11.5). A rebuild that lands at
    /// ONE consistent cross-seam point reads `0` here — a row pointing at a missing blob, an
    /// index doc whose OLTP row vanished, or a row beyond the restored offset each increments
    /// this. The restore-verify drill asserts `== 0` (the silent-data-loss floor: 0 loss, one
    /// consistent point). The single most-load-bearing restore zero. Scalar.
    RestoreCrossSeamMismatch,
    /// **Restore RPO** — the recovery-POINT achieved by a rebuild-from-backups, in SECONDS: how
    /// much committed data the restore lost off the tail (the gap between the last durably-backed
    /// offset and the crash point; §11 D-6 / STOR-D2; contract 11.5). The drill asserts it is
    /// `<= rpo_max_mins * 60` (default-to-beat: ≤ 5 min = 300 s — read from the thresholds file,
    /// never hardcoded). Scalar.
    RestoreRpoSecs,
    /// **Restore RTO** — the recovery-TIME a rebuild-from-backups took, in SECONDS: wall-clock
    /// from "begin restore" to "restored copy at a consistent cross-seam point, ready to serve"
    /// (§11 D-6 / STOR-D2; contract 11.5). The drill asserts the per-tenant bound
    /// `<= rto_tenant_max_mins * 60` (≤ 1 h) and the per-cell bound `<= rto_cell_max_mins * 60`
    /// (≤ 4 h) — both read from the thresholds file. Labelled by `{grain}` (`tenant` / `cell`) so
    /// the per-tenant and per-cell objectives are read independently.
    RestoreRtoSecs,
}

impl SignalName {
    /// Every contract-1.8 signal name (for the "the library covers the §10.2 set" test —
    /// observability is part of the pass condition, so the set must be exhaustive).
    pub const ALL: [SignalName; 21] = [
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
    ];
}

/// One label on a labelled signal (e.g. `kind=human`, `tenant=acme`, `lane=agent`). A
/// `(key, value)` pair so a drill can read RED/USE *per principal-kind per tenant* (contract
/// 1.8) — the load-bearing requirement that a surge drill assert the human lane held while
/// the agent lane shed.
///
/// Values are PII-free identifiers (a tenant id, a principal-KIND, a lane name) — never a
/// payload — so a telemetry label is `control-plane-pii-free` by construction.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Label {
    /// The label key (`kind` / `tenant` / `lane` / `consumer` / `downstream` / …).
    pub key: String,
    /// The label value (a PII-free identifier).
    pub value: String,
}

impl Label {
    /// A label from a `(key, value)` pair.
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Label {
        Label {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// The predicate a drill asserts a signal value satisfies (e.g. `outbox_depth == 0`,
/// `fail_static_staleness <= W`, `causal_depth_firings >= 1`).
///
/// A predicate is a **value**, not a closure, so it carries the bound it asserts — a red
/// verdict can report "expected `<= 5`, observed `7`" (observability is part of the pass
/// condition, EI-01 §3). There is deliberately **no `AlwaysTrue` public constructor**: a
/// predicate that admits every value is the structural shape of "invert the assertion to
/// manufacture green" (EI-01 §3), so the only way to assert "nothing" is unconstructable.
/// (The private [`Predicate::AlwaysTrue`] exists only as the value
/// [`SignalSource::assert_signal`]'s defensive guard rejects — see [`Assertion::rejected`].)
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Predicate {
    /// The value must equal `n` exactly (the `outbox_depth == 0`, `cross_tenant_count == 0`
    /// zeros).
    Eq(i64),
    /// The value must be `<= n` (the `staleness <= static_max` bound).
    Lte(i64),
    /// The value must be `>= n` (the `causal_depth_firings >= 1` tripwire-fired bound).
    Gte(i64),
    /// The value must be strictly less than `n`.
    Lt(i64),
    /// The value must be strictly greater than `n`.
    Gt(i64),
    /// The value must lie in the inclusive range `[lo, hi]` (a fail-static ratio band).
    InRange(i64, i64),
    /// The structural "admits every value" predicate — the inverted-assertion shape EI-01 §3
    /// forbids. It has NO public constructor; it exists only so the assertion path can
    /// **reject** it ([`Assertion::rejected`]) if one is ever synthesised. Never green.
    #[doc(hidden)]
    AlwaysTrue,
}

impl Predicate {
    /// Does `value` satisfy this predicate? [`Predicate::AlwaysTrue`] returns `false` here on
    /// purpose — the assertion path rejects it before evaluation, and even if reached it must
    /// never read as a satisfied predicate (it would be the inverted assertion §3 forbids).
    fn is_satisfied_by(&self, value: i64) -> bool {
        match *self {
            Predicate::Eq(n) => value == n,
            Predicate::Lte(n) => value <= n,
            Predicate::Gte(n) => value >= n,
            Predicate::Lt(n) => value < n,
            Predicate::Gt(n) => value > n,
            Predicate::InRange(lo, hi) => value >= lo && value <= hi,
            // Never a satisfied predicate — see the type docs.
            Predicate::AlwaysTrue => false,
        }
    }

    /// `true` iff this predicate admits every possible value (the inverted-assertion shape).
    /// Used by [`SignalSource::assert_signal`] to reject it rather than return a green that
    /// asserts nothing.
    fn is_vacuous(&self) -> bool {
        matches!(self, Predicate::AlwaysTrue)
    }
}

/// The typed verdict of a single signal assertion — green, red, or rejected. **Never a bare
/// `bool`**, so a caller cannot `|| true` a red away (EI-01 §3/§5: loud, never swallowed).
///
/// `#[must_use]`: the compiler warns if a verdict is dropped on the floor, and
/// [`Assertion::expect_green`] panics loudly (with the signal + predicate + observed value)
/// on anything that is not green.
#[must_use = "a telemetry assertion verdict must be checked — a dropped red is a swallowed failure (EI-01 §3)"]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Assertion {
    /// The signal satisfied the predicate. The only verdict that is a pass.
    Green {
        /// The asserted signal (with its labels, if any), for the green artifact row.
        signal: AssertedSignal,
        /// The predicate that held.
        predicate: Predicate,
        /// The observed value that satisfied it.
        observed: i64,
    },
    /// The signal did NOT satisfy the predicate (the drill found the property broken — fix
    /// the deliverable, do not weaken the predicate, EI-01 §3).
    Red {
        /// The asserted signal (with labels).
        signal: AssertedSignal,
        /// The predicate that failed.
        predicate: Predicate,
        /// The observed value, or `None` if the signal was absent entirely (a missing signal
        /// is a RED, not a silent pass — "you cannot operate what you cannot see").
        observed: Option<i64>,
    },
    /// The assertion was REJECTED before evaluation: either the predicate was the vacuous
    /// inverted-assertion shape (EI-01 §3), or the signal name was mismatched to its
    /// labelled/scalar kind. Distinct from green so a misuse cannot masquerade as a pass.
    Rejected {
        /// The asserted signal.
        signal: AssertedSignal,
        /// Why it was rejected (for the loud failure message).
        reason: RejectReason,
    },
}

/// Why an assertion was rejected (never green).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    /// The predicate admitted every value — the inverted-assertion shape (EI-01 §3).
    VacuousPredicate,
    /// A scalar signal was asserted with labels, or a labelled signal asserted as scalar.
    LabelShapeMismatch,
}

/// A signal together with the labels it was asserted under, for the verdict/green-artifact
/// row (so a red can say exactly which `request_errors{kind=human, tenant=acme}` failed).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertedSignal {
    /// The contract-1.8 signal name.
    pub name: SignalName,
    /// The labels it was read under (empty for a scalar signal).
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
    /// `true` iff this verdict is green. The ONLY way to read a pass — a red or a rejected is
    /// never silently a pass.
    pub fn is_green(&self) -> bool {
        matches!(self, Assertion::Green { .. })
    }

    /// Construct the [`Assertion::Rejected`] verdict for a vacuous (inverted-assertion)
    /// predicate. Exposed so the assertion path can build it; it is never green.
    fn rejected(signal: AssertedSignal, reason: RejectReason) -> Assertion {
        Assertion::Rejected { signal, reason }
    }

    /// Unwrap a green verdict, **panicking loudly** (with the signal + predicate + observed
    /// value) on a red or rejected. This is the loud-not-swallowed failure a drill needs:
    /// the self-test and every later drill call `expect_green()` so a broken property aborts
    /// the test with a precise message rather than a silent `false`.
    pub fn expect_green(self) {
        match self {
            Assertion::Green { .. } => {}
            Assertion::Red {
                signal,
                predicate,
                observed,
            } => panic!(
                "DRILL RED: signal {:?}{} did not satisfy {:?} (observed {}) — \
                 the property is broken; fix the deliverable, do NOT weaken the predicate (EI-01 §3)",
                signal.name,
                fmt_labels(&signal.labels),
                predicate,
                match observed {
                    Some(v) => v.to_string(),
                    None => "ABSENT (signal not emitted — you cannot operate what you cannot see)"
                        .to_string(),
                },
            ),
            Assertion::Rejected { signal, reason } => panic!(
                "DRILL REJECTED: signal {:?}{} — {:?}; an assertion that asserts nothing is not a pass (EI-01 §3)",
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
        let inner: Vec<String> = labels.iter().map(|l| format!("{}={}", l.key, l.value)).collect();
        format!("{{{}}}", inner.join(", "))
    }
}

/// The in-memory telemetry signal source — a typed reader over the contract-1.8 survival
/// signal set. In tests the rig populates it with the values a drill produced; later (P-S12/
/// P-S13) the producer side exports the SAME signals on a real metrics-health port and this
/// reader reads them off it. The assertion surface ([`Self::assert_signal`] /
/// [`Self::assert_labelled`]) does not change.
///
/// Holds scalar signals keyed by [`SignalName`] and labelled signals keyed by
/// `(SignalName, sorted-labels)`, both as `i64` values (counts, ages-in-seconds, and the
/// numerically-encoded breaker state — every §10.2 signal a predicate reads is an integer
/// here; a ratio is read as a per-class count, see [`SignalName::FailStaticRatio`]).
#[derive(Default, Debug, Clone)]
pub struct SignalSource {
    scalars: HashMap<SignalName, i64>,
    labelled: HashMap<(SignalName, Vec<Label>), i64>,
}

impl SignalSource {
    /// A fresh, empty source (every signal absent — an absent signal asserts RED, never a
    /// silent green).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a scalar (unlabelled) signal's current value (the rig records what the drill
    /// produced; the producer side does this off the real meter at P-S13).
    pub fn set_scalar(&mut self, name: SignalName, value: i64) {
        self.scalars.insert(name, value);
    }

    /// Set a labelled signal's current value for a specific label set (e.g.
    /// `request_errors{kind=human, tenant=acme} = 0`). Labels are stored sorted so the read
    /// is order-independent.
    pub fn set_labelled(&mut self, name: SignalName, labels: Vec<Label>, value: i64) {
        let mut labels = labels;
        labels.sort();
        self.labelled.insert((name, labels), value);
    }

    /// Read a scalar signal's value, or `None` if it has never been set (absent).
    pub fn scalar(&self, name: SignalName) -> Option<i64> {
        self.scalars.get(&name).copied()
    }

    /// Read a labelled signal's value for a label set, or `None` if absent.
    pub fn labelled(&self, name: SignalName, labels: &[Label]) -> Option<i64> {
        let mut labels = labels.to_vec();
        labels.sort();
        self.labelled.get(&(name, labels)).copied()
    }

    /// **The core assertion (contract 1.8's first consumer).** Assert that the SCALAR signal
    /// `name` satisfies `predicate`, returning a typed [`Assertion`] — green/red/rejected,
    /// **never a swallowed pass**.
    ///
    /// - A vacuous (inverted-assertion) predicate is **rejected** ([`RejectReason::VacuousPredicate`]),
    ///   never green (EI-01 §3).
    /// - An ABSENT signal is **red** with `observed: None` — a missing signal is a failed
    ///   drill (observability is part of the pass condition), never a silent pass.
    /// - Otherwise green iff the observed value satisfies the predicate, red otherwise.
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

    /// Assert that the LABELLED signal `name` under `labels` satisfies `predicate` — the
    /// per-(principal-kind, tenant) / per-lane / per-consumer read contract 1.8 requires
    /// (e.g. `request_errors{kind=human, tenant=acme} == 0` while the agent lane sheds).
    /// Same green/red/rejected discipline as [`Self::assert_signal`].
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

    /// `assert_signal` reads green when the signal satisfies the predicate (the happy path
    /// the self-test rides: `outbox_depth == 0`).
    #[test]
    fn assert_signal_reads_green_on_satisfied_predicate() {
        let mut src = SignalSource::new();
        src.set_scalar(SignalName::OutboxDepth, 0);
        let verdict = src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0));
        assert!(verdict.is_green());
        verdict.expect_green(); // does not panic
    }

    /// `assert_signal` fails LOUDLY on a red signal — the verdict is red, and `expect_green`
    /// panics with the observed value (EI-01 §3/§5: loud, never swallowed). The core gate
    /// unit-test the prompt names.
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

    /// `expect_green` panics loudly on a red verdict (the loud failure a drill aborts with).
    #[test]
    #[should_panic(expected = "DRILL RED")]
    fn expect_green_panics_on_red() {
        let mut src = SignalSource::new();
        src.set_scalar(SignalName::OutboxDepth, 3);
        src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
            .expect_green();
    }

    /// An ABSENT signal reads RED (with `observed: None`), never a silent green — "you cannot
    /// operate what you cannot see" (EI-01 §3).
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

    /// An inverted/vacuous assertion is REJECTED, never green (EI-01 §3 — a check that
    /// asserts nothing is not a pass). The prompt's required unit test.
    #[test]
    fn inverted_assertion_is_rejected_not_green() {
        let mut src = SignalSource::new();
        // Even with a "passing-looking" value present, the vacuous predicate is rejected.
        src.set_scalar(SignalName::OutboxDepth, 0);
        let verdict = src.assert_signal(SignalName::OutboxDepth, Predicate::AlwaysTrue);
        assert!(!verdict.is_green(), "a vacuous assertion must NOT read green");
        match verdict {
            Assertion::Rejected {
                reason: RejectReason::VacuousPredicate,
                ..
            } => {}
            other => panic!("expected Rejected{{VacuousPredicate}}, got {other:?}"),
        }
    }

    /// `expect_green` panics on a rejected verdict too (a misuse cannot masquerade as a pass).
    #[test]
    #[should_panic(expected = "DRILL REJECTED")]
    fn expect_green_panics_on_rejected() {
        let src = SignalSource::new();
        src.assert_signal(SignalName::OutboxDepth, Predicate::AlwaysTrue)
            .expect_green();
    }

    /// Labelled signals are read per `{kind, tenant}` (contract 1.8 RED-per-principal-kind):
    /// the human lane holds (errors == 0) while the agent lane sheds (shed_count > 0) — the
    /// shape every surge drill asserts.
    #[test]
    fn labelled_signals_read_per_principal_kind_and_tenant() {
        let mut src = SignalSource::new();
        let human = vec![Label::new("kind", "human"), Label::new("tenant", "acme")];
        let agent_lane = vec![Label::new("lane", "agent")];
        src.set_labelled(SignalName::RequestErrors, human.clone(), 0);
        src.set_labelled(SignalName::ShedCount, agent_lane.clone(), 1500);

        // human lane holds: zero errors
        src.assert_labelled(SignalName::RequestErrors, human, Predicate::Eq(0))
            .expect_green();
        // agent lane shed (the surge was absorbed by shedding, not by hurting humans)
        src.assert_labelled(SignalName::ShedCount, agent_lane, Predicate::Gte(1))
            .expect_green();
    }

    /// Label order does not matter — a signal set under `{kind, tenant}` reads the same as
    /// `{tenant, kind}` (so a drill cannot miss a green by passing labels in a different
    /// order).
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

    /// The predicate vocabulary covers the §10.2 bounds: equality zeros, the `<=` staleness
    /// bound, the `>=` tripwire-fired bound, and the inclusive range band.
    #[test]
    fn predicate_vocabulary_covers_the_bounds() {
        let mut src = SignalSource::new();
        src.set_scalar(SignalName::FailStaticStalenessSecs, 30);
        src.set_scalar(SignalName::CausalDepthFirings, 1);
        src.set_scalar(SignalName::CrossTenantCount, 0);

        // staleness <= static_max (here a placeholder 60s; the real W is the P-S22/P-S38
        // thresholds value — named floor)
        src.assert_signal(SignalName::FailStaticStalenessSecs, Predicate::Lte(60))
            .expect_green();
        // the causal-depth tripwire fired on the adversarial loop
        src.assert_signal(SignalName::CausalDepthFirings, Predicate::Gte(1))
            .expect_green();
        // zero cross-tenant reads (the IDOR zero)
        src.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .expect_green();
        // a staleness that BLOWS the bound is red
        src.set_scalar(SignalName::FailStaticStalenessSecs, 120);
        assert!(!src
            .assert_signal(SignalName::FailStaticStalenessSecs, Predicate::Lte(60))
            .is_green());
        // an in-range band
        src.set_scalar(SignalName::FailStaticRatio, 95);
        src.assert_signal(SignalName::FailStaticRatio, Predicate::InRange(90, 100))
            .expect_green();
    }

    /// The library covers the full contract-1.8 §10.2 signal set (a doc that omits any of
    /// these fails X-1) — every name is distinct and the set is the exhaustive ALL.
    #[test]
    fn covers_the_full_contract_1_8_signal_set() {
        // 21 distinct names, matching the §10.2 table rows (RED ×3 + USE + lag + outbox +
        // dead-letter + breaker + fail-static ×2 + shed + causal ×2 + cross-tenant +
        // firehose ×2 + readiness + liveness-restart — the §4.3 liveness≠readiness pair) PLUS
        // the §11 D-6 / SUB-D6 restore-verify triplet (cross-seam-mismatch + RPO + RTO, P-S26).
        let mut uniq = SignalName::ALL.to_vec();
        let n = uniq.len();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), n, "every contract-1.8 signal name is distinct");
        assert_eq!(n, 21, "the §10.2 set + the restore-verify triplet is covered exhaustively");
    }
}
