//! # `myelin-gdpr-macros` — the proc-macro half of the GDPR classify-derive (contract 10.2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §2.1 (the schema-level
//! `#[personal_data(category, role, basis, retention, erasure, subject_locator)]` classification
//! — the five tags answer the five questions every rights pipeline asks; `subject_locator` makes
//! `locate(subject)` structural) + §2.2 (the GENERATED data map the derive emits into). Contract-
//! index row **10.2** ("`#[personal_data(...)]` classify-derive — the `no-untagged-personal-data`
//! lint").
//!
//! ## P-GA-07 / P-107 — the macro BODY (was a no-op floor under P-GA-02 / P-050)
//! The `#[derive(PersonalData)]` derive now has a real body. For a struct whose fields carry the
//! `#[personal_data(...)]` helper it generates:
//!
//! 1. **An impl of `::myelin_gdpr::HasPersonalData`** exposing
//!    `personal_data_fields() -> &'static [::myelin_gdpr::PersonalDataField]` — one **generated
//!    registry entry** per tagged field (owning struct, field path, the five tag values + the
//!    `subject_locator`, all captured as rendered token text). This is the **compile-time-collected
//!    inventory the data-map generator (P-GA-09) walks** — the map, not a hand-written list, drives
//!    erasure/RoPA/breach-scoping.
//! 2. The **structural `subject_locator`** accessor (the default trait method over the slice) — a
//!    holder's `locate(subject)` reads the subject-key column off a row through it (gdpr §2.1:
//!    `subject_locator` makes `locate` structural).
//! 3. **A compile-time rejection of an untagged PII field** — a field whose NAME is a PII
//!    fingerprint (`email`, `display_name`, `phone`, …) that carries NO `#[personal_data(...)]` tag
//!    is a hard `compile_error!`. This is the **type-system form of the `no-untagged-personal-data`
//!    lint** — the floor the lint named landing in P-107 (`myelin-lints` §`scan_no_untagged_
//!    personal_data`): the M0 source-scanner forces the tag for any struct ANYWHERE; this makes a
//!    struct that DERIVES `PersonalData` additionally unable to COMPILE with an untagged PII field.
//!    The two are belt-and-braces (a schema author can forget the derive — the scanner still
//!    fires; a schema author who derives it cannot leave a PII field untagged — the macro fires).
//!
//! ## The captured-text reconciliation (EI-01 §1, code-wins-over-docs)
//! The helper tags use **bare-identifier enum variants** (`category = ContactInfo`,
//! `erasure = CryptoShred(subject_dek)`, `retention = Fixed(90d)`) — payloads like `subject_dek` /
//! `ops_lia` / `90d` are bare tokens, not resolvable Rust consts. The macro runs BEFORE
//! type-checking, so it cannot evaluate them. It therefore captures each tag's **rendered token
//! text** into the registry entry (a `&'static str`); the typed five-tag enums
//! (`myelin_gdpr::DataCategory` et al.) stay the surface a holder/orchestrator pattern-matches on,
//! and P-GA-09 re-parses the strings into them. The derive stays hermetic (no path resolution, no
//! const-eval) while emitting a COMPLETE entry. gdpr §2.1 shows only the field attribute; the
//! struct-level `#[derive(PersonalData)]` it is an inert helper under is the form that COMPILES (a
//! standalone field attribute macro does not exist in Rust) — this crate carries that reconciled
//! shape, and the contract-index "classify derive" name (10.2) is honoured literally.
//!
//! ## Floors named (what is STILL deferred) — VISION §3
//! - The **typed re-parse** of the captured tag text into `DataCategory`/`LawfulBasis`/… and the
//!   `Inventory`/`data_map()` walk over every holder is the **data-map generator (P-GA-09)** — this
//!   prompt ships the per-struct registry EMISSION + the CDC's consumer stub; the generator that
//!   UNIONS them is P-GA-09.
//! - The **`SpecialCategory` → DPIA router** is **P-GA-08** (it consumes
//!   `PersonalDataField::is_special_category`, emitted here).
//! ## P-GA-31 / P-334 — the worklog `Behavioural`/restricted-by-default extension (OQ-H) is STRUCTURAL
//! The OQ-H worklog/productivity/estimate posture (gdpr §2.4) tags a field
//! `data_role_default = Restricted` (restricted-by-default in cross-individual processing). That key
//! is now **captured into the registry entry** (`PersonalDataTags::data_role_default`) — the
//! restricted-by-default fact is read OFF the data map (`PersonalDataField::is_restricted_by_default`),
//! never inferred from the category. It is OPTIONAL (an ordinary field omits it → `"Default"`), so the
//! extension is additive (no existing tag changes). Any OTHER unknown extra key is still accepted +
//! ignored (forward-compat for a future tag without a macro change).

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

