# Sketch 01 — Isolation model (TE-28): the sandbox backend behind the job spec

> Phase 4 — CI subsystem, exploration. Decides the sandbox **backend** for the one unified runner
> (ADR-20 / CI-1) and the **threat model**. This is the single most security-load-bearing decision in
> the whole platform: **one escape is a cross-tenant catastrophe** (EI-03 §3; EI-04 §5.1), and a
> property **not drilled on a real kernel is a claim, not a fact** (EI-03 §3.5 — the AG-D4 escape drill
> is the single hard go/no-go before any customer code, CI *or* agent, runs).
>
> Binding inputs: CI-1 (isolation floor = gVisor-class userspace-kernel **or** microVM; plain
> shared-kernel containers rejected for untrusted code; the named hardening profile), ADR-20 (one job
> spec, `kind ∈ {ci, agent}`), the Phase-3 contract that the backend is "swappable behind a
> runtime-agnostic job spec" (EI-03 §3; decision-record D5).

---

## The decision to make

CI-1/ADR-20 already commit the *class* (userspace-kernel **or** microVM) and the hardening profile.
What is open is the **default backend** and whether one backend or two ship. The runtime-agnostic job
spec is non-negotiable either way, so this is a default-and-sequencing call, not an architecture
foreclosure.

## Candidate A — gVisor (userspace kernel) as the default

**What it is.** gVisor (Young et al., *The True Cost of Containing*, 2019; the runsc OCI runtime)
interposes a Go-implemented application kernel (the "Sentry") between the guest and the host kernel,
intercepting syscalls in userspace so the host kernel surface the guest can reach is drastically
narrowed. It runs as an OCI runtime — `runsc` is close to a drop-in for `runc`.

- **Pros.** Fast start (container-class, tens of ms — strong for "time to first log line", CI-DD §5.8);
  high density (no per-job VM memory floor); OCI-native so image handling, layered FS, and the
  digest-pinning we need are off-the-shelf; the syscall-interposition model is exactly the
  defence-in-depth the threat model wants (the guest never issues a raw host syscall).
