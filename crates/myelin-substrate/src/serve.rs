use crate::holder_registered::{assert_all_holders_registered, HolderViolation, StoreManifest};
use crate::holders::{HolderRegistration, HolderRegistry, StoreKind};
use crate::metrics_health::{CriticalDependencies, HealthTable, MetricsHealthSurface};
use crate::migrations::{HotTables, Migration, MigrationRunner, Migrations};
use crate::topology::PublicSurface;
use crate::{Config, ServeError};
#[cfg(any(test, feature = "test-support"))]
use myelin_events::relay::InProcessBus;
use myelin_events::relay::{BusTransport, EventConsumer};
use myelin_events::{
    BrokerDeliveryBody, Consumer, ConsumerName, Delivered, DeliveryQuarantineReason, DeliveryToken,
    DurableDeliveryQuarantine, EventHandler, Message, OutboxStore, Relay, Timestamp,
};
use myelin_storage::{OltpConfig, OltpPool, OltpStoreHolder};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct OutboxSpec {
    store: OutboxStore,
    transport: Option<Box<dyn BusTransport>>,
    consumer_transport: Option<Box<dyn EventConsumer>>,
    delivery_quarantine: Option<Arc<dyn DurableDeliveryQuarantine>>,
}

impl OutboxSpec {
    #[cfg(any(test, feature = "test-support"))]
    pub fn default_inproc() -> OutboxSpec {
        OutboxSpec {
            store: OutboxStore::new(),
            transport: Some(Box::new(InProcessBus::new())),
            consumer_transport: None,
            delivery_quarantine: None,
        }
    }

    pub fn new(store: OutboxStore, transport: impl BusTransport + 'static) -> OutboxSpec {
        OutboxSpec {
            store,
            transport: Some(Box::new(transport)),
            consumer_transport: None,
            delivery_quarantine: None,
        }
    }

    pub fn durable(store: OutboxStore, transport: Box<dyn BusTransport>) -> OutboxSpec {
        OutboxSpec {
            store,
            transport: Some(transport),
            consumer_transport: None,
            delivery_quarantine: None,
        }
    }

    pub fn external_relay(store: OutboxStore) -> OutboxSpec {
        OutboxSpec {
            store,
            transport: None,
            consumer_transport: None,
            delivery_quarantine: None,
        }
    }

    pub fn external_relay_with_consumer(
        store: OutboxStore,
        consumer_transport: Box<dyn EventConsumer>,
        delivery_quarantine: Arc<dyn DurableDeliveryQuarantine>,
    ) -> OutboxSpec {
        OutboxSpec {
            store,
            transport: None,
            consumer_transport: Some(consumer_transport),
            delivery_quarantine: Some(delivery_quarantine),
        }
    }

    pub fn store(&self) -> &OutboxStore {
        &self.store
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for OutboxSpec {
    fn default() -> Self {
        OutboxSpec::default_inproc()
    }
}

trait RunnableConsumer: Send + Sync {
    fn name(&self) -> ConsumerName;
    fn accepts(&self, subject: &str) -> bool;
    fn deliver(&self, msg: &Message) -> Delivered;
    fn dead_letter_exhausted_retry(&self, msg: &Message, delivery_attempt: u64) -> Delivered;
    fn lag(&self) -> u64;
}

impl<H: EventHandler + Send + Sync> RunnableConsumer for Consumer<H> {
    fn name(&self) -> ConsumerName {
        Consumer::name(self).clone()
    }
    fn accepts(&self, subject: &str) -> bool {
        Consumer::accepts(self, subject)
    }
    fn deliver(&self, msg: &Message) -> Delivered {
        Consumer::deliver(self, msg)
    }
    fn dead_letter_exhausted_retry(&self, msg: &Message, delivery_attempt: u64) -> Delivered {
        Consumer::dead_letter_exhausted_retry(self, msg, delivery_attempt)
    }
    fn lag(&self) -> u64 {
        Consumer::lag(self)
    }
}

pub struct ConsumerReg {
    inner: Arc<dyn RunnableConsumer>,
}

impl ConsumerReg {
    pub fn new<H: EventHandler + Send + Sync + 'static>(consumer: Consumer<H>) -> ConsumerReg {
        ConsumerReg {
            inner: Arc::new(consumer),
        }
    }

    fn name(&self) -> ConsumerName {
        self.inner.name()
    }

    fn deliver(&self, msg: &Message) -> Delivered {
        self.inner.deliver(msg)
    }

    fn dead_letter_exhausted_retry(&self, msg: &Message, delivery_attempt: u64) -> Delivered {
        self.inner
            .dead_letter_exhausted_retry(msg, delivery_attempt)
    }

    fn accepts(&self, subject: &str) -> bool {
        self.inner.accepts(subject)
    }

    fn lag(&self) -> u64 {
        self.inner.lag()
    }
}

#[derive(Clone, Debug, Default)]
pub struct PublicRoutes(pub ());

#[derive(Clone, Debug, Default)]
pub struct InternalRpc(pub ());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HoldersSpec {
    #[default]
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Public,
    Internal,
    MetricsHealth,
}

pub struct PortOpener {
    opened: Vec<Surface>,
    public: PublicSurface,
    metrics_health: MetricsHealthSurface<HealthTable>,
    health: HealthTable,
}

impl Default for PortOpener {
    fn default() -> Self {
        let health = HealthTable::new();
        PortOpener {
            opened: Vec::new(),
            public: PublicSurface::default(),
            metrics_health: MetricsHealthSurface::new(
                CriticalDependencies::default(),
                health.clone(),
            ),
            health,
        }
    }
}

impl PortOpener {
    pub fn open_all(&mut self, critical: CriticalDependencies) {
        self.public = PublicSurface::default();
        self.health = HealthTable::new();
        self.metrics_health = MetricsHealthSurface::new(critical, self.health.clone());
        self.opened = vec![Surface::Public, Surface::Internal, Surface::MetricsHealth];
    }

    pub fn opened(&self) -> &[Surface] {
        &self.opened
    }

    pub fn public_surface(&self) -> &PublicSurface {
        &self.public
    }

    pub fn metrics_health(&self) -> &MetricsHealthSurface<HealthTable> {
        &self.metrics_health
    }

    pub fn health_probe(&self) -> &HealthTable {
        &self.health
    }

    pub fn mark_metrics_health_started(&self) {
        self.metrics_health.mark_started();
    }
}

#[derive(Clone, Default)]
pub struct Telemetry {
    inner: Arc<Mutex<TelemetryInner>>,
}

#[derive(Default)]
struct TelemetryInner {
    outbox_depth: i64,
    dead_letter_count: i64,
    consumer_lag: BTreeMap<String, i64>,
}

impl Telemetry {
    pub fn new() -> Telemetry {
        Telemetry::default()
    }

    fn observe(&self, outbox: &OutboxStore, consumers: &[ConsumerReg]) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.outbox_depth = outbox.outbox_depth() as i64;
        inner.dead_letter_count = outbox.dead_letter_count() as i64;
        inner.consumer_lag.clear();
        for c in consumers {
            inner.consumer_lag.insert(c.name().0, c.lag() as i64);
        }
    }

