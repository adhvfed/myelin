# W6b2 Design — CostLedger durable re-point (closes the W6b honest-STOP)

Date: 2026-07-15. Read-only grounding pass (Opus). Removes the `reserve_settle.rs:283` baseline
entry (−1) that W6b left SUPPLEMENTED. Standalone wave: after W6, before W7/CT-004b.

## Grounded facts (file:line in the agent report; key ones re-cited here)

- **BudgetGate is per-run and NOT wired into any live boot.** `BudgetGate::new(wallet)` constructs
  `CostLedger::new()` inline (`myelin-flow/src/budget.rs:222–230`); the intended consumer chain
  (`FlowDispatcher::with_budget` → `WfCtx::with_budget`) has ZERO production callers, and
  `Wallet::from_budget` likewise. ci-controlplane's `CiMeter::new(&gate, markup)` BORROWS the gate
  (builds no ledger, "CI builds NO second ledger", ci lib.rs:210); its only ledger constructions are
  two drill pub-fns (`metering.rs:489`, `e2e_flagship.rs:356`) invoked only from tests.
- Both flow and ci-controlplane already depend on myelin-storage (default) — `SubstrateProvider`
  reachable without new edges. CT-004's durable-CI work targets a DIFFERENT surface (pipeline/run/
  step/scheduler SQL), not this construction site.
- The W6b sibling `ErasureLedger` role-struct (`restore_verify.rs:184`) is the exact template.
- **Keep `&mut self`** — BudgetGate already wraps the ledger in `Arc<Mutex<GateInner>>`; the durable
  arm's methods are `&self` (Clone, state in PG) and a `&mut self` wrapper can call `&self`. The
  W2-style `&self` conversion W6b deferred is NOT needed (that was for `&'a` holders like
  GateInputs). Accept the cosmetic inconsistency with ErasureLedger.
- Known ripples: `cost_events_for` unifies to OWNED `Vec<CostEvent>` (arms differ today);
  `CostEvent.unit: &'static str` → `String` (kills the `Box::leak` at reserve_settle_durable.rs:533;
  `MeteredUnit.unit` STAYS `&'static str`); the two drill pub-fns get test-support-gated (W5
  precedent); the 34/34 mutation floor at reserve_settle.rs:562–1089 must survive by moving the
  arithmetic/cap/idempotent logic INTACT into the Memory arm.
- **Store-fault semantics: keep fail-static panic.** Grounded as safe for billing: callers map
  BudgetError → WfError::CoCommit → the durable executor re-leases and re-drives; reserve is
  duplicate-key-guarded, settle is idempotent via SQL re-read; a rolled-back settle leaves no
  cost_event row so the re-drive settles fresh — never a double charge. A retryable
  `StoreUnavailable` channel is a named FOLLOW-UP (touches frozen enums + WfError retry
  classification + mutation re-prove), not this wave.

## Execution steps (one builder prompt each)

**Step 1 — storage reshape (THE RISKIEST STEP):** `CostLedger { backend: CostBackend }` with
`Memory[test-support](MemoryCostLedger — the existing HashMap/Vec/counter + ALL invariant logic
moved intact)` | `Durable(DurableCostLedger)`; `new()` gated; `with_pg(provider)` added; `&mut self`
kept, per-method dispatch; `cost_events_for` → owned; `unit` → String + delete Box::leak; re-prove
the mutation floor (≥80% non-equivalent). Riskiest because the frozen billing invariants + the
return-type + unit ripples move simultaneously — a regression here is silent-billing-shaped.

**Step 2 — flow + ci re-point:** test-support features + self dev-deps on myelin-flow /
myelin-ci-controlplane; `BudgetGate::new(wallet)` gated test-support; add
`BudgetGate::new_durable(wallet, ledger)` + `with_pg(wallet, provider)`; gate the two drills;
live-PG integration test driving reserve→begin→settle through the DURABLE BudgetGate arm.
Proof: scanner −1 (`reserve_settle.rs:283` out), DB-free workspace green, integration green.

## Out of scope (named)

Wiring BudgetGate into a live FlowDispatcher/boot (no production caller exists — a consumer wave);
the retryable store-fault channel; Wallet durability (P-ST-19 Commercial balance); `&self`
CostLedger conversion; CT-004b CI scheduler/lease SQL (W7); region sweep + scanner widening (W7).
