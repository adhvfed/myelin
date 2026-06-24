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
//!
//! # Full replay PARITY — Search/Refs/Notif read-models rebuild, ONE path (CHAT-P21 / P-416, M4-C7)
//!
//! CHAT-P6 (P-400) shipped the SKELETON: chat re-emits `chat.*.snapshot` through the OUTBOX, idempotent
//! on the deterministic id. **CHAT-P21 completes it** — the three Chat-fed derived read-models
//! (**Search** the message index, contract 6.3/6.4; **Refs** the `refs.edge.created` graph, 5.2/5.4;
//! **Notif** the notify-reason read-model, 7.1/7.6) **rebuild from the SAME `*.snapshot` re-emit through
//! the SAME live consumer step** they ingest steady-state. There is **no recovery-only code path**: the
//! rebuild drives [`ChatReadModelConsumer::ingest`] — the identical step a live `chat.message.snapshot`
//! takes — so steady-state and recovery cannot drift (EI-04 §5.3). The rebuild stays **ACL-correct**:
//! the Search leg conjoins the frozen `list_objects` Filter at QUERY time (the [`crate::search`] feeder),
//! so a non-member sees 0 rebuilt rows from a channel they are not in (OQ-E — the reindexing consumer
//! composes the Filter; CHAT-D11 holds over the rebuilt index identically to the live one). An **erased
//! subject emits a tombstone** on rebuild: an erased aggregate is removed from [`ChatReindexSource`], so
//! the replay SKIPS it (X-7) and the cold read-models do NOT resurrect its body (0 recoverable PII; the
//! full multi-holder erasure RECEIPT remains the CHAT-P22 floor).
//!
//! The [`reindex_parity_hash`] is the CHAT-D15 green artifact: a deterministic digest over all three
//! rebuilt read-models. `cold_hash == live_hash` proves byte-parity (the reindex-parity-hash-mismatch
//! signal = 0). The drill harness is `tests/drill_chat_d15_reindex_parity.rs` (the chained
//! wipe→replay→hash-compare); the CDC pair for 2.6/6.4 is `tests/cdc_2_6_6_4_chat_reindex_parity.rs`.

use std::collections::{BTreeMap, BTreeSet};

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventEnvelope, EventType, ReindexSource, SnapshotDraft,
    SnapshotScope, Visibility,
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// FULL REPLAY PARITY — the three Chat-fed read-models rebuild through ONE consumer step (CHAT-P21)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The owner's `project(message)` body the read-model consumers fetch per `*.snapshot` (contract
/// 5.6 — references-not-payloads).** A `chat.message.snapshot` payload is opaque refs (the message
/// URN + version); to materialize a read-model row, the live consumer fetches the message's
/// PROJECTABLE content from the owner — NEVER a cross-DB read of a derived store (EI-04 §5.3). This
/// is the SAME `project` body the live emit and the cold replay both go through, so cold == live by
/// construction (the no-cross-db floor is structural). PII-free: the projection carries the
/// markdown-subset body text + the structured reference targets (the mention targets are PSEUDONYMOUS
/// member URNs, never names — erasure-safe).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageProjection {
    /// The full-text body prose (the markdown-subset render — what Search analyses).
    pub body_text: String,
    /// The home channel id (the `channel.read` ACL gate object — the conjoin keys on this; a
    /// non-member of this channel sees 0 rebuilt rows for the message, CHAT-D11).
    pub channel_id: String,
    /// The structured reference edges this message body produces (`(target, rel)` — what Refs mirrors).
    /// `rel` is the frozen `mentions|links|embeds` token (contract 5.4).
    pub edges: Vec<(String, String)>,
    /// The pseudonymous member URNs this message @-mentions (the Notif notify-reason producers — each
    /// mentioned member gets a `Mentioned` inbox reason, contract 7.6).
    pub mentions: Vec<String>,
}

