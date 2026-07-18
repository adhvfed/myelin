//! # `app` — the myelin-flow AppSpec service shell (P-FLOW-02 → P-198, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §1 (the engine's
//! responsibilities), §10 (no second emit path; the engine boots from `serve(AppSpec)`), §2
//! (BUILD/DBOS-class, Postgres-embedded — NO new datastore).
//!
//! **Contract-index:** consumes 1.1 `serve(AppSpec)` (boot → migrate → outbox-relay → three ports
//! → graceful drain), 1.2 the three ports, 1.3 liveness ≠ readiness, 2.2–2.5 the transactional
//! outbox (relay wired; the consumer slot is EMPTY here). Owns no new contract.
//!
//! ## What this prompt (P-FLOW-02) ships — the runnable SHELL
//!
//! [`flow_app_spec`] assembles the [`AppSpec`](myelin_substrate::AppSpec) the harness wires:
//!   - **boot** → validate the env-first config (fail fast, §3.2);
//!   - **migrate** → run the P-FLOW-01 six-table forward-only migration set ([`crate::migrations`]);
//!   - **relay** → start the transactional outbox relay (BUS-2 — the ONLY emit path; §10, there is
//!     no second publish path the workflow engine may use);
//!   - **consumers** → the registration slot is EMPTY for now (the deterministic-replay engine + the
//!     signal/timer consumers land in P-FLOW-04..05 / P-FLOW-09 / P-FLOW-13);
//!   - **ports** → the harness opens the three surfaces (public / internal-RPC / metrics-health)
//!     with **liveness ≠ readiness** (a booting instance is not-ready-but-not-killed; readiness
//!     lifts only after migrate completes) + graceful drain.
//!
//! This is the runnable shell the later engine prompts hang code on — NO algorithms. The
//! `consumers` slot is the named empty seam; the holder auto-registration (the references-not-
//! payloads `PersonalDataHolder` over `workflow_run`/`wf_history`/`wf_signal`) lands at P-FLOW-03
//! (P-201) — at the shell, every opened store auto-registers via the harness's one door (§3.4).
//!
//! ## Why this lives here and not in a `*-service` crate (coherence, EI-01 §7)
//! Unlike `myelin-identity` (glue) ↔ `myelin-identity-service` (impl), the durable-workflow ledger
//! ships ONE crate, `myelin-flow` (the schema crate P-FLOW-01 + this shell + the engine
//! P-FLOW-04..): the architecture (§2, ADR-09) is explicit that `myelin-flow` is a single
//! Postgres-embedded engine crate, not a glue/impl split. So the bootable shell is a module of the
//! SAME crate, like the schema. `myelin-flow` remains a NAMED leaf consumer above the glue crates
//! (it gains a dep on `serve(AppSpec)` via `myelin-substrate`, already its migration-framework dep)
//! and is NOT a node in the eleven-crate library DAG (`substrate_is_root()`/`identity_is_sink()`
//! preserved — a subsystem service is the graph's terminal consumer).

use crate::migrations::migrations as flow_migrations;
use myelin_events::OutboxStore;
use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, HotTables, InternalRpc, Migrations,
    OutboxSpec, PublicRoutes, ServeError, ServeHandle, StoreManifest,
};

/// The service name — the PII-free telemetry/trace service identifier the harness labels traces
/// and the contract-1.8 signal set with.
pub const SERVICE_NAME: &str = "myelin-flow";

/// The myelin-flow forward-only migration set (contract 1.5) the boot lifecycle runs migrate → ready
/// over: the P-FLOW-01 six-table data model (`workflow_run` / `wf_history` / `wf_timer` / `wf_signal`
/// / `wf_activity_attempt` / `wf_definition`), each `(tenant, region)`-first + RLS-scoped (the five
/// tenant tables) — see [`crate::migrations`]. The harness prepends the substrate-co-located `outbox`
/// + `consumer_dedup` tables, so the relay's storage exists by the time migrate completes.
fn flow_service_migrations() -> Migrations {
    flow_migrations()
}

