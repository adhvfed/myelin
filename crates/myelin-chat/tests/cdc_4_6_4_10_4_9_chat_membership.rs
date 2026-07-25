//! # The CDC pair for contracts 4.6 / 4.10 / 4.9 — the **Chat** membership→`write_tuples`→zookie +
//! the new-enemy guard + the channel.read fragment resolution (CHAT-P8 / P-402)
//!
//! **Contract-index rows:**
//! - **4.6** `write_tuples([Δtuple], precondition?) → zookie` (atomic; emitted via the outbox) —
//!   CONSUMED: the membership tuple write returns the zookie chat stamps.
//! - **4.10** the zookie new-enemy stamp (read-your-writes; a strong, zookie-stamped read denies a
//!   just-revoked grant) — CONSUMED: a `Remove` ADVANCES the revision/zookie, and a strong read at
//!   the new watermark resolves against the post-revoke set.
//! - **4.9** the Chat ReBAC fragment (`channel.read = member + parent_project->read`) — OWNED: the
//!   runtime membership writes resolve against the declared fragment.
//!
//! The two sides are pinned here so a drift on either fails this test in the same CI job:
//! - the **CONSUMER** is the Chat membership service ([`myelin_chat::membership::MembershipService`])
//!   driving the membership→write_tuples→zookie→stamp→event flow over the
//!   [`MembershipTupleWriter`] port — and the [`MembershipGate`] gating reads through `Id.check`.
//! - the **PROVIDER** is Identity's REAL engine: `TupleStore::write_tuples` (the 4.6 write path that
//!   advances the zookie + co-commits `identity.tuple.written`) + `StoreBackedCheck` (the 4.2 `check`
//!   resolving the admitted Chat fragment's `channel.read = member + parent_project->read` rewrite).
//!
//! This proves the freeze CHAT-P8 ships is admissible AND resolves correctly against the LIVE engine
//! today: a member reads (the `member` arm), a parent-project reader reads a public channel (the
//! `+ parent_project->read` arm), a non-member is denied (fail-closed), and — the new-enemy guard —
//! a `Remove` advances the zookie so a strong read at the new watermark denies the revoked member.

use std::sync::{Arc, Mutex};

use myelin_chat::conversation::{
    Conversation, ConversationKind, ConversationStore, MemConversationStore, Membership,
    MembershipRole,
};
use myelin_chat::membership::{
    permissions, MembershipGate, MembershipService, MembershipTupleWriter,
};
use myelin_chat::store::ConversationId;
use myelin_events::{Actor, EmitContextBase, OutboxStore, OutboxTransaction, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, ObjectType, Permission,
    Precondition, Principal, PrincipalId, PrincipalKind, RelName, TupleDelta, Zookie,
};
use myelin_identity_service::{FragmentDef, PermissionRule, StoreBackedCheck, TupleStore, Userset};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

const TENANT: &str = "acme";
const REGION: &str = "fr-par";

fn scope() -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(TENANT.into()),
    );
    TenantScope::from_verified_token(&p, Region(REGION.into()))
}

fn subject(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(TENANT.into()),
    );
    p.region = Region(REGION.into());
    p
}

/// The Chat fragment's frozen permission rewrites (§5) as the rich engine `FragmentDef` form (the
/// shape the Chat M4 spine wires LIVE; here the CDC compiles it into the engine so `check` resolves).
/// Identical to the `cdc_4_9` rich form — one fragment, no second spelling.
fn chat_fragment_defs_rich() -> Vec<FragmentDef> {
    let rel = |n: &str| Userset::Relation(RelName(n.into()));
    let ttu = |tupleset: &str, computed: &str| Userset::TupleToUserset {
        tupleset: RelName(tupleset.into()),
        computed: RelName(computed.into()),
    };
    vec![
        FragmentDef {
            object_type: ObjectType("channel".into()),
            relations: vec![
                RelName("parent_project".into()),
                RelName("member".into()),
                RelName("watcher".into()),
            ],
            permissions: vec![
                PermissionRule {
                    permission: Permission("read".into()),
                    rewrite: Userset::Union(vec![rel("member"), ttu("parent_project", "view")]),
                },
                PermissionRule {
                    permission: Permission("post".into()),
                    rewrite: rel("member"),
                },
                PermissionRule {
                    permission: Permission("manage".into()),
                    rewrite: Userset::Intersect(vec![rel("member"), ttu("parent_project", "view")]),
                },
            ],
        },
        FragmentDef {
            object_type: ObjectType("message".into()),
            relations: vec![RelName("parent_channel".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: ttu("parent_channel", "read"),
            }],
        },
    ]
}

