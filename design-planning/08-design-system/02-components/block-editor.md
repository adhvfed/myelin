# Component spec — Block / rich-text editor (one render path, one AST)

> **Phase 8b · `02-components/` · Tier-2 shared component.** Direction = finalist **A "Instrument"**
> (consumes [`../01-tokens/tokens.css`](../01-tokens/tokens.css)). **File date: 2026-06-20.**
> Stack: TS + React (function components) + **React Aria Components**; the parse/serialize logic is
> **WASM-Rust shared client↔server** (`myelin-content` AST + sanitiser — 00-plan §1.6). **Not committed.**
>
> **Implements:** design-language **§5.9** (the rich-text/block editor) + **§8b.2** (the one-render-path law)
> + **§5.10** (states). Research it renders:
> [`shared-patterns.md`](../../04-research/interaction/shared-patterns.md) (R-10 §3 — the §8b.2 mandates as
> design law, slash menu, mention/ref nodes, the contenteditable pitfall) ·
> [`state-craft.md`](../../04-research/craft/state-craft.md) (R-21 §2e/§2f) ·
> [`reference-unfurl.md`](../../04-research/interaction/reference-unfurl.md) (R-09 — mention/ref nodes are chips).
>
> **Tagging:** **PROVEN** = a cited standard / hard problem (ProseMirror-class controlled-contenteditable
> lesson; the round-trip CI gate; ADR-05) or an existing contract surfaced. **HOUSE STYLE** = synthesis.
> `[DEFERRED-UNTIL-USERS]` = the IME/paste real-input risk (flagged hard below).
>
> **Reuse:** `@mention` / `#artifact` / `/embed` render as **[`<ReferenceChip>`](./reference-chip-and-unfurl.md)**;
> the slash-menu-item molecule shares shape with the palette row + dropdown item; `/database` embeds the
> **[`<Views>`](./views.md)** organism inline; this editor's render path also renders comment bodies
> ([`comments-mentions.md`](./comments-mentions.md)) and humanised notification strings
> ([`notifications-inbox.md`](./notifications-inbox.md)) — **one render path, four subsystems**.

---

## 1. Name + purpose

**`<BlockEditor>`** — ONE editor organism over the shared content model (ADR-05): the writing surface for
knowledge pages, issue descriptions/comments, PR descriptions, and chat composition — each with the **same
node taxonomy** (concurrency differs per subsystem). The user experiences **one writing surface**. Notion is
the North Star; **§8b.2 sharpens past it** with a binding correctness law (one render path). *(PROVEN model;
interaction HOUSE STYLE.)*

---

## 2. The §8b.2 one-render-path law (binding correctness, not aspiration — carried verbatim)

These are **CI-gateable correctness bars**, not preferences:

1. **ONE render path** — read and edit run the **same inline parser**. There is **no separate "viewer"** that
   diverges from the editor; the renderer for an issue comment and the editor for it are the same code path.
   *(HOUSE STYLE → PROVEN via the gate below.)*
2. **`render(parse(md)) === md` round-trip over a corpus is a HARD CI gate** *(PROVEN by the corpus)* — the
   correctness bar whatever concurrency engine Knowledge picks.
3. **Inline content stored as a markdown-subset STRING** (not an inline-range JSON model) *(HOUSE STYLE,
   structural reason)* — survives copy/paste, export, diff, reference-extraction; no server sanitisation pass;
   zero-migration through an editor rewrite. **Reconciliation with ADR-05: AST for block structure,
   markdown-subset string for inline runs**, with **`mention`/`artifact_ref`/`embed` kept as structured nodes
   — never collapsed into the string** (so reference-extraction, the wedge, stays reliable). *(PROVEN — §8b.2;
   the `mention(Principal)` node is frozen identical across Chat/Issues/Knowledge.)*
4. **Controlled `contenteditable`, NOT `<textarea>`** *(PROVEN — a textarea cannot show formatting as you
   type)*; **the caret is a char offset into the serialised markdown**, bridged to/from the DOM. The editor
   **owns its document model and reconciles the DOM, never reads from it** (the ProseMirror-class lesson:
   contenteditable ignores model state and Chrome/Firefox diverge on caret behaviour). **Browser variance
   (Enter / IME / paste) is the top Knowledge-P4 risk.**
