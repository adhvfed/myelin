# Sketch 06 — Drag-to-reorder ranking at scale (TE-19)

> Exploration note. Weighs Phase-2 §11 Q7 / deep-dive §6.4: stable fractional ranking for drag-to-prioritise
> backlogs, with the **concurrent-reorder conflict story (humans AND agents reordering)**. Leans; commit in
> `00-findings.md`. Co-read Knowledge 01 §2.1/§4.1 — it uses the *same* fractional `order_key` family for block
> order and row order, so we should align.

## The requirement

A backlog (S6), a board column, a cycle plan — all support drag-to-reorder. Reordering must:
- be **O(1) per move** (move one item between two neighbours → one write, not renumber the list);
- be **stable** (a rank, once assigned, doesn't change unless the item moves);
- survive **concurrent reorders** by multiple humans and agents without silent clobbering (deep-dive §6.4 names
  this the hard part: "concurrent reorders by multiple users or agents producing conflicts");
- not exhaust precision (the known fractional-indexing failure mode: keys between two adjacent keys eventually
  run out of digits → rebalance).

## Prior art

| Technique | Source | Note |
|---|---|---|
| **LexoRank** | Atlassian (Jira's ranking system) | base-N string keys between neighbours; the production reference for issue-tracker ranking specifically |
| **Fractional indexing** | Implementations after Figma's "LexoRank"-style ordering writeup; the general technique | a key is a string strictly between its neighbours; insert = pick a midpoint string |
| **Interleaving hazard + jitter** | Figma / Evan Wallace's fractional-indexing notes; the "two clients insert at the same gap" problem | append randomness/jitter to reduce concurrent-insert collisions |
| **CRDT sequence (the real fix for concurrency)** | RGA; Yjs/Yrs list; **Fugue** (fixes interleaving) | the principled concurrent-order answer; Knowledge's eventual engine (01 §1.3) |

## Candidate A — LexoRank / fractional string keys + server arbitration (the floor)

`rank text` column; a move computes a string strictly between the new neighbours (`"hzz" < new < "i00"`); the
write is a single-row update. Concurrency is **server-arbitrated**: the move is a conditional write (CAS on the
item's current rank or a version token); two concurrent moves to the *same gap* → one wins, the loser is told
the current order and re-bases (re-drops) — **no silent clobber** (the CAS floor philosophy, KN-1/EI-04 §2).
Rebalancing: when a gap exhausts precision, a background job re-spreads that local region's keys (rare; bounded).

- **For:** O(1) moves, one column, indexed (`issue_board (… , rank)` — sketch 03). It is the **exact same
  `order_key`/fractional family Knowledge committed** (01 §2.1 "LexoRank-style… the same family Issues uses for
  drag-rank, TE-19") — primitive parity for free.
- **For:** server-arbitrated CAS gives the no-silent-overwrite guarantee humans+agents need without the full
  CRDT machinery. An agent re-ranking 50 issues and a human dragging one race → CAS resolves; the loser re-bases
  against fresh state (optimistic-update-with-honest-rollback, design-language §8b.6).
- **For:** matches the platform's optimistic-UI default (design-language P2): the drag updates the client
  optimistically; the server confirms or returns the authoritative order to reconcile.
- **Against:** under *heavy* concurrent reordering of the *same region* (an agent bulk-reorder + several humans),
  the re-base churn and the rebalance frequency rise — the known fractional-indexing stress case (deep-dive
  §6.4). Bounded by jitter on insert + region-local rebalance, but not *merged* (a true concurrent reorder is a
  conflict resolved by re-base, not a blend). This is acceptable as a **named floor**.

## Candidate B — A move-CRDT for ordering (the principled concurrency answer)

Model the backlog order as a CRDT sequence (RGA/Yrs list with Fugue interleaving fix) so concurrent moves
*merge* deterministically without server arbitration.

- **For:** true concurrent-reorder correctness — the deep-dive §6.4 "CRDT-ish" option; the same engine Knowledge
  lands for block order (01 §1.3).
- **Against:** heavyweight for a backlog (the CRDT shines for *character-level* concurrent text; backlog
  reordering is coarse-grained and low-frequency-per-item). It imports Knowledge's whole collab transport for a
  problem the CAS floor handles. **Premature** by the doctrine ladder (KN-1: CAS floor first, CRDT on the *first
  true concurrent-edit conflict*). For ranking specifically, the "first true conflict" bar is rarely hit (two
  people dragging the *same* item to *different* spots in the *same* second).
- **Lean:** the **named follow-on**, triggered if concurrent-reorder conflict is *measured* to hurt (the R5
  promotion-trigger discipline) — and at that point we reuse Knowledge's Yrs list type rather than build our own.

## Candidate C — Integer positions, renumber on move
`position int`; move = renumber affected rows.
- **Against:** O(n) writes per move (renumber the tail); concurrent moves stomp each other badly. The classic
  antipattern. Rejected.

## The agent angle (specific to our "humans AND agents reorder" requirement)

Agents reorder via the **same permissioned `ToolDef`** (`issue.reorder` / a `rank` write through `EffectApi`,
ADR-08) a human uses — no privileged back-channel (deep-dive §11 design law). So an agent's reorder goes through
the *same* CAS arbitration; an agent that loses the CAS gets an ordinary `Denied`/stale tool result and re-plans
(AG-5). This is why server-arbitrated CAS (not client-trust) is the right floor: it makes human and agent
reorders **uniformly safe** through one mechanism.

## Leaning

**Candidate A — LexoRank/fractional string `rank` key + server-arbitrated CAS + jittered inserts + region-local
background rebalance**, as the floor. Aligned with Knowledge's `order_key` family (primitive parity). The
**move-CRDT (Candidate B, reusing Yrs) is the named follow-on**, promoted only on *measured* concurrent-reorder
pain. Agents reorder through the same permissioned tool + the same CAS arbitration as humans — one safe path.

## Hands forward

- The exact rank string encoding (base, jitter) — align with Knowledge's `order_key` in architecture.
- The rebalance trigger + region scope — architecture.
- PROVE-IT: concurrent-reorder drill (N humans + an agent re-ranking the same backlog region → zero silent
  clobber, bounded re-base, order converges) — findings §drills.
