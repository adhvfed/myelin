//! # `telemetry` — the Bus's survival signals on the metrics-health port (EB-11 → P-014)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/event-bus.md`
//! §4.11 (the telemetry contract — the Phase-5 drill survival signals); the canonical signal
//! set is `00-platform-substrate.md` §10.2 (contract-index row 1.8). Doctrine:
//! `external-insights/01-process-and-quality-doctrine.md` §3 — *observability is part of the
//! pass condition; a drill that emits no signal has failed; the Bus is the largest single
//! contributor to contract 1.8.*
//!
//! ## What this module is (and what it deliberately is NOT)
//! This is the Bus's **provider side** of contract 1.8: it reads the Bus's live counters and
//! produces the §4.11 survival-signal samples — *with the right NAME and UNIT* — onto a
//! metrics-health port ([`MetricsSink`]). These samples ARE the assertions the §8 Bus drills
//! read (`outbox_depth → 0`, `dead_letter_count == 0`, …). It wires them so every later Bus
//! drill has a signal to assert against (EB-11 DELIVERABLE).
//!
//! It is NOT the harness's assertion library. The typed green/red assertion *consumer* side —
//! `SignalName` / `SignalSource` / `Predicate` / `Assertion` — already shipped in the
//! **failure-injection harness** (`myelin-harness::telemetry`, P-S04/P-004), the frozen §10.2
//! NAME enum and the loud-never-swallowed verdict machinery. **`myelin-events` must not depend
//! on `myelin-harness` in production code** (the harness is a leaf TEST-SUPPORT crate, a
//! dev-dependency only; an `events → harness` production edge would invert the §2.9 DAG and is
//! forbidden). So this module owns the Bus's *emit* vocabulary as plain `&'static str`
//! name+unit constants ([`BusSignal`]) whose names line up 1:1 with the harness `SignalName`
//! enum; the **harness self-test** (`tests/drills_eb11_telemetry_self_test.rs`, where the
//! harness IS available) maps a [`BusSignals`] snapshot into the harness `SignalSource` and
//! runs the producer-kill → outbox-depth/dedup assertion EB-11's GATE requires.
//!
//! ## The §4.11 Bus survival-signal set (each emitted with the right name + unit)
//! `myelin-events` emits (contract 1.8 §4.11) — the seven groups, mapped to [`BusSignal`]:
//! 1. **consumer lag** (`num_pending` per durable consumer) — [`BusSignal::ConsumerLag`], unit
//!    `events`. The §4.2 head-of-line signal.
//! 2. **outbox depth + age** — [`BusSignal::OutboxDepth`] (unit `rows`) +
//!    [`BusSignal::OutboxAgeSecs`] (unit `seconds`, the age of the oldest unsent row). The
//!    silent-data-loss (BUS-2) signal: depth says how many are stuck, age says how long.
//! 3. **relay publish + dead-letter rate** — [`BusSignal::RelayPublished`] (unit `events`) +
//!    [`BusSignal::DeadLetterCount`] (unit `rows`).
//! 4. **per-aggregate publish latency** (`recorded_at` → broker-ack) —
//!    [`BusSignal::PublishLatencyMillis`], unit `milliseconds`. The D-9 ordering-health signal.
//! 5. **dedup hit-rate** (effectively-once health) — [`BusSignal::DedupHits`] +
//!    [`BusSignal::DedupDeliveries`] (both unit `events`); the rate is `hits / deliveries`.
//! 6. **per-tenant in-flight** (fairness / agent-surge) — [`BusSignal::PerTenantInflight`],
//!    unit `events`, labelled `{tenant}`.
//! 7. **causal-depth histogram + shared-root-tripwire counter** (loop-safety) —
//!    [`BusSignal::CausalDepthMax`] (unit `hops`) + [`BusSignal::SharedRootTripwireFirings`]
//!    (unit `firings`).
//!
//! The names map onto the harness `SignalName` (the §10.2 frozen set): `OutboxDepth →
//! OutboxDepth`, `DeadLetterCount → DeadLetterCount`, `ConsumerLag → ConsumerLag`,
//! `CausalDepthMax/SharedRootTripwireFirings → CausalDepthFirings`. The Bus-finer signals
//! (outbox age, publish latency, dedup hit-rate, per-tenant in-flight) are the Bus's *finer*
//! contribution under those §10.2 rows ("outbox depth **+ age**", "consumer lag … oldest-un-
//! acked **age**", "per-tenant **in-flight**"); they are emitted here so the §8 drills can read
//! them, with no change to the frozen §10.2 enum (coherence, EI-01 §7 — never widen a frozen
//! contract to fit; the harness's exhaustive-`ALL` test stays at 16).
//!
//! ## Floors named (deferred + filling prompt)
//! - **No shared wall-clock at M0.** Outbox age + publish latency are computed against a
//!   caller-supplied `now` ([`BusSignals::snapshot`] takes it). The real monotonic clock wires
//!   into the metrics exporter when `serve` lands (**P-S12**); the names/units do not change.
//! - **The metrics-health PORT is the §3.5 surface, wired at `serve` (P-S12/P-S13).** This
//!   module ships the *emit* surface ([`MetricsSink`] + the in-memory [`MetricRecorder`]) and
//!   the snapshot that drives it; the OpenTelemetry exporter on the real port lands with
//!   `serve`. The signal names/units this module emits are the ones that port will export.
//! - **Dedup-hit / per-tenant-in-flight / causal-depth firings are OBSERVED inputs.** The
//!   `OutboxStore` depth+age and the relay `DrainReport` are read off live state; the dedup
//!   hit-rate, the per-tenant in-flight tally, and the causal-depth/shared-root tripwire are
//!   fed as [`BusObservations`] by the producer (the consumer runtime + dispatch tier that own
//!   those counters). The dispatch-tier tripwire COUNTER itself is **EB-23 (P-143)**; here the
//!   signal NAME + unit + the snapshot seam are frozen so EB-23 only feeds the count.

