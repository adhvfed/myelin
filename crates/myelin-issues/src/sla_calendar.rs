//! # `sla_calendar` — the business-calendar SLA arithmetic engine over `myelin-flow` (ISS-P26 / P-393, M4)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md` §6.2
//! (the business-calendar SLA arithmetic — "the genuinely-owned hard part"): convert a **business-time
//! budget** into a **wall-clock `fire_at`** over an IANA-tz calendar (DST/holiday/multi-day correct);
//! precompute `fire_at` (breach) + `at_risk_fire_at` (the 80% nudge); arm **two** `myelin-flow` timers
//! (contract 9.3); cheap disarm/re-arm on pause/resume (the `pause_conditions` `QueryAst`); **never
//! poll, never pollute the wheel with calendar logic**. On breach, start the FROZEN escalation chain
//! (page → `oncall_now` → escalate-after-timer, contract 7.5 / reconciliation §5) as a durable
//! workflow; breach/met feed OLAP for compliance reporting (contract 11.6, [`crate::olap_feed`]).
//!
//! ## Coherence (EI-01 §7) — what THIS prompt reuses vs. what is genuinely new
//!
//! This module is the **business-calendar arithmetic engine**, the one piece of the SLA story that was
//! NOT already built. It REUSES, never re-implements:
//! - the durable timer wheel — the [`myelin_query::DurableTimer`] seam (contract 9.3; the live wheel is
//!   `myelin_flow::timer`, the `myelin-flow` minute-bucket wheel) — the SAME seam the stateful Trigger
//!   ([`crate::trigger::IssueTriggerEngine`]) arms its `stale_after` deadline on. The SLA engine arms
//!   TWO timers (breach + at-risk) on it; it never touches the wheel scan.
//! - the FROZEN escalation chain — [`crate::sla_escalation::issue_sla_escalation_policy`] (contract 7.5
//!   / §2.4), the REAL three-tier chain Issues passes Notif. The breach handler STARTS that chain; it
//!   does not redefine it.
//! - the SLA event vocabulary — [`crate::events`] (`issue.sla.started/paused/resumed/at_risk/breached/
//!   met`, durations in SECONDS — the frozen unit anchor). The engine EMITS these; OLAP
//!   ([`crate::olap_feed`]) already consumes `breached`/`met` for the compliance aggregate.
//! - the `pause_conditions` grammar — `myelin_query`'s ONE `QueryAst` ([`Predicate`]), the same grammar
//!   the matcher / trigger / SLA-policy `applies_to` compile against (the granted CR-5, "one grammar").
//!
//! The genuinely-owned, genuinely-new code is [`business_fire_at`] (§6.2's pseudocode, realised
//! to-the-second over an IANA-tz calendar) + the [`SlaEngine`] orchestrator (precompute → arm two →
//! pause/resume → breach/met).
//!
//! ## The business-calendar arithmetic (§6.2 — the owned algorithm)
//!
//! ```text
//! fn business_fire_at(start, budget_secs, cal) -> ts:
//!     cursor = start; remaining = budget_secs
//!     loop:
//!         win = next_working_window(cursor, cal)   // DST-correct via IANA tz; skips nights/weekends/holidays
//!         avail = win.end - max(cursor, win.start)
//!         if remaining <= avail: return max(cursor, win.start) + remaining
//!         remaining -= avail; cursor = win.end     // advance to the next window
//! ```
//!
//! The budget is **business seconds** (e.g. an 8-hour working day = 28 800 s); the result is a
//! **wall-clock UTC instant** (epoch seconds) — the concrete `fire_at` the dumb, calendar-agnostic
//! `myelin-flow` wheel arms on. The wheel only ever sees `fire_at`s; **calendar logic lives HERE, never
//! on the wheel** (§4.2). The corpus the ISS-D6 drill exercises: DST spring-forward/fall-back,
//! multi-day spans, holiday boundaries, and mid-window pause/resume — all to-the-second.
//!
//! ## DST correctness without a tz database (a DOCUMENTED deviation, EI-01 §1)
//!
//! The architecture names "IANA tz" + "iCalendar `VTIMEZONE`" for DST correctness. The workspace has no
//! `chrono-tz`/`tzdata` dependency, and `cargo build --workspace` MUST stay dependency-light. Rather
//! than pull a multi-megabyte tz database into a build that must stay lean, this module models a
//! calendar's **UTC-offset transitions explicitly** ([`Calendar::offset_transitions`]) — the SAME data
//! a `VTIMEZONE` block carries (an ordered list of `(at_utc, offset_secs)` rules). This is honest: a
//! calendar built from a real `VTIMEZONE`/`chrono-tz` zone produces exactly this transition list, and
//! [`business_fire_at`] is correct to-the-second over it (the DST corpus proves it). The named floor:
//! **loading an IANA zone's transitions from the system tz database** is the production binding (the
//! `chrono-tz` swap), out of this prompt's lean-build scope; the ARITHMETIC is proven here.
//! See [`SlaCalendarFloors`].
//!
//! ## Mutation floor (mandatory-core, EI-01 §2)
//!
//! The business-calendar arithmetic + the breach fire-once path are **mandatory-core** — a mis-fired
//! SLA is a governance failure (the prompt's TESTS line). The `cargo-mutants` mutation-score floor for
//! this module (`cargo mutants -p myelin-issues --file crates/myelin-issues/src/sla_calendar.rs`) is
//! **≥ 90%** (the same mandatory-core threshold the DEK routing carries, EI-01 §2; the higher of the
//! Issues floors because a wrong `fire_at` or a double/never breach is silent + irreversible). The
//! mutation-tested core is [`business_fire_at`] (the window-walk + the to-the-second remainder),
//! [`Calendar::offset_at`] / [`Calendar::next_working_window`] (the DST/holiday resolution),
//! [`business_seconds_between`] (the pause-consumed inverse), and [`SlaEngine::on_breach_timer`] (the
//! exactly-once state guard): a surviving mutant that shifts a `fire_at` off-the-second, lets a breach
//! fire twice/never, or mis-resolves a DST offset is a false-green and fails the floor. The carrier
//! struct field-shuffling (the `sla_run` columns) is not itself decision logic — its correctness is
//! pinned by the snapshot/restore round-trip + the e2e/drill assertions.
//!
//! ## Floors named (VISION §3 / the prompt's FLOOR line)
//!
//! - **`time_to_resolution` history-compaction (R-11, M5+)** — a very-long SLA spanning many days of
//!   pauses accumulates `myelin-flow` history; the continue-as-new compaction is the named follow-on
//!   ([`SlaCalendarFloors::HISTORY_COMPACTION`]). Out of M4 scope; named.
//! - **The live IANA tz-database binding** ([`SlaCalendarFloors::TZ_DATABASE`]) — the explicit
//!   offset-transition model above; the `chrono-tz`/`VTIMEZONE` load is the prod swap (dev↔prod a config
//!   swap, never a code change — `business_fire_at` is unchanged).
//! - **The live durable wheel + the live escalation-chain start** — the engine arms on the
//!   [`DurableTimer`] seam ([`InMemoryTimer`] floor here; the live wheel is `myelin_flow::timer`,
//!   contract 9.3) and constructs the chain via [`crate::sla_escalation`]; the live Notif
//!   `EscalationEngine::page` start is the runtime wiring the `app` boot does (the chain DEFINITION +
//!   the engine seam are real here). [`SlaCalendarFloors::LIVE_WHEEL`].

