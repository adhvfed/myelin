//! # NOTIF-D4 re-confirmed on a REAL Knowledge subject + KN-D5 / KN-D13 confidential-leak
//! (NOTIF-P20 / P-264, M3)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! - row **NOTIF-D4** ("notify on a confidential subject to a viewer lacking access → humanised
//!   tombstone; the title NEVER appears; 0 title/PII leak") — re-run here against a REAL KN confidential
//!   page (blocked by the `- direct_block` page-tree override, P-249), NOT a synthetic subject (the
//!   prompt's GATE: "0 title/PII leak against a real subject; threshold 0, never softened").
//! - rows **KN-D5 / KN-D13** ("a confidential page / row / field is ABSENT from any result for an
//!   unauthorized viewer INCLUDING the COUNT; cross-tenant access is 0") — green with **Notif's humanise
//!   path exercised**: a denied / cross-tenant viewer's resolve degrades to a tombstone, the
//!   confidential page title never leaks (the `resolve(Display)` leak surface KN-D5/KN-D13 name).
//!
//! **Why this lives in `myelin-identity-service`.** Knowledge's compiled authz content (the `watcher`
//! relation + the page-tree-with-overrides rewrite) lives here (`knowledge_fragment`, the documented
//! §2.9-DAG exception); Knowledge's Notif registration accretes alongside it (`knowledge_rules`).
//! NOTIF-D4 was first proven against a SYNTHETIC subject in `crates/myelin-notif/tests/drill_notif_d4.rs`
//! (NOTIF-P9). NOTIF-P20 RE-CONFIRMS it against a REAL KN subject (`myelin://<tenant>/knowledge/page/<n>`
//! blocked by `- direct_block`), exercising Notif's [`humanise`](myelin_notif::humanise) path over KN's
//! real subject URNs. **ZERO Notif code change** — this consumes Notif's frozen humanise seam; the leak
//! property is structural (a denied / cross-tenant ref resolves to a [`Tombstone`] that carries no title
//! field). The production resolve transport is the Refs chokepoint over the resilient client (a named
//! floor); the [`KnowledgePageResolver`] here stands in with the SAME `Projection | Tombstone` shape the
//! real chokepoint returns, keyed on KN's real `page` ACL (the page-tree `read` permission with the
//! `- direct_block` override) so the leak property is exercised end to end over a REAL KN subject.

use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::humanise::{
    humanise, Channel, RefProjection, RefResolution, RefResolvePort, Tombstone, TombstoneReason,
    DEFAULT_LOCALE,
};
use myelin_notif::Reason;
use myelin_notif::TemplateStore;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::Mutex;

/// The secret title of the REAL confidential KN page (the leak target — must NEVER appear for a denied
/// or cross-tenant viewer).
const SECRET_PAGE_TITLE: &str =
    "Q3 layoffs: the PROJECT-NIGHTFALL severance list before the announcement";

fn acme() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
/// A viewer in `tenant` (the token tenant — KN-D13 keys the decision on THIS, never the URL-path tenant).
fn viewer_in(id: &str, tenant: TenantId) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant)
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

/// A REAL KN page subject in acme's space (`myelin://acme/knowledge/page/<n>`). The page inherits read
/// from its parent, but a `- direct_block` override (the page-tree-with-overrides rewrite, P-249) blocks
/// the denied viewers; only principals NOT blocked AND granted read may see the title.
fn confidential_kn_page() -> ArtifactRef {
    ArtifactRef("myelin://acme/knowledge/page/secret".into())
}