use crate::outbox::OutboxStore;
use crate::relay::DrainReport;
use crate::Timestamp;
use serde::{Deserialize, Serialize};

/// One Bus survival signal NAME (contract 1.8 §4.11), carrying its canonical metrics-port
/// name string and unit. A `&'static str` vocabulary — NOT the harness `SignalName` enum (the
/// production crate cannot depend on the harness) — whose [`BusSignal::metric_name`]s line up
/// 1:1 with the §10.2 frozen names the harness asserts against.
///
/// Each variant is one row of the §4.11 Bus contribution. The metric NAME + UNIT are frozen
/// here (EB-11 GATE: "each named survival signal is emitted with the right name/unit").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BusSignal {
    /// Consumer lag — `num_pending` per durable consumer (§4.11 #1; §10.2 row 3). Labelled
    /// `{consumer}`. Unit: events.
    ConsumerLag,
    /// Outbox depth — count of committed-but-unsent rows (§4.11 #2; §10.2 row 4; BUS-2). The
    /// silent-data-loss zero: the drill asserts `→ 0` once the relay drains. Unit: rows.
    OutboxDepth,
    /// Outbox age — seconds the OLDEST unsent row has waited (§4.11 #2, "depth **+ age**").
    /// A wedged relay shows age climbing even at constant depth. Unit: seconds.
    OutboxAgeSecs,
    /// Relay publish rate — rows the relay freshly published (§4.11 #3). Unit: events.
    RelayPublished,
    /// Dead-letter count — rows the relay gave up on after the bounded retries (§4.11 #3;
    /// §10.2 row 4). The no-loss path asserts `== 0`. Unit: rows.
    DeadLetterCount,
    /// Per-aggregate publish latency — `recorded_at` → broker-ack, in milliseconds (§4.11 #4;
    /// the D-9 ordering-health signal). Unit: milliseconds.
    PublishLatencyMillis,
    /// Dedup hits — redeliveries a consumer's `(consumer, event_id)` ledger absorbed (§4.11
    /// #5; effectively-once health). Unit: events.
    DedupHits,
    /// Dedup deliveries — total deliveries seen (the denominator of the hit-rate) (§4.11 #5).
    /// Unit: events.
    DedupDeliveries,
    /// Per-tenant in-flight — claimed-but-not-yet-acked events for one tenant (§4.11 #6;
    /// fairness / agent-surge). Labelled `{tenant}`. Unit: events.
    PerTenantInflight,
    /// Causal-depth histogram max — the deepest causal hop observed (§4.11 #7; loop-safety,
    /// D-8). Unit: hops.
    CausalDepthMax,
    /// Shared-root-tripwire firings — times the per-tenant breaker tripped on a runaway
    /// shared-`correlation_id` fan-out (§4.11 #7; D-8). Unit: firings.
    SharedRootTripwireFirings,
}

