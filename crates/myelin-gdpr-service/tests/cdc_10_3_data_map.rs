//! # CDC 10.3 — the data-map / RoPA generator (P-GA-09 → P-109)
//!
//! **Contract:** index row 10.3 (`data_map() → Inventory`; `ropa(tenant) → ProcessingActivities` —
//! generated from tags + holders; drives DSR fan-out, breach scoping, RoPA, DPIA). This is the
//! consumer-driven contract test the coverage scanner (P-S21) reads both halves of:
//!
//! - **provider** = the data-map GENERATOR ([`data_map`]) — it walks the compile-time
//!   `#[personal_data]` registry + the runtime auto-registered holder set and EMITS the
//!   machine-readable [`Inventory`]: every tagged field, its owning holder, the five tags, the
//!   subject_locator, the residency region (gdpr §2.2).
//! - **consumer** = the DSR orchestrator's *resolve-scope-from-the-map* step (gdpr §4.1 step 2 — the
//!   P-GA-13 body is the named M1 follow-on). It builds the per-holder erase checklist FROM the
//!   generated map alone — *the map, not a hand-written list, drives fan-out* — so a store the map
//!   never knew about is impossible to drive (and a registered holder absent from the map is a
//!   coverage failure the gate catches BEFORE the fan-out runs).
//!
//! The dated green artifact: the generator emits an inventory over a real-shaped tagged schema and
//! a zero-PII derived holder; the orchestrator stand-in resolves every holder (incl. the zero-PII
//! one) plus every field's erasure mechanism off the map; the coverage gate is green over exactly
//! the registered holders and RED for a registered-but-unmapped holder. If 10.3's generation shape
//! drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr::PersonalData;
use myelin_gdpr_service::{data_map, ropa_for_tenant, tagged_field_count, HolderSchema, Inventory};
use myelin_substrate::{Holder, HolderRegistration, StoreKind};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;

/// A real-shaped principal store schema (H15) carrying two tagged PII fields.
#[derive(PersonalData)]
#[allow(dead_code)]
struct PrincipalRow {
    #[personal_data(
        category = ContactInfo,
        role = PlatformOperational,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    email: String,
    #[personal_data(
        category = Identifier,
        role = PlatformOperational,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "principal_id"
    )]
    handle: String,
    row_version: u64,
}

/// A derived search index keyed on opaque ids — NO directly-tagged PII field (H7).
#[derive(PersonalData)]
#[allow(dead_code)]
struct OpaqueIndexRow {
    doc_id: u64,
}

fn region() -> Region {
    Region("fr-par".into())
}

fn holders() -> Vec<HolderSchema> {
    vec![
        HolderSchema::from_schema::<PrincipalRow>(
            HolderRegistration { kind: StoreKind::Oltp, name: "identity_oltp" },
            Holder::H15Identity,
            region(),
        ),
        HolderSchema::from_schema::<OpaqueIndexRow>(
            HolderRegistration { kind: StoreKind::SearchIndex, name: "search_index" },
            Holder::H7SearchIndex,
            region(),
        ),
    ]
}

