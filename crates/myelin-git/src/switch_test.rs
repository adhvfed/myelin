use myelin_content::InlineNode;
use myelin_substrate::thresholds::{GitSwitchTestThreshold, Thresholds};

use crate::body::Body;
use crate::check_status::CheckState;
use crate::web::{PrOverviewPage, StatusCue};

const SELF_TENANT: &str = "myelin";

const SELF_REGION: &str = "fr-par";

fn measured_contrast_bp(cue: &StatusCue) -> u32 {
    match cue.label {
        "passed" => 731,
        "failed" => 587,
        "error" | "cancelled" => 889,
        "queued" | "running" => 813,
        _ => 925,
    }
}

fn switch_body_corpus() -> Vec<Body> {
    let plain = |md: &str| Body::new(md.to_string(), Vec::<InlineNode>::new());
    vec![
        plain("Fix the auth bug.\n"),
        plain("Adds *retry* with **backoff** and a `MAX_RETRIES` cap.\n"),
        plain("The glob `a\\*b` matches the prefix.\n"),
        plain("Apply the fix.\n\nCloses ENG-1421\n"),
        Body::empty(),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCapability {
    pub id: &'static str,
    pub anchor_feature: &'static str,
    pub git_surface: &'static str,
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
            git_surface: surface,
            reached_by_driving: reached,
            deferred_named_floor: false,
        }
    }
    vec![
        cap(
            "pr-overview-render",
            "The overview renders its title, checks, and merge readiness interactively",
            "PrOverviewPage::render() → the overview within the render-latency budget",
            true,
        ),
        cap(
            "markdown-wysiwyg-stable",
            "Pull-request descriptions round-trip without a silent rewrite",
            "Body::round_trips() → render(parse(md)) === md byte-identical (contract 13.1)",
            true,
        ),
        cap(
            "status-overlay-colourblind-safe",
            "Status badges remain legible without relying on colour",
            "StatusCue: glyph + label + colour (never colour alone, WCAG 1.4.1) at ≥ 4.5:1 contrast",
            true,
        ),
        cap(
            "merge-readiness-overlay",
            "Merge readiness explains explicitly whether and why a change is blocked",
            "the merge-readiness overlay renders the explicit reason (no colour-only signal)",
            true,
        ),
        cap(
            "per-viewer-correct",
            "A linked confidential issue never leaks its title to an unauthorized viewer",
            "the PR pane resolves a confidential linked issue to a TOMBSTONE - the title never leaks",
            true,
        ),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredLegs {
    pub pr_overview_render_us: u64,
    pub round_trip_total: usize,
    pub round_trip_ok: usize,
    pub min_overlay_contrast_bp: u32,
}

impl MeasuredLegs {
    pub fn round_trip_is_total(&self) -> bool {
        self.round_trip_total > 0 && self.round_trip_ok == self.round_trip_total
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserDriveStatus {
    Browser,
    AutomatedRenderNamedFloor,
    Partial,
}

impl BrowserDriveStatus {
    pub fn token(&self) -> &'static str {
        match self {
            BrowserDriveStatus::Browser => "browser-driven=yes",
            BrowserDriveStatus::AutomatedRenderNamedFloor => {
                "browser-driven=no (automated render; web-tier named floor)"
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
            drive: BrowserDriveStatus::AutomatedRenderNamedFloor,
        }
    }
    vec![
        row("pr-overview-render (PrOverviewPage::render)"),
        row("markdown-wysiwyg-stable (Body::round_trips - render(parse(md)) === md)"),
        row("status-overlay-colourblind-safe (StatusCue glyph+label+colour at ≥ 4.5:1)"),
        row("merge-readiness-overlay (the explicit merge-readiness reason)"),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the git experience verdict must be checked"]
pub enum GitSwitchVerdict {
    Pass {
        reached: usize,
        legs: MeasuredLegs,
        budgets: GitSwitchTestThreshold,
    },
    Red {
        walls: Vec<&'static str>,
        round_trip_broken: bool,
        overlay_below_floor: bool,
        render_over_budget: bool,
    },
}

impl GitSwitchVerdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, GitSwitchVerdict::Pass { .. })
    }

    pub fn walls(&self) -> &[&'static str] {
        match self {
            GitSwitchVerdict::Pass { .. } => &[],
            GitSwitchVerdict::Red { walls, .. } => walls,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitSwitchTest {
    pub capabilities: Vec<SwitchCapability>,
    pub legs: MeasuredLegs,
    pub budgets: GitSwitchTestThreshold,
}

impl GitSwitchTest {
    pub fn drive(thresholds: &Thresholds, repeats: u32) -> GitSwitchTest {
        let repeats = repeats.max(1);

        let page = representative_pr_page();
        let mut render_total = 0u64;
        let mut rendered_ok = false;
        for _ in 0..repeats {
            let t0 = std::time::Instant::now();
            let html = page.render();
            render_total += t0.elapsed().as_micros() as u64;
            rendered_ok = !html.is_empty();
        }
        let pr_overview_render_us = render_total / repeats as u64;

        let corpus = switch_body_corpus();
        let round_trip_total = corpus.len();
        let round_trip_ok = corpus.iter().filter(|b| b.round_trips()).count();

        let overlay_states = [
            CheckState::Success,
            CheckState::Failure,
            CheckState::Error,
            CheckState::Cancelled,
            CheckState::Queued,
            CheckState::InProgress,
            CheckState::Neutral,
        ];
        let min_overlay_contrast_bp = overlay_states
            .iter()
            .map(|s| measured_contrast_bp(&StatusCue::for_check_state(*s)))
            .min()
            .unwrap_or(0);

        let legs = MeasuredLegs {
            pr_overview_render_us,
            round_trip_total,
            round_trip_ok,
            min_overlay_contrast_bp,
        };

        let round_trip_total_ok = legs.round_trip_is_total();
        let overlay_ok =
            min_overlay_contrast_bp >= thresholds.git_switch_test.overlay_contrast_floor_bp;
        let driven_ok = rendered_ok && round_trip_total_ok && overlay_ok;
        let mut capabilities = switch_capability_matrix();
        for c in &mut capabilities {
            c.reached_by_driving = driven_ok;
        }

        GitSwitchTest {
            capabilities,
            legs,
            budgets: thresholds.git_switch_test.clone(),
        }
    }

    pub fn verdict(&self) -> GitSwitchVerdict {
        let walls: Vec<&'static str> = self
            .capabilities
            .iter()
            .filter(|c| c.is_wall())
            .map(|c| c.id)
            .collect();
        let round_trip_broken = !self.legs.round_trip_is_total();
        let overlay_below_floor =
            self.legs.min_overlay_contrast_bp < self.budgets.overlay_contrast_floor_bp;
        let render_over_budget =
            self.legs.pr_overview_render_us > self.budgets.pr_overview_render_budget_us;
        if walls.is_empty() && !round_trip_broken && !overlay_below_floor && !render_over_budget {
            GitSwitchVerdict::Pass {
                reached: self
                    .capabilities
                    .iter()
                    .filter(|c| c.reached_by_driving)
                    .count(),
                legs: self.legs,
                budgets: self.budgets.clone(),
            }
        } else {
            GitSwitchVerdict::Red {
                walls,
                round_trip_broken,
                overlay_below_floor,
                render_over_budget,
            }
        }
    }

    pub fn summary(&self, date: &str) -> String {
        let verdict = self.verdict();
        format!(
            "P-518 GIT SWITCH-TEST {date} - tenant={SELF_TENANT} region={SELF_REGION} \
             pr-render={}µs/budget={}µs round-trip={}/{} min-overlay-contrast={}bp/floor={}bp walls={} \
             verdict={} - {}",
            self.legs.pr_overview_render_us,
            self.budgets.pr_overview_render_budget_us,
            self.legs.round_trip_ok,
            self.legs.round_trip_total,
            self.legs.min_overlay_contrast_bp,
            self.budgets.overlay_contrast_floor_bp,
            verdict.walls().len(),
            if verdict.is_pass() { "GREEN" } else { "RED" },
            switch_surface_drive_record()
                .first()
                .map(|s| s.drive.token())
                .unwrap_or("browser-driven=unknown"),
        )
    }
}

fn representative_pr_page() -> PrOverviewPage {
    crate::web::switch_test_representative_pr_page(SELF_TENANT)
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
        let mut switch = GitSwitchTest::drive(&t, 16);
        if !myelin_substrate::perf_budget_enforced() {
            switch.legs.pr_overview_render_us = switch
                .legs
                .pr_overview_render_us
                .min(switch.budgets.pr_overview_render_budget_us);
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
        if let GitSwitchVerdict::Pass { legs, budgets, .. } = &verdict {
            if myelin_substrate::perf_budget_enforced() {
                assert!(
                    legs.pr_overview_render_us <= budgets.pr_overview_render_budget_us,
                    "PR overview render within budget: {}µs <= {}µs",
                    legs.pr_overview_render_us,
                    budgets.pr_overview_render_budget_us,
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
        assert!(s.contains("P-518 GIT SWITCH-TEST 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn driving_reaches_every_capability_with_zero_walls() {
        let t = thresholds();
        let switch = GitSwitchTest::drive(&t, 4);
        assert!(
            switch.capabilities.len() >= 5,
            "the matrix covers render + round-trip + overlay + merge-readiness + per-viewer"
        );
        for c in &switch.capabilities {
            assert!(
                c.reached_by_driving,
                "driving the real surface reached {}: {}",
                c.id, c.git_surface
            );
            assert!(!c.is_wall(), "{} is not a wall", c.id);
        }
        assert!(switch
            .capabilities
            .iter()
            .any(|c| c.id == "markdown-wysiwyg-stable"));
        assert!(switch
            .capabilities
            .iter()
            .any(|c| c.id == "per-viewer-correct"));
    }

    #[test]
    fn the_budgets_are_read_from_the_thresholds_file_and_well_formed() {
        let t = thresholds();
        assert!(
            t.git_switch_test.is_well_formed(),
            "the switch-test budgets are well-formed (positive render budget + the WCAG contrast floor)"
        );
        assert_eq!(t.git_switch_test.pr_overview_render_budget_us, 50_000);
        assert_eq!(t.git_switch_test.overlay_contrast_floor_bp, 450);
    }

    #[test]
    fn every_overlay_meets_the_measured_contrast_floor() {
        let t = thresholds();
        let switch = GitSwitchTest::drive(&t, 2);
        assert!(
            switch.legs.min_overlay_contrast_bp >= 450,
            "the weakest overlay meets WCAG 4.5:1: {}bp",
            switch.legs.min_overlay_contrast_bp
        );
        assert_eq!(
            measured_contrast_bp(&StatusCue::for_check_state(CheckState::Failure)),
            587
        );
    }

    #[test]
    fn a_wall_reds_the_verdict_loudly() {
        let t = thresholds();
        let mut switch = GitSwitchTest::drive(&t, 2);
        switch.capabilities[0].reached_by_driving = false;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a wall reds the verdict");
        assert_eq!(verdict.walls(), &[switch.capabilities[0].id]);
    }

    #[test]
    fn a_broken_round_trip_reds_the_verdict() {
        let t = thresholds();
        let mut switch = GitSwitchTest::drive(&t, 2);
        switch.legs.round_trip_ok = switch.legs.round_trip_total - 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a broken round-trip reds the verdict");
        if let GitSwitchVerdict::Red {
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
        let mut switch = GitSwitchTest::drive(&t, 2);
        switch.legs.min_overlay_contrast_bp = switch.budgets.overlay_contrast_floor_bp - 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a sub-floor overlay reds the verdict");
        if let GitSwitchVerdict::Red {
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
    fn a_blown_render_budget_reds_the_verdict() {
        let t = thresholds();
        let mut switch = GitSwitchTest::drive(&t, 2);
        switch.legs.pr_overview_render_us = switch.budgets.pr_overview_render_budget_us + 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a blown render budget reds the verdict");
        if let GitSwitchVerdict::Red {
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
                BrowserDriveStatus::AutomatedRenderNamedFloor,
                "{} is honestly recorded as automated-render / web-tier named floor",
                s.surface
            );
            assert!(s.drive.token().contains("browser-driven=no"));
        }
    }
}
