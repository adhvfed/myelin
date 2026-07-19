//! # `serve(AppSpec)` — the boot → migrate → relay → consumers → ports → drain lifecycle (P-S12)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §3.1 (the one call — `serve(AppSpec{...})`; boot → migrate → start outbox relay → start
//! consumers → open the three ports → serve until signalled → **graceful drain**; non-zero on
//! failed boot), §3.2 (config env-first, validated at boot, fail fast), §3.3 (the bounded DB
//! pool + the auto-started outbox relay), §3.4 (PersonalDataHolder auto-registration), §3.5
//! (telemetry init + the contract-1.8 signal set on the metrics-health port), §3.6 (what the
//! harness deliberately does NOT do).
//!
//! **Contract-index:** row 1.1 (`serve(AppSpec)`) — OWNED here.
//! **P-S12 → global P-010.** DEPENDS-ON P-S07 (the relay), P-S08 (the consumer runtime),
//! P-S04 (the telemetry signal set this exports the producer side of).
//!
//! ## What this module ships (the lifecycle the harness owns)
//! [`serve`] takes an [`AppSpec`] and runs the lifecycle to completion, returning non-zero
//! (an `Err`) on a failed boot. The phases, in order (architecture §3.1):
//!   1. **boot** — validate the [`Config`] (fail fast, §3.2), open the bounded OLTP pool
//!      ([`myelin_storage::OltpPool`], §3.3).
//!   2. **migrate** — run the forward-only [`Migrations`] (the runner is **P-S15**; here
//!      `serve` calls the [`MigrationRunner`] seam that applies them in order).
//!   3. **relay** — start the outbox [`myelin_events::Relay`] over the [`OutboxSpec`] store
//!      (§3.3, BUS-2 — the relay is the only thing on the publish side; auto-started).
//!   4. **consumers** — start the registered [`ConsumerReg`]s (the idempotent
//!      [`myelin_events::Consumer`] runtime, P-S08).
//!   5. **holders** — auto-register every opened store as a `PersonalDataHolder` (§3.4, GD-3;
//!      the exhaustive H1–H18 confirmation is **P-S15/P-S27** — here the OLTP store registers).
//!   6. **ports** — open the three surfaces (public / internal / metrics-health, §4). The
//!      tenant-from-token public surface (SUB-D7) is **P-S13** and liveness ≠ readiness on the
//!      metrics-health surface (SUB-D9) is **P-S14**; here `serve` calls the [`PortOpener`] seam
//!      and installs the metrics-health producer that exports the contract-1.8 signal set (§3.5).
//!   7. **serve** — drive the [`ServeHandle`] until a drain is signalled.
//!   8. **graceful drain** — stop intake, finish in-flight (drain the relay to depth 0,
//!      let the consumers ack what they hold), ack-then-exit.
//!
//! ## The producer side of the contract-1.8 signal set (§3.5)
//! `serve` installs a [`Telemetry`] meter that, at every observation, exports the live
//! survival signals — `outbox_depth`, `dead_letter_count`, `consumer_lag` (per consumer) —
//! by the SAME names the harness's telemetry-assertion library (P-S04) reads (the contract-1.8
//! set, architecture §10.2). The harness's `SignalSource` is populated from this producer in
//! the hello-world test; the assertion surface does not change. **Floor:** the OpenTelemetry
//! tracer/meter/logger + the causality+tenant trace-context middleware (§3.5) is named below
//! and lands with the metrics-health surface (P-S13/P-S14); here the producer is a typed
//! in-process meter exporting the same names, so the signal NAMES are exercised end-to-end now.
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The forward-only migration RUNNER is P-S15.** Here [`MigrationRunner`] applies the
//!   embedded DDL list in order and records what it applied (the expand→backfill→contract
//!   online runner + the holder auto-registration over every opened store is P-S15). The
//!   `forward-only-migration` lint (P-S11) reads the DDL shape; a destructive `DROP` migration
//!   is rejected here loudly (named, not silently admitted).
//! - **The three-surface topology + tenant-from-token (SUB-D7) is P-S13;** liveness ≠ readiness
//!   on the metrics-health surface (SUB-D9) is **P-S14**. Here [`PortOpener`] opens three named
//!   surfaces and the metrics-health one exports the signal set; the tenant-from-token rejection
//!   and the readiness/liveness split are those prompts. The `ports` field carries the surface
//!   set so the seam is wired now.
//! - **The real OpenTelemetry export + the OS-signal drain trigger is P-S13/P-S14.** Here the
//!   drain is triggered by [`ServeHandle::signal_drain`] (a deterministic, testable trigger);
//!   the `SIGTERM`/`SIGINT` → drain wiring lands with the real ports. Named, not assumed done.
//! - **The DB pool is the bounded-permit MODEL ([`myelin_storage::OltpPool`]).** The concrete
//!   `tokio-postgres`/`sqlx` connection lands with the driver (P-S15); the bounded-pool +
//!   fast-fail-on-saturation semantics are complete now (P-007). The relay/consumer stores are
//!   the in-memory models of the SQL `outbox` / `consumer_dedup` tables (P-S07/P-S08); the real
//!   binding inside the caller's DB transaction lands when the driver does.

use crate::holder_registered::{assert_all_holders_registered, HolderViolation, StoreManifest};
use crate::holders::{HolderRegistration, HolderRegistry, StoreKind};
use crate::metrics_health::{CriticalDependencies, HealthTable, MetricsHealthSurface};
use crate::migrations::{HotTables, Migration, MigrationRunner, Migrations};
use crate::topology::PublicSurface;
use crate::{Config, ServeError};
use myelin_events::relay::{BusTransport, EventConsumer};
// Used only by the `test-support`-gated in-process floor (`OutboxSpec::default_inproc`).
#[cfg(any(test, feature = "test-support"))]
use myelin_events::relay::InProcessBus;
use myelin_events::{
    BrokerDeliveryBody, Consumer, ConsumerName, Delivered, DeliveryQuarantineReason,
    DeliveryToken, DurableDeliveryQuarantine, EventHandler, Message, OutboxStore, Relay, Timestamp,
};
use myelin_storage::{OltpConfig, OltpPool, OltpStoreHolder};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A service's durable outbox plus an optional embedded relay transport.
///
/// Production services that share one PostgreSQL `outbox` table use [`Self::external_relay`]:
/// they commit rows but never claim or mark them locally. One separately elected cell publisher
/// owns that global drain. Tests and deliberately self-contained deployments may use [`Self::new`]
/// to keep the embedded relay/consumer loop.
pub struct OutboxSpec {
    store: OutboxStore,
    transport: Option<Box<dyn BusTransport>>,
    consumer_transport: Option<Box<dyn EventConsumer>>,
    delivery_quarantine: Option<Arc<dyn DurableDeliveryQuarantine>>,
}