/// **The owner's `project(ref)` fetcher (contract 5.6) — the ONLY way a read-model materializes a
/// message (NEVER a cross-DB read).** Backed by the owner's in-memory truth keyed by message URN; the
/// real binding resolves the message row + decrypts the body behind its per-subject DEK. The SAME
/// fetcher serves the live emit and the cold replay (one body shape → cold == live). An erased message
/// is ABSENT (its DEK is shredded), so a fetch returns `None` and the read-model gets no row — the
/// rebuild cannot resurrect erased PII (X-7).
pub trait MessageProjectFetcher {
    /// Fetch the projectable content for a message URN, or `None` if it is gone/erased (the read-model
    /// then materializes no row — a tombstone, never resurrected PII).
    fn project(&self, message_ref: &str) -> Option<MessageProjection>;
}

/// **A Chat-fed derived read-model (the shape Search/Refs/Notif each are): a projection built ONLY by
/// ingesting events — live OR `*.snapshot`, through the SAME [`ChatReadModelConsumer::ingest`] step
/// (that single step is what makes cold == live).** It NEVER reads an owner DB; it materializes its row
/// via the owner's [`MessageProjectFetcher::project`] (5.6). Three sub-projections, one per consumed
/// read-model, so the parity digest covers all three at once:
/// - **Search** — message URN → indexed `(body_text, channel_id)` (the analyzable doc + the ACL key);
/// - **Refs** — the `refs.edge.created` forward edge set (`edge:<source>-><target>` → `rel`);
/// - **Notif** — the notify-reason rows (`(recipient_member, message_ref) → reason`).
///
/// Idempotent on `event_id` (a redelivered live event OR a re-emitted snapshot is a no-op — the same
/// effectively-once the `consumer_dedup` ledger gives). LWW on version (a late snapshot of an older
/// version never clobbers a newer live row).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatReadModelConsumer {
    /// SEARCH read-model: message URN → (version, (body_text, channel_id)). The ACL conjoin keys on the
    /// channel at QUERY time (OQ-E); the indexed row holds the channel so the rebuild stays ACL-correct.
    search: BTreeMap<String, (u64, (String, String))>,
    /// REFS read-model: `edge:<source>-><target>` → (version, rel). The forward edge graph Refs mirrors.
    refs: BTreeMap<String, (u64, String)>,
    /// NOTIF read-model: `<recipient_member>|<message_ref>` → (version, reason). The notify-reason rows.
    notif: BTreeMap<String, (u64, String)>,
    /// `event_id`s already applied (the in-store dedup — effectively-once; modeled here so the consumer
    /// is self-contained, the same the `consumer_dedup` ledger gives the real wiring).
    applied: BTreeSet<String>,
}

impl ChatReadModelConsumer {
    /// A fresh, empty (wiped) read-model — the cold-rebuild starting point.
    pub fn new() -> ChatReadModelConsumer {
        ChatReadModelConsumer::default()
    }

