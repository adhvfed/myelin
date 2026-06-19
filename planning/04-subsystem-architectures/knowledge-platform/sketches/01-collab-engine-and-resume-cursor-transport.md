# Sketch 01 — Collab engine (TE-15) + the resume-cursor durable transport (KN-1)

> Phase 4, Knowledge subsystem, **exploration** (think-then-discard). Canonical: VISION §3 (name
> your floors), KN-1, KN-4, EI-04 §2, decision-record D11/TE-15. This note weighs the engine
> family and the transport that must be built **first**; the committed direction lands in
> `00-findings.md`.

---

## 0. What KN-1 actually mandates (the doctrine floor)

KN-1 / EI-04 §2.1–2.3 fix the **order of construction**, not just the destination:

1. **Resume-cursor durable transport FIRST** — a reconnect loses *zero* ops; apply is idempotent.
   "A real-time relay *without* resume cursors is itself a floor that will silently lose the gap
   on a reconnect — don't mistake it for done." This is item 0 and the reconnect-loses-zero-ops
   drill (T-5) is mine.
2. **CAS floor SECOND** — per-block optimistic compare-and-swap on a last-modified token, +
   advisory soft-locks + snapshot/restore. The named v1 floor that **does not merge**: concurrent
   editors of the same block get a *conflict*, not a blend.
3. **CRDT THIRD** — promoted on the **first true concurrent-edit conflict** (R5 named trigger).

The editor round-trip gate `render(parse(md)) === md` (KN-4/D10) is the correctness bar
**regardless of engine**. So the engine choice does not block the editor primitives — they can
ship and be unit-tested standalone (KN-4) before any concurrency lands.

The transport is the load-bearing thing to get right, because **both** the CAS floor and the CRDT
slot into it. If I build CAS over a non-resumable relay I will rebuild the transport for the CRDT;
if I build the transport right once, the CAS→CRDT promotion is an apply-function swap.

---

## 1. The transport (KN-1) — candidate designs

The Phase-3 bus already split this off: collab op-streams ride a **separate firehose transport**
(`event-bus.md` §4.3, `firehose::publish/tail`); the durable bus carries only the
`knowledge.doc.updated` *pointer* event. The bus explicitly says the resume-cursor + idempotent-apply
property is **Knowledge's deliverable** — the bus gives me the pointer seam + `tail(stream, range)`,
not the CRDT. So I own the protocol on top of the firehose seam.

What "resume cursor" must mean precisely: every op a client applies has a **monotonic, gap-detectable
position** in its doc's op-stream; on reconnect a client says "I have up to cursor C" and the server
replays `(C, now]` — no more, no less — and the client's apply is **idempotent** so a double-delivered
op at the boundary is a no-op.

### Candidate A — Per-doc append-only op-log in OLTP, server-assigned sequence, firehose fan-out

The op-stream is an **append-only table** `doc_op(tenant, doc_id, seq, op_bytes, actor, lamport, …)`
keyed `(tenant, doc_id, seq)` with `seq` a **per-doc monotonic counter assigned by the authoritative
doc server inside the append transaction** (exactly the per-aggregate `outbox.seq` pattern the bus
uses, `event-bus.md` §3.2/§2.3). The cursor is `seq`. Reconnect = `SELECT … WHERE doc_id=? AND
seq > :cursor ORDER BY seq`. Live fan-out rides `firehose::publish(doc_stream, frame)`; the firehose
is **best-effort** (a dropped frame is fine) because the OLTP log is the durable truth and the cursor
catches any gap on reconnect.

- **Zero-loss on reconnect**: structural. The durable record is the OLTP log; the firehose is just
  the low-latency push. A client that missed frames N..M gets them by `seq`-range replay. The drill
  passes because losing a frame ≠ losing an op.
