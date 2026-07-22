//! # The Git Web UI view-model render layer (GIT-P32 / P-293, M3-G8 — FIRST USEFUL)
//!
//! The **server-rendered view-model + HTML render** for the Git Web UI's load-bearing surfaces,
//! built TO the signed-off design pass
//! (`planning/04-subsystem-architectures/git-hosting/design/design-system-pass.md`, the GIT-P7
//! sign-off in `signoff.md`) and conforming to the frozen design system
//! (`design-planning/08-design-system/DESIGN-MANUAL.md`, direction A "Instrument").
//!
//! ## Why this layer is a view-MODEL, not a frontend framework
//! The platform's frontend stack is **TS + React + React Aria** (DESIGN-MANUAL §8.2), built against
//! the generated `tokens.css`. That React app is a separate deliverable; what GIT-P32 ships in the
//! `myelin-git` crate is the **Rust view-model + an HTML projection of it** — the same Rust logic the
//! WASM-Rust edges (DESIGN-MANUAL §8.2: "WASM-Rust at the edges that earn it") share with the server,
//! rendered to real, browseable HTML that:
//! - consumes **only semantic tokens** (`--surface`, `--text-primary`, `--success`, `--focus-ring`,
//!   …) via CSS classes — never a primitive, never an inline colour on an interactive element
//!   (the inline-colour ban, design pass §1);
//! - carries **status as glyph + label + position, never colour alone** (WCAG 1.4.1, design pass §3);
//! - renders the **fork-trust badge** (the security-critical, signed-off X-1 affordance — a fork's
//!   own green NEVER reads as gating-green; design pass §4.1);
//! - renders every **unglamorous state** (empty / loading-skeleton / error / permission-denied /
//!   erased / agent-pending — design pass §5; DESIGN-MANUAL §5.3 "never blank, never blame, never
//!   leak, never lie");
//! - is **driven in a real browser** (chromium headless) by the GIT-P32 e2e walkthrough
//!   (`tests/e2e_git_p32_web_browser.rs`, EI-01 §4 — the switch-test rehearsal), with each surface's
//!   states recorded.
//!
//! The view-model reads the **already-built** projections — there is **NO new contract** (the prompt
//! states this): [`crate::project::Projected`] (the per-viewer 0-leak projection / tombstone),
//! [`crate::check_status`] (`CheckStatusRow` / `CheckState` / `TrustTier`), [`crate::merge_gate`]
//! (`MergeGateOutcome` / `UnmetContext`), and [`crate::lifecycle`] (`PrState`). The Web UI is a
//! consumer of these — it renders the real projection, never a parallel vocabulary (design pass §0).
//!
//! ## Floor named (GF-6)
//! **Single-file web edit** ([`WebEditForm`]) — a v1 single-file edit + commit surface. The in-browser
//! **3-way conflict editor** is the named follow-on **GIT-P33 / M5+** (design pass §4 / view doc §2.2:
//! "no 3-way conflict editor in v1"). The web-edit commit path lowers to the SAME receive-pack one-tx
//! ref-CAS the rest of the platform uses (it is not a parallel write path); v1 simply REFUSES on a
//! stale-base conflict with an honest message rather than offering a merge editor.

use crate::check_status::{CheckState, CheckStatusRow, TrustTier};
use crate::lifecycle::PrState;
use crate::merge_gate::{MergeGateOutcome, UnmetContext, UnmetReason};
use crate::project::{ChecksSummary, Projected, RenderHint};
use base64::Engine as _;
use serde_json::{json, Value};

/// Minimal HTML-escape for text interpolated into the rendered view-model. The view-model never
/// renders attacker-controlled bytes raw (no XSS surface), and a tombstone NEVER reaches this function
/// with a title (the 0-leak invariant is upstream in [`Projected`]).
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The semantic status token a surface binds to (design pass §1 — `success`/`danger`/`warning`/`info`
/// /`text-muted`/`agent`). **Never rendered as colour alone** — every use is paired with a glyph + a
/// text label ([`StatusCue`]). This is the Rust-side enum the CSS class name is derived from; it never
/// carries a hex (the inline-colour ban).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusToken {
    /// `--success` — check passed / PR merged / approved.
    Success,
    /// `--danger` — check failed / merge blocked / request-changes.
    Danger,
    /// `--warning` — running/queued/awaiting, and the un-endorsed-fork badge.
    Warning,
    /// `--info` — queued/in-progress informational.
    Info,
    /// `--text-muted` — neutral / recorded-but-not-gating.
    Muted,
    /// `--agent` — agent-attributed content (the reserved fourth neutral axis).
    Agent,
}

impl StatusToken {
    /// The CSS class the rendered HTML carries (the semantic-token-driven class, never an inline hex).
    /// The shipped stylesheet ([`STYLE`]) binds each class to its `var(--…)` semantic token.
    pub fn css_class(self) -> &'static str {
        match self {
            StatusToken::Success => "st-success",
            StatusToken::Danger => "st-danger",
            StatusToken::Warning => "st-warning",
            StatusToken::Info => "st-info",
            StatusToken::Muted => "st-muted",
            StatusToken::Agent => "st-agent",
        }
    }

    /// The semantic-token NAME (the DATA channel the JSON projection carries — `success`/`danger`/…).
    /// The HTML render binds the CSS class ([`StatusToken::css_class`]); the edge JSON contract
    /// ([`StatusCue::to_json`]) carries this bare semantic name so a client renders its own treatment.
    pub fn name(self) -> &'static str {
        match self {
            StatusToken::Success => "success",
            StatusToken::Danger => "danger",
            StatusToken::Warning => "warning",
            StatusToken::Info => "info",
            StatusToken::Muted => "muted",
            StatusToken::Agent => "agent",
        }
    }
}

/// A status CUE — the **glyph + label** pair every status renders (never colour alone, WCAG 1.4.1 /
/// design pass §3). The glyph is an ASCII stand-in for the icon-set role glyph (the real frontend binds
/// the `<svg>` by registry name; the role mapping is the design pass §3 glyph map). The label is the
/// human-readable word; together with the `token`'s colour they form the three-channel status signal
/// (glyph + label + colour) — any two of which suffice for a colour-blind / monochrome viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusCue {
    /// The semantic token (the colour channel).
    pub token: StatusToken,
    /// The role-glyph ASCII stand-in (the shape channel — `check-mark`/`x-mark`/`alert-triangle`/…).
    pub glyph: &'static str,
    /// The text label (the language channel — always present, never colour-alone).
    pub label: &'static str,
}

impl StatusCue {
    /// The status cue for a [`CheckState`] (design pass §3 glyph map): `success`→check-mark/`success`,
    /// `failure`→x-mark/`danger`, `error`/`cancelled`→alert-triangle/`warning` (DISTINCT from failure),
    /// `queued`/`in_progress`→clock/`info`, `neutral`→dash-circle/`text-muted` (recorded, never gating).
    pub fn for_check_state(state: CheckState) -> StatusCue {
        match state {
            CheckState::Success => StatusCue {
                token: StatusToken::Success,
                glyph: "\u{2714}",
                label: "passed",
            },
            CheckState::Failure => StatusCue {
                token: StatusToken::Danger,
                glyph: "\u{2717}",
                label: "failed",
            },
            // error/cancelled are visually DISTINCT from failure (design pass §4.2) — an infra error
            // is not a test failure and must not read as one.
            CheckState::Error => StatusCue {
                token: StatusToken::Warning,
                glyph: "\u{26A0}",
                label: "error",
            },
            CheckState::Cancelled => StatusCue {
                token: StatusToken::Warning,
                glyph: "\u{2298}",
                label: "cancelled",
            },
            CheckState::Queued => StatusCue {
                token: StatusToken::Info,
                glyph: "\u{25F4}",
                label: "queued",
            },
            CheckState::InProgress => StatusCue {
                token: StatusToken::Info,
                glyph: "\u{27F3}",
                label: "running",
            },
            CheckState::Neutral => StatusCue {
                token: StatusToken::Muted,
                glyph: "\u{2296}",
                label: "neutral",
            },
        }
    }
}

// ---------------------------------------------------------------------------
// The fork-trust badge (design pass §4.1 — the security-critical, SIGNED-OFF affordance)
// ---------------------------------------------------------------------------

