//! # The cross-language harness shim — the conformance suite (P-S30 / global P-319)
//!
//! **Owning architecture doc:** `00-platform-substrate.md` §3.7 (the cross-language harness
//! shim — frozen as the divergence contract; the SEVEN non-negotiables a non-Rust subsystem
//! must satisfy *to the same guarantee the Rust harness does*) + contract-index row 1.7.
//!
//! **Owning doctrine:** EI-01 §5 (*an uncommitted gate is no gate* — the shim cannot be
//! quietly dropped at a language boundary; a violation must be **loud, never silently
//! swallowed**) and §7 (*reconcile cross-component contracts at the plan layer* — a
//! non-negotiable dropped at a boundary calcifies). EI-01 §4: *untested is acceptable only if
//! you name it untested — silent skipping is the failure mode.*
//!
//! ## What this module is
//! ADR-02 lets a subsystem diverge from Rust where it genuinely calls for it; the named
//! candidate is the **Chat connection tier** (a BEAM/Elixir tier for the connection-storm
//! load, TE-21). §3.7 freezes *what the per-language shim must provide*: a non-Rust subsystem
//! cannot ship without satisfying all seven non-negotiables, generated from the same
//! `myelin-events` / `myelin-identity` types. This module is the **enforcement mechanism** for
//! that frozen contract:
//!
//! - [`Nonnegotiable`] — the seven frozen non-negotiables as an exhaustive enum (§3.7 1..7).
//!   Adding/removing one is a deliberate enum edit, not a quiet drop.
//! - [`DivergentTierProbe`] — the trait a divergent (non-Rust) tier's conformance harness
//!   implements: it answers, per non-negotiable, whether the divergent tier provides it *to
//!   the same guarantee* (a probe against the real tier, not a self-assertion).
//! - [`ShimConformance::check`] — runs all seven probes and returns a [`ShimConformance`]
//!   verdict; `shim-conformance` is green **iff all seven pass** (the GATE signal ×7).
//! - [`ShimEnforcement`] — the *loud* record of how the shim was discharged for a given
//!   subsystem: [`ShimEnforcement::Enforced`] (the divergent tier passed the suite) **or**
//!   [`ShimEnforcement::RecordedNa`] (the subsystem stayed Rust — a NO-OP, but recorded
//!   **loudly** with a dated reason, never a silent skip). There is **no third variant** — a
//!   shim obligation is either enforced or loudly recorded N/A; it cannot evaporate.
//!
//! ## The all-Rust default (today): a loudly-recorded N/A
//! Chat's TE-21 pin is **Rust** (the BEAM hatch is written-but-CLOSED, `myelin_chat::glue`).
//! There is therefore no cross-language boundary today, so the shim's obligation is satisfied
//! as a **NO-OP** — but recorded LOUDLY via [`ShimEnforcement::recorded_na`], which carries
//! the pinned-language reason and a date, and emits an artifact row. The CDC pair + the loud
//! N/A row live in `tests/shim_conformance_p_s30.rs` (a dev-dependency on `myelin-chat`, so the
//! production harness dep set stays tiny — chat is read only by the test).
//!
//! ## When Chat diverges (CHAT-P26): the suite binds
//! If CHAT-D3/D4 prove the Rust connection tier intractable and the BEAM tier is selected, the
//! divergent tier's conformance harness implements [`DivergentTierProbe`] and
//! [`ShimEnforcement::enforce`] runs the seven probes — the tier cannot ship unless all seven
//! are green. The enum + the trait are the frozen surface either way; only the probe
//! implementation is new at divergence time.

use std::fmt;

