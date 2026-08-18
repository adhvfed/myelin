use myelin_content::events::{
    KNOWLEDGE_ACCESS_GRANTED, KNOWLEDGE_ACCESS_REVOKED, KNOWLEDGE_BLOCK_CREATED,
    KNOWLEDGE_BLOCK_DELETED, KNOWLEDGE_BLOCK_UPDATED, KNOWLEDGE_COMMENT_CREATED,
    KNOWLEDGE_COMMENT_RESOLVED, KNOWLEDGE_DATABASE_CREATED, KNOWLEDGE_DATABASE_SCHEMA_CHANGED,
    KNOWLEDGE_DOC_UPDATED, KNOWLEDGE_MENTION_CREATED, KNOWLEDGE_PAGE_ARCHIVED,
    KNOWLEDGE_PAGE_CREATED, KNOWLEDGE_PAGE_DELETED, KNOWLEDGE_PAGE_MOVED, KNOWLEDGE_PAGE_PUBLISHED,
    KNOWLEDGE_PAGE_RESTORED, KNOWLEDGE_PAGE_UNPUBLISHED, KNOWLEDGE_PAGE_UPDATED,
    KNOWLEDGE_ROW_CREATED, KNOWLEDGE_ROW_DELETED, KNOWLEDGE_ROW_MOVED, KNOWLEDGE_ROW_UPDATED,
    KNOWLEDGE_SUBJECT_ERASURE_REQUESTED, KNOWLEDGE_SUBJECT_EXPORT_REQUESTED,
    KNOWLEDGE_VIEW_CREATED, KNOWLEDGE_VIEW_UPDATED,
};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType,
    HandleOutcome, OutboxTx, Result, SubjectPattern, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_tenancy::TenantId;

pub fn event_actor_pseudonym(tenant: &str, subject: &str) -> String {
    event_actor_field_pseudonym("principal", tenant, subject)
}

fn event_actor_field_pseudonym(field: &str, tenant: &str, subject: &str) -> String {
    let digest = blake3::hash(
        format!("myelin.knowledge.event-actor.v1\0{field}\0{tenant}\0{subject}").as_bytes(),
    );
    format!("knowledge-author:{}", &digest.to_hex()[..32])
}

pub fn pseudonymized_event_principal(tenant: &str, principal: &Principal) -> Principal {
    let mut projected = principal.clone();
    projected.principal_id = PrincipalId(event_actor_pseudonym(tenant, &principal.principal_id.0));
    if let PrincipalKind::Agent {
        runtime_ref,
        on_behalf_of,
    } = &principal.kind
    {
        projected.kind = PrincipalKind::Agent {
            runtime_ref: RuntimeRef(event_actor_field_pseudonym(
                "runtime-ref",
                tenant,
                &runtime_ref.0,
            )),
            on_behalf_of: on_behalf_of.as_ref().map(|delegator| {
                PrincipalId(event_actor_field_pseudonym(
                    "on-behalf-of",
                    tenant,
                    &delegator.0,
                ))
            }),
        };
    }
    projected
}

pub fn page_ref(tenant: &TenantId, page_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{}/knowledge/page/{}", tenant.0, page_id))
}

pub fn block_ref(tenant: &TenantId, page_id: &str, block_id: &str) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/knowledge/page/{}#b{}",
        tenant.0, page_id, block_id
    ))
}

pub fn database_ref(tenant: &TenantId, db_id: &str) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/knowledge/database/{}",
        tenant.0, db_id
    ))
}

