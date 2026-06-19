# Knowledge Platform — 08 · Committed Resolutions (the Stage-1 open questions, now decided)

> See [`00-overview.md`](./00-overview.md) for framing. Stage-1
> ([`../sketches/00-findings.md`](../sketches/00-findings.md) §6) handed nine **architecture-shaped** open
> questions to Stage-2 with the explicit instruction "*Stage 2 commits them; Stage 1 has bounded each.*"
> This doc commits each — the concrete encoding/cadence/mechanism the rest of the architecture
> ([`01`](./01-tech-and-data-model.md)–[`07`](./07-drills-and-open-questions.md)) assumes. Each cites prior
> art and names a floor where v1 is partial (VISION §3). These are now **DECIDED**, not open.
>
> (The broader Phase-5 open questions — measured promotion thresholds, cross-subsystem consolidations —
> remain in [`07 §2`](./07-drills-and-open-questions.md). This doc closes the *design* questions Stage-1
> bounded for me to settle now.)

---

## CR-A · Fractional-index rebalancing (Stage-1 Q1, sketch 02)

**The question:** when concurrent inserts exhaust the precision of an `order_key` between two siblings, how
and when do we rebalance without a disruptive whole-doc rewrite — and how does this interact with the
move-CRDT?

**Decision (DECIDED):**