- **Idempotent apply**: the client dedups on `seq` (and the server dedups an op-submit on a
  client-supplied `op_id`, like the bus's `event_id` dedup). At-least-once + idempotent ≈
  effectively-once (Helland 2012; the exact substrate posture).
- **Cost**: an OLTP row per op. Coalescing (§3) keeps this bounded; periodic snapshot+compaction
  (the `compact→snapshot` job) GCs the op tail. This is the bus outbox-relay pattern reused, which
  is *operationally familiar* and reindex/restore-consistent (the op-log commit is the cross-seam
  anchor, `storage.md` §7.3).
- **Authority**: the doc server is the linearisation point per doc (it owns the `seq` counter and the
  permission/schema check on each op — exactly what CRDTs *cannot* enforce, deep-dive §6). This is
  the "server is the authority for what the merge layer can't enforce" requirement, free.

### Candidate B — Broker-native durable stream per doc (JetStream subject per doc)

Make each doc a JetStream subject `collab.<tenant>.<doc_id>`; the durable consumer's ack-floor is the
cursor; `Nats-Msg-Id = op_id` gives broker dedup.

- **Pro**: reuses the bus's exact durable-pull machinery; cursor + dedup are first-class.
- **Con**: a stream/consumer **per doc** is millions of ephemeral streams (every doc anyone opens).
  JetStream streams are heavier than rows; this is the "stream-per-entity" anti-scale the bus avoids
  by partitioning *within* a stream by aggregate. And the firehose split (`event-bus.md` §4.3) put
  collab on a *separate* transport from the durable bus precisely so collab volume can't melt the
  durable bus — routing it back through JetStream undoes that. **Rejected as the durable record**;
  acceptable only as the *live fan-out* layer (and even there, NATS *core* ephemeral pub/sub is
  lighter and the OLTP log is the truth anyway).

### Candidate C — In-memory authoritative session server + periodic snapshot (Google-Docs-shaped)

A stateful per-doc session actor holds the live state in memory, fans out over websockets, persists
snapshots periodically. Cursor = the in-memory op sequence; durability = the last snapshot + the
in-memory tail.

- **Pro**: lowest latency; the classic OT/Google-Wave model.
- **Con**: the in-memory tail between snapshots is **not durable** — a session-server crash loses
  un-snapshotted ops, which **violates KN-1's zero-loss-on-reconnect** (reconnect after a crash loses
  the gap). To fix it you re-introduce a durable per-op log → you've rebuilt Candidate A under the
  actor. **Rejected as the source of truth**; the in-memory actor is fine as a *cache/relay* over
  Candidate A's log, which is what I'll actually do (a doc session actor that writes-through to the
  op-log and fans out over the firehose).

### Transport leaning

**Candidate A: per-doc append-only op-log in OLTP (server-assigned `seq` = the cursor), with a
stateful per-doc session actor as a write-through cache + firehose fan-out (Candidate C as a layer,
not the truth).** This is the bus's own outbox/`seq`/dedup pattern applied to ops, so it inherits the
restore-consistency (op-log commit is the cross-seam anchor), the reindex posture, and the
operational familiarity — and it makes the zero-loss drill *structural* rather than hoped-for. The
firehose is the push; the log is the truth; the cursor is `seq`.

**This transport is engine-agnostic**: an "op" is opaque bytes to the transport. A CAS op, an OT op,
and a CRDT update all ride the same `doc_op` log with the same `seq` cursor and the same idempotent
apply. *That* is why KN-1 says build it first — it does not change when the engine is promoted.

---

## 2. The engine family (TE-15) — CRDT vs OT vs CAS-floor

### Option 1 — CAS floor (the mandated v1, EI-04 §2.1)

Per-block optimistic compare-and-swap on a `last_modified` token (a per-block version/Lamport stamp).
A write carries the block's expected token; on a precondition miss the loser is rejected and the
current server state returned to reconcile. **Guarantees no *silent* overwrite; does not merge.**
Layered with advisory soft-locks (a UI "Bob is editing this block" lease, advisory only) and version
snapshot/restore.

- **Pro**: trivial to make correct; no merge-correctness research surface; the server enforces
  permission/schema on every op naturally; it is the *honest floor* the doctrine wants me to ship
  named. It rides the Candidate-A transport directly (the CAS token *is* a function of the block's op
  `seq`).