5. **Editor primitives ship + unit-test STANDALONE before the integrated editor** *(HOUSE STYLE)* — the
   serializer, the offset model, and the DOM-surgery for Enter-splits-block / caret-after-split are
   independently tested. **"Enter just inserts a newline" is the #1 'not a real editor' tell.**

---

## 3. Anatomy

- **Document** — an AST of **blocks** (paragraph / heading / list / table / code / callout / image / embed),
  blocks nest. Inline runs within a block = the markdown-subset string + structured inline nodes.
- **Block** — a six-dot **drag handle** (also exposes delete / duplicate / convert) on hover **and** an
  explicit mobile affordance (hover-is-not-touch).
- **Caret / selection** — a char offset into the serialized markdown, reconciled to a DOM range.
- **Inline structured nodes** — `mention` (`@person`/`@agent`), `artifact_ref` (`#artifact`), `embed` — each a
  non-editable island rendered as a `<ReferenceChip>`; the caret passes **around** them and `Tab` exits.
- **Slash menu** — `/` opens a ranked, type-to-filter menu of insertable blocks **and** reference/embed nodes.

---

## 4. Interaction spec

- **Slash menu (`/`)** — a ranked, type-to-filter menu of blocks + reference/embed nodes (`/issue`,
  `/database` live board over an `ArtifactRef`) — the wedge reaches into the editor. **Anti-bloat rule
  (P4/P8):** a short frequency-ranked default set, depth behind search — **not a 60-item wall**.
- **Mention / ref nodes (`@` / `#`)** — first-class inline **structured nodes** rendered as chips;
  permission-pre-filtered picker (no leak); `@agent` is a **trigger into the agent fabric** (explicit-first,
  never auto-spawn); nodes are **never collapsed into the string** (rule 3).
- **Block ops** — Enter splits a block (the standalone-tested DOM-surgery, not a raw newline); Backspace at
  start merges; `Cmd-B`/`Cmd-I` toggle marks; block move via shortcut + drag handle; convert via the handle
  menu.
- **Embeds** — a knowledge page embeds a live issue board (the `<Views>` organism over an `ArtifactRef`); a
  runbook references a CI run — embeds are reference nodes rendered inline.
- **One editor, many concurrency models** — knowledge = full collaborative (CRDT/OT, TE-15); chat = small /
  mostly-immutable; issue descriptions = single-author-at-a-time. They **share the AST + editor component, not
  the engine** ("share the AST, not the editor engine", ADR-05).
- **Save** — optimistic; a quiet "saving…/saved"; never a blocking modal.

---

## 5. Variants + parameterization variant flags

- **Surface variant (prop):** `page` (full, collab) · `issue-description` (single-author) · `comment` (compact,
  chat/issue) · `chat-composer` (single-line-growing). One component, concurrency tuned per surface.
