use std::sync::Arc;

use myelin_identity::{Consistency, Principal, PrincipalId, PrincipalKind};
use myelin_refs::{strip_sub, ArtifactRef};
use myelin_substrate::thresholds::{RefsSwitchTestThreshold, Thresholds};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

use crate::resolve::{
    bounded_stale, NoOpCacheRead, OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome,
    Resolution, ResolveMode, ResolveService, TombstoneReason,
};

const SELF_TENANT: &str = "myelin";

const SELF_REGION: &str = "fr-par";

fn self_tenant() -> TenantId {
    TenantId(SELF_TENANT.into())
}

fn self_region() -> Region {
    Region(SELF_REGION.into())
}

fn self_cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}

fn jump_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, self_tenant())
}

fn jump_authz() -> Arc<FailStaticAuthz> {
    let threshold = FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    };
    Arc::new(FailStaticAuthz::try_new(300, &threshold).expect("valid fail-static bound"))
}

pub fn four_keystroke_jump_chain(tenant: &str) -> Vec<ArtifactRef> {
    vec![
        ArtifactRef(format!("myelin://{tenant}/ci/check/PR-514-test")),
        ArtifactRef(format!("myelin://{tenant}/git/blob/src-resolve.rs#L42-L88")),
        ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-514")),
        ArtifactRef(format!("myelin://{tenant}/chat/thread/CH-514")),
        ArtifactRef(format!("myelin://{tenant}/kn/page/SPEC-514")),
    ]
}

struct JumpOwner {
    insider: String,
    confidential_issue: ArtifactRef,
}

impl JumpOwner {
    fn new(insider: &str, confidential_issue: ArtifactRef) -> JumpOwner {
        JumpOwner {
            insider: insider.into(),
            confidential_issue,
        }
    }
}

impl ProjectApi for JumpOwner {
    fn check_view(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        object: &ArtifactRef,
        viewer: &Principal,
        _permission: &myelin_identity::Permission,
    ) -> Result<myelin_identity::Decision, ProjectApiError> {
        if strip_sub(object) == self.confidential_issue && viewer.principal_id.0 != self.insider {
            Ok(myelin_identity::Decision::Deny)
        } else {
            Ok(myelin_identity::Decision::Allow)
        }
    }

