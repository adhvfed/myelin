# Knowledge Platform — Wireframes (primary screens)

> Phase 4, Knowledge, **design sketch** (REQUIRED before architecture). ASCII wireframes of the
> PRIMARY screens (design-language §7.4), each showing **empty / loading / error** states (plus
> permission-denied / erased where they apply) — not just the happy path. Day-one UX primitives
> (design-language §8b) are applied throughout and called out: overlay/portal rules (§8b.1), one
> editor render path (§8b.2), measured tokens (§8b.3), layout-containment (§8b.4), humanised strings
> (§8b.5).
>
> Conventions: `[ ]` button · `▸/▾` tree toggle · `▒` skeleton (loading shows STRUCTURE, never a
> blank spinner, §8b.6) · `·` muted text · `◆` agent badge (never sparkle/magic-wand, §8b.3).
> Every overlay **portals to the document root** + shares the one z-index scale (chrome < popover <
> modal < toast) + inherits focus-trap/return-focus/scroll-lock/Escape/ARIA from the shared
> primitive (§8b.1). The shell is pinned to the viewport (`100vh`/`overflow:hidden`); each region is
> its own scroller with `min-height:0` (§8b.4).

---

## S1 — Block editor (page view) — the core surface

### Happy path
```
┌──────┬──────────────────────────┬────────────────────────────────────────┬─────────────────┐
│ RAIL │ Spaces ▸ Pages           │  Incident: API 5xx spike            ⋯   │ ▸Backlinks(3)   │
│ Code │ ▾ Ops                    │  · Ops · edited 2m ago by you · 🟢 EU  │  Outline        │
│ CI   │   ▸ Runbooks             │ ┌────────────────────────────────────┐ │  Comments(1)    │
│ Issue│   ▾ Incidents            │ │ # Incident: API 5xx spike          │ │  Page info      │
│▸Know │     • API 5xx spike  ←   │ │                                    │ │ ─────────────── │
│ Chat │   ▸ Onboarding           │ │ Severity **high**. Owner @alice    │ │ Linked refs:    │
│ Inbox│ ─────────────────────    │ │                                    │ │ ◷ ci/run/991 🔴 │
│      │ ★ Favorites              │ │ /                          ┌─────┐ │ │ #ENG-412 open   │
│      │ ◷ Recent                 │ │ ┌── slash-menu (popover) ─│ Text│ │ │ ~incidents(chat)│
│      │ 🗑 Trash                  │ │ │ ▸ Heading  ▸ To-do      │ H1  │ │ │                 │
│      │ [+ New page]             │ │ │ ▸ Embed    ▸ Database    │ ... │ │ │ avatars: ⬤⬤ live│
│      │                          │ │ └─────────────────────────└─────┘ │ │                 │
└──────┴──────────────────────────┴─┴────────────────────────────────────┴─┴─────────────────┘
```
- **One editor render path (§8b.2)**: `**high**` renders bold *as typed* (controlled
  `contenteditable`, caret = char offset into the md-subset string). `@alice` is a structured
  `mention` node (atomic single-offset placeholder, sketch 02) rendered as the §5.3 chip — not a baked
  string, so rename/erase never rewrites prose.
- **Slash menu (§8b.1)**: a popover anchored to the caret, **portaled to root**, **flips above** when
  near the viewport bottom (§8b.4) — tested against the real caret anchor.
- **Live presence**: co-editor avatars (firehose, not durable bus). Status pill `🟢 EU` is the
  residency/visibility cue (P9) — but status is never colour-alone: glyph + "EU" label (§8b.3).
- **Humanised strings (§8b.5)**: "edited 2m ago by you", "ci/run/991" renders as the live unfurl chip —
  no raw ids, no `merge_request merged`-style machine strings; humanisation comes from Notif/Refs at
  the source.

### Empty (new page)
```
│ │ Untitled                                                              │
│ │                                                                       │
│ │  ·Type  /  for commands, or just start writing.                       │  ← onboarding-forward
│ │  ·Press  @  to mention someone or reference an issue, doc, or run.    │     (§5.10): explains +
│ │                                                                       │     offers the next action
```

