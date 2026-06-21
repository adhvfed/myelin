//! # `self_hosted` — the self-hosted runner attestation gate + the tenant-`SelfHosted`-scoped
//! token mint (CI-P4 → P-240, M2)
//!
//! **Owning architecture docs (read in full before changing):**
//! - `planning/04-subsystem-architectures/continuous-integration/architecture/01-tech-and-data-model.md`
//!   §3.4 (the `runner` table — `ownership ∈ {hosted, self_hosted}`, `attestation jsonb` = TPM
//!   quote / provisioning-signed token, `attest_state ∈ {pending, attested, failed}`,
//!   `trust_tier` — "hosted runners claim trusted/untrusted; self-hosted only its own tenant's
//!   SelfHosted jobs").
//! - `.../02-internals-and-algorithms.md` §5.4 (pre-warmed snapshot pools — the SIBLING module
//!   [`crate::snapshot_pool`]).
//! - `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` X-6 / §1
//!   (the self-hosted-runner trust boundary; the scoped token): "a self-hosted runner token is
//!   scoped to **one tenant's `SelfHosted` jobs** (cannot mint cross-tenant)".
//!
//! **Contracts CONSUMED:** 4.7 (`mint_run_token` — the self-hosted runner token scoped to one
//! tenant's `SelfHosted` jobs), 12.4 (`residency_verify` — the runner pool region; a self-hosted
//! runner is pinned to its tenant's region, no cross-region placement).
//!
//! ## Two security-load-bearing properties this module ships
//!
//! 1. **The self-hosted attestation gate (fail-closed).** A self-hosted runner MUST present a valid
//!    attestation (a TPM quote / provisioning-signed token) before it can claim a `SelfHosted`-tier
//!    job. The [`AttestState`] machine is `pending → attested → failed`; only an `attested` runner
//!    may claim ([`SelfHostedRunner::may_claim`]). Absent OR forged attestation ⇒ `failed` ⇒ **no
//!    claim** — fail-closed (an unattested runner can claim ZERO jobs; the gate signal = 0 claims).
//!
//! 2. **The tenant-`SelfHosted`-scoped token mint (4.7).** An attested self-hosted runner receives a
//!    run token whose authority names ONLY its own tenant's `SelfHosted` scope
//!    (`selfhosted:<tenant>`), minted through the SAME [`myelin_flow::RunTokenMinter`] surface
//!    (contract 4.7) the agent fabric (AG-P13) and the workflow engine consume — NOT a fork. A token
//!    minted for tenant A can never claim tenant B's job ([`TenantScopedToken::admits`] refuses the
//!    cross-tenant case).
//!
//! ## How this REUSES the existing token surface (no fork)
//! The minting flows through the contract-4.7 consumer seam `myelin_flow::RunTokenMinter`
//! (`mint_run_token(agent_id, run_id, caveats, ttl) → RunTokenHandle`) — the EXACT trait the
//! Identity service's `RunTokenMinter` (P-076, `myelin-identity-service::mint`) provides and the
//! agent-service consumes (`myelin-agent-service::identity`). The CI self-hosted scope is expressed
//! as a delegation caveat `selfhosted:<tenant>` carried into that mint — the same one-tenant ceiling
//! `myelin_identity_service::mint::SELFHOSTED_GRANT_PREFIX` enforces at the Identity layer (one
//! ceiling convention). The runner then CONSUMES the minted token off `JobSpec.run_token`
//! ([`crate::RunnerAgent`], CI-P3). We do not re-implement the token type, the minter, or the
//! delegation algebra.
//!
//! ## MUTATION-SCORE FLOOR (mandatory-core, security-load-bearing)
//! Both the attestation gate ([`SelfHostedRunner::may_claim`] / [`AttestState`] transitions) and the
//! tenant-scope check ([`TenantScopedToken::admits`] / [`mint_self_hosted_token`]) are
//! **mandatory-core, security-load-bearing**: a surviving mutant here is either an unattested runner
//! that CAN claim (the attestation bypass) or a token for tenant A that admits tenant B's job (the
//! cross-tenant escape). Their cargo-mutants mutation-score floor is **100% (zero surviving
//! mutants)** — the same floor the Identity mint's self-hosted-scope re-check carries (P-076), since
//! this module is the CI-side gate over that exact ceiling.
//!
//! ## FLOORS named (CI-P4)
//! - The fixed pre-warm buffer → the MEASURED buffer-sizing function (open question 07#2) is **CI-P23
//!   (CI-M5)** — named in [`crate::snapshot_pool`].
//! - The fleet-side autoscale that PROVISIONS self-hosted runners is **CI-P10**; here it is the
//!   attestation + scoped-mint that GATES a self-hosted runner from claiming (not the provisioning).
//! - The REAL TPM-quote / provisioning-signature CRYPTO verification is the named EI-01 §1 structural
//!   seam ([`AttestationVerifier`]) — the floor verifier checks the provisioning-signed token's
//!   structural envelope; the real TPM attestation verifier swaps in behind the SAME trait (exactly
//!   as `myelin_identity_service::mint::TokenSigner` names its crypto floor). State this in writing.

