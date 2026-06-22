//! # The retention engine: tightest-policy-wins merge + legal-hold-aware suspend-don't-delete
//! (P-GA-22 → P-149, GA-D6)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§5.1** (the retention
//! engine — *"**Tightest-policy-wins merge**: effective retention = the most restrictive that does
//! not violate a legal-retention floor (a tenant's "delete chat after 30 days" beats a 90-day
//! platform default; a lawful 6-month security-log floor beats a tenant's "delete logs
//! immediately"). The merge is deterministic and recorded (auditable which input won).
//! **Legal-hold-aware (suspend, don't delete):** an active hold suspends both retention-expiry and
//! erasure for the held scope (Art. 17(3)(e)); on hold-lift the deferred deletion resumes. Expiry
//! uses the same erasure mechanisms (§3)."*). Prove-it / stop-the-bleeding:
//! `external-insights/01-process-and-quality-doctrine.md` §2 (*silent data loss outranks every
//! feature — a legal-hold must fail-safe-to-suspend, never auto-delete*) + §3 (*prove-it — 0
//! held-scope deletions, observed*).
//!
//! **Contract-index:** OWNS the **retention leg of row 10.5** — `effective_retention(category,
//! tenant, store) → Policy` (tightest-wins, legal-hold-aware) + the legal-hold-aware
//! suspend-don't-delete expiry path. The consent / sub-processor / `transfer_allowed` legs of 10.5
//! are **P-GA-23 → P-150** (a named follow-on; this module ships ONLY the retention + suspend leg).
//! CONSUMES row **10.1** (the holders expiry fans over — driven via the EXISTING
//! [`crate::orchestration::UpstreamHolderOrchestrator`], the §3 erasure mechanisms) and the EXISTING
//! G4 legal-hold gate ([`crate::fanout::LegalHoldRegistry`], wired in P-GA-12 — the engine BACKS
//! it; it does NOT re-define a second hold registry, EI-01 §7 coherence).
//!
//! ## What THIS prompt (P-GA-22) ships — and what it reuses
//! 1. **The tightest-policy-wins merge** ([`RetentionEngine::effective_retention`]) — given the
//!    per-field retention inputs for a `(category, tenant, store)` (each a frozen
//!    [`myelin_gdpr::RetentionClass`] from G3, tagged with which input named it —
//!    [`RetentionSource`]), it picks the **most restrictive** policy that **does not violate a
//!    legal-retention floor**, deterministically, and **records which input won**
//!    ([`EffectiveRetention::winning_source`]). A tenant "delete after 30 days" beats a 90-day
//!    platform default; a lawful 6-month security-log floor ([`myelin_gdpr::RetentionClass::
//!    AuditCarveOut`]) beats a tenant's "delete immediately" (the floor is never violated — it is a
//!    LOWER BOUND on retention the merge cannot drop below).
//! 2. **The legal-hold-aware suspend-don't-delete expiry** ([`RetentionEngine::expire`]) — when a
//!    field's retention window has elapsed, the engine attempts to expire it (Art. 5(1)(e)) USING
//!    the §3 erasure mechanisms (the SAME canonical-order holder fan-out the DSR erase uses). But an
//!    active `legal_hold` over the scope (read through the EXISTING [`crate::fanout::
//!    LegalHoldRegistry`], G4) **suspends** the expiry: the deletion is DEFERRED (recorded, never
//!    run — `0 held-scope deletions`), and **resumes on hold-lift** (a later [`RetentionEngine::
//!    expire`] over the now-unheld scope runs the expiry). The engine reuses the hold gate +
//!    the holder fan-out wholesale — it adds only the retention-merge + the suspend-or-run decision.
//!
//! ## The legal-retention floor (the §5.1 "does not violate a legal-retention floor" clause)
//! [`myelin_gdpr::RetentionClass::AuditCarveOut(d)`] is a **legal-retention floor**: a per-
//! jurisdiction lawful MINIMUM retention (a 6-month security-log floor; an audit carve-out). The
//! tightest-wins merge treats it as a LOWER BOUND — the effective retention may never be SHORTER
//! than the longest legal floor among the inputs, even if a tenant policy asks for `delete
//! immediately`. Among the NON-floor inputs the merge picks the most restrictive (shortest); the
//! result is then clamped UP to the longest floor. This is the "tenant delete-immediately is
//! overridden by a lawful 6-month floor" case in §5.1, made deterministic.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **GA-D6 runs at M2 scale here** (the in-memory M1-store model — the same store/KMS floor every
//!   M0/M1 store carries, P-007 / P-S12); it **re-confirms at CELL scale at M5** → **P-GA-35**
//!   (the multi-cell retention sweep). The expiry fan-out reuses this engine.
//! - **The consent / sub-processor registries + the `transfer_allowed` gate** (the rest of 10.5)
//!   → **P-GA-23 → P-150** (this prompt ships ONLY the retention + legal-hold-aware suspend leg).
//! - **The durable Postgres `retention_policy` (G3) table + the periodic expiry SWEEP scheduler**
//!   (the `myelin-flow` wheel that periodically calls [`RetentionEngine::expire`] over the elapsed
//!   fields) is the same DB / timer floor every M0/M1 store carries (P-007 / P-S12 / the P-GA-21
//!   wheel). On this floor the engine is a pure decision + a single-shot expiry driver the caller
//!   invokes; the periodic sweep is one `expire` call per elapsed field on the wheel — a config
//!   wire, not a code change. The retention WINDOW elapse check ([`EffectiveRetention::
//!   has_elapsed`]) is deterministic over an injectable [`myelin_substrate::Clock`].
//!
//! ## Mutation floor (P-GA-22 TESTS — the tightest-wins merge + the legal-hold suspend paths are
//! mandatory-core). `cargo mutants -p myelin-gdpr-service -f crates/myelin-gdpr-service/src/
//! retention.rs` (2026-06-20): **25 mutants, 18 caught, 6 unviable, 1 missed.** Every BEHAVIORAL
//! mutant on the mandatory-core paths is CAUGHT — [`RetentionEngine::effective_retention`] (the
//! most-restrictive-non-floor pick `min_by`, the floor `max`, the strict `floor > pick` clamp — `>`
//! not `>=`, the tie-break `then(source.cmp)`, the recorded winning source per arm), the
//! [`RetentionInput::is_legal_floor`] `||` (a source-named floor AND an `AuditCarveOut`-policy floor
//! are each caught), the [`RetentionInput::window_secs`] mapping, [`EffectiveRetention::has_elapsed`]
//! (the `>=` window check + the `u64::MAX` open-ended short-circuit + the `saturating_sub`), and
//! [`RetentionEngine::expire`] (the held ⇒ DEFER / un-held ⇒ RUN decision — fail-safe-to-suspend,
//! never an auto-delete under hold; the resume-on-lift). The 1 residual is the documented non-core
//! cosmetic class: `<ExpiryError as Display>::fmt -> Ok(Default::default())` — the human-readable
//! error MESSAGE text (the error VARIANT is mutation-killed: the `expire` callers assert the typed
//! [`ExpiryOutcome`] by `PartialEq`; only the rendered string body is unkilled, which is cosmetic,
//! not behavior — the SAME residual class `dsr_timer` carries). Stated, not hidden (EI-01 §3).

