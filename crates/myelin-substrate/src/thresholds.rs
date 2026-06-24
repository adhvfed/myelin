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
    /// The online-migration-under-load lock budget (contract 1.5; SUB-D10 / STOR-D8). The lock-wait
    /// p99 budget an expand→backfill→contract step may impose on concurrent writers + the 0-downtime
    /// invariant — read by the SUB-D10 drill (P-S34) + the STOR-D8 drill (P-126). The SHAPE (a
    /// measured lock-wait p99 ceiling under load + a 0-downtime invariant) is frozen; the NUMBER is the
    /// default-to-beat measured under load. `#[serde(default)]` so an older thresholds file (pre-P-126)
    /// still parses against the §9 seed.
    #[serde(default)]
    pub online_migration: OnlineMigration,
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
    /// The Refs hot-artifact backlink-read budget that triggers the R4 reach-index promotion (REF-P23
    /// / P-454; contract 5.3 at scale / 1.8 `hot_artifact_fanout`; reference-graph.md §6.3). The
    /// inbound-fanout (the count of live inbound edges to a `target_root`) above which the read-time
    /// CTE scan is MEASURED to fall over its p99 budget, so the Leopard-style flattened reach index R4
    /// is promoted to serve that hot target (R5 — measured-trigger, never predicted). `#[serde(default)]`
    /// so an older thresholds file (pre-P-454) still parses against the §6.3 seed.
    #[serde(default)]
    pub refs_hot_artifact: RefsHotArtifact,
    /// The MEASURED cell sizing-band numbers (P-CP-22 / P-431; tenancy §7.1 / ADR-10). The per-cell-
    /// class `tenants_max` + which capacity dimension BINDS FIRST, set from the load-test + the
    /// `cell_utilisation` telemetry — replacing the conservative §5.1 defaults. The avoid-migration-
    /// by-sizing floor (P-CP-05/P-CP-07) is promoted: these are the MEASURED defaults-to-beat (never
    /// predicted, ADR-10 — the binding dimension is discovered by measurement). `#[serde(default)]` so
    /// an older thresholds file (pre-P-CP-22) still parses (falls back to the conservative seeds).
    #[serde(default)]
    pub cell_sizing: CellSizing,
    /// The MEASURED resilient-client per-target tuned values (contract 1.9, §6.3; P-S36 / P-437).
    /// One row per logical downstream target the resilient client (`myelin-client`) calls — the auth
    /// hot path gets a TIGHTER timeout than a batch indexer, each number measured by the surge/latency
    /// drills (SUB-D3 P-S32/P-433, the per-surface tuning P-S33/P-434), NOT predicted (EI-01 §3).
    /// `#[serde(default)]` so an older thresholds file (pre-P-437) still parses with no per-target
    /// rows (the M0 default-per-target floor, `ResilientConfig::default`, still applies). The M0 floor
    /// named in P-S16 / P-033 is CLOSED by this row set.
    #[serde(default)]
    pub resilient_client: Vec<ResilientTargetRow>,
    /// The column-store / time-series promotion seam (BUS-6; event-bus §7.5; P-440 / EB-31, M5). The
    /// MEASUREMENT GATE — not a build — that promotes a durable stream to a ClickHouse-class column
    /// tier (behind the unchanged `BusTransport` trait, contract 2.1). `#[serde(default)]` so an older
    /// thresholds file (pre-P-440) still parses against the §7.5 seed.
    #[serde(default)]
    pub column_store_seam: ColumnStoreSeam,
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

/// The online-migration-under-load lock budget (contract 1.5; architecture §9.1/§9.2 — the lock-time-
/// against-a-restore rule). The SUB-D10 (P-S34) + STOR-D8 (P-126) drills read these: an
/// expand→backfill→contract step's p99 lock-wait must stay within `lock_wait_p99_max_ms` (a SHORT
/// metadata/catalog lock, never a table-rewrite lock at write QPS) and the migration must cause 0
/// downtime (`downtime_max_ms == 0` — an online migration NEVER takes the table offline). The SHAPE is
/// frozen; the NUMBER is the default-to-beat measured under load (re-confirmed at cell scale in the M5
/// world-scale follow-on). Never weaken either to pass (EI-01 §3) — a red is a dated
/// `[[claimed_not_proven]]` row.
///
/// This is the substrate-side TYPED accessor for the `[online_migration]` section the thresholds file
/// carries (P-126 wrote the section; this types it so the SUB-D10 drill reads it through the SAME
/// [`Thresholds`] loader every other substrate drill uses, not by re-parsing the TOML). The
/// storage-tier `myelin_storage::LockBudget` is constructed from these two numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnlineMigration {
    /// The maximum p99 lock-wait, in MILLISECONDS, an online migration step may impose on concurrent
    /// writers. A step over this is a blocking lock at write QPS, not the online idiom — the drill FAILs.
    pub lock_wait_p99_max_ms: u64,
    /// The maximum downtime, in MILLISECONDS, the migration may cause. `0` is the 0-downtime invariant:
    /// an online migration NEVER takes the table offline (drill rows SUB-D10 / STOR-D8).
    pub downtime_max_ms: u64,
}

