//! # The exhaustive `PersonalDataHolder` (H1–H18) catalog + the holder-completeness assertion (P-S27)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §3.2 (the EXHAUSTIVE
//! holder list H1–H18 — "the list is exhaustive and enforced by the data map") and
//! `00-platform-substrate.md` §3.4 (auto-register every store the harness opens; the mechanism
//! makes "we forgot a store" structurally impossible).
//!
//! **Contract-index:** row 1.4 (the exhaustive-holder mechanism against the real H1–H18 set —
//! CONFIRMED here) + 10.1 (the `PersonalDataHolder` trait + the H1–H18 list — GDPR OWNS the list;
//! the substrate CONFIRMS registration completeness against it). **P-S27 → global P-088.**
//! DEPENDS-ON P-S15 (the [`crate::holders::HolderRegistry`] auto-registration mechanism this
//! catalog confirms against), P-S11 (the `no-untagged-personal-data` lint, the GA-D5 mirror — run
//! live over the real schema by `myelin-lints`' `workspace_clean` gate), and the GDPR M1
//! holder-list prompt (which OWNS the §3.2 list; this catalog mirrors it for the substrate-side
//! confirmation).
//!
//! ## Why this module exists (the gap P-S27 fills over P-S15 / P-GA-04)
//! P-S15 shipped the auto-registration MECHANISM ([`crate::holders`]): opening a store IS
//! registering it as a holder, so a harness-opened store cannot escape the DSR fan-out. P-GA-04
//! shipped the ENFORCEMENT ([`crate::holder_registered`]): a store opened OUTSIDE the harness
//! (bypassing the one door) fails the `holder-registered` architecture test. Neither proved the
//! THIRD property the GDPR doctrine names (gdpr §3.2): the holder set is **EXHAUSTIVE** — every
//! store the harness opens maps to one of the **eighteen named holders** (H1–H18), and **no orphan
//! store exists outside the list**. A store that registers but belongs to NO H-holder is a store
//! the §3.2 data map never accounted for — a "we added a store the RoPA inventory doesn't know
//! about" hole, the GDPR + silent-data-loss bug class (EI-01 §2).
//!
//! This module ships exactly that confirmation:
//! - [`Holder`] — the EXHAUSTIVE, closed H1–H18 enum (gdpr §3.2 verbatim). The enum is closed:
//!   adding a nineteenth holder is a deliberate edit here (matching a GDPR-owned §3.2 update),
//!   never an accident — a new STORE KIND with no holder is caught by the completeness assertion;
//!   a new HOLDER is a deliberate co-edit of this enum and the §3.2 list.
//! - [`classify_store`] — maps each opened store (its [`StoreKind`] + PII-free name) to its
//!   H-holder. A store that classifies to NO holder is an ORPHAN (the completeness violation).
//! - [`holder_completeness`] / [`assert_holder_completeness`] — the **holder-completeness
//!   assertion**: every store the harness opens is in the H1–H18 set (no orphan store); a
//!   deliberately-orphaned store (a store opened without a holder classification) fails. This is
//!   the substrate's CONFIRMATION that the mechanism catches every one of the eighteen.
//!
//! ## The classification model (how a store maps to its H-holder)
//! Each holder owns a set of stores of given KINDS (gdpr §3.2 "Holder" column). The mapping is by
//! `(StoreKind, holder-affinity)`: the four substrate store kinds (§3.4 — OLTP / blob / cache /
//! search index) each fall under specific H-holders. An OLTP schema belongs to its subsystem's
//! H-holder (H1 Git, H2 CI, H3 Issues, H4 Knowledge, H5 Chat, H14 Authz tuples, H15 Identity, H16
//! Audit, H18 GDPR own stores); a blob prefix is H6 (object store); a cache namespace is H9
//! (caches/CDN); a search index is H7. A subsystem declares which holder its OLTP store belongs to
//! via [`StoreHolder`] (the data-map's "this store is holder HN" fact); the four non-OLTP kinds map
//! structurally to their single owning holder (H6/H7/H9) — they are the SAME holder regardless of
//! which subsystem opens them, exactly as gdpr §3.2 lists one object-store holder, one
//! search-index holder, one cache holder for the whole platform.
//!
//! ## Floors named (deferred bodies → filling prompt) — VISION §3 name-your-floors
//! - **GDPR OWNS the §3.2 list itself** (the canonical H1–H18 table lives in
//!   `gdpr-and-audit.md` §3.2; the GDPR M1 holder-list prompt is its code owner). This catalog is
//!   the SUBSTRATE-side MIRROR used to CONFIRM registration completeness (the P-S27 deliverable
//!   "the substrate confirms the mechanism catches every one"). If the §3.2 list grows a holder,
//!   this enum is co-edited — the [`catalog_matches_gdpr_count`] test pins the count at 18 so a
//!   drift is a loud test failure, never a silent miss.
//! - The **per-store H-holder assignment** for the real M1 stores (Identity's principal/pseudonym/
//!   tuple stores → H15/H14, Storage's blob tiers → H6, Search's index → H7, …) is DECLARED by
//!   each store as it ships (P-ID-*, P-ST-*, P-SR-*). This catalog ships the classification
//!   MECHANISM + the exhaustive enum now; the real stores attach their [`StoreHolder`] as they
//!   land. The completeness assertion is live NOW: an OLTP store with no declared holder is an
//!   orphan, caught immediately.
//! - The **DSR bodies** on each holder (`locate/export/rectify/restrict/erase`) are GDPR's M1
//!   deliverable ([`myelin_gdpr::PersonalDataHolder`], P-GA-05). This is a CONFIRMATION prompt —
//!   no new mutation floor (P-S27 TESTS).

