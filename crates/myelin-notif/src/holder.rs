//! Notif as a `PersonalDataHolder` (H13 `NotificationHistory`) — the REAL structural
//! references-not-payloads erasure surface + the harness auto-registration (NOTIF-P4 / P-182;
//! contract 7.7 holder half + 10.1 + 1.4).
//!
//! **Architecture:** `notifications.md` §3.9 (Notif IS the notification-history holder —
//! `locate/export/rectify/restrict/erase`, auto-registered by `serve(AppSpec)` so "we forgot
//! notification history" is structurally impossible; because items store **refs not strings**
//! (§2.1), **erasing a person tombstones their appearance in everyone's inbox FOR FREE** —
//! references-not-payloads (ADR-12.4) does the work; most of the inbox needs **no mutation** on
//! erasure), §2.1 (`template_args` holds refs not strings — what makes erasure free). The
//! exhaustive H1–H18 catalog ([`myelin_substrate::Holder`]) names Notif **H13
//! (`NotificationHistory`)**.
//!
//! ## What NOTIF-P4 ships — the holder HALF of 7.7 (registration + the structural erase)
//! Notif registers its OLTP store (the `notif_inbox_item`/`notif_delivery` tables, NOTIF-P2) as the
//! H13 holder through the harness one-door auto-registration (1.4 / 10.1), and implements the
//! five-operation `PersonalDataHolder` surface (7.7) over the inbox. The load-bearing property is
//! the **structural references-not-payloads erase**: because an inbox row stores the subject ONLY
//! as
//!
//!   1. the **`recipient`** — an OPAQUE Principal pseudonym (the subject's own inbox, 4.8), and
//!   2. referenced actors in the **`subject`/`origin_event`/`template_args`** [`ArtifactRef`]s
//!      (someone ELSE's inbox row that names the subject *by reference*),
//!
//! and NEVER as a stored name/email, erasing the subject tombstones their appearance in every inbox
//! **with NO PII-column mutation** — the title resolves to a tombstone at *read* time via the stored
//! ref (Identity's 4.8 pseudonym-map shred makes the opaque id unresolvable), not by scrubbing a
//! column. The holder `locate`s the appearances and `erase` RELIES on that structural posture; it
//! does **not** rewrite the refs-stored rows.
//!
//! ## The stub → the real surface (the EI-01 §7 reconcile, NOT a parallel second holder)
//! There is ONE Notif holder type ([`NotifHistoryHolder`]). Unbacked (the [`Default`]
//! registration-only form — the `serve`-before-the-router-projection-is-wired posture) it is
//! **empty-but-correct** (a tenant whose inbox the router has not populated has no located items).
//! Backed ([`NotifHistoryHolder::with_inbox`]) it runs the REAL structural body over the live
//! [`crate::router::InboxProjection`] the [`crate::SignalRouter`] (NOTIF-P3) UPSERTs into — the SAME
//! projection, never a parallel second store. The body is the real one the moment the projection is
//! wired.
//!
//! ## FLOORS named (VISION §3 / EI-01 §1 name-your-floors — the holder is NOT the full erasure path)
//! - **The off-cell-payload erasure residual** (a name in an already-DELIVERED off-cell *redacted*
//!   summary — the one inline-PII case Notif emits free text outside the cell) is handled **BY
//!   REFERENCE** to the platform erasure posture (`00-reconciliation §X-7`, contract **10.9**) — the
//!   per-subject DEK crypto-shred of inline-PII delivery columns + the `restrict` suppression + the
//!   provider-side erasure request (the named sub-processor obligation). Notif does NOT restate the
//!   posture; the Notif instancing of the residual lands in **NOTIF-P27** (P-469, N-M5.2). The
//!   DEK-crypto-shred of inline-PII delivery columns is therefore NOT performed here (the inbox
//!   stores refs, so the per-subject erase needs no key destroyed at the inbox surface;
//!   `key_epoch_destroyed = None`).
//! - **The reindex/replay half of 7.7** (`replay(scope, since)` — the inbox rebuilt by
//!   reindex-from-source, the only recovery path) is **NOTIF-P17** (P-195). This module is the
//!   holder half only; the two together complete contract 7.7.
//! - **`restrict` suppression into live routing/delivery** (stop NEW routing/delivery for a
//!   restricted subject — the §3.9 Art. 18/21 suppression) records the restriction in a shared
//!   suppression set here; the router/delivery consult of that set lands with the routing/delivery
//!   bodies (NOTIF-P10/P16). The holder records the op + the suppression set is real.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! The holder is erasure-correctness critical (the X-7 posture for Notif). Floor: **≥ 80% of viable
//! mutants caught** (`cargo mutants -p myelin-notif -f crates/myelin-notif/src/holder.rs`). Measured
//! 2026-06-20: **38 mutants generated → 23 unviable, 15 viable, 15 caught, 0 missed = 100% of viable**
//! — floor met. Every body — `locate`'s real appearance count, the structural-erase 0-mutation
//! property, the backed-vs-unbacked split, the restrict-set write + the shared-set accessors, the
//! registration + H13 classification — has a test a mutation flips.

