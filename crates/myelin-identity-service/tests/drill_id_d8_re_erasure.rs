use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle, RevokeTarget};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

fn slot() -> StoreBackedCheck {
    StoreBackedCheck::new(TupleStore::new(OutboxStore::new()))
}

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

fn at(t: &str) -> Timestamp {
    Timestamp(t.into())
}

#[test]
fn id_d8_restore_resurrects_no_authority_and_emits_a_dated_re_erasure_receipt() {
    let slot = slot();
    let s = scope("acme");
    let alice = PrincipalId("p:alice".into());
    let bob = PrincipalId("p:bob".into());

    slot.pseudonyms()
        .put_mapping(&s, &alice, handle("anon-a", "acme"))
        .unwrap();
    slot.pseudonyms()
        .put_mapping(&s, &bob, handle("anon-b", "acme"))
        .unwrap();
    slot.erase_in(&s, &alice, at("2026-06-19T10:00:00Z"));
    slot.erase_in(&s, &bob, at("2026-06-19T10:00:01Z"));

    assert!(slot.pseudonyms().resolve_subject(&s, &alice).is_none());
    assert!(slot.pseudonyms().resolve_subject(&s, &bob).is_none());
    assert!(slot.erasure_ledger().is_erased(&s, &alice));
    assert!(slot.erasure_ledger().is_erased(&s, &bob));

    slot.pseudonyms()
        .put_mapping(&s, &alice, handle("anon-a", "acme"))
        .unwrap();
    slot.pseudonyms()
        .put_mapping(&s, &bob, handle("anon-b", "acme"))
        .unwrap();
    assert!(
        slot.pseudonyms().resolve_subject(&s, &alice).is_some(),
        "the restore RESURRECTED alice (resolvable again) - the bug ID-D8 catches"
    );
    assert!(
        slot.pseudonyms().resolve_subject(&s, &bob).is_some(),
        "the restore resurrected bob"
    );

    let receipt = slot
        .re_erase_after_restore(&s, at("2026-06-19T11:00:00Z"))
        .expect("re-erasure verification");

    assert_eq!(
        receipt.re_erased, 2,
        "the ledger drove re-erasure of BOTH recorded subjects"
    );
    assert_eq!(
        receipt.pre_pass_resurrected, 2,
        "the restore resurrected both (the honest 'what the backup brought back' signal)"
    );
    assert_eq!(
        receipt.resurrected, 0,
        "0 resurrected AFTER the re-erasure pass - the ID-D8 threshold (a restore resurrects nothing)"
    );
    assert!(receipt.is_green(), "the drill is GREEN: 0 resurrected");
    assert_eq!(receipt.ran_at, at("2026-06-19T11:00:00Z"), "dated");
    assert!(receipt.summary().contains("GREEN"));
    assert!(receipt.summary().contains("2026-06-19T11:00:00Z"));

    assert!(
        slot.pseudonyms().resolve_subject(&s, &alice).is_none(),
        "alice re-erased"
    );
    assert!(
        slot.pseudonyms().resolve_subject(&s, &bob).is_none(),
        "bob re-erased"
    );
    let nowt = at("2026-06-19T11:00:00Z");
    assert!(
        slot.revocations()
            .is_revoked(&s, &RevokeTarget::Principal(alice), &nowt),
        "alice's grants stay revoked (no resurrected authority)"
    );
    assert!(
        slot.revocations()
            .is_revoked(&s, &RevokeTarget::Principal(bob), &nowt),
        "bob's grants stay revoked"
    );

    assert_eq!(receipt.per_subject.len(), 2);
    for r in &receipt.per_subject {
        assert!(
            r.content_hash.starts_with("blake3:"),
            "content-addressed receipt"
        );
        assert_eq!(r.erased_at, at("2026-06-19T11:00:00Z"), "dated");
    }
}

#[test]
fn id_d8_empty_ledger_is_trivially_green() {
    let slot = slot();
    let s = scope("acme");
    let receipt = slot
        .re_erase_after_restore(&s, at("2026-06-19T11:00:00Z"))
        .expect("empty re-erasure verification");
    assert_eq!(receipt.re_erased, 0);
    assert_eq!(receipt.resurrected, 0);
    assert!(receipt.is_green());
}

#[test]
fn id_d8_without_re_erasure_a_restore_resurrects_the_subject() {
    let slot = slot();
    let s = scope("acme");
    let alice = PrincipalId("p:alice".into());
    slot.pseudonyms()
        .put_mapping(&s, &alice, handle("anon-a", "acme"))
        .unwrap();
    slot.erase_in(&s, &alice, at("2026-06-19T10:00:00Z"));
    slot.pseudonyms()
        .put_mapping(&s, &alice, handle("anon-a", "acme"))
        .unwrap();
    assert!(
        slot.pseudonyms().resolve_subject(&s, &alice).is_some(),
        "WITHOUT re-erasure, the restore resurrects the subject - the property re_erase_after_restore \
         exists to repair (a mutation skipping the pass leaves this resurrected, reding ID-D8)"
    );
}
