use std::collections::{HashMap, HashSet};

use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal, Zookie,
};
use myelin_notif::{render_message, TemplateStore, DEFAULT_LOCALE, PLATFORM_DEFAULT_TENANT};
use myelin_refs::ArtifactRef;

use crate::glue::{TPL_CHAT_PROJECT_CHANNEL, TPL_CHAT_PROJECT_MESSAGE, TPL_CHAT_PROJECT_THREAD};
use crate::membership::{channel_object, permissions};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderHint {
    ChannelChip,
    MessageChip,
    ThreadChip,
}

impl RenderHint {
    pub fn as_str(self) -> &'static str {
        match self {
            RenderHint::ChannelChip => "ChannelChip",
            RenderHint::MessageChip => "MessageChip",
            RenderHint::ThreadChip => "ThreadChip",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            RenderHint::ChannelChip => "channel",
            RenderHint::MessageChip => "message",
            RenderHint::ThreadChip => "thread",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub title: String,
    pub state: String,
    pub icon: String,
    pub render_hint: RenderHint,
    pub sub_anchor: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    Denied,
    Gone,
    Erased,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    pub root: ArtifactRef,
    pub reason: TombstoneReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Projected {
    Visible(Projection),
    Tombstoned(Tombstone),
}

impl Projected {
    pub fn is_tombstone(&self) -> bool {
        matches!(self, Projected::Tombstoned(_))
    }

    pub fn title(&self) -> Option<&str> {
        match self {
            Projected::Visible(p) => Some(&p.title),
            Projected::Tombstoned(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    NotChat { subsystem: String },
    UnknownChatType { ty: String },
    Malformed { reference: String },
}

impl core::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProjectError::NotChat { subsystem } => {
                write!(f, "not a chat artifact (subsystem `{subsystem}`)")
            }
            ProjectError::UnknownChatType { ty } => {
                write!(
                    f,
                    "`{ty}` is not a projectable chat type (channel/message/thread)"
                )
            }
            ProjectError::Malformed { reference } => {
                write!(f, "malformed chat ref `{reference}`")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChatType {
    Channel,
    Message,
    Thread,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelMeta {
    pub label: String,
    pub archived: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageMeta {
    pub channel_id: String,
    pub preview: String,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadMeta {
    pub channel_id: String,
    pub root_preview: String,
    pub reply_count: u32,
    pub state: String,
}

#[derive(Clone, Debug, Default)]
pub struct ChatProjectionSource {
    channels: HashMap<String, ChannelMeta>,
    messages: HashMap<String, MessageMeta>,
    threads: HashMap<String, ThreadMeta>,
    erased: HashSet<String>,
    restricted: HashSet<String>,
}

impl ChatProjectionSource {
    pub fn new() -> ChatProjectionSource {
        ChatProjectionSource::default()
    }

    pub fn put_channel(&mut self, root: &ArtifactRef, meta: ChannelMeta) {
        self.channels.insert(root.0.clone(), meta);
    }

    pub fn put_message(&mut self, root: &ArtifactRef, meta: MessageMeta) {
        self.messages.insert(root.0.clone(), meta);
    }

    pub fn put_thread(&mut self, root: &ArtifactRef, meta: ThreadMeta) {
        self.threads.insert(root.0.clone(), meta);
    }

    pub fn mark_erased(&mut self, reference: &ArtifactRef) {
        self.erased.insert(reference.0.clone());
    }

    pub fn mark_restricted(&mut self, reference: &ArtifactRef) {
        self.restricted.insert(reference.0.clone());
    }
}

pub struct Projector<I: IdentityService> {
    id: I,
    source: ChatProjectionSource,
    templates: TemplateStore,
}

impl<I: IdentityService> Projector<I> {
    pub fn new(id: I, source: ChatProjectionSource, templates: TemplateStore) -> Projector<I> {
        Projector {
            id,
            source,
            templates,
        }
    }

    pub fn source_mut(&mut self) -> &mut ChatProjectionSource {
        &mut self.source
    }

    pub fn project(
        &self,
        reference: &ArtifactRef,
        viewer: &Principal,
        zookie: Zookie,
    ) -> Result<Projected, ProjectError> {
        let ty = classify(reference)?;
        let root = myelin_refs::strip_sub(reference);

        let channel_id = match self.gate_channel_id(ty, &root) {
            Some(c) => c,
            None => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Gone,
                    root,
                }));
            }
        };

        let object = myelin_tenancy::ArtifactRef(channel_object(&channel_id));
        let at = Consistency {
            at_least: zookie,
            mode: ConsistencyMode::Strong,
        };
        let permission = Permission(permissions::READ.to_string());
        match self.id.check(viewer, &permission, &object, &at, None) {
            Ok(Decision::Allow) => {}
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Denied,
                    root,
                }));
            }
        }

        if self.source.erased.contains(&root.0) || self.source.erased.contains(&reference.0) {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Erased,
                root,
            }));
        }
        if self.source.restricted.contains(&root.0) || self.source.restricted.contains(&reference.0)
        {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Erased,
                root,
            }));
        }

        let sub_anchor = sub_opaque(reference);
        match ty {
            ChatType::Channel => {
                let meta = match self.source.channels.get(&root.0) {
                    Some(m) => m.clone(),
                    None => return Ok(self.gone(root)),
                };
                let title = self.humanise_title(TPL_CHAT_PROJECT_CHANNEL, vec![meta.label], viewer);
                Ok(Projected::Visible(Projection {
                    title,
                    state: channel_state(meta.archived).to_string(),
                    icon: RenderHint::ChannelChip.icon().to_string(),
                    render_hint: RenderHint::ChannelChip,
                    sub_anchor,
                }))
            }
            ChatType::Message => {
                let meta = match self.source.messages.get(&root.0) {
                    Some(m) => m.clone(),
                    None => return Ok(self.gone(root)),
                };
                let title =
                    self.humanise_title(TPL_CHAT_PROJECT_MESSAGE, vec![meta.preview], viewer);
                Ok(Projected::Visible(Projection {
                    title,
                    state: meta.state,
                    icon: RenderHint::MessageChip.icon().to_string(),
                    render_hint: RenderHint::MessageChip,
                    sub_anchor,
                }))
            }
            ChatType::Thread => {
                let meta = match self.source.threads.get(&root.0) {
                    Some(m) => m.clone(),
                    None => return Ok(self.gone(root)),
                };
                let title = self.humanise_title(
                    TPL_CHAT_PROJECT_THREAD,
                    vec![meta.root_preview, meta.reply_count.to_string()],
                    viewer,
                );
                Ok(Projected::Visible(Projection {
                    title,
                    state: meta.state,
                    icon: RenderHint::ThreadChip.icon().to_string(),
                    render_hint: RenderHint::ThreadChip,
                    sub_anchor,
                }))
            }
        }
    }

    fn gate_channel_id(&self, ty: ChatType, root: &ArtifactRef) -> Option<String> {
        match ty {
            ChatType::Channel => ref_id(root),
            ChatType::Message => self
                .source
                .messages
                .get(&root.0)
                .map(|m| m.channel_id.clone()),
            ChatType::Thread => self
                .source
                .threads
                .get(&root.0)
                .map(|m| m.channel_id.clone()),
        }
    }

    fn humanise_title(&self, key: &str, slots: Vec<String>, _viewer: &Principal) -> String {
        let body = self
            .templates
            .lookup(PLATFORM_DEFAULT_TENANT, key, DEFAULT_LOCALE)
            .map(|t| t.body.clone())
            .unwrap_or_else(|| slots.first().cloned().unwrap_or_default());
        render_message(&body, &slots)
    }

    fn gone(&self, root: ArtifactRef) -> Projected {
        Projected::Tombstoned(Tombstone {
            reason: TombstoneReason::Gone,
            root,
        })
    }
}

