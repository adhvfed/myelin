# Phase 3 — Ready-to-Run Research Prompts (R-01 … R-22)

> Phase: `design-planning/03-research-prompts`. Operationalises the Phase-2 research roadmap
> ([`02-research-roadmap/README.md`](./02-research-roadmap/README.md) §3–§6, the ten `ws-*.md`
> workstream files, [`rubric.md`](./02-research-roadmap/rubric.md),
> [`sketch-funnel.md`](./02-research-roadmap/sketch-funnel.md)) into **22 self-contained agent prompts**
> that Phase 4 executes one at a time (the three foundational tracks in parallel), each agent learning
> from the outputs of the items before it.
>
> Status date: **2026-06-20**. Each prompt below produces exactly one deliverable file (occasionally a
> file with two clearly-marked parts) under `design-planning/04-research/<area>/`. The orchestrator runs
> them in the **execution order** in §C, feeding each agent the standing preamble (§A) plus its own
> per-item prompt (§B). **The orchestrator commits; agents never commit.**

---

## §A — Standing instructions preamble (given to EVERY research agent)

> Prepend this block verbatim to every R-NN prompt before dispatching the agent. The per-item prompts
> reference it rather than repeat it.

**You are a design-UX researcher for Myelin** — an EU-sovereign, agent-native software-delivery platform
unifying git · CI · issues · knowledge · chat. You are building **one file of a reusable research
corpus**. That corpus is the shared evidence base for the rest of the pipeline:

- **Phase 5** maps the user-facing surfaces over the design-language §7 view catalogue.
- **Phase 6** runs the two-stage sketch funnel ([`sketch-funnel.md`](./02-research-roadmap/sketch-funnel.md)):
  16–20 single-screen concepts scattered across named axes → cull to 3–4 finalists for **merit AND spread**
  → deepen each into a 3–5-screen mini-system with DTCG tokens, hard-gate demos, and the unglamorous states.
- **Phase 7** judges the finalists against the **pre-registered, binding** rubric
  ([`rubric.md`](./02-research-roadmap/rubric.md)).
- **Phase 8** picks the visual framework.
- **A single human review at the end** decides and may override the recommended direction.

**The driving directive is a maximally LOVABLE product** — not merely tolerable. Lovability is treated as
*testable* properties: speed, calm, coherence, trust, approachability, craft (design-language P1–P9). The
**central design problem** you serve throughout is **"one product, five surfaces"**: git, CI, issues,
knowledge, and chat must feel like ONE system (one shell, one identity, one palette, one reference chip,
one editor, one views component) while allowing appropriate per-surface density — the
**unification-vs-distinctness tension**.

### Canon you MUST read first (read ALL of these, then build ON them)
- [`VISION.md`](../VISION.md) (§3 non-negotiables: top-tier UX, agent-native, GDPR/EU-sovereign,
  design-before-code; the honesty rule).
- [`planning/02-holistic-architecture/design-language.md`](../planning/02-holistic-architecture/design-language.md)
  — **the mature design language**: P1–P9, §2 dual-audience, §3 tokens, §4 a11y/i18n, §5 shared
  components, §6 agent UX contract, §7 view catalogue, §8b day-one primitives. **Build ON this; never
  re-derive P1–P9 / §7 / §8b.** If you find yourself restating a principle, stop and instead *apply* it.
- [`external-insights/05-ux-and-design.md`](../external-insights/05-ux-and-design.md) (binding UX doctrine).
- [`planning/01-research/personas.md`](../planning/01-research/personas.md) (P1–P15, A1–A5 — **HYPOTHESES,
  no real users**; three clusters: engineers P1–P5, PM/delivery P6–P10, corporate/governance P11–P15).
- [`planning/01-research/competitive-landscape.md`](../planning/01-research/competitive-landscape.md)
  (North Stars + traps, the steal/avoid lists).
- [`planning/01-research/agent-native-design.md`](../planning/01-research/agent-native-design.md).
- Your own item's workstream file under
  [`02-research-roadmap/`](./02-research-roadmap/) and the roadmap README §9 completeness-critic.

### Honesty rule (VISION §3; non-negotiable)
Tag **every claim** either **PROVEN** (cite the standard / source / measured evidence) or **HOUSE STYLE**
(our taste / synthesis). **Date the file** (today is 2026-06-20). Name your uncertainties explicitly; a
claim that masquerades as settled when it is taste is a failure.

### No-user constraint + deferred handling
**No real users exist.** Personas P1–P15 are hypotheses. For any item or sub-part flagged
**deferred-until-users**: (1) fully execute the **no-user substitute now** (expert teardown / heuristic
eval / cognitive walkthrough / blueprint / per-lens critique — as the workstream specifies); AND (2) write
the deferred validation as a **concrete, executable plan** — *what* to test, *with whom* (which
persona/segment), and *what would falsify our hypothesis* — clearly tagged **`[DEFERRED-UNTIL-USERS]`**.
**Never present a no-user substitute as validated.**

### Web research
Load web tools with `ToolSearch select:WebSearch,WebFetch`. Ground all teardowns, standards, and pattern
claims in **real, current (2024–2026) sources** and **cite URLs**. Verify; do not assert from memory.
Agent/AI product features move fast — date and `[VERIFY]`-flag time-sensitive observations.

### Output discipline
- Write **ONLY your item's deliverable file(s)** at the **exact path** named in your prompt. Do not write
  other files, scaffolding, or notes-to-self.
- Be **concrete and concise** — no filler, no marketing prose. Specs, tables, named patterns, cited facts.
- End the file with a short **self-check** restating your acceptance criteria and confirming each is met
  (or honestly noting where it is partial/deferred).
- Address the **completeness-critic §9 gloss-risks** relevant to your item: name which apply, and either
  cover them or consciously defer them with a reason.

### Awareness of the downstream control artifacts
Where your research feeds sketches, make it **actionable toward**:
- [`rubric.md`](./02-research-roadmap/rubric.md) — the **hard gates G1 (accessibility, WCAG 2.1 AA / EN
  301 549 floor; 2.2 AA house target) and G2 (i18n/l10n/RTL)** and the **10 scored dimensions D1–D10**.
  Call out which gate/dimension your output equips, and make it *checkable*, not aspirational.
- [`sketch-funnel.md`](./02-research-roadmap/sketch-funnel.md) — the **6 axes of variation** (density;
  navigation paradigm; surface unification; emotional tone; agent presence; sovereignty visibility) and
  the **comparable screen set** every finalist must include. Where your item maps to an axis, say so.

### Build on prior outputs
Before starting, **read the prior `04-research/...` files listed in your prompt's "Reads".** Reference and
**extend** them; do not duplicate their content. The corpus is cumulative.

### Do NOT commit
The orchestrator handles all git. Do not run `git add/commit/push`.

---

## §B — The 22 research prompts (in execution order)

> Foundational parallel band first (R-01→R-02 teardowns ∥ R-03→R-04 JTBD→flows ∥ R-11 visual direction),
> then the sequential middle and late bands. The `Seq #` is the roadmap §5 numbering; the run order is in §C.

---

### Prompt R-01 — North-Star teardown dossier (Linear · Notion · Slack · GitHub)  (Seq #1, effort L, user-dep: none, parallel-group: FOUNDATIONAL)

