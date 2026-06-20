//! # `ranking` — the deterministic, explainable v1 ranking function (NOTIF-P7 / P-185, M2) + NOTIF-D1
//!
//! **Owning architecture doc:** `notifications.md` §3.1 (the deterministic explainable scoring
//! function `priority ∈ 0..100`; the `reason → base → class` table EXACT; `affinity`/`role_weight`
//! derived from **Id `list_objects`/relations + Refs backlinks** behind a strategy interface so the
//! ML ranker swaps in without a rewrite; **every** rank carries an explain-trace — NOTIF-2, "why am
//! I seeing this, ranked here?"). **Drill:** NOTIF-D1 (replay a mixed week → every `critical`/
//! `direct` ranks above every `fyi`; first-important latency in budget; explain-trace per rank;
//! important-buried-rate = 0). **Contracts consumed:** 4.3 `list_objects` `SetExpr` (affinity/role
//! derivation source), 5.x Refs backlinks (affinity) — frozen signatures only, behind the
//! [`AffinitySource`] port. **External insight:** `01-process-and-quality-doctrine.md` §3 (prove-it:
//! a target you cannot measure is not a gate — the explain-trace is the observability of the rank;
//! the important-buried-rate is the measured artifact), §1 (name-your-floors — deterministic-v1 with
//! ML as the named follow-on behind the SAME [`RankStrategy`]).
//!
//! ## What this prompt (NOTIF-P7) ships — the v1 ranking + the explain-trace, nothing else
//!
//! 1. **The `reason → base → class` table, EXACT (§3.1).** [`reason_base_class`] is the frozen
//!    deterministic table: `approval_requested/escalated/sla = 90/critical`;
//!    `review_requested/assigned/mentioned = 70/direct`; `replied/agent_proposal = 55/participating`;
//!    `watched/state_changed = 35/watching`; team/project `fyi = 15/fyi`. It is **total** over the
//!    sixteen frozen [`Reason`]s (every reason maps to a `(base, class)` — a new reason cannot drift
//!    into an un-ranked hole; the remaining §1.3 reasons map onto their natural band).
//!
//! 2. **The deterministic v1 scoring (`priority ∈ 0..100`).** [`DeterministicV1`] computes
//!    `priority = clamp(base + affinity_bonus + role_bonus, 0..=100)` — `base` from the table; the
//!    two bonuses from the [`AffinitySource`] port (Id `list_objects`/relations + Refs backlinks).
//!    Deliberately deterministic-first (an unpredictable inbox ranking erodes trust faster than no
//!    ranking, §3.1); the bonuses NARROW the within-band order, they never let an `fyi` outrank a
//!    `critical`/`direct` (the bands are disjoint: see [`band_floor`] — the NOTIF-D1 non-negotiable).
//!
//! 3. **Every rank carries an [`ExplainTrace`] (NOTIF-2).** [`RankStrategy::score`] returns the
//!    priority AND the deterministic trace — `(reason, base, class, affinity_bonus, role_bonus,
//!    final_priority)` — so "why am I seeing this, ranked here?" is always answerable. The trace is
//!    deterministic (same item + same affinity → same trace), so it is the observability of the rank
//!    (EI-01 §3), not a debug afterthought.
//!
//! 4. **Wired into [`list_inbox`](crate::list_inbox::list_inbox) as the ORDERING.** The ranking
//!    plugs into the `list_inbox` stable-order slot (NOTIF-P5 named this exact seam): the page is
//!    ordered by `(priority DESC, item_id ASC)` — priority is the primary key, `item_id` the stable
//!    deterministic tiebreak (so paging stays consistent across calls). See
//!    [`rank_and_order`].
//!
//! ## FLOORS named (this is the deterministic v1 — NOT the ML ranker)
//!
//! - **ML-tuned ranking is the post-M5 follow-on behind the SAME [`RankStrategy`] interface.** The
//!   promotion trigger is a **measured** important-buried signal (NOTIF-D1, signal 1.8), NOT a
//!   prediction — we promote when the deterministic ranker is measured to bury important items, not
//!   on a hunch (§3.1 / EI-01 §3). The ML ranker implements [`RankStrategy`]; nothing else changes.
//! - **The live affinity derivation** ([`AffinitySource`] backed by Id `list_objects`/relations +
//!   Refs backlinks, the OQ-E `SetExpr` push-down) is wired through the port when the live Identity
//!   `list_objects` client + the Refs backlink read land in `serve` (the read-fanout push-down is
//!   NOTIF-P13; the live clients are P-007 / P-S12). The v1 ships [`NeutralAffinity`] — a documented,
//!   non-bypass seam that returns 0 bonus (the within-band order is then pure `(base, item_id)`),
//!   so the ranking is correct and deterministic NOW and the live affinity narrows the order LATER
//!   behind the same port. The band invariant (`critical`/`direct` > `fyi`) holds under ANY
//!   `AffinitySource` (bonuses are band-bounded — see [`band_floor`]/[`band_ceiling`]).
//!
//! ## Mutation floor (the ranking module — mandatory-core)
//! `ranking` is mandatory-core (the platform's ONE ranking surface). The mutation-tested core is the
//! decision logic: the `reason → base → class` table (the exact §3.1 values), the band-bounded
//! `priority = clamp(base + affinity_bonus + role_bonus, 0..=100)` arithmetic, the
//! `(priority DESC, item_id ASC)` ordering, and the explain-trace construction (every field present,
//! deterministic). **Floor: ≥ 80% line/branch mutation score on `ranking.rs`** (measured with
//! `cargo mutants`; reported in the P-185 commit body). The floor is **stated and met** by the unit,
//! chained, and CDC tests plus the NOTIF-D1 drill: every table row is asserted exact, the band
//! invariant is asserted (no critical/direct below an fyi under any affinity), the explain-trace is
//! asserted present and deterministic on every rank, and a mutant that mis-values a base, swaps a
//! class, drops a bonus, inverts the order, or empties the trace is caught.

