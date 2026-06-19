//! # `myelin-gdpr` — the GDPR/Audit compile-time contract carrier (the `PersonalDataHolder`
//! spine + the core DSR types + the `data_role` classification anchor)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §1 (the two legal
//! postures + the `data_role` = `tenant-content | platform-operational` classification, the
//! X-5 names/units reconciliation point) and §3.1 (the five-operation `PersonalDataHolder`
//! contract). The substrate-seam framing is `00-platform-substrate.md` §2.5.
//!
//! **Contract-index cluster:** 10 — GDPR / Audit / `PersonalDataHolder`
//! (`planning/05-refined-shared-systems-architecture/contract-index.md`): row **10.1**
//! (`PersonalDataHolder{locate, export, rectify, restrict, erase}` — the SIGNATURE frozen
//! here, bodies deferred). The `data_role` enum anchors to row **2.1** (the `EventEnvelope`
//! `data_role` field — the names/units anchor, X-5).
//!
//! ## What crosses the crate boundary here (the frozen surface — P-GA-01 / P-049)
//! This glue crate is a **compile-time contract carrier** (ADR-01): it holds the cross-crate
//! TRAIT SHAPES + TYPE NAMES every store and the DSR orchestrator compile against, frozen in
//! M0 so consumers compile **before any body exists**.
//! - [`PersonalDataHolder`] (10.1) — `locate / export / rectify / restrict / erase`, the
//!   five-operation contract every store implements (gdpr §3.1). The harness
//!   **auto-registers** every store a service opens as a holder (00 §3.4, GD-3) so "we forgot
//!   a store" is structurally impossible — combined with the `no-untagged-personal-data`
//!   lint (P-GA-03). Erasure = purge / crypto-shred / pseudonymise, **never hide** (ADR-12.3).
//! - The core DSR types: [`SubjectRef`], [`TenantId`] (re-exported from `myelin-tenancy` — the
//!   canonical partition key, never re-defined), [`EraseScope`], [`Receipt`], and the
//!   per-operation report/receipt types ([`LocateReport`], [`PortableBundle`], [`Patch`],
//!   [`RectifyReceipt`], [`RestrictReceipt`], [`EraseReceipt`]).
//! - [`DataRole`] — the `tenant-content | platform-operational` classification (gdpr §1.3,
//!   §2.1). It is the **role-tag form** of the same fact the `EventEnvelope.data_role` field
//!   (2.1) carries in `controller | processor` form: `TenantContent == Processor`,
//!   `PlatformOperational == Controller`. The bidirectional anchor mapping is frozen here
//!   ([`DataRole::from_envelope`] / [`DataRole::to_envelope`]) so the two never drift.
//!
//! ## Floors named (stubbed bodies → filling prompt) — VISION §3 name-your-floors
//! All trait bodies are deferred; only the SHAPES are frozen here.
//! - The `PersonalDataHolder` **bodies** (the real locate/export/rectify/restrict/erase over
//!   each store's columns + crypto-shred) → **M1 P-GA-05** (the GDPR-owned holders + the
//!   trait bodies); the upstream-store orchestration → P-GA-06.
//! - The `#[personal_data(...)]` **classify-derive attribute names + the five-tag enum names**
//!   → **M0 P-GA-02** (the macro BODY that emits the registry entry → M1 P-GA-04/P-GA-07).
//! - The **`no-untagged-personal-data` lint** (the ratchet that forces a tag on every PII
//!   field) → **M0 P-GA-03**.
//! - The harness **auto-registration wiring** (contract 1.4) lives in `serve` (P-S12/P-S15)
//!   and is exercised by `myelin-storage`'s holder hook; here only the trait SHAPE it stores
//!   is frozen.
//!
//! ## Reconciliation note (EI-01 §1 — code-wins-over-docs; protocol §4 contract PR)
//! The P-001 workspace skeleton seeded `myelin-gdpr` with an APPROXIMATE `PersonalDataHolder`
//! (a single `Subject` arg, `restrict(subject)` with no `on` flag, `erase(subject)` instead of
//! `erase(EraseScope)`) and an over-reaching `BlobStore` trait. **P-GA-01 is the prompt that
//! FREEZES contract 10.1 to its canonical gdpr §3.1 shape.** This crate now carries the
//! §3.1 signatures verbatim (`locate(subject, tenant)`, `restrict(subject, on)`,
//! `erase(EraseScope)`, each returning its typed receipt). The `BlobStore` trait (contract
//! 11.2) is Storage's, and it already lives in `myelin-storage` (P-ST-03 / P-047) — it is
//! **removed from here** to avoid a parallel second definition (EI-01 §7: never duplicate a
//! type). The lone external consumer of the old shape (`myelin-storage`'s holder hook) is
//! reconciled to the new shape in the same change (one whole-workspace contract PR).