use crate::TrustTier;
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use myelin_tenancy::{Region, TenantId};

/// The grant prefix a self-hosted-runner run token's authority is ceiling-bounded to
/// (`"selfhosted:<tenant>"`) — the SAME prefix the Identity mint
/// (`myelin_identity_service::mint::SELFHOSTED_GRANT_PREFIX`) enforces at the token layer (one
/// ceiling convention, recon §1 / EI-01 §7). A self-hosted run token may name ONLY its OWN tenant's
/// `SelfHosted` scope (architecture §3.4, C6).
pub const SELFHOSTED_GRANT_PREFIX: &str = "selfhosted:";

/// The own-tenant `SelfHosted` grant for `tenant` (`selfhosted:<tenant>`) — the single caveat a
/// tenant-scoped self-hosted run token carries. A cross-tenant grant (a different tenant slug) is
/// never minted into a self-hosted token (the no-global-pool property at the CI layer).
pub fn self_hosted_grant(tenant: &TenantId) -> String {
    format!("{SELFHOSTED_GRANT_PREFIX}{}", tenant.0)
}

// =================================================================================================
// The self-hosted attestation gate — `pending → attested → failed`, fail-closed (arch §3.4).
// =================================================================================================

/// **The `runner.attest_state` machine (architecture §3.4): `pending → attested → failed`.** A
/// self-hosted runner is born `Pending`; it transitions to `Attested` ONLY when it presents a valid
/// attestation (a TPM quote / provisioning-signed token the [`AttestationVerifier`] accepts), and to
/// `Failed` on an absent or forged attestation. **Only `Attested` admits a claim** — `Pending` and
/// `Failed` are both fail-closed (no claim). The state is one-way for the failure case: a `Failed`
/// runner does not silently become attestable; it must re-present (a fresh [`SelfHostedRunner`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttestState {
    /// The runner has registered but not yet presented a (valid) attestation — fail-closed (no
    /// claim). The default/initial state.
    Pending,
    /// The runner presented a valid attestation (TPM quote / provisioning-signed token verified) —
    /// the ONLY state that admits a `SelfHosted`-tier claim, and only for its own tenant.
    Attested,
    /// The attestation was absent or forged (the verifier rejected it) — fail-closed (no claim).
    Failed,
}

impl AttestState {
    /// True iff this state admits a claim — `Attested` only. `Pending` and `Failed` are fail-closed.
    pub fn admits_claim(self) -> bool {
        matches!(self, AttestState::Attested)
    }
}

/// **A self-hosted runner's presented attestation (architecture §3.4 `attestation jsonb`).** Either
/// a TPM quote or a provisioning-signed token (the two forms the runner table names). The opaque
/// bearer material is verified by the [`AttestationVerifier`] — the floor verifier checks the
/// provisioning-signed token's structural envelope; the real TPM-quote verifier swaps in behind the
/// SAME seam (the named EI-01 §1 crypto floor). The material is PII-free (an opaque attestation
/// blob, never subject data).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attestation {
    /// The tenant this self-hosted runner attests it belongs to — the attestation BINDS the runner
    /// to one tenant (the scoped-token tenant). A verifier rejects an attestation whose bound tenant
    /// does not match the runner's claimed tenant (the cross-tenant attestation forgery).
    pub tenant: TenantId,
    /// The opaque attestation material (a TPM quote / provisioning-signed token). The floor verifier
    /// parses its structural envelope; the real crypto verifier swaps in behind [`AttestationVerifier`].
    pub material: String,
}