### Loading (huge doc — partial/lazy block load)
```
│ │ ▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒  (title)                                              │  Skeletons MATCH the final
│ │ ▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒                                    │  layout (§8b.6) — never a
│ │ ▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒  ▒▒▒▒▒▒▒▒▒▒                                    │  spinner on a blank page.
│ │ ▒▒▒▒▒▒▒  [▒ embed ▒]                                                  │  Below-fold blocks load on
│ │ · loading more blocks…                                               │  scroll (lazy).
```

### Error (sync failed) / Offline / Read-only / Agent-suggesting
```
ERROR:    │ ⚠ Couldn't sync — retrying. Your edits are saved locally.   [Retry] │  ← system-blamed, one
          │   (optimistic state preserved; reconnect replays via the resume cursor — zero ops lost) │  quiet line + a path
OFFLINE:  │ ● Offline — edits queued, will sync when you reconnect.            │  (§8b.6; sketch 01)
READONLY: │ 👁 View only — you don't have edit access to this page.            │  (perm-denied state)
AGENT:    │ ◆ FixAgent suggests an edit to this block.   [Accept] [Reject]     │  agent treatment (§6.1);
          │   ·suggested by agent, on behalf of @alice                         │  NOT sparkle/magic (§8b.3)
ERASED:   │ ▦ This content was erased per a data-subject request.              │  redacted, never the PII
```
- **CAS-floor conflict** (sketch 01): editing a block another user holds →
  `│ 🔒 Alice is editing this block.│` (advisory soft-lock; no silent overwrite, no merge — the named
  floor). CRDT promotion later blends concurrent prose.

---

## S2 — Database views (table / board) — the structured surface

### Table (happy)
```
│ Roadmap DB ▸  [Table ▾] [Board] [Calendar] [Timeline]   Filter· Sort· Group·  [+ Row] [+ Field]│
│ ┌──────────────┬──────────┬──────────┬─────────────┬──────────────────┬──────────────────────┐│
│ │ Title        │ Status   │ Owner    │ Due         │ Σ Effort (rollup)│ Related              ││
│ ├──────────────┼──────────┼──────────┼─────────────┼──────────────────┼──────────────────────┤│
│ │ Auth rewrite │ ● Open   │ @bob     │ 2026-07-01  │ 13  (read-time)  │ #ENG-412 · ci/run/991││
│ │ SSO support  │ ◐ Doing  │ @alice   │ 2026-07-15  │ 8                │ #ENG-440             ││
│ └──────────────┴──────────┴──────────┴─────────────┴──────────────────┴──────────────────────┘│
```
- **Status** uses glyph + label + colour (`● Open` / `◐ Doing`) — never colour alone (§8b.3).
- **`Σ Effort`** is a **read-time rollup** (KN-3, sketch 03) — computed over the visible/related set,
  not stored. **`Related`** cells are reference chips (§5.3).
- **Permission-filtered by construction**: rows the viewer can't see are *absent*, not greyed —
  no count-leak (§5.6 / sketch 04).
- **Row peek**: clicking a title opens the row-as-page in a portaled side-panel/modal (§8b.1).

### Board (drag) / Empty / Filtered-empty / Loading / Error
```
BOARD:    │ ● Open ────────┐  ◐ Doing ──────┐  ✓ Done ───────┐   drag cards between columns        │
          │ ┌────────────┐ │  ┌───────────┐ │  ┌──────────┐  │   (optimistic move, honest rollback)│
          │ │ Auth rewrite│ │  │ SSO support│ │  │ Login fix│  │                                     │
          │ └────────────┘ │  └───────────┘ │  └──────────┘  │                                     │
EMPTY DB: │  This database is empty.  [+ Add a row]   [+ Add a property]   [Import CSV]            │  (§5.10 onboarding)
FILT-EMPTY│  No rows match this filter.   [Clear filter]                                           │
LOADING:  │  ▒▒▒▒▒▒▒  ▒▒▒▒  ▒▒▒▒▒  ▒▒▒▒▒▒  (skeleton rows; heavy query → Search structured index,  │
          │  ▒▒▒▒▒▒▒  ▒▒▒▒  ▒▒▒▒▒  ▒▒▒▒▒▒   ACL-pre-filtered + paginated, sketch 03 — never a scan) │
ERROR:    │  ⚠ Couldn't load rows. [Retry]   (the view definition + columns still render — degraded)│
```

---

## S3 — Navigation sidebar / tree (secondary nav)

