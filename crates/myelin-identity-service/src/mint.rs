//! # `mint` — `mint_run_token`: per-run attenuated tokens + self-hosted scope + mid-resume re-mint
//! (P-ID-18 → global P-076; drill ID-D6)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §4 (the per-run attenuated token: capability tokens are **attenuable bearer tokens** whose
//! authority is a **macaroon/biscuit caveat chain** — offline, client-side, **monotone
//! attenuation**; life == run life; **self-hosted-runner token scoped to one tenant's `SelfHosted`
//! jobs**; **mid-resume re-mint** C9 — a days-later HITL approval re-mints a fresh attenuated
//! token), §11 / §6 (per-run agent grants are **auto-expiring tuples** `expires_at == run life` as
//! defence-in-depth for **revoke-on-crash**), §6 (the **monotone-intersection delegation algebra**
//! the mint applies: `agent.policy ∩ delegation ∩ tenant.policy`, with the **"you cannot delegate
//! authority you do not have"** re-check at mint).
//!
//! **Reconciliation (00 §1, C6/C9):** the self-hosted-runner token scope (one tenant's `SelfHosted`
//! jobs), and the per-job-token mid-resume re-mint, are frozen on 4.7.
//!
//! **Contract-index:** row **4.7** — `mint_run_token(agent_id, run_id, delegation_caveats, ttl) →
//! token` (the **mint half** — OWNED here; the `revoke` half + the S7 store shipped in P-ID-14 /
//! P-072 and are CONSUMED here). Row **4.5** `delegation` (the intersection the mint applies) —
//! CONSUMED.
//!
//! ## What this module ships (P-ID-18 — the mint half of 4.7)
//! [`RunTokenMinter::mint_run_token`] mints a **per-run attenuated token** whose authority is the
//! composed [`DelegationAlgebra`] effective policy (so a token **never exceeds the effective
//! policy** — the mint re-applies the monotone intersection, EI-01 §7 one primitive). The mint:
//!
//! 1. **applies the delegation intersection** (`agent.policy ∩ delegation ∩ tenant.policy`, with
//!    the trigger-actor re-check) — a run token can never carry authority the agent, the delegator,
//!    or the tenant did not grant. This is the **"you cannot delegate authority you do not have"**
//!    re-check at MINT (architecture §6) — the security floor that makes "an agent can do things no
//!    human role can" structurally impossible (EI-02 §2);
//! 2. **stamps the run identity** — the token's `jti` is bound to `(agent_id, run_id)`, so the
//!    token's life IS the run's life; a per-run token is single-purpose;
//! 3. **enforces the self-hosted-runner one-tenant scope** (C6) — a self-hosted run token's
//!    authority may name only its own tenant's `SelfHosted` scope (`selfhosted:<tenant>`); it
//!    **cannot mint or act cross-tenant** (the no-global-pool property at the identity layer);
//! 4. **registers the `expires_at == run-life` TTL in the S7 store** ([`RevocationStore`]) — the
//!    auto-expiring revoke-on-crash defence-in-depth: even if the explicit teardown `revoke` is
//!    lost (the run process crashed), the token **auto-expires at run-life** inside the W bound;
//! 5. (optionally) **writes the auto-expiring per-run grant tuple** (`expires_at == run life`,
//!    architecture §6/§11) through the SAME [`TupleStore::write_tuples`] path (one write primitive).
//!
//! [`RunTokenMinter::re_mint_on_resume`] is the **mid-workflow re-mint** (C9): when a multi-day
//! HITL approval lands days later (the Workflow durable-signal case), a resuming activity re-mints a
//! **fresh** attenuated token (a new `jti`, a new `expires_at == the resumed run-life`) applying the
//! intersection **as of resume time** — so a delegator who LOST the right between dispatch and
//! resume yields a NARROWER re-minted token (the intersection is recomputed, never cached-stale).
//!
//! [`RunTokenMinter::teardown`] is the explicit revoke leg (the run ended / was killed): it
//! `revoke`s the token's `jti` (the S7 denylist) so the deny is effective immediately — the ID-D6
//! `token-revocation-lag` is the gap between teardown and the deny, which is 0 (a hot consult).
//!
//! ## The two mandatory-core properties (mutation-tested, per the prompt GATE)
//! - **The mint re-check** — a minted token's authority is the **monotone intersection** of the
//!   conjuncts; a grant outside `agent ∩ delegation ∩ tenant` (or one the delegator never held) is
//!   **never minted into the token**. A mutation that SKIPS the re-check (so the token carried the
//!   raw requested authority, exceeding the effective policy) MUST be caught — it is the exact
//!   "an agent does what no one delegated" failure.
//! - **The auto-expire** — every minted token registers an `expires_at == run-life` TTL, so it
//!   stops being honoured at run-life **even if the teardown `revoke` is skipped** (the
//!   revoke-on-crash defence-in-depth). A mutation that DROPS `expires_at` (the token lived past its
//!   run) MUST be caught.
//!
//! ## Floors named
//! **None new** — the mint is complete in M1 (the prompt's DELIVERABLE: "Floor named: none new — mint
//! is complete in M1"). Two inherited, named floors are CONSUMED here, unchanged:
//! - the **cryptographic token envelope** (PASETO v4 sign / biscuit caveat-chain crypto / DPoP) is
//!   the SAME EI-01 §1 documented structural seam the [`crate::machine_auth::StructuralTokenVerifier`]
//!   models — the mint emits the same verified-fact envelope, including purpose, audience, run
//!   binding, and durable delegation snapshot where applicable, so a minted token round-trips
//!   through `authenticate` (P-ID-07); the real crypto signer
//!   swaps in behind the [`TokenSigner`] seam (named P5/P6, the same hand-off `machine_auth` names).
//! - the **wall-clock run-deadline source** (the `expires_at == now + ttl` instant) lands with the
//!   substrate clock binding (P-S12/P-S18); here the caller supplies `now` (RFC-3339), exactly as
//!   the S7 store + the audit chain + the tuple store already do (one time convention).

use crate::delegation::{authority_of, DelegationAlgebra, DelegationInput, IntersectionProof};
use crate::delegation_policy::ResolvedDelegationPolicy;
use crate::machine_auth::{scheme, CapabilityToken, CredentialPurpose, MachineKind, TokenVerifier};
use crate::revocation::{RevocationStore, RunTokenState};
use crate::tuple_store::TupleStore;
use myelin_events::Timestamp;
use myelin_identity::{
    Credential, DelegationCaveats, FailStaticBound, ObjectId, Precondition, Principal, PrincipalId,
    RelName, RelationTuple, RevokeTarget, RunId, RunToken, TupleDelta,
};
use myelin_storage::TenantScope;
use std::sync::Arc;

/// The grant prefix a self-hosted-runner run token's authority is ceiling-bounded to
/// (`"selfhosted:<tenant>"`) — the SAME prefix [`crate::machine_auth`] enforces at `authenticate`
/// (one ceiling convention, EI-01 §7). A self-hosted run token may name only its OWN tenant's
/// `SelfHosted` scope (architecture §3/§4, C6).
pub const SELFHOSTED_GRANT_PREFIX: &str = "selfhosted:";

/// The relation a per-run grant tuple is written under when the mint also stamps the auto-expiring
/// `expires_at == run life` grant tuple (architecture §6/§11). A per-run grant is a narrow,
/// auto-expiring edge `run:<run_id>#bound@<agent_id>` — its `expires_at` IS the revoke-on-crash
/// defence-in-depth at the TUPLE layer (the S7 TTL is the same defence at the token layer).
pub const RUN_GRANT_RELATION: &str = "run_bound";

/// Final-boundary verifier for an externally presented per-run token.
///
/// The router's successful mint is not itself authorization for a mutation adapter: the adapter
/// calls this verifier immediately before its public endpoint. Verification is fail-closed and
/// binds the signed token to the expected run-token record, tenant/region, principal, and every
/// independently resolved capability required by the selected tool.
#[derive(Clone)]
pub struct RunTokenAuthorizer {
    verifier: Arc<dyn TokenVerifier>,
    revocations: RevocationStore,
    now: Arc<dyn Fn() -> Timestamp + Send + Sync>,
}

/// Why a CI-job credential was refused at the final launch boundary.
///
/// Variants expose actionable policy facts only; opaque bearer material and verifier internals are
/// deliberately never retained or rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiJobAuthorizationError {
    /// The caller did not name the exact non-empty job/run identifier it is about to launch.
    EmptyExpectedIdentifier,
    /// Signature, expiry, or signed caveat verification refused the credential.
    CredentialVerificationRefused,
    /// The verified credential was not minted under [`MachineKind::Ci`].
    WrongMachineKind { actual: MachineKind },
    /// The verified credential purpose was not [`CredentialPurpose::CiJob`].
    WrongCredentialPurpose,
    /// The signed CI job/run identifier did not equal the expected launch identifier.
    JobIdentifierMismatch,
    /// The public carrier JTI did not equal the cryptographically signed JTI.
    CarrierJtiMismatch,
    /// The signed tenant did not equal the launch boundary tenant.
    TenantMismatch,
    /// The signed region did not equal the launch boundary region.
    RegionMismatch,
    /// The signed subject did not equal the expected CI principal.
    SubjectMismatch,
    /// Durable S7 did not report the token as exactly live within its run lifetime.
    NotLive { state: RunTokenState },
    /// The attenuated signed authority omitted one capability required by this launch.
    MissingCapability { capability: String },
}

impl std::fmt::Display for CiJobAuthorizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyExpectedIdentifier => {
                write!(f, "expected CI job/run identifier must be non-empty")
            }
            Self::CredentialVerificationRefused => write!(
                f,
                "CI job credential signature, expiry, or caveat verification refused"
            ),
            Self::WrongMachineKind { actual } => {
                write!(f, "CI launch requires machine kind `Ci`, got `{actual:?}`")
            }
            Self::WrongCredentialPurpose => {
                write!(f, "CI launch requires signed credential purpose `ci_job`")
            }
            Self::JobIdentifierMismatch => write!(
                f,
                "signed CI job/run identifier does not match the expected launch identifier"
            ),
            Self::CarrierJtiMismatch => {
                write!(f, "run-token carrier JTI does not match the signed JTI")
            }
            Self::TenantMismatch => {
                write!(f, "signed CI token tenant does not match the launch tenant")
            }
            Self::RegionMismatch => {
                write!(f, "signed CI token region does not match the launch region")
            }
            Self::SubjectMismatch => {
                write!(f, "signed CI token subject does not match the expected principal")
            }
            Self::NotLive { state } => {
                write!(f, "CI job token is not live in durable S7 at launch ({state:?})")
            }
            Self::MissingCapability { capability } => write!(
                f,
                "CI launch requires capability `{capability}` outside the signed attenuated authority"
            ),
        }
    }
}

