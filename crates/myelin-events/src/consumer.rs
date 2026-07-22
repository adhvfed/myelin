//! # The idempotent event-consumer runtime (SUB-D2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §5 (the
//! event-consumer template + the **seven encoded rules**) and §5.3 (causality through the
//! consumer — `emit(draft, cause = Some(incoming))`).
//!
//! **Contract-index:** row 2.4 (`EventHandler` template + `HandleOutcome`). The `consumer_dedup`
//! ledger (row 2.5, the effectively-once anchor this runtime's rule 1 keys on) is **EB-06 /
//! P-015**'s named deliverable — it lives in [`crate::dedup`] ([`crate::DedupLedger`] +
//! [`crate::CONSUMER_DEDUP_MIGRATION`]); this runtime calls it. **P-S08 → global P-009.** This is
//! the consumer half of the **silent-data-loss floor** (SUB-D2): a **PERMANENT gate**, re-run on
//! every emit-path change.
//!
//! ## What this module ships (the one template the whole platform is built from)
//! The [`EventHandler`](crate::EventHandler) trait (the body a subsystem writes — frozen at
//! [`crate`]) is wrapped by ONE runtime, [`Consumer`], so the seven correctness rules cannot be
//! skipped per-consumer:
//!
//! 1. **Idempotent on `event_id`** via the per-consumer [`crate::DedupLedger`] (`(consumer,
//!    event_id)` PK, contract 2.5, EB-06) — at-least-once delivery + an idempotent handler ≈
//!    effectively-once.
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
//! ## The same-transaction co-commit — DELIVERED (peer-review #7 / MR-023b — FIXED)
//! The `consumer_dedup` ledger ([`crate::DedupLedger`], EB-06) is modeled in-memory with
//! byte-for-byte the 2.5 semantics (its frozen DDL [`crate::CONSUMER_DEDUP_MIGRATION`] is the shape
//! the runner applies) AND binds to a durable PG backing in `serve`. **The atomicity floor is
//! closed:** [`deliver`](Consumer::deliver) no longer marks-then-hopes — it opens ONE transaction
//! via [`crate::DedupLedger::begin_co_commit`] (the `INSERT … ON CONFLICT DO NOTHING` WITHIN it),
//! runs the handler on that transaction (the handler co-commits its durable state write via the
//! [`crate::HandlerTx`] connection), then **commits on `Done` (mark + effect together) / rolls back
//! on `Retry`/failure (both vanish)**. So a kill-9 between the mark and the commit leaves NEITHER →
//! a redelivery re-runs and the effect lands (exactly-once-with-effect), never the old committed-
//! mark-with-lost-effect at-most-once floor. The seam shape (the `EventHandler` trait — now with the
//! `HandlerTx` param — the `(consumer, event_id)` key, the lag signal) is otherwise unchanged. The
//! durable broker cursor + the real `dlq.<tenant>.<subsystem>` subject is EB-04's / EB-05's
//! refinement (named, not silently skipped).
//!
//! ## The upcaster runs BEFORE handle (P-S09 — contract 2.8)
//! The consumer runtime calls the `(type, from_ver) → to_ver` upcaster registry
//! ([`crate::upcast::UpcasterRegistry`], **P-S09**) BEFORE `handle` so a handler always sees the
//! current `schema_ver` shape. The [`Consumer`] exposes the pre-handle hook
//! ([`Consumer::with_upcaster`]) the registry's [`crate::upcast::UpcasterRegistry::into_hook`]
//! plugs into. The hook is **fallible**: an unbridgeable version gap (a missing upcaster) is a
//! loud [`HandleOutcome::NonRetryable`] → the message is dead-lettered (DLQ), NEVER silently
//! dropped and NEVER handed to a handler at the wrong shape (rule 2 of contract 2.8). With no
//! registry installed the hook is the identity map (no evolution declared → every event is
//! already current).

use crate::dead_letter::{DeadLetterRecord, DeadLetterSink};
use crate::{
    DedupLedger, EventEnvelope, EventHandler, HandleOutcome, HandlerTx, Reason, SubjectPattern,
};
use myelin_tenancy::TenantId;
use std::collections::HashMap;
use std::sync::Mutex;

/// The backoff (seconds) the runtime asks for when the co-commit COMMIT itself fails (a DB hiccup
/// AFTER a `Done` handler — the mark + effect did not land, so the message redelivers; #7/MR-023b).
const DEFAULT_COMMIT_RETRY_BACKOFF_SECS: u64 = 2;

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

/// **The per-tenant in-flight cap (rule 6, fairness / agent-surge — EB-05).** Bounds how many
/// claimed-but-not-yet-acked messages ONE tenant may hold inside this consumer at once. Where
/// [`PrefetchBound`] bounds the consumer GLOBALLY (a slow *subject* can't monopolise the loop),
/// this bounds it PER TENANT (a single tenant's surge — the agent-generated-load case, §4.2 gotcha
/// 6 — can't starve every other tenant's events behind it). `max_ack_pending` is the broker-level
/// global ceiling; `per_tenant` is the fairness slice carved out of it.
///
/// The cap is **observable** ([`Consumer::tenant_inflight`]) so the contract-1.8
/// `bus.per_tenant_inflight` signal ([`crate::BusSignal::PerTenantInflight`]) reads it, and a drill
/// can assert one tenant's surge did not push a second tenant's events out (fairness held). A
/// message that would exceed a tenant's cap is **deferred, not dropped** ([`Delivered::Throttled`]):
/// it stays pending (lag, not loss) and is re-offered on the next drain once the tenant drains.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PerTenantInflight(u32);

impl PerTenantInflight {
    /// The floor default per-tenant in-flight cap (a fairness slice well under the global
    /// `max_ack_pending`); EB-05 / later tuning may raise it per consumer.
    pub const DEFAULT: PerTenantInflight = PerTenantInflight(16);

    /// A cap of `n` in-flight messages per tenant, or `None` if `n == 0` (a zero cap would let
    /// no tenant make progress — meaningless, rejected rather than silently treated as unbounded).
    pub fn new(n: u32) -> Option<PerTenantInflight> {
        if n == 0 {
            None
        } else {
            Some(PerTenantInflight(n))
        }
    }

    /// The numeric cap.
    pub fn get(self) -> u32 {
        self.0
    }
}

impl Default for PerTenantInflight {
    fn default() -> Self {
        PerTenantInflight::DEFAULT
    }
}

