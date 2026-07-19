//! # The outbox relay + the `BusTransport` seam (contract 2.3 relay half — SUB-D1 / BUS-D4)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §3.3 (the
//! outbox relay — claims unsent rows with `FOR UPDATE SKIP LOCKED` (safe across replicas),
//! stamps the stable `event_id` for broker-side dedup, publishes, marks sent, dead-letters
//! after bounded retries; the relay is the ONLY component on the broker publish side).
//!
//! **Contract-index:** row 2.3 (the relay half) + the `BusTransport` trait (the owned seam).
//! **P-S07 → global P-008.** This is the **silent-data-loss floor** (SUB-D1 / BUS-D4): a
//! **PERMANENT gate**, re-run on every emit-path change.
//!
//! ## What this module ships
//! - [`BusTransport`] — the broker abstraction the relay publishes to: `put` (publish a
//!   subject + envelope with a `dedup_id`), `consume` / `ack` / `purge`. The **only** durable
//!   path; there is intentionally **no fire-and-forget** publish (the `no-raw-publish` lint,
//!   P-S10, enforces the absence workspace-wide). **Seam:** the real JetStream-class adapter is
//!   the Bus's M0 deliverable (EB-04); this prompt ships the trait + an in-process fake
//!   ([`InProcessBus`]) so the relay is drillable now. EB-04 builds its reference impl on this
//!   exact trait shape (`put/consume/ack/purge`) — no consumer rewrite.
//! - [`Relay`] — the stateless, horizontally-replicable relay: it **claims** a batch of unsent
//!   rows with the `FOR UPDATE SKIP LOCKED` discipline (two relay workers never double-claim a
//!   row), publishes each via `transport.put(subject, envelope, dedup_id = event_id)`, marks
//!   the row sent, and **dead-letters** after a bounded number of failed attempts. Because the
//!   `dedup_id` is the **stable** `event_id`, a redelivery / a re-claim after a crash is
//!   **suppressed broker-side** → **0 ghost**; because an unsent row is never dropped (it stays
//!   claimable until published or dead-lettered), → **0 lost**.
//!
//! ## How SUB-D1 / BUS-D4 are proven (the silent-data-loss floor)
//! - **BUS-D4 (emit-iff-committed):** structural in [`crate::outbox`] — a dropped transaction
//!   writes no row, so the relay can only ever publish events whose state change committed. The
//!   relay never invents a row.
//! - **SUB-D1 (kill between commit and publish → 0 ghost, 0 lost; outbox-depth drains):** the
//!   relay drains the committed-but-unsent rows; a crash mid-publish leaves the row claimable
//!   (it was committed durably) so a re-run publishes it → **0 lost**; the broker dedup on the
//!   stable `event_id` suppresses a double-publish of the same row → **0 ghost**; after the
//!   drain `outbox_depth → 0`. The drill ([`crate::outbox`] depth signal + the broker's
//!   delivered-set) reads these.
//!
//! ## Status (P-013 / EB-04, 2026-06-19) — the relay refinements, reconciled in place
//! EB-04 ("The FOR UPDATE SKIP LOCKED relay + the `BusTransport` trait, no-ghost/no-loss
//! delivery") is the **event-bus ledger's framing of the SAME deliverable the substrate roadmap
//! already shipped** (P-S07 / P-008 above): the global run order interleaves the two roadmaps,
//! so the relay + the `BusTransport` seam are reached from both. Per the coherence rule (EI-01
//! §7: never define a type twice, never build a parallel second implementation), EB-04
//! **reconciles in place** — the [`Relay`], the [`BusTransport`] trait (`put/consume/ack/purge`),
//! the `FOR UPDATE SKIP LOCKED` claim, the stable-`event_id` broker dedup (`Nats-Msg-Id`), the
//! bounded retry → dead-letter, and the SUB-D1 / BUS-D4 drills are UNCHANGED. What EB-04 ADDS
//! are the three refinements P-S07's `lib.rs` floor named as owed to EB-04 (arch §4.1's full
//! relay sentence):
//! - **The `dlq.<tenant>.<subsystem>` dead-letter subject + the dead-letter Signal alert**
//!   ([`DeadLetterAlert`], [`Relay::dead_letter_alerts`]): when a row exhausts the retry bound
//!   the relay now records a structured alert carrying the `dlq.<tenant>.<subsystem>` subject
//!   (arch §4.1: "dead-letter to `dlq.<tenant>.<subsystem>` after N attempts with a Signal
//!   alert"). A dead-letter is **never silent** — it is surfaced both in `dead_letter_count`
//!   (the contract-1.8 signal) AND as an explicit operator alert.
//! - **The 24h published-row GC** ([`Relay::gc_published`], [`OutboxStore::gc_published_before`]):
//!   a row marked `published_at` is retained for the dedup/audit window and reaped after 24h
//!   (arch §4.1: "GC published rows after 24h"). GC removes ONLY sent rows — it can never reap
//!   an unsent row (0 lost is preserved by construction).
//! - The EB-04 dated GATE artifact re-confirming SUB-D1 / BUS-D4 still read **0 ghost / 0 lost**
//!   AFTER these refinements (`tests::eb04_*`) + the `BusTransport` put/consume CDC conformance
//!   pair (`tests::cdc_bustransport_put_consume_conformance`).
//!
//! ## DEVIATION / FLOOR — modeled claim, not a real `SELECT … FOR UPDATE SKIP LOCKED`
//! There is no live OLTP DB in M0 (P-007 / the `serve` pool P-S12). The relay's claim is
//! modeled as an **atomic claim over the in-memory [`crate::outbox::OutboxStore`]**: a claimed
//! row is marked so a second relay worker SKIPs it (the observable `FOR UPDATE SKIP LOCKED`
//! property — no double-claim across replicas), and the claim is released on a failed attempt
//! so the row is retried. The real SQL `SELECT … FOR UPDATE SKIP LOCKED` against the Storage
//! pool lands when the OLTP tier client is wired (P-007 + `serve` P-S12); the relay's
//! algorithm + the drilled property do not change. **GC time model (EI-01 §1, written down):**
//! M0 has no shared wall-clock (it is initialised in `serve`, P-S12), so [`Relay::gc_published`]
//! takes the cutoff `published_before` (an RFC-3339 UTC `Timestamp`) from the caller and reaps
//! rows whose `published_at < cutoff` by lexical comparison — valid because `Z`-suffixed RFC-3339
//! UTC strings sort in time order (the same modeled-floor discipline the rest of the crate uses).
//! The real GC sweep computes `cutoff = now() − 24h` from the `serve` clock; the algorithm + the
//! "only-sent-rows-reaped" property do not change.

use crate::outbox::OutboxStore;
// Used only by the `test-support`-gated memory-arm relay mechanics (MR-009b W3b.6).
#[cfg(any(test, feature = "test-support"))]
use crate::outbox::OutboxRow;
use crate::{ArtifactRef, EventEnvelope, EventId, TenantId, Timestamp};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// The broker abstraction the relay publishes to (the owned seam; EB-04's reference impl builds
/// on this exact shape). The relay is the ONLY component on the publish side (BUS-2).
///
/// `dedup_id` is the **stable** `event_id`: the broker MUST suppress a second `put` carrying a
/// `dedup_id` it has already accepted (the `Nats-Msg-Id = event_id` broker-side dedup that
/// makes a re-claim / redelivery a no-op → 0 ghost). There is **no** non-durable
/// fire-and-forget method on this trait by construction.
pub trait BusTransport: Send + Sync {
    /// Publish `envelope` to `subject` with the stable `dedup_id` (= the event_id). Returns
    /// `Ok(Delivery::Accepted)` on a fresh publish, `Ok(Delivery::Deduplicated)` if the
    /// `dedup_id` was already accepted (the broker suppressed a duplicate — 0 ghost), or `Err`
    /// if the broker is unreachable (the relay retries, then dead-letters).
    fn put(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> std::result::Result<Delivery, TransportError>;

    /// Consume the events published to a subject pattern (the consumer side, P-S08 builds the
    /// runtime on this). Returns the delivered envelopes in publish order.
    fn consume(&self, subject_prefix: &str) -> Vec<EventEnvelope>;

    /// Acknowledge delivery up to `event_id` (the consumer's ack; the relay does not ack —
    /// it marks the outbox row sent). Part of the frozen `put/consume/ack/purge` shape.
    fn ack(&self, consumer: &str, event_id: &EventId);

    /// Purge the broker's accepted/dedup state (test/GC convenience; the 24h published-row GC
    /// is EB-04's refinement). Part of the frozen shape.
    fn purge(&self);
}

/// The narrow publish-only broker seam used by an outbox relay.
///
/// Production outbox publishers should implement this trait instead of acquiring consumer or
/// destructive stream-management capabilities they do not need. Existing [`BusTransport`]
/// implementations remain source-compatible through the blanket adapter below.
pub trait EventPublisher: Send + Sync {
    /// Durably publish an envelope with its stable event id as the broker deduplication key.
    fn publish(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> std::result::Result<Delivery, TransportError>;
}

/// The narrow pull + explicit-ack broker seam used by event consumers.
///
/// This is deliberately separate from [`EventPublisher`]: a service that consumes from a shared
/// stream must not acquire relay/publish ownership as a side effect of registering its intake.
/// Unlike the legacy consumer methods on [`BusTransport`], failures are explicit so a production
/// lifecycle can make readiness reflect whether it can currently receive and acknowledge work.
pub trait EventConsumer: Send + Sync {
    /// Pull one bounded batch from the durable consumer.
    fn consume(
        &self,
        subject_prefix: &str,
    ) -> std::result::Result<Vec<BrokerDelivery>, TransportError>;