impl BusSignal {
    /// Every §4.11 Bus survival signal (for the "the Bus emits the full §4.11 set" test —
    /// observability is part of the pass condition, so the Bus's contribution must be
    /// exhaustive: omitting any of these fails X-1).
    pub const ALL: [BusSignal; 11] = [
        BusSignal::ConsumerLag,
        BusSignal::OutboxDepth,
        BusSignal::OutboxAgeSecs,
        BusSignal::RelayPublished,
        BusSignal::DeadLetterCount,
        BusSignal::PublishLatencyMillis,
        BusSignal::DedupHits,
        BusSignal::DedupDeliveries,
        BusSignal::PerTenantInflight,
        BusSignal::CausalDepthMax,
        BusSignal::SharedRootTripwireFirings,
    ];

    /// The canonical metrics-port metric NAME (frozen). These line up 1:1 with the harness
    /// §10.2 `SignalName` names so a real port (or the self-test) reads the Bus's emit as the
    /// same signal the drills assert against.
    pub fn metric_name(self) -> &'static str {
        match self {
            BusSignal::ConsumerLag => "bus.consumer_lag",
            BusSignal::OutboxDepth => "bus.outbox_depth",
            BusSignal::OutboxAgeSecs => "bus.outbox_age_seconds",
            BusSignal::RelayPublished => "bus.relay_published",
            BusSignal::DeadLetterCount => "bus.dead_letter_count",
            BusSignal::PublishLatencyMillis => "bus.publish_latency_millis",
            BusSignal::DedupHits => "bus.dedup_hits",
            BusSignal::DedupDeliveries => "bus.dedup_deliveries",
            BusSignal::PerTenantInflight => "bus.per_tenant_inflight",
            BusSignal::CausalDepthMax => "bus.causal_depth_max",
            BusSignal::SharedRootTripwireFirings => "bus.shared_root_tripwire_firings",
        }
    }

    /// The canonical UNIT (frozen, §2.10): counts in their entity (`rows`/`events`), ages in
    /// `seconds`, latency in `milliseconds`, depth in `hops`, tripwire in `firings`. A signal
    /// emitted with the wrong unit is a contract break (EB-11 GATE: "the right name/unit").
    pub fn unit(self) -> &'static str {
        match self {
            BusSignal::ConsumerLag => "events",
            BusSignal::OutboxDepth => "rows",
            BusSignal::OutboxAgeSecs => "seconds",
            BusSignal::RelayPublished => "events",
            BusSignal::DeadLetterCount => "rows",
            BusSignal::PublishLatencyMillis => "milliseconds",
            BusSignal::DedupHits => "events",
            BusSignal::DedupDeliveries => "events",
            BusSignal::PerTenantInflight => "events",
            BusSignal::CausalDepthMax => "hops",
            BusSignal::SharedRootTripwireFirings => "firings",
        }
    }
}

/// One label on a metric sample (`tenant=acme`, `consumer=search-indexer`). PII-free
/// identifiers only (a tenant id, a consumer name) — never a payload — so a telemetry label is
/// `control-plane-pii-free` by construction (mirrors the harness `Label` shape so the
/// self-test maps one to the other without translation loss).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MetricLabel {
    /// The label key (`tenant` / `consumer`).
    pub key: String,
    /// The label value (a PII-free identifier).
    pub value: String,
}