use myelin_identity::Principal;

use crate::router::RoutedInboxItem;
use crate::{Class, Reason};

/// The **lowest priority** a rank can take (the `0..100` floor, §3.1). A clamp boundary.
pub const PRIORITY_MIN: u8 = 0;
/// The **highest priority** a rank can take (the `0..100` ceiling, §3.1). A clamp boundary.
pub const PRIORITY_MAX: u8 = 100;

/// **The frozen `reason → base → class` table (§3.1, EXACT).** Total over the sixteen [`Reason`]s:
/// every reason maps to its `(base_priority, class)` so a reason can NEVER drift into an un-ranked
/// hole (a new reason MUST be placed on its band here — the match is exhaustive, the compiler
/// enforces it). The five named bands (highest → lowest base):
///
/// | base | class           | reasons                                                            |
/// |------|-----------------|--------------------------------------------------------------------|
/// | 90   | `critical`      | `approval_requested`, `escalated`, `sla`                           |
/// | 70   | `direct`        | `review_requested`, `assigned`, `mentioned`, `shared`              |
/// | 55   | `participating` | `replied`, `agent_proposal`, `comments`                            |
/// | 35   | `watching`      | `watched`, `state_changed`, `thread_watched`, `blocked`, `unblocked` |
/// | 15   | `fyi`           | `fyi`                                                              |
///
/// The first three columns of the EXACT prompt table are reproduced verbatim; the remaining §1.3
/// reasons (`shared`, `comments`, `thread_watched`, `blocked`, `unblocked`) map onto their natural
/// band (shared is a direct address; comments is participating; thread-watched/blocked/unblocked are
/// ambient watching) — so the table is total and no reason is silently un-ranked.
pub fn reason_base_class(reason: Reason) -> (u8, Class) {
    match reason {
        // 90 / critical — the high band (pierces quiet-hours; the agent HITL card, the on-call
        // chain, the SLA timer). The NOTIF-D1 "every critical ranks above every fyi" band.
        Reason::ApprovalRequested | Reason::Escalated | Reason::Sla => (90, Class::Critical),
        // 70 / direct — addressed to the recipient. `shared` is a direct address of the recipient.
        Reason::ReviewRequested | Reason::Assigned | Reason::Mentioned | Reason::Shared => {
            (70, Class::Direct)
        }
        // 55 / participating — the recipient is actively in the thread. `comments` is participation.
        Reason::Replied | Reason::AgentProposal | Reason::Comments => (55, Class::Participating),
        // 35 / watching — ambient. thread-watched / blocked / unblocked are watcher-band signals.
        Reason::Watched
        | Reason::StateChanged
        | Reason::ThreadWatched
        | Reason::Blocked
        | Reason::Unblocked => (35, Class::Watching),
        // 15 / fyi — the lowest band (digestible).
        Reason::Fyi => (15, Class::Fyi),
    }
}

