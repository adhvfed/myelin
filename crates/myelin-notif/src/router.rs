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
use myelin_content::InlineNode;
use myelin_identity::Principal;
use myelin_query::signals::{Severity, Signal};
use myelin_tenancy::{Region, TenantId};

use crate::prefs::QuietHours;
use crate::storm_control::{subject_root_of, RateConfig, StormContext, StormControl, StormDecision};
use crate::write_fanout::{extract_mentions, CapVerdict, HotSubjectCap};
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
    /// `mark`/`snooze`/`mark_all_read` (NOTIF-P6) flip THIS column on the SAME row across every view.
    pub state: String,
    /// The durable-snooze re-surface time (the §2.1 `snooze_until`) — `snooze(item, until)` records
    /// it here; the item is suppressed from the active inbox until then. A fresh row has `None`. The
    /// durable re-surface TIMER that flips a due snooze back to `unread` is the `myelin-flow` wheel
    /// (NOTIF-P14 / NOTIF-P18); here only the until is recorded (the named floor).
    pub snooze_until: Option<String>,
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
    /// key inserts the row (`coalesce_count = 1`); an EXISTING key COLLAPSES into it
    /// (`coalesce_count += 1`) — it does NOT create a second row. This is the storm-control mechanism-2
    /// primitive: the insert-vs-collapse VERDICT is decided one step earlier by
    /// [`StormControl::decide`](crate::storm_control::StormControl::decide) (reading
    /// [`InboxProjection::contains`] BEFORE this UPSERT), so the router can surface the N→1 collapse +
    /// the dedup-collapse-ratio for the NOTIF-D2 drill; this method performs the write either way.
    fn upsert(&self, mut item: RoutedInboxItem) {
        let key = (item.tenant.0.clone(), item.recipient.clone(), item.dedup_key.clone());
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get_mut(&key) {
            Some(existing) => {
                existing.coalesce_count += 1;
            }
            None => {
                item.coalesce_count = 1;
                guard.insert(key, item);
            }
        }
    }

    /// **Test/holder seam: UPSERT a row directly (the same write-time-collapse path the router
    /// uses).** Lets a holder property test (NOTIF-P4, `crate::holder`) seed the projection with a
    /// known set of refs-stored items, then assert the structural-erase 0-mutation property — without
    /// standing up the whole Signal pipeline. Routes through the SAME private [`Self::upsert`] (one
    /// write path, no second store).
    #[doc(hidden)]
    pub fn upsert_for_test(&self, item: RoutedInboxItem) {
        self.upsert(item);
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

    /// **Does a row already exist at `(tenant, recipient, dedup_key)`?** The storm-control
    /// dedup-collapse mechanism (NOTIF-P11) reads this BEFORE the UPSERT to decide insert-vs-collapse
    /// (so the verdict can surface `Collapse` for the drill's N→1 + the collapse-ratio). In the OLTP
    /// binding the UPSERT's `ON CONFLICT` reports this; the in-memory projection answers it directly.
    pub fn contains(&self, tenant: &TenantId, recipient: &str, dedup_key: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&(tenant.0.clone(), recipient.to_string(), dedup_key.to_string()))
    }

    /// Read one row by `(tenant, recipient, dedup_key)` (for tests / a drill).
    pub fn get(&self, tenant: &TenantId, recipient: &str, dedup_key: &str) -> Option<RoutedInboxItem> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(tenant.0.clone(), recipient.to_string(), dedup_key.to_string()))
            .cloned()
    }

    /// **Mutate the ONE read-state row for `(tenant, recipient, item_id)` in place (the NOTIF-P6
    /// read-state write path).** Finds the SINGLE row addressed to `recipient` whose `item_id` is
    /// `item_id` and applies `f` to it — the C-9 "one store → one read-state truth": the `state`
    /// column is the SAME row across every view, so the mutation is visible in the unified inbox AND
    /// in every scoped view at once (there is no second store to keep in sync). Returns `true` iff a
    /// row was found and mutated (a row not addressed to `recipient`, or a missing `item_id`, mutates
    /// NOTHING — a principal can only flip the read-state of their OWN items).
    ///
    /// The projection is keyed `(tenant, recipient, dedup_key)`; the contract addresses items by the
    /// opaque `item_id` (the 7.2 read-state handle). `item_id` is deterministic from
    /// `(tenant, recipient, dedup_key)` ([`item_id_for`]), so within a `(tenant, recipient)` it is
    /// unique — this scan finds the one row. In the OLTP binding this is a single
    /// `UPDATE notif_inbox_item SET state = $1 WHERE tenant_id = $2 AND recipient = $3 AND item_id = $4`.
    pub fn mutate_state<F: FnOnce(&mut RoutedInboxItem)>(
        &self,
        tenant: &TenantId,
        recipient: &str,
        item_id: &str,
        f: F,
    ) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for row in guard.values_mut() {
            if row.tenant == *tenant && row.recipient == recipient && row.item_id == item_id {
                f(row);
                return true;
            }
        }
        false
    }

    /// **Apply `f` to EVERY row addressed to `recipient` for which `select(row)` is true (the
    /// `mark_all_read(filter)` write path, NOTIF-P6).** Flips state on exactly the rows the filter
    /// selects — and ONLY rows addressed to `recipient` (never another principal's inbox). Returns
    /// the count mutated. In the OLTP binding this is one set-based
    /// `UPDATE notif_inbox_item SET state = 'read' WHERE tenant_id = $1 AND recipient = $2 AND <filter>`.
    pub fn mutate_matching<S, F>(&self, tenant: &TenantId, recipient: &str, select: S, mut f: F) -> usize
    where
        S: Fn(&RoutedInboxItem) -> bool,
        F: FnMut(&mut RoutedInboxItem),
    {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0;
        for row in guard.values_mut() {
            if row.tenant == *tenant && row.recipient == recipient && select(row) {
                f(row);
                n += 1;
            }
        }
        n
    }

    /// **A snapshot of all rows under one tenant (the holder's scan surface, NOTIF-P4).** The
    /// `PersonalDataHolder` (`crate::holder`) walks this to `locate`/`erase` a subject's appearances
    /// — the references-not-payloads holder reads, it never reaches around the projection. Returns a
    /// CLONE (so the holder scans without holding the lock); the projection is the model of the
    /// `notif_inbox_item` table the live OLTP `SELECT … WHERE tenant_id = $1` reads.
    pub fn snapshot_for_tenant(&self, tenant: &TenantId) -> Vec<RoutedInboxItem> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|row| row.tenant == *tenant)
            .cloned()
            .collect()
    }

    /// **Wipe every row for `tenant` (the reindex-from-source cold-rebuild precondition, NOTIF-P17).**
    /// A FULL `reindex(notif)` starts from an empty inbox — the inbox is a DERIVED read-model (a
    /// projection, not a system-of-record), so wiping it is safe: the reindex re-drives the SAME
    /// router over the owner's replayed Signals to rebuild it (cold == live). Tenant-scoped (one
    /// tenant's rebuild never disturbs another's live inbox). In the OLTP binding this is a single
    /// `DELETE FROM notif_inbox_item WHERE tenant_id = $1` (the wiped generation is rebuilt from
    /// source). Returns the number of rows wiped (observability — the drill reads it).
    pub fn wipe_tenant(&self, tenant: &TenantId) -> usize {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let before = guard.len();
        guard.retain(|_, row| row.tenant != *tenant);
        before - guard.len()
    }
}