use myelin_identity::Principal;
use myelin_tenancy::TenantId as TenancyTenantId;
use serde::{Deserialize, Serialize};

/// The canonical tenant partition key (re-exported from `myelin-tenancy`, the owning sink
/// crate). The GDPR contract surface (gdpr §3.1) threads `tenant: TenantId` through the holder
/// ops; it is the **same** type the whole platform partitions on — re-exported, never
/// re-defined (EI-01 §7: no parallel second `TenantId`).
pub type TenantId = TenancyTenantId;

/// A reference to the **data subject** a DSR operation targets (gdpr §3.1 — `subject:
/// &SubjectRef`). It identifies *whose* personal data is in scope; the *tenant* the data lives
/// under is passed as a separate `tenant: TenantId` argument (a subject may have data under
/// more than one tenant, so subject and tenant are independent coordinates — gdpr §1.3).
///
/// References-not-payloads: this carries the verified [`Principal`] reference, never PII bodies.
/// The per-field `subject_locator` (how each holder finds the subject's rows) is the
/// `#[personal_data(... subject_locator)]` tag (10.2, P-GA-02), not a field here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectRef {
    /// The verified principal whose data is the subject of the DSR.
    pub principal: Principal,
}

impl SubjectRef {
    /// A subject reference for a verified principal.
    pub fn new(principal: Principal) -> SubjectRef {
        SubjectRef { principal }
    }
}

/// The GDPR fan-out classification of a piece of personal data — the `data_role` tag
/// (gdpr §1.3, §2.1; the X-5 names/units anchor). One classification, threaded identically
/// through the bus envelope (2.1), the data map, and the DSR router.
///
/// This is the **role-tag form** of the classification the `EventEnvelope.data_role` field
/// (contract 2.1) carries in `controller | processor` form. The mapping is frozen and total:
/// - [`DataRole::TenantContent`] ⇔ `myelin_events::DataRole::Processor` — repos/issues/docs/
///   chat/CI logs; the customer org is the controller, a DSR is answered **by/for the tenant**
///   (Art. 28); Myelin must not unilaterally erase tenant content.
/// - [`DataRole::PlatformOperational`] ⇔ `myelin_events::DataRole::Controller` — tenant-admin
///   contacts, billing, the security audit log, telemetry; Myelin is the **first-line DSR
///   responder**.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRole {
    /// tenant content — processor posture (gdpr §1.3).
    TenantContent,
    /// platform-operational data — controller posture (gdpr §1.3).
    PlatformOperational,
}

impl DataRole {
    /// Map this GDPR role-tag onto the frozen 2.1 `EventEnvelope.data_role` form
    /// (`controller | processor`). The anchor (X-5): `TenantContent → Processor`,
    /// `PlatformOperational → Controller`.
    pub fn to_envelope(self) -> myelin_events::DataRole {
        match self {
            DataRole::TenantContent => myelin_events::DataRole::Processor,
            DataRole::PlatformOperational => myelin_events::DataRole::Controller,
        }
    }

    /// The inverse of [`DataRole::to_envelope`] — read the 2.1 envelope form back into the
    /// GDPR role-tag. Total + lossless: the two names denote the same classification.
    pub fn from_envelope(role: myelin_events::DataRole) -> DataRole {
        match role {
            myelin_events::DataRole::Processor => DataRole::TenantContent,
            myelin_events::DataRole::Controller => DataRole::PlatformOperational,
        }
    }
}

/// The scope of an `erase` (gdpr §3.1 — `erase(subject_or_tenant: EraseScope)`; §4.4 — tenant
/// offboarding = `EraseScope::Tenant`). Either a single data subject within one tenant
/// (Art. 17 erasure of a person) or a whole tenant (offboarding — destroy the tenant KEK ⇒ the
/// whole tenant is unrecoverable, P-GA-13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EraseScope {
    /// Erase one data subject's personal data within one tenant (Art. 17).
    Subject {
        /// the data subject to erase.
        subject: SubjectRef,
        /// the tenant the subject's data lives under.
        tenant: TenantId,
    },
    /// Erase a whole tenant (offboarding — destroy the tenant KEK; gdpr §4.4, P-GA-13).
    Tenant(TenantId),
}

/// The content-addressed proof that a holder operation completed (gdpr §3.1 — "each operation
/// returns a **receipt hash-linked into the audit log**"; §549 `… → Receipt`). On THIS floor
/// the receipt is constructed + carries its content-address; the **hash-link / Merkle seal**
/// into the tamper-evident audit log is **P-GA-20**. References-not-payloads: the receipt
/// names the op + a content-address, never PII bodies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// The holder operation this receipt attests (e.g. `"locate"`, `"erase"`).
    pub operation: String,
    /// The content-address of the receipt body (the audit-log hash-link; the Merkle seal is
    /// P-GA-20). A `blake3:<hex>` digest in the M1 bodies; opaque on this skeleton floor.
    pub content_hash: String,
}

