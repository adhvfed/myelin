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
//! ## DEVIATION / FLOOR — modeled claim, not a real `SELECT … FOR UPDATE SKIP LOCKED`
//! There is no live OLTP DB in M0 (P-007 / the `serve` pool P-S12). The relay's claim is
//! modeled as an **atomic claim over the in-memory [`crate::outbox::OutboxStore`]**: a claimed
//! row is marked so a second relay worker SKIPs it (the observable `FOR UPDATE SKIP LOCKED`
//! property — no double-claim across replicas), and the claim is released on a failed attempt
//! so the row is retried. The real SQL `SELECT … FOR UPDATE SKIP LOCKED` against the Storage
//! pool lands when the OLTP tier client is wired (P-007 + `serve` P-S12); the relay's
//! algorithm + the drilled property do not change. The 24h published-row GC + the
//! `dlq.<tenant>.<subsystem>` subject naming + the Signal alert on dead-letter are EB-04's
//! refinement of this floor (named, not silently skipped).

use crate::outbox::{OutboxRow, OutboxStore};
use crate::{ArtifactRef, EventEnvelope, EventId, Timestamp};
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
        self.lock().fail_with = Some(TransportError("broker unreachable (severed by drill)".into()));
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
        self.lock().acks.insert(consumer.to_string(), event_id.clone());
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
}

/// The outbox relay (contract 2.3 relay half) — stateless, horizontally-replicable. Holds the
/// [`OutboxStore`] it drains and the [`BusTransport`] it publishes to (both cloneable handles),
/// plus a `clock` for the `published_at` stamp.
pub struct Relay<T: BusTransport> {
    store: OutboxStore,
    transport: T,
    /// The `published_at` stamp source (a function so it is deterministic in tests; the real
    /// clock is wired at `serve` P-S12).
    clock: Box<dyn Fn() -> Timestamp + Send + Sync>,
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
        }
    }

    /// The transport this relay publishes to (so a drill can sever/heal the in-process broker).
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// **Drain pass: claim → publish → mark sent / dead-letter.** Claims every currently-unsent,
    /// unclaimed row with the `FOR UPDATE SKIP LOCKED` discipline (a row another worker holds is
    /// skipped), publishes each via `put(subject, envelope, dedup_id = event_id)` in
    /// `(aggregate, seq)` order, marks the row sent on `Accepted`/`Deduplicated`, and
    /// dead-letters a row that has failed [`MAX_PUBLISH_ATTEMPTS`] times. Returns a
    /// [`DrainReport`]. Idempotent: re-running after a crash mid-publish re-claims the unsent
    /// rows and the broker dedup suppresses any double-delivery (0 ghost).
    pub fn drain_once(&self) -> DrainReport {
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
                        report.dead_lettered += 1;
                    } else {
                        report.failed += 1;
                    }
                }
            }
        }
        report
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
            let made_progress = r.published > 0 || r.deduplicated > 0 || r.dead_lettered > 0;
            total.published += r.published;
            total.deduplicated += r.deduplicated;
            total.failed += r.failed;
            total.dead_lettered += r.dead_lettered;
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
// are defined here against the store's claim API. ---

impl OutboxStore {
    /// Claim every currently-unsent, unclaimed row (the `FOR UPDATE SKIP LOCKED` batch), ordered
    /// `(aggregate, seq)`. A claimed row is marked so a SECOND relay worker's claim SKIPs it (no
    /// double-claim across replicas). Returns the claimed rows for the relay to publish.
    pub(crate) fn claim_unsent(&self) -> Vec<OutboxRow> {
        let mut inner = self.lock_inner();
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
        let mut inner = self.lock_inner();
        if let Some(row) = inner.rows.get_mut(id) {
            row.published_at = Some(at);
        }
        inner.claimed.remove(id);
    }

