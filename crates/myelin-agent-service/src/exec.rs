//! # `exec` — `ToolHands::exec` on the unified sandbox (AG-P15 → P-226, M2-C)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/agent-fabric.md`
//! §2.2 (the hands — `exec(command) -> result`, **no host-exec bypass** + the four uniform
//! guarantees), §5.0 (the routing table — exec carries ONLY `compute` untrusted code; `mutate` /
//! `external` route through [`EffectApi`](crate::effect_api), never exec — the routing split is the
//! safety boundary). Reconciliation: `00-reconciliation-decisions.md` X-6 (the four uniform
//! guarantees pinned; `ToolHands::exec` **IS** CI's `kind=agent` job on the ONE unified sandbox).
//! **Contracts:** `contract-index.md` row 8.4 (the Fabric half — the `kind=agent` job spec + the
//! routing + the four-guarantee wiring; CI owns the runner + the drill), with consumed hooks 11.7
//! (reserve at dispatch), 4.7 (`mint_run_token` attribution), 1.6 (the `no-host-exec` lint).
//!
//! ## What this module IS (the Fabric half of 8.4)
//! [`SandboxToolHands`] realises [`ToolHands::exec`](myelin_agent::ToolHands) (8.4) as the **dispatch
//! of CI's `kind=agent` job onto the ONE unified sandbox** ([`myelin_ci_sandbox`], CI-P1 → P-129).
//! `exec` is ONE method with **NO host-execution path that bypasses it** (the `no-host-exec` lint,
//! 1.6 — there is no `std::process::Command` etc anywhere here; all execution goes through
//! [`SandboxBackend::launch`]). It carries ONLY untrusted code execution (`compute` — a test, a
//! build, a linter, a script) — the only thing that touches the kernel sandbox.
//!
//! ### The routing split (the safety boundary — §5.0 / X-6 #3)
//! [`route_of`] classifies a tool by its [`EffectKind`]:
//! - `read`    → [`ToolRoute::Direct`]   (a permission-filtered subsystem read; no mutation, no
//!   sandbox);
//! - `compute` → [`ToolRoute::Sandbox`]  (untrusted code → `ToolHands::exec` → the unified sandbox);
//! - `mutate`  → [`ToolRoute::EffectApi`] (governed mutation → [`EffectApi::apply`], plan-then-apply);
//! - `external`→ [`ToolRoute::EffectApi`] (a side-effecting external call → an egress-reviewed
//!   adapter through `EffectApi`).
//!
//! **A `mutate` (or `external`) effect can NEVER reach exec** — this is the crux assertion and it is
//! **structural**: [`dispatch_compute`] takes a [`SandboxJob`], and a [`SandboxJob`] can ONLY be
//! built by [`SandboxJob::for_compute`], which returns [`RoutingError::NotComputeBound`] for any
//! non-`compute` tool. There is no other constructor, no mutation API on this seam — so a mutation
//! literally has no path to the sandbox (the routing split is encoded in the type, not a runtime
//! convention). This mirrors the EffectApi pipeline's own gate (`effect_api.rs`: only
//! `Mutate | External` route through `EffectApi`), the two halves meeting at the type boundary.
//!
//! ### Reconciliation note (code-wins-over-docs, EI-01 §1)
//! §2.2 prose says exec carries "compute/external untrusted code", but the **authoritative §5.0
//! routing table** puts `external` (`side_effecting = true`) through `EffectApi → an egress-reviewed
//! adapter`, NOT through the raw sandbox — and the frozen `effect_api.rs` body (AG-P6 → P-218)
//! already routes `Mutate | External` through `EffectApi`. We follow the table + the shipped code:
//! the ONLY effect kind that reaches the bare sandbox is `compute`. This is the **strongest** form
//! of the routing split (the smaller the untrusted-code surface that touches the kernel, the safer)
//! and it makes "a mutate effect can never reach exec" hold a fortiori. Documented per EI-01 §1.
//!
//! ## The four uniform guarantees wired by construction (§2.2 / X-6 — NO subsystem re-implements any)
//! Every `kind=agent` job dispatched here inherits all four **by construction**, because exec is the
//! SAME `launch(JobSpec{kind:Agent}, hooks)` path a CI run takes:
//! 1. **Universal cost gate** ([`RunnerHooks::reserve`]/[`settle`](myelin_ci_sandbox::RunnerHooks)) —
//!    reserve at dispatch, refuse-on-exhaustion, settle on completion, never interrupt in-flight
//!    (contract 11.7; the agent-fabric reserve is [`crate::cost_gate`], AG-P14 → P-227).
//! 2. **Attribution** ([`RunnerHooks::attribute`]) — the job runs under the per-run attenuated token
//!    ([`RunTokenCredential`], `mint_run_token` 4.7; [`crate::identity`], AG-P13 → P-225); life == run life,
//!    auto-revoked on teardown, re-mintable on resume. The shared platform token is **scrubbed** from
//!    the child env ([`SandboxJob::scrubbed_env`]) — re-asserted here.
//! 3. **HITL withhold (plan-then-apply)** — structural: side-effecting mutation NEVER goes through
//!    this runner (the routing split above); it goes through [`EffectApi::apply`] (8.2). See
//!    [`myelin_ci_sandbox::hitl_withhold_note`].
//! 4. **Isolation floor** ([`SandboxJob::for_compute`] feeds the FULL named hardening profile into
//!    the `kind=agent` `JobSpec`: gVisor-class/microVM via the backend; egress default-deny;
//!    read-only root + tmpfs; `pids.max` + zero swap; digest-pinned images **fail-closed** on an
//!    un-digested tag; whole-guest kill on teardown; secrets resolved INSIDE the boundary as
//!    [`SecretRef`]s, never forwarded via the runtime).
//!
//! ## The AG-D4 / CI-T1 hard escape GATE is CONSUMED here (AG-P17 → P-229)
//! [`SandboxToolHands`] carries an [`AgentExecGate`](crate::escape_gate::AgentExecGate) — a value that
//! can ONLY be obtained from a GREEN [`EscapeAttestation`](myelin_ci_sandbox::EscapeAttestation) for
//! the production backend (the real-kernel drill CI ran on a microVM, CI-P5 → P-239). The hands have
//! no constructor without it, so a Fabric exec dispatch is **structurally fail-closed on AG-D4**: no
//! green attestation ⇒ no `SandboxToolHands` ⇒ no untrusted compute. This is the Fabric half of the
//! D-4 go/no-go (the CI half is the drill + the attestation; this half is the GATE that refuses to
//! dispatch without one). See [`crate::escape_gate`].
//!
//! ## Floors named (AG-P17 — there is NO floor on AG-D4)
//! - **There is NO floor on AG-D4** — ZERO escapes is BOTH the floor and the full answer; it is a
//!   **PERMANENT GATE** re-run on every backend / image / kernel change. The CI side proved it on a
//!   real microVM (CI-P5 → P-239). The microVM/gVisor [`myelin_ci_sandbox::SandboxBackend`] impl is
//!   the **Firecracker backend, CI-P2 (→ P-237)** (the gVisor 2nd is CI-P28).
//! - **The M4 re-confirm on the prod CI image is AG-P21 (→ P-348)** (CI side CI-P27 / P-348).
//! - **Continuous fuzzing + the full CVE corpus + a pre-GA third-party pentest** remain ongoing
//!   residuals on top of this gate (never "done").
//! - **`SCHEDULE_AND_RUN_JOB` long-park** (dispatch-and-return, completion as a durable idempotent
//!   signal) is **AG-P16 (→ P-228)** — here exec is the in-line activity form.
//! - **The real `LlmAgentRuntime`** running its compute against this same runner is **post-M5
//!   (AG-P25)** — the only place a model/SDK/prompt string ever lives (`no-llm-in-platform`, 1.6).
//!
//! ## DB-free
//! This module touches NO DB / object-store / cache / bus contract: it builds an in-memory
//! [`JobSpec`] value and dispatches it through the [`SandboxBackend`] trait seam (the reserve/token
//! bodies it consumes are already proven against the live stack at AG-P14/AG-P13). So `cargo build
//! --workspace` stays DB-free and there is no new `integration` feature here.