impl OutboxSpec {
    /// **The in-process floor: a fresh empty MEMORY store + the in-process broker fake — TEST /
    /// DEV ONLY (MR-009b W3b.4).** Gated behind `test-support` so no production composition root
    /// can silently pick up the per-process in-memory outbox (SI-007: events lost on restart).
    /// Production supplies a DURABLE-backed store via [`OutboxSpec::durable`]. Since MR-009b
    /// W3b.6 the memory floor cannot be constructed from production code AT ALL:
    /// `OutboxStore::new()` itself is `test-support`-gated, so the former "explicit debt site"
    /// escape hatch is closed at compile time (the W3b.4 debt is discharged).
    #[cfg(any(test, feature = "test-support"))]
    pub fn default_inproc() -> OutboxSpec {
        OutboxSpec {
            store: OutboxStore::new(),
            transport: Some(Box::new(InProcessBus::new())),
            consumer_transport: None,
            delivery_quarantine: None,
        }
    }

    /// Build the relay spec from a service's own [`OutboxStore`] + a chosen [`BusTransport`].
    pub fn new(store: OutboxStore, transport: impl BusTransport + 'static) -> OutboxSpec {
        OutboxSpec {
            store,
            transport: Some(Box::new(transport)),
            consumer_transport: None,
            delivery_quarantine: None,
        }
    }

    /// **The production spec (MR-009b W3b.4): a DURABLE-backed store + the chosen transport.**
    /// The caller constructs the store durable-first —
    /// `OutboxStore::durable(Arc::new(PgOutboxBacking::new(pool, handle)))` over the MR-022
    /// `SubstrateProvider` pool (with the provider's `migrate_foundation` applied so the frozen
    /// `outbox` table exists) — and the relay the lifecycle starts then drains PG-committed rows
    /// through `PgRelay`'s claim/mark/dead-letter discipline to `transport`. Events survive a
    /// process restart; the in-process memory floor ([`OutboxSpec::default_inproc`]) is the
    /// test/dev double. `InProcessBus` remains a legitimate default TRANSPORT (the broker hop is
    /// in-process; durability lives in the store) — the NATS-by-default transport is the
    /// EventsRuntime/integration track, out of this wave's scope by design.
    pub fn durable(store: OutboxStore, transport: Box<dyn BusTransport>) -> OutboxSpec {
        OutboxSpec {
            store,
            transport: Some(transport),
            consumer_transport: None,
            delivery_quarantine: None,
        }
    }

    /// Bind a producer to the shared durable outbox without starting a process-local relay.
    ///
    /// This is the production-safe posture when multiple services share the same table: a local
    /// relay could claim another service's row, publish it into a private process bus, and stamp it
    /// sent. Consumers cannot be registered on this spec because no consumer transport is present;
    /// boot fails loudly instead of presenting a healthy but inert consumer.
    pub fn external_relay(store: OutboxStore) -> OutboxSpec {
        OutboxSpec {
            store,
            transport: None,
            consumer_transport: None,
            delivery_quarantine: None,
        }
    }

    /// Bind a production service to an externally elected outbox relay and a separate durable
    /// pull consumer. This process can receive and explicitly acknowledge events, but it owns no
    /// relay and therefore cannot claim or mark any row in the shared outbox table.
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

    /// The store the relay drains (so a test/handler can emit into it).
    pub fn store(&self) -> &OutboxStore {
        &self.store
    }
}

/// The default spec is the in-process floor — TEST / DEV ONLY (gated with
/// [`OutboxSpec::default_inproc`], MR-009b W3b.4): production roots must choose
/// [`OutboxSpec::durable`] explicitly; there is no silent in-memory default to fall into.
#[cfg(any(test, feature = "test-support"))]
impl Default for OutboxSpec {
    fn default() -> Self {
        OutboxSpec::default_inproc()
    }
}

/// The object-safe view of a registered consumer the lifecycle drives (so a heterogeneous set
/// of [`Consumer<H>`] — each over a different `EventHandler` body — lives in one
/// `Vec<ConsumerReg>`). It delivers a [`Message`] through the seven-rule runtime (P-S08) and
/// reports the consumer's name + lag (the contract-1.8 `consumer_lag` signal, rule 7).
trait RunnableConsumer: Send + Sync {
    fn name(&self) -> ConsumerName;
    fn accepts(&self, subject: &str) -> bool;
    fn is_handled(&self, event_id: &myelin_events::EventId) -> bool;
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
    fn is_handled(&self, event_id: &myelin_events::EventId) -> bool {
        Consumer::is_handled(self, event_id)
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

/// A registered event consumer (architecture §5; contract 2.4) the harness wires. Wraps the
/// idempotent [`Consumer`] runtime (P-S08) behind an object-safe handle so consumers over
/// different handler bodies share one registration list.
pub struct ConsumerReg {
    inner: Arc<dyn RunnableConsumer>,
}

impl ConsumerReg {
    /// Register a [`Consumer<H>`] (the seven-rule runtime over a service's `EventHandler`).
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

    fn is_handled(&self, event_id: &myelin_events::EventId) -> bool {
        self.inner.is_handled(event_id)
    }

    fn lag(&self) -> u64 {
        self.inner.lag()
    }
}

/// The public surface route set (architecture §4.1; contract 1.2). The gateway-fronted,
/// tenant-from-token topology (SUB-D7) is implemented in [`crate::topology::PublicSurface`]
/// (P-S13); this is the per-service route-registration carrier `serve` opens it over. Opaque on
/// this floor (a service registers no routes yet); the live tenant-from-token surface the
/// lifecycle opens is on [`ServeHandle::public_surface`].
#[derive(Clone, Debug, Default)]
pub struct PublicRoutes(pub ());

/// The internal RPC surface registration (architecture §4.2; contract 1.2). The trust-boundary,
/// re-authorize-every-call surface is implemented in [`crate::topology::InternalSurface`] (P-S13);
/// this is the per-service carrier `serve` opens it over. Opaque on this floor.
#[derive(Clone, Debug, Default)]
pub struct InternalRpc(pub ());

/// How the harness registers `PersonalDataHolder`s (architecture §3.4; contract 1.4). `Auto`
/// means every store the harness opens is auto-registered (GD-3) — the only variant by design
/// (a service cannot opt out of holder registration; the exhaustive H1–H18 confirmation is
/// **P-S27**).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HoldersSpec {
    /// every opened store auto-registered (the §3.1 `AppSpec::auto`).
    #[default]
    Auto,
}

/// The three surfaces a service opens (architecture §4; contract 1.2). Named here so the
/// `PortOpener` seam is wired; the security boundary (public↔internal), tenant-from-token
/// (SUB-D7, P-S13), and liveness≠readiness (SUB-D9, P-S14) are those prompts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Gateway-fronted, identity-injected (tenant from token).
    Public,
    /// Inside the trust boundary; re-authorizes every call.
    Internal,
    /// Liveness + readiness + the contract-1.8 metrics export.
    MetricsHealth,
}