use core::time::Duration;

use myelin_gdpr::{EraseScope, RetentionClass};

use crate::fanout::{HoldVerdict, LegalHoldRegistry};
use crate::orchestration::{HolderReceipt, UpstreamHolderOrchestrator};

/// The `legal_hold_active_count`-paired telemetry: the `retention_held_scope_deletions` invariant
/// signal (contract 1.8 face). It is the GA-D6 green artifact's value — it MUST read `0` (a deletion
/// under an active hold is the silent-data-loss failure §2 outranks every feature). PII-free: a
/// count, never a held subject.
pub const RETENTION_HELD_SCOPE_DELETIONS: (&str, &str) =
    ("gdpr.retention_held_scope_deletions", "count");

/// The `retention_expiry_runs` telemetry: the count of retention-expiry fan-outs that RAN (a field's
/// window elapsed AND no hold barred it). PII-free. Paired with [`RETENTION_HELD_SCOPE_DELETIONS`]
/// (which must stay `0`) it is the retention engine's health face.
pub const RETENTION_EXPIRY_RUNS: (&str, &str) = ("gdpr.retention_expiry_runs", "count");

// ───────────────────────── the per-field retention input ─────────────────────────

/// **Which input named a retention policy** (gdpr §5.1 — the merge is *recorded: auditable which
/// input won*). A tenant-configured policy, the platform default, or a lawful legal-retention floor.
/// The merge records the WINNING source on [`EffectiveRetention::winning_source`] so an Art. 28
/// audit can see "the tenant's 30-day policy won" / "the 6-month legal floor overrode the tenant".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetentionSource {
    /// The tenant's configured retention policy (G3 `retention_policy`, per-tenant). The most
    /// common winner — a tenant "delete chat after 30 days" beats the platform default.
    TenantPolicy,
    /// The platform default retention (the baseline when a tenant has not configured one — a 90-day
    /// default). The most restrictive non-floor input wins, so a tenant policy usually beats it.
    PlatformDefault,
    /// A **legal-retention floor** — a per-jurisdiction lawful MINIMUM retention (a 6-month
    /// security-log floor; an audit carve-out, [`myelin_gdpr::RetentionClass::AuditCarveOut`]). A
    /// floor is a LOWER BOUND the merge can never drop below; it OVERRIDES a tenant "delete
    /// immediately" (§5.1).
    LegalFloor,
}

/// One per-field retention INPUT the merge considers — a [`RetentionClass`] (the G3 frozen tag) +
/// which [`RetentionSource`] named it. The merge over the inputs for a `(category, tenant, store)`
/// yields the [`EffectiveRetention`] (tightest-wins, floor-respecting, recorded).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionInput {
    /// The retention policy this input asserts (the G3 frozen tag).
    pub policy: RetentionClass,
    /// Which input named it (for the recorded "which won" — §5.1).
    pub source: RetentionSource,
}

