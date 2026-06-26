# MR-006 — Shape / Design Review ("is the frozen shape still right?")

Date: 2026-06-26. Status: AUTHORITATIVE (consumed by the orchestrator before dispatching the persistence/auth build).
Author: MR-006. Type: design review — **READ-ONLY, no code changed.**
Inputs: `shortcut-inventory.md` (MR-003) + `census/mr-001-substrate-findings.md` + `census/mr-002-git-findings.md`;
direction: `06-make-it-real-master-plan.md` + `07-implementation-roadmap.md` + `09-spine-prompt-ledger.md`;
architecture: `05-refined-shared-systems-architecture/{identity-and-access,agent-fabric,…}.md`.
All `file:symbol:line` refs were opened and read during this review.

---

## Executive summary

| # | Seam | Verdict |
|---|------|---------|
| 1 | **Identity / authz** (`myelin-identity-service`) | **SHAPE OK** — harden as-is |
| 2 | **Sandbox** (`myelin-ci-sandbox` launch/result seam) | **RESHAPE NEEDED** → `RESHAPE-001` (off the spine critical path; gates the CI track only) |
| 3 | **KMS / key hierarchy** (`myelin-storage/src/kms.rs`) | **SHAPE OK WITH NOTE** — envelope right; MR-025 adds the durable-root seam additively |
| 4 | **Tenancy / control-plane / RLS** (`myelin-storage/src/pg.rs`, `myelin-control-plane`) | **SHAPE OK WITH NOTE** (RLS is not a flag-flip) **+ RESHAPE NEEDED** → `RESHAPE-002` (sequencing: connection convention into MR-022) |
| 5 | **GDPR / audit** (`myelin-gdpr*`) | **SHAPE OK** — harden as-is |
| 6 | **Agent mock→`LlmAgentRuntime` seam** (`myelin-agent`, `myelin-agent-service`) | **SHAPE OK WITH NOTE** — governance choke point correctly drawn; the swap is clean |

**Single most important conclusion.** The spine's load-bearing **strategy seams are drawn correctly** — the
identity crypto seam, the KMS envelope, the GDPR erasure chain, and the agent governance choke point (`EffectApi`)
are all the right shape; the census-confirmed rot is **wired-in mock defaults + load-bearing state in `HashMap`s**,
both of which the spine MRs fill *behind the existing seams* without redrawing. Only **two** things need shape
attention before the expensive build, and **only one of them is on the spine critical path**:

- **`RESHAPE-002` (on the critical path):** the persistence foundation (MR-022) must establish a
  **tenant-scoped-transaction connection convention up front**, *before* the four durable-store MRs
  (MR-007/008/023/024) bind to today's bare-pooled-connection + session-GUC pattern. Otherwise MR-013 must
  retrofit RLS into N stores and a missed store is a silent cross-tenant bleed.
- **`RESHAPE-001` (off the critical path):** the sandbox `launch()`/result seam cannot carry a command's
  exit/stdout/stderr, so the CI long-pole (P-544) is a seam redraw, not a backend fill-in. It does **not** block
  the spine.

The single-cell dogfood path through the multi-cell machinery is **clean** (§ Single-cell-path confirmation).

---

## Seam 1 — Identity / authz — **SHAPE OK (harden as-is)**

**Current shape (verified).**
- Strategy seams, one per credential family, each a trait + a `Structural*` floor + a `with_*` injector:
  - Human/SSO: `authenticate.rs:CredentialVerifier` (trait, 125) · `StructuralVerifier` (floor, 146) ·
    `HumanSsoAuthenticator::with_verifier` (294); default ctor wires `StructuralVerifier` (286-289).
  - Machine/capability: `machine_auth.rs:TokenVerifier` (trait, 241) · `StructuralTokenVerifier` (262) ·
    `CapabilityAuthenticator::with_verifier` (413); default wires `StructuralTokenVerifier` (404-406).
  - Token mint: `mint.rs:TokenSigner` (trait, 140) · `StructuralTokenSigner` (164) ·
    `RunTokenMinter::with_signer` (272).
- One polymorphic `Principal{kind = Human|Agent|Service}`; `check` does not branch on `kind` (identity-and-access
  §3). The KMS-sharing seam exists: `lib.rs:StoreBackedCheck::with_kms` (556) lets a live service inject one shared
  `KmsEngine` (the default `StoreBackedCheck::new` mints its own at 548 — the prod-graph rot, not a seam defect).

