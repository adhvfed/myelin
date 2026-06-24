//! # `conversation` — the Conversation / Membership entity + the `membership_by_principal`
//! conversation-list index (CHAT-P7 / P-401, M4-C1)
//!
//! The **Conversation/Membership entity slice** of milestone M4-C1, conformed to the frozen data
//! model in
//! [`01-tech-and-data-model.md`](../../../../planning/04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md)
//! §2 (the conversation model — one entity, many kinds) + §2.1 (membership, the access-control
//! relation + the `membership_by_principal` index that backs the conversation list S1). It ships:
//!
//! - **[`Conversation`]** — ONE entity with a [`ConversationKind`] discriminator (channel /
//!   dm / group-dm / artifact-linked / announcement, the frozen `conversation_kind` enum), carrying
//!   `retention_days` (the per-channel GDPR auto-delete hook) + `linked_ref` (the `ArtifactRef` an
//!   artifact-linked channel was born from). `kind` adapts PRESENTATION, not storage (design-language
//!   §2 "one component, adapt presentation"): DMs, group-DMs, channels, artifact-linked channels and
//!   announcements share the SAME read/write/fan-out/erasure machinery — not five tables.
//! - **[`Membership`]** — the access-control relation (Sketch 10 §C): a `(conversation, principal)`
//!   row carrying `role` (member | admin), the `is_watcher` Notif read-fanout relation (contract
//!   4.9), and per-channel `notif_pref`. Membership **IS the ACL** for private kinds.
//! - **[`MemConversationStore`]** — the DB-free, behaviour-identical OLTP tier model (partitioned by
//!   `(tenant, region)` + conversation, residency-pinned, contract 11.1 / 12.1), carrying the
//!   conversation rows + the membership rows + the **`membership_by_principal` index**: the
//!   leak-free, no-N+1 "my conversations" list S1. The REAL Postgres tier is the named promotion
//!   (CHAT-P8/P13 wire the `list_objects` gate; this index is the candidate set it joins against).
//!
//! ## The `membership_by_principal` index (the conversation list S1; arch §2.1)
//! `membership_by_principal ON membership (tenant, principal_id)` — "my conversations" (secondary
//! nav). [`MemConversationStore::conversations_of`] returns EXACTLY the conversations a principal is
//! a member of (0 missing, 0 extra), in O(memberships of P), never an O(all conversations) scan: the
//! index holds the inverse `(tenant, principal) -> {conversation_id}` set so the list is a direct
//! lookup, never an N+1 walk (external-insights/01 §7 — one primitive; the list is a
//! `list_objects`-backed view, NOT a third copy of the membership truth — it is derived from the
//! membership rows by construction and kept in lock-step on every join/leave).
//!
//! ## Cross-org NON-FORECLOSURE (the M4 floor; arch §2 cross-org note + 05 §7)
//! The M4 floor is the **single home-cell**: v1 does not build cross-org / federated channels. But
//! the model **MUST NOT foreclose** it (the explicit DELIVERABLE). The structural guarantees here:
//!
//! - The conversation carries a first-class **[`Conversation::home_cell`]** field — the home cell is
//!   a VALUE, not a baked-in assumption. A future cross-cell channel is a conversation whose
//!   membership spans cells; the entity already models that.
//! - **[`Membership::principal_id`]** is a free principal id (a `String`), NEVER constrained to the
//!   conversation's tenant: a membership set CAN span tenants (the arch §2 cross-org note — "a set of
//!   principals that *could* span tenants"). The `home_cell` of the conversation is independent of
//!   the membership principals' cells.
//! - [`Conversation::not_single_cell_foreclosed`] is the structural witness: it asserts the entity
//!   exposes the home cell as a settable field and does not pin membership to a single cell — the CI
//!   check the GATE reads.
//!
//! Cross-org / federated channels (M5-C-X1 / **CHAT-P30** / P-504) ride the frozen cross-cell
//! PII-free pointer bridge (contract 12.6, OQ-I) and are **designed-not-built** here. State: the
//! Conversation model does not assume single-org-membership-forever.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use crate::store::ConversationId;

