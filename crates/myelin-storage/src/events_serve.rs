//! # The events `serve()` composition root (SI-008/009, MR-023, the P-539 floor)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §3.3 (the outbox
//! relay) + `event-bus.md` §4.1/§5 (the relay + the idempotent consumer). Closes census
//! SI-008 ("relay publishes to an in-process FAKE bus") and SI-009 ("no events production
//! assembly/`serve` wiring NATS → outbox → relay") at the *production* level.
//!
//! ## The gap this fills
//! The events crate shipped the OUTBOX (`myelin_events::outbox`), the RELAY logic
//! (`myelin_events::relay`), the idempotent CONSUMER (`myelin_events::consumer`), and a real
//! durable bus (`myelin_events::nats::NatsJetStreamBus`) — but **nothing wired them into a running
//! pipeline**: the default delivery path was the in-process `relay::InProcessBus` FAKE, and there
//! was no composition root that constructed the REAL durable outbox + REAL broker + the relay
//! drain + the idempotent consumer together. [`EventsRuntime`] is that root — the analogue of the
//! MR-022 [`crate::provider::SubstrateProvider`] for the event-delivery pipeline.
//!
//! ## What it wires (the production default, not the in-memory fake)
//! - **The durable transactional OUTBOX** — [`crate::pgrelay::PgRelay`] over the provider's REAL
//!   `PgPool` (SI-007). A caller co-commits its business write + the outbox row in ONE
//!   tenant-scoped transaction via [`EventsRuntime::with_tenant_tx`] + [`PgRelay::co_commit_in_tx`]
//!   (the MR-022 convention) — the transactional-outbox guarantee (emit-iff-committed, BUS-D4).
//! - **The real BROKER** — [`myelin_events::nats::NatsJetStreamBus`] (a durable JetStream stream +
//!   durable PULL consumer + `Nats-Msg-Id = event_id` broker dedup → 0 ghost), NOT `InProcessBus`.
//! - **The relay DRAIN** — [`EventsRuntime::drain_relay`] runs [`PgRelay::relay_once`] (the ONE
//!   sanctioned publisher, `FOR UPDATE SKIP LOCKED` → mark-sent). A crash mid-publish leaves the
//!   row claimable → a re-run re-publishes → 0 lost; the broker dedup suppresses the re-publish →
//!   0 ghost. (No new `BusTransport::put` site lives here — publishing is the relay's alone, BUS-2.)
//! - **The idempotent CONSUMER** — [`EventsRuntime::consumer`] builds a
//!   [`myelin_events::consumer::Consumer`] via the sanctioned [`myelin_events::consume`] entry-point
//!   (durable bind-by-name + `*`-free whitelist + bounded prefetch), wired to the **durable**
//!   [`myelin_events::DedupLedger::durable`] (SI-023) so idempotency survives a process restart.
//!   [`EventsRuntime::pump_consumer`] pulls the broker and delivers, acking only terminal outcomes.
//!
//! ## Integration point with MR-007 (documented, honest scope)
//! MR-007's tuple/principal co-commit currently emits to the in-memory `OutboxStore` (the census
//! shortcut). THIS root makes the durable outbox (`PgRelay`) the production outbox the events
//! pipeline drains; re-pointing MR-007's identity call sites to co-commit through `PgRelay` is the
//! MR-009 / route-MR scope (the same place the identity stores flip from in-memory default to the
//! durable default). Until then, the durable outbox is AVAILABLE + USED by this `serve()` path, and
//! any subsystem (chat/issues/flow already do — see `myelin_chat::store::pg`) co-commits through it.
//!
//! Feature-gated `integration` (it pulls the real sqlx + async-nats clients), like the rest of the
//! live-backend code.

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

/// The default relay drain batch size (one `FOR UPDATE SKIP LOCKED` claim per pass).
pub const DEFAULT_DRAIN_BATCH: i64 = 256;

