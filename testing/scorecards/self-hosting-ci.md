# Myelin self-hosting CI graph — the dogfood loop (P-507 / P-S37 + P-508 / P-CP-23 + P-509 / CI-P35, M6)

Run date: 2026-06-26

The substrate ratchet (the twelve architecture lints + the contract-coverage scanner + the mandatory-core cargo-mutants mutation gate) runs as Myelin CI jobs on Myelin's OWN commit, the harness drives the substrate's surge/restore/migration drills (SUB-D3/D6/D10), the Tenancy dogfood band (the two Tenancy lints + self-host residency_verify + truth-up), and the CI dogfood band (the `ci.pipeline` body's determinism/crash-recovery + the Git↔CI check seam + CI's E2E flagship + the CI switch test + the CI truth-up pass) — the dogfood loop is live and now carries the CI done-bar. The gate is GREEN iff every job below passed; a single red job reds the gate (the ratchet rejects on Myelin's own work).

| Job | Verdict | Proof / reason |
|---|---|---|
| `lints` | PASS | [2026-06-26] PASS `cargo run -p myelin-lints --bin lint-gate` (627 files, 0 violations) |
| `lints-fixtures` | PASS | [2026-06-26] PASS `cargo test -p myelin-lints` |
| `contract-coverage` | PASS | [2026-06-26] PASS `cargo run -p myelin-lints --bin contract-coverage` |
| `contract-coverage-selftest` | PASS | [2026-06-26] PASS `cargo test -p myelin-lints --test contract_coverage_gate` |
| `mutation-gate` | PASS | [2026-06-26] PASS `cargo mutants` (mandatory-core surface, .cargo/mutants.toml) |
| `SUB-D3` | PASS | [2026-06-26] PASS `cargo test -p myelin-substrate --test drill_sub_d3_surge_family` |
| `SUB-D6` | PASS | [2026-06-26] PASS `cargo test -p myelin-substrate --test drill_sub_d6_restore_verify` |
| `SUB-D10` | PASS | [2026-06-26] PASS `cargo test -p myelin-substrate --test drill_sub_d10_migration_under_load` |
| `tenancy-lints` | PASS | [2026-06-26] PASS `cargo test -p myelin-lints --test tenancy_lints --test tenancy_control_plane_lints` |
| `CP-D23-dogfood` | PASS | [2026-06-26] PASS `cargo test -p myelin-control-plane --test cp_d23_dogfood_self_host_drill` |
| `ci-pipeline-determinism` | PASS | [2026-06-26] PASS `cargo test -p myelin-ci-controlplane --test drills_ci_p15_ci_pipeline --test drills_ci_p16_effectively_once` |
| `ci-check-seam` | PASS | [2026-06-26] PASS `cargo test -p myelin-ci-controlplane --test drills_ci_p19_seam_gate` |
| `ci-e2e-flagship` | PASS | [2026-06-26] PASS `cargo test -p myelin-ci-controlplane --test drill_ci_p34_e2e2_flagship` |
| `CI-P35-dogfood` | PASS | [2026-06-26] PASS `cargo test -p myelin-ci-controlplane --test ci_p35_dogfood_switch_test_drill` (switch test driven + measured; truth-up 0 red CI gates) |

**GATE: GREEN** — the self-hosting CI graph is green on Myelin's own commit (SUB-M6 / CP-M6 / CI-M6 — the CI done-bar).

## CI switch test (the Git OQ-12 / CI switch test — driven, measured)

Driven against the real `myelin ci` run/log/deploy view surface (arch 04 §2) vs the GitHub Actions anchor (EI-01 §4 — actually try it, not a feature list). Every capability a migrating GitHub-Actions user relies on is reached by driving the corresponding `myelin ci` verb/view; the representative run/log view render latency is MEASURED within the `[ci_switch_test] render_budget_us` budget read from the FROZEN `thresholds.toml` (never hardcoded, never weakened). **Verdict: PASS — 0 walls; a GitHub-Actions user could move without hitting a wall the old tool didn't have.**

Deferred-by-design named floors RECORDED (not walls — the anchor lacks them too / they await demand): `myelin ci local` (laptop execution; GitHub Actions has no first-party local runner either); the CI registry product; cross-cell-spanning pipelines (until OQ-I demand). The ONE legitimate remaining infra floor is the world-scale 30× fleet-hardware load drill (CI-P30 runs the moderate single-cell variant).