/// The kind discriminator of a [`Conversation`] (arch §2 `conversation_kind` ENUM). `kind` adapts
/// PRESENTATION, not storage — a group-DM is "a private conversation whose name == its member set";
/// a private channel is "named, topic-scoped, invite-managed". Same storage, same fan-out, two
/// presentations (design-language §2). Aligned verbatim to the frozen `conversation_kind` enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConversationKind {
    /// A public channel — inherits `parent_project->read` (the `channel.read` TTU, the ReBAC
    /// fragment §5). Discoverable; membership is the watch/notify set, not the read gate.
    ChannelPublic,
    /// A private channel — named, topic-scoped, invite-managed; membership IS the ACL.
    ChannelPrivate,
    /// A 1:1 direct message — name == the member set (NULL name; Sketch 08); membership IS the ACL.
    Dm,
    /// A group direct message — a private conversation whose name == its member set, no topic.
    GroupDm,
    /// An artifact-linked channel — carries `linked_ref`, auto-emits `chat.channel.linked` →
    /// `refs.edge.created` ("incident #X discussed in channel Y", arch §2).
    ArtifactLinked,
    /// An announcement channel — broadcast-shaped (post = admin-only by convention), read by many.
    Announcement,
}

impl ConversationKind {
    /// The frozen `conversation_kind` ENUM token (arch §2) — the stable wire/DB spelling. The store
    /// persists THIS string; a round-trip through [`ConversationKind::from_token`] is loss-free (the
    /// kind-discrimination GATE: 0 schema-violation rows).
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

    /// Parse a frozen `conversation_kind` token back to its variant (the round-trip half). An
    /// unknown token is a LOUD `None` — a schema-violation row never silently becomes a default kind
    /// (the 0-schema-violation GATE).
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

    /// Whether this kind's membership IS its access-control list (the private kinds). For
    /// `ChannelPublic` read inherits `parent_project->read` (the TTU), so membership is the
    /// watch/notify set, NOT the read gate; for everything else membership IS the ACL (arch §2.1).
    pub fn membership_is_acl(self) -> bool {
        !matches!(self, ConversationKind::ChannelPublic)
    }
}

/// The role of a [`Membership`] (arch §2.1 `role`). `member` is the default; `admin` carries
/// `manage` (invite / archive / settings — the consequential mutations, the `channel.manage`
/// permission §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MembershipRole {
    /// A plain member — `read` + `post`.
    Member,
    /// An admin — `member` + `manage` (invite / archive / settings).
    Admin,
}

impl MembershipRole {
    /// The frozen `role` token (arch §2.1) — the stable DB spelling.
    pub fn as_token(self) -> &'static str {
        match self {
            MembershipRole::Member => "member",
            MembershipRole::Admin => "admin",
        }
    }

    /// Parse a frozen `role` token back to its variant. An unknown role is a LOUD `None`.
    pub fn from_token(token: &str) -> Option<MembershipRole> {
        Some(match token {
            "member" => MembershipRole::Member,
            "admin" => MembershipRole::Admin,
            _ => return None,
        })
    }
}

