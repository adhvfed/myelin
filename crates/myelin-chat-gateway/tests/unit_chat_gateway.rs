//! Unit tests for the CHAT-P9 stateless connection-tier gateway (the DB-free, behaviour-identical
//! floor). The gateway COMPOSES the frozen pieces — these tests prove the composition's load-bearing
//! properties:
//!  - subscribe scope is BOUNDED (never `*`; 0 unbounded subscriptions) + membership-gated;
//!  - resume backfills the gap then live (ZERO ops lost);
//!  - an out-of-window cursor → `resync_required` → the `*.snapshot` resync (resync_from), still 0 lost;
//!  - the gateway readiness-gates new connections (a dead critical dep → shed; liveness never
//!    restart-storms);
//!  - the TE-21 pin is the Rust no-op (the 1.7 cross-language harness shim).
//!
//! The CHAINED drill (subscribe → deliver → sever → resume → assert 0 lost/0 dup) lives in
//! `tests/drill_chat_d1_resume.rs` (the dated green artifact). The CDC providers for rows 3.5 / 2.6
//! live in `tests/cdc_3_5_2_6_chat_gateway.rs`.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use myelin_chat::store::{
    AuthorKind, ConversationId, MemHotTier, MessageStore, NewMessage, RangeCursor,
};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, FrameDraft, MonotonicMinter, OutboxStore, OutboxTransaction,
    Timestamp,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_substrate::metrics_health::{
    CriticalDependencies, HealthTable, MetricsHealthSurface, Readiness,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use myelin_chat::glue::{chat_channel_scope, Te21LanguagePin};
use myelin_chat_gateway::{channel_stream, te21_pin, ChatGateway, GatewayError, ResumeOutcome};
use myelin_events::Firehose;

type IdResult<T> = myelin_identity::Result<T>;

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const CHANNEL: &str = "01J0CHANNEL";

// ---------------------------------------------------------------------------------------------
// A minimal Id fixture: `authenticate` resolves a Principal (tenant FROM the token, ID-3) and
// `check` resolves the channel.read gate against a member set. NOT a second engine — a thin fake
// modelling the two surfaces the gateway consumes (EI-01 §7).
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
struct FakeId {
    /// The principals the channel admits as `member` (the `channel.read` gate resolves against this).
    members: Arc<Mutex<BTreeSet<String>>>,
    /// `true` once the credential should resolve (a revoked/disabled credential resolves to an error).
    auth_ok: Arc<Mutex<bool>>,
}

impl FakeId {
    fn new() -> FakeId {
        FakeId {
            members: Arc::new(Mutex::new(BTreeSet::new())),
            auth_ok: Arc::new(Mutex::new(true)),
        }
    }
    fn add_member(&self, who: &str) {
        self.members.lock().unwrap().insert(who.to_string());
    }
    fn remove_member(&self, who: &str) {
        self.members.lock().unwrap().remove(who);
    }
    fn set_auth_ok(&self, ok: bool) {
        *self.auth_ok.lock().unwrap() = ok;
    }
}

impl IdentityService for FakeId {
    /// 4.1 — resolve the credential to a Principal. The credential's `material` IS the principal id;
    /// the `tenant` is derived from the VERIFIED token (ID-3 — never a path the client controls).
    fn authenticate(&self, c: &Credential) -> IdResult<Principal> {
        if !*self.auth_ok.lock().unwrap() {
            return Err(AuthzError::NotYetImplemented(
                "credential revoked / disabled",
            ));
        }
        Ok(Principal::new(
            TenantId(TENANT.into()),
            Region(REGION.into()),
            PrincipalId(c.material.clone()),
            PrincipalKind::Human,
            myelin_identity::DataRole::Controller,
            myelin_identity::PrincipalStatus::Active,
        ))
    }

    /// 4.2 — the channel.read gate: a member is allowed, a non-member is denied (fail-closed).
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        _object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let who = subject.principal_id.0.as_str();
        let is_member = self.members.lock().unwrap().contains(who);
        let allow = matches!(permission.0.as_str(), "read" | "post" | "manage") && is_member;
        Ok(if allow {
            Decision::Allow
        } else {
            Decision::Deny
        })
    }

    // ── the rest of the ABI is out of scope (fail-closed stubs) ──
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

fn conv() -> ConversationId {
    ConversationId::new(TENANT, REGION, CHANNEL)
}

fn cred(principal: &str) -> Credential {
    Credential {
        scheme: "oidc".into(),
        material: principal.into(),
    }
}

/// A gateway with a small firehose window (so the over-window resync path is reachable
/// deterministically) and a started (ready) health surface.
fn gateway(
    id: FakeId,
    store: MemHotTier,
    window: usize,
) -> ChatGateway<FakeId, MemHotTier, HealthTable> {
    let firehose = Firehose::with_limits(window, myelin_events::DEFAULT_INFLIGHT_CAP);
    // No critical dependencies declared down-able by default → the only readiness lever is the
    // startup gate (flipped to started below) + an explicitly-marked-down critical dep.
    let critical = CriticalDependencies::new(["identity"]);
    let health = MetricsHealthSurface::new(critical, HealthTable::new());
    health.mark_started();
    ChatGateway::new(id, store, firehose, health)
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
        caused_by: Some(CausedBy("session:gw".into())),
    }
}

