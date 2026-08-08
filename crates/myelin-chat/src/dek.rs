use myelin_gdpr::ErasureMethod;
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, KeyChoiceError, SubjectId};
use myelin_storage::kms::{KekId, KmsEngine, PiiKeyRef, NONCE_LEN};
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

const BODY_ENVELOPE_MAGIC: &[u8; 5] = b"MYCH\x01";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatBodyEnvelopeError {
    KeyReferenceTooLong,
    Malformed,
}

impl core::fmt::Display for ChatBodyEnvelopeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ChatBodyEnvelopeError::KeyReferenceTooLong => {
                f.write_str("Chat encrypted-body key reference exceeds the wire limit")
            }
            ChatBodyEnvelopeError::Malformed => {
                f.write_str("Chat encrypted-body envelope is malformed")
            }
        }
    }
}

impl std::error::Error for ChatBodyEnvelopeError {}

/// Encodes the cryptographic metadata and ciphertext stored in the opaque
/// `body_inline` column. The versioned format keeps the PostgreSQL message
/// store independent from the key-management implementation.
pub fn encode_encrypted_body(column: &EncryptedColumn) -> Result<Vec<u8>, ChatBodyEnvelopeError> {
    let key_ref = column.key_ref.to_uri();
    let key_ref_len =
        u16::try_from(key_ref.len()).map_err(|_| ChatBodyEnvelopeError::KeyReferenceTooLong)?;
    let mut encoded = Vec::with_capacity(
        BODY_ENVELOPE_MAGIC.len() + 2 + key_ref.len() + NONCE_LEN + column.ciphertext.len(),
    );
    encoded.extend_from_slice(BODY_ENVELOPE_MAGIC);
    encoded.extend_from_slice(&key_ref_len.to_be_bytes());
    encoded.extend_from_slice(key_ref.as_bytes());
    encoded.extend_from_slice(&column.nonce);
    encoded.extend_from_slice(&column.ciphertext);
    Ok(encoded)
}

pub fn decode_encrypted_body(encoded: &[u8]) -> Result<EncryptedColumn, ChatBodyEnvelopeError> {
    let header_len = BODY_ENVELOPE_MAGIC.len() + 2;
    if encoded.len() < header_len + NONCE_LEN || !encoded.starts_with(BODY_ENVELOPE_MAGIC) {
        return Err(ChatBodyEnvelopeError::Malformed);
    }
    let key_ref_len = u16::from_be_bytes([
        encoded[BODY_ENVELOPE_MAGIC.len()],
        encoded[BODY_ENVELOPE_MAGIC.len() + 1],
    ]) as usize;
    let key_ref_start = header_len;
    let key_ref_end = key_ref_start
        .checked_add(key_ref_len)
        .ok_or(ChatBodyEnvelopeError::Malformed)?;
    let nonce_end = key_ref_end
        .checked_add(NONCE_LEN)
        .ok_or(ChatBodyEnvelopeError::Malformed)?;
    if key_ref_len == 0 || nonce_end > encoded.len() {
        return Err(ChatBodyEnvelopeError::Malformed);
    }
    let key_ref = std::str::from_utf8(&encoded[key_ref_start..key_ref_end])
        .ok()
        .and_then(PiiKeyRef::parse)
        .ok_or(ChatBodyEnvelopeError::Malformed)?;
    let nonce: [u8; NONCE_LEN] = encoded[key_ref_end..nonce_end]
        .try_into()
        .map_err(|_| ChatBodyEnvelopeError::Malformed)?;
    Ok(EncryptedColumn {
        key_ref,
        nonce,
        ciphertext: encoded[nonce_end..].to_vec(),
    })
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
    fn encrypted_body_envelope_round_trips_without_plaintext() {
        let eng = engine();
        let plaintext = b"a private release discussion";
        let column = encrypt_body(
            &eng,
            &region(),
            &tenant(),
            &author(),
            ChatFreeText::BodyInline,
            plaintext,
        )
        .expect("seal");
        let encoded = encode_encrypted_body(&column).expect("encode");
        assert!(!encoded
            .windows(plaintext.len())
            .any(|window| window == plaintext));
        let decoded = decode_encrypted_body(&encoded).expect("decode");
        assert_eq!(decoded, column);
        assert_eq!(
            decrypt_body(&eng, &region(), &decoded).expect("open"),
            plaintext
        );
    }

    #[test]
    fn encrypted_body_envelope_rejects_plaintext_and_truncation() {
        assert_eq!(
            decode_encrypted_body(b"a plaintext shortcut"),
            Err(ChatBodyEnvelopeError::Malformed)
        );
        assert_eq!(
            decode_encrypted_body(b"MYCH\x01\x00\x10short"),
            Err(ChatBodyEnvelopeError::Malformed)
        );
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
