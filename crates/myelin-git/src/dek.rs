use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, KeyChoiceError, SubjectId};
use myelin_storage::kms::{KekId, KeyClass, KmsEngine, SubjectKeyScope};
use myelin_tenancy::{Region, TenantId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitFreeText {
    PullRequestTitle,
    PullRequestBody,
}

impl GitFreeText {
    pub const ALL: [Self; 2] = [Self::PullRequestTitle, Self::PullRequestBody];

    pub const fn label(self) -> &'static str {
        match self {
            Self::PullRequestTitle => "pull_request_title",
            Self::PullRequestBody => "pull_request_body",
        }
    }
}

pub fn encrypt_free_text(
    engine: &KmsEngine,
    region: &Region,
    tenant: &TenantId,
    subject: &SubjectId,
    _kind: GitFreeText,
    plaintext: &[u8],
) -> Result<EncryptedColumn, KeyChoiceError> {
    engine.ensure_kek(&KekId::new(tenant.clone(), region.clone()))?;
    ColumnCryptor::new(engine, region.clone()).encrypt_for_subject_scope(
        tenant,
        subject,
        SubjectKeyScope::Git,
        plaintext,
    )
}

pub fn git_subject_key_class(subject: &str) -> KeyClass {
    KeyClass::ScopedSubject {
        scope: SubjectKeyScope::Git,
        subject: subject.to_string(),
    }
}

/// Accept the product-scoped key used by current writes and the generic subject key used by
/// existing rows. Accepting the legacy key here preserves reads without letting another scoped
/// product key masquerade as Git data.
pub fn is_git_subject_key_class(class: &KeyClass, subject: &str) -> bool {
    class == &git_subject_key_class(subject) || class == &KeyClass::Subject(subject.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region() -> Region {
        Region::new("fr-par")
    }

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    fn subject() -> SubjectId {
        SubjectId::new("human:ada")
    }

    #[test]
    fn every_git_free_text_column_uses_the_git_subject_key() {
        let kms = KmsEngine::new();
        for kind in GitFreeText::ALL {
            let plaintext = format!("private text in {}", kind.label());
            let encrypted = encrypt_free_text(
                &kms,
                &region(),
                &tenant(),
                &subject(),
                kind,
                plaintext.as_bytes(),
            )
            .expect("encrypt Git free text");
            assert_eq!(
                encrypted.key_ref.class,
                git_subject_key_class(subject().as_str())
            );
            assert!(!encrypted.contains_plaintext(plaintext.as_bytes()));
            assert_eq!(
                ColumnCryptor::new(&kms, region())
                    .decrypt(&encrypted)
                    .expect("decrypt Git free text"),
                plaintext.as_bytes()
            );
        }
    }

    #[test]
    fn git_keys_are_isolated_from_other_products_for_the_same_person() {
        let subject = "human:ada";
        let git = git_subject_key_class(subject);
        for scope in [
            SubjectKeyScope::AgentData,
            SubjectKeyScope::Chat,
            SubjectKeyScope::Issues,
        ] {
            assert_ne!(
                git,
                KeyClass::ScopedSubject {
                    scope,
                    subject: subject.into(),
                }
            );
        }
    }

    #[test]
    fn reader_accepts_legacy_subject_keys_but_not_another_scope_or_person() {
        let subject = "human:ada";
        assert!(is_git_subject_key_class(
            &git_subject_key_class(subject),
            subject
        ));
        assert!(is_git_subject_key_class(
            &KeyClass::Subject(subject.into()),
            subject
        ));
        assert!(!is_git_subject_key_class(
            &KeyClass::ScopedSubject {
                scope: SubjectKeyScope::Chat,
                subject: subject.into(),
            },
            subject
        ));
        assert!(!is_git_subject_key_class(
            &git_subject_key_class("human:grace"),
            subject
        ));
    }
}
