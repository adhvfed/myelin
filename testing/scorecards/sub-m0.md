# M0 exit-gate scorecard (SUB-D1/D2/BUS-D4/D5/D7/D8/D9 + 12 lints + harness self-test)

> Generated: 2026-06-19. The build-layer realisation of the master M0→M1 gate invariant (master-sequencing §2/§4, EI-01 §2): no later-band prompt runs over a red earlier gate. Each row is a dated green artifact read off the per-feature drill (this scorecard WIRES the drills, it does not re-implement them). A single RED row blocks M1 and is recorded honestly as claimed-not-proven, never edited green (EI-01 §3 / roadmap §5).

**Gate verdict: GREEN — M1 may start**

| Gate | Title | Verdict | Date | Permanent | Proof / reason |
|---|---|---|---|---|---|
| SUB-D1 | kill service between commit & publish → 0 ghost / 0 lost (outbox + dedup) | PASS | 2026-06-19 | re-run-forever | [2026-06-19] PASS  `cargo test -p myelin-events --test drills_sub_d1_bus_d4` |
| SUB-D2 | drop broker mid-stream → 0 lost across reconnect; slow subject no HoL stall | PASS | 2026-06-19 | re-run-forever | [2026-06-19] PASS  `cargo test -p myelin-events --test drills_sub_d2_consumer` |
| BUS-D4 | crash producer between state-commit and publish → emit-iff-committed | PASS | 2026-06-19 | re-run-forever | [2026-06-19] PASS  `cargo test -p myelin-storage --test sub_d1_bus_d4_coloc_drill` |
| SUB-D5 | trip a downstream breaker → fail fast, honour Retry-After, no amplification | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-client --test sub_d5_retry_storm` |
| SUB-D7 | cross-tenant read via path≠token → 0 misroute; tenant-predicate lint catches | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-substrate --test drill_sub_d7_idor` |
| SUB-D8 | agent→agent loop → depth ceiling + shared-root tripwire + bounded pool halt | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-substrate --test drill_sub_d8_causal_loop` |
| SUB-D9 | kill a critical dependency → not-ready + sheds; no liveness restart-storm | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-substrate --test drill_sub_d9_liveness_readiness` |
| lints | the twelve architecture lints — each red fixture rejects + green admits | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo run -p myelin-lints --bin lint-gate` |
| lint-fixtures | the lint fixture matrix + the CI-gate self-test (red fixture ⇒ non-zero) | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-lints` |
| contract-coverage | the contract-coverage scanner — no falsely-claimed/dropped/un-named row | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo run -p myelin-lints --bin contract-coverage` |
| harness-self-test | the harness injects a fault and reads one telemetry assertion green | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-harness drills::tests::harness_self_test` |

**Permanent gates (re-run forever).** SUB-D1 / SUB-D2 / BUS-D4 re-run on every emit-path-touching change from M0 on (master-sequencing §1 item 6); a regression on any of them halts the run.
