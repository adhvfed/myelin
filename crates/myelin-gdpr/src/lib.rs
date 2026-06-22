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
//! - The classify-derive names are FROZEN HERE (P-GA-02 / P-050): the [`PersonalData`] derive
//!   (re-exported from `myelin-gdpr-macros` at a NO-OP floor), its inert `#[personal_data(...)]`
//!   field-helper attribute, and the five tag enums [`DataCategory`], [`DataRole`] (the `role`
//!   tag), [`LawfulBasis`], [`RetentionClass`], [`ErasureMethod`] — variant NAMES to the §2.1
//!   shape. The macro BODY that emits the registry entry + validates/parses the tag variants is
//!   the M1 floor: P-GA-04 (auto-registration hook) and P-GA-07 (classify-derive body).
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

// The `#[derive(PersonalData)]` macro (P-GA-07) generates code that names the absolute path
// `::myelin_gdpr::HasPersonalData` / `::myelin_gdpr::PersonalDataField` (so a CONSUMER crate
// resolves it through its `myelin-gdpr` dependency). For the derive to also work on THIS crate's
// own types (the `classify_fixture` + the in-crate tests), the crate must be able to name itself
// by that path — `extern crate self as myelin_gdpr` makes `::myelin_gdpr` a valid self-reference.
extern crate self as myelin_gdpr;

use myelin_identity::Principal;
use myelin_tenancy::TenantId as TenancyTenantId;
use serde::{Deserialize, Serialize};

/// The `#[derive(PersonalData)]` classify-derive + its `#[personal_data(category, role, basis,
/// retention, erasure, subject_locator)]` field helper attribute (contract 10.2; gdpr §2.1),
/// re-exported from `myelin-gdpr-macros`.
///
/// A schema owner writes `use myelin_gdpr::PersonalData;`, derives it on the struct, and tags
/// every personal-data field with `#[personal_data(...)]`; the helper arguments reference the five
/// tag enums frozen in this crate ([`DataCategory`], [`DataRole`], [`LawfulBasis`],
/// [`RetentionClass`], [`ErasureMethod`]).
///
/// **P-GA-07 / P-107 — the macro BODY.** The derive is no longer a no-op: it now
/// 1. **emits a generated registry entry** per tagged field — a `&'static [`[`PersonalDataField`]`]`
///    (field path, owning store, the five tag values, the `subject_locator` expression) reachable
///    via the generated [`HasPersonalData::personal_data_fields`] impl — the compile-time inventory
///    the data-map generator (P-GA-09) walks;
/// 2. makes **`subject_locator` structural** — it generates [`HasPersonalData::subject_locator`],
///    the accessor a holder's `locate(subject)` uses to read the subject-key column off a row;
/// 3. **rejects an untagged PII field at compile time** — a field whose name is a PII fingerprint
///    (`email`, `display_name`, …) that carries no `#[personal_data(...)]` tag is a hard
///    `compile_error!` (the type-system form of the `no-untagged-personal-data` lint, the floor the
///    lint named landing in P-107).
///
/// See the `myelin-gdpr-macros` crate doc for the parsing grammar + the
/// `::myelin_gdpr::__registry::*` path the generated code resolves through this crate.
pub use myelin_gdpr_macros::PersonalData;

pub mod __registry;
pub use __registry::{
    default_data_role_default, DataRoleDefault, ErasureKeyClass, HasPersonalData,
    PersonalDataField, PersonalDataTags, SpecialCategoryFlag,
};

/// The `SpecialCategory` → DPIA router (contract 10.2; gdpr §2.3) — P-GA-08 / P-108. The
/// special-category *detection* is P-107 ([`SpecialCategoryFlag`]); this module is the *routing*
/// layer: it mints a [`dpia::DpiaMarker`] into the generated inventory per special-category field
/// and the [`dpia::DpiaRouter`] records a newly-appeared marker as a DPIA-required change the
/// data-map diff gate (P-GA-10) surfaces to a DPO (surfaced, not auto-decided).
pub mod dpia;
pub use dpia::{dpia_markers, dpia_markers_of, DpiaMarker, DpiaRouter, DpiaVerdict};

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

