use myelin_gdpr::{dpia_markers_of, DpiaMarker};
use myelin_gdpr::{HasPersonalData, PersonalDataField};
use myelin_substrate::{Holder, HolderRegistration};
use myelin_tenancy::Region;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const DATA_MAP_ENTRY_COUNT: &str = "gdpr.data_map.entry_count";

pub const DATA_MAP_HOLDER_COUNT: &str = "gdpr.data_map.holder_count";

#[derive(Clone, Debug)]
pub struct HolderSchema {
    pub registration: HolderRegistration,
    pub holder: Holder,
    pub region: Region,
    pub fields: &'static [PersonalDataField],
}

impl HolderSchema {
    pub fn from_schema<T: HasPersonalData>(
        registration: HolderRegistration,
        holder: Holder,
        region: Region,
    ) -> HolderSchema {
        HolderSchema {
            registration,
            holder,
            region,
            fields: T::personal_data_fields(),
        }
    }

    pub fn holder_id(&self) -> String {
        self.registration.holder_id()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InventoryEntry {
    pub field_path: String,
    pub holder_id: String,
    pub holder: String,
    pub region: String,
    pub category: String,
    pub role: String,
    pub basis: String,
    pub retention: String,
    pub erasure: String,
    pub subject_locator: String,
}

impl InventoryEntry {
    fn from_field(schema: &HolderSchema, field: &PersonalDataField) -> InventoryEntry {
        InventoryEntry {
            field_path: format!("{}.{}", field.owning_struct, field.field),
            holder_id: schema.holder_id(),
            holder: schema.holder.tag().to_string(),
            region: schema.region.0.clone(),
            category: field.tags.category.to_string(),
            role: field.tags.role.to_string(),
            basis: field.tags.basis.to_string(),
            retention: field.tags.retention.to_string(),
            erasure: field.tags.erasure.to_string(),
            subject_locator: field.tags.subject_locator.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Inventory {
    pub entries: Vec<InventoryEntry>,
    pub holders: BTreeSet<String>,
    pub dpia_markers: BTreeSet<DpiaMarker>,
}

impl Inventory {
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn holder_count(&self) -> usize {
        self.holders.len()
    }

    pub fn coverage_gaps(&self, registered: &[HolderRegistration]) -> Vec<String> {
        let mut gaps: Vec<String> = registered
            .iter()
            .map(HolderRegistration::holder_id)
            .filter(|id| !self.holders.contains(id))
            .collect();
        gaps.sort();
        gaps.dedup();
        gaps
    }

    pub fn fingerprint(&self) -> String {
        let canonical = serde_json::to_vec(self).expect("Inventory is serialisable");
        let digest = blake3::hash(&canonical);
        format!("blake3:{}", hex::encode(digest.as_bytes()))
    }
}

pub fn tagged_field_count(holders: &[HolderSchema]) -> usize {
    holders.iter().map(|h| h.fields.len()).sum()
}

pub fn data_map(holders: &[HolderSchema]) -> Inventory {
    let mut entries: Vec<InventoryEntry> = Vec::new();
    let mut roster: BTreeSet<String> = BTreeSet::new();
    let mut markers: BTreeSet<DpiaMarker> = BTreeSet::new();

    for schema in holders {
        roster.insert(schema.holder_id());
        for field in schema.fields {
            entries.push(InventoryEntry::from_field(schema, field));
        }
        markers.extend(dpia_markers_of(schema.fields));
    }

    entries.sort();
    Inventory {
        entries,
        holders: roster,
        dpia_markers: markers,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcessingActivity {
    pub role: String,
    pub category: String,
    pub field_paths: Vec<String>,
    pub lawful_bases: Vec<String>,
    pub retentions: Vec<String>,
    pub regions: Vec<String>,
    pub special_category: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessingActivities {
    pub activities: Vec<ProcessingActivity>,
}

impl ProcessingActivities {
    pub fn len(&self) -> usize {
        self.activities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.activities.is_empty()
    }
}

pub fn ropa_for_tenant(
    _tenant: &myelin_tenancy::TenantId,
    inventory: &Inventory,
) -> ProcessingActivities {
    ropa(inventory)
}

pub fn ropa(inventory: &Inventory) -> ProcessingActivities {
    #[derive(Default)]
    struct Acc {
        field_paths: BTreeSet<String>,
        lawful_bases: BTreeSet<String>,
        retentions: BTreeSet<String>,
        regions: BTreeSet<String>,
        special_category: bool,
    }
    let mut groups: BTreeMap<(String, String), Acc> = BTreeMap::new();

    for e in &inventory.entries {
        let acc = groups
            .entry((e.role.clone(), e.category.clone()))
            .or_default();
        acc.field_paths.insert(e.field_path.clone());
        acc.lawful_bases.insert(e.basis.clone());
        acc.retentions.insert(e.retention.clone());
        acc.regions.insert(e.region.clone());
        if e.category.starts_with("SpecialCategory(") {
            acc.special_category = true;
        }
    }

    let activities = groups
        .into_iter()
        .map(|((role, category), acc)| ProcessingActivity {
            role,
            category,
            field_paths: acc.field_paths.into_iter().collect(),
            lawful_bases: acc.lawful_bases.into_iter().collect(),
            retentions: acc.retentions.into_iter().collect(),
            regions: acc.regions.into_iter().collect(),
            special_category: acc.special_category,
        })
        .collect();

    ProcessingActivities { activities }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_gdpr::PersonalData;
    use myelin_substrate::StoreKind;

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
            category = SpecialCategory(health),
            role = PlatformOperational,
            basis = Consent(c-1),
            retention = Fixed(365d),
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        health_note: String,
        row_version: u64,
    }

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct OpaqueIndexRow {
        doc_id: u64,
        shard: u32,
    }

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn principal_schema() -> HolderSchema {
        HolderSchema::from_schema::<PrincipalRow>(
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "identity_oltp",
            },
            Holder::H15Identity,
            region(),
        )
    }

    fn index_schema() -> HolderSchema {
        HolderSchema::from_schema::<OpaqueIndexRow>(
            HolderRegistration {
                kind: StoreKind::SearchIndex,
                name: "search_index",
            },
            Holder::H7SearchIndex,
            region(),
        )
    }

    #[test]
    fn data_map_emits_an_entry_per_tagged_field_with_every_fact() {
        let holders = [principal_schema(), index_schema()];
        let inv = data_map(&holders);

        assert_eq!(inv.entry_count(), tagged_field_count(&holders));
        assert_eq!(
            inv.entry_count(),
            2,
            "PrincipalRow has 2 tagged fields; OpaqueIndexRow has 0"
        );

        let email = inv
            .entries
            .iter()
            .find(|e| e.field_path == "PrincipalRow.email")
            .expect("the email field is in the map");
        assert_eq!(email.holder_id, "oltp:identity_oltp");
        assert_eq!(email.holder, "H15");
        assert_eq!(email.region, "fr-par");
        assert_eq!(email.category, "ContactInfo");
        assert_eq!(email.role, "PlatformOperational");
        assert_eq!(email.basis, "Contract");
        assert_eq!(email.retention, "UntilContractEnd");
        assert_eq!(email.erasure, "CryptoShred(subject_dek)");
        assert_eq!(email.subject_locator, "principal_id");

        assert!(inv.dpia_markers.contains(&DpiaMarker {
            field_path: "PrincipalRow.health_note".into(),
            special_category_kind: "health".into(),
        }));
        assert_eq!(
            inv.dpia_markers.len(),
            1,
            "exactly the one special-category field"
        );
    }

    #[test]
    fn every_registered_holder_is_in_the_map_including_zero_pii_holders() {
        let holders = [principal_schema(), index_schema()];
        let inv = data_map(&holders);

        assert_eq!(inv.holder_count(), 2);
        assert!(inv.holders.contains("oltp:identity_oltp"));
        assert!(inv.holders.contains("search_index:search_index"));

        let registered = [
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
            inv.coverage_gaps(&registered).is_empty(),
            "every registered holder is in the map - 0 holders absent"
        );
    }

    #[test]
    fn a_registered_holder_absent_from_the_map_is_a_coverage_gap() {
        let inv = data_map(&[principal_schema()]);
        let registered = [
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "identity_oltp",
            },
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "ci_oltp",
            },
        ];
        let gaps = inv.coverage_gaps(&registered);
        assert_eq!(
            gaps,
            vec!["oltp:ci_oltp".to_string()],
            "the registered-but-unmapped holder is the coverage gap"
        );
    }

    #[test]
    fn the_generated_map_is_deterministic_and_order_independent() {
        let a = data_map(&[principal_schema(), index_schema()]);
        let b = data_map(&[index_schema(), principal_schema()]);
        assert_eq!(a, b, "the map is order-independent (sorted)");
        assert_eq!(
            a.fingerprint(),
            b.fingerprint(),
            "the fingerprint is deterministic"
        );
        assert!(a.fingerprint().starts_with("blake3:"));

        let c = data_map(&[principal_schema()]);
        assert_ne!(
            a.fingerprint(),
            c.fingerprint(),
            "a changed inventory diffs"
        );
    }

    #[test]
    fn ropa_projects_the_inventory_grouped_by_processing_activity() {
        let inv = data_map(&[principal_schema()]);
        let tenant = myelin_tenancy::TenantId::from_token("acme");
        let activities = ropa_for_tenant(&tenant, &inv);

        assert_eq!(activities.len(), 2);

        let contact = activities
            .activities
            .iter()
            .find(|a| a.category == "ContactInfo")
            .expect("the ContactInfo activity");
        assert_eq!(contact.role, "PlatformOperational");
        assert_eq!(contact.field_paths, vec!["PrincipalRow.email".to_string()]);
        assert_eq!(contact.lawful_bases, vec!["Contract".to_string()]);
        assert_eq!(contact.regions, vec!["fr-par".to_string()]);
        assert!(
            !contact.special_category,
            "ContactInfo is not special-category"
        );

        let health = activities
            .activities
            .iter()
            .find(|a| a.category.starts_with("SpecialCategory"))
            .expect("the special-category activity");
        assert!(
            health.special_category,
            "the Art. 9 activity is flagged special-category"
        );
        assert_eq!(health.lawful_bases, vec!["Consent(c-1)".to_string()]);
    }

    #[test]
    fn ropa_collapses_same_activity_fields_and_dedups_rollups() {
        #[derive(PersonalData)]
        #[allow(dead_code)]
        struct OtherContact {
            #[personal_data(
                category = ContactInfo,
                role = PlatformOperational,
                basis = Contract,
                retention = UntilContractEnd,
                erasure = CryptoShred(subject_dek),
                subject_locator = "principal_id"
            )]
            phone: String,
        }
        let other = HolderSchema::from_schema::<OtherContact>(
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "billing_oltp",
            },
            Holder::H18GdprOwn,
            region(),
        );
        let inv = data_map(&[principal_schema(), other]);
        let acts = ropa(&inv);

        let contact = acts
            .activities
            .iter()
            .find(|a| a.category == "ContactInfo")
            .expect("the shared ContactInfo activity");
        assert_eq!(
            contact.field_paths,
            vec![
                "OtherContact.phone".to_string(),
                "PrincipalRow.email".to_string()
            ]
        );
        assert_eq!(contact.lawful_bases, vec!["Contract".to_string()]);
        assert_eq!(contact.regions, vec!["fr-par".to_string()]);
    }

    #[test]
    fn cdc_dsr_orchestrator_resolves_the_fan_out_checklist_from_the_map() {
        let inv = data_map(&[principal_schema(), index_schema()]);

        let mut checklist: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for holder_id in &inv.holders {
            checklist.entry(holder_id.clone()).or_default();
        }
        for e in &inv.entries {
            checklist
                .entry(e.holder_id.clone())
                .or_default()
                .push(e.erasure.clone());
        }

        assert!(checklist.contains_key("oltp:identity_oltp"));
        assert!(checklist.contains_key("search_index:search_index"));
        let id_mechs = &checklist["oltp:identity_oltp"];
        assert_eq!(id_mechs.len(), 2, "both tagged fields drive an erasure");
        assert!(id_mechs.iter().all(|m| m == "CryptoShred(subject_dek)"));
        assert_eq!(checklist.len(), inv.holder_count());
    }

    #[test]
    fn inventory_and_ropa_round_trip_serialize() {
        let inv = data_map(&[principal_schema(), index_schema()]);
        let back: Inventory = serde_json::from_str(&serde_json::to_string(&inv).unwrap()).unwrap();
        assert_eq!(back, inv);

        let acts = ropa(&inv);
        let acts_back: ProcessingActivities =
            serde_json::from_str(&serde_json::to_string(&acts).unwrap()).unwrap();
        assert_eq!(acts_back, acts);
    }

    #[test]
    fn ropa_for_tenant_matches_the_pure_projection() {
        let inv = data_map(&[principal_schema()]);
        let tenant = myelin_tenancy::TenantId::from_token("acme");
        assert_eq!(ropa_for_tenant(&tenant, &inv), ropa(&inv));
    }

    #[test]
    fn empty_holder_set_yields_an_empty_map() {
        let inv = data_map(&[]);
        assert_eq!(inv.entry_count(), 0);
        assert_eq!(inv.holder_count(), 0);
        assert!(inv.coverage_gaps(&[]).is_empty());
        let acts = ropa(&inv);
        assert!(
            acts.is_empty(),
            "an empty inventory projects to no activity"
        );
        assert_eq!(acts.len(), 0);
        let populated = ropa(&data_map(&[principal_schema()]));
        assert!(
            !populated.is_empty(),
            "a populated inventory has activities"
        );
    }
}