/// The **`#[derive(PersonalData)]`** classify-derive (contract 10.2; gdpr §2.1 / §2.2). Apply it to
/// a struct whose fields carry personal data; tag each PII field with the `#[personal_data(...)]`
/// helper attribute it declares.
///
/// **P-GA-07 / P-107 — the BODY.** It emits an impl of `::myelin_gdpr::HasPersonalData` carrying a
/// `&'static [PersonalDataField]` registry entry per tagged field (the compile-time inventory
/// P-GA-09 walks) + the structural `subject_locator` accessor, and it **rejects an untagged PII
/// field at compile time** (the type-system form of the `no-untagged-personal-data` lint). See the
/// crate doc for the captured-text reconciliation + the named floors.
#[proc_macro_derive(PersonalData, attributes(personal_data))]
pub fn derive_personal_data(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match expand(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Field-name fingerprints that carry PII — the SAME set the `no-untagged-personal-data` source
/// scanner (`myelin-lints`) uses, kept in sync by convention (both enforce contract 1.6 / gdpr
/// §2.1). A field with one of these names that carries NO `#[personal_data(...)]` helper is the
/// un-erasable-subject bug class; the derive refuses to expand it.
const PII_FIELDS: &[&str] = &[
    "email",
    "name",
    "phone",
    "address",
    "ip_addr",
    "ip_address",
    "full_name",
    "given_name",
    "family_name",
    "display_name",
    "dob",
    "birth",
    "ssn",
    "passport",
    "body",
    "message_body",
    "comment_text",
];

fn expand(input: DeriveInput) -> syn::Result<TokenStream2> {
    let struct_name = &input.ident;
    let struct_name_str = struct_name.to_string();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            // A tuple/unit struct carries no named PII column to classify — it gets an empty
            // registry (the derive is uniform: every PersonalData type implements the trait).
            _ => {
                return Ok(empty_impl(
                    struct_name,
                    &impl_generics,
                    &ty_generics,
                    where_clause,
                ));
            }
        },
        // An enum/union is not a schema row; the derive only classifies struct fields.
        _ => {
            return Err(syn::Error::new_spanned(
                struct_name,
                "#[derive(PersonalData)] applies to a struct with named fields (a schema row); an \
                 enum/union has no field to classify",
            ));
        }
    };

    let mut entries: Vec<TokenStream2> = Vec::new();
    for field in fields {
        let field_ident = field
            .ident
            .as_ref()
            .expect("named field has an ident (Fields::Named)");
        let field_name = field_ident.to_string();

        let tag_attr = field
            .attrs
            .iter()
            .find(|a| a.path().is_ident("personal_data"));

        match tag_attr {
            Some(attr) => {
                let tags = parse_personal_data_tags(attr)?;
                entries.push(registry_entry(&struct_name_str, &field_name, &tags));
            }
            None => {
                // The type-system form of the no-untagged-personal-data lint: a PII-named field
                // with no tag is a hard compile error (P-107, the floor the lint named).
                if is_pii_field(&field_name) {
                    return Err(syn::Error::new_spanned(
                        field_ident,
                        format!(
                            "PII field `{field_name}` is not `#[personal_data(...)]`-tagged — a \
                             personal-data field deriving PersonalData MUST carry the five-tag \
                             classification (category/role/basis/retention/erasure/subject_locator; \
                             gdpr §2.1). An untagged PII column is the un-erasable / un-mapped \
                             subject bug class (ADR-12); tag it or it escapes the crypto-shred + \
                             RoPA fan-out."
                        ),
                    ));
                }
                // A non-PII field with no tag is fine — it carries no personal data.
            }
        }
    }

    let n = entries.len();
    Ok(quote! {
        impl #impl_generics ::myelin_gdpr::HasPersonalData for #struct_name #ty_generics #where_clause {
            fn personal_data_fields() -> &'static [::myelin_gdpr::PersonalDataField] {
                // The generated registry — one entry per tagged field, all `&'static`. This is the
                // compile-time inventory the data-map generator (P-GA-09) walks.
                const FIELDS: [::myelin_gdpr::PersonalDataField; #n] = [ #( #entries ),* ];
                &FIELDS
            }
        }
    })
}