/// The **fork-trust badge view-model** (design pass §4.1, the GIT-P7 signed-off X-1 affordance). The
/// poisoned-pipeline defence made visible: a check whose `trust_tier = untrusted_fork` is **recorded
/// but NEUTRAL for gating** until a maintainer endorses it (`fork_endorsed = true` via
/// `check(subject, approve_untrusted_ci, repo)`) or it is re-run trusted.
///
/// **The load-bearing security invariant (signed off):** a fork's own green must NEVER read as
/// gating-green. [`ForkTrustBadge::for_row`] returns `Some` ONLY for an un-endorsed `untrusted_fork`
/// row — exactly the case the badge exists to warn about — and renders `warning` + a shield-question
/// glyph + the EXPLICIT words "untrusted fork / neutral until trusted". The `[ Trust this run ]` action
/// is gated on `approve_untrusted_ci` and is **absent** (read-only) for a viewer without the
/// permission (no leaked affordance).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkTrustBadge {
    /// `true` when the current viewer holds `approve_untrusted_ci` — the ONLY case the `[ Trust this
    /// run ]` action renders. A viewer without it sees the honest badge but no action (no leak).
    pub viewer_may_endorse: bool,
}

impl ForkTrustBadge {
    /// The fork-trust badge for a projection row, given the viewer's `approve_untrusted_ci` capability
    /// and whether this context is already endorsed. Returns `Some(badge)` ONLY for an un-endorsed
    /// `untrusted_fork` row (the case the badge exists for); `None` for a trusted row OR an
    /// already-endorsed fork row (which has flipped to the shield-check/`success` state and counts
    /// toward the gate — design pass §4.1).
    pub fn for_row(
        row: &CheckStatusRow,
        viewer_may_endorse: bool,
        endorsed: bool,
    ) -> Option<ForkTrustBadge> {
        match row.trust_tier {
            TrustTier::Trusted => None,
            TrustTier::UntrustedFork if endorsed => None,
            TrustTier::UntrustedFork => Some(ForkTrustBadge { viewer_may_endorse }),
        }
    }

    /// Render the badge to its design-pass §4.1 HTML treatment — `warning` token + shield-question
    /// glyph + the explicit honest copy. The `[ Trust this run ]` button renders IFF the viewer may
    /// endorse (gated, never a leaked affordance).
    pub fn render(&self) -> String {
        let mut h = String::new();
        h.push_str(&format!(
            "<div class=\"fork-trust-badge {}\" role=\"note\">",
            StatusToken::Warning.css_class()
        ));
        // glyph + explicit label (never colour alone): shield-question + the words.
        h.push_str(
            "<span class=\"badge-line\"><span class=\"glyph\" aria-hidden=\"true\">\u{26A8}</span>\
             <strong>passed on a FORK run \u{2014} neutral until trusted</strong></span>",
        );
        h.push_str(
            "<p class=\"badge-explain\">This run executed code from an untrusted fork. \
             It does NOT satisfy the gate by itself. A maintainer must review and trust it.</p>",
        );
        if self.viewer_may_endorse {
            // The action is permission-gated; only rendered for a viewer with approve_untrusted_ci.
            h.push_str(
                "<div class=\"badge-actions\">\
                 <button class=\"btn btn-primary\" data-action=\"endorse-fork-ci\">Trust this run</button>\
                 <button class=\"btn\" data-action=\"rerun-trusted\">Re-run</button>\
                 </div>",
            );
        }
        h.push_str("</div>");
        h
    }
}

// ---------------------------------------------------------------------------
// The checks panel (design pass §4.2 — the X-1 consumer surface)
// ---------------------------------------------------------------------------

/// One rendered checks-panel row view-model (design pass §4.2). One row per `(commit_oid, context)`.
/// The humanised `summary` is carried as a pre-humanised string (the `HumanisedRef` is resolved by
/// Notif at the backend — the frontend owns NO humanisation map, design pass §5; the view-model takes
/// the already-humanised text so the panel never renders a raw CI string).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckRowView {
    /// The context label (`ci/build`, `ci/test`, …) — rendered monospace (load-bearing mono, §3.2).
    pub context: String,
    /// The status cue (glyph + label + token) for the row's [`CheckState`].
    pub cue: StatusCue,
    /// `true` iff Git's branch-protection policy gates on this context (Git decides, not CI — X-1).
    pub required: bool,
    /// The Notif-humanised summary (already humanised at the backend — never a raw CI string).
    pub summary: String,
    /// The fork-trust badge IFF this is an un-endorsed `untrusted_fork` row (design pass §4.1).
    pub fork_badge: Option<ForkTrustBadge>,
}

impl CheckRowView {
    /// Build a checks-panel row view-model from a projection row + the humanised summary + the viewer's
    /// endorse capability + whether the context is endorsed.
    pub fn from_row(
        row: &CheckStatusRow,
        humanised_summary: impl Into<String>,
        required: bool,
        viewer_may_endorse: bool,
        endorsed: bool,
    ) -> CheckRowView {
        CheckRowView {
            context: format!("{}/{}", provider_label(row), row.context.name),
            cue: StatusCue::for_check_state(row.state),
            required,
            summary: humanised_summary.into(),
            fork_badge: ForkTrustBadge::for_row(row, viewer_may_endorse, endorsed),
        }
    }

    /// Render the row to design-pass §4.2 HTML — glyph + label + required? + humanised summary, with
    /// the fork-trust badge inline beneath an un-endorsed fork row.
    pub fn render(&self) -> String {
        let mut h = String::new();
        h.push_str("<li class=\"check-row\">");
        h.push_str(&format!(
            "<span class=\"check-status {}\"><span class=\"glyph\" aria-hidden=\"true\">{}</span>\
             <span class=\"label\">{}</span></span>",
            self.cue.token.css_class(),
            self.cue.glyph,
            escape(self.cue.label),
        ));
        h.push_str(&format!(
            "<code class=\"check-context\">{}</code>",
            escape(&self.context)
        ));
        h.push_str(&format!(
            "<span class=\"check-required\">{}</span>",
            if self.required {
                "required"
            } else {
                "optional"
            }
        ));
        h.push_str(&format!(
            "<span class=\"check-summary\">{}</span>",
            escape(&self.summary)
        ));
        if let Some(badge) = &self.fork_badge {
            h.push_str(&badge.render());
        }
        h.push_str("</li>");
        h
    }
}

fn provider_label(row: &CheckStatusRow) -> &'static str {
    use crate::check_status::CheckProvider;
    match row.context.provider {
        CheckProvider::Ci => "ci",
        CheckProvider::External => "ext",
    }
}

/// The **checks-panel view-model** (design pass §4.2) with its full state coverage. Renders the live
/// per-context rows OR one of the unglamorous states (empty / loading-skeleton / error). The panel
/// **fails static for ITS surface only** (the rest of the PR renders) — design pass §4.2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChecksPanel {
    /// The live rows (≥ 1). The happy path + the per-row fork-trust badges.
    Live {
        /// One row per `(commit_oid, context)`.
        rows: Vec<CheckRowView>,
    },
    /// No checks configured for this branch — the empty state (onboarding-forward, design pass §4.2).
    Empty,
    /// Checks queued — the loading state (a SKELETON matching the final layout, never a blank spinner;
    /// `n` skeleton rows).
    Loading {
        /// How many skeleton rows to render (matches the expected final layout).
        skeleton_rows: usize,
    },
    /// The panel itself is unavailable — fail-static FOR THIS SURFACE ONLY (the rest of the PR renders).
    Error,
}

impl ChecksPanel {
    /// Render the panel to design-pass §4.2 HTML, covering its state.
    pub fn render(&self) -> String {
        let mut h = String::new();
        h.push_str("<section class=\"checks-panel\" aria-label=\"Checks\">");
        h.push_str("<h3 class=\"panel-title\">Checks</h3>");
        match self {
            ChecksPanel::Live { rows } => {
                h.push_str("<ul class=\"check-rows\">");
                for r in rows {
                    h.push_str(&r.render());
                }
                h.push_str("</ul>");
            }
            ChecksPanel::Empty => {
                // empty = onboarding-forward (next action front-and-centre).
                h.push_str(
                    "<div class=\"state-empty\"><p>No checks configured for this branch.</p>\
                     <a class=\"btn\" href=\"settings/rulesets\">Configure required checks</a></div>",
                );
            }
            ChecksPanel::Loading { skeleton_rows } => {
                // loading = structure-skeleton, aria-busy + a polite live region (DESIGN-MANUAL §6).
                h.push_str("<div class=\"state-loading\" aria-busy=\"true\">");
                for _ in 0..*skeleton_rows {
                    h.push_str("<div class=\"skeleton-row\" aria-hidden=\"true\"></div>");
                }
                h.push_str(
                    "<span class=\"sr-only\" role=\"status\" aria-live=\"polite\">Checks queued\u{2026}</span>",
                );
                h.push_str("</div>");
            }
            ChecksPanel::Error => {
                // error = blame the SYSTEM in one quiet line + a path (retry), scoped to this surface.
                h.push_str(
                    "<div class=\"state-error\" role=\"alert\">\
                     <p>Checks unavailable.</p>\
                     <button class=\"btn\" data-action=\"retry-checks\">Retry</button></div>",
                );
            }
        }
        h.push_str("</section>");
        h
    }
}

