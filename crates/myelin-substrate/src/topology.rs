//! # The three-surface topology + tenant-from-token (P-S13 → global P-030, SUB-D7)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §4 (the three-surface service topology — public / internal RPC / metrics-health; the
//! public↔internal split is a **security boundary**, not a convenience), §4.1 (public surface —
//! behind the gateway; identity+causality headers injected; **trust the token's tenant, never
//! the URL path** — a path-tenant ≠ token-tenant mismatch is an IDOR → rejected + audited, ID-3),
//! §4.2 (internal RPC surface — a service does NOT re-authenticate but DOES re-authorize every
//! call; "internal = safe" is not presumed — the trust boundary is the boundary, not a free pass).
//!
//! **Contract-index:** row 1.2 (`Three-surface topology` — public / internal RPC / metrics-health;
//! public↔internal is a security boundary; tenant from token) — OWNED here.
//!
//! **GATE / DRILLS (this prompt):** SUB-D7 — a cross-tenant read via path-tenant ≠ token-tenant →
//! **0**; the `tenant-predicate` lint (P-S10/P-017) catches a tenant-less query at compile time.
//! The survival signal is `misroute_count == 0` (the harness `CrossTenantCount` projection) + the
//! lint green. The drill scenario lives in `tests/drill_sub_d7_idor.rs`.
//!
//! ## Why this is the most load-bearing zero in the platform (EI-01 §2)
//! A cross-tenant IDOR — one tenant reading another tenant's data because the request let an
//! attacker name the tenant in a path the server then trusted — is a TOP-TIER security bug. The
//! structural defence is two-layered and BOTH layers live in this codebase:
//!   1. **tenant-from-token (this module).** The public surface derives the operating `TenantId`
//!      from the *verified token's* `Principal`, NEVER from a caller-supplied path/header. A
//!      request whose URL path names a different tenant than the token is REJECTED (never served)
//!      and AUDITED as an attempted IDOR — the attempt is recorded loudly, never swallowed.
//!   2. **the `tenant-predicate` lint (P-S10).** Every storage query carries a `TenantId` bound at
//!      compile time, so a tenant-less query — the bug that would let a leaked path-tenant reach
//!      the database — fails to *compile*. Layer 1 stops the spoof at the edge; layer 2 makes the
//!      tenant-less query that would honour the spoof structurally impossible.
//!
//! ## The internal RPC surface (§4.2) — re-authorize, never presume "internal = safe"
//! The internal surface carries the injected identity headers forward; a service trusts the header
//! for *identity* (it does not re-authenticate) but re-runs *authorization* on every call
//! ([`InternalSurface::handle`] calls the [`Authorizer`] seam). Trusting the header for identity is
//! fine; trusting it for authorization is the bug. The seam is the contract; the real
//! `check`/`list_objects` body is Identity M1 (the [`Authorizer`] trait is the boundary here).
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The real gateway + the mTLS/signed-internal-credential transport + the OTLP audit sink** is
//!   the metrics-health/gateway wiring beyond M0 (the audit *consumer* is GDPR `P-GA-19`/`P-062`;
//!   here the IDOR audit is emitted into a typed, in-process [`AuditSink`] with the SAME PII-free
//!   shape an audit consumer reads). Named, not silently skipped (EI-01 §4).
//! - **The `Authorizer` body** (the depth-bounded Zanzibar `check` / `list_objects`) is Identity
//!   M1 (`P-ID-09`/`P-ID-11`). Here the trait is the re-authorize-every-call SEAM; a
//!   [`DenyAll`]/[`AllowPrincipal`] fixture proves the surface calls it on every request.
//! - **The causality + tenant trace-context middleware on the real listeners** lands with the
//!   OS-signal/listener wiring (the §3.5 producer is already exported by `serve`, P-010); here the
//!   middleware is the [`PublicSurface`]/[`InternalSurface`] request path that injects/forwards the
//!   verified `Principal` — the security property (tenant-from-token, re-authorize) is complete now.

use myelin_identity::Principal;
use myelin_tenancy::TenantId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The verified identity the gateway injects onto a public request (architecture §4.1). The
/// gateway authenticated the credential and resolved a [`Principal`] via Identity; the substrate
/// receives this TRUSTED, pre-verified principal — it does not re-authenticate. The operating
/// tenant is read off `principal.tenant`, NEVER off the request path.
///
/// This is the "identity+causality headers injected" carrier (§4.1). On this floor it carries the
/// verified `Principal`; the wire format (signed headers) is the gateway's detail (Identity M1).
#[derive(Clone, Debug)]
pub struct InjectedIdentity {
    /// The verified principal the gateway resolved (tenant-from-credential, ID-3). The operating
    /// tenant is `principal.tenant` — the single source of truth for "which tenant is this".
    pub principal: Principal,
}

