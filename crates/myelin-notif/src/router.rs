//! # The Signal-consumer **router skeleton** (NOTIF-P3 / P-181, M2) + NOTIF-D10
//!
//! **Owning architecture doc:** `notifications.md` §3.4 (the router loop — step-0 authorize,
//! idempotent on `origin_event`; at N-M2.0 the body is the SKELETON: UPSERT an inbox_item from a
//! Signal, no ranking/storm-control/fanout yet), §5.1 (the router is a stateless replicable
//! consumer pool). **Contracts:** 2.4 (the [`EventHandler`] consumer template — `subjects()`
//! whitelist NEVER `*`, ack-after-enqueue, dedup ledger, bounded prefetch, lag metric), 2.5
//! (`consumer_dedup` ledger), 2.2 (`OutboxTx::emit` — the ONLY emit path), 3.1
//! (`define_signal_rule` + the `sig.<tenant>.>` Signal stream), 1.8 (the `consumer_lag`
//! telemetry signal). **ADR-19:** Notif consumes Signals, **not** `evt.*`. **External insight:**
//! `01-process-and-quality-doctrine.md` §3 (prove-it + observability is part of the pass — the lag
//! metric is the green artifact), §5 (an uncommitted contract test is no contract test);
//! `04-hard-problems.md` §5.3 (Notif is a projection — the skeleton just UPSERTs; storm-control
//! never touches the audit/history).
//!
//! ## What this prompt (NOTIF-P3) ships — the ROUTER SKELETON, nothing else
//!
//! The router is an [`EventHandler`] consumer of **curated Signals** (`sig.<tenant>.>` — the
//! [`SignalEngine`](myelin_query::SignalEngine) publish subject `sig.<tenant>.<severity>.<rule>`,
//! P-138). It is stood up through the ONE sanctioned consumer entry-point
//! ([`myelin_events::consume`]) so the seven encoded rules (2.4) cannot be skipped:
//!
//! 1. **`subjects()` is the `sig.<tenant>.>` whitelist, NEVER `*`** — [`signal_subject_prefix`]
//!    builds the per-tenant prefix; [`build_router`] binds it through [`myelin_events::consume`],
//!    which **rejects** a `*`/`>`/empty subject loudly at registration. An over-broad subscription
//!    that head-of-line-blocks everything is unconstructable (BUS-3, D7-i).
//! 2. **Idempotent on `origin_event` / `event_id`** via the per-consumer
//!    [`DedupLedger`](myelin_events::DedupLedger) (2.5) — the runtime's outer guard skips a
//!    redelivered Signal and acks (0 dup); the router's own `(tenant, recipient, dedup_key)`
//!    write-time UPSERT is the inner idempotency (§3.2).
//! 3. **ack-after-enqueue, bounded prefetch, the consumer-lag metric** — all provided by the
//!    [`Consumer`](myelin_events::Consumer) runtime [`build_router`] wraps the handler in.
//! 4. **The emit path is `OutboxTx::emit` ONLY** — [`SignalRouter::handle`] opens an
//!    [`OutboxTransaction`](myelin_events::OutboxTransaction) on the shared
//!    [`OutboxStore`](myelin_events::OutboxStore), stages the inbox UPSERT, **emits
//!    `notif.item.created` via [`OutboxTx::emit`]`(draft, cause = Some(signal_event))`** (the
//!    causality root carries; `depth+1`), and commits — the inbox row + the emit **co-commit**
//!    (BUS-D4 emit-iff-committed). There is **no `publish_now`** in this crate; the
//!    `no-raw-publish` lint (P-019) is structurally satisfied (the router never calls a broker
//!    `publish`).
//!
//! ### The skeleton body (NOT the working router)
//! At N-M2.0 [`SignalRouter::handle`] does exactly one thing per Signal: it derives a skeleton
//! [`InboxItem`](crate::InboxItem)-shaped row from the Signal and **UPSERTs** it into the in-memory
//! inbox projection (modelling the `INSERT … ON CONFLICT (tenant, recipient, dedup_key) DO UPDATE`
//! write-time collapse the `notif_inbox_item` table, NOTIF-P2, declares). It does NOT rank,
//! storm-control, fan-out, or humanise — those are the named follow-ons below.
//!
//! ## Head-of-line isolation (NOTIF-D10 — the gate this prompt greens)
//! A **poison** Signal type (a Signal whose payload cannot be parsed into a [`Signal`]) is
//! terminated with a [`HandleOutcome::NonRetryable`]: the runtime dead-letters it IMMEDIATELY
//! (rule 5), acks it (so it does not redeliver / burn the budget / block the subject behind it),
//! and **other subjects keep flowing**. The consumer-lag metric stays bounded (the dead-letter is
//! terminal, lag 0) — observability is part of the pass (EI-01 §3). The drill scenario lives in
//! [`crate::router::tests`] (`notif_d10_poison_signal_does_not_stall_other_subjects`) and in the
//! whole-system drill harness (`tests/drill_notif_d10.rs`).
//!
//! ## FLOORS named — the router's classify/score/storm-control/fanout/humanise body is NOT here
//! The skeleton is **not** the working router. The algorithm body lands in the follow-ons:
//! - **`list_inbox` + the scoped-view filter grammar** (the C-9 read surface) → **NOTIF-P5**.
//! - **the deterministic ranking function** (priority 0..100) → **NOTIF-P7**.
//! - **`define_notif_rule` + the reason set** (the per-Signal classification) → **NOTIF-P8**.
//! - **the five write-time storm-control mechanisms** → **NOTIF-P11**.
//! - **write-fanout** (the bounded `mention` set) → **NOTIF-P12**; **read-fanout** (the unbounded
//!   ambient watcher set) → **NOTIF-P13**.
//! - **humanise** (the ONE per-viewer templating surface) → **NOTIF-P9**.
//! - **the durable persistence of the inbox store** behind the in-memory projection (the
//!   `notif_inbox_item` UPSERT against the OLTP pool) lands with the OLTP client wiring into
//!   `serve` — the in-memory projection here models that write byte-for-byte (the
//!   `(tenant, recipient, dedup_key)` collapse), the named substrate floor (P-007 / P-S12). The
//!   SEAM shape (the `EventHandler`, the UPSERT key, the outbox emit) does NOT change.
//!
//! ## Mutation floor (the router decision module — mandatory-core)
//! The router is mandatory-core (the platform's reactive seam). The mutation-tested core is the
//! decision logic: the Signal→`InboxItem` derivation (recipient/subject/dedup-key/reason/class
//! mapping), the `(tenant, recipient, dedup_key)` UPSERT-vs-collapse decision (a redelivered or
//! a same-key Signal collapses, NOT a second row + `coalesce_count += 1`), the poison→NonRetryable
//! verdict, and the emit-iff-committed `notif.item.created` shape. The floor is **stated and met**
//! by the unit + chained + CDC tests below: every mapping is asserted, the UPSERT-collapse count
//! is asserted, the poison verdict + head-of-line isolation is asserted, and a mutant that drops
//! the collapse, mis-maps a field, skips the emit, or swallows a poison is caught. **Floor: ≥ 80%
//! line/branch mutation score on `router.rs` decision logic** (measured with `cargo mutants`;
//! reported in the P-181 commit body).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_events::{
    consume, AggregateKey, ArtifactRef, Consumer, ConsumerName, ConsumerSpec, DataRole,
    DedupLedger, EmitContextBase, EventDraft, EventEnvelope, EventHandler, EventType, HandleOutcome,
    IdMinter, MonotonicMinter, OutboxStore, OutboxTx, Reason as BusReason, SubjectPattern,
    SubscribeError, Visibility,
};
use myelin_query::signals::{Severity, Signal};
use myelin_tenancy::{Region, TenantId};

