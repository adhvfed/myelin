//! # `replay` — Chat's per-owner reindex-from-source `replay` body (EB-27 / P-327, M4)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §4.9 (reindex-from-source re-emit).
//! **Contract:** index row **2.6** (`events::reindex(scope)` → owner `replay(scope, since)` emits
//! `*.snapshot`; **sub-artifact-granular**). **Floor filled:** the Bus's `myelin_events::reindex`
//! named the per-OWNER `replay` bodies as a floor; EB-26 (P-246, M3) filled Git/KN; this is Chat's
//! M4 body (EB-27 / P-327).
//!
//! Chat replays only its **DURABLE** aggregates — channels / messages / threads (the durable
//! `chat.*` set). The FIREHOSE-only frames (presence/typing/read_state) are EPHEMERAL: they are
//! NEVER durably stored and so are NEVER reindexed — a firehose gap recovers via
//! `firehose::resume(stream, scope, last_seq)` (CHAT-D1), not via `replay`. The two recovery paths
//! are distinct: durable snapshots for the persisted set; the resume-cursor for the ephemeral set.
//!
//! - **`channel:<id>`** — a single channel (`chat.channel.snapshot`);
//! - **`message:<id>`** — a single message (`chat.message.snapshot`);
//! - **`thread:<id>`** — a single thread (`chat.thread.snapshot`).
//!
//! The deterministic snapshot `event_id` from `(aggregate, version)` makes a re-run idempotent
//! (cold == live, BUS-D5). An erased aggregate is SKIPPED (X-7).

use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};

use crate::events;

/// The sub-artifact kind a Chat reindex scope selects (contract 2.6 — DURABLE aggregates only; the
/// firehose-only frames are never reindexed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatReplayKind {
    /// A single channel — re-emits `chat.channel.snapshot`.
    Channel,
    /// A single message — re-emits `chat.message.snapshot`.
    Message,
    /// A single thread — re-emits `chat.thread.snapshot`.
    Thread,
}

impl ChatReplayKind {
    /// The `*.snapshot` event type token this kind re-emits (the NAMED chat token, never a literal).
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

    /// Parse the leading kind token off a `scope.selector`.
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

/// **Chat's [`ReindexSource`] body (EB-27 / P-327, M4 — the named floor filled).** Holds Chat's OWN
/// source of truth for the DURABLE aggregates and replays a sub-artifact-granular scope → the
/// `*.snapshot` drafts. A real wiring reads Chat's `MessageStore`; this reads its in-memory truth.
#[derive(Debug, Default)]
pub struct ChatReindexSource {
    truth: BTreeMap<String, ChatTruthRow>,
}

impl ChatReindexSource {
    /// A fresh, empty source.
    pub fn new() -> ChatReindexSource {
        ChatReindexSource::default()
    }

    /// Record/update Chat's truth for a DURABLE aggregate (the live write a message-persist made).
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

