//! # P-GA-30 → P-333 — The Issues (H3) + Chat (H5) consumer-holder erasure GATE drills
//! (ISS-D11 / CHAT-D8)
//!
//! **DATED GREEN ARTIFACT (2026-06-22).** These chained drills are the dated green artifacts the
//! P-GA-30 GATE requires (the GDPR prompts record their drill artifacts as the test itself — there is
//! no GDPR scorecard binary yet). They prove, end-to-end over the Issues + Chat consumer holders and
//! the per-derivative cascade, the GATE rows:
//!
//! **ISS-D11 (SCHED, GDPR-anchored) — erase → PII gone from issue row (per-subject DEK), change-log,
//! comments, attachments, OLAP (+restriction), Search (incl. embeddings), Refs; post-restore
//! re-erasure catches a restore; third-party residual is the `[OPEN — LEGAL]` limit. 0 recoverable
//! PII.** The drill fans a DSR erase through the orchestrator to the Issues holder (H3) + the
//! shipped Search/Refs derivatives and proves: the per-subject Issues free-text DEK is destroyed (live
//! AND backups), the subject is OLAP-suppressed (contract 11.6), Search embeddings are purged
//! (0 re-identification), Refs are tombstoned (0 recoverable, no resolve-500), the issue topology
//! survives, and a BACKUP RESTORE that re-introduces the subject's DEK is caught by a post-restore
//! re-erasure from the ledger (the restore resurrects nothing).
//!
//! **CHAT-D8 (SCHED, GDPR-anchored) — erase → bodies crypto-shred in hot+cold segments+backups;
//! mentions → `[erased user]`; read-state/drafts/unfurl-cache purged; Search/Refs/Notif cascade.
//! 0 recoverable PII.** The drill fans a DSR erase through the orchestrator to the Chat holder (H5) +
//! the shipped Search/Refs/Notif derivatives and proves the hot+cold body shred, the read-state purge,
//! the `[erased user]` humanise, and the Search/Refs cascade.
//!
//! The telemetry: the holder receipts + the re-erasure receipt are the green artifacts; the data-map
//! diff surfaces H3/H5 (no drift).
//!
//! ## What this PROVES vs what it REUSES (EI-01 §7 coherence — no new core module)
//! This file ADDS NO production code — it is a pure **chained drill** over the
//! `myelin_gdpr_service::issues_chat_instance` machinery (the faithful Issues/Chat holders + their
//! `PersonalDataHolder` seam + the `IssuesChatCascadeDriver` + the shipped `derivative_erasure`
//! holders + the `ErasureLedger`, all in the library). H3/H5 register through the SAME
//! `RegisteredHolder` seam the upstream orchestration uses (P-GA-06).
//!
//! ## Floors named (deferred → filling prompt)
//! - The **worklog/productivity Behavioural classification (OQ-H)** + the works-council trigger + the
//!   SpecialCategory→DPIA route → **P-GA-31 → P-334**. After it all H1–H18 holders exist (the GA-D1
//!   precondition, M5, P-GA-32).
//! - The **third-party / immutable residual** (free-text authored by OTHERS under the author's DEK) is
//!   the ONE platform-posture residual (10.9), `[OPEN — LEGAL]`.
//! - The **live Issues / Chat `erase` bindings** behind the seam are a config swap at boot; the
//!   per-subject DEK mechanism is `myelin-storage`'s, its OWN live-stack integration proof owned
//!   storage-side; the derivative purge/tombstone/humanise live-stack proofs are owned by
//!   Search/Refs/Notif — **no `--features integration` leg owed** here.

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

