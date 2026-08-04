use myelin_gdpr::PersonalData;
use myelin_tenancy::{Region, TenantId};

#[derive(PersonalData)]
pub struct PullRequest {
    pub tenant: TenantId,
    pub region: Region,
    pub repo_id: u128,
    pub pr_number: i32,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "author_subject_id",
    )]
    pub pr_title: String,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "author_subject_id",
    )]
    pub pr_body: Vec<u8>,
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
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "author_subject_id",
    )]
    pub author_subject_id: String,
}

#[derive(PersonalData)]
pub struct Review {
    pub tenant: TenantId,
    pub region: Region,
    pub review_id: u128,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "reviewer_pseudonym",
    )]
    pub reviewer_pseudonym: String,
}

#[derive(PersonalData)]
pub struct ReviewComment {
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
pub struct Reflog {
    pub tenant: TenantId,
    pub region: Region,
    pub repo_id: u128,
    pub ref_name: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = LegitimateInterest(git_ops_lia),
        retention = TenantPolicy,
        erasure = Pseudonymise,
        subject_locator = "pusher_pseudonym",
    )]
    pub pusher_pseudonym: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_rows_compile_with_personal_data_tags() {
        let pr = PullRequest {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            repo_id: 1,
            pr_number: 42,
            pr_title: "fix the bug".into(),
            pr_body: b"please review".to_vec(),
            author_pseudonym: "psn:abc".into(),
            author_subject_id: "principal-abc".into(),
        };
        assert_eq!(pr.pr_number, 42);
        assert_eq!(pr.author_pseudonym, "psn:abc");
        assert_eq!(pr.pr_title, "fix the bug");

        let review = Review {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            review_id: 7,
            reviewer_pseudonym: "psn:def".into(),
        };
        assert_eq!(review.reviewer_pseudonym, "psn:def");

        let comment_text = b"nit: rename this".to_vec();
        let comment = ReviewComment {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            comment_id: 9,
            author_pseudonym: "psn:ghi".into(),
            comment_text,
        };
        assert_eq!(comment.comment_id, 9);
        assert_eq!(comment.comment_text, b"nit: rename this");

        let reflog = Reflog {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            repo_id: 1,
            ref_name: "refs/heads/main".into(),
            pusher_pseudonym: "psn:jkl".into(),
        };
        assert_eq!(reflog.ref_name, "refs/heads/main");
        assert_eq!(reflog.pusher_pseudonym, "psn:jkl");
    }
}