impl std::error::Error for CiJobAuthorizationError {}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BoundaryCheckError {
    CarrierJtiMismatch,
    TenantMismatch,
    RegionMismatch,
    SubjectMismatch,
    NotLive(RunTokenState),
    MissingCapability(String),
}

impl RunTokenAuthorizer {
    /// Build the production-capable boundary over an injected cryptographic verifier and the SAME
    /// durable S7 store the minter/teardown path writes.
    pub fn new(verifier: Arc<dyn TokenVerifier>, revocations: RevocationStore) -> Self {
        Self {
            verifier,
            revocations,
            now: Arc::new(system_now_timestamp),
        }
    }

    /// Inject a deterministic boundary clock for tests and replay drills.
    pub fn with_clock(mut self, now: impl Fn() -> Timestamp + Send + Sync + 'static) -> Self {
        self.now = Arc::new(now);
        self
    }

    /// Verify and authorize a token for one exact final-boundary tool invocation.
    pub fn authorize(
        &self,
        scope: &TenantScope,
        expected_principal: &PrincipalId,
        run_token: &RunToken,
        required_caps: &[String],
    ) -> Result<CapabilityToken, String> {
        let verified = self
            .verifier
            .verify(&Credential {
                scheme: scheme::AGENT.into(),
                material: run_token.token.clone(),
            })
            .map_err(|e| format!("run-token signature/caveat verification refused: {e:?}"))?;

        if verified.kind != MachineKind::Agent {
            return Err("presented token is not an agent run token".into());
        }
        match &verified.purpose {
            CredentialPurpose::AgentRun {
                delegation_snapshot: Some(snapshot),
                ..
            } if *snapshot > 0 => {}
            CredentialPurpose::AgentRun { .. } => {
                return Err(
                    "agent run token is not bound to a positive durable delegation snapshot".into(),
                )
            }
            _ => return Err("presented token purpose is not `agent_run`".into()),
        }
        self.check_boundary(
            scope,
            expected_principal,
            run_token,
            required_caps,
            &verified,
        )
        .map_err(|error| match error {
            BoundaryCheckError::CarrierJtiMismatch => {
                "run-token carrier jti does not match the signed jti".into()
            }
            BoundaryCheckError::TenantMismatch | BoundaryCheckError::RegionMismatch => {
                "run-token signed scope does not match the mutation boundary scope".into()
            }
            BoundaryCheckError::SubjectMismatch => {
                "run-token signed subject does not match the acting principal".into()
            }
            BoundaryCheckError::NotLive(_) => {
                "run token is unknown, torn down, or expired at the mutation boundary".into()
            }
            BoundaryCheckError::MissingCapability(capability) => format!(
                "tool requires capability `{capability}` outside the signed attenuated run-token authority"
            ),
        })?;
        Ok(verified)
    }

    /// Authorize one exact CI job immediately before launch.
    ///
    /// Verification is pinned to the `ci` credential scheme and requires an exact CI machine kind,
    /// exact `ci_job` purpose and non-empty signed identifier, carrier/signed JTI equality, exact
    /// tenant/region/subject binding, cryptographic and caveat validity, live S7 run state, and every
    /// independently resolved launch capability. This method grants no runtime authority itself.
    pub fn authorize_ci_job(
        &self,
        scope: &TenantScope,
        expected_principal: &PrincipalId,
        expected_job_run_id: &str,
        run_token: &RunToken,
        required_caps: &[String],
    ) -> Result<CapabilityToken, CiJobAuthorizationError> {
        if expected_job_run_id.is_empty() {
            return Err(CiJobAuthorizationError::EmptyExpectedIdentifier);
        }
        let verified = self
            .verifier
            .verify(&Credential {
                scheme: scheme::CI.into(),
                material: run_token.token.clone(),
            })
            .map_err(|_| CiJobAuthorizationError::CredentialVerificationRefused)?;
        if verified.kind != MachineKind::Ci {
            return Err(CiJobAuthorizationError::WrongMachineKind {
                actual: verified.kind,
            });
        }
        match &verified.purpose {
            CredentialPurpose::CiJob { run_id }
                if !run_id.is_empty() && run_id == expected_job_run_id => {}
            CredentialPurpose::CiJob { .. } => {
                return Err(CiJobAuthorizationError::JobIdentifierMismatch)
            }
            _ => return Err(CiJobAuthorizationError::WrongCredentialPurpose),
        }
        self.check_boundary(
            scope,
            expected_principal,
            run_token,
            required_caps,
            &verified,
        )
        .map_err(|error| match error {
            BoundaryCheckError::CarrierJtiMismatch => CiJobAuthorizationError::CarrierJtiMismatch,
            BoundaryCheckError::TenantMismatch => CiJobAuthorizationError::TenantMismatch,
            BoundaryCheckError::RegionMismatch => CiJobAuthorizationError::RegionMismatch,
            BoundaryCheckError::SubjectMismatch => CiJobAuthorizationError::SubjectMismatch,
            BoundaryCheckError::NotLive(state) => CiJobAuthorizationError::NotLive { state },
            BoundaryCheckError::MissingCapability(capability) => {
                CiJobAuthorizationError::MissingCapability { capability }
            }
        })?;
        Ok(verified)
    }

    fn check_boundary(
        &self,
        scope: &TenantScope,
        expected_principal: &PrincipalId,
        run_token: &RunToken,
        required_caps: &[String],
        verified: &CapabilityToken,
    ) -> Result<(), BoundaryCheckError> {
        if verified.jti != run_token.jti {
            return Err(BoundaryCheckError::CarrierJtiMismatch);
        }
        if verified.tenant != *scope.tenant() {
            return Err(BoundaryCheckError::TenantMismatch);
        }
        if verified.region != *scope.region() {
            return Err(BoundaryCheckError::RegionMismatch);
        }
        if verified.subject_key != expected_principal.0 {
            return Err(BoundaryCheckError::SubjectMismatch);
        }
        let state = self.revocations.run_token_state(
            scope,
            &RevokeTarget::Jti(verified.jti.clone()),
            &(self.now)(),
        );
        if state != RunTokenState::LiveWithinRunLife {
            return Err(BoundaryCheckError::NotLive(state));
        }
        for capability in required_caps {
            if !verified.authority.holds(capability) {
                return Err(BoundaryCheckError::MissingCapability(capability.clone()));
            }
        }
        Ok(())
    }
}

fn system_now_timestamp() -> Timestamp {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let dt = chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default();
    Timestamp(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// **An error the mint refuses LOUDLY on (never a fabricated/over-broad token).** A mint that cannot
/// honour its invariants (a self-hosted run token naming another tenant's scope; a non-positive
/// TTL that could mint a never-expiring token) refuses — it does NOT mint a degraded token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MintError {
    /// A self-hosted-runner run token's authority named a grant outside its own tenant's
    /// `SelfHosted` scope (cross-tenant / non-`selfhosted` grant) — the no-global-pool property
    /// (C6). The offending grant is carried for the audit.
    SelfHostedScopeViolation(String),
    /// The requested TTL is zero — a per-run token MUST have a finite, positive life (its life ==
    /// run life). A zero-TTL token could be mistaken for "never expires"; refused.
    NonPositiveTtl,
    /// A durable run-policy cursor must be a positive storage snapshot.
    InvalidDelegationSnapshot(i64),
    /// The generic run minter cannot mint long-lived PAT or deploy-key credentials.
    UnsupportedRunKind(MachineKind),
    /// Caller arguments do not match the verified principals/run bound into the resolved snapshot.
    ResolvedPolicyBindingMismatch,
}

impl core::fmt::Display for MintError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MintError::SelfHostedScopeViolation(g) => write!(
                f,
                "self-hosted-runner run token authority `{g}` names a scope outside its own \
                 tenant's SelfHosted jobs — a runner token cannot act cross-tenant (C6, \
                 no-global-pool) — refused"
            ),
            MintError::NonPositiveTtl => write!(
                f,
                "a per-run token TTL must be positive (life == run life) — a zero-TTL token is \
                 refused (it could be mistaken for never-expiring)"
            ),
            MintError::InvalidDelegationSnapshot(snapshot) => write!(
                f,
                "durable delegation snapshot `{snapshot}` is not a positive storage cursor — refused"
            ),
            MintError::UnsupportedRunKind(kind) => write!(
                f,
                "machine kind `{kind:?}` is not a run-scoped credential kind — refused"
            ),
            MintError::ResolvedPolicyBindingMismatch => f.write_str(
                "resolved delegation policy does not match the requested run/principal/scope binding — refused",
            ),
        }
    }
}

impl std::error::Error for MintError {}

/// The pluggable token-SIGNING seam (the EI-01 §1 named floor — the mint counterpart of
/// [`crate::machine_auth::TokenVerifier`]). A signer turns the mint's trust-rooted facts (tenant,
/// region, subject key, jti, the attenuated authority) into the opaque bearer material a presented
/// credential later verifies. The REAL crypto signer (PASETO v4 sign / biscuit caveat-chain seal /
/// DPoP binding) implements this and swaps in behind the SAME seam; the mint's intersection + scope
/// + TTL logic does not change. The floor implementation is [`StructuralTokenSigner`].
pub trait TokenSigner: Send + Sync {
    /// Sign the mint's facts into the opaque bearer material. The material MUST round-trip through
    /// the verifier (the floor signer emits the verifier's complete verified-fact envelope), so a
    /// minted token authenticates as the principal it was minted
    /// for. `grants` is the ALREADY-ATTENUATED effective authority (the mint applied the
    /// intersection) — the signer never widens it.
    fn sign(&self, request: &TokenSignRequest) -> String;
}

