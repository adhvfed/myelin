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

struct LiveTupleWriter {
    store: TupleStore,
    scope: TenantScope,
    actor: Principal,
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

fn provider() -> (StoreBackedCheck, LiveTupleWriter) {
    let store = TupleStore::new(OutboxStore::new());
    let svc = StoreBackedCheck::new(store.clone());
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

#[test]
fn cdc_4_6_4_9_membership_write_resolves_channel_read_member_arm() {
    let (svc, writer) = provider();
    let conv_store = MemConversationStore::new();
    let cid = channel(&conv_store, "secret");
    let membership = MembershipService::new(writer);
    let ob = OutboxStore::new();
    let minter = Arc::new(myelin_events::MonotonicMinter::new());

    let mut t = tx(&ob, &minter);
    let zookie = membership
        .add_member(
            &mut t,
            &conv_store,
            Membership::member(cid.clone(), "p:alice"),
        )
        .expect("add_member co-commits over the live write_tuples");
    t.commit().unwrap();
    let stamped = conv_store.get(&cid).unwrap().acl_zookie.clone();
    assert_eq!(stamped.as_deref(), Some(zookie.0.as_str()));
    assert!(
        !zookie.0.is_empty(),
        "the live engine returns a real zookie"
    );

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

#[test]
fn cdc_4_9_channel_read_parent_project_arm_resolves_live() {
    let (svc, writer) = provider();
    let conv_store = MemConversationStore::new();
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
    assert!(
        gate.check_channel(&subject("p:bob"), permissions::READ, &cid, z.as_deref())
            .is_ok(),
        "a project reader inherits read on a public channel (the + parent_project->read arm)"
    );
    assert!(gate
        .check_channel(&subject("p:carol"), permissions::READ, &cid, z.as_deref())
        .is_err());
}

#[test]
fn cdc_4_10_new_enemy_guard_revoke_advances_zookie_live() {
    let (svc, writer) = provider();
    let conv_store = MemConversationStore::new();
    let cid = channel(&conv_store, "secret");
    let membership = MembershipService::new(writer);
    let gate = MembershipGate::new(svc.clone());
    let ob = OutboxStore::new();
    let minter = Arc::new(myelin_events::MonotonicMinter::new());

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
        "the revoke advanced the zookie (a real revision move - the new-enemy watermark)"
    );
    assert_eq!(stamped_revoke.as_deref(), Some(z_revoke.0.as_str()));

    let conv = conv_store.get(&cid).unwrap();
    let strong = MembershipService::<LiveTupleWriter>::read_consistency(&conv);
    assert_eq!(strong.mode, ConsistencyMode::Strong);
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
        "the revoked member reads against the post-revoke set (the new-enemy guard) - 0 stale grants"
    );
    assert!(gate
        .check_channel(
            &subject("p:alice"),
            permissions::READ,
            &cid,
            stamped_revoke.as_deref()
        )
        .is_err());
}