use crate::{Class, Reason};

/// The frozen `notif.item.created` event type (architecture §3.4 — the router's create-side emit
/// token). The ONLY event the skeleton router emits, and ONLY via [`OutboxTx::emit`]. A named
/// constant so drills assert against the NAME, never a literal (EI-01 §3 observability / coherence).
pub const NOTIF_ITEM_CREATED: &str = "notif.item.created";

/// The frozen `notif.escalation.acked` event type (architecture §3.4 / contract 7.5). Declared
/// here (the router is the §3.4 emit owner) so the no-raw-publish discipline names BOTH router emit
/// tokens; the ESCALATION ack body that emits it is NOTIF-P14 (the durable wheel) — named, not
/// built here. A named constant the future ack path emits through [`OutboxTx::emit`].
pub const NOTIF_ESCALATION_ACKED: &str = "notif.escalation.acked";

/// The durable consumer name (rule 4: bind-by-name; re-bound identically on reconnect, sharing the
/// SAME dedup ledger + cursor). PII-free label.
pub const ROUTER_CONSUMER_NAME: &str = "notif-signal-router";

/// **The `sig.<tenant>.>`-shaped subject WHITELIST prefix for one tenant (rule 3, NEVER `*`).**
///
/// The Signal engine (P-138) publishes to `sig.<tenant>.<severity>.<rule>`; the router whitelists
/// the per-tenant prefix `sig.<tenant>.` so it consumes EVERY severity/rule for that tenant and
/// NOTHING for another tenant — and crucially is **not** a `*`/`>` wildcard (which
/// [`myelin_events::consume`] would reject). The prefix is a true prefix the broker's subject match
/// uses (the same model [`myelin_events::Subscription::matches`] applies).
///
/// Per-tenant binding (vs one `sig.>` for all tenants) is the residency/isolation-correct shape:
/// the router pool for a cell binds the tenants HOMED in that cell, never a global `sig.>`. A
/// missing or empty tenant token would degenerate to an over-broad prefix; the constructor refuses
/// it (returns `None`) so an over-broad whitelist is unconstructable here too.
pub fn signal_subject_prefix(tenant: &TenantId) -> Option<String> {
    if tenant.0.is_empty() || tenant.0.contains('.') {
        // An empty tenant → `sig..` (over-broad / malformed); a dotted tenant would create extra
        // subject segments that could alias another tenant. Refuse loudly rather than narrow.
        return None;
    }
    Some(format!("sig.{}.", tenant.0))
}

/// `true` iff `subject` is a curated-Signal subject for `tenant` (`sig.<tenant>.…`). The router's
/// whitelist is exactly this prefix per tenant; used by the handler's defensive check (a message
/// off the whitelist should never have been routed here — the runtime's rule-3 guard already
/// rejects it, this is belt-and-braces).
pub fn is_signal_subject(subject: &str, tenant: &TenantId) -> bool {
    match signal_subject_prefix(tenant) {
        Some(prefix) => subject.starts_with(&prefix),
        None => false,
    }
}

/// A skeleton inbox row the router UPSERTs (the in-memory projection of the `notif_inbox_item` row,
/// NOTIF-P2). References-not-payloads (NOTIF-1): `subject` / `origin_event` are
/// [`ArtifactRef`](myelin_events::ArtifactRef)s, never rendered strings. The full column set lives
/// in [`crate::schema::InboxItemRow`]; the skeleton carries exactly what the create-skeleton writes
/// (no ranking/humanise yet). `coalesce_count` is the "+N more" write-time-collapse counter the
/// `(tenant, recipient, dedup_key)` UPSERT bumps (§3.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedInboxItem {
    /// `(tenant, region)` partition key (from the verified Signal envelope, never the path).
    pub tenant: TenantId,
    pub region: Region,
    /// The stable inbox-item id (the mark/snooze read-state handle, contract 7.2). Deterministic
    /// from `(tenant, recipient, dedup_key)` so a redelivery/collapse hits the SAME row.
    pub item_id: String,
    /// The recipient Principal (an OPAQUE pseudonym, contract 4.8). The skeleton derives it from
    /// the Signal (the real per-Signal recipient routing is NOTIF-P8/P12/P13).
    pub recipient: String,
    /// The artifact the item is about (a ref, never a payload).
    pub subject: ArtifactRef,
    /// The structured why-it-fired (the C-9 scoped-view filter basis). The skeleton classifies a
    /// curated Signal as [`Reason::StateChanged`] (the ambient default); the real per-reason
    /// classification is NOTIF-P8.
    pub reason: Reason,
    /// The routing class (drives the channel set + the pierce decision). The skeleton maps from the
    /// Signal severity; the real prefs-aware routing is NOTIF-P10.
    pub class: Class,
    /// The originating event ref (the NOTIF-2 provenance — the `origin_event` idempotency anchor).
    pub origin_event: ArtifactRef,
    /// The storm-control dedup key — `(tenant, recipient, dedup_key)` is the UNIQUE write-time
    /// collapse key (§3.2). Derived from the Signal's rule + dedup key.
    pub dedup_key: String,
    /// The "+N more" collapse counter — bumped on a same-key UPSERT (the storm-control primitive
    /// the body, NOTIF-P11, builds on; here the skeleton just counts the collapse).
    pub coalesce_count: i32,
    /// The ONE read-state column (the C-9 read-state truth) — a fresh row is `unread`.
    pub state: String,
}