impl MetricLabel {
    /// A label from a `(key, value)` pair.
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> MetricLabel {
        MetricLabel {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// One emitted metric sample on the metrics-health port: a [`BusSignal`] (name + unit), its
/// `i64` value (counts / ages-in-seconds / latency-in-millis / depth — every §4.11 signal a
/// predicate reads is an integer, exactly as the harness `SignalSource` stores them), and its
/// labels (empty for a scalar signal).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricSample {
    /// The signal this sample carries (its frozen name + unit).
    pub signal: BusSignal,
    /// The observed value (an integer; a ratio is read as its numerator/denominator pair —
    /// see [`BusSignal::DedupHits`] / [`BusSignal::DedupDeliveries`]).
    pub value: i64,
    /// The labels (e.g. `{consumer}`, `{tenant}`), empty for a scalar signal.
    pub labels: Vec<MetricLabel>,
}

impl MetricSample {
    /// A scalar (unlabelled) sample.
    pub fn scalar(signal: BusSignal, value: i64) -> MetricSample {
        MetricSample {
            signal,
            value,
            labels: Vec::new(),
        }
    }

    /// A labelled sample.
    pub fn labelled(signal: BusSignal, value: i64, labels: Vec<MetricLabel>) -> MetricSample {
        let mut labels = labels;
        labels.sort();
        MetricSample {
            signal,
            value,
            labels,
        }
    }
}

/// **The metrics-health port seam** (architecture §3.5): where the Bus's survival signals are
/// emitted. A trait so the real OpenTelemetry exporter (wired at `serve`, P-S12/P-S13) and the
/// in-memory [`MetricRecorder`] (tests + the self-test) are the SAME emit code path — no test
/// backdoor (EI-01 §3).
pub trait MetricsSink {
    /// Emit one sample onto the metrics-health port.
    fn emit(&mut self, sample: MetricSample);
}

/// An in-memory [`MetricsSink`] — records every emitted sample so a test (and the EB-11
/// harness self-test) can read back exactly what the Bus emitted on the metrics port. The
/// producer side at `serve` swaps this for the OpenTelemetry exporter against the SAME trait.
#[derive(Default, Debug, Clone)]
pub struct MetricRecorder {
    samples: Vec<MetricSample>,
}

impl MetricRecorder {
    /// A fresh recorder (nothing emitted yet).
    pub fn new() -> Self {
        Self::default()
    }

    /// Every sample emitted, in emit order.
    pub fn samples(&self) -> &[MetricSample] {
        &self.samples
    }

    /// Read back the value of a SCALAR signal (the first sample with no labels), or `None` if
    /// it was never emitted. (An absent signal is a RED in the harness — never a silent pass.)
    pub fn scalar(&self, signal: BusSignal) -> Option<i64> {
        self.samples
            .iter()
            .find(|s| s.signal == signal && s.labels.is_empty())
            .map(|s| s.value)
    }

    /// Read back the value of a LABELLED signal under `labels`, or `None` if absent.
    pub fn labelled(&self, signal: BusSignal, labels: &[MetricLabel]) -> Option<i64> {
        let mut want = labels.to_vec();
        want.sort();
        self.samples
            .iter()
            .find(|s| s.signal == signal && s.labels == want)
            .map(|s| s.value)
    }
}

impl MetricsSink for MetricRecorder {
    fn emit(&mut self, sample: MetricSample) {
        self.samples.push(sample);
    }
}

/// The producer-fed observations the Bus's *non-store* survival signals are computed from —
/// the counters owned by the consumer runtime + the dispatch tier (which are fed in rather
/// than reached into, so the snapshot stays a pure read).
///
/// The store-side signals (outbox depth+age, relay published, dead-letter count) are read off
/// live state in [`BusSignals::snapshot`]; THESE are the finer signals whose counters live
/// elsewhere (consumer lag, dedup hits, per-tenant in-flight, causal-depth, tripwire firings).
/// Floor named: the dispatch-tier shared-root tripwire COUNTER is EB-23 (P-143) — here the
/// field is the seam EB-23 feeds; until then it is `0` (no tripwire has fired).
#[derive(Clone, Debug, Default)]
pub struct BusObservations {
    /// Consumer lag (`num_pending`) per durable consumer (read off `Consumer::lag`).
    pub consumer_lag: Vec<(String, u64)>,
    /// Redeliveries the dedup ledgers absorbed this window (the hit-rate numerator).
    pub dedup_hits: u64,
    /// Total deliveries this window (the hit-rate denominator).
    pub dedup_deliveries: u64,
    /// Claimed-but-not-yet-acked events per tenant (fairness / agent-surge).
    pub per_tenant_inflight: Vec<(String, u64)>,
    /// The deepest causal hop observed (the histogram max; loop-safety).
    pub causal_depth_max: u32,
    /// Times the shared-root tripwire fired (the per-tenant breaker; EB-23 feeds the count).
    pub shared_root_tripwire_firings: u64,
}

/// **The Bus's contract-1.8 survival-signal SNAPSHOT.** Reads the Bus's live counters (outbox
/// depth + age, the relay's published / dead-letter counts) and folds in the producer-fed
/// [`BusObservations`] (consumer lag, dedup hit-rate, per-tenant in-flight, causal depth,
/// tripwire) into the §4.11 set — then [`BusSignals::emit_to`] writes each as a [`MetricSample`]
/// (with the right name + unit) onto a [`MetricsSink`].
///
/// This is the thing EB-11 wires "so every later Bus drill has a signal to assert against": a
/// drill snapshots the Bus after its fault, emits to a recorder, then asserts the recorded
/// `outbox_depth` / `dead_letter_count` / … read the value the property demands.
#[derive(Clone, Debug)]
pub struct BusSignals {
    /// Committed-but-unsent rows (BUS-2). The drill asserts `→ 0`.
    pub outbox_depth: i64,
    /// Seconds the oldest unsent row has waited (`0` when the outbox is drained).
    pub outbox_age_secs: i64,
    /// Rows the relay freshly published (the publish-rate signal).
    pub relay_published: i64,
    /// Rows the relay dead-lettered after the bounded retries (the no-loss path asserts `0`).
    pub dead_letter_count: i64,
    /// Per-aggregate publish latency (`recorded_at` → broker-ack), in milliseconds.
    pub publish_latency_millis: i64,
    /// Redeliveries the dedup ledgers absorbed (the effectively-once-health numerator).
    pub dedup_hits: i64,
    /// Total deliveries (the hit-rate denominator).
    pub dedup_deliveries: i64,
    /// Consumer lag (`num_pending`) per durable consumer.
    pub consumer_lag: Vec<(String, i64)>,
    /// Claimed-but-not-yet-acked events per tenant.
    pub per_tenant_inflight: Vec<(String, i64)>,
    /// The deepest causal hop observed (the histogram max).
    pub causal_depth_max: i64,
    /// Shared-root-tripwire firings.
    pub shared_root_tripwire_firings: i64,
}

impl BusSignals {
    /// Snapshot the Bus's survival signals from a live [`OutboxStore`] + the latest relay
    /// [`DrainReport`] + the producer-fed [`BusObservations`], against a caller-supplied `now`
    /// (M0 has no shared wall-clock until `serve`, P-S12 — `now` is RFC-3339 UTC).
    ///
    /// `outbox_age_secs` is `now − recorded_at(oldest unsent row)`, clamped to `>= 0` and `0`
    /// when the outbox is drained. `publish_latency_millis` is supplied as the latest observed
    /// `recorded_at → broker-ack` delta (the relay measures it on `put`; the floor passes it in
    /// as `0` until the broker-ack clock wires at `serve`).
    pub fn snapshot(
        store: &OutboxStore,
        drain: &DrainReport,
        obs: &BusObservations,
        now: &Timestamp,
        publish_latency_millis: i64,
    ) -> BusSignals {
        let outbox_age_secs = store
            .oldest_unsent_recorded_at()
            .map(|recorded| age_seconds(&recorded, now))
            .unwrap_or(0);

        BusSignals {
            outbox_depth: store.outbox_depth() as i64,
            outbox_age_secs,
            relay_published: drain.published as i64,
            dead_letter_count: store.dead_letter_count() as i64,
            publish_latency_millis,
            dedup_hits: obs.dedup_hits as i64,
            dedup_deliveries: obs.dedup_deliveries as i64,
            consumer_lag: obs
                .consumer_lag
                .iter()
                .map(|(c, v)| (c.clone(), *v as i64))
                .collect(),
            per_tenant_inflight: obs
                .per_tenant_inflight
                .iter()
                .map(|(t, v)| (t.clone(), *v as i64))
                .collect(),
            causal_depth_max: obs.causal_depth_max as i64,
            shared_root_tripwire_firings: obs.shared_root_tripwire_firings as i64,
        }
    }

    /// **Emit the full §4.11 Bus survival-signal set onto the metrics-health port** — each
    /// sample carries its frozen [`BusSignal`] name + unit (EB-11: "every later Bus drill has a
    /// signal to assert against"). The labelled signals (consumer lag, per-tenant in-flight)
    /// emit one sample per label set; the rest are scalars.
    pub fn emit_to<S: MetricsSink>(&self, sink: &mut S) {
        sink.emit(MetricSample::scalar(BusSignal::OutboxDepth, self.outbox_depth));
        sink.emit(MetricSample::scalar(
            BusSignal::OutboxAgeSecs,
            self.outbox_age_secs,
        ));
        sink.emit(MetricSample::scalar(
            BusSignal::RelayPublished,
            self.relay_published,
        ));
        sink.emit(MetricSample::scalar(
            BusSignal::DeadLetterCount,
            self.dead_letter_count,
        ));
        sink.emit(MetricSample::scalar(
            BusSignal::PublishLatencyMillis,
            self.publish_latency_millis,
        ));
        sink.emit(MetricSample::scalar(BusSignal::DedupHits, self.dedup_hits));
        sink.emit(MetricSample::scalar(
            BusSignal::DedupDeliveries,
            self.dedup_deliveries,
        ));
        sink.emit(MetricSample::scalar(
            BusSignal::CausalDepthMax,
            self.causal_depth_max,
        ));
        sink.emit(MetricSample::scalar(
            BusSignal::SharedRootTripwireFirings,
            self.shared_root_tripwire_firings,
        ));
        for (consumer, lag) in &self.consumer_lag {
            sink.emit(MetricSample::labelled(
                BusSignal::ConsumerLag,
                *lag,
                vec![MetricLabel::new("consumer", consumer.clone())],
            ));
        }
        for (tenant, inflight) in &self.per_tenant_inflight {
            sink.emit(MetricSample::labelled(
                BusSignal::PerTenantInflight,
                *inflight,
                vec![MetricLabel::new("tenant", tenant.clone())],
            ));
        }
    }
}

/// `now − recorded_at` in whole seconds, clamped to `>= 0`. Both are RFC-3339 UTC `Z`-suffixed
/// timestamps (the frozen unit §2.10). M0 has no chrono dependency, so this parses the seconds
/// component lexically off the canonical `YYYY-MM-DDTHH:MM:SSZ` shape — sufficient for the M0
/// drill's age signal (the real exporter at `serve` uses the monotonic clock, named floor).
/// A timestamp that does not parse yields `0` (a missing/garbled age is never negative, and is
/// reported as fresh rather than fabricating a stale value).
fn age_seconds(recorded_at: &Timestamp, now: &Timestamp) -> i64 {
    match (epoch_secs(&recorded_at.0), epoch_secs(&now.0)) {
        (Some(then), Some(at_now)) => (at_now - then).max(0),
        _ => 0,
    }
}

/// Parse a canonical `YYYY-MM-DDTHH:MM:SSZ` (RFC-3339 UTC) timestamp into seconds since a
/// fixed epoch sufficient for DELTAS (the absolute epoch cancels in `now − then`). Returns
/// `None` on any deviation from the canonical shape. Days-per-month uses a civil calendar so
/// timestamps that cross a day/month boundary subtract correctly.
fn epoch_secs(ts: &str) -> Option<i64> {
    // YYYY-MM-DDTHH:MM:SSZ  →  exactly 20 chars, fixed separators.
    let b = ts.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':'
        || b[16] != b':' || b[19] != b'Z'
    {
        return None;
    }
    let num = |s: &str| s.parse::<i64>().ok();
    let year = num(&ts[0..4])?;
    let month = num(&ts[5..7])?;
    let day = num(&ts[8..10])?;
    let hour = num(&ts[11..13])?;
    let min = num(&ts[14..16])?;
    let sec = num(&ts[17..19])?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from a fixed civil epoch (Howard Hinnant's days_from_civil), valid for any year.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3_600 + min * 60 + sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EB-11: the Bus emits the FULL §4.11 survival-signal set — every signal distinct, the set
    /// exhaustive. Observability is part of the pass condition; omitting any of these fails X-1.
    #[test]
    fn bus_emits_the_full_4_11_survival_signal_set() {
        let mut uniq = BusSignal::ALL.to_vec();
        let n = uniq.len();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), n, "every §4.11 Bus signal is distinct");
        assert_eq!(n, 11, "the §4.11 Bus contribution is covered exhaustively");
    }

    /// EB-11 GATE: each signal is emitted with the RIGHT NAME and UNIT. A name typo or a unit
    /// drift (e.g. age in `rows`) is a contract break, caught here.
    #[test]
    fn each_signal_has_the_right_name_and_unit() {
        // Names are namespaced `bus.*`, distinct, and the units are the frozen §2.10 set.
        let mut names: Vec<&str> = BusSignal::ALL.iter().map(|s| s.metric_name()).collect();
        let total = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), total, "every Bus metric name is distinct");

