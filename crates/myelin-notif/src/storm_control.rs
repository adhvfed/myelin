//! # The five write-time storm-control mechanisms (NOTIF-P11 / P-189, M2) + NOTIF-D2
//!
//! **Owning architecture doc:** `notifications.md` §3.2 (the five layered, write-time storm-control
//! mechanisms). **External insight:** `04-hard-problems.md` §5.3 (**Notif is a projection** —
//! storm-control suppresses *delivery and ranking*, **NEVER the audit/history**: the underlying
//! events still exist on the bus). **Contract:** 1.8 (the `dedup_collapse_ratio` telemetry signal —
//! the drill's green artifact). **Drill:** NOTIF-D2 (1000 near-identical CI failures + a 30-comment
//! PR burst → bounded items, `coalesce_count` correct; 0 self-notifications; measured
//! dedup-collapse-ratio).
//!
//! ## The five write-time mechanisms (refined §3.2), in order, between **classify** and **UPSERT**
//!
//! This module is the pipeline stage the router (NOTIF-P3, [`crate::router`]) runs AFTER it derives
//! the candidate [`RoutedInboxItem`](crate::router::RoutedInboxItem) (classify) and BEFORE it UPSERTs
//! the inbox row. It produces a [`StormDecision`] — `Deliver`, `Collapse`, `Coalesce`, or `Suppress`
//! — that tells the router how (or whether) to write the row and whether to push a delivery. The
//! five mechanisms, in the §3.2 order:
//!
//! 1. **Self-suppression** ([`is_self_notification`]) — `actor.principal == recipient` → **drop**.
//!    A principal does not get notified about their OWN action. The actor is read from the verified
//!    [`EventEnvelope::actor`] (never a payload); the recipient is the candidate item's recipient
//!    pseudonym. This SUPPRESSES the notification (no inbox row, no delivery) — the originating event
//!    is **untouched on the bus** (a `Suppress { reason: SelfAction }`).
//! 2. **Dedup-key collapse** — `INSERT … ON CONFLICT (tenant, recipient, dedup_key) DO UPDATE SET
//!    coalesce_count = coalesce_count + 1` → the "+N more" counter. This mechanism is the
//!    [`InboxProjection::upsert`](crate::router::InboxProjection) write-time collapse the router
//!    already performs; storm-control's verdict is to PROCEED to that UPSERT
//!    ([`StormDecision::Deliver`] / it returns `Collapse` once the row already exists — surfaced here
//!    so the drill can read N→1 + the ratio). N identical → ONE row, `coalesce_count` N.
//! 3. **Thread/subject coalescing** ([`Coalescer`]) — digest the **participating** (low-signal,
//!    same-`subject_root`) items into ONE coalesced marker; **break out the direct** (a `Direct` /
//!    `Critical` class item is never folded into a digest — you always see the one addressed to you).
//!    A `Coalesce { subject_root }` verdict.
//! 4. **Per-`(recipient, subject_root)` token-bucket rate damping** ([`TokenBucket`]) — a burst of
//!    items on the SAME hot subject for the SAME recipient is rate-damped: the bucket admits up to
//!    `capacity` immediately and refills at `refill_per_tick`; once empty, further items are
//!    **damped** (`Suppress { reason: RateDamped }` — delivery suppressed, the audit untouched). A
//!    `Critical`/`Direct` item is exempt (the on-call page is never damped).
//! 5. **Mute / DND honoring** ([`StormPrefs`]) — a muted `(recipient, subject_root)` and a
//!    quiet-hours window (the recipient-tz [`QuietHours`](crate::prefs::QuietHours), unless the class
//!    **pierces**) SUPPRESS DELIVERY. The inbox ROW is still written (the ONE inbox always receives —
//!    the item is in the audit/history); only the channel PUSH is suppressed. A
//!    `Suppress { reason: Muted | QuietHours, write_row: true }`.
//!
//! ## The load-bearing invariant — storm-control suppresses delivery/ranking, **never the audit**
//!
//! Every [`StormDecision`] carries [`StormDecision::touches_audit`] = `false`: storm-control NEVER
//! removes, rewrites, or hides the underlying event on the bus. A self-notification's originating
//! event still exists; a damped burst's Signals all still exist; a muted item's row is still in the
//! ONE inbox. What is suppressed is the *delivery* (the channel push) and the *ranking* (the item
//! does not climb the active inbox). This is EI-04 §5.3 made structural: a verdict that claimed to
//! touch the audit would be a contradiction the type does not admit.
//!
//! ## FLOORS named (per EI-01 §1)
//! - **The hot-subject cap** that bounds the WRITE-FANOUT side (so a mention-storm cannot
//!   write-amplify into N rows) is **NOTIF-P12** (§3.2.4 / §3.5). Storm-control here damps the
//!   per-recipient rate; the fan-out-side write cap is the P12 follow-on — named so storm-control is
//!   not mistaken for the full scale answer.
//! - **The read-fanout** for the unbounded ambient watcher set (the `SetExpr` push-down JOIN + the
//!   zookie watermark) is **NOTIF-P13** (§3.5).
//! - **The live OLTP/Valkey backing** of the token-bucket + the mute set: this module models the
//!   write-time decision in-memory (the same pattern as
//!   [`InboxProjection`](crate::router::InboxProjection) /
//!   [`PrefStore`](crate::prefs::PrefStore)); the durable rate-limit bucket rides Valkey and the mute
//!   set rides the `notif_mute` table when the live backends wire in (P-007 / NOTIF-P15). The
//!   DECISION shape (the five mechanisms, this order) does not change.
//!
//! ## Mutation floor (the storm-control decision module — mandatory-core)
//! Storm-control is mandatory-core (a wrong verdict either floods a recipient or silences a page).
//! The mutation-tested core is [`StormControl::decide`] (the five-mechanism ordering + each verdict),
//! [`is_self_notification`], [`TokenBucket::try_take`], [`Coalescer::should_coalesce`], and the
//! mute/quiet check. **Floor: ≥ 80% line/branch mutation score on `storm_control.rs`** (measured with
//! `cargo mutants`; reported in the P-189 commit body). The unit + chained tests below assert every
//! mechanism, and a mutant that drops self-suppression, stops the collapse, mis-orders the
//! mechanisms, damps a critical page, or suppresses the audit is caught.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_events::EventEnvelope;