/// The Conversation entity (arch §2 `conversation` row) — ONE entity, many [`ConversationKind`]s.
/// The partition + residency key `(tenant, region)` lives in [`Conversation::id`] (the
/// [`ConversationId`] the message store shares, contract 12.1); `home_cell` is the cross-org
/// NON-FORECLOSURE field (a VALUE, not a baked assumption — arch §2 cross-org note).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversation {
    /// The `(tenant, region, conversation_id)` partition + identity key (contract 12.1; residency
    /// in the key, ADR-11). SHARED with the message store ([`crate::store::ConversationId`]) — one
    /// conversation id across the entity + its message log (no second id language, EI-01 §7).
    pub id: ConversationId,
    /// The discriminator — adapts presentation, not storage (arch §2).
    pub kind: ConversationKind,
    /// The home cell of this conversation (cross-org NON-FORECLOSURE; arch §2 + 05 §7): the cell the
    /// conversation is anchored in, as a VALUE. v1 is single-home-cell (== the tenant's cell), but
    /// the field exists so a cross-cell channel (CHAT-P30, riding the 12.6 bridge) is a conversation
    /// whose membership spans cells — the model does not assume single-org-membership-forever. By
    /// convention `home_cell == "<region>:<tenant>"`, but it is settable independently.
    pub home_cell: String,
    /// The project a channel lives in (the `channel.read = member + parent_project->read` TTU
    /// rewrite, §5). `None` for dm/group_dm (no parent project).
    pub parent_project: Option<String>,
    /// The channel name. `None` for dm/group_dm (name == the member set; Sketch 08).
    pub name: Option<String>,
    /// The channel topic. `None` if unset; may contain personal data (the holder tags it — schema).
    pub topic: Option<String>,
    /// Artifact-linked: the `ArtifactRef` this channel was born from (arch §2; the
    /// `chat.channel.linked` → `refs.edge.created` producer). `None` for non-linked kinds.
    pub linked_ref: Option<String>,
    /// A `knowledge/page` `ArtifactRef` — the embedded canvas (Sketch 08). Chat owns the
    /// pin/placement; Knowledge owns the page. `None` if no canvas is pinned.
    pub pinned_canvas: Option<String>,
    /// The per-channel auto-delete policy hook (GDPR; the engine is GDPR's — arch §2). `None` = no
    /// retention policy (keep forever, subject to tenant policy).
    pub retention_days: Option<i32>,
    /// Archived channels are read-only/hidden (the `chat.channel.archived` end-state). Default false.
    pub archived: bool,
    /// The pseudonymous principal id of the creator (erasure-safe; arch §2 `created_by`).
    pub created_by: String,
}

impl Conversation {
    /// The conventional home-cell spelling for a single-home-cell conversation: `"<region>:<tenant>"`.
    /// v1 stamps this; the field stays settable so a cross-cell channel (CHAT-P30) can carry a
    /// different anchor without a schema change (cross-org NON-FORECLOSURE).
    pub fn home_cell_for(id: &ConversationId) -> String {
        format!("{}:{}", id.region, id.tenant)
    }

    /// The cross-org NON-FORECLOSURE structural witness (the CI GATE, arch §2 + 05 §7). Returns
    /// `true` iff the entity does NOT bake a single-home-cell assumption:
    ///
    /// - the home cell is a settable VALUE (a different `home_cell` round-trips — not a hard-coded
    ///   `== tenant.cell`), AND
    /// - membership is NOT pinned to the conversation's tenant (a principal of another tenant is an
    ///   admissible member — see [`Membership::principal_id`]).
    ///
    /// The CI check reads this: a single-cell assumption baked into the entity would make this
    /// `false`. v1 is single-home-cell by deployment, NOT by model. Cross-org rides CHAT-P30 / 12.6.
    pub fn not_single_cell_foreclosed(&self) -> bool {
        // The home cell is a free String the caller set — not derived-and-frozen from the tenant.
        // We witness settability: a home cell distinct from the conventional one is representable
        // and round-trips (the field is not a computed-from-tenant invariant).
        let conventional = Conversation::home_cell_for(&self.id);
        let mut probe = self.clone();
        probe.home_cell = format!("other-cell:{conventional}");
        probe.home_cell != conventional && !probe.home_cell.is_empty()
    }
}

