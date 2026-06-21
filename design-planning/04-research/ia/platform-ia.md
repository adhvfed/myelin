# R-06 — Platform IA & the "One Shell" Unification Model

> **Phase 4 research corpus** · deliverable of prompt **R-06** (workstream
> [`ws-c`](../../02-research-roadmap/), Seq #6). **File date: 2026-06-20.**
> Method **#6 (expert-led IA design — ADOPT)**. This file is the **structural answer to the
> central problem** — "one product, five surfaces" — and the **shared substrate** R-07 (card-sort/
> tree-test), R-08 (palette), R-09 (chip/unfurl), R-10 (views/editor/inbox), R-18 (i18n labels)
> critique and build against. It does **not** re-derive design-language §5.1 (nav shell) or §7
> (view catalogue); it **composes them into one tree, one object model, one address space**.
>
> **Builds ON prior `04-research`:** [R-01 teardown-dossier](../north-star/teardown-dossier.md)
> (North-Star IA patterns + their traps) and [R-04 cross-surface-flows](../jtbd-flows/cross-surface-flows.md)
> (the six named flows this IA must let users *complete without a tab-switch*). Where this file
> says "the IA must support flow F-X", that is R-04's flow, not re-stated here.
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = cited standard / an existing architecture
> contract we *surface* (ADR-13 addressing, §5.1 shell, §7 catalogue, ADR-03/06/07) — not invented.
> **HOUSE STYLE** = our design synthesis / taste. No part of this IA is user-validated; the
> validation is R-07's `[DEFERRED-UNTIL-USERS]` card-sort + tree-test, summarised in §9 here.

---

## 0. How to read this file

Seven parts, each a concrete artifact (not a principle):

1. **§1 The unification thesis** — the one sentence the whole IA defends, and the five collapses.
2. **§2 The unified object/navigation model** — the one tree every subsystem hangs in, drawn as an
   actual tree (the acceptance-criterion deliverable).
3. **§3 The shell regions** — primary nav · contextual sidebar · content · context pane — each as a
   concrete spec with what it owns, what fills it per subsystem, and the rules.
4. **§4 The five global surfaces** — palette, search, inbox, identity/scope, agent-governance — the
   cross-cutting layer that is *the same everywhere*.
5. **§5 The `ArtifactRef` / URL address space** — `myelin://…` and the web URL, down to sub-artifact,
   one scheme for all five subsystems (PROVEN: ADR-13).
6. **§6 Labelling & persona-adaptive vocabulary** — the taxonomy, the "issue ↔ work item" mapping,
   the fracturing-risk flagged (§9 open question), labels held in tokens/config so they are
   cheap-to-change and tree-testable.
7. **§7 Per-role default-landing map** — where each persona lands, deep-linked.

Then **§8** maps every §7-catalogue surface into the tree (coverage proof), **§9** the
tree-test handoff to R-07, **§10** rubric/funnel actionability, **§11** completeness-critic,
**§12** self-check.

---

## 1. The unification thesis (the one sentence the IA defends)

> **Every Myelin surface is a *view onto one addressable artifact graph*, rendered inside *one
> shell*, addressed by *one `ArtifactRef` scheme*, scoped by *one `Principal`/scope selector*, and
> reachable from *one palette/search/inbox*. "Five surfaces" is a *navigation/density facet of one
> object model* — not five products behind one login.** *(HOUSE STYLE thesis; mechanically enabled
> by the PROVEN three glue contracts, ADR-13 / system-overview §1.)*

This is the IA embodiment of the architecture's own one-paragraph claim: "the five subsystems are
*one product by construction*" because the shared layer is the substrate, not a bridge
(system-overview §1, PROVEN). The IA's job is to make that *construction* legible: a user must
never have to know *which subsystem* owns a thing to find it, address it, or act on it.

### 1.1 The five collapses (the prompt's core ask)

Each subsystem has its own native 3-level spine. They are **isomorphic** — the same
*container → item → sub-item* shape — which is *why* one object model and one address scheme fit
all five. This isomorphism is the load-bearing IA observation (HOUSE STYLE; the shapes are PROVEN
from the §7 catalogue and the deep-dives).

| Subsystem | Container (L1) | Item (L2) | Sub-item / sub-artifact (L3) | The native idiom |
|---|---|---|---|---|
| **Code** | `repo` | `PR` (or commit, branch, file) | `diff-line` / `PR-comment` / `file@SHA` | `repo → PR → diff` |
| **Knowledge** | `space` | `page` (or database) | `block` / `db-row` / `db-view` | `space → page → block` |
| **Chat** | `channel` | `thread` (or message) | `message` / `unfurl` | `channel → thread → message` |
| **CI** | `pipeline`/`repo` | `run` | `job → step` / `log-line` / `artifact` | `run → job → step` |
| **Issues** | `project`/`team` | `issue` | `sub-issue` / `comment` / `field` | `issue → sub-issue` |

**The collapse (HOUSE STYLE):** these five are **one generic shape** —
`Scope → Container → Item → Sub-artifact` — and therefore:
- **one navigation model** (§2): pick a scope, pick a container in the contextual sidebar, open an
  item in content, drill to a sub-artifact (which is itself addressable and referenceable);
- **one address scheme** (§5): `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]` (PROVEN, ADR-13)
  expresses every level of every subsystem with the *same* grammar;
- **one views component** projects the *Container→Item* level identically for Issues and Knowledge
  databases (the ADR-06 reuse boundary, §5.6) — the same table/board/list over `issue` items or
  `db-row` items;
- **one reference chip/unfurl** (R-09) renders *any* level of *any* subsystem because all are
  `ArtifactRef`s — the wedge is *the address scheme made visible*.

> **What this is NOT (the trap, from R-01 §5):** the collapse is **not** "flatten everything into
> one undifferentiated list." A diff still *feels* like code; a roadmap still *feels* like a
> roadmap (R-07 owns the per-surface distinctness ruling, Axis 3). The collapse is at the
> **object/address/navigation** layer; **earned per-surface density** lives on top (P5). Confusing
> the two is exactly the over-unification failure R-07 must guard. *(HOUSE STYLE.)*

---

## 2. The unified object / navigation model (the concrete tree)

The navigation model is a **scope-rooted forest**: one tenant root, scoped by org/team/project/
space, under which every subsystem's container hangs. Drawn as the actual tree (acceptance
criterion: "a concrete tree, not just the principle"). **`[G]` = a global surface** (§4) that is
scope-aware but not *inside* one subsystem; **`[A]` = admin/governance** (one layer down, P4).

