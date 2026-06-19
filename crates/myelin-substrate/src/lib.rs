//! # `myelin-substrate` — the bootstrap harness (`serve(AppSpec)`) + fail-static primitives
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.6 (`myelin-substrate` — the bootstrap harness crate), §3 (`serve(AppSpec)`),
//! §8 (fail-static primitives), §2.9 (the dependency root — root-last, no cycles).
//!
//! **Contract-index cluster:** 1 — Bootstrap & service shell
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` rows 1.1
//! `serve(AppSpec)`, 1.10 `FailStatic<T>`).
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! - `serve(AppSpec)` (1.1) — boot → migrate → outbox relay → consumers → three ports →
//!   graceful drain; non-zero on failed boot. The one call a service's `main.rs` makes.
//! - `AppSpec{ name, config, migrations, public, internal, consumers, holders, outbox }`
//!   (1.1, architecture §3.1) — the spec the harness consumes.
//! - `FailStatic<T>` (1.10) — bounded-staleness cache; `static_max ≤ revocation SLA` and
//!   `≥ agent-token TTL`; `get` returns `Fresh | Static(degraded) | Closed`.
//!
//! ## DAG root (§2.9): this crate is root-LAST
//! `myelin-substrate` depends on ALL the glue crates and NO glue crate depends on it.
//! The [`crate_graph`] module encodes the §2.9 DAG declaratively and the
//! `crate-graph-acyclic` test asserts (a) the graph has no cycle and (b)
//! `myelin-identity` is a sink whose only dependency is `myelin-tenancy` (identity
//! depends on nothing above tenancy). This is the build-layer realisation of the
//! `no-cross-sync-cycle` lint; P-S10 ships the real source-scanning lint.
//!
//! ## Status (P-S12 → P-010, 2026-06-19) — the `serve(AppSpec)` lifecycle is IMPLEMENTED
//! The boot → migrate → outbox relay → consumers → three ports → graceful drain lifecycle
//! (contract 1.1) is **implemented** in [`serve`]: `serve(AppSpec)` boots (validates config
//! fail-fast §3.2, opens the bounded OLTP pool §3.3), runs the forward-only migrations at boot
//! (rejecting a destructive `DROP`), auto-registers every opened store as a `PersonalDataHolder`
//! (§3.4), starts the outbox relay (P-S07) + the idempotent consumers (P-S08), opens the three
//! surfaces, exports the producer side of the contract-1.8 telemetry signal set (§3.5,
//! `outbox_depth`/`dead_letter_count`/`consumer_lag`), serves, then graceful-drains (stop intake,
//! finish in-flight, ack-then-exit; a clean drain leaves `outbox_depth == 0`). The hello-world
//! boot test (boot → emit → consume → drain) + the CDC 1.1 pair are the dated green artifact.
//!
//! ## Status (P-S13 → P-030, 2026-06-19) — the three-surface topology + tenant-from-token is DONE
//! The **tenant-from-token** mechanism (1.2, SUB-D7) is **implemented** in [`topology`]: the
//! lifecycle-opened [`topology::PublicSurface`] derives the operating tenant from the verified
//! token's `Principal`, NEVER from the URL path — a path-tenant ≠ token-tenant mismatch is
//! REJECTED ([`topology::PublicReject::CrossTenantIdor`]) and AUDITED ([`topology::AuditSink`],
//! PII-free) as a cross-tenant IDOR, and `misroute_count` stays 0. The internal RPC surface
//! ([`topology::InternalSurface`]) re-authorizes every call through the [`topology::Authorizer`]
//! seam (identity trusted, authorization re-run — "internal = safe" is not presumed). The SUB-D7
//! drill (`tests/drill_sub_d7_idor.rs`) + the CDC 1.2 pair (`tests/cdc_1_2_topology.rs`) are the
//! dated green artifact: 60 spoofs rejected + audited, 0 served (`CrossTenantCount == 0`).
//!
//! ## Status (P-S14 → P-031, 2026-06-19) — liveness ≠ readiness on the metrics-health surface DONE
//! The **liveness ≠ readiness** semantics (1.3, SUB-D9) are **implemented** in [`metrics_health`]:
//! the lifecycle-opened [`metrics_health::MetricsHealthSurface`] exposes two INDEPENDENT probes —
//! [`metrics_health::MetricsHealthSurface::liveness`] ("not wedged"; reads ONLY the process's own
//! [`metrics_health::LivenessState`], structurally incapable of checking a dependency) and
//! [`metrics_health::MetricsHealthSurface::readiness`] ("can serve correct traffic now"; a dead
//! **critical** dependency → [`metrics_health::Readiness::NotReady`] + shed; startup =
//! boot/migration incomplete → not-ready-not-killed). A severed critical dependency flips
//! readiness and sheds while liveness stays `Up` (no restart-storm). `serve` opens it in the
//! `Booting` state and [`metrics_health::MetricsHealthSurface::mark_started`]s it at the end of a
//! successful boot. The SUB-D9 drill (`tests/drill_sub_d9_liveness_readiness.rs`) + the CDC 1.3
//! pair (`tests/cdc_1_3_liveness_readiness.rs`) are the dated green artifact: a dead critical dep
//! → `readiness` gauge `1 → 0`, `liveness_restart_count == 0` (no churn). The composition with
//! **fail-static** (§8.3) is named: readiness handles the *sustained* outage, fail-static (P-S18)
//! buys the *transient* hiccup.
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The real `/livez` + `/readyz` HTTP handlers + the OTLP readiness/liveness gauge export**
//!   on the real metrics-health listener land with the real transport wiring (P-S13/P-S14+). The
//!   semantics (liveness ignores deps; readiness sheds on a dead critical dep; startup is
//!   not-ready-not-killed) are COMPLETE now; the live `DependencyHealth` probe is fed by the
//!   resilient client's breaker state (§6, P-S16) in production — here a [`metrics_health::HealthTable`]
//!   fixture + the harness dependency-break injector drive it.
//! - The real **gateway transport + mTLS/signed-internal-credential wire format + the durable
//!   tamper-evident audit sink** for the IDOR records → the gateway/listener wiring (P-S14+) and
//!   GDPR `P-GA-19`/`P-062` (the audit *consumer* reads the same PII-free
//!   [`topology::IdorAuditRecord`] shape). The substrate-side security property (tenant-from-token,
//!   re-authorize-every-call, every IDOR audited) is complete now; the wire transport is named.
//! - The [`topology::Authorizer`] body (the depth-bounded Zanzibar `check`/`list_objects`) is
//!   Identity M1 (`P-ID-09`/`P-ID-11`). Here the trait is the re-authorize-every-call SEAM.
//! ## Status (P-S15 → P-032, 2026-06-19) — holder auto-registration + the forward-only runner DONE
//! The **`PersonalDataHolder` auto-registration mechanism** (1.4) is **implemented** in
//! [`holders`]: every store the harness opens — OLTP / blob / cache / search index
//! ([`holders::StoreKind`]) — is registered through the one door, [`holders::HolderRegistry::open`]
//! (opening IS registering, so "we forgot a store" is structurally impossible, §3.4 / GD-3). The
//! **forward-only online migration runner** (1.5) is **implemented** in [`migrations`]:
//! [`migrations::MigrationRunner`] applies the embedded DDL in order at boot and REFUSES a
//! destructive (`DROP`) migration AND a blocking `ALTER` on a declared-**hot** table (§9.1/§9.4),
//! carrying the expand→backfill→contract [`migrations::MigrationPhase`] on each migration. The
//! **hot-table declaration mechanism** ([`migrations::HotTables`], the `AppSpec::hot_tables` field)
//! is the §9.4 frozen contract both the runner (at boot) and the `forward-only-migration` lint
//! (P-S11, at source-scan) read. The holder-registration + runner + lint tests are the dated green
//! artifact.
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - The env-first `Config::from_env()` parse of the real `DATABASE_URL`/broker/KMS/region knobs
//!   plus the concrete `tokio-postgres`/`sqlx` connection behind [`myelin_storage::OltpPool`] land
//!   with the driver; the bounded-pool + fast-fail semantics are complete now.
//! - The **exhaustive H1–H18 holder confirmation** against the REAL Identity/Storage/GDPR holder
//!   set is **P-S27**; here the MECHANISM auto-registers every opened store. The blob / cache /
//!   search-index holders' concrete `PersonalDataHolder` DSR bodies land with their backends
//!   (Storage M1 blob, Search M2); the OLTP holder's DSR bodies are the GDPR M1 floor (P-ST-01).
//! - **SUB-D10 (online migration under load)** — expand→backfill→contract on a restored
//!   production-scale copy under load, with no blocking lock beyond budget, plus the lock-time
//!   measurement against a restore (§9.2) — proves at **M5 (P-S34)**. The runner, the phase model,
//!   the hot-table declaration, and the destructive/blocking refusals are complete + testable at
//!   boot scale now.
//! - The per-subsystem hot-table FLAGS are **measured-not-predicted** (M1+); each high-write
//!   subsystem declares its set in its `AppSpec` as it lands (the §9.4 seed set is named).
//! - The real **OpenTelemetry meter/tracer/logger + the OTLP export + the causality+tenant
//!   trace-context middleware** (§3.5) and the **`SIGTERM`/`SIGINT` → drain** OS trigger →
//!   **P-S13/P-S14**. Here the producer is a typed in-process meter ([`serve::Telemetry`])
//!   exporting the SAME contract-1.8 `SignalName`s the harness reads, and the drain is the
//!   deterministic [`serve::ServeHandle::signal_drain`] trigger.
//! - `FailStatic<T>::get` (1.10) → **P-S18** (the mechanism) and **P-S25** (proven vs a real
//!   Identity hiccup, SUB-D4). The `static_max` VALUE is `[OPEN — LEGAL]` (DPO ratifies, L-1) —
//!   the mechanism + the `≤ revocation-SLA ≥ agent-token-TTL` constraint ship regardless; the
//!   number is a named legal floor.
//! - The failure-injection harness (load generator / dependency-break injector /
//!   telemetry-assertion library) is **P-S02–P-S04** in a separate `myelin-harness` crate. The
//!   twelve architecture lints are **P-S10/P-S11**.
//! - **Mutation floor (cargo-mutants ≥ 75% on the lifecycle module, [`serve`]).** cargo-mutants
//!   is the M6 dogfood CI gate (**P-S37**); it is not run in this prompt's environment. The
//!   lifecycle is covered by unit + CDC tests that chain boot → emit → consume → drain
//!   end-to-end (a sequence property, EI-01 §4); the mutation run is named as the M6 gate.

use serde::{Deserialize, Serialize};

pub mod crate_graph;
pub mod holders;
pub mod metrics_health;
pub mod migrations;
pub mod serve;
pub mod topology;

pub use holders::{HolderRegistration, HolderRegistry, StoreKind};
pub use metrics_health::{
    CriticalDependencies, CriticalDependency, DependencyHealth, HealthTable, Liveness,
    LivenessState, MetricsHealthSurface, Readiness, ReadinessReport, Startup,
};
pub use migrations::{
    is_blocking_alter, is_destructive, HotTables, Migration, MigrationPhase, MigrationRunner,
    Migrations,
};
pub use serve::{
    boot, serve, AppSpec, ConsumerReg, HoldersSpec, InternalRpc, OutboxSpec, PortOpener,
    PublicRoutes, ServeHandle, Surface, Telemetry,
};
pub use topology::{
    AllowPrincipal, AuditSink, Authorizer, DenyAll, IdorAuditRecord, InjectedIdentity,
    InternalReject, InternalSurface, PublicReject, PublicSurface,
};

/// Seconds (frozen unit, architecture §2.10) — the fail-static window bounds.
pub type Seconds = u64;

/// The validated, env-first service config (architecture §3.2; contract 1.1). Opaque
/// string-backed on this floor; the env-first `Config::from_env()` parse of the real
/// `DATABASE_URL`/broker/KMS/region knobs lands with the driver (**P-S15**). `serve`
/// validates it at boot (fail fast, §3.2) — see [`serve::boot`]. A config of `"BAD_POOL"`
/// models the boot-time validation-failure path the §3.2 fail-fast test exercises.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config(pub String);

/// The error type for the boot/serve lifecycle (a failed boot / failed migrate / incomplete
/// drain). A loud, typed value — a failed boot returns non-zero (architecture §3.1), never a
/// silent success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeError(pub String);

impl core::fmt::Display for ServeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ServeError {}

/// The fail-static answer (architecture §8; contract 1.10). Fail-static is the correct
/// AVAILABILITY default on a transient dependency hiccup; we NEVER fail open (the static
/// answer is coarse "actor still active / coarse grants", never an escalation of access).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer<T> {
    /// served within `fresh_ttl`.
    Fresh(T),
    /// served stale + degraded marker, between `fresh_ttl` and `static_max`.
    Static(T),
    /// past `static_max` — fail closed (the staleness budget is exhausted; deny is now
    /// correct).
    Closed,
}

/// The bounded-staleness cache (architecture §8; contract 1.10; ADR-17). On a transient
/// dependency hiccup, serve a bounded-staleness cached answer so already-authenticated
/// traffic keeps working, rather than turning one shared dependency into a whole-platform
/// cascade.
///
/// Units (frozen, §2.10): `fresh_ttl` / `static_max` are **seconds**. Constraint:
/// `static_max ≤ revocation SLA` and `static_max ≥ agent-token TTL` (the window contains
/// the short-lived agent token). **The VALUE of `static_max` is `[OPEN — LEGAL]`** — the
/// DPO ratifies it (L-1); the mechanism + the constraint ship regardless.
///
/// **Floor:** `get`'s body is `todo!()`; the mechanism lands in **P-S18**, proven vs a
/// real Identity hiccup in **P-S25** (SUB-D4).
#[derive(Clone, Debug)]
pub struct FailStatic<T> {
    /// serve fresh within this (seconds).
    pub fresh_ttl: Seconds,
    /// serve STALE (degraded marker) up to here on a hiccup (seconds);
    /// ≤ revocation SLA, ≥ agent-token TTL.
    pub static_max: Seconds,
    _marker: core::marker::PhantomData<T>,
}

impl<T> FailStatic<T> {
    /// Construct with the two frozen-unit bounds (seconds). The `static_max` value is the
    /// DPO-ratified legal floor; this constructor takes it as data, not a default.
    pub fn new(fresh_ttl: Seconds, static_max: Seconds) -> Self {
        Self {
            fresh_ttl,
            static_max,
            _marker: core::marker::PhantomData,
        }
    }

    /// On a hiccup: within `fresh_ttl` → `Fresh`; between → `Static` (degraded marker +
    /// background-refresh, stale-while-revalidate); past `static_max` → `Closed`
    /// (architecture §8; contract 1.10).
    ///
    /// **Floor:** body is `todo!()`; the mechanism lands in **P-S18**.
    pub fn get<K>(&self, _key: K, _refresh: impl Fn() -> Result<T, ServeError>) -> Answer<T> {
        todo!("the fail-static mechanism lands in P-S18; proven vs Identity in P-S25")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-asserting test: the `serve(AppSpec)` + `AppSpec{...}` field shape is frozen
    /// (contract 1.1, architecture §3.1) — the eight fields `name, config, migrations,
    /// public, internal, consumers, holders, outbox`. We construct an `AppSpec` (proving the
    /// field names) and take a fn pointer to `serve` (proving its signature). The lifecycle
    /// behaviour itself is exercised by the `serve::tests` + the CDC 1.1 integration test.
    #[test]
    fn serve_and_appspec_shape_is_frozen() {
        let spec = AppSpec {
            name: "hello",
            config: Config::default(),
            migrations: Migrations::default(),
            hot_tables: HotTables::none(),
            public: PublicRoutes::default(),
            internal: InternalRpc::default(),
            consumers: vec![],
            holders: HoldersSpec::Auto,
            outbox: OutboxSpec::default(),
            critical: CriticalDependencies::default(),
        };
        assert_eq!(spec.name, "hello");
        assert_eq!(spec.holders, HoldersSpec::Auto);
        let _f: fn(AppSpec) -> Result<(), ServeError> = serve;
    }

    /// Compile-asserting test: the `FailStatic<T>` field shape + the `Answer<T>` ladder
    /// are frozen (contract 1.10) — `fresh_ttl`/`static_max` in SECONDS, `get` returning
    /// `Fresh | Static | Closed`. We construct it and read the units; the `get` body is
    /// the P-S18 floor (not invoked).
    #[test]
    fn fail_static_shape_and_units_are_frozen() {
        // seconds (the frozen unit); the value is a placeholder — the real bound is
        // DPO-ratified (L-1), not a default set here.
        let fs: FailStatic<u8> = FailStatic::new(30, 300);
        assert_eq!(fs.fresh_ttl, 30u64);
        assert_eq!(fs.static_max, 300u64);
        assert!(fs.static_max >= fs.fresh_ttl);
        // the answer ladder exists with all three rungs (never fail-open).
        let a: Answer<u8> = Answer::Static(1);
        assert!(matches!(a, Answer::Static(_)));
        let _closed: Answer<u8> = Answer::Closed;
    }
}
