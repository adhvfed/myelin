use myelin_harness::{
    Dependency, DrillContext, DrillRegistry, DrillScenario, Label, LoadGenerator,
    LoadPrincipalKind, Multiplier, Predicate, PrincipalMix, RecordingSink, RunClass, Scope,
    SignalName, Sink, StormProfile, Surface,
};
use myelin_substrate::{
    BoundedSelector, Frame, FrameClass, FrameOutcome, FrameSelector, ScopeWindow,
};
use myelin_tenancy::TenantId;
use std::collections::HashMap;

fn stream_for(surface: Surface) -> &'static str {
    match surface {
        Surface::ConnectionTier => "chat-live",
        Surface::CollabOpStream => "kn-ops",
        other => panic!(
            "the connection-storm firehose drill only drives firehose surfaces, got {other:?}"
        ),
    }
}

fn selector_for(surface: Surface, tenant: &TenantId, conn_id: u64) -> BoundedSelector {
    let raw = match surface {
        Surface::ConnectionTier => format!("channel:{}-{conn_id}", tenant.0),
        Surface::CollabOpStream => format!("doc:{}-{conn_id}", tenant.0),
        other => panic!("non-firehose surface in the firehose drill: {other:?}"),
    };
    BoundedSelector::parse(&raw)
        .expect("the storm bridge always builds a bounded selector, never `*`")
}

fn class_for(run_class: RunClass) -> FrameClass {
    match run_class {
        RunClass::Human => FrameClass::HumanDelivery,
        RunClass::Agent => FrameClass::AgentDelivery,
        RunClass::Service | RunClass::Ci | RunClass::ExternalMcp => FrameClass::Presence,
    }
}

struct StormFirehoseSink {
    connections: HashMap<(String, u64), FrameSelector>,
    is_slow: HashMap<(String, u64), bool>,
    next_seq: HashMap<(String, u64), u64>,
    cap: u32,
    lag_ceiling: u64,
    slow_every: u64,
}

impl StormFirehoseSink {
    fn new(cap: u32, lag_ceiling: u64, slow_every: u64) -> StormFirehoseSink {
        StormFirehoseSink {
            next_seq: HashMap::new(),
            connections: HashMap::new(),
            is_slow: HashMap::new(),
            cap,
            lag_ceiling,
            slow_every: slow_every.max(1),
        }
    }

    fn connection_count(&self) -> usize {
        self.connections.len()
    }

    fn selectors(&self) -> impl Iterator<Item = (&(String, u64), &FrameSelector)> {
        self.connections.iter()
    }

    fn resync_required_count(&self) -> u64 {
        self.connections
            .values()
            .map(|s| s.buffer().resync_required_count())
            .sum()
    }

    fn max_frame_lag(&self) -> u64 {
        self.connections
            .values()
            .map(|s| s.buffer().frame_lag())
            .max()
            .unwrap_or(0)
    }

    fn class_shed_count(&self, class: FrameClass) -> u64 {
        self.connections
            .values()
            .map(|s| s.budget().shed_count(class))
            .sum()
    }

    fn dropped_for_tenant(&self, tenant: &str) -> usize {
        self.connections
            .iter()
            .filter(|((t, _), s)| t == tenant && s.buffer().resync_required())
            .count()
    }
}

impl Sink for StormFirehoseSink {
    fn handle(&mut self, request: &myelin_harness::Request) {
        let conn_id = request.seq / 8;
        let key = (request.tenant.0.clone(), conn_id);

        let cap = self.cap;
        let lag_ceiling = self.lag_ceiling;
        let surface = request.surface;
        let tenant = request.tenant.clone();
        let slow_every = self.slow_every;

        let slow = *self
            .is_slow
            .entry(key.clone())
            .or_insert_with(|| conn_id.is_multiple_of(slow_every));
        let mut seq_cursor = *self.next_seq.entry(key.clone()).or_insert(0);

        let selector = self.connections.entry(key.clone()).or_insert_with(|| {
            let sel = selector_for(surface, &tenant, conn_id);
            let window = ScopeWindow::new(0, 1, u64::MAX);
            FrameSelector::new(stream_for(surface), &sel, cap, lag_ceiling, window)
        });

        let message_class = class_for(request.run_class);
        for f in 0..u64::from(request.frames) {
            let class = if f + 1 == u64::from(request.frames) {
                message_class
            } else {
                FrameClass::Presence
            };
            let frame = Frame::new(seq_cursor, class);
            seq_cursor += 1;
            let outcome = selector.offer(frame, None);
            if !slow && outcome == FrameOutcome::Buffered {
                selector.deliver(frame);
            }
        }
        self.next_seq.insert(key, seq_cursor);
    }
}

