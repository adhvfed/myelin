use myelin_ci_sandbox::events::CI_ARTIFACT_PUBLISHED;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, PiiKeyRef as EnvelopePiiKeyRef,
    Visibility,
};
use myelin_storage::kms::{KeyClass, PiiKeyRef};
use myelin_tenancy::{Region, TenantId};

use crate::log_pipeline::CrossRegionLogWrite;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentPii {
    IsolableSubject { subject_id: String },
    NotIsolable,
}

pub fn select_log_segment_dek(tenant: &TenantId, dek_epoch: u64, pii: &SegmentPii) -> PiiKeyRef {
    let class = match pii {
        SegmentPii::IsolableSubject { subject_id } => KeyClass::Subject(subject_id.clone()),
        SegmentPii::NotIsolable => KeyClass::Tenant,
    };
    PiiKeyRef::new(tenant.clone(), dek_epoch, class)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactWritePin {
    tenant_id: String,
    cell_region: Region,
    cross_region_writes_admitted: u64,
}

impl ArtifactWritePin {
    pub fn for_cell(tenant_id: impl Into<String>, cell_region: Region) -> ArtifactWritePin {
        ArtifactWritePin {
            tenant_id: tenant_id.into(),
            cell_region,
            cross_region_writes_admitted: 0,
        }
    }

    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    pub fn cross_region_writes_admitted(&self) -> u64 {
        self.cross_region_writes_admitted
    }

    pub fn admit_write(&mut self, row_region: &Region) -> Result<(), CrossRegionLogWrite> {
        if *row_region != self.cell_region {
            return Err(CrossRegionLogWrite {
                tenant_id: self.tenant_id.clone(),
                cell_region: self.cell_region.clone(),
                row_region: row_region.clone(),
            });
        }
        self.cross_region_writes_admitted += 1;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedArtifact {
    pub tenant_id: String,
    pub region: String,
    pub run_id: String,
    pub name: String,
    pub blob_ref: String,
    pub size_bytes: u64,
    pub pii_key_ref: String,
}

impl PublishedArtifact {
    pub fn artifact_ref(&self) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://{}/ci/run/{}/artifact/{}",
            self.tenant_id, self.run_id, self.name
        ))
    }

    pub fn published_draft(&self) -> EventDraft {
        EventDraft {
            type_: EventType(CI_ARTIFACT_PUBLISHED.to_string()),
            subject: self.artifact_ref(),
            aggregate: AggregateKey(format!("run:{}", self.run_id)),
            payload: serde_json::json!({
                "run_id": self.run_id,
                "name": self.name,
                "blob_ref": self.blob_ref,
                "size_bytes": self.size_bytes,
                "region": self.region,
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: Some(EnvelopePiiKeyRef(self.pii_key_ref.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::validate_event_type;
    use myelin_storage::blob::ContentHash;

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn fr_par() -> Region {
        Region::new("fr-par")
    }

    #[test]
    fn isolable_subject_pii_selects_the_per_subject_dek() {
        let key = select_log_segment_dek(
            &tenant(),
            3,
            &SegmentPii::IsolableSubject {
                subject_id: "u-42".into(),
            },
        );
        assert_eq!(key.class, KeyClass::Subject("u-42".into()));
        assert_eq!(key.to_uri(), "kms://acme/3/subject:u-42");
    }

    #[test]
    fn non_isolable_pii_falls_back_to_the_per_tenant_dek() {
        let key = select_log_segment_dek(&tenant(), 0, &SegmentPii::NotIsolable);
        assert_eq!(key.class, KeyClass::Tenant);
        assert_eq!(key.to_uri(), "kms://acme/0/tenant");
    }

    #[test]
    fn dek_selection_is_a_pure_function_of_the_isolability_input() {
        let subj = select_log_segment_dek(
            &tenant(),
            7,
            &SegmentPii::IsolableSubject {
                subject_id: "x".into(),
            },
        );
        let tenant_key = select_log_segment_dek(&tenant(), 7, &SegmentPii::NotIsolable);
        assert_ne!(subj.class, tenant_key.class);
        assert_eq!(subj.dek_epoch, 7);
        assert_eq!(tenant_key.dek_epoch, 7);
    }

    #[test]
    fn residency_pin_admits_in_region_and_refuses_out_of_region() {
        let mut pin = ArtifactWritePin::for_cell("acme", fr_par());
        pin.admit_write(&fr_par()).expect("in-region admitted");
        assert_eq!(pin.cross_region_writes_admitted(), 1);
        let refused = pin.admit_write(&Region::new("us-east"));
        assert!(refused.is_err(), "out-of-region write must be refused");
        assert_eq!(pin.cross_region_writes_admitted(), 1);
        assert_eq!(pin.cell_region(), &fr_par());
    }

    #[test]
    fn published_artifact_emits_a_well_formed_ci_artifact_published_draft() {
        let blob = ContentHash::blake3(b"build-output").to_multihash_string();
        let art = PublishedArtifact {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            run_id: "run-1".into(),
            name: "app.tar.gz".into(),
            blob_ref: blob.clone(),
            size_bytes: 12,
            pii_key_ref: "kms://acme/0/tenant".into(),
        };
        let draft = art.published_draft();

        assert_eq!(draft.type_.0, "ci.artifact.published");
        assert!(validate_event_type(&draft.type_.0).is_ok());
        assert_eq!(
            draft.subject.0,
            "myelin://acme/ci/run/run-1/artifact/app.tar.gz"
        );
        assert_eq!(draft.aggregate.0, "run:run-1");
        assert_eq!(draft.payload["blob_ref"], blob);
        assert_eq!(draft.payload["size_bytes"], 12);
        assert!(!draft.contains_personal_data);
        assert_eq!(
            draft.pii_key_ref.map(|r| r.0).as_deref(),
            Some("kms://acme/0/tenant")
        );
    }

    #[test]
    fn artifact_with_isolable_pii_carries_the_per_subject_dek_ref() {
        let key = select_log_segment_dek(
            &tenant(),
            2,
            &SegmentPii::IsolableSubject {
                subject_id: "u-7".into(),
            },
        );
        let art = PublishedArtifact {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            run_id: "run-9".into(),
            name: "report.json".into(),
            blob_ref: "blake3:dead".into(),
            size_bytes: 4,
            pii_key_ref: key.to_uri(),
        };
        let draft = art.published_draft();
        assert_eq!(
            draft.pii_key_ref.map(|r| r.0).as_deref(),
            Some("kms://acme/2/subject:u-7")
        );
    }
}