/// Append a real durable message through the Message Service path (the store + outbox co-commit),
/// returning its durable id. (The gateway never WRITES — this is the Message Service's job; the
/// gateway only reads the store on the snapshot fallback.)
fn send(store: &MemHotTier, ob: &OutboxStore, minter: &Arc<MonotonicMinter>, nonce: &str) {
    let mut tx: OutboxTransaction = ob.begin(minter.clone(), ctx_base());
    store
        .append(
            &mut tx,
            NewMessage {
                conv: conv(),
                thread_root_id: None,
                author: "alice".into(),
                author_kind: AuthorKind::Human,
                body_inline: nonce.as_bytes().to_vec(),
                body_nodes: Vec::new(),
                client_nonce: nonce.into(),
            },
        )
        .expect("append");
    tx.commit().expect("commit");
}

// ============================================================================================
// connect — authenticate + readiness gate (4.1 / 1.3 / ID-3)
// ============================================================================================

/// **connect resolves a Principal with the tenant FROM the verified token (ID-3), never the path.**
#[test]
fn connect_resolves_principal_tenant_from_token() {
    let id = FakeId::new();
    let gw = gateway(id, MemHotTier::new(), 4096);
    let conn = gw.connect(&cred("alice")).expect("authenticate");
    assert_eq!(
        conn.tenant(),
        TENANT,
        "tenant comes from the verified token"
    );
    assert_eq!(conn.principal.principal_id.0, "alice");
    // the firehose stream is keyed by the VERIFIED tenant (fan.<tenant>), never a client path.
    assert_eq!(conn.stream, format!("fan.{TENANT}"));
}

/// **connect refuses a revoked/disabled credential (no socket opens) — fail-closed (4.1).**
#[test]
fn connect_refuses_a_revoked_credential() {
    let id = FakeId::new();
    id.set_auth_ok(false);
    let gw = gateway(id, MemHotTier::new(), 4096);
    let err = gw
        .connect(&cred("alice"))
        .expect_err("a revoked credential is refused");
    assert!(matches!(err, GatewayError::Unauthenticated(_)));
}

