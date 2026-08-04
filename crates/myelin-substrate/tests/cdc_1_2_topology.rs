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

    assert_eq!(
        handle.surfaces(),
        &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
        "the three surfaces opened (public / internal / metrics-health)"
    );

    let public = handle.public_surface();
    let acme = InjectedIdentity::new(stub("acme-user", "acme"));
    assert_eq!(
        public.resolve_tenant(&acme, &TenantId("acme".into())),
        Ok(TenantId("acme".into())),
        "an honest request is served against the token's tenant"
    );

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

#[test]
fn cdc_1_2_internal_surface_re_authorizes_every_call() {
    let internal = InternalSurface::new(AllowPrincipal("trusted-svc".into()));

    assert_eq!(
        internal.handle(&stub("trusted-svc", "acme"), "issues.read"),
        Ok(TenantId("acme".into())),
        "the admitted internal principal is authorized"
    );

    assert!(
        matches!(
            internal.handle(&stub("other-svc", "acme"), "issues.read"),
            Err(InternalReject::Unauthorized { .. })
        ),
        "a non-admitted internal call is denied - 'internal = safe' is not presumed"
    );
}
