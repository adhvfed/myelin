//! The versioned thresholds file loader (P-S22 / P-038, master band M0).
//!
//! THE single, versioned source of truth for every Q32 "default-to-beat": the quantified threshold
//! every M0 drill reads its number from. No drill hardcodes a magic number — it reads its threshold
//! from [`Thresholds`] (parsed from the workspace-root `thresholds.toml`). **A missing threshold is
//! a LOUD error** ([`ThresholdError::Missing`]), never a silent default.
//!
//! CANON:
//! - external-insights/01-process-and-quality-doctrine.md §3 — gates resolve to *quantified*
//!   thresholds; NEVER weaken a threshold or invert an assertion to make a check pass. A red gate is
//!   *information*: it becomes a [`ClaimedNotProven`] row in the file — never edited green.
//! - planning/05-refined-shared-systems-architecture/00-platform-substrate.md §8.2 — the fail-static
//!   value W is `[OPEN — LEGAL]` (the one substrate-owned legal flag, L-1); the mechanism + the
//!   `static_max ≤ revocation-SLA ≥ agent-token-TTL` constraint ship regardless.
//! - planning/06-roadmaps/shared/00-platform-substrate.md §2 SUB-M0 item 5 (the Q32 defaults) + §5
//!   (the thresholds-file / green-artifact discipline) + §6 (the honesty register).
//!
//! DISCIPLINE: a threshold is edited only by re-running the drill that measures it (e.g. the M5
//! surge tunes the shed-budget numbers, P-S33). A drill that comes back red does NOT get its
//! threshold softened; it is recorded as a [`ClaimedNotProven`] row and stays there until the
//! deliverable is genuinely repaired. The fail-static value W is absent until the DPO ratifies it —
//! reading it as a concrete number is a loud error ([`FailStaticThreshold::ratified_static_max_secs`]).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::shed::{ShedBudgetError, ShedBudgetTable, Surface, SurfaceBudget};

/// The canonical filename, at the workspace root. The single thresholds file (P-S22).
pub const THRESHOLDS_FILENAME: &str = "thresholds.toml";

/// A failure to load or read a threshold. Every variant is LOUD by construction — a missing or
/// not-yet-ratified threshold is an error, never a silent default (EI-01 §3 / the roadmap §2 item 5
/// "a missing threshold is a loud error, not a default").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThresholdError {
    /// The file could not be read from disk (path + the OS error string).
    Io(String),
    /// The file did not parse as the expected TOML shape (the toml error string).
    Parse(String),
    /// A named threshold was requested that the file does not carry. The argument is the dotted key
    /// (e.g. `shed_budgets.CiDispatch`). A drill that asks for an absent threshold halts here — it
    /// does NOT proceed against a guessed default.
    Missing(String),
    /// A threshold exists but is `[OPEN — LEGAL]` / not-yet-ratified: it carries no concrete value
    /// to read. The argument is the key + its open status. (The fail-static value W, L-1.)
    OpenLegal(String),
}

impl std::fmt::Display for ThresholdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThresholdError::Io(e) => write!(f, "thresholds file unreadable: {e}"),
            ThresholdError::Parse(e) => write!(f, "thresholds file did not parse: {e}"),
            ThresholdError::Missing(k) => {
                write!(f, "missing threshold `{k}` — a missing threshold is a loud error, not a default")
            }
            ThresholdError::OpenLegal(k) => write!(
                f,
                "threshold `{k}` is [OPEN — LEGAL] (not yet DPO-ratified) — it carries no value to read"
            ),
        }
    }
}

impl std::error::Error for ThresholdError {}

