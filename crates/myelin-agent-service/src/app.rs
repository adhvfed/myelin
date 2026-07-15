//! # `app` — the agent-service `serve(AppSpec)` shell + the dispatch consumer (AG-P4 → P-216, M2-A)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §3.1 (the SKELETON runtime
//! drives the whole gateway/dispatch path), §3.4 (explicit-first dispatch: a mention NOTIFIES, it
//! does NOT auto-spawn a costed run — CHAT-1), §1.3 (`EventInbox::deliver` — the platform delivers
//! matched events; agents don't poll).
//!
//! **Contract-index:** consumes 1.1 `serve(AppSpec)` (boot → migrate → outbox-relay → three ports →
//! graceful drain), 1.2 the three ports, 1.3 liveness ≠ readiness, 1.4 holder auto-registration (the
//! Fabric's H11 OLTP + H17 trace holders auto-register on boot), 3.6 (the dispatch tier — the
//! agent-side consumer bound by name with a `subjects()` whitelist, NEVER `*`), 8.6
//! (`EventInbox::deliver`). Owns no new contract.
//!
//! ## What this prompt (AG-P4) ships — the runnable agent service shell + the dispatch consumer
//!
//! [`agent_app_spec`] assembles the [`AppSpec`](myelin_substrate::AppSpec) the harness wires:
//!   - **boot** → validate the env-first config (fail fast, §3.2);
//!   - **migrate** → run the AG-P2 five-table forward-only migration set ([`crate::migrations`]);
//!   - **relay** → start the transactional outbox relay (BUS-2 — the ONLY emit path);
//!   - **consumers** → the AG-P4 dispatch consumer ([`agent_dispatch_consumer_reg`]) bound to the
//!     `agent.dispatch.<tenant>.` whitelist (rule 3: `consume` REJECTS a `*`/empty subject loudly) —
//!     the explicit-first dispatch tier (§3.4): a delivered event NOTIFIES the agent inbox; the
//!     COSTED run is started behind the reserve/settle gate by the SKELETON loop ([`crate::skeleton`]);
//!   - **ports** → the harness opens the three surfaces (public / internal-RPC / metrics-health) with
//!     liveness ≠ readiness (readiness lifts only after migrate completes) + graceful drain;
//!   - **holders** → every opened store auto-registers as a `PersonalDataHolder` (the AG-P3 seam —
//!     the H11 agent-memory + H17 agent-trace stores).
//!
//! ## Explicit-first dispatch (CHAT-1, §3.4) — the consumer NOTIFIES, it does not auto-spawn
//! [`SkeletonDispatchConsumer::handle`] is the dispatch tier's agent-side: it acknowledges a
//! delivered match into the inbox (the platform delivers; agents don't poll, 8.6). It does NOT
//! auto-spawn a costed run from a casual mention — implicit auto-dispatch is L-3 (counsel-gated,
//! AG-P20). An EXPLICIT "run an agent here" is what drives [`crate::skeleton::SkeletonAgent::handle_run`]
//! (which passes the reserve/settle gate). The consumer is idempotent on `event_id` (the seven-rule
//! `consume` runtime owns the dedup ledger).
//!
//! ## FLOORS named (this is the SKELETON shell; VISION §3)
//! - **The dispatch consumer NOTIFIES; it does not yet START a run.** The wiring from a delivered
//!   explicit-run event to [`crate::skeleton::SkeletonAgent::handle_run`] (building the `RunSubstrate`
//!   from the event + the binding) is the live dispatch-tier wiring; the SKELETON proves the chained
//!   path directly (the e2e test calls `handle_run`). The full event→trigger→effect loop on the mock
//!   brain is AG-P5 (→ P-217).
//! - **The mock brain + the tools** are AG-P5 / AG-P6 / AG-P15. This shell runs the SKELETON brain.

use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, SubjectPattern};
use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, HotTables, InternalRpc, Migrations,
    OutboxSpec, PublicRoutes, ServeError, ServeHandle, StoreManifest,
};