impl RetentionInput {
    /// A convenience constructor.
    pub fn new(policy: RetentionClass, source: RetentionSource) -> RetentionInput {
        RetentionInput { policy, source }
    }

    /// The retention DURATION this input bounds the data to, in seconds — the comparable scalar the
    /// tightest-wins merge orders on. The merge picks the SHORTEST (most restrictive) among the
    /// non-floor inputs and clamps UP to the longest floor.
    ///
    /// - `Fixed(d)` / `AuditCarveOut(d)` — the explicit duration `d`.
    /// - `TenantPolicy` — a SYMBOLIC "delete per the tenant's configured window"; on this engine the
    ///   tenant's concrete window is supplied as a `Fixed(d)` input (the registry resolves the
    ///   symbol to a duration before the merge). A bare `TenantPolicy` with no resolved duration is
    ///   treated as `0` (delete-as-soon-as-allowed — the most restrictive the tenant can ask), so a
    ///   legal floor will clamp it up. This is the "tenant delete-immediately" §5.1 case.
    /// - `UntilContractEnd` — an OPEN-ended retention (retain while the contract lives); modelled as
    ///   the maximum window ([`u64::MAX`]) so it never wins the most-restrictive pick (it is the
    ///   LEAST restrictive — retain longest). Offboarding (not retention-expiry) ends it (§4.4).
    pub fn window_secs(&self) -> u64 {
        match &self.policy {
            RetentionClass::Fixed(d) | RetentionClass::AuditCarveOut(d) => d.as_secs(),
            RetentionClass::TenantPolicy => 0,
            RetentionClass::UntilContractEnd => u64::MAX,
        }
    }

    /// Whether this input is a **legal-retention floor** (a LOWER BOUND the merge cannot drop below).
    /// A floor is named by its [`RetentionSource::LegalFloor`] source OR by being an
    /// [`RetentionClass::AuditCarveOut`] (the §6.4 audit carve-out IS a legal floor by construction).
    pub fn is_legal_floor(&self) -> bool {
        self.source == RetentionSource::LegalFloor
            || matches!(self.policy, RetentionClass::AuditCarveOut(_))
    }
}

// ───────────────────────── the merge result (the recorded effective retention) ─────────────────────────

/// **The effective retention for a `(category, tenant, store)`** — the deterministic, RECORDED
/// result of the tightest-policy-wins merge (§5.1). It carries the winning policy, the winning
/// SOURCE (auditable which input won), and whether a legal floor clamped the result (the "the floor
/// overrode the tenant" case). The retention WINDOW is [`EffectiveRetention::window_secs`]; whether
/// a field's window has elapsed by `now` is [`EffectiveRetention::has_elapsed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveRetention {
    /// The winning retention window, in seconds (the most-restrictive non-floor input, clamped UP to
    /// the longest legal floor). `0` ⇒ delete-as-soon-as-allowed; [`u64::MAX`] ⇒ retain (open-ended).
    pub window_secs: u64,
    /// **Which input won** (§5.1 — recorded, auditable). For a tenant policy that survived the merge
    /// this is [`RetentionSource::TenantPolicy`]; for a tenant "delete immediately" overridden by a
    /// 6-month floor this is [`RetentionSource::LegalFloor`] (the floor won the EFFECTIVE window).
    pub winning_source: RetentionSource,
    /// Whether a legal-retention floor CLAMPED the result UP (the tenant/default asked for shorter
    /// than the floor). When `true`, [`Self::winning_source`] is [`RetentionSource::LegalFloor`].
    pub floor_clamped: bool,
}

impl EffectiveRetention {
    /// The effective retention window, in seconds.
    pub fn window_secs(&self) -> u64 {
        self.window_secs
    }

    /// **Whether a field stored at `stored_at_secs` has reached its retention expiry by `now_secs`**
    /// (Art. 5(1)(e) — *retained no longer than necessary*). Open-ended retention
    /// ([`u64::MAX`] window) NEVER elapses (it ends by offboarding, not expiry — §4.4). A `0` window
    /// is elapsed immediately (delete-as-soon-as-allowed). Saturating so a far-future `stored_at`
    /// (clock skew) does not underflow into a spurious expiry.
    pub fn has_elapsed(&self, stored_at_secs: u64, now_secs: u64) -> bool {
        if self.window_secs == u64::MAX {
            return false;
        }
        now_secs.saturating_sub(stored_at_secs) >= self.window_secs
    }
}

// ───────────────────────── the expiry outcome (the legal-hold-aware decision) ─────────────────────────

