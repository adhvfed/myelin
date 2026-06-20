//! # The DSR deadline durable timer + the nearing-deadline warning Signal (P-GA-21 → P-148)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§4.1 step 6** — *"Track
//! the deadline on a **durable timer** (contract 9.3, the same `myelin-flow` timer wheel as SLA
//! timers and Trigger `stale_after` — we do not reinvent durable timers); a nearing-deadline emits
//! a warning `Signal`."* The deadline base + the Art. 12(3) extension are §4 (`deadline = now + 1
//! month`; *extendable to 3 for complex (recorded reason)*).
//!
//! **Contract-index:** OWNS the **deadline-timer leg of row 10.4** (completing the P-GA-14 coarse-
//! deadline floor — `dsr_deadline_margin` is the §1.8 telemetry signal the warning fires on).
//! CONSUMES row **9.3** (the durable timer wheel — `sleep_until` on the minute-bucket index) and
//! rows **9.2/9.4** (durable activity + signal — the warning IS a durable Signal).
//!
//! ## What THIS prompt (P-GA-21) ships — replacing the P-GA-14 coarse-deadline floor
//! The DSR orchestrator (P-GA-11, [`crate::dsr`]) set the deadline as a COARSE tracked timestamp
//! (`submitted_at + 1 month`) with no firing. This module replaces that floor with a **durable
//! timer**:
//! 1. **`arm_deadline`** — on `dsr_submit`, arm a [`DsrTimerWheel`] entry at the nearing-deadline
//!    point (`deadline − warning_margin`) keyed by [`crate::dsr::DsrId`]. The fire point is the
//!    WARNING point (not the deadline itself) — firing it emits the warning Signal so the deadline
//!    is never *reached* silently.
//! 2. **The minute-bucket wheel** ([`DsrTimerWheel`]) — `sleep_until(fire_at)` lands the entry in
//!    the `fire_at / 60` minute bucket (the §9.3 index shape); [`DsrTimerWheel::tick`] fires every
//!    entry whose minute bucket is `≤ now`. This is the SAME wheel shape Triggers' `stale_after`
//!    rides (`myelin_query::triggers::DurableTimer`) — one primitive, three uses (EI-01 §7).
//! 3. **The warning Signal** ([`DsrDeadlineWarning`]) — firing a due entry yields a PII-free
//!    warning carrying the [`crate::dsr::DsrId`], the tenant token, and the seconds-of-margin
//!    remaining (`dsr_deadline_margin`). The orchestrator (or the dispatch tier) routes it; on
//!    THIS floor [`DsrTimerWheel::tick`] RETURNS the fired warnings (the publish onto the bus is
//!    the same outbox-only emit every Signal rides — a named floor).
//! 4. **Restart-survival** — the wheel state IS the durable state (the §9.3 `wf_timer`-style rows),
//!    NOT in-process state. [`DsrTimerWheel::snapshot`] / [`DsrTimerWheel::restore`] model the
//!    crash-and-restart: an orchestrator killed BETWEEN arm and fire restores the wheel and the
//!    timer STILL fires (0 silent misses — the GA-D4 restart leg).
//! 5. **The extension re-arm** — [`DsrTimerWheel::rearm_extension`] disarms the old entry and arms
//!    a new one at the 3-month extension point with a RECORDED reason (Art. 12(3) — *extendable to
//!    3 for complex (recorded reason)*). Cheap disarm/re-arm of a precomputed `fire_at` (the §4.6
//!    idiom, never wheel pollution).
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The REAL `myelin-flow` minute-bucket wheel engine** is the named floor **P-FLOW-13 →
//!   P-207** (exactly the floor `myelin_query::triggers::DurableTimer` already names). This module
//!   is UPSTREAM of `myelin-flow` in the crate DAG (gdpr-service must not depend on the workflow
//!   service), so it carries its OWN deterministic, in-memory model of the §9.3 wheel with
//!   byte-for-byte the minute-bucket / sleep_until / tick semantics. When `myelin-flow` lands, the
//!   DSR deadline arms through ITS `sleep_until` (the seam shape — `arm(fire_at)` + the minute
//!   bucket — does NOT change; it is a config swap, not a code change). The wheel is deterministic
//!   (a fake [`myelin_substrate::Clock`] advances it across the warning + deadline boundaries), so
//!   the SAME submit/tick sequence always fires the same warnings.
//! - **The publish of [`DsrDeadlineWarning`] onto the bus** is the same outbox-only emit every
//!   Signal rides (EI-01 §5 — written via the outbox only); [`DsrTimerWheel::tick`] RETURNS the
//!   fired warnings on this floor (the caller hands them to the outbox). The wire onto the running
//!   service's emit path lands when the service boots from `serve(AppSpec)` (P-119 rides the same
//!   surface).
//! - **The durable Postgres `dsr_timer` table** (the §9.3 `wf_timer`-style rows, `(minute_bucket,
//!   dsr_id)`-keyed) is the same DB floor every M0 in-memory store carries (P-007 / P-S12). On this
//!   floor the wheel is an in-memory [`DsrTimerWheel`] with byte-for-byte the §9.3 semantics +
//!   explicit [`DsrTimerWheel::snapshot`]/[`DsrTimerWheel::restore`] modelling the durable-row
//!   crash-survival the table gives for free.
//!
//! ## Mutation floor (P-GA-21 TESTS — the arm/fire/re-arm timer path is mandatory-core).
//! `cargo mutants -p myelin-gdpr-service -f crates/myelin-gdpr-service/src/dsr_timer.rs`
//! (2026-06-20): **42 mutants, 37 caught, 4 unviable, 1 missed.** Every BEHAVIORAL mutant on the
//! mandatory-core paths is CAUGHT — [`DsrTimerWheel::arm`] (the bucket placement), [`TimerEntry::
//! minute_bucket`] (`fire_at / 60`), [`DsrTimerWheel::tick`] (the fire-when-due comparison — `≤`
//! not `<`/`>`, the off-by-one that would silently miss; the `saturating_sub` margin), the
//! warning-margin computation in [`DsrDeadlineTimer::arm_deadline`] / [`DsrDeadlineTimer::
//! extend_deadline`] (`−` not `+`, the `+ deadline_secs` base, the `.max(submitted_at)` floor), the
//! restart-survival ([`DsrTimerWheel::restore`] re-populating the buckets), and
//! [`DsrTimerWheel::rearm_extension`] (the `<=` extension-must-extend guard + the disarm-then-arm
//! with a recorded reason). The 1 residual is the documented non-core cosmetic class:
//! `<TimerError as Display>::fmt -> Ok(Default::default())` — the human-readable error MESSAGE text
//! (the error *variants* are mutation-killed: every `unwrap_err()` asserts the typed [`TimerError`]
//! by `PartialEq`); only the rendered string body is unkilled, which is cosmetic, not behavior
//! (exactly the [`crate::dsr`] `DsrError::Display` residual class). Stated, not hidden (EI-01 §3).