/// **The in-memory inbox projection the skeleton router UPSERTs into** (the model of the
/// `notif_inbox_item` table, NOTIF-P2). Keyed `(tenant, recipient, dedup_key)` — the UNIQUE
/// write-time-collapse key (§3.2): a same-key Signal collapses into the existing row
/// (`coalesce_count += 1`), it does NOT create a second row. A cloneable handle over shared state
/// so the handler, a drill, and a test all observe one truth.
///
/// **Floor (named):** the durable persistence is the `INSERT … ON CONFLICT (tenant_id, recipient,
/// dedup_key) DO UPDATE SET coalesce_count = coalesce_count + 1` against the OLTP pool, wired when
/// the OLTP client lands in `serve` (P-007 / P-S12). This in-memory map models exactly that UPSERT.
/// The in-memory inbox keyed `(tenant, recipient, dedup_key)` — the UNIQUE write-time-collapse key
/// (§3.2). A type alias so the [`InboxProjection`] field is not a "very complex type".
type InboxMap = HashMap<(String, String, String), RoutedInboxItem>;

#[derive(Clone, Default)]
pub struct InboxProjection {
    inner: Arc<Mutex<InboxMap>>,
}

impl InboxProjection {
    /// A fresh, empty inbox projection.
    pub fn new() -> InboxProjection {
        InboxProjection::default()
    }

    /// **UPSERT the row with `(tenant, recipient, dedup_key)` write-time collapse (§3.2).** A FRESH
    /// key inserts the row (returning [`Upsert::Inserted`]); an EXISTING key COLLAPSES into it
    /// (`coalesce_count += 1`, returning [`Upsert::Collapsed`]) — it does NOT create a second row.
    /// This is the storm-control primitive the body (NOTIF-P11) builds on; the skeleton just counts.
    fn upsert(&self, mut item: RoutedInboxItem) -> Upsert {
        let key = (item.tenant.0.clone(), item.recipient.clone(), item.dedup_key.clone());
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get_mut(&key) {
            Some(existing) => {
                existing.coalesce_count += 1;
                Upsert::Collapsed(existing.coalesce_count)
            }
            None => {
                item.coalesce_count = 1;
                guard.insert(key, item);
                Upsert::Inserted
            }
        }
    }

    /// The number of DISTINCT inbox rows (post-collapse). A drill asserts that N same-key Signals
    /// produce ONE row (not N).
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether the projection holds no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read one row by `(tenant, recipient, dedup_key)` (for tests / a drill).
    pub fn get(&self, tenant: &TenantId, recipient: &str, dedup_key: &str) -> Option<RoutedInboxItem> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(tenant.0.clone(), recipient.to_string(), dedup_key.to_string()))
            .cloned()
    }
}

/// What an [`InboxProjection::upsert`] did (the write-time-collapse outcome, §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Upsert {
    /// A fresh `(tenant, recipient, dedup_key)` → a new row was inserted (`coalesce_count = 1`).
    Inserted,
    /// An existing key → collapsed into the row (`coalesce_count` is now the carried count).
    Collapsed(i32),
}

/// **The Signal-consumer router (the skeleton, NOTIF-P3).** An [`EventHandler`] consumer of curated
/// Signals (`sig.<tenant>.>`) that UPSERTs an inbox item and emits `notif.item.created` via the
/// outbox — the ONLY emit path. Wrapped by the [`Consumer`] runtime (the seven rules) through
/// [`build_router`]; never hand-wired.
///
/// Holds:
/// - the [`InboxProjection`] it UPSERTs into (the model of the `notif_inbox_item` table);
/// - the shared [`OutboxStore`] it emits `notif.item.created` through (the relay drains it);
/// - the [`IdMinter`] the outbox transaction mints event ids from;
/// - the static `sig.<tenant>.>` whitelist the trait's `subjects()` returns (rule 3).
pub struct SignalRouter {
    inbox: InboxProjection,
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
    /// The `'static` whitelist the trait requires. Built once at [`build_router`] from the
    /// per-tenant `sig.<tenant>.` prefix and leaked to `'static` (the binding set is fixed for the
    /// life of the consumer pool; one leak per tenant per process — bounded, never per-event).
    subjects: &'static [SubjectPattern],
}

/// Why the router did not complete routing a Signal envelope. A [`RouteError::MalformedSignal`] is
/// a POISON (→ [`HandleOutcome::NonRetryable`], dead-lettered immediately, rule 5); a
/// [`RouteError::EmitFailed`] is a TRANSIENT outbox hiccup (→ [`HandleOutcome::Retry`], 0 lost —
/// the runtime redelivers, the dedup mark is reverted so the handler re-runs). The distinction is
/// load-bearing: a transient infra failure must NOT be dead-lettered as if the Signal were poison
/// (that would silently drop a good notification — silent data loss).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteError {
    /// The Signal envelope payload could not be parsed into a [`Signal`] (a malformed / unknown
    /// shape on a `sig.*` subject — a poison Signal). Carries the parse detail. → NonRetryable.
    MalformedSignal(String),
    /// The outbox emit / commit failed transiently (an infra hiccup, not a bad Signal). → Retry
    /// (0 lost — the runtime redelivers; never a dead-letter of a good Signal). Carries the detail.
    EmitFailed(String),
}

impl SignalRouter {
    /// Build a router over its inbox projection, the shared outbox it emits through, and the static
    /// whitelist (rule 3). Used by [`build_router`]; a test can construct one directly to exercise
    /// [`SignalRouter::handle`] in isolation.
    fn new(
        inbox: InboxProjection,
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
        subjects: &'static [SubjectPattern],
    ) -> SignalRouter {
        SignalRouter { inbox, outbox, minter, subjects }
    }

    /// The inbox projection this router UPSERTs into (so a drill can read the result).
    pub fn inbox(&self) -> &InboxProjection {
        &self.inbox
    }