/// The empty-registry impl for a struct with no named PII fields (tuple/unit struct, or a named
/// struct that happens to carry no tag) — the derive stays uniform.
fn empty_impl(
    struct_name: &syn::Ident,
    impl_generics: &syn::ImplGenerics<'_>,
    ty_generics: &syn::TypeGenerics<'_>,
    where_clause: Option<&syn::WhereClause>,
) -> TokenStream2 {
    quote! {
        impl #impl_generics ::myelin_gdpr::HasPersonalData for #struct_name #ty_generics #where_clause {
            fn personal_data_fields() -> &'static [::myelin_gdpr::PersonalDataField] {
                &[]
            }
        }
    }
}

/// Whether a field name is a PII fingerprint (matched as the whole identifier).
fn is_pii_field(name: &str) -> bool {
    PII_FIELDS.contains(&name)
}

/// The five captured tag texts + the subject locator + the OQ-H `data_role_default`, all as
/// `String` (rendered token text). `data_role_default` is the worklog/productivity extension
/// (gdpr §2.4, P-GA-31): OPTIONAL (defaults to `"Default"`) so an ordinary field need not carry it.
struct ParsedTags {
    category: String,
    role: String,
    basis: String,
    retention: String,
    erasure: String,
    subject_locator: String,
    /// `data_role_default` — `"Restricted"` for a restricted-by-default field (the OQ-H worklog
    /// posture) or `"Default"` when the tag is absent (the additive extension; P-GA-31).
    data_role_default: String,
}