use std::collections::BTreeMap;

use myelin_substrate::{Clock, DsrDeadline};
use myelin_tenancy::TenantId;

use crate::dsr::DsrId;

/// The `dsr_deadline_margin` telemetry signal NAME + UNIT (gdpr §1.8 / the GA-D4 GATE — the
/// nearing-deadline warning fires on this signal). PII-free: the value is a seconds-of-margin
/// duration, never a subject. This is the signal the warning Signal carries (the operator sees
/// "DSR `dsr:7` has N seconds of margin left").
pub const DSR_DEADLINE_MARGIN: (&str, &str) = ("gdpr.dsr_deadline_margin", "secs");

// ───────────────────────── the warning Signal (contract 9.4 — the durable Signal) ─────────────

/// **The nearing-deadline warning `Signal` (gdpr §4.1 step 6 / contract 9.4).** Fired by the
/// durable timer `warning_margin` BEFORE the statutory deadline so the deadline is NEVER silently
/// reached. PII-free by construction: it carries ONLY the opaque [`DsrId`], the tenant token, and
/// the seconds-of-margin remaining (`dsr_deadline_margin`) — never a subject name/email (the
/// subject lives behind the [`DsrId`] in the private register; this Signal is safe to route /
/// publish / display).
///
/// On THIS floor the warning is RETURNED by [`DsrTimerWheel::tick`] (the caller hands it to the
/// outbox — the one sanctioned emit path, EI-01 §5); the wire onto the running service's emit path
/// is a named `serve(AppSpec)`-boot floor.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DsrDeadlineWarning {
    /// The DSR this warning is about (the opaque id — the caller resolves the subject behind it).
    pub dsr_id: DsrId,
    /// The tenant the DSR runs under (the PII-free partition token).
    pub tenant: TenantId,
    /// The statutory deadline (`submitted_at + deadline_secs`), in seconds — the point the warning
    /// is racing. The certificate must seal before this.
    pub deadline_secs: u64,
    /// The seconds of margin remaining when the warning fired (`deadline − now`). The
    /// `dsr_deadline_margin` value (§1.8). Positive by construction: the warning fires at the
    /// nearing-deadline point, which is BEFORE the deadline.
    pub margin_remaining_secs: u64,
}

// ───────────────────────── a wheel entry (the durable §9.3 row model) ─────────────────────────

