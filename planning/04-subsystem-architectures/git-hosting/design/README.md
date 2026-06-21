# Git Hosting — Design (IA / flows / states)

> The Stage-1 design sketch for this subsystem: the information architecture, the key user flows
> (including the agent/HITL and cross-subsystem flows), and ASCII wireframes of the primary screens with
> per-screen **happy / empty / loading / error / permission / erased / agent-pending** states. These feed
> the shared design language ([`design-language.md`](../../../02-holistic-architecture/design-language.md)
> §7 view catalogue, §8b day-one overlay primitives, §11 day-one UX mandates) and are the build-to for
> the Stage-2 architecture's view/CLI/API surface
> ([`../architecture/04-views-cli-and-api.md`](../architecture/04-views-cli-and-api.md)). Date: 2026-06-19.

## The files

| File | Covers |
|---|---|
| [`information-architecture.md`](./information-architecture.md) | Where git hosting sits in the one-shell rail ("Code"); the navigation tree; deep-linking + `ArtifactRef` granularity (the wedge substrate); the context-pane cross-subsystem composition (no cross-DB); persona-adaptive density; CLI as a co-equal view; mobile/responsive. |
| [`user-flows.md`](./user-flows.md) | The eight key flows — clone/fetch/push (the wire path); open-PR-review-merge (the centrepiece); request-an-agent-review (plan-then-apply, attribution); sensitive-agent-action → HITL approval card; the PR context pane (the wedge); code search (permission-pre-filtered); erasure/restriction (DSR fan-out, holder H1); ruleset edit. Each names the shared contracts it rides. |
| [`wireframes.md`](./wireframes.md) | ASCII wireframes of the eight primary screens (Repo home, File/blame, PR overview + context pane + agent surface, PR files-changed/diff+review, Code search, Ruleset editor, Agent/HITL approval card, Erasure/redaction admin), each with happy/empty/loading/error and the permission/erased/agent-pending states where they apply. Structural fidelity; the §8b day-one UX primitives applied throughout. |
| [`design-system-pass.md`](./design-system-pass.md) | **The P4 visual/token-level design-system pass** (GIT-P7 / P-233) over the structural sketch above, **including the X-1 affordances** (the fork-trust badge, the checks panel, the merge-queue affordances) keyed to the live contract-5.9 / recon-X-1 enums. Fixes the token-to-surface bindings, type/spacing/glyph maps, cross-cutting treatments, and the a11y constraints the value-table must clear. A sketch + sign-off, **not** frontend code (the frontend lands in GIT-P31). |
| [`signoff.md`](./signoff.md) | **The dated human sign-off** (2026-06-21) for the design-system pass, with the **decision-shaped fork-trust UX explicitly approved** (EI-01 §8). The sign-off is the green artifact for GIT-P7. |

## The primary screens (each owes a visual/token pass before frontend build)

Repo home · File/blame/history/compare · PR list · **PR detail (the centrepiece)** — overview /
files-changed / checks / **context pane** / **agent-aware review surface** · Ruleset / branch-protection
editor · Repo/org settings + fork/network + mirror (residency-gated) · **Erasure / redaction admin** ·
Insights (OLAP-fed).

**Design-before-code status (VISION §3, OQ-12) — DONE & SIGNED OFF (2026-06-21).** The IA, flows, and
per-screen state enumeration satisfy the VISION §3 mandate at *structural* fidelity; the
**visual/token-level design-system pass** ([`design-system-pass.md`](./design-system-pass.md)) now adds
the token-to-surface bindings, the type/spacing/glyph maps, and the X-1 affordances (fork-trust badge,
checks panel, merge-queue), and is **human-signed-off** ([`signoff.md`](./signoff.md), 2026-06-21) with
the decision-shaped fork-trust UX explicitly approved (EI-01 §8). The concrete token *value* table + the
live styleguide + the measured-contrast/inline-colour/round-trip lints remain a named floor that lands
with the frontend foundation in **GIT-P31**; no frontend code is built before that (design-language §9
OPEN→P4; OQ-12 in [`../architecture/07-drills-and-open-questions.md`](../architecture/07-drills-and-open-questions.md)).