    /// **The ONE ingest step — live AND `*.snapshot` use it (cold == live).** Apply a chat envelope to
    /// all three read-models, idempotently on `event_id`. The consumer fetches the message's
    /// projectable content via `fetcher.project` (5.6 — never a cross-DB read); an ABSENT (erased)
    /// message materializes NO row (the rebuild does not resurrect shredded PII, X-7). Returns `true`
    /// iff a read-model changed.
    ///
    /// Only `chat.message.snapshot` (and the live `chat.message.created`/`.edited`, which carry the
    /// SAME envelope shape) feed the message read-models; a `chat.channel.snapshot` /
    /// `chat.thread.snapshot` is ingested for completeness but produces no Search/Refs/Notif message
    /// row (those read-models are message-keyed). The version is read from the payload's `version`
    /// field (the owner stamps it; a snapshot carries the live version — that is what makes a snapshot
    /// of version `v` indistinct from the live event of version `v`).
    pub fn ingest(&mut self, env: &EventEnvelope, fetcher: &dyn MessageProjectFetcher) -> bool {
        if !self.applied.insert(env.event_id.0.clone()) {
            return false; // already applied (effectively-once) — no double effect.
        }
        // Only a message event feeds the message-keyed read-models (the parity surface for CHAT-D15).
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
        // The owner's project(ref) — the ONLY content source (5.6, never a cross-DB read). An ABSENT
        // (erased) message yields no row: the rebuild cannot resurrect shredded PII (X-7).
        let proj = match fetcher.project(&message_ref) {
            Some(p) => p,
            None => return false,
        };

        let mut changed = false;
        // ── SEARCH: index the body + the ACL channel key (LWW on version). ──
        changed |= lww_insert(
            &mut self.search,
            message_ref.clone(),
            version,
            (version, (proj.body_text.clone(), proj.channel_id.clone())),
        );
        // ── REFS: the forward edge set (one row per structured reference edge). ──
        for (target, rel) in &proj.edges {
            let key = format!("edge:{message_ref}->{target}");
            changed |= lww_insert(&mut self.refs, key, version, (version, rel.clone()));
        }
        // ── NOTIF: a notify-reason row per @-mentioned member (the write-fanout producers, 7.6). ──
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

    /// The number of indexed Search rows (one per non-erased message).
    pub fn search_len(&self) -> usize {
        self.search.len()
    }

    /// The number of Refs forward edges materialized.
    pub fn refs_len(&self) -> usize {
        self.refs.len()
    }

    /// The number of Notif notify-reason rows materialized.
    pub fn notif_len(&self) -> usize {
        self.notif.len()
    }

    /// `true` iff the Search read-model indexes `message_ref` (used to assert an erased message's row
    /// is ABSENT after a rebuild — 0 resurrected PII).
    pub fn search_indexes(&self, message_ref: &str) -> bool {
        self.search.contains_key(message_ref)
    }

    /// **The ACL-correct rebuilt Search rows VISIBLE to a viewer whose readable channel set is
    /// `readable_channels` (the OQ-E Filter conjoin, modeled).** The reindexing consumer conjoins the
    /// frozen `list_objects(read, channel)` Filter so a rebuild stays ACL-correct: a row whose
    /// `channel_id` is NOT in the viewer's readable set is EXCLUDED — 0 unfiltered rebuilt rows. A
    /// non-member of every channel (`readable_channels` empty) sees 0 rows (CHAT-D11 over the rebuilt
    /// index, identical to the live one). Returns the visible message URNs, sorted (deterministic).
    pub fn search_visible_to(&self, readable_channels: &BTreeSet<String>) -> Vec<String> {
        self.search
            .iter()
            .filter(|(_, (_, (_, channel)))| readable_channels.contains(channel))
            .map(|(message_ref, _)| message_ref.clone())
            .collect()
    }
}

/// LWW insert into a versioned read-model map: apply iff this is a NEWER (or first) version of the key,
/// so a late snapshot of an older version never clobbers newer bytes. Returns `true` iff it changed.
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

/// The frozen Notif notify-reason a chat @-mention produces (`mentioned`, contract 7.6 — the
/// write-fanout high-signal reason, [`crate::glue::RULE_KEY_MENTIONED`]). A `&'static str` so the
/// parity digest pins the token, never a literal.
pub const NOTIF_REASON_MENTIONED: &str = crate::glue::RULE_KEY_MENTIONED;

/// **The CHAT-D15 reindex-parity digest (the dated green artifact).** A deterministic, canonical-bytes
/// hash over ALL THREE rebuilt read-models (Search ∥ Refs ∥ Notif). Two read-models with the same
/// digest are byte-identical for the parity assertion: `reindex_parity_hash(cold) ==
/// reindex_parity_hash(live)` is the CHAT-D15 pass (the reindex-parity-hash-mismatch signal = 0). The
/// `BTreeMap`s iterate in key order and `serde_json` serializes deterministically over that, so the
/// digest is process- and platform-stable (it must match in a CI rerun and a real OLTP binding alike).
/// Reuses the SAME dependency-free FNV-1a the Bus snapshot id uses ([`myelin_events::snapshot_event_id`]
/// posture) — an idempotency/parity key, not a security primitive.
pub fn reindex_parity_hash(rm: &ChatReadModelConsumer) -> String {
    // Canonical bytes: the three read-models, each as a key-ordered map of key → (version, value).
    let view = serde_json::json!({
        "search": rm.search,
        "refs": rm.refs,
        "notif": rm.notif,
    });
    let bytes = serde_json::to_vec(&view).expect("read-model serializes");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a offset basis (the Bus snapshot-id basis)
    for b in &bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3); // FNV-1a prime
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