/// One armed deadline-timer entry — the in-memory model of the durable `dsr_timer` row (the §9.3
/// `wf_timer`-style shape). Carries the DSR it is for, the tenant, the WARNING fire point (when the
/// warning Signal fires), the statutory deadline (carried into the warning), and the recorded
/// extension reason (set when the timer is re-armed for a complex request — Art. 12(3)).
#[derive(Clone, Debug, PartialEq, Eq)]
struct TimerEntry {
    dsr_id: DsrId,
    tenant: TenantId,
    /// The wall-clock second the WARNING Signal fires (the nearing-deadline point =
    /// `deadline − warning_margin`). The wheel buckets on `fire_at / 60`.
    fire_at_secs: u64,
    /// The statutory deadline the warning is racing (`submitted_at + deadline_secs`).
    deadline_secs: u64,
    /// The recorded extension reason, set when the timer was re-armed for a complex request
    /// (Art. 12(3) — *extendable to 3 for complex (recorded reason)*). `None` for a normal arming.
    extension_reason: Option<String>,
}

impl TimerEntry {
    /// The minute bucket this entry lands in (`fire_at / 60` — the §9.3 minute-bucket index). The
    /// wheel fires a bucket once `now`'s bucket reaches it.
    fn minute_bucket(&self) -> u64 {
        self.fire_at_secs / 60
    }
}

/// A serialisable snapshot of one timer entry (the durable `dsr_timer` row form — restart-survival
/// is modelled by snapshotting + restoring these). PII-free (an opaque id + a tenant token + two
/// durations + an optional recorded reason).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerEntrySnapshot {
    /// The DSR the entry is for.
    pub dsr_id: DsrId,
    /// The tenant token.
    pub tenant: TenantId,
    /// The warning fire point (seconds).
    pub fire_at_secs: u64,
    /// The statutory deadline (seconds).
    pub deadline_secs: u64,
    /// The recorded extension reason (Art. 12(3)), if the timer was re-armed for a complex request.
    pub extension_reason: Option<String>,
}

// ───────────────────────── typed errors (loud, never swallowed) ─────────────────────────

/// A durable-timer error (EI-01 §3 — make violations loud). A double-arm or a re-arm of an
/// un-armed DSR is a programming error, never a silent no-op (it would mask a missed deadline).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimerError {
    /// `arm` was called for a DSR that already has an armed entry (a double-submit / re-arm bug).
    /// Re-arming an EXISTING timer is [`DsrTimerWheel::rearm_extension`], not a second `arm`.
    AlreadyArmed(DsrId),
    /// `rearm_extension` / `disarm` was called for a DSR with no armed entry (a stale id / a
    /// re-arm of an already-fired timer). Loud — never a silent miss.
    NotArmed(DsrId),
    /// `rearm_extension` was called with a new deadline that is NOT later than the current one (an
    /// extension must EXTEND — Art. 12(3) extends to 3 months, never shortens). Carries
    /// (current_deadline, requested_deadline).
    ExtensionNotLater { current_secs: u64, requested_secs: u64 },
}

impl std::fmt::Display for TimerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimerError::AlreadyArmed(id) => write!(
                f,
                "DSR `{}` already has an armed deadline timer (a re-arm is `rearm_extension`, not a \
                 second `arm`)",
                id.0
            ),
            TimerError::NotArmed(id) => {
                write!(f, "DSR `{}` has no armed deadline timer (a stale id / an already-fired timer)", id.0)
            }
            TimerError::ExtensionNotLater { current_secs, requested_secs } => write!(
                f,
                "extension deadline {requested_secs}s is not later than the current {current_secs}s \
                 (Art. 12(3) extends the deadline, never shortens it)"
            ),
        }
    }
}

impl std::error::Error for TimerError {}

// ───────────────────────── the minute-bucket durable timer wheel (contract 9.3) ───────────────

/// **The DSR deadline durable timer wheel (the in-memory model of the §9.3 `myelin-flow`
/// minute-bucket wheel).** Holds the armed [`TimerEntry`] rows keyed by [`DsrId`]; `arm`'s a
/// `sleep_until(fire_at)` into the `fire_at / 60` minute bucket; [`Self::tick`] fires every entry
/// whose minute bucket is `≤ now`'s minute bucket, yielding the warning Signals.
///
/// **The wheel state IS the durable state** (the §9.3 rows), NOT in-process state — [`Self::snapshot`]
/// / [`Self::restore`] model the crash-and-restart (an orchestrator killed between arm and fire
/// restores the wheel and the timer STILL fires; 0 silent misses — the GA-D4 restart leg). The real
/// `myelin-flow` wheel (the named floor P-FLOW-13 / P-207) gives this for free via its durable
/// `wf_timer` table; on this floor the snapshot/restore models that table.
#[derive(Default)]
pub struct DsrTimerWheel {
    /// The armed entries, keyed by DSR id (so a DSR has at most one armed deadline timer — a re-arm
    /// is an explicit [`Self::rearm_extension`], never a silent second entry).
    armed: BTreeMap<DsrId, TimerEntry>,
}

