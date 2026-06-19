//! # The idempotent event-consumer runtime + the `consumer_dedup` ledger (SUB-D2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §5 (the
//! event-consumer template + the **seven encoded rules**) and §5.3 (causality through the
//! consumer — `emit(draft, cause = Some(incoming))`).
//!
//! **Contract-index:** rows 2.4 (`EventHandler` template + `HandleOutcome`) and 2.5
//! (`consumer_dedup` ledger `(consumer, event_id)` PK). **P-S08 → global P-009.** This is the
//! consumer half of the **silent-data-loss floor** (SUB-D2): a **PERMANENT gate**, re-run on
//! every emit-path change.
//!
//! ## What this module ships (the one template the whole platform is built from)
//! The [`EventHandler`](crate::EventHandler) trait (the body a subsystem writes — frozen at
//! [`crate`]) is wrapped by ONE runtime, [`Consumer`], so the seven correctness rules cannot be
//! skipped per-consumer:
//!
//! 1. **Idempotent on `event_id`** via the per-consumer [`DedupLedger`] (`(consumer, event_id)`
//!    PK, contract 2.5) — at-least-once delivery + an idempotent handler ≈ effectively-once.
//!    A redelivered `event_id` is a no-op: the runtime SKIPs the handler and acks
//!    ([`Consumer::deliver`] → [`Delivered::Deduplicated`]). Myelin does **not** chase true
//!    exactly-once.
//! 2. **Ack-after-enqueue, never before** — the runtime acks a message ONLY after the handler
//!    returns [`HandleOutcome::Done`] (or the dedup ledger absorbed it / it dead-lettered);
//!    a `Retry` does NOT ack, so a dropped broker redelivers it (0 lost). The cursor only
//!    advances on a terminal outcome.
//! 3. **Whitelist subjects, never `*`** — [`Subscription::bind`] REJECTS a `*` (or empty)
//!    subject at registration ([`SubscribeError::WildcardSubject`]); an over-broad subscription
//!    head-of-line-blocks everything (BUS-3, D7-i), so it is unconstructable.
//! 4. **Bind durable-by-name** — [`Subscription`] carries a stable [`ConsumerName`]; on a
//!    reconnect the runtime re-binds the SAME name (re-using the SAME dedup ledger + cursor)
//!    rather than re-declaring a fresh start policy (the single most operationally expensive
//!    JetStream/Kafka mistake). This is what makes "drop broker mid-stream → 0 lost across
//!    reconnect" hold.
//! 5. **Terminate poison immediately** — a [`HandleOutcome::NonRetryable`] dead-letters the
//!    message at once ([`Consumer::dead_letters`]); it does NOT burn the redelivery budget and
//!    does NOT block the subject behind it.
//! 6. **Bounded prefetch** — [`Consumer`] pulls at most [`PrefetchBound`] in-flight messages per
//!    drain; a slow subject cannot monopolise the loop. The bound is observable so a drill can
//!    assert a slow subject did not head-of-line-block a fast one.
//! 7. **Consumer lag as a first-class metric** — [`Consumer::lag`] reports `num_pending` (the
//!    un-acked backlog) so the contract-1.8 `consumer_lag` signal reads it; a drill asserts it
//!    recovers to 0 after a reconnect.
//!
//! ## §5.3 — causality through the consumer
//! A reaction a handler wants to emit calls `OutboxTx::emit(draft, cause = Some(incoming))` so
//! the depth ceiling / shared-root tripwire (AG-6) read a correct `causation_id`/`depth`
//! structurally. The runtime hands the handler the incoming [`EventEnvelope`] so the `cause` is
//! always available; the reactive/dispatch tier that consumes this is the Bus's separate design
//! (§5.4, ADR-19) — named, not built here.
//!
//! ## How SUB-D2 is proven (the consumer half of the silent-data-loss floor)
//! - **Drop broker mid-stream → 0 lost across reconnect:** the cursor advances only on a
//!   terminal ack (rule 2), and re-binding by name (rule 4) re-uses the cursor + the dedup
//!   ledger, so a reconnect resumes from the last ack and the ledger absorbs any redelivery →
//!   **0 lost, 0 dup**. The `drills_sub_d2_consumer.rs` integration drill rides the P-S03
//!   injector (`Dependency::Broker`) + reads the P-S04 `consumer_lag` signal.
//! - **A slow subject does NOT head-of-line-block others:** bounded prefetch (rule 6) + the
//!   per-subject lanes ([`Consumer::deliver_lane`]) mean a poison / slow message on one subject
//!   dead-letters or parks without stalling a fast subject. The drill asserts no HoL stall.
//! - **Re-confirm SUB-D1 end-to-end through a consumer:** the dedup ledger absorbs the relay's
//!   redelivery on a re-claim → **0 dup** even when SUB-D1's at-least-once redelivery fires.
//!
//! ## DEVIATION / FLOOR — the in-memory ledger models the SQL `consumer_dedup` table
//! There is **no live OLTP DB in M0** (the OLTP tier client is **P-007 / P-ST-01**; the
//! migration runner is **P-S15**). So the `consumer_dedup` ledger is modeled as an **in-memory
//! [`DedupLedger`]** whose semantics are byte-for-byte the 2.5 contract: `(consumer, event_id)`
//! is the PK (a second insert of the same pair is the no-op idempotency check), per-consumer so
//! two consumers of the same event each process it once. The frozen DDL
//! ([`CONSUMER_DEDUP_MIGRATION`]) is the shape the runner applies. **Floor:** the real
//! `INSERT … ON CONFLICT DO NOTHING` against the Storage pool, executed in the SAME transaction
//! as the handler's state write (so the dedup mark and the side effect commit together — the
//! atomicity that makes idempotency real, not best-effort), lands when the OLTP client is wired
//! (P-007) + the consumer runtime runs inside `serve` (**P-S12**). The seam shape (the
//! `EventHandler` trait, the `(consumer, event_id)` key, the lag signal) does NOT change. The
//! durable broker cursor + the real `dlq.<tenant>.<subsystem>` subject is EB-04's / EB-05's
//! refinement of this floor (named, not silently skipped).
//!
//! ## FLOOR — the upcaster runs BEFORE handle
//! Per the architecture, the consumer runtime calls the `(type, from_ver) → to_ver` upcaster
//! registry before `handle` so a handler always sees the current shape. That registry is
//! **P-S09** (the next prompt). The [`Consumer`] exposes the pre-handle hook
//! ([`Consumer::with_upcaster`]) the registry plugs into; until P-S09 lands it is the identity
//! map (a `schema_ver` already at the current version passes through). Named, not silently
//! assumed done.

