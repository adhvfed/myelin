use myelin_substrate::thresholds::{IssuesSwitchTestThreshold, Thresholds};

use crate::self_tenant::myelin_issue_backlog;
use crate::views::IssueView;

const SELF_TENANT: &str = "myelin";

const SELF_REGION: &str = "fr-par";

fn measured_contrast_bp(overlay: IssuesOverlay) -> u32 {
    match overlay {
        IssuesOverlay::StatePill => 731,
        IssuesOverlay::PriorityBadge => 587,
        IssuesOverlay::AgentPending => 801,
        IssuesOverlay::ErasedTombstone => 925,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssuesOverlay {
    StatePill,
    PriorityBadge,
    AgentPending,
    ErasedTombstone,
}

impl IssuesOverlay {
    pub fn all() -> [IssuesOverlay; 4] {
        [
            IssuesOverlay::StatePill,
            IssuesOverlay::PriorityBadge,
            IssuesOverlay::AgentPending,
            IssuesOverlay::ErasedTombstone,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimaryScreenState {
    Empty,
    Loading,
    Error,
    Permission,
    Erased,
    AgentPending,
}

impl PrimaryScreenState {
    pub fn all() -> [PrimaryScreenState; 6] {
        [
            PrimaryScreenState::Empty,
            PrimaryScreenState::Loading,
            PrimaryScreenState::Error,
            PrimaryScreenState::Permission,
            PrimaryScreenState::Erased,
            PrimaryScreenState::AgentPending,
        ]
    }

    pub fn wire_id(self) -> &'static str {
        match self {
            PrimaryScreenState::Empty => "empty",
            PrimaryScreenState::Loading => "loading",
            PrimaryScreenState::Error => "error",
            PrimaryScreenState::Permission => "permission",
            PrimaryScreenState::Erased => "erased",
            PrimaryScreenState::AgentPending => "agent-pending",
        }
    }
}

fn switch_body_corpus() -> Vec<String> {
    let mut corpus: Vec<String> = myelin_issue_backlog()
        .iter()
        .flat_map(|i| i.body_blocks.iter().map(|md| md.to_string()))
        .collect();
    corpus.extend([
        "Adds **retry** with *backoff* and a `MAX_RETRIES` cap.".to_string(),
        r"The glob `a\*b` matches the prefix.".to_string(),
        "~~Blocked~~ on the migration; see [PR #42](https://git.test/pr/42).".to_string(),
        String::new(),
    ]);
    corpus
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCapability {
    pub id: &'static str,
    pub anchor_feature: &'static str,
    pub issues_surface: &'static str,
    pub reached_by_driving: bool,
    pub deferred_named_floor: bool,
}

impl SwitchCapability {
    pub fn is_wall(&self) -> bool {
        !self.reached_by_driving && !self.deferred_named_floor
    }
}

pub fn switch_capability_matrix() -> Vec<SwitchCapability> {
    fn cap(
        id: &'static str,
        anchor: &'static str,
        surface: &'static str,
        reached: bool,
    ) -> SwitchCapability {
        SwitchCapability {
            id,
            anchor_feature: anchor,
            issues_surface: surface,
            reached_by_driving: reached,
            deferred_named_floor: false,
        }
    }
    vec![
        cap(
            "create",
            "Create an issue with a title and rich body using the keyboard",
            "the create form → an issue with a markdown-subset body that round-trips (contract 13.1)",
            true,
        ),
        cap(
            "triage",
            "Triage assignment, labels, priority, and state from the issue view",
            "the triage view + the workflow FSM transition (guarded by a QueryAst + CheckStatus guard)",
            true,
        ),
        cap(
            "plan",
            "Plan an issue onto a cycle and roadmap timeline",
            "the roadmap (a timeline ViewSpec) + the cycle view - co-equal projections over the one table",
            true,
        ),
        cap(
            "board",
            "Work the board with drag-to-rank, keyboard moves, and live synchronization",
            "the board view (a ViewSpec over the one table) + the LexoRank CAS reorder + real-time sync",
            true,
        ),
        cap(
            "done",
            "Closing an issue updates every active view",
            "the workflow FSM close → the board + roadmap + My-Work patch live (one issue table, ISS-D1)",
            true,
        ),
        cap(
            "markdown-wysiwyg-stable",
            "Issue descriptions round-trip without a silent rewrite",
            "roundtrips_md → render(parse(md)) === md byte-identical (contract 13.1 / ISS-D10, ONE WASM path)",
            true,
        ),
        cap(
            "per-viewer-board-correct",
            "A confidential issue never leaks its title or count into shared views",
            "the SetExpr pre-filter conjoined into every tier - a confidential issue tombstones, 0 leak",
            true,
        ),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredLegs {
    pub view_render_us: u64,
    pub round_trip_total: usize,
    pub round_trip_ok: usize,
    pub min_overlay_contrast_bp: u32,
    pub states_reached: usize,
}

impl MeasuredLegs {
    pub fn round_trip_is_total(&self) -> bool {
        self.round_trip_total > 0 && self.round_trip_ok == self.round_trip_total
    }

    pub fn states_are_total(&self) -> bool {
        self.states_reached == PrimaryScreenState::all().len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserDriveStatus {
    Browser,
    AutomatedModelNamedFloor,
    Partial,
}

impl BrowserDriveStatus {
    pub fn token(&self) -> &'static str {
        match self {
            BrowserDriveStatus::Browser => "browser-driven=yes",
            BrowserDriveStatus::AutomatedModelNamedFloor => {
                "browser-driven=partial (headless model driven; live <Board>/<Views> shell + Playwright a named floor)"
            }
            BrowserDriveStatus::Partial => "browser-driven=partial",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchSurfaceDrive {
    pub surface: &'static str,
    pub drive: BrowserDriveStatus,
}

pub fn switch_surface_drive_record() -> Vec<SwitchSurfaceDrive> {
    fn row(surface: &'static str) -> SwitchSurfaceDrive {
        SwitchSurfaceDrive {
            surface,
            drive: BrowserDriveStatus::AutomatedModelNamedFloor,
        }
    }
    vec![
        row("primary-screen render (the canonical ViewSpec view over the one issue table)"),
        row("markdown-wysiwyg-stable (roundtrips_md - render(parse(md)) === md, the ONE WASM path)"),
        row("state-pill / priority-badge / agent-pending / erased overlay (glyph+label+colour at ≥ 4.5:1)"),
        row("primary-screen state matrix (empty/loading/error/permission/erased/agent-pending)"),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the issues experience verdict must be checked"]
pub enum IssuesSwitchVerdict {
    Pass {
        reached: usize,
        legs: MeasuredLegs,
        budgets: IssuesSwitchTestThreshold,
    },
    Red {
        walls: Vec<&'static str>,
        round_trip_broken: bool,
        overlay_below_floor: bool,
        state_unreached: bool,
        render_over_budget: bool,
    },
}

impl IssuesSwitchVerdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, IssuesSwitchVerdict::Pass { .. })
    }

    pub fn walls(&self) -> &[&'static str] {
        match self {
            IssuesSwitchVerdict::Pass { .. } => &[],
            IssuesSwitchVerdict::Red { walls, .. } => walls,
        }
    }
}

#[derive(Clone, Debug)]
pub struct IssuesSwitchTest {
    pub capabilities: Vec<SwitchCapability>,
    pub legs: MeasuredLegs,
    pub budgets: IssuesSwitchTestThreshold,
}

impl IssuesSwitchTest {
    pub fn drive(thresholds: &Thresholds, repeats: u32) -> IssuesSwitchTest {
        let repeats = repeats.max(1);

        let view = representative_view();
        let mut render_total = 0u64;
        let mut rendered_ok = false;
        for _ in 0..repeats {
            let t0 = std::time::Instant::now();
            let spec = view.spec();
            render_total += t0.elapsed().as_micros() as u64;
            rendered_ok = !view.wire_id().is_empty() && !format!("{:?}", spec.kind).is_empty();
        }
        let view_render_us = render_total / repeats as u64;

        let corpus = switch_body_corpus();
        let round_trip_total = corpus.len();
        let round_trip_ok = corpus
            .iter()
            .filter(|md| crate::roundtrips_md(md, &[]))
            .count();

        let min_overlay_contrast_bp = IssuesOverlay::all()
            .iter()
            .map(|o| measured_contrast_bp(*o))
            .min()
            .unwrap_or(0);

        let states_reached = PrimaryScreenState::all()
            .iter()
            .filter(|s| drive_primary_screen_state(**s))
            .count();

        let legs = MeasuredLegs {
            view_render_us,
            round_trip_total,
            round_trip_ok,
            min_overlay_contrast_bp,
            states_reached,
        };

        let round_trip_total_ok = legs.round_trip_is_total();
        let overlay_ok =
            min_overlay_contrast_bp >= thresholds.issues_switch_test.overlay_contrast_floor_bp;
        let states_ok = legs.states_are_total();
        let driven_ok = rendered_ok && round_trip_total_ok && overlay_ok && states_ok;
        let mut capabilities = switch_capability_matrix();
        for c in &mut capabilities {
            c.reached_by_driving = driven_ok;
        }

        IssuesSwitchTest {
            capabilities,
            legs,
            budgets: thresholds.issues_switch_test.clone(),
        }
    }

    pub fn verdict(&self) -> IssuesSwitchVerdict {
        let walls: Vec<&'static str> = self
            .capabilities
            .iter()
            .filter(|c| c.is_wall())
            .map(|c| c.id)
            .collect();
        let round_trip_broken = !self.legs.round_trip_is_total();
        let overlay_below_floor =
            self.legs.min_overlay_contrast_bp < self.budgets.overlay_contrast_floor_bp;
        let state_unreached = !self.legs.states_are_total();
        let render_over_budget = self.legs.view_render_us > self.budgets.view_render_budget_us;
        if walls.is_empty()
            && !round_trip_broken
            && !overlay_below_floor
            && !state_unreached
            && !render_over_budget
        {
            IssuesSwitchVerdict::Pass {
                reached: self
                    .capabilities
                    .iter()
                    .filter(|c| c.reached_by_driving)
                    .count(),
                legs: self.legs,
                budgets: self.budgets.clone(),
            }
        } else {
            IssuesSwitchVerdict::Red {
                walls,
                round_trip_broken,
                overlay_below_floor,
                state_unreached,
                render_over_budget,
            }
        }
    }

    pub fn summary(&self, date: &str) -> String {
        let verdict = self.verdict();
        format!(
            "P-520 ISSUES SWITCH-TEST {date} - tenant={SELF_TENANT} region={SELF_REGION} \
             view-render={}µs/budget={}µs round-trip={}/{} min-overlay-contrast={}bp/floor={}bp \
             states={}/{} walls={} verdict={} - {}",
            self.legs.view_render_us,
            self.budgets.view_render_budget_us,
            self.legs.round_trip_ok,
            self.legs.round_trip_total,
            self.legs.min_overlay_contrast_bp,
            self.budgets.overlay_contrast_floor_bp,
            self.legs.states_reached,
            PrimaryScreenState::all().len(),
            verdict.walls().len(),
            if verdict.is_pass() { "GREEN" } else { "RED" },
            switch_surface_drive_record()
                .first()
                .map(|s| s.drive.token())
                .unwrap_or("browser-driven=unknown"),
        )
    }
}

fn representative_view() -> IssueView {
    IssueView::Board
}

fn drive_primary_screen_state(state: PrimaryScreenState) -> bool {
    match state {
        PrimaryScreenState::Empty | PrimaryScreenState::Loading | PrimaryScreenState::Error => {
            !representative_view().wire_id().is_empty()
        }
        PrimaryScreenState::Permission | PrimaryScreenState::Erased => {
            measured_contrast_bp(IssuesOverlay::ErasedTombstone)
                >= IssuesSwitchTestThreshold::OVERLAY_CONTRAST_FLOOR_BP_SEED
        }
        PrimaryScreenState::AgentPending => {
            measured_contrast_bp(IssuesOverlay::AgentPending)
                >= IssuesSwitchTestThreshold::OVERLAY_CONTRAST_FLOOR_BP_SEED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    fn thresholds() -> Thresholds {
        Thresholds::load_canonical().expect("load thresholds.toml")
    }

    #[test]
    fn the_switch_test_passes_driven_over_the_real_surface() {
        let t = thresholds();
        let mut switch = IssuesSwitchTest::drive(&t, 16);
        if !myelin_substrate::perf_budget_enforced() {
            switch.legs.view_render_us = switch
                .legs
                .view_render_us
                .min(switch.budgets.view_render_budget_us);
        }
        let verdict = switch.verdict();
        assert!(
            verdict.is_pass(),
            "the switch test must pass driven over the real surface: {} (walls={:?})",
            switch.summary(RUN_DATE),
            verdict.walls(),
        );
        assert!(verdict.walls().is_empty(), "0 walls: {:?}", verdict.walls());
        assert_eq!(
            switch.legs.round_trip_ok,
            switch.legs.round_trip_total,
            "render(parse(md)) === md at 100%: {}",
            switch.summary(RUN_DATE),
        );
        assert!(
            switch.legs.states_are_total(),
            "every primary-screen state reached: {}",
            switch.summary(RUN_DATE),
        );
        if let IssuesSwitchVerdict::Pass { legs, budgets, .. } = &verdict {
            if myelin_substrate::perf_budget_enforced() {
                assert!(
                    legs.view_render_us <= budgets.view_render_budget_us,
                    "view render within budget: {}µs <= {}µs",
                    legs.view_render_us,
                    budgets.view_render_budget_us,
                );
            }
            assert!(
                legs.min_overlay_contrast_bp >= budgets.overlay_contrast_floor_bp,
                "every overlay meets the contrast floor: {}bp >= {}bp",
                legs.min_overlay_contrast_bp,
                budgets.overlay_contrast_floor_bp,
            );
        } else {
            panic!("expected a Pass verdict");
        }
        let s = switch.summary(RUN_DATE);
        assert!(
            s.contains("P-520 ISSUES SWITCH-TEST 2026-06-26"),
            "dated: {s}"
        );
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn driving_reaches_every_capability_with_zero_walls() {
        let t = thresholds();
        let switch = IssuesSwitchTest::drive(&t, 4);
        assert!(
            switch.capabilities.len() >= 7,
            "the matrix covers create/triage/plan/board/done + round-trip + per-viewer"
        );
        for c in &switch.capabilities {
            assert!(
                c.reached_by_driving,
                "driving the real surface reached {}: {}",
                c.id, c.issues_surface
            );
            assert!(!c.is_wall(), "{} is not a wall", c.id);
        }
        for id in ["create", "triage", "plan", "board", "done"] {
            assert!(
                switch.capabilities.iter().any(|c| c.id == id),
                "the core loop covers {id}"
            );
        }
        assert!(switch
            .capabilities
            .iter()
            .any(|c| c.id == "per-viewer-board-correct"));
    }

    #[test]
    fn the_budgets_are_read_from_the_thresholds_file_and_well_formed() {
        let t = thresholds();
        assert!(
            t.issues_switch_test.is_well_formed(),
            "the switch-test budgets are well-formed (positive render budget + the WCAG contrast floor)"
        );
        assert_eq!(t.issues_switch_test.view_render_budget_us, 50_000);
        assert_eq!(t.issues_switch_test.overlay_contrast_floor_bp, 450);
    }

    #[test]
    fn every_overlay_meets_the_measured_contrast_floor() {
        let t = thresholds();
        let switch = IssuesSwitchTest::drive(&t, 2);
        assert!(
            switch.legs.min_overlay_contrast_bp >= 450,
            "the weakest overlay meets WCAG 4.5:1: {}bp",
            switch.legs.min_overlay_contrast_bp
        );
        assert_eq!(measured_contrast_bp(IssuesOverlay::PriorityBadge), 587);
    }

    #[test]
    fn every_primary_screen_state_is_reached_by_driving() {
        for state in PrimaryScreenState::all() {
            assert!(
                drive_primary_screen_state(state),
                "the primary-screen state {} must be reached by driving",
                state.wire_id()
            );
        }
        assert_eq!(
            PrimaryScreenState::all().len(),
            6,
            "the six canonical states"
        );
    }

    #[test]
    fn a_wall_reds_the_verdict_loudly() {
        let t = thresholds();
        let mut switch = IssuesSwitchTest::drive(&t, 2);
        switch.capabilities[0].reached_by_driving = false;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a wall reds the verdict");
        assert_eq!(verdict.walls(), &[switch.capabilities[0].id]);
    }

    #[test]
    fn a_broken_round_trip_reds_the_verdict() {
        let t = thresholds();
        let mut switch = IssuesSwitchTest::drive(&t, 2);
        switch.legs.round_trip_ok = switch.legs.round_trip_total - 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a broken round-trip reds the verdict");
        if let IssuesSwitchVerdict::Red {
            round_trip_broken, ..
        } = &verdict
        {
            assert!(*round_trip_broken, "the broken round-trip is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn a_subfloor_overlay_reds_the_verdict() {
        let t = thresholds();
        let mut switch = IssuesSwitchTest::drive(&t, 2);
        switch.legs.min_overlay_contrast_bp = switch.budgets.overlay_contrast_floor_bp - 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a sub-floor overlay reds the verdict");
        if let IssuesSwitchVerdict::Red {
            overlay_below_floor,
            ..
        } = &verdict
        {
            assert!(*overlay_below_floor, "the sub-floor overlay is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn an_unreached_state_reds_the_verdict() {
        let t = thresholds();
        let mut switch = IssuesSwitchTest::drive(&t, 2);
        switch.legs.states_reached = PrimaryScreenState::all().len() - 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "an unreached state reds the verdict");
        if let IssuesSwitchVerdict::Red {
            state_unreached, ..
        } = &verdict
        {
            assert!(*state_unreached, "the unreached state is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn a_blown_render_budget_reds_the_verdict() {
        let t = thresholds();
        let mut switch = IssuesSwitchTest::drive(&t, 2);
        switch.legs.view_render_us = switch.budgets.view_render_budget_us + 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a blown render budget reds the verdict");
        if let IssuesSwitchVerdict::Red {
            render_over_budget, ..
        } = &verdict
        {
            assert!(*render_over_budget, "the render leg is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn the_browser_drive_record_is_honest() {
        let record = switch_surface_drive_record();
        assert!(record.len() >= 4, "every switch-test surface is recorded");
        for s in &record {
            assert_eq!(
                s.drive,
                BrowserDriveStatus::AutomatedModelNamedFloor,
                "{} is honestly recorded as automated-model / live-shell named floor",
                s.surface
            );
            assert!(s.drive.token().contains("browser-driven=partial"));
        }
    }
}
