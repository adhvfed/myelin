//! # CDC 10.2 (worklog leg) — the `data_role_default = Restricted` worklog tag (P-GA-31 → P-334)
//!
//! **Contract:** index row 10.2 (the worklog/Behavioural tags — the OQ-H extension, gdpr §2.4). The
//! five-tag classify-derive froze at P-GA-02/P-GA-07; THIS leg adds the OQ-H `data_role_default`
//! worklog tag and makes it STRUCTURAL (captured into the generated registry, read off the data map).
//! This is the consumer-driven contract test the coverage scanner reads both halves of:
//!
//! - **provider** = an Issues-shaped schema struct that DERIVES `#[derive(PersonalData)]` and tags a
//!   worklog field `category = Behavioural, role = TenantContent, basis = TBD_LEGAL,
//!   erasure = CryptoShred(subject_dek), data_role_default = Restricted` (the OQ-H §2.4 posture). The
//!   derive EMITS the `data_role_default` into the registry entry alongside the five tags.
//! - **consumer** = a worklog-analytics-gate stand-in (the shape the real `WorklogAnalyticsGate` in
//!   `myelin-gdpr-service` takes): it WALKS `personal_data_fields()` and reads `data_role_default`
//!   off each entry to decide the **default-DENY** for a restricted-by-default field (excluded from
//!   cross-individual analytics by default) — the map, not a hand-written "is this worklog?" list,
//!   drives the restriction.
//!
//! The dated green artifact: the consumer reconstructs, from the derive's emitted registry alone,
//! the restricted-by-default fact for the worklog field — and applies the OQ-H default-deny. If the
//! 10.2 worklog-tag registry shape drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr::{DataRoleDefault, HasPersonalData, PersonalData, PersonalDataField};

// ── The PROVIDER side (10.2 worklog leg): a schema row tagging a worklog field restricted-by-default ──

/// A provider Issues-shaped row deriving the classify-derive — the same shape the live
/// `myelin_issues::schema::Issue` carries. The worklog field carries the OQ-H §2.4 tags incl.
/// `data_role_default = Restricted`; the ordinary content field does NOT (the default-class).
#[derive(PersonalData)]
#[allow(dead_code)]
struct IssueRow {
    /// the OQ-H worklog field — Behavioural, restricted-by-default, per-subject DEK crypto-shred.
    #[personal_data(
        category = Behavioural,
        role = TenantContent,
        basis = TBD_LEGAL,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by_pseudonym",
        data_role_default = Restricted
    )]
    worklog_seconds: i64,
    /// an ordinary free-text content field — NOT restricted-by-default (the default-class).
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by_pseudonym"
    )]
    title: String,
    /// a non-PII column — no tag, no entry.
    row_version: u64,
}

// ── The CONSUMER side (10.2 worklog leg): the worklog-analytics-gate stand-in ──────────────────

/// The shape the real `myelin_gdpr_service::worklog::WorklogAnalyticsGate` takes: it reads
/// `data_role_default` off the registry entry and applies the OQ-H **default-DENY** — a
/// restricted-by-default field is excluded from cross-individual analytics UNLESS the subject opted
/// in; an ordinary field is allowed. The consumer NEVER hand-writes which fields are worklog — the
/// map drives it.
fn cross_individual_allowed(field: &PersonalDataField, subject_opted_in: bool) -> bool {
    match field.data_role_default() {
        DataRoleDefault::Restricted => subject_opted_in,
        DataRoleDefault::Default => true,
    }
}

fn worklog_field() -> &'static PersonalDataField {
    IssueRow::personal_data_fields()
        .iter()
        .find(|f| f.field == "worklog_seconds")
        .expect("worklog_seconds is tagged")
}

fn title_field() -> &'static PersonalDataField {
    IssueRow::personal_data_fields()
        .iter()
        .find(|f| f.field == "title")
        .expect("title is tagged")
}

