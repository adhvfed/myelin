use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    restrict_holder_ids, DerivedProcessed, DerivedProcessing, DerivedStore, DerivedStoreHolder,
    RestrictFanOutDriver, RestrictRegistry,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn t(s: &str) -> TenantId {
    TenantId::from_token(s)
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        t("acme"),
    ))
}

#[test]
fn cdc_11_6_olap_honours_the_restriction_flag_no_analytics() {
    let tenant = t("acme");
    let subj = subject("u-cdc-11-6");
    let restrict = RestrictRegistry::new();
    let olap = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
    olap.seed_row(&subj, &tenant);
    assert_eq!(olap.holder_id(), restrict_holder_ids::OLAP_READ_STORE);

    let provider = DerivedStoreHolder::new(&olap);
    let consumer: &dyn PersonalDataHolder = &provider;

    assert!(matches!(
        olap.process(&subj, &tenant),
        DerivedProcessed::Processed(_)
    ));

    let set = consumer
        .restrict(&subj, true)
        .expect("OLAP restrict honours 11.6");
    assert_eq!(set.receipt.operation, "restrict");
    assert!(
        set.receipt.content_hash.starts_with("blake3:"),
        "11.6: the restrict receipt is content-addressed"
    );
    assert_eq!(
        olap.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "11.6: no analytics for a restricted subject"
    );
    assert!(
        olap.has_row(&subj, &tenant),
        "11.6: storage retained while restricted"
    );

    consumer
        .restrict(&subj, false)
        .expect("OLAP un-restrict honours 11.6");
    assert!(
        matches!(olap.process(&subj, &tenant), DerivedProcessed::Processed(_)),
        "11.6: analytics resume (reversible)"
    );
}

#[test]
fn cdc_10_1_search_refs_notif_agent_honour_restrict() {
    let tenant = t("acme");
    let subj = subject("u-cdc-10-1");
    let restrict = RestrictRegistry::new();

    for kind in [
        DerivedProcessing::SearchIndex,
        DerivedProcessing::RefsProject,
        DerivedProcessing::NotifNotify,
        DerivedProcessing::AgentRead,
    ] {
        let store = DerivedStore::new(kind, &restrict);
        store.seed_row(&subj, &tenant);
        let provider = DerivedStoreHolder::new(&store);
        let consumer: &dyn PersonalDataHolder = &provider;

        assert!(
            matches!(
                store.process(&subj, &tenant),
                DerivedProcessed::Processed(_)
            ),
            "{} processes before restriction",
            kind.holder_id()
        );
        consumer
            .restrict(&subj, true)
            .expect("derived restrict honours 10.1");
        assert_eq!(
            store.process(&subj, &tenant),
            DerivedProcessed::Suppressed,
            "{}: processing suppressed for the restricted subject (§4.4)",
            kind.holder_id()
        );
        assert!(
            store.has_row(&subj, &tenant),
            "{}: storage retained",
            kind.holder_id()
        );
        consumer
            .restrict(&subj, false)
            .expect("derived un-restrict honours 10.1");
        assert!(
            matches!(
                store.process(&subj, &tenant),
                DerivedProcessed::Processed(_)
            ),
            "{}: processing resumes (reversible)",
            kind.holder_id()
        );
    }
}

#[test]
fn cdc_restrict_fan_out_zero_processing_across_all_five() {
    let tenant = t("acme");
    let subj = subject("u-cdc-fan");
    let restrict = RestrictRegistry::new();
    let search = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
    let refs = DerivedStore::new(DerivedProcessing::RefsProject, &restrict);
    let notif = DerivedStore::new(DerivedProcessing::NotifNotify, &restrict);
    let agents = DerivedStore::new(DerivedProcessing::AgentRead, &restrict);
    let olap = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
    let stores: [&DerivedStore; 5] = [&search, &refs, &notif, &agents, &olap];
    for s in stores {
        s.seed_row(&subj, &tenant);
    }
    let sh = DerivedStoreHolder::new(&search);
    let rh = DerivedStoreHolder::new(&refs);
    let nh = DerivedStoreHolder::new(&notif);
    let ah = DerivedStoreHolder::new(&agents);
    let oh = DerivedStoreHolder::new(&olap);
    let holders: [&dyn PersonalDataHolder; 5] = [&sh, &rh, &nh, &ah, &oh];

    let set = RestrictFanOutDriver::fan_out_restrict(&subj, &tenant, true, &stores, &holders)
        .expect("the restrict fan-out honours 10.1 / 11.6");
    assert_eq!(
        set.processed_count(),
        0,
        "0 processing of a restricted subject across all five"
    );
    assert!(set.all_rows_retained(), "every derived row retained (§4.4)");
    assert_eq!(
        set.holder_receipts.len(),
        5,
        "one restrict receipt per derived store provider"
    );

    let clear = RestrictFanOutDriver::fan_out_restrict(&subj, &tenant, false, &stores, &holders)
        .expect("the un-restrict fan-out honours 10.1 / 11.6");
    assert!(!clear.all_suppressed(), "processing resumes (reversible)");
}
