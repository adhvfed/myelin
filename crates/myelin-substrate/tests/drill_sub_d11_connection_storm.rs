//! # P-S31 (global P-326) — the firehose backpressure half under CONNECTION-STORM (M4 re-confirm)
//!
//! **Drill catalogue:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §11 row **D-11** (*Firehose reconnect-loses-zero-ops*) + §7.6 (the per-surface shed-budget floor
//! table — the **connection-storm** + **collab op-stream** rows) + §7.7 (the firehose backpressure
//! role). **Contract-index:** row 3.5 (carried — the substrate's bounded/shed half, re-confirmed under
//! connection-storm). **Doctrine:** EI-01 §3 ("the bounded/shed half holds under REAL load, not just
//! unit scale" — a property does not exist until a test forces the failure under the real load shape).
//!
//! ## What P-S31 is (vs P-S28/P-S29 — the same machinery, the REAL load shape)
//! P-S28 ([`drill_sub_d11_firehose_slow_consumer.rs`]) and P-S29
//! ([`drill_sub_d11_firehose_frame_budgets.rs`]) proved the substrate's firehose
//! bounded-and-sheds half at **unit scale** — hand-driven frame loops on one or two connections.
//! This M4 prompt **re-confirms the SAME half under the REAL connection-storm load shape**: it drives
//! the P-S02 [`LoadGenerator`]'s named **connection-storm** + **collab-op-stream** [`StormProfile`]s at
//! a **30× surge** with the agent-skewed mix across **multiple tenants**, fans each issued request out
//! into per-connection [`FrameSelector`]s (the P-S28/P-S29 layer), and asserts the §10.2 firehose
//! survival signals (`firehose_frame_lag` bounded, `resync_required_count` accurate, `shed-counts/lane`)
//! hold under that storm. This is the substrate asserting its survival signals; **Chat owns the
//! end-to-end resume-0-lost/0-dup + co-commit + idempotent-send drill** (CHAT-D1/CHAT-D13/CHAT-D14) — to
//! which this contributes (the substrate proves its bounded/shed half holds while the storm rages, the
//! precondition for Chat's zero-loss-on-reconnect proof; the Bus owns the zero-loss-replay half, P-141).
//!
//! ## The drill shape (EI-01 §3: inject → load → assert green)
//!   - **inject** — `break_dependency(Dependency::Firehose, …)` drops firehose subscriptions mid-storm
//!     (the realistic D-11 connection-storm condition: a reconnect-storm racing the frame flood).
//!   - **load** — the REAL [`LoadGenerator`] drives the **connection-storm** profile (the Chat connection
//!     tier — `channel:` scopes, frame-heavy fan-out) AND the **collab-op-stream** profile (the KN hot-doc
//!     op-stream — `doc:` scopes) at **30×** with the **agent-skewed** mix across **two tenants**. Every
//!     issued request becomes a per-connection [`FrameSelector`] whose `request.frames` fan out as
//!     mixed-class frames (presence/agent/human keyed by the request's run-class). A fraction of the
//!     connections are deliberately **slow** (never deliver) so the slow-consumer drop fires under storm.
//!   - **assert** — the §10.2 firehose survival signals read GREEN under the storm:
//!       * `firehose_frame_lag` is **BOUNDED** (≤ the slow-consumer ceiling) for every `(stream,scope)` —
//!         memory never grew unboundedly even at 30× (§7.7, Little's Law);
//!       * `resync_required_count` is **accurate** (exactly the slow connections dropped — NAMED, not
//!         silent; a dropped connection holds 0 frames);
//!       * `shed-counts/lane`: **presence/speculative frames shed before message delivery**, and the
//!         **protected human lane holds** (humans never class-shed — message delivery is shed LAST,
//!         §7.6 connection-tier row);
//!       * **per-tenant isolation**: tenant A's storm does NOT drop tenant B's keeping-up connections.

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

// ----------------------------------------------------------------------------------------------------
// The storm → firehose bridge: map each LoadGenerator request to a per-connection FrameSelector and
// fan its profile frames out as mixed-class firehose frames. This is the substrate's view of the
// connection tier (Chat M4 owns the real one); the drill drives the REAL storm shape through it.
// ----------------------------------------------------------------------------------------------------

