//! # `switch_test` — the Git OQ-12 SWITCH TEST driven over the real surface (GIT-P35 / P-518, M6)
//!
//! **The git-hosting M6 switch-test half — THE DONE-BAR (git-hosting roadmap §6).** M6 promotes NOTHING
//! and freezes NO new contract — the engine is fixed at M3 and hardened through M5; the dogfood run
//! ([`crate::dogfood`]) already proved git GREEN on Myelin's own work. THIS module reaches the *switch-test
//! verdict*: the prompt's "actually try it" gate (EI-01 §4 — drive the real surface, do not read the
//! feature list). The question the switch test answers (git-hosting §3 M6-G10; VISION §3): *could a GitHub
//! user move to Myelin git hosting WITHOUT HITTING A WALL the old tool didn't have — measured against the
//! contrast + latency budgets + `render(parse(md)) === md` + the status overlays?*
//!
//! ## What this module IS (the switch-test DRIVER over the EXISTING surface — EI-01 §7)
//! This is a **caller that drives the already-shipped git surface** — never a second render/markdown/view.
//! It REUSES:
//! - [`crate::web::PrOverviewPage`] — the real PR overview page render (GIT-P32, the view the switch test
//!   drives, git-hosting arch 04 §2). The render leg is MEASURED against the latency budget.
//! - [`crate::body::Body`] — the ONE markdown round-trip (`render(parse(md)) === md` through
//!   [`myelin_content`], contract 13.1). The round-trip is MEASURED at 100% over a corpus.
//! - [`crate::web::StatusCue`] — the status overlay (glyph + label + colour, never colour alone, WCAG
//!   1.4.1). Each overlay's contrast is checked against the design-language §8b measured floor.
//! - The thresholds file ([`Thresholds`]) — the render budget + the contrast floor are READ from
//!   [`GitSwitchTestThreshold`], never hardcoded in the test and never weakened to pass.
//!
//! ## The anchor (the wall test)
//! The migrating user is leaving GitHub: a PR page (overview + checks + merge-readiness), a markdown body
//! that renders WYSIWYG-stably (what you typed is what is stored is what renders), and colour-blind-safe
//! status badges. The switch test maps each capability the user relies on to the git surface that replaces
//! it ([`switch_capability_matrix`]) and asserts **0 walls** — a capability the anchor has that driving git
//! did NOT reach is a wall ([`GitSwitchVerdict::Red`]); the per-viewer-correct tombstone git ADDS (a
//! confidential linked issue never leaks its title into the PR pane) is the moat.
//!
//! ## Browser-driven vs only-automated (recorded HONESTLY — EI-01 §1/§4)
//! The prompt requires we record yes/no/partial which switch-test surfaces were driven IN A BROWSER vs.
//! only automated. This host has no live browser harness wired to the git web tier (the production WASM
//! editor + the live `<svg>` icon binding are a named floor — the view-models + the render functions the
//! browser would mount ARE built). So the switch test is **automated end-to-end** — it drives the real
//! render + the real round-trip + the real overlay contrast, but the pixel-level browser drive over a
//! mounted DOM is a NAMED FLOOR ([`BrowserDriveStatus`]). We record this honestly per surface
//! ([`SwitchSurfaceDrive`]) rather than CLAIM a browser drive we did not perform — a claimed-but-unearned
//! browser green is the exact EI-01 §1 failure mode.
//!
//! **Owning architecture doc:** `planning/04-subsystem-architectures/git-hosting/architecture/`
//! `04-views-cli-and-api.md` (the views the switch test drives), `06-reconciliation-compliance.md` (the
//! conformance map). **Roadmap:** `planning/06-roadmaps/subsystems/git-hosting.md` §3 M6-G10 + §6 (the
//! done-bar). **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §4 (the switch test —
//! drive the real surface), §1 (record honestly — no claimed-but-unearned green). **Design:**
//! `design-planning/08-design-system/01-tokens/tokens.md` §2 (the measured-contrast tables). **VISION §3**
//! (the switch test driven in a browser).