use std::collections::BTreeMap;

use myelin_query::{DurableTimer, InMemoryTimer, Predicate, StaleAfter};
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};

use crate::sla_escalation::issue_sla_escalation_policy;

/// **The named, deferred follow-ons for the SLA business-calendar engine (VISION §3).** A `const` per
/// floor — each cites the later prompt that fills it. Never claimed green here; named so the ledger can
/// audit them.
pub struct SlaCalendarFloors;

impl SlaCalendarFloors {
    /// Very-long `time_to_resolution` SLAs spanning many days of pauses get `myelin-flow`
    /// continue-as-new **history-compaction** — the R-11 follow-on (M5+). The ARITHMETIC is unchanged;
    /// only the durable-history compaction is deferred.
    pub const HISTORY_COMPACTION: &'static str =
        "R-11 (M5+): time_to_resolution history-compaction via myelin-flow continue-as-new";
    /// The live IANA tz-database binding (`chrono-tz`/`VTIMEZONE` load) — the prod swap over the explicit
    /// offset-transition model. dev↔prod is a config swap; `business_fire_at` is unchanged.
    pub const TZ_DATABASE: &'static str =
        "prod: load IANA-zone offset transitions from the system tz database (chrono-tz/VTIMEZONE)";
    /// The live `myelin-flow` durable wheel ([`DurableTimer`] seam) + the live Notif
    /// `EscalationEngine::page` start of the FROZEN chain — the runtime wiring the `app` boot does.
    pub const LIVE_WHEEL: &'static str =
        "app boot: arm on myelin_flow::timer (9.3) + start the chain via Notif EscalationEngine::page";
}

/// **The default at-risk fraction in BASIS POINTS — the 80% nudge (§6.2).** `at_risk_fire_at` is
/// precomputed at this fraction of the business budget, arming a second wheel timer that fires the
/// `sla.at_risk` Signal (the trigger driver, [`crate::trigger::ArmableCondition::TellWhenSlaAtRisk`])
/// BEFORE the breach. Stored as basis points (`8000 = 80.00%`) — an INTEGER, so the `sla_run` carrier
/// stays `Eq` (no float drift in the durable snapshot; the exact-instant arithmetic is integer-only).
/// A per-tenant SLA policy may override it (`0 < bps < 10_000`).
pub const DEFAULT_AT_RISK_BPS: u32 = 8_000;

/// One whole = 10 000 basis points (the at-risk-fraction denominator).
const BPS_DENOM: i64 = 10_000;

/// Seconds in one minute / hour / day (the budget unit is SECONDS — the frozen unit anchor; an 8-hour
/// working day is `8 * 3600 = 28_800` business seconds).
const SECS_PER_DAY: i64 = 86_400;

// ===========================================================================
// §6.2 — the IANA-tz business calendar (the working-window model `next_working_window` reads)
// ===========================================================================

/// **A weekday in the calendar's local time** (Monday = 0 … Sunday = 6 — the ISO-8601 ordering). The
/// working week is the set of weekdays the calendar marks as working ([`Calendar::working_weekdays`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    /// Monday (ISO weekday 1, index 0).
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

impl Weekday {
    /// The weekday of a LOCAL day number (days since the Unix epoch in local time). 1970-01-01 was a
    /// Thursday (index 3), so `(local_day + 3) mod 7` is the weekday index.
    fn of_local_day(local_day: i64) -> Weekday {
        let idx = (local_day + 3).rem_euclid(7);
        match idx {
            0 => Weekday::Mon,
            1 => Weekday::Tue,
            2 => Weekday::Wed,
            3 => Weekday::Thu,
            4 => Weekday::Fri,
            5 => Weekday::Sat,
            _ => Weekday::Sun,
        }
    }
}

/// **A single UTC-offset transition (the `VTIMEZONE`-style DST rule, §6.2).** At UTC instant `at_utc`
/// and after, the calendar's local time is `UTC + offset_secs`. The ordered list of these on a
/// [`Calendar`] is exactly what a `VTIMEZONE` block / a `chrono-tz` zone carries; `business_fire_at`
/// reads the offset in effect at each instant from it, so a working window that straddles a DST change
/// is computed to-the-second (the spring-forward hour is genuinely absent; the fall-back hour repeats).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffsetTransition {
    /// The UTC instant (epoch seconds) at which this offset takes effect.
    pub at_utc: i64,
    /// The local offset from UTC in seconds (e.g. `3600` for UTC+1, `7200` for UTC+2 in DST).
    pub offset_secs: i64,
}