/// **The gateway readiness-gates new connections: a dead critical dependency → SHED, never serve
/// (1.3 liveness != readiness).** A severed `identity` flips readiness to NotReady; connect sheds.
/// Liveness is INDEPENDENT — it never restart-storms across the outage (no liveness restart ticks).
#[test]
fn connect_sheds_when_not_ready_liveness_never_restart_storms() {
    let id = FakeId::new();
    id.add_member("alice");
    let firehose = Firehose::with_limits(4096, myelin_events::DEFAULT_INFLIGHT_CAP);
    let critical = CriticalDependencies::new(["identity"]);
    let probe = HealthTable::new();
    let health = MetricsHealthSurface::new(critical, probe.clone());
    health.mark_started();
    let gw = ChatGateway::new(id, MemHotTier::new(), firehose, health);

    // ready → connect succeeds.
    assert_eq!(gw.readiness(), Readiness::Ready);
    assert!(gw.connect(&cred("alice")).is_ok());

    // sever the critical `identity` dependency → not-ready → connect SHEDS (never serves).
    probe.mark_down("identity");
    assert_eq!(gw.readiness(), Readiness::NotReady);
    assert!(matches!(
        gw.connect(&cred("alice")),
        Err(GatewayError::NotReady)
    ));

    // liveness NEVER restart-storms across the outage (readiness sheds; liveness is independent).
    assert_eq!(
        gw.health().liveness_restart_count(),
        0,
        "a dependency outage must NOT restart-storm (liveness != readiness)"
    );
    assert!(
        !gw.health().liveness().should_restart(),
        "liveness stays Up across a dependency outage"
    );

    // heal → ready again (the outage is transient; the gateway recovers without a restart).
    probe.mark_up("identity");
    assert_eq!(gw.readiness(), Readiness::Ready);
    assert!(gw.connect(&cred("alice")).is_ok());
}

// ============================================================================================
// subscribe — bounded scope + membership gate (3.5 / 4.2)
// ============================================================================================

/// **subscribe is membership-gated: a non-member is fail-closed (4.2), a member subscribes.** NO
/// field of the channel is read on a denial (the leak-free chokepoint — the gate returns NotAMember).
#[test]
fn subscribe_is_membership_gated_fail_closed() {
    let id = FakeId::new();
    id.add_member("alice");
    let mut gw = gateway(id, MemHotTier::new(), 4096);

    let alice = gw.connect(&cred("alice")).unwrap();
    assert!(
        gw.subscribe(&alice, &conv(), None, None).is_ok(),
        "a member subscribes"
    );

    // mallory is NOT a member → fail-closed NotAMember (no subscription, no leak).
    let mallory = gw.connect(&cred("mallory")).unwrap();
    assert!(matches!(
        gw.subscribe(&mallory, &conv(), None, None),
        Err(GatewayError::NotAMember(_))
    ));
}

/// **subscribe scope is BOUNDED — `channel:<id>`, never `*` (the 0-unbounded-subscriptions gate,
/// 3.5).** Every subscription a member opens rides a bounded `Channel` scope; an unbounded scope is
/// structurally unrepresentable (the gateway builds the scope through the `*`-rejecting chokepoint).
#[test]
fn subscribe_scope_is_bounded_never_star() {
    let id = FakeId::new();
    id.add_member("alice");
    let mut gw = gateway(id, MemHotTier::new(), 4096);
    let alice = gw.connect(&cred("alice")).unwrap();
    let sub = gw
        .subscribe(&alice, &conv(), None, None)
        .expect("subscribe");
    assert_eq!(
        sub.scope().selector(),
        format!("channel:{CHANNEL}"),
        "the subscription scope is the bounded channel:<id> slice, never `*`"
    );
    assert_eq!(sub.scope().kind(), myelin_events::ScopeKind::Channel);
}

/// An allocation-hostile channel id is an over-broad selector at the gateway boundary. The
/// transport's typed scope-limit refusal is never admitted or downgraded to a generic failure.
#[test]
fn subscribe_scope_limit_fails_closed_as_over_broad_scope() {
    let id = FakeId::new();
    id.add_member("alice");
    let mut gw = gateway(id, MemHotTier::new(), 4096);
    let alice = gw.connect(&cred("alice")).unwrap();
    let oversized = ConversationId::new(TENANT, REGION, "x".repeat(16 * 1024));

    assert!(matches!(
        gw.subscribe(&alice, &oversized, None, None),
        Err(GatewayError::OverBroadScope(
            myelin_events::FirehoseError::ScopeLimitExceeded { .. }
        ))
    ));
}

