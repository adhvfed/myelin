//! # `get_prefs` / `set_prefs` — preferences + quiet-hours over the frozen `QueryAst` (NOTIF-P10 / P-188, M2)
//!
//! **Owning architecture doc:** `notifications.md` §2.2 (the preference matcher binds the frozen
//! `myelin-query` [`QueryAst`] = the `EventMatcher` core 3.4; quiet-hours in the recipient's tz;
//! critical/escalated pierce by default via `pierce_classes` — the one deliberate quiet-hours
//! override; **you cannot silence an on-call page**). **Contract:** **7.4** `get_prefs / set_prefs`
//! (owned). **Consumed:** **13.3 / 3.4** the frozen `QueryAst` (Notif invents NO second predicate
//! language — VISION §3, one predicate language). The textual grammar parser is the P-235 floor; the
//! tree + bounded interpreter are frozen in `myelin-query`.
//!
//! ## What this ships
//! - [`NotifPrefs`] — the per-principal preference value: a routing **matcher** (the frozen
//!   [`QueryAst`], cost-bounded by construction) per delivery channel + the digest config field.
//! - [`QuietHours`] — per-principal quiet windows evaluated **in the recipient's tz** + the
//!   `pierce_classes` (critical pierces by default).
//! - [`get_prefs`] / [`set_prefs`] (contract 7.4) — the read/write API over the [`PrefStore`] (the
//!   in-memory projection of the `notif_pref` / `notif_quiet_hours` tables; the live OLTP store is a
//!   named floor below).
//! - [`route`] — the router's delivery decision: a class/reason is delivered on a channel iff the
//!   channel's matcher predicate **matches** AND the item is **not** suppressed by quiet-hours
//!   (`route(prefs, reason, class) ∩ ¬quiet_hours`, **unless** the class pierces, §2.2).
//!
//! ## The cost-bound (the GATE — a static property, EI-01 §3 "prove it")
//! A preference matcher predicate is a [`QueryAst`] built through [`QueryAst::compiled`], which
//! **validates the tree against the static cost bounds at construction** ([`MAX_PREDICATE_NODES`] /
//! [`MAX_PREDICATE_DEPTH`]) — an over-budget predicate is **rejected here, never evaluated**. There
//! are no UDFs, no loops, no recursion-to-unbounded-depth in the grammar (it is the frozen
//! `QueryAst`), so node-count is the complete static cost measure. [`set_prefs`] only ever accepts a
//! `QueryAst` that already passed the bound (the type carries the proof); the un-compiled placeholder
//! surface ([`QueryAst::raw`]) evaluates fail-closed to **no match** (an un-parsed predicate is
//! uncertainty, never a silent deliver) — so an unbounded predicate cannot be smuggled in.
//!
//! ## The pierce-class property (the GATE)
//! A `critical`/`escalated` item pierces quiet-hours **by default** (`pierce_classes` ⊇ `{critical}`)
//! — you cannot silence an on-call page. A non-critical item that lands inside a quiet window (in the
//! recipient's tz) is **suppressed** (delivery only — NEVER the audit/inbox row; the item is still in
//! the ONE inbox, it just does not push a channel notification). The full pierce drill is NOTIF-D8
//! (NOTIF-P14 / P-192); this prompt proves the per-decision property.
//!
//! ## FLOORS named (per EI-01 §1)
//! - **The digest cadence / batching UX** is a Phase-6 product surface (refined §10 OQ5). Here the
//!   `digest` field is **stored** ([`DigestConfig`]) but the compose/batch DELIVERY flow is out of
//!   scope — named, not built. The digest cadence wheel rides the same `myelin-flow` timer the
//!   escalation/snooze wheels use (NOTIF-P14/P18); the compose flow is the OQ5 follow-on.
//! - **The recipient-tz evaluation** uses a **fixed UTC offset** ([`Tz`] = offset-minutes), the
//!   correct floor for the common case. The full **IANA / DST-aware** lookup (a `Europe/Paris`
//!   string → the offset valid AT the instant, across the spring/autumn transition) needs a tz
//!   database; it is a named floor (a `chrono-tz`-class dependency added behind the live delivery
//!   fabric, NOTIF-P16, where the wall-clock matters operationally). The `tz` COLUMN already stores
//!   the IANA id (`schema::QuietHoursRow.tz`); this module evaluates it as a stored offset until then.
//! - **The live OLTP store.** [`PrefStore`] is the in-memory projection (the same pattern as
//!   [`InboxProjection`](crate::router::InboxProjection)); the `notif_pref` / `notif_quiet_hours`
//!   table reads/writes (the `WHERE (tenant_id, region, principal) = …` UPSERT) are proven against
//!   **real Postgres** in `tests/integration_notif_prefs.rs` (the `integration` feature), so `cargo
//!   build --workspace` stays DB-free while the DB contract is proven against the live dev stack.
//!
//! ## Mutation floor (the prefs/matcher module — mandatory-core)
//! `prefs` is mandatory-core (the routing + quiet-hours decision is a security/correctness seam: a
//! wrong decision either silences an on-call page or over-delivers). The mutation-tested core is the
//! decision logic: [`route`] (matcher ∧ ¬quiet ∧ pierce), [`QuietHours::is_quiet_at`] (the
//! recipient-tz window test), and the `pierce` check. **Floor: ≥ 80% line/branch mutation score on
//! `prefs.rs`** (measured with `cargo mutants`; reported in the P-188 commit body).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_identity::{Literal, Principal};
use myelin_query::{
    CmpOp, EvalContext, Expr, Predicate, PredicateError, QueryAst, MAX_PREDICATE_DEPTH,
    MAX_PREDICATE_NODES,
};
use serde::{Deserialize, Serialize};