/// **The production-shaped `MembershipTupleWriter` binding: it adapts Identity's REAL
/// `TupleStore::write_tuples` (the scope-carrying 4.6 write path) to the port.** This is the thin
/// adapter the Chat service wires in production — the membership service calls the port, the port
/// calls the live engine's write (advancing the zookie + co-committing `identity.tuple.written`). No
/// second write language; the port carries the frozen `[TupleDelta]` + `Precondition` → `Zookie`.
struct LiveTupleWriter {
    store: TupleStore,
    scope: TenantScope,
    actor: Principal,
    /// A monotone clock for the `occurred_at` of each write (the floor's wall-clock stand-in).
    seq: Mutex<u64>,
}

impl LiveTupleWriter {
    fn new(store: TupleStore) -> LiveTupleWriter {
        LiveTupleWriter {
            store,
            scope: scope(),
            actor: subject("p-admin"),
            seq: Mutex::new(0),
        }
    }
}

impl MembershipTupleWriter for LiveTupleWriter {
    fn write_membership_tuples(
        &self,
        deltas: &[TupleDelta],
        precondition: Option<&Precondition>,
    ) -> core::result::Result<Zookie, String> {
        let n = {
            let mut s = self.seq.lock().unwrap_or_else(|e| e.into_inner());
            *s += 1;
            *s
        };
        self.store
            .write_tuples(
                &self.scope,
                &self.actor,
                deltas,
                precondition,
                None,
                Timestamp(format!("2026-06-24T00:00:{n:02}Z")),
            )
            .map_err(|e| format!("{e:?}"))
    }
}

/// Build the provider: the REAL `StoreBackedCheck` over a `TupleStore` with the core org/team/project
/// hierarchy + the admitted Chat fragment, sharing the SAME `TupleStore` the membership writer writes
/// to (so the gate's `check` reads the membership writer's tuples). Returns `(check_engine, writer)`.
fn provider() -> (StoreBackedCheck, LiveTupleWriter) {
    let store = TupleStore::new(OutboxStore::new());
    let svc = StoreBackedCheck::new(store.clone());
    // Admit the Chat fragment so `check` resolves `channel.read = member + parent_project->read`.
    for def in chat_fragment_defs_rich() {
        assert!(
            matches!(svc.admit_fragment_def(&def), FragmentAdmit::Admitted { .. }),
            "the Chat `{}` fragment must admit",
            def.object_type.0
        );
    }
    let writer = LiveTupleWriter::new(store);
    (svc, writer)
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId(TENANT.into()),
        region: Region(REGION.into()),
        actor: Actor(subject("p-admin")),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        caused_by: None,
    }
}

fn tx(store: &OutboxStore, minter: &Arc<myelin_events::MonotonicMinter>) -> OutboxTransaction {
    store.begin(minter.clone(), ctx_base())
}

fn conv_id(id: &str) -> ConversationId {
    ConversationId::new(TENANT, REGION, id)
}

fn channel(store: &MemConversationStore, id: &str) -> ConversationId {
    let cid = conv_id(id);
    store
        .create(Conversation {
            home_cell: Conversation::home_cell_for(&cid),
            id: cid.clone(),
            kind: ConversationKind::ChannelPrivate,
            parent_project: Some("proj-web".into()),
            name: Some(id.into()),
            topic: None,
            linked_ref: None,
            pinned_canvas: None,
            retention_days: None,
            archived: false,
            created_by: "psn:creator".into(),
            acl_zookie: None,
        })
        .unwrap();
    cid
}

