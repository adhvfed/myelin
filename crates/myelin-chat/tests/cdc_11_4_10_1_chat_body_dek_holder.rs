use myelin_chat::{decrypt_body, encrypt_body, plaintext_at_rest, ChatFreeText};
use myelin_storage::encryption::SubjectId;
use myelin_storage::kms::{DekId, KeyClass, KmsEngine, SubjectKeyScope};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}
fn region() -> Region {
    Region::new("fr-par")
}
fn author() -> SubjectId {
    SubjectId::new("8a2f@acme.noreply")
}

#[test]
fn provider_chat_body_seals_under_the_authors_per_subject_dek() {
    let eng = KmsEngine::new();
    for kind in ChatFreeText::ALL {
        let plaintext = format!("personal chat body in {}", kind.label()).into_bytes();
        let column =
            encrypt_body(&eng, &region(), &tenant(), &author(), kind, &plaintext).expect("seal");
        assert!(
            matches!(
                column.key_ref.class,
                KeyClass::ScopedSubject {
                    scope: SubjectKeyScope::Chat,
                    ..
                }
            ),
            "the {} body is keyed under the per-subject DEK",
            kind.label()
        );
        assert!(column
            .key_ref
            .class
            .as_token()
            .starts_with("scoped-subject:chat:"));
        assert!(
            !plaintext_at_rest(&column, &plaintext),
            "0 plaintext body bytes at rest for {}",
            kind.label()
        );
    }
}

#[test]
fn consumer_decrypts_while_key_lives_then_shred_makes_it_unrecoverable() {
    let eng = KmsEngine::new();
    let plaintext = b"hey @ada, can you review **PR 42**?".to_vec();
    let column = encrypt_body(
        &eng,
        &region(),
        &tenant(),
        &author(),
        ChatFreeText::BodyInline,
        &plaintext,
    )
    .expect("seal");

    let opened = decrypt_body(&eng, &region(), &column).expect("decrypt while the key lives");
    assert_eq!(opened, plaintext, "the body round-trips exactly");

    let dek_id = DekId::new(column.key_ref.tenant.clone(), column.key_ref.class.clone());
    assert!(
        eng.destroy_dek(&dek_id).unwrap(),
        "the per-subject DEK is destroyed"
    );
    assert!(
        decrypt_body(&eng, &region(), &column).is_err(),
        "a shredded DEK makes the body unrecoverable (0 recoverable) - never plaintext"
    );
}

#[test]
fn consumer_distinct_authors_get_distinct_deks() {
    let eng = KmsEngine::new();
    let phrase = b"the same words typed by two people".to_vec();
    let a = encrypt_body(
        &eng,
        &region(),
        &tenant(),
        &SubjectId::new("aaaa@acme.noreply"),
        ChatFreeText::BodyInline,
        &phrase,
    )
    .unwrap();
    let b = encrypt_body(
        &eng,
        &region(),
        &tenant(),
        &SubjectId::new("bbbb@acme.noreply"),
        ChatFreeText::BodyInline,
        &phrase,
    )
    .unwrap();
    assert_ne!(
        a.key_ref.class, b.key_ref.class,
        "each author has a DISTINCT per-subject DEK"
    );
    let a_dek = DekId::new(a.key_ref.tenant.clone(), a.key_ref.class.clone());
    assert!(eng.destroy_dek(&a_dek).unwrap());
    assert!(decrypt_body(&eng, &region(), &a).is_err(), "A erased");
    assert_eq!(
        decrypt_body(&eng, &region(), &b).expect("B intact"),
        phrase,
        "erasing A leaves B's body intact (individual granularity)"
    );
}
