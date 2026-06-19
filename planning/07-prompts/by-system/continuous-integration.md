# Phase 7 — Prompt Ledger: Continuous Integration / CD (finer-granularity expansion, Phase 7-A second pass)

> Prompt count: 16 (first pass) → 35 (this finer-grained pass). Every multi-deliverable prompt is split into
> single-deliverable, clean-context, independently-committable units; all coverage (every milestone, contract,
> drill, floor the first pass covered) is preserved and re-threaded across the new finer ids. No padding — the
> extra volume is the implementation detail an isolated agent needs.
>
> Phase: 07-prompts (per-system file, Phase 7-A). The complete ordered set of implementation prompts that
> operationalize the entire continuous-integration roadmap
> (planning/06-roadmaps/subsystems/continuous-integration.md, milestones CI-M2 + CI-M4 + CI-M5 + CI-M6) into
> clean-context, independently-committable coding tasks. Built to the template in
> planning/07-prompts/00-ledger-overview.md §2 (every field present, never implicit) and banded to
> planning/06-roadmaps/00-master-sequencing.md §2 (M0..M6, the gate invariant). Frozen architecture (this file
> OPERATIONALIZES, it does not redesign): planning/04-subsystem-architectures/continuous-integration/architecture/
> (00..07) + the build-to contracts in planning/05-refined-shared-systems-architecture/contract-index.md +
> 00-reconciliation-decisions.md (X-1/X-6/X-7, OQ-C/OQ-D/OQ-E/OQ-F/OQ-I/OQ-J/OQ-K). Drills:
> planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
> (CI-T1/AG-D4 + CI-D1..CI-D11 + CI-R3 + GIT-D10/CI-D8 + E2E-1/E2E-2/E2E-3 + the F6 surge family). Plain-text
> identifiers throughout (no backticks-as-emphasis). Markdown only; this file makes no commits. Date: 2026-06-19.
>
> The global P-NNN ids are assigned by the consolidated ledger index (Phase 7-B, 01-ledger-index.md) when these
> per-system prompts are interleaved into the single execution order. Here each prompt carries a stable local
> handle CI-P<n> so its DEPENDS-ON edges are unambiguous before global numbering; the index rewrites CI-P<n> to
> its P-NNN. Where a prompt depends on another system's prompt not yet numbered, it names that system's milestone
> (the index resolves it to the P-NNN).
>
> CI is a CONSUMER subsystem (master §2 M4) with an UNUSUAL shape: its single hardest, most catastrophic
> property — the real-kernel sandbox-escape GATE (AG-D4 / CI-T1) — is OWNED by CI yet FRONT-LOADED to M2, because
> CI's runner is the same unified sandbox the agent fabric's ToolHands::exec runs on (ADR-20 / X-6). So the
> ledger has its M2 CI work (the runner substrate + the runner agent + the escape gate), the bulk of CI in M4
> (the two green-field cores — the DRR pull-lease scheduler and the EU fleet autoscaler — plus the X-1
> CheckStatus producer closing the seam Git built in M3, plus pipelines/deploys/supply-chain/metering/surfacing),
> world-scale hardening + floor follow-ons in M5, and dogfooding in M6. Two permanent gates ratchet across the
> whole build and re-appear as explicit re-confirm prompts: AG-D4 / CI-T1 (every backend/image/kernel change)
> and STOR-D1/STOR-D2 (restore-verify, every change touching a CI store).
>
> Coverage (finer ids): CI-M2 → CI-P1 + CI-P2 + CI-P3 + CI-P4 + CI-P5; CI-M4 → CI-P6..CI-P27; CI-M5 → CI-P28..
> CI-P34; CI-M6 → CI-P35. Thirty-five prompts (CI-P1..CI-P35), no milestone gap; the coverage matrix at the foot
> maps each first-pass prompt to its finer children.

---

### CI-P1 — The JobSpec struct + the SandboxBackend / FleetProvider trait seam + the four-uniform-guarantee wiring hooks

- **BAND.** M2.
- **ROADMAP MILESTONE.** CI-M2 (planning/06-roadmaps/subsystems/continuous-integration.md §3 "CI-M2 — The
  unified sandbox runner + the escape GATE"), the JobSpec + trait-seam + four-guarantee-wiring slice (the one
  struct, two kinds; the trait shapes before any backend implements them).
- **DEPENDS-ON.** The M0 substrate prompts (the Cargo workspace + the eight glue-crate skeletons + serve(AppSpec)
  + the twelve lints incl. no-host-exec + the contract-coverage scanner + the failure-injection harness; master
  §2 M0, substrate roadmap SUB-M0). The M1 Identity prompt that ships mint_run_token (contract 4.7) and the M1
  Storage prompt that ships the reserve/settle gate (11.7) + the KMS hierarchy (11.3). The index places this
  first in CI's M2 work — it is the seam the agent fabric (AG-P8) co-defines and CI-P2 implements a backend for.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) §3 (agent-native from the ground up — the strategy pattern everywhere agents plug
    in; name-your-floors; world-scale; EU-sovereign by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability — RCE/sandbox-escape
    before any feature); §7 (reconcile cross-component contracts at the plan layer — the one struct two kinds
    seam is co-defined with the agent fabric, names AND units up front).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/01-tech-and-data-model.md §2
    (the JobSpec struct + the SandboxBackend / FleetProvider traits — the one struct two kinds seam, the
    trust_tier stamped once); 02-internals-and-algorithms.md §5.2 (the four uniform guarantees — pinned);
    00-overview.md §4 (the component map — the sandbox backend is the hard isolation boundary).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-6 (the four
    uniform sandbox guarantees + the frozen requires_approval defaults; ToolHands::exec = the CI runner's
    kind=agent job).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.4 (ToolHands::exec = the CI
    runner's kind=agent job on the unified sandbox; the four uniform guarantees; no host-exec bypass), 11.7
    (reserve/settle — fronts every run; the hook), 4.7 (mint_run_token — per-job attenuated token; the hook),
    11.3 (KMS hierarchy), 1.1/1.2 (serve(AppSpec) + the public/internal trust boundary).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §2.1 (the hard prerequisites) + §3 CI-M2
    (JobSpec{kind ∈ {Ci, Agent}} + the four uniform guarantees) + §5 (the contract table rows 8.4, 11.7, 4.7).
- **DELIVERABLE (what to build + exactly where in the repo).** In a new CI sandbox crate under the workspace
  (myelin-ci-sandbox, the runner/backend home; the CI service crates land in CI-P6):
  - The JobSpec struct exactly as frozen in arch 01 §2: kind (JobKind ∈ {Ci, Agent} — the UNIFY point), image
    (ImageRef, MUST be digest-pinned), command, env (secrets are NAMES), secret_refs (resolved in-boundary),
    egress (EgressPolicy default-deny), limits (ResourceLimits incl. pids_max + zero-swap + timeout), workspace,
    trust_tier (TrustTier ∈ {Trusted, UntrustedFork, SelfHosted}), run_token (RunTokenRef), meter_to, idem_token.
    Unit-tested round-trip + the digest-pin-or-reject rule (an un-digested ImageRef is rejected fail-closed).
  - The SandboxBackend trait signature (launch(spec, hooks) -> SandboxHandle; kill — whole-guest kill on
    teardown) and the FleetProvider trait signature (provision(class, n, region) / deprovision / capacity) —
    the trait shapes only; gVisor + Firecracker impls land in CI-P2 / CI-P28; the fleet impl in CI-P14.
  - The four-uniform-guarantee wiring SEAM (arch 02 §5.2): the reserve/settle hook (11.7), the per-run-token
    attribution hook (mint_run_token, 4.7), the HITL-withhold routing note (mutation never goes through exec — it
    goes through EffectApi; exec carries only compute/external untrusted code), and the isolation-floor hook that
    CI-P5's escape drill drives. ToolHands::exec IS SandboxBackend::launch(JobSpec{kind:Agent}) — wire that
    equivalence so the agent fabric (AG-P8) dispatches onto this exact runner.
  - FLOOR named: none new here — the trait shapes are frozen; the Firecracker default impl is CI-P2, the gVisor
    second backend is CI-P28. State this in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 8.4 ToolHands::exec / the unified sandbox (owned, the CI half — the JobSpec shape +
  the trait seam + the four-guarantee hooks; co-defined with the agent fabric). Consumed: 11.7 reserve/settle
  (the hook), 4.7 mint_run_token (the attribution hook), 11.3 KMS. Implement to the frozen JobSpec/SandboxBackend
  shapes (arch 01 §2 is byte-authoritative); a needed shape change is a whole-workspace contract PR, escalated
  and written down (code-wins-over-docs).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The no-host-exec lint is GREEN over the sandbox crate's seam (0 exec paths bypass SandboxBackend::launch; the
    lint red on a deliberately-host-exec fixture, green on the launch-trait path) — CI, lint signal = 0 bypass
    paths. (The ZERO-escapes property is CI-P5's GATE; the hardened-boot self-test is CI-P2.)
  - The four-guarantee hooks are present and typed: a JobSpec dispatched through the seam reserves (11.7),
    attributes (4.7), and routes mutation away from exec — the contract-coverage scanner is green on row 8.4's CI
    half (the runner-seam half; the agent-fabric consumer half is AG-P8) — CI.
- **TESTS (required).** Unit tests for the JobSpec round-trip + the digest-pin-or-reject rule + the
  trait-signature compile + the four-guarantee hook wiring. The provider/consumer CDC pair for contract-index row
  8.4 (the runner-seam half — the kind=agent job spec + the four-guarantee hooks; the agent-fabric consumer half
  is AG-P8). State the cargo-mutants mutation-score floor for the digest-pin + the four-guarantee-hook modules
  (mandatory-core, security-load-bearing); name the floor.
- **DEFINITION OF DONE.** The JobSpec struct + the SandboxBackend/FleetProvider trait seam + the four-guarantee
  wiring hooks exist and compile; the no-host-exec lint is green with both fixtures; the unit + CDC tests pass;
  the contract-coverage scanner is green on row 8.4 (CI half); the Firecracker-impl follow-on is named (CI-P2);
  the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: JobSpec + SandboxBackend/FleetProvider trait seam + four-guarantee hooks. Body
  lists: contract 8.4 (CI seam half) wired with 11.7/4.7/11.3 hooks; the no-host-exec lint greened with red+green
  fixtures; the Firecracker-impl follow-on named (CI-P2). Branch first if on default; do not push unless asked.
  End with the workspace Co-Authored-By trailer.

---

### CI-P2 — The Firecracker default backend + the backend-independent mandatory hardening profile + the hardened-boot self-test

- **BAND.** M2.
- **ROADMAP MILESTONE.** CI-M2 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M2), the
  Firecracker-backend + hardening-profile slice (the runner BOOTS hardened before the corpus drives it).
- **DEPENDS-ON.** CI-P1 (the JobSpec struct + the SandboxBackend trait this impl satisfies + the four-guarantee
  hooks). The M0 failure-injection harness (the telemetry-assertion library — the boot self-test reads its
  assertions; SUB-M0). The index places this after CI-P1 in CI's M2 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign by construction; world-scale means world-scale — do not shy from the
    necessary complexity); ../../external-insights/04-hard-problems.md §5 (untrusted code is the never-"done"
    surface; a property not drilled on a real kernel is a claim);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — observability is part of the pass:
    the hardened-boot self-test emits its telemetry assertion).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §5.1 (microVM/Firecracker default + the why-microVM decision), §5.3 (the backend-independent mandatory
    hardening profile); 00-overview.md §6 (the floors named up front).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-6 (the four
    uniform sandbox guarantees + the hardening posture).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.4 (the unified sandbox — the
    hardening profile half), 1.6 (the no-host-exec lint over the launch path).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M2 (the SandboxBackend trait + the
    Firecracker-default backend + the mandatory backend-independent hardening profile + the floor — one backend
    through the drill first).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI sandbox crate (myelin-ci-sandbox):
  - The Firecracker default SandboxBackend impl (microVM = KVM + minimal VMM) satisfying CI-P1's trait: launch
    boots a digest-pinned JobSpec; kill whole-guest-kills on teardown.
  - The backend-independent mandatory hardening profile applied identically to every sandbox regardless of
    backend or kind (arch 02 §5.3): egress default-deny + allowlist opt-in (the cloud-metadata endpoint
    169.254.169.254, the control-plane/internal RPC, any cross-tenant network ALWAYS blocked); read-only root +
    tmpfs scratch; all caps dropped; no-new-privileges; seccomp; digest-pinned images (an un-digested tag
    rejected fail-closed); pids.max + zero swap + scratch disk quota; whole-guest kill on teardown;
    one-job-per-sandbox, ephemeral, NEVER reused across tenants/jobs; secrets resolved by name inside the
    boundary. The egress-allowlist evaluator is unit-tested (metadata/control-plane/cross-tenant always denied).
  - The hardened-boot self-test: a Firecracker microVM boots a trivial JobSpec with the full hardening profile
    on, and the harness reads a telemetry assertion that egress-default-deny + pids.max + read-only-root are in
    force (a boot self-test, the floor under CI-P5's adversarial drill). This proves the runner BOOTS hardened,
    not that it survives the corpus.
  - FLOOR named: ONE backend (Firecracker) through the drill first; gVisor as the named second backend behind the
    same trait, its own drill — CI-P28, density/latency-economics-triggered (esp. sub-second agent compute).
    State this in the crate doc.
- **CONTRACTS TO IMPLEMENT.** 8.4 the unified sandbox (owned, the hardening + Firecracker half). Obeyed: 1.6
  no-host-exec (the launch path stays the only exec route). Implement to the frozen shapes; escalate a needed
  change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The no-host-exec lint stays GREEN over the launch path (0 bypass paths) — CI.
  - The hardened-boot self-test emits its dated green artifact: a Firecracker microVM boots with the full
    hardening profile and the harness reads egress-default-deny + pids.max + read-only-root all-on — CI,
    profile-enforced signal = all-on.
- **TESTS (required).** Unit tests for the egress-allowlist evaluator (metadata/control-plane/cross-tenant always
  denied) + the digest-pin-or-reject rule on the Firecracker launch path. The hardening-profile boot self-test
  scenario on the failure-injection harness. State the cargo-mutants mutation-score floor for the
  hardening-profile module (mandatory-core, security-load-bearing); name the floor.
- **DEFINITION OF DONE.** The Firecracker backend + the mandatory hardening profile + the hardened-boot self-test
  exist and compile; the no-host-exec lint is green; the hardened-boot self-test emits its dated green artifact;
  the unit + self-test tests pass; the contract-coverage scanner is green on row 8.4 (hardening half); the gVisor
  floor is named with its follow-on (CI-P28); the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: Firecracker default backend + mandatory hardening profile + hardened-boot
  self-test. Body lists: contract 8.4 (hardening half) implemented; the hardened-boot self-test greened
  (egress-deny + pids.max + read-only-root all-on); the gVisor-second-backend floor named (CI-P28). Branch first
  if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CI-P3 — The runner agent + the lease/heartbeat handshake + the exactly-once job.done terminal report

- **BAND.** M2.
- **ROADMAP MILESTONE.** CI-M2 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M2), the
  runner-agent + lease/heartbeat + terminal-report slice (the attested binary that drives the sandbox).
- **DEPENDS-ON.** CI-P1 + CI-P2 (the SandboxBackend + Firecracker backend + hardening profile the runner
  launches). The M2 Durable-workflow prompt that froze the job.done signal shape (9.2/9.4). The M1 Identity
  prompt that ships mint_run_token (4.7). The index places this after CI-P2 in CI's M2 work; the full scheduler
  job_queue is CI-P11 (here the runner SIDE of the lease handshake only).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — the same runner under agent + CI jobs);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — exactly-once terminal report under
    at-least-once delivery is a quantified property).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/00-overview.md §4 (the runner
    agent = a small attested Rust binary; hosted + self-hosted from one artifact);
    02-internals-and-algorithms.md §2.1 (the lease + heartbeat + the runner side of the claim);
    03-events-contracts-and-glue.md (the job.done terminal report shape).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-F (the job.done
    signal idempotent on idem_token).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 9.2/9.4 (the job.done signal the
    runner reports terminal on), 4.7 (mint_run_token — self-hosted runner token scoped to one tenant's SelfHosted
    jobs), 12.4 (residency_verify — the runner pool region).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M2 (the runner agent — claims
    leases, heartbeats, launches the sandbox, streams firehose frames, reports terminal via job.done).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI sandbox crate (myelin-ci-sandbox):
  - The runner agent (a small attested Rust binary): claims leases for its labels, heartbeats the lease, launches
    the sandbox via SandboxBackend::launch (CI-P1/CI-P2), streams firehose frames (the firehose wiring is STUBBED
    here; the full log pipeline is CI-P20), and reports terminal via the job.done signal idempotent on
    idem_token. Same binary hosted + self-hosted; the self-hosted ATTESTATION GATE (TPM quote /
    provisioning-signed token) + the tenant-SelfHosted-scoped token mint is CI-P4 (here the runner consumes the
    minted token, 4.7).
  - The lease primitive REUSES the platform's FOR UPDATE SKIP LOCKED + heartbeat pattern (the full
    job_queue/scheduler is CI-P12; here the runner side of the lease handshake + heartbeat + the report path).
  - FLOOR named: pre-warmed snapshot pools (the cold-start mitigation) are CI-P4; the firehose log pipeline is
    CI-P20. State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 9.2/9.4 job.done (the terminal report), 4.7 mint_run_token (the runner
  consumes the token), 12.4 residency_verify (the runner region). Implement to the frozen shapes; escalate a
  needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The runner reports terminal exactly-once: a runner that delivers job.done twice (at-least-once) wakes the
    parked workflow exactly once (idempotent on idem_token) — CI, double-effect count = 0.
  - The lease/heartbeat handshake holds: a claimed lease is renewed by heartbeat; an expired lease is reclaimable
    (the reaper side is CI-P12) — CI.
- **TESTS (required).** Unit tests for the lease/heartbeat handshake + the job.done idempotency (delivered twice
  → woken once). The CDC pair for the runner side of 9.2/9.4. State the cargo-mutants mutation-score floor for
  the terminal-report idempotency module (mandatory-core).
- **DEFINITION OF DONE.** The runner agent + lease/heartbeat + the exactly-once job.done terminal report exist
  and compile; the exactly-once terminal report is proven (double-effect = 0); the unit + CDC tests pass; the
  contract-coverage scanner is green; the pre-warm + firehose floors are named (CI-P4 / CI-P20); the work is
  committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: runner agent + lease/heartbeat handshake + exactly-once job.done report. Body
  lists: 9.2/9.4/4.7/12.4 consumed; the exactly-once terminal report proven (double-effect = 0); the pre-warm
  (CI-P4) + firehose (CI-P20) floors named. Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### CI-P4 — Pre-warmed microVM snapshot pools + the self-hosted runner attestation gate + the tenant-scoped token mint