// ════════════════════════════════════════════════════════════════════════════════════════════
// The five-tag classification enum NAMES (contract 10.2; gdpr §2.1) — FROZEN here (P-GA-02 /
// P-050) so every store compiles against the classification surface before the macro BODY exists.
//
// The `#[personal_data(category, role, basis, retention, erasure, subject_locator)]` attribute
// (re-exported above from `myelin-gdpr-macros`) references these five enums:
//   category  → DataCategory      role → DataRole (above)   basis → LawfulBasis
//   retention → RetentionClass    erasure → ErasureMethod    subject_locator → a string expr
//
// FLOOR (named): the variant NAMES are frozen now to the §2.1 shape; the macro BODY that PARSES
// these tags out of an attribute token-stream and EMITS the generated registry entry is M1
// P-GA-04 / P-GA-07. The payload-carrying variants (`SpecialCategory(..)`, `LegitimateInterest`,
// `Consent`, `Fixed`, `AuditCarveOut`, `CryptoShred`) carry the §2.1-named payloads now so the
// variant SHAPE is stable; the full validation/parsing of those payloads lands with that body.
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The `category` tag (gdpr §2.1): WHAT KIND of personal data the field carries. The category a
/// rights pipeline keys on for breach-scoping (Arts. 33–34) and special-category routing (Art. 9
/// → the DPIA gate, P-GA-08). Frozen to the §2.1 shape:
/// `ContactInfo | Identifier | Content | Behavioural | SpecialCategory(...)`.
///
/// `SpecialCategory` is the mechanical flag that routes a field into the DPIA gate (gdpr §2.4,
/// OQ-H) — the worklog/productivity-sensitivity case. Its payload (the Art. 9 special-category
/// kind) is named now as a string ref; the typed special-category vocabulary lands with the
/// macro body (P-GA-07) + the DPIA router (P-GA-08).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataCategory {
    /// contact details — email / phone / address (gdpr §2.1).
    ContactInfo,
    /// a direct or pseudonymous identifier — principal id, account handle (gdpr §2.1).
    Identifier,
    /// user-authored content — issue bodies, doc blocks, chat messages, commit text (gdpr §2.1).
    Content,
    /// behavioural / observational data — worklog, velocity, activity (gdpr §2.1, §2.4 OQ-H;
    /// restricted-by-default in cross-individual processing).
    Behavioural,
    /// Art. 9 special-category data — routes into the DPIA gate (gdpr §2.3, P-GA-08). The
    /// special-category kind reference; the typed vocabulary is P-GA-07.
    SpecialCategory(String),
}

/// The `basis` tag (gdpr §2.1): the LAWFUL BASIS for processing the field (Art. 6 / Art. 9(2)).
/// Frozen to the §2.1 shape:
/// `Contract | LegitimateInterest(lia_ref) | Consent(consent_id) | LegalObligation`.
///
/// A `[TBD_LEGAL]` basis (gdpr §2.4) is a NAMED residual recorded against the field, never a
/// blocker — counsel ratifies it; engineering carries the tag. The `lia_ref` / `consent_id`
/// payloads are named now as string refs; they resolve into the LIA register / consent registry
/// (G5, P-GA-23) with the macro body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LawfulBasis {
    /// Art. 6(1)(b) — performance of a contract (gdpr §2.1).
    Contract,
    /// Art. 6(1)(f) — legitimate interest; carries the LIA (legitimate-interest-assessment)
    /// reference (gdpr §2.1). The `lia_ref` resolves into the LIA register with P-GA-07.
    LegitimateInterest(String),
    /// Art. 6(1)(a) — consent; carries the `consent_id` into the consent registry (G5, P-GA-23).
    Consent(String),
    /// Art. 6(1)(c) — compliance with a legal obligation (gdpr §2.1; e.g. the audit carve-out).
    LegalObligation,
}

