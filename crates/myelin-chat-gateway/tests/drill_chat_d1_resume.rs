//! # CHAT-D1 — sever the gateway↔firehose mid-publish → resume recovers the gap, 0 lost / 0 dup;
//! last_seq past the window → resync_required → *.snapshot, still 0 lost (CHAT-P9 / P-403).
//!
//! The build-order item-0 gate (external-insights/04 §2.2): a relay WITHOUT resume cursors silently
//! loses the gap on a reconnect. This drill proves the gateway's resume-cursor backbone recovers
//! EVERY op across a severed connection — the zero-loss-across-reconnect property
//! (architecture chat 02 §1.3; testing-strategy CHAT-D1). The dated green artifact is the assertion
//! pair `0 lost / 0 dup` on BOTH legs (in-window backfill AND over-window *.snapshot resync).
//!
//! The drill drives the REAL composition: a real `MemHotTier` durable store (the Message Service's
//! tier) + the real `myelin_events::Firehose` transport + the real `ChatGateway` — no mock of the
//! data layer (the in-process firehose is the unit/drill transport; the broker binding is P-S12).
//! The CHAINED scenario is exactly the prompt's TESTS line: subscribe → deliver → sever → resume →
//! assert 0 lost / 0 dup.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use myelin_chat::glue::chat_channel_scope;
use myelin_chat::store::{
    AuthorKind, ConversationId, MemHotTier, MessageStore, NewMessage, RangeCursor,
};
use myelin_chat_gateway::{ChatGateway, ResumeOutcome};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, Firehose, FrameDraft, MonotonicMinter, OutboxStore,
    OutboxTransaction, Timestamp, DEFAULT_INFLIGHT_CAP,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_substrate::metrics_health::{CriticalDependencies, HealthTable, MetricsHealthSurface};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

type IdResult<T> = myelin_identity::Result<T>;

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const CHANNEL: &str = "01J0CHANNEL";

