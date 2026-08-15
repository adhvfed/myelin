use std::collections::{BTreeMap, BTreeSet};

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventEnvelope, EventType, ReindexSource, SnapshotDraft,
    SnapshotScope, Visibility,
};

use crate::events;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatReplayKind {
    Channel,
    Message,
    Thread,
}

impl ChatReplayKind {
    fn snapshot_type(self) -> EventType {
        EventType(
            match self {
                ChatReplayKind::Channel => events::CHAT_CHANNEL_SNAPSHOT,
                ChatReplayKind::Message => events::CHAT_MESSAGE_SNAPSHOT,
                ChatReplayKind::Thread => events::CHAT_THREAD_SNAPSHOT,
            }
            .to_string(),
        )
    }

    fn from_selector(selector: &str) -> Option<ChatReplayKind> {
        match selector.split(':').next() {
            Some("channel") => Some(ChatReplayKind::Channel),
            Some("message") => Some(ChatReplayKind::Message),
            Some("thread") => Some(ChatReplayKind::Thread),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChatTruthRow {
    kind: ChatReplayKind,
    version: u64,
    payload: serde_json::Value,
    subject: ArtifactRef,
}

#[derive(Debug, Default)]
pub struct ChatReindexSource {
    truth: BTreeMap<String, ChatTruthRow>,
}

impl ChatReindexSource {
    pub fn new() -> ChatReindexSource {
        ChatReindexSource::default()
    }

    pub fn upsert(
        &mut self,
        kind: ChatReplayKind,
        aggregate: &str,
        version: u64,
        subject: &str,
        mut payload: serde_json::Value,
    ) {
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert("version".into(), serde_json::json!(version));
        }
        self.truth.insert(
            aggregate.to_string(),
            ChatTruthRow {
                kind,
                version,
                payload,
                subject: ArtifactRef(subject.to_string()),
            },
        );
    }

    pub fn erase(&mut self, aggregate: &str) -> bool {
        self.truth.remove(aggregate).is_some()
    }
}

fn matches_aggregate(agg: &str, target: &str) -> bool {
    if agg == target {
        return true;
    }
    agg.strip_suffix(target)
        .and_then(|head| head.chars().next_back())
        .is_some_and(|boundary| boundary == '/' || boundary == '#')
}

impl ReindexSource for ChatReindexSource {
    fn owner_token(&self) -> &str {
        "chat"
    }

    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        let kind = match ChatReplayKind::from_selector(&scope.selector) {
            Some(k) => k,
            None => return Vec::new(),
        };
        let target = scope
            .selector
            .split_once(':')
            .map(|(_, rest)| rest)
            .unwrap_or("");
        self.truth
            .iter()
            .filter(|(_, row)| row.kind == kind)
            .filter(|(agg, _)| target == "all" || matches_aggregate(agg, target))
            .filter(|(_, row)| since.is_none_or(|s| row.version > s))
            .map(|(agg, row)| SnapshotDraft {
                aggregate: AggregateKey(agg.clone()),
                version: row.version,
                type_: kind.snapshot_type(),
                subject: row.subject.clone(),
                payload: row.payload.clone(),
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageProjection {
    pub body_text: String,
    pub channel_id: String,
    pub edges: Vec<(String, String)>,
    pub mentions: Vec<String>,
}

pub trait MessageProjectFetcher {
    fn project(&self, message_ref: &str) -> Option<MessageProjection>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatReadModelConsumer {
    search: BTreeMap<String, (u64, (String, String))>,
    refs: BTreeMap<String, (u64, String)>,
    notif: BTreeMap<String, (u64, String)>,
    applied: BTreeSet<String>,
}

impl ChatReadModelConsumer {
    pub fn new() -> ChatReadModelConsumer {
        ChatReadModelConsumer::default()
    }

    pub fn ingest(&mut self, env: &EventEnvelope, fetcher: &dyn MessageProjectFetcher) -> bool {
        if !self.applied.insert(env.event_id.0.clone()) {
            return false;
        }
        if env.type_.0 != events::CHAT_MESSAGE_SNAPSHOT
            && env.type_.0 != events::CHAT_MESSAGE_CREATED
            && env.type_.0 != events::CHAT_MESSAGE_EDITED
        {
            return false;
        }
        let version = env
            .payload
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let message_ref = env.subject.0.clone();
        let proj = match fetcher.project(&message_ref) {
            Some(p) => p,
            None => return false,
        };

        let mut changed = false;
        changed |= lww_insert(
            &mut self.search,
            message_ref.clone(),
            version,
            (version, (proj.body_text.clone(), proj.channel_id.clone())),
        );
        for (target, rel) in &proj.edges {
            let key = myelin_refs::edge_aggregate_key(
                &myelin_refs::ArtifactRef(message_ref.clone()),
                &myelin_refs::ArtifactRef(target.clone()),
            )
            .0;
            changed |= lww_insert(&mut self.refs, key, version, (version, rel.clone()));
        }
        for member in &proj.mentions {
            let key = format!("{member}|{message_ref}");
            changed |= lww_insert(
                &mut self.notif,
                key,
                version,
                (version, NOTIF_REASON_MENTIONED.to_string()),
            );
        }
        changed
    }

    pub fn search_len(&self) -> usize {
        self.search.len()
    }

    pub fn refs_len(&self) -> usize {
        self.refs.len()
    }

    pub fn notif_len(&self) -> usize {
        self.notif.len()
    }

    pub fn search_indexes(&self, message_ref: &str) -> bool {
        self.search.contains_key(message_ref)
    }

    pub fn search_visible_to(&self, readable_channels: &BTreeSet<String>) -> Vec<String> {
        self.search
            .iter()
            .filter(|(_, (_, (_, channel)))| readable_channels.contains(channel))
            .map(|(message_ref, _)| message_ref.clone())
            .collect()
    }
}

fn lww_insert<V>(
    map: &mut BTreeMap<String, (u64, V)>,
    key: String,
    version: u64,
    value: (u64, V),
) -> bool
where
    V: PartialEq,
{
    match map.get(&key) {
        Some((existing_v, _)) if *existing_v >= version => false,
        Some((_, existing_val)) if *existing_val == value.1 => {
            map.insert(key, value);
            false
        }
        _ => {
            map.insert(key, value);
            true
        }
    }
}

pub const NOTIF_REASON_MENTIONED: &str = crate::glue::RULE_KEY_MENTIONED;

pub fn reindex_parity_hash(rm: &ChatReadModelConsumer) -> String {
    let view = serde_json::json!({
        "search": rm.search,
        "refs": rm.refs,
        "notif": rm.notif,
    });
    let bytes = serde_json::to_vec(&view).expect("read-model serializes");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in &bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("rmparity-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::snapshot_event_id;

    fn source() -> ChatReindexSource {
        let mut s = ChatReindexSource::new();
        s.upsert(
            ChatReplayKind::Message,
            "myelin://acme/chat/message/m1",
            1,
            "myelin://acme/chat/message/m1",
            serde_json::json!({ "channel": "c1" }),
        );
        s.upsert(
            ChatReplayKind::Message,
            "myelin://acme/chat/message/m2",
            4,
            "myelin://acme/chat/message/m2",
            serde_json::json!({ "channel": "c1" }),
        );
        s
    }

    #[test]
    fn message_granular_replay() {
        let drafts = source().replay(&SnapshotScope::new("chat", "message:m1"), None);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/chat/message/m1");
        assert_eq!(drafts[0].type_.0, "chat.message.snapshot");
    }

    #[test]
    fn cold_equals_live_idempotent() {
        let src = source();
        let scope = SnapshotScope::new("chat", "message:all");
        let a = src.replay(&scope, None);
        let b = src.replay(&scope, None);
        assert_eq!(a, b);
        assert_eq!(
            snapshot_event_id(&tenant(), &a[0].aggregate, a[0].version),
            snapshot_event_id(&tenant(), &b[0].aggregate, b[0].version)
        );
    }

    #[test]
    fn erased_message_is_skipped() {
        let mut src = source();
        assert!(src.erase("myelin://acme/chat/message/m1"));
        let drafts = src.replay(&SnapshotScope::new("chat", "message:all"), None);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/chat/message/m2");
    }

    #[test]
    fn firehose_only_frames_are_never_reindexed() {
        let drafts = source().replay(&SnapshotScope::new("chat", "presence:all"), None);
        assert!(
            drafts.is_empty(),
            "presence is firehose-only - never a durable snapshot"
        );
    }

    #[test]
    fn owner_token_is_chat() {
        assert_eq!(source().owner_token(), "chat");
    }

    use myelin_events::{
        reindex, Actor, EmitContextBase, OutboxStore, Region as EvRegion, TenantId as EvTenantId,
        Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> EvTenantId {
        EvTenantId("acme".into())
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: EvRegion("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            caused_by: None,
        }
    }

    #[test]
    fn replay_skeleton_emits_chat_snapshot_through_the_outbox() {
        let src = source();
        let sources: &[&dyn ReindexSource] = &[&src];
        let scope = SnapshotScope::new("chat", "message:m1");
        let mut outbox = OutboxStore::new();

        let drafts = src.replay(&scope, None);
        assert_eq!(
            drafts.len(),
            1,
            "the message:m1 scope replays exactly one message snapshot"
        );
        assert_eq!(drafts[0].type_.0, "chat.message.snapshot");

        let receipt = reindex(&scope, None, sources, &mut outbox, ctx_base())
            .expect("reindex through outbox");
        assert_eq!(
            receipt.snapshots_emitted, 1,
            "one chat.message.snapshot emitted through the outbox"
        );
        let row = outbox
            .row(&drafts[0].event_id(&tenant()))
            .expect("the chat.message.snapshot row is on the outbox (never off it)");
        assert_eq!(row.envelope.type_.0, "chat.message.snapshot");

        let again = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-reindex");
        assert_eq!(
            again.snapshots_emitted, 0,
            "a re-run emits 0 new (idempotent skeleton)"
        );
        assert_eq!(
            again.snapshots_skipped_duplicate, 1,
            "the snapshot is skipped as a duplicate"
        );
    }

    #[test]
    fn reindex_for_a_non_chat_owner_is_loud_not_a_silent_empty_emit() {
        let src = source();
        let sources: &[&dyn ReindexSource] = &[&src];
        let scope = SnapshotScope::new("git", "commit:all");
        let mut outbox = OutboxStore::new();
        let err = reindex(&scope, None, sources, &mut outbox, ctx_base())
            .expect_err("chat does not own the git scope - a loud error, not a silent empty emit");
        assert!(matches!(
            err,
            myelin_events::ReindexError::NoSourceForOwner(_)
        ));
    }

    use std::collections::BTreeSet;

    #[derive(Default)]
    struct FakeMessageProjector {
        bodies: BTreeMap<String, MessageProjection>,
    }
    impl FakeMessageProjector {
        fn with(
            message_ref: &str,
            channel: &str,
            body: &str,
            edges: &[(&str, &str)],
            mentions: &[&str],
        ) -> MessageProjection {
            MessageProjection {
                body_text: body.to_string(),
                channel_id: channel.to_string(),
                edges: edges
                    .iter()
                    .map(|(t, r)| (t.to_string(), r.to_string()))
                    .collect(),
                mentions: mentions.iter().map(|m| m.to_string()).collect(),
            }
            .tagged(message_ref)
        }
        fn put(&mut self, message_ref: &str, proj: MessageProjection) {
            self.bodies.insert(message_ref.to_string(), proj);
        }
        fn erase(&mut self, message_ref: &str) {
            self.bodies.remove(message_ref);
        }
    }
    impl MessageProjectFetcher for FakeMessageProjector {
        fn project(&self, message_ref: &str) -> Option<MessageProjection> {
            self.bodies.get(message_ref).cloned()
        }
    }
    impl MessageProjection {
        fn tagged(self, _message_ref: &str) -> MessageProjection {
            self
        }
    }

    fn message_snapshot_envelope(message_ref: &str, version: u64) -> EventEnvelope {
        use myelin_events::{Actor, AggregateKey, CorrelationId, Region as EvRegion, Timestamp};
        let agg = AggregateKey(message_ref.to_string());
        EventEnvelope {
            event_id: myelin_events::snapshot_event_id(&tenant(), &agg, version),
            type_: EventType(events::CHAT_MESSAGE_SNAPSHOT.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: EvRegion("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                tenant(),
            )),
            subject: ArtifactRef(message_ref.to_string()),
            aggregate: agg,
            causation_id: None,
            correlation_id: CorrelationId(format!("corr-{message_ref}")),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
            payload: serde_json::json!({ "version": version, "channel": "c1" }),
        }
    }

    fn corpus_projector() -> FakeMessageProjector {
        let mut f = FakeMessageProjector::default();
        f.put(
            "myelin://acme/chat/message/m1",
            FakeMessageProjector::with(
                "myelin://acme/chat/message/m1",
                "c1",
                "blocked on the deploy",
                &[("myelin://acme/issue/issue/ENG-1", "links")],
                &["myelin://acme/identity/member/alice"],
            ),
        );
        f.put(
            "myelin://acme/chat/message/m2",
            FakeMessageProjector::with(
                "myelin://acme/chat/message/m2",
                "c2",
                "the confidential fix",
                &[],
                &["myelin://acme/identity/member/bob"],
            ),
        );
        f
    }

    fn parity_source() -> ChatReindexSource {
        let mut s = ChatReindexSource::new();
        s.upsert(
            ChatReplayKind::Message,
            "myelin://acme/chat/message/m1",
            1,
            "myelin://acme/chat/message/m1",
            serde_json::json!({ "channel": "c1" }),
        );
        s.upsert(
            ChatReplayKind::Message,
            "myelin://acme/chat/message/m2",
            1,
            "myelin://acme/chat/message/m2",
            serde_json::json!({ "channel": "c2" }),
        );
        s
    }

    #[test]
    fn steady_state_and_recovery_share_one_path_parity_hash_matches() {
        let proj = corpus_projector();
        let src = parity_source();
        let scope = SnapshotScope::new("chat", "message:all");

        let mut live = ChatReadModelConsumer::new();
        for draft in src.replay(&scope, None) {
            let env = message_snapshot_envelope(&draft.aggregate.0, draft.version);
            live.ingest(&env, &proj);
        }

        let mut cold = ChatReadModelConsumer::new();
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = myelin_events::OutboxStore::new();
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
        for draft in src.replay(&scope, None) {
            let row = outbox
                .row(&draft.event_id(&tenant()))
                .expect("snapshot row present");
            cold.ingest(&row.envelope, &proj);
        }

        assert_eq!(live.search_len(), 2, "two messages indexed live");
        assert_eq!(cold.search_len(), 2, "two messages rebuilt cold");
        assert_eq!(live.refs_len(), 1, "one links edge (m1 → ENG-1)");
        assert_eq!(cold.refs_len(), 1);
        assert_eq!(
            live.notif_len(),
            2,
            "two @-mention notify rows (alice, bob)"
        );
        assert_eq!(cold.notif_len(), 2);

        assert_eq!(
            reindex_parity_hash(&cold),
            reindex_parity_hash(&live),
            "the cold-rebuilt read-models byte-match live across Search/Refs/Notif (CHAT-D15)"
        );
    }

    #[test]
    fn rebuild_is_idempotent_a_redelivered_snapshot_is_a_no_op() {
        let proj = corpus_projector();
        let src = parity_source();
        let scope = SnapshotScope::new("chat", "message:all");

        let mut rm = ChatReadModelConsumer::new();
        for draft in src.replay(&scope, None) {
            let env = message_snapshot_envelope(&draft.aggregate.0, draft.version);
            rm.ingest(&env, &proj);
        }
        let after_first = reindex_parity_hash(&rm);

        for draft in src.replay(&scope, None) {
            let env = message_snapshot_envelope(&draft.aggregate.0, draft.version);
            assert!(!rm.ingest(&env, &proj), "a redelivered snapshot is a no-op");
        }
        assert_eq!(
            after_first,
            reindex_parity_hash(&rm),
            "a re-run is idempotent (cold == live, no double effect)"
        );
    }

    #[test]
    fn an_erased_subject_emits_a_tombstone_on_rebuild_no_resurrection() {
        let mut proj = corpus_projector();
        let mut src = parity_source();
        let scope = SnapshotScope::new("chat", "message:all");

        assert!(src.erase("myelin://acme/chat/message/m2"), "m2 was present");
        proj.erase("myelin://acme/chat/message/m2");

        let mut cold = ChatReadModelConsumer::new();
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = myelin_events::OutboxStore::new();
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("post-erase reindex");
        for draft in src.replay(&scope, None) {
            let row = outbox
                .row(&draft.event_id(&tenant()))
                .expect("snapshot row");
            cold.ingest(&row.envelope, &proj);
        }

        assert_eq!(
            cold.search_len(),
            1,
            "only m1 rebuilt - m2 did not resurrect"
        );
        assert!(
            cold.search_indexes("myelin://acme/chat/message/m1"),
            "m1 is present (surgical erasure)"
        );
        assert!(
            !cold.search_indexes("myelin://acme/chat/message/m2"),
            "the erased m2 is ABSENT after reindex (0 resurrected PII; the tombstone residual)"
        );
        assert_eq!(
            cold.notif_len(),
            1,
            "only m1's @-mention notify row survives (bob's row to m2 did not resurrect)"
        );
    }

    #[test]
    fn a_rebuild_stays_acl_correct_the_filter_conjoin_excludes_unreadable_channels() {
        let proj = corpus_projector();
        let src = parity_source();
        let scope = SnapshotScope::new("chat", "message:all");

        let mut cold = ChatReadModelConsumer::new();
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = myelin_events::OutboxStore::new();
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
        for draft in src.replay(&scope, None) {
            let row = outbox
                .row(&draft.event_id(&tenant()))
                .expect("snapshot row");
            cold.ingest(&row.envelope, &proj);
        }

        let only_c1: BTreeSet<String> = ["c1".to_string()].into_iter().collect();
        assert_eq!(
            cold.search_visible_to(&only_c1),
            vec!["myelin://acme/chat/message/m1".to_string()],
            "the rebuild conjoins the channel Filter - only c1's message is visible"
        );
        assert!(
            cold.search_visible_to(&BTreeSet::new()).is_empty(),
            "a non-member sees 0 rebuilt rows (0 unfiltered rows; CHAT-D11 holds on the rebuild)"
        );
    }

    #[test]
    fn notif_reason_is_the_frozen_mentioned_rule_key() {
        assert_eq!(NOTIF_REASON_MENTIONED, crate::glue::RULE_KEY_MENTIONED);
    }
}
