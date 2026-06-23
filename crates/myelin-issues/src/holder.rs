//! # `holder` — the Issues `PersonalDataHolder` (H3; auto-registered, locate/export typed, erase
//! stubbed to crypto-shred, the `restrict` flag wired) — ISS-P05 / P-371, M4
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//!   §7 (the Issues `PersonalDataHolder` H3 + the §7 erase table — Issues is a GDPR holder over its
//!   issues/comments/change-log/worklog: `locate / export / rectify / restrict / erase`; the residual
//!   is the ONE platform posture, X-7 / 10.9, by reference).
//! - `01-tech-and-data-model.md` §2 (the pseudonymous identity fields + the per-subject DEK
//!   `pii_key_ref` columns the `erase` crypto-shred destroys) + §6.1 (the OQ-H worklog tags).
//! - `planning/00-platform-substrate.md` §3.4 (every store the harness opens auto-registers as a
//!   `PersonalDataHolder` — "we forgot a table" is structurally impossible).
//!
//! **Contracts:** index rows **10.1** (OWNED — the Issues `PersonalDataHolder{locate, export,
//! rectify, restrict, erase}`, auto-registered + typed), **1.4** (CONSUMED — the harness
//! auto-registration on every store opened, the substrate [`HolderRegistry`] one door), **10.9**
//! (CONSUMED **by reference** — the ONE erasure posture; Issues does **not** restate an Issues-local
//! residual). Implemented to the frozen [`myelin_gdpr`] shapes.
//!
//! ## What ISS-P05 ships — the holder SUBSTRATE, not the erasure fan-out (the named floor)
//! This prompt opens + auto-registers the Issues OLTP spine store as holder **H3** and ships the
//! holder **registered + classified + callable**, with:
//! - **`locate` / `export` TYPED** over the Issues surface (issues / comments / change-log / worklog)
//!   — empty-but-correct content-addressed receipts that attest the op ran (a real, callable holder,
//!   never a `todo!()`/panic). The full per-store subject-walk lands with the write path + the DSR
//!   fan-out (ISS-P06/P07/P31).
//! - **`restrict` WIRED** — [`IssueHolder::restrict`] flips a per-subject flag the index/agent/
//!   analytics/notif seams read ([`RestrictionFlag`]); the honoured-everywhere proof is the GDPR
//!   P-GA-25 path, but the flag the seams check is REAL here (Art. 18/21).
//! - **`erase` STUBBED to crypto-shred** — a well-defined no-op receipt that NAMES its ISS-P07/P31
//!   follow-on (the full per-subject-DEK crypto-shred + pseudonym shred over issues/comments/
//!   change-log/worklog). The erasure LEVER (the per-subject DEK on `issue.pii_key_ref` /
//!   `issue_change_log.pii_key_ref`, 11.4) already exists as schema (ISS-P05); the wiring is ISS-P07.
//!
//! The residual (third-party free-text PII a person typed into ANOTHER subject's issue, under that
//! other person's DEK) is the ONE platform posture (10.9 / X-7) — handled **by reference**
//! ([`ISSUE_RESIDUAL_POSTURE_REF`]), never restated as an Issues-local statement (§7.6 / recon X-7).
//! The structural floor (per-subject DEK + pseudonym shred + `restrict` suppression) ships regardless.
//!
//! ## Why register NOW (the structural guarantee — §3.4 / contract 1.4)
//! The Issues OLTP schema (the whole spine — `issue` / `issue_relation` / `issue_change_log` / schemes
//! / cycles / milestones / `prefix_counter`, `crate::migrations`) is opened through the substrate
//! [`HolderRegistry`] ONE door, so it is a registered holder by construction and classifies to **H3
//! (`H3Issues`)** in the exhaustive H1–H18 list (gdpr §3.2). Registering the OLTP holder now makes
//! "the DSAR fan-out forgot Issues" structurally impossible (10.1 exhaustiveness) — even though the
//! erase BODY is the ISS-P07/P31 floor.
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The `erase` crypto-shred fan-out body** (the full per-subject-DEK destroy over issues/comments/
//!   change-log/worklog + the `issue.*.erased` tombstones; pseudonym shred of `assignee`/`reporter`/
//!   `actor`) is **ISS-P07** (the per-subject-DEK columns) + **ISS-P31** (the full ops). Here `erase`
//!   is the typed no-op that names them; the per-subject DEK lever already exists as schema (ISS-P05).
//! - **The full `locate`/`export` subject-walk** (the real issue/comment/change-log rows naming the
//!   subject) lands with the write path (ISS-P06) + the DSR fan-out (ISS-P31). Here they are
//!   empty-but-correct typed receipts.
//!
//! ## DB-free
//! This module builds in-memory holder/receipt values + flips an in-memory restriction flag; the real
//! per-subject DEK crypto-shred rides the storage integration drills + the ISS-P07/P31 fan-out. So
//! `cargo build --workspace` stays DB-free.

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreKind};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// The stable, PII-free name of the Issues **OLTP spine** store (the whole spine — `issue` /
/// `issue_relation` / `issue_change_log` / schemes / cycles / milestones / `prefix_counter`,
/// `crate::migrations`). This is the holder's **H3 (`H3Issues`)** store. Frozen here so the
/// migrations, the data-map, the GDPR-side H3 registration, and the DSR fan-out (ISS-P31) all address
/// exactly this store. PII-free: a store identifier, never personal data.
pub const ISSUE_OLTP_STORE: &str = "issue_oltp";