impl DsrTimerWheel {
    /// An empty wheel.
    pub fn new() -> DsrTimerWheel {
        DsrTimerWheel { armed: BTreeMap::new() }
    }

    /// **`sleep_until` (contract 9.3) — arm a deadline-warning entry.** Lands the entry in the
    /// `fire_at / 60` minute bucket. `fire_at_secs` is the WARNING point (the nearing-deadline
    /// point = `deadline − warning_margin`); `deadline_secs` is the statutory deadline carried into
    /// the warning. A DSR may have at most ONE armed entry — a second `arm` is a loud
    /// [`TimerError::AlreadyArmed`] (a re-arm is [`Self::rearm_extension`]).
    fn arm(
        &mut self,
        dsr_id: DsrId,
        tenant: TenantId,
        fire_at_secs: u64,
        deadline_secs: u64,
        extension_reason: Option<String>,
    ) -> Result<(), TimerError> {
        if self.armed.contains_key(&dsr_id) {
            return Err(TimerError::AlreadyArmed(dsr_id));
        }
        self.armed.insert(
            dsr_id.clone(),
            TimerEntry { dsr_id, tenant, fire_at_secs, deadline_secs, extension_reason },
        );
        Ok(())
    }

    /// **`tick(now)` — fire every armed entry whose minute bucket is due (`≤ now`'s bucket).** The
    /// §9.3 wheel fire step: an entry whose `fire_at / 60` minute bucket has been reached fires its
    /// warning Signal and is removed from the wheel (fire-once). Returns the fired warnings sorted
    /// by [`DsrId`] (deterministic). The `margin_remaining_secs` is `deadline − now` (positive: the
    /// warning fires BEFORE the deadline).
    ///
    /// The fire test is `entry.minute_bucket() ≤ now / 60` — an entry armed for THIS minute fires
    /// THIS tick (the boundary is inclusive; `<` would silently delay a fire a full minute, the
    /// off-by-one the mutation floor guards).
    pub fn tick(&mut self, now_secs: u64) -> Vec<DsrDeadlineWarning> {
        let now_bucket = now_secs / 60;
        let due: Vec<DsrId> = self
            .armed
            .values()
            .filter(|e| e.minute_bucket() <= now_bucket)
            .map(|e| e.dsr_id.clone())
            .collect();
        let mut fired = Vec::with_capacity(due.len());
        for id in due {
            let e = self.armed.remove(&id).expect("due entry was just observed present");
            // the margin is deadline − now, floored at 0 (the warning fires before the deadline, so
            // this is positive in the happy path; a tick that runs PAST the deadline still reports a
            // 0 margin rather than underflowing).
            let margin_remaining_secs = e.deadline_secs.saturating_sub(now_secs);
            fired.push(DsrDeadlineWarning {
                dsr_id: e.dsr_id,
                tenant: e.tenant,
                deadline_secs: e.deadline_secs,
                margin_remaining_secs,
            });
        }
        fired.sort();
        fired
    }

    /// **`rearm_extension` (Art. 12(3) — extend to 3 months for a complex request, recorded
    /// reason).** Disarms the DSR's current entry and arms a NEW one at the extension warning point
    /// with the RECORDED reason. The new deadline MUST be later than the current (an extension
    /// extends — [`TimerError::ExtensionNotLater`] otherwise). Errors if the DSR has no armed entry
    /// ([`TimerError::NotArmed`]). This is the cheap disarm/re-arm of a precomputed `fire_at` (§4.6
    /// — never wheel pollution).
    pub fn rearm_extension(
        &mut self,
        dsr_id: &DsrId,
        new_fire_at_secs: u64,
        new_deadline_secs: u64,
        reason: String,
    ) -> Result<(), TimerError> {
        let current = self.armed.get(dsr_id).ok_or_else(|| TimerError::NotArmed(dsr_id.clone()))?;
        if new_deadline_secs <= current.deadline_secs {
            return Err(TimerError::ExtensionNotLater {
                current_secs: current.deadline_secs,
                requested_secs: new_deadline_secs,
            });
        }
        let tenant = current.tenant.clone();
        // disarm the old entry, then arm the new one (the §4.6 cheap disarm/re-arm).
        self.armed.remove(dsr_id);
        self.arm(dsr_id.clone(), tenant, new_fire_at_secs, new_deadline_secs, Some(reason))
            .expect("just disarmed — cannot be already-armed");
        Ok(())
    }

    /// Disarm a DSR's deadline timer (the DSR completed before the warning fired — the certificate
    /// sealed in time, so the warning is moot). Errors if there is no armed entry.
    pub fn disarm(&mut self, dsr_id: &DsrId) -> Result<(), TimerError> {
        self.armed
            .remove(dsr_id)
            .map(|_| ())
            .ok_or_else(|| TimerError::NotArmed(dsr_id.clone()))
    }