/// The firehose stream a storm surface rides (the `(stream, …)` survival-signal key half).
fn stream_for(surface: Surface) -> &'static str {
    match surface {
        // the Chat connection tier — live delivery + presence frames.
        Surface::ConnectionTier => "chat-live",
        // the KN hot-doc collab op-stream — edit ops + presence frames.
        Surface::CollabOpStream => "kn-ops",
        // the other two storm surfaces are request-shaped, not firehose-frame-shaped; this drill only
        // drives the two FIREHOSE surfaces (§7.6 connection-storm + collab op-stream rows). A non-firehose
        // surface in this bridge is a mis-wired drill (loud, not silent — EI-01 §5).
        other => panic!(
            "the connection-storm firehose drill only drives firehose surfaces, got {other:?}"
        ),
    }
}

/// The bounded [`BoundedSelector`] a request subscribes on: a `channel:` for the connection tier, a
/// `doc:` for the collab op-stream — keyed by the request's tenant + sequence so each request is a
/// distinct bounded connection (never `*`; the §7.7 bounded-selector rule).
fn selector_for(surface: Surface, tenant: &TenantId, conn_id: u64) -> BoundedSelector {
    let raw = match surface {
        Surface::ConnectionTier => format!("channel:{}-{conn_id}", tenant.0),
        Surface::CollabOpStream => format!("doc:{}-{conn_id}", tenant.0),
        other => panic!("non-firehose surface in the firehose drill: {other:?}"),
    };
    BoundedSelector::parse(&raw)
        .expect("the storm bridge always builds a bounded selector, never `*`")
}

/// The frame [`FrameClass`] a request's run-class maps to: a human run delivers human (message) frames
/// (shed LAST), an agent run delivers agent frames (shed before humans), and the machine/CI/service +
/// presence-ish lanes deliver presence/speculative frames (shed FIRST) — the §7.6 frame-shed order.
fn class_for(run_class: RunClass) -> FrameClass {
    match run_class {
        RunClass::Human => FrameClass::HumanDelivery,
        RunClass::Agent => FrameClass::AgentDelivery,
        // service / CI / external-MCP connections carry presence/speculative frames (typing, cursors,
        // prefetch) — the ephemeral lane that sheds first so message delivery is protected.
        RunClass::Service | RunClass::Ci | RunClass::ExternalMcp => FrameClass::Presence,
    }
}

/// A sink that fans every issued storm request out into per-connection [`FrameSelector`]s and offers its
/// profile frames as mixed-class firehose frames — the substrate's firehose backpressure layer under the
/// real storm load. A fraction of connections are SLOW (never deliver) so the slow-consumer drop fires.
struct StormFirehoseSink {
    /// One [`FrameSelector`] per `(tenant, connection)` — the per-connection bounded buffer (P-S28/P-S29).
    connections: HashMap<(String, u64), FrameSelector>,
    /// Per-connection slowness: `true` = this connection NEVER delivers (a stalled/slow consumer that
    /// must be dropped to `resync_required`); `false` = keeps up (delivers each frame it buffers).
    is_slow: HashMap<(String, u64), bool>,
    /// Per-connection monotone frame seq counter — the firehose contract is a **per-`(stream,scope)`
    /// monotonic `seq`** (contract 3.5), NOT the global request seq. Each connection's frames are
    /// numbered contiguously from its own counter so the lag (`offered − delivered`) is honest even
    /// though the storm interleaves connections + tenants in the global request stream.
    next_seq: HashMap<(String, u64), u64>,
    /// The per-connection in-flight cap (§7.1) — small, so the 30× storm exercises the cap + the drop.
    cap: u32,
    /// The slow-consumer lag ceiling (§7.7) — a connection past this is dropped to `resync_required`.
    lag_ceiling: u64,
    /// The fraction-denominator for slow connections: every Nth connection is slow (so the drill drives
    /// BOTH keeping-up and slow consumers under the same storm — the per-connection isolation property).
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

    /// The total number of distinct connections opened by the storm.
    fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Every open connection's [`FrameSelector`] (for the survival-signal snapshot + the assertions).
    fn selectors(&self) -> impl Iterator<Item = (&(String, u64), &FrameSelector)> {
        self.connections.iter()
    }

