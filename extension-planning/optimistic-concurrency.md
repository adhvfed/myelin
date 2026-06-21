# EXT-3 — Conflict-Surfacing UX Contract for Optimistic / Concurrent Writes

> Extension flagged in [`README.md`](./README.md). A **genuine delta** — the concurrency *engine* is
> covered; the client-facing version/conflict contract is not.

## The UX goal that requires it

Two lovability promises depend on conflict being *surfaced legibly*, never silent: (1) "optimistic
updates, honest rollback" (P2 / `external-insights/05` §4) — when an optimistic write is rejected
server-side, the user must see an honest rollback, not a silently-lost edit; (2) concurrent edits to the
**same** issue field or doc block must surface a conflict the user can resolve, not an
last-write-wins overwrite (the completeness-critic's "conflict surfacing", README §9). This is the
unglamorous state Phase 6 must depict (R-21) and the rubric scores under D8 / the switch test.

## What the extension is (summary)

A **version-token + conflict-payload contract** on the subsystem write APIs that the client renders: each
mutable artifact/field carries a **version token** (ETag-class / CAS version); a write includes the token;
on mismatch the API returns a typed **conflict payload** (your-value, their-value, base-value, actor) the
client uses to render the rollback or the conflict-resolution UI — rather than a bare 409. For
collaboratively-edited surfaces this is the CRDT's job; for the single-author-at-a-time and field-level
cases (issue fields, issue description), this CAS-version contract is the lightweight delta.

## Which architecture doc it touches (and what's already covered)

- **`05-refined/00-platform-substrate.md` / `00-reconciliation-decisions.md`** — the **CAS-floor → CRDT**
  path (KN-1) is already designed for collaborative edit; per-aggregate ordering on the bus exists.
  **Delta:** the *client-facing* version-token + typed conflict-payload contract for the non-CRDT,
  field-level optimistic cases (issues) so the client can render honest rollback + conflict surfacing.
- **Subsystem write APIs (issue tracker, knowledge non-collab fields)** — the delta lives here: emit the
  version token and the structured conflict payload.
- **Editor / views components (R-10)** — consume the contract to render the conflict/rollback state.

## Rough size / risk

**Size: S–M.** **Risk: M** — the CRDT path already handles the hardest case (collaborative doc editing);
this is the *lighter* CAS-version contract for field-level optimistic writes, which is well-trodden
(ETag/If-Match semantics). Risk is mostly in making the conflict payload rich enough for a good
resolution UI without leaking values the viewer can't see.

## Implementation-task framing

"Add a version-token + typed conflict-payload contract to the issue/field write APIs (CAS-version, not
CRDT) so the client renders honest optimistic rollback and legible concurrent-edit conflict surfacing —
reusing the existing CAS-floor and the CRDT path for collaborative surfaces."
