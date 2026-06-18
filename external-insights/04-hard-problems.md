# The Hard Problems, Named Honestly

These are the problems that are genuinely unsolved, or solved only at great expense, in
platforms of this shape. The purpose of this document is to keep you from *discovering them
the hard way*. For each: what's actually hard, what a legitimate floor looks like, and what
the real answer requires. None of these has a free lunch — plan for them.

---

## 1. GDPR erasure vs. immutability

The platform is built on immutable, append-only foundations (the event log; git history).
GDPR's right to erasure is in direct tension with that. The two halves are **not** equally
solved.

**The event-log half has a workable answer.** Separate identity from action: attribute
events by a stable opaque id, and keep personal data (name, email, profile) in a separate
record. Then erasure is **tombstoning / pseudonymisation** — replace the personal data with a
tombstone while preserving the event's structure and causal links for audit integrity. You
delete the *identity*, not the *fact*. Layer soft-delete (default) over hard-delete
(retention/eDiscovery/erasure paths), with tightest-policy-wins retention and legal-hold
awareness.

**The git-history half is genuinely unsolved and usually overlooked.** Author name and email
are **baked into the commit hash**. You cannot tombstone them without rewriting history and
changing every downstream hash. This is the part that the event-log answer does *not* cover,
and pretending it does is the trap. Plan for it explicitly — candidate directions, none free:
- **Pseudonymous commit identities by default** (commit to a stable opaque author, map to the
  person out-of-band) so the immutable bytes never contain erasable PII in the first place.
- A supported **history-rewrite path** for erasure (with the understood, disruptive
  consequence of changed hashes), versus
- Treating commit author metadata under a different lawful basis with documented limits.
- Decide this **before** the git subsystem's data model is fixed; it is nearly impossible to
  bolt on later.

**Also plan:** per-tenant / customer-managed keys as the natural substrate for crypto-shredding
(a v2 capability worth designing seams for now), residency as region-pinning (one tenant's
region is immutable and there is no cross-region query path), tamper-evident eDiscovery
exports, and the EU AI Act angle for any agent that processes personal data. **Open: treat
the erasure-vs-immutability reconciliation as a first-class design item with its own
write-up, not a checkbox.**

## 2. Real-time collaborative editing (the knowledge subsystem)

A Notion-class editor — block-based rich text, in-document databases, real-time
collaboration — is one of the hardest surfaces in the whole platform. Be honest about the
ladder of solutions.

- **A legitimate v1 floor: per-block optimistic compare-and-swap.** Guard each write on the
  block's last-modified token; on a precondition miss, reject the loser and return the current
  server state to reconcile. This **guarantees no *silent* overwrite** — but it **does not
  merge**; concurrent editors of the same block get a conflict, not a blend. Ship it *named as
  a floor*, layered with advisory soft-locks and version snapshot/restore.
- **The real answer is a CRDT** (an Automerge-/Yjs-class library). Treat it as a **named,
  scheduled subsystem, not a "someday"** — the first true concurrent-edit conflict is its
  trigger. Critically: **build the durable, resume-cursor real-time transport *first*** (so a
  dropped connection loses nothing and operations apply idempotently), because the CRDT slots
  into that transport. A real-time relay *without* resume cursors is itself a floor that will
  silently lose the gap on a reconnect — don't mistake it for done.
- **Data-model choices that proved durable:** store inline content as a **markdown-subset
  string** (not an inline-range JSON model) — it survives copy/paste, export, diff, and
  reference-extraction, and keeps saved content human-readable. Model in-document databases as
  a property bag per row, with **rollups and formulas computed at read time, never stored.**
  Expect relation columns to need careful (and initially best-effort) bidirectional
  consistency.

## 3. World-scale git storage

Hosting git at world scale is **the single heaviest subsystem to scale**, and it tends to be
sequenced last for good reason. The authoritative bytes (objects, packs) want to live in an
object store, not on a node's local disk, and that is a deep project (delta/pack management,
sharding, replication, the smart-transport paths). Build on proven structures — content-
addressed objects, Merkle history, pack/delta storage — rather than inventing. Plan the
local-disk-to-object-backed transition as an explicit, sequenced piece of work; don't let
early choices pin repositories to a single node forever.

## 4. The deferral discipline (how to ship floors without lying)

Every hard problem above will ship as a **floor before it ships as the full answer.** That is
correct and necessary. The discipline that keeps it honest:

- **Name the floor and name the follow-on.** "Compare-and-swap floor; full CRDT is the named
  next step." "Single region; multi-region is designed-not-built." "Escape-resistant by
  design; the real-kernel drill is owed."
- **A skeleton/spike is not done — name the half that's missing.** "Inventory shipped;
  ingestion is the follow-on."
- The **scorecard is source-verified, not doc-verified**: a capability is "proven" only when a
  drill produced a green artifact. Until then it is "claimed."
- Track these floors somewhere durable (a gap report) so the next worker sees the real state,
  not the optimistic one. **The gap between ambitious design and shipped floor is normal; the
  gap being *invisible* is the failure.**

## 5. A few more that bite

- **Untrusted code execution** (covered in the agent-fabric doc) is a permanent, never-"done"
  security surface — one escape is catastrophic, and a property not drilled on a real kernel
  is a claim.
- **Event volume**: an append-everything event log can outgrow a general-purpose database;
  keep a seam for a column-store/time-series engine for the highest-volume streams, but don't
  add it before the volume is *measured*.
- **Search and the reference graph** are easy to under-budget — they are the connective
  tissue, and rebuild-from-source (the index never reads source databases; it asks each owner
  to re-emit through the live consumer) is what makes them recoverable and drift-free. Treat
  reindex-from-source as a first-class resilience primitive, not an afterthought.