**Reads.** Standing preamble (§A). Canon: `competitive-landscape.md` §1–§5 (named North Stars + steal
lists), design-language §5 (shared components each teardown maps onto), §7 (view catalogue), `VISION.md`
§1. No prior `04-research` dependency (foundational).

**Task.** Apply Phase-1 **method #2 (comparative/competitive teardown)**, with **#19 heuristics** as the
"why it works" lens. Do a hands-on, screen-by-screen teardown of each North Star, mapped to the §7
catalogue and the §5 shared components. Use WebSearch/WebFetch to ground current behaviour and cite URLs.
Cover at minimum:
- **Linear** → command palette; issue board / triage / cycles; the speed/optimistic mechanics (what
  exactly makes it feel instant and keyboard-native).
- **Notion** → block editor; the database/views primitive (the §5.6 issues↔knowledge reuse boundary);
  slash menu; mentions.
- **Slack** → unfurl card; slash-commands; threading — **plus the Zulip topic model as a contrast** for
  agent volume.
- **GitHub** → PR overview; diff / files-changed; batched review; Checks API surfacing (the bar Myelin
  must meet on code review).

For **each entry** use the structure: *pattern → why it works (evidenced/cited) → how Myelin adapts it to
which principle (P1–P9) → the trap hiding inside the pattern.* Every "steal" must be paired with the Myelin
principle it serves (not "they do it").

**Deliverable.** `design-planning/04-research/north-star/teardown-dossier.md`. Screen-by-screen per North
Star, mapped to §7 + §5 as above; date the dossier; `[VERIFY]`-flag time-sensitive agent/AI features.

**Acceptance criteria (self-check).** Every §5 shared component has ≥1 North-Star teardown entry behind
it; every "steal" is paired with the Myelin principle it must serve; time-sensitive agent/AI features are
dated and `[VERIFY]`-flagged; the dossier reads as a Phase-7 "meets/beats the North Star or regresses"
baseline.

---

### Prompt R-02 — Trap / anti-pattern audit (Jira · Atlassian · Teams)  (Seq #2, effort M, user-dep: none, parallel-group: FOUNDATIONAL — runs right after R-01)

**Reads.** Standing preamble. Canon: `competitive-landscape.md` §3/§6 (the traps), §6.1 (the
"stitched-together" failure), design-language §2 (dual-audience compromise trap), P4 (progressive
disclosure), P8 (calm). Prior: `04-research/north-star/teardown-dossier.md` (R-01 — shares the
teardown method/format; reuse it, don't re-derive it).

**Task.** Apply **method #2 (the avoid half)** + **#19 heuristics** (which Nielsen / P1–P9 heuristic each
trap violates). Produce a register of named anti-patterns at the **interaction and IA layer** (not brand
complaints). For each trap: *the trap → where it shows in the incumbent → the principle it violates → the
Myelin design rule that prevents it → the surface most at risk of re-creating it.* Cover at minimum:
config-maze (Jira); stitched-together identity/permission/UI seams (Atlassian); notification overload
(all); the dual-audience "serves neither" compromise (§2); enterprise-density-without-calm. Note where
Myelin's own architecture makes a trap easy (e.g. progressive disclosure done wrong → Jira's config maze).
Cite current evidence via web research.

**Deliverable.** `design-planning/04-research/north-star/trap-audit.md`. The register as above.

**Acceptance criteria (self-check).** Each trap maps to a specific violated principle AND a specific Myelin
surface-at-risk; the register is phrased as **falsifiable design rules** Phase 5/6 can be checked against;
no trap is a generic complaint ("Jira is bad"). Feeds rubric D7/D10 anchors and the completeness-critic.

---

### Prompt R-03 — JTBD catalogue for the three audiences  (Seq #3, effort M, user-dep: none / ranking DEFERRED, parallel-group: FOUNDATIONAL)

**Reads.** Standing preamble. Canon: `personas.md` P1–P15 (three clusters: engineers, PM/delivery,
corporate/governance), `planning/01-research/use-cases.md` (raw jobs material), design-language §2
(dual-audience), §7.7 (CLI as a job surface). No prior `04-research` dependency (foundational).

**Task.** Apply **method #1 (JTBD reasoned from personas — ADAPT, no-user instantiation)** with **#3
proto-persona discipline** (carry the HYPOTHESIS tag). Write **jobs-stories** in the form *"When
[situation], I want to [motivation], so I can [outcome]"*, grouped by the **three audiences**, each tagged
**PROVEN-theory / HYPOTHESIS-instantiation**, each **mapped to the §7 surface(s) that finish it** and the
**persona(s) that hold it**. Name the **dual-audience pairs explicitly** — the same data, two jobs (e.g.
P1 "burn down a cycle" vs. P6 "communicate a roadmap" over one issue model). Corporate/governance jobs
must NOT be skipped.