use std::sync::{Arc, Mutex};

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, Result as DsrResult, RestrictReceipt, SubjectRef, TenantId as GdprTenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreHolder, StoreKind};
use myelin_tenancy::TenantId;

use crate::router::InboxProjection;

/// The stable, PII-free name of the Notif **OLTP store** (the `notif_inbox_item`/`notif_delivery`
/// tables, NOTIF-P2 — the holder's H13 store). Frozen here so the NOTIF-P2 migrations, the data-map
/// (P-GA-09), and the DSR fan-out all address exactly this store. PII-free: a store identifier,
/// never personal data.
pub const NOTIF_OLTP_STORE: &str = "notif_oltp";

/// The typed receipt that the Notif store was auto-registered as a [`PersonalDataHolder`] — the
/// proof the registration fired (mirrors `myelin_substrate::HolderRegistration`). The harness
/// collects these; the holder-registered architecture test reads them to assert the Notif store did
/// not escape registration. PII-free: a (kind, name) tag.
pub type NotifHolderRegistration = HolderRegistration;

/// Build the Notif [`StoreClassifier`] — the data-map declaration that the Notif OLTP store belongs
/// to holder **H13 (`NotificationHistory`)** (gdpr §3.2). The OLTP store names its holder explicitly
/// (the three non-OLTP kinds classify structurally; an OLTP store with no declaration is an orphan).
pub fn notif_store_classifier() -> StoreClassifier {
    StoreClassifier::of([StoreHolder::new(
        StoreKind::Oltp,
        NOTIF_OLTP_STORE,
        Holder::H13NotificationHistory,
    )])
}

/// **Register Notif's store as a `PersonalDataHolder` through the harness auto-registration (contract
/// 1.4).** Opens the Notif OLTP store through the substrate [`HolderRegistry`] — the ONE door — so it
/// is a registered holder by construction. Registering ALWAYS (even before the projection is wired)
/// makes "the DSAR fan-out forgot notification history" structurally impossible (10.1 / §3.9
/// exhaustiveness — the exact bug VISION §3 names).
pub fn register_notif_holder() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, NOTIF_OLTP_STORE);
    registry
}

/// The Art. 18/21 restriction-suppression set (the `restrict` body's shared state) — the set of
/// subjects whose NEW routing/delivery the router/delivery suppress (§3.9). A cloneable handle over
/// shared state so the holder's `restrict(subject, on)` write and the router/delivery read (NOTIF-
/// P10/P16) observe ONE truth. PII-free: it holds opaque pseudonymous subject ids, never names.
#[derive(Clone, Default)]
pub struct RestrictSet {
    inner: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl RestrictSet {
    /// A fresh, empty suppression set.
    pub fn new() -> RestrictSet {
        RestrictSet::default()
    }

    /// Set (`on = true`) or clear (`on = false`) the restriction for `subject_id` (Art. 18/21).
    /// Idempotent: setting an already-restricted subject is a no-op.
    pub fn set(&self, subject_id: &str, on: bool) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            g.insert(subject_id.to_string());
        } else {
            g.remove(subject_id);
        }
    }

    /// Whether `subject_id`'s NEW routing/delivery is currently suppressed (the router/delivery read).
    pub fn is_restricted(&self, subject_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(subject_id)
    }
}