fn drive_storm(profile: StormProfile) -> StormFirehoseSink {
    let gen = LoadGenerator::new(
        200,
        Multiplier::SURGE,
        PrincipalMix::agent_skewed(),
        profile,
        vec![TenantId("acme".into()), TenantId("globex".into())],
    )
    .expect("a 30x agent-skewed two-tenant storm is well-specified");

    let mut sink = StormFirehoseSink::new(4, 16, 5);
    gen.drive(&mut sink);
    sink
}

fn sub_d11_connection_storm_scenario() -> DrillScenario {
    DrillScenario::new("sub-d11-connection-storm", |ctx: &mut DrillContext| {
        ctx.breaker
            .break_dependency(Dependency::Firehose, Scope::Tenant(TenantId("acme".into())));

        let connection_storm = drive_storm(StormProfile::connection_storm());
        let collab_op_stream = drive_storm(StormProfile::collab_op_stream());

        assert!(
            connection_storm.connection_count() >= 100,
            "the connection-storm must open a real fleet of connections, got {}",
            connection_storm.connection_count()
        );
        assert!(
            collab_op_stream.connection_count() >= 100,
            "the collab-op-stream must open a real fleet of connections, got {}",
            collab_op_stream.connection_count()
        );

        let max_lag = connection_storm
            .max_frame_lag()
            .max(collab_op_stream.max_frame_lag());
        ctx.signals.set_labelled(
            SignalName::FirehoseFrameLag,
            vec![
                Label::new("stream", "chat-live".to_string()),
                Label::new("scope", "storm-max".to_string()),
            ],
            max_lag as i64,
        );

        let total_resync =
            connection_storm.resync_required_count() + collab_op_stream.resync_required_count();
        ctx.signals
            .set_scalar(SignalName::ResyncRequiredCount, total_resync as i64);

        let presence_shed = connection_storm.class_shed_count(FrameClass::Presence)
            + collab_op_stream.class_shed_count(FrameClass::Presence);
        let human_shed = connection_storm.class_shed_count(FrameClass::HumanDelivery)
            + collab_op_stream.class_shed_count(FrameClass::HumanDelivery);
        ctx.signals.set_labelled(
            SignalName::ShedCount,
            vec![Label::new("lane", "presence".to_string())],
            presence_shed as i64,
        );
        ctx.signals.set_labelled(
            SignalName::ShedCount,
            vec![Label::new("lane", "human".to_string())],
            human_shed as i64,
        );

        assert!(
            max_lag <= 16,
            "firehose_frame_lag must stay BOUNDED (≤ ceiling) under the 30× storm, got {max_lag}"
        );
        assert!(
            total_resync >= 1,
            "the storm's slow consumers must be dropped to resync_required (NAMED, not buffered)"
        );
        assert_eq!(
            human_shed, 0,
            "the protected human/message lane must HOLD - message delivery is shed LAST, never class-shed"
        );

        let globex_dropped = connection_storm.dropped_for_tenant("globex")
            + collab_op_stream.dropped_for_tenant("globex");
        let globex_total = connection_storm
            .selectors()
            .filter(|((t, _), _)| t == "globex")
            .count()
            + collab_op_stream
                .selectors()
                .filter(|((t, _), _)| t == "globex")
                .count();
        assert!(
            globex_dropped < globex_total,
            "globex's keeping-up connections must HOLD ({globex_dropped} dropped of {globex_total}) - \
             a per-connection drop is never tenant-wide"
        );

        ctx.breaker
            .restore_dependency(Dependency::Firehose, Scope::Tenant(TenantId("acme".into())));

        ctx.signals.assert_labelled(
            SignalName::FirehoseFrameLag,
            vec![
                Label::new("stream", "chat-live".to_string()),
                Label::new("scope", "storm-max".to_string()),
            ],
            Predicate::Lte(16),
        )
    })
}

