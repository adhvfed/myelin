//! # The CDC pair for contract 3.5 — chat's firehose scope shape (CHAT-P3 / P-245)
//!
//! **Contract:** `contract-index.md` row 3.5 (the firehose transport + the resume-cursor
//! subscription protocol — `subscribe(stream, scope, cursor?)` / `resume(stream, scope, last_seq)`;
//! `resync_required → *.snapshot` fallback; **scope is a bounded selector, never `*`**:
//! board:/doc:/channel:). **Reconciliation:** OQ-J (the resume-cursor protocol co-designed ONCE for
//! a hot board / a hot doc / a hot channel — all three use it identically; scope a bounded selector).
//! Owning architecture: chat `03-events-contracts-and-glue.md` §1.2 (the firehose-only set rides the
//! frozen resume-cursor protocol; `fan.<tenant>.<channel>`).
//!
//! ## The seam this pair pins (chat VALIDATES its scope shape; the Bus owns the protocol)
//! - **PROVIDER (chat — [`myelin_chat::glue`])** declares its per-view scope SHAPE `channel:<id>`
//!   (the bounded selector for one channel's live delivery + presence). NO transport here — only the
//!   scope shape (the transport is CHAT-P9/P10).
//! - **CONSUMER (the Bus — [`myelin_events::FirehoseScope`])** owns the frozen `*`-rejecting
//!   chokepoint: it ADMITS chat's `channel:<id>` as a bounded [`myelin_events::ScopeKind::Channel`]
//!   and REJECTS any unbounded / `*` form. Chat does NOT author a second scope validator.

use myelin_chat::events::CHAT_DURABLE_TOKENS;
use myelin_chat::glue::{chat_channel_scope, CHAT_RESYNC_SNAPSHOT_TOKENS};
use myelin_events::{FirehoseScope, ScopeKind};

/// **PROVIDER side of 3.5** — chat declares its per-view scope SHAPE for a channel: `channel:<id>`.
/// The provider's promise: chat's live-delivery scope is ALWAYS a bounded single-channel selector
/// (never the tenant firehose, never `*`).
fn provider_chat_scope(channel_id: &str) -> Result<FirehoseScope, myelin_events::FirehoseError> {
    chat_channel_scope(channel_id)
}

/// **CONSUMER side of 3.5** — the Bus's frozen `*`-rejecting chokepoint classifies a scope. The
/// consumer's promise: it admits ONLY a bounded selector and rejects anything unbounded. (Chat's
/// scope is validated THROUGH this — there is no second chat validator.)
fn consumer_classifies(raw: &str) -> Result<ScopeKind, myelin_events::FirehoseError> {
    FirehoseScope::parse(raw).map(|s| s.kind())
}

/// The 3.5 pair, end-to-end: the PROVIDER (chat) declares a `channel:<id>` scope and the CONSUMER
/// (the Bus chokepoint) ADMITS it as a bounded `Channel` selector — the dated green artifact (the
/// contract-coverage scanner's 3.5 chat row; the unbounded-scope signal = 0).
#[test]
fn cdc_3_5_chat_provider_declares_channel_scope_consumer_admits_bounded() {
    let scope = provider_chat_scope("eng").expect("chat's channel scope is bounded");
    assert_eq!(
        scope.kind(),
        ScopeKind::Channel,
        "chat's per-view scope is channel:<id>"
    );
    assert_eq!(
        scope.selector(),
        "channel:eng",
        "the channel scope round-trips its selector"
    );

    // the consumer (the Bus chokepoint) classifies chat's declared scope to the bounded Channel kind.
    assert_eq!(
        consumer_classifies("channel:eng").unwrap(),
        ScopeKind::Channel
    );
}

/// **The CONSUMER REJECTS an unbounded / `*` chat scope (the 0-unbounded-scope gate — scope is never
/// `*`).** `*`, an empty channel id, a `*`-containing id, a bare `channel:` are ALL rejected by the
/// one chokepoint — chat cannot declare an unbounded subscription (the `*`-rejection generalises to
/// `channel:` exactly as for board/doc/inbox).
#[test]
fn cdc_3_5_consumer_rejects_an_unbounded_chat_scope() {
    // `*`, an empty id, and a `*`-containing id are unbounded — rejected. (A literal channel named
    // "all" — `channel:all` — is a BOUNDED selector, NOT rejected: only `*`/empty is unbounded.)
    for bad in ["*", "", "*all", "any*"] {
        assert!(
            provider_chat_scope(bad).is_err(),
            "an unbounded channel scope `channel:{bad}` must be rejected"
        );
    }
    // the chokepoint itself rejects the raw unbounded forms.
    for raw in ["*", "channel:*", "channel:", "team:eng"] {
        let r = FirehoseScope::parse(raw);
        assert!(
            r.is_err(),
            "the Bus chokepoint must reject the unbounded scope `{raw}`"
        );
        assert!(
            r.unwrap_err().is_over_broad_scope(),
            "`{raw}` is an over-broad-scope rejection"
        );
    }
}

/// **The `resync_required → *.snapshot` fallback names chat's DURABLE snapshot tokens (3.5).** A
/// `resync_required` client cold-rebuilds from the chat reindex-from-source snapshots (channel /
/// message / thread), each a registered DURABLE token (the cold rebuild rides the durable outbox, not
/// a firehose frame — arch §6 / contract 2.6).
#[test]
fn cdc_3_5_resync_snapshot_fallback_names_chat_durable_snapshots() {
    assert_eq!(
        CHAT_RESYNC_SNAPSHOT_TOKENS.len(),
        3,
        "channel/message/thread snapshots"
    );
    for snap in CHAT_RESYNC_SNAPSHOT_TOKENS {
        assert!(
            CHAT_DURABLE_TOKENS.contains(snap),
            "the resync fallback `{snap}` must be a DURABLE token (rides the outbox)"
        );
        assert!(
            snap.ends_with(".snapshot"),
            "`{snap}` is a *.snapshot reindex-from-source token"
        );
    }
}