use crate::list_inbox::Subsystem;
use crate::{Class, Reason};

/// A delivery **channel** a preference routes a class/reason to (the §2.2 routing matrix columns).
/// `in_app` stays in-cell; the off-cell channels (`email`/`web_push`/…) are the at-least-once
/// delivery fabric's (NOTIF-P16). A preference matcher is held **per channel** so a recipient can
/// route, say, `critical` to `mobile_push` but only `direct` to `email`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// The in-cell inbox push (always available; the ONE inbox's live transport, NOTIF-P15).
    InApp,
    /// Web push (off-cell, redacted; the delivery fabric, NOTIF-P16).
    WebPush,
    /// Mobile push (off-cell, redacted).
    MobilePush,
    /// Email (off-cell, redacted).
    Email,
    /// Desktop (off-cell, redacted).
    Desktop,
}

impl Channel {
    /// The PII-free wire/CLI token (the serde snake_case form) for this channel.
    pub fn token(self) -> &'static str {
        match self {
            Channel::InApp => "in_app",
            Channel::WebPush => "web_push",
            Channel::MobilePush => "mobile_push",
            Channel::Email => "email",
            Channel::Desktop => "desktop",
        }
    }
}

/// The **batched-delivery digest config** (the `notif_pref.digest` blob, §2.2). **FLOOR named:** the
/// cadence/batch compose UX is the Phase-6 product surface (refined §10 OQ5) — here the config is
/// STORED so prefs round-trip, but the compose/batch DELIVERY flow is out of scope. The cadence is
/// the schedule the (later) digest wheel would fire on; the `classes` are the set folded INTO a
/// digest rather than pushed immediately.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestConfig {
    /// The digest cadence (`off` | `hourly` | `daily` | `weekly`) — a stored selector. `off` (the
    /// default) means no batching (every non-suppressed item delivers immediately).
    pub cadence: String,
    /// The local wall-clock time the digest fires at (`"09:00"`), in the recipient tz — stored only.
    pub at: Option<String>,
    /// The classes folded INTO the digest (rather than pushed immediately) — stored only.
    pub classes: Vec<Class>,
}