/// Trust-rooted, already-attenuated facts presented to a [`TokenSigner`].
///
/// Tenant and region travel together as a verified [`TenantScope`], while the subject and purpose
/// retain their domain types. Raw subject keys, revocation handles, and grant names are deliberately
/// omitted from `Debug` because signer requests routinely cross logging/error boundaries.
#[derive(Clone)]
pub struct TokenSignRequest {
    scope: TenantScope,
    subject: PrincipalId,
    jti: String,
    purpose: CredentialPurpose,
    expires_at: Timestamp,
    grants: Vec<String>,
}

impl TokenSignRequest {
    /// Capture the verified scope and the mint's already-attenuated authority without widening it.
    pub fn new(
        scope: &TenantScope,
        subject: PrincipalId,
        jti: impl Into<String>,
        purpose: CredentialPurpose,
        expires_at: Timestamp,
        grants: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            scope: scope.clone(),
            subject,
            jti: jti.into(),
            purpose,
            expires_at,
            grants: grants.into_iter().map(Into::into).collect(),
        }
    }

    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    pub fn subject(&self) -> &PrincipalId {
        &self.subject
    }

    pub fn jti(&self) -> &str {
        &self.jti
    }

    pub fn purpose(&self) -> &CredentialPurpose {
        &self.purpose
    }

    pub fn expires_at(&self) -> &Timestamp {
        &self.expires_at
    }

    pub fn grants(&self) -> &[String] {
        &self.grants
    }
}

impl core::fmt::Debug for TokenSignRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TokenSignRequest")
            .field("tenant", self.scope.tenant())
            .field("region", self.scope.region())
            .field("subject", &"<redacted>")
            .field("jti", &"<redacted>")
            .field("purpose", &self.purpose.claim())
            .field("expires_at", &self.expires_at)
            .field("grant_count", &self.grants.len())
            .finish()
    }
}

/// **The floor token signer (the EI-01 §1 documented deviation).** Emits the SAME structural
/// verified-token envelope [`crate::machine_auth::StructuralTokenVerifier`] parses
/// (`<tenant>|<region>|<subject_key>|<jti>|<dpop:0|1>|<grant>,<grant>,…`), so a minted run token
/// round-trips through `authenticate` (P-ID-07) — the authorization-relevant path is proven without
/// pretending to do crypto. A per-run token is short-lived (TTL-constrained), so it is minted
/// `dpop:0` (its TTL is the constraint, not DPoP — §4). The real PASETO/biscuit signer is the named
/// P5/P6 floor.
#[derive(Clone, Copy, Debug, Default)]
pub struct StructuralTokenSigner;

impl StructuralTokenSigner {
    /// A fresh floor token signer.
    pub fn new() -> StructuralTokenSigner {
        StructuralTokenSigner
    }
}

impl TokenSigner for StructuralTokenSigner {
    fn sign(&self, request: &TokenSignRequest) -> String {
        let tenant = &request.scope().tenant().0;
        let region = &request.scope().region().0;
        let subject_key = &request.subject().0;
        let jti = request.jti();
        let purpose = request.purpose();
        // A per-run token is TTL-constrained, not DPoP-bound (§4) → dpop = 0. The grant list is the
        // already-attenuated effective authority (comma-separated; empty ⇒ no grants).
        format!(
            "{tenant}|{region}|{subject_key}|{jti}|0|{}|{}|edge|{}|{}",
            request.grants().join(","),
            purpose.claim(),
            purpose.run_id().unwrap_or_default(),
            match purpose {
                crate::machine_auth::CredentialPurpose::AgentRun {
                    delegation_snapshot,
                    ..
                } => delegation_snapshot.map_or_else(String::new, |snapshot| snapshot.to_string()),
                _ => String::new(),
            },
        )
    }
}

/// **The recorded proof a kill-mid-flight is bounded (the ID-D6 green artifact, EI-01 §3 "prove
/// it").** A killed run's per-run token is revoked (teardown) AND auto-expires (`expires_at`) within
/// run-life ≤ W. This records the two legs so the drill emits the dated artifact: the
/// `revoked_on_teardown` flag (the explicit `revoke` landed → deny is effective at teardown time)
/// and the `auto_expires_within_run_life` flag (the S7 TTL guarantees the token dies at run-life
/// even if teardown is SKIPPED). Both true is the only passing value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationProof {
    /// The token's `jti` (the revocation handle).
    pub jti: String,
    /// The explicit teardown `revoke` landed → the deny is effective immediately (a hot consult).
    pub revoked_on_teardown: bool,
    /// The token auto-expires at run-life via the S7 `expires_at` TTL — even if teardown is skipped
    /// (the revoke-on-crash defence-in-depth).
    pub auto_expires_within_run_life: bool,
    /// The measured token-revocation-lag, in seconds: the gap between the kill and the deny. `0` for
    /// the teardown path (a hot denylist consult — the deny is effective at revoke time); for the
    /// crash path (teardown skipped) the bound is the TTL (run-life), asserted ≤ W by the drill.
    pub token_revocation_lag_secs: i64,
}

impl RevocationProof {
    /// Is the kill bounded (both legs hold)? `true` is the only passing value the drill greens on.
    pub fn holds(&self) -> bool {
        self.revoked_on_teardown && self.auto_expires_within_run_life
    }
}

/// **The per-run-token minter (contract 4.7, the mint half; architecture §4/§6/§11).** Mints
/// per-run attenuated tokens that never exceed the composed delegation policy, registers the
/// `expires_at == run-life` TTL (revoke-on-crash defence-in-depth), enforces the self-hosted-runner
/// one-tenant scope, and re-mints a fresh attenuated token mid-workflow on resume.
///
/// Holds the [`DelegationAlgebra`] (the intersection the mint applies — the SAME algebra P-ID-17
/// ships, one primitive), the S7 [`RevocationStore`] (the TTL + teardown revoke target P-ID-14
/// ships), an optional [`TupleStore`] (for the auto-expiring per-run grant tuple), and the pluggable
/// [`TokenSigner`] (the floor structural envelope; the real crypto signer swaps in behind it).
#[derive(Clone)]
pub struct RunTokenMinter {
    /// The monotone-intersection delegation algebra (P-ID-17) — the mint applies it so a token never
    /// exceeds `agent.policy ∩ delegation ∩ tenant.policy` (one intersection primitive).
    algebra: DelegationAlgebra,
    /// The S7 revocation list / token denylist (P-ID-14) — the mint registers the `expires_at ==
    /// run-life` TTL here (defence-in-depth), and `teardown` revokes the `jti` here.
    revocations: RevocationStore,
    /// The token signer (the floor structural envelope → the real crypto signer behind the seam).
    signer: std::sync::Arc<dyn TokenSigner>,
    /// The S3 tuple store — when wired, the mint also writes the auto-expiring per-run grant tuple
    /// (`expires_at == run life`, §6/§11). `None` for the pure-token mint (the token-layer TTL is
    /// the revoke-on-crash defence; the tuple-layer grant is an additional defence-in-depth a caller
    /// that owns the run's object graph wires).
    tuples: Option<TupleStore>,
}

impl RunTokenMinter {
    /// **The production minter constructor (MR-012) — REQUIRES an injected real [`TokenSigner`].**
    /// No `Structural*` default is built here: the composition root injects the REAL
    /// [`crate::capability_crypto::PasetoCapabilitySigner`] (PASETO v4 / Ed25519, from the cell token
    /// authority). `tuples` is `Some` when the caller also writes the auto-expiring per-run grant
    /// tuple (`expires_at == run life`, §6/§11). This is the constructor `StoreBackedCheck` wires —
    /// the `no-structural-crypto-in-prod` scanner is GREEN here (no mock-crypto construction).
    pub fn with_signer_and_tuples(
        revocations: RevocationStore,
        tuples: Option<TupleStore>,
        signer: std::sync::Arc<dyn TokenSigner>,
    ) -> RunTokenMinter {
        RunTokenMinter {
            algebra: DelegationAlgebra::new(),
            revocations,
            signer,
            tuples,
        }
    }

    /// **TEST-DOUBLE constructor (`#[cfg(test)]`, MR-012).** Build a minter over a shared S7
    /// [`RevocationStore`] with the mock floor [`StructuralTokenSigner`] and NO tuple store. The
    /// forgeable-envelope signer is NOT in the production graph — production injects the real signer
    /// via [`Self::with_signer_and_tuples`]; the scanner admits this `#[cfg(test)]`-gated construction.
    #[cfg(test)]
    pub fn new(revocations: RevocationStore) -> RunTokenMinter {
        RunTokenMinter::with_signer_and_tuples(
            revocations,
            None,
            std::sync::Arc::new(StructuralTokenSigner::new()),
        )
    }

    /// **TEST-DOUBLE constructor (`#[cfg(test)]`, MR-012).** A minter that ALSO writes the
    /// auto-expiring per-run grant tuple, with the mock floor [`StructuralTokenSigner`]. Production
    /// uses [`Self::with_signer_and_tuples`] with the real signer.
    #[cfg(test)]
    pub fn with_tuple_store(revocations: RevocationStore, tuples: TupleStore) -> RunTokenMinter {
        RunTokenMinter::with_signer_and_tuples(
            revocations,
            Some(tuples),
            std::sync::Arc::new(StructuralTokenSigner::new()),
        )
    }

    /// Swap in an explicit [`TokenSigner`] (the seam the real PASETO/biscuit crypto signer plugs
    /// into — the named floor). The mint's intersection + scope + TTL logic is unchanged.
    pub fn with_signer(mut self, signer: std::sync::Arc<dyn TokenSigner>) -> RunTokenMinter {
        self.signer = signer;
        self
    }

    /// The shared S7 revocation list / token denylist this minter writes the per-run TTL + teardown
    /// revoke into (so a caller can assert the deny / read the `revocation_lag` telemetry).
    pub fn revocations(&self) -> &RevocationStore {
        &self.revocations
    }

