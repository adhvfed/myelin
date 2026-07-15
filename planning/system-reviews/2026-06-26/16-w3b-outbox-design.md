# W3b Design — OutboxStore retirement (SI-007): durable transactional outbox as the production default

Date: 2026-07-15. Input to ledger 13 W3b / ledger 14 R1. Scanner target: `no-in-memory-durable-store`
`OutboxStore` (`crates/myelin-events/src/outbox.rs:230`). Produced by a Fable-tier design pass
(read-only); this document is the W3b execution contract.

## Grounding corrections to the ledger-14 problem statement (code wins over docs)

1. **`myelin-identity-service` is NOT a DAG sink.** It already depends on `myelin-storage`
   (W2 shipped `TupleStore::with_pg(outbox, DurableTupleBacking, rt)` at `tuple_store.rs:338` and
   `StoreBackedCheck::with_pg` at `lib.rs:655`). The only true sink is **`myelin-events` itself**
   (frozen graph `myelin-substrate/src/crate_graph.rs:101`: Events → {Tenancy, Identity};
   Storage → Events, so events→storage is a cycle). The architectural blocker shrinks to one crate,
   and the repo already contains the exact inversion pattern for it.
2. **The W3 DedupLedger flip (`bcdf2cb`) is the proven template**: sync `DurableDedup` trait defined
   LOW in events; `DedupBackend` enum with `#[cfg(any(test, feature="test-support"))] Memory` +
   production `Durable(Arc<dyn …>)`; PG impl in `myelin-storage` bridging sync→async. `OutboxStore`
   takes the identical shape ⇒ the ~300 usage-by-reference sites across 15 crates need **zero
   signature churn**; only ~10 construction sites and events-internal relay mechanics change.
3. **"Thread a PG tx through every emit site" is mostly moot** — almost no subsystem has durable
   state to co-commit WITH. Issues stages description strings; flow's journal/run store are
   in-memory (not baseline entries); git's ref state is on-disk git with the GT-003 reconciler
   reading `committed_rows()`. Real durable-PG-state crates today: **chat** (already co-commits via
   `PgRelay::co_commit_in_tx`, `myelin-chat/src/store/pg.rs:306` — the precedent) and **identity**
   (durable tuple write then NON-atomic in-memory emit — the one genuine BUS-2 re-point; fails
   BUS-2 today on the durable path). For everyone else, "durable emit" = staged rows commit in one
   PG tx of their own; state-tx threading becomes real when each subsystem's durable state lands.

## Production reference-site map (summary)

- **Owning crate `myelin-events`** (the sink): `outbox.rs:230` struct + `OutboxTransaction`
  (`:386`, staging buffer; `commit()` `:429`); `relay.rs:349+` in-process `Relay<T: BusTransport>`
  claim/mark-sent/dead-letter/GC over `Inner`; `holder.rs` erase tombstones; `reindex.rs`;
  `reerase.rs`; `telemetry.rs` reads.
- **Genuine co-commit re-point**: identity `tuple_store.rs:241/317–347`, `lib.rs:655`,
  `src/bin/mr009_kill9_writer.rs:261`.
- **Role-struct absorbs with no signature change**: flow (`wfctx.rs:257` `WfCtx::begin`,
  `engine.rs` drive, executor/job/budget/merge_queue/remint/ci_pipeline/approval/maintenance;
  constructions `app.rs:159`, `dogfood.rs:223,413`, `restore_verify.rs:139`), git
  (`receive_pack.rs:705–799` `RefStore{outbox}`, `code_projection.rs`, `reconcile.rs:25–92` boot
  recovery), issues (`write_path.rs`, `reorder.rs`, `import.rs`), notif (`router.rs` construction
  `lib.rs:530`), knowledge (construction `lib.rs:605`), search (`reindex.rs`, `restore_verify.rs`,
  e2e_wedge constructs), refs-service (`reindex.rs`, `reindex_at_scale.rs:338`), storage
  (`coloc.rs:157` constructs — its `OltpPool` is a W7 blind-spot, not this wave; `olap_feed.rs`),
  substrate (`serve.rs:75–111` `OutboxSpec::default_inproc` constructs at `:97`; `ServeHandle`),
  edge (`git_durable.rs:90` `DurableGitBackend::rooted` constructs), agent-service
  (`skeleton.rs:372` reference-only), ci-controlplane (e2e_wedge harness).
- **Production binaries on the in-process floor**: flow/notif/knowledge/issues/search/identity
  `main.rs` shims (AppSpec does `OutboxStore::new()` + `InProcessBus`), edge `main.rs`.

## Decision — Design 1: backend role-struct (DedupLedger pattern, extended to a transactional surface)

`OutboxStore { backend: OutboxBackend }` with `enum OutboxBackend { #[cfg(any(test,
feature="test-support"))] Memory(Arc<Mutex<Inner>>), Durable(Arc<dyn DurableOutboxBacking>) }`.

