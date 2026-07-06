# Phase 08 — Release & Commercialization Plan

_Written 2026-07-06, against HEAD `d1cacb8`, immediately after the 2026-07-06 full-platform review
(24 units, 85 findings) and its post-CT/MR-009b delta. This phase turns "strong substrate, unfinished
seam" into a released, commercially viable product, driven by a solo founder._

## The one-paragraph thesis

Myelin's backend reviews as genuinely strong; every critical lives at the seam where the product meets
the user (Git/PR UX) and at the last mile of authorization (object-level authz, live sandbox egress).
The path to release is therefore **not** a rewrite or a broadening — it is: close the live security
holes, finish the durable un-gating already in flight (MR-009b), complete the flagship Git+CI surface,
cut the founder's own daily work over to Myelin, and only then put other people's data on it behind the
graduation gate. Commercially, the wedge is **EU-sovereign Git+CI for regulated European teams**, with
agent-nativeness as the differentiator — sold open-core, hosted in the EU, funded by bootstrapping plus
EU digital-commons grants whose applications start now because their lead times are long.

## Documents

| Doc | What it decides |
|---|---|
| [01-technical-release-plan.md](01-technical-release-plan.md) | The R-track: phases R0–R6 from HEAD to GA, with entry/exit gates, mapped 1:1 to review findings and existing ledgers. Defines the three release tiers (Dogfood → Design-Partner Beta → GA). |
| [02-commercial-plan.md](02-commercial-plan.md) | Positioning, ICP, licensing (decision: open-core, FSL), pricing, GTM sequence, funding, solo-dev operations, and the numeric viability gates. |

## The three release tiers (shared vocabulary)

- **Tier D — Dogfood** (founder's own data only): Myelin hosts Myelin's git + CI. Gate: R0–R3 complete.
- **Tier B — Design-partner beta** (3–5 friendly teams, free, under agreement): first external data.
  Gate: R4–R5 complete + the graduation subset marked `B` in R6.
- **Tier GA — Paid general availability**: Gate: all of R6, including external pentest and legal pack.

## Standing process

Execution reuses the proven MR/GT/CT orchestration method verbatim (one builder subagent per prompt,
anti-duplication grep, orchestrator runs the full cargo gate, **independent adversarial verifier on every
security-load-bearing prompt**, evidence over assertion, commit per prompt). New ledger:
`planning/system-reviews/2026-06-26/14-release-track-ledger.md` is created when R0 execution starts.
