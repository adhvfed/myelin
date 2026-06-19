//! # `myelin-gdpr` — the `PersonalDataHolder` trait + the content-addressed `BlobStore`
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.5 (`myelin-gdpr` — the substrate-relevant seam) and §2.7 (the content-addressed
//! blob trait).
//!
//! **Contract-index cluster:** 10 — GDPR / Audit / `PersonalDataHolder` (row 10.1
//! `PersonalDataHolder`) and 11 — Storage (row 11.2 `BlobStore`)
//! (`planning/05-refined-shared-systems-architecture/contract-index.md`).
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! - `PersonalDataHolder` (10.1) — `locate/export/rectify/restrict/erase`, implemented by
//!   **every store**. The harness **auto-registers** every store a service opens as a
//!   holder (§3.4, GD-3) so "we forgot a store" is structurally impossible — combined
//!   with the `no-untagged-personal-data` lint (P-S10). Erasure = purge/crypto-shred/
//!   pseudonymise, **never hide**.
//! - `BlobStore` (11.2) — content-addressed (BLAKE3, per-tenant dedup); `put/get/head/
//!   delete`; the fs↔object choice is a one-line swap (EI-02 §8). Immutable-tier erasure
//!   is **crypto-shred** (destroy the per-tenant/per-subject key), not blob delete
//!   (ADR-12.3).
//!
//! ## Floors named (stubbed bodies → filling prompt)
//! All bodies are `todo!()`. The trait SHAPES are frozen here (P-001 skeleton):
//! - `PersonalDataHolder` impls + the DSR client + the KMS/crypto-shred abstraction →
//!   the GDPR roadmap (10.1–10.9). The harness auto-registration wiring (1.4) lands in
//!   `serve` (P-S12/P-S15).
//! - `BlobStore` backend + the KMS hierarchy behind crypto-shred (11.3) + the trust-tier/
//!   branch-scoped cache namespaces (11.2) → Storage M1. **Which blob backend (fs vs
//!   object)** is the named scale/value floor — a one-line swap, decided when volume is
//!   measured.

use myelin_identity::Principal;
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};

/// The subject whose personal data a DSR operation targets (architecture §2.5; contract
/// 10.1). Threads the verified principal + tenant. The subject-locator detail is per-tag
/// (`#[personal_data(... subject_locator)]`, 10.2), filled by the GDPR roadmap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub principal: Principal,
    pub tenant: TenantId,
}

/// A located set of a subject's personal data within one holder (architecture §2.5;
/// contract 10.1). Opaque in the skeleton; the located-data model lands with the GDPR
/// roadmap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocatedData(pub String);

/// An exported bundle of a subject's data (architecture §2.5; contract 10.1/10.4 —
/// `MerkleProvenBundle` is the GDPR roadmap's shape). Opaque in the skeleton.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportBundle(pub String);

/// Placeholder error for the skeleton (DSR-op failures). Real taxonomy lands with the
/// GDPR roadmap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsrError(pub String);

/// `Result` alias for the holder + blob surface.
pub type Result<T> = core::result::Result<T, DsrError>;

/// Every store is a holder (architecture §2.5; contract 10.1; ADR-12). Erasure is
/// purge/crypto-shred/pseudonymise, never hide; `restrict` suppresses indexing/agent-use/
/// analytics/notif for a subject. The harness auto-registers every opened store (GD-3).
///
/// **Floor:** every body is `todo!()` (frozen shape only); the GDPR roadmap implements
/// the DSR state machine + crypto-shred (10.1–10.9).
pub trait PersonalDataHolder {
    fn locate(&self, subject: &Subject) -> Result<LocatedData>;
    fn export(&self, subject: &Subject) -> Result<ExportBundle>;
    fn rectify(&self, subject: &Subject, patch: LocatedData) -> Result<()>;
    fn restrict(&self, subject: &Subject) -> Result<()>;
    /// purge / crypto-shred / pseudonymise — never hide (ADR-12.3).
    fn erase(&self, subject: &Subject) -> Result<()>;
}