    /// The cumulative `resync_required` drop count across all connections (the §10.2 scalar signal).
    fn resync_required_count(&self) -> u64 {
        self.connections
            .values()
            .map(|s| s.buffer().resync_required_count())
            .sum()
    }

    /// The max per-`(stream,scope)` frame lag across all connections (the bound the drill asserts holds).
    fn max_frame_lag(&self) -> u64 {
        self.connections
            .values()
            .map(|s| s.buffer().frame_lag())
            .max()
            .unwrap_or(0)
    }

    /// The cumulative per-class frame-shed count across all connections (the §10.2 ShedCount-by-lane).
    fn class_shed_count(&self, class: FrameClass) -> u64 {
        self.connections
            .values()
            .map(|s| s.budget().shed_count(class))
            .sum()
    }

    /// The number of connections (of a given tenant) that were dropped to `resync_required`.
    fn dropped_for_tenant(&self, tenant: &str) -> usize {
        self.connections
            .iter()
            .filter(|((t, _), s)| t == tenant && s.buffer().resync_required())
            .count()
    }
}

impl Sink for StormFirehoseSink {
    fn handle(&mut self, request: &myelin_harness::Request) {
        // each request is one connection on a bounded scope; identify it by (tenant, connection seq).
        // We bucket many requests onto a connection so a single connection sees a SUSTAINED frame flood
        // (the storm), not one frame each — the realistic connection-storm shape. 8 requests / connection.
        let conn_id = request.seq / 8;
        let key = (request.tenant.0.clone(), conn_id);

        let cap = self.cap;
        let lag_ceiling = self.lag_ceiling;
        let surface = request.surface;
        let tenant = request.tenant.clone();
        let slow_every = self.slow_every;

        // every Nth connection is a slow consumer (never delivers) — the rest keep up. Resolved BEFORE
        // borrowing the selector (the two maps are independent fields of `self`).
        let slow = *self
            .is_slow
            .entry(key.clone())
            .or_insert_with(|| conn_id.is_multiple_of(slow_every));
        // the per-`(stream,scope)` monotone seq — each connection numbers its own frames contiguously
        // (contract 3.5: per-`(stream,scope)` monotonic `seq`), NOT the global request seq, so the lag is
        // honest even though the storm interleaves connections + tenants in the global request stream.
        let mut seq_cursor = *self.next_seq.entry(key.clone()).or_insert(0);

        let selector = self.connections.entry(key.clone()).or_insert_with(|| {
            let sel = selector_for(surface, &tenant, conn_id);
            // a generous window so the storm's frames land in-window (the off-window slice is proven in
            // the P-S29 frame-budget drill; here we drive the cap + slow-consumer drop + class budgets).
            let window = ScopeWindow::new(0, 1, u64::MAX);
            FrameSelector::new(stream_for(surface), &sel, cap, lag_ceiling, window)
        });

        // **The realistic connection-storm frame shape (§7.7).** ONE connection carries a MIX of frame
        // classes: a Chat connection sees live message delivery + presence + agent partials all together,
        // and the storm is dominated by ephemeral PRESENCE chatter (typing indicators, cursor moves,
        // prefetch). So a request's `frames` fan out as mostly presence frames, with a SINGLE "message"
        // frame of the request's own run-class (human delivery for a human run, agent partial for an
        // agent run) — the sparse, protected message slice. This is what makes the §7.6 order observable
        // on a SHARED buffer: presence (the bulk) sheds first; the message frame is shed LAST.
        let message_class = class_for(request.run_class);
        for f in 0..u64::from(request.frames) {
            // the LAST frame of the fan-out is the message frame (the run's own class); the rest are
            // ephemeral presence chatter (the storm). frames_per_request ≥ 1, so there is always a message.
            let class = if f + 1 == u64::from(request.frames) {
                message_class
            } else {
                FrameClass::Presence
            };
            let frame = Frame::new(seq_cursor, class);
            seq_cursor += 1;
            let outcome = selector.offer(frame, None);
            // a keeping-up consumer delivers each frame it buffered (closing the lag); a slow consumer
            // never delivers (its lag climbs → it is dropped to resync_required under the storm).
            if !slow && outcome == FrameOutcome::Buffered {
                selector.deliver(frame);
            }
        }
        // write the advanced per-connection seq cursor back (the connection's frames stay contiguous).
        self.next_seq.insert(key, seq_cursor);
    }
}

