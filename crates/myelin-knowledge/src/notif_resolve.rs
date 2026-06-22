//! # `notif_resolve` — Knowledge `resolve(ref, viewer, Display)` for the notif/humanise glue, wired
//! through Notif's ONE templating surface (KN-P22 / P-312, M3)
//!
//! **The notif/humanise half of KN-M3d (architecture
//! `04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md`
//! §1.5 (comments/mentions events → Notif), §2.2 — the **project Display mode** = the humanisation
//! projection Notif uses: a routable `ArtifactRef` + a humanised string, the **sole `humanise`
//! templating surface**, so "alice mentioned you in <Incident runbook>" renders per-viewer for
//! every consumer; Knowledge registers **NO second template engine**, OQ-L).**
//!
//! ## What lands here vs. what already shipped (the reconciliation, EI-01 §7)
//! The **producer-accretion half** of the Knowledge↔Notif glue ALREADY shipped at NOTIF-P20 (P-264)
//! in `myelin_identity_service::knowledge_rules`: the [`define_notif_rule`](myelin_notif::define_notif_rule)
//! set (contract 7.6 — `mentioned` / `comments` / `shared` / `watched`, each reconciled against the
//! ONE §3.1 ranking table) + the `KnowledgeWatcherIndex` read-fanout reverse index behind the frozen
//! [`WatcherResolvePort`](myelin_notif::WatcherResolvePort) over the `watcher` relation Id's compiled
//! Knowledge ReBAC fragment declares (4.9, on `space` / `page` / `database_row`). That half lives in
//! `myelin-identity-service` (alongside `knowledge_fragment`) for the §2.9-DAG reason the fragment
//! does: `myelin-content` is the Knowledge data-model LEAF and `myelin-notif` already depends ON
//! content, so a `content → notif` edge would be a cycle.
//!
//! What did NOT exist, and is the genuinely-new KN-P22 deliverable, is the **resolve (5.2) →
//! Display projection bridge** the humanise gate runs over: a REAL Knowledge `RefResolvePort` that
//! drives Notif's [`humanise`](myelin_notif::humanise) (contract 7.3) by delegating to Knowledge's
//! REAL [`Projector::project`](crate::refs_glue::Projector) (contract 5.6, Display mode) — a
//! confidential page/block/row subject resolves to a **humanised tombstone, the TITLE NEVER LEAKS**.
//! This is the exact mirror of `myelin_git::git_resolve::GitRefResolver` (GIT-P31 / P-292), and it
//! lives HERE — in `myelin-knowledge`, the producer crate that OWNS the projector — because the
//! resolve seam is over Knowledge's REAL per-viewer projection logic, not a test-local stand-in. The
//! `content → notif` cycle does NOT apply: this is the SERVICE crate `myelin-knowledge` depending on
//! the `myelin-notif` LEAF (the SAME sanctioned acyclic edge `myelin-git` already carries), not the
//! `myelin-content` leaf.
//!
//! ## The inverse-signal property (EI-01 §1) — ZERO Notif code change
//! Knowledge supplies the resolve transport humanise binds its slots to using ONLY the **public,
//! frozen** Notif seam — the [`RefResolvePort`] trait (the read half of contract 5.2/7.3). No Notif
//! enum variant, no Notif match arm, no Notif recompile: humanise already calls `resolve_display` on
//! a `&dyn RefResolvePort`; Knowledge hands it ONE more impl. The leak invariant is **structural** —
//! a denied ref maps to a [`RefResolution::Tombstone`], a type with **no `title` field** for a title
//! to leak into (NOTIF-D4 — threshold 0, never softened).
//!
//! ## The resolve → humanise → tombstone chain (contract 5.2 → 7.3, the leak gate)
//! 1. Notif's [`humanise`](myelin_notif::humanise) resolves EACH `ArtifactRef` slot per-viewer via
//!    `RefResolvePort::resolve_display` BEFORE formatting (so a denied slot never carries a title
//!    into the formatter).
//! 2. [`KnowledgeRefResolver::resolve_display`] calls Knowledge's REAL
//!    [`Projector::project`](crate::refs_glue::Projector) — **permission FIRST**
//!    (`Id.check(viewer, read, page-tree root)`); a `Deny` / Id-hiccup / erased / restricted artifact
//!    returns a [`Projected::Tombstoned`](crate::refs_glue::Projected), built with NO field of the
//!    artifact read into it.
//! 3. This module maps that [`Projected`](crate::refs_glue::Projected) into Notif's
//!    [`RefResolution`]: `Visible(p)` → `Projection{title, icon}` (the ALLOWED branch — the title + a
//!    click-route link); `Tombstoned(t)` → `Tombstone{root, reason}` (the leak-free branch — KN's
//!    [`TombstoneReason`](crate::refs_glue::TombstoneReason) maps to Notif's PII-free
//!    [`TombstoneReason`](myelin_notif::TombstoneReason) which renders `a restricted page`, NEVER the
//!    title).
//!
//! The OPAQUE root URN crosses into the tombstone (so Notif renders `a restricted <kind>` from the
//! URN structure) — the title/state/render-hint never do (they live only on the `Visible` branch).
//!
//! ## NOTIF-D4-class GATE (the dated green artifact; threshold 0, never softened)
//! A confidential KN page/block/row subject → for a viewer LACKING `read` the humanise render is a
//! TOMBSTONE; the title appears EXACTLY ZERO times across every channel × every KN reason template.
//! Proven in `tests/drill_notif_d4_kn_humanise_resolve.rs` over THIS real resolver, AND
//! end-to-end against a REAL KN page-tree `- direct_block` override in
//! `crates/myelin-identity-service/tests/drill_notif_d4_kn_d5_d13_real_kn_subject.rs` (NOTIF-P20).
//!
//! ## <a name="named-floors"></a>Named floors (VISION §3)
//! - **The KB-native comment-event sibling is KN-P23 (P-313).** The `knowledge.comment.created` /
//!   `.resolved` events the `comments` notif rule fires on are PRODUCED by KN-P23 (the comment
//!   threads over the shared `#sub` grammar). This module wires the resolve→humanise path the
//!   comment Signal's `(template_key, args)` renders through; the comment-event EMITTER is KN-P23.
//! - **The production resolve transport** is the Refs resolve chokepoint over the substrate resilient
//!   client; [`KnowledgeRefResolver`] is the IN-PROCESS resolve seam (cell-local — contract 5.2 /
//!   OQ-I: KN resolution is always cell-local) over Knowledge's real Projector. The cross-cell
//!   single-home resolve is the named multi-cell floor (NOTIF-P24): a viewer in cell A unfurling a
//!   page homed in cell B has cell B run the projection; only the rendered projection crosses.
//! - **The live OLTP page/block/row store** the [`Projector`](crate::refs_glue::Projector) reads is
//!   the KN-P05 store-wiring floor (the SAME entity shapes the live store hydrates — the resolver is
//!   store-agnostic).

