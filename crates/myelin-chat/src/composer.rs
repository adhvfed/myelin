use crate::store::{MessageId, MessageStore, OutboxTx, StoreError};
use myelin_content::InlineNode;
use myelin_events::ArtifactRef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    pub token: &'static str,
    pub label: &'static str,
}

#[derive(Clone, Debug)]
pub struct SlashMenu {
    commands: Vec<SlashCommand>,
}

impl SlashMenu {
    pub fn default_commands() -> SlashMenu {
        SlashMenu {
            commands: vec![
                SlashCommand {
                    token: "remind",
                    label: "Remind me when…",
                },
                SlashCommand {
                    token: "poll",
                    label: "Start a poll",
                },
                SlashCommand {
                    token: "agent",
                    label: "Ask an agent (explicit)",
                },
                SlashCommand {
                    token: "code",
                    label: "Insert a code block",
                },
                SlashCommand {
                    token: "shrug",
                    label: "Shrug ¯\\_(ツ)_/¯",
                },
            ],
        }
    }

    pub fn new(commands: Vec<SlashCommand>) -> SlashMenu {
        SlashMenu { commands }
    }

    pub fn filter(&self, prefix: &str) -> Vec<SlashCommand> {
        let prefix = prefix.to_ascii_lowercase();
        self.commands
            .iter()
            .filter(|c| c.token.to_ascii_lowercase().starts_with(&prefix))
            .cloned()
            .collect()
    }

    pub fn is_known(&self, token: &str) -> bool {
        self.commands.iter().any(|c| c.token == token)
    }
}

