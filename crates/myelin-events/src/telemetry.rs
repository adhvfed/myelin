use crate::outbox::OutboxStore;
use crate::relay::DrainReport;
use crate::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BusSignal {
    ConsumerLag,
    OutboxDepth,
    OutboxAgeSecs,
    RelayPublished,
    DeadLetterCount,
    PublishLatencyMillis,
    DedupHits,
    DedupDeliveries,
    PerTenantInflight,
    CausalDepthMax,
    SharedRootTripwireFirings,
}

impl BusSignal {
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MetricLabel {
    pub key: String,
    pub value: String,
}

impl MetricLabel {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> MetricLabel {
        MetricLabel {
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricSample {
    pub signal: BusSignal,
    pub value: i64,
    pub labels: Vec<MetricLabel>,
}

impl MetricSample {
    pub fn scalar(signal: BusSignal, value: i64) -> MetricSample {
        MetricSample {
            signal,
            value,
            labels: Vec::new(),
        }
    }

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

pub trait MetricsSink {
    fn emit(&mut self, sample: MetricSample);
}

#[derive(Default, Debug, Clone)]
pub struct MetricRecorder {
    samples: Vec<MetricSample>,
}

impl MetricRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn samples(&self) -> &[MetricSample] {
        &self.samples
    }

    pub fn scalar(&self, signal: BusSignal) -> Option<i64> {
        self.samples
            .iter()
            .find(|s| s.signal == signal && s.labels.is_empty())
            .map(|s| s.value)
    }

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

#[derive(Clone, Debug, Default)]
pub struct BusObservations {
    pub consumer_lag: Vec<(String, u64)>,
    pub dedup_hits: u64,
    pub dedup_deliveries: u64,
    pub per_tenant_inflight: Vec<(String, u64)>,
    pub causal_depth_max: u32,
    pub shared_root_tripwire_firings: u64,
}

#[derive(Clone, Debug)]
pub struct BusSignals {
    pub outbox_depth: i64,
    pub outbox_age_secs: i64,
    pub relay_published: i64,
    pub dead_letter_count: i64,
    pub publish_latency_millis: i64,
    pub dedup_hits: i64,
    pub dedup_deliveries: i64,
    pub consumer_lag: Vec<(String, i64)>,
    pub per_tenant_inflight: Vec<(String, i64)>,
    pub causal_depth_max: i64,
    pub shared_root_tripwire_firings: i64,
}

impl BusSignals {
    pub fn snapshot(
        store: &OutboxStore,
        drain: &DrainReport,
        obs: &BusObservations,
        now: &Timestamp,
        publish_latency_millis: i64,
    ) -> crate::Result<BusSignals> {
        let outbox_age_secs = store
            .try_oldest_unsent_recorded_at()?
            .map(|recorded| age_seconds(&recorded, now))
            .unwrap_or(0);

        Ok(BusSignals {
            outbox_depth: store.try_outbox_depth()? as i64,
            outbox_age_secs,
            relay_published: drain.published as i64,
            dead_letter_count: store.try_dead_letter_count()? as i64,
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
        })
    }

    pub fn emit_to<S: MetricsSink>(&self, sink: &mut S) {
        sink.emit(MetricSample::scalar(
            BusSignal::OutboxDepth,
            self.outbox_depth,
        ));
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

fn age_seconds(recorded_at: &Timestamp, now: &Timestamp) -> i64 {
    match (epoch_secs(&recorded_at.0), epoch_secs(&now.0)) {
        (Some(then), Some(at_now)) => (at_now - then).max(0),
        _ => 0,
    }
}

fn epoch_secs(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
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

    #[test]
    fn bus_emits_the_full_4_11_survival_signal_set() {
        let mut uniq = BusSignal::ALL.to_vec();
        let n = uniq.len();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), n, "every §4.11 Bus signal is distinct");
        assert_eq!(n, 11, "the §4.11 Bus contribution is covered exhaustively");
    }

    #[test]
    fn each_signal_has_the_right_name_and_unit() {
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
        for s in BusSignal::ALL {
            assert!(!s.metric_name().is_empty(), "{s:?} has a name");
            assert!(!s.unit().is_empty(), "{s:?} has a unit");
        }
    }

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
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
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

        let obs = BusObservations::default();
        let drain = DrainReport::default();
        let now = Timestamp("2026-06-19T00:01:30Z".into());
        let sig = BusSignals::snapshot(&store, &drain, &obs, &now, 0)
            .expect("outbox telemetry is readable");
        assert_eq!(sig.outbox_depth, 3, "3 committed-but-unsent rows");
        assert_eq!(sig.outbox_age_secs, 90, "oldest unsent row waited 90s");

        let mut rec = MetricRecorder::new();
        sig.emit_to(&mut rec);
        assert_eq!(rec.scalar(BusSignal::OutboxDepth), Some(3));
        assert_eq!(rec.scalar(BusSignal::OutboxAgeSecs), Some(90));
    }

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
        )
        .expect("outbox telemetry is readable");
        let mut rec = MetricRecorder::new();
        sig.emit_to(&mut rec);

        assert_eq!(rec.scalar(BusSignal::RelayPublished), Some(8));
        assert_eq!(rec.scalar(BusSignal::PublishLatencyMillis), Some(12));
        assert_eq!(rec.scalar(BusSignal::DedupHits), Some(2));
        assert_eq!(rec.scalar(BusSignal::DedupDeliveries), Some(10));
        assert_eq!(rec.scalar(BusSignal::CausalDepthMax), Some(3));

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

    #[test]
    fn age_seconds_is_clamped_and_calendar_correct() {
        assert_eq!(
            age_seconds(
                &Timestamp("2026-06-19T00:00:00Z".into()),
                &Timestamp("2026-06-19T00:01:30Z".into())
            ),
            90
        );
        assert_eq!(
            age_seconds(
                &Timestamp("2026-06-19T23:59:59Z".into()),
                &Timestamp("2026-06-20T00:00:09Z".into())
            ),
            10
        );
        assert_eq!(
            age_seconds(
                &Timestamp("2026-06-30T12:00:00Z".into()),
                &Timestamp("2026-07-01T12:00:00Z".into())
            ),
            86_400
        );
        assert_eq!(
            age_seconds(
                &Timestamp("2026-06-19T00:01:00Z".into()),
                &Timestamp("2026-06-19T00:00:00Z".into())
            ),
            0
        );
        assert_eq!(
            age_seconds(
                &Timestamp("not-a-timestamp".into()),
                &Timestamp("2026-06-19T00:00:00Z".into())
            ),
            0
        );
    }
}