    /// Explicitly acknowledge one terminal delivery.
    fn ack(&self, consumer: &str, event_id: &EventId) -> std::result::Result<(), TransportError>;

    /// Negatively acknowledge a retryable delivery with a bounded server-side delay.
    fn retry(
        &self,
        consumer: &str,
        event_id: &EventId,
        delay_secs: u64,
    ) -> std::result::Result<(), TransportError>;

    /// Stop broker redelivery after the application durably quarantined an exhausted retry.
    fn terminate(
        &self,
        consumer: &str,
        event_id: &EventId,
    ) -> std::result::Result<(), TransportError>;
}

/// One durable broker delivery plus the server-observed delivery attempt count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerDelivery {
    pub envelope: EventEnvelope,
    /// One-based JetStream delivery count. The application retry ceiling uses this metadata;
    /// it is never inferred from volatile process state, so restarts do not reset the budget.
    pub delivery_attempt: u64,
}

impl<T: BusTransport + ?Sized> EventPublisher for T {
    fn publish(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> std::result::Result<Delivery, TransportError> {
        self.put(subject, envelope, dedup_id)
    }
}

/// The outcome of a [`BusTransport::put`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// The publish was fresh — the event was delivered for the first time.
    Accepted,
    /// The broker already had this `dedup_id` — the duplicate was suppressed (0 ghost). A
    /// re-claim after a crash mid-publish lands here.
    Deduplicated,
}

/// A transport failure (the broker is unreachable — the SUB-D1 "kill between commit and
/// publish" / "broker down" fault). The relay retries, then dead-letters after the bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportError(pub String);

/// An in-process fake broker (the test/floor [`BusTransport`]). Records every accepted publish
/// (so a drill can assert exactly-once delivery), dedups on the stable `dedup_id` (the
/// broker-side dedup property → 0 ghost), and can be told to **fail** puts (modeling the broker
/// being unreachable — the SUB-D1 fault). Cloneable handle over shared state so the relay and
/// the drill observe one truth.
#[derive(Clone, Default)]
pub struct InProcessBus {
    inner: Arc<Mutex<BusInner>>,
}

#[derive(Default)]
struct BusInner {
    /// The accepted publishes, in order (the delivered log a consumer reads).
    delivered: Vec<(ArtifactRef, EventEnvelope)>,
    /// The dedup_ids the broker has accepted (a repeat `put` of one is suppressed).
    accepted_ids: HashSet<EventId>,
    /// Per-consumer ack high-water (the ack side of the frozen shape).
    acks: HashMap<String, EventId>,
    /// When set, every `put` fails with this error (models the broker being unreachable).
    fail_with: Option<TransportError>,
    /// When `> 0`, the next this-many `put`s fail (then succeed) — models a partial/transient
    /// outage so a drill can force the relay across multiple drain passes.
    fail_next: u32,
}

impl InProcessBus {
    /// A fresh, reachable broker with nothing delivered.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sever the broker: every subsequent `put` fails (the SUB-D1 "broker down" / "kill between
    /// commit and publish" fault). The companion to the P-S03 dependency-break injector's
    /// `Dependency::Broker` — a drill that holds the injector handle flips this.
    pub fn sever(&self) {
        self.lock().fail_with = Some(TransportError(
            "broker unreachable (severed by drill)".into(),
        ));
    }

    /// Heal the broker: `put` succeeds again (the drill restores the dependency).
    pub fn heal(&self) {
        let mut inner = self.lock();
        inner.fail_with = None;
        inner.fail_next = 0;
    }

    /// Make the next `n` `put`s fail (then succeed) — a transient/partial outage. Lets a drill
    /// force the relay across multiple drain passes (some rows publish, some retry).
    pub fn fail_next(&self, n: u32) {
        self.lock().fail_next = n;
    }

    /// The number of DISTINCT events the broker delivered (the exactly-once count — a
    /// deduplicated re-publish does NOT increment this).
    pub fn delivered_count(&self) -> usize {
        self.lock().delivered.len()
    }

    /// The set of event_ids the broker has delivered (for the 0-lost / 0-ghost assertions: the
    /// delivered set must equal exactly the committed set).
    pub fn delivered_ids(&self) -> HashSet<EventId> {
        self.lock()
            .delivered
            .iter()
            .map(|(_, e)| e.event_id.clone())
            .collect()
    }

