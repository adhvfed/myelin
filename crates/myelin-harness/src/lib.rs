//! # `myelin-harness` — the failure-injection / drill harness (test-support)
//!
//! **Owning doctrine:** `external-insights/01-process-and-quality-doctrine.md` §3
//! ("prove-it-or-it-isn't-real" — build the failure-injection harness EARLY: a load
//! generator that multiplies traffic 1×/10×/30× and mixes principal types, a scoped
//! reversible dependency-break, assertions read from production telemetry).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §7.2 (the
//! five principal kinds the limiter reads + the protected human lane), §7.6 (the
//! per-surface storm profiles — CI-surge / collab op-stream / connection-storm /
//! agent-mention-storm, OQ-K), §11 (the failure-injection seam).
//!
//! **Testing-strategy doc:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! §3.1 (the load generator) + §4.1 family **F6** (30× surge + protected human lane — the
//! load generator is the spine of every F6 surge drill / the SUB-D3 surge family).
//!
//! **Contracts owned:** none. The harness is test-support machinery — the driver UNDER
//! every surge/storm drill in the whole ledger, not a cross-system contract. (Per the
//! P-S02 prompt: "None owned — this is harness machinery.")
//!
//! ## DAG position
//! This crate is **NOT** a node in the production crate DAG (architecture §2.9). It is
//! test-support that sits ABOVE `myelin-substrate` as a leaf consumer (like a service's
//! `main.rs`): it depends on `myelin-identity` (for [`myelin_identity::PrincipalKind`])
//! and `myelin-tenancy` (for [`myelin_tenancy::TenantId`]), and **nothing depends on it**.
//! The substrate's `crate-graph-acyclic` test continues to model the ten production crates
//! only — adding `myelin-harness` to a production crate's `[dependencies]` would pull
//! test-support into the substrate DAG and is forbidden.
//!
//! ## What P-S02 ships
//! The **1×/10×/30× load generator** ([`load_generator`]): a driver that issues traffic at
//! a configurable [`Multiplier`] with a configurable [`PrincipalMix`] across the five
//! [`LoadPrincipalKind`]s (human / agent / service / CI / external-MCP) and the four named
//! [`StormProfile`]s. The generator targets an abstract [`Sink`] (an in-memory request
//! handler in tests; later drills point it at a real `serve` instance).
//!
//! ## What P-S03 ships (this prompt)
//! The **scoped-reversible dependency-break injector** ([`dependency_break`]): the T-3 seam
//! every later drill rides to force ONE [`Dependency`] to fail for ONE [`Scope`] without
//! taking the rig down. [`DependencyBreaker::break_dependency`] /
//! [`DependencyBreaker::restore_dependency`] are **reversible** (a broken dep restores to a
//! fully working state), **scoped** (a break touches only its named dependency + scope —
//! never an unrelated dependency or another tenant/cell), and **idempotent** (a
//! double-break / double-restore is a no-op, observably via [`BreakOutcome::NoChange`]). It
//! is the failure-injection half of the unit-of-proof: the load generator (P-S02) drives
//! traffic, this injector severs a dependency, and the telemetry-assertion library (P-S04)
//! reads that the system survived.
//!
//! ## Floors named (deferred + filling prompt)
//! - **Storm-profile parameters are v1 defaults.** The tuned per-surface shed-budget
//!   numbers are the M5 surge / connection-storm follow-on (**P-S32 / P-S33**;
//!   architecture §7.6 names them as floors tuned by the drills, not claimed-final). Here
//!   each profile carries a v1 default shape so the generator selects the right surface
//!   behaviour; the *numbers* tighten at M5.
//! - **No runtime survival drill yet.** The telemetry-assertion library (the thing a drill
//!   asserts against) lands in **P-S04**; this prompt's gate is the injector's OWN
//!   reversibility + scoping + idempotence, and the generator's OWN correctness. The
//!   assertion-backed survival drills the injector drives are wired at the prompts that own
//!   each fault-point: SUB-D2 at **P-S07/P-S08** (sever the broker), SUB-D4 at **P-S25**
//!   (hard-down Identity / fail-static), SUB-D5 at **P-S16/P-S17** (trip a downstream).
//! - **Consult seam, not a real process-killer.** The injector models a break as shared
//!   queryable state ([`DependencyBreaker::is_broken`]); the real fault-points that consult
//!   it (the relay's publish, `AuthzClient::check`, a downstream RPC) do not exist until
//!   **P-S07 / P-S12 / P-S13 / P-S16** — each wires its fault-point to this consult then.
//! - **Abstract sink only.** The generator drives an in-memory [`Sink`]; pointing it at a
//!   real three-port `serve` instance lands once `serve` exists (**P-S12 / P-S13**).

pub mod dependency_break;
pub mod load_generator;

pub use dependency_break::{BreakOutcome, Dependency, DependencyBreaker, Scope};
pub use load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, Request, RunClass, Sink,
    StormProfile, Surface,
};
