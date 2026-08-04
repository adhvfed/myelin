use myelin_chat::events::CHAT_DURABLE_TOKENS;
use myelin_chat::glue::{chat_channel_scope, CHAT_RESYNC_SNAPSHOT_TOKENS};
use myelin_events::{FirehoseScope, ScopeKind};

fn provider_chat_scope(channel_id: &str) -> Result<FirehoseScope, myelin_events::FirehoseError> {
    chat_channel_scope(channel_id)
}

fn consumer_classifies(raw: &str) -> Result<ScopeKind, myelin_events::FirehoseError> {
    FirehoseScope::parse(raw).map(|s| s.kind())
}

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

    assert_eq!(
        consumer_classifies("channel:eng").unwrap(),
        ScopeKind::Channel
    );
}

#[test]
fn cdc_3_5_consumer_rejects_an_unbounded_chat_scope() {
    for bad in ["*", "", "*all", "any*"] {
        assert!(
            provider_chat_scope(bad).is_err(),
            "an unbounded channel scope `channel:{bad}` must be rejected"
        );
    }
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