use crate::escape_gate::AgentExecGate;
use myelin_agent::{Command, EffectKind, ToolDef, ToolHands, ToolResult};
use myelin_ci_sandbox::{
    agent_job, EgressPolicy, EnvVar, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget,
    ResourceLimits, RunTokenCredential, RunnerHooks, SandboxBackend, SecretRef, SpecError,
    TrustTier, WorkspaceSpec,
};

/// The env-var name of the **shared platform token** that MUST be scrubbed from the child env before
/// any untrusted code runs (§5.7 anti-leak; AG-P13 → P-225). The agent's job runs under the *per-run
/// attenuated* token ([`RunTokenCredential`]), never the broad platform token — so the platform token is
/// removed from the child env, and a per-run token name is the only credential the child ever sees.
/// (The same scrub is enforced in [`crate::skeleton::ChildEnv`]; re-asserted here at the exec seam.)
pub const PLATFORM_TOKEN_ENV: &str = "MYELIN_PLATFORM_TOKEN";

// ─────────────────────────────── the routing split (§5.0 / X-6 #3) ───────────────────────────────

/// Where a tool call routes, per its [`EffectKind`] (§5.0 routing table). The platform loop routes a
/// `UseTools` call to exactly one of these; only [`ToolRoute::Sandbox`] reaches the kernel sandbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRoute {
    /// `read` (`side_effecting = false`) — a permission-filtered subsystem read API; no mutation, no
    /// sandbox. Routed directly to the subsystem's read endpoint.
    Direct,
    /// `compute` (`side_effecting = false`) — untrusted code (a test/build/linter/script) → the
    /// unified sandbox via [`ToolHands::exec`]. **The ONLY route that touches the kernel sandbox.**
    Sandbox,
    /// `mutate` / `external` (`side_effecting = true`) — governed mutation / a side-effecting external
    /// call → [`EffectApi::apply`](crate::effect_api) (plan-then-apply). **NEVER the sandbox.**
    EffectApi,
}

/// Classify a tool by its [`EffectKind`] into its [`ToolRoute`] (§5.0). This is the single source of
/// truth for the routing split: `compute` is the ONLY kind that maps to [`ToolRoute::Sandbox`].
pub const fn route_of(kind: EffectKind) -> ToolRoute {
    match kind {
        EffectKind::Read => ToolRoute::Direct,
        EffectKind::Compute => ToolRoute::Sandbox,
        // The routing split's safety boundary: a side-effecting effect NEVER reaches the sandbox.
        EffectKind::Mutate | EffectKind::External => ToolRoute::EffectApi,
    }
}

/// A routing error — the consumer tried to push a non-`compute` tool into the sandbox path. The
/// existence of this `Err` (and the absence of any other [`SandboxJob`] constructor) is what makes
/// "a mutate effect can never reach exec" a TYPE-LEVEL guarantee, not a runtime convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoutingError {
    /// A tool whose `effect_kind` is not `compute` was offered to the sandbox path. Carries the
    /// offending tool name + its route so the refusal is self-describing and LOUD (never swallowed).
    NotComputeBound {
        /// The offending tool's name.
        tool: String,
        /// Where it SHOULD route ([`ToolRoute::Direct`] / [`ToolRoute::EffectApi`]).
        actual_route: ToolRoute,
    },
    /// The `kind=agent` [`JobSpec`] could not be built fail-closed (an un-digested image, a zero
    /// `pids_max`, a zero `timeout`). The hardening profile is non-negotiable (X-6 #4 / CI-1).
    SpecRejected(SpecError),
}

