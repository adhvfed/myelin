use myelin_identity::Principal;
use myelin_tenancy::TenantId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct InjectedIdentity {
    pub principal: Principal,
}

impl InjectedIdentity {
    pub fn new(principal: Principal) -> InjectedIdentity {
        InjectedIdentity { principal }
    }

    pub fn token_tenant(&self) -> &TenantId {
        &self.principal.tenant
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicReject {
    CrossTenantIdor {
        path_tenant: TenantId,
        token_tenant: TenantId,
    },
}

impl core::fmt::Display for PublicReject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PublicReject::CrossTenantIdor {
                path_tenant,
                token_tenant,
            } => write!(
                f,
                "cross-tenant IDOR: URL path named tenant {:?} but the verified token owns {:?} \
                 - rejected + audited (§4.1, ID-3; tenant from token, never the path)",
                path_tenant.0, token_tenant.0
            ),
        }
    }
}

impl std::error::Error for PublicReject {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdorAuditRecord {
    pub principal_id: String,
    pub token_tenant: TenantId,
    pub path_tenant: TenantId,
}

#[derive(Clone, Default)]
pub struct AuditSink {
    records: Arc<Mutex<Vec<IdorAuditRecord>>>,
}

impl AuditSink {
    pub fn new() -> AuditSink {
        AuditSink::default()
    }

    fn record(&self, rec: IdorAuditRecord) {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(rec);
    }

    pub fn records(&self) -> Vec<IdorAuditRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn count(&self) -> usize {
        self.records.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[derive(Clone)]
pub struct PublicSurface {
    audit: AuditSink,
    misroutes: Arc<AtomicU64>,
}

impl Default for PublicSurface {
    fn default() -> Self {
        PublicSurface::new(AuditSink::new())
    }
}

impl PublicSurface {
    pub fn new(audit: AuditSink) -> PublicSurface {
        PublicSurface {
            audit,
            misroutes: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn audit(&self) -> &AuditSink {
        &self.audit
    }

    pub fn misroute_count(&self) -> u64 {
        self.misroutes.load(Ordering::SeqCst)
    }

    pub fn resolve_tenant(
        &self,
        identity: &InjectedIdentity,
        path_tenant: &TenantId,
    ) -> Result<TenantId, PublicReject> {
        let token_tenant = identity.token_tenant();
        if path_tenant != token_tenant {
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
        Ok(token_tenant.clone())
    }
}

pub trait Authorizer: Send + Sync {
    fn authorize(&self, principal: &Principal, action: &str) -> bool;
}

pub struct DenyAll;
impl Authorizer for DenyAll {
    fn authorize(&self, _principal: &Principal, _action: &str) -> bool {
        false
    }
}

pub struct AllowPrincipal(pub String);
impl Authorizer for AllowPrincipal {
    fn authorize(&self, principal: &Principal, _action: &str) -> bool {
        principal.principal_id.0 == self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InternalReject {
    Unauthorized {
        principal_id: String,
        action: String,
    },
}

impl core::fmt::Display for InternalReject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            InternalReject::Unauthorized { principal_id, action } => write!(
                f,
                "internal RPC re-authorization denied: principal {principal_id:?} action {action:?} \
                 - the trust boundary re-authorizes every call (§4.2; 'internal = safe' is not presumed)"
            ),
        }
    }
}

impl std::error::Error for InternalReject {}

pub struct InternalSurface<A: Authorizer> {
    authorizer: A,
}

impl<A: Authorizer> InternalSurface<A> {
    pub fn new(authorizer: A) -> InternalSurface<A> {
        InternalSurface { authorizer }
    }

    pub fn handle(&self, principal: &Principal, action: &str) -> Result<TenantId, InternalReject> {
        if !self.authorizer.authorize(principal, action) {
            return Err(InternalReject::Unauthorized {
                principal_id: principal.principal_id.0.clone(),
                action: action.to_string(),
            });
        }
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

    #[test]
    fn matching_path_tenant_resolves_to_token_tenant() {
        let surface = PublicSurface::default();
        let id = InjectedIdentity::new(principal_of("p", "acme"));
        let resolved = surface
            .resolve_tenant(&id, &TenantId("acme".into()))
            .expect("a matching path tenant is served");
        assert_eq!(
            resolved,
            TenantId("acme".into()),
            "the operating tenant is the token's"
        );
        assert_eq!(
            surface.misroute_count(),
            0,
            "no cross-tenant read was served"
        );
        assert_eq!(surface.audit().count(), 0, "nothing to audit on a match");
    }

    #[test]
    fn path_tenant_mismatch_is_rejected_and_audited_as_idor() {
        let surface = PublicSurface::default();
        let id = InjectedIdentity::new(principal_of("attacker", "acme"));
        let result = surface.resolve_tenant(&id, &TenantId("globex".into()));

        assert_eq!(
            result,
            Err(PublicReject::CrossTenantIdor {
                path_tenant: TenantId("globex".into()),
                token_tenant: TenantId("acme".into()),
            }),
            "the path≠token mismatch is rejected as a cross-tenant IDOR"
        );
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
        assert_eq!(
            surface.misroute_count(),
            0,
            "misroute_count stays 0 (nothing served cross-tenant)"
        );
    }

    #[test]
    fn the_source_of_truth_is_the_token_not_the_path() {
        let surface = PublicSurface::default();
        let id = InjectedIdentity::new(principal_of("p", "acme"));
        let resolved = surface
            .resolve_tenant(&id, &TenantId("acme".into()))
            .unwrap();
        assert_eq!(&resolved, id.token_tenant());
    }

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
        assert!(
            d.contains("internal = safe"),
            "names the §4.2 doctrine: {d}"
        );
    }

    #[test]
    fn internal_surface_re_authorizes_per_principal() {
        let surface = InternalSurface::new(AllowPrincipal("trusted-svc".into()));
        let ok = surface.handle(&principal_of("trusted-svc", "acme"), "issues.read");
        assert_eq!(
            ok,
            Ok(TenantId("acme".into())),
            "the admitted principal is authorized"
        );
        let denied = surface.handle(&principal_of("other-svc", "acme"), "issues.read");
        assert!(
            matches!(denied, Err(InternalReject::Unauthorized { .. })),
            "a different principal is denied"
        );
    }
}