/// The SEVEN frozen non-negotiables a non-Rust subsystem's shim MUST satisfy, to the SAME
/// guarantee the Rust harness provides (§3.7 items 1..7). This enum is the frozen contract
/// surface: it is exhaustive, and [`Nonnegotiable::ALL`] is the canonical seven-element set the
/// conformance suite iterates. Adding or removing a member is a deliberate, reviewed enum edit —
/// a non-negotiable cannot be quietly dropped at the language boundary (EI-01 §5/§7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Nonnegotiable {
    /// §3.7.1 — **three-surface topology**: public (gateway-fronted, identity-injected) /
    /// internal RPC (trust boundary only) / metrics-health; the public↔internal split as a
    /// security boundary.
    ThreeSurfaceTopology,
    /// §3.7.2 — **liveness ≠ readiness**: a dead critical dependency reports *not ready* and
    /// sheds (it does not report healthy-but-failing, and liveness does not restart-storm).
    LivenessNotReadiness,
    /// §3.7.3 — **no fire-and-forget emit**: the outbox pattern (BUS-2) — same-transaction
    /// insert + a relay; **no** `publish_now` path exists in the divergent language either.
    NoFireAndForgetEmit,
    /// §3.7.4 — **`PersonalDataHolder` registration**: every store the shim opens registers
    /// for DSR fan-out (locate/export/rectify/restrict/erase).
    PersonalDataHolderRegistration,
    /// §3.7.5 — **resilient-client behaviour**: per-call timeout / breaker / bulkhead /
    /// jittered-retry-idempotent-only / **`Retry-After` honouring** on every outbound
    /// inter-service call.
    ResilientClientRetryAfter,
    /// §3.7.6 — **principal-aware shed order**: speculative → batch/CI → agent → human-last,
    /// with the protected human lane and per-surface budgets (§7.6).
    PrincipalAwareShedOrder,
    /// §3.7.7 — **forward-only online migrations**: expand→backfill→contract, no rollback
    /// files, no blocking `ALTER` on a hot table — a substrate law, not a Rust-library feature.
    ForwardOnlyMigrations,
}

impl Nonnegotiable {
    /// The canonical, frozen seven-element set (§3.7.1..7) the conformance suite iterates. The
    /// length is asserted to be exactly 7 by [`tests`]; the GATE signal `shim-conformance` is
    /// green only when ALL of these pass.
    pub const ALL: [Nonnegotiable; 7] = [
        Nonnegotiable::ThreeSurfaceTopology,
        Nonnegotiable::LivenessNotReadiness,
        Nonnegotiable::NoFireAndForgetEmit,
        Nonnegotiable::PersonalDataHolderRegistration,
        Nonnegotiable::ResilientClientRetryAfter,
        Nonnegotiable::PrincipalAwareShedOrder,
        Nonnegotiable::ForwardOnlyMigrations,
    ];

    /// The §3.7 clause index (1..7) of this non-negotiable — used in the artifact rows so a red
    /// names the exact clause the divergent tier failed.
    pub fn clause(self) -> u8 {
        match self {
            Nonnegotiable::ThreeSurfaceTopology => 1,
            Nonnegotiable::LivenessNotReadiness => 2,
            Nonnegotiable::NoFireAndForgetEmit => 3,
            Nonnegotiable::PersonalDataHolderRegistration => 4,
            Nonnegotiable::ResilientClientRetryAfter => 5,
            Nonnegotiable::PrincipalAwareShedOrder => 6,
            Nonnegotiable::ForwardOnlyMigrations => 7,
        }
    }

    /// A short, stable label for the artifact row / signal name.
    pub fn label(self) -> &'static str {
        match self {
            Nonnegotiable::ThreeSurfaceTopology => "three-surface-topology",
            Nonnegotiable::LivenessNotReadiness => "liveness-not-readiness",
            Nonnegotiable::NoFireAndForgetEmit => "no-fire-and-forget-emit",
            Nonnegotiable::PersonalDataHolderRegistration => "personal-data-holder-registration",
            Nonnegotiable::ResilientClientRetryAfter => "resilient-client-retry-after",
            Nonnegotiable::PrincipalAwareShedOrder => "principal-aware-shed-order",
            Nonnegotiable::ForwardOnlyMigrations => "forward-only-migrations",
        }
    }
}

impl fmt::Display for Nonnegotiable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "§3.7.{} {}", self.clause(), self.label())
    }
}