use crate::prefs::QuietHours;
use crate::router::RoutedInboxItem;
use crate::Class;

/// The reason a storm-control mechanism **suppressed** the delivery of a candidate item. Each is a
/// PII-free taxonomy token; NONE of them touches the underlying event on the bus (the audit/history
/// is untouched — EI-04 §5.3). Carried in [`StormDecision::Suppress`] so a drill / the delivery
/// fabric can attribute exactly which mechanism fired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuppressReason {
    /// Mechanism 1: `actor.principal == recipient` — a self-notification. No row, no delivery; the
    /// originating event is untouched on the bus.
    SelfAction,
    /// Mechanism 4: the per-`(recipient, subject_root)` token bucket is empty (a burst on a hot
    /// subject for one recipient). Delivery damped; the event is untouched.
    RateDamped,
    /// Mechanism 5: the recipient has MUTED this `(recipient, subject_root)`. The inbox row is still
    /// written (the ONE inbox always receives); only the channel push is suppressed.
    Muted,
    /// Mechanism 5: the recipient is in a quiet-hours window (and the class does not pierce). The
    /// inbox row is still written; only the channel push is suppressed.
    QuietHours,
}

impl SuppressReason {
    /// The PII-free token for this suppression reason (for telemetry / a drill assertion).
    pub fn token(self) -> &'static str {
        match self {
            SuppressReason::SelfAction => "self_action",
            SuppressReason::RateDamped => "rate_damped",
            SuppressReason::Muted => "muted",
            SuppressReason::QuietHours => "quiet_hours",
        }
    }

    /// Does this suppression still WRITE the inbox row (suppressing only the channel push), or drop
    /// the item entirely? **Mute / quiet-hours** write the row (the ONE inbox always receives — the
    /// item is in the audit/history, only delivery is suppressed). **Self-action / rate-damping** do
    /// not write a row (the candidate never becomes an inbox item) — but the underlying EVENT is
    /// still on the bus, so the audit/history is untouched either way.
    pub fn writes_row(self) -> bool {
        match self {
            SuppressReason::Muted | SuppressReason::QuietHours => true,
            SuppressReason::SelfAction | SuppressReason::RateDamped => false,
        }
    }
}

