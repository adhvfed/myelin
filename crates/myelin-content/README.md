# myelin-content — the frozen v1 block + inline taxonomy (contract 13.1, X-2/OQ-B)

The shared crate that **freezes** Myelin's canonical content shapes. Knowledge **leads
and freezes** this taxonomy; Chat and Issues consume strict **subsets** (neither adds a
node type, X-2). The three structured inline nodes
(`mention`/`artifact_ref`/`embed`) produce `refs.edge.created` **uniformly** across Chat,
Issues, and Knowledge (contract 5.4).

> This crate is a **FREEZE**, not a feature. It ships ONLY the shared shapes that
> Chat / Issues / Search / Refs compile against — no store, no service, no editor. The
> block tree, OLTP store, collab transport, and editor land in the Knowledge M3 prompts
> (KN-P04+).

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
- **`db_view.view`** — carried as an `ArtifactRef`-keyed `ViewHandle` until
  `myelin-query::ViewSpec` is frozen in **KN-P02** (a single compile break swaps it in).
- **WASM-artifact green** — the round-trip correctness gate is proven green natively
  against the identical single source. The "compiles to wasm32 from one source" artifact
  is gated by `build-wasm.sh`; on a host without the `wasm32-unknown-unknown` std
  component it is **red-until-proven** (flips green on CI / any host with the target
  installed).

## Mutation floor

`myelin-content`'s parser/serializer is mandatory-core: the round-trip property must
**survive mutation**. The cargo-mutants score floor for the `inline` module is **≥ 0.85**
(≥85% of viable mutants caught by the round-trip + unit tests). Run:

```
cargo mutants -p myelin-content --file crates/myelin-content/src/inline.rs --timeout 30
```

**Measured 2026-06-21:** 163/184 viable mutants caught (incl. timeouts) = **0.886** —
above the 0.85 floor. The residual survivors are loop-bound mutations in `parse_link`
that produce equivalent behaviour or infinite loops (timeouts); the round-trip + boundary
+ malformed-link tests pin the observable behaviour. This is the dated green artifact for
the mutation floor.
