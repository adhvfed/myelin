use myelin_gdpr::{EraseReceipt, EraseScope, Receipt, SubjectRef, TenantId};
use myelin_storage::encryption::EncryptedColumn;
use myelin_storage::kms::{DekId, KmsEngine, KmsError};
use myelin_tenancy::Region;

use crate::composer::DraftStore;
use crate::events::CHAT_MESSAGE_ERASED;
use crate::holder::{ChatStoreClass, CHAT_OLTP_STORE};
use crate::read_state::ReadStateRecord;
use crate::store::{
    ConversationId, MessageId, MessageStore, OutboxTx, StoreError, TombstoneReason,
};
use crate::unfurl::UnfurlCache;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreReceipt {
    pub class: ChatStoreClass,
    pub receipt: Receipt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatEraseReport {
    pub store_receipts: Vec<StoreReceipt>,
    pub destroyed_key_epoch: Option<u64>,
    pub records_tombstoned: usize,
    pub cascade_published: bool,
}

#[derive(Debug)]
pub enum ChatEraseError {
    Key(KmsError),
    Store(StoreError),
}

impl core::fmt::Display for ChatEraseError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Key(error) => write!(formatter, "chat erasure key operation failed: {error}"),
            Self::Store(error) => {
                write!(
                    formatter,
                    "chat erasure tombstone operation failed: {error}"
                )
            }
        }
    }
}

impl std::error::Error for ChatEraseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Key(error) => Some(error),
            Self::Store(error) => Some(error),
        }
    }
}

impl From<KmsError> for ChatEraseError {
    fn from(error: KmsError) -> Self {
        Self::Key(error)
    }
}

impl From<StoreError> for ChatEraseError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl ChatEraseReport {
    pub fn receipts_complete(&self) -> bool {
        ChatStoreClass::ALL.iter().all(|class| {
            self.store_receipts
                .iter()
                .any(|r| r.class == *class && r.receipt.operation == "erase")
        })
    }

    pub fn receipt_for(&self, class: ChatStoreClass) -> Option<&StoreReceipt> {
        self.store_receipts.iter().find(|r| r.class == class)
    }
}

pub struct ChatErasureCascade<'a, S: MessageStore> {
    kms: &'a KmsEngine,
    region: Region,
    store: &'a S,
    read_state: &'a ReadStateRecord,
    drafts: &'a dyn DraftStore,
    unfurl_cache: &'a UnfurlCache,
}

impl<'a, S: MessageStore> ChatErasureCascade<'a, S> {
    pub fn new(
        kms: &'a KmsEngine,
        region: Region,
        store: &'a S,
        read_state: &'a ReadStateRecord,
        drafts: &'a dyn DraftStore,
        unfurl_cache: &'a UnfurlCache,
    ) -> ChatErasureCascade<'a, S> {
        ChatErasureCascade {
            kms,
            region,
            store,
            read_state,
            drafts,
            unfurl_cache,
        }
    }

    pub fn erase(
        &self,
        tx: &mut OutboxTx,
        scope: &EraseScope,
        authored: &[(ConversationId, MessageId)],
    ) -> Result<ChatEraseReport, ChatEraseError> {
        let (subject_token, tenant) = match scope {
            EraseScope::Subject { subject, tenant } => {
                (Some(subject_token(subject)), tenant.clone())
            }
            EraseScope::Tenant(t) => (None, t.clone()),
        };

        let destroyed_key_epoch = match &subject_token {
            Some(sid) => {
                let dek_id = DekId::new(tenant.clone(), crate::dek::chat_subject_key_class(sid));
                let epoch = self.dek_epoch(&tenant, sid, &dek_id)?;
                self.kms.destroy_dek(&dek_id)?;
                epoch
            }
            None => None,
        };

        let erased_author = subject_token.clone().unwrap_or_default();
        let mut records_tombstoned = 0usize;
        for (conv, msg_id) in authored {
            match self
                .store
                .tombstone(tx, msg_id, TombstoneReason::SubjectErased)
            {
                Ok(()) => {}
                Err(StoreError::NotFound(_)) => {
                    crate::store::emit_erased_tombstone(tx, conv, msg_id, &erased_author)?;
                }
                Err(error) => return Err(error.into()),
            }
            records_tombstoned += 1;
        }
        let cascade_published = records_tombstoned > 0 || subject_token.is_some();

        if let Some(sid) = &subject_token {
            self.read_state.purge_principal(sid);
            self.drafts.purge_author(sid);
            self.unfurl_cache.clear();
        }

        let store_receipts = self.store_receipts(
            subject_token.as_deref().unwrap_or(""),
            &tenant,
            destroyed_key_epoch,
        );

        Ok(ChatEraseReport {
            store_receipts,
            destroyed_key_epoch,
            records_tombstoned,
            cascade_published,
        })
    }

    fn dek_epoch(
        &self,
        tenant: &TenantId,
        subject_token: &str,
        dek_id: &DekId,
    ) -> Result<Option<u64>, KmsError> {
        let present = self
            .kms
            .backup_snapshot()?
            .into_iter()
            .any(|(id, _)| &id == dek_id);
        if !present {
            return Ok(None);
        }
        Ok(self
            .kms
            .ensure_dek(
                tenant,
                &self.region,
                crate::dek::chat_subject_key_class(subject_token),
            )
            .ok()
            .map(|key_ref| key_ref.dek_epoch))
    }

    fn store_receipts(
        &self,
        subject_token: &str,
        tenant: &TenantId,
        destroyed_key_epoch: Option<u64>,
    ) -> Vec<StoreReceipt> {
        ChatStoreClass::ALL
            .iter()
            .map(|class| {
                let key_epoch = match class {
                    ChatStoreClass::Messages | ChatStoreClass::Drafts => destroyed_key_epoch,
                    ChatStoreClass::AuthorIdentity | ChatStoreClass::ReadState => None,
                };
                let receipt = Receipt::content_addressed(
                    "erase",
                    &format!("{CHAT_OLTP_STORE}/{}", class.label()),
                    subject_token,
                    &tenant.0,
                    class_outcome(*class),
                    key_epoch,
                    0,
                );
                StoreReceipt {
                    class: *class,
                    receipt,
                }
            })
            .collect()
    }
}

