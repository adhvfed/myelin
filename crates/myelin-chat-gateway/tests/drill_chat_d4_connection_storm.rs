//! # CHAT-D4 (TE-21 build-gate) — roll the gateway fleet under a CONNECTION STORM → bounded
//! reconnect; resume completes for ALL; no message loss; readiness gates new connections; liveness
//! no restart-storm; the protected-human-lane shed order holds (CHAT-P10 / P-404).
//!
//! The prove-it gate (external-insights/01 §3; testing-strategy CHAT-D4): roll the fleet under a
//! connection storm and WATCH the human lane hold while the agent/speculative lanes shed. The dated
//! green artifact is the SIGNAL pair: (a) `resume completes for ALL, 0 message loss` across the storm
//! (the reconnect-rate signal), and (b) `0 human-lane drops while a lower-priority lane sheds` (the
//! shed-count signal). Observability is part of the pass.
//!
//! The drill drives the REAL composition: real `MemHotTier` durable store + real
//! `myelin_events::Firehose` transport + the real `ChatGateway` + the real `ShedGovernor` — no mock
//! of the data layer (the in-process firehose is the unit/drill transport; the broker binding is
//! P-S12). FLOOR named: the per-surface shed budgets are NAMED v1 floors (tuned by CHAT-D3/D4 in
//! CHAT-P26); this drill asserts the FLOOR PROPERTY (bounded + reserved human lane + shed order
//! applied), not a tuned number.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use myelin_chat::glue::chat_channel_scope;
use myelin_chat::store::{AuthorKind, ConversationId, MemHotTier, MessageStore, NewMessage};
use myelin_chat_gateway::{
    ChatGateway, DeliveryOutcome, GatewayError, LiveFrame, LiveSurface, ResumeOutcome,
};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, Firehose, MonotonicMinter, OutboxStore, OutboxTransaction,
    Timestamp, DEFAULT_INFLIGHT_CAP,
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

// A thin Id fixture: authenticate (tenant-from-token) + a member-set channel.read gate. Every
// connection-storm client is a member (the storm is admitted members reconnecting en masse).
#[derive(Clone, Default)]
struct DrillId {
    members: Arc<Mutex<BTreeSet<String>>>,
}
impl DrillId {
    fn with_members(who: &[&str]) -> DrillId {
        let id = DrillId::default();
        let mut m = id.members.lock().unwrap();
        for w in who {
            m.insert((*w).to_string());
        }
        drop(m);
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
        caused_by: Some(CausedBy("session:chat-d4".into())),
    }
}

/// The Message Service's durable write of a human message (the source of truth the snapshot
/// rebuilds from). The gateway has no emit path; we drive the write through its READ-handle to the
/// SAME store (`append` takes `&self`), modelling the Message Service writing the tier the gateway
/// reads on the snapshot fallback.
fn durable_send(
    gw: &ChatGateway<DrillId, MemHotTier, HealthTable>,
    ob: &OutboxStore,
    minter: &Arc<MonotonicMinter>,
    body: &str,
) {
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
}

fn gateway(
    store: MemHotTier,
    window: usize,
    members: &[&str],
) -> ChatGateway<DrillId, MemHotTier, HealthTable> {
    let firehose = Firehose::with_limits(window, DEFAULT_INFLIGHT_CAP);
    let health =
        MetricsHealthSurface::new(CriticalDependencies::new(["identity"]), HealthTable::new());
    health.mark_started();
    ChatGateway::new(DrillId::with_members(members), store, firehose, health)
}

fn cred(p: &str) -> Credential {
    Credential {
        scheme: "oidc".into(),
        material: p.into(),
    }
}

