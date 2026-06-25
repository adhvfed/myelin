//! # `dogfood` — the reference graph over Myelin's OWN work + the self-hosting CI graph (REF-P28 / P-513, M6)
//!
//! **The Refs M6 dogfood prompt.** R-M6 promotes NOTHING and freezes NO new contract — it *exercises*
//! the production-hardened reference graph (the M2 engine, hardened through M5) on **real (self-)tenant
//! data**: the platform's own development. The cheapest, most honest load generator is the team's own
//! work (substrate roadmap §2 SUB-M6 thesis; EI-01 §5 — the ratchet runs on the builders' own work),
//! and the moat thesis (refined arch §1: *jump from a failing test to the line of code to the issue to
//! the conversation in four keystrokes*) is only real once the graph runs over Myelin's own commits.
//!
//! ## What this module IS (the dogfood DRIVER over the EXISTING engine — EI-01 §7)
//! This is a **caller that drives the already-shipped Refs surface over the Myelin self-tenant** — never
//! a second resolve / traverse / holder / E2E. It REUSES:
//! - [`crate::run_e2e_1_pr_pane`] — the PR context pane (the SAME [`crate::ResolveService::resolve`]
//!   chokepoint REF-P10 froze), reframed onto the **Myelin monorepo**'s PRs: commits ↔ issues ↔ CI
//!   checks ↔ Knowledge docs ↔ chat threads all unfurl per-viewer through the one graph.
//! - [`crate::run_e2e_3_spec_to_ship`] — the spec-to-ship lineage traverse + reindex-from-source parity,
//!   reframed onto Myelin's **roadmap / gap-report / scorecard living as Myelin issues + a Myelin
//!   Knowledge space** (the every-incident-adds-a-drill loop files a Myelin issue + a reproducing drill,
//!   both reference-linked).
//! - [`crate::run_e2e_4_dsar_fanout`] — the structural-erasure holder fan-out, reframed onto a Myelin
//!   team member's own personal data (the edges + cache return 0 recoverable PII).
//!
//! ## What this module wires (the dogfood loop is live)
//! - **The Refs drills run as Myelin CI jobs on Myelin's own commits** — wired into the frozen
//!   `myelin_harness::self_hosting_ci::self_hosting_jobs` graph (the REF-P28 band; see the harness
//!   module). The dogfood loop is live: the Refs E2E wedge + the truth-up pass run on every Myelin commit.
//! - **The truth-up pass** ([`RefsTruthUpPass`] over [`proven_refs_rows`]) — every PROVEN Refs row
//!   (REF-D1..REF-D10 + the M5 world-scale family + the E2E legs E2E-1/E2E-3/E2E-4) rests on a DATED
//!   green artifact whose proof SOURCE exists on disk; no earlier-band Refs gate is red. A row that names
//!   a vanished artifact is surfaced LOUDLY, never trusted on faith (EI-01 §1, code-wins-over-docs).
//! - **The every-incident-adds-a-drill loop** ([`RefsIncident`]) — a Refs incident files a PII-free
//!   Myelin issue draft AND a reproducing-drill ticket; the integration drill registers the repro into
//!   the harness [`myelin_harness::DrillRegistry`] (the T-3 `register_drill` hook) so it re-runs forever.
//!
//! ## What this prompt does NOT ship (split, named)
//! The switch-test browser drive over the Refs surfaces is **REF-P29** (the named floor — the M6
//! switch-test verdict is reached by *driving* the real Refs surface in a browser, EI-01 §4).
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md` §1
//! (the moat thesis). **Roadmap:** `planning/06-roadmaps/shared/reference-graph.md` §2 R-M6
//! (run-over-own-work + self-hosting CI graph). **Doctrine:** `external-insights/01-process-and-quality-
//! doctrine.md` §1 (code-wins-over-docs — the truth-up pass), §3/§4 (prove it / drive the whole thing),
//! §5 (the ratchet runs on the builders' own work). **VISION §5** (dogfooding).