- **Con**: two people typing the same paragraph → one gets a conflict toast. For prose this is a poor
  experience (it's the thing Notion/Google-Docs users never see). For *structured DB rows* and
  *block-tree structure* (insert/move/delete blocks) it's often acceptable. So the floor is
  *differentiated*: fine for DB rows and coarse block ops, weak for concurrent prose in one block.

### Option 2 — OT (Operational Transformation; Google Wave / Google Docs)

Clients send ops; a central server transforms concurrent ops to converge (Ellis & Gibbs 1989;
Nichols et al. Jupiter / Google Wave; Sun & Ellis 1998 on the correctness pitfalls).

- **Pro**: compact ops; mature for *text*; server-authoritative fits my permission/schema enforcement
  point perfectly; small metadata.
- **Con**: transform functions for a **rich block tree** are notoriously hard to get correct (TP1/TP2
  properties; the documented history of OT bugs — Sun & Ellis 1998; Imine et al. on OT correctness).
  Every new block type needs transform logic. Effectively requires a central authoritative server
  (weak offline story). The deep-dive flags this: "transformation functions are notoriously hard to
  get correct for rich trees." High implementation-correctness risk for the block-tree case.

### Option 3 — CRDT (Conflict-free Replicated Data Types; the EI-04 §2.1 "real answer")

Data types that merge deterministically without a central coordinator. For text/lists: RGA (Roh et al.
2011), Logoot (Weiss et al. 2009), **Yjs/Yrs** (the YATA algorithm, Nicolaescu et al. 2016),
**Automerge** (Kleppmann & Beresford 2017). Interleaving fixes: **Fugue** (Weidner et al. 2023).
Rich-text marks: **Peritext** (Litt et al. 2022). Tree moves: **Kleppmann's move operation** (Kleppmann
et al. 2021, "A highly-available move operation for replicated trees").

- **Pro**: offline-first and P2P-capable; server can be a "dumb relay + persistence"; **Yrs is
  Rust-native** (the language-aligned choice, VISION §4 / Phase-2 lean). Modern encodings (Yjs/Yrs)
  have shed the historical per-char-id overhead. Convergence is *guaranteed by construction* — no
  transform-correctness proof burden per block type.
- **Con**: CRDTs guarantee *convergence, not application-level invariants* — they will **not** enforce
  permissions or schema (deep-dive §6); that stays in the server layer above (which I have, the doc
  session actor / op-log authority). Metadata/tombstone overhead → GC/compaction needed. Block-tree
  moves need the move-CRDT (cycle handling). Higher complexity than CAS; less "server can reject a
  single op cleanly" than OT/CAS (a CRDT update is a merge, not a yes/no).

### Engine leaning + the floor→promotion ladder

**Commit to the doctrine ladder verbatim, with the engine choice pre-decided for the promotion:**

- **v1 floor (built): per-block CAS + soft-locks + snapshot/restore**, over the Candidate-A transport.
  Named as a floor that does not merge. *Differentiated UX*: DB rows and block-structure ops use CAS;
  for prose blocks the soft-lock lease + "someone is editing" presence makes the no-merge limitation
  livable in v1.
- **Promotion trigger (R5, named): the first true concurrent-edit conflict on prose** — i.e. when two
  users genuinely need to co-type the same block and the CAS conflict rate crosses a measured
  threshold. That promotes the **inline-text content of a block** to a CRDT.