- **BAND.** M2.
- **ROADMAP MILESTONE.** CI-M2 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M2), the
  pre-warm + self-hosted-attestation slice (cold-start mitigation + the self-hosted trust boundary's mint side).
- **DEPENDS-ON.** CI-P3 (the runner agent the pre-warm pool resumes and the self-hosted runner runs). CI-P2 (the
  resume-from-snapshot microVM). The M1 Identity prompt that ships mint_run_token (4.7). The index places this
  after CI-P3 in CI's M2 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale — warm-pool-fast time-to-first-log-line; EU-sovereign);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — an unattested runner cannot claim
    is a quantified property), §6 (investigate before you build — the attestation gate is security-critical).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §5.4 (pre-warmed snapshot pools — the cold-start mitigation; resume-from-snapshot microVMs as a small warm
    buffer per (region, label-class); the buffer-sizing function is open question 07#2, tuned in CI-M5);
    01-tech-and-data-model.md §3.4 (the runner table — ownership hosted|self_hosted, attestation, attest_state).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-6 (the self-hosted
    runner trust boundary; the scoped token).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.7 (mint_run_token — the
    self-hosted runner token scoped to one tenant's SelfHosted jobs), 12.4 (residency_verify — the runner pool
    region).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M2 (pre-warmed microVM snapshot
    pools; self-hosted attests and receives a tenant-SelfHosted-scoped job token).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI sandbox crate (myelin-ci-sandbox):
  - Pre-warmed snapshot pools (arch 02 §5.4): resume-from-snapshot microVMs kept as a small warm buffer per
    (region, label-class), so time-to-first-log-line is warm-pool-fast. The buffer-sizing function is a named
    open question (07#2) tuned in CI-M5 (CI-P30); ship a FIXED-BUFFER floor here.
  - The self-hosted runner attestation gate: a self-hosted runner attests (TPM quote / provisioning-signed
    token); the attest_state transitions pending → attested → failed; an attested self-hosted runner receives a
    tenant-SelfHosted-scoped job token (mint_run_token, 4.7). An attestation FAILURE → the runner cannot claim.
    (The fleet-side autoscale that provisions runners is CI-P10; here the attestation + token mint that gates a
    self-hosted runner from claiming.)
  - FLOOR named: the fixed pre-warm buffer → the measured buffer-sizing function (open question 07#2) is CI-P23
    (CI-M5). State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 4.7 mint_run_token (the self-hosted scope), 12.4 residency_verify (the
  pool region). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Self-hosted attestation: an UN-attested self-hosted runner CANNOT claim a job (0 claims by an unattested
    runner); an attestation failure → cannot claim — CI.
  - Pre-warm: a warm pool of resume-from-snapshot microVMs serves time-to-first-log-line faster than cold boot
    (the warm-pool signal tracks buffer occupancy) — CI.
- **TESTS (required).** Unit tests for the pre-warm buffer (resume-from-snapshot serves from the warm buffer) +
  the attestation state machine (pending → attested → failed; unattested → cannot claim). The CDC pair for 4.7
  (the self-hosted scope). State whether the attestation module is mandatory-core; if so, name the mutation-score
  floor.
- **DEFINITION OF DONE.** The pre-warm snapshot pools + the self-hosted attestation gate + the tenant-scoped
  token mint exist and compile; an unattested runner cannot claim; the warm pool serves faster than cold; the
  unit + CDC tests pass; the contract-coverage scanner is green; the measured-buffer-sizing floor is named
  (CI-P23); the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: pre-warm snapshot pools + self-hosted attestation gate + scoped token mint.
  Body lists: 4.7/12.4 consumed; unattested-cannot-claim proven; the warm-pool floor (fixed buffer) named with
  its CI-P30 follow-on. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### CI-P5 — The escape-drill adversarial corpus + the green-attestation format + the AG-D4 / CI-T1 hard GATE

- **BAND.** M2.
- **ROADMAP MILESTONE.** CI-M2 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M2), the escape
  GATE — CI's Tier-2 keystone and the M2-band go/no-go for ALL untrusted code (CI or agent).
- **DEPENDS-ON.** CI-P2 (the Firecracker backend + hardening profile the corpus is launched inside). CI-P3 (the
  runner agent that launches the corpus job). The M0 failure-injection harness (the 1x/10x/30x generator + the
  scoped dependency-break + the telemetry-assertion library — the escape drill IS a harness drill; SUB-M0). The
  index places this LAST in CI's M2 work — it gates M3 and everything downstream of untrusted execution; the
  agent fabric's AG-P8 consumes the green attestation this prompt produces.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale means world-scale — do not shy from the necessary complexity; agent-native);
    ../../external-insights/04-hard-problems.md §5 (untrusted code is a never-"done" surface; a property not
    drilled on a REAL kernel is a claim, not a fact — EI-04 §5.1);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (RCE/sandbox-escape outranks every feature),
    §3 (prove-it: a quantified gate — ZERO escapes — and observability is part of the pass: the green attestation
    artifact IS the pass condition; never weaken the threshold to manufacture green).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §5.5 (THE escape drill D-4/T-5 — the single hard go/no-go; the adversarial corpus enumerated; the
    green-attestation artifact); 00-overview.md §4 (the runner agent = a small attested Rust binary);
    07-drills-and-open-questions.md §1 row T-1 (the escape drill quantified gate) + §3 (the single most important
    thing — drill first, capability second).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-6 (the escape
    drill gates ALL agent execution, not only CI; the four uniform guarantees).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.4 (the real-kernel escape drill
    gates both kinds), 4.7 (mint_run_token — the corpus job runs under a scoped token), 12.4 (residency_verify —
    the runner pool region).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row AG-D4 / CI-T1 (compute tool attempts a kernel escape on a real kernel → ZERO escapes; green escape
    attestation or CI is no-go) + §3.5 (the one hard gate — the adversarial corpus families) + §2.5 (the
    survival-signal assertions).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M2 exit gate (CI-T1 / AG-D4 = ZERO
    escapes; re-run on every backend/image/kernel change) + §0 (the critical-path framing — AG-D4 is a permanent
    GATE that blocks ALL untrusted execution).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI sandbox crate (myelin-ci-sandbox) + a
  drill harness module:
  - The escape-drill adversarial corpus (the [OPEN → P6] obligation from arch 07 R-1, now BUILT) — a JobSpec
    {kind ∈ {Ci, Agent}} that attempts, on a REAL kernel against the PRODUCTION backend: kernel-exploit
    primitives; cloud-metadata SSRF (169.254.169.254) → cred theft; control-plane / internal-RPC reach;
    cross-tenant network/storage; a fork bomb (assert the pids.max ceiling holds); disk fill; secret exfil via
    egress (assert default-deny holds).
  - The green-attestation artifact format (a dated, signed record: backend + image digest + kernel version +
    corpus version + 0 escapes).
  - FLOOR named: there is NO floor on the ZERO-escapes property — it is both the floor and the full answer, and a
    permanent gate re-run on every backend/image/kernel change. The gVisor second backend (CI-P28) re-runs THIS
    SAME drill on the new backend; the M4-boundary re-confirm is CI-P27. State this in writing.
- **CONTRACTS TO IMPLEMENT.** 8.4 the escape drill (owned — the real-kernel adversarial corpus + the green
  attestation; gates both CI and agent kinds). Consumed: 4.7 mint_run_token (the corpus job token), 12.4
  residency_verify. Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-T1 / AG-D4 (THE HARD GATE, permanent): the full adversarial corpus runs inside a sandbox on a REAL kernel
    on the PRODUCTION backend → ZERO escapes (0 kernel escapes, 0 metadata-cred-theft, 0 control-plane reach, 0
    cross-tenant read, fork bomb contained by pids.max, disk fill contained, 0 secret exfil via egress). The run
    emits a DATED GREEN ESCAPE ATTESTATION (the telemetry green artifact) or CI IS NO-GO FOR UNTRUSTED CODE — and
    M3 cannot start. Re-run on every backend/image/kernel change — GATE.
- **TESTS (required).** The AG-D4 / CI-T1 real-kernel escape drill scenario on the failure-injection harness
  against the FULL adversarial corpus, asserting 0 escapes and the green-attestation artifact. The CDC pair for
  the drill half of 8.4. State the cargo-mutants mutation-score floor for the corpus + the attestation modules
  (mandatory-core, security-load-bearing).
- **DEFINITION OF DONE.** The escape-drill adversarial corpus + the green-attestation format exist and compile;
  AG-D4 / CI-T1 emits a DATED GREEN ESCAPE ATTESTATION on the production backend (PROVEN, ZERO escapes — never a
  doc claim); the drill + CDC tests pass; the contract-coverage scanner is green on row 8.4; the no-floor /
  permanent-gate note is written (gVisor re-runs it CI-P28; re-confirmed CI-P27); the work is committed. AG-D4 is
  NEVER claimed green over a red attestation — a red AG-D4 blocks ALL of M3+ and becomes a dated no-go scorecard
  row, never a weakened threshold.
- **COMMIT.** Header: P-<NNN> M2: escape adversarial corpus + green-attestation + AG-D4 / CI-T1 hard GATE. Body
  lists: contract 8.4 (the drill) greened with the ZERO-escapes attestation (backend + image digest + kernel
  version); the no-floor permanent-gate note (gVisor re-greens it CI-P28; M4 re-confirm CI-P27). Branch first if
  on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CI-P6 — The five CI service shells + the complete forward-only data-model migrations

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 "CI-M4 — The full
  subsystem"), the service-shell + data-model slice (the substrate every other M4 CI prompt builds on).
- **DEPENDS-ON.** CI-P2 + CI-P5 (the runner + AG-D4 green — untrusted CI steps may now run in M4). The M1 Storage
  prompt (OLTP tier client + RLS + the outbox in the same DB, 11.1). The M0 substrate (serve(AppSpec), the lints,
  the PersonalDataHolder auto-registration, the contract-coverage scanner). The index places this FIRST in CI's
  M4 work (CI lands first within M4, master §2).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe & EU-sovereign by construction — residency, data-subject rights, auditability
    as architectural constraints; name-your-floors);
    ../../external-insights/01-process-and-quality-doctrine.md §5 (the ratchet — an uncommitted lint is no lint),
    §7 (reconcile contracts at the plan layer — build to the frozen shapes).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/00-overview.md §4 (the five
    logical services — Trigger & Dispatch, CI Control Plane, the runner agent, the sandbox backend, the workflow
    definitions; each a serve(AppSpec) shell, its own Postgres, no cross-DB) + §5 (cell topology — no global
    pool); 01-tech-and-data-model.md §3 (the complete per-service schema — ci_run, ci_job, check_attempt,
    job_queue, fair_deficit, runner, log_segment, log_anchor, artifact, cache_entry, environment, deployment,
    secret_binding, cost_event, consumer_dedup) + §4 (encryption/residency/GDPR posture).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.1 (OLTP), 12.1 (the (tenant,
    region) partition key + the residency-pin lint), 10.2 (the #[personal_data] classify-derive + the
    no-untagged-personal-data lint), 4.8 (pseudonym subjects — triggered_by/approved_by never copied PII),
    1.1/1.2/1.3 (serve + three ports + liveness≠readiness), 1.6 (the lints — tenant-predicate, residency-pin,
    no-cross-db, no-untagged-personal-data).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 + §5 (rows 1.1/1.2/1.3, 11.1,
    12.1).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI service implementation crates under the
  workspace (myelin-ci-dispatch for Trigger & Dispatch; myelin-ci-controlplane for the CI Control Plane; the
  runner agent already in myelin-ci-sandbox from CI-P3; the ci.pipeline workflow defs in myelin-ci-controlplane),
  each a thin serve(AppSpec) shell with its own Postgres (no cross-DB):
  - The serve(AppSpec) shells for Trigger & Dispatch and the CI Control Plane (1.1) — the public/internal/
    metrics-health three-surface split (1.2); liveness must not check deps, readiness gates on DB pool + broker +
    authz reachability + at-least-one-healthy-runner-pool (1.3, arch 04 §4). (The sandbox backend + runner agent
    shells already exist from CI-P1..CI-P4; the workflow-definition registration is CI-P15.)
  - The forward-only migrations for every CI table exactly as frozen in arch 01 §3: ci_run, ci_job,
    check_attempt, job_queue (+ the jq_claimable / jq_serialize / jq_idem indexes), fair_deficit, runner,
    log_segment, log_anchor, artifact, cache_entry, environment, deployment, secret_binding, cost_event,
    consumer_dedup. Every table carries tenant uuid + region text as leading columns (the partition key, 12.1;
    the residency-pin lint asserts row.region == cell.region on write); every personal-data field carries a
    #[personal_data(...)] tag (the no-untagged-personal-data lint); identity is stored as a pseudonym reference,
    never copied PII (triggered_by/approved_by are pseudonym subjects, 4.8). Hot-table flags declared (job_queue,
    log_segment, cost_event, check_attempt).
  - FLOOR named: the table SHAPES land here; the per-table behaviour (the scheduler claim over job_queue CI-P11,
    the check_attempt counter CI-P18, the log index CI-P20, the cost_event metering CI-P17) lands in its own
    prompt. State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 11.1 OLTP, 12.1 the partition key, 10.2 the personal-data tags,
  1.1/1.2/1.3 serve. Implement to the frozen shapes (arch 01 §3 is authoritative); escalate a needed change, do
  not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The five service shells boot under serve(AppSpec) with the three-surface split and liveness≠readiness (a
    readiness probe red until DB+broker+authz+runner-pool reachable) — CI.
  - The migrations apply forward-only (the forward-only-migration lint green) — CI.
  - The no-untagged-personal-data lint is GREEN on the full CI schema (0 untagged PII fields; red on a
    deliberately-untagged fixture, green on the tagged set); the residency-pin lint is GREEN (every CI write
    asserts row.region == cell.region); the tenant-predicate + no-cross-db lints green — CI, all lint signals = 0
    violations.
- **TESTS (required).** Unit tests that each shell boots and gates readiness on its deps, and that the migrations
  apply forward-only. The red+green fixture pair for the no-untagged-personal-data lint applied to a CI type.
  State whether any touched module is mandatory-core; if so, name the mutation-score floor.
- **DEFINITION OF DONE.** The five service shells boot under serve(AppSpec) with the three-surface split and
  liveness≠readiness; the migrations apply forward-only; all four named lints (no-untagged-personal-data,
  residency-pin, tenant-predicate, no-cross-db) are green; the unit tests pass; the contract-coverage scanner is
  green on the touched rows; the per-table-behaviour follow-ons are named; the work is committed. No gate is
  greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: CI five service shells + complete forward-only data-model migrations. Body
  lists: contracts 11.1/12.1/1.1/1.2/1.3 consumed; the lints greened (no-untagged-personal-data + residency-pin
  with fixtures); the per-table-behaviour follow-ons named. Branch first if on default; do not push unless asked.
  End with the workspace Co-Authored-By trailer.

---

### CI-P7 — The complete ci.* event taxonomy registered into the Bus seed

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4), the ci.*
  taxonomy slice (CI owns the ci.* token list).
- **DEPENDS-ON.** CI-P6 (the CI service shells + the migrations the events are emitted from). The M1 Bus prompt
  with the taxonomy seed (2.9) + the §6.2 singular-token grammar. The index places this after CI-P6 in CI's M4
  work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — first-class event propagation across all subsystems);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (reconcile contracts at the plan layer — CI
    registers tokens into the one grammar, it does not author the grammar).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md
    §1 (the complete ci.* event taxonomy).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.9 (event taxonomy + the new
    ci.check.updated / ci.result tokens), 2.1/2.2 (EventEnvelope + OutboxTx::emit — the only emit path).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §5 (rows 2.1/2.2/2.9).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate
  (myelin-ci-controlplane), the event-taxonomy registration:
  - The complete ci.* event taxonomy registered into the Bus seed (2.9): the X-1 tokens (ci.check.updated +
    ci.result), the lifecycle events (ci.run.started/succeeded/failed/cancelled/timed_out/reaped, ci.job.*,
    ci.deployment.*), the pointer/resource events (ci.log.available, ci.artifact.published, ci.cost.metered), the
    fleet/config/supply-chain events (ci.runner.*, ci.pipeline.*, ci.supply_chain.verification_failed), and the
    cross-cutting tombstone/snapshot events (ci.*.erased, ci.*.snapshot). Validate against the Bus §6.2 singular
    token table (ci is the canonical token + run/deployment/pipeline/runner/artifact type tokens) — CI registers,
    it does not author the grammar.
  - FLOOR named: none — the token LIST is frozen; each event's emission lands with its producing prompt (the
    check tokens CI-P18/CI-P19, the log/artifact tokens CI-P20, the deploy/supply-chain tokens CI-P23/CI-P24, the
    snapshot tokens CI-P22). State this.
- **CONTRACTS TO IMPLEMENT.** Owned: 2.9 the ci.* tokens (registered). Consumed: 2.1/2.2 EventEnvelope + outbox.
  Implement to the frozen shapes (arch 03 §1 is authoritative); escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The ci.* tokens are present in the Bus taxonomy and parse under the §6.2 grammar (0 ungrammatical tokens) —
    CI.
- **TESTS (required).** Unit tests that each ci.* token round-trips the §6.2 grammar. The CDC stub for row 2.9
  (the ci.* tokens). State whether any touched module is mandatory-core; if so, name the mutation-score floor.
- **DEFINITION OF DONE.** The complete ci.* taxonomy is registered and grammatical (0 ungrammatical tokens); the
  unit + CDC tests pass; the contract-coverage scanner is green on row 2.9; the per-event-emission follow-ons are
  named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: complete ci.* event taxonomy registered. Body lists: contract 2.9 owned + 2.1/
  2.2 consumed; the ci.* tokens grammatical (0 ungrammatical); the per-event-emission follow-ons named. Branch
  first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CI-P8 — The CI ReBAC namespace fragment (ci_project/environment/secret/run + read & !is_untrusted_fork)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "The CI
  ReBAC fragment"), the ReBAC-fragment slice (the fork-tier-never-reads ABAC edge).
- **DEPENDS-ON.** CI-P6 (the ci_run / environment / secret schema the namespaces project over). The M1 Identity
  prompt with the ReBAC namespace ENGINE (4.9 engine) into which the CI fragment compiles. The index places this
  after CI-P6 in CI's M4 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one permission model; agent-native);
    ../../external-insights/01-process-and-quality-doctrine.md §6 (investigate before you build — the trust
    classification is security-critical: get the read & !is_untrusted_fork edge right).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md
    §5.2 (the CI ReBAC namespace fragment).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen CI
    ReBAC fragment — ci_project/ci_environment/ci_secret/ci_run + read & !is_untrusted_fork).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.9 (the CI ReBAC fragment —
    frozen).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (the ReBAC fragment) + §5 (row
    4.9).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the ReBAC fragment:
  - The CI ReBAC namespace fragment submitted into the one cell schema Identity compiles (4.9, arch 03 §5.2): the
    frozen ci_project / ci_environment / ci_secret / ci_run namespaces + the read & !is_untrusted_fork ABAC edge
    (the fork-tier-never-reads rule) + the approver list_subjects target + the watcher relation per watchable
    type. The fragment must COMPILE in the cell schema.
  - FLOOR named: none — the fragment is frozen. The consumers of the fragment (the trust-tier classifier CI-P12,
    the secret-broker fork-no-secrets edge CI-P24, the list_objects push-down CI-P25) land in their own prompts.
    State this.
- **CONTRACTS TO IMPLEMENT.** Owned: 4.9 the CI ReBAC fragment (compiled by Identity). Implement to the frozen
  shape (arch 03 §5.2 + reconciliation §1 are authoritative); escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The CI ReBAC fragment COMPILES in the shared cell schema Identity builds (build-time gate) — CI.
- **TESTS (required).** Unit tests that the fragment compiles and the read & !is_untrusted_fork edge classifies a
  fork subject as non-reader. The CDC stub for row 4.9. State whether any touched module is mandatory-core; if
  so, name the mutation-score floor.
- **DEFINITION OF DONE.** The CI ReBAC fragment compiles in the cell schema; the read & !is_untrusted_fork edge
  classifies correctly; the unit + CDC tests pass; the contract-coverage scanner is green on row 4.9; the
  fragment-consumer follow-ons are named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: CI ReBAC fragment — ci_project/environment/secret/run + read &
  !is_untrusted_fork. Body lists: contract 4.9 owned; the fragment compiles in the cell schema; the
  fragment-consumer follow-ons named. Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### CI-P9 — The CI PersonalDataHolder (auto-registered, locate/export typed, erase stubbed to crypto-shred)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "the
  holder"), the PersonalDataHolder slice (every CI store auto-registers as a holder).
