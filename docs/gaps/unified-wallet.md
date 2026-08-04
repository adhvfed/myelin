# Unified wallet — design of record

One prepaid balance per tenant, covering CI **and** agents, in one money unit, with
spend limits. This closes the billing gap that blocks a paid public launch. Money is
the correctness bar here: get it wrong and a tenant is over- or under-charged.

## Where we are today (verified map)

Three physical money tables, **two scales**, and only a partial surface dimension:

| store | unit | RLS | surface dim |
|---|---|---|---|
| `agent_wallet` / `agent_wallet_ledger` (mig 0080) | micro-dollars (`MicroUsd`) | FORCE-RLS | no |
| storage `cost_reservation` / `cost_event` (mig 0050) | cents (`MinorUnits`) | FORCE-RLS | no |
| CI `ci_cost_event` (mig ci_0014) | cents | **none** (app predicates only) | `kind ∈ {ci,agent}` |

`1 cent = 10,000 micro-dollars`. `MinorUnits` and `MicroUsd` are both `pub struct X(pub u64)`
with no conversion between them.

Two independent pricing functions at two scales: agent `metering::price` → `MicroUsd`;
CI `ci_pipeline_driver` → cents (1¢ per cpu-second / gb-second).

**The load-bearing fact:** the reserve/settle cost gate carries **no live money**. Its
`available` balance is a hardcoded literal everywhere — `MinorUnits(100)` in
`agent-host/src/lib.rs:94`, RunBudget constants in flow/CI — and the CI reserve path
(`CiMeter::reserve_budget`) has no production caller. Real agent money moves only through
the *separate* `AgentWallet.debit` per-turn path (`skeleton.rs:853`), already in
micro-dollars. So there is no dangerous live-money migration to perform: we are building
the real wiring, at one scale, where hardcoded placeholders stand today.

## Target

- **One money type, one scale: `MicroUsd` (micro-dollars), system-wide.** The agent side
  and pricing are already there; the cost ledger and CI move to it (×10,000). `MinorUnits`
  is deleted. Sub-cent precision is required — a ~2% cut on a sub-cent Luna task rounds to
  zero at cent scale.
- **One wallet per tenant.** Generalize `agent_wallet` into *the* tenant prepaid balance.
  Both agent and CI reserve/settle consume `wallet.available(tenant)` — replacing the
  hardcoded `available`. Keep the immutable-ledger + `balance == Σ ledger` + FORCE-RLS
  invariants the agent wallet already proves.
- **A `surface ∈ {ci, agent}` dimension** on the cost ledger and wallet ledger, so spend is
  attributable per surface (for limits + reporting). `ci_cost_event` already has `kind`;
  fold it into the unified ledger and give it the FORCE-RLS the others have.
- **A limits model.** A `wallet_limit(tenant, scope, amount_micro, period)` table where
  `scope ∈ {global, surface:ci, surface:agent, project:<id>}`, checked at reserve time
  *in addition to* the balance. A limit is a spend ceiling; the prepaid balance is the hard
  floor. This is greenfield — no spend-cap concept exists today.

## Slice plan (each: delegate build → adversarial review → live-PG verify → commit)

1. **Unify the type** — merge `MinorUnits` into `MicroUsd` (one type, canonical home in
   `myelin-storage`), delete `MinorUnits`, mechanical rename across the 8 crates. **No scale
   change** — values stay numerically identical, so it is behaviour-preserving and the
   ledger-arithmetic tests stay green with unchanged assertions (they test scale-invariant
   math: `reserve(120) − settle(90) = refund(30)` holds in any unit). Large churn but zero
   money judgment — the full test suite is the oracle. Adversarial review confirms no
   semantic drift (nothing that read a value *as cents* — a `/100`, a "$" render, a
   cross-type interaction).
2. **Correct the real-world values** — the only diff that changes what money *means*, kept
   tiny and isolated: the CI pricing rule 1¢ → 10,000µ per cpu-/gb-second
   (`ci_pipeline_driver.rs:813-815`), the unwired placeholders (`agent-host` `estimate`
   `MinorUnits(10)`→`MicroUsd(100_000)`, `available` `100`→`1_000_000`; RunBudget defaults;
   dogfood constants), and the handful of tests that assert a real dollar value (pricing
   tests, not the arithmetic tests). Money-critical but small → careful review + live-PG.
   (Between slices 1 and 2 the CI pricing rule is nominally 10,000× cheap, but the path is
   unwired — no caller, no real money — so the interim is harmless pre-launch.)
