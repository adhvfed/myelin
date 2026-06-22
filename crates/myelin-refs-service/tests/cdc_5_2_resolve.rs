//! **REF-P10 / P-159 — the per-viewer resolution chokepoint (contract 5.2) provider+consumer CDC
//! pair.**
//!
//! Contract 5.2 is `resolve(ref, viewer, mode) -> Projection | Tombstone` (OWNED by Refs). Unlike a
//! bus-event CDC (a serialized envelope), 5.2 is a per-viewer REQUEST/RESPONSE shape — so this CDC pair
//! pins the **response variants + the leak invariant** the provider (Refs) promises and the consumers
//! (Chat unfurl / PR context pane / KN embed / Notif Display) depend on:
//!
//! - **PROVIDER (Refs):** `resolve` returns exactly one of two arms — a [`Resolution::Projection`]
//!   (the per-viewer unfurl, carrying `{ref, title, state, icon, render_hint, sub_anchor?, flag}`) or a
//!   [`Resolution::Tombstone`] (carrying ONLY `{root, reason}` — NO content). The provider promises:
//!   denied → `Tombstone{denied}`; allowed+live → `Projection`; the §4.6 ladder
//!   (root_gone/sub_gone/erased) → the matching tombstone reason.
//! - **CONSUMER (an unfurl renderer):** a renderer that pattern-matches the two arms — it renders the
//!   projection fields on `Projection`, and a non-leaking "referenced <root>" placeholder on
//!   `Tombstone`. The consumer NEVER sees a title for a denied viewer (the leak invariant it relies on
//!   to be safe to embed a confidential ref in a public channel).
//!
//! The CONTRACT this pair freezes: **a denied viewer's `resolve` carries 0 content** — the consumer
//! can render any resolve result without a permission re-check, because the provider already
//! tombstoned the denied case. This is the load-bearing 5.2 promise (the chokepoint that makes every
//! unfurl non-leaking).

use std::sync::Arc;

use myelin_events::ArtifactRef;
use myelin_identity::{Decision, Permission, Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};

use myelin_refs_service::{
    bounded_stale, NoOpCacheRead, OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome,
    Resolution, ResolveMode, ResolveService, TombstoneReason,
};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN — LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}

/// The PROVIDER's owner-subsystem stand-in (the synthetic Git/KN/Chat `project` + Identity `check`).
/// Programmable: an allow-list + a project outcome. The production wire is the named ResilientClient
/// floor; this is the contract-shape provider.
struct ProviderOwner {
    allowed: Vec<String>,
    outcome: ProjectOutcome,
}
impl ProjectApi for ProviderOwner {
    fn check_view(
        &self,
        _t: &TenantId,
        _r: &Region,
        _o: &ArtifactRef,
        viewer: &Principal,
        _p: &Permission,
    ) -> Result<Decision, ProjectApiError> {
        if self.allowed.iter().any(|a| a == &viewer.principal_id.0) {
            Ok(Decision::Allow)
        } else {
            Ok(Decision::Deny)
        }
    }
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        _rf: &ArtifactRef,
        _v: &Principal,
        _m: ResolveMode,
    ) -> Result<ProjectOutcome, ProjectApiError> {
        Ok(self.outcome.clone())
    }
}

fn provider(allowed: Vec<&str>, outcome: ProjectOutcome) -> ResolveService {
    let owner = ProviderOwner {
        allowed: allowed.into_iter().map(String::from).collect(),
        outcome,
    };
    ResolveService::new(
        Arc::new(FailStaticAuthz::try_new(300, &threshold()).expect("valid bound")),
        Arc::new(NoOpCacheRead),
        Arc::new(owner),
        cell(),
    )
}

/// The CONSUMER: an unfurl renderer that pattern-matches the 5.2 response. It renders the projection
/// fields on `Projection`, and a non-leaking placeholder ("referenced <root>") on `Tombstone`. It
/// NEVER re-checks permission — it trusts the provider's chokepoint. Returns the rendered string the
/// end-user would see.
fn render_unfurl(r: &Resolution) -> String {
    match r {
        Resolution::Projection(p) => {
            format!("[{}] {} ({})", p.icon, p.title, p.state)
        }
        Resolution::Tombstone(t) => {
            // the non-leaking placeholder — only the OPAQUE root URN + the reason, never content.
            format!("referenced {} (unavailable: {:?})", t.root.0, t.reason)
        }
    }
}

fn live_secret() -> ProjectOutcome {
    ProjectOutcome::Live(OwnerProjection {
        title: "TOP SECRET acquisition plan".into(),
        state: "open".into(),
        icon: "lock".into(),
        render_hint: "issue-card".into(),
        sub_anchor: None,
        flag: None,
    })
}

