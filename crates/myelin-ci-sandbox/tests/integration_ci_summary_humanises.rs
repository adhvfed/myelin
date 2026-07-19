//! # NOTIF-P23 / P-344 — CI's `CheckStatus.summary` `HumanisedRef` resolves THROUGH humanise
//!
//! **The X-1 / 5.9 invariant under test (the CI half of the M4 consumer accretion).** CI's
//! `CheckStatus.summary` is a `(template_key, args)` pair (a `HumanisedRef`), **never a raw string**
//! (`00-reconciliation-decisions.md` X-1, line 79). This integration test drives CI's
//! [`ci_summary`](myelin_ci_sandbox::ci_summary) `(template_key, args)` pair through the ONE
//! templating surface ([`myelin_notif::humanise`], contract 7.3) and asserts:
//!
//! 1. **It resolves through humanise** — CI's summary template_key (registered on the ONE surface by
//!    [`register_ci_summary_templates`](myelin_ci_sandbox::register_ci_summary_templates)) renders
//!    the registered per-viewer body, NOT the generic fallback. Threshold: 100% resolve through
//!    humanise.
//! 2. **A raw-string summary is REJECTED at the seam** — structurally: the `summary` field is typed
//!    `HumanisedRef` (`{template_key, args}`), so a raw string cannot be constructed or carried; the
//!    serialised summary has NO raw `text` field. Threshold: 0 raw-string summary accepted.
//! 3. **NOTIF-D4 (0 title/PII leak)** — a check on a PRIVATE repo humanises to a tombstone for a
//!    viewer who lacks access; the check title NEVER appears. The CI summary inherits the SAME leak
//!    floor every other consumer's summary rides (it goes through the ONE humanise pipeline).
//!
//! **The CDC pair (provider CI, consumer Notif — the X-1 cross-band seam's M4 PRODUCER half).** The
//! `(template_key, args)` CI produces is byte-faithfully the `(template_key, args)` Notif's humanise
//! consumes, AND the same shape the Git consumer (`myelin_git::check_status::HumanisedRef`) decodes —
//! the producer half and BOTH consumer halves agree on the ONE frozen 5.9 `HumanisedRef` shape.

use std::collections::BTreeMap;

use myelin_ci_sandbox::{
    ci_summary, register_ci_summary_templates, summary_template_key, CheckVerdict,
};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::{
    humanise, Channel, RefProjection, RefResolution, RefResolvePort, TemplateStore, Tombstone,
    TombstoneReason,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong() -> Consistency {
    Consistency {
        at_least: Zookie("zk-1".into()),
        mode: ConsistencyMode::Strong,
    }
}

/// The CI check subject — the canonical commit root plus check sub-anchor the summary is about.
fn check_subject() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/commit/core:abc123#check-build".into())
}

/// The title a denied viewer must NEVER see (the leak-test payload — a private-repo check title).
const SECRET_CHECK_TITLE: &str = "build #42 on the secret-acquisition branch";

/// A synthetic Refs resolve chokepoint (REF-P10 stands in here): per (viewer, ref) it returns a
/// projection (allowed) or a tombstone (denied). The SAME `Projection | Tombstone` shape the real
/// chokepoint returns — humanise binds each slot per-viewer through it.
struct SyntheticResolver {
    allowed_viewer: String,
}