/// The live runtime the REAL NOTIF-P4 holder body operates over: the inbox projection (to `locate`
/// the subject's appearances + report the structural-erase surface) + the restrict-suppression set
/// (to suppress a restricted subject's NEW routing/delivery). **References-not-payloads:** the holder
/// reads only the OPAQUE recipient pseudonym + the structured refs — never a stored name. Cloneable.
#[derive(Clone)]
pub struct NotifBacking {
    /// The live inbox projection (NOTIF-P3) — the holder scans it for the subject's appearances.
    inbox: InboxProjection,
    /// The restrict-suppression set (Art. 18/21) — `restrict(subject, true)` records the subject so
    /// the router/delivery keep its NEW routing/delivery suppressed (§3.9).
    restrict: RestrictSet,
}

impl NotifBacking {
    /// Wire the holder over a live inbox projection (the NOTIF-P4 real body). The restrict set is
    /// fresh (empty) — `restrict(subject, true)` adds to it.
    pub fn new(inbox: InboxProjection) -> NotifBacking {
        NotifBacking { inbox, restrict: RestrictSet::new() }
    }

    /// Wire the holder over a live projection AND a shared restrict-suppression set (so the
    /// suppression a holder records is the SAME set the router/delivery consult).
    pub fn with_restrict(inbox: InboxProjection, restrict: RestrictSet) -> NotifBacking {
        NotifBacking { inbox, restrict }
    }

    /// The shared restrict-suppression set (the router/delivery read it to suppress a restricted
    /// subject's NEW routing/delivery).
    pub fn restrict_set(&self) -> &RestrictSet {
        &self.restrict
    }
}

/// Notif's **notification history** AS a [`PersonalDataHolder`] (H13; contract 7.7 holder half +
/// 10.1). NOTIF-P4: the REAL structural references-not-payloads erasure surface when
/// [`Self::with_inbox`] wires the live inbox projection; **empty-but-correct** (the registration-only
/// [`Default`] form) when unbacked (`serve` before the router projection is populated). Cloneable.
#[derive(Clone, Default)]
pub struct NotifHistoryHolder {
    /// `None` = the registration-only stub (empty-but-correct); `Some` = the REAL NOTIF-P4 body over
    /// the live inbox projection + the restrict set.
    backing: Option<NotifBacking>,
}

impl NotifHistoryHolder {
    /// **The REAL NOTIF-P4 holder over a live inbox projection (§3.9).** `locate` walks the
    /// projection for items naming the subject (as recipient OR a referenced actor ref); `erase` is
    /// the STRUCTURAL references-not-payloads erase (the appearance tombstones for free via Identity's
    /// 4.8 pseudonym shred — NO PII-column mutation on the refs-stored rows); `restrict` suppresses
    /// the subject's NEW routing/delivery.
    pub fn with_inbox(inbox: InboxProjection) -> NotifHistoryHolder {
        NotifHistoryHolder { backing: Some(NotifBacking::new(inbox)) }
    }

    /// The REAL holder over a live projection AND a shared restrict set (the router/delivery read it).
    pub fn with_backing(backing: NotifBacking) -> NotifHistoryHolder {
        NotifHistoryHolder { backing: Some(backing) }
    }

    /// Register this holder through the substrate registry (the `serve`-called auto-registration
    /// seam), returning the receipt — the proof the Notif store registered as holder H13.
    pub fn register(&self, registry: &mut HolderRegistry) -> NotifHolderRegistration {
        registry.open(StoreKind::Oltp, NOTIF_OLTP_STORE)
    }

    /// The shared restrict-suppression set (when backed) — so a test / the router can read the
    /// suppression the holder records.
    pub fn restrict_set(&self) -> Option<&RestrictSet> {
        self.backing.as_ref().map(|b| b.restrict_set())
    }

    /// The opaque, PII-free subject id the receipt body keys on (the pseudonymous Principal id) —
    /// never a name/email. This is the opaque `recipient`/actor-ref pseudonym posture (§3.9, 4.8).
    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }

    /// Count the inbox items naming the subject (the structural `locate` surface): rows where the
    /// subject is the **recipient** (their own inbox) OR a referenced actor in a stored ref. Returns
    /// 0 when unbacked. Tenant-first (the fan-out is per (subject, tenant)).
    fn count_appearances(&self, tenant: &GdprTenantId, subject_id: &str) -> usize {
        let Some(b) = &self.backing else {
            return 0;
        };
        let t = TenantId(tenant.0.clone());
        b.inbox
            .snapshot_for_tenant(&t)
            .iter()
            .filter(|row| row.references_subject(subject_id))
            .count()
    }
}