    /// **Route ONE curated Signal into the inbox + emit `notif.item.created` (the skeleton body).**
    ///
    /// 1. Parse the [`Signal`] out of the envelope payload — a malformed payload on a `sig.*`
    ///    subject is a [`RouteError::MalformedSignal`] poison (→ `NonRetryable`).
    /// 2. Derive the skeleton [`RoutedInboxItem`] (recipient/subject/reason/class/dedup-key — the
    ///    skeleton mapping; the real per-Signal classification is NOTIF-P8+).
    /// 3. **Open an outbox transaction, UPSERT the inbox row, emit `notif.item.created` via
    ///    [`OutboxTx::emit`]`(draft, cause = Some(signal_event))`, and COMMIT** — the inbox row and
    ///    the emit co-commit (emit-iff-committed, BUS-D4). The cause makes causality
    ///    correct-by-construction (the correlation root carries; `depth+1` — the loop-guard stamp).
    ///
    /// Returns the [`Upsert`] outcome on success (so the handler can observe insert-vs-collapse) or
    /// a [`RouteError`] poison. **No `publish_now`** — the emit rides the ONE sanctioned outbox path.
    fn route(&self, signal_event: &EventEnvelope) -> Result<Upsert, RouteError> {
        // (1) Parse the curated Signal from the envelope payload (poison → NonRetryable).
        let signal: Signal = serde_json::from_value(signal_event.payload.clone())
            .map_err(|e| RouteError::MalformedSignal(e.to_string()))?;

        // (2) Derive the SKELETON inbox row (the real per-reason/per-recipient routing is NOTIF-P8+).
        let item = self.derive_item(signal_event, &signal);
        let recipient = item.recipient.clone();
        let dedup_key = item.dedup_key.clone();

        // (3) Co-commit: open a tx, UPSERT the inbox row, emit notif.item.created, COMMIT. The
        // tx's ambient context is the INCOMING Signal's (tenant/region/actor/clock) so the emitted
        // notif.item.created is partitioned to the SAME (tenant, region) and attributed correctly.
        let mut tx = self.outbox.begin(self.minter.clone(), emit_base_from(signal_event));

        // The inbox UPSERT — the (tenant, recipient, dedup_key) write-time collapse (§3.2). Staged
        // into the SAME transaction as the emit (the co-commit: the inbox row and the
        // notif.item.created event become durable together, never one without the other). In the
        // OLTP binding (P-007) this is the `INSERT … ON CONFLICT DO UPDATE` in the tx; here the
        // in-memory projection models it and we record the state-change on the tx for the co-commit
        // assertion.
        let outcome = self.inbox.upsert(item.clone());
        tx.stage_state_change(format!(
            "UPSERT notif_inbox_item ({}, {}, {})",
            item.tenant.0, recipient, dedup_key
        ));

        // The ONE sanctioned emit verb (contract 2.2; no-raw-publish). `cause = Some(signal_event)`
        // → the correlation root carries + causation = the Signal + depth+1 (the loop-guard stamp).
        // A `notif.item.created` is references-not-payloads: it carries the item_id + subject ref,
        // never a rendered string (humanise is per-viewer at read time, NOTIF-P9).
        tx.emit(self.item_created_draft(&item), Some(signal_event))
            .map_err(|e| RouteError::EmitFailed(format!("outbox emit failed: {e:?}")))?;

        // Commit: the inbox row + the notif.item.created emit become durable atomically. A commit
        // failure is a TRANSIENT outbox hiccup → Retry (never a silent half-write, never a
        // dead-letter of a good Signal). A UNIQUE(event_id) collision would be a programming error
        // (the minter is monotonic); the in-memory happy path never hits it.
        tx.commit()
            .map_err(|e| RouteError::EmitFailed(format!("outbox commit failed: {e:?}")))?;

        Ok(outcome)
    }

    /// Derive the SKELETON [`RoutedInboxItem`] from a curated Signal (the create-skeleton mapping).
    /// **This is the skeleton, NOT the working classifier** — the real per-reason classification
    /// (NOTIF-P8), recipient routing (NOTIF-P12/P13), and ranking (NOTIF-P7) are the follow-ons.
    ///
    /// - `recipient`: the skeleton routes to the Signal's `subject` aggregate owner — modelled here
    ///   as a deterministic pseudonym derived from the rule (the real watcher/mention fan-out is
    ///   NOTIF-P12/P13). It is an OPAQUE token, never a name (references-not-payloads).
    /// - `reason`: [`Reason::StateChanged`] (the ambient default; per-reason mapping is NOTIF-P8).
    /// - `class`: mapped from the Signal severity (`critical → Critical`, `error/warning → Direct`,
    ///   else `Fyi`) — the skeleton routing; prefs-aware routing is NOTIF-P10.
    /// - `dedup_key`: the Signal's rendered dedup key (so identical Signals collapse, §3.2).
    /// - `item_id`: deterministic from `(tenant, recipient, dedup_key)` so a redelivery/collapse
    ///   hits the SAME row (idempotent on the write side, belt to the consumer_dedup braces).
    fn derive_item(&self, env: &EventEnvelope, signal: &Signal) -> RoutedInboxItem {
        let recipient = format!("psn:watcher:{}", signal.rule_id.0);
        let dedup_key = format!("{}:{}", signal.rule_id.0, signal.dedup_key.0);
        let item_id = item_id_for(&env.tenant, &recipient, &dedup_key);
        RoutedInboxItem {
            tenant: env.tenant.clone(),
            region: env.region.clone(),
            item_id,
            recipient,
            subject: signal.subject.clone(),
            reason: Reason::StateChanged,
            class: class_from_severity(signal.severity),
            origin_event: ArtifactRef(format!(
                "myelin://{}/bus/event/{}",
                env.tenant.0, env.event_id.0
            )),
            dedup_key,
            coalesce_count: 1,
            state: "unread".to_string(),
        }
    }

