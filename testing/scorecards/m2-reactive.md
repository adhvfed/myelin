# M2 (reactive shared layer) exit-gate scorecard (BUS-D1/D3/D6/D5/D8 + REF-CDC + SRCH-D1/D2/D3/D4/D7 + NOTIF-D1..D11+snooze + AG-D1/2/3/5/7/8/11 + AG-D4 (real-kernel escape, proven-on-real-hardware) + FLOW-D1/D3/D4/D5/D6/D7+mergeq + contract-coverage)

> Generated: 2026-06-21. The build-layer realisation of the master band gate invariant (master-sequencing §2/§4, EI-01 §2): no later-band prompt runs over a red earlier gate. Each row is a dated green artifact read off the per-feature drill (this scorecard WIRES the drills, it does not re-implement them). A single RED row blocks M3 and is recorded honestly as claimed-not-proven, never edited green (EI-01 §3 / roadmap §5).

**Gate verdict: GREEN — M3 may start**

| Gate | Title | Verdict | Date | Permanent | Proof / reason |
|---|---|---|---|---|---|
| BUS-D1 | dispatch reconnect → 0 lost across a broker drop on the reactive dispatch path | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-query --test drills_bus_d1_dispatch_reconnect` |
| BUS-D3 | dispatch replay → at-least-once redelivery is idempotent (no double-fire) | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-query --test drills_bus_d3_dispatch_replay` |
| BUS-D6 | dispatch loop guards → reactive automation cycle halts at the depth ceiling | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-query --test drills_bus_d6_dispatch_loop_guards` |
| BUS-D5 | reindex → a re-index pass is replay-safe; no stale/dup projection rows | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-events --test drills_bus_d5_reindex` |
| BUS-D8 | crypto-shred → erased-key payload is unrecoverable across the bus replay path | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-events --test drills_bus_d8_crypto_shred` |
| REF-CDC | ArtifactRef provider mints canonical URN / consumer parses + rejects display projections (CDC 5.1) | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-refs --test cdc_5_1_artifactref` |
| SRCH-D1 | zero-leak keystone → a confidential doc never appears in any unauthorized search result set | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-search --test drill_srch_d1_zero_escape_leak` |
| SRCH-D2 | no stale grant → revoke then re-query → the just-revoked doc is gone (watermark honoured) | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-search --test drill_srch_d2_no_stale_grant` |
| SRCH-D3 | cross-tenant → a path-spoofed query reads 0 cross-tenant documents | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-search --test drill_srch_d3_cross_tenant` |
| SRCH-D4 | erasure → an erased doc is purged from the index; re-query returns 0 hits | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-search --test drill_srch_d4_erasure` |
| SRCH-D7 | freshness → a just-indexed doc is visible within the freshness bound (no lost write) | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-search --test drill_srch_d7_freshness` |
| NOTIF-D1 | notif drill D1 — mention fan-out delivers exactly the addressed recipients | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-notif --test drill_notif_d1` |
| NOTIF-D2 | notif drill D2 — read-state fan-out / dedup is consistent across surfaces | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-notif --test drill_notif_d2` |
| NOTIF-D3 | notif drill D3 — preference gating: a muted channel delivers 0 | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-notif --test drill_notif_d3` |
| NOTIF-D4 | notif drill D4 — escalation honours the ladder without double-paging | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-notif --test drill_notif_d4` |
| NOTIF-D7 | notif drill D7 — cross-tenant: a notification never leaks across the tenant boundary | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-notif --test drill_notif_d7` |
| NOTIF-D8 | notif drill D8 — erasure: an erased subject's notifications are structurally purged | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-notif --test drill_notif_d8` |
| NOTIF-D9 | notif drill D9 — holder replay: redelivery after a crash is idempotent | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-notif --test drill_notif_d9` |
| NOTIF-D10 | notif drill D10 — delivery survives a consumer reconnect with 0 lost | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-notif --test drill_notif_d10` |
| NOTIF-D11 | notif drill D11 — inbox watch / list consistency under concurrent reads | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-notif --test drill_notif_d11` |
| NOTIF-snooze | notif snooze → a snoozed notification resurfaces exactly once at the wake time | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-notif --test drill_notif_snooze_resurface` |
| AG-D1/2/3 | plan-then-apply pipeline: schema→cap→delegation→tenant→budget→HITL→apply→meter (CDC 8.2) | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-agent-service --test cdc_8_2_apply_pipeline` |
| AG-D5-batch | per-effect HITL exactly-once (batch) — each effect gated by its own approval (CDC 8.2) | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-agent-service --test cdc_8_2_hitl_batch` |
| AG-D5-loop | per-effect HITL exactly-once (loop) — re-entry never double-applies an approved effect (CDC 8.2) | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-agent-service --test cdc_8_2_hitl_loop` |
| AG-D7 | agent loop guards → a self-invoking agent halts at the depth ceiling / shared-root tripwire | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-agent-service --test drills_ag_d7_loop_guards` |
| AG-D8 | per-run identity/skeleton → each run drives the chained substrate path under its own run token (CDC 8.5) | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-agent-service --test cdc_8_5_skeleton_loop` |
| AG-D11 | runaway self-limiter → a reserve/settle runaway agent is rate-limited and halts | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-agent-service --test drills_ag_d11_runaway_self_limiter` |
| AG-D4 | REAL-kernel escape gate → a real Firecracker microVM boots, runs the adversarial corpus, 0 escapes (proven-on-real-hardware) | PASS | 2026-06-21 | re-run-forever | [2026-06-21] PASS  `cargo test -p myelin-ci-sandbox --features integration --test escape_drill_test` |
| FLOW-D1 | durable replay → a workflow re-driven from its log lands on the same deterministic state | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-flow --test drills_flow_d1_replay` |
| FLOW-D3 | timer wheel → a durable timer fires exactly once at its deadline across a restart | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-flow --test drills_flow_d3_timer_wheel` |
| FLOW-D4-hitl | multiday HITL → a workflow parked on human approval resumes correctly days later | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-flow --test drills_flow_d4_multiday_hitl` |
| FLOW-D4-per-effect | per-effect durability → each effect is committed exactly once across replay | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-flow --test drills_flow_d4_per_effect` |
| FLOW-D5 | co-commit → state + emitted effect commit in the same transaction (emit-iff-committed) | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-flow --test drills_flow_d5_cocommit` |
| FLOW-D6 | reserve/settle → a reserved budget settles exactly once; a crash leaves no double-charge | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-flow --test drills_flow_d6_reserve_settle` |
| FLOW-D7 | loop safety → a self-scheduling workflow halts at the loop-safety ceiling | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-flow --test drills_flow_d7_loop_safety` |
| FLOW-mergeq | merge queue → serialized merge-queue admission commits in order without a lost update | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo test -p myelin-flow --test drills_flow_merge_queue` |
| contract-coverage | the contract-coverage scanner re-affirms the M2 CDC rows — no falsely-claimed/dropped row | PASS | 2026-06-21 | — | [2026-06-21] PASS  `cargo run -p myelin-lints --bin contract-coverage` |