/// The service name — the PII-free telemetry/trace service identifier the harness labels traces
/// and the contract-1.8 signal set with.
pub const SERVICE_NAME: &str = "myelin-agent";

/// The dispatch-subject prefix the agent-side consumer whitelists (rule 3: NEVER `*`). The dispatch
/// tier (3.6) publishes a matched event to `agent.dispatch.<tenant>.<...>`; the consumer binds
/// `agent.dispatch.<tenant>.` so one tenant's dispatch never head-of-line-blocks another's.
pub const AGENT_DISPATCH_SUBJECT_PREFIX: &str = "agent.dispatch.";

/// The agent-service forward-only migration set (contract 1.5) the `serve` boot lifecycle runs
/// migrate → ready over: the AG-P2 five-table data model (`run` / `tool_def` / `proposed_effect` /
/// `hitl_gate` / `trace`), each `(tenant, region)`-first + RLS — see [`crate::migrations`].
///
/// **Why a translation (coherence, EI-01 §7):** [`crate::migrations::migrations`] (AG-P2) returns
/// `myelin_storage::Migrations` because it is validated by the storage forward-only ONLINE runner
/// (the RLS/expand-backfill-contract discipline lives in storage). The `serve(AppSpec)` lifecycle
/// (contract 1.1) takes a `myelin_substrate::Migrations`. The SAME five DDLs (+ the RLS-scope step)
/// are carried across to the substrate set here — ONE schema, two type-carriers, no second schema.
fn agent_service_migrations() -> Migrations {
    use crate::migrations::{
        rls_scope_sql, HITL_GATE_DDL, PROPOSED_EFFECT_DDL, RUN_DDL, TOOL_DEF_DDL, TRACE_DDL,
    };
    use myelin_substrate::{Migration, MigrationPhase};
    // The five tables, in order, each (tenant, region)-first; every tenant-scoped table emits the
    // `myelin_make_tenant_scoped` RLS-readiness step after its CREATE (the AG-P2 convention). The DDL
    // is leaked once (bounded — the set is built once at boot; the substrate `Migration` holds
    // `&'static str`, the same pattern `myelin-flow` / `myelin-notif` use).
    let tables: [(&'static str, &str, &str); 5] = [
        ("0001_create_agent_run", RUN_DDL, "agent_run"),
        ("0002_create_agent_tool_def", TOOL_DEF_DDL, "agent_tool_def"),
        (
            "0003_create_agent_proposed_effect",
            PROPOSED_EFFECT_DDL,
            "agent_proposed_effect",
        ),
        (
            "0004_create_agent_hitl_gate",
            HITL_GATE_DDL,
            "agent_hitl_gate",
        ),
        ("0005_create_agent_trace", TRACE_DDL, "agent_trace"),
    ];
    Migrations::of(tables.into_iter().map(|(id, create_ddl, table)| {
        let mut ddl = String::new();
        ddl.push_str(create_ddl);
        ddl.push(';');
        ddl.push('\n');
        ddl.push_str(&rls_scope_sql(table));
        ddl.push(';');
        let ddl: &'static str = Box::leak(ddl.into_boxed_str());
        Migration::phased(id, ddl, MigrationPhase::Plain, table)
    }))
}

/// **The explicit-first dispatch consumer (§3.4 / 3.6 / 8.6) — the agent-side of the dispatch
/// tier.** Binds the `agent.dispatch.<tenant>.` subject whitelist (rule 3: NEVER `*`). On a delivered
/// match it NOTIFIES the agent inbox (the platform delivers; agents don't poll) and returns
/// [`HandleOutcome::Done`] — it does NOT auto-spawn a costed run from a casual mention (implicit
/// auto-dispatch is L-3, AG-P20). An EXPLICIT run drives [`crate::skeleton::SkeletonAgent::handle_run`]
/// (behind the reserve/settle gate). Idempotent on `event_id` (the `consume` runtime's dedup ledger).
pub struct SkeletonDispatchConsumer {
    /// The `*`-free subject whitelist bound for this consumer's tenant (leaked to `'static` once per
    /// tenant per process — the binding set is fixed for the consumer pool's life).
    subjects: &'static [SubjectPattern],
}

impl SkeletonDispatchConsumer {
    /// Build the dispatch consumer over a `*`-free subject `subjects` whitelist (the
    /// `agent.dispatch.<tenant>.` prefix). The whitelist is validated again at [`consume`] time.
    pub fn new(subjects: &'static [SubjectPattern]) -> SkeletonDispatchConsumer {
        SkeletonDispatchConsumer { subjects }
    }
}

impl EventHandler for SkeletonDispatchConsumer {
    /// The whitelist — NEVER `*` (BUS-3). `consume` re-rejects a `*`/empty subject at registration.
    fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    /// Handle a delivered dispatch match (explicit-first, §3.4): NOTIFY the agent inbox. A mention
    /// notifies; it does NOT auto-spawn a costed run here (that is the explicit-run path through the
    /// reserve/settle gate). Idempotent on `event_id` (the runtime's dedup ledger). Returns `Done` —
    /// the notification is delivered; the COSTED run is the SKELETON loop's, fronted by reserve.
    fn handle(&self, _ev: &EventEnvelope) -> HandleOutcome {
        // Explicit-first: deliver/notify into the inbox. No auto-spawn (no costed run started here).
        // The SKELETON's `handle_run` is driven by an EXPLICIT run event (behind the cost gate).
        HandleOutcome::Done
    }
}

/// **Assemble the agent-service [`AppSpec`] (contract 1.1; §3.1) the harness wires.** The harness
/// owns the lifecycle: boot → migrate (the AG-P2 five tables) → outbox relay (BUS-2) → consumers (the
/// dispatch consumer seam) → three ports (liveness ≠ readiness) → graceful drain.
///
/// `config` is the validated, env-first config (§3.2). The agent OLTP store is implicitly critical
/// (the harness adds it). A healthy boot is ready once the migrations apply.
///
/// **Floors wired as named seams:**
/// - the `consumers` slot is EMPTY in the bare spec — wire the dispatch consumer per-tenant via
///   [`agent_dispatch_consumer_reg`] (the explicit-first dispatch tier, §3.4);
/// - holder auto-registration: every opened store auto-registers (the AG-P3 H11/H17 seam, §3.4).
///
/// **The outbox is INJECTED (MR-009b W3b.6 — the W3b.4 debt discharged):** this builder
/// constructs NO store. There is no agent-service binary yet; when its production root lands it
/// must pass `OutboxStore::durable(PgOutboxBacking)` (the W3b.4 provider-from-env, fail-loud
/// main.rs pattern the six service mains follow); a test/drill passes the `test-support`-gated
/// in-memory `OutboxStore::new()` double.
pub fn agent_app_spec(config: Config, outbox: myelin_events::OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        // The AG-P2 five-table data model (run / tool_def / proposed_effect / hitl_gate / trace).
        migrations: agent_service_migrations(),
        // Fresh `CREATE TABLE`s at this milestone — no expand→backfill→contract discipline yet.
        hot_tables: HotTables::none(),
        // The public surface (gateway-fronted, tenant-from-token) — the run-control API bodies are the
        // later prompts; the harness opens the live tenant-from-token surface.
        public: PublicRoutes::default(),
        // The internal-RPC surface — the Fabric exposes its run-control to siblings; bodies are later.
        internal: InternalRpc::default(),
        // The dispatch consumer is wired per-tenant via `agent_dispatch_consumer_reg` (a process binds
        // the tenants it serves). The bare spec's slot is empty — the explicit-first dispatch tier.
        consumers: Vec::new(),
        // Every opened store auto-registers as a PersonalDataHolder (§3.4, the AG-P3 H11/H17 seam).
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        // The transactional outbox relay (BUS-2 — the ONLY emit path; the SKELETON's trace emit rides
        // this). The relay drains the INJECTED store (MR-009b W3b.6 — the named W3b.4 debt
        // discharged: this builder no longer constructs the memory floor). The in-process broker
        // fake stays the default TRANSPORT (durability lives in the store); the JetStream-class
        // adapter is a config swap (dev<->prod), never a code change.
        outbox: OutboxSpec::new(outbox, myelin_events::InProcessBus::new()),
        // The agent OLTP store is implicitly critical; no further critical downstream at the shell.
        critical: CriticalDependencies::default(),
    }
}