/// The whole parsed thresholds file (P-S22). Every Q32 default-to-beat as a named, dated row.
///
/// NB: `Eq` is intentionally NOT derived — [`FlexDb::facet_promotion_ratio`] (and any future ratio
/// threshold) is an `f64` (which is `PartialEq` but not `Eq`). The file is compared with
/// `assert_eq!` in the round-trip test (`PartialEq` suffices); no consumer keys a map/set on a whole
/// `Thresholds` value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    /// The file's schema version (forward-only).
    pub version: u32,
    /// The ISO-8601 date this row set was last asserted against its drills.
    pub as_of: String,
    /// N — the deprovision / revocation SLA.
    pub revocation: Revocation,
    /// The surge load-multiplier (SUB-D3 / the F6 family).
    pub surge: Surge,
    /// The authz-surge (ID-D9) protected-human-lane latency budget — the p99 the human lane must
    /// hold within under the 30× authz-hot-path surge (identity §13: authz is the highest-QPS shared
    /// system; contract 1.8 `auth_decision_latency`). The SHAPE is frozen here (P-ID-31); the NUMBER
    /// is the default-to-beat measured under the surge. `#[serde(default)]` so an older thresholds
    /// file (pre-P-ID-31) still parses (falls back to the seed).
    #[serde(default)]
    pub authz_surge: AuthzSurge,
    /// W — the fail-static bounded-staleness window (`[OPEN — LEGAL]`, L-1).
    pub fail_static: FailStaticThreshold,
    /// The durability objectives (RPO / RTO), asserted by restore-verify (STOR-D1/D2, SUB-D6).
    pub rpo_rto: RpoRto,
    /// The causal-depth ceilings the agent-loop guard halts a loop at (SUB-D8).
    pub depth_ceilings: DepthCeilings,
    /// The S8 authz-reverse-index measured tunables (the Ids↔Filter cardinality cap + the
    /// reverse_index_lag SLO; P-ID-11 / contract 4.3). The SHAPE is frozen; the NUMBERS are the
    /// default-to-beat re-measured + finalised at world-scale in P-ID-31. `#[serde(default)]` so an
    /// older thresholds file (pre-P-ID-11) still parses (the cap falls back to the seed).
    #[serde(default)]
    pub authz_index: AuthzIndex,
    /// The per-surface shed-budget v1 floors (contract 1.11, §7.6).
    #[serde(default)]
    pub shed_budgets: Vec<ShedBudgetRow>,
    /// The DSR (data-subject-request) deadline thresholds (contract 10.4 / gdpr §4.1 step 6 / the
    /// GA-D4 drill): the statutory 1-month window + the nearing-deadline warning margin the durable
    /// timer fires the `dsr_deadline_margin` warning Signal at. `#[serde(default)]` so an older
    /// thresholds file (pre-P-GA-21) still parses (the rows fall back to the Art. 12(3) seed).
    #[serde(default)]
    pub dsr: DsrDeadline,
    /// The Refs recursive-CTE traverse bounds (REF-P13 / contract 5.3 — the traverse depth ceiling +
    /// the collected-node budget the `WITH RECURSIVE` walk truncates at). DISTINCT from
    /// [`DepthCeilings`] (those are the agent CAUSAL depth, contract 1.11; this is the GRAPH-HOP
    /// traverse ceiling, architecture §4.5 default 16). `#[serde(default)]` so an older thresholds
    /// file (pre-REF-P13) still parses (the bounds fall back to the §4.5 seed).
    #[serde(default)]
    pub refs_traverse: RefsTraverse,
    /// The Knowledge flexible-database read budgets + the frozen `> 5%` facet-promotion threshold
    /// (KN-P17 / KN-D9; contract 6.3 / OQ-C). The flex-DB view read p99 budget (the KN-D9 latency
    /// gate) + the page row-cap + the facet-promotion ratio the telemetry measures the trigger
    /// against. `#[serde(default)]` so an older thresholds file (pre-KN-P17) still parses (falls back
    /// to the §4.1 / 6.3 seeds).
    #[serde(default)]
    pub flex_db: FlexDb,
    /// The MEASURED cell sizing-band numbers (P-CP-22 / P-431; tenancy §7.1 / ADR-10). The per-cell-
    /// class `tenants_max` + which capacity dimension BINDS FIRST, set from the load-test + the
    /// `cell_utilisation` telemetry — replacing the conservative §5.1 defaults. The avoid-migration-
    /// by-sizing floor (P-CP-05/P-CP-07) is promoted: these are the MEASURED defaults-to-beat (never
    /// predicted, ADR-10 — the binding dimension is discovered by measurement). `#[serde(default)]` so
    /// an older thresholds file (pre-P-CP-22) still parses (falls back to the conservative seeds).
    #[serde(default)]
    pub cell_sizing: CellSizing,
    /// The scorecard: drills that came back red live here, never edited green (EI-01 §3).
    #[serde(default)]
    pub claimed_not_proven: Vec<ClaimedNotProven>,
}

/// N — the deprovision / revocation SLA (the disabled-user blast-radius bound).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revocation {
    /// N, in minutes. The fail-static window must sit under this (`static_max ≤ revocation-SLA`).
    pub sla_mins: u64,
}

/// The surge load-multiplier (the 1×/10×/30× generator's top multiplier).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surge {
    /// The multiplier the surge family drives at (default-to-beat: 30×).
    pub multiplier: u32,
    /// **The substrate-level protected-human-lane p99 latency budget under the 30× surge, in
    /// MICROSECONDS** (`_us`; the §2.10 units anchor). This is the SUB-D3 (P-S32) ceiling: across a
    /// 30× agent/CI surge on one tenant the substrate's protected human lane (the §7.2 reserved lane
    /// at the public surface) must still complete a human request within this p99. Distinct from
    /// [`AuthzSurge::human_lane_p99_budget_us`] (the authz hot-path `check` budget, ID-D9): this is
    /// the GENERIC public-surface human-lane budget the substrate's shed lane protects. A human
    /// request that is admitted (never queued behind a machine lane) is served at its normal latency;
    /// a human that were SHED would blow this budget (a 429 is not "within budget"). The SHAPE (a
    /// measured human-lane p99 ceiling under surge) is frozen here; the NUMBER is the default-to-beat
    /// the SUB-D3 drill measures + the M5 budget-tuning follow-on (P-S33) re-confirms. A regression
    /// that pushes the human-lane p99 past it is a dated `[[claimed_not_proven]]` row, never a lowered
    /// bar (EI-01 §3). `#[serde(default)]` so an older file without the field still parses against the
    /// seed default-to-beat.
    #[serde(default = "Surge::default_human_lane_p99_budget_us")]
    pub human_lane_p99_budget_us: u64,
}

impl Surge {
    /// The seed default-to-beat for the substrate's protected human-lane p99 under the 30× surge:
    /// 10 000 µs (= 10 ms). A human request admitted into the reserved lane (never queued behind a
    /// machine lane) completes well under this; the SUB-D3 drill (P-S32) measures + dates the real
    /// number and the M5 budget-tuning follow-on (P-S33) re-confirms it. Generous-but-real: a
    /// human-lane p99 over 10 ms at the public surface under a 30× surge IS a regression worth a red.
    pub fn default_human_lane_p99_budget_us() -> u64 {
        10_000
    }
}