use crate::holders::{HolderRegistration, StoreKind};

/// The **exhaustive** `PersonalDataHolder` list (gdpr §3.2, H1–H18) — the eighteen named holders
/// the whole platform's personal data lives in. The enum is **closed + exhaustive**: GDPR's §3.2
/// data map names exactly these eighteen, and the holder-completeness assertion proves every store
/// the harness opens maps to one of them (no orphan store outside the list). PII-free: an H-number
/// tag, never personal data.
///
/// Adding a nineteenth holder is a DELIBERATE co-edit of this enum AND the gdpr §3.2 list (the
/// [`Holder::ALL`] count is pinned at 18 by a drift-guard test). A new STORE that fits no holder is
/// an orphan, caught by the completeness assertion — so "we added a store the RoPA never knew
/// about" is a structural failure, not a review miss (gdpr §3.2; EI-01 §2 — a forgotten store is a
/// GDPR + data-loss hole).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Holder {
    /// H1 — Git subsystem DB (PR/review/comment authorship, free-text bodies).
    H1Git,
    /// H2 — CI subsystem DB + log segments (run actors, log refs, inline log-line PII).
    H2Ci,
    /// H3 — Issues subsystem DB (assignees/watchers/mentions, free-text, worklog).
    H3Issues,
    /// H4 — Knowledge subsystem DB (page authorship, free-text content, db-row values).
    H4Knowledge,
    /// H5 — Chat subsystem DB (message authorship, message bodies).
    H5Chat,
    /// H6 — Object/blob store (avatars, attachments, doc media, CI artifacts).
    H6BlobStore,
    /// H7 — Search index (plaintext-derived tokens + embeddings).
    H7SearchIndex,
    /// H8 — Event-bus history (pseudonymous actor; rare inline-PII events).
    H8EventBus,
    /// H9 — Caches / CDN (derived copies, unfurl renders, clone/bundle blob class).
    H9Caches,
    /// H10 — Backups / snapshots (ciphertext of all of the above).
    H10Backups,
    /// H11 — Agent memory / embeddings (retrieved context, derived embeddings, RAG state).
    H11AgentMemory,
    /// H12 — Reference graph (edges referencing the subject; unfurl projections).
    H12ReferenceGraph,
    /// H13 — Notification history (recipient + actor pseudonyms, humanised strings).
    H13NotificationHistory,
    /// H14 — Authz tuples (`…@subject` tuples + the authz reverse index).
    H14AuthzTuples,
    /// H15 — Identity (Principal/Auth DB + the pseudonym↔real-identity map — the erasure lever).
    H15Identity,
    /// H16 — Audit log (carve-out: who-did-what, minimised).
    H16AuditLog,
    /// H17 — Agent execution trace (a content-addressed Knowledge doc of a run's trace).
    H17AgentTrace,
    /// H18 — GDPR/Audit own stores (G1–G7: DSR subjects, consent records, RoPA).
    H18GdprOwn,
}

