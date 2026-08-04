use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_content::{parse_inline, Block, HeadingLevel, InlineNode};
use myelin_query::FieldValue;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventId, EventType, Timestamp,
    Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::{
    message_index_specs, message_search_projection, AclFilter, IncrementalIndexer,
    MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher, SearchProjection, FACET_ARTIFACT_REF,
    FACET_MENTION,
};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str, t: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(t.into()),
    )
}

fn mention_node(id: &str) -> InlineNode {
    InlineNode::Mention(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    ))
}

#[derive(Default)]
struct ChatFetcher {
    projections: Mutex<BTreeMap<String, SearchProjection>>,
}
impl ChatFetcher {
    fn put(&self, ref_: &str, p: SearchProjection) {
        self.projections.lock().unwrap().insert(ref_.to_string(), p);
    }
}
impl ProjectFetcher for ChatFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        match self.projections.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

fn chat_event(id: &str, type_: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(viewer("platform", "acme")),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(format!("agg:{subject}")),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: true,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        payload: serde_json::json!({ "zookie": "zk-chat-1", "version": 1 }),
    }
}

fn event_in(id: &str, type_: &str, subject: &str, t: &str) -> EventEnvelope {
    let mut ev = chat_event(id, type_, subject);
    ev.tenant = TenantId(t.into());
    ev
}

fn chat_indexer(fetcher: Arc<ChatFetcher>) -> IncrementalIndexer {
    IncrementalIndexer::new(
        message_index_specs(),
        fetcher,
        Arc::new(MockEmbeddingAdapter::new(8)),
    )
}

fn prose(text: &str) -> Vec<Block> {
    vec![Block::Paragraph {
        inline: parse_inline(text, &[]),
    }]
}

#[test]
fn chat_index_and_query_returns_the_right_message() {
    let m_en = "myelin://acme/chat/message/m-1";
    let m_de = "myelin://acme/chat/message/m-2";
    let m_mention = "myelin://acme/chat/message/m-3";

    let fetcher = Arc::new(ChatFetcher::default());
    fetcher.put(
        m_en,
        message_search_projection(&prose("the scheduler deadlock is back"), Some("en")),
    );
    fetcher.put(
        m_de,
        message_search_projection(&prose("der Scheduler ist wieder blockiert"), Some("de")),
    );
    fetcher.put(
        m_mention,
        message_search_projection(
            &[Block::Paragraph {
                inline: parse_inline(
                    &format!("hey {} can you look", myelin_content::OBJ),
                    &[mention_node("alice")],
                ),
            }],
            Some("en"),
        ),
    );

    let ix = chat_indexer(fetcher);
    ix.index(&chat_event("e-1", "chat.message.created", m_en))
        .expect("index en message");
    ix.index(&chat_event("e-2", "chat.message.created", m_de))
        .expect("index de message");
    ix.index(&chat_event("e-3", "chat.message.created", m_mention))
        .expect("index mention message");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        3,
        "all three messages are live"
    );

    let acl = AclFilter::ids([m_en, m_de, m_mention]);

    let ft = ix
        .search_ft(&tenant(), &region(), &acl, "deadlock", 10)
        .expect("ft search");
    assert!(
        ft.iter().any(|h| h.doc_id == m_en),
        "the English body term finds the English message"
    );
    assert!(
        !ft.iter().any(|h| h.doc_id == m_de),
        "the English term does not match the German message"
    );

    let ft_de = ix
        .search_ft(&tenant(), &region(), &acl, "blockiert", 10)
        .expect("ft de search");
    assert!(
        ft_de.iter().any(|h| h.doc_id == m_de),
        "the German body term finds the German message (multilingual, §4.7)"
    );

    let by_mention = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            FACET_MENTION,
            &FieldValue::Relation("alice".into()),
            10,
        )
        .expect("mention facet scan");
    assert_eq!(by_mention.len(), 1, "exactly the message mentioning alice");
    assert_eq!(by_mention[0].doc_id, m_mention);
}

#[test]
fn srch_d1_non_member_search_returns_zero() {
    let member_msg = "myelin://acme/chat/message/m-50";
    let private_msg = "myelin://acme/chat/message/m-51";
    let fetcher = Arc::new(ChatFetcher::default());
    fetcher.put(
        member_msg,
        message_search_projection(&prose("the zarquon launch is on track"), Some("en")),
    );
    fetcher.put(
        private_msg,
        message_search_projection(&prose("the secret zarquon acquisition closed"), Some("en")),
    );
    let ix = chat_indexer(fetcher);
    ix.index(&chat_event("v", "chat.message.created", member_msg))
        .expect("index member message");
    ix.index(&chat_event("p", "chat.message.created", private_msg))
        .expect("index private message");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        2,
        "both messages are indexed"
    );

    let acl_non_member = AclFilter::ids([member_msg]);

    let ft = ix
        .search_ft(&tenant(), &region(), &acl_non_member, "zarquon", 10)
        .expect("ft");
    assert_eq!(
        ft.len(),
        1,
        "0 count-leak: exactly the one member message (the private message never counted)"
    );
    assert_eq!(ft[0].doc_id, member_msg);
    assert!(
        !ft.iter().any(|h| h.doc_id == private_msg),
        "0 leak: the private-channel message never surfaces for a non-member"
    );

    let acl_joined = AclFilter::ids([member_msg, private_msg]);
    let joined = ix
        .search_ft(&tenant(), &region(), &acl_joined, "zarquon", 10)
        .expect("ft joined");
    assert_eq!(
        joined.len(),
        2,
        "after joining the channel BOTH messages surface (the rejection was the ACL, not a deny)"
    );
    assert!(
        joined.iter().any(|h| h.doc_id == private_msg),
        "the now-member's private-channel message appears"
    );
}