    /// **`mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token` (contract 4.7, the mint
    /// half).** Mints a per-run attenuated token whose authority is the composed effective policy
    /// (the monotone intersection the mint applies), with `expires_at == run-life` registered in S7.
    ///
    /// - `scope` is the verified `(tenant, region)` the run executes under (the partition the TTL is
    ///   registered in; tenant-from-token, never a path — the tenant-predicate floor). The ABI 4.7
    ///   signature has no scope; the service surface ([`crate::StoreBackedCheck`]) wires THIS entry.
    /// - `agent_id` / `run_id` identify the run; the token's `jti` is bound to them (the token's life
    ///   IS the run's life).
    /// - `input` carries the delegation conjuncts (the agent ceiling, the delegation chain, the
    ///   tenant guardrails, the trigger-actor's held set) the intersection composes — the SAME
    ///   [`DelegationInput`] P-ID-17 uses. `delegation_caveats` (the frozen ABI carrier) is the
    ///   projection of `input.delegation`; both are accepted so the ABI shape and the rich algebra
    ///   input stay aligned.
    /// - `kind` selects the self-hosted-runner ceiling (a [`MachineKind::PerJob`] run token is
    ///   bounded to its own tenant's `SelfHosted` scope, C6).
    /// - `ttl` is the run's life (the `expires_at` window); `now` is the mint instant (RFC-3339).
    ///
    /// Returns the minted [`RunToken`] (the opaque bearer material + the `jti`), or a [`MintError`]
    /// (a self-hosted scope violation; a non-positive TTL) — never a fabricated/over-broad token.
    #[allow(clippy::too_many_arguments)]
    pub fn mint_run_token(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &DelegationInput,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now: &Timestamp,
    ) -> Result<RunToken, MintError> {
        let (token, _proof) = self.mint_proved(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            input,
            delegation_caveats,
            kind,
            ttl,
            now,
        )?;
        Ok(token)
    }

    /// **The mint WITH the recorded [`IntersectionProof`] (the ID-D5/ID-D6 "prove-it" observation).**
    /// Identical to [`RunTokenMinter::mint_run_token`] but additionally returns the proof that the
    /// minted token's authority is the monotone intersection (the four conjunct sets + the composed
    /// effective set + the verified subset-of-every-conjunct post-condition). EI-01 §3: the property
    /// does not exist until a test forces it AND the mint records it.
    #[allow(clippy::too_many_arguments)]
    pub fn mint_proved(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &DelegationInput,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now: &Timestamp,
    ) -> Result<(RunToken, IntersectionProof), MintError> {
        self.mint_proved_with_snapshot(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            input,
            delegation_caveats,
            kind,
            ttl,
            now,
            None,
        )
    }

    /// Mint from a server-resolved durable delegation policy. This is the authoritative run-token
    /// path: the signed credential binds both the run id and the exact durable policy snapshot.
    #[allow(clippy::too_many_arguments)]
    pub fn mint_from_resolved_policy(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        resolved: &ResolvedDelegationPolicy,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now: &Timestamp,
    ) -> Result<RunToken, MintError> {
        let snapshot = resolved.cursor.snapshot;
        if snapshot <= 0 {
            return Err(MintError::InvalidDelegationSnapshot(snapshot));
        }
        if &resolved.run_id != run_id
            || &resolved.agent_id != agent_id
            || &agent.principal_id != agent_id
            || resolved.trigger_actor_id != trigger_actor.principal_id
            || scope.tenant() != &agent.tenant
            || scope.region() != &agent.region
            || scope.tenant() != &trigger_actor.tenant
            || scope.region() != &trigger_actor.region
            || self
                .algebra
                .delegation(agent, trigger_actor, &resolved.input)
                != resolved.effective_policy
        {
            return Err(MintError::ResolvedPolicyBindingMismatch);
        }
        let (token, _) = self.mint_proved_with_snapshot(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            &resolved.input,
            delegation_caveats,
            kind,
            ttl,
            now,
            Some(snapshot),
        )?;
        Ok(token)
    }

    #[allow(clippy::too_many_arguments)]
    fn mint_proved_with_snapshot(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &DelegationInput,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now: &Timestamp,
        delegation_snapshot: Option<i64>,
    ) -> Result<(RunToken, IntersectionProof), MintError> {
        // (0) A per-run token MUST have a finite, positive life (life == run life). A zero-TTL token
        //     could be mistaken for never-expiring — refuse it (never mint a no-expiry per-run token).
        if ttl.static_max_secs == 0 {
            return Err(MintError::NonPositiveTtl);
        }
        let _ = delegation_caveats; // the ABI carrier; the rich `input.delegation` is the authority source.

        // (1) THE MINT RE-CHECK (architecture §6 — "you cannot delegate authority you do not have").
        //     Apply the SAME monotone intersection the delegation algebra (P-ID-17) computes:
        //     effective = agent.policy ∩ (delegation ∩ trigger_actor_held) ∩ tenant.policy. The
        //     minted token's authority is THIS effective set — a token never exceeds the effective
        //     policy. (One intersection primitive; the mint does not re-implement the algebra.)
        let (effective_policy, proof) = self.algebra.delegation_proved(agent, trigger_actor, input);
        let effective = authority_of(&effective_policy);

        // (2) THE SELF-HOSTED-RUNNER ONE-TENANT SCOPE (architecture §3/§4, C6). A self-hosted run
        //     token's authority may name ONLY its own tenant's SelfHosted scope. We enforce it
        //     AFTER the intersection (so the ceiling is checked on the authority actually minted) —
        //     a grant outside the own-tenant SelfHosted scope is refused (never silently dropped or
        //     widened: the run was asked to act on a scope it must not, which is a loud error).
        if kind.is_self_hosted_runner() {
            let own = format!("{SELFHOSTED_GRANT_PREFIX}{}", scope.tenant().0);
            for g in effective.grants() {
                if !g.starts_with(SELFHOSTED_GRANT_PREFIX) || g != own {
                    return Err(MintError::SelfHostedScopeViolation(g.to_string()));
                }
            }
        }

        // (3) The run identity: the token's jti is bound to (agent_id, run_id) — the token's life IS
        //     the run's life. A per-run token is single-purpose; its jti is the revocation handle.
        let jti = run_token_jti(agent_id, run_id, now);

        // Compute the authoritative run deadline before signing so the outer cryptographic `exp` and
        // the durable S7 lifecycle record share the same boundary.
        let expires_at = expires_at_of(now, ttl);

        let purpose = match kind {
            MachineKind::Agent => CredentialPurpose::AgentRun {
                run_id: run_id.0.clone(),
                delegation_snapshot,
            },
            MachineKind::Ci => CredentialPurpose::CiJob {
                run_id: run_id.0.clone(),
            },
            MachineKind::PerJob => CredentialPurpose::PerJob {
                run_id: run_id.0.clone(),
            },
            MachineKind::Pat | MachineKind::DeployKey => {
                return Err(MintError::UnsupportedRunKind(kind))
            }
        };

        // (4) Sign the (already-attenuated) effective authority into the bearer material (the floor
        //     structural envelope → the real crypto signer behind the seam). The signer NEVER widens
        //     the authority — it carries exactly the effective set the intersection produced.
        let request = TokenSignRequest::new(
            scope,
            agent_id.clone(),
            &jti,
            purpose,
            expires_at.clone(),
            effective.grants(),
        );
        let material = self.signer.sign(&request);

        // (5) Register the `expires_at == run-life` TTL in S7 (the revoke-on-crash defence-in-depth,
        //     §11). The token is denylisted UNTIL `expires_at` — wait, no: the S7 TTL models the
        //     auto-EXPIRY (the token stops being a *live grant* at run-life). The revoke-on-crash
        //     property: even if the explicit teardown revoke is lost, the token cannot outlive its
        //     run — authentication requires `run_token_state == LiveWithinRunLife`, so expired,
        //     torn-down, and unknown records are dead. We register the TTL so run-life is RECORDED in the
        //     durable S7 mirror (so a crash-recovered cell still knows the run-life boundary). This
        //     is the per-run-token TTL store P-ID-14 (`register_run_token_ttl`) shipped for this mint.
        self.revocations
            .register_run_token_ttl(scope, &jti, now.clone(), expires_at.clone());

        // (6) Optionally write the auto-expiring per-run GRANT tuple (`expires_at == run life`,
        //     §6/§11) — the tuple-layer revoke-on-crash defence, alongside the token-layer S7 TTL.
        //     A narrow `run:<run_id>#run_bound@<agent_id>` edge whose expiry IS the run-life. Through
        //     the SAME write_tuples path (one write primitive); a write failure is non-fatal to the
        //     mint (the token-layer TTL is the primary defence) but is surfaced (never swallowed).
        if let Some(tuples) = &self.tuples {
            let delta = TupleDelta::Add(RelationTuple {
                object: ObjectId(format!("run:{}", run_id.0)),
                relation: RelName(RUN_GRANT_RELATION.into()),
                subject: agent_id.clone(),
                caveat: None,
            });
            // The grant tuple co-commits with its iam.tuple_written emit; expires_at == run life.
            let _ = tuples.write_tuples(
                scope,
                agent,
                &[delta],
                None::<&Precondition>,
                Some(expires_at),
                now.clone(),
            );
        }

        Ok((
            RunToken {
                token: material,
                jti,
            },
            proof,
        ))
    }

    /// **The mid-workflow re-mint on resume (architecture §4, C9 — the Workflow durable-signal
    /// case).** When a multi-day HITL approval lands days later, the resuming activity re-mints a
    /// **fresh** attenuated token: a NEW `jti`, a NEW `expires_at == the resumed run-life`, applying
    /// the intersection **as of resume time** (`now_resume`). Because the intersection is recomputed
    /// against the conjuncts as-of-resume, a delegator who LOST the right between dispatch and resume
    /// yields a NARROWER re-minted token — the re-mint is never a stale copy of the dispatch token.
    ///
    /// Returns the fresh [`RunToken`] (a distinct `jti` from any prior mint for the run). The prior
    /// token is NOT auto-revoked here (the workflow engine tears it down on its own boundary,
    /// [`RunTokenMinter::teardown`]) — the re-mint is purely "a fresh, possibly-narrower token for
    /// the resumed leg".
    #[allow(clippy::too_many_arguments)]
    pub fn re_mint_on_resume(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        input_as_of_resume: &DelegationInput,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now_resume: &Timestamp,
    ) -> Result<RunToken, MintError> {
        // The re-mint IS a mint, run again as-of resume time (one mint primitive, no bespoke resume
        // path). The fresh `now_resume` yields a fresh jti + a fresh expires_at; the recomputed
        // intersection yields the as-of-resume (possibly narrower) authority.
        self.mint_run_token(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            input_as_of_resume,
            delegation_caveats,
            kind,
            ttl,
            now_resume,
        )
    }

