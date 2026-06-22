# myelin-content — the frozen v1 block + inline taxonomy (contract 13.1, X-2/OQ-B)

The shared crate that **freezes** Myelin's canonical content shapes. Knowledge **leads
and freezes** this taxonomy; Chat and Issues consume strict **subsets** (neither adds a
node type, X-2). The three structured inline nodes
(`mention`/`artifact_ref`/`embed`) produce `refs.edge.created` **uniformly** across Chat,
Issues, and Knowledge (contract 5.4).

> This crate is a **FREEZE + the editor PRIMITIVES**. It ships the shared shapes that
> Chat / Issues / Search / Refs compile against (no store, no service) AND, since KN-P08
> (P-298), the three editor primitives STANDALONE (`editor` module) — the offset model +
> the DOM-surgery, both over the ONE frozen render path. The OLTP store, collab transport,
> and the INTEGRATED editor land in the Knowledge M3 prompts (KN-P04+ / KN-P09).

## The editor primitives (`editor` module, KN-P08 → P-298)

The three primitives ship + unit-test **standalone before the integrated editor** (EI-05
§2; design-system `02-components/block-editor.md` §2 rule 5 — *"Enter just inserts a
newline" is the #1 'not a real editor' tell*):

1. **The serializer** — already frozen in `inline` (`parse_inline`/`serialize_inline`); the
   primitives REUSE it (no second renderer). Its standalone KN-D2 leg is in `corpus`.
2. **The offset model** (`editor::offset`) — **the caret is a char offset into the
   serialized markdown**, bridged to/from controlled-`contenteditable` DOM positions
   (`offset_to_dom` / `dom_to_offset`). A serialized line tiles into a **segment grid**:
   `Text` runs (each char a caret stop) interleaved with single-`OBJ` `Node` islands — a
   **structured node is exactly one caret position**. The bridge is a total bijection on
   `0..=char_len` with **0 off-by-one**.
3. **The DOM-surgery** (`editor::surgery`) — **Enter-splits-a-block** (`split_at`) +
   **caret-placement-after-split** (`BlockSplit::caret == 0` of the new block, always), and
   the **paste/IME normalisation** seam (`normalize_paste` / `insert_text`) that re-parses
   + re-serializes through the SAME render path (never injects raw). Char offsets, never
   byte offsets (the CJK / accented-input obligation, G2).

WASM-clean by construction (`std`-only + the frozen `inline` core), so the primitives
compile native (server) AND to `wasm32-unknown-unknown` (the editor) from one source. The
integrated single-doc editor over these + the KN-P07 transport (browser-drive, KN-D2
re-run) is the **immediate follow-on KN-P09 (P-299)** — a green primitive here is NOT yet
an editor.

### The editor-primitives gates (CI — `tests/editor_primitives_gate.rs`)

- **KN-D2 standalone leg** — `serialize_inline(parse_inline(md)) === md` 100% over the
  frozen corpus on the serializer primitive (0 regressions).
- **The offset/DOM-surgery property gate** — the caret round-trips DOM-position ↔
  char-offset across **every structured node** in the corpus (**off-by-one count == 0**),
  and the **caret-placement counter** is green (every Enter-split lands the caret at offset
  0 of the new block).

## What's frozen here

- **`block::Block`** — the canonical 15-variant v1 taxonomy, byte-for-byte from
  architecture `01-tech-and-data-model.md` §2.1: paragraph / heading(1..6) / bullet_list /
  ordered_list / task_list / blockquote / code_block / callout / table / divider / image /
  embed / db_view / toggle / sync_block. `code_block.text` is **raw** (NOT markdown-parsed).
- **`inline`** — the markdown-subset inline grammar (§2.2): `**bold**`, `*italic*`,
  `` `code` ``, `~~strike~~`, `[text](url)`, plus the three structured nodes encoded as a
  single `U+FFFC` placeholder with a positional `inline_nodes` array (the i-th `U+FFFC`
  binds `nodes[i]`). Reference-extraction is a **node-array walk**, never a regex over
  prose.
- **The one render path** — `parse_inline` / `serialize_inline` are ONE implementation,
  compiled native (server) and to `wasm32-unknown-unknown` (client editor). This
  eliminates the two-divergent-renderers trap structurally (EI-01 §7).

## The KN-D2 gate

`serialize_inline(parse_inline(md)) == md` over the frozen corpus
(`crates/myelin-content/corpus/`): **100% round-trip, 0 regressions**. The corpus is
embedded at compile time (`include_str!`) so the gate runs identically native and on WASM.

```
cargo test -p myelin-content          # the round-trip gate (proven green natively)
./build-wasm.sh                        # the WASM-artifact gate (one source → wasm32)
```

The corpus deliberately exercises the three structured nodes anchored in bold / lists /
tables, code blocks, and IME/paste edge cases.

### Frozen disambiguation rule

Markdown is ambiguous where a closing delimiter run is longer than its opener (`***`
closing `**`). The frozen v1 rule: match the closing run at its **last `width` chars**, so
surplus delimiters bind the nested emphasis (`**bold *and italic***` = bold(`bold ` +
italic)). Adjacent emphasis is written with a separator (`**a** *b*`). This single rule
makes the round-trip byte-stable.

## Named floors

- **`sync_block` engine** — the node TYPE is frozen so the taxonomy is complete, but its
  v1 engine is a **read-projection FLOOR** (renders like `embed`, §2.4) shipped in
  **KN-P12**; the editable-in-place multi-home follow-on is post-M5 (**KQ-6**, designed
  against the CRDT).
- **`db_view.view`** — RESOLVED in **KN-P02 (P-235)**: the `ViewHandle` floor is gone;
  `db_view` now carries the frozen `myelin_query::ViewSpec` (13.3, X-3) directly. The
  **ADF → `myelin-content` lossy-map (13.2)** also landed here (`adf` module): Knowledge
  freezes the conversion table; Issues consumes it at import.
- **WASM-artifact green** — the round-trip correctness gate is proven green natively
  against the identical single source. The "compiles to wasm32 from one source" artifact
  is gated by `build-wasm.sh`; on a host without the `wasm32-unknown-unknown` std
  component it is **red-until-proven** (flips green on CI / any host with the target
  installed).

## Mutation floor

`myelin-content`'s parser/serializer AND the editor offset model + DOM-surgery are
mandatory-core: their correctness properties must **survive mutation**. The cargo-mutants
score floor is **≥ 0.85** (≥85% of viable mutants caught).

**`inline` module** (the parser/serializer):

```
cargo mutants -p myelin-content --file crates/myelin-content/src/inline.rs --timeout 30
```

**Measured 2026-06-21:** 163/184 viable mutants caught (incl. timeouts) = **0.886** —
above the 0.85 floor. The residual survivors are loop-bound mutations in `parse_link`
that produce equivalent behaviour or infinite loops (timeouts); the round-trip + boundary
+ malformed-link tests pin the observable behaviour.

**`editor::offset` + `editor::surgery`** (the offset model + DOM-surgery, KN-P08):

```
cargo mutants -p myelin-content \
  --file crates/myelin-content/src/editor/offset.rs \
  --file crates/myelin-content/src/editor/surgery.rs --timeout 30
```

**Measured 2026-06-22:** **41/41 viable mutants caught = 1.000** — every viable mutant in
the offset bridge + the Enter-split + the paste/IME normalisation is killed by the offset
round-trip gate + the caret-placement counter + the targeted boundary tests (no dead
defensive branch — the `offset_to_dom` fallthrough is an `unreachable!` documenting the
tiled-grid invariant, not a behaviour-equivalent arithmetic branch a mutant can survive
on). This is the dated green artifact for the editor-primitives mutation floor.