    // ── CHAT-P21 (P-416): the FULL replay PARITY — Search/Refs/Notif read-models rebuild, ONE path ──
    //
    // The CHAT-P21 GATE (CHAT-D15): the three Chat-fed read-models rebuild from the SAME `*.snapshot`
    // re-emit through the SAME live consumer step they ingest steady-state — 0 recovery-only code paths.
    // The rebuild stays ACL-correct (the channel-keyed Filter conjoin); an erased subject emits a
    // tombstone (its row is absent — 0 resurrected PII); the reindex-parity hash matches the live hash.

    use std::collections::BTreeSet;

    /// A `MessageProjectFetcher` over an in-memory owner truth (the owner's `project(message)`, 5.6).
    /// The SAME fetcher serves the live emit and the cold replay → cold == live. An ABSENT (erased)
    /// message returns `None` (the read-model materializes no row — X-7).
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
    // A tiny helper so the corpus reads cleanly (tag is a no-op identity on the projection value).
    impl MessageProjection {
        fn tagged(self, _message_ref: &str) -> MessageProjection {
            self
        }
    }

    /// Build the `*.snapshot` envelope a relay delivers for a chat message draft (the consumer's input
    /// shape — the SAME shape the live `chat.message.created` carries; that is cold == live).
    fn message_snapshot_envelope(message_ref: &str, version: u64) -> EventEnvelope {
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, Region as EvRegion, TenantId as EvTenantId,
            Timestamp,
        };
        let agg = AggregateKey(message_ref.to_string());
        EventEnvelope {
            event_id: myelin_events::snapshot_event_id(&agg, version),
            type_: EventType(events::CHAT_MESSAGE_SNAPSHOT.into()),
            schema_ver: 1,
            tenant: EvTenantId("acme".into()),
            region: EvRegion("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                EvTenantId("acme".into()),
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

    /// A two-message corpus: m1 in channel c1 links an issue + mentions alice; m2 in channel c2
    /// mentions bob. The SAME corpus drives the live emit and the cold replay (the owner's truth).
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

    /// **The steady-state-vs-recovery ONE-PATH identity (the CI gate; 0 recovery-only code paths).** A
    /// read-model built by the LIVE consumer step and a read-model REBUILT by the cold replay are
    /// byte-identical — they go through the SAME [`ChatReadModelConsumer::ingest`], so the
    /// reindex-parity hash matches.
    #[test]
    fn steady_state_and_recovery_share_one_path_parity_hash_matches() {
        let proj = corpus_projector();
        let src = parity_source();
        let scope = SnapshotScope::new("chat", "message:all");

        // LIVE: ingest the live message events (modeled as the same snapshot drafts the owner emits —
        // the cold==live invariant is precisely that these are the same envelope shape).
        let mut live = ChatReadModelConsumer::new();
        for draft in src.replay(&scope, None) {
            let env = message_snapshot_envelope(&draft.aggregate.0, draft.version);
            live.ingest(&env, &proj);
        }

        // COLD: a WIPED read-model, rebuilt ONLY from the reindex snapshot replay through the
        // outbox→relay path (the §4.9 ONLY rebuild path). Reindex → read the rows → ingest into the
        // wiped store via the SAME ingest step.
        let mut cold = ChatReadModelConsumer::new();
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = myelin_events::OutboxStore::new();
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
        for draft in src.replay(&scope, None) {
            let row = outbox.row(&draft.event_id()).expect("snapshot row present");
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

        // THE GREEN ARTIFACT: the reindex-parity hash matches (cold == live, byte-identical across all
        // three read-models). 0 reindex-parity-hash-mismatch.
        assert_eq!(
            reindex_parity_hash(&cold),
            reindex_parity_hash(&live),
            "the cold-rebuilt read-models byte-match live across Search/Refs/Notif (CHAT-D15)"
        );
    }

    /// **A re-run of the rebuild is IDEMPOTENT (the deterministic snapshot id no-ops the duplicate).** A
    /// redelivered snapshot is absorbed on `event_id` — the parity hash is unchanged after a re-run.
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

        // Re-deliver every snapshot — the deterministic id no-ops the duplicates (effectively-once).
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

    /// **An erased subject emits a TOMBSTONE on rebuild — its body does NOT resurrect (0 recoverable
    /// PII; X-7).** Erase m2 at the owner (removed from BOTH the reindex source and the project fetcher
    /// — its DEK is shredded). A rebuild SKIPS it: the cold Search read-model does NOT index m2, and its
    /// notify row is absent. m1 is untouched (the erasure is surgical, not a wipe).
    #[test]
    fn an_erased_subject_emits_a_tombstone_on_rebuild_no_resurrection() {
        let mut proj = corpus_projector();
        let mut src = parity_source();
        let scope = SnapshotScope::new("chat", "message:all");

        // ERASE m2: removed from the source of truth (the *.erased tombstone) AND its DEK shredded
        // (the project fetch returns None).
        assert!(src.erase("myelin://acme/chat/message/m2"), "m2 was present");
        proj.erase("myelin://acme/chat/message/m2");

        // Rebuild from the post-erase replay.
        let mut cold = ChatReadModelConsumer::new();
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = myelin_events::OutboxStore::new();
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("post-erase reindex");
        for draft in src.replay(&scope, None) {
            let row = outbox.row(&draft.event_id()).expect("snapshot row");
            cold.ingest(&row.envelope, &proj);
        }

        assert_eq!(
            cold.search_len(),
            1,
            "only m1 rebuilt — m2 did not resurrect"
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

    /// **A rebuild stays ACL-correct (the reindexing consumer conjoins the Filter; 0 unfiltered rebuilt
    /// rows).** The rebuilt Search read-model holds the channel key; conjoining the viewer's readable
    /// channel set EXCLUDES rows from channels they cannot read — a non-member of c2 sees only m1 (in
    /// c1), and a non-member of EVERY channel sees 0 rows (CHAT-D11 over the rebuilt index).
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
            let row = outbox.row(&draft.event_id()).expect("snapshot row");
            cold.ingest(&row.envelope, &proj);
        }

        // A viewer who can read ONLY c1 sees only m1 (m2 in c2 is excluded — 0 unfiltered rows).
        let only_c1: BTreeSet<String> = ["c1".to_string()].into_iter().collect();
        assert_eq!(
            cold.search_visible_to(&only_c1),
            vec!["myelin://acme/chat/message/m1".to_string()],
            "the rebuild conjoins the channel Filter — only c1's message is visible"
        );
        // A non-member of EVERY channel sees 0 rows (CHAT-D11 over the rebuilt index, identical to live).
        assert!(
            cold.search_visible_to(&BTreeSet::new()).is_empty(),
            "a non-member sees 0 rebuilt rows (0 unfiltered rows; CHAT-D11 holds on the rebuild)"
        );
    }

    /// **The Notif notify-reason token is the frozen `mentioned` (contract 7.6).** The parity digest
    /// pins the registered rule-key token, never a literal.
    #[test]
    fn notif_reason_is_the_frozen_mentioned_rule_key() {
        assert_eq!(NOTIF_REASON_MENTIONED, crate::glue::RULE_KEY_MENTIONED);
    }
}
