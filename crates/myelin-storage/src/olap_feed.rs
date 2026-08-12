use std::collections::BTreeMap;

use myelin_events::{
    reindex, ArtifactRef, BusTransport, DataRole, EmitContextBase, EventEnvelope, InProcessBus,
    OutboxStore, Region, ReindexError, ReindexReceipt, ReindexSource, Relay, SnapshotDraft,
    SnapshotScope, Visibility,
};

use crate::olap::{OlapApply, OlapEvent, OlapIngestError, OlapReadStore};

#[derive(Clone, Debug)]
pub struct OlapBusConsumer {
    store: OlapReadStore,
}

impl OlapBusConsumer {
    pub fn boot(region: Region) -> OlapBusConsumer {
        OlapBusConsumer {
            store: OlapReadStore::pinned_to(region),
        }
    }

    pub fn ingest(&mut self, env: &EventEnvelope) -> Result<OlapApply, OlapIngestError> {
        self.store.apply(&OlapEvent::from_envelope(env))
    }

    pub fn ingest_batch(&mut self, envs: &[EventEnvelope]) -> Result<usize, OlapIngestError> {
        let mut fresh = 0;
        for env in envs {
            if self.ingest(env)? == OlapApply::Fresh {
                fresh += 1;
            }
        }
        Ok(fresh)
    }

    pub fn store(&self) -> &OlapReadStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut OlapReadStore {
        &mut self.store
    }

    pub fn parity_bytes(&self) -> Vec<u8> {
        self.store.parity_bytes()
    }
}

pub struct OlapAnalyticsSource {
    owner: String,
    truth: BTreeMap<String, (u64, Option<String>)>,
}

impl OlapAnalyticsSource {
    pub fn new(owner: impl Into<String>) -> OlapAnalyticsSource {
        OlapAnalyticsSource {
            owner: owner.into(),
            truth: BTreeMap::new(),
        }
    }

    pub fn upsert(&mut self, aggregate_row: &str, version: u64, subject: Option<&str>) {
        self.truth.insert(
            aggregate_row.to_string(),
            (version, subject.map(str::to_string)),
        );
    }

    fn snapshot_type(&self) -> myelin_events::EventType {
        myelin_events::EventType(format!(
            "{}.analytics.{}",
            self.owner,
            reindex::SNAPSHOT_EVENT_NAME
        ))
    }
}

impl ReindexSource for OlapAnalyticsSource {
    fn owner_token(&self) -> &str {
        &self.owner
    }