/// **ISS-D11 — the Issues per-subject DEK shred + the full per-derivative cascade + post-restore
/// re-erasure. 0 recoverable PII.** Driven end-to-end through the cascade driver.
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

    // ── H3 is in the data map (no holder-without-map drift). ──
    let inv = data_map(&issues_chat_holder_schemas(Region("fr-par".into())));
    assert!(inv.holders.contains("oltp:issue_oltp"), "H3 in the map");
    assert!(
        inv.coverage_gaps(&issues_chat_registrations()).is_empty(),
        "the registered Issues/Chat holders are in the map — 0 holders missed"
    );

    // ── The cascade fan-out: primary per-subject DEK shred + OLAP/Search/Refs. ──
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

    // 0 recoverable PII across every leg.
    assert!(
        receipt.primary_shredded,
        "issue-row/change-log/comment free-text shredded"
    );
    assert_eq!(
        kms.recoverable_in_backup(&subject_dek("u-erase")),
        0,
        "0 recoverable in backups (crypto-shred reaches backups — ISS-D11)"
    );
    assert!(receipt.olap_suppressed, "OLAP honours restriction (11.6)");
    assert!(
        receipt.embeddings_purged,
        "Search embeddings purged, not hidden"
    );
    assert!(receipt.refs_tombstoned, "Refs tombstoned, no resolve-500");
    assert!(receipt.structure_survives, "the issue topology survives");
    // The per-subject reach: a different subject survives untouched.
    assert!(
        kms.is_present(&subject_dek("u-keep")),
        "a different subject survives"
    );
    assert_eq!(search.reidentify_hits("u-erase"), 0);
    assert_eq!(refs.recoverable_edges("u-erase"), 0);

    // The DSR receipt is the green artifact (content-addressed, records the destroyed key epoch).
    let primary = &receipt.holder_receipts[0].receipt;
    assert!(primary.content_hash.starts_with("blake3:"));
    let destroyed_epoch = primary.key_epoch_destroyed;
    assert!(
        destroyed_epoch.is_some(),
        "the per-subject-DEK shred is recorded"
    );

    // ── Post-restore re-erasure: a backup RESTORE re-introduces the subject's DEK; the ledger drives a
    //    re-erasure that catches it (the restore resurrects nothing). ──
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
        /* completed_at_offset */ 1_000,
        /* completed_at_secs */ 42,
    );
    // A restore lands at PIT 500 (BEFORE the erasure at 1000) — the ledger flags it for re-erasure.
    let to_reerase = ledger.post_pit_records_after(500);
    assert!(
        to_reerase.iter().any(|r| r.subject == "u-erase"),
        "the post-PIT ledger flags the erased subject for re-erasure (the restore resurrects it)"
    );
    // The restore re-introduced the subject's DEK (ciphertext key resurrected by the backup).
    kms.provision(subject_dek("u-erase"), 999);
    assert!(
        kms.is_present(&subject_dek("u-erase")),
        "the restore resurrected the DEK"
    );
    // Re-erase from the ledger record: the DEK is destroyed AGAIN (0 recoverable post-restore).
    ih.erase(subject_scope("u-erase")).unwrap();
    assert!(
        !kms.is_present(&subject_dek("u-erase")),
        "the post-restore re-erasure destroys the resurrected DEK — the restore resurrects nothing"
    );
    assert_eq!(kms.recoverable_in_backup(&subject_dek("u-erase")), 0);
}

/// **CHAT-D8 — the Chat per-subject body DEK shred (hot+cold+backups) + read-state purge + the
/// Search/Refs/Notif cascade (mentions → `[erased user]`). 0 recoverable PII.**
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

    // ── H5 is in the data map. ──
    let inv = data_map(&issues_chat_holder_schemas(Region("fr-par".into())));
    assert!(inv.holders.contains("oltp:chat_oltp"), "H5 in the map");

    // ── The cascade fan-out: primary body DEK shred (hot+cold) + read-state purge + Search/Refs/Notif. ──
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

    // 0 recoverable PII: the per-subject body DEK is destroyed in hot AND cold AND backups.
    assert!(receipt.bodies_shredded, "the message-body DEK is shredded");
    assert_eq!(
        kms.recoverable_in_backup(&subject_dek("u-chat")),
        0,
        "0 recoverable in backups — hot AND cold AND backups (CHAT-D8)"
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

    // The primary receipt names the hot+cold body DEK reach (the content-address pins the outcome).
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

/// **The Issues per-tenant FALLBACK leg — a tenant offboarding destroys the per-tenant Issues DEK
/// fallback (the non-isolable interleaved residual goes with the tenant).**
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