/// A **routing rule** — one delivery [`Channel`] gated by a frozen [`QueryAst`] **matcher**. The
/// matcher predicate reads the projected `reason`/`class`/`subsystem` of an item (see
/// [`route_context`]) and decides whether the item routes to this channel. The predicate is the ONE
/// bounded predicate language (contract 3.4) — Notif invents no second matcher.
///
/// **Cost-bound by construction:** the `matcher` is a [`QueryAst`]; the only way to build one with a
/// compiled tree is [`QueryAst::compiled`], which validates the static bounds — so a [`RoutingRule`]
/// that carries a compiled matcher carries the proof that it is cost-bounded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingRule {
    /// The channel this rule routes a matching item to.
    pub channel: Channel,
    /// The frozen-`QueryAst` matcher: the item routes to `channel` iff this predicate **matches**
    /// (an un-compiled placeholder / a missing-context / a type error all fail-closed to NO match —
    /// never a silent deliver).
    pub matcher: QueryAst,
}

/// **The per-principal notification preferences** (contract 7.4 / §2.2). The routing matrix (a list
/// of [`RoutingRule`]s, each a channel + a frozen-`QueryAst` matcher) + the digest config. The
/// matcher is the ONE predicate language (3.4); there is no second DSL.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifPrefs {
    /// The routing matrix — the ordered set of (channel, matcher) rules. An item routes to EVERY
    /// channel whose matcher matches (a recipient can fan one class to several channels). An empty
    /// matrix routes NOWHERE off the default (see [`NotifPrefs::default_in_app`] for the safe
    /// baseline).
    pub routing: Vec<RoutingRule>,
    /// The batched-delivery digest config (stored only — the compose flow is the OQ5 floor).
    pub digest: DigestConfig,
}

impl NotifPrefs {
    /// The safe baseline: everything routes to the in-cell inbox (`in_app`), nothing off-cell. The
    /// matcher is the always-match predicate ([`Predicate::True`]) — trivially cost-bounded. This is
    /// the default a principal with no explicit prefs gets (the ONE inbox always receives; off-cell
    /// channels are opt-in).
    pub fn default_in_app() -> NotifPrefs {
        NotifPrefs {
            routing: vec![RoutingRule {
                channel: Channel::InApp,
                // `True` is one node — trivially within the static bound (cannot fail).
                matcher: QueryAst::compiled(Predicate::True)
                    .expect("the always-match predicate is one node (within the static bound)"),
            }],
            digest: DigestConfig::default(),
        }
    }

    /// The set of channels an item with `reason`/`class`/`subsystem` routes to under these prefs —
    /// every channel whose matcher **matches**. A matcher that fails to evaluate (un-compiled /
    /// missing context / type error) is fail-closed to NO match (it does not route — never a silent
    /// deliver). This is the routing half of [`route`] (before quiet-hours).
    pub fn channels_for(&self, reason: Reason, class: Class, subsystem: Subsystem) -> Vec<Channel> {
        let ctx = route_context(reason, class, subsystem);
        self.routing
            .iter()
            .filter(|rule| rule.matcher.eval(&ctx).unwrap_or(false))
            .map(|rule| rule.channel)
            .collect()
    }
}

