//! # `machine_auth` — the capability-token + machine-identity credential set (P-ID-07 → P-066)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §3 (the machine-identity pin C6: every machine credential resolves to the SAME polymorphic
//! `Principal` — SSH-pubkey / repo-scoped deploy-key / PAT / per-job token; **a self-hosted runner
//! token is scoped to ONE tenant's `SelfHosted` jobs** and cannot mint/act cross-tenant), §4 (the
//! token format: capability tokens are **attenuable bearer tokens** — PASETO v4 / JWT envelope —
//! whose authority is a **macaroon/biscuit caveat chain**: offline, client-side, **monotone
//! attenuation**; **DPoP** sender-constrains long-lived PATs; revocation = **denylist (S7) + short
//! TTL** where the TTL is the fail-static staleness ceiling).
//!
//! **Reconciliation (00 §1, C6):** the self-hosted-runner token scope (one tenant's `SelfHosted`
//! jobs), the deploy-key repo-scoped machine principal, and the per-job-token mid-resume re-mint are
//! frozen on 4.1/4.7.
//!
//! **Contract-index:** row 4.1 `authenticate(credential) → Principal` (the **token/machine-identity
//! half** — OWNED here, extending the P-ID-06 human/SSO half behind the unchanged 4.1 signature);
//! row 11.1 (the S1 token records this body resolves) — CONSUMED.
//!
//! ## What this module ships (P-ID-07 — the token/machine-identity half of `authenticate`)
//! [`CapabilityAuthenticator::authenticate`] resolves the **three capability-token types** (PAT,
//! CI-job, agent-run) and the **two machine-identity surfaces** (repo-scoped deploy-key, per-job /
//! self-hosted-runner token) — each to the ONE polymorphic [`Principal`] backed by the S1
//! [`crate::principal_store::PrincipalStore`]. The pipeline is:
//!
//! ```text
//! credential ──verify envelope──▶ CapabilityToken{ tenant, subject_key, kind, authority, dpop?, jti }
//!            ──tenant-from-token (NEVER the URL path, ID-3)──▶ TenantScope
//!            ──S7 denylist check (revoked? → fail-closed)──▶ live token
//!            ──S1 token-record lookup──▶ PrincipalRow ──▶ Principal{kind=Service|…, …}
//!            └─authority ceiling stamped (repo-scope / SelfHosted one-tenant scope)
//!            └─emit auth_decision_latency (per request)
//! ```
//!
//! ## The two load-bearing scope invariants (mutation-tested mandatory-core, per the prompt GATE)
//! - **A self-hosted-runner / per-job token cannot resolve a cross-tenant Principal** (§3, C6, ties
//!   to Tenancy 12.4 no-global-pool). The token's `tenant` is the trust root; the resolved
//!   `Principal.tenant` is ALWAYS the token's, and the [`Authority`] it carries is bounded to that
//!   one tenant's `SelfHosted` jobs. A mutation that derived the tenant from anywhere else (the
//!   path, a second token field) or widened a runner authority beyond its one tenant MUST be caught.
//! - **Attenuation is MONOTONE — a caveat chain only ever NARROWS authority, never amplifies** (§4,
//!   the macaroon/biscuit law). [`Authority::attenuate`] is a set INTERSECTION: the attenuated
//!   authority is a strict subset of (or equal to) its parent. A mutation turning intersection into
//!   union (so attenuation grew authority) MUST be caught.
//!
//! ## Floors named (frozen shape now → bodies in a later prompt / parallel track)
//! - **Revocation routes through the DURABLE [`crate::revocation::RevocationStore`] (MR-011 — the
//!   carried-forward S7Denylist fix is DISCHARGED).** `authenticate` consults the same `(tenant,
//!   region)`-partitioned, PG-backed (`with_pg`) store every surface shares: a revoked `jti` denies
//!   AND the denial survives a restart (proven cross-restart under `--features integration`). The old
//!   tenant-less in-memory `S7Denylist` (a bare `Mutex<BTreeSet>` rebuilt empty on construction — a
//!   token revoked there re-validated after restart) is REMOVED; the durable store is the source of
//!   truth. Fail-closed: a revocation-store read error denies.
//! - **The per-job-token mid-resume RE-MINT is P-ID-17 (`mint_run_token`, P-076).** This body
//!   resolves a per-job token and stamps its one-tenant `SelfHosted` ceiling; the mid-workflow
//!   re-mint on resume (token life == activity life) is the named follow-on (`mint_run_token`).
//! - **Cryptographic token VERIFICATION (PASETO v4 sign/verify, JWT/JWKS, the biscuit caveat-chain
//!   crypto, DPoP RFC 9449 proof-of-possession) is modelled at the structural seam, not the wire**
//!   — the EI-01 §1 documented deviation, the SAME posture the human/SSO [`crate::authenticate`]
//!   body documents for its [`crate::authenticate::CredentialVerifier`]. What this body ships — and
//!   proves — is the AUTHORIZATION-relevant logic: **tenant-from-token** (never the path), the
//!   **monotone caveat-chain attenuation**, the **deploy-key repo ceiling**, the **self-hosted
//!   one-tenant scope**, the **DPoP binding presence** for long-lived PATs, the **denylist + TTL
//!   revocation consult**, the **S1 token-record resolution**, and the **per-request telemetry**.
//!   The real crypto verifier swaps in behind the [`TokenVerifier`] seam without changing this body.

use crate::authenticate::{scheme as human_scheme, AuthTelemetry, IdorCounters};
use crate::principal_store::PrincipalStore;
use crate::revocation::{RevocationStore, RunTokenState};
use myelin_events::Timestamp;
use myelin_identity::{
    AuthzError, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RevokeTarget,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The default wall-clock "now" for the revocation consult, as an RFC-3339 instant. Injected via
/// [`CapabilityAuthenticator::with_clock`] in tests/drills; the production default reads the system
/// clock (chrono is the parse/format-only dep already in the workspace — no `clock` feature needed for
/// the epoch→RFC-3339 conversion). The consult only needs `now` to honour a per-run-token TTL; a plain
/// `revoke(jti)` (no TTL) denies regardless of `now`.
fn system_now_ts() -> Timestamp {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let dt = chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default();
    Timestamp(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// The injected "now" source (RFC-3339) for the revocation consult.
type NowFn = Arc<dyn Fn() -> Timestamp + Send + Sync>;

/// The capability-token / machine-identity credential schemes this body resolves (architecture §3/§4).
/// A scheme is a `&'static str` (matching the frozen [`myelin_identity::Credential::scheme`]
/// free-string carrier, P-ID-01) so a new surface is an additive change. The five human/SSO schemes
/// (`oidc`/`saml`/`scim`/`passkey`/`ssh`) are P-ID-06's ([`crate::authenticate::scheme`]).
pub mod scheme {
    /// A scoped **Personal Access Token** — a Human or Service with capability caveats
    /// (attenuate-only, §6 delegation algebra). DPoP sender-constrains a long-lived PAT (§4).
    pub const PAT: &str = "pat";
    /// A **CI-job token** — a `kind = service` Principal minted per run through Id; a self-hosted
    /// runner token is scoped to one tenant's `SelfHosted` jobs (§3, C6).
    pub const CI: &str = "ci";
    /// A **per-run agent token** — a `kind = service` Principal carrying the run's attenuated caveat
    /// chain; re-mintable mid-workflow on resume (`mint_run_token`, P-ID-17 floor).
    pub const AGENT: &str = "agent";
    /// A **repo-scoped deploy key** → a `kind = service` machine principal whose authority ceiling is
    /// ONE repo (§3, C6) — never a project-wide grant.
    pub const DEPLOY_KEY: &str = "deploy_key";
    /// A **per-job / self-hosted-runner token** → a `kind = service` Principal scoped to one tenant's
    /// `SelfHosted` jobs; cannot mint/act cross-tenant (§3, C6; the no-global-pool property).
    pub const PER_JOB: &str = "per_job";

    /// The complete token/machine-identity scheme set (the surfaces this prompt ships).
    pub const MACHINE_SCHEMES: &[&str] = &[PAT, CI, AGENT, DEPLOY_KEY, PER_JOB];

    /// Is `s` one of the five token/machine-identity schemes this body owns? (A human/SSO scheme is
    /// P-ID-06's — this body refuses it with `BadRequest`.)
    pub fn is_machine(s: &str) -> bool {
        MACHINE_SCHEMES.contains(&s)
    }
}

/// The machine-identity kind discriminant (architecture §3, C6) — which of the four pinned shapes a
/// resolved credential is. It selects the **authority ceiling** the body stamps; it does NOT change
/// the polymorphic [`Principal`] code path (every shape resolves to the same record, §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineKind {
    /// A scoped Personal Access Token (Human or Service; DPoP-bound when long-lived).
    Pat,
    /// A CI-job token (a per-run Service principal).
    Ci,
    /// A per-run agent token (a per-run Service principal carrying the run's caveat chain).
    Agent,
    /// A repo-scoped deploy key (a Service principal whose ceiling is one repo).
    DeployKey,
    /// A per-job / self-hosted-runner token (a Service principal scoped to one tenant's
    /// `SelfHosted` jobs).
    PerJob,
}

/// The signed purpose of a capability credential. This is distinct from both the transport scheme
/// and [`PrincipalKind`]: an operator bootstrap credential and a delegated agent-run credential use
/// the `agent` verifier, but have deliberately different authorization semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialPurpose {
    /// A human-operated bootstrap credential minted by the offline operator command.
    OperatorBootstrap,
    /// A short-lived delegated credential bound to one durable run.
    AgentRun {
        run_id: String,
        /// Durable delegation-policy snapshot used to resolve the four authority conjuncts. Older
        /// caller-supplied mint paths leave this absent and are refused at an authorization surface.
        delegation_snapshot: Option<i64>,
    },
    /// A DPoP-bound personal access token.
    Pat,
    /// A CI job credential.
    CiJob,
    /// A repository deploy key credential.
    DeployKey,
    /// A self-hosted per-job credential.
    PerJob,
}

impl CredentialPurpose {
    /// The stable signed claim token for this purpose.
    pub fn claim(&self) -> &'static str {
        match self {
            CredentialPurpose::OperatorBootstrap => "operator_bootstrap",
            CredentialPurpose::AgentRun { .. } => "agent_run",
            CredentialPurpose::Pat => "pat",
            CredentialPurpose::CiJob => "ci_job",
            CredentialPurpose::DeployKey => "deploy_key",
            CredentialPurpose::PerJob => "per_job",
        }
    }

    /// The machine verifier kind this signed purpose is valid under.
    pub fn machine_kind(&self) -> MachineKind {
        match self {
            CredentialPurpose::OperatorBootstrap | CredentialPurpose::AgentRun { .. } => {
                MachineKind::Agent
            }
            CredentialPurpose::Pat => MachineKind::Pat,
            CredentialPurpose::CiJob => MachineKind::Ci,
            CredentialPurpose::DeployKey => MachineKind::DeployKey,
            CredentialPurpose::PerJob => MachineKind::PerJob,
        }
    }

    /// Whether this is the durable run-bound credential shape.
    pub fn is_agent_run(&self) -> bool {
        matches!(self, CredentialPurpose::AgentRun { .. })
    }
}

