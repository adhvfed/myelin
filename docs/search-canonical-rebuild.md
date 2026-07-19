# Handoff — legacy→canonical Git blob search identity migration

**Branch:** `claude/search-canonical-rebuild` (pushed, not merged)
**Base:** `codex/ci-publisher-activation` @ `95795fd`
**Worktree:** `/home/adhv/Projects/myelin-worktrees/search-canonical-rebuild` (isolation `iso-e13d7f27`, services stopped, left for review)
**Date:** 2026-07-20

---

## TL;DR

Git blob projection identities moved from raw slash-delimited ids to canonical
percent-encoded `ArtifactRef`s. The index is keyed by the id string, so that cutover rewrote
nothing: every document, vector and metadata record written under a legacy id survives untouched
and unaddressable by the new writer.

This branch ships the repair — a durable, fenced, phase-ordered index rebuild — plus a fix for the
tombstone bug that made the problem permanent.

**One piece is independently verified and shippable on its own: `23fdddd`, the real blob tombstone.**
Everything else is correct-and-drilled but **has no production caller yet** (see
[Not wired](#not-wired-read-this-before-trusting-it)).

---

## Why it mattered

The failure modes were all silent — none surfaced as an error, only as wrong answers:

| symptom | cause |
|---|---|
| a live blob answers queries **twice** | canonical re-index ADDS a document rather than replacing the legacy one |
| **deleted** source code stays queryable | the delete addressed the canonical id; the legacy twin survived |
| **restricted** content stays queryable | same, on the content a GDPR `restrict` exists to suppress |
| semantic queries answer from nothing | a legacy **vector** survives with no live document |

Compounding it: `git.blob.snapshot` carrying payload `op = "delete"` was **never a Search
tombstone**. Search dispatches removal on the event type's trailing verb (`deleted`/`removed`/
`erased`) or an owner `project` resolving `Gone` — it never reads a payload `op`. Deletes therefore
fell through to the UPSERT path.

---

## What landed

15 commits. Read them in order; each carries its reasoning.

| commit | what |
|---|---|
| `23fdddd` | **the tombstone fix** — `git.blob.removed`, a verb the indexer honours |
| `2f48e03` | `canonical` — the legacy/canonical discriminator + 3-space index inventory |
| `e97b896` | the durable coordinator: phases, exclusive lease, forward-only migrations |
| `de3e325` | fail-empty read fence + result-cache fence |
| `29571e8` | the adversarial drill suite |
| `27b4459` | disclosure sweep (tenant ids, artifact refs out of errors) |
| `14306a3` | live-Postgres proof of journal exclusivity |
| `4d55d34` | round-1 verifier fixes |
| `42d39be` | make the fence bite on real entries |
| `02c8b7c` | seq-based catch-up bound; erase refuses mid-rebuild |
| `d965a01` | Git canonical blob truth enumeration |
| `64f6a0d` | owner-truth verification; gated serving constructor |
| `848f4ef` | refuse to verify a rebuild that destroyed the corpus |
| `121b15f` | disambiguate blob aggregate keys |
| `9b18e6a` | catch PARTIAL corpus loss |

### The phase machine (`crates/myelin-search/src/rebuild.rs`)

```
Claimed → Fenced → Wiped → CursorsReset → Replayed → CaughtUp → Verified → Complete
```

The ordering carries the safety argument:

- **Fence before wipe** — wiping a served index is a silent wrong-answer window.
- **Mark at fence time** — a mark taken after the replay leaves events that arrived during it
  permanently unapplied.
- **Reset cursors after the wipe** — so no crash window leaves a cursor trusted for documents that
  still exist.
- **Verify before reopening** — a half-succeeded rebuild must be distinguishable from a finished one.

Crash convergence: each transition is journaled **after** its idempotent action, so a crash before
the journal re-runs the action and a crash after it resumes at the next phase. The phase gate makes
re-running safe — the wipe only executes below `Wiped`.

---

## Four adversarial passes

This is the part worth knowing. Four independent verifiers ran against the work. **Each found real
defects, including in the previous round's fixes.** Several fixes exist only because a verifier ran
an executed probe that broke my earlier one. The drills didn't just pass — they were repeatedly
proven insufficient and rebuilt.

Defects found and closed:

1. **Catch-up bound, twice wrong.** First positional (`take(hwm)`) — but the durable outbox orders
   `committed_live_rows` by `(aggregate, seq)`, aggregate-lexicographic, while the count is over the
   whole live set, so a positional take selected an arbitrary mix and dropped pre-fence events on
   high-sorting aggregates. Then `recorded_at` — an unvalidated `String` compared
   lexicographically, where `"…T10:00:00Z" > "…T10:00:00.500Z"` is `true`, and stamped from the
   *producer's* clock at transaction open rather than by the store at commit. **Now a per-aggregate
   `seq` watermark** — store-assigned inside the commit tx, integer, monotone. Absent aggregate is
   skipped wholly via explicit `Option` (`seq` is 0-based; `unwrap_or(0)` would admit first events).
2. **Fence token checked before the destructive act, enforced after.** A stalled holder could wake
   after losing its lease and wipe the replacement's work. Destructive phases now re-assert the
   lease through the journal CAS first. *Residual: narrows, does not close — see below.*
3. **Erase could resurrect erased personal data behind a success receipt.** Erase drives `index()`
   directly, bypassing the intake fence; mid-rebuild `locate_subject` found 0 docs and
   `erase_subject` returned `docs_purged: 0, zero_orphan_embedding: true`. Now refuses loudly.
   `erase_tenant` could not refuse at all (returned `EraseOutcome`, not `Result`) — now can.
4. **Verification was tautological, then still incomplete.** `ExpectedCorpus::from_index` read the
   expectation from the index it verified (`x == x`). Replaced with owner-truth
   (`ReplayOutcome::replayed_subjects` ∪ catch-up subjects). Then a probe showed empty owner truth
   still passed over a wiped index; then a further probe showed *partial* loss passed too — missing
   corpora leave the index AND the expectation together, so every leg balances. **Now anchored on
   `pre_wipe_docs`** with an explicit `acknowledged_shrink` budget.
5. **Blob aggregate key collision.** `(repo="a", ref="b/c")` and `(repo="a/b", ref="c")` produced an
   identical prefix, so one ref's reload silently deleted the other's corpus. Components are now
   `SubjectComponent`-encoded.
6. **Disclosure.** `QueryError::TenantMismatch` named both tenants; the indexer's `Malformed` errors
   quoted the artifact ref (a blob ref embeds repo name + file path) into the dead-letter store.
   Both scrubbed, on the replay leg as well as catch-up.

---

## Not wired — read this before trusting it

**`RebuildCoordinator` has no production caller.** Neither do `for_service`,
`enumerate_canonical_truth`, `load_canonical_blob_truth`, `from_replay`, or `abandon`. Every
reference outside the defining module is a test.

The root cause is pre-existing: `search_app_spec` still ships `consumers: Vec::new()` (the SRCH-P06
floor), so there is no production indexer to attach a gate to. `IncrementalIndexer::for_service`
takes the gate as a **required** argument and is the hook that wiring must use when it lands —
because `with_rebuild_gate` is a builder a composition root can forget, which is exactly how a fence
ships as a capability nobody calls.

**Consequence:** requirement "reads fail-empty during rebuild" is a capability, not a behaviour.
Nothing is regressed — the machinery is unreachable — but nothing is live either.

### To wire it

1. Build the durable journal: `PgRebuildJournal::new(pool, rt)`.
2. Construct the indexer via `IncrementalIndexer::for_service(specs, fetcher, embedder, gate)`.
3. Pass the same gate to `ScopedEngine::with_rebuild_gate`, `ResultCache::with_rebuild_gate`, and
   `SearchEraseHolder::with_rebuild_gate`.
4. Populate Git blob truth: `CodeProjectionEmitter::enumerate_canonical_truth` →
   `GitReindexSource::load_canonical_blob_truth`, per indexed ref.
5. Register the indexer as the service consumer (SRCH-P06).

---

## Residual risks

| risk | severity | note |
|---|---|---|
| Lease fence has a residual window | MEDIUM | The CAS precedes the destructive act, but the index wipe is in-process and the journal is external. Closing it properly needs the index write path to carry the fence epoch (a generation stamp rejected if stale). |
| Dead-lettered event between fence and catch-up | LOW | `committed_live_rows` filters rows that exhausted their publish budget. Such a row is counted in the watermark and absent at catch-up — never applied, never redelivered. Silent. |
| `for_service` is convention, not enforcement | LOW | `new` is still `pub` with ~30 callers across 12 crates. Real enforcement needs `new` made `pub(crate)` or the gate non-optional on the type. |
| `abandon` reopens reads over a partial corpus | BY DESIGN | Documented contract. Only defensible when the index is about to be destroyed anyway (tenant decommission) or immediately rebuilt. |
| `acknowledged_shrink` is operator-supplied | BY DESIGN | A rebuild legitimately shrinks the corpus, so the reduction must be stated. A wrong budget weakens the check — it cannot be inferred. |
| Migration `0011` edited in place | LOW | Never shipped (introduced on this branch), so in-place edit is correct. Any environment that applied an earlier revision must drop the table. |

---

## Verification

```bash
cargo test -p myelin-search -p myelin-git
cargo clippy -p myelin-search -p myelin-git --all-targets -- -D warnings
cargo run -p myelin-lints --bin lint-gate

# live isolated Postgres
fed isolate enable && fed start postgres
DATABASE_URL="postgres://myelin_admin:myelin_dev_pw@localhost:<port>/myelin" \
  cargo test -p myelin-search --features integration
```

- 25 drills in `tests/drill_srch_canonical_rebuild.rs`; 5 in
  `myelin-git/tests/drills_git_canonical_blob_truth.rs`.
- Architecture lint gate: **zero violations in the 22 files this branch touches**. The gate is red on
  `main` for unrelated reasons — `crates/myelin-git/src/pg_pr_store.rs` (13 `tenant-predicate` + 1
  `residency-pin`), another agent's file, failing since 2026-07-18.
- Workspace: one failing target, `myelin-ci-controlplane --test drills_ci_p17_reserve_settle_parity`,
  which fails identically at baseline.

---

## Files changed

**New**
```
crates/myelin-search/src/canonical.rs
crates/myelin-search/src/rebuild.rs
crates/myelin-search/src/rebuild_durable.rs
crates/myelin-search/tests/drill_srch_canonical_rebuild.rs
crates/myelin-search/tests/integration_srch_rebuild_journal.rs
crates/myelin-git/tests/drills_git_canonical_blob_truth.rs
docs/search-canonical-rebuild.md
```

**Modified**
```
crates/myelin-search/src/{cache,engine,indexer,lib,pipeline,reindex,shell,vector,erase}.rs
crates/myelin-search/Cargo.toml                     # sqlx non-optional (CT-004a posture)
crates/myelin-search/tests/{cdc_cache_4_10_3_4,cdc_query_pipeline_6_1,drill_srch_d3_cross_tenant}.rs
crates/myelin-git/src/{code_projection,events,replay}.rs
crates/myelin-git/tests/e2e_git_p287_code_projection_emit.rs
```

Untouched, as required: `myelin-outbox-publisher/**`, `service-federation.yaml`,
`myelin-events/src/nats.rs`, `myelin-storage/src/{elected_relay,provider}.rs`,
`myelin-git/src/pg_pr_store.rs`.

---

## Recommendation

Land `23fdddd` (the tombstone fix) on its own — it is independently verified across all four passes
and fixes a live bug where deleted and restricted blobs stayed queryable.

Treat the rest as reviewable scaffolding. It is drilled hard, but it is unreachable until the
composition roots wire it, and the wiring step is where the remaining risk concentrates.