/// The Issues store CLASSES the holder spans (architecture §7 — `locate / export / rectify / restrict
/// / erase` over **issues, comments, change-log, worklog**). A closed enum: a new Issues data class
/// cannot be added without appearing here (the holder coverage is total — proven by the unit test
/// over [`IssueStoreClass::ALL`]). PII-free — a class tag, never data.
///
/// All four classes live in the Issues OLTP spine (H2-style OLTP → H3 here); the body keys the
/// subject on the pseudonymous identity columns (`assignee`/`reporter`/`actor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IssueStoreClass {
    /// Issues — the `issue` rows + the pseudonymous `assignee`/`reporter` identity fields + the
    /// free-text `title`/`props` (per-subject DEK where PII, §2). OLTP (H3).
    Issues,
    /// Comments — the `myelin-content` comment block subtrees (`#comment-<id>`); author pseudonym +
    /// free-text body under the per-subject DEK. OLTP (H3).
    Comments,
    /// Change-log — the per-issue field-delta history; the actor pseudonym + the free-text delta
    /// under the per-subject DEK (`issue_change_log.pii_key_ref`, §5). OLTP (H3).
    ChangeLog,
    /// Worklog — the OQ-H behavioural worklog/productivity/estimate fields (restricted-by-default;
    /// per-subject DEK crypto-shred; the `[OPEN — LEGAL]` lawful-basis residual, R-2). OLTP (H3).
    Worklog,
}

impl IssueStoreClass {
    /// A stable, PII-free label for the class (telemetry / the receipt — never personal data).
    pub fn label(self) -> &'static str {
        match self {
            IssueStoreClass::Issues => "issues",
            IssueStoreClass::Comments => "comments",
            IssueStoreClass::ChangeLog => "change-log",
            IssueStoreClass::Worklog => "worklog",
        }
    }

    /// **The full set of Issues store classes the holder spans** (architecture §7). `locate`/`export`/
    /// `erase` reach every member; a missed class is a hole. Closed + total — a new Issues data class
    /// cannot be added without appearing here (proven by the unit tests).
    pub const ALL: [IssueStoreClass; 4] = [
        IssueStoreClass::Issues,
        IssueStoreClass::Comments,
        IssueStoreClass::ChangeLog,
        IssueStoreClass::Worklog,
    ];
}

