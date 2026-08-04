use myelin_gdpr::{DataRole, ErasureMethod};

pub const HOLDER_ID: &str = "H3";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataLocus {
    pub locus: &'static str,
    pub role: DataRole,
    pub erasure: ErasureMethod,
    pub is_x7_residual: bool,
}

pub fn personal_data_inventory() -> Vec<DataLocus> {
    vec![
        DataLocus {
            locus: "assignee/reporter/created_by/mentionee/watcher identity (opaque pseudonym; real identity in Identity's map)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::Pseudonymise,
            is_x7_residual: false,
        },
        DataLocus {
            locus: "issue title/props + comment bodies + change-log deltas (encrypted under the per-subject DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("subject_dek".into()),
            is_x7_residual: false,
        },
        DataLocus {
            locus: "third-party free-text PII in another person's issue body/comment (author's DEK - X-7 residual)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CarveOut,
            is_x7_residual: true,
        },
        DataLocus {
            locus: "worklog / productivity / estimate fields (OQ-H behavioural; restricted-by-default; per-subject DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("subject_dek".into()),
            is_x7_residual: false,
        },
        DataLocus {
            locus: "attachment filenames / blobs (content-addressed; per-tenant blob DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("tenant_blob_dek".into()),
            is_x7_residual: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_is_holder_h3() {
        assert_eq!(HOLDER_ID, "H3");
    }

    #[test]
    fn inventory_is_exhaustive_and_uses_frozen_levers() {
        let inv = personal_data_inventory();
        assert_eq!(inv.len(), 5, "the 03 §7 inventory has five loci");

        assert!(
            inv.iter().all(|d| d.role == DataRole::TenantContent),
            "every Issues data locus is processor-posture tenant content (03 §7)"
        );

        let residuals: Vec<_> = inv.iter().filter(|d| d.is_x7_residual).collect();
        assert_eq!(
            residuals.len(),
            1,
            "exactly one locus is the X-7 residual (third-party/immutable free-text)"
        );
        assert_eq!(residuals[0].erasure, ErasureMethod::CarveOut);

        assert!(
            inv.iter().any(|d| d.erasure == ErasureMethod::Pseudonymise),
            "pseudonymise lever (issue identity, contract 4.8) must be in the inventory"
        );
        assert!(
            inv.iter()
                .any(|d| d.erasure == ErasureMethod::CryptoShred("subject_dek".into())),
            "per-subject DEK crypto-shred (free-text + OQ-H worklog, contract 11.4) must be in the inventory"
        );
    }
}