    /// **The explicit teardown revoke (the run ended / was killed) — the ID-D6 teardown leg.**
    /// Revokes the token's `jti` (the S7 denylist) so the deny is effective immediately (a hot
    /// consult — the token-revocation-lag is 0). Idempotent + crash-safe (it is a `revoke` of the
    /// jti, P-ID-14). Pairs with the auto-expiry: teardown is the immediate deny, the `expires_at`
    /// TTL is the defence-in-depth if teardown is skipped (the crash path).
    pub fn teardown(&self, scope: &TenantScope, token: &RunToken, now: &Timestamp) {
        self.revocations
            .tear_down_run_token(scope, &token.jti, now.clone());
    }

    /// **Is the run token still live as of `now` (the consult a surface runs before honouring a per-
    /// run token)?** A token is dead when (a) it was explicitly revoked (teardown), OR (b) its
    /// `expires_at == run-life` TTL has passed (the auto-expire, even if teardown was skipped). This
    /// is the consult `authenticate`/`check` already run for the jti (the SAME S7 `is_revoked`
    /// honouring expiry) — exposed here as the run-token-liveness predicate the drill asserts.
    pub fn is_live(&self, scope: &TenantScope, token: &RunToken, now: &Timestamp) -> bool {
        // `is_revoked` is TRUE while a TTL'd entry has NOT expired (it is on the denylist UNTIL
        // expiry as the run-life marker), and stays true forever for a teardown revoke (a no-TTL
        // jti revoke). So liveness needs BOTH: the entry exists (the run was minted) AND we are
        // before expiry AND it was not torn down. We model this directly against the two facts:
        // the TTL window and the teardown flag — see `revocation_state`.
        match self.revocation_state(scope, token, now) {
            RunTokenState::LiveWithinRunLife => true,
            RunTokenState::Expired | RunTokenState::TornDown | RunTokenState::Unknown => false,
        }
    }

    /// The state of a per-run token's S7 record as of `now` (the basis for [`RunTokenMinter::is_live`]
    /// and the ID-D6 proof) — re-exported from the S7 store ([`RevocationStore::run_token_state`]).
    /// Distinguishes the deaths: torn-down (explicit teardown), expired (the TTL passed — the
    /// auto-expire), and the live window.
    pub fn revocation_state(
        &self,
        scope: &TenantScope,
        token: &RunToken,
        now: &Timestamp,
    ) -> RunTokenState {
        let target = RevokeTarget::Jti(token.jti.clone());
        self.revocations.run_token_state(scope, &target, now)
    }
}

/// Build a per-run token's `jti` from `(agent_id, run_id, mint_instant)`. A re-mint at a later
/// instant yields a DISTINCT jti (the mint-instant disambiguates the resumed leg from the dispatch
/// leg) — so the re-mint is a fresh token, never a collision with the prior one. PII-free (opaque
/// ids only).
pub fn run_token_jti(agent_id: &PrincipalId, run_id: &RunId, mint_instant: &Timestamp) -> String {
    format!("runtok:{}:{}:{}", agent_id.0, run_id.0, mint_instant.0)
}

/// Compute the per-run token's `expires_at == now + ttl` (the run-life boundary). The wall-clock
/// arithmetic source lands with the substrate clock binding (P-S12/P-S18); on this floor `now` is an
/// RFC-3339 instant and the TTL is added by parsing the trailing seconds — the structural carrier
/// the real clock swaps in behind. We model the boundary as `now#+<ttl>s` only when `now` is not a
/// parseable instant; for the canonical `YYYY-MM-DDTHH:MM:SSZ` form we add the TTL to the seconds.
pub fn expires_at_of(now: &Timestamp, ttl: &FailStaticBound) -> Timestamp {
    Timestamp(add_secs_rfc3339(&now.0, ttl.static_max_secs))
}

/// Add `secs` to an RFC-3339 `YYYY-MM-DDTHH:MM:SSZ` instant, carrying minutes/hours/days as needed.
/// The lexical order of the result preserves chronological order (the SAME convention the S7 store,
/// the tuple-store zookie, and the audit chain rely on), so the S7 `now < expires_at` string compare
/// is correct. A non-canonical instant falls back to a lexically-greater suffix so the TTL is never
/// silently zero (the auto-expire is mandatory-core — it must always be a strictly-later instant).
fn add_secs_rfc3339(instant: &str, secs: u64) -> String {
    // Parse `YYYY-MM-DDTHH:MM:SSZ`. If it does not match, return a lexically-greater marker so the
    // expiry is always strictly after `now` (never a no-op that would drop the auto-expire).
    let parsed = parse_rfc3339(instant);
    match parsed {
        Some((y, mo, d, h, mi, s)) => {
            // Convert to a day-second count, add, re-normalise. Days-in-month is the proleptic
            // Gregorian calendar; this is the structural floor (the real clock library lands with
            // the substrate binding). Correct for the bounded TTLs a run-life uses (≤ hours).
            let total = day_seconds(h, mi, s) + secs;
            let extra_days = total / 86_400;
            let rem = total % 86_400;
            let (nh, nmi, ns) = (
                (rem / 3_600) as u32,
                ((rem % 3_600) / 60) as u32,
                (rem % 60) as u32,
            );
            let (ny, nmo, nd) = add_days(y, mo, d, extra_days);
            format!("{ny:04}-{nmo:02}-{nd:02}T{nh:02}:{nmi:02}:{ns:02}Z")
        }
        None => format!("{instant}#+{secs}s"),
    }
}

fn day_seconds(h: u32, mi: u32, s: u32) -> u64 {
    h as u64 * 3_600 + mi as u64 * 60 + s as u64
}

/// Parse a `YYYY-MM-DDTHH:MM:SSZ` instant into its fields (the canonical floor form). `None` for any
/// other shape (the caller falls back to a lexically-greater suffix so the auto-expire never drops).
fn parse_rfc3339(s: &str) -> Option<(i64, u32, u32, u32, u32, u32)> {
    // Expected exact length `YYYY-MM-DDTHH:MM:SSZ` == 20 chars.
    let b = s.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: u32 = s.get(5..7)?.parse().ok()?;
    let d: u32 = s.get(8..10)?.parse().ok()?;
    let h: u32 = s.get(11..13)?.parse().ok()?;
    let mi: u32 = s.get(14..16)?.parse().ok()?;
    let sec: u32 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    Some((y, mo, d, h, mi, sec))
}

/// Add `extra_days` to a `(year, month, day)`, carrying across month/year boundaries (proleptic
/// Gregorian). For the bounded TTLs a run-life uses, `extra_days` is 0 or 1; the loop handles any
/// value robustly.
fn add_days(mut y: i64, mut mo: u32, mut d: u32, extra_days: u64) -> (i64, u32, u32) {
    let mut remaining = extra_days;
    while remaining > 0 {
        let dim = days_in_month(y, mo);
        if d < dim {
            d += 1;
        } else {
            d = 1;
            if mo == 12 {
                mo = 1;
                y += 1;
            } else {
                mo += 1;
            }
        }
        remaining -= 1;
    }
    (y, mo, d)
}

