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

fn provider(s: &TenantScope, subject: &PrincipalId, pseudonym: &str) -> StoreBackedCheck {
    let slot = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));
    slot.pseudonyms()
        .put_mapping(s, subject, handle(pseudonym, &s.tenant().0))
        .expect("seed mapping");
    slot
}

fn git_attribution_for(
    svc: &StoreBackedCheck,
    s: &TenantScope,
    subject: &PrincipalId,
) -> Result<String, PseudonymEraseError> {
    svc.resolve_pseudonym_in(s, subject).map(|h| h.render())
}

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

#[test]
fn cdc_4_8_erase_makes_de_pseudonymisation_fail_closed_but_attribution_stands() {
    let s = scope("acme");
    let alice = PrincipalId("p:alice".into());
    let svc = provider(&s, &alice, "anon-7f3a");

    let baked = git_attribution_for(&svc, &s, &alice).expect("attribute before erase");
    assert_eq!(baked, "anon-7f3a@acme.noreply");

    let receipt = svc
        .erase_in(&s, &alice, ts("2026-06-19T12:00:00Z"))
        .unwrap();
    assert!(
        receipt.dek_destroyed && receipt.row_shredded,
        "the provider crypto-shredded the subject"
    );

    let r = git_attribution_for(&svc, &s, &alice);
    assert!(
        matches!(r, Err(PseudonymEraseError::Erased { .. })),
        "after erase, the consumer cannot de-pseudonymise the subject (fails closed): {r:?}"
    );
    assert!(
        svc.erasure_ledger()
            .is_erased(&s, &alice)
            .expect("the erasure ledger remains readable"),
        "the provider recorded the erasure in the PII-free ledger (10.8)"
    );
}

#[test]
fn cdc_4_8_erase_is_tenant_scoped_across_the_seam() {
    let acme = scope("acme");
    let globex = scope("globex");
    let alice = PrincipalId("p:alice".into());
    let svc = provider(&acme, &alice, "anon-a");
    svc.pseudonyms()
        .put_mapping(&globex, &alice, handle("anon-g", "globex"))
        .unwrap();

    svc.erase_in(&acme, &alice, ts("2026-06-19T12:00:00Z"))
        .unwrap();

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
