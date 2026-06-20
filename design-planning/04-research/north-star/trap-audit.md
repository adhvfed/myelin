# R-02 — Trap / Anti-Pattern Audit (Jira · Atlassian · Teams)

> Phase 4 research corpus item **R-02** (WS-A, Seq #2, foundational — runs right after R-01). Method
> **#2 (the avoid half)** + **#19 heuristics** (which Nielsen / P1–P9 heuristic each trap violates). This
> is the **register of named anti-patterns at the interaction & IA layer** — the inverse of R-01's
> "steal" dossier. R-01 named the *trap hiding inside each good pattern*; R-02 names the *traps the
> incumbents fell into wholesale*, and turns each into a **falsifiable Myelin design rule** Phase 5/6 can
> be checked against.
>
> **Status date: 2026-06-20.** Tagging (VISION §3 honesty rule): **PROVEN** = cited current source /
> standard / observed incumbent behaviour; **HOUSE STYLE** = our design judgement / synthesis.
> `[VERIFY]` = time-sensitive (re-confirm before Phase 7 external use).
>
> Builds ON, does not re-derive: design-language **P1–P9**, **§2** (dual-audience), **P4** (progressive
> disclosure), **P8** (calm); `competitive-landscape.md` §3/§6/§6.1; **reuses R-01's teardown format**
> (`teardown-dossier.md`) — the per-entry structure below is the inverse of R-01's per-pattern structure.
> Reading order downstream: R-06/R-07 (IA), R-16 (dual-audience), R-08/R-10 (interaction), R-15/R-21
> (calm/storm) all cite the rules here; rubric **D7/D10** anchors and the completeness-critic draw on §7–§8.

---

## 0. How to read this audit (the register schema)

R-01 proved that **a North-Star pattern copied without its trap is how you inherit the incumbent's
failure.** R-02 audits the traps **as a register of falsifiable rules** so that inheritance is *checkable*.

Every entry uses the **fixed inverse-of-R-01 structure** the prompt mandates:

> **The trap → where it shows in the incumbent (evidenced/cited) → the principle it violates (Nielsen #
> + Myelin P) → the Myelin design rule that prevents it (falsifiable) → the surface most at risk of
> re-creating it.**

Two rules are load-bearing (the acceptance criteria, made operational):

1. **No trap is a generic complaint.** "Jira is bad" is not a trap. Each entry names a *specific
   interaction or IA mechanism*, the *heuristic it breaks*, and a *rule a sketch can be measured against*.
2. **Every trap names the Myelin surface most at risk of re-creating it** — because Myelin's own
   architecture (progressive disclosure, agent fabric, unified backend) makes several of these traps
   *easy* to re-commit. The "surface at risk" column is the Phase-5/6 watch-list.

**Falsifiability convention.** Each design rule is phrased so a reviewer can mark it **PASS/FAIL** on a
sketch (e.g. "*FAIL if a primary action requires opening a configuration screen to become available*"),
not as an aspiration ("be simple"). This is what makes the register usable as a checklist, not a slogan.

`[a11y-debt]` / `[gov-risk]` tags mark traps that also threaten a rubric hard gate (G1) or the
sovereignty/governance dimension (D9) respectively.

---

## 1. THE CONFIG-MAZE FAMILY (Jira) — power that buries the job

**Why Jira is the trap North Star here:** it is the explicit **enterprise-depth reference** R-01 stole
from (`competitive-landscape.md §3`) — and the canonical warning of what that depth *costs* when
disclosure is done wrong. In 2025 developer sentiment, **"JIRA stood out on the 'most hated tools' list,
attracting more criticism than the next four tools combined … slow, complex, and painful to use, and
clearly not designed for developers"** (PROVEN — Developer-First survey roundup, 2025). The mechanism
matters more than the sentiment; this section dissects it into discrete traps.

### 1.1 The configuration maze — depth with no floor  `[→ at risk: issue tracker §7.3, admin/settings §7.6]`

- **The trap.** Power surfaced as an *ever-growing flat configuration space* — custom fields, work types,
  statuses, workflow schemes, permission schemes, screen schemes — that accumulates until the everyday
  job (file an issue, find a board) is buried under setup the user never asked for.
- **Where it shows (PROVEN).** "Over time, custom fields, work types, and statuses **accumulate and make
  it harder for users to find what they need**" (Atlassian Community admin guidance, 2025). The pathology
  is endemic enough that Atlassian is **enforcing hard data limits (700 custom fields/instance; per-project
  field/work-type caps) from Feb 2026** specifically "to prevent performance slowdowns and ensure a
  smoother user experience" (PROVEN — Atlassian guardrails docs, 2025/2026 `[VERIFY]` exact dates). The
  config burden is heavy enough that **"the 'Jira Admin' has become a necessary, full-time role for many
  enterprises just to keep workflows from collapsing under their own complexity"** (PROVEN — Linear-vs-Jira
  analyses, 2025/2026).
- **Principle violated.** Nielsen **#8 aesthetic-and-minimalist-design** + **#6 recognition-not-recall**
  (every accreted field is recall load); Myelin **P4 (progressive disclosure done *wrong*)** and **P8
  (attention is sacred)**. This is the *failure mode of P4*: depth that is present-but-not-imposed
  degenerates into depth-that-is-imposed when there is no disclosure floor.