fn days_in_month(y: i64, mo: u32) -> u32 {
    match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine_auth::{Authority, StructuralTokenVerifier};
    use myelin_events::OutboxStore;
    use myelin_identity::{PrincipalKind, RuntimeRef};
    use myelin_tenancy::{Region, TenantId};

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn scope_in(tenant: &str, region: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region(region.into()))
    }

    fn agent(id: &str, tenant: &str) -> Principal {
        let mut p = Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt-1".into()),
                on_behalf_of: Some(PrincipalId("p:human".into())),
            },
            TenantId(tenant.into()),
        );
        p.region = Region("eu-west".into());
        p
    }

    fn human(id: &str, tenant: &str) -> Principal {
        let mut p = Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        p.region = Region("eu-west".into());
        p
    }

    fn auth(grants: &[&str]) -> Authority {
        Authority::of(grants.iter().copied())
    }

    #[test]
    fn token_sign_request_debug_redacts_bearer_identifiers_and_authority() {
        let request = TokenSignRequest::new(
            &scope("acme"),
            PrincipalId("svc:secret-agent".into()),
            "runtok:secret-jti",
            CredentialPurpose::AgentRun {
                run_id: "run:secret".into(),
                delegation_snapshot: Some(42),
            },
            Timestamp("2030-01-01T00:00:00Z".into()),
            ["repo:secret:admin"],
        );
        let debug = format!("{request:?}");
        assert!(!debug.contains("svc:secret-agent"));
        assert!(!debug.contains("runtok:secret-jti"));
        assert!(!debug.contains("run:secret"));
        assert!(!debug.contains("repo:secret:admin"));
        assert!(debug.contains("agent_run"));
        assert!(debug.contains("grant_count: 1"));
    }

    fn input(agent: &[&str], deleg: &[&str], tenant: &[&str], held: &[&str]) -> DelegationInput {
        DelegationInput {
            agent_policy: auth(agent),
            delegation: auth(deleg),
            tenant_policy: auth(tenant),
            trigger_actor_held: auth(held),
        }
    }

    fn ts(s: &str) -> Timestamp {
        Timestamp(s.into())
    }

    fn ttl(secs: u64) -> FailStaticBound {
        FailStaticBound {
            static_max_secs: secs,
        }
    }

    fn caveats(g: &[&str]) -> DelegationCaveats {
        DelegationCaveats(g.iter().map(|s| s.to_string()).collect())
    }

    fn resolved_policy(
        run_id: &str,
        agent: &Principal,
        trigger_actor: &Principal,
        input: DelegationInput,
        snapshot: i64,
    ) -> ResolvedDelegationPolicy {
        let effective_policy = DelegationAlgebra::new().delegation(agent, trigger_actor, &input);
        ResolvedDelegationPolicy {
            run_id: RunId(run_id.into()),
            agent_id: agent.principal_id.clone(),
            trigger_actor_id: trigger_actor.principal_id.clone(),
            input,
            effective_policy,
            cursor: crate::delegation_policy::DelegationRunPolicyCursor {
                snapshot,
                versions: myelin_storage::DurableDelegationPolicyVersions {
                    agent: 1,
                    delegation: 1,
                    tenant: 1,
                    trigger_actor: 1,
                },
                revisions: myelin_storage::DurableDelegationPolicyRevisions {
                    agent: 1,
                    delegation: 1,
                    tenant: 1,
                    trigger_actor: 1,
                },
            },
        }
    }

    fn mint_ci_token(
        s7: &RevocationStore,
        scope: &TenantScope,
        subject: &str,
        job_run_id: &str,
        grants: &[&str],
        lifetime_secs: u64,
    ) -> RunToken {
        RunTokenMinter::new(s7.clone())
            .mint_run_token(
                scope,
                &PrincipalId(subject.into()),
                &RunId(job_run_id.into()),
                &agent(subject, &scope.tenant().0),
                &human("p:human", &scope.tenant().0),
                &input(grants, grants, grants, grants),
                &caveats(grants),
                MachineKind::Ci,
                &ttl(lifetime_secs),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint CI job token")
    }

    #[derive(Clone)]
    struct FixedCiSchemeVerifier(CapabilityToken);

    impl TokenVerifier for FixedCiSchemeVerifier {
        fn verify(&self, credential: &Credential) -> myelin_identity::Result<CapabilityToken> {
            assert_eq!(
                credential.scheme,
                scheme::CI,
                "CI boundary pins the verifier scheme"
            );
            Ok(self.0.clone())
        }
    }

    fn fixed_capability(kind: MachineKind, purpose: CredentialPurpose) -> CapabilityToken {
        CapabilityToken {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            kind,
            subject_key: "svc:ci".into(),
            authority: auth(&["job.launch", "artifact.write"]),
            jti: "fixed-jti".into(),
            dpop_bound: false,
            purpose,
            audience: crate::machine_auth::CredentialAudience::Edge,
            exp_unix: i64::MAX,
        }
    }

    #[test]
    fn final_boundary_rechecks_signed_attenuated_authority_and_liveness() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7.clone());
        let acme = scope("acme");
        let minted_at = ts("2026-06-19T00:00:00Z");
        let agent = agent("p:agent", "acme");
        let trigger = human("p:human", "acme");
        let policy = resolved_policy(
            "run-authority",
            &agent,
            &trigger,
            input(
                &["repo.push", "pull_request.merge"],
                &["repo.push"],
                &["repo.push", "pull_request.merge"],
                &["repo.push", "pull_request.merge"],
            ),
            7,
        );
        let token = minter
            .mint_from_resolved_policy(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-authority".into()),
                &agent,
                &trigger,
                &policy,
                &caveats(&["repo.push"]),
                MachineKind::Agent,
                &ttl(300),
                &minted_at,
            )
            .expect("mint attenuated run token");
        let authorizer =
            RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7.clone())
                .with_clock(|| ts("2026-06-19T00:01:00Z"));

        authorizer
            .authorize(
                &acme,
                &PrincipalId("p:agent".into()),
                &token,
                &["repo.push".into()],
            )
            .expect("the one capability surviving delegation is admitted");
        let denied = authorizer
            .authorize(
                &acme,
                &PrincipalId("p:agent".into()),
                &token,
                &["pull_request.merge".into()],
            )
            .expect_err("attenuated-away capability must be denied");
        assert!(denied.contains("outside the signed attenuated"));

        minter.teardown(&acme, &token, &ts("2026-06-19T00:02:00Z"));
        let denied = authorizer
            .authorize(
                &acme,
                &PrincipalId("p:agent".into()),
                &token,
                &["repo.push".into()],
            )
            .expect_err("a torn-down token must fail again at the mutation boundary");
        assert!(denied.contains("torn down"));
    }

    #[test]
    fn final_boundary_refuses_mixed_carrier_identity_and_scope() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7.clone());
        let acme = scope("acme");
        let agent = agent("p:agent", "acme");
        let trigger = human("p:human", "acme");
        let policy = resolved_policy(
            "run-binding",
            &agent,
            &trigger,
            input(
                &["repo.push"],
                &["repo.push"],
                &["repo.push"],
                &["repo.push"],
            ),
            9,
        );
        let token = minter
            .mint_from_resolved_policy(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-binding".into()),
                &agent,
                &trigger,
                &policy,
                &caveats(&["repo.push"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint");
        let authorizer = RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7)
            .with_clock(|| ts("2026-06-19T00:01:00Z"));

        let mut mixed = token.clone();
        mixed.jti = "attacker-selected-jti".into();
        assert!(authorizer
            .authorize(
                &acme,
                &PrincipalId("p:agent".into()),
                &mixed,
                &["repo.push".into()],
            )
            .expect_err("carrier/signed jti mismatch")
            .contains("does not match"));
        assert!(authorizer
            .authorize(
                &acme,
                &PrincipalId("p:other".into()),
                &token,
                &["repo.push".into()],
            )
            .expect_err("subject mismatch")
            .contains("subject"));
        assert!(authorizer
            .authorize(
                &scope("globex"),
                &PrincipalId("p:agent".into()),
                &token,
                &["repo.push".into()],
            )
            .expect_err("scope mismatch")
            .contains("scope"));
    }

    #[test]
    fn final_boundary_refuses_snapshotless_agent_run_even_when_capability_matches() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7.clone());
        let acme = scope("acme");
        let token = minter
            .mint_run_token(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("legacy-raw-policy-run".into()),
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &input(
                    &["repo.push"],
                    &["repo.push"],
                    &["repo.push"],
                    &["repo.push"],
                ),
                &caveats(&["repo.push"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("legacy raw-policy mint remains available outside production routing");
        let authorizer = RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7)
            .with_clock(|| ts("2026-06-19T00:01:00Z"));
        let denied = authorizer
            .authorize(
                &acme,
                &PrincipalId("p:agent".into()),
                &token,
                &["repo.push".into()],
            )
            .expect_err("snapshot-less AgentRun must never reach a mutation adapter");
        assert!(denied.contains("durable delegation snapshot"));
    }

    #[test]
    fn ci_job_final_boundary_binds_scheme_kind_purpose_identity_scope_and_authority() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        let token = mint_ci_token(
            &s7,
            &acme,
            "svc:ci",
            "job:run-17:build",
            &["job.launch", "artifact.write"],
            300,
        );
        let authorizer =
            RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7.clone())
                .with_clock(|| ts("2026-06-19T00:01:00Z"));

        let authorized = authorizer
            .authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-17:build",
                &token,
                &["job.launch".into(), "artifact.write".into()],
            )
            .expect("the exact live CI job token is admitted immediately before launch");
        assert_eq!(authorized.kind, MachineKind::Ci);
        assert_eq!(
            authorized.purpose,
            CredentialPurpose::CiJob {
                run_id: "job:run-17:build".into()
            }
        );

        assert_eq!(
            authorizer.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "",
                &token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::EmptyExpectedIdentifier)
        );
        assert_eq!(
            authorizer.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-17:test",
                &token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::JobIdentifierMismatch)
        );
        assert_eq!(
            authorizer.authorize_ci_job(
                &scope("globex"),
                &PrincipalId("svc:ci".into()),
                "job:run-17:build",
                &token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::TenantMismatch)
        );
        assert_eq!(
            authorizer.authorize_ci_job(
                &scope_in("acme", "eu-north"),
                &PrincipalId("svc:ci".into()),
                "job:run-17:build",
                &token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::RegionMismatch)
        );
        assert_eq!(
            authorizer.authorize_ci_job(
                &acme,
                &PrincipalId("svc:other".into()),
                "job:run-17:build",
                &token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::SubjectMismatch)
        );
        let mut mixed_carrier = token.clone();
        mixed_carrier.jti = "attacker-selected-jti".into();
        assert_eq!(
            authorizer.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-17:build",
                &mixed_carrier,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::CarrierJtiMismatch)
        );
        assert_eq!(
            authorizer.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-17:build",
                &token,
                &["secret.read".into()],
            ),
            Err(CiJobAuthorizationError::MissingCapability {
                capability: "secret.read".into()
            })
        );
    }

    #[test]
    fn ci_job_final_boundary_refuses_wrong_credentials_and_every_non_live_s7_state() {
        let acme = scope("acme");
        let live_s7 = RevocationStore::new();
        let ci_token = mint_ci_token(
            &live_s7,
            &acme,
            "svc:ci",
            "job:run-18:test",
            &["job.launch"],
            300,
        );

        let agent_token = RunTokenMinter::new(live_s7.clone())
            .mint_run_token(
                &acme,
                &PrincipalId("svc:ci".into()),
                &RunId("job:run-18:test".into()),
                &agent("svc:ci", "acme"),
                &human("p:human", "acme"),
                &input(
                    &["job.launch"],
                    &["job.launch"],
                    &["job.launch"],
                    &["job.launch"],
                ),
                &caveats(&["job.launch"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:01Z"),
            )
            .unwrap();
        let structural =
            RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), live_s7.clone())
                .with_clock(|| ts("2026-06-19T00:01:00Z"));
        assert_eq!(
            structural.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-18:test",
                &agent_token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::CredentialVerificationRefused),
            "an Agent credential cannot be reinterpreted under the required `ci` scheme"
        );
        let per_job_token = RunTokenMinter::new(live_s7.clone())
            .mint_run_token(
                &acme,
                &PrincipalId("svc:ci".into()),
                &RunId("job:run-18:test".into()),
                &agent("svc:ci", "acme"),
                &human("p:human", "acme"),
                &input(
                    &["selfhosted:acme"],
                    &["selfhosted:acme"],
                    &["selfhosted:acme"],
                    &["selfhosted:acme"],
                ),
                &caveats(&["selfhosted:acme"]),
                MachineKind::PerJob,
                &ttl(300),
                &ts("2026-06-19T00:00:02Z"),
            )
            .unwrap();
        assert_eq!(
            structural.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-18:test",
                &per_job_token,
                &["selfhosted:acme".into()],
            ),
            Err(CiJobAuthorizationError::CredentialVerificationRefused),
            "a PerJob credential cannot be reinterpreted under the required `ci` scheme"
        );
        let malformed = RunToken {
            token: "not-a-verified-token".into(),
            jti: "public-jti".into(),
        };
        let verification_error = structural
            .authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-18:test",
                &malformed,
                &["job.launch".into()],
            )
            .unwrap_err();
        assert_eq!(
            verification_error,
            CiJobAuthorizationError::CredentialVerificationRefused
        );
        assert!(!verification_error.to_string().contains(&malformed.token));

        for (kind, purpose, expected) in [
            (
                MachineKind::Agent,
                CredentialPurpose::CiJob {
                    run_id: "job:run-18:test".into(),
                },
                CiJobAuthorizationError::WrongMachineKind {
                    actual: MachineKind::Agent,
                },
            ),
            (
                MachineKind::PerJob,
                CredentialPurpose::CiJob {
                    run_id: "job:run-18:test".into(),
                },
                CiJobAuthorizationError::WrongMachineKind {
                    actual: MachineKind::PerJob,
                },
            ),
            (
                MachineKind::Pat,
                CredentialPurpose::CiJob {
                    run_id: "job:run-18:test".into(),
                },
                CiJobAuthorizationError::WrongMachineKind {
                    actual: MachineKind::Pat,
                },
            ),
            (
                MachineKind::DeployKey,
                CredentialPurpose::CiJob {
                    run_id: "job:run-18:test".into(),
                },
                CiJobAuthorizationError::WrongMachineKind {
                    actual: MachineKind::DeployKey,
                },
            ),
            (
                MachineKind::Ci,
                CredentialPurpose::PerJob {
                    run_id: "job:run-18:test".into(),
                },
                CiJobAuthorizationError::WrongCredentialPurpose,
            ),
            (
                MachineKind::Ci,
                CredentialPurpose::AgentRun {
                    run_id: "job:run-18:test".into(),
                    delegation_snapshot: Some(1),
                },
                CiJobAuthorizationError::WrongCredentialPurpose,
            ),
        ] {
            let fixed = fixed_capability(kind, purpose);
            let carrier = RunToken {
                token: "opaque".into(),
                jti: fixed.jti.clone(),
            };
            let authorizer = RunTokenAuthorizer::new(
                Arc::new(FixedCiSchemeVerifier(fixed)),
                RevocationStore::new(),
            );
            assert_eq!(
                authorizer.authorize_ci_job(
                    &acme,
                    &PrincipalId("svc:ci".into()),
                    "job:run-18:test",
                    &carrier,
                    &["job.launch".into()],
                ),
                Err(expected)
            );
        }

        let authorize_with = |s7: RevocationStore, now: &'static str| {
            RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7)
                .with_clock(move || ts(now))
                .authorize_ci_job(
                    &acme,
                    &PrincipalId("svc:ci".into()),
                    "job:run-18:test",
                    &ci_token,
                    &["job.launch".into()],
                )
        };
        assert_eq!(
            authorize_with(RevocationStore::new(), "2026-06-19T00:01:00Z"),
            Err(CiJobAuthorizationError::NotLive {
                state: RunTokenState::Unknown
            })
        );
        assert_eq!(
            authorize_with(live_s7.clone(), "2026-06-19T00:06:00Z"),
            Err(CiJobAuthorizationError::NotLive {
                state: RunTokenState::Expired
            })
        );

        let torn_down_s7 = RevocationStore::new();
        let torn_down = mint_ci_token(
            &torn_down_s7,
            &acme,
            "svc:ci",
            "job:run-19:test",
            &["job.launch"],
            300,
        );
        torn_down_s7.tear_down_run_token(&acme, &torn_down.jti, ts("2026-06-19T00:01:00Z"));
        assert_eq!(
            RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), torn_down_s7,)
                .with_clock(|| ts("2026-06-19T00:01:01Z"))
                .authorize_ci_job(
                    &acme,
                    &PrincipalId("svc:ci".into()),
                    "job:run-19:test",
                    &torn_down,
                    &["job.launch".into()],
                ),
            Err(CiJobAuthorizationError::NotLive {
                state: RunTokenState::TornDown
            })
        );

        let revoked_s7 = RevocationStore::new();
        revoked_s7.revoke(
            &acme,
            &RevokeTarget::Jti(ci_token.jti.clone()),
            ts("2026-06-19T00:00:30Z"),
        );
        assert_eq!(
            authorize_with(revoked_s7, "2026-06-19T00:01:00Z"),
            Err(CiJobAuthorizationError::NotLive {
                state: RunTokenState::TornDown
            })
        );
    }

    /// **Minting re-checks "cannot delegate what you lack": the token's authority is the monotone
    /// intersection, never the raw requested set (architecture §6 — the mint re-check, mandatory-
    /// core).** The delegation NAMES a grant the delegator never held; the minted token does NOT
    /// carry it (the mint applied the intersection).
    #[test]
    fn mint_applies_the_intersection_cannot_delegate_what_you_lack() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        // The delegation TRIES to grant admin; the delegator never HELD admin → it is dropped.
        let inp = input(
            &["repo:acme/web#admin", "repo:acme/web#read"],
            &["repo:acme/web#admin", "repo:acme/web#read"],
            &["repo:acme/web#admin", "repo:acme/web#read"],
            &["repo:acme/web#read"], // delegator holds only read
        );
        let (token, proof) = minter
            .mint_proved(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-1".into()),
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &inp,
                &caveats(&["repo:acme/web#admin", "repo:acme/web#read"]),
                MachineKind::Agent,
                &ttl(60),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint succeeds");
        // The minted token's material carries ONLY repo:acme/web#read (admin was never minted).
        assert!(
            token.token.contains("repo:acme/web#read"),
            "the held grant is minted"
        );
        assert!(
            !token.token.contains("admin"),
            "a grant the delegator never held is NEVER minted into the token (the mint re-check)"
        );
        assert!(
            proof.holds(),
            "the minted authority is ⊆ every conjunct (monotone)"
        );
        assert_eq!(proof.effective, vec!["repo:acme/web#read".to_string()]);
    }

    /// **A self-hosted-runner run token cannot act cross-tenant (architecture §3/§4, C6 — mandatory-
    /// core).** A per-job run token may name ONLY its own tenant's SelfHosted scope; an effective
    /// authority naming another tenant's scope is REFUSED (the no-global-pool property).
    #[test]
    fn self_hosted_runner_token_cannot_act_cross_tenant() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");

        // Own-tenant SelfHosted scope → mints.
        let ok = input(
            &["selfhosted:acme"],
            &["selfhosted:acme"],
            &["selfhosted:acme"],
            &["selfhosted:acme"],
        );
        let token = minter
            .mint_run_token(
                &acme,
                &PrincipalId("svc:runner".into()),
                &RunId("run-1".into()),
                &agent("svc:runner", "acme"),
                &human("p:human", "acme"),
                &ok,
                &caveats(&["selfhosted:acme"]),
                MachineKind::PerJob,
                &ttl(60),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("own-tenant SelfHosted run token mints");
        assert!(token.token.contains("selfhosted:acme"));

        // An effective authority naming ANOTHER tenant's SelfHosted scope → refused.
        // (The conjuncts all name globex's scope, so the intersection is non-empty — it is the
        // SCOPE ceiling, not the intersection, that must catch this.)
        let cross = input(
            &["selfhosted:globex"],
            &["selfhosted:globex"],
            &["selfhosted:globex"],
            &["selfhosted:globex"],
        );
        let r = minter.mint_run_token(
            &acme,
            &PrincipalId("svc:runner".into()),
            &RunId("run-2".into()),
            &agent("svc:runner", "acme"),
            &human("p:human", "acme"),
            &cross,
            &caveats(&["selfhosted:globex"]),
            MachineKind::PerJob,
            &ttl(60),
            &ts("2026-06-19T00:00:00Z"),
        );
        assert_eq!(
            r,
            Err(MintError::SelfHostedScopeViolation(
                "selfhosted:globex".into()
            )),
            "a self-hosted run token naming another tenant's scope is refused (C6, no-global-pool)"
        );
    }

    /// **A re-mint on resume yields a FRESH attenuated token (architecture §4, C9).** The resumed
    /// leg re-mints a distinct `jti`; and when the delegator LOST the right between dispatch and
    /// resume, the re-minted token is NARROWER (the intersection recomputed as-of-resume).
    #[test]
    fn re_mint_on_resume_is_fresh_and_recomputes_the_intersection() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let agent_id = PrincipalId("p:agent".into());
        let run = RunId("run-1".into());

        // DISPATCH: the delegator holds both read + write → both flow into the token.
        let dispatch = input(
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
        );
        let t0 = minter
            .mint_run_token(
                &acme,
                &agent_id,
                &run,
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &dispatch,
                &caveats(&["repo:acme/web#read", "repo:acme/web#write"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("dispatch mint");
        assert!(
            t0.token.contains("repo:acme/web#write"),
            "dispatch token carries #write"
        );

        // RESUME days later: the delegator LOST #write. The re-mint recomputes the intersection
        // as-of-resume → the fresh token is narrower (no #write).
        let resume = input(
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read"], // delegator's held set shrank
        );
        let t1 = minter
            .re_mint_on_resume(
                &acme,
                &agent_id,
                &run,
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &resume,
                &caveats(&["repo:acme/web#read", "repo:acme/web#write"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-22T09:00:00Z"),
            )
            .expect("re-mint on resume");
        assert_ne!(
            t1.jti, t0.jti,
            "the re-mint is a FRESH token (distinct jti)"
        );
        assert!(
            t1.token.contains("repo:acme/web#read"),
            "the re-minted token keeps #read"
        );
        assert!(
            !t1.token.contains("#write"),
            "the re-minted token is NARROWER (the delegator lost #write — recomputed as-of-resume)"
        );
    }

    /// **A per-run token auto-expires at run-life (architecture §11 — mandatory-core).** The minted
    /// token is live within its run-life window and DEAD after `expires_at` — even with NO teardown
    /// revoke (the revoke-on-crash defence-in-depth).
    #[test]
    fn per_run_token_auto_expires_at_run_life() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let token = minter
            .mint_run_token(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-1".into()),
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &input(&["g"], &["g"], &["g"], &["g"]),
                &caveats(&["g"]),
                MachineKind::Agent,
                &ttl(300), // run-life = 5 min
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint");
        // WITHIN run-life: live (no teardown issued).
        assert!(
            minter.is_live(&acme, &token, &ts("2026-06-19T00:02:00Z")),
            "the token is live within its run-life window"
        );
        // AFTER run-life: dead (auto-expired) — even though teardown was never called.
        assert!(
            !minter.is_live(&acme, &token, &ts("2026-06-19T00:06:00Z")),
            "the token auto-expires at run-life even if teardown is skipped (revoke-on-crash)"
        );
        assert_eq!(
            minter.revocation_state(&acme, &token, &ts("2026-06-19T00:06:00Z")),
            RunTokenState::Expired,
            "past run-life the token's state is Expired (the auto-expire)"
        );
    }

    /// **A killed run's token is revoked AND auto-expires (the ID-D6 core — mandatory-core).** On
    /// teardown the token is denied immediately (token-revocation-lag = 0); and even on the crash
    /// path (teardown skipped) it auto-expires at run-life. Both legs hold.
    #[test]
    fn killed_run_token_is_revoked_and_auto_expires() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let token = minter
            .mint_run_token(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-1".into()),
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &input(&["g"], &["g"], &["g"], &["g"]),
                &caveats(&["g"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint");
        // Live mid-run.
        assert!(minter.is_live(&acme, &token, &ts("2026-06-19T00:01:00Z")));
        // KILL mid-flight → teardown revoke. The deny is effective immediately.
        minter.teardown(&acme, &token, &ts("2026-06-19T00:01:30Z"));
        assert!(
            !minter.is_live(&acme, &token, &ts("2026-06-19T00:01:31Z")),
            "after teardown the token is dead immediately (token-revocation-lag = 0)"
        );
        assert_eq!(
            minter.revocation_state(&acme, &token, &ts("2026-06-19T00:01:31Z")),
            RunTokenState::TornDown
        );
        // And the auto-expire leg: even past run-life it stays dead (defence-in-depth).
        assert!(!minter.is_live(&acme, &token, &ts("2026-06-19T00:06:00Z")));
    }

    /// **A zero-TTL mint is refused (a per-run token must have a finite, positive life).**
    #[test]
    fn zero_ttl_mint_is_refused() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let r = minter.mint_run_token(
            &acme,
            &PrincipalId("p:agent".into()),
            &RunId("run-1".into()),
            &agent("p:agent", "acme"),
            &human("p:human", "acme"),
            &input(&["g"], &["g"], &["g"], &["g"]),
            &caveats(&["g"]),
            MachineKind::Agent,
            &ttl(0),
            &ts("2026-06-19T00:00:00Z"),
        );
        assert_eq!(r, Err(MintError::NonPositiveTtl));
    }

    /// **The mint optionally writes the auto-expiring per-run GRANT tuple (`expires_at == run
    /// life`, §6/§11) — the tuple-layer revoke-on-crash defence.** With a tuple store wired, the
    /// minted run writes a narrow `run:<run_id>#run_bound@<agent_id>` edge whose `expires_at` is the
    /// run-life.
    #[test]
    fn mint_writes_the_auto_expiring_run_grant_tuple() {
        let s7 = RevocationStore::new();
        let tuples = TupleStore::new(OutboxStore::new());
        let minter = RunTokenMinter::with_tuple_store(s7, tuples.clone());
        let acme = scope("acme");
        minter
            .mint_run_token(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-77".into()),
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &input(&["g"], &["g"], &["g"], &["g"]),
                &caveats(&["g"]),
                MachineKind::Agent,
                &ttl(120),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint");
        // The per-run grant tuple is present, auto-expiring (expires_at == run life = +120s).
        let stored = tuples.tuples_in(&acme);
        let grant = stored
            .iter()
            .find(|t| t.tuple.object.0 == "run:run-77")
            .expect("the auto-expiring per-run grant tuple was written");
        assert_eq!(grant.tuple.relation.0, RUN_GRANT_RELATION);
        assert_eq!(grant.tuple.subject.0, "p:agent");
        assert_eq!(
            grant.expires_at,
            Some(ts("2026-06-19T00:02:00Z")),
            "the per-run grant tuple's expires_at == run life (now + 120s)"
        );
    }

    /// **`expires_at_of` adds the TTL to the run's mint instant (the run-life boundary), carrying
    /// minute/hour/day boundaries (the auto-expire is mandatory-core — it is ALWAYS strictly later).**
    #[test]
    fn expires_at_is_always_strictly_after_now() {
        // A simple within-minute add.
        assert_eq!(
            expires_at_of(&ts("2026-06-19T00:00:00Z"), &ttl(30)).0,
            "2026-06-19T00:00:30Z"
        );
        // Carrying across a minute.
        assert_eq!(
            expires_at_of(&ts("2026-06-19T00:00:45Z"), &ttl(30)).0,
            "2026-06-19T00:01:15Z"
        );
        // Carrying across an hour.
        assert_eq!(
            expires_at_of(&ts("2026-06-19T00:59:45Z"), &ttl(30)).0,
            "2026-06-19T01:00:15Z"
        );
        // Carrying across a day (and a month boundary).
        assert_eq!(
            expires_at_of(&ts("2026-06-30T23:59:50Z"), &ttl(20)).0,
            "2026-07-01T00:00:10Z"
        );
        // The auto-expire is NEVER a no-op: the result is always lexically (chronologically) GREATER.
        for (now, secs) in [
            ("2026-06-19T00:00:00Z", 1u64),
            ("2026-12-31T23:59:59Z", 1),
            ("not-a-real-instant", 60),
        ] {
            let exp = expires_at_of(&ts(now), &ttl(secs));
            assert!(
                exp.0.as_str() > now,
                "expires_at ({}) must be strictly after now ({now})",
                exp.0
            );
        }
    }

    #[test]
    fn authoritative_mint_binds_the_resolved_run_and_snapshot() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let agent = agent("svc:agent", "acme");
        let trigger = human("p:human", "acme");
        let resolved = resolved_policy(
            "run-9",
            &agent,
            &trigger,
            input(
                &["repo.pull"],
                &["repo.pull"],
                &["repo.pull"],
                &["repo.pull"],
            ),
            42,
        );

        let token = minter
            .mint_from_resolved_policy(
                &acme,
                &agent.principal_id,
                &RunId("run-9".into()),
                &agent,
                &trigger,
                &resolved,
                &caveats(&["repo.pull"]),
                MachineKind::Agent,
                &ttl(60),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("authoritative mint");

        let parts: Vec<&str> = token.token.split('|').collect();
        assert_eq!(parts[8], "run-9");
        assert_eq!(parts[9], "42", "the durable snapshot is signed");
    }

    #[test]
    fn authoritative_mint_refuses_a_mismatched_resolved_binding() {
        let minter = RunTokenMinter::new(RevocationStore::new());
        let acme = scope("acme");
        let agent = agent("svc:agent", "acme");
        let trigger = human("p:human", "acme");
        let resolved = resolved_policy(
            "run-9",
            &agent,
            &trigger,
            input(
                &["repo.pull"],
                &["repo.pull"],
                &["repo.pull"],
                &["repo.pull"],
            ),
            42,
        );

        let result = minter.mint_from_resolved_policy(
            &acme,
            &agent.principal_id,
            &RunId("run-other".into()),
            &agent,
            &trigger,
            &resolved,
            &caveats(&["repo.pull"]),
            MachineKind::Agent,
            &ttl(60),
            &ts("2026-06-19T00:00:00Z"),
        );

        assert_eq!(result, Err(MintError::ResolvedPolicyBindingMismatch));
    }

    /// **The minted token round-trips through the `authenticate` envelope shape (one token
    /// convention).** The floor signer emits the SAME signed-fact envelope
    /// `StructuralTokenVerifier` parses, including purpose, audience, run id, and optional durable
    /// delegation snapshot. The legacy caller-supplied mint leaves the snapshot empty, so it can
    /// exercise mint algebra but cannot authenticate as an authoritative edge run credential.
    #[test]
    fn minted_token_uses_the_authenticate_envelope_shape() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let token = minter
            .mint_run_token(
                &acme,
                &PrincipalId("svc:agent".into()),
                &RunId("run-9".into()),
                &agent("svc:agent", "acme"),
                &human("p:human", "acme"),
                &input(
                    &["agent:run"],
                    &["agent:run"],
                    &["agent:run"],
                    &["agent:run"],
                ),
                &caveats(&["agent:run"]),
                MachineKind::Agent,
                &ttl(60),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint");
        let parts: Vec<&str> = token.token.split('|').collect();
        assert_eq!(parts.len(), 10, "the envelope has all signed-fact fields");
        assert_eq!(parts[0], "acme", "tenant from the verified scope");
        assert_eq!(parts[1], "eu-west", "region from the verified scope");
        assert_eq!(parts[2], "svc:agent", "subject_key is the agent id");
        assert_eq!(
            parts[3], token.jti,
            "the envelope jti matches the RunToken jti"
        );
        assert_eq!(
            parts[4], "0",
            "a per-run token is dpop=0 (TTL-constrained, not DPoP-bound)"
        );
        assert_eq!(
            parts[5], "agent:run",
            "the grants are the attenuated effective authority"
        );
        assert_eq!(parts[6], "agent_run");
        assert_eq!(parts[7], "edge");
        assert_eq!(parts[8], "run-9");
        assert_eq!(
            parts[9], "",
            "caller-supplied legacy mint has no durable policy snapshot"
        );
    }
}
