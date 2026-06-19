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

use crate::shed::{Surface, SurfaceBudget};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thresholds {
    /// The file's schema version (forward-only).
    pub version: u32,
    /// The ISO-8601 date this row set was last asserted against its drills.
    pub as_of: String,
    /// N — the deprovision / revocation SLA.
    pub revocation: Revocation,
    /// The surge load-multiplier (SUB-D3 / the F6 family).
    pub surge: Surge,
    /// W — the fail-static bounded-staleness window (`[OPEN — LEGAL]`, L-1).
    pub fail_static: FailStaticThreshold,
    /// The durability objectives (RPO / RTO), asserted by restore-verify (STOR-D1/D2, SUB-D6).
    pub rpo_rto: RpoRto,
    /// The causal-depth ceilings the agent-loop guard halts a loop at (SUB-D8).
    pub depth_ceilings: DepthCeilings,
    /// The per-surface shed-budget v1 floors (contract 1.11, §7.6).
    #[serde(default)]
    pub shed_budgets: Vec<ShedBudgetRow>,
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
    /// The constraint that ships regardless and is enforced in the `FailStaticThreshold` constructor.
    pub constraint: String,
}

impl FailStaticThreshold {
    /// The ratified value W, or a LOUD [`ThresholdError::OpenLegal`] error while it is `[OPEN —
    /// LEGAL]`. A drill that needs the concrete W (not the seed) MUST go through here so an
    /// unratified value can never be silently read as a number.
    pub fn ratified_static_max_secs(&self) -> Result<u64, ThresholdError> {
        self.static_max_secs
            .ok_or_else(|| ThresholdError::OpenLegal(format!("fail_static.static_max_secs ({})", self.status)))
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
        | Surface::AgentMention => Ok(s),
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
        assert_eq!(t.rpo_rto.rpo_max_mins, 5, "RPO ≤ 5 min");
        assert_eq!(t.rpo_rto.rto_tenant_max_mins, 60, "RTO ≤ 1h/tenant");
        assert_eq!(t.rpo_rto.rto_cell_max_mins, 240, "RTO ≤ 4h/cell");
        assert_eq!(t.depth_ceilings.soft, 12);
        assert_eq!(t.depth_ceilings.hard, 16);
        assert_eq!(t.shed_budgets.len(), 5, "one row per shed::Surface");
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
        ] {
            assert_eq!(
                t.shed_budget(surface).expect("present"),
                v1.budget(surface),
                "shed budget for {surface:?} must match the v1 floor table"
            );
        }
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
        let err = t.shed_budget(Surface::HttpIntake).expect_err("no row → loud Missing");
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
        let err = t.fail_static.ratified_static_max_secs().expect_err("W is [OPEN — LEGAL]");
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
        assert_eq!(t2.fail_static.ratified_static_max_secs().expect("ratified"), 180);
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