- **Myelin design rule (falsifiable).** **R-CFG-1:** *Every primary job completes at the default
  configuration — zero setup.* FAIL if any comparable-screen task (create issue, open board, find an
  artifact) requires visiting a configuration/admin screen to become possible. **R-CFG-2:** *Depth is
  reached by progressive disclosure with a hard floor — the newcomer surface never grows with org
  configuration.* FAIL if adding custom fields/workflows changes the *default* create/triage surface a
  startup (P1) sees (it must live one layer down, P4). **R-CFG-3:** *No "admin-as-a-job" requirement* —
  governance "scales from invisible to fully controlled without forcing the startup through enterprise
  complexity" (`personas.md §6`). FAIL if a sketch implies a dedicated admin role is required for daily
  operation.
- **Surface most at risk (HOUSE STYLE).** **The issue tracker (§7.3) and its create/edit forms** — Myelin
  *promises* Jira-grade depth (custom fields, hierarchies, SLAs, audit), so it is structurally the most
  likely surface to re-grow the maze. The §5.6 views component and the slash-menu (R-01 §2.3, "slash-menu
  bloat") inherit a miniature of this risk. **This is the precise tension R-16 resolves and R-06/R-07's IA
  must keep config out of the daily tree.**

### 1.2 The over-complex form — 15 fields for a 3-field job  `[→ at risk: issue create/edit, knowledge db forms §7.4]`

- **The trap.** A creation/edit form that exposes the *union of all stakeholders' fields* to *every*
  user — "request forms with 15 fields when users only care about three" (PROVEN — Atlassian Community,
  2025). The corporate need to capture data overrides the engineer's need to file fast.
- **Where it shows (PROVEN).** "Forms that are too complex or cluttered **confuse non-technical users,
  leading to errors or incomplete submissions** … additional admin work to correct data" (Atlassian
  Community, 2025). Note the irony: the *over-collection* harms the *corporate* audience too (dirty data),
  not just engineers — so it serves neither (links to §4, the dual-audience trap).
- **Principle violated.** Nielsen **#8 minimalist design** + **#5 error prevention** (over-asking causes
  bad data); Myelin **P4** and **P5 (density is earned)**. A form is the densest single moment of the
  product; un-earned density here is the worst place for it.
- **Myelin design rule (falsifiable).** **R-FORM-1:** *The default create form shows only required +
  high-frequency fields; everything else is one disclosure step away.* FAIL if the default issue-create
  surface in any sketch shows more than the minimal field set for the persona's lens. **R-FORM-2:**
  *Field sets are lens-adaptive, not user-additive* — the engineer's quick-create and the PM's
  structured-create are *two lenses on one schema* (R-16), never "everyone gets every field."
- **Surface most at risk.** **Issue create/edit (§7.3) and knowledge-database record forms (§7.4)** — the
  two surfaces that share the §5.6 views/forms machinery and serve both audiences.

### 1.3 Configurability-as-inconsistency — every project a snowflake  `[→ at risk: cross-surface coherence, §5.6 views]`

- **The trap.** Per-project/per-team configurability so unbounded that **two projects in the same
  instance behave differently** — different statuses, fields, board semantics — so muscle memory does not
  transfer and "consistency & standards" collapses *inside one product*.
- **Where it shows (PROVEN).** "Creating workflows for multiple teams, each with unique needs, **quickly
  becomes overwhelming** … balancing simplicity for end-users with the complexity required by specific
  processes" (Atlassian Community, 2025). The result is the well-known Jira experience of relearning the
  board every time you switch project context.
- **Principle violated.** Nielsen **#4 consistency-and-standards**; Myelin **P1 (one product)** at the
  *intra*-product level — coherence broken not across surfaces but across instances of one surface.
- **Myelin design rule (falsifiable).** **R-CONS-1:** *Interaction grammar is invariant across configured
  instances.* FAIL if customisation can change *how* a board/table is navigated or acted on (keyboard
  model, action verbs, status semantics) — config may change *what data/fields* appear, never the *grammar*
  (R-01 §1.3: "one interaction grammar across views"). **R-CONS-2:** *Status categories are a fixed,
  shared vocabulary* (a configured status maps to one of a small fixed set of *category* semantics) so
  "what does this column mean" reads identically across projects.
- **Surface most at risk.** **The §5.6 views component** — it is shared across issues AND knowledge, so an
  unbounded per-space customisation model fractures *two* subsystems at once.

---

## 2. THE STITCHED-TOGETHER FAMILY (Atlassian suite) — integration that is not unification

**Why the Atlassian suite is the trap North Star here:** it is the canonical **"integrated but not
unified"** failure (`competitive-landscape.md §6.1`): Jira + Confluence + Bitbucket + JSM "feel **stitched
together, not unified** (separate data models, permission models, UIs)." This is the *exact* failure
Myelin's whole architecture exists to avoid (P1; one identity, one permission model, one event bus, one
reference graph). R-01 §4.1 previewed this at the PR level ("PR as a junk-drawer of bolted-on tabs"); here
it is the register.

### 2.1 The seam between products — separate identity/permission/UI models  `[→ at risk: §5.1 shell, §5.11 identity, the whole platform]`

- **The trap.** Each product carries its **own identity surface, its own permission model, and its own UI
  conventions**; "integration" is API-level cross-linking laid *over* the seams, so the user feels the
  boundary every time they cross it (different nav, different avatar treatment, different permission
  language, a context-switch tax).
- **Where it shows (PROVEN/HOUSE STYLE).** `competitive-landscape.md §6.1` (PROVEN-as-prior-research):
  "products feel stitched together, not unified (separate data models, permission models, UIs)." 2025
  corroboration: customers report **"deep integrations / interfaces" that "do not work" when migrating**,
  and the suite is held together by per-product links and add-ons (Atlassian Guard etc.) rather than one
  model (The Register, 2025-09). The cloud-only forced migration further fragmented the install base across
  capability tiers (PROVEN — The Register, 2025-09) `[VERIFY]`.
