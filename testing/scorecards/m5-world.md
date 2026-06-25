# M5 (world-scale hardening) exit-gate scorecard (F6 surge family (SUB-D3/ID-D9/BUS-D7/REF-D10/SRCH-D6/NOTIF-D5/AG-D6/FLOW-D8/GIT-D6/CI-D2/CHAT-D3/D4) + GIT-D4/D5 + KN-D1-re-green/KN-D8 + GA-D1/GA-D8/CP-D7/CP-D8 + E2E-1/E2E-2/E2E-3/E2E-4 + STOR-D2 (cell scale, permanent restore gate) + contract-coverage)

> Generated: 2026-06-25. The build-layer realisation of the master band gate invariant (master-sequencing §2/§4, EI-01 §2): no later-band prompt runs over a red earlier gate. Each row is a dated green artifact read off the per-feature drill (this scorecard WIRES the drills, it does not re-implement them). A single RED row blocks M6 and is recorded honestly as claimed-not-proven, never edited green (EI-01 §3 / roadmap §5).

**Gate verdict: GREEN — M6 may start**

| Gate | Title | Verdict | Date | Permanent | Proof / reason |
|---|---|---|---|---|---|
| SUB-D3 | F6 surge → substrate 30× surge family: human lane within budget, agent lane sheds, cross-tenant impact 0 | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-substrate --test drill_sub_d3_surge_family` |
| ID-D9 | F6 surge → Identity authz 30× surge: check/list path holds under load, agent sheds, cross-tenant 0 | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-identity-service --test drill_id_d9_authz_surge` |
| BUS-D7 | F6 surge → Bus agent 30× surge: reactive dispatch holds, agent lane sheds, no cross-tenant amplification | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-substrate --test drills_bus_d7_agent_surge` |
| REF-D10 | F6 surge → Reference Graph 30× surge: resolution holds within budget, agent sheds, cross-tenant 0 | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-refs-service --test ref_d10_surge_drill` |
| SRCH-D6 | F6 surge → Search 30× surge: query path within budget, agent sheds, 0 cross-tenant leak under load | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-search --test drill_srch_d6_surge` |
| NOTIF-D5 | F6 surge → Notifications 30× surge: fan-out within budget, agent sheds, cross-tenant impact 0 | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-notif --test drill_notif_d5` |
| AG-D6 | F6 surge → Agent dispatch 30× surge: human lane within budget, agent dispatch sheds, cross-tenant 0 | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-agent-service --test ag_d6_dispatch_surge_drill` |
| FLOW-D8 | F6 surge → Durable Workflow 30× surge: human lane within budget, agent lane sheds, cross-tenant 0 | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-flow --test drills_flow_d8_surge` |
| GIT-D6 | F6 surge → Git clone 30× surge: clone p99 held within budget, agent sheds, cross-tenant 0 | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-git --test drill_git_d6_clone_surge` |
| CI-D2 | F6 surge → CI 30× surge: pipeline admission within budget, agent lane sheds, cross-tenant 0 | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-ci-controlplane --test ci_d2_surge_drill` |
| CHAT-D3/D4 | F6 surge → Chat agent 30× surge: human lane within budget, agent lane sheds, cross-tenant 0 | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-chat-gateway --test drill_chat_d3_agent_surge` |
| GIT-D4 | monorepo ceiling → object-backed packs: large-monorepo ceiling documented + clone p99 held under object-backed packs | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-git --test drills_git_d4_object_backed_packs` |
| GIT-D5 | concurrent-merge linearizability under failover → ref-CAS linearizable, no split-brain, 0 lost merge | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-git --test drills_git_d5_concurrent_merge_linearizability` |
| KN-D1-re-green | Yrs CRDT promotion re-green → KN-D1 resume-cursor collab holds ACROSS the CRDT boundary (no gap / no double-apply) | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-knowledge --test drill_kn_p29_yrs_promotion` |
| KN-D8 | all-hands doc surge → thousands of concurrent editors on one doc → the per-doc caps hold (no runaway / no lost edit) | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-knowledge --test drill_kn_d8_allhands_surge` |
| GA-D1 | full H1–H18 DSR fan-out at cell scale → an erasure reaches every holder family, 0 holders missed, per-holder receipt | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-gdpr-service --test ga_d1_full_fanout_cell_scale` |
| GA-D8 | multi-cell DSR fan-out → an erasure fans out across every member cell, per-cell receipt set complete, 0 cell missed | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-gdpr-service --test ga_d8_multi_cell_fanout` |
| CP-D7 | cell→cell live migration → a tenant migrates between cells with 0 loss (no lost/ghost write across the cutover) | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-control-plane --test cp_d7_live_migration_drill` |
| CP-D8 | cross-cell PII-free bridge → a cross-cell reference resolves via the PII-free CrossCellPointer bridge; 0 PII crosses | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-control-plane --test cp_d8_cross_cell_bridge_drill` |
| E2E-2 | agent-native flagship → CI-fail → triage agent → issue → chat → fix-PR drives end-to-end with its named green artifact | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-agent-service --test drills_ag_p24_e2e2_flagship` |
| E2E-4 | DSAR fan-out flagship → 0 holders missed; 0 recoverable PII incl. vectors incl. backups; certificate sealed (named green artifact) | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-gdpr-service --test e2e_4_dsar_fanout_flagship` |
| E2E-3 | spec-to-ship / reindex-parity (storage half) → a cold re-index pass equals the live projection; audit tamper detected (named artifact) | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-storage --test e2e3_reindex_parity_drill` |
| E2E-1 | PR context pane (git slice) → the whole-system PR-context wedge drives end-to-end with its named green artifact | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo test -p myelin-git --test e2e_wedge_git_p34` |
| STOR-D2-cell | restore-verify at CELL SCALE under world-scale load → RPO/RTO within bound, 0 loss per cell (the permanent restore gate) | PASS | 2026-06-25 | re-run-forever | [2026-06-25] PASS  `cargo test -p myelin-storage --test stor_d2_d8_cell_scale_under_world_scale_load_drill` |
| contract-coverage | the contract-coverage scanner re-affirms the M5 CDC rows — no falsely-claimed/dropped row | PASS | 2026-06-25 | — | [2026-06-25] PASS  `cargo run -p myelin-lints --bin contract-coverage` |

**The world-scale 30× surge family is proven here as a SINGLE-BOX SCALED drill** (the shed-order / lane-priority / cross-tenant-isolation LOGIC is exercised and green). The **true multi-node FLEET proof** (30× fan-out across a real multi-box cluster, measured blast-radius/density at fleet scale) remains the ONE genuine named floor — it needs real fleet hardware this dev host does not have. The drill proves the mechanism; the fleet-scale residual is NAMED, never faked green (EI-01 §1).

**STOR-D2 at cell scale** is the permanent restore gate (a backup never restored is not a backup, EI-01 §3) — re-run-forever.

**Carried-forward floor (M7):** the AG-D4 sandbox isolation boundary is proven-on-real-hardware, but a real `JobSpec.command` does not yet flow through the PRODUCTION `launch()` on either backend (Firecracker prod boots `init=/bin/true`; gVisor prod runs only `runsc --version`) — production exec is filled by M7 P-544/P-545, named here, not a row that reds this M5 gate.

**Measured-trigger-gated floors named in M5 (trigger not fired):** Chat ScyllaDB hot-tier promotion (M4-C1), mega-channel channel-sharded home-node (M4-C2), comment-threading consolidation (OQ-L) — each ships its seam + named follow-on, promoted only on its measured trigger; not a row that reds this gate.
