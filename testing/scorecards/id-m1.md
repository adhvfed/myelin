# M1→M2 (Identity) exit-gate scorecard (ID-D1/D2/D3/D4/D5/D6/D7/D8 + the 4.1–4.11 contract-coverage re-affirm)

> Generated: 2026-06-19. The build-layer realisation of the master band gate invariant (master-sequencing §2/§4, EI-01 §2): no later-band prompt runs over a red earlier gate. Each row is a dated green artifact read off the per-feature drill (this scorecard WIRES the drills, it does not re-implement them). A single RED row blocks M2 and is recorded honestly as claimed-not-proven, never edited green (EI-01 §3 / roadmap §5).

**Gate verdict: GREEN — M2 may start**

| Gate | Title | Verdict | Date | Permanent | Proof / reason |
|---|---|---|---|---|---|
| ID-D3 | cross-tenant check/list/read via path spoof → 0 cross-tenant tuples readable | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-identity-service --test drill_id_d3_cross_tenant` |
| ID-D2 | break Id dep → authenticated traffic survives on the coarse fail-static cache; just-revoked still denied (zookie bypass) | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-identity-service --test drill_id_d2_fail_static` |
| ID-D1 | SCIM-disable → every surface denies within N = 5 min; cache+token+denylist ≤ W; stale re-grant 0 | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-identity-service --test drill_id_d1_revocation` |
| ID-D4 | confidential object ABSENT from any list_objects for an unauthorized viewer, incl. the Filter-lowered S8 JOIN (zero-escape == 0) | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-identity-service --test drill_id_d4_zero_escape` |
| ID-D7 | revoke then re-read with the post-revoke zookie → no stale allow (watermark honoured) | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-identity-service --test drill_id_d7_watermark` |
| ID-D5 | adversarial delegation confined to agent.policy ∩ delegation ∩ tenant.policy (intersection proof; 0 escapes) | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-identity-service --test drill_id_d5_delegation` |
| ID-D6 | kill a run mid-flight → per-run token revoked + auto-expires within run-life ≤ W (revocation lag ≤ W) | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-identity-service --test drill_id_d6_run_token` |
| ID-D8 | restore to a consistent point → no resurrected grants past an erasure; post-restore re-erasure receipt emitted | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo test -p myelin-identity-service --test drill_id_d8_re_erasure` |
| contract-coverage | the contract-coverage scanner re-affirms the 4.1–4.11 CDC pairs are all present (the coverage gate) | PASS | 2026-06-19 | — | [2026-06-19] PASS  `cargo run -p myelin-lints --bin contract-coverage` |

**Floor named (M5 hardening).** Identity is *correct* at M1 and *hardened* at M5: ID-D9 (the 30× surge) + the multi-cell floor drills are M5 (P-ID-31 / P-ID-35) — not part of this M1→M2 go/no-go, recorded here so the deferral is visible, never invisible (EI-01 §1). ID-D8 rides the permanent restore-verify gate (STOR-D1/D2, Storage-owned P-061/P-100), which re-runs on every store-touching change.