use myelin_identity::{Consistency, IdentityService, Principal, Zookie};
use myelin_notif::humanise::{RefProjection, RefResolution, RefResolvePort, Tombstone, TombstoneReason};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::refs_glue::{Projected, Projector, TombstoneReason as KnTombstoneReason};

/// **The REAL Knowledge resolve seam for the notif/humanise glue — `resolve(ref, viewer, Display)`
/// over Knowledge's `Projector` (contract 5.2; the transport Notif's humanise binds its slots to).**
/// Wraps Knowledge's REAL [`Projector`](crate::refs_glue::Projector) (contract 5.6 — permission-FIRST,
/// per-viewer, the 4-step tombstone ladder §2.1) and adapts its
/// [`Projected`](crate::refs_glue::Projected) into Notif's [`RefResolution`]. Notif holds this behind
/// a `&dyn RefResolvePort` (the same seam discipline as the rule registry — Notif holds the PORT, not
/// the full Knowledge projector).
///
/// **The project Display mode IS the humanisation projection (architecture §2.2 / OQ-L).** Knowledge
/// registers NO second template engine — it feeds this per-viewer `{title, icon}` Display projection
/// into the ONE humanise ICU surface, and the resolved slot is the title (allowed) or the PII-free
/// tombstone display (denied/erased/restricted), so the same `mention`/`comment`/`share`/`watched`
/// reason template renders per-viewer.
///
/// **Cell-local (contract 5.2 / OQ-I).** A KN unfurl resolves the artifact in ITS home cell; the
/// cross-cell single-home resolve is the named multi-cell floor (NOTIF-P24).
pub struct KnowledgeRefResolver<I: IdentityService> {
    /// Knowledge's REAL per-viewer projector — the permission-first 5.6 Display projection the
    /// resolve adapts.
    projector: Projector<I>,
}

impl<I: IdentityService> KnowledgeRefResolver<I> {
    /// Compose the resolve seam over Knowledge's REAL [`Projector`](crate::refs_glue::Projector).
    pub fn new(projector: Projector<I>) -> KnowledgeRefResolver<I> {
        KnowledgeRefResolver { projector }
    }