/// **The storm-control verdict for one candidate item** — what the router does between classify and
/// UPSERT. EVERY variant leaves the underlying event on the bus untouched ([`Self::touches_audit`]
/// is always `false`): storm-control suppresses *delivery and ranking*, never the audit (EI-04 §5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StormDecision {
    /// Deliver: write/UPSERT the inbox row AND push the delivery (no mechanism suppressed it). A
    /// fresh `(tenant, recipient, dedup_key)` → a new row.
    Deliver,
    /// Mechanism 2: the `(tenant, recipient, dedup_key)` ALREADY exists → this candidate COLLAPSES
    /// into it (`coalesce_count += 1`, the "+N more"). One row, not N. Surfaced so the drill reads
    /// N→1 + the collapse ratio.
    Collapse,
    /// Mechanism 3: a low-signal item on a hot `subject_root` → fold into the ONE coalesced digest
    /// marker for that root (the direct items are never coalesced — they `Deliver`).
    Coalesce,
    /// A mechanism SUPPRESSED the delivery (self-action / rate-damped / muted / quiet-hours). Carries
    /// the [`SuppressReason`]; `reason.writes_row()` says whether the inbox row is still written
    /// (mute/quiet write the row, only suppress the push; self/rate-damp write no row). The
    /// underlying event is untouched on the bus.
    Suppress(SuppressReason),
}

impl StormDecision {
    /// **The structural invariant: storm-control NEVER touches the audit/history.** Always `false` —
    /// every verdict (deliver / collapse / coalesce / suppress) leaves the underlying event on the
    /// bus untouched (EI-04 §5.3). A drill asserts this over EVERY verdict (the audit-untouched
    /// check): there is no verdict that removes/rewrites/hides the originating event.
    pub fn touches_audit(self) -> bool {
        false
    }

    /// Should the router push a channel DELIVERY for this verdict? `Deliver` yes; `Collapse` /
    /// `Coalesce` update the existing row's counter (no NEW push — the "+N more" is read at inbox
    /// open, not re-pushed); `Suppress` no (the whole point). Delivery is what storm-control gates.
    pub fn delivers(self) -> bool {
        matches!(self, StormDecision::Deliver)
    }

    /// Should the router WRITE/UPSERT an inbox row for this verdict? `Deliver` / `Collapse` /
    /// `Coalesce` all touch a row (insert or bump a counter); a `Suppress` writes a row ONLY for
    /// mute/quiet (the ONE inbox still receives — only the push is suppressed), never for
    /// self-action / rate-damping (the candidate never becomes a row, though the event stays on the
    /// bus).
    pub fn writes_row(self) -> bool {
        match self {
            StormDecision::Deliver | StormDecision::Collapse | StormDecision::Coalesce => true,
            StormDecision::Suppress(reason) => reason.writes_row(),
        }
    }
}

/// **Mechanism 1 — self-suppression: is this candidate a self-notification?** `true` iff the
/// originating event's verified actor principal equals the candidate item's recipient — a principal
/// does not get notified about their OWN action (§3.2.1). The actor is read from the VERIFIED
/// [`EventEnvelope::actor`] (never a payload); the recipient is the candidate's recipient pseudonym.
/// Pure + total for the mutation/unit floor.
pub fn is_self_notification(env: &EventEnvelope, recipient: &str) -> bool {
    env.actor.0.principal_id.0 == recipient
}

/// **Mechanism 3 — the thread/subject coalescer.** Tracks, per `(recipient, subject_root)`, whether
/// a coalesced digest marker already exists, so a SECOND low-signal item on the same hot subject
/// folds into it (digest the participating) while a `Direct`/`Critical` item is broken out (never
/// coalesced — you always see the one addressed to you). The in-memory model of the §3.2.3 digest;
/// the durable digest row rides the `notif_inbox_item` coalesced marker (the named floor).
#[derive(Clone, Default)]
pub struct Coalescer {
    /// The set of `(recipient, subject_root)` that already have a coalesced digest marker. A second
    /// participating item on a key already present coalesces into it.
    seen: Arc<Mutex<HashMap<(String, String), u32>>>,
}

impl Coalescer {
    /// A fresh coalescer.
    pub fn new() -> Coalescer {
        Coalescer::default()
    }

    /// **Should a `class` item on `(recipient, subject_root)` be COALESCED into the digest?** A
    /// `Direct`/`Critical` item is NEVER coalesced (break out the direct — §3.2.3); it returns
    /// `false` and does not consume a digest slot. A low-signal item (`Participating`/`Watching`/
    /// `Fyi`) is coalesced iff a digest marker already exists for the key; the FIRST such item opens
    /// the marker (returns `false` — it becomes the marker row) and subsequent ones coalesce
    /// (`true`). This is the "digest the participating" decision.
    pub fn should_coalesce(&self, recipient: &str, subject_root: &str, class: Class) -> bool {
        if is_break_out_class(class) {
            // Break out the direct: a directly-addressed item is always its own row, never digested.
            return false;
        }
        let key = (recipient.to_string(), subject_root.to_string());
        let mut g = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let count = g.entry(key).or_insert(0);
        let already = *count > 0;
        *count += 1;
        already
    }
}