```
EMPTY WS: │ Your workspace is empty.            LOADING (deep tree):  │ ▾ Ops                       │
          │ [+ Create your first page]          │ ▾ Engineering        │   ▒▒▒▒▒▒▒                  │
          │ [Use a template]                    │   ▾ Specs             │   ▒▒▒▒▒▒▒▒▒                │
SEARCH-   │ ⌘K → "auth"                         │     • ▒▒▒▒▒  ·loading │   ▒▒▒▒                     │
NO-RESULT:│ · No pages match "auth".            └──────────────────────┘  (virtualised; lazy subtree)│
```
- The tree **is** the folder hierarchy (pure-pages, sketch 04). Trash holds soft-deleted pages
  (reversibility over confirmation, §8b.6 — undo window, not "are you sure?").

---

## S4 — Backlinks / references pane (context pane — the wedge made visible, P6)

```
│ ▸ Backlinks (3)                  │  EMPTY:  · Nothing references this page yet.                  │
│ ─────────────────────────────── │  LOADING:· ▒▒▒▒▒▒▒  ▒▒▒▒▒▒▒▒▒                                 │
│ ◷ ci/run/991 · failed · "embeds"│  PERM:   · 2 references are hidden (you can't access them).   │  ← permission-filtered:
│ #ENG-412 · open · "mentions"    │           (leak-free: a hidden ref shows as a COUNT, never a   │     you see a backlink iff
│ ~incidents · chat · "links"     │            title — sketch 04 / Refs D-1)                       │     you can see its source
│                                 │  ERASED: · ▦ (reference to a removed item)                     │
```
- Hover-peek any backlink → the §5.3 unfurl card (live, per-viewer-permission-checked). The system
  assembles + pre-fetches context (§8b.6) — never sends the user to another tab.

---

## S5 — Comments / discussion (context pane + inline)

```
INLINE ANCHOR (on a text range):       │ THREAD (context pane):                                   │
│ …Severity high. Owner @alice▐──┐      │ ▸ Comments (1)                                           │
│                          💬 1 ─┘      │ ┌──────────────────────────────────────────────────────┐│
│                                       │ │ @bob · 5m   "Is high right? Saw only 2% error rate."  ││
EMPTY:  · No comments yet. [Comment]    │ │   ↳ reply…   [Resolve]                                ││
LOADING:· ▒▒▒▒▒▒  ▒▒▒▒▒▒▒▒              │ └──────────────────────────────────────────────────────┘│
ERROR:  · ⚠ Couldn't load comments.[Retry]│  composer: same one-editor render path (§8b.2)          │
```
- Comments anchor to a range/block/`#sub` (KB-native anchor; shared comment/thread primitive,
  sketch 07). `@mention` routes to the inbox; `@agent` is an agent trigger (§5.5).

---

## S6 — Page history

```
│ History ▸ Incident: API 5xx spike                         [Restore this version]                │
│ ┌── version timeline ──┐  ┌── diff (vs current) ────────────────────────────────────────────┐  │
│ │ ● now (you)          │  │  - Severity medium                                               │  │
│ │ ◷ 2m ago (you)       │  │  + Severity **high**                                             │  │
│ │ ◷ 1h ago (@bob)      │  │    Owner @alice                                                  │  │
│ │ ◷ ◆ 3h ago (FixAgent)│  │  ▦ [content erased per a data-subject request]   ← erased segment │  │
│ └──────────────────────┘  └──────────────────────────────────────────────────────────────────┘  │
EMPTY:    · No history yet.        LOADING: · ▒▒▒ timeline + ▒▒▒ diff skeleton                     │
RESTORE:  confirm dialog (irreversible-ish → confirm, §8b.6) → restore runs post-restore re-erasure │
          (can't un-erase a subject, sketch 06 / GD-14)                                            │
```
- An erased segment renders **redacted**, never the personal data (sketch 06). An agent-authored
  version carries the ◆ badge (§6.1).

---

## S7 — Sharing / permissions dialog (modal, portaled)