impl Holder {
    /// The **exhaustive** H1–H18 set (gdpr §3.2). Eighteen holders — the count is pinned by
    /// [`tests::catalog_is_exhaustive_eighteen`]: a drift (a holder added/removed without a §3.2
    /// co-edit) is a loud test failure.
    pub const ALL: [Holder; 18] = [
        Holder::H1Git,
        Holder::H2Ci,
        Holder::H3Issues,
        Holder::H4Knowledge,
        Holder::H5Chat,
        Holder::H6BlobStore,
        Holder::H7SearchIndex,
        Holder::H8EventBus,
        Holder::H9Caches,
        Holder::H10Backups,
        Holder::H11AgentMemory,
        Holder::H12ReferenceGraph,
        Holder::H13NotificationHistory,
        Holder::H14AuthzTuples,
        Holder::H15Identity,
        Holder::H16AuditLog,
        Holder::H17AgentTrace,
        Holder::H18GdprOwn,
    ];

    /// The stable, PII-free H-number tag (`"H1"`..`"H18"`) — the address the §3.2 data map / RoPA
    /// inventory keys on.
    pub fn tag(self) -> &'static str {
        match self {
            Holder::H1Git => "H1",
            Holder::H2Ci => "H2",
            Holder::H3Issues => "H3",
            Holder::H4Knowledge => "H4",
            Holder::H5Chat => "H5",
            Holder::H6BlobStore => "H6",
            Holder::H7SearchIndex => "H7",
            Holder::H8EventBus => "H8",
            Holder::H9Caches => "H9",
            Holder::H10Backups => "H10",
            Holder::H11AgentMemory => "H11",
            Holder::H12ReferenceGraph => "H12",
            Holder::H13NotificationHistory => "H13",
            Holder::H14AuthzTuples => "H14",
            Holder::H15Identity => "H15",
            Holder::H16AuditLog => "H16",
            Holder::H17AgentTrace => "H17",
            Holder::H18GdprOwn => "H18",
        }
    }
}

/// A store's declared H-holder assignment (the data-map's "this OLTP store belongs to holder HN"
/// fact). A subsystem opening its OLTP schema declares which of the H1–H18 holders the store falls
/// under (H1 Git, H3 Issues, H15 Identity, …); the four non-OLTP kinds (blob/cache/search) map
/// structurally to their single platform-wide holder (H6/H9/H7), so they need no per-store
/// declaration. A [`StoreHolder`] threads the declaration through the [`StoreClassifier`] so a
/// store with no holder is an ORPHAN (the completeness violation). PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreHolder {
    /// The store's class (§3.4).
    pub kind: StoreKind,
    /// The store's stable, PII-free name (the same name the registry keys on).
    pub name: &'static str,
    /// The H-holder this store belongs to (gdpr §3.2).
    pub holder: Holder,
}

impl StoreHolder {
    /// Declare an OLTP store's H-holder (the per-subsystem assignment, e.g. `("issue_oltp", H3)`).
    pub fn new(kind: StoreKind, name: &'static str, holder: Holder) -> StoreHolder {
        StoreHolder { kind, name, holder }
    }
}

/// The per-service declaration of which H-holder each OLTP store it opens belongs to (gdpr §3.2).
/// The non-OLTP kinds (blob → H6, cache → H9, search index → H7) are classified STRUCTURALLY by
/// [`classify_store`] (one platform-wide holder per kind), so only OLTP stores need a declaration
/// here. The completeness assertion joins the harness's [`crate::holders::HolderRegistry`] against
/// this classifier: every opened store must map to an H-holder, or it is an orphan.
#[derive(Clone, Debug, Default)]
pub struct StoreClassifier {
    declarations: Vec<StoreHolder>,
}

impl StoreClassifier {
    /// An empty classifier (no OLTP holder assignment declared yet).
    pub fn new() -> StoreClassifier {
        StoreClassifier { declarations: Vec::new() }
    }

    /// Build a classifier from a set of OLTP store→holder assignments.
    pub fn of(decls: impl IntoIterator<Item = StoreHolder>) -> StoreClassifier {
        StoreClassifier { declarations: decls.into_iter().collect() }
    }

    /// Declare an OLTP store's H-holder.
    pub fn declare(&mut self, kind: StoreKind, name: &'static str, holder: Holder) -> &mut StoreClassifier {
        self.declarations.push(StoreHolder::new(kind, name, holder));
        self
    }

    /// The declared OLTP-store→holder assignments.
    pub fn declarations(&self) -> &[StoreHolder] {
        &self.declarations
    }
}