    /// The number of currently-armed entries (for telemetry / tests).
    pub fn armed_count(&self) -> usize {
        self.armed.len()
    }

    /// Whether a DSR currently has an armed deadline timer.
    pub fn is_armed(&self, dsr_id: &DsrId) -> bool {
        self.armed.contains_key(dsr_id)
    }

    /// The warning fire point for an armed DSR (for tests / the extension re-arm check).
    pub fn fire_at_for(&self, dsr_id: &DsrId) -> Option<u64> {
        self.armed.get(dsr_id).map(|e| e.fire_at_secs)
    }

    /// The recorded extension reason for an armed DSR (Art. 12(3)), if it was re-armed.
    pub fn extension_reason_for(&self, dsr_id: &DsrId) -> Option<String> {
        self.armed.get(dsr_id).and_then(|e| e.extension_reason.clone())
    }

    /// **Snapshot the wheel state (the durable `dsr_timer` rows).** The restart-survival model: the
    /// wheel state IS the durable state, so a crash-and-restart [`Self::restore`]s from this
    /// snapshot and the armed timers STILL fire (0 silent misses). The real `myelin-flow` wheel
    /// gives this for free via its durable table; this models it.
    pub fn snapshot(&self) -> Vec<TimerEntrySnapshot> {
        self.armed
            .values()
            .map(|e| TimerEntrySnapshot {
                dsr_id: e.dsr_id.clone(),
                tenant: e.tenant.clone(),
                fire_at_secs: e.fire_at_secs,
                deadline_secs: e.deadline_secs,
                extension_reason: e.extension_reason.clone(),
            })
            .collect()
    }

    /// **Restore a wheel from a snapshot (a crash-and-restart).** Re-populates the minute buckets
    /// from the durable rows — an orchestrator killed between arm and fire restores here and the
    /// timer STILL fires. The wheel is rebuilt EXACTLY (same entries, same buckets), so a `tick`
    /// after restore fires the same warnings it would have without the crash.
    pub fn restore(rows: Vec<TimerEntrySnapshot>) -> DsrTimerWheel {
        let mut armed = BTreeMap::new();
        for r in rows {
            armed.insert(
                r.dsr_id.clone(),
                TimerEntry {
                    dsr_id: r.dsr_id,
                    tenant: r.tenant,
                    fire_at_secs: r.fire_at_secs,
                    deadline_secs: r.deadline_secs,
                    extension_reason: r.extension_reason,
                },
            );
        }
        DsrTimerWheel { armed }
    }
}

// ───────────────────────── the deadline-timer facade (the orchestrator's arm path) ────────────

/// **The DSR deadline durable timer (the facade the orchestrator drives).** Wraps a
/// [`DsrTimerWheel`] + the [`DsrDeadline`] thresholds + an injectable [`Clock`]. On `dsr_submit`
/// the orchestrator calls [`Self::arm_deadline`] to arm the durable warning timer (REPLACING the
/// P-GA-14 coarse `deadline_secs` field tracking); a [`Self::tick`] fires due warnings; an
/// extension re-arms via [`Self::extend_deadline`].
///
/// The clock is injectable so the wheel is deterministic across the warning + deadline boundaries
/// (the drills advance a [`myelin_substrate::TestClock`]); the [`DsrDeadline`] thresholds come from
/// the versioned `thresholds.toml` (`dsr.deadline_secs` / `dsr.warning_margin_secs` /
/// `dsr.extension_total_secs`) — no magic number is hardcoded.
pub struct DsrDeadlineTimer<C: Clock> {
    clock: C,
    thresholds: DsrDeadline,
    wheel: DsrTimerWheel,
}

impl<C: Clock> DsrDeadlineTimer<C> {
    /// Build a deadline timer over an injectable clock + the DSR deadline thresholds (from
    /// `thresholds.toml`). Production wires [`myelin_substrate::SystemClock`] + the loaded
    /// thresholds; the drills wire [`myelin_substrate::TestClock`] + the default thresholds.
    pub fn new(clock: C, thresholds: DsrDeadline) -> DsrDeadlineTimer<C> {
        DsrDeadlineTimer { clock, thresholds, wheel: DsrTimerWheel::new() }
    }