- **Principle violated.** Nielsen **#4 consistency-and-standards** + **#2 match-system-and-real-world**
  (the user's mental model is "my work," not "Jira's copy vs Confluence's copy"); Myelin **P1 (one
  product)** — *the* defining bet. The seam is precisely where P1 either holds or fails.
- **Myelin design rule (falsifiable).** **R-SEAM-1 (one identity):** *The same `Principal` renders the
  identical identity badge on every surface* (design-language P1). FAIL if an avatar/identity treatment
  differs between git, issues, knowledge, chat, or agent surfaces. **R-SEAM-2 (one permission language):**
  *Permission state is expressed in one vocabulary and one visual treatment platform-wide* — "no access"
  looks and reads the same in a PR, a doc, a chip, and a search result (ties to R-09 no-access card).
  **R-SEAM-3 (one shell):** *Crossing subsystems never changes the navigation grammar or chrome
  identity* — primary nav, palette, and reference chip are invariant; only the *content surface* changes
  (R-06 owns the unified tree). **R-SEAM-4 (no API-stitch tells):** *Cross-surface references are native
  reference-graph objects, not embedded foreign panels* — FAIL if a referenced artifact renders as a
  visibly foreign widget (different fonts/colours/loading behaviour) rather than a native §5.3 chip/unfurl.
- **Surface most at risk (HOUSE STYLE).** **The PR context pane / wedge flagship (§7.1, system-overview
  §8.1)** — it aggregates issue + doc + CI + chat, so it is the single most likely place to *look*
  stitched if the referenced artifacts aren't truly native (this is R-01 §4.1's trap, now a rule). Second:
  **the shell (§5.1)** itself when a surface that was historically its own product (chat, knowledge) is
  given "its own personality" past the §2/Axis-3 budget (link to §5 below).

### 2.2 The cross-product permission leak — different models, divergent enforcement  `[→ at risk: §5.3 chip/unfurl, search §5.7]`  `[gov-risk]`

- **The trap.** When each product has its *own* permission model, **a reference/preview from one product
  can leak a title or snippet the viewer can't access in the source product** — the permission boundary
  isn't shared, so cross-product surfaces (link previews, search, embeds) resolve against the *wrong* or a
  *stale* authority.
- **Where it shows (HOUSE STYLE, structural).** This is the *predicted* consequence of §2.1's separate
  permission models; R-01 flagged the concrete instances in the North Stars (Notion mention preview §2.4,
  Slack snapshot unfurl §3.1 leaking a restricted title). The stitched suite multiplies the risk because
  enforcement lives in N places. (Tagged HOUSE STYLE because it is a structural inference + R-01's cited
  North-Star instances, not a single cited Atlassian CVE.)
- **Principle violated.** Nielsen **#1 visibility-of-system-status** done *wrongly* (showing status the
  viewer shouldn't see); Myelin **P9 (sovereignty/GDPR as UX)** and the ADR-03 permission-pre-filter
  *correctness* invariant. This is not a nicety — it is a GDPR leak.
- **Myelin design rule (falsifiable).** **R-LEAK-1:** *You can only find/preview what you may see —
  permission is pre-filtered at the single shared model, never per-surface.* FAIL if any chip, unfurl,
  search result, or backlink can render a title/snippet/metadata for an artifact the viewer lacks read
  access to (it must degrade to a graceful "no access" card, never a leaked title — R-09 owns the state).
  **R-LEAK-2:** *One enforcement point* — the same ADR-03 `list-objects` pre-filter governs palette,
  search, chips, and embeds; FAIL if any surface resolves visibility independently.
- **Surface most at risk.** **The reference chip/unfurl (§5.3) and search (§5.7)** — the cross-cutting
  surfaces that, by design, reach into every subsystem. **Owned in full by R-09; R-08 for palette/search.**

### 2.3 Agent / connector sprawl — automation no one can govern  `[→ at risk: agent governance console §7.6, agent surfaces]`  `[gov-risk]` `[VERIFY]`

- **The trap.** Agents/automations added *per product* with no shared inventory, authority model, or audit
  — "**messy connector sprawl** and painful audit cycles because **no one can prove what data AI touched**"
  (PROVEN — Atlassian/Deviniti governance analyses, 2026). The trap is the *2026 update* of §2.1: the
  stitched suite, now stitched with agents, where each agent is a new ungoverned seam.
- **Where it shows (PROVEN).** Atlassian itself frames the problem as **"agent sprawl"** and has had to
  retrofit governance — "permissions for AI access and agent building **separated**," "**org-wide agent
  lists and insights** give admins a live inventory of who built what, where it's running" (PROVEN —
  Atlassian Rovo blog / Team '26, 2026 `[VERIFY]` exact features). The fact that this was a *retrofit*
  after stalled adoption ("rollout stalls after compliance questions or access concerns arise") is the
  evidence it was a designed-in gap.
- **Principle violated.** Nielsen **#1 visibility-of-system-status** (you can't see what the agents did)
  + **#10 help-users-recover** (no audit to recover from); Myelin **P7 (agents visible, labelled,
  attributed)** and **P9 (audit/provenance as UX)**. The security/DPO personas' (P12/P13) deepest fear is
  *exactly* "ungoverned automation."
- **Myelin design rule (falsifiable).** **R-AGT-1 (born-governed):** *Every agent is a first-class
  `Principal` in the one identity/permission/audit model from creation — never a per-surface bolt-on.*
  FAIL if an agent can act without appearing in the single audit log with `correlation_id`, actor,
  on-behalf-of, and trigger (R-15). **R-AGT-2 (one inventory):** *A single agent-governance console lists
  every agent, its scope, budget, and activity, with a kill-switch* — the inventory is a *day-one* surface
  (§7.6), not a retrofit. FAIL if agent presence is discoverable only per-surface. **R-AGT-3 (provable
  provenance):** *"What did this agent touch?" is answerable in the UI* — FAIL if any agent action lacks an
  inline "why did this happen?" + audit-trail link (R-15).