- **`density` flag** — block spacing / line-height / handle size via `--space-*`.
- **`tone` flag (`utilitarian`↔`warm`↔`sober`)** — **the flag with the most reach here:** the reading-surface
  type config (serif on reading headings under `surfaceUnification: distinct`/`warm`, measure ~66ch,
  `--lh-reading` 1.6) + the empty-state voice. Token-and-copy, not chrome. (A's default = `utilitarian`.)
- **`surfaceUnification` flag** — `distinct-per-surface` may apply serif-on-reading-headings + a reading
  measure; `one-skin` (A) keeps the UI sans. **Bounded** — the editor *chrome* (handles, slash menu, chips)
  is invariant either way.
- **NOT affected:** `nav`, `agentPresence`, `sovereigntyVisibility`. **No `switch(direction)`.**

---

## 6. ALL states

| State | Behaviour |
|---|---|
| **Empty** | a calm placeholder + slash-hint ("Type `/` for blocks, `@` to mention"); onboarding-forward. Voice per `tone`. |
| **Loading** | **block-skeleton matching final structure** (headings/paras as ghost bars), `aria-busy` + polite live region; never a blank spinner; suppress flash <~1s. |
| **Saving / pending** | optimistic; quiet "saving…/saved"; never a blocking modal. |
| **Error (save failed)** | one quiet **system-blaming** line + retry; **the typed content is NEVER lost** (local buffer). |
| **Permission-denied** | read-only render with a graceful "you can view but not edit" cue, **via the same render path** (no divergent viewer); or a whole-doc no-access card; never a silent swallow of edits. |
| **Erased / tombstoned ref** | a mention/ref node to an erased artifact renders the **tombstone chip inline** ("[erased user]" for an erased principal); the node degrades, the surrounding text survives. |
| **Agent-pending** | an agent drafting / suggesting renders the agent treatment; an agent edit-proposal surfaces the `<AgentHitlCard>`. |
| **Degraded** | a ref-node chip that can't refresh shows last-known + "can't refresh" dot (per-node); the document stays editable (fails static). |
| **Conflict (collab)** | concurrent edits surface **presence + the CRDT/OT merge**; genuine collisions shown legibly, **never silently dropped**; **never lose an in-progress edit to a background live update** (OPT-3). Deep collab model `[OPEN → P4]` (TE-15). |
| **Offline / reconnecting** | buffered locally; quiet "Reconnecting…" / "showing cached content"; re-syncs losslessly on reconnect. Full offline-editing model `[OPEN → P4]`. |

> The collab editor **owns** conflict + reconnect (R-21 §2h) — its defining stress states.

---

## 7. Keyboard + ARIA model (a named G1 "hard component")

- **Full keyboard operability** — all block ops reachable by keyboard (slash insert, Enter-split,
  Backspace-merge, block-move shortcut, mark toggles); **no trap** — the embedded non-editable chip islands
  must allow the **caret to pass and `Tab` to exit** (the PROVEN contenteditable-island caret pitfall).
- **The contenteditable root** carries `role="textbox"` `aria-multiline="true"` + an accessible name; the
  slash menu is a **`Menu`/`ListBox`** (React Aria), the mention picker a **`ComboBox`** (roving via
  `aria-activedescendant`, APG combobox), each dismissable with `Esc`, no trap.
- **Screen-reader correctness is a component contract** — AT announces **block type on entry**; the round-trip
  markdown **is the accessible text fallback**.
- **IME / composition events handled** (CJK + accented EU input) — part of the §8b.2 controlled-caret model and
  a **G2 (i18n) obligation**. **Paste** is normalised through the WASM sanitiser into the AST (paste-from-Word
  is the classic failure surface — see §11 risk).
- **Sanitisation + safe rendering** is a component responsibility inherited by all consumers (ADR-05, the
  shared WASM sanitiser — same logic client + server).
- **Visible focus** via `--focus-ring`; **RTL** via logical properties (handle on inline-start, mirrors free).

---

## 8. Semantic tokens consumed

| Purpose | Token(s) |
|---|---|
| Editor surface | `--surface`; code blocks `--surface-raised` |
| Body / headings / muted | `--text-primary`, `--text-muted`; placeholders `--text-subtle` |
| Marks / code | `--font-mono` + `--fs-code`; inline code bg `--surface-raised` |
| Reading config (tone/distinct) | `--lh-reading` (1.6), the reading measure; serif family where the flag is on |
| Mention / ref / embed nodes | `<ReferenceChip>` tokens (`--c-chip-*`) |
| **Agent** (agent draft/suggestion node) | **`--agent`** / `--agent-subtle` / `--c-agent-mark` |
| Block handle / drag affordance | `--text-subtle` idle, `--text-muted` hover |
| Slash menu / mention picker overlay | `--shadow-popover`, `--surface-overlay`, `--z-popover` |
| Selection / focus | selection `--accent-weak`; caret `--text-primary`; focus `--focus-ring` |
| Callout / diff (in code review desc) | `--info-subtle`/etc.; `--diff-*` for inline diffs |

Binds only to semantics / chip handles. Type scale + line-heights from the scale vars.

---

## 8b. Icons (canonical glyphs — registry names)

From the 42-icon library ([`../04-icons/ICONS-README.md`](../04-icons/ICONS-README.md) §2;
[USAGE-MAP](../04-icons/USAGE-MAP.md) §A).

- **Block drag handle / convert menu:** `kebab` (the six-dot handle / overflow mark).
- **Slash-menu insertable blocks:** `doc` · `database` (`/database` live board) · `file` · `link` (`/embed`) ·
  `issue` (`/issue`).
- **Inline mention / ref / embed nodes:** `<ReferenceChip>` type-icons; `@agent` → `agent`.
- **Disclosure (toggle blocks):** `chevron` (CSS-rotated).
- *Gap:* attachment/paperclip and a generic add `+` have no core glyph yet ([USAGE-MAP](../04-icons/USAGE-MAP.md) §C).

## 9. Motion (token-based, reduced-motion first-class)

- **Slash menu / mention picker open** — `--dur-fast` `--ease-enter`; appears, doesn't slide the paragraph.
- **Block insert / reorder** — `--dur-fast` `--ease-standard`.
- **Save settle** — `--dur-micro` quiet "saved" fade.
- **Live collab presence cursor** — `--dur-micro` ease-linear glide.
- **No bounce/sparkle.** **`prefers-reduced-motion`** → 0; inserts/reorders are instant; state announces.

---

## 10. Usage do / don't

**Do**
- Keep **one render path**; gate `render(parse(md)) === md` in CI; build + unit-test the serializer / offset
  model / Enter-split DOM-surgery **standalone first**.
- Keep `mention`/`artifact_ref`/`embed` as **structured nodes** (never collapsed into the string).
- Make Enter a real block-split, Backspace-at-start a real merge (not raw newlines).
- Keep the slash menu short + ranked; depth behind search.
- Normalise paste through the shared WASM sanitiser; handle IME composition events.

**Don't**
- Don't ship a fast read-only renderer that diverges from the editor (the "preview looks different from edit"
  bug — the §8b.2 trap).