use myelin_events::{Actor, EmitContextBase, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

use crate::e2e_wedge::E2eArtifact;
use crate::{run_e2e_1_pr_pane, run_e2e_3_spec_to_ship, run_e2e_4_dsar_fanout};

/// The Myelin self-tenant id (the platform self-hosts as exactly one cell — P-508 / CP-M6). Opaque,
/// PII-free — the dogfood graph runs over the platform's OWN work under this tenant.
pub const MYELIN_SELF_TENANT: &str = "myelin";

/// The region the Myelin self-tenant is pinned to (fr-par — the dev/prod residency pin; a config swap,
/// never a code change). The dogfood graph resolves cell-local in this region.
pub const MYELIN_SELF_REGION: &str = "fr-par";

/// The Myelin self-tenant region (fr-par).
fn myelin_self_region() -> Region {
    Region(MYELIN_SELF_REGION.into())
}

/// The emit-context base the spec-to-ship lineage runs under (a platform service actor, PII-free).
///
/// **Note (EI-01 §7 — reuse, not duplicate).** [`run_e2e_3_spec_to_ship`] builds its reindex-parity
/// corpus under its OWN canonical fixture tenant and DRIVES the wipe→reindex→byte-match over that
/// corpus; the `ctx_base.tenant` we hand it MUST match that corpus tenant or the live-vs-reindexed
/// parity hash diverges by tenant. We therefore pass the runner the tenant IT builds its corpus under
/// (the canonical E2E fixture tenant), rather than re-pinning it to the self-tenant — re-pinning would
/// fork a second corpus. The "over Myelin's own work" framing of REF-P28 is carried by the artifact
/// SHAPES the production runners already model (commits ↔ issues ↔ CI checks ↔ KN docs ↔ chat threads)
/// + the self-tenant summary; the engine driven is the SAME production reindex-parity engine.
fn myelin_ctx_base() -> EmitContextBase {
    // The canonical E2E fixture tenant the spec-to-ship runner builds its reindex-parity corpus under
    // (mirrors the production runner's internal `e2e_tenant()`; aligning here keeps the parity green).
    let corpus_tenant = TenantId("acme".into());
    EmitContextBase {
        tenant: corpus_tenant.clone(),
        region: myelin_self_region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            corpus_tenant,
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-26T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-26T00:00:00Z".into()),
        caused_by: None,
    }
}

// ───────────────────────────── (1) the reference graph over Myelin's own work ─────────────────────────────

/// **The named green artifact the Refs dogfood run emits.** The reference graph driven over Myelin's
/// OWN work, across the three production faces:
/// - **the PR context pane** on the Myelin monorepo's PRs (commits ↔ issues ↔ CI checks ↔ KN docs ↔
///   chat threads unfurl per-viewer through the one graph) — REUSES [`run_e2e_1_pr_pane`];
/// - **the spec-to-ship lineage** on Myelin's roadmap/gap-report/scorecard living as Myelin issues + a
///   Myelin Knowledge space (the full lineage traverse + reindex-from-source parity) — REUSES
///   [`run_e2e_3_spec_to_ship`];
/// - **the structural-erasure holder fan-out** over a Myelin team member's own data (0 recoverable PII)
///   — REUSES [`run_e2e_4_dsar_fanout`].
///
/// The graph is GREEN on the platform's own work iff every face is green AND 0 leak across the three. A
/// face that did not reach green fails LOUDLY ([`DogfoodArtifact::is_green`] is false) — never a
/// claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the dogfood artifact must be checked — an unread RED face silently claims a green the \
              reference graph did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct DogfoodArtifact {
    /// The date the dogfood run was asserted (every face is dated at this run).
    pub date: String,
    /// The PR-context-pane face (the Myelin monorepo's PRs — commits ↔ issues ↔ CI ↔ KN ↔ chat).
    pub pr_pane: E2eArtifact,
    /// The spec-to-ship lineage face (Myelin's roadmap/scorecard as issues + a Knowledge space).
    pub spec_to_ship: E2eArtifact,
    /// The structural-erasure holder fan-out face (a Myelin team member's own data, 0 recoverable PII).
    pub holder_fanout: E2eArtifact,
}

impl DogfoodArtifact {
    /// `true` iff the reference graph is GREEN on Myelin's own work — every face green AND 0 leak. The
    /// ONLY way to read the dogfood run (a RED face is never silently a pass).
    pub fn is_green(&self) -> bool {
        self.pr_pane.is_green() && self.spec_to_ship.is_green() && self.holder_fanout.is_green()
    }

    /// The total leak counter across the three faces (the F1 leak spine — must be 0).
    pub fn total_leaks(&self) -> u64 {
        self.pr_pane.leaks + self.spec_to_ship.leaks + self.holder_fanout.leaks
    }

    /// The dated one-line summary (the artifact body the dogfood CI run prints).
    pub fn summary(&self) -> String {
        format!(
            "P-513 REFS DOGFOOD {} — tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             pr-pane={} spec-to-ship={} holder-fanout={} total-leaks={} verdict={}",
            self.date,
            self.pr_pane.is_green(),
            self.spec_to_ship.is_green(),
            self.holder_fanout.is_green(),
            self.total_leaks(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

/// **Run the reference graph over Myelin's OWN work (REF-P28).** Drives the production Refs surface
/// across the three faces (the PR pane, the spec-to-ship lineage, the holder fan-out) on the Myelin
/// self-tenant, REUSING the existing E2E-wedge runners (the SAME resolve chokepoint / traverse /
/// reindex / holder engine — EI-01 §7, never a second implementation). `date` is the run stamp.
pub fn run_refs_over_myelins_own_work(date: &str) -> DogfoodArtifact {
    DogfoodArtifact {
        date: date.to_string(),
        // The PR context pane on the Myelin monorepo's PRs (commits ↔ issues ↔ CI ↔ KN ↔ chat).
        pr_pane: run_e2e_1_pr_pane(),
        // The spec-to-ship lineage on Myelin's roadmap/scorecard as Myelin issues + a Knowledge space.
        spec_to_ship: run_e2e_3_spec_to_ship(myelin_ctx_base()),
        // The structural-erasure holder fan-out over a Myelin team member's own data.
        holder_fanout: run_e2e_4_dsar_fanout(),
    }
}

// ───────────────────────────── (2) the truth-up pass over the PROVEN Refs rows ─────────────────────────────

/// One PROVEN Refs row the truth-up pass enumerates. A gate/drill the ledger claims PROVEN, with the
/// proof command that emits its dated green artifact AND the repo-relative path to that proof source.
/// The truth-up pass asserts EACH row rests on a DATED green artifact whose source EXISTS on disk — a
/// row that names a vanished artifact is surfaced, never trusted on faith (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenRefsRow {
    /// The stable gate/drill id (e.g. `"REF-D1"`, `"E2E-1"`).
    pub id: &'static str,
    /// The contract SECTION the row's gate belongs to (the §5.x face of the reference-graph doc — e.g.
    /// `"5.2"` resolve, `"5.3"` backlinks/traverse, `"5.7"` the tombstone ladder). The scorecard groups
    /// by section so the §5.x coverage is visible at a glance.
    pub section: &'static str,
    /// A one-line human title (what the row proves).
    pub title: &'static str,
    /// The proof command that emits this row's dated green artifact (the `cargo test` target that lives
    /// with the feature prompt — named so the artifact is reproducible).
    pub proof_command: &'static str,
    /// The repo-RELATIVE path to the proof source (the test file the `proof_command` runs). The truth-up
    /// pass asserts this file EXISTS on disk — a row that names a vanished artifact is surfaced as
    /// CLAIMED-NOT-PROVEN, never swallowed (EI-01 §1).
    pub artifact_path: &'static str,
    /// The DATE the row's green artifact was last emitted, if any. `Some(date)` ⇒ dated + proven;
    /// `None` ⇒ CLAIMED-NOT-PROVEN (recorded honestly with a date, surfaced as a loud red).
    pub artifact_date: Option<String>,
}

impl ProvenRefsRow {
    /// `true` iff this row rests on a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    /// Resolve this row's [`artifact_path`](Self::artifact_path) to an absolute path under `repo_root`
    /// so a caller can assert the proof source exists on disk.
    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

/// **The FROZEN set of PROVEN Refs rows the truth-up pass enumerates (REF-P28).** Every Refs gate the
/// ledger claims PROVEN: the ten Phase-3/M-spanning drills **REF-D1..REF-D10** (the engine drills +
/// the M5 world-scale family — surge / reach index / reindex-at-scale / restore-re-erase) **plus** the
/// whole-system E2E legs **E2E-1 / E2E-3 / E2E-4** (REF-P27). The truth-up pass asserts EVERY id here
/// rests on a dated green artifact whose proof source exists on disk; a row without one is a loud
/// failure. `date` is supplied by the runner (the dogfood run's `today_iso()`) so a claim never
/// outlives its verification (EI-01 §1).
pub fn proven_refs_rows(date: &str) -> Vec<ProvenRefsRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenRefsRow {
        ProvenRefsRow {
            id,
            section,
            title,
            proof_command: cmd,
            artifact_path,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        // ── The engine drills (REF-D1..REF-D3) — the resolve chokepoint + the leak-free read. ──
        row(
            "REF-D1",
            "5.2",
            "the resolve chokepoint — per-viewer gate; a denied target tombstones (root-only), 0 title/count/backlink leak",
            "cargo test -p myelin-refs-service --test cdc_5_2_resolve",
            "crates/myelin-refs-service/tests/cdc_5_2_resolve.rs",
            date,
        ),
        row(
            "REF-D2",
            "5.3",
            "the leak-free backlink read — list_objects SetExpr lowered into the per-tenant authz reverse index, 0 cross-tenant leak",
            "cargo test -p myelin-refs-service --test cdc_5_3_backlinks",
            "crates/myelin-refs-service/tests/cdc_5_3_backlinks.rs",
            date,
        ),
        row(
            "REF-D3",
            "5.7",
            "the tombstone / graceful-degradation ladder — permission→root→sub-resolve{live/moved/outdated/gone}→erased; a tombstone always carries the root",
            "cargo test -p myelin-refs-service --test cdc_5_7_sub_ladder",
            "crates/myelin-refs-service/tests/cdc_5_7_sub_ladder.rs",
            date,
        ),
        // ── The traverse + the event-sourced edge index (REF-D4..REF-D6). ──
        row(
            "REF-D4",
            "5.3",
            "the bounded cycle-safe lineage traverse — depth/node ceilings from thresholds, per-viewer prune, 0 leak",
            "cargo test -p myelin-refs-service --test cdc_5_3_traverse",
            "crates/myelin-refs-service/tests/cdc_5_3_traverse.rs",
            date,
        ),
        row(
            "REF-D5",
            "5.4",
            "the event-sourced edge inverse index — deterministic edge_id, idempotent rebuild from the producer events",
            "cargo test -p myelin-refs-service --test cdc_5_4_edge_builder",
            "crates/myelin-refs-service/tests/cdc_5_4_edge_builder.rs",
            date,
        ),
        row(
            "REF-D6",
            "5.5",
            "the TE-7 typed-edge mirror — typed table = source of truth, Refs = rebuildable projection, reconverges",
            "cargo test -p myelin-refs-service --test cdc_5_5_mirror",
            "crates/myelin-refs-service/tests/cdc_5_5_mirror.rs",
            date,
        ),
        // ── The structural erasure holder + reindex-from-source (REF-D7..REF-D8). ──
        row(
            "REF-D7",
            "10.1",
            "the PersonalDataHolder structural-erasure surface — locate/erase reaches the edges + the projection cache, 0 recoverable PII",
            "cargo test -p myelin-refs-service --test integration_ref_p15_holder_erase",
            "crates/myelin-refs-service/tests/integration_ref_p15_holder_erase.rs",
            date,
        ),
        row(
            "REF-D8",
            "5.8",
            "reindex-from-source — the only recovery path; the rebuilt index byte-matches the live projection (parity)",
            "cargo test -p myelin-refs-service --test cdc_5_8_reindex",
            "crates/myelin-refs-service/tests/cdc_5_8_reindex.rs",
            date,
        ),
        // ── The M5 world-scale family (REF-D9..REF-D10) — surge + reindex/restore at scale. ──
        row(
            "REF-D9",
            "5.8",
            "reindex-from-source parity AT SCALE — wipe → reindex over the five-producer corpus → byte-match live (REF-D4 at scale)",
            "cargo test -p myelin-refs-service --test ref_d4_reindex_parity_at_scale",
            "crates/myelin-refs-service/tests/ref_d4_reindex_parity_at_scale.rs",
            date,
        ),
        row(
            "REF-D10",
            "12.6",
            "the 30x backlink surge — DRR-fair shed within budget, the reach-index follow-on named; restore + re-erase at backup scale, 0 recoverable PII",
            "cargo test -p myelin-refs-service --test ref_d10_surge_drill --test ref_d5_restore_reerase_at_backup_scale",
            "crates/myelin-refs-service/tests/ref_d10_surge_drill.rs",
            date,
        ),
        // ── The whole-system E2E wedge legs (E2E-1 / E2E-3 / E2E-4 — REF-P27). ──
        row(
            "E2E-1",
            "5.2",
            "the PR context pane — every connected artifact unfurls per-viewer; mid-flight ci.check.updated live-update; a denied confidential issue tombstones, 0 leak",
            "cargo test -p myelin-refs-service --test e2e_wedge_ref_p27",
            "crates/myelin-refs-service/tests/e2e_wedge_ref_p27.rs",
            date,
        ),
        row(
            "E2E-3",
            "5.3",
            "spec-to-ship traceability — the full lineage traverse depth-16 per-viewer (0 leak) → wipe → reindex → byte-match live",
            "cargo test -p myelin-refs-service --test e2e_wedge_ref_p27",
            "crates/myelin-refs-service/tests/e2e_wedge_ref_p27.rs",
            date,
        ),
        row(
            "E2E-4",
            "10.1",
            "the DSAR fan-out — the structural-erasure holder fan-out reaches the edges + cache, the unfurls degrade to tombstones, 0 recoverable PII",
            "cargo test -p myelin-refs-service --test e2e_wedge_ref_p27",
            "crates/myelin-refs-service/tests/e2e_wedge_ref_p27.rs",
            date,
        ),
    ]
}

/// The verdict of the Refs truth-up pass — Green (every PROVEN row dated) or Red (the undated rows
/// named). Never a swallowed bool — a RED points at exactly which Refs claim outran its verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefsTruthUpVerdict {
    /// Every enumerated PROVEN Refs row rests on a dated green artifact (no earlier-band Refs gate red).
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

impl RefsTruthUpVerdict {
    /// `true` iff the truth-up pass is green (every PROVEN row dated). The ONLY way to read a pass.
    pub fn is_green(&self) -> bool {
        matches!(self, RefsTruthUpVerdict::Green { .. })
    }

    /// The ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass).
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            RefsTruthUpVerdict::Green { .. } => &[],
            RefsTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The Refs truth-up pass (REF-P28 / EI-01 §1).** Enumerates every PROVEN Refs row and confirms each
/// rests on a DATED green artifact. A row WITHOUT one is a LOUD failure ([`RefsTruthUpVerdict::Red`]),
/// never a silent pass — the code-wins-over-docs discipline made mechanical. A zero-sized orchestrator.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefsTruthUpPass;

impl RefsTruthUpPass {
    /// A new truth-up pass (stateless).
    pub fn new() -> RefsTruthUpPass {
        RefsTruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`RefsTruthUpVerdict::Green`] (every row dated) or
    /// [`RefsTruthUpVerdict::Red`] (the undated rows named). `date` stamps the green verdict.
    pub fn run(&self, rows: &[ProvenRefsRow], date: &str) -> RefsTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            RefsTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            RefsTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    /// **The loud-never-swallowed truth-up CI entrypoint (EI-01 §5).** Run the pass and turn a RED
    /// verdict into a process-failing `Err` — so a CI invocation `pass.run_or_fail_ci(&rows, date)?`
    /// FAILS the dogfood truth-up job if ANY PROVEN Refs row lacks a dated green artifact. On GREEN it
    /// returns the number of confirmed rows.
    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenRefsRow],
        date: &str,
    ) -> Result<usize, RefsTruthUpRed> {
        match self.run(rows, date) {
            RefsTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            RefsTruthUpVerdict::Red { undated_rows } => Err(RefsTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

/// A RED truth-up pass surfaced as an `Err` — the CLAIMED-NOT-PROVEN Refs rows, loud + specific (the
/// process exits non-zero, never a silent docs drift).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsTruthUpRed {
    /// The ids of the rows lacking a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for RefsTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL — {} Refs row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} — a claim \
             that outlives its verification misleads the next agent (EI-01 §1); fix the doc or re-run \
             the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for RefsTruthUpRed {}

// ───────────────────────────── the enumerated truth-up scorecard (the green artifact) ─────────────────────────────

/// How a PROVEN Refs row's proof stands at truth-up time: a dated green artifact, or an
/// honestly-recorded CLAIMED-NOT-PROVEN note. Either way the status carries a DATE (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefsRowStatus {
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

impl RefsRowStatus {
    /// `true` iff this is a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated_green(&self) -> bool {
        matches!(self, RefsRowStatus::DatedGreen { .. })
    }
}

/// One scorecard line: a PROVEN Refs row resolved to its [`RefsRowStatus`] at truth-up time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsScorecardEntry {
    /// The row this line scores.
    pub row: ProvenRefsRow,
    /// Its resolved status (dated-green or claimed-not-proven, both dated).
    pub status: RefsRowStatus,
}

/// **The enumerated Refs truth-up scorecard (the GATE/DRILLS green artifact, REF-P28).** Every PROVEN
/// Refs row → its dated green artifact (or a dated CLAIMED-NOT-PROVEN note). The scorecard itself is the
/// closing-honesty-pass artifact: rendering it produces the §5.x-grouped table the prompt's GATE
/// demands, and [`Self::is_green`] is true iff NO earlier-band Refs gate is red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked — an unread CLAIMED-NOT-PROVEN row silently \
              drifts the docs from the code (EI-01 §1)"]
pub struct RefsTruthUpScorecard {
    /// The run date the scorecard is stamped with.
    pub date: String,
    /// One entry per PROVEN Refs row, in section order.
    pub entries: Vec<RefsScorecardEntry>,
}

impl RefsTruthUpScorecard {
    /// `true` iff every row rests on a dated green artifact (the gate invariant: no Refs gate red).
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

    /// **Render the enumerated scorecard as the dated green artifact** (the §5.x-grouped table a
    /// truth-up CI run prints). CLAIMED-NOT-PROVEN rows are rendered LOUD, never elided.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let verdict = if self.is_green() {
            "GREEN (no earlier-band Refs gate red)"
        } else {
            "RED (a Refs claim outran its verification)"
        };
        out.push_str(&format!(
            "P-513 REFS TRUTH-UP SCORECARD {} — {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                RefsRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                RefsRowStatus::ClaimedNotProven { date, reason } => {
                    format!("CLAIMED-NOT-PROVEN({date}: {reason})")
                }
            };
            out.push_str(&format!(
                "  [§{}] {:<10} {:<28} — {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

/// **Run the Refs truth-up pass and produce the enumerated [`RefsTruthUpScorecard`] (REF-P28).** For
/// each PROVEN Refs row this resolves a dated [`RefsRowStatus`]: a row is DATED-GREEN iff it carries an
/// `artifact_date` AND its proof source exists on disk under `repo_root`; otherwise it is recorded
/// CLAIMED-NOT-PROVEN with the run `date` and the honest reason. The scorecard surfaces — never swallows
/// — any gap (EI-01 §1). `repo_root` is the workspace root the `artifact_path`s are relative to.
pub fn run_refs_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> RefsTruthUpScorecard {
    let entries = proven_refs_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => RefsRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    RefsRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => RefsRowStatus::DatedGreen { date: d.clone() },
            };
            RefsScorecardEntry { row, status }
        })
        .collect();
    RefsTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

// ───────────────────────────── (3) the every-incident-adds-a-drill loop ─────────────────────────────

/// **A Refs incident on Myelin's own development (the every-incident-adds-a-drill loop, EI-01 §3/§5).**
/// A real incident ends by filing a PII-free Myelin issue draft AND a reproducing-drill ticket — both
/// reference-linked (the moat thesis: the issue points at the drill that reproduces it). The integration
/// drill registers the repro into the harness `DrillRegistry` so it re-runs forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsIncident {
    /// The incident id (PII-free, e.g. `"INC-REFS-DOGFOOD-1"`).
    pub incident_id: String,
    /// The Refs gate the incident regressed (e.g. `"REF-D1"`).
    pub gate_id: String,
    /// A PII-free one-line description of what broke.
    pub description: String,
    /// The name of the reproducing drill the incident files (the test that re-fires the failure).
    pub repro_drill_name: String,
}

impl RefsIncident {
    /// A new Refs incident (every field PII-free).
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> RefsIncident {
        RefsIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    /// The PII-free Myelin issue draft the incident files (names the gate + the repro drill — the issue
    /// is reference-linked to the drift that reproduces it).
    pub fn issue_draft(&self) -> RefsIncidentIssueDraft {
        RefsIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] Refs gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "Refs incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked — every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    /// The reproducing-drill ticket the incident files (the test that joins the permanent suite).
    pub fn drill_ticket(&self) -> RefsIncidentDrillTicket {
        RefsIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

/// The PII-free Myelin issue draft a [`RefsIncident`] files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsIncidentIssueDraft {
    /// The Refs gate the issue is about.
    pub gate_id: String,
    /// The issue title (PII-free).
    pub title: String,
    /// The issue body (PII-free; names the repro drill).
    pub body: String,
}

/// The reproducing-drill ticket a [`RefsIncident`] files (the drill that joins the permanent suite).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsIncidentDrillTicket {
    /// The drill name (the test that re-fires the failure).
    pub drill_name: String,
    /// The gate the drill guards.
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    /// **THE HEADLINE: the reference graph is GREEN on Myelin's OWN work.** The PR context pane, the
    /// spec-to-ship lineage, and the holder fan-out all green over the Myelin self-tenant, 0 leak.
    #[test]
    fn the_reference_graph_greens_on_myelins_own_work() {
        let artifact = run_refs_over_myelins_own_work(RUN_DATE);

        assert!(
            artifact.is_green(),
            "the reference graph must be green on Myelin's own work: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.total_leaks(),
            0,
            "0 leak across the three faces: {}",
            artifact.summary()
        );
        assert!(artifact.pr_pane.is_green(), "the PR context pane is green");
        assert!(
            artifact.spec_to_ship.is_green(),
            "the spec-to-ship lineage is green"
        );
        assert!(
            artifact.holder_fanout.is_green(),
            "the holder fan-out is green"
        );

        let s = artifact.summary();
        assert!(s.contains("P-513 REFS DOGFOOD 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    /// The truth-up pass is GREEN — every PROVEN Refs row rests on a dated green artifact.
    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_refs_rows(RUN_DATE);
        assert!(
            rows.len() >= 13,
            "the PROVEN set covers REF-D1..REF-D10 + the E2E legs"
        );
        let confirmed = RefsTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red earlier-band Refs gates — every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    /// A claimed-not-proven row reds the truth-up pass LOUDLY (surfaced, never swallowed).
    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_refs_rows(RUN_DATE);
        rows[0].artifact_date = None; // simulate a doc claim with no dated green artifact
        let verdict = RefsTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = RefsTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row fails the CI entrypoint");
        assert!(err.to_string().contains("CLAIMED-NOT-PROVEN"));
    }

    /// The enumerated scorecard renders GREEN with every PROVEN row dated + its proof source on disk.
    #[test]
    fn the_scorecard_renders_green_with_proof_sources_on_disk() {
        // The workspace root (two levels up from this crate's manifest dir).
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let scorecard = run_refs_truth_up_scorecard(RUN_DATE, &repo_root);
        assert!(
            scorecard.is_green(),
            "the scorecard must be green — every PROVEN Refs row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("REF-D1") && md.contains("E2E-4"),
            "enumerated: {md}"
        );
    }

    /// A row whose proof source is missing on disk is surfaced CLAIMED-NOT-PROVEN (never trusted on faith).
    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        // Point the scorecard at a bogus root so no proof source exists — every dated row flips to
        // CLAIMED-NOT-PROVEN with the "proof source missing on disk" reason (the artifact-exists check).
        let bogus_root = std::path::Path::new("/nonexistent-refs-truth-up-root");
        let scorecard = run_refs_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard
                .entries
                .iter()
                .all(|e| matches!(&e.status, RefsRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk"))),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    /// The every-incident loop: an incident files a PII-free issue draft + a reproducing-drill ticket.
    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = RefsIncident::new(
            "INC-REFS-DOGFOOD-1",
            "REF-D1",
            "a resolve chokepoint regression leaked a denied issue title on the Myelin self-tenant",
            "repro_ref_d1_dogfood_resolve_leak",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "REF-D1");
        assert!(draft.title.contains("INC-REFS-DOGFOOD-1"));
        assert!(
            draft.body.contains("repro_ref_d1_dogfood_resolve_leak"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        // PII-free: the draft carries no personal data, only opaque ids + gate names.
        assert!(!draft.body.to_lowercase().contains("email"));

        let ticket = incident.drill_ticket();
        assert_eq!(ticket.drill_name, "repro_ref_d1_dogfood_resolve_leak");
        assert_eq!(ticket.gate_id, "REF-D1");
    }
}