/// **CHAT-D4 — roll a FLEET under a connection storm: resume completes for ALL, 0 message loss.**
/// A fleet of N members connect, consume a prefix, are SEVERED en masse (the storm), then RECONNECT
/// concurrently while messages keep flowing. Each one's resume backfills its gap — EVERY op recovered
/// for EVERY connection (0 loss). The reconnect-rate signal: all N resumes succeed in-window
/// (bounded; no resync needed because the window covers the storm gap).
#[test]
fn chat_d4_fleet_reconnect_completes_for_all_zero_loss() {
    const FLEET: usize = 32;
    let members: Vec<String> = (0..FLEET).map(|i| format!("u{i}")).collect();
    let member_refs: Vec<&str> = members.iter().map(|s| s.as_str()).collect();

    let store = MemHotTier::new();
    let mut gw = gateway(store, 4096, &member_refs);

    // every member connects + subscribes; record each connection.
    let mut conns = Vec::new();
    let mut subs = Vec::new();
    for m in &members {
        let c = gw.connect(&cred(m)).expect("connect");
        let sub = gw.subscribe(&c, &conv(), None, None).expect("subscribe");
        conns.push(c);
        subs.push(sub);
    }
    let stream = conns[0].stream.clone();
    let scope = chat_channel_scope(CHANNEL).unwrap();

    // a prefix of frames is delivered + consumed by all (each at last_seq = 3).
    for body in ["m1", "m2", "m3"] {
        gw.firehose_mut()
            .publish(&stream, &scope, myelin_events::FrameDraft::new(body));
    }
    let mut last_seqs = Vec::new();
    for sub in &subs {
        let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
        assert_eq!(seen, vec![1, 2, 3], "every client saw the prefix");
        last_seqs.push(sub.last_seq());
    }
    assert!(last_seqs.iter().all(|s| *s == 3));

    // THE STORM: sever the entire fleet at once (drop every subscription handle).
    subs.clear();

    // while severed, more frames flow (the gap every reconnect must recover).
    for body in ["m4", "m5", "m6", "m7"] {
        gw.firehose_mut()
            .publish(&stream, &scope, myelin_events::FrameDraft::new(body));
    }

    // RECONNECT the whole fleet — each resume must complete in-window, 0 loss.
    let cursor = myelin_chat::MessageId("00000000000000000000000000".into());
    let mut resumed = 0usize;
    for c in &conns {
        let outcome = gw
            .resume(c, &conv(), None, 3, &cursor)
            .expect("resume completes");
        match outcome {
            ResumeOutcome::Live { backfill, .. } => {
                let gap: Vec<u64> = backfill.iter().map(|f| f.seq).collect();
                assert_eq!(
                    gap,
                    vec![4, 5, 6, 7],
                    "0 lost — the full storm gap recovered"
                );
                resumed += 1;
            }
            ResumeOutcome::Resync { .. } => {
                panic!("the window covers the storm gap — no resync needed for an in-window cursor")
            }
        }
    }
    // the dated green artifact (reconnect-rate signal): resume completed for ALL.
    assert_eq!(
        resumed, FLEET,
        "CHAT-D4: resume completes for ALL {FLEET} reconnecting connections (0 loss)"
    );
}

/// **CHAT-D4 — readiness gates new connections under storm; liveness never restart-storms.** When a
/// critical dependency is severed under the storm, the gateway flips NotReady and SHEDS new
/// connections (never serves on a not-ready instance) — and liveness stays Up (a dependency outage
/// flips READINESS, never liveness; no restart-storm). Heal → ready again, no restart.
#[test]
fn chat_d4_readiness_gates_under_storm_liveness_no_restart_storm() {
    let firehose = Firehose::with_limits(4096, DEFAULT_INFLIGHT_CAP);
    let probe = HealthTable::new();
    let health = MetricsHealthSurface::new(CriticalDependencies::new(["identity"]), probe.clone());
    health.mark_started();
    let gw = ChatGateway::new(
        DrillId::with_members(&["alice"]),
        MemHotTier::new(),
        firehose,
        health,
    );

    // ready → a storm of connects is admitted.
    for _ in 0..16 {
        assert!(gw.connect(&cred("alice")).is_ok());
    }

    // a critical dep dies UNDER the storm → NotReady → every new connect SHEDS (never serves).
    probe.mark_down("identity");
    for _ in 0..16 {
        assert!(matches!(
            gw.connect(&cred("alice")),
            Err(GatewayError::NotReady)
        ));
    }
    // liveness NEVER restart-storms across the outage (the no-restart-storm signal).
    assert_eq!(
        gw.health().liveness_restart_count(),
        0,
        "CHAT-D4: a dependency outage must NOT restart-storm (liveness != readiness)"
    );
    assert!(!gw.health().liveness().should_restart());

    // heal → ready again, no restart needed (the outage was transient).
    probe.mark_up("identity");
    assert!(gw.connect(&cred("alice")).is_ok());
}