- **The CRDT, when promoted, is Yrs (Rust Yjs / YATA)** for inline text + a **Kleppmann move-CRDT for
  the block tree**, *over the same Candidate-A op-log transport* (a Yrs update is just opaque op bytes
  on the `doc_op` log; the `seq` cursor and idempotent apply are unchanged). This is the Phase-2 lean
  (Yrs), now justified: Rust-native, battle-tested encoding, convergence-by-construction so no
  transform-correctness burden, offline-first matches the UX, and the server stays the authority for
  permission/schema/erasure *above* the merge layer (the one thing CRDTs can't do).
- **OT is rejected** as the promotion target: the rich-block-tree transform-correctness surface
  (TP1/TP2 across custom block types) is a worse risk than CRDT metadata/GC, and OT's offline story
  is weaker — both of which cut against the UX and the Rust ecosystem (Yrs) we already have.

**Why pre-decide the CRDT now rather than at promotion**: so the transport, the op-log schema, the
snapshot format, and the editor's offset model are all built CRDT-compatible from day one (a Yrs update
is the op payload; the block id is the CRDT root; the inline offset model — KN-4 — is the caret bridge).
The CAS floor is then a *strict subset* of the same machinery (CAS = "the op is a whole-block replace,
guarded by `seq`"), so promotion is additive, not a rewrite. This is the "build the transport once"
payoff applied to the whole stack.

---

## 3. Coalescing, snapshots, history (the supporting decisions)

- **The durable bus gets semantic events, never raw ops** (`knowledge-platform.md` §7.1; bus §4.3):
  the op-log/firehose carries every op; a **debounced/coalesced `knowledge.doc.updated`** (and
  `knowledge.row.updated`) pointer event goes to the durable bus on a quiet-period or N-op threshold.
  Agents/Search/Refs/Notif react to the semantic event, never per-keystroke (the head-of-line defence,
  bus §4.3 / EI-03 §6.1).
- **Snapshots + compaction**: periodic compacted snapshots of the doc state (content-addressed blob,
  `storage.md` §3.2) bound the op-log growth; the op tail since the last snapshot is the live history;
  older ops GC after a snapshot. This is the deep-dive §2.8 hybrid (op-log for live collab + periodic
  snapshots for history/restore). Snapshots double as the **`replay(scope, since)` source** for
  reindex-from-source (a snapshot is a deterministic state at a `seq`).
- **History/restore** = named version snapshots + diff between snapshots; the op-log gives fine-grained
  history until compaction, snapshots give bounded long-term restore points. GDPR erasure reaches both
  via crypto-shred (sketch 06).

## 4. Offline depth (interacts with the engine, deep-dive Q9)

- **v1 floor: read-anywhere + optimistic light-edit-online**, with **queued offline edits behind the
  CAS floor** (an offline edit is an op with an expected `seq`; on reconnect it either applies or
  conflicts — honest, no silent merge). **Full offline-first co-editing is the CRDT-promotion
  follow-on** (a CRDT is *designed* for offline merge; CAS is not). This keeps offline depth tied to
  the engine ladder rather than over-promising merge in v1.

## 5. What this sketch commits to the findings

- **Transport (KN-1)**: per-doc append-only op-log in OLTP, server-assigned `seq` = the resume cursor,
  idempotent apply (op_id + seq dedup), best-effort firehose fan-out, write-through session actor.
  Built **first**. The reconnect-loses-zero-ops drill is structural.
- **Engine (TE-15)**: CAS floor (built, named "does not merge") → Yrs inline-text CRDT + Kleppmann
  move-CRDT for the tree (promoted on first true concurrent-prose conflict), **over the same
  transport**. OT rejected (transform-correctness + offline). Yrs justified: Rust-native, YATA encoding,
  convergence-by-construction, server stays the permission/schema/erasure authority above the merge.
- **The editor round-trip gate** (KN-4) is the correctness bar for either engine and ships standalone
  first (sketch 04 / the editor wireframe).

## Cited prior art

- OT: Ellis & Gibbs, *Concurrency Control in Groupware Systems* (SIGMOD 1989); Nichols et al.,
  *Jupiter Collaboration System* (UIST 1995); Sun & Ellis, *Operational Transformation in Real-Time
  Group Editors: Issues, Algorithms, and Achievements* (CSCW 1998); Google Wave OT whitepaper (2009).
- CRDT: Shapiro et al., *Conflict-free Replicated Data Types* (SSS 2011); Roh et al., *RGA* (JPDC
  2011); Weiss et al., *Logoot* (ICDCS 2009); Nicolaescu et al., *Near Real-Time Peer-to-Peer Shared
  Editing on Extensible Data Types / YATA* (GROUP 2016, the Yjs/Yrs algorithm); Kleppmann & Beresford,
  *A Conflict-Free Replicated JSON Datatype / Automerge* (IEEE TPDS 2017); Weidner et al., *Fugue:
  The Art of Maintaining Maintainable Order* (2023, interleaving); Litt et al., *Peritext: A CRDT for
  Rich-Text Collaboration* (2022); Kleppmann et al., *A Highly-Available Move Operation for Replicated
  Trees* (IEEE TPDS 2021).
- Idempotency/effectively-once + the outbox/`seq`/dedup pattern reused for ops: Helland, *Idempotence
  Is Not a Medical Condition* (ACM Queue 2012); Kleppmann, *DDIA* ch. 11 (logs + change capture).
- Doctrine: EI-04 §2 (the CAS→CRDT ladder, resume-cursor-first); KN-1/KN-4; decision-record D11/TE-15.
