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
fn restrict_is_honoured_end_to_end_across_the_five_derived_stores() {
    let tenant = t("acme");
    let subj = subject("u-d7");

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

    for s in stores {
        assert!(
            matches!(s.process(&subj, &tenant), DerivedProcessed::Processed(_)),
            "{} processes the subject before restriction",
            s.holder_id()
        );
    }

    let sh = DerivedStoreHolder::new(&search);
    let rh = DerivedStoreHolder::new(&refs);
    let nh = DerivedStoreHolder::new(&notif);
    let ah = DerivedStoreHolder::new(&agents);
    let oh = DerivedStoreHolder::new(&olap);
    let holders: [&dyn PersonalDataHolder; 5] = [&sh, &rh, &nh, &ah, &oh];

    let set = RestrictFanOutDriver::fan_out_restrict(&subj, &tenant, true, &stores, &holders)
        .expect("the restrict fan-out succeeds");

    assert_eq!(
        set.processed_count(),
        0,
        "0 processing of a restricted subject across Search/Refs/Notif/Agents/OLAP (GA-D7)"
    );
    assert!(
        set.all_suppressed(),
        "every derived store SUPPRESSED the restricted subject"
    );
    assert_eq!(
        set.verdicts.len(),
        5,
        "Search + Refs + Notif + Agents + OLAP"
    );

    assert_eq!(
        search.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "Search: no indexing"
    );
    assert_eq!(
        refs.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "Refs: no edge projection"
    );
    assert_eq!(
        notif.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "Notif: no notification"
    );
    assert_eq!(
        agents.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "Agents: no agent-use"
    );
    assert_eq!(
        olap.process(&subj, &tenant),
        DerivedProcessed::Suppressed,
        "OLAP: no analytics (11.6)"
    );

    assert!(
        set.all_rows_retained(),
        "every derived row RETAINED while restricted (§4.4)"
    );
    for s in stores {
        assert!(
            s.has_row(&subj, &tenant),
            "{} retains its derived row while restricted",
            s.holder_id()
        );
    }

    assert!(
        set.verdicts
            .iter()
            .any(|v| v.holder_id == restrict_holder_ids::OLAP_READ_STORE
                && matches!(v.outcome, DerivedProcessed::Suppressed)),
        "OLAP honours the restriction flag - no analytics for a restricted subject (11.6)"
    );

    let clear = RestrictFanOutDriver::fan_out_restrict(&subj, &tenant, false, &stores, &holders)
        .expect("the un-restrict fan-out succeeds");
    assert!(
        !clear.all_suppressed(),
        "processing resumes after the restriction is lifted (reversible)"
    );
    let processed_again = clear
        .verdicts
        .iter()
        .filter(|v| matches!(v.outcome, DerivedProcessed::Processed(_)))
        .count();
    assert_eq!(
        processed_again, 5,
        "all five derived stores process again (reversible)"
    );
    for s in stores {
        assert!(
            matches!(s.process(&subj, &tenant), DerivedProcessed::Processed(_)),
            "{} processes again after un-restrict",
            s.holder_id()
        );
    }
}

#[test]
fn the_restriction_is_scoped_other_subjects_still_process_across_the_derived_stores() {
    let tenant = t("acme");
    let restricted = subject("u-restricted");
    let other = subject("u-other");

    let restrict = RestrictRegistry::new();
    let search = DerivedStore::new(DerivedProcessing::SearchIndex, &restrict);
    let olap = DerivedStore::new(DerivedProcessing::OlapAnalyse, &restrict);
    for s in [&search, &olap] {
        s.seed_row(&restricted, &tenant);
        s.seed_row(&other, &tenant);
    }

    restrict.set(&restricted, &tenant, true);

    assert_eq!(
        search.process(&restricted, &tenant),
        DerivedProcessed::Suppressed
    );
    assert_eq!(
        olap.process(&restricted, &tenant),
        DerivedProcessed::Suppressed
    );
    assert!(matches!(
        search.process(&other, &tenant),
        DerivedProcessed::Processed(_)
    ));
    assert!(matches!(
        olap.process(&other, &tenant),
        DerivedProcessed::Processed(_)
    ));
}