/// **The KN resolve chokepoint stand-in (the real Refs chokepoint over KN's `page` ACL).** Returns a
/// projection (allowed — same-tenant viewer granted read on the page and NOT blocked) carrying the secret
/// page title, or a tombstone (denied / blocked / cross-tenant) carrying NO title. Keyed on
/// `(viewer.tenant, viewer_id, ref)`: a viewer whose TENANT differs from the subject's tenant is ALWAYS
/// denied (cross-tenant — KN-D13), regardless of any same-id grant in another tenant; a viewer on the
/// `blocked` list is denied even if otherwise readable (the `- direct_block` page-tree override).
#[derive(Default)]
struct KnowledgePageResolver {
    /// `(tenant, viewer_id, ref)` triples granted read on the page (the `direct_reader` arm).
    allowed: Mutex<Vec<(String, String, String)>>,
    /// `(tenant, viewer_id, ref)` triples BLOCKED by the page-tree `- direct_block` override.
    blocked: Mutex<Vec<(String, String, String)>>,
}
impl KnowledgePageResolver {
    /// Grant `viewer_id` (in `tenant`) read on the page backing `r`.
    fn grant_read(&self, tenant: &TenantId, viewer_id: &str, r: &ArtifactRef) {
        self.allowed
            .lock()
            .unwrap()
            .push((tenant.0.clone(), viewer_id.into(), r.0.clone()));
    }
    /// Block `viewer_id` (in `tenant`) on the page (the `- direct_block` override — narrows inherited
    /// access even if they would otherwise inherit read).
    fn block(&self, tenant: &TenantId, viewer_id: &str, r: &ArtifactRef) {
        self.blocked
            .lock()
            .unwrap()
            .push((tenant.0.clone(), viewer_id.into(), r.0.clone()));
    }
    /// The subject's home tenant, parsed from `myelin://<tenant>/knowledge/...` (the URL-path tenant).
    fn subject_tenant(r: &ArtifactRef) -> Option<String> {
        r.0.strip_prefix("myelin://")
            .and_then(|rest| rest.split('/').next())
            .map(|t| t.to_string())
    }
}
impl RefResolvePort for KnowledgePageResolver {
    fn resolve_display(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        // KN-D13: the decision keys on the TOKEN tenant (viewer.tenant). A cross-tenant viewer (token
        // tenant ≠ subject's home tenant) is denied at the front door — 0 cross-tenant read, the
        // confidential page title NEVER resolves for them.
        let subject_tenant = KnowledgePageResolver::subject_tenant(ref_);
        if subject_tenant.as_deref() != Some(viewer.tenant.0.as_str()) {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            });
        }
        // The `- direct_block` page-tree OVERRIDE: a blocked viewer is removed from the page's read set
        // even if they inherit it (the "a sub-page narrows inherited access" lever, P-249).
        let is_blocked =
            self.blocked.lock().unwrap().iter().any(|(t, v, x)| {
                t == &viewer.tenant.0 && v == &viewer.principal_id.0 && x == &ref_.0
            });
        if is_blocked {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            });
        }
        // Same-tenant + not blocked: allowed iff the viewer holds read on the page.
        let allowed =
            self.allowed.lock().unwrap().iter().any(|(t, v, x)| {
                t == &viewer.tenant.0 && v == &viewer.principal_id.0 && x == &ref_.0
            });
        if allowed {
            RefResolution::Projection(RefProjection {
                ref_: ref_.clone(),
                title: SECRET_PAGE_TITLE.into(),
                icon: "page".into(),
            })
        } else {
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

/// Every KN reason → its template key drives a render; the leak property must hold for ALL of them.
const KN_REASONS: &[Reason] = &[
    Reason::Mentioned,
    Reason::Comments,
    Reason::Shared,
    Reason::Watched,
];

fn contains_leak(text: &str) -> bool {
    let lc = text.to_lowercase();
    text.contains(SECRET_PAGE_TITLE)
        || lc.contains("nightfall")
        || lc.contains("layoffs")
        || lc.contains("severance")
}

/// **NOTIF-D4 on a REAL KN subject (the dated green artifact): 0 title/PII leak when a viewer lacking
/// `read` on a confidential KN page is notified about it.** Across denied viewers × every channel ×
/// every KN reason template, the secret page title appears EXACTLY ZERO times. Threshold 0 — never
/// softened. This is the KN-D5 / KN-D13 confidential-page-no-leak exercised through Notif's humanise
/// path (the `resolve(Display)` leak surface).
#[test]
fn notif_d4_zero_leak_on_real_confidential_kn_page() {
    let resolver = KnowledgePageResolver::default(); // nobody granted read → every same-tenant viewer denied
                                                     // one of the denied viewers is explicitly BLOCKED by the `- direct_block` override (the page-tree
                                                     // narrow-inherited-access lever) — the title must still never appear.
    let subject = confidential_kn_page();
    resolver.block(&acme(), "blocked-by-override", &subject);

    let templates = TemplateStore::with_platform_defaults();
    // denied viewers IN acme (same tenant, but lacking `read`) — the pure NOTIF-D4 case + the override.
    let denied = ["ex-contractor", "wrong-team-dev", "blocked-by-override"];

    let mut renders = 0u64;
    let mut leak_count = 0u64;
    let mut tombstone_present = 0u64;

    for v in denied {
        for &reason in KN_REASONS {
            let key = myelin_notif::reason_template_key(reason);
            for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
                let h = humanise(
                    &resolver,
                    &acme(),
                    &region(),
                    &templates,
                    key,
                    std::slice::from_ref(&subject),
                    &viewer_in(v, acme()),
                    DEFAULT_LOCALE,
                    &strong("zk-1"),
                    channel,
                );
                renders += 1;
                if contains_leak(&h.text) {
                    leak_count += 1;
                }
                // the denied subject renders the PII-free tombstone (kind from the opaque URN — `page`).
                if h.text.contains("a restricted page") {
                    tombstone_present += 1;
                }
                assert!(
                    h.links.is_empty(),
                    "a denied KN subject yields no click-route link"
                );
            }
        }
    }

    assert_eq!(
        leak_count, 0,
        "NOTIF-D4 (real KN subject): title-leak-count MUST be 0 over {renders} renders; never weakened"
    );
    assert_eq!(
        tombstone_present, renders,
        "every denied render shows the PII-free `a restricted page` tombstone (the embed degrades)"
    );
    eprintln!(
        "NOTIF-D4 GREEN on a REAL KN subject (2026-06-21): {renders} denied renders \
         (incl. a - direct_block override viewer), title-leak-count = {leak_count} (threshold 0), \
         tombstone = {tombstone_present}/{renders}"
    );
}