- **Trait in `myelin-events`** (sync, like `DurableDedup`): `commit_staged(Vec<OutboxRow>)` — all
  staged rows in ONE PG tx (seq per-aggregate inside the tx) = `OutboxTransaction::commit`'s
  durable arm, emit-iff-committed exact; reads (`outbox_depth`, `dead_letter_count`,
  `oldest_unsent_recorded_at`, `committed_count`, `row`, `committed_rows`, `dead_letters`); and
  `drain_once(&dyn BusTransport, batch) -> DrainReport` — a SINGLE composite verb (do NOT decompose
  claim/mark, which would force a claimed-column rebuild of `FOR UPDATE SKIP LOCKED`).
- **Impl in `myelin-storage/src/outbox_durable.rs`** over `PgPool + Handle`, delegating to the
  EXISTING `PgRelay` (`pgrelay.rs:83` `co_commit_in_tx`, `:168` `relay_once`); extend `PgRelay`
  with per-row `attempts` increment + dead-letter marking on publish failure (the one genuine gap —
  parity with `MAX_PUBLISH_ATTEMPTS`/`DeadLetterAlert`).
- **Identity hybrid** (BUS-2 exact): `DurableTupleBacking` write path takes the envelope and calls
  `co_commit_in_tx` inside its existing tuple tx (both in myelin-storage; zero DAG issues) — the
  chat precedent applied at the backing layer.
- **In-process floor**: memory store + memory relay drain become test/dev-only; production `serve`
  holds a DURABLE OutboxStore drained by PgRelay to the configured `BusTransport` — `InProcessBus`
  remains the default transport (not a scanner entry; NATS-by-default stays the
  EventsRuntime/integration track). Events survive restart.
- **Rejected**: Design 2 (emit-port trait at every call site — re-types hundreds of signatures,
  converges on Design 1's trait surface with maximal churn); Design 3 (callers own emission —
  breaks frozen contract-2.2 `OutboxTx::emit` causality derivation, destroys emit-iff-committed
  structure, contradicts no-raw-publish doctrine).
- **Known risks**: durable-vs-memory commit semantics parity (dup event_id `ON CONFLICT DO NOTHING`
  vs error; SQL-allocated seq) — needs a CDC parity suite run against BOTH backends;
  sync-over-async bridge on hot emit paths (same bridge already carries dedup marks + NATS puts).

## Execution steps (each = one buildable/verifiable prompt; strict order; 2/3 parallel after 1)

- **W3b.1** role-struct + trait in events only; Memory UN-gated this step (scanner provably
  neutral, 12); dispatching `commit`/`drain`; admit fixture for a Durable-only store.
- **W3b.2** storage backing + PgRelay attempts/dead-letter + read SELECTs; integration: commit
  atomicity (abort inserts nothing), per-aggregate seq gap-free under concurrent committers (EB-03
  re-run durably), memory/durable CDC parity suite, drain + crash-window re-publish.
- **W3b.3** identity co-commit re-point; `with_pg`/`with_pg_minter`/`StoreBackedCheck::with_pg`
  drop the OutboxStore param on the durable path; update `mr009_kill9_writer`; proof: tuple row +
  outbox row commit/abort together; kill-9 no-ghost/no-lost.
- **W3b.4 ⚠ RISKIEST** composition roots + in-process floor re-shape: `OutboxSpec::durable`;
  `default_inproc` + memory-relay lifecycle behind test-support; app-spec outbox injection
  (notif/knowledge/flow/issues/search/identity spec builders gain an `outbox` param); all service
  `main.rs` shims construct `SubstrateProvider` from env → `migrate_foundation` →
  `OutboxStore::durable(…)`, **fail LOUD on missing durable config**; edge
  `DurableGitBackend::rooted` injected; `coloc.rs` injected. Verifier must re-run
  `crate_graph_acyclic` + `cargo tree -e no-dev` audit (test-support back-edge leak risk).
- **W3b.5** harness-module gating (chat e2e_wedge/e2e_dsar, notif/knowledge/search/ci-cp e2e_wedge,
  storage e2e3_reindex_parity, flow restore_verify+dogfood, refs-service reindex_at_scale,
  `restore_committed_row_for_test`) behind test-support with self-dev-dep.
- **W3b.6 THE FLIP** (scanner → OutboxStore gone): gate `Memory` variant, `Inner`,
  `new`/`Default`, memory relay mechanics; full gates + kill-9 emit drill (extend
  mr009_kill9_writer) proving committed events survive restart.

## Out of scope (named floors, not silent skips)

NATS-by-default transport (EventsRuntime stays integration-gated); durable state for flow
RunStore/WfJournal/TimerStore, issues typed core, notif inbox projection, git PR store,
knowledge/search projections (their own durability tracks); `OltpPool` permit model (W7 SI-021);
consumer same-tx dedup atomicity (MR-023b floor); W5/W6/W7 waves; residency_drill; chat's
in-memory message tier (only its harness gating is in W3b.5).
