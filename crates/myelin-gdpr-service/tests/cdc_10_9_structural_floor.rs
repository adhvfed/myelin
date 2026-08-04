use myelin_gdpr::{SubjectRef, TenantId};
use myelin_gdpr_service::{
    classify_residual, shred_pseudonym_identity, Authorship, CryptoShredKms, InMemoryShredKms,
    LeverCoverage, M1Store, Processed, Processing, RestrictRegistry, StoredContent,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

#[test]
fn restrict_is_honoured_by_an_m1_holder_suppressing_processing_retaining_storage() {
    let tenant = TenantId::from_token("acme");
    let subj = subject("u-cdc-restrict");
    let restrict = RestrictRegistry::new();
    let kms = InMemoryShredKms::new();
    let store = M1Store::new("search_index", &restrict, &kms);
    kms.provision(M1Store::dek_handle(&subj, &tenant), 1);
    store.store_self_authored(&subj, &tenant, "indexed body");

    assert!(matches!(
        store.index(&subj, &tenant),
        Processed::Processed(_)
    ));

    restrict.set(&subj, &tenant, true);
    for op in Processing::all() {
        let r = match op {
            Processing::Index => store.index(&subj, &tenant),
            Processing::AgentRead => store.agent_read(&subj, &tenant),
            Processing::Analyse => store.analyse(&subj, &tenant),
            Processing::Notify => store.notify(&subj, &tenant),
        };
        assert_eq!(
            r,
            Processed::Suppressed,
            "{op:?} suppressed for a restricted subject"
        );
    }
    assert_eq!(
        store.fetch_stored(&subj, &tenant),
        Some(StoredContent::Recoverable("indexed body".into())),
        "storage is RETAINED while restricted (§4.4)"
    );
    restrict.set(&subj, &tenant, false);
    assert!(matches!(
        store.index(&subj, &tenant),
        Processed::Processed(_)
    ));
}

#[test]
fn per_subject_dek_shred_renders_self_authored_free_text_unrecoverable() {
    let tenant = TenantId::from_token("acme");
    let subj = subject("u-cdc-erase");
    let restrict = RestrictRegistry::new();
    let kms = InMemoryShredKms::new();
    let store = M1Store::new("chat_store", &restrict, &kms);
    kms.provision(M1Store::dek_handle(&subj, &tenant), 5);
    store.store_self_authored(&subj, &tenant, "message body");

    assert_eq!(store.erase_self_authored(&subj, &tenant), Some(5));
    assert_eq!(
        store.fetch_stored(&subj, &tenant),
        Some(StoredContent::Unrecoverable),
        "the per-subject DEK shred renders the self-authored content unrecoverable (§7.1.1)"
    );
    assert_eq!(
        kms.recoverable_in_backup(&M1Store::dek_handle(&subj, &tenant)),
        0
    );
}

#[test]
fn pseudonym_map_shred_leaves_only_the_frozen_grammar() {
    let handle = PseudonymHandle::new("anon-42", "acme").expect("valid pseudonym");
    let shredded = shred_pseudonym_identity(&handle);
    assert_eq!(shredded.immutable_bytes, "anon-42@acme.noreply");
    assert!(shredded.holds_only_the_pseudonym_form());
}

#[test]
fn the_residual_is_restrict_suppress_only_not_crypto_shredded() {
    assert_eq!(
        classify_residual(Authorship::SelfAuthored),
        LeverCoverage::CryptoShred
    );
    assert_eq!(
        classify_residual(Authorship::ThirdPartyMention),
        LeverCoverage::RestrictSuppressOnly,
        "the third-party residual is governed ONLY by restrict (the documented limit), not the \
         subject's crypto-shred (§7.2)"
    );
}