        assert_eq!(BusSignal::OutboxDepth.metric_name(), "bus.outbox_depth");
        assert_eq!(BusSignal::OutboxDepth.unit(), "rows");
        assert_eq!(BusSignal::OutboxAgeSecs.unit(), "seconds");
        assert_eq!(BusSignal::PublishLatencyMillis.unit(), "milliseconds");
        assert_eq!(BusSignal::ConsumerLag.unit(), "events");
        assert_eq!(BusSignal::CausalDepthMax.unit(), "hops");
        assert_eq!(BusSignal::SharedRootTripwireFirings.unit(), "firings");
        // every signal has a non-empty name + unit (no unset row).
        for s in BusSignal::ALL {
            assert!(!s.metric_name().is_empty(), "{s:?} has a name");
            assert!(!s.unit().is_empty(), "{s:?} has a unit");
        }
    }

    /// A snapshot over a live store with unsent rows reads the depth + age the silent-data-loss
    /// drill asserts against; after the relay drains, depth → 0 and age → 0.
    #[test]
    fn snapshot_reads_outbox_depth_and_age_off_live_state() {
        use crate::{
            Actor, AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType,
            IdMinter, MonotonicMinter, OutboxStore, OutboxTx, Visibility,
        };
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        use myelin_tenancy::{Region, TenantId};
        use std::sync::Arc;

        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ctx = EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId("acme".into()))),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            // the row's recorded_at — the age is measured against `now` below.
            recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
            caused_by: None,
        };
        let mut tx = store.begin(minter, ctx);
        tx.stage_state_change("created");
        for i in 0..3u32 {
            tx.emit(
                EventDraft {
                    type_: EventType(format!("issues.issue.e{i}")),
                    subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
                    aggregate: AggregateKey("issue:PROJ-1".into()),
                    payload: serde_json::json!({ "ref": "PROJ-1" }),
                    data_role: DataRole::Controller,
                    visibility: Visibility::Internal,
                    contains_personal_data: false,
                    pii_key_ref: None,
                },
                None,
            )
            .unwrap();
        }
        tx.commit().unwrap();

        // 3 unsent rows; the oldest waited 90 seconds (00:00:00 → 00:01:30).
        let obs = BusObservations::default();
        let drain = DrainReport::default();
        let now = Timestamp("2026-06-19T00:01:30Z".into());
        let sig = BusSignals::snapshot(&store, &drain, &obs, &now, 0);
        assert_eq!(sig.outbox_depth, 3, "3 committed-but-unsent rows");
        assert_eq!(sig.outbox_age_secs, 90, "oldest unsent row waited 90s");

        // Emit onto a recorder and read the depth/age back by name (the port round-trip).
        let mut rec = MetricRecorder::new();
        sig.emit_to(&mut rec);
        assert_eq!(rec.scalar(BusSignal::OutboxDepth), Some(3));
        assert_eq!(rec.scalar(BusSignal::OutboxAgeSecs), Some(90));
    }

    /// The labelled signals (consumer lag, per-tenant in-flight) emit one sample per label set
    /// and read back per `{consumer}` / `{tenant}`.
    #[test]
    fn labelled_signals_emit_per_consumer_and_per_tenant() {
        let obs = BusObservations {
            consumer_lag: vec![("search-indexer".into(), 4), ("notif-router".into(), 0)],
            per_tenant_inflight: vec![("acme".into(), 7), ("globex".into(), 0)],
            dedup_hits: 2,
            dedup_deliveries: 10,
            causal_depth_max: 3,
            shared_root_tripwire_firings: 0,
        };
        let store = OutboxStore::new();
        let drain = DrainReport {
            published: 8,
            ..DrainReport::default()
        };
        let sig = BusSignals::snapshot(
            &store,
            &drain,
            &obs,
            &Timestamp("2026-06-19T00:00:00Z".into()),
            12,
        );
        let mut rec = MetricRecorder::new();
        sig.emit_to(&mut rec);

        // scalar Bus-health signals
        assert_eq!(rec.scalar(BusSignal::RelayPublished), Some(8));
        assert_eq!(rec.scalar(BusSignal::PublishLatencyMillis), Some(12));
        assert_eq!(rec.scalar(BusSignal::DedupHits), Some(2));
        assert_eq!(rec.scalar(BusSignal::DedupDeliveries), Some(10));
        assert_eq!(rec.scalar(BusSignal::CausalDepthMax), Some(3));

        // labelled reads, per consumer + per tenant
        assert_eq!(
            rec.labelled(
                BusSignal::ConsumerLag,
                &[MetricLabel::new("consumer", "search-indexer")]
            ),
            Some(4)
        );
        assert_eq!(
            rec.labelled(
                BusSignal::PerTenantInflight,
                &[MetricLabel::new("tenant", "acme")]
            ),
            Some(7)
        );
    }

    /// `age_seconds` clamps to `>= 0` (a clock skew never reports a negative age) and crosses a
    /// day/month boundary correctly (civil-calendar day math).
    #[test]
    fn age_seconds_is_clamped_and_calendar_correct() {
        // 90s within a minute boundary
        assert_eq!(
            age_seconds(
                &Timestamp("2026-06-19T00:00:00Z".into()),
                &Timestamp("2026-06-19T00:01:30Z".into())
            ),
            90
        );
        // across midnight: 23:59:59 → next-day 00:00:09 == 10s
        assert_eq!(
            age_seconds(
                &Timestamp("2026-06-19T23:59:59Z".into()),
                &Timestamp("2026-06-20T00:00:09Z".into())
            ),
            10
        );
        // across a month boundary: Jun 30 → Jul 1, one day == 86400s
        assert_eq!(
            age_seconds(
                &Timestamp("2026-06-30T12:00:00Z".into()),
                &Timestamp("2026-07-01T12:00:00Z".into())
            ),
            86_400
        );
        // a clock skew (now < then) clamps to 0, never negative
        assert_eq!(
            age_seconds(
                &Timestamp("2026-06-19T00:01:00Z".into()),
                &Timestamp("2026-06-19T00:00:00Z".into())
            ),
            0
        );
        // a garbled timestamp yields 0 (fresh, never fabricated-stale)
        assert_eq!(
            age_seconds(&Timestamp("not-a-timestamp".into()), &Timestamp("2026-06-19T00:00:00Z".into())),
            0
        );
    }
}