/// The result of `locate` (gdpr §3.1 — Art. 15 access: where a subject's data lives within one
/// holder). The located-data model lands with the GDPR M1 bodies (P-GA-05); on this skeleton
/// floor it is the receipt the op produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocateReport {
    /// the content-addressed receipt for the locate op.
    pub receipt: Receipt,
}

/// The result of `export` (gdpr §3.1 — Art. 20 portability: a portable bundle of the subject's
/// data). The full `MerkleProvenBundle` shape (10.4/10.7) lands with the GDPR M1/M2 bodies; on
/// this floor it is the receipt the op produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortableBundle {
    /// the content-addressed receipt for the export op.
    pub receipt: Receipt,
}

/// The patch a `rectify` applies (gdpr §3.1 — Art. 16 rectification). The patch model lands
/// with the GDPR M1 bodies (rectification via reindex-from-source, P-GA-24); on this floor it
/// is an opaque carrier so the signature is frozen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch(pub String);

/// The receipt for a `rectify` (Art. 16).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RectifyReceipt {
    /// the content-addressed receipt for the rectify op.
    pub receipt: Receipt,
}

/// The receipt for a `restrict` (gdpr §3.1 — Art. 18/21 restriction of processing: suppress
/// indexing / agent-use / analytics / notif for the subject; the honoured-everywhere proof is
/// M2 P-GA-25). The `on` flag (set/clear the restriction) is the trait argument.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestrictReceipt {
    /// the content-addressed receipt for the restrict op.
    pub receipt: Receipt,
}

/// The receipt for an `erase` (gdpr §3.1 — Art. 17 erasure = purge / crypto-shred /
/// pseudonymise, never hide). Names the destroyed key/cursor and seals into the audit Merkle
/// tree (P-GA-20).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraseReceipt {
    /// the content-addressed receipt for the erase op.
    pub receipt: Receipt,
}

/// The error a DSR / holder operation can fail with. The real taxonomy (legal-hold blocked,
/// holder unavailable, key-already-destroyed, …) lands with the GDPR M1 bodies (P-GA-05/-11);
/// a string-backed marker on this skeleton floor so the signature is frozen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DsrError(pub String);

/// `Result` alias for the holder surface (gdpr §3.1).
pub type Result<T> = core::result::Result<T, DsrError>;