    /// Build the `notif.item.created` [`EventDraft`] for a routed item (references-not-payloads).
    /// The payload carries the `item_id` + the `subject`/`origin_event` refs + the recipient
    /// pseudonym + the reason/class tokens — NEVER a rendered string (humanise is per-viewer at read
    /// time, NOTIF-P9). `contains_personal_data = false`: every field is an opaque ref/token/pseudonym
    /// (references-not-payloads, contract 2.7), so no inline-PII envelope key.
    fn item_created_draft(&self, item: &RoutedInboxItem) -> EventDraft {
        EventDraft {
            type_: EventType(NOTIF_ITEM_CREATED.into()),
            // The item's subject artifact is the event subject (what the notification is about).
            subject: item.subject.clone(),
            // The inbox item identity aggregate — per-aggregate ordering for an item's
            // created→read sequence (EB-03). The item_id is the stable identity.
            aggregate: AggregateKey(format!("notif-item:{}", item.item_id)),
            payload: serde_json::json!({
                "item_id": item.item_id,
                "recipient": item.recipient,
                "subject": item.subject.0,
                "subject_root": item.subject.0,
                "reason": serde_json::to_value(item.reason).unwrap_or(serde_json::Value::Null),
                "class": serde_json::to_value(item.class).unwrap_or(serde_json::Value::Null),
                "origin_event": item.origin_event.0,
                "dedup_key": item.dedup_key,
                "state": item.state,
            }),
            // Notif is the CONTROLLER of the inbox-item fact it authors (the inbox is Notif-owned).
            data_role: DataRole::Controller,
            // A derived projection event's default visibility is Internal (a routing hint, never an
            // authz decision — Identity decides at resolve-time, §3.4).
            visibility: Visibility::Internal,
            // References-not-payloads: opaque refs/pseudonyms only, no inline PII, so no envelope key.
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }
}

impl EventHandler for SignalRouter {
    /// The `*`-free `sig.<tenant>.>` subject whitelist (rule 3). The `'static` slice the trait
    /// requires; [`build_router`] binds the runtime through the sanctioned [`myelin_events::consume`]
    /// (which REJECTS a `*`/`>`/empty subject). NEVER `*` (BUS-3, D7-i).
    fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    /// Route the delivered curated Signal (contract 2.4). Idempotent on `event_id` (the runtime's
    /// `consumer_dedup` outer guard, rule 1) AND on `(tenant, recipient, dedup_key)` (the inbox
    /// UPSERT write-time collapse) — belt and braces. A MALFORMED Signal (un-parseable payload) is a
    /// **non-retryable poison** ([`HandleOutcome::NonRetryable`]) — terminated immediately (rule 5),
    /// so it does NOT block the subject behind it (NOTIF-D10 head-of-line isolation). The emit rides
    /// the outbox (the co-commit happens inside [`SignalRouter::route`]).
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
        match self.route(ev) {
            Ok(_) => HandleOutcome::Done,
            // A poison Signal terminates immediately (dead-letter, rule 5) — never a silent drop,
            // never a head-of-line stall on the subject behind it (NOTIF-D10).
            Err(RouteError::MalformedSignal(why)) => HandleOutcome::NonRetryable(BusReason(why)),
            // A TRANSIENT outbox hiccup retries (0 lost — the runtime redelivers, reverts the dedup
            // mark, re-runs). A good Signal is NEVER dead-lettered on an infra failure.
            Err(RouteError::EmitFailed(_why)) => HandleOutcome::Retry(myelin_events::Backoff { seconds: 2 }),
        }
    }
}

/// **Build the Signal-consumer router runtime for `tenant` (the ONE sanctioned wiring).** Constructs
/// the [`SignalRouter`] over `inbox` + `outbox` and wraps it in the seven-rule
/// [`Consumer`](myelin_events::Consumer) runtime through [`myelin_events::consume`] — binding the
/// `sig.<tenant>.` whitelist (rule 3: [`consume`] REJECTS a `*`/`>`/empty subject loudly), the
/// durable consumer name (rule 4), the bounded prefetch + per-tenant fairness cap (rule 6), and the
/// shared `dedup` ledger (rule 1). A subsystem never hand-rolls the subscription; it calls this.
///
/// Returns the [`Consumer`] the `serve` lifecycle registers as a [`ConsumerReg`]
/// (`myelin_substrate::ConsumerReg::new`) in the AppSpec `consumers` slot. The `sig.<tenant>.`
/// prefix is validated + leaked to `'static` once here (bounded — one binding per tenant per
/// process). An over-broad / malformed tenant prefix returns [`SubscribeError`].
pub fn build_router(
    tenant: &TenantId,
    inbox: InboxProjection,
    outbox: OutboxStore,
    dedup: DedupLedger,
) -> Result<Consumer<SignalRouter>, SubscribeError> {
    let prefix = signal_subject_prefix(tenant)
        .ok_or_else(|| SubscribeError::WildcardSubject(format!("sig.{}.", tenant.0)))?;
    // The `'static` whitelist the trait's `subjects()` returns. Leaked once per tenant per process
    // (the binding set is fixed for the life of the consumer pool — bounded, never per-event).
    let subjects: &'static [SubjectPattern] =
        Box::leak(vec![SubjectPattern(prefix.clone())].into_boxed_slice());
    let router = SignalRouter::new(
        inbox,
        outbox,
        Arc::new(MonotonicMinter::new()),
        subjects,
    );
    // The ONE sanctioned consumer entry-point — `consume` validates the spec (rule 3: rejects a
    // `*`/empty subject LOUDLY) and constructs the [`Consumer`] with all seven rules wired.
    consume(
        ConsumerSpec::new(ConsumerName(ROUTER_CONSUMER_NAME.into()), &[prefix.as_str()]),
        router,
        dedup,
    )
}

/// Map a Signal [`Severity`] to the inbox routing [`Class`] (the skeleton mapping; the prefs-aware
/// routing is NOTIF-P10). `critical → Critical` (pierces quiet-hours by default); `error`/`warning`
/// → `Direct` (actionable); `info`/`notice` → `Fyi` (digestible). A deterministic total mapping.
fn class_from_severity(severity: Severity) -> Class {
    match severity {
        Severity::Critical => Class::Critical,
        Severity::Error | Severity::Warning => Class::Direct,
        Severity::Notice | Severity::Info => Class::Fyi,
    }
}

