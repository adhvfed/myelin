# Comprehensibility backlog — finding the zen

The standing goal: code whose structure you can follow with ease. Clean APIs,
tear down bad abstractions, split logically, prune verbose comments (they're tech
debt). One piece at a time. Nothing is released — simplify boldly.

## Progress
- **`myelin-agent` crate** — fully clean. Retired `seam.rs` (a doc-as-code const table
  whose only reader was its own self-test; the real knowledge moved to
  `agent-fabric-floors.md`), and pruned the process-archaeology citations
  (`AG-P4 → P-216`, `contract 8.3`, `§`-refs) from every doc comment, keeping the
  invariants. The contract surface now reads as architecture. Done.
- **agent-host crate** — pruned + the dispatch web collapsed (five entry points → two,
  a `Tools` bundle). Reads clean. Done.
- **`gvisor.rs`** — decomposing a clean piece at a time via pure moves into `gvisor/`,
  each verified by identical test counts (550/0/36 + 588/0/49) before merge:
  cgroup enforcement → `gvisor/cgroup.rs`, git wire-protocol codec →
  `gvisor/git_wire_codec.rs`, Linux capability ABI → `gvisor/linux_capabilities.rs`,
  explicit-userns runsc invocation policy → `gvisor/explicit_userns.rs`, git-wire
  (tenant,region,repo) confinement validators → `gvisor/git_wire_confinement.rs`.
  25871 → 23125 so far (5 extractions).
- **agent-service engine doc-prune** — the process-archaeology (contract numbers, `§`
  refs, `AG-P*`/`P-*` IDs) pruned from the biggest engine files while KEEPING every
  security/money invariant rationale (ordering, fail-closed, plan-then-apply, token
  attenuation): `skeleton.rs` (the metered loop) done; `effect_api.rs` (plan-then-apply)
  in progress. Verified comment-only (0 code lines changed), test counts unchanged.
- **Lint-gate** — GREEN. The gvisor extractions above moved runtime-spawn code into
  `gvisor/` submodules the `gvisor.rs` no-host-exec exclusion didn't cover (a regression
  the green-gate drive caught); fixed with a tighter per-lint exclusion. Also taught
  `tenant-predicate` to recognize the `with_tenant_tx` RLS convention (11 secret-store
  false positives, verified safe) instead of blanket-excluding the file.

## Abstraction pass — myelin-agent-service (2026-08-04, founder: "make the abstractions shine; over and under are equally bad")
An abstraction audit → taste → verified-refactor pass on the hosted-agent crate. The audit's verdict: the CORE safety abstractions are already excellent (type-encoded routing split, type-state `AgentExecGate`, `PlanVerdict`, the dependency-breaking single-impl seams — do NOT collapse these); the real problems were all UNDER-abstraction (the same shape retyped instead of named once). Landed + pushed:
- **`RenderCtx`** (`17957a2a`) — the per-viewer render context was 8 positional args threaded through 4 functions behind `#[allow(too_many_arguments)]`; reified to a type. API reads `(ctx, what)`.
- **tool-def data tables** (`89c13b10`) — 6 byte-identical `register_*` fns + 17 near-identical `ToolDef` literals + a dead `requires_approval` write in each → one `mutate_tool_def`/`register_tool_defs`/`cap` in `defaults.rs`; each subsystem file is now a data table. Registered defs byte-identical (CDC-proven).
- **`handle_run` untangled** (`91ee756e`) — the 13-site token-teardown revoke dance → ONE RAII `RunTeardown` guard (revoke-on-drop, exactly-once-on-every-path, now with a regression test the copy-paste never had); the 95-line inline metering machine → `meter_turn`. Independently adversarially reviewed (6 hazard fronts, incl. panic-unwind = strictly safer).
- **`resolve_decision`** (`f59c8274`) — the HITL approve→admit order (the mutation-authorization decision) was duplicated verbatim across both loop drivers; extracted to one fn. Batch-specific `ledger.record` kept in the caller. HITL security tests unchanged.

**Taste decisions (declined / deferred):** SKIPPED folding `ApplyLedger`+`IdempotentToolLedger` into a generic `ExactlyOnceLedger<K>` — trivial ~15-line BTreeSet wrappers with clear domain APIs; a generic + 2 delegating wrappers would be mild OVER-abstraction. Optional follow-ups (medium payoff, more risk): `ComputeHardeningProfile` (exec.rs 11-arg `for_compute`, behavioral) and `GateOpenRequest` (hitl.rs 6-arg gate-open, security) — same "reify the many-arg concept" as RenderCtx if pursued. Plus a LOW: extend the teardown-guard test to the `MaxTurnsExhausted` + validation/exec-abort exits.

**Next abstraction targets** (codebase-wide directive): audit another crate the same way, and the big one — `gvisor.rs`'s remaining decomposition is an under-abstraction problem (GvisorBackend god-object) needing REAL decoupling, not pure moves.

## Known offenders
- **`gvisor.rs` (~23k lines)** — still the biggest module. The remaining clusters
  (finalize/quiesce, workspace/mount, runsc-launch, checkout-transport) are more
  coupled — threaded through `GvisorBackend`, sharing types — so they likely need
  real decoupling, not just a move. Load-bearing security code → behavior-preserving.
  (The clean pure-move seams may be nearly exhausted; the next bite is the test.)
- **Comment density, workspace-wide** — heavy module-doc + inline narration. Prune to
  what the code can't say.
- **Seam placeholders** — some `myelin-agent` §2.1 types were opaque newtypes filled
  in ad hoc; audit for ones that should now be real or removed.

## The move that works
Delegate one cohesive cluster to a fresh agent on a worktree → pure move (relocate +
minimum visibility + re-export to keep paths) → prove identical test counts → merge.
If a cluster only extracts by dragging half the file's shared types, don't — that
trades one tangle for another; it needs decoupling first.

## Approach
Pair every feature build with a simplification of what it touches. When editing a
file, leave it clearer than found. Prefer deleting a bad abstraction over adding a
wrapper around it. Keep correctness discipline on load-bearing/money/security paths.