/// The signed service audience. A token minted for a future MCP endpoint cannot silently be replayed
/// at the ordinary product edge, and vice versa.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialAudience {
    /// The HTTP product and Git edge.
    Edge,
    /// The governed MCP endpoint (not mounted yet).
    Mcp,
}

impl CredentialAudience {
    /// The stable signed claim token.
    pub fn claim(self) -> &'static str {
        match self {
            CredentialAudience::Edge => "edge",
            CredentialAudience::Mcp => "mcp",
        }
    }
}

/// Sender-constraint state retained after verification. A handler never reparses an untrusted DPoP
/// header to recover this decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpopState {
    /// The signed credential was not sender-constrained.
    Unbound,
    /// The verifier validated the request proof against the signed key binding.
    Verified,
}

impl MachineKind {
    /// Parse the credential scheme to its machine kind (or `None` for a non-machine scheme). Public so
    /// the real [`crate::capability_crypto::PasetoCapabilityVerifier`] reads the kind from the scheme
    /// the SAME way the structural floor does (the kind is not in the signed body — MR-011b hardening).
    pub fn from_scheme(s: &str) -> Option<MachineKind> {
        match s {
            scheme::PAT => Some(MachineKind::Pat),
            scheme::CI => Some(MachineKind::Ci),
            scheme::AGENT => Some(MachineKind::Agent),
            scheme::DEPLOY_KEY => Some(MachineKind::DeployKey),
            scheme::PER_JOB => Some(MachineKind::PerJob),
            _ => None,
        }
    }

    /// Is this a **per-job / self-hosted-runner** token — the one-tenant `SelfHosted` scope (C6)?
    pub fn is_self_hosted_runner(self) -> bool {
        matches!(self, MachineKind::PerJob)
    }
}

/// **An authority — the SET of capability grants a token carries (architecture §4, the
/// macaroon/biscuit model).** Authority is the thing attenuation NARROWS: a child token's authority
/// is a SUBSET of its parent's. Modelled as an ordered set of opaque grant strings (e.g.
/// `"repo:acme/web#write"`, `"selfhosted:acme"`) — the load-bearing property is the SET ALGEBRA
/// (intersection narrows, never widens), not the grant grammar (the full ABAC caveat language is the
/// `CaveatContext`/`QueryAst` core, P-ID-09/P-ID-22).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Authority {
    grants: BTreeSet<String>,
}

impl Authority {
    /// An authority over an explicit grant set.
    pub fn of<I, S>(grants: I) -> Authority
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Authority {
            grants: grants.into_iter().map(Into::into).collect(),
        }
    }

    /// The grants this authority carries (sorted, deduplicated).
    pub fn grants(&self) -> impl Iterator<Item = &str> {
        self.grants.iter().map(String::as_str)
    }

    /// How many grants this authority carries (for the attenuation-monotone assertion).
    pub fn len(&self) -> usize {
        self.grants.len()
    }

    /// Is this authority empty (grants nothing)?
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    /// Does this authority hold `grant`?
    pub fn holds(&self, grant: &str) -> bool {
        self.grants.contains(grant)
    }

    /// **Attenuate by a caveat — `self ∩ requested` (architecture §4: MONOTONE attenuation).** The
    /// caveat (a child token, a delegated narrowing) can only KEEP grants the parent already had; a
    /// grant the parent never held is dropped (never minted). The result is therefore a SUBSET of
    /// `self` — attenuation can shrink authority to nothing, but it can NEVER amplify it. This is the
    /// macaroon/biscuit law the GATE mutation-tests: a mutation turning this `∩` into a `∪` would let
    /// a caveat ADD authority, breaking the security floor.
    pub fn attenuate(&self, requested: &Authority) -> Authority {
        Authority {
            grants: self
                .grants
                .intersection(&requested.grants)
                .cloned()
                .collect(),
        }
    }

    /// Is `self` a subset of (no wider than) `parent`? The post-condition every attenuation step
    /// upholds (the monotone law made assertable).
    pub fn is_subset_of(&self, parent: &Authority) -> bool {
        self.grants.is_subset(&parent.grants)
    }
}

/// **A verified capability token — the trust-rooted facts a [`TokenVerifier`] extracts from a
/// presented capability/machine credential (architecture §3/§4).**
///
/// `tenant` is the load-bearing field: it is the tenant the token was MINTED for — the trust root for
/// the whole request, NEVER the URL path (ID-3). `authority` is the (already-attenuated) capability
/// set the token carries. `jti` is the token's unique id (the denylist/revocation handle, §4).
/// `dpop_bound` records whether a long-lived PAT is DPoP sender-constrained (RFC 9449, §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityToken {
    /// The tenant the token was minted for (the trust root — never the URL path, ID-3).
    pub tenant: TenantId,
    /// The residency region the principal is pinned to (`(tenant, region)`, 12.1).
    pub region: Region,
    /// The machine-identity kind (selects the authority ceiling the body stamps).
    pub kind: MachineKind,
    /// The S1 token-record subject key (the deploy-key fingerprint, the PAT id, the run id, …).
    pub subject_key: String,
    /// The capability set the token carries (already client-side attenuated, §4).
    pub authority: Authority,
    /// The token's unique id — the revocation/denylist handle (§4, S7).
    pub jti: String,
    /// Is this (long-lived PAT) token DPoP sender-constrained (RFC 9449, §4)? Required for a PAT;
    /// `false`/irrelevant for the short-lived per-run tokens (their TTL is the constraint).
    pub dpop_bound: bool,
    /// The signed credential purpose (never inferred from an HTTP header).
    pub purpose: CredentialPurpose,
    /// The signed service audience.
    pub audience: CredentialAudience,
    /// The signed outer expiry as a Unix instant. It was checked by the verifier and remains
    /// available to downstream final-boundary policy and audit code.
    pub exp_unix: i64,
}