**Soundness — census lens.** The census names the auth bypass the single most dangerous defect (SI-001..004): the
production graph wires the `Structural*` mocks as the **default** and every token/grant is a forgeable
pipe-delimited string. But every `Structural*` is reached *only through a trait the real impl also implements* —
the rot is the **default wiring**, not the seam. This is the textbook strategy-pattern shape the whole make-it-real
plan depends on.

**Soundness — make-it-real lens.** Real crypto (MR-010 human/SSO: OIDC/SAML/WebAuthn/SSH; MR-011 machine: PASETO/
biscuit/DPoP) is a *new impl injected through the existing `with_verifier`/`with_signer` seam* — the resolution/
telemetry body "does not change" (verified in the doc-comments at `authenticate.rs:286`, `machine_auth.rs:401`,
`mint.rs:139`). MR-012 then removes the `Structural*` defaults from the prod graph and the MR-004 scanner enforces
their absence. The human/SSO-vs-machine/capability split is **right**: two credential front-ends feeding one
`Principal`/`check` core — exactly the "one principal, one authz path" invariant (EI-02 §2).

**Verdict: SHAPE OK.** Harden as-is. No reshape. The only structural note (already in the duplicate-risk map): the
real impls must be injected through the existing `with_*` seams and the prod graph must **share** one `KmsEngine`
via `with_kms`, not mint per-process roots — a wiring job for MR-012/MR-022, not a seam redraw.

---

## Seam 2 — Sandbox `JobSpec → launch() → result` — **RESHAPE NEEDED (`RESHAPE-001`)**

**Current shape (verified).**
- Input side is **right**: `lib.rs:JobSpec` (131) carries `command: Vec<String>` (139), `limits.timeout_secs`,
  `env`, `secret_refs`, `egress`, `trust_tier`, `run_token`, `meter_to`, `idem_token`. The hardening recipe is real
  and complete (`hardening.rs:HardeningProfile` 149-185; `firecracker.rs:FcMachineConfig`; `gvisor.rs:OciConfig`,
  whose `from_spec` even captures `spec.command` at 67).
- Output/lifecycle side is **wrong for execution**: `lib.rs:SandboxBackend::launch` (441) returns
  `Result<SandboxHandle, _>`, and `SandboxHandle` (421-424) is **`{ guest_id: String }` only** — no exit code,
  stdout, stderr, or measured resource usage. `firecracker.rs:launch` boots `init=/bin/true` and never reads
  `spec.command`; `gvisor.rs:spawn_real_runsc` (227) only runs `runsc --version`. The runner
  (`runner.rs:610-645`) does **launch → ship stub frames → `kill` → `report_done(report)`** where `report` is
  supplied *into* the call, not derived from a run (`TerminalReport{passed, result_refs}`, runner.rs:353).

**Soundness — census lens.** SI-016/017/031/032 confirm: prod `launch()` runs no command on either backend, no
capture/timeout/limit enforcement, and the escape corpus runs through bespoke drill harnesses rather than the prod
path. This review adds the load-bearing finding the census did not draw out: **the result type cannot carry a
job's outcome.** Filling in a real `runsc run --bundle` / FC `init=` is not enough — there is nowhere for
exit/stdout/stderr/usage to flow back, and the runner kills the guest before any job could complete.

**Soundness — make-it-real lens.** CI is the explicit long pole (master plan Tier 3 / roadmap E2.1, P-544/545),
**deferred and not yet decomposed into MRs**, on its own parallel track. So this reshape **does not block the
spine**. But the prompt asks directly "is the production-exec path shaped wrong?" — and it is: P-544 must *redraw
the seam* (a completion/result-carrying return, or a `run()`/`wait()` method, plus result fields on the
handle/report), not just write backend bodies. Because `ToolHands::exec` *is* `launch(JobSpec{kind:Agent})` on the
one unified sandbox (`lib.rs:431`), the same redraw also unblocks agent compute-tools — one reshape covers both.

**Verdict: RESHAPE NEEDED** — see `RESHAPE-001`. **Scoped to the CI track; does not gate the spine.**

---

## Seam 3 — KMS / key hierarchy — **SHAPE OK WITH NOTE**

