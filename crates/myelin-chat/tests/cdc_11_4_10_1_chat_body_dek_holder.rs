use myelin_chat::{
    chat_store_classifier, decrypt_body, encrypt_body, plaintext_at_rest, register_chat_holders,
    ChatFreeText, ChatHolder, CHAT_OLTP_STORE,
};
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::SubjectId;
use myelin_storage::kms::{DekId, KeyClass, KmsEngine};
use myelin_substrate::{
    assert_holder_completeness, classify_store, Holder, HolderRegistry, StoreKind,
};
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
            matches!(column.key_ref.class, KeyClass::Subject(_)),
            "the {} body is keyed under the per-subject DEK",
            kind.label()
        );
        assert!(column.key_ref.class.as_token().starts_with("subject:"));
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
    assert!(eng.destroy_dek(&dek_id), "the per-subject DEK is destroyed");
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
    assert!(eng.destroy_dek(&a_dek));
    assert!(decrypt_body(&eng, &region(), &a).is_err(), "A erased");
    assert_eq!(
        decrypt_body(&eng, &region(), &b).expect("B intact"),
        phrase,
        "erasing A leaves B's body intact (individual granularity)"
    );
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

#[test]
fn provider_chat_store_registers_and_classifies_to_h5_no_orphan() {
    let registry = register_chat_holders();
    assert!(registry.is_registered(StoreKind::Oltp, CHAT_OLTP_STORE));
    let classifier = chat_store_classifier();
    assert_eq!(
        classify_store(StoreKind::Oltp, CHAT_OLTP_STORE, &classifier),
        Some(Holder::H5Chat),
        "the Chat OLTP store is holder H5"
    );
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &classifier),
        Ok(()),
        "0 orphan Chat stores - every store maps to an H-holder"
    );
}

#[test]
fn consumer_chat_holder_surface_is_callable_typed() {
    let mut registry = HolderRegistry::new();
    let holder = ChatHolder::new();
    holder.register(&mut registry);
    assert!(registry.is_registered(StoreKind::Oltp, CHAT_OLTP_STORE));

    let subj = subject("psn:chat-7");
    let t = GdprTenantId::from_token("acme");

    let locate = holder.locate(&subj, t.clone()).expect("locate");
    assert_eq!(locate.receipt.operation, "locate");
    assert!(locate.receipt.content_hash.starts_with("blake3:"));

    let export = holder.export(&subj, t.clone()).expect("export");
    assert_eq!(export.receipt.operation, "export");

    let restrict = holder.restrict(&subj, true).expect("restrict");
    assert_eq!(restrict.receipt.operation, "restrict");
    assert!(
        holder.restriction().is_restricted("psn:chat-7"),
        "restrict flips the real flag the seams read"
    );

    let erase = holder
        .erase(EraseScope::Subject {
            subject: subj,
            tenant: t,
        })
        .expect("erase returns a typed crypto-shred receipt");
    assert_eq!(erase.receipt.operation, "erase");
    assert!(erase.receipt.content_hash.starts_with("blake3:"));
}