/// The probe a divergent (non-Rust) tier's conformance harness implements — it answers, per
/// [`Nonnegotiable`], whether the divergent tier provides that non-negotiable **to the same
/// guarantee** the Rust harness does. This is a PROBE against the real divergent tier (the
/// BEAM/Phoenix connection tier under CHAT-P26), not a self-asserted boolean: each method is
/// expected to be backed by an actual conformance test against the running tier (e.g. kill a
/// critical dep and read that the tier's readiness probe flips, §3.7.2). The trait is the
/// frozen seam the divergent tier must satisfy; the harness owns the iteration + verdict.
pub trait DivergentTierProbe {
    /// The name of the divergent tier under probe (e.g. `"chat-beam-connection-tier"`) — used
    /// in artifact rows so a red names the tier.
    fn tier_name(&self) -> &str;

    /// Probe one non-negotiable: `true` iff the divergent tier provides it to the same
    /// guarantee the Rust harness does. The default is **deliberately absent** — the divergent
    /// tier must answer every clause explicitly (a forgotten clause is a compile error, not a
    /// silent pass).
    fn probe(&self, n: Nonnegotiable) -> bool;
}

/// The result of running the seven-non-negotiable conformance suite against a divergent tier.
/// `shim-conformance` is green **iff** [`ShimConformance::all_green`] — i.e. all seven clauses
/// passed. A red names the failing clauses; it is never silently swallowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShimConformance {
    tier_name: String,
    /// Per-clause pass/fail, in the frozen [`Nonnegotiable::ALL`] order.
    results: Vec<(Nonnegotiable, bool)>,
}

impl ShimConformance {
    /// Run the seven probes against a divergent tier and collect the verdict. Iterates the
    /// frozen [`Nonnegotiable::ALL`] set — every clause is probed (a missing clause is
    /// impossible: the array is exhaustive and the trait has no defaulted method).
    pub fn check<P: DivergentTierProbe + ?Sized>(probe: &P) -> ShimConformance {
        let results = Nonnegotiable::ALL
            .iter()
            .map(|&n| (n, probe.probe(n)))
            .collect();
        ShimConformance {
            tier_name: probe.tier_name().to_owned(),
            results,
        }
    }

    /// The tier this verdict is for.
    pub fn tier_name(&self) -> &str {
        &self.tier_name
    }

    /// The per-clause results, in the frozen §3.7.1..7 order.
    pub fn results(&self) -> &[(Nonnegotiable, bool)] {
        &self.results
    }

    /// The clauses that FAILED — empty iff the suite is fully green. A non-empty list is the
    /// loud red: it names exactly which §3.7 non-negotiables the divergent tier dropped.
    pub fn failures(&self) -> Vec<Nonnegotiable> {
        self.results
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(n, _)| *n)
            .collect()
    }

    /// `true` iff ALL seven non-negotiables passed — the `shim-conformance` ×7 GATE is green.
    pub fn all_green(&self) -> bool {
        self.results.len() == Nonnegotiable::ALL.len() && self.results.iter().all(|(_, ok)| *ok)
    }
}

/// **The loud record of how the cross-language shim was discharged for a subsystem.** There are
/// exactly TWO ways the shim obligation can be satisfied — and NO way for it to evaporate (EI-01
/// §5: a non-negotiable dropped at a boundary calcifies; §4: a skip must be named, never
/// silent):
///
/// - [`ShimEnforcement::Enforced`] — the subsystem diverged to a non-Rust tier and that tier
///   **passed the seven-non-negotiable conformance suite** (the `shim-conformance` ×7 GATE is
///   green). Produced by [`ShimEnforcement::enforce`].
/// - [`ShimEnforcement::RecordedNa`] — the subsystem stayed Rust, so there is **no
///   cross-language boundary** and the shim is a NO-OP — but it is recorded **LOUDLY** with the
///   pinned-language reason and a date (the dated N/A row), never silently skipped. Produced by
///   [`ShimEnforcement::recorded_na`].
///
/// A divergent tier that does NOT pass the suite yields neither variant — [`ShimEnforcement::enforce`]
/// returns `Err(ShimConformance)` carrying the failing clauses, which the gate reads RED. There
/// is no constructor that yields an `Enforced` from a failing suite, mirroring the scorecard's
/// "a green must be earned" ratchet ([`crate::scorecard`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the shim enforcement record must be asserted/recorded — an un-recorded shim is a silent skip (EI-01 §5)"]
pub enum ShimEnforcement {
    /// The divergent tier passed the seven-non-negotiable conformance suite. Carries the green
    /// verdict (all seven clauses passed).
    Enforced {
        /// The divergent tier that was enforced (e.g. the chat BEAM connection tier).
        tier_name: String,
        /// The fully-green conformance verdict (`shim-conformance` ×7).
        conformance: ShimConformance,
    },
    /// The subsystem stayed Rust — a loudly-recorded N/A. There is no cross-language boundary,
    /// so the shim is a NO-OP; this is the *recorded, dated* N/A row, NOT a silent skip.
    RecordedNa {
        /// The subsystem whose shim obligation is N/A (e.g. `"chat-connection-tier"`).
        subsystem: String,
        /// The pinned language that makes the shim a no-op (e.g. `"Rust (TE-21 pin; BEAM hatch closed)"`).
        pinned_language: String,
        /// The ISO date the N/A was recorded.
        recorded_on: String,
    },
}

