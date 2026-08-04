use myelin_chat::{
    agent_may_read, analytics_eligible, encrypt_body, index_projection_if_allowed,
    is_body_unrecoverable, notif_may_route, paragraph_body, render_mention, AuthorKind,
    ChatErasureCascade, ChatFreeText, ChatHolder, ConversationId, Draft, DraftKey, DraftStore,
    MemDraftStore, MemHotTier, MentionRender, MentionResolver, MessageId, MessageState,
    MessageStore, NewMessage, RangeCursor, ReadMarker, ReadStateRecord, RestrictionGate,
    TombstoneReason, UnfurlCache, CHAT_ERASE_CASCADE_TOKEN, ERASED_USER,
};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, OutboxTransaction, Timestamp,
};
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::{EncryptedColumn, SubjectId};
use myelin_storage::kms::{DekId, KeyClass, KmsEngine};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeSet;
use std::sync::Arc;

const SUBJECT: &str = "psn:ada";

fn region() -> Region {
    Region::new("fr-par")
}
fn tenant() -> TenantId {
    TenantId::from_token("acme")
}
fn gdpr_tenant() -> GdprTenantId {
    GdprTenantId::from_token("acme")
}
fn conv() -> ConversationId {
    ConversationId::new("acme", "fr-par", "c-1")
}
fn subject() -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(SUBJECT.into()),
        PrincipalKind::Human,
        gdpr_tenant(),
    ))
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn begin(outbox: &OutboxStore, minter: &Arc<MonotonicMinter>) -> OutboxTransaction {
    outbox.begin(minter.clone(), ctx_base())
}

fn seal_body(kms: &KmsEngine, plaintext: &[u8]) -> EncryptedColumn {
    encrypt_body(
        kms,
        &region(),
        &tenant(),
        &SubjectId::new(SUBJECT),
        ChatFreeText::BodyInline,
        plaintext,
    )
    .expect("seal under the subject's per-subject DEK")
}

fn append(
    store: &MemHotTier,
    outbox: &OutboxStore,
    minter: &Arc<MonotonicMinter>,
    nonce: &str,
) -> MessageId {
    let mut tx = begin(outbox, minter);
    let id = store
        .append(
            &mut tx,
            NewMessage {
                conv: conv(),
                thread_root_id: None,
                author: SUBJECT.to_string(),
                author_kind: AuthorKind::Human,
                body_inline: b"private body".to_vec(),
                body_nodes: Vec::new(),
                client_nonce: nonce.to_string(),
            },
        )
        .expect("append");
    tx.commit().expect("commit");
    id
}

#[test]
fn chat_d8_chained_erase_zero_recoverable_pii_complete_receipts() {
    let kms = KmsEngine::new();
    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();

    let m1 = append(&store, &outbox, &minter, "n1");
    let m2 = append(&store, &outbox, &minter, "n2");
    let m3 = append(&store, &outbox, &minter, "n3");
    let sealed = store.seal_before(&conv(), &m3).expect("seal cold");
    assert_eq!(sealed, 2, "m1 + m2 sealed to the cold segment");

    let hot_body = seal_body(&kms, b"my private chat in the hot tier");
    let cold_body = seal_body(&kms, b"my private chat archived in the cold segment");
    let backup_before = kms.backup_snapshot();
    let dek_id = DekId::new(tenant(), KeyClass::Subject(SUBJECT.into()));
    assert!(
        backup_before.iter().any(|(id, _)| id == &dek_id),
        "the backup carries the subject's per-subject DEK BEFORE the erase"
    );
    assert!(!is_body_unrecoverable(&kms, &region(), &hot_body));
    assert!(!is_body_unrecoverable(&kms, &region(), &cold_body));

    read_state.upsert(&ReadMarker::new(conv(), SUBJECT, m1.clone()));
    drafts.save(
        &DraftKey::new(conv().conversation_id, SUBJECT),
        &Draft::text("an unsent private draft"),
    );

    let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
    let mut tx = begin(&outbox, &minter);
    let report = cascade.erase(
        &mut tx,
        &EraseScope::Subject {
            subject: subject(),
            tenant: gdpr_tenant(),
        },
        &[
            (conv(), m1.clone()),
            (conv(), m2.clone()),
            (conv(), m3.clone()),
        ],
    );
    tx.commit().expect("commit");

    assert!(
        is_body_unrecoverable(&kms, &region(), &hot_body),
        "HOT: 0 recoverable - the body is unrecoverable after the DEK shred"
    );
    assert!(
        is_body_unrecoverable(&kms, &region(), &cold_body),
        "COLD: 0 recoverable - the archived cold-segment body is unrecoverable too"
    );
    let backup_after = kms.backup_snapshot();
    assert!(
        !backup_after.iter().any(|(id, _)| id == &dek_id),
        "BACKUP: the crypto-shredded DEK is excluded - a restore cannot resurrect the body"
    );

    assert_eq!(report.records_tombstoned, 3, "all three records addressed");
    let rows = store
        .range(&conv(), RangeCursor::Recent, 10)
        .expect("range across hot + cold");
    assert_eq!(
        rows.len(),
        3,
        "the records survive (structure / order intact)"
    );
    let hot_tail = rows.iter().find(|r| r.message_id == m3).expect("m3 hot");
    assert_eq!(
        hot_tail.state,
        MessageState::Tombstoned,
        "the hot-tail record is tombstoned (body cleared)"
    );
    assert!(hot_tail.body_inline.is_empty(), "the hot body is dropped");

    assert!(report.cascade_published, "the cascade rode the bus");
    let erased = outbox
        .committed_rows()
        .into_iter()
        .filter(|r| r.envelope.type_.0 == CHAT_ERASE_CASCADE_TOKEN)
        .count();
    assert_eq!(
        erased, 3,
        "three chat.message.erased tombstones on the outbox - the DSR cascade the derivatives consume"
    );

    assert!(
        read_state.load(&conv(), SUBJECT).is_none(),
        "read-state purged"
    );
    assert!(
        drafts
            .load(&DraftKey::new(conv().conversation_id, SUBJECT))
            .is_none(),
        "the unsent draft purged"
    );

    assert!(
        report.receipts_complete(),
        "every registered Chat store got an erase receipt - 0 holders missed"
    );
    assert!(
        report.destroyed_key_epoch.is_some(),
        "the destroyed-key epoch is recorded (the post-restore re-erase audit trail, 10.8)"
    );
}