/// The verified capability facts retained through the request lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedCapabilityContext {
    pub purpose: CredentialPurpose,
    pub audience: CredentialAudience,
    pub jti: String,
    pub effective_authority: Authority,
    pub expires_at_unix: i64,
    pub dpop: DpopState,
}

/// The credential context associated with a request. Human sessions can become an additive variant;
/// capability authentication never collapses to a bare principal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialContext {
    Capability(VerifiedCapabilityContext),
}

/// The trusted request identity passed from authentication through action and object authorization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestIdentity {
    pub principal: Principal,
    pub scope: TenantScope,
    pub credential: CredentialContext,
}

impl RequestIdentity {
    /// The verified capability facts for this request.
    pub fn capability(&self) -> &VerifiedCapabilityContext {
        match &self.credential {
            CredentialContext::Capability(capability) => capability,
        }
    }
}

/// The pluggable token-verification seam (the EI-01 §1 named floor — the SAME posture as the
/// human/SSO [`crate::authenticate::CredentialVerifier`]). A verifier turns a presented
/// [`myelin_identity::Credential`] into a trust-rooted [`CapabilityToken`] — OR refuses it loudly.
/// The REAL crypto verifiers (PASETO v4 / JWT-JWKS / biscuit caveat-chain / DPoP proof) implement
/// this trait and swap in behind the SAME seam; this body's resolution + scope logic does not change.
/// The floor implementation is [`StructuralTokenVerifier`].
pub trait TokenVerifier: Send + Sync {
    /// Verify `credential` and extract its trust-rooted capability token, or refuse it. A refusal is
    /// a LOUD [`AuthzError`] (never a fabricated/empty token — an unverifiable token does not resolve
    /// to a Principal). The tenant in the returned token is the token's, never a caller-supplied path.
    fn verify(
        &self,
        credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<CapabilityToken>;
}

/// **The floor token verifier (the EI-01 §1 documented deviation).** It parses the frozen verified-
/// token envelope from the credential's opaque [`myelin_identity::Credential::material`] — the
/// structural model of "the token's signature/caveat-chain verified and asserts these facts". The
/// real cryptographic verification (PASETO sign, biscuit caveat crypto, DPoP proof) is the named
/// P5/P6 floor; this verifier proves the AUTHORIZATION-relevant path without pretending to do crypto.
///
/// ## The frozen verified-token envelope (the floor wire shape)
/// `material = "<tenant>|<region>|<subject_key>|<jti>|<dpop:0|1>|<grant>,<grant>,…"` — six
/// `|`-separated fields, the last a comma-separated grant list (possibly empty). A malformed envelope
/// is refused ([`AuthzError::BadRequest`]) — never coerced into a partial/empty token.
#[derive(Clone, Copy, Debug, Default)]
pub struct StructuralTokenVerifier;

impl StructuralTokenVerifier {
    /// A fresh floor token verifier.
    pub fn new() -> StructuralTokenVerifier {
        StructuralTokenVerifier
    }
}

impl TokenVerifier for StructuralTokenVerifier {
    fn verify(
        &self,
        credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<CapabilityToken> {
        // This body owns ONLY the five token/machine schemes; a human/SSO scheme belongs to P-ID-06
        // and is refused here loudly (never silently mis-resolved through the wrong authenticator).
        let kind = MachineKind::from_scheme(&credential.scheme).ok_or_else(|| {
            // Make the human/SSO redirection explicit when the scheme is a known human surface.
            if human_scheme::is_human_sso(&credential.scheme) {
                AuthzError::BadRequest(format!(
                    "scheme `{}` is a v1 human/SSO surface (P-ID-06), not a capability-token / \
                     machine-identity surface (pat/ci/agent/deploy_key/per_job)",
                    credential.scheme
                ))
            } else {
                AuthzError::BadRequest(format!(
                    "scheme `{}` is not a capability-token / machine-identity surface \
                     (pat/ci/agent/deploy_key/per_job)",
                    credential.scheme
                ))
            }
        })?;

        // Parse the frozen verified-token envelope. The real PASETO/biscuit/DPoP verification is the
        // named floor; this is the structural stand-in.
        let parts: Vec<&str> = credential.material.split('|').collect();
        if parts.len() != 10 {
            return Err(AuthzError::BadRequest(
                "malformed verified-token envelope (expected \
                 `<tenant>|<region>|<subject_key>|<jti>|<dpop:0|1>|<grants>|<purpose>|<aud>|<run_id>|<delegation_snapshot>`)"
                    .into(),
            ));
        }
        let (
            tenant,
            region,
            subject_key,
            jti,
            dpop,
            grants_csv,
            purpose,
            audience,
            run_id,
            delegation_snapshot,
        ) = (
            parts[0], parts[1], parts[2], parts[3], parts[4], parts[5], parts[6], parts[7],
            parts[8], parts[9],
        );
        if tenant.is_empty() || region.is_empty() || subject_key.is_empty() || jti.is_empty() {
            return Err(AuthzError::BadRequest(
                "malformed verified-token envelope: tenant/region/subject_key/jti must be non-empty"
                    .into(),
            ));
        }
        let dpop_bound = match dpop {
            "1" => true,
            "0" => false,
            other => {
                return Err(AuthzError::BadRequest(format!(
                    "malformed DPoP flag `{other}` (expected `0` or `1`)"
                )))
            }
        };
        // The grant list (comma-separated; empty string ⇒ no grants).
        let authority = if grants_csv.is_empty() {
            Authority::default()
        } else {
            Authority::of(grants_csv.split(',').map(str::to_string))
        };
        let purpose = match purpose {
            "operator_bootstrap" => CredentialPurpose::OperatorBootstrap,
            "agent_run" if !run_id.is_empty() => CredentialPurpose::AgentRun {
                run_id: run_id.to_string(),
                delegation_snapshot: if delegation_snapshot.is_empty() {
                    None
                } else {
                    Some(delegation_snapshot.parse::<i64>().map_err(|_| {
                        AuthzError::BadRequest(
                            "signed delegation snapshot must be an integer".into(),
                        )
                    })?)
                },
            },
            "pat" => CredentialPurpose::Pat,
            "ci_job" => CredentialPurpose::CiJob,
            "deploy_key" => CredentialPurpose::DeployKey,
            "per_job" => CredentialPurpose::PerJob,
            "test_kind" => match kind {
                MachineKind::Pat => CredentialPurpose::Pat,
                MachineKind::Ci => CredentialPurpose::CiJob,
                MachineKind::Agent => CredentialPurpose::OperatorBootstrap,
                MachineKind::DeployKey => CredentialPurpose::DeployKey,
                MachineKind::PerJob => CredentialPurpose::PerJob,
            },
            other => {
                return Err(AuthzError::BadRequest(format!(
                    "unknown or incomplete signed credential purpose `{other}`"
                )))
            }
        };
        if purpose.machine_kind() != kind {
            return Err(AuthzError::FailClosed(format!(
                "credential scheme kind `{kind:?}` does not match signed purpose `{}`",
                purpose.claim()
            )));
        }
        let audience = match audience {
            "edge" => CredentialAudience::Edge,
            "mcp" => CredentialAudience::Mcp,
            other => {
                return Err(AuthzError::BadRequest(format!(
                    "unknown signed credential audience `{other}`"
                )))
            }
        };

        Ok(CapabilityToken {
            tenant: TenantId(tenant.to_string()),
            region: Region(region.to_string()),
            kind,
            subject_key: subject_key.to_string(),
            authority,
            jti: jti.to_string(),
            dpop_bound,
            purpose,
            audience,
            exp_unix: i64::MAX,
        })
    }
}

/// The grant prefix a repo-scoped deploy key's authority is ceiling-bounded to (`"repo:"`). A deploy
/// key's authority may name only grants UNDER its one repo (architecture §3, C6) — the repo-scope
/// ceiling this body enforces.
const REPO_GRANT_PREFIX: &str = "repo:";
/// The grant prefix a self-hosted-runner / per-job token's authority is ceiling-bounded to
/// (`"selfhosted:<tenant>"`). A runner token may name only its OWN tenant's `SelfHosted` scope
/// (architecture §3, C6) — the no-global-pool property at the identity layer.
const SELFHOSTED_GRANT_PREFIX: &str = "selfhosted:";

/// **The capability-token + machine-identity `authenticate` body (contract 4.1, the token/machine
/// half).** Resolves PAT / CI-job / agent-run capability tokens and deploy-key / per-job machine
/// identities to the one polymorphic [`Principal`] over the S1 store, with **tenant-from-token**
/// (ID-3), **monotone caveat-chain attenuation**, the **deploy-key repo ceiling**, the
/// **self-hosted-runner one-tenant scope** (C6), the **DPoP-binding requirement** for long-lived
/// PATs, the **durable S7 revocation consult** (the `(tenant, region)`-partitioned
/// [`RevocationStore`], MR-008 — a revoked `jti` denies AND survives restart), and per-request
/// `auth_decision_latency` telemetry. Extends the same 4.1 surface the human/SSO
/// [`crate::authenticate::HumanSsoAuthenticator`] owns — the frozen signature is unchanged.
pub struct CapabilityAuthenticator {
    store: PrincipalStore,
    verifier: Arc<dyn TokenVerifier>,
    /// The DURABLE S7 revocation store (MR-008/MR-011) — the source of truth the consult denies on.
    /// Shared (cloneable) with the mint/teardown side so a revoke from any surface denies here.
    revocations: RevocationStore,
    /// The injected "now" (RFC-3339) for the revocation TTL consult (default: system clock).
    now: NowFn,
    telemetry: Arc<AuthTelemetry>,
    idor: Arc<IdorCounters>,
}

impl CapabilityAuthenticator {
    /// **TEST-DOUBLE constructor (`#[cfg(test)]`, MR-012).** Builds the authenticator over the S1
    /// [`PrincipalStore`] with the mock floor [`StructuralTokenVerifier`] and a fresh in-memory
    /// [`RevocationStore`]. This forgeable-envelope default is NOT in the production graph — production
    /// builds the REAL [`crate::capability_crypto::PasetoCapabilityVerifier`] (PASETO v4 / Ed25519,
    /// from the cell trust anchor) via [`Self::with_verifier`]. The `no-structural-crypto-in-prod`
    /// scanner admits this construction because it is `#[cfg(test)]`-gated.
    #[cfg(test)]
    pub fn new(store: PrincipalStore) -> CapabilityAuthenticator {
        CapabilityAuthenticator::with_verifier(
            store,
            Arc::new(StructuralTokenVerifier::new()),
            RevocationStore::new(),
        )
    }