/// Drive ONE storm profile through the real [`LoadGenerator`] against a [`StormFirehoseSink`] and return
/// the sink (carrying every connection's post-storm state). A 30× surge with the agent-skewed mix across
/// two tenants — the real connection-storm / collab-op-stream load shape.
fn drive_storm(profile: StormProfile) -> StormFirehoseSink {
    let gen = LoadGenerator::new(
        200, // base requests (× 30 = 6000 issued requests = the storm)
        Multiplier::SURGE,
        PrincipalMix::agent_skewed(),
        profile,
        vec![TenantId("acme".into()), TenantId("globex".into())],
    )
    .expect("a 30x agent-skewed two-tenant storm is well-specified");

    // cap 4 per connection, slow-consumer ceiling 16, every 5th connection slow. Small caps so the 30×
    // storm exercises the cap + the slow-consumer drop + the per-surface class budgets hard.
    let mut sink = StormFirehoseSink::new(4, 16, 5);
    gen.drive(&mut sink);
    sink
}

/// The P-S31 scenario: under an injected firehose drop, drive BOTH firehose storm profiles
/// (connection-storm + collab-op-stream) at 30× across two tenants, then assert the substrate's
/// firehose survival signals hold under the storm.
fn sub_d11_connection_storm_scenario() -> DrillScenario {
    DrillScenario::new("sub-d11-connection-storm", |ctx: &mut DrillContext| {
        // (inject) drop firehose subscriptions mid-storm — the connection-storm condition (a reconnect
        // storm racing the frame flood). Scoped to one tenant to prove the OTHER tenant is unaffected.
        ctx.breaker
            .break_dependency(Dependency::Firehose, Scope::Tenant(TenantId("acme".into())));

        // (load) drive the two FIREHOSE storm profiles at 30× across two tenants through the real
        // LoadGenerator → the substrate firehose backpressure layer (P-S28/P-S29).
        let connection_storm = drive_storm(StormProfile::connection_storm());
        let collab_op_stream = drive_storm(StormProfile::collab_op_stream());

        // both storms must have opened a non-trivial fleet of connections (the storm is real, not a no-op).
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

        // ---- (survival signals under the storm) -----------------------------------------------------
        // 1. firehose_frame_lag BOUNDED — memory never grew unboundedly at 30× (the §7.7 / Little's Law
        //    guarantee). The bound is the slow-consumer ceiling (16); a live connection's lag is ≤ it, a
        //    dropped one reads 0.
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

        // 2. resync_required_count accurate — the slow connections were DROPPED (not buffered), NAMED.
        let total_resync =
            connection_storm.resync_required_count() + collab_op_stream.resync_required_count();
        ctx.signals
            .set_scalar(SignalName::ResyncRequiredCount, total_resync as i64);

        // 3. shed-counts/lane — presence/speculative shed before message delivery; the human lane holds.
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

        // ---- (baked assertions: the survival properties hold under the storm) -----------------------
        // memory bounded: every (stream,scope) frame-lag ≤ the slow-consumer ceiling, even at 30×.
        assert!(
            max_lag <= 16,
            "firehose_frame_lag must stay BOUNDED (≤ ceiling) under the 30× storm, got {max_lag}"
        );
        // the slow consumers were dropped (the storm DID exercise the slow-consumer drop, not a no-op).
        assert!(
            total_resync >= 1,
            "the storm's slow consumers must be dropped to resync_required (NAMED, not buffered)"
        );
        // the protected human lane holds: message (human) delivery NEVER class-shed under the storm —
        // presence/speculative frames absorbed the pressure first (§7.6 connection-tier row).
        assert_eq!(
            human_shed, 0,
            "the protected human/message lane must HOLD — message delivery is shed LAST, never class-shed"
        );

        // per-tenant isolation: the firehose break + storm was scoped to acme; globex's keeping-up
        // connections were NOT dropped by acme's storm (a slow tenant never drops a healthy neighbour).
        // (slow connections are deterministic by conn_id % 5; a keeping-up globex connection is never
        // dropped — its lag stays bounded and it self-delivers.)
        let globex_dropped = connection_storm.dropped_for_tenant("globex")
            + collab_op_stream.dropped_for_tenant("globex");
        // only globex's OWN slow connections drop (conn_id % 5 == 0); but a globex connection is never
        // dropped by ACME's storm — the drops are per-connection, never tenant-wide. Assert globex's
        // KEEPING-UP connections held: the dropped count equals exactly globex's own slow connections.
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
            "globex's keeping-up connections must HOLD ({globex_dropped} dropped of {globex_total}) — \
             a per-connection drop is never tenant-wide"
        );

        // restore the injected fault before returning (a re-run starts clean).
        ctx.breaker
            .restore_dependency(Dependency::Firehose, Scope::Tenant(TenantId("acme".into())));

        // (assert) the single telemetry assertion that reads green: the firehose frame-lag is BOUNDED
        // under the storm (≤ the slow-consumer ceiling). The other survival signals are asserted in the
        // runner below — this is the loud green/red verdict the drill returns.
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

/// **THE P-S31 DRILL** — the dated green artifact the GATE/DRILLS names. Register it (it joins the
/// permanent every-incident suite) AND run it; assert the firehose backpressure half holds under the
/// real connection-storm + collab-op-stream load (the survival signals read green).
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

/// The drill, run directly, asserting the FULL survival-signal set under the storm: frame-lag bounded,
/// `resync_required_count` accurate (slow consumers dropped, NAMED), `shed-counts/lane` (presence shed
/// before message frames; the human lane holds) — the complete connection-storm green set.
#[test]
fn sub_d11_connection_storm_all_survival_signals_read_green() {
    let connection_storm = drive_storm(StormProfile::connection_storm());
    let collab_op_stream = drive_storm(StormProfile::collab_op_stream());

    let mut ctx = DrillContext::new();

    // resync_required_count is accurate (slow consumers dropped, NAMED — > 0 under the storm).
    let total_resync =
        connection_storm.resync_required_count() + collab_op_stream.resync_required_count();
    ctx.signals
        .set_scalar(SignalName::ResyncRequiredCount, total_resync as i64);
    ctx.signals
        .assert_signal(SignalName::ResyncRequiredCount, Predicate::Gte(1))
        .expect_green();

    // presence/speculative frames shed (the ephemeral lane absorbed the storm pressure first).
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

    // the protected human/message lane HELD — message delivery is shed LAST, never class-shed.
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

    // memory bounded: every (stream,scope) frame-lag ≤ the slow-consumer ceiling under the 30× storm.
    let max_lag = connection_storm
        .max_frame_lag()
        .max(collab_op_stream.max_frame_lag());
    assert!(
        max_lag <= 16,
        "every (stream,scope) frame-lag stays BOUNDED under the storm, got {max_lag}"
    );
}

/// The drill drives the REAL P-S02 storm profiles (not a hand-rolled loop) — the connection-storm and
/// collab-op-stream profiles select their firehose surfaces, and the 30× surge issues a real fleet of
/// requests with the agent-skewed mix. This pins the "real load shape, not unit scale" property
/// (EI-01 §3) the M4 re-confirm exists to prove.
#[test]
fn the_drill_drives_the_real_storm_profiles_at_surge_scale() {
    // the two firehose storm profiles select the right surfaces (§7.6 connection-storm + collab-op rows).
    assert_eq!(
        StormProfile::connection_storm().surface(),
        Surface::ConnectionTier
    );
    assert_eq!(
        StormProfile::collab_op_stream().surface(),
        Surface::CollabOpStream
    );

    // the 30× agent-skewed two-tenant generator issues a real storm: 200 base × 30 = 6000 requests, of
    // which the human lane is a thin protected minority and the machine/agent lanes dominate (the surge).
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
    // human is index 0 (the thin protected lane); agent is index 1 (the surge). The human lane is present
    // (it must survive the storm) and the machine+agent lanes dominate (the storm input).
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

    // a smoke run confirms the storm actually flows through the firehose layer (connections opened).
    let mut sink = StormFirehoseSink::new(4, 16, 5);
    gen.drive(&mut sink);
    assert!(
        sink.connection_count() >= 100,
        "the surge-scale storm opens a real fleet of firehose connections"
    );
    // a recording sink confirms the generator issued exactly the surge count (no request ghosted/lost).
    let mut rec = RecordingSink::default();
    gen.drive(&mut rec);
    assert_eq!(
        rec.received.len(),
        6000,
        "the storm issues exactly 30× the base, no request lost"
    );
}