/// **Build the explicit-first dispatch `ConsumerReg` for `tenant` — the bus side of the dispatch
/// tier wired into the AppSpec `consumers` slot (3.6 / 8.6 / §3.4).** Constructs the
/// [`SkeletonDispatchConsumer`] over the `agent.dispatch.<tenant>.` whitelist and wraps it in the
/// seven-rule [`Consumer`](myelin_events::Consumer) through the SANCTIONED [`consume`](myelin_events::consume)
/// — binding the subject whitelist (rule 3: `consume` REJECTS a `*`/`>`/empty subject loudly), the
/// durable consumer name (rule 4), the bounded prefetch + per-tenant fairness cap (rule 6), and the
/// shared `dedup` ledger (rule 1: `event_id` idempotency).
///
/// Returns the [`ConsumerReg`](myelin_substrate::ConsumerReg) the `serve` lifecycle registers — the
/// registration that FILLS the dispatch seam. An over-broad / malformed tenant prefix returns
/// [`SubscribeError`](myelin_events::SubscribeError) — the shell never silently narrows to an
/// over-broad subscription.
pub fn agent_dispatch_consumer_reg(
    tenant: &myelin_tenancy::TenantId,
    dedup: myelin_events::DedupLedger,
) -> Result<myelin_substrate::ConsumerReg, myelin_events::SubscribeError> {
    use myelin_events::{consume, ConsumerName, ConsumerSpec};
    // The `agent.dispatch.<tenant>.` subject whitelist (NEVER `*`, BUS-3). Validated + leaked to
    // `'static` once per tenant per process (the binding set is fixed for the pool's life — bounded).
    let prefix = format!("{AGENT_DISPATCH_SUBJECT_PREFIX}{}.", tenant.0);
    let subjects: &'static [SubjectPattern] =
        Box::leak(vec![SubjectPattern(prefix.clone())].into_boxed_slice());
    let consumer = SkeletonDispatchConsumer::new(subjects);
    // The ONE sanctioned consumer entry-point — `consume` validates the spec (rule 3: rejects a
    // `*`/empty subject LOUDLY) and constructs the [`Consumer`] with all seven rules wired.
    let runtime = consume(
        ConsumerSpec::new(
            ConsumerName(format!("agent-dispatch-{}", tenant.0)),
            &[prefix.as_str()],
        ),
        consumer,
        dedup,
    )?;
    Ok(myelin_substrate::ConsumerReg::new(runtime))
}

