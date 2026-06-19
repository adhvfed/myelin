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

## The primary screens (each owes a visual/token pass before frontend build)

Repo home · File/blame/history/compare · PR list · **PR detail (the centrepiece)** — overview /
files-changed / checks / **context pane** / **agent-aware review surface** · Ruleset / branch-protection
editor · Repo/org settings + fork/network + mirror (residency-gated) · **Erasure / redaction admin** ·
Insights (OLAP-fed).

**Floor (VISION §3, named as OQ-12).** The IA, flows, and per-screen state enumeration (above) **satisfy**
the VISION §3 mandate "no frontend code without a design sketch behind it" at *structural* fidelity. The
remaining follow-on is the **visual/token-level design-system pass** on these wireframes (concrete
tokens/colours/spacing from design-language §3) before any frontend code — the P4-design build, named in
[`../architecture/07-drills-and-open-questions.md`](../architecture/07-drills-and-open-questions.md) §OQ-12.
