use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::shed::{ShedBudgetError, ShedBudgetTable, Surface, SurfaceBudget};

pub const THRESHOLDS_FILENAME: &str = "thresholds.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThresholdError {
    Io(String),
    Parse(String),
    Missing(String),
    OpenLegal(String),
}

impl std::fmt::Display for ThresholdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThresholdError::Io(e) => write!(f, "thresholds file unreadable: {e}"),
            ThresholdError::Parse(e) => write!(f, "thresholds file did not parse: {e}"),
            ThresholdError::Missing(k) => {
                write!(f, "missing threshold `{k}` - a missing threshold is a loud error, not a default")
            }
            ThresholdError::OpenLegal(k) => write!(
                f,
                "threshold `{k}` is [OPEN - LEGAL] (not yet DPO-ratified) - it carries no value to read"
            ),
        }
    }
}

impl std::error::Error for ThresholdError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    pub version: u32,
    pub as_of: String,
    pub revocation: Revocation,
    pub surge: Surge,
    #[serde(default)]
    pub authz_surge: AuthzSurge,
    pub fail_static: FailStaticThreshold,
    pub rpo_rto: RpoRto,
    #[serde(default)]
    pub online_migration: OnlineMigration,
    pub depth_ceilings: DepthCeilings,
    #[serde(default)]
    pub authz_index: AuthzIndex,
    #[serde(default)]
    pub shed_budgets: Vec<ShedBudgetRow>,
    #[serde(default)]
    pub dsr: DsrDeadline,
    #[serde(default)]
    pub refs_traverse: RefsTraverse,
    #[serde(default)]
    pub flex_db: FlexDb,
    #[serde(default)]
    pub refs_hot_artifact: RefsHotArtifact,
    #[serde(default)]
    pub cell_sizing: CellSizing,
    #[serde(default)]
    pub resilient_client: Vec<ResilientTargetRow>,
    #[serde(default)]
    pub column_store_seam: ColumnStoreSeam,
    #[serde(default)]
    pub search_freshness: SearchFreshness,
    #[serde(default)]
    pub filtered_ann: FilteredAnn,
    #[serde(default)]
    pub projection_feeder: ProjectionFeeder,
    #[serde(default)]
    pub timer_wheel_promotion: TimerWheelPromotion,
    #[serde(default)]
    pub ci_surge: CiSurge,
    #[serde(default)]
    pub ci_switch_test: CiSwitchTestThreshold,
    #[serde(default)]
    pub refs_switch_test: RefsSwitchTestThreshold,
    #[serde(default)]
    pub search_switch_test: SearchSwitchTestThreshold,
    #[serde(default)]
    pub git_switch_test: GitSwitchTestThreshold,
    #[serde(default)]
    pub knowledge_switch_test: KnowledgeSwitchTestThreshold,
    #[serde(default)]
    pub chat_switch_test: ChatSwitchTestThreshold,
    #[serde(default)]
    pub claimed_not_proven: Vec<ClaimedNotProven>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revocation {
    pub sla_mins: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surge {
    pub multiplier: u32,
    #[serde(default = "Surge::default_human_lane_p99_budget_us")]
    pub human_lane_p99_budget_us: u64,
}

impl Surge {
    pub fn default_human_lane_p99_budget_us() -> u64 {
        10_000
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthzSurge {
    pub human_lane_p99_budget_us: u64,
}

impl Default for AuthzSurge {
    fn default() -> Self {
        AuthzSurge {
            human_lane_p99_budget_us: 5_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailStaticThreshold {
    pub status: String,
    pub owner: String,
    #[serde(default)]
    pub static_max_secs: Option<u64>,
    pub static_max_default_secs: u64,
    pub agent_token_ttl_secs: u64,
    pub constraint: String,
}

impl FailStaticThreshold {
    pub fn ratified_static_max_secs(&self) -> Result<u64, ThresholdError> {
        self.static_max_secs.ok_or_else(|| {
            ThresholdError::OpenLegal(format!("fail_static.static_max_secs ({})", self.status))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpoRto {
    pub rpo_max_mins: u64,
    pub rto_tenant_max_mins: u64,
    pub rto_cell_max_mins: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnlineMigration {
    pub lock_wait_p99_max_ms: u64,
    pub downtime_max_ms: u64,
}

impl Default for OnlineMigration {
    fn default() -> Self {
        OnlineMigration {
            lock_wait_p99_max_ms: 500,
            downtime_max_ms: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnStoreSeam {
    pub promote_events_per_sec_per_stream: u64,
    pub degraded_publish_latency_p99_ms: u64,
    pub promotion_owed: bool,
}

impl Default for ColumnStoreSeam {
    fn default() -> Self {
        ColumnStoreSeam {
            promote_events_per_sec_per_stream: 50_000,
            degraded_publish_latency_p99_ms: 100,
            promotion_owed: false,
        }
    }
}

impl ColumnStoreSeam {
    pub fn promotion_owed_for(
        &self,
        measured_events_per_sec: u64,
        measured_publish_latency_p99_ms: u64,
    ) -> bool {
        measured_events_per_sec > self.promote_events_per_sec_per_stream
            && measured_publish_latency_p99_ms > self.degraded_publish_latency_p99_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerWheelPromotion {
    pub promote_due_now_per_sec_per_cell: u64,
    pub degraded_wheel_lag_budget: u64,
    pub promotion_owed: bool,
}

impl Default for TimerWheelPromotion {
    fn default() -> Self {
        TimerWheelPromotion {
            promote_due_now_per_sec_per_cell: 100_000,
            degraded_wheel_lag_budget: 0,
            promotion_owed: false,
        }
    }
}

impl TimerWheelPromotion {
    pub fn promotion_owed_for(
        &self,
        measured_due_now_per_sec: u64,
        measured_wheel_lag: u64,
    ) -> bool {
        measured_due_now_per_sec > self.promote_due_now_per_sec_per_cell
            && measured_wheel_lag > self.degraded_wheel_lag_budget
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiSurge {
    pub per_tenant_in_flight_cap: u32,
    pub drr_base_quantum: i64,
    pub drr_deficit_ceiling: i64,
    pub starvation_wait_p99_max_ticks: u64,
    pub hierarchical_scheduler_promotion_owed: bool,
    pub prewarm_buffer_per_arrival_rate_bps: u32,
    pub prewarm_max_buffer: u32,
}

impl CiSurge {
    pub const PER_TENANT_IN_FLIGHT_CAP_SEED: u32 = 64;
    pub const DRR_BASE_QUANTUM_SEED: i64 = 1;
    pub const DRR_DEFICIT_CEILING_SEED: i64 = 64;
    pub const STARVATION_WAIT_P99_MAX_TICKS_SEED: u64 = 32;
    pub const PREWARM_BUFFER_PER_ARRIVAL_RATE_BPS_SEED: u32 = 1000;
    pub const PREWARM_MAX_BUFFER_SEED: u32 = 16;

    pub fn hierarchical_promotion_owed_for(&self, measured_wait_p99_ticks: u64) -> bool {
        measured_wait_p99_ticks > self.starvation_wait_p99_max_ticks
    }

    pub fn prewarm_buffer_for(&self, arrival_rate: u32) -> u32 {
        let want =
            ((arrival_rate as u64) * (self.prewarm_buffer_per_arrival_rate_bps as u64)) / 10_000;
        (want as u32).min(self.prewarm_max_buffer)
    }

    pub fn is_well_formed(&self) -> bool {
        self.per_tenant_in_flight_cap > 0
            && self.drr_base_quantum > 0
            && self.drr_base_quantum < self.drr_deficit_ceiling
            && self.starvation_wait_p99_max_ticks > 0
            && self.prewarm_buffer_per_arrival_rate_bps > 0
            && self.prewarm_buffer_per_arrival_rate_bps <= 10_000
    }
}

impl Default for CiSurge {
    fn default() -> Self {
        CiSurge {
            per_tenant_in_flight_cap: Self::PER_TENANT_IN_FLIGHT_CAP_SEED,
            drr_base_quantum: Self::DRR_BASE_QUANTUM_SEED,
            drr_deficit_ceiling: Self::DRR_DEFICIT_CEILING_SEED,
            starvation_wait_p99_max_ticks: Self::STARVATION_WAIT_P99_MAX_TICKS_SEED,
            hierarchical_scheduler_promotion_owed: false,
            prewarm_buffer_per_arrival_rate_bps: Self::PREWARM_BUFFER_PER_ARRIVAL_RATE_BPS_SEED,
            prewarm_max_buffer: Self::PREWARM_MAX_BUFFER_SEED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiSwitchTestThreshold {
    #[serde(default = "CiSwitchTestThreshold::default_render_budget_us")]
    pub render_budget_us: u64,
}

impl CiSwitchTestThreshold {
    pub const RENDER_BUDGET_US_SEED: u64 = 50_000;

    pub fn default_render_budget_us() -> u64 {
        Self::RENDER_BUDGET_US_SEED
    }

    pub fn is_well_formed(&self) -> bool {
        self.render_budget_us > 0
    }
}

impl Default for CiSwitchTestThreshold {
    fn default() -> Self {
        CiSwitchTestThreshold {
            render_budget_us: Self::RENDER_BUDGET_US_SEED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefsSwitchTestThreshold {
    #[serde(default = "RefsSwitchTestThreshold::default_backlink_read_budget_us")]
    pub backlink_read_budget_us: u64,
    #[serde(default = "RefsSwitchTestThreshold::default_unfurl_budget_us")]
    pub unfurl_budget_us: u64,
    #[serde(default = "RefsSwitchTestThreshold::default_jump_no_spinner_budget_us")]
    pub jump_no_spinner_budget_us: u64,
}

impl RefsSwitchTestThreshold {
    pub const BACKLINK_READ_BUDGET_US_SEED: u64 = 20_000;
    pub const UNFURL_BUDGET_US_SEED: u64 = 16_000;
    pub const JUMP_NO_SPINNER_BUDGET_US_SEED: u64 = 100_000;

    pub fn default_backlink_read_budget_us() -> u64 {
        Self::BACKLINK_READ_BUDGET_US_SEED
    }

    pub fn default_unfurl_budget_us() -> u64 {
        Self::UNFURL_BUDGET_US_SEED
    }

    pub fn default_jump_no_spinner_budget_us() -> u64 {
        Self::JUMP_NO_SPINNER_BUDGET_US_SEED
    }

    pub fn is_well_formed(&self) -> bool {
        self.backlink_read_budget_us > 0
            && self.unfurl_budget_us > 0
            && self.jump_no_spinner_budget_us > 0
    }
}

impl Default for RefsSwitchTestThreshold {
    fn default() -> Self {
        RefsSwitchTestThreshold {
            backlink_read_budget_us: Self::BACKLINK_READ_BUDGET_US_SEED,
            unfurl_budget_us: Self::UNFURL_BUDGET_US_SEED,
            jump_no_spinner_budget_us: Self::JUMP_NO_SPINNER_BUDGET_US_SEED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchSwitchTestThreshold {
    #[serde(default = "SearchSwitchTestThreshold::default_code_by_symbol_budget_us")]
    pub code_by_symbol_budget_us: u64,
    #[serde(default = "SearchSwitchTestThreshold::default_doc_by_content_budget_us")]
    pub doc_by_content_budget_us: u64,
    #[serde(default = "SearchSwitchTestThreshold::default_issue_by_facet_budget_us")]
    pub issue_by_facet_budget_us: u64,
}

impl SearchSwitchTestThreshold {
    pub const CODE_BY_SYMBOL_BUDGET_US_SEED: u64 = 30_000;
    pub const DOC_BY_CONTENT_BUDGET_US_SEED: u64 = 40_000;
    pub const ISSUE_BY_FACET_BUDGET_US_SEED: u64 = 20_000;

    pub fn default_code_by_symbol_budget_us() -> u64 {
        Self::CODE_BY_SYMBOL_BUDGET_US_SEED
    }

    pub fn default_doc_by_content_budget_us() -> u64 {
        Self::DOC_BY_CONTENT_BUDGET_US_SEED
    }

    pub fn default_issue_by_facet_budget_us() -> u64 {
        Self::ISSUE_BY_FACET_BUDGET_US_SEED
    }

    pub fn is_well_formed(&self) -> bool {
        self.code_by_symbol_budget_us > 0
            && self.doc_by_content_budget_us > 0
            && self.issue_by_facet_budget_us > 0
    }
}

impl Default for SearchSwitchTestThreshold {
    fn default() -> Self {
        SearchSwitchTestThreshold {
            code_by_symbol_budget_us: Self::CODE_BY_SYMBOL_BUDGET_US_SEED,
            doc_by_content_budget_us: Self::DOC_BY_CONTENT_BUDGET_US_SEED,
            issue_by_facet_budget_us: Self::ISSUE_BY_FACET_BUDGET_US_SEED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSwitchTestThreshold {
    #[serde(default = "GitSwitchTestThreshold::default_pr_overview_render_budget_us")]
    pub pr_overview_render_budget_us: u64,
    #[serde(default = "GitSwitchTestThreshold::default_overlay_contrast_floor_bp")]
    pub overlay_contrast_floor_bp: u32,
}

impl GitSwitchTestThreshold {
    pub const PR_OVERVIEW_RENDER_BUDGET_US_SEED: u64 = 50_000;
    pub const OVERLAY_CONTRAST_FLOOR_BP_SEED: u32 = 450;

    pub fn default_pr_overview_render_budget_us() -> u64 {
        Self::PR_OVERVIEW_RENDER_BUDGET_US_SEED
    }

    pub fn default_overlay_contrast_floor_bp() -> u32 {
        Self::OVERLAY_CONTRAST_FLOOR_BP_SEED
    }

    pub fn is_well_formed(&self) -> bool {
        self.pr_overview_render_budget_us > 0 && self.overlay_contrast_floor_bp >= 450
    }
}

impl Default for GitSwitchTestThreshold {
    fn default() -> Self {
        GitSwitchTestThreshold {
            pr_overview_render_budget_us: Self::PR_OVERVIEW_RENDER_BUDGET_US_SEED,
            overlay_contrast_floor_bp: Self::OVERLAY_CONTRAST_FLOOR_BP_SEED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeSwitchTestThreshold {
    #[serde(default = "KnowledgeSwitchTestThreshold::default_page_render_budget_us")]
    pub page_render_budget_us: u64,
    #[serde(default = "KnowledgeSwitchTestThreshold::default_overlay_contrast_floor_bp")]
    pub overlay_contrast_floor_bp: u32,
}

impl KnowledgeSwitchTestThreshold {
    pub const PAGE_RENDER_BUDGET_US_SEED: u64 = 50_000;
    pub const OVERLAY_CONTRAST_FLOOR_BP_SEED: u32 = 450;

    pub fn default_page_render_budget_us() -> u64 {
        Self::PAGE_RENDER_BUDGET_US_SEED
    }

    pub fn default_overlay_contrast_floor_bp() -> u32 {
        Self::OVERLAY_CONTRAST_FLOOR_BP_SEED
    }

    pub fn is_well_formed(&self) -> bool {
        self.page_render_budget_us > 0 && self.overlay_contrast_floor_bp >= 450
    }
}

impl Default for KnowledgeSwitchTestThreshold {
    fn default() -> Self {
        KnowledgeSwitchTestThreshold {
            page_render_budget_us: Self::PAGE_RENDER_BUDGET_US_SEED,
            overlay_contrast_floor_bp: Self::OVERLAY_CONTRAST_FLOOR_BP_SEED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSwitchTestThreshold {
    #[serde(default = "ChatSwitchTestThreshold::default_perceived_send_budget_us")]
    pub perceived_send_budget_us: u64,
    #[serde(default = "ChatSwitchTestThreshold::default_overlay_contrast_floor_bp")]
    pub overlay_contrast_floor_bp: u32,
}

impl ChatSwitchTestThreshold {
    pub const PERCEIVED_SEND_BUDGET_US_SEED: u64 = 100_000;
    pub const OVERLAY_CONTRAST_FLOOR_BP_SEED: u32 = 450;

    pub fn default_perceived_send_budget_us() -> u64 {
        Self::PERCEIVED_SEND_BUDGET_US_SEED
    }

    pub fn default_overlay_contrast_floor_bp() -> u32 {
        Self::OVERLAY_CONTRAST_FLOOR_BP_SEED
    }

    pub fn is_well_formed(&self) -> bool {
        self.perceived_send_budget_us > 0 && self.overlay_contrast_floor_bp >= 450
    }
}

impl Default for ChatSwitchTestThreshold {
    fn default() -> Self {
        ChatSwitchTestThreshold {
            perceived_send_budget_us: Self::PERCEIVED_SEND_BUDGET_US_SEED,
            overlay_contrast_floor_bp: Self::OVERLAY_CONTRAST_FLOOR_BP_SEED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepthCeilings {
    pub soft: u32,
    pub hard: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthzIndex {
    pub ids_cardinality_cap: usize,
    pub reverse_index_lag_slo_ms: u64,
}

impl Default for AuthzIndex {
    fn default() -> Self {
        AuthzIndex {
            ids_cardinality_cap: 1000,
            reverse_index_lag_slo_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DsrDeadline {
    pub deadline_secs: u64,
    pub warning_margin_secs: u64,
    pub extension_total_secs: u64,
}

impl Default for DsrDeadline {
    fn default() -> Self {
        DsrDeadline {
            deadline_secs: 30 * 24 * 60 * 60,
            warning_margin_secs: 7 * 24 * 60 * 60,
            extension_total_secs: 90 * 24 * 60 * 60,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefsTraverse {
    pub depth_ceiling: u32,
    pub max_nodes: u32,
}

impl Default for RefsTraverse {
    fn default() -> Self {
        RefsTraverse {
            depth_ceiling: 16,
            max_nodes: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefsHotArtifact {
    pub read_budget_fanout: u64,
}

impl Default for RefsHotArtifact {
    fn default() -> Self {
        RefsHotArtifact {
            read_budget_fanout: 1000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFreshness {
    pub freshness_p99_ms: u64,
    pub index_lag_alarm_margin_ms: u64,
}

impl SearchFreshness {
    pub const FRESHNESS_P99_SEED_MS: u64 = 2000;
    pub const ALARM_MARGIN_SEED_MS: u64 = 500;

    pub fn alarm_threshold_ms(&self) -> u64 {
        self.freshness_p99_ms
            .saturating_sub(self.index_lag_alarm_margin_ms)
    }

    pub fn alarm_fires_before_staleness(&self) -> bool {
        self.index_lag_alarm_margin_ms < self.freshness_p99_ms
    }
}

impl Default for SearchFreshness {
    fn default() -> Self {
        SearchFreshness {
            freshness_p99_ms: Self::FRESHNESS_P99_SEED_MS,
            index_lag_alarm_margin_ms: Self::ALARM_MARGIN_SEED_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilteredAnn {
    pub recall_at_k_bps: u32,
    pub brute_force_fallback_visible_bps: u32,
    pub ivf_pq_promotion_live_vectors: u64,
}

impl FilteredAnn {
    pub const RECALL_AT_K_BPS_SEED: u32 = 10_000;
    pub const BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED: u32 = 2_000;
    pub const IVF_PQ_PROMOTION_LIVE_VECTORS_SEED: u64 = 1_000_000;

    pub fn recall_floor_fraction(&self) -> f64 {
        self.recall_at_k_bps as f64 / 10_000.0
    }

    pub fn is_very_selective(&self, visible: u64, total: u64) -> bool {
        if total == 0 {
            return false;
        }
        (visible as u128) * 10_000
            <= (self.brute_force_fallback_visible_bps as u128) * (total as u128)
    }

    pub fn should_promote_to_ivf_pq(&self, live_vectors: u64) -> bool {
        live_vectors >= self.ivf_pq_promotion_live_vectors
    }

    pub fn is_well_formed(&self) -> bool {
        self.recall_at_k_bps > 0
            && self.brute_force_fallback_visible_bps > 0
            && self.brute_force_fallback_visible_bps <= 10_000
            && self.ivf_pq_promotion_live_vectors > 0
    }
}

impl Default for FilteredAnn {
    fn default() -> Self {
        FilteredAnn {
            recall_at_k_bps: Self::RECALL_AT_K_BPS_SEED,
            brute_force_fallback_visible_bps: Self::BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED,
            ivf_pq_promotion_live_vectors: Self::IVF_PQ_PROMOTION_LIVE_VECTORS_SEED,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProjectionFeeder {
    pub promotion_ratio: f64,
    pub min_executions: u64,
}

impl ProjectionFeeder {
    pub const PROMOTION_RATIO_SEED: f64 = 0.05;
    pub const MIN_EXECUTIONS_SEED: u64 = 20;

    pub fn should_promote(&self, uses: u64, total: u64) -> bool {
        if total == 0 || total < self.min_executions {
            return false;
        }
        (uses as f64) > self.promotion_ratio * (total as f64)
    }

    pub fn is_well_formed(&self) -> bool {
        self.promotion_ratio > 0.0 && self.promotion_ratio < 1.0 && self.min_executions > 0
    }
}

impl Default for ProjectionFeeder {
    fn default() -> Self {
        ProjectionFeeder {
            promotion_ratio: Self::PROMOTION_RATIO_SEED,
            min_executions: Self::MIN_EXECUTIONS_SEED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlexDb {
    pub view_read_p99_max_ms: u64,
    pub page_row_cap: u32,
    pub facet_promotion_ratio: f64,
    pub rollup_read_p99_max_ms: u64,
}

impl Default for FlexDb {
    fn default() -> Self {
        FlexDb {
            view_read_p99_max_ms: 200,
            page_row_cap: 500,
            facet_promotion_ratio: 0.05,
            rollup_read_p99_max_ms: 250,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellSizing {
    pub pool_tenants_max: u32,
    pub pool_write_qps_max: u32,
    pub pool_storage_bytes_max: u64,
    pub pool_binding_dimension: String,
    pub pool_hot_headroom_bps: u32,
}

impl Default for CellSizing {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShedBudgetRow {
    pub surface: String,
    pub per_tenant_in_flight_cap: u32,
    pub human_lane_reservation: u32,
    pub retry_after_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResilientTargetRow {
    pub target: String,
    pub hot_path: bool,
    pub latency_budget_ms: u64,
    pub timeout_ms: u64,
    pub backoff_base_ms: u64,
    pub max_attempts: u32,
    pub breaker_failure_ratio: f64,
    pub breaker_min_requests: u32,
    pub breaker_window: u32,
    pub breaker_open_ms: u64,
    pub bulkhead_max_concurrency: u32,
}

impl ResilientTargetRow {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResilientTuningError {
    TimeoutLooserThanBudget {
        target: String,
        timeout_ms: u64,
        latency_budget_ms: u64,
    },
    DegenerateValue {
        target: String,
        field: String,
    },
    HotPathNotTighter {
        tightest_hot_path_ms: u64,
        batch_target: String,
        batch_timeout_ms: u64,
    },
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
                 measured latency budget {latency_budget_ms}ms - a value tuned looser than the \
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
                 is not looser than the auth hot path's {tightest_hot_path_ms}ms - the auth hot path \
                 MUST be strictly tighter than every batch indexer (§6.3)"
            ),
            ResilientTuningError::NoHotPathTarget => write!(
                f,
                "resilient-client tuning: no hot-path target declared - the tuned set must name the \
                 auth hot path so the tighter-than-batch relation holds"
            ),
        }
    }
}

impl std::error::Error for ResilientTuningError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedNotProven {
    pub gate: String,
    pub threshold_key: String,
    pub date: String,
    pub owner: String,
    pub note: String,
}

impl Thresholds {
    pub fn from_toml(s: &str) -> Result<Thresholds, ThresholdError> {
        toml::from_str(s).map_err(|e| ThresholdError::Parse(e.to_string()))
    }

    pub fn to_toml(&self) -> Result<String, ThresholdError> {
        toml::to_string(self).map_err(|e| ThresholdError::Parse(e.to_string()))
    }

    pub fn load(path: &std::path::Path) -> Result<Thresholds, ThresholdError> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| ThresholdError::Io(format!("{}: {e}", path.display())))?;
        Thresholds::from_toml(&s)
    }

    pub fn load_canonical() -> Result<Thresholds, ThresholdError> {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = manifest
            .parent()
            .and_then(|p| p.parent())
            .ok_or_else(|| ThresholdError::Io("could not resolve the workspace root".into()))?;
        Thresholds::load(&root.join(THRESHOLDS_FILENAME))
    }

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

    pub fn shed_budget(&self, surface: Surface) -> Result<SurfaceBudget, ThresholdError> {
        self.shed_budget_table()?
            .remove(&surface)
            .ok_or_else(|| ThresholdError::Missing(format!("shed_budgets.{surface:?}")))
    }

    pub fn validate_shed_budgets(&self) -> Result<(), ShedBudgetError> {
        let table_map = self
            .shed_budget_table()
            .map_err(|_| ShedBudgetError::Unbounded(Surface::HttpIntake))?;
        for (surface, budget) in &table_map {
            budget.validate_tuned(*surface)?;
        }
        Ok(())
    }

    pub fn shed_budget_table_validated(&self) -> Result<ShedBudgetTable, ThresholdError> {
        self.validate_shed_budgets()
            .map_err(|e| ThresholdError::Parse(e.to_string()))?;
        let map = self.shed_budget_table()?;
        Ok(ShedBudgetTable::from_rows(map))
    }

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

    pub fn validate_resilient_targets(&self) -> Result<(), ResilientTuningError> {
        if self.resilient_client.is_empty() {
            return Ok(());
        }
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
            if row.timeout_ms > row.latency_budget_ms {
                return Err(ResilientTuningError::TimeoutLooserThanBudget {
                    target: row.target.clone(),
                    timeout_ms: row.timeout_ms,
                    latency_budget_ms: row.latency_budget_ms,
                });
            }
        }
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
        "SearchQuery" => Surface::SearchQuery,
        "WorkflowAgentLane" => Surface::WorkflowAgentLane,
        other => {
            return Err(ThresholdError::Parse(format!(
                "unknown shed-budget surface `{other}` (not a shed::Surface variant)"
            )))
        }
    };
    match s {
        Surface::HttpIntake
        | Surface::CiDispatch
        | Surface::CollabOpStream
        | Surface::ConnectionTier
        | Surface::AgentMention
        | Surface::GitFrontDoor
        | Surface::RefsBacklinkRead
        | Surface::RefsRefCreate
        | Surface::SearchQuery
        | Surface::WorkflowAgentLane => Ok(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_load::DepthCeiling;

    #[test]
    fn sample_drill_reads_depth_ceiling_from_the_canonical_file() {
        let t = Thresholds::load_canonical().expect("the canonical thresholds file must load");
        assert_eq!(t.depth_ceilings.soft, DepthCeiling::V1_SOFT);
        assert_eq!(t.depth_ceilings.hard, DepthCeiling::V1_HARD);
        let from_file = DepthCeiling::new(t.depth_ceilings.soft, t.depth_ceilings.hard);
        let v1 = DepthCeiling::v1_floor();
        assert_eq!(from_file.soft(), v1.soft());
        assert_eq!(from_file.hard(), v1.hard());
    }

    #[test]
    fn canonical_file_holds_the_measured_search_freshness_budget() {
        let t = Thresholds::load_canonical().expect("load");
        assert_eq!(
            t.search_freshness.freshness_p99_ms,
            SearchFreshness::FRESHNESS_P99_SEED_MS,
            "the §4.10 seconds-grade freshness p99 budget (held under the 30× surge with ~100× headroom)"
        );
        assert_eq!(
            t.search_freshness.index_lag_alarm_margin_ms,
            SearchFreshness::ALARM_MARGIN_SEED_MS,
            "the index-lag alarm margin"
        );
        assert!(
            t.search_freshness.alarm_fires_before_staleness(),
            "the alarm margin must sit strictly below the budget (the alarm fires FIRST)"
        );
        assert_eq!(
            t.search_freshness.alarm_threshold_ms(),
            1500,
            "the alarm fires at budget − margin = 2000 − 500 = 1500 ms"
        );
    }

    #[test]
    fn canonical_file_holds_the_tuned_filtered_ann_strategy() {
        let t = Thresholds::load_canonical().expect("load");
        let f = &t.filtered_ann;
        assert_eq!(
            f.recall_at_k_bps,
            FilteredAnn::RECALL_AT_K_BPS_SEED,
            "the §4.2.2 filtered-ANN recall floor: exact recall (100.00 %) under a selective filter"
        );
        assert_eq!(
            f.brute_force_fallback_visible_bps,
            FilteredAnn::BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED,
            "the brute-force-fallback very-selective trigger (≤ 20 % visible)"
        );
        assert_eq!(
            f.ivf_pq_promotion_live_vectors,
            FilteredAnn::IVF_PQ_PROMOTION_LIVE_VECTORS_SEED,
            "the §3.3 HNSW→IVF-PQ promotion point (per-cell live vectors)"
        );
        assert!(
            f.is_well_formed(),
            "the strategy numbers must be well-formed (a 0 recall floor / 0 promotion point is rejected)"
        );
        assert_eq!(
            f.recall_floor_fraction(),
            1.0,
            "the recall floor is exact (1.0) - no visible nearest neighbour is ever dropped"
        );
        assert!(f.is_very_selective(5, 100), "5 % visible is very selective");
        assert!(
            !f.is_very_selective(50, 100),
            "50 % visible is not selective"
        );
        assert!(f.should_promote_to_ivf_pq(1_000_000));
        assert!(!f.should_promote_to_ivf_pq(999_999));
    }

    #[test]
    fn canonical_file_holds_the_projection_feeder_threshold() {
        let t = Thresholds::load_canonical().expect("load");
        let p = &t.projection_feeder;
        assert_eq!(
            p.promotion_ratio,
            ProjectionFeeder::PROMOTION_RATIO_SEED,
            "the §4.6.1 / OQ-C frozen > 5 % promotion ratio (Search consumes the Issues/KN signal)"
        );
        assert_eq!(
            p.min_executions,
            ProjectionFeeder::MIN_EXECUTIONS_SEED,
            "the rolling-window execution floor (too-few executions is too noisy to promote on)"
        );
        assert!(
            p.is_well_formed(),
            "the threshold must be well-formed (a 0 / ≥ 1 ratio or 0 floor is rejected)"
        );
        assert!(p.should_promote(6, 100), "6 % > 5 % promotes");
        assert!(
            !p.should_promote(5, 100),
            "exactly 5 % does NOT promote (strict >)"
        );
        assert!(
            !p.should_promote(1, 1),
            "below the execution floor never promotes"
        );
    }

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
        assert_eq!(t.shed_budgets.len(), 10, "one row per shed::Surface");
        assert_eq!(
            t.resilient_client.len(),
            4,
            "the measured resilient-client per-target tuned rows (P-S36): authz/event-bus + 2 batch"
        );
    }

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
            Surface::WorkflowAgentLane,
        ] {
            assert_eq!(
                t.shed_budget(surface).expect("present"),
                v1.budget(surface),
                "shed budget for {surface:?} must match the v1 floor table"
            );
        }
    }

    #[test]
    fn the_canonical_tuned_shed_budgets_validate() {
        let t = Thresholds::load_canonical().expect("load");
        t.validate_shed_budgets()
            .expect("the tuned shed budgets in the canonical file must validate (P-S33)");
        t.shed_budget_table_validated()
            .expect("the validated tuned table builds from the file");
    }

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
            status = "OPEN - LEGAL"
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

    #[test]
    fn thresholds_file_round_trips() {
        let t = Thresholds::load_canonical().expect("load");
        let serialized = t.to_toml().expect("serialize");
        let reparsed = Thresholds::from_toml(&serialized).expect("re-parse");
        assert_eq!(t, reparsed, "parse → serialize → parse must be identity");
    }

    #[test]
    fn a_missing_required_threshold_is_a_loud_error() {
        let missing_surge = r#"
            version = 1
            as_of = "2026-06-19"
            [revocation]
            sla_mins = 5
            [fail_static]
            status = "OPEN - LEGAL"
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

        let no_budgets = r#"
            version = 1
            as_of = "2026-06-19"
            [revocation]
            sla_mins = 5
            [surge]
            multiplier = 30
            [fail_static]
            status = "OPEN - LEGAL"
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

    #[test]
    fn open_legal_w_carries_its_constraint_and_is_loud_to_read() {
        let t = Thresholds::load_canonical().expect("load");
        assert!(t.fail_static.constraint.contains("revocation-SLA"));
        assert!(t.fail_static.constraint.contains("agent-token-TTL"));
        assert_eq!(t.fail_static.status, "OPEN - LEGAL");
        let err = t
            .fail_static
            .ratified_static_max_secs()
            .expect_err("W is [OPEN - LEGAL]");
        assert!(matches!(err, ThresholdError::OpenLegal(_)), "got {err:?}");
        assert_eq!(t.fail_static.static_max_default_secs, 300);
        assert!(t.fail_static.static_max_default_secs <= t.revocation.sla_mins * 60);

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

    #[test]
    fn online_migration_budget_reads_through_the_typed_loader() {
        let t = Thresholds::load_canonical().expect("load");
        assert_eq!(t.online_migration.lock_wait_p99_max_ms, 500);
        assert_eq!(t.online_migration.downtime_max_ms, 0);
    }

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
            status = "OPEN - LEGAL"
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
            status = "OPEN - LEGAL"
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
            status = "OPEN - LEGAL"
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

    #[test]
    fn a_timeout_looser_than_the_measured_budget_fails_the_gate() {
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

    #[test]
    fn a_timeout_within_the_measured_budget_validates() {
        let mut rows = resilient_row("identity-authz", true, 150, 120);
        rows.push_str(&resilient_row("search-index", false, 30000, 25000));
        let t = thresholds_with_resilient(&rows);
        t.validate_resilient_targets()
            .expect("a hot-path timeout within budget + a looser batch target must validate");
    }

    #[test]
    fn the_auth_hot_path_must_be_tighter_than_the_batch_indexer() {
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

    #[test]
    fn a_tuned_set_with_no_hot_path_target_fails() {
        let rows = resilient_row("search-index", false, 30000, 25000);
        let t = thresholds_with_resilient(&rows);
        assert_eq!(
            t.validate_resilient_targets(),
            Err(ResilientTuningError::NoHotPathTarget),
            "a non-empty tuned set must name the auth hot path (P-S36)"
        );
    }

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

    #[test]
    fn an_empty_tuned_set_is_vacuously_valid() {
        let t = thresholds_with_resilient("");
        assert!(t.resilient_client.is_empty());
        t.validate_resilient_targets()
            .expect("an empty tuned set is vacuously valid (the M0 floor applies)");
    }

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

    #[test]
    fn an_unknown_resilient_target_is_a_loud_missing_error() {
        let t = Thresholds::load_canonical().expect("load");
        assert!(matches!(
            t.resilient_config("no-such-target"),
            Err(ThresholdError::Missing(_))
        ));
    }

    #[test]
    fn scorecard_is_honest_and_empty_at_this_commit() {
        let t = Thresholds::load_canonical().expect("load");
        assert!(
            t.claimed_not_proven.is_empty(),
            "every shipped M0 drill is green at its threshold; a red one would add a row"
        );
    }

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

    #[test]
    fn column_store_promotion_owed_only_when_both_criteria_cross() {
        let seam = ColumnStoreSeam::default();
        assert!(!seam.promotion_owed_for(10_000, 500));
        assert!(!seam.promotion_owed_for(80_000, 50));
        assert!(!seam.promotion_owed_for(40_000, 500));
        assert!(seam.promotion_owed_for(80_000, 200));
        assert!(!seam.promotion_owed_for(50_000, 200));
        assert!(!seam.promotion_owed_for(80_000, 100));
    }

    #[test]
    fn timer_wheel_promotion_is_named_not_built_at_this_commit() {
        let t = Thresholds::load_canonical().expect("load");
        assert!(
            !t.timer_wheel_promotion.promotion_owed,
            "FLOW-D3 full: the 1M+ run drains within budget → the PG-indexed wheel suffices (§7.3)"
        );
        assert!(
            t.timer_wheel_promotion.promote_due_now_per_sec_per_cell > 0,
            "OQ #5: the per-cell due-now-rate promotion criterion is recorded (compared against, not beaten)"
        );
    }

    #[test]
    fn timer_wheel_promotion_owed_only_when_both_criteria_cross() {
        let seam = TimerWheelPromotion::default();
        assert!(!seam.promotion_owed_for(10_000, 5_000));
        assert!(!seam.promotion_owed_for(250_000, 0));
        assert!(!seam.promotion_owed_for(40_000, 5_000));
        assert!(seam.promotion_owed_for(250_000, 5_000));
        assert!(!seam.promotion_owed_for(100_000, 5_000));
    }

    #[test]
    fn ci_surge_controls_are_recorded_and_well_formed() {
        let t = Thresholds::load_canonical().expect("load");
        assert!(
            t.ci_surge.is_well_formed(),
            "the CI-surge numbers are well-formed (no vacuous bar)"
        );
        let ci_cap = t
            .shed_budget(crate::shed::Surface::CiDispatch)
            .expect("CiDispatch shed budget present")
            .per_tenant_in_flight_cap;
        assert_eq!(
            t.ci_surge.per_tenant_in_flight_cap, ci_cap,
            "the tuned CI in-flight cap MUST equal the CiDispatch shed-budget cap (one v1 floor)"
        );
        assert!(
            !t.ci_surge.hierarchical_scheduler_promotion_owed,
            "CI-D2: the 30× surge measured the wait p99 within budget → flat DRR holds; the \
             hierarchical scheduler stays a named floor (CI-P29)"
        );
    }

    #[test]
    fn ci_surge_hierarchical_promotion_owed_only_when_starvation_trigger_crossed() {
        let ci = CiSurge::default();
        assert!(
            !ci.hierarchical_promotion_owed_for(5),
            "a short wait is fairly served - no promotion"
        );
        assert!(
            !ci.hierarchical_promotion_owed_for(32),
            "exactly at the trigger does NOT cross (strict `>` - within budget)"
        );
        assert!(
            ci.hierarchical_promotion_owed_for(33),
            "a wait p99 over the trigger is the starvation signal → the hierarchy is owed (CI-P29)"
        );
    }

    #[test]
    fn ci_surge_prewarm_buffer_is_proportional_then_clamped() {
        let ci = CiSurge::default();
        assert_eq!(
            ci.prewarm_buffer_for(0),
            0,
            "an idle pool pre-warms nothing"
        );
        assert_eq!(
            ci.prewarm_buffer_for(50),
            5,
            "10% of 50 arrivals = 5 warm VMs"
        );
        assert_eq!(ci.prewarm_buffer_for(100), 10, "10% of 100 = 10 warm VMs");
        assert_eq!(
            ci.prewarm_buffer_for(100_000),
            16,
            "the warm buffer is CLAMPED at the per-VM-memory ceiling (never unbounded)"
        );
    }
}