- **DEPENDS-ON.** CI-P6 (every CI store the holder registers over). The M1 GDPR prompt with the
  PersonalDataHolder trait (10.1) + the one erasure posture (10.9, X-7). The index places this after CI-P6/CI-P8
  in CI's M4 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction — data-subject rights as architectural constraints);
    ../../external-insights/01-process-and-quality-doctrine.md §5 (the ratchet — every store auto-registers as a
    holder; "we forgot the cache table" is structurally impossible).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md
    §6 (PersonalDataHolder); 01-tech-and-data-model.md §4 (the GDPR posture).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the one erasure
    posture instantiated by reference).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.1 (PersonalDataHolder), 10.9
    (the one erasure posture by reference), 1.4 (holder auto-registration on every store opened).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (the holder) + §5 (rows 10.1,
    10.9).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the holder module:
  - The CI PersonalDataHolder (10.1): the holder auto-registered by serve when each store opens (1.4); the
    locate/export/rectify/restrict/erase trait over run-state/logs/artifacts/caches/deployments STUBBED-but-typed
    here (the full crypto-shred erasure path is CI-P32's CI-D3 drill); the restrict flag wired at the per-subject
    check seam. The erasure residual is by reference to the one platform posture (X-7, 10.9) — CI does NOT restate
    a CI-local residual statement.
  - FLOOR named: the holder erase path is STUBBED here (locate/export typed, erase wired to crypto-shred in
    CI-P32/CI-D3); name CI-P32 as the follow-on. No feature ships here beyond the holder substrate.
- **CONTRACTS TO IMPLEMENT.** Owned: 10.1 the CI PersonalDataHolder (auto-registered, typed). Consumed: 10.9 the
  one erasure posture by reference. Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The PersonalDataHolder auto-registers on every CI store the harness opens (the holder-count signal includes
    every CI table — "we forgot the cache table" is structurally impossible) — CI.
- **TESTS (required).** Unit tests that every store auto-registers as a holder and that locate/export are typed
  over each store. The CDC stub for row 10.1. State whether the holder is mandatory-core; if so, name the
  mutation-score floor (the erase path's floor is named with CI-P32).
- **DEFINITION OF DONE.** The CI PersonalDataHolder auto-registers on every store; locate/export are typed; the
  restrict flag is wired; the unit + CDC tests pass; the contract-coverage scanner is green on row 10.1; the
  stubbed-erase floor is named (CI-P26); the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: CI PersonalDataHolder (auto-registered, locate/export typed, erase stubbed).
  Body lists: contract 10.1 owned + 10.9 by reference; the holder auto-registration proven; the stubbed-erase
  floor named (CI-P32). Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---
### CI-P10 — Trigger & Dispatch: the EventMatcher (= QueryAst) + exactly-once dedup + the trust-tier evaluation and single stamp

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "Trigger &
  dispatch"), the match + dedup + trust-classification slice (the front of every run, security-critical).
- **DEPENDS-ON.** CI-P6 (the dispatch service shell + the consumer_dedup ledger + the ci_run schema + the trust
  tier column). CI-P8 (the read & !is_untrusted_fork ABAC edge — the trust classification). The M2 Bus/Signals
  prompt that froze the EventMatcher = QueryAst (3.4). The M1 Identity prompt with check (4.2). The M3 Git
  prompts (git.ref.updated / git.pull_request.synchronized are the triggering events). The index places this
  after CI-P6/CI-P8 in CI's M4 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — first-class event propagation and triggers);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — exactly-once EFFECT under
    at-least-once delivery is a quantified property), §6 (investigate before you build — the trust-tier stamp is
    security-critical: get the classification right).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §1 (trigger → dispatch: match via EventMatcher, dedup on event_id, trust-tier evaluation+stamp);
    01-tech-and-data-model.md §2 (the trust_tier is one value stamped once — onto JobSpec.trust_tier AND every
    CheckStatus.trust_tier) + §3.8 (the dedup ledger); 03-events-contracts-and-glue.md §2 (events consumed) +
    §5.1 (Id.check at every entrypoint).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (trust_tier
    stamped by CI from run provenance; an untrusted_fork success is neutral for gating), §OQ-C (the QueryAst is
    the one expression language — not CEL).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 3.4 (EventMatcher = the frozen
    QueryAst), 2.5 (consumer_dedup — exactly-once effect), 4.2 (check + CaveatContext), 4.9 (the read &
    !is_untrusted_fork edge — the trust classification).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (trigger & dispatch — match,
    dedup, stamp trust_tier from provenance) + §2.2 (upstream deps rows 3.4, 13.3).
- **DELIVERABLE (what to build + exactly where in the repo).** In the Trigger & Dispatch crate
  (myelin-ci-dispatch):
  - The EventMatcher: a project's armed pipelines compile to the shared bounded QueryAst (3.4) — an
    on: pull_request: {...} compiles to a QueryAst, NOT a CI-specific trigger language, NOT CEL. The matcher runs
    close to the bus against the triggering events (git.ref.updated, git.pull_request.synchronized, git.pr.opened,
    issue.transitioned, manual, schedule, agent request).
  - Exactly-once dedup on the triggering event_id via the consumer_dedup ledger (2.5): one push = one run
    (exactly-once EFFECT even under at-least-once delivery).
  - Trust-tier evaluation + the single stamp (arch 02 §1.3): classify the run Trusted (member push) /
    UntrustedFork (PR from a fork or any run executing untrusted contributor code) / SelfHosted, using run
    provenance + the ReBAC ABAC edge read & !is_untrusted_fork (4.9). The result is stamped ONCE onto (a)
    JobSpec.trust_tier (gating secrets/cache-scope/egress) and (b) every emitted CheckStatus.trust_tier (X-1).
  - FLOOR named: the definition resolution → CAS snapshot + the reserve/start handoff is CI-P11; the
    sandboxed dynamic-generation escape hatch is wired in CI-P11. State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 3.4 EventMatcher = QueryAst, 2.5 consumer_dedup, 4.2 check + CaveatContext,
  4.9 the read & !is_untrusted_fork classification. Implement to the frozen shapes; escalate a needed change, do
  not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Exactly-once effect: a triggering event delivered twice (at-least-once) → exactly ONE ci_run (1 run per
    event_id; dedup-count signal = 0 duplicate runs) — CI.
  - The trust-tier stamp is correct and consistent: a fork PR → UntrustedFork stamped onto BOTH JobSpec.trust_tier
    AND CheckStatus.trust_tier (the same value; 0 divergence) — CI.
- **TESTS (required).** Unit tests for the QueryAst trigger compile (an on: pull_request matches the right
  events), the dedup ledger (one effect per event_id), and the trust-tier classifier (member→Trusted, fork→
  UntrustedFork). The CDC pair for the consumed rows 3.4 + 2.5 + 4.9 (the consumer side). The exactly-once-effect
  drill scenario on the failure-injection harness (deliver the trigger twice). State the cargo-mutants
  mutation-score floor for the trust-tier classifier (mandatory-core — the security classification).
- **DEFINITION OF DONE.** The EventMatcher compiles triggers to QueryAst; dedup yields exactly-once effect; the
  trust-tier is stamped once and consistently onto JobSpec + CheckStatus; the unit + CDC + drill tests pass; the
  contract-coverage scanner is green; the definition-resolution + dynamic-generation floor is named (CI-P11); the
  work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Trigger & Dispatch — EventMatcher + dedup + trust-tier stamp. Body lists:
  3.4/2.5/4.2/4.9 consumed; exactly-once-effect proven; trust-tier-stamp-consistency proven; the
  definition-resolution floor named (CI-P11). Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### CI-P11 — Trigger & Dispatch: the definition resolution → content-addressed snapshot + the reserve/start handoff

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "Trigger &
  dispatch"), the resolve + content-address + reserve+start slice.
- **DEPENDS-ON.** CI-P10 (the matched, deduped, trust-stamped trigger). CI-P6 (the ci_run schema). The M1 Storage
  prompt with the T2 BlobStore (the CAS snapshot blob, 11.2). The M2 Durable-workflow prompt that froze
  DurableExecutor::start (9.1). The index places this after CI-P10 in CI's M4 work (the ci.pipeline workflow body
  it starts is CI-P15).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign — reproducible, content-addressed definition);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — a floating-tag reference is rejected
    fail-closed, a quantified property).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §1 (definition resolution → CAS snapshot, reserve+start) + §7.4 (config grammar — the bounded QueryAst + the
    sandboxed dynamic-generation escape hatch + shift-left validate/plan); 03-events-contracts-and-glue.md §2
    (the ci.run.started emit).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.2 (BlobStore — the CAS
    definition snapshot), 9.1 (DurableExecutor::start — the reserve+start handoff to CI-P15's workflow), 2.2
    (outbox-only emit — ci.run.started + the first ci.check.updated).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (resolve + content-address the
    definition snapshot) + §2.2 (upstream dep 11.2).
- **DELIVERABLE (what to build + exactly where in the repo).** In the Trigger & Dispatch crate
  (myelin-ci-dispatch):
  - Definition resolution → the content-addressed snapshot: read .myelin/ci.* at the triggering commit, validate
    against the published JSON Schema, expand the matrix deterministically, resolve every component/image
    reference TO A DIGEST (fail-closed on a floating tag — the supply-chain hook proven in CI-P23), and write the
    resolved DAG as a CAS blob (T2, 11.2). The snapshot is identical to the myelin ci plan output (shift-left).
  - The reserve+start handoff: call DurableExecutor::start(StartSpec{input: snapshot_ref, ..}) for the
    ci.pipeline workflow (9.1) — the workflow body itself is CI-P15; here the dispatch writes the ci_run row and
    emits ci.run.started + the first ci.check.updated{state: queued} per context via the outbox in the same tx.
  - FLOOR named: the sandboxed dynamic-generation escape hatch (a job that emits a pipeline fragment, running in
    the same sandbox as any untrusted code) is the named programmatic-fan-out path — wire the hook here; the
    generation step runs on the CI-P3 runner (no privileged config-eval path). State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 11.2 BlobStore (the CAS snapshot), 9.1 DurableExecutor::start, 2.2 the
  outbox emit. Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - A floating-tag reference in the definition is REJECTED at resolution (fail-closed; 0 un-digested references
    reach a snapshot) — CI.
  - The reserve+start handoff is atomic: the ci_run row + ci.run.started + the queued check are written via the
    outbox in ONE tx (no partial run) — CI.
- **TESTS (required).** Unit tests for the digest-pin-or-reject snapshot resolver (a floating tag is refused) and
  the deterministic matrix expansion. The CDC pair for the consumed rows 11.2 + 9.1. State whether the resolver
  is mandatory-core; if so, name the mutation-score floor.
- **DEFINITION OF DONE.** The definition resolves to a digest-pinned CAS snapshot (floating tags rejected); the
  reserve+start handoff writes ci_run + the queued check via the outbox in one tx; the unit + CDC tests pass; the
  contract-coverage scanner is green; the dynamic-generation escape-hatch floor is named; the work is committed.
  No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: Trigger & Dispatch — definition resolution + CAS snapshot + reserve/start
  handoff. Body lists: 11.2/9.1/2.2 consumed; floating-tag-rejected proven; the atomic reserve+start handoff
  proven; the dynamic-generation floor named. Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---
### CI-P12 — Green-field core: the pull-lease claim query + concurrency groups + affinity + the dead-runner reaper

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "the
  distributed scheduler"), the claim-and-recover slice (the scheduler's whole intelligence is the claim; the
  reaper is its recovery side).
- **DEPENDS-ON.** CI-P6 (the job_queue schema + the jq_claimable/jq_serialize/jq_idem indexes + the control-plane
  shell). CI-P3 (the runner agent claims leases against this queue). The M1 Storage prompt (FOR UPDATE SKIP
  LOCKED on OLTP, 11.1). The index places this after CI-P6 (the DRR fair-share over the claim is CI-P13).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale from day 1 — the platform's heaviest scheduling problem);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the reaper-recovery + concurrency
    properties are quantified drills).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §2.1 (pull-leasing — the claim query with FOR UPDATE SKIP LOCKED, the lease, the heartbeat, the dead-runner
    reaper), §2.3 (concurrency groups — deploy:prod serialize + pr:web:42 cancel-superseded, affinity labels);
    01-tech-and-data-model.md §3.3 (the scheduler tables — job_queue + the claim indexes) + §3.4 (runners).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.1 (OLTP — the claim hot path +
    FOR UPDATE SKIP LOCKED), 1.8 (the telemetry signal set — scheduler queue-depth, claim latency, lease-reap
    count).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (the pull-leasing scheduler +
    concurrency/affinity + the reaper).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate
  (myelin-ci-controlplane), a scheduler module:
  - Pull-leasing: a runner long-polls and claims the next eligible job via the FOR UPDATE SKIP LOCKED claim query
    (arch 02 §2.1) — the claim encodes residency (region predicate), affinity (labels <@ runner_labels), trust
    (trust_tier = ANY(runner_allowed_tiers)), concurrency (the deploy:* serialize NOT EXISTS), lanes
    (lane_priority DESC), fairness (fair_deficit DESC — the DRR weights are advanced by CI-P13), then enqueued_at
    ASC — ONE query. On claim: set lease_owner/lease_expires, state='leased'.
  - Concurrency groups: deploy:prod as a serialization key (the jq_serialize partial unique index), pr:web:42 as
    cancel-superseded (a new push cancels the in-flight run for that group). Affinity: labels <@ runner_labels.
  - The dead-runner reaper: a sweep of expired leases → re-queue → which makes the run's SCHEDULE_AND_RUN_JOB
    activity retry idempotently (the enqueue is idempotent on idem_token via jq_idem; the reaper re-dispatch is
    one row).
  - FLOOR named: the DRR fair-share advance/replenish over fair_key + the priority lanes + backpressure are
    CI-P13; the flat-DRR → hierarchical scheduler follow-on is CI-P29 (measured-starvation-triggered). State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 11.1 OLTP (the claim hot path + FOR UPDATE SKIP LOCKED), 1.8 the
  telemetry set. Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Reaper recovery: kill a runner mid-lease → the reaper re-queues the job within the lease TTL, the re-dispatch
    is idempotent on idem_token (1 enqueue row, not 2) — CI, 0 orphaned jobs, 0 duplicate enqueues.
  - Concurrency: two deploy:prod jobs → at most ONE running at a time (the serialize index holds); a new PR push
    → the prior pr:<...> run is cancelled (cancel-superseded) — CI.
- **TESTS (required).** Unit tests for the claim predicate (each of residency/affinity/trust/concurrency/lane
  filters correctly), the reaper re-queue idempotency, and the concurrency-group serialize + cancel-superseded.
  The reaper-recovery drill scenario on the failure-injection harness. State the cargo-mutants mutation-score
  floor for the claim + reaper modules (mandatory-core — the scheduling correctness hot path).
- **DEFINITION OF DONE.** The pull-lease claim query + concurrency groups + affinity + the reaper exist and
  compile; the reaper recovers dead leases within the lease TTL with 0 orphans and 0 duplicate enqueues;
  concurrency serialize + cancel-superseded hold; the unit + drill tests pass; the contract-coverage scanner is
  green; the DRR-fair-share + hierarchical-scheduler floors are named (CI-P13 / CI-P29); the work is committed.
  No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: scheduler — pull-lease claim + concurrency + affinity + reaper. Body lists:
  11.1/1.8 consumed; reaper-recovery-within-lease-TTL proven (0 orphans); concurrency serialize/cancel-superseded
  proven; the DRR (CI-P13) + hierarchical (CI-P29) floors named. Branch first if on default; do not push unless
  asked. End with the workspace Co-Authored-By trailer.

---

### CI-P13 — Green-field core: DRR fair-share over fair_key + priority lanes + per-tenant backpressure

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "the
  distributed scheduler"), the fairness slice (the multi-tenant-starvation defence).
- **DEPENDS-ON.** CI-P12 (the claim query the DRR deficit feeds into; the fair_deficit ordering term). CI-P6 (the
  fair_deficit schema). The index places this after CI-P12 in CI's M4 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale — fairness across tenants from day 1);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the fairness property is a
    quantified drill, not an aspiration).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §2.2 (fairness — Deficit Round Robin over fair_key, the per-fair_key deficit counter, the
    canonical-CI-multi-tenant-starvation failure it prevents; the floor → hierarchical scheduler), §2.3 (priority
    lanes interactive>batch>deploy), §2.4 (backpressure & abuse — per-tenant in-flight caps, the bounded
    run-queue); 01-tech-and-data-model.md §3.3 (fair_deficit).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-K (the
    per-surface shed budget — bounded run-queue per tenant; the CI-surge lane shed order speculative → batch/CI →
    agent → human-last).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.11 (the protected-human-lane
    shed order + the CI-surge per-surface budget floor), 1.8 (the per-fair_key wait-time histogram).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (DRR fair-share + lanes; the
    floor → hierarchical scheduler).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the scheduler
  fairness module:
  - DRR fair-share over fair_key (= tenant or tenant:project): the per-fair_key deficit counter advanced at claim
    time (the term CI-P12's claim orders on), replenished weighted by plan tier, so one tenant's 10k-job matrix
    cannot starve every other tenant.
  - Priority lanes (interactive > batch > deploy) as the strict ORDER BY — the protected-human-lane analogue
    inside CI; composes with the platform shed order (1.11): under surge, shed batch first, hold interactive.
  - Backpressure: per-tenant in-flight caps (the bounded run-queue, OQ-K), statement timeouts; over-cap jobs queue
    gracefully, never collapse the scheduler.
  - FLOOR named: flat DRR fair-share at claim time → a richer hierarchical (per-tenant → per-project →
    per-pipeline) scheduler is the named follow-on (CI-P29), promotion-triggered by a measured per-fair_key
    starvation-histogram signal (open question 07#1). The 30x surge tuning of the DRR weights/budget numbers is
    CI-P30 (CI-M5). State both.
- **CONTRACTS TO IMPLEMENT.** Consumed: 1.11 the shed order (the CI-surge lane budget), 1.8 the per-fair_key
  histogram. Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - Fairness under contention: one tenant enqueues a large matrix while others enqueue interactive jobs → no
    other tenant is starved (the per-fair_key wait-time histogram bounded; the interactive lane holds its latency
    budget) — CI, fairness signal within budget. (The full 30x CI-D2 surge drill is CI-P30/CI-M5; here the
    deterministic fairness + lane property.)
- **TESTS (required).** Unit tests for the DRR deficit advance/replenish (plan-weighted) + the priority-lane
  ORDER BY + the per-tenant in-flight cap. The fairness drill scenario on the failure-injection harness. State
  the cargo-mutants mutation-score floor for the DRR module (mandatory-core).
- **DEFINITION OF DONE.** The DRR fair-share + priority lanes + per-tenant backpressure exist and compile; the
  fairness property holds (no starvation, interactive lane holds); the unit + drill tests pass; the
  contract-coverage scanner is green; the flat-DRR → hierarchical floor (CI-P29) + the surge-tuning follow-on
  (CI-P30) are named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: scheduler — DRR fair-share + priority lanes + per-tenant backpressure. Body
  lists: 1.11/1.8 consumed; fairness-no-starvation proven; the flat-DRR floor (CI-P29) + surge-tuning (CI-P30)
  named. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CI-P14 — Green-field core: the EU fleet autoscaler (FleetProvider + autoscale-on-queue-depth + per-residency-zone pools + fleet events)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "the EU
  fleet autoscaler"), CI's second genuine green-field core (the self-hosted attestation already shipped in
  CI-P4).
- **DEPENDS-ON.** CI-P12/CI-P13 (the scheduler — autoscaling reads queue depth off the job_queue). CI-P6 (the
  runner table + the fleet events). CI-P4 (the self-hosted attestation the fleet's runners pass before claiming).
  The M1 Tenancy prompt with (tenant, region) + residency_verify (12.1/12.4). The index places this after the
  scheduler in CI's M4 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign by construction — run entirely on EU-controlled infrastructure; world-scale);
    §4 (Rust default — the autoscaler is built, not rented, because ADR-11 declines the hyperscaler primitive);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — residency is a quantified drill).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §5 (the unified runner context) + the fleet sections (autoscale-on-queue-depth over EU IaaS/bare-metal behind
    FleetProvider; pre-warmed microVM snapshot pools; scale-to-zero; bin-packing; NO global pool — partitioned
    per residency zone); 01-tech-and-data-model.md §2 (the FleetProvider trait) + §3.4 (the runner table);
    00-overview.md §5 (cell topology — no global runner pool); 05-hard-problems.md HP-2 (runner-fleet elasticity
    on EU infra — the divergence-by-constraint).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §10 (the CI
    no-global-pool residency attestation).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 12.1 (the (tenant, region)
    partition key), 12.4 (residency_verify — covers the runner pool region), 1.6 (the residency-pin lint), 2.2
    (the fleet events via outbox).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (the autoscale-on-queue-depth +
    FleetProvider + no-global-pool-partitioned-per-residency-zone + the floor — one/two adapters) + §2.2 (rows
    12.1, 12.4).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, a fleet-autoscaler
  module:
  - The FleetProvider impl satisfying CI-P1's frozen trait (provision(class, n, region) / deprovision /
    capacity), with one-or-two concrete EU adapters (e.g. a generic-EU-IaaS or bare-metal-PXE adapter) +
    self-hosted as a delegated backend. K8s is a FleetProvider OPTION, never the default.
  - Autoscale-on-queue-depth: scale the per-(region, label-class) pool up when the scheduler's queue depth rises,
    scale-to-zero when idle, bin-packing under the microVM memory floor. NO global pool — every pool is
    partitioned per residency zone; provisioning passes region and the runner row is residency-pinned (the
    residency-pin lint asserts row.region == cell.region; residency_verify attests the pool region, 12.4).
  - Emit the fleet events (ci.runner.registered / attested / degraded / offline) via the outbox.
  - FLOOR named: one/two FleetProvider adapters → more EU-provider adapters (additive, demand-driven, adapters
    not redesigns). The cell-scale residency drill (CI-R3) is CI-P31. State both.
- **CONTRACTS TO IMPLEMENT.** Consumed: 12.1 the partition key, 12.4 residency_verify, 1.6 residency-pin, 2.2 the
  fleet events. Implement to the frozen FleetProvider shape (arch 01 §2); escalate a needed change, do not
  diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - No global pool: the residency-pin lint is GREEN on every fleet write (0 cross-region runner rows); a tenant's
    pool is provisioned only in its region — CI, residency-pin signal = 0 violations. (The full CI-R3 residency
    drill at cell scale is CI-P31/CI-M5; here the structural no-global-pool property.)
  - Autoscale: queue depth rising → the pool scales up; idle → scale-to-zero (the autoscale signal tracks queue
    depth) — CI.
- **TESTS (required).** Unit tests for the FleetProvider impl (provision/deprovision/capacity round-trip), the
  autoscale-on-queue-depth policy, and the per-residency-zone partition (a region-A pool never provisions in
  region B). The no-global-pool red+green residency-pin fixture. The CDC pair for the consumed row 12.4. State
  whether the autoscaler is mandatory-core; if so, name the mutation-score floor.