    /// The consumer's ack high-water (the last event_id it acked), or `None` if it never acked.
    /// Lets a test observe the `ack` side of the frozen `put/consume/ack/purge` shape.
    pub fn ack_of(&self, consumer: &str) -> Option<EventId> {
        self.lock().acks.get(consumer).cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BusInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl BusTransport for InProcessBus {
    fn put(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> std::result::Result<Delivery, TransportError> {
        let mut inner = self.lock();
        if let Some(err) = &inner.fail_with {
            return Err(err.clone());
        }
        if inner.fail_next > 0 {
            inner.fail_next -= 1;
            return Err(TransportError("broker transient outage (fail_next)".into()));
        }
        // Broker-side dedup on the stable id: a repeat publish of the same event is suppressed
        // (the no-ghost guarantee — a re-claim after a crash mid-publish lands here).
        if inner.accepted_ids.contains(dedup_id) {
            return Ok(Delivery::Deduplicated);
        }
        inner.accepted_ids.insert(dedup_id.clone());
        inner.delivered.push((subject.clone(), envelope.clone()));
        Ok(Delivery::Accepted)
    }

    fn consume(&self, subject_prefix: &str) -> Vec<EventEnvelope> {
        self.lock()
            .delivered
            .iter()
            .filter(|(subject, _)| subject.0.starts_with(subject_prefix))
            .map(|(_, e)| e.clone())
            .collect()
    }

    fn ack(&self, consumer: &str, event_id: &EventId) {
        self.lock()
            .acks
            .insert(consumer.to_string(), event_id.clone());
    }

    fn purge(&self) {
        let mut inner = self.lock();
        inner.delivered.clear();
        inner.accepted_ids.clear();
        inner.acks.clear();
    }
}

/// Forwarding impl so a `Box<dyn BusTransport>` is itself a [`BusTransport`] (added P-S12 →
/// P-010): the substrate harness ([`crate`]'s `serve`) holds a `Relay<Box<dyn BusTransport>>`
/// so it can wire EITHER the in-process fake OR EB-04's JetStream-class adapter without making
/// `serve` generic over the transport. A trivial delegation — every method forwards to the
/// boxed value — so no behaviour changes; it only lets the relay erase the concrete transport
/// type. (DEVIATION note, EI-01 §1: this is an additive impl the consumer needs; it does not
/// change the frozen `put/consume/ack/purge` shape.)
impl BusTransport for Box<dyn BusTransport> {
    fn put(
        &self,
        subject: &ArtifactRef,
        envelope: &EventEnvelope,
        dedup_id: &EventId,
    ) -> std::result::Result<Delivery, TransportError> {
        (**self).put(subject, envelope, dedup_id)
    }

    fn consume(&self, subject_prefix: &str) -> Vec<EventEnvelope> {
        (**self).consume(subject_prefix)
    }

    fn ack(&self, consumer: &str, event_id: &EventId) {
        (**self).ack(consumer, event_id)
    }

    fn purge(&self) {
        (**self).purge()
    }
}

/// How many failed publish attempts a row gets before the relay dead-letters it (the bounded
/// retry — never an unbounded retry-storm). EB-04 may tune this; the floor default is 5.
pub const MAX_PUBLISH_ATTEMPTS: u32 = 5;

/// The default batch size a single durable [`Relay::drain_once`] pass claims + publishes (the
/// `FOR UPDATE SKIP LOCKED` claim bound). The in-memory arm claims every unsent row in one pass;
/// the durable arm bounds each pass so a hot outbox drains over several passes (a later wave may
/// tune this). Only observed on the `Durable` backend (passed to
/// [`DurableOutboxBacking::drain_once`]).
pub const DEFAULT_DRAIN_BATCH: usize = 256;

/// The dead-letter alert the relay raises when a row exhausts [`MAX_PUBLISH_ATTEMPTS`] (EB-04,
/// arch §4.1: "dead-letter to `dlq.<tenant>.<subsystem>` after N attempts with a Signal alert").
/// A dead-letter is **never silent**: it is surfaced both as the `dead_letter_count` survival
/// signal (contract 1.8) and as one of these explicit operator alerts, carrying the routing
/// subject + the offending `event_id` so an operator can find and re-drive the row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadLetterAlert {
    /// The `dlq.<tenant>.<subsystem>` subject the dead row is routed to (see [`dlq_subject`]).
    pub dlq_subject: String,
    /// The dead-lettered event (the stable id — the operator's handle to the quarantined row).
    pub event_id: EventId,
    /// The tenant the row belonged to (the residency/partition key — alerts stay tenant-scoped).
    pub tenant: TenantId,
    /// The owning subsystem (the first dotted segment of the event `type`) — the DLQ stream key.
    pub subsystem: String,
    /// How many publish attempts were exhausted before the row was quarantined (== the bound).
    pub attempts: u32,
}

/// The dead-letter subject for a tenant + subsystem (arch §4.1; the `dlq.<tenant>.<subsystem>`
/// routing key the relay dead-letters a poison row to after the retry bound). Subsystem is the
/// FIRST dotted segment of the event `type` (`<subsystem>.<artifact>.<event>`, §6.1) — e.g. an
/// `issues.issue.created` row dead-letters to `dlq.acme.issues`.
pub fn dlq_subject(tenant: &TenantId, subsystem: &str) -> String {
    format!("dlq.{}.{}", tenant.0, subsystem)
}

/// The owning subsystem token of an event (the first dotted segment of its `type`, §6.1). An
/// empty / malformed type yields `"unknown"` so a dead-letter is still routable (never dropped).
/// Called only by the `test-support`-gated memory drain arm (the durable arm derives the DLQ
/// routing inside `PgRelay`'s dead-letter pass), so it is gated with it (MR-009b W3b.6).
#[cfg(any(test, feature = "test-support"))]
fn subsystem_of(envelope: &EventEnvelope) -> String {
    envelope
        .type_
        .0
        .split('.')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

/// The result of one relay drain pass.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DrainReport {
    /// How many rows were published (freshly delivered) this pass.
    pub published: usize,
    /// How many publishes the broker DEDUPLICATED (a re-claim of an already-delivered row —
    /// the no-ghost path; counted so a drill can prove a re-run did not double-deliver).
    pub deduplicated: usize,
    /// How many rows failed to publish this pass (broker unreachable — they stay claimable).
    pub failed: usize,
    /// How many rows were dead-lettered this pass (exhausted the retry bound).
    pub dead_lettered: usize,
    /// **How many durable `drain_once` calls ERRORED this pass — the durable-arm error surface
    /// (W3b.2, resolving the W3b.1 staged debt).** On the `Durable` backend a `drain_once` that
    /// returns `Err` (the DB claim / the drain transaction itself failed — DISTINCT from a
    /// per-row publish `failed`, which is a reachable-broker-rejects-one-row event) is surfaced
    /// HERE and logged LOUDLY, instead of silently collapsing to an all-zero "outbox empty"
    /// report. It is `0` on every healthy pass AND on the in-memory arm (whose claim cannot
    /// fail), so an all-zero report still unambiguously means "nothing to do", while
    /// `drain_errors > 0` means "the drain itself failed — investigate" (the contract-1.8
    /// `outbox_depth`/`oldest_unsent_recorded_at` survival signals stay the durable second line
    /// of defence: a stalled drain keeps them climbing).
    pub drain_errors: usize,
}

/// The outbox relay (contract 2.3 relay half) — stateless, horizontally-replicable. Holds the
/// [`OutboxStore`] it drains and the [`BusTransport`] it publishes to (both cloneable handles),
/// plus a `clock` for the `published_at` stamp.
pub struct Relay<T: BusTransport> {
    store: OutboxStore,
    transport: T,
    /// The `published_at` stamp source (a function so it is deterministic in tests; the real
    /// clock is wired at `serve` P-S12). Read only by the `test-support`-gated memory drain arm
    /// (the durable arm stamps `published_at` in SQL inside `PgRelay`); the field + the
    /// constructor parameter stay so the `Relay::new` API is identical on both builds.
    #[cfg_attr(not(any(test, feature = "test-support")), allow(dead_code))]
    clock: Box<dyn Fn() -> Timestamp + Send + Sync>,
    /// The dead-letter alerts raised this relay's lifetime (EB-04). Each exhausted-retry row
    /// pushes a [`DeadLetterAlert`] here (the `dlq.<tenant>.<subsystem>` Signal alert, arch §4.1)
    /// — read by [`dead_letter_alerts`](Self::dead_letter_alerts) so an operator/drill sees that
    /// a poison row was SURFACED, never silently dropped.
    dead_letter_alerts: Arc<Mutex<Vec<DeadLetterAlert>>>,
}

impl<T: BusTransport> Relay<T> {
    /// A relay draining `store` to `transport`, stamping `published_at` from `clock`.
    pub fn new(
        store: OutboxStore,
        transport: T,
        clock: impl Fn() -> Timestamp + Send + Sync + 'static,
    ) -> Self {
        Relay {
            store,
            transport,
            clock: Box::new(clock),
            dead_letter_alerts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The transport this relay publishes to (so a drill can sever/heal the in-process broker).
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The dead-letter alerts this relay has raised (EB-04). A non-empty result means a poison
    /// row exhausted the retry bound and was SURFACED (routed to `dlq.<tenant>.<subsystem>` +
    /// alerted) — never silently lost. The `dead_letter_count` survival signal counts the same
    /// rows; this carries the operator-facing routing detail.
    pub fn dead_letter_alerts(&self) -> Vec<DeadLetterAlert> {
        self.dead_letter_alerts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// **GC published rows older than `published_before` (EB-04, arch §4.1: "GC published rows
    /// after 24h").** Reaps every row that has been published and whose `published_at` is
    /// strictly before the cutoff (an RFC-3339 UTC `Timestamp`; the real sweep passes
    /// `now() − 24h`). Returns the number of rows reaped. **0-lost invariant preserved:** GC only
    /// ever removes rows that were already delivered (`published_at` is set) — it can never reap
    /// an unsent row, so it cannot lose a not-yet-delivered event.
    pub fn gc_published(&self, published_before: &Timestamp) -> usize {
        // Memory-arm GC mechanic (MR-009b W3b.6: `test-support`-gated with the memory arm). The
        // durable arm reaps in the DB — the durable GC verb is the NAMED W3b.2 residual floor
        // (no rows are ever lost by its absence: only ALREADY-PUBLISHED rows are GC candidates).
        #[cfg(any(test, feature = "test-support"))]
        {
            self.store.gc_published_before(published_before)
        }
        #[cfg(not(any(test, feature = "test-support")))]
        {
            let _ = published_before;
            0
        }
    }

    /// **Drain pass: claim → publish → mark sent / dead-letter.** Claims every currently-unsent,
    /// unclaimed row with the `FOR UPDATE SKIP LOCKED` discipline (a row another worker holds is
    /// skipped), publishes each via `put(subject, envelope, dedup_id = event_id)` in
    /// `(aggregate, seq)` order, marks the row sent on `Accepted`/`Deduplicated`, and
    /// dead-letters a row that has failed [`MAX_PUBLISH_ATTEMPTS`] times. Returns a
    /// [`DrainReport`]. Idempotent: re-running after a crash mid-publish re-claims the unsent
    /// rows and the broker dedup suppresses any double-delivery (0 ghost).
    pub fn drain_once(&self) -> DrainReport {
        // Durable dispatch: the whole claim → publish → mark-sent / dead-letter pass is ONE
        // composite verb on the backing (it owns the `FOR UPDATE SKIP LOCKED` claim atomicity).
        if let Some(backing) = self.store.durable_backing() {
            // W3b.2 (resolves the W3b.1 staged debt): a backing `Err` is SURFACED, never swallowed
            // into an all-zero "outbox empty" report. It is logged LOUDLY (an operator-visible
            // stderr line) AND reported via `DrainReport::drain_errors` (distinct from a per-row
            // publish `failed`), so a stalled/failed drain is unambiguously distinguishable from a
            // drained outbox — the count is non-zero and the `outbox_depth`/`oldest_unsent`
            // survival signals stay the durable second line of defence (they keep climbing while
            // the drain cannot make progress). No committed row is lost: a failed drain transaction
            // rolls back and every unsent row stays claimable for the next pass.
            return match backing.drain_once(&self.transport, DEFAULT_DRAIN_BATCH) {
                Ok(report) => report,
                Err(e) => {
                    eprintln!(
                        "[outbox-relay] LOUD: durable drain_once FAILED — rows stay claimable \
                         (0 lost), depth/age signals remain the alarm: {}",
                        e.0
                    );
                    DrainReport {
                        drain_errors: 1,
                        ..DrainReport::default()
                    }
                }
            };
        }
        // Memory-arm drain mechanics (MR-009b W3b.6: `test-support`-gated with the memory arm;
        // in the production build `durable_backing()` is always `Some` — the enum presents only
        // `Durable` — so this point is structurally unreachable there).
        #[cfg(any(test, feature = "test-support"))]
        {
            let mut report = DrainReport::default();
            let claimed = self.store.claim_unsent();
            for row in claimed {
                match self
                    .transport
                    .put(&row.subject, &row.envelope, &row.event_id)
                {
                    Ok(Delivery::Accepted) => {
                        self.store.mark_published(&row.event_id, (self.clock)());
                        report.published += 1;
                    }
                    Ok(Delivery::Deduplicated) => {
                        // The broker already had this id (a re-claim after a crash mid-publish): the
                        // event WAS delivered, so mark the row sent — no ghost, no re-delivery.
                        self.store.mark_published(&row.event_id, (self.clock)());
                        report.deduplicated += 1;
                    }
                    Err(_) => {
                        // Broker unreachable: release the claim + bump attempts. The row stays
                        // unsent (claimable on the next pass) — 0 lost. After the bound, dead-letter.
                        let attempts = self.store.fail_attempt(&row.event_id);
                        if attempts >= MAX_PUBLISH_ATTEMPTS {
                            self.store.dead_letter(&row.event_id);
                            // EB-04: route the poison row to dlq.<tenant>.<subsystem> + raise the
                            // operator Signal alert — a dead-letter is SURFACED, never silent.
                            let subsystem = subsystem_of(&row.envelope);
                            self.dead_letter_alerts
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(DeadLetterAlert {
                                    dlq_subject: dlq_subject(&row.envelope.tenant, &subsystem),
                                    event_id: row.event_id.clone(),
                                    tenant: row.envelope.tenant.clone(),
                                    subsystem,
                                    attempts,
                                });
                            report.dead_lettered += 1;
                        } else {
                            report.failed += 1;
                        }
                    }
                }
            }
            report
        }
        #[cfg(not(any(test, feature = "test-support")))]
        unreachable!(
            "a production OutboxStore is Durable-only (the Memory arm is test-support-gated)"
        )
    }

    /// Drain repeatedly until the outbox depth reaches 0 or no progress is made (every remaining
    /// row is failing / dead-lettered). Returns the cumulative report. This is the "outbox-depth
    /// drains" half of SUB-D1: with a reachable broker it drives depth → 0.
    pub fn drain_to_empty(&self) -> DrainReport {
        let mut total = DrainReport::default();
        loop {
            if self.store.outbox_depth() == 0 {
                break;
            }
            let r = self.drain_once();
            // Progress = published OR deduplicated. The `dead_lettered` count is structurally ALWAYS
            // 0 within THIS loop (a row only dead-letters on its MAX_PUBLISH_ATTEMPTS-th FAILED pass,
            // but an all-failed pass makes NO progress so the loop breaks BEFORE any row reaches the
            // dead-letter bound — dead-lettering happens via repeated `drain_once`, never inside
            // `drain_to_empty`). So a `dead_lettered` progress term / accumulation would be dead code
            // (always 0); it is deliberately OMITTED — the report still surfaces dead-letters via
            // `drain_once`, and `dead_letter_count()` is the durable surfaced signal.
            let made_progress = r.published > 0 || r.deduplicated > 0;
            total.published += r.published;
            total.deduplicated += r.deduplicated;
            total.failed += r.failed;
            // Surface a durable-arm drain error across the loop too (a failed drain makes no
            // progress → the loop breaks below, but the error is not silently dropped).
            total.drain_errors += r.drain_errors;
            // No progress (broker down: every row failed, none dead-lettered yet) → stop so we
            // do not spin forever. The caller heals the broker and drains again.
            if !made_progress {
                break;
            }
        }
        total
    }
}

// --- The claim / mark-sent / dead-letter operations the relay drives over the store. These
// live on OutboxStore but are relay-internal mechanics (modeling the SQL row updates), so they
// are defined here against the store's claim API.
//
// **MR-009b W3b.6 — `test-support`-gated with the memory arm they operate on** (the durable arm
// owns claim/mark/dead-letter/GC inside its single composite `drain_once` verb — `PgRelay`'s
// `FOR UPDATE SKIP LOCKED` transaction — and never routes here). ---

#[cfg(any(test, feature = "test-support"))]
impl OutboxStore {
    /// Claim every currently-unsent, unclaimed row (the `FOR UPDATE SKIP LOCKED` batch), ordered
    /// `(aggregate, seq)`. A claimed row is marked so a SECOND relay worker's claim SKIPs it (no
    /// double-claim across replicas). Returns the claimed rows for the relay to publish.
    pub(crate) fn claim_unsent(&self) -> Vec<OutboxRow> {
        // Memory-arm mechanic: the durable arm claims inside its own `drain_once` composite verb,
        // never here (so a `Durable` store yields no in-memory claim).
        let Some(mut inner) = self.mem() else {
            return Vec::new();
        };
        // Collect unsent + unclaimed ids in insertion order, then sort by (aggregate, seq) so a
        // given aggregate drains in order (per-aggregate ordering, D-9).
        let mut to_claim: Vec<OutboxRow> = inner
            .order
            .iter()
            .filter_map(|id| inner.rows.get(id).cloned())
            .filter(|r| r.published_at.is_none() && !inner.claimed.contains(&r.event_id))
            .collect();
        to_claim.sort_by_key(|r| (r.aggregate.0.clone(), r.seq));
        for r in &to_claim {
            inner.claimed.insert(r.event_id.clone());
        }
        to_claim
    }

    /// Mark a row published (the relay published it / the broker deduped it). Releases the claim.
    pub(crate) fn mark_published(&self, id: &EventId, at: Timestamp) {
        let Some(mut inner) = self.mem() else {
            return;
        };
        if let Some(row) = inner.rows.get_mut(id) {
            row.published_at = Some(at);
        }
        inner.claimed.remove(id);
    }

    /// Record a failed publish attempt (broker unreachable): bump `attempts`, release the claim
    /// so the row is re-claimable next pass. Returns the new attempt count.
    pub(crate) fn fail_attempt(&self, id: &EventId) -> u32 {
        let Some(mut inner) = self.mem() else {
            return 0;
        };
        let attempts = if let Some(row) = inner.rows.get_mut(id) {
            row.attempts += 1;
            row.attempts
        } else {
            0
        };
        inner.claimed.remove(id);
        attempts
    }

    /// Dead-letter a row (exhausted the retry bound): move it out of the live set into the
    /// dead-letter list. The operator alert + the `dlq.<tenant>.<subsystem>` subject is raised by
    /// the relay ([`Relay::drain_once`], EB-04). It is no longer in `outbox_depth` (it is not
    /// lost — it is quarantined, visibly, in `dead_letter_count`).
    pub(crate) fn dead_letter(&self, id: &EventId) {
        let Some(mut inner) = self.mem() else {
            return;
        };
        if let Some(row) = inner.rows.remove(id) {
            inner.order.retain(|x| x != id);
            inner.dead_letters.push(row);
        }
        inner.claimed.remove(id);
    }

    /// **GC published rows older than `cutoff` (EB-04, arch §4.1: "GC published rows after 24h").**
    /// Removes every row whose `published_at` is set AND strictly lexically before `cutoff` (an
    /// RFC-3339 UTC `Z`-suffixed `Timestamp` sorts in time order). Returns the count reaped.
    ///
    /// **0-lost preserved by construction:** the filter requires `published_at.is_some()`, so an
    /// **unsent** row (`published_at IS NULL`, the rows `outbox_depth` counts) is NEVER reaped —
    /// GC can only ever free already-delivered rows from the dedup/audit retention window.
    pub(crate) fn gc_published_before(&self, cutoff: &Timestamp) -> usize {
        // Memory-arm GC mechanic; the durable arm reaps in the DB (a later wave), never here.
        let Some(mut inner) = self.mem() else {
            return 0;
        };
        let reap: Vec<EventId> = inner
            .rows
            .values()
            .filter(|r| r.published_at.as_ref().is_some_and(|p| p.0 < cutoff.0))
            .map(|r| r.event_id.clone())
            .collect();
        for id in &reap {
            inner.rows.remove(id);
            inner.order.retain(|x| x != id);
        }
        reap.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::{EmitContextBase, IdMinter, MonotonicMinter};
    use crate::{
        Actor, AggregateKey, CausedBy, DataRole, EventDraft, EventType, OutboxTx, Region, TenantId,
        Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn clock() -> Timestamp {
        Timestamp("2026-06-19T00:00:02Z".into())
    }

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
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn draft(type_: &str, aggregate: &str) -> EventDraft {
        EventDraft {
            type_: EventType(type_.into()),
            subject: crate::ArtifactRef(format!("myelin://acme/issues/issue/{aggregate}")),
            aggregate: AggregateKey(aggregate.into()),
            payload: serde_json::json!({ "ref": aggregate }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    fn commit_n(
        store: &OutboxStore,
        minter: Arc<dyn IdMinter>,
        n: usize,
        aggregate: &str,
    ) -> Vec<EventId> {
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("state");
        let mut ids = Vec::new();
        for i in 0..n {
            ids.push(
                tx.emit(draft(&format!("issues.issue.e{i}"), aggregate), None)
                    .unwrap(),
            );
        }
        tx.commit().unwrap();
        ids
    }

    /// The happy path: a relay drains every committed row exactly once and the depth → 0.
    #[test]
    fn relay_drains_committed_rows_exactly_once_depth_to_zero() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 3, "issue:PROJ-1");
        assert_eq!(store.outbox_depth(), 3);

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let report = relay.drain_to_empty();

        assert_eq!(report.published, 3, "every committed event published once");
        assert_eq!(report.deduplicated, 0);
        assert_eq!(store.outbox_depth(), 0, "outbox-depth drains to 0 (SUB-D1)");
        // 0 ghost / 0 lost: the delivered set equals exactly the committed set.
        assert_eq!(bus.delivered_count(), 3);
        assert_eq!(bus.delivered_ids(), ids.into_iter().collect());
    }

    /// **SUB-D1 — kill between commit and publish → 0 ghost, 0 lost.** The broker is severed
    /// (the "kill between commit and publish" fault); the relay cannot publish — the rows stay
    /// in the outbox (0 lost). On heal, the relay drains them → delivered exactly once (0 ghost),
    /// depth → 0.
    #[test]
    fn sub_d1_kill_between_commit_and_publish_zero_ghost_zero_lost() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 4, "issue:PROJ-1");

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);

        // (1) sever the broker (kill between commit and publish): the events are committed but
        // cannot be published.
        bus.sever();
        relay.drain_to_empty();
        // 0 LOST: every committed event is still in the outbox (depth unchanged), none ghosted.
        assert_eq!(
            store.outbox_depth(),
            4,
            "severed broker → events parked, not lost"
        );
        assert_eq!(bus.delivered_count(), 0, "nothing delivered while severed");

        // (2) heal + drain: every event is delivered exactly once (0 ghost), depth → 0.
        bus.heal();
        let report = relay.drain_to_empty();
        assert_eq!(report.published, 4);
        assert_eq!(
            store.outbox_depth(),
            0,
            "outbox-depth drains after heal (SUB-D1)"
        );
        assert_eq!(
            bus.delivered_count(),
            4,
            "exactly-once: 4 committed → 4 delivered"
        );
        assert_eq!(
            bus.delivered_ids(),
            ids.into_iter().collect(),
            "0 ghost / 0 lost"
        );
    }

    /// **SUB-D1 idempotence — a re-claim after a crash mid-publish does not double-deliver.** We
    /// publish, then RE-CLAIM the same row (simulating a relay that crashed after publishing but
    /// before marking the row sent) and drive it through the broker again: the broker dedup
    /// suppresses it (Deduplicated, not a second delivery) → 0 ghost.
    #[test]
    fn relay_reclaim_after_crash_is_deduplicated_zero_ghost() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 1, "issue:PROJ-1");
        let id = ids[0].clone();

        let bus = InProcessBus::new();
        // Publish directly via the transport (the relay published, then "crashed" before marking
        // the row sent — so the row is still unsent in the store).
        let row = store.row(&id).unwrap();
        assert_eq!(
            bus.put(&row.subject, &row.envelope, &row.event_id).unwrap(),
            Delivery::Accepted
        );
        assert_eq!(
            store.outbox_depth(),
            1,
            "row still unsent (crash before mark)"
        );

        // The relay re-runs: it re-claims the unsent row and re-publishes — the broker dedups it.
        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let report = relay.drain_once();
        assert_eq!(
            report.deduplicated, 1,
            "the re-claim was deduplicated (0 ghost)"
        );
        assert_eq!(report.published, 0);
        assert_eq!(
            bus.delivered_count(),
            1,
            "still exactly one delivery (no ghost)"
        );
        assert_eq!(
            store.outbox_depth(),
            0,
            "the row is marked sent after the dedup"
        );
    }

    /// `FOR UPDATE SKIP LOCKED`: two relay workers claiming concurrently never double-claim a
    /// row — the second worker SKIPs what the first holds.
    #[test]
    fn skip_locked_two_workers_never_double_claim() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 5, "issue:PROJ-1");

        // worker A claims the batch.
        let a = store.claim_unsent();
        assert_eq!(a.len(), 5);
        // worker B claims concurrently: A holds all 5, so B gets none (SKIP LOCKED).
        let b = store.claim_unsent();
        assert!(b.is_empty(), "a second worker skips already-claimed rows");

        // A fails to publish (releases the claims); now B can claim them.
        for r in &a {
            store.fail_attempt(&r.event_id);
        }
        let b2 = store.claim_unsent();
        assert_eq!(b2.len(), 5, "released claims are re-claimable");
    }

    /// A row that fails to publish [`MAX_PUBLISH_ATTEMPTS`] times is dead-lettered (bounded
    /// retry — never an unbounded retry-storm), and is then OUT of `outbox_depth` but visibly
    /// quarantined in `dead_letter_count` (not lost — surfaced).
    #[test]
    fn bounded_retries_then_dead_letter() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 1, "issue:PROJ-1");

