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

fn check_subject() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/repo/core#commit-abc123/check-build".into())
}

const SECRET_CHECK_TITLE: &str = "build #42 on the secret-acquisition branch";

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
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

fn humanise_ci_summary(
    resolver: &dyn RefResolvePort,
    templates: &TemplateStore,
    summary_key: &str,
    viewer: &Principal,
) -> myelin_notif::HumanisedString {
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
    assert_eq!(out.text, format!("Checks failed on {SECRET_CHECK_TITLE}"));
    assert_eq!(out.links, vec![check_subject().0]);
    assert_eq!(out.icon, "ci");
}

#[test]
fn ci_summary_is_never_a_raw_string() {
    let summary = ci_summary(CheckVerdict::Failure, "build");
    let v = serde_json::to_value(&summary).unwrap();
    assert!(v.get("template_key").is_some());
    assert!(v.get("args").is_some());
    assert!(v.get("text").is_none());
    assert!(v.get("summary").is_none());
    assert!(v.is_object());
    assert_eq!(v.as_object().unwrap().len(), 2, "only template_key + args");
}

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

    assert!(
        !out.text.contains(SECRET_CHECK_TITLE),
        "the check title must NEVER appear for a denied viewer (NOTIF-D4, 0 leak)"
    );
    assert!(
        !out.text.contains("secret-acquisition"),
        "no fragment of the title leaks"
    );
    assert!(out.text.starts_with("Checks failed on a restricted "));
    assert!(out.links.is_empty(), "a tombstone never leaks a route");
}

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
        assert!(
            out.text.starts_with(prefix),
            "verdict {verdict:?} must render its registered CI body, got `{}`",
            out.text
        );
        assert!(out.text.contains(SECRET_CHECK_TITLE));
    }
}

#[test]
fn ci_summary_decodes_to_the_git_consumer_humanised_ref() {
    let summary = ci_summary(CheckVerdict::Success, "test/unit");

    let opaque = serde_json::to_value(&summary).expect("CI's summary serialises");

    let consumer: myelin_git::check_status::HumanisedRef =
        serde_json::from_value(opaque).expect("the Git consumer decodes CI's HumanisedRef");

    assert_eq!(consumer.template_key, "ci.check.success");
    let mut want = BTreeMap::new();
    want.insert("context".to_string(), "test/unit".to_string());
    assert_eq!(consumer.args, want);
}
