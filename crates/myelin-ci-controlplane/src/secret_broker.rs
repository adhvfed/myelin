//! # The in-boundary secret broker — fork-gets-no-secrets (CI-P24 / P-367, M4, CI-D7).
//!
//! Architecture: `continuous-integration/architecture/02-internals-and-algorithms.md` §7.3 (secrets
//! resolved inside the boundary); `05-hard-problems.md` HP-8 (secrets); `01-tech-and-data-model.md`
//! §3.7 (`secret_binding`). Drill: CI-D7 (`01-whole-system-e2e-and-drill-catalogue.md`).
//!
//! ## The boundary rule (CI-1, non-negotiable)
//!
//! The [`myelin_ci_sandbox::JobSpec`] carries secret **NAMES** ([`SecretRef`]), never values. The
//! **in-boundary broker** resolves them **after the sandbox is up**, **scoped to exactly this job's
//! references** — it never enumerates the project's full secret set, only the names the spec already
//! declares. Resolution is gated on the **`read & !is_untrusted_fork`** ABAC edge (contract 4.9): an
//! `UntrustedFork` run resolves to **NO secrets by default** (the canonical "fork exfiltrates prod
//! secrets" CVE class — the poisoned-pipeline attack). A protected-environment secret additionally
//! requires an explicit `secret#direct_reader` grant on the run's subject (the DIRECT NARROW
//! non-inheritance, CI-1 §1 — a project reader does NOT inherit secret read).
//!
//! ## What this module OWNS vs CONSUMES (EI-01 §7 coherence — reconcile-in-place)
//!
//! - **CONSUMES** the FROZEN [`SecretRef`] shape (NAMES + opaque handle, never material) already on
//!   `JobSpec` (`myelin-ci-sandbox`), the FROZEN [`TrustTier`] 3-way enum (the SAME enum the
//!   dispatcher/sandbox stamp), and the FROZEN [`IdentityService::check`] gate (contract 4.2) over the
//!   already-declared `ci_secret` ReBAC namespace (`crate::rebac_fragment`). It builds NO second trust
//!   enum and NO second authz path.
//! - **OWNS** only the in-boundary resolution loop: the per-job scope filter + the fork-no-secrets
//!   structural gate + the OIDC short-lived audience-scoped credential mint over static keys (4.7).
//!
//! ## The two structural defences (both must hold — CI-D7)
//!
//! 1. **Structural (fork-tier):** a [`TrustTier::UntrustedFork`] job NEVER reaches the authz gate at
//!    all — [`SecretBroker::resolve`] short-circuits to an EMPTY resolution before any `check`. This
//!    is the `!is_untrusted_fork` arm by construction: even a misconfigured grant cannot leak a secret
//!    to a fork, because a fork never asks.
//! 2. **Authz (trusted-tier):** a [`TrustTier::Trusted`] / [`TrustTier::SelfHosted`] job resolves ONLY
//!    the names it references AND only those its subject can `read` via the DIRECT NARROW
//!    `secret#direct_reader` grant (contract 4.9). A trusted job with no grant on a protected secret
//!    resolves that one name to a WITHHOLD (not an error — the secret is simply absent), never a leak.
//!
//! ## MUTATION-SCORE FLOOR (mandatory-core — security-load-bearing, EI-01 §5)
//!
//! The secret broker is the in-boundary leak boundary; its `cargo-mutants` mutation-score floor is
//! **≥ 90% viable mutants caught**. The exhaustive resolution tests (`fork_resolves_to_zero_secrets`,
//! `trusted_resolves_only_referenced_names`, `protected_without_grant_withholds`, the CI-D7 drill) are
//! written to KILL every boundary/branch mutant: flipping the fork short-circuit, dropping the
//! `Decision::Allow` check, or resolving an un-referenced name all flip a pinned assertion. A `< 90%`
//! survivor count is a regression — the floor is never weakened to pass.

use myelin_ci_sandbox::{SecretRef, TrustTier};
use myelin_identity::{
    Consistency, Decision, IdentityService, Permission, Principal, Result as IdResult,
};
use myelin_tenancy::ArtifactRef;
use std::fmt;