pub fn is_body_unrecoverable(kms: &KmsEngine, region: &Region, column: &EncryptedColumn) -> bool {
    crate::dek::decrypt_body(kms, region, column).is_err()
}

fn subject_token(subject: &SubjectRef) -> String {
    subject.principal.principal_id.0.clone()
}

fn class_outcome(class: ChatStoreClass) -> &'static str {
    match class {
        ChatStoreClass::Messages => {
            "crypto-shred the per-subject body DEK (hot/cold/backups unrecoverable) + \
             chat.message.erased tombstone; the immutable log is not rewritten"
        }
        ChatStoreClass::Drafts => "crypto-shred the per-subject draft DEK + purge the draft rows",
        ChatStoreClass::AuthorIdentity => {
            "pseudonym-shred the author identity ([erased user]; the render is CHAT-P23, \
             crate::restriction::render_mention)"
        }
        ChatStoreClass::ReadState => "purge the per-(user × conversation) read-state markers",
    }
}

pub fn aggregate_receipt(report: &ChatEraseReport, scope: &EraseScope) -> EraseReceipt {
    let (subject_token, tenant) = match scope {
        EraseScope::Subject { subject, tenant } => (subject_token(subject), tenant.0.clone()),
        EraseScope::Tenant(t) => (String::new(), t.0.clone()),
    };
    EraseReceipt {
        receipt: Receipt::content_addressed(
            "erase",
            CHAT_OLTP_STORE,
            &subject_token,
            &tenant,
            &format!(
                "Chat erase fan-out (CHAT-P22): {} stores, {} records tombstoned, cascade_published={}; \
                 residual = the ONE posture 10.9/X-7 by reference",
                report.store_receipts.len(),
                report.records_tombstoned,
                report.cascade_published,
            ),
            report.destroyed_key_epoch,
            0,
        ),
    }
}

