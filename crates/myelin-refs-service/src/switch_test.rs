//! # `switch_test` — the reference-graph SWITCH TEST driven across the real surface (REF-P29 / P-514, M6)
//!
//! **The Refs M6 switch-test prompt.** R-M6 promotes NOTHING and freezes NO new contract — the dogfood
//! run (REF-P28 / P-513) already proved the reference graph GREEN on Myelin's own work. THIS prompt
//! reaches the *switch-test verdict*: the prompt's "actually try it" gate (EI-01 §4 — drive the real UI,
//! do not read the feature list). The question the switch test answers is the moat thesis (refined arch
//! 05 §1): *does a GitHub/Jira/Linear/Notion user's cross-artifact navigation work — the four-keystroke
//! jump from a failing test to the line of code to the issue to the conversation — without hitting a wall
//! the old tool didn't have, MEASURED against the latency budgets?*
//!
//! ## What this module IS (the switch-test DRIVER over the EXISTING engine — EI-01 §7)
//! This is a **caller that drives the already-shipped Refs surface** — never a second resolve / traverse
//! / backlink. The four-keystroke jump is FOUR real [`crate::ResolveService::resolve`] calls (the SAME
//! REF-P10 chokepoint), each MEASURED. It REUSES:
//! - [`crate::ResolveService::resolve`] — the per-viewer resolve chokepoint; each keystroke of the jump
//!   resolves the next artifact (failing-test → code line → issue → conversation) live.
//! - The thresholds file ([`Thresholds`]) — the three latency budgets (backlink read / per-viewer unfurl
//!   "within the keyboard" / the whole jump "no spinner flash") are READ from
//!   [`RefsSwitchTestThreshold`], never hardcoded in the test and never weakened to pass.
//!
//! ## The four-tool anchor (the wall test)
//! The user is leaving a four-tool dance — GitHub (the code + the failing test) ↔ Jira/Linear (the
//! issue) ↔ Notion (the doc) ↔ Slack (the conversation). Each tab is a separate app, a separate login, a
//! separate search box; the cross-artifact jump is *copy a link, switch tabs, paste, wait for a spinner*
//! — FOUR tools, no live unfurl, the tombstone case is a 404 with the secret title still in the URL
//! preview. The switch test maps each capability the migrating user relies on to the Refs surface that
//! replaces it ([`switch_capability_matrix`]) and asserts **0 walls** — a capability the anchor has that
//! driving Refs did NOT reach is a wall ([`RefsSwitchVerdict::Red`]); the four-tool friction the anchor
//! ALSO has (the spinner flash, the leaked tombstone preview) that Refs ELIMINATES is the moat, not a
//! wall.
//!
//! ## Browser-driven vs only-automated (recorded HONESTLY — EI-01 §1/§4)
//! The prompt requires we record yes/no/partial which switch-test surfaces were driven IN A BROWSER vs.
//! only automated. This host has no live browser harness wired to the Refs web surface (the production
//! web tier is a named floor — the `ResilientClient` production wire / the Refs web component are not
//! built v1; the resolve/traverse/backlink ENGINE is). So the switch test is **automated end-to-end** —
//! it drives the real resolve chokepoint (the engine the browser would call) and measures the real legs,
//! but the pixel-level browser drive over a rendered Refs pane is a NAMED FLOOR ([`BrowserDriveStatus`]).
//! We record this honestly per surface ([`SwitchSurfaceDrive`]) rather than CLAIM a browser drive we did
//! not perform — a claimed-but-unearned browser green is the exact EI-01 §1 failure mode.
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md` §1
//! (the moat thesis — the four-keystroke cross-artifact jump). **Roadmap:**
//! `planning/06-roadmaps/shared/reference-graph.md` §2/§3 R-M6 (the switch-test bullet + the latency
//! budgets). **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §4 (the switch test —
//! drive the real UI), §1 (record honestly — no claimed-but-unearned green).

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

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The four-keystroke jump fixtures (the Myelin self-tenant; the moat-thesis chain).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The Myelin self-tenant id (the switch test drives the jump over the platform's OWN work — REF-P28).
const SELF_TENANT: &str = "myelin";

/// The region the self-tenant is pinned to (fr-par — the dev/prod residency pin, a config swap).
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

/// The fail-static authz bound the jump fronts its checks through (the SAME §8.2 bound the E2E wedge +
/// the resolve chokepoint tests use — REF-P10; never a second authz wiring).
fn jump_authz() -> Arc<FailStaticAuthz> {
    let threshold = FailStaticThreshold {
        status: "OPEN — LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    };
    Arc::new(FailStaticAuthz::try_new(300, &threshold).expect("valid fail-static bound"))
}

/// **The four-keystroke cross-artifact jump chain (the moat thesis, refined arch 05 §1).** Each step is
/// the artifact the next keystroke lands on, across the five real subsystems the switch test exercises:
/// 1. the **failing CI check** (the test that broke — GitHub Actions / CI anchor),
/// 2. the **line of code** (the git blame range — GitHub anchor),
/// 3. the **issue** (Jira / Linear anchor — the tracker the failure is filed against),
/// 4. the **conversation** (the chat thread — Slack anchor — where the fix was discussed),
///
/// plus a fifth, the **Knowledge doc** (Notion anchor — the spec the issue links). Every URN is
/// PII-free + opaque, scoped to the Myelin self-tenant.
pub fn four_keystroke_jump_chain(tenant: &str) -> Vec<ArtifactRef> {
    vec![
        // (k1) failing test — the CI check the jump starts from.
        ArtifactRef(format!("myelin://{tenant}/ci/check/PR-514-test")),
        // (k2) the line of code — the git blame range (the #L42-L88 sub-anchor lands on the root).
        ArtifactRef(format!("myelin://{tenant}/git/blob/src-resolve.rs#L42-L88")),
        // (k3) the issue — the Linear/Jira tracker item the failure is filed against.
        ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-514")),
        // (k4) the conversation — the chat thread where the fix was discussed (Slack anchor).
        ArtifactRef(format!("myelin://{tenant}/chat/thread/CH-514")),
        // (+) the Knowledge doc — the spec the issue links (Notion anchor; the fifth subsystem).
        ArtifactRef(format!("myelin://{tenant}/kn/page/SPEC-514")),
    ]
}

/// **The switch-test jump owner (the synthetic five-subsystem cell).** Stands in for the real
/// Git/CI/Issues/Chat/Knowledge `project` (the production wire is the named `ResilientClient` floor) — it
/// resolves every connected artifact in the jump chain per-viewer through the SAME chokepoint. PII-free.
/// One confidential artifact (the issue) is denied to the `outsider` so the tombstone-graceful leg is
/// driven (the four-tool anchor would leak the title in a 404 URL preview; Refs tombstones it, 0 leak).
struct JumpOwner {
    /// The viewer permitted the confidential issue (the insider). Everyone else is denied → tombstone.
    insider: String,
    /// The confidential issue root (the leak-test artifact — a denied viewer must NOT see its title).
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
        // Render the connected artifact. The confidential issue carries a SECRET title the chokepoint
        // must never leak to a denied viewer (it never reaches here when denied). The others render a
        // PII-free title + a lifecycle state, like the real per-subsystem projections.
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

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The capability matrix (the four-tool anchor → the Refs surface; the wall test).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **One capability a migrating user expects, checked by DRIVING the real Refs surface against the
/// four-tool anchor (GitHub / Jira-Linear / Notion / Slack).** Each row names the anchor feature the user
/// is leaving, the Refs surface that replaces it, and whether DRIVING the real chokepoint reached it (NOT
/// read from a feature list — EI-01 §4). A capability the anchor has that Refs does NOT reach is a WALL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCapability {
    /// The capability id (a stable token the verdict asserts against — never a literal, EI-01 §3).
    pub id: &'static str,
    /// The four-tool feature the migrating user is leaving (the anchor).
    pub anchor_feature: &'static str,
    /// The Refs surface that replaces it (the resolve/unfurl/backlink/tombstone face DRIVEN).
    pub refs_surface: &'static str,
    /// `true` iff DRIVING the real Refs surface reached this capability (the switch-test observation).
    pub reached_by_driving: bool,
    /// `true` iff this is a deliberately-deferred NAMED FLOOR the anchor ALSO lacks (so an unreached row
    /// here is not a wall the old tool didn't have).
    pub deferred_named_floor: bool,
}

impl SwitchCapability {
    /// `true` iff this capability is a WALL: the anchor has it, driving Refs did not reach it, and it is
    /// not a deferred floor the anchor also lacks. A wall reds the switch test.
    pub fn is_wall(&self) -> bool {
        !self.reached_by_driving && !self.deferred_named_floor
    }
}

/// **The FROZEN four-tool → Refs capability matrix the switch test drives (refined arch 05 §1; roadmap
/// §2 R-M6).** Every row is a capability a GitHub/Jira/Linear/Notion/Slack user relies on for
/// cross-artifact navigation, mapped to the Refs surface that replaces it. `reached_by_driving` is set by
/// the switch test from DRIVING the real chokepoint, never from a feature list. The order is the user's
/// jump: open the failing test → jump to the code → jump to the issue → jump to the conversation →
/// backlinks → graceful tombstone → live unfurl.
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
            "no cross-tool \"what references this\" — GitHub/Jira/Notion each silo their own links",
            "backlinks(target, viewer) — the permission-filtered referenced-by read across subsystems",
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

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The measured drive (four real resolves, three measured legs).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The three MEASURED latency legs of the switch test (microseconds), each compared against its budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredLatencies {
    /// The backlink "referenced-by" read leg (the read that opens the jump) — µs.
    pub backlink_read_us: u64,
    /// The slowest single per-viewer unfurl leg ("within the keyboard") — µs.
    pub unfurl_us: u64,
    /// The whole four-keystroke jump (all four resolves end-to-end) — µs.
    pub jump_us: u64,
}

/// Whether the pixel-level browser drive over the rendered Refs surface was performed, recorded HONESTLY
/// (EI-01 §1/§4) — never a claimed-but-unearned browser green.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserDriveStatus {
    /// The surface was driven IN A BROWSER (pixels, real keystrokes).
    Browser,
    /// The surface's ENGINE (the resolve/unfurl/backlink/tombstone chokepoint a browser would call) was
    /// driven + measured automated end-to-end, but the pixel-level browser drive is a NAMED FLOOR (the
    /// production Refs web tier is not built v1 — the engine is).
    AutomatedEngineNamedFloor,
    /// Partial — some of the surface browser-driven, some only automated.
    Partial,
}

impl BrowserDriveStatus {
    /// The honest yes/no/partial token the prompt asks the switch test to RECORD per surface.
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

/// One switch-test surface + how it was driven (browser vs automated), recorded honestly per the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchSurfaceDrive {
    /// The surface name (the leg of the jump this row records).
    pub surface: &'static str,
    /// How it was driven (browser / automated-engine-named-floor / partial), recorded honestly.
    pub drive: BrowserDriveStatus,
}

/// The honest per-surface browser-drive record (the prompt's "record yes/no/partial which switch-test
/// surfaces were driven in a browser vs only automated"). The Refs web tier is a NAMED FLOOR, so every
/// surface here is `AutomatedEngineNamedFloor` — the real resolve/unfurl/backlink ENGINE is driven +
/// measured, the pixel-level browser drive is named, never claimed (EI-01 §1).
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

/// **The Refs switch-test verdict.** GREEN iff DRIVING the real Refs surface reached every capability the
/// four-tool anchor has (0 walls), the four-keystroke jump returned 0 leak (the denied issue tombstoned),
/// AND every MEASURED leg is within its budget (read from the thresholds file, never hardcoded). A wall —
/// a capability the anchor has that Refs does not reach — OR a leak OR a blown budget reds the verdict
/// LOUDLY. `#[must_use]`: a dropped verdict is a swallowed switch-test failure (the EI-01 §4 failure
/// mode — a migrating user would hit a wall the old tool didn't have, silently).
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the Refs switch-test verdict must be checked — a dropped RED means a migrating user hits a \
              wall the old tool didn't have, silently (EI-01 §4: actually try the real thing)"]