impl Default for SlashMenu {
    fn default() -> Self {
        SlashMenu::default_commands()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutocompleteKind {
    Mention,
    Artifact,
}

impl AutocompleteKind {
    pub fn trigger(self) -> char {
        match self {
            AutocompleteKind::Mention => '@',
            AutocompleteKind::Artifact => '#',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub target: ArtifactRef,
    pub label: String,
    pub kind: AutocompleteKind,
}

impl Suggestion {
    pub fn artifact_node(&self) -> Option<InlineNode> {
        match self.kind {
            AutocompleteKind::Artifact => Some(InlineNode::ArtifactRefNode(self.target.clone())),
            AutocompleteKind::Mention => None,
        }
    }
}

pub trait AutocompletePort {
    fn suggest(&self, kind: AutocompleteKind, prefix: &str, limit: u32) -> Vec<Suggestion>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnfurlIntent {
    Artifact(ArtifactRef),
    External(ArtifactRef),
}

impl UnfurlIntent {
    pub fn node(&self) -> InlineNode {
        match self {
            UnfurlIntent::Artifact(r) => InlineNode::ArtifactRefNode(r.clone()),
            UnfurlIntent::External(r) => InlineNode::Embed(r.clone()),
        }
    }
}

pub fn detect_pasted_url(pasted: &str) -> Option<UnfurlIntent> {
    let trimmed = pasted.trim();
    if trimmed.is_empty() || trimmed.split_whitespace().count() != 1 {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("myelin://") {
        if rest.is_empty() {
            return None;
        }
        return Some(UnfurlIntent::Artifact(ArtifactRef(trimmed.to_string())));
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let after_scheme = trimmed
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        if after_scheme.is_empty() {
            return None;
        }
        return Some(UnfurlIntent::External(ArtifactRef(trimmed.to_string())));
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Draft {
    pub body_inline: String,
    pub body_nodes: Vec<InlineNode>,
}

impl Draft {
    pub fn text(body_inline: impl Into<String>) -> Draft {
        Draft {
            body_inline: body_inline.into(),
            body_nodes: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.body_inline.is_empty() && self.body_nodes.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DraftKey {
    pub conversation_id: String,
    pub author_pseudonym: String,
}

impl DraftKey {
    pub fn new(
        conversation_id: impl Into<String>,
        author_pseudonym: impl Into<String>,
    ) -> DraftKey {
        DraftKey {
            conversation_id: conversation_id.into(),
            author_pseudonym: author_pseudonym.into(),
        }
    }
}

pub trait DraftStore {
    fn save(&self, key: &DraftKey, draft: &Draft);

    fn load(&self, key: &DraftKey) -> Option<Draft>;

    fn clear(&self, key: &DraftKey);

    fn purge_author(&self, author_pseudonym: &str) -> usize;
}

#[derive(Default)]
pub struct MemDraftStore {
    drafts: std::sync::Mutex<std::collections::HashMap<DraftKey, Draft>>,
}

impl MemDraftStore {
    pub fn new() -> MemDraftStore {
        MemDraftStore::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<DraftKey, Draft>> {
        self.drafts.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl DraftStore for MemDraftStore {
    fn save(&self, key: &DraftKey, draft: &Draft) {
        let mut drafts = self.lock();
        if draft.is_empty() {
            drafts.remove(key);
        } else {
            drafts.insert(key.clone(), draft.clone());
        }
    }

    fn load(&self, key: &DraftKey) -> Option<Draft> {
        self.lock().get(key).cloned()
    }

    fn clear(&self, key: &DraftKey) {
        self.lock().remove(key);
    }

    fn purge_author(&self, author_pseudonym: &str) -> usize {
        let mut drafts = self.lock();
        let before = drafts.len();
        drafts.retain(|k, _| k.author_pseudonym != author_pseudonym);
        before - drafts.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditRequest {
    pub message_id: MessageId,
    pub body_inline: Vec<u8>,
    pub body_nodes: Vec<u8>,
    pub expect_seq: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOutcome {
    Applied {
        new_seq: i32,
    },
    Rejected {
        expected: i32,
        current_seq: i32,
    },
    NotFound,
    StoreFault(String),
}

pub struct EditCas<'a, S: MessageStore> {
    store: &'a S,
}

impl<'a, S: MessageStore> EditCas<'a, S> {
    pub fn new(store: &'a S) -> EditCas<'a, S> {
        EditCas { store }
    }

    pub fn apply(&self, tx: &mut OutboxTx, req: &EditRequest) -> EditOutcome {
        match self.store.revise(
            tx,
            &req.message_id,
            req.body_inline.clone(),
            req.body_nodes.clone(),
            req.expect_seq,
        ) {
            Ok(()) => EditOutcome::Applied {
                new_seq: req.expect_seq + 1,
            },
            Err(StoreError::CasConflict { actual, .. }) => EditOutcome::Rejected {
                expected: req.expect_seq,
                current_seq: actual,
            },
            Err(StoreError::NotFound(_)) => EditOutcome::NotFound,
            Err(e) => EditOutcome::StoreFault(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{AuthorKind, ConversationId, MemHotTier, NewMessage};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
        Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::Arc;

    fn alice() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(alice()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn conv() -> ConversationId {
        ConversationId::new("acme", "fr-par", "01J0CONV")
    }

    #[test]
    fn slash_menu_filters_by_prefix_and_guards_unknown_commands() {
        let menu = SlashMenu::default_commands();
        assert_eq!(menu.filter("").len(), menu.filter("").len().max(5));
        let r = menu.filter("re");
        assert!(r.iter().any(|c| c.token == "remind"));
        assert!(!r.iter().any(|c| c.token == "poll"));
        assert_eq!(menu.filter("RE"), r, "prefix filter is case-insensitive");
        assert!(menu.is_known("remind"));
        assert!(!menu.is_known("rm -rf"), "a client cannot mint a command");
    }

    struct FakeSearchPort;
    impl AutocompletePort for FakeSearchPort {
        fn suggest(&self, kind: AutocompleteKind, prefix: &str, limit: u32) -> Vec<Suggestion> {
            let target = match kind {
                AutocompleteKind::Mention => {
                    ArtifactRef(format!("myelin://acme/identity/member/{prefix}"))
                }
                AutocompleteKind::Artifact => {
                    ArtifactRef(format!("myelin://acme/issue/issue/{prefix}"))
                }
            };
            vec![Suggestion {
                target,
                label: format!("{prefix} (authorised)"),
                kind,
            }]
            .into_iter()
            .take(limit as usize)
            .collect()
        }
    }

    #[test]
    fn autocomplete_goes_through_the_port_and_artifact_inserts_a_structured_node() {
        let port = FakeSearchPort;
        let mentions = port.suggest(AutocompleteKind::Mention, "ali", 5);
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].kind, AutocompleteKind::Mention);
        assert!(mentions[0].target.0.contains("/identity/member/"));
        assert!(mentions[0].artifact_node().is_none());

        let arts = port.suggest(AutocompleteKind::Artifact, "ENG-1", 5);
        assert_eq!(arts[0].kind, AutocompleteKind::Artifact);
        let node = arts[0]
            .artifact_node()
            .expect("artifact inserts a structured node");
        assert!(matches!(node, InlineNode::ArtifactRefNode(_)));

        assert_eq!(AutocompleteKind::Mention.trigger(), '@');
        assert_eq!(AutocompleteKind::Artifact.trigger(), '#');
    }

    #[test]
    fn paste_in_platform_url_is_a_structured_artifact_ref() {
        let intent =
            detect_pasted_url("myelin://acme/git/pr/88").expect("an in-platform URL unfurls");
        assert!(matches!(intent, UnfurlIntent::Artifact(_)));
        assert!(matches!(intent.node(), InlineNode::ArtifactRefNode(_)));
    }

    #[test]
    fn paste_external_url_is_an_embed() {
        let intent =
            detect_pasted_url("https://example.com/page").expect("an external URL unfurls");
        assert!(matches!(intent, UnfurlIntent::External(_)));
        assert!(matches!(intent.node(), InlineNode::Embed(_)));
    }

    #[test]
    fn non_url_paste_is_ordinary_text() {
        assert!(detect_pasted_url("see myelin://acme/git/pr/88 please").is_none());
        assert!(detect_pasted_url("https://").is_none());
        assert!(detect_pasted_url("myelin://").is_none());
        assert!(detect_pasted_url("just some text").is_none());
        assert!(detect_pasted_url("").is_none());
    }

    #[test]
    fn draft_save_load_round_trips_and_empty_clears() {
        let store = MemDraftStore::new();
        let key = DraftKey::new("01J0CONV", "p-opaque-alice");
        assert!(store.load(&key).is_none(), "no draft → empty composer");

        let draft = Draft {
            body_inline: "an unsent **message**".into(),
            body_nodes: vec![InlineNode::ArtifactRefNode(ArtifactRef(
                "myelin://acme/issue/issue/ENG-1".into(),
            ))],
        };
        store.save(&key, &draft);
        assert_eq!(
            store.load(&key).as_ref(),
            Some(&draft),
            "the draft round-trips (restored on re-open)"
        );

        store.save(&key, &Draft::default());
        assert!(store.load(&key).is_none(), "an empty save clears the draft");

        let bob = DraftKey::new("01J0CONV", "p-opaque-bob");
        store.save(&bob, &Draft::text("bob's draft"));
        store.save(&key, &Draft::text("alice's draft"));
        assert_eq!(store.load(&bob).unwrap().body_inline, "bob's draft");
        assert_eq!(store.load(&key).unwrap().body_inline, "alice's draft");
    }

    fn append_message(
        store: &MemHotTier,
        outbox: &OutboxStore,
        minter: &Arc<MonotonicMinter>,
        body: &[u8],
    ) -> MessageId {
        let minter: Arc<dyn IdMinter> = minter.clone();
        let mut tx = outbox.begin(minter, ctx_base());
        let id = store
            .append(
                &mut tx,
                NewMessage {
                    conv: conv(),
                    thread_root_id: None,
                    author: "p-opaque-alice".into(),
                    author_kind: AuthorKind::Human,
                    body_inline: body.to_vec(),
                    body_nodes: Vec::new(),
                    client_nonce: "nonce-1".into(),
                },
            )
            .unwrap();
        tx.commit().unwrap();
        id
    }

    #[test]
    fn edit_cas_applies_a_fresh_edit_and_bumps_the_seq() {
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let id = append_message(&store, &outbox, &minter, b"original");

        let cas = EditCas::new(&store);
        let mut tx = outbox.begin(minter.clone() as Arc<dyn IdMinter>, ctx_base());
        let outcome = cas.apply(
            &mut tx,
            &EditRequest {
                message_id: id.clone(),
                body_inline: b"edited once".to_vec(),
                body_nodes: Vec::new(),
                expect_seq: 0,
            },
        );
        tx.commit().unwrap();
        assert_eq!(
            outcome,
            EditOutcome::Applied { new_seq: 1 },
            "a fresh edit applies and bumps edited_seq 0 → 1"
        );
    }

    #[test]
    fn edit_cas_rejects_a_stale_edit_with_the_current_state_zero_silent_overwrite() {
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let id = append_message(&store, &outbox, &minter, b"original");

        let cas = EditCas::new(&store);

        let mut tx = outbox.begin(minter.clone() as Arc<dyn IdMinter>, ctx_base());
        let first = cas.apply(
            &mut tx,
            &EditRequest {
                message_id: id.clone(),
                body_inline: b"winner edit".to_vec(),
                body_nodes: Vec::new(),
                expect_seq: 0,
            },
        );
        tx.commit().unwrap();
        assert_eq!(first, EditOutcome::Applied { new_seq: 1 });

        let mut tx2 = outbox.begin(minter.clone() as Arc<dyn IdMinter>, ctx_base());
        let stale = cas.apply(
            &mut tx2,
            &EditRequest {
                message_id: id.clone(),
                body_inline: b"loser clobber".to_vec(),
                body_nodes: Vec::new(),
                expect_seq: 0,
            },
        );
        assert_eq!(
            stale,
            EditOutcome::Rejected {
                expected: 0,
                current_seq: 1
            },
            "a stale edit is rejected with the current state - 0 silent overwrite"
        );

        let rows = store
            .range(&conv(), crate::store::RangeCursor::Recent, 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].body_inline, b"winner edit",
            "the clobber did not overwrite"
        );
        assert_eq!(
            rows[0].edited_seq, 1,
            "the seq reflects exactly one applied edit"
        );
    }

    #[test]
    fn edit_cas_not_found_for_a_missing_message() {
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let cas = EditCas::new(&store);
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let mut tx = outbox.begin(minter, ctx_base());
        let outcome = cas.apply(
            &mut tx,
            &EditRequest {
                message_id: MessageId("01J-does-not-exist".into()),
                body_inline: b"x".to_vec(),
                body_nodes: Vec::new(),
                expect_seq: 0,
            },
        );
        assert_eq!(outcome, EditOutcome::NotFound);
    }
}