/// **The pluggable attestation VERIFIER seam (the named EI-01 §1 crypto floor — the CI counterpart
/// of `myelin_identity_service::mint::TokenSigner`).** A verifier turns a presented [`Attestation`]
/// into a yes/no on whether the runner is genuinely the provisioned, tenant-bound host it claims to
/// be. The REAL TPM-quote verifier (PCR-policy + nonce-freshness + AK-cert-chain) / the
/// provisioning-signature verifier implements this and swaps in behind the SAME seam; the gate's
/// `pending → attested → failed` logic does not change. The floor implementation is
/// [`StructuralAttestationVerifier`].
pub trait AttestationVerifier {
    /// Verify a presented attestation for a runner that claims to belong to `claimed_tenant`. Returns
    /// `true` iff the attestation is valid AND binds the runner to `claimed_tenant` (an attestation
    /// for another tenant, or one with a forged/absent envelope, returns `false` ⇒ the runner fails
    /// the gate fail-closed).
    fn verify(&self, attestation: &Attestation, claimed_tenant: &TenantId) -> bool;
}

/// **The floor attestation verifier (the EI-01 §1 documented deviation).** Accepts a
/// provisioning-signed-token attestation whose structural envelope is well-formed AND whose bound
/// tenant matches the runner's claimed tenant; rejects an absent (empty), malformed, or
/// cross-tenant-bound attestation. The envelope is the structural form
/// `provsig:<tenant>:<nonce>` — a non-empty signature over the tenant + a freshness nonce; the real
/// TPM/PASETO verifier (the named P5/P6 floor) checks the real crypto behind the SAME seam.
///
/// This proves the GATE's authorization-relevant path (valid admits, absent/forged refused) without
/// pretending to do TPM crypto — exactly as `StructuralTokenSigner` proves the mint path.
#[derive(Clone, Copy, Debug, Default)]
pub struct StructuralAttestationVerifier;

impl StructuralAttestationVerifier {
    /// The structural envelope prefix a provisioning-signed-token attestation carries.
    pub const ENVELOPE_PREFIX: &'static str = "provsig:";

    /// A fresh floor verifier.
    pub fn new() -> StructuralAttestationVerifier {
        StructuralAttestationVerifier
    }

    /// Build the structural floor attestation material for `tenant` with freshness `nonce`
    /// (`provsig:<tenant>:<nonce>`) — the form a genuinely-provisioned runner presents. (Test/dev
    /// helper; a real runner's provisioning agent emits the real TPM quote / signed token.)
    pub fn provisioned_material(tenant: &TenantId, nonce: &str) -> String {
        format!("{}{}:{}", Self::ENVELOPE_PREFIX, tenant.0, nonce)
    }
}

impl AttestationVerifier for StructuralAttestationVerifier {
    fn verify(&self, attestation: &Attestation, claimed_tenant: &TenantId) -> bool {
        // (1) The attestation must BIND to the tenant the runner claims (an attestation minted for
        //     another tenant is a cross-tenant forgery — refused).
        if &attestation.tenant != claimed_tenant {
            return false;
        }
        // (2) The material must carry the well-formed provisioning-signed envelope
        //     `provsig:<tenant>:<nonce>` with a NON-empty nonce and the tenant segment matching the
        //     bound tenant. An absent (empty) or malformed envelope is forged/absent ⇒ refused.
        let Some(rest) = attestation
            .material
            .strip_prefix(Self::ENVELOPE_PREFIX)
        else {
            return false; // absent / wrong-scheme envelope ⇒ forged.
        };
        let Some((tenant_seg, nonce)) = rest.split_once(':') else {
            return false; // malformed (no nonce segment) ⇒ forged.
        };
        // The signature's own tenant segment must match the bound tenant (a token signed for another
        // tenant but presented under this tenant's binding is a forgery), and the nonce must be present.
        tenant_seg == claimed_tenant.0 && !nonce.is_empty()
    }
}