    /// Build the authenticator with an explicit [`TokenVerifier`] + the durable [`RevocationStore`]
    /// (the seams the real PASETO/macaroon/DPoP verifier and the PG-backed S7 store plug into). The
    /// revocation store is the SAME one the mint/teardown side writes to, so a revoke is consulted
    /// here — and, with `RevocationStore::with_pg`, the denial survives a restart.
    pub fn with_verifier(
        store: PrincipalStore,
        verifier: Arc<dyn TokenVerifier>,
        revocations: RevocationStore,
    ) -> CapabilityAuthenticator {
        CapabilityAuthenticator {
            store,
            verifier,
            revocations,
            now: Arc::new(system_now_ts),
            telemetry: Arc::new(AuthTelemetry::new()),
            idor: Arc::new(IdorCounters::new()),
        }
    }

    /// Inject a deterministic clock (RFC-3339 "now") for the revocation TTL consult — the test / drill
    /// seam (the production default reads the system clock).
    pub fn with_clock(
        mut self,
        now: impl Fn() -> Timestamp + Send + Sync + 'static,
    ) -> CapabilityAuthenticator {
        self.now = Arc::new(now);
        self
    }

    /// The per-request `auth_decision_latency` telemetry sink (row 1.8) — for the drill assertion.
    pub fn telemetry(&self) -> &AuthTelemetry {
        &self.telemetry
    }

    /// The IDOR survival counters (ID-3) — for the drill assertion (`path_derived_tenant_count == 0`).
    pub fn idor_counters(&self) -> &IdorCounters {
        &self.idor
    }

    /// The DURABLE S7 revocation store (MR-008/MR-011) — to revoke a `jti` (and prove the cross-restart
    /// denial) in a drill, and the SAME store the mint/teardown side shares.
    pub fn revocations(&self) -> &RevocationStore {
        &self.revocations
    }

    /// **`authenticate(credential) → Principal` (contract 4.1, the token/machine half).** `path_tenant`
    /// is the tenant the URL path ASSERTED (if any) — observed only to count a rejected IDOR mismatch;
    /// the resolved `Principal.tenant` is ALWAYS the verified token's (ID-3). Every call (success OR
    /// failure) emits exactly one `auth_decision_latency` observation.
    ///
    /// Resolution:
    /// 1. **verify** the credential → [`CapabilityToken`] (the trust-rooted tenant + authority + jti);
    /// 2. **durable revocation consult** — a revoked `jti` (the durable [`RevocationStore`]) fail-closes
    ///    (never a live session); the consult is `(tenant, region)`-scoped from the VERIFIED token, and
    ///    a revocation-store read error denies (fail-closed). With `with_pg`, the deny survives restart;
    /// 3. **DPoP requirement** — a long-lived PAT MUST be DPoP sender-constrained (§4);
    /// 4. **tenant-from-token** (never the path): build the [`TenantScope`] from the token's tenant,
    ///    run the ONE IDOR primitive [`TenantScope::resolve`] over `path_tenant` (the effective tenant
    ///    is the token's; `path_derived_tenant_count` stays 0);
    /// 5. **authority ceiling** — a deploy key's authority is repo-scoped; a self-hosted-runner /
    ///    per-job token's authority is bounded to its OWN tenant's `SelfHosted` jobs (cross-tenant
    ///    resolution count = 0, C6);
    /// 6. **S1 token-record lookup** within the verified scope → the [`Principal`];
    /// 7. **fail-closed** if the principal is `Suspended`/`Disabled`.
    pub fn authenticate_identity(
        &self,
        credential: &myelin_identity::Credential,
        path_tenant: Option<&TenantId>,
    ) -> myelin_identity::Result<RequestIdentity> {
        // (0) Observability is part of the pass: every decision (success OR failure) emits exactly
        //     one auth_decision_latency observation, recorded FIRST so no early return can skip it.
        self.telemetry.observe();

        // (1) Verify the credential → the trust-rooted token. An unverifiable token is a LOUD error.
        let token = self.verifier.verify(credential)?;

        // Build the verified `(tenant, region)` scope from the token (tenant-from-token, ID-3) — used
        // for BOTH the revocation consult (partitioned) and the S1 lookup. No path-derived scope.
        let scope = self.scope_for(&token);

        // (2) DURABLE revocation consult (the MR-008 `(tenant, region)`-partitioned S7 store; the
        //     carried-forward S7Denylist fix). A revoked jti never resolves to a live session
        //     (fail-closed). With `with_pg`, the deny survives a restart (the durable source of truth).
        //     The store's read is itself fail-closed (a PG read error denies), so a consult that cannot
        //     complete denies rather than admit a possibly-revoked token.
        let now = (self.now)();
        let target = RevokeTarget::Jti(token.jti.clone());
        if token.purpose.is_agent_run() {
            let CredentialPurpose::AgentRun {
                delegation_snapshot,
                ..
            } = &token.purpose
            else {
                unreachable!("is_agent_run matched a non-run purpose")
            };
            if !matches!(delegation_snapshot, Some(snapshot) if *snapshot > 0) {
                return Err(AuthzError::FailClosed(
                    "agent-run credential has no valid durable delegation-policy snapshot; \
                     caller-supplied legacy run mints are not authorization credentials"
                        .into(),
                ));
            }
            let state = self.revocations.run_token_state(&scope, &target, &now);
            if state != RunTokenState::LiveWithinRunLife {
                return Err(AuthzError::FailClosed(format!(
                    "agent-run token `{}` is not live in durable S7 ({state:?}) — expired, torn-down, \
                     and unknown run credentials are refused",
                    token.jti
                )));
            }
        } else if self.revocations.is_revoked(&scope, &target, &now) {
            return Err(AuthzError::FailClosed(format!(
                "token `{}` is revoked (durable S7 revocation store) — fail-closed (the deny survives \
                 restart; tenant `{}`)",
                token.jti, token.tenant.0
            )));
        }

        // (3) DPoP requirement for a long-lived PAT (§4): a PAT must be sender-constrained. The short-
        //     lived per-run tokens (CI/agent/per_job) are TTL-constrained, not DPoP-bound — their
        //     life IS the constraint.
        if token.kind == MachineKind::Pat && !token.dpop_bound {
            return Err(AuthzError::BadRequest(
                "a long-lived PAT must be DPoP sender-constrained (RFC 9449, §4) — a bearer-only PAT \
                 is refused"
                    .into(),
            ));
        }

        // (4) THE IDOR FLOOR (ID-3): the tenant is the VERIFIED TOKEN's, never the URL path. The scope
        //     (built above from the token's tenant) feeds the ONE storage-tier IDOR primitive over the
        //     path assertion — purely to COUNT a rejected mismatch.
        let resolved = scope.resolve(path_tenant);
        debug_assert_eq!(
            resolved.tenant, token.tenant,
            "the effective tenant must be the verified token's (ID-3, C6)"
        );
        if resolved.path_derived {
            // Unreachable by construction; counted so a future mutation that broke it trips the
            // drill's `path_derived_tenant_count == 0` rather than a silent IDOR.
            self.idor.count_path_derived();
        }
        if resolved.attempted_path_mismatch {
            self.idor.count_attempted_path_mismatch();
        }

        // (5) The authority ceiling (C6): a deploy key is repo-scoped; a self-hosted-runner / per-job
        //     token is bounded to ITS OWN tenant's SelfHosted jobs (it cannot name another tenant's
        //     scope — the no-global-pool property at the identity layer). A token whose authority
        //     exceeds its ceiling is refused (never silently widened).
        self.enforce_authority_ceiling(&token)?;

        // (6) Resolve the token's subject key to a principal in the VERIFIED tenant directory (the S1
        //     token-record index). No cross-tenant lookup: a token verified for tenant A resolves
        //     only into A's partition.
        let row = self
            .store
            .resolve_credential(&scope, credential.scheme.as_str(), &token.subject_key)
            .ok_or_else(|| {
                AuthzError::FailClosed(format!(
                    "no `{}` token record for the verified subject in tenant `{}` (unknown token — \
                     fail-closed, never a fabricated session)",
                    credential.scheme, token.tenant.0
                ))
            })?;

        // (7) Fail-closed on a deprovisioned principal (the machine principal was suspended/disabled).
        match row.status {
            PrincipalStatus::Active => {}
            PrincipalStatus::Suspended | PrincipalStatus::Disabled => {
                return Err(AuthzError::FailClosed(format!(
                    "machine principal `{}` is {:?} — authenticate fail-closes (it never resolves to \
                     an active session); full revocation is P-ID-14",
                    row.principal_id.0, row.status
                )));
            }
        }

        // The one polymorphic Principal (§3): a machine credential resolves to the SAME record. The
        // kind discriminant (Service for a machine identity) changes governance metadata, never the
        // authz code path. tenant/region are the VERIFIED token's.
        let principal = Principal::new(
            token.tenant.clone(),
            token.region.clone(),
            row.principal_id,
            row.kind,
            row.data_role,
            row.status,
        );
        Ok(RequestIdentity {
            principal,
            scope,
            credential: CredentialContext::Capability(VerifiedCapabilityContext {
                purpose: token.purpose,
                audience: token.audience,
                jti: token.jti,
                effective_authority: token.authority,
                expires_at_unix: token.exp_unix,
                dpop: if token.dpop_bound {
                    DpopState::Verified
                } else {
                    DpopState::Unbound
                },
            }),
        })
    }