- **DEFINITION OF DONE.** The FleetProvider impl + one/two EU adapters + autoscale-on-queue-depth +
  per-residency-zone pools + the fleet events exist and compile; the residency-pin lint is green (no global
  pool); autoscale tracks queue depth; the unit + CDC + fixture tests pass; the contract-coverage scanner is
  green; the more-adapters + CI-R3 floors are named; the work is committed. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M4: EU fleet autoscaler — FleetProvider + per-residency-zone pools + fleet events.
  Body lists: 12.1/12.4/1.6/2.2 consumed; no-global-pool residency-pin greened; autoscale-on-queue-depth proven;
  the more-adapters floor + CI-R3 (CI-P31) named. Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---
### CI-P15 — The ci.pipeline durable workflow body + the determinism guard (CI-D9)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "pipeline
  orchestration (composition of frozen myelin-flow)"), the pipeline-IS-a-durable-workflow slice. Drill: CI-D9
  (determinism guard).
- **DEPENDS-ON.** CI-P11 (Trigger & Dispatch hands the CAS snapshot to DurableExecutor::start). CI-P6 (the
  workflow-def registration shell). The M2 Durable-workflow prompts that froze DurableExecutor + WfCtx
  (9.1/9.2) and the flow-determinism lint. The index places this after CI-P11 in CI's M4 work (the
  SCHEDULE_AND_RUN_JOB handshake is CI-P16; reserve/settle metering is CI-P17).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — the same engine under agent runs and CI pipelines);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — bit-identical replay is a quantified
    gate).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §3.1 (the hybrid boundary + the ci_pipeline pseudocode) + §3.2 (granularity: the activity boundary is the JOB
    not the step) + §3.4 (definition snapshot vs workflow versioning).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-F (the
    SCHEDULE_AND_RUN_JOB idiom — consumed by CI-P16).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 9.1 (DurableExecutor), 9.2 (WfCtx
    + the flow-determinism lint), 9.3 (the timer wheel — SLA timers), 1.6 (the flow-determinism lint).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row CI-D9 (determinism guard — flow-determinism lint, bit-identical replay).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (the ci.pipeline durable workflow
    + the determinism) + exit gate row CI-D9.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the ci.pipeline
  workflow definition (a deterministic Rust function registered on myelin-flow at serve, guarded by the
  flow-determinism lint):
  - The ci.pipeline workflow body exactly as the arch 02 §3.1 pseudocode: the snapshot is the resolved+pinned
    definition; stages gate sequentially (a protected-env/manual gate via ctx.wait_for_signal("approval:<stage>",
    window), 9.4 — may wait days); jobs within a stage dispatch in parallel respecting the needs DAG +
    concurrency group; on any failure emit_check(CheckStatus{failure}) per context + ctx.emit(ci.run.failed,
    structured_failure) + signal the merge queue (ci.result{failure}); on success emit_check(success) +
    ci.run.succeeded + signal(ci.result{success}). NO clock/RNG/IO outside WfCtx (the flow-determinism lint). The
    reserve_budget()/settle_budget() bookends are wired in CI-P17; the SCHEDULE_AND_RUN_JOB dispatch is CI-P16
    (here the body shape + the determinism property).
  - SLA timers (step/queue/deploy) on the durable timer wheel (9.3).
  - FLOOR named: none new here (the ci.pipeline body composes frozen myelin-flow). The SCHEDULE_AND_RUN_JOB
    handshake is CI-P16; the metering is CI-P17. State this.
- **CONTRACTS TO IMPLEMENT.** Implemented: 9.1 the ci.pipeline workflow registration, 9.2 the WfCtx body, 9.3 the
  SLA timers. Obeyed: 1.6 the flow-determinism lint. Implement to the frozen idiom; escalate a needed change, do
  not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D9 (determinism guard): the ci.pipeline body → the flow-determinism lint passes (no clock/RNG/IO outside
    WfCtx); replay is BIT-IDENTICAL; only the journaled job.done signal result feeds the body — CI, lint green +
    bit-identical replay.
- **TESTS (required).** Unit tests for the workflow body's stage-gating + per-context emit. The CI-D9 drill
  scenario on the failure-injection harness. The flow-determinism red+green fixture (a body with a raw clock
  fails the lint). State the cargo-mutants mutation-score floor for the workflow body (mandatory-core).
- **DEFINITION OF DONE.** The ci.pipeline workflow body + the SLA timers exist and compile under the
  flow-determinism lint; CI-D9 (bit-identical replay + lint green) emits its dated green artifact; the unit +
  drill tests pass; the flow-determinism lint is green with both fixtures; the contract-coverage scanner is
  green; the SCHEDULE_AND_RUN_JOB (CI-P16) + metering (CI-P17) follow-ons are named; the work is committed. No
  gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: ci.pipeline durable workflow body + determinism guard (CI-D9). Body lists:
  9.1/9.2/9.3 implemented; CI-D9 (bit-identical replay) greened; the flow-determinism lint greened with fixtures;
  the SCHEDULE_AND_RUN_JOB (CI-P16) + metering (CI-P17) follow-ons named. Branch first if on default; do not push
  unless asked. End with the workspace Co-Authored-By trailer.

---

### CI-P16 — The SCHEDULE_AND_RUN_JOB long-park idiom + crash-recovery / effectively-once (CI-D1)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "pipeline
  orchestration"), the dispatch-and-park slice. Drill: CI-D1 (crash-recovery / effectively-once).
- **DEPENDS-ON.** CI-P15 (the ci.pipeline body that calls SCHEDULE_AND_RUN_JOB). CI-P12/CI-P13 (the scheduler —
  SCHEDULE_AND_RUN_JOB enqueues into job_queue). CI-P3 (the runner reports job.done). The M2 Durable-workflow
  prompts that froze the SCHEDULE_AND_RUN_JOB idiom + the durable job.done signal (9.2/9.4). The index places
  this after CI-P15 in CI's M4 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — effectively-once is a quantified
    gate), §4 (chain mutations end-to-end — kill the runner mid-job, kill the control plane mid-run).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §3.3 (THE frozen SCHEDULE_AND_RUN_JOB handshake — dispatch+park, woken by the job.done signal idempotent on
    idem_token, the reaper retry = effectively-once).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-F (the
    SCHEDULE_AND_RUN_JOB idiom + the per-effect idem_key + the job.done signal).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 9.2 (the SCHEDULE_AND_RUN_JOB
    long-park idiom), 9.4 (the durable job.done signal).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row CI-D1 (kill runner + control plane → effectively-once, 0 lost/double-deploy/duplicate-publish).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (SCHEDULE_AND_RUN_JOB) + exit
    gate row CI-D1.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the
  SCHEDULE_AND_RUN_JOB activity:
  - The SCHEDULE_AND_RUN_JOB handshake (9.2/9.4, arch 02 §3.3): the activity mints the idem_token at the workflow
    (deterministic from command_id — producer/consumer agree with NO round-trip), enqueues the job into job_queue
    (idempotent on idem_token via jq_idem) with lane/labels/trust-tier/concurrency-group/fair-key from the
    snapshot, journals activity_completed, and RETURNS (holds no runtime); the workflow
    wait_for_signal("job.done", idem_key=idem_token) parks; the runner's terminal signal(run, "job.done",
    {result}, idem_key=idem_token) is idempotent (delivered twice → wakes once). The activity boundary is the JOB,
    not the step (keeps the journal small at CI's firehose volume; step progress is firehose/log state, recovered
    by re-running on retry). The reserve-at-dispatch / settle-on-job.done bookends are wired in CI-P17.
  - FLOOR named: the reserve/settle metering bookends are CI-P17. State this.
- **CONTRACTS TO IMPLEMENT.** Implemented: 9.2 the SCHEDULE_AND_RUN_JOB idiom, 9.4 the job.done signal wait.
  Implement to the frozen idiom (OQ-F); escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D1 (crash-recovery / effectively-once): kill the runner mid-job; kill the control plane mid-run → the run
    resumes (workflow replay + SCHEDULE_AND_RUN_JOB idempotent re-dispatch on idem_token) → 0 lost runs, 0
    double-deploys, 0 duplicate artifact publishes (effectively-once) — CI, double-effect count = 0.
- **TESTS (required).** Unit tests for the SCHEDULE_AND_RUN_JOB handshake (the idem_token determinism, the
  idempotent enqueue, the idempotent job.done wake). The CI-D1 drill scenario on the failure-injection harness
  (CI-D1 chains mutations end-to-end — kill mid-run, assert effectively-once — per EI-01 §4). State the
  cargo-mutants mutation-score floor for the handshake module (mandatory-core).
- **DEFINITION OF DONE.** The SCHEDULE_AND_RUN_JOB idiom exists and compiles; CI-D1 (effectively-once) emits its
  dated green artifact; the unit + drill tests pass; the contract-coverage scanner is green; the reserve/settle
  follow-on (CI-P17) is named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: SCHEDULE_AND_RUN_JOB idiom + crash-recovery / effectively-once (CI-D1). Body
  lists: 9.2/9.4 implemented; CI-D1 (0 double-effect) greened; the reserve/settle follow-on (CI-P17) named.
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CI-P17 — Reserve/settle = the one metering path + the cost_event ledger + parity CI ↔ agent (CI-D5)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 —
  "metering"), the reserve/settle-is-the-one-metering-path slice. Drill: CI-D5 (reserve/settle parity CI ↔
  agent).
- **DEPENDS-ON.** CI-P16 (the SCHEDULE_AND_RUN_JOB dispatch the reserve fronts; the job.done the settle hangs
  off). CI-P15 (the ci.pipeline body's reserve_budget()/settle_budget() bookends). CI-P6 (the cost_event schema).
  The M1 Storage reserve/settle gate (11.7). The index places this after CI-P16 in CI's M4 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — one wallet, one meter for CI + agent compute, X-6);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 over-exhaustion starts + wholesale
    ≠ markup are quantified), §7 (names AND units — integer minor-units, never floats; wholesale vs markup
    columns up front).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §6 (reserve/settle — the one metering path) + §8 (the metering algorithm — resource-seconds, wholesale ≠
    markup); 01-tech-and-data-model.md §3.7 (the cost_event schema — integer minor-units, wholesale + markup
    separate columns).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-6 (the
    reserve/settle is the one metering path for both CI and agent).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.7 (reserve/settle — fronts
    every run + every SCHEDULE_AND_RUN_JOB dispatch), 9.5 (workflow↔agent mapping — reserve/settle = the
    bookends).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row CI-D5 (exhaust wallet, start CI run + agent compute; replay across pricing change → refuse-start, 0
    over-exhaustion starts, wholesale ≠ markup).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (the metering) + exit gate row
    CI-D5.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the metering
  module:
  - Reserve/settle = the one metering path (11.7/9.5, arch 02 §6/§8): reserve_budget() at workflow start
    (refuse-to-start if exhausted, never interrupt in flight); reserve at each SCHEDULE_AND_RUN_JOB dispatch;
    settle on job.done. Resource-seconds (cpu_seconds, mem_gb_seconds, gpu_seconds, storage_gb_hours, egress_gb)
    as the wholesale meter; one cost_event row per metered unit; integer minor-units (NEVER floats); wholesale and
    markup in separate columns (Commercial owns the markup mapping — arch 06 R-2 named follow-on); kind ∈ {ci,
    agent} for reporting, same schema, same wallet (UNIFY / X-6).
  - FLOOR named: the Commercial resource-second → credit/price markup mapping is a named follow-on (arch 06 R-2)
    owned by Commercial; CI owns only the meter + the wholesale column. State this.
- **CONTRACTS TO IMPLEMENT.** Implemented: 11.7 reserve/settle (the one metering path), 9.5 the reserve/settle
  bookends. Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D5 (reserve/settle parity CI ↔ agent): exhaust the wallet, start a CI run + an agent compute job → BOTH
    refuse-start (never interrupt in flight); replay across a pricing change → 0 starts past exhaustion, wholesale
    ≠ markup holds (one cost_event per metered unit) — CI, 0 over-exhaustion starts, cost parity.
- **TESTS (required).** Unit tests for the reserve/settle bookends (refuse-start-on-exhaustion,
  settle-on-job.done) and the cost_event integer-minor-units invariant. The CI-D5 drill scenario on the
  failure-injection harness. State the cargo-mutants mutation-score floor for the reserve/settle + cost_event
  modules (mandatory-core).
- **DEFINITION OF DONE.** Reserve/settle + the cost_event ledger exist and compile; CI-D5 (reserve/settle parity,
  0 over-exhaustion starts, wholesale ≠ markup) emits its dated green artifact; the unit + drill tests pass; the
  contract-coverage scanner is green; the Commercial-markup follow-on is named; the work is committed. No gate is
  greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: reserve/settle one metering path + cost_event ledger + parity (CI-D5). Body
  lists: 11.7/9.5 implemented; CI-D5 (0 over-exhaustion starts, wholesale ≠ markup) greened; the Commercial-markup
  follow-on named. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---
### CI-P18 — The X-1 check_attempt monotonic counter + the ci.check.updated producer (the CheckStatus fact)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "the X-1
  CheckStatus producer half"), the check-fact-emission slice (CI is the SOURCE of run_attempt).
- **DEPENDS-ON.** CI-P15 (the ci.pipeline workflow emits the check from its body). CI-P6 (the check_attempt
  counter table + the tokens registered in CI-P7). CI-P10 (the trust_tier stamp the CheckStatus carries). The M2
  reconciliation that froze the 5.9 CheckStatus shape. The index places this in CI's M4 work; the ci.result
  rollup + the seam gate is CI-P19.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (the connective tissue — work flows between tools);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — monotonic supersession is
    quantified), §7 (reconcile cross-component contracts at the plan layer: CI is the producer, the seam is
    frozen).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §4 (the Git↔CI check seam, produced — bump the (commit_oid, context) attempt counter, assemble CheckStatus,
    emit ci.check.updated via the outbox; what CI does NOT do); 01-tech-and-data-model.md §3.2 (the check_attempt
    counter — CI's source of run_attempt; monotonic, never wall-clock); 03-events-contracts-and-glue.md §4 (the
    CheckStatus seam — the full shape, what CI owns vs what Git owns); 05-hard-problems.md HP-0 (the seam,
    frozen — the poisoned-pipeline-execution defence).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 / OQ-A (the
    frozen CheckStatus struct, the monotonic-run_attempt supersession, the fork-trust-tier gating).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.9 (the Git↔CI CheckStatus seam —
    CI is the producer; ci.check.updated), 2.9 (the ci.check.updated token), 2.2 (outbox-only emit), 5.7 (the
    details_ref = #step-<n> sub-anchor), 7.3 (the summary as a HumanisedRef).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows GIT-D10 + CI-D8 (the run_attempt-monotonic supersession half — the full seam gate is CI-P19).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (the producer half) + §0 (X-1 is
    the tightest seam).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the check-emitter
  module:
  - The check_attempt counter logic (arch 01 §3.2): on a new run / re-run for (commit_oid, context),
    UPDATE check_attempt SET next_attempt = next_attempt + 1 RETURNING (attempt - 1) — the returned attempt is
    stamped into the emitted CheckStatus.run_attempt. CI is the SOURCE of run_attempt (monotonic, never
    wall-clock — clocks are not authority); Git's last-writer-wins is on the attempt.
  - Emit ci.check.updated per (commit_oid, context) carrying the frozen CheckStatus struct (arch 03 §4):
    {repo, commit_oid, context, state ∈ {queued, in_progress, success, failure, error, neutral, cancelled},
    run, run_attempt, trust_tier (the value stamped at trigger time, CI-P10), details_ref = #step-<n>
    (jump-to-failure, resolves through CI-P21's log index), summary as a (template_key, args) HumanisedRef (7.3,
    NEVER a raw string), started_at, completed_at?, cost_settled (flips true only when the reserve settles — a
    check is not final until settled)}. Emitted via the OUTBOX ONLY (2.2), aggregate = (repo, commit_oid) so all
    checks for one commit are per-aggregate ordered; references-not-payloads (never log bytes).
  - The trust-tier discipline (the poisoned-pipeline-execution defence): a CheckStatus with trust_tier =
    untrusted_fork is recorded faithfully but CI NEVER endorses it — CI stamps the tier from provenance only. CI
    does NOT own the projection table, does NOT decide required, does NOT recompute trust, does NOT merge.
  - FLOOR named: the ci.result rollup signal + the end-to-end seam gate GIT-D10/CI-D8 are CI-P19;
    external-provider checks via CheckStatus{provider:external} are a demand-driven follow-on. State both.
- **CONTRACTS TO IMPLEMENT.** Owned (the producer half, the check-fact): 5.9 the Git↔CI CheckStatus seam
  (ci.check.updated + the run_attempt source). Consumed: 2.9 the token, 2.2 outbox emit, 5.7 the #step-<n>
  details_ref, 7.3 the humanised summary. Implement to the frozen 5.9 shape EXACTLY (CI never diverges the seam —
  a needed change is a whole-workspace contract PR, escalated and written down); code-wins-over-docs.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The check_attempt counter is monotonic: a re-run bumps the attempt; a lower attempt is the stale one (CI
    never uses wall-clock for supersession; the run_attempt is the only supersession key) — CI.
  - ci.check.updated is well-formed: the summary is a HumanisedRef not a raw string; cost_settled flips only on
    settle; emitted via the outbox only (the no-raw-publish lint green) — CI.
- **TESTS (required).** Unit tests for the check_attempt monotonic counter (a re-run bumps the attempt; a lower
  attempt is the stale one) and the CheckStatus assembly (cost_settled flips only on settle; summary is a
  HumanisedRef). The CDC provider stub for row 5.9 (the check-fact half; the consumer Git half + the end-to-end
  gate is CI-P19). State the cargo-mutants mutation-score floor for the check-emitter + attempt-counter module
  (mandatory-core — the seam correctness).
- **DEFINITION OF DONE.** The check_attempt counter + the ci.check.updated producer exist and emit via the
  outbox; CI stamps run_attempt monotonically and trust_tier from provenance, never endorsing forks; the unit +
  CDC tests pass; the no-raw-publish lint is green; the contract-coverage scanner is green on the check-fact half
  of row 5.9; the ci.result + seam-gate (CI-P19) + external-provider follow-ons are named; the work is committed.
  The seam is implemented to the frozen 5.9 shape exactly — no local divergence. No gate is greened by weakening
  a threshold.
- **COMMIT.** Header: P-<NNN> M4: X-1 check_attempt counter + ci.check.updated producer. Body lists: contract 5.9
  (check-fact half) owned + 2.9/2.2/5.7/7.3 wired; monotonic run_attempt proven; the ci.result + seam-gate
  (CI-P19) + external-provider follow-ons named. Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---

### CI-P19 — The X-1 ci.result rollup signal + the GIT-D10 / CI-D8 check-seam end-to-end GATE (0 double-merge)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "the X-1
  CheckStatus producer half"), the merge-queue-wake + seam-gate slice. Drill: GIT-D10 / CI-D8 (the X-1 check seam
  end-to-end, 0 double-merge).
- **DEPENDS-ON.** CI-P18 (the ci.check.updated producer + the check_attempt counter). CI-P15 (the ci.pipeline
  workflow body emits ci.result from its body). The M3 Git prompt that OWNS the check_status projection table +
  the run_attempt supersession rule + the branch-protection required-set + the fork-endorsement
  (approve_untrusted_ci) + the merge-queue durable workflow (git-hosting GIT-P7 — the consumer half this prompt
  closes). The index places this as the X-1 seam-closer in CI's M4 work; its M4-exit drill GIT-D10/CI-D8 is the
  joint Git+CI gate.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (the connective tissue — work flows between tools without friction);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 double-merge is quantified), §7
    (reconcile cross-component contracts at the plan layer — CI is the producer, Git the gate, the seam is
    frozen).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §4 (emit ci.result once all required contexts terminal; what CI does NOT do);
    03-events-contracts-and-glue.md §1.1 (the ci.result frozen token) + §4 (the CheckStatus seam — what CI owns
    vs what Git owns); 05-hard-problems.md HP-0 (the poisoned-pipeline-execution defence).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 / OQ-A (the
    ci.result merge-queue signal, the fork-trust-tier gating — an untrusted_fork success neutral until endorsed).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 5.9 (the Git↔CI CheckStatus seam —
    the ci.result rollup signal), 2.9 (the ci.result token), 9.4 (the ci.result durable signal that wakes Git's
    merge queue).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows GIT-D10 + CI-D8 (the X-1 check seam: out-of-order/dup ci.check.updated → run_attempt-monotonic
    supersession; fork self-green neutral; maintainer endorses → green; doubly-delivered ci.result → merge-queue
    wakes exactly once; 0 double-merge; 1 current row per (commit_oid, context); merge-count == 1).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 + exit gate row GIT-D10/CI-D8.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the check-emitter
  module (the rollup half):
  - Emit the ci.result rollup SIGNAL (not a bus event) once all required contexts for the commit reach terminal:
    signal(merge_queue_run, "ci.result", {commit_oid, overall: success|failure, contexts, idem_token}), idempotent
    on idem_token — this wakes Git's merge-queue durable workflow (9.4).
  - The fork-gating discipline at the seam: Git treats an untrusted_fork success as neutral for gating until a
    maintainer endorses or the context is re-run trusted — CI emits the fact, Git gates. CI does NOT merge.
  - FLOOR named: none new — this closes the X-1 seam. State that the seam is frozen and CI never diverges it.
