# GDPR/Audit truth-up pass — every PROVEN GDPR gate rests on a dated green artifact (P-GA-37 / P-511, GA-M6)

Run date: 2026-06-26

The code wins over the docs (EI-01 §1): each PROVEN GDPR row below names its DATED green artifact (the `cargo test` target that emits it), not a doc claim. The pass is GREEN iff EVERY row rests on a dated artifact — the gate invariant holds end-to-end (no earlier-band GDPR gate is red).

Enumerated by `myelin_gdpr_service::dogfood::proven_gdpr_rows` (the FROZEN §9.2 PROVEN set) and asserted by `myelin_gdpr_service::dogfood::TruthUpPass::run_or_fail_ci` — the committed drill `ga_p511_dogfood_self_served_dsr_drill::the_truth_up_pass_confirms_every_proven_gdpr_row_is_dated` fails LOUDLY on any claimed-not-proven row (the gate the CI must not swallow).

| Gate / drill | Dated artifact | Proof command |
|---|---|---|
| `GA-D1` — erasure reaches every holder — 0 holders missed over H1–H18 at cell scale | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ga_d1_full_fanout_cell_scale` |
| `GA-D2` — erasure reaches search — docs + embeddings purged-not-hidden, 0 re-identification | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ga_d2_derivative_erasure` |
| `GA-D3` — audit-tamper detection — a retroactive edit detected 3 independent ways | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ga_d3_audit_tamper` |
| `GA-D4` — DSR deadline — the durable timer warns before the statutory clock expires | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ga_d4_dsr_deadline_timer` |
| `GA-D6` — legal-hold — erase under an active hold suspended, 0 held-scope deletions, resumes on lift | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ga_d6_retention_legal_hold` |
| `GA-D7` — restriction-leak — restrict → 0 processing across the five derived stores, storage retained | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ga_d7_derived_restrict` |
| `GA-D8` — multi-cell erasure — 0 cells missed over member_cells ∪ home_cell, per-cell receipts complete | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ga_d8_multi_cell_fanout` |
| `GA-10` — history-rewrite-invalidation — fan-out reaches forks/mirrors/clone-cache, op audited, 0 stale-PII hits | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ga_10_history_rewrite_invalidation` |
| `GA-11` — outbound-residency-gate — extra-EU PII push-mirror denied by default, within-EU CDN clone allowed | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ga_11_outbound_mirror_residency_gate` |
| `CI-D3` — CI consumer-holder erasure — per-subject CI-log DEK crypto-shred reaches isolable log PII | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ci_d3_ci_holder_erasure` |
| `GIT-D2` — pseudonymous-commit — erase author → 0 recoverable real identity in immutable git bytes | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test git_d2_pseudonymous_commit` |
| `E2E-3` — spec-to-ship traceability — the GDPR audit-tamper proof feeds the E2E-3 leg | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test ga_p153_ediscovery_trace_history` |
| `E2E-4` — the DSAR fan-out flagship — 0 holders missed, 0 cells missed, certificate sealed | [2026-06-26] PROVEN | `cargo test -p myelin-gdpr-service --test e2e_4_dsar_fanout_flagship` |

**TRUTH-UP: GREEN** — 13 PROVEN GDPR rows (the §9.2 GA-D*/GA-10/GA-11 family + the E2E legs), 0 claimed-not-proven; the gate invariant holds end-to-end (no earlier-band GDPR gate is red).

**Named floor (EI-01 §1):** the FULL row-by-row truth-up enumeration across contracts 10.1–10.9 (the closing honesty pass that cross-checks every PROVEN GDPR row, not only the §9.2 drill family) is **P-GA-38 → P-512**. The `[OPEN — LEGAL]` residuals (the RoPA legal text, the worklog special-category classification, the Art. 17 reach into immutable git bytes, the Schrems-II `transfer_allowed` entries) are parallel-legal tracks — the DPO ratifies; the **structural floor ships regardless** and is what these PROVEN rows rest on. The live OLTP `audit_entry`/`dsr_request` tables + the real KMS signing key + a real RFC-3161 TSA witness + the live self-hosting JetStream subscription are the same DB/KMS/bus floor every M0/M1 store carries (P-007 / P-S12) — a config swap at boot, not a code change.

## The GDPR/Audit machinery live on Myelin's own commits + a self-served DSR (live)

The GDPR/Audit machinery now runs ON THE PLATFORM (`myelin_gdpr_service::dogfood`):

- **The audit consumer is live on the Myelin self-hosting outbox** (`run_audit_consumer_on_dogfood`): every Myelin action — a human commit/CI-run/issue/chat AND a coding-agent action (agents audited identically, EI-02 §2) — is delivered through the REAL outbox-only `AuditConsumer` and becomes one minimised, hash-chained, Merkle-leaf entry. The audit graph is **green on the platform's own actions** (the chain verifies, a per-tenant Merkle root exists, `audit_append_lag` reads green).
- **A self-served DSR over a Myelin team member's own data** (`run_self_served_dsr_on_dogfood`): a `dsr_submit` fans out across the whole H1–H18 holder catalogue (GA-D1, 0 holders missed) over `member_cells ∪ home_cell` (GA-D8, 0 cells missed) and **seals a certificate** into the per-tenant audit Merkle tree (the inclusion proof is the green artifact).
- **The RoPA + the data map live as a Myelin Knowledge space** (`RopaKnowledgeSpace::for_myelin_team`): the GENERATED data map (contract 10.3) + the Art. 30 RoPA projection render as the Myelin team's own GDPR space pages — the same generated artifacts (never hand-written), now living as the platform's own internal docs.
- **The every-incident-adds-a-drill loop is self-hosted** (`GdprIncident`): a GDPR incident files a PII-free Myelin **issue** draft AND registers a reproducing **drill** into the harness `DrillRegistry` via the T-3 `register_drill` hook (the committed drill re-runs the repro and asserts it reads GREEN; an incident missing either leg is a loud gap — never a silent skip).

The whole dogfood loop is the self-hosting CI graph job `GA-P511-dogfood` (`crates/myelin-harness/src/self_hosting_ci.rs`): a red audit chain on Myelin's own actions / a missed DSR holder / an undated PROVEN GDPR row reds the gate (the ratchet rejects on Myelin's own work, EI-01 §5).