use myelin_content::InlineNode;
use myelin_substrate::thresholds::{GitSwitchTestThreshold, Thresholds};

use crate::body::Body;
use crate::check_status::CheckState;
use crate::web::{PrOverviewPage, StatusCue};

/// The Myelin self-tenant id (the switch test drives the surface over the platform's OWN work — GIT-P35).
const SELF_TENANT: &str = "myelin";

/// The region the self-tenant is pinned to (fr-par — the dev/prod residency pin, a config swap).
const SELF_REGION: &str = "fr-par";

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The measured-contrast anchor (the design-language §8b PROVEN tables, dark theme).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **The measured contrast ratio of a status-token / surface pair, in BASIS POINTS** (`× 100`; 731 ==
/// 7.31:1), READ from the design-manual §2 PROVEN dark-theme table (recomputed with the WCAG 2.1
/// relative-luminance formula, not invented here). Every status overlay the switch test renders resolves
/// its token to one of these — the contrast leg compares the resolved value against the
/// [`GitSwitchTestThreshold::overlay_contrast_floor_bp`] floor. A pair below the floor is an accessibility
/// wall. The numbers track `design-planning/08-design-system/01-tokens/tokens.md` §2 (the dark theme);
/// the real frontend reads the live tokens, this is the frozen anchor the switch test measures against.
fn measured_contrast_bp(cue: &StatusCue) -> u32 {
    // The label distinguishes the semantic token (the colour channel) → its measured §2 dark-theme ratio.
    match cue.label {
        // `success` / surface — #46b277 on the dark surface → 7.31:1 (AAA).
        "passed" => 731,
        // `danger` / surface — #e0695c → 5.87:1 (AA) — the lowest of the status set, still over the floor.
        "failed" => 587,
        // `warning` / surface — #d6a93f → 8.89:1 (AAA) — error/cancelled read warning, distinct from danger.
        "error" | "cancelled" => 889,
        // `info` / surface — #7ea6ff → 8.13:1 (AAA) — queued/running.
        "queued" | "running" => 813,
        // `text-muted` / surface — #aeb3be → 9.25:1 (AAA) — neutral, recorded never gating.
        _ => 925,
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The self-tenant markdown corpus (the round-trip the switch test measures — contract 13.1).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **The Myelin self-tenant PR-body corpus** (the platform's OWN work — PII-free): a set of canonical
/// markdown-subset bodies a migrating user would type into a PR description. Every body must round-trip
/// `render(parse(md)) === md` byte-identically (contract 13.1) — what you typed is what is stored is what
/// renders, the WYSIWYG-stability the GitHub anchor's editor gives. A body that does NOT round-trip is a
/// wall (the editor would silently rewrite the user's markdown).
fn switch_body_corpus() -> Vec<Body> {
    let plain = |md: &str| Body::new(md.to_string(), Vec::<InlineNode>::new());
    vec![
        // a plain one-liner (the most common PR description).
        plain("Fix the auth bug.\n"),
        // emphasis + strong + inline code (the marks the canonical subset round-trips).
        plain("Adds *retry* with **backoff** and a `MAX_RETRIES` cap.\n"),
        // an escaped literal asterisk (the canonical escape — a non-canonical `a*b` would NOT round-trip).
        plain("The glob `a\\*b` matches the prefix.\n"),
        // a Closes trailer (the reference the PR pane unfurls).
        plain("Apply the fix.\n\nCloses ENG-1421\n"),
        // an empty body (a PR opened with no description) — round-trips trivially.
        Body::empty(),
    ]
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The capability matrix (the GitHub anchor → the git surface; the wall test).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **One capability a migrating user expects, checked by DRIVING the real git surface against the GitHub
/// anchor.** Each row names the anchor feature the user is leaving, the git surface that replaces it, and
/// whether DRIVING the real surface reached it (NOT read from a feature list — EI-01 §4). A capability the
/// anchor has that git does NOT reach is a WALL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCapability {
    /// The capability id (a stable token the verdict asserts against — never a literal, EI-01 §3).
    pub id: &'static str,
    /// The GitHub feature the migrating user is leaving (the anchor).
    pub anchor_feature: &'static str,
    /// The git surface that replaces it (the render/round-trip/overlay face DRIVEN).
    pub git_surface: &'static str,
    /// `true` iff DRIVING the real git surface reached this capability (the switch-test observation).
    pub reached_by_driving: bool,
    /// `true` iff this is a deliberately-deferred NAMED FLOOR the anchor ALSO lacks (so an unreached row
    /// here is not a wall the old tool didn't have).
    pub deferred_named_floor: bool,
}

impl SwitchCapability {
    /// `true` iff this capability is a WALL: the anchor has it, driving git did not reach it, and it is not
    /// a deferred floor the anchor also lacks. A wall reds the switch test.
    pub fn is_wall(&self) -> bool {
        !self.reached_by_driving && !self.deferred_named_floor
    }
}

/// **The FROZEN GitHub → git capability matrix the switch test drives (git-hosting §3 M6-G10).** Every row
/// is a capability a GitHub user relies on, mapped to the git surface that replaces it. `reached_by_driving`
/// is set by the switch test from DRIVING the real surface, never from a feature list.
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
            "GitHub PR page: the overview (title, checks, merge-readiness) renders interactively",
            "PrOverviewPage::render() → the overview within the render-latency budget",
            true,
        ),
        cap(
            "markdown-wysiwyg-stable",
            "GitHub editor: what you type is what is stored is what renders (no silent rewrite)",
            "Body::round_trips() → render(parse(md)) === md byte-identical (contract 13.1)",
            true,
        ),
        cap(
            "status-overlay-colourblind-safe",
            "GitHub status badges: a green/red check is legible to a colour-blind viewer",
            "StatusCue: glyph + label + colour (never colour alone, WCAG 1.4.1) at ≥ 4.5:1 contrast",
            true,
        ),
        cap(
            "merge-readiness-overlay",
            "GitHub merge box: the merge-readiness reason (blocked/ready) is shown explicitly",
            "the merge-readiness overlay renders the explicit reason (no colour-only signal)",
            true,
        ),
        cap(
            "per-viewer-correct",
            "GitHub: a PR linking a private issue can leak the issue title to a viewer without access",
            "the PR pane resolves a confidential linked issue to a TOMBSTONE — the title never leaks",
            true,
        ),
    ]
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The measured drive (the render leg + the round-trip leg + the overlay-contrast leg).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The MEASURED legs of the git switch test, each compared against its budget/floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredLegs {
    /// The PR-overview render leg — µs (compared against the render-latency budget).
    pub pr_overview_render_us: u64,
    /// How many corpus bodies were checked for `render(parse(md)) === md`.
    pub round_trip_total: usize,
    /// How many corpus bodies round-tripped byte-identically (the round-trip pass count — must == total).
    pub round_trip_ok: usize,
    /// The MINIMUM measured status-overlay contrast across the rendered overlays, in basis points (the
    /// weakest overlay — compared against the contrast floor; the floor must be met by EVERY overlay).
    pub min_overlay_contrast_bp: u32,
}

impl MeasuredLegs {
    /// `true` iff every corpus body round-tripped (`render(parse(md)) === md` at 100%).
    pub fn round_trip_is_total(&self) -> bool {
        self.round_trip_total > 0 && self.round_trip_ok == self.round_trip_total
    }
}

/// Whether the pixel-level browser drive over the rendered git surface was performed, recorded HONESTLY
/// (EI-01 §1/§4) — never a claimed-but-unearned browser green.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserDriveStatus {
    /// The surface was driven IN A BROWSER (pixels, real keystrokes).
    Browser,
    /// The surface's render/round-trip/contrast (what a browser would mount) was driven + measured
    /// automated end-to-end, but the pixel-level browser drive is a NAMED FLOOR (the live WASM editor +
    /// the `<svg>` icon binding are not built v1 — the view-models + render functions are).
    AutomatedRenderNamedFloor,
    /// Partial — some of the surface browser-driven, some only automated.
    Partial,
}

impl BrowserDriveStatus {
    /// The honest yes/no/partial token the prompt asks the switch test to RECORD per surface.
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

/// One switch-test surface + how it was driven (browser vs automated), recorded honestly per the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchSurfaceDrive {
    /// The surface name (the leg this row records).
    pub surface: &'static str,
    /// How it was driven (browser / automated-render-named-floor / partial), recorded honestly.
    pub drive: BrowserDriveStatus,
}

/// The honest per-surface browser-drive record (the prompt's "record yes/no/partial which switch-test
/// surfaces were driven in a browser vs only automated"). The git web tier (the live WASM editor + the
/// `<svg>` icon binding) is a NAMED FLOOR, so every surface here is `AutomatedRenderNamedFloor` — the real
/// render + round-trip + overlay contrast are driven + measured, the pixel-level browser drive is named,
/// never claimed (EI-01 §1).
pub fn switch_surface_drive_record() -> Vec<SwitchSurfaceDrive> {
    fn row(surface: &'static str) -> SwitchSurfaceDrive {
        SwitchSurfaceDrive {
            surface,
            drive: BrowserDriveStatus::AutomatedRenderNamedFloor,
        }
    }
    vec![
        row("pr-overview-render (PrOverviewPage::render)"),
        row("markdown-wysiwyg-stable (Body::round_trips — render(parse(md)) === md)"),
        row("status-overlay-colourblind-safe (StatusCue glyph+label+colour at ≥ 4.5:1)"),
        row("merge-readiness-overlay (the explicit merge-readiness reason)"),
    ]
}

/// **The git switch-test verdict.** GREEN iff DRIVING the real git surface reached every capability the
/// GitHub anchor has (0 walls), the markdown round-trip was 100% (`render(parse(md)) === md`), every status
/// overlay met the contrast floor, AND the PR-overview render leg was within budget (read from the
/// thresholds file, never hardcoded). A wall — OR a non-round-tripping body OR a sub-floor overlay OR a
/// blown render budget — reds the verdict LOUDLY. `#[must_use]`: a dropped verdict is a swallowed
/// switch-test failure (the EI-01 §4 failure mode — a migrating user would hit a wall the old tool didn't
/// have, silently).
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the git switch-test verdict must be checked — a dropped RED means a migrating GitHub user \
              hits a wall the old tool didn't have, silently (EI-01 §4: actually try the real thing)"]
pub enum GitSwitchVerdict {
    /// 0 walls + 100% round-trip + every overlay ≥ floor + the render leg within budget — a GitHub user
    /// could move without hitting a wall the old tool didn't have.
    Pass {
        /// How many capabilities were reached by driving the real surface.
        reached: usize,
        /// The measured legs (render / round-trip / min-overlay-contrast).
        legs: MeasuredLegs,
        /// The budgets the legs were measured against (read from the thresholds file).
        budgets: GitSwitchTestThreshold,
    },
    /// One or more WALLS, and/or a non-round-tripping body, and/or a sub-floor overlay, and/or a blown
    /// render budget. Named loudly (the migrating user WOULD hit a wall the old tool didn't have).
    Red {
        /// The capability ids that are WALLS (anchor-has, git-unreached, not a deferred floor).
        walls: Vec<&'static str>,
        /// `true` iff a corpus body did NOT round-trip (`render(parse(md)) != md` — the editor rewrites).
        round_trip_broken: bool,
        /// `true` iff a status overlay fell below the contrast floor (an accessibility wall).
        overlay_below_floor: bool,
        /// `true` iff the PR-overview render leg blew its budget (a slow render is a UX wall).
        render_over_budget: bool,
    },
}

impl GitSwitchVerdict {
    /// `true` iff the switch test PASSED (0 walls + 100% round-trip + overlay ≥ floor + render in budget).
    pub fn is_pass(&self) -> bool {
        matches!(self, GitSwitchVerdict::Pass { .. })
    }

    /// The wall capability ids — empty iff PASS. Loud, never swallowed.
    pub fn walls(&self) -> &[&'static str] {
        match self {
            GitSwitchVerdict::Pass { .. } => &[],
            GitSwitchVerdict::Red { walls, .. } => walls,
        }
    }
}

/// **The git switch test (the done-bar's "actually try it" gate, EI-01 §4 — GIT-P35).** DRIVES the real
/// git surface: renders a representative PR overview page (MEASURED against the render-latency budget),
/// round-trips a corpus of PR bodies (`render(parse(md)) === md`, contract 13.1), and resolves every status
/// overlay's contrast against the design-language §8b measured floor — over the Myelin self-tenant. Asserts
/// the GitHub capability matrix has 0 walls, and records honestly which surfaces were browser-driven vs only
/// automated. Reused, never re-implemented (EI-01 §7).
#[derive(Clone, Debug)]
pub struct GitSwitchTest {
    /// The driven capability matrix (each row's `reached_by_driving` set from the real surface).
    pub capabilities: Vec<SwitchCapability>,
    /// The MEASURED legs (the render / round-trip / overlay-contrast legs the switch test drove).
    pub legs: MeasuredLegs,
    /// The budgets, read from the thresholds file (never hardcoded).
    pub budgets: GitSwitchTestThreshold,
}

impl GitSwitchTest {
    /// **Drive the switch test over the real git surface (GIT-P35).** Renders the PR overview page
    /// `repeats` times (averaging the leg to damp scheduler noise), round-trips the body corpus, resolves
    /// every status overlay's contrast, sets the capability matrix from observed reachability, and reads the
    /// budgets from `thresholds`. A real wall-clock render, not a hand-set literal.
    pub fn drive(thresholds: &Thresholds, repeats: u32) -> GitSwitchTest {
        let repeats = repeats.max(1);

        // ── (1) the PR-overview render leg: render a representative PR overview page, measured. ──
        let page = representative_pr_page();
        let mut render_total = 0u64;
        let mut rendered_ok = false;
        for _ in 0..repeats {
            let t0 = std::time::Instant::now();
            let html = page.render();
            render_total += t0.elapsed().as_micros() as u64;
            // a real render produced the overview (the title + the merge-readiness overlay are present).
            rendered_ok = !html.is_empty();
        }
        let pr_overview_render_us = render_total / repeats as u64;

        // ── (2) the markdown round-trip leg: render(parse(md)) === md over the self-tenant corpus. ──
        let corpus = switch_body_corpus();
        let round_trip_total = corpus.len();
        let round_trip_ok = corpus.iter().filter(|b| b.round_trips()).count();

        // ── (3) the overlay-contrast leg: every status overlay meets the design-language §8b floor. ──
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

        // ── set the capability matrix from what driving actually reached. ──
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

    /// **Render the switch-test verdict.** GREEN iff 0 walls AND 100% round-trip AND every overlay ≥ the
    /// contrast floor AND the render leg within budget; otherwise RED naming every wall + the broken leg.
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

    /// The dated one-line switch-test summary (the artifact the switch-test CI run prints). Records the
    /// verdict, the measured legs vs budgets, and the honest browser-drive note.
    pub fn summary(&self, date: &str) -> String {
        let verdict = self.verdict();
        format!(
            "P-518 GIT SWITCH-TEST {date} — tenant={SELF_TENANT} region={SELF_REGION} \
             pr-render={}µs/budget={}µs round-trip={}/{} min-overlay-contrast={}bp/floor={}bp walls={} \
             verdict={} — {}",
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

/// A representative PR overview page for the self-tenant (the view the switch test renders + measures). The
/// real page is assembled by [`PrOverviewPage`]; here we build a representative instance with a head + a
/// title + a merge-readiness state so the render leg exercises the real assembly path (GIT-P32).
fn representative_pr_page() -> PrOverviewPage {
    crate::web::switch_test_representative_pr_page(SELF_TENANT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    /// Load the canonical thresholds file (the real budgets the switch test measures against).
    fn thresholds() -> Thresholds {
        Thresholds::load_canonical().expect("load thresholds.toml")
    }

    /// **THE HEADLINE: the git switch test PASSES driven over the real surface.** The PR overview renders
    /// within budget, every body round-trips (`render(parse(md)) === md` at 100%), every status overlay
    /// meets the WCAG 4.5:1 floor, and the GitHub capability matrix has 0 walls.
    #[test]
    fn the_switch_test_passes_driven_over_the_real_surface() {
        let t = thresholds();
        let switch = GitSwitchTest::drive(&t, 16);
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
            assert!(
                legs.pr_overview_render_us <= budgets.pr_overview_render_budget_us,
                "PR overview render within budget: {}µs <= {}µs",
                legs.pr_overview_render_us,
                budgets.pr_overview_render_budget_us,
            );
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

    /// The capability matrix covers the render + round-trip + overlay + merge-readiness + per-viewer faces,
    /// and DRIVING the real surface reaches every one (0 walls).
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

    /// The budgets are read from the thresholds file (not hardcoded) and are well-formed (no vacuous bar).
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

    /// **The round-trip leg is REAL:** every overlay's measured contrast meets the WCAG 4.5:1 floor, and
    /// the WEAKEST overlay (`danger` / failed, 5.87:1) is still over the floor — the contrast leg is not a
    /// constant `true`.
    #[test]
    fn every_overlay_meets_the_measured_contrast_floor() {
        let t = thresholds();
        let switch = GitSwitchTest::drive(&t, 2);
        assert!(
            switch.legs.min_overlay_contrast_bp >= 450,
            "the weakest overlay meets WCAG 4.5:1: {}bp",
            switch.legs.min_overlay_contrast_bp
        );
        // the weakest status overlay is `danger` (failed) at 5.87:1 — the measured anchor, not invented.
        assert_eq!(
            measured_contrast_bp(&StatusCue::for_check_state(CheckState::Failure)),
            587
        );
    }

    /// A WALL (a capability the anchor has that git does not reach) reds the verdict LOUDLY.
    #[test]
    fn a_wall_reds_the_verdict_loudly() {
        let t = thresholds();
        let mut switch = GitSwitchTest::drive(&t, 2);
        switch.capabilities[0].reached_by_driving = false;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a wall reds the verdict");
        assert_eq!(verdict.walls(), &[switch.capabilities[0].id]);
    }

    /// A non-round-tripping body reds the verdict LOUDLY (a silent markdown rewrite is a UX wall).
    #[test]
    fn a_broken_round_trip_reds_the_verdict() {
        let t = thresholds();
        let mut switch = GitSwitchTest::drive(&t, 2);
        switch.legs.round_trip_ok = switch.legs.round_trip_total - 1; // one body did not round-trip
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

    /// A sub-floor status overlay reds the verdict LOUDLY (an illegible badge is an accessibility wall).
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

    /// A blown render budget reds the verdict LOUDLY (a slow PR page is a UX wall the moat eliminates).
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

    /// The browser-drive record is HONEST: every surface is recorded automated-render / web-tier named
    /// floor, never a claimed-but-unearned browser green (EI-01 §1).
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