/// **A None-cursor subscribe starts live from now; only post-open frames are delivered.**
#[test]
fn subscribe_none_cursor_starts_live_from_now() {
    let id = FakeId::new();
    id.add_member("alice");
    let mut gw = gateway(id, MemHotTier::new(), 4096);
    let alice = gw.connect(&cred("alice")).unwrap();
    let stream = alice.stream.clone();
    let scope = chat_channel_scope(CHANNEL).unwrap();

    // a frame published BEFORE the subscribe opens is not delivered (live-from-now).
    gw.firehose_mut()
        .publish(&stream, &scope, FrameDraft::new("old"));
    let sub = gw.subscribe(&alice, &conv(), None, None).unwrap();
    assert!(sub.drain_ready().is_empty(), "no backfill on a None cursor");

    // frames published after the subscribe ARE delivered.
    gw.firehose_mut()
        .publish(&stream, &scope, FrameDraft::new("new-1"));
    gw.firehose_mut()
        .publish(&stream, &scope, FrameDraft::new("new-2"));
    let live: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(live, vec![2, 3], "only post-open live frames are delivered");
}

// ============================================================================================
// resume — backfill the gap then live, ZERO ops lost (3.5 / arch §1.3)
// ============================================================================================

/// **resume backfills `(last_seq, now]` then live — ZERO ops lost (the CHAT-D1 in-window leg).** A
/// client at last_seq=2; 3,4,5 published while disconnected; resume recovers EXACTLY {3,4,5} then a
/// live 6 — contiguous, 0 lost, 0 dup.
#[test]
fn resume_in_window_recovers_the_gap_zero_lost() {
    let id = FakeId::new();
    id.add_member("alice");
    let mut gw = gateway(id, MemHotTier::new(), 4096);
    let alice = gw.connect(&cred("alice")).unwrap();
    let stream = alice.stream.clone();
    let scope = chat_channel_scope(CHANNEL).unwrap();

    for p in ["m1", "m2", "m3", "m4", "m5"] {
        gw.firehose_mut()
            .publish(&stream, &scope, FrameDraft::new(p));
    }
    // a durable cursor (unused on the in-window path) — the message-id space resync anchor.
    let cursor = myelin_chat::MessageId("00000000000000000000000000".into());

    let outcome = gw
        .resume(&alice, &conv(), None, 2, &cursor)
        .expect("in-window resume");
    let (backfill, sub) = match outcome {
        ResumeOutcome::Live { backfill, sub } => (backfill, sub),
        ResumeOutcome::Resync { .. } => panic!("in-window cursor must NOT resync"),
    };
    let gap: Vec<u64> = backfill.iter().map(|f| f.seq).collect();
    assert_eq!(
        gap,
        vec![3, 4, 5],
        "the gap (last_seq, now] is replayed — 0 lost"
    );

    // live continues gap-free, no duplicate.
    gw.firehose_mut()
        .publish(&stream, &scope, FrameDraft::new("m6"));
    let live: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(live, vec![6], "live continues gap-free, 0 dup");

    let mut all = gap;
    all.extend(live);
    assert_eq!(all, vec![3, 4, 5, 6], "across the reconnect: 0 lost, 0 dup");
}