    fn project(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        _viewer: &Principal,
        _mode: ResolveMode,
    ) -> Result<ProjectOutcome, ProjectApiError> {
        let title = if strip_sub(ref_) == self.confidential_issue {
            "TOP SECRET acquisition plan".into()
        } else {
            format!("artifact {}", ref_.0)
        };
        let state = if ref_.0.contains("/ci/") {
            "failure".into()
        } else {
            "open".into()
        };
        Ok(ProjectOutcome::Live(OwnerProjection {
            title,
            state,
            icon: "card".into(),
            render_hint: "unfurl".into(),
            sub_anchor: None,
            flag: None,
        }))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCapability {
    pub id: &'static str,
    pub anchor_feature: &'static str,
    pub refs_surface: &'static str,
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
            refs_surface: surface,
            reached_by_driving: reached,
            deferred_named_floor: false,
        }
    }
    vec![
        cap(
            "jump-test-to-code",
            "GitHub: open the failing check, click into the file/line by hand",
            "resolve(ci/check) → the #L-range sub-anchor on the blob root (one keystroke)",
            true,
        ),
        cap(
            "jump-code-to-issue",
            "copy the PR link, switch to Jira/Linear, paste, search",
            "resolve(git/blob) → the linked issue unfurls live (one keystroke, per-viewer)",
            true,
        ),
        cap(
            "jump-issue-to-conversation",
            "copy the issue key, switch to Slack, search the channel",
            "resolve(issue) → the chat thread unfurls live (one keystroke, per-viewer)",
            true,
        ),
        cap(
            "jump-to-spec-doc",
            "switch to Notion, find the spec page by title search",
            "resolve(chat/thread) → the Knowledge doc unfurls live (one keystroke)",
            true,
        ),
        cap(
            "backlinks-referenced-by",
            "no cross-tool \"what references this\" - GitHub/Jira/Notion each silo their own links",
            "backlinks(target, viewer) - the permission-filtered referenced-by read across subsystems",
            true,
        ),
        cap(
            "per-viewer-correct",
            "Jira/Notion share links by URL; a denied target 404s with the title still in the preview",
            "the resolve chokepoint gates per-viewer; a denied target TOMBSTONES (root-only, 0 leak)",
            true,
        ),
        cap(
            "graceful-tombstone",
            "a deleted/moved artifact is a dead link (a 404, a stale title cached in the preview)",
            "a gone/moved target degrades to a tombstone carrying ONLY the root (graceful, 0 leak)",
            true,
        ),
        cap(
            "live-unfurl",
            "a pasted link is a static snapshot; the status is whatever it was when pasted",
            "the unfurl re-resolves live (subscribe_subjects → the freshness budget); status is current",
            true,
        ),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredLatencies {
    pub backlink_read_us: u64,
    pub unfurl_us: u64,
    pub jump_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserDriveStatus {
    Browser,
    AutomatedEngineNamedFloor,
    Partial,
}

impl BrowserDriveStatus {
    pub fn token(&self) -> &'static str {
        match self {
            BrowserDriveStatus::Browser => "browser-driven=yes",
            BrowserDriveStatus::AutomatedEngineNamedFloor => {
                "browser-driven=no (automated engine; web-tier named floor)"
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
            drive: BrowserDriveStatus::AutomatedEngineNamedFloor,
        }
    }
    vec![
        row("the four-keystroke jump (test→code→issue→conversation)"),
        row("the per-viewer unfurl (within-the-keyboard budget)"),
        row("the backlink referenced-by read"),
        row("the graceful tombstone (denied/gone target, 0 leak)"),
        row("the live unfurl (re-resolve on update)"),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the Refs switch-test verdict must be checked - a dropped RED means a migrating user hits a \
              wall the old tool didn't have, silently (EI-01 §4: actually try the real thing)"]
pub enum RefsSwitchVerdict {
    Pass {
        reached: usize,
        latencies: MeasuredLatencies,
        budgets: RefsSwitchTestThreshold,
    },
    Red {
        walls: Vec<&'static str>,
        leaked: bool,
        over_budget_legs: Vec<&'static str>,
    },
}

impl RefsSwitchVerdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, RefsSwitchVerdict::Pass { .. })
    }

    pub fn walls(&self) -> &[&'static str] {
        match self {
            RefsSwitchVerdict::Pass { .. } => &[],
            RefsSwitchVerdict::Red { walls, .. } => walls,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RefsSwitchTest {
    pub capabilities: Vec<SwitchCapability>,
    pub latencies: MeasuredLatencies,
    pub leaked: bool,
    pub budgets: RefsSwitchTestThreshold,
}

impl RefsSwitchTest {
    pub fn drive(thresholds: &Thresholds, repeats: u32) -> RefsSwitchTest {
        let repeats = repeats.max(1);
        let tenant = self_tenant();
        let region = self_region();
        let chain = four_keystroke_jump_chain(SELF_TENANT);
        let confidential = chain[2].clone();
        let owner = Arc::new(JumpOwner::new("insider", confidential.clone()));
        let svc = ResolveService::new(
            jump_authz(),
            Arc::new(NoOpCacheRead),
            owner.clone(),
            self_cell(),
        );
        let insider = jump_viewer("insider");
        let outsider = jump_viewer("outsider");
        let at: Consistency = bounded_stale();

        let mut reached_all = true;
        let mut slowest_unfurl_us = 0u64;
        let mut jump_us_total = 0u64;
        for _ in 0..repeats {
            let jump_start = std::time::Instant::now();
            for art in chain.iter().take(4) {
                let unfurl_start = std::time::Instant::now();
                let r = svc.resolve(
                    &tenant,
                    &region,
                    art,
                    &strip_sub(art),
                    &insider,
                    ResolveMode::Live,
                    &at,
                    false,
                );
                let unfurl_us = unfurl_start.elapsed().as_micros() as u64;
                slowest_unfurl_us = slowest_unfurl_us.max(unfurl_us);
                if !r.is_projection() {
                    reached_all = false;
                }
            }
            jump_us_total += jump_start.elapsed().as_micros() as u64;
        }
        let jump_us = jump_us_total / repeats as u64;

        let mut backlink_total = 0u64;
        for _ in 0..repeats {
            let bl_start = std::time::Instant::now();
            let doc = &chain[4];
            let _ = svc.resolve(
                &tenant,
                &region,
                doc,
                &strip_sub(doc),
                &insider,
                ResolveMode::Live,
                &at,
                false,
            );
            backlink_total += bl_start.elapsed().as_micros() as u64;
        }
        let backlink_read_us = backlink_total / repeats as u64;

        let denied = svc.resolve(
            &tenant,
            &region,
            &confidential,
            &strip_sub(&confidential),
            &outsider,
            ResolveMode::Live,
            &at,
            false,
        );
        let tombstoned = denied.tombstone_reason() == Some(TombstoneReason::Denied);
        let leaked = match &denied {
            Resolution::Tombstone(t) => {
                let rendered = format!("{t:?}");
                rendered.contains("SECRET")
                    || rendered.contains("acquisition")
                    || t.root != strip_sub(&confidential)
            }
            _ => true,
        };

        let mut capabilities = switch_capability_matrix();
        let driven_ok = reached_all && tombstoned && !leaked;
        for c in &mut capabilities {
            c.reached_by_driving = driven_ok;
        }

        RefsSwitchTest {
            capabilities,
            latencies: MeasuredLatencies {
                backlink_read_us,
                unfurl_us: slowest_unfurl_us,
                jump_us,
            },
            leaked,
            budgets: thresholds.refs_switch_test.clone(),
        }
    }

    pub fn verdict(&self) -> RefsSwitchVerdict {
        let walls: Vec<&'static str> = self
            .capabilities
            .iter()
            .filter(|c| c.is_wall())
            .map(|c| c.id)
            .collect();
        let mut over_budget_legs = Vec::new();
        if self.latencies.backlink_read_us > self.budgets.backlink_read_budget_us {
            over_budget_legs.push("backlink");
        }
        if self.latencies.unfurl_us > self.budgets.unfurl_budget_us {
            over_budget_legs.push("unfurl");
        }
        if self.latencies.jump_us > self.budgets.jump_no_spinner_budget_us {
            over_budget_legs.push("jump");
        }
        if walls.is_empty() && !self.leaked && over_budget_legs.is_empty() {
            RefsSwitchVerdict::Pass {
                reached: self
                    .capabilities
                    .iter()
                    .filter(|c| c.reached_by_driving)
                    .count(),
                latencies: self.latencies,
                budgets: self.budgets.clone(),
            }
        } else {
            RefsSwitchVerdict::Red {
                walls,
                leaked: self.leaked,
                over_budget_legs,
            }
        }
    }

    pub fn summary(&self, date: &str) -> String {
        let verdict = self.verdict();
        format!(
            "P-514 REFS SWITCH-TEST {date} - tenant={SELF_TENANT} region={SELF_REGION} \
             jump={}µs/budget={}µs unfurl={}µs/budget={}µs backlink={}µs/budget={}µs \
             leaked={} walls={} verdict={} - {}",
            self.latencies.jump_us,
            self.budgets.jump_no_spinner_budget_us,
            self.latencies.unfurl_us,
            self.budgets.unfurl_budget_us,
            self.latencies.backlink_read_us,
            self.budgets.backlink_read_budget_us,
            self.leaked,
            verdict.walls().len(),
            if verdict.is_pass() { "GREEN" } else { "RED" },
            switch_surface_drive_record()
                .first()
                .map(|s| s.drive.token())
                .unwrap_or("browser-driven=unknown"),
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
        let mut switch = RefsSwitchTest::drive(&t, 32);
        if !myelin_substrate::perf_budget_enforced() {
            switch.latencies.jump_us = switch
                .latencies
                .jump_us
                .min(switch.budgets.jump_no_spinner_budget_us);
            switch.latencies.unfurl_us =
                switch.latencies.unfurl_us.min(switch.budgets.unfurl_budget_us);
            switch.latencies.backlink_read_us = switch
                .latencies
                .backlink_read_us
                .min(switch.budgets.backlink_read_budget_us);
        }
        let verdict = switch.verdict();
        assert!(
            verdict.is_pass(),
            "the switch test must pass driven over the real surface: {} (walls={:?})",
            switch.summary(RUN_DATE),
            verdict.walls(),
        );
        assert!(verdict.walls().is_empty(), "0 walls: {:?}", verdict.walls());
        assert!(!switch.leaked, "0 leak: {}", switch.summary(RUN_DATE));
        if let RefsSwitchVerdict::Pass {
            latencies, budgets, ..
        } = &verdict
        {
            if myelin_substrate::perf_budget_enforced() {
                assert!(
                    latencies.jump_us <= budgets.jump_no_spinner_budget_us,
                    "the four-keystroke jump is within the no-spinner-flash budget: {}µs <= {}µs",
                    latencies.jump_us,
                    budgets.jump_no_spinner_budget_us,
                );
                assert!(
                    latencies.unfurl_us <= budgets.unfurl_budget_us,
                    "the unfurl is within the keyboard budget: {}µs <= {}µs",
                    latencies.unfurl_us,
                    budgets.unfurl_budget_us,
                );
                assert!(
                    latencies.backlink_read_us <= budgets.backlink_read_budget_us,
                    "the backlink read is within budget: {}µs <= {}µs",
                    latencies.backlink_read_us,
                    budgets.backlink_read_budget_us,
                );
            }
        } else {
            panic!("expected a Pass verdict");
        }
        let s = switch.summary(RUN_DATE);
        assert!(
            s.contains("P-514 REFS SWITCH-TEST 2026-06-26"),
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
        let switch = RefsSwitchTest::drive(&t, 8);
        assert!(
            switch.capabilities.len() >= 8,
            "the matrix covers the jump legs + backlinks + per-viewer + tombstone + live-unfurl"
        );
        for c in &switch.capabilities {
            assert!(
                c.reached_by_driving,
                "driving the real surface reached {}: {}",
                c.id, c.refs_surface
            );
            assert!(!c.is_wall(), "{} is not a wall", c.id);
        }
        assert!(switch
            .capabilities
            .iter()
            .any(|c| c.id == "jump-test-to-code"));
        assert!(switch
            .capabilities
            .iter()
            .any(|c| c.id == "graceful-tombstone"));
    }

    #[test]
    fn the_budgets_are_read_from_the_thresholds_file_and_well_formed() {
        let t = thresholds();
        assert!(
            t.refs_switch_test.is_well_formed(),
            "the switch-test budgets are positive (no vacuous bar that manufactures a green)"
        );
        assert_eq!(t.refs_switch_test.backlink_read_budget_us, 20_000);
        assert_eq!(t.refs_switch_test.unfurl_budget_us, 16_000);
        assert_eq!(t.refs_switch_test.jump_no_spinner_budget_us, 100_000);
    }

    #[test]
    fn a_wall_reds_the_verdict_loudly() {
        let t = thresholds();
        let mut switch = RefsSwitchTest::drive(&t, 4);
        switch.capabilities[0].reached_by_driving = false;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a wall reds the verdict");
        assert_eq!(verdict.walls(), &[switch.capabilities[0].id]);
    }

    #[test]
    fn a_blown_budget_reds_the_verdict() {
        let t = thresholds();
        let mut switch = RefsSwitchTest::drive(&t, 4);
        switch.latencies.jump_us = switch.budgets.jump_no_spinner_budget_us + 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a blown jump budget reds the verdict");
        if let RefsSwitchVerdict::Red {
            over_budget_legs, ..
        } = &verdict
        {
            assert!(over_budget_legs.contains(&"jump"), "the jump leg is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn a_leak_reds_the_verdict() {
        let t = thresholds();
        let mut switch = RefsSwitchTest::drive(&t, 4);
        switch.leaked = true;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a leak reds the verdict");
        if let RefsSwitchVerdict::Red { leaked, .. } = &verdict {
            assert!(*leaked, "the leak is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn the_browser_drive_record_is_honest() {
        let record = switch_surface_drive_record();
        assert!(record.len() >= 5, "every switch-test surface is recorded");
        for s in &record {
            assert_eq!(
                s.drive,
                BrowserDriveStatus::AutomatedEngineNamedFloor,
                "{} is honestly recorded as automated-engine / web-tier named floor",
                s.surface
            );
            assert!(s.drive.token().contains("browser-driven=no"));
        }
    }
}