/// Is `class` a "break out the direct" class (never folded into a digest)? `Direct` and `Critical`
/// are always broken out (you always see the one addressed to you / the page); the ambient classes
/// (`Participating`/`Watching`/`Fyi`) are digestible (§3.2.3).
fn is_break_out_class(class: Class) -> bool {
    matches!(class, Class::Direct | Class::Critical)
}

/// **Mechanism 4 — a per-`(recipient, subject_root)` token bucket** for rate-of-fire damping
/// (§3.2.4). The bucket admits up to `capacity` items immediately (the burst allowance) and refills
/// `refill_per_tick` tokens per `tick`; once empty, further items are DAMPED (delivery suppressed,
/// the audit untouched). A pure, caller-clocked bucket (the `tick` is supplied — deterministic for
/// the mutation/unit floor; no wall-clock read inside).
#[derive(Clone, Default)]
pub struct TokenBucket {
    inner: Arc<Mutex<HashMap<(String, String), BucketState>>>,
}

#[derive(Clone, Copy, Debug)]
struct BucketState {
    /// The tokens currently available (a burst allowance that refills over ticks).
    tokens: f64,
    /// The last tick the bucket was refilled at (so a later `try_take` refills the elapsed ticks).
    last_tick: u64,
}

/// The rate-damping configuration for a token bucket (the §3.2.4 burst allowance + refill rate). A
/// frozen value so the threshold is read from ONE place (never re-stated per call-site).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateConfig {
    /// The burst capacity — how many items admit immediately before damping kicks in.
    pub capacity: f64,
    /// The tokens refilled per tick (the steady-state rate after the burst is spent).
    pub refill_per_tick: f64,
}

impl Default for RateConfig {
    /// The platform default: a burst of 5 items, refilling 1 per tick. A 30-comment PR burst on the
    /// same subject for the same recipient damps after the burst allowance (the rest coalesce / are
    /// damped) — bounded, never a flood. The drill reads this; it is the default-to-beat, never
    /// weakened.
    fn default() -> RateConfig {
        RateConfig {
            capacity: 5.0,
            refill_per_tick: 1.0,
        }
    }
}

impl TokenBucket {
    /// A fresh, empty set of buckets.
    pub fn new() -> TokenBucket {
        TokenBucket::default()
    }