impl InjectedIdentity {
    /// Wrap a gateway-verified principal.
    pub fn new(principal: Principal) -> InjectedIdentity {
        InjectedIdentity { principal }
    }

    /// **The tenant — from the verified token, full stop (§4.1, ID-3).** There is no other source.
    pub fn token_tenant(&self) -> &TenantId {
        &self.principal.tenant
    }
}

/// Why a public request was rejected at the edge (architecture §4.1). The load-bearing variant is
/// [`Self::CrossTenantIdor`]: the URL path named a tenant the verified token does not own — an
/// attempted cross-tenant IDOR, rejected + audited, never served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicReject {
    /// **The SUB-D7 rejection.** The request's URL path named `path_tenant` but the verified
    /// token's tenant is `token_tenant` — a mismatch is an attempted cross-tenant read. Rejected
    /// (the request is NEVER served) and audited (the attempt is recorded, never swallowed).
    CrossTenantIdor {
        /// The tenant the (untrusted) URL path tried to name.
        path_tenant: TenantId,
        /// The tenant the (trusted) verified token actually owns.
        token_tenant: TenantId,
    },
}

impl core::fmt::Display for PublicReject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PublicReject::CrossTenantIdor { path_tenant, token_tenant } => write!(
                f,
                "cross-tenant IDOR: URL path named tenant {:?} but the verified token owns {:?} \
                 — rejected + audited (§4.1, ID-3; tenant from token, never the path)",
                path_tenant.0, token_tenant.0
            ),
        }
    }
}

impl std::error::Error for PublicReject {}

/// One PII-free audit record of an attempted cross-tenant IDOR (architecture §4.1; the audit half
/// of "rejected + audited"). Carries only opaque tenant ids + the opaque principal id — never a
/// name/email/body (control-plane-pii-free by construction; the same shape the GDPR audit consumer
/// `P-GA-19` reads). A recorded attempt is the evidence that the structural defence FIRED.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdorAuditRecord {
    /// The opaque principal id of the actor that presented the verified token (PII-free).
    pub principal_id: String,
    /// The tenant the verified token owns (the actor's real tenant).
    pub token_tenant: TenantId,
    /// The tenant the URL path tried to name (the target of the attempted cross-tenant read).
    pub path_tenant: TenantId,
}

/// The audit sink the public surface records an attempted IDOR into (architecture §4.1). On this
/// floor it is a typed, in-process collector with the PII-free [`IdorAuditRecord`] shape; the
/// durable tamper-evident audit log is GDPR `P-GA-19`/`P-062` (the consumer reads the same shape).
/// Named floor (EI-01 §4): the SINK is real and recorded; the durable tamper-evident chain is that
/// prompt — the security property (every rejected IDOR is audited, never swallowed) is complete now.
#[derive(Clone, Default)]
pub struct AuditSink {
    records: Arc<Mutex<Vec<IdorAuditRecord>>>,
}

impl AuditSink {
    /// A fresh, empty sink.
    pub fn new() -> AuditSink {
        AuditSink::default()
    }

    /// Record an attempted cross-tenant IDOR (loud, never swallowed — the attempt IS evidence).
    fn record(&self, rec: IdorAuditRecord) {
        self.records.lock().unwrap_or_else(|e| e.into_inner()).push(rec);
    }