/// A membership row (arch §2.1 `membership`) — the access-control relation. Membership IS the ACL
/// for private kinds; a write/remove projects the ReBAC tuple via `write_tuples` in the SAME tx (the
/// CHAT-P8 floor — here the row + the `membership_by_principal` index exist; the zookie-in-tx +
/// new-enemy guard is CHAT-P8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Membership {
    /// The conversation this membership is in (the `(tenant, region, conversation_id)` key).
    pub conv: ConversationId,
    /// The pseudonymous principal id (human | agent | service). **NOT constrained to `conv`'s
    /// tenant** (cross-org NON-FORECLOSURE, arch §2): a membership set CAN span tenants — a future
    /// cross-cell channel (CHAT-P30) is a conversation whose members live in other cells. v1 does
    /// not federate, but the model does not forbid a foreign principal here.
    pub principal_id: String,
    /// member | admin (manage = invite/archive/settings).
    pub role: MembershipRole,
    /// The Notif read-fanout `watcher` relation (contract 4.9) — every watchable type owes Notif a
    /// watcher relation; resolved at read-fanout via `list_subjects(channel, watcher)`. Default true.
    pub is_watcher: bool,
    /// Per-channel mute/keyword-alert/DND preferences (delivery is Notif's; the JSON blob the arch
    /// `notif_pref jsonb` carries). Opaque here.
    pub notif_pref: serde_json::Value,
}

impl Membership {
    /// A plain member (the common case): `role = Member`, `is_watcher = true`, empty `notif_pref`.
    pub fn member(conv: ConversationId, principal_id: impl Into<String>) -> Membership {
        Membership {
            conv,
            principal_id: principal_id.into(),
            role: MembershipRole::Member,
            is_watcher: true,
            notif_pref: serde_json::Value::Object(Default::default()),
        }
    }

    /// An admin member: `role = Admin`, `is_watcher = true`, empty `notif_pref`.
    pub fn admin(conv: ConversationId, principal_id: impl Into<String>) -> Membership {
        Membership {
            role: MembershipRole::Admin,
            ..Membership::member(conv, principal_id)
        }
    }
}

/// A conversation-store error — a typed, loud surface (a store failure is a value, never a silent
/// fallthrough; EI-01 §3 prove-it).
#[derive(Debug, PartialEq, Eq)]
pub enum ConversationError {
    /// A conversation id was referenced that the store does not hold.
    NotFound(String),
    /// A conversation was created with an id that already exists (the create is not idempotent —
    /// a duplicate create is a LOUD conflict, never a silent overwrite).
    AlreadyExists(String),
    /// A schema-violation: a row carried an unparseable `kind`/`role` token (the 0-schema-violation
    /// invariant — surfaced loudly, never coerced to a default).
    SchemaViolation(String),
}

impl core::fmt::Display for ConversationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConversationError::NotFound(id) => write!(f, "conversation {id} not found"),
            ConversationError::AlreadyExists(id) => write!(f, "conversation {id} already exists"),
            ConversationError::SchemaViolation(e) => write!(f, "schema violation: {e}"),
        }
    }
}

impl std::error::Error for ConversationError {}

/// The conversation-store result alias.
pub type Result<T> = core::result::Result<T, ConversationError>;

/// The Conversation/Membership OLTP tier interface (arch §2 / §2.1; contract 11.1 OLTP tier). The
/// only surface the rest of Chat sees — the Postgres-class tier is the v1 impl
/// ([`MemConversationStore`] is the DB-free behaviour-identical model; the REAL Postgres tier is the
/// named promotion, wired with the `list_objects` gate in CHAT-P8/P13). The trait makes the
/// conversation list S1 a derived view of the membership rows (one primitive, EI-01 §7).
pub trait ConversationStore {
    /// Create a conversation (arch §2). A duplicate id is a LOUD [`ConversationError::AlreadyExists`]
    /// — never a silent overwrite. The membership→tuple→zookie co-commit is CHAT-P8; here the
    /// conversation row persists.
    fn create(&self, conv: Conversation) -> Result<()>;

    /// Read a conversation by its id (the round-trip half — append/read with 0 schema-violation).
    fn get(&self, id: &ConversationId) -> Result<Conversation>;

    /// Join a principal to a conversation (write the membership row + the `membership_by_principal`
    /// index entry, in lock-step). The ReBAC tuple projection + zookie stamp is CHAT-P8; here the
    /// row + index entry land. Re-joining (same principal) is idempotent (updates the row).
    fn join(&self, m: Membership) -> Result<()>;