/// **A self-hosted runner at the trust boundary (architecture §3.4 `runner` row, the self-hosted
/// view).** Holds its bound tenant + region (residency — a self-hosted runner is pinned to its
/// tenant's region, no cross-region placement, 12.4), its `attest_state`, and the attestation it
/// presented. The gate is [`SelfHostedRunner::may_claim`]: an `Attested` runner may claim a job iff
/// the job is its own tenant's `SelfHosted`-tier job — `Pending`/`Failed` claim nothing (fail-closed).
#[derive(Clone, Debug)]
pub struct SelfHostedRunner {
    /// The tenant this self-hosted runner belongs to — it may claim ONLY this tenant's `SelfHosted`
    /// jobs (the one-tenant scope; cross-tenant is refused).
    tenant: TenantId,
    /// The region the runner is pinned to (residency — no cross-region claim, 12.4).
    region: Region,
    /// The attestation state machine (`pending → attested → failed`). Born `Pending`; only `Attested`
    /// admits a claim.
    attest_state: AttestState,
}

impl SelfHostedRunner {
    /// Register a self-hosted runner for `tenant` in `region`, born `Pending` (it has NOT yet
    /// attested — fail-closed until it presents a valid attestation). The default initial state of
    /// the `runner` row.
    pub fn register(tenant: TenantId, region: Region) -> SelfHostedRunner {
        SelfHostedRunner {
            tenant,
            region,
            attest_state: AttestState::Pending,
        }
    }

    /// The runner's bound tenant (it may claim ONLY this tenant's `SelfHosted` jobs).
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// The runner's pinned region (residency — no cross-region claim).
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// The runner's current `attest_state`.
    pub fn attest_state(&self) -> AttestState {
        self.attest_state
    }

    /// **Present an attestation through the `verifier` — transition `pending → attested` on a valid
    /// attestation, `pending → failed` on an absent/forged one (architecture §3.4).** Fail-closed:
    /// the runner moves to `Attested` ONLY when the verifier accepts the attestation AND it binds to
    /// this runner's tenant; otherwise it moves to `Failed`. Returns the resulting [`AttestState`].
    pub fn attest(
        &mut self,
        attestation: &Attestation,
        verifier: &dyn AttestationVerifier,
    ) -> AttestState {
        self.attest_state = if verifier.verify(attestation, &self.tenant) {
            AttestState::Attested
        } else {
            AttestState::Failed
        };
        self.attest_state
    }

    /// **THE GATE (architecture §3.4, fail-closed): may this runner claim `job_tier` for
    /// `job_tenant` in `job_region`?** Admits a claim iff ALL hold:
    /// 1. the runner is `Attested` (an un-attested `Pending`/`Failed` runner claims NOTHING — the
    ///    fail-closed attestation gate; 0 claims by an unattested runner);
    /// 2. the job is `SelfHosted`-tier (a self-hosted runner serves only self-hosted jobs);
    /// 3. the job is THIS runner's own tenant's job (cross-tenant is refused — the one-tenant scope);
    /// 4. the job is in THIS runner's region (residency — no cross-region claim, 12.4).
    ///
    /// Any failed clause ⇒ no claim (fail-closed). This is the CLAIM-eligibility gate the
    /// scheduler/runner consults BEFORE handing a self-hosted job to a self-hosted runner.
    pub fn may_claim(
        &self,
        job_tier: TrustTier,
        job_tenant: &TenantId,
        job_region: &Region,
    ) -> bool {
        self.attest_state.admits_claim()
            && job_tier == TrustTier::SelfHosted
            && job_tenant == &self.tenant
            && job_region == &self.region
    }
}

// =================================================================================================
// The tenant-`SelfHosted`-scoped token mint (4.7) — REUSES `myelin_flow::RunTokenMinter` (no fork).
// =================================================================================================