/// **The READ permission on a `ci_secret` (the FROZEN `secret.read` gate, contract 4.9).** The ONLY
/// path to a secret is a DIRECT `secret#direct_reader@subject` grant resolving `read` (CI-1 §1 — NOT
/// inherited from `parent_ci_project`). The broker checks THIS permission per referenced name.
pub const SECRET_READ_PERMISSION: &str = "read";

/// **A resolved secret binding — the clear material the broker hands INTO the boundary (CI-1).** This
/// is the ONLY place a secret value materialises, and it lives only inside the boundary (this struct is
/// never serialised onto a `JobSpec`, an event, or a log — references-not-payloads, §3.4). The `value`
/// is the resolved material; the broker mints it from the shared secret capability scoped to this job.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedSecret {
    /// The env var name the resolved secret binds to inside the boundary (mirrors `SecretRef::name`).
    pub name: String,
    /// The resolved secret material (the clear value). NEVER leaves the boundary — not onto a spec,
    /// an event, or a log. The broker scopes it to THIS job only.
    pub value: String,
}

impl fmt::Debug for ResolvedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSecret")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

/// **Why a referenced secret was NOT resolved (the withhold reason — observable, never silent).** A
/// withheld secret is the boundary WORKING, not an error (EI-02 §4): the run simply does not get that
/// binding. The reason is a machine token (no PII) for the audit trail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WithholdReason {
    /// The run is an `UntrustedFork` — the structural fork-no-secrets gate (CI-D7). NO `check` is even
    /// attempted; the fork never reaches the authz path.
    UntrustedFork,
    /// The run is trusted but the subject lacks a DIRECT `secret#direct_reader` grant resolving
    /// `secret.read` (the DIRECT NARROW non-inheritance, CI-1 §1) — a protected secret without an
    /// explicit grant.
    NotGranted,
}

impl WithholdReason {
    /// The machine token for the audit trail (no PII).
    pub fn as_token(self) -> &'static str {
        match self {
            WithholdReason::UntrustedFork => "untrusted_fork",
            WithholdReason::NotGranted => "not_granted",
        }
    }
}

/// **The outcome of resolving ONE referenced secret name.** Either the material (resolved + scoped to
/// this job) or a withhold (the boundary held — observable, never a silent leak).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretOutcome {
    /// The name resolved to its material (a trusted run with the DIRECT grant).
    Resolved(ResolvedSecret),
    /// The name was WITHHELD (a fork run, or a trusted run without the grant) — carries the reason.
    Withheld {
        /// The env var name that was withheld (the run does not get this binding).
        name: String,
        /// Why it was withheld (a machine token — `untrusted_fork` | `not_granted`).
        reason: WithholdReason,
    },
}

impl SecretOutcome {
    /// The resolved material, if this outcome resolved (else `None` for a withhold).
    pub fn resolved(&self) -> Option<&ResolvedSecret> {
        match self {
            SecretOutcome::Resolved(r) => Some(r),
            SecretOutcome::Withheld { .. } => None,
        }
    }
}

/// **The complete resolution of a job's referenced secrets (the in-boundary broker output).** Carries
/// one [`SecretOutcome`] per referenced name (in the SAME order as the spec's `secret_refs`), so the
/// caller can tell exactly which names resolved and which were withheld (and why). A fork run yields
/// ALL withholds; the [`secret_count`](SecretResolution::secret_count) is the quantified CI-D7 gate
/// surface (`0` for a fork-tier run).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretResolution {
    /// One outcome per referenced name, in spec order.
    pub outcomes: Vec<SecretOutcome>,
}

impl SecretResolution {
    /// **The number of secrets that RESOLVED to material (the CI-D7 quantified gate).** For a
    /// fork-tier run this is `0` BY CONSTRUCTION (the structural fork short-circuit). The drill asserts
    /// `secret_count() == 0` for an adversarial fork.
    pub fn secret_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.resolved().is_some())
            .count()
    }

    /// The resolved bindings (the material the broker hands into the boundary) — withholds are absent.
    pub fn resolved(&self) -> impl Iterator<Item = &ResolvedSecret> {
        self.outcomes.iter().filter_map(SecretOutcome::resolved)
    }

    /// Did EVERY referenced name resolve (a fully-provisioned trusted run)?
    pub fn all_resolved(&self) -> bool {
        self.outcomes.iter().all(|o| o.resolved().is_some())
    }

    /// Did the broker resolve ZERO secrets (the fork-tier verdict surface)?
    pub fn is_empty(&self) -> bool {
        self.secret_count() == 0
    }
}