    /// Mark an aggregate erased (X-7) — REMOVED from the truth so a subsequent replay SKIPS it.
    pub fn erase(&mut self, aggregate: &str) -> bool {
        self.truth.remove(aggregate).is_some()
    }
}

/// Segment-anchored aggregate match (the over-match guard).
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
                // A chat message body is tenant content (PII body behind a per-subject DEK); the
                // snapshot is references-not-payloads (the body never rides a ref-only snapshot).
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
            })
            .collect()
    }
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

    /// **Sub-artifact-granular replay (contract 2.6).** A `message:m1` scope replays exactly that
    /// message's snapshot.
    #[test]
    fn message_granular_replay() {
        let drafts = source().replay(&SnapshotScope::new("chat", "message:m1"), None);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/chat/message/m1");
        assert_eq!(drafts[0].type_.0, "chat.message.snapshot");
    }

    /// **cold == live + idempotent re-run (BUS-D5).**
    #[test]
    fn cold_equals_live_idempotent() {
        let src = source();
        let scope = SnapshotScope::new("chat", "message:all");
        let a = src.replay(&scope, None);
        let b = src.replay(&scope, None);
        assert_eq!(a, b);
        assert_eq!(
            snapshot_event_id(&a[0].aggregate, a[0].version),
            snapshot_event_id(&b[0].aggregate, b[0].version)
        );
    }

    /// **An erased message is SKIPPED (X-7).**
    #[test]
    fn erased_message_is_skipped() {
        let mut src = source();
        assert!(src.erase("myelin://acme/chat/message/m1"));
        let drafts = src.replay(&SnapshotScope::new("chat", "message:all"), None);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/chat/message/m2");
    }

    /// **The FIREHOSE-only frames are NEVER reindexed.** A `presence:*` scope (a firehose-only kind)
    /// is not a durable replay kind → it yields nothing (the firehose recovers via resume, not
    /// replay).
    #[test]
    fn firehose_only_frames_are_never_reindexed() {
        let drafts = source().replay(&SnapshotScope::new("chat", "presence:all"), None);
        assert!(
            drafts.is_empty(),
            "presence is firehose-only — never a durable snapshot"
        );
    }

    /// **The owner_token is the canonical `chat` subsystem token.**
    #[test]
    fn owner_token_is_chat() {
        assert_eq!(source().owner_token(), "chat");
    }

    // ── CHAT-P6 (P-400): the replay-SKELETON snapshot emission THROUGH THE OUTBOX ─────────────────
    //
    // The CHAT-P6 GATE: the replay skeleton re-emits `chat.*.snapshot` through the OUTBOX (the SAME
    // outbox→bus→live-consumer path durable writes take, contract 2.6 / Bus §4.9), for a sub-artifact
    // scope — 0 snapshots emitted OFF the outbox. The Bus's `reindex(...)` driver dispatches the scope
    // to chat's `ChatReindexSource::replay` and emits each draft through the outbox at its
    // DETERMINISTIC `snapshot_event_id` (cold == live, idempotent re-run). This is the SKELETON; the
    // full Search/Refs/Notif replay PARITY is CHAT-P21 (the named floor).

    use myelin_events::{
        reindex, Actor, EmitContextBase, OutboxStore, Region as EvRegion, TenantId as EvTenantId,
        Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: EvTenantId("acme".into()),
            region: EvRegion("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                EvTenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            caused_by: None,
        }
    }

    /// **The replay skeleton emits `chat.message.snapshot` THROUGH THE OUTBOX for a replayed scope
    /// (the CHAT-P6 GATE; contract 2.6).** Drive the Bus `reindex` over a `message:m1` scope: chat's
    /// source replays the snapshot draft and the driver emits it into the outbox — the snapshot row is
    /// present at its deterministic id (0 snapshots emitted off the outbox), and a re-run emits 0 new
    /// (cold == live, idempotent).
    #[test]
    fn replay_skeleton_emits_chat_snapshot_through_the_outbox() {
        let src = source();
        let sources: &[&dyn ReindexSource] = &[&src];
        let scope = SnapshotScope::new("chat", "message:m1");
        let mut outbox = OutboxStore::new();

        // The drafts the skeleton would emit (to read their deterministic ids back off the outbox).
        let drafts = src.replay(&scope, None);
        assert_eq!(
            drafts.len(),
            1,
            "the message:m1 scope replays exactly one message snapshot"
        );
        assert_eq!(drafts[0].type_.0, "chat.message.snapshot");

        // Drive the reindex THROUGH the outbox — the snapshot must land on the outbox, never off it.
        let receipt = reindex(&scope, None, sources, &mut outbox, ctx_base())
            .expect("reindex through outbox");
        assert_eq!(
            receipt.snapshots_emitted, 1,
            "one chat.message.snapshot emitted through the outbox"
        );
        // 0 snapshots emitted off the outbox: the row is present at its deterministic id.
        let row = outbox
            .row(&drafts[0].event_id())
            .expect("the chat.message.snapshot row is on the outbox (never off it)");
        assert_eq!(row.envelope.type_.0, "chat.message.snapshot");

        // A re-run is idempotent (cold == live, BUS-D5): 0 new snapshots emitted (the deterministic id
        // is already on the outbox → ON CONFLICT DO NOTHING).
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

    /// **An unknown-owner reindex is a LOUD error, never a silent empty emit (the skeleton fails
    /// loud).** Driving `reindex` for a scope chat does not own yields `NoSourceForOwner` — 0 snapshots
    /// emitted off ANY path.
    #[test]
    fn reindex_for_a_non_chat_owner_is_loud_not_a_silent_empty_emit() {
        let src = source();
        let sources: &[&dyn ReindexSource] = &[&src];
        let scope = SnapshotScope::new("git", "commit:all");
        let mut outbox = OutboxStore::new();
        let err = reindex(&scope, None, sources, &mut outbox, ctx_base())
            .expect_err("chat does not own the git scope — a loud error, not a silent empty emit");
        assert!(matches!(
            err,
            myelin_events::ReindexError::NoSourceForOwner(_)
        ));
    }
}