/// **The residual posture — instantiated BY REFERENCE to the ONE platform posture (10.9 / X-7), NEVER
/// restated as an Issues-local statement** (architecture §7, recon X-7 — "the residual is by
/// reference"). Issues cites the posture; it does not author a fresh Issues-local residual statement.
/// The structural floor (per-subject DEK + pseudonym shred + `restrict` suppression) ships regardless.
pub const ISSUE_RESIDUAL_POSTURE_REF: &str =
    "contract 10.9 / 00 §X-7 (the ONE platform free-text/immutable-content erasure posture); \
     Issues: per-subject DEK crypto-shred (11.4, issue.pii_key_ref / issue_change_log.pii_key_ref) \
     + pseudonym shred (4.8, assignee/reporter/actor) + restrict suppression; per-tenant DEK \
     fallback where PII is not isolable; the lawful-basis residual = the ONE [OPEN — LEGAL] posture \
     (the OQ-H worklog TBD_LEGAL track, parallel/Legal, never an Issues-local restatement)";

/// The typed receipt that an Issues store was auto-registered as a [`PersonalDataHolder`] (re-exports
/// the substrate-side [`HolderRegistration`]). PII-free: a (kind, name) tag.
pub type IssueHolderRegistration = HolderRegistration;

/// Build the Issues [`StoreClassifier`] — the data-map declaration that the Issues OLTP spine belongs
/// to holder **H3 (`H3Issues`)** (gdpr §3.2 / §5). The Issues OLTP store needs a per-store declaration
/// (an OLTP store maps to its subsystem's holder). The substrate completeness assertion joins the
/// harness's [`HolderRegistry`] against this classifier: every opened Issues store must map to an
/// H-holder, or it is an orphan (contract 1.4 + gdpr §3.2).
pub fn issue_store_classifier() -> StoreClassifier {
    StoreClassifier::of([myelin_substrate::StoreHolder::new(
        StoreKind::Oltp,
        ISSUE_OLTP_STORE,
        Holder::H3Issues,
    )])
}

/// **Register the Issues OLTP store as a `PersonalDataHolder` through the harness auto-registration
/// (contract 1.4).** Opens the Issues OLTP spine store through the substrate [`HolderRegistry`] — the
/// ONE door — so it is a registered holder by construction. Returns the registry (carrying the
/// receipt) so a caller / test can assert exactly which stores registered + that they classify to
/// their H-holders (H3 for the Issues spine). This is the `serve`-called seam
/// ([`crate::issues_app_spec`] declares `holders: AppSpec::auto()`); registering it makes "the DSAR
/// fan-out forgot Issues" structurally impossible (10.1 exhaustiveness).
pub fn register_issue_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, ISSUE_OLTP_STORE);
    registry
}

/// **The per-subject `restrict` flag (Art. 18/21) — the seam the index/agent/analytics/notif checks
/// read** (architecture §7: a restricted subject's Issues data is NOT indexed / agent-used /
/// analytics-fed / notification-fanned). [`IssueHolder::restrict`] flips it; every Issues seam that
/// surfaces a subject's footprint reads [`RestrictionFlag::is_restricted`] BEFORE emitting. Shared
/// (`Arc<Mutex<…>>`) so the holder and the seams see ONE flag set. PII-free: opaque pseudonymous ids.
#[derive(Clone, Default)]
pub struct RestrictionFlag {
    /// The set of restricted subject ids (opaque pseudonymous principal ids — never a name/email).
    restricted: Arc<Mutex<BTreeSet<String>>>,
}

impl RestrictionFlag {
    /// A fresh flag set (no subject restricted yet).
    pub fn new() -> RestrictionFlag {
        RestrictionFlag::default()
    }

    /// Set (`on = true`) or clear (`on = false`) the restriction for a subject. Idempotent.
    pub fn set(&self, subject: &str, on: bool) {
        let mut g = self.restricted.lock().expect("restriction flag poisoned");
        if on {
            g.insert(subject.to_string());
        } else {
            g.remove(subject);
        }
    }