/// **Provider emits the worklog tag; consumer reads the restricted-by-default fact off the map.**
/// The derive captures `data_role_default = Restricted` into the registry entry; the consumer reads
/// it back structurally (not by field name) — the CDC seam for the OQ-H worklog tag.
#[test]
fn provider_emits_and_consumer_reads_the_restricted_by_default_worklog_tag() {
    let w = worklog_field();
    // The provider emitted the tag into the registry.
    assert_eq!(w.tags.data_role_default, "Restricted");
    assert_eq!(w.data_role_default(), DataRoleDefault::Restricted);
    assert!(w.is_restricted_by_default());
    assert!(w.is_behavioural(), "the worklog field is Behavioural");

    // The ordinary field defaults to "Default" (the additive extension — an absent tag is no
    // restriction). This is the wire-shape-stability half: an untagged-for-default field still has a
    // valid registry entry.
    let t = title_field();
    assert_eq!(t.tags.data_role_default, "Default");
    assert_eq!(t.data_role_default(), DataRoleDefault::Default);
    assert!(!t.is_restricted_by_default());
}

/// **The consumer applies the OQ-H default-DENY off the map (the §2.4 exclusion).** A
/// restricted-by-default worklog field is excluded from cross-individual analytics by default;
/// allowed only with an explicit per-subject opt-in. An ordinary field is allowed.
#[test]
fn consumer_applies_the_default_deny_for_a_restricted_by_default_field() {
    let w = worklog_field();
    // Restricted-by-default ⇒ DENIED by default, ALLOWED with an explicit opt-in.
    assert!(
        !cross_individual_allowed(w, false),
        "worklog denied by default (§2.4)"
    );
    assert!(
        cross_individual_allowed(w, true),
        "explicit opt-in lifts the deny"
    );

    // Ordinary ⇒ allowed regardless (the OQ-H gate has no default-deny for it).
    let t = title_field();
    assert!(cross_individual_allowed(t, false));
    assert!(cross_individual_allowed(t, true));
}

/// **The registry entry serialises with the worklog tag, and an old entry without it deserialises to
/// `"Default"` (the additive-extension wire-shape stability).** `PersonalDataField` borrows its tag
/// strings `&'static` (the derive emits `&'static` consts), so a deserialise yields `&'static str`
/// only from a `'static`-input deserialiser — we therefore drive the deserialise legs off `'static`
/// string literals (the registry's compiled form), which is exactly the wire form the data-map
/// generator commits.
#[test]
fn the_worklog_registry_entry_round_trips_with_the_data_role_default_tag() {
    let w = worklog_field();
    let json = serde_json::to_string(w).expect("serialize");
    assert!(
        json.contains("\"data_role_default\":\"Restricted\""),
        "the worklog tag serialises into the registry entry"
    );

    // Deserialise off a `'static` literal (the `&'static str` field constraint): the full entry
    // round-trips, recovering the restricted-by-default fact.
    const NEW_ENTRY: &str = r#"{"owning_struct":"S","field":"f","tags":{"category":"Behavioural","role":"TenantContent","basis":"TBD_LEGAL","retention":"TenantPolicy","erasure":"CryptoShred(subject_dek)","subject_locator":"id","data_role_default":"Restricted"}}"#;
    let back: PersonalDataField = serde_json::from_str(NEW_ENTRY).expect("deserialize");
    assert_eq!(back.data_role_default(), DataRoleDefault::Restricted);

    // An OLD entry serialised before this tag existed (no `data_role_default` key) deserialises to
    // "Default" — the additive-extension wire-shape stability (`#[serde(default)]`).
    const LEGACY: &str = r#"{"owning_struct":"S","field":"f","tags":{"category":"Content","role":"TenantContent","basis":"Contract","retention":"TenantPolicy","erasure":"CryptoShred(subject_dek)","subject_locator":"id"}}"#;
    let legacy_field: PersonalDataField = serde_json::from_str(LEGACY).expect("legacy deserialize");
    assert_eq!(
        legacy_field.data_role_default(),
        DataRoleDefault::Default,
        "a pre-P-GA-31 registry entry round-trips to Default (additive extension)"
    );
}