    /// Compatibility projection for callers that only resolve a principal. Authorization surfaces
    /// must use [`Self::authenticate_identity`] so credential authority is not discarded.
    pub fn authenticate(
        &self,
        credential: &myelin_identity::Credential,
        path_tenant: Option<&TenantId>,
    ) -> myelin_identity::Result<Principal> {
        self.authenticate_identity(credential, path_tenant)
            .map(|identity| identity.principal)
    }

    /// The frozen-4.1 trait form of `authenticate` (no `path_tenant`) — delegates to the path-aware
    /// [`Self::authenticate`] with `path_tenant = None`. Used when the gateway already stripped the
    /// path; the IDOR floor is unchanged (tenant is still the token's).
    pub fn authenticate_trait(
        &self,
        credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<Principal> {
        self.authenticate(credential, None)
    }

    /// **Enforce the per-kind authority ceiling (architecture §3, C6).** A deploy key's authority may
    /// name only grants under one repo (`repo:…`); a self-hosted-runner / per-job token's authority
    /// may name only ITS OWN tenant's `SelfHosted` scope (`selfhosted:<tenant>`). A grant outside the
    /// ceiling is refused — the authority is NEVER silently widened. The PAT/CI/agent kinds carry
    /// their attenuated caveat chain unchanged (no extra structural ceiling — their narrowing is the
    /// monotone delegation algebra, §6).
    fn enforce_authority_ceiling(&self, token: &CapabilityToken) -> myelin_identity::Result<()> {
        match token.kind {
            MachineKind::DeployKey => {
                // Every grant must be a repo grant (the one-repo ceiling). A deploy key naming a
                // project-wide / non-repo grant is refused.
                for g in token.authority.grants() {
                    if !g.starts_with(REPO_GRANT_PREFIX) {
                        return Err(AuthzError::FailClosed(format!(
                            "deploy-key authority `{g}` exceeds the repo-scope ceiling (a deploy key \
                             is a repo-scoped Service principal, C6) — refused"
                        )));
                    }
                }
                Ok(())
            }
            MachineKind::PerJob => {
                // A self-hosted-runner token may name ONLY its own tenant's SelfHosted scope. A grant
                // for another tenant's SelfHosted scope (or any non-selfhosted grant) is refused —
                // the no-global-pool property: a runner token cannot act cross-tenant.
                let own = format!("{SELFHOSTED_GRANT_PREFIX}{}", token.tenant.0);
                for g in token.authority.grants() {
                    if !g.starts_with(SELFHOSTED_GRANT_PREFIX) {
                        return Err(AuthzError::FailClosed(format!(
                            "self-hosted-runner authority `{g}` is not a SelfHosted-scoped grant (C6) \
                             — refused"
                        )));
                    }
                    if g != own {
                        return Err(AuthzError::FailClosed(format!(
                            "self-hosted-runner authority `{g}` names a tenant other than its own \
                             (`{own}`) — a runner token cannot act cross-tenant (C6, no-global-pool) \
                             — refused"
                        )));
                    }
                }
                Ok(())
            }
            MachineKind::Pat | MachineKind::Ci | MachineKind::Agent => Ok(()),
        }
    }

    /// Mint the verified [`TenantScope`] from a [`CapabilityToken`] — the tenant + region are the
    /// token's (the trust root). The only `TenantScope` constructor is `from_verified_token`, so a
    /// scope derived from a path is structurally impossible here.
    fn scope_for(&self, token: &CapabilityToken) -> TenantScope {
        let principal = Principal::stub(
            PrincipalId(format!("tok:{}", token.subject_key)),
            PrincipalKind::Service,
            token.tenant.clone(),
        );
        TenantScope::from_verified_token(&principal, token.region.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{iam_events::signals, Credential, DataRole};
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

    /// The frozen verified-token envelope
    /// `<tenant>|<region>|<subject_key>|<jti>|<dpop:0|1>|<grants>`.
    fn material(
        tenant: &str,
        region: &str,
        subject_key: &str,
        jti: &str,
        dpop: bool,
        grants: &[&str],
    ) -> String {
        format!(
            "{tenant}|{region}|{subject_key}|{jti}|{}|{}|test_kind|edge||",
            if dpop { "1" } else { "0" },
            grants.join(",")
        )
    }

    /// Seed a machine principal in `tenant`/`region`, link a `scheme`/`subject_key` token record to
    /// it, and return the authenticator over the seeded store.
    #[allow(clippy::too_many_arguments)]
    fn seeded(
        scheme: &str,
        tenant: &str,
        region: &str,
        subject_key: &str,
        principal_id: &str,
        kind: PrincipalKind,
        status: PrincipalStatus,
    ) -> CapabilityAuthenticator {
        let st = store();
        let sc = scope(tenant, region);
        st.put_principal(
            &sc,
            PrincipalId(principal_id.into()),
            kind,
            DataRole::Controller,
            status,
            None,
        )
        .unwrap();
        st.link_credential(&sc, scheme, subject_key, &PrincipalId(principal_id.into()))
            .unwrap();
        CapabilityAuthenticator::new(st)
    }

    fn cred(scheme: &str, material: String) -> Credential {
        Credential {
            scheme: scheme.into(),
            material,
        }
    }

    /// **One happy-path per credential kind resolves to the correct polymorphic Principal (4.1, the
    /// token/machine half).** PAT / CI / agent / deploy-key / per-job each resolve to a
    /// `Principal{kind, tenant, region}` from S1 — the five token/machine surfaces.
    #[test]
    fn each_machine_scheme_resolves_to_its_principal() {
        // (scheme, dpop_required, ceiling-legal grant)
        let cases: &[(&str, bool, &str)] = &[
            (scheme::PAT, true, "repo:acme/web#write"),
            (scheme::CI, false, "ci:run"),
            (scheme::AGENT, false, "agent:run"),
            (scheme::DEPLOY_KEY, false, "repo:acme/web#push"),
            (scheme::PER_JOB, false, "selfhosted:acme"),
        ];
        for (s, dpop, grant) in cases {
            let auth = seeded(
                s,
                "acme",
                "eu-west",
                "subj-1",
                "svc:machine",
                PrincipalKind::Service,
                PrincipalStatus::Active,
            );
            let p = auth
                .authenticate(
                    &cred(
                        s,
                        material("acme", "eu-west", "subj-1", "jti-1", *dpop, &[grant]),
                    ),
                    None,
                )
                .unwrap_or_else(|e| panic!("scheme `{s}` should resolve: {e:?}"));
            assert_eq!(
                p.principal_id,
                PrincipalId("svc:machine".into()),
                "scheme {s}"
            );
            assert_eq!(
                p.tenant,
                TenantId("acme".into()),
                "scheme {s} tenant from token"
            );
            assert_eq!(p.region, Region("eu-west".into()), "scheme {s} region");
            assert_eq!(
                p.kind,
                PrincipalKind::Service,
                "scheme {s} machine → Service"
            );
        }
    }

    fn run_material(jti: &str) -> String {
        format!(
            "acme|eu-west|run-subject|{jti}|0|repo.pull|agent_run|edge|run-1|42"
        )
    }

    fn run_authenticator(revocations: RevocationStore, now: &'static str) -> CapabilityAuthenticator {
        let st = store();
        let sc = scope("acme", "eu-west");
        st.put_principal(
            &sc,
            PrincipalId("svc:agent".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .unwrap();
        st.link_credential(
            &sc,
            scheme::AGENT,
            "run-subject",
            &PrincipalId("svc:agent".into()),
        )
        .unwrap();
        CapabilityAuthenticator::with_verifier(
            st,
            Arc::new(StructuralTokenVerifier::new()),
            revocations,
        )
        .with_clock(move || Timestamp(now.into()))
    }

    /// Agent-run authentication is a positive lifecycle check: only a known S7 record inside its
    /// run-life window is accepted; unknown, expired, and explicitly torn-down credentials deny.
    #[test]
    fn agent_run_requires_live_durable_s7_state() {
        let sc = scope("acme", "eu-west");

        let live_s7 = RevocationStore::new();
        live_s7.register_run_token_ttl(
            &sc,
            "jti-live",
            Timestamp("2026-07-18T10:00:00Z".into()),
            Timestamp("2026-07-18T10:05:00Z".into()),
        );
        let identity = run_authenticator(live_s7, "2026-07-18T10:01:00Z")
            .authenticate_identity(&cred(scheme::AGENT, run_material("jti-live")), None)
            .expect("a known run token inside its run life is live");
        assert_eq!(identity.capability().effective_authority.len(), 1);

        let unknown = run_authenticator(RevocationStore::new(), "2026-07-18T10:01:00Z")
            .authenticate_identity(&cred(scheme::AGENT, run_material("jti-unknown")), None);
        assert!(matches!(unknown, Err(AuthzError::FailClosed(_))));

        let expired_s7 = RevocationStore::new();
        expired_s7.register_run_token_ttl(
            &sc,
            "jti-expired",
            Timestamp("2026-07-18T10:00:00Z".into()),
            Timestamp("2026-07-18T10:05:00Z".into()),
        );
        let expired = run_authenticator(expired_s7, "2026-07-18T10:05:00Z")
            .authenticate_identity(&cred(scheme::AGENT, run_material("jti-expired")), None);
        assert!(matches!(expired, Err(AuthzError::FailClosed(_))));

        let torn_down_s7 = RevocationStore::new();
        torn_down_s7.register_run_token_ttl(
            &sc,
            "jti-torn-down",
            Timestamp("2026-07-18T10:00:00Z".into()),
            Timestamp("2026-07-18T10:05:00Z".into()),
        );
        torn_down_s7.tear_down_run_token(
            &sc,
            "jti-torn-down",
            Timestamp("2026-07-18T10:01:00Z".into()),
        );
        let torn_down = run_authenticator(torn_down_s7, "2026-07-18T10:02:00Z")
            .authenticate_identity(&cred(scheme::AGENT, run_material("jti-torn-down")), None);
        assert!(matches!(torn_down, Err(AuthzError::FailClosed(_))));
    }

    #[test]
    fn agent_run_without_durable_snapshot_fails_closed() {
        let sc = scope("acme", "eu-west");
        let s7 = RevocationStore::new();
        s7.register_run_token_ttl(
            &sc,
            "jti-legacy",
            Timestamp("2026-07-18T10:00:00Z".into()),
            Timestamp("2026-07-18T10:05:00Z".into()),
        );
        let auth = run_authenticator(s7, "2026-07-18T10:01:00Z");
        let material = "acme|eu-west|run-subject|jti-legacy|0|repo.pull|agent_run|edge|run-1|";
        let result = auth.authenticate_identity(&cred(scheme::AGENT, material.into()), None);
        assert!(matches!(result, Err(AuthzError::FailClosed(_))));
    }

    /// **A deploy key resolves to a repo-scoped Service principal (architecture §3, C6).** Its
    /// authority ceiling is one repo; a deploy-key whose authority names a non-repo (project-wide)
    /// grant is refused — never silently widened.
    #[test]
    fn deploy_key_is_repo_scoped() {
        let auth = seeded(
            scheme::DEPLOY_KEY,
            "acme",
            "eu-west",
            "SHA256:dk",
            "svc:deploy",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        // A repo-scoped grant resolves to a Service principal.
        let p = auth
            .authenticate(
                &cred(
                    scheme::DEPLOY_KEY,
                    material(
                        "acme",
                        "eu-west",
                        "SHA256:dk",
                        "jti-dk",
                        false,
                        &["repo:acme/web#push"],
                    ),
                ),
                None,
            )
            .unwrap();
        assert_eq!(
            p.kind,
            PrincipalKind::Service,
            "a deploy key → repo-scoped Service principal"
        );

        // A deploy key naming a PROJECT-WIDE (non-repo) grant exceeds its ceiling → refused.
        let r = auth.authenticate(
            &cred(
                scheme::DEPLOY_KEY,
                material(
                    "acme",
                    "eu-west",
                    "SHA256:dk",
                    "jti-dk2",
                    false,
                    &["project:acme#admin"],
                ),
            ),
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(_))),
            "a deploy key may not exceed its one-repo ceiling (C6)"
        );
    }

    /// **A self-hosted-runner token cannot act cross-tenant (architecture §3, C6 — the mutation-
    /// tested mandatory-core).** A per-job token verified for `acme` resolves only into `acme`
    /// (cross-tenant resolution count = 0), and its authority may name ONLY `acme`'s SelfHosted scope;
    /// a grant for another tenant's SelfHosted scope is refused (the no-global-pool property).
    #[test]
    fn self_hosted_runner_cannot_act_cross_tenant() {
        let auth = seeded(
            scheme::PER_JOB,
            "acme",
            "eu-west",
            "run-1",
            "svc:runner",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        // The runner's own-tenant SelfHosted scope resolves.
        let p = auth
            .authenticate(
                &cred(
                    scheme::PER_JOB,
                    material(
                        "acme",
                        "eu-west",
                        "run-1",
                        "jti-r1",
                        false,
                        &["selfhosted:acme"],
                    ),
                ),
                None,
            )
            .unwrap();
        assert_eq!(
            p.tenant,
            TenantId("acme".into()),
            "the runner resolves into its own tenant"
        );
        assert_eq!(
            auth.idor_counters().path_derived_tenant_count(),
            0,
            "0 cross-tenant runner resolutions (the C6 mandatory-core)"
        );

        // A runner token whose authority names ANOTHER tenant's SelfHosted scope is refused.
        let r = auth.authenticate(
            &cred(
                scheme::PER_JOB,
                material(
                    "acme",
                    "eu-west",
                    "run-1",
                    "jti-r2",
                    false,
                    &["selfhosted:globex"],
                ),
            ),
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(_))),
            "a self-hosted-runner token cannot name another tenant's scope (C6, no-global-pool)"
        );
    }

    /// **A token verified for tenant A cannot resolve a principal in tenant B (no cross-tenant
    /// resolution).** Even though `svc:runner` exists in `acme`, a per-job token verified for `globex`
    /// resolves into `globex`'s (empty) directory and fail-closes — it never reaches `acme`'s rows.
    #[test]
    fn token_for_one_tenant_cannot_resolve_another_tenants_principal() {
        let st = store();
        let acme = scope("acme", "eu-west");
        st.put_principal(
            &acme,
            PrincipalId("svc:runner".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .unwrap();
        st.link_credential(
            &acme,
            scheme::PER_JOB,
            "run-1",
            &PrincipalId("svc:runner".into()),
        )
        .unwrap();
        let auth = CapabilityAuthenticator::new(st);

        // A token VERIFIED for globex presenting the same subject key resolves into globex's
        // directory — empty → fail-closed. (globex's own SelfHosted scope passes the ceiling.)
        let r = auth.authenticate(
            &cred(
                scheme::PER_JOB,
                material(
                    "globex",
                    "eu-west",
                    "run-1",
                    "jti-x",
                    false,
                    &["selfhosted:globex"],
                ),
            ),
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(_))),
            "a globex-verified token cannot resolve acme's principal (no cross-tenant resolve)"
        );
    }

    /// **THE IDOR FLOOR (ID-3): tenant comes from the token, never the URL path.** A token verified
    /// for `acme` presented at a path asserting `globex` resolves to `acme`; `path_derived_tenant_count`
    /// is 0 and the rejected mismatch is counted.
    #[test]
    fn tenant_is_from_token_not_the_url_path() {
        let auth = seeded(
            scheme::CI,
            "acme",
            "eu-west",
            "run-7",
            "svc:ci",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        let p = auth
            .authenticate(
                &cred(
                    scheme::CI,
                    material("acme", "eu-west", "run-7", "jti-ci", false, &["ci:run"]),
                ),
                Some(&TenantId("globex".into())),
            )
            .unwrap();
        assert_eq!(
            p.tenant,
            TenantId("acme".into()),
            "the resolved tenant is the TOKEN's (acme), never the path's (globex)"
        );
        assert_eq!(
            auth.idor_counters().path_derived_tenant_count(),
            0,
            "path_derived_tenant_count == 0 (the IDOR floor — tenant never from the path)"
        );
        assert_eq!(
            auth.idor_counters().attempted_path_mismatch_count(),
            1,
            "the rejected IDOR attempt (path ≠ token) was counted (the guard held)"
        );
    }

    /// **An attenuated PAT's caveat chain narrows authority — MONOTONE, never amplifying
    /// (mutation-tested mandatory-core, architecture §4).** Attenuating a parent authority by a caveat
    /// yields a STRICT SUBSET: a grant the parent never held is never minted, and the attenuated set
    /// is no wider than the parent. A `∩→∪` mutation (attenuation that GREW authority) is caught here.
    #[test]
    fn attenuated_pat_caveat_chain_narrows_authority() {
        let parent = Authority::of([
            "repo:acme/web#read",
            "repo:acme/web#write",
            "repo:acme/api#read",
        ]);
        // A caveat that KEEPS only the two read grants (and tries to sneak in a grant the parent
        // never had — which must NOT be minted).
        let caveat = Authority::of([
            "repo:acme/web#read",
            "repo:acme/api#read",
            "repo:acme/web#admin", // the parent never granted this
        ]);
        let attenuated = parent.attenuate(&caveat);

        // The attenuated authority is a STRICT SUBSET of the parent (it dropped #write, it did NOT
        // mint #admin).
        assert!(
            attenuated.is_subset_of(&parent),
            "attenuation is monotone: the child authority is no wider than the parent"
        );
        assert!(
            attenuated.len() < parent.len(),
            "the chain strictly narrowed (#write dropped)"
        );
        assert!(
            !attenuated.holds("repo:acme/web#admin"),
            "a grant the parent never held is NEVER minted by a caveat (monotone law)"
        );
        assert!(attenuated.holds("repo:acme/web#read"));
        assert!(attenuated.holds("repo:acme/api#read"));

        // A multi-step chain stays monotone: attenuating again only narrows further.
        let step2 = attenuated.attenuate(&Authority::of(["repo:acme/web#read"]));
        assert!(
            step2.is_subset_of(&attenuated),
            "a second caveat narrows again (never widens)"
        );
        assert_eq!(step2.len(), 1, "the chain converged to one grant");
    }

    /// **Attenuation NEVER amplifies, for any parent/caveat pair (the macaroon/biscuit law).** The
    /// post-condition the whole delegation algebra rests on: the attenuated set is always a subset of
    /// the parent. This pins the `∩` (a `∪` mutation would let some pair amplify).
    #[test]
    fn attenuation_is_never_amplifying() {
        let cases: &[(&[&str], &[&str])] = &[
            (&["a", "b"], &["a", "b", "c"]), // caveat asks for more than parent has
            (&["a", "b", "c"], &["b"]),      // caveat narrows
            (&[], &["a"]),                   // empty parent stays empty
            (&["a"], &[]),                   // empty caveat ⇒ empty result
            (&["x", "y"], &["x", "y"]),      // equal ⇒ equal (subset, not strict)
        ];
        for (parent_g, caveat_g) in cases {
            let parent = Authority::of(parent_g.iter().copied());
            let caveat = Authority::of(caveat_g.iter().copied());
            let child = parent.attenuate(&caveat);
            assert!(
                child.is_subset_of(&parent),
                "attenuate({parent_g:?}, {caveat_g:?}) = {:?} must be ⊆ the parent (never amplify)",
                child.grants().collect::<Vec<_>>()
            );
        }
    }

    /// **A long-lived PAT must be DPoP sender-constrained (architecture §4, RFC 9449).** A bearer-only
    /// PAT (no DPoP binding) is refused; a DPoP-bound PAT resolves. The short-lived per-run tokens are
    /// TTL-constrained, not DPoP-bound.
    #[test]
    fn long_lived_pat_must_be_dpop_bound() {
        let auth = seeded(
            scheme::PAT,
            "acme",
            "eu-west",
            "pat-1",
            "p:alice",
            PrincipalKind::Human,
            PrincipalStatus::Active,
        );
        // Bearer-only PAT (dpop = false) → refused.
        let r = auth.authenticate(
            &cred(
                scheme::PAT,
                material(
                    "acme",
                    "eu-west",
                    "pat-1",
                    "jti-p1",
                    false,
                    &["repo:acme/web#read"],
                ),
            ),
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::BadRequest(_))),
            "a bearer-only PAT (no DPoP) is refused (§4)"
        );
        // DPoP-bound PAT → resolves.
        let p = auth
            .authenticate(
                &cred(
                    scheme::PAT,
                    material(
                        "acme",
                        "eu-west",
                        "pat-1",
                        "jti-p2",
                        true,
                        &["repo:acme/web#read"],
                    ),
                ),
                None,
            )
            .unwrap();
        assert_eq!(
            p.principal_id,
            PrincipalId("p:alice".into()),
            "a DPoP-bound PAT resolves"
        );
    }

    /// **A revoked token fails closed through the DURABLE revocation store (the MR-011 carried-forward
    /// fix).** A token whose `jti` is revoked in the `(tenant, region)`-partitioned [`RevocationStore`]
    /// never resolves to a live session. (Cross-RESTART durability is proven in the live-PG integration
    /// test `integration_mr011_machine_token_revocation_durable` — here the in-memory model proves the
    /// consult routes through the durable store, not the old tenant-less stub.)
    #[test]
    fn revoked_token_fails_closed() {
        let auth = seeded(
            scheme::CI,
            "acme",
            "eu-west",
            "run-9",
            "svc:ci",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        // Before revocation: resolves.
        auth.authenticate(
            &cred(
                scheme::CI,
                material("acme", "eu-west", "run-9", "jti-live", false, &["ci:run"]),
            ),
            None,
        )
        .unwrap();
        // Revoke the jti through the DURABLE store, in the token's `(tenant, region)` partition.
        let sc = scope("acme", "eu-west");
        auth.revocations().revoke(
            &sc,
            &RevokeTarget::Jti("jti-live".into()),
            Timestamp("2026-06-26T00:00:00Z".into()),
        );
        // After revocation: fail-closed.
        let r = auth.authenticate(
            &cred(
                scheme::CI,
                material("acme", "eu-west", "run-9", "jti-live", false, &["ci:run"]),
            ),
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(_))),
            "a revoked token (durable S7 store) fails closed (never a live session)"
        );
        // The revoke is idempotent + tenant-partitioned: a DIFFERENT tenant's identical jti is NOT
        // revoked (no cross-tenant denylist path — the partition is the verified token's).
        assert!(!auth.revocations().is_revoked(
            &scope("globex", "eu-west"),
            &RevokeTarget::Jti("jti-live".into()),
            &Timestamp("2026-06-26T00:00:01Z".into()),
        ));
    }

    /// **A human/SSO scheme is REFUSED by this body (it is P-ID-06's).** The capability authenticator
    /// owns only the five token/machine surfaces; an `oidc`/`saml`/… credential is refused loudly
    /// (never silently mis-resolved through the wrong authenticator).
    #[test]
    fn human_sso_scheme_is_refused_here() {
        let auth = CapabilityAuthenticator::new(store());
        for s in human_scheme::HUMAN_SSO_SCHEMES {
            let r = auth.authenticate(
                &cred(s, material("acme", "eu-west", "x", "jti", false, &[])),
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::BadRequest(_))),
                "scheme `{s}` is P-ID-06's (human/SSO), refused by the machine-identity body"
            );
        }
    }