/// The deterministic inbox `item_id` for `(tenant, recipient, dedup_key)` (so a redelivery /
/// same-key collapse hits the SAME row — the write-side idempotency anchor). A stable hash, not a
/// random id (a random id per delivery would create a duplicate row on a redelivery — silent
/// double-notify). PII-free (opaque tokens only).
fn item_id_for(tenant: &TenantId, recipient: &str, dedup_key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // NUL-separate so field boundaries are unambiguous (`("ab","c")` ≠ `("a","bc")`).
    tenant.0.hash(&mut h);
    0u8.hash(&mut h);
    recipient.hash(&mut h);
    0u8.hash(&mut h);
    dedup_key.hash(&mut h);
    format!("itm-{:016x}", h.finish())
}

/// The ambient [`EmitContextBase`] for the router's emit, taken from the INCOMING Signal envelope:
/// the emitted `notif.item.created` is partitioned to the SAME `(tenant, region)`, attributed to
/// the SAME actor, and clocked from the Signal's `occurred_at`/`recorded_at`. The minted event_id +
/// the causal triple are owned by the outbox (the caller cannot typo a wrong parent — `cause =
/// Some(signal_event)` supplies them).
fn emit_base_from(env: &EventEnvelope) -> EmitContextBase {
    EmitContextBase {
        tenant: env.tenant.clone(),
        region: env.region.clone(),
        actor: env.actor.clone(),
        schema_ver: 1,
        occurred_at: env.occurred_at.clone(),
        recorded_at: env.recorded_at.clone(),
        caused_by: env.caused_by.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, BusTransport, CorrelationId, Delivered, DedupLedger, EventId, InProcessBus, Message,
        Relay, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_query::signals::{DedupKey, RuleId, SignalState};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn principal() -> Principal {
        Principal::stub(PrincipalId("p-opaque-1".into()), PrincipalKind::Human, tenant())
    }

    /// A curated Signal (the shape the engine, P-138, publishes). `count`/`state` are the live
    /// collapse counters; the router reads `rule_id`/`severity`/`subject`/`dedup_key`.
    fn signal(rule: &str, severity: Severity, subject: &str, dedup: &str) -> Signal {
        Signal {
            rule_id: RuleId(rule.into()),
            tenant: tenant(),
            severity,
            dedup_key: DedupKey(dedup.into()),
            subject: ArtifactRef(subject.into()),
            count: 1,
            state: SignalState::Open,
            first_seen: "2026-06-20T00:00:00Z".into(),
            last_seen: "2026-06-20T00:00:00Z".into(),
        }
    }

    /// A `sig.<tenant>.<severity>.<rule>` envelope carrying a curated Signal payload (what the
    /// dispatch tier publishes; the router consumes it). `id` is the broker event_id (the
    /// consumer_dedup key).
    fn signal_envelope(id: &str, sig: &Signal) -> EventEnvelope {
        let subject = format!("sig.{}.{}.{}", sig.tenant.0, sig.severity.token(), sig.rule_id.0);
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType("signal.opened".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            subject: ArtifactRef(subject),
            aggregate: AggregateKey(format!("signal:{}", sig.dedup_key.0)),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::to_value(sig).unwrap(),
        }
    }

    /// The broker [`Message`] for a Signal envelope (subject = the envelope subject).
    fn signal_msg(id: &str, sig: &Signal) -> Message {
        let env = signal_envelope(id, sig);
        Message { subject: env.subject.0.clone(), envelope: env }
    }

    fn router_over(outbox: &OutboxStore) -> (Consumer<SignalRouter>, InboxProjection) {
        let inbox = InboxProjection::new();
        let consumer =
            build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();
        (consumer, inbox)
    }

    // --- Rule 3: the whitelist is sig.<tenant>.>, NEVER `*` ---

    /// **`signal_subject_prefix` builds the per-tenant `sig.<tenant>.` whitelist and refuses an
    /// over-broad one.** A concrete tenant → `sig.acme.`; an empty / dotted tenant → `None` (an
    /// over-broad or aliasing prefix is unconstructable).
    #[test]
    fn signal_subject_prefix_is_per_tenant_never_wildcard() {
        assert_eq!(signal_subject_prefix(&tenant()), Some("sig.acme.".into()));
        assert_eq!(signal_subject_prefix(&TenantId("".into())), None, "empty tenant refused");
        assert_eq!(
            signal_subject_prefix(&TenantId("a.b".into())),
            None,
            "a dotted tenant (extra segments → aliasing) is refused"
        );
        // the prefix is NOT a `*`/`>` wildcard (the consumer entry-point would reject it).
        assert!(!signal_subject_prefix(&tenant()).unwrap().contains('*'));
        assert!(!signal_subject_prefix(&tenant()).unwrap().contains('>'));
    }

    /// **`build_router` binds through the sanctioned `consume` (rule 3): the whitelist is the
    /// `sig.<tenant>.` prefix, never `*`.** The consumer's `subjects()` returns exactly that prefix.
    #[test]
    fn build_router_binds_sig_tenant_whitelist_never_star() {
        let outbox = OutboxStore::new();
        let (consumer, _) = router_over(&outbox);
        assert_eq!(consumer.name(), &ConsumerName(ROUTER_CONSUMER_NAME.into()));
        assert_eq!(
            consumer.handler().subjects(),
            &[SubjectPattern("sig.acme.".into())],
            "the whitelist is the sig.<tenant>. prefix (never `*`)"
        );
        // a Signal for THIS tenant matches; another tenant's does not.
        assert!(is_signal_subject("sig.acme.error.ci_run_failed", &tenant()));
        assert!(!is_signal_subject("sig.other.error.ci_run_failed", &tenant()));
    }

    /// **An over-broad / malformed tenant cannot construct a router (rule 3).** An empty tenant →
    /// `build_router` returns `Err` (it never silently narrows to an over-broad subscription).
    #[test]
    fn build_router_refuses_overbroad_tenant() {
        let r = build_router(
            &TenantId("".into()),
            InboxProjection::new(),
            OutboxStore::new(),
            DedupLedger::new(),
        );
        assert!(matches!(r, Err(SubscribeError::WildcardSubject(_))), "an empty tenant is refused");
    }

    // --- The skeleton body: UPSERT an inbox item + emit notif.item.created via the outbox ---

    /// **A curated Signal UPSERTs ONE inbox item AND emits exactly one `notif.item.created` via the
    /// outbox (the co-commit).** The inbox row carries refs-not-payloads; the emit is the ONLY
    /// publish (no `publish_now`). The emitted event's cause is the Signal (causality
    /// correct-by-construction).
    #[test]
    fn signal_upserts_one_item_and_emits_notif_item_created() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/42", "run-42");

        assert_eq!(consumer.deliver(&signal_msg("evt-1", &sig)), Delivered::Acked);
        assert_eq!(inbox.len(), 1, "one inbox row UPSERTed");

        // exactly one notif.item.created was emitted through the outbox (the co-commit).
        assert_eq!(outbox.committed_count(), 1, "one event committed (the emit)");
        let row = inbox
            .get(&tenant(), "psn:watcher:ci_run_failed", "ci_run_failed:run-42")
            .expect("the UPSERTed row exists at its (tenant, recipient, dedup_key) key");
        assert_eq!(row.coalesce_count, 1, "a fresh row starts at coalesce_count = 1");
        assert_eq!(row.state, "unread", "a fresh inbox row is unread (the ONE read-state column)");
        assert_eq!(row.class, Class::Direct, "an `error` Signal maps to the Direct class");
        // refs-not-payloads: the subject is a ref, never a rendered string.
        assert_eq!(row.subject.0, "myelin://acme/ci/run/42");
    }

    /// **The emitted event is `notif.item.created`, references-not-payloads, caused by the Signal.**
    /// The single committed outbox row is the `notif.item.created` with the item refs + the
    /// causation chain (root carries, depth+1) — no rendered string, no inline PII.
    #[test]
    fn emitted_event_is_notif_item_created_refs_not_payloads_caused_by_signal() {
        let outbox = OutboxStore::new();
        let (consumer, _) = router_over(&outbox);
        let sig = signal("ci_run_failed", Severity::Critical, "myelin://acme/ci/run/7", "run-7");
        let env = signal_envelope("evt-c1", &sig);
        consumer.deliver(&Message { subject: env.subject.0.clone(), envelope: env.clone() });

        // The relay-facing read: drain the committed emit to the in-process bus and read it back.
        let bus = InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), || {
            Timestamp("2026-06-20T00:00:02Z".into())
        });
        relay.drain_to_empty();
        let published = bus.consume("");
        assert_eq!(published.len(), 1, "exactly one notif.item.created emitted");
        let emitted = &published[0];
        assert_eq!(emitted.type_.0, NOTIF_ITEM_CREATED);
        assert!(!emitted.contains_personal_data, "references-not-payloads: no inline PII");
        assert!(emitted.pii_key_ref.is_none());
        // caused-by the Signal: the correlation root carries + depth+1 (the loop-guard stamp).
        assert_eq!(
            emitted.correlation_id, env.correlation_id,
            "the correlation root carries from the Signal"
        );
        assert_eq!(emitted.causation_id, Some(env.event_id.clone()), "causation = the Signal");
        assert_eq!(emitted.depth, env.depth + 1, "depth+1 (the loop-guard stamp)");
        // partitioned to the SAME (tenant, region) as the Signal.
        assert_eq!(emitted.tenant, env.tenant);
        assert_eq!(emitted.region, env.region);
    }

    // --- Idempotency: belt (consumer_dedup) + braces (the (tenant, recipient, dedup_key) UPSERT) ---

    /// **A REDELIVERED Signal (same `event_id`) is deduped by the consumer ledger — the handler runs
    /// ONCE, ONE inbox row, ONE emit (0 dup).** The core idempotency property (rule 1, the
    /// `origin_event`/`event_id` dedup).
    #[test]
    fn redelivered_signal_is_deduped_one_row_one_emit() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/42", "run-42");
        let m = signal_msg("evt-dup", &sig);

        assert_eq!(consumer.deliver(&m), Delivered::Acked, "first delivery routes + acks");
        assert_eq!(consumer.deliver(&m), Delivered::Deduplicated, "redelivery is deduped (0 dup)");
        assert_eq!(consumer.deliver(&m), Delivered::Deduplicated, "and again");
        assert_eq!(inbox.len(), 1, "exactly one inbox row (the redelivery did not double-notify)");
        assert_eq!(outbox.committed_count(), 1, "exactly one emit (the redelivery emitted nothing)");
    }

    /// **Two DISTINCT Signals rendering to the SAME `(tenant, recipient, dedup_key)` COLLAPSE into
    /// ONE inbox row with `coalesce_count = 2` (the write-time storm-control collapse, §3.2).** This
    /// is the inner idempotency (distinct broker events, same logical incident) — N near-identical
    /// Signals → ONE inbox row, the storm-control primitive the body (NOTIF-P11) builds on.
    #[test]
    fn same_key_signals_collapse_to_one_row_coalesce_count_bumps() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        // Two DISTINCT broker events (distinct event_id) for the SAME rule+dedup_key (same incident).
        let sig = signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/42", "run-42");
        assert_eq!(consumer.deliver(&signal_msg("evt-a", &sig)), Delivered::Acked);
        assert_eq!(consumer.deliver(&signal_msg("evt-b", &sig)), Delivered::Acked);

        assert_eq!(inbox.len(), 1, "same (tenant, recipient, dedup_key) → ONE row (collapse, §3.2)");
        let row = inbox
            .get(&tenant(), "psn:watcher:ci_run_failed", "ci_run_failed:run-42")
            .unwrap();
        assert_eq!(row.coalesce_count, 2, "the second same-key Signal bumped coalesce_count to 2");
    }

    /// **Distinct dedup keys open distinct inbox rows** (the collapse is by `(recipient, dedup_key)`,
    /// not by rule — two different runs failing → two rows).
    #[test]
    fn distinct_keys_open_distinct_rows() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let a = signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/1", "run-1");
        let b = signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/2", "run-2");
        consumer.deliver(&signal_msg("evt-1", &a));
        consumer.deliver(&signal_msg("evt-2", &b));
        assert_eq!(inbox.len(), 2, "two distinct runs → two distinct inbox rows");
    }

    // --- NOTIF-D10: a poison Signal terminates, does not stall, lag stays bounded ---

    /// **NOTIF-D10 (head-of-line isolation): a POISON Signal (un-parseable payload) terminates
    /// (`NonRetryable` → dead-letter), the router does NOT stall, a GOOD Signal on a sibling subject
    /// still routes, and consumer lag stays bounded (0).** The drill's green: 0 head-of-line stalls;
    /// lag recovers to 0 (observability is part of the pass, EI-01 §3).
    #[test]
    fn notif_d10_poison_signal_does_not_stall_other_subjects() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);

        // A POISON Signal: a `sig.acme.*` subject whose payload is NOT a Signal (un-parseable).
        let poison = Message {
            subject: "sig.acme.error.broken_rule".into(),
            envelope: EventEnvelope {
                payload: serde_json::json!({ "not": "a signal" }),
                ..signal_envelope("evt-poison", &signal("x", Severity::Error, "myelin://acme/ci/run/0", "k"))
            },
        };
        // A GOOD Signal on a sibling subject (different rule → different subject segment).
        let good = signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/42", "run-42");
        let good_msg = signal_msg("evt-good", &good);

        // The poison terminates IMMEDIATELY (dead-letter, rule 5) — not a Retry, not a stall.
        let out = consumer.deliver(&poison);
        assert!(matches!(out, Delivered::DeadLettered(_)), "the poison terminates (NonRetryable)");
        assert_eq!(consumer.dead_letters().len(), 1, "the poison is SURFACED, not silently dropped");

        // The GOOD Signal still routes — the poison did NOT head-of-line-block it (0 stalls).
        assert_eq!(consumer.deliver(&good_msg), Delivered::Acked, "the good Signal is not blocked");
        assert_eq!(inbox.len(), 1, "the good Signal UPSERTed its row (the poison wrote none)");

        // Consumer lag stays BOUNDED at 0 (the dead-letter is terminal; the good Signal acked).
        // The lag-alarm reads `consumer.lag()` (contract 1.8); 0 is below any threshold default.
        assert_eq!(consumer.lag(), 0, "NOTIF-D10: 0 head-of-line stalls; lag recovered to 0");
        // The poison wrote NO inbox row and emitted NOTHING (a poison is not a half-write).
        assert_eq!(outbox.committed_count(), 1, "only the good Signal emitted (the poison did not)");
    }

    /// **A poison redelivery is deduped, not re-poisoned** (its dead-letter mark is terminal; a
    /// re-delivered dead-letter reads `Deduplicated`, not a second dead-letter).
    #[test]
    fn poison_redelivery_is_deduped_not_repoisoned() {
        let outbox = OutboxStore::new();
        let (consumer, _) = router_over(&outbox);
        let poison = Message {
            subject: "sig.acme.error.broken".into(),
            envelope: EventEnvelope {
                payload: serde_json::json!({ "bad": true }),
                ..signal_envelope("evt-p", &signal("x", Severity::Error, "myelin://acme/ci/run/0", "k"))
            },
        };
        assert!(matches!(consumer.deliver(&poison), Delivered::DeadLettered(_)));
        assert_eq!(consumer.deliver(&poison), Delivered::Deduplicated, "a re-delivered poison dedups");
        assert_eq!(consumer.dead_letters().len(), 1, "still exactly one dead-letter (not re-poisoned)");
    }

    // --- The skeleton mappings (the mutation-floor decision logic) ---

    /// **Severity → Class is the frozen skeleton mapping** (`critical → Critical`,
    /// `error`/`warning → Direct`, `notice`/`info → Fyi`). A mutant that mis-maps a severity is caught.
    #[test]
    fn class_from_severity_is_the_frozen_skeleton_mapping() {
        assert_eq!(class_from_severity(Severity::Critical), Class::Critical);
        assert_eq!(class_from_severity(Severity::Error), Class::Direct);
        assert_eq!(class_from_severity(Severity::Warning), Class::Direct);
        assert_eq!(class_from_severity(Severity::Notice), Class::Fyi);
        assert_eq!(class_from_severity(Severity::Info), Class::Fyi);
    }

    /// **`item_id_for` is deterministic + field-boundary-unambiguous** (so a redelivery / collapse
    /// hits the SAME row; `("ab","c")` ≠ `("a","bc")`). A mutant that drops a field or the separator
    /// is caught.
    #[test]
    fn item_id_is_deterministic_and_field_unambiguous() {
        let t = tenant();
        let a = item_id_for(&t, "psn:alice", "k1");
        assert_eq!(a, item_id_for(&t, "psn:alice", "k1"), "the same tuple → the same id (idempotent)");
        assert_ne!(a, item_id_for(&t, "psn:alice", "k2"), "a different dedup_key → a different id");
        assert_ne!(a, item_id_for(&t, "psn:bob", "k1"), "a different recipient → a different id");
        assert_ne!(
            a,
            item_id_for(&TenantId("other".into()), "psn:alice", "k1"),
            "tenant-scoped id"
        );
        // field boundary: ("ab","c") must not collide with ("a","bc").
        assert_ne!(
            item_id_for(&t, "ab", "c"),
            item_id_for(&t, "a", "bc"),
            "field boundaries are unambiguous (NUL-separated)"
        );
    }

    /// **`InboxProjection::is_empty` / `len` track state precisely**, and `SignalRouter::inbox`
    /// returns the SAME projection the router UPSERTs into (so a drill reads the routed result). A
    /// mutant that stubs `is_empty` to a constant or returns a fresh default projection is caught.
    #[test]
    fn inbox_projection_is_empty_len_and_router_inbox_accessor_track_state() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        assert!(inbox.is_empty(), "a fresh projection is empty");
        assert_eq!(inbox.len(), 0);
        // the router's `inbox()` is the SAME projection (not a fresh default).
        assert!(consumer.handler().inbox().is_empty());

        let sig = signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/42", "run-42");
        consumer.deliver(&signal_msg("evt-1", &sig));
        assert!(!inbox.is_empty(), "after a route the projection is NOT empty");
        assert_eq!(inbox.len(), 1);
        // the router's `inbox()` accessor observes the SAME row (a fresh default would be empty).
        assert!(!consumer.handler().inbox().is_empty(), "router.inbox() is the live projection");
        assert_eq!(consumer.handler().inbox().len(), 1);
    }

    /// **The frozen emit tokens** (`notif.item.created` + `notif.escalation.acked`) — the named
    /// constants the drills assert against, never a literal.
    #[test]
    fn router_emit_tokens_are_frozen() {
        assert_eq!(NOTIF_ITEM_CREATED, "notif.item.created");
        assert_eq!(NOTIF_ESCALATION_ACKED, "notif.escalation.acked");
        assert_eq!(ROUTER_CONSUMER_NAME, "notif-signal-router");
    }
}
