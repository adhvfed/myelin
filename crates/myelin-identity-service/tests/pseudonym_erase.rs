//! # P-ID-20 (P-078) — `resolve_pseudonym` + `erase`: the per-subject crypto-shred lever
//!
//! The required UNIT-level proofs of the 4.8 erase body the prompt names (architecture §11/§12,
//! recon §X-7, EI-04 §1, EI-01 §3, drill-catalogue ID-D8 + STOR-D4):
//!
//! - `erase(subject)` destroys the per-subject DEK AND shreds the pseudonym-map row;
//! - an erased subject's real identity is unrecoverable WHILE its opaque `principal_id` still
//!   attributes events (the EI-04 §1 immutable-attribution split);
//! - `resolve_pseudonym` round-trips for a live subject and FAILS CLOSED for an erased one;
//! - the erasure is written to the PII-free erasure ledger (10.8);
//! - **STOR-D4-adjacent:** the per-subject crypto-shred is unrecoverable IN BACKUPS (the destroyed
//!   DEK is excluded from `backup_snapshot`);
//! - **the per-subject-key MUTATION FLOOR:** an erase that left the DEK recoverable MUST be caught
//!   (a post-erase resolve fails loudly; a destroyed DEK stays out of the backup).

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};
use myelin_identity_service::{
    PseudonymEraseError, StoreBackedCheck, TupleStore,
};
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

/// **`resolve_pseudonym_in` round-trips for a LIVE subject — returns the public pseudonym handle
/// (DSR step-1 attribution).** A mapped subject resolves to its `<pseudonym>@<tenant>.noreply`.
#[test]
fn resolve_pseudonym_round_trips_for_a_live_subject() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    let h = handle("anon-7f3a", "acme");
    slot.pseudonyms().put_mapping(&s, &subject, h.clone()).unwrap();

    let resolved = slot
        .resolve_pseudonym_in(&s, &subject)
        .expect("a live subject resolves");
    assert_eq!(resolved, h, "the live subject resolves to its public pseudonym handle");
    assert_eq!(resolved.render(), "anon-7f3a@acme.noreply", "the frozen grammar");
}

/// **`erase_in` destroys the per-subject DEK AND shreds the pseudonym-map row (the crypto-shred
/// body, 4.8).** After erase: the map row is gone, the per-subject DEK is destroyed, and the subject
/// is recorded in the PII-free ledger.
#[test]
fn erase_destroys_the_dek_and_shreds_the_row_and_records_the_ledger() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
        .unwrap();

    let receipt = slot.erase_in(&s, &subject, now());
    assert!(receipt.dek_destroyed, "the per-subject DEK was destroyed (crypto-shred)");
    assert!(receipt.row_shredded, "the pseudonym-map row was shredded");
    assert_eq!(receipt.shredded_dek_class, "subject:p:alice", "the destroyed key is named");

    // The map row is gone.
    assert!(
        slot.pseudonyms().mapping_of(&s, &subject).is_none(),
        "the pseudonym-map row is shredded"
    );
    // The per-subject DEK is destroyed (the real-identity link is unrecoverable).
    assert!(
        slot.pseudonyms().resolve_subject(&s, &subject).is_none(),
        "the subject's real identity is unrecoverable (the DEK is crypto-shredded)"
    );
    // The PII-free ledger remembers the erasure (so re-erasure can replay).
    assert!(
        slot.erasure_ledger().is_erased(&s, &subject),
        "the erasure is written to the PII-free ledger (10.8)"
    );
}

/// **An erased subject's real identity is unrecoverable WHILE its opaque `principal_id` still
/// attributes events (the EI-04 §1 immutable-attribution split).** The opaque id is unchanged by the
/// erase — it still names the actor of already-emitted events; only the `pseudonym → real_identity`
/// resolution is destroyed.
#[test]
fn erased_subject_real_identity_unrecoverable_but_opaque_id_still_attributes() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
        .unwrap();
    slot.erase_in(&s, &subject, now());

    // The real identity is gone.
    assert!(slot.pseudonyms().resolve_subject(&s, &subject).is_none());
    // The opaque principal_id is unchanged — it STILL attributes events (the receipt carries it; an
    // event authored by `p:alice` is still attributable to the opaque id, just not de-pseudonymisable).
    let receipt = slot.erase_in(&s, &subject, now());
    assert_eq!(
        receipt.subject, subject,
        "the opaque principal_id survives erasure (it still attributes events)"
    );
}

/// **`resolve_pseudonym_in` FAILS CLOSED for an erased subject — never a fabricated handle (the
/// 0-fail-open invariant).** After erase the row is shredded; a resolve returns a LOUD `Erased`
/// error, distinguishable from a never-mapped `NoMapping`.
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

    // A never-mapped subject is distinguishable (NoMapping, not Erased).
    let never = PrincipalId("p:nobody".into());
    assert!(
        matches!(
            slot.resolve_pseudonym_in(&s, &never),
            Err(PseudonymEraseError::NoMapping { .. })
        ),
        "a never-mapped subject is NoMapping (distinct from Erased)"
    );
}

/// **No resurrected GRANTS past an erasure (the ID-D8 grant side).** `erase_in` disables the
/// principal in S7, so every surface's `check` denies the erased subject.
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

/// **STOR-D4: the per-subject crypto-shred is unrecoverable IN BACKUPS.** After erase, the
/// destroyed per-subject DEK is EXCLUDED from `backup_snapshot` (a backup cannot resurrect the
/// shredded key). This is the mutation-floor: an erase that left the DEK in the backup MUST be caught.
#[test]
fn crypto_shred_is_unrecoverable_in_backups() {
    let slot = slot();
    let s = scope("acme");
    let subject = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
        .unwrap();
    let dek_class = KeyClass::Subject("p:alice".into());

    // Before erase: the subject's DEK is present in a backup.
    let pre = slot.kms().backup_snapshot();
    assert!(
        pre.iter().any(|(id, _)| id.class == dek_class && id.tenant == *s.tenant()),
        "the subject's DEK is in the backup BEFORE erase"
    );

    slot.erase_in(&s, &subject, now());

    // After erase: the destroyed DEK is EXCLUDED from the backup (crypto-shred reaches backups).
    let post = slot.kms().backup_snapshot();
    assert!(
        !post.iter().any(|(id, _)| id.class == dek_class && id.tenant == *s.tenant()),
        "the crypto-shredded DEK is unrecoverable in backups (STOR-D4): 0 recoverable"
    );
}

/// **Idempotency: a re-erase of an already-shredded subject is a no-op-but-recorded (the receipt
/// reports `dek_destroyed=false`) and never fails.** Post-restore re-erasure relies on this.
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
    assert!(!second.dek_destroyed, "the DEK was already destroyed (no-op)");
    assert!(!second.row_shredded, "the row was already shredded (no-op)");
    assert!(
        slot.erasure_ledger().is_erased(&s, &subject),
        "the subject is still recorded erased"
    );
}

/// **The erase is `(tenant, region)`-scoped — a cross-tenant erase cannot reach another tenant.** An
/// erase under `acme` does not touch `globex`'s identically-named subject's mapping.
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

    assert!(slot.pseudonyms().mapping_of(&acme, &subject).is_none(), "acme's mapping is erased");
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
