use myelin_gdpr::PersonalData;
use myelin_tenancy::{Region, TenantId};

#[derive(PersonalData)]
pub struct ChatMessageRow {
    pub tenant: TenantId,
    pub region: Region,
    pub conversation_id: u128,
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
    pub message_body: Vec<u8>,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "author_pseudonym",
    )]
    pub body_nodes: Vec<u8>,
}

#[derive(PersonalData)]
pub struct ChatDraftRow {
    pub tenant: TenantId,
    pub region: Region,
    pub conversation_id: u128,
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
    pub message_body: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_rows_compile_with_personal_data_tags() {
        let message_body = b"hey @ada, can you review **PR 42**?".to_vec();
        let body_nodes = br#"[{"mention":"ada"}]"#.to_vec();
        let msg = ChatMessageRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            conversation_id: 7,
            author_pseudonym: "psn:abc".into(),
            message_body,
            body_nodes,
        };
        assert_eq!(msg.conversation_id, 7);
        assert_eq!(msg.author_pseudonym, "psn:abc");
        assert_eq!(msg.message_body, b"hey @ada, can you review **PR 42**?");

        let message_body = b"draft i haven't sent".to_vec();
        let draft = ChatDraftRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            conversation_id: 7,
            author_pseudonym: "psn:abc".into(),
            message_body,
        };
        assert_eq!(draft.conversation_id, 7);
        assert_eq!(draft.message_body, b"draft i haven't sent");
    }
}