1. **Base-62 fractional keys with unbounded length, append-on-collision** (the LexoRank / Figma-LSEQ family,
   Wang & Mehdi *LSEQ* 2013; Figma's "fractional indexing" write-up, 2022). An insert between `a` and `c`
   mints `b`; an insert between `a` and `b` mints `aU` (a suffix), **growing the string, never renumbering
   siblings**. Keys are arbitrary-length strings (`order_key text`, [01 §2](./01-tech-and-data-model.md)), so
   precision is *never structurally exhausted* — it degrades to longer keys.
2. **Per-position jitter on the suffix** (a small random low-order component) so two clients inserting "at the
   same gap" concurrently mint **different** keys with high probability — avoiding the interleaving pathology
   where concurrent same-gap inserts collide deterministically (the Logoot/RGA interleaving anomaly Fugue
   fixes, [05 §1](./05-hard-problems.md)). On the rare exact collision, the stable `block_id` is the
   deterministic tie-break (total order = `(order_key, block_id)`).
3. **Rebalancing is a background, lazy, per-sibling-list operation**, triggered by a **measured key-length
   threshold** (e.g. a sibling list whose max key length crosses a budget), never a hot-path whole-doc
   rewrite. Rebalance re-spreads *one sibling list's* keys evenly and is itself an ordinary op stream on the
   transport ([02 §2](./02-internals-and-algorithms.md)) — idempotent, replayable, and **emitted as
   `move` ops** so concurrent editors converge. A rebalance touches at most one parent's children, so its
   blast radius is one list, not the document.

**Interaction with the move-CRDT (the promotion, [02 §3.3](./02-internals-and-algorithms.md)):** once the
Yrs/Kleppmann move-CRDT lands, **the CRDT's list type owns sibling ordering** and the fractional key becomes
a *derived* ordering hint for the OLTP index range read, recomputed from the CRDT state — rebalancing then
falls out of the CRDT's own structure and the bespoke jitter logic retires. Until then, the
jitter+lazy-rebalance floor is the CAS-era answer. **Floor:** the CAS-era rebalancer is named; the CRDT
subsumes it.

**Drill tie-in:** the hot-document drill (KD-8) includes a *concurrent-same-gap-insert storm* asserting
no key collision-induced reorder and bounded rebalance cost.

---

## CR-B · Snapshot compaction cadence + format (Stage-1 Q2, sketch 01/06)

**The question:** the snapshot is the `replay` source **and** the history restore point **and** the
crypto-shred unit boundary — one format serving three masters. Pin the cadence + format.

**Decision (DECIDED):** one snapshot format, three consumers, content-addressed.

- **Format.** A `doc_snapshot` ([01 §3](./01-tech-and-data-model.md)) is a **content-addressed (BLAKE3) blob
  in the object tier** holding the full materialised block-tree state at `snap_seq` — the *serialised*
  block rows (tree structure + `props` + inline md-subset string + the `inline_nodes` structured-node array),
  **not** a fold of opaque ops. This is the one format the three masters share:
  - **`replay` source** ([03 §2.3](./03-events-contracts-and-glue.md)): the snapshot is walked to emit
    `knowledge.page.snapshot` at **block granularity** + the per-edge `refs.edge.snapshot` — so the rebuild
    path reads a snapshot, not the live OLTP, and cold==live (KD-6).
  - **History restore point**: a named or auto snapshot is a restore target; restore applies the snapshot
    state as a new op (not a destructive overwrite — restore is itself forward history).
  - **Crypto-shred unit boundary**: PII-bearing inline runs inside the snapshot are encrypted under the
    **per-subject DEK** (GD-4, CR-I below), so destroying the key shreds the subject's content **inside every
    snapshot** without rewriting the immutable blob ([05 §6](./05-hard-problems.md)). The snapshot blob itself
    is per-tenant-DEK wrapped (K6).
- **Cadence (the doctrine floor — measured, then promoted):**
  - **Op-count trigger**: compact when `doc_op` live-tail rows for a doc exceed a budget (e.g. N ops) — keeps
    the resume read (`op_seq > cursor`) bounded and the op-log table small. The log-as-truth pattern (Kreps,
    *The Log*, 2013): the snapshot is a checkpoint; the live tail is the delta since.
  - **Quiescence trigger**: also compact on a doc going idle (no ops for a window) so a quiet doc settles to
    one snapshot + empty tail.
  - **Named-version trigger**: a user "save version" / publish mints a *named* snapshot (`named_label` set),
    never GC'd by auto-compaction.
  - GC: after compaction, `doc_op` rows ≤ `snap_seq` are deleted **except** those an open client's resume
    cursor still trails (a slow reconnecting client never loses its gap — the cursor is the GC watermark,
    KN-1). This is the one rule that makes the KD-1 reconnect drill survive compaction.

**Floor:** the exact N (op-count) and idle window are **measured-promotion thresholds** (KQ-4-adjacent); the
*format and the three-masters discipline* are committed now.

---

## CR-C · Structured-inline-node placeholder encoding (Stage-1 Q3, sketch 02)

**The question:** how is a `mention`/`artifact_ref`/`embed` structured node anchored *inside* the
markdown-subset inline string so it (a) survives copy/paste + the `render(parse(md)) === md` round-trip, and
(b) is unambiguous for reference-extraction — never a regex-over-prose.

**Decision (DECIDED): an explicit, escapable sentinel token `￼{kind:idx}` is rejected in favour of an
explicit bracketed marker `⟦ref:N⟧`-class token bound to the `inline_nodes` array by index.** Concretely:

- The inline string holds a **single-character logical placeholder per structured node** at the node's
  offset; the placeholder is the Unicode **Object Replacement Character `U+FFFC`** (the standard
  "an object goes here" code point — used by ICU/CSS/AT for exactly this), and the *binding* to which object
  is the **positional order of `U+FFFC` occurrences mapped to `inline_nodes[]`** (the i-th `U+FFFC` ⇒
  `inline_nodes[i]`). The node carries its own `kind` + `target ArtifactRef`/`principal_id` in the array, so
  the string never carries the id.
- **Why a single offset, not a span:** a mention/ref/embed is **atomic** — the caret treats it as one
  position (you can't put the cursor *inside* `@alice`), which the offset model ([02 §8.2](./02-internals-and-algorithms.md))
  requires. `U+FFFC` is one UTF-8-stable code point, so caret-offset arithmetic is uniform with text runs.
- **Round-trip + copy/paste:** on **serialize for export/markdown** (CLI `page get --format md`,
  [04 §2](./04-views-cli-and-api.md)), `U+FFFC[i]` is rendered to a *human-portable* markdown form — a
  mention to `@Display Name`, an `artifact_ref` to its `myelin://…` URN, an embed to a fenced
  `!embed(myelin://…)` directive — and `parse` reconstructs the node + re-anchors `U+FFFC`. The
  **`render(parse(md)) === md` corpus (KD-2) carries fixtures for every structured-node kind**, nested in
  bold/lists/tables, plus paste-from-Word and IME edge cases. The internal storage form (`U+FFFC` +
  `inline_nodes`) and the export form (URN/`@name`/directive) are two serializations of the one AST, both
  produced by the one `myelin-content` core (KN-4).
- **Reference-extraction is a node-array walk, never a string regex:** the producer of `refs.edge.created`
  ([03 §1.4](./03-events-contracts-and-glue.md)) iterates `inline_nodes`, not the prose — so rename and erase
  **never** touch stored prose, and extraction is exact (Stage-1 finding §1.5). This is the load-bearing
  reason the structured node is kept *out* of the string (KN-2 / D10).

**Corpus addition (KD-2):** the `U+FFFC`-anchored fixtures (mention/ref/embed × nesting × paste/IME) are
added to the round-trip corpus. **No floor** — committed.

---

## CR-D · Row-level permission: tuple vs ABAC caveat (Stage-1 Q4, sketch 04)

**The question:** which mechanism for "see only your team's rows" stays off the hot `list_objects` path at
scale — tuple-per-row, or an ABAC caveat — and the per-database opt-in UX.

**Decision (DECIDED): row-level visibility is a `database_row#read` *userset relation* (a tuple), NOT a
per-row ABAC caveat; field-level visibility is the ABAC caveat.** The split is deliberate:

- **Row-level = tuples** ([01 §5](./01-tech-and-data-model.md), `database_row.row_reader`). The common
  "see only your team's rows" is a **`row_reader: team#member`** tuple on the database (or on a small set of
  row-groups), not one tuple per row: a single group-grant covers thousands of rows via tuple-to-userset
  rewrite (Zanzibar, Pang et al. USENIX ATC 2019). This stays **on** the `list_objects` push-down path
  (CR-E) — it is a set the index can answer in bulk. Per-row tuples are reserved for the rare explicit
  single-row grant. This avoids the *caveat-per-row evaluation* cost that would force a row-by-row `check`.
- **Field-level = ABAC caveat off the hot path** ([05 §5](./05-hard-problems.md)): "hide the salary column"
  is a caveat on a `field.view` permission evaluated at `check` time with context (SpiceDB caveats / OpenFGA
  conditions; NIST SP 800-162). Field-hiding is a **render-time projection** on the already-permitted page of
  rows — it never enters the bulk `list_objects` filter, so it cannot blow up the pre-filter.
- **The per-database opt-in UX** ([04 §1.7](./04-views-cli-and-api.md), sharing dialog): row-level visibility
  is **off by default** (a database inherits page-level read). A db owner opts in per-database ("restrict rows
  by a person/team field"), which **declares the `row_reader` grant rule** — the UI writes the group tuples,
  not the owner hand-tupling rows. This keeps the buyer-facing capability ("teams see their own rows") without
  the operator authoring per-row ACLs.

**Floor:** the per-database *predicate catalogue* for field-level caveats (which columns, which contexts) is
the KQ-5 P5 detail co-designed with Id's role-bundle catalogue; the **mechanism split (rows=tuples,
fields=caveats) is committed now.**

---

## CR-E · The `list_objects` push-down for big-DB views (Stage-1 Q5, sketch 03; search §10)

**The question:** how does the ACL filter conjoin into the structured DB query at scale, rather than
post-filtering a permission-blind result?

**Decision (DECIDED): `list_objects` returns a *set expression* (a `Filter{set_expr}`, Id §8.2), and the DB
query lowers it into the SQL `WHERE`, conjoined with the JSONB predicate — never a post-filter, never an
opaque materialised id-list at scale.** The execution ([02 §4.1](./02-internals-and-algorithms.md)):

1. `list_objects(viewer, read, 'database_row', zookie)` returns a **filter** the query can push down:
   - **the common case** (a database the viewer can read wholesale via page-level inheritance, no row-level
     opt-in) ⇒ the filter is a single **`parent_db` membership predicate** — effectively "all rows of this
     db", a no-op conjunct, so the JSONB query runs unencumbered.
   - **the row-restricted case** (CR-D opt-in) ⇒ the filter is a **set membership over the row-group facet**
     (e.g. `props->>'team' IN (viewer's teams)`), pushed down as a SQL predicate **against the derived
     projection column** ([01 §4.1](./01-tech-and-data-model.md)) — the same generated/expression index the
     filter/sort path uses, so the ACL conjunct is *index-served*, not a scan.
   - **the explicit-grant case** (rare per-row tuples) ⇒ a bounded id-list `IN (…)` — bounded because explicit
     per-row grants are rare by CR-D's design.
2. The conjunction is **always present** (`acl_clause(filter)` in [02 §4.1](./02-internals-and-algorithms.md)),
   so a row the viewer can't read is **absent from the result, not post-filtered** — closing the count-leak
   (KD-5: even an aggregate/`COUNT` over the view is permission-correct because the ACL conjunct is *inside*
   the query).

**The shared dependency (ID-CR-2 / SEARCH ask S-10, [06 §3](./06-shared-system-change-requests.md)):**
`list_objects` `Filter` must be **facet-expressible / push-downable over an arbitrary id-or-facet column**,
not opaque-id-only. This is the *same* ask Search and Refs made; Knowledge confirms the usage and the
row-group-facet shape. **No floor** — committed, with the shared-contract confirmation logged in [06](./06-shared-system-change-requests.md).

---

## CR-F · CAS→CRDT online per-block migration (Stage-1 Q6, sketch 01)

**The question:** the concrete metric + threshold that fires the CRDT promotion (R5), and **how a per-block
migration from CAS to a Yrs doc runs online** without a stop-the-world cutover.

**Decision (DECIDED):**

- **The metric (the trigger):** the **per-doc CAS conflict rate** — the fraction of `EDIT_BLOCK` ops that
  hit a precondition miss (`rows_affected == 0`, [02 §3.2](./02-internals-and-algorithms.md)) — telemetered
  per doc and per tenant (KD-3 reads it). Promotion fires when a doc's (or a tenant's hot-doc class's)
  same-block concurrent-conflict rate crosses a **measured threshold** (KQ-1; the exact number is Phase-5
  measured, not guessed). A second signal: sustained multi-author *simultaneous* presence on one block.
- **The online migration (per-doc, not global):** because the **transport is engine-agnostic** (KN-1; the
  op-log, `op_seq` cursor, and idempotent apply are identical for CAS and Yrs, [02 §2](./02-internals-and-algorithms.md)),
  promotion is a **Layer-3 swap on one doc at a time**, run as an ordinary op sequence:
  1. **Quiesce-lite**: at a compaction boundary (CR-B), snapshot the doc's current materialised state.
  2. **Seed**: construct the Yrs document from that snapshot (block tree → `Y.XmlFragment`/move-CRDT; each
     block's inline md-subset string → `Y.Text`). This is a deterministic function of the snapshot, so it is
     reproducible and replay-safe.
  3. **Cutover op**: append a single `engine_promote` op to the doc's op-log at the next `op_seq`. From that
     `op_seq` forward, `doc_op.payload` carries **Yrs update bytes**; before it, CAS deltas. A reconnecting
     client resumes from its cursor and learns the engine at the `engine_promote` boundary — **zero ops lost**
     (the resume read spans the boundary; the client loads the seeded Yrs state once and applies the tail).
  4. In-flight CAS edits straddling the cutover reconcile via the **last CAS conflict check** (no edit is
     silently dropped); the editor client, being CRDT-ready from day one (KN-4), switches its merge module at
     the boundary with no schema change.
- **Reversibility:** because the snapshot at step 1 predates the cutover, a botched promotion rolls back to
  the pre-cutover snapshot (forward history, CR-B) — the migration is not a one-way door.

**Floor:** v1 ships the **CAS floor + the engine-agnostic transport that makes this swap possible**; the Yrs
seed/cutover machinery is **built when the trigger fires** (the named promotion, [05 §1](./05-hard-problems.md)).
The *mechanism* (snapshot→seed→`engine_promote` op, per-doc, online) is committed now so the transport is
designed for it. **Drill:** KD-1 (reconnect) is re-run *across* an `engine_promote` boundary.

---

## CR-G · Comments: shared thread component vs KB-native (Stage-1 Q7, sketch 07)

**The question:** the comment data+anchor is KB-native; is the *rendering* reused from Chat's component, or
KB-native?

**Decision (DECIDED): the comment *data model + anchor* is KB-native and owned by Knowledge; the *thread
rendering component* is the shared design-system thread primitive, reused — not forked.** Concretely:

- **Data (Knowledge-owned):** a comment is a Knowledge sub-artifact (`#comment-<id>`,
  [03 §2.1](./03-events-contracts-and-glue.md)) anchored to a **block id + an optional character range**
  inside that block's inline string (the offset model, [02 §8.2](./02-internals-and-algorithms.md)). The
  anchor must survive concurrent edits to the block — so it is a **block-id + relative offset that the CRDT
  re-anchors** (a Yrs *relative position* once promoted; on the CAS floor, a block-id + offset that degrades
  to "comment on this block" if the exact range shifts). Comment lifecycle events
  (`knowledge.comment.created`/`.resolved`) and mention→Notif are Knowledge's ([03 §1.5](./03-events-contracts-and-glue.md)).
- **Rendering (shared component):** the *thread* UI (a list of authored messages, reply, resolve, @-mention
  autocomplete, the identity/agent badge) is the **shared design-system thread component** (DL §8.1 — one
  shell, no per-subsystem design system). Chat and Knowledge both *consume* it; neither owns the other's
  data. This honours "share the implementation, not a fork" without coupling Knowledge's anchor model to
  Chat's message model.

**Floor:** whether the shared thread component is *first authored* in Chat or extracted to the design system
is a **cross-subsystem sequencing call co-owned with Chat (KQ-2)** — but the *decision that Knowledge reuses
the shared thread render and owns the anchor data* is committed now.

---

## CR-H · Cross-cell collab fan-out (Stage-1 Q8, sketch 07)

**The question:** the named multi-cell floor — the control-plane PII-free pointer-bridge detail, co-owned
with the bus/Refs cross-cell floor + SC-2/SC-3.

**Decision (DECIDED — confirming the floor + pinning the bridge shape):** v1 **pins a doc's authoritative
collab session to the tenant's cell** (residency by construction, [05 §9](./05-hard-problems.md)); cross-cell
collaboration for a multi-cell tenant is **designed-not-built**, and the bridge shape is now pinned so the
contracts extend without a rewrite:

- **The op-stream never crosses a cell.** A doc lives in exactly one cell; all its collab ops, presence, and
  op-log are local. A user in another cell who opens the doc is **routed to the doc's home cell** for the live
  session (the session follows the data, not the user) — keeping collab-session state residency-pinned (a GDPR
  property, not just latency, [05 §9](./05-hard-problems.md)).
- **The control plane carries only a PII-free pointer** (ADR-11.4; event-bus §7.4): "doc `myelin://…` is
  homed in cell EU-1". No content, no presence, no PII crosses the control plane — only the **routing
  pointer** + the cross-cell `*.snapshot`/`*.erased` lifecycle pointers Refs/Search already bridge. Each cell
  resolves `ArtifactRef`s **locally, per viewer** via `project` (the contract is cell-agnostic, ADR-13).
- **What's deferred (the floor):** *simultaneous* low-latency co-editing of one doc by users physically in
  two different cells (true cross-cell op fan-out) is the named follow-on, owned by **control-plane /
  multi-cell tenancy (SC-2/SC-3)** — it inherits the bus cross-cell pointer bridge, not a Knowledge-bespoke
  channel.

**Floor:** committed as the floor; the bridge is a *pointer*, the session is *pinned*, the contracts are
*cell-agnostic*. **Owner:** P5 control-plane + SC-2/SC-3 (KQ-7).

---

## CR-I · Per-subject DEK granularity vs key-count explosion (Stage-1 Q9, sketch 06)

**The question:** confirm the GD-4 rule keys **only** the genuinely-inline-PII classes per-subject (not every
block) to avoid millions of keys per tenant.

**Decision (DECIDED): the per-subject DEK is allocated per (subject × tenant), and applied *selectively* only
to the PII-bearing content classes — NOT per block, per op, or per row.** The granularity rule:

- **One per-subject DEK per (subject, tenant)** (Storage §5.1; GD-4). Not one key per block, not per op — a
  *subject* has one key in a tenant, reused across all of that subject's PII-bearing content. So a tenant's
  key count is **O(subjects who have inline PII), not O(blocks)** — bounded by people, not by content volume.
  This is the explicit answer to the key-count-explosion fear (Stage-1 Q9).
- **Selective application** ([01 §2](./01-tech-and-data-model.md), `contains_personal_data` +
  `pii_key_ref`): only blocks/ops/rows the data-map flags as carrying a *specific subject's* inline free-text
  PII get encrypted under that subject's DEK. The vast majority of content (`contains_personal_data = false`)
  is under the **per-tenant DEK** (bulk structure, K1–K3) and survives untouched on a subject erasure. The
  `#[personal_data(...)]` tags (Stage-1 §4) drive which columns are candidates; a classifier/flag sets the
  per-block `pii_key_ref`.
- **The "many subjects in one block" case:** a block mentioning two people's PII is encrypted under a
  **composite** — the block's PII runs are split by subject where the data-map can attribute them, each run
  under its subject's DEK; where attribution is ambiguous, it falls into the **free-text residual**
  (GD-6, [05 §6](./05-hard-problems.md)) — the honest limit. Structured references (the `person` prop, the
  `mention` node) are *always* attributable and erase reliably.
- **Authorship is never a per-subject-DEK case** — it's the opaque `principal_id` in Id's erasable pseudonym
  map ([03 §6.1](./03-events-contracts-and-glue.md)), so erasing an *author* needs no per-block key at all.

**Floor:** the *classifier* that decides which free-text runs are a given subject's PII is the GD-6 residual
(tooling + process, co-owned with Legal); the **key-allocation rule (per-subject-per-tenant, selective, not
per-block) is committed now** — it is the thing that keeps key count bounded. **Drill:** KD-4 asserts a
subject erasure crypto-shreds exactly their content with a *bounded* key-shred operation (one key per
subject), and that `contains_personal_data=false` content is untouched.

---

## Summary — the nine, now committed

| # | Stage-1 question | Committed resolution | Floor |
|---|---|---|---|
| CR-A | Fractional-index rebalancing | base-62 unbounded keys + jitter + lazy per-list rebalance via `move` ops; CRDT subsumes | CAS-era rebalancer → CRDT list type |
| CR-B | Snapshot cadence + format | content-addressed serialised-tree blob; op-count/quiescence/named triggers; cursor = GC watermark | thresholds measured |
| CR-C | Inline-node placeholder | `U+FFFC` positional anchor → `inline_nodes[]`; export to URN/`@name`/directive; node-walk extraction | none |
| CR-D | Row vs field permission | rows = userset tuples (group grant); fields = ABAC caveat off hot path; per-db opt-in | field predicate catalogue = P5 |
| CR-E | `list_objects` push-down | `Filter{set_expr}` lowered into SQL `WHERE` over the derived projection column; no post-filter | none (shared confirm) |
| CR-F | CAS→CRDT online migration | per-doc snapshot→seed Yrs→`engine_promote` op at a `op_seq` boundary; reversible | machinery built at trigger |
| CR-G | Comments component | KB-native anchor data + shared thread render component | extract-from-Chat sequencing = P5 |
| CR-H | Cross-cell collab | session pinned to home cell; control plane carries PII-free routing pointer only | true cross-cell fan-out = SC-2/3 |
| CR-I | Per-subject DEK granularity | one DEK per (subject, tenant), selective on flagged PII classes only | classifier = GD-6 residual |

Index: [`../README.md`](../README.md).