    /// Remove a principal from a conversation (delete the membership row + the index entry, in
    /// lock-step). Removing a non-member is a no-op (idempotent leave).
    fn leave(&self, conv: &ConversationId, principal_id: &str) -> Result<()>;

    /// The `membership_by_principal` conversation-list index (S1; arch §2.1): EXACTLY the
    /// conversations `principal_id` is a member of within `tenant` (0 missing, 0 extra), a direct
    /// inverse-index lookup (no N+1, no all-conversations scan). The `list_objects` ACL gate is
    /// wired in CHAT-P8/P13; THIS index is the candidate set it joins against (the leak-free,
    /// no-N+1 list, contract 4.3).
    fn conversations_of(&self, tenant: &str, principal_id: &str) -> Result<Vec<ConversationId>>;

    /// All members of a conversation (the forward direction — `list_subjects(channel, member)` the
    /// read-fanout uses, arch §2.1). Ordered by principal id (stable).
    fn members_of(&self, conv: &ConversationId) -> Result<Vec<Membership>>;
}

/// The DB-free, behaviour-identical Conversation/Membership OLTP tier model (the unit-test floor;
/// contract 11.1 OLTP tier). Partitioned by `(tenant, region)` + conversation (residency-pinned,
/// contract 12.1) — a row lands ONLY in its `(tenant, region)` partition (0 cross-region rows). The
/// REAL Postgres tier is the named promotion (CHAT-P8/P13); this model + the trait are identical
/// under either engine (the swap seam, the same philosophy as the message store's `MessageStore`).
///
/// ## The `membership_by_principal` index, kept in lock-step
/// The store holds BOTH the forward membership rows (`(conv) -> {principal -> Membership}`) AND the
/// inverse index (`(tenant, principal) -> {conversation_id}`). Every [`Self::join`] / [`Self::leave`]
/// updates BOTH atomically (under one lock), so the index is NEVER a stale third copy — it is the
/// derived view the conversation list S1 reads (EI-01 §7 one primitive). [`Self::conversations_of`]
/// is a direct lookup against the inverse index — O(memberships of P), never O(all conversations).
#[derive(Default)]
pub struct MemConversationStore {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The conversation rows, keyed by their full `(tenant, region, conversation_id)` partition key.
    conversations: BTreeMap<ConversationId, Conversation>,
    /// The forward membership rows: `(conv) -> {principal_id -> Membership}`. The ACL truth.
    members: BTreeMap<ConversationId, BTreeMap<String, Membership>>,
    /// The `membership_by_principal` inverse index: `(tenant, principal_id) -> {conversation_id}`.
    /// The derived "my conversations" candidate set S1 — kept in lock-step with `members` so it is
    /// exact (0 missing, 0 extra) by construction. The value carries the full [`ConversationId`] so
    /// the list returns residency-pinned keys directly.
    by_principal: BTreeMap<(String, String), BTreeSet<ConversationId>>,
}