impl Default for OnlineMigration {
    /// The §9 seed default-to-beat: a 500 ms lock-wait p99 ceiling (a SHORT metadata/catalog lock) and
    /// the 0-downtime invariant. Mirrors the `[online_migration]` row in the canonical file; an older
    /// thresholds file (pre-P-126) falls back to this.
    fn default() -> Self {
        OnlineMigration {
            lock_wait_p99_max_ms: 500,
            downtime_max_ms: 0,
        }
    }
}

/// The column-store / time-series promotion seam (BUS-6; event-bus §7.5; P-440 / EB-31, M5).
///
/// The highest-volume durable streams keep a seam for a column-store / time-series engine
/// (ClickHouse-class, aligning with the OLAP read store, ADR-10) **behind the unchanged
/// `BusTransport` trait** (contract 2.1 — `put`/`consume`/`ack`/`purge`, so the swap is a relay-target
/// change, never a consumer rewrite). The canonical posture (event-bus §7.5, EI-04 §5.2) is
/// **specified-not-built**: *do not add the column tier before the volume is MEASURED.* Until a
/// per-stream volume is measured to outgrow the JetStream tier at degraded latency, the 90-day-hot log
/// + the OLAP long-term holder suffice.
///
/// This struct records the **measurement gate**, not a build: the per-stream throughput threshold +
/// the degraded-latency criterion that, IF a real stream is MEASURED to cross BOTH, owes a promotion
/// follow-on prompt (post-M5, measured-not-predicted). The SHAPE is frozen here (P-440); the NUMBERS
/// are the §7.5 promotion-criterion seeds — a stream is a promotion CANDIDATE iff its measured sustained
/// publish rate exceeds `promote_events_per_sec_per_stream` AND, at that rate, the JetStream tier serves
/// it at a per-aggregate publish-latency p99 over `degraded_publish_latency_p99_ms` (degraded). Neither
/// is a value to "beat": it is the threshold that, once a real measurement crosses it, FLIPS the seam
/// from named-not-built to owed-a-build (the gate is the promotion trigger, EI-04 §5.2).
///
/// `promotion_owed` records whether any production stream has been MEASURED to cross both criteria. It
/// is `false` at this commit — **no production volume has been measured to outgrow JetStream**, so the
/// seam stays NAMED, no build is owed (the honest state, EI-01 §3: a seam is not a floor with a dated
/// follow-on until a measurement makes it one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnStoreSeam {
    /// The measured sustained per-stream publish rate (events/sec on a SINGLE durable stream) above
    /// which a stream becomes a column-tier promotion CANDIDATE. A stream below this stays on the
    /// JetStream tier (§7.5: the hot log + OLAP holder suffice). MEASURED-not-predicted: this is the
    /// criterion a real measurement is compared against, never a value the platform tunes toward.
    pub promote_events_per_sec_per_stream: u64,
    /// The per-aggregate publish-latency p99, in MILLISECONDS, above which the JetStream tier is judged
    /// DEGRADED at the candidate stream's volume — the SECOND half of the promotion trigger. A candidate
    /// stream is promoted ONLY if, at its measured rate, the tier's measured publish-latency p99 crosses
    /// this (volume alone is not enough — the tier must be measurably degraded, EI-04 §5.2).
    pub degraded_publish_latency_p99_ms: u64,
    /// Whether a production stream has been MEASURED to cross BOTH criteria (volume AND degraded
    /// latency) → a build is owed. `false` at this commit: no measured volume outgrows JetStream, so the
    /// seam stays specified-not-built (no dated follow-on prompt is owed until a measurement flips this).
    pub promotion_owed: bool,
}

impl Default for ColumnStoreSeam {
    /// The §7.5 promotion-criterion seeds. A durable JetStream stream sustaining over 50 000 events/sec
    /// AND served at a per-aggregate publish-latency p99 over 100 ms at that rate is a promotion
    /// candidate (the column tier is then owed, behind the unchanged `BusTransport`). Neither number is
    /// "beaten" — they are the thresholds a real measurement is compared against. `promotion_owed` is
    /// `false`: no production volume has been measured to cross them, so the seam stays NAMED (no build).
    /// An older thresholds file (pre-P-440) falls back to this.
    fn default() -> Self {
        ColumnStoreSeam {
            promote_events_per_sec_per_stream: 50_000,
            degraded_publish_latency_p99_ms: 100,
            promotion_owed: false,
        }
    }
}