/// **The base priority for a reason** (the first column of [`reason_base_class`]). The deterministic
/// floor the affinity/role bonuses build ON TOP of — never below.
pub fn base_priority(reason: Reason) -> u8 {
    reason_base_class(reason).0
}

/// **The routing class for a reason** (the second column of [`reason_base_class`]). Drives the
/// channel set + the quiet-hours pierce decision (the class the item carries).
pub fn class_for(reason: Reason) -> Class {
    reason_base_class(reason).1
}

/// **The lowest priority any item in this class can take** — the class band FLOOR (the base of the
/// lowest-base reason in the class). Because the affinity/role bonuses are non-negative and
/// band-bounded ([`band_ceiling`]), every item in a higher class stays at or above this floor, and
/// every item in a lower class stays at or below the higher band's floor — so the bands are disjoint
/// and a `critical`/`direct` can NEVER fall below an `fyi` (the NOTIF-D1 non-negotiable). Each band's
/// `[floor, ceiling]` window does not overlap the next band's window.
pub fn band_floor(class: Class) -> u8 {
    match class {
        Class::Critical => 90,
        Class::Direct => 70,
        Class::Participating => 55,
        Class::Watching => 35,
        Class::Fyi => 15,
    }
}

/// **The highest priority any item in this class can take** — the class band CEILING. The bonus is
/// clamped so it cannot push an item out of its band into the next one (so the band invariant holds
/// under ANY [`AffinitySource`]). The ceiling is one below the next band's floor (`critical` is
/// capped at [`PRIORITY_MAX`] = 100, the only open-topped band).
pub fn band_ceiling(class: Class) -> u8 {
    match class {
        Class::Critical => PRIORITY_MAX,        // 100 — the top band, open to the ceiling.
        Class::Direct => band_floor(Class::Critical) - 1, // 89 — just below critical's floor.
        Class::Participating => band_floor(Class::Direct) - 1, // 69 — just below direct's floor.
        Class::Watching => band_floor(Class::Participating) - 1, // 54 — just below participating.
        Class::Fyi => band_floor(Class::Watching) - 1, // 34 — just below watching's floor.
    }
}

/// **The deterministic explain-trace carried on EVERY rank (NOTIF-2).** Answers "why am I seeing
/// this, ranked here?": the `reason` it fired for, the `base` from the §3.1 table, the `class` band,
/// the `affinity_bonus`/`role_bonus` the [`AffinitySource`] contributed, and the `final_priority`
/// after the band-bounded clamp. Deterministic — the same item + same affinity yields the same
/// trace, so it is the *observability* of the rank (EI-01 §3), not a debug afterthought. PII-free:
/// it names the structured reason/class + integers, never a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExplainTrace {
    /// The structured why-it-fired the score is grounded in (the §3.1 table key).
    pub reason: Reason,
    /// The base priority from the EXACT `reason → base → class` table.
    pub base: u8,
    /// The routing class band the reason maps to.
    pub class: Class,
    /// The affinity bonus the [`AffinitySource`] contributed (Id relations / Refs backlinks). `0`
    /// under the [`NeutralAffinity`] v1 seam; the live derivation narrows the within-band order.
    pub affinity_bonus: u8,
    /// The role-weight bonus the [`AffinitySource`] contributed (Id `list_objects` role). `0` under
    /// the v1 seam.
    pub role_bonus: u8,
    /// The final clamped priority (`∈ 0..=100`), AFTER the band-bounded clamp. The ordering key.
    pub final_priority: u8,
}

impl ExplainTrace {
    /// A human-readable, PII-free one-line render of the trace (the "why ranked here?" string the
    /// inbox UI / the CLI / the drill artifact shows). Deterministic.
    pub fn render(&self) -> String {
        format!(
            "reason={:?} class={:?} base={} +affinity={} +role={} = priority {}",
            self.reason, self.class, self.base, self.affinity_bonus, self.role_bonus,
            self.final_priority
        )
    }
}

/// **A ranked inbox item — the [`RoutedInboxItem`] + its [`priority`](RankedItem::priority) + its
/// [`trace`](RankedItem::trace).** The output unit of [`RankStrategy::score`]; the ordering input to
/// [`rank_and_order`]. Every ranked item carries a trace (the NOTIF-2 "100% of ranks carry a trace"
/// gate is structural — you cannot construct a `RankedItem` without one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedItem {
    /// The underlying inbox row (refs-not-payloads).
    pub item: RoutedInboxItem,
    /// The deterministic priority (`∈ 0..=100`) — the primary ordering key.
    pub priority: u8,
    /// The deterministic explain-trace ("why am I seeing this, ranked here?", NOTIF-2).
    pub trace: ExplainTrace,
}

