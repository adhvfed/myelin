use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use crate::store::ConversationId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConversationKind {
    ChannelPublic,
    ChannelPrivate,
    Dm,
    GroupDm,
    ArtifactLinked,
    Announcement,
}

impl ConversationKind {
    pub fn as_token(self) -> &'static str {
        match self {
            ConversationKind::ChannelPublic => "channel_public",
            ConversationKind::ChannelPrivate => "channel_private",
            ConversationKind::Dm => "dm",
            ConversationKind::GroupDm => "group_dm",
            ConversationKind::ArtifactLinked => "artifact_linked",
            ConversationKind::Announcement => "announcement",
        }
    }

    pub fn from_token(token: &str) -> Option<ConversationKind> {
        Some(match token {
            "channel_public" => ConversationKind::ChannelPublic,
            "channel_private" => ConversationKind::ChannelPrivate,
            "dm" => ConversationKind::Dm,
            "group_dm" => ConversationKind::GroupDm,
            "artifact_linked" => ConversationKind::ArtifactLinked,
            "announcement" => ConversationKind::Announcement,
            _ => return None,
        })
    }

    pub fn membership_is_acl(self) -> bool {
        !matches!(self, ConversationKind::ChannelPublic)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MembershipRole {
    Member,
    Admin,
}

impl MembershipRole {
    pub fn as_token(self) -> &'static str {
        match self {
            MembershipRole::Member => "member",
            MembershipRole::Admin => "admin",
        }
    }

    pub fn from_token(token: &str) -> Option<MembershipRole> {
        Some(match token {
            "member" => MembershipRole::Member,
            "admin" => MembershipRole::Admin,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversation {
    pub id: ConversationId,
    pub kind: ConversationKind,
    pub home_cell: String,
    pub parent_project: Option<String>,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub linked_ref: Option<String>,
    pub pinned_canvas: Option<String>,
    pub retention_days: Option<i32>,
    pub archived: bool,
    pub created_by: String,
    pub acl_zookie: Option<String>,
}

impl Conversation {
    pub fn home_cell_for(id: &ConversationId) -> String {
        format!("{}:{}", id.region, id.tenant)
    }

    pub fn not_single_cell_foreclosed(&self) -> bool {
        let conventional = Conversation::home_cell_for(&self.id);
        let mut probe = self.clone();
        probe.home_cell = format!("other-cell:{conventional}");
        probe.home_cell != conventional && !probe.home_cell.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Membership {
    pub conv: ConversationId,
    pub principal_id: String,
    pub role: MembershipRole,
    pub is_watcher: bool,
    pub notif_pref: serde_json::Value,
}

impl Membership {
    pub fn member(conv: ConversationId, principal_id: impl Into<String>) -> Membership {
        Membership {
            conv,
            principal_id: principal_id.into(),
            role: MembershipRole::Member,
            is_watcher: true,
            notif_pref: serde_json::Value::Object(Default::default()),
        }
    }

    pub fn admin(conv: ConversationId, principal_id: impl Into<String>) -> Membership {
        Membership {
            role: MembershipRole::Admin,
            ..Membership::member(conv, principal_id)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConversationError {
    NotFound(String),
    AlreadyExists(String),
    SchemaViolation(String),
    Storage(String),
}

impl core::fmt::Display for ConversationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConversationError::NotFound(id) => write!(f, "conversation {id} not found"),
            ConversationError::AlreadyExists(id) => write!(f, "conversation {id} already exists"),
            ConversationError::SchemaViolation(e) => write!(f, "schema violation: {e}"),
            ConversationError::Storage(e) => write!(f, "conversation storage failed: {e}"),
        }
    }
}

impl std::error::Error for ConversationError {}

pub type Result<T> = core::result::Result<T, ConversationError>;

pub trait ConversationStore {
    fn create(&self, conv: Conversation) -> Result<()>;

    fn get(&self, id: &ConversationId) -> Result<Conversation>;

    fn join(&self, m: Membership) -> Result<()>;

    fn leave(&self, conv: &ConversationId, principal_id: &str) -> Result<()>;

    fn conversations_of(&self, tenant: &str, principal_id: &str) -> Result<Vec<ConversationId>>;

    fn members_of(&self, conv: &ConversationId) -> Result<Vec<Membership>>;

    fn stamp_acl_zookie(&self, conv: &ConversationId, zookie: &str) -> Result<()>;
}

#[derive(Default)]
pub struct MemConversationStore {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    conversations: BTreeMap<ConversationId, Conversation>,
    members: BTreeMap<ConversationId, BTreeMap<String, Membership>>,
    by_principal: BTreeMap<(String, String), BTreeSet<ConversationId>>,
}

impl MemConversationStore {
    pub fn new() -> MemConversationStore {
        MemConversationStore::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl ConversationStore for MemConversationStore {
    fn create(&self, conv: Conversation) -> Result<()> {
        let mut inner = self.lock();
        if inner.conversations.contains_key(&conv.id) {
            return Err(ConversationError::AlreadyExists(
                conv.id.conversation_id.clone(),
            ));
        }
        inner.conversations.insert(conv.id.clone(), conv);
        Ok(())
    }

    fn get(&self, id: &ConversationId) -> Result<Conversation> {
        self.lock()
            .conversations
            .get(id)
            .cloned()
            .ok_or_else(|| ConversationError::NotFound(id.conversation_id.clone()))
    }

    fn join(&self, m: Membership) -> Result<()> {
        let mut inner = self.lock();
        if !inner.conversations.contains_key(&m.conv) {
            return Err(ConversationError::NotFound(m.conv.conversation_id.clone()));
        }
        let idx_key = (m.conv.tenant.clone(), m.principal_id.clone());
        inner
            .by_principal
            .entry(idx_key)
            .or_default()
            .insert(m.conv.clone());
        inner
            .members
            .entry(m.conv.clone())
            .or_default()
            .insert(m.principal_id.clone(), m);
        Ok(())
    }

    fn leave(&self, conv: &ConversationId, principal_id: &str) -> Result<()> {
        let mut inner = self.lock();
        if let Some(members) = inner.members.get_mut(conv) {
            members.remove(principal_id);
            if members.is_empty() {
                inner.members.remove(conv);
            }
        }
        let idx_key = (conv.tenant.clone(), principal_id.to_string());
        if let Some(set) = inner.by_principal.get_mut(&idx_key) {
            set.remove(conv);
            if set.is_empty() {
                inner.by_principal.remove(&idx_key);
            }
        }
        Ok(())
    }

    fn conversations_of(&self, tenant: &str, principal_id: &str) -> Result<Vec<ConversationId>> {
        let inner = self.lock();
        let idx_key = (tenant.to_string(), principal_id.to_string());
        Ok(inner
            .by_principal
            .get(&idx_key)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default())
    }

    fn members_of(&self, conv: &ConversationId) -> Result<Vec<Membership>> {
        let inner = self.lock();
        Ok(inner
            .members
            .get(conv)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default())
    }

    fn stamp_acl_zookie(&self, conv: &ConversationId, zookie: &str) -> Result<()> {
        let mut inner = self.lock();
        match inner.conversations.get_mut(conv) {
            Some(c) => {
                c.acl_zookie = Some(zookie.to_string());
                Ok(())
            }
            None => Err(ConversationError::NotFound(conv.conversation_id.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv_id(conversation_id: &str) -> ConversationId {
        ConversationId::new("acme", "fr-par", conversation_id)
    }

    fn sample(kind: ConversationKind, id: &str) -> Conversation {
        let id = conv_id(id);
        Conversation {
            home_cell: Conversation::home_cell_for(&id),
            id,
            kind,
            parent_project: Some("proj-1".into()),
            name: Some("general".into()),
            topic: Some("the topic".into()),
            linked_ref: None,
            pinned_canvas: None,
            retention_days: Some(90),
            archived: false,
            created_by: "psn:creator".into(),
            acl_zookie: None,
        }
    }

    #[test]
    fn conversation_round_trips() {
        let store = MemConversationStore::new();
        let mut c = sample(ConversationKind::ArtifactLinked, "01J0CONV");
        c.linked_ref = Some("issue/ABC-1".into());
        c.retention_days = Some(30);
        store.create(c.clone()).unwrap();

        let got = store.get(&c.id).unwrap();
        assert_eq!(got, c, "the conversation round-trips verbatim");
        assert_eq!(got.kind, ConversationKind::ArtifactLinked);
        assert_eq!(got.retention_days, Some(30));
        assert_eq!(got.linked_ref.as_deref(), Some("issue/ABC-1"));

        assert_eq!(
            store.create(c.clone()),
            Err(ConversationError::AlreadyExists("01J0CONV".into()))
        );
    }

    #[test]
    fn kind_discrimination_round_trips() {
        for kind in [
            ConversationKind::ChannelPublic,
            ConversationKind::ChannelPrivate,
            ConversationKind::Dm,
            ConversationKind::GroupDm,
            ConversationKind::ArtifactLinked,
            ConversationKind::Announcement,
        ] {
            assert_eq!(ConversationKind::from_token(kind.as_token()), Some(kind));
        }
        assert_eq!(ConversationKind::from_token("not_a_kind"), None);
        assert!(!ConversationKind::ChannelPublic.membership_is_acl());
        assert!(ConversationKind::ChannelPrivate.membership_is_acl());
        assert!(ConversationKind::Dm.membership_is_acl());
        for role in [MembershipRole::Member, MembershipRole::Admin] {
            assert_eq!(MembershipRole::from_token(role.as_token()), Some(role));
        }
        assert_eq!(MembershipRole::from_token("owner"), None);
    }

    #[test]
    fn membership_by_principal_index_is_exact() {
        let store = MemConversationStore::new();
        for (id, _) in [("c1", ()), ("c2", ()), ("c3", ())] {
            store
                .create(sample(ConversationKind::ChannelPrivate, id))
                .unwrap();
        }
        store
            .join(Membership::member(conv_id("c1"), "alice"))
            .unwrap();
        store
            .join(Membership::admin(conv_id("c3"), "alice"))
            .unwrap();
        store
            .join(Membership::member(conv_id("c2"), "bob"))
            .unwrap();
        store
            .join(Membership::member(conv_id("c3"), "bob"))
            .unwrap();

        let alice = store.conversations_of("acme", "alice").unwrap();
        assert_eq!(
            alice,
            vec![conv_id("c1"), conv_id("c3")],
            "Alice's list is EXACTLY c1 + c3 (0 missing, 0 extra)"
        );
        let bob = store.conversations_of("acme", "bob").unwrap();
        assert_eq!(bob, vec![conv_id("c2"), conv_id("c3")]);

        assert!(store.conversations_of("acme", "carol").unwrap().is_empty());

        let c3_members: Vec<_> = store
            .members_of(&conv_id("c3"))
            .unwrap()
            .into_iter()
            .map(|m| m.principal_id)
            .collect();
        assert_eq!(c3_members, vec!["alice".to_string(), "bob".to_string()]);

        store.leave(&conv_id("c3"), "alice").unwrap();
        assert_eq!(
            store.conversations_of("acme", "alice").unwrap(),
            vec![conv_id("c1")],
            "after leave the index drops c3 (0 stale rows)"
        );
        assert_eq!(
            store.conversations_of("acme", "bob").unwrap(),
            vec![conv_id("c2"), conv_id("c3")],
            "Bob is untouched by Alice's leave"
        );
        store.leave(&conv_id("c3"), "alice").unwrap();
        store.leave(&conv_id("c1"), "nobody").unwrap();
    }

    #[test]
    fn membership_index_is_residency_pinned() {
        let store = MemConversationStore::new();
        let fr = ConversationId::new("acme", "fr-par", "conv");
        let de = ConversationId::new("acme", "de-fra", "conv");
        store
            .create(Conversation {
                home_cell: Conversation::home_cell_for(&fr),
                id: fr.clone(),
                kind: ConversationKind::ChannelPrivate,
                parent_project: None,
                name: Some("fr".into()),
                topic: None,
                linked_ref: None,
                pinned_canvas: None,
                retention_days: None,
                archived: false,
                created_by: "psn:c".into(),
                acl_zookie: None,
            })
            .unwrap();
        store
            .create(Conversation {
                home_cell: Conversation::home_cell_for(&de),
                id: de.clone(),
                kind: ConversationKind::ChannelPrivate,
                parent_project: None,
                name: Some("de".into()),
                topic: None,
                linked_ref: None,
                pinned_canvas: None,
                retention_days: None,
                archived: false,
                created_by: "psn:c".into(),
                acl_zookie: None,
            })
            .unwrap();
        store.join(Membership::member(fr.clone(), "alice")).unwrap();
        store.join(Membership::member(de.clone(), "alice")).unwrap();
        let list = store.conversations_of("acme", "alice").unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&fr) && list.contains(&de));
        assert_ne!(
            fr, de,
            "fr-par and de-fra are distinct residency-pinned keys"
        );
    }

    #[test]
    fn conversation_does_not_foreclose_multi_cell() {
        let store = MemConversationStore::new();
        let c = sample(ConversationKind::ChannelPrivate, "c1");
        store.create(c.clone()).unwrap();

        assert!(
            c.not_single_cell_foreclosed(),
            "the home cell is a value, not a baked single-cell assumption"
        );

        let mut cross = sample(ConversationKind::ChannelPrivate, "c2");
        cross.home_cell = "de-fra:other-org".into();
        store.create(cross.clone()).unwrap();
        assert_eq!(store.get(&cross.id).unwrap().home_cell, "de-fra:other-org");

        store
            .join(Membership::member(
                c.id.clone(),
                "psn:foreign-tenant-principal",
            ))
            .unwrap();
        let members = store.members_of(&c.id).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].principal_id, "psn:foreign-tenant-principal");
    }
}