/// The authz-surge (ID-D9 / F6) protected-human-lane latency budget (identity §10/§13; contract 1.8
/// `auth_decision_latency` / 4.11 the shed order on the authz surface).
///
/// ID-D9 drives a 30× agent surge on the authz hot path (`check`) — the highest-QPS shared system.
/// The protected human lane must hold WITHIN this p99 budget while the agent lane sheds (429 +
/// Retry-After) and cross-tenant impact stays 0. The SHAPE (a measured p99 budget on the human lane
/// under surge) is frozen here (P-ID-31); the NUMBER is the default-to-beat measured by the ID-D9
/// drill at 30×. A regression that pushes the human-lane p99 past it is a dated `[[claimed_not_proven]]`
/// row, never a lowered bar (EI-01 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthzSurge {
    /// The human-lane authz-decision p99 budget under the 30× surge, in MICROSECONDS (`_us`; the
    /// §2.10 units anchor). A `check` is an in-cell, memo-bounded tuple resolution; the budget is
    /// the ceiling the protected human lane's p99 stays under while the agent lane is shed. Default-
    /// to-beat measured by ID-D9 (P-ID-31).
    pub human_lane_p99_budget_us: u64,
}

impl Default for AuthzSurge {
    /// The seed default-to-beat for the human-lane authz p99 under surge: 5 000 µs (= 5 ms). A `check`
    /// resolved from the in-cell tuple store under the protected lane sits far under this; the surge
    /// drill (ID-D9, P-ID-31) measures + dates the real number. Generous-but-real: a human-lane p99
    /// over 5 ms under a 30× surge IS a regression worth a red.
    fn default() -> Self {
        AuthzSurge {
            human_lane_p99_budget_us: 5_000,
        }
    }
}

/// W — the fail-static bounded-staleness window (contract 1.10 / 4.11; architecture §8.2).
///
/// The VALUE `static_max_secs` is `[OPEN — LEGAL]` (L-1) and is intentionally ABSENT until the DPO
/// ratifies it; reading it via [`FailStaticThreshold::ratified_static_max_secs`] is a loud
/// [`ThresholdError::OpenLegal`] error until then. The MECHANISM and the constraint ship regardless;
/// `static_max_default_secs` is the engineering *seed* the mechanism is drilled against (NOT the
/// ratified W).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailStaticThreshold {
    /// The legal status marker (e.g. `"OPEN — LEGAL"`). Present while the value is unratified.
    pub status: String,
    /// Who ratifies the value (the DPO / Legal).
    pub owner: String,
    /// The ratified value W, in seconds — ABSENT (`None`) until the DPO ratifies it.
    #[serde(default)]
    pub static_max_secs: Option<u64>,
    /// The engineering seed the mechanism is drilled against (the largest value the constraint
    /// admits). NOT the ratified W — labelled a seed, not a default-to-beat.
    pub static_max_default_secs: u64,
    /// The LOWER bound of the staleness constraint (architecture §8.2): `static_max ≥ agent-token
    /// TTL` — the window must CONTAIN the short-lived agent token (ID-1 / GD-3 / ADR-17), in
    /// seconds. The `FailStatic` constructor (P-S18) rejects a `static_max` below this. The agent
    /// token's life == its run life; this is the floor that keeps the window from being shorter than
    /// the token it must outlive. Read by [`crate::StalenessBound::from_threshold`].
    pub agent_token_ttl_secs: u64,
    /// The constraint that ships regardless and is enforced in the `FailStatic` constructor (P-S18).
    pub constraint: String,
}

impl FailStaticThreshold {
    /// The ratified value W, or a LOUD [`ThresholdError::OpenLegal`] error while it is `[OPEN —
    /// LEGAL]`. A drill that needs the concrete W (not the seed) MUST go through here so an
    /// unratified value can never be silently read as a number.
    pub fn ratified_static_max_secs(&self) -> Result<u64, ThresholdError> {
        self.static_max_secs.ok_or_else(|| {
            ThresholdError::OpenLegal(format!("fail_static.static_max_secs ({})", self.status))
        })
    }
}

/// The durability objectives (RPO / RTO), asserted by restore-verify (STOR-D1/D2, SUB-D6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpoRto {
    /// Recovery-point objective: the maximum data-loss window, in minutes (default-to-beat: ≤ 5).
    pub rpo_max_mins: u64,
    /// Recovery-time objective per tenant, in minutes (default-to-beat: ≤ 60).
    pub rto_tenant_max_mins: u64,
    /// Recovery-time objective per cell, in minutes (default-to-beat: ≤ 240).
    pub rto_cell_max_mins: u64,
}

/// The causal-depth ceilings the agent-loop guard halts a loop at (SUB-D8, contract 1.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepthCeilings {
    /// The soft ceiling — at/over this depth a reaction is admitted-but-flagged (default: 12).
    pub soft: u32,
    /// The hard ceiling — at/over this depth a reaction is halted (default: 16).
    pub hard: u32,
}

/// The S8 authz-reverse-index measured tunables (P-ID-11 / P-069; identity §7.1, contract 4.3/1.8).
///
/// The Ids↔Filter SHAPE is frozen (small reachable sets materialise as `Ids`, large ones push down
/// as `Filter`); only the NUMBERS — the cardinality cap that switches between them + the
/// `reverse_index_lag` freshness SLO — were open. **MEASURED + FINALISED at world-scale in P-ID-32
/// (P-425, 2026-06-24)**, riding the P-ID-31 30× surge + the cell-scale list/scan load: the measured
/// Ids↔Filter crossover sits AT the cap (the materialise plan is linear, the push-down is a fixed
/// JOIN), and the `reverse_index_lag` stayed 0 under the surge (well within the SLO, bounded < the
/// revocation SLA). **This CLOSED the P-ID-11 cardinality-cap floor.** `#[serde(default)]` on the
/// parent field + [`Default`] here so an older thresholds file still parses (falls back to the seed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthzIndex {
    /// The Ids↔Filter cardinality cap (the §7.1 switch point): at-or-under → `Ids` (materialise),
    /// above → `Filter` (push down). Default-to-beat: 1000 (re-measured + finalised at P-ID-31).
    pub ids_cardinality_cap: usize,
    /// The `reverse_index_lag` freshness SLO, in milliseconds (the watermark-fallback the P-ID-12
    /// read path reads). Default-to-beat: 1000 ms (re-measured + finalised at P-ID-31).
    pub reverse_index_lag_slo_ms: u64,
}