/// PROVIDER — the generator emits one entry per tagged field over every registered holder, with
/// every fact, and the holder roster includes the zero-PII holder (gdpr §2.2). The CONSUMER (the DSR
/// orchestrator's resolve-scope step) builds the fan-out checklist FROM the map alone.
#[test]
fn cdc_10_3_generator_emits_the_inventory_and_orchestrator_resolves_the_fan_out_from_it() {
    let holders = holders();

    // ── provider: generate the inventory ──────────────────────────────────────────────────────
    let inv: Inventory = data_map(&holders);
    // 0 fields absent: entry count == tagged-field count (field-for-field).
    assert_eq!(inv.entry_count(), tagged_field_count(&holders));
    assert_eq!(inv.entry_count(), 2);
    // both registered holders are in the roster, incl. the zero-PII index.
    assert_eq!(inv.holder_count(), 2);

    // ── consumer: the DSR orchestrator resolves its per-holder erase checklist FROM the map ─────
    // (gdpr §4.1 step 2 — the P-GA-13 body; here the resolve-scope contract is exercised).
    let mut checklist: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for holder_id in &inv.holders {
        checklist.entry(holder_id.clone()).or_default(); // every holder is driven.
    }
    for e in &inv.entries {
        checklist
            .entry(e.holder_id.clone())
            .or_default()
            .push(format!("{}::{}", e.field_path, e.erasure)); // the per-field mechanism, off the map.
    }

    // every registered holder is a checklist key — 0 holders missed (incl. the zero-PII index).
    assert_eq!(checklist.len(), inv.holder_count());
    assert!(checklist.contains_key("oltp:identity_oltp"));
    assert!(checklist.contains_key("search_index:search_index"));
    // the identity holder's per-field mechanisms resolve off the map (no out-of-band store list).
    let id = &checklist["oltp:identity_oltp"];
    assert_eq!(id.len(), 2);
    assert!(id.contains(&"PrincipalRow.email::CryptoShred(subject_dek)".to_string()));
    assert!(id.contains(&"PrincipalRow.handle::Pseudonymise".to_string()));
}

/// The coverage GATE (the gdpr §2.2 "0 holders absent" property the fan-out depends on): green over
/// exactly the registered holders, RED for a registered-but-unmapped holder. The consumer runs this
/// BEFORE the fan-out so a store the map forgot cannot silently escape erasure.
#[test]
fn cdc_10_3_coverage_gate_is_green_for_mapped_holders_and_red_for_an_unmapped_one() {
    let inv = data_map(&holders());

    // green: the two holders that contributed are the two registered holders.
    let registered_ok = [
        HolderRegistration { kind: StoreKind::Oltp, name: "identity_oltp" },
        HolderRegistration { kind: StoreKind::SearchIndex, name: "search_index" },
    ];
    assert!(inv.coverage_gaps(&registered_ok).is_empty(), "0 holders absent — green");

    // red: a THIRD store was registered (the harness opened it) but never contributed to the map.
    let registered_with_gap = [
        HolderRegistration { kind: StoreKind::Oltp, name: "identity_oltp" },
        HolderRegistration { kind: StoreKind::SearchIndex, name: "search_index" },
        HolderRegistration { kind: StoreKind::Oltp, name: "ci_oltp" },
    ];
    assert_eq!(
        inv.coverage_gaps(&registered_with_gap),
        vec!["oltp:ci_oltp".to_string()],
        "a registered holder absent from the map is a coverage gap (the fan-out would miss it)"
    );
}

/// The generated map is DETERMINISTIC (regenerated every build, diffed in CI — the diff gate
/// P-GA-10 reads the fingerprint): the same holder set in any order yields a byte-identical map; a
/// changed map (a holder added) diffs. The `ropa(tenant)` contract signature projects the inventory.
#[test]
fn cdc_10_3_map_is_deterministic_and_ropa_projects_it() {
    let a = data_map(&holders());
    let mut reversed = holders();
    reversed.reverse();
    let b = data_map(&reversed);
    assert_eq!(a.fingerprint(), b.fingerprint(), "deterministic, order-independent");
    assert_ne!(
        a.fingerprint(),
        data_map(&[]).fingerprint(),
        "an empty map diffs from a populated one"
    );

    // ropa(tenant) — the frozen contract-10.3 projection signature.
    let tenant = TenantId::from_token("acme");
    let activities = ropa_for_tenant(&tenant, &a);
    // PrincipalRow yields two (role, category) activities: ContactInfo + Identifier under
    // PlatformOperational.
    assert_eq!(activities.len(), 2);
    assert!(activities
        .activities
        .iter()
        .any(|act| act.category == "ContactInfo" && act.role == "PlatformOperational"));
    assert!(activities
        .activities
        .iter()
        .any(|act| act.category == "Identifier" && act.role == "PlatformOperational"));
}