/// **A tenant-`SelfHosted`-scoped run token (contract 4.7, the CI self-hosted scope).** Wraps the
/// minted [`RunTokenHandle`] (the bearer material + `jti` + TTL — the EXACT token type the Identity
/// mint produces and the runner consumes) with the ONE tenant whose `SelfHosted` jobs it admits.
/// [`TenantScopedToken::admits`] is the cross-tenant refusal: a token minted for tenant A can never
/// claim tenant B's job.
#[derive(Clone, Debug)]
pub struct TenantScopedToken {
    /// The tenant whose `SelfHosted` jobs this token admits — the ONLY tenant (the one-tenant scope).
    tenant: TenantId,
    /// The minted per-run token (the contract-4.7 `RunTokenHandle`: bearer material + `jti` + TTL).
    handle: RunTokenHandle,
}

impl TenantScopedToken {
    /// The tenant this token is scoped to (it admits ONLY this tenant's `SelfHosted` jobs).
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// The underlying minted [`RunTokenHandle`] (the bearer material + `jti` + TTL the runner stamps
    /// onto `JobSpec.run_token` and consumes — the SAME token type, not a fork).
    pub fn handle(&self) -> &RunTokenHandle {
        &self.handle
    }

    /// **THE CROSS-TENANT REFUSAL (recon §1, the no-global-pool property): does this token admit a
    /// `SelfHosted` job for `job_tenant` in `job_tier`?** Admits iff the job is `SelfHosted`-tier AND
    /// `job_tenant` is EXACTLY this token's scoped tenant. A token for tenant A presented against
    /// tenant B's job is REFUSED — a self-hosted runner can never run another tenant's jobs.
    pub fn admits(&self, job_tier: TrustTier, job_tenant: &TenantId) -> bool {
        job_tier == TrustTier::SelfHosted && job_tenant == &self.tenant
    }
}

/// **An error minting a tenant-scoped self-hosted token.** A mint refuses LOUDLY (never a fabricated
/// or cross-tenant token).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelfHostedMintError {
    /// The runner is not `Attested` — an un-attested runner cannot receive a token (the fail-closed
    /// attestation gate at the mint). Carries the runner's `attest_state` for the audit.
    NotAttested(AttestState),
    /// The underlying contract-4.7 mint failed (Identity unavailable / refused). The machine error
    /// string (no subject data) is carried, surfaced loud.
    MintFailed(RunTokenError),
}

impl core::fmt::Display for SelfHostedMintError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SelfHostedMintError::NotAttested(s) => write!(
                f,
                "a self-hosted run token cannot be minted for a runner in attest_state {s:?} — only \
                 an Attested runner receives a tenant-scoped token (fail-closed)"
            ),
            SelfHostedMintError::MintFailed(e) => {
                write!(f, "the contract-4.7 self-hosted token mint failed: {e}")
            }
        }
    }
}

impl std::error::Error for SelfHostedMintError {}