```
myelin (tenant root — the scope selector sets the active subtree, §4.4)
│
├─ [G] Home / "My Work"            ← per-role default-landing target (§7); cross-subsystem
├─ [G] Inbox  ("what needs me")    ← §4.3 — the one notifications surface, all 5 subsystems
├─ [G] Search (full view)          ← §4.2 — the palette's heavyweight sibling
│
├─ Code  (subsystem area)
│   └─ <repo>                       ← Container (contextual sidebar = repo list / repo tree)
│       ├─ Code (file tree @ ref)   → <file@SHA> ──#─ <line-range>      (sub-artifact)
│       ├─ Pull requests
│       │   └─ <PR>                 → Overview · Diff/files · Review · Checks · Conversation
│       │       └─ <diff-line> · <PR-comment>                          (sub-artifact)
│       ├─ Branches / Tags / Compare
│       ├─ Code search
│       └─ [A] Repo settings · Branch-protection / rulesets
│
├─ CI  (subsystem area)
│   └─ <pipeline | repo runs>       ← Container (contextual sidebar = run list, filterable)
│       └─ <run>                    → DAG · Matrix · Logs
│           └─ <job> → <step> ──#─ <log-line> · <artifact>             (sub-artifact)
│       ├─ Environments & deployments  (HITL approvals queue, §6.3)
│       ├─ Pipeline definition editor
│       └─ [A] Secrets · Usage/quota
│
├─ Issues  (subsystem area)
│   └─ <project | team>             ← Container (contextual sidebar = views tree, §5.6)
│       ├─ Views: List · Board · Table · Timeline/Roadmap · Calendar   (one views component)
│       ├─ Cycle (sprint) view · Triage inbox
│       └─ <issue>                  → body · properties · relations · activity
│           └─ <sub-issue> · <comment> · <field>                       (sub-artifact)
│       ├─ Roadmap / Portfolio  (PM/exec lens — same records, §6/R-16)
│       ├─ Dashboards · Saved views
│       └─ [A] Workflow / SLA / field-scheme admin
│
├─ Knowledge  (subsystem area)
│   └─ <space>                      ← Container (contextual sidebar = space→page tree)
│       └─ <page | database>        → editor (§5.9)  |  db views (§5.6)
│           └─ <block> · <db-row> · <db-view>                          (sub-artifact)
│       ├─ Backlinks / references panel  (reference graph made visible, P6)
│       ├─ Page history · Templates
│       └─ Sharing & permissions · Export
│
├─ Chat  (subsystem area)
│   └─ <channel | DM>               ← Container (contextual sidebar = channel/DM list)
│       └─ <thread | message>       → timeline · composer · unfurls · thread pane
│           └─ <message> · <unfurl> · HITL approval card (§5.4)        (sub-artifact)
│       └─ Activity / mentions      (feeds the global Inbox)
│
└─ [A] Platform / Governance  (cross-cutting admin, §4.5 — P4: one layer down)
    ├─ Identity & profile  (theme/density/locale/notif prefs · sessions · tokens · keys · MFA)
    ├─ Org / team / project / space admin  (SSO/SCIM)
    ├─ Permission / role management (RBAC face over ReBAC, ADR-03)
    ├─ Agent governance console  (agents · scopes · budgets · autonomy policy · kill-switch)
    ├─ Audit-log explorer  (every human + agent action, correlation-threaded)
    ├─ GDPR / data-rights console  (DSR orchestrator — DPO P13's surface)
    ├─ Data map / RoPA & residency console  ("where does this data live")
    ├─ Tenant / cell & residency settings
    ├─ Onboarding & empty-platform flows
    └─ Billing / usage / export & exit
```

**Reading the tree (the navigation rules, HOUSE STYLE unless cited):**