- **Cons.** The Sentry is still a **shared userspace process on a shared host kernel** — the trust
  boundary is the Sentry + the narrow host-syscall set it makes, not hardware. Historically gVisor has
  had its own CVEs (the trust surface moved, it didn't vanish). Some syscalls are unimplemented/slow
  (a perf+compat tax on heavy I/O builds). Nested virtualization / Docker-in-CI is awkward.

## Candidate B — microVM (Firecracker / Cloud Hypervisor) as the default

**What it is.** Firecracker (Agache et al., *Firecracker: Lightweight Virtualization for Serverless*,
NSDI 2020) and Cloud Hypervisor run a guest under **hardware virtualization (KVM)** with a minimal
device model. The boundary is the CPU's VT-x/AMD-V + a tiny VMM, the strongest practical multi-tenant
isolation short of separate hosts.

- **Pros.** **Hardware-enforced** tenant boundary — the conservative default the Phase-1/Phase-2 docs
  already leaned toward (CI-DD §6.1; Phase-2 §3 "microVM-class default for untrusted"). A VMM escape is
  far rarer than a kernel/Sentry escape. Real guest kernel → Docker-in-CI, nested-virt, and arbitrary
  syscall workloads "just work". Firecracker's minimal device model is itself a small, audited surface.
- **Cons.** Higher start latency (~100ms–sub-second cold; pre-warmed pools mitigate but cost RAM); a
  per-VM memory floor caps density; requires KVM (bare-metal or nested-virt-capable hosts — fine on the
  EU bare-metal infra TE-29 points at, a constraint on some cloud instance types); the VMM + guest
  kernel are more to operate.

## Candidate C — both, behind the job spec, sequenced

Ship **the runtime-agnostic `SandboxBackend` trait first** with **one** production backend, and treat
the second as a measured/sensitivity-driven addition — not two from day one.

---

## Leaning: **microVM (Firecracker) is the default for untrusted; gVisor is the named alternative behind the same trait; ship one backend to the escape drill first.**

Reasoning, weighed against the binding docs:

1. **The drill, not the benchmark, governs.** The decision that matters is which backend we can take
   through AG-D4 (zero escapes on a real kernel) with the most confidence. Hardware virtualization is
   the more defensible "zero escapes" claim to a DPO and to the F-family security gate — the boundary
   is the CPU, not a userspace reimplementation of a kernel. For *the* single hard gate, conservatism
   wins. This matches the prior-art lean already recorded (CI-DD §11 TE-28 "leaning microVM"; Phase-2
   §3).
2. **CI's real workloads need a real kernel.** Docker-in-CI / image builds / nested-virt / arbitrary
   syscalls are first-class CI demands (CI-DD §6.1). gVisor's syscall gaps make these a compat fight;
   a microVM guest kernel removes the fight. (Rootless builders — Buildah/Kaniko/BuildKit-rootless —
   are still the *preferred* in-guest path for image builds; the microVM just removes the floor of
   "can't even.")
3. **Start latency is a solved-enough problem.** Pre-warmed microVM pools + snapshot-restore
   (Firecracker resume from a memory snapshot) bring "time to first job" down to the tens-of-ms range
   for warm pools; the cold path is the cost we pay for the boundary, mitigated, not eliminated. The
   density tax is a cost-model input (sketch 03 elasticity), not a safety compromise.
4. **gVisor is not discarded — it is the second backend behind the trait**, valuable for very-high-
   density, low-risk, *trusted*-tier or short agent `compute` calls where the start-latency/density
   economics favour it and the workload doesn't need a full kernel. Because the job spec is
   runtime-agnostic, adding it later is a backend impl + its own escape drill, not a rewrite.

**The hardening profile is backend-independent and mandatory on both** (CI-1; EI-03 §3): no host
network (egress default-deny, allowlist opt-in), read-only root + tmpfs scratch, all caps dropped,
no-new-privileges, seccomp, **images pinned by digest (reject an un-digested tag, fail-closed)**,
whole-guest kill on teardown, cgroup `pids.max` (fork-bomb ceiling) + **zero swap**, and **secrets
resolved by name *inside* the boundary, never baked into images and never handed to the agent runtime
to forward**. The sandbox is **one-job-per-sandbox, ephemeral, never reused across tenants/jobs**.

## The job spec (the runtime-agnostic seam — ADR-20)

```rust
// One spec, two kinds (ADR-20). The SandboxBackend trait hides Firecracker vs gVisor.
pub struct JobSpec {
    pub kind: JobKind,                 // Ci | Agent  (the unify point — TE-31 resolved=UNIFY)
    pub image: ImageRef,               // MUST be digest-pinned; an un-digested tag is rejected (fail-closed)
    pub command: Vec<String>,
    pub env: Vec<EnvVar>,              // secrets are NAMES here; resolved inside the boundary (CI-1)
    pub secret_refs: Vec<SecretRef>,   // resolved by the in-boundary broker, scoped to THIS job only
    pub egress: EgressPolicy,          // default-deny; allowlist opt-in; metadata/control-plane/cross-tenant always blocked
    pub limits: ResourceLimits,        // cpu, mem, disk, pids_max, timeout, zero-swap
    pub workspace: WorkspaceSpec,      // checkout via job-token git wire; read-only root + tmpfs scratch
    pub trust_tier: TrustTier,         // Trusted | Untrusted(fork) | SelfHosted — gates secrets/cache-write/egress
}
pub enum JobKind { Ci, Agent }

pub trait SandboxBackend {             // Firecracker (default) | Gvisor (alt) | SelfHosted (delegated)
    fn launch(&self, spec: &JobSpec, hooks: &RunnerHooks) -> Result<SandboxHandle>;
    fn kill(&self, h: &SandboxHandle) -> Result<()>;     // whole-guest kill on teardown
}
```

`ToolHands::exec` (Agent Fabric, contract 8.4) is realised as `launch(JobSpec{ kind: Agent, .. })` —
**the same runner, the same hardening, the same drill.** That is TE-31 = UNIFY made concrete.

## What the escape drill (AG-D4) must assert (PROVE-IT)

Owned by CI; the single hard gate (drills doc §, T-5). The quantified gate:
**zero escapes** under an adversarial corpus on a **real kernel** — a `compute`/`ci` job attempts:
kernel-exploit primitives, the cloud-metadata endpoint (169.254.169.254 SSRF→cred theft), the
control-plane/internal RPC, another tenant's network/storage, a fork bomb (assert `pids.max` ceiling),
disk fill, and a secret-exfiltration via egress (assert default-deny holds). The drill emits a green
artifact or CI is **no-go for untrusted code** (E-9 / E-1 go/no-go). It runs against the *production*
backend on a *real* host, re-run on every backend/image/kernel change.

## Floors & follow-ons
- **FLOOR:** one backend (Firecracker) to the drill first; gVisor-as-second-backend is the named
  follow-on (measured by density/latency economics, sketch 03), behind the same trait + its own drill.
- **FLOOR:** non-Linux targets (macOS on Apple HW, Windows) are **out of scope v1** (CI-DD §6.1);
  full-VM backend is the seam, not built.
- **Self-hosted runners** (TrustTier::SelfHosted, customer infra) are semi-trusted nodes — attestation
  + scoped job tokens; the isolation is the customer's, our boundary is the control-plane token. Sketch
  03 covers the fleet; the trust/attestation model is sketch 05-supply-chain-adjacent.
