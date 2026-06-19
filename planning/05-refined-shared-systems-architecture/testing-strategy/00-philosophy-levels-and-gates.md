# Phase 5-C — Testing Strategy: Philosophy, Test Levels, and the Gate/Ratchet Model

> Phase: `05-refined-shared-systems-architecture/testing-strategy`. **The keystone** of the system-wide
> testing strategy (VISION §5: "specifies a testing strategy for the system as a whole and in parts").
> Canonical brief: [`VISION.md`](../../../VISION.md) (never contradicted). THE philosophy source:
> [`external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (cited `EI-01 P<n>`); the hard problems
> [`external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) (`EI-04 §<n>`).
> Binding testing directives: [`integration-directives.md`](../../02b-doctrine-integration/integration-directives.md)
> Phase-5 block (T-1..T-9), Phase-8 block (E-1..E-9), and R-1..R-6. Spine:
> [`architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md) (ADR-16 backpressure/
> human-lane, ADR-17 fail-static, ADR-18 backup/restore-verify, ADR-19 four-primitive, ADR-20 one sandbox).
> Frozen build-to surface: [`../contract-index.md`](../contract-index.md) +
> [`../00-reconciliation-decisions.md`](../00-reconciliation-decisions.md). The drill inventory this
> consolidates: [`../../03-shared-systems-architecture/drills-and-open-questions.md`](../../03-shared-systems-architecture/drills-and-open-questions.md)
> (Part A: 101 enumerated drills → 9 families + ~7 unique; Part B: open questions; Q32/Q33 = the threshold
> numbers Phase 5 sets). Design-QA gates: [`design-language.md`](../../02-holistic-architecture/design-language.md)
> §8b. Date: 2026-06-19.
>
> **What this document is.** The *philosophy, the level model, and the gate/ratchet machinery* — the spine
> the rest of the testing-strategy folder hangs off. It does NOT re-enumerate every drill (that is the drill
> catalogue, doc 01, which lifts the 101 Phase-3 drills and freezes their thresholds against Q32). It fixes:
> (1) the doctrine made concrete for Myelin; (2) the test pyramid for a Rust-on-Postgres workspace incl.
> where mutation testing is **mandatory**; (3) the committed-gate/ratchet model — what runs on EVERY change
> vs SCHEDULED, the gate invariant, loud-never-swallowed, and dogfooding as a testing lever.
>
> **Plain-text identifiers throughout** (no backticks-as-emphasis); lint/contract names are written plainly.
> **Markdown only; no commits.**

---

## Part 0 — The one-paragraph thesis (so no later doc loses it)

A property of Myelin **does not exist** until a drill forces its failure and the platform's own telemetry
watches it survive (EI-01 P3; T-1). Every gate therefore resolves to a **single quantified threshold** you
could read off a dashboard; a target you cannot measure is not a gate. A capability is **proven** only when a
drill emitted a **green artifact** (a signed, dated, scorecard-linked record); until then it is **claimed**,
and saying "claimed" is honest while saying "done" is the failure (EI-04 §4; T-4). Gates are **committed and
mechanical** — an uncommitted gate is no gate (EI-01 P5) — and violations are **loud, never swallowed** (no
`|| true`, no silent filter). Work is ordered by **what kills you first**: silent data-loss and sandbox-escape
gates outrank every feature, and **no later phase is done over a red earlier gate** (EI-01 P2; R-1/R-2). And
because Myelin **hosts itself** (one CI graph, dogfooded), the gates are not a side-car — they run on the
platform's own commits, which is the cheapest, most honest load generator we have.

---

## Part 1 — The philosophy, made concrete for Myelin

The doctrine (EI-01) is the philosophy. This section turns each of its tenets into a *binding rule with a
Myelin-specific shape*, so an implementing agent in Phase 8 can apply it without re-deriving it.

### 1.1 Prove-it-or-it-isn't-real — the failure-injection harness is the unit of proof (T-1, T-3; EI-01 P3)

The atomic unit of "proof" is **a drill**, not a test. A drill is: a **fault** (a scoped, reversible break of
a dependency or a load multiplier), a **workload** running through the **real** code path, and an
**assertion read from production telemetry** (contract 1.8, the survival-signal set). A passing unit test that
never injected the fault proves *the code*, not *the property*. The harness that makes drills possible is a
**first-class platform deliverable, sequenced early** (R-3): the load generator that multiplies traffic
**1× / 10× / 30×** and mixes principal kinds (human / agent / service / CI — the `actor.kind` axis from
contract 4.1), the **scoped reversible dependency-break** primitive (kill a broker, pause Identity, corrupt a
blob, sever a cell), and the **assertion library that reads the metrics port** (contract 1.8: RED/USE per
principal-kind+tenant, consumer-lag, outbox-depth/dead-letter, breaker-state + Retry-After issuance,
fail-static fresh/stale/closed ratios, shed counts per lane, causal-depth histogram + tripwire firings,
reindex parity-hash, erase receipts, residency attestation, misroute count, plus the per-system signals named
in Phase-3 A.3).

**Observability is part of the pass condition (T-1; EI-01 P3, the non-negotiable):** a system that survives a
drill **but emits no signal that it survived has FAILED the drill.** Concretely, every drill's pass condition
is the conjunction of (a) the behavioural assertion (0 lost / 0 leaked / latency-within-budget) **AND** (b)
the telemetry assertion (the survival signal was present, correct, and alarmed where expected). A Phase-3 doc
that omits its signals fails the X-1 telemetry contract; a drill that does not read them is not a proof.

### 1.2 Quantified thresholds — every gate names its number (T-2; EI-01 P3)

Each gate below and in the drill catalogue resolves to a number. Phase 3 proposed **defaults-to-beat**;
**Phase 5 sets the binding numbers** (Q32/Q33) by measuring on the failure-injection harness against a
restored prod-scale copy. The headline numbers (frozen here as the v1 binding set, tunable only upward in
strictness, never weakened):

| Quantified floor | Binding number (v1) | Source / drill it gates |
|---|---|---|
| Sandbox escapes (real kernel) | **0** — the single hard go/no-go | AG-D4; ADR-20; E-9; T-5 |
| Cross-tenant reads (IDOR, any surface) | **0** rows / 0 edges / 0 docs / 0 count-or-IDF leak | F2 family; ID-D3, SUB-D7, REF-D2, SRCH-D3, CP-D2 |
| Unauthorized-visibility leak (F1) | **0** leaked docs/edges/backlinks/notifs, **0** count/IDF/ranking leak | F1 family; ID-D4, REF-D1, SRCH-D1, NOTIF-D4 |
| Disabled/revoked user → access gone | **≤ 5 minutes** on EVERY surface (UI/API/git-wire/agent) | F8 family; ID-D1; N=5 (Q32) |
| Fail-static staleness window W | **≤ revocation SLA** and **≥ agent-token TTL** (proposed 5 min, DPO-ratified L-1) | F7 family; ID-D2; ADR-17; Q34 |
| Messages lost across a reconnect / broker drop | **0** lost, **0** ghost, **0** duplicate-effect | F5 family; BUS-D1/D4, SUB-D1/D2, FLOW-D1/D5 |
| Reconnect-loses-zero-ops (collab transport) | **0** ops lost across a dropped connection | OQ-J; KN-1; T-5 |
| Agent surge the human lane survives | **30×** surge: human-lane latency within budget, agent lane sheds (429+Retry-After), **0** cross-tenant impact | F6 family; ADR-16; surge=30× (Q32) |
| RPO (recovery-point) | **≤ 5 min** (WAL tail) | STOR-D2; ADR-18; Q32 |
| RTO (recovery-time) | **≤ 1h / tenant, ≤ 4h / cell** | STOR-D2; Q32 |
| Restore cross-seam integrity | **0** loss; OLTP↔blob↔index↔offset at one consistent point | F3 family; STOR-D1 (the headline durability gate); ADR-18 |
| Reindex-from-cold parity | cold **==** live (docs, ACL, ranking, edges, vectors, inbox), live-consumer path only | F4 family; SRCH-D5, REF-D4, NOTIF-D3, BUS-D5 |
| Causal-loop halt | **≤ depth ceiling 12** (agent/causal); Refs traversal **≤ 16**; tripwire trips the per-tenant breaker | F9 family; AG-D7, BUS-D6; C-G two-ceilings |
| Erasure reaches every holder | **0** holders missed (H1–H18), 0 recoverable PII post-erase | GA-D1; EI-04 §1 |
| Audit tamper detection | **100%** (chain break + STH-proof fail + external-witness mismatch) | GA-D3 |
| Editor round-trip | **render(parse(md)) === md** over the corpus, byte-exact | DL §8b.2; T-5; KN-4 |
| Measured contrast | **WCAG 2.2 AA** measured (never a stated ratio) | DL §8b.3; T-8 |
| Keyboard response | **< ~100 ms**; suppress spinner-flash **< ~1s**; pages render not animate-in | DL §8b.6; T-8 |
| Mutation score on the mandatory cores | **≥ threshold** (§2.4) — the cargo-mutants quality bar | T-5/AG-D9; `.gitignore` mutants.out signal |

**Never weaken a threshold or invert an assertion to make a check pass (T-2; EI-01 P3).** A red gate is
*information*. The honest move is a dated "needs human verification" / "claimed, not proven" scorecard entry,
never softening `== 0` into `< small` or flipping an assertion. This rule is itself enforced (see the
no-threshold-weakening gate, §3.5).

### 1.3 Name-your-floors + the source-verified scorecard (T-4; VISION §3; EI-04 §4)

Shipping a **floor** (partial / untested / deferred) is correct and necessary — the whole platform ships as a
ladder of named floors (CAS→CRDT, single-cell→multi-cell, node-backed→object-backed git, read-time-rollup→
materialised, KB-comments→Chat-threading). The failure is a floor **masquerading as done**. Two artifacts keep
it honest:

- **The source-verified scorecard (doc 02 of this folder).** Two columns per capability: **claimed** (the doc
  says it does X) and **proven** (a drill emitted a green artifact for X, dated, linked to the CI run). A row
  is "proven" **only** when source-verified — a doc claim is never proof (EI-04 §4.3; E-2). A FLOOR drill
  (GA-D8, CP-D7, CP-D8 multi-cell/migration) is **owed only when its follow-on is built**, and is listed now
  so the gap is visible, not invisible.
- **The durable gap report (E-3).** Shipped floors + their explicit linked follow-ons + claimed/proven
  status. Seeded with: CAS-floor→CRDT (KN-1), single-cell→multi-cell (OQ-I), pseudonymous-commit residual
  limit (X-7), read-time-rollup floor (KN-3), node-backed→object-backed git (STOR-5), KB-comments→Chat-
  threading (OQ-L), SCIP/LSIF find-usages, the per-surface shed budgets (OQ-K, v1 floors tuned by drills).

**Untested-but-named is acceptable; silent skipping is the failure (EI-01 P4).** Every piece of work records
whether it was exercised: yes / no / partial.

### 1.4 Code-wins-over-docs + dated status (E-1, E-2; EI-01 P1)

The running code and its observable behaviour are the source of truth; planning docs are intent to be
re-verified. **When a doc and the code disagree, the code wins — fix the doc, then proceed.** Every status /
capability note is **dated** with the commit/verification it was true at, and is reported as **claimed** until
a drill makes it **proven** (E-2). Stale capability notes *actively mislead* the next sequential agent (the
"X is a stub" that is no longer true), so the testing strategy budgets periodic **truth-up passes** (R-6)
whose only job is to re-sync docs to code, and the event log makes **replay-the-symptom** root-causing
first-class (E-6: investigate before you build).

### 1.5 Order-by-non-negotiability — the gates are sequenced, not the features (R-1, R-2; EI-01 P2)

Sequence by what kills you first. **Stop-the-bleeding gates outrank every feature:** the sandbox-escape drill
(AG-D4, the single hard go/no-go before any untrusted customer code runs — CI *or* agent) and the silent-
data-loss gates (STOR-D1 restore cross-seam; F5 zero-loss-across-reconnect; F3 restore integrity) are
**keystone milestones**, built and green before the feature surfaces that sit on top of them. Then the
keystones (the load-bearing `list_objects` push-down, the outbox, the durable-workflow engine, the resilient
client, the failure-injection harness itself), then breadth, then polish/scale. The **gate invariant** makes
this enforced, not aspirational (§3.3).

### 1.6 Actually-try-it — drive the real UI, chain the mutations (T-6, T-7; EI-01 P4)

Automated tests prove the parts and routinely miss what only appears when a real user/agent drives the whole
thing. Two binding consequences:

- **Chained-mutation E2E (T-6).** Integration tests that use a fresh DB, call one handler, and render once
  with final state miss the bugs that live in **mid-flight state updates**. Myelin's E2E tier **chains
  mutations** over one session and asserts state *between* steps — and chains them **across subsystems** (a
  commit → a CI check → a merge-gate → a notification → an inbox read), exactly the cross-artifact flow the
  platform exists to enable.
- **The switch test (T-7).** A user-facing surface is "done" only when a team could **move to it without
  hitting a wall the old tool didn't have**, and that verdict is reached by **driving the real UI in a
  browser**, not by reading a feature list. Frontend gates (T-8; DL §8b) — measured contrast, hard latency
  budgets, overlays tested against the real anchor — are part of the done-bar.

---

## Part 2 — The test levels (the pyramid for a Rust-on-Postgres workspace)

Myelin's default stack is Rust + Postgres (VISION §4; the `.gitignore` is pre-seeded for Cargo and
**cargo-mutants**, which signals the expected quality bar). The pyramid has **eight levels**; cheaper/wider at
the base, scarcer/more expensive at the top. Each level names what it proves, its tooling, and what it does
NOT prove (so the next level up is justified).

```
                 ┌─────────────────────────┐
   scheduled  ▲  │  8. Load / chaos DRILLS  │  the failure-injection harness; 1×/10×/30×; the 9 drill families
   expensive  │  ├─────────────────────────┤
              │  │  7. Cross-subsystem E2E  │  chained mutations across subsystems; drive the REAL UI (switch test)
              │  ├─────────────────────────┤
              │  │  6. Per-service          │  one service + its real Postgres (testcontainers); serve(AppSpec) booted
   every      │  │     integration          │
   change     │  ├─────────────────────────┤
   (CI)       │  │  5. Contract / seam      │  both sides of every frozen contract; envelope/SetExpr/CheckStatus shapes
              │  ├─────────────────────────┤
              │  │  4. MUTATION (cargo-      │  the quality bar on the mandatory cores (§2.4) — tests that don't test fail here
              │  │     mutants)             │
              │  ├─────────────────────────┤
              │  │  3. Property-based        │  proptest: invariants over generated inputs (ordering, idempotency, round-trip)
   cheap      │  │     (proptest)           │
   wide       │  ├─────────────────────────┤
              ▼  │  2. Unit                 │  pure functions, single types; the markdown parser, LexoRank, the upcasters
                 ├─────────────────────────┤
                 │  1. Static / lint        │  the committed architecture lints; compile-time gates; the cheapest ratchet
                 └─────────────────────────┘
```

### 2.1 Level 1 — Static / lint (compile-time, the cheapest ratchet)

The committed architecture lints (contract 1.6) run as part of `cargo build` / clippy / a custom lint pass on
**every crate, every change**. They make whole bug-classes *impossible to compile*, not merely tested. They
are the ratchet's floor (§3) and are enumerated there. Cost: ~seconds; coverage: structural invariants.

### 2.2 Level 2 — Unit (pure functions, single types)

Standard `#[test]` over pure logic with no I/O: the markdown-subset parser/serializer, the LexoRank
`order_key` encoder (base-62, midpoint bisection, jitter, 48-char rebalance trigger — X-3), the schema
upcasters (`(type, from_ver) → to_ver` pure fns, contract 2.8), the `QueryAst` interpreter's evaluation, the
`SetExpr` → SQL lowering, the `#sub` grammar parse/reject. **Editor primitives are unit-tested standalone
before the integrated editor** (KN-4): enter-splits-block, caret-as-char-offset, the serializer/offset/DOM-
surgery primitives. Does not prove: anything touching Postgres, concurrency, or the bus.

### 2.3 Level 3 — Property-based (proptest)

Where an **invariant** must hold over a large input space, a single example is not enough — generate inputs
and assert the invariant. The mandatory property targets:

- **render(parse(md)) === md** over a generated markdown-subset corpus (DL §8b.2; T-5; the WASM editor and
  the server share the Rust core — same code, both sides).
- **LexoRank total order**: for any sequence of inserts/moves, byte-comparison of `order_key`s equals intended
  rank order; two concurrent midpoint inserts (with jitter) produce **distinct** keys (the concurrency-safety
  property).
- **Outbox/consumer idempotency**: applying any event twice has the same effect as once (dedup ledger,
  contract 2.5) — generated redelivery orderings.
- **Upcaster soundness**: any old-version envelope upcasts to a valid current-version envelope.
- **`SetExpr` monotonicity**: the lowered SQL predicate admits exactly the tuple-set the algebra denotes (no
  tuple admitted that `check` would deny) — the leak-free property in algebraic form, before it is a drill.
- **`#sub` resolution ladder**: every input resolves to exactly one of {live, moved, outdated, gone, denied,
  erased} and **never** to a leak (denied always tombstones).

### 2.4 Level 4 — MUTATION testing (cargo-mutants) — the quality bar, MANDATORY on named cores

cargo-mutants mutates the source (flips a `>` to `>=`, deletes a `?`, swaps an arm) and asserts the test suite
**fails** — a mutant that survives is a line your tests do not actually test. The `.gitignore` pre-seed
(`**/mutants.out*/`) is the signal that this is the expected bar. Mutation is **expensive**, so it is
**mandatory on the cores where a silent logic bug is catastrophic**, and best-effort (advisory, non-blocking)
elsewhere. **Mandatory mutation surfaces (the gate is RED below the threshold):**

| Mandatory mutation core | Why it is non-negotiable | Threshold (v1) | Drill tie |
|---|---|---|---|
| **The agent loop: event → trigger → effect → event** | the agent-native heart; a swallowed guard = an unbounded loop or a wrong effect | **≥ 90%** caught | AG-D9 (cargo-mutants over event→trigger→effect→event ≥ threshold) |
| **Authorization decision path** — `check`, `list_objects`/`SetExpr` lowering, `delegation` intersection, caveat eval, zookie watermark | a surviving mutant here is a silent leak or a privilege escalation | **≥ 95%** caught (highest bar) | F1/F2/F8 families; ID-D3/D4 |
| **The outbox** — `OutboxTx::emit`, relay claim (`FOR UPDATE SKIP LOCKED`), dedup ledger, supersession (`run_attempt`) | a surviving mutant = a lost/ghost/duplicate event = silent data loss | **≥ 95%** caught | F5 family; BUS-D1/D4 |
| **Reserve/settle cost gate** | a surviving mutant = a runaway spend or a refused-when-funded | **≥ 90%** caught | AG-D11, FLOW-D6 |
| **The durable-workflow determinism guard** (`flow-determinism`, replay divergence halt) | a surviving mutant = a silent double-effect on replay | **≥ 90%** caught | FLOW-D2 |
| **Crypto-shred / erasure key-selection** (per-subject vs per-tenant DEK choice; pseudonym shred) | a surviving mutant = PII that survives erasure | **≥ 95%** caught | GA-D1, STOR-D4 |

Elsewhere (UI glue, projection rendering, non-security CRUD) mutation runs **advisory** (reported, not
blocking) on a scheduled cadence so the trend is visible without throttling every PR. The mandatory cores'
mutation gate runs **on every change to those crates** (it is path-scoped, so it does not tax unrelated PRs).

### 2.5 Level 5 — Contract / seam tests (both sides of every frozen contract)

Every contract in the index is **stable** (ADR-01): changing it is one whole-workspace PR that breaks every
consumer's build *now*. The seam tests assert the **shape and units** at the boundary so two subsystems cannot
drift (T-9; EI-01 P7 — reconcile names AND units before either side ships). Mandatory seam tests:

- **The two reconciliation anchors** (highest fan-in, highest drift-risk): the `EventEnvelope` field list +
  **units** (`00 §2.10` — timestamps RFC-3339 UTC, costs integer minor-units, TTLs/timers seconds, client
  timeouts ms, `pii_key_ref` grammar) and the `ArtifactRef` token table (Bus §6.2). A serialization round-trip
  + unit-assertion test on both, run on every change.
- **`list_objects` / `SetExpr` push-down (4.3)** — the single most load-bearing inter-system contract: a
  golden test per consumer id-column (git pr/repo, CI run, issue, KN database_row, chat channel/message) that
  the lowered JOIN admits exactly the visible set and **post-filter == pre-filter** (no leak, no N+1).
- **The Git↔CI `CheckStatus` seam (5.9)** — producer (CI emits `ci.check.updated` + `ci.result`) and consumer
  (Git projection + `run_attempt` supersession + fork-trust gating) tested against the frozen struct; the
  poisoned-pipeline property (an `untrusted_fork` success is neutral-for-gating until endorsed).
- **`project(ref, viewer)` (5.6)** — required on every subsystem; a contract test that every artifact type
  returns a per-viewer, pre-permission-checked projection (the only cross-subsystem read path).
- **`PersonalDataHolder` (10.1)** — every store implements locate/export/rectify/restrict/erase; a contract
  test that harness auto-registration covered it (the "we forgot a store" impossibility).
- **The cross-language harness shim (1.7)** — if any subsystem diverges from Rust (the Chat connection tier,
  TE-21), it satisfies the identical wire contract; the seam test is run against the non-Rust impl.

### 2.6 Level 6 — Per-service integration (one service + its real Postgres)

`serve(AppSpec)` (contract 1.1) booted against a **real Postgres** (testcontainers/ephemeral instance, never a
mock DB — STOR-3: the cache is never a source of truth, and a mocked DB hides the bugs that live in real SQL/
RLS/locking). Proves: migrations apply forward-only (1.5), the outbox relay drains, consumers bind-by-name and
ack-after-enqueue, RLS/tenant-predicate actually scopes rows, the three-surface topology enforces the
public/internal security boundary. One service, its real store, its real outbox — not yet the bus end-to-end.

### 2.7 Level 7 — Cross-subsystem E2E (chained mutations, the real UI)

The chained-mutation tier (T-6) and the switch test (T-7). Drives **multiple subsystems over one session**:
e.g. push to a repo → CI emits `ci.check.updated` → the merge-queue durable workflow waits on `ci.result` →
merge → `git.pr.merged` → a Signal → a Notif inbox item → mark-read consistency. Asserts state **between**
steps (mid-flight), not just final state. For user-facing surfaces, **drive the real UI in a browser** and
apply the frontend gates (T-8; DL §8b): measured contrast over the token table, latency budgets (keyboard
<~100ms, no spinner-flash <~1s, pages render not animate-in), overlays/popovers tested against the **real**
anchor (the off-screen-picker class of bug). Done = the switch test passes.

### 2.8 Level 8 — Load / chaos drills (the failure-injection harness; the 9 families)

The top of the pyramid: the drills (§1.1). The 101 Phase-3 drills collapse to **9 reusable families** (F1–F9)
plus ~7 unique drills (sandbox escape, erasure-reaches-everything, online-migration, audit-tamper, plus the
non-family per-system ones). Phase 5 runs a **family across all its owners with one harness and one scorecard
column** (the A.1/A.2 mapping). The drill catalogue (doc 01) enumerates each with its frozen threshold; this
document fixes only that **the drill is the unit of proof** and that **cheap drills run in CI on every change,
expensive ones run scheduled** (§3).

---

## Part 3 — The gate / ratchet model

A gate is a committed, mechanical check with a quantified pass condition. The ratchet (EI-01 P5) is the
discipline of **converting every quality habit into such a gate** — built from the *fingerprint of a recurring
failure* — so it cannot be skipped. This part fixes which gates run **on every change** vs **scheduled**, the
committed lints, the gate invariant, and the loud-never-swallowed rule.

### 3.1 Committed lints — the architecture gates that run on EVERY change (EI-01 P5; E-4/E-5)

These are the twelve frozen architecture lints (contract 1.6), each the mechanical embodiment of a bug-class
the platform must make *impossible*, not merely *tested*. **An uncommitted gate is no gate** — each is wired
into CI (and pre-commit where cheap), and a violation **fails the build loudly**.

| Lint (committed gate) | The bug-class it makes impossible | Quantified pass condition |
|---|---|---|
| **no-cross-db** | a service reaching into another service's database (the cross-DB coupling that breaks one-DB-per-service) | 0 cross-DB references at compile |
| **no-raw-publish** | a fire-and-forget emit bypassing the outbox (the lost-event source) | 0 publish outside `OutboxTx::emit` |
| **tenant-predicate** | a tenant-less query (the IDOR root) | 0 query without a tenant predicate at compile |
| **no-host-exec** | a host-execution path bypassing the sandbox tool trait (the RCE source) | 0 host-exec outside `ToolHands::exec` |
| **residency-pin** | an out-of-region write (cross-region PII egress) | 0 write where row.region ≠ cell.region |
| **no-llm-in-platform** | a model call baked into platform code (vs the strategy-seam runtime) | 0 LLM call in platform crates |
| **no-untagged-personal-data** | a personal-data field with no `#[personal_data]` tag (a hole in the data map / DSR fan-out) | build RED on any untagged PII field |
| **flow-determinism** | non-determinism in a workflow body that would diverge on replay (silent double-effect) | 0 non-journaled non-determinism in a `WfCtx` body |
| **search-requires-acl-filter** | a search query that scores before conjoining the `list_objects` Filter (the leak) | 0 query path that skips the ACL conjoin |
| **forward-only-migration** | a rollback migration or a blocking `ALTER` on a hot table | 0 down-migration; 0 blocking ALTER on a flagged hot table |
| **control-plane-pii-free** | a name/email written into a control-plane column (PII in the routing tier) | 0 `is_personal=true` column in the control-plane schema |
| **no-cross-sync-cycle** | a synchronous call cycle between services (the deadlock/cascade) | 0 sync cycle in the call graph |

Two more, beyond the twelve, are committed gates of the same status:
- **restore-verify wired into CI (ADR-18; STOR-D1):** the durability gate is not a doc — the restore +
  cross-seam-integrity check is a CI job, not an aspiration. An unwired restore-verify is no gate.
- **the mandatory-core mutation gate (§2.4):** path-scoped cargo-mutants on the agent loop / authz / outbox /
  reserve-settle / determinism / crypto-shred cores, RED below the threshold.

### 3.2 What runs on EVERY change (cheap, in CI) vs SCHEDULED (expensive drills)

The split follows cost, per EI-01 P3 ("cheap drills run in CI on every change; expensive ones run scheduled").

**On every change (the CI gate — must be green to merge):**
- All twelve architecture lints + clippy + `cargo build` (Level 1).
- Unit + proptest suites (Levels 2–3).
- Path-scoped mutation on the mandatory cores (Level 4) — only when those crates changed.
- Contract/seam tests (Level 5) — the anchor round-trips, `SetExpr`, `CheckStatus`, `project`, holder.
- Per-service integration against ephemeral Postgres (Level 6).
- The **cheap drills**: the deterministic, single-process, fault-injectable ones that fit a CI budget — the
  IDOR / leak-free assertions on a small adversarial corpus (F1/F2 in-process), the outbox kill-between-commit-
  and-publish (F5 single-node, BUS-D4/SUB-D1), the `#sub` tombstone ladder, the agent-loop determinism replay
  (AG-D9), the reindex-from-cold parity on a small scope (F4), the editor round-trip corpus (DL §8b.2).
- A smoke E2E (Level 7) chained-mutation path on the dogfood graph (§3.6).
- Frontend gates (T-8): measured-contrast + latency-budget checks on changed surfaces.

**Scheduled (nightly/weekly, or milestone-gated — the expensive drills):**
- **The 30× surge family (F6)** across all owners on the harness — needs a load generator and a prod-scale
  copy; asserts human-lane-holds / agent-lane-sheds / 0 cross-tenant impact.
- **Restore + RPO/RTO (STOR-D1/D2, F3)** — restore a prod-scale copy, measure RPO ≤ 5 min / RTO ≤ 1h-tenant
  /4h-cell, assert cross-seam consistency. The headline durability drill.
- **The sandbox-escape drill on a real kernel (AG-D4)** — the single hard go/no-go; a **milestone gate**
  before any untrusted customer code runs (CI or agent), re-run on any change to the runner/hardening profile.
- **Online-migration safety (STOR-D8/SUB-D10)** — expand→backfill→contract on a restored prod-scale copy under
  load, assert no blocking lock beyond budget, 0 downtime.
- **Full reindex-from-cold parity at scale (F4)**, full erasure-reaches-every-holder (GA-D1), audit-tamper
  (GA-D3), multi-cell FLOOR drills (when their follow-on is built).
- **Fail-static / Id-hiccup (F7), loop/runaway (F9) at scale, the multi-day HITL** (FLOW-D4, spanning a worker
  restart + deploy).

**Every real incident ends by adding a drill that reproduces it (EI-01 P3).** The drill set grows; it is never
pruned to make CI faster (a pruned drill is a re-opened door).

### 3.3 The gate invariant — no later phase done over a red earlier gate (R-2; EI-01 P2)

**No later phase may be claimed done while an earlier phase's gate is still red.** The ordering (R-1) is
enforced, not aspirational: the stop-the-bleeding gates (sandbox-escape AG-D4; the data-loss gates STOR-D1 /
F5 / F3) are **milestone gates** that block downstream phases in the roadmap. A feature surface whose substrate
gate is red is **not done**, regardless of how complete the surface looks (EI-01 P2: "you build a beautiful
feature surface on top of a substrate that silently corrupts, and discover it the day a real tenant loses real
data"). The scorecard (doc 02) makes the invariant checkable: a green feature row over a red substrate row is a
contradiction the truth-up pass flags.

### 3.4 Loud-never-swallowed (E-4; EI-01 P5)

Violations are **loud, never silently swallowed.** Concretely, banned in the codebase and enforced by a gate:
- **`|| true`** and equivalent "make-it-pass" suffixes on any gate/test/CI step.
- **Silent filters** that drop a contract violation (a malformed event quietly skipped instead of dead-
  lettered; a `Deny` swallowed into an empty list instead of surfaced).
- A drill that **passes by not failing** rather than by asserting the survival signal (§1.1): no-signal is a
  fail, so a "green" with no telemetry assertion is itself a loud lint failure in the drill harness.

A swallowed contract violation is a multi-day misdiagnosis waiting to happen (EI-01 P5); the gate that bans it
is cheap and runs on every change.

### 3.5 The no-threshold-weakening gate (enforcing §1.2)

Because "never weaken a threshold to go green" is the load-bearing honesty rule, it gets its own committed
gate: drill thresholds live in **one versioned thresholds file** (the Q32 binding set), changes to it require
an explicit reviewed PR (never an inline edit in a test), and a CI check fails if an assertion is **inverted**
or a `== 0` is **loosened** without a corresponding dated, signed-off thresholds-file change. A red gate stays
red and becomes a "claimed, not proven" scorecard row — it is never edited green.

### 3.6 Dogfooding as a testing lever — Myelin hosts itself, one CI graph (VISION §1; EI-01)

Myelin is built on Myelin: its own repositories live in Myelin git hosting, its own pipelines run on Myelin
CI, its issues/docs/chat are Myelin subsystems, and **there is one CI graph** the whole platform's development
flows through. This is the single cheapest and most honest testing lever:

- **The platform's own commits are the load generator.** Every dogfood push exercises the real
  commit→check→merge→notify chain on real data; the chained-mutation E2E (Level 7) runs against the dogfood
  graph on every change, so the cross-subsystem seams are exercised continuously, not only in scheduled drills.
- **The gates run on the platform that hosts the gates.** The ratchet lints, the restore-verify, the mutation
  cores all run *in Myelin CI on Myelin's repos* — a regression in a gate is felt immediately by the team that
  owns it (eat-your-own-dogfood: a broken Notif humanisation shows up in *our* inbox).
- **The dogfood tenant is the first drill target.** The 30× surge, the fail-static hiccup, the restore drill
  run first against the dogfood tenant on the harness — a real tenant whose data the team cares about, so "0
  data loss" is felt, not abstract. (Residency and tenant-isolation drills still use synthetic adversarial
  tenants — you do not run a cross-tenant IDOR drill that could touch real dogfood PII; F2 uses seeded
  adversarial corpora.)
- **One CI graph means one place the gate invariant (§3.3) is visible** — a red substrate gate blocks the
  dogfood graph, so a later-phase feature cannot quietly ship over it.

The dogfood graph does **not** replace the failure-injection harness (you cannot wait for a real reconnect to
test reconnect-loses-zero-ops); it complements it by keeping the real-UI / chained-mutation / one-graph
exercise continuous and free.

---

## Part 4 — How this folder is organised (the rest of the testing strategy)

This document is the spine. The companions (named here so the structure is explicit; this doc does not write
them):

- **01 — The drill catalogue.** The 101 Phase-3 drills lifted, de-duplicated into the 9 families + ~7 unique,
  each with its **frozen Phase-5 threshold** (resolving Q32), its owner, the survival signals it reads (A.3),
  and its CI-vs-scheduled placement (§3.2). The one place every drill's number lives.
- **02 — The source-verified scorecard.** The claimed-vs-proven matrix (T-4), one row per capability, the gap
  report (E-3) integrated, the gate-invariant view (§3.3).
- **03 — The failure-injection harness spec.** The load generator (1×/10×/30×, mixed `actor.kind`), the
  scoped-reversible dependency-break primitives, the telemetry-assertion library (reads contract 1.8), and the
  per-family harness so one drill family runs across all owners.
- (Frontend QA — measured contrast, latency budgets, overlay-against-real-anchor, the switch test — folds into
  DL §8b and the E2E level; called out as gates here, detailed there.)

---

## Part 5 — Cross-references

- Philosophy source: [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
  (P1 code-wins, P2 order-by-non-negotiability, P3 prove-it + harness, P4 actually-try-it, P5 the ratchet,
  P6 investigate-before-build, P7 coherence, P8 human-sign-off); the hard problems
  [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) (§4 the
  scorecard/floor discipline; §1/§5 erasure, sandbox, reindex-from-source).
- Binding directives: [`../../02b-doctrine-integration/integration-directives.md`](../../02b-doctrine-integration/integration-directives.md)
  (T-1..T-9 Phase-5; E-1..E-9 Phase-8; R-1..R-6 Phase-6; the named lints E-5).
- The drill inventory consolidated here: [`../../03-shared-systems-architecture/drills-and-open-questions.md`](../../03-shared-systems-architecture/drills-and-open-questions.md)
  (Part A families F1–F9 + A.2 per-system table + A.3 survival signals; Part B Q32/Q33 thresholds; Q34 W).
- Frozen build-to surface (what the drills test): [`../contract-index.md`](../contract-index.md) +
  [`../00-reconciliation-decisions.md`](../00-reconciliation-decisions.md) (X-1 CheckStatus seam, X-6 unified
  sandbox + four uniform guarantees, X-7 erasure posture, OQ-E SetExpr push-down, OQ-J resume-cursor, OQ-K
  shed budgets).
- Spine: [`../../02-holistic-architecture/architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md)
  (ADR-16 backpressure/human-lane, ADR-17 fail-static, ADR-18 backup/restore-verify, ADR-19 four-primitive,
  ADR-20 one sandbox); design-QA gates [`../../02-holistic-architecture/design-language.md`](../../02-holistic-architecture/design-language.md)
  §8b (render(parse(md))===md, measured contrast, latency budgets, the switch test).
- The quality-bar signal: repo `.gitignore` pre-seeds `**/mutants.out*/` (cargo-mutants) — mutation testing is
  the expected bar, mandatory on the cores in §2.4.
