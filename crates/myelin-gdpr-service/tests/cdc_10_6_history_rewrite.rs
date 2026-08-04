use myelin_gdpr_service::history_rewrite::{
    CacheEntryRef, CacheNamespaceInvalidator, FirstClassRewriteOp, HistoryRewriteRequest,
    InMemoryCacheNamespaces, RewriteRateLimiter, RewriteWiring, HISTORY_REWRITE_ACTION,
};
use myelin_gdpr_service::{
    AuditConsumer, CryptoShredKms, InMemoryShredKms, ShredKeyClass, ShredKeyHandle,
};
use myelin_tenancy::{ArtifactRef, TenantId};

fn request(tenant: &TenantId, repo: &ArtifactRef) -> HistoryRewriteRequest {
    HistoryRewriteRequest {
        tenant: tenant.clone(),
        repo: repo.clone(),
        actor_pseudonym: "admin-1@acme.noreply".into(),
        rewrite_spec: "filter-repo:remove-blob:leaked-secret".into(),
    }
}

#[test]
fn cdc_10_6_history_rewrite_op_drives_the_cache_namespace_invalidation_consumer() {
    let tenant = TenantId("acme".into());
    let repo = ArtifactRef("myelin://acme/git/secret-repo".into());

    let caches = InMemoryCacheNamespaces::new();
    caches.seed(
        &tenant,
        &repo,
        &CacheEntryRef::new("trusted", "clone-bundle"),
    );
    caches.seed(
        &tenant,
        &repo,
        &CacheEntryRef::new("trusted", "pack-bitmap"),
    );
    caches.seed(
        &tenant,
        &repo,
        &CacheEntryRef::new("fork:101", "fork-clone-bundle"),
    );
    let other = ArtifactRef("myelin://acme/git/other-repo".into());
    caches.seed(
        &tenant,
        &other,
        &CacheEntryRef::new("trusted", "clone-bundle"),
    );

    let limiter = RewriteRateLimiter::new(4);
    let audit = AuditConsumer::new();
    let kms = InMemoryShredKms::new();
    kms.provision(
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Tenant,
        },
        3,
    );
    let wiring = RewriteWiring {
        rate_limiter: &limiter,
        audit: &audit,
        kms: &kms,
        caches: &caches,
    };

    let cert = FirstClassRewriteOp::run(&request(&tenant, &repo), &wiring, 0)
        .expect("the op is admitted under budget");

    assert_eq!(cert.stale_pii_hits, 0, "0 stale-PII cache/clone hits");
    assert!(cert.is_complete(), "the GA-10 certificate is complete");
    assert_eq!(
        cert.fan_out.purged.len(),
        3,
        "all three stale blobs for this repo purged"
    );
    for entry in &cert.fan_out.purged {
        assert!(
            !caches.still_present(&tenant, &repo, entry),
            "{entry:?} is gone after the fan-out"
        );
    }
    assert!(
        caches.still_present(
            &tenant,
            &other,
            &CacheEntryRef::new("trusted", "clone-bundle")
        ),
        "a different repo's cache is NOT invalidated by this repo's rewrite"
    );

    let entries = audit.log().entries_for(&tenant);
    assert_eq!(
        entries.len(),
        1,
        "the op is audited as one git.history_rewrite entry"
    );
    assert_eq!(entries[0].action, HISTORY_REWRITE_ACTION);
    assert_eq!(
        entries[0].actor.actor, "admin-1@acme.noreply",
        "tenant-admin pseudonym actor"
    );
    assert!(
        audit.log().verify_chain(&tenant),
        "the audit chain verifies intact"
    );

    assert_eq!(
        cert.pack_shred_epoch,
        Some(3),
        "the pack-tier DEK epoch recorded"
    );
    assert!(
        !kms.is_present(&ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Tenant,
        }),
        "the pack-tier DEK is destroyed (reflogs/bitmaps/pack backups unrecoverable)"
    );

    assert!(cert.residual_named.contains("off-platform clones"));
}
