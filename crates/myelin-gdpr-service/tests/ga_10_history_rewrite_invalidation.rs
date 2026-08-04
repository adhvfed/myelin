use myelin_gdpr::EraseScope;
use myelin_gdpr_service::audit::verify_entries_for_test;
use myelin_gdpr_service::dsr::DsrKind;
use myelin_gdpr_service::history_rewrite::{
    CacheEntryRef, FirstClassRewriteOp, HistoryRewriteRequest, InMemoryCacheNamespaces,
    RewriteRateLimiter, RewriteWiring,
};
use myelin_gdpr_service::{
    AuditConsumer, CryptoShredKms, HoldScope, HoldVerdict, InMemoryShredKms, LegalHoldRegistry,
    ShredKeyClass, ShredKeyHandle,
};
use myelin_tenancy::{ArtifactRef, TenantId};

fn req(tenant: &TenantId, repo: &ArtifactRef) -> HistoryRewriteRequest {
    HistoryRewriteRequest {
        tenant: tenant.clone(),
        repo: repo.clone(),
        actor_pseudonym: "admin@acme.noreply".into(),
        rewrite_spec: "filter-repo:remove-blob:pii".into(),
    }
}

#[test]
fn ga_10_history_rewrite_invalidation_zero_stale_pii_op_audited() {
    let tenant = TenantId("acme".into());
    let repo = ArtifactRef("myelin://acme/git/leaky".into());

    let caches = InMemoryCacheNamespaces::new();
    caches.seed(&tenant, &repo, &CacheEntryRef::new("trusted", "clone"));
    caches.seed(&tenant, &repo, &CacheEntryRef::new("trusted", "bundle"));
    caches.seed(
        &tenant,
        &repo,
        &CacheEntryRef::new("branch:main", "mirror-bundle"),
    );
    caches.seed(&tenant, &repo, &CacheEntryRef::new("fork:7", "fork-bundle"));

    let limiter = RewriteRateLimiter::new(2);
    let audit = AuditConsumer::new();
    let kms = InMemoryShredKms::new();
    kms.provision(
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Tenant,
        },
        9,
    );
    let wiring = RewriteWiring {
        rate_limiter: &limiter,
        audit: &audit,
        kms: &kms,
        caches: &caches,
    };

    let cert = FirstClassRewriteOp::run(&req(&tenant, &repo), &wiring, 0).expect("op admitted");

    assert_eq!(cert.stale_pii_hits, 0, "0 stale-PII cache/clone hits");
    assert!(cert.fan_out.all_purged());
    assert_eq!(cert.fan_out.purged.len(), 4, "all four stale blobs purged");
    assert!(cert.is_complete(), "GA-10 certificate complete");
    assert_eq!(audit.log().entries_for(&tenant).len(), 1, "op audited");
    assert!(
        audit.log().verify_chain(&tenant),
        "the audit chain is intact"
    );
    assert_eq!(cert.pack_shred_epoch, Some(9));
    assert!(!kms.is_present(&ShredKeyHandle {
        tenant: tenant.clone(),
        class: ShredKeyClass::Tenant,
    }));
    assert!(cert.residual_named.contains("off-platform clones"));
    assert!(
        cert.residual_named.contains("P-GA-36"),
        "the outbound gate follow-on is named"
    );
}

#[test]
fn ga_d3_audit_tamper_detected_100_percent_at_cell_scale() {
    let tenant = TenantId("acme".into());
    let repo = ArtifactRef("myelin://acme/git/r".into());
    let limiter = RewriteRateLimiter::new(u32::MAX);
    let audit = AuditConsumer::new();
    let kms = InMemoryShredKms::new();
    kms.provision(
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Tenant,
        },
        1,
    );
    let caches = InMemoryCacheNamespaces::new();
    let wiring = RewriteWiring {
        rate_limiter: &limiter,
        audit: &audit,
        kms: &kms,
        caches: &caches,
    };

    const CELL_SCALE: u64 = 1024;
    for w in 0..CELL_SCALE {
        FirstClassRewriteOp::run(&req(&tenant, &repo), &wiring, w).expect("op admitted");
    }
    let entries = audit.log().entries_for(&tenant);
    assert_eq!(entries.len() as u64, CELL_SCALE, "cell-scale chain built");
    assert!(
        verify_entries_for_test(&entries),
        "the pristine chain verifies intact"
    );

    let mut detected = 0usize;
    for i in 0..entries.len() {
        let mut tampered = entries.clone();
        tampered[i].subject = ArtifactRef(format!("myelin://acme/TAMPERED/{i}"));
        if !verify_entries_for_test(&tampered) {
            detected += 1;
        }
    }
    assert_eq!(
        detected,
        entries.len(),
        "audit tamper detected 100% at cell scale"
    );
}

#[test]
fn ga_d6_legal_hold_defers_rewrite_erasure_resumes_on_lift() {
    let tenant = TenantId("acme".into());
    let scope = EraseScope::Tenant(tenant.clone());
    let holds = LegalHoldRegistry::new();

    holds.set(HoldScope::Tenant(tenant.0.clone()), true);
    assert_eq!(
        holds.verdict(DsrKind::Erasure, &scope),
        HoldVerdict::Deferred,
        "0 held-scope deletions - the rewrite erasure is DEFERRED under the hold"
    );

    holds.set(HoldScope::Tenant(tenant.0.clone()), false);
    assert_eq!(
        holds.verdict(DsrKind::Erasure, &scope),
        HoldVerdict::Proceed,
        "the deferred erasure resumes on hold-lift"
    );
}