- **Surface most at risk (HOUSE STYLE).** **Myelin's agent fabric is its biggest differentiator AND its
  biggest sprawl risk** — one fabric across five surfaces means agents *everywhere*, which is the calm/
  governance challenge at maximum. **The agent governance console (§7.6) and every agent-touching surface;
  owned by R-14 (legibility) and R-15 (attribution/calm/governance).**

---

## 3. THE NOTIFICATION-OVERLOAD FAMILY (all incumbents) — the firehose

**Why this is "all incumbents":** R-01 named it the *universal* incumbent failure; the design language
calls it out for GitHub (noisy/hard-to-tune), Slack (channel sprawl + overload), and the suites. Teams is
the **worst-in-class exemplar** and the one with the freshest, hardest evidence, so it anchors this
section.

### 3.1 Notification overload / always-on firehose (Teams as exemplar)  `[→ at risk: §5.8 inbox, §7.5 chat, agent volume §6.5]`

- **The trap.** Every event becomes a push notification by default, across **too many notification *types*
  with no unified prioritisation**, so the signal-to-noise ratio collapses and the user either drowns or
  mutes everything (losing the genuinely important ones).
- **Where it shows (PROVEN).** Teams is the canonical case: a top community thread is literally titled
  **"Teams has too many notifications, and too many types of notifications"** (Microsoft Tech Community).
  The cost is measured: "**60% of professionals experience high stress and burnout due to excessive online
  communication**" (2024 report, cited via m.io); "important messages easily get lost in the noise,
  leading to missed critical details … errors and rework"; workplace distraction from collaboration-tool
  notifications is estimated to "**cost U.S. businesses $650 billion per year**" (PROVEN-as-reported —
  GiaSpace / m.io / Speakwise roundups, 2024–2026 `[VERIFY]` figures). The vendor "solution" is *more
  configuration* (org-wide notification defaults) — which re-introduces the §1 config-maze trap.
- **Principle violated.** Nielsen **#8 minimalist design** + **#1 visibility-of-system-status** inverted
  (status that shouts everything signals nothing); Myelin **P8 (calm by default; attention is sacred)**.
- **Myelin design rule (falsifiable).** **R-NOISE-1 (one prioritised inbox):** *There is exactly one
  cross-subsystem "what needs *me*" inbox; the default is calm.* FAIL if a sketch shows multiple parallel
  notification streams or per-subsystem inboxes (§5.8). **R-NOISE-2 (opt-in, not opt-out):** *The user
  opts *into* more volume, never out of a firehose* (P8). FAIL if the zero-config default is high-volume.
  **R-NOISE-3 (why-it-fired):** *Every notification states why it fired* (`origin_event` + `reason`,
  R-10/notifications architecture) so it is triageable in one glance — FAIL if a notification has no
  provenance line. **R-NOISE-4 (dedup + storm-control):** *Bursts are deduped/collapsed, not enumerated*
  (ADR-12); FAIL if a 30×-agent-surge would render as N separate inbox rows (R-21 storm state).
- **Surface most at risk (HOUSE STYLE).** **The §5.8 inbox and chat (§7.5)** — and, uniquely for Myelin,
  **agent volume (§6.5)**: agents generate review comments, triage updates, status posts. Myelin's
  *agent-native* bet is the thing most likely to re-create the firehose at 30× scale. **Owned by R-15
  (calm-agent-volume) and R-21 (the storm state); the Zulip-topic contrast from R-01 §3.3 is the
  structural counter.**

### 3.2 Per-comment / per-event notification (the un-batched default)  `[→ at risk: §5.5 review/comments, §7.1 PR]`

- **The trap.** Each inline comment / field change fires its own notification, so a single review or edit
  session emits a *barrage* of pings rather than one coherent event.
- **Where it shows (PROVEN).** R-01 §4.3: GitHub's *un-batched* default is "14 pings" vs. the batched
  "one coherent review"; GitHub's own batching feature exists *because* the per-comment default was the
  failure. The trap is copying a review/comment surface **without** the batching discipline.
- **Principle violated.** Nielsen **#8 minimalist**; Myelin **P8** and **P1** (one comment/thread model →
  batching should feel identical across PR/issue/doc/chat).
- **Myelin design rule (falsifiable).** **R-BATCH-1:** *Review and multi-field edits emit one coherent
  event into the one inbox, not per-comment pings.* FAIL if a sketch's review flow implies per-comment
  notification. **R-BATCH-2:** *Batching is shared* — the same batched-event behaviour applies to PR
  review, issue discussion, and doc comments (§5.5).
- **Surface most at risk.** **PR review (§7.1) and any §5.5 comment surface.** (R-01 §4.3 is the steal;
  this is its trap as a rule.)

---

## 4. THE DUAL-AUDIENCE "SERVES NEITHER" COMPROMISE (§2) — the half-product trap

**Why this is the deepest trap:** §2 names it "the single hardest UX mandate." The market's defining
failure is the **engineering-tool-vs-management-tool split**: Jira/Linear for engineers,
Productboard/Notion/monday for PMs — forcing PMs to **maintain a parallel reality** (`competitive-landscape.md
§3`; design-language §2; `personas.md` P6). There are two opposite ways to fail, and a third (the naive
"middle") that fails worst.

### 4.1 The specialise-and-abandon trap (Linear's deliberate choice, as a warning)  `[→ at risk: issue tracker §7.3, dashboards]`

