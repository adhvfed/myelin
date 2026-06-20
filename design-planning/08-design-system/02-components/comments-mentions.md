# Component spec — Comments / threads / mentions / reactions (one conversation primitive)

> **Phase 8b · `02-components/` · Tier-2 shared component.** Direction = finalist **A "Instrument"**
> (consumes [`../01-tokens/tokens.css`](../01-tokens/tokens.css)). **File date: 2026-06-20.**
> Stack: TS + React (function components) + **React Aria Components**. **Not committed.**
>
> **Implements:** design-language **§5.5** (one comment/thread/mention/reaction primitive across PR review,
> issue discussion, doc comments, chat) + **§5.10** (states). Research it renders:
> [`shared-patterns.md`](../../04-research/interaction/shared-patterns.md) (R-10 §1 atomic taxonomy, §3
> editor reuse) · [`reference-unfurl.md`](../../04-research/interaction/reference-unfurl.md) (R-09 — mentions
> render as chips) · [`legibility-and-hitl.md`](../../04-research/agent-ux/legibility-and-hitl.md) (R-14 §1 —
> the agent treatment on `@agent` chips) · [`state-craft.md`](../../04-research/craft/state-craft.md) (R-21).
>
> **Tagging:** **PROVEN** = a cited standard, or an existing contract this spec *surfaces* (ADR-05 shared
> content model; the `mention(Principal)` / `artifact_ref` structured nodes; ADR-08 agent-mention-as-trigger;
> `myelin-refs` anchoring §3.5). **HOUSE STYLE** = our synthesis. `[DEFERRED-UNTIL-USERS]` flagged inline.
>
> **Reuse (load-bearing):** the comment **body is rendered by the
> [`<BlockEditor>`](./block-editor.md) render path** (one render path, ADR-05/§8b.2 — there is no separate
> comment renderer); `@mention`s and `#artifact`s render as **[`<ReferenceChip>`](./reference-chip-and-unfurl.md)**;
> mentions of people route to the [notifications inbox](./notifications-inbox.md), mentions of **agents** are a
> trigger into the agent fabric and can surface an [`<AgentHitlCard>`](./agent-hitl-card.md). The PR-review
> verdict is the same "approve a proposed effect" shape as the HITL card (P1).

---

## 1. Name + purpose

**`<Thread>` / `<Comment>` / `<MentionNode>` / `<ReactionBar>`** — the **one** conversation primitive so
"discuss an artifact" feels identical everywhere (P1). One comment/thread model over the shared content model
(ADR-05): rich text, `@mentions` (people **and** agents, as chips), `#artifact` references, code blocks,
reactions, **review batching** (start review → batch inline comments → submit verdict), and **anchored
comments** (a comment pinned to a diff line, a doc block, or a sub-artifact). *(PROVEN model; interaction
HOUSE STYLE.)*

---

## 2. Anatomy

### 2.1 Comment
```
┌────────────────────────────────────────────────────────────┐
│ {identity badge} {author name} {agent treatment?}   {when}  │  ← header (one identity treatment, §5.11)
│ {comment body — rendered via the BlockEditor render path}   │  ← rich text + @chips + #chips + code
│ {ReactionBar: 👍·❤·… counts}        {reply · ··· actions}   │  ← reactions + per-comment actions (edit/delete/quote/copy-ref)
└────────────────────────────────────────────────────────────┘
```
- **Header** — one **identity badge** per `Principal` (human/agent/service); an **agent author** carries the
  four-channel agent treatment (label + plain mark + `--agent` + attribution) — never disguised as a human.
- **Body** — the same content AST + markdown-subset-string render path as the editor (no divergent renderer);
  `mention`/`artifact_ref`/`embed` are **structured nodes**, never collapsed into the string (so
  reference-extraction stays reliable — §8b.2 rule 3).
- **ReactionBar** — emoji *reactions* are allowed (they are user content, not UI chrome — distinct from the
  §8b.3 "no emoji as the agent marker" rule, which governs *agent identity*, not reactions); each shows a
  count + who-reacted on hover.

### 2.2 Thread
A header comment + replies (indented or flat per surface) + a composer. **Anchored threads** carry an
anchor chip ("on `src/auth.rs#L42`" / "on the *Rollback* block") that is itself a `<ReferenceChip>` and
**relocates/orphans honestly** when the anchor moves (§5).

### 2.3 Mention / ref node (inline, in any body)
`@person` · `@agent` · `#artifact` — first-class inline **structured nodes** rendered as `<ReferenceChip>`s
(R-09). `@agent` is a **trigger into the agent fabric** (ADR-08) — mentioning it *notifies*, never
auto-spawns a costed run (explicit-first, CHAT-1); an explicit ask can surface a plan card.