// A thin Id fixture: authenticate (tenant-from-token) + a member-set channel.read gate.
#[derive(Clone, Default)]
struct DrillId {
    members: Arc<Mutex<BTreeSet<String>>>,
}
impl DrillId {
    fn with_member(who: &str) -> DrillId {
        let id = DrillId::default();
        id.members.lock().unwrap().insert(who.to_string());
        id
    }
}
impl IdentityService for DrillId {
    fn authenticate(&self, c: &Credential) -> IdResult<Principal> {
        Ok(Principal::new(
            TenantId(TENANT.into()),
            Region(REGION.into()),
            PrincipalId(c.material.clone()),
            PrincipalKind::Human,
            myelin_identity::DataRole::Controller,
            myelin_identity::PrincipalStatus::Active,
        ))
    }
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        _o: &ArtifactRef,
        _a: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let is_member = self
            .members
            .lock()
            .unwrap()
            .contains(subject.principal_id.0.as_str());
        Ok(
            if matches!(permission.0.as_str(), "read" | "post" | "manage") && is_member {
                Decision::Allow
            } else {
                Decision::Deny
            },
        )
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

fn conv() -> ConversationId {
    ConversationId::new(TENANT, REGION, CHANNEL)
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId(TENANT.into()),
        region: Region(REGION.into()),
        actor: Actor(Principal::stub(
            PrincipalId("svc".into()),
            PrincipalKind::Service,
            TenantId(TENANT.into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:drill".into())),
    }
}

/// The Message Service's durable send + the firehose publish of the rendered frame (architecture
/// §1.2: when the Message Service persists a message, the Indexing/Outbox feeder ALSO publishes the
/// rendered frame to the firehose). The gateway has NO emit path — it only reads these. We drive the
/// write through the gateway's READ-handle to the SAME store (`MessageStore::append` takes `&self`),
/// modelling the Message Service writing the tier the gateway reads on the snapshot fallback.
fn deliver(
    gw: &mut ChatGateway<DrillId, MemHotTier, HealthTable>,
    ob: &OutboxStore,
    minter: &Arc<MonotonicMinter>,
    stream: &str,
    body: &str,
) {
    // 1) the DURABLE write (the source of truth; the snapshot rebuilds from this).
    let mut tx: OutboxTransaction = ob.begin(minter.clone(), ctx_base());
    gw.store()
        .append(
            &mut tx,
            NewMessage {
                conv: conv(),
                thread_root_id: None,
                author: "alice".into(),
                author_kind: AuthorKind::Human,
                body_inline: body.as_bytes().to_vec(),
                body_nodes: Vec::new(),
                client_nonce: body.into(),
            },
        )
        .expect("durable append");
    tx.commit().expect("commit");
    // 2) the EPHEMERAL firehose frame (allowed-to-drop; resume/snapshot recovers a missed one) —
    //    published onto the gateway's transport so an OPEN subscription is fanned to.
    let scope = chat_channel_scope(CHANNEL).unwrap();
    gw.firehose_mut()
        .publish(stream, &scope, FrameDraft::new(body));
}

fn gateway(store: MemHotTier, window: usize) -> ChatGateway<DrillId, MemHotTier, HealthTable> {
    let firehose = Firehose::with_limits(window, DEFAULT_INFLIGHT_CAP);
    let health =
        MetricsHealthSurface::new(CriticalDependencies::new(["identity"]), HealthTable::new());
    health.mark_started();
    ChatGateway::new(DrillId::with_member("alice"), store, firehose, health)
}

/// **CHAT-D1 — IN-WINDOW leg: sever mid-publish, resume recovers the gap, 0 lost / 0 dup.** alice
/// subscribes and consumes up to seq 2; the connection is SEVERED (we drop the subscription handle);
/// 3,4,5 are delivered while she is gone; on reconnect `resume(last_seq=2)` recovers EXACTLY {3,4,5}
/// then a live 6 — 0 lost, 0 dup.
#[test]
fn chat_d1_sever_then_resume_recovers_the_gap_zero_lost_zero_dup() {
    let store = MemHotTier::new();
    let (ob, minter) = (OutboxStore::new(), Arc::new(MonotonicMinter::new()));
    let mut gw = gateway(store, 4096);
    let alice = gw
        .connect(&Credential {
            scheme: "oidc".into(),
            material: "alice".into(),
        })
        .unwrap();
    let stream = alice.stream.clone();

    // subscribe → deliver 1,2 → consume them.
    let sub = gw
        .subscribe(&alice, &conv(), None, None)
        .expect("subscribe");
    deliver(&mut gw, &ob, &minter, &stream, "m1");
    deliver(&mut gw, &ob, &minter, &stream, "m2");
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(seen, vec![1, 2], "the client saw 1,2 before the sever");
    let last_seq = sub.last_seq();
    assert_eq!(last_seq, 2);

    // SEVER: drop the subscription handle (the connection died). Meanwhile 3,4,5 are delivered.
    drop(sub);
    deliver(&mut gw, &ob, &minter, &stream, "m3");
    deliver(&mut gw, &ob, &minter, &stream, "m4");
    deliver(&mut gw, &ob, &minter, &stream, "m5");

    // RECONNECT: resume from last_seq=2 → backfill {3,4,5} then live.
    let cursor = myelin_chat::MessageId("00000000000000000000000000".into());
    let outcome = gw
        .resume(&alice, &conv(), None, last_seq, &cursor)
        .expect("resume");
    let (backfill, sub2) = match outcome {
        ResumeOutcome::Live { backfill, sub } => (backfill, sub),
        ResumeOutcome::Resync { .. } => panic!("an in-window cursor must NOT resync"),
    };
    let recovered: Vec<u64> = backfill.iter().map(|f| f.seq).collect();
    assert_eq!(recovered, vec![3, 4, 5], "0 lost — the full gap recovered");

    // live continues gap-free, 0 dup.
    deliver(&mut gw, &ob, &minter, &stream, "m6");
    let live: Vec<u64> = sub2.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(live, vec![6], "0 dup — live continues from the gap head");

    // the dated green artifact: every op exactly once across the reconnect (0 lost, 0 dup).
    let mut all = seen;
    all.extend(recovered);
    all.extend(live);
    assert_eq!(all, vec![1, 2, 3, 4, 5, 6], "CHAT-D1: 0 lost / 0 dup");
}

/// **CHAT-D1 — RESYNC leg: last_seq past the window → resync_required → *.snapshot, still 0 lost.** A
/// SMALL firehose window evicts the gap's head; the gateway falls back to the durable snapshot
/// (resync_from) which is gap-free — every durable message after the cursor is recovered (0 lost).
#[test]
fn chat_d1_out_of_window_resync_required_snapshot_zero_lost() {
    let store = MemHotTier::new();
    let (ob, minter) = (OutboxStore::new(), Arc::new(MonotonicMinter::new()));
    // window holds only 3 frames — the over-window resync is deterministic.
    let mut gw = gateway(store, 3);
    let alice = gw
        .connect(&Credential {
            scheme: "oidc".into(),
            material: "alice".into(),
        })
        .unwrap();
    let stream = alice.stream.clone();

    // deliver m1..m6 durably + over the firehose (window now holds frames 4,5,6; 1,2,3 evicted).
    for n in ["m1", "m2", "m3", "m4", "m5", "m6"] {
        deliver(&mut gw, &ob, &minter, &stream, n);
    }
    let all = gw.store().range(&conv(), RangeCursor::Recent, 100).unwrap();
    assert_eq!(all.len(), 6, "the durable log is the source of truth");
    let cursor = all[1].message_id.clone(); // the client last rendered m2.

    // resume at last_seq=2 → seq 3 evicted → resync_required → durable *.snapshot resync.
    let outcome = gw
        .resume(&alice, &conv(), None, 2, &cursor)
        .expect("resync fallback");
    let snapshot = match outcome {
        ResumeOutcome::Resync { snapshot, .. } => snapshot,
        ResumeOutcome::Live { .. } => panic!("an out-of-window cursor MUST resync"),
    };
    let bodies: Vec<Vec<u8>> = snapshot.iter().map(|m| m.body_inline.clone()).collect();
    assert_eq!(
        bodies,
        vec![
            b"m3".to_vec(),
            b"m4".to_vec(),
            b"m5".to_vec(),
            b"m6".to_vec()
        ],
        "CHAT-D1 resync leg: the *.snapshot recovers everything after the cursor — 0 lost"
    );
}