/// The `retention` tag (gdpr §2.1): HOW LONG the field may be retained (Art. 5(1)(e)). Frozen to
/// the §2.1 shape: `TenantPolicy | Fixed(Duration) | UntilContractEnd | AuditCarveOut(Duration)`.
///
/// The retention ENGINE (tightest-policy-wins merge + legal-hold-aware suspend-don't-delete) is
/// P-GA-22; this tag is the per-field INPUT it merges. `Fixed`/`AuditCarveOut` carry a
/// [`core::time::Duration`] — the std type, no parallel duration newtype (EI-01 §7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetentionClass {
    /// retained per the tenant's configured retention policy (gdpr §2.1, §5.1; tightest-wins).
    TenantPolicy,
    /// retained for a fixed duration, then crypto-shred-expired (gdpr §2.1).
    Fixed(core::time::Duration),
    /// retained until the tenant's contract ends, then offboarding-erased (gdpr §2.1, §4.4).
    UntilContractEnd,
    /// the audit-log retention carve-out (gdpr §6.4, GD-5) — a per-jurisdiction legal-retention
    /// floor that suspends erasure for the held duration, then expires via audit-key shred.
    AuditCarveOut(core::time::Duration),
}

/// The `erasure` tag (gdpr §2.1): the MECHANISM by which the field is erased on an Art. 17 right
/// (erasure = purge / crypto-shred / pseudonymise, **never hide** — ADR-12.3). Frozen to the
/// §2.1 shape: `Pseudonymise | CryptoShred(key_class) | PurgeReindex | CarveOut`.
///
/// This tag is what the erase fan-out (P-GA-06 canonical erase order) dispatches on per holder
/// per field. `CryptoShred` carries the `key_class` (which key-hierarchy class to destroy —
/// per-subject DEK | per-tenant KEK; Storage 11.3/11.4); named now as a string ref, resolved
/// into the KMS hierarchy with the macro body + the holder bodies (P-GA-05).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErasureMethod {
    /// replace the subject's identifying values with a stable pseudonym (gdpr §2.1, §3.2).
    Pseudonymise,
    /// destroy the key whose ciphertext is the only copy — the ciphertext becomes unrecoverable
    /// (gdpr §2.1, §3.2; the `key_class` names which key-hierarchy class to shred).
    CryptoShred(String),
    /// purge the rows and reindex-from-source the derived stores (gdpr §2.1; P-GA-24).
    PurgeReindex,
    /// the audit carve-out — suppress/restrict rather than purge, expiring via key shred at the
    /// retention floor (gdpr §6.4, GD-5; the H16 audit carve-out, P-GA-19).
    CarveOut,
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
///
/// **P-GA-05 (the bodies):** [`Receipt::content_addressed`] builds the `content_hash` as a
/// **BLAKE3 digest over a canonical, PII-free receipt body** (op + holder + subject/tenant ids +
/// outcome + the destroyed key epoch + timestamp). The digest is rendered `blake3:<hex>` — the
/// ONE multihash convention the BlobStore content-address and the audit Merkle leaf also use, so
/// a receipt is verifiable + content-addressed, never hand-rolled. `key_epoch_destroyed` records
/// **which key epoch a crypto-shred destroyed** (the GD-4 erasure lever's audit trail — the prompt
/// requirement "every holder op returns a receipt recording the destroyed key epoch"); it is
/// `None` for a non-shredding op (locate/export/rectify/restrict) and `Some(epoch)` for an erase
/// that destroyed a key. `#[serde(default)]` keeps the frozen P-GA-01 wire shape stable (an old
/// `{operation, content_hash}` receipt round-trips to `key_epoch_destroyed: None`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// The holder operation this receipt attests (e.g. `"locate"`, `"erase"`).
    pub operation: String,
    /// The content-address of the receipt body (the audit-log hash-link; the Merkle seal is
    /// P-GA-20). A `blake3:<hex>` digest in the M1 bodies; opaque on the skeleton floor.
    pub content_hash: String,
    /// The key epoch a crypto-shred destroyed (the GD-4 lever's audit trail). `None` for a
    /// non-shredding op; `Some(epoch)` when an erase destroyed a key. Folded INTO the
    /// content-addressed body so a receipt cannot claim a destroyed epoch it did not address.
    #[serde(default)]
    pub key_epoch_destroyed: Option<u64>,
}