// ---------------------------------------------------------------------------
// The merge-readiness affordance (design pass §4.3)
// ---------------------------------------------------------------------------

/// The **merge-readiness view-model** (design pass §4.3). The merge UX driven by the durable
/// `ci.result` wait — names WHICH context is unmet (humanised, never a bare "blocked"), shows the
/// "queued → testing → merged" lifecycle, and the multi-day HITL hold (the workflow holds no runtime
/// while it waits). Mirrors [`MergeGateOutcome`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeReadiness {
    /// Every required check green + approvals satisfied — the merge may proceed.
    Ready {
        /// `(current, required)` approvals.
        approvals: (u32, u32),
    },
    /// Blocked — the specific unmet contexts (humanised) + the next action.
    Blocked {
        /// The unmet contexts (from [`MergeGateOutcome::Blocked`]).
        unmet: Vec<UnmetContext>,
    },
    /// In a durable merge queue — position + the "queued → testing → merged" lifecycle.
    Queued {
        /// 1-based position in the queue.
        position: usize,
    },
    /// On a multi-day HITL hold (the durable gate is awaiting a human approval; no runtime held).
    HitlHold {
        /// The humanised "awaiting approval from @maintainer (held Nd)" text (humanised at backend).
        awaiting: String,
    },
}

impl MergeReadiness {
    /// Build a merge-readiness view-model from the merge-gate outcome (the blocked path) or a ready
    /// signal. A `Blocked` gate maps to the humanised unmet list (design pass §4.3 — names WHICH gate).
    pub fn from_gate(outcome: &MergeGateOutcome, approvals: (u32, u32)) -> MergeReadiness {
        match outcome {
            MergeGateOutcome::Admitted => MergeReadiness::Ready { approvals },
            MergeGateOutcome::Blocked { unmet } => MergeReadiness::Blocked {
                unmet: unmet.clone(),
            },
        }
    }

    /// Render to design-pass §4.3 HTML.
    pub fn render(&self) -> String {
        let mut h = String::new();
        h.push_str("<section class=\"merge-readiness\" aria-label=\"Merge readiness\">");
        h.push_str("<h3 class=\"panel-title\">Merge readiness</h3>");
        match self {
            MergeReadiness::Ready { approvals } => {
                h.push_str(&format!(
                    "<p class=\"ready {}\"><span class=\"glyph\" aria-hidden=\"true\">\u{2714}</span>\
                     All required checks green \u{00B7} {}/{} approvals \u{00B7} threads resolved</p>",
                    StatusToken::Success.css_class(),
                    approvals.0,
                    approvals.1,
                ));
                h.push_str(
                    "<div class=\"merge-actions\">\
                     <button class=\"btn btn-primary\" data-action=\"merge\">Merge</button>\
                     <button class=\"btn\" data-action=\"auto-merge\">Enable auto-merge when green</button>\
                     </div>",
                );
            }
            MergeReadiness::Blocked { unmet } => {
                // Names WHICH context is unmet, humanised — never a bare "blocked".
                let reasons: Vec<String> = unmet.iter().map(humanise_unmet).collect();
                h.push_str(&format!(
                    "<p class=\"blocked {}\"><span class=\"glyph\" aria-hidden=\"true\">\u{26A0}</span>\
                     Blocked: {}</p>",
                    StatusToken::Warning.css_class(),
                    escape(&reasons.join(" \u{00B7} ")),
                ));
            }
            MergeReadiness::Queued { position } => {
                h.push_str(&format!(
                    "<p class=\"queued {}\"><span class=\"glyph\" aria-hidden=\"true\">\u{27F3}</span>\
                     Queued (position {}) \u{2192} testing \u{2192} merged</p>",
                    StatusToken::Info.css_class(),
                    position,
                ));
            }
            MergeReadiness::HitlHold { awaiting } => {
                // The agent-pending / waiting state applied to the merge queue (no runtime held).
                h.push_str(&format!(
                    "<p class=\"hitl-hold {}\"><span class=\"glyph\" aria-hidden=\"true\">\u{27F3}</span>\
                     {}</p>",
                    StatusToken::Warning.css_class(),
                    escape(awaiting),
                ));
            }
        }
        h.push_str("</section>");
        h
    }
}

/// Humanise an unmet context into the design-pass §4.3 "WHICH gate is unmet" text (never a bare
/// "blocked"; never a raw CI string). The context NAME + the typed reason become the human line.
fn humanise_unmet(u: &UnmetContext) -> String {
    let ctx = &u.context.name;
    match &u.reason {
        UnmetReason::Missing => format!("{ctx} not reported"),
        UnmetReason::NotGreen { state } => {
            let cue = StatusCue::for_check_state(*state);
            format!("{ctx} {}", cue.label)
        }
        UnmetReason::UntrustedForkNeutral => format!("{ctx} awaiting fork trust"),
    }
}

// ---------------------------------------------------------------------------
// The PR overview (design pass §2.2 — the centrepiece) — built on the per-viewer projection
// ---------------------------------------------------------------------------

/// The **PR overview page view-model** (architecture view doc §2.2 — the centrepiece). Built on the
/// per-viewer [`Projected`] (0-leak: a denied viewer gets a tombstone, NEVER the title) + the checks
/// panel + merge readiness. The page renders the projection's title/state ONLY when visible; a
/// tombstone renders the dignified permission/erased state (design pass §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrOverviewPage {
    /// The per-viewer projection of the PR (visible projection OR tombstone — the 0-leak boundary).
    pub projected: Projected,
    /// The PR lifecycle state (for the state pill). Only meaningful when `projected` is visible.
    pub pr_state: PrState,
    /// The checks panel (the X-1 consumer surface).
    pub checks: ChecksPanel,
    /// The merge-readiness affordance.
    pub merge: MergeReadiness,
}

impl PrOverviewPage {
    /// Render the full PR overview page to HTML. A tombstone short-circuits to the dignified, content-
    /// free permission/erased state (the title NEVER reaches the render — the 0-leak invariant).
    pub fn render(&self) -> String {
        let mut h = String::new();
        h.push_str("<main class=\"pr-overview\">");
        match &self.projected {
            Projected::Tombstoned(_t) => {
                // permission-denied / erased: dignified, content-free, no leaked title (design pass §5;
                // no-access is deliberately indistinguishable from erased to an unauthorised viewer).
                h.push_str(
                    "<div class=\"state-restricted\" role=\"note\">\
                     <span class=\"glyph\" aria-hidden=\"true\">\u{1F512}</span>\
                     <p>This pull request is not available to you.</p></div>",
                );
            }
            Projected::Visible(p) => {
                h.push_str(&format!("<h1 class=\"pr-title\">{}</h1>", escape(&p.title)));
                h.push_str(&format!(
                    "<span class=\"pr-state-pill {}\">{}</span>",
                    pr_state_token(self.pr_state).css_class(),
                    pr_state_label(self.pr_state),
                ));
                if let Some(hint) = &p.render_hint {
                    h.push_str(&render_pr_hint(hint));
                }
                h.push_str(&self.checks.render());
                h.push_str(&self.merge.render());
            }
        }
        h.push_str("</main>");
        h
    }
}

