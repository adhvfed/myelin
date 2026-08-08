use myelin_gdpr::ErasureMethod;
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, KeyChoiceError, SubjectId};
use myelin_storage::kms::KmsEngine;
use myelin_tenancy::{Region, TenantId};

pub fn knowledge_subject_erasure() -> ErasureMethod {
    ErasureMethod::CryptoShred("subject_dek".into())
}

/// Encrypts one independently erasable Knowledge field. `scope` is stable object/field identity
/// (for example `page:<id>:title` or `page:<id>:block:<id>`) and is authenticated as AAD so
/// ciphertext cannot be swapped between blocks or pages.
pub fn encrypt_text(
    engine: &KmsEngine,
    region: &Region,
    tenant: &TenantId,
    subject: &SubjectId,
    scope: &str,
    plaintext: &[u8],
) -> Result<EncryptedColumn, KeyChoiceError> {
    ColumnCryptor::new(engine, region.clone()).encrypt_with_aad(
        tenant,
        Some(subject),
        &knowledge_subject_erasure(),
        plaintext,
        scope.as_bytes(),
    )
}

pub fn decrypt_text(
    engine: &KmsEngine,
    region: &Region,
    column: &EncryptedColumn,
    scope: &str,
) -> Result<Vec<u8>, KeyChoiceError> {
    ColumnCryptor::new(engine, region.clone()).decrypt_with_aad(column, scope.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::kms::KekId;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn engine() -> KmsEngine {
        let engine = KmsEngine::new();
        engine.ensure_kek(&KekId::new(tenant(), region()));
        engine
    }

    #[test]
    fn field_scope_is_authenticated_and_plaintext_never_reaches_the_column() {
        let engine = engine();
        let plaintext = b"A private incident runbook";
        let column = encrypt_text(
            &engine,
            &region(),
            &tenant(),
            &SubjectId::new("knowledge-author:alice"),
            "page:P1:block:B1",
            plaintext,
        )
        .expect("seal");
        assert!(!column.contains_plaintext(plaintext));
        assert_eq!(
            decrypt_text(&engine, &region(), &column, "page:P1:block:B1").expect("open"),
            plaintext
        );
        assert!(decrypt_text(&engine, &region(), &column, "page:P2:block:B1").is_err());
    }
}