/// The outcome of attempting to expire a field whose retention window has elapsed
/// ([`RetentionEngine::expire`]). The legal-hold-aware suspend-don't-delete decision (§5.1):
/// **the expiry RAN** (no hold barred it — the holders fanned, the field erased via the §3
/// mechanisms), or **the expiry was DEFERRED under a legal hold** (suspend-don't-delete — the
/// deletion did NOT run, recorded; it resumes on hold-lift). The "0 held-scope deletions" GA-D6
/// invariant is: a [`ExpiryOutcome::DeferredUnderHold`] NEVER carries a holder receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpiryOutcome {
    /// The retention-expiry RAN: the holders fanned in canonical order, the field erased via the §3
    /// erasure mechanisms. Carries the ordered per-holder receipts (each recording its destroyed key
    /// epoch — the auditable expiry trail).
    Expired(Vec<HolderReceipt>),
    /// The expiry was **suspended under an active legal hold** (Art. 17(3)(e) — suspend, don't
    /// delete). The deletion did NOT run (0 held-scope deletions); it is recorded as deferred and
    /// **resumes on hold-lift** (a later [`RetentionEngine::expire`] over the now-unheld scope runs
    /// it). Carries NO holder receipt (nothing was erased).
    DeferredUnderHold,
}

impl ExpiryOutcome {
    /// Whether this outcome RAN a deletion (an [`ExpiryOutcome::Expired`]). The GA-D6 invariant: a
    /// deletion under an active hold must NEVER report `true` (0 held-scope deletions).
    pub fn ran_deletion(&self) -> bool {
        matches!(self, ExpiryOutcome::Expired(_))
    }
}

// ───────────────────────── the retention engine ─────────────────────────

/// **The retention engine (contract 10.5, the retention leg).** Two responsibilities, both §5.1:
/// (1) the **tightest-policy-wins merge** ([`Self::effective_retention`]) — deterministic + recorded
/// which input won, floor-respecting; (2) the **legal-hold-aware suspend-don't-delete expiry**
/// ([`Self::expire`]) — an elapsed field is expired via the §3 holder fan-out UNLESS an active hold
/// suspends it (then the deletion is deferred + resumes on lift; 0 held-scope deletions).
///
/// The engine BACKS the EXISTING G4 legal-hold gate ([`LegalHoldRegistry`], wired in P-GA-12) — it
/// reads the SAME registry the DSR-erase gate reads (no second hold store, EI-01 §7 coherence). It
/// DRIVES the EXISTING canonical-order holder fan-out ([`UpstreamHolderOrchestrator`], P-GA-06) for
/// the §3 erasure mechanisms — it does NOT re-implement the fan-out.
///
/// The engine is intentionally STATELESS (it holds only a reference to the hold gate): the durable
/// state is the G3 `retention_policy` table (the inputs) + the G4 `legal_hold` registry (the gate) +
/// the per-field `stored_at` (the caller's store). A re-`expire` over the same scope is idempotent
/// against the durable checklist the fan-out keeps (resumability is a property of the checklist).
pub struct RetentionEngine<'a> {
    /// The EXISTING G4 legal-hold gate (P-GA-12) the engine backs — read to suspend an expiry under
    /// a hold (fail-safe-to-suspend). Not re-defined here (coherence).
    holds: &'a LegalHoldRegistry,
}

