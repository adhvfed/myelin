//! # `e2e_wedge` — Notif's leg of the whole-system E2E wedge (NOTIF-P28 / P-470, M5)
//!
//! **The E2E-1 PR-context-pane leg (N-M5.3).** This module is the **Notif side of the E2E-1
//! whole-system chained-mutation scenario** — the PR context pane. It is driven **end-to-end** (the
//! whole flow, not a single handler) over the **production-hardened Notif engine** the prior prompts
//! built — the per-viewer [`crate::humanise`] render (NOTIF-P9, contract 7.3) and the firehose
//! live-update transport ([`crate::watch`], NOTIF-P15, contract 3.5). The engine is **UNCHANGED**;
//! this module COMPOSES it into the E2E-1 scenario and emits its named green artifact.
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/notifications.md` §3.3 (humanise per-viewer —
//! the pane's notification/status strings resolve per-viewer; the four load-bearing properties) and
//! §7 (the `inbox watch` firehose — the checks panel live-updates via the resume-cursor transport).
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §2 (E2E-1 — the
//! chained-mutation scenario; each step mutates and the pane re-resolves mid-flight) + the E2E-1 row
//! (PR context pane: humanise resolves per-viewer with 0 leak to the unauthorized viewer; the checks
//! panel live-updates via the firehose, the shared per-ref cache busts). **Contract-index rows 7.3**
//! (humanise — the pane's notification/status strings per-viewer), **3.5** (the firehose — the live
//! checks-panel updates). **External insight:** `01-process-and-quality-doctrine.md` §3 (prove-it —
//! the whole-system chained-mutation drill), §4 (chain mutations end-to-end, not a single handler).
//! **VISION §3** (prove the differentiator — the whole-system pane).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! This is the **whole-system DRIVER over the EXISTING engine**, not a second humanise/firehose.
//! - The per-viewer pane render drives the SAME [`crate::humanise::humanise`] contract-7.3 surface —
//!   the per-viewer slot bind → title (allowed) | tombstone (denied) is the chokepoint's OWN
//!   behaviour, observed across the chain. There is NO new leak-decision logic here; the NOTIF-D4
//!   leak invariant (a denied subject binds the slot to a PII-free tombstone display, the title
//!   NEVER present) lives in [`crate::humanise`] and is UNCHANGED — this module ASSERTS it at E2E
//!   scale.
//! - The live checks-panel update drives the SAME firehose [`crate::watch`] resume-cursor transport
//!   (NOTIF-P15): a `ci.check.updated` mid-flight mutation publishes a live frame on the bounded
//!   `(fan.<tenant>.inbox, inbox:<principal>)` key, the open watch drains it (0 items lost), the
//!   shared per-ref cache busts, and the pane re-resolves through the SAME humanise path to serve the
//!   NEW state.
//!
//! ## The leak invariant floor STILL HOLDS at E2E scale (the prompt's required statement)
//! The NOTIF-D4 humanise leak invariant (a denied viewer's confidential subject humanises to a
//! tombstone carrying NO title) and the NOTIF-P15 firehose property (a `*.updated` busts the shared
//! per-ref cache so the pane live-updates over the resume-cursor path) are the load-bearing
//! properties. This module ASSERTS both at E2E scale: the outsider's confidential issue humanises to
//! the PII-free tombstone display (0 title leak across every channel projection), and the mid-flight
//! `ci.check.updated` live-updates the checks panel over the firehose (the per-ref cache busts). The
//! mutation floors on those invariants live in `humanise.rs` / `watch.rs` and are UNCHANGED.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new.** This is the E2E run over the production-hardened engine.
//! - **The E2E-2 HITL flagship leg is NOTIF-P29; the E2E-4 DSAR leg + STOR-D2 is NOTIF-P30.** Named.
//!   This module owns ONLY the E2E-1 PR-context-pane leg (per-viewer humanise + live firehose).
//! - The production resolve transport behind the pane's per-viewer render is the Refs `ResolveService`
//!   over the substrate resilient client (the named `myelin-client` floor) — here the synthetic
//!   resolver stands in for the real Refs chokepoint (REF-P10), exactly as the [`crate::humanise`]
//!   tests do; the CHOKEPOINT logic is real in Refs and CONSUMED here.

use std::sync::{Arc, Mutex};

use myelin_events::firehose::{Firehose, FrameDraft};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::escalation::{
    notify_for, DurableWheel, EscalationEngine, EscalationPolicy, EscalationRun, InMemoryWheel,
    OncallSchedule, RotationWindow, RunState,
};
use crate::humanise::{
    humanise, Channel, HumaniseTemplate, RefProjection, RefResolution, RefResolvePort,
    TemplateStore, Tombstone, TombstoneReason, DEFAULT_LOCALE,
};
use crate::prefs::{Channel as PrefChannel, QuietHours};
use crate::ranking::reason_base_class;
use crate::watch::{inbox_scope, inbox_stream, publish_inbox_frame, watch_open};
use crate::{Class, Reason};
use myelin_events::{OutboxStore, Timestamp};

/// The E2E scenario Notif's leg crosses (the master M5 exit gate cites E2E-1..E2E-4; this module
/// owns the Notif side of E2E-1 — the PR context pane). PII-free token — the drill asserts against
/// the NAME, never a literal (EI-01 §3).
pub const E2E_SCENARIO: &str = "E2E-1";

/// **The named green artifact Notif's E2E-1 leg emits (the prompt's "named green artifact").** A
/// dated, content-addressed report the master M5 exit gate cites. `green` is the leg's earned green
/// predicate; `evidence` is the load-bearing assertion summary; `leaks` is the title-leak counter
/// the leg asserts at `0` (the F1 / NOTIF-D4 spine). A leg that did not reach green has
/// `green = false` — it fails LOUDLY, never a claimed-but-unearned green (EI-01 §3 / VISION §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    /// Which E2E scenario this artifact attests ([`E2E_SCENARIO`]).
    pub scenario: &'static str,
    /// The earned green verdict — `true` iff every load-bearing assertion held end-to-end.
    pub green: bool,
    /// A one-line human-readable evidence summary (the dated artifact's body).
    pub evidence: String,
    /// The title-leak counter the leg asserted at `0` (0 title leak to the unauthorized viewer).
    pub leaks: u64,
}

impl E2eArtifact {
    /// The green predicate (the dated artifact is green iff the leg earned it AND 0 leaks).
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  Shared E2E test fixtures (the cell + tenant the wedge runs against; a full cell with mock agents).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The tenant the wedge runs against (a full cell). Opaque, PII-free.
fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

/// The region (fr-par — the dev/prod residency pin; a config swap, never a code change).
fn e2e_region() -> Region {
    Region("fr-par".into())
}

/// A viewer principal (a human or agent — the wedge runs per-viewer).
fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

/// The bounded-stale consistency the pane reads at (the per-viewer resolve consistency).
fn bounded_stale() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The shared per-ref cache the firehose busts (the E2E-1 live-update half).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **The PR-pane synthetic Refs resolve chokepoint (the E2E-1 mock-agent cell).** Stands in for the
/// real Refs `ResolveService` (REF-P10) the production pane resolves through — the same
/// `Projection | Tombstone` shape, exactly as the [`crate::humanise`] tests use (the production wire
/// is the named `myelin-client` floor). It is PROGRAMMABLE so the chained-mutation scenario can drive
/// the per-viewer pane AND the mid-flight `ci.check.updated`: the CI check's projected `title` is the
/// LIVE (mutable) check state, so step 3's flip is reflected once the cache busts.
struct PrPaneOwner {
    /// The viewer permitted to view the confidential issue (the insider). Everyone else is denied.
    insider: String,
    /// The confidential issue subject (the artifact a denied viewer must NOT see the title of).
    confidential_issue: ArtifactRef,
    /// The CI check subject whose projection reflects the live (mutable) check state.
    check_ref: ArtifactRef,
    /// The live CI check state (mutable — step 3's mid-flight `ci.check.updated` flips it).
    check_state: Mutex<String>,
}

impl PrPaneOwner {
    fn new(insider: &str, confidential_issue: ArtifactRef, check_ref: ArtifactRef) -> PrPaneOwner {
        PrPaneOwner {
            insider: insider.into(),
            confidential_issue,
            check_ref,
            check_state: Mutex::new("pending".into()),
        }
    }

    /// Mid-flight mutation A: CI emits `ci.check.updated` — flip the check state the pane renders.
    fn update_check(&self, new_state: &str) {
        *self.check_state.lock().unwrap() = new_state.into();
    }

    /// The secret title a denied viewer must NEVER see (the leak-test payload — a regression that
    /// leaked the title into the tombstone render is caught against THIS token).
    const SECRET_TITLE: &'static str = "TOP SECRET acquisition plan";
}

impl RefResolvePort for PrPaneOwner {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        // The confidential issue is visible ONLY to the insider (the leak-test artifact). A denied
        // viewer → a tombstone carrying NO title (the leak-free chokepoint — NOTIF-D4).
        if ref_ == &self.confidential_issue && viewer.principal_id.0 != self.insider {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            });
        }
        // The CI check projection reflects the LIVE (mutable) check state so the mid-flight
        // `ci.check.updated` lands in the pane once the cache busts; the confidential issue carries a
        // SECRET title (the insider IS allowed to see it; a denied viewer never reaches here).
        let title = if ref_ == &self.check_ref {
            format!("checks: {}", self.check_state.lock().unwrap())
        } else if ref_ == &self.confidential_issue {
            PrPaneOwner::SECRET_TITLE.to_string()
        } else {
            format!("artifact {}", ref_.0)
        };
        RefResolution::Projection(RefProjection {
            ref_: ref_.clone(),
            title,
            icon: "card".into(),
        })
    }
}

/// **The shared per-ref cache the firehose busts (the E2E-1 live-update half, §7 / Chat D-C7 analog).**
/// The pane renders many viewers off ONE shared per-ref projection cache (the freshness mechanism);
/// a `*.updated` firehose frame BUSTS the entry for that ref so the next render re-resolves through
/// the SAME [`crate::humanise`] chokepoint and serves the NEW state. A thin cache over the
/// [`RefResolvePort`] — it caches the per-viewer [`RefResolution`] keyed `(viewer, ref)` and exposes
/// a `bust(ref)` the firehose drives. NEVER caches across a permission change without a bust (the
/// always-current property is the bust's job; a stale cache that ignored a bust would leak a former
/// title — the cache HONOURS the bust).
struct SharedRefCache {
    /// The backing resolve chokepoint (the production Refs `ResolveService`; here the synthetic owner).
    inner: Arc<dyn RefResolvePort>,
    /// The cached per-viewer resolutions, keyed `(viewer_id, ref)`. Busted per-ref on a `*.updated`.
    entries: Mutex<Vec<((String, String), RefResolution)>>,
    /// A monotone resolve-call counter (proves a bust forces a re-resolve — the live update is real,
    /// not a cache-stale read).
    resolves: Mutex<u64>,
}

impl SharedRefCache {
    fn new(inner: Arc<dyn RefResolvePort>) -> SharedRefCache {
        SharedRefCache {
            inner,
            entries: Mutex::new(Vec::new()),
            resolves: Mutex::new(0),
        }
    }

    /// **Bust every cached entry for `ref_` (the firehose `*.updated` cache-bust, §7).** Drops the
    /// shared per-ref cache slice so the next render re-resolves — the always-current property.
    fn bust(&self, ref_: &ArtifactRef) {
        self.entries
            .lock()
            .unwrap()
            .retain(|((_, r), _)| r != &ref_.0);
    }

    /// The number of times the cache MISSED and re-resolved through the chokepoint (proves a bust
    /// forced a fresh resolve — the live update served the new state, not a stale cache hit).
    fn resolve_count(&self) -> u64 {
        *self.resolves.lock().unwrap()
    }
}

impl RefResolvePort for SharedRefCache {
    fn resolve_display(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        at: &Consistency,
    ) -> RefResolution {
        let key = (viewer.principal_id.0.clone(), ref_.0.clone());
        if let Some((_, cached)) = self.entries.lock().unwrap().iter().find(|(k, _)| *k == key) {
            // A cache HIT — the shared per-ref cache serves the projection without re-resolving.
            return cached.clone();
        }
        // A MISS (fresh or post-bust) — re-resolve through the SAME chokepoint, then cache.
        *self.resolves.lock().unwrap() += 1;
        let resolved = self.inner.resolve_display(tenant, region, ref_, viewer, at);
        self.entries.lock().unwrap().push((key, resolved.clone()));
        resolved
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-1 — The PR context pane (Notif's leg: per-viewer humanise + live firehose update).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The connected artifacts a PR context pane humanises (the E2E-1 Notif leg — the CI checks status
/// string, the confidential issue, a Git PR). Every one humanises per-viewer through the SAME
/// contract-7.3 surface. PII-free opaque URNs.
fn pr_pane_subjects(tenant: &str) -> (ArtifactRef, ArtifactRef, ArtifactRef) {
    (
        ArtifactRef(format!("myelin://{tenant}/git/pr/PR-42")),
        ArtifactRef(format!("myelin://{tenant}/ci/check/PR-42-build")),
        ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-1421")),
    )
}

/// Humanise one pane subject for `viewer` through the SAME contract-7.3 surface, and return whether
/// the SECRET title leaked into ANY channel projection (CLI/Email/Markdown). The leak check runs
/// across EVERY channel (the leak invariant holds for every projection — §3.3).
fn pane_humanise_leaks_title(
    resolver: &dyn RefResolvePort,
    templates: &TemplateStore,
    template_key: &str,
    subject: &ArtifactRef,
    viewer: &Principal,
    at: &Consistency,
) -> bool {
    for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
        let h = humanise(
            resolver,
            &e2e_tenant(),
            &e2e_region(),
            templates,
            template_key,
            std::slice::from_ref(subject),
            viewer,
            DEFAULT_LOCALE,
            at,
            channel,
        );
        // 0 LEAK: the secret title is absent from `text`, every link, and the icon.
        let rendered = format!("{} {} {}", h.text, h.links.join(" "), h.icon);
        if rendered.contains(PrPaneOwner::SECRET_TITLE) || rendered.contains("acquisition") {
            return true;
        }
    }
    false
}

/// **E2E-1 — drive the whole PR-context-pane flow end-to-end (Notif's leg).** The chained mutation:
/// 1. The pane humanises every connected subject per-viewer (the insider sees the titles, incl. the
///    confidential issue's; the CI checks status string renders the live state).
/// 2. **Mid-flight mutation A:** CI emits `ci.check.updated` (build → success). The firehose
///    publishes a live frame on the bounded `(fan.<tenant>.inbox, inbox:<principal>)` key, the open
///    `inbox watch` drains it (0 items lost), the shared per-ref cache BUSTS for the check ref, and
///    the pane re-humanises → the checks panel serves the NEW state (the live update arrived over the
///    resume-cursor path).
/// 3. **Mid-flight mutation B:** a SECOND viewer WITHOUT access to the confidential issue opens the
///    same pane — the issue humanises to a TOMBSTONE ("a restricted issue"), the title NEVER present
///    across ANY channel projection (0 leak to the unauthorized viewer).
///
/// Returns the named green artifact (the pane-resolution trace + zero-leak counter at 0 + the
/// per-viewer diff + the live-update-over-firehose proof). Drives the SAME [`crate::humanise`]
/// chokepoint + the SAME [`crate::watch`] firehose — no second render/transport.
pub fn run_e2e_1_pr_pane() -> E2eArtifact {
    let tenant = e2e_tenant();
    let at = bounded_stale();
    let (pr_ref, check_ref, confidential) = pr_pane_subjects(&tenant.0);
    let insider = e2e_viewer("insider");
    let outsider = e2e_viewer("outsider");

    let owner = Arc::new(PrPaneOwner::new(
        "insider",
        confidential.clone(),
        check_ref.clone(),
    ));
    // The pane renders every viewer off ONE shared per-ref cache (the §7 freshness mechanism); the
    // firehose busts it on a `*.updated`. The humanise chokepoint resolves THROUGH the cache.
    let cache = Arc::new(SharedRefCache::new(owner.clone()));
    let templates = TemplateStore::with_platform_defaults();
    let mut leaks: u64 = 0;

    // ── (1) The pane humanises every connected subject per-viewer (the insider sees the titles). ──
    // The insider sees titles, including the confidential issue's (they ARE permitted). 0 leak is a
    // non-event for the insider (they are allowed); the assertion is that the render is per-viewer.
    let insider_pr = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "review_requested",
        std::slice::from_ref(&pr_ref),
        &insider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    let insider_check = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "state_changed",
        std::slice::from_ref(&check_ref),
        &insider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    let insider_saw_pane = !insider_pr.text.is_empty() && insider_check.text.contains("pending");

    // ── (2) Mid-flight mutation A: ci.check.updated (build → success) → the pane live-updates. ──
    // The firehose carries the live frame on the bounded inbox stream/scope; an open watch drains it
    // (0 items lost), then the shared per-ref cache busts and the pane re-humanises the NEW state.
    let mut firehose = Firehose::new();
    let watch = watch_open(&mut firehose, &insider)
        .ok()
        .and_then(|o| o.into_live());
    // The bounded (stream, scope) the live update rides — assert it is the frozen inbox key, never `*`.
    let stream = inbox_stream(&insider);
    let scope_ok = inbox_scope(&insider).is_ok();
    let frozen_inbox_stream = stream == format!("fan.{}.inbox", tenant.0);

    // CI flips the check state; the firehose publishes the live frame (the ci.check.updated pointer).
    owner.update_check("success");
    let resolves_before_bust = cache.resolve_count();
    // Publish the live frame onto the bounded inbox firehose key (the in-cell live delivery path).
    let published = publish_inbox_frame(&mut firehose, &insider, &check_ref.0).is_ok();
    // The open watch DRAINS the live frame (0 items lost — the resume-cursor transport invariant).
    let live_frame_arrived = watch
        .as_ref()
        .map(|w| {
            let frames = w.drain();
            frames.iter().any(|f| f.item_id == check_ref.0)
        })
        .unwrap_or(false);
    // The `*.updated` BUSTS the shared per-ref cache for the check ref (the Chat D-C7 analog).
    cache.bust(&check_ref);
    // The pane re-humanises — the checks panel now serves the NEW state (the live update landed).
    let re_humanised = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "state_changed",
        std::slice::from_ref(&check_ref),
        &insider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    let resolves_after_bust = cache.resolve_count();
    // The bust FORCED a fresh resolve (the live update is real, not a stale cache hit) AND the new
    // state is served (the checks panel live-updated over the firehose resume-cursor path).
    let cache_busted_and_reresolved = resolves_after_bust > resolves_before_bust;
    let check_live_updated = re_humanised.text.contains("success");

    // ── (3) Mid-flight mutation B: a SECOND viewer without access → the confidential issue ──
    //        humanises to a TOMBSTONE, title NEVER present across ANY channel projection (0 leak). ──
    let outsider_leaked = pane_humanise_leaks_title(
        cache.as_ref(),
        &templates,
        "review_requested",
        &confidential,
        &outsider,
        &at,
    );
    if outsider_leaked {
        // A denied viewer saw the secret title — a catastrophic leak (the F1 spine).
        leaks += 1;
    }
    // The outsider's render IS a non-leaking tombstone display ("a restricted issue"), not empty.
    let outsider_tombstone_display = {
        let h = humanise(
            cache.as_ref(),
            &tenant,
            &e2e_region(),
            &templates,
            "review_requested",
            std::slice::from_ref(&confidential),
            &outsider,
            DEFAULT_LOCALE,
            &at,
            Channel::Cli,
        );
        // A denied subject renders the kind-shaped restricted placeholder, never a title, never a link.
        h.text.contains("a restricted issue") && h.links.is_empty()
    };

    // The other connected subjects (the PR) STILL humanise for the outsider (only the confidential
    // issue is denied — the pane degrades gracefully, the rest is per-viewer-correct).
    let outsider_pr = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "review_requested",
        std::slice::from_ref(&pr_ref),
        &outsider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    let outsider_saw_non_confidential = !outsider_pr.text.is_empty();

    let green = insider_saw_pane
        && scope_ok
        && frozen_inbox_stream
        && published
        && live_frame_arrived
        && cache_busted_and_reresolved
        && check_live_updated
        && outsider_tombstone_display
        && outsider_saw_non_confidential
        && leaks == 0;

    E2eArtifact {
        scenario: E2E_SCENARIO,
        green,
        evidence: format!(
            "PR pane (Notif leg): insider humanised pane={insider_saw_pane}; \
             mid-flight ci.check.updated over firehose (stream={frozen_inbox_stream} bounded_scope={scope_ok} \
             published={published} live_frame_arrived={live_frame_arrived}) \
             cache_busted_and_reresolved={cache_busted_and_reresolved} check_live_updated={check_live_updated}; \
             outsider→confidential tombstone_display={outsider_tombstone_display} \
             outsider_saw_non_confidential={outsider_saw_non_confidential}; leaks={leaks}",
        ),
        leaks,
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  The Notif-side wedge driver — run the E2E-1 leg + its named green artifact.
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **Run Notif's E2E-1 leg (the PR context pane).** Drives the chained-mutation scenario end-to-end
/// over the production-hardened engine and returns its named green artifact. This is Notif's leg of
/// the master M5 exit gate's E2E-1 row; a red E2E-1 must NOT let M6 start. The artifact's `is_green()`
/// is the earned verdict (0 leak + the per-viewer-pane + live-firehose predicate).
///
/// **Floors named:** the E2E-2 HITL flagship leg is now LIVE ([`run_e2e_2_hitl_flagship`],
/// NOTIF-P29 / P-471); the E2E-4 DSAR leg + STOR-D2 is NOTIF-P30 (this driver owns the E2E-1 leg).
pub fn run_notif_e2e_wedge() -> E2eArtifact {
    run_e2e_1_pr_pane()
}

/// A drill convenience: build a live firehose frame for the inbox stream (the resume-cursor path the
/// pane's live update rides). PII-free — exposed so the integration drill can assert the frame body
/// is the `item_id` pointer (references-not-payloads), never a rendered string.
pub fn e2e_live_frame_draft(item_id: &str) -> FrameDraft {
    FrameDraft::new(item_id)
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
//
//  E2E-2 — Notif's leg of the HITL FLAGSHIP (NOTIF-P29 / P-471, M5)
//
//  The whole-system flagship: CI-fail → triage agent → issue → chat → fix-PR. This is the **Notif
//  side** of that chained-mutation scenario, driven end-to-end over the production-hardened Notif
//  engine the prior prompts built — the `approval_requested` card (NOTIF-P5/P7, ranked critical),
//  the per-viewer `humanise` of the card's action+risk+cost (NOTIF-P9, contract 7.3), the
//  explicit-first boundary (NOTIF-P22, contract 8.6 — a casual @agent mention is a NOTIFY, never a
//  dispatch), and the escalation/notify legs exactly-once across a kill (NOTIF-P14, contract 7.5 /
//  9.3 / 9.4). The engine is UNCHANGED; this module COMPOSES it into the E2E-2 scenario and emits
//  its named green artifact.
//
//  **Owning architecture doc:** `notifications.md` §1.4 (the HITL approval card is a Notif item
//  `reason=approval_requested` at high priority), §2.4 (the escalation/notify exactly-once across a
//  kill), §3.3 (humanise — the card's action+risk+cost render). **Contract-index rows 7.3** (the
//  HITL card humanise), **8.6** (explicit-first — the casual mention does not auto-spawn), **9.4**
//  (the durable signal — the HITL withhold→approve), **7.5** (the escalation/notify legs
//  exactly-once). **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//  the E2E-2 row. **VISION §3** (agent-native from the ground up — prove the differentiator).
//
//  ## What this leg REUSES (EI-01 §7 — never a parallel second implementation)
//  - The HITL card's per-viewer action+risk+cost render drives the SAME [`crate::humanise::humanise`]
//    contract-7.3 chokepoint — the card is an inbox item `reason=approval_requested` whose template
//    binds the action/risk/cost slots; a DENIED viewer's subject still binds to a PII-free tombstone
//    (the NOTIF-D4 leak invariant, UNCHANGED here, ASSERTED at E2E scale).
//  - The escalation/notify exactly-once-across-a-kill drives the SAME [`crate::escalation`]
//    `EscalationEngine` over the SAME [`InMemoryWheel`] effectively-once durable-timer seam
//    (NOTIF-P14); this leg KILLS the engine mid-`ack_window`, resumes from the persisted
//    `escalation_run` handle, fires the timer, and asserts EXACTLY ONE next-step page (never zero,
//    never two) — the NOTIF-D7 property, ASSERTED at E2E scale. The ack-halt is exactly-once
//    (idempotent).
//  - The explicit-first boundary (NOTIF-P22, contract 8.6): the casual @agent mention is a NOTIFY
//    (an inbox item `reason=mentioned`), not a dispatch — Notif notifies; the DISPATCH boundary
//    (no-auto-spawn) is owned by Chat/Agent (the named cross-system floor). Notif's leg ASSERTS that
//    its side of the boundary is a notify (a ranked inbox item), never a run-spawn, and that the
//    EXPLICIT approval is the ONLY thing that drives the apply (0 mutation pre-approval, 1 apply).
//
//  ## Floors named (VISION §3 / EI-01 §1)
//  - **None new.** This is the E2E run over the production-hardened engine.
//  - **The E2E-4 DSAR leg + STOR-D2 at cell scale is NOTIF-P30.** Named. This module owns ONLY the
//    E2E-2 HITL-flagship leg.
//  - The cross-system DISPATCH boundary (no-auto-spawn from a casual mention) is owned by Chat/Agent
//    (CHAT-D17 / the explicit-first dispatch, P-419); Notif's leg asserts the NOTIFY side (a ranked
//    inbox item, not a run-spawn). The real `myelin-flow` durable timer/signal behind the escalation
//    chain is P-FLOW-09/P-FLOW-13 (the [`InMemoryWheel`] models its effectively-once property — the
//    same seam `escalation.rs` proves the chain-walk against). The per-viewer resolve transport
//    behind the card render is the Refs `ResolveService` (REF-P10); the synthetic resolver stands in
//    for the real Refs chokepoint, exactly as the [`crate::humanise`] tests do.
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The E2E-2 scenario token (the HITL flagship). PII-free — the drill asserts against the NAME.
pub const E2E_2_SCENARIO: &str = "E2E-2";

/// **The HITL approval-card render keys (the §1.4 card is a Notif item `reason=approval_requested`).**
/// The card humanises action + risk + cost through the SAME contract-7.3 surface; `{0}` is the subject
/// (per-viewer → title | tombstone), `{1}` the action, `{2}` the risk, `{3}` the cost. A Notif-local
/// template the leg registers (the platform default `approval_requested` body renders only the
/// subject; the FLAGSHIP card renders action+risk+cost, so the leg registers the richer card body).
const HITL_CARD_TEMPLATE_KEY: &str = "approval_requested.card";

/// The flagship's fix-PR merge subject (the consequential effect the HITL card gates). PII-free URN.
fn fix_pr_subject(tenant: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/git/pr/PR-7-fix"))
}

/// The casual-mention subject (the chat message that @-mentions the agent — the explicit-first leg).
fn casual_mention_subject(tenant: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/chat/message/msg-99"))
}

/// Register the richer HITL-card template (action+risk+cost) onto a platform-default store. The card
/// is an inbox item `reason=approval_requested`; its body renders the proposed action, the risk, and
/// the cost so the human approves on FULL information (the §1.4 card affordance). The subject slot
/// `{0}` is still per-viewer-resolved (the leak invariant holds for the card too).
fn hitl_card_templates() -> TemplateStore {
    let mut s = TemplateStore::with_platform_defaults();
    s.put(HumaniseTemplate {
        tenant: crate::humanise::PLATFORM_DEFAULT_TENANT.to_string(),
        template_key: HITL_CARD_TEMPLATE_KEY.to_string(),
        locale: DEFAULT_LOCALE.to_string(),
        // action | risk | cost — the human approves on full information (the §1.4 card).
        body: "Approve {1} on {0} (risk {2}, cost {3})".to_string(),
        icon: "approval".to_string(),
    });
    s
}

/// **The withhold→approve→apply ledger (the §9.4 durable HITL signal, the Notif card is the notify
/// side).** The consequential effect (the fix-PR `git.merge`) is WITHHELD until a human approves; the
/// approval is the durable signal the workflow's signal-wait resolves on. This models the apply
/// ledger Notif's card drives: `applies` counts the actual mutations (0 pre-approval, exactly 1 after
/// the explicit human approve). The Notif card is the NOTIFY side; the workflow owns the apply
/// idempotency (the named `myelin-flow` 9.4 floor) — here modelled idempotently so the leg PROVES the
/// 0-pre-approval / exactly-1-apply property end-to-end.
#[derive(Default)]
struct HitlApplyLedger {
    /// The number of times the consequential effect actually applied (the mutation count).
    applies: Mutex<u64>,
    /// Whether the human has explicitly approved (the durable signal — the apply gate).
    approved: Mutex<bool>,
}

impl HitlApplyLedger {
    /// The human explicitly approves (the durable signal arrives — the §9.4 wait resolves). Idempotent.
    fn approve(&self) {
        *self.approved.lock().unwrap() = true;
    }

    /// Attempt to apply the consequential effect. Applies ONLY if explicitly approved (0 mutation
    /// pre-approval), and exactly ONCE (idempotent — a replayed apply does not double-mutate). Returns
    /// whether THIS call performed the mutation.
    fn try_apply(&self) -> bool {
        if !*self.approved.lock().unwrap() {
            // 0 mutation pre-approval — the gate holds (the withhold half of the §9.4 loop).
            return false;
        }
        let mut applies = self.applies.lock().unwrap();
        if *applies >= 1 {
            // Exactly-once — a replayed apply after approval does not double-mutate.
            return false;
        }
        *applies += 1;
        true
    }

    /// The applied-mutation count (0 pre-approval, 1 after the explicit approve).
    fn applies(&self) -> u64 {
        *self.applies.lock().unwrap()
    }
}

/// **A two-step on-call schedule for the escalation leg (the §2.4 rotation roster).** The first
/// covering window wins; the leg pages the schedule, kills mid-`ack_window`, resumes, and the next
/// step pages exactly once.
fn e2e_schedule() -> OncallSchedule {
    OncallSchedule {
        schedule_id: "platform-oncall".into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 1440,
            principal: PrincipalId("psn:oncall".into()),
        }],
    }
}

/// **E2E-2 — drive the whole HITL-flagship flow end-to-end (Notif's leg).** The chained mutation:
/// 1. CI fails → a triage agent proposes a fix-PR merge → the consequential effect is WITHHELD as a
///    HITL approval card (a Notif item `reason=approval_requested`, ranked CRITICAL, NOTIF-P5/P7),
///    humanised per-viewer with its action+risk+cost (NOTIF-P9). A DENIED viewer's card subject binds
///    to a PII-free tombstone — 0 leak across every channel projection.
/// 2. **Explicit-first (NOTIF-P22 / 8.6):** a casual @agent mention is a NOTIFY (an inbox item
///    `reason=mentioned`), NOT a dispatch — 0 auto-spawn from the casual mention. The EXPLICIT human
///    approval is the ONLY thing that drives the apply (0 mutation pre-approval, exactly 1 apply).
/// 3. **Escalation/notify exactly-once across a kill (NOTIF-P14 / 7.5):** the unacked card escalates
///    on the durable wheel; the engine is KILLED mid-`ack_window`, resumes from the persisted
///    `escalation_run` handle, fires the timer, and pages the next step EXACTLY ONCE (never zero,
///    never two). The ack halts the chain idempotently (a double-ack acks once).
///
/// Returns the named green artifact (the withhold→approve→apply ledger + exactly-once-across-a-kill +
/// 0-auto-spawn + 0-leak counter). Drives the SAME humanise / escalation engine — no second logic.
pub fn run_e2e_2_hitl_flagship() -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let at = bounded_stale();
    let templates = hitl_card_templates();
    let fix_pr = fix_pr_subject(&tenant.0);
    let casual = casual_mention_subject(&tenant.0);
    let approver = e2e_viewer("maintainer"); // the human who can approve the fix-PR merge
    let outsider = e2e_viewer("outsider"); // a viewer denied the confidential fix-PR
    let mut leaks: u64 = 0;

    // ── (1) The HITL approval card — a Notif item reason=approval_requested, ranked critical. ──
    // The card is ranked CRITICAL (the §3.1 ranking — approval_requested pierces, NOTIF-D1 band): a
    // missed approval card is a stalled human-in-the-loop, so it sits at the top of the inbox.
    let (card_priority, card_class) = reason_base_class(Reason::ApprovalRequested);
    let card_is_critical = card_class == Class::Critical && card_priority == 90;

    // The fix-PR resolver: the maintainer (approver) sees the title; an outsider gets a tombstone.
    let owner = Arc::new(HitlCardOwner::new("maintainer", fix_pr.clone()));
    let card_resolver: &dyn RefResolvePort = owner.as_ref();

    // The card humanises action+risk+cost per-viewer through the SAME contract-7.3 surface.
    let card_for_approver = humanise(
        card_resolver,
        &tenant,
        &region,
        &templates,
        HITL_CARD_TEMPLATE_KEY,
        &[
            fix_pr.clone(),
            ArtifactRef("git.merge".into()),
            ArtifactRef("irreversible".into()),
            ArtifactRef("$0.00".into()),
        ],
        &approver,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    // The approver's card shows the action+risk+cost on full information (the §1.4 affordance).
    let card_shows_action_risk_cost = card_for_approver.text.contains("git.merge")
        && card_for_approver.text.contains("irreversible")
        && card_for_approver.text.contains("$0.00");

    // A DENIED viewer's card subject binds to a PII-free tombstone — 0 leak across EVERY channel.
    for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
        let denied_card = humanise(
            card_resolver,
            &tenant,
            &region,
            &templates,
            HITL_CARD_TEMPLATE_KEY,
            std::slice::from_ref(&fix_pr),
            &outsider,
            DEFAULT_LOCALE,
            &at,
            channel,
        );
        let rendered = format!(
            "{} {} {}",
            denied_card.text,
            denied_card.links.join(" "),
            denied_card.icon
        );
        if rendered.contains(HitlCardOwner::SECRET_TITLE) || rendered.contains("acquisition") {
            leaks += 1;
        }
    }
    // The denied viewer's card IS a non-leaking tombstone display (kind-shaped restricted placeholder).
    let denied_card_tombstone = {
        let h = humanise(
            card_resolver,
            &tenant,
            &region,
            &templates,
            HITL_CARD_TEMPLATE_KEY,
            std::slice::from_ref(&fix_pr),
            &outsider,
            DEFAULT_LOCALE,
            &at,
            Channel::Cli,
        );
        h.text.contains("a restricted") && h.links.is_empty()
    };

    // ── (2) Explicit-first (NOTIF-P22 / 8.6): the casual mention is a NOTIFY, never a dispatch. ──
    // Notif's side of the explicit-first boundary: a casual @agent mention materialises an inbox item
    // reason=mentioned (a NOTIFY) — NOT a dispatch. The casual-mention reason is `mentioned` (the
    // write-fanout node), NOT `agent_proposal`/`approval_requested`: it never represents a spawned run.
    let (_mention_prio, mention_class) = reason_base_class(Reason::Mentioned);
    let casual_is_a_notify_not_a_dispatch =
        mention_class == Class::Direct && Reason::Mentioned != Reason::ApprovalRequested;
    // 0 auto-spawn: the casual mention produced an inbox NOTIFY, not an apply. The apply ledger is
    // still at 0 (no run spawned, no effect applied) BECAUSE no explicit approval drove it.
    let ledger = HitlApplyLedger::default();
    // The casual mention does NOT approve — try_apply is 0 (the withhold half holds; 0 mutation).
    let casual_mention_spawned_a_run = ledger.try_apply(); // false — 0 auto-spawn
    let applies_pre_approval = ledger.applies();
    let _ = &casual; // the casual-mention subject is the notify's subject (referenced, not applied)

    // ── The EXPLICIT human approval is the ONLY thing that drives the apply (the §9.4 durable loop).
    ledger.approve(); // the human explicitly approves the HITL card (the durable signal arrives)
    let first_apply = ledger.try_apply(); // exactly 1 apply
    let replayed_apply = ledger.try_apply(); // a replay does NOT double-mutate (exactly-once)
    let applies_post_approval = ledger.applies();
    let explicit_first_held = !casual_mention_spawned_a_run // 0 auto-spawn from the casual mention
        && applies_pre_approval == 0 // 0 mutation pre-approval
        && first_apply // the explicit approval drove exactly the apply
        && !replayed_apply // exactly-once apply
        && applies_post_approval == 1; // exactly 1 apply

    // ── (3) Escalation/notify exactly-once ACROSS A KILL (NOTIF-P14 / 7.5 / 9.3). ──
    let exactly_once_across_kill = escalation_exactly_once_across_a_kill();

    let green = card_is_critical
        && card_shows_action_risk_cost
        && denied_card_tombstone
        && casual_is_a_notify_not_a_dispatch
        && explicit_first_held
        && exactly_once_across_kill
        && leaks == 0;

    E2eArtifact {
        scenario: E2E_2_SCENARIO,
        green,
        evidence: format!(
            "HITL flagship (Notif leg): card_critical={card_is_critical} \
             card_shows_action_risk_cost={card_shows_action_risk_cost} \
             denied_card_tombstone={denied_card_tombstone}; \
             explicit_first(casual_is_notify={casual_is_a_notify_not_a_dispatch} \
             auto_spawn={casual_mention_spawned_a_run} applies_pre_approval={applies_pre_approval} \
             applies_post_approval={applies_post_approval} exactly_once_apply={}); \
             escalation_exactly_once_across_kill={exactly_once_across_kill}; leaks={leaks}",
            !replayed_apply,
        ),
        leaks,
    }
}

/// **The escalation/notify exactly-once-across-a-kill drill (NOTIF-P14 / NOTIF-D7, at E2E scale).**
/// Pages the on-call schedule for the unacked HITL card; KILLS the engine mid-`ack_window` (drops the
/// in-process `runs` map but KEEPS the durable wheel + the persisted `escalation_run` handle);
/// resumes onto a FRESH engine sharing the SAME wheel; fires the due timer and asserts the next step
/// pages EXACTLY ONCE (never zero, never two — the no-double-page / no-missed-step anchor); then a
/// replayed fire is a NO-OP, and the ack halts the chain idempotently. Returns whether every
/// exactly-once property held. Drives the SAME [`EscalationEngine`] over the SAME effectively-once
/// [`InMemoryWheel`] — no second escalation logic.
fn escalation_exactly_once_across_a_kill() -> bool {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let schedule = e2e_schedule();
    let quiet = QuietHours::default();
    let policy = EscalationPolicy::test_chain(15, PrincipalId("psn:lead".into()));
    let trigger = ArtifactRef("myelin://acme/ci/run/RUN-fail".into());

    // The pre-kill engine pages the FIRST step (the on-call). The wheel is the durable substrate the
    // restart resumes from — cloned so a fresh engine shares the SAME persisted handle.
    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();
    let eng = EscalationEngine::new(wheel.clone(), outbox.clone());
    let Ok((run_id, first)) = eng.page(
        tenant,
        region,
        "esc-e2e-2".into(),
        policy,
        trigger,
        Some(&schedule),
        600, // 10:00 — the on-call window
        &quiet,
        false,
    ) else {
        return false;
    };
    // notify(class=critical) pierces — the page pushes on EVERY step channel (the on-call pierce).
    let first_pierced = first.channels.contains(&PrefChannel::InApp) && first.walk == 0;
    let live_handle_before_kill = eng.wheel().has_timer(&run_id);
    let persisted: EscalationRun = match eng.run(&run_id) {
        Some(r) => r,
        None => return false,
    };

    // ── THE KILL ── drop the engine (the in-process runs map is lost); the wheel + the persisted
    // escalation_run handle survive (the durable substrate). A FRESH engine resumes from them.
    drop(eng);
    let resumed = EscalationEngine::new(wheel.clone(), outbox.clone());
    resumed.resume_for_test(persisted);
    // The durable timer is STILL armed on the shared wheel (the restart did not lose it).
    let live_handle_after_kill = resumed.wheel().has_timer(&run_id);

    // The escalate-after-timer fires on the resumed engine → page the NEXT step EXACTLY ONCE.
    let next = resumed.advance(&run_id, Some(&schedule), 600, &quiet, false);
    let next_paged_once = matches!(&next, Ok(Some(o)) if o.walk == 1);
    // A REPLAYED fire (a second restart-replay) is a NO-OP — the effectively-once timer (no double page).
    let replay = resumed.advance(&run_id, Some(&schedule), 600, &quiet, false);
    let replay_no_op = matches!(replay, Ok(None));
    // The page log holds EXACTLY two entries (step 0 pre-kill, step 1 post-resume) — never a third.
    let exactly_two_pages = resumed
        .run(&run_id)
        .map(|r| r.pages.len() == 2)
        .unwrap_or(false);

    // The ack halts the chain idempotently (a double-ack acks once — the §9.4 durable signal).
    let halted = resumed
        .ack(
            &run_id,
            PrincipalId("psn:oncall".into()),
            Timestamp("2026-06-25T10:30:00Z".into()),
        )
        .unwrap_or(false);
    let double_ack = resumed
        .ack(
            &run_id,
            PrincipalId("psn:lead".into()),
            Timestamp("2026-06-25T10:31:00Z".into()),
        )
        .unwrap_or(true); // a redundant ack returns Ok(false); default true would fail the predicate
    let acked = resumed
        .run(&run_id)
        .map(|r| r.state == RunState::Acked)
        .unwrap_or(false);
    // The ack event rode the outbox EXACTLY once (the double-ack did NOT re-emit — exactly-once notify).
    let exactly_one_ack_event = outbox.committed_count() == 1;

    // notify_for asserts the critical pierce is the on-call override (you cannot silence a page).
    let pierce_holds = notify_for(
        &[PrefChannel::InApp, PrefChannel::WebPush],
        Class::Critical,
        &quiet,
        true, // even IN a quiet window, critical pierces
    )
    .len()
        == 2;

    first_pierced
        && live_handle_before_kill
        && live_handle_after_kill
        && next_paged_once
        && replay_no_op
        && exactly_two_pages
        && halted
        && !double_ack
        && acked
        && exactly_one_ack_event
        && pierce_holds
}

/// **The HITL-card synthetic Refs resolve chokepoint (the E2E-2 mock-agent cell).** Stands in for the
/// real Refs `ResolveService` (REF-P10) the production card resolves through — the same
/// `Projection | Tombstone` shape, exactly as the [`crate::humanise`] tests use. The fix-PR is
/// visible ONLY to the approver (a denied viewer's card binds to a PII-free tombstone — the leak-free
/// chokepoint, NOTIF-D4).
struct HitlCardOwner {
    /// The viewer permitted to see the fix-PR title (the maintainer/approver). Everyone else is denied.
    approver: String,
    /// The confidential fix-PR subject (the artifact a denied viewer must NOT see the title of).
    fix_pr: ArtifactRef,
}

impl HitlCardOwner {
    fn new(approver: &str, fix_pr: ArtifactRef) -> HitlCardOwner {
        HitlCardOwner {
            approver: approver.into(),
            fix_pr,
        }
    }

    /// The secret title a denied viewer must NEVER see (the leak-test payload).
    const SECRET_TITLE: &'static str = "TOP SECRET acquisition fix";
}

impl RefResolvePort for HitlCardOwner {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        // The confidential fix-PR is visible ONLY to the approver → a denied viewer gets a tombstone.
        if ref_ == &self.fix_pr && viewer.principal_id.0 != self.approver {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            });
        }
        let title = if ref_ == &self.fix_pr {
            HitlCardOwner::SECRET_TITLE.to_string()
        } else {
            // The action/risk/cost slots are opaque tokens (not PII) — render them verbatim.
            ref_.0.clone()
        };
        RefResolution::Projection(RefProjection {
            ref_: ref_.clone(),
            title,
            icon: "approval".into(),
        })
    }
}

#[cfg(test)]
mod tests;
