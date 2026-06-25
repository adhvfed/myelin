# Myelin self-hosting CI graph — the dogfood loop (P-507 / P-S37, SUB-M6)

Run date: 2026-06-25

The substrate ratchet (the twelve architecture lints + the contract-coverage scanner + the mandatory-core cargo-mutants mutation gate) runs as Myelin CI jobs on Myelin's OWN commit, and the harness drives the substrate's surge/restore/migration drills (SUB-D3/D6/D10) — the dogfood loop is live. The gate is GREEN iff every job below passed; a single red job reds the gate (the ratchet rejects on Myelin's own work).

| Job | Verdict | Proof / reason |
|---|---|---|
| `lints` | PASS | [2026-06-25] PASS `cargo run -p myelin-lints --bin lint-gate` |
| `lints-fixtures` | PASS | [2026-06-25] PASS `cargo test -p myelin-lints` |
| `contract-coverage` | PASS | [2026-06-25] PASS `cargo run -p myelin-lints --bin contract-coverage` |
| `contract-coverage-selftest` | PASS | [2026-06-25] PASS `cargo test -p myelin-lints --test contract_coverage_gate` |
| `mutation-gate` | PASS | [2026-06-25] PASS `cargo mutants` |
| `SUB-D3` | PASS | [2026-06-25] PASS `cargo test -p myelin-substrate --test drill_sub_d3_surge_family` |
| `SUB-D6` | PASS | [2026-06-25] PASS `cargo test -p myelin-substrate --test drill_sub_d6_restore_verify` |
| `SUB-D10` | PASS | [2026-06-25] PASS `cargo test -p myelin-substrate --test drill_sub_d10_migration_under_load` |

**GATE: GREEN** — the self-hosting CI graph is green on Myelin's own commit (SUB-M6).
