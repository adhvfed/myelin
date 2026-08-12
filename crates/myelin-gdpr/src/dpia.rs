use crate::{HasPersonalData, PersonalDataField};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DpiaMarker {
    pub field_path: String,
    pub special_category_kind: String,
}

impl DpiaMarker {
    pub fn from_field(field: &PersonalDataField) -> Option<DpiaMarker> {
        field.is_special_category().map(|flag| DpiaMarker {
            field_path: format!("{}.{}", field.owning_struct, field.field),
            special_category_kind: flag.kind.to_string(),
        })
    }
}

pub fn dpia_markers<T: HasPersonalData>() -> BTreeSet<DpiaMarker> {
    T::personal_data_fields()
        .iter()
        .filter_map(DpiaMarker::from_field)
        .collect()
}

pub fn dpia_markers_of(fields: &[PersonalDataField]) -> BTreeSet<DpiaMarker> {
    fields.iter().filter_map(DpiaMarker::from_field).collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DpiaVerdict {
    Required { marker: DpiaMarker, reason: String },
}

impl DpiaVerdict {
    pub fn field_path(&self) -> &str {
        match self {
            DpiaVerdict::Required { marker, .. } => &marker.field_path,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DpiaRouter;

impl DpiaRouter {
    pub fn new() -> DpiaRouter {
        DpiaRouter
    }

    pub fn route(
        &self,
        prior: &BTreeSet<DpiaMarker>,
        current: &BTreeSet<DpiaMarker>,
    ) -> Vec<DpiaVerdict> {
        current
            .difference(prior)
            .map(|marker| DpiaVerdict::Required {
                marker: marker.clone(),
                reason: format!(
                    "Art. 35 DPIA required: new special-category flow `{}` (kind: {}) - \
                     awaiting DPO adjudication (surfaced, not auto-decided)",
                    marker.field_path, marker.special_category_kind
                ),
            })
            .collect()
    }

    pub fn route_all_new(&self, current: &BTreeSet<DpiaMarker>) -> Vec<DpiaVerdict> {
        self.route(&BTreeSet::new(), current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PersonalData;

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct Subject {
        #[personal_data(
            category = SpecialCategory(health),
            role = PlatformOperational,
            basis = Consent(c-1),
            retention = Fixed(365d),
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        health_note: String,
        #[personal_data(
            category = ContactInfo,
            role = TenantContent,
            basis = Contract,
            retention = TenantPolicy,
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        email: String,
        row_version: u64,
    }

    #[test]
    fn special_category_field_emits_the_dpia_marker_and_ordinary_fields_do_not() {
        let markers = dpia_markers::<Subject>();
        assert_eq!(
            markers.len(),
            1,
            "100% of (and ONLY) special-category fields emit a marker"
        );
        let marker = markers.iter().next().unwrap();
        assert_eq!(marker.field_path, "Subject.health_note");
        assert_eq!(marker.special_category_kind, "health");

        let special_field_count = Subject::personal_data_fields()
            .iter()
            .filter(|f| f.is_special_category().is_some())
            .count();
        assert_eq!(markers.len(), special_field_count);
    }

    #[test]
    fn marker_minting_agrees_with_the_p107_special_category_flag() {
        for f in Subject::personal_data_fields() {
            assert_eq!(
                DpiaMarker::from_field(f).is_some(),
                f.is_special_category().is_some(),
                "the marker fires iff (and only iff) the P-107 flag does, for {}",
                f.field
            );
        }
    }

    #[test]
    fn router_fires_dpia_required_on_a_new_special_category_flow_only() {
        let router = DpiaRouter::new();
        let prior: BTreeSet<DpiaMarker> = BTreeSet::new();
        let current = dpia_markers::<Subject>();

        let verdicts = router.route(&prior, &current);
        assert_eq!(
            verdicts.len(),
            1,
            "a new special-category flow fires the DPIA gate"
        );
        match &verdicts[0] {
            DpiaVerdict::Required { marker, reason } => {
                assert_eq!(marker.field_path, "Subject.health_note");
                assert!(reason.contains("DPIA required"));
                assert!(
                    reason.contains("DPO"),
                    "the adjudication is surfaced for a DPO, not auto-decided"
                );
            }
        }
        assert_eq!(verdicts[0].field_path(), "Subject.health_note");

        let no_change = router.route(&current, &current);
        assert!(
            no_change.is_empty(),
            "an unchanged flow does not re-fire the DPIA gate"
        );
    }

    #[test]
    fn a_reclassification_to_a_new_special_category_kind_re_fires_the_gate() {
        let router = DpiaRouter::new();
        let prior: BTreeSet<DpiaMarker> = [DpiaMarker {
            field_path: "Subject.health_note".into(),
            special_category_kind: "health".into(),
        }]
        .into_iter()
        .collect();
        let current: BTreeSet<DpiaMarker> = [DpiaMarker {
            field_path: "Subject.health_note".into(),
            special_category_kind: "biometric".into(),
        }]
        .into_iter()
        .collect();
        let verdicts = router.route(&prior, &current);
        assert_eq!(verdicts.len(), 1, "a kind reclassification is a new flow");
        assert_eq!(verdicts[0].field_path(), "Subject.health_note");
    }

    #[test]
    fn route_all_new_records_every_flow_as_a_fresh_obligation() {
        let router = DpiaRouter::new();
        let current = dpia_markers::<Subject>();
        let all = router.route_all_new(&current);
        assert_eq!(all.len(), current.len());
    }

    #[test]
    fn marker_and_verdict_round_trip_serialize() {
        let marker = DpiaMarker {
            field_path: "S.f".into(),
            special_category_kind: "health".into(),
        };
        let back: DpiaMarker =
            serde_json::from_str(&serde_json::to_string(&marker).unwrap()).unwrap();
        assert_eq!(back, marker);

        let verdict = DpiaVerdict::Required {
            marker,
            reason: "r".into(),
        };
        let v_back: DpiaVerdict =
            serde_json::from_str(&serde_json::to_string(&verdict).unwrap()).unwrap();
        assert_eq!(v_back, verdict);
    }

    #[test]
    fn dpia_markers_of_a_slice_matches_the_typed_walk() {
        let from_slice = dpia_markers_of(Subject::personal_data_fields());
        let from_type = dpia_markers::<Subject>();
        assert_eq!(from_slice, from_type);
    }
}
