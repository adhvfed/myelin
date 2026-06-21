# R-08 — Command Palette + Search/Find Interaction Spec (the keyboard nerve-centre)

> **Phase 4 research corpus** · deliverable of prompt **R-08** (workstream ws-c, Seq #8).
> **File date: 2026-06-20.** Methods: **#2 (Linear/Notion/Raycast palette teardown bar)**,
> **#20 (cognitive walkthrough — "can a new PM discover what an engineer reaches by muscle
> memory?")**, **#19 (P1–P9 heuristics)**.
>
> This file specs the **one keyboard nerve-centre (P3)** present on **every screen**, unifying
> **navigate + act + search + build-query** over the R-06 IA, composing **one query AST** (ADR-07),
> **permission-pre-filtered** (ADR-03), and **agent-tool-symmetric** (the typed `ToolDef`s agents
> use, ADR-08). It then specs the **search *view*** as the palette's heavyweight sibling.
>
> **Builds ON prior `04-research` (does not duplicate):**
> - [R-01 teardown-dossier](../north-star/teardown-dossier.md) §1.1 (Linear palette = local-pool
>   instant, recognition+recall dual, the **discoverability-cliff trap**), §2.3 (Notion slash menu),
>   §3.2 (Slack slash-commands = same action vocabulary). The "meets/beats Linear or regresses" bar
>   is R-01's; this file makes it implementable.
> - [R-06 platform-ia](../ia/platform-ia.md) §2 (the tree the palette **navigates, never invents**),
>   §4.1/§4.2 (palette + search as shell-owned global surfaces), §5 (`ArtifactRef` address space —
>   every result is an `ArtifactRef`), §6.2/§6.3 (persona-adaptive vocabulary + the synonym-mapping
>   the palette must resolve), §7 (per-role landing the palette can jump to).
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = cited standard / vendor behaviour / an existing
> architecture contract we *surface* (ADR-03/07/08/13, §5.1/§5.2/§5.7). **HOUSE STYLE** = our design
> synthesis / taste. **`[VERIFY]`** = time-sensitive (re-confirm before Phase-7 external use).
> This item is **`user-dep: none`** — the cognitive walkthrough (#20) is the no-user substitute and
> *is* the deliverable; there is no `[DEFERRED-UNTIL-USERS]` core, but §11 records the one
> discoverability study worth running once users exist.

---

## 0. How to read this file

1. **§1 — The one-sentence thesis** + the four modes it serves.
2. **§2 — The four modes** (Navigate / Act / Search / Build-query) with their triggers, behaviour,
   and the seamless transitions between them.
3. **§3 — The keyboard model** (the complete contract: open, type, move, select, scope, escape).
4. **§4 — The query-AST surfacing** — how a human builds the *same* AST as a saved view / agent
   trigger, humanly.
5. **§5 — Result model & ranking** — grouping, ranking signals, the result row anatomy.
6. **§6 — The permission-pre-filter guarantee as a UX behaviour** (you can only find what you may
   see — graceful, never leaks a title).
7. **§7 — Human↔agent tool-catalogue symmetry** — palette actions = agent `ToolDef`s.
8. **§8 — The full state set** (empty / loading / no-results / no-access / error + the in-between
   states the happy path skips).
9. **§9 — The search *view*** (the heavyweight sibling: facets, scoping, multilingual).
10. **§10 — Cognitive walkthrough (#20)** — the new-PM discoverability path, walked.
11. **§11 — a11y (G1), §12 — rubric/funnel actionability, §13 — completeness-critic, §14 —
    self-check + uncertainties.**

---

## 1. The thesis (the one sentence this spec defends)

> **The command palette is the single keyboard surface from which a user can reach any artifact
> (Navigate), run any permitted action (Act), find anything they may see (Search), or compose a
> filter (Build-query) — over *one* IA (R-06 §2), through *one* query AST (ADR-07), against *one*
> permission-pre-filter (ADR-03), using the *same* typed action catalogue agents use (ADR-08).**
> *(HOUSE STYLE thesis; each clause is a PROVEN architecture contract surfaced as UX.)*

Three load-bearing consequences, each a design rule rather than an aspiration:

- **One palette, every screen** (PROVEN — §5.2). `⌘K` / `Ctrl-K` opens the *same* component in
  Code, CI, Issues, Knowledge, Chat, and admin. If a subsystem ships its own palette, the product
  has fractured (R-06 §4 global-surface invariant). The palette is **scope-aware** (it knows the
  open artifact) but **shell-owned** (the component is identical everywhere) — serves **P1**.
- **The palette navigates the tree; it never invents a parallel one** (PROVEN — R-06 §4.1). Every
  jump target and every action is an `ArtifactRef` / `ToolDef` that already exists in the IA and is
  permission-visible. There is no "palette-only" object.
- **Recognition *and* recall in one surface** (PROVEN heuristic #19 — Nielsen "flexibility &
  efficiency of use" / "recognition rather than recall"; the Linear/Notion/Raycast dual, R-01 §1.1).
  Novices **browse** (the palette is the discovery surface); experts **type/recall** (the palette is
  the speed surface). Same component — so the new PM and the muscle-memory engineer meet there
  (§10).

---

## 2. The four modes (Navigate · Act · Search · Build-query)

The palette is **one input with mode *signalling*, not four palettes.** The mode is inferred from
what the user types and is shown explicitly so the user is never guessing which mode they are in
(heuristic #19 visibility of system status). *(Mode taxonomy HOUSE STYLE, built on the §5.2
contract; the three-mode core — navigate/search/action — is a PROVEN convergent pattern,
[uxpatterns.dev command palette](https://uxpatterns.dev/patterns/advanced/command-palette).)*

### 2.1 The mode model

| Mode | Trigger (how you enter it) | What it does | Default result emphasis |
|---|---|---|---|
| **Navigate** (default) | Open palette; type a name fragment ("payme…") | Jump to any container/item/sub-artifact in the R-06 tree (repo, PR, issue, page, channel, run, view, person, **agent**, admin console) | Recent + best-name-match `ArtifactRef`s |
| **Act** | Type a verb, or `>` prefix ("`>` create issue", "transition…", "re-run") | Run a permitted action = a typed `ToolDef` (§7); may target the focused artifact or prompt for a target | Verbs (with the artifact they act on inline) |
| **Search** | Press `Tab`/`Enter` on the "Search for '…'" row, or `?` prefix | Hand off the typed text to the full cross-artifact engine; results inline, "Open in Search view" overflow (§9) | Full-text + structured matches, type-grouped |
| **Build-query** | Type a field token ("`status:`", "`assignee:@me`", "`#`", "`is:`") or press `⌘F` inside a views surface | Compose a **query AST** (§4) — the same AST that saves as a view or arms an agent trigger | Field/operator/value autocomplete chips |

### 2.2 Mode signalling & transitions (the part that makes it feel like one surface)

- **One input field, a mode pill.** A small **mode pill** at the input's start (`logical-start`, RTL-
  safe — R-18) reads `Go to` / `Run` / `Search` / `Filter`, set from the parse of the current input.
  It is **a label, never a colour-only cue** (G1, §8b.3). *(HOUSE STYLE.)*
- **Prefix verbs are an accelerant, not a requirement.** `>` (act), `#` (artifact/ref), `@` (person/
  agent), `?` (search), `/` (in-editor block insert, defers to §5.9). A user who types plain text
  still gets the right results — prefixes only *sharpen* (the Raycast/Slack model, R-01 §3.2;
  [Raycast 2026](https://raycast-discount-code.com/blog/raycast-2026-updates)). **No prefix is ever
  *mandatory*** — mandatory syntax is the discoverability cliff (§10, R-01 §1.1 trap). *(HOUSE
  STYLE.)*
- **Seamless promotion.** Typing in **Navigate** that matches no name but matches content silently
  surfaces a "Search for '…'" row at the bottom (`Tab` to promote to **Search**). Typing a known
  field key in **Navigate** ("status:") promotes the input into **Build-query** in place (the chip
  appears). **No modal re-entry** — the mode changes under one continuous keystroke stream. *(HOUSE
  STYLE; the "plan empty/loading/error in the same container" + "don't make controls feel
  disconnected" anti-pattern guidance, uxpatterns.dev.)*
- **Scope is orthogonal to mode** (§3.4): in any mode the user can narrow to *this repo / this
  space / this project* without leaving the input.

---

## 3. The keyboard model (the complete contract)

The palette is implemented as an **editable combobox with a listbox popup** — the PROVEN-accessible
primitive for "type to filter, arrow to choose" (PROVEN —
[WAI-ARIA APG Combobox pattern](https://www.w3.org/WAI/ARIA/apg/patterns/combobox/);
[editable combobox with list autocomplete](https://www.w3.org/WAI/ARIA/apg/patterns/combobox/examples/combobox-autocomplete-list/)).
**DOM focus stays on the input; `aria-activedescendant` points at the active row** (PROVEN — APG;
this is what lets typing and navigating coexist without focus jumps, and what keeps the active row
scrolled into view at 200 % zoom, G1). The keymap below is the binding contract; every binding is
also reachable by pointer (P3's second half).

### 3.1 The core keymap

| Key | Behaviour | Source |
|---|---|---|
| `⌘K` / `Ctrl-K` | Open the palette from anywhere (global). `⌘K` again or `Esc` closes. | PROVEN — §5.2; ubiquitous (uxpatterns.dev) |
| *(type)* | Filters/parses live; updates the listbox + the mode pill | PROVEN — APG list-autocomplete |
| `↓` / `↑` | Move active row (wraps at ends); `aria-activedescendant` follows | PROVEN — APG |
| `Home` / `End` | First / last row | PROVEN — APG combobox |
| `Enter` | Execute the active row (Navigate→open · Act→run · Search-row→promote · query-value→commit chip) | PROVEN — APG |
| `⌘Enter` / `Ctrl-Enter` | **Open in background / secondary target** (e.g. open in split, or "run and stay") | HOUSE STYLE |
| `Tab` | **Drill / promote**: complete the active suggestion *into* the input (e.g. `status:` → ready for a value; "Search for '…'" → Search mode) — Tab does **not** leave the palette while it has a completion to offer; with none, `Tab` exits to the next focusable element | HOUSE STYLE over APG (Tab-to-close is the APG default; we overload it for completion first) |
| `→` | On a row that has a **peek/unfurl** (an `ArtifactRef`), open the inline peek (§5.3 chip) without leaving the palette | HOUSE STYLE |
| `←` / `Backspace`-on-empty | Pop the last query chip / scope token; collapse a peek | HOUSE STYLE |
| `Esc` | Close peek if open; else clear query if non-empty; else close palette and **return focus to the prior element** (PROVEN focus-return, APG/§8b.1) | PROVEN |
| `?` (in empty input) | Show the **keyboard cheat-sheet / mode legend** (the discoverability hook, §10) | HOUSE STYLE |

### 3.2 Bare single-key shortcuts (the muscle-memory layer) — and the discoverability bridge

Following Linear (R-01 §1.1), high-frequency actions also have **bare shortcuts** outside the
palette (`C` create, `G` then `I` go-to-inbox, `/` focus search). **Binding rule (HOUSE STYLE, the
anti-cliff):** *every* bare shortcut's action is **also** listed in the palette with its shortcut
shown on the right of the row, so the palette is the **single discoverable index of the keyboard
model**. A user never has to know a shortcut exists to find the action — they find it by name in the
palette and *learn* the shortcut from the row. This is the design answer to R-01 §1.1's
"discoverability cliff" trap and the #20 walkthrough (§10).

### 3.3 Reduced-motion & no-trap (G1)

- **No focus trap beyond the modal contract:** focus is trapped *within* the open palette (it is a
  modal overlay, §8b.1 portal/focus-trap/return-focus mandates) and released cleanly on `Esc` with
  return-focus. No dead ends. *(PROVEN — §8b.1; G1 "no keyboard trap".)*
- **Reduced-motion first-class:** open/close uses an opacity/position token that **collapses to an
  instant show** under `prefers-reduced-motion` — the palette never *animates in* (§8b.6 "pages
  render, they don't animate in"; motion craft deferred to R-12). *(PROVEN constraint.)*

### 3.4 Scope control inside the palette

A **scope chip** (`logical-start`, after the mode pill) shows the current search scope, seeded from
the open artifact (e.g. opening the palette inside a repo seeds `in:repo/payments`). The user can:
`⌘⇧K` to toggle scope global↔local; `Backspace` on the empty input to clear the scope chip; or type
`in:` to set scope explicitly. Scope is part of the query AST (§4), so "search in this space" and "a
saved view scoped to this space" are the *same* mechanism. *(HOUSE STYLE over ADR-07.)*

---

## 4. The query-AST surfacing (humanise the same AST views & agents use)

This is the prompt's central technical ask: the palette must let a human compose **the same query
AST** (ADR-07) that backs **saved views** (§5.6) and **agent triggers** (ADR-08) — surfaced
*humanly*, not as raw JSON. *(The AST + one-parser claim is PROVEN — §5.2/§5.6/ADR-07; the
humanisation grammar below is HOUSE STYLE.)*

### 4.1 The token→chip→AST pipeline

Typing a recognised **field key** turns into a **filter chip**; the chips *are* the AST, rendered:

```
input:   status: in-progress  assignee:@me  label:billing  updated:<7d
                       ▼ each token becomes a chip; the row of chips IS the AST

AST:     { and: [ {field:"status",  op:"eq", value:"in_progress"},
                  {field:"assignee",op:"eq", value:"@me"},
                  {field:"label",   op:"has",value:"billing"},
                  {field:"updated", op:"lt", value:"now-7d"} ] }
```

- **Field autocomplete is permission- and schema-aware** (you are offered only fields you may filter
  on, for the scoped object type — ADR-03/ADR-06). Typing `status:` opens a **value listbox** of the
  permitted enum values (same APG combobox-in-combobox; each value a row). *(PROVEN pre-filter;
  HOUSE STYLE surfacing.)*
- **Operators are humanised:** `updated:<7d` ⇒ "updated in the last 7 days"; `is:open`, `is:mine`,
  `has:linked-pr` are saved shorthands that expand to AST clauses. The chip shows the **human**
  phrase; the underlying AST is canonical. This is the same humanise-machine-strings rule as §8b.5
  (no raw `status_in_progress` shown). *(HOUSE STYLE over §8b.5.)*
- **Free text + structured coexist:** `payment status:open` = full-text `payment` AND
  `status:open` — text terms and chips compose in one AST (the live-filtering model that replaced
  submit-then-pray query builders —
  [UXPin advanced search UX 2026](https://www.uxpin.com/studio/blog/advanced-search-ux/)). *(PROVEN
  pattern.)*

### 4.2 The three lives of one AST (why this matters)

| The user does | The AST becomes | Surface |
|---|---|---|
| Builds a filter in the palette and presses **`⌘S`** | a **saved view** (named, shareable, permissioned) | §5.6 views; appears in the IA tree's views node (R-06 §3.2) |
| Builds a filter and presses **Enter** | a **live result set** | inline / Search view (§9) |
| Builds a filter and chooses **"Arm as agent trigger"** (where authorised) | an **agent trigger condition** (ADR-08) | agent governance (R-14/R-15) |

> **Coherence pay-off (HOUSE STYLE, rubric D4):** a power user learns the filter grammar **once** and
> uses it to search, to save a board, and to arm an agent. A teammate can read any of the three as the
> same human chips. This is "one query language, surfaced humanly" (§5.2) made concrete — and a
> *checkable* coherence property for Phase 7: the chip row in the palette must be visually the same
> grammar as the filter bar on a saved view.

### 4.3 Persona-adaptive vocabulary in the query surface (R-06 §6.3)

The palette must resolve **lens labels to the canonical field**: a PM who types "work item" or
"deliverable" and an engineer who types "issue" land on the **same** object type (synonym-mapped per
R-06 §6.3's bounding rule — canonical type/ref/icon never vary). The chip renders in the **viewer's
lens label**, the AST stores the **canonical** key. *(Surfacing of R-06 §6.3 PROVEN-bounded mapping;
HOUSE STYLE in the palette.)* This is part of why the §10 walkthrough succeeds for a PM who does not
know the engineer vocabulary.

---

## 5. Result model & ranking

### 5.1 Result row anatomy (one row shape, all modes)

Every row is an `ArtifactRef`-or-`ToolDef` rendered as a compact line — **the same atoms as the
§5.3 reference chip** (R-09), so a result and a chip are visibly one family (P1):

```
[icon] Primary label (lens-aware)           ·  status hint        [ kbd shortcut ]
       secondary: scope · type · "why" (e.g. "assigned to you", matched-field snippet)
```

- **`icon`** = canonical type glyph (NOT colour-only — G1; the icon disambiguates PR vs issue vs run
  for colour-blind users). **`status hint`** = glyph + label (e.g. ✓ "Passing", ◑ "In review") —
  never colour alone (§8b.3, G1). **`why` line** = match provenance ("matched in title", "you're
  assignee") — recognition over recall (#19). *(HOUSE STYLE over §5.3 / §8b.3.)*

### 5.2 Ranking signals (HOUSE STYLE; ordered by weight)

1. **Recency & frequency for *this* user** — recently opened / acted-on `ArtifactRef`s rank first on
   an empty or short query (the "recent-first palette" variant, uxpatterns.dev; Linear's local pool,
   R-01 §1.1). The empty palette is a **recents list**, never blank (§8.1).
2. **Scope proximity** — items in the current scope (this repo/space/project) outrank distant ones.
3. **Match quality** — exact-prefix > fuzzy-subsequence > full-text-body; name matches outrank
   body matches.
4. **Type priors per surface** — on a code surface, PRs/files rank above docs; on a knowledge
   surface, pages rank above runs. (Scope-aware, not a fixed global order.)
5. **Actions surface for the focused artifact first** — if a PR is open, "Approve PR", "Re-run
   checks", "Request review" rank at the top of Act mode.

**Ranking is never a permission signal** — see §6: an item the user can't see is *absent*, it does
not rank low.

### 5.3 Grouping

Results group into labelled buckets in a stable order — **Recent · Navigate (by type) · Actions ·
"Search everywhere"** — so the eye learns where each kind lives ("separate commands, pages, recent
items into understandable buckets" — uxpatterns.dev). Group headers are non-interactive `role`
separators; arrow-nav skips them. *(PROVEN grouping pattern; HOUSE STYLE bucket order.)*

---

## 6. The permission-pre-filter guarantee, *as a UX behaviour*

This is a **correctness and GDPR property surfaced as UX** (PROVEN — §5.2/§5.7; ADR-03
`list-objects`; "a user must never find or see what they cannot access", system-overview §5.2). The
spec states it as observable UX behaviour, not backend prose:

1. **Pre-filtered, never post-filtered.** The palette/search asks Id's `list-objects` for *the set
   the viewer may see*, then ranks within it. Results the user cannot access are **absent from the
   candidate set** — they are never fetched-then-hidden, never greyed, never count-shown. *(PROVEN —
   ADR-03.)*
2. **No title leak, ever.** A no-access target is **not** rendered as a dimmed row with a real title.
   If a user pastes/types a *specific* `ArtifactRef` they lack access to (e.g. a deep link a
   colleague sent), the palette resolves it to the **graceful no-access row** (R-09's no-access card,
   compact form): *"You don't have access to this item"* — **no title, no type-specific metadata, no
   existence-vs-deletion distinction** that could be an oracle. *(PROVEN invariant — §5.3 hard rule /
   ADR-03; HOUSE STYLE wording.)*
3. **Counts don't leak either.** "12 results" counts only permitted results. A facet (§9) never shows
   "Issues (3 hidden)". The absence is silent. *(HOUSE STYLE over ADR-03.)*
4. **Cross-tenant/cross-cell** refs resolve to a projection **if visible, else the same no-access
   row** — never a raw id, never the home-cell title (R-06 §5.4; R-19 owns residency depth). *(PROVEN-
   as-open — R-06 §5.4.)*
5. **The UX is identical to "doesn't exist".** Crucially, the no-access and the not-found states are
   **indistinguishable** to the user, so the palette cannot be used to probe whether an artifact
   exists. *(HOUSE STYLE — this is the anti-oracle rule; it is a deliberate, testable behaviour.)*

> **Why this is a *feature*, not a limitation (HOUSE STYLE):** for the DPO/security personas (P12/
> P13), "you can only find what you may see, and search can't be turned into a reconnaissance tool"
> is a *trust* affordance (P9 sovereignty-as-UX). It is also the thing every North Star is weak at
> (R-01 §2.4 Notion mention-leak, §3.1 Slack snapshot-unfurl) — a **beat-not-match** for Phase 7.

---

## 7. Human ↔ agent tool-catalogue symmetry

**Palette actions *are* the typed `ToolDef`s agents use** (PROVEN — §5.2 / ADR-08). This is not a
metaphor: there is one catalogue of typed, permission-scoped capabilities, and both a human (via
Act mode / chat slash-command) and an agent invoke from it.

| Property | Human in palette (Act mode) | Agent | Why symmetric |
|---|---|---|---|
| **Source** | The `ToolDef` catalogue | The same `ToolDef` catalogue | One vocabulary (P1) |
| **Permission** | `list-objects` pre-filter — you only see verbs you may run on the target | The agent's delegated scope filters the same set (ADR-08) | One ReBAC check (ADR-03) |
| **Typed args** | The palette prompts for the `ToolDef`'s typed parameters (a "transition issue" verb prompts for the target state from the permitted enum) | The agent fills the same typed args | One schema → identical validation/audit |
| **Audit** | The action is attributed to the human `Principal` + `correlation_id` | …to the agent `Principal` + `correlation_id` | One audit trail (R-15) |
| **Plan-then-apply** | A *consequential* human action can show the same confirm/effect summary an agent's plan card shows (§5.4) | The agent's plan-then-apply card (ADR-08) | A human suggestion and an agent proposal are the **same shape** (R-01 §4.3) |

**Design consequences (HOUSE STYLE):**
- **Discoverability of agent capability:** because agent tools and human verbs are one catalogue, a
  human can *see what an agent could do* by browsing the palette's Act mode filtered to a target —
  the agent's power is legible, not hidden (P7).
- **Consequential verbs carry their consequence in the palette.** Running "Delete branch" / "Erase
  data subject" from the palette surfaces the same **plan/confirm** affordance (effects + scope) the
  agent's HITL card shows — the palette is not a fast path *around* the gate (R-14 owns the card;
  the palette **routes into** it). *(HOUSE STYLE; the no-fast-path-around-the-gate rule is the
  P12/P13 trust requirement.)*
- **Slash-command parity:** `/transition`, `/run`, `/assign` typed in the chat composer resolve
  through the **same** catalogue as the palette's Act mode and the agent (R-01 §3.2). One action
  vocabulary across human-in-palette, human-in-chat, and agent. *(PROVEN — §5.2 / ADR-08.)*

---

## 8. The full state set (the happy path is the smallest part)

Per the prompt's required set + the states the happy-path bias skips. Each maps to §8b.6 "plan
empty/loading/error in the same container" (uxpatterns.dev anti-pattern: *"the pattern feels
polished until loading, empty, and failure states appear"*). State-craft depth is R-21's; here is the
palette's own enumeration. *(State set PROVEN-required — §5.10 / §8b.6; treatments HOUSE STYLE.)*

| State | Trigger | UX behaviour |
|---|---|---|
| **Idle / empty input** | Palette opens, nothing typed | **Recents + suggested actions for the focused artifact** (never a blank box). The `?` cheat-sheet hint is visible. This is the onboarding-forward empty (§5.10). |
| **Loading** | Query needs a round-trip (search engine, cross-cell) | **Structure skeleton rows** in the same container, *not* a centred spinner. **Suppress flash <~1 s** (§8b.6): local-pool results render <100 ms; only the network-dependent overflow shows skeletons. The input stays live; typing more never blocks. Announce "loading results" politely once (`aria-live=polite`, not per keystroke — G1 no-spam). |
| **No results** | Query parses, matches nothing the user may see | One quiet line: *"No matches for '…'."* + the **escape hatches**: "Search everywhere", "Create … named '…'" (Act), "Clear filters". Explains what to do (uxpatterns.dev). Never blames the user. |
| **No access** (specific ref) | A targeted `ArtifactRef` the viewer can't see | The **graceful no-access row** (§6.2): *"You don't have access to this item"* — no title, indistinguishable from not-found. |
| **Error** | Search backend / resolver fails | One quiet line that **blames the system**: *"Search is temporarily unavailable — local results shown."* The palette **degrades to the local pool** (recents + in-scope cached) so Navigate/Act still work (fails *useful*, not blank — §8b.6 "fails static/useful"). Retry affordance. |
| **Permission changed mid-session** | A cached recent the user lost access to | Silently dropped from results on next resolve; if mid-open, the row tombstones to no-access (R-09). |
| **Stale / offline** | Firehose dropped; cache may be stale | A subtle "results may be out of date" cue on the local-pool fallback; refresh on reconnect (R-21 owns the reconnecting craft). |
| **Erased / tombstoned** | A recent points at an erased artifact (ADR-12) | The row renders as a **tombstone** ("This item was deleted"), never a dangling title (R-09 / §5.3 hard rule). |
| **Agent-armable** | Build-query mode, user has agent-trigger authority | An "Arm as agent trigger" row appears (§4.2); absent if unauthorised (no teasing a verb you can't run). |

---

## 9. The search *view* (the palette's heavyweight sibling)

When a query outgrows the palette (faceting, scanning many results, refining iteratively), the user
promotes to the **Search view** — a destination node in the IA (`[G] Search`, R-06 §4.2). **Same
engine, same AST, same permission pre-filter, same result-row family** — the difference is *room*,
not a second search product (PROVEN — §5.7 "two entry points, one engine"). *(View layout HOUSE
STYLE over §5.7.)*

### 9.1 Anatomy

```
┌─ Search ────────────────────────────────────────────────────────────────────┐
│  [query input — same chips/AST as the palette]                  [Save view ⌘S]│
├───────────────┬───────────────────────────────────────────────────────────── ┤
│  FACETS       │  RESULTS (same row family as palette; grouped or flat)        │
│  Type ▸       │   [icon] label · why · status              [open ▸ peek]      │
│   ☑ Issues 12 │   …                                                           │
│   ☑ PRs 4     │                                                               │
│   ☐ Docs      │  (permission-pre-filtered; counts = permitted only, §6)       │
│  Subsystem ▸  │                                                               │
│  Scope ▸      │                                                               │
│  Updated ▸    │                                                               │
│  Language ▸   │                                                               │
└───────────────┴────────────────────────────────────────────────────────────── ┘
```

- **Facets = AST clauses with counts.** Toggling a facet adds/removes a chip in the *same* AST —
  facet UI and palette chips are two renderings of one query (the live-faceted model, UXPin 2026).
  Counts obey §6 (permitted-only; no "(n hidden)"). *(PROVEN engine; HOUSE STYLE facet↔chip
  equivalence.)*
- **Type / subsystem scoping** is a first-class facet (search across all five, or scope to Code /
  Knowledge / one repo / one space) — the cross-artifact-by-default-but-scopable rule (§5.7).
- **Multilingual (G2):** the engine is multilingual and (later) semantic/vector (ADR-10/ADR-14); the
  **`Language` facet** lets a user scope or widen by content language; results render in their own
  script (Greek/Cyrillic/RTL) without clipping (R-18 owns the rendering; flagged here as a facet the
  view owns). Query input accepts non-Latin scripts; the chip grammar is logical-property-based for
  RTL. *(Engine PROVEN — §5.7/ADR-10; the language facet HOUSE STYLE; rendering → R-18.)*
- **Saved searches = saved views** (§4.2) — the Save action here produces the same first-class,
  shareable, permissioned view object (§5.6). A "search" and a "view" are the same artifact at two
  moments. *(PROVEN — §5.6/ADR-07.)*

### 9.2 Search-view state set
Inherits §8 (empty = "search across everything you can see" + recents; loading = result skeletons;
no-results = escape hatches; error = degraded; no-access = absent/anti-oracle). Adds **per-facet
loading** (facet counts may stream in after results) and **deep-link**: the full query is in the URL
(`/search?q=…` resolving to the AST, R-06 §5) so a search is shareable — and the shared link
re-runs **against the recipient's** permissions, not the sender's (§6). *(HOUSE STYLE over §5.7/
ADR-03.)*

---

## 10. Cognitive walkthrough (#20) — "can a new PM discover what an engineer reaches by muscle memory?"

The prompt's decisive no-user test. We walk a **new PM (P6)** — no keyboard muscle memory, does not
know the engineer vocabulary — doing a real job, against the three #20 questions: *(1) will the user
know what to do? (2) will they see the control? (3) will they understand the feedback?* *(Method
PROVEN — Nielsen/Wharton cognitive walkthrough; the walk is HOUSE STYLE analysis, the no-user
substitute for this `user-dep: none` item.)*

**Goal:** "find the work item I'm tracking for the billing launch and re-prioritise it." An engineer
would do this in ~2 s by muscle memory (`⌘K`, type, `Enter`). Can the PM?

| Step | (1) Know what to do? | (2) See the control? | (3) Understand feedback? | Verdict |
|---|---|---|---|---|
| Open the palette | The PM may not know `⌘K`. **But** a persistent, *visible*, pointer-clickable search/`⌘K` affordance sits in the top bar (R-06 §3, P3 second half). The hint shows `⌘K`. | ✅ visible affordance, not keyboard-only | The palette opens with **recents** (not blank) — immediately legible | ✅ — the visible affordance is the anti-cliff (R-01 §1.1 trap mitigated) |
| Find the item | The PM types "billing" in plain language — **no prefix required** (§2.2). | ✅ plain text works; mode pill says "Go to" | Results show with **lens-aware labels** ("Work item", not "Issue" — §4.3) and a "why" line | ✅ — synonym-mapping (R-06 §6.3) means the PM's vocabulary lands |
| Re-prioritise | PM doesn't know there's a "set priority" verb. They **open the item** and act there (pointer path), **or** type "priority" and Act mode surfaces "Set priority" with the artifact inline. | ✅ both a pointer path and a discoverable verb | The Act row shows the verb + the target + (on run) an optimistic state change | ✅ — recognition path exists; no recall required |
| Learn the fast path | After doing it once, the PM notices the palette row showed the **shortcut on the right** (§3.2) and the `?` cheat-sheet. | ✅ the shortcut is *shown*, not assumed | Next time they may type it | ✅ — the palette **teaches** the muscle-memory path it also serves |

**Walkthrough conclusion (HOUSE STYLE):** the design **passes** *iff* three rules hold, which this
spec mandates: (a) a **visible pointer affordance** for the palette (never keyboard-only entry); (b)
**no mandatory prefix syntax** (plain text always works); (c) the **shortcut is shown on the row** so
the palette is the index that teaches recall. Remove any one and the new PM hits the discoverability
cliff R-01 §1.1 warns of. **These three are the cull-checks for any palette-led Axis-2 finalist
(§12).** The residual risk that *real* PMs still fail to form the `⌘K` habit is the one thing worth
user-testing (§11) — the walkthrough is a substitute, not proof.

---

## 11. Accessibility (G1) — the palette as a named "hard component"

The command palette is one of the rubric's explicitly-named **hard components** G1 must demonstrate
keyboard-operable + screen-reader-correct (rubric G1; R-17 owns the audit method — this section is
the palette's a11y contract feeding it). Each item cites its basis. *(All PROVEN unless noted.)*

- **Combobox semantics:** `role="combobox"` input + `role="listbox"` popup + `role="option"` rows;
  DOM focus stays on the input, **`aria-activedescendant`** tracks the active row (WAI-ARIA APG
  Combobox; this is *the* pattern, not a custom invention). Active option is **scrolled into view**
  for 200 %-zoom users (APG note). Group headers are non-option separators arrow-nav skips.
- **Keyboard-complete, no trap:** the §3 keymap is total; focus is trapped within the modal and
  **returned** on close (§8b.1 mandate; G1 2.1.2 no-keyboard-trap, 2.4.3 focus-order).
- **Visible focus:** the active row uses the derived `focus-ring` token (≠ identity token, §8b.3) at
  ≥3:1, in light/dark/high-contrast (G1 1.4.11, 2.4.7/2.4.11 focus-not-obscured).
- **Status not by colour alone:** every result's status hint is glyph+label+position, the type is an
  icon not a colour (§5.1), the mode pill is a word (§2.2) — G1 1.4.1.
- **Live regions without spam:** result-count / loading / error announced via **one** `aria-live=
  polite` region, debounced — **not** per keystroke (G1 4.1.3; the uxpatterns.dev "announce in the
  right place, right politeness" rule). Errors announced as they appear.
- **i18n/RTL (G2 hook):** mode pill, scope chip, query chips, and result rows use **logical
  start/end** properties so the whole palette mirrors in RTL (R-18 owns the mirrored-state demo);
  labels are externalised tokens (R-06 §6); German-length verbs must not clip the row (§8b.4 no
  fixed-width). *(Surfaced here; R-18 owns the demonstration.)*
- **Reduced-motion:** §3.3 (instant show under `prefers-reduced-motion`).

---

## 12. Actionability toward the control artifacts

| Control artifact | What this spec equips | Where |
|---|---|---|
| **rubric D1 — power-user efficiency (12%)** | The complete keyboard model (§3) + bare-shortcut layer (§3.2) + local-pool-instant ranking (§5.2) is the literal D1 bar ("a power user crosses the whole flow on the keyboard faster than Linear"). The palette is *the* D1 surface. | §3, §5.2 |
| **rubric D4 — one-product coherence** | One palette every screen (§1); one AST across search/view/agent (§4.2); one action catalogue human↔agent (§7); one result-row family palette↔search↔chip (§5.1/§9). The "open the same palette in Code and Chat — identical?" check (R-06 §4 invariant) is testable here. | §1, §4.2, §7 |
| **rubric G1 (gate)** | The palette's full a11y contract (§11) — the named hard-component keyboard + SR entry R-17/G1 require; status-not-by-colour and no-trap explicitly covered. | §11, §3.3 |
| **rubric G2 (gate)** | Logical-property chips, externalised labels, language facet, non-Latin input (§9.1/§11) — R-18 demonstrates. | §9.1, §11 |
| **rubric D6 — agent legibility** | Human↔agent tool symmetry (§7): agent capability is *browsable* in the palette; consequential verbs route into the HITL gate, not around it. | §7 |
| **sketch-funnel Axis 2 (navigation: rail ↔ palette ↔ contextual)** | The palette is the **command-palette-led pole**; §10's three cull-checks are the test a maximally palette-led finalist must pass to avoid the discoverability cliff. A finalist's Axis-2 position is a *tuning* of this one surface over the R-06 tree. | §10, §2.2 |
| **Every finalist's wedge moment (sketch-funnel comparable screen #5)** | The palette-in-action *is* one of the two allowed wedge-moment screens (sketch-funnel §"comparable screens"). This spec is its blueprint. | all |

---

## 13. Completeness-critic (README §9) — gloss-risks this item touches

R-08 **owns** these for the palette/search surface (covers them here), and **routes** depth to the
owners:

- **Keyboard-only operability of a hard component** — **OWNED & covered** (§3 keymap, §11 combobox
  contract, §3.3 no-trap). The palette is a named G1 hard component; this is its keyboard spec.
- **Status-not-by-colour-alone** — **OWNED & covered** (§5.1 glyph+label rows, §2.2 word mode pill,
  §11). 
- **Permission-denied "no access", never a leaked title** — **OWNED & covered** as a UX behaviour
  (§6, the anti-oracle rule); the *card* visual depth → R-09.
- **Erased/tombstoned in results** — **covered** (§8 tombstone row); depth → R-09/R-21.
- **Loading (structure-skeleton, suppress flash) / error (blame-system, fail-useful)** — **covered**
  (§8); cross-surface depth → R-21/R-13.
- **Stale/offline/reconnecting** — **named** (§8); craft → R-21.
- **Multilingual/RTL** — **named & hooked** (§9.1/§11); demonstration → R-18.
- **Consciously deferred (with reason):** motion craft of open/close + optimistic-action settle
  (R-12); the no-access **card** visual + the rebase-orphaned chip (R-09); the full per-surface state
  catalogue (R-21). Naming-and-routing keeps the corpus cumulative.

---

## 14. Self-check against R-08 acceptance criteria

| Criterion (prompt R-08) | Status | Evidence |
|---|---|---|
| **Palette unifies nav + actions + search with one query AST** | ✅ Met | §2 (four modes, one input) + §4 (token→chip→AST; the three lives of one AST) |
| **Permission-pre-filter specified as UX (graceful, never leaks)** | ✅ Met | §6 (pre-filtered not post-filtered; no-title-leak; counts don't leak; **anti-oracle** = no-access indistinguishable from not-found) |
| **Keyboard model complete** | ✅ Met | §3 full keymap incl. open/move/select/promote/scope/escape/return-focus; APG combobox basis |
| **New-user discoverability path walked (#20)** | ✅ Met | §10 (new-PM walkthrough, three #20 questions per step, three cull-checks) |
| **All states enumerated** (empty/loading/no-results/no-access/error + skipped ones) | ✅ Met | §8 (9 states incl. stale/offline, tombstone, perm-changed-mid-session) + §9.2 |
| **Human/agent tool symmetry shown** | ✅ Met | §7 (one `ToolDef` catalogue; permission/typed-args/audit/plan-then-apply parity; slash-command parity; no-fast-path-around-the-gate) |
| **Search *view* specced (facets, type/subsystem scoping, multilingual)** | ✅ Met | §9 (facets=AST clauses; type/subsystem/scope/language facets; saved-search=saved-view; deep-link re-runs against recipient perms) |
| **Builds ON R-01 + R-06, doesn't duplicate** | ✅ Met | Cites R-01 §1.1/§2.3/§3.2 traps + R-06 §2/§4/§5/§6 by section; extends, not restates |
| **PROVEN/HOUSE-STYLE tags + date + cited web sources** | ✅ Met | Tagged throughout; dated 2026-06-20; APG/uxpatterns.dev/UXPin/Raycast cited inline + §15 |
| **Actionable toward rubric D1, G1 + funnel Axis 2** | ✅ Met | §12 (D1 = §3/§5.2; G1 = §11; Axis 2 = §10 cull-checks) |
| **§9 gloss-risks addressed** | ✅ Met | §13 (keyboard-op + status-not-by-colour OWNED; rest named/routed) |

**Top uncertainties (honest):**
1. **The `Tab`-overload (complete-then-exit, §3.1)** is a HOUSE-STYLE deviation from APG's plain
   Tab-to-close; it must be verified not to confuse AT users / break expectations in the R-17 audit
   and (ideally) a keyboard-AT user test. The fallback is strict APG Tab-to-close + a separate
   completion key.
2. **The #20 walkthrough is a substitute, not proof (§10/§11 deferred note).** Whether real PMs
   form the `⌘K` habit (vs. always clicking the visible affordance) is the one genuinely
   user-testable question — recorded below.
3. **Persona-adaptive synonym mapping in the query surface (§4.3)** inherits R-06 §6.3's open
   fracturing-risk: if the lens-label↔canonical synonym set is mis-bounded, a PM's "deliverable"
   query and an engineer's "issue" query could diverge. Bounded mapping is the HOUSE-STYLE bet;
   R-07's per-segment tree-test is the resolver.
4. **Ranking weights (§5.2) are HOUSE STYLE** — the exact recency/scope/match ordering is a
   hypothesis tunable with usage telemetry once it exists.

### Optional user study worth running once users exist *(not a deferred core — this item is
`user-dep: none`, but recorded for honesty)*
- **What:** A first-use task ("find and re-prioritise X") with **new PMs (P6)** and **engineers
  (P1)**, instrumented for: did they discover the palette (click vs `⌘K`); did plain-text search
  succeed without prefixes; did they learn the shortcut after one exposure. **What would falsify the
  design:** PMs systematically fail to find the palette, or require prefix syntax to get results, or
  the lens-label synonym mapping causes a cross-segment object mismatch. Pairs with R-07's
  per-segment tree-test.

---

## 15. Sources (web-verified, 2024–2026)

- WAI-ARIA APG — Combobox pattern (the accessible primitive for the palette): https://www.w3.org/WAI/ARIA/apg/patterns/combobox/
- WAI-ARIA APG — Editable combobox with list autocomplete (keyboard + `aria-activedescendant`): https://www.w3.org/WAI/ARIA/apg/patterns/combobox/examples/combobox-autocomplete-list/
- UX Patterns for Developers — Command Palette (modes, grouping, recent-first, state model + anti-patterns): https://uxpatterns.dev/patterns/advanced/command-palette
- Mobbin — Command Palette UI design (variants, ⌘K convention): https://mobbin.com/glossary/command-palette
- Raycast 2026 updates / keyboard-first command palette (prefix-accelerant model, Linear integration): https://raycast-discount-code.com/blog/raycast-2026-updates · https://windowsforum.com/threads/raycast-on-windows-a-keyboard-first-command-palette-for-fast-actions.395552/
- UXPin — Advanced Search UX 2026 (live faceted filtering replacing submit-then-pray query builders): https://www.uxpin.com/studio/blog/advanced-search-ux/
- (carried from R-01) Linear speed / local-pool palette: https://performance.dev/how-is-linear-so-fast-a-technical-breakdown

---

*End of R-08 deliverable. Date: 2026-06-20. Interaction spec HOUSE STYLE over the PROVEN contracts
(ADR-03 pre-filter, ADR-07 query AST, ADR-08 `ToolDef` symmetry, ADR-13 `ArtifactRef`) and the
PROVEN WAI-ARIA APG combobox pattern; not user-validated (§14). Builds on R-01 + R-06. Feeds R-12
(palette motion), R-18 (palette i18n/RTL), Phase 5/6, rubric D1/D4/D6/G1/G2, sketch-funnel Axis 2.*