/// **Assemble the myelin-flow service [`AppSpec`] (contract 1.1; architecture §1/§10) the harness
/// wires.** The harness owns the lifecycle around it: boot → migrate (the P-FLOW-01 six tables) →
/// outbox relay (the ONLY emit path, BUS-2) → consumers (the EMPTY seam) → three ports (public /
/// internal-RPC / metrics-health, liveness ≠ readiness) → graceful drain.
///
/// `config` is the validated, env-first config (§3.2; validated at boot, fail fast). The flow OLTP
/// store is implicitly critical (the harness adds it — the engine cannot serve correct traffic
/// without its own Postgres); the engine declares no further critical downstream at the SHELL (its
/// consumer call-sites — the signal/timer consumers, the replay loop — land with the engine
/// prompts, P-FLOW-04..). A healthy boot is ready once the migrations apply.
///
/// **Floors wired as empty seams (named):**
/// - the `consumers` slot is EMPTY — the deterministic-replay engine consumer + the signal/timer
///   consumers are P-FLOW-04..05 / P-FLOW-09 / P-FLOW-13. This shell has NO workflow executor; an
///   empty consumer slot is not a working engine.
/// - holder auto-registration: every opened store auto-registers as a `PersonalDataHolder` (§3.4,
///   GD-3) through the harness's one door; the references-not-payloads flow store-holder over
///   `workflow_run`/`wf_history`/`wf_signal` lands at P-FLOW-03 (P-201).
///
/// **The outbox is INJECTED (MR-009b W3b.4 — the composition root owns durability):** the
/// production `main.rs` constructs `OutboxStore::durable(PgOutboxBacking)` over the MR-022
/// `SubstrateProvider` pool (foundation migrations applied, fail-loud on missing durable config);
/// a test/drill passes the in-memory `OutboxStore::new()` double. This builder constructs NO
/// store of its own — the W3 dedup-injection precedent applied to the outbox.
pub fn flow_app_spec(config: Config, outbox: OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        // The P-FLOW-01 six-table data model (the migrate phase the boot lifecycle runs → ready).
        migrations: flow_service_migrations(),
        // workflow_run is the engine's high-write control table. Declaring it hot makes every
        // follow-on pass the online expand/backfill/contract gate; its initial CREATE remains safe.
        hot_tables: HotTables::declare(["workflow_run"]),
        // The public surface (gateway-fronted, tenant-from-token) — the durable-execution API route
        // bodies are the engine prompts. The harness opens the live tenant-from-token surface.
        public: PublicRoutes::default(),
        // The internal-RPC surface — `myelin-flow` exposes the durable-execution API to sibling
        // subsystems (agent-fabric, CI, issues) on this surface; the bodies are the follow-ons.
        internal: InternalRpc::default(),
        // No consumers yet — the replay engine + the signal/timer consumers are P-FLOW-04..05/09/13.
        consumers: Vec::new(),
        // Every opened store auto-registers as a PersonalDataHolder (§3.4, GD-3). The flow OLTP store
        // auto-registers at boot; the references-not-payloads store-holder over
        // workflow_run/wf_history/wf_signal lands at P-FLOW-03 (P-201).
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        // Producer-only shared durable outbox. The elected cell relay is the only publisher.
        outbox: OutboxSpec::external_relay(outbox),
        // The engine declares no further critical downstream at the shell; the OLTP store is implicit.
        critical: CriticalDependencies::default(),
    }
}

/// **Boot the myelin-flow service shell under the harness (contract 1.1)** up to the pre-serve
/// state, returning the [`ServeHandle`] the lifecycle drives. A thin wrapper over
/// [`boot`](myelin_substrate::boot) of [`flow_app_spec`] — separated so a test/drill can boot,
/// inspect the three ports + the liveness ≠ readiness state, drive ticks, and drive the graceful
/// drain deterministically.
///
/// Returns `Err` (the non-zero exit) on a failed boot (§3.1) — loud, never a silent success.
pub fn boot_flow(config: Config, outbox: OutboxStore) -> Result<ServeHandle, ServeError> {
    boot(flow_app_spec(config, outbox))
}

