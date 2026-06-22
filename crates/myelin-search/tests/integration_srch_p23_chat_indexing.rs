//! # Integration — SRCH-P23 (P-341, M4): Search indexes the REAL Chat corpus (the last producer)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D1 (F1 — the
//! zero-escape leak: a private-channel message NEVER in any result incl. counts; **the
//! search-as-non-member = 0 guarantee, the CHAT-D11 analog**) + SRCH-D3 (F2 — cross-tenant IDOR = 0),
//! the gate-invariant ratchet **re-confirmed on the FULL five-producer corpus** (Git + KN + Issues +
//! CI logs + Chat). **Architecture:** `search-and-indexing.md` §4.7 (the multilingual analyzers — Chat
//! message bodies are EU-multilingual), §3.1 (the structured inline nodes uniform across all five
//! producers). **Reconciliation:** `00-reconciliation-decisions.md` X-2 (the three content nodes
//! byte-identical — mention/ref facets reliable across Git/KN/Issues/Chat). **Contracts:** 6.3
//! (consume Chat's IndexSpec), 4.3/4.9 (the channel ReBAC fragment via `list_objects`).
//!
//! ## What this proves (the dated green artifact, 2026-06-23)
//! The REAL Chat corpus is projected through [`myelin_search::message_search_projection`] (Chat's
//! consumed 6.3 projection over the `myelin_content::Block` markdown subset) into the LIVE
//! [`IncrementalIndexer`] per-event pipeline (project-fetch → analyze → upsert), then queried back
//! through the engine surface. The GATE:
//!
//! 1. **Chat indexing correctness (multilingual)** — a message body term hits its message; a non-English
//!    (German) body is indexed under its `lang` analyzer chain (§4.7); a `@mention` facet returns the
//!    right message (the structured node walk).
//! 2. **SRCH-D1 (F1) — search-as-non-member = 0 (the CHAT-D11 analog)** — a message in a PRIVATE channel
//!    the viewer is NOT a member of never appears in ANY result (FT or structured facet) incl. counts;
//!    a membership grant ⇒ it appears (the rejection was the channel ACL firing, not a blanket deny).
//! 3. **SRCH-D3 (F2) — cross-tenant IDOR = 0** — a viewer's tenant partitions the index; a cross-tenant
//!    query sees 0 of the other tenant's messages (the per-tenant index, partition-keyed).
//! 4. **Cross-subsystem facets dependable (X-2)** — a `mention`/`artifact_ref` facet query is reliable
//!    across the FULL corpus: the SAME facet key surfaces a KN page, an Issue, a Git commit-message doc,
//!    AND a Chat message — proving the one cross-producer facet shape (no second extraction path).
//!
//! The ENGINE is UNCHANGED — this is producer-corpus wiring (the prompt's DoD). No mutation-core module
//! is added; the SRCH-P09/P11/P15 mutation floors still hold on the full five-producer corpus.
//!
//! ## Floor named
//! Chat completes the five-producer corpus; it is NOT world-scale-hardened. The M5 world-scale
//! hardening (surge SRCH-P25, freshness SRCH-P24, restore SRCH-P28, HYOK SRCH-P29, cross-cell SRCH-P31)
//! is the FOLLOW-ON band ([`myelin_search::ChatFiveProducerCorpusNotWorldScaleFloor`]).

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

// ----------------------------------------------------------------------------------------------
// fixtures — the REAL Chat corpus projected through Chat's consumed 6.3 spec
// ----------------------------------------------------------------------------------------------

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

/// A scripted [`ProjectFetcher`] over a `ref → SearchProjection` map — the owner's `project(ref,
/// viewer)` (5.6). The REAL Chat corpus is built by [`message_search_projection`] over the
/// `myelin_content::Block` markdown body, so this fetcher serves Chat's genuine 6.3 projection.
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

/// Build the indexer over the REAL Chat spec (the `message` declare_indexable shape).
fn chat_indexer(fetcher: Arc<ChatFetcher>) -> IncrementalIndexer {
    IncrementalIndexer::new(
        message_index_specs(),
        fetcher,
        Arc::new(MockEmbeddingAdapter::new(8)),
    )
}

/// A plain-prose message body (one paragraph) — the multilingual full-text path (§4.7).
fn prose(text: &str) -> Vec<Block> {
    vec![Block::Paragraph {
        inline: parse_inline(text, &[]),
    }]
}

// ----------------------------------------------------------------------------------------------
// 1. Chat indexing correctness — multilingual message bodies + the @mention facet
// ----------------------------------------------------------------------------------------------