impl ColumnStoreSeam {
    /// **The promotion-gate decision (the measurement gate, not a build).** Given a stream's MEASURED
    /// sustained publish rate (events/sec) and its MEASURED per-aggregate publish-latency p99 (ms) at
    /// that rate, decide whether the column-store tier is owed for it. Promotion is owed **iff BOTH**
    /// criteria are crossed: the rate exceeds [`Self::promote_events_per_sec_per_stream`] AND the
    /// latency p99 exceeds [`Self::degraded_publish_latency_p99_ms`] (volume alone never promotes — the
    /// tier must be measurably DEGRADED at that volume, §7.5 / EI-04 §5.2). A stream below either stays
    /// on JetStream (named-not-built). This is the single decision the seam exposes: it never builds the
    /// tier, it only reads a measurement and reports whether a build is owed.
    pub fn promotion_owed_for(
        &self,
        measured_events_per_sec: u64,
        measured_publish_latency_p99_ms: u64,
    ) -> bool {
        measured_events_per_sec > self.promote_events_per_sec_per_stream
            && measured_publish_latency_p99_ms > self.degraded_publish_latency_p99_ms
    }
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

/// The Refs hot-artifact backlink-read budget that triggers the R4 reach-index promotion (REF-P23 /
/// P-454; contract 5.3 at scale / 1.8 `hot_artifact_fanout`; architecture §6.3).
///
/// The §6.3 hot-artifact backlink scale (the "viral PR / referenced-by-50,000" case): the BUILT floor
/// (REF-P11) is the read-time CTE + `list_objects` filter + pagination + replica — you never
/// materialise 50,000 backlinks, you PAGE them. The FOLLOW-ON (REF-P23) is the Leopard-style flattened
/// reach index R4, **promoted ONLY when MEASURED hot-fanout exceeds the read budget (R5), not
/// predicted** (ADR-10 measure-before-shard). `read_budget_fanout` is R5: the inbound-fanout above
/// which the CTE scan is measured to fall over its p99 budget, so R4 is promoted to serve that hot
/// target. The trigger is STRICTLY greater-than (a target AT the budget still serves from the CTE
/// floor — the §6.3 "exceeding the read budget" wording). `#[serde(default)]` + [`Default`] so an
/// older thresholds file (pre-P-454) parses against the §6.3 seed. Mirrors
/// `myelin_refs_service::reach_index::R4_READ_BUDGET_FANOUT` (the seed constant); this file is its
/// source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefsHotArtifact {
    /// R5 — the inbound-fanout (the count of live inbound edges to a `target_root`) above which the
    /// read-time CTE scan is MEASURED to fall over its p99 budget, so the R4 reach index is promoted
    /// to serve that hot target. STRICTLY greater-than (a target AT the budget still serves from the
    /// CTE floor). MEASURED-not-predicted (the `hot_artifact_fanout` telemetry measures the real
    /// fanout; promotion is acted on only when a real target crosses this).
    pub read_budget_fanout: u64,
}

