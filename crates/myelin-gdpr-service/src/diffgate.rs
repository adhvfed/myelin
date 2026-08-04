use crate::datamap::{Inventory, InventoryEntry};
use myelin_gdpr::{DpiaMarker, DpiaRouter, DpiaVerdict};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedBaseline {
    pub inventory: Inventory,
    pub fingerprint: String,
}

impl CommittedBaseline {
    pub fn seal(inventory: Inventory) -> CommittedBaseline {
        let fingerprint = inventory.fingerprint();
        CommittedBaseline {
            inventory,
            fingerprint,
        }
    }

    pub fn is_self_consistent(&self) -> bool {
        self.inventory.fingerprint() == self.fingerprint
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Reclassification {
    pub field_path: String,
    pub before: InventoryEntry,
    pub after: InventoryEntry,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DataMapDiff {
    pub added_fields: Vec<InventoryEntry>,
    pub removed_fields: Vec<InventoryEntry>,
    pub reclassifications: Vec<Reclassification>,
    pub added_holders: Vec<String>,
    pub removed_holders: Vec<String>,
    pub dpia_verdicts: Vec<DpiaVerdict>,
}

impl DataMapDiff {
    pub fn is_clean(&self) -> bool {
        self.added_fields.is_empty()
            && self.removed_fields.is_empty()
            && self.reclassifications.is_empty()
            && self.added_holders.is_empty()
            && self.removed_holders.is_empty()
    }

    pub fn requires_dpia(&self) -> bool {
        !self.dpia_verdicts.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut lines = Vec::new();
        for e in &self.added_fields {
            lines.push(format!(
                "+ field {} ({} @ {})",
                e.field_path, e.holder_id, e.region
            ));
        }
        for e in &self.removed_fields {
            lines.push(format!(
                "- field {} ({} @ {})",
                e.field_path, e.holder_id, e.region
            ));
        }
        for r in &self.reclassifications {
            lines.push(format!(
                "~ reclassified {}: role {}→{}, category {}→{}, basis {}→{}, retention {}→{}, \
                 erasure {}→{}, holder {}→{}, region {}→{}",
                r.field_path,
                r.before.role,
                r.after.role,
                r.before.category,
                r.after.category,
                r.before.basis,
                r.after.basis,
                r.before.retention,
                r.after.retention,
                r.before.erasure,
                r.after.erasure,
                r.before.holder_id,
                r.after.holder_id,
                r.before.region,
                r.after.region,
            ));
        }
        for h in &self.added_holders {
            lines.push(format!("+ holder {h}"));
        }
        for h in &self.removed_holders {
            lines.push(format!("- holder {h}"));
        }
        for v in &self.dpia_verdicts {
            match v {
                DpiaVerdict::Required { marker, reason } => lines.push(format!(
                    "! DPIA REQUIRED {} (kind {}): {}",
                    marker.field_path, marker.special_category_kind, reason
                )),
            }
        }
        lines.join("\n")
    }
}

pub fn diff(baseline: &Inventory, current: &Inventory) -> DataMapDiff {
    let base_by_path: BTreeMap<&str, &InventoryEntry> = baseline
        .entries
        .iter()
        .map(|e| (e.field_path.as_str(), e))
        .collect();
    let cur_by_path: BTreeMap<&str, &InventoryEntry> = current
        .entries
        .iter()
        .map(|e| (e.field_path.as_str(), e))
        .collect();

    let mut added_fields = Vec::new();
    let mut removed_fields = Vec::new();
    let mut reclassifications = Vec::new();

    for (path, cur_entry) in &cur_by_path {
        match base_by_path.get(path) {
            None => added_fields.push((*cur_entry).clone()),
            Some(base_entry) => {
                if base_entry != cur_entry {
                    reclassifications.push(Reclassification {
                        field_path: (*path).to_string(),
                        before: (*base_entry).clone(),
                        after: (*cur_entry).clone(),
                    });
                }
            }
        }
    }
    for (path, base_entry) in &base_by_path {
        if !cur_by_path.contains_key(path) {
            removed_fields.push((*base_entry).clone());
        }
    }

    let added_holders: Vec<String> = current
        .holders
        .difference(&baseline.holders)
        .cloned()
        .collect();
    let removed_holders: Vec<String> = baseline
        .holders
        .difference(&current.holders)
        .cloned()
        .collect();

    let prior_markers: &BTreeSet<DpiaMarker> = &baseline.dpia_markers;
    let current_markers: &BTreeSet<DpiaMarker> = &current.dpia_markers;
    let dpia_verdicts = DpiaRouter::new().route(prior_markers, current_markers);

    reclassifications.sort();
    DataMapDiff {
        added_fields,
        removed_fields,
        reclassifications,
        added_holders,
        removed_holders,
        dpia_verdicts,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateVerdict {
    Unchanged,
    Changed(Box<DataMapDiff>),
    CorruptBaseline,
}

impl GateVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, GateVerdict::Unchanged)
    }

    pub fn diff(&self) -> Option<&DataMapDiff> {
        match self {
            GateVerdict::Changed(d) => Some(d),
            _ => None,
        }
    }
}

pub fn check_against_baseline(baseline: &CommittedBaseline, current: &Inventory) -> GateVerdict {
    if !baseline.is_self_consistent() {
        return GateVerdict::CorruptBaseline;
    }
    if current.fingerprint() == baseline.fingerprint {
        return GateVerdict::Unchanged;
    }
    let d = diff(&baseline.inventory, current);
    debug_assert!(
        !d.is_clean(),
        "fingerprint differs but structured diff is clean - generator/fingerprint disagreement"
    );
    GateVerdict::Changed(Box::new(d))
}

pub const COMMITTED_BASELINE_FINGERPRINT: &str = "gdpr.data_map.committed_baseline.fingerprint";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamap::{data_map, HolderSchema};
    use myelin_gdpr::PersonalData;
    use myelin_substrate::{Holder, HolderRegistration, StoreKind};
    use myelin_tenancy::Region;

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct PrincipalRow {
        #[personal_data(
            category = ContactInfo,
            role = TenantContent,
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
    struct PrincipalRowReclassified {
        #[personal_data(
            category = ContactInfo,
            role = PlatformOperational,
            basis = Contract,
            retention = UntilContractEnd,
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        email: String,
    }

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct ProfileRow {
        #[personal_data(
            category = SpecialCategory(health),
            role = PlatformOperational,
            basis = Consent(c-1),
            retention = Fixed(365d),
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        health_note: String,
    }

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct OpaqueIndexRow {
        doc_id: u64,
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

    fn baseline() -> CommittedBaseline {
        CommittedBaseline::seal(data_map(&[principal_schema(), index_schema()]))
    }

    #[test]
    fn ga_d5_changed_inventory_fails_the_gate_and_unchanged_passes() {
        let base = baseline();

        let same = data_map(&[principal_schema(), index_schema()]);
        let verdict = check_against_baseline(&base, &same);
        assert_eq!(
            verdict,
            GateVerdict::Unchanged,
            "an unchanged map passes the gate"
        );
        assert!(verdict.is_green());
        assert!(verdict.diff().is_none());

        let dropped = data_map(&[principal_schema()]);
        let verdict = check_against_baseline(&base, &dropped);
        assert!(!verdict.is_green(), "a changed map fails the gate");
        let d = verdict.diff().expect("the diff is surfaced for a DPO");
        assert!(!d.is_clean());
        assert_eq!(
            d.removed_holders,
            vec!["search_index:search_index".to_string()]
        );
        assert!(d.added_holders.is_empty());
        assert!(d.added_fields.is_empty() && d.removed_fields.is_empty());
        assert!(
            !d.requires_dpia(),
            "no special-category flow appeared - no DPIA obligation"
        );
        assert!(d.summary().contains("- holder search_index:search_index"));
    }

    #[test]
    fn a_new_pii_field_fails_the_gate_with_the_field_surfaced() {
        let base = CommittedBaseline::seal(data_map(&[index_schema()]));
        let current = data_map(&[principal_schema(), index_schema()]);
        let verdict = check_against_baseline(&base, &current);

        let d = verdict.diff().expect("changed");
        let added: Vec<&str> = d
            .added_fields
            .iter()
            .map(|e| e.field_path.as_str())
            .collect();
        assert_eq!(added, vec!["PrincipalRow.email", "PrincipalRow.handle"]);
        assert_eq!(d.added_holders, vec!["oltp:identity_oltp".to_string()]);
        assert!(d.removed_fields.is_empty());
        let email = d
            .added_fields
            .iter()
            .find(|e| e.field_path == "PrincipalRow.email")
            .unwrap();
        assert_eq!(email.role, "TenantContent");
        assert_eq!(email.category, "ContactInfo");
        assert!(d.summary().contains("+ field PrincipalRow.email"));
    }

    #[test]
    fn a_field_path_change_is_surfaced_as_remove_plus_add() {
        let base = CommittedBaseline::seal(data_map(&[HolderSchema::from_schema::<PrincipalRow>(
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "identity_oltp",
            },
            Holder::H15Identity,
            region(),
        )]));
        let current = data_map(&[HolderSchema::from_schema::<PrincipalRowReclassified>(
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "identity_oltp",
            },
            Holder::H15Identity,
            region(),
        )]);
        let verdict = check_against_baseline(&base, &current);

        let d = verdict.diff().expect("changed");
        let after_email = d
            .added_fields
            .iter()
            .find(|e| e.field_path == "PrincipalRowReclassified.email")
            .expect("the new-path email appears as added");
        assert_eq!(after_email.role, "PlatformOperational");
        let before_email = d
            .removed_fields
            .iter()
            .find(|e| e.field_path == "PrincipalRow.email")
            .expect("the old-path email appears as removed");
        assert_eq!(before_email.role, "TenantContent");
        assert!(d.reclassifications.is_empty());
    }

    #[test]
    fn a_same_path_tag_change_is_a_reclassification_not_add_remove() {
        let before_entry = InventoryEntry {
            field_path: "PrincipalRow.email".into(),
            holder_id: "oltp:identity_oltp".into(),
            holder: "H15".into(),
            region: "fr-par".into(),
            category: "ContactInfo".into(),
            role: "TenantContent".into(),
            basis: "Contract".into(),
            retention: "UntilContractEnd".into(),
            erasure: "CryptoShred(subject_dek)".into(),
            subject_locator: "principal_id".into(),
        };
        let mut after_entry = before_entry.clone();
        after_entry.role = "PlatformOperational".into();

        let mut base_inv = Inventory::default();
        base_inv.entries.push(before_entry.clone());
        base_inv.holders.insert("oltp:identity_oltp".into());
        let base = CommittedBaseline::seal(base_inv);

        let mut cur_inv = Inventory::default();
        cur_inv.entries.push(after_entry.clone());
        cur_inv.holders.insert("oltp:identity_oltp".into());

        let verdict = check_against_baseline(&base, &cur_inv);
        let d = verdict.diff().expect("changed");
        assert!(d.added_fields.is_empty(), "same path ⇒ not an add");
        assert!(d.removed_fields.is_empty(), "same path ⇒ not a remove");
        assert_eq!(d.reclassifications.len(), 1);
        let r = &d.reclassifications[0];
        assert_eq!(r.field_path, "PrincipalRow.email");
        assert_eq!(r.before.role, "TenantContent");
        assert_eq!(r.after.role, "PlatformOperational");
        assert!(d.added_holders.is_empty() && d.removed_holders.is_empty());
        assert!(d.summary().contains("~ reclassified PrincipalRow.email"));
    }

    #[test]
    fn a_new_special_category_flow_routes_into_the_dpia_gate() {
        let base = CommittedBaseline::seal(data_map(&[principal_schema()]));
        assert!(
            base.inventory.dpia_markers.is_empty(),
            "baseline carries no special-category flow"
        );

        let profile = HolderSchema::from_schema::<ProfileRow>(
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "profile_oltp",
            },
            Holder::H15Identity,
            region(),
        );
        let current = data_map(&[principal_schema(), profile]);

        let verdict = check_against_baseline(&base, &current);
        let d = verdict.diff().expect("changed");
        assert!(!d.is_clean());
        assert!(
            d.requires_dpia(),
            "a new special-category flow requires a DPIA"
        );
        assert_eq!(d.dpia_verdicts.len(), 1);
        match &d.dpia_verdicts[0] {
            DpiaVerdict::Required { marker, reason } => {
                assert_eq!(marker.field_path, "ProfileRow.health_note");
                assert_eq!(marker.special_category_kind, "health");
                assert!(reason.contains("DPIA required"));
                assert!(
                    reason.contains("DPO"),
                    "surfaced for a DPO, never auto-decided"
                );
            }
        }
        assert!(d
            .summary()
            .contains("! DPIA REQUIRED ProfileRow.health_note"));
    }