### 2.4 Review batching
A **Start review** affordance opens a batch; inline comments accumulate (pending, visible only to the author)
until **Submit review** posts them with a single **verdict** (Approve / Request changes / Comment). The
verdict on a consequential PR routes through the HITL/Confirm shape (P1 — a human verdict and an agent
proposal are the same approve-a-proposed-effect shape).

---

## 3. Interaction spec

- **Compose** — the composer is the `<BlockEditor>` (chat-/comment-tuned: small, mostly-immutable
  concurrency); `@` opens the mention picker, `#` the artifact picker, `/` the slash menu (shared molecules —
  R-10 §3.4). Send is **optimistic** (the comment appears instantly, settles on ack, honest-rollback on
  reject with the typed text preserved — OPT-1/OPT-3).
- **Mention picker** — a type-to-filter combobox of principals + artifacts, **permission-pre-filtered**
  (`list_objects`, ADR-03 — you can't @ what you can't see; no leak). People and agents are visually
  distinguished (the agent treatment in the option row).
- **React** — toggle a reaction (optimistic); reversible, no confirm.
- **Anchored comment** — select a diff line / block → "comment here"; the anchor is a content-anchored
  `ArtifactRef#sub` (BLAKE3 + 3-way match, `myelin-refs` §3.5) that survives/relocates (§5).
- **Edit / delete** — edit keeps history; delete tombstones (the comment degrades to "comment removed", never
  a dangling reply tree).
- **Resolve thread** — a thread can be resolved/collapsed (review + doc-comment surfaces).

---

## 4. Variants + parameterization variant flags

- **Surface variant (prop, not a direction flag):** `review` (batched, verdict, indented) · `issue-discussion`
  (flat, single-author edit) · `doc-comment` (anchored, resolvable, margin-pinned) · `chat` (flat timeline,
  presence). **One component, one model**, surface-tuned by props + the per-subsystem concurrency engine.
- **`density` flag (`comfortable`↔`compact`)** — comment padding / reply indent / reaction size via `--space-*`;
  compact is A's default.
- **`agentPresence` flag (`ambient`↔`foregrounded`)** — sets whether an agent *comment/review* is collapsed/
  threaded (ambient) or shown inline as a present participant (foregrounded). The agent treatment is constant.
- **`tone` flag (`utilitarian`↔`warm`↔`sober`)** — touches only the empty-state copy voice (§5), not chrome.
- **NOT affected:** `nav`, `surfaceUnification`. **No `switch(direction)`.**

---

## 5. ALL states

| State | Render |
|---|---|
| **Empty** | onboarding-forward, per surface: "Start the discussion" / "No comments yet — `@` to mention, `/` for blocks"; doc-comment empty is calm, not a nag. Voice per `tone`. |
| **Loading** | comment-row skeletons matching the final layout (avatar circle + text bars), `aria-busy` + polite live region; never a blank spinner; suppress flash <~1s. |
| **Saving / pending** | optimistic comment with a quiet "sending…" affordance; never blocks composing the next. |
| **Error (send failed)** | one quiet **system-blaming** line + retry; **the typed text is never lost** (local buffer). A *permission* failure is the permission row below, not "Error". |
| **Permission-denied** | read-only render with "you can view but not comment" cue; the composer absent (not greyed). A whole-thread no-access → the no-access card (the body never leaks). |
| **Erased / tombstoned** | an `@person` whose principal was erased → **"[erased user]"** in the header + chip (R-21 §1.5); a `#artifact` to an erased target → the tombstone chip inline; a deleted comment → "comment removed", thread integrity preserved. |
| **Agent-pending** | an `@agent` mention awaiting / working renders the agent-pending treatment; an agent *proposal* in a thread is the `<AgentHitlCard>` (Approve/Edit/Reject). |
| **Degraded** | a comment whose chip can't refresh shows the last-known + "can't refresh" dot (per-chip), never blanking the thread (fails static). |
| **Stale / reconnecting** | a live thread (chat, collab doc comments) that drops realtime keeps last-known content + a quiet "Reconnecting…"; resumes losslessly (firehose resume); never blanks. |
| **Conflict** | concurrent edits to the *same comment* surface presence + the merge (CRDT for rich text) or "this changed while you were editing — keep yours / take theirs" (CAS); **never a silent overwrite** (R-21 §1.10). |
| **Moved / outdated anchor** | an anchored comment whose target moved → "moved" pill; rebased diff-line comment → relocates ("moved") / detaches to "outdated — was on former line N" and lifts to conversation level — **never silently re-anchors to a wrong line** (R-09 §5.9). |
| **Cross-cell** | a `#artifact` mention to another residency cell → normal chip + residency tag, else no-access (R-09 §5.8). |

---

## 6. Keyboard + ARIA model