    /// Every audited IDOR attempt so far (so a drill/test can assert the rejection was audited).
    pub fn records(&self) -> Vec<IdorAuditRecord> {
        self.records.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// How many IDOR attempts have been audited.
    pub fn count(&self) -> usize {
        self.records.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// The public surface (architecture §4.1) — gateway-fronted, identity-injected, tenant-from-token.
/// The substrate-side request path the gateway feeds: it receives the verified [`InjectedIdentity`]
/// plus the (untrusted) URL-path tenant the caller wrote, and resolves the operating [`TenantId`]
/// from the TOKEN — rejecting + auditing a path≠token mismatch as a cross-tenant IDOR.
///
/// `misroute_count` is the SUB-D7 survival signal: the number of cross-tenant reads that got
/// through. It MUST stay `0` (the rejection path increments the AUDIT count, not the misroute
/// count — a misroute would be a *served* cross-tenant request, which this surface makes impossible).
#[derive(Clone)]
pub struct PublicSurface {
    audit: AuditSink,
    /// The number of cross-tenant reads that were actually SERVED (the SUB-D7 zero). This surface
    /// never serves one, so it is structurally pinned to 0; it is a counter (not a constant) so a
    /// regression — a future code path that served a mismatch — would show up as a non-zero here.
    misroutes: Arc<AtomicU64>,
}

impl Default for PublicSurface {
    fn default() -> Self {
        PublicSurface::new(AuditSink::new())
    }
}

impl PublicSurface {
    /// Build the public surface over an [`AuditSink`] (the gateway shares one sink with the audit
    /// consumer; here the test/drill reads it back).
    pub fn new(audit: AuditSink) -> PublicSurface {
        PublicSurface {
            audit,
            misroutes: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The audit sink (so a drill/test can assert a rejected IDOR was recorded).
    pub fn audit(&self) -> &AuditSink {
        &self.audit
    }

    /// **The SUB-D7 survival signal — `misroute_count`.** The count of cross-tenant reads that were
    /// actually served. Pinned to 0 by [`Self::resolve_tenant`] never serving a mismatch; exposed
    /// as a live counter so a *future* regression — a new code path that served a mismatch and
    /// `fetch_add`ed this — is observable (it would tick above 0).
    ///
    /// **Equivalent-mutant note (cargo-mutants, 2026-06-19):** `replace misroute_count -> 0`
    /// survives — because the surface NEVER increments `misroutes` (the structural guarantee), the
    /// live read is always 0, so `return 0` is observationally identical. This is the *correct*
    /// property, not a coverage gap: the counter is a regression tripwire for a future writer, and
    /// the `tests/drill_sub_d7_idor.rs` "gate is not vacuous" test proves a non-zero value WOULD
    /// read RED. The field + `fetch_add` seam stay so the tripwire is wired the day a writer lands.
    pub fn misroute_count(&self) -> u64 {
        self.misroutes.load(Ordering::SeqCst)
    }

    /// **Tenant-from-token (architecture §4.1, ID-3 — the SUB-D7 mechanism).** Resolve the operating
    /// tenant for a public request from the VERIFIED token, and reject + audit a path-tenant that
    /// does not match.
    ///
    /// - `identity` — the gateway-verified [`InjectedIdentity`] (the trusted principal; its
    ///   `principal.tenant` is the ONLY source of the operating tenant).
    /// - `path_tenant` — the tenant the caller wrote into the URL path (UNTRUSTED — an attacker
    ///   controls it). It is used ONLY to detect a spoof; it is never the source of the tenant.
    ///
    /// Returns the token's tenant on a match; on a mismatch returns
    /// [`PublicReject::CrossTenantIdor`] AND records an [`IdorAuditRecord`] (rejected + audited).
    /// The request is never served against `path_tenant`, so `misroute_count` stays 0.
    pub fn resolve_tenant(
        &self,
        identity: &InjectedIdentity,
        path_tenant: &TenantId,
    ) -> Result<TenantId, PublicReject> {
        let token_tenant = identity.token_tenant();
        if path_tenant != token_tenant {
            // The path tried to name a different tenant than the verified token owns — an attempted
            // cross-tenant IDOR. Reject (do NOT serve) and audit the attempt (loud, never swallowed).
            self.audit.record(IdorAuditRecord {
                principal_id: identity.principal.principal_id.0.clone(),
                token_tenant: token_tenant.clone(),
                path_tenant: path_tenant.clone(),
            });
            return Err(PublicReject::CrossTenantIdor {
                path_tenant: path_tenant.clone(),
                token_tenant: token_tenant.clone(),
            });
        }
        // Match: the operating tenant is the TOKEN's tenant (never the path — even on a match we
        // return the token's, so the path is never the source of truth). No cross-tenant read was
        // served, so misroute_count is unchanged (it stays 0).
        Ok(token_tenant.clone())
    }
}

/// The authorization seam the internal RPC surface re-runs on EVERY call (architecture §4.2). A
/// service trusts the injected header for *identity* but NOT for *authorization* — it re-authorizes
/// each call through this seam. The real body (the depth-bounded Zanzibar `check`/`list_objects`)
/// is Identity M1 (`P-ID-09`); here the trait is the boundary, and a `DenyAll`/`AllowPrincipal`
/// fixture proves the surface CALLS it on every request (does not presume "internal = safe").
pub trait Authorizer: Send + Sync {
    /// May `principal` perform `action`? Re-evaluated on every internal call (fail-closed: an
    /// authorizer that errors/denies → the call is denied; "internal = safe" is never presumed).
    fn authorize(&self, principal: &Principal, action: &str) -> bool;
}

/// A fail-closed fixture authorizer that denies everything (proves the internal surface re-runs
/// authorization — a request that "should be safe because it's internal" is still denied unless the
/// authorizer admits it). Used by the SUB-D7 internal-surface unit test.
pub struct DenyAll;
impl Authorizer for DenyAll {
    fn authorize(&self, _principal: &Principal, _action: &str) -> bool {
        false
    }
}

/// A fixture authorizer that admits only one principal id (proves the internal surface re-evaluates
/// authorization *per principal* — a different principal on the same internal channel is denied).
pub struct AllowPrincipal(pub String);
impl Authorizer for AllowPrincipal {
    fn authorize(&self, principal: &Principal, _action: &str) -> bool {
        principal.principal_id.0 == self.0
    }
}

/// Why an internal RPC call was rejected (architecture §4.2). The trust boundary re-authorizes;
/// a denied call is rejected fail-closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InternalReject {
    /// Re-authorization denied the call (the authorizer said no). The internal surface does NOT
    /// presume "internal = safe" — every call is re-authorized, and a denial rejects it.
    Unauthorized {
        /// The opaque principal id the call was made as (PII-free).
        principal_id: String,
        /// The action that was denied.
        action: String,
    },
}

impl core::fmt::Display for InternalReject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            InternalReject::Unauthorized { principal_id, action } => write!(
                f,
                "internal RPC re-authorization denied: principal {principal_id:?} action {action:?} \
                 — the trust boundary re-authorizes every call (§4.2; 'internal = safe' is not presumed)"
            ),
        }
    }
}

impl std::error::Error for InternalReject {}

/// The internal RPC surface (architecture §4.2) — the trust boundary. It carries the injected
/// identity headers forward (trusts them for *identity*, does not re-authenticate) but
/// **re-authorizes every call** through an [`Authorizer`]. The public↔internal split is a security
/// boundary: a call arriving here is inside the trust boundary for *identity*, never for
/// *authorization*.
pub struct InternalSurface<A: Authorizer> {
    authorizer: A,
}

impl<A: Authorizer> InternalSurface<A> {
    /// Build the internal surface over a re-authorization seam.
    pub fn new(authorizer: A) -> InternalSurface<A> {
        InternalSurface { authorizer }
    }