impl Receipt {
    /// Build a **content-addressed** receipt: hash a canonical, PII-free body
    /// (`operation ∥ holder ∥ subject ∥ tenant ∥ outcome ∥ key_epoch_destroyed ∥ at_ms`) with
    /// BLAKE3 and render `blake3:<hex>` (gdpr §3.1 — each op returns a receipt; §6 — hash-linked
    /// into the audit log; the Merkle seal is P-GA-20). The body carries only **opaque ids**
    /// (the pseudonymous subject id / the tenant token) — never PII — so the receipt itself is
    /// safe to seal into the tamper-evident log.
    ///
    /// Deterministic: the SAME (op, holder, subject, tenant, outcome, epoch, at) always yields the
    /// SAME content hash — so an **idempotent re-erase returns the identical receipt** (the prompt
    /// requirement). The `at` for an idempotent re-run is the FIRST erase's timestamp (the holder
    /// re-affirms the original completion), keeping the content-address stable across re-runs.
    pub fn content_addressed(
        operation: &str,
        holder: &str,
        subject: &str,
        tenant: &str,
        outcome: &str,
        key_epoch_destroyed: Option<u64>,
        at_ms: u64,
    ) -> Receipt {
        // The canonical body — field-tagged + `∥`-joined so two different field sets can never
        // collide into the same digest (a fixed separator, not raw concatenation).
        let body = format!(
            "op={operation}\u{1f}holder={holder}\u{1f}subject={subject}\u{1f}tenant={tenant}\
             \u{1f}outcome={outcome}\u{1f}key_epoch={}\u{1f}at={at_ms}",
            match key_epoch_destroyed {
                Some(e) => e.to_string(),
                None => "none".to_string(),
            }
        );
        let digest = blake3::hash(body.as_bytes());
        Receipt {
            operation: operation.to_string(),
            content_hash: format!("blake3:{}", hex::encode(digest.as_bytes())),
            key_epoch_destroyed,
        }
    }
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

/// A **fixture store** that exercises the frozen P-GA-02 surface in real (compiled, non-test)
/// crate source: it derives `#[derive(PersonalData)]`, tags a field with the `#[personal_data(...)]`
/// helper, and references a variant of each of the five tag enums. Its purpose is the GATE — "a
/// store can write `#[derive(PersonalData)]` + `#[personal_data(...)]` and reference each enum
/// variant now; it will not compile correctly against drift later". It is also the GREEN witness
/// the live `no-untagged-personal-data` lint (P-GA-03) scans: the PII-typed field below carries
/// the tag, so the lint ADMITS it.
///
/// **Floor:** the derive is a no-op here (it emits nothing; the struct + helper are left as
/// written); the registry-entry emission is P-GA-04 / P-GA-07. This is a fixture (a compile-surface
/// witness), not a real holder — the real M1 stores carry their own tagged fields.
pub mod classify_fixture {
    use super::{DataCategory, DataRole, ErasureMethod, LawfulBasis, PersonalData, RetentionClass};

    /// A toy contact record proving the `#[derive(PersonalData)]` derive + the `#[personal_data(...)]`
    /// helper + each of the five tag enums compile when applied/referenced by a store. The `email`
    /// field is a PII-fingerprinted field name (the `no-untagged-personal-data` lint's `PII_FIELDS`
    /// set) carrying the tag — so it is also the LIVE green fixture: tagged ⇒ admitted.
    #[derive(PersonalData)]
    pub struct ContactRecord {
        /// a non-PII key — no tag needed.
        pub id: u64,
        /// a PII field, tagged with the full five-tag classification (the helper compiles). Kept
        /// on ONE line so the `no-untagged-personal-data` scanner (which checks the immediately
        /// preceding line for `#[personal_data`) admits it — the multi-line-attribute case is a
        /// scanner refinement for P-GA-03/P-051, not weakened here.
        #[personal_data(category = ContactInfo, role = TenantContent, basis = Contract, retention = TenantPolicy, erasure = CryptoShred(subject_dek), subject_locator = "id")]
        pub email: String,
    }