    /// **Arm the durable deadline timer on submit (gdpr §4.1 step 6).** Computes the statutory
    /// deadline (`submitted_at + deadline_secs`) and the WARNING point (`deadline −
    /// warning_margin`), and arms a [`DsrTimerWheel`] entry at the warning point. Returns the
    /// statutory deadline (the `deadline_secs` the [`crate::dsr::Dsr`] carries — unchanged shape).
    ///
    /// The warning point is `deadline − warning_margin` (the `−` the mutation floor guards: a `+`
    /// would fire the warning AFTER the deadline, defeating the purpose). If the margin would push
    /// the warning before `submitted_at` (a margin wider than the whole window — a misconfiguration),
    /// the warning is armed at `submitted_at` (fire immediately on the first tick — fail-loud-early,
    /// never silently never-fire).
    pub fn arm_deadline(
        &mut self,
        dsr_id: DsrId,
        tenant: TenantId,
        submitted_at_secs: u64,
    ) -> Result<u64, TimerError> {
        let deadline_secs = submitted_at_secs + self.thresholds.deadline_secs;
        // the warning fires `warning_margin` BEFORE the deadline (the nearing-deadline point).
        // saturating at submitted_at: a margin wider than the window fires early, never never.
        let warning_at_secs =
            deadline_secs.saturating_sub(self.thresholds.warning_margin_secs).max(submitted_at_secs);
        self.wheel.arm(dsr_id, tenant, warning_at_secs, deadline_secs, None)?;
        Ok(deadline_secs)
    }

    /// **Extend the deadline to 3 months for a complex request (Art. 12(3), recorded reason).**
    /// Re-arms the wheel entry at the extension warning point (`extension_deadline −
    /// warning_margin`) with the recorded reason. The extension deadline is `submitted_at +
    /// extension_total_secs` (the 3-month total). Returns the new statutory deadline.
    pub fn extend_deadline(
        &mut self,
        dsr_id: &DsrId,
        submitted_at_secs: u64,
        reason: String,
    ) -> Result<u64, TimerError> {
        let new_deadline_secs = submitted_at_secs + self.thresholds.extension_total_secs;
        let new_warning_at_secs = new_deadline_secs
            .saturating_sub(self.thresholds.warning_margin_secs)
            .max(submitted_at_secs);
        self.wheel.rearm_extension(dsr_id, new_warning_at_secs, new_deadline_secs, reason)?;
        Ok(new_deadline_secs)
    }

    /// Disarm a DSR's deadline timer (it completed before the warning — the certificate sealed in
    /// time). Errors if there is no armed entry.
    pub fn disarm(&mut self, dsr_id: &DsrId) -> Result<(), TimerError> {
        self.wheel.disarm(dsr_id)
    }

    /// **`tick` — fire every due warning at the CURRENT clock.** Reads `now` off the injectable
    /// clock and fires the wheel. Returns the fired [`DsrDeadlineWarning`]s (the caller hands them
    /// to the outbox — the named emit floor).
    pub fn tick(&mut self) -> Vec<DsrDeadlineWarning> {
        self.wheel.tick(self.clock.now_secs())
    }

    /// **`tick_at(now)` — fire every due warning at an EXPLICIT `now`.** The deterministic drill /
    /// CDC entry point: the wheel state is the durable truth, so firing it at an explicit wall-clock
    /// second (rather than reading the injectable clock) lets a drill step the wheel across the
    /// warning + deadline boundaries without re-borrowing the clock. Identical semantics to
    /// [`Self::tick`] — the same minute-bucket fire test.
    pub fn tick_at(&mut self, now_secs: u64) -> Vec<DsrDeadlineWarning> {
        self.wheel.tick(now_secs)
    }

    /// Borrow the underlying wheel (for snapshot / restore / introspection in the drills).
    pub fn wheel(&self) -> &DsrTimerWheel {
        &self.wheel
    }

    /// Replace the underlying wheel (the restart-survival path: snapshot the wheel, kill, restore).
    pub fn restore_wheel(&mut self, wheel: DsrTimerWheel) {
        self.wheel = wheel;
    }
}