**AG-D4 is PROVEN-ON-REAL-HARDWARE, NOT vacuous.** The escape gate runs `--features integration` with `MYELIN_REQUIRE_KVM=1` set by the scorecard runner: on a host without /dev/kvm or firecracker the drill HARD-FAILS (it does not skip), so this row only goes green when a real Firecracker microVM actually boots, runs the 11-attack adversarial corpus, and attests 0 escapes (a dated `target/ag-d4-attestation/<date>.json`). It is marked re-run-forever (EI-01 §2: RCE/sandbox-escape outranks every feature).

**AG-D4 — three NAMED residuals (proven-on-real-hardware is not absence-of-all-escapes):**
- (a) **one green run proves THIS config against THIS battery on THIS kernel** — continuous fuzzing + full CVE-corpus tracking + a pre-GA third-party pentest remain ongoing; a single green is necessary, not sufficient-forever.
- (b) **production must run on KVM-capable Scaleway hardware** (Elastic Metal / nested-virt) — an explicit infra requirement; on a non-KVM box this gate cannot be greened (the row hard-fails, never fakes green).
- (c) **single-box ≠ fleet** — multi-tenant density / blast-radius at scale still overlaps the unproven 30× world-scale LOAD floor below.

**The ONE true remaining floor (named, dated deferral — NOT a row that reds this gate):** the **world-scale 30× LOAD drill** needs real fleet hardware (a multi-node cluster), so it is deferred to **M5** (the FLOW-D8 / AG-D6 / NOTIF surge prompts). It is the only genuine remaining floor — everything else in M2 is proven with a dated artifact above. The deferral is visible, never invisible (EI-01 §1).

**Named residual (not a floor, run-when-available):** gVisor (`runsc`) as a SECOND escape-drill backend (CI-P28) — runsc is on PATH but running the corpus under it needs an OCI bundle + root/userns privileges this host lacks; the AG-D4 attestation records it as a NAMED parametrized residual, never faked green. Firecracker (the production default) IS the exercised gate backend.
