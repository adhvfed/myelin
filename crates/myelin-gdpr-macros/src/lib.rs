//! # `myelin-gdpr-macros` — the proc-macro half of the GDPR classify-derive (contract 10.2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §2.1 (the schema-level
//! `#[personal_data(category, role, basis, retention, erasure, subject_locator)]` classification
//! — the five tags answer the five questions every rights pipeline asks; `subject_locator` makes
//! `locate(subject)` structural). Contract-index row **10.2** ("`#[personal_data(...)]` classify
//! derive — the `no-untagged-personal-data` lint").
//!
//! ## What this crate freezes NOW (P-GA-02 / P-050) — the DERIVE + helper ATTRIBUTE NAMES only
//! This crate exports the **`#[derive(PersonalData)]`** classify-derive + its
//! **`#[personal_data(...)]` field helper attribute** so that, the moment this prompt lands, every
//! schema owner across the workspace can write
//!
//! ```ignore
//! #[derive(PersonalData)]
//! struct PrincipalRow {
//!     id: PrincipalId,
//!     #[personal_data(
//!         category   = ContactInfo,
//!         role       = TenantContent,
//!         basis      = Contract,
//!         retention  = TenantPolicy,
//!         erasure    = CryptoShred(subject_dek),
//!         subject_locator = "principal_id",
//!     )]
//!     email: EncryptedField<Email>,
//! }
//! ```
//!
//! and **compile** — a `#[personal_data(...)]` field attribute is only legal as the INERT HELPER
//! of a struct-level derive (Rust attribute macros cannot decorate individual fields; only derive
//! helper attributes can). Without this proc-macro the helper is a hard compile error (`cannot
//! find attribute`). Freezing the derive + helper names now (alongside the five-tag enum NAMES in
//! `myelin-gdpr`, P-GA-02) is what lets the M1 stores compile against the classification surface
//! before the macro BODY exists. The five-tag enum TYPES the helper arguments reference live in
//! `myelin-gdpr` (this crate must export only macros).
//!
//! **Reconciliation (EI-01 §1, code-wins-over-docs).** gdpr §2.1 shows only the FIELD attribute
//! `#[personal_data(...)]`; it elides the struct-level `#[derive(PersonalData)]` the helper is
//! inert under. The field-grain requirement makes the derive-with-helper form the one that
//! COMPILES — a standalone field attribute macro does not exist in Rust. This crate carries that
//! reconciled shape; the contract-index's "classify derive" name (10.2) is honoured literally
//! (`PersonalData` IS a derive).
//!
//! ## Floor named (the body) → P-GA-04 (global P-055) / P-GA-07 (global P-107) — VISION §3
//! **On THIS floor the attribute is a deliberate NO-OP**: it parses nothing and emits the
//! annotated item back unchanged (it does not even validate the tag keys — that is the lint's
//! job, P-GA-03, and the macro's job once it has a body). It therefore:
//! - does NOT yet emit the **generated registry entry** (field path, owning store, the five tag
//!   values, the `subject_locator` expression) into the compile-time inventory the data-map
//!   generator (P-GA-09) walks — that emission is the **macro BODY**, the M1 deliverable
//!   **P-GA-04** (the auto-registration hook) / **P-GA-07** (the classify-derive macro body +
//!   the five-tag enum parsing). Its CDC pair lands there (contract-coverage 10.2 → `landing =
//!   "P-107"`).
//! - does NOT yet validate the five tag keys or the variant payloads — `category` / `role` /
//!   `basis` / `retention` / `erasure` / `subject_locator` parsing is the P-GA-07 body.
//!
//! Because it is a pure pass-through, an arbitrary (even malformed) `#[personal_data(...)]`
//! argument list compiles today; the M1 body tightens that into a validated, registry-emitting
//! derive WITHOUT changing the attribute NAME frozen here (the consumers never re-write their
//! tags). The `no-untagged-personal-data` lint (P-GA-03) is the independent ratchet that forces a
//! field to CARRY the tag at all; this crate makes carrying it COMPILE.

use proc_macro::TokenStream;

/// The **`#[derive(PersonalData)]`** classify-derive (contract 10.2; gdpr §2.1). Apply it to a
/// struct whose fields carry personal data; tag each PII field with the `#[personal_data(...)]`
/// helper attribute it declares.
///
/// **This is the FROZEN DERIVE NAME at a NO-OP floor (P-GA-02 / P-050).** It emits NOTHING (an
/// empty `TokenStream`) — it neither reads the `#[personal_data(...)]` helper arguments nor emits
/// the compile-time registry entry yet. Declaring `attributes(personal_data)` is what makes the
/// field helper a legal INERT attribute (so a tagged field compiles). The body that walks the
/// five tags and emits the registry entry the data map (P-GA-09) walks is the M1 deliverable
/// **P-GA-04 / P-GA-07** (see the crate doc comment). Freezing the names now lets every M1 store
/// apply the tags and compile against the classification surface before that body exists.
///
/// The contract-index (10.2) names this a "classify derive"; `PersonalData` IS a derive, and the
/// `#[personal_data(...)]` field helper is the field-grain annotation §2.1 shows — the only Rust
/// form in which a per-field tag compiles (a standalone field attribute macro does not exist).
#[proc_macro_derive(PersonalData, attributes(personal_data))]
pub fn derive_personal_data(_item: TokenStream) -> TokenStream {
    // NO-OP floor: a derive emits ADDITIONAL items; here it emits none, leaving the annotated
    // struct (and its inert `#[personal_data(...)]` helpers) exactly as written. The
    // registry-emitting body is P-GA-04 / P-GA-07.
    TokenStream::new()
}