        let bus = InProcessBus::new();
        bus.sever(); // broker permanently down for this row.
        let relay = Relay::new(store.clone(), bus.clone(), clock);

        // Each pass is one failed attempt; after MAX_PUBLISH_ATTEMPTS the row dead-letters.
        for _ in 0..MAX_PUBLISH_ATTEMPTS {
            relay.drain_once();
        }
        assert_eq!(store.outbox_depth(), 0, "the row left the unsent set");
        assert_eq!(
            store.dead_letter_count(),
            1,
            "it is dead-lettered, not silently lost"
        );
        assert_eq!(bus.delivered_count(), 0);
    }

    /// `dead_letter` removes ONLY the dead-lettered row from the live `order`, leaving the other
    /// committed rows intact and deliverable. Pins the `order.retain(x != id)` filter (a flipped
    /// `==` would drop the survivors / keep the dead one).
    #[test]
    fn dead_letter_removes_only_the_dead_row_survivors_still_deliver() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 3, "issue:PROJ-1");
        assert_eq!(store.outbox_depth(), 3);

        // dead-letter the MIDDLE row directly (the relay mechanic).
        store.dead_letter(&ids[1]);
        assert_eq!(store.dead_letter_count(), 1);
        assert_eq!(
            store.outbox_depth(),
            2,
            "only the dead row left the live set"
        );
        assert!(
            store.row(&ids[1]).is_none(),
            "the dead row is gone from live rows"
        );
        assert!(store.row(&ids[0]).is_some(), "survivor 0 still live");
        assert!(store.row(&ids[2]).is_some(), "survivor 2 still live");

        // the two survivors still drain to the broker exactly once.
        let bus = InProcessBus::new();
        Relay::new(store.clone(), bus.clone(), clock).drain_to_empty();
        assert_eq!(bus.delivered_count(), 2);
        let delivered = bus.delivered_ids();
        assert!(delivered.contains(&ids[0]) && delivered.contains(&ids[2]));
        assert!(
            !delivered.contains(&ids[1]),
            "the dead-lettered row is not delivered"
        );
    }

    /// `drain_once` reports exactly what it did this pass: a fresh drain of 3 rows reports
    /// `published == 3` and zero everything-else. Pins the per-pass report increments.
    #[test]
    fn drain_once_report_counts_this_pass_exactly() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 3, "issue:PROJ-1");
        let relay = Relay::new(store.clone(), InProcessBus::new(), clock);

        let r = relay.drain_once();
        assert_eq!(r.published, 3, "this pass published exactly 3");
        assert_eq!(r.deduplicated, 0);
        assert_eq!(r.failed, 0);
        assert_eq!(r.dead_lettered, 0);
        // a second pass on an empty outbox publishes nothing.
        let r2 = relay.drain_once();
        assert_eq!(
            r2,
            DrainReport::default(),
            "an empty drain pass reports all-zero"
        );
    }

    /// `drain_once` reports `failed == N` (one per claimed row) when the broker is severed but
    /// the retry bound is not yet hit — the report's `failed` field is load-bearing.
    #[test]
    fn drain_once_reports_failed_while_severed_under_bound() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 2, "issue:PROJ-1");
        let bus = InProcessBus::new();
        bus.sever();
        let relay = Relay::new(store.clone(), bus, clock);

        let r = relay.drain_once();
        assert_eq!(r.failed, 2, "both rows failed this pass (broker severed)");
        assert_eq!(r.published, 0);
        assert_eq!(r.dead_lettered, 0);
        // the rows are NOT lost: still in the outbox, attempts bumped.
        assert_eq!(store.outbox_depth(), 2);
    }

    /// Across repeated relay cycles (each a `drain_once`) a permanently-severed row reports
    /// `failed` per pass under the bound, then `dead_lettered` on the bound-th pass — and the
    /// dead-lettered row is SURFACED (not silently lost). Pins the per-pass `failed` /
    /// `dead_lettered` report fields + the dead-letter surfacing.
    #[test]
    fn repeated_drain_failed_then_dead_letters_and_surfaces() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 1, "issue:PROJ-1");
        let bus = InProcessBus::new();
        bus.sever();
        let relay = Relay::new(store.clone(), bus, clock);

        let mut failed_total = 0usize;
        let mut dead_total = 0usize;
        for pass in 1..=MAX_PUBLISH_ATTEMPTS {
            let r = relay.drain_once();
            failed_total += r.failed;
            dead_total += r.dead_lettered;
            if pass < MAX_PUBLISH_ATTEMPTS {
                assert_eq!(r.failed, 1, "under the bound, each pass reports one failed");
                assert_eq!(r.dead_lettered, 0);
            } else {
                assert_eq!(r.dead_lettered, 1, "the bound-th pass dead-letters");
                assert_eq!(r.failed, 0);
            }
        }
        assert_eq!(failed_total, (MAX_PUBLISH_ATTEMPTS - 1) as usize);
        assert_eq!(dead_total, 1);
        assert_eq!(store.dead_letter_count(), 1);
        // the dead-lettered row is surfaced (not silently lost) — `dead_letters()` returns it.
        let dl = store.dead_letters();
        assert_eq!(dl.len(), 1);
        assert_eq!(
            dl[0].attempts, MAX_PUBLISH_ATTEMPTS,
            "attempts hit the bound"
        );
    }

    /// `drain_to_empty` keeps looping while it makes progress and ACCUMULATES the published
    /// count across passes: with a transient outage failing the first 2 puts, pass 1 publishes
    /// 3 (the 5 claimed minus 2 transient failures), pass 2 publishes the 2 retried — total 5,
    /// depth → 0. Pins the cross-pass `total.published +=` accumulation + the keep-looping
    /// progress guard.
    #[test]
    fn drain_to_empty_loops_and_accumulates_published_across_passes() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 5, "issue:PROJ-1");
        let bus = InProcessBus::new();
        // the first 2 puts fail transiently, then the broker recovers.
        bus.fail_next(2);
        let relay = Relay::new(store.clone(), bus.clone(), clock);

        let total = relay.drain_to_empty();
        // pass 1: 5 claimed, 2 transient-fail, 3 published; pass 2: 2 published. Total 5.
        assert_eq!(
            total.published, 5,
            "published accumulates across passes (3 + 2)"
        );
        assert_eq!(total.failed, 2, "the 2 transient failures are reported");
        assert_eq!(
            store.outbox_depth(),
            0,
            "the outbox fully drained over 2 passes"
        );
        assert_eq!(
            bus.delivered_count(),
            5,
            "exactly 5 delivered, none ghosted/lost"
        );
    }

    /// `drain_to_empty` counts DEDUPLICATED rows as progress and accumulates them: if the rows
    /// were already delivered to the broker (a prior relay crashed after publishing, before
    /// marking sent), `drain_to_empty` re-claims them, the broker dedups, and they are marked
    /// sent — `total.deduplicated` reflects them and depth → 0. Pins the `deduplicated`
    /// aggregation + its progress-guard term.
    #[test]
    fn drain_to_empty_counts_deduplicated_as_progress() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 3, "issue:PROJ-1");
        let bus = InProcessBus::new();
        // pre-deliver every row to the broker directly (the "crashed before mark sent" state):
        // the rows are delivered but still unsent in the store.
        for id in &ids {
            let row = store.row(id).unwrap();
            bus.put(&row.subject, &row.envelope, &row.event_id).unwrap();
        }
        assert_eq!(
            store.outbox_depth(),
            3,
            "rows delivered but not yet marked sent"
        );

        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let total = relay.drain_to_empty();
        assert_eq!(
            total.deduplicated, 3,
            "all 3 re-claims were deduplicated (0 ghost)"
        );
        assert_eq!(
            total.published, 0,
            "nothing newly published (already delivered)"
        );
        assert_eq!(store.outbox_depth(), 0, "depth drains via the dedup path");
        assert_eq!(
            bus.delivered_count(),
            3,
            "still exactly 3 delivered (no double-delivery)"
        );
    }

    /// `drain_to_empty` stops (does not spin forever) when the broker is severed and no progress
    /// is made — and leaves the rows safely parked (0 lost). The progress-guard is load-bearing.
    #[test]
    fn drain_to_empty_stops_on_no_progress_when_severed() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 3, "issue:PROJ-1");
        let bus = InProcessBus::new();
        bus.sever();
        let relay = Relay::new(store.clone(), bus, clock);

        // Returns (does not hang); reports the failed rows; the rows are still parked.
        let total = relay.drain_to_empty();
        assert_eq!(total.published, 0);
        assert_eq!(total.failed, 3, "one failed per claimed row, then it stops");
        assert_eq!(
            store.outbox_depth(),
            3,
            "0 lost: the rows stay parked while severed"
        );
    }

    /// **`drain_to_empty` keeps looping while it makes progress via DEDUP across MULTIPLE passes.**
    /// Pre-deliver 5 rows to the broker (the crash-before-mark state), then a transient outage fails
    /// the first 2 puts of pass 1: pass 1 dedups 3 (depth 2 left, progress via dedup); pass 2 dedups
    /// the 2 retried — total deduplicated 5, depth → 0. Pins the `r.deduplicated > 0` progress term
    /// and the second `||` (a `>`→`<` or `||`→`&&` mutant would stop after pass 1, leaving depth 2).
    /// (P-507 mutation gate.)
    #[test]
    fn drain_to_empty_loops_on_dedup_progress_across_passes() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 5, "issue:PROJ-1");
        let bus = InProcessBus::new();
        // pre-deliver every row directly (crash-before-mark): each id is now in the broker's
        // accepted set, so a relay re-claim will be Deduplicated (not published).
        for id in &ids {
            let row = store.row(id).unwrap();
            bus.put(&row.subject, &row.envelope, &row.event_id).unwrap();
        }
        assert_eq!(store.outbox_depth(), 5, "delivered but not yet marked sent");
        // a transient outage fails the first 2 re-claims of pass 1 (checked BEFORE dedup), so pass 1
        // can only dedup 3 → depth 2 remains → the loop MUST continue (driven by dedup progress).
        bus.fail_next(2);

        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let total = relay.drain_to_empty();
        assert_eq!(
            total.deduplicated, 5,
            "all 5 dedups accumulate ACROSS the two passes (the dedup progress term keeps looping)"
        );
        assert_eq!(
            total.published, 0,
            "nothing newly published (already delivered)"
        );
        assert_eq!(total.failed, 2, "the 2 transient failures are reported");
        assert_eq!(
            store.outbox_depth(),
            0,
            "the outbox fully drained over 2 passes via the dedup path (0 lost)"
        );
        assert_eq!(
            bus.delivered_count(),
            5,
            "still exactly 5 delivered (no ghost)"
        );
    }

    /// The in-process broker's `purge` clears the delivered/dedup state, and `ack` records the
    /// consumer high-water (the frozen `put/consume/ack/purge` shape is fully exercised).
    #[test]
    fn transport_ack_and_purge_behave() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 2, "issue:PROJ-1");
        let bus = InProcessBus::new();
        Relay::new(store.clone(), bus.clone(), clock).drain_to_empty();
        assert_eq!(bus.delivered_count(), 2);

        bus.ack("indexer", &ids[1]);
        assert_eq!(
            bus.ack_of("indexer").as_ref(),
            Some(&ids[1]),
            "ack recorded the high-water"
        );
        assert_eq!(
            bus.ack_of("nobody"),
            None,
            "an un-acked consumer reads None"
        );
        // purge clears the delivered + dedup state, so a re-publish is accepted afresh.
        bus.purge();
        assert_eq!(bus.delivered_count(), 0, "purge cleared the delivered log");
        let row = store.row(&ids[0]).unwrap();
        assert_eq!(
            bus.put(&row.subject, &row.envelope, &row.event_id).unwrap(),
            Delivery::Accepted,
            "after purge the dedup state is gone — the id is accepted fresh"
        );
    }

    /// The consumer side of the 2.1/2.3 CDC: a consumer reading the broker sees the SAME wire
    /// envelope the relay published (the provider→consumer pair the contract-coverage scanner,
    /// P-S21, reads). This is the CONSUMER half the P-S05/P-S06 provider CDC named as landing
    /// here.
    #[test]
    fn cdc_2_3_consumer_reads_the_relayed_envelope() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 2, "issue:PROJ-1");

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);
        relay.drain_to_empty();

        // The consumer reads the published subjects: it gets exactly the relayed envelopes,
        // in (aggregate, seq) order, byte-identical to what the provider emitted.
        let consumed = bus.consume("myelin://acme/issues/issue/");
        assert_eq!(consumed.len(), 2);
        assert_eq!(consumed[0].event_id, ids[0]);
        assert_eq!(consumed[1].event_id, ids[1]);
        // the envelope the consumer reads round-trips against what the store holds (provider).
        let provider_row = store.row(&ids[0]).unwrap();
        assert_eq!(
            consumed[0], provider_row.envelope,
            "consumer sees the provider's wire shape"
        );

        // ack is part of the frozen shape.
        bus.ack("indexer", &ids[1]);
    }

    // ===================================================================================
    // EB-04 (P-013) — the relay refinements P-S07 floor-named as owed here: the
    // dlq.<tenant>.<subsystem> dead-letter Signal alert, the 24h published-row GC, and the
    // BusTransport put/consume CDC conformance pair + the SUB-D1/BUS-D4 re-confirm gate.
    // ===================================================================================

    /// `dlq_subject` builds the arch §4.1 routing key `dlq.<tenant>.<subsystem>` and
    /// `subsystem_of` reads the FIRST dotted segment of the event `type` (§6.1). An
    /// `issues.issue.created` row for tenant `acme` routes to `dlq.acme.issues`.
    #[test]
    fn eb04_dlq_subject_is_tenant_and_subsystem() {
        assert_eq!(
            dlq_subject(&TenantId("acme".into()), "issues"),
            "dlq.acme.issues"
        );
        // subsystem is the first dotted segment of the type; a malformed/empty type is routable.
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 1, "issue:PROJ-1");
        let row = store.row(&ids[0]).unwrap();
        assert_eq!(
            subsystem_of(&row.envelope),
            "issues",
            "type 'issues.issue.eN' → 'issues'"
        );
    }

    /// **EB-04 — a poison row that exhausts the retry bound is dead-lettered AND raises a
    /// `dlq.<tenant>.<subsystem>` Signal alert (surfaced, never silent).** The broker is
    /// permanently severed; after [`MAX_PUBLISH_ATTEMPTS`] the row dead-letters, `dead_letter_count`
    /// counts it (contract-1.8 signal), AND the relay records a [`DeadLetterAlert`] carrying the
    /// `dlq.acme.issues` subject + the offending event_id — the two surfacing paths arch §4.1
    /// requires.
    #[test]
    fn eb04_dead_letter_raises_dlq_alert_surfaced_not_silent() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 1, "issue:PROJ-1");

        let bus = InProcessBus::new();
        bus.sever(); // broker permanently down for this row.
        let relay = Relay::new(store.clone(), bus, clock);

        // no alert before the bound is hit.
        for _ in 0..(MAX_PUBLISH_ATTEMPTS - 1) {
            relay.drain_once();
        }
        assert!(
            relay.dead_letter_alerts().is_empty(),
            "no alert before the retry bound"
        );
        // the bound-th pass dead-letters AND alerts.
        relay.drain_once();

        assert_eq!(
            store.dead_letter_count(),
            1,
            "the row is dead-lettered (count signal)"
        );
        let alerts = relay.dead_letter_alerts();
        assert_eq!(
            alerts.len(),
            1,
            "exactly one dead-letter Signal alert raised"
        );
        assert_eq!(
            alerts[0].dlq_subject, "dlq.acme.issues",
            "routed to dlq.<tenant>.<subsystem>"
        );
        assert_eq!(
            alerts[0].event_id, ids[0],
            "the alert carries the offending event_id"
        );
        assert_eq!(alerts[0].tenant, TenantId("acme".into()));
        assert_eq!(alerts[0].subsystem, "issues");
        assert_eq!(
            alerts[0].attempts, MAX_PUBLISH_ATTEMPTS,
            "the bound was exhausted"
        );
    }

    /// **EB-04 — the 24h published-row GC reaps only SENT rows past the cutoff; it can never reap
    /// an unsent row (0 lost preserved).** Three rows: two published (one before the cutoff, one
    /// after), one left unsent. GC with the cutoff reaps ONLY the old published row — the recent
    /// published row and the unsent row both survive.
    #[test]
    fn eb04_gc_reaps_only_old_published_rows_never_unsent() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 3, "issue:PROJ-1");

        // Mark row 0 published "long ago", row 1 published "just now"; leave row 2 unsent.
        store.mark_published(&ids[0], Timestamp("2026-06-17T00:00:00Z".into())); // > 24h old
        store.mark_published(&ids[1], Timestamp("2026-06-19T11:59:00Z".into())); // recent
        assert_eq!(store.outbox_depth(), 1, "row 2 is still unsent");

        // GC cutoff = now − 24h = 2026-06-18T12:00:00Z. Row 0 (06-17) is older → reaped; row 1
        // (06-19) is newer → kept; row 2 is unsent → NEVER reaped.
        let relay = Relay::new(store.clone(), InProcessBus::new(), clock);
        let reaped = relay.gc_published(&Timestamp("2026-06-18T12:00:00Z".into()));

        assert_eq!(reaped, 1, "only the >24h-old published row is GC'd");
        assert!(
            store.row(&ids[0]).is_none(),
            "the old published row was reaped"
        );
        assert!(
            store.row(&ids[1]).is_some(),
            "the recent published row is retained"
        );
        assert!(
            store.row(&ids[2]).is_some(),
            "the UNSENT row is NEVER reaped (0 lost)"
        );
        assert_eq!(store.outbox_depth(), 1, "GC did not touch the unsent set");
    }

    /// **EB-04 — GC over a fully-drained outbox with an old cutoff frees the whole retention
    /// window; an unsent outbox under any cutoff loses nothing.** Pins that GC's `published_at
    /// IS NOT NULL` guard is load-bearing: a severed outbox (every row unsent) is untouched by GC.
    #[test]
    fn eb04_gc_never_reaps_a_severed_outbox() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 4, "issue:PROJ-1");
        // nothing published — every row is unsent.
        let relay = Relay::new(store.clone(), InProcessBus::new(), clock);
        // even with a cutoff in the far future, no unsent row is reaped (0 lost).
        let reaped = relay.gc_published(&Timestamp("2099-01-01T00:00:00Z".into()));
        assert_eq!(reaped, 0, "an unsent outbox loses nothing to GC");
        assert_eq!(
            store.outbox_depth(),
            4,
            "all four rows still parked + deliverable"
        );
    }

    /// **EB-04 — GC reaps STRICTLY before the cutoff: a row published EXACTLY at the cutoff is
    /// retained.** Pins the `published_at < cutoff` strict comparison (a `<=` mutant would reap a
    /// row at the cutoff instant — but the retention window is `[cutoff, now]`, so a row at the
    /// cutoff boundary is the youngest still-retained row, NOT reaped). (P-507 mutation gate.)
    #[test]
    fn eb04_gc_is_strict_before_cutoff_a_row_at_the_cutoff_is_retained() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 2, "issue:PROJ-1");
        let cutoff = Timestamp("2026-06-18T12:00:00Z".into());

        // row 0 published one second BEFORE the cutoff → reaped; row 1 published EXACTLY at the
        // cutoff → retained (strict `<`).
        store.mark_published(&ids[0], Timestamp("2026-06-18T11:59:59Z".into()));
        store.mark_published(&ids[1], cutoff.clone());

        let relay = Relay::new(store.clone(), InProcessBus::new(), clock);
        let reaped = relay.gc_published(&cutoff);
        assert_eq!(
            reaped, 1,
            "only the row strictly BEFORE the cutoff is reaped"
        );
        assert!(
            store.row(&ids[0]).is_none(),
            "the row before the cutoff was reaped"
        );
        assert!(
            store.row(&ids[1]).is_some(),
            "the row AT the cutoff is retained (strict `<`, not `<=`)"
        );
    }

    /// **EB-04 — the `Box<dyn BusTransport>` blanket forward is faithful for `ack` + `purge`.** The
    /// harness holds a `Relay<Box<dyn BusTransport>>`, so the boxed forwarders (each `(**self)
    /// .method(..)`) MUST reach the inner transport. Drives `ack`/`purge` THROUGH the boxed type
    /// and asserts the effect lands on the inner bus (a no-op `()` mutant of either forwarder would
    /// silently drop the ack/purge). (P-507 mutation gate.)
    #[test]
    fn boxed_bustransport_forwards_ack_and_purge_to_the_inner_bus() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 2, "issue:PROJ-1");
        let inner = InProcessBus::new();
        // box the transport (the harness shape) and drain through it.
        let boxed: Box<dyn BusTransport> = Box::new(inner.clone());
        Relay::new(store.clone(), boxed, clock).drain_to_empty();
        assert_eq!(
            inner.delivered_count(),
            2,
            "drained through the boxed transport"
        );

        // consume THROUGH a boxed handle returns the relayed envelopes (a `vec![]` mutant of the
        // boxed `consume` forward would return nothing — pin the forward).
        let boxed_consumer: Box<dyn BusTransport> = Box::new(inner.clone());
        let consumed = boxed_consumer.consume("myelin://acme/issues/issue/");
        assert_eq!(
            consumed.len(),
            2,
            "the boxed `consume` forward returns the inner bus's 2 envelopes (not vec![])"
        );

        // ack THROUGH a fresh boxed handle reaches the inner bus's high-water.
        let boxed2: Box<dyn BusTransport> = Box::new(inner.clone());
        boxed2.ack("indexer", &ids[1]);
        assert_eq!(
            inner.ack_of("indexer").as_ref(),
            Some(&ids[1]),
            "the boxed `ack` forward landed on the inner bus"
        );

        // purge THROUGH the boxed handle clears the inner delivered/dedup state.
        boxed2.purge();
        assert_eq!(
            inner.delivered_count(),
            0,
            "the boxed `purge` forward cleared the inner bus"
        );
    }

    /// **EB-04 — the BusTransport put/consume CDC conformance pair.** The frozen seam contract:
    /// a `put(subject, envelope, dedup_id)` is observable by a `consume(subject_prefix)` that
    /// returns the SAME wire envelope, in publish order, exactly once per distinct `dedup_id`
    /// (the `Nats-Msg-Id = event_id` broker dedup). This is the conformance any BusTransport impl
    /// (the in-process fake here; EB-04's JetStream-class adapter later) MUST satisfy — the
    /// provider (put) + consumer (consume) pair the contract-coverage scanner reads.
    #[test]
    fn cdc_bustransport_put_consume_conformance() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 2, "issue:PROJ-1");
        let r0 = store.row(&ids[0]).unwrap();
        let r1 = store.row(&ids[1]).unwrap();

        let bus = InProcessBus::new();
        // PROVIDER side: put two distinct events, then re-put the first (a redelivery).
        assert_eq!(
            bus.put(&r0.subject, &r0.envelope, &r0.event_id).unwrap(),
            Delivery::Accepted
        );
        assert_eq!(
            bus.put(&r1.subject, &r1.envelope, &r1.event_id).unwrap(),
            Delivery::Accepted
        );
        assert_eq!(
            bus.put(&r0.subject, &r0.envelope, &r0.event_id).unwrap(),
            Delivery::Deduplicated,
            "the same dedup_id is suppressed (Nats-Msg-Id broker dedup → 0 ghost)"
        );

        // CONSUMER side: consume returns exactly the put envelopes, in publish order, once each.
        let consumed = bus.consume("myelin://acme/issues/issue/");
        assert_eq!(
            consumed.len(),
            2,
            "exactly two distinct events delivered (dedup suppressed the 3rd put)"
        );
        assert_eq!(
            consumed[0], r0.envelope,
            "consumer sees the provider's wire envelope #0"
        );
        assert_eq!(
            consumed[1], r1.envelope,
            "consumer sees the provider's wire envelope #1, in order"
        );
    }

    /// **EB-04 dated GATE re-confirm — SUB-D1 / BUS-D4 stay 0 ghost / 0 lost AFTER the relay
    /// refinements (DLQ alert + GC) land.** Kill between commit and publish, heal, drain → every
    /// committed event delivered exactly once; THEN run GC over the now-published rows → still
    /// exactly the committed set delivered, depth 0, 0 dead-letters. Proves the EB-04 additions
    /// did not regress the silent-data-loss floor.
    #[test]
    fn eb04_sub_d1_bus_d4_reconfirm_zero_ghost_zero_lost_after_refinements() {
        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 5, "issue:PROJ-1");
        let committed: HashSet<EventId> = ids.iter().cloned().collect();

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);

        // kill between commit and publish (SUB-D1 / BUS-D4 fault): severed → 0 delivered, 0 lost.
        bus.sever();
        relay.drain_to_empty();
        assert_eq!(
            store.outbox_depth(),
            5,
            "0 lost while severed (events parked)"
        );
        assert_eq!(bus.delivered_count(), 0, "0 ghost while severed");
        assert!(
            relay.dead_letter_alerts().is_empty(),
            "no premature dead-letter"
        );

        // heal + drain: exactly-once delivery (0 ghost, 0 lost), depth → 0.
        bus.heal();
        relay.drain_to_empty();
        assert_eq!(store.outbox_depth(), 0, "outbox-depth drains (SUB-D1)");
        assert_eq!(
            store.dead_letter_count(),
            0,
            "0 dead-letters on the no-loss path"
        );
        assert_eq!(
            bus.delivered_count(),
            5,
            "exactly-once: 5 committed → 5 delivered"
        );
        assert_eq!(
            bus.delivered_ids(),
            committed,
            "delivered set == committed set (0 ghost/0 lost)"
        );

        // EB-04 GC runs over the published rows: the delivered set is unchanged (GC frees the
        // retention window, never an undelivered event).
        let reaped = relay.gc_published(&Timestamp("2099-01-01T00:00:00Z".into()));
        assert_eq!(reaped, 5, "all 5 published rows aged out of the 24h window");
        assert_eq!(store.outbox_depth(), 0, "still 0 unsent after GC");
        assert_eq!(
            bus.delivered_count(),
            5,
            "GC did not lose or duplicate any delivery"
        );
        assert_eq!(
            bus.delivered_ids(),
            committed,
            "delivered set STILL == committed set after GC"
        );
        println!("[2026-06-19] PASS  gate=EB-04  SUB-D1/BUS-D4 reconfirm  ghost=0 lost=0  (after DLQ-alert + 24h-GC refinements)");
    }
}
