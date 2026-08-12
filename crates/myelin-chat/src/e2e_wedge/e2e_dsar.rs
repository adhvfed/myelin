use std::sync::Arc;

use myelin_events::{
    Actor, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, OutboxTransaction, Timestamp,
};
use myelin_gdpr::{EraseScope, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::{EncryptedColumn, SubjectId};
use myelin_storage::kms::{DekId, KeyClass, KmsEngine, KmsError};
use myelin_tenancy::{Region, TenantId};

use crate::dek::{encrypt_body, ChatFreeText};
use crate::erase::{is_body_unrecoverable, ChatErasureCascade};
use crate::read_state::{ReadMarker, ReadStateRecord};
use crate::store::{
    AuthorKind, ConversationId, MemHotTier, MessageId, MessageStore, NewMessage, RangeCursor,
};
use crate::{Draft, DraftKey, DraftStore, MemDraftStore, UnfurlCache, CHAT_ERASE_CASCADE_TOKEN};

use super::{dsar_holder_green, ChatE2eArtifact};

pub const E2E_SCENARIO: &str = "E2E-4";

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
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:dsar".into())),
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

pub fn run_e2e_4_chat_dsar_holder() -> ChatE2eArtifact {
    match try_run_e2e_4_chat_dsar_holder() {
        Ok(artifact) => artifact,
        Err(error) => ChatE2eArtifact {
            scenario: E2E_SCENARIO,
            green: false,
            evidence: format!("Chat H5 DSAR holder stopped because crypto-shred failed: {error}"),
            leaks: 1,
        },
    }
}

fn try_run_e2e_4_chat_dsar_holder() -> Result<ChatE2eArtifact, KmsError> {
    let kms = KmsEngine::new();
    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();

    let mut leaks: u64 = 0;

    let m1 = append(&store, &outbox, &minter, "n1");
    let m2 = append(&store, &outbox, &minter, "n2");
    let m3 = append(&store, &outbox, &minter, "n3");
    let sealed = store.seal_before(&conv(), &m3).expect("seal cold");
    let cold_seeded = sealed == 2;

    let hot_body = seal_body(&kms, b"my private chat in the hot tier");
    let cold_body = seal_body(&kms, b"my private chat archived in the cold segment");
    let dek_id = DekId::new(tenant(), KeyClass::Subject(SUBJECT.into()));
    let backup_before = kms.backup_snapshot()?;
    let backup_seeded = backup_before.iter().any(|(id, _)| id == &dek_id);
    let pii_recoverable_before = !is_body_unrecoverable(&kms, &region(), &hot_body)
        && !is_body_unrecoverable(&kms, &region(), &cold_body);

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
    )?;
    tx.commit().expect("commit");

    let hot_unrecoverable = is_body_unrecoverable(&kms, &region(), &hot_body);
    let cold_unrecoverable = is_body_unrecoverable(&kms, &region(), &cold_body);
    let backup_after = kms.backup_snapshot()?;
    let backup_unrecoverable = !backup_after.iter().any(|(id, _)| id == &dek_id);
    if !hot_unrecoverable {
        leaks += 1;
    }
    if !cold_unrecoverable {
        leaks += 1;
    }
    if !backup_unrecoverable {
        leaks += 1;
    }

    let records_addressed = report.records_tombstoned == 3;
    let rows = store
        .range(&conv(), RangeCursor::Recent, 10)
        .expect("range across hot + cold");
    let records_survive = rows.len() == 3;

    let cascade_count = outbox
        .committed_rows()
        .into_iter()
        .filter(|r| r.envelope.type_.0 == CHAT_ERASE_CASCADE_TOKEN)
        .count();
    let cascade_bus_only = cascade_count == 3 && report.cascade_published;

    let read_models_purged = read_state.load(&conv(), SUBJECT).is_none()
        && drafts
            .load(&DraftKey::new(conv().conversation_id, SUBJECT))
            .is_none();

    let holder_green = dsar_holder_green(&report);

    let green = cold_seeded
        && backup_seeded
        && pii_recoverable_before
        && hot_unrecoverable
        && cold_unrecoverable
        && backup_unrecoverable
        && records_addressed
        && records_survive
        && cascade_bus_only
        && read_models_purged
        && holder_green;

    Ok(ChatE2eArtifact {
        scenario: E2E_SCENARIO,
        green,
        evidence: format!(
            "Chat H5 DSAR holder (CHAT-D8 named in the 0-holders-missed certificate): \
             0 recoverable PII (hot={hot_unrecoverable}, cold={cold_unrecoverable}, \
             backup={backup_unrecoverable}); records survive as tombstones \
             (addressed={records_addressed}, survive={records_survive}); cascade bus-only no-backdoor \
             ({cascade_count} chat.message.erased)={cascade_bus_only}; read-models purged \
             ={read_models_purged}; 0 holders missed (complete receipt set + destroyed-key epoch \
             recorded)={holder_green}; leaks={leaks}",
        ),
        leaks,
    })
}