- **CONTRACTS TO IMPLEMENT.** Owned (the producer half, the rollup): 5.9 the ci.result rollup signal. Consumed:
  2.9 the token, 9.4 the ci.result signal wait (Git's side). Implement to the frozen 5.9 shape EXACTLY; escalate
  a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - GIT-D10 / CI-D8 (the X-1 check seam end-to-end, the joint Git+CI gate): (a) out-of-order/dup ci.check.updated
    → run_attempt-monotonic supersession holds (lower attempt DROPPED, higher SUPERSEDES; exactly 1 current row
    per (commit_oid, context) in Git's projection); (b) a fork PR self-greens → NEUTRAL for gating; (c) maintainer
    endorses → green; (d) doubly-delivered ci.result → the merge-queue workflow wakes EXACTLY ONCE; 0 double-merge
    (merge-count == 1) — CI, 1 current row per key + merge-count == 1.
- **TESTS (required).** Unit tests for the ci.result idempotency (idem_token). The GIT-D10 / CI-D8 drill scenario
  on the failure-injection harness — deliver ci.check.updated out-of-order and duplicated, deliver ci.result
  twice, assert supersession + exactly-once merge-queue wake + 0 double-merge (the CDC pair for row 5.9 — CI
  provider + Git consumer — is the seam's contract test; the contract-coverage scanner fails the build without
  it). State the cargo-mutants mutation-score floor for the rollup module (mandatory-core — the seam
  correctness).
- **DEFINITION OF DONE.** The ci.result rollup signal exists and emits idempotently on idem_token; GIT-D10 /
  CI-D8 emits its dated green artifact (1 current row per key, merge-count == 1, fork neutral, supersession
  monotonic); the unit + drill tests + the 5.9 CDC pair pass; the contract-coverage scanner is green on row 5.9;
  the work is committed. The seam is implemented to the frozen 5.9 shape exactly — no local divergence. No gate is
  greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: X-1 ci.result rollup + GIT-D10/CI-D8 check-seam GATE. Body lists: contract 5.9
  (rollup half) owned + 2.9/9.4 wired; GIT-D10/CI-D8 greened (1 current row/key, merge-count == 1,
  fork-success-neutral, monotonic supersession). Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---
### CI-P20 — Logs over the firehose + the sealed T3 (job, step, byte-range) log tier + ci.log.available pointers

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "logs /
  artifacts / caches"), the log-production slice (CI is the heaviest firehose producer).
- **DEPENDS-ON.** CI-P6 (the log_segment / log_anchor schema). CI-P3 (the runner streams firehose frames). The
  M2 Bus prompt that froze the firehose transport (3.5). The M1 Storage prompts with the T2 BlobStore (11.2) +
  the T3 log tier (job,step,byte-range) index (11.8). The index places this in CI's M4 work; the resume-cursor
  live-tail is CI-P21; artifacts/caches are CI-P22.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign — residency-local logs);
    ../../external-insights/04-hard-problems.md §2 (the resume-cursor transport is the durable real-time
    transport built first);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — observability is part of the pass).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §7.1 (logs ride the firehose; ci.log.available is the only durable log event, coalesced never per-line; the
    durable archive — sealed segments → T2 + the (job,step,byte-range) index); 01-tech-and-data-model.md §3.5
    (the log range index — log_segment + log_anchor).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 3.5 (firehose publish), 11.8 (the
    T3 log tier — the (job,step,byte-range) index resolves the details_ref), 11.2 (BlobStore T2 — sealed
    segments), 2.2 (the ci.log.available pointer via outbox).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (logs ride the firehose; frames
    seal into the T3 log tier).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the log-pipeline
  coordinator:
  - The log pipeline (arch 02 §7.1): ship_line redacts secrets (in-flight masking — DEFENCE-IN-DEPTH, not the
    boundary), firehose::publish the frame keyed by (run, job, step) (the LIVE TAIL — never the durable bus; CI is
    the heaviest firehose producer), seals segments into T2 content-addressed blobs + the (job, step, byte-range)
    index (log_segment / log_anchor, 11.8), and emits ci.log.available POINTER events (coalesced, never per-line)
    via the outbox.
  - FLOOR named: the object-segment T3 log tier + the OLTP (job,step,byte-range) index ships v1 → a dedicated
    time-series/wide-column log tier is the named follow-on (CI-P29/CI-M5), promoted ONLY once event volume is
    MEASURED to outgrow the OLTP-indexed tier (EI-04 §5 — not before). The resume-cursor live-tail (CI-P21) + the
    per-subject DEK (CI-P22) are separate prompts. State all of this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 3.5 the firehose publish, 11.8 the T3 (job,step,byte-range) index (CI is
  the heaviest consumer — co-owns the index usage), 11.2 BlobStore (sealed segments), 2.2 the ci.log.available
  pointer. Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ci.log.available is coalesced never per-line (0 per-line durable bus events; the live tail is firehose-only)
    — CI.
  - Sealed segments index correctly: a sealed segment writes a (job, step, byte-range) anchor (0 dangling
    anchors at seal time) — CI.
  - The residency-pin lint is green on every log write (logs near the runner region) — CI.
- **TESTS (required).** Unit tests for the segment-seal + the (job,step,byte-range) index write + the
  ci.log.available coalescing. The CDC pair for the consumed rows 3.5 + 11.8 + 11.2. State whether the log
  pipeline is mandatory-core; if so, name the mutation-score floor.
- **DEFINITION OF DONE.** The log pipeline (firehose publish + segment seal + the T3 index + ci.log.available
  pointers) exists and compiles; ci.log.available is coalesced; sealed segments index with 0 dangling anchors;
  the residency-pin lint is green; the unit + CDC tests pass; the contract-coverage scanner is green; the
  time-series-log-tier (CI-P29) + resume-cursor (CI-P21) + DEK (CI-P22) follow-ons are named; the work is
  committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: CI logs (firehose + sealed T3 index + ci.log.available pointers). Body lists:
  3.5/11.8/11.2/2.2 consumed; ci.log.available coalesced; sealed segments 0 dangling anchors; the
  time-series-log-tier (CI-P29) + resume-cursor (CI-P21) + DEK (CI-P22) follow-ons named. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CI-P21 — The resume-cursor live-tail + the details_ref jump-to-failure resolution (CI-D11)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "logs /
  artifacts / caches"), the live-tail + jump-to-failure slice. Drill: CI-D11 (live-log reconnect-loses-zero-ops).
- **DEPENDS-ON.** CI-P20 (the firehose log pipeline + the sealed T3 index). CI-P18 (the CheckStatus.details_ref =
  #step-<n> resolves through this log index). The M2 Bus prompt that froze the resume-cursor subscription
  protocol (3.5, OQ-J). The index places this after CI-P20 in CI's M4 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign);
    ../../external-insights/04-hard-problems.md §2 (the resume-cursor transport is the durable real-time
    transport);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 lost lines is quantified).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §7.1 (the resume-cursor protocol; the durable archive range-read fallback) + §4 (the details_ref =
    #step-<n> jump-to-failure path).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-J (the firehose
    resume-cursor protocol), §OQ-D (the details_ref jump-to-failure).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 3.5 (firehose +
    resume-cursor subscribe/resume/scope), 11.8 (the T3 index resolves the details_ref).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row CI-D11 (drop the live-tail mid-run, reconnect with last_seq → backfill (last_seq, now]; 0 lines lost;
    last_seq past window → resync_required → range-read; scope bounded never *).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (the resume-cursor live-tail) +
    exit gate row CI-D11.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the live-tail
  coordinator:
  - The resume-cursor live-tail (3.5, OQ-J): a viewer subscribe(stream, scope = run:<id>/job:<id>, cursor?); on
    reconnect resume(stream, scope, last_seq) backfills (last_seq, now] then goes live (a reconnect loses ZERO
    log lines); a last_seq past the retention window → resync_required → fall back to a range-read of the sealed
    segments (CI-P20). Scope is BOUNDED, never *.
  - The details_ref resolution path: CheckStatus.details_ref = .../ci/run/<id>#step-<n> resolves through
    log_anchor → log_segment → the byte range (the X-1 / OQ-D jump-to-failure path); step ids are opaque and
    stable across retries (assigned deterministically from the snapshot, not runtime order).
  - FLOOR named: none new (the live-tail composes the frozen firehose transport). State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 3.5 the resume-cursor protocol, 11.8 the T3 index (the details_ref
  resolution). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D11 (live-log reconnect-loses-zero-ops): drop the live-tail mid-run, reconnect with last_seq → the
    firehose backfills (last_seq, now] → 0 log lines lost; a last_seq past the retention window → resync_required
    → a clean range-read fallback; scope stays bounded (never *) — CI, 0 lost lines.
  - The details_ref jump-to-failure resolves: a CheckStatus.details_ref = #step-<n> resolves through
    log_anchor → the byte range (0 dangling step anchors) — CI.
- **TESTS (required).** Unit tests for the resume-cursor backfill (the (last_seq, now] math + the resync_required
  fallback + the bounded-scope rejection of *) and the details_ref resolution (step-<n> → byte range). The CI-D11
  drill scenario on the failure-injection harness. The CDC pair for the consumed row 3.5. State whether the
  live-tail is mandatory-core; if so, name the mutation-score floor.
- **DEFINITION OF DONE.** The resume-cursor live-tail + the details_ref resolution exist and compile; CI-D11 (0
  lost lines) emits its dated green artifact; the details_ref resolves (0 dangling step anchors); the unit + drill
  + CDC tests pass; the contract-coverage scanner is green; the work is committed. No gate is greened by weakening
  a threshold.
- **COMMIT.** Header: P-<NNN> M4: CI resume-cursor live-tail + details_ref jump-to-failure (CI-D11). Body lists:
  3.5/11.8 consumed; CI-D11 (0 lost lines) greened; the details_ref jump-to-failure proven (0 dangling anchors).
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CI-P22 — Trust-scoped artifacts & caches + the within-EU CDN clone class + per-subject log DEK (CI-D6)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 — "logs /
  artifacts / caches"), the artifact/cache + poisoning-defence slice. Drill: CI-D6
  (fork-cannot-poison-trusted-cache).
- **DEPENDS-ON.** CI-P6 (the artifact / cache_entry schema). CI-P20 (the log segments the per-subject DEK keys).
  CI-P10 (the trust_tier that scopes the cache namespace). The M1 Storage prompts with the T2 BlobStore + the
  trust-tier/branch-scoped cache namespaces (C4) + the within-EU CDN clone class (C3) + per-subject DEK (11.4).
  The index places this after CI-P20 in CI's M4 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign — residency-local artifacts/caches);
    ../../external-insights/04-hard-problems.md §5 (reindex-from-source for derived stores);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 fork-cache-poison is quantified).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §7.2 (artifacts & caches — content-addressed, poison-resistant via trust-scoped namespaces, residency-local);
    01-tech-and-data-model.md §3.5 (per-subject DEK for inline log PII) + §3.6 (the cache_entry scope = the
    trust-tier/branch namespace).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §8 (the trust-scoped
    cache namespaces + the within-EU CDN clone class + the per-subject CI-log DEK).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.2 (BlobStore T2 + the
    trust-tier/branch-scoped cache namespaces + the within-EU CDN clone class), 11.4 (per-subject DEK for isolable
    inline log PII), 2.2 (the ci.artifact.published pointer).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row CI-D6 (UntrustedFork run writes the default-branch cache scope → trust-tier/branch namespace holds
    structurally; 0 trusted-cache writes from a fork).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (artifacts/caces) + exit gate row
    CI-D6.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the artifact/cache
  modules:
  - Artifacts & caches on T2 BlobStore (11.2, BLAKE3, per-tenant dedup — cross-tenant dedup is a residency leak):
    artifacts are retained outputs (ArtifactRef-addressable, explicit TTL/GC); caches are reconstructible (key =
    hash(lockfile + os + toolchain), LRU). The cache_entry.scope = the trust-tier/branch-scoped namespace (C4):
    an UntrustedFork write lands in a fork-scoped namespace and CANNOT reach the trusted (default-branch) cache
    scope — Storage enforces the write-scope rule STRUCTURALLY (the storage half of the X-1 trust-tier defence).
    Blobs live near the runner region; the within-EU CDN clone/bundle class (C3) accelerates hot-repo clones
    within EU only.
  - Per-subject DEK for isolable inline log PII (11.4): where a subject's inline PII in a segment is isolable, the
    log_segment.pii_key_ref names a per-subject DEK (subject:<id>); per-tenant DEK is the fallback. (The erasure
    crypto-shred drill CI-D3 is CI-P32.)
  - FLOOR named: per-subject DEK crypto-shred is BUILT here as the key-selection substrate; the full erase fan-out
    (CI-D3) is CI-P32. State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 11.2 BlobStore + the trust-scoped cache namespaces + the CDN clone class,
  11.4 per-subject DEK, 2.2 the ci.artifact.published pointer. Implement to the frozen shapes; escalate a needed
  change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D6 (fork-cannot-poison-trusted-cache): an adversarial UntrustedFork run attempts to write the
    default-branch cache scope → the trust-tier/branch-scoped namespace holds STRUCTURALLY → 0 trusted-cache
    writes from a fork-tier run — CI, 0 fork→trusted writes.
  - The within-EU CDN clone class never leaves EU (the residency-pin lint green on every artifact/cache write) —
    CI.
- **TESTS (required).** Unit tests for the cache-scope derivation (a fork run derives a fork scope; cannot write
  the trusted scope) and the per-subject DEK selection (isolable PII → per-subject DEK; else per-tenant). The
  CI-D6 drill scenario on the failure-injection harness. The CDC pair for the consumed rows 11.2 + 11.4. State
  the cargo-mutants mutation-score floor for the cache-scope-enforcement module (mandatory-core — the poisoning
  defence).
- **DEFINITION OF DONE.** Trust-scoped artifacts/caches + the within-EU CDN clone class + per-subject DEK
  selection exist and compile; CI-D6 (0 fork→trusted writes) emits its dated green artifact; the residency-pin
  lint is green on every artifact/cache write; the unit + drill + CDC tests pass; the contract-coverage scanner is
  green; the erase-fan-out floor (CI-P32) is named; the work is committed. No gate is greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M4: trust-scoped artifacts/caches + within-EU CDN class + per-subject log DEK
  (CI-D6). Body lists: 11.2/11.4/2.2 consumed; CI-D6 (0 fork→trusted writes) greened; the residency-pin lint
  green; the erase-fan-out floor (CI-P32) named. Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---
### CI-P23 — Supply-chain trust: digest-pin-or-fail-closed + sigstore sign/verify + SLSA/SBOM provenance (CI-D4)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 —
  "supply-chain"), the supply-chain-fail-closed slice. Drill: CI-D4 (supply-chain fail-closed).
- **DEPENDS-ON.** CI-P11 (the definition resolver digest-pins at plan time — this prompt makes the fail-closed
  rule real + sign-verify). CI-P6 (the supply-chain events registered in CI-P7). The M1 Identity/Audit prompt
  with the tamper-evident audit log / CT-Merkle pattern (10.6). The index places this in CI's M4 work; the secret
  broker + deployments are CI-P24.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign by construction — sigstore EU-hosted);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 un-pinned/unsigned executions are
    quantified gates).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/05-hard-problems.md HP-4 (the
    component/action registry supply-chain — digest-pin-or-fail-closed, sign+verify-before-use sigstore
    Fulcio+Rekor, SLSA L1-L2 provenance + SBOM); 03-events-contracts-and-glue.md §1.4
    (ci.supply_chain.verification_failed); 02-internals-and-algorithms.md §7.4 (shift-left validate/plan).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.6 (the tamper-evident audit
    log — the sigstore Rekor / CT-Merkle pattern is shared), 2.2 (ci.supply_chain.verification_failed via outbox),
    4.7 (OIDC short-lived audience-scoped credentials over static keys — the build identity).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row CI-D4 (floating tag / tampered-unsigned component → digest-pin + sign-verify fail closed at plan/run;
    ci.supply_chain.verification_failed emitted; 0 un-pinned/unsigned executions).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (supply-chain) + exit gate row
    CI-D4.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the supply-chain
  verifier:
  - Supply-chain (arch 05 HP-4): digest-pin-or-fail-closed for images AND components (a floating tag REFUSED at
    plan time — the highest-leverage supply-chain control; this makes CI-P11's resolver rule enforced at run too);
    sign + verify-before-use (sigstore Fulcio keyless + Rekor transparency log, EU-hosted — reuse the platform's
    CT-Merkle pattern, 10.6); SLSA L1-L2 provenance (signed: which run, which snapshot, which inputs built an
    artifact) + SBOM (CycloneDX/SPDX) for produced artifacts; emit ci.supply_chain.verification_failed on any
    refusal (the fail-closed proof, audit-critical).
  - FLOOR named: SLSA L1-L2 + SBOM ships v1 → hermetic / two-party (L3+) provenance is a demand-triggered
    follow-on (CI-M5/demand). The component trust model (digest-pin + sign-verify + SLSA) is built regardless; the
    registry PRODUCT (hosting/discovery) is commercial-flagged. State both.
- **CONTRACTS TO IMPLEMENT.** Consumed: 10.6 the audit log / CT-Merkle pattern, 2.2 the verification_failed event,
  4.7 OIDC credentials (the build identity). Implement to the frozen shapes; escalate a needed change, do not
  diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D4 (supply-chain fail-closed): a floating tag / a tampered-unsigned component → digest-pin + sign-verify
    fail CLOSED at plan/run; ci.supply_chain.verification_failed emitted → 0 un-pinned executions, 0
    unsigned-component runs — CI, 0 un-pinned runs + audit event.
- **TESTS (required).** Unit tests for the digest-pin-or-reject rule (a floating tag is refused), the
  sign-verify-before-use, and the SLSA/SBOM attestation generation. The CI-D4 drill scenario on the
  failure-injection harness. State the cargo-mutants mutation-score floor for the supply-chain verifier
  (mandatory-core — security-load-bearing).
- **DEFINITION OF DONE.** The supply-chain verifier (digest-pin + sigstore sign/verify + SLSA/SBOM) exists and
  compiles; CI-D4 (0 un-pinned/unsigned executions + the verification_failed audit event) emits its dated green
  artifact; the unit + drill tests pass; the contract-coverage scanner is green; the SLSA-L3+ and registry-product
  floors are named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: supply-chain (digest-pin + sigstore + SLSA/SBOM) fail-closed (CI-D4). Body
  lists: 10.6/2.2/4.7 consumed; CI-D4 (0 un-pinned/unsigned + verification_failed) greened; the SLSA-L3+ and
  registry-product floors named. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### CI-P24 — The in-boundary secret broker (fork-gets-no-secrets, CI-D7) + deployments & the protected-env HITL gate

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 —
  "deployments & HITL ... secrets"), the secrets + HITL-deploy slice. Drill: CI-D7 (fork-gets-no-secrets).
