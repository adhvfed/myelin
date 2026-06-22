//! # `authenticate` — the v1 human/SSO credential set (P-ID-06 → P-065)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §3 (the ONE polymorphic `Principal{kind, tenant, region, data_role, status}` — machine identity
//! resolves to the SAME record), §4 (the authentication surfaces — SAML 2.0 / OIDC / SCIM 2.0 /
//! WebAuthn-FIDO2 passkeys / SSH; **tenant is taken from the verified credential, never the URL
//! path**, ID-3, EI-02 §1), §2 (the S1 store: principals + "SSO/SCIM links" — the lookup this body
//! keys on).
//!
//! **Contract-index:** row 4.1 `authenticate(credential) → Principal` (the human/SSO half — OWNED
//! here); rows 11.1 (the S1 store backing it), 12.1 (`(tenant, region)` partition), 1.8
//! (`auth_decision_latency` — emitted per request) — CONSUMED / WIRED.
//!
//! ## What this module ships (P-ID-06 — the human/SSO half of `authenticate`)
//! [`HumanSsoAuthenticator::authenticate`] resolves the **v1 human/SSO credential set** — OIDC,
//! SAML 2.0, SCIM 2.0, WebAuthn/FIDO2 passkeys, SSH-pubkey — each to the one polymorphic
//! [`Principal`] backed by the S1 [`crate::principal_store::PrincipalStore`]. The pipeline is:
//!
//! ```text
//! credential ──verify──▶ VerifiedAssertion{ tenant, region, scheme, subject_key }
//!            ──tenant-from-credential (NEVER the URL path, ID-3)──▶ TenantScope
//!            ──S1 SSO/SCIM-link lookup──▶ PrincipalRow ──▶ Principal{kind, tenant, region, …}
//!            └─emit auth_decision_latency (per request)
//! ```
//!
//! ### The IDOR floor (ID-3, the stop-the-bleeding invariant, EI-01 §2)
//! The tenant is taken from the **verified credential**, never the URL path. The optional
//! `path_tenant` a gateway received is passed in only so the body can COUNT a rejected mismatch for
//! the survival signal — the resolved `Principal.tenant` is ALWAYS the credential's, and the
//! `path_derived_tenant_count` is **0** (asserted by the drill). This reuses the ONE primitive the
//! storage tier already froze for this exact decision: [`TenantScope::resolve`] (no second IDOR
//! path, EI-01 §7).
//!
//! ### The capability-token / machine-identity half is P-ID-07 (P-066)
//! PAT / CI-job / agent-run capability tokens and the deploy-key / per-job machine identity are the
//! NEXT prompt (P-ID-07 → P-066); they EXTEND this same [`HumanSsoAuthenticator`] surface (the
//! frozen 4.1 signature is unchanged). This module ships the five human/SSO surfaces only.
//!
//! ## Floors named (frozen shape now → bodies in a later prompt / parallel track)
//! - **Cryptographic credential VERIFICATION is modelled at the structural seam, not the wire.**
//!   The real OIDC JWKS-signature / SAML XML-DSig / WebAuthn attestation / SSH challenge-response
//!   verification is the gateway/IdP-integration deliverable (named P5/P6 below); what this body
//!   ships — and proves — is the load-bearing AUTHORIZATION-relevant logic: the credential's
//!   **tenant is the trust root (never the path)**, the per-scheme **subject-key extraction**, the
//!   **S1 directory resolution**, the **polymorphic Principal shape**, the **disabled-principal
//!   fail-closed**, and the **per-request telemetry**. The verifier is a pluggable
//!   [`CredentialVerifier`] seam: the floor verifier ([`StructuralVerifier`]) parses the frozen
//!   verified-assertion envelope; a real cryptographic verifier swaps in behind the SAME seam
//!   without changing this body. This is the EI-01 §1 documented deviation (the in-process model of
//!   a contract whose live binding lands later — the same posture the outbox / S1 store / S3 store
//!   already document).
//! - **hardware-attested device binding + full passkey-sync governance + SAML SLO → P5/P6** (named
//!   in the architecture §4 floor). v1 ships the five surfaces; the attestation/sync hardening is
//!   post-M5. **SCIM deprovision is the v1 AUTHORITATIVE revocation path** (architecture §4): a
//!   SCIM-disabled principal authenticates to `Disabled` and fails closed — the full revocation
//!   list (S7) + the SCIM-disable revocation wiring is P-ID-14 (P-072); here a `Disabled`/`Suspended`
//!   principal is refused at `authenticate` (it never resolves to an active session).

