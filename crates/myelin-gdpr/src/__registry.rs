//! # `__registry` — the compile-time personal-data inventory the classify-derive EMITS
//! (contract 10.2; gdpr §2.1 / §2.2) — P-GA-07 / P-107
//!
//! The `#[derive(PersonalData)]` macro body (`myelin-gdpr-macros`) emits, for every
//! `#[personal_data(...)]`-tagged field of a struct, a [`PersonalDataField`] **registry entry**
//! into a `&'static` table reachable via the generated [`HasPersonalData`] impl. This is the
//! **compile-time-collected inventory the data-map generator (P-GA-09) walks** — *the map, not a
//! hand-written list, drives erasure / RoPA / breach-scoping*, so "we forgot a field" is
//! structurally impossible (the derive can only emit what the field carries, and the
//! `no-untagged-personal-data` lint + the derive's own compile-error both force the field to carry
//! a tag at all).
//!
//! ## Why string-typed tag values (the reconciliation, EI-01 §1)
//! The `#[personal_data(...)]` helper tags use **bare-identifier enum variants** in the schema
//! source (`category = ContactInfo`, `erasure = CryptoShred(subject_dek)`) — they are NOT
//! resolvable Rust expressions the macro could evaluate (the macro runs before type-checking, and
//! a payload like `subject_dek` / `ops_lia` / `90d` is a bare token, not a const). The macro
//! therefore captures each tag's **rendered token text** into the registry entry; the typed
//! five-tag enums ([`crate::DataCategory`] et al.) remain the surface a holder/orchestrator
//! pattern-matches on, and the data-map generator (P-GA-09) re-parses the string tags into them.
//! This keeps the derive hermetic (no path-resolution, no const-eval) while still emitting a
//! complete, machine-readable entry. The string form is the registry's WIRE shape; the enums are
//! the in-memory shape.
//!
//! ## What the consumer (P-GA-09) gets
//! For a tagged struct `S`, `S::personal_data_fields()` returns the `&'static [PersonalDataField]`
//! and `s.subject_locator(field)` resolves the subject-key column the holder's `locate` uses.

use serde::{Deserialize, Serialize};

/// One generated registry entry — a single `#[personal_data(...)]`-tagged field (contract 10.2;
/// gdpr §2.2). The data-map generator (P-GA-09) walks the union of these over every registered
/// holder to build the machine-readable PII inventory (what PII exists, where, role/basis/category,
/// retention, locator). Emitted as `&'static` by the derive — no allocation, no runtime collection.
///
/// All five tag values + the `subject_locator` are captured as the **rendered token text** of the
/// helper argument (see the module doc): `tags.category == "ContactInfo"`,
/// `tags.erasure == "CryptoShred(subject_dek)"`. The typed re-parse into [`crate::DataCategory`]
/// etc. is the data-map generator's job (P-GA-09); the convenience extractors
/// [`PersonalDataField::erasure_key_class`] / [`PersonalDataField::is_special_category`] cover the
/// two routing decisions consumers make on the raw form today (the GD-4 key choice + the DPIA
/// route, P-GA-08).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalDataField {
    /// The owning struct's name (the "store" / schema the field lives in), e.g. `"PrincipalProfile"`.
    pub owning_struct: &'static str,
    /// The field's identifier within the struct, e.g. `"email"`. `owning_struct ∥ field` is the
    /// field PATH the inventory keys on.
    pub field: &'static str,
    /// The five classification tags + the subject locator, as captured token text.
    pub tags: PersonalDataTags,
}

impl PersonalDataField {
    /// The `erasure = CryptoShred(<key_class>)` key class, if this field is crypto-shred-erased
    /// (the GD-4 per-subject-vs-per-tenant key choice the Storage `key_class_for` rule consumes —
    /// gdpr §3.2, contract 11.4). `Some("subject_dek")` / `Some("tenant_kek")` for a `CryptoShred`
    /// tag; `None` for `Pseudonymise` / `PurgeReindex` / `CarveOut`.
    pub fn erasure_key_class(&self) -> Option<ErasureKeyClass> {
        ErasureKeyClass::from_erasure_tag(self.tags.erasure)
    }

    /// Whether this field is `category = SpecialCategory(...)` (Art. 9) — the mechanical flag that
    /// routes the field into the DPIA gate (gdpr §2.3). The *router* on top of this flag is
    /// [`crate::dpia`] (P-GA-08): it mints a `DpiaMarker` into the inventory and records a
    /// newly-appeared marker as DPIA-required. Carries the special-category kind reference when
    /// present.
    pub fn is_special_category(&self) -> Option<SpecialCategoryFlag> {
        SpecialCategoryFlag::from_category_tag(self.tags.category)
    }
}

/// The five classification tags + `subject_locator`, captured as rendered token text (see the
/// module doc on why string-typed). The order mirrors gdpr §2.1:
/// `category | role | basis | retention | erasure | subject_locator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalDataTags {
    /// `category` — `ContactInfo | Identifier | Content | Behavioural | SpecialCategory(...)`.
    pub category: &'static str,
    /// `role` — `TenantContent | PlatformOperational`.
    pub role: &'static str,
    /// `basis` — `Contract | LegitimateInterest(..) | Consent(..) | LegalObligation`.
    pub basis: &'static str,
    /// `retention` — `TenantPolicy | Fixed(..) | UntilContractEnd | AuditCarveOut(..)`.
    pub retention: &'static str,
    /// `erasure` — `Pseudonymise | CryptoShred(..) | PurgeReindex | CarveOut`.
    pub erasure: &'static str,
    /// `subject_locator` — the column/expression a holder's `locate(subject)` reads the subject
    /// key off (gdpr §2.1: makes `locate` structural). The captured string LITERAL value, e.g.
    /// `"principal_id"`.
    pub subject_locator: &'static str,
}