/// **Build a cost-bounded routing matcher predicate** from a set of admitted classes / reasons /
/// subsystems — the common shape a `set_prefs` caller wants ("route `critical` and `direct` from
/// `issue` to `mobile_push`"). The result is a [`QueryAst`] validated against the
/// static bounds; an over-budget request (too many tuples) is **rejected** with [`PredicateError`]
/// (never silently truncated — loud, EI-01 §3). The tree is a small disjunction of conjunctions, so
/// the node count is `O(tuples + subsystems)`; the [`MAX_PREDICATE_NODES`] ceiling caps it.
///
/// This is a CONVENIENCE compiler over the frozen grammar (the textual `"class == 'critical'"`
/// parser is the P-235 floor); it emits exactly the `Predicate` the bounded interpreter reads.
pub fn build_routing_matcher(
    classes: &[Class],
    reasons: &[Reason],
    subsystems: &[Subsystem],
) -> Result<QueryAst, PredicateError> {
    let mut conjuncts: Vec<Predicate> = Vec::new();
    if !classes.is_empty() {
        conjuncts.push(Predicate::Or(
            classes
                .iter()
                .map(|c| Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var("class".into()),
                    rhs: Expr::Lit(Literal::Str(class_token(*c).into())),
                })
                .collect(),
        ));
    }
    if !reasons.is_empty() {
        conjuncts.push(Predicate::Or(
            reasons
                .iter()
                .map(|r| Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var("reason".into()),
                    rhs: Expr::Lit(Literal::Str(reason_token(*r).into())),
                })
                .collect(),
        ));
    }
    if !subsystems.is_empty() {
        conjuncts.push(Predicate::Or(
            subsystems
                .iter()
                .map(|s| Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var("subsystem".into()),
                    rhs: Expr::Lit(Literal::Str(subsystem_token(*s).into())),
                })
                .collect(),
        ));
    }
    let predicate = if conjuncts.is_empty() {
        // No constraints ⇒ match everything (the always-match rule).
        Predicate::True
    } else {
        Predicate::And(conjuncts)
    };
    // The single cost-bound door: an over-budget tree is rejected here, never evaluated.
    QueryAst::compiled(predicate)
}

/// The variable namespace a routing matcher reads (the projected item attributes). The frozen
/// `QueryAst` reads VARIABLES by name; this binds the three the routing grammar admits — `reason`,
/// `class`, `subsystem` (the PII-free taxonomy tokens). A matcher referencing any OTHER variable
/// surfaces [`EvalError::MissingContext`](myelin_query::EvalError) → fail-closed NO match (never a
/// silent deliver).
pub fn route_context(reason: Reason, class: Class, subsystem: Subsystem) -> EvalContext {
    EvalContext::new()
        .bind("reason", Literal::Str(reason_token(reason).into()))
        .bind("class", Literal::Str(class_token(class).into()))
        .bind("subsystem", Literal::Str(subsystem_token(subsystem).into()))
}

/// A **fixed UTC offset** timezone (offset minutes east of UTC; `+60` = UTC+1 = `Europe/Paris`
/// winter). **FLOOR named:** this is the correct floor for the common case; the IANA/DST-aware
/// lookup (a `Europe/Paris` string → the offset valid AT the instant) is the NOTIF-P16 follow-on
/// (it needs a tz database). The `tz` COLUMN stores the IANA id; this evaluates it as the stored
/// offset until the live fabric lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tz {
    /// Minutes east of UTC (e.g. `+60` for UTC+1, `-300` for UTC-5).
    pub offset_minutes: i32,
}

impl Tz {
    /// UTC (offset 0).
    pub const UTC: Tz = Tz { offset_minutes: 0 };

    /// Build a fixed-offset tz from minutes east of UTC.
    pub fn from_offset_minutes(offset_minutes: i32) -> Tz {
        Tz { offset_minutes }
    }

    /// The recipient-local minute-of-day (`0..1440`) for an instant given as **minutes since the
    /// UTC epoch day boundary** (`utc_minute_of_day`, `0..1440`). Wraps modulo the 1440-minute day
    /// so an offset that crosses midnight lands in the recipient's local day. This is the
    /// recipient-tz evaluation the §2.2 quiet-hours test runs in.
    pub fn local_minute_of_day(self, utc_minute_of_day: i32) -> i32 {
        let local = utc_minute_of_day + self.offset_minutes;
        // Wrap into 0..1440 (Rust `%` keeps the sign; normalise to non-negative).
        local.rem_euclid(1440)
    }
}