    pub fn outbox_depth(&self) -> i64 {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outbox_depth
    }

    pub fn dead_letter_count(&self) -> i64 {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .dead_letter_count
    }

    pub fn consumer_lag(&self, consumer: &str) -> Option<i64> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .consumer_lag
            .get(consumer)
            .copied()
    }
}

pub struct AppSpec {
    pub name: &'static str,
    pub config: Config,
    pub migrations: Migrations,
    pub hot_tables: HotTables,
    pub public: PublicRoutes,
    pub internal: InternalRpc,
    pub consumers: Vec<ConsumerReg>,
    pub holders: HoldersSpec,
    pub stores: StoreManifest,
    pub outbox: OutboxSpec,
    pub critical: CriticalDependencies,
}

impl AppSpec {
    pub const fn auto() -> HoldersSpec {
        HoldersSpec::Auto
    }

    pub fn minimal(name: &'static str, config: Config, outbox: OutboxSpec) -> AppSpec {
        AppSpec {
            name,
            config,
            migrations: Migrations::default(),
            hot_tables: HotTables::none(),
            public: PublicRoutes::default(),
            internal: InternalRpc::default(),
            consumers: Vec::new(),
            holders: HoldersSpec::Auto,
            stores: StoreManifest::new(),
            outbox,
            critical: CriticalDependencies::default(),
        }
    }
}

fn oltp_config_from(config: &Config) -> Result<OltpConfig, ServeError> {
    if config.0 == "BAD_POOL" {
        return Err(ServeError(
            "config validation failed at boot: OLTP pool config is unbounded (§3.2 fail-fast)"
                .into(),
        ));
    }
    let cfg = OltpConfig {
        max_pool_size: 32,
        statement_timeout_ms: 5_000,
        per_tenant_in_flight_cap: 8,
    };
    cfg.validate()
        .map_err(|e| ServeError(format!("OLTP pool config invalid at boot: {e}")))?;
    Ok(cfg)
}

pub struct ServeHandle {
    name: &'static str,
    pool: OltpPool,
    outbox: OutboxStore,
    relay: Option<Relay<RelayTransport>>,
    consumer_transport: Option<Box<dyn EventConsumer>>,
    delivery_quarantine: Option<Arc<dyn DurableDeliveryQuarantine>>,
    consumers: Vec<ConsumerReg>,
    holders: HolderRegistry,
    manifest: StoreManifest,
    ports: PortOpener,
    telemetry: Telemetry,
    draining: Arc<AtomicBool>,
}

type RelayTransport = Box<dyn BusTransport>;

pub const MAX_CONSUMER_DELIVERIES: u64 = 20;

impl ServeHandle {
    fn settle_retry(&self, token: DeliveryToken, delay_secs: u64) {
        if let Some(transport) = &self.consumer_transport {
            if transport.retry(token, delay_secs.min(300)).is_err() {
                self.health_probe().mark_down("broker");
            } else {
                self.health_probe().mark_up("broker");
            }
        }
    }