- **Thread = a list of articles** — each comment a region with an accessible name (author + when); React Aria
  patterns for the composer (**`TextField`**-backed contenteditable via the editor), the mention picker
  (**`ComboBox`** — roving via `aria-activedescendant`, the APG combobox pattern), reactions
  (**`ToggleButton`** group), and per-comment actions (**`MenuTrigger`/`Menu`**).
- **Keyboard-first** — `r` reply, `e` edit own, reaction shortcuts; `Tab` order is logical; the mention picker
  is keyboard-operable and dismissable with `Esc` (no trap).
- **Visible focus** via `--focus-ring` every theme; the agent treatment is colour-blind-safe (label + mark).
- **Live-region announcement** of *new* replies in a thread the user is in (polite, not every background
  message — no spam); mention-of-me is a notification (inbox), not a focus-steal.
- **Reflow / RTL** — replies, reactions, anchors reflow at 200%/320px; logical properties mirror RTL (reply
  indent uses inline-start).
- **Anchored comments** survive content-anchor relocation legibly (the anchor chip carries the state).

---

## 7. Semantic tokens consumed

| Purpose | Token(s) |
|---|---|
| Comment surface / divider | `--surface` / `--surface-raised`, `--border` (hairline reply dividers) |
| Author / meta / when | `--text-primary`, `--text-muted`, `--text-subtle` |
| **Agent author** | **`--agent`** / `--on-agent` / `--agent-subtle` / `--c-agent-mark` (never colour-alone, never a status colour) |
| Mention / ref chips | the `<ReferenceChip>` tokens (`--c-chip-*`) |
| Reaction pill (active) | `--accent-weak` bg + `--text-primary`; count `--text-muted` |
| Review verdict (approve/request-changes/comment) | `--success` / `--danger` / `--text-muted` — **always glyph + label** |
| Pending / sending | `--text-subtle` |
| Focus | `--focus-ring` |
| Mention-picker overlay | `--shadow-popover`, `--z-popover` |

Binds only to semantics / the chip handles.

---

## 8. Motion (token-based, reduced-motion first-class)

- **New comment enters** — `--dur-base` `--ease-enter`, subtle, no scroll-jump if the user is reading above.
- **Reaction toggle** — `--dur-micro`.
- **Optimistic send settle / rollback** — settle `--dur-micro`; rollback reverses so failure looks different.
- **Thread expand/collapse** — `--dur-fast` `--ease-standard`.
- **No bounce/sparkle.** **`prefers-reduced-motion`** → 0; state flips + announces.

---

## 9. Usage do / don't

**Do**
- Render the body through the **one editor render path** (no separate comment renderer — that is how
  `render(parse(md))!==md` divergence bugs are born).
- Render mentions/refs as **the same `<ReferenceChip>`** seen everywhere (D4 coherence).
- Keep `@person` vs `@agent` distinguished (the agent treatment); make `@agent` an explicit trigger, never an
  auto-spawn.
- Batch review comments behind a single verdict where the surface is review.
- Preserve thread integrity through deletes/erasure (tombstone the node, keep the tree).

**Don't**
- Don't ship a fork-per-subsystem comment model — one model, surface-tuned by props (the trap §5.5 exists to
  kill).
- Don't leak a `@person`/`#artifact` the viewer can't see in the mention picker (pre-filter, never
  post-filter).
- Don't use emoji/sparkle for *agent identity* (reactions-as-content are fine; agent-marker-as-emoji is not).
- Don't silently re-anchor a rebased diff-line comment to the wrong line.
- Don't lose typed text on a send error.

---

## 10. Honesty — PROVEN vs HOUSE STYLE vs deferred

- **PROVEN:** the one-content-model / one-render-path (ADR-05 / §8b.2); structured mention/ref nodes never
  collapsed; permission-pre-filtered mention picker (ADR-03); agent-mention-as-trigger + explicit-first
  (ADR-08 / CHAT-1); content-anchored comment relocation (`myelin-refs` §3.5); firehose-lossless reconnect;
  the agent treatment legibility duty (1.4.1 / AI-Act).
- **HOUSE STYLE:** review-batching choreography; the per-surface variant tuning; anchored-comment chip
  behaviour; the reaction interaction.
- **`[DEFERRED-UNTIL-USERS]`:** the **dual-audience vocabulary** caveat — whether engineer-review language and
  PM-discussion language read as "the same primitive" or feel forked is a comprehension hypothesis (shares
  R-16's per-lens question). Whether the agent-author treatment reads as "an agent, not a human" in a busy
  thread (R-14 §10). Method: per-segment RITE on F-ENG-1 (review) + F-PM-1 (discussion).

*End. Component spec HOUSE STYLE over the PROVEN ADR-05 content model + §5.5 + the structured mention/ref node
contract; body renders via `<BlockEditor>`, mentions via `<ReferenceChip>`, agent proposals via
`<AgentHitlCard>`. Consumes the finalist-A token set. Not committed.*