pub fn row_ref(tenant: &TenantId, db_id: &str, row_id: &str) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/knowledge/database/{}#row-{}",
        tenant.0, db_id, row_id
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeChange {
    PageCreated { page_id: String },
    PageUpdated { page_id: String },
    PageMoved { page_id: String },
    PageArchived { page_id: String },
    PageRestored { page_id: String },
    PageDeleted { page_id: String },
    PagePublished { page_id: String },
    PageUnpublished { page_id: String },
    DocUpdated { page_id: String },
    BlockCreated { page_id: String, block_id: String },
    BlockUpdated { page_id: String, block_id: String },
    BlockDeleted { page_id: String, block_id: String },
    DatabaseCreated { db_id: String },
    DatabaseSchemaChanged { db_id: String },
    ViewCreated { db_id: String, view_id: String },
    ViewUpdated { db_id: String, view_id: String },
    RowCreated { db_id: String, row_id: String },
    RowUpdated { db_id: String, row_id: String },
    RowDeleted { db_id: String, row_id: String },
    RowMoved { db_id: String, row_id: String },
    CommentCreated { page_id: String, comment_id: String },
    CommentResolved { page_id: String, comment_id: String },
    MentionCreated { page_id: String, comment_id: String },
    AccessGranted { page_id: String },
    AccessRevoked { page_id: String },
    SubjectExportRequested { page_id: String },
    SubjectErasureRequested { page_id: String },
}

