# Substrate truth-up pass — every PROVEN substrate row rests on a dated green artifact (P-S38 / P-510, SUB-M6)

Run date: 2026-06-26

The code wins over the docs (EI-01 §1): each substrate PROVEN row below names its DATED green artifact (the `cargo test`/`cargo run` target that emits it), not a doc claim. The pass is GREEN iff EVERY row rests on a dated artifact — the gate invariant holds end-to-end (no earlier substrate gate is red).

Rendered by `myelin_harness::dogfood::SubstrateTruthUpPass::render_markdown` over the FROZEN
`proven_substrate_rows` set; the committed test `dogfood_tests::truth_up_pass_is_green_every_substrate_proven_row_is_dated`
asserts 0 claimed-not-proven rows (the gate the CI must not swallow).

| Gate / drill | Dated artifact | Proof command |
|---|---|---|
| `SUB-D1` — kill service between commit & publish → 0 ghost / 0 lost (outbox + dedup) | [2026-06-26] PROVEN | `cargo test -p myelin-events --test drills_sub_d1_bus_d4` |
| `SUB-D2` — drop broker mid-stream → 0 lost across reconnect; slow subject no HoL stall | [2026-06-26] PROVEN | `cargo test -p myelin-events --test drills_sub_d2_consumer` |
| `BUS-D4` — crash producer between state-commit and publish → emit-iff-committed (co-commit) | [2026-06-26] PROVEN | `cargo test -p myelin-storage --test sub_d1_bus_d4_coloc_drill` |
| `SUB-D5` — trip a downstream breaker → fail fast, honour Retry-After, no amplification | [2026-06-26] PROVEN | `cargo test -p myelin-client --test sub_d5_retry_storm` |
| `SUB-D7` — cross-tenant read via path≠token → 0 misroute; tenant-predicate lint catches | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_d7_idor` |
| `SUB-D8` — agent→agent loop → depth ceiling + shared-root tripwire + bounded pool halt | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_d8_causal_loop` |
| `SUB-D9` — kill a critical dependency → not-ready + sheds; no liveness restart-storm | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_d9_liveness_readiness` |
| `lints` — the twelve architecture lints — each red fixture rejects + green admits | [2026-06-26] PROVEN | `cargo run -p myelin-lints --bin lint-gate` |
| `contract-coverage` — the contract-coverage scanner — no falsely-claimed/dropped/un-named row | [2026-06-26] PROVEN | `cargo run -p myelin-lints --bin contract-coverage` |
| `harness-self-test` — the harness injects a fault and reads one telemetry assertion green | [2026-06-26] PROVEN | `cargo test -p myelin-harness drills::tests::harness_self_test` |
| `SUB-D4` — Id-hiccup → already-authenticated survives within W; revoked denied (fail-static) | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_d4_fail_static` |
| `SUB-D11-slow` — firehose hot-stream slow consumer → frame-cap + drop-to-resync, no unbounded buffer | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_d11_firehose_slow_consumer` |
| `SUB-D11-budgets` — firehose frame-budget + scope-selector → per-surface shed budget bounds frames | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_d11_firehose_frame_budgets` |
| `SUB-D11-storm` — firehose backpressure under connection-storm → bounded everything, human lane holds | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_d11_connection_storm` |
| `SUB-D3` — 30× surge family → human lane within budget, agent lane sheds, cross-tenant impact 0 | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_d3_surge_family` |
| `SUB-D10` — online-migration-under-load → lock-wait p99 within budget, 0 errored writes, 0 downtime | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_d10_migration_under_load` |
| `SUB-D6/STOR-D2-cell` — restore-verify re-confirmed at cell scale under world-scale load → RPO/RTO held | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_d6_restore_verify_cell_scale` |
| `BUS-D7` — 30× agent publish surge → human lane holds, agent sheds, other tenants unaffected | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drills_bus_d7_agent_surge` |
| `P-S33` — tuned per-surface shed budgets → human-lane starvation 0 at the measured numbers | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_p_s33_tuned_shed_budgets` |
| `P-S36` — tuned resilient-client per-target values → each target within its measured budget | [2026-06-26] PROVEN | `cargo test -p myelin-substrate --test drill_sub_p_s36_resilient_target_tuning` |

**TRUTH-UP: GREEN** — 20 PROVEN substrate rows, 0 claimed-not-proven; the gate invariant holds end-to-end (no earlier substrate gate is red).

**Named floor (EI-01 §1):** the world-scale 30× FLEET-hardware load drill (SUB-D3 at true multi-box fleet scale) is the ONE legitimate remaining infra floor — the single-box SCALED surge runs green in the self-hosting CI graph; the fleet corpus is named, not claimed (it is not a row that reds this pass — the substrate is *correct*; the fleet proof is *load-hardened-at-scale*).

## The every-incident-adds-a-drill loop on Myelin's own tracker (live)

The substrate's T-3 loop now runs ON THE PLATFORM (`myelin_harness::dogfood::SubstrateIncidentLoop`): a
simulated substrate incident (an outbox relay stall) files a Myelin **issue** ref on the platform's own
tracker (`myelin-issues#SUB-INC-1`) AND registers a reproducing **drill** (`repro-outbox-relay-stall`) into
the substrate's REAL `DrillRegistry` via the P-S04 `register_drill` hook. The loop is **live**, not a ref
check: the committed test
`dogfood_tests::incident_loop_files_a_myelin_issue_and_a_reproducing_drill_on_a_simulated_incident` re-runs
the repro and asserts it reads GREEN (the regression stays fixed); an incident missing either leg is a LOUD
gap (`unguarded_incidents`), and a regressed repro is a LOUD red (`red_repros`) — never a silent skip
(EI-01 §3/§5).