```
┌─ Share "Incident: API 5xx spike" ──────────────────────────────[Esc]┐   ← modal: portal-to-root,
│ Access (inherited from ▸ Incidents ▸ Ops)                            │     focus-trap, scroll-lock,
│  ⬤ @alice    Manage ▾      ⬤ Ops team   Edit ▾     [+ Invite people] │     backdrop+Esc dismiss
│  ⬤ @bob      Edit   ▾                                                 │     (§8b.1, all in the
│  · This page overrides its parent for: @bob (added)                  │      primitive)
│ ─────────────────────────────────────────────────────────────────── │
│ Link sharing:  ○ Off   ● Anyone with the link (view)                 │
│ Publish to web: ○ Off  ⚠ Publishing may expose personal data.        │   ← lawful-basis prompt at
│   [Set lawful basis…]  required before publish (sketch 06)           │      publish (P9, sketch 06)
└──────────────────────────────────────────────────────────────[Save]─┘
PERM-DENIED (opener lacks manage): "You can view this page but can't change sharing."
ERROR (save): ⚠ Couldn't update sharing. [Retry]  (no partial apply; zookie-stamped, sketch 04)
```

---

## S8 — Templates (gallery modal)

```
┌─ New from template ────────────────────────────────────────────[Esc]┐
│ [Blank page]  [Meeting notes]  [Runbook]  [PRD]  [Onboarding]        │
│  ⚠ "Customers DB" template pre-seeds personal-data fields — review   │   ← GDPR-flagged PII template
│     lawful basis on creation. (§7.6)                                 │      (knowledge-platform §4.8)
EMPTY:  · No org templates yet. [Create one from this page]            │
└─────────────────────────────────────────────────────────────────────┘
```

---

## S9 — Search / quick-switcher (⌘K palette overlay + full search view)

```
PALETTE (header overlay, portaled, z=modal):              FULL SEARCH VIEW:
┌─ ⌘K ───────────────────────────────────┐               │ Search: "sso login"  [Semantic ◐]        │
│ > sso login                             │               │ Filters: type[page▾] space[Ops▾] lang[de▾]│
│ ── Jump to ───────────────────────────  │               │ ┌────────────────────────────────────────┐│
│  📄 SSO support (Roadmap)               │               │ │ 📄 SSO support → #b9 "…Login schlägt…"  ││ ← jump-to-BLOCK
│  📄 Login runbook                       │               │ │ #ENG-412 Login fails on SSO  · open     ││   (sketch 07)
│ ── Actions ───────────────────────────  │               │ │ ◷ ci/run/991 · sso e2e · failed         ││
│  + Create page "sso login"              │               │ └────────────────────────────────────────┘│
└─────────────────────────────────────────┘               │ (cross-type, permission-PRE-filtered, §5.7)│
EMPTY:    · Type to search across pages, databases, and the whole platform.
NO-RESULT:· No results for "sso login".  [Create a page]   (and: hidden results never leak as counts)
LOADING:  · ▒ result rows  (sub-100ms keyboard response budget, §8b.6)
ERROR:    · ⚠ Search is temporarily unavailable.   (degraded surface fails STATIC — §8b.6)
```
- **Permission-pre-filtered** via `list_objects` (you only find what you may see, §5.7); **multilingual**
  (German query stems German body, `search-and-indexing.md` §4.7); **semantic toggle** for vector/RAG.

---

## S10 — Agent affordances + HITL approval card (woven; the trust surface, §6)

```
"Ask agent" entry (page header ⋯ menu):  [◆ Ask agent ▾] → "Turn action items into issues"

APPROVAL CARD (in Chat + Inbox; can inline on the page) — the agent treatment (§5.4/§6.3):
┌─ ◆ TriageAgent proposes 3 changes ──────────────────────────────────┐   ← agent badge, NOT sparkle/
│ on behalf of @alice · via "meeting-notes → issues" · est. cost €0.04 │     magic-wand (§8b.3)
│  1. Create issue "Fix SSO redirect"  → Eng project                   │   plan-then-apply: WHAT will
│  2. Create issue "Add login metrics" → Eng project                   │   change, on WHICH artifacts,
│  3. Edit this page: link the 2 issues back        ⚠ gated            │   under WHOSE authority (§6.2)
│ ──────────────────────────────────────────────────────────────────── │
│            [Approve]   [Edit…]   [Reject]                             │   Edit = amend before apply
└──────────────────────────────────────────────────────────────────────┘   (control of content, §6.3)
PENDING:  · ◆ Agent is reviewing this page…              (agent-pending state, §5.10)
DENIED:   · ◆ The agent couldn't create issue 2 — no permission.  (ordinary outcome, no fallback, AG-5)
DECIDED:  · ✓ Approved by @alice 1m ago · 2 issues created · page updated [view audit]   (§6.4)
```
- Surfaces in **Chat (primary HITL surface) + Inbox** so a gate is never missed (§5.8); persists across
  days (durable workflow, never silently lost, §6.3). Identical UX for mock + real agents (§6 payoff).

