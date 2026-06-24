//! Unit tests for the CHAT-P8 membership co-commit + the new-enemy guard + the send/membership gate
//! (the DB-free, behaviour-identical floor). The REAL engine proves the SAME flow against Identity's
//! live `TupleStore` + `StoreBackedCheck` in `tests/cdc_4_6_4_10_4_9_chat_membership.rs` (the CDC's
//! engine leg — the channel.read fragment resolution + the zookie advance).

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

// ---------------------------------------------------------------------------------------------
// The shared fake ReBAC state: the tuple writer + the gate read the SAME state so the new-enemy
// guard is exercised end-to-end (a remove advances the revision; a strong read at-or-after that
// revision sees the post-revoke set). This is test scaffolding modelling the engine's revision
// semantics — NOT a second permission engine (EI-01 §7).
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct RebacState {
    /// The current tuple set: `(object, relation, subject)`.
    tuples: BTreeSet<(String, String, String)>,
    /// The monotonic revision — bumped on every write; the zookie is `"z<rev>"`.
    revision: u64,
    /// The revision at which the LAST mutation to each `(object, relation, subject)` happened, so a
    /// strong read at zookie `z<r>` sees a tuple iff its last add was at-or-before `r` AND it was
    /// not removed after `r`. The fake keeps the simple invariant: the CURRENT `tuples` set is the
    /// state at the CURRENT revision, and a strong read at the latest zookie sees exactly it
    /// (read-your-writes; the chained test always reads at the just-stamped zookie).
    rev_seen: BTreeSet<(String, String, String, u64)>,
}

#[derive(Clone, Default)]
struct SharedRebac(Arc<Mutex<RebacState>>);

impl SharedRebac {
    fn lock(&self) -> std::sync::MutexGuard<'_, RebacState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether `subject` currently holds `relation` on `object` (the current revision's set).
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

/// A gate-side `IdentityService` over the SAME [`SharedRebac`] state. `check(channel.read|post|manage)`
/// resolves `channel.read = member + parent_project->read` against the CURRENT tuple set (the strong
/// read at the latest zookie sees the post-write set — read-your-writes). A `member` grants
/// read/post; `manage` requires `member` (the admin distinction is the membership row's, off this
/// floor). A parent-project reader inherits read.
struct FakeId {
    rebac: SharedRebac,
    /// The principals that read the channel's parent project (the `+ parent_project->read` arm).
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
    /// The `channel:<id>` object id the tuples are keyed on. The gate checks against the
    /// `channel:<id>` ObjectId form directly (the same spelling the tuples use), so the ref string IS
    /// the object id — return it verbatim (mirrors the engine's `object_id_of`, which takes the last
    /// `/`-segment; a `channel:<id>` string has none, so it is its own object id).
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
            // read = member + parent_project->read
            "read" => is_member || self.project_readers.contains(who),
            // post = member
            "post" => is_member,
            // manage = member & parent_project->admin (modelled as member on this floor)
            "manage" => is_member,
            _ => false,
        };
        Ok(if allow {
            Decision::Allow
        } else {
            Decision::Deny
        })
    }

    // ── the rest of the ABI is out of scope for these tests (fail-closed stubs) ──
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

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

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

/// **THE ATOMICITY GATE: the membership row + the `write_tuples` zookie stamp + the
/// `chat.channel.member_added` event commit in ONE transaction (0 partial membership).** On commit
/// all three are durable; an aborted (dropped, uncommitted) transaction commits NEITHER the event
/// (outbox depth 0) — and the order guarantees the row/zookie do not change without the tuple write.
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

    // (a) the zookie is advanced + stamped on the conversation (the new-enemy watermark).
    assert_eq!(zookie.0, "z1");
    assert_eq!(store.get(&cid).unwrap().acl_zookie.as_deref(), Some("z1"));
    // (b) the membership row + index landed.
    assert_eq!(
        store.conversations_of("acme", "alice").unwrap(),
        vec![cid.clone()]
    );
    // (c) the tuple was written (member + watcher).
    assert!(rebac.holds("channel:c1", "member", "alice"));
    assert!(rebac.holds("channel:c1", "watcher", "alice"));
    // (d) the event is STAGED but not yet durable (outbox depth 0 pre-commit — emit-iff-committed).
    assert_eq!(ob.outbox_depth(), 0, "nothing durable before commit");
    assert_eq!(t.staged_len(), 1, "exactly one member_added event staged");

    // commit → the event becomes durable (the co-commit).
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

/// **THE ATOMICITY GATE (the abort half): a `write_tuples` failure aborts the WHOLE change BEFORE
/// the membership row or the event mutate (0 partial membership).** A writer that errors leaves the
/// conversation un-stamped, the index empty, and nothing staged.
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

    // 0 partial membership: no zookie stamped, no row, no staged event.
    assert_eq!(store.get(&cid).unwrap().acl_zookie, None);
    assert!(store.conversations_of("acme", "alice").unwrap().is_empty());
    assert_eq!(t.staged_len(), 0, "no event staged on the aborted change");
}