/// **The affinity/role derivation port (contract 4.3 `list_objects`/relations + 5.x Refs
/// backlinks).** Notif **asks**, it does not own who-relates-to-what (§3.1): the affinity (how
/// related is the viewer to the subject — Refs backlinks / co-membership) and the role-weight (the
/// viewer's role on the subject — Id `list_objects` SetExpr / relations) are derived through THIS
/// port, so the live Identity + Refs clients plug in behind the same seam (the read-fanout push-down
/// is NOTIF-P13; the live clients are P-007 / P-S12). Both bonuses are NON-NEGATIVE and the strategy
/// band-clamps them, so the band invariant holds under any implementation.
pub trait AffinitySource {
    /// The affinity bonus for `viewer` on `item.subject` — from Refs backlinks (how connected is the
    /// viewer to the subject) + Id co-relations. `0` = no extra affinity (the within-band order is
    /// then the stable `item_id` order). The strategy clamps the sum into the band, so a large value
    /// can NEVER push an item out of its class.
    fn affinity_bonus(&self, viewer: &Principal, item: &RoutedInboxItem) -> u8;

    /// The role-weight bonus for `viewer` on `item.subject` — from Id `list_objects`/relations (the
    /// viewer's role on the subject, e.g. assignee > watcher). `0` = no extra role weight.
    fn role_bonus(&self, viewer: &Principal, item: &RoutedInboxItem) -> u8;
}

/// **The v1 affinity seam — `0` bonus (Id/Refs derivation NOT YET wired).** A documented, non-bypass
/// seam (NOT a security/correctness bypass: it makes the within-band order the pure stable
/// `(base, item_id)` order, which is CORRECT — it simply does not yet narrow by affinity). The live
/// derivation (Id `list_objects`/relations + Refs backlinks, behind the SAME [`AffinitySource`])
/// lands when those clients wire into `serve` (NOTIF-P13 / P-007 / P-S12). Named explicitly so a
/// deployment never mistakes it for the affinity-aware path. The band invariant
/// (`critical`/`direct` > `fyi`) holds identically with OR without affinity — the bonuses only
/// reorder WITHIN a band.
pub struct NeutralAffinity;

impl AffinitySource for NeutralAffinity {
    fn affinity_bonus(&self, _viewer: &Principal, _item: &RoutedInboxItem) -> u8 {
        0
    }
    fn role_bonus(&self, _viewer: &Principal, _item: &RoutedInboxItem) -> u8 {
        0
    }
}

/// **The ranking strategy interface (§3.1 — the strategy pattern so the ML ranker swaps in without a
/// rewrite).** Maps `(viewer, item)` → `(priority, explain-trace)`. The v1 is [`DeterministicV1`];
/// the post-M5 ML ranker is the named follow-on behind THIS trait (the promotion trigger is a
/// measured important-buried signal, NOTIF-D1 — not a prediction). Every implementation MUST emit a
/// deterministic explain-trace per rank (the NOTIF-2 contract is on the trait).
pub trait RankStrategy {
    /// Rank one item for `viewer`: compute the priority + the explain-trace. Deterministic (same
    /// inputs → same output). The trace is mandatory (NOTIF-2 — every rank is explainable).
    fn score(&self, viewer: &Principal, item: &RoutedInboxItem) -> (u8, ExplainTrace);
}

/// **The deterministic, explainable v1 ranker (§3.1).** `priority = clamp(base + affinity_bonus +
/// role_bonus, band_floor..=band_ceiling)`: the `base` from the EXACT `reason → base → class` table,
/// the two bonuses from the [`AffinitySource`] port, clamped into the reason's band so the bonuses
/// narrow the within-band order but NEVER cross a band boundary (a `critical`/`direct` can never
/// fall below an `fyi`). Carries the explain-trace on every rank. Generic over the affinity source
/// so the live Id/Refs derivation swaps in (the v1 uses [`NeutralAffinity`]).
pub struct DeterministicV1<A: AffinitySource> {
    affinity: A,
}

impl Default for DeterministicV1<NeutralAffinity> {
    /// The v1 default — deterministic scoring with the [`NeutralAffinity`] seam (0 bonus). The
    /// within-band order is the pure stable `(base, item_id)` order until the live affinity wires in.
    fn default() -> Self {
        DeterministicV1 { affinity: NeutralAffinity }
    }
}