impl RoutedInboxItem {
    /// **Does this item reference the subject (the references-not-payloads `locate` predicate,
    /// NOTIF-P4)?** A subject appears in an inbox row in two structural places, BOTH as refs/ids,
    /// never as a stored name:
    ///
    /// 1. as the **`recipient`** (the subject's OWN inbox — the opaque Principal pseudonym, 4.8); or
    /// 2. as a referenced actor in the **`subject`** / **`origin_event`** [`ArtifactRef`] (a
    ///    `myelin://<tenant>/identity/principal/<id>` ref the subject is the actor of) — someone
    ///    ELSE's inbox row that names the subject by reference.
    ///
    /// In NEITHER case is the subject's name stored: the recipient is an opaque pseudonym and the
    /// refs resolve per-viewer at humanise time. So erasing the subject tombstones the appearance
    /// **for free** (Identity's 4.8 pseudonym-map shred makes the opaque id unresolvable) with NO
    /// mutation of these columns — the structural references-not-payloads property (§3.9, C7).
    pub fn references_subject(&self, subject_id: &str) -> bool {
        self.recipient == subject_id
            || self.subject.0.ends_with(&format!("/principal/{subject_id}"))
            || self.origin_event.0.ends_with(&format!("/principal/{subject_id}"))
    }
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
    /// **The five write-time storm-control mechanisms (NOTIF-P11 / §3.2).** Run between classify
    /// ([`SignalRouter::derive_item`]) and UPSERT: self-suppression, dedup-key collapse,
    /// thread/subject coalescing, per-`(recipient, subject_root)` rate damping, and mute/DND honoring.
    /// Holds the per-pool storm state (the coalescer + the token buckets + the mute set). A cloneable
    /// handle, so the whole router pool shares ONE storm-control truth.
    storm: StormControl,
    /// **The hot-subject cap (NOTIF-P12 / §3.2.4 / §3.5).** Bounds the WRITE-FANOUT side: a
    /// mention-storm on a hot subject_root materialises at most [`HotSubjectCap::cap`] DISTINCT
    /// recipient rows; further distinct mentions coalesce into the ONE "+N more were mentioned"
    /// marker rather than write-amplifying into N rows. A cloneable handle so the whole pool shares
    /// ONE cap truth per subject_root.
    hot_cap: HotSubjectCap,
    /// **The read-fanout ambient marker store (NOTIF-P13 / §3.5).** The UNBOUNDED ambient set
    /// (watchers / 50k-channel members) is NOT exploded into per-recipient writes — the router records
    /// ONE coalesced marker per `subject_root` here, and the viewer's slice is materialised LAZILY on
    /// inbox open via [`crate::read_fanout::read_fanout`] (the `SetExpr` watcher push-down JOIN + the
    /// zookie watermark). A 50k-watcher celebrity subject costs ONE marker, never 50k rows (zero write
    /// amplification). A cloneable handle so the read path + a drill share one truth.
    ambient: crate::read_fanout::AmbientMarkerStore,
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
        SignalRouter {
            inbox,
            outbox,
            minter,
            storm: StormControl::new(),
            hot_cap: HotSubjectCap::new(),
            ambient: crate::read_fanout::AmbientMarkerStore::new(),
            subjects,
        }
    }

    /// The inbox projection this router UPSERTs into (so a drill can read the result).
    pub fn inbox(&self) -> &InboxProjection {
        &self.inbox
    }

    /// **The read-fanout ambient marker store (NOTIF-P13 / §3.5).** A drill / the inbox-open read
    /// reads the ONE coalesced marker per watched `subject_root` the router records (zero write
    /// amplification); the per-viewer slice is materialised lazily via
    /// [`crate::read_fanout::read_fanout`] (the `SetExpr` watcher push-down JOIN + the zookie
    /// watermark). Cloneable.
    pub fn ambient(&self) -> &crate::read_fanout::AmbientMarkerStore {
        &self.ambient
    }

    /// The storm-control stage this router runs between classify and UPSERT (so a drill / a test can
    /// read its state or mute a thread). The five §3.2 mechanisms live here.
    pub fn storm(&self) -> &StormControl {
        &self.storm
    }

    /// The hot-subject cap this router bounds write-fanout with (so a drill can read the cap +
    /// the per-`subject_root` admitted/overflow counts). The §3.2.4/§3.5 write-amplification bound.
    pub fn hot_cap(&self) -> &HotSubjectCap {
        &self.hot_cap
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
    /// Returns the [`StormDecision`] on success (so the handler can observe deliver/collapse/coalesce/
    /// suppress) or a [`RouteError`] poison. **No `publish_now`** — the emit rides the ONE sanctioned
    /// outbox path.
    ///
    /// **The five write-time storm-control mechanisms (NOTIF-P11) run between (2) classify and (3)
    /// UPSERT.** A self-notification / rate-damped candidate writes NO row and emits NOTHING (the
    /// underlying Signal stays on the bus — the audit is untouched, EI-04 §5.3); a muted / quiet-hours
    /// candidate WRITES the row (the ONE inbox always receives) but does not push (the emit is the
    /// create-side projection event, so a suppressed-delivery candidate writes the row WITHOUT the
    /// emit); a deliver/collapse/coalesce candidate co-commits the row + the emit.
    fn route(&self, signal_event: &EventEnvelope) -> Result<StormDecision, RouteError> {
        // (1) Parse the curated Signal from the envelope payload (poison → NonRetryable).
        let signal: Signal = serde_json::from_value(signal_event.payload.clone())
            .map_err(|e| RouteError::MalformedSignal(e.to_string()))?;

        // (1b) WRITE-FANOUT for the bounded high-signal set (NOTIF-P12, §3.5 step-1 DIRECT). If the
        // Signal carries `mention(Principal)` STRUCTURED nodes (the recipient was directly addressed),
        // materialise one inbox_item per mentioned recipient — bounded by the hot-subject cap so a
        // mention-storm can't write-amplify. Notif reads the STRUCTURED node, NEVER free text (AG-6):
        // `mentions_of` returns `Vec<Principal>` from `&[InlineNode]`, a free-text parse is
        // unconstructable. The ambient skeleton candidate (below) is the §3.5 read-fanout *floor*
        // (the unbounded watcher set is NOTIF-P13); the mention set is the bounded write-fanout leg.
        self.write_fanout(signal_event, &signal)?;

        // (1c) READ-FANOUT for the UNBOUNDED ambient set (NOTIF-P13, §3.5 step-1 AMBIENT). The
        // watcher set (every watcher of a hot PR, every member of a 50k channel) is NOT exploded into
        // per-recipient writes — the router records ONE coalesced marker per `subject_root` in the
        // ambient marker store (zero write amplification, regardless of watcher count). The viewer's
        // slice is materialised LAZILY on inbox open via `read_fanout` (the `SetExpr` watcher push-down
        // JOIN + the zookie watermark). The subject's `#sub` fragment is stripped to the root so all
        // ambient activity on the same thread/PR coalesces into the ONE marker (§3.2.3). A `Watched`
        // ambient event feeds the marker; the bounded DIRECT mention set (above) feeds per-recipient
        // rows. (The marker is recorded for EVERY routed Signal: an ambient event on a watched subject
        // is the read-fanout's input; who WATCHES it is resolved at read time, not here.)
        self.ambient.record(
            &signal.tenant,
            &signal.subject,
            Reason::Watched,
            &ArtifactRef(format!("myelin://{}/bus/event/{}", signal_event.tenant.0, signal_event.event_id.0)),
        );

        // (2) Derive the SKELETON ambient inbox row (the real per-reason/per-recipient routing is
        // NOTIF-P8+; the unbounded ambient watcher read-fanout is recorded above — this skeleton row
        // is the per-rule `psn:watcher:<rule>` digest candidate, kept for the storm-control contract).
        let item = self.derive_item(signal_event, &signal);
        let subject_root = subject_root_of(&item.subject.0);
        self.route_one_candidate(signal_event, item, &subject_root)
    }

    /// **Write-fanout the bounded high-signal mention set (NOTIF-P12, §3.5/§3.2.4).** Reads the
    /// `mention(Principal)` STRUCTURED nodes carried by the Signal (via [`mentions_of`] — `&[InlineNode]`,
    /// NEVER a free-text parse, AG-6) and materialises **one inbox_item per mentioned recipient**,
    /// classified `reason = Mentioned` / `class = Direct`, through the SAME storm-control collapse +
    /// outbox co-commit as the ambient candidate.
    ///
    /// **The hot-subject cap (§3.2.4) bounds the write-amplification:** per `subject_root`, at most
    /// [`HotSubjectCap::cap`] DISTINCT mention rows materialise; a NEW distinct recipient past the cap
    /// **overflows** into the ONE coalesced "+N more were mentioned" marker (it writes NO new row, it
    /// emits NO new push) — so a `@here` spray on a 10k channel costs at most `cap` write rows, never
    /// 10k. A repeat mention of an already-admitted recipient is admitted (it collapses on the dedup
    /// key — `coalesce_count += 1` — it never opens a new row). Returns the FIRST [`RouteError`] (a
    /// transient outbox hiccup on any recipient → Retry the whole Signal; 0 lost).
    fn write_fanout(
        &self,
        signal_event: &EventEnvelope,
        signal: &Signal,
    ) -> Result<(), RouteError> {
        let mentions = mentions_of(signal_event);
        if mentions.is_empty() {
            return Ok(());
        }
        let subject_root = subject_root_of(&signal.subject.0);
        for principal in &mentions {
            let item = self.derive_mention_item(signal_event, signal, principal);
            // The hot-subject cap decision FIRST (§3.2.4): a NEW distinct recipient past the cap
            // OVERFLOWS into the coalesced marker (no new row, no write-amplification). An admitted
            // recipient (within the cap, or a repeat) proceeds to the storm-control collapse + UPSERT.
            match self.hot_cap.admit(&item.recipient, &subject_root) {
                CapVerdict::Overflow => {
                    // Bounded: the storm is coalesced into the marker, NOT materialised as a new row.
                    // The count is preserved (`overflow_count`) — bounded, never silently lost.
                    continue;
                }
                CapVerdict::Admit => {
                    self.route_one_candidate(signal_event, item, &subject_root)?;
                }
            }
        }
        Ok(())
    }

    /// **Route ONE candidate inbox item through storm-control + the outbox co-commit** (the shared
    /// per-recipient write path used by BOTH the ambient skeleton candidate and each write-fanout
    /// mention candidate). Runs the five §3.2 storm-control mechanisms between classify and UPSERT,
    /// then co-commits the inbox row + the `notif.item.created` emit (emit-iff-committed). Returns the
    /// [`StormDecision`] (so the caller can observe deliver/collapse/coalesce/suppress).
    fn route_one_candidate(
        &self,
        signal_event: &EventEnvelope,
        item: RoutedInboxItem,
        subject_root: &str,
    ) -> Result<StormDecision, RouteError> {
        let recipient = item.recipient.clone();
        let dedup_key = item.dedup_key.clone();

        // (2b) STORM-CONTROL (NOTIF-P11, §3.2) — the five write-time mechanisms, between classify and
        // UPSERT. `row_exists` (the dedup-collapse input) is read BEFORE the UPSERT so the verdict can
        // surface a Collapse (the drill's N→1 + the collapse-ratio). The quiet-hours/rate context is
        // the per-pool default (never-quiet + critical-pierce, the §3.2.4 default rate) until the live
        // per-recipient PrefStore (NOTIF-P10) wires in here — a NAMED floor; the mechanisms that need
        // no prefs (self-suppression, dedup-collapse, coalescing, rate-damping) are fully live now.
        let row_exists = self.inbox.contains(&item.tenant, &recipient, &dedup_key);
        let quiet = QuietHours::default();
        let storm_ctx = StormContext {
            // The logical clock the token bucket is damped on: the skeleton uses tick 0 (a single
            // pool tick); the live wiring advances it from the Signal clock (the named floor).
            tick: 0,
            utc_minute_of_day: 0,
            utc_weekday: 0,
            quiet: &quiet,
            rate: RateConfig::default(),
        };
        let decision = self
            .storm
            .decide(signal_event, &item, subject_root, row_exists, &storm_ctx);

        // A storm-control verdict NEVER touches the audit (EI-04 §5.3): the underlying Signal is on
        // the bus regardless. A self-action / rate-damped candidate writes no row and emits nothing.
        if !decision.writes_row() {
            return Ok(decision);
        }

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
        self.inbox.upsert(item.clone());
        tx.stage_state_change(format!(
            "UPSERT notif_inbox_item ({}, {}, {})",
            item.tenant.0, recipient, dedup_key
        ));

        // The emit is the create-side DELIVERY projection event. It rides the outbox ONLY when the
        // verdict DELIVERS (a fresh deliver, a collapse, or a coalesce bumps a counter the inbox
        // reads — those still surface the item). A muted / quiet-hours verdict WRITES the row (above)
        // but SUPPRESSES the channel push, so it does NOT emit the delivery event — the row is in the
        // ONE inbox (the audit/history), only the off-cell push is silenced (§3.2.5).
        if decision.delivers() {
            // The ONE sanctioned emit verb (contract 2.2; no-raw-publish). `cause = Some(signal_event)`
            // → the correlation root carries + causation = the Signal + depth+1 (the loop-guard stamp).
            // A `notif.item.created` is references-not-payloads: it carries the item_id + subject ref,
            // never a rendered string (humanise is per-viewer at read time, NOTIF-P9).
            tx.emit(self.item_created_draft(&item), Some(signal_event))
                .map_err(|e| RouteError::EmitFailed(format!("outbox emit failed: {e:?}")))?;
        }

        // Commit: the inbox row (+ the notif.item.created emit, when delivered) become durable
        // atomically. A commit failure is a TRANSIENT outbox hiccup → Retry (never a silent
        // half-write, never a dead-letter of a good Signal).
        tx.commit()
            .map_err(|e| RouteError::EmitFailed(format!("outbox commit failed: {e:?}")))?;

        Ok(decision)
    }

    /// **Derive a write-fanout mention [`RoutedInboxItem`] for one mentioned [`Principal`]** (the
    /// §3.5 step-1 DIRECT high-signal set). The recipient is the mentioned principal's OPAQUE
    /// `principal_id` (4.8 pseudonym, never a name); `reason = Mentioned` (the C-9 scoped-view filter
    /// basis — Chat "Activity/Mentions", Git "Review requests"); `class = Direct` (directly addressed
    /// — a break-out class storm-control never folds into a digest, §3.2.3). The `dedup_key` is
    /// `mention:<rule>:<dedup>:<principal>` so EACH mentioned recipient gets their OWN row (one
    /// inbox_item per recipient), while a redelivery of the SAME mention collapses (§3.2).
    fn derive_mention_item(
        &self,
        env: &EventEnvelope,
        signal: &Signal,
        principal: &Principal,
    ) -> RoutedInboxItem {
        let recipient = principal.principal_id.0.clone();
        // Per-recipient dedup key: one row per mentioned recipient (write-fanout), idempotent on
        // redelivery (the same mention re-fires onto the SAME row, never a duplicate).
        let dedup_key = format!(
            "mention:{}:{}:{}",
            signal.rule_id.0, signal.dedup_key.0, recipient
        );
        let item_id = item_id_for(&env.tenant, &recipient, &dedup_key);
        RoutedInboxItem {
            tenant: env.tenant.clone(),
            region: env.region.clone(),
            item_id,
            recipient,
            subject: signal.subject.clone(),
            // A mention is the canonical DIRECT high-signal reason (§3.5 / contract 13.1).
            reason: Reason::Mentioned,
            // Directly addressed → Direct (broken out of every digest; pierces by prefs at NOTIF-P10).
            class: Class::Direct,
            origin_event: ArtifactRef(format!(
                "myelin://{}/bus/event/{}",
                env.tenant.0, env.event_id.0
            )),
            dedup_key,
            coalesce_count: 1,
            state: "unread".to_string(),
            snooze_until: None,
        }
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
            snooze_until: None,
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

/// **The frozen envelope key the STRUCTURED `mention(Principal)` nodes ride on (NOTIF-P12, §3.5).**
/// The curated Signal's envelope payload carries the originating content's STRUCTURED mention nodes
/// under this key — populated by the dispatch tier (EB-23) from the originating event's
/// `myelin-content` body, NOT scraped from free text. A named constant so the producer + the consumer
/// agree on the WIRE (the CDC pins it); a drift breaks the build, never silently in prod.
pub const SIGNAL_MENTIONS_KEY: &str = "mentions";

/// **Read the STRUCTURED `mention(Principal)` nodes the Signal carries — NEVER parse free text
/// (AG-6).** The dispatch tier stamps the originating content's structured inline nodes onto the
/// Signal envelope payload under [`SIGNAL_MENTIONS_KEY`] as a JSON array of [`InlineNode`]s (the
/// frozen 13.1 taxonomy node). This reads ONLY that structured array and returns the mentioned
/// [`Principal`]s (deduped by `principal_id`) via [`extract_mentions`]. There is NO `&str` overload
/// anywhere on this path: Notif reads the structured node the producer froze; it does NOT re-derive
/// the mention shape from raw text (the agent-loop reference gate — only a structured ref re-triggers).
/// A missing / malformed `mentions` key → NO mentions (the Signal is ambient-only); it is NOT a
/// poison (the Signal itself parsed — a content-less Signal is normal, e.g. a CI failure).
fn mentions_of(env: &EventEnvelope) -> Vec<Principal> {
    let Some(value) = env.payload.get(SIGNAL_MENTIONS_KEY) else {
        return Vec::new();
    };
    // The structured nodes — a JSON array of `InlineNode` (the 13.1 taxonomy). A malformed shape is
    // treated as no-mentions (ambient-only), never a free-text fallback (there is none).
    let nodes: Vec<InlineNode> = serde_json::from_value(value.clone()).unwrap_or_default();
    extract_mentions(&nodes)
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

    // --- NOTIF-P12: write-fanout for the bounded high-signal mention set ---

    use myelin_content::InlineNode;

    /// A `sig.<tenant>.…` envelope carrying the Signal payload + the STRUCTURED `mention(Principal)`
    /// nodes under [`SIGNAL_MENTIONS_KEY`] (the dispatch tier stamps them from the originating content;
    /// Notif reads the structured node, never free text — AG-6).
    fn signal_msg_with_mentions(id: &str, sig: &Signal, mentions: &[Principal]) -> Message {
        let mut env = signal_envelope(id, sig);
        let nodes: Vec<InlineNode> =
            mentions.iter().cloned().map(InlineNode::Mention).collect();
        // The Signal payload is an object; add the structured `mentions` array beside it.
        if let serde_json::Value::Object(map) = &mut env.payload {
            map.insert(SIGNAL_MENTIONS_KEY.into(), serde_json::to_value(&nodes).unwrap());
        }
        Message { subject: env.subject.0.clone(), envelope: env }
    }

    fn mentioned(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }

    /// **The mention-write-fanout check (NOTIF-P12 GATE): a Signal carrying `mention(Principal)`
    /// nodes materialises EXACTLY ONE inbox_item per mentioned recipient; the row is classified
    /// `Mentioned`/`Direct`; Notif read the STRUCTURED node (0 free-text parse).** Threshold: 1 item
    /// per mentioned recipient.
    #[test]
    fn write_fanout_materialises_one_item_per_mentioned_recipient() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal("pr_review", Severity::Info, "myelin://acme/git/pr/9", "pr-9");
        let mentions = [mentioned("p-alice"), mentioned("p-bob"), mentioned("p-carol")];

        assert_eq!(
            consumer.deliver(&signal_msg_with_mentions("evt-m1", &sig, &mentions)),
            Delivered::Acked
        );

        // ONE row per mentioned recipient + ONE ambient skeleton row (the read-fanout floor). The
        // three mention rows are the bounded write-fanout; assert each mentioned recipient has a row.
        for p in &mentions {
            let dedup = format!("mention:pr_review:pr-9:{}", p.principal_id.0);
            let row = inbox
                .get(&tenant(), &p.principal_id.0, &dedup)
                .unwrap_or_else(|| panic!("a mention row for {}", p.principal_id.0));
            assert_eq!(row.reason, Reason::Mentioned, "a mention → reason Mentioned");
            assert_eq!(row.class, Class::Direct, "a mention is directly addressed → Direct");
            assert_eq!(row.recipient, p.principal_id.0, "the recipient is the mentioned principal");
            // refs-not-payloads: the subject is a ref, the recipient an opaque id (no name stored).
            assert_eq!(row.subject.0, "myelin://acme/git/pr/9");
        }
        // 3 mention rows + 1 ambient skeleton row = 4 distinct rows.
        assert_eq!(inbox.len(), 4, "one row per mentioned recipient (3) + the ambient row (1)");
    }

    /// **A redelivered / repeated mention COLLAPSES — one row per recipient, never a duplicate.** The
    /// SAME mention delivered twice (distinct broker ids so the consumer-dedup ledger does not
    /// short-circuit) collapses on the per-recipient dedup key (`coalesce_count += 1`); it does NOT
    /// open a second row for that recipient (write-fanout is idempotent on the write side).
    #[test]
    fn write_fanout_repeat_mention_collapses_one_row_per_recipient() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal("pr_review", Severity::Info, "myelin://acme/git/pr/9", "pr-9");
        let mentions = [mentioned("p-alice")];

        consumer.deliver(&signal_msg_with_mentions("evt-a", &sig, &mentions));
        consumer.deliver(&signal_msg_with_mentions("evt-b", &sig, &mentions));

        let dedup = "mention:pr_review:pr-9:p-alice";
        let row = inbox.get(&tenant(), "p-alice", dedup).unwrap();
        assert_eq!(row.coalesce_count, 2, "the repeated mention collapsed (one row, count 2)");
        // alice has exactly ONE mention row (the ambient skeleton row is the only other row).
        let alice_rows = inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .filter(|r| r.recipient == "p-alice")
            .count();
        assert_eq!(alice_rows, 1, "exactly one row for the mentioned recipient (no duplicate)");
    }

    /// **A Signal with NO structured mention nodes fans out NOTHING (no free-text parse).** A
    /// content-less Signal (a CI failure) routes only the ambient skeleton candidate — there is no
    /// free-text fallback, so 0 mention rows. The AG-6 property: the only recipient source is the
    /// structured node.
    #[test]
    fn no_mention_nodes_means_no_write_fanout() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/42", "run-42");
        // The plain Signal envelope carries NO `mentions` key (only the serialized Signal).
        consumer.deliver(&signal_msg("evt-1", &sig));
        assert_eq!(inbox.len(), 1, "only the ambient skeleton row — 0 mention write-fanout rows");
    }

    /// **The hot-subject-cap check (NOTIF-P12 GATE / NOTIF-D2): past the cap, a mention-storm on a hot
    /// subject COALESCES rather than write-amplifies.** A spray of distinct mentions on ONE subject is
    /// bounded by the hot-subject cap: at most `cap` mention rows materialise; the rest overflow into
    /// the coalesced marker. Threshold: write rows bounded by the cap; 0 unbounded write amplification.
    #[test]
    fn write_fanout_hot_subject_cap_bounds_a_mention_storm() {
        let outbox = OutboxStore::new();
        let inbox = InboxProjection::new();
        // A SMALL cap so the test exercises the overflow without thousands of rows. Build the router
        // and replace its hot_cap with a cap-5 one (the SAME bound, smaller for the test).
        let mut router = SignalRouter::new(
            inbox.clone(),
            outbox.clone(),
            Arc::new(MonotonicMinter::new()),
            Box::leak(vec![SubjectPattern("sig.acme.".into())].into_boxed_slice()),
        );
        router.hot_cap = HotSubjectCap::with_cap(5);

        // A mention-storm: 50 DISTINCT recipients mentioned on ONE hot subject_root.
        let sig = signal("mention_spray", Severity::Info, "myelin://acme/chat/thread/hot", "spray");
        let storm: Vec<Principal> = (0..50).map(|i| mentioned(&format!("p-{i}"))).collect();
        let _ = router.route(&signal_envelope("evt-storm", &sig));

        let subject_root = "myelin://acme/chat/thread/hot";
        // Drive the storm through write_fanout directly (one Signal envelope carrying 50 mentions).
        let env = {
            let mut e = signal_envelope("evt-storm-2", &sig);
            let nodes: Vec<InlineNode> = storm.iter().cloned().map(InlineNode::Mention).collect();
            if let serde_json::Value::Object(map) = &mut e.payload {
                map.insert(SIGNAL_MENTIONS_KEY.into(), serde_json::to_value(&nodes).unwrap());
            }
            e
        };
        router.write_fanout(&env, &sig).unwrap();

        // BOUNDED: at most `cap` (5) distinct mention rows materialised on the hot subject_root; the
        // other 45 overflowed into the coalesced marker (counted, never lost — bounded, not 50 rows).
        assert_eq!(
            router.hot_cap().admitted_count(subject_root),
            5,
            "the mention-storm is bounded to `cap` write rows (0 unbounded write amplification)"
        );
        assert_eq!(
            router.hot_cap().overflow_count(subject_root),
            45,
            "the rest overflowed into the coalesced marker (the +N more were mentioned counter)"
        );
        // The inbox holds the bounded mention rows (5) — NOT 50. A mention-storm cannot write-amplify.
        let mention_rows = inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .filter(|r| r.reason == Reason::Mentioned)
            .count();
        assert_eq!(mention_rows, 5, "exactly `cap` mention rows materialised (bounded write-fanout)");
    }

    /// **`SIGNAL_MENTIONS_KEY` is the frozen wire key** — the named constant the CDC pins (producer +
    /// consumer agree on it). A mutant that renames it breaks the build, never silently in prod.
    #[test]
    fn signal_mentions_key_is_frozen() {
        assert_eq!(SIGNAL_MENTIONS_KEY, "mentions");
    }
}
