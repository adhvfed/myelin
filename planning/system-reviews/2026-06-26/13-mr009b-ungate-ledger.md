# Make-It-Real Ledger — MR-009b: un-gate the durable path to the production default (closes SI-022)

Date: 2026-07-03. Status: PLAN → EXECUTING. Chosen by the founder as the highest-leverage make-it-real step
after the spine + Git daily driver + sandbox floor. Flips the PRODUCTION default from the in-memory models to
the durable backings (real PG/S3/Valkey/NATS), moves in-memory to TEST-ONLY doubles, and drives the
production-graph absence scanner's `no-in-memory-durable-store` baseline from **17 → 0** (the `residency_drill`
structural-crypto entry is a separate CI/attestation-track survivor, out of scope). Subsumes CT-004b (the CI slice).

## The two load-bearing facts (from the grounding pass)
1. **The scanner is a TEXT scanner over type shape** (`myelin-lints/src/production_graph.rs`): a `*Store/*Registry/
   *Outbox/*Ledger` (or a `NAMED_DURABLE_HOLDERS` entry) fires if it holds an in-memory collection directly / via
   a type alias / via a delegated `Inner` / via a backend enum with a `Memory(...)` variant, AND carries no
   pool token. It recognizes ONLY literal `cfg(test)` — NOT `cfg(feature=...)`, and pass-1 backings aren't
   `in_test`-filtered. ⇒ merely feature-gating the `Memory` variant does NOT turn it green. An entry leaves the
   set only when the PRODUCTION-COMPILED type presents no in-memory collection/backing/enum.
2. **The DB-free-unit-test crux:** the in-memory doubles are consumed CROSS-CRATE in non-test lib code (~90
   `KmsEngine::new()` sites across nearly every crate), so they can't move to `#[cfg(test)]` (crate-local only).
   They must live behind a `test-support`/`memory` cargo feature that downstream crates enable as a
   dev-dependency — which REQUIRES a scanner enhancement (Wave 0) to treat `feature="test-support"` as a test gate.
   DB-free BUILD is safe to flip: zero compile-time `sqlx::query!` macros exist (all durable code is runtime
   `sqlx::query(&str)`), so making sqlx/tokio/aws/fred non-optional compiles with no DB at build time.

## Conventions
Same as prior ledgers: builder → orchestrator full gate → independent verifier (never the builder) → commit per
wave. The **objective proof** each wave: the scanner baseline SHRINKS by exactly the flipped holders AND
`cargo build --workspace` + `cargo test --workspace` STAY DB-FREE AND `--features integration` proves durable-
by-default on live PG. Anti-duplication grep opens each. Reuse the MR-007/008/009/022/023/024/025 + CT-004
durable impls — wire/re-point, don't rebuild (except Wave 6, which builds the missing durable backings).

## The wave ledger

| Wave | Title | Holders (baseline entries flipped) | Baseline | Kind |
|---|---|---|---|---|
| **W0** | **Scanner enhancement + `test-support` convention** — filter pass-1 backings by `!in_test`; strip test/feature-gated enum-variant lines; treat `cfg(feature="test-support")`/`any(test, test-support)` as a test gate; add admit+bite fixtures. Add the `test-support` (alias `memory`) feature to the spine crates. | none (enabler) | 17→17 (provably neutral) | mechanism |
| **W1** | **Storage foundation: durable deps non-optional** — move sqlx/tokio/myelin-config/aws-*/fred out of `optional`/`integration` into plain `[dependencies]`; `integration` becomes a test-selector. | none (compile root) | 17→17 | mechanical |
| **W2** | **Identity spine stores durable-default** — PrincipalStore (SI-018), TupleStore (SI-019), RevocationStore (SI-019/020): un-gate `Pg`/`with_pg`, gate `Memory`/`Inner` behind test-support, boot wires `SubstrateProvider`+`with_pg`. | 3 | 17→14 | wire |
| **W3** | **DedupLedger durable-default** (SI-023) — DONE (`bcdf2cb`), 14→13. Split from outbox (code-wins-over-docs: outbox is a larger separate problem). | 1 | 14→13 | wire |
| **W3b** | **OutboxStore retirement** (SI-007) — the in-memory-ness IS the struct (no backend enum); the durable counterpart `PgRelay::co_commit_in_tx` commits in the CALLER's tx, so this = RETIRE the in-process outbox floor from production + a per-subsystem durable-emit re-point (thread a PG tx through every emit site: events relay/reindex/telemetry/holder/reerase + issues/git/flow/storage/substrate + identity tuple_store) preserving BUS-2 emit-iff-committed. Higher-coupling; its own wave. | 1 | 13→12 | retire + re-point |
| **W4** | **Control-plane durable-default** — Registry (SI-011) → `DurablePlacementRegistry::with_pg`; MisrouteAudit (SI-028) → `record_misroute`; CP durable deps non-optional. | 2 | 12→10 | re-point |
| **W5** | **KMS durable-default (SI-006) — isolated, HIGH blast radius** — prod `KmsEngine` holds the durable software-sealed backing (`kms_durable::load_or_generate`, opaque field); the in-memory engine stays behind test-support for the ~90 downstream unit-test constructions; audit the ~90 sites to classify prod-root vs test-setup (genuine prod roots: substrate `serve.rs`, edge `main.rs`, each service boot). | 1 | 10→9 | reshape (careful) |
| **W6** | **Build-first-then-wire cluster** (durable impl does NOT exist yet — real net-new persistence): 6a identity PseudonymStore(S2)+PseudonymErasureLedger(10.8); 6b storage CostLedger(SI-021)+ErasureLedger(SI-036)+PostPitLedger(P-ST-14); 6c events BusErasureLedger(SI-039)+CellResolverRegistry(SI-052). Each: build `Durable*Backing`+migration+`with_pg`+live-PG test, THEN flip. | 7 | 9→2 | build + wire |
| **W7** | **Blob byte-durability + CI slice (CT-004b) + scanner blind-spots** — FsBlobStore(SI-014/15/29) → S3/git byte backing (P-ST-30 track); make CI deps non-optional + default the CT-004 durable SQL scheduler/metering (CT-004b) + widen the scanner scope to the CI crates; widen `NAMED_DURABLE_HOLDERS`/`DURABLE_ROLE_SUFFIXES` to the blind spots (Consumer SI-024, Firehose SI-037, InMemoryShredder SI-038, OltpPool SI-021, PlacementService SI-026), add-then-flip. | 1 (blob) + CI + blind spots | 2→0 (`no-in-memory-durable-store`) | flip + widen |

Residual survivor after W7: `residency_drill.rs:444` (`no-structural-crypto-in-prod`) — a CI runner-attestation
floor, a DIFFERENT scanner/subsystem, explicitly deferred to the CI/attestation track (NOT MR-009b).

## Hard ordering & top risks
Ordering: **W0 → W1 → {W2,W3,W4} (independent) → W5 (isolated) → W6 (per-owner) → W7**.
Top risks (ranked): (1) DB-free-unit-test preservation hinges on W0 (test-support + scanner) being right — if W0
is wrong, every later wave breaks unit tests or fails to go green; (2) KMS blast radius (~90 sites — a
misclassified prod root ships an in-memory KMS = silent key loss on restart); (3) the `default` build now needs
runtime DB config — boot shims must fail LOUD on missing durable config, and CI must run the docker stack;
(4) shared dev-DB integration flakiness — preserve per-test tenant/region partitioning + admin-pool cleanup;
(5) no circular-dep regression — `test-support` dev-dep back-edges must not leak into a runtime DAG
(`crate_graph.rs::substrate_is_root()` must still hold).