/// The `PersonalDataHolder` contract — the **only** way the DSR orchestrator touches a store
/// (gdpr §3.1, contract 10.1; ADR-12). Every store and subsystem registers as a holder
/// implementing these five operations for a subject (or a tenant, for offboarding); the
/// harness **auto-registers** every store it opens (contract 1.4, 00 §3.4) so the holder list
/// cannot drift below the data map. Erasure is **purge / crypto-shred / pseudonymise, never
/// hide** (ADR-12.3); `restrict` suppresses indexing / agent-use / analytics / notif for a
/// subject (Art. 18/21).
///
/// The signatures are frozen here to the gdpr §3.1 shape verbatim. **Floor:** every BODY is
/// the GDPR M1 deliverable (P-GA-05 — the GDPR-owned holders + the bodies; P-GA-06 — the
/// upstream-store orchestration). The per-operation CDC pair lands with the bodies (P-GA-05);
/// this prompt's gate is the consumer-compiles-against-signature property.
pub trait PersonalDataHolder {
    /// Art. 15 access — where the subject's data lives within this holder.
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> Result<LocateReport>;
    /// Art. 20 portability — a portable bundle of the subject's data.
    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> Result<PortableBundle>;
    /// Art. 16 rectification — apply a patch to the subject's data.
    fn rectify(&self, subject: &SubjectRef, patch: Patch) -> Result<RectifyReceipt>;
    /// Art. 18/21 restriction — set (`on = true`) or clear (`on = false`) the processing
    /// restriction for the subject (suppress indexing / agent-use / analytics / notif).
    fn restrict(&self, subject: &SubjectRef, on: bool) -> Result<RestrictReceipt>;
    /// Art. 17 erasure — purge / crypto-shred / pseudonymise (never hide) over the scope
    /// (a single subject, or a whole tenant for offboarding).
    fn erase(&self, scope: EraseScope) -> Result<EraseReceipt>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        )
    }

    fn subject() -> SubjectRef {
        SubjectRef::new(principal())
    }

    /// Compile-asserting test: the frozen `PersonalDataHolder` five-method shape (gdpr §3.1 /
    /// contract 10.1) is **object-safe** — usable behind `dyn` exactly as the DSR orchestrator
    /// and the holder registry need (they hold a heterogeneous set of holders). The bodies are
    /// the named GDPR-M1 floor (P-GA-05); here a stub proves the signatures.
    #[test]
    fn personal_data_holder_shape_is_frozen_and_object_safe() {
        struct Store;
        impl PersonalDataHolder for Store {
            fn locate(&self, _s: &SubjectRef, _t: TenantId) -> Result<LocateReport> {
                Err(DsrError("locate body → GDPR M1 (P-GA-05)".into()))
            }
            fn export(&self, _s: &SubjectRef, _t: TenantId) -> Result<PortableBundle> {
                Err(DsrError("export body → GDPR M1 (P-GA-05; 10.4)".into()))
            }
            fn rectify(&self, _s: &SubjectRef, _p: Patch) -> Result<RectifyReceipt> {
                Err(DsrError("rectify body → GDPR M1 (P-GA-05)".into()))
            }
            fn restrict(&self, _s: &SubjectRef, _on: bool) -> Result<RestrictReceipt> {
                Err(DsrError("restrict body → GDPR M1 (P-GA-05)".into()))
            }
            fn erase(&self, _scope: EraseScope) -> Result<EraseReceipt> {
                Err(DsrError(
                    "erase = crypto-shred → GDPR M1 (P-GA-05; ADR-12.3)".into(),
                ))
            }
        }
        // The crux: a `dyn PersonalDataHolder` must exist — the registry/orchestrator hold
        // holders behind a trait object. If any method broke object-safety this would not
        // compile.
        let holder: Box<dyn PersonalDataHolder> = Box::new(Store);
        let subj = subject();
        assert!(holder.locate(&subj, TenantId::from_token("acme")).is_err());
        assert!(holder
            .erase(EraseScope::Tenant(TenantId::from_token("acme")))
            .is_err());
    }

    /// The `data_role` enum is anchored to the frozen 2.1 `EventEnvelope.data_role` field
    /// (the X-5 names/units anchor): the round-trip through the envelope form is total +
    /// lossless, and the mapping is the one gdpr §1.3 fixes (`TenantContent ⇔ Processor`,
    /// `PlatformOperational ⇔ Controller`). If the envelope enum drifts, this fails to compile
    /// or fails the assertion — the two can never silently diverge.
    #[test]
    fn data_role_serializes_to_the_frozen_2_1_envelope_field() {
        // The frozen mapping (gdpr §1.3).
        assert_eq!(
            DataRole::TenantContent.to_envelope(),
            myelin_events::DataRole::Processor
        );
        assert_eq!(
            DataRole::PlatformOperational.to_envelope(),
            myelin_events::DataRole::Controller
        );
        // Total + lossless round-trip through the 2.1 envelope form, both directions.
        for role in [DataRole::TenantContent, DataRole::PlatformOperational] {
            assert_eq!(DataRole::from_envelope(role.to_envelope()), role);
        }
        for env in [
            myelin_events::DataRole::Processor,
            myelin_events::DataRole::Controller,
        ] {
            assert_eq!(DataRole::from_envelope(env).to_envelope(), env);
        }
        // The GDPR role-tag serializes (it lives in the data map + RoPA, P-GA-09).
        let json = serde_json::to_string(&DataRole::TenantContent).unwrap();
        assert_eq!(json, "\"TenantContent\"");
        assert_eq!(
            serde_json::from_str::<DataRole>(&json).unwrap(),
            DataRole::TenantContent
        );
    }

    /// The four core types (gdpr §3.1) round-trip serialize — they cross the crate boundary
    /// (the DSR orchestrator submits a `SubjectRef`/`EraseScope`, holders return a `Receipt`),
    /// so a stable serde shape is part of the frozen contract.
    #[test]
    fn core_types_round_trip_serialize() {
        // SubjectRef
        let s = subject();
        let back: SubjectRef = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);

        // TenantId (the re-exported canonical type)
        let t = TenantId::from_token("acme");
        let t_back: TenantId = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(t_back, t);

        // EraseScope — both variants (subject + tenant offboarding)
        for scope in [
            EraseScope::Subject {
                subject: s.clone(),
                tenant: t.clone(),
            },
            EraseScope::Tenant(t.clone()),
        ] {
            let sc_back: EraseScope =
                serde_json::from_str(&serde_json::to_string(&scope).unwrap()).unwrap();
            assert_eq!(sc_back, scope);
        }

        // Receipt
        let r = Receipt {
            operation: "erase".into(),
            content_hash: "blake3:deadbeef".into(),
        };
        let r_back: Receipt = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r_back, r);
    }
}
