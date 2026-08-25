use std::collections::HashMap;

use myelin_chat::store::{AuthorKind, Message, MessageId, MessageState};
use myelin_chat::{
    decode_encrypted_body, decrypt_body, encode_encrypted_body, encrypt_body,
    is_chat_subject_key_class, ChatFreeText,
};
use myelin_content::{InlineNode, OBJ};
use myelin_identity::Principal;
use myelin_storage::{KmsEngine, SubjectId};
use serde_json::{json, Value};

use crate::{EdgeError, ReferenceCard};

pub(super) struct ReadableMessage<'a> {
    stored: &'a Message,
    pub(super) content: String,
    pub(super) nodes: Vec<InlineNode>,
}

impl ReadableMessage<'_> {
    pub(super) fn reference_nodes(&self) -> impl Iterator<Item = String> + '_ {
        self.nodes.iter().filter_map(|node| match node {
            InlineNode::ArtifactRefNode(reference) | InlineNode::Embed(reference) => {
                Some(reference.0.clone())
            }
            InlineNode::Mention(_) => None,
        })
    }
}

pub(super) fn decode_readable_message<'a>(
    message: &'a Message,
    kms: &KmsEngine,
) -> Result<ReadableMessage<'a>, EdgeError> {
    if matches!(
        message.state,
        MessageState::Deleted | MessageState::Tombstoned
    ) {
        return Ok(ReadableMessage {
            stored: message,
            content: String::new(),
            nodes: Vec::new(),
        });
    }
    let content = decrypt_message_column(message, kms, &message.body_inline, "body_inline")?;
    let content = std::str::from_utf8(&content)
        .map_err(|_| EdgeError::Internal("stored Chat message is not valid UTF-8".into()))?
        .to_string();
    let nodes = decode_message_nodes(message, kms)?;
    if content
        .chars()
        .filter(|character| *character == OBJ)
        .count()
        != nodes.len()
    {
        return Err(EdgeError::Internal(
            "stored Chat content and structured nodes disagree".into(),
        ));
    }
    Ok(ReadableMessage {
        stored: message,
        content,
        nodes,
    })
}

pub(super) fn readable_message_json(
    message: &ReadableMessage<'_>,
    reply_count: u64,
    viewer: &str,
    cards: &HashMap<String, ReferenceCard>,
) -> Value {
    let stored = message.stored;
    json!({
        "id": stored.message_id.as_str(),
        "author": stored.author,
        "author_kind": match stored.author_kind {
            AuthorKind::Human => "human",
            AuthorKind::Agent => "agent",
            AuthorKind::Service => "service",
        },
        "is_you": stored.author == viewer,
        "content": message.content,
        "nodes": message.nodes.iter().map(|node| message_node_json(node, cards)).collect::<Vec<_>>(),
        "thread_root_id": stored.thread_root_id.as_ref().map(MessageId::as_str),
        "reply_count": reply_count,
        "edited": stored.edited_seq > 0,
        "state": stored.state.token(),
        "created_at": stored.message_id.timestamp_ms().map(|value| value / 1000),
    })
}

fn decode_message_nodes(message: &Message, kms: &KmsEngine) -> Result<Vec<InlineNode>, EdgeError> {
    // The first public Edge floor wrote only an empty plaintext node array.
    // It contains no personal data and remains readable during the rolling
    // transition; every newly written node array is encrypted.
    if message.body_nodes.is_empty() || message.body_nodes == b"[]" {
        return Ok(Vec::new());
    }
    let node_bytes = decrypt_message_column(message, kms, &message.body_nodes, "body_nodes")?;
    serde_json::from_slice(&node_bytes)
        .map_err(|_| EdgeError::Internal("stored Chat structured nodes are not valid".into()))
}

pub(super) fn message_node_json(
    node: &InlineNode,
    cards: &HashMap<String, ReferenceCard>,
) -> Value {
    match node {
        InlineNode::Mention(principal) => json!({
            "kind": "mention",
            "principal_id": principal.principal_id.0,
        }),
        InlineNode::ArtifactRefNode(reference) => {
            reference_node_json("artifact_ref", reference, cards)
        }
        InlineNode::Embed(reference) => reference_node_json("embed", reference, cards),
    }
}

fn reference_node_json(
    kind: &str,
    reference: &myelin_refs::ArtifactRef,
    cards: &HashMap<String, ReferenceCard>,
) -> Value {
    json!({
        "kind": kind,
        "ref": reference.0,
        "card": cards.get(&reference.0).unwrap_or(&ReferenceCard::Tombstone),
    })
}

fn decrypt_message_column(
    message: &Message,
    kms: &KmsEngine,
    encoded: &[u8],
    column: &str,
) -> Result<Vec<u8>, EdgeError> {
    let encrypted = decode_encrypted_body(encoded).map_err(|_| {
        EdgeError::Internal(format!(
            "stored Chat message has an invalid encrypted {column}"
        ))
    })?;
    if encrypted.key_ref.tenant.as_str() != message.conv.tenant
        || !is_chat_subject_key_class(&encrypted.key_ref.class, &message.author)
    {
        return Err(EdgeError::Internal(
            "stored Chat message encryption scope does not match its author".into(),
        ));
    }
    decrypt_body(
        kms,
        &myelin_tenancy::Region(message.conv.region.clone()),
        &encrypted,
    )
    .map_err(|_| EdgeError::Internal(format!("stored Chat {column} cannot be decrypted")))
}

pub(super) fn encrypt_message_column(
    kms: &KmsEngine,
    principal: &Principal,
    author: &str,
    kind: ChatFreeText,
    plaintext: &[u8],
) -> Result<Vec<u8>, EdgeError> {
    let column = encrypt_body(
        kms,
        &principal.region,
        &principal.tenant,
        &SubjectId::new(author),
        kind,
        plaintext,
    )
    .map_err(|error| EdgeError::Internal(format!("Chat message encryption failed: {error}")))?;
    encode_encrypted_body(&column)
        .map_err(|error| EdgeError::Internal(format!("Chat message encoding failed: {error}")))
}