impl Default for AuthzIndex {
    /// The seed default-to-beat (mirrors `myelin_identity_service::DEFAULT_IDS_CARDINALITY_CAP`).
    fn default() -> Self {
        AuthzIndex {
            ids_cardinality_cap: 1000,
            reverse_index_lag_slo_ms: 1000,
        }
    }
}

/// The DSR deadline thresholds (contract 10.4 / gdpr §4.1 step 6 — the durable deadline timer +
/// the nearing-deadline warning Signal; the GA-D4 drill).
///
/// `deadline_secs` is the Art. 12(3) statutory window (1 month = 30 days). `warning_margin_secs` is
/// the nearing-deadline margin: the durable timer fires the `dsr_deadline_margin` warning Signal
/// `warning_margin_secs` BEFORE the deadline, so a deadline is never silently missed (the default-to-
/// beat margin GA-D4 reads). `extension_secs` is the Art. 12(3) extension-for-complex window (3
/// months total — the re-arm carries a recorded reason). The default-to-beat numbers here are the
/// statutory seeds; Phase 6 may tighten the warning margin (a wider margin = earlier warning).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DsrDeadline {
    /// The statutory deadline window, in seconds (Art. 12(3): 1 month = 30 days = 2 592 000 s).
    pub deadline_secs: u64,
    /// The nearing-deadline warning margin, in seconds (the durable timer fires the warning Signal
    /// this far BEFORE the deadline). Default-to-beat: 7 days = 604 800 s (a full week's warning).
    pub warning_margin_secs: u64,
    /// The Art. 12(3) extension window for a complex request, in seconds — the TOTAL deadline after
    /// an extension (3 months = 90 days = 7 776 000 s; the re-arm carries a recorded reason).
    pub extension_total_secs: u64,
}

impl Default for DsrDeadline {
    /// The statutory seeds (Art. 12(3)): a 1-month deadline, a 1-week nearing-deadline warning, a
    /// 3-month extension ceiling. These are the default-to-beat; Phase 6 may widen the warning margin.
    fn default() -> Self {
        DsrDeadline {
            deadline_secs: 30 * 24 * 60 * 60,
            warning_margin_secs: 7 * 24 * 60 * 60,
            extension_total_secs: 90 * 24 * 60 * 60,
        }
    }
}

/// The Refs recursive-CTE traverse bounds (REF-P13 / P-162; contract 5.3; architecture §4.5).
///
/// The bounded, cycle-safe `WITH RECURSIVE` walk over the `edge` adjacency list reads its DEPTH
/// CEILING (default 16, §4.5) + its COLLECTED-NODE budget (the max distinct nodes a single traverse
/// may visit before it returns a PARTIAL result + a `truncated` marker, never an unbounded scan,
/// X-3) from HERE — the single source of truth, no hardcoded magic number in the traverse. This is
/// the GRAPH-HOP ceiling, DISTINCT from the agent CAUSAL [`DepthCeilings`] (contract 1.11): a deep
/// dependency tree is not a runaway agent loop. `#[serde(default)]` + [`Default`] so an older file
/// parses (falls back to the §4.5 seed). Mirrors `myelin_refs_service::traverse::TRAVERSE_DEPTH_CEILING`
/// (the seed constant); this file is its source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefsTraverse {
    /// The traverse depth ceiling (§4.5 default 16): the walk stops descending past this many hops
    /// from the root and marks the result `truncated`. Never an unbounded scan.
    pub depth_ceiling: u32,
    /// The collected-node budget (X-3): the max distinct nodes a single traverse may visit before it
    /// returns a PARTIAL result + a `truncated` marker. Bounds a wide (high-fan-out) graph the depth
    /// ceiling alone would not. Default-to-beat seed; re-measured at world-scale (REF-P22).
    pub max_nodes: u32,
}

impl Default for RefsTraverse {
    /// The §4.5 seed: a depth ceiling of 16, a collected-node budget of 10 000 (the default-to-beat
    /// a single bounded traverse stays under; re-measured at world-scale in REF-P22).
    fn default() -> Self {
        RefsTraverse {
            depth_ceiling: 16,
            max_nodes: 10_000,
        }
    }
}