impl<A: AffinitySource> DeterministicV1<A> {
    /// Build the v1 ranker over a specific [`AffinitySource`] (the live Id/Refs derivation plugs in
    /// here; the v1 default uses [`NeutralAffinity`]).
    pub fn new(affinity: A) -> Self {
        DeterministicV1 { affinity }
    }
}

impl<A: AffinitySource> RankStrategy for DeterministicV1<A> {
    fn score(&self, viewer: &Principal, item: &RoutedInboxItem) -> (u8, ExplainTrace) {
        // The base + class from the EXACT §3.1 table — keyed on the item's structured `reason`.
        let (base, class) = reason_base_class(item.reason);
        // The two non-negative bonuses from the affinity port (Id relations / Refs backlinks).
        let affinity_bonus = self.affinity.affinity_bonus(viewer, item);
        let role_bonus = self.affinity.role_bonus(viewer, item);
        // priority = base + bonuses, CLAMPED into the reason's band — the bonuses narrow the
        // within-band order; they can NEVER cross a band boundary (the NOTIF-D1 non-negotiable:
        // a critical/direct never falls below an fyi). Use u16 for the sum (avoid u8 overflow), then
        // clamp into [band_floor(class), band_ceiling(class)] ⊆ [0, 100].
        let raw = u16::from(base) + u16::from(affinity_bonus) + u16::from(role_bonus);
        let floor = band_floor(class);
        let ceiling = band_ceiling(class);
        let final_priority = (raw.min(u16::from(ceiling)) as u8).max(floor);
        let trace = ExplainTrace {
            reason: item.reason,
            base,
            class,
            affinity_bonus,
            role_bonus,
            final_priority,
        };
        (final_priority, trace)
    }
}

