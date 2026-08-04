use myelin_gdpr::{EraseReceipt, EraseScope, PersonalDataHolder, Receipt, SubjectRef, TenantId};
use myelin_gdpr_service::{
    AuditCarveOutHolder, CryptoShredKms, GdprOwnStoreHolder, InMemoryShredKms, ShredKeyClass,
    ShredKeyHandle,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

struct DsrOrchestratorConsumer<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}

impl<'a> DsrOrchestratorConsumer<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrOrchestratorConsumer { holders }
    }

    fn fan_out_erase(&self, scope: EraseScope) -> Vec<EraseReceipt> {
        self.holders
            .iter()
            .map(|h| {
                h.erase(scope.clone())
                    .expect("a GDPR-owned holder erase succeeds")
            })
            .collect()
    }
}

#[test]
fn dsr_orchestrator_fans_erase_out_to_the_gdpr_owned_holders_via_the_contract() {
    let tenant = TenantId::from_token("acme");
    let subj = subject("u-cdc");

    let kms = InMemoryShredKms::new();
    kms.provision(
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subj.principal.principal_id.0.clone()),
        },
        11,
    );

    let h18 = GdprOwnStoreHolder::new(&kms);
    let h16 = AuditCarveOutHolder::new(&kms);

    let orchestrator = DsrOrchestratorConsumer::new(vec![&h18, &h16]);
    let receipts = orchestrator.fan_out_erase(EraseScope::Subject {
        subject: subj.clone(),
        tenant: tenant.clone(),
    });

    assert_eq!(
        receipts.len(),
        2,
        "the fan-out reached both GDPR-owned holders"
    );
    for r in &receipts {
        assert_eq!(r.receipt.operation, "erase");
        assert!(r.receipt.content_hash.starts_with("blake3:"));
    }

    let handle = ShredKeyHandle {
        tenant: tenant.clone(),
        class: ShredKeyClass::Subject(subj.principal.principal_id.0.clone()),
    };
    assert_eq!(
        kms.recoverable_in_backup(&handle),
        0,
        "H18 crypto-shred: 0 recoverable"
    );
    assert!(
        receipts
            .iter()
            .any(|r| r.receipt.key_epoch_destroyed == Some(11)),
        "the H18 erase receipt records the destroyed key epoch"
    );
}

#[test]
fn receipt_shape_is_the_frozen_provider_consumer_contract() {
    let r = Receipt::content_addressed(
        "erase",
        "gdpr_own_store",
        "u",
        "acme",
        "crypto_shred",
        Some(2),
        0,
    );
    assert_eq!(r.operation, "erase");
    assert_eq!(r.key_epoch_destroyed, Some(2));
    let back: Receipt = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back, r);
}