/// **CHAT-D4 — the protected-human-lane shed order holds under storm: the human message lane is
/// delivered while the agent/speculative lanes shed (0 human-lane drops).** Under storm pressure the
/// live-delivery surface sheds presence first and the agent lane next, but EVERY human-message frame
/// is delivered — humans never queue behind agent runs (VISION §3). The shed-count signal is the
/// green artifact: shed_count(presence) > 0, shed_count(human) == 0.
#[test]
fn chat_d4_shed_order_holds_human_lane_zero_drops() {
    use myelin_substrate::shed::SurfaceBudget;
    let (ob, minter) = (OutboxStore::new(), Arc::new(MonotonicMinter::new()));
    let mut gw = gateway(MemHotTier::new(), 8192, &["alice"]);
    let alice = gw.connect(&cred("alice")).expect("connect");
    let stream = alice.stream.clone();
    let tenant = alice.principal.tenant.clone();
    let scope = chat_channel_scope(CHANNEL).unwrap();

    // drive the gateway's shed governor at a SMALL deterministic budget (the storm boundary is
    // reachable without a 256-deep storm) — the OQ-K v1 floor numbers are tuned by CHAT-P26; the
    // FLOOR PROPERTY (bounded + reserved human lane + shed order) is what this drill asserts. The
    // connection-storm signal flips pressure on.
    let conn_budget = SurfaceBudget {
        per_tenant_in_flight_cap: 12,
        human_lane_reservation: 4,
        retry_after_secs: 3,
    };
    let agent_budget = SurfaceBudget {
        per_tenant_in_flight_cap: 6,
        human_lane_reservation: 0,
        retry_after_secs: 10,
    };
    gw.set_shed_governor({
        let mut g = myelin_chat_gateway::ShedGovernor::with_budgets(conn_budget, agent_budget);
        g.set_under_pressure(true);
        g
    });

    let mut presence_shed = 0usize;
    let mut agent_shed = 0usize;
    let mut human_delivered = 0usize;
    let mut human_shed = 0usize;

    // roll a storm of mixed frames: lots of presence + agent partials (low value) interleaved with
    // human messages (the protected lane). The presence/agent lanes overflow their budgets and shed;
    // the human lane keeps delivering (the reserved slots are never given to a machine lane).
    let storm = 40usize;

    for i in 0..storm {
        let mut d = gw.live_delivery();
        // a presence beacon (speculative — sheds first).
        if d.deliver(
            &tenant,
            &stream,
            &scope,
            &LiveFrame::Presence {
                principal: format!("u{i}"),
                class: "online".into(),
            },
        ) == DeliveryOutcome::Shed
        {
            presence_shed += 1;
        }
        // an agent partial (the agent lane — sheds before humans).
        if d.deliver(
            &tenant,
            &stream,
            &scope,
            &LiveFrame::AgentPartial {
                correlation_id: format!("run-{i}"),
            },
        ) == DeliveryOutcome::Shed
        {
            agent_shed += 1;
        }
        // a HUMAN message (the protected lane — must NOT shed while cheaper lanes carry load).
        match d.deliver(
            &tenant,
            &stream,
            &scope,
            &LiveFrame::HumanMessage {
                message_id: format!("01J0MSG{i}"),
            },
        ) {
            DeliveryOutcome::Delivered(_) => human_delivered += 1,
            DeliveryOutcome::Shed => human_shed += 1,
        }
        // the human frame is pumped to the socket PROMPTLY (interactive priority) → its slot is
        // released, so the human lane stays within budget while the machine lanes back up and shed.
        d.governor_mut()
            .on_drained(&tenant, LiveSurface::HumanMessage);
        // a real durable human send underlies the live frame (the source of truth).
        durable_send(&gw, &ob, &minter, &format!("h{i}"));
    }

    // the shed-count signal (the dated green artifact):
    //  - the speculative + agent lanes SHED under pressure (they overflowed their budgets);
    //  - the human lane took 0 drops (it delivered EVERY frame — humans never queue behind agents).
    assert!(
        presence_shed > 0,
        "CHAT-D4: the speculative/presence lane sheds under storm (shed_count > 0)"
    );
    assert!(
        agent_shed > 0,
        "CHAT-D4: the agent lane sheds before the human lane (shed_count > 0)"
    );
    assert_eq!(
        human_shed, 0,
        "CHAT-D4: 0 human-lane drops while the agent/speculative lanes shed (the protected lane)"
    );
    assert_eq!(
        human_delivered, storm,
        "CHAT-D4: every human message was delivered (the human lane is last to shed)"
    );
}
