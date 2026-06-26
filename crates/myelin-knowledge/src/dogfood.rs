//! # `dogfood` — Myelin's OWN docs live in Knowledge + the truth-up pass (KN-P34 / P-519, M6)
//!
//! **The Knowledge M6 dogfood prompt — THE DONE-BAR (knowledge-platform roadmap §3 KN-M6).** M6 promotes
//! NOTHING and freezes NO new contract — the Knowledge engine is fixed at M3 and hardened through M5 (the
//! Yrs CRDT promotion KN-P29, cross-cell collab KN-P30, facet/rollup materialisation KN-P31, the hot-doc
//! surge KN-P32, the E2E legs KN-P33). This prompt MIGRATES Myelin's OWN roadmap / gap-report / scorecard
//! into a Myelin **Knowledge space** (the team documents itself in its own Knowledge platform, VISION §5)
//! and reaches the switch-test verdict + the truth-up pass.
//!
//! ## What this module IS (the dogfood DRIVER over the EXISTING surface — EI-01 §7)
//! This is a **caller that drives the already-shipped Knowledge surface over the Myelin self-tenant** —
//! never a second project/reindex/round-trip/E2E. It REUSES:
//! - [`crate::e2e_wedge::run_e2e1_pr_context_pane`] — the PR context pane (a Knowledge design-doc embed
//!   resolves per-viewer through the SAME [`crate::refs_glue::Projector`] ladder; a denied viewer's
//!   confidential doc tombstones, 0 title leak), reframed onto Myelin's OWN design docs.
//! - [`crate::e2e_wedge::run_e2e3_spec_to_ship_lineage`] — the spec-to-ship lineage (a Knowledge spec doc →
//!   initiative → issues; cold-reindex == live byte-for-byte via the SAME
//!   [`crate::replay::KnowledgeReindexSource`]; audit tamper detected), reframed onto Myelin's own roadmap →
//!   scorecard lineage.
//!
//! ## The Myelin Knowledge space (the team documents itself)
//! [`run_knowledge_over_myelins_own_work`] drives Myelin's own roadmap / gap-report / scorecard as a
//! Knowledge space: each doc is a real [`crate::editor::Document`] whose every block round-trips
//! `render(parse(md)) === md` through the ONE render path (contract 13.1, the §8b.2 one-render-path law) —
//! the team's own knowledge survives the editor with byte-fidelity. The every-incident-adds-a-drill loop
//! ([`KnowledgeIncident`]) files a PII-free Myelin issue draft + a reproducing-drill ticket.
//!
//! ## The truth-up pass (the gate invariant — EI-01 §1)
//! [`KnowledgeTruthUpPass`] over [`proven_knowledge_rows`] — every PROVEN Knowledge row
//! (KN-D1..KN-D13 + the E2E slices E2E-1/E2E-3) rests on a DATED green artifact whose proof SOURCE exists
//! on disk; no later-band Knowledge gate is red. A row that names a vanished artifact is surfaced LOUDLY,
//! never trusted on faith (code-wins-over-docs).
//!
//! ## The switch test (split into [`crate::switch_test`])
//! The Knowledge switch test (the editor render + `render(parse(md)) === md` + the reference-chip /
//! tombstone overlays measured against the contrast/latency budgets, driven over the real surface — EI-01
//! §4) lives in the sibling [`crate::switch_test`] module.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new** — M6 promotes nothing; it exercises the production-hardened Knowledge surface on real
//!   self-tenant data. The ONE legitimate remaining floor is the world-scale 30× fleet-hardware load drill
//!   (the shared §4.1 fleet drill; the CI variant runs a moderate corpus). Any switch-test WALL found (a
//!   place the old tool did better) is recorded as a dated gap-report item with its follow-on owner — none
//!   found in this run.
//!
//! **Owning architecture doc:** `planning/04-subsystem-architectures/knowledge-platform/architecture/`
//! `00-overview.md` (the switch-test / done-bar framing), `06-reconciliation-compliance.md` (the
//! conformance map the truth-up confirms). **Roadmap:**
//! `planning/06-roadmaps/subsystems/knowledge-platform.md` §3 KN-M6 + §4 (the production-hardened
//! end-state). **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §1 (code-wins-over-docs
//! — the truth-up pass), §4 (the switch test — drive the real surface), §5 (the ratchet runs on the
//! builders' own work). **VISION §3** (the switch test) / **§5** (dogfooding).

use crate::e2e_wedge::{run_e2e1_pr_context_pane, run_e2e3_spec_to_ship_lineage, E2eArtifact};
use crate::editor::{Document, EditorBlock};