**Current shape (verified).** `kms.rs:KmsEngine` (461) holds `root: CellRoot`, `keks`, `deks`. The L0→L1→L2
envelope is correct: `CellRoot::generate()` (254) → `ensure_kek` wraps the KEK under the root (494/262) →
`ensure_dek` wraps the DEK under the KEK (523/553). AES-256-GCM wrap/unwrap is **real RustCrypto** (19-21, 262-275)
— census-confirmed (Section C). `destroy_kek` (664) is a real crypto-shred (`BTreeMap::remove`). The DEK read-path
is already behind a trait (`KmsAdapter::resolve_dek`, ~770), and BYOK/HYOK sit behind `key_origin.rs:KeyOrigin`.
A **shared-instance seam already exists** (`StoreBackedCheck::with_kms`).

**The two real gaps (census SI-006).** `KmsEngine::new()` mints a **fresh random root per process** (484), and
`backup_snapshot` (685-698) serializes **wrapped DEKs only** — it omits the root and the wrapped KEKs. So today a
restart/restore loses all keys and every encrypted column is unrecoverable; MR-009's kill-9/restart verify is
hollow without a fix.

**Soundness assessment.** The envelope *shape* is right and the crypto is real — nothing here gets hardened into a
defect. The fix MR-025 ("KMS durable cell-root + KEK persistence slice") owns is **additive**: (a) add a
**root-origin seam** — a `load-or-generate` constructor (e.g. `KmsEngine::open(sealed_root_store, cell_id)`) beside
`new()`, so the root comes from a software-sealed store; the existing envelope/crypto/`destroy_kek`/`KeyOrigin`
stay untouched; (b) extend `backup_snapshot` (or add a sibling) to include the **sealed root + wrapped KEKs** so a
restore reconstructs the hierarchy. These are new entry points, not a redraw — and they are exactly MR-025's scope.
HSM is therefore a *clean graduation of the root-origin seam* for the L0 root.

**The one honest caveat (Tier-4, not now).** HSM is **not** a pure trait-swap *today*: provisioning/rotation
(`ensure_kek`/`ensure_dek`/`rotate_kek`/`destroy_kek`/`backup_snapshot`) are concrete `KmsEngine` methods, not on a
trait — only `resolve_dek` is behind `KmsAdapter`. When Tier-4 HSM lands (P-524/525), the admin path will need a
provisioning trait. This is a **known graduation cost, deferred**, not a now-reshape — reshaping for HSM now would
be speculative against a Tier-4 surface.

**Verdict: SHAPE OK WITH NOTE.** MR-025 must (1) add the root-origin `load-or-generate` seam and (2) extend
`backup_snapshot` to carry the sealed root + wrapped KEKs; both additive, both within MR-025. Tier-4 will extend
`KmsAdapter` (or add a provisioning trait) for HSM — flag, don't pre-build.

---

## Seam 4 — Tenancy / control-plane / RLS — **SHAPE OK WITH NOTE + RESHAPE NEEDED (`RESHAPE-002`)**

**Current shape (verified).**
- RLS: `pg.rs:set_session_scope_in_region` (405-420) runs `set_config('myelin.tenant_id', $1, false), …` —
  **session-scoped** GUCs on a **bare `PoolConnection`** with **no transaction**, called inline before each
  statement (`put_tuple` 213, `reverse_index` 289, `check_tuple` 329). Pool is built with bare
  `PgPoolOptions::new().max_connections(..)` (129-132) — **no `after_release` hook**. A bare `pool()` accessor
  exists (149-152, documented "smoke check only").
- Tenancy vocabulary: `myelin-tenancy/src/lib.rs` is value-types-only by design (opaque `TenantId`, no
  `From<String>`; immutable `Region`; PII-free `CrossCellPointer` with a `compile_fail` guard). Enforcement is
  meant to live in storage — confirmed.
- Placement registry: `control-plane/src/registry.rs:Registry` (104-127) is in-memory `BTreeMap`s, but the
  internals are **private** and all access is through an encapsulated API (`placement`, `place_tenant` (gated by
  `check_placement_invariant`), `discover`).

**Soundness — registry (SHAPE OK).** The placement-registry API is correctly encapsulated, so a durable Postgres
backing (MR-024) is a **drop-in behind the same signatures** — callers unaffected. The tenancy value-type
discipline is good and intentional; keep it. Single-cell-now/multi-cell-later is right (see § Single-cell-path).