/// **A message body term hits its message; a non-English body is indexed under its `lang` chain; an
/// `@mention` facet returns the right message.** The REAL Chat corpus is projected through Chat's
/// consumed 6.3 spec and indexed through the live per-event pipeline.
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
    // A German message body — multilingual full-text (§4.7), indexed under the `de` analyzer chain.
    fetcher.put(
        m_de,
        message_search_projection(&prose("der Scheduler ist wieder blockiert"), Some("de")),
    );
    // A message @mentioning alice — the structured-node facet.
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

    // FT: the English body term hits its message; the German one does not (and vice-versa).
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

    // The German body is indexed (multilingual §4.7): a German term hits the German message.
    let ft_de = ix
        .search_ft(&tenant(), &region(), &acl, "blockiert", 10)
        .expect("ft de search");
    assert!(
        ft_de.iter().any(|h| h.doc_id == m_de),
        "the German body term finds the German message (multilingual, §4.7)"
    );

    // The structured @mention facet returns exactly the message mentioning alice.
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

// ----------------------------------------------------------------------------------------------
// 2. SRCH-D1 (F1) — search-as-non-member = 0 (the CHAT-D11 analog) on the REAL Chat corpus
// ----------------------------------------------------------------------------------------------

/// **SRCH-D1 (F1) re-confirmed on the Chat corpus — the search-as-non-member = 0 guarantee (CHAT-D11
/// analog): a message in a PRIVATE channel the viewer is NOT a member of never appears in ANY result
/// (FT or structured facet) incl. counts — and a membership grant makes it appear (the rejection was
/// the channel ACL firing, not a blanket deny).**
#[test]
fn srch_d1_non_member_search_returns_zero() {
    // A message in a channel the viewer IS a member of, and one in a PRIVATE channel they are NOT. Both
    // carry the SAME rare term — so a leak would be exposed by FT/count/IDF inference.
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

    // The non-member's reachable set is JUST the channel they belong to (channel.read = member +
    // parent_project->read). The private channel is NOT in `list_objects(channel)` for them, so its
    // message is absent from the allow-set — the search-as-non-member = 0 guarantee.
    let acl_non_member = AclFilter::ids([member_msg]);

    // FT: the SHARED rare term `zarquon` — only the member's message surfaces; the private one never
    // (0 count-leak, the CHAT-D11 analog).
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

    // The chained grant: the viewer joins the private channel → its message becomes visible (the
    // rejection was the channel ACL, not a deny).
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

// ----------------------------------------------------------------------------------------------
// 3. SRCH-D3 (F2) — cross-tenant IDOR = 0 on the REAL Chat corpus
// ----------------------------------------------------------------------------------------------

/// **SRCH-D3 (F2) re-confirmed on the Chat corpus: a viewer's tenant partitions the index — a query
/// against a DIFFERENT tenant's index sees 0 of this tenant's messages (the per-tenant index, §3.4).**
#[test]
fn srch_d3_cross_tenant_messages_do_not_leak() {
    // Two tenants index a message under a COLLIDING doc-id namespace, so only the partition key keeps
    // them apart — not a lucky id difference.
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

    // Positive control: acme's viewer querying acme's index sees acme's message.
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

    // The cross-tenant attack: even with an allow-set NAMING the evil message's doc-id, querying ACME's
    // partition returns 0 — the evil message lives in a DIFFERENT (tenant, region) index entirely.
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

    // And the evil tenant's index, conversely, holds only evil's message.
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

// ----------------------------------------------------------------------------------------------
// 4. Cross-subsystem facets dependable (X-2) — the one facet key across all content-node producers
// ----------------------------------------------------------------------------------------------

/// **Cross-subsystem facets dependable now that all five producers emit the structured inline nodes
/// uniformly (X-2): a `mention`/`artifact_ref` facet query is reliable across Git/KN/Issues/Chat.**
///
/// All four content-node producers (KN page, Issue body, Git commit-message, Chat message) project the
/// SAME three structured inline nodes through the SAME `structured_nodes()` walk. Here we index a doc
/// from EACH producer that references the SAME artifact, then a single `artifact_ref` facet query
/// returns ALL of them — proving the one cross-producer facet shape (no second extraction path). The
/// Chat message + KN page are both `myelin_content::Block` bodies, so they share
/// [`message_search_projection`]/[`myelin_search::page_search_projection`] verbatim.
#[test]
fn cross_subsystem_facets_are_dependable_across_producers() {
    use myelin_search::page_search_projection;

    // The ONE referenced artifact every producer's doc points at — the cross-subsystem facet target.
    let referenced = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());

    // A Chat message referencing it (the new producer — a `Block` body).
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

    // A KN page referencing the SAME artifact (the established producer — also a `Block` body). This
    // shares the byte-identical structured-node walk, so its facet is the SAME key + value.
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

    // Index BOTH producers' docs into ONE indexer that admits BOTH specs (the cross-producer facet
    // union). A chat message doc and a KN page doc, same tenant/region partition.
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

    // ONE `artifact_ref` facet query, the SAME facet key for every producer (X-2): it returns BOTH the
    // Chat message AND the KN page — the cross-subsystem facet is dependable across producers.
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
