use std::collections::{BTreeMap, HashMap};

use myelin_chat::store::MessageId;
use myelin_identity::Principal;

use super::{claimed_tombstones, root_references, ReferenceCard, ReferenceCardProjector};
use crate::DurableChatReferenceApi;

pub(super) struct ChatReferenceCardProjector {
    chat: DurableChatReferenceApi,
}

impl ChatReferenceCardProjector {
    pub(super) fn new(chat: DurableChatReferenceApi) -> Self {
        Self { chat }
    }
}

impl ReferenceCardProjector for ChatReferenceCardProjector {
    fn project(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard> {
        let conversation_roots = root_references(viewer, references, "chat", "channel");
        let message_roots = message_references(viewer, references);
        if conversation_roots.is_empty() && message_roots.is_empty() {
            return HashMap::new();
        }

        let mut cards = claimed_tombstones(&conversation_roots);
        cards.extend(claimed_message_tombstones(&message_roots));
        self.project_conversations(viewer, &conversation_roots, &mut cards);
        self.project_messages(viewer, &message_roots, &mut cards);
        cards
    }
}

impl ChatReferenceCardProjector {
    fn project_conversations(
        &self,
        viewer: &Principal,
        roots: &BTreeMap<String, Vec<String>>,
        cards: &mut HashMap<String, ReferenceCard>,
    ) {
        let canonical = roots
            .iter()
            .filter(|(conversation_id, _)| myelin_chat::is_canonical_ulid(conversation_id))
            .map(|(conversation_id, references)| (conversation_id.clone(), references.clone()))
            .collect::<BTreeMap<_, _>>();
        let conversation_ids = canonical.keys().cloned().collect::<Vec<_>>();
        let Ok(visible) = self.chat.project_conversations(viewer, &conversation_ids) else {
            return;
        };

        for conversation in visible {
            let (Some(title), Some(references)) = (
                conversation.topic,
                canonical.get(&conversation.id.conversation_id),
            ) else {
                continue;
            };
            insert_card(
                cards,
                references,
                ReferenceCard::projection(title, "active", "chat", "chat_conversation"),
            );
        }
    }

    fn project_messages(
        &self,
        viewer: &Principal,
        roots: &BTreeMap<String, Vec<OwnedMessageReference>>,
        cards: &mut HashMap<String, ReferenceCard>,
    ) {
        let canonical = canonical_message_references(roots);
        let message_ids = canonical.keys().cloned().map(MessageId).collect::<Vec<_>>();
        let Ok(visible) = self.chat.project_messages(viewer, &message_ids) else {
            return;
        };

        for message in visible {
            let Some(references) = canonical.get(&message.message_id) else {
                continue;
            };
            let title = format!("Message in {}", message.conversation_topic);
            for reference in references {
                cards.insert(
                    reference.original.clone(),
                    ReferenceCard::projection_at(
                        title.clone(),
                        message.state.token(),
                        "message",
                        "chat_message",
                        Some(reference.sub_anchor.clone()),
                    ),
                );
            }
        }
    }
}

fn insert_card(
    cards: &mut HashMap<String, ReferenceCard>,
    references: &[String],
    card: ReferenceCard,
) {
    for reference in references {
        cards.insert(reference.clone(), card.clone());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OwnedMessageReference {
    original: String,
    sub_anchor: String,
}

fn message_references(
    viewer: &Principal,
    references: &[String],
) -> BTreeMap<String, Vec<OwnedMessageReference>> {
    let mut owned = BTreeMap::<String, Vec<OwnedMessageReference>>::new();
    for reference in references {
        let Ok(parsed) = myelin_refs::parse_scoped(reference) else {
            continue;
        };
        if parsed.tenant != viewer.tenant || parsed.subsystem != "chat" || parsed.type_ != "message"
        {
            continue;
        }
        let sub_anchor = match parsed.sub {
            None => format!("message-{}", parsed.id),
            Some(myelin_refs::Sub::Message(message_id)) => format!("message-{message_id}"),
            Some(_) => continue,
        };
        owned
            .entry(parsed.id)
            .or_default()
            .push(OwnedMessageReference {
                original: reference.clone(),
                sub_anchor,
            });
    }
    owned
}

fn canonical_message_references(
    roots: &BTreeMap<String, Vec<OwnedMessageReference>>,
) -> BTreeMap<String, Vec<OwnedMessageReference>> {
    roots
        .iter()
        .filter_map(|(message_id, references)| {
            if !myelin_chat::is_canonical_ulid(message_id) {
                return None;
            }
            let expected_anchor = format!("message-{message_id}");
            let canonical = references
                .iter()
                .filter(|reference| reference.sub_anchor == expected_anchor)
                .cloned()
                .collect::<Vec<_>>();
            (!canonical.is_empty()).then(|| (message_id.clone(), canonical))
        })
        .collect()
}

fn claimed_message_tombstones(
    roots: &BTreeMap<String, Vec<OwnedMessageReference>>,
) -> HashMap<String, ReferenceCard> {
    roots
        .values()
        .flatten()
        .map(|reference| (reference.original.clone(), ReferenceCard::Tombstone))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn viewer() -> Principal {
        Principal::stub(
            PrincipalId("reader".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    #[test]
    fn message_roots_and_their_matching_anchor_share_one_exact_coordinate() {
        let id = "01J00000000000000000000000";
        let root = format!("myelin://acme/chat/message/{id}");
        let references = [
            root.clone(),
            format!("{root}#message-{id}"),
            format!("{root}#message-01J00000000000000000000001"),
            format!("{root}#thread-{id}"),
            format!("myelin://other/chat/message/{id}"),
        ];

        let owned = message_references(&viewer(), &references);
        assert_eq!(owned.len(), 1);
        assert_eq!(owned.values().next().unwrap().len(), 3);
        assert_eq!(
            canonical_message_references(&owned),
            BTreeMap::from([(
                id.into(),
                vec![
                    OwnedMessageReference {
                        original: root.clone(),
                        sub_anchor: format!("message-{id}"),
                    },
                    OwnedMessageReference {
                        original: format!("{root}#message-{id}"),
                        sub_anchor: format!("message-{id}"),
                    },
                ]
            )])
        );
    }
}