impl RefResolvePort for SyntheticResolver {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if viewer.principal_id.0 == self.allowed_viewer {
            RefResolution::Projection(RefProjection {
                ref_: ref_.clone(),
                title: SECRET_CHECK_TITLE.to_string(),
                icon: "ci".to_string(),
            })
        } else {
            // Denied → a tombstone that structurally carries NO title (the leak invariant).
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

/// Render a CI summary `HumanisedRef` through the ONE templating surface for `viewer`.
fn humanise_ci_summary(
    resolver: &dyn RefResolvePort,
    templates: &TemplateStore,
    summary_key: &str,
    viewer: &Principal,
) -> myelin_notif::HumanisedString {
    // Slot 0 is the SUBJECT (the check sub-anchor) — resolved per-viewer. This is exactly how the
    // PR checks panel humanises a `CheckStatus.summary`: the template_key + the subject ref.
    let args = vec![check_subject()];
    humanise(
        resolver,
        &tenant(),
        &region(),
        templates,
        summary_key,
        &args,
        viewer,
        "en",
        &strong(),
        Channel::Markdown,
    )
}

/// **(1) CI's summary resolves THROUGH humanise to the registered body (allowed viewer).** A
/// `ci.check.failure` summary renders the registered "Checks failed on {0}" body with the check
/// title bound into slot 0 — NOT the generic fallback. 100% resolve through humanise.
#[test]
fn ci_summary_resolves_through_humanise_for_an_allowed_viewer() {
    let mut templates = TemplateStore::with_platform_defaults();
    register_ci_summary_templates(&mut templates);
    let resolver = SyntheticResolver {
        allowed_viewer: "psn:author".into(),
    };

    let summary = ci_summary(CheckVerdict::Failure, "build");
    assert_eq!(summary.template_key, "ci.check.failure");

    let out = humanise_ci_summary(
        &resolver,
        &templates,
        &summary.template_key,
        &viewer("psn:author"),
    );
    // the REGISTERED CI body rendered (not the generic fallback `ci.check.failure: {0}`).
    assert_eq!(out.text, format!("Checks failed on {SECRET_CHECK_TITLE}"));
    // the resolved subject yielded a click-route link (the allowed branch).
    assert_eq!(out.links, vec![check_subject().0]);
    assert_eq!(out.icon, "ci");
}

/// **(2) A raw-string summary is REJECTED at the seam (structural).** The summary CI builds is a
/// `(template_key, args)` pair — the serialised shape has NO raw `text` field a producer could carry
/// a "build failed" string in. 0 raw-string summary accepted.
#[test]
fn ci_summary_is_never_a_raw_string() {
    let summary = ci_summary(CheckVerdict::Failure, "build");
    let v = serde_json::to_value(&summary).unwrap();
    // exactly the frozen 5.9 HumanisedRef field set: {template_key, args}.
    assert!(v.get("template_key").is_some());
    assert!(v.get("args").is_some());
    // there is NO raw-summary string field — a "build failed" cannot ride the seam.
    assert!(v.get("text").is_none());
    assert!(v.get("summary").is_none());
    assert!(v.is_object());
    assert_eq!(v.as_object().unwrap().len(), 2, "only template_key + args");
}

/// **(3) NOTIF-D4 (0 title/PII leak): a check on a PRIVATE repo humanises to a tombstone.** A viewer
/// who lacks access to the repo sees "Checks failed on a restricted repo" — the check title NEVER
/// appears. The CI summary inherits the leak floor (it rides the ONE humanise pipeline).
#[test]
fn ci_summary_tombstones_for_a_denied_viewer_no_title_leak() {
    let mut templates = TemplateStore::with_platform_defaults();
    register_ci_summary_templates(&mut templates);
    let resolver = SyntheticResolver {
        allowed_viewer: "psn:author".into(),
    };

    let summary = ci_summary(CheckVerdict::Failure, "build");
    let out = humanise_ci_summary(
        &resolver,
        &templates,
        &summary.template_key,
        &viewer("psn:intruder"),
    );

    // the title NEVER leaks — the slot is the PII-free tombstone display.
    assert!(
        !out.text.contains(SECRET_CHECK_TITLE),
        "the check title must NEVER appear for a denied viewer (NOTIF-D4, 0 leak)"
    );
    assert!(
        !out.text.contains("secret-acquisition"),
        "no fragment of the title leaks"
    );
    // the body still renders, with the restricted-kind tombstone bound into the slot.
    assert!(out.text.starts_with("Checks failed on a restricted "));
    // a tombstone yields NO click-route link (a denied ref is not routable).
    assert!(out.links.is_empty(), "a tombstone never leaks a route");
}

/// **The CDC: every verdict's summary resolves through humanise (never the fallback).** All seven
/// `CheckState` verdicts produce a summary whose template_key renders a REGISTERED CI body for an
/// allowed viewer — the full producer set is accepted by the consumer (Notif) unchanged.
#[test]
fn every_verdict_summary_resolves_through_humanise() {
    let mut templates = TemplateStore::with_platform_defaults();
    register_ci_summary_templates(&mut templates);
    let resolver = SyntheticResolver {
        allowed_viewer: "psn:author".into(),
    };
    let verdicts = [
        (CheckVerdict::Queued, "Checks queued on"),
        (CheckVerdict::InProgress, "Checks running on"),
        (CheckVerdict::Success, "Checks passed on"),
        (CheckVerdict::Failure, "Checks failed on"),
        (CheckVerdict::Error, "Checks errored on"),
        (CheckVerdict::Neutral, "Checks neutral on"),
        (CheckVerdict::Cancelled, "Checks cancelled on"),
    ];
    for (verdict, prefix) in verdicts {
        let summary = ci_summary(verdict, "build");
        assert_eq!(summary.template_key, summary_template_key(verdict));
        let out = humanise_ci_summary(
            &resolver,
            &templates,
            &summary.template_key,
            &viewer("psn:author"),
        );
        // the REGISTERED body rendered (the generic fallback would be `<key>: <subject>`).
        assert!(
            out.text.starts_with(prefix),
            "verdict {verdict:?} must render its registered CI body, got `{}`",
            out.text
        );
        assert!(out.text.contains(SECRET_CHECK_TITLE));
    }
}

/// **The X-1 CDC: CI's producer `(template_key, args)` is byte-faithfully Git's consumer
/// `HumanisedRef`.** CI builds the summary; it serialises to the OPAQUE shape the Bus carries; the
/// Git consumer (`myelin_git::check_status::HumanisedRef`) decodes it. The producer half and the
/// consumer half agree on the ONE frozen 5.9 `HumanisedRef` shape — no drift across the X-1 seam.
#[test]
fn ci_summary_decodes_to_the_git_consumer_humanised_ref() {
    let summary = ci_summary(CheckVerdict::Success, "test/unit");

    // CI serialises the summary to the opaque shape the Bus carries on the CheckStatus fact...
    let opaque = serde_json::to_value(&summary).expect("CI's summary serialises");

    // ...and the Git CONSUMER view decodes EXACTLY that shape (no second struct — one frozen 5.9).
    let consumer: myelin_git::check_status::HumanisedRef =
        serde_json::from_value(opaque).expect("the Git consumer decodes CI's HumanisedRef");

    assert_eq!(consumer.template_key, "ci.check.success");
    let mut want = BTreeMap::new();
    want.insert("context".to_string(), "test/unit".to_string());
    assert_eq!(consumer.args, want);
}