/// **The frozen consumer-registration spec (EB-05 — the one entry-point the whole platform binds
/// through).** `consume(ConsumerSpec{ … }, handler)` is the single sanctioned way to stand up a
/// consumer: it bundles the durable name + the `*`-free subject whitelist + the bounded prefetch
/// (`max_ack_pending`) + the per-tenant in-flight fairness cap, and constructs the [`Consumer`]
/// runtime with all seven rules wired. A subsystem never hand-rolls [`Subscription::bind`] +
/// [`Consumer::new`]; it fills a `ConsumerSpec` and calls [`consume`]. This is "abstract at the
/// first copy" (EI-01 §7): there will be dozens of consumers, and this spec is the one shape they
/// all share — `*`-rejection, bind-by-name, prefetch, and the fairness cap cannot be skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumerSpec {
    /// The durable consumer name (rule 4: bind-by-name; re-bound identically on reconnect).
    pub durable: ConsumerName,
    /// The subject **whitelist** (rule 3: NEVER `*`). [`consume`] rejects a wildcard/empty subject
    /// loudly at registration via [`Subscription::bind`].
    pub subjects: Vec<String>,
    /// The bounded prefetch (rule 6, global): the broker `max_ack_pending` — the max in-flight
    /// (claimed-but-un-acked) messages across the whole consumer per drain.
    pub max_ack_pending: PrefetchBound,
    /// The per-tenant in-flight fairness cap (rule 6, per-tenant): the agent-surge guard so one
    /// tenant cannot starve every other tenant's events.
    pub per_tenant_inflight: PerTenantInflight,
}

impl ConsumerSpec {
    /// A spec with the floor-default prefetch + per-tenant cap, naming the durable consumer and its
    /// `*`-free subject whitelist (still validated at [`consume`] time — this is just the ergonomic
    /// constructor for the common case).
    pub fn new(durable: ConsumerName, subjects: &[&str]) -> ConsumerSpec {
        ConsumerSpec {
            durable,
            subjects: subjects.iter().map(|s| (*s).to_string()).collect(),
            max_ack_pending: PrefetchBound::DEFAULT,
            per_tenant_inflight: PerTenantInflight::DEFAULT,
        }
    }

}

/// **The one sanctioned consumer entry-point (EB-05).** Validates the `ConsumerSpec` (rule 3:
/// rejects a `*`/empty subject LOUDLY) and constructs the [`Consumer`] runtime bound to the durable
/// name (rule 4), the bounded prefetch + per-tenant fairness cap (rule 6), sharing `dedup` (rule 1;
/// a reconnect passes the SAME ledger so a redelivery is absorbed). Every later consumer (Signal
/// engine, Search indexer, Notif router, the dispatch tier) is stood up through THIS function, not
/// by hand-wiring the pieces.
pub fn consume<H: EventHandler>(
    spec: ConsumerSpec,
    handler: H,
    dedup: DedupLedger,
) -> Result<Consumer<H>, SubscribeError> {
    let subjects: Vec<&str> = spec.subjects.iter().map(|s| s.as_str()).collect();
    let subscription = Subscription::bind(spec.durable, &subjects, spec.max_ack_pending)?;
    Ok(Consumer::new(handler, subscription, dedup)
        .with_per_tenant_inflight(spec.per_tenant_inflight))
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
            subjects: subjects
                .iter()
                .map(|s| SubjectPattern((*s).to_string()))
                .collect(),
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
    /// A critical backing is unavailable; pending work does not consume the app/DLQ budget.
    DependencyUnavailable(crate::relay::IntakeDependency, u64),
    /// The message's tenant is already at its per-tenant in-flight cap (rule 6, fairness): the
    /// message was DEFERRED, not handled and not dropped — it stays pending (lag, not loss) and is
    /// re-offered on the next drain once the tenant drains. Carries the tenant that was throttled
    /// so a drill can assert which tenant's surge was bounded (and that OTHER tenants flowed).
    Throttled(TenantId),
}

/// A message the runtime is about to deliver: the durable subject + the canonical envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// The broker subject the message arrived on (whitelisted by the subscription).
    pub subject: String,
    /// The canonical event body (what the handler reads + the `event_id` the ledger dedups on).
    pub envelope: EventEnvelope,
}

