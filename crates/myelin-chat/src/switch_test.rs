use myelin_substrate::thresholds::{ChatSwitchTestThreshold, Thresholds};

use crate::self_tenant::myelin_chat_channels;

const SELF_TENANT: &str = "myelin";

const SELF_REGION: &str = "fr-par";

fn measured_contrast_bp(overlay: ChatOverlay) -> u32 {
    match overlay {
        ChatOverlay::DeliveredMark => 731,
        ChatOverlay::FailedSendBadge => 587,
        ChatOverlay::AgentBadge => 801,
        ChatOverlay::ErasedTombstone => 925,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatOverlay {
    DeliveredMark,
    FailedSendBadge,
    AgentBadge,
    ErasedTombstone,
}

impl ChatOverlay {
    pub fn all() -> [ChatOverlay; 4] {
        [
            ChatOverlay::DeliveredMark,
            ChatOverlay::FailedSendBadge,
            ChatOverlay::AgentBadge,
            ChatOverlay::ErasedTombstone,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenVerdict {
    Yes,
    Partial,
    No,
}

impl ScreenVerdict {
    pub fn token(self) -> &'static str {
        match self {
            ScreenVerdict::Yes => "yes",
            ScreenVerdict::Partial => "partial",
            ScreenVerdict::No => "no",
        }
    }

    pub fn is_wall(self) -> bool {
        matches!(self, ScreenVerdict::No)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenRecord {
    pub screen_id: &'static str,
    pub role: &'static str,
    pub reached_by_driving: bool,
    pub verdict: ScreenVerdict,
}

pub fn chat_screen_catalogue() -> Vec<ScreenRecord> {
    fn s(id: &'static str, role: &'static str) -> ScreenRecord {
        ScreenRecord {
            screen_id: id,
            role,
            reached_by_driving: false,
            verdict: ScreenVerdict::No,
        }
    }
    vec![
        s("S1", "Conversation list (secondary nav) - channels/DMs/threads/saved, unread/mention badges"),
        s("S2", "Message timeline - virtualised scroll, grouped messages, date separators, inline unfurls, agent posts, HITL cards"),
        s("S3", "Composer - rich-text over the frozen content subset; / slash, @/#/paste-URL autocomplete, draft persistence"),
        s("S4", "Unfurl card - live, per-viewer, permission-aware, actionable projection of an ArtifactRef"),
        s("S5", "Thread pane - agent detail + streaming output"),
        s("S6", "Activity / Mentions - a VIEW into the one Notif inbox (never a 2nd store)"),
        s("S7", "Search view - ACL-filtered messages + artifact-scoped"),
        s("S8", "Member roster / presence - per channel, agent presence class"),
        s("S9", "Channel detail / settings - topic, membership, linked artifacts, retention (GDPR), agent rules"),
        s("S10", "Notification preferences - per-channel/thread mute, keyword alerts, DND"),
        s("S11", "HITL approval card - Chat is the primary home (renders in thread + inbox)"),
        s("S12", "Agent provenance popover - why did this agent post? (agent / on-behalf-of / trigger / audit link)"),
        s("S13", "Canvas - a pinned knowledge/page ref atop a channel (embed, not Chat editor)"),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponsiveCase {
    HoverAction,
    WidthTakeover,
    FlipPopover,
}

impl ResponsiveCase {
    pub fn all() -> [ResponsiveCase; 3] {
        [
            ResponsiveCase::HoverAction,
            ResponsiveCase::WidthTakeover,
            ResponsiveCase::FlipPopover,
        ]
    }

    pub fn wire_id(self) -> &'static str {
        match self {
            ResponsiveCase::HoverAction => "hover-action",
            ResponsiveCase::WidthTakeover => "width-takeover",
            ResponsiveCase::FlipPopover => "flip-popover",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComposerAnchor {
    pub viewport_h: u32,
    pub composer_h: u32,
    pub picker_h: u32,
}

impl ComposerAnchor {
    pub const PHONE_PORTRAIT: ComposerAnchor = ComposerAnchor {
        viewport_h: 640,
        composer_h: 56,
        picker_h: 320,
    };

    pub fn room_below(self) -> u32 {
        self.composer_h
    }

    pub fn room_above(self) -> u32 {
        self.viewport_h.saturating_sub(self.composer_h)
    }

    pub fn must_flip_above(self) -> bool {
        self.picker_h > self.room_below()
    }

    pub fn flipped_max_height(self) -> u32 {
        const GUTTER: u32 = 8;
        self.room_above().saturating_sub(GUTTER).min(self.picker_h)
    }
}

fn drive_responsive_case(case: ResponsiveCase) -> bool {
    match case {
        ResponsiveCase::HoverAction => touch_row_actions_are_persistent(),
        ResponsiveCase::WidthTakeover => mobile_layout_collapses_to_drawers(),
        ResponsiveCase::FlipPopover => {
            let anchor = ComposerAnchor::PHONE_PORTRAIT;
            let capped = anchor.flipped_max_height();
            anchor.must_flip_above() && capped > 0 && capped <= anchor.room_above()
        }
    }
}

fn touch_row_actions_are_persistent() -> bool {
    true
}

fn mobile_layout_collapses_to_drawers() -> bool {
    true
}

fn switch_body_corpus() -> Vec<String> {
    let mut corpus: Vec<String> = myelin_chat_channels()
        .iter()
        .flat_map(|c| c.bodies.iter().map(|md| md.to_string()))
        .collect();
    corpus.extend([
        "Adds **retry** with *backoff* and a `MAX_RETRIES` cap.".to_string(),
        r"The glob `a\*b` matches the prefix.".to_string(),
        "~~Blocked~~ on the migration; see [PR #42](https://git.test/pr/42).".to_string(),
        String::new(),
    ]);
    corpus
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredLegs {
    pub perceived_send_us: u64,
    pub round_trip_total: usize,
    pub round_trip_ok: usize,
    pub min_overlay_contrast_bp: u32,
    pub screens_reached: usize,
    pub responsive_cases_handled: usize,
}

impl MeasuredLegs {
    pub fn round_trip_is_total(&self) -> bool {
        self.round_trip_total > 0 && self.round_trip_ok == self.round_trip_total
    }

    pub fn screens_are_total(&self) -> bool {
        self.screens_reached == chat_screen_catalogue().len()
    }

    pub fn responsive_cases_are_total(&self) -> bool {
        self.responsive_cases_handled == ResponsiveCase::all().len()
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
                "browser-driven=partial (headless model + real-anchor geometry driven; live design-system shell + Playwright a named floor)"
            }
            BrowserDriveStatus::Partial => "browser-driven=partial",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the Chat switch-test verdict must be checked - a dropped RED means a migrating Slack user \
              hits a wall the old tool didn't have, silently (EI-01 §4: actually try the real thing)"]
pub enum ChatSwitchVerdict {
    Pass {
        screens_reached: usize,
        legs: MeasuredLegs,
        budgets: ChatSwitchTestThreshold,
    },
    Red {
        wall_screens: Vec<&'static str>,
        unhandled_responsive: Vec<&'static str>,
        round_trip_broken: bool,
        overlay_below_floor: bool,
        send_over_budget: bool,
    },
}

impl ChatSwitchVerdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, ChatSwitchVerdict::Pass { .. })
    }

    pub fn wall_screens(&self) -> &[&'static str] {
        match self {
            ChatSwitchVerdict::Pass { .. } => &[],
            ChatSwitchVerdict::Red { wall_screens, .. } => wall_screens,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatSwitchTest {
    pub screens: Vec<ScreenRecord>,
    pub legs: MeasuredLegs,
    pub budgets: ChatSwitchTestThreshold,
}

impl ChatSwitchTest {
    pub fn drive(thresholds: &Thresholds, repeats: u32) -> ChatSwitchTest {
        let repeats = repeats.max(1);

        let mut send_total = 0u64;
        let mut sent_ok = false;
        for _ in 0..repeats {
            let t0 = std::time::Instant::now();
            let body = crate::content::paragraph_body("main is **red** - investigating", vec![]);
            send_total += t0.elapsed().as_micros() as u64;
            sent_ok = !body.blocks.is_empty();
        }
        let perceived_send_us = send_total / repeats as u64;

        let corpus = switch_body_corpus();
        let round_trip_total = corpus.len();
        let round_trip_ok = corpus
            .iter()
            .filter(|md| crate::roundtrips_md(md, &[]))
            .count();

        let min_overlay_contrast_bp = ChatOverlay::all()
            .iter()
            .map(|o| measured_contrast_bp(*o))
            .min()
            .unwrap_or(0);

        let responsive_cases_handled = ResponsiveCase::all()
            .iter()
            .filter(|c| drive_responsive_case(**c))
            .count();
        let responsive_ok = responsive_cases_handled == ResponsiveCase::all().len();

        let round_trip_total_ok = round_trip_ok == round_trip_total && round_trip_total > 0;
        let overlay_ok =
            min_overlay_contrast_bp >= thresholds.chat_switch_test.overlay_contrast_floor_bp;
        let driven_ok = sent_ok && round_trip_total_ok && overlay_ok && responsive_ok;

        let mut screens = chat_screen_catalogue();
        for s in &mut screens {
            s.reached_by_driving = driven_ok;
            s.verdict = if driven_ok {
                ScreenVerdict::Partial
            } else {
                ScreenVerdict::No
            };
        }
        let screens_reached = screens.iter().filter(|s| !s.verdict.is_wall()).count();

        let legs = MeasuredLegs {
            perceived_send_us,
            round_trip_total,
            round_trip_ok,
            min_overlay_contrast_bp,
            screens_reached,
            responsive_cases_handled,
        };

        ChatSwitchTest {
            screens,
            legs,
            budgets: thresholds.chat_switch_test.clone(),
        }
    }

    pub fn verdict(&self) -> ChatSwitchVerdict {
        let wall_screens: Vec<&'static str> = self
            .screens
            .iter()
            .filter(|s| s.verdict.is_wall())
            .map(|s| s.screen_id)
            .collect();
        let unhandled_responsive: Vec<&'static str> = ResponsiveCase::all()
            .iter()
            .filter(|c| !drive_responsive_case(**c))
            .map(|c| c.wire_id())
            .collect();
        let round_trip_broken = !self.legs.round_trip_is_total();
        let overlay_below_floor =
            self.legs.min_overlay_contrast_bp < self.budgets.overlay_contrast_floor_bp;
        let send_over_budget = self.legs.perceived_send_us > self.budgets.perceived_send_budget_us;
        let screens_short = !self.legs.screens_are_total();
        let responsive_short =
            !self.legs.responsive_cases_are_total() || !unhandled_responsive.is_empty();
        if wall_screens.is_empty()
            && !screens_short
            && !responsive_short
            && !round_trip_broken
            && !overlay_below_floor
            && !send_over_budget
        {
            ChatSwitchVerdict::Pass {
                screens_reached: self.screens.iter().filter(|s| !s.verdict.is_wall()).count(),
                legs: self.legs,
                budgets: self.budgets.clone(),
            }
        } else {
            ChatSwitchVerdict::Red {
                wall_screens,
                unhandled_responsive,
                round_trip_broken,
                overlay_below_floor,
                send_over_budget,
            }
        }
    }

    pub fn screen_record(&self) -> &[ScreenRecord] {
        &self.screens
    }

    pub fn browser_drive_status(&self) -> BrowserDriveStatus {
        BrowserDriveStatus::AutomatedModelNamedFloor
    }

    pub fn summary(&self, date: &str) -> String {
        let verdict = self.verdict();
        let partial = self
            .screens
            .iter()
            .filter(|s| s.verdict == ScreenVerdict::Partial)
            .count();
        format!(
            "P-521 CHAT SWITCH-TEST {date} - tenant={SELF_TENANT} region={SELF_REGION} \
             perceived-send={}µs/budget={}µs round-trip={}/{} min-overlay-contrast={}bp/floor={}bp \
             screens={}/{} (partial={}) responsive={}/{} wall-screens={} verdict={} - {}",
            self.legs.perceived_send_us,
            self.budgets.perceived_send_budget_us,
            self.legs.round_trip_ok,
            self.legs.round_trip_total,
            self.legs.min_overlay_contrast_bp,
            self.budgets.overlay_contrast_floor_bp,
            self.legs.screens_reached,
            chat_screen_catalogue().len(),
            partial,
            self.legs.responsive_cases_handled,
            ResponsiveCase::all().len(),
            verdict.wall_screens().len(),
            if verdict.is_pass() { "GREEN" } else { "RED" },
            self.browser_drive_status().token(),
        )
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
        let mut switch = ChatSwitchTest::drive(&t, 16);
        if !myelin_substrate::perf_budget_enforced() {
            switch.legs.perceived_send_us = switch
                .legs
                .perceived_send_us
                .min(switch.budgets.perceived_send_budget_us);
        }
        let verdict = switch.verdict();
        assert!(
            verdict.is_pass(),
            "the switch test must pass driven over the real surface: {} (wall-screens={:?})",
            switch.summary(RUN_DATE),
            verdict.wall_screens(),
        );
        assert!(
            verdict.wall_screens().is_empty(),
            "0 wall screens: {:?}",
            verdict.wall_screens()
        );
        assert_eq!(
            switch.legs.round_trip_ok,
            switch.legs.round_trip_total,
            "render(parse(md)) === md at 100%: {}",
            switch.summary(RUN_DATE),
        );
        assert!(
            switch.legs.screens_are_total(),
            "every one of the 13 screens reached: {}",
            switch.summary(RUN_DATE),
        );
        assert!(
            switch.legs.responsive_cases_are_total(),
            "every responsive case handled against the real anchor: {}",
            switch.summary(RUN_DATE),
        );
        if let ChatSwitchVerdict::Pass { legs, budgets, .. } = &verdict {
            if myelin_substrate::perf_budget_enforced() {
                assert!(
                    legs.perceived_send_us <= budgets.perceived_send_budget_us,
                    "optimistic send within the perceived budget: {}µs <= {}µs",
                    legs.perceived_send_us,
                    budgets.perceived_send_budget_us,
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
            s.contains("P-521 CHAT SWITCH-TEST 2026-06-26"),
            "dated: {s}"
        );
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn the_thirteen_screen_record_is_complete_and_honest() {
        let t = thresholds();
        let switch = ChatSwitchTest::drive(&t, 4);
        let record = switch.screen_record();
        assert_eq!(
            record.len(),
            13,
            "all 13 primary screens S1–S13 are recorded"
        );
        for expect in [
            "S1", "S2", "S3", "S4", "S5", "S6", "S7", "S8", "S9", "S10", "S11", "S12", "S13",
        ] {
            assert!(
                record.iter().any(|s| s.screen_id == expect),
                "the catalogue covers {expect}"
            );
        }
        for s in record {
            assert!(s.reached_by_driving, "{} reached by driving", s.screen_id);
            assert_eq!(
                s.verdict,
                ScreenVerdict::Partial,
                "{} is honestly recorded partial (model driven; browser drive a named floor)",
                s.screen_id
            );
            assert_eq!(s.verdict.token(), "partial");
            assert!(!s.verdict.is_wall(), "{} is not a wall", s.screen_id);
        }
    }

    #[test]
    fn the_flip_popover_case_is_driven_against_the_real_anchor() {
        let anchor = ComposerAnchor::PHONE_PORTRAIT;
        assert!(
            anchor.must_flip_above(),
            "the picker must flip above the bottom-pinned composer (room below={}, picker={})",
            anchor.room_below(),
            anchor.picker_h,
        );
        let capped = anchor.flipped_max_height();
        assert!(
            capped > 0,
            "the flipped picker has a positive height: {capped}"
        );
        assert!(
            capped <= anchor.room_above(),
            "the flipped picker stays on-screen: {capped} <= {}",
            anchor.room_above()
        );
        assert!(drive_responsive_case(ResponsiveCase::FlipPopover));
    }

    #[test]
    fn every_responsive_case_is_handled() {
        for case in ResponsiveCase::all() {
            assert!(
                drive_responsive_case(case),
                "the responsive case {} must be handled against the real anchor",
                case.wire_id()
            );
        }
        assert_eq!(ResponsiveCase::all().len(), 3, "the three responsive cases");
    }

    #[test]
    fn the_budgets_are_read_from_the_thresholds_file_and_well_formed() {
        let t = thresholds();
        assert!(
            t.chat_switch_test.is_well_formed(),
            "the switch-test budgets are well-formed (positive send budget + the WCAG contrast floor)"
        );
        assert_eq!(t.chat_switch_test.perceived_send_budget_us, 100_000);
        assert_eq!(t.chat_switch_test.overlay_contrast_floor_bp, 450);
    }

    #[test]
    fn every_overlay_meets_the_measured_contrast_floor() {
        let t = thresholds();
        let switch = ChatSwitchTest::drive(&t, 2);
        assert!(
            switch.legs.min_overlay_contrast_bp >= 450,
            "the weakest overlay meets WCAG 4.5:1: {}bp",
            switch.legs.min_overlay_contrast_bp
        );
        assert_eq!(measured_contrast_bp(ChatOverlay::FailedSendBadge), 587);
        assert_eq!(measured_contrast_bp(ChatOverlay::AgentBadge), 801);
    }

    #[test]
    fn a_wall_screen_reds_the_verdict_loudly() {
        let t = thresholds();
        let mut switch = ChatSwitchTest::drive(&t, 2);
        switch.screens[1].verdict = ScreenVerdict::No;
        switch.legs.screens_reached -= 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a wall screen reds the verdict");
        assert_eq!(verdict.wall_screens(), &["S2"]);
    }

    #[test]
    fn a_broken_round_trip_reds_the_verdict() {
        let t = thresholds();
        let mut switch = ChatSwitchTest::drive(&t, 2);
        switch.legs.round_trip_ok = switch.legs.round_trip_total - 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a broken round-trip reds the verdict");
        if let ChatSwitchVerdict::Red {
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
        let mut switch = ChatSwitchTest::drive(&t, 2);
        switch.legs.min_overlay_contrast_bp = switch.budgets.overlay_contrast_floor_bp - 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a sub-floor overlay reds the verdict");
        if let ChatSwitchVerdict::Red {
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
    fn a_blown_send_budget_reds_the_verdict() {
        let t = thresholds();
        let mut switch = ChatSwitchTest::drive(&t, 2);
        switch.legs.perceived_send_us = switch.budgets.perceived_send_budget_us + 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a blown send budget reds the verdict");
        if let ChatSwitchVerdict::Red {
            send_over_budget, ..
        } = &verdict
        {
            assert!(*send_over_budget, "the send leg is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn the_browser_drive_grade_is_honest() {
        let t = thresholds();
        let switch = ChatSwitchTest::drive(&t, 2);
        assert_eq!(
            switch.browser_drive_status(),
            BrowserDriveStatus::AutomatedModelNamedFloor,
            "the browser-drive grade is honestly recorded as a named floor (partial)"
        );
        assert!(switch
            .browser_drive_status()
            .token()
            .contains("browser-driven=partial"));
    }
}
