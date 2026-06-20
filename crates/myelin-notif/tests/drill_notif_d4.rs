//! # NOTIF-D4 — the per-viewer humanise leak drill (0 title/PII leak) (P-187)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **NOTIF-D4** ("notify on a confidential subject to a viewer lacking access → humanised
//! tombstone; the title NEVER appears; the item is suppressed if the recipient can't see the subject;
//! **0 title/PII leak**"), VISION.md §3 (GDPR-safe by construction — per-viewer permission-safe),
//! external-insights/01 §3 (prove-it — the leak drill FORCES the failure: a real confidential
//! subject, a real denied viewer, the title asserted absent), §1 (name-your-floors).
//!
//! **The dated GREEN artifact (2026-06-20).** A confidential subject (a private issue) whose TITLE is
//! the secret carries through the ONE inbox; humanise renders it PER-VIEWER. The drill measures +
//! asserts, with NO threshold weakened:
//!
//! 1. **0 title/PII leak (the F1 floor)** — across a corpus of denied viewers × every channel
//!    projection (CLI plain / email HTML / raw markdown) × every reason template, the secret title
//!    appears in the rendered output EXACTLY ZERO times. `title-leak-count == 0`. The threshold is 0 —
//!    never inverted, never softened.
//! 2. **the denied slot is the PII-free tombstone display** — a denied subject renders as `a
//!    restricted <kind>` (kind from the OPAQUE URN, never content); an erased actor as `[erased
//!    user]`. The placeholder is present (the embed degrades, it does not vanish).
//! 3. **the permitted viewer DOES see the title** — the complement (the gate is real, not a blanket
//!    redaction): an allowed viewer of the SAME subject renders the title (so the drill proves the
//!    chokepoint discriminates, it does not just blank everything).
//! 4. **item-suppression in the inbox read** — an item whose subject the recipient cannot `check`-see
//!    is held, not leaked: the ranked `list_inbox` drops it (the router/read-path defence) BEFORE
//!    humanise is even reached (defence in depth — two independent lines, both 0-leak).
//!
//! The drill resolves refs through a synthetic Refs chokepoint (REF-P10 stands in — the production
//! wire is the named `ResolveService`-over-resilient-client floor); the synthetic returns the SAME
//! `Projection | Tombstone` shape the real chokepoint returns, so the leak property is exercised end
//! to end (the title only ever exists on the allowed branch; a tombstone has no field to leak into).