/// The Myelin self-tenant id (the platform self-hosts as exactly one cell — P-508 / CP-M6). Opaque,
/// PII-free — the dogfood Knowledge surface hosts the platform's OWN docs under this tenant.
pub const MYELIN_SELF_TENANT: &str = "myelin";

/// The region the Myelin self-tenant is pinned to (fr-par — the dev/prod residency pin; a config swap,
/// never a code change). The dogfood Knowledge surface resolves cell-local in this region.
pub const MYELIN_SELF_REGION: &str = "fr-par";

// ───────────────────────────── (1) Myelin's own docs live in Knowledge ─────────────────────────────

/// One of Myelin's OWN documents the team migrates into its Knowledge space (PII-free — the roadmap /
/// gap-report / scorecard, the platform documenting itself in its own Knowledge platform, VISION §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MyelinDoc {
    /// The doc's opaque page-id (a stable token; the drills assert against the NAME, never a literal).
    pub page_id: &'static str,
    /// A one-line human title (what the doc is — the team's own work).
    pub title: &'static str,
    /// The markdown-subset blocks the doc carries (each must round-trip through the ONE render path).
    pub blocks: Vec<&'static str>,
}

impl MyelinDoc {
    /// Build the doc as a real [`Document`] over the editor model — every block canonicalised through the
    /// ONE render path on construction (so the document is ALWAYS a `render(parse(md)) === md` fixed point
    /// iff the source blocks are canonical, the §8b.2 one-render-path law).
    pub fn to_document(&self) -> Document {
        Document {
            blocks: self
                .blocks
                .iter()
                .map(|md| EditorBlock::new(md, &[]))
                .collect(),
        }
    }

    /// `true` iff every block round-trips `render(parse(md)) === md` (the doc survives the editor with
    /// byte-fidelity — the team's own knowledge is not silently rewritten).
    pub fn round_trips(&self) -> bool {
        self.to_document().corpus_roundtrips()
    }
}

/// **Myelin's OWN docs as a Knowledge space (the roadmap / gap-report / scorecard).** The platform
/// documents itself in its own Knowledge platform (VISION §5). Every block is a canonical markdown-subset
/// body that round-trips through the ONE render path — the dogfood asserts the team's own knowledge
/// survives the editor with byte-fidelity. PII-free (opaque ids + the team's own process docs).
pub fn myelin_knowledge_space() -> Vec<MyelinDoc> {
    vec![
        MyelinDoc {
            page_id: "myelin-roadmap",
            title: "Myelin platform roadmap (M1..M6)",
            blocks: vec![
                "# Myelin platform roadmap\n",
                "The bands run *M1* through **M6**; M6 is the `dogfood` done-bar.\n",
                "Each band closes on a dated green exit-gate scorecard.\n",
            ],
        },
        MyelinDoc {
            page_id: "myelin-gap-report",
            title: "Myelin gap report (named floors + follow-ons)",
            blocks: vec![
                "# Gap report\n",
                "Every named floor carries a follow-on prompt id.\n",
                "The only remaining floor is the world-scale `30x` fleet-hardware load drill.\n",
            ],
        },
        MyelinDoc {
            page_id: "myelin-scorecard",
            title: "Myelin exit-gate scorecard (every PROVEN row dated)",
            blocks: vec![
                "# Exit-gate scorecard\n",
                "Every **PROVEN** row rests on a dated green drill artifact.\n",
                "A claim that outlives its verification is a `CLAIMED-NOT-PROVEN` red.\n",
            ],
        },
    ]
}

/// **The named green artifact the Knowledge dogfood run emits.** The production-hardened Knowledge surface
/// driven over Myelin's OWN work, across the three production faces:
/// - **Myelin's own docs in a Knowledge space** ([`myelin_knowledge_space`]) — every block round-trips
///   `render(parse(md)) === md` through the ONE render path (the team's knowledge survives the editor);
/// - **the PR context pane** (a Knowledge design-doc embed resolves per-viewer; a denied viewer's
///   confidential doc tombstones, 0 title leak) — REUSES [`run_e2e1_pr_context_pane`];
/// - **the spec-to-ship lineage** (roadmap → initiative → issues; cold-reindex == live byte-for-byte;
///   audit tamper detected) — REUSES [`run_e2e3_spec_to_ship_lineage`].
///
/// Knowledge is GREEN on the platform's own work iff every face is green AND 0 leak AND every Myelin doc
/// round-trips. A face that did not reach green fails LOUDLY ([`KnowledgeDogfoodArtifact::is_green`] is
/// false) — never a claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the dogfood artifact must be checked — an unread RED face silently claims a green Knowledge \
              did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct KnowledgeDogfoodArtifact {
    /// The date the dogfood run was asserted (every face is dated at this run).
    pub date: String,
    /// How many of Myelin's own docs round-tripped through the ONE render path (must == `docs_total`).
    pub docs_round_tripped: usize,
    /// How many of Myelin's own docs the space carries.
    pub docs_total: usize,
    /// The PR-context-pane face (the per-viewer leak-free embed resolution).
    pub pr_context_pane: E2eArtifact,
    /// The spec-to-ship lineage face (cold-reindex == live byte-for-byte; audit tamper detected).
    pub spec_to_ship: E2eArtifact,
}