    fn quarantine_then_terminate(
        &self,
        token: DeliveryToken,
        broker_ref: &myelin_events::BrokerDeliveryRef,
        delivery_attempt: u64,
        reason: DeliveryQuarantineReason,
    ) {
        let Some(transport) = &self.consumer_transport else {
            return;
        };
        let Some(quarantine) = &self.delivery_quarantine else {
            self.health_probe().mark_down("oltp");
            self.settle_retry(token, 1);
            return;
        };
        if quarantine
            .record(
                transport.durable_name(),
                broker_ref,
                reason,
                delivery_attempt,
            )
            .is_err()
        {
            self.health_probe().mark_down("oltp");
            self.settle_retry(token, 1);
            return;
        }
        self.health_probe().mark_up("oltp");
        if transport.terminate(token).is_err() {
            self.health_probe().mark_down("broker");
        } else {
            self.health_probe().mark_up("broker");
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn pool(&self) -> &OltpPool {
        &self.pool
    }

    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    pub fn registered_holders(&self) -> &[HolderRegistration] {
        self.holders.registrations()
    }

    pub fn holder_registry(&self) -> &HolderRegistry {
        &self.holders
    }

    pub fn store_manifest(&self) -> &StoreManifest {
        &self.manifest
    }

    pub fn holder_registered(&self) -> Result<(), Vec<HolderViolation>> {
        assert_all_holders_registered(&self.manifest, &self.holders)
    }

    pub fn surfaces(&self) -> &[Surface] {
        self.ports.opened()
    }

    pub fn public_surface(&self) -> &PublicSurface {
        self.ports.public_surface()
    }

    pub fn metrics_health(&self) -> &MetricsHealthSurface<HealthTable> {
        self.ports.metrics_health()
    }

    pub fn health_probe(&self) -> &HealthTable {
        self.ports.health_probe()
    }

    pub fn telemetry(&self) -> &Telemetry {
        self.telemetry.observe(&self.outbox, &self.consumers);
        &self.telemetry
    }

    pub fn tick(&self) -> Vec<(ConsumerName, usize)> {
        if let Some(relay) = &self.relay {
            relay.drain_to_empty();
        }

        if self.is_draining() && self.consumer_transport.is_some() {
            self.telemetry.observe(&self.outbox, &self.consumers);
            return Vec::new();
        }

        let batch = if let Some(transport) = &self.consumer_transport {
            match transport.pre_intake_readiness() {
                Ok(Some(dependency)) => self.health_probe().mark_up(dependency.name()),
                Ok(None) => {}
                Err(dependency) => {
                    self.health_probe().mark_down(dependency.name());
                    self.telemetry.observe(&self.outbox, &self.consumers);
                    return Vec::new();
                }
            }
            match transport.consume("") {
                Ok(batch) => {
                    self.health_probe().mark_up("broker");
                    batch
                }
                Err(error) => {
                    eprintln!("[{}] broker pull failed: {}", self.name, error.0);
                    self.health_probe().mark_down("broker");
                    self.telemetry.observe(&self.outbox, &self.consumers);
                    return Vec::new();
                }
            }
        } else if let Some(relay) = &self.relay {
            relay
                .transport()
                .consume("")
                .into_iter()
                .map(|envelope| myelin_events::BrokerDelivery {
                    token: DeliveryToken::new(1).expect("one is a valid delivery token"),
                    broker_ref: None,
                    body: BrokerDeliveryBody::Event(Box::new(envelope)),
                    delivery_attempt: Some(1),
                })
                .collect()
        } else {
            self.telemetry.observe(&self.outbox, &self.consumers);
            return Vec::new();
        };

        let mut delivered: Vec<(ConsumerName, usize)> = self
            .consumers
            .iter()
            .map(|consumer| (consumer.name(), 0))
            .collect();
        for delivery in batch {
            let token = delivery.token;
            let delivery_attempt = delivery.delivery_attempt;
            let broker_ref = delivery.broker_ref;
            let env = match delivery.body {
                BrokerDeliveryBody::TransientMetadataFault => {
                    self.settle_retry(token, 1);
                    continue;
                }
                BrokerDeliveryBody::Poison(kind) => {
                    if let (Some(broker_ref), Some(delivery_attempt)) =
                        (broker_ref.as_ref(), delivery_attempt)
                    {
                        self.quarantine_then_terminate(
                            token,
                            broker_ref,
                            delivery_attempt,
                            kind.into(),
                        );
                    } else {
                        self.settle_retry(token, 1);
                    }
                    continue;
                }
                BrokerDeliveryBody::Event(envelope) => *envelope,
            };
            if self.consumer_transport.is_some()
                && (delivery_attempt.is_none() || broker_ref.is_none())
            {
                self.settle_retry(token, 1);
                continue;
            }
            let delivery_attempt = delivery_attempt.unwrap_or(1);
            let matching: Vec<usize> = self
                .consumers
                .iter()
                .enumerate()
                .filter_map(|(index, consumer)| consumer.accepts(&env.subject.0).then_some(index))
                .collect();
            if matching.is_empty() {
                if let Some(broker_ref) = broker_ref.as_ref() {
                    self.quarantine_then_terminate(
                        token,
                        broker_ref,
                        delivery_attempt,
                        DeliveryQuarantineReason::NoRegisteredConsumer,
                    );
                } else if self.consumer_transport.is_some() {
                    self.settle_retry(token, 1);
                }
                continue;
            }

            let consumer_name = self.consumers[matching[0]].name();
            let mut terminal = true;
            let mut exhausted = false;
            let mut retry_after_secs = None;
            for index in matching {
                let msg = Message {
                    subject: env.subject.0.clone(),
                    envelope: env.clone(),
                };
                let outcome = self.consumers[index].deliver(&msg);
                match outcome {
                    Delivered::Acked | Delivered::Deduplicated => delivered[index].1 += 1,
                    Delivered::DeadLettered(_) => {}
                    Delivered::Retried(delay_secs) => {
                        if delivery_attempt >= MAX_CONSUMER_DELIVERIES {
                            match self.consumers[index]
                                .dead_letter_exhausted_retry(&msg, delivery_attempt)
                            {
                                Delivered::DeadLettered(_) => exhausted = true,
                                Delivered::Retried(dlq_retry_secs) => {
                                    terminal = false;
                                    retry_after_secs = Some(
                                        retry_after_secs.map_or(dlq_retry_secs, |current: u64| {
                                            current.max(dlq_retry_secs)
                                        }),
                                    );
                                }
                                _ => unreachable!("retry exhaustion returns DLQ or Retry"),
                            }
                        } else {
                            terminal = false;
                            retry_after_secs = Some(
                                retry_after_secs
                                    .map_or(delay_secs, |current: u64| current.max(delay_secs)),
                            );
                        }
                    }
                    Delivered::DependencyUnavailable(dependency, delay_secs) => {
                        self.health_probe().mark_down(dependency.name());
                        terminal = false;
                        retry_after_secs = Some(
                            retry_after_secs
                                .map_or(delay_secs, |current: u64| current.max(delay_secs)),
                        );
                    }
                    Delivered::Throttled(_) => {
                        terminal = false;
                        retry_after_secs = Some(retry_after_secs.unwrap_or(1).max(1));
                    }
                }
            }

            if terminal {
                if let Some(transport) = &self.consumer_transport {
                    let result = if exhausted {
                        transport.terminate(token)
                    } else {
                        transport.ack(token)
                    };
                    if let Err(error) = result {
                        eprintln!(
                            "[{}] broker terminal acknowledgement failed for event {}: {}",
                            self.name, env.event_id.0, error.0
                        );
                        self.health_probe().mark_down("broker");
                    } else {
                        self.health_probe().mark_up("broker");
                    }
                } else if let Some(relay) = &self.relay {
                    relay.transport().ack(&consumer_name.0, &env.event_id);
                }
            } else if let (Some(transport), Some(delay_secs)) =
                (&self.consumer_transport, retry_after_secs)
            {
                if let Err(error) = transport.retry(token, delay_secs.min(300)) {
                    eprintln!(
                        "[{}] broker retry NAK failed for event {}: {}",
                        self.name, env.event_id.0, error.0
                    );
                    self.health_probe().mark_down("broker");
                }
            }
        }
        self.telemetry.observe(&self.outbox, &self.consumers);
        delivered
    }

    pub fn signal_drain(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    pub fn drain_checked(self) -> Result<Telemetry, ServeError> {
        self.draining.store(true, Ordering::SeqCst);
        if let Some(transport) = &self.consumer_transport {
            if transport.flush_settlements().is_err() {
                self.health_probe().mark_down("broker");
                return Err(ServeError(
                    "graceful drain has unresolved broker settlements".into(),
                ));
            }
            self.health_probe().mark_up("broker");
        }
        self.tick();
        self.telemetry.observe(&self.outbox, &self.consumers);
        Ok(self.telemetry)
    }

    pub fn drain(self) -> Telemetry {
        self.drain_checked()
            .expect("graceful drain must reconcile broker settlements")
    }

    fn owns_relay(&self) -> bool {
        self.relay.is_some()
    }
}

pub fn boot(spec: AppSpec) -> Result<ServeHandle, ServeError> {
    let AppSpec {
        name,
        config,
        migrations,
        hot_tables,
        public: _public,
        internal: _internal,
        consumers,
        holders,
        stores,
        outbox,
        critical,
    } = spec;

    let pool_config = oltp_config_from(&config)?;
    // @residency-cell-pinned — NAMED M0 FLOOR (residency-pin lint, P-ST-04 → P-020): the boot pool
    let pool = OltpPool::open(pool_config)
        .map_err(|e| ServeError(format!("failed to open the OLTP pool at boot: {e}")))?;

    let mut runner = MigrationRunner::new();
    let mut full_migrations = Migrations(vec![
        Migration::plain("0000_outbox", myelin_events::OUTBOX_MIGRATION),
        Migration::plain(
            "0001_consumer_dedup",
            myelin_events::CONSUMER_DEDUP_MIGRATION,
        ),
        Migration::plain(
            "0002_consumer_dead_letter",
            myelin_events::CONSUMER_DEAD_LETTER_MIGRATION,
        ),
        Migration::plain(
            "0003_outbox_quarantine",
            myelin_events::OUTBOX_QUARANTINE_MIGRATION,
        ),
        Migration::plain(
            "0004_consumer_delivery_quarantine",
            myelin_events::CONSUMER_DELIVERY_QUARANTINE_MIGRATION,
        ),
        Migration::plain(
            "0005_outbox_publisher_grants",
            myelin_events::OUTBOX_PUBLISHER_GRANTS_MIGRATION,
        ),
        Migration::plain(
            "0006_outbox_publisher_grant_scope",
            myelin_events::OUTBOX_PUBLISHER_GRANT_SCOPE_MIGRATION,
        ),
    ]);
    full_migrations.0.extend(migrations.0);
    runner.run(&full_migrations, &hot_tables)?;

    let HoldersSpec::Auto = holders;
    let mut holder_registry = HolderRegistry::new();
    let oltp_holder = OltpStoreHolder::new(name);
    let _oltp_receipt = oltp_holder.register();
    holder_registry.open(StoreKind::Oltp, name);
    for store in stores.stores() {
        holder_registry.open(store.kind, store.name);
    }
    let mut full_manifest = StoreManifest::of([crate::holder_registered::DeclaredStore::new(
        StoreKind::Oltp,
        name,
    )]);
    for store in stores.stores() {
        full_manifest.declare(store.kind, store.name);
    }

    let OutboxSpec {
        store: outbox_store,
        transport,
        consumer_transport,
        delivery_quarantine,
    } = outbox;
    if transport.is_none() && consumer_transport.is_none() && !consumers.is_empty() {
        return Err(ServeError(format!(
            "service {name} registered consumers without a consumer transport; external-relay \
             producer mode cannot consume"
        )));
    }
    if consumer_transport.is_some() && delivery_quarantine.is_none() {
        return Err(ServeError(format!(
            "service {name} configured external consumer transport without durable delivery quarantine"
        )));
    }
    let relay = transport.map(|transport| {
        Relay::new(outbox_store.clone(), transport, || {
            Timestamp("1970-01-01T00:00:00Z".into())
        })
    });

    let mut full_critical = vec!["oltp".to_string()];
    full_critical.extend(critical.deps().iter().map(|d| d.0.clone()));
    let mut ports = PortOpener::default();
    ports.open_all(CriticalDependencies::new(full_critical));

    if let Some(consumer_transport) = consumer_transport.as_ref() {
        match consumer_transport.pre_intake_readiness() {
            Ok(Some(dependency)) => ports.health_probe().mark_up(dependency.name()),
            Ok(None) => {}
            Err(dependency) => ports.health_probe().mark_down(dependency.name()),
        }
    }

    let telemetry = Telemetry::new();
    telemetry.observe(&outbox_store, &consumers);

    ports.mark_metrics_health_started();

    Ok(ServeHandle {
        name,
        pool,
        outbox: outbox_store,
        relay,
        consumer_transport,
        delivery_quarantine,
        consumers,
        holders: holder_registry,
        manifest: full_manifest,
        ports,
        telemetry,
        draining: Arc::new(AtomicBool::new(false)),
    })
}

pub fn serve(spec: AppSpec) -> Result<(), ServeError> {
    let handle = boot(spec)?;
    let owns_relay = handle.owns_relay();
    handle.tick();
    handle.signal_drain();
    let final_telemetry = handle.drain_checked()?;
    if owns_relay && final_telemetry.outbox_depth() != 0 {
        return Err(ServeError(format!(
            "graceful drain incomplete: outbox_depth = {} (expected 0)",
            final_telemetry.outbox_depth()
        )));
    }
    Ok(())
}

pub async fn serve_until_shutdown<F>(spec: AppSpec, shutdown: F) -> Result<(), ServeError>
where
    F: std::future::Future<Output = ()>,
{
    let handle = boot(spec)?;
    let owns_relay = handle.owns_relay();
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            biased;
            () = &mut shutdown => break,
            _ = interval.tick() => { handle.tick(); }
        }
    }

    handle.signal_drain();
    let final_telemetry = handle.drain_checked()?;
    if owns_relay && final_telemetry.outbox_depth() != 0 {
        return Err(ServeError(format!(
            "graceful drain incomplete: outbox_depth = {} (expected 0)",
            final_telemetry.outbox_depth()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, Backoff, DataRole, DedupLedger, EmitContextBase,
        EventDraft, EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxTx,
        PrefetchBound, Reason, Subscription, Visibility,
    };
    use myelin_events::{HandleOutcome, SubjectPattern};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicU32;

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(principal()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            caused_by: None,
        }
    }

    fn draft(type_: &str) -> EventDraft {
        EventDraft {
            type_: EventType(type_.into()),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            payload: serde_json::json!({ "ref": "PROJ-1" }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    static SUBJECTS: &[SubjectPattern] = &[];

    fn hello_consumer(dedup: DedupLedger) -> (ConsumerReg, Arc<AtomicU32>) {
        let runs = Arc::new(AtomicU32::new(0));
        struct H {
            runs: Arc<AtomicU32>,
        }
        impl EventHandler for H {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(
                &self,
                _ev: &EventEnvelope,
                _tx: &mut myelin_events::HandlerTx<'_>,
            ) -> HandleOutcome {
                self.runs.fetch_add(1, Ordering::SeqCst);
                HandleOutcome::Done
            }
        }
        let sub = Subscription::bind(
            ConsumerName("indexer".into()),
            &["myelin://acme/issues/"],
            PrefetchBound::DEFAULT,
        )
        .unwrap();
        let consumer = Consumer::new(H { runs: runs.clone() }, sub, dedup);
        (ConsumerReg::new(consumer), runs)
    }

    fn event_for_transport() -> EventEnvelope {
        let outbox = OutboxStore::new();
        let bus = InProcessBus::new();
        let mut tx = outbox.begin(Arc::new(MonotonicMinter::new()), ctx_base());
        tx.stage_state_change("transport fixture");
        tx.emit(draft("issues.issue.created"), None).unwrap();
        tx.commit().unwrap();
        Relay::new(outbox, bus.clone(), || {
            Timestamp("2026-06-19T00:00:02Z".into())
        })
        .drain_to_empty();
        bus.consume("").into_iter().next().unwrap()
    }

    fn token(value: u64) -> DeliveryToken {
        DeliveryToken::new(value).expect("test delivery token is non-zero")
    }

    #[derive(Clone, Default)]
    struct PullProbe {
        state: Arc<Mutex<PullProbeState>>,
    }

    #[derive(Default)]
    struct PullProbeState {
        batches: VecDeque<Vec<myelin_events::BrokerDelivery>>,
        pulls: usize,
        acks: Vec<DeliveryToken>,
        retries: Vec<DeliveryToken>,
        terms: Vec<DeliveryToken>,
        fail_ack: bool,
        flushes: usize,
        fail_flush: bool,
    }

    impl PullProbe {
        fn with_batches(batches: impl IntoIterator<Item = Vec<EventEnvelope>>) -> Self {
            let mut next_token = 0_u64;
            Self {
                state: Arc::new(Mutex::new(PullProbeState {
                    batches: batches
                        .into_iter()
                        .map(|batch| {
                            batch
                                .into_iter()
                                .map(|envelope| {
                                    next_token += 1;
                                    myelin_events::BrokerDelivery {
                                        token: token(next_token),
                                        broker_ref: Some(myelin_events::BrokerDeliveryRef {
                                            stream: "TEST".into(),
                                            stream_sequence: next_token,
                                        }),
                                        body: BrokerDeliveryBody::Event(Box::new(envelope)),
                                        delivery_attempt: Some(1),
                                    }
                                })
                                .collect()
                        })
                        .collect(),
                    ..Default::default()
                })),
            }
        }

        fn state(&self) -> std::sync::MutexGuard<'_, PullProbeState> {
            self.state.lock().unwrap_or_else(|error| error.into_inner())
        }

        fn with_delivery(envelope: EventEnvelope, delivery_attempt: u64) -> Self {
            Self {
                state: Arc::new(Mutex::new(PullProbeState {
                    batches: [vec![myelin_events::BrokerDelivery {
                        token: token(1),
                        broker_ref: Some(myelin_events::BrokerDeliveryRef {
                            stream: "TEST".into(),
                            stream_sequence: 1,
                        }),
                        body: BrokerDeliveryBody::Event(Box::new(envelope)),
                        delivery_attempt: Some(delivery_attempt),
                    }]]
                    .into_iter()
                    .collect(),
                    ..Default::default()
                })),
            }
        }
    }

    impl EventConsumer for PullProbe {
        fn durable_name(&self) -> &str {
            "test-durable"
        }

        fn consume(
            &self,
            _subject_prefix: &str,
        ) -> Result<Vec<myelin_events::BrokerDelivery>, myelin_events::TransportError> {
            let mut state = self.state();
            state.pulls += 1;
            Ok(state.batches.pop_front().unwrap_or_default())
        }

        fn flush_settlements(&self) -> Result<(), myelin_events::TransportError> {
            let mut state = self.state();
            state.flushes += 1;
            if state.fail_flush {
                Err(myelin_events::TransportError(
                    "settlement flush unavailable".into(),
                ))
            } else {
                Ok(())
            }
        }

        fn ack(&self, token: DeliveryToken) -> Result<(), myelin_events::TransportError> {
            let mut state = self.state();
            if state.fail_ack {
                return Err(myelin_events::TransportError("ack unavailable".into()));
            }
            state.acks.push(token);
            Ok(())
        }

        fn retry(
            &self,
            token: DeliveryToken,
            _delay_secs: u64,
        ) -> Result<(), myelin_events::TransportError> {
            self.state().retries.push(token);
            Ok(())
        }

        fn terminate(&self, token: DeliveryToken) -> Result<(), myelin_events::TransportError> {
            self.state().terms.push(token);
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct QuarantineProbe {
        records: Arc<Mutex<Vec<(myelin_events::BrokerDeliveryRef, DeliveryQuarantineReason)>>>,
        fail_next: Arc<AtomicBool>,
    }

    impl DurableDeliveryQuarantine for QuarantineProbe {
        fn record(
            &self,
            _consumer: &str,
            broker_ref: &myelin_events::BrokerDeliveryRef,
            reason: DeliveryQuarantineReason,
            _delivery_attempt: u64,
        ) -> Result<(), String> {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err("injected quarantine failure".into());
            }
            self.records
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((broker_ref.clone(), reason));
            Ok(())
        }
    }

    fn consumer_named(name: &str, prefix: &'static str, retry_first: bool) -> ConsumerReg {
        struct H {
            retry_first: bool,
            calls: AtomicU32,
        }
        impl EventHandler for H {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(
                &self,
                _event: &EventEnvelope,
                _tx: &mut myelin_events::HandlerTx<'_>,
            ) -> HandleOutcome {
                if self.retry_first && self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    HandleOutcome::Retry(Backoff { seconds: 2 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
        ConsumerReg::new(Consumer::new(
            H {
                retry_first,
                calls: AtomicU32::new(0),
            },
            Subscription::bind(ConsumerName(name.into()), &[prefix], PrefetchBound::DEFAULT)
                .unwrap(),
            DedupLedger::new(),
        ))
    }

    #[derive(Clone, Default)]
    struct DlqProbe {
        records: Arc<Mutex<Vec<myelin_events::DeadLetterRecord>>>,
        fail_next: Arc<AtomicU32>,
    }

    impl myelin_events::DurableDeadLetter for DlqProbe {
        fn record(
            &self,
            consumer: &ConsumerName,
            event_id: &EventId,
            reason: &str,
        ) -> Result<(), String> {
            if self
                .fail_next
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    if left > 0 {
                        Some(left - 1)
                    } else {
                        None
                    }
                })
                .is_ok()
            {
                return Err("DLQ unavailable".into());
            }
            self.records
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(myelin_events::DeadLetterRecord {
                    consumer: consumer.clone(),
                    event_id: event_id.clone(),
                    reason: reason.into(),
                });
            Ok(())
        }

        fn dead_letters(&self, consumer: &ConsumerName) -> Vec<myelin_events::DeadLetterRecord> {
            self.records
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .filter(|record| &record.consumer == consumer)
                .cloned()
                .collect()
        }
    }

    fn always_retry_consumer(dedup: DedupLedger, dlq: DlqProbe) -> (ConsumerReg, Arc<AtomicU32>) {
        struct H(Arc<AtomicU32>);
        impl EventHandler for H {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(
                &self,
                _event: &EventEnvelope,
                _tx: &mut myelin_events::HandlerTx<'_>,
            ) -> HandleOutcome {
                self.0.fetch_add(1, Ordering::SeqCst);
                HandleOutcome::Retry(Backoff { seconds: 2 })
            }
        }
        let calls = Arc::new(AtomicU32::new(0));
        let consumer = ConsumerReg::new(
            Consumer::new(
                H(calls.clone()),
                Subscription::bind(
                    ConsumerName("retrying".into()),
                    &["myelin://acme/issues/"],
                    PrefetchBound::DEFAULT,
                )
                .unwrap(),
                dedup,
            )
            .with_dead_letter_sink(myelin_events::DeadLetterSink::durable(Arc::new(dlq))),
        );
        (consumer, calls)
    }

    fn dependency_unavailable_consumer(dedup: DedupLedger, dlq: DlqProbe) -> ConsumerReg {
        struct H;
        impl EventHandler for H {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(
                &self,
                _event: &EventEnvelope,
                _tx: &mut myelin_events::HandlerTx<'_>,
            ) -> HandleOutcome {
                HandleOutcome::DependencyUnavailable {
                    dependency: myelin_events::relay::IntakeDependency::Blob,
                    backoff: Backoff { seconds: 2 },
                }
            }
        }
        ConsumerReg::new(
            Consumer::new(
                H,
                Subscription::bind(
                    ConsumerName("blob-dependent".into()),
                    &["myelin://acme/issues/"],
                    PrefetchBound::DEFAULT,
                )
                .unwrap(),
                dedup,
            )
            .with_dead_letter_sink(myelin_events::DeadLetterSink::durable(Arc::new(dlq))),
        )
    }

    fn external_consumer_spec(probe: PullProbe, consumers: Vec<ConsumerReg>) -> AppSpec {
        external_consumer_spec_with_quarantine(
            probe,
            consumers,
            Arc::new(QuarantineProbe::default()),
        )
    }

    fn external_consumer_spec_with_quarantine(
        probe: PullProbe,
        consumers: Vec<ConsumerReg>,
        quarantine: Arc<dyn DurableDeliveryQuarantine>,
    ) -> AppSpec {
        let mut spec = AppSpec::minimal(
            "external-consumer",
            Config::default(),
            OutboxSpec::external_relay_with_consumer(
                OutboxStore::new(),
                Box::new(probe),
                quarantine,
            ),
        );
        spec.consumers = consumers;
        spec.critical = CriticalDependencies::new(["broker"]);
        spec
    }

    #[tokio::test]
    async fn shutdown_ready_at_boot_wins_before_the_first_intake_pull() {
        let probe = PullProbe::default();
        let observed = probe.clone();

        serve_until_shutdown(external_consumer_spec(probe, Vec::new()), async {})
            .await
            .expect("immediate shutdown drains cleanly");

        assert_eq!(
            observed.state().pulls,
            0,
            "a ready shutdown future prevents fresh intake"
        );
    }

    #[tokio::test]
    async fn shutdown_fails_loudly_when_broker_settlements_cannot_flush_without_pulling() {
        let probe = PullProbe::default();
        probe.state().fail_flush = true;
        let observed = probe.clone();

        let error = serve_until_shutdown(external_consumer_spec(probe, Vec::new()), async {})
            .await
            .expect_err("unresolved settlement makes the drain unclean");

        assert_eq!(error.0, "graceful drain has unresolved broker settlements");
        let state = observed.state();
        assert_eq!(state.pulls, 0, "shutdown settlement flush never pulls work");
        assert_eq!(
            state.flushes, 1,
            "shutdown attempts the retained intents once"
        );
    }

    #[tokio::test]
    async fn steady_state_ticks_until_shutdown_then_drains_without_an_extra_pull() {
        let probe = PullProbe::default();
        let observed = probe.clone();
        let shutdown_probe = probe.clone();

        serve_until_shutdown(external_consumer_spec(probe, Vec::new()), async move {
            loop {
                if shutdown_probe.state().pulls > 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("signalled steady-state loop drains cleanly");

        assert_eq!(
            observed.state().pulls,
            1,
            "shutdown wins the next select and drain never starts another pull"
        );
    }

    #[test]
    fn hello_world_boot_emit_consume_drain() {
        let outbox = OutboxStore::new();
        let (consumer, runs) = hello_consumer(DedupLedger::new());

        let spec = AppSpec {
            name: "hello",
            config: Config::default(),
            migrations: Migrations::new([(
                "0010_hello",
                "CREATE TABLE IF NOT EXISTS hello (id TEXT)",
            )]),
            hot_tables: HotTables::none(),
            public: PublicRoutes::default(),
            internal: InternalRpc::default(),
            consumers: vec![consumer],
            holders: AppSpec::auto(),
            stores: StoreManifest::new(),
            outbox: OutboxSpec::new(outbox.clone(), InProcessBus::new()),
            critical: CriticalDependencies::default(),
        };

        let handle = boot(spec).expect("boot succeeds");
        assert_eq!(handle.name(), "hello");
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports opened in the lifecycle"
        );
        assert_eq!(
            handle.registered_holders(),
            &[HolderRegistration {
                kind: StoreKind::Oltp,
                name: "hello"
            }],
            "the OLTP store auto-registered as a holder (§3.4)"
        );
        assert!(
            handle
                .holder_registry()
                .is_registered(StoreKind::Oltp, "hello"),
            "no store escaped registration (opening IS registering)"
        );

        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let mut tx = outbox.begin(minter, ctx_base());
        tx.stage_state_change("hello created");
        tx.emit(draft("issues.issue.created"), None).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            handle.outbox().outbox_depth(),
            1,
            "one committed-but-unsent event"
        );

        let delivered = handle.tick();
        assert_eq!(
            delivered,
            vec![(ConsumerName("indexer".into()), 1)],
            "the consumer saw 1 event"
        );
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the handler ran exactly once"
        );
        assert_eq!(
            handle.outbox().outbox_depth(),
            0,
            "the relay drained the outbox"
        );

        let t = handle.telemetry();
        assert_eq!(t.outbox_depth(), 0);
        assert_eq!(t.dead_letter_count(), 0);
        assert_eq!(t.consumer_lag("indexer"), Some(0));

        handle.signal_drain();
        let final_t = handle.drain();
        assert_eq!(
            final_t.outbox_depth(),
            0,
            "graceful drain leaves outbox_depth == 0"
        );
    }

    #[test]
    fn serve_runs_lifecycle_and_returns_ok() {
        let spec = AppSpec::minimal("svc", Config::default(), OutboxSpec::default_inproc());
        assert_eq!(serve(spec), Ok(()), "serve boots → … → drains cleanly");
    }

    #[test]
    fn external_relay_producer_never_claims_shared_rows() {
        let outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let mut tx = outbox.begin(minter, ctx_base());
        tx.stage_state_change("owned by the elected cell relay");
        tx.emit(draft("issues.issue.created"), None).unwrap();
        tx.commit().unwrap();

        let spec = AppSpec::minimal(
            "producer",
            Config::default(),
            OutboxSpec::external_relay(outbox.clone()),
        );
        assert_eq!(serve(spec), Ok(()));
        assert_eq!(
            outbox.outbox_depth(),
            1,
            "a producer lifecycle must not claim or stamp the shared row"
        );
    }

    #[test]
    fn external_relay_mode_refuses_inert_consumers() {
        let outbox = OutboxStore::new();
        let (consumer, _) = hello_consumer(DedupLedger::new());
        let mut spec = AppSpec::minimal(
            "invalid-consumer",
            Config::default(),
            OutboxSpec::external_relay(outbox),
        );
        spec.consumers = vec![consumer];

        let error = match boot(spec) {
            Ok(_) => panic!("a consumer without a broker transport must fail boot"),
            Err(error) => error,
        };
        assert!(error.0.contains("without a consumer transport"));
    }

    #[test]
    fn external_intake_quarantines_and_terms_unmatched_delivery() {
        let event = event_for_transport();
        let probe = PullProbe::with_batches([vec![event]]);
        let quarantine = Arc::new(QuarantineProbe::default());
        let handle = boot(external_consumer_spec_with_quarantine(
            probe.clone(),
            vec![consumer_named("other", "myelin://acme/chat/", false)],
            quarantine.clone(),
        ))
        .unwrap();

        handle.tick();

        assert!(probe.state().acks.is_empty());
        assert_eq!(probe.state().terms, vec![token(1)]);
        assert_eq!(
            quarantine.records.lock().unwrap()[0].1,
            DeliveryQuarantineReason::NoRegisteredConsumer
        );
        assert!(handle.metrics_health().readiness().is_ready());
    }

    #[test]
    fn transport_poison_does_not_discard_a_valid_sibling() {
        let event = event_for_transport();
        let probe = PullProbe::default();
        probe.state().batches.push_back(vec![
            myelin_events::BrokerDelivery {
                token: token(1),
                broker_ref: Some(myelin_events::BrokerDeliveryRef {
                    stream: "TEST".into(),
                    stream_sequence: 1,
                }),
                body: BrokerDeliveryBody::Poison(
                    myelin_events::DeliveryPoisonKind::MalformedEnvelope,
                ),
                delivery_attempt: Some(1),
            },
            myelin_events::BrokerDelivery {
                token: token(2),
                broker_ref: Some(myelin_events::BrokerDeliveryRef {
                    stream: "TEST".into(),
                    stream_sequence: 2,
                }),
                body: BrokerDeliveryBody::Event(Box::new(event)),
                delivery_attempt: Some(1),
            },
        ]);
        let quarantine = Arc::new(QuarantineProbe::default());
        let (consumer, runs) = hello_consumer(DedupLedger::new());
        let handle = boot(external_consumer_spec_with_quarantine(
            probe.clone(),
            vec![consumer],
            quarantine.clone(),
        ))
        .unwrap();

        handle.tick();

        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(probe.state().terms, vec![token(1)]);
        assert_eq!(probe.state().acks, vec![token(2)]);
        assert_eq!(quarantine.records.lock().unwrap().len(), 1);
    }

    #[test]
    fn quarantine_failure_naks_only_poison_while_valid_sibling_acks() {
        let event = event_for_transport();
        let probe = PullProbe::default();
        probe.state().batches.push_back(vec![
            myelin_events::BrokerDelivery {
                token: token(1),
                broker_ref: Some(myelin_events::BrokerDeliveryRef {
                    stream: "TEST".into(),
                    stream_sequence: 1,
                }),
                body: BrokerDeliveryBody::Poison(
                    myelin_events::DeliveryPoisonKind::SubjectMismatch,
                ),
                delivery_attempt: Some(1),
            },
            myelin_events::BrokerDelivery {
                token: token(2),
                broker_ref: Some(myelin_events::BrokerDeliveryRef {
                    stream: "TEST".into(),
                    stream_sequence: 2,
                }),
                body: BrokerDeliveryBody::Event(Box::new(event)),
                delivery_attempt: Some(1),
            },
        ]);
        let quarantine = Arc::new(QuarantineProbe::default());
        quarantine.fail_next.store(true, Ordering::SeqCst);
        let (consumer, runs) = hello_consumer(DedupLedger::new());
        let handle = boot(external_consumer_spec_with_quarantine(
            probe.clone(),
            vec![consumer],
            quarantine,
        ))
        .unwrap();

        handle.tick();

        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(probe.state().retries, vec![token(1)]);
        assert_eq!(probe.state().acks, vec![token(2)]);
        assert!(probe.state().terms.is_empty());
        assert!(
            !handle.metrics_health().readiness().is_ready(),
            "OLTP failure is unhealthy"
        );
    }

    #[test]
    fn transient_metadata_fault_is_nak_only_and_never_quarantined() {
        let probe = PullProbe::default();
        probe
            .state()
            .batches
            .push_back(vec![myelin_events::BrokerDelivery {
                token: token(9),
                broker_ref: None,
                body: BrokerDeliveryBody::TransientMetadataFault,
                delivery_attempt: None,
            }]);
        let quarantine = Arc::new(QuarantineProbe::default());
        let handle = boot(external_consumer_spec_with_quarantine(
            probe.clone(),
            Vec::new(),
            quarantine.clone(),
        ))
        .unwrap();

        handle.tick();

        assert_eq!(probe.state().retries, vec![token(9)]);
        assert!(probe.state().terms.is_empty());
        assert!(quarantine.records.lock().unwrap().is_empty());
    }

    #[test]
    fn external_intake_acks_only_after_every_matching_consumer_is_terminal() {
        let event = event_for_transport();
        let probe = PullProbe::with_batches([vec![event.clone()], vec![event]]);
        let handle = boot(external_consumer_spec(
            probe.clone(),
            vec![
                consumer_named("first", "myelin://acme/issues/", false),
                consumer_named("second", "myelin://acme/issues/", true),
            ],
        ))
        .unwrap();

        handle.tick();
        assert!(
            probe.state().acks.is_empty(),
            "one Retry gates the broker ack"
        );
        assert_eq!(probe.state().retries, vec![token(1)]);

        handle.tick();
        assert_eq!(probe.state().acks, vec![token(2)]);
    }

    #[test]
    fn external_ack_failure_flips_broker_readiness_down() {
        let probe = PullProbe::with_batches([vec![event_for_transport()]]);
        probe.state().fail_ack = true;
        let handle = boot(external_consumer_spec(
            probe,
            vec![consumer_named("indexer", "myelin://acme/issues/", false)],
        ))
        .unwrap();

        handle.tick();

        assert!(!handle.metrics_health().readiness().is_ready());
    }

    #[test]
    fn exhausted_retry_persists_dlq_then_terms_without_dedup_and_replay_can_execute() {
        let event = event_for_transport();
        let event_id = event.event_id.clone();
        let probe = PullProbe::with_delivery(event.clone(), MAX_CONSUMER_DELIVERIES);
        let dlq = DlqProbe::default();
        let dedup = DedupLedger::new();
        let (consumer, calls) = always_retry_consumer(dedup.clone(), dlq.clone());
        let handle = boot(external_consumer_spec(probe.clone(), vec![consumer])).unwrap();

        handle.tick();

        let records = dlq
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].event_id, event_id);
        assert_eq!(probe.state().terms, vec![token(1)]);
        assert!(probe.state().acks.is_empty());
        assert_eq!(dedup.len(), 0, "retry exhaustion writes no dedup tombstone");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        struct Repaired;
        impl EventHandler for Repaired {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(
                &self,
                _event: &EventEnvelope,
                _tx: &mut myelin_events::HandlerTx<'_>,
            ) -> HandleOutcome {
                HandleOutcome::Done
            }
        }
        let repaired = Consumer::new(
            Repaired,
            Subscription::bind(
                ConsumerName("retrying".into()),
                &["myelin://acme/issues/"],
                PrefetchBound::DEFAULT,
            )
            .unwrap(),
            dedup.clone(),
        );
        assert_eq!(
            repaired.deliver(&Message {
                subject: event.subject.0.clone(),
                envelope: event,
            }),
            Delivered::Acked,
            "operator replay executes once after the backend is repaired"
        );
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn unavailable_dependency_bypasses_retry_exhaustion_and_never_dlqs_or_terms() {
        let probe = PullProbe::with_delivery(event_for_transport(), MAX_CONSUMER_DELIVERIES + 1);
        let dlq = DlqProbe::default();
        let dedup = DedupLedger::new();
        let consumer = dependency_unavailable_consumer(dedup.clone(), dlq.clone());
        let handle = boot(external_consumer_spec(probe.clone(), vec![consumer])).unwrap();

        handle.tick();

        assert_eq!(probe.state().retries, vec![token(1)]);
        assert!(probe.state().terms.is_empty());
        assert!(probe.state().acks.is_empty());
        assert!(dlq
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
        assert!(
            dedup.is_empty(),
            "dependency failure commits no dedup effect"
        );
    }

    #[test]
    fn exhausted_retry_naks_when_durable_dlq_write_fails() {
        let event = event_for_transport();
        let probe = PullProbe::with_delivery(event.clone(), MAX_CONSUMER_DELIVERIES);
        probe
            .state()
            .batches
            .push_back(vec![myelin_events::BrokerDelivery {
                token: token(2),
                broker_ref: Some(myelin_events::BrokerDeliveryRef {
                    stream: "TEST".into(),
                    stream_sequence: 2,
                }),
                body: BrokerDeliveryBody::Event(Box::new(event)),
                delivery_attempt: Some(MAX_CONSUMER_DELIVERIES + 1),
            }]);
        let dlq = DlqProbe::default();
        dlq.fail_next.store(1, Ordering::SeqCst);
        let (consumer, calls) = always_retry_consumer(DedupLedger::new(), dlq);
        let handle = boot(external_consumer_spec(probe.clone(), vec![consumer])).unwrap();

        handle.tick();

        assert!(probe.state().terms.is_empty());
        assert_eq!(probe.state().retries, vec![token(1)]);
        assert_eq!(handle.telemetry().consumer_lag("retrying"), Some(1));

        handle.tick();

        assert_eq!(probe.state().terms, vec![token(2)]);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "failed quarantine reruns rolled-back work rather than discarding dependency-deferred work"
        );
        assert_eq!(
            handle.telemetry().consumer_lag("retrying"),
            Some(0),
            "the successful quarantine clears the one pending entry without phantom lag"
        );
    }

    #[test]
    fn signalled_external_drain_never_makes_a_fresh_pull() {
        let probe = PullProbe::with_batches([vec![event_for_transport()]]);
        let handle = boot(external_consumer_spec(
            probe.clone(),
            vec![consumer_named("indexer", "myelin://acme/issues/", false)],
        ))
        .unwrap();

        handle.signal_drain();
        handle.drain();

        assert_eq!(probe.state().pulls, 0);
        assert!(probe.state().acks.is_empty());
    }

    #[test]
    fn failed_boot_returns_non_zero() {
        let spec = AppSpec::minimal(
            "svc",
            Config("BAD_POOL".into()),
            OutboxSpec::default_inproc(),
        );
        let r = serve(spec);
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the boot error names the §3.2 fail-fast config validation"
        );
    }

    #[test]
    fn config_validation_fails_fast_on_bad_env() {
        let spec = AppSpec::minimal(
            "svc",
            Config("BAD_POOL".into()),
            OutboxSpec::default_inproc(),
        );
        let r = boot(spec);
        assert!(r.is_err(), "boot fails fast on a bad config");
    }

    #[test]
    fn graceful_drain_finishes_in_flight_before_exit() {
        let outbox = OutboxStore::new();
        let (consumer, runs) = hello_consumer(DedupLedger::new());
        let spec = AppSpec {
            name: "drainer",
            config: Config::default(),
            migrations: Migrations::default(),
            hot_tables: HotTables::none(),
            public: PublicRoutes::default(),
            internal: InternalRpc::default(),
            consumers: vec![consumer],
            holders: AppSpec::auto(),
            stores: StoreManifest::new(),
            outbox: OutboxSpec::new(outbox.clone(), InProcessBus::new()),
            critical: CriticalDependencies::default(),
        };
        let handle = boot(spec).expect("boot");

        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let mut tx = outbox.begin(minter, ctx_base());
        tx.stage_state_change("two");
        tx.emit(draft("issues.issue.created"), None).unwrap();
        tx.emit(draft("issues.issue.updated"), None).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            handle.outbox().outbox_depth(),
            2,
            "two in-flight events at drain time"
        );

        handle.signal_drain();
        assert!(handle.is_draining(), "intake is stopped");
        let final_t = handle.drain();
        assert_eq!(
            final_t.outbox_depth(),
            0,
            "drain finished the in-flight events"
        );
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2,
            "both in-flight events were delivered before exit"
        );
    }

    #[test]
    fn lifecycle_dedups_redelivery() {
        let outbox = OutboxStore::new();
        let (consumer, runs) = hello_consumer(DedupLedger::new());
        let spec = AppSpec {
            name: "dedup",
            config: Config::default(),
            migrations: Migrations::default(),
            hot_tables: HotTables::none(),
            public: PublicRoutes::default(),
            internal: InternalRpc::default(),
            consumers: vec![consumer],
            holders: AppSpec::auto(),
            stores: StoreManifest::new(),
            outbox: OutboxSpec::new(outbox.clone(), InProcessBus::new()),
            critical: CriticalDependencies::default(),
        };
        let handle = boot(spec).expect("boot");

        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let mut tx = outbox.begin(minter, ctx_base());
        tx.stage_state_change("one");
        tx.emit(draft("issues.issue.created"), None).unwrap();
        tx.commit().unwrap();

        handle.tick();
        handle.tick();
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the redelivery was deduped (handler ran once)"
        );
    }

    #[test]
    fn destructive_migration_is_rejected_at_boot() {
        let spec = AppSpec {
            name: "bad",
            config: Config::default(),
            migrations: Migrations::new([("0010_bad", "DROP TABLE hello")]),
            hot_tables: HotTables::none(),
            public: PublicRoutes::default(),
            internal: InternalRpc::default(),
            consumers: vec![],
            holders: AppSpec::auto(),
            stores: StoreManifest::new(),
            outbox: OutboxSpec::default(),
            critical: CriticalDependencies::default(),
        };
        match boot(spec) {
            Err(e) => assert!(
                e.0.contains("forward-only"),
                "the error names the forward-only rule"
            ),
            Ok(_) => panic!("a destructive migration must fail boot"),
        }
    }

    #[test]
    fn migration_runner_applies_outbox_dedup_then_service_migrations_in_order() {
        let mut runner = MigrationRunner::new();
        let migrations = Migrations::of([
            Migration::plain("0000_outbox", myelin_events::OUTBOX_MIGRATION),
            Migration::plain(
                "0001_consumer_dedup",
                myelin_events::CONSUMER_DEDUP_MIGRATION,
            ),
            Migration::plain("0010_svc", "CREATE TABLE IF NOT EXISTS svc (id TEXT)"),
        ]);
        runner.run(&migrations, &HotTables::none()).unwrap();
        assert_eq!(
            runner.applied(),
            &["0000_outbox", "0001_consumer_dedup", "0010_svc"]
        );
    }

    #[test]
    fn lifecycle_dead_letters_poison_and_continues() {
        let outbox = OutboxStore::new();
        let runs = Arc::new(AtomicU32::new(0));
        struct Poison {
            runs: Arc<AtomicU32>,
        }
        impl EventHandler for Poison {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(
                &self,
                _ev: &EventEnvelope,
                _tx: &mut myelin_events::HandlerTx<'_>,
            ) -> HandleOutcome {
                self.runs.fetch_add(1, Ordering::SeqCst);
                HandleOutcome::NonRetryable(Reason("poison".into()))
            }
        }
        let sub = Subscription::bind(
            ConsumerName("indexer".into()),
            &["myelin://acme/issues/"],
            PrefetchBound::DEFAULT,
        )
        .unwrap();
        let consumer = Consumer::new(Poison { runs: runs.clone() }, sub, DedupLedger::new());
        let spec = AppSpec {
            name: "poison",
            config: Config::default(),
            migrations: Migrations::default(),
            hot_tables: HotTables::none(),
            public: PublicRoutes::default(),
            internal: InternalRpc::default(),
            consumers: vec![ConsumerReg::new(consumer)],
            holders: AppSpec::auto(),
            stores: StoreManifest::new(),
            outbox: OutboxSpec::new(outbox.clone(), InProcessBus::new()),
            critical: CriticalDependencies::default(),
        };
        let handle = boot(spec).expect("boot");

        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let mut tx = outbox.begin(minter, ctx_base());
        tx.stage_state_change("poison");
        tx.emit(draft("issues.issue.created"), None).unwrap();
        tx.commit().unwrap();

        handle.tick();
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the handler poisoned exactly once"
        );
        let final_t = handle.drain();
        assert_eq!(
            final_t.outbox_depth(),
            0,
            "the outbox drained even though the consumer poisoned"
        );
    }
}