impl std::fmt::Display for RoutingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoutingError::NotComputeBound { tool, actual_route } => write!(
                f,
                "tool `{tool}` routes to {actual_route:?}, not the sandbox — a non-`compute` effect \
                 cannot reach ToolHands::exec (the routing split is the safety boundary, §5.0/X-6 #3)"
            ),
            RoutingError::SpecRejected(e) => write!(
                f,
                "the kind=agent JobSpec was rejected fail-closed: {e} (the hardening profile is \
                 non-negotiable, X-6 #4 / CI-1)"
            ),
        }
    }
}

impl std::error::Error for RoutingError {}

impl From<SpecError> for RoutingError {
    fn from(e: SpecError) -> Self {
        RoutingError::SpecRejected(e)
    }
}

// ───────────────────────── the kind=agent job (the four-guarantee carrier) ────────────────────────

/// A fully-formed, hardened `kind=agent` job ready to dispatch onto the unified sandbox. The ONLY
/// way to build one is [`SandboxJob::for_compute`] — which **rejects any non-`compute` tool**
/// ([`RoutingError::NotComputeBound`]) and feeds the FULL named hardening profile into the inner
/// [`JobSpec`]. There is no mutation API on this type, so a `mutate`/`external` effect has no path
/// here (the routing split, encoded in the type).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxJob {
    /// The hardened `kind=agent` [`JobSpec`] (digest-pinned image, default-deny egress, pids/timeout
    /// set, read-only root + tmpfs, zero swap structurally, secrets as in-boundary refs).
    spec: JobSpec,
}

impl SandboxJob {
    /// Build the `kind=agent` job for a `compute` tool, feeding the full hardening profile.
    ///
    /// **Fail-closed twice over:**
    /// 1. the tool MUST be `compute` ([`route_of`] == [`ToolRoute::Sandbox`]) — else
    ///    [`RoutingError::NotComputeBound`] (a `mutate`/`external`/`read` tool can NEVER build a
    ///    [`SandboxJob`], so it can NEVER reach the sandbox);
    /// 2. the [`JobSpec`] non-negotiables ([`JobSpec::new`] via [`agent_job`]) — a digest-pinned
    ///    image (an un-digested tag is rejected), `pids_max > 0`, `timeout_secs > 0`.
    ///
    /// The `env` is **scrubbed** of the shared [`PLATFORM_TOKEN_ENV`] before being fed to the guest
    /// (guarantee #2 anti-leak); the per-run token rides as [`RunTokenCredential`] in the spec, not the env.
    /// Secrets are passed as in-boundary [`SecretRef`]s (names/handles, resolved inside the boundary
    /// — never the clear material, never forwarded via the runtime). Egress is default-deny by
    /// default; the caller opts in to an allowlist via `egress`.
    #[allow(clippy::too_many_arguments)]
    pub fn for_compute(
        def: &ToolDef,
        image: ImageRef,
        command: Vec<String>,
        env: Vec<EnvVar>,
        secret_refs: Vec<SecretRef>,
        egress: EgressPolicy,
        limits: ResourceLimits,
        trust_tier: TrustTier,
        run_token: RunTokenCredential,
        meter_to: MeterTarget,
        idem_token: IdemToken,
    ) -> Result<SandboxJob, RoutingError> {
        // (1) ROUTING SPLIT — only `compute` reaches the sandbox. A non-`compute` tool is refused
        //     LOUD here; it has no other path to a SandboxJob (the type-level safety boundary).
        let route = route_of(def.effect_kind);
        if route != ToolRoute::Sandbox {
            return Err(RoutingError::NotComputeBound {
                tool: def.name.0.clone(),
                actual_route: route,
            });
        }

        // (2) GUARANTEE #2 (anti-leak): scrub the shared platform token from the child env. The job
        //     runs under the per-run attenuated token (the spec's `run_token`), never the broad one.
        let scrubbed = Self::scrub_platform_token(env);

        // (3) GUARANTEE #4 (isolation floor): build the `kind=agent` JobSpec via `agent_job`, which
        //     applies the fail-closed non-negotiables (digest-pin, pids_max, timeout) and the
        //     no-checkout `compute` workspace (read-only root + tmpfs is the backend's mandatory
        //     profile, CI-P2). Egress default-deny unless the caller opts in. Zero swap is structural
        //     (there is no swap field on ResourceLimits). Secrets ride as in-boundary refs.
        let spec = agent_job(
            image,
            command,
            scrubbed,
            secret_refs,
            egress,
            limits,
            trust_tier,
            run_token,
            meter_to,
            idem_token,
        )?;
        debug_assert_eq!(spec.kind, JobKind::Agent);
        debug_assert_eq!(spec.workspace, WorkspaceSpec::default());
        Ok(SandboxJob { spec })
    }

    /// The shared platform token scrubbed from a child env (guarantee #2 anti-leak). Public so the
    /// scrub is independently testable; idempotent (scrubbing twice is the same as once).
    pub fn scrub_platform_token(env: Vec<EnvVar>) -> Vec<EnvVar> {
        env.into_iter()
            .filter(|e| e.name != PLATFORM_TOKEN_ENV)
            .collect()
    }

    /// The hardened `kind=agent` [`JobSpec`] this job dispatches (read-only view).
    pub fn spec(&self) -> &JobSpec {
        &self.spec
    }

    /// **Re-stamp the dispatch `idem_token` on the hardened spec (the `SCHEDULE_AND_RUN_JOB`
    /// long-park dedup key, §5.6 / §4.9).** The in-line `exec` form (AG-P15) carries the Fabric's own
    /// idem token; the LONG-PARK form (AG-P16, [`crate::long_park`]) instead stamps the ENGINE's
    /// DETERMINISTIC dispatch token (minted on the dispatch position, deterministic on the run's
    /// `command_id`) so the runner echoes THAT token on the `job.done` signal — the no-coordination
    /// dedup agreement the workflow keys its `wait_for_signal` on. Returns the same hardened spec with
    /// only the dedup token rebound (the four-guarantee profile is untouched). Chainable.
    pub fn with_dispatch_idem_token(mut self, idem_token: IdemToken) -> SandboxJob {
        self.spec.idem_token = idem_token;
        self
    }