#[test]
fn chat_d8_cascade_is_bus_only_no_backdoor() {
    let kms = KmsEngine::new();
    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();

    let m1 = append(&store, &outbox, &minter, "n1");
    let before = outbox.committed_count();

    let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
    let mut tx = begin(&outbox, &minter);
    cascade.erase(
        &mut tx,
        &EraseScope::Subject {
            subject: subject(),
            tenant: gdpr_tenant(),
        },
        &[(conv(), m1)],
    );
    tx.commit().expect("commit");

    assert_eq!(
        outbox.committed_count() - before,
        1,
        "exactly one chat.message.erased on the bus - no backdoor write into Search/Refs/Notif"
    );
}

struct D8PseudonymMap {
    live: BTreeSet<String>,
}
impl D8PseudonymMap {
    fn with(ids: &[&str]) -> D8PseudonymMap {
        D8PseudonymMap {
            live: ids.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn erase(&mut self, id: &str) {
        self.live.remove(id);
    }
}
impl MentionResolver for D8PseudonymMap {
    fn resolve_display_name(&self, mentioned: &Principal) -> Option<String> {
        self.live
            .contains(&mentioned.principal_id.0)
            .then(|| format!("@{}", mentioned.principal_id.0))
    }
}

#[test]
fn chat_d8_mention_half_renders_erased_user() {
    let subject_p = Principal::stub(
        PrincipalId(SUBJECT.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    let mut map = D8PseudonymMap::with(&[SUBJECT, "psn:other"]);
    assert_eq!(
        render_mention(&subject_p, &map),
        MentionRender::Live(format!("@{SUBJECT}")),
        "before erase: the mention resolves the subject's per-viewer name"
    );

    map.erase(SUBJECT);

    let after = render_mention(&subject_p, &map);
    assert_eq!(after, MentionRender::Erased);
    assert_eq!(after.display(), ERASED_USER);
    let other = Principal::stub(
        PrincipalId("psn:other".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    assert_eq!(
        render_mention(&other, &map),
        MentionRender::Live("@psn:other".into())
    );
}

#[test]
fn chat_d8_restricted_subject_suppressed_at_every_read_path() {
    let holder = ChatHolder::new();
    let gate = RestrictionGate::new(holder.restriction().clone());
    let body = paragraph_body("a message the restricted subject authored", vec![]);

    assert!(index_projection_if_allowed(&gate, SUBJECT, &body, None).is_some());
    assert!(agent_may_read(&gate, SUBJECT));
    assert!(notif_may_route(&gate, SUBJECT));
    assert!(analytics_eligible(&gate, SUBJECT));

    holder.restrict(&subject(), true).expect("restrict on");

    assert!(
        index_projection_if_allowed(&gate, SUBJECT, &body, None).is_none(),
        "indexing suppressed (incl. embeddings - the projection is the embedding's source)"
    );
    assert!(!agent_may_read(&gate, SUBJECT), "agent-use suppressed");
    assert!(!notif_may_route(&gate, SUBJECT), "notif-routing suppressed");
    assert!(!analytics_eligible(&gate, SUBJECT), "analytics suppressed");
    assert!(
        gate.suppressed_everywhere(SUBJECT),
        "the restricted subject is suppressed across ALL read paths (Art. 18 totality)"
    );
}

#[test]
fn chat_d8_tombstone_reason_is_subject_erased_and_idempotent() {
    let kms = KmsEngine::new();
    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();

    let m1 = append(&store, &outbox, &minter, "n1");
    let mut tx = begin(&outbox, &minter);
    store
        .tombstone(&mut tx, &m1, TombstoneReason::SubjectErased)
        .expect("tombstone");
    tx.commit().expect("commit");

    let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
    let mut tx = begin(&outbox, &minter);
    cascade.erase(
        &mut tx,
        &EraseScope::Subject {
            subject: subject(),
            tenant: gdpr_tenant(),
        },
        &[(conv(), m1.clone())],
    );
    tx.commit().expect("commit");

    let rows = store
        .range(&conv(), RangeCursor::Recent, 10)
        .expect("range");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].state,
        MessageState::Tombstoned,
        "the record stays tombstoned (idempotent re-erase, post-restore re-apply safe)"
    );
}
