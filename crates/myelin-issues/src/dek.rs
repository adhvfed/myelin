use myelin_gdpr::ErasureMethod;
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, KeyChoiceError, SubjectId};
use myelin_storage::kms::{KekId, KeyClass, KmsEngine, SubjectKeyScope};
use myelin_tenancy::{Region, TenantId};

pub fn subject_dek_erasure() -> ErasureMethod {
    ErasureMethod::CryptoShred("subject_dek".to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IssueFreeText {
    Title,
    Props,
    ChangeDelta,
    CommentBody,
}

impl IssueFreeText {
    pub fn label(self) -> &'static str {
        match self {
            IssueFreeText::Title => "title",
            IssueFreeText::Props => "props",
            IssueFreeText::ChangeDelta => "change_delta",
            IssueFreeText::CommentBody => "comment_body",
        }
    }

    pub fn erasure(self) -> ErasureMethod {
        subject_dek_erasure()
    }

    pub const ALL: [IssueFreeText; 4] = [
        IssueFreeText::Title,
        IssueFreeText::Props,
        IssueFreeText::ChangeDelta,
        IssueFreeText::CommentBody,
    ];
}

pub fn encrypt_free_text(
    engine: &KmsEngine,
    region: &Region,
    tenant: &TenantId,
    subject: &SubjectId,
    _kind: IssueFreeText,
    plaintext: &[u8],
) -> Result<EncryptedColumn, KeyChoiceError> {
    engine.ensure_kek(&KekId::new(tenant.clone(), region.clone()))?;
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.encrypt_for_subject_scope(tenant, subject, SubjectKeyScope::Issues, plaintext)
}

pub fn decrypt_free_text(
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

pub fn issue_subject_key_class(subject: &str) -> KeyClass {
    KeyClass::ScopedSubject {
        scope: SubjectKeyScope::Issues,
        subject: subject.to_string(),
    }
}

pub fn is_issue_subject_key_class(class: &KeyClass, subject: &str) -> bool {
    class == &issue_subject_key_class(subject) || class == &KeyClass::Subject(subject.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> KmsEngine {
        KmsEngine::new()
    }
    fn region() -> Region {
        Region::new("fr-par")
    }
    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn subject() -> SubjectId {
        SubjectId::new("8a2f@acme.noreply")
    }

    #[test]
    fn free_text_round_trips_through_subject_dek() {
        let eng = engine();
        let plaintext = b"fix the login bug for Ada".to_vec();
        let column = encrypt_free_text(
            &eng,
            &region(),
            &tenant(),
            &subject(),
            IssueFreeText::Title,
            &plaintext,
        )
        .expect("seal under the subject DEK");
        let opened = decrypt_free_text(&eng, &region(), &column).expect("open while the key lives");
        assert_eq!(opened, plaintext, "the free-text round-trips exactly");
    }

    #[test]
    fn zero_plaintext_free_text_at_rest_for_every_column_kind() {
        let eng = engine();
        for kind in IssueFreeText::ALL {
            let plaintext = format!("personal free text in {}", kind.label()).into_bytes();
            let column =
                encrypt_free_text(&eng, &region(), &tenant(), &subject(), kind, &plaintext)
                    .expect("seal");
            assert!(
                column
                    .key_ref
                    .class
                    .as_token()
                    .starts_with("scoped-subject:issues:"),
                "the {} column is keyed under the per-subject DEK (GD-4)",
                kind.label()
            );
            assert!(
                !plaintext_at_rest(&column, &plaintext),
                "0 plaintext free-text at rest for the {} column",
                kind.label()
            );
        }
    }

    #[test]
    fn every_free_text_column_is_keyed_per_subject() {
        for kind in IssueFreeText::ALL {
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
    fn distinct_subjects_get_distinct_deks() {
        let eng = engine();
        let plaintext = b"shared phrase".to_vec();
        let a = encrypt_free_text(
            &eng,
            &region(),
            &tenant(),
            &SubjectId::new("aaaa@acme.noreply"),
            IssueFreeText::Title,
            &plaintext,
        )
        .unwrap();
        let b = encrypt_free_text(
            &eng,
            &region(),
            &tenant(),
            &SubjectId::new("bbbb@acme.noreply"),
            IssueFreeText::Title,
            &plaintext,
        )
        .unwrap();
        assert_ne!(
            a.key_ref.class, b.key_ref.class,
            "each subject has a DISTINCT per-subject DEK (GD-4 individual granularity)"
        );
    }

    #[test]
    fn issue_keys_are_distinct_from_chat_and_agent_data_for_the_same_person() {
        let subject = "human:ada";
        let issues = issue_subject_key_class(subject);
        assert_ne!(
            issues,
            KeyClass::ScopedSubject {
                scope: SubjectKeyScope::Chat,
                subject: subject.into(),
            }
        );
        assert_ne!(
            issues,
            KeyClass::ScopedSubject {
                scope: SubjectKeyScope::AgentData,
                subject: subject.into(),
            }
        );
        assert!(is_issue_subject_key_class(&issues, subject));
        assert!(is_issue_subject_key_class(
            &KeyClass::Subject(subject.into()),
            subject
        ));
        assert!(!is_issue_subject_key_class(
            &KeyClass::ScopedSubject {
                scope: SubjectKeyScope::Issues,
                subject: "human:grace".into(),
            },
            subject
        ));
    }

    #[test]
    fn the_free_text_column_set_is_the_full_coverage() {
        assert_eq!(IssueFreeText::ALL.len(), 4);
        for c in [
            IssueFreeText::Title,
            IssueFreeText::Props,
            IssueFreeText::ChangeDelta,
            IssueFreeText::CommentBody,
        ] {
            assert!(IssueFreeText::ALL.contains(&c), "{} is covered", c.label());
        }
    }
}
