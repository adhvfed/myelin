use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};
use myelin_identity_service::{PseudonymEraseError, StoreBackedCheck, TupleStore};
use myelin_storage::{KeyClass, TenantScope};
use myelin_tenancy::{Region, TenantId};

fn slot() -> StoreBackedCheck {
    StoreBackedCheck::new(TupleStore::new(OutboxStore::new()))
}

fn scope(tenant: &str) -> TenantScope {
    scope_region(tenant, "eu-west")
}

fn scope_region(tenant: &str, region: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region(region.into()))
}

fn handle(p: &str, t: &str) -> PseudonymHandle {
    PseudonymHandle::new(p, t).expect("a well-formed handle")
}

fn now() -> Timestamp {
    Timestamp("2026-06-19T00:00:00Z".into())
}

#[test]
fn resolve_pseudonym_round_trips_for_a_live_subject() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    let h = handle("anon-7f3a", "acme");
    slot.pseudonyms()
        .put_mapping(&s, &subject, h.clone())
        .unwrap();

    let resolved = slot
        .resolve_pseudonym_in(&s, &subject)
        .expect("a live subject resolves");
    assert_eq!(
        resolved, h,
        "the live subject resolves to its public pseudonym handle"
    );
    assert_eq!(
        resolved.render(),
        "anon-7f3a@acme.noreply",
        "the frozen grammar"
    );
}

#[test]
fn erase_destroys_the_dek_and_shreds_the_row_and_records_the_ledger() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
        .unwrap();

    let receipt = slot.erase_in(&s, &subject, now());
    assert!(
        receipt.dek_destroyed,
        "the per-subject DEK was destroyed (crypto-shred)"
    );
    assert!(receipt.row_shredded, "the pseudonym-map row was shredded");
    assert_eq!(
        receipt.shredded_dek_class, "subject:p:alice",
        "the destroyed key is named"
    );

    assert!(
        slot.pseudonyms().mapping_of(&s, &subject).is_none(),
        "the pseudonym-map row is shredded"
    );
    assert!(
        slot.pseudonyms().resolve_subject(&s, &subject).is_none(),
        "the subject's real identity is unrecoverable (the DEK is crypto-shredded)"
    );
    assert!(
        slot.erasure_ledger().is_erased(&s, &subject),
        "the erasure is written to the PII-free ledger (10.8)"
    );
}

#[test]
fn erased_subject_real_identity_unrecoverable_but_opaque_id_still_attributes() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
        .unwrap();
    slot.erase_in(&s, &subject, now());

    assert!(slot.pseudonyms().resolve_subject(&s, &subject).is_none());
    let receipt = slot.erase_in(&s, &subject, now());
    assert_eq!(
        receipt.subject, subject,
        "the opaque principal_id survives erasure (it still attributes events)"
    );
}

#[test]
fn resolve_pseudonym_fails_closed_for_an_erased_subject() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
        .unwrap();
    slot.erase_in(&s, &subject, now());

    let r = slot.resolve_pseudonym_in(&s, &subject);
    assert!(
        matches!(r, Err(PseudonymEraseError::Erased { .. })),
        "an erased subject's resolve fails CLOSED (Erased), never a fabricated handle: {r:?}"
    );

    let never = PrincipalId("p:nobody".into());
    assert!(
        matches!(
            slot.resolve_pseudonym_in(&s, &never),
            Err(PseudonymEraseError::NoMapping { .. })
        ),
        "a never-mapped subject is NoMapping (distinct from Erased)"
    );
}

#[test]
fn erase_disables_the_principal_so_no_grants_survive() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
        .unwrap();
    let target = myelin_identity::RevokeTarget::Principal(subject.clone());
    assert!(
        !slot.revocations().is_revoked(&s, &target, &now()),
        "the principal is not disabled before erase"
    );
    slot.erase_in(&s, &subject, now());
    assert!(
        slot.revocations().is_revoked(&s, &target, &now()),
        "the erased principal is disabled (no resurrected grants past the erasure)"
    );
}

#[test]
fn crypto_shred_is_unrecoverable_in_backups() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
        .unwrap();
    let dek_class = KeyClass::Subject("p:alice".into());

    let pre = slot.kms().backup_snapshot();
    assert!(
        pre.iter()
            .any(|(id, _)| id.class == dek_class && id.tenant == *s.tenant()),
        "the subject's DEK is in the backup BEFORE erase"
    );

    slot.erase_in(&s, &subject, now());

    let post = slot.kms().backup_snapshot();
    assert!(
        !post
            .iter()
            .any(|(id, _)| id.class == dek_class && id.tenant == *s.tenant()),
        "the crypto-shredded DEK is unrecoverable in backups (STOR-D4): 0 recoverable"
    );
}

#[test]
fn re_erase_of_an_already_shredded_subject_is_a_noop_but_recorded() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
        .unwrap();
    let first = slot.erase_in(&s, &subject, now());
    assert!(first.dek_destroyed && first.row_shredded);

    let second = slot.erase_in(&s, &subject, now());
    assert!(
        !second.dek_destroyed,
        "the DEK was already destroyed (no-op)"
    );
    assert!(!second.row_shredded, "the row was already shredded (no-op)");
    assert!(
        slot.erasure_ledger().is_erased(&s, &subject),
        "the subject is still recorded erased"
    );
}

#[test]
fn erase_is_tenant_scoped() {
    let slot = slot();
    let acme = scope("acme");
    let globex = scope("globex");
    let subject = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&acme, &subject, handle("anon-a", "acme"))
        .unwrap();
    slot.pseudonyms()
        .put_mapping(&globex, &subject, handle("anon-g", "globex"))
        .unwrap();

    slot.erase_in(&acme, &subject, now());

    assert!(
        slot.pseudonyms().mapping_of(&acme, &subject).is_none(),
        "acme's mapping is erased"
    );
    assert!(
        slot.pseudonyms().mapping_of(&globex, &subject).is_some(),
        "globex's identically-named subject is untouched (the erase is tenant-scoped)"
    );
    assert!(slot.erasure_ledger().is_erased(&acme, &subject));
    assert!(
        !slot.erasure_ledger().is_erased(&globex, &subject),
        "globex's subject is NOT in acme's erasure ledger"
    );
}