impl KnowledgeChange {
    pub fn event_type(&self) -> &'static str {
        match self {
            KnowledgeChange::PageCreated { .. } => KNOWLEDGE_PAGE_CREATED,
            KnowledgeChange::PageUpdated { .. } => KNOWLEDGE_PAGE_UPDATED,
            KnowledgeChange::PageMoved { .. } => KNOWLEDGE_PAGE_MOVED,
            KnowledgeChange::PageArchived { .. } => KNOWLEDGE_PAGE_ARCHIVED,
            KnowledgeChange::PageRestored { .. } => KNOWLEDGE_PAGE_RESTORED,
            KnowledgeChange::PageDeleted { .. } => KNOWLEDGE_PAGE_DELETED,
            KnowledgeChange::PagePublished { .. } => KNOWLEDGE_PAGE_PUBLISHED,
            KnowledgeChange::PageUnpublished { .. } => KNOWLEDGE_PAGE_UNPUBLISHED,
            KnowledgeChange::DocUpdated { .. } => KNOWLEDGE_DOC_UPDATED,
            KnowledgeChange::BlockCreated { .. } => KNOWLEDGE_BLOCK_CREATED,
            KnowledgeChange::BlockUpdated { .. } => KNOWLEDGE_BLOCK_UPDATED,
            KnowledgeChange::BlockDeleted { .. } => KNOWLEDGE_BLOCK_DELETED,
            KnowledgeChange::DatabaseCreated { .. } => KNOWLEDGE_DATABASE_CREATED,
            KnowledgeChange::DatabaseSchemaChanged { .. } => KNOWLEDGE_DATABASE_SCHEMA_CHANGED,
            KnowledgeChange::ViewCreated { .. } => KNOWLEDGE_VIEW_CREATED,
            KnowledgeChange::ViewUpdated { .. } => KNOWLEDGE_VIEW_UPDATED,
            KnowledgeChange::RowCreated { .. } => KNOWLEDGE_ROW_CREATED,
            KnowledgeChange::RowUpdated { .. } => KNOWLEDGE_ROW_UPDATED,
            KnowledgeChange::RowDeleted { .. } => KNOWLEDGE_ROW_DELETED,
            KnowledgeChange::RowMoved { .. } => KNOWLEDGE_ROW_MOVED,
            KnowledgeChange::CommentCreated { .. } => KNOWLEDGE_COMMENT_CREATED,
            KnowledgeChange::CommentResolved { .. } => KNOWLEDGE_COMMENT_RESOLVED,
            KnowledgeChange::MentionCreated { .. } => KNOWLEDGE_MENTION_CREATED,
            KnowledgeChange::AccessGranted { .. } => KNOWLEDGE_ACCESS_GRANTED,
            KnowledgeChange::AccessRevoked { .. } => KNOWLEDGE_ACCESS_REVOKED,
            KnowledgeChange::SubjectExportRequested { .. } => KNOWLEDGE_SUBJECT_EXPORT_REQUESTED,
            KnowledgeChange::SubjectErasureRequested { .. } => KNOWLEDGE_SUBJECT_ERASURE_REQUESTED,
        }
    }

    pub fn aggregate(&self, _tenant: &TenantId) -> AggregateKey {
        // canonical `type:id` aggregate form (the outbox publisher refuses
        // anything else): page-scoped events share `page:<id>` so a page's
        // history orders as one partition; database-scoped events share
        // `database:<id>`.
        match self {
            KnowledgeChange::PageCreated { page_id }
            | KnowledgeChange::PageUpdated { page_id }
            | KnowledgeChange::PageMoved { page_id }
            | KnowledgeChange::PageArchived { page_id }
            | KnowledgeChange::PageRestored { page_id }
            | KnowledgeChange::PageDeleted { page_id }
            | KnowledgeChange::PagePublished { page_id }
            | KnowledgeChange::PageUnpublished { page_id }
            | KnowledgeChange::DocUpdated { page_id }
            | KnowledgeChange::AccessGranted { page_id }
            | KnowledgeChange::AccessRevoked { page_id }
            | KnowledgeChange::SubjectExportRequested { page_id }
            | KnowledgeChange::SubjectErasureRequested { page_id } => {
                AggregateKey(format!("page:{page_id}"))
            }
            KnowledgeChange::BlockCreated { page_id, .. }
            | KnowledgeChange::BlockUpdated { page_id, .. }
            | KnowledgeChange::BlockDeleted { page_id, .. }
            | KnowledgeChange::CommentCreated { page_id, .. }
            | KnowledgeChange::CommentResolved { page_id, .. }
            | KnowledgeChange::MentionCreated { page_id, .. } => {
                AggregateKey(format!("page:{page_id}"))
            }
            KnowledgeChange::DatabaseCreated { db_id }
            | KnowledgeChange::DatabaseSchemaChanged { db_id }
            | KnowledgeChange::ViewCreated { db_id, .. }
            | KnowledgeChange::ViewUpdated { db_id, .. }
            | KnowledgeChange::RowCreated { db_id, .. }
            | KnowledgeChange::RowUpdated { db_id, .. }
            | KnowledgeChange::RowDeleted { db_id, .. }
            | KnowledgeChange::RowMoved { db_id, .. } => AggregateKey(format!("database:{db_id}")),
        }
    }

    pub fn subject(&self, tenant: &TenantId) -> ArtifactRef {
        match self {
            KnowledgeChange::BlockCreated { page_id, block_id }
            | KnowledgeChange::BlockUpdated { page_id, block_id }
            | KnowledgeChange::BlockDeleted { page_id, block_id } => {
                block_ref(tenant, page_id, block_id)
            }
            KnowledgeChange::CommentCreated {
                page_id,
                comment_id,
            }
            | KnowledgeChange::CommentResolved {
                page_id,
                comment_id,
            }
            | KnowledgeChange::MentionCreated {
                page_id,
                comment_id,
            } => ArtifactRef(format!(
                "myelin://{}/knowledge/page/{}#comment-{}",
                tenant.0, page_id, comment_id
            )),
            KnowledgeChange::ViewCreated { db_id, view_id }
            | KnowledgeChange::ViewUpdated { db_id, view_id } => ArtifactRef(format!(
                "myelin://{}/knowledge/database/{}#view-{}",
                tenant.0, db_id, view_id
            )),
            KnowledgeChange::RowCreated { db_id, row_id }
            | KnowledgeChange::RowUpdated { db_id, row_id }
            | KnowledgeChange::RowDeleted { db_id, row_id }
            | KnowledgeChange::RowMoved { db_id, row_id } => row_ref(tenant, db_id, row_id),
            _ => ArtifactRef(self.aggregate(tenant).0),
        }
    }

    pub fn contains_personal_data(&self) -> bool {
        false
    }
}