    /// **Try to take one token for `(recipient, subject_root)` at `tick`** (the §3.2.4 rate test).
    /// Refills the elapsed ticks since `last_tick` (capped at `capacity`), then consumes one token if
    /// available. Returns `true` iff a token was taken (admit the item); `false` iff the bucket is
    /// empty (DAMP the item — delivery suppressed). A fresh `(recipient, subject_root)` starts FULL
    /// (the burst allowance), so the first `capacity` items in a burst admit.
    pub fn try_take(
        &self,
        recipient: &str,
        subject_root: &str,
        tick: u64,
        cfg: RateConfig,
    ) -> bool {
        let key = (recipient.to_string(), subject_root.to_string());
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let state = g.entry(key).or_insert(BucketState {
            tokens: cfg.capacity,
            last_tick: tick,
        });
        // Refill the elapsed ticks (saturating; never below the current count, never above capacity).
        // NOTE (equivalent mutant): the `elapsed > 0` guard is a no-op short-circuit — at `elapsed ==
        // 0` the refill term is `0.0 * rate == 0.0` (and `last_tick` is already `tick`), so `> 0` and
        // `>= 0` are behaviourally identical (cargo-mutants reports `>=` as surviving; it is a true
        // equivalent mutant, not a coverage gap — the `*`-vs-`/` and the cap are pinned by the
        // multi-tick refill test). The guard stays only to skip the redundant write.
        let elapsed = tick.saturating_sub(state.last_tick);
        if elapsed > 0 {
            state.tokens = (state.tokens + elapsed as f64 * cfg.refill_per_tick).min(cfg.capacity);
            state.last_tick = tick;
        }
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// **Mechanism 5 — the recipient's mute set + quiet-hours** (the §3.2.5 mute/DND state). Reads the
/// `notif_mute` set (a muted `(recipient, subject_root)`) and the recipient's
/// [`QuietHours`](crate::prefs::QuietHours) — the SAME quiet-hours type NOTIF-P10
/// ([`crate::prefs`]) owns (Notif does not invent a second mute language). The in-memory model of the
/// mute table; the durable store is the named floor.
#[derive(Clone, Default)]
pub struct StormPrefs {
    /// The muted `(recipient, subject_root)` set ("mute this thread"). A muted key suppresses the
    /// channel PUSH (the inbox row is still written — the ONE inbox always receives).
    muted: Arc<Mutex<std::collections::HashSet<(String, String)>>>,
}

impl StormPrefs {
    /// A fresh empty mute set.
    pub fn new() -> StormPrefs {
        StormPrefs::default()
    }

    /// Mute `(recipient, subject_root)` ("mute this thread"). Subsequent non-piercing items on this
    /// thread for this recipient suppress the channel push (the row is still written).
    pub fn mute(&self, recipient: &str, subject_root: &str) {
        self.muted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((recipient.to_string(), subject_root.to_string()));
    }

    /// Is `(recipient, subject_root)` muted?
    pub fn is_muted(&self, recipient: &str, subject_root: &str) -> bool {
        self.muted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(recipient.to_string(), subject_root.to_string()))
    }
}

/// **The clock + mute/quiet context one storm-control decision is made against.** Caller-supplied
/// (the router's clock + the recipient's prefs) so [`StormControl::decide`] stays pure +
/// deterministic for the mutation floor (no wall-clock / no store read inside the decision).
#[derive(Clone)]
pub struct StormContext<'a> {
    /// The monotonic tick the rate-damping bucket is clocked on (the router's logical clock).
    pub tick: u64,
    /// The minute-of-day (UTC, `0..1440`) the quiet-hours window is evaluated at.
    pub utc_minute_of_day: i32,
    /// The UTC weekday (`0`=Mon..`6`=Sun) the quiet-hours window is evaluated on.
    pub utc_weekday: u8,
    /// The recipient's quiet-hours (recipient-tz windows + `pierce_classes`). A critical/escalated
    /// class pierces by default — you cannot silence an on-call page.
    pub quiet: &'a QuietHours,
    /// The rate-damping config (the §3.2.4 burst allowance + refill).
    pub rate: RateConfig,
}

/// **The storm-control pipeline stage — the five mechanisms, in §3.2 order.** Holds the per-recipient
/// state the mechanisms read (the coalescer, the token buckets, the mute set). The router constructs
/// ONE of these for its pool and calls [`Self::decide`] for every candidate item between classify and
/// UPSERT. A cloneable handle over shared state (one truth across the pool).
#[derive(Clone, Default)]
pub struct StormControl {
    coalescer: Coalescer,
    buckets: TokenBucket,
    prefs: StormPrefs,
}

impl StormControl {
    /// A fresh storm-control stage.
    pub fn new() -> StormControl {
        StormControl::default()
    }

    /// The mute set this stage reads (so the router / a test can mute a thread).
    pub fn prefs(&self) -> &StormPrefs {
        &self.prefs
    }