    fn replay(&self, _scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        self.truth
            .iter()
            .filter(|(_, (v, _))| since.is_none_or(|s| *v > s))
            .map(|(agg, (v, subject))| {
                let mut payload = serde_json::json!({ "aggregate_row": agg });
                if let Some(s) = subject {
                    payload["subject"] = serde_json::json!(s);
                }
                SnapshotDraft {
                    aggregate: myelin_events::AggregateKey(agg.clone()),
                    version: *v,
                    type_: self.snapshot_type(),
                    subject: ArtifactRef(
                        subject.clone().unwrap_or_else(|| {
                            format!("myelin://t/{}/analytics/{agg}", self.owner)
                        }),
                    ),
                    payload,
                    data_role: DataRole::Processor,
                    visibility: Visibility::Internal,
                }
            })
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn reindex_olap_from_bus(
    region: Region,
    scope: &SnapshotScope,
    sources: &[&dyn ReindexSource],
    outbox: &mut OutboxStore,
    bus: &InProcessBus,
    relay: &Relay<InProcessBus>,
    ctx_base: EmitContextBase,
    subject_prefix: &str,
) -> Result<(OlapBusConsumer, ReindexReceipt), ReindexError> {
    let receipt = reindex::reindex(scope, None, sources, outbox, ctx_base)?;

    relay.drain_to_empty();

    let mut consumer = OlapBusConsumer::boot(region);
    let published: Vec<EventEnvelope> = bus.consume(subject_prefix);
    consumer
        .ingest_batch(&published)
        .map_err(|e| ReindexError::OutboxFailed(format!("OLAP reindex ingest: {e}")))?;

    Ok((consumer, receipt))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OlapReindexParitySignal {
    pub store: &'static str,
    pub reindex_matches_live: bool,
    pub oltp_scan_path_count: u64,
    pub snapshots_emitted_first: usize,
    pub snapshots_emitted_second: usize,
}

impl OlapReindexParitySignal {
    pub fn is_green(&self) -> bool {
        self.reindex_matches_live
            && self.oltp_scan_path_count == 0
            && self.snapshots_emitted_second == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, EventId, EventType, OutboxStore, TenantId, Timestamp,
        OUTBOX_MIGRATION,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn region() -> Region {
        Region("fr-par".into())
    }
    fn tenant() -> TenantId {
        TenantId("01J0ACME".into())
    }
    fn now() -> Timestamp {
        Timestamp("2026-06-20T00:00:00Z".into())
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: now(),
            recorded_at: now(),
            caused_by: None,
        }
    }

    fn live_envelope(
        agg: &str,
        version: u64,
        event_id: &str,
        subject: Option<&str>,
    ) -> EventEnvelope {
        let mut payload = serde_json::json!({ "aggregate_row": agg, "version": version });
        if let Some(s) = subject {
            payload["subject"] = serde_json::json!(s);
        }
        EventEnvelope {
            event_id: EventId(event_id.into()),
            type_: EventType("olap_src.analytics.created".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            subject: ArtifactRef(
                subject
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("myelin://t/olap_src/analytics/{agg}")),
            ),
            aggregate: AggregateKey(agg.into()),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: now(),
            recorded_at: now(),
            payload,
        }
    }

    fn olap_source() -> OlapAnalyticsSource {
        let mut src = OlapAnalyticsSource::new("olap_src");
        src.upsert("issue:PROJ-1", 1, Some("subj:alice"));
        src.upsert("issue:PROJ-2", 2, Some("subj:bob"));
        src.upsert("issue:PROJ-3", 1, None);
        src
    }

    fn live_projection(src: &OlapAnalyticsSource) -> OlapBusConsumer {
        let mut consumer = OlapBusConsumer::boot(region());
        for draft in src.replay(&SnapshotScope::new("olap_src", "all"), None) {
            let subject = draft.payload.get("subject").and_then(|s| s.as_str());
            let env = live_envelope(
                &draft.aggregate.0,
                draft.version,
                &draft.event_id().0,
                subject,
            );
            consumer
                .ingest(&env)
                .expect("an in-region live event is admitted");
        }
        consumer
    }

    fn booted_bus() -> (OutboxStore, InProcessBus, Relay<InProcessBus>) {
        assert!(
            OUTBOX_MIGRATION.contains("event_id"),
            "the frozen outbox DDL is present"
        );
        let outbox = OutboxStore::new();
        let bus = InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), || {
            Timestamp("2026-06-20T00:00:02Z".into())
        });
        (outbox, bus, relay)
    }

    #[test]
    fn olap_store_is_fed_by_the_bus_idempotently() {
        let mut consumer = OlapBusConsumer::boot(region());
        let env = live_envelope("issue:PROJ-1", 1, "01J-1", Some("subj:alice"));
        assert_eq!(consumer.ingest(&env).unwrap(), OlapApply::Fresh);
        assert_eq!(
            consumer.ingest(&env).unwrap(),
            OlapApply::Duplicate,
            "a redelivery off the bus is a no-op (dedup on event_id)"
        );
        assert_eq!(consumer.store().doc_count(), 1, "exactly one projected doc");
        assert_eq!(consumer.store().oltp_scan_path_count(), 0);
    }

    #[test]
    fn an_out_of_region_bus_event_is_rejected_by_the_feed() {
        let mut consumer = OlapBusConsumer::boot(region());
        let mut env = live_envelope("issue:PROJ-1", 1, "01J-1", None);
        env.region = Region("us-east".into());
        let err = consumer
            .ingest(&env)
            .expect_err("out-of-region is rejected");
        assert!(matches!(err, OlapIngestError::OutOfRegion { .. }));
        assert_eq!(
            consumer.store().doc_count(),
            0,
            "nothing projected out-of-region"
        );
    }

    #[test]
    fn reindex_from_bus_byte_matches_live() {
        let src = olap_source();

        let live = live_projection(&src);
        assert_eq!(
            live.store().doc_count(),
            3,
            "all three facts projected live"
        );

        let (mut outbox, bus, relay) = booted_bus();
        let scope = SnapshotScope::new("olap_src", "all");
        let sources: Vec<&dyn ReindexSource> = vec![&src];
        let (cold, receipt) = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .expect("the OLAP reindex-from-bus succeeds");

        assert_eq!(
            receipt.snapshots_emitted, 3,
            "three snapshots re-emitted (the rebuild)"
        );
        assert_eq!(
            cold.store().doc_count(),
            3,
            "the cold rebuild projected all three"
        );
        assert_eq!(
            cold.parity_bytes(),
            live.parity_bytes(),
            "COLD reindex == LIVE projection, BYTE-FOR-BYTE (the F4 reindex-parity gate)"
        );
        assert_eq!(
            cold.store().oltp_scan_path_count(),
            0,
            "no OLTP-scan backdoor"
        );
    }

    #[test]
    fn a_second_reindex_emits_zero_new_snapshots() {
        let src = olap_source();
        let (mut outbox, bus, relay) = booted_bus();
        let scope = SnapshotScope::new("olap_src", "all");
        let sources: Vec<&dyn ReindexSource> = vec![&src];

        let (first, r1) = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .unwrap();
        assert_eq!(r1.snapshots_emitted, 3, "the first rebuild emits three");

        let (second, r2) = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .unwrap();
        assert_eq!(
            r2.snapshots_emitted, 0,
            "the re-run emits 0 NEW snapshots (idempotent)"
        );
        assert_eq!(
            r2.snapshots_skipped_duplicate, 3,
            "all three skipped as duplicate"
        );
        assert_eq!(
            first.parity_bytes(),
            second.parity_bytes(),
            "the parity bytes are byte-stable across re-runs"
        );
    }

    #[test]
    fn an_unknown_owner_reindex_fails_loudly() {
        let src = olap_source();
        let (mut outbox, bus, relay) = booted_bus();
        let scope = SnapshotScope::new("not_registered", "all");
        let sources: Vec<&dyn ReindexSource> = vec![&src];
        let err = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .expect_err("an unknown-owner reindex must fail loudly");
        assert!(matches!(err, ReindexError::NoSourceForOwner(_)));
    }

    #[test]
    fn olap_reindex_parity_signal_is_green() {
        let src = olap_source();
        let live = live_projection(&src);
        let (mut outbox, bus, relay) = booted_bus();
        let scope = SnapshotScope::new("olap_src", "all");
        let sources: Vec<&dyn ReindexSource> = vec![&src];

        let (cold, r1) = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .unwrap();
        let (_again, r2) = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .unwrap();

        let signal = OlapReindexParitySignal {
            store: "issue_analytics_olap",
            reindex_matches_live: cold.parity_bytes() == live.parity_bytes(),
            oltp_scan_path_count: cold.store().oltp_scan_path_count(),
            snapshots_emitted_first: r1.snapshots_emitted,
            snapshots_emitted_second: r2.snapshots_emitted,
        };
        assert!(
            signal.is_green(),
            "the F4 OLAP reindex-parity artifact is green: {signal:?}"
        );
        assert_eq!(signal.snapshots_emitted_first, 3);
        assert_eq!(signal.snapshots_emitted_second, 0);
    }

    #[test]
    fn olap_reindex_parity_signal_reads_red_when_any_invariant_fails() {
        let green = OlapReindexParitySignal {
            store: "issue_analytics_olap",
            reindex_matches_live: true,
            oltp_scan_path_count: 0,
            snapshots_emitted_first: 3,
            snapshots_emitted_second: 0,
        };
        assert!(green.is_green());
        assert!(!OlapReindexParitySignal {
            reindex_matches_live: false,
            ..green.clone()
        }
        .is_green());
        assert!(!OlapReindexParitySignal {
            oltp_scan_path_count: 1,
            ..green.clone()
        }
        .is_green());
        assert!(!OlapReindexParitySignal {
            snapshots_emitted_second: 1,
            ..green.clone()
        }
        .is_green());
    }

    fn subject_prefix() -> &'static str {
        ""
    }
}