1. **Depth ≤ 4 to any item; ≤ 5 to any sub-artifact.** The tree is deliberately shallow — the
   NN/g tree-test "sweet spot" is 3–4 levels (PROVEN — [NN/g, Card Sorting vs. Tree
   Testing](https://www.nngroup.com/articles/card-sorting-tree-testing-differences/)). This makes
   the IA *directly tree-testable* in R-07 without restructuring (acceptance criterion).
2. **The subsystem area is a *facet*, not a destination.** A user rarely "goes to Issues" as a
   first act; they go to *their work* (`[G] Home`) or *follow a reference* into a specific issue.
   The subsystem L1 is the **browse fallback**, the palette/inbox/chip are the **primary entry**
   (this is the R-01 §1.1 lesson — Linear's palette-first nav — applied platform-wide).
3. **Every node is an `ArtifactRef` and therefore deep-linkable + referenceable** (§5; PROVEN
   ADR-13). The tree is *isomorphic to the address space* — there is no navigable place that
   cannot be linked, mentioned, or unfurled.
4. **`[A]` governance is one layer down per subsystem AND consolidated at the root.** A repo's
   branch-protection editor lives under `<repo>`; the platform-wide governance consoles live under
   `[A] Platform`. Same surfaces, two entry depths — depth-by-role (P4). This is the explicit
   counter to Jira's config-maze (R-01 / R-02): governance is *reachable*, never *imposed*.

---

## 3. The shell regions (primary nav · contextual sidebar · content · context pane)

Builds **ON** design-language §5.1 (do not re-derive); here each region is pinned to a concrete
role in the §2 tree, with what fills it per subsystem and the binding rules. This is the
"one skeleton five subsystems compose into" made specific. *(Structure PROVEN from §5.1; the
per-region fill mapping is HOUSE STYLE synthesis.)*

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│  TOP BAR (global, constant): scope selector (§4.4) · ⌘K palette (§4.1) · search    │
│            · Inbox (§4.3) · identity/agent badge (§4.4) · residency cue (P9)        │
├───────────┬──────────────────────────┬──────────────────────────┬─────────────────┤
│ PRIMARY   │  CONTEXTUAL SIDEBAR       │  CONTENT                  │  CONTEXT PANE    │
│ NAV       │  (current subsystem's     │  (the open item / view)   │  (right; refs +  │
│ (rail)    │   tree/list)              │                           │   details;       │
│           │                           │                           │   collapsible)   │
│ Home      │  Code: repo list/tree     │  the diff, the board,     │  linked artifacts│
│ Inbox     │  CI:   run list           │  the page, the timeline,  │  (chips/unfurls, │
│ Search    │  Issues: views tree       │  the channel timeline     │   §5.3/R-09),    │
│ ─────     │  Know.: space→page tree   │                           │  details,        │
│ Code      │  Chat: channel/DM list    │                           │  activity,       │
│ CI        │                           │                           │  agent panel,    │
│ Issues    │  (Issues+Knowledge share  │                           │  audit/provenance│
│ Knowledge │   the §5.6 views tree     │                           │  (P7/P9)         │
│ Chat      │   shape — the reuse seam) │                           │                  │
│ ─────     │                           │                           │                  │
│ [A] Admin │                           │                           │                  │
└───────────┴──────────────────────────┴──────────────────────────┴─────────────────┘
```

### 3.1 Primary nav (the rail) — *what subsystem/area am I in*
- **Owns:** the subsystem/area switcher (`Code · CI · Issues · Knowledge · Chat`) + the three
  global surfaces hoisted above them (`Home · Inbox · Search`) + the `[A] Admin` entry at the
  bottom. *(§5.1: "primary nav: subsystem/area switcher".)*
- **Rule (HOUSE STYLE):** the rail is **constant across all five subsystems** — switching never
  re-skins the rail. This is the single most visible coherence cue (rubric D4). The active subsystem
  is indicated by selection state, never by a different rail.
- **Axis-2 note (sketch-funnel):** the rail is the **persistent-rail pole**; the palette (§4.1) is
  the **command-palette-led pole**; the context pane following a reference is the **contextual
  pole**. A finalist may *visually* shrink the rail toward palette-led, but the *object model*
  underneath is the same tree. (R-01 §1.1 trap: a maximally palette-led shell still owes a
  pointer-discoverable rail for non-keyboard PMs — P3's second half.)

### 3.2 Contextual sidebar — *what containers/items exist here*
- **Owns:** the current subsystem's container/item tree — repo list/tree (Code), run list (CI),
  **the views tree** (Issues), space→page tree (Knowledge), channel/DM list (Chat). *(§5.1:
  "contextual sidebar: the current subsystem's tree/list".)*
- **The reuse seam (PROVEN, ADR-06):** Issues' views tree and Knowledge's database views tree are
  **the same §5.6 views-component navigation** over different item types (`issue` vs `db-row`) —
  this is where "Issues ↔ Knowledge, the biggest reuse boundary" (system-overview §4) becomes an
  *IA* fact, not just a component fact. R-10 specs the component; here it is one sidebar shape.
- **Rule:** subsystems own *only* their sidebar + content (§5.1). They do **not** own the rail,
  top bar, or context pane — those are shell-owned and identical everywhere (the mechanical
  coherence guarantee, P1).

### 3.3 Content — *the open item*
- **Owns:** the item/view itself: the diff, the board, the page, the channel timeline. This is the
  **only region that earns per-surface density/distinctness** (P5; R-07 Axis-3 ruling). The diff is
  dense and keyboard-forward; the roadmap is spacious and chart-forward — *same shell, tuned
  content* (rubric D4: "per-surface density is tuning of shared components, not a fork").

### 3.4 Context pane — *what this connects to* (the wedge surfaces here)
- **Owns:** the right-hand, collapsible pane where **cross-artifact references surface** —
  the PR's linked issue/doc/run, a page's backlinks, an issue's linked PRs — rendered as §5.3
  chips/unfurls (R-09), plus details, activity, the **agent panel** (proposed effects / provenance,
  §6), and the **audit/provenance link** (P7/P9). *(§5.1: "optional right-hand context pane where
  cross-artifact references and details surface".)*
- **Why it is structurally central (HOUSE STYLE):** the context pane is **where the wedge is felt**
  (R-01 §4.1 PR-context-pane; R-04 F-ENG-1/F-ENG-2; R-22). The PR-context-pane (system-overview
  §8.1, the wedge flagship) **is** this pane populated by the reference graph. The pane is the same
  component everywhere — what differs is which `ArtifactRef`s the open item resolves into it.
- **Rule:** the context pane is **shell-owned and reference-graph-driven**, never a per-subsystem
  bespoke panel. A PR's "linked issue" and a doc's "backlink" render through the *same* pane and the
  *same* chip — that identity is the coherence test.

### 3.5 The shell containment mandates (carried from §8b.4, PROVEN bugs)
The shell **must** design around these net-new bug classes (binding for every sketch): pin the
shell to the viewport (`100vh`/`overflow:hidden`); each region owns its own scroller; a flex child
that scrolls needs `min-height:0` + overscroll-contain; collapse the contextual sidebar / context
pane at the breakpoint (`width:100%` is not a takeover); the context pane and contextual sidebar
become **mobile drawers** (toggle + backdrop + Escape + route-change auto-close). *(All PROVEN —
§8b.4; named here because the shell is where they bite first.)*

---

## 4. The five global surfaces (the cross-cutting "same everywhere" layer)

These are **not** in any subsystem — they are the shell's cross-cutting surfaces, scope-aware,
identical across all five subsystems. They are *how the platform stays one product even as content
diverges* (rubric D4). Each is specced in depth downstream; here is its **IA role + addressing +
the one rule that keeps it unified**.

### 4.1 Command palette (`⌘K`) — *the universal verb + jump surface*
- **IA role:** the primary entry into the §2 tree (navigate to any container/item/sub-artifact) AND
  the action surface (create/transition/open/insert) AND search entry. **One palette, every screen**
  (§5.2; R-01 §1.1). Owned in full by **R-08**.
- **Unification rule (PROVEN, ADR-07/ADR-08):** palette filters compose **the same query AST** as
  saved views and agent triggers, and palette actions are **the same typed `ToolDef`s** agents use.
  One verb vocabulary across human-in-palette, human-in-chat (slash-commands), and agent.
- **IA dependency:** the palette can only jump to / act on `ArtifactRef`s that exist in §5 and are
  permission-visible (ADR-03 `list-objects` pre-filter) — *the palette navigates the tree; it does
  not invent a parallel one.*

### 4.2 Search (full view) — *the palette's heavyweight sibling*
- **IA role:** cross-artifact, type/subsystem-faceted, multilingual search over the *entire* §2
  tree; a destination node (`[G] Search`) and the palette's overflow. Owned by **R-08** + §5.7.
- **Unification rule (PROVEN, ADR-03):** **permission-pre-filtered, never post-filtered** — "you
  can only find what you may see," surfaced as a UX guarantee (P9). Results are `ArtifactRef`s
  rendered as the same chips (R-09) — search results *are* navigable tree nodes, not a separate
  list format.

### 4.3 Inbox ("what needs me") — *the one prioritised cross-subsystem queue*
- **IA role:** the single aggregation of mentions, review requests, assignments, SLA warnings, HITL
  approvals, CI failures on my work, agent proposals — across all five subsystems (§5.8; consumes
  the bus). A global node (`[G] Inbox`) and a per-role landing target (§7). Owned by **R-10**.
- **Unification rule (PROVEN — §8b.5 humanised strings + `origin_event`+`reason`):** every item
  carries **"why am I getting this"** provenance from the notifications system, and one read-state
  truth across views. Calm-by-default; agent volume routed out of the main stream (P8/§6.5).
- **Why it's a *global* surface, not Chat's:** notifications span CI/Issues/Code/Knowledge/Chat —
  binding it to one subsystem would re-fracture the product. It is the **anti-firehose** spine.

### 4.4 Identity & scope selector — *who/what am I, and in what scope*
- **IA role:** the top-bar pair: (a) the **scope selector** (org → team → project/space/channel)
  that sets the active subtree of §2 and carries the **residency/region cue** (P9), and (b) the
  **identity/agent badge** (the current `Principal`, with the agent treatment if acting as/with an
  agent). *(§5.1: "current Principal's identity menu, tenant/space context, org/team/project scope
  indicator which doubles as a residency/visibility cue".)*
- **Unification rule (PROVEN, ADR-13):** **one `Principal` renders the same identity badge
  everywhere; one scope selector governs all five subsystems' sidebars.** Switching scope re-roots
  every subsystem's contextual sidebar at once — you do not set "team" five times. This is the
  single-identity / single-scope embodiment of the central problem.
- **Sovereignty hook (P9; R-19 owns depth):** the scope indicator is *the* always-on residency cue
  ("this scope's data lives in `eu-west`") — sketch-funnel **Axis 6** lives partly here.

### 4.5 Agent presence / governance — *the agent layer, legible everywhere*
- **IA role:** two faces of one thing. **(a) Ambient:** the agent badge/treatment on any
  `Principal` and the agent panel in the context pane (§3.4) — agents are visible *in place*
  (P7/§6.1). **(b) Governed:** the `[A] Agent governance console` (which agents exist, scopes,
  budgets, autonomy policy, **kill-switch**) — one layer down. Owned by **R-14/R-15**.
- **Unification rule (PROVEN, ADR-08):** an agent is a first-class `Principal` in the *same* tree
  and address space as humans and services — its proposals, attribution, and audit thread through
  the *same* surfaces (inbox, context pane, audit explorer), never a bolted-on bot console.
  Sketch-funnel **Axis 5** (agent ambient ↔ foregrounded) is a tuning of how loud this layer is,
  not whether it exists.

> **The global-surface invariant (HOUSE STYLE, rubric D4):** these five surfaces are **rendered by
> the shell, never by a subsystem.** A reviewer's coherence test: *open the same palette / inbox /
> identity badge / chip in Code and in Chat — they must be the identical component.* If any
> subsystem ships its own palette, its own inbox, or its own identity badge, the product has
> fractured (the Atlassian "stitched" failure, R-01 §4.1 / R-02).

---

## 5. The `ArtifactRef` / URL address space (down to sub-artifact)

**One scheme, all five subsystems, every level.** *(PROVEN — ADR-13: every artifact is addressable
as `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`, resolvable to current projection +
permission check + update events; "down to sub-artifact granularity — a PR comment, a doc block, a
CI step".)* This is *the* unification mechanism: the chip you see (R-09), the handle you paste in
the CLI (§7.7), and the URL in the browser are **the same identity** (P1/P6).

### 5.1 The canonical grammar

```
myelin://<tenant>/<subsystem>/<type>/<id>[#<sub-path>]
        └─scope──┘ └─area────┘ └object┘       └─sub-artifact (the [#sub] of ADR-13)
```

- `<tenant>` — the scope root (also the residency/routing key; PROVEN — every record carries
  `tenant`+`region`, system-overview §2 / ADR-11).
- `<subsystem>` — `code | ci | issues | knowledge | chat` (+ `platform` for §4.5 global/admin).
- `<type>` — the object type within the subsystem (`pr`, `repo`, `run`, `issue`, `page`, `db`,
  `channel`, `thread`, …).
- `<id>` — the stable, resolvable object id.
- `[#<sub-path>]` — the sub-artifact selector (the ADR-13 `#sub`); structured, not free-text, so it
  is machine-stable AND human-anchorable.

### 5.2 The sub-artifact (`#sub`) grammar per subsystem — the deep-link spine

This is the part the prompt insists on ("down to sub-artifact granularity"). One `#sub` grammar,
specialised per subsystem; **content-anchored where the sub-artifact can move** (so a ref survives
a rebase/edit — PROVEN mechanism, reference-graph content-anchored line-ranges; R-09 owns the
relocate/orphan UX). *(Grammar shape HOUSE STYLE over the PROVEN ADR-13 `#sub` contract.)*

| Subsystem | `<type>` examples | `#<sub-path>` examples | Anchoring |
|---|---|---|---|
| **code** | `repo`, `pr`, `commit`, `blob` | `#L120-145` (line-range), `#comment-<id>`, `#file=<path>` | **content-anchored** (line-range relocates after rebase; R-09) |
| **ci** | `run`, `pipeline` | `#job=<id>/step=<id>`, `#log-line=<n>`, `#artifact=<name>` | positional + `correlation_id`-stable |
| **issues** | `issue`, `project`, `view` | `#comment-<id>`, `#field=<key>`, `#sub-issue=<id>` | id-stable |
| **knowledge** | `page`, `db`, `space` | `#block=<id>`, `#row=<id>`, `#view=<id>` | **block-id-stable** (survives reorder) |
| **chat** | `channel`, `thread` | `#msg=<id>`, `#unfurl=<refid>` | id-stable |

**Web-URL projection (HOUSE STYLE, for browser readability):** the human URL mirrors the tree
1:1 — `/<scope>/code/<repo>/pulls/<n>#L120-145` — so the **breadcrumb is derivable from the URL**
(PROVEN value of location breadcrumbs for deep links —
[NN/g via UXmatters](https://www.uxmatters.com/mt/archives/2025/07/sample-chapter-designing-information-architecture-design-principles.php)).
The `myelin://` form is the canonical machine identity; the `/scope/...` web path is its
SEO-clean, deep-linkable, breadcrumb-bearing rendering. **Both resolve to the same `ArtifactRef`.**

### 5.3 The four resolution guarantees (PROVEN — ADR-13 + ADR-03 + ADR-12)
Every `ArtifactRef`, at *any* level, resolves to exactly these, identically across subsystems —
this is what makes the chip/unfurl one component (R-09) and search results one format (§4.2):
1. **Current rendered projection** (for unfurl/embed) — *live, not snapshot* (R-01 §3.1 beats Slack
   here).
2. **A permission check** (ADR-03, per-viewer) — graceful no-access card, never a leaked title.
3. **Update events** (cache invalidation per ref) — the live-projection mechanism.
4. **Tombstone on erasure** (ADR-12) — the GDPR-aware degraded state, never a dangling leak.

### 5.4 Cross-tenant / cross-cell addressing (the residency edge — flagged, R-19 owns depth)
A public OSS repo referenced from another tenant is a **visibility-gated special case that must not
become a personal-data side-channel** *(PROVEN-as-open — ADR-13 Deferred/Open §; gdpr-eu-sovereignty
§3.1)*. IA rule (HOUSE STYLE): a cross-tenant/cross-cell ref resolves to **a projection if visible,
else a no-access card** — *never a raw id, never the title* — and carries the residency tag of its
home cell (R-04 F-ENG-1/F-ENG-2 cross-cell branch; R-09 state). The address scheme already names
`<tenant>`, so cross-tenant is expressible; the *policy* is the open item.

---

## 6. Labelling, taxonomy & persona-adaptive vocabulary

The taxonomy is the §2 tree's labels. **Labels are held in tokens/config, not hard-coded**, so they
are (a) cheap to change, (b) **tree-testable per-segment** (R-07), (c) i18n-externalised (R-18 / G2),
and (d) persona-adaptive without a schema fork. *(HOUSE STYLE structure; the i18n-externalisation is
PROVEN-required — §4 / G2 / §8b.5.)*

### 6.1 The canonical label set (the "neutral" tree labels)
Primary-nav + tree labels default to the **engineer-neutral** vocabulary because it is the most
precise and the keyboard audience is the speed audience (R-01 §1.1):
`Code · CI · Issues · Knowledge · Chat · Inbox · Search · Home`. Container labels:
`repo · pipeline/run · project · space · channel`.

### 6.2 Persona-adaptive vocabulary mapping (the "issue ↔ work item" candidates)
The same object, two surface labels by lens — a **presentation choice over one model, never a schema
fork** (PROVEN — design-language §2: "the same object is 'issue' to an engineer and 'work item' /
'deliverable' in a roadmap context; terminology is a presentation choice over one model, surfaced
per-space configuration"). Candidate map *(HOUSE STYLE proposal — must be tree-tested per-segment,
R-07)*:

| One object (canonical) | Engineer lens (default) | PM/delivery lens | Corporate/exec lens |
|---|---|---|---|
| `issue` | **Issue** | **Work item** | **Deliverable** |
| `project` | **Project** | **Initiative** | **Programme** |
| `cycle` | **Cycle** | **Sprint** | (rolled up) |
| `view` (roadmap proj.) | **Board / List** | **Roadmap** | **Portfolio** |
| `page` | **Page / Doc** | **Doc** | **Document** |
| `space` | **Space** | **Space / Wiki** | **Knowledge base** |
| `PR` | **Pull request** | **Change** | (rolled up) |

The mapping is **scope-configurable** (per org/space) and **lens-switchable by the viewer** (a PM
can switch to the engineer label; no one is locked out — §2 nav rule 2 / design-language §2).

### 6.3 The fracturing-risk (the §9 open question — flagged, not glossed)
> **[OPEN — design-language §9 / R-07 / R-16]** *Persona-adaptive vocabulary risks **fracturing the
> shared mental model**: if "issue", "work item", and "deliverable" diverge too far (or per-tenant
> customization runs unbounded), a PM and an engineer can no longer talk about the *same object* with
> the *same word*, and cross-references stop being self-explanatory — re-creating the very
> dual-product split Myelin exists to kill (design-language §2 trap; §9 open).* **Bounding rule
> (HOUSE STYLE):** vocabulary varies at the **label/presentation layer only**; the **canonical type,
> the `ArtifactRef`, the icon, and the URL `<type>` segment never change** — so a chip/unfurl/audit
> entry always shows the canonical identity underneath the lens label, and a search/palette query
> resolves regardless of which label the searcher uses (synonym-mapped). Per-tenant free-text
> renaming is **bounded to a fixed, mapped synonym set**, not arbitrary strings — this is the line
> R-07's per-segment card-sort/tree-test must validate (do PMs and engineers still co-locate the
> same object under different labels?). **This is the largest IA uncertainty in the file (§12).**

---

## 7. Per-role default-landing map (deep-linked)

Where each persona lands on entry — a deep-link into §2, role-defaulted but switchable by anyone
(PROVEN — design-language §2: "default lens by role, switchable by anyone"). The default is a
**cross-subsystem `[G]` surface or a role-shaped view**, *not* a subsystem L1 — because the first
act is "what needs me / my work", not "browse a subsystem" (§2 nav rule 2). *(Mapping HOUSE STYLE
over the PROVEN persona clusters, personas.md.)*

| Persona cluster | Persona(s) | Default landing | Deep-link (web form) | Why |
|---|---|---|---|---|
| **Engineer** | P1–P5 | **My cycle board** (current cycle, assigned) | `/<team>/issues/view/<cycle-board>` | the day starts on the board they burn down (R-04 F-ENG-1) |
| **Engineer (CLI-first)** | P1–P3 | terminal `myelin` + **Inbox** on web | `/inbox` | the job finishes in either rendering (§7.7) |
| **PM / delivery** | P6–P10 | **Roadmap** (now-next-later, same records) | `/<team>/issues/roadmap` | communicate delivery from reality, not a parallel deck (R-04 F-PM-2) |
| **EM / lead** | P7 | **My Work / team health Home** | `/home` | cross-subsystem rollup of team + own work |
| **Exec / corporate** | P11 | **Portfolio rollup** | `/<org>/issues/portfolio` | initiatives/OKRs over time (read-forward) |
| **DPO** | P13 | **GDPR / data-rights console** | `/platform/gdpr` | DSR is their primary surface (R-04 F-GOV-1) |
| **Security** | P12 | **Agent governance + audit explorer** | `/platform/agents` | ungoverned automation is their fear (P7/P12) |
| **Enterprise admin** | P15 | **Onboarding / org admin** (first-run) | `/platform/admin` | stands up SSO/residency/agent-policy (R-20) |

**Rule (HOUSE STYLE):** the landing is a **preference token** (role-seeded, user-overridable), held
in config like the labels (§6) — so it is tree-testable and changeable without code. Every landing
is itself a deep-linkable `ArtifactRef`-bearing URL (§5), so "send me where you landed" works.

---

## 8. Coverage: every §7-catalogue surface has a place in the tree

Acceptance criterion: "every §7 surface has a place in the unified tree." Mapping (the §7 catalogue
is the inventory; §2 is its placement). All present:

| §7 catalogue group | Surfaces | Placement in §2 tree |
|---|---|---|
| **§7.1 Git** | repo home, file tree/view, history/commit, compare, code search, PR overview, diff, review, checks, branch-protection, repo settings | `Code → <repo> → {Code · Pull requests<PR> · Branches · Code search · [A] settings}` |
| **§7.2 CI** | run list, single-run, live log, matrix, pipeline editor, environments/deployments, secrets, usage, agent-triage | `CI → <run> → {DAG · Matrix · Logs} · Environments · Pipeline editor · [A] Secrets/Usage` |
| **§7.3 Issues** | issue detail, list/board/table/timeline/calendar, cycle, roadmap/portfolio, triage, My Work, dashboards, saved views, workflow/SLA admin, team page | `Issues → <project> → {Views · Cycle · Triage · <issue> · Roadmap/Portfolio · Dashboards · [A] admin}` (My Work → `[G] Home`) |
| **§7.4 Knowledge** | block editor, database views, sidebar tree, backlinks, history, templates, sharing, export, search | `Knowledge → <space> → <page\|db> → {editor · db views · Backlinks · History · Templates · Sharing · Export}` |
| **§7.5 Chat** | channel/conv list, timeline, composer, thread pane, unfurls, mentions/activity, search, incident/canvas, HITL card surface | `Chat → <channel> → <thread> → {timeline · composer · unfurls · thread pane · HITL card}` (activity → `[G] Inbox`) |
| **§7.6 Shared/admin/GDPR** | inbox, global search, identity/profile, org/team admin, RBAC, agent governance, audit explorer, GDPR console, data-map/residency, tenant/cell settings, onboarding, billing/exit | `[G] Inbox` · `[G] Search` · `[A] Platform/Governance → {Identity · Org admin · RBAC · Agents · Audit · GDPR · Data-map · Tenant/cell · Onboarding · Billing}` |
| **§7.7 CLI** | the CLI as a peer rendering of the same tree | **same tree, textual rendering** — `myelin <subsystem> <verb>` maps to the same §2 nodes; `myelin://` is the shared handle (§5) |

**Coverage ✓** — every §7.1–§7.7 surface placed. The CLI is not a separate IA; it is the same §2
tree rendered textually, sharing the §5 address scheme (PROVEN coherence rule, §7.7).

---

## 9. `[DEFERRED-UNTIL-USERS]` — the tree-test handoff (R-07 owns it)

This IA is an **expert-led design (the no-user substitute), NOT a validated structure.** The
validation is **R-07's owned deliverable** (card-sort + tree-test design); recorded here as the
concrete plan so this file is honest about what it has and hasn't earned. *(PROVEN methods —
[NN/g, Card Sorting vs. Tree Testing](https://www.nngroup.com/articles/card-sorting-tree-testing-differences/);
[NN/g, Tree Testing video](https://www.nngroup.com/videos/tree-testing/).)*

- **What to test:** (1) a **hybrid card-sort** to check whether the §2 container/item groupings and
  the §6 labels match users' mental models (do users group `PR`, `run`, `issue`, `page`, `message`
  the way the tree does? do they expect `Inbox`/`Search`/`Home` hoisted above the subsystems?);
  (2) a **tree-test** over the §2 tree with realistic task scenarios derived from R-04 flows ("find
  the failing CI step for your PR"; "find the runbook linked to this incident"; "find every place a
  data subject appears"); (3) a **label-comprehension** test of the §6 persona-adaptive mapping —
  *the decisive fracturing-risk test* (§6.3).
- **Per-segment (the dual-audience split, the point):** run **separately for engineers (P1–P5) vs
  PMs/delivery (P6–P10) vs corporate/governance (P11–P15)** — the IA is only validated if *both*
  the engineer-neutral labels and the PM/exec lens labels score, and if neither segment fails to
  co-locate the *same object* under its own label (§6.3). NN/g floor: ≥15–20 participants per
  segment for thematic confidence (PROVEN — NN/g).
- **What would falsify this IA:** (1) users cannot find an item within the §2 tree above the
  tree-test success threshold (the grouping is wrong); (2) a label is systematically
  mis-categorised by a segment (the §6 vocabulary fractures — e.g. PMs file "work item" somewhere an
  engineer never looks); (3) users expect a subsystem L1 as the *primary* entry rather than
  `[G] Home`/palette (nav rule 2 is wrong); (4) the persona-adaptive labels cause a PM and an
  engineer to believe they are looking at *different objects* (the §2 dual-product split returns).
- **Caveat (binding):** **do not treat this IA as validated before R-07 runs.** Labels and landings
  are config/token-held precisely so the tree-test result can be applied without restructuring code.

---

## 10. Actionability toward the control artifacts

| Control artifact | What this IA equips | Where |
|---|---|---|
| **rubric.md D4** (one-product coherence — *the central problem, 14%*) | The structural answer: one tree (§2), one shell with shell-owned global surfaces (§3–§4), one identity/scope (§4.4), one address scheme (§5). The "open the same palette/inbox/chip/badge in Code and Chat — identical?" test (§4 invariant) IS the D4 check, made concrete. | §1–§5 |
| **rubric.md G1/G2** | Shell containment mandates (§3.5, PROVEN §8b.4 bugs); labels externalised/i18n-ready and RTL via logical properties (§6 → R-18); depth ≤4 keeps the tree screen-reader-navigable. | §3.5, §6 |
| **sketch-funnel Axis 2 (navigation)** | The rail↔palette↔contextual triad pinned to concrete shell regions (§3.1); a finalist's nav position is a tuning of *one* object model (§2). | §3.1, §4.1 |
| **sketch-funnel Axis 3 (unification ↔ distinct)** | The explicit split: unification at the **object/address/nav** layer (§1.1, §2, §5); distinctness *earned* only in **content** (§3.3) — R-07 rules per-surface. Finalists must differ on Axis 3 over *this* shared substrate. | §1.1, §3.3 |
| **sketch-funnel Axis 5 / 6** | Agent layer (§4.5) and residency cue (§4.4) as IA surfaces finalists tune. | §4.4, §4.5 |
| **R-07 / R-08 / R-09 / R-10 / R-18** | R-07 tree-tests §2/§6; R-08 navigates §2 via §4.1/§4.2; R-09 renders §5 refs; R-10 builds §3.2 views + §4.3 inbox; R-18 localises §6 labels. | §4, §6, §9 |

---

## 11. Completeness-critic (README §9) — gloss-risks this item touches

R-06 is the **IA/shell** owner; it **names and places** these, routing depth to the owners (honest
scoping per standing instructions):

- **Permission-denied / no-access in navigation** — **placed**: every tree node + address resolves
  through the ADR-03 per-viewer check; the palette/search/chip never show what you can't see
  (§4.1/§4.2/§5.3). Depth (the card UX) → R-09.
- **Erased/tombstoned in the address space** — **placed**: §5.3 guarantee #4 (tombstone on erasure);
  depth → R-09/R-21.
- **Cross-cell / cross-tenant references** — **placed & flagged**: §5.4 (visibility-gated, residency
  tag, never a leak); the *policy* is ADR-13's open item; depth → R-19.
- **Mobile/touch shell** — **placed**: §3.5 (drawer pattern, breakpoint collapse, §8b.4 bugs);
  full state-craft → R-21.
- **Persona-adaptive vocabulary fracturing** — **OWNED here as the flag** (§6.3): the bounding rule
  is stated; the *validation* is R-07 (§9); the *per-lens critique* is R-16.
- **Consciously deferred (with reason):** per-component state sets (R-21), the palette/search/chip/
  inbox *interaction* specs (R-08/R-09/R-10), the per-surface distinctness *ruling* (R-07) — this
  file is the **skeleton**, not the components; duplicating them would break the cumulative-corpus
  rule. Named-and-placed, not specced.

---

## 12. Self-check against R-06 acceptance criteria

| Criterion (prompt R-06) | Status | Evidence |
|---|---|---|
| **Every §7 surface has a place in the unified tree** | ✅ Met | §8 coverage table (all §7.1–§7.7 placed); §2 tree |
| **One `ArtifactRef` scheme covers all five subsystems down to sub-artifact** | ✅ Met | §5: `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]` + per-subsystem `#sub` grammar (§5.2) + 4 resolution guarantees (§5.3); PROVEN ADR-13 |
| **The five spines collapse into one object/nav model + one URL/ArtifactRef** | ✅ Met | §1.1 isomorphism table → §2 generic `Scope→Container→Item→Sub-artifact`; §5 one address grammar |
| **Primary-nav + contextual-sidebar + content + context-pane as a concrete tree (not just principle)** | ✅ Met | §2 (the drawn tree) + §3 (each region specced, per-subsystem fill, rules) |
| **Persona-adaptive vocabulary proposed with fracturing-risk flagged** | ✅ Met | §6.2 mapping (issue↔work item↔deliverable) + §6.3 fracturing-risk (§9 open question) + bounding rule |
| **Per-role default-landing specified** | ✅ Met | §7 (PM→roadmap, engineer→cycle board, DPO→GDPR console, etc.) deep-linked |
| **Labels config/token-held; IA structured to be tree-tested in Phase 4** | ✅ Met | §6 (labels in tokens/config), §7 (landing as preference token), depth ≤4 (§2 rule 1); §9 tree-test handoff to R-07 |
| **Build ON §5.1/§7, don't re-derive** | ✅ Met | §3 composes §5.1 regions; §8 places §7 catalogue; neither re-derived |
| **Build ON R-01/R-04, don't duplicate** | ✅ Met | §1.1/§3.1 cite R-01 traps; §2/§7 cite R-04 flows by ID; not re-stated |
| **PROVEN/HOUSE-STYLE tags + date + cited web sources** | ✅ Met | Tagged throughout; dated 2026-06-20; NN/g + UXmatters cited (§5.2/§9) |
| **Deferred validation recorded as a plan, not faked** | ✅ Met | §9 `[DEFERRED-UNTIL-USERS]` (R-07 owns); §6.3 + §12 name it the top uncertainty |
| **Actionable toward rubric D4 + funnel Axis 2/3** | ✅ Met | §10 mapping (D4 = the §4 invariant test; Axis 2 = §3.1; Axis 3 = §1.1/§3.3) |
| **Completeness-critic §9 gloss-risks addressed** | ✅ Met | §11 (placed: no-access, tombstone, cross-cell, mobile, vocabulary; deferred: state-sets, component specs) |

**Top uncertainties (honest):**
1. **Persona-adaptive vocabulary (§6.3) is the largest open IA risk** — whether bounded label-lens
   variance holds without fracturing the shared mental model is a HYPOTHESIS only R-07's
   per-segment tree-test resolves; the bounding rule (canonical type/ref/icon/URL never vary) is
   our HOUSE-STYLE bet.
2. **Nav rule 2 (subsystem L1 as fallback, not primary entry)** — that users prefer `Home`/palette
   over "go to Issues" is a HYPOTHESIS (R-04-derived); falsifiable in R-07.
3. **The `#sub` content-anchoring grammar (§5.2)** surfaces an existing mechanism (PROVEN
   reference-graph anchoring) but the *human URL legibility* of `#L120-145` after a rebase is a
   relocate/orphan UX problem owned by R-09 — the IA names the address, R-09 designs the failure.
4. **Whether Issues+Knowledge truly share one sidebar shape (§3.2)** rests on ADR-06 (PROVEN
   contract) but the *navigation feel* of one views-tree over two item types is an R-10/R-07 test.

---

*End of R-06 deliverable. Date: 2026-06-20. IA structure HOUSE STYLE over the PROVEN three glue
contracts (ADR-13), §5.1 shell, §7 catalogue, ADR-03/06/07; not user-validated — see §9. Feeds
R-07, R-08, R-09, R-10, R-18, Phase 5, Phase 6.*