/// A content address — the BLAKE3 hash of the blob bytes (architecture §2.7; contract
/// 11.2). String-backed in the skeleton; the typed BLAKE3 digest lands with Storage M1.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentHash(pub String);

/// Blob metadata returned by `head` (architecture §2.7; contract 11.2). Opaque in the
/// skeleton.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobMeta(pub String);

/// The content-addressed blob trait (architecture §2.7; contract 11.2; STOR-1).
/// Content-addressing gives dedup + integrity for free; the narrow trait keeps the
/// fs-vs-S3 choice a one-line swap. `delete` is crypto-shred for immutable/backup tiers.
///
/// **Floor:** every body is `todo!()`; the backend (fs vs object — the named scale floor)
/// + the KMS hierarchy behind crypto-shred (11.3) land in Storage M1.
pub trait BlobStore {
    /// content address = the hash (BLAKE3; per-tenant dedup).
    fn put(&self, bytes: &[u8]) -> Result<ContentHash>;
    fn get(&self, h: &ContentHash) -> Result<Vec<u8>>;
    fn head(&self, h: &ContentHash) -> Result<BlobMeta>;
    /// crypto-shred is the real erasure (ADR-12.3).
    fn delete(&self, h: &ContentHash) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn subject() -> Subject {
        Subject {
            principal: Principal {
                id: PrincipalId("p".into()),
                kind: PrincipalKind::Human,
                tenant: TenantId("acme".into()),
            },
            tenant: TenantId("acme".into()),
        }
    }

    /// Compile-asserting test: the `PersonalDataHolder` five-method shape is frozen
    /// (contract 10.1) — `locate/export/rectify/restrict/erase`. A stub proves the
    /// signatures; bodies deferred to the GDPR roadmap.
    #[test]
    fn personal_data_holder_shape_is_frozen() {
        struct Store;
        impl PersonalDataHolder for Store {
            fn locate(&self, _s: &Subject) -> Result<LocatedData> {
                todo!("locate lands in the GDPR roadmap (10.1)")
            }
            fn export(&self, _s: &Subject) -> Result<ExportBundle> {
                todo!("export lands in the GDPR roadmap (10.1/10.4)")
            }
            fn rectify(&self, _s: &Subject, _p: LocatedData) -> Result<()> {
                todo!("rectify lands in the GDPR roadmap (10.1)")
            }
            fn restrict(&self, _s: &Subject) -> Result<()> {
                todo!("restrict lands in the GDPR roadmap (10.1)")
            }
            fn erase(&self, _s: &Subject) -> Result<()> {
                todo!("erase = crypto-shred lands in the GDPR roadmap (10.1; ADR-12.3)")
            }
        }
        let _store = Store;
        let _subj = subject();
    }

    /// Compile-asserting test: the `BlobStore` four-method shape is frozen (contract
    /// 11.2) — `put/get/head/delete`, content-addressed. A stub proves the signatures;
    /// the backend is the named one-line-swap floor (Storage M1).
    #[test]
    fn blob_store_shape_is_frozen() {
        struct Blobs;
        impl BlobStore for Blobs {
            fn put(&self, _bytes: &[u8]) -> Result<ContentHash> {
                todo!("BLAKE3 hash-on-write lands in Storage M1 (11.2)")
            }
            fn get(&self, _h: &ContentHash) -> Result<Vec<u8>> {
                todo!("get lands in Storage M1 (11.2)")
            }
            fn head(&self, _h: &ContentHash) -> Result<BlobMeta> {
                todo!("head lands in Storage M1 (11.2)")
            }
            fn delete(&self, _h: &ContentHash) -> Result<()> {
                todo!("delete = crypto-shred lands in Storage M1 (11.2; ADR-12.3)")
            }
        }
        let _blobs = Blobs;
        let _h = ContentHash("blake3:abc".into());
    }
}