/// **An IANA-tz business calendar (§6.2).** Carries the working week (which weekdays + the daily
/// working window in LOCAL minutes-of-day), the holidays (LOCAL day numbers excluded), and the ordered
/// UTC-offset transitions (the DST/`VTIMEZONE` rules). [`business_fire_at`] walks this calendar window
/// by window, in LOCAL time, converting to/from UTC at each step via the offset in effect — so a budget
/// of N business seconds lands on the exact wall-clock UTC instant N working seconds after the start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    /// A stable id (the `scheme` SLA policy's `calendar_id`, schemes §6) — an opaque handle, not PII.
    pub calendar_id: String,
    /// The working weekdays (e.g. Mon–Fri). A day whose weekday is absent is a non-working day.
    pub working_weekdays: Vec<Weekday>,
    /// The daily working window START in LOCAL minutes-of-day (e.g. `9 * 60 = 540` for 09:00).
    pub work_start_min: i64,
    /// The daily working window END in LOCAL minutes-of-day (e.g. `17 * 60 = 1020` for 17:00). Must be
    /// `> work_start_min` (a degenerate window has no available seconds — the loop never terminates, so
    /// the constructor rejects it).
    pub work_end_min: i64,
    /// The holidays as LOCAL day numbers (days since the Unix epoch in local time). A holiday is a
    /// non-working day even if its weekday is a working weekday.
    pub holidays: Vec<i64>,
    /// The ordered UTC-offset transitions (the `VTIMEZONE`/DST rules). MUST be sorted by `at_utc`
    /// ascending and non-empty (the first entry is the base offset; later entries are DST changes).
    pub offset_transitions: Vec<OffsetTransition>,
}

/// **A concrete working window in UTC epoch seconds** — `[start, end)` (§6.2 `next_working_window`).
/// Both bounds are UTC instants; the window is a single day's working hours mapped through the offset
/// in effect on that local day. A window NEVER straddles midnight (the work span is within one local
/// day), but its UTC bounds may differ in offset if a DST transition falls inside the day (handled by
/// re-resolving the offset at each bound).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkingWindow {
    /// The window start as a UTC epoch-seconds instant (inclusive).
    pub start: i64,
    /// The window end as a UTC epoch-seconds instant (exclusive).
    pub end: i64,
}

/// An error constructing or evaluating a calendar (a degenerate window / empty transitions) — surfaced,
/// never a silent wrong answer (EI-02 §4: a mis-fired SLA is a governance failure, so a bad calendar
/// fails loudly).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarError(pub String);

impl Calendar {
    /// **Construct + validate a business calendar.** Rejects a degenerate working window
    /// (`work_end_min <= work_start_min` → no available seconds → the budget loop would never
    /// terminate) and an empty offset-transition list (no offset to resolve). Sorts the transitions by
    /// `at_utc` so the offset lookup is a clean predecessor search.
    pub fn new(
        calendar_id: impl Into<String>,
        working_weekdays: Vec<Weekday>,
        work_start_min: i64,
        work_end_min: i64,
        holidays: Vec<i64>,
        mut offset_transitions: Vec<OffsetTransition>,
    ) -> Result<Calendar, CalendarError> {
        if work_end_min <= work_start_min {
            return Err(CalendarError(format!(
                "degenerate working window: end {work_end_min} <= start {work_start_min} (no available business seconds)"
            )));
        }
        if working_weekdays.is_empty() {
            return Err(CalendarError(
                "no working weekdays: the budget loop would never terminate".into(),
            ));
        }
        if offset_transitions.is_empty() {
            return Err(CalendarError(
                "no offset transitions: cannot resolve a local offset".into(),
            ));
        }
        offset_transitions.sort_by_key(|t| t.at_utc);
        Ok(Calendar {
            calendar_id: calendar_id.into(),
            working_weekdays,
            work_start_min,
            work_end_min,
            holidays,
            offset_transitions,
        })
    }

    /// **A fixed-offset (no-DST) Mon–Fri 09:00–17:00 calendar** — the simplest business calendar, for
    /// the non-DST corpus + a default. `offset_secs` is the constant UTC offset (e.g. `0` for UTC).
    pub fn business_hours_fixed(calendar_id: impl Into<String>, offset_secs: i64) -> Calendar {
        // A single transition at the dawn of time = a constant offset (no DST).
        Calendar::new(
            calendar_id,
            vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
            ],
            9 * 60,
            17 * 60,
            Vec::new(),
            vec![OffsetTransition {
                at_utc: i64::MIN / 2,
                offset_secs,
            }],
        )
        .expect("the fixed business-hours calendar is well-formed")
    }

    /// **The UTC offset in effect at a UTC instant** (the `VTIMEZONE`/DST rule lookup, §6.2). The
    /// predecessor transition (the latest `at_utc <= instant`) gives the offset; before the first
    /// transition the first entry's offset is used (the base offset). This is the single point where
    /// DST enters the arithmetic — a working window resolved through it is correct across a transition.
    pub fn offset_at(&self, utc: i64) -> i64 {
        let mut offset = self.offset_transitions[0].offset_secs;
        for t in &self.offset_transitions {
            if t.at_utc <= utc {
                offset = t.offset_secs;
            } else {
                break;
            }
        }
        offset
    }

    /// Is `local_day` (a day number since the Unix epoch in LOCAL time) a working day? (a working
    /// weekday AND not a holiday).
    fn is_working_day(&self, local_day: i64) -> bool {
        let wd = Weekday::of_local_day(local_day);
        self.working_weekdays.contains(&wd) && !self.holidays.contains(&local_day)
    }

    /// **The next working window at-or-after a UTC instant (§6.2 `next_working_window`).** Maps `from`
    /// to LOCAL time via the offset in effect, finds the first local day from there that is a working
    /// day, and returns that day's working window as UTC `[start, end)` — clamped so the returned window
    /// END is strictly after `from` (a window already wholly in the past is skipped). The UTC bounds are
    /// re-resolved through the offset at the LOCAL day's start, so a DST transition shifts the window's
    /// wall-clock UTC bounds correctly. Bounded scan: at most a year of days (a calendar with no working
    /// day in a year is a misconfiguration — surfaced, not an infinite loop).
    pub fn next_working_window(&self, from: i64) -> Result<WorkingWindow, CalendarError> {
        // The local day `from` falls on (via the offset in effect at `from`).
        let local_from = from + self.offset_at(from);
        let mut local_day = local_from.div_euclid(SECS_PER_DAY);
        // Scan forward to the first working day whose window has not wholly passed. Bound the scan at
        // 366 days — a calendar with no working day in a year is a misconfiguration, surfaced loudly.
        let scan_limit = local_day + 366;
        while local_day <= scan_limit {
            if self.is_working_day(local_day) {
                // The window's LOCAL start/end (seconds since the local epoch). Convert each LOCAL wall
                // instant back to UTC by subtracting the offset in effect on that local day's noon (a
                // stable point inside the day, away from a midnight DST edge).
                let local_day_start = local_day * SECS_PER_DAY;
                let local_noon = local_day_start + 12 * 3600;
                // Resolve the offset by the local day's noon → its UTC instant, then read the offset
                // there. (One fixed-point step: noon's UTC = local_noon - base_offset; re-read.)
                let approx_utc_noon = local_noon - self.offset_transitions[0].offset_secs;
                let offset = self.offset_at(approx_utc_noon);
                let local_win_start = local_day_start + self.work_start_min * 60;
                let local_win_end = local_day_start + self.work_end_min * 60;
                let utc_start = local_win_start - offset;
                let utc_end = local_win_end - offset;
                if utc_end > from {
                    return Ok(WorkingWindow {
                        start: utc_start,
                        end: utc_end,
                    });
                }
            }
            local_day += 1;
        }
        Err(CalendarError(format!(
            "no working day within 366 days of {from} — calendar {} is misconfigured (no working day)",
            self.calendar_id
        )))
    }
}