**Soundness — RLS (the load-bearing note).** MR-013's `SET LOCAL` + reset-on-release is **not a flag-flip**:
`SET LOCAL`/`set_config(..., true)` only lives for the *current transaction*, and the current code runs scoped
statements on a bare pooled connection with **no transaction**. Flipping `false→true` without introducing a
transaction boundary makes the scope a **silent no-op** — RLS would not apply and the bleed would persist
undetected. MR-013 must introduce a tenant-scoped-transaction wrapper (acquire → `BEGIN` → `set_config(..,true)` →
work → `COMMIT`), wire `after_release(DISCARD ALL)` as defence-in-depth, and close/guard the bare `pool()` hatch.
This is internal to `PgStore` (its public API is unchanged) and *is* MR-013's job — hence a NOTE, not a separate
reshape **for MR-013**.

**The sequencing reshape (`RESHAPE-002`).** The risk is *ordering*, not the seam: the durable-store MRs
**MR-007/008** (W2-W3, identity), **MR-023** (events), **MR-024** (control-plane) all build *before* MR-013 (W4)
and will run tenant-scoped queries. If each binds to today's bare-connection + session-GUC pattern, MR-013 must
retrofit transaction-scoped RLS into **every one of them**, and a store that misses the retrofit is a silent
cross-tenant bleed. The fix is to pull the **connection-acquisition + tenant-scope convention** forward into
**MR-022 (persistence foundation)** so every durable store inherits correct-by-construction scoping, leaving MR-013
to harden *policy* (RLS predicates, identifier allowlist, mTLS/region fail-fast) rather than re-plumb N stores.

