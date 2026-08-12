use std::sync::Arc;

use sqlx::postgres::PgPool;

use myelin_events::consumer::{Consumer, ConsumerSpec, Delivered, Message, SubscribeError};
use myelin_events::nats::NatsJetStreamBus;
use myelin_events::relay::{BusTransport, TransportError};
use myelin_events::{
    BusErasureLedger, DeadLetterSink, DedupLedger, DurableBusErasure, DurableDeadLetter,
    DurableDedup, EventHandler, Region, TenantId,
};

use crate::events_durable::{
    DurableBusErasureBacking, DurableDeadLetterBacking, DurableDedupBacking,
};
use crate::pg::PgError;
use crate::pgrelay::PgRelay;
use crate::provider::SubstrateProvider;
use crate::tenant_tx::{with_tenant_tx, TxScope};

pub const DEFAULT_DRAIN_BATCH: i64 = 256;

#[derive(Debug)]
pub enum EventsServeError {
    Transport(TransportError),
    Pg(PgError),
}

impl core::fmt::Display for EventsServeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EventsServeError::Transport(e) => write!(f, "events serve transport error: {}", e.0),
            EventsServeError::Pg(e) => write!(f, "events serve backend error: {e}"),
        }
    }
}

impl std::error::Error for EventsServeError {}

impl From<TransportError> for EventsServeError {
    fn from(e: TransportError) -> Self {
        EventsServeError::Transport(e)
    }
}

impl From<PgError> for EventsServeError {
    fn from(e: PgError) -> Self {
        EventsServeError::Pg(e)
    }
}

pub struct EventsRuntime {
    pool: PgPool,
    region: String,
    subject_root: String,
    consumer_name: String,
    relay: PgRelay,
    bus: NatsJetStreamBus,
    dedup_backing: DurableDedupBacking,
    dead_letter_backing: DurableDeadLetterBacking,
    bus_erasure_backing: DurableBusErasureBacking,
}

impl EventsRuntime {
    pub fn connect(
        provider: &SubstrateProvider,
        stream_name: &str,
        subject_root: &str,
        consumer_name: &str,
        rt: tokio::runtime::Handle,
    ) -> Result<EventsRuntime, EventsServeError> {
        let cfg = provider.config();
        Self::over_pool(
            provider.db_pool().clone(),
            &cfg.region,
            &cfg.nats_url,
            stream_name,
            subject_root,
            consumer_name,
            rt,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn over_pool(
        pool: PgPool,
        region: &str,
        nats_url: &str,
        stream_name: &str,
        subject_root: &str,
        consumer_name: &str,
        rt: tokio::runtime::Handle,
    ) -> Result<EventsRuntime, EventsServeError> {
        let bus = NatsJetStreamBus::connect(
            nats_url,
            stream_name,
            subject_root,
            consumer_name,
            rt.clone(),
        )?;
        let relay = PgRelay::new(pool.clone());
        let dedup_backing = DurableDedupBacking::new(pool.clone(), rt.clone());
        let dead_letter_backing = DurableDeadLetterBacking::new(pool.clone(), rt.clone());
        let bus_erasure_backing = DurableBusErasureBacking::new(pool.clone(), rt);
        Ok(EventsRuntime {
            pool,
            region: region.to_string(),
            subject_root: subject_root.to_string(),
            consumer_name: consumer_name.to_string(),
            relay,
            bus,
            dedup_backing,
            dead_letter_backing,
            bus_erasure_backing,
        })
    }

    pub fn relay(&self) -> &PgRelay {
        &self.relay
    }

    pub fn bus(&self) -> &NatsJetStreamBus {
        &self.bus
    }

    pub fn dedup_ledger(&self) -> DedupLedger {
        DedupLedger::durable(Arc::new(self.dedup_backing.clone()) as Arc<dyn DurableDedup>)
    }

    pub fn dead_letter_sink(&self) -> DeadLetterSink {
        DeadLetterSink::durable(
            Arc::new(self.dead_letter_backing.clone()) as Arc<dyn DurableDeadLetter>
        )
    }

    pub fn bus_erasure_ledger(&self, tenant: TenantId) -> BusErasureLedger {
        BusErasureLedger::durable(
            tenant,
            Region(self.region.clone()),
            Arc::new(self.bus_erasure_backing.clone()) as Arc<dyn DurableBusErasure>,
        )
    }

    pub fn consumer<H: EventHandler>(
        &self,
        spec: ConsumerSpec,
        handler: H,
    ) -> Result<Consumer<H>, SubscribeError> {
        myelin_events::consume(spec, handler, self.dedup_ledger())
            .map(|c| c.with_dead_letter_sink(self.dead_letter_sink()))
    }

    pub async fn with_tenant_tx<R, F>(&self, tenant: &str, op: F) -> Result<R, PgError>
    where
        F: for<'c> FnOnce(&'c mut sqlx::PgConnection) -> TxScope<'c, R> + Send,
        R: Send,
    {
        with_tenant_tx(&self.pool, tenant, &self.region, op).await
    }

    pub async fn drain_relay(&self, batch: i64) -> Result<usize, PgError> {
        self.relay.relay_once(&self.bus, batch).await
    }

    pub async fn drain_relay_to_empty(&self) -> Result<usize, PgError> {
        let mut total = 0usize;
        loop {
            let n = self
                .relay
                .relay_once(&self.bus, DEFAULT_DRAIN_BATCH)
                .await?;
            total += n;
            if n == 0 {
                break;
            }
        }
        Ok(total)
    }

    pub async fn outbox_depth(&self) -> Result<i64, PgError> {
        self.relay.outbox_depth().await
    }

    pub fn pump_consumer<H: EventHandler>(
        &self,
        consumer: &Consumer<H>,
        max_passes: usize,
    ) -> usize {
        let mut delivered = 0usize;
        for _ in 0..max_passes {
            let batch = self.bus.consume(&self.subject_root);
            if batch.is_empty() {
                break;
            }
            for envelope in batch {
                let event_id = envelope.event_id.clone();
                let msg = Message {
                    subject: envelope.subject.0.clone(),
                    envelope,
                };
                let delivered_outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        consumer.deliver(&msg)
                    }));
                match delivered_outcome {
                    Ok(Delivered::Acked | Delivered::Deduplicated | Delivered::DeadLettered(_)) => {
                        self.bus.ack(&self.consumer_name, &event_id);
                    }
                    Ok(
                        Delivered::Retried(_)
                        | Delivered::DependencyUnavailable(_, _)
                        | Delivered::Throttled(_),
                    ) => {}
                    Err(_panic) => {
                        eprintln!(
                            "[events-pump] LOUD: delivery machinery PANICKED for event_id={} \
                             subject={} - NOT acked (redeliverable; 0 lost), pump continues",
                            event_id.0, msg.subject
                        );
                    }
                }
                delivered += 1;
            }
        }
        delivered
    }
}