/// **An out-of-window cursor → `resync_required` → the durable `*.snapshot` resync (resync_from),
/// STILL 0 lost (the CHAT-D1 resync leg, contract 2.6 — NAMED not silent).** A SMALL firehose window
/// forces the gap's head to be evicted; the gateway falls back to the store's gap-free snapshot then
/// resumes live. Every durable message after the snapshot cursor is in the snapshot (0 lost).
#[test]
fn resume_out_of_window_falls_back_to_snapshot_resync_zero_lost() {
    let id = FakeId::new();
    id.add_member("alice");
    let store = MemHotTier::new();
    let (ob, minter) = (OutboxStore::new(), Arc::new(MonotonicMinter::new()));

    // the DURABLE log holds m1..m6 (the source of truth the snapshot rebuilds from).
    for n in ["m1", "m2", "m3", "m4", "m5", "m6"] {
        send(&store, &ob, &minter, n);
    }
    let all = store.range(&conv(), RangeCursor::Recent, 100).unwrap();
    assert_eq!(all.len(), 6);
    // the client last rendered m2 (its durable resume cursor in the message-id space).
    let cursor = all[1].message_id.clone();

    // a SMALL firehose window (3 frames): publish 6 live frames → 1,2,3 evicted (window holds 4,5,6).
    let mut gw = gateway(id, store, 3);
    let alice = gw.connect(&cred("alice")).unwrap();
    let stream = alice.stream.clone();
    let scope = chat_channel_scope(CHANNEL).unwrap();
    for n in 0..6 {
        gw.firehose_mut()
            .publish(&stream, &scope, FrameDraft::new(format!("f{n}")));
    }

    // resume at last_seq=2 → the firehose gap head (seq 3) was evicted → resync_required → fall back
    // to the durable *.snapshot resync (resync_from after the m2 cursor).
    let outcome = gw
        .resume(&alice, &conv(), None, 2, &cursor)
        .expect("resync fallback");
    let (snapshot, sub) = match outcome {
        ResumeOutcome::Resync { snapshot, sub } => (snapshot, sub),
        ResumeOutcome::Live { .. } => panic!("an out-of-window cursor MUST resync, not in-window"),
    };
    // the snapshot is everything strictly after m2 → m3,m4,m5,m6 (gap-free, ordered) — 0 lost.
    let bodies: Vec<Vec<u8>> = snapshot.iter().map(|m| m.body_inline.clone()).collect();
    assert_eq!(
        bodies,
        vec![
            b"m3".to_vec(),
            b"m4".to_vec(),
            b"m5".to_vec(),
            b"m6".to_vec()
        ],
        "the snapshot recovers everything after the cursor — 0 lost"
    );
    // and a fresh LIVE subscription continues after the snapshot (overlap is client-deduped on
    // message_id) — a subsequent live frame is delivered with no gap.
    gw.firehose_mut()
        .publish(&stream, &scope, FrameDraft::new("f6"));
    assert_eq!(
        sub.drain_ready().len(),
        1,
        "live continues after the snapshot resync"
    );
}

/// **A revoked member cannot resume (the new-enemy guard at the gateway, 4.2).** A grant revoked
/// while the client was disconnected denies the resume — a removed member cannot recover the live
/// stream of a channel it no longer reads.
#[test]
fn resume_re_gates_membership_new_enemy_guard() {
    let id = FakeId::new();
    id.add_member("alice");
    let mut gw = gateway(id.clone(), MemHotTier::new(), 4096);
    let alice = gw.connect(&cred("alice")).unwrap();
    let stream = alice.stream.clone();
    let scope = chat_channel_scope(CHANNEL).unwrap();
    gw.firehose_mut()
        .publish(&stream, &scope, FrameDraft::new("m1"));

    // alice was removed while disconnected → the resume is denied (fail-closed).
    id.remove_member("alice");
    let cursor = myelin_chat::MessageId("00000000000000000000000000".into());
    assert!(matches!(
        gw.resume(&alice, &conv(), None, 0, &cursor),
        Err(GatewayError::NotAMember(_))
    ));
}

// ============================================================================================
// TE-21 — the Rust connection-tier pin is a 1.7 no-op
// ============================================================================================

/// **The TE-21 connection-tier pin is Rust (the default) and the 1.7 cross-language harness shim is
/// a NO-OP.** The BEAM/Phoenix hatch is written-but-CLOSED (opened only at CHAT-P26).
#[test]
fn te21_pin_is_the_rust_no_op() {
    assert_eq!(te21_pin(), Te21LanguagePin::Rust);
    assert!(te21_pin().is_no_op(), "the all-Rust pin is a 1.7 no-op");
}

/// **The firehose stream name is derived from the tenant, never a literal (X-5 — names anchor).**
#[test]
fn channel_stream_is_derived_from_tenant() {
    assert_eq!(channel_stream("acme"), "fan.acme");
    assert_eq!(channel_stream("globex"), "fan.globex");
}