- **DEPENDS-ON.** CI-P2 (secrets resolve inside the sandbox boundary). CI-P8 (the read & !is_untrusted_fork
  edge — fork gets no secrets). CI-P15 (deploy gates are durable-signal waits in the ci.pipeline workflow). CI-P6
  (the environment / deployment / secret_binding schema). The M1 Identity prompt with list_subjects (the approver
  set, 4.4). The M2 Notif prompt with humanise (7.3 — the approval card). The index places this after CI-P15 in
  CI's M4 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign — OIDC short-lived credentials);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 fork secret reads is a quantified
    gate), §8 (the human sign-off is the bottleneck — protected-env deploy is decision-shaped: the HITL gate,
    per-effect idem_key).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §7.3 (secrets resolved inside the boundary — names in the spec, an in-boundary broker scoped per job, OIDC
    short-lived audience-scoped credentials, untrusted/fork runs get NO secrets by default, log masking is
    defence-in-depth NOT the boundary); 05-hard-problems.md HP-8 (secrets); 03-events-contracts-and-glue.md §1.2
    (the ci.deployment.* HITL flow); 01-tech-and-data-model.md §3.7 (environment/deployment/secret_binding).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-F (the per-effect
    idem_key for batch approval cards), §X-6 (the requires_approval defaults — deploy/secret = yes).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.4 (list_subjects — the HITL
    approver set), 4.9 (the read & !is_untrusted_fork ABAC edge — fork gets no secrets), 9.4 (the durable approval
    signal — multi-day HITL), 7.3 (humanise — the approval card), 4.7 (OIDC short-lived audience-scoped
    credentials over static keys).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row CI-D7 (adversarial fork run reads protected secrets → read & !is_untrusted_fork holds; 0 secret reads by a
    fork-tier run).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (deployments & HITL + secrets) +
    exit gate row CI-D7.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the secret broker +
  the deployment/HITL modules:
  - The in-boundary secret broker (arch 02 §7.3): the JobSpec carries secret NAMES (SecretRef), not values; the
    broker resolves them AFTER the sandbox is up, scoped to exactly this job's references, via the shared secret
    capability (placed under Id/GDPR). OIDC short-lived audience-scoped federated credentials over static keys.
    UNTRUSTED/FORK runs get NO secrets by default (the read & !is_untrusted_fork ABAC edge, 4.9 — the canonical
    "fork exfiltrates prod secrets" CVE class); protected environments require explicit grants/approval.
  - Deployments & HITL (arch 03 §1.2): protected-environment gates as durable signals
    (ci.deployment.approval_required → the approved signal, per-effect idem_key, OQ-F); the approver set resolves
    via list_subjects(env, approve) (4.4); the approval queue + the chat approval card via humanise (7.3);
    rollback first-class (ci.deployment.rolled_back — reversibility, not "are you sure?").
  - FLOOR named: none new (the deploy gate composes frozen myelin-flow signals + the frozen requires_approval
    defaults). State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 4.4 list_subjects (the approver set), 4.9 the read & !is_untrusted_fork
  edge, 9.4 the durable approval signal, 7.3 humanise (the card), 4.7 OIDC credentials. Implement to the frozen
  shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D7 (fork-gets-no-secrets): an adversarial fork run attempts to read protected secrets → the read &
    !is_untrusted_fork ABAC edge holds → 0 secret reads by a fork-tier run; protected-env secrets require explicit
    grant/approval — CI, 0 fork secret reads.
  - HITL: a protected-env deploy withholds until approved; a double-click approval (per-effect idem_key) is ONE
    approval; a declined effect is withheld (returns Denied, never mutates) — CI.
- **TESTS (required).** Unit tests for the in-boundary secret broker scoping (a fork run resolves to NO secrets;
  a trusted run resolves only its referenced names) and the per-effect idem_key approval (double-click = one
  apply). The CI-D7 drill scenario on the failure-injection harness. The CDC pair for the consumed rows 4.4 +
  4.9 + 9.4. State the cargo-mutants mutation-score floor for the secret broker (mandatory-core —
  security-load-bearing).
- **DEFINITION OF DONE.** The in-boundary secret broker + the deployment/HITL gates exist and compile; CI-D7 (0
  fork secret reads) emits its dated green artifact; the HITL per-effect idem_key holds (double-click = one
  apply); the unit + drill + CDC tests pass; the contract-coverage scanner is green; the work is committed. No
  gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: in-boundary secret broker (CI-D7) + deploy HITL gate. Body lists: 4.4/4.9/9.4/
  7.3/4.7 consumed; CI-D7 (0 fork secret reads) greened; the per-effect idem_key HITL proven. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---
### CI-P25 — Cross-fabric surfacing: the list_objects SetExpr push-down + ArtifactRef/#sub mints + project(ref, viewer)

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 —
  "cross-fabric surfacing (facts only)" + "list_objects over run_id"), the leak-free-read + ref-mint slice (CI
  reports; it gates nothing itself).
- **DEPENDS-ON.** CI-P6 (the ci_run schema). CI-P8 (the ReBAC fragment the push-down filters through). CI-P18
  (the check fact the unfurls render). CI-P21 (the #step-<n> log anchors project resolves). The M2 Refs prompts
  (ArtifactRef + the #sub grammar + the project() requirement + the 4-step tombstone ladder, 5.1/5.6/5.7). The M1
  Identity prompt with list_objects + the SetExpr push-down (4.3). The index places this near the end of CI's M4
  work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (the cross-artifact reference graph — work flows between tools);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the leak-free pre-filter is a
    quantified property), §5 (the search-requires-acl-filter lint is a committed gate).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md
    §5.1 (the list_objects SetExpr push-down over ci_run.run_id — the OQ-E JOIN, no N+1, no post-filter) + §7.1
    (the ArtifactRef + the #sub mints — step-<n>, check-<context>, L<a>-L<b>) + §7.2 (project(ref, viewer) — the
    only cross-DB read of a CI artifact); 04-views-cli-and-api.md §2 (the CLI — myelin ci list uses the SetExpr
    push-down).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-E (the SetExpr
    push-down), §X-4/OQ-D (the #sub grammar + tombstone ladder).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.3 (list_objects SetExpr
    push-down — the leak-free pre-filter, the search-requires-acl-filter lint), 5.1 (ArtifactRef — the ci mints),
    5.6 (project(ref, viewer) — REQUIRED on every subsystem), 5.7 (the #sub grammar — ci owns step-/check-/
    L<a>-L<b>).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (the list_objects JOIN +
    cross-fabric surfacing) + §5 (rows 4.3, 5.1, 5.6, 5.7).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the surfacing
  module (the read + ref half):
  - The list_objects SetExpr push-down over ci_run.run_id (4.3, arch 03 §5.1): the run list / all-runs /
    release-readiness / CI search lower Filter{set_expr, zookie} to a JOIN against the per-tenant authz reverse
    index over the ci_run.run_id column (ColRef{table:"ci_run", column:"run_id"}) — ONE query, NO N+1 per-row
    check, NO post-filter (the search-requires-acl-filter lint). The CLI myelin ci list rides this.
  - ArtifactRef mints + the #sub grammar (5.1/5.7, arch 03 §7.1): mint myelin://<t>/ci/<type>/<id>[#sub] using
    the canonical ci token (types run/deployment/pipeline/runner/artifact); the CI-owned #sub kinds step-<n>
    (jump-to-failure, resolves CheckStatus.details_ref) + check-<context> + L<a>-L<b>; step ids opaque and stable
    across retries; Refs stores the full sub-URN + the stripped root (a broken sub-anchor still resolves to the
    parent run); the 4-step tombstone ladder degrades broken anchors.
  - project(ref, viewer) (5.6, arch 03 §7.2): the ONLY cross-subsystem read of a CI artifact — per-viewer,
    pre-permission-checked (Id.check viewer view → Deny returns a Tombstone, never leaks); returns {title, state,
    icon, render_hint, sub_anchor?} for run/deployment/pipeline. Backs the chat run unfurl, the PR context pane,
    the knowledge embed, the inbox humanisation, the search snippet.
  - FLOOR named: declare_indexable + humanise + replay + the ToolDef registrations are CI-P26. State this.
- **CONTRACTS TO IMPLEMENT.** Owned/implemented: 5.1 ArtifactRef mints, 5.6 project(ref, viewer), 5.7 the ci #sub
  mints. Consumed: 4.3 list_objects (the SetExpr push-down). Implement to the frozen shapes; escalate a needed
  change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The list_objects SetExpr push-down is leak-free: a partial-visibility run list returns ONLY the visible rows
    via the JOIN (0 leak, ONE query, revoke reflected); the search-requires-acl-filter lint is GREEN (Search
    always conjoins the Filter before scoring) — CI, 0 leaked rows + lint green. (The E2E-1 PR context pane 0-leak
    is CI-P33.)
  - project(ref, viewer) tombstones on deny: a viewer without view permission gets a Tombstone, never a
    projection (0 title leak) — CI.
- **TESTS (required).** Unit tests for the SetExpr-lowering (the JOIN over run_id, no N+1), project()'s
  permission-checked tombstone, and the #sub mint stability across retries. The CDC pairs for the owned rows 5.6
  + 5.7 and the consumed row 4.3. The leak-free-list drill scenario on the failure-injection harness. State
  whether the SetExpr-lowering is mandatory-core; if so, name the mutation-score floor.
- **DEFINITION OF DONE.** The list_objects SetExpr push-down + the ArtifactRef/#sub mints + project() exist and
  compile; the run-list is leak-free (0 leaked rows, the search-requires-acl-filter lint green); project()
  tombstones on deny; the unit + drill + CDC tests pass; the contract-coverage scanner is green on every touched
  row; the declare_indexable/humanise/replay/ToolDef follow-on (CI-P26) is named; the work is committed. No gate
  is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: CI surfacing — list_objects push-down + ArtifactRef/#sub + project. Body lists:
  5.1/5.6/5.7 owned + 4.3 consumed; the leak-free run-list proven (0 leak, lint green); project()-tombstones-on-
  deny proven; the declare_indexable/humanise/replay/ToolDef follow-on (CI-P26) named. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CI-P26 — Cross-fabric surfacing: declare_indexable + humanise registrations + replay(*.snapshot) + the ToolDef registrations

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 —
  "cross-fabric surfacing"), the index/humanise/replay/agent-surface slice.
- **DEPENDS-ON.** CI-P25 (project() the IndexSpec projection reuses; the ci #sub mints). CI-P7 (the ci.*.snapshot
  tokens replay emits). The M2 Search prompt (declare_indexable, 6.3). The M2 Notif prompt (humanise, 7.3). The
  M2 Agent fabric prompt with ToolSurface::register_tool (8.1). The M1 Identity prompt with list_subjects (the
  watcher read-fanout, 4.4). The index places this at the end of CI's M4 surfacing work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (the cross-artifact reference graph; agents are first-class);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — replay is the only rebuild path, a
    quantified no-cross-db property), §5 (the ratchet).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md
    §7.3 (replay — reindex-from-source, sub-artifact-granular) + §7.4 (the IndexSpec); 04-views-cli-and-api.md §1
    (the view inventory CI feeds) + §3 (the ToolDef registrations with the frozen X-6 requires_approval defaults).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-6 (the
    requires_approval defaults).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 6.3 (declare_indexable — the CI
    IndexSpec), 7.3 (humanise — CI status summaries + cards), 2.6 (replay — sub-artifact-granular *.snapshot), 8.1
    (ToolDef + the frozen requires_approval defaults), 4.4 (list_subjects — the watcher read-fanout).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 (cross-fabric surfacing) + §5
    (rows 6.3, 7.3, 2.6, 8.1, 4.4).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate, the surfacing
  module (the index/humanise/replay/tool half):
  - declare_indexable (6.3, arch 03 §7.4): the CI IndexSpec (acl_object_type: ci_run so Search pre-filters via
    list_objects; ft_fields, struct_fields, the semantic failure_summary field; the projection reuses project());
    the restriction flag honoured (a restricted subject's runs excluded from the index).
  - humanise registrations (7.3): every CI status summary + agent-authored card/message registers into the
    NOTIF-1 ICU template registry — (template_key, args) + routable ArtifactRefs, NEVER a raw string. The
    CheckStatus.summary is a HumanisedRef.
  - replay(scope, since) (2.6, arch 03 §7.3): emit ci.run.snapshot / ci.deployment.snapshot / ci.pipeline.snapshot
    through the outbox → the live consumer path; sub-artifact-granular (one-run scope); also the post-restore
    re-erasure path.
  - The ToolDef registrations (8.1, arch 04 §3): register CI's agent-facing actions into the one ToolSurface with
    the FROZEN X-6 requires_approval defaults — ci.run/cancel/retry/read_log/read_run/validate/plan =
    requires_approval no; ci.deploy (protected) / ci.approve_deploy / ci.rollback (prod) / ci.write_secret =
    requires_approval YES. ToolHands::exec is NOT in this table (it is the runner itself, never a side-effecting
    tool). The structured ci.run.failed payload (which step, which test, log excerpt) is the deliberate
    agent-native triage hook (the E2E-2 flagship reads it, CI-P34).
  - FLOOR named: the SCIP/LSIF "find usages" code-search input (6.5) is a named follow-on (post-CI-M5) — CI
    produces the artifact, Search consumes it later (arch 06 R-3). State this.
- **CONTRACTS TO IMPLEMENT.** Owned/implemented: 6.3 declare_indexable, 7.3 the humanise registrations, 2.6
  replay, 8.1 the ToolDef registrations. Consumed: 4.4 list_subjects (the watcher fanout). Implement to the
  frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - replay is the only rebuild path: a ci.run.snapshot through the live consumer rebuilds the Search/Refs/OLAP
    derived view WITHOUT reading CI's DB (the no-cross-db lint green) — CI.
  - declare_indexable honours restriction: a restricted subject's runs are excluded from the index (0 restricted
    rows indexed) — CI.
  - The ToolDef requires_approval defaults are frozen-correct (deploy/secret = yes; run/read = no) — CI.
- **TESTS (required).** Unit tests for the IndexSpec projection (reuses project()), the humanise template
  resolution, replay's no-cross-db rebuild, and the ToolDef requires_approval defaults. The CDC pairs for the
  owned rows 6.3 + 7.3 + 2.6 + 8.1. State whether any touched module is mandatory-core; if so, name the
  mutation-score floor.
- **DEFINITION OF DONE.** declare_indexable + humanise + replay + the ToolDef registrations exist and compile;
  replay rebuilds without reading CI's DB (no-cross-db lint green); the index honours restriction; the ToolDef
  defaults are frozen-correct; the unit + CDC tests pass; the contract-coverage scanner is green on every touched
  row; the SCIP/LSIF follow-on is named; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M4: CI surfacing — declare_indexable + humanise + replay + ToolDefs. Body lists:
  6.3/7.3/2.6/8.1 owned + 4.4 consumed; replay-rebuilds-without-CI-DB proven (no-cross-db green); the ToolDef
  defaults frozen-correct; the SCIP/LSIF follow-on named. Branch first if on default; do not push unless asked.
  End with the workspace Co-Authored-By trailer.

---
### CI-P27 — Re-confirm the two permanent gates at the M4 boundary: AG-D4 / CI-T1 on the prod runner image + STOR-D1/STOR-D2 restore-verify on the CI stores

- **BAND.** M4.
- **ROADMAP MILESTONE.** CI-M4 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M4 exit gate —
  "CI-T1 / AG-D4 re-confirmed green on the production CI runner image"). The two permanent gates re-confirmed at
  the M4 boundary (ledger-overview §3.2 rule 4 — the permanent gates get explicit re-confirm prompts in each band
  that re-runs them). Both gates are committed-CI-job wiring over one prod artifact, so they are one
  permanent-gate-re-confirmation unit.
- **DEPENDS-ON.** CI-P5 (the original AG-D4 green attestation + the escape corpus). CI-P6 through CI-P26 (the full
  CI subsystem now exists, including the production CI runner image with the real CI workload paths). The M1
  Storage restore-verify prompt (STOR-D1/STOR-D2, 11.5). The index places this as the LAST CI prompt in M4 — the
  M4→M5 band boundary cannot pass while either permanent gate is red.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors; the code wins over the docs — a dated green artifact, never a claim);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (the gate invariant — no later band over a red
    earlier gate; the two permanent gates ratchet across the whole build), §3 (prove-it — re-run the escape drill
    on the actual production image; the green attestation IS the pass), §5 (an uncommitted gate is no gate — wire
    these as committed CI jobs).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §5.5 (the escape drill re-runs on EVERY backend/image/kernel change); 07-drills-and-open-questions.md §1 row
    T-1 + §3.
  - Master sequencing: ../06-roadmaps/00-master-sequencing.md §2 (the M4 exit gate — CI-T1/AG-D4 re-confirmed on
    the prod runner) + §4 (the two permanent gates).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.4 (the escape drill — permanent
    gate), 11.5 (backup/restore/cross-seam + restore-verify, CI-gated, ADR-18 — RPO ≤ 5 min, RTO ≤ 1h/tenant ≤
    4h/cell, 0 loss).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows CI-T1 / AG-D4 (= the M2 escape drill, re-run on the prod image) + STOR-D1 / STOR-D2 (restore-verify; the
    cross-seam OLTP↔blob↔index↔offset consistent point).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §6 (the two CI-relevant permanent gates —
    AG-D4/CI-T1 and, transitively, STOR-D1/STOR-D2).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI CI-jobs config + the drill-harness:
  - Wire the AG-D4 / CI-T1 escape drill (CI-P5's adversarial corpus + green-attestation format) as a COMMITTED CI
    job that runs against the PRODUCTION CI runner image (the image CI-P6..CI-P26 actually run on), re-confirming
    ZERO escapes on the real production backend + image + kernel. This is not new drill code — it is the committed
    re-run gate on the actual prod artifact (an uncommitted re-run is no gate, EI-01 §5).
  - Wire the STOR-D1 / STOR-D2 restore-verify (11.5) as a committed CI job over the CI stores: restore the CI
    OLTP + the T2 blob (artifacts/caches) + the T3 log segments + the event-log offset from backups to ONE
    consistent cross-seam point (OLTP↔blob↔index↔offset), assert 0 loss, RPO ≤ 5 min, RTO ≤ 1h/tenant ≤ 4h/cell.
    (Storage owns the restore-verify machinery from M1; this prompt wires the CI stores into it — every change
    touching a CI store re-runs it.)
  - FLOOR named: none — these are permanent gates, both the floor and the full answer; they re-run forever (gVisor
    re-runs AG-D4 at CI-P28). State this.
- **CONTRACTS TO IMPLEMENT.** Re-confirmed (no new contract — committed gate wiring): 8.4 (the escape drill on the
  prod image), 11.5 (restore-verify over the CI stores). Implement to the frozen shapes; escalate a needed
  change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-T1 / AG-D4 re-confirmed (the hard GATE): the adversarial corpus on the PRODUCTION CI runner image on a
    real kernel → ZERO escapes; a DATED GREEN ESCAPE ATTESTATION (prod backend + image digest + kernel version) —
    GATE. M5 cannot start over a red re-confirm.
  - STOR-D1 / STOR-D2 over the CI stores: restore from backups → 0 loss; OLTP↔blob↔index↔offset one consistent
    point; RPO ≤ 5 min, RTO ≤ 1h/tenant ≤ 4h/cell — GATE (permanent; re-runs on every CI-store change).
- **TESTS (required).** The AG-D4 / CI-T1 re-run scenario on the failure-injection harness against the prod image
  (reusing CI-P5's corpus). The STOR-D1/STOR-D2 restore-verify scenario over the CI stores (the cross-seam
  consistency assertion). No new unit logic; the test is the committed-gate wiring + its green artifact. State
  that the gates are committed CI jobs (an uncommitted gate is no gate).
- **DEFINITION OF DONE.** The AG-D4 / CI-T1 escape drill is a committed CI job re-confirmed GREEN on the
  production CI runner image (dated attestation, ZERO escapes); the STOR-D1/STOR-D2 restore-verify is a committed
  CI job GREEN over the CI stores (0 loss, RPO/RTO within thresholds); both are wired loud-never-swallowed (no ||
  true); the no-floor permanent-gate note is written; the work is committed. Neither gate is claimed green over a
  red drill — a red AG-D4 or STOR-D1 blocks M5 and becomes a dated no-go scorecard row, never a weakened
  threshold.
- **COMMIT.** Header: P-<NNN> M4: re-confirm AG-D4/CI-T1 on prod runner image + STOR-D1/D2 restore-verify on CI
  stores. Body lists: 8.4 re-confirmed (ZERO escapes on prod image, dated attestation) + 11.5 restore-verify
  greened over the CI stores (0 loss, RPO ≤ 5 min); both wired as committed CI jobs; the permanent-gate note.
  Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---
### CI-P28 — Floor follow-on: the gVisor second backend behind the SandboxBackend trait + re-greening the escape GATE (trigger-gated)

- **BAND.** M5.
- **ROADMAP MILESTONE.** CI-M5 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 — "the floor
  follow-ons"). The gVisor named-floor promotion whose density/latency trigger has fired. Permanent gate: the
  escape drill re-runs on the gVisor backend.
- **DEPENDS-ON.** CI-P1 (the SandboxBackend trait — gVisor slots in behind it), CI-P2 (the hardening profile,
  backend-independent), CI-P5 (the escape corpus that re-runs on the new backend), CI-P30 (the measured
  density/latency-economics trigger). The index places this in CI's M5 work, after CI-P30's measurements.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — a named floor with a scheduled follow-on is correct; the floor's
    promotion is triggered, never premature); §4 (the strategy pattern — a backend swap is a config/impl swap,
    not a rewrite); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the gVisor escape
    gate re-greens or gVisor is no-go).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §5.1 (gVisor as the named second backend behind the same trait, its own drill — the density/latency trigger);
    05-hard-problems.md HP-1 (the backend floor); 07-drills-and-open-questions.md §1 (T-1 re-runs on every
    backend).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 8.4 (the escape drill re-runs on
    the gVisor backend — the permanent gate), 1.8 (the telemetry signals that trigger the promotion).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row AG-D4 / CI-T1 (re-run on the new backend — the permanent gate).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 (the gVisor floor follow-on) + §4
    (the floors table — gVisor, with its trigger).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI sandbox crate (myelin-ci-sandbox),
  GATED on the measured density/latency trigger (if the trigger has NOT fired, gVisor stays a named floor — do
  not build it speculatively; record the trigger status dated in the gap report):
  - A gVisor impl behind the SandboxBackend trait (CI-P1) — a config/impl swap, NOT a rewrite. The hardening
    profile is backend-independent (applies identically, CI-P2).
  - The escape drill (CI-P5's adversarial corpus) RE-RUNS on the gVisor backend (the permanent gate — zero
    escapes on the new backend or gVisor is no-go).
  - FLOOR named: this is a TRIGGER-GATED promotion. If the density/latency trigger has not fired at M5, the floor
    REMAINS named (do not build it) and the trigger status is recorded dated. State this.