- **The trap.** Optimise so hard for one audience that the other is *structurally* unserved — Linear
  **deliberately refuses** custom fields, hierarchies, SLAs, audit, roadmaps for non-eng PMs
  (`competitive-landscape.md §3` avoid; R-01 §1.3 trap). Excellent for engineers; **half of Myelin's
  mandate is simply absent.**
- **Where it shows (PROVEN).** Linear "is opinionated to the point of inflexibility for corporate
  workflows" and "**deliberately doesn't serve the corporate/PM-governance end**"
  (`competitive-landscape.md §3`). The inverse is Jira: serves corporate governance, and engineers route
  around it ("most hated," §1).
- **Principle violated.** Myelin **§2 (co-equal audiences)** + **P4**; this trap *is* the §2 failure.
- **Myelin design rule (falsifiable).** **R-DUAL-1 (no abandoned audience):** *Every dual-audience surface
  has a first-class lens for each audience over the same data* (engineer board, PM roadmap, exec rollup) —
  FAIL if a surface serves one audience and offers the other only a degraded or absent view. **R-DUAL-2
  (sketch both lenses):** Phase 6 must render dual-audience surfaces in *both* lenses (same data as
  engineer board AND PM roadmap) — a finalist showing only one lens is **incomplete** on D5 (carried into
  R-16's deferred plan and sketch-funnel §6c).
- **Surface most at risk.** **Issue tracker (§7.3), dashboards, knowledge databases (§7.4).**

### 4.2 The averaged-middle trap — "serves neither"  `[→ at risk: §5.6 views, default density Axis 1]`

- **The trap.** Trying to serve both by building **one averaged UI** — not dense enough for engineers, not
  approachable enough for PMs — so it satisfies neither and feels mediocre to both. The naive resolution of
  the dual-audience tension.
- **Where it shows (HOUSE STYLE, from doctrine).** §2 names "serves neither" as the explicit anti-pattern;
  the "mid-weight" trackers (Shortcut etc.) sit here per `competitive-landscape.md §3` ("limited
  differentiation"). The Sourcehut lesson is the inverse warning (developer-purist alienates non-engineers;
  `§1`). (HOUSE STYLE because it's a synthesised pattern across the doctrine, not one cited product page.)
- **Principle violated.** Myelin **§2** + **P5 (density is earned — for *whom*)** + **P1**.
- **Myelin design rule (falsifiable).** **R-MID-1 (lens, not average):** *Resolve the tension with
  role/density/vocabulary lenses on one component — never one averaged UI.* FAIL if the engineer surface is
  "calmed down" or the PM surface is "densified" into one compromise rather than two tuned configurations
  of the same component (the §5.6/§3.4 density-token mechanism; method #18). **R-MID-2 (neither lens
  degraded):** *Each lens, critiqued against its persona, must read as excellent on its own* (R-16's
  per-lens critique) — FAIL if either lens is only acceptable "given the constraint."
- **Surface most at risk.** **The §5.6 views component and the Axis-1 (density) default decision** — the
  whole funnel must scatter density poles (sketch-funnel Axis 1) precisely so the averaged-middle is
  *visible and rejectable*, not arrived at by accident. **Owned by R-16; informs the funnel's density axis.**

### 4.3 The vocabulary-fracture trap — persona-adaptive labels that split the model  `[→ at risk: IA labels §7.6/R-06, search]`

- **The trap.** Letting persona-adaptive vocabulary ("issue" ↔ "work item" ↔ "deliverable") **fracture
  the shared mental model** so that an engineer and a PM literally cannot talk about the same object, or a
  search/reference resolves differently per persona.
- **Where it shows (HOUSE STYLE).** This is the §9 *open question* in the design language (persona-adaptive
  vocabulary fracturing-risk), not yet a cited incumbent failure — flagged honestly as a Myelin-specific
  risk that *our own* §2 resolution introduces.
- **Principle violated.** Nielsen **#2 match-system-and-real-world** (good) vs **#4 consistency** (the
  risk) — the tension *between* them; Myelin **§2** + **P1**.
- **Myelin design rule (falsifiable).** **R-VOCAB-1 (presentation, not schema):** *Vocabulary is a
  per-space presentation layer over one object identity* — FAIL if a vocabulary choice changes the
  underlying object, its `ArtifactRef`, or how it is referenced/searched (R-06 holds labels in
  tokens/config). **R-VOCAB-2 (bounded, mappable):** *The vocabulary map is a small bounded set with a
  canonical underlying term surfaced on hover/inspect*, so cross-persona conversation never breaks.
- **Surface most at risk.** **IA labelling (R-06) and search (§5.7).** **Owned by R-06 (IA) and R-16
  (vocabulary mapping with fracturing-risk bounded); flagged here as the trap they must close.**

---

## 5. ENTERPRISE-DENSITY-WITHOUT-CALM — the heaviness trap

### 5.1 Density that is heavy, not earned  `[→ at risk: governance/admin §7.6, dashboards, diff/board]`

- **The trap.** Equating "enterprise/serious" with **visual heaviness** — everything bordered, boxed,
  shadowed, badged, colour-filled at once — so a dense surface *shouts* instead of guiding. Distinct from
  §1 (config maze, an IA trap); this is the *visual/interaction* density trap.
- **Where it shows (PROVEN/HOUSE STYLE).** GitLab "**UX can feel dense and slow**"; Teams "**bloated,
  sluggish, confusing**"; Confluence "clunky/dated … slowness" (`competitive-landscape.md §6.1/§4/§5`,
  PROVEN-as-prior-research). The shared tell: density delivered as *weight* (chrome, fills, boxes) rather
  than as *information hierarchy*.