use crate::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// The frozen forward-only DDL for the `consumer_dedup` ledger (contract 2.5). This is the
/// shape the migration runner (P-S15) applies when the OLTP tier client (P-007) is wired; the
/// in-memory [`DedupLedger`] models exactly these semantics until then.
///
/// - `(consumer, event_id)` is the **PRIMARY KEY** — the idempotency key is per-consumer, so
///   two distinct consumers of the same event each process it exactly once, and a redelivery to
///   the SAME consumer is suppressed (`ON CONFLICT DO NOTHING`);
/// - `recorded_at` is when the consumer durably marked the event handled (read in the SAME
///   transaction as the handler's state write — the atomicity floor named in the module docs).
///
/// **Forward-only** (the `forward-only-migration` lint, P-S11): this is an `expand` migration
/// (it only adds the table); there is no destructive down-migration.
pub const CONSUMER_DEDUP_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS consumer_dedup (
    consumer    TEXT        NOT NULL,
    event_id    TEXT        NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT consumer_dedup_pk PRIMARY KEY (consumer, event_id)
);";

/// The stable, durable consumer name (rule 4: bind-by-name). Two `Consumer`s with the SAME name
/// share the SAME dedup ledger key-space + cursor — a reconnect re-binds this name rather than
/// declaring a fresh start policy. PII-free identifier (a subsystem/consumer label), never a
/// payload — a telemetry/trace label by construction.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConsumerName(pub String);

/// The bounded prefetch (rule 6): the maximum number of in-flight (claimed-but-not-yet-acked)
/// messages the runtime pulls per drain. A slow subject cannot monopolise the loop because the
/// runtime never holds more than this many at once. The floor default is 64 (EB-05 may tune it
/// per consumer); a value of 0 is meaningless (it would consume nothing), so the constructor
/// rejects it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PrefetchBound(u32);

impl PrefetchBound {
    /// The floor default bounded prefetch.
    pub const DEFAULT: PrefetchBound = PrefetchBound(64);

    /// A bound of `n` in-flight messages, or `None` if `n == 0` (a zero prefetch would consume
    /// nothing — meaningless, rejected rather than silently treated as unbounded).
    pub fn new(n: u32) -> Option<PrefetchBound> {
        if n == 0 {
            None
        } else {
            Some(PrefetchBound(n))
        }
    }

    /// The numeric bound.
    pub fn get(self) -> u32 {
        self.0
    }
}

impl Default for PrefetchBound {
    fn default() -> Self {
        PrefetchBound::DEFAULT
    }
}

/// Why a subscription was rejected at registration (rule 3: never `*`). A rejected subscription
/// is a LOUD error, never a silently-narrowed pass — an over-broad subscription that head-of-line
/// blocks the whole consumer must be impossible to construct (BUS-3, D7-i).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubscribeError {
    /// A subject was the wildcard `*` (or `>`, or contained a bare `*` segment, or was empty) —
    /// an over-broad subscription. Rejected: subjects are a whitelist, never `*`.
    WildcardSubject(String),
    /// The subscription named no subjects at all (a consumer must whitelist at least one).
    NoSubjects,
}

/// A registered consumer subscription (rule 3 + rule 4): a durable [`ConsumerName`] + a frozen
/// **whitelist** of subject patterns that is guaranteed `*`-free (the constructor rejects a
/// wildcard, so a `Subscription` value witnesses the whitelist invariant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    name: ConsumerName,
    subjects: Vec<SubjectPattern>,
    prefetch: PrefetchBound,
}

impl Subscription {
    /// Register a subscription, REJECTING a `*`/`>`/empty subject (rule 3). The returned value
    /// witnesses the `*`-free whitelist invariant: there is no other constructor, so a
    /// `Subscription` cannot carry a wildcard. Binds the durable `name` (rule 4) and the bounded
    /// `prefetch` (rule 6).
    pub fn bind(
        name: ConsumerName,
        subjects: &[&str],
        prefetch: PrefetchBound,
    ) -> Result<Subscription, SubscribeError> {
        if subjects.is_empty() {
            return Err(SubscribeError::NoSubjects);
        }
        for s in subjects {
            if is_wildcard_subject(s) {
                return Err(SubscribeError::WildcardSubject((*s).to_string()));
            }
        }
        Ok(Subscription {
            name,
            subjects: subjects.iter().map(|s| SubjectPattern((*s).to_string())).collect(),
            prefetch,
        })
    }