/// **Mint a tenant-`SelfHosted`-scoped run token for an attested self-hosted `runner` running
/// `run_id` (contract 4.7 — REUSING `myelin_flow::RunTokenMinter`, no fork).**
///
/// The mint is fail-closed on the attestation gate FIRST: a runner that is not `Attested` receives
/// NO token ([`SelfHostedMintError::NotAttested`]) — the attestation gate and the mint are one gate.
/// For an attested runner, the token's authority caveat chain carries EXACTLY the runner's own
/// tenant's `SelfHosted` grant (`selfhosted:<tenant>`) — the SAME one-tenant ceiling the Identity
/// mint (`SELFHOSTED_GRANT_PREFIX`) enforces — so the minted token can never name another tenant's
/// scope. The result wraps the minted [`RunTokenHandle`] in a [`TenantScopedToken`] whose `admits`
/// refuses cross-tenant claims.
///
/// `minter` is the contract-4.7 surface (the Identity provider behind the trait); `agent_id` is the
/// self-hosted runner's machine principal id; `ttl_secs` is the run-life bound (life == run life).
pub fn mint_self_hosted_token(
    runner: &SelfHostedRunner,
    minter: &dyn RunTokenMinter,
    agent_id: &str,
    run_id: &str,
    ttl_secs: u64,
) -> Result<TenantScopedToken, SelfHostedMintError> {
    // (1) THE ATTESTATION GATE AT THE MINT (fail-closed): an un-attested runner receives no token.
    if !runner.attest_state.admits_claim() {
        return Err(SelfHostedMintError::NotAttested(runner.attest_state));
    }

    // (2) THE ONE-TENANT SCOPE: the token's authority caveat chain carries ONLY the runner's own
    //     tenant's SelfHosted grant (`selfhosted:<tenant>`) — the SAME ceiling the Identity mint
    //     enforces (SELFHOSTED_GRANT_PREFIX). The mint can never widen this; a self-hosted token is
    //     bounded to one tenant's SelfHosted scope by construction.
    let caveats = DelegationCaveats(vec![self_hosted_grant(&runner.tenant)]);

    // (3) Mint through the contract-4.7 surface (the Identity provider behind the trait — NOT a
    //     fork). The runner CONSUMES this token off JobSpec.run_token (CI-P3).
    let handle = minter
        .mint_run_token(agent_id, run_id, &caveats, ttl_secs)
        .map_err(SelfHostedMintError::MintFailed)?;

    Ok(TenantScopedToken {
        tenant: runner.tenant.clone(),
        handle,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    /// One recorded mint call (the args the minter was driven with).
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct MintCall {
        agent_id: String,
        run_id: String,
        caveats: Vec<String>,
        ttl_secs: u64,
    }

    /// A recording [`RunTokenMinter`] that mints a token ECHOING the caveats into the bearer material
    /// (so a test can assert the self-hosted scope is the ONLY grant). Mirrors the real Identity
    /// mint's envelope shape (the grants ride in the material). Records each [`MintCall`].
    #[derive(Default)]
    struct RecordingMinter {
        calls: Mutex<Vec<MintCall>>,
    }
    impl RunTokenMinter for RecordingMinter {
        fn mint_run_token(
            &self,
            agent_id: &str,
            run_id: &str,
            caveats: &DelegationCaveats,
            ttl_secs: u64,
        ) -> Result<RunTokenHandle, RunTokenError> {
            self.calls.lock().unwrap().push(MintCall {
                agent_id: agent_id.into(),
                run_id: run_id.into(),
                caveats: caveats.0.clone(),
                ttl_secs,
            });
            Ok(RunTokenHandle {
                // The bearer material carries the (already-attenuated) grants — the real Identity
                // mint's structural envelope does the same (grants ride in the material).
                token: format!("runtok:{run_id}|{}", caveats.0.join(",")),
                jti: format!("jti:{agent_id}:{run_id}"),
                ttl_secs,
            })
        }
    }

    // ───────────────────────────── the attestation gate (fail-closed) ────────────────────────────

    /// **A valid attestation ADMITS (pending → attested); the attested runner may claim its own
    /// tenant's SelfHosted job.**
    #[test]
    fn valid_attestation_admits_and_attested_runner_may_claim() {
        let mut runner = SelfHostedRunner::register(tenant("acme"), region());
        assert_eq!(runner.attest_state(), AttestState::Pending);
        // BEFORE attesting (Pending) it claims NOTHING (fail-closed).
        assert!(
            !runner.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()),
            "a Pending (un-attested) runner cannot claim — fail-closed"
        );

        let verifier = StructuralAttestationVerifier::new();
        let att = Attestation {
            tenant: tenant("acme"),
            material: StructuralAttestationVerifier::provisioned_material(&tenant("acme"), "nonce-1"),
        };
        assert_eq!(runner.attest(&att, &verifier), AttestState::Attested);
        // The attested runner MAY claim its own tenant's SelfHosted job in its region.
        assert!(runner.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()));
    }

    /// **An ABSENT attestation REFUSES (pending → failed) ⇒ cannot claim (0 claims by an unattested
    /// runner).**
    #[test]
    fn absent_attestation_fails_closed_cannot_claim() {
        let mut runner = SelfHostedRunner::register(tenant("acme"), region());
        let verifier = StructuralAttestationVerifier::new();
        // An EMPTY (absent) attestation material — forged/absent.
        let absent = Attestation {
            tenant: tenant("acme"),
            material: String::new(),
        };
        assert_eq!(runner.attest(&absent, &verifier), AttestState::Failed);
        assert!(
            !runner.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()),
            "an absent attestation ⇒ Failed ⇒ 0 claims (fail-closed)"
        );
    }

    /// **A FORGED attestation REFUSES (a wrong-scheme envelope, or one bound to ANOTHER tenant).**
    #[test]
    fn forged_attestation_fails_closed_cannot_claim() {
        let verifier = StructuralAttestationVerifier::new();

        // (a) A wrong-scheme envelope (not `provsig:...`) — forged.
        let mut r1 = SelfHostedRunner::register(tenant("acme"), region());
        let forged_scheme = Attestation {
            tenant: tenant("acme"),
            material: "totally-made-up-token".into(),
        };
        assert_eq!(r1.attest(&forged_scheme, &verifier), AttestState::Failed);
        assert!(!r1.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()));

        // (b) An attestation whose signature is for ANOTHER tenant (cross-tenant forgery) — refused.
        let mut r2 = SelfHostedRunner::register(tenant("acme"), region());
        let cross = Attestation {
            tenant: tenant("acme"),
            // material signed for globex, presented under acme's binding — a forgery.
            material: StructuralAttestationVerifier::provisioned_material(&tenant("globex"), "n"),
        };
        assert_eq!(r2.attest(&cross, &verifier), AttestState::Failed);
        assert!(!r2.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()));

        // (c) An attestation BOUND to another tenant (the bound-tenant mismatch) — refused.
        let mut r3 = SelfHostedRunner::register(tenant("acme"), region());
        let mismatched = Attestation {
            tenant: tenant("globex"),
            material: StructuralAttestationVerifier::provisioned_material(&tenant("globex"), "n"),
        };
        assert_eq!(r3.attest(&mismatched, &verifier), AttestState::Failed);
        assert!(!r3.may_claim(TrustTier::SelfHosted, &tenant("acme"), &region()));
    }

    /// **An attested runner cannot claim a NON-SelfHosted job, a CROSS-TENANT job, or an
    /// out-of-REGION job (the gate's other three clauses).**
    #[test]
    fn attested_runner_gate_refuses_wrong_tier_tenant_or_region() {
        let mut runner = SelfHostedRunner::register(tenant("acme"), region());
        let verifier = StructuralAttestationVerifier::new();
        runner.attest(
            &Attestation {
                tenant: tenant("acme"),
                material: StructuralAttestationVerifier::provisioned_material(&tenant("acme"), "n"),
            },
            &verifier,
        );
        assert_eq!(runner.attest_state(), AttestState::Attested);

        // wrong TIER (a Trusted / UntrustedFork job is never a self-hosted runner's to claim).
        assert!(!runner.may_claim(TrustTier::Trusted, &tenant("acme"), &region()));
        assert!(!runner.may_claim(TrustTier::UntrustedFork, &tenant("acme"), &region()));
        // CROSS-TENANT (another tenant's SelfHosted job) — refused.
        assert!(!runner.may_claim(TrustTier::SelfHosted, &tenant("globex"), &region()));
        // out-of-REGION (residency — no cross-region claim) — refused.
        assert!(!runner.may_claim(TrustTier::SelfHosted, &tenant("acme"), &Region("de-fra".into())));
    }

    // ─────────────────── the tenant-scoped token mint (4.7) — cross-tenant refused ────────────────

    /// **An attested runner is minted a token scoped to ONLY its own tenant's SelfHosted grant — and
    /// the token's `admits` REFUSES another tenant's job (cross-tenant refused, recon §1).**
    #[test]
    fn mint_scopes_to_own_tenant_and_cross_tenant_is_refused() {
        let mut runner = SelfHostedRunner::register(tenant("acme"), region());
        let verifier = StructuralAttestationVerifier::new();
        runner.attest(
            &Attestation {
                tenant: tenant("acme"),
                material: StructuralAttestationVerifier::provisioned_material(&tenant("acme"), "n"),
            },
            &verifier,
        );

        let minter = RecordingMinter::default();
        let token = mint_self_hosted_token(&runner, &minter, "svc:runner-acme", "run-1", 300)
            .expect("an attested runner is minted a tenant-scoped token");

        // The token is scoped to acme.
        assert_eq!(token.tenant(), &tenant("acme"));
        // The minted caveat chain is EXACTLY the own-tenant SelfHosted grant (no cross-tenant grant).
        let calls = minter.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].caveats, vec!["selfhosted:acme".to_string()]);
        assert_eq!(calls[0].ttl_secs, 300, "minted under the run-life TTL");
        // The bearer material carries only the own-tenant grant.
        assert!(token.handle().token.contains("selfhosted:acme"));
        assert!(!token.handle().token.contains("globex"));

        // CROSS-TENANT REFUSAL: a token for acme cannot claim globex's SelfHosted job.
        assert!(
            token.admits(TrustTier::SelfHosted, &tenant("acme")),
            "the token admits its OWN tenant's SelfHosted job"
        );
        assert!(
            !token.admits(TrustTier::SelfHosted, &tenant("globex")),
            "a token for tenant acme CANNOT claim tenant globex's job (cross-tenant refused)"
        );
        // And it never admits a non-SelfHosted tier.
        assert!(!token.admits(TrustTier::Trusted, &tenant("acme")));
    }

    /// **An UN-attested runner is REFUSED a token (the attestation gate at the mint — fail-closed).**
    #[test]
    fn unattested_runner_is_refused_a_token() {
        let runner = SelfHostedRunner::register(tenant("acme"), region()); // Pending — never attested.
        let minter = RecordingMinter::default();
        let r = mint_self_hosted_token(&runner, &minter, "svc:runner", "run-1", 300);
        assert_eq!(
            r.unwrap_err(),
            SelfHostedMintError::NotAttested(AttestState::Pending),
            "an un-attested runner receives NO token (fail-closed)"
        );
        // No mint was even attempted (the gate is BEFORE the mint).
        assert_eq!(minter.calls.lock().unwrap().len(), 0);

        // A FAILED runner is likewise refused.
        let mut failed = SelfHostedRunner::register(tenant("acme"), region());
        let verifier = StructuralAttestationVerifier::new();
        failed.attest(
            &Attestation {
                tenant: tenant("acme"),
                material: String::new(),
            },
            &verifier,
        );
        assert_eq!(failed.attest_state(), AttestState::Failed);
        let r2 = mint_self_hosted_token(&failed, &minter, "svc:runner", "run-1", 300);
        assert_eq!(
            r2.unwrap_err(),
            SelfHostedMintError::NotAttested(AttestState::Failed)
        );
    }

    /// **A mint surfaces a contract-4.7 failure LOUD (Identity unavailable / refused) — never a
    /// silent fabricated token.**
    #[test]
    fn mint_surfaces_identity_failure_loud() {
        struct FailingMinter;
        impl RunTokenMinter for FailingMinter {
            fn mint_run_token(
                &self,
                _a: &str,
                _r: &str,
                _c: &DelegationCaveats,
                _t: u64,
            ) -> Result<RunTokenHandle, RunTokenError> {
                Err(RunTokenError("identity unavailable (fail-static)".into()))
            }
        }
        let mut runner = SelfHostedRunner::register(tenant("acme"), region());
        let verifier = StructuralAttestationVerifier::new();
        runner.attest(
            &Attestation {
                tenant: tenant("acme"),
                material: StructuralAttestationVerifier::provisioned_material(&tenant("acme"), "n"),
            },
            &verifier,
        );
        let r = mint_self_hosted_token(&runner, &FailingMinter, "svc:runner", "run-1", 300);
        assert!(matches!(r, Err(SelfHostedMintError::MintFailed(_))));
    }
}
