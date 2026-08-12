use myelin_gdpr::{EraseScope, PersonalDataHolder, Receipt, SubjectRef, TenantId};
use myelin_gdpr_service::{
    data_map, issues_chat_holder_schemas, issues_chat_registrations, ChatStoreHolder,
    ChatStoreModel, CryptoShredKms, DestroyedKeyEpoch, DsrId, ErasureLedger, InMemoryShredKms,
    IssuesChatCascadeDriver, IssuesStoreHolder, IssuesStoreModel, NotifHistoryHolder,
    NotifHistoryModel, RefsGraphHolder, RefsGraphModel, SearchIndexHolder, SearchIndexModel,
    ShredKeyClass, ShredKeyHandle, CHAT_DB, ERASED_USER, ISSUES_DB,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::Region;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

fn subject_scope(id: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject(id),
        tenant: tenant(),
    }
}

fn subject_dek(id: &str) -> ShredKeyHandle {
    ShredKeyHandle {
        tenant: tenant(),
        class: ShredKeyClass::Subject(id.into()),
    }
}

#[test]
fn iss_d11_erase_fans_to_issues_plus_cascade_zero_recoverable_post_restore_reerase() {
    let kms = InMemoryShredKms::new();
    kms.provision(subject_dek("u-erase"), 200);
    kms.provision(subject_dek("u-keep"), 201);

    let issues = IssuesStoreModel::new();
    issues.index_topology_from_source("u-erase");
    issues.index_topology_from_source("u-keep");
    let search = SearchIndexModel::new();
    let refs = RefsGraphModel::new();
    search.index_from_source("u-erase", "alice@example.com");
    refs.add_edge_from_source("u-erase", "issue:42");

    let ih = IssuesStoreHolder::new(&issues, &kms);
    let sh = SearchIndexHolder::new(&search);
    let rh = RefsGraphHolder::new(&refs);

    let inv = data_map(&issues_chat_holder_schemas(Region("fr-par".into())));
    assert!(inv.holders.contains("oltp:issue_oltp"), "H3 in the map");
    assert!(
        inv.coverage_gaps(&issues_chat_registrations()).is_empty(),
        "the registered Issues/Chat holders are in the map - 0 holders missed"
    );

    let receipt = IssuesChatCascadeDriver::fan_out_issue_erase(
        &subject_scope("u-erase"),
        &issues,
        &ih,
        &search,
        &sh,
        &refs,
        &rh,
        &kms,
    )
    .unwrap();

    assert!(
        receipt.primary_shredded,
        "issue-row/change-log/comment free-text shredded"
    );
    assert_eq!(
        kms.recoverable_in_backup(&subject_dek("u-erase")),
        0,
        "0 recoverable in backups (crypto-shred reaches backups - ISS-D11)"
    );
    assert!(receipt.olap_suppressed, "OLAP honours restriction (11.6)");
    assert!(
        receipt.embeddings_purged,
        "Search embeddings purged, not hidden"
    );
    assert!(receipt.refs_tombstoned, "Refs tombstoned, no resolve-500");
    assert!(receipt.structure_survives, "the issue topology survives");
    assert!(
        kms.is_present(&subject_dek("u-keep")),
        "a different subject survives"
    );
    assert_eq!(search.reidentify_hits("u-erase"), 0);
    assert_eq!(refs.recoverable_edges("u-erase"), 0);

    let primary = &receipt.holder_receipts[0].receipt;
    assert!(primary.content_hash.starts_with("blake3:"));
    let destroyed_epoch = primary.key_epoch_destroyed;
    assert!(
        destroyed_epoch.is_some(),
        "the per-subject-DEK shred is recorded"
    );

    let ledger = ErasureLedger::new();
    ledger.record_completion(
        DsrId("dsr:iss".into()),
        "u-erase".into(),
        "acme".into(),
        vec![ISSUES_DB.into()],
        vec![DestroyedKeyEpoch {
            holder_id: ISSUES_DB.into(),
            key_epoch_destroyed: destroyed_epoch,
        }],
        1_000,
        42,
    );
    let to_reerase = ledger.post_pit_records_after(500);
    assert!(
        to_reerase.iter().any(|r| r.subject == "u-erase"),
        "the post-PIT ledger flags the erased subject for re-erasure (the restore resurrects it)"
    );
    kms.provision(subject_dek("u-erase"), 999);
    assert!(
        kms.is_present(&subject_dek("u-erase")),
        "the restore resurrected the DEK"
    );
    ih.erase(subject_scope("u-erase")).unwrap();
    assert!(
        !kms.is_present(&subject_dek("u-erase")),
        "the post-restore re-erasure destroys the resurrected DEK - the restore resurrects nothing"
    );
    assert_eq!(kms.recoverable_in_backup(&subject_dek("u-erase")), 0);
}