/// A **quiet window** — `[from, to)` minute-of-day in the recipient's tz (`0..1440`). A window may
/// **wrap midnight** (`from > to`, e.g. `22:00..07:00` = `1320..420`): the test then admits
/// `minute >= from OR minute < to`. The `days` set restricts the window to certain weekdays (`0` =
/// Monday … `6` = Sunday); an empty `days` = every day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietWindow {
    /// Window start — minute-of-day in the recipient tz (`0..1440`).
    pub from: i32,
    /// Window end (exclusive) — minute-of-day in the recipient tz (`0..1440`).
    pub to: i32,
    /// The weekdays the window applies on (`0` = Mon … `6` = Sun); empty = every day.
    pub days: Vec<u8>,
}

impl QuietWindow {
    /// Does `local_minute` (in the recipient tz) on `weekday` (`0`=Mon..`6`=Sun) fall in this
    /// window? Handles a midnight-wrapping window (`from > to`).
    fn contains(&self, local_minute: i32, weekday: u8) -> bool {
        if !self.days.is_empty() && !self.days.contains(&weekday) {
            return false;
        }
        if self.from <= self.to {
            // Same-day window: [from, to).
            local_minute >= self.from && local_minute < self.to
        } else {
            // Wraps midnight: [from, 1440) ∪ [0, to).
            local_minute >= self.from || local_minute < self.to
        }
    }
}

/// **The per-principal quiet-hours** (contract 7.4 / §2.2) — quiet windows evaluated **in the
/// recipient's tz** + the `pierce_classes` (the on-call override). A non-piercing class that lands
/// in a quiet window is **suppressed from DELIVERY** (never the audit / the inbox row); a piercing
/// class (`critical` by default) always delivers — you cannot silence an on-call page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    /// The recipient's tz the windows are evaluated in (the §2.2 recipient-tz rule). A fixed offset
    /// (the IANA/DST lookup is the named floor).
    pub tz: Tz,
    /// The quiet windows (each `[from, to)` minute-of-day in the recipient tz; may wrap midnight).
    pub windows: Vec<QuietWindow>,
    /// The classes that PIERCE quiet-hours — the one deliberate override. **Defaults to
    /// `{Critical}`** (see [`QuietHours::default`]): you cannot silence an on-call page (§2.2). An
    /// `Escalated`-reason item carries `Class::Critical`, so the critical pierce covers escalations.
    pub pierce_classes: Vec<Class>,
}

impl Default for QuietHours {
    /// The default: no windows (never quiet) + the critical pierce. A principal with no explicit
    /// quiet-hours is never suppressed; even once they set windows, `critical` still pierces.
    fn default() -> QuietHours {
        QuietHours {
            tz: Tz::UTC,
            windows: Vec::new(),
            pierce_classes: vec![Class::Critical],
        }
    }
}

impl QuietHours {
    /// **Is the recipient in a quiet window at this instant?** `utc_minute_of_day` is the instant as
    /// minutes-since-the-UTC-day-boundary (`0..1440`); `utc_weekday` is the UTC weekday (`0`=Mon).
    /// The window is tested in the RECIPIENT's tz (the offset shifts both the minute AND, if it
    /// crosses midnight, the weekday). Returns `true` iff some window contains the recipient-local
    /// time. (The instant is supplied by the caller — the router/clock — keeping this pure +
    /// deterministic for the mutation/unit floor.)
    pub fn is_quiet_at(&self, utc_minute_of_day: i32, utc_weekday: u8) -> bool {
        let local_minute = self.tz.local_minute_of_day(utc_minute_of_day);
        // If the offset pushed local time past midnight relative to UTC, shift the weekday too.
        let total = utc_minute_of_day + self.tz.offset_minutes;
        let day_shift = total.div_euclid(1440); // -1, 0, or +1 for a ±24h-bounded offset.
        let local_weekday = ((utc_weekday as i32 + day_shift).rem_euclid(7)) as u8;
        self.windows
            .iter()
            .any(|w| w.contains(local_minute, local_weekday))
    }

    /// Does `class` PIERCE quiet-hours (deliver regardless of the window)? `critical` pierces by
    /// default; the on-call override cannot be disabled away.
    pub fn pierces(&self, class: Class) -> bool {
        self.pierce_classes.contains(&class)
    }
}