/// **CONSUMER → PROVIDER (4.6 / 4.9): a membership add writes the `member` tuple through the REAL
/// `write_tuples`, returns a zookie chat stamps, and the channel.read fragment resolves the member
/// arm.** Proves the runtime membership write resolves `channel.read = member + parent_project->read`
/// against the LIVE engine — a member reads, a non-member is denied (fail-closed, no leak).
#[test]
fn cdc_4_6_4_9_membership_write_resolves_channel_read_member_arm() {
    let (svc, writer) = provider();
    let conv_store = MemConversationStore::new();
    let cid = channel(&conv_store, "secret");
    let membership = MembershipService::new(writer);
    let ob = OutboxStore::new();
    let minter = Arc::new(myelin_events::MonotonicMinter::new());

    // add alice → the REAL write_tuples advances the zookie; chat stamps it on the conversation.
    let mut t = tx(&ob, &minter);
    let zookie = membership
        .add_member(
            &mut t,
            &conv_store,
            Membership::member(cid.clone(), "p:alice"),
        )
        .expect("add_member co-commits over the live write_tuples");
    t.commit().unwrap();
    // the conversation carries the LIVE zookie (non-empty — a real revision watermark).
    let stamped = conv_store.get(&cid).unwrap().acl_zookie.clone();
    assert_eq!(stamped.as_deref(), Some(zookie.0.as_str()));
    assert!(
        !zookie.0.is_empty(),
        "the live engine returns a real zookie"
    );

    // the gate resolves channel.read through the admitted fragment over the membership tuple.
    let gate = MembershipGate::new(svc.clone());
    assert!(
        gate.check_channel(
            &subject("p:alice"),
            permissions::READ,
            &cid,
            stamped.as_deref()
        )
        .is_ok(),
        "a member reads (channel.read = member arm) over the live engine"
    );
    assert!(
        gate.check_channel(
            &subject("p:carol"),
            permissions::READ,
            &cid,
            stamped.as_deref()
        )
        .is_err(),
        "a non-member is denied (fail-closed, no leak)"
    );
}

/// **PROVIDER (4.9): the `+ parent_project->read` arm resolves over the live engine.** A reader of
/// the channel's parent project reads a PUBLIC channel WITHOUT a membership tuple — the frozen
/// inheritance arm. We seed the parent-project tuples directly through the live write path.
#[test]
fn cdc_4_9_channel_read_parent_project_arm_resolves_live() {
    let (svc, writer) = provider();
    let conv_store = MemConversationStore::new();
    // a public channel parented to project:proj-web; bob reads the project.
    let cid = {
        let cid = conv_id("general");
        conv_store
            .create(Conversation {
                home_cell: Conversation::home_cell_for(&cid),
                id: cid.clone(),
                kind: ConversationKind::ChannelPublic,
                parent_project: Some("proj-web".into()),
                name: Some("general".into()),
                topic: None,
                linked_ref: None,
                pinned_canvas: None,
                retention_days: None,
                archived: false,
                created_by: "psn:creator".into(),
                acl_zookie: None,
            })
            .unwrap();
        cid
    };
    // seed the parent-project inheritance tuples directly through the live write path (the membership
    // add path writes the `member` arm; here we exercise the inheritance arm). The returned zookie is
    // the real revision watermark a strong read reads at-or-after.
    let z_seed = writer
        .write_membership_tuples(
            &[
                TupleDelta::Add(myelin_identity::RelationTuple {
                    object: myelin_identity::ObjectId("channel:general".into()),
                    relation: RelName("parent_project".into()),
                    subject: PrincipalId("project:web#view".into()),
                    caveat: None,
                }),
                TupleDelta::Add(myelin_identity::RelationTuple {
                    object: myelin_identity::ObjectId("project:web".into()),
                    relation: RelName("reader".into()),
                    subject: PrincipalId("p:bob".into()),
                    caveat: None,
                }),
            ],
            None,
        )
        .unwrap();
    conv_store.stamp_acl_zookie(&cid, &z_seed.0).unwrap();
    let z = conv_store.get(&cid).unwrap().acl_zookie;

    let gate = MembershipGate::new(svc.clone());
    // bob (a project reader, NO membership) reads the public channel (the + parent_project->read arm).
    assert!(
        gate.check_channel(&subject("p:bob"), permissions::READ, &cid, z.as_deref())
            .is_ok(),
        "a project reader inherits read on a public channel (the + parent_project->read arm)"
    );
    // carol (neither) cannot.
    assert!(gate
        .check_channel(&subject("p:carol"), permissions::READ, &cid, z.as_deref())
        .is_err());
}

