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
//! ## Floors named (stubbed bodies → filling prompt)
//! All bodies are `todo!()`. The harness lifecycle lands across the M0 substrate prompts:
//! - `serve` boot→migrate→relay→consumers→ports→drain (1.1) → **P-S12**; the three-surface
//!   topology + tenant-from-token (1.2, SUB-D7) → **P-S13**; liveness ≠ readiness (1.3,
//!   SUB-D9) → **P-S14**; the migration runner + holder auto-registration (1.4/1.5) →
//!   **P-S15**.
//! - `FailStatic<T>::get` (1.10) → **P-S18** (the mechanism) and **P-S25** (proven vs a
//!   real Identity hiccup, SUB-D4). The `static_max` VALUE is `[OPEN — LEGAL]` (DPO
//!   ratifies, L-1) — the mechanism + the `≤ revocation-SLA ≥ agent-token-TTL` constraint
//!   ship regardless; the number is a named legal floor.
//! - The failure-injection harness (load generator / dependency-break injector /
//!   telemetry-assertion library) is **P-S02–P-S04** in a separate `myelin-harness`
//!   crate. The twelve architecture lints are **P-S10/P-S11**.
//!
//! ## GATE FLOOR (named explicitly, per the prompt)
//! P-001 ships the **skeleton only** — there is **no quantified runtime drill** at this
//! prompt (it ships types/traits, not behaviour). The green artifact is a clean
//! `cargo build --workspace` + `cargo test --workspace` log plus the
//! `crate-graph-acyclic` test. The runtime survival drills (SUB-D1..D10) are greened by
//! the later prompts named above.

use serde::{Deserialize, Serialize};

pub mod crate_graph;

/// Seconds (frozen unit, architecture §2.10) — the fail-static window bounds.
pub type Seconds = u64;

/// The validated, env-first service config (architecture §3.2; contract 1.1). Opaque in
/// the skeleton; `Config::from_env()` + boot-time validation land in P-S12.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config(pub String);

/// The forward-only embedded migration set (architecture §3.1, §9; contract 1.5). Opaque
/// in the skeleton; the runner lands in P-S15.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Migrations(pub Vec<String>);

/// The public surface route set (architecture §4.1; contract 1.2). Opaque in the
/// skeleton; the gateway-fronted, tenant-from-token topology lands in P-S13.
#[derive(Clone, Debug, Default)]
pub struct PublicRoutes(pub ());

/// The internal RPC surface (architecture §4.2; contract 1.2). Opaque in the skeleton;
/// re-authorize-every-call lands in P-S13.
#[derive(Clone, Debug, Default)]
pub struct InternalRpc(pub ());

/// A registered event consumer (architecture §5; contract 2.4). Opaque handle in the
/// skeleton; the consumer runtime lands in P-S08 and is wired by `serve` in P-S12.
#[derive(Clone, Debug, Default)]
pub struct ConsumerReg(pub ());

/// How the harness registers `PersonalDataHolder`s (architecture §3.4; contract 1.4).
/// `Auto` means every store the harness opens is auto-registered (GD-3). Wired in
/// P-S12/P-S15.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HoldersSpec {
    /// every opened store auto-registered (the §3.1 `AppSpec::auto`).
    #[default]
    Auto,
}

/// The outbox relay spec (architecture §3.3; contract 2.3). `Default` = relay started
/// automatically. The relay lands in P-S07; `serve` starts it in P-S12.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutboxSpec(pub ());

/// The one spec a service's `main.rs` supplies (architecture §3.1; contract 1.1). The
/// harness owns the lifecycle around it: boot → migrate → relay → consumers → three
/// ports → graceful drain.
#[derive(Clone, Debug, Default)]
pub struct AppSpec {
    pub name: &'static str,
    pub config: Config,
    pub migrations: Migrations,
    pub public: PublicRoutes,
    pub internal: InternalRpc,
    pub consumers: Vec<ConsumerReg>,
    pub holders: HoldersSpec,
    pub outbox: OutboxSpec,
}

/// Placeholder error for the skeleton (failed boot / failed migrate). Real taxonomy lands
/// with P-S12.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeError(pub String);

/// The ONE call (architecture §3.1; contract 1.1). Blocks, owning the lifecycle: boot →
/// migrate → start outbox relay → start consumers → open the three ports → serve until
/// signalled → graceful drain (stop intake, finish in-flight, ack-then-exit). Non-zero on
/// failed boot.
///
/// **Floor:** body is `todo!()`; the lifecycle lands in **P-S12** (drain), **P-S13**
/// (three ports + tenant-from-token, SUB-D7), **P-S14** (liveness ≠ readiness, SUB-D9),
/// **P-S15** (migration runner + holder auto-registration).
pub fn serve(_spec: AppSpec) -> Result<(), ServeError> {
    todo!("the serve lifecycle lands across P-S12..P-S15")
}

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
    /// public, internal, consumers, holders, outbox`. We construct an `AppSpec` (proving
    /// the field names) and take a fn pointer to `serve` (proving its signature) without
    /// invoking its `todo!()` body.
    #[test]
    fn serve_and_appspec_shape_is_frozen() {
        let spec = AppSpec {
            name: "hello",
            config: Config::default(),
            migrations: Migrations::default(),
            public: PublicRoutes::default(),
            internal: InternalRpc::default(),
            consumers: vec![],
            holders: HoldersSpec::Auto,
            outbox: OutboxSpec::default(),
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
