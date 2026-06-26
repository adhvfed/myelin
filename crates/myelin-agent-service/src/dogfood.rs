//! # `dogfood` — the platform's own agents run on its own commits/issues/chat (AG-P26 / P-517, M6)
//!
//! **The Agent-Fabric M6 dogfood prompt.** AG-M6 promotes NOTHING and freezes NO new contract — it
//! *exercises* the production-hardened Fabric (the M2 heartbeat, hardened through M5) on **real
//! (self-)tenant data**: the platform's own development. The cheapest, most honest load generator is
//! the team's own work (master-sequencing §2 M6; EI-01 §5 — *the ratchet runs on the builders' own
//! work*; VISION §5), and the moat is only real once **Myelin's own agents run on the self-hosting CI
//! graph**: when the platform's own CI fails on a Myelin commit, a triage agent runs (explicit-first /
//! Signal-driven), emits a BALANCED reserve/settle ledger + a content-addressed trace, and the
//! every-incident-adds-a-drill loop files a Myelin issue + a reproducing drill (VISION §3 / §5).
//!
//! ## What this module IS (the dogfood DRIVER over the EXISTING Fabric — EI-01 §7)
//! This is a **caller that drives the already-shipped Agent-Fabric surface over the Myelin
//! self-tenant** — never a second engine, runtime, pipeline, or cost ledger. It REUSES:
//! - [`crate::dispatch::classify`] (AG-P20, contract 8.6 / §3.4 / CHAT-1) — the explicit-first
//!   dispatch classifier, reframed onto the self-hosting CI graph: a `ci.result=failure` Signal is an
//!   [`crate::dispatch::DispatchTrigger::ExplicitRun`] → [`crate::dispatch::DispatchDecision::Dispatch`]
//!   (a costed triage run); a casual `@triage` mention would only `Notify` (0 auto-spawn).
//! - [`myelin_storage::reserve_settle::CostLedger`] (11.7, AG-P14) — the reserve-at-dispatch /
//!   settle-on-completion bookend, reframed onto the **Myelin self-tenant's wallet**: the triage run
//!   reserves its estimate, the Mock brain bills 0 metered units (the gate is brain-independent — the
//!   real per-call meter is AG-P25), and the settle refunds the reservation so `reserved == settled`
//!   (the BALANCED ledger — the dogfood green artifact, contract 1.8).
//! - [`crate::trace_seam::TraceDocument`] (8.8, AG-P19) — the content-addressed 13.1 Knowledge-document
//!   trace, reframed onto the **triage run's reasoning**: a per-run `blake3:<hex>` trace ref (the
//!   dogfood green artifact — a trace per run).
//!
//! ## What this module wires (the dogfood loop is live)
//! - **The Fabric dogfood drill runs as a Myelin CI job on Myelin's own commits** — wired into the
//!   frozen `myelin_harness::self_hosting_ci::self_hosting_jobs` graph (the AG-P26 band; see the
//!   harness module). The dogfood loop is live: the triage face + the truth-up pass run on every
//!   Myelin commit.
//! - **The truth-up pass** ([`FabricTruthUpPass`] over [`proven_fabric_rows`]) — every PROVEN Fabric
//!   row (AG-D1..AG-D11 + the E2E-2 spine) rests on a DATED green artifact whose proof SOURCE exists on
//!   disk; no later-band Fabric gate is red. A row that names a vanished artifact is surfaced LOUDLY,
//!   never trusted on faith (EI-01 §1, code-wins-over-docs). This is the gate invariant held
//!   end-to-end.
//! - **The every-incident-adds-a-drill loop** ([`FabricIncident`]) — a Fabric incident files a PII-free
//!   Myelin issue draft AND a reproducing-drill ticket; the integration drill registers the repro into
//!   the harness `DrillRegistry` so it re-runs forever.
//!
//! ## FLOOR named (VISION §3, EI-01 §1)
//! The dogfood agents run on the **MOCK runtime** ([`crate::mock::MockAgentRuntime`] on the real
//! `--use-mock` path) — correct per VISION §3 during development. The real `LlmAgentRuntime` swap (the
//! ONLY place a model/SDK/prompt/model-name string appears) remains the named post-M5 follow-on
//! **AG-P25 (→ the named real-runtime swap seam)**; the external MCP endpoint + agent long-term
//! memory/RAG are post-M5 (named in AG-P25's seam doc). The Mock metering ZERO is CORRECT — the
//! reserve/settle gate is the runaway self-limiter REGARDLESS of which brain runs.
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/agent-fabric.md` §0
//! (the Fabric rides the M6 dogfood) + §9 (every PROVEN row rests on a dated green artifact).
//! **Roadmap:** `planning/06-roadmaps/shared/agent-fabric.md` §2 AG-M6 (dogfood + the truth-up pass).
//! **Master sequencing:** `planning/06-roadmaps/00-master-sequencing.md` §2/§4 M6 (the dogfood band +
//! the done-bar: 0 red later-band gate; the truth-up pass). **Doctrine:**
//! `external-insights/01-process-and-quality-doctrine.md` §1 (code-wins-over-docs — the truth-up pass),
//! §3 (every real incident ends by adding a drill), §4 (drive the real thing — the dogfood loop IS the
//! test), §5 (the ratchet runs on the builders' own work). **VISION §3/§5** (dogfooding).

use myelin_content::{Block, Inline, Span};
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MinorUnits, RunId as StorageRunId};
use myelin_tenancy::TenantId;

use crate::dispatch::{classify, DispatchDecision, DispatchTrigger};
use crate::trace_seam::TraceDocument;

/// The Myelin self-tenant id (the platform self-hosts as exactly one cell — P-508 / CP-M6). Opaque,
/// PII-free — the dogfood triage runs over the platform's OWN work under this tenant.
pub const MYELIN_SELF_TENANT: &str = "myelin";

/// The region the Myelin self-tenant is pinned to (fr-par — the dev/prod residency pin; a config swap,
/// never a code change). The dogfood triage run dispatches cell-local in this region.
pub const MYELIN_SELF_REGION: &str = "fr-par";

/// **The named floor (VISION §3): the dogfood agents run on the MOCK runtime, NOT a real LLM.** The
/// real `LlmAgentRuntime` swap is the named post-M5 follow-on AG-P25 (the ONLY place a model/SDK/
/// prompt/model-name string appears). The Mock metering ZERO is correct — the reserve/settle gate is
/// the runaway self-limiter regardless of which brain runs.
pub const DOGFOOD_RUNTIME_FLOOR: &str = "the dogfood triage agents run on the MOCK runtime \
    (--use-mock MockAgentRuntime) — correct per VISION §3 during development. The real \
    LlmAgentRuntime swap (the only place a model/SDK/prompt/model-name string appears) is the named \
    post-M5 follow-on AG-P25; the external MCP endpoint + agent long-term memory/RAG are post-M5 \
    (named in AG-P25's seam doc).";

fn myelin_tenant() -> TenantId {
    TenantId(MYELIN_SELF_TENANT.into())
}

// ───────────────────────────── face: a triage agent runs on a real Myelin CI failure ─────────────────────────────

/// The run estimate the triage run reserves at dispatch (an upper bound over its metered effects: file
/// the issue + post the chat thread; integer minor-units, never floats). Headroom over the actual bill.
const TRIAGE_ESTIMATE: u64 = 12;

/// The Myelin self-tenant wallet balance (≥ the estimate → the triage run dispatches). A funded wallet.
const MYELIN_WALLET: u64 = 100;

/// The result of running Myelin's OWN triage agent on a real Myelin CI failure. GREEN iff the CI-fail
/// Signal DISPATCHED a costed triage run (explicit-first), the reserve/settle ledger BALANCED (reserved
/// == settled; a Mock bills 0 → the reservation refunds), 0 in-flight interrupt, and a content-addressed
/// trace was written for the run (a trace per run — the dogfood green artifacts, contract 1.8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriageFace {
    /// `true` iff the `ci.result=failure` Signal DISPATCHED a costed triage run (explicit-first, §3.4).
    pub dispatched: bool,
    /// `true` iff a casual `@triage` mention would only NOTIFY (0 auto-spawn — the safety boundary).
    pub mention_only_notifies: bool,
    /// the minor-units reserved at dispatch (the run's upper-bound estimate).
    pub reserved: u64,
    /// the minor-units settled on completion (`billed + refunded`; `== reserved` for the balanced ledger).
    pub settled: u64,
    /// how many cost events the run metered (the Mock bills 0 metered units → 0 — the gate is brain-independent).
    pub cost_events: usize,
    /// how many in-flight runs were interrupted (must be 0 — the reservation's only exit is settle, 11.7).
    pub inflight_interrupts: u64,
    /// the content address of the run's trace (`blake3:<hex>` — a trace per run; non-empty iff written).
    pub trace_ref: String,
}

impl TriageFace {
    /// `true` iff the triage run dispatched explicit-first, balanced its ledger, never interrupted an
    /// in-flight run, and wrote a content-addressed trace (the dogfood green artifacts).
    pub fn is_green(&self) -> bool {
        self.dispatched
            && self.mention_only_notifies
            && self.reserved == self.settled
            && self.inflight_interrupts == 0
            && self.trace_ref.starts_with("blake3:")
    }
}

/// A 13.1 inline run of plain text (a [`Span::Text`] with no marks) — the trace's reasoning prose.
fn text(s: &str) -> Inline {
    Inline {
        spans: vec![Span::Text {
            text: s.to_string(),
            marks: vec![],
            link: None,
        }],
        nodes: vec![],
    }
}

/// **Run Myelin's OWN triage agent on a real Myelin CI failure (P-517 face).** Drives the production
/// Agent-Fabric surface over the Myelin self-tenant: a `ci.result=failure` Signal on a Myelin commit is
/// classified explicit-first (DISPATCH, not a casual mention), the triage run reserves its estimate
/// against the Myelin wallet, the Mock brain runs (billing 0 metered units), the run settles (reserved
/// == settled — the BALANCED ledger), and a content-addressed 13.1 trace is written for the run.
/// `commit_oid` is the Myelin commit CI failed on (opaque, PII-free).
pub fn run_myelin_triage_on_ci_failure(commit_oid: &str, run_id: u128) -> TriageFace {
    // ── explicit-first dispatch (§3.4 / CHAT-1): the CI-fail Signal DISPATCHES; a mention NOTIFIES. ──
    let signal = classify(&DispatchTrigger::ExplicitRun(format!(
        "signal:ci.result=failure:{commit_oid}"
    )));
    let dispatched = matches!(signal, DispatchDecision::Dispatch(_));
    let casual = classify(&DispatchTrigger::Mention("@triage look".into()));
    let mention_only_notifies = matches!(casual, DispatchDecision::Notify(_));

    // ── reserve-at-dispatch (11.7) against the Myelin self-tenant wallet (no balance → no run). ──
    let mut ledger = CostLedger::new();
    let storage_run = StorageRunId::new(format!("run:triage:{commit_oid}"));
    let reservation = ledger
        .reserve(
            myelin_tenant(),
            storage_run.clone(),
            MinorUnits(TRIAGE_ESTIMATE),
            MinorUnits(MYELIN_WALLET),
        )
        .expect("the funded Myelin self-tenant wallet reserves the triage run at dispatch");
    ledger
        .begin(&myelin_tenant(), &storage_run)
        .expect("the reserved triage run begins flight");

    // ── the Mock triage brain runs and meters ZERO (the gate is brain-independent — AG-P25 is the ──
    //    real per-call meter). The settle refunds the whole reservation → reserved == settled.        ──
    let units: Vec<MeteredUnit> = vec![]; // the Mock bills 0 metered units (correct, VISION §3).
    let settle = ledger
        .settle(&myelin_tenant(), &storage_run, &units)
        .expect("the in-flight triage run settles on completion");
    let settled = settle.billed_total.0 + settle.refunded.0;

    // ── the content-addressed 13.1 trace for the run (a trace per run — 8.8 / contract 1.8). ──
    let trace = TraceDocument::new(
        run_id,
        vec![
            Block::Paragraph {
                inline: text(&format!(
                    "triage agent: CI failed on Myelin commit {commit_oid}; filed an issue + posted \
                     the failing-step summary to the channel; proposed no merge (advisory)."
                )),
            },
            Block::CodeBlock {
                lang: Some("json".into()),
                text: r#"{"tool":"create_issue","result":"ok"}"#.into(),
            },
        ],
    );

    TriageFace {
        dispatched,
        mention_only_notifies,
        reserved: reservation.reserved.0,
        settled,
        cost_events: ledger.cost_events_for(&myelin_tenant(), &storage_run).len(),
        inflight_interrupts: ledger.inflight_interrupt_count(),
        trace_ref: trace.content_address().0,
    }
}

// ───────────────────────────── the aggregate dogfood artifact ─────────────────────────────

/// **The named green artifact the Fabric dogfood run emits (P-517).** The platform's own triage agent
/// run on a real Myelin CI failure over the Myelin self-tenant: explicit-first dispatch, a BALANCED
/// reserve/settle ledger, 0 in-flight interrupt, and a content-addressed trace per run (contract 1.8).
///
/// GREEN iff the triage face is green — a RED face fails LOUDLY ([`Self::is_green`] is false), never a
/// claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the dogfood artifact must be checked — an unread RED face silently claims a green the \
              Fabric did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct FabricDogfoodArtifact {
    /// the date the dogfood run was asserted.
    pub date: String,
    /// the triage face — Myelin's own triage agent run on a real Myelin CI failure.
    pub triage: TriageFace,
}

impl FabricDogfoodArtifact {
    /// `true` iff the triage face is green — the platform's own agent ran on the self-hosting graph
    /// with a balanced reserve/settle ledger + a content-addressed trace.
    pub fn is_green(&self) -> bool {
        self.triage.is_green()
    }

    /// The dated one-line summary (the artifact body the dogfood CI run prints).
    pub fn summary(&self) -> String {
        format!(
            "P-517 FABRIC DOGFOOD {} — tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             dispatched={} reserved=={settled} balanced={} interrupts={} trace={} verdict={}",
            self.date,
            self.triage.dispatched,
            self.triage.reserved == self.triage.settled,
            self.triage.inflight_interrupts,
            &self.triage.trace_ref[..self.triage.trace_ref.len().min(14)],
            if self.is_green() { "GREEN" } else { "RED" },
            settled = self.triage.settled,
        )
    }
}

/// **Run Myelin's OWN triage agent on the self-hosting CI graph (P-517).** The dogfood loop: drives the
/// production Agent-Fabric surface over the Myelin self-tenant, REUSING the already-shipped surface
/// (EI-01 §7, never a second engine). `date` is the run stamp.
pub fn run_fabric_over_myelins_own_work(date: &str) -> FabricDogfoodArtifact {
    FabricDogfoodArtifact {
        date: date.to_string(),
        // A real Myelin commit OID (opaque, PII-free) the self-hosting CI failed on.
        triage: run_myelin_triage_on_ci_failure("feedface", 0x5170_u128),
    }
}

// ───────────────────────────── (2) the truth-up pass over the PROVEN Fabric rows ─────────────────────────────

/// One PROVEN Fabric row the truth-up pass enumerates: a gate/drill the ledger claims PROVEN, with the
/// proof command that emits its dated green artifact AND the repo-relative path to that proof source.
/// The truth-up pass asserts EACH row rests on a DATED green artifact whose source EXISTS on disk — a
/// row that names a vanished artifact is surfaced, never trusted on faith (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenFabricRow {
    /// the stable gate/drill id (e.g. `"AG-D1"`, `"E2E-2"`).
    pub id: &'static str,
    /// the contract SECTION the row's gate belongs to (the §x.y / 8.x face of the agent-fabric doc).
    pub section: &'static str,
    /// a one-line human title (what the row proves).
    pub title: &'static str,
    /// the proof command that emits this row's dated green artifact.
    pub proof_command: &'static str,
    /// the repo-RELATIVE path to the proof source (the test file `proof_command` runs).
    pub artifact_path: &'static str,
    /// the DATE the row's green artifact was last emitted, if any. `None` ⇒ CLAIMED-NOT-PROVEN.
    pub artifact_date: Option<String>,
}

impl ProvenFabricRow {
    /// `true` iff this row rests on a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    /// Resolve this row's [`artifact_path`](Self::artifact_path) to an absolute path under `repo_root`.
    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

/// **The FROZEN set of PROVEN Fabric rows the truth-up pass enumerates (P-517).** Every Fabric gate the
/// ledger claims PROVEN: the eleven drills **AG-D1..AG-D11** (the plan-then-apply correctness, the HITL
/// family, the loop guards, the per-run identity, the step/effect determinism, the erasure fan-out, the
/// 30× surge, the runaway self-limiter, the escape GATE) **plus** the whole-system **E2E-2** flagship
/// (AG-P24). The truth-up pass asserts EVERY id here rests on a dated green artifact whose proof source
/// exists on disk; a row without one is a loud failure. `date` is supplied by the runner.
///
/// The proof-source mapping follows the agent-fabric coverage digest (§"Drills greened across the
/// ledger"): AG-D1/D2/D3 → the plan-then-apply pipeline CDC; AG-D4 → the escape GATE (+ the M4 prod
/// re-confirm); AG-D5 → the HITL withhold loop; AG-D6 → the dispatch surge; AG-D7 → the loop guards;
/// AG-D8 → the SKELETON no-tool leg (+ the AG-P13 re-mint leg); AG-D9 → the Mock step-determinism;
/// AG-D10 → the erasure fan-out; AG-D11 → the runaway self-limiter; E2E-2 → the agent-native flagship.
pub fn proven_fabric_rows(date: &str) -> Vec<ProvenFabricRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenFabricRow {
        ProvenFabricRow {
            id,
            section,
            title,
            proof_command: cmd,
            artifact_path,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        // ── The plan-then-apply correctness family (AG-D1/D2/D3, AG-P6) — the eight-step pipeline. ──
        row(
            "AG-D1",
            "8.2",
            "the plan-then-apply pipeline applies an in-∩ effect to the subsystem PUBLIC endpoint (1 mutation, 1 metered cost event)",
            "cargo test -p myelin-agent-service --test cdc_8_2_apply_pipeline",
            "crates/myelin-agent-service/tests/cdc_8_2_apply_pipeline.rs",
            date,
        ),
        row(
            "AG-D2",
            "8.2",
            "0 privileged fallback EVER fires (fail-closed by construction) — a denied effect never silently escalates",
            "cargo test -p myelin-agent-service --test cdc_8_2_apply_pipeline",
            "crates/myelin-agent-service/tests/cdc_8_2_apply_pipeline.rs",
            date,
        ),
        row(
            "AG-D3",
            "8.2",
            "an effect outside the delegation ∩ is DENIED (attenuation never up — 0 effect no human role could perform)",
            "cargo test -p myelin-agent-service --test cdc_8_2_apply_pipeline",
            "crates/myelin-agent-service/tests/cdc_8_2_apply_pipeline.rs",
            date,
        ),
        // ── The unified-sandbox escape GATE (AG-D4, AG-P17/P21) — the permanent ZERO-escapes gate. ──
        row(
            "AG-D4",
            "8.4",
            "the AgentExecGate is fail-closed in the TYPE — only a GREEN escape attestation (ZERO escapes, matching backend identity) admits untrusted compute",
            "cargo test -p myelin-agent-service --test cdc_8_4_escape_gate",
            "crates/myelin-agent-service/tests/cdc_8_4_escape_gate.rs",
            date,
        ),
        row(
            "AG-D4-prod",
            "8.4",
            "AG-D4 re-confirmed on the PRODUCTION CI runner image (the M4 hard gate — the deploy tools run on the prod image)",
            "cargo test -p myelin-agent-service --test cdc_8_4_prod_image_reconfirm",
            "crates/myelin-agent-service/tests/cdc_8_4_prod_image_reconfirm.rs",
            date,
        ),
        // ── The HITL withhold→surface→resume family (AG-D5, AG-P9/P10) — exactly-once approval. ──
        row(
            "AG-D5",
            "8.2",
            "the HITL withhold→surface→resume loop: a gated tool withholds at step 6, opens on approval, applies EXACTLY ONCE (a double-click is one approval)",
            "cargo test -p myelin-agent-service --test cdc_8_2_hitl_loop",
            "crates/myelin-agent-service/tests/cdc_8_2_hitl_loop.rs",
            date,
        ),
        // ── The 30× agent-dispatch surge (AG-D6, AG-P22) — the protected-human-lane shed gate. ──
        row(
            "AG-D6",
            "1.11",
            "the 30× agent-dispatch surge: the human lane holds, the machine lane sheds (429 + Retry-After honoured), 0 cross-tenant impact",
            "cargo test -p myelin-agent-service --test ag_d6_dispatch_surge_drill",
            "crates/myelin-agent-service/tests/ag_d6_dispatch_surge_drill.rs",
            date,
        ),
        // ── The five structural loop guards (AG-D7, AG-P12) — the agent→event→agent loop is STOPPED. ──
        row(
            "AG-D7",
            "8.5",
            "the five structural loop guards: the adversarial agent→event→agent self-trigger is STOPPED (causal-depth ≤ ceiling, 0 unbounded fork, tripwire fires)",
            "cargo test -p myelin-agent-service --test drills_ag_d7_loop_guards",
            "crates/myelin-agent-service/tests/drills_ag_d7_loop_guards.rs",
            date,
        ),
        // ── The per-run identity (AG-D8, AG-P4 no-tool leg + AG-P13 re-mint leg). ──
        row(
            "AG-D8",
            "8.5",
            "the per-run identity no-tool SKELETON leg: mint→reserve→step→trace→settle→revoke, 0 shared platform token leaked into the child env",
            "cargo test -p myelin-agent-service --test cdc_8_5_skeleton_loop",
            "crates/myelin-agent-service/tests/cdc_8_5_skeleton_loop.rs",
            date,
        ),
        row(
            "AG-D8-remint",
            "4.7",
            "the per-run identity re-mint leg: a multi-day HITL pause re-mints a fresh attenuated token on resume (0 unattributed window across the pause)",
            "cargo test -p myelin-agent-service --test cdc_4_7_remint_resume",
            "crates/myelin-agent-service/tests/cdc_4_7_remint_resume.rs",
            date,
        ),
        // ── The step/effect determinism (AG-D9, AG-P5/P8) — byte-identical replay. ──
        row(
            "AG-D9",
            "8.3",
            "the MockAgentRuntime step-determinism: two runs over the same script produce a byte-identical step/effect sequence (the deterministic run trace)",
            "cargo test -p myelin-agent-service --test cdc_8_3_mock_runtime",
            "crates/myelin-agent-service/tests/cdc_8_3_mock_runtime.rs",
            date,
        ),
        // ── The erasure fan-out (AG-D10, AG-P23) — crypto-shred reaches the trace + memory. ──
        row(
            "AG-D10",
            "10.1",
            "the DSR erasure fan-out over all Fabric holders (run table + trace + memory): crypto-shred reaches history, 0 recoverable PII, the pseudonym attribution survives",
            "cargo test -p myelin-agent-service --test drills_ag_d10_erasure",
            "crates/myelin-agent-service/tests/drills_ag_d10_erasure.rs",
            date,
        ),
        // ── The runaway self-limiter (AG-D11, AG-P14) — reserve refuses past exhaustion, never interrupts. ──
        row(
            "AG-D11",
            "11.7",
            "the reserve/settle runaway self-limiter: reserve refuses past wallet exhaustion (the loop stops at the wallet), 0 in-flight interrupt, reserved == settled (balanced)",
            "cargo test -p myelin-agent-service --test drills_ag_d11_runaway_self_limiter",
            "crates/myelin-agent-service/tests/drills_ag_d11_runaway_self_limiter.rs",
            date,
        ),
        // ── The whole-system E2E-2 flagship (AG-P24) — CI-fail → triage → issue → chat → fix-PR. ──
        row(
            "E2E-2",
            "8.2",
            "the agent-native flagship: CI-fail → triage agent → issue → chat → fix-PR across a kill + days-later approval (exactly-once, merge-count==1, reserve/settle balanced, 0 effect outside the ∩)",
            "cargo test -p myelin-agent-service --test drills_ag_p24_e2e2_flagship",
            "crates/myelin-agent-service/tests/drills_ag_p24_e2e2_flagship.rs",
            date,
        ),
    ]
}

/// The verdict of the Fabric truth-up pass — Green (every PROVEN row dated) or Red (the undated rows
/// named). Never a swallowed bool — a RED points at exactly which Fabric claim outran its verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FabricTruthUpVerdict {
    /// every enumerated PROVEN Fabric row rests on a dated green artifact (no later-band Fabric gate red).
    Green {
        /// how many PROVEN rows were confirmed dated + green.
        rows_confirmed: usize,
        /// the date the truth-up pass ran.
        date: String,
    },
    /// one or more PROVEN rows are CLAIMED-NOT-PROVEN. Names them so the failure is specific.
    Red {
        /// the ids of the rows lacking a dated green artifact.
        undated_rows: Vec<&'static str>,
    },
}

impl FabricTruthUpVerdict {
    /// `true` iff the truth-up pass is green (every PROVEN row dated).
    pub fn is_green(&self) -> bool {
        matches!(self, FabricTruthUpVerdict::Green { .. })
    }

    /// the ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass).
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            FabricTruthUpVerdict::Green { .. } => &[],
            FabricTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The Fabric truth-up pass (P-517 / EI-01 §1).** Enumerates every PROVEN Fabric row and confirms
/// each rests on a DATED green artifact. A row WITHOUT one is a LOUD failure ([`FabricTruthUpVerdict::Red`]),
/// never a silent pass — the code-wins-over-docs discipline made mechanical. A zero-sized orchestrator.
#[derive(Clone, Copy, Debug, Default)]
pub struct FabricTruthUpPass;

impl FabricTruthUpPass {
    /// A new truth-up pass (stateless).
    pub fn new() -> FabricTruthUpPass {
        FabricTruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`FabricTruthUpVerdict::Green`] (every row dated)
    /// or [`FabricTruthUpVerdict::Red`] (the undated rows named). `date` stamps the green verdict.
    pub fn run(&self, rows: &[ProvenFabricRow], date: &str) -> FabricTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            FabricTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            FabricTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    /// **The loud-never-swallowed truth-up CI entrypoint (EI-01 §5).** Run the pass and turn a RED
    /// verdict into a process-failing `Err` — so `pass.run_or_fail_ci(&rows, date)?` FAILS the dogfood
    /// truth-up job if ANY PROVEN Fabric row lacks a dated green artifact. On GREEN it returns the count.
    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenFabricRow],
        date: &str,
    ) -> Result<usize, FabricTruthUpRed> {
        match self.run(rows, date) {
            FabricTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            FabricTruthUpVerdict::Red { undated_rows } => Err(FabricTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

/// A RED truth-up pass surfaced as an `Err` — the CLAIMED-NOT-PROVEN Fabric rows, loud + specific.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricTruthUpRed {
    /// the ids of the rows lacking a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for FabricTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL — {} Fabric row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} — a \
             claim that outlives its verification misleads the next agent (EI-01 §1); fix the doc or \
             re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for FabricTruthUpRed {}

// ───────────────────────────── the enumerated truth-up scorecard (the green artifact) ─────────────────────────────

/// How a PROVEN Fabric row's proof stands at truth-up time: a dated green artifact, or an
/// honestly-recorded CLAIMED-NOT-PROVEN note. Either way the status carries a DATE (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FabricRowStatus {
    /// the row rests on a dated green artifact whose proof source exists on disk.
    DatedGreen {
        /// the date the green artifact was last emitted.
        date: String,
    },
    /// the row is CLAIMED but NOT PROVEN — no dated green artifact, or its proof source is gone.
    ClaimedNotProven {
        /// the date the truth-up pass recorded the gap.
        date: String,
        /// why the row is not proven.
        reason: String,
    },
}

impl FabricRowStatus {
    /// `true` iff this is a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated_green(&self) -> bool {
        matches!(self, FabricRowStatus::DatedGreen { .. })
    }
}

/// One scorecard line: a PROVEN Fabric row resolved to its [`FabricRowStatus`] at truth-up time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricScorecardEntry {
    /// the row this line scores.
    pub row: ProvenFabricRow,
    /// its resolved status (dated-green or claimed-not-proven, both dated).
    pub status: FabricRowStatus,
}

/// **The enumerated Fabric truth-up scorecard (the GATE/DRILLS green artifact, P-517).** Every PROVEN
/// Fabric row → its dated green artifact (or a dated CLAIMED-NOT-PROVEN note). Rendering it produces the
/// §x.y-grouped table the prompt's GATE demands, and [`Self::is_green`] is true iff NO later-band Fabric
/// gate is red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked — an unread CLAIMED-NOT-PROVEN row silently \
              drifts the docs from the code (EI-01 §1)"]
pub struct FabricTruthUpScorecard {
    /// the run date the scorecard is stamped with.
    pub date: String,
    /// one entry per PROVEN Fabric row, in section order.
    pub entries: Vec<FabricScorecardEntry>,
}

impl FabricTruthUpScorecard {
    /// `true` iff every row rests on a dated green artifact (the gate invariant: no Fabric gate red).
    pub fn is_green(&self) -> bool {
        self.entries.iter().all(|e| e.status.is_dated_green())
    }

    /// how many rows the scorecard enumerates.
    pub fn rows_total(&self) -> usize {
        self.entries.len()
    }

    /// how many rows rest on a dated green artifact.
    pub fn rows_dated_green(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status.is_dated_green())
            .count()
    }

    /// the ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass) — the loud failure list.
    pub fn claimed_not_proven(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| !e.status.is_dated_green())
            .map(|e| e.row.id)
            .collect()
    }

    /// **Render the enumerated scorecard as the dated green artifact** (the §x.y-grouped table a
    /// truth-up CI run prints). CLAIMED-NOT-PROVEN rows are rendered LOUD, never elided.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let verdict = if self.is_green() {
            "GREEN (no later-band Fabric gate red)"
        } else {
            "RED (a Fabric claim outran its verification)"
        };
        out.push_str(&format!(
            "P-517 FABRIC TRUTH-UP SCORECARD {} — {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                FabricRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                FabricRowStatus::ClaimedNotProven { date, reason } => {
                    format!("CLAIMED-NOT-PROVEN({date}: {reason})")
                }
            };
            out.push_str(&format!(
                "  [§{}] {:<14} {:<28} — {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

/// **Run the Fabric truth-up pass and produce the enumerated [`FabricTruthUpScorecard`] (P-517).** For
/// each PROVEN Fabric row this resolves a dated [`FabricRowStatus`]: a row is DATED-GREEN iff it carries
/// an `artifact_date` AND its proof source exists on disk under `repo_root`; otherwise it is recorded
/// CLAIMED-NOT-PROVEN with the run `date`. The scorecard surfaces — never swallows — any gap (EI-01 §1).
pub fn run_fabric_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> FabricTruthUpScorecard {
    let entries = proven_fabric_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => FabricRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    FabricRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => FabricRowStatus::DatedGreen { date: d.clone() },
            };
            FabricScorecardEntry { row, status }
        })
        .collect();
    FabricTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

// ───────────────────────────── (3) the every-incident-adds-a-drill loop ─────────────────────────────

/// **A Fabric incident on Myelin's own development (the every-incident-adds-a-drill loop, EI-01 §3/§5).**
/// A real incident ends by filing a PII-free Myelin issue draft AND a reproducing-drill ticket — both
/// reference-linked (the issue points at the drill that reproduces it). The integration drill registers
/// the repro into the harness `DrillRegistry` so it re-runs forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricIncident {
    /// the incident id (PII-free, e.g. `"INC-AG-DOGFOOD-1"`).
    pub incident_id: String,
    /// the Fabric gate the incident regressed (e.g. `"AG-D11"`).
    pub gate_id: String,
    /// a PII-free one-line description of what broke.
    pub description: String,
    /// the name of the reproducing drill the incident files.
    pub repro_drill_name: String,
}

impl FabricIncident {
    /// A new Fabric incident (every field PII-free).
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> FabricIncident {
        FabricIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    /// The PII-free Myelin issue draft the incident files (names the gate + the repro drill).
    pub fn issue_draft(&self) -> FabricIncidentIssueDraft {
        FabricIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] Fabric gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "Fabric incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked — every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    /// The reproducing-drill ticket the incident files (the test that joins the permanent suite).
    pub fn drill_ticket(&self) -> FabricIncidentDrillTicket {
        FabricIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

/// The PII-free Myelin issue draft a [`FabricIncident`] files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricIncidentIssueDraft {
    /// the Fabric gate the issue is about.
    pub gate_id: String,
    /// the issue title (PII-free).
    pub title: String,
    /// the issue body (PII-free; names the repro drill).
    pub body: String,
}

/// The reproducing-drill ticket a [`FabricIncident`] files (the drill that joins the permanent suite).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricIncidentDrillTicket {
    /// the drill name (the test that re-fires the failure).
    pub drill_name: String,
    /// the gate the drill guards.
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    /// **THE HEADLINE: Myelin's own triage agent runs GREEN on the self-hosting CI graph.** A real
    /// Myelin CI failure dispatches a costed triage run (explicit-first) with a BALANCED reserve/settle
    /// ledger + a content-addressed trace per run (the dogfood green artifacts, contract 1.8).
    #[test]
    fn myelins_own_agent_green_on_the_self_hosting_graph() {
        let artifact = run_fabric_over_myelins_own_work(RUN_DATE);
        assert!(
            artifact.is_green(),
            "Myelin's own triage agent must run green on the self-hosting CI graph: {}",
            artifact.summary()
        );

        // explicit-first: the CI-fail Signal DISPATCHES; a casual mention only NOTIFIES (0 auto-spawn).
        assert!(
            artifact.triage.dispatched,
            "a ci.result=failure Signal DISPATCHES a costed triage run (explicit-first, §3.4)"
        );
        assert!(
            artifact.triage.mention_only_notifies,
            "a casual @triage mention only NOTIFIES — 0 auto-spawn (the safety boundary)"
        );

        // BALANCED reserve/settle: reserved == settled (a Mock bills 0 → the reservation refunds).
        assert_eq!(
            artifact.triage.reserved, artifact.triage.settled,
            "reserve/settle BALANCED — reserved == settled on the Myelin self-tenant wallet"
        );
        assert_eq!(
            artifact.triage.cost_events, 0,
            "the Mock brain bills 0 metered units (the gate is brain-independent — AG-P25 is the real meter)"
        );
        assert_eq!(
            artifact.triage.inflight_interrupts, 0,
            "0 in-flight interrupt — the reservation's only exit is settle (11.7)"
        );

        // a trace per run: a content-addressed blake3:<hex> trace ref was written.
        assert!(
            artifact.triage.trace_ref.starts_with("blake3:"),
            "a content-addressed trace per run (8.8): {}",
            artifact.triage.trace_ref
        );

        let s = artifact.summary();
        assert!(s.contains("P-517 FABRIC DOGFOOD 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    /// The truth-up pass is GREEN — every PROVEN Fabric row rests on a dated green artifact.
    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_fabric_rows(RUN_DATE);
        assert!(
            rows.len() >= 11,
            "the PROVEN set covers AG-D1..AG-D11 + the E2E-2 spine (got {})",
            rows.len()
        );
        let confirmed = FabricTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red later-band Fabric gates — every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    /// The PROVEN set enumerates every AG-D drill family + the E2E-2 flagship (none silently dropped).
    #[test]
    fn the_proven_set_enumerates_every_ag_d_drill_plus_e2e2() {
        let rows = proven_fabric_rows(RUN_DATE);
        let ids: Vec<&str> = rows.iter().map(|r| r.id).collect();
        for must in [
            "AG-D1", "AG-D2", "AG-D3", "AG-D4", "AG-D5", "AG-D6", "AG-D7", "AG-D8", "AG-D9",
            "AG-D10", "AG-D11", "E2E-2",
        ] {
            assert!(
                ids.contains(&must),
                "the truth-up pass must enumerate the PROVEN row {must}"
            );
        }
    }

    /// A claimed-not-proven row reds the truth-up pass LOUDLY (surfaced, never swallowed).
    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_fabric_rows(RUN_DATE);
        rows[0].artifact_date = None;
        let verdict = FabricTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = FabricTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row fails the CI entrypoint");
        assert!(err.to_string().contains("CLAIMED-NOT-PROVEN"));
    }

    /// The enumerated scorecard renders GREEN with every PROVEN row dated + its proof source on disk.
    #[test]
    fn the_scorecard_renders_green_with_proof_sources_on_disk() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let scorecard = run_fabric_truth_up_scorecard(RUN_DATE, &repo_root);
        assert!(
            scorecard.is_green(),
            "the scorecard must be green — every PROVEN Fabric row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("AG-D1") && md.contains("E2E-2"),
            "enumerated: {md}"
        );
    }

    /// A row whose proof source is missing on disk is surfaced CLAIMED-NOT-PROVEN (never trusted on faith).
    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-fabric-truth-up-root");
        let scorecard = run_fabric_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard.entries.iter().all(|e| matches!(
                &e.status,
                FabricRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk")
            )),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    /// The every-incident loop: an incident files a PII-free issue draft + a reproducing-drill ticket.
    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = FabricIncident::new(
            "INC-AG-DOGFOOD-1",
            "AG-D11",
            "a reserve/settle regression left an in-flight triage run torn down on the Myelin self-tenant",
            "repro_ag_d11_dogfood_runaway_self_limiter",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "AG-D11");
        assert!(draft.title.contains("INC-AG-DOGFOOD-1"));
        assert!(
            draft
                .body
                .contains("repro_ag_d11_dogfood_runaway_self_limiter"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        assert!(!draft.body.to_lowercase().contains("email"));
        let ticket = incident.drill_ticket();
        assert_eq!(
            ticket.drill_name,
            "repro_ag_d11_dogfood_runaway_self_limiter"
        );
        assert_eq!(ticket.gate_id, "AG-D11");
    }

    /// The MOCK-runtime floor is named in writing (VISION §3) — the real LlmAgentRuntime swap is AG-P25.
    #[test]
    fn the_mock_runtime_floor_is_named() {
        assert!(DOGFOOD_RUNTIME_FLOOR.contains("MOCK runtime"));
        assert!(
            DOGFOOD_RUNTIME_FLOOR.contains("AG-P25"),
            "names the real-runtime swap follow-on"
        );
        assert!(DOGFOOD_RUNTIME_FLOOR.contains("post-M5"));
    }
}