impl PersonalDataHolder for NotifHistoryHolder {
    fn locate(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<LocateReport> {
        // REAL §3.9 locate: the inbox items naming the subject (by the OPAQUE recipient pseudonym OR a
        // referenced-actor ref — never a name). Unbacked → empty-but-correct (0 located). Tenant-first.
        let sid = Self::subject_id(subject);
        let count = self.count_appearances(&tenant, &sid);
        let outcome = format!(
            "located {count} inbox items naming the subject (recipient pseudonym + referenced-actor \
             refs, references-not-payloads — no stored name)"
        );
        Ok(LocateReport {
            receipt: Receipt::content_addressed("locate", NOTIF_OLTP_STORE, &sid, &tenant.0, &outcome, None, 0),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<PortableBundle> {
        // The inbox is a PROJECTION (it holds no free-text body the subject is the controller for — its
        // subject data is the opaque recipient pseudonym + structured refs, both already covered by the
        // owning subsystems' exports + Identity). The portable bundle is the located-appearance count
        // receipt (references-not-payloads — nothing to export but the count + a content-address).
        let sid = Self::subject_id(subject);
        let count = self.count_appearances(&tenant, &sid);
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                NOTIF_OLTP_STORE,
                &sid,
                &tenant.0,
                &format!("references-not-payloads bundle: {count} inbox appearances, no free-text body"),
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        // The inbox stores refs, never rendered strings (NOTIF-1) → rectification of an item is via
        // reindex-from-source over the corrected owner content + the re-resolved title at read time
        // (NOTIF-P17), never an in-place edit here. A no-op at the holder surface (correct: there is
        // nothing to rectify in a refs-stored row — the re-resolve corrects it).
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                NOTIF_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (references-not-payloads — rectify via reindex-from-source + read-time re-resolve, NOTIF-P17)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // REAL §3.9 restrict (Art. 18/21): record the subject in the suppression set so the router /
        // delivery keep its NEW routing/delivery suppressed (and indexing/agent-use/analytics, 10.1).
        // Unbacked → a well-defined no-op (no live routing to suppress over). Idempotent.
        let sid = Self::subject_id(subject);
        let applied = match &self.backing {
            Some(b) => {
                b.restrict.set(&sid, on);
                true
            }
            None => false,
        };
        let outcome = if applied {
            format!("restrict={on} recorded in the suppression set (new routing/delivery suppressed; indexing/agent-use too)")
        } else {
            format!("restrict={on} no-op (no live routing; suppression lands with routing/delivery NOTIF-P10/P16)")
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed("restrict", NOTIF_OLTP_STORE, &sid, "", &outcome, None, 0),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        // REAL §3.9 erase: Notif's erasure surface is SMALL + STRUCTURAL (references-not-payloads). An
        // inbox row stores the subject ONLY as the OPAQUE recipient pseudonym + structured refs; the
        // appearance TOMBSTONES FOR FREE — Identity's 4.8 pseudonym-map shred makes the opaque id
        // unresolvable, and the title resolves to a tombstone at READ time via the stored ref. So the
        // holder erase needs NO PII-column mutation on the refs-stored rows (the structural property
        // the gate pins): it reports the surface covered + relies on the platform posture. It destroys
        // NO key at the inbox surface (key_epoch_destroyed = None) — the inline-PII delivery-column
        // DEK-crypto-shred + the off-cell-payload residual are the X-7 / 10.9 floor instanced in
        // NOTIF-P27. The reindex/replay rebuild is NOTIF-P17. No erasure backdoor: the row stays; the
        // person becomes unresolvable.
        let (sid, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => (Self::subject_id(subject), tenant.0.clone()),
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        let count = match &scope {
            EraseScope::Subject { tenant, .. } => self.count_appearances(tenant, &sid),
            // A tenant erase is the crypto-shred (destroy the per-tenant DEK) — the tenant-decommission
            // lever (11.3/11.4), not a per-item scan here.
            EraseScope::Tenant(_) => 0,
        };
        let outcome = match &scope {
            EraseScope::Subject { .. } => format!(
                "structural erase: {count} inbox appearances tombstone for free (refs-not-payloads; \
                 Identity 4.8 pseudonym-shred makes the opaque id unresolvable) — 0 PII columns mutated; \
                 off-cell residual + inline-PII DEK shred = X-7/10.9 (NOTIF-P27); replay NOTIF-P17"
            ),
            EraseScope::Tenant(_) => {
                "tenant crypto-shred: destroy the per-tenant DEK (11.3/11.4) — every inbox row unrecoverable".into()
            }
        };
        Ok(EraseReceipt {
            // No KEY destroyed at the inbox holder (the row is refs-only; the crypto-shred is the
            // inline-PII delivery DEK + the per-tenant DEK, the X-7/10.9 floor). key_epoch_destroyed = None.
            receipt: Receipt::content_addressed("erase", NOTIF_OLTP_STORE, &sid, &tenant, &outcome, None, 0),
        })
    }
}

/// The H-holder the Notif OLTP store classifies to (H13 `NotificationHistory`) — a convenience over
/// [`myelin_substrate::classify_store`] against the Notif classifier. Returns the holder (always
/// `Some(H13NotificationHistory)` for the declared store) so a caller can pin the classification.
pub fn notif_history_holder() -> Option<Holder> {
    myelin_substrate::classify_store(StoreKind::Oltp, NOTIF_OLTP_STORE, &notif_store_classifier())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{InboxProjection, RoutedInboxItem};
    use crate::{Class, Reason};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_refs::ArtifactRef;
    use myelin_substrate::{assert_holder_completeness, classify_store};
    use myelin_tenancy::Region;

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            GdprTenantId::from_token("acme"),
        ))
    }

    fn tenant() -> GdprTenantId {
        GdprTenantId::from_token("acme")
    }

    fn t() -> TenantId {
        TenantId::from_token("acme")
    }

    /// A row in `acme`'s inbox: `recipient`'s own inbox row about `subject`, naming `actor` by ref in
    /// `origin_event` (a referenced actor — the someone-else's-inbox case). All refs, never a name.
    fn row(recipient: &str, subject: &str, actor: &str, dedup_key: &str) -> RoutedInboxItem {
        RoutedInboxItem {
            tenant: t(),
            region: Region::new("fr-par"),
            item_id: format!("itm-{dedup_key}"),
            recipient: recipient.into(),
            subject: ArtifactRef(format!("myelin://acme/issues/issue/{subject}")),
            reason: Reason::Mentioned,
            class: Class::Direct,
            origin_event: ArtifactRef(format!("myelin://acme/identity/principal/{actor}")),
            dedup_key: dedup_key.into(),
            coalesce_count: 1,
            state: "unread".into(),
        }
    }

    /// **Notif registers its store as a holder through the one door (contract 1.4).** The OLTP store
    /// is opened through the substrate registry, so it is a registered holder by construction — 0
    /// stores escape registration (the §3.9 "we forgot notification history" bug is impossible).
    #[test]
    fn notif_registers_its_store_as_a_holder() {
        let registry = register_notif_holder();
        assert!(registry.is_registered(StoreKind::Oltp, NOTIF_OLTP_STORE));
        assert_eq!(registry.len(), 1, "exactly the one Notif store registered");
    }

    /// **Re-registration is idempotent** — `serve` re-running the registration on a restart records
    /// the Notif store exactly once.
    #[test]
    fn re_registration_is_idempotent() {
        let mut registry = register_notif_holder();
        NotifHistoryHolder::default().register(&mut registry);
        assert_eq!(registry.len(), 1, "re-opening the same Notif store does not double-register");
    }

    /// **The Notif store classifies to H13 — 0 orphans (contract 1.4 + gdpr §3.2).** The OLTP store
    /// maps to **H13 (`NotificationHistory`)** via the declared classifier. The substrate completeness
    /// assertion is GREEN — the Notif store is inside the exhaustive H1–H18 list, so the M5 DSAR
    /// fan-out cannot miss notification history.
    #[test]
    fn notif_store_classifies_to_h13_no_orphan() {
        let registry = register_notif_holder();
        let classifier = notif_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, NOTIF_OLTP_STORE, &classifier),
            Some(Holder::H13NotificationHistory),
            "the Notif OLTP store is holder H13"
        );
        assert_eq!(notif_history_holder(), Some(Holder::H13NotificationHistory));
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "the Notif store is in the exhaustive H1–H18 list — 0 orphan stores"
        );
    }