#[test]
fn chat_d8_erase_fans_to_chat_hot_cold_plus_cascade_zero_recoverable() {
    let kms = InMemoryShredKms::new();
    kms.provision(subject_dek("u-chat"), 300);

    let chat = ChatStoreModel::new();
    chat.index_from_source("u-chat");
    let search = SearchIndexModel::new();
    let refs = RefsGraphModel::new();
    let notif = NotifHistoryModel::new();
    search.index_from_source("u-chat", "bob's message body");
    refs.add_edge_from_source("u-chat", "msg:7");
    notif.add_item_from_source("inbox-x", "u-chat");

    let ch = ChatStoreHolder::new(&chat, &kms);
    let sh = SearchIndexHolder::new(&search);
    let rh = RefsGraphHolder::new(&refs);
    let nh = NotifHistoryHolder::new(&notif);

    let inv = data_map(&issues_chat_holder_schemas(Region("fr-par".into())));
    assert!(inv.holders.contains("oltp:chat_oltp"), "H5 in the map");

    let receipt = IssuesChatCascadeDriver::fan_out_chat_erase(
        &subject_scope("u-chat"),
        &chat,
        &ch,
        &search,
        &sh,
        &refs,
        &rh,
        &notif,
        &nh,
        &kms,
    )
    .unwrap();

    assert!(receipt.bodies_shredded, "the message-body DEK is shredded");
    assert_eq!(
        kms.recoverable_in_backup(&subject_dek("u-chat")),
        0,
        "0 recoverable in backups - hot AND cold AND backups (CHAT-D8)"
    );
    assert!(
        receipt.read_state_purged,
        "read-state/drafts/unfurl-cache purged"
    );
    assert!(
        receipt.notif_humanised,
        "mentions humanise to [erased user]"
    );
    assert!(receipt.embeddings_purged, "Search embeddings purged");
    assert!(receipt.refs_tombstoned, "Refs tombstoned");
    assert!(receipt.structure_survives, "the channel topology survives");
    assert_eq!(
        notif.render_mention("inbox-x").as_deref(),
        Some(ERASED_USER)
    );

    let primary = &receipt.holder_receipts[0].receipt;
    let expected = Receipt::content_addressed(
        "erase",
        CHAT_DB,
        "u-chat",
        "acme",
        "crypto_shred:per_subject_chat_body_dek:hot_and_cold;read_state_purged;structure_survives",
        primary.key_epoch_destroyed,
        0,
    );
    assert_eq!(
        primary.content_hash, expected.content_hash,
        "the receipt names the per-subject hot+cold body DEK reach"
    );
}

#[test]
fn iss_d11_tenant_offboarding_destroys_the_per_tenant_fallback() {
    let kms = InMemoryShredKms::new();
    kms.provision(subject_dek("u-iso"), 400);
    kms.provision(
        ShredKeyHandle {
            tenant: tenant(),
            class: ShredKeyClass::Tenant,
        },
        401,
    );
    let issues = IssuesStoreModel::new();
    let ih = IssuesStoreHolder::new(&issues, &kms);

    let receipt = ih.erase(EraseScope::Tenant(tenant())).unwrap();
    assert!(
        !kms.is_present(&ShredKeyHandle {
            tenant: tenant(),
            class: ShredKeyClass::Tenant,
        }),
        "a tenant offboarding destroys the per-tenant Issues DEK fallback"
    );
    let expected = Receipt::content_addressed(
        "erase",
        ISSUES_DB,
        "*tenant*",
        "acme",
        "crypto_shred:per_tenant_issues_dek_fallback:tenant_offboard;structure_survives",
        receipt.receipt.key_epoch_destroyed,
        0,
    );
    assert_eq!(receipt.receipt.content_hash, expected.content_hash);
}