impl<'a> RetentionEngine<'a> {
    /// Build a retention engine over the EXISTING G4 legal-hold registry (the gate it backs).
    pub fn new(holds: &'a LegalHoldRegistry) -> RetentionEngine<'a> {
        RetentionEngine { holds }
    }

    /// **`effective_retention(inputs) → Policy` — the tightest-policy-wins merge (§5.1).**
    /// Deterministic + RECORDED which input won. The algorithm:
    ///
    /// 1. Partition the inputs into **legal floors** ([`RetentionInput::is_legal_floor`]) and
    ///    **non-floor** inputs (tenant policy / platform default).
    /// 2. Among the **non-floor** inputs pick the **most restrictive** = the SHORTEST window (a
    ///    tenant "delete after 30 days" beats a 90-day platform default). Ties break toward the
    ///    [`RetentionSource`] with the lower ordinal (`TenantPolicy < PlatformDefault`) — a tenant
    ///    policy beats an equal-length platform default (the tenant's choice is recorded as winner).
    /// 3. The **legal floor** is a LOWER BOUND: the effective window is `max(most_restrictive,
    ///    longest_floor)`. If the longest floor is LONGER than the most-restrictive non-floor pick,
    ///    the floor CLAMPS the result UP (the "tenant delete-immediately overridden by a 6-month
    ///    floor" §5.1 case) and the recorded winner becomes [`RetentionSource::LegalFloor`].
    ///
    /// With NO inputs the result is open-ended ([`u64::MAX`], retain — never auto-delete absent a
    /// policy; §2 stop-the-bleeding: never delete data we have no policy authorising us to delete).
    /// With ONLY floors the result is the longest floor (the lawful minimum).
    pub fn effective_retention(&self, inputs: &[RetentionInput]) -> EffectiveRetention {
        // The longest legal floor (the LOWER BOUND the effective window cannot drop below). `0` if
        // there is no floor.
        let longest_floor: Option<u64> = inputs
            .iter()
            .filter(|i| i.is_legal_floor())
            .map(RetentionInput::window_secs)
            .max();

        // The most restrictive (shortest) NON-floor input. Ties break toward the lower-ordinal
        // source (TenantPolicy beats PlatformDefault at equal length). `None` if there is no
        // non-floor input.
        let most_restrictive: Option<&RetentionInput> = inputs
            .iter()
            .filter(|i| !i.is_legal_floor())
            .min_by(|a, b| {
                a.window_secs()
                    .cmp(&b.window_secs())
                    .then(a.source.cmp(&b.source))
            });

        match (most_restrictive, longest_floor) {
            // No inputs at all — retain (open-ended). Never auto-delete absent a policy (§2).
            (None, None) => EffectiveRetention {
                window_secs: u64::MAX,
                winning_source: RetentionSource::PlatformDefault,
                floor_clamped: false,
            },
            // Only legal floors — the longest floor is the effective window (the lawful minimum).
            (None, Some(floor)) => EffectiveRetention {
                window_secs: floor,
                winning_source: RetentionSource::LegalFloor,
                floor_clamped: true,
            },
            // Non-floor inputs, no floor — the most-restrictive non-floor pick wins outright.
            (Some(pick), None) => EffectiveRetention {
                window_secs: pick.window_secs(),
                winning_source: pick.source,
                floor_clamped: false,
            },
            // Both — clamp the most-restrictive pick UP to the longest floor (the floor is a lower
            // bound). If the floor clamped (floor > pick), the floor is the recorded winner.
            (Some(pick), Some(floor)) => {
                let pick_window = pick.window_secs();
                if floor > pick_window {
                    EffectiveRetention {
                        window_secs: floor,
                        winning_source: RetentionSource::LegalFloor,
                        floor_clamped: true,
                    }
                } else {
                    EffectiveRetention {
                        window_secs: pick_window,
                        winning_source: pick.source,
                        floor_clamped: false,
                    }
                }
            }
        }
    }

    /// **`expire(scope, upstream, checklist)` — the legal-hold-aware suspend-don't-delete expiry
    /// (§5.1).** Call this when a field's retention window has elapsed
    /// ([`EffectiveRetention::has_elapsed`]). The engine:
    ///
    /// 1. **Reads the EXISTING G4 legal-hold gate** ([`LegalHoldRegistry::verdict`]) for the scope —
    ///    a retention-expiry is an ERASE (it deletes data), so an active hold SUSPENDS it
    ///    (fail-safe-to-suspend — Art. 17(3)(e)). On a [`HoldVerdict::Deferred`] verdict the engine
    ///    returns [`ExpiryOutcome::DeferredUnderHold`] **without running any deletion** (0
    ///    held-scope deletions — the GA-D6 invariant). The deferred expiry **resumes on hold-lift**:
    ///    a later `expire` over the now-unheld scope runs the fan-out.
    /// 2. **Runs the §3 erasure mechanisms** when no hold bars the scope — it fans the erase out
    ///    through the EXISTING canonical-order holder orchestrator (the SAME §3 mechanisms the DSR
    ///    erase uses — crypto-shred / pseudonymise / purge-reindex). Returns
    ///    [`ExpiryOutcome::Expired`] with the ordered per-holder receipts.
    ///
    /// Idempotent + resumable: the fan-out re-drives only un-receipted holders (the durable
    /// [`crate::orchestration::EraseChecklist`] IS the state — a worker kill re-drives only the rest).
    pub fn expire(
        &self,
        scope: &EraseScope,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &crate::orchestration::EraseChecklist,
    ) -> Result<ExpiryOutcome, ExpiryError> {
        // §5.1 — the legal-hold gate. A retention-expiry IS an erase (it deletes data), so it is
        // gated as an erasure: an active hold (or an un-readable registry, fail-safe-to-suspend)
        // DEFERS the deletion. We do NOT run a single holder under a hold (0 held-scope deletions).
        match self.holds.verdict(crate::dsr::DsrKind::Erasure, scope) {
            HoldVerdict::Deferred => Ok(ExpiryOutcome::DeferredUnderHold),
            HoldVerdict::Proceed => {
                // No hold bars the scope — run the §3 erasure mechanisms via the EXISTING
                // canonical-order holder fan-out (resumable; re-drives only un-receipted holders).
                let receipts = upstream
                    .fan_out_erase(scope, checklist)
                    .map_err(|e| ExpiryError::HolderFanOut(e.0))?;
                Ok(ExpiryOutcome::Expired(receipts))
            }
        }
    }
}

/// A retention-expiry error — a holder fan-out failure (a holder erase errored; the checklist stays
/// resumable, so a re-`expire` re-drives only the un-receipted holders).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpiryError {
    /// A holder's erase failed during the expiry fan-out. Carries the holder's error string. The
    /// checklist is left resumable — a re-`expire` re-drives only the un-receipted holders.
    HolderFanOut(String),
}

impl core::fmt::Display for ExpiryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExpiryError::HolderFanOut(e) => {
                write!(f, "retention-expiry holder fan-out failed: {e}")
            }
        }
    }
}

impl std::error::Error for ExpiryError {}

/// Convenience: a [`RetentionInput`] for the platform default retention `d`.
pub fn platform_default(d: Duration) -> RetentionInput {
    RetentionInput::new(RetentionClass::Fixed(d), RetentionSource::PlatformDefault)
}