/// The Knowledge flexible-database read budgets + the frozen `> 5%` facet-promotion threshold
/// (KN-P17 / P-307; KN-D9; contract 6.3 / OQ-C, architecture 01 §1.2 / 02 §4.1).
///
/// `view_read_p99_max_ms` is the KN-D9 flex-DB latency gate: filter/sort/group a large multi-tenant
/// database (JSONB + the GIN projection + the `SetExpr` conjoin) → the read-time p99 must stay
/// within this budget. `page_row_cap` is the §4.1 step-5 row cap (a single view read is ALWAYS
/// paginated/row-capped — never an unbounded scan). `facet_promotion_ratio` is the FROZEN
/// Search-owned `> 5%` tunable (contract 6.3): a facet in MORE than this fraction of a collection's
/// view executions over a rolling window promotes from a cold GIN scan to a per-facet generated
/// index — MEASURED here (KN-P17, `FacetTelemetry`), ACTED on in KN-P31 (M5). `#[serde(default)]` +
/// [`Default`] so an older file parses (the seeds). Mirrors
/// `myelin_knowledge::{database::PageBound::MAX, FACET_PROMOTION_THRESHOLD}` (the seed constants);
/// this file is their source of truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlexDb {
    /// The KN-D9 flex-DB view-read p99 budget, in milliseconds. The filter/sort/group read (JSONB +
    /// GIN + the `SetExpr` conjoin) must stay within this at scale. Default-to-beat re-confirmed at
    /// world scale in KN-P31 (M5).
    pub view_read_p99_max_ms: u64,
    /// The §4.1 step-5 page row cap (a single view read is ALWAYS row-capped — never unbounded).
    pub page_row_cap: u32,
    /// The frozen `> 5%` facet-promotion ratio (contract 6.3 / OQ-C): a facet in MORE than this
    /// fraction of a collection's executions promotes to a generated index (measured here, acted on
    /// in KN-P31). The trigger is STRICTLY greater-than (a facet at exactly the ratio does NOT
    /// promote — the frozen wording).
    pub facet_promotion_ratio: f64,
    /// The KN-D10 read-time rollup/formula recompute p99 budget, in milliseconds (KN-P18 / P-308,
    /// architecture §4.2 / KQ-4). A rollup over a large related set is computed at READ TIME
    /// (permission-filtered, conjoining `list_objects`), never stored; its recompute p99 must stay
    /// within this budget. A rollup whose MEASURED read-time recompute p99 crosses it
    /// (`RollupLatencyTelemetry::materialisation_candidates`) is the per-rollup promotion trigger —
    /// promoted to an incrementally-maintained materialised aggregate fed off the bus → the OLAP
    /// read store (contract 11.6) in KN-P31 (M5). The trigger is STRICTLY greater-than (a rollup at
    /// exactly the budget is within budget). Weakening this to pass is forbidden (EI-01 §3) — a red
    /// is a dated `[[claimed_not_proven]]` row, never a lowered bar.
    pub rollup_read_p99_max_ms: u64,
}

impl Default for FlexDb {
    /// The §4.1 / 6.3 / §4.2 seeds: a 200 ms flex-DB view-read p99 budget (the KN-D9 default-to-beat,
    /// re-confirmed at world scale in KN-P31), a 500-row page cap (`PageBound::MAX`), the frozen 5%
    /// facet-promotion ratio, and a 250 ms read-time rollup recompute p99 budget (the KN-D10
    /// default-to-beat; a rollup crossing it is the per-rollup materialisation trigger, KN-P31).
    fn default() -> Self {
        FlexDb {
            view_read_p99_max_ms: 200,
            page_row_cap: 500,
            facet_promotion_ratio: 0.05,
            rollup_read_p99_max_ms: 250,
        }
    }
}

/// **The MEASURED cell sizing-band numbers (P-CP-22 / P-431; tenancy-and-control-plane.md §7.1,
/// ADR-10 measure-before-shard).**
///
/// The §5.1 cell `capacity` vector is multi-dimensional (`tenants_max` / `write_qps_max` /
/// `storage_bytes_max`); §7.1 says the BINDING dimension — the one a cell class fills first — is
/// discovered by MEASUREMENT, NEVER predicted. This row records the MEASURED Pool-tier sizing band:
/// the per-cell `tenants_max` and which dimension bound first under the load test, set from the
/// `cell_utilisation` telemetry (contract 1.8) — the avoid-migration-by-sizing floor
/// (P-CP-05/P-CP-07) promoted to a measured default-to-beat. When a cell's MEASURED utilisation
/// crosses the headroom this band reserves, sealing cannot relieve it, and the live cell→cell
/// migration (P-CP-22 / [`crate`]'s control-plane `migration` module) is the relief lever.
///
/// **MEASURED, not predicted (ADR-10).** The numbers below are the load-test result fed back here as
/// a dated default-to-beat (the §7.1 `[OPEN → P5, measured]` default-to-beat is now CLOSED — measured,
/// not the conservative §5.1 seed). A regression that exceeds `tenants_max` headroom is the
/// MEASURED-hot-cell migration trigger, never a silently-raised bar (EI-01 §3). `#[serde(default)]`
/// on the parent field + [`Default`] here so an older thresholds file (pre-P-CP-22) still parses
/// (falls back to the conservative §5.1 seed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellSizing {
    /// The MEASURED per-cell-class `tenants_max` for the Pool tier (the tenant-count dimension) — the
    /// number of tenants one Pool cell admits before the sizing band's headroom is consumed. Measured
    /// from the load test + the `cell_utilisation` telemetry, replacing the conservative §5.1 seed.
    pub pool_tenants_max: u32,
    /// The MEASURED sustained write-QPS ceiling for the Pool tier (the throughput dimension).
    pub pool_write_qps_max: u32,
    /// The MEASURED stored-bytes ceiling for the Pool tier (the storage dimension), in bytes.
    pub pool_storage_bytes_max: u64,
    /// **Which capacity dimension BINDS FIRST** for the Pool tier — the dimension a Pool cell fills
    /// before the others, discovered by MEASUREMENT (ADR-10). One of `tenants` / `write_qps` /
    /// `storage_bytes` — the sizing algorithm (§7.1) reads this to know which utilisation to watch for
    /// the migration trigger. Measured, NEVER predicted.
    pub pool_binding_dimension: String,
    /// The headroom fraction (basis points, `0..10000`) a Pool cell reserves below its binding
    /// dimension before it is considered "hot" — when MEASURED utilisation crosses
    /// `binding_max * (1 - headroom)`, sealing cannot relieve it and the live migration (P-CP-22) is
    /// the lever. Default-to-beat: 2000 bps (= 20% headroom). A cell past this is the migration trigger.
    pub pool_hot_headroom_bps: u32,
}