/// **The shared secret capability (CONSUMED — placed under Id/GDPR, arch §7.3).** Resolves an opaque
/// broker handle (the `SecretRef::handle` already on the spec) to its material, scoped to this job. The
/// broker NEVER enumerates a project's secrets through this surface — it resolves ONLY the handles the
/// spec already references (the per-job scope filter). Modeled as a trait so `myelin-ci-controlplane`
/// does NOT depend on the concrete secret store in production (the real store is the shared Id/GDPR
/// secret capability; the DAG stays acyclic — the broker owns only the scope+gate logic).
pub trait SecretCapability {
    /// Resolve ONE opaque handle to its material (scoped to this job). Returns `None` if the handle
    /// does not resolve (a stale/absent binding — withheld, never an error). The implementation mints
    /// the material from the EU-sovereign secret store; OIDC short-lived credentials are minted by
    /// [`SecretBroker::mint_oidc`] over the resolved binding, NOT here.
    fn resolve_handle(&self, handle: &str) -> Option<String>;
}

/// **An OIDC short-lived audience-scoped federated credential (contract 4.7, arch §7.3).** CI mints
/// these OVER static keys for talking to a registry / cloud target — a strong EU-sovereign
/// least-privilege fit (the credential's life == the job's life, scoped to ONE audience). The token
/// material is opaque; it never leaves the boundary.
#[derive(Clone, PartialEq, Eq)]
pub struct OidcCredential {
    /// The audience the credential is scoped to (a registry / cloud target — least-privilege, 4.7).
    pub audience: String,
    /// The short-lived token material (opaque; never logged / serialised onto a spec or event).
    pub token: String,
    /// The credential's lifetime in seconds (short-lived == the job's life, NOT a long-lived key).
    pub ttl_secs: u32,
}

impl fmt::Debug for OidcCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OidcCredential")
            .field("audience", &self.audience)
            .field("token", &"[REDACTED]")
            .field("ttl_secs", &self.ttl_secs)
            .finish()
    }
}

/// **The in-boundary secret broker (CI-1; arch §7.3).** Resolves a job's referenced secrets AFTER the
/// sandbox is up, scoped to exactly the job's references, gated on `read & !is_untrusted_fork`. A
/// fork-tier job resolves to ZERO secrets (the structural short-circuit — CI-D7).
///
/// The broker is generic over the consumed FROZEN surfaces (the secret capability + the
/// [`IdentityService`] authz gate) so it depends on the contract surfaces, never a concrete store —
/// the production DAG stays acyclic (a leaf consumer).
pub struct SecretBroker<'a, C: SecretCapability, I: IdentityService> {
    cap: &'a C,
    identity: &'a I,
}

impl<'a, C: SecretCapability, I: IdentityService> SecretBroker<'a, C, I> {
    /// Construct the broker over the shared secret capability + the Identity authz gate.
    pub fn new(cap: &'a C, identity: &'a I) -> Self {
        SecretBroker { cap, identity }
    }

