# Make-It-Real Ledger — Phase 4: the Actions/CI subsystem track (E2.1–E2.5)

Date: 2026-06-29. Status: PLAN. The spine + the sandbox-independent Git daily driver (GT-001..005) are done.
This decomposes **Actions/CI — "the long pole"** (roadmap priority #2), grounded in the MR-002 census + the
shortcut-inventory CI CRITICALs (SI-016/017) + MR-006 RESHAPE-001 + roadmap E2.1–E2.5. Per the master plan:
**no rush; get the sandbox right before trusting it with your supply chain.** CI runs your own build +
dependency code, so a weak sandbox is a supply-chain hole.

**Execution reconciliation (2026-07-23):** this file remains the decomposition; the live source of
truth is release ledger 14 §R4.2. CT-004 now includes the opt-in coordinated production runner and the
durable CI→Git check projection. Attempt numbers are allocated immutably per run/context in the same
reserve commit as the run, queued outbox rows, and trigger dedup; PipelineStarter consumes rather than
reallocates them. CT-004 remains in progress until a real founder push produces, settles, and surfaces
its exact-head check. CT-007 is still unopened; GitHub Actions must not be removed before that
acceptance pass and the required CT-005 surface are genuinely usable.

## Environment (confirmed — this track is testable here)
Firecracker v1.16.0 + gVisor (`runsc`) on PATH; `/dev/kvm` present. So the production microVM boot + the
AG-D4 escape corpus run for real here (set `MYELIN_REQUIRE_KVM=1` so a real guest must boot — no skip).

## The census CRITICALs this track closes
- **RESHAPE-001 (MR-006):** `SandboxHandle{guest_id}` + `launch()->Result<SandboxHandle>` can't carry a
  command's exit/stdout/stderr/usage; the runner does launch→kill with `TerminalReport` passed IN. The
  result/lifecycle seam must be redrawn → CT-001.
- **SI-016** sandbox `launch()` is a no-op in prod (Firecracker `init=/bin/true`; gVisor only probes
  `runsc --version`; `spec.command` never runs) → CT-002.
- **SI-017** the AG-D4 escape corpus runs through separate drill harnesses, NOT the production `launch()` —
  so "0 escapes" certifies a path real jobs never take → CT-003.

## Conventions
Same as the spine/Git ledgers: `CT-NNN` ids; anti-duplication grep + ledger-vs-commits cross-check opens every
prompt (reuse `hardening.rs::HardeningProfile`, `firecracker.rs`, `gvisor.rs`, `escape_corpus.rs`, the runner,
+ the MR-014/015 edge + MR-019 shell + MR-020/021 CLI/MCP — extend, never fork); orchestrator runs the FULL
gate; **independent SECURITY verification** on every prompt — and for CT-002/003 the verifier runs the escape
corpus THROUGH the production path itself (a sandbox escape = supply-chain compromise; the verifier tries to
escape). Commit per prompt. **No green without a real microVM boot** (`MYELIN_REQUIRE_KVM=1`).

## The CI-track prompt set

| ID | Epic | Title | Deps | Size |
|---|---|---|---|---|
| CT-001 | (RESHAPE-001) | **Redraw the sandbox launch/result/lifecycle seam:** `launch → run(spec.command) → wait → SandboxResult{exit, stdout, stderr, usage, timed_out}` (+ settle-once); the seam both Firecracker + gVisor backends and the runner implement. Sandbox-INDEPENDENT (a Rust type/API redraw; no exec yet). | — | mid |
| CT-002a | E2.1 (P-544) | **Firecracker PRODUCTION exec** (the DEFAULT backend; CI-P2 = the one through the escape drill FIRST): a real `spec.command` runs in a real microVM (NOT `init=/bin/true`) — reuse the proven `drill_config_json` recipe (2nd read-only virtio drive + `init=/bin/bash /dev/vdb`) with a command-runner init script; capture exit/stdout/stderr (bounded by `SANDBOX_CAPTURE_BOUND`) + usage from the serial console + `VmmChild::wait`; `spec.limits.timeout_secs` kills the whole guest (`timed_out=true`); settle once. Real boots (`MYELIN_REQUIRE_KVM=1`). | CT-001 | high |
| CT-002b | E2.1 (P-544) | **gVisor PRODUCTION exec** (the NAMED SECOND backend, CI-P28): real `runsc run --bundle` of the OCI bundle built from the spec (`OciConfig::from_spec` + a rootfs) — NOT a `--version` probe; same `SandboxResult` capture convention + timeout→whole-guest-kill as CT-002a. Real `runsc run`. | CT-002a | high |
| CT-003 | E2.2 (P-545) | **Production-path escape verification:** re-run the **AG-D4 escape corpus through the PRODUCTION `launch()`** on both backends — **0 escapes** — with a guard that fails RED if a case is routed to the harness shortcut instead of the prod path. The supply-chain-safety proof. | CT-002 | high |
| CT-004 | E2.3 | **CI backend HARDEN + RECONCILE** (ci-controlplane / dispatch / sandbox): durable pipeline/run/step state, the scheduler/lease/metering, the log pipeline — on the durable substrate (MR-022). | CT-002 | high |
| CT-005 | E2.4 | **CI API + UI + CLI/MCP:** pipelines / runs / live log-tail (SSE) through the edge (MR-014/015) + the web UI (MR-019) + the CLI/MCP (MR-020/021); reuse the CI ViewSpec. | CT-004 | high; may split (API/UI vs CLI/MCP) |
| CT-006 | (GT-006) | **The Git smart-transport WIRE** (`upload-pack`/`receive-pack` = real `git clone`/`push`/`fetch`) via the now-hardened sandboxed git + the production `WireExecutor` + the git server binary/listener + the external-oracle test (real `git clone`/`push` + `git fsck`). UNBLOCKED by CT-002. **DONE** — split into: **CT-006a** sandbox git-wire capability (RO repo mount + quarantine + stdin/stdout, confined) `15b01a3`; **CT-006b** production WireExecutor + GitCore wiring, clone/fetch through the seam `2ce5742`; **CT-006c** wire stdout streaming + HTTP upload-pack server + external-oracle CLONE/FETCH (real `git clone` works) `1fe630d`; **CT-006d** PUSH path (rootless quarantine via ingest-not-receive-pack + in-process fsck/policy + one-tx ref-CAS/outbox + external-oracle PUSH) `fd0eb19`. All security-verified. Follow-ups: per-repo authz seam (task #60, platform-wide), orphan-on-reject (task #61, bounded). | CT-002, GT-001 | high |
| CT-007 | E2.5 | **Cut over from GitHub Actions** — move CI off GitHub Actions onto the hardened sandbox. ONLY after CT-003 (0 escapes through the prod path) is green. The reward AFTER the work. | CT-003, CT-004, CT-005 | mid |

## Waves
- **W1:** CT-001 (seam redraw — sandbox-independent)
- **W2:** CT-002a (Firecracker prod exec — real boots) → CT-002b (gVisor prod exec — real `runsc run`)
- **W3:** CT-003 (escape verification, 0 escapes through prod) · CT-004 (CI backend harden) · CT-006 (the git wire, unblocked)
- **W4:** CT-005 (CI API+UI+CLI/MCP)
- **W5:** CT-007 (cut over — only after the sandbox is genuinely hardened)

The danger concentrates in CT-002/003 (untrusted execution + escape verification). Those get a security
verifier that actively tries to escape the production sandbox; "0 escapes" is only credible THROUGH the prod
path (CT-003's guard enforces that). CT-007 (the GitHub-Actions bill killer) is the reward, gated on CT-003.