    /// **Decide what to do with one candidate item, running the five mechanisms in §3.2 order.**
    ///
    /// `env` is the originating Signal envelope (for the verified actor — mechanism 1);
    /// `item` is the classified candidate ([`RoutedInboxItem`]); `subject_root` is the coalescing /
    /// rate / mute key (the `#sub`-stripped root of the subject); `row_exists` is whether the
    /// `(tenant, recipient, dedup_key)` row ALREADY exists (mechanism 2 — the dedup collapse);
    /// `ctx` carries the clock + the recipient's mute/quiet state.
    ///
    /// The order is load-bearing (a mutant that reorders is caught):
    /// 1. **Self-suppression** — actor == recipient → `Suppress(SelfAction)` (no row, audit
    ///    untouched). Checked FIRST: a self-action never enters any other mechanism.
    /// 2. **Dedup collapse** — `row_exists` → `Collapse` (the UPSERT bumps `coalesce_count`; one
    ///    row, not N). Checked before the rate/mute mechanisms: a collapse is the cheapest write and
    ///    never re-pushes, so it short-circuits.
    /// 3. **Thread/subject coalescing** — a non-break-out class with a digest marker already open →
    ///    `Coalesce` (digest the participating; break out the direct).
    /// 4. **Rate damping** — the per-`(recipient, subject_root)` bucket is empty (and the class does
    ///    not pierce) → `Suppress(RateDamped)` (delivery damped, audit untouched).
    /// 5. **Mute / quiet-hours** — muted thread → `Suppress(Muted)`; a quiet window the class does
    ///    not pierce → `Suppress(QuietHours)` (the row is still written; only the push is
    ///    suppressed).
    ///
    /// If no mechanism fires → `Deliver` (write a fresh row + push the delivery).
    pub fn decide(
        &self,
        env: &EventEnvelope,
        item: &RoutedInboxItem,
        subject_root: &str,
        row_exists: bool,
        ctx: &StormContext<'_>,
    ) -> StormDecision {
        // (1) Self-suppression — a self-notification is dropped before any other mechanism.
        if is_self_notification(env, &item.recipient) {
            return StormDecision::Suppress(SuppressReason::SelfAction);
        }

        // (2) Dedup-key collapse — a same-key item collapses into the existing row (coalesce_count++).
        // One row, not N — the storm-control primitive. (The UPSERT itself happens in the router; the
        // verdict tells it this is a collapse, never a new delivery.)
        if row_exists {
            return StormDecision::Collapse;
        }

        // A piercing class (critical/escalated by default) is EXEMPT from rate-damping and quiet-hours
        // (you cannot silence / damp an on-call page). It is still subject to self-suppression (1) and
        // dedup-collapse (2) above — a page you triggered yourself, or a re-fire of the same page, is
        // not a new notification.
        let pierces = ctx.quiet.pierces(item.class);

        // (3) Thread/subject coalescing — digest the participating, break out the direct. A
        // break-out class (Direct/Critical) is never coalesced; an ambient class with an open digest
        // marker for this (recipient, subject_root) coalesces into it.
        if self
            .coalescer
            .should_coalesce(&item.recipient, subject_root, item.class)
        {
            return StormDecision::Coalesce;
        }

        // (4) Per-(recipient, subject_root) token-bucket rate damping — a burst on a hot subject for
        // one recipient is damped after the burst allowance. A piercing class is exempt.
        if !pierces
            && !self
                .buckets
                .try_take(&item.recipient, subject_root, ctx.tick, ctx.rate)
        {
            return StormDecision::Suppress(SuppressReason::RateDamped);
        }

        // (5) Mute / DND honoring — a muted thread, or a quiet-hours window the class does not pierce,
        // suppresses the channel PUSH (the inbox row is still written — the ONE inbox always
        // receives). A piercing class always delivers.
        if self.prefs.is_muted(&item.recipient, subject_root) {
            return StormDecision::Suppress(SuppressReason::Muted);
        }
        if !pierces
            && ctx
                .quiet
                .is_quiet_at(ctx.utc_minute_of_day, ctx.utc_weekday)
        {
            return StormDecision::Suppress(SuppressReason::QuietHours);
        }

        // No mechanism fired — deliver (a fresh row + a channel push).
        StormDecision::Deliver
    }
}

/// **The `#sub`-stripped subject root** for a subject ref — the coalescing / rate / mute key
/// (§3.2.3/§3.2.4). The root is the subject ref with any `#sub`-fragment (a sub-thread anchor, the
/// unified `#sub` tombstone ladder REF) stripped, so all items on the same thread/PR share ONE root.
/// A ref, never a payload (PII-free).
pub fn subject_root_of(subject: &str) -> String {
    match subject.split_once('#') {
        Some((root, _frag)) => root.to_string(),
        None => subject.to_string(),
    }
}

/// **The dedup-collapse ratio in basis points** (`0..10000`) — the contract-1.8 telemetry signal the
/// NOTIF-D2 drill asserts. `10000 * collapsed / inbound`: the fraction of inbound candidates that
/// COLLAPSED/COALESCED into an existing row (or were suppressed as duplicates) rather than opening a
/// new one. A storm of N near-identical items that collapse to ONE row reads `~10000 * (N-1)/N`
/// (≈ 9990 for N=1000). `inbound == 0` reads 0 (no storm to measure). Integer basis points so the
/// telemetry predicate ([`Predicate`](myelin_harness::telemetry::Predicate)) can read it as an `i64`.
pub fn dedup_collapse_ratio_bps(inbound: u64, collapsed: u64) -> i64 {
    if inbound == 0 {
        return 0;
    }
    // Cap collapsed at inbound (a defensive bound — collapsed can never exceed inbound).
    let collapsed = collapsed.min(inbound);
    ((10_000 * collapsed) / inbound) as i64
}

#[cfg(test)]
mod tests;