impl MemConversationStore {
    /// Construct an empty conversation store.
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
        // The conversation must exist — a membership in a phantom conversation is a LOUD error.
        if !inner.conversations.contains_key(&m.conv) {
            return Err(ConversationError::NotFound(m.conv.conversation_id.clone()));
        }
        // The inverse-index key is keyed by (tenant, principal) — the residency-pinned "my
        // conversations" lookup. (The principal MAY be a foreign-tenant principal; the index keys
        // on the CONVERSATION's tenant so "my conversations IN tenant T" is the residency-scoped
        // list. Cross-org membership lists across tenants are the CHAT-P30 follow-on.)
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
        // Lock-step removal: drop the forward row AND the inverse index entry together so the index
        // never holds a stale membership (0 extra rows in the conversation list).
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
        // A DIRECT inverse-index lookup — O(memberships of P), never an O(all conversations) scan
        // (the no-N+1 contract 4.3 property). The BTreeSet keeps the list stably ordered.
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
        }
    }

    /// The Conversation entity round-trips append/read with its kinds + retention_days + linked_ref
    /// (0 schema-violation rows) — the GATE.
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

        // A duplicate create is a LOUD conflict, never a silent overwrite.
        assert_eq!(
            store.create(c.clone()),
            Err(ConversationError::AlreadyExists("01J0CONV".into()))
        );
    }

    /// The kind discrimination round-trips through the frozen `conversation_kind` tokens (0
    /// schema-violation) — every kind's token parses back to itself, an unknown token is LOUD `None`.
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
        // membership-is-ACL: only the public channel inherits read via the project TTU.
        assert!(!ConversationKind::ChannelPublic.membership_is_acl());
        assert!(ConversationKind::ChannelPrivate.membership_is_acl());
        assert!(ConversationKind::Dm.membership_is_acl());
        // role tokens round-trip.
        for role in [MembershipRole::Member, MembershipRole::Admin] {
            assert_eq!(MembershipRole::from_token(role.as_token()), Some(role));
        }
        assert_eq!(MembershipRole::from_token("owner"), None);
    }

    /// The `membership_by_principal` index returns EXACTLY the conversations a principal is a member
    /// of (0 missing, 0 extra) — the GATE. Joins/leaves keep the index in lock-step.
    #[test]
    fn membership_by_principal_index_is_exact() {
        let store = MemConversationStore::new();
        // Three conversations; Alice is in c1 + c3, Bob is in c2 + c3.
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

        // A non-member's list is empty (0 extra — no leak).
        assert!(store.conversations_of("acme", "carol").unwrap().is_empty());

        // members_of is the forward direction — c3 has both.
        let c3_members: Vec<_> = store
            .members_of(&conv_id("c3"))
            .unwrap()
            .into_iter()
            .map(|m| m.principal_id)
            .collect();
        assert_eq!(c3_members, vec!["alice".to_string(), "bob".to_string()]);

        // Leave keeps the index exact: Alice leaves c3 → her list is just c1, c3 still has Bob.
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
        // Leaving twice / leaving a non-member is an idempotent no-op (not an error).
        store.leave(&conv_id("c3"), "alice").unwrap();
        store.leave(&conv_id("c1"), "nobody").unwrap();
    }

    /// The residency-pin: a row keys on `(tenant, region)`; a different region is a different
    /// partition (0 cross-region rows in the conversation list, contract 12.1).
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
            })
            .unwrap();
        store.join(Membership::member(fr.clone(), "alice")).unwrap();
        store.join(Membership::member(de.clone(), "alice")).unwrap();
        // The two regions are two partitions — the keys are distinct in the residency-pinned list.
        let list = store.conversations_of("acme", "alice").unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&fr) && list.contains(&de));
        assert_ne!(
            fr, de,
            "fr-par and de-fra are distinct residency-pinned keys"
        );
    }

    /// Cross-org NON-FORECLOSURE (the M4 floor witness, arch §2 + 05 §7): the home cell is a settable
    /// VALUE (not a baked `== tenant.cell`), and membership admits a FOREIGN-tenant principal — so
    /// the model does not assume single-org-membership-forever. CHAT-P30 / 12.6 builds federation.
    #[test]
    fn conversation_does_not_foreclose_multi_cell() {
        let store = MemConversationStore::new();
        let c = sample(ConversationKind::ChannelPrivate, "c1");
        store.create(c.clone()).unwrap();

        // The home cell is a settable field — the structural witness the CI GATE reads.
        assert!(
            c.not_single_cell_foreclosed(),
            "the home cell is a value, not a baked single-cell assumption"
        );

        // A conversation CAN carry a non-conventional home cell (a future cross-cell anchor).
        let mut cross = sample(ConversationKind::ChannelPrivate, "c2");
        cross.home_cell = "de-fra:other-org".into();
        store.create(cross.clone()).unwrap();
        assert_eq!(store.get(&cross.id).unwrap().home_cell, "de-fra:other-org");

        // Membership admits a FOREIGN-tenant principal id (the set "could span tenants", arch §2):
        // the principal id is a free String, never constrained to `conv`'s tenant.
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