    /// **Resolve a job's referenced secrets in-boundary (the CI-1 boundary, arch §7.3).**
    ///
    /// ## The fork short-circuit (the structural defence — CI-D7, defence #1)
    ///
    /// If `tier == UntrustedFork`, this returns ALL withholds ([`WithholdReason::UntrustedFork`])
    /// WITHOUT even attempting an authz `check` — the fork never reaches the authz path. This is the
    /// `!is_untrusted_fork` arm BY CONSTRUCTION: a misconfigured grant cannot leak a secret to a fork,
    /// because a fork never asks. `secret_count() == 0` holds for EVERY fork-tier resolution.
    ///
    /// ## The authz gate (the trusted-tier defence — CI-D7, defence #2)
    ///
    /// For a `Trusted` / `SelfHosted` run, each referenced name's secret object is gated on
    /// `IdentityService::check(subject, "read", secret_object, at)` — the DIRECT NARROW
    /// `secret#direct_reader` grant (CI-1 §1, contract 4.9). Only a `Decision::Allow` resolves the
    /// material; a `Deny` / `Conditional` WITHHOLDS the name ([`WithholdReason::NotGranted`]) — never a
    /// leak. A handle that does not resolve (a stale binding) is also withheld (observable, never an
    /// error — EI-02 §4).
    ///
    /// The scope is EXACTLY the spec's `secret_refs` — the broker never enumerates the project's full
    /// secret set (the per-job scope filter, arch §7.3).
    pub fn resolve(
        &self,
        tier: TrustTier,
        subject: &Principal,
        secret_object_of: impl Fn(&SecretRef) -> ArtifactRef,
        secret_refs: &[SecretRef],
        at: &Consistency,
    ) -> IdResult<SecretResolution> {
        // DEFENCE #1 — the structural fork short-circuit (the `!is_untrusted_fork` arm by
        // construction). A fork NEVER reaches the authz `check`: it resolves to ALL withholds, so
        // `secret_count() == 0` BY CONSTRUCTION even with a misconfigured grant (CI-D7).
        if tier == TrustTier::UntrustedFork {
            return Ok(SecretResolution {
                outcomes: secret_refs
                    .iter()
                    .map(|r| SecretOutcome::Withheld {
                        name: r.name.clone(),
                        reason: WithholdReason::UntrustedFork,
                    })
                    .collect(),
            });
        }

        // DEFENCE #2 — the authz gate, per referenced name (the DIRECT NARROW `secret.read`, 4.9).
        // The scope is EXACTLY `secret_refs` (the per-job filter — never the project's full set).
        let mut outcomes = Vec::with_capacity(secret_refs.len());
        for sref in secret_refs {
            let object = secret_object_of(sref);
            let decision = self.identity.check(
                subject,
                &Permission(SECRET_READ_PERMISSION.to_string()),
                &object,
                at,
                None,
            )?;
            // Only an explicit Allow (the DIRECT `secret#direct_reader` grant) resolves the material;
            // a Deny / Conditional WITHHOLDS (never a leak) — fail-closed (ADR-03).
            if decision == Decision::Allow {
                match self.cap.resolve_handle(&sref.handle) {
                    Some(value) => outcomes.push(SecretOutcome::Resolved(ResolvedSecret {
                        name: sref.name.clone(),
                        value,
                    })),
                    // A granted name whose handle does not resolve (a stale binding) is WITHHELD —
                    // observable, never a panic / silent leak (EI-02 §4).
                    None => outcomes.push(SecretOutcome::Withheld {
                        name: sref.name.clone(),
                        reason: WithholdReason::NotGranted,
                    }),
                }
            } else {
                outcomes.push(SecretOutcome::Withheld {
                    name: sref.name.clone(),
                    reason: WithholdReason::NotGranted,
                });
            }
        }
        Ok(SecretResolution { outcomes })
    }

    /// **Mint an OIDC short-lived audience-scoped federated credential (contract 4.7, arch §7.3).** CI
    /// mints these OVER static keys for a registry / cloud target — the credential's life == the job's
    /// life, scoped to ONE audience (least-privilege, EU-sovereign). A fork-tier job is REFUSED a
    /// credential (the same fork-no-secrets boundary: a fork gets no audience-scoped cloud access) —
    /// returns `None`.
    pub fn mint_oidc(
        &self,
        tier: TrustTier,
        audience: &str,
        ttl_secs: u32,
        mint: impl FnOnce(&str, u32) -> Option<String>,
    ) -> Option<OidcCredential> {
        // A fork gets no audience-scoped cloud credential (the same boundary as secret resolution).
        if tier == TrustTier::UntrustedFork {
            return None;
        }
        mint(audience, ttl_secs).map(|token| OidcCredential {
            audience: audience.to_string(),
            token,
            ttl_secs,
        })
    }
}

#[cfg(test)]
#[path = "secret_broker_tests.rs"]
mod tests;