/// Classify an opened store (its [`StoreKind`] + PII-free name) into its H-holder, against a
/// service's [`StoreClassifier`] (gdpr §3.2). Returns the [`Holder`] the store belongs to, or
/// `None` if the store maps to NO holder — the **orphan** verdict (a store outside the exhaustive
/// H1–H18 list, the completeness violation).
///
/// The four §3.4 store kinds classify as:
/// - **blob** → always [`Holder::H6BlobStore`] (the single platform-wide object store, gdpr §3.2);
/// - **cache** → always [`Holder::H9Caches`] (the single platform-wide caches/CDN holder);
/// - **search index** → always [`Holder::H7SearchIndex`] (the single search-index holder);
/// - **OLTP** → the subsystem-specific holder the [`StoreClassifier`] declares for that store name
///   (H1 Git / H3 Issues / H15 Identity / …). An OLTP store with NO declared holder is the orphan.
pub fn classify_store(
    kind: StoreKind,
    name: &str,
    classifier: &StoreClassifier,
) -> Option<Holder> {
    match kind {
        // The three non-OLTP kinds map structurally to their single platform-wide holder — the
        // SAME holder regardless of which subsystem opens them (gdpr §3.2 lists ONE object store,
        // ONE search index, ONE caches holder for the whole platform).
        StoreKind::Blob => Some(Holder::H6BlobStore),
        StoreKind::Cache => Some(Holder::H9Caches),
        StoreKind::SearchIndex => Some(Holder::H7SearchIndex),
        // An OLTP schema belongs to its subsystem's holder — declared by the subsystem. No
        // declaration ⇒ orphan (a store the §3.2 data map never accounted for).
        StoreKind::Oltp => classifier
            .declarations()
            .iter()
            .find(|d| d.kind == StoreKind::Oltp && d.name == name)
            .map(|d| d.holder),
    }
}

/// A store the harness opened that classifies to NO H-holder — an **orphan store outside the
/// exhaustive H1–H18 list** (gdpr §3.2). This is the holder-completeness violation: a store the
/// RoPA/data-map inventory never accounted for would escape the DSR fan-out AND the §3.2
/// completeness guarantee, so it is a structural (build-time) failure, never a quiet miss (EI-01
/// §2 — a forgotten store is a GDPR + data-loss hole). PII-free.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrphanStore {
    /// The orphan store's class (§3.4).
    pub kind: StoreKind,
    /// The orphan store's stable, PII-free name.
    pub name: String,
}

impl OrphanStore {
    /// A loud, PII-free message naming the orphan store + WHY it failed (it maps to none of the
    /// eighteen H-holders, so it is outside the exhaustive §3.2 list).
    pub fn message(&self) -> String {
        format!(
            "holder-completeness assertion FAILED: store `{}:{}` maps to NONE of the exhaustive \
             H1–H18 holders (gdpr §3.2) — it is an ORPHAN store outside the data map. A store the \
             RoPA inventory never accounted for escapes the DSR fan-out + the §3.2 completeness \
             guarantee (EI-01 §2 — a forgotten store is a GDPR + data-loss hole). Declare its \
             H-holder in the service's StoreClassifier (an OLTP store), or add it to the §3.2 list \
             + the Holder enum (a genuinely new holder kind, a deliberate GDPR co-edit).",
            self.kind.label(),
            self.name,
        )
    }
}

/// **The holder-completeness assertion (the P-S27 GATE — pure core).** Joins the set of stores the
/// harness OPENED (the [`HolderRegistration`] receipts the auto-registration mechanism produced)
/// against the exhaustive H1–H18 catalog: every opened store MUST classify into one of the
/// eighteen holders (gdpr §3.2). Returns the set of [`OrphanStore`]s — stores that map to NO
/// holder (the completeness violations). An **empty** result is the green verdict (no orphan store;
/// every store the harness opens is in the H1–H18 set).
///
/// This is the substrate's CONFIRMATION (the P-S27 deliverable): the auto-registration mechanism
/// (P-S15) opens every store through the one door; this assertion confirms each of those opened
/// stores is accounted for in the exhaustive holder list — so no store escapes the §3.2 data map.
pub fn holder_completeness(
    opened: &[HolderRegistration],
    classifier: &StoreClassifier,
) -> Vec<OrphanStore> {
    opened
        .iter()
        .filter(|reg| classify_store(reg.kind, reg.name, classifier).is_none())
        .map(|reg| OrphanStore { kind: reg.kind, name: reg.name.to_string() })
        .collect()
}