/// **Boot the agent service shell under the harness (contract 1.1)** up to the pre-serve state,
/// returning the [`ServeHandle`] the lifecycle drives. A thin wrapper over
/// [`boot`](myelin_substrate::boot) of [`agent_app_spec`] — separated so a test/drill can boot,
/// inspect the three ports + the liveness ≠ readiness state, and drive the graceful drain.
///
/// Returns `Err` (the non-zero exit) on a failed boot (§3.1) — loud, never a silent success.
pub fn boot_agent(
    config: Config,
    outbox: myelin_events::OutboxStore,
) -> Result<ServeHandle, ServeError> {
    boot(agent_app_spec(config, outbox))
}

/// **Run the agent service to completion under the harness** (boot → migrate → relay → consumers →
/// three ports → graceful drain). The `myelin-agent` binary calls this; a failed boot / incomplete
/// drain returns `Err` (the non-zero process exit, §3.1).
pub fn run_agent(config: Config, outbox: myelin_events::OutboxStore) -> Result<(), ServeError> {
    serve(agent_app_spec(config, outbox))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{
        HealthTable, Liveness, MetricsHealthSurface, Readiness, Startup, Surface,
    };

    /// **The shell boots under the harness and the three ports bind (contracts 1.1/1.2).** The agent
    /// AppSpec runs the boot → migrate → relay → ports lifecycle; the public / internal /
    /// metrics-health surfaces are all opened (3/3 ports up); no hand-rolled main.
    #[test]
    fn agent_shell_boots_and_three_ports_bind() {
        let handle = boot_agent(Config::default(), myelin_events::OutboxStore::new())
            .expect("the myelin-agent shell boots");
        assert_eq!(handle.name(), SERVICE_NAME);
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports (public / internal-RPC / metrics-health) all bound (3/3)"
        );
    }

    /// **Liveness ≠ readiness (contract 1.3): readiness is FALSE before migrate completes, liveness
    /// stays Up.** A booting instance is not-ready (it sheds) but not-killed; the migrate-complete
    /// gate lifts readiness.
    #[test]
    fn readiness_is_false_pre_migrate_but_liveness_is_up() {
        let surface =
            MetricsHealthSurface::new(CriticalDependencies::new(["oltp"]), HealthTable::new());
        assert_eq!(surface.startup(), Startup::Booting);
        let r = surface.readiness();
        assert_eq!(
            r.verdict,
            Readiness::NotReady,
            "readiness FALSE until migrate completes"
        );
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        assert_eq!(
            surface.liveness(),
            Liveness::Up,
            "liveness ≠ readiness: booting is not-killed"
        );
        surface.mark_started();
        assert_eq!(
            surface.readiness().verdict,
            Readiness::Ready,
            "migrate-complete lifts readiness"
        );
    }

    /// **A booted instance is ready only AFTER migrate completes (contract 1.3).** The harness flips
    /// the startup gate to `Complete` at the end of a successful boot over the AG-P2 five-table set.
    #[test]
    fn booted_instance_is_ready_after_migrate_complete() {
        let handle =
            boot_agent(Config::default(), myelin_events::OutboxStore::new()).expect("boot");
        assert_eq!(
            handle.metrics_health().startup(),
            Startup::Complete,
            "boot completed → the migrate gate lifted (the five-table set applied)"
        );
        assert_eq!(
            handle.metrics_health().readiness().verdict,
            Readiness::Ready,
            "a booted agent instance (the five tables migrated, deps up) is ready"
        );
    }

    /// **The AppSpec wires the AG-P2 five-table migration set + the empty dispatch-consumer seam.**
    /// The migrate phase runs over the five tables; the bare `consumers` slot is empty (filled
    /// per-tenant by `agent_dispatch_consumer_reg`).
    #[test]
    fn shell_wires_the_five_table_migration_set_and_empty_consumer_seam() {
        let spec = agent_app_spec(Config::default(), myelin_events::OutboxStore::new());
        assert_eq!(spec.name, SERVICE_NAME);
        assert!(
            spec.consumers.is_empty(),
            "the dispatch consumer is wired per-tenant (empty bare seam)"
        );
        // the migrate phase wires EXACTLY the AG-P2 five tables (one schema, two type-carriers) — no
        // second schema: the substrate set carries the same five ids in the same order, each scoped.
        assert_eq!(
            spec.migrations.0.len(),
            5,
            "the AppSpec wires the AG-P2 five-table data model"
        );
        let ids: Vec<&str> = spec.migrations.0.iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            vec![
                "0001_create_agent_run",
                "0002_create_agent_tool_def",
                "0003_create_agent_proposed_effect",
                "0004_create_agent_hitl_gate",
                "0005_create_agent_trace",
            ],
            "the substrate migrate set carries the SAME five AG-P2 migrations (one schema)"
        );
    }

    /// **The dispatch consumer FILLS the `consumers` slot bound to a `*`-free whitelist (3.6 /
    /// §3.4).** Building the dispatch `ConsumerReg` binds the `agent.dispatch.<tenant>.` whitelist
    /// through the sanctioned `consume` (rule 3: `*`/empty rejected) and registers it.
    #[test]
    fn dispatch_consumer_fills_the_consumer_slot() {
        use myelin_events::DedupLedger;
        use myelin_tenancy::TenantId;
        let tenant = TenantId("acme".into());
        let reg = agent_dispatch_consumer_reg(&tenant, DedupLedger::new()).expect(
            "the agent.dispatch.acme. whitelist binds through the sanctioned consume (never `*`)",
        );
        let mut spec = agent_app_spec(Config::default(), myelin_events::OutboxStore::new());
        spec.consumers = vec![reg];
        assert_eq!(
            spec.consumers.len(),
            1,
            "the dispatch consumer occupies the consumer slot (3.6)"
        );
    }

    /// **The agent OLTP + trace stores auto-register as `PersonalDataHolder`s at boot (§3.4, AG-P3
    /// seam).** Opening IS registering — "we forgot a store" is structurally impossible.
    #[test]
    fn agent_stores_auto_register_as_holders_at_boot() {
        use myelin_substrate::StoreKind;
        let handle =
            boot_agent(Config::default(), myelin_events::OutboxStore::new()).expect("boot");
        assert!(
            handle
                .holder_registry()
                .is_registered(StoreKind::Oltp, SERVICE_NAME),
            "the agent OLTP store auto-registered as a holder at boot"
        );
        assert!(
            handle.holder_registered().is_ok(),
            "no store the service declares escaped registration (the holder-registered architecture test)"
        );
    }

    /// **`run_agent` runs the whole lifecycle end-to-end and returns Ok on a clean drain (the CDC
    /// consumer side of 1.1 — a `main` that just calls `serve`).** A clean drain leaves the outbox at
    /// depth 0 (nothing committed is left unpublished).
    #[test]
    fn run_agent_boots_serves_and_drains_cleanly() {
        assert_eq!(
            run_agent(Config::default(), myelin_events::OutboxStore::new()),
            Ok(()),
            "the agent shell boots → migrates → relays → drains cleanly (depth 0)"
        );
    }

    /// **A failed boot returns non-zero (an `Err`), never a silent success (§3.1).** A config that
    /// fails boot-time validation aborts boot with a loud error.
    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_agent(Config("BAD_POOL".into()), myelin_events::OutboxStore::new());
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
    }

    /// **The dispatch consumer is explicit-first (§3.4): it NOTIFIES, it does not auto-spawn a costed
    /// run.** `handle` returns `Done` (the notification delivered) without starting a run — implicit
    /// auto-dispatch is L-3 (AG-P20). A `*` subject is unconstructable (rule 3).
    #[test]
    fn dispatch_consumer_is_explicit_first_notify_only() {
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
            Timestamp, Visibility,
        };
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        use myelin_refs::ArtifactRef;
        use myelin_tenancy::{Region, TenantId};
        let subjects: &'static [SubjectPattern] =
            Box::leak(vec![SubjectPattern("agent.dispatch.acme.".into())].into_boxed_slice());
        let consumer = SkeletonDispatchConsumer::new(subjects);
        // a delivered dispatch match → NOTIFY (Done), no costed run started. The envelope is built to
        // the frozen contract-2.10 field list (the consumer reads only its taxonomy + idempotency).
        let tenant = TenantId("acme".into());
        let ev = EventEnvelope {
            event_id: EventId("ev-1".into()),
            type_: EventType("agent.dispatch.acme.mention".into()),
            schema_ver: 1,
            tenant: tenant.clone(),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant,
            )),
            subject: ArtifactRef("myelin://acme/chat/msg/1".into()),
            aggregate: AggregateKey("conv:1".into()),
            causation_id: None,
            correlation_id: CorrelationId("c1".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            payload: serde_json::json!({}),
        };
        assert_eq!(
            consumer.handle(&ev),
            HandleOutcome::Done,
            "explicit-first: a delivered match NOTIFIES (Done); it does not auto-spawn a costed run"
        );
    }
}