impl Default for CellSizing {
    /// The conservative §5.1 SEED (the pre-measurement default — the avoid-migration-by-sizing floor
    /// before the load test lands the measured numbers). An older thresholds file falls back to this;
    /// the canonical file carries the MEASURED P-CP-22 band. The binding dimension defaults to
    /// `tenants` (the conservative assumption the tenant-count cap binds first); the measured band
    /// records the ACTUAL binding dimension. Mirrors the §5.1 `Capacity` seed in the registry.
    fn default() -> Self {
        CellSizing {
            pool_tenants_max: 1000,
            pool_write_qps_max: 5000,
            pool_storage_bytes_max: 1 << 40,
            pool_binding_dimension: "tenants".into(),
            pool_hot_headroom_bps: 2000,
        }
    }
}

/// A per-surface shed-budget v1-floor row (the `shed::SurfaceBudget` for one `shed::Surface`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShedBudgetRow {
    /// The surface this budget governs — matches the `shed::Surface` enum variant name.
    pub surface: String,
    /// Per-tenant in-flight cap.
    pub per_tenant_in_flight_cap: u32,
    /// The reserved protected-human-lane slots.
    pub human_lane_reservation: u32,
    /// The `Retry-After` hint (seconds) when shed.
    pub retry_after_secs: u64,
}

/// A scorecard row: a drill that came back RED, recorded honestly, NEVER edited green (EI-01 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedNotProven {
    /// The gate / drill that is red (e.g. `"SUB-D3"`).
    pub gate: String,
    /// The dotted threshold key the gate's number lives under.
    pub threshold_key: String,
    /// The date the red was recorded (ISO-8601).
    pub date: String,
    /// Who owns repairing it.
    pub owner: String,
    /// A short note on what is unproven and the follow-on.
    pub note: String,
}

impl Thresholds {
    /// Parse the thresholds file from a TOML string. A parse failure is a loud
    /// [`ThresholdError::Parse`].
    pub fn from_toml(s: &str) -> Result<Thresholds, ThresholdError> {
        toml::from_str(s).map_err(|e| ThresholdError::Parse(e.to_string()))
    }

    /// Serialize back to TOML (used by the round-trip test and any dated-update write-back).
    pub fn to_toml(&self) -> Result<String, ThresholdError> {
        toml::to_string(self).map_err(|e| ThresholdError::Parse(e.to_string()))
    }

    /// Load the thresholds file from a path on disk. A read failure is a loud
    /// [`ThresholdError::Io`].
    pub fn load(path: &std::path::Path) -> Result<Thresholds, ThresholdError> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| ThresholdError::Io(format!("{}: {e}", path.display())))?;
        Thresholds::from_toml(&s)
    }

    /// Load the canonical workspace-root thresholds file. The path is resolved from
    /// `CARGO_MANIFEST_DIR` (this crate sits at `crates/myelin-substrate`, so the workspace root is
    /// two levels up). This is the accessor a drill uses to read its threshold.
    pub fn load_canonical() -> Result<Thresholds, ThresholdError> {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = manifest
            .parent()
            .and_then(|p| p.parent())
            .ok_or_else(|| ThresholdError::Io("could not resolve the workspace root".into()))?;
        Thresholds::load(&root.join(THRESHOLDS_FILENAME))
    }

    /// The shed-budget rows indexed by `shed::Surface`, ready to hand to `shed::ShedBudgetTable`.
    /// An unknown surface name in the file is a loud [`ThresholdError::Parse`].
    pub fn shed_budget_table(&self) -> Result<HashMap<Surface, SurfaceBudget>, ThresholdError> {
        let mut out = HashMap::new();
        for row in &self.shed_budgets {
            let surface = parse_surface(&row.surface)?;
            out.insert(
                surface,
                SurfaceBudget {
                    per_tenant_in_flight_cap: row.per_tenant_in_flight_cap,
                    human_lane_reservation: row.human_lane_reservation,
                    retry_after_secs: row.retry_after_secs,
                },
            );
        }
        Ok(out)
    }

    /// The shed budget for ONE surface — a missing surface is a loud [`ThresholdError::Missing`]
    /// (the drill does not proceed against a guessed budget).
    pub fn shed_budget(&self, surface: Surface) -> Result<SurfaceBudget, ThresholdError> {
        self.shed_budget_table()?
            .remove(&surface)
            .ok_or_else(|| ThresholdError::Missing(format!("shed_budgets.{surface:?}")))
    }

    /// **Validate the file's TUNED shed budgets against the §7.6 floor discipline (P-S33).** Each row
    /// loaded from the file must hold: bounded (cap > 0), reservation-within-cap, and — for a
    /// human-facing surface — at-or-above the measured human-lane floor
    /// ([`SurfaceBudget::HUMAN_LANE_FLOOR_BPS`]). A row that tuned a human lane into starvation is a
    /// LOUD [`ShedBudgetError`], NOT a silently-accepted regression: this is the gate that makes "you
    /// cannot tune the human lane into starvation" un-bypassable from the thresholds file itself, so a
    /// future hand-edit that drops a reservation below the floor fails at load, never at runtime under
    /// surge. Each row in the file MUST validate (the tuned numbers are measured, not weakened).
    pub fn validate_shed_budgets(&self) -> Result<(), ShedBudgetError> {
        let table_map = self
            .shed_budget_table()
            .map_err(|_| ShedBudgetError::Unbounded(Surface::HttpIntake))?;
        for (surface, budget) in &table_map {
            budget.validate_tuned(*surface)?;
        }
        Ok(())
    }

    /// The file's tuned shed budgets as a validated [`ShedBudgetTable`]. Every row must be present and
    /// each must hold the §7.6 floor; otherwise a loud error. The table the surge drill re-runs
    /// against the tuned numbers (P-S33).
    pub fn shed_budget_table_validated(&self) -> Result<ShedBudgetTable, ThresholdError> {
        self.validate_shed_budgets()
            .map_err(|e| ThresholdError::Parse(e.to_string()))?;
        let map = self.shed_budget_table()?;
        Ok(ShedBudgetTable::from_rows(map))
    }
}