/// **A representative PR overview page for the GIT-P35 switch test** (the view-model the switch test
/// renders + measures, [`crate::switch_test`]). Assembles a real visible [`PrOverviewPage`] — a visible
/// projection (title + state pill + checks-green render hint) + a live checks panel + a ready
/// merge-readiness affordance — so the switch test's render leg exercises the SAME GIT-P32 assembly +
/// render path (EI-01 §7, never a second renderer). `tenant` frames the page for the self-tenant. The
/// page is PII-free (opaque ids only).
pub fn switch_test_representative_pr_page(tenant: &str) -> PrOverviewPage {
    use crate::check_status::{CheckContext, CheckStatus, GitOid, HumanisedRef, Timestamp};
    use myelin_tenancy::{ArtifactRef, TenantId};
    let fact = CheckStatus {
        tenant: TenantId(tenant.into()),
        repo: ArtifactRef(format!("myelin://{tenant}/git/repo/myelin")),
        commit_oid: GitOid("blake3:switchtesthead".into()),
        context: CheckContext::ci("build"),
        state: CheckState::Success,
        required: true,
        run: ArtifactRef(format!("myelin://{tenant}/ci/run/1")),
        run_attempt: 1,
        trust_tier: TrustTier::Trusted,
        details_ref: ArtifactRef(format!("myelin://{tenant}/ci/run/1#step-1")),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args: std::collections::BTreeMap::new(),
        },
        started_at: Timestamp("2026-06-26T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-26T00:01:00Z".into())),
        cost_settled: true,
    };
    let row = CheckStatusRow::from_fact(&fact);
    PrOverviewPage {
        projected: Projected::Visible(crate::project::Projection {
            title: format!("[{tenant}] Restore-verify gate at cell scale"),
            state: "open".into(),
            icon: "pr".into(),
            render_hint: Some(RenderHint {
                checks: ChecksSummary::Green,
                approvals: (2, 2),
                is_draft: false,
            }),
            sub_anchor: None,
        }),
        pr_state: PrState::Open,
        checks: ChecksPanel::Live {
            rows: vec![CheckRowView::from_row(
                &row,
                "build passed",
                true,
                false,
                false,
            )],
        },
        merge: MergeReadiness::from_gate(&MergeGateOutcome::Admitted, (2, 2)),
    }
}

fn render_pr_hint(hint: &RenderHint) -> String {
    let cue = match hint.checks {
        ChecksSummary::Green => StatusCue {
            token: StatusToken::Success,
            glyph: "\u{2714}",
            label: "checks green",
        },
        ChecksSummary::Red => StatusCue {
            token: StatusToken::Danger,
            glyph: "\u{2717}",
            label: "checks blocked",
        },
        ChecksSummary::Neutral => StatusCue {
            token: StatusToken::Muted,
            glyph: "\u{2296}",
            label: "no required checks",
        },
    };
    format!(
        "<div class=\"pr-hint\"><span class=\"check-status {}\">\
         <span class=\"glyph\" aria-hidden=\"true\">{}</span>\
         <span class=\"label\">{}</span></span>\
         <span class=\"approvals\">{}/{} approvals</span>{}</div>",
        cue.token.css_class(),
        cue.glyph,
        escape(cue.label),
        hint.approvals.0,
        hint.approvals.1,
        if hint.is_draft {
            "<span class=\"draft-pill\">draft</span>"
        } else {
            ""
        },
    )
}

fn pr_state_token(s: PrState) -> StatusToken {
    match s {
        PrState::Merged => StatusToken::Success,
        PrState::Open => StatusToken::Info,
        PrState::Draft => StatusToken::Muted,
        PrState::Closed => StatusToken::Danger,
    }
}

fn pr_state_label(s: PrState) -> &'static str {
    match s {
        PrState::Draft => "draft",
        PrState::Open => "open",
        PrState::Merged => "merged",
        PrState::Closed => "closed",
    }
}

// ---------------------------------------------------------------------------
// Repo home + file view (architecture view doc §2.1)
// ---------------------------------------------------------------------------

/// Maximum UTF-8 bytes accepted for a repository-list row slug.
pub const REPO_LIST_ROW_MAX_SLUG_BYTES: usize = 255;
/// Maximum UTF-8 bytes accepted for a repository-list row clone URL.
pub const REPO_LIST_ROW_MAX_CLONE_URL_BYTES: usize = 4 * 1024;
/// Prefix for the versioned repository-list continuation token.
pub const REPO_LIST_CURSOR_PREFIX: &str = "rl1_";
/// Maximum encoded repository-list continuation-token bytes.
pub const REPO_LIST_CURSOR_MAX_BYTES: usize = 512;
/// Prefix for the versioned pull-request commit continuation token.
pub const PR_COMMIT_CURSOR_PREFIX: &str = "pc1_";
/// Maximum encoded pull-request commit continuation-token bytes.
pub const PR_COMMIT_CURSOR_MAX_BYTES: usize = 256;
/// Deepest continuation position accepted by the pull-request commit walker.
pub const PR_COMMIT_CURSOR_MAX_POSITION: usize = crate::durable::PR_COMMIT_MAX_POSITION;

const REPO_LIST_CURSOR_VERSION: u8 = 1;
const REPO_LIST_CURSOR_FIXED_BYTES: usize = 1 + 32 + 2;
const PR_COMMIT_CURSOR_VERSION: u8 = 1;
const PR_COMMIT_CURSOR_FRAME_BYTES: usize = 1 + 32 + 1 + 20 + 20 + 4;

/// A malformed or non-canonical repository-list continuation token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepoListCursorError;

impl std::fmt::Display for RepoListCursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("repository-list cursor is malformed")
    }
}

impl std::error::Error for RepoListCursorError {}

/// Canonical continuation state for the lightweight repository list. The opaque scope is owned by
/// the transport; the cursor only carries it alongside the last visible bare slug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoListCursor {
    scope: [u8; 32],
    last_slug: String,
}

impl RepoListCursor {
    /// Construct a cursor from an opaque transport scope and a validated bare repository slug.
    pub fn new(scope: [u8; 32], last_slug: impl Into<String>) -> Result<Self, RepoListCursorError> {
        let last_slug = last_slug.into();
        if !valid_repo_list_cursor_slug(&last_slug) {
            return Err(RepoListCursorError);
        }
        Ok(Self { scope, last_slug })
    }

    /// Parse the exact versioned, unpadded base64url token representation.
    pub fn parse(value: &str) -> Result<Self, RepoListCursorError> {
        let encoded = value
            .strip_prefix(REPO_LIST_CURSOR_PREFIX)
            .ok_or(RepoListCursorError)?;
        if encoded.is_empty() || value.len() > REPO_LIST_CURSOR_MAX_BYTES {
            return Err(RepoListCursorError);
        }
        let frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| RepoListCursorError)?;
        if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&frame) != encoded
            || frame.len() < REPO_LIST_CURSOR_FIXED_BYTES
            || frame[0] != REPO_LIST_CURSOR_VERSION
        {
            return Err(RepoListCursorError);
        }
        let mut scope = [0_u8; 32];
        scope.copy_from_slice(&frame[1..33]);
        let slug_len = usize::from(u16::from_be_bytes([frame[33], frame[34]]));
        if slug_len == 0 || frame.len() != REPO_LIST_CURSOR_FIXED_BYTES + slug_len {
            return Err(RepoListCursorError);
        }
        let last_slug = std::str::from_utf8(&frame[35..])
            .map_err(|_| RepoListCursorError)?
            .to_string();
        Self::new(scope, last_slug)
    }

    /// Render the canonical versioned, unpadded base64url token.
    pub fn encode(&self) -> String {
        let slug = self.last_slug.as_bytes();
        let mut frame = Vec::with_capacity(REPO_LIST_CURSOR_FIXED_BYTES + slug.len());
        frame.push(REPO_LIST_CURSOR_VERSION);
        frame.extend_from_slice(&self.scope);
        frame.extend_from_slice(&(slug.len() as u16).to_be_bytes());
        frame.extend_from_slice(slug);
        format!(
            "{REPO_LIST_CURSOR_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    /// The opaque scope bytes the transport must compare with its verified request scope.
    pub fn scope(&self) -> [u8; 32] {
        self.scope
    }

    /// The last visible bare slug used only as a keyset continuation.
    pub fn last_slug(&self) -> &str {
        &self.last_slug
    }
}

fn valid_repo_list_cursor_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= REPO_LIST_ROW_MAX_SLUG_BYTES
        && slug != "."
        && slug != ".."
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// A malformed or non-canonical pull-request commit continuation token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrCommitCursorError;

impl std::fmt::Display for PrCommitCursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("pull-request commit cursor is malformed")
    }
}

impl std::error::Error for PrCommitCursorError {}

/// Canonical continuation state for one immutable pull-request commit snapshot. The transport owns
/// `scope`; Git owns the pinned object ids and bounded revwalk position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCommitCursor {
    scope: [u8; 32],
    base_oid: Option<String>,
    head_oid: String,
    position: usize,
}