- **CONTRACTS TO IMPLEMENT.** Re-confirmed/extended: 8.4 (the escape drill on gVisor — the permanent gate).
  Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - IF gVisor is promoted: the escape drill RE-RUNS on the gVisor backend → ZERO escapes; a dated green escape
    attestation on the new backend — GATE. (If the trigger has not fired, gVisor stays a named floor; record the
    trigger status — no gate owed.)
- **TESTS (required).** If promoted: the escape drill re-run scenario on gVisor (the full adversarial corpus). If
  NOT promoted: a dated gap-report entry recording the trigger status (untested-but-named is acceptable; silent
  skipping is the failure — EI-01 §4). State the mutation-score floor for any gVisor hardening module built.
- **DEFINITION OF DONE.** gVisor is EITHER promoted (built behind the trait + its escape gate green-and-dated on
  the new backend) OR left a named floor with its dated trigger status recorded in the gap report; where
  promoted, the drill tests pass and the contract-coverage scanner is green; the work is committed. No gate is
  greened by weakening a threshold; a gVisor escape gate is NEVER claimed green over a red attestation.
- **COMMIT.** Header: P-<NNN> M5: gVisor second backend + escape-gate re-green (trigger-gated). Body lists:
  whether gVisor fired (with its greened escape re-green attestation) or remains a named floor (dated trigger
  status); 8.4 re-confirmed where promoted. Branch first if on default; do not push unless asked. End with the
  workspace Co-Authored-By trailer.

---

### CI-P29 — Floor follow-ons: the time-series log tier + the hierarchical scheduler (each measured-trigger-gated)

- **BAND.** M5.
- **ROADMAP MILESTONE.** CI-M5 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 — "the floor
  follow-ons"). The two measured-trigger-gated non-gate promotions (log volume; scheduler starvation).
