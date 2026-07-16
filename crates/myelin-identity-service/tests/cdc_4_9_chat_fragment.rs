//! # The CDC pair for contract 4.9 — Id's compiled **Chat** ReBAC fragment (P-ID-30 / P-323)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment — each subsystem declares
//! relations and permissions, compiled into ONE cell schema; Identity owns the engine, the
//! admit-contract, and the core hierarchy and never invents object ids). The engine half is pinned by
//! `cdc_4_9_namespace_engine.rs` (P-068); the Git fragment by `cdc_4_9_git_fragment.rs`; the Knowledge
//! fragment by `cdc_4_9_knowledge_fragment.rs`; the CI fragment by `cdc_4_9_ci_fragment.rs`; the Issues
//! fragment by `cdc_4_9_issue_fragment.rs`. THIS file pins the Identity-side compiled **Chat** fragment
//! (the rich rewrites Id owns, P-ID-30 / P-323) — the FIFTH and FINAL fragment, CLOSING the M1
//! engine-only floor.
//!
//! - The **PROVIDER** is Identity's namespace engine ([`StoreBackedCheck`] over `with_core_hierarchy`):
//!   it admits Id's compiled Chat [`FragmentDef`]s, resolves the Chat permissions through the userset
//!   operators, and never invents an id.
//! - The **CONSUMER** is the Chat subsystem, which gates a channel/message read ONLY on a resolved
//!   grant + lists the ambient channel set via `list_objects(subject, read, channel)` keyed on
//!   `channel.id` (§7.3).
//!
//! **The headline invariants this CDC behaviourally pins (CHAT authz side):**
//! - **`channel.read = member ∪ parent_project->view` (§5):** a direct member OR a project reader can
//!   read the channel; a NON-member (in neither arm) cannot — and is ABSENT from the ambient
//!   channel-list `list_objects(subject, read, channel)` (the search-as-non-member 0-results gate).
//! - **`message.view = parent_channel->read` (§5):** a message inherits its channel's readability, so a
//!   non-member's message view DENIES by construction.
//! - **the ambient channel-list conjoins in ONE query** keyed on `channel.id` (§7.3, contract 4.3).

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind,
    RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    chat_fragment, ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer,
    StoreBackedCheck, TupleStore, CHANNEL_MEMBER, CHANNEL_READ, MESSAGE_VIEW,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn subject(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

/// The PROVIDER surface seeded with `tuples` — the core org/team/project hierarchy is preloaded, then
/// Id's compiled Chat fragment is admitted on top (so `channel.read`'s `parent_project->view`
/// inheritance terminates on the core `project.view`).
fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed tuples");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_chat_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Id's compiled Chat fragment admits: {admit:?}"
        );
    }
    svc
}

/// **PROVIDER → the compiled Chat fragment ADMITS into the cell schema (the engine-only-floor
/// closure).** Id declares + compiles its Chat fragment via the fragment-admit contract; every Chat
/// object type admits on top of the core hierarchy; the headline permissions are compiled. With this,
/// all five subsystem fragments exist.
#[test]
fn cdc_4_9_id_compiled_chat_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);
    let ns = svc.namespace();
    for ty in ["channel", "message", "unfurl"] {
        assert!(
            ns.object_types().contains(&ty.to_string()),
            "`{ty}` admitted into the cell schema"
        );
    }
    assert!(ns.resolve_permission("channel", CHANNEL_READ).is_some());
    assert!(ns.resolve_permission("message", MESSAGE_VIEW).is_some());
    assert!(ns.resolve_permission("unfurl", MESSAGE_VIEW).is_some());
    assert!(
        ns.is_watchable("channel"),
        "the channel is watchable (the 50k-density read-fanout, §7.5)"
    );
}