#[test]
fn sub_d11_connection_storm_green_artifact() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(sub_d11_connection_storm_scenario());

    let results = registry.run_all();
    assert_eq!(results.len(), 1);
    let result = &results[0];

    assert!(
        result.is_pass(),
        "P-S31: the firehose bounded/shed half must HOLD under the real 30× connection-storm + \
         collab-op-stream load (frame-lag bounded, slow consumers dropped, human lane holds): {result:?}"
    );

    let row = result.artifact_row("2026-06-22");
    assert_eq!(
        row,
        "[2026-06-22] PASS  drill=sub-d11-connection-storm  (inject → load → assert green)"
    );
    println!("{row}");
}

#[test]
fn sub_d11_connection_storm_all_survival_signals_read_green() {
    let connection_storm = drive_storm(StormProfile::connection_storm());
    let collab_op_stream = drive_storm(StormProfile::collab_op_stream());

    let mut ctx = DrillContext::new();

    let total_resync =
        connection_storm.resync_required_count() + collab_op_stream.resync_required_count();
    ctx.signals
        .set_scalar(SignalName::ResyncRequiredCount, total_resync as i64);
    ctx.signals
        .assert_signal(SignalName::ResyncRequiredCount, Predicate::Gte(1))
        .expect_green();

    let presence_shed = connection_storm.class_shed_count(FrameClass::Presence)
        + collab_op_stream.class_shed_count(FrameClass::Presence);
    ctx.signals.set_labelled(
        SignalName::ShedCount,
        vec![Label::new("lane", "presence".to_string())],
        presence_shed as i64,
    );
    ctx.signals
        .assert_labelled(
            SignalName::ShedCount,
            vec![Label::new("lane", "presence".to_string())],
            Predicate::Gte(1),
        )
        .expect_green();

    let human_shed = connection_storm.class_shed_count(FrameClass::HumanDelivery)
        + collab_op_stream.class_shed_count(FrameClass::HumanDelivery);
    ctx.signals.set_labelled(
        SignalName::ShedCount,
        vec![Label::new("lane", "human".to_string())],
        human_shed as i64,
    );
    ctx.signals
        .assert_labelled(
            SignalName::ShedCount,
            vec![Label::new("lane", "human".to_string())],
            Predicate::Eq(0),
        )
        .expect_green();

    let max_lag = connection_storm
        .max_frame_lag()
        .max(collab_op_stream.max_frame_lag());
    assert!(
        max_lag <= 16,
        "every (stream,scope) frame-lag stays BOUNDED under the storm, got {max_lag}"
    );
}

#[test]
fn the_drill_drives_the_real_storm_profiles_at_surge_scale() {
    assert_eq!(
        StormProfile::connection_storm().surface(),
        Surface::ConnectionTier
    );
    assert_eq!(
        StormProfile::collab_op_stream().surface(),
        Surface::CollabOpStream
    );

    let gen = LoadGenerator::new(
        200,
        Multiplier::SURGE,
        PrincipalMix::agent_skewed(),
        StormProfile::connection_storm(),
        vec![TenantId("acme".into()), TenantId("globex".into())],
    )
    .unwrap();
    assert_eq!(gen.total_requests(), 6000, "30× surge of base 200");
    let mix = gen.planned_mix();
    let human = mix[LoadPrincipalKind::ALL
        .iter()
        .position(|k| *k == LoadPrincipalKind::Human)
        .unwrap()];
    let agent = mix[LoadPrincipalKind::ALL
        .iter()
        .position(|k| *k == LoadPrincipalKind::Agent)
        .unwrap()];
    assert!(
        human > 0,
        "the protected human lane must carry storm traffic"
    );
    assert!(
        agent > human * 3,
        "the storm must be agent-skewed (the surge)"
    );

    let mut sink = StormFirehoseSink::new(4, 16, 5);
    gen.drive(&mut sink);
    assert!(
        sink.connection_count() >= 100,
        "the surge-scale storm opens a real fleet of firehose connections"
    );
    let mut rec = RecordingSink::default();
    gen.drive(&mut rec);
    assert_eq!(
        rec.received.len(),
        6000,
        "the storm issues exactly 30× the base, no request lost"
    );
}
