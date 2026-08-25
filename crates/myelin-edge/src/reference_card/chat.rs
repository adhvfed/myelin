use std::collections::{BTreeMap, BTreeSet, HashMap};

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
        let message_roots = exact_references(viewer, references, ExactReferenceKind::Message);
        let thread_roots = exact_references(viewer, references, ExactReferenceKind::Thread);
        if conversation_roots.is_empty() && message_roots.is_empty() && thread_roots.is_empty() {
            return HashMap::new();
        }

        let mut cards = claimed_tombstones(&conversation_roots);
        cards.extend(claimed_exact_tombstones(&message_roots));
        cards.extend(claimed_exact_tombstones(&thread_roots));
        self.project_conversations(viewer, &conversation_roots, &mut cards);
        self.project_exact_chat(viewer, &message_roots, &thread_roots, &mut cards);
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

    fn project_exact_chat(
        &self,
        viewer: &Principal,
        message_roots: &BTreeMap<String, Vec<OwnedExactReference>>,
        thread_roots: &BTreeMap<String, Vec<OwnedExactReference>>,
        cards: &mut HashMap<String, ReferenceCard>,
    ) {
        let messages = canonical_exact_references(message_roots, ExactReferenceKind::Message);
        let threads = canonical_exact_references(thread_roots, ExactReferenceKind::Thread);
        let message_ids = messages
            .keys()
            .chain(threads.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(MessageId)
            .collect::<Vec<_>>();
        let Ok(visible) = self.chat.project_messages(viewer, &message_ids) else {
            return;
        };

        for message in visible {
            if let Some(references) = messages.get(&message.message_id) {
                insert_exact_cards(
                    cards,
                    references,
                    format!("Message in {}", message.conversation_topic),
                    message.state.token(),
                    "message",
                    "chat_message",
                );
            }
            if message.thread_root_id.is_none() {
                if let Some(references) = threads.get(&message.message_id) {
                    insert_exact_cards(
                        cards,
                        references,
                        format!("Thread in {}", message.conversation_topic),
                        message.state.token(),
                        "message",
                        "chat_thread",
                    );
                }
            }
        }
    }
}

fn insert_exact_cards(
    cards: &mut HashMap<String, ReferenceCard>,
    references: &[OwnedExactReference],
    title: String,
    state: &str,
    icon: &str,
    render_hint: &str,
) {
    for reference in references {
        cards.insert(
            reference.original.clone(),
            ReferenceCard::projection_at(
                title.clone(),
                state,
                icon,
                render_hint,
                Some(reference.sub_anchor.clone()),
            ),
        );
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
struct OwnedExactReference {
    original: String,
    sub_anchor: String,
}

#[derive(Clone, Copy)]
enum ExactReferenceKind {
    Message,
    Thread,
}

impl ExactReferenceKind {
    const fn type_token(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Thread => "thread",
        }
    }

    fn anchor(self, id: &str) -> String {
        format!("{}-{id}", self.type_token())
    }

    fn sub_anchor(self, sub: Option<myelin_refs::Sub>, id: &str) -> Option<String> {
        match (self, sub) {
            (Self::Message, None) => Some(self.anchor(id)),
            (Self::Message, Some(myelin_refs::Sub::Message(sub_id))) => Some(self.anchor(&sub_id)),
            (Self::Thread, None) => Some(self.anchor(id)),
            (Self::Thread, Some(myelin_refs::Sub::Thread(sub_id))) => Some(self.anchor(&sub_id)),
            _ => None,
        }
    }
}

fn exact_references(
    viewer: &Principal,
    references: &[String],
    kind: ExactReferenceKind,
) -> BTreeMap<String, Vec<OwnedExactReference>> {
    let mut owned = BTreeMap::<String, Vec<OwnedExactReference>>::new();
    for reference in references {
        let Ok(parsed) = myelin_refs::parse_scoped(reference) else {
            continue;
        };
        if parsed.tenant != viewer.tenant
            || parsed.subsystem != "chat"
            || parsed.type_ != kind.type_token()
        {
            continue;
        }
        let Some(sub_anchor) = kind.sub_anchor(parsed.sub, &parsed.id) else {
            continue;
        };
        owned
            .entry(parsed.id)
            .or_default()
            .push(OwnedExactReference {
                original: reference.clone(),
                sub_anchor,
            });
    }
    owned
}

fn canonical_exact_references(
    roots: &BTreeMap<String, Vec<OwnedExactReference>>,
    kind: ExactReferenceKind,
) -> BTreeMap<String, Vec<OwnedExactReference>> {
    roots
        .iter()
        .filter_map(|(message_id, references)| {
            if !myelin_chat::is_canonical_ulid(message_id) {
                return None;
            }
            let expected_anchor = kind.anchor(message_id);
            let canonical = references
                .iter()
                .filter(|reference| reference.sub_anchor == expected_anchor)
                .cloned()
                .collect::<Vec<_>>();
            (!canonical.is_empty()).then(|| (message_id.clone(), canonical))
        })
        .collect()
}

fn claimed_exact_tombstones(
    roots: &BTreeMap<String, Vec<OwnedExactReference>>,
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

        let owned = exact_references(&viewer(), &references, ExactReferenceKind::Message);
        assert_eq!(owned.len(), 1);
        assert_eq!(owned.values().next().unwrap().len(), 3);
        assert_eq!(
            canonical_exact_references(&owned, ExactReferenceKind::Message),
            BTreeMap::from([(
                id.into(),
                vec![
                    OwnedExactReference {
                        original: root.clone(),
                        sub_anchor: format!("message-{id}"),
                    },
                    OwnedExactReference {
                        original: format!("{root}#message-{id}"),
                        sub_anchor: format!("message-{id}"),
                    },
                ]
            )])
        );
    }

    #[test]
    fn thread_roots_accept_only_their_matching_thread_anchor() {
        let id = "01J00000000000000000000000";
        let root = format!("myelin://acme/chat/thread/{id}");
        let references = [
            root.clone(),
            format!("{root}#thread-{id}"),
            format!("{root}#thread-01J00000000000000000000001"),
            format!("{root}#message-{id}"),
        ];
        let owned = exact_references(&viewer(), &references, ExactReferenceKind::Thread);
        assert_eq!(owned.values().next().unwrap().len(), 3);
        assert_eq!(
            canonical_exact_references(&owned, ExactReferenceKind::Thread),
            BTreeMap::from([(
                id.into(),
                vec![
                    OwnedExactReference {
                        original: root.clone(),
                        sub_anchor: format!("thread-{id}"),
                    },
                    OwnedExactReference {
                        original: format!("{root}#thread-{id}"),
                        sub_anchor: format!("thread-{id}"),
                    },
                ],
            )]),
        );
    }
}