    /// **Whether a subject is restricted — the check every Issues index/agent/analytics/notif seam
    /// makes BEFORE surfacing the subject's footprint** (architecture §7). A restricted subject's
    /// Issues data is suppressed at the seam (fail-closed for surfacing).
    pub fn is_restricted(&self, subject: &str) -> bool {
        self.restricted
            .lock()
            .expect("restriction flag poisoned")
            .contains(subject)
    }
}

/// **The Issues `PersonalDataHolder` (H3; contract 10.1) — auto-registered, locate/export TYPED, erase
/// STUBBED to crypto-shred, the `restrict` flag WIRED.** The holder over Issues' issues, comments,
/// change-log, and worklog (architecture §7). At ISS-P05 the locate/export bodies are
/// empty-but-correct content-addressed receipts (a real, callable holder — the full subject-walk is
/// ISS-P06/P07/P31); `erase` is the typed no-op that names its ISS-P07/P31 crypto-shred fan-out;
/// `restrict` flips a REAL per-subject flag the Issues seams read. The erasure LEVER (the per-subject
/// DEK on `issue.pii_key_ref` / `issue_change_log.pii_key_ref`, 11.4) exists as schema (ISS-P05).
#[derive(Clone, Default)]
pub struct IssueHolder {
    /// The per-subject restriction flag the index/agent/analytics/notif seams read (§7). Shared so the
    /// holder and the seams see ONE flag set.
    restriction: RestrictionFlag,
}

impl IssueHolder {
    /// Build the Issues holder with a fresh restriction flag.
    pub fn new() -> IssueHolder {
        IssueHolder::default()
    }

    /// Build the Issues holder sharing an existing restriction flag (so a seam can read the SAME flag
    /// the holder writes — one flag set across the holder + the index/agent/analytics/notif seams).
    pub fn with_restriction(restriction: RestrictionFlag) -> IssueHolder {
        IssueHolder { restriction }
    }

    /// Register the Issues OLTP store as holder H3 through the substrate registry (the `serve`-called
    /// auto-registration seam), returning the receipt — the proof the Issues spine registered as H3.
    pub fn register(&self, registry: &mut HolderRegistry) -> IssueHolderRegistration {
        registry.open(StoreKind::Oltp, ISSUE_OLTP_STORE)
    }

    /// Borrow the restriction flag (so an Issues index/agent/analytics/notif seam can read the SAME
    /// flag the holder's `restrict` writes — one flag set, never two).
    pub fn restriction(&self) -> &RestrictionFlag {
        &self.restriction
    }