    /// The durable consumer name (rule 4: re-bound on reconnect).
    pub fn name(&self) -> &ConsumerName {
        &self.name
    }

    /// The `*`-free whitelist of subject patterns (rule 3).
    pub fn subjects(&self) -> &[SubjectPattern] {
        &self.subjects
    }

    /// The bounded prefetch (rule 6).
    pub fn prefetch(&self) -> PrefetchBound {
        self.prefetch
    }

    /// Does this subscription's whitelist match `subject`? A subject pattern is a prefix the
    /// event subject must start with (the same prefix model the broker fake's `consume` uses);
    /// `*` is unrepresentable here by construction so this is never an over-broad match.
    pub fn matches(&self, subject: &str) -> bool {
        self.subjects.iter().any(|p| subject.starts_with(&p.0))
    }
}

/// `true` iff `s` is a wildcard / over-broad / empty subject (rule 3 rejects it). A `*` or `>`
/// anywhere as a whole segment is the JetStream/NATS wildcard; an empty subject matches
/// everything. We reject any of these LOUDLY rather than silently narrowing.
fn is_wildcard_subject(s: &str) -> bool {
    // An empty subject matches everything (over-broad). Otherwise: any segment that is exactly
    // `*` or `>` (the NATS subject wildcards) is over-broad — this covers a bare `*`/`>` (a
    // single segment), an interior `issues.*.created`, and a trailing greedy `issues.>`.
    s.is_empty() || s.split('.').any(|seg| seg == "*" || seg == ">")
}

/// The per-consumer `consumer_dedup` ledger (contract 2.5, the in-memory model). `(consumer,
/// event_id)` is the PK: [`DedupLedger::mark_handled`] records the pair and returns whether it
/// was FRESH (newly inserted) or a DUPLICATE (already present — the idempotency no-op). A
/// cloneable handle over shared state so a reconnected `Consumer` re-bound by the same name
/// re-uses the SAME ledger (rule 4) and the redelivery is absorbed.
#[derive(Clone, Default)]
pub struct DedupLedger {
    inner: Arc<Mutex<HashSet<(ConsumerName, crate::EventId)>>>,
}

impl DedupLedger {
    /// A fresh, empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `(consumer, event_id)` and report whether it was FRESH. This is the `INSERT …
    /// ON CONFLICT DO NOTHING` model: a fresh pair returns `true` (the handler should run); a
    /// pair already present returns `false` (a redelivery — the handler is SKIPped, the message
    /// is acked, 0 dup). The PK is the pair, so the SAME event delivered to two DIFFERENT
    /// consumers is fresh for each.
    pub fn mark_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool {
        let mut set = self.lock();
        set.insert((consumer.clone(), event_id.clone()))
    }

    /// Has `(consumer, event_id)` already been handled? (Read-only check; `mark_handled` is the
    /// transactional one.)
    pub fn is_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool {
        self.lock().contains(&(consumer.clone(), event_id.clone()))
    }

    /// How many `(consumer, event_id)` pairs the ledger holds (for tests / a depth read).
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<(ConsumerName, crate::EventId)>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The outcome of the runtime delivering ONE message to a handler (distinct from the handler's
/// own [`HandleOutcome`]: this is what the RUNTIME did after applying the seven rules).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivered {
    /// The handler ran and returned `Done`; the message was acked (the cursor advanced).
    Acked,
    /// The `event_id` was already in the dedup ledger (a redelivery): the handler was SKIPped
    /// and the message acked — 0 dup. This is the idempotency path (rule 1).
    Deduplicated,
    /// The handler returned `NonRetryable` (poison): the message was dead-lettered immediately
    /// (rule 5) and acked so it does NOT redeliver / block the subject. Carries the reason.
    DeadLettered(Reason),
    /// The handler returned `Retry`: the message was NOT acked (rule 2) — it stays pending and
    /// redelivers. Carries the backoff seconds the handler asked for.
    Retried(u64),
}

/// A message the runtime is about to deliver: the durable subject + the canonical envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// The broker subject the message arrived on (whitelisted by the subscription).
    pub subject: String,
    /// The canonical event body (what the handler reads + the `event_id` the ledger dedups on).
    pub envelope: EventEnvelope,
}

/// The pre-handle hook the upcaster registry (P-S09) plugs into: `(envelope) -> envelope`,
/// applied before `handle` so a handler always sees the current `schema_ver` shape. Until P-S09
/// lands it is the identity map ([`Consumer::with_upcaster`] is how P-S09 will install the real
/// registry). Boxed so the registry can be swapped without changing the runtime.
type Upcaster = Box<dyn Fn(EventEnvelope) -> EventEnvelope + Send + Sync>;

/// A dead-lettered message (rule 5): the envelope + the non-retryable reason it was poisoned for.
/// Surfaced (the operator alert / `dlq.<tenant>.<subsystem>` subject is EB-04/EB-05's
/// refinement), never silently dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadLetter {
    pub envelope: EventEnvelope,
    pub reason: Reason,
}