use crate::principal_store::PrincipalStore;
use myelin_identity::{
    iam_events::signals, AuthzError, CaveatContext, Consistency, Credential, Decision,
    DelegationCaveats, EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService,
    ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission, Precondition,
    Principal, PrincipalId, PrincipalStatus, RevokeTarget, RewriteTrace, RunId, RunToken,
    SubjectTree, TupleDelta, Zookie,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The v1 human/SSO credential schemes this body resolves (architecture §4). A scheme is a
/// `&'static str` (matching the frozen [`Credential::scheme`] free-string carrier, P-ID-01) so a
/// new surface is an additive change, not an enum break. The machine-identity / capability-token
/// schemes (`pat` / `ci` / `agent` / `deploy_key`) are P-ID-07's (P-066) extension.
pub mod scheme {
    /// OpenID Connect (OIDC Core 1.0) — a human signing in via their org's IdP.
    pub const OIDC: &str = "oidc";
    /// SAML 2.0 — a human signing in via an enterprise SAML IdP.
    pub const SAML: &str = "saml";
    /// SCIM 2.0 (RFC 7642/3/4) — a directory-provisioned identity (the SCIM-managed user).
    pub const SCIM: &str = "scim";
    /// WebAuthn / FIDO2 passkey — a human authenticating with a platform/roaming authenticator.
    pub const PASSKEY: &str = "passkey";
    /// SSH public key — the principal a Git smart-transport authenticates as (then `check` per ref).
    pub const SSH: &str = "ssh";

    /// The complete v1 human/SSO scheme set (the surfaces this prompt ships).
    pub const HUMAN_SSO_SCHEMES: &[&str] = &[OIDC, SAML, SCIM, PASSKEY, SSH];

    /// Is `s` one of the five v1 human/SSO schemes this body owns? (A capability-token /
    /// machine-identity scheme is P-ID-07's — this body refuses it with `WrongAuthenticator`.)
    pub fn is_human_sso(s: &str) -> bool {
        HUMAN_SSO_SCHEMES.contains(&s)
    }
}

/// A **verified credential assertion** — the trust-rooted facts a [`CredentialVerifier`] extracts
/// from a presented [`Credential`] after verifying it (architecture §4: all surfaces resolve to the
/// same Principal; tenant is from the verified credential).
///
/// **`tenant` is the load-bearing field:** it is the tenant the IdP/credential VERIFIED this subject
/// belongs to — the trust root for the whole request. It is NEVER taken from the URL path (ID-3).
/// `subject_key` is the per-scheme stable subject identifier (the OIDC `sub`, the SAML NameID, the
/// SCIM externalId, the passkey credential id, the SSH key fingerprint) the S1 SSO/SCIM-link index
/// keys on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedAssertion {
    /// The tenant the credential was VERIFIED for (the trust root — never the URL path, ID-3).
    pub tenant: TenantId,
    /// The residency region the principal is pinned to (`(tenant, region)`, 12.1).
    pub region: Region,
    /// The credential scheme (`oidc` / `saml` / `scim` / `passkey` / `ssh`).
    pub scheme: String,
    /// The per-scheme stable subject key the S1 SSO/SCIM-link index resolves (e.g. the OIDC `sub`).
    pub subject_key: String,
}

/// The pluggable credential-verification seam (the EI-01 §1 named floor). A verifier turns a
/// presented [`Credential`] into a trust-rooted [`VerifiedAssertion`] — OR refuses it loudly. The
/// REAL cryptographic verifiers (OIDC JWKS / SAML XML-DSig / WebAuthn attestation / SSH
/// challenge-response) implement this trait and swap in behind the SAME seam; this body's
/// resolution + telemetry logic does not change. The floor implementation is [`StructuralVerifier`].
pub trait CredentialVerifier: Send + Sync {
    /// Verify `credential` and extract its trust-rooted assertion, or refuse it. A refusal is a
    /// LOUD [`AuthzError`] (never a fabricated/empty assertion — an unverifiable credential does not
    /// resolve to a Principal). The tenant in the returned assertion is the credential's, never a
    /// caller-supplied path.
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion>;
}

/// **The floor credential verifier (the EI-01 §1 documented deviation).** It parses the frozen
/// verified-assertion envelope from the credential's opaque [`Credential::material`] — the
/// structural model of "the IdP verified this credential and asserts these facts". The real
/// cryptographic verification (signature/attestation) is the named P5/P6 floor; this verifier
/// proves the AUTHORIZATION-relevant path (tenant-from-credential, subject-key extraction) without
/// pretending to do crypto it does not.
///
/// ## The frozen verified-assertion envelope (the floor wire shape)
/// `material = "<tenant>|<region>|<subject_key>"` — three `|`-separated fields. This is NOT a
/// security claim about the bytes (a real IdP signs them); it is the structural stand-in for "the
/// verifier returned these verified facts". A malformed envelope is refused
/// ([`AuthzError::BadRequest`]) — never coerced into a partial/empty assertion.
#[derive(Clone, Copy, Debug, Default)]
pub struct StructuralVerifier;