/// **THE NEW-ENEMY GUARD (the chained test — add → revoke → read): a just-revoked grant cannot read
/// stale (0 stale grants post-revoke).** Add alice (she can read), remove her (the revoke advances
/// the zookie + restamps), then a strong, zookie-stamped read DENIES her — she reads against the
/// post-revoke tuple set, never the pre-revoke one. This is a CHAINED test (EI-01 §4), not a single
/// handler call: the read uses the watermark the revoke stamped.
#[test]
fn new_enemy_guard_revoked_member_cannot_read_post_revoke() {
    let store = MemConversationStore::new();
    let cid = channel(&store, "c1");
    let rebac = SharedRebac::default();
    let svc = MembershipService::new(rebac.clone());
    let gate = MembershipGate::new(FakeId::new(rebac.clone()));
    let (ob, minter) = outbox();

    // ── add alice ──
    let mut t1 = tx(&ob, &minter);
    svc.add_member(&mut t1, &store, Membership::member(cid.clone(), "alice"))
        .unwrap();
    t1.commit().unwrap();
    // alice CAN read at the stamped zookie (she is a member).
    let conv_after_add = store.get(&cid).unwrap();
    let z_add = conv_after_add.acl_zookie.clone();
    assert!(
        gate.check_channel(&subject("alice"), permissions::READ, &cid, z_add.as_deref())
            .is_ok(),
        "a member reads (the `member` arm of channel.read)"
    );

    // ── revoke alice (the new-enemy event) ──
    let mut t2 = tx(&ob, &minter);
    let z_revoke = svc
        .remove_member(&mut t2, &store, &cid, "alice", MembershipRole::Member, true)
        .unwrap();
    t2.commit().unwrap();
    // the revoke ADVANCED the zookie + RESTAMPED the conversation.
    assert_eq!(z_revoke.0, "z2");
    let conv_after_revoke = store.get(&cid).unwrap();
    assert_eq!(conv_after_revoke.acl_zookie.as_deref(), Some("z2"));

    // ── the new-enemy read: alice reads at-or-after the post-revoke watermark → DENIED ──
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
    // structurally: the member tuple is gone.
    assert!(!rebac.holds("channel:c1", "member", "alice"));
}

/// **The Chat ReBAC fragment runtime writes resolve `channel.read = member + parent_project->read`
/// correctly (a non-member denied; a member allowed; a parent-project reader allowed).** The
/// membership write projects the `member` tuple; the gate resolves the frozen clause's two arms.
#[test]
fn channel_read_resolves_member_plus_parent_project() {
    let store = MemConversationStore::new();
    let cid = channel(&store, "c1");
    let rebac = SharedRebac::default();
    let svc = MembershipService::new(rebac.clone());
    // bob reads the parent project (the `+ parent_project->read` arm), carol is a stranger.
    let gate = MembershipGate::new(FakeId::new(rebac.clone()).with_project_reader("bob"));
    let (ob, minter) = outbox();

    let mut t = tx(&ob, &minter);
    svc.add_member(&mut t, &store, Membership::member(cid.clone(), "alice"))
        .unwrap();
    t.commit().unwrap();
    let z = store.get(&cid).unwrap().acl_zookie;

    // the `member` arm: alice reads.
    assert!(gate
        .check_channel(&subject("alice"), permissions::READ, &cid, z.as_deref())
        .is_ok());
    // the `+ parent_project->read` arm: bob (no membership) reads.
    assert!(gate
        .check_channel(&subject("bob"), permissions::READ, &cid, z.as_deref())
        .is_ok());
    // fail-closed: carol (non-member, non-project-reader) is DENIED.
    assert!(matches!(
        gate.check_channel(&subject("carol"), permissions::READ, &cid, z.as_deref()),
        Err(MembershipError::Denied { .. })
    ));
}

/// **The send gate is fail-closed: a non-member cannot post; a member can.** Every send is gated by
/// `channel.post = member` (contract 4.2).
#[test]
fn send_gate_is_fail_closed() {
    let store = MemConversationStore::new();
    let cid = channel(&store, "c1");
    let rebac = SharedRebac::default();
    let svc = MembershipService::new(rebac.clone());
    let gate = MembershipGate::new(FakeId::new(rebac.clone()));
    let (ob, minter) = outbox();

    // before any membership: a non-member cannot send (fail-closed, no leak).
    assert!(matches!(
        gate.check_send(&subject("alice"), &cid, None),
        Err(MembershipError::Denied { .. })
    ));

    let mut t = tx(&ob, &minter);
    svc.add_member(&mut t, &store, Membership::member(cid.clone(), "alice"))
        .unwrap();
    t.commit().unwrap();
    let z = store.get(&cid).unwrap().acl_zookie;
    // now alice (a member) can send.
    assert!(gate
        .check_send(&subject("alice"), &cid, z.as_deref())
        .is_ok());
    // bob (still a non-member) cannot.
    assert!(matches!(
        gate.check_send(&subject("bob"), &cid, z.as_deref()),
        Err(MembershipError::Denied { .. })
    ));
}

/// **The channel lifecycle events co-commit: `chat.channel.created` (+ `chat.channel.linked` for an
/// artifact-linked channel → `refs.edge.created`), and `chat.channel.archived`.** An artifact-linked
/// channel emits BOTH created + linked (the "discussed in" producer, §1.1).
#[test]
fn channel_lifecycle_events_co_commit() {
    let store = MemConversationStore::new();
    let rebac = SharedRebac::default();
    let svc = MembershipService::new(rebac);
    let (ob, minter) = outbox();

    // create an artifact-linked channel → created + linked.
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
    // the linked event carries the linked_ref (the refs.edge.created producer payload).
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

/// **A membership change against a PHANTOM conversation is LOUD (`NotFound`) — never a silent
/// tuple write into nowhere.** The conversation-existence check is the first rung.
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