---

## S11 — Export (modal)

```
┌─ Export "Ops" space ───────────────────────────────────────────[Esc]┐
│ Format:  ● Markdown  ○ Lossless JSON  ○ PDF   (DBs also: ○ CSV)      │   lossless JSON = portability
│ Scope:   ● This space  ○ This page  ○ Whole workspace               │   (Art. 20) + DSAR spine
│          ○ Everything about a subject…  (DSAR export)               │   (sketch 06)
└──────────────────────────────────────────────────────────[Export]──┘
LOADING: · Preparing export…  (large export → progress, not a frozen modal)
ERROR:   · ⚠ Export failed partway. [Retry]  (resumable; no partial silent success)
```

---

## S12 — Mobile read + light-edit (responsive, §8b.4)

```
┌──────────────────────────┐   - Rail + secondary-nav collapse to a TOGGLED DRAWER (backdrop +
│ ☰  Incident: API 5xx  ⋯  │     Esc + route-change auto-close, §8b.4); a full-width panel COLLAPSES
│ ─────────────────────────│     the other column (width:100% is not a takeover, §8b.4).
│ # Incident: API 5xx spike│   - Row actions / hover affordances are surfaced by default (hover is
│ Severity **high**. @alice│     not touch-reachable, §8b.4).
│ ◷ ci/run/991 🔴 (embed)  │   - Read-anywhere + optimistic light-edit-online; full offline co-edit
│ [+ block]                │     is the CRDT-promotion follow-on (sketch 01).
│ ─────────────────────────│   - Popovers (slash/mention) flip + max-height so they don't render
│ ⌨ /  @  done             │     off the bottom of the screen, tested against the real anchor (§8b.4).
└──────────────────────────┘
```

---

## State-pattern checklist applied to every screen (design-language §5.10 / §8b.6)

| State | Rule applied here |
|---|---|
| **Empty** | onboarding-forward; explains + offers the create/next action (S1/S2/S3/S8). |
| **Loading** | skeletons that **match the final layout**; never a blank-page spinner; suppress flash under ~1s (§8b.6). |
| **Error** | system-blamed in one quiet line + a path (Retry); optimistic state preserved; a degraded surface fails **static** ("temporarily unavailable"). |
| **Permission-denied** | graceful "no access" / hidden-as-count; never a leaked title (sketch 04; §5.3). |
| **Erased/tombstoned** | redacted placeholder / tombstone; never the personal data; no dangling crash (sketch 05/06). |
| **Agent-pending** | "agent is working / awaiting your approval" (§5.10/§6). |

## Day-one UX primitives checklist (design-language §8b)

- **§8b.1 overlays**: slash-menu, mention/ref autocomplete, drag-handle menu, sharing/export/template
  modals, row peek, unfurl hovercards, confirm dialogs — all portal-to-root, one z-index scale, shared
  focus/dismiss/ARIA. Caret-anchored popovers flip above near the viewport bottom (tested against the
  real caret).
- **§8b.2 one editor render path**: read+edit share `parseInline`; `render(parse(md)) === md` is the
  corpus gate; controlled `contenteditable`; caret = md offset; structured nodes are atomic
  placeholders; serializer/offset/DOM-surgery ship + unit-test standalone first.
- **§8b.3 measured tokens**: status = glyph+label+colour (never colour alone); no saturated status
  fills; agents look like agents (◆ badge, no sparkle/magic-wand, no emoji-as-UI); focus token ≠ identity
  token; no inline colour on interactive elements.
- **§8b.4 layout-containment**: shell `100vh`/`overflow:hidden`, each region `min-height:0` scroller;
  `width:100%` collapses the other column on mobile; hover actions surfaced on touch; popovers flip.
- **§8b.5 humanised strings**: "edited 2m ago", unfurl chips, "approved by @alice" — humanised at the
  source (Notif/Refs), never a frontend string map; no raw ids / machine strings / unrendered markdown.

## Cross-references
- design-language §5 (shared components), §5.10 (states), §6 (agent UX), §7.4 (Knowledge catalogue),
  §8b (day-one primitives). sketches 01–07. `information-architecture.md`, `user-flows.md`.