    impl ContactRecord {
        /// Construct a fixture record AND reference one variant of each of the five tag enums —
        /// proving the variant NAMES are frozen + usable by a store today. The values are not
        /// stored (the registry-emitting macro body is the P-GA-07 floor); referencing them is
        /// the compile-surface gate.
        pub fn new(id: u64, email: String) -> ContactRecord {
            // Reference one variant of each of the five tag enums (the frozen NAMES).
            let _category = DataCategory::ContactInfo;
            let _role = DataRole::TenantContent;
            let _basis = LawfulBasis::Contract;
            let _retention = RetentionClass::TenantPolicy;
            let _erasure = ErasureMethod::CryptoShred("subject_dek".into());
            ContactRecord { id, email }
        }
    }
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
            key_epoch_destroyed: Some(3),
        };
        let r_back: Receipt = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r_back, r);

        // A legacy `{operation, content_hash}` receipt (no key_epoch_destroyed) round-trips to
        // `None` — `#[serde(default)]` keeps the frozen P-GA-01 wire shape stable.
        let legacy: Receipt =
            serde_json::from_str(r#"{"operation":"locate","content_hash":"blake3:00"}"#).unwrap();
        assert_eq!(legacy.key_epoch_destroyed, None);
    }

    /// P-GA-05 — the content-addressed receipt machinery (gdpr §3.1, §6): a holder op's receipt
    /// is a BLAKE3 `blake3:<hex>` digest over a canonical PII-free body recording the destroyed
    /// key epoch, and it is **deterministic** (an idempotent re-erase returns the IDENTICAL
    /// receipt — the prompt requirement). A different field flips the address.
    #[test]
    fn content_addressed_receipt_is_deterministic_and_records_the_key_epoch() {
        let r1 = Receipt::content_addressed(
            "erase",
            "oltp:dsr_request",
            "u-1",
            "acme",
            "crypto_shred",
            Some(7),
            1_000,
        );
        let r2 = Receipt::content_addressed(
            "erase",
            "oltp:dsr_request",
            "u-1",
            "acme",
            "crypto_shred",
            Some(7),
            1_000,
        );
        // Deterministic: same inputs → same content-address (so a re-erase returns the same receipt).
        assert_eq!(r1, r2);
        assert!(r1.content_hash.starts_with("blake3:"));
        assert_eq!(
            r1.key_epoch_destroyed,
            Some(7),
            "the destroyed key epoch is recorded"
        );
        // A different destroyed epoch is a DIFFERENT receipt (the epoch is folded into the address).
        let r3 = Receipt::content_addressed(
            "erase",
            "oltp:dsr_request",
            "u-1",
            "acme",
            "crypto_shred",
            Some(8),
            1_000,
        );
        assert_ne!(r1.content_hash, r3.content_hash);
        // A non-shredding op records no epoch.
        let loc = Receipt::content_addressed(
            "locate",
            "oltp:dsr_request",
            "u-1",
            "acme",
            "located",
            None,
            5,
        );
        assert_eq!(loc.key_epoch_destroyed, None);
        assert_ne!(loc.content_hash, r1.content_hash);
    }

    // ───────────────────────── P-GA-02 / P-050 — the classify surface ──────────────────────────

    /// The `#[personal_data(...)]` field helper (under `#[derive(PersonalData)]`) parses its FIVE
    /// TAG KEYS (`category` / `role` / `basis` / `retention` / `erasure` / `subject_locator`)
    /// without erroring (contract 10.2; gdpr §2.1). At the NO-OP floor the derive emits nothing
    /// and the helper is inert, so the proof is structural: a struct deriving `PersonalData` with
    /// a field carrying the full six-key helper COMPILES (the helper is only legal under the
    /// derive — without it this is a hard `cannot find attribute` error if the names were not
    /// frozen + re-exported). The struct being constructable + its field readable proves the no-op
    /// derive left the item unchanged.
    #[test]
    fn personal_data_attribute_parses_the_five_tag_keys() {
        // Apply the derive + the helper with ALL five tag keys (+ subject_locator) — must compile.
        #[derive(PersonalData)]
        struct Tagged {
            #[personal_data(
                category = Identifier,
                role = PlatformOperational,
                basis = LegitimateInterest(ops_lia),
                retention = Fixed(90d),
                erasure = PurgeReindex,
                subject_locator = "principal_id"
            )]
            principal_id: String,
        }
        // The no-op derive left the item unchanged: the field is present + readable.
        let t = Tagged {
            principal_id: "p-1".into(),
        };
        assert_eq!(t.principal_id, "p-1");

        // The compiled, non-test fixture store (`classify_fixture`) also applies the attribute +
        // references each enum variant — exercise it here to bind the gate to a real store shape.
        let rec = classify_fixture::ContactRecord::new(7, "a@b.test".into());
        assert_eq!(rec.id, 7);
        assert_eq!(rec.email, "a@b.test");
    }

    /// Each of the FIVE TAG ENUMS exists with its §2.1 variant NAMES, and every variant
    /// round-trips through serde (the tags live in the generated data map + RoPA, P-GA-09, so a
    /// stable serde shape is part of the frozen contract). Constructing every variant by name is
    /// the drift guard: if a §2.1 variant name changed, this fails to compile.
    #[test]
    fn five_tag_enum_names_and_variants_exist_and_round_trip() {
        // category: ContactInfo | Identifier | Content | Behavioural | SpecialCategory(...)
        let categories = [
            DataCategory::ContactInfo,
            DataCategory::Identifier,
            DataCategory::Content,
            DataCategory::Behavioural,
            DataCategory::SpecialCategory("health".into()),
        ];
        // role: TenantContent | PlatformOperational (the `role` tag is DataRole, frozen in P-GA-01)
        let roles = [DataRole::TenantContent, DataRole::PlatformOperational];
        // basis: Contract | LegitimateInterest(lia_ref) | Consent(consent_id) | LegalObligation
        let bases = [
            LawfulBasis::Contract,
            LawfulBasis::LegitimateInterest("lia-1".into()),
            LawfulBasis::Consent("consent-1".into()),
            LawfulBasis::LegalObligation,
        ];
        // retention: TenantPolicy | Fixed(Duration) | UntilContractEnd | AuditCarveOut(Duration)
        let retentions = [
            RetentionClass::TenantPolicy,
            RetentionClass::Fixed(core::time::Duration::from_secs(86_400)),
            RetentionClass::UntilContractEnd,
            RetentionClass::AuditCarveOut(core::time::Duration::from_secs(86_400 * 365)),
        ];
        // erasure: Pseudonymise | CryptoShred(key_class) | PurgeReindex | CarveOut
        let erasures = [
            ErasureMethod::Pseudonymise,
            ErasureMethod::CryptoShred("subject_dek".into()),
            ErasureMethod::PurgeReindex,
            ErasureMethod::CarveOut,
        ];

        fn round_trip<T>(values: &[T])
        where
            T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
        {
            for v in values {
                let json = serde_json::to_string(v).unwrap();
                let back: T = serde_json::from_str(&json).unwrap();
                assert_eq!(&back, v, "tag enum variant must round-trip: {json}");
            }
        }
        round_trip(&categories);
        round_trip(&roles);
        round_trip(&bases);
        round_trip(&retentions);
        round_trip(&erasures);

        // Spot-check a frozen variant NAME serializes to its §2.1 spelling (drift tripwire).
        assert_eq!(
            serde_json::to_string(&DataCategory::ContactInfo).unwrap(),
            "\"ContactInfo\""
        );
        assert_eq!(
            serde_json::to_string(&ErasureMethod::PurgeReindex).unwrap(),
            "\"PurgeReindex\""
        );
    }

    // ───────────────────── P-GA-07 / P-107 — the classify-derive macro BODY ─────────────────────

    /// The derive EMITS a complete generated registry entry for each of the five tag axes (gdpr
    /// §2.2; the compile-time inventory the data-map generator P-GA-09 walks). One struct exercises
    /// every category / role / basis / retention / erasure variant FORM across its fields; the
    /// derive captures each tag's text into a [`PersonalDataField`] and the impl exposes them as a
    /// `&'static` slice. This is the registry-emission proof + the mutation-core path (the
    /// tag-parsing + registry-emission body): if the macro dropped or mis-captured a tag, a field's
    /// entry would be wrong here.
    #[test]
    fn derive_emits_a_complete_registry_entry_for_every_tag_form() {
        #[derive(PersonalData)]
        #[allow(dead_code)]
        struct EveryTag {
            // category=ContactInfo, role=TenantContent, basis=Contract, retention=TenantPolicy,
            // erasure=Pseudonymise (a bare variant on every axis + a literal locator).
            #[personal_data(
                category = ContactInfo,
                role = TenantContent,
                basis = Contract,
                retention = TenantPolicy,
                erasure = Pseudonymise,
                subject_locator = "principal_id"
            )]
            contact: String,
            // category=SpecialCategory(health) (a CALL-form category — the DPIA route),
            // basis=Consent(c-1), retention=Fixed(90d) (a NON-Rust-expr payload `90d`),
            // erasure=CryptoShred(subject_dek) (the GD-4 key class).
            #[personal_data(
                category = SpecialCategory(health),
                role = PlatformOperational,
                basis = Consent(c-1),
                retention = Fixed(90d),
                erasure = CryptoShred(subject_dek),
                subject_locator = "subject_ref"
            )]
            sensitive: String,
            // basis=LegitimateInterest(ops_lia) (a bare-ident payload), retention=UntilContractEnd,
            // erasure=PurgeReindex (the derived-store rebuild method).
            #[personal_data(
                category = Identifier,
                role = TenantContent,
                basis = LegitimateInterest(ops_lia),
                retention = UntilContractEnd,
                erasure = PurgeReindex,
                subject_locator = "id"
            )]
            handle: String,
            // A non-PII untagged field is fine — it carries no personal data, so no entry.
            row_version: u64,
        }

        let fields = EveryTag::personal_data_fields();
        assert_eq!(
            fields.len(),
            3,
            "one entry per TAGGED field, the non-PII field has none"
        );

        // The owning struct + field path is captured on every entry.
        assert!(fields.iter().all(|f| f.owning_struct == "EveryTag"));
        let by_field: std::collections::HashMap<&str, &PersonalDataField> =
            fields.iter().map(|f| (f.field, f)).collect();

        // Field 1 — every-bare-variant form, captured verbatim.
        let contact = by_field["contact"];
        assert_eq!(contact.tags.category, "ContactInfo");
        assert_eq!(contact.tags.role, "TenantContent");
        assert_eq!(contact.tags.basis, "Contract");
        assert_eq!(contact.tags.retention, "TenantPolicy");
        assert_eq!(contact.tags.erasure, "Pseudonymise");
        // subject_locator is the LITERAL'S INNER VALUE (the column name a holder reads).
        assert_eq!(contact.tags.subject_locator, "principal_id");

        // Field 2 — call-form payloads captured as whitespace-free text; the routing extractors work.
        let sensitive = by_field["sensitive"];
        assert_eq!(sensitive.tags.category, "SpecialCategory(health)");
        assert_eq!(sensitive.tags.basis, "Consent(c-1)");
        assert_eq!(sensitive.tags.retention, "Fixed(90d)"); // `90d` is NOT a Rust expr — captured as text.
        assert_eq!(sensitive.tags.erasure, "CryptoShred(subject_dek)");
        // The GD-4 key choice + the DPIA route are structural off the captured text.
        assert_eq!(
            sensitive.erasure_key_class(),
            Some(ErasureKeyClass::SubjectDek)
        );
        assert_eq!(
            sensitive.is_special_category(),
            Some(SpecialCategoryFlag { kind: "health" })
        );

        // Field 3 — a bare-ident payload (`LegitimateInterest(ops_lia)`) captured intact.
        let handle = by_field["handle"];
        assert_eq!(handle.tags.basis, "LegitimateInterest(ops_lia)");
        assert_eq!(handle.tags.erasure, "PurgeReindex");
        assert_eq!(handle.erasure_key_class(), None); // PurgeReindex names no key class.
        assert_eq!(handle.is_special_category(), None);
    }

    /// `subject_locator` is STRUCTURAL (gdpr §2.1): the derive generates the accessor a holder's
    /// `locate(subject)` uses to find the subject-key column off a row. The accessor resolves a
    /// tagged field's locator and returns `None` for an unknown/untagged field.
    #[test]
    fn subject_locator_accessor_is_structural() {
        // The real S1-shaped fixture (`classify_fixture::ContactRecord`) carries `email` tagged
        // with `subject_locator = "id"`.
        use classify_fixture::ContactRecord;
        assert_eq!(ContactRecord::subject_locator("email"), Some("id"));
        // An untagged / unknown field resolves to no locator.
        assert_eq!(ContactRecord::subject_locator("id"), None);
        assert_eq!(ContactRecord::subject_locator("does_not_exist"), None);
        // The registry over the fixture has exactly the one tagged field.
        let fields = ContactRecord::personal_data_fields();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field, "email");
        assert_eq!(fields[0].tags.erasure, "CryptoShred(subject_dek)");
    }

    /// A struct with NO tagged field still implements `HasPersonalData` with an empty registry —
    /// the derive is UNIFORM (every `PersonalData` type is walkable by P-GA-09; an empty entry set
    /// is the truthful "this struct carries no PII" answer, not a missing impl). Covers both the
    /// named-struct-no-tags path AND the tuple/unit-struct path (which the macro routes through its
    /// own `empty_impl` — both MUST yield a real, callable `HasPersonalData` impl, never a no-op).
    #[test]
    fn derive_is_uniform_empty_registry_for_a_pii_free_struct() {
        // Named struct, no tagged fields → the main path with an empty entry set.
        #[derive(PersonalData)]
        #[allow(dead_code)]
        struct NoPii {
            id: u64,
            region: String,
        }
        // The impl EXISTS and is callable (if the derive emitted nothing, this would not compile).
        assert!(NoPii::personal_data_fields().is_empty());
        assert_eq!(NoPii::subject_locator("id"), None);

        // Tuple struct → the macro's `empty_impl` path. It too MUST yield a real impl (the derive
        // is uniform); a no-op there would fail to resolve `personal_data_fields` and not compile.
        #[derive(PersonalData)]
        #[allow(dead_code)]
        struct TupleRow(u64, String);
        assert!(TupleRow::personal_data_fields().is_empty());
        assert_eq!(TupleRow::subject_locator("0"), None);

        // Unit struct → also the `empty_impl` path.
        #[derive(PersonalData)]
        struct UnitRow;
        assert!(UnitRow::personal_data_fields().is_empty());
    }
}

