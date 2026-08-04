use myelin_gdpr::PersonalData;
use myelin_gdpr_service::{data_map, ropa_for_tenant, tagged_field_count, HolderSchema, Inventory};
use myelin_substrate::{Holder, HolderRegistration, StoreKind};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;

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
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "identity_oltp",
            },
            Holder::H15Identity,
            region(),
        ),
        HolderSchema::from_schema::<OpaqueIndexRow>(
            HolderRegistration {
                kind: StoreKind::SearchIndex,
                name: "search_index",
            },
            Holder::H7SearchIndex,
            region(),
        ),
    ]
}

#[test]
fn cdc_10_3_generator_emits_the_inventory_and_orchestrator_resolves_the_fan_out_from_it() {
    let holders = holders();

    let inv: Inventory = data_map(&holders);
    assert_eq!(inv.entry_count(), tagged_field_count(&holders));
    assert_eq!(inv.entry_count(), 2);
    assert_eq!(inv.holder_count(), 2);

    let mut checklist: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for holder_id in &inv.holders {
        checklist.entry(holder_id.clone()).or_default();
    }
    for e in &inv.entries {
        checklist
            .entry(e.holder_id.clone())
            .or_default()
            .push(format!("{}::{}", e.field_path, e.erasure));
    }

    assert_eq!(checklist.len(), inv.holder_count());
    assert!(checklist.contains_key("oltp:identity_oltp"));
    assert!(checklist.contains_key("search_index:search_index"));
    let id = &checklist["oltp:identity_oltp"];
    assert_eq!(id.len(), 2);
    assert!(id.contains(&"PrincipalRow.email::CryptoShred(subject_dek)".to_string()));
    assert!(id.contains(&"PrincipalRow.handle::Pseudonymise".to_string()));
}

#[test]
fn cdc_10_3_coverage_gate_is_green_for_mapped_holders_and_red_for_an_unmapped_one() {
    let inv = data_map(&holders());

    let registered_ok = [
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: "identity_oltp",
        },
        HolderRegistration {
            kind: StoreKind::SearchIndex,
            name: "search_index",
        },
    ];
    assert!(
        inv.coverage_gaps(&registered_ok).is_empty(),
        "0 holders absent - green"
    );

    let registered_with_gap = [
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: "identity_oltp",
        },
        HolderRegistration {
            kind: StoreKind::SearchIndex,
            name: "search_index",
        },
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: "ci_oltp",
        },
    ];
    assert_eq!(
        inv.coverage_gaps(&registered_with_gap),
        vec!["oltp:ci_oltp".to_string()],
        "a registered holder absent from the map is a coverage gap (the fan-out would miss it)"
    );
}

#[test]
fn cdc_10_3_map_is_deterministic_and_ropa_projects_it() {
    let a = data_map(&holders());
    let mut reversed = holders();
    reversed.reverse();
    let b = data_map(&reversed);
    assert_eq!(
        a.fingerprint(),
        b.fingerprint(),
        "deterministic, order-independent"
    );
    assert_ne!(
        a.fingerprint(),
        data_map(&[]).fingerprint(),
        "an empty map diffs from a populated one"
    );

    let tenant = TenantId::from_token("acme");
    let activities = ropa_for_tenant(&tenant, &a);
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