impl StructuralVerifier {
    /// A fresh floor verifier.
    pub fn new() -> StructuralVerifier {
        StructuralVerifier
    }
}

impl CredentialVerifier for StructuralVerifier {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        // This body owns ONLY the five human/SSO schemes; a capability-token / machine-identity
        // scheme belongs to P-ID-07 (P-066) and is refused here loudly (never silently mis-resolved
        // through the wrong authenticator).
        if !scheme::is_human_sso(&credential.scheme) {
            return Err(AuthzError::BadRequest(format!(
                "scheme `{}` is not a v1 human/SSO surface (oidc/saml/scim/passkey/ssh); the \
                 capability-token + machine-identity surfaces are P-ID-07",
                credential.scheme
            )));
        }
        // Parse the frozen verified-assertion envelope `<tenant>|<region>|<subject_key>`. The real
        // cryptographic verification is the named floor; this is the structural stand-in.
        let parts: Vec<&str> = credential.material.split('|').collect();
        if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
            return Err(AuthzError::BadRequest(
                "malformed verified-assertion envelope (expected `<tenant>|<region>|<subject_key>`, \
                 all non-empty)"
                    .into(),
            ));
        }
        Ok(VerifiedAssertion {
            tenant: TenantId(parts[0].to_string()),
            region: Region(parts[1].to_string()),
            scheme: credential.scheme.clone(),
            subject_key: parts[2].to_string(),
        })
    }
}

/// **The per-request `auth_decision_latency` telemetry sink (contract-index row 1.8).** Every
/// `authenticate` call — success OR failure — records exactly one `auth_decision_latency`
/// observation (observability is part of the pass, EI-01 §3: an auth decision that emits no signal
/// has failed the gate). The signal is keyed by the FROZEN name constant
/// [`signals::AUTH_DECISION_LATENCY`] (later prompts assert against the named signal, never a
/// literal). The metrics-health-port export wiring (OpenTelemetry, architecture §3.5/§10) lands with
/// the real port binding; this is the in-process counter the body increments and a drill asserts.
#[derive(Debug, Default)]
pub struct AuthTelemetry {
    /// The count of `auth_decision_latency` observations emitted (one per `authenticate` request).
    count: AtomicU64,
}

impl AuthTelemetry {
    /// A fresh telemetry sink (zero observations).
    pub fn new() -> AuthTelemetry {
        AuthTelemetry {
            count: AtomicU64::new(0),
        }
    }

    /// The FROZEN signal name this sink records under (row 1.8) — `auth_decision_latency`.
    pub const SIGNAL: &'static str = signals::AUTH_DECISION_LATENCY;

