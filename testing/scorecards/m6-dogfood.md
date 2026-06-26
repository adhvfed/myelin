# M6 (dogfooding) exit-gate scorecard (switch tests (ISS-D14/CHAT-D19/GIT-OQ-12/KN-switch/REF-switch/SRCH-switch/CI-P35-switch) + self-hosting-CI + dogfood drills (FLOW-P29/AG-P26/CP-D23-selfhost/STOR-D37/GA-P511/REF-P28/SRCH-P33/KN-P34/GIT-P35) + truth-up pass (GA-truth-up + contract-coverage))

> Generated: 2026-06-26. The build-layer realisation of the master band gate invariant (master-sequencing §2/§4, EI-01 §2): no later-band prompt runs over a red earlier gate. Each row is a dated green artifact read off the per-feature drill (this scorecard WIRES the drills, it does not re-implement them). A single RED row blocks M7 and is recorded honestly as claimed-not-proven, never edited green (EI-01 §3 / roadmap §5).

**Gate verdict: GREEN — M7 may start**

| Gate | Title | Verdict | Date | Permanent | Proof / reason |
|---|---|---|---|---|---|
| ISS-D14 | Issues switch test → driven over the real surface with measured contrast + latency (not a feature-list read-off) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-issues --test iss_p37_switch_test_drill` |
| CHAT-D19 | Chat switch test → driven over the real surface with measured contrast + latency (the lib switch_test module) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-chat --lib switch_test` |
| GIT-OQ-12 | Git switch test → driven over the real surface with measured contrast + latency (not a feature-list read-off) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-git --test git_p35_switch_test_drill` |
| KN-switch | Knowledge switch test → driven over the real surface with measured contrast + latency (not a feature-list read-off) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-knowledge --test drill_kn_p34_switch_test` |
| REF-switch | Refs switch test → driven over the real surface with measured contrast + latency (not a feature-list read-off) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-refs-service --test ref_p29_switch_test_drill` |
| SRCH-switch | Search switch test → driven over the real surface with measured contrast + latency (not a feature-list read-off) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-search --test srch_p33_switch_test_drill` |
| CI-P35-switch | CI dogfood + switch test → the CI surface driven over the real surface with measured contrast + latency | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-ci-controlplane --test ci_p35_dogfood_switch_test_drill` |
| self-hosting-CI | the self-hosting CI graph is green on the platform's own commits → the dogfood loop is live; every-incident-adds-a-drill | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-harness --test self_hosting_ci_dogfood` |
| FLOW-P29 | Flow dogfood → a flow incident files an issue and joins the permanent drill suite (the platform runs on its own work) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-flow --test flow_p29_dogfood_drill` |
| AG-P26 | Agent fabric dogfood → a fabric incident files an issue and joins the permanent drill suite (the platform runs on its own work) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-agent-service --test ag_p26_dogfood_drill` |
| CP-D23-selfhost | Control-plane dogfood → Myelin self-hosts as one cell, residency-verify green, truth-up passes (the platform runs on its own work) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-control-plane --test cp_d23_dogfood_self_host_drill` |
| STOR-D37 | restore-verify on Myelin's own commits → a synthetic storage incident files an issue and joins the permanent drill suite (the permanent restore gate) | PASS | 2026-06-26 | re-run-forever | [2026-06-26] PASS  `cargo test -p myelin-storage --test stor_d37_dogfood_restore_verify_drill` |
| GA-P511 | self-served DSR → the dogfood DSR loop runs end-to-end self-hosting (the platform serves its own data-subject requests) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-gdpr-service --test ga_p511_dogfood_self_served_dsr_drill` |
| REF-P28 | Refs dogfood → a refs incident files an issue and joins the permanent drill suite (the platform runs on its own work) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-refs-service --test ref_p28_dogfood_drill` |
| SRCH-P33 | Search dogfood → a search incident files an issue and joins the permanent drill suite (the platform runs on its own work) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-search --test srch_p33_dogfood_drill` |
| KN-P34 | Knowledge dogfood → the every-incident loop joins the permanent suite and re-runs green (the platform runs on its own work) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-knowledge --test drill_kn_p34_dogfood` |
| GIT-P35 | Git dogfood → the every-incident loop joins the permanent suite and re-runs green (the platform runs on its own work) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-git --test git_p35_dogfood_drill` |
| GA-truth-up | truth-up pass → every PROVEN gate rests on a dated green artifact, never a doc claim (code-wins-over-docs, EI-01 §1) | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo test -p myelin-gdpr-service --test ga_p512_truth_up_pass` |
| contract-coverage | the contract-coverage scanner re-affirms the M6 CDC rows — no falsely-claimed/dropped row | PASS | 2026-06-26 | — | [2026-06-26] PASS  `cargo run -p myelin-lints --bin contract-coverage` |

**M6 is the platform done-bar reached by DOGFOODING** — Myelin hosts its own repos/CI/issues/docs/chat, and the switch tests are driven over the real surface (measured contrast + latency), not read off a feature list (EI-01 §4).

**The self-hosting CI graph is green on the platform's own commits** — the dogfood loop is live; every-incident-adds-a-drill.

**STOR-D37 dogfood restore-verify on Myelin's own commits** is permanent (a backup never restored is not a backup, EI-01 §3).

**The truth-up pass holds:** every PROVEN row here rests on a dated green artifact, never a doc claim (code-wins-over-docs, EI-01 §1).

**M7 (P-522..P-546) is the next band — production readiness & security hardening — and is NOT yet implemented.** M0..M6 deliberately shipped several production mechanisms as documented EI-01 §1 structural FLOORS (correct in shape, honestly named, not production-real): auth-token crypto (StructuralTokenSigner/Verifier still in prod constructors), HSM-class KMS, durable Identity stores (in-memory maps), real backup/restore (modeled offsets), and **sandbox PRODUCTION exec on both backends** (Firecracker prod boots `init=/bin/true`; gVisor prod runs only `runsc --version` — the AG-D4 isolation boundary is proven on real hardware, but a real `JobSpec.command` does not yet flow through prod `launch()`). M7 fills each floor with a real implementation + a SEPARATE verification prompt, and gates the first production release fail-closed (P-546). **This M6 green is dogfood-complete, NOT production-ready** — do not read it as the latter.