**Deferred note.** Reserve a clearly-marked section for the **`[DEFERRED-UNTIL-USERS]`
importance×satisfaction ranking** (the decisive ODI core) — record it as an executable Phase-4 plan
(interview/survey to rank jobs by importance × current satisfaction; what would falsify a "this is a
top job" hypothesis). **Do not fake the ranking.**

**Deliverable.** `design-planning/04-research/jtbd-flows/jtbd-catalogue.md`.

**Acceptance criteria (self-check).** All three audiences have jobs (corporate/governance present); every
job maps to a §7 surface and a persona; every job is HYPOTHESIS-tagged; the dual-audience same-data pairs
are named; the deferred ranking is recorded as a plan, not faked.

---

### Prompt R-04 — Named cross-surface task flows (service blueprints + job flows)  (Seq #4, effort L, user-dep: none, depends on R-03)

**Reads.** Standing preamble. Prior: `04-research/jtbd-flows/jtbd-catalogue.md` (R-03 — the jobs these
flows realize). Canon: `agent-native-design.md` §8 (worked agent flows = blueprints-in-waiting),
`planning/02-holistic-architecture/system-overview.md` §8.1 (PR context pane), §8.2 (agent HITL flagship),
§8.3 (DSR fan-out), `planning/01-research/use-cases.md` (UC-ISS-13/14/15, UC-GIT-3/17, UC-CI-4),
design-language §5.10 (states).

**Task.** Apply **method #8 (service blueprinting)** — frontstage §7 screens → backstage events/triggers
(ADR-04/08) → every actor including **agents** — and **method #9 (job-flow mapping)** — entry points,
keyboard + pointer paths (P3), and the **full per-screen state set**. Author **named** cross-surface flows,
**at least one per audience**, each drawn as blueprint PLUS job-flow with all states
(empty/loading/error/permission/erased/agent-pending + **partial-failure agent branches**). Required flows:
- **Engineer:** failing CI check → step → line of code → open fix PR → link issue (the wedge engineer
  flagship).
- **PM/delivery:** triage an incident from chat → issue → knowledge runbook → back to chat.
- **Corporate/governance:** a DPO answers a data-subject-access request across all five surfaces (the DSR
  fan-out, system-overview §8.3).
- **Agent HITL flagship:** CI fail → triage agent → issue → chat → proposed fix PR → approval card →
  human approves → review agent — drawn **with its partial-failure branches** (gate rejected; agent errors
  mid-chain; budget exceeded; loop-guard tripped).
Mark explicitly the **seams** (where today's stitched stack forces a tab-switch) — these are the moments
Myelin dissolves.

**Deliverable.** `design-planning/04-research/jtbd-flows/cross-surface-flows.md`.

**Acceptance criteria (self-check).** ≥1 named flow per audience + the agent flagship; each shows
frontstage screens mapped to §7, backstage events, and agent actors; each job-flow enumerates the full
state set incl. partial-failure agent branches; the seams are explicitly marked. Feeds Phase 5, Phase 6,
R-22, R-19. Covers completeness-critic edge-case/cross-surface flows.

---

### Prompt R-05 — Persona pressure-test & validation-priority register  (Seq #5, effort S, user-dep: none / real-persona replacement DEFERRED, depends on R-03)

**Reads.** Standing preamble. Prior: `04-research/jtbd-flows/jtbd-catalogue.md` (R-03 — jobs inherit
persona assumptions); also informed by `04-research/jtbd-flows/cross-surface-flows.md` (R-04 tensions) if
available. Canon: `personas.md` (all, esp. §6 archetypes, §7 open questions), design-language §9 (open
questions).

**Task.** Apply **method #3 (proto-persona pressure-testing)** + **method #4 (assumption/risk mapping)**.
Produce, **per persona**: its load-bearing assumptions + confidence + what-breaks-if-wrong. Then a
**conflict matrix** of persona pairs whose needs collide (e.g. P5 OSS-public-default vs. P12
private-governance-default; P1 dense vs. P6 calm) with the surface each conflict endangers. Then a ranked
**validation-priority register** — which personas to validate first — nominating per Phase-1 README §4 the
**P6-PM vs P1-engineer dual-audience tension** and the **P13-DPO / P12-security sovereignty-legibility**
bet as first; justify the ranking.

**Deferred note.** Every row carries the **`[DEFERRED-UNTIL-USERS]` real-persona replacement** flag (the
single most important Phase-4 research item per README §5.1). Record it as a plan (replace proto-personas
with interview-derived real personas; what would falsify each load-bearing assumption). Do not do it now.

**Deliverable.** `design-planning/04-research/jtbd-flows/persona-pressure-test.md`.

**Acceptance criteria (self-check).** Every persona has named load-bearing assumptions; the conflict matrix
names real pairs + the endangered surface; the validation-priority ranking is explicit and justified; the
deferred real-persona replacement is recorded, not done. Feeds R-16.

---

### Prompt R-06 — Platform IA & the "one shell" unification model  (Seq #6, effort L, user-dep: none / validation DEFERRED to R-07, depends on R-01, R-04)

**Reads.** Standing preamble. Prior: `04-research/north-star/teardown-dossier.md` (R-01 — North-Star IA
patterns), `04-research/jtbd-flows/cross-surface-flows.md` (R-04 — the flows the IA must support). Canon:
design-language §5.1 (nav shell structure), §7 (full view catalogue as IA inventory), §2 (default-landing
& vocabulary), §5.3 (`ArtifactRef` deep-link spine); `system-overview.md` §1–§2 (three glue contracts,
one `ArtifactRef`); ADR-13 (`ArtifactRef` down to sub-artifact).

**Task.** Apply **method #6 (expert-led IA design — ADOPT)**, building ON §5.1 + §7 (do NOT re-derive
them). Show how `repo→PR→diff`, `space→page→block`, `channel→thread→message`, `run→job→step`,
`issue→sub-issue` collapse into **ONE navigation/object model and ONE deep-linkable URL / `ArtifactRef`
structure**. Deliver: the unified object/navigation model; the primary-nav + contextual-sidebar + content
+ context-pane structure as a **concrete tree** (not just the principle); the labelling/taxonomy scheme
incl. the **persona-adaptive vocabulary candidates** ("issue" ↔ "work item"), with the fracturing-risk
flagged (§9 open question); the `myelin://…` / URL `ArtifactRef` structure down to sub-artifact
granularity; the **per-role default-landing map** (PM→roadmap, engineer→cycle board). Keep labels in
tokens/config so they're cheap to change and tree-testable later.

**Deliverable.** `design-planning/04-research/ia/platform-ia.md`.

**Acceptance criteria (self-check).** Every §7 surface has a place in the unified tree; one `ArtifactRef`
scheme covers all five subsystems down to sub-artifact; persona-adaptive vocabulary proposed with
fracturing-risk flagged; default-landing per role specified; labels config/token-held; the IA is
structured to be tree-tested in Phase 4. This IS the central problem's structural answer (rubric D4).

---

### Prompt R-07 — Unification-vs-distinctness study + card-sort/tree-test plan  (Seq #7, effort M, user-dep: none / validation DEFERRED, depends on R-06, R-03)

**Reads.** Standing preamble. Prior: `04-research/ia/platform-ia.md` (R-06 — the IA to study/validate),
`04-research/jtbd-flows/jtbd-catalogue.md` (R-03 — jobs → realistic tree-test task scenarios). Canon:
design-language §2 (density adapts), P1 (coherence), P5 (earned density), §5.6 (views component as the
unification mechanism); README §1 (the central design problem statement).

**Task.** Two parts.
**(1) The study** (method #6): a **per-surface ruling** on where each surface sits on the
unification↔distinctness axis, with the **rule** for *earning* distinctness (e.g. "a diff earns its own
density tier because <reason>; a roadmap earns its own pacing because <reason>; both keep the shared
chip/identity/palette"). This **directly informs sketch-funnel Axis 3** — say so explicitly.
**(2) The `[DEFERRED-UNTIL-USERS]` validation plan** (method #7): a closed/hybrid **card-sort** design + a
**tree-test** design over the R-06 IA, with realistic task scenarios derived from R-03 jobs, run
**per-segment** (engineer vs. PM/corporate) to expose the dual-audience split. State the "don't treat the
IA as validated before this runs" caveat.

**Deliverable.** `design-planning/04-research/ia/unification-study.md` (the two parts above).

**Acceptance criteria (self-check).** Every surface has a unification↔distinctness ruling with a stated
*rule* (not case-by-case whim); the ruling feeds Axis 3; the card-sort + tree-test designs are executable
as-written with grounded tasks and per-segment runs; the deferred flag and caveat are explicit.

---

### Prompt R-08 — Command palette + search-find interaction spec  (Seq #8, effort M, user-dep: none, depends on R-01, R-06)

**Reads.** Standing preamble. Prior: `04-research/north-star/teardown-dossier.md` (R-01 — Linear/Notion
palette), `04-research/ia/platform-ia.md` (R-06 — the IA the palette navigates). Canon: design-language
§5.2 (palette), §5.7 (search), §2.5 / §5.2 agent-tool symmetry (ADR-08 `ToolDef`s), ADR-07 (query AST),
ADR-03 (`list-objects` permission-pre-filter).

**Task.** Apply **method #2 (Linear/Notion palette teardown bar)**, **#20 cognitive walkthrough** (can a
new PM discover what an engineer reaches by muscle memory?), **#19 heuristics**. Specify the full
interaction: **modes** (navigate / act / search / build-query); the **query-AST surfacing** humanly (the
same AST as saved views and agent triggers, ADR-07); the **keyboard model**; result ranking; the
**permission-pre-filter guarantee as a UX behaviour** (you can only find what you may see — graceful,
never leaks a title, ADR-03); the **human↔agent tool-catalogue symmetry** (palette actions = the typed
`ToolDef`s agents use); and the **state set** (empty/loading/no-results/no-access/error). Then spec the
search *view* (facets, type/subsystem scoping, multilingual) as the palette's heavyweight sibling.

**Deliverable.** `design-planning/04-research/interaction/command-palette.md`.

**Acceptance criteria (self-check).** Palette unifies nav+actions+search with one query AST;
permission-pre-filter specified as UX (graceful, never leaks); keyboard model complete and the new-user
discoverability path walked (#20); all states enumerated; human/agent tool symmetry shown. Feeds rubric
D1 and every finalist's wedge moment.

---

### Prompt R-09 — Reference chip + artifact unfurl interaction spec (the wedge component)  (Seq #9, effort L, user-dep: none, depends on R-01, R-06)

**Reads.** Standing preamble. Prior: `04-research/north-star/teardown-dossier.md` (R-01 — Slack/GitHub
unfurl), `04-research/ia/platform-ia.md` (R-06 — the IA / `ArtifactRef` spine). Canon: design-language
§5.3 (the hard rules: live / permission-aware / tombstones), §5.5 (mentions as ref chips), P6 (the wedge);
ADR-13/ADR-03/ADR-12; the reference-graph architecture
(`planning/05-refined-shared-systems-architecture/.../reference-graph.md` — projection cache, the
tombstone ladder, content-anchored line-ranges); `04-research/jtbd-flows/cross-surface-flows.md` (R-04 —
the flows the chip threads).

**Task.** Apply **method #2 (Slack unfurl teardown bar)**, **#9 (the full state set is the point)**, **#8b
(live-projection, humanised strings)**. Spec the **most important shared component in the platform**
(§5.3): both forms — **compact chip** and **rich unfurl card per artifact type** (PR / issue / doc / run /
thread); the **inline-action surface** (re-run job, transition issue, approve PR — *where permitted*); and
**every state**: live (default, not snapshot), peeking (hover), **no-access** (graceful card, never a
leaked title), moved/outdated, **tombstoned/erased**, **cross-cell-resolves-to-projection-or-tombstone**,
and the **diff-line-anchored chip that relocates/orphans after rebase**. **Surface** the existing
reference-graph resolver behaviour — do not redesign the backend. Use humanised strings (no raw ids).

**Deliverable.** `design-planning/04-research/interaction/reference-unfurl.md`.

**Acceptance criteria (self-check).** Both forms specced per artifact type; inline actions specified with
permission behaviour; **all** states present incl. no-access, tombstoned, moved/outdated, cross-cell,
rebase-orphaned; live-not-snapshot default shown; humanised strings; maps onto the existing reference-graph
resolver, not a new one. Owns several §9 unglamorous states; feeds R-22 and Phase 6.

---

### Prompt R-10 — Shared interaction patterns: views, editor, notifications inbox, overlays  (Seq #10, effort L, user-dep: none, depends on R-01, R-06)

**Reads.** Standing preamble. Prior: `04-research/north-star/teardown-dossier.md` (R-01 — Notion
views/editor + Slack/Linear inbox), `04-research/ia/platform-ia.md` (R-06 — IA). Canon: design-language
§5.6 (views), §5.9 (editor), §5.8 (inbox), §8b.1 (overlays), §8b.2 (editor render path); ADR-05/06/07;
notifications architecture (`planning/05-refined-shared-systems-architecture/.../notifications.md` —
dedup, `origin_event`+`reason` "why fired", read-state).

**Task.** Apply **method #11 (atomic design — loose taxonomy over the §5 inventory)**, the **§8b.1 / §8b.2
day-one mandates**, **#19 heuristics**. Per component give: interaction spec + state set + atomic-taxonomy
placement (atom/molecule/organism, anchored to §5). Cover:
- **Views component** — table/board/calendar/list/gallery/timeline as projections of one query AST;
  persona-adaptive per §2; keyboard nav; inline-edit; drag. (This is the issues↔knowledge reuse boundary
  and the dual-audience mechanism.)
- **Editor** — block model, slash menu, mention/ref nodes; the **one render path + `render(parse(md))===md`
  round-trip gate** as a binding design constraint; controlled `contenteditable` (caret = char offset into
  serialized markdown).
- **Notifications inbox** — prioritised, deduped, **"why am I getting this"** provenance from
  `origin_event`+`reason`, one-action triage, one read-state truth across views, calm-by-default, agent
  volume out of the main stream.
- **Overlay primitives** — Dialog/Confirm/Popover/Dropdown/Tooltip/Toast: **portal-always, one z-index
  scale, centralised focus-trap/return-focus/scroll-lock/Escape/ARIA, single-purpose-by-shape** — carry
  the §8b.1 mandates verbatim as design rules.

**Deliverable.** `design-planning/04-research/interaction/shared-patterns.md`.

**Acceptance criteria (self-check).** All four families specced with state sets; views shown as the
issues↔knowledge reuse boundary AND the dual-audience mechanism; the editor's one-render-path + round-trip
constraint stated as binding; the inbox surfaces "why it fired" from existing `origin_event`+`reason` (not
a new mechanism); overlay primitives carry §8b.1 mandates verbatim; atomic taxonomy makes cross-component
reuse visible for Phase-7 coherence scoring. Feeds R-16, R-21, Phase 6.

---

### Prompt R-11 — Visual direction & mood-boards (3 directions, tone-words)  (Seq #3-parallel, effort M, user-dep: none [HOUSE STYLE], parallel-group: FOUNDATIONAL)

**Reads.** Standing preamble. Canon: design-language §3 (neutral-led, accent-restrained,
borders-over-shadow, the reserved `agent` treatment), §8b.3 (anti-aesthetic + measured rules);
`competitive-landscape.md` §1/§4 (Sourcehut-purism ↔ Notion-friendliness poles);
`external-insights/05-ux-and-design.md` §3/§4. No prior `04-research` dependency (foundational).

**Task.** Apply **method #13 (visual/aesthetic direction & mood-boarding — ADOPT, HOUSE STYLE)**. Propose
**three genuinely distinct** visual directions (not three shades of one) so the Phase-6 funnel spans
aesthetic variety on purpose. For each: a mood-board / reference collage (cite real sources via web
research), **tone-words**, the §3 constraints it honours, **how it places on sketch-funnel Axis 4
(emotional tone) and Axis 1 (density)**, and the **anti-aesthetic it explicitly avoids** (no traffic-light
fills, no emoji-as-UI, no AI sparkle, §8b.3). Tag each **HOUSE STYLE** and reference the tie-break rule
(P1–P9 + measured gates decide; pure aesthetics break ties only, README §5.6).

**Deliverable.** `design-planning/04-research/visual/visual-direction.md`.

**Acceptance criteria (self-check).** Three genuinely distinct directions; each tied to §3 constraints +
tone-words + an axis position; the anti-aesthetic explicit; every direction HOUSE-STYLE-tagged and the
tie-break rule referenced. Unblocks the funnel's tone/density axes early; feeds R-12, Phase 6 tokens,
Phase 8 look-fit.

---

### Prompt R-14 — Agent legibility & the plan-then-apply / HITL trust pattern set  (Seq #11, effort L, user-dep: none, depends on R-04)

**Reads.** Standing preamble. Prior: `04-research/jtbd-flows/cross-surface-flows.md` (R-04 — the agent
flagship flow). Canon: design-language §6 (the full agent contract), §5.4 (the HITL card component), §3.2
(the `agent` treatment), §8b.3 (no sparkle/emoji); `agent-native-design.md` §4 (plan-then-apply), §8
(worked flows); agent-fabric architecture
(`planning/05-refined-shared-systems-architecture/.../agent-fabric.md` — effect/gate/attribution mechanics
to surface, not redesign).

**Task.** Apply **method #15 (Microsoft HAX 18 guidelines — PROVEN, CHI 2019; cite)** + **method #17 (NN/g
agentic patterns + the §6.1–§6.5 critique checklist)**. Use web research for current HAX/NN-g material;
cite URLs. Where §6 doctrine is **stricter than HAX, doctrine wins** (note the conflict). Deliver:
- The **agent-treatment spec** (badge/colour/icon + label, **color-blind-safe**, never colour-alone, never
  sparkle/magic).
- The **plan-then-apply card** showing concrete **proposed effects per artifact + delegated authority**
  (what will change, on which artifacts, under whose authority) before they happen.
- The **Approve / Edit / Reject** behaviour incl. the **Edit path** (human amends the proposed effect).
- The **surfaces** it appears on (chat primary, inbox, inline).
- A **per-surface HAX-18 conformance note** for each agent-touching §7 surface (PR agent-reviewer, issue
  triage inbox, CI triage view, chat HITL card, agent governance console) — esp. HAX "Initially" + "When
  wrong".
- The **agent state set**: agent-pending, agent-working, gate-awaiting, gate-rejected, agent-error,
  budget-exceeded.

**Deliverable.** `design-planning/04-research/agent-ux/legibility-and-hitl.md`.

**Acceptance criteria (self-check).** Agent treatment unmistakable + color-blind-safe; plan-then-apply
shows concrete proposed effects + authority; the Edit path specified; every agent-touching §7 surface has
a HAX-18 note; full agent state set (incl. partial-failure: rejected/error/budget) present;
doctrine-beats-HAX conflicts resolved in doctrine's favour and noted; surfaces the existing agent-fabric
mechanics, not new ones. Feeds rubric D6 and every finalist's agent/HITL moment.

---

### Prompt R-15 — Agent attribution/audit + calm-agent-volume patterns; trust-calibration plan  (Seq #12, effort M, user-dep: none / trust-calibration DEFERRED, depends on R-14)

**Reads.** Standing preamble. Prior: `04-research/agent-ux/legibility-and-hitl.md` (R-14 — the legibility
patterns). Canon: design-language §6.4 (attribution/audit affordances), §6.5 (calm volume), §7.6 (agent
governance console, audit log explorer), §5.3 (tombstone/erased state); `agent-native-design.md` §5.5
(attribution/audit/GDPR); the existing audit/correlation mechanics (gdpr-and-audit + agent-fabric architecture).

**Task.** Apply **method #16 (Google PAIR — principles now, trust-calibration testing DEFERRED)** +
**method #15 (HAX "convey consequences" / "make clear how well it can do it")** + **#17**. Two parts.
**(1) Patterns:** per-action **provenance affordance** (who / what / on-behalf-of / trigger /
`correlation_id`); the inline **"why did this happen?"** + audit-trail link; the **scope/budget/delegation
inspector**; the **agent governance console + kill-switch** surface; and the **calm-volume patterns**
(threading, collapsible summaries, inbox routing, agent-out-of-main-timeline).
**(2) `[DEFERRED-UNTIL-USERS]` trust-calibration plan** (PAIR-style): do users correctly understand what
the agent can/can't do and when to trust it — tested on the HITL flow + the agent-reviewed PR; record the
caveat that **mock-agent trust may not predict real-LLM trust — design the *contract* to be trustworthy
regardless of runtime**.

**Deliverable.** `design-planning/04-research/agent-ux/attribution-and-calm.md` (the two parts).

**Acceptance criteria (self-check).** Per-action provenance + inline "why" + audit link specified;
calm-volume patterns concrete; governance/kill-switch surface specced; the deferred trust-calibration study
is executable-as-written and explicitly flagged; the "design the contract trustworthy regardless of
runtime" caveat recorded. Feeds rubric D6/D9.

---

### Prompt R-16 — Dual-/tri-audience persona-adaptive design study  (Seq #12-parallel, effort M, user-dep: none / both-audience validation DEFERRED, depends on R-03, R-05, R-10)

**Reads.** Standing preamble. Prior: `04-research/jtbd-flows/jtbd-catalogue.md` (R-03 — same-data job
pairs), `04-research/jtbd-flows/persona-pressure-test.md` (R-05 — persona conflicts),
`04-research/interaction/shared-patterns.md` (R-10 — the views component spec). Canon: design-language §2
(the full dual-audience resolution), §5.6 (views component as the literal mechanism — same records →
engineer board *or* PM roadmap), §3.4 (density modes), §9 (persona-adaptive vocabulary open question).

**Task.** Apply **method #18 (dual-audience / persona-adaptive — "one component, many lenses")**:
(a) identify both/three jobs over the same data; (b) design one component; (c) define **role/density/
vocabulary deltas as configuration, not separate code**; (d) **critique each lens against its persona**.
For each dual-audience surface (issue views, knowledge databases, dashboards): the two/three jobs over the
same data; the one component; the deltas as config; a **per-lens critique** (engineer board critiqued as
P1; PM roadmap critiqued as P6; exec rollup critiqued as P11) proving **neither lens is a degraded
compromise** (the §2 "serves neither" trap); and the **vocabulary-mapping proposal with the
fracturing-risk bounded**.

**Deferred note.** Add the **`[DEFERRED-UNTIL-USERS]` both-audience validation plan** — only PMs *and*
engineers using the same surface prove it holds (per-segment usability/RITE in Phase 4; what would falsify
"one component serves both"). Record that **Phase 6 must sketch dual-audience surfaces in *both* lenses**
(same data as engineer board AND PM roadmap).

**Deliverable.** `design-planning/04-research/dual-audience/persona-adaptive.md`.

**Acceptance criteria (self-check).** Each dual-audience surface has both/three jobs over the same data;
deltas expressed as configuration; each lens critiqued against its persona and shown not to be a
compromise; vocabulary mapping proposed with fracturing-risk bounded; the deferred both-audience validation
is executable-as-written and flagged; the "sketch in both lenses" requirement recorded. Feeds rubric D5.

---

### Prompt R-12 — Motion, microinteractions & emotional tone language  (Seq #13, effort M, user-dep: none, depends on R-11, R-08/R-09/R-10)

**Reads.** Standing preamble. Prior: `04-research/visual/visual-direction.md` (R-11 — the tone motion must
match), `04-research/interaction/command-palette.md` (R-08), `04-research/interaction/reference-unfurl.md`
(R-09), `04-research/interaction/shared-patterns.md` (R-10 — the components motion animates). Canon:
design-language §3.6 (motion principles, reduced-motion first-class), §8b.6 ("pages render, they don't
animate in").

**Task.** Apply **method #13 (direction extended to motion)** + **#19 heuristics** (does motion communicate
state or just decorate?) + **§8b motion budgets** (≈120–200ms, interruptible). Deliver the motion language:
**named easing/duration tokens (DTCG-structured)**; the catalogue of **functional motions**
(optimistic-settle, card-moves-column, panel-open, live-update-transition e.g. a PR going green,
agent-proposal-appear/resolve); the **microinteraction set that earns delight** AND the **anti-list** of
ones explicitly ruled out; and **reduced-motion equivalents as first-class** (not degraded) paths for every
motion. Tag each PROVEN (perception/a11y standard, cite) vs HOUSE STYLE.

**Deliverable.** `design-planning/04-research/visual/motion-microinteractions.md`.

**Acceptance criteria (self-check).** Motion tokens DTCG-structured and within the §3.6 budget; every
motion communicates a state change (no decoration); agent-proposal + live-update motions specced;
reduced-motion first-class for every motion; delight microinteractions named and the anti-list explicit;
"pages render, they don't animate in" honoured. Feeds rubric D3/D8 and Phase 6.

---

### Prompt R-13 — Perceived-performance & density-made-calm patterns  (Seq #14, effort M, user-dep: none, depends on R-10, R-12)

**Reads.** Standing preamble. Prior: `04-research/interaction/shared-patterns.md` (R-10 — the components
these patterns dress), `04-research/visual/motion-microinteractions.md` (R-12 — motion). Canon:
design-language P2 (speed), P5 (earned density), P8 (calm), §8b.6 (budgets + skeleton/error specifics);
`external-insights/05-ux-and-design.md` §4 (density-made-calm philosophy, optimistic+honest-rollback); the
prefetch/context-assembly extension
([`extension-planning/perceived-performance.md`](../extension-planning/perceived-performance.md)).

**Task.** Apply **method #19 (visibility of system status)** + **#24 switch test (latency / "feels
finished" bar)** + **§8b.6 hard latency budgets** (keyboard <100ms; suppress flash-of-spinner <1s). Two
halves.
**(1) Perceived performance** — per-surface **skeleton patterns** (structure-matching, never a blank
spinner); **optimistic-update + honest-rollback** patterns; the **prefetch/context-assembly UX** (failing
check → step → line, pre-fetched) **linked to its extension**; latency-budget targets restated as design
constraints. Note the residency constraint (no global CDN for personal data, P2/ADR-11 — perceived speed
bought via optimistic UI, in-region edge, prefetch, not global replication).
**(2) Density-made-calm** — concrete patterns that make dense surfaces calm: hierarchy from weight/colour
before size; borders over shadow; agent volume out of the main timeline; the one-prioritised-inbox
discipline; restraint as default. Tag each PROVEN / HOUSE STYLE.

**Deliverable.** `design-planning/04-research/visual/perceived-performance.md` (the two halves).

**Acceptance criteria (self-check).** Per-surface skeleton + optimistic + rollback patterns specified; the
prefetch/context-assembly UX named and linked to its extension; latency budgets restated as constraints;
density-made-calm patterns concrete (not "be calm"); each pattern PROVEN/HOUSE-STYLE-tagged. Feeds rubric
D7/D8 and the loading/optimistic-rollback completeness-critic states.

---

### Prompt R-17 — Accessibility audit method & per-surface a11y checklist  (Seq #15, effort M, user-dep: none / AT user-testing DEFERRED, depends on R-10)

**Reads.** Standing preamble. Prior: `04-research/interaction/shared-patterns.md` (R-10 — the components to
audit) and the other interaction specs (R-08, R-09) + R-14 (HITL card) for the hard components. Canon:
design-language §4 (the full baseline), §8b.3 (measured-token rules); the rubric **G1**. Standards (verify
current via web research, cite): WCAG 2.2; EN 301 549; EAA enforceable 2025-06-28; note **WCAG 2.2 ⊇ 2.1
except obsoleted 4.1.1**.

**Task.** Apply **method #21 (accessibility audit — WCAG 2.2 AA / EN 301 549 / EAA; automated + manual
expert review)** + **method #12 (measured-not-claimed token QA)**. Deliver the **audit method** (automated
pass + manual expert pass per surface) and a **per-surface a11y checklist** that the rubric's **G1
references and can be checked against**: contrast-measured-not-claimed (incl. the
**focus-token-≠-identity-token** derivation rule); visible focus in light/dark/high-contrast; full keyboard
operability + no traps for each **hard component** (diff, board drag, views inline-edit, block editor, HITL
card, command palette, nested overlays); **status-not-by-colour-alone**; correct semantics/ARIA per
pattern; **live-region announcement of event-driven updates without spamming**; 200% zoom / reflow on dense
surfaces; reduced-motion first-class. **Each item cites its WCAG / EN 301 549 criterion** and is tagged
PROVEN. State the WCAG-2.1-floor vs 2.2-target relationship correctly.

**Deferred note.** Include the **`[DEFERRED-UNTIL-USERS]` assistive-technology user-testing plan** (the
~60% the audit can't catch; AA ≠ usable-with-AT) — what to test, with which AT users, what would falsify
"this is operable with AT".

**Deliverable.** `design-planning/04-research/accessibility/audit-method.md`.

**Acceptance criteria (self-check).** The checklist is specific enough that **G1 is checkable** (not "be
accessible"); every hard component has a keyboard + screen-reader entry; the focus-token rule and
measured-contrast rule present; each item cites its WCAG / EN 301 549 criterion; the deferred AT user test
recorded; the 2.1-floor / 2.2-target relationship stated correctly.

---

### Prompt R-18 — i18n / l10n / RTL interaction-pattern research  (Seq #16, effort M, user-dep: none, depends on R-06, R-08/R-09/R-10)

**Reads.** Standing preamble. Prior: `04-research/ia/platform-ia.md` (R-06 — IA/labels),
`04-research/interaction/command-palette.md` (R-08), `04-research/interaction/reference-unfurl.md` (R-09),
`04-research/interaction/shared-patterns.md` (R-10 — the components that must survive expansion + RTL).
Canon: design-language §4 (i18n-first, EU-language support, RTL via logical properties, locale-aware
dates/calendars), §3.3 (EU-multilingual type coverage: Latin-extended, Greek, Cyrillic), §8b.4
(fixed-width/mobile bug classes), §8b.5 (humanise machine strings); the rubric **G2**.

**Task.** Apply **method #21 (a11y audit — i18n/RTL portion)** + **method #6 (IA labelling as i18n
surface)**. Deliver the i18n/l10n/RTL pattern set that the rubric's **G2 references**: **text-expansion
handling** (German ~30–40% longer — no truncation/clipping; no fixed-width assumptions); **non-Latin
rendering** requirements (font coverage, line-height, no clipping for Greek/Cyrillic); the **RTL pattern**
(logical start/end properties throughout; the **whole shell** + editor + views + overlays mirrored; tested
with a **real RTL string**, not a flipped mockup); **locale-aware date/number/calendar** formatting (SLA /
business-calendar load-bearing); the **humanised-string** requirement (no raw machine strings, §8b.5).
**Specify the exact G2 demonstration set** Phase-6 finalists must show: **≥1 long-word language (German),
≥1 non-Latin script (Greek/Cyrillic), ≥1 mirrored RTL state, locale-formatted dates.** Name the §8b.4
fixed-width-assumption bug classes to design around. Cite standards/data via web research.

**Deliverable.** `design-planning/04-research/accessibility/i18n-rtl-patterns.md`.

**Acceptance criteria (self-check).** Text-expansion + non-Latin + RTL patterns concrete and reference
logical properties; the exact G2 demonstration set specified; whole-shell mirroring (incl. editor / views /
overlays) required, not just text direction; humanised strings required; the §8b.4 fixed-width bug classes
named. Feeds rubric G2 and Phase 6 (designed in from sketch #1).

---

### Prompt R-19 — Sovereignty-as-UX: residency / GDPR / DSR / audit legibility patterns  (Seq #17, effort M, user-dep: none / regulated-buyer review DEFERRED, depends on R-04)

**Reads.** Standing preamble. Prior: `04-research/jtbd-flows/cross-surface-flows.md` (R-04 — the DPO DSR
cross-surface flow). Canon: design-language P9 (sovereignty as UX), §7.6 (GDPR/data-rights console,
data-map/RoPA & residency console, audit-log explorer, tenant/cell & residency settings, agent governance
console), §5.3 (tombstone/erased state); `system-overview.md` §8.3 (DSR fan-out); the gdpr-and-audit
architecture (`planning/05-refined-shared-systems-architecture/.../gdpr-and-audit.md` — DSR orchestrator,
crypto-shred, data map, restriction flag — to surface, not redesign); `competitive-landscape.md` §6.2
(what EU-sovereign must mean).

**Task.** Apply **method #8 (service blueprinting — the DSR/erasure flow + the residency console)** +
**method #15 (HAX "convey consequences" of an erasure)** + **#19 heuristics (the P9 sovereignty-as-UX
heuristic)**. Deliver the sovereignty-as-UX pattern set: **residency/visibility cue patterns** (where they
sit near data — the scope indicator's region/residency cue, per-artifact visibility chip); the **DSR
console blueprint** (locate/export/rectify/restrict/erase across holders; deadline tracking; verifiable
receipts; **the data-subject view AND the DPO view**); the **data-map/RoPA & residency console**; the
**audit-log explorer** with provenance/correlation threading; the **agent governance/kill-switch** surface;
and the **erased/tombstoned UX** (the GDPR-aware degraded state). Articulate the **Axis-6 trade-off**
(always-on cues ↔ on-demand consoles). Tag PROVEN (where a GDPR / EN 301 549 requirement backs it) vs
HOUSE STYLE; **honestly mark sovereignty-as-UX under-evidenced where it is HOUSE STYLE** (no external
playbook). Surface the existing gdpr-and-audit mechanics, don't invent them.

**Deferred note.** Include the **`[DEFERRED-UNTIL-USERS]` regulated-buyer (P13/P14) review plan** — a
DPO/procurement review substitutes for user testing; what to put in front of them and what would falsify
"a DPO trusts this at a glance".

**Deliverable.** `design-planning/04-research/sovereignty/sovereignty-as-ux.md`.

**Acceptance criteria (self-check).** Residency/visibility cues placed concretely near data; DSR console
blueprinted from both data-subject and DPO sides; erased/tombstoned UX specified; audit-log explorer
surfaces provenance/correlation; patterns surface existing mechanics; Axis-6 trade-off articulated; deferred
regulated-buyer review recorded; sovereignty-as-UX honestly tagged under-evidenced where HOUSE STYLE. Feeds
rubric D9 and Axis 6.

---

### Prompt R-20 — First-run / onboarding delight patterns (3 archetypes)  (Seq #18, effort M, user-dep: none, depends on R-01, R-04)

**Reads.** Standing preamble. Prior: `04-research/north-star/teardown-dossier.md` (R-01 — onboarding
teardown), `04-research/jtbd-flows/cross-surface-flows.md` (R-04 — the flows onboarding leads into). Canon:
design-language §5.10 (empty = onboarding-forward), §7.6 (onboarding & empty-platform flows; startup vs.
enterprise-admin first-run); `personas.md` §6 (the archetypes; "weak onboarding loses the startup
instantly"); P4 (progressive disclosure).

**Task.** Apply **method #20 (cognitive walkthrough — learnability, no users)** + **method #2 (Linear /
Notion / Slack onboarding teardown bar; cite via web research)** + **#19 heuristics**. Deliver first-run
patterns for the **three archetypes**: the **low-friction startup (P1)** — near-zero-friction or lost; the
**scale-up introducing PMs/process**; the **regulated-enterprise admin (P15)** standing up
SSO/residency/agent-policy. Show the **guided-start sequence** tying the empty states (§5.10) into a
coherent start (first repo → first issue → first doc → first channel → first agent run); how **depth is
disclosed progressively** (the admin's SSO/residency/agent-policy depth one layer down, not in the
startup's face); and the **delight moments** (first wedge appearing, first agent proposal) **without a
tutorial slog**. **Cognitive-walkthrough-check each first-step** (will the user know what to do / see the
control / understand the feedback?).

**Deliverable.** `design-planning/04-research/craft/onboarding-delight.md`.

**Acceptance criteria (self-check).** Three archetype first-runs specified; the guided-start ties the empty
states together; progressive disclosure keeps enterprise depth out of the startup's face; each first-step
walkthrough-checked; delight moments named without a tutorial slog. Feeds rubric D2 and every finalist's
empty/first-run state.

---

### Prompt R-21 — Empty / loading / error / permission / erased state craft  (Seq #19, effort M, user-dep: none, depends on R-09, R-10, R-13)

**Reads.** Standing preamble. Prior: `04-research/interaction/reference-unfurl.md` (R-09 — chip/unfurl
no-access/tombstone states), `04-research/interaction/shared-patterns.md` (R-10 — each component's states),
`04-research/visual/perceived-performance.md` (R-13 — skeleton/optimistic-rollback patterns). Canon:
design-language §5.10 (cross-cutting state patterns), §8b.6 (loading-shows-structure / error-blames-system
/ fails-static); `external-insights/05-ux-and-design.md` §4 (states as first-class designed); README §9
(the completeness-critic state list — **this item owns it**).

**Task.** Apply **method #9 (job-flow per-screen state checklist)** + **#19 heuristics (error-recovery,
visibility of status)** + **§8b.6 specifics**. Deliver a **state-craft catalogue** that, **per shared
component AND per primary §7 surface**, specifies all states: **empty** (onboarding-forward) / **loading**
(structure-skeleton, never blank spinner) / **error** (blame the system in one quiet line + a path) /
**permission-denied** (graceful no-access, never a leak) / **erased-tombstone** (GDPR-aware degraded) /
**agent-pending** — PLUS the states the happy-path bias skips: **optimistic-rollback**,
**conflict-surfacing** (the CAS→CRDT path shown legibly), **stale/offline/reconnecting** (firehose
drop+resume), **degraded-surface "temporarily unavailable"** (fails static), and the **storm /
30×-agent-surge** inbox experience. Apply the §8b.6 specifics to each. This is the checklist Phase-6
finalists demonstrate on ≥1 surface (rubric "comparable screens").

**Deliverable.** `design-planning/04-research/craft/state-craft.md`.

**Acceptance criteria (self-check).** Every shared component + primary surface has its full state set; the
skipped states (optimistic-rollback, conflict, reconnecting, degraded-static, storm) present, not just the
six common ones; §8b.6 specifics applied; the catalogue usable as the Phase-6 state checklist; it
explicitly covers the README §9 list. Feeds rubric D8 + the switch test (D10).

---

### Prompt R-22 — The cross-artifact "wedge" moments (delight at the seams)  (Seq #20, effort M, user-dep: none, depends on R-04, R-09, R-13)

**Reads.** Standing preamble. Prior: `04-research/jtbd-flows/cross-surface-flows.md` (R-04 — the
cross-surface flows), `04-research/interaction/reference-unfurl.md` (R-09 — the chip/unfurl),
`04-research/visual/perceived-performance.md` (R-13 — prefetch/context-assembly). Canon: design-language P6
(reference everything — the wedge), §5.3 (chip/unfurl), §8b.6 (the system assembles + pre-fetches context);
`system-overview.md` §8.1 (PR context pane — the wedge flagship); `competitive-landscape.md` §6 (the
integration *is* the differentiator); the perceived-performance + (if present) unfurl-projection extensions.

**Task.** Apply **method #8 (service blueprinting — the wedge flows)** + **method #9 (job-flow,
moment-by-moment experience)**. Deliver a catalogue of **named wedge moments** — each a specific point in a
cross-surface flow where the integration produces delight the fragmented stack can't — with: **the moment**,
**the cross-surface mechanics behind it** (the events/refs that make it possible), **the design that makes
it felt** (not buried), and **the "the old stack can't do this" contrast**. At minimum **≥5 moments**: the
PR context-pane assembly; the live chat unfurl with inline actions; the "why-it-fired + pre-fetched next
hop" notification; the cross-subsystem live backlinked reference; the agent flow threading one
`correlation_id` across surfaces visibly. Each maps to a real R-04 flow and a real component (R-09). Design
them as **deliberate love-moments**, not a feature list.

**Deliverable.** `design-planning/04-research/craft/wedge-moments.md`.

**Acceptance criteria (self-check).** ≥5 named wedge moments, each with cross-surface mechanics, felt-design,
and "old-stack-can't" contrast; each maps to a real cross-surface flow (R-04) and component (R-09); the
moments are deliberate love-moments; usable as the Phase-6 wedge-moment screen. Feeds rubric D4/D10.

---

## §C — Execution plan (for the orchestrator)

### Run order (one line)
`[R-01→R-02 ∥ R-03→R-04 ∥ R-11]` (foundational parallel band) → `R-05` → `R-06`→`R-07` →
`R-08`→`R-09`→`R-10` → `[R-14→R-15 ∥ R-16]` → `R-12`→`R-13` → `R-17`→`R-18` → `R-19` →
`R-20`→`R-21`→`R-22`.

### The parallel band (start concurrently)
Three foundational tracks have **no mutual dependency** and each unblocks the middle band — dispatch them
together at the start:
1. **Teardowns:** R-01 → R-02 (R-02 reuses R-01's format; run R-02 once R-01 is done).
2. **JTBD→flows:** R-03 → R-04 (R-04 depends on R-03).
3. **Visual direction:** R-11 (standalone; no dependency).

Everything downstream of the IA + interaction-pattern band is sequential because the patterns are shared
substrate the later items critique against.

### Per-item dependency note (what must complete first)
| Item | Must be done before it |
|---|---|
| R-01 | — (foundational) |
| R-02 | R-01 |
| R-03 | — (foundational) |
| R-04 | R-03 |
| R-11 | — (foundational) |
| R-05 | R-03 (informed by R-04) |
| R-06 | R-01, R-04 |
| R-07 | R-06, R-03 |
| R-08 | R-01, R-06 |
| R-09 | R-01, R-06 |
| R-10 | R-01, R-06 |
| R-14 | R-04 |
| R-15 | R-14 |
| R-16 | R-03, R-05, R-10 |
| R-12 | R-11, R-08, R-09, R-10 |
| R-13 | R-10, R-12 |
| R-17 | R-10 (+ R-08, R-09, R-14 for hard components) |
| R-18 | R-06, R-08, R-09, R-10 |
| R-19 | R-04 |
| R-20 | R-01, R-04 |
| R-21 | R-09, R-10, R-13 |
| R-22 | R-04, R-09, R-13 |

**Safe concurrency within bands** (beyond the foundational band): once R-06 is done, **R-08 / R-09 / R-10
can run concurrently** (each depends only on R-01 + R-06). Once R-14 is done, **R-15 and R-16 can run
concurrently** (R-16 also needs R-03, R-05, R-10, which precede this band). R-17 and R-18 may run
concurrently once their interaction-spec inputs exist. R-21 and R-22 may run concurrently once R-13 is done
(both depend on R-09/R-10/R-13; R-22 also on R-04). The single-thread spine is: foundational band →
R-05 → R-06 → R-07 → (R-08/R-09/R-10) → (R-14→R-15 ∥ R-16) → R-12 → R-13 → (R-17 ∥ R-18) → R-19 →
R-20 → (R-21 ∥ R-22).

### How deferred-until-users items are handled in this autonomous run
For each item with a deferred sub-part, the agent **executes the no-user substitute now** and **records the
deferred validation as a concrete, executable plan tagged `[DEFERRED-UNTIL-USERS]`** — never presenting the
substitute as validated. The deferred sub-parts:
- **R-03** — JTBD importance×satisfaction ranking (ODI core). Substitute now: HYPOTHESIS-tagged jobs-story
  catalogue.
- **R-05** — real-persona replacement (the load-bearing risk). Substitute now: pressure-test +
  validation-priority register.
- **R-07** — card-sort + tree-test (per-segment). Substitute now: expert-led IA ruling + the study design.
- **R-15** — PAIR-style agent trust-calibration testing. Substitute now: §6-contract critique + HAX audit.
- **R-16** — both-audience validation of "one component, many lenses". Substitute now: per-lens critique
  against each persona.
- **R-17** — assistive-technology user testing. Substitute now: manual expert a11y audit + measured tokens.
- **R-19** — regulated-buyer (P13/P14) review of sovereignty consoles. Substitute now: expert blueprint +
  heuristic audit.
- **(cross)** — RITE loops on Phase-6 sketches/finalists are handled in Phase 6/Phase 4 via heuristic eval
  + cognitive walkthrough + switch test as the no-user substitute (not an R-item here).

The orchestrator should ensure each agent's output file actually contains its `[DEFERRED-UNTIL-USERS]`
section before marking the item done, and should **commit each file** (agents do not commit).
