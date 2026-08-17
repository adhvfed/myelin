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

#[test]
fn chat_d4_fleet_reconnect_completes_for_all_zero_loss() {
    const FLEET: usize = 32;
    let members: Vec<String> = (0..FLEET).map(|i| format!("u{i}")).collect();
    let member_refs: Vec<&str> = members.iter().map(|s| s.as_str()).collect();

    let store = MemHotTier::new();
    let mut gw = gateway(store, 4096, &member_refs);

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

    for body in ["m1", "m2", "m3"] {
        gw.firehose_mut()
            .publish(&stream, &scope, myelin_events::FrameDraft::new(body))
            .expect("the fixture publishes a valid frame");
    }
    let mut last_seqs = Vec::new();
    for sub in &subs {
        let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
        assert_eq!(seen, vec![1, 2, 3], "every client saw the prefix");
        last_seqs.push(sub.last_seq());
    }
    assert!(last_seqs.iter().all(|s| *s == 3));

    subs.clear();

    for body in ["m4", "m5", "m6", "m7"] {
        gw.firehose_mut()
            .publish(&stream, &scope, myelin_events::FrameDraft::new(body))
            .expect("the fixture publishes a valid frame");
    }

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
                    "0 lost - the full storm gap recovered"
                );
                resumed += 1;
            }
            ResumeOutcome::Resync { .. } => {
                panic!("the window covers the storm gap - no resync needed for an in-window cursor")
            }
        }
    }
    assert_eq!(
        resumed, FLEET,
        "CHAT-D4: resume completes for ALL {FLEET} reconnecting connections (0 loss)"
    );
}

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

    for _ in 0..16 {
        assert!(gw.connect(&cred("alice")).is_ok());
    }

    probe.mark_down("identity");
    for _ in 0..16 {
        assert!(matches!(
            gw.connect(&cred("alice")),
            Err(GatewayError::NotReady)
        ));
    }
    assert_eq!(
        gw.health().liveness_restart_count(),
        0,
        "CHAT-D4: a dependency outage must NOT restart-storm (liveness != readiness)"
    );
    assert!(!gw.health().liveness().should_restart());

    probe.mark_up("identity");
    assert!(gw.connect(&cred("alice")).is_ok());
}

#[test]
fn chat_d4_shed_order_holds_human_lane_zero_drops() {
    use myelin_substrate::shed::SurfaceBudget;
    let (ob, minter) = (OutboxStore::new(), Arc::new(MonotonicMinter::new()));
    let mut gw = gateway(MemHotTier::new(), 8192, &["alice"]);
    let alice = gw.connect(&cred("alice")).expect("connect");
    let stream = alice.stream.clone();
    let tenant = alice.principal.tenant.clone();
    let scope = chat_channel_scope(CHANNEL).unwrap();

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

    let storm = 40usize;

    for i in 0..storm {
        let mut d = gw.live_delivery();
        if d.deliver(
            &tenant,
            &stream,
            &scope,
            &LiveFrame::Presence {
                principal: format!("u{i}"),
                class: "online".into(),
            },
        )
        .expect("the bounded frame publishes")
            == DeliveryOutcome::Shed
        {
            presence_shed += 1;
        }
        if d.deliver(
            &tenant,
            &stream,
            &scope,
            &LiveFrame::AgentPartial {
                correlation_id: format!("run-{i}"),
            },
        )
        .expect("the bounded frame publishes")
            == DeliveryOutcome::Shed
        {
            agent_shed += 1;
        }
        match d
            .deliver(
                &tenant,
                &stream,
                &scope,
                &LiveFrame::HumanMessage {
                    message_id: format!("01J0MSG{i}"),
                },
            )
            .expect("the bounded frame publishes")
        {
            DeliveryOutcome::Delivered(_) => human_delivered += 1,
            DeliveryOutcome::Shed => human_shed += 1,
        }
        d.governor_mut()
            .on_drained(&tenant, LiveSurface::HumanMessage);
        durable_send(&gw, &ob, &minter, &format!("h{i}"));
    }

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

#[test]
fn chat_d4_at_scale_deploy_herd_reconnect_completes_for_all_zero_loss() {
    const FLEET: usize = 256;
    const WAVES: usize = 8;
    let members: Vec<String> = (0..FLEET).map(|i| format!("u{i}")).collect();
    let member_refs: Vec<&str> = members.iter().map(|s| s.as_str()).collect();

    let store = MemHotTier::new();
    let mut gw = gateway(store, 16384, &member_refs);

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

    for body in ["m1", "m2"] {
        gw.firehose_mut()
            .publish(&stream, &scope, myelin_events::FrameDraft::new(body))
            .expect("the fixture publishes a valid frame");
    }
    for sub in &subs {
        let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
        assert_eq!(
            seen,
            vec![1, 2],
            "every client saw the prefix before the roll"
        );
    }

    subs.clear();
    let cursor = myelin_chat::MessageId("00000000000000000000000000".into());
    let cohort = FLEET / WAVES;
    let mut next_seq = 3u64;
    let mut resumed = 0usize;

    for wave in 0..WAVES {
        let lo = wave * cohort;
        let hi = lo + cohort;

        let gap_lo = next_seq;
        for k in 0..3 {
            gw.firehose_mut()
                .publish(
                    &stream,
                    &scope,
                    myelin_events::FrameDraft::new(format!("w{wave}f{k}")),
                )
                .expect("the fixture publishes a valid frame");
            next_seq += 1;
        }
        let gap_hi = next_seq - 1;
        let expected_gap: Vec<u64> = (gap_lo..=gap_hi).collect();

        for c in &conns[lo..hi] {
            let outcome = gw
                .resume(c, &conv(), None, gap_lo - 1, &cursor)
                .expect("resume completes");
            match outcome {
                ResumeOutcome::Live { backfill, .. } => {
                    let got: Vec<u64> = backfill.iter().map(|f| f.seq).collect();
                    assert_eq!(
                        got, expected_gap,
                        "0 lost - wave {wave} cohort recovered its full roll gap"
                    );
                    resumed += 1;
                }
                ResumeOutcome::Resync { .. } => panic!(
                    "the window covers the deploy-herd gap - no resync for an in-window cursor (wave {wave})"
                ),
            }
        }
    }

    assert_eq!(
        resumed, FLEET,
        "CHAT-D4-at-scale: resume completes for ALL {FLEET} reconnecting connections across the \
         {WAVES}-wave deploy herd (0 loss)"
    );
    println!(
        "[P-500 CHAT-D4-at-scale GREEN 2026-06-25] deploy-herd: {FLEET} connections rolled in \
         {WAVES} waves, resume completed for all, 0 message loss (bounded reconnect)"
    );
}