/// Map a `shed::Surface` variant NAME (as written in the file) to the enum. The match is exhaustive,
/// so a new `Surface` variant is a compile error here (the file's vocabulary stays in lock-step with
/// the enum). An unknown name is a loud parse error.
fn parse_surface(name: &str) -> Result<Surface, ThresholdError> {
    let s = match name {
        "HttpIntake" => Surface::HttpIntake,
        "CiDispatch" => Surface::CiDispatch,
        "CollabOpStream" => Surface::CollabOpStream,
        "ConnectionTier" => Surface::ConnectionTier,
        "AgentMention" => Surface::AgentMention,
        "GitFrontDoor" => Surface::GitFrontDoor,
        other => {
            return Err(ThresholdError::Parse(format!(
                "unknown shed-budget surface `{other}` (not a shed::Surface variant)"
            )))
        }
    };
    // Exhaustiveness guard: if a new Surface variant lands, this match must be extended too.
    match s {
        Surface::HttpIntake
        | Surface::CiDispatch
        | Surface::CollabOpStream
        | Surface::ConnectionTier
        | Surface::AgentMention
        | Surface::GitFrontDoor => Ok(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_load::DepthCeiling;

    /// A sample drill reads its threshold from the canonical file (the green artifact required by
    /// the GATE: "a sample drill reading its threshold from the file"). The depth-ceiling drill
    /// asserts the loaded ceilings == the seed constants `DepthCeiling::{V1_SOFT, V1_HARD}` — this
    /// file is now their single source of truth, with no hardcoded magic number in the drill.
    #[test]
    fn sample_drill_reads_depth_ceiling_from_the_canonical_file() {
        let t = Thresholds::load_canonical().expect("the canonical thresholds file must load");
        // the drill reads its threshold from the file — NOT a hardcoded literal.
        assert_eq!(t.depth_ceilings.soft, DepthCeiling::V1_SOFT);
        assert_eq!(t.depth_ceilings.hard, DepthCeiling::V1_HARD);
        // and the ceiling the agent-loop guard would build from the file matches the v1 floor
        // (the file is now the single source of truth for the seed constants).
        let from_file = DepthCeiling::new(t.depth_ceilings.soft, t.depth_ceilings.hard);
        let v1 = DepthCeiling::v1_floor();
        assert_eq!(from_file.soft(), v1.soft());
        assert_eq!(from_file.hard(), v1.hard());
    }

    /// The canonical file carries every Q32 default-to-beat at its documented value.
    #[test]
    fn canonical_file_holds_every_q32_default() {
        let t = Thresholds::load_canonical().expect("load");
        assert_eq!(t.revocation.sla_mins, 5, "N = 5 min revocation");
        assert_eq!(t.surge.multiplier, 30, "30× surge");
        assert_eq!(
            t.surge.human_lane_p99_budget_us, 10_000,
            "SUB-D3 substrate human-lane p99 budget = 10 ms (10000 µs)"
        );
        assert_eq!(
            t.authz_surge.human_lane_p99_budget_us, 5000,
            "ID-D9 human-lane authz p99 budget = 5 ms (5000 µs)"
        );
        assert_eq!(t.rpo_rto.rpo_max_mins, 5, "RPO ≤ 5 min");
        assert_eq!(t.rpo_rto.rto_tenant_max_mins, 60, "RTO ≤ 1h/tenant");
        assert_eq!(t.rpo_rto.rto_cell_max_mins, 240, "RTO ≤ 4h/cell");
        assert_eq!(t.depth_ceilings.soft, 12);
        assert_eq!(t.depth_ceilings.hard, 16);
        assert_eq!(t.shed_budgets.len(), 6, "one row per shed::Surface");
    }

    /// The shed-budget rows in the file match the `shed::ShedBudgetTable::v1_floor()` seed table
    /// surface-for-surface (the file is the source of truth; the seed table mirrors it).
    #[test]
    fn shed_budgets_in_file_match_the_v1_floor_table() {
        let t = Thresholds::load_canonical().expect("load");
        let v1 = crate::shed::ShedBudgetTable::v1_floor();
        for surface in [
            Surface::HttpIntake,
            Surface::CiDispatch,
            Surface::CollabOpStream,
            Surface::ConnectionTier,
            Surface::AgentMention,
            Surface::GitFrontDoor,
        ] {
            assert_eq!(
                t.shed_budget(surface).expect("present"),
                v1.budget(surface),
                "shed budget for {surface:?} must match the v1 floor table"
            );
        }
    }

    /// **P-S33: the canonical file's TUNED shed budgets validate against the §7.6 human-lane floor.**
    /// Every row is bounded, reservation-within-cap, and (human-facing) at-or-above the measured 20%
    /// floor — the load-time gate that makes "you cannot tune the human lane into starvation" hold.
    #[test]
    fn the_canonical_tuned_shed_budgets_validate() {
        let t = Thresholds::load_canonical().expect("load");
        t.validate_shed_budgets()
            .expect("the tuned shed budgets in the canonical file must validate (P-S33)");
        // the validated table is buildable from the file.
        t.shed_budget_table_validated()
            .expect("the validated tuned table builds from the file");
    }

    /// **P-S33: a thresholds file that starves a human lane FAILS the validation gate** (not edited
    /// green). A ConnectionTier reservation under the measured floor is a loud `HumanLaneStarved`.
    #[test]
    fn a_starved_human_lane_in_the_file_fails_validation() {
        let starved = r#"
            version = 1
            as_of = "2026-06-24"
            [revocation]
            sla_mins = 5
            [surge]
            multiplier = 30
            [fail_static]
            status = "OPEN — LEGAL"
            owner = "DPO / Legal"
            static_max_default_secs = 300
            agent_token_ttl_secs = 60
            constraint = "x"
            [rpo_rto]
            rpo_max_mins = 5
            rto_tenant_max_mins = 60
            rto_cell_max_mins = 240
            [depth_ceilings]
            soft = 12
            hard = 16
            [[shed_budgets]]
            surface = "HttpIntake"
            per_tenant_in_flight_cap = 200
            human_lane_reservation = 3
            retry_after_secs = 5
        "#;
        let t = Thresholds::from_toml(starved).expect("parses");
        assert!(
            matches!(
                t.validate_shed_budgets(),
                Err(ShedBudgetError::HumanLaneStarved { .. })
            ),
            "a human lane tuned under the measured floor must fail the gate (P-S33, EI-01 §3)"
        );
    }

    /// The file round-trips: parse → serialize → parse yields the identical structure.
    #[test]
    fn thresholds_file_round_trips() {
        let t = Thresholds::load_canonical().expect("load");
        let serialized = t.to_toml().expect("serialize");
        let reparsed = Thresholds::from_toml(&serialized).expect("re-parse");
        assert_eq!(t, reparsed, "parse → serialize → parse must be identity");
    }

    /// A missing threshold is a LOUD error, not a silent default (the roadmap §2 item-5 rule). A
    /// file missing a required section fails to parse rather than defaulting.
    #[test]
    fn a_missing_required_threshold_is_a_loud_error() {
        // a file missing the `surge` section: parse must FAIL (no silent multiplier default).
        let missing_surge = r#"
            version = 1
            as_of = "2026-06-19"
            [revocation]
            sla_mins = 5
            [fail_static]
            status = "OPEN — LEGAL"
            owner = "DPO / Legal"
            static_max_default_secs = 300
            agent_token_ttl_secs = 60
            constraint = "x"
            [rpo_rto]
            rpo_max_mins = 5
            rto_tenant_max_mins = 60
            rto_cell_max_mins = 240
            [depth_ceilings]
            soft = 12
            hard = 16
        "#;
        let err = Thresholds::from_toml(missing_surge).expect_err("a missing section must error");
        assert!(matches!(err, ThresholdError::Parse(_)), "got {err:?}");

        // and asking for a shed budget for a surface with no row is a loud Missing error.
        let no_budgets = r#"
            version = 1
            as_of = "2026-06-19"
            [revocation]
            sla_mins = 5
            [surge]
            multiplier = 30
            [fail_static]
            status = "OPEN — LEGAL"
            owner = "DPO / Legal"
            static_max_default_secs = 300
            agent_token_ttl_secs = 60
            constraint = "x"
            [rpo_rto]
            rpo_max_mins = 5
            rto_tenant_max_mins = 60
            rto_cell_max_mins = 240
            [depth_ceilings]
            soft = 12
            hard = 16
        "#;
        let t = Thresholds::from_toml(no_budgets).expect("parses (shed_budgets defaults empty)");
        let err = t
            .shed_budget(Surface::HttpIntake)
            .expect_err("no row → loud Missing");
        assert!(matches!(err, ThresholdError::Missing(_)), "got {err:?}");
    }

    /// The `[OPEN — LEGAL]` W carries its constraint and is a LOUD error to read as a number until
    /// the DPO ratifies it; the seed value is readable (it is NOT the default-to-beat).
    #[test]
    fn open_legal_w_carries_its_constraint_and_is_loud_to_read() {
        let t = Thresholds::load_canonical().expect("load");
        // the constraint ships regardless of the value.
        assert!(t.fail_static.constraint.contains("revocation-SLA"));
        assert!(t.fail_static.constraint.contains("agent-token-TTL"));
        assert_eq!(t.fail_static.status, "OPEN — LEGAL");
        // the value W is absent (unratified) → reading it as a number is a loud OpenLegal error.
        let err = t
            .fail_static
            .ratified_static_max_secs()
            .expect_err("W is [OPEN — LEGAL]");
        assert!(matches!(err, ThresholdError::OpenLegal(_)), "got {err:?}");
        // the engineering seed is readable and obeys the constraint (≤ revocation SLA in seconds).
        assert_eq!(t.fail_static.static_max_default_secs, 300);
        assert!(t.fail_static.static_max_default_secs <= t.revocation.sla_mins * 60);

        // a file WITH a ratified W reads cleanly through the same accessor.
        let ratified = r#"
            version = 1
            as_of = "2026-06-19"
            [revocation]
            sla_mins = 5
            [surge]
            multiplier = 30
            [fail_static]
            status = "RATIFIED"
            owner = "DPO / Legal"
            static_max_secs = 180
            static_max_default_secs = 300
            agent_token_ttl_secs = 60
            constraint = "static_max <= revocation-SLA AND static_max >= agent-token-TTL"
            [rpo_rto]
            rpo_max_mins = 5
            rto_tenant_max_mins = 60
            rto_cell_max_mins = 240
            [depth_ceilings]
            soft = 12
            hard = 16
        "#;
        let t2 = Thresholds::from_toml(ratified).expect("parse");
        assert_eq!(
            t2.fail_static.ratified_static_max_secs().expect("ratified"),
            180
        );
    }

    /// The scorecard list is honest: empty at this commit (every shipped M0 drill is green at its
    /// threshold). A red drill would add a row here — never edit a threshold green.
    #[test]
    fn scorecard_is_honest_and_empty_at_this_commit() {
        let t = Thresholds::load_canonical().expect("load");
        assert!(
            t.claimed_not_proven.is_empty(),
            "every shipped M0 drill is green at its threshold; a red one would add a row"
        );
    }
}