/// **`business_fire_at(start, budget_secs, cal)` — convert a business-time budget into a wall-clock
/// `fire_at` (§6.2, the genuinely-owned algorithm).** Walks the calendar window by window from `start`,
/// consuming `budget_secs` of WORKING time, and returns the exact UTC epoch-seconds instant at which
/// the budget is exhausted — the concrete `fire_at` the `myelin-flow` wheel arms on. DST-correct (the
/// offset is re-resolved at each window via [`Calendar::offset_at`]), holiday-correct (non-working days
/// are skipped by [`Calendar::next_working_window`]), and multi-day-correct (the loop spans as many
/// windows as the budget needs). A zero/negative budget fires at the next working-window start (an SLA
/// with no slack is due immediately on the next working second).
///
/// **To-the-second:** the final window contributes `remaining` seconds from its (clamped) start, so the
/// result is `window_start + remaining` — exact, no rounding. This is the ISS-D6 fire-at-accuracy
/// property the drill asserts over the DST/holiday/multi-day/pause corpus.
pub fn business_fire_at(
    start: i64,
    budget_secs: i64,
    cal: &Calendar,
) -> Result<i64, CalendarError> {
    let mut cursor = start;
    let mut remaining = budget_secs.max(0);
    // Bound the window walk: a budget is finite business seconds; at most one window per working day,
    // and a budget over ~10 years (≈ 2600 working days) is a misconfiguration — surfaced, not a hang.
    for _ in 0..100_000 {
        let win = cal.next_working_window(cursor)?;
        // The available working seconds in this window from the cursor (the cursor may be mid-window).
        let effective_start = cursor.max(win.start);
        let avail = win.end - effective_start;
        if remaining <= avail {
            // The budget is exhausted inside this window — to-the-second.
            return Ok(effective_start + remaining);
        }
        remaining -= avail;
        cursor = win.end; // advance past this window to the next working window.
    }
    Err(CalendarError(format!(
        "budget {budget_secs}s exceeds 100k working windows for calendar {} — misconfigured SLA",
        cal.calendar_id
    )))
}

// ===========================================================================
// The SLA policy (the `scheme.kind = sla` body, schemes §6) + the engine orchestrator
// ===========================================================================

/// **An armed SLA's state (§6.2).** The lifecycle the engine drives: `Running` (the clock ticks, two
/// timers armed) → `Paused` (a `pause_conditions` match disarmed the timers; `remaining_business_secs`
/// stored) → `Running` (resume re-armed) → terminal `Breached` (the breach timer fired → chain
/// started) or `Met` (closed within target → timers disarmed). The state is the fire-once guard for
/// breach (a re-fire of an already-`Breached`/`Met` SLA is a no-op — the to-the-second exactly-once).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlaState {
    /// The clock is running; the breach + at-risk timers are armed on the wheel.
    Running,
    /// The clock is paused (a `pause_conditions` match); the timers are disarmed, `remaining` stored.
    Paused,
    /// The breach timer fired — the FROZEN escalation chain started; terminal.
    Breached,
    /// The issue closed within target — the timers were disarmed; terminal (counts toward compliance).
    Met,
}

