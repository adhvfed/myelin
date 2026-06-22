//! # The CDC pair for contract 4.8 (`resolve_pseudonym` + `erase`) — P-ID-20 / P-078
//!
//! **Contract-index row 4.8** (`resolve_pseudonym(subject, tenant)` + the `PersonalDataHolder`
//! `erase(subject)` = DSR step 1; the pseudonym grammar `<pseudonym>@<tenant>.noreply`). This is the
//! dedicated provider+consumer pair the P-ID-20 TESTS field names — the in-CI evidence that the two
//! sides of the **resolve/erase** seam cannot drift apart:
//!
//! - the **PROVIDER** is Identity's S2 pseudonym map + erase body
//!   ([`StoreBackedCheck::erase_in`] / [`StoreBackedCheck::resolve_pseudonym_in`]): an `erase`
//!   destroys the per-subject DEK + shreds the map row + records the PII-free erasure ledger; a
//!   `resolve_pseudonym` returns the public handle for a LIVE subject and FAILS CLOSED for an erased
//!   one.
//! - the **CONSUMER** is the **DSR orchestrator + a Git/Audit consumer** (the row's named consumers:
//!   "Git, Audit, DSR orchestrator"). The DSR orchestrator drives `erase` (DSR step 1). The Git/Audit
//!   consumer attributes an immutable artifact by the subject's PUBLIC pseudonym (a git commit
//!   author) — and, crucially, **HONOURS the erasure**: once a subject is erased, the consumer can no
//!   longer de-pseudonymise it back to a real identity (resolve fails closed), but the OPAQUE
//!   attribution it already baked into the immutable bytes still stands (EI-04 §1).
//!
//! The provider's promise (a live subject resolves to its public handle; an erased subject's resolve
//! fails closed; the per-subject DEK + row are crypto-shredded; the erasure is durably recorded) and
//! the consumer's promise (it attributes by the public handle, drives erase as DSR step 1, and
//! refuses to de-pseudonymise an erased subject) are pinned here so a change to either side fails this
//! test in the same CI job.

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};
use myelin_identity_service::{PseudonymEraseError, StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn handle(p: &str, t: &str) -> PseudonymHandle {
    PseudonymHandle::new(p, t).expect("a well-formed handle")
}

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

/// The PROVIDER: Identity's S2 + erase body, seeded with alice's `subject ↔ pseudonym` mapping.
fn provider(s: &TenantScope, subject: &PrincipalId, pseudonym: &str) -> StoreBackedCheck {
    let slot = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));
    slot.pseudonyms()
        .put_mapping(s, subject, handle(pseudonym, &s.tenant().0))
        .expect("seed mapping");
    slot
}

/// **The CONSUMER (Git/Audit): attribute an immutable artifact by the subject's PUBLIC pseudonym.**
/// Returns the rendered `<pseudonym>@<tenant>.noreply` to bake into a commit author — or fails if the
/// subject is erased (the consumer can no longer de-pseudonymise it). This is the canonical 4.8
/// consumer shape (Git pseudonymous-by-default commits, M3 P-ID-25, consume exactly this).
fn git_attribution_for(
    svc: &StoreBackedCheck,
    s: &TenantScope,
    subject: &PrincipalId,
) -> Result<String, PseudonymEraseError> {
    svc.resolve_pseudonym_in(s, subject).map(|h| h.render())
}

/// **The 4.8 happy path: a live subject attributes to its public pseudonym (DSR step-1 attribution).**
/// The Git consumer bakes `anon-7f3a@acme.noreply` into the commit author — PII-free, erasure-safe.
#[test]
fn cdc_4_8_live_subject_attributes_to_public_pseudonym() {
    let s = scope("acme");
    let alice = PrincipalId("p:alice".into());
    let svc = provider(&s, &alice, "anon-7f3a");
    assert_eq!(
        git_attribution_for(&svc, &s, &alice).expect("a live subject attributes"),
        "anon-7f3a@acme.noreply",
        "the consumer attributes by the public pseudonym (the frozen grammar)"
    );
}

/// **The 4.8 erase seam: the DSR orchestrator drives erase → the consumer can no longer
/// de-pseudonymise the subject (fails closed), but the public handle it ALREADY baked stands.** This
/// is the contract both sides must agree on: erase destroys the *resolution*, not the *attribution*.
#[test]
fn cdc_4_8_erase_makes_de_pseudonymisation_fail_closed_but_attribution_stands() {
    let s = scope("acme");
    let alice = PrincipalId("p:alice".into());
    let svc = provider(&s, &alice, "anon-7f3a");

    // The Git consumer already baked the public handle into an immutable commit (it stands forever).
    let baked = git_attribution_for(&svc, &s, &alice).expect("attribute before erase");
    assert_eq!(baked, "anon-7f3a@acme.noreply");

    // The DSR orchestrator drives erase (DSR step 1).
    let receipt = svc.erase_in(&s, &alice, ts("2026-06-19T12:00:00Z"));
    assert!(
        receipt.dek_destroyed && receipt.row_shredded,
        "the provider crypto-shredded the subject"
    );

    // The consumer can no longer de-pseudonymise the erased subject — resolve FAILS CLOSED.
    let r = git_attribution_for(&svc, &s, &alice);
    assert!(
        matches!(r, Err(PseudonymEraseError::Erased { .. })),
        "after erase, the consumer cannot de-pseudonymise the subject (fails closed): {r:?}"
    );
    // The provider durably recorded the erasure (so post-restore re-erasure can replay — the DSR
    // orchestrator relies on this for the ID-D8 restore path).
    assert!(
        svc.erasure_ledger().is_erased(&s, &alice),
        "the provider recorded the erasure in the PII-free ledger (10.8)"
    );
}

/// **The provider+consumer agree on the partition: an erase under one tenant does not change another
/// tenant's resolution.** The consumer attributing globex's subject is unaffected by acme's erase.
#[test]
fn cdc_4_8_erase_is_tenant_scoped_across_the_seam() {
    let acme = scope("acme");
    let globex = scope("globex");
    let alice = PrincipalId("p:alice".into());
    let svc = provider(&acme, &alice, "anon-a");
    svc.pseudonyms()
        .put_mapping(&globex, &alice, handle("anon-g", "globex"))
        .unwrap();

    svc.erase_in(&acme, &alice, ts("2026-06-19T12:00:00Z"));

    // acme: fails closed. globex: still attributes (the seam is tenant-scoped).
    assert!(matches!(
        git_attribution_for(&svc, &acme, &alice),
        Err(PseudonymEraseError::Erased { .. })
    ));
    assert_eq!(
        git_attribution_for(&svc, &globex, &alice).expect("globex attributes"),
        "anon-g@globex.noreply",
        "globex's identically-named subject is untouched by acme's erase"
    );
}
