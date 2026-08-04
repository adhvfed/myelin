use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use myelin_events::{Actor, CausedBy, EmitContextBase, OutboxStore, OutboxTransaction, Timestamp};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, ConsistencyMode, Credential, Decision,
    DelegationCaveats, EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService,
    ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission, Precondition,
    Principal, PrincipalId, PrincipalKind, RevokeTarget, RewriteTrace, RunId, RunToken,
    SubjectTree, TupleDelta, Zookie,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use super::*;
use crate::conversation::{
    Conversation, ConversationKind, ConversationStore, MemConversationStore, Membership,
    MembershipRole,
};

type IdResult<T> = myelin_identity::Result<T>;

#[derive(Default)]
struct RebacState {
    tuples: BTreeSet<(String, String, String)>,
    revision: u64,
    rev_seen: BTreeSet<(String, String, String, u64)>,
}

#[derive(Clone, Default)]
struct SharedRebac(Arc<Mutex<RebacState>>);

impl SharedRebac {
    fn lock(&self) -> std::sync::MutexGuard<'_, RebacState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn holds(&self, object: &str, relation: &str, subject: &str) -> bool {
        self.lock().tuples.contains(&(
            object.to_string(),
            relation.to_string(),
            subject.to_string(),
        ))
    }
}

impl MembershipTupleWriter for SharedRebac {
    fn write_membership_tuples(
        &self,
        deltas: &[TupleDelta],
        _precondition: Option<&Precondition>,
    ) -> core::result::Result<Zookie, String> {
        let mut st = self.lock();
        st.revision += 1;
        let rev = st.revision;
        for d in deltas {
            match d {
                TupleDelta::Add(t) => {
                    let key = (
                        t.object.0.clone(),
                        t.relation.0.clone(),
                        t.subject.0.clone(),
                    );
                    st.tuples.insert(key.clone());
                    st.rev_seen.insert((key.0, key.1, key.2, rev));
                }
                TupleDelta::Remove(t) => {
                    let key = (
                        t.object.0.clone(),
                        t.relation.0.clone(),
                        t.subject.0.clone(),
                    );
                    st.tuples.remove(&key);
                    st.rev_seen.insert((key.0, key.1, key.2, rev));
                }
            }
        }
        Ok(Zookie(format!("z{rev}")))
    }
}

struct FakeId {
    rebac: SharedRebac,
    project_readers: BTreeSet<String>,
}

impl FakeId {
    fn new(rebac: SharedRebac) -> FakeId {
        FakeId {
            rebac,
            project_readers: BTreeSet::new(),
        }
    }
    fn with_project_reader(mut self, who: &str) -> FakeId {
        self.project_readers.insert(who.to_string());
        self
    }
    fn channel_object(reference: &ArtifactRef) -> String {
        reference.0.clone()
    }
}

impl IdentityService for FakeId {
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let obj = Self::channel_object(object);
        let who = subject.principal_id.0.as_str();
        let is_member = self.rebac.holds(&obj, "member", who);
        let allow = match permission.0.as_str() {
            "read" => is_member || self.project_readers.contains(who),
            "post" => is_member,
            "manage" => is_member,
            _ => false,
        };
        Ok(if allow {
            Decision::Allow
        } else {
            Decision::Deny
        })
    }

    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _a: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _a: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _a: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn outbox() -> (OutboxStore, Arc<myelin_events::MonotonicMinter>) {
    (
        OutboxStore::new(),
        Arc::new(myelin_events::MonotonicMinter::new()),
    )
}

fn tx(store: &OutboxStore, minter: &Arc<myelin_events::MonotonicMinter>) -> OutboxTransaction {
    store.begin(minter.clone(), ctx_base())
}

fn conv_id(id: &str) -> ConversationId {
    ConversationId::new("acme", "fr-par", id)
}