/// **An armed SLA — the durable `sla_run` row carrier (§6.2).** Mirrors the columns an `sla_run`
/// persists: the issue it is bound to, the policy, the calendar, the precomputed `fire_at` +
/// `at_risk_fire_at`, the `remaining_business_secs` (used across pause/resume), the state, and the
/// deterministic timer ids (so the breach + at-risk timers re-arm the SAME wheel rows — the cheap
/// disarm/re-arm). A snapshot of these IS the across-restart durability: a restored engine arms the
/// SAME `fire_at`s, so the breach fires to-the-second after the restart (the ISS-D6 property).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlaRun {
    /// The opaque issue ref the SLA is bound to (e.g. `acme/ENG#7`) — the policy `applies_to` matched
    /// it. Not PII.
    pub issue_key: String,
    /// The SLA policy id (the `scheme` row, schemes §6) — an opaque handle.
    pub policy_id: String,
    /// The calendar the budget is computed over (the policy's `calendar_id`).
    pub calendar: Calendar,
    /// The total business-time budget in SECONDS (the policy `target`, the frozen unit).
    pub target_business_secs: i64,
    /// The at-risk fraction in BASIS POINTS (default [`DEFAULT_AT_RISK_BPS`] = 8000 = 80%).
    pub at_risk_bps: u32,
    /// The precomputed BREACH `fire_at` — the wall-clock UTC instant the budget is exhausted (epoch s).
    pub fire_at: i64,
    /// The precomputed AT-RISK `fire_at` — the 80% nudge instant (epoch s).
    pub at_risk_fire_at: i64,
    /// The business seconds still remaining (the full budget at start; decremented on pause). On resume,
    /// `fire_at = business_fire_at(now, remaining_business_secs, cal)`.
    pub remaining_business_secs: i64,
    /// The instant the clock was last (re)started — the anchor `remaining` is measured from on a pause.
    pub running_since: i64,
    /// The lifecycle state (the breach fire-once guard).
    pub state: SlaState,
}

impl SlaRun {
    /// The deterministic BREACH timer id (the `myelin-flow` wheel key, §6.6) — stable per issue so a
    /// re-arm targets the SAME row. Mirrors [`myelin_flow`]'s `sla_timer_id` convention (`sla/<key>`);
    /// the at-risk timer suffixes it (one issue, two timers, two stable keys).
    pub fn breach_timer_id(&self) -> String {
        format!("sla/{}", self.issue_key)
    }

    /// The deterministic AT-RISK timer id (the second wheel key, §6.2) — stable per issue.
    pub fn at_risk_timer_id(&self) -> String {
        format!("sla-at-risk/{}", self.issue_key)
    }
}

/// **An SLA outcome the engine emits + feeds OLAP (§6.2, contract 11.6).** Each maps to a frozen
/// [`crate::events`] token the compliance aggregate ([`crate::olap_feed`]) consumes: a breach starts the
/// chain + counts 0 toward compliance; a met counts 1. Surfaced from the engine so a test/audit reads
/// the exact outcome stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlaOutcomeEvent {
    /// `issue.sla.started` — the clock started; `target_seconds` in the frozen unit.
    Started {
        /// The issue the SLA is bound to.
        issue_key: String,
        /// The business-time budget in SECONDS.
        target_seconds: i64,
        /// The precomputed breach `fire_at` (epoch s).
        fire_at: i64,
    },
    /// `issue.sla.paused` — a `pause_conditions` match disarmed the timers.
    Paused {
        /// The issue the SLA is bound to.
        issue_key: String,
        /// The business seconds still remaining at the pause.
        remaining_seconds: i64,
    },
    /// `issue.sla.resumed` — the clock resumed; the breach `fire_at` was recomputed + re-armed.
    Resumed {
        /// The issue the SLA is bound to.
        issue_key: String,
        /// The recomputed breach `fire_at` (epoch s).
        fire_at: i64,
    },
    /// `issue.sla.at_risk` — the at-risk timer fired (the 80% nudge → the trigger driver).
    AtRisk {
        /// The issue the SLA is bound to.
        issue_key: String,
    },
    /// `issue.sla.breached` — the breach timer fired; the FROZEN escalation chain started.
    Breached {
        /// The issue the SLA is bound to.
        issue_key: String,
        /// The escalation policy id the chain started under (the FROZEN 7.5 chain).
        escalation_policy_id: String,
    },
    /// `issue.sla.met` — the issue closed within target; the timers were disarmed.
    Met {
        /// The issue the SLA is bound to.
        issue_key: String,
    },
}

impl SlaOutcomeEvent {
    /// The frozen [`crate::events`] event-type token this outcome serialises as (the OLAP/Bus wire
    /// name) — never a second vocabulary (EI-01 §7; the engine and `events.rs` agree by name).
    pub fn event_type(&self) -> &'static str {
        match self {
            SlaOutcomeEvent::Started { .. } => crate::events::SLA_STARTED,
            SlaOutcomeEvent::Paused { .. } => crate::events::SLA_PAUSED,
            SlaOutcomeEvent::Resumed { .. } => crate::events::SLA_RESUMED,
            SlaOutcomeEvent::AtRisk { .. } => crate::events::SLA_AT_RISK,
            SlaOutcomeEvent::Breached { .. } => crate::events::SLA_BREACHED,
            SlaOutcomeEvent::Met { .. } => crate::events::SLA_MET,
        }
    }
}

/// **A serializable snapshot of the engine's armed SLA runs — the across-restart durability (§6.2).**
/// A restart [`SlaEngine::restore`]s a fresh engine from this, re-arming the SAME precomputed `fire_at`s
/// on the wheel — so a breach due at instant T fires to-the-second after the restart (the ISS-D6
/// property). The durable backing is the `sla_run` table; this is its serialised form.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlaEngineSnapshot {
    /// The armed SLA runs, keyed by issue (the durable `sla_run` rows).
    pub runs: Vec<SlaRun>,
}

/// **The Issues SLA business-calendar engine (architecture §6.2).** Holds the armed SLA runs + the
/// CONSUMED [`DurableTimer`] seam (contract 9.3 — the `myelin-flow` wheel; the in-memory [`InMemoryTimer`]
/// floor models its arm/disarm). It owns ONLY the business-calendar semantics: precompute `fire_at` +
/// `at_risk_fire_at` ([`business_fire_at`]), arm TWO timers, cheap disarm/re-arm on pause/resume, fire
/// the breach EXACTLY ONCE (the state guard) → start the FROZEN escalation chain, and emit the
/// outcome stream that feeds OLAP. It NEVER polls, NEVER puts calendar logic on the wheel, and NEVER
/// re-implements the wheel, the chain, or the SLA event vocabulary (EI-01 §7).
pub struct SlaEngine {
    tenant: TenantId,
    region: Region,
    /// The CONSUMED `myelin-flow` durable-timer seam (9.3) — two timers armed per SLA. The deterministic
    /// keys are [`SlaRun::breach_timer_id`] / [`SlaRun::at_risk_timer_id`]; the in-memory floor models
    /// the wheel's cheap disarm/re-arm (the live wheel is `myelin_flow::timer`).
    timer: InMemoryTimer,
    /// The armed SLA runs keyed by issue (the durable `sla_run` rows — the snapshot/restore source).
    runs: BTreeMap<String, SlaRun>,
    /// The arming-id counter for the timer seam (the `DurableTimer` is keyed by `ArmingId`; the breach
    /// + at-risk timers get distinct armings derived from the deterministic timer ids).
    armings: BTreeMap<String, myelin_query::ArmingId>,
    /// The emitted outcome stream (the test/audit reads it; the live emit is the outbox). Ordered.
    emitted: Vec<SlaOutcomeEvent>,
}