/// An error constructing or driving the events composition root.
#[derive(Debug)]
pub enum EventsServeError {
    /// The broker (NATS JetStream) could not be reached / the stream-consumer could not be ensured.
    Transport(TransportError),
    /// A relay/outbox query against the live DB failed.
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

/// **The events composition root.** Holds the durable outbox relay over the REAL pool, the REAL
/// JetStream broker, the durable dedup backing, and the `(tenant, region)` scope the co-commit
/// transactions run under. Cloneable handles throughout (pool + dedup are `Arc`-backed; the bus
/// is shared by `&`).
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
    /// **Boot the events pipeline over the MR-022 [`SubstrateProvider`] (the prod default path).**
    /// Reuses the provider's REAL bounded pool + region pin + the `nats_url` from its env-driven
    /// config; connects the durable JetStream stream + durable PULL consumer. The caller must have
    /// run [`SubstrateProvider::migrate_foundation`] (which applies the frozen
    /// [`myelin_events::CONSUMER_DEDUP_MIGRATION`] + [`myelin_events::OUTBOX_MIGRATION`]) first.
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

    /// Build the runtime over an explicit pool + region + `nats_url` (the test seam — e.g. the admin
    /// role for DDL, or a bounded pool). Connects the durable stream + durable PULL consumer.
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

    /// The durable transactional OUTBOX (the production outbox — SI-007). A caller co-commits its
    /// business write + the outbox row through [`Self::with_tenant_tx`] + [`PgRelay::co_commit_in_tx`].
    pub fn relay(&self) -> &PgRelay {
        &self.relay
    }

    /// The REAL JetStream broker the relay publishes to + the consumer reads from.
    pub fn bus(&self) -> &NatsJetStreamBus {
        &self.bus
    }

    /// **The DURABLE dedup ledger (SI-023)** — bound to the PG `consumer_dedup` table so consumer
    /// idempotency survives a process restart. Two `EventsRuntime`s over the same pool (e.g. across
    /// a restart) share the dedup state (it is in PG, not a per-process `HashSet`).
    pub fn dedup_ledger(&self) -> DedupLedger {
        DedupLedger::durable(Arc::new(self.dedup_backing.clone()) as Arc<dyn DurableDedup>)
    }

    /// **The DURABLE consumer dead-letter sink (CT-004d.2 chunk 6 / #7b)** — bound to the PG
    /// `consumer_dead_letter` table so a dead-lettered event (especially the H2 panic path) SURVIVES
    /// a process restart. The pump acks a dead-letter (terminal) so the broker cursor advances; with
    /// the in-memory sink that record vanished on restart (a debt the #7 H2 fix introduced) — the
    /// durable sink persists the PII-free `(consumer, event_id, reason)` row so the poison stays
    /// replayable after the bug is fixed + redeployed. Injected into every [`Consumer`] this runtime
    /// builds ([`EventsRuntime::consumer`]).
    pub fn dead_letter_sink(&self) -> DeadLetterSink {
        DeadLetterSink::durable(
            Arc::new(self.dead_letter_backing.clone()) as Arc<dyn DurableDeadLetter>
        )
    }

    /// **The DURABLE Bus erasure ledger (contract 10.8, W6c-events)** — bound to the PG
    /// `bus_erasure_ledger` table so an erasure record survives a process restart AND a backup restore
    /// (the non-shred-erasable property `BusHolder::re_erase_after_restore` replays). Scoped to the
    /// VERIFIED `tenant` + the runtime's region pin (the Bus never crosses a cell — residency-pin).
    /// Two `EventsRuntime`s over the same pool (e.g. across a restart) share the ledger state (it is in
    /// PG, not a per-process `BTreeMap`), so a restored pre-erase backup cannot silently resurrect a
    /// subject the ledger remembers erasing.
    pub fn bus_erasure_ledger(&self, tenant: TenantId) -> BusErasureLedger {
        BusErasureLedger::durable(
            tenant,
            Region(self.region.clone()),
            Arc::new(self.bus_erasure_backing.clone()) as Arc<dyn DurableBusErasure>,
        )
    }

    /// Build an idempotent [`Consumer`] through the sanctioned [`myelin_events::consume`] entry-point
    /// (durable bind-by-name, `*`-free whitelist, bounded prefetch), wired to the durable dedup
    /// ledger. This is the production consumer construction — not a hand-rolled subscription.
    pub fn consumer<H: EventHandler>(
        &self,
        spec: ConsumerSpec,
        handler: H,
    ) -> Result<Consumer<H>, SubscribeError> {
        // Wire the DURABLE dead-letter sink (CT-004d.2 chunk 6 / #7b) alongside the durable dedup
        // ledger so a service that opts into `serve()` gets a restart-surviving consumer DLQ.
        myelin_events::consume(spec, handler, self.dedup_ledger())
            .map(|c| c.with_dead_letter_sink(self.dead_letter_sink()))
    }