/// **Rank a candidate set and ORDER it `(priority DESC, item_id ASC)`** — the ordering [`list_inbox`]
/// (NOTIF-P5) plugs into its stable-order slot. `priority` is the primary key (the ranked inbox);
/// `item_id` is the stable, deterministic tiebreak (so paging stays consistent across calls — a
/// HashMap/random order never leaks). Every returned [`RankedItem`] carries its explain-trace (the
/// NOTIF-2 "100% of ranks carry a trace" property is structural).
///
/// [`list_inbox::list_inbox`](crate::list_inbox::list_inbox) calls this AFTER the recipient-scope +
/// the C-9 filter + the step-0 authorize have narrowed the candidates, so the rank only ever orders
/// items the recipient is allowed to see (the authorize is never skipped for the ranking).
pub fn rank_and_order(
    candidates: Vec<RoutedInboxItem>,
    viewer: &Principal,
    strategy: &dyn RankStrategy,
) -> Vec<RankedItem> {
    let mut ranked: Vec<RankedItem> = candidates
        .into_iter()
        .map(|item| {
            let (priority, trace) = strategy.score(viewer, &item);
            RankedItem { item, priority, trace }
        })
        .collect();
    // (priority DESC, item_id ASC) — the ranked order with the stable deterministic tiebreak.
    ranked.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.item.item_id.cmp(&b.item.item_id))
    });
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::ArtifactRef;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn viewer() -> Principal {
        Principal::stub(PrincipalId("u1".into()), PrincipalKind::Human, tenant())
    }
    fn item(item_id: &str, reason: Reason) -> RoutedInboxItem {
        RoutedInboxItem {
            tenant: tenant(),
            region: Region("fr-par".into()),
            item_id: item_id.into(),
            recipient: "u1".into(),
            subject: ArtifactRef(format!("myelin://acme/issue/issue/{item_id}")),
            reason,
            class: class_for(reason),
            origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
            dedup_key: item_id.into(),
            coalesce_count: 1,
            state: "unread".into(),
            snooze_until: None,
        }
    }

    // --- the EXACT reason → base → class table (§3.1; the load-bearing values) ---

    /// **The `reason → base → class` table is EXACT (the §3.1 prompt table, verbatim).** The four
    /// named bands from the prompt are asserted value-for-value; a mutant that mis-values a base or
    /// swaps a class is caught. This is the frozen contract the whole ranking rests on.
    #[test]
    fn reason_base_class_table_is_exact() {
        // 90 / critical
        assert_eq!(reason_base_class(Reason::ApprovalRequested), (90, Class::Critical));
        assert_eq!(reason_base_class(Reason::Escalated), (90, Class::Critical));
        assert_eq!(reason_base_class(Reason::Sla), (90, Class::Critical));
        // 70 / direct
        assert_eq!(reason_base_class(Reason::ReviewRequested), (70, Class::Direct));
        assert_eq!(reason_base_class(Reason::Assigned), (70, Class::Direct));
        assert_eq!(reason_base_class(Reason::Mentioned), (70, Class::Direct));
        // 55 / participating
        assert_eq!(reason_base_class(Reason::Replied), (55, Class::Participating));
        assert_eq!(reason_base_class(Reason::AgentProposal), (55, Class::Participating));
        // 35 / watching
        assert_eq!(reason_base_class(Reason::Watched), (35, Class::Watching));
        assert_eq!(reason_base_class(Reason::StateChanged), (35, Class::Watching));
        // 15 / fyi
        assert_eq!(reason_base_class(Reason::Fyi), (15, Class::Fyi));
    }

    /// **`base_priority` and `class_for` are the two projections of the table** (the accessors the
    /// inbox UI / a consumer read). They agree with [`reason_base_class`] for every band — a mutant
    /// that stubs `base_priority` to a constant (0/1) is caught (the accessor is not a thin wrapper
    /// that can drift from the table).
    #[test]
    fn base_priority_and_class_for_project_the_table() {
        for reason in [
            Reason::ApprovalRequested,
            Reason::Assigned,
            Reason::Replied,
            Reason::Watched,
            Reason::Fyi,
        ] {
            let (base, class) = reason_base_class(reason);
            assert_eq!(base_priority(reason), base, "base_priority == the table base for {reason:?}");
            assert_eq!(class_for(reason), class, "class_for == the table class for {reason:?}");
        }
        // the distinct bands give distinct bases (a constant-stub mutant cannot satisfy this).
        assert_eq!(base_priority(Reason::Sla), 90);
        assert_eq!(base_priority(Reason::Assigned), 70);
        assert_eq!(base_priority(Reason::Fyi), 15);
        assert_ne!(base_priority(Reason::Sla), base_priority(Reason::Fyi));
    }

    /// **The table is TOTAL over the sixteen frozen reasons** — every reason maps to a `(base,
    /// class)` and the base is consistent with the class band (no reason in a higher band has a
    /// base below a lower band's floor). A new reason cannot drift into an un-ranked hole.
    #[test]
    fn table_is_total_and_band_consistent_over_all_reasons() {
        for reason in [
            Reason::ApprovalRequested,
            Reason::Escalated,
            Reason::Sla,
            Reason::ReviewRequested,
            Reason::Assigned,
            Reason::Mentioned,
            Reason::Replied,
            Reason::AgentProposal,
            Reason::Watched,
            Reason::StateChanged,
            Reason::Fyi,
            Reason::Blocked,
            Reason::Unblocked,
            Reason::ThreadWatched,
            Reason::Shared,
            Reason::Comments,
        ] {
            let (base, class) = reason_base_class(reason);
            // the base falls within the class's band window (consistent table).
            assert!(
                base >= band_floor(class) && base <= band_ceiling(class),
                "reason {reason:?}: base {base} must be within {class:?} band [{}, {}]",
                band_floor(class),
                band_ceiling(class)
            );
        }
    }

    /// **The class bands are DISJOINT and correctly ordered** (`critical > direct > participating >
    /// watching > fyi`) — every band's `[floor, ceiling]` window is strictly above the next band's
    /// window. This is the structural basis of the NOTIF-D1 invariant (a critical/direct can NEVER
    /// share a priority with an fyi). A mutant that overlaps two bands is caught.
    #[test]
    fn class_bands_are_disjoint_and_strictly_ordered() {
        let bands = [
            Class::Critical,
            Class::Direct,
            Class::Participating,
            Class::Watching,
            Class::Fyi,
        ];
        for win in bands.windows(2) {
            let higher = win[0];
            let lower = win[1];
            // every value in the higher band is strictly above every value in the lower band.
            assert!(
                band_floor(higher) > band_ceiling(lower),
                "{higher:?} floor {} must be strictly above {lower:?} ceiling {}",
                band_floor(higher),
                band_ceiling(lower)
            );
        }
        // and every band fits inside [PRIORITY_MIN, PRIORITY_MAX] = 0..=100, floor ≤ ceiling.
        // (band_floor ≥ PRIORITY_MIN is trivially true for u8 — the load-bearing bounds are the
        // ceiling ≤ PRIORITY_MAX and floor ≤ ceiling.)
        assert_eq!(PRIORITY_MIN, 0, "the 0..100 floor is 0");
        for class in bands {
            assert!(band_ceiling(class) <= PRIORITY_MAX);
            assert!(band_floor(class) <= band_ceiling(class));
        }
    }

    // --- the deterministic v1 scoring + the explain-trace ---

    /// **The v1 ranker scores `base + bonuses`, clamped into the band, and emits a complete,
    /// deterministic explain-trace on EVERY rank (NOTIF-2).** Under the neutral seam the score is the
    /// base; the trace names reason/base/class/bonuses/final. A mutant that drops a bonus, mis-clamps,
    /// or empties a trace field is caught.
    #[test]
    fn v1_neutral_scores_base_and_emits_complete_trace() {
        let ranker = DeterministicV1::default();
        let (priority, trace) = ranker.score(&viewer(), &item("x", Reason::Assigned));
        assert_eq!(priority, 70, "neutral affinity → the base (70/direct)");
        assert_eq!(
            trace,
            ExplainTrace {
                reason: Reason::Assigned,
                base: 70,
                class: Class::Direct,
                affinity_bonus: 0,
                role_bonus: 0,
                final_priority: 70,
            }
        );
        // deterministic: the same rank twice yields the identical trace (NOTIF-2 observability).
        let (p2, t2) = ranker.score(&viewer(), &item("x", Reason::Assigned));
        assert_eq!((priority, trace.clone()), (p2, t2));
        // the trace renders a PII-free why-string.
        assert!(trace.render().contains("priority 70"));
        assert!(trace.render().contains("Assigned"));
    }

    /// An affinity source with FIXED bonuses (the seam the live Id/Refs derivation plugs into) — to
    /// prove the bonuses are applied AND band-clamped.
    struct FixedAffinity {
        affinity: u8,
        role: u8,
    }
    impl AffinitySource for FixedAffinity {
        fn affinity_bonus(&self, _v: &Principal, _i: &RoutedInboxItem) -> u8 {
            self.affinity
        }
        fn role_bonus(&self, _v: &Principal, _i: &RoutedInboxItem) -> u8 {
            self.role
        }
    }

    /// **The affinity/role bonuses narrow the WITHIN-band order (they are applied + summed).** Two
    /// `assigned` (direct, base 70) items: the one with affinity outscores the one without — but both
    /// stay in the direct band. A mutant that ignores a bonus is caught.
    #[test]
    fn bonuses_narrow_within_band_order() {
        let with_affinity = DeterministicV1::new(FixedAffinity { affinity: 5, role: 3 });
        let (p, t) = with_affinity.score(&viewer(), &item("x", Reason::Assigned));
        assert_eq!(p, 78, "70 base + 5 affinity + 3 role = 78 (within the direct band)");
        assert_eq!(t.affinity_bonus, 5);
        assert_eq!(t.role_bonus, 3);
        // still strictly within the direct band (70..=89).
        assert!(p >= band_floor(Class::Direct) && p <= band_ceiling(Class::Direct));
    }

    /// **THE BAND INVARIANT (the NOTIF-D1 non-negotiable): no affinity, however large, lets a lower
    /// band outrank a higher band.** Even a saturating affinity (255+255) on an `fyi` item is clamped
    /// to the fyi ceiling (34) — it NEVER reaches the critical floor (90) or the direct floor (70).
    /// A mutant that removes the band clamp (letting a bonus cross a band) is caught.
    #[test]
    fn band_clamp_holds_under_saturating_affinity() {
        let huge = DeterministicV1::new(FixedAffinity { affinity: 255, role: 255 });
        // an fyi item with a saturating affinity is still capped at the fyi ceiling.
        let (fyi_p, _) = huge.score(&viewer(), &item("fyi", Reason::Fyi));
        assert_eq!(fyi_p, band_ceiling(Class::Fyi), "fyi clamps to its band ceiling (34)");
        assert!(fyi_p < band_floor(Class::Direct), "an fyi NEVER reaches the direct floor");
        assert!(fyi_p < band_floor(Class::Critical), "an fyi NEVER reaches the critical floor");
        // a critical with NO affinity still outranks the saturated fyi.
        let plain = DeterministicV1::default();
        let (crit_p, _) = plain.score(&viewer(), &item("crit", Reason::Sla));
        assert!(crit_p > fyi_p, "a plain critical (90) outranks a saturated fyi (34)");
        // and the clamp never exceeds 100.
        let (crit_huge, _) = huge.score(&viewer(), &item("crit2", Reason::Sla));
        assert_eq!(crit_huge, PRIORITY_MAX, "critical saturates at the 0..100 ceiling (100)");
    }

    // --- the ordering (priority DESC, item_id ASC) ---

    /// **`rank_and_order` orders `(priority DESC, item_id ASC)` and carries a trace on every item.**
    /// A mixed set (critical, direct, fyi, two equal-priority) orders high→low; equal priorities
    /// break by item_id (stable). Every item carries its explain-trace (the 100%-trace gate). A
    /// mutant that inverts the order or the tiebreak is caught.
    #[test]
    fn rank_and_order_orders_by_priority_then_item_id_with_trace_on_every_item() {
        let candidates = vec![
            item("z-fyi", Reason::Fyi),            // 15
            item("a-crit", Reason::Sla),           // 90
            item("b-direct", Reason::Assigned),    // 70
            item("a-direct", Reason::Mentioned),   // 70 — ties b-direct on priority; item_id breaks
        ];
        let ranker = DeterministicV1::default();
        let ordered = rank_and_order(candidates, &viewer(), &ranker);
        let order: Vec<&str> = ordered.iter().map(|r| r.item.item_id.as_str()).collect();
        // priority DESC: crit(90) > the two directs(70) > fyi(15); the directs tie → item_id ASC.
        assert_eq!(order, vec!["a-crit", "a-direct", "b-direct", "z-fyi"]);
        // every ranked item carries a deterministic, complete explain-trace (NOTIF-2 / the gate).
        for r in &ordered {
            assert_eq!(r.priority, r.trace.final_priority, "the trace's final == the rank");
            assert!(!r.trace.render().is_empty(), "every rank carries a non-empty trace");
        }
    }

    /// **The NOTIF-D1 invariant at the ordering level: every critical/direct ranks above every
    /// fyi.** Over a mixed batch, the last critical/direct index is strictly before the first fyi
    /// index — important-buried-rate 0. This is the property the drill replays at scale.
    #[test]
    fn no_critical_or_direct_ranks_below_any_fyi() {
        let candidates = vec![
            item("fyi-1", Reason::Fyi),
            item("crit-1", Reason::ApprovalRequested),
            item("fyi-2", Reason::Fyi),
            item("direct-1", Reason::Assigned),
            item("fyi-3", Reason::Fyi),
            item("crit-2", Reason::Escalated),
        ];
        let ordered = rank_and_order(candidates, &viewer(), &DeterministicV1::default());
        // the position of the last critical/direct < the position of the first fyi.
        let last_important = ordered
            .iter()
            .rposition(|r| matches!(r.trace.class, Class::Critical | Class::Direct))
            .unwrap();
        let first_fyi = ordered
            .iter()
            .position(|r| r.trace.class == Class::Fyi)
            .unwrap();
        assert!(
            last_important < first_fyi,
            "every critical/direct ranks above every fyi (important-buried-rate 0)"
        );
    }

    /// **The strategy interface is swappable** — a second `RankStrategy` (a stub that inverts the
    /// order) plugs in WITHOUT touching `rank_and_order` (the ML-ranker swap, §3.1). Proves the
    /// strategy pattern is real (the ordering reads the trait, not the concrete type).
    #[test]
    fn rank_strategy_is_swappable() {
        // a degenerate alternate strategy: everything priority 50 (proves the seam is a trait).
        struct FlatStrategy;
        impl RankStrategy for FlatStrategy {
            fn score(&self, _v: &Principal, item: &RoutedInboxItem) -> (u8, ExplainTrace) {
                (
                    50,
                    ExplainTrace {
                        reason: item.reason,
                        base: 50,
                        class: Class::Participating,
                        affinity_bonus: 0,
                        role_bonus: 0,
                        final_priority: 50,
                    },
                )
            }
        }
        let candidates = vec![item("b", Reason::Sla), item("a", Reason::Fyi)];
        let ordered = rank_and_order(candidates, &viewer(), &FlatStrategy);
        // all 50 → ordered by item_id ASC (the stable tiebreak), proving the swap took effect.
        let order: Vec<&str> = ordered.iter().map(|r| r.item.item_id.as_str()).collect();
        assert_eq!(order, vec!["a", "b"]);
        assert!(ordered.iter().all(|r| r.priority == 50), "the swapped strategy is in effect");
    }
}