/// **The one consumer runtime — the seven encoded rules wrapped around an [`EventHandler`].**
/// Built from the frozen [`crate::EventHandler`] trait so no consumer can skip a rule. Holds the
/// durable [`Subscription`] (name + `*`-free whitelist + bounded prefetch), the per-consumer
/// [`DedupLedger`] (rule 1, cloneable so a reconnect re-binds it), the pre-handle upcaster hook
/// (P-S09), a per-subject pending-lag tally (rule 7), and the dead-letter list (rule 5).
pub struct Consumer<H: EventHandler> {
    handler: H,
    subscription: Subscription,
    dedup: DedupLedger,
    upcaster: Upcaster,
    /// `num_pending` per subject (rule 7: consumer lag). A message that retries (not acked)
    /// counts toward lag; an acked/deduped/dead-lettered message does not.
    pending: Mutex<HashMap<String, u64>>,
    /// Dead-lettered (poison) messages (rule 5), surfaced.
    dead_letters: Mutex<Vec<DeadLetter>>,
}

impl<H: EventHandler> Consumer<H> {
    /// Build the runtime for `handler` bound to `subscription`, sharing `dedup` (rule 1+4: a
    /// reconnect passes the SAME ledger so the redelivery is absorbed). The upcaster is the
    /// identity map until P-S09 installs the registry via [`Consumer::with_upcaster`].
    pub fn new(handler: H, subscription: Subscription, dedup: DedupLedger) -> Self {
        Consumer {
            handler,
            subscription,
            dedup,
            upcaster: Box::new(|e| e),
            pending: Mutex::new(HashMap::new()),
            dead_letters: Mutex::new(Vec::new()),
        }
    }

    /// Install the pre-handle upcaster (P-S09 plugs the `(type, from_ver) → to_ver` registry in
    /// here). The runtime applies it before `handle` so a handler always sees the current shape.
    pub fn with_upcaster(
        mut self,
        upcaster: impl Fn(EventEnvelope) -> EventEnvelope + Send + Sync + 'static,
    ) -> Self {
        self.upcaster = Box::new(upcaster);
        self
    }

    /// The durable consumer name (rule 4).
    pub fn name(&self) -> &ConsumerName {
        self.subscription.name()
    }

    /// The shared dedup ledger (so a reconnect can re-bind it).
    pub fn dedup(&self) -> &DedupLedger {
        &self.dedup
    }

    /// The wrapped handler (so a drill can read what the consumer's body observed — e.g. how
    /// many times it actually ran, to prove dedup-skips happened).
    pub fn handler(&self) -> &H {
        &self.handler
    }

    /// **Consumer lag (rule 7): `num_pending`** — the total un-acked backlog across all subjects.
    /// The contract-1.8 `consumer_lag` signal reads this; a drill asserts it recovers to 0 after
    /// a reconnect.
    pub fn lag(&self) -> u64 {
        self.pending().values().copied().sum()
    }

    /// The un-acked backlog on ONE subject (rule 6+7: a slow subject's lag does not stall a fast
    /// one — the drill reads per-subject lag to assert no head-of-line block).
    pub fn lag_on(&self, subject: &str) -> u64 {
        self.pending().get(subject).copied().unwrap_or(0)
    }

