# Phase 2b — Doctrine Integration

> Inserted between Phase 2 (holistic architecture) and Phase 3 (shared-systems architecture).
> Canonical brief: [`VISION.md`](../../VISION.md). Doctrine source:
> [`external-insights/`](../../external-insights/) (canonical, default-you-follow-unless-you-write-down-why).

## What this phase did

The user supplied [`external-insights/`](../../external-insights/) — hard-won engineering doctrine for
a world-scale, multi-tenant, agent-native developer platform of exactly Myelin's shape — with canonical
status equal to VISION. This phase decided **what** from the doctrine to integrate, **where** it binds,
and **how** — folding it into the spine *without rework churn*. The finding: the substrate doctrine
**overwhelmingly confirms** the Phase-2 14-ADR spine (same prior art), with **no genuine conflicts**.
Energy went to a handful of real deltas (committed as ADR-16…ADR-20 + design-language §8b) and to
**routing** every non-confirming item to the phase where it binds (the binding directives).

## The artifacts of this phase

- [`analysis/`](./analysis/) — five per-doc integration analyses (the inputs this phase consolidated):
  - [`process-quality.md`](./analysis/process-quality.md) — `external-insights/01`
  - [`platform-substrate.md`](./analysis/platform-substrate.md) — `external-insights/02`
  - [`agent-native-fabric.md`](./analysis/agent-native-fabric.md) — `external-insights/03`
  - [`hard-problems.md`](./analysis/hard-problems.md) — `external-insights/04`
  - [`ux-design.md`](./analysis/ux-design.md) — `external-insights/05`
- [`decision-record.md`](./decision-record.md) — **the consolidated decision**: the degree of
  convergence, the confirmations table, the deltas we adopt (with where each binds + rationale), the
  conflicts (none) and their resolution, the stronger priors we carry as defaults-to-beat, and what we
  deliberately do not change.
- [`integration-directives.md`](./integration-directives.md) — **the binding hand-off**: per-destination
  directives (Phase 3 per shared system, Phase 4 per subsystem, Phase 5 testing, Phase 6 roadmap, Phase 8
  execution, Legal/DPO, Commercial), each a crisp imperative + the external-insights citation + the
  adopted default. This is what makes the doctrine actually bite.

## Files this phase edited (outside `02b-`)

| File | Edit |
|---|---|
| [`VISION.md`](../../VISION.md) | §3: added the "name your floors; code wins over docs" honesty clause. §4: added **external-insights as canonical doctrine** (default-you-follow; read the relevant doc before each phase). §5: inserted **phase 2b** into the process list. |
| [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md) | Appended **ADR-16** (backpressure + protected human lane + shed order), **ADR-17** (fail-static vs fail-closed), **ADR-18** (backup/restore-verification gate), **ADR-19** (Event/Signal/Automation-rule/Trigger), **ADR-20** (ONE sandbox for CI+agents, resolves TE-31). Updated the ADR index; added a "Resolved/sharpened by Phase 2b" block + ADR-08.6 generalisation note to the ADR-15 carry-forward. No existing decision altered. |
| [`design-language.md`](../02-holistic-architecture/design-language.md) | Appended **§8b — Day-one UX primitives (external-insights doc 05)**: overlay primitives (portal-always, z-scale, focus-trap/scroll-lock/ARIA in the primitive, single-purpose-by-shape); one editor render path + `render(parse(md))===md` gate + markdown-subset string; measured-not-claimed tokens (focus≠identity, status-not-colour-alone, no-inline-colour); the layout-containment/mobile bug checklist; backend humanisation; the switch test; "build these first." |

## The deltas adopted (one line each)

1. Backpressure + protected human lane + shed order (speculative→batch/CI→agent→human-last) — **ADR-16**.
2. Fail-static (bounded-staleness) vs fail-closed on the Id hot path — **ADR-17**.
3. Backup/restore-verification + cross-seam (row↔blob↔index↔offset) integrity as a CI durability gate — **ADR-18**.
4. The Event / Signal / Automation-rule / Trigger four-primitive model ("Trigger" disambiguated) — **ADR-19**.
5. ONE hardened sandbox for CI + agents, one job spec `kind ∈ {ci,agent}` — **ADR-20** (resolves TE-31).
6. Brain (stateless `step`) + hands (`exec`, no-bypass) boundary + skeleton mode — directives AG-1…AG-3.
7. Orchestrator gotchas (whitelist+lag, bind-by-name, ack-after-enqueue, nested causality) — directives BUS-3…BUS-5.
8. Universal reserve/settle cost gate before every run (CI included) — directive CI-2 (generalises ADR-08.6).
9. Process/quality doctrine (prove-it, the ratchet, name-your-floors, code-wins, drive-the-real-UI, order-by-non-negotiability) — VISION §3 + Phase 5/6/8 directives.
10. Day-one UX primitives — design-language §8b.
11. Resolved open questions: TE-7 (typed relation table), TE-31 (one sandbox), TE-15 (CAS-floor→CRDT + resume-cursor-transport-first), TE-16/18 (markdown-subset string + read-time rollups), reindex-from-source.

## Hand-off to Phase 3

Phase 3 (`planning/03-shared-systems-architecture`) **must** read this phase's
[`decision-record.md`](./decision-record.md) and [`integration-directives.md`](./integration-directives.md)
alongside the Phase-2 ADRs (now including ADR-16…ADR-20). The directives under **Phase 3** in
`integration-directives.md` are binding per-shared-system inputs (Identity, Bus, Refs, Search,
Notifications, Agent Fabric, Storage, GDPR/Audit), and the cross-cutting X-1…X-5 (telemetry contract,
bootstrap harness + three-surface topology, bounded everything, stateful-component register, pre-ship
contract reconciliation) apply to every shared system. The newly-resolved open questions (TE-7, AG-3,
reindex-from-source, the durability/fail-static/backpressure decisions) enter Phase 3 with a concrete
default-to-beat: adopt it, or write down why you deviated.