/// **CONSUMER → PROVIDER (4.10): THE NEW-ENEMY GUARD over the live engine.** add → revoke → read:
/// the `Remove` advances the live zookie; a strong read at the new watermark resolves against the
/// post-revoke set → the revoked member is DENIED. The two zookies are distinct (a real revision
/// advance), proving the watermark moved — 0 stale grants readable post-revoke.
#[test]
fn cdc_4_10_new_enemy_guard_revoke_advances_zookie_live() {
    let (svc, writer) = provider();
    let conv_store = MemConversationStore::new();
    let cid = channel(&conv_store, "secret");
    let membership = MembershipService::new(writer);
    let gate = MembershipGate::new(svc.clone());
    let ob = OutboxStore::new();
    let minter = Arc::new(myelin_events::MonotonicMinter::new());

    // ── add alice → she reads ──
    let mut t1 = tx(&ob, &minter);
    let z_add = membership
        .add_member(
            &mut t1,
            &conv_store,
            Membership::member(cid.clone(), "p:alice"),
        )
        .unwrap();
    t1.commit().unwrap();
    let stamped_add = conv_store.get(&cid).unwrap().acl_zookie.clone();
    assert!(gate
        .check_channel(
            &subject("p:alice"),
            permissions::READ,
            &cid,
            stamped_add.as_deref()
        )
        .is_ok());

    // ── revoke alice → the live zookie ADVANCES + restamps ──
    let mut t2 = tx(&ob, &minter);
    let z_revoke = membership
        .remove_member(
            &mut t2,
            &conv_store,
            &cid,
            "p:alice",
            MembershipRole::Member,
            true,
        )
        .unwrap();
    t2.commit().unwrap();
    let stamped_revoke = conv_store.get(&cid).unwrap().acl_zookie.clone();
    assert_ne!(
        z_add.0, z_revoke.0,
        "the revoke advanced the zookie (a real revision move — the new-enemy watermark)"
    );
    assert_eq!(stamped_revoke.as_deref(), Some(z_revoke.0.as_str()));

    // ── the new-enemy read: alice reads at the post-revoke watermark → DENIED (0 stale grants) ──
    let conv = conv_store.get(&cid).unwrap();
    let strong = MembershipService::<LiveTupleWriter>::read_consistency(&conv);
    assert_eq!(strong.mode, ConsistencyMode::Strong);
    // resolve through the live engine at the post-revoke watermark. The check object is the Id-side
    // `channel:<id>` ObjectId form the membership tuples key on (the same spelling the gate uses).
    let object = myelin_tenancy::ArtifactRef(myelin_chat::membership::channel_object("secret"));
    let decision = svc.check(
        &subject("p:alice"),
        &Permission("read".into()),
        &object,
        &Consistency {
            at_least: Zookie(z_revoke.0.clone()),
            mode: ConsistencyMode::Strong,
        },
        None,
    );
    assert!(
        !matches!(decision, Ok(Decision::Allow)),
        "the revoked member reads against the post-revoke set (the new-enemy guard) — 0 stale grants"
    );
    // and through the gate.
    assert!(gate
        .check_channel(
            &subject("p:alice"),
            permissions::READ,
            &cid,
            stamped_revoke.as_deref()
        )
        .is_err());
}