/// **CDC 5.2 (provider/allowed arm):** an allowed viewer's `resolve` returns a `Projection` carrying
/// the owner's fields; the consumer renders them. The contract shape (the five projection fields) is
/// pinned.
#[test]
fn cdc_5_2_allowed_viewer_resolves_to_a_projection() {
    let svc = provider(vec!["insider"], live_secret());
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-secret".into());
    let root = ref_.clone();
    let r = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &root,
        &viewer("insider"),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(
        r.is_projection(),
        "PROVIDER: an allowed viewer resolves to a Projection"
    );
    // the CONSUMER renders the projection fields (the contract shape it depends on).
    let rendered = render_unfurl(&r);
    assert_eq!(rendered, "[lock] TOP SECRET acquisition plan (open)");
}

/// **CDC 5.2 (provider/denied arm — the LEAK INVARIANT):** a denied viewer's `resolve` returns a
/// `Tombstone{denied}` carrying ONLY the root + the reason — NO content. The consumer renders a
/// non-leaking placeholder, and the secret title is provably absent from what the consumer can see.
/// This is the contract promise that makes it SAFE to embed a confidential ref in a public channel.
#[test]
fn cdc_5_2_denied_viewer_resolves_to_a_tombstone_with_zero_content() {
    let svc = provider(vec!["insider"], live_secret()); // intruder NOT allowed
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-secret".into());
    let root = ref_.clone();
    let r = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &root,
        &viewer("intruder"),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(
        r.is_tombstone(),
        "PROVIDER: a denied viewer resolves to a Tombstone"
    );
    assert_eq!(r.tombstone_reason(), Some(TombstoneReason::Denied));
    // the CONSUMER's rendered output (everything it can possibly show) carries NO secret content.
    let rendered = render_unfurl(&r);
    assert!(
        !rendered.contains("SECRET") && !rendered.contains("acquisition"),
        "CONSUMER LEAK INVARIANT: the denied unfurl must not contain the secret title, got `{rendered}`"
    );
    // it DOES carry the opaque root so the embed degrades gracefully (§4.6 — "tombstone carries root").
    assert!(
        rendered.contains("myelin://acme/issue/issue/ENG-secret"),
        "the tombstone carries the root"
    );
}

/// **CDC 5.2 (provider/§4.6 ladder arms):** each non-LIVE owner outcome maps onto the contracted
/// tombstone reason the consumer pattern-matches (root_gone / sub_gone / erased). The consumer renders
/// the same non-leaking placeholder for all of them.
#[test]
fn cdc_5_2_sub_ladder_reasons_are_contracted() {
    for (outcome, want) in [
        (ProjectOutcome::RootGone, TombstoneReason::RootGone),
        (ProjectOutcome::SubGone, TombstoneReason::SubGone),
        (ProjectOutcome::Erased, TombstoneReason::Erased),
    ] {
        let svc = provider(vec!["insider"], outcome.clone());
        let ref_ = ArtifactRef("myelin://acme/knowledge/page/7c2#b9".into());
        let root = ArtifactRef("myelin://acme/knowledge/page/7c2".into());
        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        assert_eq!(
            r.tombstone_reason(),
            Some(want),
            "PROVIDER: outcome {outcome:?} → {want:?}"
        );
        // the consumer can render every tombstone reason without a content leak.
        let rendered = render_unfurl(&r);
        assert!(rendered.starts_with("referenced myelin://acme/knowledge/page/7c2"));
    }
}

/// **CDC 5.2 (consumer trust):** the consumer renders ANY resolve result WITHOUT re-checking
/// permission — it trusts the provider's chokepoint. This test proves the two arms are the COMPLETE
/// response surface (a `Resolution` is exhaustively `Projection | Tombstone`), so a consumer's
/// pattern-match is total and it can never accidentally render unfiltered content.
#[test]
fn cdc_5_2_consumer_trusts_the_chokepoint_total_match() {
    // the response surface is exactly two arms — a consumer matches both, exhaustively. (If a third
    // arm were added, this match would fail to compile — the contract surface is frozen at two.)
    let svc = provider(vec!["insider"], live_secret());
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-secret".into());
    let root = ref_.clone();
    for who in ["insider", "intruder"] {
        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer(who),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        // a total match — the consumer handles the full 5.2 response surface.
        let _rendered: String = match r {
            Resolution::Projection(p) => p.title,
            Resolution::Tombstone(t) => format!("{:?}", t.reason),
        };
    }
}