fn channel_state(archived: bool) -> &'static str {
    if archived {
        "archived"
    } else {
        "active"
    }
}

fn classify(reference: &ArtifactRef) -> Result<ChatType, ProjectError> {
    let root = myelin_refs::strip_sub(reference);
    let segments = scope_segments(&root).ok_or_else(|| ProjectError::Malformed {
        reference: reference.0.clone(),
    })?;
    let (subsystem, ty) = (segments.1, segments.2);
    if subsystem.as_str() != crate::subs::CHAT_SUBSYSTEM {
        return Err(ProjectError::NotChat { subsystem });
    }
    match ty.as_str() {
        "channel" => Ok(ChatType::Channel),
        "message" => Ok(ChatType::Message),
        "thread" => Ok(ChatType::Thread),
        _ => Err(ProjectError::UnknownChatType { ty }),
    }
}

fn scope_segments(root: &ArtifactRef) -> Option<(String, String, String, String)> {
    let rest = root.0.strip_prefix(myelin_refs::SCHEME)?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() != 4 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    Some((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
        parts[3].to_string(),
    ))
}

fn ref_id(root: &ArtifactRef) -> Option<String> {
    scope_segments(root).map(|s| s.3)
}

fn sub_opaque(reference: &ArtifactRef) -> Option<String> {
    use myelin_refs::Sub;
    match myelin_refs::sub_kind(reference)? {
        Sub::Message(id) | Sub::Thread(id) => Some(id),
        Sub::Comment(id)
        | Sub::Block(id)
        | Sub::Heading(id)
        | Sub::Row(id)
        | Sub::Field(id)
        | Sub::Check(id) => Some(id),
        Sub::CommitCheck {
            commit_oid,
            context,
        } => Some(format!("commit-{commit_oid}/check-{context}")),
        Sub::CommitCiResult { commit_oid } => Some(format!("commit-{commit_oid}/ci-result")),
        Sub::Step(n) => Some(n.to_string()),
        Sub::LineRange { start, end } => Some(format!("L{start}-L{end}")),
    }
}

pub fn densest_edge_producer(
    source: &ArtifactRef,
    corpus: &[crate::content::MessageBody],
) -> usize {
    corpus
        .iter()
        .map(|body| crate::content::extract_message_edges(source, body).len())
        .sum()
}

#[cfg(test)]
mod tests;