    /// A borrow of the underlying projector (for the front door / drills to seed its store or inspect).
    pub fn projector_mut(&mut self) -> &mut Projector<I> {
        &mut self.projector
    }

    /// Map Knowledge's REAL [`Projected`](crate::refs_glue::Projected) into Notif's [`RefResolution`]
    /// (the leak invariant lives in the SHAPE — a tombstone has no title field). The ALLOWED branch
    /// carries the title + icon + the click-route ref; the denied/erased/restricted/gone branch
    /// carries ONLY the opaque root + a PII-free reason (NEVER the title — it was never read on the
    /// deny path).
    fn to_resolution(reference: &ArtifactRef, projected: Projected) -> RefResolution {
        match projected {
            Projected::Visible(p) => RefResolution::Projection(RefProjection {
                ref_: reference.clone(),
                title: p.title,
                icon: p.icon,
            }),
            Projected::Tombstoned(t) => RefResolution::Tombstone(Tombstone {
                // The OPAQUE root crosses (so Notif renders `a restricted <kind>` from the URN); the
                // title/state never do (they live only on the Visible branch above).
                root: t.root,
                reason: map_tombstone_reason(t.reason),
            }),
        }
    }
}

/// Map Knowledge's [`TombstoneReason`](crate::refs_glue::TombstoneReason) onto Notif's PII-free
/// [`TombstoneReason`](myelin_notif::TombstoneReason). Both are STRUCTURED enums (never free text);
/// each renders a fixed, content-free display (`a restricted page` / `[erased user]`). The mapping is
/// total — a future KN reason MUST be mapped here (the compiler enforces it), never silently widened.
fn map_tombstone_reason(reason: KnTombstoneReason) -> TombstoneReason {
    match reason {
        // Step 1 — `check(viewer, read, root)` denied (incl. the page-tree `- direct_block` override).
        // The leak-free `a restricted <kind>` (the NOTIF-D4 chokepoint).
        KnTombstoneReason::Denied => TombstoneReason::Denied,
        // Step 2 — the root page/database is gone (a dangling reference).
        KnTombstoneReason::RootGone => TombstoneReason::RootGone,
        // Step 3 — the root resolves but the sub-anchor (block/row/comment) is dead; the embed shows
        // the parent page.
        KnTombstoneReason::SubGone => TombstoneReason::SubGone,
        // Step 4 — pseudonym-/crypto-shred (KN-D4) OR the GDPR `restrict` suppression made the content
        // unrenderable → the canonical `[erased user]` / erased display (EI-04 §1). (KN folds the
        // restriction window into the Erased tombstone reason; both degrade to a content-free slot.)
        KnTombstoneReason::Erased => TombstoneReason::Erased,
    }
}

