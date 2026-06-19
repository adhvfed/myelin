# CI/CD — 05 Hard Problems (resolutions + cited prior art + floors)

> Phase 4 — CI Stage-2. The consolidated resolution of every CI-specific hard problem, each with **cited
> prior art** and a **named floor** where v1 is partial. The detailed mechanisms live in 02 (algorithms)
> and 03 (contracts); this doc is the decision record. The Stage-1 leanings (`../sketches/`) are confirmed
> here at architecture altitude.

---

## HP-1 — Isolation model (TE-28)

**Resolution.** microVM (**Firecracker**) is the default backend for untrusted code, behind a
runtime-agnostic `SandboxBackend` trait; **gVisor** is the named second backend; **one backend goes
through the escape drill first**. The hardening profile (egress default-deny, read-only root + tmpfs, caps
dropped, no-new-privileges, seccomp, digest-pinned images fail-closed, whole-guest kill, `pids.max` + zero
swap, secrets-in-boundary) is backend-independent and mandatory on both. One-job-per-sandbox, ephemeral,
never reused across tenants. **Why microVM default:** the *drill governs* — hardware virtualization (KVM +
a minimal VMM) is the more defensible "zero escapes" claim for the platform's single hard gate than a
userspace-kernel reimplementation; and CI's real workloads (Docker-in-CI, image builds, nested-virt) need a
real guest kernel. (Detail: 02 §4.)

**Prior art.** Firecracker (Agache et al., *Firecracker: Lightweight Virtualization for Serverless*, NSDI
2020); gVisor (Young et al., *The True Cost of Containing*, 2019; `runsc`); Cloud Hypervisor (alt VMM); the
hardening profile follows the EI-03 §3 agent-sandbox doctrine and CI-1.

**Floor.** One backend (Firecracker) to the drill first; gVisor-second is the named follow-on
(density/latency-economics-triggered, esp. sub-second agent `compute`). Non-Linux targets (macOS/Windows)
out of scope v1. **Drill:** AG-D4 escape drill — *zero escapes* on a real kernel (HP-9 / 07 T-1).

## HP-2 — Runner-fleet elasticity on EU infra (TE-29)

**Resolution.** **Pull-leasing** (lease + heartbeat + reaper, the platform's `FOR UPDATE SKIP LOCKED`
primitive) for assignment; **autoscale-on-queue-depth over EU IaaS/bare-metal** behind a `FleetProvider`
trait (no hyperscaler autoscaling — the divergence-by-constraint, ADR-11); pre-warmed microVM snapshot
pools; scale-to-zero; bin-packing under the microVM memory floor; **no global pool — partitioned per
residency zone** (enforced at claim time by the `region` predicate + the `residency-pin` lint). Self-hosted
runners attest and receive scoped job tokens. (Detail: 02 §5.)

**Prior art.** Buildkite-agent / HashiCorp Nomad pull model; AWS Builders' Library cell/bulkhead (no global
pool); the autoscaler is built (not rented) because ADR-11 declines the hyperscaler primitive. K8s is kept
as a `FleetProvider` *option*, never the default.

**Floor.** One/two `FleetProvider` adapters (the commercial infra pick) + self-hosted v1; more are
adapters. **Drill:** 30× surge → interactive lane holds, batch sheds, other tenants unaffected; kill a
runner mid-lease → reaper re-queues within the lease TTL, zero orphaned jobs (07 D-2).

## HP-3 — Config grammar / config-as-code

**Resolution.** Declarative **JSON-Schema'd core** (authored YAML/TOML, validated against a published
schema) + the **shared bounded query-AST** for expressions (one expression language platform-wide — *not*
CEL/JSONLogic) + a **sandboxed dynamic-generation escape hatch** (a job that emits a pipeline fragment,
running in the same sandbox as any untrusted code — no privileged config-eval path). Deterministic
resolution → a **content-addressed snapshot** per run (reproducibility/audit). Shift-left `validate`/`plan`
(no runner spend) is core, not nice-to-have (runner compute is the cost center). (Detail: 02 §7.4.)

**Prior art.** The shared `EventMatcher` / query-AST (ADR-07; Bus §4.5); Buildkite dynamic pipelines (the
hardened escape-hatch shape); GitHub Actions / GitLab CI (the declarative-YAML baseline whose
"YAML-programming" pain we avoid by reusing the one expression AST + the dynamic escape hatch).

**Floor.** JSON-Schema + bounded-expression core + sandboxed dynamic generation ship v1; richer authoring
ergonomics are additive. **Drill:** un-digested-reference rejection at plan time (HP-4).