3. **Wire the real agent balance feed into the reserve gate** (no schema change). Today the
   agent reserve/settle gate reserves against a hardcoded `available: MicroUsd(1_000_000)`
   (`agent-host/src/lib.rs:94`), while real money moves only through the separate per-turn
   `wallet.debit`. This slice makes the gate a real "no balance → no run" gate: `available =
   wallet.balance(tenant) − ledger.outstanding_reservations(tenant)` (saturating), so a tenant
   who can't afford the estimate can't dispatch and concurrent runs can't over-reserve past the
   balance. Needs a new `CostLedger::outstanding_reservations(tenant)` (Σ `reserved` where state
   ∈ {Reserved, InFlight}) on both the in-memory and durable-PG arms — `cost_reservation`
   already has the columns, so NO migration. When no wallet is present (skeleton/mock/drill),
   keep the nominal literal (byte-identical). The per-turn debit is unchanged (it is the actual
   billing; the gate is the affordability check — no double-charge, since reserve never touches
   the wallet). Gap: `estimate` stays nominal for now (a usage-derived estimate is a follow-on).
4. **Wire CI reserve/settle to the wallet**; fold `ci_cost_event` into the unified,
   FORCE-RLS cost ledger; **add the `surface ∈ {ci, agent}` column** to the wallet ledger
   (forward-only migration) now that both surfaces debit the one wallet. **Also owes (from the slice-2b review):** (a) a v2 reservation
   `reserved ≥ price` regression test — slice 2b added the v1 one; v2's multi-execution
   aggregate pricing is manually-verified-safe but unguarded; (b) enforce the whole-GiB
   per-execution memory precondition (documented at `operational_amount_from_ceiling`) or
   `div_ceil` per execution before aggregating, so a future non-whole-GiB limit can't reopen a
   small under-reserve.

### Slice 2b (done out of order — a correctness fix to slice 2)
The slice-2 adversarial review caught a latent bug: slice 2 rescaled the Tier-P *price* to
micro-dollars but left the reservation ceiling (`operational_reservation_amount`, v1 + v2) in
raw resource-seconds. The durable settle caps `billed` at `reserved`
(`reserve_settle_durable.rs:629`), so `billed (≈reserved×10,000) > reserved` silently
under-bills 10,000× — masked because the balance invariant still holds. Inert today (CI
reserve path unwired, pre-cutover, nothing issued) but a live-on-activation regression, so
fixed immediately rather than deferred, with the regression test that was missing.

**Design fork (decided: Option A).** The reservation amount is hashed into the *batch* digest
that forms the reservation handle (and flows transitively into the v3/v4 token-authority
handle), so rescaling it changes handle bytes.
- **Option A (chosen):** rescale the amount to micro-dollars everywhere; regenerate the
  affected pinned golden vectors. One money scale everywhere (the unification's whole point);
  safe because nothing is issued; not a crypto-logic change (hashing untouched, only the input
  value scales).
- **Option B (rejected):** keep hashing the raw amount (handles byte-frozen) but store a
  separately-derived micro-USD ceiling. Rejected — it reintroduces the capacity-vs-money
  duality the unification exists to remove, a future-confusion trap for slice-4 wiring.
5. **Limits model** — the `wallet_limit` table + a reserve-time check.

Ordering isolates risk: slice 1 is large but mechanically safe (pure rename, no scale
judgment); slice 2 is the only money-meaning change and its diff is tiny and enumerated;
slices 3–5 are additive wiring.

## Open sub-decisions (resolve as we build; flag any that are the founder's call)

- **Limit period semantics** — absolute vs. per-period (monthly reset). Lean: support both
  via a `period ∈ {total, monthly}` column, default `total` for v1 simplicity.
- **Limit vs. balance interaction** — a limit never *adds* spending power; it only caps
  below the balance. A reserve is refused if it would breach *either*.
- **Top-up** — internal/admin seed path for v1 (`AgentWallet::credit` with `Topup`); Stripe
  top-ups are a separate launch-surface slice (`launch-surface.md`).
- **Migration strategy** — pre-release, no live tenant balances to preserve, so the cost
  tables can be rescaled by forward-only migration without a data backfill. Confirm the
  dogfood DB has no cent-denominated rows that must survive.