    /// **The MR-022 tenant-scoped-transaction convention bound to this runtime's pool.** A caller
    /// co-commits its business state write AND the outbox row ([`PgRelay::co_commit_in_tx`]) inside
    /// the SAME transaction here — both commit or neither (the transactional-outbox guarantee,
    /// emit-iff-committed BUS-D4). `tenant` is the VERIFIED tenant; the region is the runtime's pin.
    pub async fn with_tenant_tx<R, F>(&self, tenant: &str, op: F) -> Result<R, PgError>
    where
        F: for<'c> FnOnce(&'c mut sqlx::PgConnection) -> TxScope<'c, R> + Send,
        R: Send,
    {
        with_tenant_tx(&self.pool, tenant, &self.region, op).await
    }

    /// One relay drain pass — the production publisher. Claims unsent outbox rows
    /// (`FOR UPDATE SKIP LOCKED`), publishes each to the REAL broker, marks sent. Returns how many
    /// rows were published. (The only `BusTransport::put` site is inside [`PgRelay`], BUS-2.)
    pub async fn drain_relay(&self, batch: i64) -> Result<usize, PgError> {
        self.relay.relay_once(&self.bus, batch).await
    }

    /// Drain the outbox until depth is 0 (or a pass makes no progress). Returns the cumulative
    /// published count. This is the "outbox-depth drains" half of SUB-D1 against the real broker.
    pub async fn drain_relay_to_empty(&self) -> Result<usize, PgError> {
        let mut total = 0usize;
        loop {
            let n = self.relay.relay_once(&self.bus, DEFAULT_DRAIN_BATCH).await?;
            total += n;
            if n == 0 {
                break;
            }
        }
        Ok(total)
    }

    /// The count of unsent outbox rows (`published_at IS NULL`) — the `outbox_depth` signal.
    pub async fn outbox_depth(&self) -> Result<i64, PgError> {
        self.relay.outbox_depth().await
    }

    /// **Pump the broker into the idempotent consumer.** Pulls up to `max_passes` batches from the
    /// durable PULL consumer, delivers each envelope through the consumer's seven rules (dedup via
    /// the DURABLE ledger → a redelivery, even after a restart, is suppressed), and ACKs the broker
    /// message only on a TERMINAL outcome (`Acked`/`Deduplicated`/`DeadLettered`) so a `Retry` /
    /// `Throttled` redelivers (0 lost). Returns the number of messages delivered to the handler
    /// (excluding empty passes). This is the running event-delivery pipeline — the production
    /// consumer loop a service spawns (here as a bounded, testable pass-loop).
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
                // **H2 (peer-review #7 re-prosecution) — defense in depth at the pump boundary.**
                // `Consumer::deliver` already `catch_unwind`s the handler (a handler panic becomes a
                // graceful dead-letter, and the durable co-commit tx is a native `sqlx::Transaction`
                // that rolls back on drop — no leaked open tx). This OUTER guard covers a panic in the
                // delivery machinery ITSELF (a framework bug around `deliver`): it must not tear down
                // the whole pump task, and it must NEVER ack (a non-terminal panic leaves the message
                // pending so a later redelivery can re-run — 0 lost). A deterministic framework panic
                // is surfaced LOUDLY on every pass (never a silent swallow); the operator sees it.
                let delivered_outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| consumer.deliver(&msg)));
                match delivered_outcome {
                    // Terminal: the cursor advances — ack the broker message so it does not redeliver.
                    Ok(Delivered::Acked | Delivered::Deduplicated | Delivered::DeadLettered(_)) => {
                        self.bus.ack(&self.consumer_name, &event_id);
                    }
                    // NOT terminal: a Retry/Throttle stays pending — do NOT ack (it redelivers; 0 lost).
                    Ok(Delivered::Retried(_) | Delivered::Throttled(_)) => {}
                    // A panic in the delivery machinery itself (should never happen — the handler is
                    // already guarded inside `deliver`). Surface loudly, do NOT ack (0 lost), keep the
                    // pump alive so other subjects/tenants keep flowing.
                    Err(_panic) => {
                        eprintln!(
                            "[events-pump] LOUD: delivery machinery PANICKED for event_id={} \
                             subject={} — NOT acked (redeliverable; 0 lost), pump continues",
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
