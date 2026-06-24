//! # CDC 10.6 — history-rewrite as a first-class audited op + invalidation fan-out (P-GA-35 → P-451)
//!
//! **Contract:** index row 10.6 (the history-rewrite audited op + the invalidation fan-out to the
//! forks/mirrors/clone-cache tied to Storage's trust-tier/branch-scoped cache namespaces 11.2). This
//! is the M5 promotion of the P-GA-26 skeleton to the first-class op (gdpr §6.6).
//!
//! The contract-coverage scanner (P-S21) reads BOTH halves of the pair from this file:
//! - **provider** = the `git.history_rewrite` op ([`FirstClassRewriteOp::run`]) — it OWNS the policy
//!   (a rewrite invalidates the repo's stale clone/bundle blobs) and drives the invalidation across
//!   the cache-namespace seam;
//! - **consumer** = the cache-namespace invalidator ([`CacheNamespaceInvalidator`]) — Storage's
//!   trust-scoped cache namespaces (11.2-C4) + the within-EU CDN clone/bundle class (11.2-C3) behind
//!   the seam; it PURGES each stale clone/bundle blob the op names. `myelin-gdpr-service` never imports
//!   `myelin-storage` (the no-cross-store-read law) — the invalidation crosses this seam.
//!
//! The dated green artifact: a `git.history_rewrite` op runs over a seeded trust-scoped clone/bundle
//! cache; the op audits itself (outbox-only), crypto-shreds the pack-tier shreddables, and the
//! invalidation fan-out purges every stale blob — 0 stale-PII cache/clone hits, op audited.

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

/// **The 10.6 history-rewrite provider+consumer CDC pair.** The provider (the op) drives the
/// invalidation across the cache-namespace consumer (Storage's trust-scoped namespaces behind the
/// seam); every stale clone/bundle blob is purged (0 stale-PII cache/clone hits) and the op is
/// audited through the outbox-only consumer.
#[test]
fn cdc_10_6_history_rewrite_op_drives_the_cache_namespace_invalidation_consumer() {
    let tenant = TenantId("acme".into());
    let repo = ArtifactRef("myelin://acme/git/secret-repo".into());

    // CONSUMER (the cache-namespace seam): seed a trust-scoped clone/bundle cache — a trusted CI
    // build clone, a trusted pack bitmap, and an untrusted-fork run's bundle. After the rewrite these
    // are stale (they would serve rewritten-away PII from a cache).
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
    // A DIFFERENT repo's blob in the same tenant must NOT be touched by this repo's rewrite.
    let other = ArtifactRef("myelin://acme/git/other-repo".into());
    caches.seed(
        &tenant,
        &other,
        &CacheEntryRef::new("trusted", "clone-bundle"),
    );

    // PROVIDER (the op): the per-tenant rate limiter, the outbox-only audit consumer, the pack-tier
    // crypto-shred KMS, and the cache-namespace invalidator — all behind their seams.
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

    // The op runs: audit → crypto-shred pack shreddables → invalidation fan-out.
    let cert = FirstClassRewriteOp::run(&request(&tenant, &repo), &wiring, 0)
        .expect("the op is admitted under budget");

    // GA-10: 0 stale-PII cache/clone hits — the consumer purged every stale clone/bundle blob.
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
    // The OTHER repo's blob is UNTOUCHED — the fan-out is scoped to the rewritten repo.
    assert!(
        caches.still_present(
            &tenant,
            &other,
            &CacheEntryRef::new("trusted", "clone-bundle")
        ),
        "a different repo's cache is NOT invalidated by this repo's rewrite"
    );

    // The op is audited (outbox-only consumer): exactly one git.history_rewrite entry, chain intact.
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

    // The pack-tier shreddables were crypto-shred (NOT the commit-object bytes — the honest split).
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

    // The residual is named, not pretended-solved (§6.6).
    assert!(cert.residual_named.contains("off-platform clones"));
}
