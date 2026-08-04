# Hosted agent — real vs. stubbed

The metered, tenant-isolated, tool-executing agent works end-to-end and is
live-verified (real Luna reads real tenant data via a governed tool, billed).
What's not yet real:

## Billing — unify the wallet
Today: a *dedicated* micro-dollar agent wallet, separate from CI's cent ledger.
Wanted: **one unified wallet per tenant** covering CI + agents, in one micro-dollar
unit, with **spend limits** — global, per-surface (CI vs agent), and per-project.
Work: rescale the shared `MinorUnits` ledger to micro-dollars (touches CI billing,
accepted), fold the agent wallet into it, add the limits model. Done = one org
balance a user can cap per project/surface.

## Tools — derive from use cases
Today: one read tool (`git.read_check_status`) as the mechanism proof.
Wanted: a tool set that flows from what real users need agents to *do*. Needs a
use-case pass first, then the executors (read + mutate-via-EffectApi). Mutate
executors need a real EffectApi construction (today only mock pipelines in tests).

## Identity — production config
Real mint + durable S7 revocation are wired and live-verified. But the drill
self-supplies `cell_id` + a dev seal key; a production service main must source
`MYELIN_CELL_ID` / `MYELIN_KMS_SEAL_KEY` from env (same as edge/CI).

## Compute tools — the Tier-2 sandbox
Read tools need no sandbox. Compute tools (run tests, build) need the warm
session workspace on gVisor + a deny-by-default egress allowlist proxy (does not
exist). See `planning/08-release/04-hosted-agent-product-plan.md` §5.

## Loop follow-ons
- Max-turns leaves the reservation in-flight (mirrors a kill); with the unified
  wallet it should settle-to-usage + release on graceful exhaustion.
- Cap = a coarse `balance > 0` pre-step gate + per-turn debit (overspend ≤ 1 turn);
  a per-call `max_tokens` estimate would tighten it.