/// **CONSUMER → PROVIDER: `channel.read` resolves via `member` AND via `parent_project->view`; a
/// non-member DENIES (§5).** alice is a DIRECT member of channel:general; bob is a project reader (the
/// ambient arm); carol is neither — she denies. `message.view` inherits the channel (a member sees the
/// message, a non-member does not).
#[test]
fn cdc_4_9_channel_read_resolves_via_member_and_via_project() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // alice is a DIRECT member of the channel.
            add("channel:general", CHANNEL_MEMBER, "p:alice"),
            // bob inherits via the project-read arm (parent_project->view).
            add("project:proj", "reader", "p:bob"),
            add("channel:general", "parent_project", "project:proj#view"),
            // A message in the channel (inherits channel.read).
            add("message:m1", "parent_channel", "channel:general#read"),
        ],
    );
    let can_read = |actor: &Principal, channel: &str| {
        matches!(
            svc.check(
                actor,
                &Permission(CHANNEL_READ.into()),
                &ArtifactRef(channel.into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    let can_view_msg = |actor: &Principal, message: &str| {
        matches!(
            svc.check(
                actor,
                &Permission(MESSAGE_VIEW.into()),
                &ArtifactRef(message.into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    // alice reads via the direct member arm.
    assert!(
        can_read(&subject("p:alice"), "channel:general"),
        "a direct member reads the channel (the `member` arm)"
    );
    // bob reads via the ambient parent_project->view arm.
    assert!(
        can_read(&subject("p:bob"), "channel:general"),
        "a project reader reads the channel (the parent_project->view arm)"
    );
    // carol (neither member nor project reader) is DENIED.
    assert!(
        !can_read(&subject("p:carol"), "channel:general"),
        "a non-member denies (in neither arm of channel.read)"
    );
    // message.view inherits the channel: alice sees it, carol does not.
    assert!(
        can_view_msg(&subject("p:alice"), "message:m1"),
        "a member views the channel's messages (message.view = parent_channel->read)"
    );
    assert!(
        !can_view_msg(&subject("p:carol"), "message:m1"),
        "a non-member cannot view the channel's messages (by construction)"
    );
}

/// Build a wired `list_objects` over Id's compiled Chat fragment + a LIVE S8 index fed off the bus from
/// `grants`, at an explicit cardinality `cap` (so the Ids↔Filter switch is deterministic). The S8
/// reverse index projects only DIRECT principal-subject grants, so a direct channel `member` is a
/// candidate row; the inherited `parent_project->view` arm of the ambient channel scan is resolved by
/// the consumer's JOIN against `authz_visible` on the `Filter` push-down (P-ID-12).
fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> ListObjects {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in chat_fragment::chat_fragment_defs() {
        let admit = namespace.admit(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Chat `{}` fragment admits",
            def.object_type.0
        );
    }
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            grants,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    ListObjects::with_cap(store, namespace, index, cap)
}

/// **CONSUMER → PROVIDER: the ambient channel list conjoins in ONE query — `list_objects(subject, read,
/// channel)` pushes down to the `channel.id` Filter (§7.3, contract 4.3).** Above the cap the channel
/// scan returns the S8 push-down naming the consumer's OWN id column (`channel.id`) which the channel
/// list's query planner conjoins in ONE query (no N+1, never a post-filter).
#[test]
fn cdc_4_9_channel_list_conjoins_in_one_query() {
    let s = scope("acme");
    // alice is a direct member of a handful of channels; with the cap BELOW that slice the scan pushes
    // down to the Filter (the channel list's one-query conjoin).
    let mut grants: Vec<TupleDelta> = Vec::new();
    for i in 0..8 {
        grants.push(add(&format!("channel:c-{i}"), CHANNEL_MEMBER, "p:alice"));
    }
    let lo = wired(2, &s, &grants);
    let result = lo.list_objects(
        &s,
        &subject("p:alice"),
        &Permission(CHANNEL_READ.into()),
        &ObjectType("channel".into()),
        &at_latest(),
    );
    match result {
        ListObjectsResult::Filter { set_expr, .. } => match set_expr {
            SetExpr::InRelation { via_column, .. } => {
                assert_eq!(
                    via_column,
                    ColRef {
                        table: "channel".into(),
                        column: "id".into()
                    },
                    "the channel-list Filter names the consumer's own id column (channel.id, §7.3) — \
                     one query, no N+1"
                );
            }
            other => {
                panic!("the channel-list Filter is the InRelation push-down shape, got {other:?}")
            }
        },
        ListObjectsResult::Ids { .. } => {
            panic!("above the cap the channel list must push down to the channel.id Filter")
        }
    }
}

/// **CONSUMER → PROVIDER: a NON-MEMBER's channel list is leak-free — a channel they are not a member of
/// is ABSENT (no count leak; the search-as-non-member 0-results gate).** Below the cap the channel list
/// materialises `Ids` carrying ONLY the subject's directly-member channels; a channel they are not a
/// member of never becomes a candidate (the S8 reverse index keys on `(subject, relation)` — a
/// non-member channel is not even a candidate, never a post-filter).
#[test]
fn cdc_4_9_non_member_channel_list_is_leak_free() {
    let s = scope("acme");
    let grants = vec![
        // alice is a member of two channels → her visible channel list.
        add("channel:visible-1", CHANNEL_MEMBER, "p:alice"),
        add("channel:visible-2", CHANNEL_MEMBER, "p:alice"),
        // A private channel alice is NOT a member of — the leak witness she must never see.
        add("channel:secret", CHANNEL_MEMBER, "p:other"),
    ];
    let lo = wired(100, &s, &grants);
    let result = lo.list_objects(
        &s,
        &subject("p:alice"),
        &Permission(CHANNEL_READ.into()),
        &ObjectType("channel".into()),
        &at_latest(),
    );
    let ids = match result {
        ListObjectsResult::Ids { ids, .. } => ids.into_iter().map(|o| o.0).collect::<Vec<String>>(),
        ListObjectsResult::Filter { .. } => {
            panic!("below the cap the channel list materialises Ids")
        }
    };
    assert!(
        ids.iter().any(|i| i == "channel:visible-1")
            && ids.iter().any(|i| i == "channel:visible-2"),
        "alice's channel list shows her two member channels: {ids:?}"
    );
    assert!(
        !ids.iter().any(|i| i == "channel:secret"),
        "a private channel alice is not a member of is ABSENT (leak-free, no count leak; \
         search-as-non-member 0 results): {ids:?}"
    );
}
