use myelin_gdpr::ErasureMethod;
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, KeyChoiceError, SubjectId};
use myelin_storage::kms::{KekId, KmsEngine};
use myelin_tenancy::{Region, TenantId};

pub fn subject_dek_erasure() -> ErasureMethod {
    ErasureMethod::CryptoShred("subject_dek".to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChatFreeText {
    BodyInline,
    BodyNodes,
    Draft,
}

impl ChatFreeText {
    pub fn label(self) -> &'static str {
        match self {
            ChatFreeText::BodyInline => "body_inline",
            ChatFreeText::BodyNodes => "body_nodes",
            ChatFreeText::Draft => "draft",
        }
    }

    pub fn erasure(self) -> ErasureMethod {
        subject_dek_erasure()
    }

    pub const ALL: [ChatFreeText; 3] = [
        ChatFreeText::BodyInline,
        ChatFreeText::BodyNodes,
        ChatFreeText::Draft,
    ];
}

pub fn encrypt_body(
    engine: &KmsEngine,
    region: &Region,
    tenant: &TenantId,
    author: &SubjectId,
    _kind: ChatFreeText,
    plaintext: &[u8],
) -> Result<EncryptedColumn, KeyChoiceError> {
    engine.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.encrypt(tenant, Some(author), &subject_dek_erasure(), plaintext)
}

pub fn decrypt_body(
    engine: &KmsEngine,
    region: &Region,
    column: &EncryptedColumn,
) -> Result<Vec<u8>, KeyChoiceError> {
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.decrypt(column)
}

pub fn plaintext_at_rest(column: &EncryptedColumn, plaintext: &[u8]) -> bool {
    column.contains_plaintext(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::kms::KeyClass;

    fn engine() -> KmsEngine {
        KmsEngine::new()
    }
    fn region() -> Region {
        Region::new("fr-par")
    }
    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn author() -> SubjectId {
        SubjectId::new("8a2f@acme.noreply")
    }

    #[test]
    fn body_round_trips_through_subject_dek() {
        let eng = engine();
        let plaintext = b"hey @ada, can you review **PR 42**?".to_vec();
        let column = encrypt_body(
            &eng,
            &region(),
            &tenant(),
            &author(),
            ChatFreeText::BodyInline,
            &plaintext,
        )
        .expect("seal under the author's per-subject DEK");
        let opened = decrypt_body(&eng, &region(), &column).expect("open while the key lives");
        assert_eq!(opened, plaintext, "the body round-trips exactly");
    }

    #[test]
    fn zero_plaintext_body_in_log_for_every_column_kind() {
        let eng = engine();
        for kind in ChatFreeText::ALL {
            let plaintext = format!("personal chat content in {}", kind.label()).into_bytes();
            let column = encrypt_body(&eng, &region(), &tenant(), &author(), kind, &plaintext)
                .expect("seal");
            assert!(
                column.key_ref.class.as_token().starts_with("subject:"),
                "the {} column is keyed under the per-subject DEK (GD-4)",
                kind.label()
            );
            assert!(
                !plaintext_at_rest(&column, &plaintext),
                "0 plaintext body bytes in the immutable log for the {} column",
                kind.label()
            );
        }
    }

    #[test]
    fn every_body_column_is_keyed_per_subject() {
        for kind in ChatFreeText::ALL {
            match kind.erasure() {
                ErasureMethod::CryptoShred(class) => {
                    assert_eq!(class, "subject_dek", "{} → per-subject DEK", kind.label());
                }
                other => panic!(
                    "{} must crypto-shred per-subject, got {other:?}",
                    kind.label()
                ),
            }
        }
    }

    #[test]
    fn distinct_authors_get_distinct_deks() {
        let eng = engine();
        let plaintext = b"same phrase typed by two people".to_vec();
        let a = encrypt_body(
            &eng,
            &region(),
            &tenant(),
            &SubjectId::new("aaaa@acme.noreply"),
            ChatFreeText::BodyInline,
            &plaintext,
        )
        .unwrap();
        let b = encrypt_body(
            &eng,
            &region(),
            &tenant(),
            &SubjectId::new("bbbb@acme.noreply"),
            ChatFreeText::BodyInline,
            &plaintext,
        )
        .unwrap();
        assert_ne!(
            a.key_ref.class, b.key_ref.class,
            "each author has a DISTINCT per-subject DEK (GD-4 individual granularity)"
        );
        assert!(matches!(a.key_ref.class, KeyClass::Subject(_)));
    }

    #[test]
    fn shredded_dek_makes_the_body_unrecoverable() {
        let eng = engine();
        let plaintext = b"secret".to_vec();
        let column = encrypt_body(
            &eng,
            &region(),
            &tenant(),
            &author(),
            ChatFreeText::BodyInline,
            &plaintext,
        )
        .unwrap();
        assert!(decrypt_body(&eng, &region(), &column).is_ok());
        let dek_id = myelin_storage::kms::DekId::new(
            column.key_ref.tenant.clone(),
            column.key_ref.class.clone(),
        );
        assert!(eng.destroy_dek(&dek_id), "destroy the per-subject DEK");
        assert!(
            decrypt_body(&eng, &region(), &column).is_err(),
            "a shredded DEK makes the body unrecoverable (0 recoverable)"
        );
    }

    #[test]
    fn the_body_column_set_is_the_full_coverage() {
        assert_eq!(ChatFreeText::ALL.len(), 3);
        for c in [
            ChatFreeText::BodyInline,
            ChatFreeText::BodyNodes,
            ChatFreeText::Draft,
        ] {
            assert!(ChatFreeText::ALL.contains(&c), "{} is covered", c.label());
        }
    }
}
