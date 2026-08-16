use crate::check_status::{CheckState, CheckStatusRow, TrustTier};
use crate::lifecycle::PrState;
use crate::merge_gate::{MergeGateOutcome, UnmetContext, UnmetReason};
use crate::project::{ChecksSummary, Projected, RenderHint};
use base64::Engine as _;
use serde_json::{json, Value};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusToken {
    Success,
    Danger,
    Warning,
    Info,
    Muted,
    Agent,
}

impl StatusToken {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusCue {
    pub token: StatusToken,
    pub glyph: &'static str,
    pub label: &'static str,
}

impl StatusCue {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkTrustBadge {
    pub viewer_may_endorse: bool,
}

impl ForkTrustBadge {
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

    pub fn render(&self) -> String {
        let mut h = String::new();
        h.push_str(&format!(
            "<div class=\"fork-trust-badge {}\" role=\"note\">",
            StatusToken::Warning.css_class()
        ));
        h.push_str(
            "<span class=\"badge-line\"><span class=\"glyph\" aria-hidden=\"true\">\u{26A8}</span>\
             <strong>passed on a FORK run \u{2014} neutral until trusted</strong></span>",
        );
        h.push_str(
            "<p class=\"badge-explain\">This run executed code from an untrusted fork. \
             It does NOT satisfy the gate by itself. A maintainer must review and trust it.</p>",
        );
        if self.viewer_may_endorse {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckRowView {
    pub context: String,
    pub cue: StatusCue,
    pub required: bool,
    pub summary: String,
    pub fork_badge: Option<ForkTrustBadge>,
}

impl CheckRowView {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChecksPanel {
    Live { rows: Vec<CheckRowView> },
    Empty,
    Loading { skeleton_rows: usize },
    Error,
}

impl ChecksPanel {
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
                h.push_str(
                    "<div class=\"state-empty\"><p>No checks configured for this branch.</p>\
                     <a class=\"btn\" href=\"settings/rulesets\">Configure required checks</a></div>",
                );
            }
            ChecksPanel::Loading { skeleton_rows } => {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeReadiness {
    Ready { approvals: (u32, u32) },
    Blocked { unmet: Vec<UnmetContext> },
    Queued { position: usize },
    HitlHold { awaiting: String },
}

impl MergeReadiness {
    pub fn from_gate(outcome: &MergeGateOutcome, approvals: (u32, u32)) -> MergeReadiness {
        match outcome {
            MergeGateOutcome::Admitted => MergeReadiness::Ready { approvals },
            MergeGateOutcome::Blocked { unmet } => MergeReadiness::Blocked {
                unmet: unmet.clone(),
            },
        }
    }

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

fn humanise_unmet(u: &UnmetContext) -> String {
    let ctx = &u.context.name;
    match &u.reason {
        UnmetReason::Missing => format!("{ctx} not reported"),
        UnmetReason::NotGreen { state } => {
            let cue = StatusCue::for_check_state(*state);
            format!("{ctx} {}", cue.label)
        }
        UnmetReason::CostUnsettled => format!("{ctx} awaiting settlement"),
        UnmetReason::UntrustedForkNeutral => format!("{ctx} awaiting fork trust"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrOverviewPage {
    pub projected: Projected,
    pub pr_state: PrState,
    pub checks: ChecksPanel,
    pub merge: MergeReadiness,
}

impl PrOverviewPage {
    pub fn render(&self) -> String {
        let mut h = String::new();
        h.push_str("<main class=\"pr-overview\">");
        match &self.projected {
            Projected::Tombstoned(_t) => {
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

pub fn representative_pr_page(tenant: &str) -> PrOverviewPage {
    use crate::check_status::{CheckContext, CheckStatus, GitOid, HumanisedRef, Timestamp};
    use myelin_tenancy::{ArtifactRef, TenantId};
    let fact = CheckStatus {
        tenant: TenantId(tenant.into()),
        repo: ArtifactRef(format!("myelin://{tenant}/git/repo/myelin")),
        commit_oid: GitOid("blake3:representativehead".into()),
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
            title: format!("[{tenant}] Verify backup recovery at cell scale"),
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

pub const REPO_LIST_ROW_MAX_SLUG_BYTES: usize = crate::coordinate::MAX_REPOSITORY_SLUG_BYTES;
pub const REPO_LIST_ROW_MAX_CLONE_URL_BYTES: usize = 4 * 1024;
pub const REPO_LIST_CURSOR_PREFIX: &str = "rl2_";
pub const REPO_LIST_CURSOR_MAX_BYTES: usize = 512;
pub const PR_COMMIT_CURSOR_PREFIX: &str = "pc1_";
pub const PR_COMMIT_CURSOR_MAX_BYTES: usize = 256;
pub const PR_COMMIT_CURSOR_MAX_POSITION: usize = crate::durable::PR_COMMIT_MAX_POSITION;

const REPO_LIST_CURSOR_VERSION: u8 = 2;
const REPO_LIST_CURSOR_FIXED_BYTES: usize = 1 + 32 + 1 + 2;
const PR_COMMIT_CURSOR_VERSION: u8 = 1;
const PR_COMMIT_CURSOR_FRAME_BYTES: usize = 1 + 32 + 1 + 20 + 20 + 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepoListCursorError;

impl std::fmt::Display for RepoListCursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("repository-list cursor is malformed")
    }
}

impl std::error::Error for RepoListCursorError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoListCursor {
    scope: [u8; 32],
    last_catalogue_key: Option<String>,
    last_slug: String,
}

impl RepoListCursor {
    pub fn catalogued(
        scope: [u8; 32],
        last_catalogue_key: impl Into<String>,
        last_slug: impl Into<String>,
    ) -> Result<Self, RepoListCursorError> {
        Self::from_parts(scope, Some(last_catalogue_key.into()), last_slug.into())
    }

    pub fn legacy(
        scope: [u8; 32],
        last_slug: impl Into<String>,
    ) -> Result<Self, RepoListCursorError> {
        Self::from_parts(scope, None, last_slug.into())
    }

    fn from_parts(
        scope: [u8; 32],
        last_catalogue_key: Option<String>,
        last_slug: String,
    ) -> Result<Self, RepoListCursorError> {
        if !valid_repo_list_cursor_slug(&last_slug)
            || last_catalogue_key
                .as_deref()
                .is_some_and(|key| !valid_repo_list_catalogue_key(key))
        {
            return Err(RepoListCursorError);
        }
        Ok(Self {
            scope,
            last_catalogue_key,
            last_slug,
        })
    }

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
        let catalogue_key_len = usize::from(frame[33]);
        let slug_len = usize::from(u16::from_be_bytes([frame[34], frame[35]]));
        if slug_len == 0
            || frame.len() != REPO_LIST_CURSOR_FIXED_BYTES + catalogue_key_len + slug_len
        {
            return Err(RepoListCursorError);
        }
        let catalogue_key_end = REPO_LIST_CURSOR_FIXED_BYTES + catalogue_key_len;
        let last_catalogue_key = if catalogue_key_len == 0 {
            None
        } else {
            Some(
                std::str::from_utf8(&frame[REPO_LIST_CURSOR_FIXED_BYTES..catalogue_key_end])
                    .map_err(|_| RepoListCursorError)?
                    .to_string(),
            )
        };
        let last_slug = std::str::from_utf8(&frame[catalogue_key_end..])
            .map_err(|_| RepoListCursorError)?
            .to_string();
        Self::from_parts(scope, last_catalogue_key, last_slug)
    }

    pub fn encode(&self) -> String {
        let catalogue_key = self
            .last_catalogue_key
            .as_deref()
            .unwrap_or_default()
            .as_bytes();
        let slug = self.last_slug.as_bytes();
        let mut frame =
            Vec::with_capacity(REPO_LIST_CURSOR_FIXED_BYTES + catalogue_key.len() + slug.len());
        frame.push(REPO_LIST_CURSOR_VERSION);
        frame.extend_from_slice(&self.scope);
        frame.push(catalogue_key.len() as u8);
        frame.extend_from_slice(&(slug.len() as u16).to_be_bytes());
        frame.extend_from_slice(catalogue_key);
        frame.extend_from_slice(slug);
        format!(
            "{REPO_LIST_CURSOR_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    pub fn scope(&self) -> [u8; 32] {
        self.scope
    }

    pub fn last_catalogue_key(&self) -> Option<&str> {
        self.last_catalogue_key.as_deref()
    }

    pub fn last_slug(&self) -> &str {
        &self.last_slug
    }
}

fn valid_repo_list_cursor_slug(slug: &str) -> bool {
    crate::coordinate::RepositorySlug::parse(slug).is_ok()
}

fn valid_repo_list_catalogue_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= crate::durable::REPOSITORY_CATALOGUE_KEY_MAX_BYTES
        && key.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrCommitCursorError;

impl std::fmt::Display for PrCommitCursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("pull-request commit cursor is malformed")
    }
}

impl std::error::Error for PrCommitCursorError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCommitCursor {
    scope: [u8; 32],
    base_oid: Option<CursorOid>,
    head_oid: CursorOid,
    position: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CursorOid {
    bytes: [u8; 20],
    text: String,
}

impl CursorOid {
    fn parse(value: &str) -> Result<Self, PrCommitCursorError> {
        let bytes = parse_cursor_oid(value)?;
        Ok(Self {
            bytes,
            text: value.to_string(),
        })
    }

    fn from_bytes(bytes: [u8; 20]) -> Self {
        Self {
            text: cursor_oid_string(&bytes),
            bytes,
        }
    }
}

impl PrCommitCursor {
    pub fn new(
        scope: [u8; 32],
        base_oid: Option<&str>,
        head_oid: &str,
        position: usize,
    ) -> Result<Self, PrCommitCursorError> {
        let base_oid = base_oid.map(CursorOid::parse).transpose()?;
        let head_oid = CursorOid::parse(head_oid)?;
        Self::from_parts(scope, base_oid, head_oid, position)
    }

    fn from_parts(
        scope: [u8; 32],
        base_oid: Option<CursorOid>,
        head_oid: CursorOid,
        position: usize,
    ) -> Result<Self, PrCommitCursorError> {
        if !(1..=PR_COMMIT_CURSOR_MAX_POSITION).contains(&position) {
            return Err(PrCommitCursorError);
        }
        Ok(Self {
            scope,
            base_oid,
            head_oid,
            position,
        })
    }

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
            1 => Some(CursorOid::from_bytes(base_bytes)),
            _ => return Err(PrCommitCursorError),
        };
        let mut head_bytes = [0_u8; 20];
        head_bytes.copy_from_slice(&frame[54..74]);
        let position = usize::try_from(u32::from_be_bytes(
            frame[74..78].try_into().map_err(|_| PrCommitCursorError)?,
        ))
        .map_err(|_| PrCommitCursorError)?;
        Self::from_parts(scope, base_oid, CursorOid::from_bytes(head_bytes), position)
    }

    pub fn encode(&self) -> String {
        let mut frame = Vec::with_capacity(PR_COMMIT_CURSOR_FRAME_BYTES);
        frame.push(PR_COMMIT_CURSOR_VERSION);
        frame.extend_from_slice(&self.scope);
        match &self.base_oid {
            Some(oid) => {
                frame.push(1);
                frame.extend_from_slice(&oid.bytes);
            }
            None => {
                frame.push(0);
                frame.extend_from_slice(&[0; 20]);
            }
        }
        frame.extend_from_slice(&self.head_oid.bytes);
        frame.extend_from_slice(&(self.position as u32).to_be_bytes());
        debug_assert_eq!(frame.len(), PR_COMMIT_CURSOR_FRAME_BYTES);
        format!(
            "{PR_COMMIT_CURSOR_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    pub fn scope(&self) -> [u8; 32] {
        self.scope
    }

    pub fn base_oid(&self) -> Option<&str> {
        self.base_oid.as_ref().map(|oid| oid.text.as_str())
    }

    pub fn head_oid(&self) -> &str {
        &self.head_oid.text
    }

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
pub enum RepoListRow {
    Populated { slug: String, clone_url: String },
    Empty { slug: String },
}

impl RepoListRow {
    pub fn populated(
        slug: impl Into<String>,
        clone_url: impl Into<String>,
    ) -> Result<Self, RepoListRowError> {
        let slug = validated_repo_list_slug(slug.into())?;
        let clone_url = validated_repo_list_clone_url(clone_url.into())?;
        Ok(Self::Populated { slug, clone_url })
    }

    pub fn empty(slug: impl Into<String>) -> Result<Self, RepoListRowError> {
        Ok(Self::Empty {
            slug: validated_repo_list_slug(slug.into())?,
        })
    }

    pub fn to_json(&self) -> Value {
        match self {
            Self::Populated { slug, clone_url } => json!({
                "state": "populated",
                "slug": slug,
                "clone_url": clone_url,
            }),
            Self::Empty { slug } => json!({
                "state": "empty",
                "slug": slug,
            }),
        }
    }
}

fn validated_repo_list_slug(slug: String) -> Result<String, RepoListRowError> {
    let valid = crate::coordinate::RepositorySlug::parse(&slug).is_ok();
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebEditForm {
    pub path: String,
    pub contents: String,
    pub base_oid: String,
    pub viewer_may_edit: bool,
}

impl WebEditForm {
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
            h.push_str(&format!(
                "<pre class=\"edit-buffer readonly\">{}</pre>",
                escape(&self.contents)
            ));
        }
        h.push_str("</main>");
        h
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebEditOutcome {
    Committed { new_oid: String },
    StaleBase { current_oid: String },
    Denied,
}

impl WebEditOutcome {
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
            WebEditOutcome::StaleBase {
                current_oid: current_head.to_string(),
            }
        }
    }
}

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

impl StatusCue {
    pub fn to_json(&self) -> Value {
        json!({ "token": self.token.name(), "glyph": self.glyph, "label": self.label })
    }
}

impl ForkTrustBadge {
    pub fn to_json(&self) -> Value {
        json!({ "viewer_may_endorse": self.viewer_may_endorse })
    }
}

impl CheckRowView {
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
            MergeReadiness::Queued { position } => {
                json!({ "state": "queued", "position": position })
            }
            MergeReadiness::HitlHold { awaiting } => {
                json!({ "state": "hitl_hold", "awaiting": awaiting })
            }
        }
    }
}

impl PrOverviewPage {
    pub fn to_json(&self) -> Value {
        match &self.projected {
            Projected::Tombstoned(_t) => json!({
                "visible": false,
                "restricted": true,
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

impl WebEditForm {
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

pub fn short_oid(oid: &str) -> String {
    oid.chars().take(12).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRow {
    pub oid: String,
    pub summary: String,
    pub author: String,
    pub committed_at: i64,
    pub parents: Vec<String>,
}

impl CommitRow {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLineView {
    pub origin: char,
    pub content: String,
}

impl DiffLineView {
    pub fn to_json(&self) -> Value {
        json!({ "origin": self.origin.to_string(), "content": self.content })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: char,
    pub lines: Vec<DiffLineView>,
}

impl DiffFile {
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "old_path": self.old_path,
            "status": self.status.to_string(),
            "lines": self.lines.iter().map(DiffLineView::to_json).collect::<Vec<_>>(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDiff {
    pub commit: CommitRow,
    pub message: String,
    pub files: Vec<DiffFile>,
}

impl CommitDiff {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrDiffFile {
    pub path: String,
    pub old_path: Option<String>,
    pub new_blob_oid: Option<String>,
    pub status: char,
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
            "new_blob_oid": self.new_blob_oid,
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