/// Convenience: a [`RetentionInput`] for a tenant-configured fixed retention window `d`.
pub fn tenant_window(d: Duration) -> RetentionInput {
    RetentionInput::new(RetentionClass::Fixed(d), RetentionSource::TenantPolicy)
}

/// Convenience: a [`RetentionInput`] for a tenant "delete as soon as allowed" policy (the symbolic
/// `TenantPolicy` with no concrete window — the §5.1 "delete immediately" case a legal floor clamps).
pub fn tenant_delete_immediately() -> RetentionInput {
    RetentionInput::new(RetentionClass::TenantPolicy, RetentionSource::TenantPolicy)
}

/// Convenience: a [`RetentionInput`] for a legal-retention floor of `d` (a lawful MINIMUM retention —
/// a 6-month security-log floor; the §6.4 audit carve-out).
pub fn legal_floor(d: Duration) -> RetentionInput {
    RetentionInput::new(
        RetentionClass::AuditCarveOut(d),
        RetentionSource::LegalFloor,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fanout::HoldScope;
    use crate::holders::{InMemoryShredKms, ShredKeyClass, ShredKeyHandle};
    use crate::orchestration::{
        holder_ids, EraseChecklist, SeamHolder, UpstreamHolderOrchestrator,
    };
    use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    const DAY: u64 = 24 * 60 * 60;

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            t("acme"),
        ))
    }

    fn subject_scope(s: &str) -> EraseScope {
        EraseScope::Subject {
            subject: subject(s),
            tenant: t("acme"),
        }
    }

    fn kms_with_all_holder_keys(tenant: &TenantId, base_epoch: u64) -> InMemoryShredKms {
        let kms = InMemoryShredKms::new();
        for (i, id) in [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .iter()
        .enumerate()
        {
            kms.provision(
                ShredKeyHandle {
                    tenant: tenant.clone(),
                    class: ShredKeyClass::Subject((*id).to_string()),
                },
                base_epoch + i as u64,
            );
        }
        kms
    }

    fn seam_holders(kms: &InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'_>)> {
        [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .into_iter()
        .map(|id| {
            (
                id,
                SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), kms),
            )
        })
        .collect()
    }

    fn upstream_over<'a>(
        holders: &'a [(&'static str, SeamHolder<'a>)],
    ) -> UpstreamHolderOrchestrator<'a> {
        UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        )
    }

    // ───────────── the tightest-policy-wins merge (§5.1) ─────────────

    /// **A tenant "delete after 30 days" beats a 90-day platform default** (§5.1 — the most
    /// restrictive non-floor input wins; the winning source is RECORDED).
    #[test]
    fn tenant_30_days_beats_platform_default_90_days() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[
            platform_default(Duration::from_secs(90 * DAY)),
            tenant_window(Duration::from_secs(30 * DAY)),
        ]);
        assert_eq!(
            eff.window_secs(),
            30 * DAY,
            "the tenant's 30 days (most restrictive) wins"
        );
        assert_eq!(
            eff.winning_source,
            RetentionSource::TenantPolicy,
            "recorded: the tenant won"
        );
        assert!(!eff.floor_clamped, "no floor involved");
    }

    /// **A lawful 6-month security-log floor beats a tenant's "delete logs immediately"** (§5.1 —
    /// the legal floor is a LOWER BOUND the merge cannot drop below; it overrides the tenant). The
    /// recorded winner is the LegalFloor (auditable: the floor overrode the tenant).
    #[test]
    fn legal_6_month_floor_overrides_tenant_delete_immediately() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let six_months = 180 * DAY;
        let eff = engine.effective_retention(&[
            tenant_delete_immediately(),
            legal_floor(Duration::from_secs(six_months)),
        ]);
        assert_eq!(
            eff.window_secs(),
            six_months,
            "the legal floor clamps the effective window UP"
        );
        assert_eq!(
            eff.winning_source,
            RetentionSource::LegalFloor,
            "recorded: the floor won"
        );
        assert!(
            eff.floor_clamped,
            "the floor clamped the tenant delete-immediately UP"
        );
    }

    /// **A floor that is SHORTER than the tenant's window does NOT change the result** (the floor is
    /// a lower bound; the tenant already retains longer than the minimum, so the tenant wins).
    #[test]
    fn a_floor_shorter_than_the_tenant_window_does_not_clamp() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[
            tenant_window(Duration::from_secs(365 * DAY)), // tenant retains a year
            legal_floor(Duration::from_secs(30 * DAY)),    // a 30-day minimum
        ]);
        assert_eq!(
            eff.window_secs(),
            365 * DAY,
            "the tenant year exceeds the floor — tenant wins"
        );
        assert_eq!(eff.winning_source, RetentionSource::TenantPolicy);
        assert!(
            !eff.floor_clamped,
            "the floor did not clamp (tenant > floor)"
        );
    }

    /// **No inputs ⇒ retain (open-ended), never auto-delete** (§2 stop-the-bleeding: never delete
    /// data absent a policy authorising it).
    #[test]
    fn no_inputs_retain_never_auto_delete() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[]);
        assert_eq!(
            eff.window_secs(),
            u64::MAX,
            "open-ended — retain, never auto-delete"
        );
        assert!(
            !eff.has_elapsed(0, u64::MAX),
            "an open-ended window never elapses"
        );
    }

    /// **A tenant policy beats an EQUAL-length platform default** (the tie breaks toward the tenant —
    /// the tenant's choice is the recorded winner).
    #[test]
    fn equal_length_tie_breaks_toward_the_tenant() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[
            platform_default(Duration::from_secs(30 * DAY)),
            tenant_window(Duration::from_secs(30 * DAY)),
        ]);
        assert_eq!(eff.window_secs(), 30 * DAY);
        assert_eq!(
            eff.winning_source,
            RetentionSource::TenantPolicy,
            "tie → the tenant won"
        );
    }

    /// **`has_elapsed` is deterministic over the clock** — a field stored at `t0` with a 30-day
    /// window is NOT elapsed at `t0 + 29d` and IS elapsed at `t0 + 30d`.
    #[test]
    fn has_elapsed_is_a_deterministic_window_check() {
        let eff = EffectiveRetention {
            window_secs: 30 * DAY,
            winning_source: RetentionSource::TenantPolicy,
            floor_clamped: false,
        };
        assert!(
            !eff.has_elapsed(1000, 1000 + 29 * DAY),
            "29 days < 30-day window — not elapsed"
        );
        assert!(
            eff.has_elapsed(1000, 1000 + 30 * DAY),
            "30 days reaches the window — elapsed"
        );
    }

    // ───────────── the legal-hold-aware suspend-don't-delete expiry (§5.1, GA-D6) ─────────────

    /// **An active legal hold SUSPENDS a retention-expiry (suspend-don't-delete) — 0 held-scope
    /// deletions; on hold-lift the deferred deletion RESUMES** (the core GA-D6 property).
    #[test]
    fn a_legal_hold_suspends_expiry_and_resumes_on_lift() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 100);
        let holders = seam_holders(&kms);
        let upstream = upstream_over(&holders);
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);

        // Set a hold over the subject — the engine MUST suspend the expiry.
        holds.set(
            HoldScope::Subject {
                tenant: "acme".into(),
                subject: "u-held".into(),
            },
            true,
        );
        let scope = subject_scope("u-held");
        let checklist = EraseChecklist::new();

        let outcome = engine.expire(&scope, &upstream, &checklist).unwrap();
        assert_eq!(
            outcome,
            ExpiryOutcome::DeferredUnderHold,
            "suspend-don't-delete under the hold"
        );
        assert!(!outcome.ran_deletion(), "0 held-scope deletions");
        // NOT A SINGLE HOLDER was erased under the hold (the silent-data-loss invariant, §2).
        assert_eq!(checklist.done_count(), 0, "no holder driven under the hold");
        for (_, h) in &holders {
            assert_eq!(
                h.erase_call_count(),
                0,
                "0 held-scope deletions — no holder called"
            );
        }

        // LIFT the hold and re-`expire`: the deferred deletion RESUMES (the fan-out now runs).
        holds.set(
            HoldScope::Subject {
                tenant: "acme".into(),
                subject: "u-held".into(),
            },
            false,
        );
        let outcome2 = engine.expire(&scope, &upstream, &checklist).unwrap();
        assert!(
            outcome2.ran_deletion(),
            "the deferred deletion resumes on hold-lift"
        );
        let receipts = match outcome2 {
            ExpiryOutcome::Expired(r) => r,
            other => panic!("expected Expired on resume, got {other:?}"),
        };
        assert_eq!(
            receipts.len(),
            6,
            "every holder fanned on resume (the §3 mechanisms)"
        );
        assert_eq!(
            receipts[0].holder_id,
            holder_ids::IDENTITY,
            "Identity FIRST (canonical order)"
        );
        // Every receipt records its destroyed key epoch (the §3 crypto-shred mechanism, auditable).
        for hr in &receipts {
            assert!(hr.receipt.receipt.key_epoch_destroyed.is_some());
        }
    }

    /// **An expiry NOT under a hold RUNS via the §3 erasure mechanisms** (the canonical-order holder
    /// fan-out — the same mechanisms the DSR erase uses).
    #[test]
    fn an_unheld_expiry_runs_the_section_3_erasure_mechanisms() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 200);
        let holders = seam_holders(&kms);
        let upstream = upstream_over(&holders);
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);

        let outcome = engine
            .expire(
                &subject_scope("u-expire"),
                &upstream,
                &EraseChecklist::new(),
            )
            .unwrap();
        assert!(outcome.ran_deletion());
        let receipts = match outcome {
            ExpiryOutcome::Expired(r) => r,
            other => panic!("expected Expired, got {other:?}"),
        };
        assert_eq!(receipts.len(), 6, "every holder expired in canonical order");
    }

    /// **A whole-tenant hold suspends a retention-expiry for a subject within the tenant** (the held
    /// scope covers every subject — 0 held-scope deletions).
    #[test]
    fn a_tenant_hold_suspends_a_subject_expiry() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 300);
        let holders = seam_holders(&kms);
        let upstream = upstream_over(&holders);
        let holds = LegalHoldRegistry::new();
        holds.set(HoldScope::Tenant("acme".into()), true);
        let engine = RetentionEngine::new(&holds);

        let outcome = engine
            .expire(&subject_scope("anyone"), &upstream, &EraseChecklist::new())
            .unwrap();
        assert_eq!(
            outcome,
            ExpiryOutcome::DeferredUnderHold,
            "the tenant hold suspends the expiry"
        );
        for (_, h) in &holders {
            assert_eq!(h.erase_call_count(), 0, "0 held-scope deletions");
        }
    }

    /// **Fail-safe-to-suspend: an un-readable hold registry SUSPENDS a retention-expiry** (never
    /// auto-deletes under a hold state it cannot rule out — §2 silent-data-loss outranks every
    /// feature).
    #[test]
    fn an_unreadable_hold_registry_fails_safe_to_suspend_expiry() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 400);
        let holders = seam_holders(&kms);
        let upstream = upstream_over(&holders);
        let holds = LegalHoldRegistry::new();
        holds.set_unreadable(true);
        let engine = RetentionEngine::new(&holds);

        let outcome = engine
            .expire(&subject_scope("x"), &upstream, &EraseChecklist::new())
            .unwrap();
        assert_eq!(
            outcome,
            ExpiryOutcome::DeferredUnderHold,
            "an un-readable registry fails safe to suspend (never auto-deletes)"
        );
        for (_, h) in &holders {
            assert_eq!(
                h.erase_call_count(),
                0,
                "fail-safe-to-suspend — no holder called"
            );
        }
    }

    /// **`is_legal_floor` is `source == LegalFloor` OR `AuditCarveOut` — EITHER alone suffices** (a
    /// floor named only by its SOURCE, and a floor named only by its AuditCarveOut POLICY, are both
    /// floors). This pins the `||` (not `&&`): a `Fixed` policy with a `LegalFloor` source IS a floor
    /// (a security-log floor expressed as a fixed window), and an `AuditCarveOut` with a non-floor
    /// source IS a floor (the §6.4 carve-out is a floor by construction).
    #[test]
    fn is_legal_floor_is_an_or_either_condition_alone_is_a_floor() {
        // a Fixed policy + a LegalFloor source — a floor by SOURCE alone (AuditCarveOut is false).
        let by_source = RetentionInput::new(
            RetentionClass::Fixed(Duration::from_secs(180 * DAY)),
            RetentionSource::LegalFloor,
        );
        assert!(
            by_source.is_legal_floor(),
            "LegalFloor source alone makes it a floor"
        );
        // an AuditCarveOut policy + a non-LegalFloor source — a floor by POLICY alone.
        let by_policy = RetentionInput::new(
            RetentionClass::AuditCarveOut(Duration::from_secs(180 * DAY)),
            RetentionSource::PlatformDefault,
        );
        assert!(
            by_policy.is_legal_floor(),
            "AuditCarveOut policy alone makes it a floor"
        );

        // and the merge HONOURS a source-named floor: a tenant "delete immediately" is clamped UP by
        // a Fixed-window floor named only by its LegalFloor source (the `||` is load-bearing here).
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[tenant_delete_immediately(), by_source]);
        assert_eq!(
            eff.window_secs(),
            180 * DAY,
            "the source-named floor clamps the tenant UP"
        );
        assert_eq!(eff.winning_source, RetentionSource::LegalFloor);
    }

    /// **A floor EXACTLY EQUAL to the non-floor pick does NOT clamp (the clamp is `floor > pick`,
    /// strict)** — when the tenant window equals the floor, the TENANT is the recorded winner (it
    /// already meets the minimum; the floor did not override). This pins the `>` (not `>=`): at
    /// equality the non-floor pick wins, not the floor.
    #[test]
    fn a_floor_equal_to_the_pick_does_not_clamp_the_pick_wins() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let thirty = 30 * DAY;
        let eff = engine.effective_retention(&[
            tenant_window(Duration::from_secs(thirty)),
            legal_floor(Duration::from_secs(thirty)), // floor == pick, exactly.
        ]);
        assert_eq!(
            eff.window_secs(),
            thirty,
            "equal windows — the window is the same either way"
        );
        assert_eq!(
            eff.winning_source,
            RetentionSource::TenantPolicy,
            "at floor == pick the TENANT wins (the clamp is strict `>`, not `>=`)"
        );
        assert!(
            !eff.floor_clamped,
            "the floor did not clamp (it equals, not exceeds, the pick)"
        );
    }

    /// **Only-floors inputs yield the longest floor** (the lawful minimum, recorded as the floor
    /// winner) — the audit carve-out / security-log retention floor with no tenant policy.
    #[test]
    fn only_floors_yield_the_longest_floor() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[
            legal_floor(Duration::from_secs(30 * DAY)),
            legal_floor(Duration::from_secs(180 * DAY)),
        ]);
        assert_eq!(
            eff.window_secs(),
            180 * DAY,
            "the longest floor is the lawful minimum"
        );
        assert_eq!(eff.winning_source, RetentionSource::LegalFloor);
        assert!(eff.floor_clamped);
    }
}