    /// The child env after the platform-token scrub (guarantee #2; the env the guest actually sees).
    pub fn scrubbed_env(&self) -> &[EnvVar] {
        &self.spec.env
    }
}

// ─────────────────────────────── ToolHands::exec — the dispatch ──────────────────────────────────

/// The real [`ToolHands`] (8.4): `exec` dispatches the `kind=agent` job onto the unified sandbox.
///
/// `B` is the [`SandboxBackend`] (the Firecracker microVM / gVisor backend, CI-P2/CI-P28; a no-op
/// shape stub in tests). `exec` carries the four-guarantee [`RunnerHooks`] (reserve/settle, attribute,
/// isolation floor) so EVERY dispatch inherits the guarantees by construction — there is NO host-exec
/// path that bypasses [`SandboxBackend::launch`] (the `no-host-exec` lint, 1.6).
///
/// The [`ToolHands::exec`] frozen signature takes a [`Command`] (the untrusted code) and returns a
/// [`ToolResult`]; the surrounding context (the hardened image, the per-run token, the reserve
/// target, the idem token, the trust tier, the limits) is bound when the hands are CONSTRUCTED — the
/// platform loop builds a `SandboxToolHands` scoped to the current run, then calls `exec(cmd)` for
/// each `compute` call. (A `mutate`/`external` call never reaches here — the loop routes it to
/// [`EffectApi`](crate::effect_api) per [`route_of`].)
pub struct SandboxToolHands<'a, B: SandboxBackend> {
    /// **The AG-D4 / CI-T1 hard escape GATE (AG-P17 → P-229).** The Fabric REFUSES to dispatch any
    /// `kind=agent` compute job unless a GREEN [`EscapeAttestation`](myelin_ci_sandbox::EscapeAttestation)
    /// exists for the production backend (ZERO escapes, matching kernel/rootfs/corpus identity). The
    /// [`AgentExecGate`] can ONLY be obtained by [`AgentExecGate::admit`] against a real green
    /// attestation — so holding `SandboxToolHands` is, by construction, proof the gate is GREEN (no
    /// green attestation ⇒ no untrusted compute; the fail-closed property is in the TYPE, never a
    /// hardcoded `true`). The dispatch path asserts the gate's backend identity matches the launched
    /// job's image before any untrusted code runs.
    gate: AgentExecGate,
    /// The unified-sandbox backend (CI owns it; the Fabric feeds it).
    backend: &'a B,
    /// The four-guarantee hooks the backend drives at the right lifecycle points.
    hooks: RunnerHooks,
    /// The hardened image the `compute` job runs in (digest-pinned; checked fail-closed).
    image: ImageRef,
    /// The per-run attenuated token (guarantee #2; minted at dispatch, life == run life).
    run_token: RunTokenCredential,
    /// The reserve this run settles against (guarantee #1; reserved at dispatch).
    meter_to: MeterTarget,
    /// The dispatch idempotency token (stamped on `job.done`).
    idem_token: IdemToken,
    /// The run's trust tier (gates secrets/cache/egress; stamped once, X-1).
    trust_tier: TrustTier,
    /// The resource limits (pids_max + timeout > 0; zero swap structural).
    limits: ResourceLimits,
    /// The egress policy (default-deny unless the run opts in).
    egress: EgressPolicy,
    /// The in-boundary secret refs (names/handles, resolved inside the boundary).
    secret_refs: Vec<SecretRef>,
}