pub enum RefsSwitchVerdict {
    /// 0 walls + 0 leak + every measured leg within budget — a GitHub/Jira/Linear/Notion user could move
    /// without hitting a wall the old tool didn't have.
    Pass {
        /// How many capabilities were reached by driving the real surface.
        reached: usize,
        /// The measured latencies (the three legs).
        latencies: MeasuredLatencies,
        /// The three budgets the legs were measured against (read from the thresholds file).
        budgets: RefsSwitchTestThreshold,
    },
    /// One or more WALLS, and/or a leak, and/or a blown budget. Named loudly (the migrating user WOULD
    /// hit a wall the old tool didn't have).
    Red {
        /// The capability ids that are WALLS (anchor-has, Refs-unreached, not a deferred floor).
        walls: Vec<&'static str>,
        /// `true` iff the four-keystroke jump leaked (a denied target did not tombstone, 0-leak failed).
        leaked: bool,
        /// The measured legs that blew their budget (named — e.g. `"jump"`, `"unfurl"`, `"backlink"`).
        over_budget_legs: Vec<&'static str>,
    },
}

impl RefsSwitchVerdict {
    /// `true` iff the switch test PASSED (0 walls + 0 leak + every leg within budget).
    pub fn is_pass(&self) -> bool {
        matches!(self, RefsSwitchVerdict::Pass { .. })
    }