impl PrCommitCursor {
    /// Construct a continuation token from a transport scope, pinned snapshot, and non-zero bounded
    /// continuation position.
    pub fn new(
        scope: [u8; 32],
        base_oid: Option<&str>,
        head_oid: &str,
        position: usize,
    ) -> Result<Self, PrCommitCursorError> {
        let base_oid = base_oid.map(parse_cursor_oid).transpose()?;
        let head_oid = parse_cursor_oid(head_oid)?;
        if !(1..=PR_COMMIT_CURSOR_MAX_POSITION).contains(&position) {
            return Err(PrCommitCursorError);
        }
        Ok(Self {
            scope,
            base_oid: base_oid.map(|bytes| cursor_oid_string(&bytes)),
            head_oid: cursor_oid_string(&head_oid),
            position,
        })
    }

    /// Parse the exact fixed-size, versioned, unpadded base64url token representation.
    pub fn parse(value: &str) -> Result<Self, PrCommitCursorError> {
        let encoded = value
            .strip_prefix(PR_COMMIT_CURSOR_PREFIX)
            .ok_or(PrCommitCursorError)?;
        if encoded.is_empty() || value.len() > PR_COMMIT_CURSOR_MAX_BYTES {
            return Err(PrCommitCursorError);
        }
        let frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| PrCommitCursorError)?;
        if frame.len() != PR_COMMIT_CURSOR_FRAME_BYTES
            || frame[0] != PR_COMMIT_CURSOR_VERSION
            || base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&frame) != encoded
        {
            return Err(PrCommitCursorError);
        }

        let mut scope = [0_u8; 32];
        scope.copy_from_slice(&frame[1..33]);
        let mut base_bytes = [0_u8; 20];
        base_bytes.copy_from_slice(&frame[34..54]);
        let base_oid = match frame[33] {
            0 if base_bytes == [0; 20] => None,
            1 => Some(cursor_oid_string(&base_bytes)),
            _ => return Err(PrCommitCursorError),
        };
        let mut head_bytes = [0_u8; 20];
        head_bytes.copy_from_slice(&frame[54..74]);
        let position = usize::try_from(u32::from_be_bytes(
            frame[74..78].try_into().map_err(|_| PrCommitCursorError)?,
        ))
        .map_err(|_| PrCommitCursorError)?;
        Self::new(
            scope,
            base_oid.as_deref(),
            &cursor_oid_string(&head_bytes),
            position,
        )
    }

    /// Render the canonical fixed-size, versioned, unpadded base64url token.
    pub fn encode(&self) -> String {
        let mut frame = Vec::with_capacity(PR_COMMIT_CURSOR_FRAME_BYTES);
        frame.push(PR_COMMIT_CURSOR_VERSION);
        frame.extend_from_slice(&self.scope);
        match self.base_oid.as_deref() {
            Some(oid) => {
                frame.push(1);
                frame.extend_from_slice(&parse_cursor_oid(oid).expect("validated cursor base oid"));
            }
            None => {
                frame.push(0);
                frame.extend_from_slice(&[0; 20]);
            }
        }
        frame.extend_from_slice(
            &parse_cursor_oid(&self.head_oid).expect("validated cursor head oid"),
        );
        frame.extend_from_slice(&(self.position as u32).to_be_bytes());
        debug_assert_eq!(frame.len(), PR_COMMIT_CURSOR_FRAME_BYTES);
        format!(
            "{PR_COMMIT_CURSOR_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    /// Opaque transport-owned scope bytes.
    pub fn scope(&self) -> [u8; 32] {
        self.scope
    }

    /// Pinned base commit, or `None` when the base ref was absent on page one.
    pub fn base_oid(&self) -> Option<&str> {
        self.base_oid.as_deref()
    }

    /// Pinned pull-request head commit.
    pub fn head_oid(&self) -> &str {
        &self.head_oid
    }

    /// Number of snapshot commits already consumed.
    pub fn position(&self) -> usize {
        self.position
    }
}

fn parse_cursor_oid(value: &str) -> Result<[u8; 20], PrCommitCursorError> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(PrCommitCursorError);
    }
    let mut bytes = [0_u8; 20];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = cursor_hex_nibble(pair[0]).ok_or(PrCommitCursorError)?;
        let low = cursor_hex_nibble(pair[1]).ok_or(PrCommitCursorError)?;
        bytes[index] = (high << 4) | low;
    }
    Ok(bytes)
}

fn cursor_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn cursor_oid_string(bytes: &[u8; 20]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(40);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

/// Invalid input for the lightweight repository-list row projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoListRowError {
    InvalidSlug,
    InvalidCloneUrl,
}

impl std::fmt::Display for RepoListRowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSlug => {
                f.write_str("repository-list slug is invalid or exceeds its byte limit")
            }
            Self::InvalidCloneUrl => {
                f.write_str("repository-list clone URL is invalid or exceeds its byte limit")
            }
        }
    }
}

impl std::error::Error for RepoListRowError {}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RepoListRowState {
    Populated,
    Empty,
    Restricted,
}

/// A lightweight repository catalogue row. Unlike [`RepoHome`], this never carries a tree, README,
/// ref counts, default branch, or history metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoListRow {
    state: RepoListRowState,
    slug: Option<String>,
    clone_url: Option<String>,
}

impl RepoListRow {
    pub fn populated(
        slug: impl Into<String>,
        clone_url: impl Into<String>,
    ) -> Result<Self, RepoListRowError> {
        let slug = validated_repo_list_slug(slug.into())?;
        let clone_url = validated_repo_list_clone_url(clone_url.into())?;
        Ok(Self {
            state: RepoListRowState::Populated,
            slug: Some(slug),
            clone_url: Some(clone_url),
        })
    }

    pub fn empty(slug: impl Into<String>) -> Result<Self, RepoListRowError> {
        Ok(Self {
            state: RepoListRowState::Empty,
            slug: Some(validated_repo_list_slug(slug.into())?),
            clone_url: None,
        })
    }

    pub fn restricted() -> Self {
        Self {
            state: RepoListRowState::Restricted,
            slug: None,
            clone_url: None,
        }
    }

    /// Exact compatibility projection for the repository catalogue endpoint.
    pub fn to_json(&self) -> Value {
        match self.state {
            RepoListRowState::Populated => json!({
                "state": "populated",
                "slug": self.slug.as_deref().expect("validated populated row has a slug"),
                "clone_url": self
                    .clone_url
                    .as_deref()
                    .expect("validated populated row has a clone URL"),
            }),
            RepoListRowState::Empty => json!({
                "state": "empty",
                "slug": self.slug.as_deref().expect("validated empty row has a slug"),
            }),
            RepoListRowState::Restricted => json!({ "state": "restricted" }),
        }
    }
}

fn validated_repo_list_slug(slug: String) -> Result<String, RepoListRowError> {
    let valid = !slug.is_empty()
        && slug.len() <= REPO_LIST_ROW_MAX_SLUG_BYTES
        && slug.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        });
    valid.then_some(slug).ok_or(RepoListRowError::InvalidSlug)
}

fn validated_repo_list_clone_url(clone_url: String) -> Result<String, RepoListRowError> {
    let valid = !clone_url.is_empty()
        && clone_url.len() <= REPO_LIST_ROW_MAX_CLONE_URL_BYTES
        && !clone_url.chars().any(char::is_whitespace)
        && !clone_url.chars().any(char::is_control);
    valid
        .then_some(clone_url)
        .ok_or(RepoListRowError::InvalidCloneUrl)
}

/// The **repo home page view-model** (architecture view doc §2.1) — README render, branch switcher,
/// the default-branch file tree, quick actions (clone URL). Carries the per-viewer projection (a repo
/// the viewer cannot see is a tombstone — 0 leak) and the empty state (no commits → clone/push
/// instructions, onboarding-forward).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoHome {
    /// A populated repo — the slug + the README (already rendered to safe HTML upstream — here a plain
    /// text excerpt) + the file-tree entries + the clone URL.
    Populated {
        /// The repo slug (`org/name`).
        slug: String,
        /// A README excerpt (plain text — rendered through the one editor render path upstream).
        readme_excerpt: String,
        /// The top-level tree entries (path, is_dir).
        entries: Vec<(String, bool)>,
        /// The clone URL (ssh/https).
        clone_url: String,
    },
    /// An empty repo — no commits → clone/push instructions (the onboarding-forward empty state).
    Empty {
        /// The repo slug.
        slug: String,
        /// The clone URL to push to.
        clone_url: String,
    },
    /// The viewer may not see the repo — the permission/erased tombstone (no leaked slug content).
    Restricted,
}