impl ShimEnforcement {
    /// **Enforce the shim against a divergent tier.** Runs the seven-non-negotiable conformance
    /// suite; returns [`ShimEnforcement::Enforced`] iff ALL seven passed, else `Err` with the
    /// red [`ShimConformance`] naming the failing clauses (the gate reads RED — the tier cannot
    /// ship). This is the path taken iff Chat diverges (CHAT-P26).
    pub fn enforce<P: DivergentTierProbe + ?Sized>(
        probe: &P,
    ) -> Result<ShimEnforcement, ShimConformance> {
        let conformance = ShimConformance::check(probe);
        if conformance.all_green() {
            Ok(ShimEnforcement::Enforced {
                tier_name: conformance.tier_name().to_owned(),
                conformance,
            })
        } else {
            Err(conformance)
        }
    }

    /// **Record the shim as a loud N/A** — the subsystem stayed Rust, so there is no
    /// cross-language boundary and the shim is a NO-OP. This is NOT a silent skip: it carries
    /// the subsystem, the pinned language (the reason the boundary is absent), and the date.
    /// The `#[must_use]` on the type + the artifact row make the N/A *loud* (EI-01 §4/§5).
    pub fn recorded_na(
        subsystem: impl Into<String>,
        pinned_language: impl Into<String>,
        recorded_on: impl Into<String>,
    ) -> ShimEnforcement {
        ShimEnforcement::RecordedNa {
            subsystem: subsystem.into(),
            pinned_language: pinned_language.into(),
            recorded_on: recorded_on.into(),
        }
    }

    /// `true` iff this record is a loudly-recorded N/A (the subsystem stayed Rust).
    pub fn is_na(&self) -> bool {
        matches!(self, ShimEnforcement::RecordedNa { .. })
    }

    /// `true` iff this record is an enforced (green) divergent tier.
    pub fn is_enforced(&self) -> bool {
        matches!(self, ShimEnforcement::Enforced { .. })
    }

    /// The dated artifact row — the LOUD committed line the gate emits, whether enforced or
    /// N/A. An N/A is recorded as a `[shim-conformance N/A]` row naming the pinned language; an
    /// enforced tier as a `[shim-conformance ×7 GREEN]` row naming the tier. Mirrors the
    /// scorecard's `artifact_row` convention so the band-boundary gate can scrape it.
    pub fn artifact_row(&self) -> String {
        match self {
            ShimEnforcement::Enforced {
                tier_name,
                conformance: _,
            } => format!(
                "[shim-conformance ×7 GREEN] contract 1.7 — divergent tier `{tier_name}` satisfies all seven §3.7 non-negotiables"
            ),
            ShimEnforcement::RecordedNa {
                subsystem,
                pinned_language,
                recorded_on,
            } => format!(
                "[shim-conformance N/A] {recorded_on} contract 1.7 — `{subsystem}` stays Rust ({pinned_language}); no cross-language boundary, shim is a NO-OP (recorded loudly, NOT a silent skip — EI-01 §4/§5)"
            ),
        }
    }
}

#[cfg(test)]
mod tests;