## HP-4 — Component / action registry supply-chain (TE-30)

**Resolution.** **Digest-pin-or-fail-closed** for images **and** components (a floating tag is rejected at
plan time — the single highest-leverage supply-chain control; kills tag-mutation attacks). **Sign +
verify-before-use** (sigstore Fulcio keyless + Rekor transparency log; reuse the platform's CT-Merkle
pattern, EU-hosted). **SLSA provenance** (signed: which run, which snapshot, which inputs built an
artifact) + **SBOM** (CycloneDX/SPDX) for produced artifacts — a deliberate EU-sovereign differentiator.
The resolved digests record into the per-run CAS snapshot (a run is reproducible down to which component
bytes it ran). Self-hosted runner **attestation** is the fleet-trust half. (Detail: 02 §1.4/§7.4; 03 §1.3.)

**Prior art.** SLSA (OpenSSF; the successor framing of Google Binary Authorization for Borg); sigstore
(Fulcio + Rekor); Certificate Transparency (RFC 6962 / Trillian — the same Merkle structure GDPR/Audit
already builds); CycloneDX/SPDX SBOMs.

**Floor.** SLSA L1–L2-grade provenance ships; hermetic/two-party (L3+) is demand-triggered. The component
**trust model** (digest-pin + sign + verify + SLSA) is built regardless; the registry *product* (hosting,
discovery) is commercial-flagged. **Drill:** un-digested → fail-closed; tampered/unsigned component →
refused; *zero* un-pinned/unsigned executions (07 D-4).

## HP-5 — CI ↔ agent substrate unification depth (TE-31 = UNIFY)

**Resolution.** ADR-20/D5 resolved TE-31 = UNIFY; CI **does not diverge**. **Shared (one thing):** the job
spec (`JobSpec{ kind ∈ {Ci, Agent} }`), the sandbox runner + hardening profile, the escape drill, the
reserve/settle gate + the one `CostEvent` schema, the secret broker. `ToolHands::exec` (Agent contract 8.4)
**is** `SandboxBackend::launch(JobSpec{ kind: Agent })` on CI's runner. **Distinct (deliberately not
collapsed):** the orchestration workflow (`ci.pipeline` vs `agent_run`), the brain (`AgentRuntime::step` —
CI has none), and the **`EffectApi::apply` governed-mutation path** (an agent's side-effecting tools go
through `EffectApi`, **never** `ToolHands::exec`; collapsing them would be a security regression). The
one-liner: *shared hands + hardening; distinct head + governance.* (Detail: 02 §5 of sketch / 02 §4 here;
03 §7.5.)

**Prior art.** ADR-20 / decision-record D5; Agent §2.2/§5.0 (the routing rule that side-effecting mutation
never touches `exec`); the capability/least-privilege separation (Saltzer & Schroeder 1975).

**Floor.** gVisor is the likely first home for short agent `compute` calls where microVM start-latency
dominates a sub-second tool call (a measured economics decision, not a v1 commitment). **Drill:**
reserve/settle parity — an exhausted wallet refuses to start *both* a CI run and an agent `compute` job
(07 D-5); and the escape drill gates both kinds (HP-9).

## HP-6 — Metering unit (TE-32)

**Resolution.** **Resource-seconds** as the wholesale meter (`cpu_seconds`, `mem_gb_seconds`,
`gpu_seconds`, `storage_gb_hours`, `egress_gb`) — the honest cost basis, bin-packs well, and is **directly
comparable to an agent run's cost**. Commercial maps resource-seconds → credits at the **markup** layer
(kept in a separate column; immutable pricing). One `cost_event` per metered unit; integer minor-units
(never floats); `kind` distinguishes CI vs agent for reporting, not for the mechanism. The reserve checks
the prepaid balance before start (no balance → no run), so a runaway is self-limiting (a spend-down stop,
not a surprise infra bill). (Detail: 02 §8.)

**Prior art.** D8 / CI-2 / contract 11.7 (the universal gate); EI-03 §5.2 (refuse-start-on-exhaustion,
never interrupt in flight); X-5 (integer minor-units; wholesale ≠ markup).

**Floor.** CPU/mem/GPU-seconds + storage-GB-hours + egress-GB v1; finer meters (network shaping, cache-hit
credits) are measured follow-ons.

## HP-7 — Caching / artifacts at scale + GDPR-in-CI

