use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::humanise::{
    humanise, Channel as HumaniseChannel, RefProjection, RefResolution, RefResolvePort, Tombstone,
    TombstoneReason, DEFAULT_LOCALE,
};
use myelin_notif::prefs::Channel;
use myelin_notif::{
    build_idem_key, erase_residual, reason_template_key, redact_for_offcell, Class,
    EuSovereignAdapter, HumanisedString, InMemoryDeliveryShredder, InlineDeliveryShredder,
    NotifErasureLedger, OffCellResidual, Reason, RecordingEuTransport, RestrictSet, TemplateStore,
    OPEN_LEGAL_PROVIDER_DPA,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::{Arc, Mutex};

use myelin_identity::{Consistency, ConsistencyMode, Zookie};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

const SUBJECT_ID: &str = "u-erase";

fn subject_actor_ref() -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/identity/principal/{SUBJECT_ID}"))
}

struct ErasingResolver {
    erased_refs: Mutex<Vec<String>>,
}
impl ErasingResolver {
    fn new() -> ErasingResolver {
        ErasingResolver {
            erased_refs: Mutex::new(Vec::new()),
        }
    }
    fn mark_erased(&self, r: &ArtifactRef) {
        self.erased_refs.lock().unwrap().push(r.0.clone());
    }
}
impl RefResolvePort for ErasingResolver {
    fn resolve_display(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
        _viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if self
            .erased_refs
            .lock()
            .unwrap()
            .iter()
            .any(|x| x == &ref_.0)
        {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Erased,
            });
        }
        RefResolution::Projection(RefProjection {
            ref_: ref_.clone(),
            title: "issue PROJ-1".into(),
            icon: "issue".into(),
        })
    }
}

fn redacted_summary() -> HumanisedString {
    HumanisedString {
        text: "you were mentioned by a teammate".into(),
        links: vec!["myelin://acme/issues/issue/PROJ-1".into()],
        icon: "mention".into(),
    }
}

#[test]
fn notif_d6_erase_user_zero_recoverable_pii_and_every_appearance_tombstones() {
    let resolver = ErasingResolver::new();
    resolver.mark_erased(&subject_actor_ref());
    let templates = TemplateStore::with_platform_defaults();

    let appearances: &[(Reason, &str)] = &[
        (Reason::Mentioned, "itm-1"),
        (Reason::Replied, "itm-2"),
        (Reason::Assigned, "itm-3"),
    ];
    let mut tombstoned = 0u64;
    for (reason, _item) in appearances {
        let key = reason_template_key(*reason);
        let h = humanise(
            &resolver,
            &tenant(),
            &region(),
            &templates,
            key,
            std::slice::from_ref(&subject_actor_ref()),
            &viewer("u-bob"),
            DEFAULT_LOCALE,
            &strong("z1"),
            HumaniseChannel::Cli,
        );
        assert!(
            h.text.contains("[erased user]"),
            "every appearance of the erased subject humanises to [erased user] (reason={key}): got {:?}",
            h.text
        );
        assert!(
            h.links.is_empty(),
            "an erased subject yields no link (reason={key})"
        );
        tombstoned += 1;
    }
    assert_eq!(
        tombstoned,
        appearances.len() as u64,
        "EVERY inbox appearance of the erased subject tombstones to [erased user] (0 missed)"
    );

    let transport = RecordingEuTransport::new("eu-mailer");
    let provider = EuSovereignAdapter::new(Channel::Email, region(), Arc::new(transport.clone()));
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    let idem = build_idem_key("itm-offcell", Channel::Email);
    let _msg = redact_for_offcell(redacted_summary(), Class::Direct);
    provider
        .try_send(
            &redact_for_offcell(redacted_summary(), Class::Direct),
            &idem,
        )
        .expect("the off-cell redacted summary is delivered (EU region)");
    let provider_ref = provider
        .provider_ref_for(&idem)
        .expect("the off-cell copy has a durable provider_ref");

    let dek = myelin_events::PiiKeyRef(format!("kms://acme/epoch-1/subject:{SUBJECT_ID}"));
    shredder.seal(&dek);
    assert!(
        shredder.is_live(&dek),
        "the inline-PII delivery column is RECOVERABLE before erase"
    );

    let residuals = vec![OffCellResidual {
        idem_key: idem.clone(),
        inline_pii_key: Some(dek.clone()),
    }];

    let receipt = erase_residual(
        SUBJECT_ID,
        &tenant(),
        &residuals,
        &shredder,
        &restrict,
        &provider,
        &ledger,
        myelin_events::Timestamp("2026-06-25T00:00:00Z".into()),
    )
    .expect("the structural erase succeeds (the X-7 posture instanced)");

    assert_eq!(
        receipt.recoverable_remaining, 0,
        "NOTIF-D6: 0 inline-PII delivery columns recoverable (the gate threshold - never softened)"
    );
    assert!(
        receipt.is_green(),
        "NOTIF-D6 GREEN: 0 recoverable PII + restrict suppression applied"
    );
    assert!(
        !shredder.is_live(&dek),
        "the off-cell payload's inline-PII DEK is crypto-shredded (unrecoverable in live + backups)"
    );
    assert!(
        transport.was_erased(&provider_ref),
        "the already-sent off-cell copy was provider-side erasure-requested (sub-processor purge)"
    );
    assert!(
        ledger.is_erased(SUBJECT_ID),
        "the erase-receipt is sealed in the erasure ledger (10.8) - provable + survives a restore"
    );
    assert!(
        restrict.is_restricted(SUBJECT_ID),
        "the subject's NEW routing/delivery is suppressed (restrict, 10.1)"
    );

    let open_legal = OPEN_LEGAL_PROVIDER_DPA;
    assert!(
        !open_legal.resolved,
        "the residual lawful-basis statement (10.9) is flagged OPEN for counsel - the structural floor ships regardless"
    );

    eprintln!(
        "NOTIF-D6 GREEN (2026-06-25): erased subject {SUBJECT_ID} - {tombstoned} inbox appearances → \
         [erased user]; inline-PII recoverable = {} (threshold 0); off-cell DEK shredded; provider-side \
         erasure requested for {provider_ref}; erase-receipt sealed in the ledger. [OPEN - LEGAL] \
         residual (10.9) flagged for counsel.",
        receipt.recoverable_remaining
    );
}