/// **Run the myelin-flow service to completion under the harness** (boot → migrate → relay →
/// consumers → three ports → graceful drain). The `myelin-flow` binary calls this; a failed boot /
/// incomplete drain returns `Err` (the non-zero process exit, §3.1).
pub fn run_flow(config: Config, outbox: OutboxStore) -> Result<(), ServeError> {
    serve(flow_app_spec(config, outbox))
}

/// **Build the myelin-flow AppSpec WITH the replay/lease engine wired into the consumer seam
/// (P-FLOW-05).** Returns the spec the harness wires PLUS the [`FlowDispatcher`] worker loop —
/// the engine's per-partition worker that leases a runnable run and drives it (replaying the
/// journal, resuming at the first un-journaled command, §4.1/§4.7). The dispatcher emits into — and
/// the relay drains — the SAME [`OutboxStore`] (the ONE sanctioned outbox path, BUS-2/2.2), so the
/// drive's co-commits flow through the sanctioned emit → relay → bus path.
///
/// **Why this returns the dispatcher instead of pushing a `ConsumerReg`:** the replay/lease engine
/// is a tick-driven WORKER that polls the run store for leasable work (the `FOR UPDATE SKIP LOCKED`
/// claim, §4.7) — NOT a bus-subscriber `EventHandler` the `consumers` slot holds. So the engine
/// occupies the consumer SEAM as the returned dispatcher; the harness drives its `tick` on the
/// worker cadence. The DurableExecutor surface that SEEDS runnable runs (`start`) + the signal/timer
/// wakers (that flip `waiting` → `running`) land at P-FLOW-06/09/13 — this wires the loop + the core.
///
/// **Floor named:** the workflow-body registry the dispatcher drives is supplied by the caller here
/// (the engine/test registers bodies via [`FlowDispatcher::register`]); the boot-time definition
/// registry (§3.6, populated by `DurableExecutor`) lands at **P-FLOW-06**.
pub fn flow_app_spec_with_engine(
    config: Config,
    outbox: OutboxStore,
    minter: std::sync::Arc<dyn myelin_events::IdMinter>,
    ctx_base: myelin_events::EmitContextBase,
    partition: i16,
    worker: impl Into<String>,
    lease_ttl_secs: i64,
) -> (
    AppSpec,
    crate::engine::FlowDispatcher,
    crate::timer::TimerWheel,
) {
    // The ONE outbox the engine drives co-commit into AND the relay drains (BUS-2) is INJECTED
    // (MR-009b W3b.4): the production root passes a durable-backed store (and a UNIQUE id source —
    // never the per-store-resetting default `MonotonicMinter`, whose colliding event_ids the durable
    // `ON CONFLICT (event_id) DO NOTHING` silently drops; `myelin_events::UlidMinter` is the
    // production source); a test passes the in-memory double + a seeded deterministic minter.
    let runs = crate::engine::RunStore::new();
    let journal = crate::wfctx::WfJournal::new();
    let telemetry = crate::engine::FlowTelemetry::new();
    // The ONE durable-timer wheel store BOTH the dispatcher arms into (a body `sleep` → a `wf_timer`
    // row) AND the timer-wheel worker scans (the SC-11 minute-bucket wheel, §4.2). One substrate.
    let timers = crate::timer::TimerStore::new();
    let dispatcher = crate::engine::FlowDispatcher::new(
        runs.clone(),
        outbox.clone(),
        journal.clone(),
        telemetry.clone(),
        minter,
        ctx_base,
        partition,
        worker,
        lease_ttl_secs,
    )
    .with_timers(timers.clone());
    // The timer-wheel worker for THIS partition — the scan loop wired into the consumer seam alongside
    // the dispatcher (P-FLOW-13, §4.2). Each wheel tick scans the due bucket (`bucket <= now AND NOT
    // fired`, far-future untouched), fires each due timer effectively-once (set fired + journal
    // `timer_fired` + wake the run), and refreshes the timer-wheel-lag telemetry (the SC-11 health
    // signal). The harness drives its tick on the wheel cadence (jittered ~1s, §4.2). The bounded
    // per-tick fire LIMIT is generous (a burst drains over a few ticks; the lag signal tracks it).
    let wheel = crate::timer::TimerWheel::new(
        timers, journal, runs, telemetry, partition, /* batch */ 4_096,
    );
    // The engine drives co-commit into `outbox`; the relay drains the SAME injected store (the
    // sanctioned emit → relay → bus path — one store, one truth).
    let spec = flow_app_spec(config, outbox);
    (spec, dispatcher, wheel)
}

