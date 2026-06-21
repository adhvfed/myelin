//! # The CDC pair for contract 4.9 — the **Chat** ReBAC namespace fragment (CHAT-P2 / P-244)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment — each subsystem declares
//! relations + permissions, compiled into ONE cell schema; Identity owns the engine and never
//! invents object ids). The engine + admit-contract half is pinned by the Identity CDC
//! (`crates/myelin-identity-service/tests/cdc_4_9_namespace_engine.rs`); THIS file pins the **Chat
//! fragment slice** of the same row — the freeze CHAT-P2 ships:
//!
//! - the **CONSUMER** is the **Chat subsystem declaring its namespace fragment at build time**
//!   ([`myelin_chat::rebac_fragment::chat_fragment`]) — the frozen names-only
//!   [`myelin_identity::NamespaceFragment`] carriers Identity admits into the cell schema. The
//!   consumer's promise: it declares exactly the §5 relations (`member` ACL, the `parent_project`
//!   inheritance edge, the `watcher` Notif read-fanout) and the frozen `channel.read = member +
//!   parent_project->read` clause (recon §1).
//! - the **PROVIDER** is Identity's namespace engine ([`StoreBackedCheck`] over the
//!   `with_core_hierarchy` cell schema) — it admits the Chat fragment (`Admitted{fragment_id}`),
//!   resolves the Chat permissions through the userset operators (the `+ parent_project->read` TTU
//!   inheritance, the `& parent_project->admin` intersection, the `message.view = parent_channel->read`
//!   inheritance), and never invents an id.
//!
//! The two sides are pinned here so a drift on either (Chat drops/renames a relation; Identity's
//! admit-contract changes shape) fails this test in the same CI job. **The gate of CHAT-P2 is the
//! build-time compile** — Identity's cell schema compiles against the Chat fragment; this CDC is the
//! mechanical evidence that the frozen shape ADMITS (well-formed) and resolves correctly (a member
//! reads; a public-channel project-reader inherits read; a non-member of a private channel is denied).
//! The permission *rewrites* are wired LIVE on the Chat M4 spine (the membership tuple writes are the
//! CHAT-P8 floor); here we PROVE they are admissible against the real engine TODAY (the freeze anchor).
//!
//! (No cargo-mutants floor: this is a NAMES freeze + admissibility evidence over the already-proven
//! engine + the already-proven Refs grammar — not new load-bearing resolution logic. The resolver
//! mutation floors land with the Chat M4 projection spine.)

use myelin_chat::rebac_fragment::{self, object_types};
use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, NamespaceFragment,
    ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple,
    TupleDelta, Zookie,
};
use myelin_identity_service::{
    FragmentDef, PermissionRule, StoreBackedCheck, TupleStore, Userset,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
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

/// The PROVIDER surface: the engine (with the org→team→project core hierarchy preloaded, so the Chat
/// fragment's `parent_project->…` inheritance has its parent type) seeded with `tuples`.
fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-20T00:00:00Z".into()),
        )
        .expect("seed tuples");
    StoreBackedCheck::new(store)
}

/// The Chat fragment's frozen permission **rewrites** (§5), as the rich engine `FragmentDef` form the
/// Chat M4 spine wires live. CHAT-P2 ships only the names (the [`NamespaceFragment`] carriers); this
/// rich form is the CDC's compile-against-the-engine evidence that the frozen shape — including the
/// `+ parent_project->read` inheritance and the `& parent_project->admin` intersection — is admissible.
///
/// `channel.read = member + parent_project->read` is encoded as `Union(member, parent_project->view)`;
/// the core `project` exposes `view` (reader+writer+parent_team->view), so `parent_project->read` maps
/// onto the core `project.view` where the inheritance terminates (the §5 `read` is the core `view`,
/// the same terminate-at-core convention the Issues 4.9 CDC uses for `parent_project->write`).
/// `channel.manage = member & parent_project->admin` maps `parent_project->admin` onto the same core
/// `project.view` (the core hierarchy carries no separate `admin` permission at this layer — the LIVE
/// admin distinction is the Chat M4 spine's, off this freeze anchor; here the intersection STRUCTURE
/// is what we prove admissible).
fn chat_fragment_defs_rich() -> Vec<FragmentDef> {
    let rel = |n: &str| Userset::Relation(RelName(n.into()));
    let ttu = |tupleset: &str, computed: &str| Userset::TupleToUserset {
        tupleset: RelName(tupleset.into()),
        computed: RelName(computed.into()),
    };
    vec![
        // channel: read (the inheritance crux) / post / manage (the intersection).
        FragmentDef {
            object_type: ObjectType(object_types::CHANNEL.into()),
            relations: vec![
                RelName("parent_project".into()),
                RelName("member".into()),
                RelName("watcher".into()),
            ],
            permissions: vec![
                // read = member + parent_project->read (the frozen Chat clause, recon §1).
                PermissionRule {
                    permission: Permission("read".into()),
                    rewrite: Userset::Union(vec![rel("member"), ttu("parent_project", "view")]),
                },
                // post = member.
                PermissionRule {
                    permission: Permission("post".into()),
                    rewrite: rel("member"),
                },
                // manage = member & parent_project->admin (core `view` is where admin terminates).
                PermissionRule {
                    permission: Permission("manage".into()),
                    rewrite: Userset::Intersect(vec![rel("member"), ttu("parent_project", "view")]),
                },
            ],
        },
        // message: view = parent_channel->read (a message inherits its channel's read).
        FragmentDef {
            object_type: ObjectType(object_types::MESSAGE.into()),
            relations: vec![RelName("parent_channel".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: ttu("parent_channel", "read"),
            }],
        },
    ]
}