/// **The type-system form of the `no-untagged-personal-data` lint** (P-GA-07 / P-107 — the floor
/// `myelin-lints` named landing here). A struct that DERIVES `PersonalData` with a PII-named field
/// (`email`) carrying NO `#[personal_data(...)]` tag is a HARD COMPILE ERROR — the un-erasable /
/// un-mapped subject bug class cannot compile. This `compile_fail` doc-test is the RED fixture
/// (the macro fires); the GREEN fixture is every M1 store + `classify_fixture::ContactRecord`
/// (which compile, the tag present). The two together are the derive-face of GA-D5.
///
/// ```compile_fail
/// use myelin_gdpr::PersonalData;
/// // An `email` field deriving PersonalData with NO `#[personal_data(...)]` tag must FAIL to
/// // compile — the macro refuses to expand an untagged PII column.
/// #[derive(PersonalData)]
/// struct Leaky {
///     email: String,
/// }
/// ```
///
/// The contrasting GREEN form compiles (the same field, tagged):
/// ```
/// use myelin_gdpr::PersonalData;
/// #[derive(PersonalData)]
/// struct Tagged {
///     #[personal_data(
///         category = ContactInfo, role = TenantContent, basis = Contract,
///         retention = TenantPolicy, erasure = CryptoShred(subject_dek),
///         subject_locator = "id",
///     )]
///     email: String,
/// }
/// ```
pub mod untagged_pii_rejection_doc {}