    #[test]
    fn an_ordinary_category_addition_does_not_require_a_dpia() {
        let base = CommittedBaseline::seal(data_map(&[index_schema()]));
        let current = data_map(&[principal_schema(), index_schema()]);
        let d = check_against_baseline(&base, &current)
            .diff()
            .expect("changed")
            .clone();
        assert!(!d.is_clean(), "an added field fails the gate");
        assert!(
            !d.requires_dpia(),
            "but an ordinary-category field is not a DPIA obligation"
        );
        assert!(d.dpia_verdicts.is_empty());
    }

    #[test]
    fn a_corrupt_baseline_is_refused() {
        let mut base = baseline();
        base.fingerprint = "blake3:deadbeef".into();
        assert!(!base.is_self_consistent());
        let current = data_map(&[principal_schema(), index_schema()]);
        let verdict = check_against_baseline(&base, &current);
        assert_eq!(verdict, GateVerdict::CorruptBaseline);
        assert!(!verdict.is_green());
        assert!(
            verdict.diff().is_none(),
            "a corrupt baseline surfaces no content diff"
        );
    }

    #[test]
    fn re_sealing_the_baseline_after_review_makes_the_gate_green() {
        let base = baseline();
        let changed = data_map(&[
            principal_schema(),
            index_schema(),
            HolderSchema::from_schema::<ProfileRow>(
                HolderRegistration {
                    kind: StoreKind::Oltp,
                    name: "profile_oltp",
                },
                Holder::H15Identity,
                region(),
            ),
        ]);
        assert!(
            !check_against_baseline(&base, &changed).is_green(),
            "the change fails the gate"
        );
        let re_sealed = CommittedBaseline::seal(changed.clone());
        assert!(re_sealed.is_self_consistent());
        assert!(
            check_against_baseline(&re_sealed, &changed).is_green(),
            "the re-sealed baseline passes the regenerated map"
        );
    }