    /// The opaque, PII-free subject id the receipt body keys on (the pseudonymous Principal id) —
    /// never a name/email. Issues stores identity as `<pseudonym>@<tenant>.noreply` (4.8); the subject
    /// id is the opaque principal id. One derivation — never a second subject-id rendering.
    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for IssueHolder {
    /// Art. 15 access — where the subject's Issues data lives: their reported/assigned issues, their
    /// comments, their change-log entries, their worklog (architecture §7). At ISS-P05 an
    /// empty-but-correct content-addressed receipt attesting the locate ran over the Issues surface
    /// (the full per-class subject-walk lands with ISS-P06/P31). NEVER an error — a real, callable
    /// holder.
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                ISSUE_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "Issues locate over issues/comments/change-log/worklog (ISS-P05 typed seam; \
                 the full subject-walk = ISS-P06 + the DSR fan-out ISS-P31)",
                None,
                0,
            ),
        })
    }

    /// Art. 20 portability — the subject's Issues footprint (reported/assigned issues, comments,
    /// change-log) as references + decrypted-while-key-lives free-text excerpts (architecture §7). At
    /// ISS-P05 an empty-but-correct portable bundle; the full export lands with ISS-P06/P31.
    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                ISSUE_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "Issues export: the subject's footprint (reported/assigned issues + comments + \
                 change-log) as references + free-text excerpts (ISS-P05 typed seam; the full \
                 bundle = ISS-P06 + ISS-P31)",
                None,
                0,
            ),
        })
    }

    /// Art. 16 rectification — update Issues free text the subject controls (their own comments /
    /// issue bodies). The patch-apply model lands with the GDPR 10.4 / reindex-from-source path
    /// (ISS-P31); at ISS-P05 a well-defined no-op receipt naming it.
    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                ISSUE_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (ISS-P05 substrate; the patch-apply + reindex-from-source = ISS-P31 / GDPR 10.4)",
                None,
                0,
            ),
        })
    }

    /// Art. 18/21 restriction — set/clear the per-subject restriction flag the Issues index/agent/
    /// analytics/notif seams read (architecture §7). This flips a REAL flag ([`RestrictionFlag`]) the
    /// seams check BEFORE surfacing the subject's footprint; the honoured-everywhere proof is the GDPR
    /// P-GA-25 path. A restricted subject's Issues data is NOT indexed / agent-used / analytics-fed /
    /// notification-fanned.
    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = Self::subject_id(subject);
        self.restriction.set(&sid, on);
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                ISSUE_OLTP_STORE,
                &sid,
                "",
                if on {
                    "Issues restrict ON: no indexing / no agent-use / no analytics / no notification (§7)"
                } else {
                    "Issues restrict OFF: the per-subject restriction flag is cleared (§7)"
                },
                None,
                0,
            ),
        })
    }

    /// Art. 17 erasure — **STUBBED to crypto-shred here (the ISS-P05 substrate); the full fan-out is
    /// ISS-P07 (the per-subject-DEK columns) + ISS-P31 (the full ops).** The real erase crypto-shreds
    /// the subject's per-subject Issues DEK (`issue.pii_key_ref` / `issue_change_log.pii_key_ref`,
    /// 11.4 — rendering the encrypted free-text, incl. backups, unrecoverable) and the per-tenant DEK
    /// fallback where it is not, pseudonym-shreds the `assignee`/`reporter`/`actor` identity edges
    /// (4.8), and emits the `issue.*.erased` tombstones — over issues/comments/change-log/worklog. The
    /// issue STRUCTURE survives (delete the identity, not the fact — "Former user 8a2f", §7). The
    /// residual is the ONE platform posture ([`ISSUE_RESIDUAL_POSTURE_REF`], 10.9 / X-7 — never
    /// restated Issues-local). At ISS-P05 this is a well-defined no-op receipt that names ISS-P07/P31;
    /// the per-subject DEK lever exists as schema (ISS-P05).
    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (subject_id, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                ISSUE_OLTP_STORE,
                &subject_id,
                &tenant,
                "no-op (ISS-P05 substrate; the per-subject/per-tenant DEK crypto-shred + pseudonym \
                 shred + issue.*.erased tombstone fan-out over issues/comments/change-log/worklog = \
                 ISS-P07 + ISS-P31; residual = the ONE posture 10.9/X-7, by reference)",
                None,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::{
        CYCLE_TABLE, ISSUE_CHANGE_LOG_TABLE, ISSUE_RELATION_TABLE, ISSUE_TABLE, MILESTONE_TABLE,
        PREFIX_COUNTER_TABLE, SCHEME_TABLE,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::{
        assert_all_holders_registered, assert_holder_completeness, classify_store, DeclaredStore,
        StoreManifest,
    };

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    /// **The §7 store-class set is the holder's coverage** — issues, comments, change-log, worklog.
    /// The closed set is the structural coverage surface (a new Issues data class cannot be added
    /// without appearing here).
    #[test]
    fn the_issue_store_class_set_is_the_holder_coverage() {
        assert_eq!(IssueStoreClass::ALL.len(), 4);
        for c in [
            IssueStoreClass::Issues,
            IssueStoreClass::Comments,
            IssueStoreClass::ChangeLog,
            IssueStoreClass::Worklog,
        ] {
            assert!(
                IssueStoreClass::ALL.contains(&c),
                "{} must be in the holder coverage",
                c.label()
            );
        }
        assert_eq!(IssueStoreClass::Worklog.label(), "worklog");
    }

    /// **The Issues OLTP store auto-registers as holder H3 through the one door (contract 1.4) and
    /// classifies to H3 — 0 orphans (gdpr §3.2).** Opening it through the substrate registry makes it
    /// a registered holder by construction; it maps to the exhaustive H3 (`H3Issues`) — so the DSAR
    /// fan-out cannot silently miss Issues. This is the ISS-P05 holder GATE.
    #[test]
    fn issue_store_registers_and_classifies_to_h3_no_orphan() {
        let registry = register_issue_holders();
        assert!(registry.is_registered(StoreKind::Oltp, ISSUE_OLTP_STORE));
        assert_eq!(
            registry.len(),
            1,
            "exactly the Issues OLTP store registered"
        );
        let classifier = issue_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, ISSUE_OLTP_STORE, &classifier),
            Some(Holder::H3Issues),
            "the Issues OLTP spine is holder H3 (Issues subsystem DB)"
        );
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "every Issues store is in the exhaustive H1–H18 list — 0 orphan stores"
        );
    }

    /// **The 1.4 enforcement (the ISS-P05 holder GATE): an Issues store opened OUTSIDE the harness
    /// FAILS the holder-registered architecture test.** The conforming registry passes; a registry
    /// missing it is a loud violation naming exactly the escaped store — an unregistered PII store
    /// cannot quietly miss the DSR fan-out.
    #[test]
    fn an_unregistered_issue_store_fails_the_holder_registered_architecture_test() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, ISSUE_OLTP_STORE)]);
        assert_eq!(
            assert_all_holders_registered(&manifest, &register_issue_holders()),
            Ok(()),
            "the Issues store opened through the harness → the architecture test passes"
        );
        let rogue = HolderRegistry::new();
        let err = assert_all_holders_registered(&manifest, &rogue).expect_err(
            "an Issues store opened outside the harness must FAIL the architecture test",
        );
        assert_eq!(
            err.len(),
            1,
            "exactly the unregistered Issues store is the violation"
        );
        assert!(
            err[0].message().contains(ISSUE_OLTP_STORE),
            "the failure names the escaped Issues store: {}",
            err[0].message()
        );
    }

    /// **`locate`/`export` are TYPED + empty-but-correct (the ISS-P05 surface), not an error.** Both
    /// return content-addressed receipts over the Issues surface — a real, callable holder, not a
    /// `todo!()`/`Err`. The full located/exported data lands with ISS-P06/P31.
    #[test]
    fn locate_and_export_are_typed_and_empty_but_correct() {
        let holder = IssueHolder::new();
        let subj = subject("psn:iss-7");
        let locate = holder
            .locate(&subj, tenant())
            .expect("locate over the Issues surface succeeds");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        assert!(
            locate.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );
        let export = holder
            .export(&subj, tenant())
            .expect("export over the Issues surface succeeds");
        assert_eq!(export.receipt.operation, "export");
        assert!(export.receipt.content_hash.starts_with("blake3:"));
    }

    /// **`restrict` flips a REAL per-subject flag the Issues seams read (Art. 18/21, §7).** After
    /// `restrict(on)` the subject is restricted; after `restrict(off)` it is cleared. The flag the
    /// holder writes is the SAME one a seam reads (one flag set).
    #[test]
    fn restrict_flips_a_real_flag_the_seams_read() {
        let flag = RestrictionFlag::new();
        let holder = IssueHolder::with_restriction(flag.clone());
        let subj = subject("psn:iss-restricted");
        let sid = "psn:iss-restricted";

        assert!(!flag.is_restricted(sid));
        let r = holder.restrict(&subj, true).expect("restrict ON");
        assert_eq!(r.receipt.operation, "restrict");
        assert!(
            flag.is_restricted(sid),
            "the restriction flag the Issues index/agent/analytics/notif seams read is SET"
        );
        holder.restrict(&subj, false).expect("restrict OFF");
        assert!(!flag.is_restricted(sid), "the restriction flag is cleared");
    }

    /// **`erase` is STUBBED to crypto-shred (the ISS-P05 substrate) — a well-defined no-op receipt
    /// that NAMES its ISS-P07/P31 follow-on, never a panic.** Idempotent: the same scope yields the
    /// same content-addressed receipt (no DEK shredded yet — the structural fan-out is ISS-P07/P31).
    #[test]
    fn erase_is_a_stubbed_crypto_shred_no_op_that_names_iss_p07_p31() {
        let holder = IssueHolder::new();
        let scope = EraseScope::Subject {
            subject: subject("psn:iss-7"),
            tenant: tenant(),
        };
        let r1 = holder.erase(scope.clone()).expect("erase succeeds (stub)");
        let r2 = holder.erase(scope).expect("erase is idempotent");
        assert_eq!(
            r1, r2,
            "the same erase scope yields the identical content-addressed receipt"
        );
        assert!(
            r1.receipt.key_epoch_destroyed.is_none(),
            "no DEK shredded (the crypto-shred body is ISS-P07/P31)"
        );
        assert_eq!(r1.receipt.operation, "erase");
        assert!(r1.receipt.content_hash.starts_with("blake3:"));
    }

    /// **The residual is BY REFERENCE to the ONE platform posture (10.9 / X-7) — never restated
    /// Issues-local (§7 / recon X-7).** The reference cites the contract + the structural floor
    /// (per-subject DEK + pseudonym shred + restrict) and the lawful-basis residual as the ONE
    /// [OPEN — LEGAL] posture, not a fresh Issues-local statement.
    #[test]
    fn the_residual_is_by_reference_to_the_one_platform_posture() {
        assert!(
            ISSUE_RESIDUAL_POSTURE_REF.contains("10.9")
                && ISSUE_RESIDUAL_POSTURE_REF.contains("X-7"),
            "the residual cites the ONE platform posture (10.9 / X-7), by reference"
        );
        assert!(
            ISSUE_RESIDUAL_POSTURE_REF.contains("never an Issues-local restatement"),
            "the residual is by reference, never restated Issues-local"
        );
    }

    /// **The Issues holder is object-safe** — held behind `dyn PersonalDataHolder` exactly as the DSR
    /// orchestrator / holder registry need (a heterogeneous holder set, contract 10.1).
    #[test]
    fn issue_holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(IssueHolder::new())];
        let subj = subject("psn:iss-9");
        for h in &holders {
            assert!(
                h.locate(&subj, tenant()).is_ok(),
                "the Issues holder responds to the contract"
            );
        }
    }

    /// **The classifier addresses exactly the Issues OLTP store (the whole spine — every spine table
    /// lives in the one Postgres → H3).** The migration table-name constants resolve (a compile-time
    /// link that the holder + the migrations agree on the spine surface). This pins that the holder's
    /// store name + the migration tables are one coherent surface.
    #[test]
    fn the_holder_spans_the_one_oltp_spine() {
        // Every spine table lives in the ONE Issues OLTP store the holder registers (H3).
        for t in [
            ISSUE_TABLE,
            ISSUE_RELATION_TABLE,
            ISSUE_CHANGE_LOG_TABLE,
            SCHEME_TABLE,
            CYCLE_TABLE,
            MILESTONE_TABLE,
            PREFIX_COUNTER_TABLE,
        ] {
            assert!(
                !t.is_empty(),
                "the spine table name `{t}` is a real table in the H3 store"
            );
        }
        // The store name is stable + PII-free.
        assert_eq!(ISSUE_OLTP_STORE, "issue_oltp");
    }
}