    /// Record ONE `auth_decision_latency` observation (called once per `authenticate`, on every
    /// path). On this floor we record the OCCURRENCE (the count); the latency-bucket histogram lands
    /// with the metrics-health-port binding — the named signal + the per-request emission are what
    /// the gate asserts.
    ///
    /// `pub(crate)` so BOTH authenticator bodies (the human/SSO [`HumanSsoAuthenticator`] and the
    /// machine-identity [`crate::machine_auth::CapabilityAuthenticator`]) emit through the SAME
    /// telemetry primitive — one signal seam, never two parallel counters (EI-01 §7).
    pub(crate) fn observe(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// The number of `auth_decision_latency` observations emitted (for the drill assertion).
    pub fn decision_count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

/// The IDOR survival counters the `authenticate` drill asserts (ID-3). Held alongside the body so a
/// drill can read `path_derived_tenant_count == 0` and `attempted_path_mismatch_count` directly.
#[derive(Debug, Default)]
pub struct IdorCounters {
    /// The number of requests whose effective tenant was taken from the URL PATH — **always 0**
    /// (the IDOR floor: tenant is from the credential, never the path).
    path_derived_tenant_count: AtomicU64,
    /// The number of requests where the URL path ASSERTED a different tenant than the credential (a
    /// rejected IDOR attempt — the guard held; the effective tenant was still the credential's).
    attempted_path_mismatch_count: AtomicU64,
}

impl IdorCounters {
    /// A fresh counter set.
    pub fn new() -> IdorCounters {
        IdorCounters::default()
    }

    /// The path-derived-tenant count (the IDOR floor asserts `== 0`).
    pub fn path_derived_tenant_count(&self) -> u64 {
        self.path_derived_tenant_count.load(Ordering::Relaxed)
    }

    /// The count of rejected path/credential tenant mismatches (attacks the guard caught).
    pub fn attempted_path_mismatch_count(&self) -> u64 {
        self.attempted_path_mismatch_count.load(Ordering::Relaxed)
    }

    /// Count a request whose effective tenant was (unexpectedly) path-derived — **must stay 0** (the
    /// IDOR floor). `pub(crate)` so the machine-identity body shares the SAME counter primitive (one
    /// IDOR seam, never two; EI-01 §7).
    pub(crate) fn count_path_derived(&self) {
        self.path_derived_tenant_count
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Count a rejected path/credential tenant mismatch (the guard held; the effective tenant was
    /// still the credential's/token's). `pub(crate)` for the shared machine-identity body.
    pub(crate) fn count_attempted_path_mismatch(&self) {
        self.attempted_path_mismatch_count
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// **The v1 human/SSO `authenticate` body (contract 4.1, the human/SSO half).** Resolves OIDC /
/// SAML / SCIM / passkey / SSH credentials to the one polymorphic [`Principal`] over the S1 store,
/// with **tenant-from-credential** (ID-3), per-request `auth_decision_latency` telemetry, and a
/// fail-closed disabled-principal posture. The capability-token + machine-identity half is P-ID-07
/// (P-066) — it extends this same surface behind the unchanged 4.1 signature.
pub struct HumanSsoAuthenticator {
    store: PrincipalStore,
    verifier: Arc<dyn CredentialVerifier>,
    telemetry: Arc<AuthTelemetry>,
    idor: Arc<IdorCounters>,
}

impl HumanSsoAuthenticator {
    /// Build the authenticator over the S1 [`PrincipalStore`] with the floor [`StructuralVerifier`].
    /// The real cryptographic verifier swaps in via [`Self::with_verifier`] without changing the
    /// resolution body.
    pub fn new(store: PrincipalStore) -> HumanSsoAuthenticator {
        HumanSsoAuthenticator::with_verifier(store, Arc::new(StructuralVerifier::new()))
    }

    /// Build the authenticator with an explicit [`CredentialVerifier`] (the seam the real
    /// OIDC/SAML/WebAuthn/SSH verifiers plug into — the named P5/P6 floor).
    pub fn with_verifier(
        store: PrincipalStore,
        verifier: Arc<dyn CredentialVerifier>,
    ) -> HumanSsoAuthenticator {
        HumanSsoAuthenticator {
            store,
            verifier,
            telemetry: Arc::new(AuthTelemetry::new()),
            idor: Arc::new(IdorCounters::new()),
        }
    }

    /// The per-request `auth_decision_latency` telemetry sink (row 1.8) — for the drill assertion.
    pub fn telemetry(&self) -> &AuthTelemetry {
        &self.telemetry
    }

    /// The IDOR survival counters (ID-3) — for the drill assertion (`path_derived_tenant_count == 0`).
    pub fn idor_counters(&self) -> &IdorCounters {
        &self.idor
    }

    /// **`authenticate(credential) → Principal` (contract 4.1, the human/SSO half).** The
    /// gateway-facing entry point: `path_tenant` is the tenant the URL path ASSERTED (if any) —
    /// observed only to count a rejected IDOR mismatch; the resolved `Principal.tenant` is ALWAYS
    /// the verified credential's (ID-3). Every call (success OR failure) emits exactly one
    /// `auth_decision_latency` observation.
    ///
    /// Resolution:
    /// 1. **verify** the credential → [`VerifiedAssertion`] (the trust-rooted tenant + subject key);
    /// 2. **tenant-from-credential** (never the path): build the [`TenantScope`] from the assertion's
    ///    tenant, then run the ONE IDOR primitive [`TenantScope::resolve`] over `path_tenant` (the
    ///    effective tenant is the credential's; `path_derived_tenant_count` stays 0);
    /// 3. **S1 SSO/SCIM-link lookup** within the verified scope → the [`Principal`];
    /// 4. **fail-closed** if the principal is `Suspended`/`Disabled` (SCIM-disable is the v1
    ///    authoritative revocation path — a deprovisioned principal never resolves to a session).
    pub fn authenticate(
        &self,
        credential: &Credential,
        path_tenant: Option<&TenantId>,
    ) -> myelin_identity::Result<Principal> {
        // (0) Observability is part of the pass: every decision (success OR failure) emits exactly
        //     one auth_decision_latency observation. Recorded FIRST so no early-return path can skip
        //     the per-request emission (the EI-01 §3 "emit a signal on every path" discipline).
        self.telemetry.observe();

        // (1) Verify the credential → the trust-rooted assertion. An unverifiable credential is a
        //     LOUD error (never a fabricated Principal).
        let assertion = self.verifier.verify(credential)?;

        // (2) THE IDOR FLOOR (ID-3): the tenant is the VERIFIED CREDENTIAL's, never the URL path.
        //     We build the scope from the credential's tenant, then run the ONE storage-tier IDOR
        //     primitive (TenantScope::resolve) over the path assertion — purely to COUNT a rejected
        //     mismatch; the effective tenant is unconditionally the credential's.
        let scope = self.scope_for(&assertion);
        let resolved = scope.resolve(path_tenant);
        debug_assert_eq!(
            resolved.tenant, assertion.tenant,
            "the effective tenant must be the verified credential's (ID-3)"
        );
        if resolved.path_derived {
            // Unreachable by construction (resolve never derives from the path) — but if a future
            // mutation broke that, COUNT it so the drill's `path_derived_tenant_count == 0` fails
            // loudly rather than a silent IDOR.
            self.idor
                .path_derived_tenant_count
                .fetch_add(1, Ordering::Relaxed);
        }
        if resolved.attempted_path_mismatch {
            // An attack was attempted and the guard held (the effective tenant is still the
            // credential's). Counted for the survival signal.
            self.idor
                .attempted_path_mismatch_count
                .fetch_add(1, Ordering::Relaxed);
        }

        // (3) Resolve the credential's subject key to a principal in the VERIFIED tenant directory
        //     (the S1 SSO/SCIM-link index). No cross-tenant lookup: a credential verified for tenant
        //     A resolves only into A's partition.
        let row = self
            .store
            .resolve_credential(&scope, &assertion.scheme, &assertion.subject_key)
            .ok_or_else(|| {
                AuthzError::FailClosed(format!(
                    "no `{}` principal mapped for the verified subject in tenant `{}` (unknown \
                     credential — fail-closed, never a fabricated session)",
                    assertion.scheme, assertion.tenant.0
                ))
            })?;

        // (4) Fail-closed on a deprovisioned principal. SCIM-disable is the v1 AUTHORITATIVE
        //     revocation path (architecture §4): a Suspended/Disabled principal does NOT resolve to
        //     an active session. The full S7 revocation list + the SCIM-disable wiring is P-ID-14.
        match row.status {
            PrincipalStatus::Active => {}
            PrincipalStatus::Suspended | PrincipalStatus::Disabled => {
                return Err(AuthzError::FailClosed(format!(
                    "principal `{}` is {:?} (SCIM-deprovisioned / suspended) — authenticate \
                     fail-closes (it never resolves to an active session); full revocation is \
                     P-ID-14",
                    row.principal_id.0, row.status
                )));
            }
        }

        // The one polymorphic Principal (architecture §3): the kind discriminant changes governance
        // metadata, never the authz code path. tenant/region are the VERIFIED credential's.
        Ok(Principal::new(
            assertion.tenant,
            assertion.region,
            row.principal_id,
            row.kind,
            row.data_role,
            row.status,
        ))
    }

    /// The frozen-4.1 trait form of `authenticate` (no `path_tenant`) — the version the
    /// `IdentityService` ABI exposes (a gateway that already stripped the path passes no path here).
    /// Delegates to the path-aware [`Self::authenticate`] with `path_tenant = None`; the IDOR floor
    /// is unchanged (tenant is still the credential's). Kept separate so the richer gateway-facing
    /// form can COUNT a path mismatch while the bare ABI form stays the frozen signature.
    pub fn authenticate_trait(
        &self,
        credential: &Credential,
    ) -> myelin_identity::Result<Principal> {
        self.authenticate(credential, None)
    }

    /// Mint the verified [`TenantScope`] from a [`VerifiedAssertion`] — the tenant + region are the
    /// credential's (the trust root). The only `TenantScope` constructor is
    /// `from_verified_token`, so a scope derived from a path is structurally impossible here.
    fn scope_for(&self, assertion: &VerifiedAssertion) -> TenantScope {
        // Construct a minimal verified Principal carrying the credential's tenant — the scope
        // constructor reads the tenant from the verified token, not a path/string.
        let token = Principal::stub(
            myelin_identity::PrincipalId(format!("cred:{}", assertion.subject_key)),
            myelin_identity::PrincipalKind::Human,
            assertion.tenant.clone(),
        );
        TenantScope::from_verified_token(&token, assertion.region.clone())
    }
}

/// **The frozen `IdentityService` ABI (contract 4.1–4.11) wired to the real human/SSO
/// `authenticate` body.** `authenticate` (4.1, the human/SSO half) is LANDED — this is the CDC
/// PROVIDER half of the 4.1 pair (a gateway-side consumer calls it through the trait). The other ten
/// methods are this prompt's NAMED floors (each landing in its own M1 prompt): `check` → P-ID-09,
/// `list_objects` → P-ID-11/12, `write_tuples` → P-ID-08, … They fail closed (`check` → `Deny`) or
/// return their named-floor `NotYetImplemented`, exactly as the shell's [`crate::FailClosedCheck`]
/// does — so wiring this authenticator into the service surface never silently opens an un-wired
/// gate. The capability-token / machine-identity half of 4.1 (P-ID-07) EXTENDS this same body.
impl IdentityService for HumanSsoAuthenticator {
    /// 4.1 (human/SSO half — LANDED). Resolves the credential through the real body (trait form, no
    /// path); the IDOR floor (tenant-from-credential) is unchanged.
    fn authenticate(&self, credential: &Credential) -> myelin_identity::Result<Principal> {
        self.authenticate_trait(credential)
    }

    /// 4.2 — fail-closed until P-ID-09 (the depth-bounded Zanzibar evaluation). Never fail-open.
    fn check(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        Ok(Decision::Deny)
    }

    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented(
            "list_objects → P-ID-11/12 (M1)",
        ))
    }

    fn list_subjects(
        &self,
        _object: &ObjectId,
        _permission: &Permission,
        _at: &Consistency,
    ) -> myelin_identity::Result<SubjectTree> {
        Err(AuthzError::NotYetImplemented(
            "list_subjects → P-ID-13 (M1)",
        ))
    }

    fn explain(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &ObjectId,
        _at: &Consistency,
    ) -> myelin_identity::Result<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("explain → P-ID-13 (M1)"))
    }

    fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
    ) -> myelin_identity::Result<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("delegation → P-ID-17 (M1)"))
    }

    fn write_tuples(
        &self,
        _deltas: &[TupleDelta],
        _precondition: Option<&Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        Err(AuthzError::NotYetImplemented("write_tuples → P-ID-08 (M1)"))
    }

    fn mint_run_token(
        &self,
        _agent_id: &PrincipalId,
        _run_id: &RunId,
        _delegation_caveats: &DelegationCaveats,
        _ttl: &FailStaticBound,
    ) -> myelin_identity::Result<RunToken> {
        Err(AuthzError::NotYetImplemented(
            "mint_run_token → P-ID-18 (M1)",
        ))
    }

    fn revoke(&self, _target: &RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("revoke → P-ID-14 (M1)"))
    }

    fn resolve_pseudonym(
        &self,
        _subject: &PrincipalId,
        _tenant: &TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented(
            "resolve_pseudonym → P-ID-19 (M1)",
        ))
    }

    fn erase(&self, _subject: &PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("erase → P-ID-20 (M1)"))
    }

    fn admit_fragment(
        &self,
        _fragment: &NamespaceFragment,
    ) -> myelin_identity::Result<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented(
            "admit_fragment → P-ID-10 (M1)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalKind};
    use myelin_storage::KmsEngine;

    fn store() -> PrincipalStore {
        PrincipalStore::new(Arc::new(KmsEngine::new()))
    }

    fn scope(tenant: &str, region: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region(region.into()))
    }

    /// The frozen verified-assertion envelope `<tenant>|<region>|<subject_key>` (the floor wire).
    fn material(tenant: &str, region: &str, subject_key: &str) -> String {
        format!("{tenant}|{region}|{subject_key}")
    }

    /// Seed an active principal in `tenant`/`region` and link a `scheme`/`subject_key` credential to
    /// it, returning the authenticator over the seeded store.
    fn seeded(
        scheme: &str,
        tenant: &str,
        region: &str,
        subject_key: &str,
        principal_id: &str,
        kind: PrincipalKind,
        status: PrincipalStatus,
    ) -> HumanSsoAuthenticator {
        let st = store();
        let sc = scope(tenant, region);
        st.put_principal(
            &sc,
            PrincipalId(principal_id.into()),
            kind,
            DataRole::Processor,
            status,
            None,
        )
        .unwrap();
        st.link_credential(&sc, scheme, subject_key, &PrincipalId(principal_id.into()))
            .unwrap();
        HumanSsoAuthenticator::new(st)
    }

    /// **One happy-path per credential kind resolves to the correct polymorphic Principal (4.1).**
    /// OIDC / SAML / SCIM / passkey / SSH each resolve to a `Principal{kind, tenant, region}` from
    /// S1 — the five v1 human/SSO surfaces.
    #[test]
    fn each_v1_human_sso_scheme_resolves_to_its_principal() {
        for s in scheme::HUMAN_SSO_SCHEMES {
            let auth = seeded(
                s,
                "acme",
                "eu-west",
                "subj-1",
                "p:alice",
                PrincipalKind::Human,
                PrincipalStatus::Active,
            );
            let p = auth
                .authenticate(
                    &Credential {
                        scheme: (*s).into(),
                        material: material("acme", "eu-west", "subj-1"),
                    },
                    None,
                )
                .unwrap_or_else(|e| panic!("scheme `{s}` should resolve: {e:?}"));
            assert_eq!(p.principal_id, PrincipalId("p:alice".into()), "scheme {s}");
            assert_eq!(
                p.tenant,
                TenantId("acme".into()),
                "scheme {s} tenant from credential"
            );
            assert_eq!(p.region, Region("eu-west".into()), "scheme {s} region");
            assert_eq!(p.kind, PrincipalKind::Human, "scheme {s} polymorphic kind");
        }
    }

    /// **Machine identity resolves to the SAME polymorphic record (architecture §3):** an SSH key
    /// can map a `Service`-kind principal. The kind discriminant changes governance metadata, never
    /// the resolution code path.
    #[test]
    fn ssh_can_resolve_a_service_kind_principal() {
        let auth = seeded(
            scheme::SSH,
            "acme",
            "eu-west",
            "SHA256:deadbeef",
            "svc:deploy",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        let p = auth
            .authenticate(
                &Credential {
                    scheme: scheme::SSH.into(),
                    material: material("acme", "eu-west", "SHA256:deadbeef"),
                },
                None,
            )
            .unwrap();
        assert_eq!(p.kind, PrincipalKind::Service);
    }

    /// **THE IDOR FLOOR (ID-3, the GATE drill): tenant comes from the credential, never the path.**
    /// A credential verified for tenant `acme` presented at a URL path asserting `globex` resolves
    /// to `acme` (the credential's), the `path_derived_tenant_count` is 0, and the rejected mismatch
    /// is counted. A mutation deriving tenant from the path would make `p.tenant == globex` here —
    /// the catch the prompt GATE requires.
    #[test]
    fn tenant_is_from_credential_not_the_url_path() {
        let auth = seeded(
            scheme::OIDC,
            "acme",
            "eu-west",
            "subj-1",
            "p:alice",
            PrincipalKind::Human,
            PrincipalStatus::Active,
        );
        // The URL path LIES (asserts globex); the credential is verified for acme.
        let p = auth
            .authenticate(
                &Credential {
                    scheme: scheme::OIDC.into(),
                    material: material("acme", "eu-west", "subj-1"),
                },
                Some(&TenantId("globex".into())),
            )
            .unwrap();
        assert_eq!(
            p.tenant,
            TenantId("acme".into()),
            "the resolved tenant is the CREDENTIAL's (acme), never the path's (globex)"
        );
        assert_eq!(
            auth.idor_counters().path_derived_tenant_count(),
            0,
            "path_derived_tenant_count == 0 (the IDOR floor — tenant never from the path)"
        );
        assert_eq!(
            auth.idor_counters().attempted_path_mismatch_count(),
            1,
            "the rejected IDOR attempt (path ≠ credential) was counted (the guard held)"
        );
    }

    /// **A credential verified for tenant A cannot resolve a principal in tenant B (no cross-tenant
    /// resolution).** Even though `p:alice` exists in `acme`, a credential verified for `globex`
    /// resolves into `globex`'s (empty) directory and fail-closes — it never reaches `acme`'s rows.
    #[test]
    fn credential_for_one_tenant_cannot_resolve_another_tenants_principal() {
        // Seed alice in acme.
        let st = store();
        let acme = scope("acme", "eu-west");
        st.put_principal(
            &acme,
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            DataRole::Processor,
            PrincipalStatus::Active,
            None,
        )
        .unwrap();
        st.link_credential(
            &acme,
            scheme::OIDC,
            "subj-1",
            &PrincipalId("p:alice".into()),
        )
        .unwrap();
        let auth = HumanSsoAuthenticator::new(st);

        // A credential VERIFIED for globex (a different tenant) presenting the same subject key
        // resolves into globex's directory — which has no such principal → fail-closed.
        let r = auth.authenticate(
            &Credential {
                scheme: scheme::OIDC.into(),
                material: material("globex", "eu-west", "subj-1"),
            },
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(_))),
            "a globex-verified credential cannot resolve acme's principal (no cross-tenant resolve)"
        );
    }

    /// **A SCIM-disabled / suspended principal fails closed (SCIM deprovision = the v1 authoritative
    /// revocation path, architecture §4).** A `Disabled` principal does NOT resolve to an active
    /// session — `authenticate` refuses it loudly (the full S7 revocation is P-ID-14).
    #[test]
    fn disabled_principal_fails_closed() {
        for status in [PrincipalStatus::Disabled, PrincipalStatus::Suspended] {
            let auth = seeded(
                scheme::SCIM,
                "acme",
                "eu-west",
                "ext-7",
                "p:bob",
                PrincipalKind::Human,
                status,
            );
            let r = auth.authenticate(
                &Credential {
                    scheme: scheme::SCIM.into(),
                    material: material("acme", "eu-west", "ext-7"),
                },
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::FailClosed(_))),
                "a {status:?} principal fails closed (never an active session)"
            );
        }
    }

    /// **`auth_decision_latency` is emitted per request — on EVERY path (success AND failure).**
    /// Observability is part of the pass (EI-01 §3): an auth decision that emits no signal has failed
    /// the gate. The signal is keyed by the FROZEN name constant (row 1.8), never a literal.
    #[test]
    fn auth_decision_latency_emits_once_per_request_on_every_path() {
        let auth = seeded(
            scheme::PASSKEY,
            "acme",
            "eu-west",
            "cred-id-9",
            "p:carol",
            PrincipalKind::Human,
            PrincipalStatus::Active,
        );
        assert_eq!(auth.telemetry().decision_count(), 0);
        // A success.
        auth.authenticate(
            &Credential {
                scheme: scheme::PASSKEY.into(),
                material: material("acme", "eu-west", "cred-id-9"),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            auth.telemetry().decision_count(),
            1,
            "success emits one observation"
        );
        // A FAILURE (unknown subject) STILL emits — every decision is observed.
        let _ = auth.authenticate(
            &Credential {
                scheme: scheme::PASSKEY.into(),
                material: material("acme", "eu-west", "no-such-subject"),
            },
            None,
        );
        assert_eq!(
            auth.telemetry().decision_count(),
            2,
            "a failed decision ALSO emits one observation (signal on every path)"
        );
        // The frozen signal name is the row-1.8 constant, never a literal.
        assert_eq!(AuthTelemetry::SIGNAL, "auth_decision_latency");
        assert_eq!(AuthTelemetry::SIGNAL, signals::AUTH_DECISION_LATENCY);
    }

    /// **A capability-token / machine-identity scheme is REFUSED by this body (it is P-ID-07's).**
    /// The human/SSO authenticator owns only the five v1 SSO surfaces; a `pat`/`agent`/`deploy_key`
    /// credential is refused loudly (never silently mis-resolved through the wrong authenticator).
    #[test]
    fn capability_token_scheme_is_refused_here() {
        let auth = HumanSsoAuthenticator::new(store());
        for s in ["pat", "ci", "agent", "deploy_key"] {
            let r = auth.authenticate(
                &Credential {
                    scheme: s.into(),
                    material: material("acme", "eu-west", "x"),
                },
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::BadRequest(_))),
                "scheme `{s}` is P-ID-07's (machine identity), refused by the human/SSO body"
            );
        }
    }

    /// **A malformed verified-assertion envelope is refused (never a partial/empty assertion).** A
    /// credential whose material is not `<tenant>|<region>|<subject_key>` (all non-empty) is a loud
    /// `BadRequest` — and STILL emits its telemetry observation.
    #[test]
    fn malformed_assertion_is_refused_loudly() {
        let auth = HumanSsoAuthenticator::new(store());
        for bad in ["", "acme", "acme|eu-west", "acme||subj", "|eu-west|subj"] {
            let r = auth.authenticate(
                &Credential {
                    scheme: scheme::OIDC.into(),
                    material: bad.into(),
                },
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::BadRequest(_))),
                "malformed envelope `{bad}` is refused"
            );
        }
        assert_eq!(
            auth.telemetry().decision_count(),
            5,
            "every refused decision still emitted its observation"
        );
    }

    /// **An unknown credential fail-closes (never a fabricated session).** A well-formed credential
    /// for a tenant with no such SSO/SCIM link resolves to a loud `FailClosed`, not a synthesised
    /// principal.
    #[test]
    fn unknown_credential_fails_closed() {
        let auth = HumanSsoAuthenticator::new(store());
        let r = auth.authenticate(
            &Credential {
                scheme: scheme::SAML.into(),
                material: material("acme", "eu-west", "ghost"),
            },
            None,
        );
        assert!(matches!(r, Err(AuthzError::FailClosed(_))));
    }
}
