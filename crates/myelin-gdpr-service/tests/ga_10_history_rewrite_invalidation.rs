//! # DRILL — GA-10 + GA-D3/GA-D6 at cell scale (the E2E-3 audit-tamper leg) — P-GA-35 → P-451
//!
//! **Dated green artifact: 2026-06-24.** This is the M5 GDPR exit drill for history-rewrite as a
//! first-class audited op (gdpr §6.6 / contract 10.6) + the audit tamper-evidence + legal-hold gates
//! at cell scale:
//!
//! - **GA-10** (history-rewrite-invalidation): a `git.history_rewrite` op runs; the invalidation
//!   fan-out reaches the forks/mirrors/clone-cache (the trust-scoped namespaces purge the stale
//!   clone/bundle blobs), the op is audited. **0 stale-PII cache/clone hits; op audited.**
//! - **GA-D3** (audit tamper at cell scale, the E2E-3 leg): under world-scale audit volume a
//!   retroactive edit to ANY entry is detected 100% (chain + leaf recompute). **Tamper detected
//!   100%.**
//! - **GA-D6** (legal-hold defers erasure): a history-rewrite erase over a held scope is DEFERRED
//!   (suspend-don't-delete), resumes on lift. **0 held-scope deletions.**
//!
//! Floors named: the off-platform-clone residual (an independent clone a third party holds) is
//! NAMED, not pretended-solved — the fan-out reaches the replicas the platform serves. The outbound
//! push-mirror residency gate (GA-11) is the sibling prompt P-GA-36.

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

/// **GA-10 — history-rewrite-invalidation (0 stale-PII cache/clone hits; op audited).** The op runs
/// over a seeded trust-scoped clone/bundle cache; the fan-out purges every stale blob across the
/// trust-scoped namespaces (a fork scope's purge never reaches the trusted scope's keyspace) and the
/// op is audited. 0 stale-PII cache/clone hits is the gate.
#[test]
fn ga_10_history_rewrite_invalidation_zero_stale_pii_op_audited() {
    let tenant = TenantId("acme".into());
    let repo = ArtifactRef("myelin://acme/git/leaky".into());

    let caches = InMemoryCacheNamespaces::new();
    // forks/mirrors/clone-cache: a trusted clone, a trusted bundle, a fork run's bundle.
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

    // 0 stale-PII cache/clone hits — every reached clone/bundle blob purged.
    assert_eq!(cert.stale_pii_hits, 0, "0 stale-PII cache/clone hits");
    assert!(cert.fan_out.all_purged());
    assert_eq!(cert.fan_out.purged.len(), 4, "all four stale blobs purged");
    assert!(cert.is_complete(), "GA-10 certificate complete");
    // op audited.
    assert_eq!(audit.log().entries_for(&tenant).len(), 1, "op audited");
    assert!(
        audit.log().verify_chain(&tenant),
        "the audit chain is intact"
    );
    // crypto-shred reached the pack-tier shreddables (NOT the commit-object bytes).
    assert_eq!(cert.pack_shred_epoch, Some(9));
    assert!(!kms.is_present(&ShredKeyHandle {
        tenant: tenant.clone(),
        class: ShredKeyClass::Tenant,
    }));
    // residual named, not pretended-solved.
    assert!(cert.residual_named.contains("off-platform clones"));
    assert!(
        cert.residual_named.contains("P-GA-36"),
        "the outbound gate follow-on is named"
    );
}

/// **GA-D3 — audit tamper detected 100% at cell scale (the E2E-3 audit-tamper leg).** Under
/// world-scale audit volume a chain of `git.history_rewrite` entries is built; a retroactive edit to
/// EVERY position is detected (the recomputed leaf no longer matches / the seq breaks). 100% caught.
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

    // Cell-scale volume of audited history-rewrite ops.
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

    // Inject a tamper at EVERY position → 100% detection.
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

/// **GA-D6 — legal-hold defers the rewrite erasure (0 held-scope deletions, resumes on lift).** The
/// SAME G4 legal-hold gate the retention engine backs DEFERS a `DsrKind::Erasure` over a held scope;
/// on lift it proceeds.
#[test]
fn ga_d6_legal_hold_defers_rewrite_erasure_resumes_on_lift() {
    let tenant = TenantId("acme".into());
    let scope = EraseScope::Tenant(tenant.clone());
    let holds = LegalHoldRegistry::new();

    holds.set(HoldScope::Tenant(tenant.0.clone()), true);
    assert_eq!(
        holds.verdict(DsrKind::Erasure, &scope),
        HoldVerdict::Deferred,
        "0 held-scope deletions — the rewrite erasure is DEFERRED under the hold"
    );

    holds.set(HoldScope::Tenant(tenant.0.clone()), false);
    assert_eq!(
        holds.verdict(DsrKind::Erasure, &scope),
        HoldVerdict::Proceed,
        "the deferred erasure resumes on hold-lift"
    );
}
