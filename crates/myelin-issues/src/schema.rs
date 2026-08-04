use myelin_gdpr::PersonalData;
use myelin_tenancy::{Region, TenantId};

#[derive(PersonalData)]
pub struct Issue {
    pub tenant: TenantId,
    pub region: Region,
    pub project_id: u128,
    pub seqno: i64,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by_pseudonym",
    )]
    pub title: String,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by_pseudonym",
    )]
    pub props: Vec<u8>,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "assignee_pseudonym",
    )]
    pub assignee_pseudonym: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "reporter_pseudonym",
    )]
    pub reporter_pseudonym: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "created_by_pseudonym",
    )]
    pub created_by_pseudonym: String,
    #[personal_data(
        category = Behavioural,
        role = TenantContent,
        basis = TBD_LEGAL,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by_pseudonym",
        data_role_default = Restricted,
    )]
    pub worklog_seconds: i64,
    #[personal_data(
        category = Behavioural,
        role = TenantContent,
        basis = TBD_LEGAL,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "assignee_pseudonym",
        data_role_default = Restricted,
    )]
    pub story_points: f64,
}

#[derive(PersonalData)]
pub struct IssueComment {
    pub tenant: TenantId,
    pub region: Region,
    pub comment_id: u128,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "author_pseudonym",
    )]
    pub author_pseudonym: String,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "author_pseudonym",
    )]
    pub comment_text: Vec<u8>,
}

#[derive(PersonalData)]
pub struct IssueChangeLog {
    pub tenant: TenantId,
    pub region: Region,
    pub change_id: u128,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "actor_pseudonym",
    )]
    pub actor_pseudonym: String,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "actor_pseudonym",
    )]
    pub change_delta: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_rows_compile_with_personal_data_tags() {
        let issue = Issue {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            project_id: 1,
            seqno: 1421,
            title: "fix the login bug".into(),
            props: b"{\"severity\":3}".to_vec(),
            assignee_pseudonym: "psn:abc".into(),
            reporter_pseudonym: "psn:def".into(),
            created_by_pseudonym: "psn:ghi".into(),
            worklog_seconds: 3600,
            story_points: 5.0,
        };
        assert_eq!(issue.seqno, 1421);
        assert_eq!(issue.assignee_pseudonym, "psn:abc");
        assert_eq!(issue.title, "fix the login bug");
        assert_eq!(issue.worklog_seconds, 3600);
        assert_eq!(issue.story_points, 5.0);

        let comment_text = b"nit: rename this".to_vec();
        let comment = IssueComment {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            comment_id: 9,
            author_pseudonym: "psn:jkl".into(),
            comment_text,
        };
        assert_eq!(comment.comment_id, 9);
        assert_eq!(comment.comment_text, b"nit: rename this");

        let log = IssueChangeLog {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            change_id: 7,
            actor_pseudonym: "psn:mno".into(),
            change_delta: b"state: todo -> in_progress".to_vec(),
        };
        assert_eq!(log.change_id, 7);
        assert_eq!(log.actor_pseudonym, "psn:mno");
    }
}