- **Principle violated.** Nielsen **#8 minimalist design**; Myelin **P5 (density is earned)** + **P8
  (calm)** + **§3.5 (borders over shadow; neutral-led, accent-restrained)**.
- **Myelin design rule (falsifiable).** **R-CALM-1 (hierarchy before chrome):** *Visual hierarchy comes
  from weight/colour/spacing on the ramp before size, and from borders before shadow* (§3.4/§3.5) — FAIL
  if a dense surface relies on boxes/shadows/fills to separate content. **R-CALM-2 (functional colour
  only):** *Colour is neutral-led with a restrained functional palette; no decorative or traffic-light
  fills* (§3.2, §8b.3) — FAIL if status is conveyed by background fills rather than glyph+label+position.
  **R-CALM-3 (dense ≠ loud):** *A dense surface (diff, board, log) must pass the "the eye knows where to
  go" check* (rubric D7) — calm is demonstrated, not claimed.
- **Surface most at risk.** **Governance/admin & dashboard surfaces (§7.6) — the surfaces *built* to look
  "enterprise" — plus the diff and board** (the densest engineer surfaces). **Owned by R-13
  (density-made-calm) and R-12 (motion/restraint); feeds rubric D7.**

### 5.2 Status-by-colour-alone — the traffic-light screen  `[→ at risk: CI checks §7.2, PR states §7.1, SLA, agent treatment]`  `[a11y-debt]`

- **The trap.** Encoding state purely as red/amber/green fills (CI, PR state, SLA breach, agent
  treatment) — the "traffic-light screen" — which both *shouts* (§5.1) and *excludes* colour-blind users.
- **Where it shows (PROVEN).** R-01 §4.4 flagged GitHub's checks panel risk; it is endemic across
  enterprise dashboards. It is simultaneously a **calm trap** (§5.1) and a **G1 hard-gate failure**.
- **Principle violated.** Nielsen **#4 consistency** + WCAG **1.4.1 Use of Colour** (PROVEN — WCAG 2.1/2.2
  AA); Myelin **§8b.3 (status never by colour alone)** + **P8**. `[a11y-debt]` — this is a **rubric G1
  gate failure**, not just a taste issue.
- **Myelin design rule (falsifiable).** **R-COLOUR-1:** *Every status carries glyph + label/position in
  addition to colour* (§8b.3, WCAG 1.4.1) — FAIL on G1 if any CI/PR/SLA/agent status is colour-only.
  **R-COLOUR-2 (shared functional palette):** *"red means trouble" reads identically across CI, issues,
  and chat unfurls* from one functional palette (§3.2, P1).
- **Surface most at risk.** **CI checks (§7.2), PR states (§7.1), SLA breach (issues), and the `agent`
  treatment (§3.2/§6).** **Enforced by R-17 + rubric G1; the agent-treatment colour-blind-safety is R-14.**

---

## 6. The cross-incumbent synthesis (what the traps teach together)

| Trap family | The incumbent exemplar | Root mechanism | The Myelin principle at stake | The surface most at risk |
|---|---|---|---|---|
| **Config maze** (§1) | Jira | Unbounded flat config space; no disclosure floor | **P4** (disclosure done wrong), **P8** | Issue tracker (§7.3), create forms, §5.6 views |
| **Stitched seams** (§2) | Atlassian suite | Per-product identity/permission/UI; agent sprawl | **P1** (the defining bet), **P7/P9** | PR context pane (§7.1), shell (§5.1), chip/search, agent console |
| **Notification overload** (§3) | Teams (all) | Every event pushes; no unified priority/dedup | **P8** | §5.8 inbox, chat (§7.5), **agent volume §6.5** |
| **Serves-neither** (§4) | Jira↔Linear split | Specialise-and-abandon / averaged middle | **§2**, **P5** | Issue tracker (§7.3), §5.6 views, density default |
| **Density-without-calm** (§5) | GitLab/Teams/Confluence | Density as weight + colour-only status | **P5**, **P8**, **§8b.3** | Admin/dashboards (§7.6), diff/board, CI checks |

**The meta-lesson (HOUSE STYLE).** Every trap is the **shadow of a Myelin strength**: Myelin's *depth*
(P4) shadows into the config maze; its *unification* promise makes the stitched-seam regression the most
embarrassing failure it could commit; its *agent-native fabric* (P7) is the single biggest new sprawl-and-
firehose risk no incumbent had at this scale; its *dual-audience* mandate (§2) is the serves-neither trap
by construction. **The architecture that gives Myelin its wedge is the same architecture that makes these
specific traps *easy*.** That is why each rule below is falsifiable: the danger is not ignorance of the
traps but quietly re-committing them while believing the architecture immunises us. It does not — only the
*design rules*, checked on every sketch, do.

---

## 7. How this maps to the rubric & the sketch-funnel axes (actionability)

**Rubric anchors this item equips (made checkable):**
- **D7 (density-made-calm) — primary.** §5 is the negative definition of D7: a "0" on D7 *is*
  §5.1 (heavy) + §5.2 (traffic-light). R-CALM-1/2/3 and R-COLOUR-1/2 are the FAIL conditions a D7 score of
  0–1 must trigger. §3 (notification overload) sets the "agent volume kept out of the main timeline / one
  prioritised inbox / no firehose" half of D7's anchor.
- **D10 (the switch test) — primary.** Every trap is a "wall" a switching team would hit. R-CFG (could a
  team adopt without a full-time admin?), R-SEAM (does it feel like one product or five?), R-NOISE (does
  the inbox survive real volume?), R-DUAL (can *both* my PMs and engineers move?) are the switch-test
  failure modes operationalised. A sketch that re-commits any §1–§5 trap **fails the switch test on that
  axis** (D10 = 0–1).
