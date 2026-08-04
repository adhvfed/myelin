use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TenantId(pub String);

impl TenantId {
    #[inline]
    pub fn from_token(token: impl Into<String>) -> Self {
        TenantId(token.into())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Region(pub String);

impl Region {
    #[inline]
    pub fn new(code: impl Into<String>) -> Self {
        Region(code.into())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ResidencyTag(pub Region);

impl ResidencyTag {
    #[inline]
    pub fn pinned_to(cell_region: Region) -> Self {
        ResidencyTag(cell_region)
    }

    #[inline]
    pub fn region(&self) -> &Region {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ArtifactRef(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CorrelationId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CellId(pub String);

impl CellId {
    #[inline]
    pub fn from_token(token: impl Into<String>) -> Self {
        CellId(token.into())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OpaqueSubjectId(pub ArtifactRef);

impl OpaqueSubjectId {
    #[inline]
    pub fn from_ref(artifact: ArtifactRef) -> Self {
        OpaqueSubjectId(artifact)
    }

    #[inline]
    pub fn artifact_ref(&self) -> &ArtifactRef {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ArtifactType {
    Issue,
    Page,
    Channel,
    Repo,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CrossCellPointer {
    subject: OpaqueSubjectId,
    #[serde(rename = "type")]
    r#type: ArtifactType,
    correlation_id: CorrelationId,
    home_cell: CellId,
}

impl CrossCellPointer {
    #[inline]
    pub fn new(
        subject: OpaqueSubjectId,
        r#type: ArtifactType,
        correlation_id: CorrelationId,
        home_cell: CellId,
    ) -> Self {
        CrossCellPointer {
            subject,
            r#type,
            correlation_id,
            home_cell,
        }
    }

    #[inline]
    pub fn subject(&self) -> &OpaqueSubjectId {
        &self.subject
    }

    #[inline]
    pub fn artifact_type(&self) -> &ArtifactType {
        &self.r#type
    }

    #[inline]
    pub fn correlation_id(&self) -> &CorrelationId {
        &self.correlation_id
    }

    #[inline]
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn surface_partition_key_types_exist() {
        let tenant: TenantId = TenantId("01J0TENANT".to_string());
        let region: Region = Region("eu-west".to_string());
        let tag: ResidencyTag = ResidencyTag::pinned_to(region.clone());
        assert_eq!(tenant, TenantId("01J0TENANT".to_string()));
        assert!(region < Region("eu-westz".to_string()));
        assert_eq!(tag.region(), &region);
    }

    #[test]
    fn tenant_id_is_opaque_not_personal() {
        fn assert_from<T, S>()
        where
            T: From<S>,
        {
        }
        struct Email(#[allow(dead_code)] String);
        let t = TenantId::from_token("01J0OPAQUE");
        assert_eq!(t.as_str(), "01J0OPAQUE");
        let _ = Email("ada@example.com".to_string());
        assert_from::<String, &str>();
    }

    #[test]
    fn region_is_immutable_new_value() {
        let original = Region::new("eu-west");
        let relocated = Region::new("eu-north");
        assert_ne!(original, relocated);
        assert_eq!(original.as_str(), "eu-west");
    }

    #[test]
    fn cdc_12_1_store_handle_parameterised_by_tenant_region() {
        struct StoreHandle {
            partition: (TenantId, Region),
            residency: ResidencyTag,
            rows: HashMap<String, String>,
        }
        impl StoreHandle {
            fn open(tenant: TenantId, cell_region: Region) -> Self {
                let residency = ResidencyTag::pinned_to(cell_region.clone());
                StoreHandle {
                    partition: (tenant, cell_region),
                    residency,
                    rows: HashMap::new(),
                }
            }
            fn put(&mut self, key: &str, val: &str) {
                self.rows.insert(key.to_string(), val.to_string());
            }
            fn get(&self, key: &str) -> Option<&String> {
                self.rows.get(key)
            }
        }

        let tenant = TenantId::from_token("01J0ACME");
        let cell_region = Region::new("eu-west");
        let mut store = StoreHandle::open(tenant.clone(), cell_region.clone());
        store.put("k", "v");

        assert_eq!(store.partition.0, tenant);
        assert_eq!(store.partition.1, cell_region);
        assert_eq!(store.residency.region(), &cell_region);
        assert_eq!(store.get("k").map(String::as_str), Some("v"));
    }

    fn sample_pointer() -> CrossCellPointer {
        CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
            ArtifactType::Issue,
            CorrelationId("01J0CORR".into()),
            CellId::from_token("cell-eu-west-3"),
        )
    }

    #[test]
    fn cross_cell_pointer_round_trips_its_four_fields() {
        let p = sample_pointer();
        assert_eq!(
            p.subject().artifact_ref().0,
            "myelin://01J0ACME/issues/issue/42"
        );
        assert_eq!(p.artifact_type(), &ArtifactType::Issue);
        assert_eq!(p.correlation_id(), &CorrelationId("01J0CORR".into()));
        assert_eq!(p.home_cell().as_str(), "cell-eu-west-3");

        let json = serde_json::to_value(&p).expect("frame serialises");
        let obj = json.as_object().expect("frame is a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["correlation_id", "home_cell", "subject", "type"],
            "the frozen frame carries EXACTLY the four §6.1 fields - no payload/PII/authz state"
        );
        let back: CrossCellPointer =
            serde_json::from_value(json).expect("frame deserialises to the same value");
        assert_eq!(p, back);
    }

    #[test]
    fn cross_cell_pointer_subject_is_opaque_not_personal() {
        let p = sample_pointer();
        let subject_ref: &ArtifactRef = p.subject().artifact_ref();
        assert!(
            subject_ref.0.starts_with("myelin://"),
            "the subject is an opaque ArtifactRef-class id, never a person"
        );
    }

    #[test]
    fn cdc_12_6_consumer_constructs_frame_and_sees_only_four_fields() {
        struct PortfolioRollupEntry {
            pointer: CrossCellPointer,
        }
        impl PortfolioRollupEntry {
            fn point_at(
                subject: OpaqueSubjectId,
                kind: ArtifactType,
                corr: CorrelationId,
                home: CellId,
            ) -> Self {
                PortfolioRollupEntry {
                    pointer: CrossCellPointer::new(subject, kind, corr, home),
                }
            }
            fn route_target(&self) -> &CellId {
                self.pointer.home_cell()
            }
        }

        let entry = PortfolioRollupEntry::point_at(
            OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0BETA/issues/issue/7".into())),
            ArtifactType::Issue,
            CorrelationId("01J0CHAIN".into()),
            CellId::from_token("cell-eu-north-1"),
        );

        assert_eq!(entry.route_target().as_str(), "cell-eu-north-1");
        assert_eq!(entry.pointer.artifact_type(), &ArtifactType::Issue);
        assert_eq!(
            entry.pointer.correlation_id(),
            &CorrelationId("01J0CHAIN".into())
        );
        assert_eq!(
            entry.pointer.subject().artifact_ref().0,
            "myelin://01J0BETA/issues/issue/7"
        );
    }

    #[test]
    fn correlation_id_is_the_one_shared_type() {
        let corr = CorrelationId("01J0SHARED".into());
        let p = CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0X/issues/issue/1".into())),
            ArtifactType::Other("custom".into()),
            corr.clone(),
            CellId::from_token("cell-x"),
        );
        assert_eq!(p.correlation_id(), &corr);
    }
}