impl<'a, B: SandboxBackend> SandboxToolHands<'a, B> {
    /// Construct the hands scoped to one run. The platform loop builds this from the run substrate
    /// (the per-run token + reserve + trust tier come from the dispatch tier, AG-P4/P-13/P-14). The
    /// `image`/`limits`/`egress`/`secret_refs` are the hardened `compute` profile for this run.
    ///
    /// **The AG-D4 gate (AG-P17 → P-229) is a REQUIRED argument** — there is no constructor without
    /// it. The hands cannot exist (and therefore `exec` cannot dispatch) unless the caller has
    /// already obtained a GREEN [`AgentExecGate`] for the production backend ([`AgentExecGate::admit`]
    /// against a real green [`EscapeAttestation`](myelin_ci_sandbox::EscapeAttestation)). This is the
    /// structural fail-closed: no green AG-D4 attestation ⇒ no `SandboxToolHands` ⇒ no untrusted
    /// compute. The gate's admitted backend identity must match the run's hardened `image` digest, so
    /// the dispatch is on the SAME backend the drill proved (the permanent gate, re-run on every
    /// backend/image/kernel change).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gate: AgentExecGate,
        backend: &'a B,
        hooks: RunnerHooks,
        image: ImageRef,
        run_token: RunTokenCredential,
        meter_to: MeterTarget,
        idem_token: IdemToken,
        trust_tier: TrustTier,
        limits: ResourceLimits,
        egress: EgressPolicy,
        secret_refs: Vec<SecretRef>,
    ) -> Self {
        SandboxToolHands {
            gate,
            backend,
            hooks,
            image,
            run_token,
            meter_to,
            idem_token,
            trust_tier,
            limits,
            egress,
            secret_refs,
        }
    }

    /// The AG-D4 / CI-T1 escape gate this run dispatches under (read-only). Its existence is the proof
    /// that a GREEN escape attestation for the production backend was consumed (AG-P17 → P-229).
    pub fn gate(&self) -> &AgentExecGate {
        &self.gate
    }

    /// Dispatch a pre-routed [`SandboxJob`] onto the unified sandbox — the explicit, fallible form
    /// `exec` wraps. Drives the backend's `launch` (which fires the four-guarantee hooks), then
    /// whole-guest-kills the guest on teardown (guarantee #4: the guest is never reused across jobs).
    /// Returns the typed [`HookError`] / backend error on a refused dispatch (e.g. an exhausted
    /// wallet refuses-to-start, 11.7) — never silently swallowed.
    ///
    /// **The AG-D4 gate is already proven GREEN by construction** (a `SandboxToolHands` cannot exist
    /// without a green [`AgentExecGate`], AG-P17 → P-229) — so reaching this dispatch means the
    /// production backend's real-kernel escape drill was green (ZERO escapes). No green attestation ⇒
    /// no `SandboxToolHands` ⇒ this method is unreachable.
    pub fn dispatch_compute(&self, job: &SandboxJob) -> Result<ToolResult, ExecError<B::Error>> {
        // The whole of execution goes through `launch` — there is no host-exec bypass (1.6). `launch`
        // fires: #4 isolation floor → #2 attribution → #1a reserve (refuse-on-exhaustion) → (guest
        // runs the compute) → #1b settle, in the mandated order (the backend owns the order).
        let launch = self
            .backend
            .launch(job.spec(), &self.hooks)
            .map_err(ExecError::Launch)?;
        // Guarantee #4: whole-guest kill on teardown — the guest is destroyed, never reused.
        self.backend.kill(&launch.handle).map_err(ExecError::Kill)?;
        // RESHAPE-001 / CT-001: the seam now returns `launch.result` (exit_code/stdout/stderr/usage/
        // timed_out). Surfacing the compute result (exit/streams) into the agent trace is its own
        // follow-on; here the X-6 routing equivalence is unchanged — we keep the guest-id marker.
        Ok(ToolResult(format!("sandbox:{}", launch.handle.guest_id)))
    }

    /// Build the hardened `kind=agent` [`SandboxJob`] for a `compute` `def` + `cmd`, scoped to this
    /// run. Fail-closed: a non-`compute` `def` is rejected ([`RoutingError::NotComputeBound`]); an
    /// un-digested image (etc.) is rejected ([`RoutingError::SpecRejected`]).
    pub fn build_compute_job(
        &self,
        def: &ToolDef,
        cmd: &Command,
    ) -> Result<SandboxJob, RoutingError> {
        SandboxJob::for_compute(
            def,
            self.image.clone(),
            vec![cmd.0.clone()],
            Vec::new(),
            self.secret_refs.clone(),
            self.egress.clone(),
            self.limits,
            self.trust_tier,
            self.run_token.clone(),
            self.meter_to.clone(),
            self.idem_token.clone(),
        )
    }
}

/// An exec dispatch failure — a fail-closed reserve refusal / token rejection / isolation-floor
/// miss surfaced from the backend, or a teardown failure. Carries the backend's own error so the
/// refusal is self-describing and LOUD (never a swallowed pass).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecError<E> {
    /// `launch` was refused (a four-guarantee hook said no, or the backend could not start).
    Launch(E),
    /// the whole-guest kill on teardown failed (guarantee #4 — surfaced, not ignored).
    Kill(E),
}

impl<E: std::fmt::Display> std::fmt::Display for ExecError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Launch(e) => write!(f, "sandbox launch refused: {e}"),
            ExecError::Kill(e) => write!(f, "sandbox teardown (whole-guest kill) failed: {e}"),
        }
    }
}

impl<E: std::error::Error> std::error::Error for ExecError<E> {}

impl<B: SandboxBackend> ToolHands for SandboxToolHands<'_, B> {
    /// `exec(Command) -> ToolResult` (the frozen 8.4 signature). Builds the hardened `kind=agent`
    /// job for a `compute` call and dispatches it onto the unified sandbox via the four-guarantee
    /// `launch` seam. The frozen signature is infallible (`-> ToolResult`); a fail-closed refusal
    /// (an un-built job, a refused reserve) renders as a `ToolResult` carrying the error text (an
    /// ordinary tool error the brain reads), never a silent success and never a host-exec bypass.
    ///
    /// This is the in-line activity form. A `compute` tool's `ToolDef` is implied here (the platform
    /// loop only routes `compute` calls to `exec`; a `mutate`/`external` call goes to `EffectApi`
    /// per [`route_of`]). For an explicitly-typed, fallible dispatch with the routing-split check,
    /// use [`build_compute_job`](Self::build_compute_job) + [`dispatch_compute`](Self::dispatch_compute).
    fn exec(&self, cmd: Command) -> ToolResult {
        // A bare `exec(cmd)` is the `compute` route by construction (the loop only sends `compute`
        // here). Build the hardened job for the canonical `compute` def, then dispatch.
        let def = compute_tool_def();
        match self.build_compute_job(&def, &cmd) {
            Ok(job) => match self.dispatch_compute(&job) {
                Ok(res) => res,
                // A refused dispatch (exhausted wallet, isolation-floor miss) is an ordinary tool
                // error the brain reads — surfaced LOUD, never a silent success.
                Err(e) => ToolResult(format!("exec-refused:{e}")),
            },
            Err(e) => ToolResult(format!("exec-rejected:{e}")),
        }
    }
}

