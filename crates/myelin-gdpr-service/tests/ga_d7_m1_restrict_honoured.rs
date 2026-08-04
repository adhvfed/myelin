use myelin_gdpr::{SubjectRef, TenantId};
use myelin_gdpr_service::{
    classify_residual, shred_pseudonym_identity, Authorship, CryptoShredKms, InMemoryShredKms,
    LeverCoverage, M1Store, Processed, Processing, RestrictRegistry, StoredContent,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};

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
fn the_structural_floor_is_proven_end_to_end_on_the_m1_stores() {
    let tenant = t("acme");
    let subj = subject("u-drill");

    let restrict = RestrictRegistry::new();
    let kms = InMemoryShredKms::new();
    let chat = M1Store::new("chat_store", &restrict, &kms);
    let issues = M1Store::new("issues_store", &restrict, &kms);
    let knowledge = M1Store::new("knowledge_store", &restrict, &kms);
    let m1_holders = [&chat, &issues, &knowledge];

    kms.provision(M1Store::dek_handle(&subj, &tenant), 11);
    chat.store_self_authored(&subj, &tenant, "my chat message");
    issues.store_self_authored(&subj, &tenant, "my issue comment");
    knowledge.store_self_authored(&subj, &tenant, "my doc block");

    for h in m1_holders {
        for op in Processing::all() {
            let r = run(h, op, &subj, &tenant);
            assert!(
                matches!(r, Processed::Processed(_)),
                "{}:{op:?} processes before restriction",
                h.id()
            );
        }
    }

    restrict.set(&subj, &tenant, true);
    assert!(restrict.is_restricted(&subj, &tenant));
    let mut suppressed_ops = 0u32;
    for h in m1_holders {
        for op in Processing::all() {
            assert_eq!(
                run(h, op, &subj, &tenant),
                Processed::Suppressed,
                "{}:{op:?} SUPPRESSED for the restricted subject (§4.4)",
                h.id()
            );
            suppressed_ops += 1;
        }
        assert!(
            matches!(
                h.fetch_stored(&subj, &tenant),
                Some(StoredContent::Recoverable(_))
            ),
            "{} retains storage while restricted",
            h.id()
        );
    }
    assert_eq!(suppressed_ops, 12, "3 holders × 4 §4.4 ops all suppressed");

    restrict.set(&subj, &tenant, false);
    for h in m1_holders {
        assert!(matches!(
            run(h, Processing::Index, &subj, &tenant),
            Processed::Processed(_)
        ));
    }

    let handle = PseudonymHandle::new("anon-drill", "acme").expect("valid pseudonym");
    let shredded = shred_pseudonym_identity(&handle);
    assert_eq!(shredded.immutable_bytes, "anon-drill@acme.noreply");
    assert!(
        shredded.holds_only_the_pseudonym_form(),
        "the immutable bytes hold ONLY <pseudonym>@<tenant>.noreply (no real PII) - §7.1.2"
    );

    let destroyed = chat.erase_self_authored(&subj, &tenant);
    assert_eq!(
        destroyed,
        Some(11),
        "the DEK shred records the destroyed epoch (the audit trail)"
    );
    let mut zero_recoverable_holders = 0u32;
    for h in m1_holders {
        assert_eq!(
            h.fetch_stored(&subj, &tenant),
            Some(StoredContent::Unrecoverable),
            "{} is UNRECOVERABLE after the DEK shred (one DEK seals every holder's content)",
            h.id()
        );
        zero_recoverable_holders += 1;
    }
    assert_eq!(
        zero_recoverable_holders, 3,
        "0 recoverable PII across all 3 M1 holders"
    );
    assert_eq!(
        kms.recoverable_in_backup(&M1Store::dek_handle(&subj, &tenant)),
        0,
        "0 recoverable in backup - the crypto-shred reaches backups by construction (§7.5)"
    );

    assert_eq!(
        classify_residual(Authorship::ThirdPartyMention),
        LeverCoverage::RestrictSuppressOnly,
        "the residual is restrict-suppress-only - the documented limit, not crypto-shredded (§7.2)"
    );
    assert_eq!(
        classify_residual(Authorship::SelfAuthored),
        LeverCoverage::CryptoShred
    );
}

fn run(h: &M1Store, op: Processing, subj: &SubjectRef, tenant: &TenantId) -> Processed {
    match op {
        Processing::Index => h.index(subj, tenant),
        Processing::AgentRead => h.agent_read(subj, tenant),
        Processing::Analyse => h.analyse(subj, tenant),
        Processing::Notify => h.notify(subj, tenant),
    }
}