    /// **THE GATE — the structural-erase property (CI): erase a subject → a refs-stored inbox_item
    /// tombstones with NO PII mutation.** A row naming the subject (recipient AND by-ref actor) is
    /// erased; the row's stored columns are UNCHANGED (0 PII columns mutated) — the title resolves to
    /// a tombstone at read time via the stored ref (Identity's 4.8 shred), not by scrubbing a column.
    /// The item still "tombstones" (its appearance is now unresolvable to a human). This is the §3.9
    /// references-not-payloads tombstone-for-free, proven at the unit grain.
    #[test]
    fn structural_erase_tombstones_a_refs_stored_item_with_zero_pii_mutation() {
        let inbox = InboxProjection::new();
        // The subject's OWN inbox row, AND a row in someone else's inbox naming the subject by ref.
        inbox.upsert_for_test(row("u-erase", "PROJ-1", "u-other", "own"));
        inbox.upsert_for_test(row("u-bob", "PROJ-2", "u-erase", "byref"));
        // A control row that does NOT name the subject — must be untouched + uncounted.
        inbox.upsert_for_test(row("u-carol", "PROJ-3", "u-dave", "control"));

        let holder = NotifHistoryHolder::with_inbox(inbox.clone());

        // Snapshot the EXACT stored bytes of the subject's rows BEFORE erase.
        let before: Vec<RoutedInboxItem> = inbox.snapshot_for_tenant(&t());
        let subj_rows_before: Vec<&RoutedInboxItem> =
            before.iter().filter(|r| r.references_subject("u-erase")).collect();
        assert_eq!(subj_rows_before.len(), 2, "locate finds both appearances (own + by-ref)");

        // locate reports the appearance count over the structural surface.
        let loc = holder.locate(&subject("u-erase"), tenant()).expect("locate succeeds");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));
        assert!(loc.receipt.key_epoch_destroyed.is_none(), "locate shreds no key");

        // ERASE the subject.
        let scope = EraseScope::Subject { subject: subject("u-erase"), tenant: tenant() };
        let er = holder.erase(scope.clone()).expect("structural erase succeeds");
        assert!(er.receipt.key_epoch_destroyed.is_none(), "0 keys shredded at the inbox surface (refs-only)");

        // THE PROPERTY: 0 PII columns mutated — every stored row is byte-identical after erase.
        let after: Vec<RoutedInboxItem> = inbox.snapshot_for_tenant(&t());
        let mut before_sorted = before.clone();
        let mut after_sorted = after.clone();
        before_sorted.sort_by(|a, b| a.item_id.cmp(&b.item_id));
        after_sorted.sort_by(|a, b| a.item_id.cmp(&b.item_id));
        assert_eq!(
            after_sorted, before_sorted,
            "the refs-stored items tombstone for FREE — 0 PII columns mutated (references-not-payloads)"
        );
        assert_eq!(after.len(), 3, "no row deleted either — the appearance stays, only resolution changes");

        // Idempotent: a re-erase returns the IDENTICAL content-addressed receipt.
        let er2 = holder.erase(scope).expect("re-erase is idempotent");
        assert_eq!(er, er2, "the same erase scope yields the identical receipt");
    }

    /// **`locate` over the backed projection counts the REAL appearances (the structural surface).**
    /// 0 over an unbacked holder (empty-but-correct), N over the live projection. Pins that the
    /// count is the references-not-payloads predicate, not a constant.
    #[test]
    fn locate_counts_real_appearances_backed_vs_unbacked() {
        let unbacked = NotifHistoryHolder::default();
        assert_eq!(unbacked.count_appearances(&tenant(), "u-x"), 0, "unbacked → empty-but-correct");

        let inbox = InboxProjection::new();
        inbox.upsert_for_test(row("u-x", "PROJ-1", "u-y", "a")); // recipient = subject
        inbox.upsert_for_test(row("u-z", "PROJ-2", "u-x", "b")); // by-ref actor = subject
        inbox.upsert_for_test(row("u-z", "PROJ-3", "u-q", "c")); // neither
        let backed = NotifHistoryHolder::with_inbox(inbox);
        assert_eq!(backed.count_appearances(&tenant(), "u-x"), 2, "both structural appearances counted");
        assert_eq!(backed.count_appearances(&tenant(), "u-none"), 0, "an absent subject → 0");
    }

    /// **`restrict` records the subject in the SHARED suppression set (the router/delivery read it).**
    /// Backed: `restrict(on)` adds, `restrict(off)` clears — the SAME set a router would consult.
    /// Unbacked: a well-defined no-op. Idempotent.
    #[test]
    fn restrict_writes_the_shared_suppression_set() {
        let restrict = RestrictSet::new();
        let backing = NotifBacking::with_restrict(InboxProjection::new(), restrict.clone());
        let holder = NotifHistoryHolder::with_backing(backing);
        let subj = subject("u-r");

        assert!(!restrict.is_restricted("u-r"), "not restricted initially");
        holder.restrict(&subj, true).expect("restrict on succeeds");
        assert!(restrict.is_restricted("u-r"), "the holder recorded the restriction in the shared set");
        holder.restrict(&subj, false).expect("restrict off succeeds");
        assert!(!restrict.is_restricted("u-r"), "restrict off clears it");

        // Unbacked → a well-defined no-op (no panic), records nothing.
        let unbacked = NotifHistoryHolder::default();
        assert!(unbacked.restrict(&subj, true).is_ok(), "unbacked restrict is a no-op receipt");
    }

    /// **The holder is empty-but-correct unbacked (the registration-only surface), not an error.**
    /// `export`/`locate` over a tenant the router has not populated return content-addressed receipts
    /// over an empty surface — a real, callable holder, never a `todo!()`/`Err`.
    #[test]
    fn unbacked_holder_is_empty_but_correct() {
        let holder = NotifHistoryHolder::default();
        let subj = subject("u-1");
        let loc = holder.locate(&subj, tenant()).expect("locate over empty surface succeeds");
        assert_eq!(loc.receipt.operation, "locate");
        let exp = holder.export(&subj, tenant()).expect("export of empty bundle succeeds");
        assert_eq!(exp.receipt.operation, "export");
        let rec = holder.rectify(&subj, Patch("x".into())).expect("rectify no-op succeeds");
        assert_eq!(rec.receipt.operation, "rectify");
    }

    /// **The `restrict_set` accessors return the SHARED set the holder records into.** Both
    /// [`NotifBacking::restrict_set`] and [`NotifHistoryHolder::restrict_set`] hand back the SAME set
    /// the holder's `restrict` writes — so a router/delivery reader and the holder writer observe ONE
    /// truth. Unbacked → `None` (no live set to suppress over). Pins the accessors are not a constant.
    #[test]
    fn restrict_set_accessors_return_the_shared_set() {
        let restrict = RestrictSet::new();
        let backing = NotifBacking::with_restrict(InboxProjection::new(), restrict.clone());
        // NotifBacking::restrict_set hands back the SAME set (writing through it is visible via the holder).
        backing.restrict_set().set("u-shared", true);
        assert!(restrict.is_restricted("u-shared"), "the backing accessor is the shared set, not a fresh one");

        let holder = NotifHistoryHolder::with_backing(backing);
        // NotifHistoryHolder::restrict_set surfaces Some(&that_same_set).
        let via_holder = holder.restrict_set().expect("backed holder exposes its restrict set");
        assert!(via_holder.is_restricted("u-shared"), "the holder accessor is the SAME shared set");
        via_holder.set("u-shared", false);
        assert!(!restrict.is_restricted("u-shared"), "a write through the holder accessor reaches the shared set");

        // Unbacked → None (no live suppression set).
        assert!(NotifHistoryHolder::default().restrict_set().is_none(), "unbacked → no restrict set");
    }

    /// **The holder is object-safe** — held behind `dyn PersonalDataHolder` exactly as the DSR
    /// orchestrator / holder registry need (a heterogeneous holder set, contract 10.1).
    #[test]
    fn holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(NotifHistoryHolder::default())];
        let subj = subject("u-3");
        for h in &holders {
            assert!(h.locate(&subj, tenant()).is_ok(), "the holder responds to the contract");
        }
    }
}