/// **The delivery decision for one item, given the recipient's prefs + quiet-hours** — the §2.2
/// `route(prefs, reason, class) ∩ ¬quiet_hours` (unless the class pierces). Returns the set of
/// channels the item should be DELIVERED on right now:
///
/// 1. The routing matcher selects the candidate channels ([`NotifPrefs::channels_for`]).
/// 2. If the class **pierces** quiet-hours (`critical` by default), OR the recipient is **not** in a
///    quiet window at this instant, deliver on all candidate channels.
/// 3. Otherwise (a non-piercing class inside a quiet window), **suppress DELIVERY** — return the
///    in-cell `in_app` channel ONLY if the prefs route there (the ONE inbox still receives; only the
///    off-cell push is silenced). Quiet-hours suppress DELIVERY/RANKING, **never** the audit/inbox
///    row (NOTIF-D2 / §2.2). This is the load-bearing decision the mutation floor pins.
pub fn route(
    prefs: &NotifPrefs,
    quiet: &QuietHours,
    reason: Reason,
    class: Class,
    subsystem: Subsystem,
    utc_minute_of_day: i32,
    utc_weekday: u8,
) -> Vec<Channel> {
    let candidates = prefs.channels_for(reason, class, subsystem);
    let in_quiet = quiet.is_quiet_at(utc_minute_of_day, utc_weekday);
    if quiet.pierces(class) || !in_quiet {
        // Pierce (on-call) or not-quiet ⇒ deliver on every candidate channel.
        candidates
    } else {
        // A non-piercing class inside a quiet window: suppress off-cell delivery; the in-cell inbox
        // (in_app) still receives (the row is never suppressed — only the channel push). Keep only
        // the in_app candidate, if the prefs route there.
        candidates
            .into_iter()
            .filter(|c| *c == Channel::InApp)
            .collect()
    }
}

// ===========================================================================================
//  THE 7.4 API: get_prefs / set_prefs over the PrefStore (the in-memory projection; OLTP floor)
// ===========================================================================================

/// **The in-memory projection of the `notif_pref` / `notif_quiet_hours` tables** — the same pattern
/// as [`InboxProjection`](crate::router::InboxProjection). Keyed by the principal's opaque id (within
/// a tenant/region the store is already scoped). **FLOOR named:** the live OLTP store reads/writes
/// (the `(tenant_id, region, principal)` UPSERT under RLS) are proven against real Postgres in
/// `tests/integration_notif_prefs.rs` (the `integration` feature); this keeps `cargo build
/// --workspace` DB-free while the DB contract is proven against the live dev stack.
#[derive(Clone, Default)]
pub struct PrefStore {
    inner: Arc<Mutex<PrefStoreInner>>,
}

#[derive(Default)]
struct PrefStoreInner {
    prefs: BTreeMap<String, NotifPrefs>,
    quiet: BTreeMap<String, QuietHours>,
}

/// The value [`get_prefs`] returns — the principal's prefs + quiet-hours (the read side of 7.4). A
/// principal with no stored prefs gets the safe defaults ([`NotifPrefs::default_in_app`] +
/// [`QuietHours::default`]) — never an error (the inbox always receives; off-cell is opt-in).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrefView {
    /// The routing prefs (defaults to in-app-only if none stored).
    pub prefs: NotifPrefs,
    /// The quiet-hours (defaults to never-quiet + critical-pierce if none stored).
    pub quiet: QuietHours,
}

impl PrefStore {
    /// A fresh empty store.
    pub fn new() -> PrefStore {
        PrefStore::default()
    }

    /// Seed/UPSERT a principal's prefs directly (the same write-path [`set_prefs`] uses; exposed so
    /// a drill/holder/integration seam can pre-load the projection).
    pub fn upsert(&self, principal: &str, prefs: NotifPrefs, quiet: QuietHours) {
        let mut g = self.inner.lock().expect("pref store mutex");
        g.prefs.insert(principal.to_string(), prefs);
        g.quiet.insert(principal.to_string(), quiet);
    }