/// The port-opening seam `serve` calls. Opens the three surfaces (§4) and OWNS the live
/// tenant-from-token [`PublicSurface`] (P-S13) — the security boundary, not just a recorded enum.
/// Records which surfaces were opened so a test can assert the topology was opened in the
/// lifecycle. The metrics-health surface is where the producer side of the contract-1.8 signal
/// set is exported (§3.5). **Floor:** the real listeners + the OS-signal event loop land with the
/// real transport (named in [`ServeHandle::signal_drain`]); the tenant-from-token + re-authorize
/// SECURITY mechanism is complete now ([`crate::topology`]).
pub struct PortOpener {
    opened: Vec<Surface>,
    /// The live public surface the lifecycle opens — tenant-from-token + IDOR reject/audit (P-S13).
    public: PublicSurface,
    /// The live metrics-health surface the lifecycle opens — liveness ≠ readiness (P-S14, SUB-D9).
    /// Opened in the `Booting` startup state (not-ready-not-killed); `serve` marks it started at
    /// the end of a successful boot. The `DependencyHealth` probe is a [`HealthTable`] on this
    /// floor (the resilient client's breaker state feeds it in production, §6 P-S16).
    metrics_health: MetricsHealthSurface<HealthTable>,
    /// The shared probe handle behind the metrics-health surface, so the (future) resilient
    /// client / a drill can mark a critical dependency down and watch readiness flip.
    health: HealthTable,
}

impl Default for PortOpener {
    fn default() -> Self {
        let health = HealthTable::new();
        PortOpener {
            opened: Vec::new(),
            public: PublicSurface::default(),
            // No critical deps declared on the bare default (a service declares its set at boot,
            // §4.3); `open_all` rebuilds with the service's declared critical set.
            metrics_health: MetricsHealthSurface::new(
                CriticalDependencies::default(),
                health.clone(),
            ),
            health,
        }
    }
}

impl PortOpener {
    /// Open the three surfaces (public / internal / metrics-health, §4) over the service's
    /// declared critical-dependency set. Constructs the live tenant-from-token [`PublicSurface`]
    /// (the public↔internal security boundary, P-S13) and the live liveness ≠ readiness
    /// [`MetricsHealthSurface`] (P-S14, opened `Booting` = not-ready-not-killed; the lifecycle
    /// marks it started after a successful boot). The real listeners + the standard middleware
    /// stack land with the real transport (P-S14+).
    pub fn open_all(&mut self, critical: CriticalDependencies) {
        self.public = PublicSurface::default();
        self.health = HealthTable::new();
        self.metrics_health = MetricsHealthSurface::new(critical, self.health.clone());
        self.opened = vec![Surface::Public, Surface::Internal, Surface::MetricsHealth];
    }

    /// The surfaces opened (for the lifecycle assertion).
    pub fn opened(&self) -> &[Surface] {
        &self.opened
    }

    /// The live tenant-from-token public surface opened in the lifecycle (P-S13). The gateway
    /// feeds requests through [`PublicSurface::resolve_tenant`]; a path≠token mismatch is rejected
    /// + audited as an IDOR, and `misroute_count` stays 0.
    pub fn public_surface(&self) -> &PublicSurface {
        &self.public
    }

    /// The live liveness ≠ readiness metrics-health surface opened in the lifecycle (P-S14,
    /// SUB-D9). Liveness = "not wedged" (never checks a dependency); readiness = "can serve
    /// correct traffic now" (a dead critical dependency → not-ready + shed); startup =
    /// not-ready-not-killed until [`Self::mark_metrics_health_started`].
    pub fn metrics_health(&self) -> &MetricsHealthSurface<HealthTable> {
        &self.metrics_health
    }

    /// The shared dependency-health probe behind the metrics-health surface (so the resilient
    /// client / a drill can mark a critical dependency down and watch readiness flip).
    pub fn health_probe(&self) -> &HealthTable {
        &self.health
    }

    /// Flip the metrics-health surface's startup gate to complete (boot + migrations succeeded).
    pub fn mark_metrics_health_started(&self) {
        self.metrics_health.mark_started();
    }
}

/// The producer side of the contract-1.8 telemetry signal set (architecture §3.5 / §10.2). A
/// typed in-process meter `serve` installs that exports the live survival signals by the SAME
/// names the harness's telemetry-assertion library (P-S04) reads. **Floor:** the OpenTelemetry
/// meter + the OTLP export on the metrics-health port is **P-S13/P-S14**; here the meter exports
/// the same signal names in-process so the producer→consumer signal contract is exercised now.
///
/// Scalar signals are stored by name; the per-consumer `consumer_lag` is labelled by consumer.
#[derive(Clone, Default)]
pub struct Telemetry {
    inner: Arc<Mutex<TelemetryInner>>,
}