    #[test]
    fn the_gate_is_deterministic_and_order_independent() {
        let base = baseline();
        let a = data_map(&[principal_schema(), index_schema()]);
        let b = data_map(&[index_schema(), principal_schema()]);
        assert_eq!(
            check_against_baseline(&base, &a),
            check_against_baseline(&base, &b),
            "order-independent verdict"
        );
        assert!(check_against_baseline(&base, &a).is_green());
    }

    #[test]
    fn diff_and_verdict_round_trip_serialize() {
        let base = CommittedBaseline::seal(data_map(&[index_schema()]));
        let current = data_map(&[principal_schema(), index_schema()]);
        let verdict = check_against_baseline(&base, &current);

        let back: GateVerdict =
            serde_json::from_str(&serde_json::to_string(&verdict).unwrap()).unwrap();
        assert_eq!(back, verdict);

        let base_back: CommittedBaseline =
            serde_json::from_str(&serde_json::to_string(&base).unwrap()).unwrap();
        assert_eq!(base_back, base);
        assert!(base_back.is_self_consistent());
    }

    #[test]
    fn an_unchanged_empty_map_is_clean() {
        let base = CommittedBaseline::seal(data_map(&[]));
        let d = diff(&base.inventory, &data_map(&[]));
        assert!(d.is_clean(), "empty→empty is no change");
        assert!(check_against_baseline(&base, &data_map(&[])).is_green());
    }
}