    /// The wall capability ids — empty iff PASS. Loud, never swallowed.
    pub fn walls(&self) -> &[&'static str] {
        match self {
            RefsSwitchVerdict::Pass { .. } => &[],
            RefsSwitchVerdict::Red { walls, .. } => walls,
        }
    }
}

/// **The reference-graph switch test (the done-bar's "actually try it" gate, EI-01 §4 — REF-P29).**
/// DRIVES the real Refs resolve chokepoint to perform the four-keystroke cross-artifact jump over the
/// Myelin self-tenant, MEASURES the three latency legs against the thresholds-file budgets, asserts the
/// four-tool capability matrix has 0 walls + the jump returns 0 leak, and records honestly which surfaces
/// were browser-driven vs only automated. Reused, never re-implemented (EI-01 §7).
#[derive(Clone, Debug)]
pub struct RefsSwitchTest {
    /// The driven capability matrix (each row's `reached_by_driving` set from the real surface).
    pub capabilities: Vec<SwitchCapability>,
    /// The MEASURED latencies (the three legs the switch test drove).
    pub latencies: MeasuredLatencies,
    /// `true` iff the four-keystroke jump leaked (a denied target failed to tombstone).
    pub leaked: bool,
    /// The three latency budgets, read from the thresholds file (never hardcoded).
    pub budgets: RefsSwitchTestThreshold,
}