    /// **A malformed verified-token envelope is refused (never a partial/empty token) — and STILL
    /// emits its telemetry observation.**
    #[test]
    fn malformed_token_envelope_is_refused() {
        let auth = CapabilityAuthenticator::new(store());
        let bad = [
            "",                             // empty
            "acme|eu-west|s|jti|0",         // 5 fields (missing grants)
            "acme|eu-west|s|jti|0|g|extra", // 7 fields
            "|eu-west|s|jti|0|",            // empty tenant
            "acme|eu-west||jti|0|",         // empty subject_key
            "acme|eu-west|s||0|",           // empty jti
            "acme|eu-west|s|jti|2|",        // bad dpop flag
        ];
        for m in bad {
            let r = auth.authenticate(&cred(scheme::CI, m.into()), None);
            assert!(
                matches!(r, Err(AuthzError::BadRequest(_))),
                "malformed token envelope `{m}` is refused"
            );
        }
        assert_eq!(
            auth.telemetry().decision_count(),
            bad.len() as u64,
            "every refused decision still emitted its observation"
        );
    }

    /// **`auth_decision_latency` is emitted per request — on EVERY path (success AND failure).** The
    /// signal is keyed by the FROZEN row-1.8 name constant, never a literal.
    #[test]
    fn auth_decision_latency_emits_once_per_request() {
        let auth = seeded(
            scheme::AGENT,
            "acme",
            "eu-west",
            "run-x",
            "svc:agent",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        assert_eq!(auth.telemetry().decision_count(), 0);
        auth.authenticate(
            &cred(
                scheme::AGENT,
                material("acme", "eu-west", "run-x", "jti-a", false, &["agent:run"]),
            ),
            None,
        )
        .unwrap();
        assert_eq!(
            auth.telemetry().decision_count(),
            1,
            "success emits one observation"
        );
        let _ = auth.authenticate(
            &cred(
                scheme::AGENT,
                material("acme", "eu-west", "no-such", "jti-b", false, &["agent:run"]),
            ),
            None,
        );
        assert_eq!(
            auth.telemetry().decision_count(),
            2,
            "a failed decision also emits"
        );
        assert_eq!(AuthTelemetry::SIGNAL, signals::AUTH_DECISION_LATENCY);
    }

    /// **The `Authority` set predicates are exact (pins the monotone-law helpers).** `is_subset_of`
    /// distinguishes a strict subset / equal / non-subset; `is_empty`/`len`/`holds` agree with the
    /// grant set; `MachineKind::is_self_hosted_runner` is true ONLY for the per-job kind; `is_machine`
    /// recognises exactly the five machine schemes. These pin the helper predicates the attenuation
    /// monotone law and the C6 ceiling rest on (a flipped predicate would mis-decide a real grant).
    #[test]
    fn authority_and_kind_predicates_are_exact() {
        let parent = Authority::of(["a", "b"]);
        assert!(
            Authority::of(["a"]).is_subset_of(&parent),
            "a strict subset is ⊆"
        );
        assert!(
            parent.is_subset_of(&parent),
            "an equal set is ⊆ (not strict)"
        );
        assert!(
            !Authority::of(["a", "c"]).is_subset_of(&parent),
            "a non-subset is NOT ⊆"
        );
        assert!(
            Authority::default().is_empty(),
            "the empty authority is empty"
        );
        assert!(!parent.is_empty(), "a non-empty authority is not empty");
        assert_eq!(parent.len(), 2);
        assert!(parent.holds("a") && !parent.holds("z"));

        // is_self_hosted_runner is true ONLY for PerJob.
        assert!(MachineKind::PerJob.is_self_hosted_runner());
        for k in [
            MachineKind::Pat,
            MachineKind::Ci,
            MachineKind::Agent,
            MachineKind::DeployKey,
        ] {
            assert!(
                !k.is_self_hosted_runner(),
                "{k:?} is not a self-hosted runner"
            );
        }

        // is_machine recognises exactly the five machine schemes (and nothing human/SSO).
        for s in scheme::MACHINE_SCHEMES {
            assert!(scheme::is_machine(s), "`{s}` is a machine scheme");
        }
        for s in human_scheme::HUMAN_SSO_SCHEMES {
            assert!(
                !scheme::is_machine(s),
                "`{s}` is NOT a machine scheme (it is human/SSO)"
            );
        }
        assert!(!scheme::is_machine("nonsense"));
    }

    /// **A suspended/disabled machine principal fails closed.** A deprovisioned machine principal does
    /// not resolve to an active session.
    #[test]
    fn disabled_machine_principal_fails_closed() {
        for status in [PrincipalStatus::Disabled, PrincipalStatus::Suspended] {
            let auth = seeded(
                scheme::DEPLOY_KEY,
                "acme",
                "eu-west",
                "SHA256:dk",
                "svc:deploy",
                PrincipalKind::Service,
                status,
            );
            let r = auth.authenticate(
                &cred(
                    scheme::DEPLOY_KEY,
                    material(
                        "acme",
                        "eu-west",
                        "SHA256:dk",
                        "jti",
                        false,
                        &["repo:acme/web#push"],
                    ),
                ),
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::FailClosed(_))),
                "a {status:?} machine principal fails closed"
            );
        }
    }