/// The crypto-shred key class an `erasure = CryptoShred(<class>)` tag names — the GD-4 lever's
/// per-subject-vs-per-tenant key choice (gdpr §3.2; Storage `key_class_for`, contract 11.4). The
/// raw tag carries a bare key-class identifier; this extracts the two classes the platform uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErasureKeyClass {
    /// per-SUBJECT DEK — the individual erasure lever (Art. 17 erasure of one person): destroy the
    /// subject's key ⇒ only that subject's ciphertext is unrecoverable (gdpr §3.2, GD-4).
    SubjectDek,
    /// per-TENANT KEK — the bulk/offboarding class (tenant offboarding destroys the tenant KEK ⇒
    /// the whole tenant is unrecoverable; gdpr §4.4).
    TenantKek,
    /// any other named key class (forward-compatible; the data-map generator surfaces it verbatim).
    Other(&'static str),
}

impl ErasureKeyClass {
    /// Parse `erasure` tag text into a key class iff it is a `CryptoShred(<class>)`. `None` for the
    /// non-shredding erasure methods.
    pub fn from_erasure_tag(erasure: &'static str) -> Option<ErasureKeyClass> {
        let inner = erasure
            .strip_prefix("CryptoShred(")
            .and_then(|s| s.strip_suffix(')'))?
            .trim();
        Some(match inner {
            "subject_dek" => ErasureKeyClass::SubjectDek,
            "tenant_kek" => ErasureKeyClass::TenantKek,
            other => ErasureKeyClass::Other(other),
        })
    }
}

/// The `category = SpecialCategory(<kind>)` flag (Art. 9) — the DPIA-route marker (gdpr §2.3;
/// router P-GA-08). Carries the special-category kind reference as captured text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialCategoryFlag {
    /// the special-category kind reference, e.g. `"health"` (the typed vocabulary is P-GA-08).
    pub kind: &'static str,
}

impl SpecialCategoryFlag {
    /// Parse `category` tag text into a special-category flag iff it is `SpecialCategory(<kind>)`.
    pub fn from_category_tag(category: &'static str) -> Option<SpecialCategoryFlag> {
        let kind = category
            .strip_prefix("SpecialCategory(")
            .and_then(|s| s.strip_suffix(')'))?
            .trim();
        Some(SpecialCategoryFlag { kind })
    }
}

/// The trait `#[derive(PersonalData)]` implements on a tagged struct (P-GA-07 / P-107). It exposes
/// the generated registry inventory + the structural `subject_locator` accessor.
///
/// **The generated impl is total + zero-cost:** `personal_data_fields()` returns a `&'static`
/// slice the data-map generator (P-GA-09) walks; `subject_locator(field)` returns the locator
/// expression text a holder's `locate(subject)` uses to find the subject-key column. A struct with
/// NO tagged fields gets an empty slice (it still implements the trait — the derive is uniform).
pub trait HasPersonalData {
    /// The `&'static` registry entries for this struct's tagged fields (gdpr §2.2; P-GA-09 walks
    /// the union over every registered holder). Empty iff the struct carries no `#[personal_data]`
    /// field.
    fn personal_data_fields() -> &'static [PersonalDataField];

    /// The `subject_locator` expression for a named field (gdpr §2.1 — makes `locate(subject)`
    /// structural). Returns `Some(locator)` for a tagged field, `None` for an unknown/untagged
    /// field. A holder's `locate` calls this to read the subject-key column off a row.
    fn subject_locator(field: &str) -> Option<&'static str> {
        Self::personal_data_fields()
            .iter()
            .find(|f| f.field == field)
            .map(|f| f.tags.subject_locator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erasure_key_class_parses_the_crypto_shred_payload() {
        assert_eq!(
            ErasureKeyClass::from_erasure_tag("CryptoShred(subject_dek)"),
            Some(ErasureKeyClass::SubjectDek)
        );
        assert_eq!(
            ErasureKeyClass::from_erasure_tag("CryptoShred(tenant_kek)"),
            Some(ErasureKeyClass::TenantKek)
        );
        assert_eq!(
            ErasureKeyClass::from_erasure_tag("CryptoShred(custom)"),
            Some(ErasureKeyClass::Other("custom"))
        );
        // A non-shredding method names no key class.
        assert_eq!(ErasureKeyClass::from_erasure_tag("Pseudonymise"), None);
        assert_eq!(ErasureKeyClass::from_erasure_tag("PurgeReindex"), None);
        assert_eq!(ErasureKeyClass::from_erasure_tag("CarveOut"), None);
    }

    #[test]
    fn special_category_flag_parses_the_art9_kind() {
        assert_eq!(
            SpecialCategoryFlag::from_category_tag("SpecialCategory(health)"),
            Some(SpecialCategoryFlag { kind: "health" })
        );
        // An ordinary category is not a special-category route.
        assert_eq!(SpecialCategoryFlag::from_category_tag("ContactInfo"), None);
        assert_eq!(SpecialCategoryFlag::from_category_tag("Behavioural"), None);
    }
}