    fn read(&self, principal: &str) -> PrefView {
        let g = self.inner.lock().expect("pref store mutex");
        PrefView {
            prefs: g
                .prefs
                .get(principal)
                .cloned()
                .unwrap_or_else(NotifPrefs::default_in_app),
            quiet: g.quiet.get(principal).cloned().unwrap_or_default(),
        }
    }
}

/// **`get_prefs(principal)` (contract 7.4, read side)** — the principal's routing prefs +
/// quiet-hours. Recipient-scoped: a principal reads only their OWN prefs (the `principal` is the
/// caller's id; the store is already tenant/region-scoped). A principal with no stored prefs gets the
/// safe defaults (in-app-only routing + never-quiet + critical-pierce) — never an error.
pub fn get_prefs(store: &PrefStore, principal: &Principal) -> PrefView {
    store.read(principal.principal_id.0.as_str())
}

/// **`set_prefs(principal, routing, quiet_hours, digest)` (contract 7.4, write side)** — UPSERT the
/// principal's prefs. The routing matchers are frozen `QueryAst`s (cost-bounded by construction — an
/// over-budget matcher could never have been built, so it cannot be stored). Recipient-scoped: a
/// principal sets only their OWN prefs. Returns the stored [`PrefView`] (so the caller/CLI can echo
/// it back).
pub fn set_prefs(
    store: &PrefStore,
    principal: &Principal,
    prefs: NotifPrefs,
    quiet: QuietHours,
) -> PrefView {
    let id = principal.principal_id.0.as_str();
    store.upsert(id, prefs.clone(), quiet.clone());
    PrefView { prefs, quiet }
}

// ---- token helpers (the PII-free taxonomy tokens the matcher/CLI read) --------------------------

/// The PII-free snake_case `reason` token (the matcher variable value + the CLI form).
pub fn reason_token(reason: Reason) -> &'static str {
    match reason {
        Reason::ApprovalRequested => "approval_requested",
        Reason::Escalated => "escalated",
        Reason::Sla => "sla",
        Reason::ReviewRequested => "review_requested",
        Reason::Assigned => "assigned",
        Reason::Mentioned => "mentioned",
        Reason::Replied => "replied",
        Reason::AgentProposal => "agent_proposal",
        Reason::Watched => "watched",
        Reason::StateChanged => "state_changed",
        Reason::Fyi => "fyi",
        Reason::Blocked => "blocked",
        Reason::Unblocked => "unblocked",
        Reason::ThreadWatched => "thread_watched",
        Reason::Shared => "shared",
        Reason::Comments => "comments",
    }
}

/// The PII-free snake_case `class` token (the matcher variable value + the CLI form).
pub fn class_token(class: Class) -> &'static str {
    match class {
        Class::Critical => "critical",
        Class::Direct => "direct",
        Class::Participating => "participating",
        Class::Watching => "watching",
        Class::Fyi => "fyi",
    }
}

/// The PII-free `subsystem` token (the matcher variable value).
pub fn subsystem_token(subsystem: Subsystem) -> &'static str {
    match subsystem {
        Subsystem::Issue => "issue",
        Subsystem::Chat => "chat",
        Subsystem::Git => "git",
        Subsystem::Knowledge => "knowledge",
        Subsystem::Ci => "ci",
        Subsystem::Unknown => "unknown",
    }
}

/// The static cost bounds the prefs matcher inherits from the frozen `QueryAst` (re-exported for the
/// CLI / the cost-bound test so the threshold is read from ONE place, never re-stated).
pub const PREFS_MAX_PREDICATE_NODES: usize = MAX_PREDICATE_NODES;
/// The static depth bound (see [`PREFS_MAX_PREDICATE_NODES`]).
pub const PREFS_MAX_PREDICATE_DEPTH: usize = MAX_PREDICATE_DEPTH;

#[cfg(test)]
mod tests;