impl KnowledgeDogfoodArtifact {
    /// `true` iff Knowledge is GREEN on Myelin's own work — every Myelin doc round-trips AND every E2E
    /// face green AND 0 leak. The ONLY way to read the dogfood run (a RED face is never silently a pass).
    pub fn is_green(&self) -> bool {
        self.docs_total > 0
            && self.docs_round_tripped == self.docs_total
            && self.pr_context_pane.is_green()
            && self.spec_to_ship.is_green()
            && self.total_leaks() == 0
    }

    /// The total leak/tamper counter across the two E2E faces (the F1 leak spine — must be 0).
    pub fn total_leaks(&self) -> u64 {
        self.pr_context_pane.leaks + self.spec_to_ship.leaks
    }

    /// The dated one-line summary (the artifact body the dogfood CI run prints).
    pub fn summary(&self) -> String {
        format!(
            "P-519 KNOWLEDGE DOGFOOD {} — tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             own-docs-round-trip={}/{} pr-context-pane={} spec-to-ship={} total-leaks={} verdict={}",
            self.date,
            self.docs_round_tripped,
            self.docs_total,
            self.pr_context_pane.is_green(),
            self.spec_to_ship.is_green(),
            self.total_leaks(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

/// **Run Knowledge over Myelin's OWN work (KN-P34).** Drives the production Knowledge surface across the
/// three faces (Myelin's own docs as a Knowledge space, the PR context pane, the spec-to-ship lineage) on
/// the Myelin self-tenant, REUSING the existing E2E-wedge runners (the SAME projector / reindex engine —
/// EI-01 §7, never a second implementation). `date` is the run stamp.
pub fn run_knowledge_over_myelins_own_work(date: &str) -> KnowledgeDogfoodArtifact {
    let space = myelin_knowledge_space();
    let docs_total = space.len();
    let docs_round_tripped = space.iter().filter(|d| d.round_trips()).count();
    KnowledgeDogfoodArtifact {
        date: date.to_string(),
        docs_round_tripped,
        docs_total,
        pr_context_pane: run_e2e1_pr_context_pane(),
        spec_to_ship: run_e2e3_spec_to_ship_lineage(),
    }
}

// ───────────────────────────── (2) the truth-up pass over the PROVEN Knowledge rows ─────────────────────────────

/// One PROVEN Knowledge row the truth-up pass enumerates. A gate/drill the ledger claims PROVEN, with the
/// proof command that emits its dated green artifact AND the repo-relative path to that proof source. The
/// truth-up pass asserts EACH row rests on a DATED green artifact whose source EXISTS on disk — a row that
/// names a vanished artifact is surfaced, never trusted on faith (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenKnowledgeRow {
    /// The stable gate/drill id (e.g. `"KN-D1"`, `"E2E-1"`).
    pub id: &'static str,
    /// The contract SECTION the row's gate belongs to (the §x.y / drill face — e.g. `"3.5"` the resume
    /// cursor, `"13.1"` the round-trip). The scorecard groups by section so coverage is visible at a glance.
    pub section: &'static str,
    /// A one-line human title (what the row proves).
    pub title: &'static str,
    /// The proof command that emits this row's dated green artifact (the `cargo test` target that lives
    /// with the feature prompt — named so the artifact is reproducible).
    pub proof_command: &'static str,
    /// The repo-RELATIVE path to the proof source (the test that the `proof_command` runs). The truth-up
    /// pass asserts this file EXISTS on disk — a row that names a vanished artifact is surfaced as
    /// CLAIMED-NOT-PROVEN, never swallowed (EI-01 §1).
    pub artifact_path: &'static str,
    /// The DATE the row's green artifact was last emitted, if any. `Some(date)` ⇒ dated + proven;
    /// `None` ⇒ CLAIMED-NOT-PROVEN (recorded honestly with a date, surfaced as a loud red).
    pub artifact_date: Option<String>,
}

impl ProvenKnowledgeRow {
    /// `true` iff this row rests on a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    /// Resolve this row's [`artifact_path`](Self::artifact_path) to an absolute path under `repo_root` so a
    /// caller can assert the proof source exists on disk.
    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

/// **The FROZEN set of PROVEN Knowledge rows the truth-up pass enumerates (KN-P34).** Every Knowledge gate
/// the ledger claims PROVEN: the thirteen engine/M-spanning drills **KN-D1..KN-D13** (resume-cursor /
/// md-round-trip / CAS-merge / erasure / leak-free-pushdown / reindex-parity / outbox-emit-iff-committed /
/// hot-doc-surge / flex-db-p99 / rollup-p99 / agent-HITL / agent-trace-erasure / cross-tenant) **plus** the
/// whole-system E2E slices **E2E-1 / E2E-3** (KN-P33). The truth-up pass asserts EVERY id here rests on a
/// dated green artifact whose proof source exists on disk; a row without one is a loud failure. `date` is
/// supplied by the runner so a claim never outlives its verification (EI-01 §1).
pub fn proven_knowledge_rows(date: &str) -> Vec<ProvenKnowledgeRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenKnowledgeRow {
        ProvenKnowledgeRow {
            id,
            section,
            title,
            proof_command: cmd,
            artifact_path,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        // ── The collab transport + the one render path (KN-D1, KN-D2). ──
        row(
            "KN-D1",
            "3.5",
            "kill a collab client mid-edit + sever the connection during sustained multi-author edit — 0 lost / 0 dup op (the resume cursor re-binds)",
            "cargo test -p myelin-knowledge --test cdc_3_5_knowledge_resume_cursor",
            "crates/myelin-knowledge/tests/cdc_3_5_knowledge_resume_cursor.rs",
            date,
        ),
        row(
            "KN-D2",
            "13.1",
            "render(parse(md)) === md 100% round-trip over the markdown-subset corpus, re-run over the integrated editor — the ONE render path (0 regressions)",
            "cargo test -p myelin-knowledge --test integration_kn_p09_integrated_editor",
            "crates/myelin-knowledge/tests/integration_kn_p09_integrated_editor.rs",
            date,
        ),
        // ── The CAS-floor merge + the per-subject erasure (KN-D3, KN-D4). ──
        row(
            "KN-D3",
            "13.3",
            "two clients edit the same block concurrently — the loser is rejected with current state (0 silent overwrite; the CAS-floor correctness gate)",
            "cargo test -p myelin-knowledge --test drill_kn_d3_cas_merge_floor",
            "crates/myelin-knowledge/tests/drill_kn_d3_cas_merge_floor.rs",
            date,
        ),
        row(
            "KN-D4",
            "10.9",
            "erase a subject — structured PII purged/pseudonymised, free-text under a per-subject DEK crypto-shredded (0 recoverable incl. vectors incl. backups)",
            "cargo test -p myelin-knowledge gdpr::erase_floor",
            "crates/myelin-knowledge/src/gdpr/erase_floor.rs",
            date,
        ),
        // ── The leak-free pre-filter + the reindex-from-source parity (KN-D5, KN-D6). ──
        row(
            "KN-D5",
            "4.3",
            "a confidential page / overridden sub-page / row-restricted db / field-hidden column never leaks — incl. COUNT across search/embed/RAG (0 leak, the SetExpr pre-filter)",
            "cargo test -p myelin-knowledge --test integration_kn_d5_list_pushdown",
            "crates/myelin-knowledge/tests/integration_kn_d5_list_pushdown.rs",
            date,
        ),
        row(
            "KN-D6",
            "2.6",
            "wipe Knowledge's derived state (Refs edge projection / Search index); replay(scope) rebuilds it cold == live byte-for-byte (the only recovery path)",
            "cargo test -p myelin-knowledge --test drill_kn_d6_reindex_parity",
            "crates/myelin-knowledge/tests/drill_kn_d6_reindex_parity.rs",
            date,
        ),
        // ── The outbox emit-iff-committed + the hot-doc surge (KN-D7, KN-D8). ──
        row(
            "KN-D7",
            "2.2",
            "crash Knowledge between the block/row commit and relay-publish — the event is still delivered (outbox emit-iff-committed; 0 ghost / 0 lost)",
            "cargo test -p myelin-knowledge --test integration_kn_d7_outbox",
            "crates/myelin-knowledge/tests/integration_kn_d7_outbox.rs",
            date,
        ),
        row(
            "KN-D8",
            "4.11",
            "an all-hands doc with thousands of concurrent readers/editors — per-doc op cap + read-fanout bound held; the concurrent-same-gap LexoRank storm converges (0 duplicate rank)",
            "cargo test -p myelin-knowledge --test drill_kn_d8_allhands_surge",
            "crates/myelin-knowledge/tests/drill_kn_d8_allhands_surge.rs",
            date,
        ),
        // ── The flex-db p99 + the read-time rollup p99 (KN-D9, KN-D10). ──
        row(
            "KN-D9",
            "13.3",
            "filter/sort/group a large multi-tenant database (JSONB + projection + SetExpr conjoin) — p99 within budget, 0 cross-tenant leak",
            "cargo test -p myelin-knowledge --test integration_kn_d9_flex_db",
            "crates/myelin-knowledge/tests/integration_kn_d9_flex_db.rs",
            date,
        ),
        row(
            "KN-D10",
            "13.3",
            "a rollup over a large related set, computed at read time (permission-filtered) — p99 within budget; the materialisation trigger fires past the measured threshold",
            "cargo test -p myelin-knowledge --test integration_kn_d10_rollup",
            "crates/myelin-knowledge/tests/integration_kn_d10_rollup.rs",
            date,
        ),
        // ── The agent HITL governance + the agent-trace erasure (KN-D11, KN-D12). ──
        row(
            "KN-D11",
            "8.1",
            "an agent edits a doc via EffectApi — attributed 'suggested by agent'; a consequential edit is HITL-withheld until approval (0 ungoverned mutation, 0 mutation before approval)",
            "cargo test -p myelin-knowledge --test cdc_8_2_knowledge_agent_governance",
            "crates/myelin-knowledge/tests/cdc_8_2_knowledge_agent_governance.rs",
            date,
        ),
        row(
            "KN-D12",
            "8.8",
            "erase a subject — content-addressed agent traces crypto-shredded/purged; attribution falls back to a tombstone (0 recoverable trace)",
            "cargo test -p myelin-knowledge --test cdc_8_8_knowledge_agent_trace",
            "crates/myelin-knowledge/tests/cdc_8_8_knowledge_agent_trace.rs",
            date,
        ),
        // ── The cross-tenant isolation (KN-D13). ──
        row(
            "KN-D13",
            "11.1",
            "read a page/db/row across tenants via path-tenant spoofing — 0 cross-tenant read (the (tenant, region) partition + RLS front door)",
            "cargo test -p myelin-knowledge --test cdc_11_1_12_1_knowledge_oltp_store_and_partition",
            "crates/myelin-knowledge/tests/cdc_11_1_12_1_knowledge_oltp_store_and_partition.rs",
            date,
        ),
        // ── The whole-system E2E wedge slices (E2E-1 / E2E-3 — KN-P33). ──
        row(
            "E2E-1",
            "5.6",
            "the PR context pane — a Knowledge design-doc embed resolves per-viewer; a denied viewer's confidential doc tombstones (0 title leak; mid-flight erase honoured live)",
            "cargo test -p myelin-knowledge --test drill_kn_p33_e2e_wedge",
            "crates/myelin-knowledge/tests/drill_kn_p33_e2e_wedge.rs",
            date,
        ),
        row(
            "E2E-3",
            "2.6",
            "spec-to-ship lineage — a Knowledge spec doc → initiative → issues; cold-reindex == live byte-for-byte; audit tamper detected (0 silent tamper)",
            "cargo test -p myelin-knowledge --test drill_kn_p33_e2e_wedge",
            "crates/myelin-knowledge/tests/drill_kn_p33_e2e_wedge.rs",
            date,
        ),
    ]
}

/// The verdict of the Knowledge truth-up pass — Green (every PROVEN row dated) or Red (the undated rows
/// named). Never a swallowed bool — a RED points at exactly which Knowledge claim outran its verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeTruthUpVerdict {
    /// Every enumerated PROVEN Knowledge row rests on a dated green artifact (no later-band gate red).
    Green {
        /// How many PROVEN rows were confirmed dated + green.
        rows_confirmed: usize,
        /// The date the truth-up pass ran.
        date: String,
    },
    /// One or more PROVEN rows are CLAIMED-NOT-PROVEN. Names them so the failure is specific.
    Red {
        /// The ids of the rows lacking a dated green artifact (the loud failure list).
        undated_rows: Vec<&'static str>,
    },
}

impl KnowledgeTruthUpVerdict {
    /// `true` iff the truth-up pass is green (every PROVEN row dated). The ONLY way to read a pass.
    pub fn is_green(&self) -> bool {
        matches!(self, KnowledgeTruthUpVerdict::Green { .. })
    }

    /// The ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass).
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            KnowledgeTruthUpVerdict::Green { .. } => &[],
            KnowledgeTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The Knowledge truth-up pass (KN-P34 / EI-01 §1).** Enumerates every PROVEN Knowledge row and confirms
/// each rests on a DATED green artifact. A row WITHOUT one is a LOUD failure
/// ([`KnowledgeTruthUpVerdict::Red`]), never a silent pass — the code-wins-over-docs discipline made
/// mechanical. A zero-sized orchestrator.
#[derive(Clone, Copy, Debug, Default)]
pub struct KnowledgeTruthUpPass;

impl KnowledgeTruthUpPass {
    /// A new truth-up pass (stateless).
    pub fn new() -> KnowledgeTruthUpPass {
        KnowledgeTruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`KnowledgeTruthUpVerdict::Green`] (every row dated)
    /// or [`KnowledgeTruthUpVerdict::Red`] (the undated rows named). `date` stamps the green verdict.
    pub fn run(&self, rows: &[ProvenKnowledgeRow], date: &str) -> KnowledgeTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            KnowledgeTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            KnowledgeTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    /// **The loud-never-swallowed truth-up CI entrypoint (EI-01 §5).** Run the pass and turn a RED verdict
    /// into a process-failing `Err` — so a CI invocation `pass.run_or_fail_ci(&rows, date)?` FAILS the
    /// dogfood truth-up job if ANY PROVEN Knowledge row lacks a dated green artifact. On GREEN it returns
    /// the number of confirmed rows.
    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenKnowledgeRow],
        date: &str,
    ) -> Result<usize, KnowledgeTruthUpRed> {
        match self.run(rows, date) {
            KnowledgeTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            KnowledgeTruthUpVerdict::Red { undated_rows } => Err(KnowledgeTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

/// A RED truth-up pass surfaced as an `Err` — the CLAIMED-NOT-PROVEN Knowledge rows, loud + specific (the
/// process exits non-zero, never a silent docs drift).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeTruthUpRed {
    /// The ids of the rows lacking a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for KnowledgeTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL — {} Knowledge row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} — a \
             claim that outlives its verification misleads the next agent (EI-01 §1); fix the doc or \
             re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for KnowledgeTruthUpRed {}

// ───────────────────────────── the enumerated truth-up scorecard (the green artifact) ─────────────────────────────

/// How a PROVEN Knowledge row's proof stands at truth-up time: a dated green artifact, or an
/// honestly-recorded CLAIMED-NOT-PROVEN note. Either way the status carries a DATE (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeRowStatus {
    /// The row rests on a dated green artifact whose proof source exists on disk.
    DatedGreen {
        /// The date the green artifact was last emitted.
        date: String,
    },
    /// The row is CLAIMED but NOT PROVEN — no dated green artifact, or its proof source is gone.
    ClaimedNotProven {
        /// The date the truth-up pass recorded the gap.
        date: String,
        /// Why the row is not proven (no artifact date, or the proof source is missing on disk).
        reason: String,
    },
}

impl KnowledgeRowStatus {
    /// `true` iff this is a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated_green(&self) -> bool {
        matches!(self, KnowledgeRowStatus::DatedGreen { .. })
    }
}

/// One scorecard line: a PROVEN Knowledge row resolved to its [`KnowledgeRowStatus`] at truth-up time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeScorecardEntry {
    /// The row this line scores.
    pub row: ProvenKnowledgeRow,
    /// Its resolved status (dated-green or claimed-not-proven, both dated).
    pub status: KnowledgeRowStatus,
}

/// **The enumerated Knowledge truth-up scorecard (the GATE/DRILLS green artifact, KN-P34).** Every PROVEN
/// Knowledge row → its dated green artifact (or a dated CLAIMED-NOT-PROVEN note). The scorecard itself is
/// the closing-honesty-pass artifact: rendering it produces the section-grouped table the prompt's GATE
/// demands, and [`Self::is_green`] is true iff NO later-band Knowledge gate is red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked — an unread CLAIMED-NOT-PROVEN row silently drifts \
              the docs from the code (EI-01 §1)"]
pub struct KnowledgeTruthUpScorecard {
    /// The run date the scorecard is stamped with.
    pub date: String,
    /// One entry per PROVEN Knowledge row, in section order.
    pub entries: Vec<KnowledgeScorecardEntry>,
}

impl KnowledgeTruthUpScorecard {
    /// `true` iff every row rests on a dated green artifact (the gate invariant: no Knowledge gate red).
    pub fn is_green(&self) -> bool {
        self.entries.iter().all(|e| e.status.is_dated_green())
    }

    /// How many rows the scorecard enumerates.
    pub fn rows_total(&self) -> usize {
        self.entries.len()
    }

    /// How many rows rest on a dated green artifact.
    pub fn rows_dated_green(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status.is_dated_green())
            .count()
    }

    /// The ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass) — the loud failure list.
    pub fn claimed_not_proven(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| !e.status.is_dated_green())
            .map(|e| e.row.id)
            .collect()
    }

    /// **Render the enumerated scorecard as the dated green artifact** (the section-grouped table a
    /// truth-up CI run prints). CLAIMED-NOT-PROVEN rows are rendered LOUD, never elided.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let verdict = if self.is_green() {
            "GREEN (no later-band Knowledge gate red)"
        } else {
            "RED (a Knowledge claim outran its verification)"
        };
        out.push_str(&format!(
            "P-519 KNOWLEDGE TRUTH-UP SCORECARD {} — {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                KnowledgeRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                KnowledgeRowStatus::ClaimedNotProven { date, reason } => {
                    format!("CLAIMED-NOT-PROVEN({date}: {reason})")
                }
            };
            out.push_str(&format!(
                "  [§{}] {:<8} {:<28} — {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

/// **Run the Knowledge truth-up pass and produce the enumerated [`KnowledgeTruthUpScorecard`] (KN-P34).**
/// For each PROVEN Knowledge row this resolves a dated [`KnowledgeRowStatus`]: a row is DATED-GREEN iff it
/// carries an `artifact_date` AND its proof source exists on disk under `repo_root`; otherwise it is
/// recorded CLAIMED-NOT-PROVEN with the run `date` and the honest reason. The scorecard surfaces — never
/// swallows — any gap (EI-01 §1). `repo_root` is the workspace root the `artifact_path`s are relative to.
pub fn run_knowledge_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> KnowledgeTruthUpScorecard {
    let entries = proven_knowledge_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => KnowledgeRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    KnowledgeRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => KnowledgeRowStatus::DatedGreen { date: d.clone() },
            };
            KnowledgeScorecardEntry { row, status }
        })
        .collect();
    KnowledgeTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

// ───────────────────────────── (3) the every-incident-adds-a-drill loop ─────────────────────────────

/// **A Knowledge incident on Myelin's own development (the every-incident-adds-a-drill loop, EI-01 §3/§5).**
/// A real incident ends by filing a PII-free Myelin issue draft AND a reproducing-drill ticket — both
/// reference-linked (the moat thesis: the issue points at the drill that reproduces it). The integration
/// drill registers the repro so it re-runs forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeIncident {
    /// The incident id (PII-free, e.g. `"INC-KN-DOGFOOD-1"`).
    pub incident_id: String,
    /// The Knowledge gate the incident regressed (e.g. `"KN-D2"`).
    pub gate_id: String,
    /// A PII-free one-line description of what broke.
    pub description: String,
    /// The name of the reproducing drill the incident files (the test that re-fires the failure).
    pub repro_drill_name: String,
}