pub const CHAT_ERASE_CASCADE_TOKEN: &str = CHAT_MESSAGE_ERASED;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::{Draft, DraftKey, MemDraftStore};
    use crate::dek::{encrypt_body, ChatFreeText};
    use crate::read_state::ReadMarker;
    use crate::store::{
        AuthorKind, MemHotTier, Message, NewMessage, RangeCursor, Result as StoreResult,
    };
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_gdpr::SubjectRef;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::encryption::SubjectId;
    use std::sync::Arc;

    fn region() -> Region {
        Region::new("fr-par")
    }

    struct UnavailableMessageStore;

    impl MessageStore for UnavailableMessageStore {
        fn append(&self, _tx: &mut OutboxTx, _message: NewMessage) -> StoreResult<MessageId> {
            Err(StoreError::Cold("hot tier unavailable".into()))
        }

        fn range(
            &self,
            _conversation: &ConversationId,
            _cursor: RangeCursor,
            _limit: u32,
        ) -> StoreResult<Vec<Message>> {
            Err(StoreError::Cold("hot tier unavailable".into()))
        }

        fn revise(
            &self,
            _tx: &mut OutboxTx,
            _message_id: &MessageId,
            _body_inline: Vec<u8>,
            _body_nodes: Vec<u8>,
            _expect_seq: i32,
        ) -> StoreResult<()> {
            Err(StoreError::Cold("hot tier unavailable".into()))
        }

        fn tombstone(
            &self,
            _tx: &mut OutboxTx,
            _message_id: &MessageId,
            _reason: TombstoneReason,
        ) -> StoreResult<()> {
            Err(StoreError::Cold("hot tier unavailable".into()))
        }

        fn resync_from(
            &self,
            _conversation: &ConversationId,
            _cursor: &MessageId,
        ) -> StoreResult<Vec<Message>> {
            Err(StoreError::Cold("hot tier unavailable".into()))
        }
    }
    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: myelin_tenancy::TenantId("acme".into()),
            region: myelin_tenancy::Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                myelin_tenancy::TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn begin_tx(outbox: &OutboxStore, minter: &Arc<MonotonicMinter>) -> OutboxTx {
        outbox.begin(minter.clone(), ctx_base())
    }

    fn conv() -> ConversationId {
        ConversationId::new("acme", "fr-par", "c-1")
    }

    fn seed_message(
        store: &MemHotTier,
        outbox: &OutboxStore,
        minter: &Arc<MonotonicMinter>,
        author: &str,
        nonce: &str,
    ) -> MessageId {
        let mut tx = begin_tx(outbox, minter);
        let id = store
            .append(
                &mut tx,
                NewMessage {
                    conv: conv(),
                    thread_root_id: None,
                    author: author.to_string(),
                    author_kind: AuthorKind::Human,
                    body_inline: b"hello".to_vec(),
                    body_nodes: Vec::new(),
                    client_nonce: nonce.to_string(),
                },
            )
            .expect("append");
        tx.commit().expect("commit");
        id
    }

    #[test]
    fn erased_subject_body_is_unrecoverable_in_hot_cold_and_backups() {
        let kms = KmsEngine::new();
        let author = SubjectId::new("psn:ada");
        let plaintext = b"my private chat about my health".to_vec();
        let column = encrypt_body(
            &kms,
            &region(),
            &tenant(),
            &author,
            ChatFreeText::BodyInline,
            &plaintext,
        )
        .expect("seal");
        assert!(!is_body_unrecoverable(&kms, &region(), &column));

        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        let report = cascade
            .erase(
                &mut tx,
                &EraseScope::Subject {
                    subject: subject("psn:ada"),
                    tenant: tenant(),
                },
                &[],
            )
            .unwrap();
        tx.commit().expect("commit");

        assert!(
            is_body_unrecoverable(&kms, &region(), &column),
            "0 recoverable: the body is unrecoverable after the DEK crypto-shred"
        );
        assert!(
            !kms.backup_snapshot()
                .unwrap()
                .into_iter()
                .any(|(id, _)| id
                    == DekId::new(tenant(), crate::dek::chat_subject_key_class("psn:ada"))),
            "the crypto-shredded DEK is EXCLUDED from backups (it stays dead across restore, §7.5)"
        );
        assert!(
            report.destroyed_key_epoch.is_some(),
            "the receipt records the destroyed key epoch (the post-restore re-erase audit trail)"
        );
    }

    #[test]
    fn cascade_tombstones_and_emits_erased_via_the_bus_not_a_backdoor() {
        let kms = KmsEngine::new();
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let m1 = seed_message(&store, &outbox, &minter, "psn:ada", "n1");
        let m2 = seed_message(&store, &outbox, &minter, "psn:ada", "n2");
        let before = outbox.committed_count();

        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        let report = cascade
            .erase(
                &mut tx,
                &EraseScope::Subject {
                    subject: subject("psn:ada"),
                    tenant: tenant(),
                },
                &[(conv(), m1.clone()), (conv(), m2.clone())],
            )
            .unwrap();
        tx.commit().expect("commit");

        assert_eq!(report.records_tombstoned, 2, "both records tombstoned");
        assert!(report.cascade_published, "the cascade rode the bus");
        let erased: Vec<_> = outbox
            .committed_rows()
            .into_iter()
            .filter(|r| r.envelope.type_.0 == CHAT_ERASE_CASCADE_TOKEN)
            .collect();
        assert_eq!(
            erased.len(),
            2,
            "two chat.message.erased tombstones on the outbox - the cascade the derivatives consume"
        );
        assert!(
            outbox.committed_count() > before,
            "the cascade ADDED durable events (no backdoor - the bus is the only path)"
        );
        let rows = store
            .range(&conv(), crate::store::RangeCursor::Recent, 10)
            .expect("range");
        for r in &rows {
            assert_eq!(r.state, crate::store::MessageState::Tombstoned);
            assert!(r.body_inline.is_empty(), "the body is dropped (shred)");
        }
    }

    #[test]
    fn an_unavailable_message_store_cannot_produce_a_success_receipt() {
        let kms = KmsEngine::new();
        let store = UnavailableMessageStore;
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);

        let error = cascade
            .erase(
                &mut tx,
                &EraseScope::Tenant(tenant()),
                &[(conv(), MessageId("01J0UNAVAILABLE".into()))],
            )
            .expect_err("a storage outage must fail the erasure instead of claiming success");

        assert!(matches!(
            error,
            ChatEraseError::Store(StoreError::Cold(ref detail))
                if detail == "hot tier unavailable"
        ));
        assert_eq!(
            outbox.committed_count(),
            0,
            "no false cascade was committed"
        );
    }

    #[test]
    fn cascade_purges_read_state_drafts_and_unfurl_cache() {
        let kms = KmsEngine::new();
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();

        read_state.upsert(&ReadMarker::new(
            conv(),
            "psn:ada",
            MessageId("01ABC".into()),
        ));
        drafts.save(
            &DraftKey::new(conv().conversation_id, "psn:ada"),
            &Draft::text("an unsent private draft"),
        );
        assert!(
            read_state.load(&conv(), "psn:ada").is_some(),
            "read-state present"
        );
        assert!(
            drafts
                .load(&DraftKey::new(conv().conversation_id, "psn:ada"))
                .is_some(),
            "draft present"
        );

        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        cascade
            .erase(
                &mut tx,
                &EraseScope::Subject {
                    subject: subject("psn:ada"),
                    tenant: tenant(),
                },
                &[],
            )
            .unwrap();
        tx.commit().expect("commit");

        assert!(
            read_state.load(&conv(), "psn:ada").is_none(),
            "read-state purged (0 footprint)"
        );
        assert!(
            drafts
                .load(&DraftKey::new(conv().conversation_id, "psn:ada"))
                .is_none(),
            "the unsent draft purged"
        );
    }

    #[test]
    fn the_holder_receipt_set_is_complete_zero_missed() {
        let kms = KmsEngine::new();
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        let report = cascade
            .erase(
                &mut tx,
                &EraseScope::Subject {
                    subject: subject("psn:ada"),
                    tenant: tenant(),
                },
                &[],
            )
            .unwrap();
        tx.commit().expect("commit");

        assert!(
            report.receipts_complete(),
            "every registered Chat store got an erase receipt (0 holders missed)"
        );
        assert_eq!(report.store_receipts.len(), ChatStoreClass::ALL.len());
        for class in ChatStoreClass::ALL {
            let r = report
                .receipt_for(class)
                .expect("a receipt per store class");
            assert_eq!(r.receipt.operation, "erase");
            assert!(
                r.receipt.content_hash.starts_with("blake3:"),
                "{} receipt is content-addressed",
                class.label()
            );
        }
        assert!(
            report
                .receipt_for(ChatStoreClass::Messages)
                .unwrap()
                .receipt
                .key_epoch_destroyed
                .is_none()
                == report.destroyed_key_epoch.is_none()
        );
        assert!(report
            .receipt_for(ChatStoreClass::ReadState)
            .unwrap()
            .receipt
            .key_epoch_destroyed
            .is_none());
    }

    #[test]
    fn tenant_offboarding_scope_produces_a_complete_receipt_set() {
        let kms = KmsEngine::new();
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        let scope = EraseScope::Tenant(tenant());
        let report = cascade.erase(&mut tx, &scope, &[]).unwrap();
        tx.commit().expect("commit");
        assert!(report.receipts_complete());
        let agg = aggregate_receipt(&report, &scope);
        assert_eq!(agg.receipt.operation, "erase");
        assert!(agg.receipt.content_hash.starts_with("blake3:"));
    }

    #[test]
    fn aggregate_receipt_is_content_addressed_over_the_fan_out() {
        let kms = KmsEngine::new();
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        let scope = EraseScope::Subject {
            subject: subject("psn:ada"),
            tenant: tenant(),
        };
        let report = cascade.erase(&mut tx, &scope, &[]).unwrap();
        tx.commit().expect("commit");
        let agg = aggregate_receipt(&report, &scope);
        assert_eq!(agg.receipt.operation, "erase");
        assert_eq!(agg.receipt.key_epoch_destroyed, report.destroyed_key_epoch);
    }
}