impl Default for RefsHotArtifact {
    /// The §6.3 seed default-to-beat: a read-budget fanout of 1000 — a target with more than 1000 live
    /// inbound edges is a hot artifact the read-time CTE scan is measured to fall over its p99 budget
    /// for, so R4 is promoted to serve it. The world-scale fleet-hardware re-measure of the real
    /// crossover is the named M5 floor (REF-P23). An older thresholds file (pre-P-454) falls back here.
    fn default() -> Self {
        RefsHotArtifact {
            read_budget_fanout: 1000,
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

/// A MEASURED resilient-client per-target tuned-value row (contract 1.9, §6.3; P-S36 / P-437).
///
/// One logical downstream target the resilient client calls, with its per-target value set tuned to
/// the numbers MEASURED by the surge/latency drills — the auth hot path's timeout is tighter than a
/// batch indexer's. The SHAPE and the on-by-default posture are unchanged from the M0 floor
/// ([`myelin_client::ResilientConfig`]); only the per-target NUMBERS are tuned here.
///
/// `latency_budget_ms` is the MEASURED p99 latency budget for the target (the surge/latency drill
/// number, headroom included). The gate ([`Thresholds::validate_resilient_targets`]) enforces that
/// the tuned `timeout_ms` is **not looser** than this budget: a `timeout_ms > latency_budget_ms` is a
/// value tuned looser than what the drill measured the target can take within budget — that is the
/// regression this gate rejects (EI-01 §3 — never a softened bar). The timeout must also be no
/// tighter than the measured budget would allow a legitimate call to complete; the budget IS the
/// timeout's ceiling (a deadline beyond the budget means a slow call is admitted past the SLO it was
/// measured against, the "looser-than-budget" failure mode).
///
/// NB: `Eq` is intentionally NOT derived — [`Self::breaker_failure_ratio`] is an `f64` (`PartialEq`
/// but not `Eq`); `PartialEq` suffices for the round-trip + value tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResilientTargetRow {
    /// The logical downstream target name — matches the [`myelin_client::Target`] string the
    /// resilient client keys breaker/bulkhead on (e.g. `"identity-authz"`, `"search-index"`).
    pub target: String,
    /// `true` iff this is an interactive/auth HOT-PATH target (the auth-decision spine: the
    /// highest-QPS, latency-critical downstream). The gate asserts at least one hot-path target's
    /// `timeout_ms` is STRICTLY tighter than every non-hot-path (batch) target's — the
    /// "auth-hot-path-tighter-than-batch-indexer" relation (§6.3, the P-S16 floor's stated tuning).
    pub hot_path: bool,
    /// The MEASURED p99 latency budget for this target, in **milliseconds** (the surge/latency drill
    /// number, headroom included). The tuned `timeout_ms` MUST NOT exceed this (a looser deadline is
    /// the regression the gate rejects).
    pub latency_budget_ms: u64,
    /// The tuned per-call deadline in **milliseconds** (§6.3). Tighter for the auth hot path than for
    /// a batch indexer.
    pub timeout_ms: u64,
    /// The tuned full-jitter backoff base in **milliseconds** (§6.3).
    pub backoff_base_ms: u64,
    /// The tuned maximum number of attempts for an `Idempotent` call (1 = no retry).
    pub max_attempts: u32,
    /// The tuned breaker trip threshold: the rolling-window failure **ratio** in `[0.0, 1.0]`.
    pub breaker_failure_ratio: f64,
    /// The tuned breaker minimum request count.
    pub breaker_min_requests: u32,
    /// The tuned breaker rolling-window size.
    pub breaker_window: u32,
    /// The tuned breaker open duration in **milliseconds**.
    pub breaker_open_ms: u64,
    /// The tuned per-target bulkhead integer concurrency cap (§6.3).
    pub bulkhead_max_concurrency: u32,
}

impl ResilientTargetRow {
    /// Build the tuned [`myelin_client::ResilientConfig`] this row carries — the SHAPE is unchanged
    /// from the M0 floor; only the per-target NUMBERS are the tuned values. This is the ONE place a
    /// thresholds-file row becomes the config the resilient client runs with for the target.
    pub fn to_config(&self) -> myelin_client::ResilientConfig {
        myelin_client::ResilientConfig {
            timeout_ms: self.timeout_ms,
            max_attempts: self.max_attempts,
            backoff_base_ms: self.backoff_base_ms,
            breaker_failure_ratio: self.breaker_failure_ratio,
            breaker_min_requests: self.breaker_min_requests,
            breaker_window: self.breaker_window,
            breaker_open_ms: self.breaker_open_ms,
            bulkhead_max_concurrency: self.bulkhead_max_concurrency,
        }
    }
}

/// A resilient-client per-target tuning that violates the §6.3 measured-tuning discipline (P-S36).
/// LOUD by construction — a row that tuned a value looser than its measured budget, or broke the
/// auth-hot-path-tighter relation, fails at LOAD, never silently at runtime under surge (EI-01 §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResilientTuningError {
    /// A target's tuned `timeout_ms` is LOOSER (greater) than its MEASURED `latency_budget_ms` — the
    /// regression the gate rejects (a deadline beyond the measured budget admits a call past its SLO).
    TimeoutLooserThanBudget {
        /// The offending target name.
        target: String,
        /// The tuned timeout (ms).
        timeout_ms: u64,
        /// The measured latency budget (ms) it exceeded.
        latency_budget_ms: u64,
    },
    /// A degenerate per-target value (a zero deadline, a zero bulkhead, a zero window/attempt count,
    /// or a ratio outside `[0.0, 1.0]`) — an unbounded/disabled primitive is a future cascade.
    DegenerateValue {
        /// The offending target name.
        target: String,
        /// Which field was degenerate.
        field: String,
    },
    /// The auth-hot-path-tighter-than-batch-indexer relation is broken: a non-hot-path (batch) target
    /// has a `timeout_ms` at-or-tighter-than the tightest hot-path target's. The auth hot path MUST be
    /// strictly tighter than every batch target (§6.3, the P-S16 floor's stated tuning).
    HotPathNotTighter {
        /// The tightest hot-path timeout found (ms).
        tightest_hot_path_ms: u64,
        /// The offending batch target.
        batch_target: String,
        /// Its (too-tight) timeout (ms).
        batch_timeout_ms: u64,
    },
    /// No hot-path target is declared at all — the tuned set must name the auth hot path so the
    /// tighter-than relation is meaningful (an empty hot-path set is a silently-missing floor).
    NoHotPathTarget,
}

impl std::fmt::Display for ResilientTuningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResilientTuningError::TimeoutLooserThanBudget {
                target,
                timeout_ms,
                latency_budget_ms,
            } => write!(
                f,
                "resilient-client target `{target}`: tuned timeout {timeout_ms}ms is LOOSER than the \
                 measured latency budget {latency_budget_ms}ms — a value tuned looser than the \
                 measured budget fails the gate (EI-01 §3; never softened)"
            ),
            ResilientTuningError::DegenerateValue { target, field } => write!(
                f,
                "resilient-client target `{target}`: degenerate tuned value for `{field}` (a \
                 zeroed/out-of-range primitive is a future cascade)"
            ),
            ResilientTuningError::HotPathNotTighter {
                tightest_hot_path_ms,
                batch_target,
                batch_timeout_ms,
            } => write!(
                f,
                "resilient-client tuning: batch target `{batch_target}` timeout {batch_timeout_ms}ms \
                 is not looser than the auth hot path's {tightest_hot_path_ms}ms — the auth hot path \
                 MUST be strictly tighter than every batch indexer (§6.3)"
            ),
            ResilientTuningError::NoHotPathTarget => write!(
                f,
                "resilient-client tuning: no hot-path target declared — the tuned set must name the \
                 auth hot path so the tighter-than-batch relation holds"
            ),
        }
    }
}