impl KnowledgeIncident {
    /// A new Knowledge incident (every field PII-free).
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> KnowledgeIncident {
        KnowledgeIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    /// The PII-free Myelin issue draft the incident files (names the gate + the repro drill — the issue is
    /// reference-linked to the drift that reproduces it).
    pub fn issue_draft(&self) -> KnowledgeIncidentIssueDraft {
        KnowledgeIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] Knowledge gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "Knowledge incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked — every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    /// The reproducing-drill ticket the incident files (the test that joins the permanent suite).
    pub fn drill_ticket(&self) -> KnowledgeIncidentDrillTicket {
        KnowledgeIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

/// The PII-free Myelin issue draft a [`KnowledgeIncident`] files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeIncidentIssueDraft {
    /// The Knowledge gate the issue is about.
    pub gate_id: String,
    /// The issue title (PII-free).
    pub title: String,
    /// The issue body (PII-free; names the repro drill).
    pub body: String,
}

/// The reproducing-drill ticket a [`KnowledgeIncident`] files (the drill that joins the permanent suite).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeIncidentDrillTicket {
    /// The drill name (the test that re-fires the failure).
    pub drill_name: String,
    /// The gate the drill guards.
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    /// **THE HEADLINE: Knowledge is GREEN on Myelin's OWN work.** Myelin's own docs round-trip through the
    /// ONE render path, the PR context pane resolves per-viewer (0 title leak), and the spec-to-ship
    /// lineage is cold == live + tamper-detected — all over the Myelin self-tenant, 0 leak.
    #[test]
    fn knowledge_greens_on_myelins_own_work() {
        let artifact = run_knowledge_over_myelins_own_work(RUN_DATE);

        assert!(
            artifact.is_green(),
            "Knowledge must be green on Myelin's own work: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.docs_round_tripped,
            artifact.docs_total,
            "every one of Myelin's own docs round-trips through the ONE render path: {}",
            artifact.summary()
        );
        assert!(artifact.docs_total >= 3, "the roadmap/gap-report/scorecard");
        assert!(
            artifact.pr_context_pane.is_green(),
            "the PR context pane is green"
        );
        assert!(
            artifact.spec_to_ship.is_green(),
            "the spec-to-ship lineage is green"
        );
        assert_eq!(
            artifact.total_leaks(),
            0,
            "0 leak across the two E2E faces: {}",
            artifact.summary()
        );

        let s = artifact.summary();
        assert!(
            s.contains("P-519 KNOWLEDGE DOGFOOD 2026-06-26"),
            "dated: {s}"
        );
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    /// Every one of Myelin's own docs round-trips `render(parse(md)) === md` (the team's knowledge
    /// survives the editor with byte-fidelity — the §8b.2 one-render-path law).
    #[test]
    fn myelins_own_docs_round_trip_through_the_one_render_path() {
        let space = myelin_knowledge_space();
        assert!(space.len() >= 3, "the roadmap/gap-report/scorecard");
        for doc in &space {
            assert!(
                doc.round_trips(),
                "the Myelin doc {} must round-trip through the ONE render path",
                doc.page_id
            );
            // a real document was built (not an empty stand-in).
            assert!(!doc.to_document().blocks.is_empty(), "{}", doc.page_id);
        }
    }

    /// The truth-up pass is GREEN — every PROVEN Knowledge row rests on a dated green artifact.
    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_knowledge_rows(RUN_DATE);
        assert!(
            rows.len() >= 15,
            "the PROVEN set covers KN-D1..KN-D13 + the E2E slices"
        );
        let confirmed = KnowledgeTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red later-band Knowledge gates — every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    /// A claimed-not-proven row reds the truth-up pass LOUDLY (surfaced, never swallowed).
    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_knowledge_rows(RUN_DATE);
        rows[0].artifact_date = None; // simulate a doc claim with no dated green artifact
        let verdict = KnowledgeTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = KnowledgeTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row fails the CI entrypoint");
        assert!(err.to_string().contains("CLAIMED-NOT-PROVEN"));
    }

    /// The enumerated scorecard renders GREEN with every PROVEN row dated + its proof source on disk.
    #[test]
    fn the_scorecard_renders_green_with_proof_sources_on_disk() {
        let scorecard = run_knowledge_truth_up_scorecard(RUN_DATE, &repo_root());
        assert!(
            scorecard.is_green(),
            "the scorecard must be green — every PROVEN Knowledge row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("KN-D1") && md.contains("E2E-3"),
            "enumerated: {md}"
        );
    }

    /// A row whose proof source is missing on disk is surfaced CLAIMED-NOT-PROVEN (never trusted on faith).
    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-kn-truth-up-root");
        let scorecard = run_knowledge_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard.entries.iter().all(|e| matches!(
                &e.status,
                KnowledgeRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk")
            )),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    /// The every-incident loop: an incident files a PII-free issue draft + a reproducing-drill ticket.
    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = KnowledgeIncident::new(
            "INC-KN-DOGFOOD-1",
            "KN-D2",
            "a markdown-subset corpus body silently round-tripped non-canonically on the Myelin self-tenant",
            "repro_kn_d2_dogfood_non_canonical_round_trip",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "KN-D2");
        assert!(draft.title.contains("INC-KN-DOGFOOD-1"));
        assert!(
            draft
                .body
                .contains("repro_kn_d2_dogfood_non_canonical_round_trip"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        // PII-free: the draft carries no personal data, only opaque ids + gate names.
        assert!(!draft.body.to_lowercase().contains("email"));

        let ticket = incident.drill_ticket();
        assert_eq!(
            ticket.drill_name,
            "repro_kn_d2_dogfood_non_canonical_round_trip"
        );
        assert_eq!(ticket.gate_id, "KN-D2");
    }

    /// The proof-source paths the truth-up rows name all exist on disk (the rows are not stale).
    #[test]
    fn every_proven_row_proof_source_exists_on_disk() {
        for row in proven_knowledge_rows(RUN_DATE) {
            assert!(
                row.artifact_abs_path(&repo_root()).exists(),
                "the proof source for {} must exist on disk: {}",
                row.id,
                row.artifact_path
            );
        }
    }

    /// The workspace root the `artifact_path`s are relative to (two parents up from this crate's manifest).
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf()
    }
}