pub fn emit_change(
    tx: &mut dyn OutboxTx,
    tenant: &TenantId,
    change: &KnowledgeChange,
    cause: Option<&EventEnvelope>,
) -> Result<EventId> {
    let draft = EventDraft {
        type_: EventType(change.event_type().into()),
        subject: change.subject(tenant),
        aggregate: change.aggregate(tenant),
        payload: serde_json::json!({ "subject": change.subject(tenant).0 }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: change.contains_personal_data(),
        pii_key_ref: None,
    };
    tx.emit(draft, cause)
}

pub static KNOWLEDGE_LIVING_DOC_SUBJECTS: &[SubjectPattern] = &[];

pub const KNOWLEDGE_LIVING_DOC_TRIGGERS: &[&str] = &[
    "issue.issue.updated",
    "issue.issue.closed",
    "ci.run.passed",
    "ci.run.failed",
    "git.commit.pushed",
    "chat.message.created",
    "refs.edge.removed",
];

#[derive(Debug, Default)]
pub struct KnowledgeLivingDocHandler {
    observed: std::sync::atomic::AtomicU64,
}

impl KnowledgeLivingDocHandler {
    pub fn new() -> KnowledgeLivingDocHandler {
        KnowledgeLivingDocHandler::default()
    }

    pub fn observed(&self) -> u64 {
        self.observed.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn reacts_to(type_: &str) -> bool {
        KNOWLEDGE_LIVING_DOC_TRIGGERS.contains(&type_)
    }
}

impl myelin_events::EventHandler for KnowledgeLivingDocHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        KNOWLEDGE_LIVING_DOC_SUBJECTS
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if KnowledgeLivingDocHandler::reacts_to(&ev.type_.0) {
            self.observed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        HandleOutcome::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        validate_event_type, Actor, CausedBy, EmitContextBase, EventHandler, IdMinter,
        MonotonicMinter, OutboxStore, Region, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::Arc;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn principal() -> Principal {
        Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, tenant())
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
        (
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
    }

    #[test]
    fn every_change_maps_to_a_grammatical_knowledge_token() {
        let changes = all_representative_changes();
        for ch in &changes {
            let t = ch.event_type();
            assert!(
                validate_event_type(t).is_ok(),
                "change {ch:?} → `{t}` is UNGRAMMATICAL: {:?}",
                validate_event_type(t)
            );
            assert!(
                t.starts_with("knowledge."),
                "`{t}` must be a knowledge.* token"
            );
        }
    }

    #[test]
    fn aggregate_is_the_doc_or_db_not_the_sub_artifact() {
        let t = tenant();
        let block = KnowledgeChange::BlockUpdated {
            page_id: "7c2".into(),
            block_id: "9".into(),
        };
        assert_eq!(block.aggregate(&t).0, "page:7c2");
        assert_eq!(block.subject(&t).0, "myelin://acme/knowledge/page/7c2#b9");

        let row = KnowledgeChange::RowUpdated {
            db_id: "tasks".into(),
            row_id: "r1".into(),
        };
        assert_eq!(
            row.aggregate(&t).0,
            "myelin://acme/knowledge/database/tasks"
        );
        assert_eq!(
            row.subject(&t).0,
            "myelin://acme/knowledge/database/tasks#row-r1"
        );

        let page = KnowledgeChange::PageUpdated {
            page_id: "7c2".into(),
        };
        assert_eq!(page.aggregate(&t).0, "page:7c2");
    }

    #[test]
    fn blocks_of_one_page_share_the_page_aggregate() {
        let t = tenant();
        let b1 = KnowledgeChange::BlockUpdated {
            page_id: "p1".into(),
            block_id: "b1".into(),
        };
        let b2 = KnowledgeChange::BlockCreated {
            page_id: "p1".into(),
            block_id: "b2".into(),
        };
        let other = KnowledgeChange::BlockUpdated {
            page_id: "p2".into(),
            block_id: "b9".into(),
        };
        assert_eq!(
            b1.aggregate(&t),
            b2.aggregate(&t),
            "same page → same aggregate (per-doc order)"
        );
        assert_ne!(
            b1.aggregate(&t),
            other.aggregate(&t),
            "a different page → a different aggregate"
        );
    }

    #[test]
    fn emit_change_is_emit_iff_committed_zero_ghost_zero_lost() {
        let (store, minter) = store_and_minter();

        let mut tx = store.begin(Arc::clone(&minter), ctx_base());
        tx.stage_state_change("block b9 of page 7c2 updated (version 5)");
        let change = KnowledgeChange::BlockUpdated {
            page_id: "7c2".into(),
            block_id: "9".into(),
        };
        let id = emit_change(&mut tx, &tenant(), &change, None).expect("emit");
        assert_eq!(
            store.outbox_depth(),
            0,
            "an OPEN transaction has written nothing (buffered)"
        );
        tx.commit()
            .expect("commit the state change + its event together");
        assert_eq!(
            store.outbox_depth(),
            1,
            "after commit: exactly the one knowledge event is durable"
        );
        let row = store.row(&id).expect("the committed row");
        assert_eq!(row.envelope.type_.0, KNOWLEDGE_BLOCK_UPDATED);
        assert_eq!(
            row.aggregate.0, "page:7c2",
            "aggregate = the page partition in canonical type:id form"
        );

        {
            let mut tx2 = store.begin(Arc::clone(&minter), ctx_base());
            tx2.stage_state_change("block b9 of page 7c2 updated (version 6)");
            emit_change(&mut tx2, &tenant(), &change, None).expect("emit");
        }
        assert_eq!(
            store.outbox_depth(),
            1,
            "the aborted transaction wrote NO event (0 ghost)"
        );
        assert_eq!(
            store.committed_count(),
            1,
            "no committed state without its event, none with a ghost"
        );
    }

    #[test]
    fn a_reaction_carries_causation_and_depth_plus_one() {
        let (store, minter) = store_and_minter();
        let trigger = trigger_envelope("issue.issue.updated");
        assert_eq!(trigger.depth, 0);

        let mut tx = store.begin(Arc::clone(&minter), ctx_base());
        tx.stage_state_change("living-doc home refreshed from issue PROJ-1");
        let reaction = KnowledgeChange::DocUpdated {
            page_id: "home".into(),
        };
        let reaction_id =
            emit_change(&mut tx, &tenant(), &reaction, Some(&trigger)).expect("emit reaction");
        tx.commit().expect("commit");

        let row = store.row(&reaction_id).expect("the committed reaction row");
        assert_eq!(row.envelope.type_.0, KNOWLEDGE_DOC_UPDATED);
        assert_eq!(
            row.envelope.depth,
            trigger.depth + 1,
            "a reaction is depth parent+1 (loop guard)"
        );
        assert_eq!(
            row.envelope.causation_id,
            Some(trigger.event_id.clone()),
            "causation_id = the incoming trigger event"
        );
        assert_eq!(
            row.envelope.correlation_id, trigger.correlation_id,
            "the correlation root carries from the trigger (one causal thread)"
        );
    }

    #[test]
    fn living_doc_consumer_whitelist_is_never_wildcard() {
        let h = KnowledgeLivingDocHandler::new();
        for SubjectPattern(p) in h.subjects() {
            assert!(
                !p.split('.').any(|seg| seg == "*" || seg == ">") && !p.is_empty(),
                "subject `{p}` must not be a wildcard / empty (rule 3)"
            );
        }
        assert!(KnowledgeLivingDocHandler::reacts_to("issue.issue.updated"));
        assert!(KnowledgeLivingDocHandler::reacts_to("ci.run.passed"));
        assert!(
            !KnowledgeLivingDocHandler::reacts_to("knowledge.block.op"),
            "no raw firehose op"
        );
    }

    #[test]
    fn living_doc_handler_is_idempotent_through_the_runtime() {
        use myelin_events::{consume, ConsumerName, ConsumerSpec, DedupLedger, Delivered, Message};
        let spec = ConsumerSpec::new(
            ConsumerName("knowledge-living-doc".into()),
            &["myelin://acme/issues/"],
        );
        let consumer = consume(spec, KnowledgeLivingDocHandler::new(), DedupLedger::new())
            .expect("the *-free whitelist binds");
        let msg = Message {
            subject: "myelin://acme/issues/issue/PROJ-1".into(),
            envelope: trigger_envelope("issue.issue.updated"),
        };
        assert_eq!(
            consumer.deliver(&msg),
            Delivered::Acked,
            "first delivery runs + acks"
        );
        assert_eq!(
            consumer.deliver(&msg),
            Delivered::Deduplicated,
            "redelivery is deduped (0 dup)"
        );
        assert_eq!(
            consumer.handler().observed(),
            1,
            "the handler ran EXACTLY once (idempotent)"
        );
    }

    fn all_representative_changes() -> Vec<KnowledgeChange> {
        vec![
            KnowledgeChange::PageCreated {
                page_id: "p".into(),
            },
            KnowledgeChange::PageUpdated {
                page_id: "p".into(),
            },
            KnowledgeChange::PageMoved {
                page_id: "p".into(),
            },
            KnowledgeChange::PageArchived {
                page_id: "p".into(),
            },
            KnowledgeChange::PageRestored {
                page_id: "p".into(),
            },
            KnowledgeChange::PageDeleted {
                page_id: "p".into(),
            },
            KnowledgeChange::PagePublished {
                page_id: "p".into(),
            },
            KnowledgeChange::PageUnpublished {
                page_id: "p".into(),
            },
            KnowledgeChange::DocUpdated {
                page_id: "p".into(),
            },
            KnowledgeChange::BlockCreated {
                page_id: "p".into(),
                block_id: "b".into(),
            },
            KnowledgeChange::BlockUpdated {
                page_id: "p".into(),
                block_id: "b".into(),
            },
            KnowledgeChange::BlockDeleted {
                page_id: "p".into(),
                block_id: "b".into(),
            },
            KnowledgeChange::DatabaseCreated { db_id: "d".into() },
            KnowledgeChange::DatabaseSchemaChanged { db_id: "d".into() },
            KnowledgeChange::ViewCreated {
                db_id: "d".into(),
                view_id: "v".into(),
            },
            KnowledgeChange::ViewUpdated {
                db_id: "d".into(),
                view_id: "v".into(),
            },
            KnowledgeChange::RowCreated {
                db_id: "d".into(),
                row_id: "r".into(),
            },
            KnowledgeChange::RowUpdated {
                db_id: "d".into(),
                row_id: "r".into(),
            },
            KnowledgeChange::RowDeleted {
                db_id: "d".into(),
                row_id: "r".into(),
            },
            KnowledgeChange::RowMoved {
                db_id: "d".into(),
                row_id: "r".into(),
            },
            KnowledgeChange::CommentCreated {
                page_id: "p".into(),
                comment_id: "c".into(),
            },
            KnowledgeChange::CommentResolved {
                page_id: "p".into(),
                comment_id: "c".into(),
            },
            KnowledgeChange::MentionCreated {
                page_id: "p".into(),
                comment_id: "c".into(),
            },
            KnowledgeChange::AccessGranted {
                page_id: "p".into(),
            },
            KnowledgeChange::AccessRevoked {
                page_id: "p".into(),
            },
            KnowledgeChange::SubjectExportRequested {
                page_id: "p".into(),
            },
            KnowledgeChange::SubjectErasureRequested {
                page_id: "p".into(),
            },
        ]
    }

    fn trigger_envelope(type_: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("01J-{type_}")),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: None,
            correlation_id: myelin_events::CorrelationId("01J-corr".into()),
            caused_by: Some(CausedBy("session:abc".into())),
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn knowledge_envelopes_pass_the_real_publishers_admission_check() {
        // knowledge.page.created died in outbox_quarantine as an
        // aggregate_mismatch when the row and envelope disagreed; the
        // canonical `page:<id>` partition must clear the publisher's check.
        let t = tenant();
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(Arc::clone(&minter), ctx_base());
        tx.stage_state_change("admission check emits");
        for change in [
            KnowledgeChange::PageCreated { page_id: "7c2".into() },
            KnowledgeChange::BlockUpdated { page_id: "7c2".into(), block_id: "b9".into() },
            KnowledgeChange::RowCreated { db_id: "db1".into(), row_id: "r1".into() },
        ] {
            emit_change(&mut tx, &t, &change, None).expect("emit");
        }
        tx.commit().expect("commit");
        for row in store.committed_rows() {
            let config = myelin_storage::pgrelay::RelayValidationConfig::new(
                row.envelope.region.clone(),
                256 * 1024,
            )
            .unwrap();
            myelin_storage::pgrelay::publisher_admission(&row.envelope, &config).unwrap_or_else(
                |(code, detail)| {
                    panic!(
                        "{} would be QUARANTINED ({code}: {detail})",
                        row.envelope.type_.0
                    )
                },
            );
        }
    }
}
