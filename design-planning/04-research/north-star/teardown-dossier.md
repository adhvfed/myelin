# R-01 — North-Star Teardown Dossier (Linear · Notion · Slack · GitHub)

> Phase 4 research corpus item **R-01** (WS-A, Seq #1, foundational). Method **#2 comparative/competitive
> teardown** with **#19 heuristics** as the "why it works" lens. This is the **Phase-7 comparative
> baseline**: each finalist will be judged "meets / beats this North Star, or regresses."
>
> **Status date: 2026-06-20.** Agent/AI features move fast — every time-sensitive feature is dated and
> `[VERIFY]`-flagged. Tagging convention (VISION §3 honesty rule): **PROVEN** = cited standard / vendor
> doc / measured/observed behaviour; **HOUSE STYLE** = our taste / synthesis / design judgement.
>
> Builds ON, does not re-derive: design-language **P1–P9**, **§5** shared components, **§7** view
> catalogue, **§8b** day-one primitives; `competitive-landscape.md` §1–§5 steal/avoid lists; VISION §1.
> No prior `04-research` dependency (foundational). Reading order for downstream items: R-02 reuses this
> format (the avoid-half); R-08/R-09/R-10/R-20 cite the per-pattern entries below.

---

## 0. How to read this dossier

Each North Star gets a **screen-by-screen teardown**. Every entry uses the fixed structure the prompt
mandates:

> **Pattern → Why it works (evidenced/cited) → How Myelin adapts it (to which P-principle + which §5/§7
> surface) → The trap hiding inside the pattern.**

Two rules are load-bearing:

1. **Every "steal" is paired with the Myelin principle it must serve** — never "they do it, so we do it."
   If a pattern doesn't serve a P1–P9 principle, it isn't stolen.
2. **Every pattern names its trap.** A North Star pattern copied without its trap is how you inherit the
   incumbent's failure (this is the bridge into R-02, which audits the traps as falsifiable rules).

**Coverage map — every §5 shared component has ≥1 teardown entry behind it** (acceptance criterion 1).
The §11 matrix at the end proves this; the inline tags `[→ §5.x]` make it checkable per entry.

`[VERIFY]` = time-sensitive (re-confirm before external use / Phase-7). `[a11y-debt]` = a place where the
North Star itself is a *weak* baseline we must **beat**, not match (so Phase-7 doesn't treat parity as the
ceiling on G1/G2).

---

## 1. LINEAR — speed, keyboard-nativity, the optimistic core

**Why Linear is the North Star here:** it is the explicit UX reference for **P2 (speed is a feature)** and
**P3 (keyboard-first)** (`competitive-landscape.md §3`; design-language P2). The reason teams switch is raw
speed (PROVEN — `competitive-landscape.md §3`; corroborated by multiple 2025–2026 reviews calling it "the
standard for teams that care about developer experience"). Linear feeds **§7.3** (issue tracker views),
**§5.2** (command palette), and the §8b.6 latency budgets.

### 1.1 The command palette (`Cmd/Ctrl-K`) — the keyboard nerve-centre  `[→ §5.2]`

- **Pattern.** One keystroke opens a fuzzy palette that *unifies* navigation (jump to any issue/project/
  view), actions (create issue `C`, change status, assign), and search. Every common action also has a
  bare single-key shortcut (`C` create, `X` select, `G I` go-to-inbox), so the palette is the discovery
  surface and muscle-memory is the speed surface.
- **Why it works (PROVEN).** The palette "searches the local MobX object pool, not a server," so results
  are instant — no network in the interaction loop (PROVEN — performance.dev technical breakdown, 2025).
  Heuristically (#19): it collapses *recognition* (browse the palette) and *recall* (type the shortcut)
  into one affordance — novices recognise, experts recall, **same surface** (Nielsen "flexibility &
  efficiency of use"; "recognition rather than recall").
- **How Myelin adapts it.** §5.2 already mandates a Linear/Notion-grade palette on **every** screen,
  serving **P3 (keyboard-first, mouse-complete)**. Myelin extends it past Linear in two ways the design
  language already commits: (a) the palette **composes the same query AST** (ADR-07) as saved views and
  agent triggers — one query language surfaced humanly; (b) palette actions map to the **same typed
  `ToolDef`s agents use** (ADR-08) — human/agent tool symmetry. Both serve **P1 (one product)**: the
  palette is the one nerve-centre across git/CI/issues/knowledge/chat, not five palettes. (Detailed in
  R-08.)
- **The trap.** Palette-as-primary-navigation is a **discoverability cliff for non-keyboard users**
  (P6/P11 PMs/corporate) — "discoverability via search" punishes anyone who doesn't know what to type.
  Myelin's mitigation is **P3's second half ("never keyboard-only")**: every palette action must also be
  reachable by a visible, pointer-discoverable affordance, and the cognitive-walkthrough question (#20)
  "can a new PM find what an engineer reaches by muscle memory?" is a gate (R-08). Maps to **sketch-funnel
  Axis 2** (command-palette-led ↔ persistent-rail): a maximally palette-led finalist must still pass that
  walkthrough.

### 1.2 The optimistic / local-first sync engine — *why it feels instant*  `[→ §5.10 optimistic-rollback, P2]`

- **Pattern.** The browser is the primary database. "Mutations apply locally first, then asynchronously
  push to the server"; the local store is **IndexedDB**, with an in-memory **MobX observable** layer; a
  durable **transaction queue** holds pending mutations; on server rejection "the observable reverts and
  there's a brief flicker" (PROVEN — performance.dev, 2025).
- **Why it works (PROVEN).** Three compounding mechanisms: (1) **no network in the interaction loop** —
  the write returns instantly; (2) **granular observables** — "a change that updates one field of one
  issue re-renders exactly the components that read that field … one cell," not the whole list — so even
  large boards stay smooth; (3) **data-level code-splitting / lazy hydration** — heavy tables hydrate on
  demand, so startup is cheap (all PROVEN — performance.dev, 2025). This is **visibility of system status
  done by *absence* of waiting** (#19): the system never makes you watch it think.
- **How Myelin adapts it.** This is the mechanical substance of **P2 (speed)** and the §8b.6 budgets
  (keyboard < ~100ms; suppress flash-of-spinner < ~1s). Myelin already commits optimistic updates + live
  bus-pushed updates (design-language P2; `git-hosting.md §4`). The critical adaptation is the **honesty
  half**: §5.10 / §8b.6 require **optimistic-update + honest-rollback** as a *designed* state, not a
  flicker we hope never fires. R-13/R-21 own the rollback craft; this dossier flags it as the state
  Linear treats as rare-and-quiet that Myelin must treat as **first-class and legible** (D8 anchor).
- **The trap.** Two. (a) **Optimism that lies** — showing success then silently swallowing a server
  rejection is the "optimism for latency, *dishonesty* on failure" anti-pattern; Myelin's rule (P2 + §8b.6)
  is optimism for latency, **honesty on failure**. (b) **Local-first ⟂ EU residency tension**: Linear's
  speed assumes aggressive client caching of all data; Myelin cannot replicate personal data to a global
  CDN (P2 note; ADR-11). So Myelin buys perceived speed via **optimistic UI + in-region edge + prefetch**,
  **not** global replication. This is a real constraint the sketches must honour (R-13), not a free win.

### 1.3 Issue board / list / triage / cycles  `[→ §5.6 views, §7.3]`

- **Pattern.** One issue model rendered as keyboard-navigable **list**, **board** (drag between status
  columns), and grouped views; a dedicated **Triage** inbox for incoming issues; **Cycles** (time-boxed
  sprints) with capacity/burndown. `j/k` moves the focused row; single-key actions act on focus.
- **Why it works (PROVEN/HOUSE STYLE).** Opinionated, minimal-config defaults mean a team is productive
  without a Jira-style setup project (PROVEN — `competitive-landscape.md §3`). The board/list are the
  *same data, different projection* — switching is free. Triage as a distinct queue keeps the backlog calm
  (P8-adjacent). Heuristically (#19): strong **consistency & standards** (one interaction grammar across
  views) and **user control** (instant view switch).
- **How Myelin adapts it.** This is the literal **§5.6 views component** (the shared database/views
  primitive, ADR-06) serving **P5 (earned density)** on the engineer board and **P1 (one product)** by
  being the *same* component the PM roadmap and knowledge databases use. Myelin's adaptation past Linear is
  **§2 dual-audience**: the same records also render as a PM roadmap/now-next-later and an exec rollup —
  Linear *deliberately refuses* the PM/corporate end (`competitive-landscape.md §3` avoid). The Triage
  inbox feeds **§5.8** (the unified inbox) and §6.5 (agent volume out of the main stream). (R-10 specs the
  views component; R-16 the dual-audience lenses.)
- **The trap.** **Opinionation that becomes inflexibility.** Linear's defaults are loved by engineers and
  rejected by governance-heavy orgs that need custom fields, hierarchies, SLAs, audit (the half of Myelin's
  mandate Linear drops). Copying Linear's minimalism *without* P4 (progressive disclosure) re-creates the
  "serves engineers, fails corporate" split. Myelin's rule: Linear's speed **and** Jira's depth, with depth
  one layer down (P4), never imposed. (This is the precise tension R-02 audits and R-16 resolves.)

### 1.4 Linear AI agents & auto-triage  `[VERIFY]` `[→ §6, §5.4]`

- **Pattern (dated).** As of **2026** `[VERIFY]`, Linear ships built-in agents (Linear Asks Slack→Linear
  triage, AI issue summaries, duplicate detection, auto-classification) and a **2025 Agent API** that lets
  third-party agents (Claude Code, Devin, Cursor, Copilot) appear as **first-class workspace members** —
  assignable issues, posting updates "like a teammate." **Skills** are reusable workflows triggered by
  slash-command or auto-activated by context (PROVEN-as-reported — Linear AI guides, 2026; vendor specifics
  `[VERIFY]`).
- **Why it works (HOUSE STYLE, reported).** "Agent as a real workspace member with an identity" is exactly
  the model Myelin's VISION mandates — agents as first-class principals, not webhook bots. It validates the
  agent-native thesis and **raises the Phase-7 bar**: by 2026 "agent posts an update like a teammate" is
  table stakes, not a wow.
- **How Myelin adapts it.** Myelin's §6 contract is **stricter and more legible** than "agent posts like a
  teammate": agents are **always labelled** (P7; AI-Act, never disguised as human), **plan-then-apply**
  (propose effects before acting, ADR-08), and consequential actions pass a **HITL Approve/Edit/Reject
  card** (§5.4/§6.3). Where Linear lets an agent act like a teammate, Myelin makes the agent's *proposed
  effects, scope, delegation, and provenance* legible first. Serves **P7 (visible, labelled, trustworthy)**.
- **The trap.** "**Agent-as-teammate**" can **erase the agent/human distinction** — the P12/P13 (security/
  DPO) deepest fear (ungoverned automation). Copying Linear's frictionless agent membership without the §6
  labelling + plan-then-apply + HITL is how you ship "magic" that a regulated buyer cannot approve. Myelin's
  rule: never autonomous-by-default on consequential actions; agent volume out of the main timeline (§6.5).
  (R-14/R-15 own this.)

---

## 2. NOTION — the block editor and the database/views primitive

**Why Notion is the North Star here:** it is the reference for **P4 (approachability)** and the **§5.9
editor** + **§5.6 views**, and it is the canonical source of the **issues↔knowledge reuse boundary**
(`competitive-landscape.md §3/§4`; design-language §5.6). Feeds **§7.4** (knowledge views) and **§5.9**.

### 2.1 The block editor — everything is a composable block  `[→ §5.9, §8b.2]`

- **Pattern.** Every paragraph, heading, list item, table, callout, image, embed is a **block**; pages nest
  infinitely; blocks drag-to-reorder via a six-dot handle that also exposes delete/duplicate/convert
  (PROVEN — Notion help docs; carlosrayala.com block deep-dive, 2025). The editor is a custom-built
  block model, not a `<textarea>` (PROVEN — TechAhead Notion stack write-up, 2025).
- **Why it works (PROVEN/HOUSE STYLE).** One uniform substrate ("everything is a block") means a single set
  of interactions (insert, drag, convert, comment) applies everywhere — strong **consistency & standards**
  (#19) and **flexibility** (any block, any page). Approachable because the default is just typing; power
  is summoned, not imposed (P4-aligned).
- **How Myelin adapts it.** Myelin already mandates a Notion-class block editor over the **shared content
  model** (ADR-05), serving **P1 (one editor everywhere)** — knowledge pages, issue descriptions/comments,
  PR descriptions, chat composition share the *same node taxonomy*. The §8b.2 day-one mandates **sharpen
  past Notion**: **one render path** (read and edit run the same parser), the **`render(parse(md)) === md`
  round-trip as a hard CI gate**, inline runs stored as a **markdown-subset string** (survives paste/export/
  diff), with `mention`/`artifact_ref`/`embed` as **structured nodes** so reference-extraction stays
  reliable. (R-10 specs the editor.)
- **The trap.** Two. (a) **Proprietary block model = lock-in + imperfect export** (PROVEN — Notion avoid,
  `competitive-landscape.md §4`); Myelin's markdown-subset-string + open-format export (P14/§8b.5) is the
  deliberate counter. (b) **`contenteditable` browser variance** (Enter/IME/paste) is "the #1 'not a real
  editor' tell" (§8b.2) — Notion solves it with years of engineering; Myelin must ship the serializer +
  caret-offset model + Enter-split surgery **as independently unit-tested primitives before the integrated
  editor** (§8b.2), or it inherits the bug, not the polish.

### 2.2 The database/views primitive — same data, many views  `[→ §5.6, the reuse boundary]`

- **Pattern.** A Notion **database** holds records (pages with typed properties) viewable as **table,
  board, list, timeline, calendar, gallery** over the *same* data; relations/rollups link databases. As of
  the **2025 "data sources" change** `[VERIFY]`, a *database* is now a **container** that can hold one or
  more **data sources** (the actual collections); but crucially **a single view still cannot pull pages
  from two data sources at once — you still need relations** (PROVEN — notionapps.com data-sources update,
  2025).
- **Why it works (PROVEN).** "Structured data + multiple projections of it" is **the single most important
  idea** to steal (`competitive-landscape.md §4`): it gives non-engineers a spreadsheet/board/calendar
  mental model with zero new concepts, and it is the *same* idea as PM/issue views (§3 ↔ §4 parallel). The
  2025 container/data-source split is evidence Notion is moving toward **per-data-source permissions** —
  validating Myelin's permission-aware-views direction (ADR-03/ADR-07).
- **How Myelin adapts it.** This is **§5.6 verbatim** — one views component shared by the **issue tracker
  AND knowledge databases**, the platform's biggest reuse boundary (ADR-06). A view = **query AST + grouping
  + sort + visible fields** (ADR-06/ADR-07), **permission-aware by construction** (ADR-03 — a view can never
  show rows the viewer can't see; this is the permission-pre-filter that Notion is only now approaching).
  Serves **P1 (one product), P5 (earned density on the table), §2 (dual-audience: engineer board vs PM
  roadmap over one schema)**.
- **The trap.** (a) **Performance at scale** — large Notion workspaces get slow (PROVEN — `§4` avoid);
  Myelin's optimistic + virtualized + permission-pre-filtered approach (P2) must not regress here. (b)
  **The "one view can't span two data sources" limitation is a *feature gap to beat*, not copy** — Myelin's
  query AST + reference graph should let a view/embed span subsystems (issue board embedded in a knowledge
  runbook, §5.9), which Notion structurally cannot. Don't inherit Notion's silo boundary as if it were a
  law.

### 2.3 The slash menu (`/`) — summon any block  `[→ §5.9, P4]`

- **Pattern.** Typing `/` opens a menu of every insertable block type; arrow-or-type to filter, Enter to
  insert (PROVEN — Notion writing-basics docs, 2025).
- **Why it works (PROVEN).** It is **progressive disclosure made physical** (#19 / P4): the page looks
  empty and simple, but every capability is one keystroke away — power present, not imposed. Keyboard-fast
  for experts, menu-discoverable for novices (the same recognition+recall dual that the command palette
  pulls off, §1.1).
- **How Myelin adapts it.** §5.9 already mandates a slash menu in the editor. It serves **P4 (simple by
  default, powerful on demand)** and **P3 (keyboard-first)**. The Myelin twist: `/` can insert **reference
  nodes and live embeds** (a `#issue`, a live database view over an `ArtifactRef`) — the wedge (P6) reaches
  into the editor.
- **The trap.** **Slash-menu bloat** — a `/` menu with 60 block types becomes its own config maze (the Jira
  trap in miniature). Myelin's P4/P8 rule: a short, ranked, frequency-aware default set with depth behind
  search, not a wall of options.

### 2.4 Mentions (`@`) — people, pages, dates, link-previews  `[→ §5.5, §5.3]`

- **Pattern.** `@` mentions a person, a page, a date, or a database; mentioned pages render as inline
  "link-preview" chips (PROVEN — Notion block reference docs / writing basics, 2025).
- **Why it works (PROVEN).** Mentions turn references into **live, navigable inline objects** — the
  document becomes a graph, not flat text. Recognition over recall (#19): you see what's linked.
- **How Myelin adapts it.** This is **§5.5 (mentions) rendered as §5.3 reference chips**. Myelin generalises
  far past Notion: an `@mention` of an **agent is a trigger into the agent fabric** (ADR-08), and a
  `#artifact` ref spans **all five subsystems** (commit/PR/issue/doc-block/CI-run/chat) via the reference
  graph — serving **P6 (reference everything, the wedge)**. Notion's mentions stay inside Notion; Myelin's
  cross the whole platform. (R-09 owns the chip/unfurl; R-22 the wedge moments.)
- **The trap.** **Permission leakage via the preview** — a mention chip that shows a title the viewer can't
  access is a GDPR/ADR-03 leak. Notion's previews are workspace-scoped; Myelin's chips are **permission-aware
  per viewer** and degrade to a graceful "no access" card, never a leaked title (§5.3 hard rule). This is a
  *correctness* property, not a nicety.

---

## 3. SLACK — unfurl, slash-commands, threading (+ Zulip topics contrast)

**Why Slack is the North Star here:** it owns the **§5.3 unfurl** and **§5.5 threads/reactions**, and the
**link-unfurl + slash-command** model is "exactly how Myelin's chat should reference any artifact"
(`competitive-landscape.md §5`). Feeds **§7.5** (chat) and **§5.3**. The **Zulip topic model** is included
per the prompt as the **contrast for agent volume**.

### 3.1 The unfurl / rich link preview  `[→ §5.3, the wedge component]`

- **Pattern.** Paste a URL → Slack fetches and renders a **rich card** (title, description, image, favicon);
  **up to 5 links unfurl per message** (PROVEN — Slack developer docs; dev.to unfurl breakdown, 2025).
  Apps can register to unfurl their own domains, and **Work Objects** (newer) give "richer previews and
  greater feature extensibility than app unfurling" — interactive cards for entities/data inside Slack
  (PROVEN — Salesforce/Slack help, 2025) `[VERIFY]` exact Work Objects rollout/scope.
- **Why it works (PROVEN/HOUSE STYLE).** The unfurl turns a bare link into **context without a tab-switch** —
  the reader stays in flow and still sees the artifact's state. `competitive-landscape.md §5` and `chat.md
  §2.4` both call the unfurl "the differentiator." Heuristically: **recognition over recall** + **minimise
  navigation cost** (#19).
- **How Myelin adapts it.** This is the **§5.3 reference chip + unfurl card — "the most important shared
  component in the platform" (P6)**. Myelin's adaptation **beats Slack on three axes the design language
  already commits**: (1) **live, not snapshot** — the unfurl is a *current* projection kept fresh by bus
  update events, where Slack's preview is a fetch-time snapshot; (2) **permission-aware per viewer** — never
  leaks a title (Slack unfurls are not per-viewer permission-resolved against the target system); (3)
  **inline actions where permitted** — re-run a CI job, transition an issue, approve a PR *from the unfurl*
  (Slack's Work Objects approach this; Myelin makes it native across all five subsystems). Serves **P6 (the
  wedge)** and **P1 (one chip everywhere)**. (R-09 owns this in full.)
- **The trap.** Two. (a) **Snapshot unfurls rot** — a cached title goes stale, or worse, shows content the
  target later restricted/erased (a GDPR leak). Myelin's live + permission-aware + **tombstone-on-erasure**
  rules (§5.3) are the explicit counter — this is *why* Myelin defaults to live, not snapshot. (b) **Unfurl
  noise** — 5 fat cards per message is visual clutter; Myelin's compact-chip-by-default + expand-on-demand
  (P8 calm) avoids turning the timeline into a card wall.

### 3.2 Slash-commands  `[→ §5.2 (palette symmetry), §7.5 composer]`

- **Pattern.** `/command` in the composer invokes an action (`/remind`, app commands); a discoverable
  command list; app-provided commands extend the set (PROVEN — Slack docs / common-hacks coverage, 2025).
- **Why it works (PROVEN).** A **typed, discoverable action surface inside the conversation** — you act
  without leaving the message box. Same recognition+recall dual as the palette.
- **How Myelin adapts it.** Myelin unifies this with **§5.2**: server-side slash-commands in the chat
  composer are the **same `ToolDef` catalogue** the command palette and agents use (ADR-08) — **one action
  vocabulary across human-in-palette, human-in-chat, and agent**. Serves **P1 (coherence)** and the
  agent-native symmetry (P7). A slash-command and an `@agent` trigger resolve through the same fabric.
- **The trap.** **Command sprawl / undiscoverable verbs** — Slack's per-app slash-commands fragment into a
  vocabulary no one fully knows. Myelin's rule: one typed catalogue, permission-pre-filtered (you only see
  commands you may run, ADR-03), surfaced consistently in palette + composer — not a per-integration
  free-for-all. (R-08.)

### 3.3 Threading — and the Zulip topic contrast  `[→ §5.5, §6.5 calm agent volume, §7.5 thread pane]`

- **Pattern (Slack).** Channels carry a **flat chronological** stream; a **thread** hangs off a single
  parent message as an opt-in side-conversation. Threads must be *deliberately created*; un-threaded chatter
  stays in the flat channel (PROVEN — Slack vs Zulip comparisons, 2025–2026).
- **Pattern (Zulip contrast).** **Every** message belongs to a **topic** within a **stream** — topics are
  mandatory, lightweight, named threads. You can **follow/mute topics per-user**, get **narrowed views**
  showing only one topic, and **email-digest catch-up** (PROVEN — almtoolbox Zulip overview, 2026). Zulip's
  pitch: preserve conversation context **by default at scale**, where Slack's "linear channels" cause
  "scroll fatigue."
- **Why it matters for Myelin (the contrast, HOUSE STYLE).** **Agents generate volume** (review comments,
  triage updates, status posts). Slack's *opt-in* threading means agent chatter pollutes the flat channel
  unless someone threads it; Zulip's *mandatory topic* model keeps high-volume async conversation legible by
  construction. This is precisely the **agent-volume problem** §6.5 names.
- **How Myelin adapts it.** §6.5 + `competitive-landscape.md §5` already say Myelin **considers Zulip-style
  topics specifically because agent participation raises volume**. Concretely for the sketches: agent
  verbosity defaults **out of the main timeline** (threads/collapsible summaries/inbox), serving **P8 (calm
  by default)**. Whether Myelin goes full Zulip-mandatory-topics or Slack-threads-plus-discipline is a
  **sketch-funnel Axis 5 (agent presence: ambient ↔ foregrounded)** decision — name it, sketch both poles.
- **The trap.** Two opposite traps. (a) **Slack's flat-channel scroll-fatigue** under agent load — adopting
  Slack threading naively re-creates the "30×-agent-surge drowns humans" failure (README §9 storm case).
  (b) **Zulip's learning curve** — mandatory topics confuse newcomers (PROVEN — Zulip avoid, `§5`), risking
  P4/P6's approachability. Myelin must pick a default that tames agent volume **without** a Zulip-grade
  onboarding tax. (R-15 owns calm-agent-volume; R-21 owns the storm state.)

### 3.4 Reactions, presence, the composer  `[→ §5.5, §5.11, §7.5]`

- **Pattern.** Emoji reactions (lightweight ack without a reply), presence/typing indicators, a rich
  composer (formatting, mentions, file upload) (PROVEN — Slack feature set, ubiquitous).
- **Why it works (PROVEN).** Reactions are **low-friction acknowledgement** that reduces reply-noise (P8-
  adjacent); presence builds **social co-awareness**; the rich composer keeps message authoring in one place.
- **How Myelin adapts it.** **§5.5** (one comment/thread/reaction model across PR review, issue discussion,
  doc comments, chat — "discuss an artifact feels identical everywhere", P1) and **§5.11** (presence/typing
  ride the firehose transport, ADR-04). The composer is the **§5.9 editor** in a chat-tuned concurrency mode
  (small/mostly-immutable messages) — *same editor component*, serving **P1**.
- **The trap.** **Emoji-as-UI.** Slack leans on emoji as functional controls; §8b.3 forbids emoji-as-UI
  ("an emoji can't inherit `currentColor` or be re-themed") and forbids reactions-as-status. Myelin steals
  the *low-friction-ack* idea, not the emoji-as-interface implementation.

---

## 4. GITHUB — the code-review bar Myelin must meet

**Why GitHub is the North Star here:** the **pull request is the gold-standard unit of code collaboration**
(diff + conversation + checks + reviewers + suggestions in one surface), and Myelin's review UX **must meet
it** (`competitive-landscape.md §1`). Feeds **§7.1** (PR/diff/review/checks) and the **PR context pane**
(the wedge flagship, system-overview §8.1). **2025–2026 note:** GitHub overhauled the "Files changed"
experience and shipped agentic review — both dated/`[VERIFY]` below.

### 4.1 The PR overview — the unit of collaboration  `[→ §7.1, §5.3, §5.5]`

- **Pattern.** One surface aggregates: description, linked issues, participants/reviewers, **required-checks
  summary + merge readiness**, the conversation timeline, and the merge control. **CODEOWNERS**
  auto-routes reviewers; `#123` cross-references and `@mentions` thread issues/PRs/people together (PROVEN —
  `competitive-landscape.md §1`; GitHub docs).
- **Why it works (PROVEN).** It is a **single source of truth for "is this mergeable and who must act"** —
  status, conversation, and gate in one place (#19 visibility of system status; match between system and
  real-world task). Cross-refs (`#123`, `@`) make the PR a hub in a graph.
- **How Myelin adapts it.** The PR overview is the **PR context pane — the wedge flagship** (system-overview
  §8.1; §7.1): it first-class-references the linked issue, the design doc (knowledge), the CI run, and the
  chat thread through the **shared reference graph** with bidirectional backlinks — context the fragmented
  stack can only *link*, Myelin **assembles and pre-fetches** (§8b.6). Serves **P6 (the wedge)** and **P1
  (one product)**. GitHub's `#123` is the seed; Myelin generalises it across all five subsystems (the
  primitive `competitive-landscape.md §1` explicitly says to generalise). (R-22 owns the wedge moment.)
- **The trap.** **The PR as a junk-drawer of bolted-on tabs** — checks from one system, deploys from
  another, security from a third, each a separate integration with its own model (the Atlassian "stitched"
  feel at the PR level, `§6.1`). Myelin's counter is **native unification**: checks, issues, docs, chat,
  agents are *the same platform's objects*, not API-stitched panels. (This is exactly the R-02 stitched-seam
  trap, previewed here.)

### 4.2 The diff / files-changed view  `[VERIFY]` `[→ §7.1 diff, §4 a11y]`

- **Pattern (dated).** Unified + split diff, syntax highlight, per-file collapse/viewed-state, whitespace
  toggles, rename/move handling. The **rebuilt "Files changed" experience** (public preview 2025-06;
  **on by default 2026-01-22** `[VERIFY]`) adds: **faster diff rendering with lower memory**, a **side panel
  for status-check errors/warnings (annotations)**, **commit-by-commit review**, improved filtering, and —
  notably — **"consistent keyboard navigation and screen-reader landmarks added for better accessibility"**
  (PROVEN — GitHub changelog, 2025-06-26 / 2025-12-11 / 2026-01-22).
- **Why it works (PROVEN).** The diff is a **dense engineer surface done right**: per-file viewed-state and
  collapse manage cognitive load on big PRs; the annotations side-panel puts CI failures *next to the code
  that failed* (visibility of system status, #19). The 2025–2026 rebuild explicitly prioritised **rendering
  performance** and, belatedly, **keyboard + screen-reader** support.
- **How Myelin adapts it.** The diff is the **archetypal §5 "earned density" surface (P5)** and a **§7.1**
  flagship. Myelin's adaptation: the diff is **keyboard-complete from day one** (P3 — not a 2026 retrofit),
  it is an **agent-aware review surface** (visually distinct agent reviewers/authors, §6.1), and a diff line
  is an **`ArtifactRef` down to sub-artifact** (ADR-13) so a chat message or issue can reference *this exact
  line* and the comment **content-anchors / relocates after rebase** (§5.5; the diff-anchored chip that
  orphans, README §9 — R-09 owns it).
- **The trap.** `[a11y-debt]` **GitHub's own diff was a weak accessibility baseline until 2025–2026** —
  keyboard nav and screen-reader landmarks were *added* in the rebuild, meaning prior parity = a G1 failure.
  Myelin must **beat, not match**: the diff is a named "hard component" the R-17 a11y audit and rubric **G1**
  require keyboard-operable + screen-reader-correct *by construction*. Treating GitHub's historical diff as
  the bar would import its a11y debt. The diff is also the worst case for **200% zoom / reflow** (README §9)
  and **RTL** (a mirrored diff is non-trivial, G2) — flag for R-17/R-18.

### 4.3 Batched review + suggested changes  `[→ §5.5 review batching, §7.1 review surface]`

- **Pattern.** "Start review → add inline comments → submit one verdict" batches feedback into a single
  review event rather than firing a notification per comment. **Suggested changes** let a reviewer propose
  an exact diff the author applies in one click; suggestions can be **batched and committed together** with
  an editable commit message (PROVEN — GitHub docs; changelog 2025–2026 on batching in the new Files-changed
  page).
- **Why it works (PROVEN).** Batching is a **calm-by-default notification pattern** — the author gets one
  coherent review, not 14 pings (P8; directly the "notification overload" incumbent failure Myelin must
  avoid). Suggested-changes collapses "describe the fix" → "apply the fix" into one action (minimise user
  effort, #19).
- **How Myelin adapts it.** **§5.5 explicitly calls review batching "strongly preferred for review
  quality"** (`git-hosting.md §4.2`). Serves **P8 (attention is sacred)** and **P1** (one comment/thread
  model, so batched review feels like batched issue/doc discussion). Suggested-changes maps to the same
  apply-a-proposed-effect interaction as an **agent's plan-then-apply** (§6.2) — a human suggestion and an
  agent proposal are *the same shape* of "proposed change you approve," which is a coherence win (P1/P7).
- **The trap.** **Per-comment notification firehose** (the un-batched default) is the exact P8 failure;
  copying GitHub's *review* surface but not its *batching* discipline re-creates notification overload.
  Myelin's rule: batched-by-default, single coherent review event into the **§5.8 inbox** with "why am I
  getting this" provenance (§8b.6).

### 4.4 Checks API surfacing  `[→ §7.1 checks panel, §7.2 CI, §5.3 unfurl]`

- **Pattern.** A clean producer/consumer separation: **many producers** (CI, linters, security scanners,
  external services) post status to **one consumer** (the PR), surfaced as required-vs-optional checks with
  re-run actions and, in the 2025–2026 rebuild, an **annotations side-panel** mapping errors to lines
  (PROVEN — `competitive-landscape.md §1`; GitHub changelog 2025–2026).
- **Why it works (PROVEN).** It **decouples** signal producers from the review surface — any number of tools
  can report status without bespoke UI, and the developer sees one merge-readiness verdict (consistency &
  standards; visibility of status, #19). `competitive-landscape.md §1` praises this exact "clean separation
  that lets many producers post status to one consumer."
- **How Myelin adapts it.** Myelin gets this **natively via the event bus** — CI (and any subsystem event)
  posts check status that surfaces in the PR context pane (§7.1) and unfurls (§5.3), serving **P1 (one
  product)**: a check in the PR, a CI run in chat, and an issue's CI status are *the same functional-status
  palette* reading identically everywhere (design-language §3.2 shared functional colours). The CI failure
  → step → line **prefetch** (§8b.6) is the wedge that GitHub's annotations side-panel only gestures at.
- **The trap.** **Status-by-colour-alone.** A red/green checks panel that relies on colour fails colour-blind
  users (G1 / §8b.3 — status never by colour alone). And **marketplace supply-chain risk**: GitHub Actions'
  third-party action ecosystem has had supply-chain incidents (`competitive-landscape.md §2` avoid) — a UX
  trap too, since an unfurled/embedded third-party check is a trust surface. Myelin's rule: every status
  carries glyph + label + position (§8b.3), and check provenance is legible (P7/P9).

---

## 5. The cross-North-Star synthesis (what the four teach together)

| North Star | The one thing it proves | The Myelin principle it serves | The trap it carries |
|---|---|---|---|
| **Linear** | Instant = no network in the interaction loop (optimistic, local-first, granular re-render). | **P2 speed**, **P3 keyboard**. | Opinionation→inflexibility; local-first ⟂ EU residency; optimism that lies. |
| **Notion** | "Structured data, many views" + one block substrate is the approachable power model. | **P4 approachability**, **P5 density**, **§5.6/§5.9 reuse**, **§2 dual-audience**. | Proprietary lock-in / weak export; `contenteditable` variance; per-silo view boundary. |
| **Slack** | A live, rich, in-flow reference (unfurl) + typed in-conversation actions (slash). | **P6 the wedge**, **P1 one chip/one action vocabulary**. | Snapshot-unfurl rot + permission leak; flat-channel scroll-fatigue under agent volume; emoji-as-UI. |
| **GitHub** | The PR as one mergeability-source-of-truth; batched review; producer/consumer checks. | **P6 PR context pane**, **P8 batching**, **P1 unified checks**. | Bolted-on-tab "stitched" PR; status-by-colour; a11y-debt diff (must beat, not match). |

**The meta-lesson (HOUSE STYLE).** Every North Star is *excellent at one thing and structurally weak where
Myelin is strong*: Linear refuses the PM/corporate half; Notion silos its database boundary and isn't
sovereign; Slack's references are snapshots and not permission-aware; GitHub's PR is a junk-drawer of
stitched integrations. **Myelin's wedge is the seam** — the unified reference graph + one identity/permission/
event model + agent fabric — which is exactly the place each North Star is weakest. The dossier's job for
Phase 7: a finalist that merely *matches* a North Star at its strength while inheriting its seam-trap has
**regressed**, because it has spent the unification advantage and bought nothing.

---

## 6. How this maps to the sketch-funnel axes (so divergence is grounded)

- **Axis 1 (density: dense ↔ calm).** Linear/GitHub-diff = the dense pole done well (P5 earned density);
  Notion = the calm pole done well (P4). Finalists must show both; the diff and the board are the dense
  test, the roadmap/knowledge page the calm test.
- **Axis 2 (nav: rail ↔ palette ↔ contextual).** Linear's palette-led is one pole; GitHub/GitLab's
  persistent-rail the other; the **PR context pane** is the contextual pole (the shell adapts to the
  artifact). The §1.1 trap (palette discoverability cliff) is the cull-check.
- **Axis 3 (unification ↔ distinct-per-surface) — the central problem.** The dossier shows each North Star
  is a *single-surface* specialist; Myelin must make the diff feel like code, the roadmap feel like a
  roadmap, **yet share chip/identity/palette/editor/views**. Every "how Myelin adapts" entry above is an
  Axis-3 datum. (R-06/R-07 own the ruling.)
- **Axis 4 (tone: utilitarian ↔ warm).** Linear/GitHub = utilitarian-precise; Notion/Slack = warm-
  approachable. The four bracket the tone space; R-11 proposes the three directions within it.
- **Axis 5 (agent presence).** §1.4 (Linear agents-as-members) = foregrounded pole; §3.3 (Zulip topics for
  agent volume) = the ambient/calm pole. Sketch both.
- **Axis 6 (sovereignty visibility).** No North Star addresses this — it is Myelin's net-new axis; R-19
  owns it. The dossier only flags where North-Star patterns *leak* (snapshot unfurls, mention previews)
  and thus where sovereignty cues must sit.

---

## 7. Completeness-critic (§9) gloss-risks this item touches

R-01 is a teardown, not a state catalogue, so it **names and routes** the relevant gloss-risks rather than
fully specifying them (those are owned downstream — honest scoping per the standing instructions):

- **Permission-denied "no access" card (never a leaked title).** Surfaced as a trap in §2.4 (Notion mention
  preview) and §3.1 (Slack snapshot unfurl). **Routed to R-09** (chip/unfurl states) — the North Stars are
  *weak* here, so it's a beat-not-match.
- **Erased / tombstoned.** Named in §3.1 (unfurl-of-erased-content leak). **Routed to R-09/R-21.**
- **Optimistic-update rollback.** Specified as Linear's rare-quiet-flicker that Myelin must make first-class
  (§1.2). **Routed to R-13/R-21** (D8 anchor).
- **Storm / 30×-agent-surge.** Named in §3.3 (Slack flat-channel under agent volume). **Routed to R-15/R-21.**
- **Keyboard-only operability of hard components (diff).** Flagged as GitHub a11y-debt in §4.2. **Routed to
  R-17 + rubric G1.**
- **Status-not-by-colour-alone.** Flagged in §4.4 (checks panel). **Routed to R-17 + G1.**
- **200% zoom/reflow + RTL of the diff.** Flagged in §4.2 as the hardest G1/G2 case. **Routed to R-17/R-18.**
- **The diff-anchored comment that relocates/orphans after rebase.** Flagged in §4.2. **Routed to R-09.**

**Consciously deferred (with reason):** the *full* empty/loading/error/permission/erased/agent-pending state
set per surface (that is R-21's owned deliverable, not a teardown's), and device/touch/mobile glosses (R-13/
R-21). Naming-and-routing here, not duplicating, keeps the corpus cumulative (standing instruction).

---

## 8. Rubric & funnel actionability (what this item equips, made checkable)

- **Equips the comparative baseline for ALL of Phase 7.** Each entry's "how Myelin adapts → which P" line is
  the "meets/beats/regresses" yardstick. Concretely: a finalist's command palette is scored against §1.1; a
  diff against §4.2; an unfurl against §3.1; a PR pane against §4.1.
- **Feeds D1 (power-user efficiency)** — §1.1 palette + §1.2 optimistic core + §4.2 keyboard-complete diff
  are the bar.
- **Feeds D4 (one-product coherence)** — §5 synthesis + the "stitched PR" trap (§4.1) define what *not* one
  product looks like.
- **Feeds D6 (agent legibility)** — §1.4 (Linear agents) + §4.3 (suggestion = plan-then-apply shape) set the
  agent bar R-14 deepens.
- **Feeds D7 (density-made-calm)** — §3.3 (agent volume) + §4.3 (batched review) are the calm anchors.
- **Feeds D8 (perceived performance)** — §1.2 is the optimistic/rollback anchor.
- **Equips G1/G2 honestly** — §4.2 explicitly marks GitHub's diff as an a11y-debt baseline to **beat**, so
  Phase 7 doesn't treat North-Star parity as G1-sufficient.

---

## 9. Uncertainties & `[VERIFY]` register (dated 2026-06-20)

| Claim | Status | Re-verify before |
|---|---|---|
| Linear AI agents / Agent API / Skills, third-party agents as workspace members (2025–2026). | Reported by multiple 2026 guides; **vendor-specific names/scope `[VERIFY]`**. | Phase 7 external use. |
| GitHub Copilot **agentic code review** + **coding agent** (issue→PR, self-review-before-review), shipped ~Mar 2026; code-review GA ~Apr 2025. | Reported by GitHub blog 2025–2026; **`[VERIFY]` exact GA dates/scope**. | Phase 7. |
| GitHub "Files changed" rebuild: preview 2025-06-26, **default 2026-01-22**, batched suggestions, annotations side-panel, added keyboard/SR support. | PROVEN — GitHub changelog (dated). | — (dated, but re-confirm "default" stuck). |
| Slack **Work Objects** richer-than-app-unfurl interactive previews. | Reported — Slack/Salesforce help; **`[VERIFY]` rollout/availability**. | Phase 7. |
| Notion **2025 "data sources"** split (database = container of ≥1 data source; one view still can't span two sources). | PROVEN — notionapps.com 2025; **`[VERIFY]` current as Notion iterates**. | Phase 7. |
| Linear's local-first/IndexedDB/MobX optimistic mechanics + rollback flicker. | PROVEN — performance.dev technical breakdown (2025), consistent with Linear eng talks. | — (architecture-stable). |
| Zulip mandatory stream+topic model, per-user topic follow/mute, digest catch-up. | PROVEN — almtoolbox 2026; Slack-vs-Zulip comparisons. | — (model-stable). |

**Honest limitation.** This is an **expert teardown from public docs, vendor changelogs, and reviews — not a
hands-on walkthrough with live accounts** (no user/test-account access in this autonomous run). The behaviour
claims are grounded in cited 2024–2026 sources and the design-language's own prior teardown in
`competitive-landscape.md`, but UI specifics (exact key bindings, pixel-level layout) should be spot-checked
hands-on before any are treated as load-bearing in a sketch. This is **not** a deferred-until-users item (R-01
has `user-dep: none`), so there is no `[DEFERRED-UNTIL-USERS]` plan — the no-user substitute *is* the
deliverable.

---

## 10. Sources (web-verified, 2024–2026)

- Linear speed / sync engine technical breakdown: https://performance.dev/how-is-linear-so-fast-a-technical-breakdown
- Linear keyboard shortcuts & palette (reviews): https://www.tooljunction.io/ai-tools/linear-app · https://keycombiner.com/collections/linear/
- Linear AI agents / Agent API / Skills (2026): https://www.oflight.co.jp/en/columns/linear-ai-agent-triage-intelligence-2026 · https://blog.buildbetter.ai/linear-ai-agents-2026-guide-5-alternatives-for-engineering-teams/ · https://www.idlen.io/news/linear-agent-issue-tracking-dead-ai-agents-product-management/
- Notion writing/editing & slash menu: https://www.notion.com/help/guides/writing-and-editing-basics
- Notion block model deep-dive: https://carlosrayala.com/deep-dive-into-notion-blocks/ · https://www.techaheadcorp.com/blog/tech-stack-powering-notion-block-based-editor/
- Notion 2025 data-sources update: https://www.notionapps.com/blog/notion-data-sources-update-2025
- Notion block API reference: https://developers.notion.com/reference/block
- Slack unfurling (dev docs): https://docs.slack.dev/messaging/unfurling-links-in-messages/ · https://help.salesforce.com/s/articleView?id=slack.digital_hq_slack_apps_rns_unfurling.htm
- URL unfurling mechanics: https://dev.to/eatyou_eatyou_d79d27e5622/url-unfurling-how-slack-discord-and-twitter-generate-link-previews-5hgb
- Zulip stream+topic model: https://www.almtoolbox.com/blog/zulip-chat-overview/ · Slack-vs-Zulip: https://clickup.com/blog/zulip-vs-slack/
- GitHub "Files changed" rebuild changelogs: https://github.blog/changelog/2025-06-26-improved-pull-request-files-changed-experience-now-in-public-preview/ · https://github.blog/changelog/2025-12-11-review-commit-by-commit-improved-filtering-and-more-in-the-pull-request-files-changed-public-preview/ · https://github.blog/changelog/2026-01-22-improved-pull-request-files-changed-page-on-by-default/
- GitHub reviewing proposed changes (batched/suggested): https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/reviewing-changes-in-pull-requests
- GitHub Copilot code review / coding agent (2025–2026): https://github.blog/news-insights/product-news/github-copilot-meet-the-new-coding-agent/ · https://github.blog/ai-and-ml/github-copilot/whats-new-with-github-copilot-coding-agent/ · https://docs.github.com/copilot/using-github-copilot/code-review/using-copilot-code-review

---

## 11. §5-shared-component coverage matrix (acceptance criterion #1 — every §5 component has ≥1 entry)

| §5 shared component | North-Star teardown entry behind it |
|---|---|
| §5.1 Navigation shell | §4.1 (PR context pane as contextual shell) + §6 Axis-2 mapping (rail vs palette vs contextual) |
| §5.2 Command palette | **Linear §1.1** + Slack slash-commands §3.2 (palette/action symmetry) |
| §5.3 Reference chip + unfurl | **Slack §3.1** + Notion mention chip §2.4 + GitHub `#123`/PR pane §4.1 |
| §5.4 Agent / HITL approval card | **Linear agents §1.4** + GitHub suggestion-as-proposed-effect §4.3 |
| §5.5 Comments / threads / mentions / reactions | **GitHub batched review §4.3** + Slack threads/reactions §3.3/§3.4 + Notion mentions §2.4 |
| §5.6 Tables/boards/views | **Notion database/views §2.2** + Linear board/list/triage §1.3 |
| §5.7 Search & find | Linear palette-search §1.1 + GitHub annotations/filtering §4.2 (search-in-context) |
| §5.8 Notifications inbox | GitHub batched-review→one-event §4.3 + Linear triage queue §1.3 + Slack threading-for-calm §3.3 |
| §5.9 Block / rich-text editor | **Notion block editor §2.1** + slash menu §2.3 + Slack composer §3.4 |
| §5.10 Cross-cutting states | Linear optimistic-rollback §1.2 + the §7 routing of permission/erased/storm states |
| §5.11 Identity / presence / attribution | Slack presence/reactions §3.4 + Linear agent-identity §1.4 + GitHub reviewer/CODEOWNERS attribution §4.1 |

Every §5.x row has ≥1 entry. ✔

---

## 12. Self-check against acceptance criteria

1. **Every §5 shared component has ≥1 North-Star teardown entry behind it.** ✔ — proven by the §11 matrix
   (all of §5.1–§5.11 covered) and the inline `[→ §5.x]` tags per entry.
2. **Every "steal" is paired with the Myelin principle it serves (not "they do it").** ✔ — every "How
   Myelin adapts it" line names the specific P1–P9 principle and §5/§7 surface (e.g. palette→P1/P3;
   unfurl→P6; batched review→P8; views→P1/P5/§2). The §5 synthesis table makes the pairing explicit.
3. **Time-sensitive agent/AI features are dated and `[VERIFY]`-flagged.** ✔ — Linear agents (§1.4), GitHub
   Copilot review/coding agent + Files-changed rebuild (§4.2/§9), Slack Work Objects (§3.1), Notion data-
   sources (§2.2) are each dated 2025–2026 and carry `[VERIFY]`; consolidated in the §9 register; file
   dated 2026-06-20.
4. **Reads as a Phase-7 "meets/beats the North Star or regresses" baseline.** ✔ — every entry carries the
   trap that defines "regresses," the §5 synthesis defines the beat-the-seam test, §8 maps entries to the
   exact rubric dimensions, and §4.2/§4.4 mark GitHub's a11y/colour baselines as *beat-not-match* so parity
   isn't mistaken for G1 conformance.
5. **PROVEN/HOUSE-STYLE tagging + honesty.** ✔ — claims tagged inline; §9 names the expert-teardown-not-
   hands-on limitation and the `[VERIFY]` items; correctly notes R-01 has no deferred-until-users sub-part.
6. **Completeness-critic §9 gloss-risks addressed (named/routed or consciously deferred).** ✔ — §7.
7. **Funnel actionability (6 axes + comparable screens).** ✔ — §6 maps every relevant axis; §8 ties to the
   comparable screen set (shell, dense engineer surface, approachable surface, agent moment, wedge moment).

**Partial / honestly noted:** behaviour grounded in cited public docs + the prior `competitive-landscape.md`
teardown rather than live hands-on accounts (§9 limitation); vendor agent-feature specifics carry `[VERIFY]`
and should be re-confirmed at Phase 7.