#[derive(Default)]
struct TelemetryInner {
    /// `outbox_depth` (count of unsent rows) — exported each observation.
    outbox_depth: i64,
    /// `dead_letter_count` (rows the relay gave up on) — exported each observation.
    dead_letter_count: i64,
    /// `consumer_lag` per consumer name (rule 7; `num_pending` un-acked backlog).
    consumer_lag: BTreeMap<String, i64>,
}

impl Telemetry {
    /// A fresh meter (every signal at its initial zero — observability is part of the pass,
    /// so the producer starts emitting the moment `serve` installs it).
    pub fn new() -> Telemetry {
        Telemetry::default()
    }

    /// Observe the live survival signals off the relay store + consumers (the producer side of
    /// the §10.2 set). Called after each drain pass / on demand so the metrics reflect truth.
    fn observe(&self, outbox: &OutboxStore, consumers: &[ConsumerReg]) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.outbox_depth = outbox.outbox_depth() as i64;
        inner.dead_letter_count = outbox.dead_letter_count() as i64;
        inner.consumer_lag.clear();
        for c in consumers {
            inner.consumer_lag.insert(c.name().0, c.lag() as i64);
        }
    }

    /// The exported `outbox_depth` signal value (contract 1.8, §10.2 row 4).
    pub fn outbox_depth(&self) -> i64 {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outbox_depth
    }

    /// The exported `dead_letter_count` signal value (contract 1.8, §10.2 row 4).
    pub fn dead_letter_count(&self) -> i64 {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .dead_letter_count
    }

    /// The exported `consumer_lag` signal value for a consumer (contract 1.8, §10.2 row 3),
    /// or `None` if that consumer is not registered.
    pub fn consumer_lag(&self, consumer: &str) -> Option<i64> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .consumer_lag
            .get(consumer)
            .copied()
    }
}

/// The one spec a service's `main.rs` supplies (architecture §3.1; contract 1.1). The harness
/// owns the lifecycle around it: boot → migrate → relay → consumers → three ports → graceful
/// drain. The eight fields are the frozen 1.1 shape (`name, config, migrations, public,
/// internal, consumers, holders, outbox`).
pub struct AppSpec {
    /// The service name (a PII-free label, the telemetry/trace service identifier).
    pub name: &'static str,
    /// The validated, env-first config (§3.2; validated at boot, fail fast).
    pub config: Config,
    /// The forward-only embedded migration set (§9; run at boot).
    pub migrations: Migrations,
    /// The per-subsystem hot-table declaration (§9.4; contract 1.5; C-3). A table is flagged hot
    /// when its write rate warrants expand→backfill→contract (measured, not predicted). The
    /// migration runner refuses a blocking `ALTER` on a declared-hot table at boot, and the
    /// `forward-only-migration` lint (P-S11) reads the SAME declaration at source-scan. Defaults
    /// to none ([`HotTables::none`]); a high-write subsystem declares its set here (M1+).
    pub hot_tables: HotTables,
    /// The public surface (§4.1 — behind the gateway, identity-injected; SUB-D7 is P-S13).
    pub public: PublicRoutes,
    /// The internal RPC surface (§4.2 — inside the trust boundary only).
    pub internal: InternalRpc,
    /// The durable, idempotent, whitelisted consumers (§5).
    pub consumers: Vec<ConsumerReg>,
    /// Holder registration policy (§3.4 — every opened store auto-registered, GD-3).
    pub holders: HoldersSpec,
    /// The set of stores the service declares it owns beyond the implicit OLTP store (§3.4;
    /// contract 1.4). The harness opens (and therefore auto-registers) **every** declared store
    /// through the one door, so the `holder-registered` architecture test (P-GA-04) is green by
    /// construction for a harness-opened service. A store a service constructs OUTSIDE this
    /// manifest/boot path never registers — that is the violation the architecture test catches.
    /// Defaults to the implicit OLTP store only ([`StoreManifest::new`]); a service that owns a
    /// blob prefix / cache namespace / search index declares them here.
    pub stores: StoreManifest,
    /// The outbox relay spec (§3.3 — relay started automatically, BUS-2).
    pub outbox: OutboxSpec,
    /// The critical-dependency set the metrics-health surface's readiness probe reads (§4.3,
    /// SUB-D9). A dead **critical** dependency reports not-ready + sheds; a non-critical one does
    /// not flip readiness. Declared at boot (P-S14). The OLTP store is implicitly critical (a
    /// service cannot serve correct traffic without its own DB); a service adds its other critical
    /// downstreams (e.g. `identity`) here.
    pub critical: CriticalDependencies,
}

impl AppSpec {
    /// The `AppSpec::auto` holder policy the §3.1 verbatim call names.
    pub const fn auto() -> HoldersSpec {
        HoldersSpec::Auto
    }

