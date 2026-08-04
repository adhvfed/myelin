use myelin_gdpr::PersonalData;
use myelin_tenancy::{Region, TenantId};

#[derive(PersonalData)]
pub struct CiRunRow {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: u128,
    pub commit_oid: String,
    pub trust_tier: String,
    #[personal_data(
        category = Identifier,
        role = Controller,
        basis = LegitimateInterest,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "triggered_by",
    )]
    pub triggered_by: String,
}

#[derive(PersonalData)]
pub struct DeploymentRow {
    pub tenant: TenantId,
    pub region: Region,
    pub dep_id: u128,
    pub state: String,
    #[personal_data(
        category = Identifier,
        role = Controller,
        basis = LegitimateInterest,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "approved_by",
    )]
    pub approved_by: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_rows_compile_with_personal_data_tags() {
        let run = CiRunRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            run_id: 42,
            commit_oid: "blake3:abcd".into(),
            trust_tier: "trusted".into(),
            triggered_by: "psn:actor-8a2f".into(),
        };
        assert_eq!(run.run_id, 42);
        assert_eq!(run.triggered_by, "psn:actor-8a2f");
        assert_eq!(run.trust_tier, "trusted");

        let dep = DeploymentRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            dep_id: 7,
            state: "awaiting_approval".into(),
            approved_by: "psn:approver-1b3c".into(),
        };
        assert_eq!(dep.dep_id, 7);
        assert_eq!(dep.approved_by, "psn:approver-1b3c");
        assert_eq!(dep.state, "awaiting_approval");
    }
}