impl RepoHome {
    /// Render the repo home to HTML.
    pub fn render(&self) -> String {
        let mut h = String::new();
        h.push_str("<main class=\"repo-home\">");
        match self {
            RepoHome::Populated {
                slug,
                readme_excerpt,
                entries,
                clone_url,
            } => {
                h.push_str(&format!("<h2 class=\"repo-title\">{}</h2>", escape(slug)));
                h.push_str(&format!(
                    "<div class=\"clone-url\"><code>{}</code>\
                     <button class=\"btn\" data-action=\"copy-clone-url\">Copy</button></div>",
                    escape(clone_url)
                ));
                h.push_str("<ul class=\"file-tree\">");
                for (path, is_dir) in entries {
                    let glyph = if *is_dir { "\u{1F4C1}" } else { "\u{1F4C4}" };
                    h.push_str(&format!(
                        "<li class=\"tree-entry\"><span class=\"glyph\" aria-hidden=\"true\">{}</span>\
                         <a href=\"blob/{}\"><code>{}</code></a></li>",
                        glyph,
                        escape(path),
                        escape(path),
                    ));
                }
                h.push_str("</ul>");
                h.push_str(&format!(
                    "<section class=\"readme\"><pre>{}</pre></section>",
                    escape(readme_excerpt)
                ));
            }
            RepoHome::Empty { slug, clone_url } => {
                h.push_str(&format!("<h2 class=\"repo-title\">{}</h2>", escape(slug)));
                h.push_str(&format!(
                    "<div class=\"state-empty\"><p>This repository has no commits yet.</p>\
                     <pre class=\"onboard\">git clone {}\ngit push -u origin main</pre></div>",
                    escape(clone_url),
                ));
            }
            RepoHome::Restricted => {
                h.push_str(
                    "<div class=\"state-restricted\" role=\"note\">\
                     <span class=\"glyph\" aria-hidden=\"true\">\u{1F512}</span>\
                     <p>This repository is not available to you.</p></div>",
                );
            }
        }
        h.push_str("</main>");
        h
    }
}

/// The **single-file web edit form** (the GF-6 floor — design pass §4 / view doc §2.2). A v1
/// single-file edit + commit surface. **No 3-way conflict editor** (GIT-P33/M5+): on a stale-base
/// conflict v1 REFUSES with an honest message rather than offering a merge editor. The commit lowers to
/// the SAME receive-pack one-tx ref-CAS the rest of the platform uses (not a parallel write path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebEditForm {
    /// The file path being edited (rendered monospace).
    pub path: String,
    /// The current file contents (the editable buffer).
    pub contents: String,
    /// The base blob oid the edit is against (the CAS expectation — a stale base REFUSES, GF-6).
    pub base_oid: String,
    /// `true` iff the viewer may edit (write permission) — a read-only viewer sees the file, no
    /// composer (the composer is ABSENT, not greyed — DESIGN-MANUAL §4.2 forms rule).
    pub viewer_may_edit: bool,
}

impl WebEditForm {
    /// Render the single-file web-edit form. A read-only viewer gets the view, no composer (absent,
    /// never greyed). The conflict floor (GF-6) is documented in the form's note.
    pub fn render(&self) -> String {
        let mut h = String::new();
        h.push_str("<main class=\"web-edit\">");
        h.push_str(&format!(
            "<h3 class=\"edit-path\"><code>{}</code></h3>",
            escape(&self.path)
        ));
        if self.viewer_may_edit {
            h.push_str(&format!(
                "<form data-action=\"web-commit\" data-base-oid=\"{}\">",
                escape(&self.base_oid)
            ));
            h.push_str(&format!(
                "<textarea class=\"edit-buffer\" name=\"contents\" aria-label=\"File contents\">{}</textarea>",
                escape(&self.contents)
            ));
            h.push_str(
                "<label class=\"commit-msg-label\">Commit message\
                 <input class=\"commit-msg\" name=\"message\" /></label>",
            );
            h.push_str(
                "<p class=\"edit-note st-muted\">Single-file edit. If the file changed since you \
                 opened it, this edit will be refused so nothing is silently overwritten.</p>",
            );
            h.push_str(
                "<button class=\"btn btn-primary\" type=\"submit\" data-action=\"commit-file\">\
                 Commit change</button>",
            );
            h.push_str("</form>");
        } else {
            // read-only: the file renders; the composer is ABSENT (not greyed).
            h.push_str(&format!(
                "<pre class=\"edit-buffer readonly\">{}</pre>",
                escape(&self.contents)
            ));
        }
        h.push_str("</main>");
        h
    }
}

/// The outcome of a single-file web-edit commit (GF-6). v1 admits a clean fast-forward and REFUSES a
/// stale-base conflict with an honest message (no 3-way editor; GIT-P33/M5+). This is the view-model
/// the form's submit handler renders; the actual ref-CAS lowers to the receive-pack one-tx path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebEditOutcome {
    /// Committed — the base matched and the ref advanced (the receive-pack one-tx ref-CAS succeeded).
    Committed {
        /// The new commit oid.
        new_oid: String,
    },
    /// Refused — the base oid no longer matches HEAD (someone else committed). v1 refuses honestly
    /// rather than offering a 3-way merge editor (GF-6 floor). The viewer must reload + re-apply.
    StaleBase {
        /// The current HEAD oid the editor expected to match `base_oid`.
        current_oid: String,
    },
    /// The viewer may not write to this ref (permission-denied).
    Denied,
}