/// The pre-handle hook the upcaster registry (P-S09, contract 2.8) plugs into:
/// `(envelope) -> Result<envelope, Reason>`, applied before `handle` so a handler always sees the
/// current `schema_ver` shape. It is **fallible**: a successful bridge yields the upcasted
/// envelope; an unbridgeable version gap yields the [`Reason`] the runtime turns into a
/// [`HandleOutcome::NonRetryable`] → dead-letter (rule 2 of 2.8: loud, never a silent pass).
/// With no registry installed it is the identity map. Boxed so the registry can be swapped
/// without changing the runtime ([`crate::upcast::UpcasterRegistry::into_hook`] produces it).
type Upcaster = Box<dyn Fn(EventEnvelope) -> Result<EventEnvelope, Reason> + Send + Sync>;

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
    pending: Mutex<HashMap<String, std::collections::HashSet<crate::EventId>>>,
    /// Dead-lettered (poison) messages (rule 5), surfaced. **CT-004d.2 chunk 6 / #7b:** now a
    /// [`DeadLetterSink`] (was a bare `Mutex<Vec<DeadLetter>>`) so the durable arm makes a
    /// dead-lettered event — especially the H2 panic path — SURVIVE a restart. The in-memory arm is
    /// the default (unchanged behavior); the durable arm is opt-in via [`Consumer::with_dead_letter_sink`].
    dead_letters: DeadLetterSink,
    /// The per-tenant in-flight fairness cap (rule 6, EB-05). A tenant at its cap is throttled
    /// (deferred, not dropped) so one tenant's surge cannot starve another's events.
    per_tenant_cap: PerTenantInflight,
    /// The set of DISTINCT outstanding (delivered-but-not-yet-terminal) `event_id`s PER TENANT
    /// (rule 6). An event enters when it is first handled and stays while it `Retry`s (it is still
    /// in-flight at the broker); it leaves on a terminal `Done`/`DeadLettered`. Keyed by distinct
    /// event so a REDELIVERY of an already-outstanding event re-claims its existing slot rather
    /// than consuming a second one (the real broker model). The per-tenant set size is what the
    /// cap bounds and what the contract-1.8 `bus.per_tenant_inflight` signal reads.
    tenant_inflight: Mutex<HashMap<TenantId, std::collections::HashSet<crate::EventId>>>,
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
            upcaster: Box::new(Ok),
            pending: Mutex::new(HashMap::new()),
            dead_letters: DeadLetterSink::in_memory(),
            per_tenant_cap: PerTenantInflight::DEFAULT,
            tenant_inflight: Mutex::new(HashMap::new()),
        }
    }

    /// Whether this consumer's frozen whitelist accepts the concrete event subject.
    /// Broker pumps use this before delivery so an event is never sent to an unrelated consumer.
    pub fn accepts(&self, subject: &str) -> bool {
        self.subscription.matches(subject)
    }

    /// Whether this consumer already committed the event's dedup mark.
    pub fn is_handled(&self, event_id: &crate::EventId) -> bool {
        self.dedup.is_handled(self.name(), event_id)
    }

    /// Durably quarantine a handler Retry whose broker delivery budget is exhausted.
    ///
    /// The handler's co-commit transaction has already rolled back on [`Delivered::Retried`], so
    /// this path records no dedup tombstone. A later operator replay can therefore execute the
    /// repaired handler. If durable DLQ persistence fails, the result remains `Retried` and the
    /// broker must not terminate the delivery.
    pub fn dead_letter_exhausted_retry(&self, msg: &Message, delivery_attempt: u64) -> Delivered {
        let reason = Reason(format!(
            "retry budget exhausted after {delivery_attempt} broker deliveries"
        ));
        match self.push_dead_letter(msg.envelope.clone(), reason.clone()) {
            Ok(()) => {
                self.clear_pending(&msg.subject, &msg.envelope.event_id);
                self.clear_tenant_inflight(&msg.envelope.tenant, &msg.envelope.event_id);
                Delivered::DeadLettered(reason)
            }
            Err(_) => {
                // Pending is keyed by event id, so this is idempotent whether `deliver` recorded
                // the retry in this process or a restart reached quarantine directly.
                self.bump_pending(&msg.subject, &msg.envelope.event_id);
                Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
            }
        }
    }

    /// Set the per-tenant in-flight fairness cap (rule 6, EB-05). [`consume`] wires this from the
    /// [`ConsumerSpec::per_tenant_inflight`]; the default is [`PerTenantInflight::DEFAULT`].
    pub fn with_per_tenant_inflight(mut self, cap: PerTenantInflight) -> Self {
        self.per_tenant_cap = cap;
        self
    }

    /// The per-tenant in-flight fairness cap this consumer enforces (rule 6).
    pub fn per_tenant_inflight_cap(&self) -> PerTenantInflight {
        self.per_tenant_cap
    }

    /// **Inject a DURABLE dead-letter sink (CT-004d.2 chunk 6 / #7b).** By default the consumer's
    /// dead-letter set is in-memory ([`DeadLetterSink::in_memory`]) — the unchanged behavior. The
    /// events `serve()` composition root (`myelin_storage::events_serve::EventsRuntime::consumer`)
    /// wires the PG-backed [`DeadLetterSink::durable`] here so a dead-lettered event — especially the
    /// H2 panic path — SURVIVES a restart (it stays in the `consumer_dead_letter` table after the pump
    /// acks it and the broker cursor advances). Mirrors how the durable [`DedupLedger`] is injected.
    pub fn with_dead_letter_sink(mut self, sink: DeadLetterSink) -> Self {
        self.dead_letters = sink;
        self
    }

    /// The current in-flight (delivered-but-not-terminal, distinct-event) count for ONE tenant
    /// (rule 6). The contract-1.8 `bus.per_tenant_inflight` survival signal reads this; a fairness
    /// drill asserts it never exceeds the cap and that a throttled tenant's surge did not push
    /// another tenant out.
    pub fn tenant_inflight(&self, tenant: &TenantId) -> u32 {
        self.tenant_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .map(|s| s.len() as u32)
            .unwrap_or(0)
    }

    /// Install the pre-handle upcaster (P-S09 plugs the `(type, from_ver) → to_ver` registry in
    /// here, via [`crate::upcast::UpcasterRegistry::into_hook`]). The runtime applies it before
    /// `handle` so a handler always sees the current shape. The hook is **fallible**: an
    /// unbridgeable gap returns `Err(Reason)` → the runtime dead-letters the message
    /// ([`HandleOutcome::NonRetryable`] / DLQ), never silently passing the wrong shape (2.8).
    pub fn with_upcaster(
        mut self,
        upcaster: impl Fn(EventEnvelope) -> Result<EventEnvelope, Reason> + Send + Sync + 'static,
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
        self.pending().values().map(|events| events.len() as u64).sum()
    }

    /// The un-acked backlog on ONE subject (rule 6+7: a slow subject's lag does not stall a fast
    /// one — the drill reads per-subject lag to assert no head-of-line block).
    pub fn lag_on(&self, subject: &str) -> u64 {
        self.pending()
            .get(subject)
            .map(|events| events.len() as u64)
            .unwrap_or(0)
    }

    /// The dead-lettered (poison) messages so far (rule 5), surfaced IN-PROCESS. On the in-memory
    /// sink this is the full list; on the durable sink it is the fail-fallback list only (poison the
    /// durable `record` could not persist) — the restart-surviving durable rows are read via
    /// [`Consumer::durable_dead_letters`] (mirrors [`DedupLedger::len`] returning the in-process view
    /// on the durable backend).
    pub fn dead_letters(&self) -> Vec<DeadLetter> {
        self.dead_letters.surfaced()
    }

    /// **The DURABLE dead-letter snapshot for ops (CT-004d.2 chunk 6 / #7b).** The PII-free
    /// restart-surviving rows for THIS consumer (`event_id` + bounded `reason`, never the envelope).
    /// Empty on the in-memory sink (there is no durable table). A dead-lettered event — especially the
    /// H2 panic path — is still present here after a restart (the whole point of the durable sink).
    pub fn durable_dead_letters(&self) -> Vec<DeadLetterRecord> {
        self.dead_letters.durable_dead_letters(self.name())
    }

    /// **Deliver ONE message through the seven rules.** Applies, in order:
    /// rule 3 (the subject must be on the whitelist — a non-matching subject is a programming
    /// error and is rejected, never silently processed); the upcaster (P-S09); rule 1 (dedup —
    /// a re-seen `event_id` SKIPs the handler and acks → [`Delivered::Deduplicated`]); the
    /// handler; then the terminal-outcome dispatch — rule 2 (ack only on `Done`), rule 5
    /// (`NonRetryable` → dead-letter immediately), and `Retry` (NOT acked → stays pending, lag
    /// rises). Returns what the runtime DID.
    ///
    /// **Rule 6, per-tenant fairness (EB-05):** before handling, if the message's tenant is already
    /// at its [`PerTenantInflight`] cap, the message is THROTTLED ([`Delivered::Throttled`]) —
    /// deferred, not dropped — so one tenant's surge cannot starve another tenant's events. A
    /// `Retry` holds the tenant's in-flight slot (the work is still outstanding); a `Done` /
    /// `Deduplicated` / `DeadLettered` releases it.
    pub fn deliver(&self, msg: &Message) -> Delivered {
        // Rule 3: the subject MUST be on this consumer's `*`-free whitelist. (A `Subscription`
        // cannot carry a `*`, so this is a real whitelist match — never over-broad.)
        if !self.subscription.matches(&msg.subject) {
            // A message off-whitelist should never have been routed here; treat it as poison so
            // it is surfaced, not silently swallowed.
            let reason = Reason(format!("subject {} not on consumer whitelist", msg.subject));
            return match self.push_dead_letter(msg.envelope.clone(), reason.clone()) {
                Ok(()) => {
                    self.clear_pending(&msg.subject, &msg.envelope.event_id);
                    Delivered::DeadLettered(reason)
                }
                Err(_) => {
                    self.bump_pending(&msg.subject, &msg.envelope.event_id);
                    Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                }
            };
        }

        // Rule 6, per-tenant fairness (EB-05): if this tenant is already at its in-flight cap AND
        // this is a NEW event (not the redelivery of an already-outstanding one), the message is
        // THROTTLED — deferred (stays pending, lag rises), never dropped and never handed to the
        // handler. This is the agent-surge guard: a single tenant flooding the consumer holds at
        // most `per_tenant_cap` distinct in-flight events, so EVERY other tenant's events keep
        // flowing. A redelivery of an event already in-flight re-claims its existing slot (the real
        // broker model), so a retried event can always make progress. The check is BEFORE the dedup
        // mark so a throttled message is fully re-offerable on the next drain (it was never handled).
        let tenant = msg.envelope.tenant.clone();
        let event_id = msg.envelope.event_id.clone();
        if !self.tenant_has_inflight(&tenant, &event_id)
            && self.tenant_inflight(&tenant) >= self.per_tenant_cap.get()
        {
            self.bump_pending(&msg.subject, &event_id);
            return Delivered::Throttled(tenant);
        }

        // The upcaster (P-S09, contract 2.8) runs BEFORE handle so the handler sees the current
        // `schema_ver` shape. It is FALLIBLE: an unbridgeable version gap (a missing upcaster) is
        // a loud dead-letter (rule 2 of 2.8) — the message is term'd to the DLQ, NEVER silently
        // dropped and NEVER handed to the handler at the wrong shape. The dedup ledger is NOT
        // marked for a gap: the event was never handled, so a later redelivery (e.g. after the
        // missing upcaster is deployed) can be processed — this is forward-only, not a tombstone.
        let envelope = match (self.upcaster)(msg.envelope.clone()) {
            Ok(env) => env,
            Err(reason) => {
                return match self.push_dead_letter(msg.envelope.clone(), reason.clone()) {
                    Ok(()) => {
                        self.clear_pending(&msg.subject, &event_id);
                        Delivered::DeadLettered(reason)
                    }
                    Err(_) => {
                        self.bump_pending(&msg.subject, &event_id);
                        Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                    }
                };
            }
        };
        debug_assert_eq!(
            envelope.event_id, event_id,
            "an upcaster never changes event_id"
        );

        // Rule 1 + the #7/MR-023b same-transaction co-commit: open ONE transaction and INSERT the
        // `(consumer, event_id)` dedup mark WITHIN it (the freshness check). A FRESH pair runs the
        // handler ON THIS TRANSACTION; a DUPLICATE (redelivery) SKIPs it and acks — 0 dup. The mark
        // is NOT yet committed: it commits only when the handler returns `Done` (mark + effect
        // together) and rolls back on `Retry`/failure (both vanish, so a redelivery re-runs — never
        // a committed mark with a lost effect, which was the old at-most-once floor).
        let (mut cotx, fresh) =
            self.dedup
                .begin_co_commit(self.name(), &event_id, &tenant, &msg.envelope.region);
        if !fresh {
            // Already handled by this consumer: skip + ack (the cursor advances; lag clears). Roll
            // back the (empty) transaction — the pre-existing mark is untouched.
            cotx.rollback();
            self.clear_pending(&msg.subject, &event_id);
            return Delivered::Deduplicated;
        }

        // Rule 6: this fresh message now occupies a per-tenant in-flight slot (the work is
        // outstanding until a terminal ack). The slot is keyed by `event_id` so a redelivery of an
        // already-outstanding event re-claims the SAME slot. It is released on Done/DeadLettered
        // below; a Retry KEEPS it (the work is still pending) so a tenant whose events keep failing
        // fills its cap and throttles, while other tenants flow.
        self.bump_tenant_inflight(&tenant, &event_id);

        // Run the handler (the consumer's body) ON THE CO-COMMIT TRANSACTION. A durable handler
        // runs its state write on `tx.connection::<sqlx::PgConnection>()` so it co-commits with the
        // dedup mark (#7/MR-023b); an in-memory / outbox-seam handler ignores the tx. A reaction it
        // emits calls `OutboxTx::emit(draft, cause = Some(&envelope))` (§5.3). The `HandlerTx`
        // borrows the transaction; it is dropped before the runtime commits/rolls back the tx.
        //
        // **H2 (peer-review #7 re-prosecution) — DEFENSE IN DEPTH against a handler PANIC.** The
        // handler is arbitrary code; a bug can `panic!` (an unwind). WITHOUT a guard the unwind
        // propagates out of `deliver` and tears down the pump task, and — before H2's native-tx fix in
        // the durable backing — leaked the OPEN co-commit tx to the pool (the next delivery then
        // `COMMIT`ed the panicked delivery's mark+effect → its valid effect LOST). We `catch_unwind`
        // the handler (the handler is `&self`, so `AssertUnwindSafe` is sound — we touch no broken
        // invariant after the unwind: we only ROLL BACK the tx and dead-letter). On unwind we ROLL
        // BACK the co-commit tx (mark + any partial write vanish together) and DEAD-LETTER the message
        // (surfaced, terminal → acked so it does not redeliver-and-re-panic in a loop). We deliberately
        // do NOT record a dedup tombstone (unlike the `NonRetryable` poison path): a panic is a BUG,
        // not poison — the event may carry a VALID effect that the buggy code failed to apply, so the
        // undeduped event stays replayable from the dead-letter set after the bug is fixed + redeployed
        // (never a silent loss). **CT-004d.2 chunk 6 / #7b — this replayability is now DURABLE:** the
        // `push_dead_letter` below writes through the [`DeadLetterSink`]; with a durable sink the poison
        // persists to the `consumer_dead_letter` table on its OWN fresh pool connection (the co-commit
        // tx here was just rolled back — it cannot be reused), so after the pump acks this (terminal)
        // and the broker cursor advances, the record SURVIVES a restart (before #7b it lived only in a
        // volatile `Vec` that vanished with the process — the "stays replayable" claim was self-
        // defeating as shipped). A `Retry` was rejected here because a deterministic panic would
        // livelock the subject (panic-loop); a dead-letter quarantines the bug loudly and lets the
        // subject progress.
        let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut htx = match cotx.connection() {
                Some(conn) => HandlerTx::with_connection(conn),
                None => HandlerTx::none(),
            };
            self.handler.handle(&envelope, &mut htx)
        }));
        let outcome = match handled {
            Ok(outcome) => outcome,
            Err(panic) => {
                // The handler panicked (a bug). Roll back the co-commit tx (H2: mark + any partial
                // effect vanish; the native `sqlx::Transaction` also rolls back on drop, so the pool
                // connection is never leaked mid-tx), then dead-letter LOUDLY without a tombstone.
                cotx.rollback();
                let detail = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "<non-string panic payload>".to_string());
                // #7b PII-SAFETY: the panic `detail` is developer-authored but can interpolate event
                // content (e.g. `panic!("bad {field}", …)`), so it may carry inline PII. It goes to the
                // EPHEMERAL loud log ONLY — NEVER into the durable `consumer_dead_letter.reason` (a
                // crypto-shred/erasure-strict store that subject erasure does not reach). The persisted
                // reason is a PII-FREE constant; the event_id (a ULID) + this constant are enough for
                // ops to find the poison and correlate it with the logged detail.
                eprintln!(
                    "[consumer:{}] handler PANICKED for event {} — dead-lettered (no tombstone; effect \
                     replayable): {detail}",
                    self.name().0,
                    event_id.0
                );
                let reason = Reason(
                    "handler PANICKED (a bug, dead-lettered without a dedup tombstone so the valid \
                     effect stays replayable; panic detail in the loud log, kept OUT of the durable \
                     DLQ to stay PII-safe)"
                        .to_string(),
                );
                return match self.push_dead_letter(envelope, reason.clone()) {
                    Ok(()) => {
                        self.clear_pending(&msg.subject, &event_id);
                        self.clear_tenant_inflight(&tenant, &event_id);
                        Delivered::DeadLettered(reason)
                    }
                    Err(_) => {
                        // The poison itself is known, but its durable replay/quarantine record did
                        // not commit. Keep the event outstanding and withhold the broker ack.
                        self.bump_pending(&msg.subject, &event_id);
                        Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                    }
                };
            }
        };
        match outcome {
            HandleOutcome::Done => {
                // Rule 2: ack AFTER the handler succeeded — but ONLY once the co-commit COMMITS (the
                // dedup mark + the handler's durable effect land together). A commit failure means
                // NEITHER landed → treat it as a Retry (do NOT ack; the redelivery re-runs — 0 lost).
                match cotx.commit() {
                    Ok(()) => {
                        self.clear_pending(&msg.subject, &event_id);
                        self.clear_tenant_inflight(&tenant, &event_id);
                        Delivered::Acked
                    }
                    Err(_e) => {
                        // The tx did not commit (DB hiccup) — mark + effect both absent. Redeliver
                        // (0 lost); keep the in-flight slot (the work is still outstanding).
                        self.bump_pending(&msg.subject, &event_id);
                        Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                    }
                }
            }
            HandleOutcome::NonRetryable(reason) => {
                // Rule 5: poison terminates immediately — dead-letter + ack so it does NOT burn the
                // redelivery budget and does NOT block the subject behind it.
                //
                // Roll back the co-commit tx FIRST so any speculative partial write the poison
                // handler made is discarded (a malformed event must leave no partial effect). Then
                // record a STANDALONE tombstone mark so a redelivered dead-letter is itself
                // deduplicated (never re-poisons) — there is no valid effect to co-commit for a
                // poison, so the standalone mark is correct here (it does not re-open #7: #7 is about
                // losing a VALID effect; a poison has none). This PRESERVES the existing rule-5
                // "redelivered dead-letter is Deduplicated" semantics.
                cotx.rollback();
                match self.push_dead_letter(envelope, reason.clone()) {
                    Ok(()) => {
                        // The poison is durably replayable/quarantined. Only now may the terminal
                        // dedup tombstone land and the service-level broker cursor advance.
                        self.dedup.mark_handled(self.name(), &event_id);
                        self.clear_pending(&msg.subject, &event_id);
                        self.clear_tenant_inflight(&tenant, &event_id);
                        Delivered::DeadLettered(reason)
                    }
                    Err(_) => {
                        self.bump_pending(&msg.subject, &event_id);
                        Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                    }
                }
            }
            HandleOutcome::Retry(backoff) => {
                // NOT acked (rule 2): the message stays pending → it redelivers, and lag rises. The
                // per-tenant in-flight slot is KEPT (the work is still outstanding) — this is what
                // makes a surging tenant whose events keep retrying eventually hit its cap and
                // throttle. Roll back the co-commit tx: the dedup mark AND any handler write vanish
                // together (a retry is NOT a completed handle, so a later redelivery must run the
                // handler again — this IS the old speculative-mark-revert, now atomic with the
                // handler's effect).
                cotx.rollback();
                self.bump_pending(&msg.subject, &event_id);
                Delivered::Retried(backoff.seconds)
            }
            HandleOutcome::DependencyUnavailable { dependency, backoff } => {
                cotx.rollback();
                self.bump_pending(&msg.subject, &event_id);
                Delivered::DependencyUnavailable(dependency, backoff.seconds)
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

    /// Record a poison through the dead-letter sink (rule 5). **CT-004d.2 chunk 6 / #7b:** on the
    /// durable arm this persists a PII-free `(consumer, event_id, bounded_reason)` row on the sink's
    /// OWN fresh pool connection so it survives a restart. This matters most on the H2 panic path,
    /// whose co-commit tx was already ROLLED BACK — the durable backing must (and does) acquire a
    /// fresh connection, never the rolled-back co-commit conn. A durable write failure is returned
    /// to the delivery path, which converts it to Retry and withholds the broker ack.
    fn push_dead_letter(&self, envelope: EventEnvelope, reason: Reason) -> Result<(), String> {
        self.dead_letters
            .push(self.name(), DeadLetter { envelope, reason })
    }

    fn pending(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, std::collections::HashSet<crate::EventId>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump_pending(&self, subject: &str, event_id: &crate::EventId) {
        self.pending()
            .entry(subject.to_string())
            .or_default()
            .insert(event_id.clone());
    }

    fn clear_pending(&self, subject: &str, event_id: &crate::EventId) {
        let mut pending = self.pending();
        if let Some(events) = pending.get_mut(subject) {
            events.remove(event_id);
            if events.is_empty() {
                pending.remove(subject);
            }
        }
    }

    /// Is `event_id` already in `tenant`'s outstanding (in-flight) set? A redelivery of an
    /// outstanding event re-claims its slot rather than consuming a new one.
    fn tenant_has_inflight(&self, tenant: &TenantId, event_id: &crate::EventId) -> bool {
        self.tenant_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .map(|s| s.contains(event_id))
            .unwrap_or(false)
    }

    fn bump_tenant_inflight(&self, tenant: &TenantId, event_id: &crate::EventId) {
        self.tenant_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(tenant.clone())
            .or_default()
            .insert(event_id.clone());
    }

    fn clear_tenant_inflight(&self, tenant: &TenantId, event_id: &crate::EventId) {
        let mut guard = self
            .tenant_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(set) = guard.get_mut(tenant) {
            set.remove(event_id);
            if set.is_empty() {
                guard.remove(tenant);
            }
        }
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
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
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
        fn handle(&self, ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
            self.runs.fetch_add(1, Ordering::SeqCst);
            (self.outcome)(ev)
        }
    }

    static SUBJECTS: &[SubjectPattern] = &[];

    fn done_handler() -> CountingHandler {
        CountingHandler {
            runs: AtomicU32::new(0),
            subjects: SUBJECTS,
            outcome: |_| HandleOutcome::Done,
        }
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
            assert!(
                matches!(r, Err(SubscribeError::WildcardSubject(_))),
                "`{bad}` must be rejected"
            );
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
        assert!(
            !s.matches("myelin://acme/chat/message/1"),
            "off-whitelist subject does not match"
        );
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

    #[test]
    fn consumer_routing_and_handled_status_accessors_reflect_live_state() {
        let consumer = Consumer::new(
            done_handler(),
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let message = msg("01J-STATUS", "myelin://acme/issues/issue/PROJ-1");

        assert!(consumer.accepts(&message.subject));
        assert!(!consumer.accepts("myelin://acme/chat/message/1"));
        assert!(!consumer.is_handled(&message.envelope.event_id));
        assert_eq!(consumer.deliver(&message), Delivered::Acked);
        assert!(consumer.is_handled(&message.envelope.event_id));
    }

    // --- Rule 1: idempotent on event_id via the dedup ledger ---

    /// A redelivered `event_id` is a no-op: the ledger absorbs it, the handler runs exactly ONCE,
    /// and the redelivery reads `Deduplicated` (0 dup). The core idempotency property (SUB-D2 /
    /// SUB-D1-through-a-consumer).
    #[test]
    fn redelivered_event_id_is_a_no_op_handler_runs_once() {
        let h = done_handler();
        let c = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let m = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");

        assert_eq!(
            c.deliver(&m),
            Delivered::Acked,
            "first delivery runs + acks"
        );
        assert_eq!(
            c.deliver(&m),
            Delivered::Deduplicated,
            "redelivery is deduped"
        );
        assert_eq!(c.deliver(&m), Delivered::Deduplicated, "and again");
        assert_eq!(
            c.handler.runs.load(Ordering::SeqCst),
            1,
            "the handler ran EXACTLY once"
        );
        assert_eq!(c.dedup().len(), 1, "one (consumer, event_id) pair recorded");
    }

    /// The dedup PK is `(consumer, event_id)`: the SAME event delivered to TWO consumers (sharing
    /// the ledger) is fresh for EACH — each processes it once. A per-consumer key, not global.
    #[test]
    fn dedup_key_is_per_consumer_two_consumers_each_process_once() {
        let ledger = DedupLedger::new();
        let a = Consumer::new(
            done_handler(),
            sub("indexer", &["myelin://acme/issues/"]),
            ledger.clone(),
        );
        let b = Consumer::new(
            done_handler(),
            sub("notifier", &["myelin://acme/issues/"]),
            ledger.clone(),
        );
        let m = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");

        assert_eq!(a.deliver(&m), Delivered::Acked, "consumer A processes it");
        assert_eq!(
            b.deliver(&m),
            Delivered::Acked,
            "consumer B ALSO processes it (different PK)"
        );
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
        assert!(
            !ledger.is_empty(),
            "the ledger is no longer empty after a mark"
        );
        assert!(
            ledger.is_handled(&consumer, &id),
            "the exact pair is handled"
        );
        // a different consumer's view of the same id is still unhandled (per-consumer PK).
        assert!(!ledger.is_handled(&ConsumerName("other".into()), &id));
        // a re-mark of the same pair is NOT fresh (the idempotency no-op).
        assert!(
            !ledger.mark_handled(&consumer, &id),
            "re-mark is a duplicate, not fresh"
        );
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
            let c = Consumer::new(
                done_handler(),
                sub("indexer", &["myelin://acme/issues/"]),
                ledger.clone(),
            );
            assert_eq!(c.deliver(&m1), Delivered::Acked);
            // broker drops here — m2 was never delivered.
        }

        // Reconnect: SAME name, SAME ledger. The broker redelivers BOTH m1 and m2 (at-least-once).
        let h = done_handler();
        let c2 = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/issues/"]),
            ledger.clone(),
        );
        assert_eq!(
            c2.deliver(&m1),
            Delivered::Deduplicated,
            "m1 already handled → 0 dup"
        );
        assert_eq!(
            c2.deliver(&m2),
            Delivered::Acked,
            "m2 handled after reconnect → 0 lost"
        );
        assert_eq!(
            c2.handler.runs.load(Ordering::SeqCst),
            1,
            "only m2 re-ran the handler"
        );
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
        let c = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let poison = msg("01J-bad", "myelin://acme/issues/issue/PROJ-1");

        let out = c.deliver(&poison);
        assert_eq!(out, Delivered::DeadLettered(Reason("malformed".into())));
        assert_eq!(
            c.dead_letters().len(),
            1,
            "the poison is SURFACED, not silently dropped"
        );
        assert_eq!(c.dead_letters()[0].reason, Reason("malformed".into()));
        assert_eq!(
            c.lag(),
            0,
            "a dead-lettered message does not sit in lag (it is terminal)"
        );

        // a SECOND delivery of the same poison is deduped (its mark stayed) — it does not
        // re-poison / re-burn anything.
        assert_eq!(
            c.deliver(&poison),
            Delivered::Deduplicated,
            "a redelivered dead-letter is deduped"
        );
        assert_eq!(
            c.dead_letters().len(),
            1,
            "still exactly one dead-letter (not re-poisoned)"
        );
    }

    /// A poison is terminal only after its durable DLQ row commits. If that storage write fails,
    /// delivery returns Retry, leaves the dedup mark absent, and keeps lag/in-flight non-zero so a
    /// service-level broker pump must withhold its ack.
    #[test]
    fn durable_dead_letter_failure_is_retryable_never_terminally_acked() {
        struct FailingDlq;
        impl crate::DurableDeadLetter for FailingDlq {
            fn record(
                &self,
                _consumer: &ConsumerName,
                _event_id: &crate::EventId,
                _reason: &str,
            ) -> Result<(), String> {
                Err("durable DLQ unavailable".into())
            }

            fn dead_letters(&self, _consumer: &ConsumerName) -> Vec<crate::DeadLetterRecord> {
                Vec::new()
            }
        }

        let h = CountingHandler {
            runs: AtomicU32::new(0),
            subjects: SUBJECTS,
            outcome: |_| HandleOutcome::NonRetryable(Reason("malformed".into())),
        };
        let ledger = DedupLedger::new();
        let c = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/issues/"]),
            ledger.clone(),
        )
        .with_dead_letter_sink(DeadLetterSink::durable(Arc::new(FailingDlq)));
        let poison = msg("01J-dlq-down", "myelin://acme/issues/issue/PROJ-1");

        assert_eq!(
            c.deliver(&poison),
            Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS),
            "a failed durable quarantine write is non-terminal"
        );
        assert!(!ledger.is_handled(c.name(), &poison.envelope.event_id));
        assert_eq!(c.lag(), 1, "the unacked poison remains visible as lag");
        assert_eq!(
            c.dead_letters().len(),
            1,
            "the process-local fallback remains operator-visible without authorizing ack"
        );
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
        assert_eq!(
            c.deliver(&b),
            Delivered::Acked,
            "subject B is not head-of-line-blocked by A"
        );
        assert_eq!(
            c.lag_on("myelin://acme/A/x"),
            0,
            "the poison subject did not accumulate lag"
        );
        assert_eq!(c.lag_on("myelin://acme/B/y"), 0, "B drained");
    }

    // --- Rule 2: ack-after-enqueue (a Retry is NOT acked — 0 lost) ---

    /// A `Retry` does NOT ack: the message stays pending (lag rises), the dedup mark is REVERTED,
    /// and a later redelivery RE-RUNS the handler (a transient failure is never swallowed — that
    /// would be silent data loss). When the redelivery succeeds, lag clears.
    #[test]
    fn multiple_retries_track_one_pending_event_then_success_clears_lag() {
        // The handler fails (Retry) twice, then succeeds.
        struct Flaky {
            calls: AtomicU32,
        }
        impl EventHandler for Flaky {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, _ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
                if self.calls.fetch_add(1, Ordering::SeqCst) < 2 {
                    HandleOutcome::Retry(Backoff { seconds: 2 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
        let c = Consumer::new(
            Flaky {
                calls: AtomicU32::new(0),
            },
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let m = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");

        // first delivery: Retry → NOT acked → pending lag rises, dedup mark reverted.
        assert_eq!(c.deliver(&m), Delivered::Retried(2));
        assert_eq!(c.lag(), 1, "an un-acked retry sits in consumer lag");
        assert!(
            !c.dedup().is_handled(c.name(), &m.envelope.event_id),
            "a retry leaves NO dedup mark"
        );

        // A second redelivery is the SAME pending event, not another unit of lag.
        assert_eq!(c.deliver(&m), Delivered::Retried(2));
        assert_eq!(c.lag(), 1, "redelivery does not double-count pending lag");

        // Third delivery: the handler RE-RUNS (0 lost) and now succeeds → acked, lag clears.
        assert_eq!(
            c.deliver(&m),
            Delivered::Acked,
            "the redelivery re-ran the handler and succeeded"
        );
        assert_eq!(
            c.lag(),
            0,
            "lag recovers to 0 after the successful redelivery (SUB-D2)"
        );
        assert!(
            c.dedup().is_handled(c.name(), &m.envelope.event_id),
            "now it is durably handled"
        );
    }

    // --- Rule 6: bounded prefetch ---

    /// `PrefetchBound::new(0)` is rejected (a zero prefetch would consume nothing); a positive
    /// bound is admitted and read back.
    #[test]
    fn prefetch_bound_rejects_zero() {
        assert_eq!(
            PrefetchBound::new(0),
            None,
            "a zero prefetch is meaningless, rejected"
        );
        assert_eq!(PrefetchBound::new(8).unwrap().get(), 8);
        assert_eq!(PrefetchBound::DEFAULT.get(), 64);
    }

    /// `deliver_lane` honours the bounded prefetch (rule 6): with a bound of 2, a lane of 5
    /// delivers only the first 2 this drain; the rest are left for the next drain (lag, not loss).
    #[test]
    fn deliver_lane_honours_bounded_prefetch() {
        let bound = PrefetchBound::new(2).unwrap();
        let s = Subscription::bind(
            ConsumerName("indexer".into()),
            &["myelin://acme/issues/"],
            bound,
        )
        .unwrap();
        let c = Consumer::new(done_handler(), s, DedupLedger::new());

        let lane: Vec<Message> = (0..5)
            .map(|i| msg(&format!("01J-{i}"), "myelin://acme/issues/issue/PROJ-1"))
            .collect();
        let out = c.deliver_lane("myelin://acme/issues/issue/PROJ-1", &lane);
        assert_eq!(
            out.len(),
            2,
            "bounded prefetch: only 2 of 5 delivered this drain"
        );
        assert!(out.iter().all(|o| *o == Delivered::Acked));
        assert_eq!(
            c.handler.runs.load(Ordering::SeqCst),
            2,
            "the handler ran exactly twice"
        );
    }

    // --- The dedup ledger (contract 2.5) lives in `crate::dedup` (EB-06) — see its tests there
    //     for the migration shape + the per-consumer idempotency proofs; here we exercise the
    //     ledger only THROUGH the runtime (rules 1 + 4 above). ---

    // --- The upcaster pre-handle hook (P-S09 floor) ---

    /// The upcaster runs BEFORE handle (P-S09 plugs the registry here via `into_hook`): a
    /// `with_upcaster` that rewrites `schema_ver` 1 → 2 means the handler sees the upcasted shape.
    /// With no registry installed the default hook is the identity map.
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
            fn handle(&self, ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
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
            // model a v1 → v2 upcaster (the real registry is P-S09's UpcasterRegistry).
            if e.schema_ver == 1 {
                e.schema_ver = 2;
            }
            Ok(e)
        });
        c.deliver(&msg("01J-1", "myelin://acme/issues/issue/PROJ-1"));
        assert_eq!(
            seen_ver.load(Ordering::SeqCst),
            2,
            "the handler saw the upcasted schema_ver"
        );
    }

    /// **Contract 2.8 rule 2 through the runtime:** an UNBRIDGEABLE version gap (the upcaster hook
    /// returns `Err`) is a LOUD dead-letter (DLQ), never a silent pass and never handed to the
    /// handler at the wrong shape. The handler MUST NOT run; the message is surfaced on the
    /// dead-letter list; the dedup ledger is NOT marked (a later redelivery, after the missing
    /// upcaster ships, can still be processed — forward-only, not a tombstone).
    #[test]
    fn unbridgeable_gap_dead_letters_loudly_never_silently_passes() {
        let h = done_handler();
        let c = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        )
        .with_upcaster(|_e| Err(Reason("unbridgeable schema gap: no upcaster".into())));

        let m = msg("01J-gap", "myelin://acme/issues/issue/PROJ-1");
        let out = c.deliver(&m);

        assert!(
            matches!(out, Delivered::DeadLettered(_)),
            "a gap dead-letters → DLQ"
        );
        assert_eq!(
            c.handler.runs.load(Ordering::SeqCst),
            0,
            "the handler never saw the wrong shape"
        );
        assert_eq!(
            c.dead_letters().len(),
            1,
            "the gap is surfaced, not silently dropped"
        );
        // The dedup ledger was NOT marked: re-delivering the SAME event after the upcaster ships
        // is not deduplicated away. Prove it by re-installing an identity hook and re-delivering.
        assert!(
            c.dedup().mark_handled(c.name(), &EventId("01J-gap".into())),
            "the gapped event_id is still FRESH — it was never marked handled"
        );
    }

    /// An off-whitelist message routed to the consumer is surfaced as a dead-letter, never
    /// silently processed (defence in depth around rule 3).
    #[test]
    fn off_whitelist_message_is_dead_lettered_not_silently_processed() {
        let c = Consumer::new(
            done_handler(),
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let off = msg("01J-off", "myelin://acme/chat/message/1");
        assert!(matches!(c.deliver(&off), Delivered::DeadLettered(_)));
        assert_eq!(
            c.handler.runs.load(Ordering::SeqCst),
            0,
            "the handler never ran for an off-whitelist subject"
        );
        assert_eq!(c.dead_letters().len(), 1);
    }

    #[test]
    fn off_whitelist_dlq_failure_then_success_clears_pending_lag() {
        struct FailOnce(AtomicBool);
        impl crate::DurableDeadLetter for FailOnce {
            fn record(
                &self,
                _consumer: &ConsumerName,
                _event_id: &crate::EventId,
                _reason: &str,
            ) -> Result<(), String> {
                if self.0.swap(false, Ordering::SeqCst) {
                    Err("DLQ unavailable".into())
                } else {
                    Ok(())
                }
            }
            fn dead_letters(&self, _consumer: &ConsumerName) -> Vec<crate::DeadLetterRecord> {
                Vec::new()
            }
        }
        let c = Consumer::new(
            done_handler(),
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        )
        .with_dead_letter_sink(DeadLetterSink::durable(Arc::new(FailOnce(AtomicBool::new(
            true,
        )))));
        let off = msg("01J-off-retry", "myelin://acme/chat/message/1");

        assert!(matches!(c.deliver(&off), Delivered::Retried(_)));
        assert_eq!(c.lag(), 1);
        assert!(matches!(c.deliver(&off), Delivered::DeadLettered(_)));
        assert_eq!(c.lag(), 0);
    }

    // --- EB-05: the `consume(ConsumerSpec{ … }, handler)` entry-point ---

    /// `consume(ConsumerSpec, …)` is the one sanctioned entry-point: it validates the `*`-free
    /// whitelist (rule 3), binds the durable name (rule 4), and wires the bounded prefetch +
    /// per-tenant cap (rule 6). A wildcard subject is REJECTED at registration, exactly as the
    /// hand-wired `Subscription::bind` path is — the spec cannot smuggle in an over-broad consumer.
    #[test]
    fn consume_entrypoint_validates_whitelist_and_wires_the_runtime() {
        // a wildcard in the spec is rejected loudly.
        let bad = ConsumerSpec::new(ConsumerName("indexer".into()), &["issues.*"]);
        assert!(matches!(
            consume(bad, done_handler(), DedupLedger::new()),
            Err(SubscribeError::WildcardSubject(_))
        ));

        // a concrete whitelist is admitted and the runtime carries the spec's caps.
        let spec = ConsumerSpec {
            durable: ConsumerName("indexer".into()),
            subjects: vec!["myelin://acme/issues/".into()],
            max_ack_pending: PrefetchBound::new(8).unwrap(),
            per_tenant_inflight: PerTenantInflight::new(4).unwrap(),
        };
        let c = consume(spec, done_handler(), DedupLedger::new()).unwrap();
        assert_eq!(c.name(), &ConsumerName("indexer".into()));
        assert_eq!(
            c.per_tenant_inflight_cap().get(),
            4,
            "the per-tenant cap is wired from the spec"
        );

        // the wired consumer actually processes a whitelisted message.
        assert_eq!(
            c.deliver(&msg("01J-1", "myelin://acme/issues/issue/PROJ-1")),
            Delivered::Acked
        );
    }

    /// `PerTenantInflight::new(0)` is rejected (a zero cap lets no tenant progress); a positive cap
    /// is admitted and read back; the default is 16.
    #[test]
    fn per_tenant_inflight_rejects_zero() {
        assert_eq!(
            PerTenantInflight::new(0),
            None,
            "a zero per-tenant cap is meaningless, rejected"
        );
        assert_eq!(PerTenantInflight::new(4).unwrap().get(), 4);
        assert_eq!(PerTenantInflight::DEFAULT.get(), 16);
    }

    // --- EB-05 rule 6: per-tenant in-flight fairness cap (the agent-surge guard) ---

    /// **A surging tenant is bounded to its per-tenant in-flight cap; a second tenant is NOT
    /// starved.** With a cap of 2, a tenant whose messages keep RETRYING fills its 2 in-flight
    /// slots, and the 3rd is THROTTLED (deferred, not dropped). A DIFFERENT tenant's message still
    /// ACKs — fairness held, the surge did not push the other tenant out.
    #[test]
    fn surging_tenant_is_throttled_at_its_cap_other_tenant_flows() {
        // "surge" tenant always retries (its work stays outstanding → slots accumulate);
        // every other tenant is Done immediately.
        struct SurgeHandler;
        impl EventHandler for SurgeHandler {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
                if ev.tenant.0 == "surge" {
                    HandleOutcome::Retry(Backoff { seconds: 5 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
        let spec = ConsumerSpec {
            durable: ConsumerName("indexer".into()),
            subjects: vec!["myelin://".into()],
            max_ack_pending: PrefetchBound::DEFAULT,
            per_tenant_inflight: PerTenantInflight::new(2).unwrap(),
        };
        let c = consume(spec, SurgeHandler, DedupLedger::new()).unwrap();

        let surge = |id: &str| Message {
            subject: "myelin://surge/issues/x".into(),
            envelope: tenant_envelope(id, "myelin://surge/issues/x", "surge"),
        };

        // The surge tenant's first 2 retries occupy its 2 in-flight slots.
        assert_eq!(c.deliver(&surge("01J-s1")), Delivered::Retried(5));
        assert_eq!(c.deliver(&surge("01J-s2")), Delivered::Retried(5));
        assert_eq!(
            c.tenant_inflight(&TenantId("surge".into())),
            2,
            "the surge tenant holds its 2 slots"
        );

        // The 3rd surge message is THROTTLED (deferred, not dropped) — at cap.
        assert_eq!(
            c.deliver(&surge("01J-s3")),
            Delivered::Throttled(TenantId("surge".into())),
            "the surge tenant is bounded to its cap"
        );
        assert_eq!(
            c.tenant_inflight(&TenantId("surge".into())),
            2,
            "still 2 — the throttled message took no slot"
        );

        // A DIFFERENT tenant's message still ACKs — the surge did NOT starve it (fairness).
        let other = Message {
            subject: "myelin://other/issues/y".into(),
            envelope: tenant_envelope("01J-o1", "myelin://other/issues/y", "other"),
        };
        assert_eq!(
            c.deliver(&other),
            Delivered::Acked,
            "the other tenant is not starved by the surge"
        );
        assert_eq!(
            c.tenant_inflight(&TenantId("other".into())),
            0,
            "the other tenant's Done released its slot"
        );
    }

    /// A throttled message is DEFERRED, not lost: once the surging tenant's slots free (its work
    /// finally completes), the previously-throttled message is re-offered and processed. 0 loss.
    #[test]
    fn throttled_message_is_re_offerable_after_the_tenant_drains() {
        // First call per id retries; a re-delivery after the slot frees succeeds.
        struct DrainHandler {
            failed: Mutex<HashSet<String>>,
        }
        impl EventHandler for DrainHandler {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
                let mut f = self.failed.lock().unwrap();
                if f.insert(ev.event_id.0.clone()) {
                    HandleOutcome::Retry(Backoff { seconds: 1 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
        let spec = ConsumerSpec {
            durable: ConsumerName("indexer".into()),
            subjects: vec!["myelin://surge/".into()],
            max_ack_pending: PrefetchBound::DEFAULT,
            per_tenant_inflight: PerTenantInflight::new(1).unwrap(),
        };
        let c = consume(
            spec,
            DrainHandler {
                failed: Mutex::new(HashSet::new()),
            },
            DedupLedger::new(),
        )
        .unwrap();
        let m = |id: &str| Message {
            subject: "myelin://surge/x".into(),
            envelope: tenant_envelope(id, "myelin://surge/x", "surge"),
        };

        // m1 retries (holds the single slot); m2 is throttled (cap 1).
        assert_eq!(c.deliver(&m("01J-1")), Delivered::Retried(1));
        assert_eq!(
            c.deliver(&m("01J-2")),
            Delivered::Throttled(TenantId("surge".into()))
        );

        // m1 redelivers and now succeeds → the slot frees.
        assert_eq!(c.deliver(&m("01J-1")), Delivered::Acked);
        assert_eq!(
            c.tenant_inflight(&TenantId("surge".into())),
            0,
            "the slot freed"
        );

        // m2 is re-offered (it was deferred, never dropped): first retry, then it acks. 0 loss.
        assert_eq!(
            c.deliver(&m("01J-2")),
            Delivered::Retried(1),
            "the previously-throttled message is re-offered"
        );
        assert_eq!(
            c.deliver(&m("01J-2")),
            Delivered::Acked,
            "and eventually processed — 0 loss"
        );
    }

    /// Build an envelope for a given tenant (the per-tenant fairness tests key on `envelope.tenant`).
    fn tenant_envelope(id: &str, subject: &str, tenant: &str) -> EventEnvelope {
        let mut e = envelope(id, subject);
        e.tenant = TenantId(tenant.into());
        e
    }
}
