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

use crate::humanise::{
    humanise, Channel, RefProjection, RefResolution, RefResolvePort, TemplateStore, Tombstone,
    TombstoneReason, DEFAULT_LOCALE,
};
use crate::watch::{inbox_scope, inbox_stream, publish_inbox_frame, watch_open};

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
/// **Floors named:** the E2E-2 HITL flagship leg is NOTIF-P29; the E2E-4 DSAR leg + STOR-D2 is
/// NOTIF-P30 (this driver owns ONLY the E2E-1 leg).
pub fn run_notif_e2e_wedge() -> E2eArtifact {
    run_e2e_1_pr_pane()
}

/// A drill convenience: build a live firehose frame for the inbox stream (the resume-cursor path the
/// pane's live update rides). PII-free — exposed so the integration drill can assert the frame body
/// is the `item_id` pointer (references-not-payloads), never a rendered string.
pub fn e2e_live_frame_draft(item_id: &str) -> FrameDraft {
    FrameDraft::new(item_id)
}

#[cfg(test)]
mod tests;
