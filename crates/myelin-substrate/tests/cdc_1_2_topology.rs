//! # CDC 1.2 — the three-surface topology + tenant-from-token (P-S13 → global P-030)
//!
//! **Contract:** index row 1.2 (`Three-surface topology` — public / internal RPC / metrics-health;
//! public↔internal is a security boundary; **tenant from token**). The contract-coverage scanner
//! (P-S21) reads BOTH halves:
//! - **provider** = `myelin_substrate::topology` (the [`PublicSurface`] tenant-from-token mechanism
//!   plus the [`InternalSurface`] re-authorize-every-call trust boundary), unit-tested in
//!   `src/topology.rs`.
//! - **consumer** = a service-`main`-shaped caller that boots the lifecycle via `serve`/`boot`,
//!   takes the lifecycle-opened public surface off the `ServeHandle`, and resolves the operating
//!   tenant for a public request through it — THIS file. It proves the three surfaces open in the
//!   lifecycle AND that the gateway-fed request path derives the tenant from the verified token,
//!   rejecting + auditing a path≠token spoof (SUB-D7).
//!
//! This is the consumer half of the dated green artifact P-S13 names (the SUB-D7 drill itself is
//! `tests/drill_sub_d7_idor.rs`).

use myelin_events::relay::InProcessBus;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::serve::{boot, AppSpec, OutboxSpec, Surface};
use myelin_substrate::{
    AllowPrincipal, Config, CriticalDependencies, HotTables, InjectedIdentity, InternalReject,
    InternalRpc, InternalSurface, Migrations, PublicReject, PublicRoutes, StoreManifest,
};
use myelin_tenancy::TenantId;

fn stub(id: &str, tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

/// **CDC 1.2 — the consumer side (public surface).** A service boots via `serve`'s lifecycle; the
/// three surfaces open; the lifecycle-opened public surface resolves the operating tenant from the
/// verified token (never the URL path), rejecting + auditing a path≠token spoof as a cross-tenant
/// IDOR (SUB-D7). `misroute_count` stays 0.
#[test]
fn cdc_1_2_lifecycle_public_surface_is_tenant_from_token() {
    let spec = AppSpec {
        name: "svc",
        config: Config::default(),
        migrations: Migrations::default(),
        hot_tables: HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: vec![],
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::new(myelin_events::OutboxStore::new(), InProcessBus::new()),
        critical: CriticalDependencies::default(),
    };
    let handle = boot(spec).expect("the service boots from serve(AppSpec)");

    // The three-surface topology opened in the lifecycle (§4; the security boundary).
    assert_eq!(
        handle.surfaces(),
        &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
        "the three surfaces opened (public / internal / metrics-health)"
    );

    // The gateway feeds a request through the lifecycle-opened public surface. An honest request
    // (path == token) is served against the TOKEN's tenant.
    let public = handle.public_surface();
    let acme = InjectedIdentity::new(stub("acme-user", "acme"));
    assert_eq!(
        public.resolve_tenant(&acme, &TenantId("acme".into())),
        Ok(TenantId("acme".into())),
        "an honest request is served against the token's tenant"
    );

    // A spoof (acme token, globex path) is rejected + audited as a cross-tenant IDOR — never served.
    assert_eq!(
        public.resolve_tenant(&acme, &TenantId("globex".into())),
        Err(PublicReject::CrossTenantIdor {
            path_tenant: TenantId("globex".into()),
            token_tenant: TenantId("acme".into()),
        }),
        "a path≠token spoof is rejected as an IDOR"
    );
    assert_eq!(
        public.audit().count(),
        1,
        "the spoof was audited (PII-free)"
    );
    assert_eq!(
        public.misroute_count(),
        0,
        "the SUB-D7 zero: no cross-tenant read served"
    );
}

/// **CDC 1.2 — the consumer side (internal surface).** A service builds the internal RPC trust
/// boundary over an authorizer and re-authorizes every call — identity trusted, authorization
/// re-run. "internal = safe" is not presumed.
#[test]
fn cdc_1_2_internal_surface_re_authorizes_every_call() {
    let internal = InternalSurface::new(AllowPrincipal("trusted-svc".into()));

    // an admitted principal is authorized → operating tenant is its verified tenant.
    assert_eq!(
        internal.handle(&stub("trusted-svc", "acme"), "issues.read"),
        Ok(TenantId("acme".into())),
        "the admitted internal principal is authorized"
    );

    // a different principal on the same internal channel is denied (re-authorized per call).
    assert!(
        matches!(
            internal.handle(&stub("other-svc", "acme"), "issues.read"),
            Err(InternalReject::Unauthorized { .. })
        ),
        "a non-admitted internal call is denied — 'internal = safe' is not presumed"
    );
}