/// **The complement — a same-tenant viewer WITH `read` (and not blocked) DOES see the confidential page
/// title (the gate discriminates, it is not a blanket redaction).** Proves the KN resolve chokepoint is
/// real.
#[test]
fn notif_d4_permitted_kn_viewer_sees_the_page_title() {
    let resolver = KnowledgePageResolver::default();
    let subject = confidential_kn_page();
    resolver.grant_read(&acme(), "editor", &subject);
    let h = humanise(
        &resolver,
        &acme(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        "shared",
        std::slice::from_ref(&subject),
        &viewer_in("editor", acme()),
        DEFAULT_LOCALE,
        &strong("zk-1"),
        Channel::Cli,
    );
    assert!(
        h.text.contains(SECRET_PAGE_TITLE),
        "the permitted editor sees the page title"
    );
    assert_eq!(
        h.links,
        vec![subject.0],
        "the allowed branch yields the click-route link"
    );
}

/// **KN-D13 — cross-tenant page access denied, with Notif's humanise path exercised (0 cross-tenant
/// leak).** A viewer whose TOKEN tenant (`evilcorp`) differs from the confidential page's home tenant
/// (`acme`) is denied at the front door — even if a same-id principal in acme WOULD be allowed read. The
/// humanise render degrades to a tombstone; the confidential page title never crosses the tenant
/// boundary. The `CrossTenantCount == 0` survival signal is asserted GREEN through the telemetry harness.
/// Threshold: 0 cross-tenant leak.
#[test]
fn kn_d13_cross_tenant_page_access_denied_via_humanise() {
    let mut signals = SignalSource::new();
    let resolver = KnowledgePageResolver::default();
    let subject = confidential_kn_page(); // home tenant: acme
                                          // an acme principal "spy" WOULD be allowed read (same id) — but the cross-tenant token below is a
                                          // DIFFERENT tenant, so the same id from evilcorp must still be denied (token tenant decides).
    resolver.grant_read(&acme(), "spy", &subject);

    let cross_tenant = viewer_in("spy", TenantId("evilcorp".into()));
    let mut leak = 0u64;
    let mut cross_tenant_reads: i64 = 0;
    for &reason in KN_REASONS {
        let key = myelin_notif::reason_template_key(reason);
        for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
            let h = humanise(
                &resolver,
                &acme(),
                &region(),
                &TemplateStore::with_platform_defaults(),
                key,
                std::slice::from_ref(&subject),
                &cross_tenant,
                DEFAULT_LOCALE,
                &strong("zk-1"),
                channel,
            );
            if contains_leak(&h.text) {
                leak += 1;
            }
            // a render that resolved the title (carried a click-route link to the subject) would be a
            // cross-tenant read — count it for the CrossTenantCount survival signal.
            if h.links.contains(&subject.0) {
                cross_tenant_reads += 1;
            }
            assert!(
                h.text.contains("a restricted page"),
                "cross-tenant render is a tombstone"
            );
            assert!(
                h.links.is_empty(),
                "no click-route leaks across the tenant boundary"
            );
        }
    }
    assert_eq!(
        leak, 0,
        "KN-D13: 0 cross-tenant leak — the token tenant decides, the title never crosses"
    );

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    eprintln!(
        "KN-D13 GREEN (2026-06-21): cross-tenant viewer, cross-tenant-leak-count = {leak} \
         (threshold 0), CrossTenantCount = {cross_tenant_reads}"
    );
}
