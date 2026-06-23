//! # CDC pair — CI Control-Plane artifact/cache WRITE PATH ↔ Storage 11.2 + 11.4 (CI-P22 / P-365).
//!
//! The CONSUMER-side contract test for the two storage rows CI-P22 consumes:
//! - **11.2** — the content-addressed `BlobStore` + the **trust-scoped cache namespaces** (C4) + the
//!   **within-EU CDN clone class** (C3). CI's [`derive_cache_scope`] hands the storage layer the
//!   EXACT `(TrustTier, CacheScope, run_pr_id)` triple its `CiCacheNamespace::put` enforces against —
//!   this test PINS that the CI-derived scope and the storage write-scope rule AGREE (a fork derives
//!   only its `fork:<pr_id>` scope and its write to `trusted` is refused).
//! - **11.4** — the per-subject-vs-per-tenant DEK selection. CI's [`select_log_segment_dek`] composes
//!   the FROZEN `myelin_storage::kms::{KeyClass, PiiKeyRef}` grammar — this test PINS that an isolable
//!   subject routes to `subject:<id>` and the non-isolable case to `tenant`, byte-identically to the
//!   storage `kms://<tenant>/<epoch>/<class>` ref the `log_segment.pii_key_ref` column carries.
//!
//! "CDC" here is the PROVIDER↔CONSUMER pin: the storage crate OWNS the primitives (the
//! `CiCacheNamespace` enforcement + the `PiiKeyRef` grammar); CI is the CONSUMER that must drive them
//! with byte-identical inputs. A drift in either (the scope vocabulary, the key-class grammar) breaks
//! this test — exactly the no-drift property a CDC pair guards.

use myelin_ci_controlplane::artifact_cache::{
    derive_cache_scope, select_log_segment_dek, RunProvenance, SegmentPii,
};
use myelin_storage::blob::FsBlobStore;
use myelin_storage::ci_cache_scope::{CacheScope, CacheScopeError, CiCacheNamespace, TrustTier};
use myelin_storage::kms::KeyClass;
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

/// **11.2 (C4) — the CI-derived fork scope and the storage write-scope rule AGREE: a fork run writes
/// ONLY its own `fork:<pr_id>` scope; its write to `trusted` is refused by the blob client.** The CI
/// half ([`derive_cache_scope`]) + the storage half (`CiCacheNamespace::put`) compose with no drift.
#[test]
fn ci_derived_fork_scope_writes_own_scope_and_is_refused_at_trusted() {
    let base = FsBlobStore::new();
    let cache = CiCacheNamespace::over(tenant(), &base);

    // CI derives the fork run's scope off the CI-stamped trust_tier fact.
    let prov = RunProvenance {
        trust_tier: "untrusted_fork".into(),
        protected_branch: None,
        pr_id: Some("42".into()),
    };
    let (tier, own_scope, run_pr) = derive_cache_scope(&prov).expect("fork derives its scope");
    assert_eq!(tier, TrustTier::UntrustedFork);
    assert_eq!(own_scope, CacheScope::Fork { pr_id: "42".into() });

    // The CI-derived inputs drive the STORAGE write-scope rule: the own fork scope WRITES.
    cache
        .put(tier, &run_pr, &own_scope, "deps", b"fork-deps")
        .expect("fork writes its own scope (the two halves agree)");
    assert!(cache.contains(&own_scope, "deps"));

    // The same CI-derived fork tier CANNOT write the trusted scope (the storage refusal) — 0 landings.
    let refused = cache.put(tier, &run_pr, &CacheScope::Trusted, "deps", b"poison");
    assert!(matches!(
        refused,
        Err(CacheScopeError::ForkWriteToTrusted { .. })
    ));
    assert!(!cache.contains(&CacheScope::Trusted, "deps"));
}

/// **11.2 (C4) — a CI-derived TRUSTED scope writes the trusted scope (the provider accepts the
/// consumer's trusted-tier input).** The protected-branch case derives a `branch:<name>` scope.
#[test]
fn ci_derived_trusted_scope_writes_trusted_and_branch() {
    let base = FsBlobStore::new();
    let cache = CiCacheNamespace::over(tenant(), &base);

    let trusted = RunProvenance {
        trust_tier: "trusted".into(),
        protected_branch: None,
        pr_id: None,
    };
    let (tier, scope, pr) = derive_cache_scope(&trusted).expect("trusted derives");
    cache
        .put(tier, &pr, &scope, "deps", b"trusted-deps")
        .expect("trusted writes trusted");
    assert!(cache.contains(&CacheScope::Trusted, "deps"));

    let branch = RunProvenance {
        trust_tier: "trusted".into(),
        protected_branch: Some("main".into()),
        pr_id: None,
    };
    let (btier, bscope, bpr) = derive_cache_scope(&branch).expect("branch derives");
    assert_eq!(
        bscope,
        CacheScope::Branch {
            name: "main".into()
        }
    );
    cache
        .put(btier, &bpr, &bscope, "deps", b"branch-deps")
        .expect("trusted writes branch");
}

/// **11.4 — the CI per-subject DEK selection composes the storage `KeyClass`/`PiiKeyRef` grammar
/// byte-identically.** Isolable subject → `subject:<id>`; non-isolable → `tenant`.
#[test]
fn ci_dek_selection_matches_the_storage_pii_key_ref_grammar() {
    // Isolable subject PII → the per-subject DEK (the GD-4 individual crypto-shred lever).
    let subj = select_log_segment_dek(
        &tenant(),
        3,
        &SegmentPii::IsolableSubject {
            subject_id: "u-42".into(),
        },
    );
    assert_eq!(subj.class, KeyClass::Subject("u-42".into()));
    // Byte-identical to the storage kms://<tenant>/<epoch>/<class> grammar.
    assert_eq!(subj.to_uri(), "kms://acme/3/subject:u-42");

    // Non-isolable → the per-tenant DEK fallback.
    let bulk = select_log_segment_dek(&tenant(), 0, &SegmentPii::NotIsolable);
    assert_eq!(bulk.class, KeyClass::Tenant);
    assert_eq!(bulk.to_uri(), "kms://acme/0/tenant");
}