    /// **THE MR-012 PROD-DEFAULT PROOF: the REAL PASETO verifier REFUSES a forged token (it is real
    /// Ed25519 crypto, never the mock `StructuralTokenVerifier`).** The production capability
    /// authenticator is built via [`CapabilityAuthenticator::with_verifier`] over the REAL
    /// [`crate::capability_crypto::PasetoCapabilityVerifier`] (the cell trust anchor). A hand-rolled
    /// plaintext `<tenant>|<region>|…` envelope — which the mock `StructuralTokenVerifier` would
    /// parse and resolve — is NOT a valid PASETO v4.public token, so the real verifier REFUSES it.
    /// This proves the prod default does real crypto: a forged token does not resolve a Principal.
    #[test]
    fn production_paseto_verifier_refuses_forged_token_never_mocks() {
        use crate::capability_crypto::{CellTokenAuthority, PasetoCapabilityVerifier};
        let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority");
        let auth = CapabilityAuthenticator::with_verifier(
            store(),
            Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
            RevocationStore::new(),
        );
        // The forgeable plaintext Structural envelope the MOCK verifier would accept — the real
        // PASETO verifier rejects it (it is not a signed v4.public token).
        let forged = material(
            "acme",
            "eu-west",
            "run-1",
            "jti-forge",
            false,
            &["agent:run"],
        );
        let r = auth.authenticate(&cred(scheme::AGENT, forged), None);
        assert!(
            matches!(r, Err(AuthzError::BadRequest(_)) | Err(AuthzError::FailClosed(_))),
            "the production PASETO verifier must REFUSE a forged plaintext envelope (real crypto), \
             never resolve it through the mock StructuralTokenVerifier"
        );
    }
}