/// **The CI gate (P-S27): every store the harness opens is in the H1–H18 set (no orphan store).**
/// Runs [`holder_completeness`]; returns `Ok(())` when every opened store classifies into one of
/// the eighteen holders (the green verdict), or `Err(orphans)` naming every store that maps to no
/// holder (a store outside the exhaustive §3.2 list). A service's holder-completeness test asserts
/// this is `Ok` — a deliberately-orphaned store (one opened without a holder classification) makes
/// it `Err`, surfaced as a loud CI failure (gdpr §3.2; EI-01 §5 — a committed gate).
pub fn assert_holder_completeness(
    opened: &[HolderRegistration],
    classifier: &StoreClassifier,
) -> Result<(), Vec<OrphanStore>> {
    let orphans = holder_completeness(opened, classifier);
    if orphans.is_empty() {
        Ok(())
    } else {
        Err(orphans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holders::HolderRegistry;
    use std::collections::BTreeSet;

    /// The catalog is EXHAUSTIVE — exactly eighteen holders (gdpr §3.2: H1–H18). This is the drift
    /// guard: a holder added/removed without a §3.2 co-edit (and a count update here) is a loud
    /// failure, so the substrate catalog can never silently diverge from the GDPR-owned list.
    #[test]
    fn catalog_is_exhaustive_eighteen() {
        assert_eq!(Holder::ALL.len(), 18, "the §3.2 holder list is exhaustive: H1–H18");
        // every H-tag is distinct H1..H18 (no duplicate / no gap).
        let tags: BTreeSet<&str> = Holder::ALL.iter().map(|h| h.tag()).collect();
        assert_eq!(tags.len(), 18, "the eighteen H-tags are distinct");
        for n in 1..=18 {
            assert!(tags.contains(format!("H{n}").as_str()), "the catalog names H{n}");
        }
    }

    /// **The GREEN verdict (the holder-completeness gate passing).** Every store the harness opens
    /// classifies into one of the eighteen H-holders: an OLTP store declared as H3 (Issues), a blob
    /// prefix (→ H6 structurally), a cache namespace (→ H9), a search index (→ H7). No orphan store
    /// — the §3.2 completeness guarantee holds.
    #[test]
    fn every_opened_store_maps_to_an_h_holder_no_orphan() {
        // The harness opens four stores through the one door.
        let mut reg = HolderRegistry::new();
        reg.open(StoreKind::Oltp, "issue_oltp");
        reg.open(StoreKind::Blob, "issue_blobs");
        reg.open(StoreKind::Cache, "issue_cache");
        reg.open(StoreKind::SearchIndex, "issue_index");
        // the service declares its OLTP store's H-holder (the non-OLTP kinds classify structurally).
        let classifier = StoreClassifier::of([StoreHolder::new(
            StoreKind::Oltp,
            "issue_oltp",
            Holder::H3Issues,
        )]);

        assert!(
            holder_completeness(reg.registrations(), &classifier).is_empty(),
            "every opened store is in the H1–H18 set; no orphan"
        );
        assert_eq!(
            assert_holder_completeness(reg.registrations(), &classifier),
            Ok(()),
            "the holder-completeness assertion passes — no store outside the exhaustive list"
        );
        // the four classify to exactly their §3.2 holders.
        assert_eq!(classify_store(StoreKind::Oltp, "issue_oltp", &classifier), Some(Holder::H3Issues));
        assert_eq!(classify_store(StoreKind::Blob, "issue_blobs", &classifier), Some(Holder::H6BlobStore));
        assert_eq!(classify_store(StoreKind::Cache, "issue_cache", &classifier), Some(Holder::H9Caches));
        assert_eq!(classify_store(StoreKind::SearchIndex, "issue_index", &classifier), Some(Holder::H7SearchIndex));
    }

    /// **The RED verdict (a deliberately-orphaned store fails).** A service opens an OLTP store but
    /// declares NO H-holder for it — so it maps to none of the eighteen. The holder-completeness
    /// assertion FAILS, naming the orphan + why: a store outside the exhaustive §3.2 list escapes
    /// the data map. This is the captured-expected failure the P-S27 GATE requires.
    #[test]
    fn a_deliberately_orphaned_store_fails_the_completeness_assertion() {
        let mut reg = HolderRegistry::new();
        reg.open(StoreKind::Oltp, "rogue_oltp"); // opened, but no holder declared → orphan.
        let classifier = StoreClassifier::new(); // declares nothing.

        let orphans = holder_completeness(reg.registrations(), &classifier);
        assert_eq!(
            orphans,
            vec![OrphanStore { kind: StoreKind::Oltp, name: "rogue_oltp".into() }],
            "the OLTP store with no declared holder is the orphan"
        );
        let err = assert_holder_completeness(reg.registrations(), &classifier)
            .expect_err("a store outside the H1–H18 list MUST fail the completeness assertion");
        assert_eq!(err.len(), 1);
        let msg = err[0].message();
        assert!(msg.contains("rogue_oltp"), "names the orphan store: {msg}");
        assert!(msg.contains("H1–H18"), "names the exhaustive list: {msg}");
        assert!(msg.contains("ORPHAN"), "names WHY it failed: {msg}");
    }

    /// A PARTIAL orphan: some opened stores classify, one OLTP store does not. The assertion
    /// reports EXACTLY the unclassified one (the store the data map forgot), not the conforming
    /// ones — so the fix is targeted at the actual hole.
    #[test]
    fn reports_only_the_orphan_in_a_partial_violation() {
        let mut reg = HolderRegistry::new();
        reg.open(StoreKind::Oltp, "issue_oltp"); // declared.
        reg.open(StoreKind::Oltp, "shadow_oltp"); // NOT declared → orphan.
        reg.open(StoreKind::Blob, "issue_blobs"); // structural H6.
        let classifier =
            StoreClassifier::of([StoreHolder::new(StoreKind::Oltp, "issue_oltp", Holder::H3Issues)]);

        let orphans = holder_completeness(reg.registrations(), &classifier);
        assert_eq!(orphans.len(), 1, "exactly the one undeclared OLTP store is the orphan");
        assert_eq!(orphans[0], OrphanStore { kind: StoreKind::Oltp, name: "shadow_oltp".into() });
    }

    /// The real M1 holders' OLTP stores classify to their §3.2 H-holders (the confirmation against
    /// the REAL holder set as Identity/Storage/GDPR stores come online — P-S27 DELIVERABLE). Each
    /// subsystem's OLTP store declares its H-number; the assertion confirms the mechanism catches
    /// every one.
    #[test]
    fn the_real_m1_holder_stores_classify_to_their_h_numbers() {
        // The M1 store set as those prompts land: Git→H1, CI→H2, Issues→H3, Knowledge→H4, Chat→H5,
        // Authz tuples→H14, Identity→H15, Audit→H16, GDPR own→H18 (the OLTP-backed holders).
        let classifier = StoreClassifier::of([
            StoreHolder::new(StoreKind::Oltp, "git_oltp", Holder::H1Git),
            StoreHolder::new(StoreKind::Oltp, "ci_oltp", Holder::H2Ci),
            StoreHolder::new(StoreKind::Oltp, "issue_oltp", Holder::H3Issues),
            StoreHolder::new(StoreKind::Oltp, "knowledge_oltp", Holder::H4Knowledge),
            StoreHolder::new(StoreKind::Oltp, "chat_oltp", Holder::H5Chat),
            StoreHolder::new(StoreKind::Oltp, "authz_tuples", Holder::H14AuthzTuples),
            StoreHolder::new(StoreKind::Oltp, "identity_oltp", Holder::H15Identity),
            StoreHolder::new(StoreKind::Oltp, "audit_oltp", Holder::H16AuditLog),
            StoreHolder::new(StoreKind::Oltp, "gdpr_oltp", Holder::H18GdprOwn),
        ]);
        // every declared store classifies to exactly its H-holder (no orphan, no misclassification).
        for d in classifier.declarations() {
            assert_eq!(
                classify_store(d.kind, d.name, &classifier),
                Some(d.holder),
                "the M1 store `{}` classifies to {}",
                d.name,
                d.holder.tag()
            );
        }
        // and the assertion is green over the whole real declared set opened through the registry.
        let mut reg = HolderRegistry::new();
        for d in classifier.declarations() {
            reg.open(d.kind, d.name);
        }
        assert_eq!(assert_holder_completeness(reg.registrations(), &classifier), Ok(()));
    }

    /// An empty harness (no store opened) trivially passes — no opened store ⇒ no orphan.
    #[test]
    fn empty_harness_passes() {
        assert_eq!(
            assert_holder_completeness(&[], &StoreClassifier::new()),
            Ok(())
        );
    }
}