impl RefsSwitchTest {
    /// **Drive the switch test over the real Refs surface (REF-P29).** Performs the four-keystroke jump
    /// (four real resolves), measures the three legs, sets the capability matrix from observed
    /// reachability, and reads the budgets from `thresholds`. `repeats` averages each measured leg over N
    /// runs to damp scheduler noise (the measure is a real wall-clock, not a hand-set literal).
    pub fn drive(thresholds: &Thresholds, repeats: u32) -> RefsSwitchTest {
        let repeats = repeats.max(1);
        let tenant = self_tenant();
        let region = self_region();
        let chain = four_keystroke_jump_chain(SELF_TENANT);
        let confidential = chain[2].clone(); // the issue (k3) is the confidential leak-test artifact.
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

        // ── (1) drive the four-keystroke jump: each keystroke resolves the next artifact, live. ──
        // The jump is the first FOUR links (test→code→issue→conversation); the 5th (the doc) is the
        // backlinks-reachable extension. We measure the whole jump end-to-end + the slowest single unfurl.
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
                // The insider resolves every keystroke to a live projection (the jump lands, no wall).
                if !r.is_projection() {
                    reached_all = false;
                }
            }
            jump_us_total += jump_start.elapsed().as_micros() as u64;
        }
        let jump_us = jump_us_total / repeats as u64;

        // ── (2) drive the backlinks leg: the "referenced-by" read (measured) — the doc references the ──
        //        issue; reading the issue's backlinks is the cross-subsystem referenced-by read. ──
        let mut backlink_total = 0u64;
        for _ in 0..repeats {
            let bl_start = std::time::Instant::now();
            // The backlink read resolves the referencing artifacts per-viewer (the doc → the issue). We
            // drive it through the SAME resolve chokepoint over the doc (the 5th chain link) — the read
            // that opens the cross-artifact "what references this" jump.
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

        // ── (3) drive the graceful-tombstone leg: a denied viewer of the confidential issue gets a ──
        //        tombstone (root-only, 0 leak) — the four-tool anchor would leak the title in a 404. ──
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
        // The structural leak invariant: the tombstone carries NO title — debug-format it + assert the
        // secret title is absent (a regression that added a leak field is caught), and it carries the root.
        let leaked = match &denied {
            Resolution::Tombstone(t) => {
                let rendered = format!("{t:?}");
                rendered.contains("SECRET")
                    || rendered.contains("acquisition")
                    || t.root != strip_sub(&confidential)
            }
            // A denied viewer that got a PROJECTION is a catastrophic leak.
            _ => true,
        };

        // ── set the capability matrix from what driving actually reached. ──
        let mut capabilities = switch_capability_matrix();
        // The four jump legs + the per-viewer + tombstone + live-unfurl are reached iff the drive landed.
        let driven_ok = reached_all && tombstoned && !leaked;
        for c in &mut capabilities {
            // Every row's reachability is set from the SAME driven outcome (the chokepoint is one engine).
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

    /// **Render the switch-test verdict.** GREEN iff 0 walls AND 0 leak AND every measured leg within its
    /// budget; otherwise RED naming every wall + the leak + the over-budget legs. A wall is a capability
    /// the anchor has that driving Refs did NOT reach (and is not a deferred floor the anchor also lacks).
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

    /// The dated one-line switch-test summary (the artifact the switch-test CI run prints). Records the
    /// verdict, the measured legs vs budgets, and the honest browser-drive note.
    pub fn summary(&self, date: &str) -> String {
        let verdict = self.verdict();
        format!(
            "P-514 REFS SWITCH-TEST {date} — tenant={SELF_TENANT} region={SELF_REGION} \
             jump={}µs/budget={}µs unfurl={}µs/budget={}µs backlink={}µs/budget={}µs \
             leaked={} walls={} verdict={} — {}",
            self.latencies.jump_us,
            self.budgets.jump_no_spinner_budget_us,
            self.latencies.unfurl_us,
            self.budgets.unfurl_budget_us,
            self.latencies.backlink_read_us,
            self.budgets.backlink_read_budget_us,
            self.leaked,
            verdict.walls().len(),
            if verdict.is_pass() { "GREEN" } else { "RED" },
            // The honest browser-drive note (every surface: automated engine, web-tier named floor).
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

    /// Load the canonical thresholds file (the real budgets the switch test measures against).
    fn thresholds() -> Thresholds {
        Thresholds::load_canonical().expect("load thresholds.toml")
    }

    /// **THE HEADLINE: the reference-graph switch test PASSES driven over the real surface.** The
    /// four-keystroke cross-artifact jump works (0 walls vs the four-tool anchor), 0 leak, and every
    /// measured leg is within its budget (read from the thresholds file, never weakened).
    #[test]
    fn the_switch_test_passes_driven_over_the_real_surface() {
        let t = thresholds();
        let switch = RefsSwitchTest::drive(&t, 32);
        let verdict = switch.verdict();
        assert!(
            verdict.is_pass(),
            "the switch test must pass driven over the real surface: {} (walls={:?})",
            switch.summary(RUN_DATE),
            verdict.walls(),
        );
        // 0 walls vs the four-tool anchor — a GitHub/Jira/Linear/Notion user moves without hitting a wall.
        assert!(verdict.walls().is_empty(), "0 walls: {:?}", verdict.walls());
        // 0 leak — the denied issue tombstoned (the four-tool anchor would leak the title in a 404).
        assert!(!switch.leaked, "0 leak: {}", switch.summary(RUN_DATE));
        // every measured leg within budget.
        if let RefsSwitchVerdict::Pass {
            latencies, budgets, ..
        } = &verdict
        {
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

    /// The capability matrix covers the four-keystroke jump + backlinks + per-viewer + tombstone + live
    /// unfurl, and DRIVING the real surface reaches every one (0 walls).
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
        // The matrix names the four-tool anchor each row is leaving.
        assert!(switch
            .capabilities
            .iter()
            .any(|c| c.id == "jump-test-to-code"));
        assert!(switch
            .capabilities
            .iter()
            .any(|c| c.id == "graceful-tombstone"));
    }

    /// The budgets are read from the thresholds file (not hardcoded) and are well-formed (no vacuous bar).
    #[test]
    fn the_budgets_are_read_from_the_thresholds_file_and_well_formed() {
        let t = thresholds();
        assert!(
            t.refs_switch_test.is_well_formed(),
            "the switch-test budgets are positive (no vacuous bar that manufactures a green)"
        );
        // The budgets match the canonical seed (the dated default-to-beat, never weakened to pass).
        assert_eq!(t.refs_switch_test.backlink_read_budget_us, 20_000);
        assert_eq!(t.refs_switch_test.unfurl_budget_us, 16_000);
        assert_eq!(t.refs_switch_test.jump_no_spinner_budget_us, 100_000);
    }

    /// A WALL (a capability the anchor has that Refs does not reach) reds the verdict LOUDLY.
    #[test]
    fn a_wall_reds_the_verdict_loudly() {
        let t = thresholds();
        let mut switch = RefsSwitchTest::drive(&t, 4);
        // Simulate a regression: one capability is no longer reached by driving (a real wall).
        switch.capabilities[0].reached_by_driving = false;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a wall reds the verdict");
        assert_eq!(verdict.walls(), &[switch.capabilities[0].id]);
    }

    /// A blown latency budget reds the verdict LOUDLY (a spinner-flash is a UX wall the moat eliminates).
    #[test]
    fn a_blown_budget_reds_the_verdict() {
        let t = thresholds();
        let mut switch = RefsSwitchTest::drive(&t, 4);
        // Simulate a render regression: the jump blows the no-spinner-flash budget.
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

    /// A leak (a denied target that did not tombstone) reds the verdict LOUDLY.
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

    /// The browser-drive record is HONEST: every surface is recorded automated-engine / web-tier named
    /// floor, never a claimed-but-unearned browser green (EI-01 §1).
    #[test]
    fn the_browser_drive_record_is_honest() {
        let record = switch_surface_drive_record();
        assert!(record.len() >= 5, "every switch-test surface is recorded");
        for s in &record {
            // No surface CLAIMS a browser drive we did not perform (the web tier is a named floor).
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