impl WebEditOutcome {
    /// Evaluate a web-edit commit against the expected base oid (the GF-6 single-file CAS). A matching
    /// base fast-forwards (committed); a mismatched base REFUSES (stale — no silent overwrite, no 3-way
    /// editor). `viewer_may_write = false` is denied.
    pub fn evaluate(
        expected_base: &str,
        current_head: &str,
        new_oid: &str,
        viewer_may_write: bool,
    ) -> WebEditOutcome {
        if !viewer_may_write {
            return WebEditOutcome::Denied;
        }
        if expected_base == current_head {
            WebEditOutcome::Committed {
                new_oid: new_oid.to_string(),
            }
        } else {
            // GF-6: refuse honestly rather than silently overwrite or offer a 3-way editor.
            WebEditOutcome::StaleBase {
                current_oid: current_head.to_string(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The page shell + stylesheet (semantic tokens only; no inline interactive colour)
// ---------------------------------------------------------------------------

/// The minimal **stylesheet** the rendered HTML carries — binds each semantic class to its
/// `var(--…)` token (design pass §1). It consumes ONLY semantic tokens (never a primitive, never an
/// inline interactive colour — the inline-colour ban). The real frontend links the generated
/// `tokens.css`; this embedded sheet imports those token NAMES so a stack-down browser render still
/// shows the correct treatments (with the design system's fallback values). Status classes carry the
/// COLOUR channel only — the glyph + label channels are in the markup (never colour alone).
pub const STYLE: &str = "\
:root{\
  --surface:#0e1116;--surface-raised:#161b22;--surface-overlay:#1c2230;\
  --text-primary:#e6edf3;--text-muted:#8b949e;--border:#30363d;\
  --accent:#3b82f6;--focus-ring:#7aa2ff;\
  --success:#3fb950;--danger:#f85149;--warning:#d29922;--info:#58a6ff;--agent:#a371f7;\
}\
body{background:var(--surface);color:var(--text-primary);\
  font-family:-apple-system,system-ui,sans-serif;font-size:14px;margin:0;padding:16px;}\
code,pre,.check-context{font-family:ui-monospace,'JetBrains Mono',monospace;font-size:13px;}\
h1,h2,h3{font-weight:600;}\
.panel-title{font-size:16px;}\
section,.fork-trust-badge,.web-edit form{background:var(--surface-raised);\
  border:1px solid var(--border);border-radius:3px;padding:12px;margin:12px 0;}\
.st-success,.check-status.st-success .label{color:var(--success);}\
.st-danger,.check-status.st-danger .label{color:var(--danger);}\
.st-warning,.check-status.st-warning .label{color:var(--warning);}\
.st-info,.check-status.st-info .label{color:var(--info);}\
.st-muted,.check-status.st-muted .label{color:var(--text-muted);}\
.st-agent{color:var(--agent);}\
.check-row{display:flex;gap:12px;align-items:baseline;list-style:none;padding:4px 0;}\
.check-rows{padding:0;margin:0;}\
.check-summary{color:var(--text-muted);}\
.fork-trust-badge{border-color:var(--warning);}\
.badge-explain{color:var(--text-muted);font-size:13px;}\
.btn{background:var(--surface-overlay);color:var(--text-primary);\
  border:1px solid var(--border);border-radius:3px;padding:4px 12px;cursor:pointer;}\
.btn-primary{background:var(--accent);color:#fff;border-color:var(--accent);}\
.btn:focus-visible,.btn-primary:focus-visible,a:focus-visible,textarea:focus-visible,\
input:focus-visible{outline:2px solid var(--focus-ring);outline-offset:2px;}\
.skeleton-row{height:20px;background:var(--surface-overlay);border-radius:3px;margin:6px 0;}\
.state-restricted,.state-empty,.state-error,.state-loading{color:var(--text-muted);}\
.sr-only{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0,0,0,0);}\
.pr-state-pill,.draft-pill{border:1px solid var(--border);border-radius:999px;\
  padding:2px 8px;font-size:12px;}\
.edit-buffer{width:100%;min-height:160px;background:var(--surface);color:var(--text-primary);\
  border:1px solid var(--border);}\
@media (prefers-reduced-motion:reduce){*{animation:none!important;transition:none!important;}}\
";

/// Wrap a rendered surface body in the full HTML page shell (the pinned-viewport shell, the embedded
/// semantic-token stylesheet, the `lang`/`dir` for i18n, the reduced-motion path). This is the
/// browseable page the GIT-P32 e2e walkthrough drives in chromium.
pub fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\" dir=\"ltr\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{}</title><style>{}</style></head><body data-theme=\"dark\">{}</body></html>",
        escape(title),
        STYLE,
        body,
    )
}

// ---------------------------------------------------------------------------
// The JSON projection — the edge DATA contract (MR-015, E0.6)
// ---------------------------------------------------------------------------
//
// The product edge (`myelin-edge`, MR-014) serves the ViewModel DATA as JSON — the UI renders, the
// edge provides the projection (catalogue.rs: "the JSON view-model/data contract"). Each `to_json`
// MIRRORS the ViewModel's own fields — the SAME vocabulary the `render()` HTML projection consumes
// (design pass §0: never a parallel vocabulary). One ViewModel, two projections (HTML + JSON), fed by
// the same already-built backend logic — Git OWNS the data vocabulary, the edge is a thin re-rooter.

impl StatusCue {
    /// The status cue as JSON `{ token, glyph, label }` — the three-channel status signal (semantic
    /// token NAME + role glyph + text label), never colour alone (the same data `render()` shows).
    pub fn to_json(&self) -> Value {
        json!({ "token": self.token.name(), "glyph": self.glyph, "label": self.label })
    }
}

impl ForkTrustBadge {
    /// The fork-trust badge as JSON. The `[ Trust this run ]` affordance is permission-gated: the
    /// `viewer_may_endorse` flag mirrors `render()` (a viewer without `approve_untrusted_ci` gets the
    /// honest badge but no action — no leaked affordance).
    pub fn to_json(&self) -> Value {
        json!({ "viewer_may_endorse": self.viewer_may_endorse })
    }
}

impl CheckRowView {
    /// One checks-panel row as JSON `{ context, cue, required, summary, fork_badge }` — the X-1
    /// consumer surface's data (design pass §4.2). `fork_badge` is present ONLY for an un-endorsed
    /// `untrusted_fork` row (the load-bearing X-1 affordance, design pass §4.1).
    pub fn to_json(&self) -> Value {
        json!({
            "context": self.context,
            "cue": self.cue.to_json(),
            "required": self.required,
            "summary": self.summary,
            "fork_badge": self.fork_badge.as_ref().map(ForkTrustBadge::to_json),
        })
    }
}

impl ChecksPanel {
    /// The checks panel as JSON, carrying ITS state (`live` rows / `empty` / `loading` skeleton /
    /// `error`) — the panel fails static for its own surface only (design pass §4.2).
    pub fn to_json(&self) -> Value {
        match self {
            ChecksPanel::Live { rows } => json!({
                "state": "live",
                "rows": rows.iter().map(CheckRowView::to_json).collect::<Vec<_>>(),
            }),
            ChecksPanel::Empty => json!({ "state": "empty" }),
            ChecksPanel::Loading { skeleton_rows } => {
                json!({ "state": "loading", "skeleton_rows": skeleton_rows })
            }
            ChecksPanel::Error => json!({ "state": "error" }),
        }
    }
}

impl MergeReadiness {
    /// The merge-readiness affordance as JSON (design pass §4.3) — names WHICH context is unmet
    /// (humanised, never a bare "blocked"), the queue position, or the multi-day HITL hold.
    pub fn to_json(&self) -> Value {
        match self {
            MergeReadiness::Ready { approvals } => json!({
                "state": "ready",
                "approvals": { "current": approvals.0, "required": approvals.1 },
            }),
            MergeReadiness::Blocked { unmet } => json!({
                "state": "blocked",
                "unmet": unmet
                    .iter()
                    .map(|u| json!({ "context": u.context.name, "reason": humanise_unmet(u) }))
                    .collect::<Vec<_>>(),
            }),
            MergeReadiness::Queued { position } => json!({ "state": "queued", "position": position }),
            MergeReadiness::HitlHold { awaiting } => {
                json!({ "state": "hitl_hold", "awaiting": awaiting })
            }
        }
    }
}

impl PrOverviewPage {
    /// The PR overview page as JSON (the centrepiece, view doc §2.2). A tombstone short-circuits to the
    /// content-free restricted state — the title/state NEVER reach the JSON (the 0-leak invariant, the
    /// SAME boundary `render()` enforces: no-access is indistinguishable from erased).
    pub fn to_json(&self) -> Value {
        match &self.projected {
            Projected::Tombstoned(_t) => json!({
                "visible": false,
                "restricted": true,
                // No title, no state, no reason — the viewer learns only "not available" (0 leak).
            }),
            Projected::Visible(p) => json!({
                "visible": true,
                "title": p.title,
                "state": p.state,
                "icon": p.icon,
                "pr_state": pr_state_label(self.pr_state),
                "render_hint": p.render_hint.as_ref().map(render_hint_json),
                "sub_anchor": p.sub_anchor.as_ref().map(|s| json!({
                    "kind": s.kind, "excerpt": s.excerpt,
                })),
                "checks": self.checks.to_json(),
                "merge": self.merge.to_json(),
            }),
        }
    }
}

/// The PR render-hint as JSON (the coarse checks/approvals/draft summary, project §3). The checks
/// summary is the Git-OWNED gate state (green/red/neutral), never a raw CI string.
fn render_hint_json(h: &RenderHint) -> Value {
    let checks = match h.checks {
        ChecksSummary::Green => "green",
        ChecksSummary::Red => "red",
        ChecksSummary::Neutral => "neutral",
    };
    json!({
        "checks": checks,
        "approvals": { "current": h.approvals.0, "required": h.approvals.1 },
        "is_draft": h.is_draft,
    })
}

impl RepoHome {
    /// The repo-home view-model as JSON (view doc §2.1) — populated (slug + README excerpt + tree +
    /// clone URL) / empty (onboarding-forward) / restricted (the 0-leak tombstone, no leaked slug).
    pub fn to_json(&self) -> Value {
        match self {
            RepoHome::Populated { slug, readme_excerpt, entries, clone_url } => json!({
                "state": "populated",
                "slug": slug,
                "readme_excerpt": readme_excerpt,
                "clone_url": clone_url,
                "entries": entries
                    .iter()
                    .map(|(path, is_dir)| json!({ "path": path, "is_dir": is_dir }))
                    .collect::<Vec<_>>(),
            }),
            RepoHome::Empty { slug, clone_url } => {
                json!({ "state": "empty", "slug": slug, "clone_url": clone_url })
            }
            RepoHome::Restricted => json!({ "state": "restricted" }),
        }
    }
}

impl WebEditForm {
    /// The single-file web-edit/file-view form as JSON (GF-6). A read-only viewer (`viewer_may_edit =
    /// false`) still gets the file contents — the COMPOSER is absent, not the content (the same posture
    /// `render()` takes: the composer is absent, not greyed).
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "contents": self.contents,
            "base_oid": self.base_oid,
            "viewer_may_edit": self.viewer_may_edit,
        })
    }
}