- **DEPENDS-ON.** CI-P20 (the object-segment T3 log tier the time-series tier promotes from), CI-P13 (the flat
  DRR scheduler the hierarchical one promotes from), CI-P30 (the measured triggers — measured log volume, the
  measured per-fair_key starvation signal). The index places this in CI's M5 work, after CI-P30's measurements.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — promotion is triggered, never premature);
    ../../external-insights/04-hard-problems.md §5 (the time-series tier is promoted ONLY once volume is MEASURED
    — not before).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §7.1 (the object-segment T3 tier → time-series tier, measured volume) + §2.2 (the flat DRR → hierarchical
    promotion, the measured starvation signal); 05-hard-problems.md HP-7 (the log-tier floor);
    07-drills-and-open-questions.md §2 (open questions 1, 4 — the promotion triggers).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §8 (the
    (job,step,byte-range) index → the dedicated log tier seam).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.8 (the T3 log tier — the
    time-series promotion preserving the addressability contract), 11.2 (BlobStore), 1.8 (the telemetry signals
    that trigger each promotion).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 (the floor follow-ons) + §4 (the
    floors table — time-series log tier, hierarchical scheduler, with their triggers).
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI control-plane crate, EACH promotion
  GATED on its measured trigger (a promotion whose trigger has NOT fired stays a named floor — do not build it
  speculatively; record the trigger status dated in the gap report):
  - The time-series/wide-column log tier (the measured-volume trigger): promoted from the object-segment T3 floor
    (CI-P20) ONLY once event volume is MEASURED to outgrow the OLTP-indexed object-segment tier (EI-04 §5). The
    (job, step, byte-range) addressability contract is preserved (the details_ref still resolves through the new
    tier; the migration loses 0 log bytes).
  - The hierarchical scheduler (the measured-starvation trigger from CI-P30's per-fair_key histogram): promoted
    from flat DRR (CI-P13) ONLY on a measured starvation signal — per-tenant → per-project → per-pipeline DRR.
  - FLOOR named: each is a TRIGGER-GATED promotion; if a trigger has not fired at M5, the floor REMAINS named (do
    not build it) and the trigger status is recorded dated. Cross-cell-spanning pipelines (inheriting the 12.6
    bridge) and SLSA L3+ are deferred-until-demand named floors handled by reference (the multi-cell bridge lifts
    in M5's shared work). State all of this.
- **CONTRACTS TO IMPLEMENT.** Extended where promoted: 11.8 (the time-series log tier — preserving the
  addressability contract). Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - IF the time-series log tier is promoted: the (job, step, byte-range) details_ref still resolves through the
    new tier (0 dangling step anchors); the migration loses 0 log bytes — CI. (Else named-floor; trigger status
    recorded.)
  - IF the hierarchical scheduler is promoted: the per-fair_key starvation histogram improves vs flat DRR under
    the same surge (the measured starvation signal clears) — CI. (Else named-floor; trigger status recorded.)
- **TESTS (required).** For each promotion that fires: the time-series-tier addressability test (the details_ref
  resolves; 0 log bytes lost); the hierarchical-DRR fairness test. For each that does NOT fire: a dated
  gap-report entry recording the trigger status. State the mutation-score floor for any module built.
- **DEFINITION OF DONE.** Each of the two floor follow-ons is EITHER promoted (built + its gate green-and-dated —
  the time-series addressability preserved, the hierarchical fairness improved) OR left a named floor with its
  dated trigger status recorded; where promoted, the tests pass and the contract-coverage scanner is green; the
  cross-cell + SLSA-L3+ deferred floors are named by reference; the work is committed. No gate is greened by
  weakening a threshold.
- **COMMIT.** Header: P-<NNN> M5: CI floor follow-ons — time-series log tier / hierarchical scheduler
  (measured-trigger-gated). Body lists: which promotions fired (with their greened gates) and which remain named
  floors (with dated trigger status); 11.8 extended where the log tier promoted; cross-cell + SLSA-L3+ deferred
  floors named. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---
### CI-P30 — World-scale hardening: the 30x CI surge family (CI-D2) + the tuned DRR/shed-budget numbers + the pre-warm buffer sizing

- **BAND.** M5.
- **ROADMAP MILESTONE.** CI-M5 (planning/06-roadmaps/subsystems/continuous-integration.md §3 "CI-M5 —
  World-scale hardening"), the surge/fairness-under-load slice. Drill: CI-D2 (surge / fairness, the F6 family).
- **DEPENDS-ON.** CI-P13 (the DRR fair-share the surge tunes), CI-P12 (the reaper the surge drives), CI-P4 (the
  fixed pre-warm buffer the surge sizes), CI-P27 (the M4 band is green — deterministic correctness holds before
  world-scale). The index places this FIRST in CI's M5 work (its measurements trigger CI-P28/CI-P29).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale from day 1);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it under load — the 1x/10x/30x generator;
    the protected-human-lane holds, the agent/batch lane sheds).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §2.2 (the DRR floor → hierarchical promotion, the per-fair_key starvation histogram — open question 07#1) +
    §2.4 (backpressure & abuse — the 30x surge sheds batch/CI, holds interactive, others unaffected) + §5.4 (the
    pre-warm buffer sizing function — open question 07#2); 07-drills-and-open-questions.md §1 row D-2 + §2 (the CI
    build open questions 1, 2, 5 — the shed-budget concrete numbers).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-K (the
    per-surface CI-surge shed budget).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.11 (the shed order + the CI
    per-surface budget), 1.8 (the telemetry survival signals).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row CI-D2 (30x CI surge one tenant → interactive holds, batch sheds 429+Retry-After, others unaffected,
    reserve refuses over-budget, killed-runner jobs re-queue within lease TTL 0 orphans).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 (world-scale hardening) + exit
    gate row CI-D2.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate + the drill harness:
  - Tune the surge controls against measured 30x load (open questions 07#1, 07#5): the DRR weights + the
    replenishment cadence + the per-fair_key starvation-histogram threshold; the per-surface shed budget concrete
    numbers (bounded run-queue per tenant, runners pull-bounded; the shed order speculative → batch/CI → agent →
    human-last, 1.11). Write the tuned numbers into the versioned thresholds file (and surface the measured
    starvation signal that CI-P29's hierarchical-scheduler promotion is gated on).
  - Size the pre-warm buffer function (open question 07#2): warm-pool size vs arrival rate vs the per-VM memory
    floor, measured per (region, label-class). Write it into the autoscaler (replacing CI-P4's fixed-buffer
    floor).
  - FLOOR named: flat DRR → the hierarchical scheduler is promoted at CI-P29 only if the measured starvation
    signal fires (otherwise it stays the named floor — measured-not-predicted). State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 1.11 the shed budget, 1.8 the telemetry. Implement to the frozen shapes;
  escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D2 (surge / fairness, the F6 family): a 30x CI surge on one tenant → the interactive lane HOLDS its
    latency budget; the batch/CI lane SHEDS (429 + Retry-After honoured by myelin ci); OTHER tenants unaffected;
    reserve/settle refuses over-budget; a killed runner's jobs re-queue WITHIN the lease TTL, 0 orphans — SCHED.
- **TESTS (required).** The CI-D2 drill scenario on the failure-injection harness (the 1x/10x/30x generator with
  mixed principal kinds). Unit tests for the tuned DRR weights + the pre-warm sizing function. State the
  cargo-mutants mutation-score floor for the shed-budget module (mandatory-core — the surge survival path).
- **DEFINITION OF DONE.** The tuned surge controls + the pre-warm sizing exist and compile; CI-D2 (lanes
  hold/shed, others unaffected, 0 orphans) emits its dated green artifact; the measured DRR/pre-warm thresholds
  are written into the versioned thresholds file; the unit + drill tests pass; the contract-coverage scanner is
  green; the hierarchical-scheduler floor (CI-P29, measured-triggered) is named; the work is committed. No gate
  is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M5: CI world-scale surge (CI-D2) + tuned DRR/shed-budget + pre-warm sizing. Body
  lists: 1.11/1.8 consumed; CI-D2 (interactive holds, batch sheds, others unaffected, 0 orphans) greened; the
  measured DRR/pre-warm thresholds written; the hierarchical-scheduler floor (CI-P29) named. Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CI-P31 — World-scale hardening: residency at cell scale (CI-R3) + the self-hosted runner trust boundary (CI-D10)

- **BAND.** M5.
- **ROADMAP MILESTONE.** CI-M5 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5), the
  residency-at-scale + self-hosted-boundary slice. Drills: CI-R3 (residency at scale), CI-D10 (self-hosted trust
  boundary).
- **DEPENDS-ON.** CI-P14 (the fleet — the residency attestation + the no-global-pool structural property), CI-P4
  (the self-hosted attestation + scoped token), CI-P22 (the log/artifact/cache region). CI-P27 (the M4 band is
  green). The M1 Tenancy prompt with residency_verify (12.4). The index places this in CI's M5 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign — run entirely on EU-controlled infrastructure; residency at scale);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — residency + the self-hosted boundary
    are quantified drills), §2 (the leak families).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/00-overview.md §5 (cell
    topology — no global runner pool; an EU-resident tenant's job claimed only by an in-region runner);
    05-hard-problems.md HP-2 (runner-fleet elasticity on EU infra); 07-drills-and-open-questions.md §1 rows R-3,
    D-10.
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §10 (the CI
    no-global-pool residency attestation).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 12.4 (residency_verify — at cell
    scale), 4.7 (the self-hosted scoped token), 1.6 (the residency-pin lint), 1.8 (the telemetry).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows CI-R3 (in-region runner only; logs/artifacts/caches never leave region; residency_verify attests), CI-D10
    (compromised self-hosted runner → scoped token bounds it; 0 cross-tenant job/secret reads).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 + exit gate rows CI-R3/CI-D10.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate + the drill harness:
  - Residency at cell scale: residency_verify attests the runner pool + log/artifact/cache region; the CDN edge is
    within-EU only; the residency-pin lint passes on every CI write. (The structural no-global-pool property is
    CI-P14; here the cell-scale attestation under drill.)
  - The self-hosted trust-boundary hardening: a compromised self-hosted runner's scoped job token bounds it to its
    own tenant's SelfHosted-tier jobs only (0 cross-tenant job/secret reads); attestation failure → cannot claim.
    (The attestation gate is CI-P4; here the compromised-runner boundary under drill.)
  - FLOOR named: cross-cell-spanning runs (inheriting the 12.6 bridge) are a deferred-until-demand named floor
    handled at CI-P29 by reference. State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 12.4 residency_verify, 4.7 the self-hosted token, 1.6 residency-pin, 1.8
  the telemetry. Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-R3 (residency at scale): an EU-resident tenant's run → claimed ONLY by an in-region runner;
    logs/artifacts/caches NEVER leave the region (CDN within-EU only); residency_verify attests; the residency-pin
    lint passes on every CI write — SCHED.
  - CI-D10 (self-hosted trust boundary): a compromised self-hosted runner → the scoped job token bounds it to its
    own tenant's SelfHosted jobs; 0 cross-tenant job/secret reads; attestation failure → cannot claim — SCHED.
- **TESTS (required).** The CI-R3 + CI-D10 drill scenarios on the failure-injection harness. Unit tests for the
  self-hosted token scoping (bounded to one tenant's SelfHosted jobs). State the cargo-mutants mutation-score
  floor for the self-hosted-token-scoping module (mandatory-core — the cross-tenant isolation boundary).
- **DEFINITION OF DONE.** The cell-scale residency attestation + the self-hosted boundary hardening exist and
  compile; CI-R3 (in-region only) and CI-D10 (0 cross-tenant reads) each emit their dated green artifact; the
  residency-pin lint is green; the unit + drill tests pass; the contract-coverage scanner is green; the
  cross-cell floor is named by reference; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M5: CI residency at scale (CI-R3) + self-hosted trust boundary (CI-D10). Body
  lists: 12.4/4.7/1.6 consumed; CI-R3 (in-region only) + CI-D10 (0 cross-tenant reads) greened; the cross-cell
  floor named by reference. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### CI-P32 — World-scale hardening: the PersonalDataHolder crypto-shred erase path — erasure-reaches-every-holder (CI-D3)

- **BAND.** M5.
- **ROADMAP MILESTONE.** CI-M5 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5), the erasure
  fan-out slice (the crypto-shred path that fills CI-P9's stub). Drill: CI-D3 (erasure fan-out).
- **DEPENDS-ON.** CI-P9 (the PersonalDataHolder erase stub this fills in), CI-P22 (the per-subject DEK the
  crypto-shred destroys), CI-P20 (the log stores erased), CI-P26 (the ci.*.erased tombstones degrade unfurls).
  CI-P27 (the M4 band is green). The M1 GDPR prompt with the DSR fan-out (10.4) + per-subject DEK (11.4). The
  index places this in CI's M5 work.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe — erasure reaches every holder incl. backups);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — 0 recoverable PII incl. backups is
    quantified), §2 (the silent-data-loss / leak families).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md
    §6 (PersonalDataHolder — the crypto-shred erasure path, per-subject DEK where isolable);
    07-drills-and-open-questions.md §1 row D-3.
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-7 (the one erasure
    posture — the residual by reference), §OQ-D (the tombstone ladder degrades broken anchors).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 10.1/10.4 (PersonalDataHolder
    erase + the DSR fan-out), 11.4 (per-subject DEK crypto-shred), 10.9 (the residual by reference).
  - Drills: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row CI-D3 (erase(subject) fans to CI → PII in logs/artifacts/caches/run-state destroyed incl. backups;
    structure survives; 0 dangling leak).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 + exit gate row CI-D3.
- **DELIVERABLE (what to build + exactly where in the repo).** In the CI Control Plane crate (filling CI-P9's
  holder stub):
  - Fill in the PersonalDataHolder erase path (the stub from CI-P9): erase(subject) crypto-shreds the subject's
    PII across logs/artifacts/caches/run-state — per-subject DEK destroyed where isolable (11.4), per-tenant DEK
    fallback — INCLUDING backups; run STRUCTURE survives for audit; ci.*.erased tombstones degrade every unfurl
    to a tombstone via the OQ-D ladder (0 dangling leak). The residual third-party span is by reference to the one
    posture (X-7).
  - FLOOR named: the structural crypto-shred ships here; the residual third-party free-text PII basis (per the one
    platform posture, X-7) is a parallel legal-ratification floor handled by reference. State this.
- **CONTRACTS TO IMPLEMENT.** Consumed: 10.1/10.4 the erase fan-out, 11.4 per-subject DEK crypto-shred, 10.9 the
  residual by reference. Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CI-D3 (erasure-reaches-every-holder): erase(subject) fans to CI → PII in logs/artifacts/caches/run-state
    DESTROYED (per-subject DEK where isolable, per-tenant fallback) INCL. backups; structure survives for audit; 0
    dangling leak in any unfurl/embed — SCHED.
- **TESTS (required).** The CI-D3 drill scenario on the failure-injection harness. Unit tests for the erase
  crypto-shred (per-subject DEK destroyed → ciphertext unrecoverable incl. backups; structure rows survive). State
  the cargo-mutants mutation-score floor for the erase path (mandatory-core — the GDPR-load-bearing crypto-shred).
- **DEFINITION OF DONE.** The erase crypto-shred path exists and compiles (filling CI-P9's stub); CI-D3 (0
  recoverable PII incl. backups, structure survives, 0 dangling leak) emits its dated green artifact; the unit +
  drill tests pass; the contract-coverage scanner is green; the residual-third-party floor is named by reference;
  the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M5: CI PersonalDataHolder crypto-shred erase — erasure-reaches-every-holder (CI-D3).
  Body lists: 10.1/10.4/11.4 consumed; CI-D3 (0 recoverable PII incl. backups, structure survives) greened; the
  residual-third-party floor named by reference. Branch first if on default; do not push unless asked. End with
  the workspace Co-Authored-By trailer.

---
### CI-P33 — CI's slices of the whole-system E2E wedge: E2E-1 (PR context pane) + E2E-3 (spec-to-ship traceability)

- **BAND.** M5.
- **ROADMAP MILESTONE.** CI-M5 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 — "CI's slices
  of the whole-system E2E wedge"), the surfacing-and-traceability E2E slices. Drills: E2E-1, E2E-3 (CI's slices).
- **DEPENDS-ON.** CI-P18 (the ci.check.updated the context pane shows), CI-P21 (the #step-<n> jump-to-failure
  anchor), CI-P25/CI-P26 (project/IndexSpec the surfaces resolve through), CI-P24 (the HITL-gated deploy E2E-3
  ships), CI-P26 (replay/*.snapshot for cold-reindex). CI-P31 + CI-P32 green. The joint E2E scenarios depend on
  ALL subsystems' M4/M5 work. The index places this in CI's M5 work; the E2E scenarios are joint with every
  subsystem.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (the differentiator — one identity/permission/event/agent fabric so work flows between
    tools); ../../external-insights/01-process-and-quality-doctrine.md §4 (chain mutations end-to-end — the E2E
    wedge chains operations; drive the real thing).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md
    §7.2 (project — the context pane / unfurl); 04-views-cli-and-api.md §1 (the cross-subsystem surfaces CI
    feeds).
  - Testing strategy: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §1.E2E-1 (the PR context pane — every connected artifact resolves per-viewer, 0 leak, live check-update within
    the freshness budget), §1.E2E-3 (spec-to-ship traceability — complete lineage per-viewer, cold-reindex ==
    live, audit tamper detected) + §3.4 (the green-artifact format).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the check seam
    end-to-end).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.9 (the check seam, end-to-end in
    E2E-1), 5.6 (project — the pane/unfurl), 2.6 (replay — cold-reindex == live for E2E-3).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 (CI's E2E slices) + exit gate rows
    E2E-1/E2E-3.
- **DELIVERABLE (what to build + exactly where in the repo).** In a cross-subsystem E2E test crate (CI's slices),
  against a full cell with mock agents (--use-mock; mock agents only during development per VISION §3):
  - E2E-1 (PR context pane): CI emits ci.check.updated (build → success, test → failure); the context pane shows
    the live check rows + the jump-to-failure #step-<n> anchor (resolving through the CI-P21 log index); 0 leak to
    an unauthorized viewer; the live check-update lands within the freshness budget.
  - E2E-3 (spec-to-ship traceability): CI runs attach CheckStatus; a protected-env deploy (HITL-gated, CI-P24)
    ships it; cold-reindex (replay / *.snapshot, CI-P26) == live; audit tamper detected. (CI's slice: the check
    attach + the deploy + the cold-reindex-equals-live property.)
  - FLOOR named: these are CI's SLICES of joint scenarios — the full E2E green requires every subsystem's slice;
    CI's slice must green. The E2E-2 flagship is CI-P34; E2E-4 (DSAR fan-out) is covered for CI by CI-P32's CI-D3
    (erasure fan-out), not duplicated here. State this.
- **CONTRACTS TO IMPLEMENT.** Exercised end-to-end (no new contract): 5.9 the check seam, 5.6 project, 2.6 replay.
  Implement to the frozen shapes; escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-1 (CI's slice): the context pane resolves CI's check rows per-viewer with 0 leak to the unauthorized
    viewer; the live check-update lands within the freshness budget; the #step-<n> anchor resolves — SCHED, CI's
    pane-resolution trace + zero-leak = 0.
  - E2E-3 (CI's slice): CheckStatus attaches; the HITL-gated deploy ships; cold-reindex (replay) == live (0
    drift); audit tamper detected — SCHED, lineage diff (live vs cold) at 0 drift.
- **TESTS (required).** The E2E-1 + E2E-3 chained-mutation scenarios (CI's slices) on the full-cell E2E harness
  with mock agents — each CHAINS operations end-to-end (push → check → deploy → cold-reindex), not single-handler
  tests (EI-01 §4). Each emits its named green artifact. State that the joint scenarios are co-owned with every
  subsystem; CI's slice is the deliverable here.
- **DEFINITION OF DONE.** CI's slices of E2E-1 and E2E-3 exist and run against a full cell with mock agents; each
  emits its dated green artifact (E2E-1 0 leak + freshness; E2E-3 cold-reindex == live + tamper detected); the
  chained-mutation tests pass; the contract-coverage scanner is green; the E2E-2 (CI-P34) + E2E-4-via-CI-D3 notes
  are written; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M5: CI E2E slices — E2E-1 PR context pane + E2E-3 traceability. Body lists:
  5.9/5.6/2.6 exercised; E2E-1 (0 leak) + E2E-3 (cold-reindex == live) greened; the E2E-2 (CI-P34) +
  E2E-4-via-CI-D3 notes. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### CI-P34 — CI's slice of the whole-system E2E wedge: E2E-2 the agent-native flagship (CI-fail → triage agent → issue → chat → fix-PR)

- **BAND.** M5.
- **ROADMAP MILESTONE.** CI-M5 (planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 — "CI's slices
  of the whole-system E2E wedge"), the agent-native flagship slice. Drill: E2E-2 (CI's slice).
- **DEPENDS-ON.** CI-P26 (the structured ci.run.failed payload the triage agent reads + the ToolDefs), CI-P5 (the
  AG-D4-gated runner the triage agent's compute runs on), CI-P19 (the ci.result merge wake), CI-P17 (reserve/
  settle balanced). CI-P33 (E2E-1/E2E-3 green). The joint scenario depends on ALL subsystems' M4/M5 work (Git,
  Issues, Chat, Knowledge, Refs, Search, Identity, Notif, Agent, Workflow). The index places this in CI's M5
  work; the E2E-2 flagship is joint with every subsystem.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (the differentiator — agents are first-class; work flows between tools);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (chain mutations end-to-end — the bugs live
    mid-flight; drive the real thing).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md
    §4 (the structured ci.run.failed payload — the deliberate triage hook the E2E-2 flagship reads);
    03-events-contracts-and-glue.md §1.2 (ci.run.failed carries structured failure); 04-views-cli-and-api.md §1
    (the agent-surfaced triage).
  - Testing strategy: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §1.E2E-2 (CI-fail → triage agent → issue → chat → fix-PR — the flagship: 0 effect outside the ∩, 0 mutation
    before approval, exactly-once approval + merge across a kill, reserve/settle balanced, merge-count == 1) +
    §2.5 (the agent-native cross-cutting assertions).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-6 (the agent
    compute runs on CI's AG-D4-gated runner).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 5.9 (the check seam, end-to-end in
    E2E-2), 9.4 (the ci.result merge wake), 11.7 (reserve/settle balanced), 8.4 (the AG-D4-gated runner).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M5 (the E2E-2 flagship) + exit gate
    row E2E-2.
- **DELIVERABLE (what to build + exactly where in the repo).** In the cross-subsystem E2E test crate (CI's
  E2E-2 slice), against a full cell with mock agents (--use-mock):
  - E2E-2 (CI-fail → triage agent → issue → chat → fix-PR, the agent-native FLAGSHIP): CI's ci.run.failed carries
    STRUCTURED failure (which step, which test, log excerpt — the deliberate triage hook, arch 02 §4); the triage
    agent's compute runs on CI's AG-D4-gated runner; the fix-PR's CI goes green; the merge-queue wakes on
    ci.result (idempotent on idem_token); reserve/settle balanced; merge-count == 1. (CI's slice: the structured
    failure hook + the runner + the check seam + the merge wake + the reserve/settle balance.)
  - FLOOR named: this is CI's SLICE of the joint flagship — the full E2E-2 green requires every subsystem's slice;
    CI's slice must green and the joint orchestration is the cross-subsystem M5 wedge. State this.
- **CONTRACTS TO IMPLEMENT.** Exercised end-to-end (no new contract): 5.9 the check seam, 9.4 the ci.result merge
  wake, 11.7 reserve/settle, 8.4 the AG-D4-gated runner. Implement to the frozen shapes; escalate a needed change,
  do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-2 (CI's slice, the flagship): the structured ci.run.failed feeds the triage agent; the agent compute runs
    AG-D4-gated; the fix-PR CI greens; the merge-queue wakes EXACTLY ONCE on ci.result; reserve/settle balanced;
    merge-count == 1 — SCHED, deterministic run trace + reserve/settle parity + merge-count == 1.
- **TESTS (required).** The E2E-2 chained-mutation scenario (CI's slice) on the full-cell E2E harness with mock
  agents — it CHAINS operations end-to-end (push → check → fail → triage → fix → merge), not single-handler tests
  (EI-01 §4). It emits its named green artifact. State that the joint scenario is co-owned with every subsystem;
  CI's slice is the deliverable here.
- **DEFINITION OF DONE.** CI's slice of E2E-2 exists and runs against a full cell with mock agents; it emits its
  dated green artifact (deterministic trace + merge-count == 1 + reserve/settle parity); the chained-mutation test
  passes; the contract-coverage scanner is green; the joint-orchestration note is written; the work is committed.
  No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M5: CI E2E-2 flagship — CI-fail → triage agent → issue → chat → fix-PR. Body lists:
  5.9/9.4/11.7/8.4 exercised; E2E-2 (merge-count == 1, reserve/settle parity, AG-D4-gated triage compute) greened;
  the joint-orchestration note. Branch first if on default; do not push unless asked. End with the workspace
  Co-Authored-By trailer.

---

### CI-P35 — Dogfooding: Myelin's own build/test/lint/mutation pipeline runs as a Myelin CI pipeline + the switch test

- **BAND.** M6.
- **ROADMAP MILESTONE.** CI-M6 (planning/06-roadmaps/subsystems/continuous-integration.md §3 "CI-M6 —
  Dogfooding: Myelin's own CI runs on Myelin CI"). The cheapest, most honest load generator is the platform's own
  development. The done-bar.
- **DEPENDS-ON.** CI-P34 + CI-P32 + CI-P27 green (CI is world-scale-ready; the E2E wedge is proven; restore-verify
  + DSAR fan-out green BEFORE real team data — the team's build data is real tenant data). The M6 Git dogfood
  prompt (the Myelin monorepo migrated onto Myelin git hosting). The other subsystems' M6 dogfood work (Issues,
  Knowledge, Chat self-hosting). The index places this LAST in CI's work — the platform done-bar.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (the differentiator driven by the builders themselves); §3 (the code wins over the docs —
    a dated green self-hosting graph, not a claim);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (the switch test — driven in a browser/CLI, not
    read from a feature list; actually try the real thing), §5 (the ratchet now self-hosted — the lints + the
    mutation gate run as Myelin CI jobs on every Myelin commit).
  - Architecture: ../04-subsystem-architectures/continuous-integration/architecture/04-views-cli-and-api.md §2
    (the myelin ci CLI + the run/log/deploy views the switch test drives); 00-overview.md §1 (CI's role — the
    twelve lints + the mutation gate as Myelin CI jobs).
  - Master sequencing: ../06-roadmaps/00-master-sequencing.md §2 M6 (the dogfooding band — Myelin hosts itself;
    the self-hosting CI graph green; the switch tests driven in a browser).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.6 (the twelve lints now run as
    Myelin CI jobs), 9.2 (the ci.pipeline workflow hosting the Myelin build), 5.9 (the check seam on Myelin's own
    commits).
  - Roadmap: planning/06-roadmaps/subsystems/continuous-integration.md §3 CI-M6 (the work + the exit gate — the
    self-hosting CI graph green + the CI switch test) + §1 (the dogfooded stage).
- **DELIVERABLE (what to build + exactly where in the repo).** In the Myelin repo's CI configuration (the
  self-hosting pipeline def) + a switch-test record:
  - Migrate the Myelin build/test/lint/mutation pipeline onto a Myelin ci.pipeline (the ci.pipeline workflow from
    CI-P15 running on the CI-P12/CI-P13 scheduler + the CI-P14 fleet, gated by the CI-P5 escape gate). The twelve
    architecture lints (1.6) + the mandatory-core cargo-mutants mutation gate now run AS MYELIN CI JOBS on every
    Myelin commit — the ratchet is now self-hosted, reading the one-prompt-one-commit ledger order.
  - The every-incident-adds-a-drill loop: an incident files a Myelin issue + a reproducing CI drill (the loop is
    now self-hosted on the platform).
  - Drive the real myelin ci CLI + the run/log/deploy views for the switch test (the Git OQ-12 / CI switch test):
    could a GitHub-Actions / GitLab-CI user move to Myelin without hitting a wall the old tool didn't have? —
    reached by DRIVING it (in a browser/CLI), not by reading the feature list; measured latency + the run/log UX
    against the GitHub Actions anchor.
  - FLOOR named: this is the done-bar (no follow-on). State that any remaining CI named floors (myelin ci local,
    the registry product, cross-cell-spanning pipelines until OQ-I demand) stay deferred-by-design named floors,
    recorded in the gap report.
- **CONTRACTS TO IMPLEMENT.** Exercised in production (no new contract): 1.6 the lints as Myelin CI jobs, 9.2 the
  ci.pipeline hosting the Myelin build, 5.9 the check seam on Myelin's commits. Implement to the frozen shapes;
  escalate a needed change, do not diverge.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Myelin self-hosting CI graph is GREEN on the platform's own commits (the build/test/lint/mutation
    pipeline passes as a Myelin CI pipeline; the twelve lints + the mutation gate run on every Myelin commit) —
    the dogfood loop is live — SCHED.
  - The CI switch test PASSES (driven in a browser/CLI; measured latency + the run/log UX against the GitHub
    Actions anchor — a GitHub-Actions user could move without hitting a wall the old tool didn't have) — SCHED.
  - No later-band CI gate is red (the truth-up pass confirms every PROVEN CI row rests on a dated green artifact,
    not a doc claim — code-wins-over-docs) — CI.
- **TESTS (required).** The self-hosting CI graph IS the test (the platform's own build/test/lint/mutation run as
  a Myelin CI pipeline, green on every commit). The switch test is driven in a browser/CLI (actually try the real
  thing — EI-01 §4) and its verdict recorded dated. The truth-up pass over every CI PROVEN row (each rests on a
  dated green artifact). No new unit logic; the dogfood graph is the proof.
- **DEFINITION OF DONE.** The Myelin build/test/lint/mutation pipeline runs as a Myelin ci.pipeline, green on the
  platform's own commits (the twelve lints + the mutation gate self-hosted); the CI switch test passes (driven,
  measured, recorded dated); the every-incident-adds-a-drill loop is self-hosted; the truth-up pass confirms 0 red
  earlier CI gates (every PROVEN row dated-green); any remaining named floors are recorded in the gap report; the
  work is committed. No gate is greened by weakening a threshold; the switch-test verdict is reached by driving the
  real thing, not by reading a feature list.
- **COMMIT.** Header: P-<NNN> M6: dogfood — Myelin CI runs on Myelin CI + the switch test. Body lists: 1.6/9.2/5.9
  exercised on the platform's own commits; the self-hosting CI graph green (lints + mutation gate self-hosted);
  the switch test passed (driven, measured); the truth-up pass (0 red earlier CI gates); remaining named floors
  recorded. Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---
## Coverage matrix (every CI roadmap milestone → its finer prompt(s); first-pass → finer split)

| CI roadmap milestone (planning/06-roadmaps/subsystems/continuous-integration.md §3) | First-pass prompt | Finer prompt(s) | Band |
|---|---|---|---|
| CI-M2 — the unified sandbox runner + the escape GATE (SandboxBackend + Firecracker + hardening + JobSpec) | CI-P1 (first pass) | CI-P1 (JobSpec + trait seam + four-guarantee hooks), CI-P2 (Firecracker backend + hardening profile + hardened-boot self-test) | M2 |
| CI-M2 — the runner agent + the escape-drill adversarial corpus + AG-D4 / CI-T1 (the hard GATE) | CI-P2 (first pass) | CI-P3 (runner agent + lease/heartbeat + exactly-once job.done), CI-P4 (pre-warm pools + self-hosted attestation + scoped token), CI-P5 (escape corpus + green-attestation + AG-D4/CI-T1 GATE) | M2 |
| CI-M4 — service shells + data model + ci.* taxonomy + the CI ReBAC fragment + PersonalDataHolder | CI-P3 (first pass) | CI-P6 (five service shells + complete migrations), CI-P7 (ci.* event taxonomy), CI-P8 (CI ReBAC fragment), CI-P9 (PersonalDataHolder) | M4 |
| CI-M4 — Trigger & Dispatch (EventMatcher/dedup/trust-tier stamp/CAS snapshot) | CI-P4 (first pass) | CI-P10 (EventMatcher + dedup + trust-tier stamp), CI-P11 (definition resolution + CAS snapshot + reserve/start handoff) | M4 |
| CI-M4 — green-field core: the distributed scheduler (pull-lease/DRR/lanes/concurrency/affinity/reaper) | CI-P5 (first pass) | CI-P12 (pull-lease claim + concurrency + affinity + reaper), CI-P13 (DRR fair-share + priority lanes + backpressure) | M4 |
| CI-M4 — green-field core: the EU fleet autoscaler (FleetProvider/per-residency-zone/self-hosted attestation) | CI-P6 (first pass) | CI-P14 (FleetProvider + autoscale + per-residency-zone pools + fleet events); self-hosted attestation in CI-P4 | M4 |
| CI-M4 — the ci.pipeline durable workflow + SCHEDULE_AND_RUN_JOB + reserve/settle metering + determinism (CI-D1/D5/D9) | CI-P7 (first pass) | CI-P15 (ci.pipeline body + determinism CI-D9), CI-P16 (SCHEDULE_AND_RUN_JOB + crash-recovery CI-D1), CI-P17 (reserve/settle metering + cost_event CI-D5) | M4 |
| CI-M4 — the X-1 CheckStatus producer half + ci.result (closing the seam Git built in M3) (GIT-D10/CI-D8) | CI-P8 (first pass) | CI-P18 (check_attempt counter + ci.check.updated producer), CI-P19 (ci.result rollup + GIT-D10/CI-D8 seam GATE) | M4 |
| CI-M4 — logs/firehose/resume-cursor + T3 log tier + trust-scoped artifacts & caches (CI-D6/CI-D11) | CI-P9 (first pass) | CI-P20 (firehose + sealed T3 index + ci.log.available), CI-P21 (resume-cursor live-tail + details_ref CI-D11), CI-P22 (trust-scoped artifacts/caches + per-subject DEK CI-D6) | M4 |
| CI-M4 — supply-chain (digest-pin/sigstore/SLSA/SBOM) + secret broker + deployments & HITL (CI-D4/CI-D7) | CI-P10 (first pass) | CI-P23 (supply-chain fail-closed CI-D4), CI-P24 (in-boundary secret broker CI-D7 + deploy HITL) | M4 |
| CI-M4 — cross-fabric surfacing (project/ArtifactRef/#sub/declare_indexable/humanise/replay) + list_objects SetExpr + ToolDefs | CI-P11 (first pass) | CI-P25 (list_objects push-down + ArtifactRef/#sub + project), CI-P26 (declare_indexable + humanise + replay + ToolDefs) | M4 |
| CI-M4 — exit gate: AG-D4/CI-T1 re-confirmed on the prod runner image (+ STOR-D1/D2 permanent gate) | CI-P12 (first pass) | CI-P27 (re-confirm AG-D4/CI-T1 on prod image + STOR-D1/D2 restore-verify) | M4 |
| CI-M5 — world-scale hardening: CI-D2 surge + CI-R3 residency + CI-D10 self-hosted + CI-D3 erasure | CI-P13 (first pass) | CI-P30 (30x surge CI-D2 + tuned DRR/shed-budget + pre-warm sizing), CI-P31 (residency CI-R3 + self-hosted CI-D10), CI-P32 (crypto-shred erase CI-D3) | M5 |
| CI-M5 — the floor follow-ons (gVisor 2nd backend / time-series log tier / hierarchical scheduler) | CI-P14 (first pass) | CI-P28 (gVisor backend + escape-gate re-green), CI-P29 (time-series log tier + hierarchical scheduler) | M5 |
| CI-M5 — CI's slices of the whole-system E2E wedge (E2E-1/E2E-2/E2E-3) | CI-P15 (first pass) | CI-P33 (E2E-1 PR context pane + E2E-3 traceability), CI-P34 (E2E-2 agent-native flagship) | M5 |
| CI-M6 — dogfooding (Myelin CI runs on Myelin CI + the switch test) | CI-P16 (first pass) | CI-P35 (dogfood + switch test) | M6 |

**First-pass → finer split summary (16 → 35):** CI-P1→{CI-P1,CI-P2}; CI-P2→{CI-P3,CI-P4,CI-P5}; CI-P3→{CI-P6,
CI-P7,CI-P8,CI-P9}; CI-P4→{CI-P10,CI-P11}; CI-P5→{CI-P12,CI-P13}; CI-P6→{CI-P14} (+ self-hosted attestation
hoisted to CI-P4); CI-P7→{CI-P15,CI-P16,CI-P17}; CI-P8→{CI-P18,CI-P19}; CI-P9→{CI-P20,CI-P21,CI-P22};
CI-P10→{CI-P23,CI-P24}; CI-P11→{CI-P25,CI-P26}; CI-P12→{CI-P27}; CI-P13→{CI-P30,CI-P31,CI-P32};
CI-P14→{CI-P28,CI-P29}; CI-P15→{CI-P33,CI-P34}; CI-P16→{CI-P35}. No first-pass deliverable, contract, drill, or
floor is dropped — each finer prompt carries a strict subset of its parent's coverage and the union is identical
(plus the bundled sub-deliverables are now their own gateable prompts).

**Every drill mapped to its prompt:** CI-T1/AG-D4 → CI-P5 (shipped) + CI-P27 (M4 re-confirm) + CI-P28 (gVisor
re-green); CI-D1 → CI-P16; CI-D2 → CI-P30; CI-D3 → CI-P32; CI-D4 → CI-P23; CI-D5 → CI-P17; CI-D6 → CI-P22;
CI-D7 → CI-P24; CI-D8/GIT-D10 → CI-P19; CI-D9 → CI-P15; CI-D10 → CI-P31; CI-D11 → CI-P21; CI-R3 → CI-P31;
STOR-D1/STOR-D2 → CI-P27; E2E-1 → CI-P33; E2E-2 → CI-P34; E2E-3 → CI-P33; E2E-4 (DSAR) → covered for CI by CI-P32
(CI-D3). The hardened-boot self-test → CI-P2; the exactly-once terminal report → CI-P3; the fairness property →
CI-P13; the reaper recovery → CI-P12.

**Floor → follow-on pairs (name-your-floors, both prompts present):** Firecracker-only backend (CI-P1/CI-P2) →
gVisor second backend re-greening the escape gate (CI-P28). Flat DRR fair-share (CI-P13) → hierarchical scheduler
(CI-P29, measured-starvation-triggered; the measurement is CI-P30). Fixed pre-warm buffer (CI-P4) → measured
buffer-sizing function (CI-P30). Object-segment T3 log tier (CI-P20) → time-series/wide-column log tier (CI-P29,
measured-volume-triggered). One/two FleetProvider adapters (CI-P14) → more EU-provider adapters (demand). Stubbed
PersonalDataHolder erase (CI-P9) → the crypto-shred erase path (CI-P32, CI-D3). Per-subject DEK crypto-shred
substrate (CI-P22) → the full erase fan-out (CI-P32). SLSA L1-L2 (CI-P23) → hermetic/L3+ (demand). The X-1
producer floor — external-provider checks (CI-P18) → richer external-CI integrations (demand). Single-cell
pipelines (CI-M4) → cross-cell-spanning runs (CI-P29 by reference, OQ-I bridge). The two permanent gates
(AG-D4/CI-T1, STOR-D1/D2): shipped CI-P5, re-confirmed CI-P27, re-run on the gVisor backend CI-P28.
