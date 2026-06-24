//! # CDC pair — contract 11.4 (per-subject-DEK message bodies) + 10.1 (the Chat `PersonalDataHolder`)
//! for Chat (CHAT-P6 / P-400, M4-C1)
//!
//! **The two halves this artifact proves (the prompt's GATE):**
//! - **11.4 — per-subject-DEK message bodies (the no-plaintext-body property).** PROVIDER: the Chat
//!   body path ([`myelin_chat::encrypt_body`]) seals `body_inline` / `body_nodes` / the composer
//!   `draft` under the AUTHOR's per-subject DEK through the ONE shared
//!   [`myelin_storage::encryption::ColumnCryptor`] over the P-058 `KmsEngine` (NO second cryptor, NO
//!   parallel key store — EI-01 §7): ciphertext + the `kms://<tenant>/<epoch>/subject:<id>`
//!   `pii_key_ref` at rest, 0 plaintext body bytes in the immutable log. CONSUMER: a read-side that
//!   decrypts the sealed body with the named DEK while the key lives, and asserts the crypto-shred
//!   lever works (a destroyed DEK → the body is unrecoverable, never plaintext).
//! - **10.1 — the Chat `PersonalDataHolder` (H5).** PROVIDER: Chat auto-registers its OLTP store as a
//!   holder through the harness ONE door ([`myelin_chat::register_chat_holders`]) and classifies it to
//!   the exhaustive **H5 (`H5Chat`)**. CONSUMER: the DSR-orchestrator-facing surface — the holder is
//!   callable (`locate`/`export`/`restrict`/`erase` return typed content-addressed receipts), and the
//!   completeness assertion sees 0 orphan Chat stores.
//!
//! The provider + consumer are the SAME frozen shapes (one cryptor, one holder trait — EI-01 §7),
//! proven against the in-memory `KmsEngine` + the in-memory holder registry (DB-free). The
//! live-Postgres at-rest round-trip is the `integration`-feature artifact.

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

// ───────────────────────────── 11.4 — per-subject-DEK message bodies ─────────────────────────────

/// **PROVIDER (11.4): the Chat write path seals every body column under the author's per-subject DEK
/// — ciphertext + the `subject:<id>` pii_key_ref at rest, 0 plaintext body bytes in the log.**
#[test]
fn provider_chat_body_seals_under_the_authors_per_subject_dek() {
    let eng = KmsEngine::new();
    for kind in ChatFreeText::ALL {
        let plaintext = format!("personal chat body in {}", kind.label()).into_bytes();
        let column =
            encrypt_body(&eng, &region(), &tenant(), &author(), kind, &plaintext).expect("seal");
        // the pii_key_ref names the per-subject DEK (the GD-4 individual lever).
        assert!(
            matches!(column.key_ref.class, KeyClass::Subject(_)),
            "the {} body is keyed under the per-subject DEK",
            kind.label()
        );
        assert!(column.key_ref.class.as_token().starts_with("subject:"));
        // 0 plaintext body bytes in the immutable log (the no-plaintext-body GATE).
        assert!(
            !plaintext_at_rest(&column, &plaintext),
            "0 plaintext body bytes at rest for {}",
            kind.label()
        );
    }
}

/// **CONSUMER (11.4): the read-side decrypts the sealed body while the key lives, and a crypto-shred
/// of the author's DEK makes the body unrecoverable (the GD-4 erasure lever working) — never a
/// plaintext fall-through.**
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

    // decrypt-while-the-key-lives → the exact plaintext (the per-subject-DEK round-trip).
    let opened = decrypt_body(&eng, &region(), &column).expect("decrypt while the key lives");
    assert_eq!(opened, plaintext, "the body round-trips exactly");

    // crypto-shred the author's per-subject DEK (the Art. 17 lever) → the body is unrecoverable.
    let dek_id = DekId::new(column.key_ref.tenant.clone(), column.key_ref.class.clone());
    assert!(eng.destroy_dek(&dek_id), "the per-subject DEK is destroyed");
    assert!(
        decrypt_body(&eng, &region(), &column).is_err(),
        "a shredded DEK makes the body unrecoverable (0 recoverable) — never plaintext"
    );
}

/// **CONSUMER (11.4): two authors get DISTINCT per-subject DEKs — erasing one leaves the other's
/// bodies intact (the GD-4 individual granularity).**
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
    // shred A's DEK; B's body still opens.
    let a_dek = DekId::new(a.key_ref.tenant.clone(), a.key_ref.class.clone());
    assert!(eng.destroy_dek(&a_dek));
    assert!(decrypt_body(&eng, &region(), &a).is_err(), "A erased");
    assert_eq!(
        decrypt_body(&eng, &region(), &b).expect("B intact"),
        phrase,
        "erasing A leaves B's body intact (individual granularity)"
    );
}

// ───────────────────────────── 10.1 — the Chat `PersonalDataHolder` (H5) ─────────────────────────

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

/// **PROVIDER (10.1): Chat auto-registers its OLTP store as holder H5 through the harness ONE door —
/// classifies to H5 (`H5Chat`), 0 orphans (the DSR fan-out cannot silently miss Chat).**
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
        "0 orphan Chat stores — every store maps to an H-holder"
    );
}

/// **CONSUMER (10.1): the DSR-orchestrator-facing holder surface is callable — `locate`/`export`/
/// `restrict`/`erase` return typed content-addressed receipts (never a `todo!()`/panic).**
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