impl WebEditOutcome {
    /// The web-edit commit outcome as JSON (GF-6) — `committed` (the ref WOULD advance) / `stale_base`
    /// (refused honestly, no 3-way editor) / `denied` (no write permission).
    pub fn to_json(&self) -> Value {
        match self {
            WebEditOutcome::Committed { new_oid } => {
                json!({ "outcome": "committed", "new_oid": new_oid })
            }
            WebEditOutcome::StaleBase { current_oid } => {
                json!({ "outcome": "stale_base", "current_oid": current_oid })
            }
            WebEditOutcome::Denied => json!({ "outcome": "denied" }),
        }
    }
}

// ---------------------------------------------------------------------------
// Commit log + commit diff (the browse surface — GT-004) — JSON-contract ViewModels
// ---------------------------------------------------------------------------
//
// These browse ViewModels carry ONLY a `to_json` projection (no legacy HTML `render()`): the
// load-bearing render path for the browse surface is the Solid Git web UI (GT-004), which renders
// this JSON. Git still OWNS the data vocabulary — the edge reads the real on-disk commit objects
// (via libgit2, the same backend `GixCore` wraps) and projects them through these ViewModels; the UI
// never invents a parallel shape. PII-free: the author is the GIT-1 tenant pseudonym, never a raw
// identity; `committed_at` is unix seconds (the client formats it for the viewer's locale).

/// The first 12 chars of an oid — the short form the log/diff headers render (full oid is the link).
pub fn short_oid(oid: &str) -> String {
    oid.chars().take(12).collect()
}

/// One **commit-log row** (the browse log surface). The summary is the commit's first line; the
/// author is the tenant pseudonym; `committed_at` is unix seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRow {
    /// The full commit oid (the diff link target).
    pub oid: String,
    /// The commit summary (first line of the message).
    pub summary: String,
    /// The author — the GIT-1 tenant pseudonym (never a raw identity).
    pub author: String,
    /// Commit time, unix seconds (the client formats per the viewer's locale).
    pub committed_at: i64,
    /// The parent oids (≥1 = a merge commit has >1; 0 = the root commit).
    pub parents: Vec<String>,
}

impl CommitRow {
    /// One log row as JSON `{ oid, short_oid, summary, author, committed_at, parents }`.
    pub fn to_json(&self) -> Value {
        json!({
            "oid": self.oid,
            "short_oid": short_oid(&self.oid),
            "summary": self.summary,
            "author": self.author,
            "committed_at": self.committed_at,
            "parents": self.parents,
        })
    }
}

/// One **diff line** in a file delta — `origin` is `'+'` (added) / `'-'` (removed) / `' '` (context),
/// the same three-channel signal the diff render binds (never colour alone — the `+`/`-` glyph + the
/// line position carry the meaning for a monochrome viewer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLineView {
    /// `+` add / `-` remove / ` ` context.
    pub origin: char,
    /// The line content (newline-trimmed).
    pub content: String,
}

impl DiffLineView {
    /// One diff line as JSON `{ origin, content }`.
    pub fn to_json(&self) -> Value {
        json!({ "origin": self.origin.to_string(), "content": self.content })
    }
}

/// One **changed file** in a commit diff — its path, the rename source (if any), the change status
/// (`A`/`M`/`D`/`R`/`C`), and the unified-diff lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFile {
    /// The (new) file path.
    pub path: String,
    /// The old path for a rename/copy (`None` otherwise).
    pub old_path: Option<String>,
    /// The change status glyph — `A` added / `M` modified / `D` deleted / `R` renamed / `C` copied.
    pub status: char,
    /// The unified-diff lines (added/removed/context).
    pub lines: Vec<DiffLineView>,
}

impl DiffFile {
    /// One changed file as JSON `{ path, old_path, status, lines }`.
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "old_path": self.old_path,
            "status": self.status.to_string(),
            "lines": self.lines.iter().map(DiffLineView::to_json).collect::<Vec<_>>(),
        })
    }
}

/// The **commit diff page** (the browse diff surface) — the commit header (oid/summary/full message/
/// author/time) + the per-file unified diff against the first parent (the root commit diffs against
/// the empty tree). Built from the real on-disk commit object (libgit2 `diff_tree_to_tree`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDiff {
    /// The commit header row (oid/summary/author/time/parents).
    pub commit: CommitRow,
    /// The full commit message (body + summary).
    pub message: String,
    /// The changed files (each with its unified-diff lines).
    pub files: Vec<DiffFile>,
}

impl CommitDiff {
    /// The commit diff page as JSON — the commit header (flattened) + `message` + `files`.
    pub fn to_json(&self) -> Value {
        json!({
            "oid": self.commit.oid,
            "short_oid": short_oid(&self.commit.oid),
            "summary": self.commit.summary,
            "message": self.message,
            "author": self.commit.author,
            "committed_at": self.commit.committed_at,
            "parents": self.commit.parents,
            "files": self.files.iter().map(DiffFile::to_json).collect::<Vec<_>>(),
        })
    }
}

// ───────────────────────────── PR diff ViewModel (R3.2 · G-7 N1) ─────────────────────────────────

/// One PR-diff line — origin + BOTH line numbers (`old_no` null on `+`, `new_no` null on `-`). The
/// SR prefix and the anchor/deep-link machinery need the numbers as first-class data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrDiffLine {
    pub origin: char,
    pub content: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

impl PrDiffLine {
    pub fn to_json(&self) -> Value {
        json!({
            "origin": self.origin.to_string(),
            "content": self.content,
            "old_no": self.old_no,
            "new_no": self.new_no,
        })
    }
}

/// One hunk of a PR-diff file — the `@@` header + boundaries + lines (collapsed-run + expand-context
/// need the boundaries a flat `lines[]` can't carry).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrDiffHunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<PrDiffLine>,
}

impl PrDiffHunk {
    pub fn to_json(&self) -> Value {
        json!({
            "header": self.header,
            "old_start": self.old_start,
            "old_lines": self.old_lines,
            "new_start": self.new_start,
            "new_lines": self.new_lines,
            "lines": self.lines.iter().map(PrDiffLine::to_json).collect::<Vec<_>>(),
        })
    }
}

/// One changed file in a PR diff. A RESTRICTED file is NEVER in this list — the count-only disclosure
/// lives on [`PrDiffVM::restricted_files`] (non-leak by construction: no path/diffstat crosses the wire).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrDiffFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: char,
    /// `text` / `binary` / `lfs` / `submodule` — drives the R-21 binary/LFS row (never a garbled dump).
    pub kind: String,
    pub additions: u32,
    pub deletions: u32,
    pub size_bytes: Option<u64>,
    pub hunks: Vec<PrDiffHunk>,
    pub deleted_body_available: bool,
    pub truncated: bool,
}

impl PrDiffFile {
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "old_path": self.old_path,
            "status": self.status.to_string(),
            "kind": self.kind,
            "additions": self.additions,
            "deletions": self.deletions,
            "size_bytes": self.size_bytes,
            "hunks": self.hunks.iter().map(PrDiffHunk::to_json).collect::<Vec<_>>(),
            "deleted_body_available": self.deleted_body_available,
            "truncated": self.truncated,
        })
    }
}

/// The **PR diff page** (`GET …/prs/{n}/diff`) — the three-dot `merge-base(base, head) … head` diff.
/// `restricted_files` is COUNT-ONLY (no path/diffstat); `three_dot == false` labels the two-dot floor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrDiffVM {
    pub number: u64,
    pub base_ref: String,
    pub base_oid: String,
    pub head_oid: String,
    pub three_dot: bool,
    pub files: Vec<PrDiffFile>,
    pub restricted_files: u64,
    pub total_files: usize,
    pub total_additions: u32,
    pub total_deletions: u32,
    /// The MR-014 file cursor + the viewer-local viewed marks are client-side; the wire carries only
    /// the page cursor (viewed = localStorage, R3 Q6 floor).
    pub next_cursor: Option<String>,
    pub limit: usize,
}

impl PrDiffVM {
    pub fn to_json(&self) -> Value {
        json!({
            "number": self.number,
            "base_ref": self.base_ref,
            "base_oid": self.base_oid,
            "short_base_oid": short_oid(&self.base_oid),
            "head_oid": self.head_oid,
            "short_head_oid": short_oid(&self.head_oid),
            "three_dot": self.three_dot,
            "files": self.files.iter().map(PrDiffFile::to_json).collect::<Vec<_>>(),
            "restricted_files": self.restricted_files,
            "total_files": self.total_files,
            "total_additions": self.total_additions,
            "total_deletions": self.total_deletions,
            "page": { "next_cursor": self.next_cursor, "limit": self.limit },
        })
    }
}

#[cfg(test)]
mod tests;