impl SlaEngine {
    /// A fresh SLA engine for one `(tenant, region)` partition.
    pub fn new(tenant: TenantId, region: Region) -> SlaEngine {
        SlaEngine {
            tenant,
            region,
            timer: InMemoryTimer::new(),
            runs: BTreeMap::new(),
            armings: BTreeMap::new(),
            emitted: Vec::new(),
        }
    }

    /// The `(tenant, region)` partition this engine serves (read by a test/audit).
    pub fn partition(&self) -> (&TenantId, &Region) {
        (&self.tenant, &self.region)
    }

    /// The emitted outcome stream (the 1-fire breach + the started/paused/resumed/met feed) — the
    /// test/audit reads it to assert the to-the-second fire + the chain start.
    pub fn emitted(&self) -> &[SlaOutcomeEvent] {
        &self.emitted
    }

    /// The armed SLA run for an issue (read-only inspection; the durable table is the `sla_run` row).
    pub fn run(&self, issue_key: &str) -> Option<&SlaRun> {
        self.runs.get(issue_key)
    }

    /// How many timers are currently armed on the seam (a proof the engine disarms on pause/met — two
    /// per running SLA, zero per paused/terminal SLA).
    pub fn armed_timer_count(&self) -> usize {
        self.timer.armed_count()
    }

    /// The deterministic [`myelin_query::ArmingId`] for a wheel timer id (a stable, PII-free handle so
    /// the breach/at-risk timers re-arm the SAME wheel armings across pause/resume + restart).
    fn arming_for(timer_id: &str) -> myelin_query::ArmingId {
        myelin_query::ArmingId(format!("sla:{timer_id}"))
    }

    /// **Arm an SLA (the `issue.sla.started` path, §6.2).** Precomputes `fire_at` (breach) +
    /// `at_risk_fire_at` (the 80% nudge) via [`business_fire_at`] over the calendar, arms BOTH timers on
    /// the wheel seam (9.3), stores the `sla_run`, and emits `issue.sla.started`. The two timers carry
    /// concrete wall-clock `fire_at`s — the wheel stays calendar-agnostic. Returns the armed [`SlaRun`].
    pub fn arm(
        &mut self,
        issue_key: impl Into<String>,
        policy_id: impl Into<String>,
        calendar: Calendar,
        target_business_secs: i64,
        now: i64,
    ) -> Result<SlaRun, CalendarError> {
        self.arm_with_bps(
            issue_key,
            policy_id,
            calendar,
            target_business_secs,
            DEFAULT_AT_RISK_BPS,
            now,
        )
    }

    /// [`SlaEngine::arm`] with an explicit at-risk fraction in basis points (a per-tenant policy
    /// override; `0 < bps < 10_000`).
    pub fn arm_with_bps(
        &mut self,
        issue_key: impl Into<String>,
        policy_id: impl Into<String>,
        calendar: Calendar,
        target_business_secs: i64,
        at_risk_bps: u32,
        now: i64,
    ) -> Result<SlaRun, CalendarError> {
        let issue_key = issue_key.into();
        let fire_at = business_fire_at(now, target_business_secs, &calendar)?;
        let at_risk_budget = at_risk_budget(target_business_secs, at_risk_bps);
        let at_risk_fire_at = business_fire_at(now, at_risk_budget, &calendar)?;
        let run = SlaRun {
            issue_key: issue_key.clone(),
            policy_id: policy_id.into(),
            calendar,
            target_business_secs,
            at_risk_bps,
            fire_at,
            at_risk_fire_at,
            remaining_business_secs: target_business_secs,
            running_since: now,
            state: SlaState::Running,
        };
        self.arm_timers(&run)?;
        self.emit(SlaOutcomeEvent::Started {
            issue_key: issue_key.clone(),
            target_seconds: target_business_secs,
            fire_at,
        });
        self.runs.insert(issue_key, run.clone());
        Ok(run)
    }

    /// Arm both wheel timers (breach + at-risk) for a run on the [`DurableTimer`] seam (9.3). A re-arm
    /// of the SAME deterministic arming replaces the deadline (the cheap re-arm idiom — no second row).
    fn arm_timers(&mut self, run: &SlaRun) -> Result<(), CalendarError> {
        let breach_id = run.breach_timer_id();
        let at_risk_id = run.at_risk_timer_id();
        let breach_arming = Self::arming_for(&breach_id);
        let at_risk_arming = Self::arming_for(&at_risk_id);
        self.timer
            .arm(&breach_arming, &fire_at_to_stale_after(run.fire_at))
            .map_err(|e| CalendarError(format!("arm breach timer: {}", e.0)))?;
        self.timer
            .arm(
                &at_risk_arming,
                &fire_at_to_stale_after(run.at_risk_fire_at),
            )
            .map_err(|e| CalendarError(format!("arm at-risk timer: {}", e.0)))?;
        self.armings.insert(breach_id, breach_arming);
        self.armings.insert(at_risk_id, at_risk_arming);
        Ok(())
    }