impl<I: IdentityService + Send + Sync> RefResolvePort for KnowledgeRefResolver<I> {
    /// **`resolve(ref, viewer, Display)` (contract 5.2) — over Knowledge's REAL permission-first
    /// projector.** Per-viewer, permission-checked: a denied/erased/restricted ref returns a
    /// [`RefResolution::Tombstone`] (NEVER a title); an allowed ref returns a
    /// [`RefResolution::Projection`]. The `tenant`/`region` are carried for the cell-local resolve
    /// (contract 5.2 / OQ-I); the per-viewer decision keys on `viewer` (the TOKEN tenant). A
    /// `ProjectError` (a malformed / non-KN ref) maps to a non-leaking `RootGone` tombstone — the
    /// safe degrade (an un-projectable ref unfurls as a kind-shaped placeholder, never a panic, never
    /// a leak).
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        at: &Consistency,
    ) -> RefResolution {
        // The projector takes a read-consistency fence; humanise passes a strong zookie for a
        // security-sensitive render. A `ProjectError` degrades to a non-leaking `RootGone` tombstone.
        let zookie: Zookie = at.at_least.clone();
        match self.projector.project(ref_, viewer, zookie) {
            Ok(projected) => Self::to_resolution(ref_, projected),
            // A malformed / non-KN ref → a safe, non-leaking placeholder (never a panic, never a leak;
            // the unfurl shows `a restricted <kind>` from the URN).
            Err(_) => RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::RootGone,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refs_glue::{PageMeta, PageStore, Projector};
    use myelin_identity::{
        AuthzError, CaveatContext, ConsistencyMode, Credential, Decision, ListObjectsResult,
        ObjectId, ObjectType, Permission, PrincipalId, PrincipalKind, Result as IdResult,
        RewriteTrace, SubjectTree, TupleDelta,
    };
    use myelin_notif::humanise::{humanise, Channel, TemplateStore, DEFAULT_LOCALE};
    use myelin_notif::reason_template_key;
    use myelin_notif::Reason;
    use std::collections::HashSet;

    // ── a deterministic Id stub: a `read@object` allow-list (absent ⇒ Deny, fail-closed). ──
    struct StubId {
        allow: HashSet<String>,
    }
    impl StubId {
        fn new() -> Self {
            Self { allow: HashSet::new() }
        }
        fn allow_read(mut self, object: &ArtifactRef) -> Self {
            self.allow.insert(format!("read@{}", object.0));
            self
        }
    }
    impl IdentityService for StubId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            _caveat: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            let key = format!("{}@{}", permission.0, object.0);
            Ok(if self.allow.contains(&key) { Decision::Allow } else { Decision::Deny })
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _at: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _at: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(
            &self,
            _a: &Principal,
            _t: &Principal,
        ) -> IdResult<myelin_identity::EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(
            &self,
            _d: &[TupleDelta],
            _p: Option<&myelin_identity::Precondition>,
        ) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &myelin_identity::RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &myelin_identity::FailStaticBound,
        ) -> IdResult<myelin_identity::RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(
            &self,
            _f: &myelin_identity::NamespaceFragment,
        ) -> IdResult<myelin_identity::FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    const SECRET_TITLE: &str = "Incident runbook: rotate the PROJECT-NIGHTFALL key before acquisition";

    fn acme() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, acme())
    }
    fn strong(zk: &str) -> Consistency {
        Consistency { at_least: Zookie(zk.into()), mode: ConsistencyMode::Strong }
    }
    fn secret_page() -> ArtifactRef {
        ArtifactRef("myelin://acme/knowledge/page/7c2".into())
    }

    /// A resolver over a confidential page; `grant_read` grants `read` (so the projector allows it).
    fn confidential_page_resolver(grant_read: bool) -> (KnowledgeRefResolver<StubId>, ArtifactRef) {
        let page = secret_page();
        let mut store = PageStore::new();
        store.put_root(&page, PageMeta { title: SECRET_TITLE.into(), state: "published".into() });
        let id = if grant_read { StubId::new().allow_read(&page) } else { StubId::new() };
        (KnowledgeRefResolver::new(Projector::new(id, store)), page)
    }

    /// **A denied viewer's resolve is a TOMBSTONE carrying NO title (the structural leak invariant).**
    /// The resolver delegates to Knowledge's permission-first projector; a viewer lacking `read` gets
    /// a `RefResolution::Tombstone` — a type with no `title` field for the secret to leak into.
    #[test]
    fn denied_viewer_resolves_to_a_tombstone_no_title() {
        let (resolver, page) = confidential_page_resolver(false); // nobody granted
        let r = resolver.resolve_display(&acme(), &region(), &page, &viewer("ex-contractor"), &strong("zk-1"));
        match r {
            RefResolution::Tombstone(t) => {
                assert_eq!(t.root, page, "the opaque root crosses (for `a restricted page`)");
                assert_eq!(t.reason, TombstoneReason::Denied);
            }
            RefResolution::Projection(_) => panic!("a denied viewer must NOT get a projection (leak!)"),
        }
    }

    /// **A permitted viewer's resolve IS a projection carrying the title + a click-route (the gate
    /// discriminates — it is not a blanket redaction).** Proves the resolver is REAL, over Knowledge's
    /// projector.
    #[test]
    fn permitted_viewer_resolves_to_a_projection_with_the_title() {
        let (resolver, page) = confidential_page_resolver(true);
        let r = resolver.resolve_display(&acme(), &region(), &page, &viewer("maintainer"), &strong("zk-1"));
        match r {
            RefResolution::Projection(p) => {
                assert_eq!(p.ref_, page);
                assert_eq!(p.title, SECRET_TITLE, "the permitted viewer sees the page title");
                assert_eq!(p.icon, "page");
            }
            RefResolution::Tombstone(_) => panic!("the permitted viewer must see the projection"),
        }
    }

    /// **NOTIF-D4-class (the leak gate, unit slice): a confidential KN page humanised for a DENIED
    /// viewer across every channel × every KN reason → 0 title leak.** The resolver feeds Notif's REAL
    /// humanise; the title appears EXACTLY ZERO times — the per-viewer Display slot binds to the
    /// PII-free tombstone before the formatter ever runs. Threshold 0, never softened. The KN reason
    /// templates are the SAME platform-default keys NOTIF-P20 registers (`mentioned` / `comments` /
    /// `shared` / `watched`) — the ONE templating surface, no second engine.
    #[test]
    fn notif_d4_zero_title_leak_through_humanise() {
        let (resolver, page) = confidential_page_resolver(false); // denied
        let templates = TemplateStore::with_platform_defaults();
        let reasons = [Reason::Mentioned, Reason::Comments, Reason::Shared, Reason::Watched];
        let mut renders = 0u64;
        let mut leaks = 0u64;
        let mut tombstones = 0u64;
        for &reason in &reasons {
            let key = reason_template_key(reason);
            for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
                let h = humanise(
                    &resolver,
                    &acme(),
                    &region(),
                    &templates,
                    key,
                    std::slice::from_ref(&page),
                    &viewer("ex-contractor"),
                    DEFAULT_LOCALE,
                    &strong("zk-1"),
                    channel,
                );
                renders += 1;
                if h.text.contains(SECRET_TITLE) || h.text.to_lowercase().contains("nightfall") {
                    leaks += 1;
                }
                if h.text.contains("a restricted page") {
                    tombstones += 1;
                }
                assert!(h.links.is_empty(), "a denied KN unfurl yields no click-route link");
            }
        }
        assert_eq!(leaks, 0, "NOTIF-D4-class: 0 title leak over {renders} denied KN renders (threshold 0)");
        assert_eq!(
            tombstones, renders,
            "every denied render shows the PII-free `a restricted page` tombstone (the embed degrades)"
        );
        eprintln!(
            "NOTIF-D4 GREEN through KN resolve→humanise (2026-06-22): {renders} denied renders, \
             title-leak-count = {leaks} (threshold 0), tombstone = {tombstones}/{renders}"
        );
    }

    /// **The KN tombstone-reason mapping is total + PII-free** (every KN ladder reason maps to a Notif
    /// content-free reason; never a free-text leak). A mutant that mis-maps a reason is caught.
    #[test]
    fn tombstone_reason_mapping_is_total_and_pii_free() {
        assert_eq!(map_tombstone_reason(KnTombstoneReason::Denied), TombstoneReason::Denied);
        assert_eq!(map_tombstone_reason(KnTombstoneReason::RootGone), TombstoneReason::RootGone);
        assert_eq!(map_tombstone_reason(KnTombstoneReason::SubGone), TombstoneReason::SubGone);
        assert_eq!(map_tombstone_reason(KnTombstoneReason::Erased), TombstoneReason::Erased);
    }

    /// **An erased KN subject humanises to `[erased user]` (the erasure-safe display, EI-04 §1).** A
    /// crypto-shred / pseudonym-shred (KN-D4) makes the content unrenderable; the resolve degrades to
    /// the canonical erased display, never the title.
    #[test]
    fn an_erased_kn_subject_humanises_to_the_erased_display() {
        let page = secret_page();
        let mut store = PageStore::new();
        store.put_root(&page, PageMeta { title: SECRET_TITLE.into(), state: "published".into() });
        store.mark_erased(&page);
        let resolver = KnowledgeRefResolver::new(Projector::new(StubId::new().allow_read(&page), store));
        let h = humanise(
            &resolver,
            &acme(),
            &region(),
            &TemplateStore::with_platform_defaults(),
            reason_template_key(Reason::Mentioned),
            std::slice::from_ref(&page),
            &viewer("any"),
            DEFAULT_LOCALE,
            &strong("zk-1"),
            Channel::Cli,
        );
        assert!(h.text.contains("[erased user]"), "an erased KN subject renders the erased display");
        assert!(!h.text.contains(SECRET_TITLE), "the erased subject's title never leaks");
    }

    /// **A malformed / non-KN ref degrades to a non-leaking `RootGone` tombstone (never a panic).**
    #[test]
    fn a_non_kn_ref_degrades_to_a_non_leaking_tombstone() {
        let (resolver, _) = confidential_page_resolver(false);
        let not_kn = ArtifactRef("myelin://acme/git/pr/9".into());
        let r = resolver.resolve_display(&acme(), &region(), &not_kn, &viewer("alice"), &strong("zk-1"));
        match r {
            RefResolution::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::RootGone),
            RefResolution::Projection(_) => panic!("a non-KN ref must not project a title"),
        }
    }
}