- **D4 (one-product coherence).** §2 (stitched seams) is the *negative* of D4 — R-SEAM-1…4 are D4's FAIL
  conditions.
- **D6 (agent legibility) & D9 (sovereignty).** §2.3 (agent sprawl, `[gov-risk]`) and §2.2 (cross-product
  leak, `[gov-risk]`) are D6/D9 FAIL conditions; R-AGT/R-LEAK feed them.
- **G1 (hard gate).** §5.2 (status-by-colour-alone, `[a11y-debt]`) is a direct G1 failure — R-COLOUR-1 is a
  G1 PASS/FAIL line, not a D-score nuance.

**Sketch-funnel axes this item grounds:**
- **Axis 1 (density: dense ↔ calm).** §5 (density-without-calm) and §4.2 (averaged middle) are *why* the
  funnel must scatter the density poles ≥2× each: so the heavy pole and the averaged-middle are made
  visible and rejectable, not arrived at by default.
- **Axis 3 (unification ↔ distinct-per-surface) — the central problem.** §2 (stitched seams) is the
  *failure* at the distinct-per-surface extreme; R-SEAM-1…4 bound how far Axis 3 can go toward
  distinctness before it fractures into the Atlassian trap. **The distinct-per-surface pole must never
  cross into separate identity/permission/grammar** (R-SEAM).
- **Axis 5 (agent presence).** §2.3 (sprawl) + §3.1 (agent volume firehose) are why the foregrounded-agent
  pole must be governed/calmed; they bound the foregrounded extreme.
- **Axis 6 (sovereignty visibility).** §2.2/§2.3 (`[gov-risk]`) define what the on-demand-console pole must
  still guarantee (one enforcement point; provable provenance).

---

## 8. Completeness-critic (§9) gloss-risks this item touches

R-02 is a trap register, so it **owns the *anti-pattern* framing of several gloss-risks and routes the
state-craft to the owners** (honest scoping per standing instructions):

- **Permission-denied "no access" card (never a leaked title).** Owned as a *trap-and-rule* here
  (R-LEAK-1/2, §2.2) — the falsifiable rule; **routed to R-09** for the state-craft.
- **Erased / tombstoned.** Touched via R-LEAK (a reference must degrade, not leak, on erasure);
  **routed to R-09/R-21** for the state.
- **Storm / 30×-agent-surge.** Owned as the *trap framing* (R-NOISE-4, §3.1) — "a 30×-surge must not
  render as N rows"; **routed to R-21** for the state and **R-15** for calm-volume patterns.
- **Status-not-by-colour-alone.** Owned as a *falsifiable G1 rule* here (R-COLOUR-1, §5.2);
  **enforced by R-17 + rubric G1.**
- **Partial-failure agent branches** (gate rejected/agent error/budget/loop-guard). Touched via R-AGT
  (born-governed, provable provenance); **routed to R-04/R-14/R-15** for the flows/states.
- **Cross-cell / cross-tenant reference → no-access/tombstone.** Touched via R-LEAK-1; **routed to R-09.**

**Consciously deferred (with reason):** the *full per-surface state set* (that is R-21's owned
deliverable), the *editor `contenteditable` variance* trap (R-01 §2.1 named it; R-10 owns the editor), and
*device/touch/mobile* glosses (R-13/R-21, design-language §8b.4) — naming-and-routing keeps the corpus
cumulative rather than duplicating downstream owners.

---

## 9. Uncertainties & `[VERIFY]` register (dated 2026-06-20)