/// **CONSUMER → PROVIDER: the Chat fragment (names-only ABI carriers) ADMITS into the cell schema.**
/// This is the build-time gate of CHAT-P2 reified as a runtime assertion: Identity admits every Chat
/// object type the consumer declares — the cell schema compiles against the Chat fragment.
#[test]
fn cdc_4_9_chat_names_only_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);

    // The CONSUMER declares its fragment at build time (the frozen names-only carriers).
    let consumer_fragment: Vec<NamespaceFragment> = rebac_fragment::chat_fragment();
    assert_eq!(consumer_fragment.len(), 2, "channel + message");
    let types: Vec<&str> = consumer_fragment
        .iter()
        .map(|f| f.object_type.0.as_str())
        .collect();
    assert_eq!(types, vec!["channel", "message"]);

    // The PROVIDER admits each (the rich form carrying the rewrites — the shape Identity compiles).
    for def in chat_fragment_defs_rich() {
        let admit = svc.admit_fragment_def(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Chat `{}` fragment must admit into the cell schema: {admit:?}",
            def.object_type.0
        );
    }
}

/// **PROVIDER: `channel.read = member + parent_project->read` resolves correctly (the frozen Chat
/// clause, recon §1 / §5).** The two arms:
/// - a `member` of a private channel CAN read (the `member` arm) — membership IS the ACL.
/// - a project READER of the channel's parent project CAN read a PUBLIC channel (the
///   `+ parent_project->read` inheritance) even WITHOUT a membership tuple.
/// - a non-member, non-project-reader CANNOT read (fail-closed — no leak).
#[test]
fn cdc_4_9_channel_read_member_plus_parent_project_resolves() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // alice is a direct member of the private channel:secret.
            add("channel:secret", "member", "p:alice"),
            // channel:general is a PUBLIC channel parented to project:web; bob reads the project.
            add("channel:general", "parent_project", "project:web#view"),
            add("project:web", "reader", "p:bob"),
        ],
    );
    for def in chat_fragment_defs_rich() {
        assert!(
            matches!(svc.admit_fragment_def(&def), FragmentAdmit::Admitted { .. }),
            "Chat `{}` admits",
            def.object_type.0
        );
    }

    let can_read = |channel: &str, actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission("read".into()),
                &ArtifactRef(channel.into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };

    // the `member` arm: alice (a direct member) reads the private channel.
    assert!(
        can_read("channel:secret", &subject("p:alice")),
        "a member reads the private channel (the `member` arm of channel.read)"
    );
    // the `+ parent_project->read` arm: bob (a project reader, NO membership) reads the public channel.
    assert!(
        can_read("channel:general", &subject("p:bob")),
        "a project reader inherits read on a public channel (the + parent_project->read arm)"
    );
    // fail-closed: a stranger reads NEITHER channel.
    assert!(
        !can_read("channel:secret", &subject("p:carol")),
        "a non-member, non-project-reader cannot read the private channel (no leak)"
    );
    assert!(
        !can_read("channel:general", &subject("p:carol")),
        "a non-member, non-project-reader cannot read the public channel (no leak)"
    );
}

/// **PROVIDER: `message.view = parent_channel->read` resolves (a message inherits its channel's read,
/// §5).** A reader of the parent channel CAN view the message; a stranger CANNOT — the message's
/// visibility is exactly its channel's, no bespoke message ACL.
#[test]
fn cdc_4_9_message_view_inherits_parent_channel_read() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // alice is a member of channel:secret.
            add("channel:secret", "member", "p:alice"),
            // message:m1 is parented to channel:secret (inherits its read).
            add("message:m1", "parent_channel", "channel:secret#read"),
        ],
    );
    for def in chat_fragment_defs_rich() {
        assert!(matches!(
            svc.admit_fragment_def(&def),
            FragmentAdmit::Admitted { .. }
        ));
    }

    let can_view = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission("view".into()),
                &ArtifactRef("message:m1".into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };

    assert!(
        can_view(&subject("p:alice")),
        "a member of the parent channel can view the message (message.view = parent_channel->read)"
    );
    assert!(
        !can_view(&subject("p:carol")),
        "a non-member cannot view the message (it inherits the channel's read — no leak)"
    );
}

/// **The `watcher` relation is admitted but is NOT a read grant (it is the Notif read-fanout
/// declaration, §5 / contract 4.9).** A `watcher` tuple does NOT confer `read` — read is `member +
/// parent_project->read`. Notif resolves `list_subjects(channel, watcher)` for the unbounded ambient
/// set; the watcher relation exists for that fanout, not for authz. This pins that `watcher` is a
/// pure read-fanout relation, never an accidental access arm.
#[test]
fn cdc_4_9_watcher_is_read_fanout_not_a_read_grant() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // dave is a WATCHER of channel:secret but NOT a member and NOT a project reader.
            add("channel:secret", "watcher", "p:dave"),
        ],
    );
    for def in chat_fragment_defs_rich() {
        assert!(matches!(
            svc.admit_fragment_def(&def),
            FragmentAdmit::Admitted { .. }
        ));
    }

    // a watcher that is not a member / project-reader does NOT get read (watcher ≠ access).
    assert!(
        !matches!(
            svc.check(
                &subject("p:dave"),
                &Permission("read".into()),
                &ArtifactRef("channel:secret".into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        ),
        "a `watcher` is the Notif read-fanout relation, NOT a read grant (read = member + \
         parent_project->read; §5 / contract 4.9)"
    );
}