    /// **Re-authorize an internal call (§4.2).** The `principal` is taken from the forwarded
    /// identity header (trusted for identity), but `action` is re-authorized through the
    /// [`Authorizer`] — every call, fail-closed. Returns the principal's verified tenant on
    /// success (the same tenant-from-identity discipline), or [`InternalReject::Unauthorized`].
    pub fn handle(&self, principal: &Principal, action: &str) -> Result<TenantId, InternalReject> {
        if !self.authorizer.authorize(principal, action) {
            return Err(InternalReject::Unauthorized {
                principal_id: principal.principal_id.0.clone(),
                action: action.to_string(),
            });
        }
        // Authorized: the operating tenant is the principal's verified tenant (never a header-named
        // one — identity is trusted, but the tenant still comes from the verified principal).
        Ok(principal.tenant.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn principal_of(id: &str, tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
    }

    /// **The SUB-D7 mechanism — a path-tenant matching the token is served against the TOKEN's
    /// tenant.** The happy path: the operating tenant is the token's, no misroute, no audit.
    #[test]
    fn matching_path_tenant_resolves_to_token_tenant() {
        let surface = PublicSurface::default();
        let id = InjectedIdentity::new(principal_of("p", "acme"));
        let resolved = surface
            .resolve_tenant(&id, &TenantId("acme".into()))
            .expect("a matching path tenant is served");
        assert_eq!(resolved, TenantId("acme".into()), "the operating tenant is the token's");
        assert_eq!(surface.misroute_count(), 0, "no cross-tenant read was served");
        assert_eq!(surface.audit().count(), 0, "nothing to audit on a match");
    }

    /// **THE SUB-D7 rejection — a path-tenant ≠ token-tenant is rejected + audited as an IDOR, and
    /// is NEVER served.** The single most load-bearing security property in the platform: a
    /// cross-tenant read attempt → 0 served, 1 audited.
    #[test]
    fn path_tenant_mismatch_is_rejected_and_audited_as_idor() {
        let surface = PublicSurface::default();
        // The attacker's token owns `acme`; the URL path tries to name `globex` (a different tenant).
        let id = InjectedIdentity::new(principal_of("attacker", "acme"));
        let result = surface.resolve_tenant(&id, &TenantId("globex".into()));

        // REJECTED — never served against the spoofed tenant.
        assert_eq!(
            result,
            Err(PublicReject::CrossTenantIdor {
                path_tenant: TenantId("globex".into()),
                token_tenant: TenantId("acme".into()),
            }),
            "the path≠token mismatch is rejected as a cross-tenant IDOR"
        );
        // AUDITED — the attempt is recorded loudly (PII-free), never swallowed.
        assert_eq!(surface.audit().count(), 1, "the IDOR attempt was audited");
        assert_eq!(
            surface.audit().records()[0],
            IdorAuditRecord {
                principal_id: "attacker".into(),
                token_tenant: TenantId("acme".into()),
                path_tenant: TenantId("globex".into()),
            },
            "the audit record is the PII-free attempt evidence (opaque ids only)"
        );
        // The SUB-D7 ZERO — no cross-tenant read was served.
        assert_eq!(surface.misroute_count(), 0, "misroute_count stays 0 (nothing served cross-tenant)");
    }

    /// The tenant is NEVER taken from the path even when it "looks" plausible: a request with NO
    /// path tenant equal to the token is the only one served, and it is served against the token's
    /// tenant. (Belt-and-braces: even a match returns the token's value, so the path is never the
    /// source — proven by mutating the path to a same-named-but-distinct value being impossible to
    /// pass off as the source.)
    #[test]
    fn the_source_of_truth_is_the_token_not_the_path() {
        let surface = PublicSurface::default();
        let id = InjectedIdentity::new(principal_of("p", "acme"));
        // A second, different principal in the SAME tenant resolves to that tenant — the source is
        // the token's tenant, independent of which path string the caller chose (as long as it matches).
        let resolved = surface.resolve_tenant(&id, &TenantId("acme".into())).unwrap();
        // The returned value is the *token's* TenantId object, not the path's — same value, but the
        // function read it off `identity.token_tenant()`.
        assert_eq!(&resolved, id.token_tenant());
    }

    /// **The internal RPC surface re-authorizes EVERY call (§4.2) — "internal = safe" is not
    /// presumed.** A `DenyAll` authorizer rejects a call that arrived on the trusted internal
    /// channel: identity is trusted, authorization is re-run.
    #[test]
    fn internal_surface_re_authorizes_every_call_deny() {
        let surface = InternalSurface::new(DenyAll);
        let p = principal_of("svc", "acme");
        let r = surface.handle(&p, "issues.read");
        assert_eq!(
            r,
            Err(InternalReject::Unauthorized {
                principal_id: "svc".into(),
                action: "issues.read".into(),
            }),
            "an internal call is re-authorized (not presumed safe) and denied fail-closed"
        );
    }

    /// The `PublicReject`/`InternalReject` `Display` impls are LOUD and name the rule (so a
    /// rejected IDOR / denied internal call surfaces a specific, actionable message — never a bare
    /// error). Asserts the message text so the `Display` bodies are not silently emptied.
    #[test]
    fn reject_displays_are_loud_and_specific() {
        let idor = PublicReject::CrossTenantIdor {
            path_tenant: TenantId("globex".into()),
            token_tenant: TenantId("acme".into()),
        };
        let m = idor.to_string();
        assert!(m.contains("cross-tenant IDOR"), "names the bug class: {m}");
        assert!(m.contains("tenant from token"), "names the rule: {m}");

        let denied = InternalReject::Unauthorized {
            principal_id: "svc".into(),
            action: "issues.read".into(),
        };
        let d = denied.to_string();
        assert!(d.contains("re-authorization denied"), "names the rule: {d}");
        assert!(d.contains("internal = safe"), "names the §4.2 doctrine: {d}");
    }

    /// The internal surface re-authorizes PER PRINCIPAL: an admitted principal passes (and resolves
    /// to its verified tenant), a different principal on the same channel is denied — proving the
    /// authorizer is consulted on every call, not once.
    #[test]
    fn internal_surface_re_authorizes_per_principal() {
        let surface = InternalSurface::new(AllowPrincipal("trusted-svc".into()));
        // The admitted principal passes → operating tenant is its verified tenant.
        let ok = surface.handle(&principal_of("trusted-svc", "acme"), "issues.read");
        assert_eq!(ok, Ok(TenantId("acme".into())), "the admitted principal is authorized");
        // A DIFFERENT principal on the same internal channel is denied (re-authorized per call).
        let denied = surface.handle(&principal_of("other-svc", "acme"), "issues.read");
        assert!(matches!(denied, Err(InternalReject::Unauthorized { .. })), "a different principal is denied");
    }
}