use myelin_identity::{
    Consistency, ConsistencyMode, Decision, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::humanise::{
    humanise, Channel, RefProjection, RefResolution, RefResolvePort, Tombstone, TombstoneReason,
    DEFAULT_LOCALE,
};
use myelin_notif::list_inbox::{list_inbox_ranked, InboxFilter, Page, ReadAuthorizePort};
use myelin_notif::ranking::DeterministicV1;
use myelin_notif::router::{InboxProjection, RoutedInboxItem};
use myelin_notif::{Class, Reason, TemplateStore};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::Mutex;

const SECRET_TITLE: &str = "PROJECT NIGHTFALL — confidential acquisition terms";

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong(zk: &str) -> Consistency {
    Consistency { at_least: Zookie(zk.into()), mode: ConsistencyMode::Strong }
}
fn confidential_issue() -> ArtifactRef {
    ArtifactRef("myelin://acme/issue/issue/ENG-secret".into())
}

/// The synthetic Refs resolve chokepoint (REF-P10 stand-in). Returns a projection (allowed) carrying
/// the secret title, or a tombstone (denied/erased) carrying NO title — the SAME shape as the real
/// chokepoint, so the leak property is real end to end.
#[derive(Default)]
struct DrillResolver {
    allowed: Mutex<Vec<(String, String)>>,
    erased: Mutex<Vec<String>>,
}
impl DrillResolver {
    fn allow(&self, viewer_id: &str, r: &ArtifactRef) {
        self.allowed.lock().unwrap().push((viewer_id.into(), r.0.clone()));
    }
    fn erase(&self, r: &ArtifactRef) {
        self.erased.lock().unwrap().push(r.0.clone());
    }
}
impl RefResolvePort for DrillResolver {
    fn resolve_display(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if self.erased.lock().unwrap().iter().any(|x| x == &ref_.0) {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Erased,
            });
        }
        if self
            .allowed
            .lock()
            .unwrap()
            .iter()
            .any(|(v, x)| v == &viewer.principal_id.0 && x == &ref_.0)
        {
            RefResolution::Projection(RefProjection {
                ref_: ref_.clone(),
                title: SECRET_TITLE.into(),
                icon: "lock".into(),
            })
        } else {
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

/// Every reason → its template key drives a render; the leak property must hold for ALL of them.
const ALL_REASONS: &[Reason] = &[
    Reason::ApprovalRequested,
    Reason::Escalated,
    Reason::Sla,
    Reason::ReviewRequested,
    Reason::Assigned,
    Reason::Mentioned,
    Reason::Replied,
    Reason::AgentProposal,
    Reason::Watched,
    Reason::StateChanged,
    Reason::Fyi,
    Reason::Blocked,
    Reason::Unblocked,
    Reason::ThreadWatched,
    Reason::Shared,
    Reason::Comments,
];

fn contains_leak(text: &str) -> bool {
    let lc = text.to_lowercase();
    text.contains(SECRET_TITLE)
        || lc.contains("nightfall")
        || lc.contains("acquisition")
        || lc.contains("confidential")
}

/// **NOTIF-D4 (the dated green artifact, 2026-06-20): 0 title/PII leak across denied viewers × every
/// channel × every reason template.** The threshold is 0 — measured, never weakened.
#[test]
fn notif_d4_zero_title_leak_across_viewers_channels_reasons() {
    let resolver = DrillResolver::default(); // nobody allowed → every viewer is denied
    let templates = TemplateStore::with_platform_defaults();
    let subject = confidential_issue();
    let denied_viewers = ["intruder-a", "intruder-b", "ex-employee"];

    let mut renders = 0u64;
    let mut leak_count = 0u64;
    let mut tombstone_present = 0u64;

    for v in denied_viewers {
        for &reason in ALL_REASONS {
            let key = myelin_notif::reason_template_key(reason);
            for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
                let h = humanise(
                    &resolver,
                    &tenant(),
                    &region(),
                    &templates,
                    key,
                    std::slice::from_ref(&subject),
                    &viewer(v),
                    DEFAULT_LOCALE,
                    &strong("z1"),
                    channel,
                );
                renders += 1;
                if contains_leak(&h.text) {
                    leak_count += 1;
                }
                if h.text.contains("a restricted issue") {
                    tombstone_present += 1;
                }
                // a denied ref is never routable — no link to leak a route.
                assert!(h.links.is_empty(), "a denied subject yields no link (reason={key}, channel={channel:?})");
            }
        }
    }

    // ── The measured artifact ──
    assert_eq!(
        leak_count, 0,
        "NOTIF-D4: title-leak-count MUST be 0 over {renders} renders (the F1 floor); never weakened"
    );
    assert_eq!(
        tombstone_present, renders,
        "every denied render shows the PII-free tombstone display (the embed degrades, never vanishes)"
    );
    eprintln!(
        "NOTIF-D4 GREEN (2026-06-20): {renders} denied renders, title-leak-count = {leak_count} (threshold 0), \
         tombstone-present = {tombstone_present}/{renders}"
    );
}

/// **The complement — a PERMITTED viewer DOES see the title (the gate discriminates, it is not a
/// blanket redaction).** Proves the chokepoint is real: the title flows on the allowed branch.
#[test]
fn notif_d4_permitted_viewer_sees_the_title() {
    let resolver = DrillResolver::default();
    let subject = confidential_issue();
    resolver.allow("insider", &subject);
    let h = humanise(
        &resolver,
        &tenant(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        "review_requested",
        std::slice::from_ref(&subject),
        &viewer("insider"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Cli,
    );
    assert!(h.text.contains(SECRET_TITLE), "the permitted viewer sees the title (the gate is real)");
    assert_eq!(h.links, vec![subject.0], "the allowed branch yields the click-route link");
}

/// **An erased actor → `[erased user]` (0 PII leak; the erasure-safe property — references not
/// payloads).** Even for an otherwise-permitted viewer, an erased ref is unrenderable.
#[test]
fn notif_d4_erased_actor_is_erased_user_zero_pii() {
    let resolver = DrillResolver::default();
    let actor = ArtifactRef("myelin://acme/identity/user/u-77".into());
    resolver.allow("colleague", &actor);
    resolver.erase(&actor);
    let h = humanise(
        &resolver,
        &tenant(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        "mentioned",
        &[actor],
        &viewer("colleague"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Email,
    );
    assert!(h.text.contains("[erased user]"), "an erased actor renders [erased user], got `{}`", h.text);
    assert!(h.links.is_empty(), "an erased ref is not routable");
}

/// **Defence in depth: the inbox READ suppresses an item whose subject the recipient cannot see —
/// BEFORE humanise is reached.** The router/read-path is the FIRST line; humanise is the second. A
/// denying `check` drops the row from the ranked read, so a routed-by-mistake confidential item is
/// never even handed to humanise.
#[test]
fn notif_d4_inbox_read_suppresses_unseeable_item() {
    // A read-authorize port that DENIES the confidential subject (the recipient lost access).
    struct DenyConfidential;
    impl ReadAuthorizePort for DenyConfidential {
        fn can_read(&self, _v: &Principal, subject: &ArtifactRef, _at: &Consistency) -> Decision {
            if subject == &confidential_issue() {
                Decision::Deny
            } else {
                Decision::Allow
            }
        }
    }

    let inbox = InboxProjection::new();
    let visible = ArtifactRef("myelin://acme/issue/issue/ENG-public".into());
    // two rows for the recipient: one confidential (must be dropped), one visible (must remain).
    inbox.upsert_for_test(row("c", confidential_issue(), Reason::ReviewRequested));
    inbox.upsert_for_test(row("p", visible.clone(), Reason::Assigned));

    let page = list_inbox_ranked(
        &inbox,
        &viewer("recipient"),
        &InboxFilter::all(),
        &Page::default(),
        &DenyConfidential,
        &strong("z1"),
        &DeterministicV1::default(),
    );

    let subjects: Vec<&ArtifactRef> = page.items.iter().map(|r| &r.item.subject).collect();
    assert!(
        !subjects.contains(&&confidential_issue()),
        "the unseeable confidential item is SUPPRESSED from the read (held, not leaked)"
    );
    assert!(subjects.contains(&&visible), "the visible item remains");
}

fn row(id: &str, subject: ArtifactRef, reason: Reason) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: region(),
        item_id: id.into(),
        recipient: "recipient".into(),
        subject,
        reason,
        class: Class::Direct,
        origin_event: ArtifactRef("myelin://acme/issue/event/e".into()),
        dedup_key: id.into(),
        coalesce_count: 0,
        state: "unread".into(),
        snooze_until: None,
    }
}