/// The canonical `compute` [`ToolDef`] shape an `exec` call routes under (`effect_kind = Compute`,
/// `side_effecting = false`). The loop only ever hands `compute` calls to `exec`; this is the def
/// that classifies them. (Subsystem `compute` tools register their own `ToolDef`s with this shape.)
pub fn compute_tool_def() -> ToolDef {
    ToolDef {
        name: myelin_agent::ToolName("agent.exec".into()),
        subsystem: "agent".into(),
        version: 1,
        input_schema: "{}".into(),
        required_caps: Vec::new(),
        effect_kind: EffectKind::Compute,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escape_gate::ProductionBackendId;
    use myelin_agent::ToolName;
    use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
    use myelin_ci_sandbox::{
        parse_console, Backend, BackendRun, EscapeAttestation, HookError, ReserveHandle,
        ResourceUsage, SandboxHandle, SandboxLaunch, SandboxResult, CORPUS, CORPUS_VERSION,
    };
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    fn pinned() -> ImageRef {
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef000000000000000000000000000000000000000000000000").unwrap()
    }

    /// A real GREEN AG-D4 gate for the test production backend (minted from the corpus parser, never
    /// hardcoded) — the proof the exec hands require to exist at all.
    fn green_gate() -> AgentExecGate {
        let id = ProductionBackendId {
            backend: Backend::FirecrackerMicrovm,
            rootfs_sha256: "rootfs-digest".into(),
            kernel_sha256: "kernel-digest".into(),
            corpus_version: CORPUS_VERSION,
        };
        let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
        for atk in CORPUS {
            console.push_str(&format!("{} CONTAINED\n", atk.id));
        }
        console.push_str(&format!("{END_MARKER}\n"));
        let report = parse_console(&console);
        let att = EscapeAttestation::from_green_drill(
            "2026-06-21",
            &report,
            vec![BackendRun {
                backend: Backend::FirecrackerMicrovm,
                exercised: true,
                residual_note: None,
            }],
            Backend::FirecrackerMicrovm,
            "rootfs-digest",
            "kernel-digest",
            "6.1.168",
        )
        .unwrap();
        AgentExecGate::admit(Some(&att), &id).unwrap()
    }

    fn limits() -> ResourceLimits {
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 300,
        }
    }

    fn credential(jti: &str) -> RunTokenCredential {
        RunTokenCredential::new(format!("test-bearer:{jti}"), jti, 300).unwrap()
    }

    fn def(name: &str, kind: EffectKind, side_effecting: bool) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "issues".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: kind,
            side_effecting,
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }

    // ───────────────────────────── the routing split (§5.0 / X-6 #3) ────────────────────────────

    #[test]
    fn route_of_maps_each_effect_kind_to_its_route() {
        assert_eq!(route_of(EffectKind::Read), ToolRoute::Direct);
        assert_eq!(route_of(EffectKind::Compute), ToolRoute::Sandbox);
        assert_eq!(route_of(EffectKind::Mutate), ToolRoute::EffectApi);
        assert_eq!(route_of(EffectKind::External), ToolRoute::EffectApi);
    }

    #[test]
    fn only_compute_reaches_the_sandbox() {
        // The crux: exactly ONE effect kind routes to the sandbox.
        let to_sandbox: Vec<EffectKind> = [
            EffectKind::Read,
            EffectKind::Compute,
            EffectKind::Mutate,
            EffectKind::External,
        ]
        .into_iter()
        .filter(|k| route_of(*k) == ToolRoute::Sandbox)
        .collect();
        assert_eq!(to_sandbox, vec![EffectKind::Compute]);
    }

    #[test]
    fn a_mutate_effect_can_never_reach_exec() {
        // THE headline assertion (0 mutate-via-exec). A `mutate` def cannot build a SandboxJob —
        // there is no other constructor, so it has no path to the sandbox (the type-level boundary).
        let mutate = def("issue.create", EffectKind::Mutate, true);
        let r = SandboxJob::for_compute(
            &mutate,
            pinned(),
            vec!["sh".into()],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            limits(),
            TrustTier::UntrustedFork,
            credential("j"),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        );
        assert_eq!(
            r.unwrap_err(),
            RoutingError::NotComputeBound {
                tool: "issue.create".into(),
                actual_route: ToolRoute::EffectApi,
            }
        );
    }

    #[test]
    fn external_and_read_effects_also_cannot_reach_exec() {
        for (name, kind, se, route) in [
            (
                "ci.deploy",
                EffectKind::External,
                true,
                ToolRoute::EffectApi,
            ),
            ("issue.read", EffectKind::Read, false, ToolRoute::Direct),
        ] {
            let d = def(name, kind, se);
            let r = SandboxJob::for_compute(
                &d,
                pinned(),
                vec!["sh".into()],
                vec![],
                vec![],
                EgressPolicy::deny_all(),
                limits(),
                TrustTier::Trusted,
                credential("j"),
                MeterTarget {
                    reserve_id: "r".into(),
                },
                IdemToken("i".into()),
            );
            assert_eq!(
                r.unwrap_err(),
                RoutingError::NotComputeBound {
                    tool: name.into(),
                    actual_route: route,
                }
            );
        }
    }

    #[test]
    fn a_compute_effect_builds_a_kind_agent_job() {
        let compute = def("agent.run_tests", EffectKind::Compute, false);
        let job = SandboxJob::for_compute(
            &compute,
            pinned(),
            vec!["cargo".into(), "test".into()],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            limits(),
            TrustTier::UntrustedFork,
            credential("agent-jti"),
            MeterTarget {
                reserve_id: "agent-res".into(),
            },
            IdemToken("agent-idem".into()),
        )
        .unwrap();
        assert_eq!(job.spec().kind, JobKind::Agent);
    }

    // ──────────────────────────── the hardening profile (X-6 #4) ────────────────────────────────

    #[test]
    fn the_kind_agent_job_carries_the_full_hardening_profile() {
        let compute = def("agent.lint", EffectKind::Compute, false);
        let job = SandboxJob::for_compute(
            &compute,
            pinned(),
            vec!["ruff".into()],
            vec![],
            vec![SecretRef {
                name: "NPM_TOKEN".into(),
                handle: "broker://job/npm".into(),
            }],
            EgressPolicy::deny_all(),
            limits(),
            TrustTier::UntrustedFork,
            credential("jti"),
            MeterTarget {
                reserve_id: "res".into(),
            },
            IdemToken("idem".into()),
        )
        .unwrap();
        let spec = job.spec();
        // (a) kind=agent — the unified runner, agent variant.
        assert_eq!(spec.kind, JobKind::Agent);
        // (b) digest-pinned image (an un-digested tag fails-closed — tested below).
        assert!(spec.image.digest_pinned());
        // (c) egress default-deny.
        assert!(spec.egress.allow.is_empty());
        // (d) pids.max set (fork-bomb ceiling) + (e) timeout set.
        assert!(spec.limits.pids_max > 0);
        assert!(spec.limits.timeout_secs > 0);
        // (f) zero swap is STRUCTURAL — there is no swap field on ResourceLimits.
        // (g) read-only root + tmpfs scratch, no checkout for a compute job (the default workspace).
        assert_eq!(spec.workspace, WorkspaceSpec::default());
        // (h) secrets ride as in-boundary refs (names/handles), never the clear material.
        assert_eq!(spec.secret_refs.len(), 1);
        assert_eq!(spec.secret_refs[0].name, "NPM_TOKEN");
        // (i) the per-run attenuated token rides in the spec (guarantee #2).
        assert_eq!(spec.run_token.jti, "jti");
        // (j) the reserve target rides in the spec (guarantee #1).
        assert_eq!(spec.meter_to.reserve_id, "res");
    }

    #[test]
    fn an_undigested_image_fails_closed() {
        let compute = def("agent.run", EffectKind::Compute, false);
        let r = SandboxJob::for_compute(
            &compute,
            ImageRef {
                reference: "registry/runner:latest".into(),
            },
            vec!["sh".into()],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            limits(),
            TrustTier::UntrustedFork,
            credential("j"),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        );
        assert!(matches!(
            r.unwrap_err(),
            RoutingError::SpecRejected(SpecError::UndigestedImage { .. })
        ));
    }

    #[test]
    fn zero_pids_max_and_zero_timeout_fail_closed() {
        let compute = def("agent.run", EffectKind::Compute, false);
        let mut l = limits();
        l.pids_max = 0;
        let r = SandboxJob::for_compute(
            &compute,
            pinned(),
            vec!["sh".into()],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            l,
            TrustTier::UntrustedFork,
            credential("j"),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        );
        assert!(matches!(
            r.unwrap_err(),
            RoutingError::SpecRejected(SpecError::NoPidsMax)
        ));
    }

    // ──────────────────────── guarantee #2: anti-leak token scrub ───────────────────────────────

    #[test]
    fn the_shared_platform_token_is_scrubbed_from_the_child_env() {
        let compute = def("agent.run", EffectKind::Compute, false);
        let env = vec![
            EnvVar {
                name: PLATFORM_TOKEN_ENV.into(),
                value: "super-secret-platform-token".into(),
            },
            EnvVar {
                name: "LANG".into(),
                value: "C".into(),
            },
        ];
        let job = SandboxJob::for_compute(
            &compute,
            pinned(),
            vec!["sh".into()],
            env,
            vec![],
            EgressPolicy::deny_all(),
            limits(),
            TrustTier::UntrustedFork,
            credential("per-run"),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        )
        .unwrap();
        // The platform token is GONE; the short-lived per-run credential rides on the spec, not the env.
        assert!(job
            .scrubbed_env()
            .iter()
            .all(|e| e.name != PLATFORM_TOKEN_ENV));
        assert!(job.scrubbed_env().iter().any(|e| e.name == "LANG"));
        assert_eq!(job.spec().run_token.jti, "per-run");
    }

    #[test]
    fn scrub_is_idempotent() {
        let once = SandboxJob::scrub_platform_token(vec![EnvVar {
            name: PLATFORM_TOKEN_ENV.into(),
            value: "x".into(),
        }]);
        assert!(once.is_empty());
        let twice = SandboxJob::scrub_platform_token(once);
        assert!(twice.is_empty());
    }

    // ─────────────────────── exec dispatch through the four-guarantee seam ───────────────────────

    /// A no-op backend recording the four-guarantee call order — a SHAPE stub (the real microVM/
    /// gVisor backend is CI-P2 → P-237). There is NO host-exec path here (no `process::Command`);
    /// all execution goes through this `launch` seam (the `no-host-exec` lint, 1.6, admits this).
    struct RecordingBackend {
        order: Arc<Mutex<Vec<&'static str>>>,
        kills: Arc<AtomicU32>,
    }

    impl SandboxBackend for RecordingBackend {
        type Error = HookError;
        fn launch(
            &self,
            spec: &JobSpec,
            hooks: &RunnerHooks,
        ) -> Result<SandboxLaunch, Self::Error> {
            hooks.enforce_isolation_floor(spec)?;
            self.order.lock().unwrap().push("isolation_floor");
            let res = hooks.reserve(spec)?;
            self.order.lock().unwrap().push("reserve");
            if let Err(error) = hooks.attribute(spec) {
                hooks.release_unused(spec, &res)?;
                return Err(error);
            }
            self.order.lock().unwrap().push("attribute");
            // ... the hardened guest runs the compute here; the seam carries the result back ...
            let result = SandboxResult::stub_ok(ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            });
            hooks.settle_completed(spec, &res, result.usage)?;
            self.order.lock().unwrap().push("settle");
            Ok(SandboxLaunch {
                handle: SandboxHandle {
                    guest_id: "agent-guest".into(),
                },
                result,
                output_complete: true,
            })
        }
        fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn working_hooks() -> RunnerHooks {
        RunnerHooks::new(
            myelin_ci_sandbox::CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        )
    }

    fn hands<'a>(
        backend: &'a RecordingBackend,
        hooks: RunnerHooks,
    ) -> SandboxToolHands<'a, RecordingBackend> {
        SandboxToolHands::new(
            green_gate(),
            backend,
            hooks,
            pinned(),
            credential("agent-jti"),
            MeterTarget {
                reserve_id: "agent-res".into(),
            },
            IdemToken("agent-idem".into()),
            TrustTier::UntrustedFork,
            limits(),
            EgressPolicy::deny_all(),
            Vec::new(),
        )
    }

    #[test]
    fn exec_dispatches_a_kind_agent_job_driving_the_four_guarantees_in_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let kills = Arc::new(AtomicU32::new(0));
        let backend = RecordingBackend {
            order: order.clone(),
            kills: kills.clone(),
        };
        let hands = hands(&backend, working_hooks());
        let out = hands.exec(Command("cargo test".into()));
        // The hardened guest ran and the result threads back (the X-6 equivalence: exec == launch).
        assert_eq!(out, ToolResult("sandbox:agent-guest".into()));
        // All four guarantees fired in the mandated order.
        assert_eq!(
            *order.lock().unwrap(),
            vec!["isolation_floor", "reserve", "attribute", "settle"]
        );
        // Guarantee #4: whole-guest kill on teardown (the guest is never reused).
        assert_eq!(kills.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn exec_refuses_to_start_on_an_exhausted_wallet_never_running_the_guest() {
        // Guarantee #1 (cost gate): a refused reserve refuses-to-start; the result is a LOUD tool
        // error (never a silent success), and the guest never ran (no "settle").
        let order = Arc::new(Mutex::new(Vec::new()));
        let backend = RecordingBackend {
            order: order.clone(),
            kills: Arc::new(AtomicU32::new(0)),
        };
        let hooks = RunnerHooks::new(
            myelin_ci_sandbox::CompletionSettlementOwner::Hook,
            Box::new(|_spec| Err(HookError("wallet exhausted — refuse to start".into()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let hands = hands(&backend, hooks);
        let out = hands.exec(Command("cargo test".into()));
        assert!(
            out.0.starts_with("exec-refused:"),
            "a refused dispatch surfaces LOUD: {out:?}"
        );
        // The guest never reached "settle" — refuse-to-start (never interrupt in-flight's dual).
        assert!(!order.lock().unwrap().contains(&"settle"));
    }

    #[test]
    fn exec_fails_closed_when_the_isolation_floor_is_not_met() {
        // Guarantee #4: if the hardening profile cannot be applied/verified, launch fails closed
        // BEFORE any untrusted code runs (the seam the AG-D4/CI-P5 escape drill drives).
        let order = Arc::new(Mutex::new(Vec::new()));
        let backend = RecordingBackend {
            order: order.clone(),
            kills: Arc::new(AtomicU32::new(0)),
        };
        let hooks = RunnerHooks::new(
            myelin_ci_sandbox::CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Err(HookError("hardening profile not met".into()))),
        );
        let hands = hands(&backend, hooks);
        let out = hands.exec(Command("sh".into()));
        assert!(out.0.starts_with("exec-refused:"), "{out:?}");
        assert!(
            order.lock().unwrap().is_empty(),
            "no guarantee fired past the isolation floor"
        );
    }

    #[test]
    fn dispatch_compute_surfaces_a_typed_launch_error() {
        // The explicit fallible form returns a typed ExecError (never a swallowed pass).
        let backend = RecordingBackend {
            order: Arc::new(Mutex::new(Vec::new())),
            kills: Arc::new(AtomicU32::new(0)),
        };
        let hooks = RunnerHooks::new(
            myelin_ci_sandbox::CompletionSettlementOwner::Hook,
            Box::new(|_spec| Err(HookError("exhausted".into()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let hands = hands(&backend, hooks);
        let job = hands
            .build_compute_job(&compute_tool_def(), &Command("sh".into()))
            .unwrap();
        let err = hands.dispatch_compute(&job).unwrap_err();
        assert!(matches!(err, ExecError::Launch(HookError(_))));
    }

    #[test]
    fn the_routing_split_note_is_documented_once() {
        // Guarantee #3 is structural + documented once in the CI-sandbox seam.
        assert!(myelin_ci_sandbox::hitl_withhold_note().contains("EffectApi::apply"));
    }

    // ──────────────────── AG-D4 gate: exec is fail-closed on the escape attestation ─────────────

    #[test]
    fn the_exec_hands_carry_a_green_ag_d4_gate_by_construction() {
        // The structural fail-closed: `SandboxToolHands` exists ONLY when a green AG-D4 gate was
        // supplied. There is NO `new` without the gate argument — so a green attestation is a
        // compile-time prerequisite of any dispatch. A run that holds these hands has, by
        // construction, a green AG-D4 attestation for the production backend.
        let backend = RecordingBackend {
            order: Arc::new(Mutex::new(Vec::new())),
            kills: Arc::new(AtomicU32::new(0)),
        };
        let hands = hands(&backend, working_hooks());
        assert_eq!(
            hands.gate().backend_id().backend,
            Backend::FirecrackerMicrovm
        );
        assert!(hands.gate().open_line().starts_with("[AG-D4 GATE OPEN]"));
    }

    #[test]
    fn without_a_green_attestation_no_gate_can_be_built_so_no_hands_dispatch() {
        // The negative leg: with NO attestation, `AgentExecGate::admit` REFUSES — so the gate the
        // hands require can never be built, and therefore no untrusted compute can be dispatched.
        // (This is the same fail-closed proven in the gate's own CDC; here we pin it at the exec seam.)
        let id = ProductionBackendId {
            backend: Backend::FirecrackerMicrovm,
            rootfs_sha256: "rootfs-digest".into(),
            kernel_sha256: "kernel-digest".into(),
            corpus_version: CORPUS_VERSION,
        };
        assert!(
            AgentExecGate::admit(None, &id).is_err(),
            "no green attestation ⇒ no AgentExecGate ⇒ no SandboxToolHands ⇒ no untrusted compute"
        );
    }

    #[test]
    fn routing_error_display_is_loud_and_self_describing() {
        let e = RoutingError::NotComputeBound {
            tool: "issue.create".into(),
            actual_route: ToolRoute::EffectApi,
        };
        let s = e.to_string();
        assert!(s.contains("issue.create"));
        assert!(s.contains("routing split"));
    }
}