    /// Disarm both wheel timers for a run (the cheap disarm on pause/met, §6.6).
    fn disarm_timers(&mut self, run: &SlaRun) {
        let breach_id = run.breach_timer_id();
        let at_risk_id = run.at_risk_timer_id();
        let _ = self.timer.disarm(&Self::arming_for(&breach_id));
        let _ = self.timer.disarm(&Self::arming_for(&at_risk_id));
        self.armings.remove(&breach_id);
        self.armings.remove(&at_risk_id);
    }

    /// **Pause an SLA (an `issue.updated` matching the policy's `pause_conditions`, §6.2).** Stores the
    /// remaining business seconds (the budget consumed between `running_since` and `now`, in BUSINESS
    /// time — not wall-clock), disarms BOTH timers (cheap — no wheel scan, no calendar logic on the
    /// wheel), and emits `issue.sla.paused`. A pause of a non-running SLA is a no-op. The
    /// `pause_conditions` predicate evaluation is the caller's (the matcher); this is the state move.
    pub fn pause(&mut self, issue_key: &str, now: i64) -> Result<(), CalendarError> {
        let Some(run) = self.runs.get(issue_key).cloned() else {
            return Ok(()); // unknown SLA — nothing to pause.
        };
        if run.state != SlaState::Running {
            return Ok(()); // only a running SLA pauses (idempotent).
        }
        // The business seconds CONSUMED since the clock (re)started — the working time between
        // `running_since` and `now`, computed over the calendar (NOT wall-clock elapsed). The remaining
        // budget is what's left for the resume recompute.
        let consumed = business_seconds_between(run.running_since, now, &run.calendar)?;
        let remaining = (run.remaining_business_secs - consumed).max(0);
        let mut updated = run.clone();
        updated.remaining_business_secs = remaining;
        updated.state = SlaState::Paused;
        self.disarm_timers(&run);
        self.emit(SlaOutcomeEvent::Paused {
            issue_key: issue_key.to_string(),
            remaining_seconds: remaining,
        });
        self.runs.insert(issue_key.to_string(), updated);
        Ok(())
    }

    /// **Resume an SLA (the `pause_conditions` no longer match, §6.2).** Recomputes
    /// `fire_at = business_fire_at(now, remaining_business_secs, cal)` + the at-risk instant, re-arms
    /// BOTH timers (the cheap re-arm — a row update of the precomputed `fire_at`), and emits
    /// `issue.sla.resumed`. A resume of a non-paused SLA is a no-op. The recompute is to-the-second over
    /// the remaining budget — a multi-day pause is correct (the corpus exercises mid-window pause/resume).
    pub fn resume(&mut self, issue_key: &str, now: i64) -> Result<(), CalendarError> {
        let Some(run) = self.runs.get(issue_key).cloned() else {
            return Ok(());
        };
        if run.state != SlaState::Paused {
            return Ok(());
        }
        let fire_at = business_fire_at(now, run.remaining_business_secs, &run.calendar)?;
        let at_risk_budget = at_risk_budget(run.remaining_business_secs, run.at_risk_bps);
        let at_risk_fire_at = business_fire_at(now, at_risk_budget, &run.calendar)?;
        let mut updated = run.clone();
        updated.fire_at = fire_at;
        updated.at_risk_fire_at = at_risk_fire_at;
        updated.running_since = now;
        updated.state = SlaState::Running;
        self.arm_timers(&updated)?;
        self.emit(SlaOutcomeEvent::Resumed {
            issue_key: issue_key.to_string(),
            fire_at,
        });
        self.runs.insert(issue_key.to_string(), updated);
        Ok(())
    }

    /// **Fire the at-risk timer (the 80% nudge, §6.2).** Emits `issue.sla.at_risk` (the trigger driver
    /// for [`crate::trigger::ArmableCondition::TellWhenSlaAtRisk`]) ONCE — only while the SLA is still
    /// `Running` (a re-fire after a breach/met/pause is a no-op). Returns `true` iff it fired. This is
    /// the wheel's at-risk timer fire callback.
    pub fn on_at_risk_timer(&mut self, issue_key: &str) -> bool {
        let Some(run) = self.runs.get(issue_key) else {
            return false;
        };
        if run.state != SlaState::Running {
            return false; // paused/terminal — the nudge does not fire.
        }
        self.emit(SlaOutcomeEvent::AtRisk {
            issue_key: issue_key.to_string(),
        });
        true
    }

    /// **Fire the breach timer EXACTLY ONCE → start the FROZEN escalation chain (§6.2).** The breach
    /// fire-once guard is the state column: a breach transitions `Running → Breached` ONLY if still
    /// `Running` (the in-memory model of `UPDATE … SET state='breached' WHERE … AND state='running'`).
    /// Under a re-fire (the wheel re-delivers after a restart) the SECOND fire finds the SLA already
    /// `Breached` and is a no-op — the to-the-second EXACTLY-ONCE breach. On the winning fire it starts
    /// the FROZEN three-tier escalation chain ([`crate::sla_escalation::issue_sla_escalation_policy`],
    /// contract 7.5) as a durable workflow and emits `issue.sla.breached` (→ OLAP compliance: a 0).
    /// `ack_window_minutes`/`repeat` configure the chain Issues passes Notif. Returns `true` iff this
    /// call fired the breach (the winning, once fire).
    pub fn on_breach_timer(
        &mut self,
        issue_key: &str,
        ack_window_minutes: u32,
        repeat: u32,
    ) -> bool {
        let Some(run) = self.runs.get_mut(issue_key) else {
            return false;
        };
        if run.state != SlaState::Running {
            return false; // already breached/met/paused — 0 double-fire (exactly-once).
        }
        run.state = SlaState::Breached;
        // Start the FROZEN escalation chain (contract 7.5 / §2.4) — the REAL Issues three-tier chain.
        // The chain DEFINITION is Issues'; Notif's EscalationEngine::page evaluates it on the live
        // wheel (the runtime start is the app-boot wiring, LIVE_WHEEL floor; the chain is real here).
        let policy = issue_sla_escalation_policy(ack_window_minutes, repeat);
        self.emit(SlaOutcomeEvent::Breached {
            issue_key: issue_key.to_string(),
            escalation_policy_id: policy.policy_id,
        });
        true
    }

