# Surface group: The CLI as a first-class peer surface (§7.7)

> Phase 5 surface map · group **X** · maps [`design-language §7.7`](../../planning/02-holistic-architecture/design-language.md)
> against the [§2 template](./README.md#2-the-per-surface-map-template). Pointer map; PROVEN / HOUSE
> STYLE tagged; date 2026-06-20. Cross-cutting obligations ([README §3](./README.md#3)) inherited where
> they translate to a textual rendering. **The CLI is NOT a separate IA — it is the same R-06 §2 tree
> rendered textually, sharing the §5 address scheme** (R-06 §8; R-07 §5 — the CLI inherits the eight
> invariants, with textual density as its J2 tier).

---

## X-1 — The CLI (one design surface, two renderings)
1. **Audience + jobs:** Engineers (P1–P5), CLI-first (P1–P3). Many engineer jobs **complete in the
   terminal, not the web UI** — the design must finish the job in **either rendering of the one surface**
   (R-03 §1 note). Named jobs with a CLI path: **E1** (`myelin pr view`), **E3** (`myelin run view`,
   log tail — F-ENG-1 entry is "a red Checks badge **or** a `myelin run watch` alert"), **E7** (palette
   verbs ↔ CLI verbs), E8 (config validate). Verb set (design-language §7.7):
   `myelin repo|pr|run|issue|view|kb|chat … <verb>`.
2. **IA + shell:** the **same §2 tree, textual** — `myelin <subsystem> <verb>` maps to the same nodes
   (R-06 §8). No separate tree to learn.
3. **Shared components (textual analogues):** the **`ArtifactRef` is the shared handle** — the chip you
   see in the UI and the `myelin://…` handle you paste in the CLI are the **same identity** (R-09 §7.1,
   P1/P6); CLI output, error states, and reference rendering follow the **same vocabulary** as the UI.
4. **Density:** the CLI's own **textual density tier** (R-07 J2 — textual density is its earned tier);
   it inherits the eight invariants (R-07 §5, the CLI shares the tree).
5. **Agent:** `myelin agent review request` (design-language §7.7); the **`--dry-run` parity** with
   plan-then-apply (R-14 — the CLI shows the proposed effects before applying, the same plan-then-apply
   contract in text); agent attribution + `correlation_id` legible in CLI output (R-15 §1).
6. **Sovereignty:** the residency tag travels on the `myelin://` ref (cross-cell refs carry their home
   cell, R-06 §5.4); no-access resolves to a clean "no access" line, **never a leaked title** (R-09).
7. **State set (R-21 §2g CLI row):** the unglamorous states have **textual spellings** — no-access
   line, tombstone line ("artifact no longer available"), error blames the system + a path/correlation
   id (§8b.5 humanised, not a raw stack), stale/reconnecting for `watch`/`log tail` (resume on replay).
8. **A11y/i18n:** the CLI is **inherently keyboard-operable** (no G1 visual gate, but output must be
   screen-reader-friendly plain text — not ASCII-art-only status). **G2:** **humanised + localised
   strings** (no raw machine ids/enum keys, R-18 §6 — `merge_request merged` is the canonical "feels
   unfinished" tell the CLI must avoid too); status as **glyph/word + label**, never colour-alone in the
   terminal (R-02 R-COLOUR-1 — terminals strip colour; a `✓ passed` / `✗ failed` word is mandatory).
9. **Device / form-factor:** the CLI is the **terminal form-factor** — it *is* the answer to "engineer
   on the keyboard" and is unaffected by the touch/mobile §8b.4 bug set (those are web-shell concerns).
   Its "responsive" concern is **narrow-terminal reflow** (tables degrade to key:value lists at small
   `$COLUMNS`); it must not assume a fixed wide terminal (the textual analogue of MOB-5 "name
   fixed-width assumptions").
10. **Wedge / delight:** the CLI carries the wedge in text — **W4** (`myelin run view` → failing step
    → `myelin pr diff` at the line; the failing line one command away) and **W6** (the agent
    `correlation_id` chain readable in `myelin agent` output). The delight is **speed + parity**: the
    job finishes identically in web or terminal (R-20 startup arc — "code in-region" via clone/CLI).
    No motion (textual); progress via plain streaming, not a blocking spinner.
11. **DoD + switch test:** **every primary capability has a CLI verb** (design-language §7.7 —
    consistent `myelin <subsystem> <verb>` across subsystems); the `ArtifactRef` (`myelin://…`) is the
    same handle as the UI chip; agent actions go through `--dry-run`/plan-then-apply parity; errors and
    refs are humanised, not raw. **Switch test:** a CLI-first engineer (P1–P3) completes red-to-green
    (F-ENG-1) entirely in the terminal without dropping to the web UI for a step the web has but the CLI
    lacks — the CLI is a **peer surface, not an afterthought**.

---

**Group invariants reminder:** the CLI is the proof that the eight invariants (R-07 §3) hold **across
renderings**, not just across web subsystems — one address scheme, one vocabulary, one agent contract,
one humanisation, expressed in text. Its "tokens" are textual, but it is in scope for the consistency
the design language enforces (design-language §7.7). The CLI was **never an orphan** (R-21 §2g CLI row;
critic §3) — it is the §2 tree's seventh rendering.