**Verdict: SHAPE OK WITH NOTE (registry drop-in; RLS is MR-013's transaction-boundary job) + RESHAPE NEEDED**
(`RESHAPE-002`, sequencing — on the spine critical path).

---

## Seam 5 — GDPR / audit — **SHAPE OK (harden as-is)**

**Current shape (verified).** `#[personal_data]` is a real proc-macro (`myelin-gdpr-macros/src/lib.rs:76-187`)
emitting a static `PersonalDataField` registry per struct and `compile_error!`-ing untagged PII-named fields. The
`PersonalDataHolder` trait (`myelin-gdpr/src/lib.rs:442-455`: locate/export/rectify/restrict/erase) is implemented
by `gdpr-service/src/holders.rs:GdprOwnStoreHolder` (194); `erase` (319-371) crypto-shreds via the
`CryptoShredKms` seam (121-136) — today an `InMemoryShredKms` double (144-179), with the real `KmsEngine` binding a
**named floor** wired at boot. RoPA (`datamap.rs:ropa` 404-445) and the DSR state machine (`dsr.rs:162-251`, total
and ordered — `Verified` only reachable from `AwaitingHolders`) are real; the erasure checklist is **derived from
the data-map**, not hand-written, so RoPA/erasure can't drift.

**Soundness.** The tagging→shred→RoPA/erasure chain is hermetic, total, ordered, and coverage-tested (field-count
and holder-roster gates are non-vacuous). The `CryptoShredKms` seam is minimal and correctly drawn, with a deliberate
no-`myelin_storage`-import boundary (architecture-tested). Hardening = binding the real `KmsEngine` behind the
existing `CryptoShredKms` seam + durable G1-G7 tables — a fill-in, no redraw.

**Verdict: SHAPE OK.** Harden as-is. **Dependency to flag:** GDPR erasure's "0 recoverable, incl. backups"
guarantee depends on KMS per-subject DEK destruction surviving backup exclusion — i.e. it rides on **MR-025**
(durable KMS + a `backup_snapshot` that excludes shredded keys). The current `backup_snapshot` already excludes a
DEK whose tenant KEK was destroyed (kms.rs:694), so the shape is aligned; MR-025 makes it durable.

---

## Seam 6 — Agent mock→`LlmAgentRuntime` seam — **SHAPE OK WITH NOTE** (THE MOST IMPORTANT)

**Current shape (verified).** `myelin-agent/src/lib.rs` freezes six traits; the **only** strategy-swappable members
are `AgentRuntime::step(&Conversation)->StepOutcome` (the stateless brain, 258) and `ToolHands::exec` (the sandbox
hands, 283). `Agent`/`ToolSurface`/`EventInbox`/`EffectApi` are **platform-owned, identical for mock and real**.
`seam.rs` records `LlmAgentRuntime` as designed-not-built behind the frozen `AgentRuntime` seam (NAMED_FLOORS[0]),
with the swap defined as "a config/impl swap behind the frozen seam, NOT a rewrite." The platform-owned governance
body is **already built** in `myelin-agent-service` (`effect_api.rs` is ~82KB: the 8-step
schema→capability→delegation→tenant→budget→HITL→apply-via-public-endpoint→meter pipeline; plus `hitl*.rs`,
`identity.rs`, `cost_gate.rs`, `dsr.rs`). The external-MCP path is a `ToolDef` projection (NAMED_FLOORS[1]):
"an external MCP client is a Principal (no carve-out), gets a per-run token, flows through `EffectApi` exactly like
an internal agent."

**Question (a): does a local agent via CLI/MCP get the SAME auth + audit as a human — is agent governance real from
day one?** The **seam is drawn correctly** to make this true. `EffectApi::apply(&RunCtx, ProposedEffect)` is
**brain-agnostic** — it does not care whether the `ProposedEffect` originated from `MockAgentRuntime`, a future
`LlmAgentRuntime`, or an external/local Claude over MCP. Every mutation funnels through the *one* governance choke
point (Id `check` + delegation `∩` + HITL gate + apply-via-public-endpoint + meter + audit), and `mint_run_token`
(re-mintable on resume) exists to attribute a run. So "agent governance" (who/what did what, with what authority,
HITL on consequential actions, delegation intersection, per-run trace) does **not** depend on agent *hosting* — it
rides on `EffectApi`, which is built. **NOTE (binding on MR-021):** the MCP server must route tool calls through
`mint_run_token → EffectApi::apply`, *not* let local Claude act as a bare human PAT against the product API — the
latter would yield human auth+audit but skip agent-specific governance (HITL/delegation/run attribution). The seam
supports the right path; MR-021 must take it. This also depends on MR-014/015 (the public/edge endpoint
`EffectApi` step-7 calls) — the ledger already sequences MR-021 after MR-015, so the dependency is respected.

**Question (b): does mock→real swap cleanly later?** Yes. `AgentRuntime::step` is a single stateless method; the
platform owns the `Conversation` history, the loop, `EffectApi`, the trace. Swapping `MockAgentRuntime` for
`LlmAgentRuntime` touches one impl behind a frozen signature (the `no-llm-in-platform` lint keeps the rest of the
platform vendor-free). This is correct strategy-pattern; **no redraw**. (The opaque `String`-newtype value types in
`myelin-agent/src/lib.rs` are intentional frozen *signature* carriers; their bodies are fleshed in the service
crate — not a reshape risk.)

**One coupling to surface:** `ToolHands::exec` *is* the sandbox `launch()` (Seam 2). Agent compute-tools therefore
ride on the `RESHAPE-001` seam — but that is the CI track, deferred, and does not affect the governance shape.

**Verdict: SHAPE OK WITH NOTE.** The governance choke point is correctly drawn; the swap is clean. The note is an
implementation directive for MR-021 (route through `EffectApi`/`mint_run_token`), not a seam redraw.

---

## RESHAPE PROMPTS

### `RESHAPE-001` — Redraw the sandbox production-exec seam before the CI long-pole

- **What to redraw.** The execution result/lifecycle of the sandbox seam in `myelin-ci-sandbox`:
  - `SandboxBackend::launch` (`lib.rs:441`) currently returns `Result<SandboxHandle, _>` where
    `SandboxHandle = { guest_id }` — it cannot express a completed job. Give the seam a **completion-carrying
    outcome**: either change `launch` to return a result type with `{ exit_code, stdout/stderr refs, measured
    ResourceUsage, timed_out }`, or split into `launch` + `wait()/run()` (the async/long-park path already hints at
    this via `accept_async` + the `job.done` signal). Populate `TerminalReport` (`runner.rs:353`) from the real
    outcome instead of accepting it as a parameter, and reorder the runner to **launch → run command → collect →
    kill** (today: launch → kill, runner.rs:610-636).
  - Wire `spec.command` into both backends (FC `init=`/cmdline; gVisor `runsc run --bundle` over the already-built
    `OciConfig`) and enforce `spec.limits.timeout_secs` (present, never read).
- **Why (concrete failure if hardened as-is).** Filling in real backend bodies behind the current `launch()`
  leaves nowhere for a job's exit/stdout/stderr to return and the runner kills the guest before completion — the
  backend work would have to be re-done once the seam is corrected. This is a seam-signature defect, not a backend
  fill-in.
- **Which MR it must precede.** **P-544 / roadmap E2.1** (sandbox production exec) and P-545 (escape corpus through
  the prod path). It is the *first step* of the CI long-pole track. Because `ToolHands::exec == launch(JobSpec{kind:
  Agent})`, it also precedes any agent compute-tool execution.
- **Does NOT gate the spine.** CI is deferred to its own parallel track; the spine (persistence/auth/API/UI/CLI/MCP)
  proceeds without it.
- **Rough scope.** ~mid. Seam + result types + runner control-flow in `myelin-ci-sandbox`; the hardening profile,
  `FcMachineConfig`, and `OciConfig` builders are real and **reused**, not rewritten.

### `RESHAPE-002` — Pull the tenant-scoped-transaction connection convention into MR-022 (sequencing)

- **What to redraw.** Establish, in **MR-022 (persistence foundation)**, the **single connection-acquisition +
  tenant-scope convention** the whole substrate uses: acquire → open a transaction → `set_config(..., true)` /
  `SET LOCAL` → work → commit, with `PgPoolOptions::after_release(DISCARD ALL)` wired and the bare `PgStore::pool`
  hatch (`pg.rs:149`) removed or guarded. Provide it as a helper (e.g. `with_tenant_tx(tenant, region, |tx| …)`)
  that every durable store acquires through. This is *not* the full RLS policy (predicates/allowlist/mTLS — that
  stays MR-013); it is the **connection plumbing** the RLS policy needs to be correct.
- **Why (concrete failure if hardened as-is).** Today's pattern (`pg.rs:405-420`) sets **session-scoped** GUCs on a
  **bare pooled connection with no transaction**. If MR-007/008/023/024 build their durable stores on this pattern,
  then (a) MR-013 must retrofit transaction-scoped RLS into four separate store families = wasted work, and (b) any
  store that misses the retrofit silently bleeds across tenants (a `SET LOCAL` with no surrounding transaction is a
  silent no-op). Getting the convention right once, first, makes all four stores correct-by-construction.
- **Which MRs it must precede.** **MR-007, MR-008, MR-023, MR-024** (all the durable-store builds) — i.e. it must be
  in MR-022's scope before the W2/W3 persistence band runs. MR-013 then hardens *policy* on a correct foundation.
- **On the spine critical path.** Yes — this is the headline pre-build action.
- **Rough scope.** ~low-mid. One connection/transaction helper + pool config in `myelin-storage`, adopted as the
  convention; no public `PgStore` API change.

> Conservative-call note: RLS-within-MR-013 and the KMS durable-root-within-MR-025 are deliberately **NOT** raised
> as reshape prompts — they are fixable inside the build MR that already owns them (a NOTE, per the prompt's bar).
> `RESHAPE-002` is raised only because the failure is one of **ordering across multiple MRs**, which the build MRs
> cannot self-correct. `RESHAPE-001` is raised because it is a genuine **seam-signature** defect the eventual P-544
> author must not mistake for a backend fill-in.

---

## Single-cell-path confirmation

**Confirmed clean.** The single-cell dogfood path runs *through* the multi-cell machinery via the **same shared
organs**, with no fork and no requirement that multi-cell be real:

- `control-plane/src/self_host.rs:DegenerateControlPlane` delegates to the shared `Registry::discover` /
  `Registry::placement_of` / `CellGateway::route` / free `residency_verify` — there is **no degenerate-only routing
  API** (self_host.rs:25-30), and the parity is drill-tested end-to-end (606-640) against the fleet's exact path.
- The placement registry is a **one-row `BTreeMap`** in single-cell mode; routing answers are trivially "this
  cell," but computed by the *same* functions a fleet uses. Making the registry durable (MR-024) is a drop-in behind
  the encapsulated API and does **not** require multi-cell to be live.
- Identity (`Principal`/`check`), KMS (one cell root), tenancy vocabulary, GDPR holders, and the agent governance
  choke point are all cell-local by construction; the multi-cell tail (live migration, cross-cell DSR/bridge/zookie,
  fleet bulkheads — SI-042/043/049-057) is **off the critical path** and correctly left in place, not ripped out.

The master plan's "build the full package but single-cell first" holds at the seam level: every spine seam has a
clean single-cell instantiation, and the multi-cell machinery is dormant-but-present, not a prerequisite.

---

## Cross-references
- Shortcut inventory (the seam SI-NNN map): `shortcut-inventory.md` (MR-003).
- Spine ledger (the MRs these verdicts gate): `09-spine-prompt-ledger.md` (MR-022/007/008/023/024/025/013;
  MR-010/011/012; MR-014/015/020/021).
- Architecture: `05-refined-shared-systems-architecture/identity-and-access.md`, `agent-fabric.md`.