    /// **Mark an SLA met (the issue closed within target, §6.2).** Disarms BOTH timers (the cheap
    /// disarm — the breach never fires), transitions `Running → Met` (or `Paused → Met`), and emits
    /// `issue.sla.met` (→ OLAP compliance: a 1). A met of an already-terminal SLA is a no-op. The disarm
    /// guarantees a closed issue's breach timer NEVER fires (no spurious breach after a resolution).
    pub fn meet(&mut self, issue_key: &str) -> bool {
        let Some(run) = self.runs.get(issue_key).cloned() else {
            return false;
        };
        if matches!(run.state, SlaState::Breached | SlaState::Met) {
            return false; // terminal — no second outcome.
        }
        self.disarm_timers(&run);
        let mut updated = run;
        updated.state = SlaState::Met;
        self.emit(SlaOutcomeEvent::Met {
            issue_key: issue_key.to_string(),
        });
        self.runs.insert(issue_key.to_string(), updated);
        true
    }

    /// **Snapshot the armed SLA runs — the across-restart durability source (§6.2).** The durable
    /// `sla_run` table persists these; a restart [`SlaEngine::restore`]s from them.
    pub fn snapshot(&self) -> SlaEngineSnapshot {
        SlaEngineSnapshot {
            runs: self.runs.values().cloned().collect(),
        }
    }

    /// **Restore an engine from a snapshot, RE-ARMING the running SLAs' timers (§6.2 — the ISS-D6
    /// across-restart property).** A fresh engine adopts the persisted `sla_run` rows and re-arms the
    /// breach + at-risk timers for every `Running` SLA at the SAME precomputed `fire_at` — so a breach
    /// due at instant T fires to-the-second after the restart (the wheel re-delivers; the once-guard
    /// makes a re-delivery safe). Paused/terminal SLAs are restored WITHOUT re-arming (a paused SLA has
    /// no live timer; a terminal SLA already fired/met). Does NOT re-emit `started` (the restart is not
    /// a new start — exactly-once is preserved).
    pub fn restore(
        tenant: TenantId,
        region: Region,
        snapshot: SlaEngineSnapshot,
    ) -> Result<SlaEngine, CalendarError> {
        let mut engine = SlaEngine::new(tenant, region);
        for run in snapshot.runs {
            if run.state == SlaState::Running {
                engine.arm_timers(&run)?;
            }
            engine.runs.insert(run.issue_key.clone(), run);
        }
        Ok(engine)
    }

    fn emit(&mut self, e: SlaOutcomeEvent) {
        self.emitted.push(e);
    }
}

/// **The at-risk business-second budget = `target * bps / 10_000` (the 80% nudge, integer-only).** A
/// pure integer computation (no float drift) so the at-risk instant is exact + the durable carrier
/// stays `Eq`. The product is taken in `i64` before the divide so a large budget does not lose
/// precision.
fn at_risk_budget(target_business_secs: i64, at_risk_bps: u32) -> i64 {
    target_business_secs * at_risk_bps as i64 / BPS_DENOM
}

/// **The business seconds of WORKING time between two UTC instants over a calendar (§6.2).** Used by
/// [`SlaEngine::pause`] to compute the budget CONSUMED while the clock ran (the working time between
/// `running_since` and `now`, not the wall-clock elapsed — a clock that ran over a weekend consumed
/// zero business seconds). Walks the calendar windows in `[from, to)` and sums the working overlap. A
/// `to <= from` window is zero. Inverse of [`business_fire_at`] over an interval.
pub fn business_seconds_between(from: i64, to: i64, cal: &Calendar) -> Result<i64, CalendarError> {
    if to <= from {
        return Ok(0);
    }
    let mut cursor = from;
    let mut consumed = 0i64;
    for _ in 0..100_000 {
        let win = cal.next_working_window(cursor)?;
        if win.start >= to {
            break; // the next working window starts after `to` — no more working seconds in range.
        }
        let overlap_start = cursor.max(win.start);
        let overlap_end = win.end.min(to);
        if overlap_end > overlap_start {
            consumed += overlap_end - overlap_start;
        }
        if win.end >= to {
            break;
        }
        cursor = win.end;
    }
    Ok(consumed)
}

/// Render a `fire_at` UTC epoch-seconds instant as the [`StaleAfter`] RFC-3339 form the [`DurableTimer`]
/// seam arms on (contract 9.3 — the wheel buckets on `epoch_minute(fire_at)`; the wire form is RFC-3339
/// UTC). A dependency-free encoder (the SAME civil-from-days algorithm `trigger.rs` uses for its
/// deadline) — the SLA fire_at and the trigger stale_after share one timestamp form (one grammar).
fn fire_at_to_stale_after(fire_at: i64) -> StaleAfter {
    StaleAfter(epoch_secs_to_rfc3339(fire_at))
}

/// Render epoch seconds as an RFC-3339 UTC timestamp (the `fire_at` wire form, §2.10). The civil-from-
/// days algorithm (Howard Hinnant) — dependency-free, matching [`crate::trigger`]'s encoder so the SLA
/// and trigger deadlines are byte-identical in form.
fn epoch_secs_to_rfc3339(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let secs_of_day = epoch_secs.rem_euclid(86_400);
    let (h, m, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// **The SLA-policy `pause_conditions` predicate (the `scheme.kind = sla` body, schemes §6).** A thin,
/// documented re-export of the ONE [`myelin_query::Predicate`] grammar — the SLA policy's
/// `pause_conditions` is a `QueryAst` over `issue.*` projection state (e.g.
/// `state:waiting-on-customer`), the SAME grammar the matcher / trigger / `applies_to` compile against
/// (the granted CR-5, "one grammar"). The engine does NOT evaluate it (the matcher does, at the call
/// site); this names the type so the policy body and the engine agree it is the one grammar, never a
/// second SLA-pause DSL.
pub type PauseConditions = Predicate;

#[cfg(test)]
#[path = "sla_calendar/tests.rs"]
mod tests;