impl std::error::Error for ResilientTuningError {}

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

    /// The tuned [`myelin_client::ResilientConfig`] for ONE target (contract 1.9, §6.3; P-S36). A
    /// target with no tuned row is a loud [`ThresholdError::Missing`] — the resilient client does NOT
    /// proceed against a guessed per-target value (it falls back to `ResilientConfig::default` only
    /// where the CALLER explicitly opts into the M0 floor, never by silently swallowing a missing row).
    pub fn resilient_config(
        &self,
        target: &str,
    ) -> Result<myelin_client::ResilientConfig, ThresholdError> {
        self.resilient_client
            .iter()
            .find(|r| r.target == target)
            .map(ResilientTargetRow::to_config)
            .ok_or_else(|| ThresholdError::Missing(format!("resilient_client.{target}")))
    }

    /// **Validate the file's TUNED resilient-client per-target values (P-S36, the §6.3 measured-tuning
    /// discipline).** Each row must hold: every primitive bounded (no zeroed deadline/bulkhead/window/
    /// attempt, ratio in `[0.0, 1.0]`); the tuned `timeout_ms` no LOOSER than its measured
    /// `latency_budget_ms` (the looser-than-budget regression — the gate's headline); and the
    /// auth-hot-path-tighter-than-batch-indexer relation (every batch target strictly looser than the
    /// tightest hot-path target). A row that tuned a value looser than its measured budget — or that
    /// broke the tighter-than relation — fails at LOAD, never silently under surge (EI-01 §3 — a red is
    /// a dated `[[claimed_not_proven]]` row, never a softened bar). An EMPTY row set is vacuously valid
    /// (the M0 default-per-target floor still applies); a NON-empty set must declare a hot-path target.
    pub fn validate_resilient_targets(&self) -> Result<(), ResilientTuningError> {
        if self.resilient_client.is_empty() {
            return Ok(());
        }
        // (1) Per-row: bounded primitives + the looser-than-budget headline.
        for row in &self.resilient_client {
            let degenerate = |field: &str| ResilientTuningError::DegenerateValue {
                target: row.target.clone(),
                field: field.to_string(),
            };
            if row.timeout_ms == 0 {
                return Err(degenerate("timeout_ms"));
            }
            if row.latency_budget_ms == 0 {
                return Err(degenerate("latency_budget_ms"));
            }
            if row.bulkhead_max_concurrency == 0 {
                return Err(degenerate("bulkhead_max_concurrency"));
            }
            if row.breaker_window == 0 {
                return Err(degenerate("breaker_window"));
            }
            if row.breaker_min_requests == 0 {
                return Err(degenerate("breaker_min_requests"));
            }
            if row.max_attempts == 0 {
                return Err(degenerate("max_attempts"));
            }
            if !(0.0..=1.0).contains(&row.breaker_failure_ratio) {
                return Err(degenerate("breaker_failure_ratio"));
            }
            // THE HEADLINE GATE: a timeout tuned LOOSER than the measured latency budget fails.
            if row.timeout_ms > row.latency_budget_ms {
                return Err(ResilientTuningError::TimeoutLooserThanBudget {
                    target: row.target.clone(),
                    timeout_ms: row.timeout_ms,
                    latency_budget_ms: row.latency_budget_ms,
                });
            }
        }
        // (2) The auth-hot-path-tighter-than-batch-indexer relation. The tightest hot-path timeout
        // must be STRICTLY tighter than every batch (non-hot-path) target's.
        let tightest_hot_path = self
            .resilient_client
            .iter()
            .filter(|r| r.hot_path)
            .map(|r| r.timeout_ms)
            .min();
        let Some(tightest_hot_path_ms) = tightest_hot_path else {
            return Err(ResilientTuningError::NoHotPathTarget);
        };
        for row in self.resilient_client.iter().filter(|r| !r.hot_path) {
            if row.timeout_ms <= tightest_hot_path_ms {
                return Err(ResilientTuningError::HotPathNotTighter {
                    tightest_hot_path_ms,
                    batch_target: row.target.clone(),
                    batch_timeout_ms: row.timeout_ms,
                });
            }
        }
        Ok(())
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
        "RefsBacklinkRead" => Surface::RefsBacklinkRead,
        "RefsRefCreate" => Surface::RefsRefCreate,
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
        | Surface::GitFrontDoor
        | Surface::RefsBacklinkRead
        | Surface::RefsRefCreate => Ok(s),
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
        assert_eq!(
            t.online_migration.lock_wait_p99_max_ms, 500,
            "SUB-D10/STOR-D8: online-migration lock-wait p99 budget = 500 ms"
        );
        assert_eq!(
            t.online_migration.downtime_max_ms, 0,
            "SUB-D10/STOR-D8: the 0-downtime invariant is structural"
        );
        assert_eq!(t.depth_ceilings.soft, 12);
        assert_eq!(t.depth_ceilings.hard, 16);
        assert_eq!(t.shed_budgets.len(), 8, "one row per shed::Surface");
        assert_eq!(
            t.resilient_client.len(),
            4,
            "the measured resilient-client per-target tuned rows (P-S36): authz/event-bus + 2 batch"
        );
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

    /// **SUB-D10 (P-S34): the online-migration lock budget is read through the typed [`Thresholds`]
    /// loader, not by re-parsing the TOML.** The canonical file carries the `[online_migration]`
    /// section (P-126 wrote it); this types it so the SUB-D10 drill reads it via the SAME loader every
    /// other substrate drill uses (the single source of truth, EI-01 §3 — never a hardcoded number).
    #[test]
    fn online_migration_budget_reads_through_the_typed_loader() {
        let t = Thresholds::load_canonical().expect("load");
        assert_eq!(t.online_migration.lock_wait_p99_max_ms, 500);
        assert_eq!(t.online_migration.downtime_max_ms, 0);
    }

    /// An older thresholds file WITHOUT the `[online_migration]` section still parses (the field is
    /// `#[serde(default)]`) and falls back to the §9 seed — so a pre-P-126 file is not a parse error.
    #[test]
    fn an_older_file_without_online_migration_falls_back_to_the_seed() {
        let pre_p126 = r#"
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
        let t = Thresholds::from_toml(pre_p126).expect("a pre-P-126 file parses");
        assert_eq!(
            t.online_migration,
            OnlineMigration::default(),
            "an absent [online_migration] falls back to the §9 seed"
        );
        assert_eq!(t.online_migration.lock_wait_p99_max_ms, 500);
        assert_eq!(t.online_migration.downtime_max_ms, 0);
    }

    // ───────────────────── REF-P23 / P-454: the Refs hot-artifact R4 read budget ─────────────────────

    /// **REF-P23 (P-454): the Refs hot-artifact read budget (R5) is read through the typed
    /// [`Thresholds`] loader, not by re-parsing the TOML.** The canonical file carries the
    /// `[refs_hot_artifact]` section; this types it so the R4 reach-index promotion gate reads it via
    /// the SAME loader every other Refs drill uses (the single source of truth, EI-01 §3 — never a
    /// hardcoded number). The §6.3 seed default-to-beat is 1000.
    #[test]
    fn refs_hot_artifact_budget_reads_through_the_typed_loader() {
        let t = Thresholds::load_canonical().expect("load");
        assert_eq!(
            t.refs_hot_artifact.read_budget_fanout, 1000,
            "the §6.3 R5 read-budget fanout seed (R4 promotes above this)"
        );
        assert!(
            t.refs_hot_artifact.read_budget_fanout > 0,
            "the read budget must be a positive fanout (a 0 budget would promote R4 vacuously)"
        );
    }

    /// An older thresholds file WITHOUT the `[refs_hot_artifact]` section still parses (the field is
    /// `#[serde(default)]`) and falls back to the §6.3 seed — so a pre-P-454 file is not a parse error.
    #[test]
    fn an_older_file_without_refs_hot_artifact_falls_back_to_the_seed() {
        let pre_p454 = r#"
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
        let t = Thresholds::from_toml(pre_p454).expect("a pre-P-454 file parses");
        assert_eq!(
            t.refs_hot_artifact,
            RefsHotArtifact::default(),
            "an absent [refs_hot_artifact] falls back to the §6.3 seed"
        );
        assert_eq!(t.refs_hot_artifact.read_budget_fanout, 1000);
    }

    // ───────────────────── P-S36 / P-437: resilient-client per-target tuning ─────────────────────

    /// A minimal valid thresholds body with a tunable `[[resilient_client]]` set spliced in, so the
    /// tuning tests are self-contained (no dependence on the canonical file's exact numbers).
    fn thresholds_with_resilient(rows_toml: &str) -> Thresholds {
        let body = format!(
            r#"
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
            {rows_toml}
        "#
        );
        Thresholds::from_toml(&body).expect("the resilient-client test body parses")
    }

    /// A full valid tuned row for one target, parameterised so a test can perturb a single field.
    fn resilient_row(target: &str, hot_path: bool, budget: u64, timeout: u64) -> String {
        format!(
            r#"
            [[resilient_client]]
            target = "{target}"
            hot_path = {hot_path}
            latency_budget_ms = {budget}
            timeout_ms = {timeout}
            backoff_base_ms = 20
            max_attempts = 3
            breaker_failure_ratio = 0.5
            breaker_min_requests = 5
            breaker_window = 20
            breaker_open_ms = 2000
            bulkhead_max_concurrency = 64
        "#
        )
    }

    /// **THE HEADLINE GATE (P-S36 DoD):** a per-target value tuned LOOSER than the measured latency
    /// budget FAILS the gate — never edited green (EI-01 §3). A hot-path target whose `timeout_ms`
    /// exceeds its measured `latency_budget_ms` is a loud `TimeoutLooserThanBudget`.
    #[test]
    fn a_timeout_looser_than_the_measured_budget_fails_the_gate() {
        // budget 150 ms, timeout 200 ms — LOOSER than the measured budget.
        let rows = resilient_row("identity-authz", true, 150, 200);
        let t = thresholds_with_resilient(&rows);
        assert!(
            matches!(
                t.validate_resilient_targets(),
                Err(ResilientTuningError::TimeoutLooserThanBudget {
                    timeout_ms: 200,
                    latency_budget_ms: 150,
                    ..
                })
            ),
            "a timeout tuned looser than the measured latency budget MUST fail the gate (P-S36)"
        );
    }

    /// The complementary green: a timeout tuned AT-OR-UNDER its measured budget validates (the
    /// looser-than-budget gate does not reject a correctly-tightened value).
    #[test]
    fn a_timeout_within_the_measured_budget_validates() {
        let mut rows = resilient_row("identity-authz", true, 150, 120); // hot, tight
        rows.push_str(&resilient_row("search-index", false, 30000, 25000)); // batch, loose
        let t = thresholds_with_resilient(&rows);
        t.validate_resilient_targets()
            .expect("a hot-path timeout within budget + a looser batch target must validate");
    }

    /// **The auth-hot-path-tighter-than-batch-indexer relation holds (P-S36 DoD).** A batch target
    /// whose timeout is NOT strictly looser than the tightest hot-path target's fails the gate.
    #[test]
    fn the_auth_hot_path_must_be_tighter_than_the_batch_indexer() {
        // hot path 120 ms; a "batch" target tuned to 100 ms — TIGHTER than the hot path (backwards).
        let mut rows = resilient_row("identity-authz", true, 150, 120);
        rows.push_str(&resilient_row("search-index", false, 30000, 100));
        let t = thresholds_with_resilient(&rows);
        assert!(
            matches!(
                t.validate_resilient_targets(),
                Err(ResilientTuningError::HotPathNotTighter {
                    tightest_hot_path_ms: 120,
                    batch_timeout_ms: 100,
                    ..
                })
            ),
            "a batch target tighter-than-or-equal to the hot path must fail the relation gate (P-S36)"
        );
    }

    /// A non-empty tuned set with NO hot-path target declared is a loud `NoHotPathTarget` (the
    /// tighter-than relation is meaningless without naming the auth hot path).
    #[test]
    fn a_tuned_set_with_no_hot_path_target_fails() {
        let rows = resilient_row("search-index", false, 30000, 25000); // batch only
        let t = thresholds_with_resilient(&rows);
        assert_eq!(
            t.validate_resilient_targets(),
            Err(ResilientTuningError::NoHotPathTarget),
            "a non-empty tuned set must name the auth hot path (P-S36)"
        );
    }

    /// A degenerate primitive (a zeroed deadline) is rejected — an unbounded/disabled primitive is a
    /// future cascade, never silently accepted.
    #[test]
    fn a_zeroed_deadline_fails_the_gate() {
        let rows = resilient_row("identity-authz", true, 150, 0);
        let t = thresholds_with_resilient(&rows);
        assert!(
            matches!(
                t.validate_resilient_targets(),
                Err(ResilientTuningError::DegenerateValue { field, .. }) if field == "timeout_ms"
            ),
            "a zeroed per-call deadline must fail the gate (P-S36)"
        );
    }

    /// An EMPTY tuned set is vacuously valid — the M0 default-per-target floor still applies (an
    /// older pre-P-437 file with no rows still loads + validates).
    #[test]
    fn an_empty_tuned_set_is_vacuously_valid() {
        let t = thresholds_with_resilient("");
        assert!(t.resilient_client.is_empty());
        t.validate_resilient_targets()
            .expect("an empty tuned set is vacuously valid (the M0 floor applies)");
    }

    /// **The canonical file's tuned resilient-client values VALIDATE (the dated green artifact).** The
    /// per-target numbers in `thresholds.toml` hold the looser-than-budget gate AND the
    /// auth-hot-path-tighter-than-batch-indexer relation — the M0 default-per-target floor (P-S16) is
    /// CLOSED with measured numbers.
    #[test]
    fn the_canonical_tuned_resilient_targets_validate() {
        let t = Thresholds::load_canonical().expect("load");
        assert!(
            !t.resilient_client.is_empty(),
            "the canonical file ships measured per-target rows (P-S36 closed the M0 floor)"
        );
        t.validate_resilient_targets()
            .expect("the canonical tuned resilient-client values must validate (P-S36, EI-01 §3)");
    }

    /// **The auth-hot-path-tighter-than-batch-indexer relation holds in the CANONICAL file** (the
    /// concrete P-S36 claim): `identity-authz` (hot) has a strictly tighter timeout than
    /// `search-index` (the batch indexer).
    #[test]
    fn canonical_auth_hot_path_is_tighter_than_the_batch_indexer() {
        let t = Thresholds::load_canonical().expect("load");
        let authz = t
            .resilient_config("identity-authz")
            .expect("the auth hot-path target is tuned in the canonical file");
        let indexer = t
            .resilient_config("search-index")
            .expect("the batch-indexer target is tuned in the canonical file");
        assert!(
            authz.timeout_ms < indexer.timeout_ms,
            "the auth hot path ({}ms) must be tighter than the batch indexer ({}ms) (§6.3, P-S36)",
            authz.timeout_ms,
            indexer.timeout_ms
        );
    }

    /// A missing per-target row is a LOUD `Missing` error — the resilient client does not proceed
    /// against a guessed per-target value.
    #[test]
    fn an_unknown_resilient_target_is_a_loud_missing_error() {
        let t = Thresholds::load_canonical().expect("load");
        assert!(matches!(
            t.resilient_config("no-such-target"),
            Err(ThresholdError::Missing(_))
        ));
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

    /// **BUS-6 column-store seam (P-440): the measurement gate is RECORDED, not a build.** The
    /// canonical file carries the promotion criteria, and at this commit NO production volume has been
    /// measured to outgrow JetStream → `promotion_owed == false` (the seam stays NAMED, no build owed).
    #[test]
    fn column_store_seam_is_named_not_built_at_this_commit() {
        let t = Thresholds::load_canonical().expect("load");
        assert!(
            !t.column_store_seam.promotion_owed,
            "BUS-6: no measured volume outgrows JetStream → the seam stays specified-not-built (§7.5)"
        );
        assert!(
            t.column_store_seam.promote_events_per_sec_per_stream > 0
                && t.column_store_seam.degraded_publish_latency_p99_ms > 0,
            "BUS-6: the promotion criteria are recorded (a measurement is compared against them)"
        );
    }

    /// **BUS-6 promotion gate is REAL, not vacuous (the measurement decision).** A stream measured
    /// BELOW either criterion stays on JetStream; only a stream that crosses BOTH (volume AND degraded
    /// latency) owes the column tier (§7.5 / EI-04 §5.2 — volume alone never promotes).
    #[test]
    fn column_store_promotion_owed_only_when_both_criteria_cross() {
        let seam = ColumnStoreSeam::default(); // 50_000 ev/s, 100 ms p99.
                                               // Below the rate (whatever the latency) → JetStream suffices, no build owed.
        assert!(!seam.promotion_owed_for(10_000, 500));
        // Above the rate but NOT degraded (p99 within budget) → still no build (the tier is fine).
        assert!(!seam.promotion_owed_for(80_000, 50));
        // Degraded latency but below the rate → not a candidate (volume too small).
        assert!(!seam.promotion_owed_for(40_000, 500));
        // BOTH crossed → the column tier is owed (the gate flips to a build, behind BusTransport).
        assert!(seam.promotion_owed_for(80_000, 200));
        // Exactly at a criterion does NOT cross (strict `>` — the threshold is a floor to exceed).
        assert!(!seam.promotion_owed_for(50_000, 200));
        assert!(!seam.promotion_owed_for(80_000, 100));
    }
}