    /// A minimal spec for a service `name` with a validated `config` (the hello-world shape):
    /// no migrations, no consumers, the CALLER-INJECTED outbox. Builders add migrations/consumers.
    ///
    /// **W3b.6 debt DISCHARGED (MR-009b W3b.6):** `minimal` no longer constructs the in-memory
    /// floor — the [`OutboxSpec`] is INJECTED, so a production root that builds on `minimal`
    /// (control-plane) chooses [`OutboxSpec::durable`] explicitly, and a test passes the
    /// `test-support`-gated [`OutboxSpec::default_inproc`]. There is no silent in-memory default
    /// left to fall into (the gated `OutboxStore::new` breaks a production reach LOUDLY at
    /// compile time).
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

/// The bounded-OLTP-pool config the harness validates at boot from the [`Config`] (§3.2/§3.3).
/// **Floor:** the env parse of the real `DATABASE_URL` + pool knobs lands with the driver
/// (P-S15); here `serve` validates a bounded default so the fail-fast-on-a-bad-pool property is
/// exercised. A `max_pool_size` of 0 (unbounded/zero) fails boot loudly (never start unbounded).
fn oltp_config_from(config: &Config) -> Result<OltpConfig, ServeError> {
    // The Config is opaque on this floor (P-001/§3.2 names the env-first parse as P-S15). A
    // service name'd config of "BAD_POOL" models the boot-time validation failure path so the
    // fail-fast property is testable; everything else gets the bounded default.
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

/// The live, booted service the lifecycle drives (the post-boot/pre-drain state). Exposed so a
/// test / a drill can drive one tick of the lifecycle and inspect the producer signals before
/// signalling the graceful drain. `serve` builds one, opens the ports, drives it to a drain
/// signal, then drains it.
pub struct ServeHandle {
    name: &'static str,
    pool: OltpPool,
    outbox: OutboxStore,
    relay: Option<Relay<RelayTransport>>,
    consumer_transport: Option<Box<dyn EventConsumer>>,
    delivery_quarantine: Option<Arc<dyn DurableDeliveryQuarantine>>,
    consumers: Vec<ConsumerReg>,
    holders: HolderRegistry,
    /// The full store manifest (implicit OLTP + the service's declared stores) the
    /// `holder-registered` architecture test joins against [`Self::holder_registry`].
    manifest: StoreManifest,
    ports: PortOpener,
    telemetry: Telemetry,
    /// Set once a drain has been requested (the graceful-drain trigger). After this, intake is
    /// stopped; the lifecycle finishes in-flight work and exits.
    draining: Arc<AtomicBool>,
}

/// The relay's transport on this floor is the in-process broker behind a `Box` (so `serve` can
/// hold a concrete `Relay<RelayTransport>`). EB-04's adapter implements the same `BusTransport`.
type RelayTransport = Box<dyn BusTransport>;

/// Application retry ceiling. JetStream itself keeps unlimited redelivery so a final unacked
/// attempt is never stranded; at this count the consumer must durably record its DLQ reference
/// before the pump sends `TERM`.
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

    /// Persist a fixed-code, payload-free quarantine reference before TERM. On OLTP failure only
    /// this delivery is NAKed; the caller continues processing valid siblings in the same batch.
    fn quarantine_then_terminate(
        &self,
        token: DeliveryToken,
        broker_ref: &myelin_events::BrokerDeliveryRef,
        delivery_attempt: u64,
        reason: DeliveryQuarantineReason,
    ) {
        let Some(transport) = &self.consumer_transport else { return };
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

    /// The booted service name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The bounded OLTP pool opened at boot (§3.3).
    pub fn pool(&self) -> &OltpPool {
        &self.pool
    }

    /// The outbox store the relay drains (so a handler/test emits into it).
    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    /// The stores auto-registered as `PersonalDataHolder`s at boot (§3.4). Every store the
    /// harness opened — OLTP / blob / cache / search index — is one [`HolderRegistration`]
    /// receipt; "we forgot a store" is structurally impossible (opening IS registering).
    pub fn registered_holders(&self) -> &[HolderRegistration] {
        self.holders.registrations()
    }

    /// The auto-registration registry the lifecycle populated (so the holder-registered
    /// architecture test can assert no store escaped registration, §3.4 / contract 1.4).
    pub fn holder_registry(&self) -> &HolderRegistry {
        &self.holders
    }

    /// The store manifest (implicit OLTP + the service's declared stores) the
    /// `holder-registered` architecture test joins against the registry (§3.4 / contract 1.4).
    pub fn store_manifest(&self) -> &StoreManifest {
        &self.manifest
    }

    /// **The `holder-registered` architecture test for this booted service (P-GA-04, contract
    /// 1.4).** Asserts every store the service declares it owns was auto-registered through the
    /// harness's one door — `Ok(())` for a harness-opened service (every declared store went
    /// through boot's [`HolderRegistry::open`]), or `Err(violations)` naming any store that
    /// escaped registration (opened outside the harness). Because [`boot`] opens every declared
    /// store, a service booted through the harness is green by construction; the violating
    /// fixture is a manifest checked against a registry that did NOT open one of its stores
    /// (the store-opened-outside-the-harness case). This is the structural realization of "the
    /// holder list cannot drift below the data map" (gdpr §3.1).
    pub fn holder_registered(&self) -> Result<(), Vec<HolderViolation>> {
        assert_all_holders_registered(&self.manifest, &self.holders)
    }

    /// The three surfaces opened in the lifecycle (§4).
    pub fn surfaces(&self) -> &[Surface] {
        self.ports.opened()
    }

    /// The live tenant-from-token public surface opened in the lifecycle (§4.1, P-S13). The
    /// gateway resolves the operating tenant for a public request through
    /// [`PublicSurface::resolve_tenant`] — tenant from the verified token, never the URL path; a
    /// mismatch is rejected + audited as a cross-tenant IDOR. `misroute_count` is the SUB-D7 zero.
    pub fn public_surface(&self) -> &PublicSurface {
        self.ports.public_surface()
    }

    /// The live liveness ≠ readiness metrics-health surface opened in the lifecycle (§4.3, P-S14,
    /// SUB-D9). After a successful boot the startup gate is `Complete`, so readiness is governed by
    /// the critical-dependency health: a dead critical dependency reports not-ready + sheds, while
    /// liveness stays `Up` (no restart-storm). `liveness_restart_count` is the SUB-D9 no-churn zero.
    pub fn metrics_health(&self) -> &MetricsHealthSurface<HealthTable> {
        self.ports.metrics_health()
    }

    /// The shared dependency-health probe behind the metrics-health surface (so a drill / the
    /// resilient client can mark a critical dependency down and watch readiness flip, §4.3).
    pub fn health_probe(&self) -> &HealthTable {
        self.ports.health_probe()
    }

    /// The producer side of the contract-1.8 telemetry signal set (§3.5). Re-observes the live
    /// signals first so a reader always sees the current truth.
    pub fn telemetry(&self) -> &Telemetry {
        self.telemetry.observe(&self.outbox, &self.consumers);
        &self.telemetry
    }

    /// **One steady-state tick:** an embedded test/deployment relay drains and delivers locally.
    /// An external-relay producer only refreshes telemetry; the elected cell publisher owns its
    /// rows and this process must not claim them.
    pub fn tick(&self) -> Vec<(ConsumerName, usize)> {
        if let Some(relay) = &self.relay {
            // Publish every committed-but-unsent outbox row only when this process explicitly owns
            // the embedded relay. External-relay consumers never enter this branch.
            relay.drain_to_empty();
        }

        // Once draining is signalled, never make a fresh broker pull. Delivery is synchronous, so
        // there is no untracked in-flight batch to finish after `tick` returns.
        if self.is_draining() && self.consumer_transport.is_some() {
            self.telemetry.observe(&self.outbox, &self.consumers);
            return Vec::new();
        }

        let batch = if let Some(transport) = &self.consumer_transport {
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
                    // Embedded delivery never crosses the external settlement seam, but the
                    // shared shape still requires a well-formed opaque token.
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
                let retry_budget_already_exhausted = delivery_attempt
                    > MAX_CONSUMER_DELIVERIES
                    && !self.consumers[index].is_handled(&env.event_id);
                let outcome = if retry_budget_already_exhausted {
                    // A prior attempt already ran the handler and exhausted its budget. Retry only
                    // the durable quarantine write; never execute the failing handler again.
                    self.consumers[index]
                        .dead_letter_exhausted_retry(&msg, delivery_attempt)
                } else {
                    self.consumers[index].deliver(&msg)
                };
                match outcome {
                    Delivered::Acked | Delivered::Deduplicated => delivered[index].1 += 1,
                    Delivered::DeadLettered(_) => {
                        if retry_budget_already_exhausted {
                            exhausted = true;
                        }
                    }
                    Delivered::Retried(delay_secs) => {
                        if delivery_attempt >= MAX_CONSUMER_DELIVERIES {
                            match self.consumers[index]
                                .dead_letter_exhausted_retry(&msg, delivery_attempt)
                            {
                                Delivered::DeadLettered(_) => exhausted = true,
                                Delivered::Retried(dlq_retry_secs) => {
                                    terminal = false;
                                    retry_after_secs = Some(retry_after_secs.map_or(
                                        dlq_retry_secs,
                                        |current: u64| current.max(dlq_retry_secs),
                                    ));
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
                if let Err(error) = transport.retry(token, delay_secs.min(300))
                {
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

    /// Request the graceful drain (stop intake, finish in-flight, ack-then-exit). The
    /// deterministic, testable drain trigger; the `SIGTERM`/`SIGINT` → this call wiring lands
    /// with the real ports (P-S13/P-S14). Idempotent.
    pub fn signal_drain(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    /// Whether a drain has been requested (intake stopped).
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// **Graceful drain (architecture §3.1):** stop intake (already signalled), finish in-flight
    /// — drain the relay to outbox-depth 0 and deliver the last published events to the
    /// consumers so nothing committed is left unprocessed — then ack-then-exit. Returns the
    /// final producer telemetry snapshot (so the drain artifact records `outbox_depth == 0`).
    pub fn drain(self) -> Telemetry {
        // stop intake.
        self.draining.store(true, Ordering::SeqCst);
        // finish in-flight: one more tick drains the relay + delivers to the consumers.
        self.tick();
        // ack-then-exit: re-observe so the final snapshot reflects depth 0 / lag 0.
        self.telemetry.observe(&self.outbox, &self.consumers);
        self.telemetry
    }

    fn owns_relay(&self) -> bool {
        self.relay.is_some()
    }
}

/// Boot the service from its [`AppSpec`] up to the pre-serve state: validate config (fail fast),
/// open the bounded pool, run the migrations, auto-register the holders, start the relay, and
/// open the three ports. Returns the [`ServeHandle`] the lifecycle drives, or an `Err` (the
/// non-zero exit) on any boot failure (§3.1). Separated from [`serve`] so a test/drill can boot,
/// drive ticks, inspect telemetry, and drive the drain deterministically.
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

    // (1) boot — validate config (§3.2, fail fast) + open the bounded OLTP pool (§3.3).
    let pool_config = oltp_config_from(&config)?;
    // @residency-cell-pinned — NAMED M0 FLOOR (residency-pin lint, P-ST-04 → P-020): the boot pool
    // is the M0 region-less pool MODEL; the cell's region pins data via the per-query
    // `(tenant, region)` `TenantScope`. The per-POOL runtime region-pin lands end-to-end in
    // P-ST-15 / P-102 (STOR-D5). Loud, named waiver (EI-01 §4), not a silent skip.
    let pool = OltpPool::open(pool_config)
        .map_err(|e| ServeError(format!("failed to open the OLTP pool at boot: {e}")))?;

    // (2) migrate — run the forward-only migrations at boot (P-S15): the runner applies them in
    //     order, refusing a destructive (`DROP`) migration AND a blocking `ALTER` on a declared-
    //     hot table (§9.1/§9.4). The substrate co-located `outbox` + `consumer_dedup` tables are
    //     part of every service's embedded set.
    let mut runner = MigrationRunner::new();
    let mut full_migrations = Migrations(vec![
        Migration::plain("0000_outbox", myelin_events::OUTBOX_MIGRATION),
        Migration::plain(
            "0001_consumer_dedup",
            myelin_events::CONSUMER_DEDUP_MIGRATION,
        ),
        // CT-004d.2 chunk 6 / #7b: the durable consumer DEAD-LETTER set (foundation id 0002) so a
        // dead-lettered event (esp. the H2 panic path) survives a restart.
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
    ]);
    full_migrations.0.extend(migrations.0);
    runner.run(&full_migrations, &hot_tables)?;

    // (5) holders — auto-register EVERY store the harness opened as a PersonalDataHolder
    //     (§3.4, GD-3) through the one door, the HolderRegistry: opening IS registering, so "we
    //     forgot a store" is structurally impossible. On this M0 floor the harness opens the
    //     OLTP store (every service has one); a service that owns a blob prefix / cache namespace
    //     / search index opens those through the same registry as its backends land (the
    //     exhaustive H1–H18 confirmation against the real holder set is P-S27). `Auto` is the
    //     only policy (a service cannot opt out).
    let HoldersSpec::Auto = holders;
    let mut holder_registry = HolderRegistry::new();
    // The OLTP store — opened, therefore registered. The OltpStoreHolder is the concrete
    // PersonalDataHolder the DSR fan-out drives (its DSR bodies are the GDPR M1 floor, P-ST-01).
    let oltp_holder = OltpStoreHolder::new(name);
    let _oltp_receipt = oltp_holder.register();
    holder_registry.open(StoreKind::Oltp, name);
    // Every OTHER store the service declares (a blob prefix / cache namespace / search index) is
    // opened through the SAME one door — so it auto-registers (§3.4, GD-3). Because boot opens
    // every declared store, the `holder-registered` architecture test (P-GA-04,
    // [`ServeHandle::holder_registered`]) is green by construction for a harness-opened service;
    // a store constructed OUTSIDE this path never registers, which the architecture test catches.
    for store in stores.stores() {
        holder_registry.open(store.kind, store.name);
    }
    // The full manifest the architecture test joins against the registry. The implicit OLTP store
    // is always part of it (every service has one); the service's declared stores extend it.
    let mut full_manifest = StoreManifest::of([crate::holder_registered::DeclaredStore::new(
        StoreKind::Oltp,
        name,
    )]);
    for store in stores.stores() {
        full_manifest.declare(store.kind, store.name);
    }

    // (3) relay — start the outbox relay automatically (§3.3, BUS-2). The relay's `published_at`
    //     clock is the boot clock; the real wall-clock source lands with the driver (P-S15).
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

    // (6) ports — open the three surfaces (§4). The tenant-from-token public surface (SUB-D7) is
    //     P-S13; liveness≠readiness on the metrics-health surface (SUB-D9, P-S14) is opened over
    //     the service's declared critical set, in the `Booting` startup state (not-ready-not-killed
    //     while boot is in flight, §4.3). The OLTP store is always critical (a service cannot serve
    //     correct traffic without its own DB); the service's declared `critical` extends that.
    let mut full_critical = vec!["oltp".to_string()];
    full_critical.extend(critical.deps().iter().map(|d| d.0.clone()));
    let mut ports = PortOpener::default();
    ports.open_all(CriticalDependencies::new(full_critical));

    // (3.5) telemetry — install the producer side of the contract-1.8 signal set on the
    //       metrics-health surface; observe the initial (zero) state.
    let telemetry = Telemetry::new();
    telemetry.observe(&outbox_store, &consumers);

    // boot succeeded → flip the metrics-health startup gate to Complete (the instance is no longer
    // not-ready *for the startup reason*; readiness is now governed purely by the critical-
    // dependency health, §4.3). A failed boot returns Err ABOVE this point, so the gate is flipped
    // only on a genuinely-complete boot — a half-booted instance never reads ready.
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

/// **The ONE call (architecture §3.1; contract 1.1).** Boots the service from its [`AppSpec`],
/// drives the lifecycle until a drain is signalled, then graceful-drains (stop intake, finish
/// in-flight, ack-then-exit). Returns `Ok(())` on a clean drain, or an `Err` (the non-zero
/// exit) on a failed boot (§3.1).
///
/// On this M0 floor the "serve until signalled" loop is driven once and the drain is signalled
/// immediately (there is no OS event loop yet — the `SIGTERM`→drain wiring is P-S13/P-S14). The
/// boot→migrate→relay→consumers→ports→drain SEQUENCE is the contract this owns and is complete;
/// use [`boot`] + [`ServeHandle`] to drive multiple ticks and inspect telemetry deterministically.
pub fn serve(spec: AppSpec) -> Result<(), ServeError> {
    let handle = boot(spec)?;
    let owns_relay = handle.owns_relay();
    // serve until signalled — drive the steady-state once, then take the drain signal. (The
    // real event loop blocking on OS signals + inbound traffic is P-S13/P-S14.)
    handle.tick();
    handle.signal_drain();
    // graceful drain — stop intake, finish in-flight, ack-then-exit. A clean drain leaves
    // outbox_depth == 0 (nothing committed is left unpublished/unprocessed).
    let final_telemetry = handle.drain();
    if owns_relay && final_telemetry.outbox_depth() != 0 {
        return Err(ServeError(format!(
            "graceful drain incomplete: outbox_depth = {} (expected 0)",
            final_telemetry.outbox_depth()
        )));
    }
    Ok(())
}

/// Run the harness-owned steady-state loop until an external shutdown trigger resolves.
///
/// The shutdown future is deliberately supplied by the service binary so OS-specific signal
/// registration stays at the process edge. Lifecycle ordering remains owned here: boot completes
/// before intake, each tick is bounded, a shutdown wins over a simultaneously-ready tick, and drain
/// is signalled before the final synchronous in-flight work is completed.
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
    let final_telemetry = handle.drain();
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

    /// A consumer whose handler increments a shared `runs` counter (so the test can assert how
    /// many distinct events it processed — dedup-skips do not increment it).
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

        fn ack(
            &self,
            token: DeliveryToken,
        ) -> Result<(), myelin_events::TransportError> {
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

        fn terminate(
            &self,
            token: DeliveryToken,
        ) -> Result<(), myelin_events::TransportError> {
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
                    if left > 0 { Some(left - 1) } else { None }
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

    fn always_retry_consumer(
        dedup: DedupLedger,
        dlq: DlqProbe,
    ) -> (ConsumerReg, Arc<AtomicU32>) {
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

    fn external_consumer_spec(probe: PullProbe, consumers: Vec<ConsumerReg>) -> AppSpec {
        external_consumer_spec_with_quarantine(probe, consumers, Arc::new(QuarantineProbe::default()))
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
                OutboxStore::new(), Box::new(probe), quarantine,
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

    /// **THE hello-world boot test (the M0 "first runnable", contract 1.1).** A service boots
    /// from `serve`'s lifecycle (`boot`), emits one event through the outbox, the relay publishes
    /// it, a consumer processes it (deduping a redelivery), and the graceful drain leaves
    /// outbox_depth == 0. The whole boot → migrate → relay → consumers → ports → drain sequence.
    #[test]
    fn hello_world_boot_emit_consume_drain() {
        // A service-owned outbox store the handler emits into + the relay drains.
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

        // (boot) — the lifecycle boots: pool open, migrations applied, holders registered, relay
        // started, three ports opened.
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

        // emit ONE event through the outbox (a handler's state-change + event, co-committed).
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

        // (serve) one steady-state tick: relay publishes the event, the consumer processes it.
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

        // the producer telemetry exports outbox_depth == 0 / dead_letter == 0 / lag == 0.
        let t = handle.telemetry();
        assert_eq!(t.outbox_depth(), 0);
        assert_eq!(t.dead_letter_count(), 0);
        assert_eq!(t.consumer_lag("indexer"), Some(0));

        // (graceful drain) — finishes in-flight, leaves depth 0.
        handle.signal_drain();
        let final_t = handle.drain();
        assert_eq!(
            final_t.outbox_depth(),
            0,
            "graceful drain leaves outbox_depth == 0"
        );
    }

    /// `serve` runs the whole lifecycle end-to-end and returns Ok on a clean drain (the CDC
    /// consumer side of 1.1 — a hello-world `main` that just calls `serve`).
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
                    stream: "TEST".into(), stream_sequence: 1,
                }),
                body: BrokerDeliveryBody::Poison(
                    myelin_events::DeliveryPoisonKind::MalformedEnvelope,
                ),
                delivery_attempt: Some(1),
            },
            myelin_events::BrokerDelivery {
                token: token(2),
                broker_ref: Some(myelin_events::BrokerDeliveryRef {
                    stream: "TEST".into(), stream_sequence: 2,
                }),
                body: BrokerDeliveryBody::Event(Box::new(event)),
                delivery_attempt: Some(1),
            },
        ]);
        let quarantine = Arc::new(QuarantineProbe::default());
        let (consumer, runs) = hello_consumer(DedupLedger::new());
        let handle = boot(external_consumer_spec_with_quarantine(
            probe.clone(), vec![consumer], quarantine.clone(),
        )).unwrap();

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
                    stream: "TEST".into(), stream_sequence: 1,
                }),
                body: BrokerDeliveryBody::Poison(
                    myelin_events::DeliveryPoisonKind::SubjectMismatch,
                ),
                delivery_attempt: Some(1),
            },
            myelin_events::BrokerDelivery {
                token: token(2),
                broker_ref: Some(myelin_events::BrokerDeliveryRef {
                    stream: "TEST".into(), stream_sequence: 2,
                }),
                body: BrokerDeliveryBody::Event(Box::new(event)),
                delivery_attempt: Some(1),
            },
        ]);
        let quarantine = Arc::new(QuarantineProbe::default());
        quarantine.fail_next.store(true, Ordering::SeqCst);
        let (consumer, runs) = hello_consumer(DedupLedger::new());
        let handle = boot(external_consumer_spec_with_quarantine(
            probe.clone(), vec![consumer], quarantine,
        )).unwrap();

        handle.tick();

        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(probe.state().retries, vec![token(1)]);
        assert_eq!(probe.state().acks, vec![token(2)]);
        assert!(probe.state().terms.is_empty());
        assert!(!handle.metrics_health().readiness().is_ready(), "OLTP failure is unhealthy");
    }

    #[test]
    fn transient_metadata_fault_is_nak_only_and_never_quarantined() {
        let probe = PullProbe::default();
        probe.state().batches.push_back(vec![myelin_events::BrokerDelivery {
            token: token(9),
            broker_ref: None,
            body: BrokerDeliveryBody::TransientMetadataFault,
            delivery_attempt: None,
        }]);
        let quarantine = Arc::new(QuarantineProbe::default());
        let handle = boot(external_consumer_spec_with_quarantine(
            probe.clone(), Vec::new(), quarantine.clone(),
        )).unwrap();

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
        let handle = boot(external_consumer_spec(
            probe.clone(),
            vec![consumer],
        ))
        .unwrap();

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
        let handle = boot(external_consumer_spec(
            probe.clone(),
            vec![consumer],
        ))
        .unwrap();

        handle.tick();

        assert!(probe.state().terms.is_empty());
        assert_eq!(probe.state().retries, vec![token(1)]);
        assert_eq!(handle.telemetry().consumer_lag("retrying"), Some(1));

        handle.tick();

        assert_eq!(probe.state().terms, vec![token(2)]);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "attempts beyond the ceiling retry only the quarantine write"
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

    /// **A failed boot returns non-zero (an `Err`), never a silent success (§3.1).** A config
    /// that fails boot-time validation (a bad/unbounded pool) aborts boot with a loud error.
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

    /// Config validation fails fast at boot on a bad env (§3.2) — `boot` itself returns the
    /// loud error before opening the pool / running migrations.
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

    /// **Graceful drain finishes in-flight before exit:** events committed but not yet
    /// published when the drain is signalled are still published + delivered during the drain
    /// (nothing in-flight is dropped). The §3.1 drain semantics.
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

        // commit two events, then signal the drain WITHOUT a steady-state tick: the drain must
        // still finish them (in-flight), not drop them.
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
        // in-flight finished: both events published + delivered, depth 0.
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

    /// A redelivery during the lifecycle is deduped (the consumer runtime's idempotency, P-S08,
    /// wired through `serve`'s tick): a second tick over the same published event does not
    /// re-run the handler. Proves `serve` wires the idempotent consumer, not a raw delivery.
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

        handle.tick(); // first delivery: handler runs once.
        handle.tick(); // the same published event is re-delivered → deduped, handler does NOT re-run.
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the redelivery was deduped (handler ran once)"
        );
    }

    /// The forward-only migration runner REJECTS a destructive (DROP) migration at boot —
    /// forward-only is structural (the `forward-only-migration` lint, P-S11, enforces the same
    /// over source; the runner refuses one at boot so a service cannot start having destroyed data).
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

    /// The migration runner applies the substrate-co-located `outbox` + `consumer_dedup` tables
    /// FIRST, then the service's own migrations, in order (the boot-time migrate phase).
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

    /// A poison event during the lifecycle dead-letters (the consumer runtime, P-S08, wired
    /// through `serve`) and is SURFACED, never silently dropped — and does not stall the tick.
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

        // the tick delivers the poison; it dead-letters (terminal) but the lifecycle continues +
        // drains cleanly (the relay still drained the outbox to 0).
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
