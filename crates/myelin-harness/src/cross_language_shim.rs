use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Nonnegotiable {
    ThreeSurfaceTopology,
    LivenessNotReadiness,
    NoFireAndForgetEmit,
    PersonalDataHolderRegistration,
    ResilientClientRetryAfter,
    PrincipalAwareShedOrder,
    ForwardOnlyMigrations,
}

impl Nonnegotiable {
    pub const ALL: [Nonnegotiable; 7] = [
        Nonnegotiable::ThreeSurfaceTopology,
        Nonnegotiable::LivenessNotReadiness,
        Nonnegotiable::NoFireAndForgetEmit,
        Nonnegotiable::PersonalDataHolderRegistration,
        Nonnegotiable::ResilientClientRetryAfter,
        Nonnegotiable::PrincipalAwareShedOrder,
        Nonnegotiable::ForwardOnlyMigrations,
    ];

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

pub trait DivergentTierProbe {
    fn tier_name(&self) -> &str;

    fn probe(&self, n: Nonnegotiable) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShimConformance {
    tier_name: String,
    results: Vec<(Nonnegotiable, bool)>,
}

impl ShimConformance {
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

    pub fn tier_name(&self) -> &str {
        &self.tier_name
    }

    pub fn results(&self) -> &[(Nonnegotiable, bool)] {
        &self.results
    }

    pub fn failures(&self) -> Vec<Nonnegotiable> {
        self.results
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(n, _)| *n)
            .collect()
    }

    pub fn all_green(&self) -> bool {
        self.results.len() == Nonnegotiable::ALL.len() && self.results.iter().all(|(_, ok)| *ok)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the shim enforcement record must be asserted/recorded - an un-recorded shim is a silent skip (EI-01 §5)"]
pub enum ShimEnforcement {
    Enforced {
        tier_name: String,
        conformance: ShimConformance,
    },
    RecordedNa {
        subsystem: String,
        pinned_language: String,
        recorded_on: String,
    },
}

impl ShimEnforcement {
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

    pub fn is_na(&self) -> bool {
        matches!(self, ShimEnforcement::RecordedNa { .. })
    }

    pub fn is_enforced(&self) -> bool {
        matches!(self, ShimEnforcement::Enforced { .. })
    }

    pub fn artifact_row(&self) -> String {
        match self {
            ShimEnforcement::Enforced {
                tier_name,
                conformance: _,
            } => format!(
                "[shim-conformance ×7 GREEN] contract 1.7 - divergent tier `{tier_name}` satisfies all seven §3.7 non-negotiables"
            ),
            ShimEnforcement::RecordedNa {
                subsystem,
                pinned_language,
                recorded_on,
            } => format!(
                "[shim-conformance N/A] {recorded_on} contract 1.7 - `{subsystem}` stays Rust ({pinned_language}); no cross-language boundary, shim is a NO-OP (recorded loudly, NOT a silent skip - EI-01 §4/§5)"
            ),
        }
    }
}

#[cfg(test)]
mod tests;