// A test-only clock-advance helper on the facade (the drills advance the timer's own clock across
// the warning + deadline boundaries). Gated to the crate's tests.
#[cfg(test)]
impl DsrDeadlineTimer<myelin_substrate::TestClock> {
    fn advance_for_test(&self, secs: u64) {
        self.clock.advance(secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::TestClock;

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    fn dsr(n: u64) -> DsrId {
        DsrId(format!("dsr:{n}"))
    }

    fn thresholds() -> DsrDeadline {
        DsrDeadline::default()
    }

    // ───────────── arming a DSR schedules a wheel entry at the nearing-deadline point ─────────

    #[test]
    fn arming_a_dsr_schedules_a_wheel_entry_at_now_plus_one_month_minus_margin() {
        let t0 = 1_700_000_000;
        let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thresholds());
        let deadline = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();
        // the statutory deadline is now + 1 month (the §4.1 shape — unchanged).
        assert_eq!(deadline, t0 + 30 * 24 * 60 * 60);
        // the warning is armed at deadline − warning_margin (1 week before).
        let expected_warning = deadline - 7 * 24 * 60 * 60;
        assert_eq!(timer.wheel().fire_at_for(&dsr(0)), Some(expected_warning));
        assert!(timer.wheel().is_armed(&dsr(0)));
        assert_eq!(timer.wheel().armed_count(), 1);
    }

    // ───────────── the nearing-deadline margin fires the warning Signal ─────────

    #[test]
    fn the_nearing_deadline_margin_fires_the_warning_signal_before_the_deadline() {
        let t0 = 1_700_000_000;
        let clock = TestClock::at(t0);
        let mut timer = DsrDeadlineTimer::new(clock, thresholds());
        let deadline = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();

        // a tick well before the warning point fires NOTHING (the deadline is not yet near).
        let early = timer.tick();
        assert!(early.is_empty(), "no warning fires a month out");
        assert!(timer.wheel().is_armed(&dsr(0)), "the timer stays armed");

        // advance to the warning point (1 week before the deadline) and tick.
        // NOTE: TestClock is shared by reference — re-borrow via a fresh timer over the same wheel
        // is unnecessary; we advance the timer's own clock.
        let warning_at = deadline - 7 * 24 * 60 * 60;
        // advance to exactly the warning minute.
        let to_advance = warning_at - t0;
        timer.advance_for_test(to_advance);
        let fired = timer.tick();
        assert_eq!(fired.len(), 1, "the warning fires at the nearing-deadline point");
        let w = &fired[0];
        assert_eq!(w.dsr_id, dsr(0));
        assert_eq!(w.tenant, tenant());
        assert_eq!(w.deadline_secs, deadline);
        // the warning fires BEFORE the deadline — the margin is positive (~1 week).
        assert!(w.margin_remaining_secs > 0, "the warning fires before the deadline");
        assert_eq!(w.margin_remaining_secs, 7 * 24 * 60 * 60);
        // fire-once: the entry is gone after firing.
        assert!(!timer.wheel().is_armed(&dsr(0)), "the warning fired once and is disarmed");
        assert_eq!(timer.wheel().armed_count(), 0);
    }

    // ───────────── a restart between arm and fire STILL fires (the wheel is durable) ─────────

    #[test]
    fn a_restart_between_arm_and_fire_still_fires_the_warning() {
        let t0 = 1_700_000_000;
        let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thresholds());
        let deadline = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();

        // CRASH: snapshot the durable wheel state, then drop the orchestrator entirely.
        let durable_rows = timer.wheel().snapshot();
        assert_eq!(durable_rows.len(), 1, "the armed timer is durable state, not in-process state");
        drop(timer);

        // RESTART: a fresh orchestrator (a fresh clock) restores the wheel from the durable rows.
        let warning_at = deadline - 7 * 24 * 60 * 60;
        let mut restarted = DsrDeadlineTimer::new(TestClock::at(warning_at), thresholds());
        restarted.restore_wheel(DsrTimerWheel::restore(durable_rows));
        assert!(restarted.wheel().is_armed(&dsr(0)), "the timer survived the restart");

        // the timer STILL fires (0 silent misses — the GA-D4 restart leg).
        let fired = restarted.tick();
        assert_eq!(fired.len(), 1, "the restored timer fires — the restart did not lose it");
        assert_eq!(fired[0].dsr_id, dsr(0));
    }

    // ───────────── the extension-to-3-months re-arms with a recorded reason (Art. 12(3)) ─────

    #[test]
    fn the_extension_to_three_months_rearms_with_a_recorded_reason() {
        let t0 = 1_700_000_000;
        let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thresholds());
        let one_month = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();
        let one_month_warning = timer.wheel().fire_at_for(&dsr(0)).unwrap();

        // extend to 3 months for a complex request, with a recorded reason.
        let reason = "complex: cross-cell member iteration".to_string();
        let three_months = timer.extend_deadline(&dsr(0), t0, reason.clone()).unwrap();
        // the new deadline is 3 months (90 days) — later than 1 month.
        assert_eq!(three_months, t0 + 90 * 24 * 60 * 60);
        assert!(three_months > one_month);
        // the warning re-armed LATER (3-month warning point > 1-month warning point).
        let three_month_warning = timer.wheel().fire_at_for(&dsr(0)).unwrap();
        assert!(three_month_warning > one_month_warning, "the warning re-armed later");
        assert_eq!(three_month_warning, three_months - 7 * 24 * 60 * 60);
        // the reason is RECORDED on the entry (Art. 12(3)).
        assert_eq!(timer.wheel().extension_reason_for(&dsr(0)), Some(reason));
        // still exactly one armed entry (the re-arm disarmed the old, armed the new — no pollution).
        assert_eq!(timer.wheel().armed_count(), 1);

