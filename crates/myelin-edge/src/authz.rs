//! # The authorization seam at the edge (re-authorize every call)
//!
//! The gateway re-authorizes EVERY dispatched call through the substrate
//! [`Authorizer`](myelin_substrate::Authorizer) seam (the same trait the internal RPC surface uses) —
//! "internal = safe" is never presumed, and a denial is fail-closed (a 403). The REAL body (the
//! depth-bounded Zanzibar `check`/`list_objects`) is Identity M1; here the edge consumes the SAME
//! trait, and the substrate fixtures ([`DenyAll`](myelin_substrate::DenyAll) /
//! [`AllowPrincipal`](myelin_substrate::AllowPrincipal)) prove the gateway calls it.
//!
//! [`AllowAll`] is the M0 seam fixture for the trivial whoami proof handler (the same posture as the
//! substrate's `AllowPrincipal`/`DenyAll` fixtures shipped in production `topology.rs`): it admits any
//! principal so the happy-path proof can dispatch. A real subsystem injects the Identity-M1
//! authorizer; the gate proves a `DenyAll` → 403, so the seam is load-bearing, not vacuous.

use myelin_identity::Principal;
use myelin_substrate::Authorizer;

/// A seam fixture that admits every principal/action (the M0 happy-path authorizer for the whoami
/// proof). Production injects the real Identity-M1 [`Authorizer`]; this is the analogue of the
/// substrate's `AllowPrincipal`/`DenyAll` fixtures (shipped in `topology.rs`) — the seam is real, the
/// body is the named Identity-M1 floor. A `DenyAll` (substrate) proves the deny path is a 403.
pub struct AllowAll;

impl Authorizer for AllowAll {
    fn authorize(&self, _principal: &Principal, _action: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_substrate::DenyAll;
    use myelin_tenancy::TenantId;

    #[test]
    fn allow_all_admits_and_deny_all_refuses() {
        let p = Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId("acme".into()));
        assert!(AllowAll.authorize(&p, "edge.whoami"));
        assert!(!DenyAll.authorize(&p, "edge.whoami"));
    }
}