| Claim | Status | Re-verify before |
|---|---|---|
| Jira enforcing hard data limits (700 custom fields/instance; per-project caps) from ~Feb 2026. | PROVEN-as-reported (Atlassian guardrails docs/community 2025–2026); **`[VERIFY]` exact dates/limits**. | Phase 7 external use. |
| "Jira most-hated tool, more criticism than next 4 combined" (2025 dev survey). | PROVEN-as-reported (Developer-First roundup, 2025). | Phase 7. |
| "Jira Admin = full-time role" framing. | PROVEN-as-reported (Linear-vs-Jira analyses 2025/2026); widely echoed, but it's a characterisation. | — (characterisation, stable). |
| Atlassian cloud-only / Data-Center EOL forcing migration; ~28% cost increase. | PROVEN-as-reported (The Register 2025-09); **`[VERIFY]` exact EOL dates/figures**. | Phase 7. |
| Atlassian "agent sprawl"; Rovo governance retrofits (separated permissions, org-wide agent lists). | PROVEN-as-reported (Atlassian Rovo blog / Team '26 2026; Deviniti); **`[VERIFY]` feature names/scope**. | Phase 7. |
| Teams notification-overload figures (60% burnout; $650B/yr; "too many types" thread). | PROVEN-as-reported (m.io/GiaSpace/Speakwise/MS community 2024–2026); **`[VERIFY]` figures**. | Phase 7. |
| Cross-product permission-leak trap (§2.2). | **HOUSE STYLE** — structural inference from §2.1 + R-01's cited North-Star leak instances; not a single cited Atlassian incident. | — (flagged as inference). |
| Averaged-middle (§4.2) and vocabulary-fracture (§4.3) traps. | **HOUSE STYLE** — synthesised from doctrine (§2, §9 open question); §4.3 is a Myelin-introduced risk, not an incumbent's. | — (flagged). |

**Honest limitation.** Like R-01, this is an **expert teardown from public docs, vendor changelogs,
community threads, and analyst roundups — not hands-on testing with live enterprise instances** (no
account access in this autonomous run). Behaviour/sentiment claims are grounded in cited 2024–2026 sources
and the prior `competitive-landscape.md` research; the falsifiable design rules are HOUSE-STYLE synthesis
*from* that evidence. **R-02 has `user-dep: none`** — there is no `[DEFERRED-UNTIL-USERS]` plan; the no-user
expert-audit substitute *is* the deliverable. (The rules themselves become *validated* only when Phase-6
sketches are run against them in the rubric, and ultimately when real users test the sketches.)

---

## 10. Sources (web-verified, 2024–2026)

- Jira admin complexity / field & workflow accumulation: https://community.atlassian.com/forums/App-Central-articles/10-Most-Common-Challenges-Every-Jira-Admin-Faces-Daily-And-How/ba-p/2941917 · https://community.atlassian.com/forums/App-Central-articles/How-to-Audit-and-Reduce-Custom-Fields-in-Jira-Before-the-700/ba-p/3202130 · https://support.atlassian.com/jira-cloud-administration/docs/data-limits-and-guardrails/ · https://www.salto.io/blog-posts/jira-resolutions-2025
- Over-complex forms (15 fields for 3-field job): https://community.atlassian.com/forums/App-Central-articles/Top-7-Jira-Admin-Mistakes-and-How-to-Avoid-Them/ba-p/3115370
- Jira "most hated tool" / Jira-Admin-as-full-time-role / Linear-vs-Jira: https://developerfirst.substack.com/p/developer-first-163-developers-most · https://tech-insider.org/linear-vs-jira-2026/ · https://www.eesel.ai/blog/linear-vs-jira · https://monday.com/blog/rnd/linear-or-jira/
- Atlassian cloud-only / Data-Center EOL / migration & integration pain: https://www.theregister.com/2025/09/09/atlassian_will_go_cloudonly_customers/
- Atlassian "agent sprawl" / Rovo governance / connector sprawl / "no one can prove what data AI touched": https://www.atlassian.com/blog/rovo/ai-agents-in-jira · https://deviniti.com/blog/enterprise-software/38-atlassian-ai-statistics-for-2026-rovo-atlassian-intelligence-adoption/ · https://siliconangle.com/2026/05/06/atlassian-opens-teamwork-graph-pushes-rovo-agentic-execution-team-26/ · https://community.atlassian.com/forums/Atlassian-AI-Rovo-discussions/Discussing-governance-for-Rovo-agents-within-Automation-flows/td-p/3239485
- Teams notification overload / "too many notifications and too many types" / cost figures: https://techcommunity.microsoft.com/discussions/microsoftteams/teams-has-too-many-notifications-and-too-many-types-of-notifications/1652834 · https://www.m.io/blog/notification-overload · https://www.giaspace.com/the-hidden-costs-of-microsoft-teams-overload/ · https://speakwiseapp.com/blog/microsoft-teams-statistics
- WCAG 1.4.1 Use of Colour (G1 anchor): https://www.w3.org/WAI/WCAG21/Understanding/use-of-color.html

---

## 11. Self-check against acceptance criteria

1. **Each trap maps to a specific violated principle AND a specific Myelin surface-at-risk.** ✔ — every
   §1–§5 entry has a "Principle violated" line (Nielsen # + Myelin P) and a "Surface most at risk" line;
   the §6 synthesis table makes both columns explicit per family.
2. **The register is phrased as falsifiable design rules Phase 5/6 can be checked against.** ✔ — every entry
   carries `R-*` rules with explicit **FAIL conditions** (PASS/FAIL phrasing, e.g. R-CFG-1, R-SEAM-1,
   R-NOISE-1, R-COLOUR-1). The §0 falsifiability convention states the discipline.
3. **No trap is a generic complaint ("Jira is bad").** ✔ — each trap names a *specific interaction/IA
   mechanism* (config accumulation, 15-field forms, per-product permission models, agent inventory,
   per-comment pings, traffic-light fills), not a brand verdict; sentiment is cited only as *evidence the
   mechanism exists*.
4. **Reuses R-01's teardown method/format.** ✔ — §0 mirrors R-01 §0 with the inverse per-entry structure;
   entries cross-cite R-01 by section (§1.3, §2.4, §3.1, §3.3, §4.1/§4.3/§4.4) rather than re-deriving.
5. **Feeds rubric D7/D10 anchors + the completeness-critic.** ✔ — §7 maps every trap family to D7
   (primary), D10 (primary), D4, D6, D9, and G1 with explicit FAIL conditions; §8 covers the §9
   gloss-risks (owns the anti-pattern framing of no-access leak, storm, colour-only status; routes state-
   craft to R-09/R-15/R-21).
6. **PROVEN/HOUSE-STYLE tagging + date + `[VERIFY]`.** ✔ — claims tagged inline; §9 register dates and
   `[VERIFY]`-flags every time-sensitive incumbent feature; HOUSE-STYLE inferences (§2.2, §4.2, §4.3)
   flagged explicitly; file dated 2026-06-20.
7. **No-user handling.** ✔ — R-02 is `user-dep: none`; §9 states there is no `[DEFERRED-UNTIL-USERS]` plan
   and that the rules are validated downstream (sketches → rubric → eventual user tests), never presented
   as already-validated.
8. **Web-grounded with cited URLs.** ✔ — §10 lists current (2024–2026) sources for every incumbent trap;
   `ToolSearch select:WebSearch,WebFetch` used to ground Jira/Atlassian/Teams claims.

**Partial / honestly noted:** evidence is public-source expert audit, not hands-on enterprise testing
(§9 limitation); several cited figures (700-field limit dates, $650B, 28% cost, agent-governance feature
names) carry `[VERIFY]` for Phase 7; the §2.2 cross-product-leak and §4.2/§4.3 traps are HOUSE-STYLE
inferences flagged as such.
