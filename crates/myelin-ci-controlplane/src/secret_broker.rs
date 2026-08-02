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

use myelin_ci_sandbox::{JobSpec, ResolvedSecretEnv, SecretInjectionError, SecretRef, TrustTier};
use myelin_identity::{
    Consistency, Decision, IdentityService, Permission, Principal, Result as IdResult,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::fmt;
use zeroize::Zeroize;

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

impl Drop for ResolvedSecret {
    fn drop(&mut self) {
        self.value.zeroize();
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
    /// No secret-store capability is composed in this cell. This is a terminal launch refusal, not
    /// an absent-spec condition for the lease reaper to retry forever.
    CapabilityUnavailable,
}

impl WithholdReason {
    /// The machine token for the audit trail (no PII).
    pub fn as_token(self) -> &'static str {
        match self {
            WithholdReason::UntrustedFork => "untrusted_fork",
            WithholdReason::NotGranted => "not_granted",
            WithholdReason::CapabilityUnavailable => "capability_unavailable",
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
        /// Why it was withheld (a machine token — `untrusted_fork` | `not_granted` |
        /// `capability_unavailable`).
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

/// One visible, material-free reason a launch was refused by the all-or-nothing secret policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithheldSecret {
    pub name: String,
    pub reason: WithholdReason,
}

/// A fail-closed refusal while composing the ephemeral, secret-bearing sandbox launch spec.
#[derive(Debug)]
pub enum SecretLaunchError {
    /// The ReBAC check itself failed; no launch spec was produced.
    Authorization(myelin_identity::AuthzError),
    /// At least one declared secret was withheld. Policy is all-or-nothing: the workload is not
    /// launched with a surprising partial environment, and every reason remains observable.
    Withheld(Vec<WithheldSecret>),
    /// The sandbox rejected the binding-set or env↔needle coupling.
    Injection(SecretInjectionError),
}

impl fmt::Display for SecretLaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authorization(_) => formatter.write_str("secret launch authorization failed"),
            Self::Withheld(withheld) => {
                formatter.write_str("secret launch withheld: ")?;
                for (index, item) in withheld.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(",")?;
                    }
                    write!(formatter, "{}={}", item.name, item.reason.as_token())?;
                }
                Ok(())
            }
            Self::Injection(error) => write!(formatter, "secret launch injection refused: {error}"),
        }
    }
}

impl std::error::Error for SecretLaunchError {}

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
    /// Resolve ONE tenant-scoped handle to its material, bound to the exact object Identity authorized.
    /// Implementations MUST refuse unless `handle` belongs to `tenant` and identifies `object`; the
    /// redundant inputs deliberately prevent a global handle lookup from becoming a confused deputy.
    /// Returns `None` for a stale, absent, or scope-mismatched binding.
    fn resolve_handle(
        &self,
        tenant: &TenantId,
        object: &ArtifactRef,
        handle: &str,
    ) -> Option<String>;
}

const MAX_SECRET_HANDLE_SEGMENT_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CanonicalSecretHandle<'a> {
    pub tenant: &'a str,
    pub id: &'a str,
}

/// Parse the only secret-handle spelling accepted at either the manifest or broker boundary.
///
/// Both variable segments use the deliberately smaller grammar `[A-Za-z0-9_-]{1,128}`. In
/// particular, `.`/`:`/`%` and `/` are impossible, so the reference cannot acquire traversal,
/// encoding, anchor, or type-prefix semantics in a downstream parser. The final `object_key` checks
/// pin the exact authz identity to `secret:<id>` and reject any future normalization drift.
pub(crate) fn parse_canonical_secret_handle(handle: &str) -> Option<CanonicalSecretHandle<'_>> {
    let rest = handle.strip_prefix("myelin://")?;
    let mut segments = rest.split('/');
    let tenant = segments.next()?;
    let subsystem = segments.next()?;
    let object_type = segments.next()?;
    let id = segments.next()?;
    if segments.next().is_some()
        || subsystem != "ci"
        || object_type != "secret"
        || !strict_secret_segment(tenant)
        || !strict_secret_segment(id)
    {
        return None;
    }

    let canonical = format!("myelin://{tenant}/ci/secret/{id}");
    if handle != canonical {
        return None;
    }
    let key = myelin_refs::object_key(&ArtifactRef(canonical))?;
    if key.tenant.as_deref() != Some(tenant)
        || key.subsystem.as_deref() != Some("ci")
        || key.object_type.as_deref() != Some("secret")
        || key.id != id
        || key.tuple_key() != format!("secret:{id}")
    {
        return None;
    }

    Some(CanonicalSecretHandle { tenant, id })
}

fn strict_secret_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= MAX_SECRET_HANDLE_SEGMENT_BYTES
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn tenant_bound_secret_object(
    tenant: &TenantId,
    sref: &SecretRef,
    secret_object_of: &impl Fn(&SecretRef) -> ArtifactRef,
) -> Option<ArtifactRef> {
    let parsed = parse_canonical_secret_handle(&sref.handle)?;
    if parsed.tenant != tenant.0 {
        return None;
    }
    let object = secret_object_of(sref);
    let authorized = parse_canonical_secret_handle(&object.0)?;
    (authorized == parsed && object.0 == sref.handle).then_some(object)
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
            let Some(object) = tenant_bound_secret_object(&subject.tenant, sref, &secret_object_of)
            else {
                outcomes.push(SecretOutcome::Withheld {
                    name: sref.name.clone(),
                    reason: WithholdReason::NotGranted,
                });
                continue;
            };
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
                match self
                    .cap
                    .resolve_handle(&subject.tenant, &object, &sref.handle)
                {
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

    /// Resolve and attach a job's secret material at the ephemeral launch-composition boundary.
    ///
    /// This is the single control-plane bridge from opaque `SecretRef` handles to the sandbox's
    /// inseparable [`myelin_ci_sandbox::ResolvedJobSecrets`] value. The launch policy is deliberately
    /// all-or-nothing: any withhold rejects the launch with its material-free machine reason rather
    /// than silently deleting an env entry. On success, [`JobSpec::with_resolved_secrets`] derives
    /// both OCI env and the redaction plan from the same bindings and checks exact coverage.
    pub fn resolve_for_launch(
        &self,
        spec: JobSpec,
        subject: &Principal,
        secret_object_of: impl Fn(&SecretRef) -> ArtifactRef,
        at: &Consistency,
    ) -> Result<JobSpec, SecretLaunchError> {
        let resolution = self
            .resolve(
                spec.trust_tier,
                subject,
                secret_object_of,
                &spec.secret_refs,
                at,
            )
            .map_err(SecretLaunchError::Authorization)?;

        let withheld: Vec<WithheldSecret> = resolution
            .outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                SecretOutcome::Resolved(_) => None,
                SecretOutcome::Withheld { name, reason } => Some(WithheldSecret {
                    name: name.clone(),
                    reason: *reason,
                }),
            })
            .collect();
        if !withheld.is_empty() {
            return Err(SecretLaunchError::Withheld(withheld));
        }

        let bindings = resolution
            .resolved()
            .map(|secret| ResolvedSecretEnv::new(secret.name.clone(), secret.value.clone()))
            .collect();
        spec.with_resolved_secrets(bindings)
            .map_err(SecretLaunchError::Injection)
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