    /// Record a failed publish attempt (broker unreachable): bump `attempts`, release the claim
    /// so the row is re-claimable next pass. Returns the new attempt count.
    pub(crate) fn fail_attempt(&self, id: &EventId) -> u32 {
        let mut inner = self.lock_inner();
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
    /// dead-letter list (the operator alert + the `dlq.<tenant>.<subsystem>` subject is EB-04's
    /// refinement). It is no longer in `outbox_depth` (it is not lost — it is quarantined,
    /// visibly, in `dead_letter_count`).
    pub(crate) fn dead_letter(&self, id: &EventId) {
        let mut inner = self.lock_inner();
        if let Some(row) = inner.rows.remove(id) {
            inner.order.retain(|x| x != id);
            inner.dead_letters.push(row);
        }
        inner.claimed.remove(id);
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
        Principal {
            id: PrincipalId("p".into()),
            kind: PrincipalKind::Human,
            tenant: TenantId("acme".into()),
        }
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

    fn commit_n(store: &OutboxStore, minter: Arc<dyn IdMinter>, n: usize, aggregate: &str) -> Vec<EventId> {
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("state");
        let mut ids = Vec::new();
        for i in 0..n {
            ids.push(tx.emit(draft(&format!("issues.issue.e{i}"), aggregate), None).unwrap());
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
        assert_eq!(store.outbox_depth(), 4, "severed broker → events parked, not lost");
        assert_eq!(bus.delivered_count(), 0, "nothing delivered while severed");

        // (2) heal + drain: every event is delivered exactly once (0 ghost), depth → 0.
        bus.heal();
        let report = relay.drain_to_empty();
        assert_eq!(report.published, 4);
        assert_eq!(store.outbox_depth(), 0, "outbox-depth drains after heal (SUB-D1)");
        assert_eq!(bus.delivered_count(), 4, "exactly-once: 4 committed → 4 delivered");
        assert_eq!(bus.delivered_ids(), ids.into_iter().collect(), "0 ghost / 0 lost");
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
        assert_eq!(store.outbox_depth(), 1, "row still unsent (crash before mark)");

        // The relay re-runs: it re-claims the unsent row and re-publishes — the broker dedups it.
        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let report = relay.drain_once();
        assert_eq!(report.deduplicated, 1, "the re-claim was deduplicated (0 ghost)");
        assert_eq!(report.published, 0);
        assert_eq!(bus.delivered_count(), 1, "still exactly one delivery (no ghost)");
        assert_eq!(store.outbox_depth(), 0, "the row is marked sent after the dedup");
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
        assert_eq!(store.dead_letter_count(), 1, "it is dead-lettered, not silently lost");
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
        assert_eq!(store.outbox_depth(), 2, "only the dead row left the live set");
        assert!(store.row(&ids[1]).is_none(), "the dead row is gone from live rows");
        assert!(store.row(&ids[0]).is_some(), "survivor 0 still live");
        assert!(store.row(&ids[2]).is_some(), "survivor 2 still live");

        // the two survivors still drain to the broker exactly once.
        let bus = InProcessBus::new();
        Relay::new(store.clone(), bus.clone(), clock).drain_to_empty();
        assert_eq!(bus.delivered_count(), 2);
        let delivered = bus.delivered_ids();
        assert!(delivered.contains(&ids[0]) && delivered.contains(&ids[2]));
        assert!(!delivered.contains(&ids[1]), "the dead-lettered row is not delivered");
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
        assert_eq!(r2, DrainReport::default(), "an empty drain pass reports all-zero");
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
        assert_eq!(dl[0].attempts, MAX_PUBLISH_ATTEMPTS, "attempts hit the bound");
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
        assert_eq!(total.published, 5, "published accumulates across passes (3 + 2)");
        assert_eq!(total.failed, 2, "the 2 transient failures are reported");
        assert_eq!(store.outbox_depth(), 0, "the outbox fully drained over 2 passes");
        assert_eq!(bus.delivered_count(), 5, "exactly 5 delivered, none ghosted/lost");
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
        assert_eq!(store.outbox_depth(), 3, "rows delivered but not yet marked sent");

        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let total = relay.drain_to_empty();
        assert_eq!(total.deduplicated, 3, "all 3 re-claims were deduplicated (0 ghost)");
        assert_eq!(total.published, 0, "nothing newly published (already delivered)");
        assert_eq!(store.outbox_depth(), 0, "depth drains via the dedup path");
        assert_eq!(bus.delivered_count(), 3, "still exactly 3 delivered (no double-delivery)");
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
        assert_eq!(store.outbox_depth(), 3, "0 lost: the rows stay parked while severed");
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
        assert_eq!(bus.ack_of("indexer").as_ref(), Some(&ids[1]), "ack recorded the high-water");
        assert_eq!(bus.ack_of("nobody"), None, "an un-acked consumer reads None");
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
        assert_eq!(consumed[0], provider_row.envelope, "consumer sees the provider's wire shape");

        // ack is part of the frozen shape.
        bus.ack("indexer", &ids[1]);
    }
}