/// **Build the inbound-signal `ConsumerReg` for `tenant` — the bus side of `DurableExecutor::signal`
/// wired into the P-FLOW-02 `consumers` slot (P-FLOW-09).** Constructs the
/// [`FlowSignalConsumer`](crate::FlowSignalConsumer) over `executor` and wraps it in the seven-rule
/// [`Consumer`](myelin_events::Consumer) runtime through the SANCTIONED
/// [`consume`](myelin_events::consume) — binding the `sig.<tenant>.` whitelist (rule 3: `consume`
/// REJECTS a `*`/`>`/empty subject loudly), the durable consumer name (rule 4), the bounded prefetch
/// plus per-tenant fairness cap (rule 6), and the shared `dedup` ledger (rule 1: `event_id`
/// idempotency, belt-and-braces with the `wf_signal` PK INSIDE `signal`).
///
/// Returns the [`ConsumerReg`](myelin_substrate::ConsumerReg) the `serve` lifecycle registers in the
/// AppSpec `consumers` slot — the registration that FILLS the P-FLOW-02 empty signal seam. An
/// over-broad / malformed tenant prefix returns [`SubscribeError`](myelin_events::SubscribeError) —
/// the shell never silently narrows to an over-broad subscription.
pub fn flow_signal_consumer_reg(
    tenant: &myelin_tenancy::TenantId,
    executor: crate::FlowExecutor,
    dedup: myelin_events::DedupLedger,
) -> Result<myelin_substrate::ConsumerReg, myelin_events::SubscribeError> {
    use myelin_events::{consume, ConsumerName, ConsumerSpec, SubjectPattern};
    // The `sig.<tenant>.` subject whitelist (NEVER `*`, BUS-3). Validated + leaked to `'static` once
    // per tenant per process (the binding set is fixed for the consumer pool's life — bounded).
    let prefix = format!("sig.{}.", tenant.0);
    let subjects: &'static [SubjectPattern] =
        Box::leak(vec![SubjectPattern(prefix.clone())].into_boxed_slice());
    let consumer = crate::FlowSignalConsumer::new(executor, subjects);
    // The ONE sanctioned consumer entry-point — `consume` validates the spec (rule 3: rejects a
    // `*`/empty subject LOUDLY) and constructs the [`Consumer`] with all seven rules wired.
    let runtime = consume(
        ConsumerSpec::new(
            ConsumerName(format!("flow-signal-{}", tenant.0)),
            &[prefix.as_str()],
        ),
        consumer,
        dedup,
    )?;
    Ok(myelin_substrate::ConsumerReg::new(runtime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{
        HealthTable, Liveness, MetricsHealthSurface, Readiness, Startup, Surface,
    };

    /// **The shell boots under the harness and the three ports bind (contracts 1.1/1.2).** The flow
    /// AppSpec runs the boot → migrate → relay → ports lifecycle; the public / internal /
    /// metrics-health surfaces are all opened (3/3 ports up); no hand-rolled main.
    #[test]
    fn flow_shell_boots_and_three_ports_bind() {
        let handle =
            boot_flow(Config::default(), OutboxStore::new()).expect("the myelin-flow shell boots");
        assert_eq!(handle.name(), SERVICE_NAME);
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports (public / internal-RPC / metrics-health) all bound (3/3)"
        );
    }

    /// **Liveness ≠ readiness (contract 1.3): readiness is FALSE *before* migrate completes, but
    /// liveness stays Up.** The metrics-health surface opens in the `Booting` startup state —
    /// not-ready (it cannot serve correct traffic before its schema exists) but not-killed. The
    /// migrate-complete gate is what lifts readiness — the smoke-test core of the §3.1 boot order.
    #[test]
    fn readiness_is_false_pre_migrate_but_liveness_is_up() {
        let surface =
            MetricsHealthSurface::new(CriticalDependencies::new(["oltp"]), HealthTable::new());
        // before migrate completes: Booting → not-ready (sheds) but liveness Up (not-killed).
        assert_eq!(surface.startup(), Startup::Booting);
        let r = surface.readiness();
        assert_eq!(
            r.verdict,
            Readiness::NotReady,
            "readiness is FALSE until the migrate-complete gate lifts"
        );
        assert!(
            r.startup_incomplete,
            "the not-ready reason names the startup (pre-migrate) gate"
        );
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        assert_eq!(
            surface.liveness(),
            Liveness::Up,
            "liveness ≠ readiness: a booting instance is not-killed (liveness stays Up)"
        );
        // migrate completes → readiness lifts.
        surface.mark_started();
        assert_eq!(
            surface.readiness().verdict,
            Readiness::Ready,
            "after migrate-complete the readiness gate lifts → ready"
        );
    }

    /// **A booted flow instance reports ready only AFTER migrate completes (contract 1.3).** The
    /// harness flips the startup gate to `Complete` at the end of a successful boot — over the
    /// non-empty P-FLOW-01 six-table migration set (so the migrate phase actually runs). The
    /// metrics-health port reports ready only once migrate + relay are live.
    #[test]
    fn booted_instance_is_ready_after_migrate_complete() {
        let handle = boot_flow(Config::default(), OutboxStore::new()).expect("boot");
        assert_eq!(
            handle.metrics_health().startup(),
            Startup::Complete,
            "boot completed → the migrate gate lifted (the six-table set applied)"
        );
        assert_eq!(
            handle.metrics_health().readiness().verdict,
            Readiness::Ready,
            "a booted flow instance (the six tables migrated, deps up) is ready"
        );
    }

    /// **The AppSpec wires the six-table schema plus online control expands and the EMPTY consumer
    /// seam.** The `consumers` slot is empty (the replay engine +
    /// the signal/timer consumers are the P-FLOW-04..05/09/13 floor — this shell has NO executor).
    #[test]
    fn shell_wires_the_six_table_migration_set_and_empty_consumer_seam() {
        let spec = flow_app_spec(Config::default(), OutboxStore::new());
        assert_eq!(spec.name, SERVICE_NAME);
        assert!(
            spec.consumers.is_empty(),
            "the replay engine + signal/timer consumers are the P-FLOW-04..05/09/13 floor (empty seam)"
        );
        assert_eq!(
            spec.migrations.0.len(),
            8,
            "six table creates plus two online workflow control expands"
        );
        assert_eq!(
            spec.migrations,
            crate::migrations::migrations(),
            "the migrate phase wires EXACTLY the P-FLOW-01 set (no second schema)"
        );
    }

    /// **The inbound-signal consumer FILLS the P-FLOW-02 `consumers` slot (P-FLOW-09).** Building the
    /// signal `ConsumerReg` over a [`FlowExecutor`] binds the `sig.<tenant>.` whitelist through the
    /// sanctioned `consume` (rule 3: `*`/empty rejected) and registers it — the empty signal seam is
    /// now occupied. A subsequent AppSpec carrying it has a non-empty `consumers` slot.
    #[test]
    fn inbound_signal_consumer_fills_the_consumer_slot() {
        use myelin_events::DedupLedger;
        use myelin_tenancy::{Region, TenantId};
        let minter: std::sync::Arc<dyn myelin_events::IdMinter> =
            std::sync::Arc::new(myelin_events::MonotonicMinter::new());
        let tenant = TenantId("acme".into());
        let ex = crate::FlowExecutor::new(minter, tenant.clone(), Region("fr-par".into()));
        let reg = flow_signal_consumer_reg(&tenant, ex, DedupLedger::new())
            .expect("the sig.acme. whitelist binds through the sanctioned consume (never `*`)");

        // wire it into the slot — the P-FLOW-02 empty seam is now filled for the signal leg.
        let mut spec = flow_app_spec(Config::default(), OutboxStore::new());
        spec.consumers = vec![reg];
        assert_eq!(
            spec.consumers.len(),
            1,
            "the inbound-signal consumer occupies the consumer slot (P-FLOW-09)"
        );
    }

    /// **The flow OLTP store auto-registers as a `PersonalDataHolder` at boot (§3.4, GD-3).** Even
    /// before the references-not-payloads store-holder body (P-FLOW-03), the harness's one door
    /// registers the opened OLTP store — "we forgot a store" is structurally impossible.
    #[test]
    fn flow_oltp_store_auto_registers_as_a_holder_at_boot() {
        use myelin_substrate::StoreKind;
        let handle = boot_flow(Config::default(), OutboxStore::new()).expect("boot");
        assert!(
            handle
                .holder_registry()
                .is_registered(StoreKind::Oltp, SERVICE_NAME),
            "the flow OLTP store auto-registered as a holder at boot (opening IS registering)"
        );
        assert!(
            handle.holder_registered().is_ok(),
            "no store the service declares escaped registration (the holder-registered architecture test)"
        );
    }

    /// **The boot-registered flow OLTP store classifies to H8 — holder-completeness GREEN on boot
    /// (P-FLOW-03; contract 1.4 + gdpr §3.2).** The store the harness auto-registers on boot is in the
    /// exhaustive H1–H18 list (H8, the §5.5 references-not-payloads reconcile) — 0 orphan, so the M5
    /// DSAR fan-out cannot miss workflow history. This is the structural gate: the holder
    /// auto-registers on boot AND is accounted for in the data map.
    #[test]
    fn boot_registered_flow_store_classifies_and_completeness_is_green() {
        use crate::holder::{flow_history_holder, flow_store_classifier};
        use myelin_substrate::{assert_holder_completeness, Holder};
        let handle = boot_flow(Config::default(), OutboxStore::new()).expect("boot");
        // the auto-registered boot store classifies to H8 (no orphan).
        assert_eq!(flow_history_holder(), Some(Holder::H8EventBus));
        // the holder-completeness assertion is green over the boot registry + the flow classifier.
        assert_eq!(
            assert_holder_completeness(
                handle.holder_registry().registrations(),
                &flow_store_classifier(),
            ),
            Ok(()),
            "every store the flow harness opens is in the exhaustive H1–H18 list — 0 orphan"
        );
    }

    /// **`run_flow` runs the whole lifecycle end-to-end and returns Ok on a clean drain (the CDC
    /// consumer side of 1.1 — a `main` that just calls `serve`).** A clean drain leaves the outbox
    /// at depth 0 (nothing committed is left unpublished) — the silent-data-loss floor.
    #[test]
    fn run_flow_boots_serves_and_drains_cleanly() {
        assert_eq!(
            run_flow(Config::default(), OutboxStore::new()),
            Ok(()),
            "the flow shell boots → migrates → relays → drains cleanly (depth 0)"
        );
    }

    /// **A failed boot returns non-zero (an `Err`), never a silent success (§3.1).** A config that
    /// fails boot-time validation (a bad/unbounded pool) aborts boot with a loud error — the
    /// fail-fast property the bootable shell must exhibit.
    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_flow(Config("BAD_POOL".into()), OutboxStore::new());
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the boot error names the §3.2 fail-fast config validation"
        );
    }

    /// **The engine-wired AppSpec returns a dispatcher whose tick drives a runnable run (P-FLOW-05).**
    /// `flow_app_spec_with_engine` assembles the SAME schema/control set PLUS the replay/lease worker
    /// loop; a registered body + a seeded runnable run drive to completion on one tick — the consumer
    /// seam is filled by the engine's worker (not a bus subscriber). The spec still wires the six
    /// tables (the migrate phase is unchanged).
    #[test]
    fn engine_wired_spec_returns_a_driving_dispatcher() {
        use crate::engine::{run_state, DriveOutcome, RunRow};
        use crate::RetryPolicy;
        use myelin_events::{Actor, EmitContextBase, MonotonicMinter, Timestamp};
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        use myelin_refs::ArtifactRef;
        use myelin_tenancy::{Region, TenantId};
        use std::sync::Arc;

        let tenant = TenantId("acme".into());
        let region = Region("fr-par".into());
        let ctx_base = EmitContextBase {
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant.clone(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: None,
        };
        let (spec, mut dispatcher, _wheel) = flow_app_spec_with_engine(
            Config::default(),
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
            ctx_base,
            0,
            "worker-1",
            30,
        );
        // Engine wiring does not add a second schema.
        assert_eq!(
            spec.migrations.0.len(),
            8,
            "the engine-wired spec keeps the schema and online control expands"
        );

        // register a body + seed a runnable run; one tick drives it to completion.
        dispatcher.register(
            "agent.run",
            Box::new(|ctx: &mut crate::WfCtx| {
                ctx.activity(RetryPolicy::default_policy(), |_i, _a| {
                    Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
                })
                .map_err(|e| format!("{e:?}"))?;
                Ok(vec![])
            }),
        );
        dispatcher.runs().put(RunRow::new_runnable(
            tenant.clone(),
            region,
            "R1",
            "agent.run",
            0,
        ));
        let outcome = dispatcher.tick(1000, "2026-06-21T00:00:00Z", 7);
        assert!(
            matches!(outcome, Some(DriveOutcome::Completed(_))),
            "the dispatcher drove the run"
        );
        assert_eq!(
            dispatcher.runs().get(&tenant, "R1").unwrap().state,
            run_state::COMPLETED,
            "the seeded run completed under the engine-wired dispatcher"
        );
        assert_eq!(
            dispatcher.telemetry().double_effect_count(),
            0,
            "0 double-effect"
        );
    }

    /// **The timer-wheel scan loop is wired into the consumer seam alongside the dispatcher
    /// (P-FLOW-13, §4.2).** `flow_app_spec_with_engine` returns the dispatcher PLUS the
    /// [`crate::timer::TimerWheel`] over the SAME run store + journal + timer store. A body that
    /// `ctx.sleep_for`s arms a `wf_timer` row and PARKS the run (`waiting`); the wheel's tick fires the
    /// timer at its minute (waking the run `waiting → running`); a SECOND dispatcher tick re-drives the
    /// run past the sleep to completion. End-to-end: arm → park → wheel-fire → wake → re-drive.
    #[test]
    fn timer_wheel_wired_into_consumer_seam_parks_fires_and_re_drives() {
        use crate::engine::{run_state, DriveOutcome, RunRow};
        use myelin_events::{Actor, EmitContextBase, MonotonicMinter, Timestamp};
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        use myelin_refs::ArtifactRef;
        use myelin_tenancy::{Region, TenantId};
        use std::sync::Arc;

        let tenant = TenantId("acme".into());
        let region = Region("fr-par".into());
        let ctx_base = EmitContextBase {
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant.clone(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: None,
        };
        let (_spec, mut dispatcher, wheel) = flow_app_spec_with_engine(
            Config::default(),
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
            ctx_base,
            0,
            "worker-1",
            30,
        );

        // a body that sleeps 600s then completes — the sleep is the FIRST command (a fresh run parks).
        dispatcher.register(
            "sla.run",
            Box::new(|ctx: &mut crate::WfCtx| {
                ctx.sleep_for(600).map_err(|e| format!("{e:?}"))?;
                ctx.activity(crate::RetryPolicy::default_policy(), |_i, _a| {
                    Ok(vec![ArtifactRef(
                        "myelin://acme/agent/effect/after-sleep".into(),
                    )])
                })
                .map_err(|e| format!("{e:?}"))?;
                Ok(vec![])
            }),
        );
        dispatcher.runs().put(RunRow::new_runnable(
            tenant.clone(),
            region,
            "R1",
            "sla.run",
            0,
        ));

        // tick 1 (now=1000s): the body arms a 600s timer (fires at 1600s) + PARKS — the run is waiting.
        let o1 = dispatcher.tick(1000, "2026-06-21T00:00:00Z", 7);
        assert!(
            matches!(o1, Some(DriveOutcome::Waiting)),
            "the sleep parked the run, got {o1:?}"
        );
        assert_eq!(
            dispatcher.runs().get(&tenant, "R1").unwrap().state,
            run_state::WAITING,
            "the run is waiting (no runtime)"
        );
        assert_eq!(
            wheel.timers().unfired_count(),
            1,
            "one durable timer armed on the wheel"
        );

        // a wheel tick BEFORE the minute (now=1100s): the timer is far-future (bucket > now) → not fired.
        assert_eq!(
            wheel.tick(1100),
            0,
            "the not-yet-due timer is NOT fired (far-future bucket untouched)"
        );
        assert_eq!(
            dispatcher.runs().get(&tenant, "R1").unwrap().state,
            run_state::WAITING,
            "still waiting"
        );

        // a wheel tick AT the minute (now=1600s): the timer fires → the run wakes (waiting → running).
        assert_eq!(wheel.tick(1600), 1, "the due timer fires at its minute");
        assert_eq!(
            dispatcher.runs().get(&tenant, "R1").unwrap().state,
            run_state::RUNNING,
            "the wheel woke the run"
        );
        assert_eq!(
            wheel.telemetry().timer_wheel_lag(),
            0,
            "the timer-wheel-lag is 0 after the fire (SC-11 health signal)"
        );

        // tick 2 (now=1601s): the dispatcher re-leases + re-drives — the sleep replays (no re-arm), the
        // post-sleep activity runs, the run completes.
        let o2 = dispatcher.tick(1601, "2026-06-21T00:00:01Z", 7);
        assert!(
            matches!(o2, Some(DriveOutcome::Completed(_))),
            "the run completed past the sleep, got {o2:?}"
        );
        assert_eq!(
            dispatcher.runs().get(&tenant, "R1").unwrap().state,
            run_state::COMPLETED,
            "the run is completed"
        );
        assert_eq!(
            dispatcher.telemetry().double_effect_count(),
            0,
            "0 double-effect (the sleep replayed, did not re-arm)"
        );
    }

    /// **Graceful drain leaves the outbox at depth 0 (contract 1.1 / §3.1).** A booted-then-drained
    /// flow instance finishes in-flight work and exits clean — the relay drained the outbox, nothing
    /// committed is left unprocessed.
    #[test]
    fn graceful_drain_leaves_outbox_depth_zero() {
        let handle = boot_flow(Config::default(), OutboxStore::new()).expect("boot");
        handle.signal_drain();
        assert!(handle.is_draining(), "intake is stopped");
        // one tick finishes in-flight; the telemetry snapshot reports depth 0.
        handle.tick();
        let t = handle.telemetry();
        assert_eq!(
            t.outbox_depth(),
            0,
            "the graceful drain leaves outbox_depth == 0"
        );
        assert_eq!(
            t.dead_letter_count(),
            0,
            "nothing dead-lettered on a clean shell drain"
        );
    }
}