        // the OLD (1-month) warning point no longer fires (a tick at it fires nothing).
        let mut at_old_warning = DsrDeadlineTimer::new(TestClock::at(one_month_warning), thresholds());
        at_old_warning.restore_wheel(DsrTimerWheel::restore(timer.wheel().snapshot()));
        assert!(at_old_warning.tick().is_empty(), "the old warning point is disarmed by the extension");
    }

    // ───────────── an extension must EXTEND, never shorten (Art. 12(3)) ─────────

    #[test]
    fn an_extension_that_does_not_extend_is_a_loud_error() {
        let t0 = 1_700_000_000;
        let mut wheel = DsrTimerWheel::new();
        wheel.arm(dsr(0), tenant(), t0 + 100, t0 + 200, None).unwrap();
        // a re-arm to an EARLIER deadline is rejected (an extension extends).
        let err = wheel.rearm_extension(&dsr(0), t0 + 50, t0 + 150, "x".into()).unwrap_err();
        assert_eq!(err, TimerError::ExtensionNotLater { current_secs: t0 + 200, requested_secs: t0 + 150 });
        // the original entry is untouched (the rejected re-arm did not pollute the wheel).
        assert_eq!(wheel.fire_at_for(&dsr(0)), Some(t0 + 100));
    }

    // ───────────── arming the same DSR twice is a loud error (a double-submit bug) ─────────

    #[test]
    fn arming_the_same_dsr_twice_is_a_loud_error() {
        let mut wheel = DsrTimerWheel::new();
        wheel.arm(dsr(0), tenant(), 100, 200, None).unwrap();
        let err = wheel.arm(dsr(0), tenant(), 300, 400, None).unwrap_err();
        assert_eq!(err, TimerError::AlreadyArmed(dsr(0)));
    }

    // ───────────── re-arming / disarming an un-armed DSR is a loud error ─────────

    #[test]
    fn rearming_or_disarming_an_unarmed_dsr_is_a_loud_error() {
        let mut wheel = DsrTimerWheel::new();
        assert_eq!(
            wheel.rearm_extension(&dsr(9), 100, 200, "x".into()).unwrap_err(),
            TimerError::NotArmed(dsr(9))
        );
        assert_eq!(wheel.disarm(&dsr(9)).unwrap_err(), TimerError::NotArmed(dsr(9)));
    }

    // ───────────── the minute-bucket fire boundary is inclusive (≤, not <) ─────────

    #[test]
    fn the_minute_bucket_fire_boundary_is_inclusive() {
        let mut wheel = DsrTimerWheel::new();
        // arm at second 600 (minute bucket 10), deadline 660.
        wheel.arm(dsr(0), tenant(), 600, 660, None).unwrap();
        // a tick at second 599 (minute bucket 9) does NOT fire (the bucket is not yet reached).
        assert!(wheel.tick(599).is_empty(), "minute bucket 9 < 10: not yet due");
        assert!(wheel.is_armed(&dsr(0)), "still armed");
        // a tick at second 600 (minute bucket 10) FIRES (the boundary is inclusive — ≤).
        let fired = wheel.tick(600);
        assert_eq!(fired.len(), 1, "minute bucket 10 == 10: due (inclusive boundary)");
        assert_eq!(fired[0].dsr_id, dsr(0));
    }

    // ───────────── a tick past the deadline reports a 0 margin (never underflows) ─────────

    #[test]
    fn a_tick_past_the_deadline_reports_zero_margin_not_an_underflow() {
        let mut wheel = DsrTimerWheel::new();
        wheel.arm(dsr(0), tenant(), 600, 660, None).unwrap();
        // tick well PAST the deadline (second 1000 > deadline 660).
        let fired = wheel.tick(1000);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].margin_remaining_secs, 0, "past-deadline margin is 0, never an underflow");
    }

    // ───────────── disarm on completion (the certificate sealed before the warning) ─────────

    #[test]
    fn disarm_on_completion_removes_the_armed_warning() {
        let t0 = 1_700_000_000;
        let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thresholds());
        timer.arm_deadline(dsr(0), tenant(), t0).unwrap();
        assert!(timer.wheel().is_armed(&dsr(0)));
        // the DSR completes (the certificate sealed in time) → disarm.
        timer.disarm(&dsr(0)).unwrap();
        assert!(!timer.wheel().is_armed(&dsr(0)));
        // a subsequent tick fires nothing (the moot warning was disarmed).
        timer.advance_for_test(40 * 24 * 60 * 60);
        assert!(timer.tick().is_empty(), "a completed DSR fires no warning");
    }

    // ───────────── multiple DSRs fire in a deterministic id order ─────────

    #[test]
    fn multiple_due_warnings_fire_in_deterministic_id_order() {
        let mut wheel = DsrTimerWheel::new();
        wheel.arm(dsr(2), tenant(), 600, 660, None).unwrap();
        wheel.arm(dsr(0), tenant(), 600, 660, None).unwrap();
        wheel.arm(dsr(1), tenant(), 600, 660, None).unwrap();
        let fired = wheel.tick(600);
        let ids: Vec<&str> = fired.iter().map(|w| w.dsr_id.0.as_str()).collect();
        assert_eq!(ids, vec!["dsr:0", "dsr:1", "dsr:2"], "deterministic id order");
    }
}