    /// The dead-lettered (poison) messages so far (rule 5), surfaced.
    pub fn dead_letters(&self) -> Vec<DeadLetter> {
        self.dead_letters.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// **Deliver ONE message through the seven rules.** Applies, in order:
    /// rule 3 (the subject must be on the whitelist — a non-matching subject is a programming
    /// error and is rejected, never silently processed); the upcaster (P-S09); rule 1 (dedup —
    /// a re-seen `event_id` SKIPs the handler and acks → [`Delivered::Deduplicated`]); the
    /// handler; then the terminal-outcome dispatch — rule 2 (ack only on `Done`), rule 5
    /// (`NonRetryable` → dead-letter immediately), and `Retry` (NOT acked → stays pending, lag
    /// rises). Returns what the runtime DID.
    pub fn deliver(&self, msg: &Message) -> Delivered {
        // Rule 3: the subject MUST be on this consumer's `*`-free whitelist. (A `Subscription`
        // cannot carry a `*`, so this is a real whitelist match — never over-broad.)
        if !self.subscription.matches(&msg.subject) {
            // A message off-whitelist should never have been routed here; treat it as poison so
            // it is surfaced, not silently swallowed.
            let reason = Reason(format!("subject {} not on consumer whitelist", msg.subject));
            self.push_dead_letter(msg.envelope.clone(), reason.clone());
            return Delivered::DeadLettered(reason);
        }

        // The upcaster (P-S09) runs BEFORE handle so the handler sees the current shape.
        let envelope = (self.upcaster)(msg.envelope.clone());
        let event_id = envelope.event_id.clone();

        // Rule 1: idempotent on `event_id` via the (consumer, event_id) dedup ledger. A FRESH
        // pair runs the handler; a DUPLICATE (redelivery) SKIPs it and acks — 0 dup.
        let fresh = self.dedup.mark_handled(self.name(), &event_id);
        if !fresh {
            // Already handled by this consumer: skip + ack (the cursor advances; lag clears).
            self.clear_pending(&msg.subject);
            return Delivered::Deduplicated;
        }

        // Run the handler (the consumer's body). A reaction it emits calls
        // `OutboxTx::emit(draft, cause = Some(&envelope))` (§5.3) — provided by the caller's tx.
        match self.handler.handle(&envelope) {
            HandleOutcome::Done => {
                // Rule 2: ack AFTER the handler succeeded — the cursor advances, lag clears.
                self.clear_pending(&msg.subject);
                Delivered::Acked
            }
            HandleOutcome::NonRetryable(reason) => {
                // Rule 5: poison terminates immediately — dead-letter + ack so it does NOT burn
                // the redelivery budget and does NOT block the subject behind it.
                //
                // The dedup mark stays (this event_id is terminal for this consumer): a
                // redelivery of a dead-lettered message is itself deduplicated, never re-poisons.
                self.clear_pending(&msg.subject);
                self.push_dead_letter(envelope, reason.clone());
                Delivered::DeadLettered(reason)
            }
            HandleOutcome::Retry(backoff) => {
                // NOT acked (rule 2): the message stays pending → it redelivers, and lag rises.
                // The dedup mark must be REVERTED — a retry is NOT a completed handle, so a
                // later redelivery must run the handler again (else a transient failure would be
                // permanently swallowed: silent data loss).
                self.dedup_revert(&event_id);
                self.bump_pending(&msg.subject);
                Delivered::Retried(backoff.seconds)
            }
        }
    }

    /// **Deliver a whole prefetch lane for ONE subject (rule 6: bounded prefetch).** Pulls at
    /// most [`Subscription::prefetch`] messages from `lane` and delivers each, returning the
    /// per-message outcomes. A slow / poison message on this subject does NOT consume another
    /// subject's budget — each subject is drained independently, so a slow subject cannot
    /// head-of-line-block a fast one. Messages beyond the prefetch bound are left for the next
    /// drain (they stay pending — lag, not loss).
    pub fn deliver_lane(&self, subject: &str, lane: &[Message]) -> Vec<Delivered> {
        let bound = self.subscription.prefetch().get() as usize;
        lane.iter()
            .take(bound)
            .map(|m| {
                debug_assert_eq!(m.subject, subject, "a lane carries one subject's messages");
                self.deliver(m)
            })
            .collect()
    }

    fn push_dead_letter(&self, envelope: EventEnvelope, reason: Reason) {
        self.dead_letters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(DeadLetter { envelope, reason });
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump_pending(&self, subject: &str) {
        *self.pending().entry(subject.to_string()).or_insert(0) += 1;
    }

    fn clear_pending(&self, subject: &str) {
        if let Some(p) = self.pending().get_mut(subject) {
            *p = p.saturating_sub(1);
        }
    }

    /// Revert a dedup mark (a `Retry` is not a completed handle — the pair must be removed so a
    /// redelivery re-runs the handler). Crate-internal mechanic; the real `consumer_dedup` is
    /// written in the SAME transaction as the handler's state write (P-007/P-S12), so a rolled-
    /// back handler rolls back its dedup mark for free — this models that atomicity.
    fn dedup_revert(&self, event_id: &crate::EventId) {
        let mut set = self.dedup.lock();
        set.remove(&(self.name().clone(), event_id.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Actor, AggregateKey, ArtifactRef, Backoff, CausedBy, CorrelationId, DataRole, EventId,
        EventType, PiiKeyRef, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn principal() -> Principal {
        Principal {
            id: PrincipalId("p".into()),
            kind: PrincipalKind::Human,
            tenant: TenantId("acme".into()),
        }
    }

    fn envelope(id: &str, subject: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType("issues.issue.created".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(principal()),
            subject: ArtifactRef(subject.into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: Some(CausedBy("session:abc".into())),
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None::<PiiKeyRef>,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            payload: serde_json::json!({ "ref": "x" }),
        }
    }

    fn msg(id: &str, subject: &str) -> Message {
        Message {
            subject: subject.into(),
            envelope: envelope(id, subject),
        }
    }

    /// A handler that COUNTS how many times it actually ran (so dedup-skips are observable) and
    /// returns a scripted outcome per call.
    struct CountingHandler {
        runs: AtomicU32,
        subjects: &'static [SubjectPattern],
        outcome: fn(&EventEnvelope) -> HandleOutcome,
    }
    impl EventHandler for CountingHandler {
        fn subjects(&self) -> &'static [SubjectPattern] {
            self.subjects
        }
        fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
            self.runs.fetch_add(1, Ordering::SeqCst);
            (self.outcome)(ev)
        }
    }

    static SUBJECTS: &[SubjectPattern] = &[];

    fn done_handler() -> CountingHandler {
        CountingHandler { runs: AtomicU32::new(0), subjects: SUBJECTS, outcome: |_| HandleOutcome::Done }
    }

    fn sub(name: &str, subjects: &[&str]) -> Subscription {
        Subscription::bind(ConsumerName(name.into()), subjects, PrefetchBound::DEFAULT).unwrap()
    }

    // --- Rule 3: whitelist subjects, never `*` ---

    /// A `*` subscription is REJECTED at registration (rule 3) — and so are `>`, a `*` segment,
    /// and an empty subject. An over-broad subscription is unconstructable.
    #[test]
    fn wildcard_subscription_is_rejected_at_registration() {
        let name = ConsumerName("indexer".into());
        for bad in ["*", ">", "issues.*", "issues.>", "issues.*.created", ""] {
            let r = Subscription::bind(name.clone(), &[bad], PrefetchBound::DEFAULT);
            assert!(matches!(r, Err(SubscribeError::WildcardSubject(_))), "`{bad}` must be rejected");
        }
        // a subscription with NO subjects is rejected too.
        assert_eq!(
            Subscription::bind(name.clone(), &[], PrefetchBound::DEFAULT),
            Err(SubscribeError::NoSubjects)
        );
        // a concrete subject is admitted.
        let ok = Subscription::bind(name, &["issues.issue.created"], PrefetchBound::DEFAULT);
        assert!(ok.is_ok(), "a concrete (non-wildcard) subject is admitted");
    }

    /// A `Subscription` value witnesses the `*`-free whitelist: `matches` is a real prefix match,
    /// never over-broad. A subject NOT on the whitelist does not match.
    #[test]
    fn subscription_matches_only_whitelisted_subjects() {
        let s = sub("indexer", &["myelin://acme/issues/"]);
        assert!(s.matches("myelin://acme/issues/issue/PROJ-1"));
        assert!(!s.matches("myelin://acme/chat/message/1"), "off-whitelist subject does not match");
    }

    /// `Subscription` carries the durable name + the exact whitelist + the bounded prefetch it
    /// was bound with (the accessors are the registration record a reconnect re-binds).
    #[test]
    fn subscription_carries_name_subjects_and_prefetch() {
        let s = Subscription::bind(
            ConsumerName("indexer".into()),
            &["myelin://acme/issues/", "myelin://acme/refs/"],
            PrefetchBound::new(8).unwrap(),
        )
        .unwrap();
        assert_eq!(s.name(), &ConsumerName("indexer".into()));
        assert_eq!(s.prefetch().get(), 8);
        assert_eq!(
            s.subjects(),
            &[
                SubjectPattern("myelin://acme/issues/".into()),
                SubjectPattern("myelin://acme/refs/".into()),
            ],
            "the whitelist is exactly the (non-wildcard) subjects bound"
        );
    }

    // --- Rule 1: idempotent on event_id via the dedup ledger ---

    /// A redelivered `event_id` is a no-op: the ledger absorbs it, the handler runs exactly ONCE,
    /// and the redelivery reads `Deduplicated` (0 dup). The core idempotency property (SUB-D2 /
    /// SUB-D1-through-a-consumer).
    #[test]
    fn redelivered_event_id_is_a_no_op_handler_runs_once() {
        let h = done_handler();
        let c = Consumer::new(h, sub("indexer", &["myelin://acme/issues/"]), DedupLedger::new());
        let m = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");

        assert_eq!(c.deliver(&m), Delivered::Acked, "first delivery runs + acks");
        assert_eq!(c.deliver(&m), Delivered::Deduplicated, "redelivery is deduped");
        assert_eq!(c.deliver(&m), Delivered::Deduplicated, "and again");
        assert_eq!(c.handler.runs.load(Ordering::SeqCst), 1, "the handler ran EXACTLY once");
        assert_eq!(c.dedup().len(), 1, "one (consumer, event_id) pair recorded");
    }

    /// The dedup PK is `(consumer, event_id)`: the SAME event delivered to TWO consumers (sharing
    /// the ledger) is fresh for EACH — each processes it once. A per-consumer key, not global.
    #[test]
    fn dedup_key_is_per_consumer_two_consumers_each_process_once() {
        let ledger = DedupLedger::new();
        let a = Consumer::new(done_handler(), sub("indexer", &["myelin://acme/issues/"]), ledger.clone());
        let b = Consumer::new(done_handler(), sub("notifier", &["myelin://acme/issues/"]), ledger.clone());
        let m = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");

        assert_eq!(a.deliver(&m), Delivered::Acked, "consumer A processes it");
        assert_eq!(b.deliver(&m), Delivered::Acked, "consumer B ALSO processes it (different PK)");
        // A second delivery to each is deduped.
        assert_eq!(a.deliver(&m), Delivered::Deduplicated);
        assert_eq!(b.deliver(&m), Delivered::Deduplicated);
        assert_eq!(ledger.len(), 2, "two distinct (consumer, event_id) pairs");
    }

    /// The dedup ledger's `is_empty` / `is_handled` read its state precisely: empty before any
    /// mark, non-empty + `is_handled` true for the exact `(consumer, event_id)` pair after.
    #[test]
    fn dedup_ledger_is_empty_and_is_handled_track_state() {
        let ledger = DedupLedger::new();
        let consumer = ConsumerName("indexer".into());
        let id = EventId("01J-1".into());
        assert!(ledger.is_empty(), "a fresh ledger is empty");
        assert!(!ledger.is_handled(&consumer, &id), "nothing handled yet");

        assert!(ledger.mark_handled(&consumer, &id), "first mark is fresh");
        assert!(!ledger.is_empty(), "the ledger is no longer empty after a mark");
        assert!(ledger.is_handled(&consumer, &id), "the exact pair is handled");
        // a different consumer's view of the same id is still unhandled (per-consumer PK).
        assert!(!ledger.is_handled(&ConsumerName("other".into()), &id));
        // a re-mark of the same pair is NOT fresh (the idempotency no-op).
        assert!(!ledger.mark_handled(&consumer, &id), "re-mark is a duplicate, not fresh");
    }

    // --- Rule 4: bind-by-name — a reconnect resumes (the SUB-D2 0-lost-across-reconnect core) ---

    /// **Bind-by-name across a reconnect: 0 lost, 0 dup.** A consumer processes some events, then
    /// "reconnects" (a NEW `Consumer` re-bound by the SAME name + the SAME ledger). The already-
    /// handled events are deduped (0 dup), the not-yet-handled events are processed (0 lost).
    #[test]
    fn reconnect_rebinds_by_name_zero_lost_zero_dup() {
        let ledger = DedupLedger::new();
        let m1 = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");
        let m2 = msg("01J-2", "myelin://acme/issues/issue/PROJ-1");

        // First connection: handles m1, then the broker "drops" before m2.
        {
            let c = Consumer::new(done_handler(), sub("indexer", &["myelin://acme/issues/"]), ledger.clone());
            assert_eq!(c.deliver(&m1), Delivered::Acked);
            // broker drops here — m2 was never delivered.
        }

        // Reconnect: SAME name, SAME ledger. The broker redelivers BOTH m1 and m2 (at-least-once).
        let h = done_handler();
        let c2 = Consumer::new(h, sub("indexer", &["myelin://acme/issues/"]), ledger.clone());
        assert_eq!(c2.deliver(&m1), Delivered::Deduplicated, "m1 already handled → 0 dup");
        assert_eq!(c2.deliver(&m2), Delivered::Acked, "m2 handled after reconnect → 0 lost");
        assert_eq!(c2.handler.runs.load(Ordering::SeqCst), 1, "only m2 re-ran the handler");
        assert_eq!(ledger.len(), 2, "both events are now in the ledger");
    }

    // --- Rule 5: terminate poison immediately, don't burn the redelivery budget / block ---

    /// A poison message (`NonRetryable`) dead-letters IMMEDIATELY (rule 5): it is surfaced in
    /// `dead_letters`, acked (does not redeliver), and does NOT block the subject behind it.
    #[test]
    fn poison_message_dead_letters_immediately_and_is_surfaced() {
        let h = CountingHandler {
            runs: AtomicU32::new(0),
            subjects: SUBJECTS,
            outcome: |_| HandleOutcome::NonRetryable(Reason("malformed".into())),
        };
        let c = Consumer::new(h, sub("indexer", &["myelin://acme/issues/"]), DedupLedger::new());
        let poison = msg("01J-bad", "myelin://acme/issues/issue/PROJ-1");

        let out = c.deliver(&poison);
        assert_eq!(out, Delivered::DeadLettered(Reason("malformed".into())));
        assert_eq!(c.dead_letters().len(), 1, "the poison is SURFACED, not silently dropped");
        assert_eq!(c.dead_letters()[0].reason, Reason("malformed".into()));
        assert_eq!(c.lag(), 0, "a dead-lettered message does not sit in lag (it is terminal)");

        // a SECOND delivery of the same poison is deduped (its mark stayed) — it does not
        // re-poison / re-burn anything.
        assert_eq!(c.deliver(&poison), Delivered::Deduplicated, "a redelivered dead-letter is deduped");
        assert_eq!(c.dead_letters().len(), 1, "still exactly one dead-letter (not re-poisoned)");
    }

    /// A fast subject is NOT head-of-line-blocked by a poison/slow subject: a poison on subject A
    /// dead-letters (terminal, lag 0) while a fast message on subject B acks. Rule 5 + rule 6.
    #[test]
    fn slow_or_poison_subject_does_not_block_a_fast_one() {
        // one consumer subscribed to TWO subjects; A poisons, B is fast.
        let h = CountingHandler {
            runs: AtomicU32::new(0),
            subjects: SUBJECTS,
            // subject "A" poisons; everything else is Done.
            outcome: |ev| {
                if ev.subject.0.contains("/A/") {
                    HandleOutcome::NonRetryable(Reason("poison A".into()))
                } else {
                    HandleOutcome::Done
                }
            },
        };
        let c = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/A/", "myelin://acme/B/"]),
            DedupLedger::new(),
        );
        let a = msg("01J-A", "myelin://acme/A/x");
        let b = msg("01J-B", "myelin://acme/B/y");

        // A poisons (dead-letters, terminal); B still processes — A did not block B.
        assert!(matches!(c.deliver(&a), Delivered::DeadLettered(_)));
        assert_eq!(c.deliver(&b), Delivered::Acked, "subject B is not head-of-line-blocked by A");
        assert_eq!(c.lag_on("myelin://acme/A/x"), 0, "the poison subject did not accumulate lag");
        assert_eq!(c.lag_on("myelin://acme/B/y"), 0, "B drained");
    }

    // --- Rule 2: ack-after-enqueue (a Retry is NOT acked — 0 lost) ---

    /// A `Retry` does NOT ack: the message stays pending (lag rises), the dedup mark is REVERTED,
    /// and a later redelivery RE-RUNS the handler (a transient failure is never swallowed — that
    /// would be silent data loss). When the redelivery succeeds, lag clears.
    #[test]
    fn retry_does_not_ack_redelivery_reruns_then_succeeds() {
        // The handler fails (Retry) the FIRST time it sees an id, then succeeds.
        struct Flaky {
            seen: Mutex<HashSet<String>>,
        }
        impl EventHandler for Flaky {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
                let mut seen = self.seen.lock().unwrap();
                if seen.insert(ev.event_id.0.clone()) {
                    HandleOutcome::Retry(Backoff { seconds: 2 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
        let c = Consumer::new(
            Flaky { seen: Mutex::new(HashSet::new()) },
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let m = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");

        // first delivery: Retry → NOT acked → pending lag rises, dedup mark reverted.
        assert_eq!(c.deliver(&m), Delivered::Retried(2));
        assert_eq!(c.lag(), 1, "an un-acked retry sits in consumer lag");
        assert!(!c.dedup().is_handled(c.name(), &m.envelope.event_id), "a retry leaves NO dedup mark");

        // redelivery: the handler RE-RUNS (0 lost) and now succeeds → acked, lag clears.
        assert_eq!(c.deliver(&m), Delivered::Acked, "the redelivery re-ran the handler and succeeded");
        assert_eq!(c.lag(), 0, "lag recovers to 0 after the successful redelivery (SUB-D2)");
        assert!(c.dedup().is_handled(c.name(), &m.envelope.event_id), "now it is durably handled");
    }

    // --- Rule 6: bounded prefetch ---

    /// `PrefetchBound::new(0)` is rejected (a zero prefetch would consume nothing); a positive
    /// bound is admitted and read back.
    #[test]
    fn prefetch_bound_rejects_zero() {
        assert_eq!(PrefetchBound::new(0), None, "a zero prefetch is meaningless, rejected");
        assert_eq!(PrefetchBound::new(8).unwrap().get(), 8);
        assert_eq!(PrefetchBound::DEFAULT.get(), 64);
    }

    /// `deliver_lane` honours the bounded prefetch (rule 6): with a bound of 2, a lane of 5
    /// delivers only the first 2 this drain; the rest are left for the next drain (lag, not loss).
    #[test]
    fn deliver_lane_honours_bounded_prefetch() {
        let bound = PrefetchBound::new(2).unwrap();
        let s = Subscription::bind(ConsumerName("indexer".into()), &["myelin://acme/issues/"], bound).unwrap();
        let c = Consumer::new(done_handler(), s, DedupLedger::new());

        let lane: Vec<Message> = (0..5)
            .map(|i| msg(&format!("01J-{i}"), "myelin://acme/issues/issue/PROJ-1"))
            .collect();
        let out = c.deliver_lane("myelin://acme/issues/issue/PROJ-1", &lane);
        assert_eq!(out.len(), 2, "bounded prefetch: only 2 of 5 delivered this drain");
        assert!(out.iter().all(|o| *o == Delivered::Acked));
        assert_eq!(c.handler.runs.load(Ordering::SeqCst), 2, "the handler ran exactly twice");
    }

    // --- The migration shape (contract 2.5) ---

    /// The `consumer_dedup` migration is the frozen 2.5 shape: the `(consumer, event_id)` PK +
    /// the columns are present; forward-only (no destructive DROP).
    #[test]
    fn migration_is_the_frozen_2_5_shape() {
        assert!(CONSUMER_DEDUP_MIGRATION.contains("CREATE TABLE IF NOT EXISTS consumer_dedup"));
        assert!(CONSUMER_DEDUP_MIGRATION.contains("PRIMARY KEY (consumer, event_id)"));
        for col in ["consumer", "event_id", "recorded_at"] {
            assert!(CONSUMER_DEDUP_MIGRATION.contains(col), "missing column {col}");
        }
        assert!(!CONSUMER_DEDUP_MIGRATION.contains("DROP TABLE"), "forward-only: no destructive down");
    }

    // --- The upcaster pre-handle hook (P-S09 floor) ---

    /// The upcaster runs BEFORE handle (P-S09 plugs the registry here): a `with_upcaster` that
    /// rewrites `schema_ver` 1 → 2 means the handler sees the upcasted shape. Until P-S09 the
    /// default is the identity map.
    #[test]
    fn upcaster_runs_before_handle() {
        let seen_ver = Arc::new(AtomicU32::new(0));
        let seen2 = seen_ver.clone();
        struct VerHandler {
            seen: Arc<AtomicU32>,
        }
        impl EventHandler for VerHandler {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
                self.seen.store(ev.schema_ver, Ordering::SeqCst);
                HandleOutcome::Done
            }
        }
        let c = Consumer::new(
            VerHandler { seen: seen2 },
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        )
        .with_upcaster(|mut e| {
            // model a v1 → v2 upcaster (the real registry is P-S09).
            if e.schema_ver == 1 {
                e.schema_ver = 2;
            }
            e
        });
        c.deliver(&msg("01J-1", "myelin://acme/issues/issue/PROJ-1"));
        assert_eq!(seen_ver.load(Ordering::SeqCst), 2, "the handler saw the upcasted schema_ver");
    }

    /// An off-whitelist message routed to the consumer is surfaced as a dead-letter, never
    /// silently processed (defence in depth around rule 3).
    #[test]
    fn off_whitelist_message_is_dead_lettered_not_silently_processed() {
        let c = Consumer::new(done_handler(), sub("indexer", &["myelin://acme/issues/"]), DedupLedger::new());
        let off = msg("01J-off", "myelin://acme/chat/message/1");
        assert!(matches!(c.deliver(&off), Delivered::DeadLettered(_)));
        assert_eq!(c.handler.runs.load(Ordering::SeqCst), 0, "the handler never ran for an off-whitelist subject");
        assert_eq!(c.dead_letters().len(), 1);
    }
}