#[test]
fn srch_d3_cross_tenant_messages_do_not_leak() {
    let acme_msg = "myelin://acme/chat/message/m-1";
    let evil_msg = "myelin://evil/chat/message/m-1";
    let fetcher = Arc::new(ChatFetcher::default());
    fetcher.put(
        acme_msg,
        message_search_projection(&prose("scheduler standup"), Some("en")),
    );
    fetcher.put(
        evil_msg,
        message_search_projection(&prose("scheduler standup"), Some("en")),
    );
    let ix = chat_indexer(fetcher);
    ix.index(&event_in("a", "chat.message.created", acme_msg, "acme"))
        .expect("index acme");
    ix.index(&event_in("e", "chat.message.created", evil_msg, "evil"))
        .expect("index evil");

    let acme_t = TenantId("acme".into());
    let evil_t = TenantId("evil".into());

    let acme_hits = ix
        .search_ft(
            &acme_t,
            &region(),
            &AclFilter::ids([acme_msg]),
            "scheduler",
            10,
        )
        .expect("acme search");
    assert!(
        acme_hits.iter().any(|h| h.doc_id == acme_msg),
        "acme sees its own message"
    );

    let cross = ix
        .search_ft(
            &acme_t,
            &region(),
            &AclFilter::ids([evil_msg]),
            "scheduler",
            10,
        )
        .expect("cross-tenant search");
    assert!(
        cross.is_empty(),
        "0 cross-tenant: acme's index holds none of evil's messages"
    );

    let evil_hits = ix
        .search_ft(
            &evil_t,
            &region(),
            &AclFilter::ids([acme_msg]),
            "scheduler",
            10,
        )
        .expect("evil search");
    assert!(
        evil_hits.is_empty(),
        "0 cross-tenant: evil's index holds none of acme's messages"
    );
}

#[test]
fn cross_subsystem_facets_are_dependable_across_producers() {
    use myelin_search::page_search_projection;

    let referenced = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());

    let chat_msg = "myelin://acme/chat/message/m-9";
    let chat_proj = message_search_projection(
        &[Block::Paragraph {
            inline: parse_inline(
                &format!("see {} for context", myelin_content::OBJ),
                &[InlineNode::ArtifactRefNode(referenced.clone())],
            ),
        }],
        Some("en"),
    );

    let kn_page = "myelin://acme/knowledge/page/p-9";
    let kn_proj = page_search_projection(
        &[
            Block::Heading {
                level: HeadingLevel::new(1).unwrap(),
                inline: parse_inline("Design", &[]),
            },
            Block::Paragraph {
                inline: parse_inline(
                    &format!("tracked in {}", myelin_content::OBJ),
                    &[InlineNode::ArtifactRefNode(referenced.clone())],
                ),
            },
        ],
        Some("en"),
    );

    let specs = {
        let mut s = message_index_specs();
        s.extend(myelin_search::kn_index_specs());
        s
    };
    let fetcher = Arc::new(ChatFetcher::default());
    fetcher.put(chat_msg, chat_proj);
    fetcher.put(kn_page, kn_proj);
    let ix = IncrementalIndexer::new(specs, fetcher, Arc::new(MockEmbeddingAdapter::new(16)));

    ix.index(&chat_event("c", "chat.message.created", chat_msg))
        .expect("index chat message");
    ix.index(&chat_event("k", "knowledge.page.created", kn_page))
        .expect("index kn page");

    let acl = AclFilter::ids([chat_msg, kn_page]);
    let hits = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            FACET_ARTIFACT_REF,
            &FieldValue::Relation(referenced.0.clone()),
            10,
        )
        .expect("artifact_ref facet scan");
    let ids: Vec<&str> = hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(
        ids.contains(&chat_msg),
        "the Chat message is found by the cross-subsystem artifact_ref facet"
    );
    assert!(
        ids.contains(&kn_page),
        "the KN page is found by the SAME cross-subsystem artifact_ref facet (X-2, one facet shape)"
    );
    assert_eq!(
        hits.len(),
        2,
        "exactly the two referencing docs across the two producers (the dependable cross-subsystem facet)"
    );
}