/// Parse a `#[personal_data(category = .., role = .., basis = .., retention = .., erasure = ..,
/// subject_locator = "..")]` helper into its six captured texts. Each value is captured as RENDERED
/// TOKEN TEXT (see the crate doc): a bare ident/variant (`ContactInfo`), a call form
/// (`CryptoShred(subject_dek)` / `Fixed(90d)`), or a string literal (`subject_locator = "id"`).
///
/// Tolerant by design — it does NOT type-check the variant names (that is the job of P-GA-09's
/// typed re-parse and the five-tag enums); it only requires the five classification keys and the
/// locator be PRESENT, so an incomplete tag is a loud compile error (a half-classified field is the
/// bug class the registry must never carry). The OQ-H **`data_role_default`** key (gdpr §2.4,
/// P-GA-31) is OPTIONAL and now STRUCTURAL — it is captured into the registry (the worklog
/// restricted-by-default posture is read off the map, not inferred); an absent tag defaults to
/// `"Default"`. Any OTHER unknown extra key is still accepted + ignored (forward-compat).
fn parse_personal_data_tags(attr: &syn::Attribute) -> syn::Result<ParsedTags> {
    let mut category: Option<String> = None;
    let mut role: Option<String> = None;
    let mut basis: Option<String> = None;
    let mut retention: Option<String> = None;
    let mut erasure: Option<String> = None;
    let mut subject_locator: Option<String> = None;
    let mut data_role_default: Option<String> = None;

    // `parse_nested_meta` walks the `key = value` (or `key(value)`) list inside the parentheses.
    attr.parse_nested_meta(|meta| {
        let key = meta
            .path
            .get_ident()
            .map(|i| i.to_string())
            .ok_or_else(|| meta.error("each #[personal_data(...)] tag is a `key = value`"))?;

        // Capture the value as rendered token text — but ONLY this value's tokens, not the rest of
        // the helper list. `meta.value()` returns the stream positioned after `=`; a bare
        // `value.parse::<TokenStream2>()` would greedily consume the FOLLOWING `, role = ...` too.
        // We therefore consume value tokens up to the next top-level comma (or end), one
        // token-tree at a time. The value is a string literal (the locator → its inner value), a
        // bare variant (`ContactInfo`), or a call form (`CryptoShred(subject_dek)` / `Fixed(90d)` —
        // NOT valid `syn::Expr` syntax, so we never parse it as one; we capture its TEXT).
        let value = meta.value()?;
        let text = if value.peek(syn::LitStr) {
            let s: syn::LitStr = value.parse()?;
            s.value()
        } else {
            let mut collected = TokenStream2::new();
            while !value.is_empty() && !value.peek(syn::Token![,]) {
                let tt: proc_macro2::TokenTree = value.parse()?;
                collected.extend(std::iter::once(tt));
            }
            collected
                .to_string()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        };

        match key.as_str() {
            "category" => category = Some(text),
            "role" => role = Some(text),
            "basis" => basis = Some(text),
            "retention" => retention = Some(text),
            "erasure" => erasure = Some(text),
            "subject_locator" => subject_locator = Some(text),
            // The OQ-H worklog extension (gdpr §2.4, P-GA-31) — now STRUCTURAL: captured into the
            // registry so the restricted-by-default posture is read off the data map, not inferred.
            "data_role_default" => data_role_default = Some(text),
            // Any OTHER unknown extra key is accepted + ignored (forward-compat: a future tag
            // compiles without a macro change).
            _ => {}
        }
        Ok(())
    })?;

    let require = |opt: Option<String>, key: &str| -> syn::Result<String> {
        opt.ok_or_else(|| {
            syn::Error::new_spanned(
                attr,
                format!(
                    "#[personal_data(...)] is missing the `{key}` tag — the five-tag classification \
                     (category/role/basis/retention/erasure) + subject_locator are ALL required \
                     (gdpr §2.1); a half-classified field is the un-mapped-subject bug class"
                ),
            )
        })
    };

    Ok(ParsedTags {
        category: require(category, "category")?,
        role: require(role, "role")?,
        basis: require(basis, "basis")?,
        retention: require(retention, "retention")?,
        erasure: require(erasure, "erasure")?,
        subject_locator: require(subject_locator, "subject_locator")?,
        // OPTIONAL — defaults to `"Default"` (no restriction) when the tag is absent (gdpr §2.4;
        // the additive OQ-H extension, P-GA-31). A non-worklog field need not carry it.
        data_role_default: data_role_default.unwrap_or_else(|| "Default".to_string()),
    })
}

/// Emit one `::myelin_gdpr::PersonalDataField` const-expression for a tagged field.
fn registry_entry(struct_name: &str, field_name: &str, tags: &ParsedTags) -> TokenStream2 {
    let category = &tags.category;
    let role = &tags.role;
    let basis = &tags.basis;
    let retention = &tags.retention;
    let erasure = &tags.erasure;
    let subject_locator = &tags.subject_locator;
    let data_role_default = &tags.data_role_default;
    quote! {
        ::myelin_gdpr::PersonalDataField {
            owning_struct: #struct_name,
            field: #field_name,
            tags: ::myelin_gdpr::PersonalDataTags {
                category: #category,
                role: #role,
                basis: #basis,
                retention: #retention,
                erasure: #erasure,
                subject_locator: #subject_locator,
                data_role_default: #data_role_default,
            },
        }
    }
}
