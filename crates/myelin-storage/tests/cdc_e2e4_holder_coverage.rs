use myelin_gdpr_service::full_fanout::Holder;
use myelin_gdpr_service::orchestration::holder_ids;
use myelin_storage::{holder_ids_not_covered, HolderClass};

#[test]
fn cdc_storage_catalogue_covers_the_orchestrator_storage_holder_ids() {
    let orchestrator_storage_holders = [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ];

    let not_covered = holder_ids_not_covered(&orchestrator_storage_holders);
    assert!(
        not_covered.is_empty(),
        "the storage H1–H18 catalogue MUST cover every storage-owned orchestrator holder id; \
         not covered: {not_covered:?}"
    );
}

#[test]
fn cdc_each_orchestrator_holder_id_resolves_to_one_storage_holder() {
    for id in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ] {
        let matches: Vec<HolderClass> = HolderClass::ALL
            .iter()
            .copied()
            .filter(|h| h.holder_id() == id)
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "the orchestrator holder id {id:?} resolves to EXACTLY one storage HolderClass (got {matches:?})"
        );
    }
}

#[test]
fn cdc_catalogue_is_the_full_h1_h18_set() {
    assert_eq!(
        HolderClass::ALL.len(),
        18,
        "the E2E-4 holder catalogue is the exhaustive H1–H18 set"
    );
}

#[test]
fn cdc_both_catalogues_are_18_holders() {
    assert_eq!(
        Holder::ALL.len(),
        18,
        "the gdpr §3.2 catalogue is exhaustive (18)"
    );
    assert_eq!(
        HolderClass::ALL.len(),
        18,
        "the storage D-S5 catalogue is exhaustive (18)"
    );
}

#[test]
fn cdc_storage_catalogue_covers_the_gdpr_storage_owned_holder_ids() {
    let gdpr_storage_owned: Vec<&str> = [
        Holder::Identity,
        Holder::ObjectStore,
        Holder::EventBus,
        Holder::CachesAndCdn,
        Holder::Backups,
        Holder::AuthzTuples,
        Holder::SearchIndex,
        Holder::AgentMemory,
        Holder::ReferenceGraph,
        Holder::NotificationHistory,
        Holder::AuditCarveOut,
    ]
    .iter()
    .map(|h| h.holder_id())
    .collect();

    let not_covered = holder_ids_not_covered(&gdpr_storage_owned);
    assert!(
        not_covered.is_empty(),
        "the storage D-S5 catalogue must cover every storage-owned gdpr §3.2 holder id; missing: {not_covered:?}"
    );
}

#[test]
fn cdc_each_gdpr_storage_owned_id_resolves_to_one_storage_class() {
    for h in [
        Holder::Identity,
        Holder::ObjectStore,
        Holder::EventBus,
        Holder::CachesAndCdn,
        Holder::Backups,
        Holder::AuthzTuples,
        Holder::SearchIndex,
        Holder::AgentMemory,
        Holder::ReferenceGraph,
        Holder::NotificationHistory,
        Holder::AuditCarveOut,
    ] {
        let id = h.holder_id();
        let matches: Vec<HolderClass> = HolderClass::ALL
            .iter()
            .copied()
            .filter(|c| c.holder_id() == id)
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "the gdpr holder {} ({id:?}) resolves to EXACTLY one storage HolderClass (got {matches:?})",
            h.h_label()
        );
    }
}

#[test]
fn cdc_orchestrator_holder_ids_resolve_in_the_gdpr_catalogue() {
    for id in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ] {
        assert!(
            Holder::from_id(id).is_some(),
            "the orchestrator holder id {id:?} resolves to a gdpr §3.2 H-class"
        );
    }
}
