use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    holder_ids, CryptoShredKms, EraseChecklist, InMemoryShredKms, SeamHolder, ShredKeyClass,
    ShredKeyHandle, UpstreamHolderOrchestrator,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

#[test]
fn orchestrator_fans_erase_out_to_the_upstream_holders_in_canonical_order() {
    let tenant = TenantId::from_token("acme");
    let subj = subject("u-cdc-orch");

    let kms = InMemoryShredKms::new();
    let ids = [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ];
    for (i, id) in ids.iter().enumerate() {
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Subject((*id).to_string()),
            },
            10 + i as u64,
        );
    }

    let holders: Vec<(&'static str, SeamHolder)> = ids
        .iter()
        .map(|id| {
            (
                *id,
                SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), &kms),
            )
        })
        .collect();

    let orch = UpstreamHolderOrchestrator::register_m1_upstream(
        holders
            .iter()
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect(),
    );
    let checklist = EraseChecklist::new();
    let receipts = orch
        .fan_out_erase(
            &EraseScope::Subject {
                subject: subj.clone(),
                tenant: tenant.clone(),
            },
            &checklist,
        )
        .expect("the canonical fan-out succeeds");

    assert_eq!(
        receipts.len(),
        6,
        "the fan-out reached every M1 upstream holder"
    );
    assert_eq!(
        receipts[0].holder_id,
        holder_ids::IDENTITY,
        "Identity (pseudonym map) erased FIRST"
    );
    assert_eq!(
        receipts.last().unwrap().holder_id,
        holder_ids::BACKUP,
        "backups erased LAST"
    );

    for r in &receipts {
        assert_eq!(r.receipt.receipt.operation, "erase");
        assert!(r.receipt.receipt.content_hash.starts_with("blake3:"));
        assert!(r.receipt.receipt.key_epoch_destroyed.is_some());
    }

    assert_eq!(orch.fanout_coverage(&checklist), 1.0);

    for id in ids {
        let handle = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(id.to_string()),
        };
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            0,
            "{id}: 0 recoverable after erase"
        );
    }
}

#[test]
fn re_driving_the_fan_out_is_idempotent_for_the_consumer() {
    let tenant = TenantId::from_token("acme");
    let kms = InMemoryShredKms::new();
    let id = holder_ids::BLOB;
    kms.provision(
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(id.into()),
        },
        7,
    );
    let h = SeamHolder::new(id, ShredKeyClass::Subject(id.into()), &kms);
    let orch =
        UpstreamHolderOrchestrator::register_m1_upstream(vec![(id, &h as &dyn PersonalDataHolder)]);
    let checklist = EraseChecklist::new();
    let scope = EraseScope::Subject {
        subject: subject("u-idem-cdc"),
        tenant: tenant.clone(),
    };

    let first = orch.fan_out_erase(&scope, &checklist).unwrap();
    let second = orch.fan_out_erase(&scope, &checklist).unwrap();
    assert_eq!(
        first, second,
        "an idempotent re-drive returns the SAME receipts"
    );
    assert_eq!(
        h.erase_call_count(),
        1,
        "the already-receipted holder is NOT re-called"
    );
}