fn channel(store: &MemConversationStore, id: &str) -> ConversationId {
    let cid = conv_id(id);
    store
        .create(Conversation {
            home_cell: Conversation::home_cell_for(&cid),
            id: cid.clone(),
            kind: ConversationKind::ChannelPrivate,
            parent_project: Some("proj-1".into()),
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

fn subject(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

#[test]
fn membership_add_co_commits_row_zookie_and_event_in_one_tx() {
    let store = MemConversationStore::new();
    let cid = channel(&store, "c1");
    let rebac = SharedRebac::default();
    let svc = MembershipService::new(rebac.clone());
    let (ob, minter) = outbox();
    let mut t = tx(&ob, &minter);

    let zookie = svc
        .add_member(&mut t, &store, Membership::member(cid.clone(), "alice"))
        .expect("add_member co-commits");

    assert_eq!(zookie.0, "z1");
    assert_eq!(store.get(&cid).unwrap().acl_zookie.as_deref(), Some("z1"));
    assert_eq!(
        store.conversations_of("acme", "alice").unwrap(),
        vec![cid.clone()]
    );
    assert!(rebac.holds("channel:c1", "member", "alice"));
    assert!(rebac.holds("channel:c1", "watcher", "alice"));
    assert_eq!(ob.outbox_depth(), 0, "nothing durable before commit");
    assert_eq!(t.staged_len(), 1, "exactly one member_added event staged");

    t.commit().expect("commit");
    assert_eq!(
        ob.outbox_depth(),
        1,
        "the member_added event is now durable"
    );
    let rows = ob.committed_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].envelope.type_.0, "chat.channel.member_added");
    assert_eq!(rows[0].aggregate.0, "c1", "aggregate = conversation_id");
}

#[test]
fn membership_add_tuple_write_failure_leaves_zero_partial_state() {
    struct FailingWriter;
    impl MembershipTupleWriter for FailingWriter {
        fn write_membership_tuples(
            &self,
            _d: &[TupleDelta],
            _p: Option<&Precondition>,
        ) -> core::result::Result<Zookie, String> {
            Err("S3 write rejected".into())
        }
    }
    let store = MemConversationStore::new();
    let cid = channel(&store, "c1");
    let svc = MembershipService::new(FailingWriter);
    let (ob, minter) = outbox();
    let mut t = tx(&ob, &minter);

    let err = svc
        .add_member(&mut t, &store, Membership::member(cid.clone(), "alice"))
        .expect_err("the tuple write fails → the whole change aborts");
    assert!(matches!(err, MembershipError::TupleWrite(_)));

    assert_eq!(store.get(&cid).unwrap().acl_zookie, None);
    assert!(store.conversations_of("acme", "alice").unwrap().is_empty());
    assert_eq!(t.staged_len(), 0, "no event staged on the aborted change");
}

#[test]
fn new_enemy_guard_revoked_member_cannot_read_post_revoke() {
    let store = MemConversationStore::new();
    let cid = channel(&store, "c1");
    let rebac = SharedRebac::default();
    let svc = MembershipService::new(rebac.clone());
    let gate = MembershipGate::new(FakeId::new(rebac.clone()));
    let (ob, minter) = outbox();

    let mut t1 = tx(&ob, &minter);
    svc.add_member(&mut t1, &store, Membership::member(cid.clone(), "alice"))
        .unwrap();
    t1.commit().unwrap();
    let conv_after_add = store.get(&cid).unwrap();
    let z_add = conv_after_add.acl_zookie.clone();
    assert!(
        gate.check_channel(&subject("alice"), permissions::READ, &cid, z_add.as_deref())
            .is_ok(),
        "a member reads (the `member` arm of channel.read)"
    );

    let mut t2 = tx(&ob, &minter);
    let z_revoke = svc
        .remove_member(&mut t2, &store, &cid, "alice", MembershipRole::Member, true)
        .unwrap();
    t2.commit().unwrap();
    assert_eq!(z_revoke.0, "z2");
    let conv_after_revoke = store.get(&cid).unwrap();
    assert_eq!(conv_after_revoke.acl_zookie.as_deref(), Some("z2"));

    let strong = MembershipService::<SharedRebac>::read_consistency(&conv_after_revoke);
    assert_eq!(strong.mode, ConsistencyMode::Strong, "the read is strong");
    assert_eq!(
        strong.at_least.0, "z2",
        "at-or-after the post-revoke zookie"
    );
    let denied = gate.check_channel(
        &subject("alice"),
        permissions::READ,
        &cid,
        conv_after_revoke.acl_zookie.as_deref(),
    );
    assert!(
        matches!(denied, Err(MembershipError::Denied { .. })),
        "0 stale grants: the revoked member reads against the post-revoke set (new-enemy guard)"
    );
    assert!(!rebac.holds("channel:c1", "member", "alice"));
}

#[test]
fn channel_read_resolves_member_plus_parent_project() {
    let store = MemConversationStore::new();
    let cid = channel(&store, "c1");
    let rebac = SharedRebac::default();
    let svc = MembershipService::new(rebac.clone());
    let gate = MembershipGate::new(FakeId::new(rebac.clone()).with_project_reader("bob"));
    let (ob, minter) = outbox();

    let mut t = tx(&ob, &minter);
    svc.add_member(&mut t, &store, Membership::member(cid.clone(), "alice"))
        .unwrap();
    t.commit().unwrap();
    let z = store.get(&cid).unwrap().acl_zookie;

    assert!(gate
        .check_channel(&subject("alice"), permissions::READ, &cid, z.as_deref())
        .is_ok());
    assert!(gate
        .check_channel(&subject("bob"), permissions::READ, &cid, z.as_deref())
        .is_ok());
    assert!(matches!(
        gate.check_channel(&subject("carol"), permissions::READ, &cid, z.as_deref()),
        Err(MembershipError::Denied { .. })
    ));
}

#[test]
fn send_gate_is_fail_closed() {
    let store = MemConversationStore::new();
    let cid = channel(&store, "c1");
    let rebac = SharedRebac::default();
    let svc = MembershipService::new(rebac.clone());
    let gate = MembershipGate::new(FakeId::new(rebac.clone()));
    let (ob, minter) = outbox();

    assert!(matches!(
        gate.check_send(&subject("alice"), &cid, None),
        Err(MembershipError::Denied { .. })
    ));

    let mut t = tx(&ob, &minter);
    svc.add_member(&mut t, &store, Membership::member(cid.clone(), "alice"))
        .unwrap();
    t.commit().unwrap();
    let z = store.get(&cid).unwrap().acl_zookie;
    assert!(gate
        .check_send(&subject("alice"), &cid, z.as_deref())
        .is_ok());
    assert!(matches!(
        gate.check_send(&subject("bob"), &cid, z.as_deref()),
        Err(MembershipError::Denied { .. })
    ));
}

#[test]
fn channel_lifecycle_events_co_commit() {
    let store = MemConversationStore::new();
    let rebac = SharedRebac::default();
    let svc = MembershipService::new(rebac);
    let (ob, minter) = outbox();

    let cid = conv_id("c-linked");
    let conv = Conversation {
        home_cell: Conversation::home_cell_for(&cid),
        id: cid.clone(),
        kind: ConversationKind::ArtifactLinked,
        parent_project: Some("proj-1".into()),
        name: Some("incident".into()),
        topic: None,
        linked_ref: Some("myelin://acme/issues/issue/ABC-1".into()),
        pinned_canvas: None,
        retention_days: None,
        archived: false,
        created_by: "psn:creator".into(),
        acl_zookie: None,
    };
    let mut t = tx(&ob, &minter);
    svc.create_channel(&mut t, &store, conv).unwrap();
    svc.archive_channel(&mut t, &cid).unwrap();
    t.commit().unwrap();

    let types: Vec<String> = ob
        .committed_rows()
        .iter()
        .map(|r| r.envelope.type_.0.clone())
        .collect();
    assert_eq!(
        types,
        vec![
            "chat.channel.created".to_string(),
            "chat.channel.linked".to_string(),
            "chat.channel.archived".to_string(),
        ],
        "created → linked (discussed-in) → archived, all on one co-commit, aggregate-ordered"
    );
    let linked = ob
        .committed_rows()
        .into_iter()
        .find(|r| r.envelope.type_.0 == "chat.channel.linked")
        .unwrap();
    assert_eq!(
        linked.envelope.payload["linked_ref"],
        serde_json::json!("myelin://acme/issues/issue/ABC-1")
    );
}

#[test]
fn membership_change_on_missing_conversation_is_loud() {
    let store = MemConversationStore::new();
    let rebac = SharedRebac::default();
    let svc = MembershipService::new(rebac);
    let (ob, minter) = outbox();
    let mut t = tx(&ob, &minter);
    let err = svc
        .add_member(
            &mut t,
            &store,
            Membership::member(conv_id("ghost"), "alice"),
        )
        .expect_err("a phantom conversation is LOUD");
    assert!(matches!(err, MembershipError::NotFound(_)));
}