- Don't store inline runs as an inline-range JSON model (breaks copy/paste/export/diff/reference-extraction).
- Don't collapse ref/mention nodes into the string (destroys the wedge).
- Don't trap the caret on a chip island (it must pass; `Tab` must exit).
- Don't let a save error lose typed content. Don't `switch(direction)`.

---

## 11. Honesty — PROVEN vs HOUSE STYLE vs deferred

- **PROVEN:** the one-render-path law + the round-trip CI gate (§8b.2); AST-for-blocks /
  markdown-string-for-inline / structured-nodes (ADR-05 reconciliation); the controlled-contenteditable / caret
  -as-offset / own-the-model-reconcile-the-DOM lesson (ProseMirror-class, cited); shared WASM sanitiser; the
  chip-island caret pitfall; full keyboard operability (G1).
- **HOUSE STYLE:** the slash-menu ranking + anti-bloat default set; the per-surface concurrency tuning; the
  block-handle interaction; the reading-config application of `tone`/`surfaceUnification`.
- **`[DEFERRED-UNTIL-USERS]` — the editor IME / paste risk (flagged hard):** the §8b.2 mandates are the right
  defence (a PROVEN *class* of bug), but **IME (CJK + accented EU input), paste-from-Word, and mobile keyboards
  are notoriously where contenteditable editors fail** — the standalone-test discipline + the round-trip gate
  are the *bet*, not a guarantee. Validation = AT + real-input testing (R-17's deferred AT plan); falsifier = a
  caret/Enter/IME class of bug the round-trip gate didn't catch. This is the component's single largest
  build-time risk; the human should expect editor IME/paste to need real-device QA before "done".

*End. Component spec HOUSE STYLE over the PROVEN §8b.2 one-render-path law + ADR-05 + the controlled-
contenteditable lesson; mention/ref nodes are `<ReferenceChip>`s, `/database` embeds `<Views>`, the render path
also serves comments + humanised notification strings. Consumes the finalist-A token set. Not committed.*