**Resolution.** Content-addressed `BlobStore` (BLAKE3, **per-tenant dedup** — cross-tenant dedup is a
residency leak), residency-local (no global blob pool). **Artifacts** = retained outputs (correctness,
ArtifactRef-addressable, explicit TTL/GC). **Caches** = reconstructible (perf only; key = `hash(lockfile +
os + toolchain)`; LRU). **Poisoning resistance:** an `UntrustedFork` run gets an isolated cache scope and
**cannot write the trusted cache** (a restored cache is a build input → the scope boundary is the defence).
**GDPR:** CI is a careful `PersonalDataHolder` (PII leaks incidentally into logs/artifacts); identity is
stored as **pseudonym references**, not copied PII; erasure = **crypto-shred** (per-tenant DEK destroy) +
tombstone + short default TTL; the restriction flag suppresses indexing/agent-use/analytics/notif.
(Detail: 02 §7.2; 03 §6.)

**Prior art.** Storage T2 `BlobStore`; the git object model / Venti (FAST 2002) / IPFS CID (content
addressing); NIST SP 800-88r1 + Boneh & Lipton (1996) crypto-shred; Kleppmann *DDIA* ch.5 (tombstones,
references-not-payloads).

**Floor.** Object-segment log tier (T3) + OLTP range index ships; a dedicated time-series/wide-column tier
is measured-volume follow-on. **Per-tenant-DEK** crypto-shred ships; **per-subject free-text shred in logs**
is the GD-6 / `[OPEN → LEGAL]` follow-on. **Drills:** erasure-reaches-every-holder; fork-cannot-poison-cache;
fork-gets-no-secrets; residency (07 D-3/D-6/D-7/R-3).

## HP-8 — Secrets resolved inside the boundary (CI-1)

**Resolution.** Secret **names** in the job spec; resolved by an **in-boundary broker** per run, scoped to
exactly this job's references, via the shared secret capability (Id/GDPR-placed). **OIDC short-lived
audience-scoped credentials** over static keys (EU-sovereign least-privilege). **Untrusted/fork runs get
NO secrets by default** (the "fork exfiltrates prod secrets" CVE class); protected environments require
explicit grants/approval. **Log masking is best-effort defence-in-depth, NOT the boundary** — egress
default-deny is. (Detail: 02 §7.3.)

**Prior art.** CI-1 / EI-03 §3; OIDC federated short-lived credentials (the modern CI-secrets best
practice); the macaroon/biscuit attenuation model for the scoped token (Id §7).

**Floor.** OIDC + named-secret broker v1; per-cloud federation adapters are additive. **Drill:**
fork-gets-no-secrets — an adversarial fork run reads zero secrets (07 D-7).

## HP-9 — The sandbox escape drill (AG-D4) — the single hard gate

**Resolution.** This is not a hard problem to *resolve* but the hard *property* CI must prove. CI owns the
drill; it is the gating milestone before any untrusted code (CI **or** agent) runs. **Gate: zero escapes**
under an adversarial corpus on a **real kernel** (kernel-exploit primitives; cloud-metadata SSRF→cred
theft; control-plane/internal-RPC reach; cross-tenant network/storage; fork bomb vs `pids.max`; disk fill;
secret exfil via egress). Green attestation artifact or **CI is no-go**. Re-run on every backend/image/kernel
change. (Detail: 02 §4.3.) **Prior art.** EI-03 §3.5 ("a property not drilled on a real kernel is a claim,
not a fact"); the Firecracker/gVisor threat models. **Floor / `[OPEN → P5]`:** CI enumerates the
obligation; the full adversarial corpus is executed in Phase 5.

---

## Floors — consolidated (VISION §3)

| Floor (ships v1) | Named follow-on | Trigger |
|---|---|---|
| One sandbox backend (Firecracker) drilled | gVisor second backend | density/latency economics |
| Single-cell pipelines | cross-cell-spanning runs | Workflow multi-cell floor lifts |
| 1–2 `FleetProvider` adapters + self-hosted | more EU providers | demand (adapters) |
| DRR fair-share at claim time | hierarchical scheduler | measured starvation signal |
| Object-segment log tier + OLTP range index | time-series/wide-column log tier | measured volume |
| Per-tenant-DEK crypto-shred | per-subject free-text shred in logs | GD-6 / LEGAL |
| SLSA L1–L2 + SBOM | hermetic/two-party (L3+) | customer demand |
| Component trust model (digest-pin/sign/SLSA) | registry *product* | commercial |
| `myelin ci local` not built | laptop execution | UX-vs-fidelity decision |
