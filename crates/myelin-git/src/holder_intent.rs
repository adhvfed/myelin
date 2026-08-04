use myelin_gdpr::{DataRole, ErasureMethod};

pub const HOLDER_ID: &str = "H1";

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
            locus: "commit author/committer identity (opaque pseudonym; real identity in Identity's map)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::Pseudonymise,
            is_x7_residual: false,
        },
        DataLocus {
            locus: "PR/review/comment free-text bodies + titles (encrypted under the per-subject DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("subject_dek".into()),
            is_x7_residual: false,
        },
        DataLocus {
            locus: "personal data inside file content / commit messages authored by others (X-7 residual)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CarveOut,
            is_x7_residual: true,
        },
        DataLocus {
            locus: "LFS blobs (content-addressed; per-tenant blob DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("tenant_blob_dek".into()),
            is_x7_residual: false,
        },
        DataLocus {
            locus: "reflog / push records / SSH-key fingerprints (pseudonymised actor + per-tenant blob DEK)",
            role: DataRole::TenantContent,
            erasure: ErasureMethod::CryptoShred("tenant_blob_dek".into()),
            is_x7_residual: false,
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderRegistration {
    pub holder_id: &'static str,
    pub registered: bool,
}

impl HolderRegistration {
    pub fn auto_register() -> Self {
        Self {
            holder_id: HOLDER_ID,
            registered: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_is_holder_h1() {
        assert_eq!(HOLDER_ID, "H1");
    }

    #[test]
    fn auto_register_produces_a_real_h1_receipt() {
        let r = HolderRegistration::auto_register();
        assert_eq!(r.holder_id, "H1");
        assert!(r.registered);
    }

    #[test]
    fn inventory_is_exhaustive_and_uses_frozen_levers() {
        let inv = personal_data_inventory();
        assert_eq!(inv.len(), 5, "the §4.5 inventory has five loci");

        assert!(
            inv.iter().all(|d| d.role == DataRole::TenantContent),
            "every git data locus is processor-posture tenant content (§6)"
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
            "pseudonymise lever (commit identity, contract 4.8) must be in the inventory"
        );
        assert!(
            inv.iter()
                .any(|d| d.erasure == ErasureMethod::CryptoShred("subject_dek".into())),
            "per-subject DEK crypto-shred (free-text bodies, contract 11.4) must be in the inventory"
        );
    }
}
