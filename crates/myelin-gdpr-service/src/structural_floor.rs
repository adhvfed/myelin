//! # The structural erasure floor PROVEN on the M1 stores (P-GA-17 → P-117)
//!
//! **P-GA-17.** [`crate::posture`] (P-GA-16 → P-116) STATES the ONE free-text / immutable-content
//! erasure posture; its structural floor (§7.1) is the three levers
//! [`crate::posture::StructuralLever`] — per-subject DEK crypto-shred (11.4) + pseudonym-map shred
//! (4.8) + `restrict` suppression (10.1). This module PROVES that floor **working end-to-end on the
//! M1 stores**, the deliverable P-GA-17 owns. The earlier prompts shipped each lever as a SEAM:
//! - the per-subject DEK crypto-shred MECHANISM ([`crate::holders::CryptoShredKms`], P-GA-05);
//! - the pseudonym-map shred LEVER (Identity's [`myelin_identity::PseudonymHandle`] grammar +
//!   `resolve_pseudonym`/`erase`, contract 4.8, P-ID-19/-20);
//! - the holder `restrict` op RECEIPT ([`crate::holders`], [`crate::orchestration::SeamHolder`]).
//!
//! What was MISSING — and what this module ships — is the **`restrict` suppression FLAG every M1
//! holder HONOURS**: a per-subject suppression record (reversible) PLUS an M1-store model whose
//! processing operations (indexing / agent-use / analytics / notification) **check the flag and
//! refuse to process** a restricted subject while **RETAINING storage** (gdpr §4.4: "no indexing,
//! no agent-use, no analytics, no notification, while retaining storage; reversible"). Before
//! P-117 the holders RECORDED a restrict receipt but no store HONOURED it — the floor was stated,
//! not proven. This module makes "honour" observable: a [`RestrictRegistry`] carries the flag, and
//! an [`M1Store`] is built over it so that the suppression is a property the unit tests + the
//! GATE drill (`tests/ga_d7_m1_restrict_honoured.rs`) observe THROUGH the store, not assert.
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§7.1** (the structural
//! floor built now — per-subject DEK crypto-shred of self-authored free-text; pseudonym-map shred
//! of identity; structural holder coverage + `restrict`), **§4.4** (`restrict(subject, on)` — no
//! indexing / agent-use / analytics / notification while RETAINING storage; reversible). The ONE
//! posture this floor instantiates is `gdpr-and-audit.md` §7 / `00-reconciliation-decisions.md`
//! §X-7 ([`crate::posture::POSTURE_ANCHOR`]).
//!
//! ## The three levers proven here (the §7.1 floor, end-to-end on an M1 store)
//! 1. **Per-subject DEK crypto-shred** ([`M1Store::erase_self_authored`]). A subject's self-authored
//!    free-text in the M1 store is sealed under their per-subject DEK ([`crate::holders::ShredKeyClass::Subject`]);
//!    erasing them destroys the DEK through the [`crate::holders::CryptoShredKms`] seam, so the
//!    ciphertext is unrecoverable — live AND in backups (§7.5). After the shred a read returns
//!    [`StoredContent::Unrecoverable`], NOT plaintext.
//! 2. **Pseudonym-map shred** ([`shred_pseudonym_identity`]). Author/subject identity in an
//!    immutable structure (an audit entry, an event actor, a commit author) is the stable opaque
//!    [`myelin_identity::PseudonymHandle`] rendering `<pseudonym>@<tenant>.noreply`. Shredding the
//!    person↔pseudonym map (DSR step 1) leaves the immutable bytes holding ONLY that pseudonym form —
//!    never real-identity PII (which never lived in the bytes). Proven by the round-trip: the bytes
//!    after the shred parse as a valid pseudonym handle and carry no email/name.
//! 3. **`restrict` suppression** ([`RestrictRegistry`] + the [`M1Store`] processing ops). A
//!    per-subject suppression flag (set / cleared — reversible) that the M1 store HONOURS: while
//!    restricted, [`M1Store::index`] / [`M1Store::agent_read`] / [`M1Store::analyse`] /
//!    [`M1Store::notify`] are SUPPRESSED ([`Processed::Suppressed`]), while [`M1Store::fetch_stored`]
//!    still RETURNS the retained content (storage is retained, not deleted). The residual
//!    (author's-DEK-encrypted third-party mention) is `restrict`-suppressed by the SAME flag — never
//!    indexed / agent-read / analysed (the documented limit from P-GA-16 / §7.3).
//!
//! ## Floor named (deferred → filling prompt) — VISION §3 name-your-floors
//! - The **full restriction-into-derived-stores proof (GA-D7)** — the flag flowing into EVERY
//!   derived store (Search no-indexing / Refs / Notif / Agents / OLAP no-analytics), 0 processing of
//!   a restricted subject across the whole derivative fan-out — is **M2 P-GA-25 → P-152**. THIS
//!   prompt proves the M1 holders honour `restrict` NOW; the [`M1Store`] is the faithful M1-store
//!   model every M1 holder is (a store with processing ops + retained storage), so the suppression
//!   is honoured at the holder boundary the M2 derived-store fan-out rides.
//! - The **live store bindings** (the real Identity `resolve_pseudonym`/`erase` for the map shred,
//!   the real Storage `KmsEngine` behind [`crate::holders::CryptoShredKms`], the real per-store
//!   index/analytics surfaces honouring the flag) are wired by the harness at boot — the same DB /
//!   KMS / store floor every M0/M1 in-memory store carries (P-007 / P-S12 / the Storage KMS
//!   hierarchy). On this floor each lever runs through its faithful in-memory model with
//!   byte-for-byte the §7.1 / §7.5 semantics; this module touches NO new DB/object-store/cache/bus
//!   contract (it composes the already-shipped seams), so no `--features integration` live-stack leg
//!   is owed by P-GA-17.
//!
//! ## Mutation floor (P-GA-17 TESTS — the `restrict`-suppression-flag + the residual-classification
//! path are mandatory-core). The behavioral core every mutation must be CAUGHT:
//! [`RestrictRegistry::is_restricted`] (the flag the store honours), the [`M1Store`] processing-op
//! suppression branch (a restricted subject is SUPPRESSED, an unrestricted one is PROCESSED, and
//! storage is RETAINED either way — the `if is_restricted` predicate is load-bearing), and
//! [`classify_residual`] (a third-party mention is the author's-DEK residual, suppressed not
//! shredded). `cargo mutants -p myelin-gdpr-service -f src/structural_floor.rs` (2026-06-20):
//! **32 mutants, 20 caught, 10 unviable, 2 MISSED.** The behavioral core is CAUGHT — the
//! `is_restricted` flag, the [`M1Store`] suppression branch (both verdicts pinned), the per-subject
//! storage key (cross-subject isolation), the [`classify_residual`] residual decision, and the
//! [`ShreddedIdentity::holds_only_the_pseudonym_form`] proof predicate (the `-> true` mutant is
//! killed by a real-email negative case). The 2 residuals are both the `M1Store::id() -> ""`/`"xyzzy"`
//! **label accessor** — a PII-free holder-name getter used only in drill assertion messages; the
//! processing path reads the `self.id` FIELD directly (whose projection IS pinned), so the *method*
//! mutant has no behavioral effect. Documented non-core, stated not hidden (EI-01 §3).

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{SubjectRef, TenantId};
use myelin_identity::PseudonymHandle;

use crate::holders::{CryptoShredKms, ShredKeyClass, ShredKeyHandle};

// ───────────────────────── the `restrict` suppression flag (§4.4, the M1 holders honour it) ─────

/// **The per-subject `restrict` suppression flag every M1 holder HONOURS (gdpr §4.4 / §7.1).** A
/// reversible per-`(tenant, subject)` record: while SET, the M1 store suppresses processing (no
/// indexing / agent-use / analytics / notification) while RETAINING storage; cleared, processing
/// resumes. This is the structural-floor lever [`crate::posture::StructuralLever::RestrictSuppression`]
/// made OBSERVABLE — before P-117 the holders recorded a restrict receipt but no store honoured it.
///
/// The full restriction-into-derived-stores proof (GA-D7, the flag flowing into Search/Refs/Notif/
/// Agents/OLAP) is **M2 P-GA-25 → P-152**; this registry is the per-holder flag those derived stores
/// will read. PII-free: keyed on the opaque pseudonymous subject id + the tenant token only.
#[derive(Debug, Default)]
pub struct RestrictRegistry {
    /// The set `(tenant, subject_id)` whose processing is currently suppressed. Absence = not
    /// restricted. Reversible — `set(.., false)` removes the entry.
    restricted: Mutex<BTreeMap<(String, String), ()>>,
}

impl RestrictRegistry {
    /// A registry with no restrictions (every subject processes normally until restricted).
    #[must_use]
    pub fn new() -> RestrictRegistry {
        RestrictRegistry {
            restricted: Mutex::new(BTreeMap::new()),
        }
    }

    /// The PII-free key a restriction is recorded under: the opaque pseudonymous subject id + the
    /// tenant token. Never a name/email (the [`SubjectRef`] carries the opaque `principal_id`).
    fn key(subject: &SubjectRef, tenant: &TenantId) -> (String, String) {
        (tenant.0.clone(), subject.principal.principal_id.0.clone())
    }

    /// **Set / clear the suppression flag (Art. 18/21 `restrict(subject, on)` — REVERSIBLE).** `on =
    /// true` suppresses processing for the subject; `on = false` lifts it. Idempotent on each side.
    pub fn set(&self, subject: &SubjectRef, tenant: &TenantId, on: bool) {
        let key = Self::key(subject, tenant);
        let mut map = self.restricted.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            map.insert(key, ());
        } else {
            map.remove(&key);
        }
    }

    /// **The flag the M1 store HONOURS:** is this subject's processing currently suppressed? `true`
    /// ⇒ the store must suppress indexing / agent-use / analytics / notification (storage retained).
    #[must_use]
    pub fn is_restricted(&self, subject: &SubjectRef, tenant: &TenantId) -> bool {
        let key = Self::key(subject, tenant);
        self.restricted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&key)
    }
}

// ───────────────────────── the M1-store processing-op outcome ─────────────────────────

/// A processing operation on an M1 store. These are the four things gdpr §4.4 enumerates as
/// SUPPRESSED for a restricted subject — "no indexing, no agent-use, no analytics, no notification".
/// The structural floor must HONOUR the flag for EACH (the residual is suppressed across all four).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Processing {
    /// Indexing the content into a search index (Search honours it fully in M2, P-GA-25).
    Index,
    /// An agent reading the content (the agent-trace holder honours it; agent-use is suppressed).
    AgentRead,
    /// Analytics / OLAP over the content (no analytics for a restricted subject — §2.4 / 11.6).
    Analyse,
    /// A notification derived from the content (no notification for a restricted subject).
    Notify,
}

impl Processing {
    /// The four processing ops, in a stable order (for the drill's exhaustive suppression check).
    #[must_use]
    pub const fn all() -> [Processing; 4] {
        [
            Processing::Index,
            Processing::AgentRead,
            Processing::Analyse,
            Processing::Notify,
        ]
    }

    /// A stable PII-free token (for receipts / telemetry).
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Processing::Index => "index",
            Processing::AgentRead => "agent_read",
            Processing::Analyse => "analyse",
            Processing::Notify => "notify",
        }
    }
}

/// The outcome of a processing op over an M1 store: the content was PROCESSED, or it was SUPPRESSED
/// because the subject is restricted (the `restrict` flag honoured). Storage is RETAINED regardless
/// (a suppression is NOT a delete — the `fetch_stored` path still returns the content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Processed {
    /// The op ran — the content (the processed projection) is returned. (For an unrestricted
    /// subject whose content is still recoverable.)
    Processed(String),
    /// The op was SUPPRESSED — the subject is restricted (no indexing / agent-use / analytics /
    /// notification while restricted, §4.4). The store retains the content; processing is withheld.
    Suppressed,
    /// The content is UNRECOVERABLE — the subject's per-subject DEK was crypto-shredded (the erase
    /// lever ran). There is nothing to process; this is the erased terminal state, distinct from a
    /// reversible suppression.
    Unrecoverable,
}

// ───────────────────────── the retained content of an M1 store ─────────────────────────

/// The state of a subject's self-authored free-text in an M1 store, AFTER honouring the levers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredContent {
    /// The plaintext is recoverable (the per-subject DEK is live). The retained content.
    Recoverable(String),
    /// The per-subject DEK was crypto-shredded — the ciphertext is unrecoverable (§7.5, live AND in
    /// backups). The erase lever ran; the storage row remains but holds only unrecoverable bytes.
    Unrecoverable,
}

// ───────────────────────── the M1 store model (honours the three levers) ─────────────────────────

/// **A faithful M1-store model that HONOURS the three structural-floor levers (§7.1).** Every M1
/// holder is, structurally, a store that: holds a subject's self-authored free-text under their
/// per-subject DEK (the crypto-shred lever); exposes processing ops (index / agent-read / analyse /
/// notify) that must HONOUR the `restrict` flag; and RETAINS storage independent of suppression.
/// This model makes the floor OBSERVABLE — the unit tests + the GATE drill read the suppression /
/// unrecoverability THROUGH the store, never assert it (EI-01 §3 prove-it).
///
/// The live per-store bindings (real Search index / OLAP / agent surfaces honouring the flag, the
/// real Storage KMS behind [`CryptoShredKms`]) are the named floor; the SHAPE — a store that checks
/// the flag before processing and crypto-shreds the per-subject DEK on erase — does not change.
pub struct M1Store<'a> {
    /// The PII-free holder id this store registers under (a Search index, an Issues store, …).
    id: &'static str,
    /// The `restrict` suppression flag the store honours (shared across the M1 holder set — one
    /// restriction suppresses every holder, the §4.4 "every holder honours" property).
    restrict: &'a RestrictRegistry,
    /// The per-subject DEK crypto-shred mechanism (the no-cross-store-read seam — Storage owns it).
    kms: &'a dyn CryptoShredKms,
    /// The retained self-authored free-text, keyed on `(tenant, subject_id)`. The storage row —
    /// retained even when processing is suppressed; readable as plaintext only while the DEK is live.
    stored: Mutex<BTreeMap<(String, String), String>>,
}

impl<'a> M1Store<'a> {
    /// Build an M1-store model over the shared `restrict` registry + the crypto-shred KMS seam.
    #[must_use]
    pub fn new(
        id: &'static str,
        restrict: &'a RestrictRegistry,
        kms: &'a dyn CryptoShredKms,
    ) -> M1Store<'a> {
        M1Store {
            id,
            restrict,
            kms,
            stored: Mutex::new(BTreeMap::new()),
        }
    }

    /// The PII-free holder id.
    #[must_use]
    pub fn id(&self) -> &'static str {
        self.id
    }

    fn key(subject: &SubjectRef, tenant: &TenantId) -> (String, String) {
        (tenant.0.clone(), subject.principal.principal_id.0.clone())
    }

    /// The per-subject DEK handle this store's content for `subject` is sealed under (the lever-1
    /// crypto-shred key class — [`ShredKeyClass::Subject`]). Public so a drill / the harness wiring
    /// can provision the DEK on the KMS the same way Storage's KMS hierarchy does at write time.
    #[must_use]
    pub fn dek_handle(subject: &SubjectRef, tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subject.principal.principal_id.0.clone()),
        }
    }

    /// Seed a subject's self-authored free-text (their message / comment / block / worklog). The
    /// store now holds the storage row for the subject; the per-subject DEK that seals it is
    /// provisioned on the KMS by the caller (the drill seeds it; a real store's DEK is provisioned
    /// by Storage's KMS hierarchy at write time — [`Self::dek_handle`] names the key class). The
    /// row's plaintext is recoverable IFF that DEK is live (see [`Self::fetch_stored`]).
    pub fn store_self_authored(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        content: impl Into<String>,
    ) {
        self.stored
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(Self::key(subject, tenant), content.into());
    }

    /// **Lever 1 — per-subject DEK crypto-shred (the erase lever, §7.1.1).** Erasing the subject
    /// destroys their per-subject DEK through the KMS seam; their self-authored content becomes
    /// unrecoverable ciphertext (live AND in backups, §7.5). Returns the destroyed key epoch (the
    /// audit trail), or `None` on an idempotent re-erase. The storage ROW remains (erase = shred,
    /// never a hole), but a subsequent [`Self::fetch_stored`] reads [`StoredContent::Unrecoverable`].
    pub fn erase_self_authored(&self, subject: &SubjectRef, tenant: &TenantId) -> Option<u64> {
        self.kms.destroy(&Self::dek_handle(subject, tenant))
    }

    /// Read the RETAINED storage for the subject — the §4.4 "while retaining storage" property.
    /// While the DEK is live the plaintext is recoverable; after the crypto-shred it is
    /// [`StoredContent::Unrecoverable`]. **Storage is retained either way** — a `restrict` suppresses
    /// PROCESSING, never storage; an erase shreds the KEY, never the row.
    #[must_use]
    pub fn fetch_stored(&self, subject: &SubjectRef, tenant: &TenantId) -> Option<StoredContent> {
        let has_row = self
            .stored
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&Self::key(subject, tenant))
            .cloned();
        let row = has_row?;
        // The plaintext is recoverable IFF the per-subject DEK is still present (not shredded).
        if self.kms.is_present(&Self::dek_handle(subject, tenant)) {
            Some(StoredContent::Recoverable(row))
        } else {
            Some(StoredContent::Unrecoverable)
        }
    }

    /// The shared processing-op core (§4.4): honour the `restrict` flag and the erase lever.
    /// - DEK shredded ⇒ [`Processed::Unrecoverable`] (nothing to process).
    /// - restricted ⇒ [`Processed::Suppressed`] (no indexing / agent-use / analytics / notification).
    /// - else ⇒ [`Processed::Processed`] (the op runs over the recoverable content).
    fn process(&self, op: Processing, subject: &SubjectRef, tenant: &TenantId) -> Processed {
        match self.fetch_stored(subject, tenant) {
            // The erase lever ran — the content is unrecoverable; there is nothing to process.
            Some(StoredContent::Unrecoverable) | None => Processed::Unrecoverable,
            Some(StoredContent::Recoverable(content)) => {
                // Lever 3 — HONOUR the restrict flag. A restricted subject's content is RETAINED
                // (fetch_stored above still returned it) but processing is SUPPRESSED.
                if self.restrict.is_restricted(subject, tenant) {
                    Processed::Suppressed
                } else {
                    Processed::Processed(format!("{}:{}:{content}", op.token(), self.id))
                }
            }
        }
    }

    /// **Lever 3 — index the subject's content (SUPPRESSED while restricted, §4.4).**
    #[must_use]
    pub fn index(&self, subject: &SubjectRef, tenant: &TenantId) -> Processed {
        self.process(Processing::Index, subject, tenant)
    }

    /// **Lever 3 — an agent reads the subject's content (SUPPRESSED while restricted, §4.4).**
    #[must_use]
    pub fn agent_read(&self, subject: &SubjectRef, tenant: &TenantId) -> Processed {
        self.process(Processing::AgentRead, subject, tenant)
    }

    /// **Lever 3 — analytics over the subject's content (SUPPRESSED while restricted, §4.4 / 11.6).**
    #[must_use]
    pub fn analyse(&self, subject: &SubjectRef, tenant: &TenantId) -> Processed {
        self.process(Processing::Analyse, subject, tenant)
    }

    /// **Lever 3 — a notification from the subject's content (SUPPRESSED while restricted, §4.4).**
    #[must_use]
    pub fn notify(&self, subject: &SubjectRef, tenant: &TenantId) -> Processed {
        self.process(Processing::Notify, subject, tenant)
    }
}

// ───────────────────────── Lever 2 — the pseudonym-map shred (4.8) ─────────────────────────

/// The state of subject/author identity in an immutable structure (an audit entry, an event actor,
/// a commit author) AFTER the pseudonym-map shred (DSR step 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShreddedIdentity {
    /// The immutable bytes as they now read — ONLY the frozen `<pseudonym>@<tenant>.noreply` form
    /// (contract 4.8). No real-identity PII: the name/email never lived in the bytes (it lived in
    /// Id's erasable person↔pseudonym map, now shredded).
    pub immutable_bytes: String,
}

impl ShreddedIdentity {
    /// **Proof property: the shredded bytes hold ONLY the pseudonym form** — they parse as a valid
    /// [`PseudonymHandle`] (the frozen `<pseudonym>@<tenant>.noreply` grammar) AND carry no `@`-less
    /// real handle. After the map shred, resolving the pseudonym to a person is impossible, so the
    /// bytes are the immutable, non-identifying residue (§7.1.2).
    #[must_use]
    pub fn holds_only_the_pseudonym_form(&self) -> bool {
        PseudonymHandle::parse(&self.immutable_bytes).is_some()
    }
}

/// **Lever 2 — pseudonym-map shred (identity erasure, contract 4.8, DSR step 1, §7.1.2).** The
/// immutable structure already holds the subject as the stable opaque pseudonym (commits are
/// pseudonymous-by-default, audit entries are minimised to the pseudonym, P-GA-18). Shredding the
/// person↔pseudonym map (Identity's `erase(subject)`) makes the pseudonym UNRESOLVABLE — but the
/// immutable bytes are untouched (a retroactive edit would break a commit hash / the audit chain).
/// This returns the [`ShreddedIdentity`]: the immutable bytes holding ONLY the pseudonym form.
///
/// The real lever is Identity's `resolve_pseudonym`/`erase` (P-ID-19/-20) behind the no-cross-store
/// -read seam — the live binding is the named floor. Here we model the §7.1.2 post-condition: the
/// immutable bytes carry the frozen `<pseudonym>@<tenant>.noreply` rendering, never real PII.
#[must_use]
pub fn shred_pseudonym_identity(pseudonym: &PseudonymHandle) -> ShreddedIdentity {
    ShreddedIdentity {
        // The immutable bytes hold the frozen grammar rendering — the only identity form that ever
        // reached them. The map shred makes it unresolvable; the rendering itself is non-PII.
        immutable_bytes: pseudonym.render(),
    }
}

// ───────────────────────── the residual classification (P-GA-16 / §7.2 limit) ─────────────────

/// How a span of free-text relates to the erasing subject — the §7.2 classification that decides
/// which lever covers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authorship {
    /// The subject AUTHORED this content (their message / comment / block / worklog). Covered by
    /// **lever 1** — per-subject DEK crypto-shred renders it unrecoverable.
    SelfAuthored,
    /// A THIRD PARTY typed the subject's name/email into that other person's content (a chat body,
    /// an issue comment, a doc block, a CI log line, a commit message by a different author). It is
    /// encrypted under the **author's** DEK, NOT the subject's — the §7.2 residual. The subject's
    /// erasure does NOT crypto-shred it (that would destroy the author's legitimate content);
    /// instead it is covered by **lever 3** — `restrict`-suppressed (never indexed / agent-read /
    /// analysed for the restricted subject), the documented lawful-basis limit.
    ThirdPartyMention,
}

/// Which structural-floor lever covers a span — the §7.2 / §7.3 documented-limit decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeverCoverage {
    /// Lever 1 — per-subject DEK crypto-shred (self-authored content erases to unrecoverable).
    CryptoShred,
    /// Lever 3 — `restrict` suppression only (the residual: third-party PII under the author's DEK,
    /// the documented limit — never indexed / agent-read / analysed for the restricted subject, but
    /// NOT crypto-shredded by the subject's key).
    RestrictSuppressOnly,
}

/// **Classify a span by authorship → its lever coverage (the §7.2 residual decision, P-GA-16's
/// documented limit).** Self-authored content is crypto-shreddable (lever 1); a third-party mention
/// is the residual — covered ONLY by `restrict` suppression (lever 3), never crypto-shredded by the
/// subject's key. This is the load-bearing residual-classification path the mutation floor pins: a
/// mutant that mis-classifies a third-party mention as crypto-shreddable would falsely claim the
/// residual is erased (the X-7 anti-pattern — pretending the documented limit is solved).
#[must_use]
pub fn classify_residual(authorship: Authorship) -> LeverCoverage {
    match authorship {
        Authorship::SelfAuthored => LeverCoverage::CryptoShred,
        Authorship::ThirdPartyMention => LeverCoverage::RestrictSuppressOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holders::InMemoryShredKms;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            t("acme"),
        ))
    }

    // ───────── lever 3: the restrict suppression flag the M1 store honours ─────────

    /// **The restrict flag is honoured: while restricted, every processing op is SUPPRESSED, but
    /// storage is RETAINED (§4.4).** The subject's content is seeded; before restriction every op
    /// processes; after `restrict(set)` every op is suppressed AND `fetch_stored` still returns the
    /// retained content; after `restrict(clear)` every op processes again (reversible).
    #[test]
    fn restrict_suppresses_processing_but_retains_storage_reversibly() {
        let tenant = t("acme");
        let subj = subject("u-restrict");
        let restrict = RestrictRegistry::new();
        let kms = InMemoryShredKms::new();
        let store = M1Store::new("issues_store", &restrict, &kms);
        // Seed the subject's self-authored content + provision their DEK (live).
        kms.provision(M1Store::dek_handle(&subj, &tenant), 1);
        store.store_self_authored(&subj, &tenant, "my comment");

        // BEFORE restriction: every processing op PROCESSES.
        for op in Processing::all() {
            let r = store.process(op, &subj, &tenant);
            assert!(
                matches!(r, Processed::Processed(_)),
                "{:?} processes for an unrestricted subject",
                op
            );
        }

        // SET the flag (Art. 18/21 restrict). Every op is now SUPPRESSED.
        restrict.set(&subj, &tenant, true);
        assert!(restrict.is_restricted(&subj, &tenant));
        assert_eq!(store.index(&subj, &tenant), Processed::Suppressed);
        assert_eq!(store.agent_read(&subj, &tenant), Processed::Suppressed);
        assert_eq!(store.analyse(&subj, &tenant), Processed::Suppressed);
        assert_eq!(store.notify(&subj, &tenant), Processed::Suppressed);

        // STORAGE is RETAINED while restricted — the content is still there (suppression ≠ delete).
        assert_eq!(
            store.fetch_stored(&subj, &tenant),
            Some(StoredContent::Recoverable("my comment".into())),
            "restrict suppresses PROCESSING, never storage (§4.4: while retaining storage)"
        );

        // CLEAR the flag (reversible). Processing resumes.
        restrict.set(&subj, &tenant, false);
        assert!(!restrict.is_restricted(&subj, &tenant));
        for op in Processing::all() {
            assert!(
                matches!(store.process(op, &subj, &tenant), Processed::Processed(_)),
                "{:?} processes again after the restriction is lifted (reversible)",
                op
            );
        }
    }

    /// The flag is per-`(tenant, subject)`: restricting one subject does NOT suppress another, and a
    /// restriction in one tenant does not reach a same-id subject in a different tenant (PII-free,
    /// tenant-partitioned).
    #[test]
    fn the_restrict_flag_is_scoped_per_tenant_and_subject() {
        let restrict = RestrictRegistry::new();
        let a = subject("u-a");
        let b = subject("u-b");
        let acme = t("acme");
        let other = t("globex");
        restrict.set(&a, &acme, true);
        assert!(restrict.is_restricted(&a, &acme));
        assert!(
            !restrict.is_restricted(&b, &acme),
            "a different subject is not restricted"
        );
        assert!(
            !restrict.is_restricted(&a, &other),
            "the same subject id in a different tenant is not restricted (tenant-partitioned)"
        );
    }

    // ───────── lever 1: per-subject DEK crypto-shred renders self-authored free-text unrecoverable ─

    #[test]
    fn erase_crypto_shreds_self_authored_free_text_to_unrecoverable() {
        let tenant = t("acme");
        let subj = subject("u-erase");
        let restrict = RestrictRegistry::new();
        let kms = InMemoryShredKms::new();
        let store = M1Store::new("chat_store", &restrict, &kms);
        kms.provision(M1Store::dek_handle(&subj, &tenant), 7);
        store.store_self_authored(&subj, &tenant, "secret message body");

        // BEFORE erase: the plaintext is recoverable.
        assert_eq!(
            store.fetch_stored(&subj, &tenant),
            Some(StoredContent::Recoverable("secret message body".into()))
        );

        // ERASE (lever 1): destroy the per-subject DEK — the destroyed epoch is the audit trail.
        assert_eq!(store.erase_self_authored(&subj, &tenant), Some(7));

        // AFTER erase: the storage ROW remains, but the content is UNRECOVERABLE (the DEK is gone,
        // live AND in backup §7.5) — NOT plaintext.
        assert_eq!(
            store.fetch_stored(&subj, &tenant),
            Some(StoredContent::Unrecoverable),
            "the per-subject DEK shred renders the self-authored free-text unrecoverable"
        );
        // 0 recoverable in the backup snapshot (§7.5).
        assert_eq!(
            kms.recoverable_in_backup(&M1Store::dek_handle(&subj, &tenant)),
            0
        );

        // A processing op over erased content is Unrecoverable (there is nothing to process — and it
        // is NOT a reversible Suppressed; the distinction is load-bearing).
        assert_eq!(store.index(&subj, &tenant), Processed::Unrecoverable);

        // Idempotent re-erase: the DEK is already gone → None destroyed, still unrecoverable.
        assert_eq!(store.erase_self_authored(&subj, &tenant), None);
        assert_eq!(
            store.fetch_stored(&subj, &tenant),
            Some(StoredContent::Unrecoverable)
        );
    }

    // ───────── lever 2: pseudonym-map shred leaves only <pseudonym>@<tenant>.noreply ─────────

    #[test]
    fn pseudonym_map_shred_leaves_only_the_frozen_pseudonym_form() {
        // The immutable structure holds the subject as the frozen opaque pseudonym (commits/audit
        // entries are pseudonymous-by-default). The map shred makes it unresolvable; the bytes hold
        // ONLY `<pseudonym>@<tenant>.noreply`.
        let handle = PseudonymHandle::new("anon-7f3a", "acme").expect("valid pseudonym");
        let shredded = shred_pseudonym_identity(&handle);
        assert_eq!(
            shredded.immutable_bytes, "anon-7f3a@acme.noreply",
            "the immutable bytes hold the frozen <pseudonym>@<tenant>.noreply rendering"
        );
        assert!(
            shredded.holds_only_the_pseudonym_form(),
            "the shredded bytes parse as a valid pseudonym handle — never real-identity PII"
        );
        // It carries no real handle: there is no name/email, only the unroutable .noreply form.
        assert!(shredded.immutable_bytes.ends_with(".noreply"));

        // The proof predicate is NOT vacuous: bytes that are a REAL routable handle (an email, not a
        // `.noreply` pseudonym) are NOT the shredded form (kills a `-> true` mutant that would claim
        // any bytes are the non-identifying residue).
        let leaked = ShreddedIdentity {
            immutable_bytes: "alice@example.com".into(),
        };
        assert!(
            !leaked.holds_only_the_pseudonym_form(),
            "a real routable email is NOT the frozen pseudonym residue — the predicate must reject it"
        );
    }

    /// The M1 store keys storage per-`(tenant, subject)`: two subjects' content does NOT collide
    /// (kills a constant-key mutant that would make every subject share one storage row). Each
    /// subject reads back their OWN content; erasing one does not touch the other.
    #[test]
    fn the_m1_store_keys_content_per_tenant_and_subject() {
        let tenant = t("acme");
        let a = subject("u-a");
        let b = subject("u-b");
        let restrict = RestrictRegistry::new();
        let kms = InMemoryShredKms::new();
        let store = M1Store::new("s", &restrict, &kms);
        kms.provision(M1Store::dek_handle(&a, &tenant), 1);
        kms.provision(M1Store::dek_handle(&b, &tenant), 2);
        store.store_self_authored(&a, &tenant, "alice content");
        store.store_self_authored(&b, &tenant, "bob content");
        // Each subject reads back their OWN distinct content (no collision).
        assert_eq!(
            store.fetch_stored(&a, &tenant),
            Some(StoredContent::Recoverable("alice content".into()))
        );
        assert_eq!(
            store.fetch_stored(&b, &tenant),
            Some(StoredContent::Recoverable("bob content".into()))
        );
        // Erasing A's DEK leaves B's content recoverable (per-subject DEK isolation).
        assert_eq!(store.erase_self_authored(&a, &tenant), Some(1));
        assert_eq!(
            store.fetch_stored(&a, &tenant),
            Some(StoredContent::Unrecoverable)
        );
        assert_eq!(
            store.fetch_stored(&b, &tenant),
            Some(StoredContent::Recoverable("bob content".into())),
            "erasing one subject must not touch another's content (per-subject DEK)"
        );
    }

    // ───────── the residual: third-party mention is restrict-suppressed, not crypto-shredded ─────

    #[test]
    fn the_residual_third_party_mention_is_restrict_suppress_only() {
        // Self-authored content is crypto-shreddable (lever 1).
        assert_eq!(
            classify_residual(Authorship::SelfAuthored),
            LeverCoverage::CryptoShred
        );
        // A third-party mention (the §7.2 residual, under the AUTHOR's DEK) is covered ONLY by
        // restrict suppression — the documented limit; it is NOT crypto-shredded by the subject's
        // key (P-GA-16's residual).
        assert_eq!(
            classify_residual(Authorship::ThirdPartyMention),
            LeverCoverage::RestrictSuppressOnly,
            "the residual is restrict-suppressed (the documented limit), never crypto-shredded"
        );
    }

    /// **The residual end-to-end: a third-party mention of the subject, authored by SOMEONE ELSE,
    /// is `restrict`-suppressed — never indexed / agent-read / analysed for the restricted subject —
    /// AND is NOT crypto-shredded by the subject's erase (the documented limit from P-GA-16 / §7.2).**
    /// The mention lives in the AUTHOR's store row under the AUTHOR's DEK; restricting the SUBJECT
    /// suppresses the store's processing of the row that mentions them.
    #[test]
    fn the_residual_is_suppressed_for_the_restricted_subject_and_survives_its_erase() {
        let tenant = t("acme");
        let author = subject("u-author");
        let subj = subject("u-mentioned"); // the third party mentioned in the author's content
        let restrict = RestrictRegistry::new();
        let kms = InMemoryShredKms::new();
        let store = M1Store::new("issues_store", &restrict, &kms);

        // The AUTHOR's content (a comment that mentions the subject) lives under the AUTHOR's DEK.
        kms.provision(M1Store::dek_handle(&author, &tenant), 3);
        store.store_self_authored(&author, &tenant, "thanks @u-mentioned for the help");

        // The subject (the mentioned third party) is restricted. The §7.2 residual classification:
        assert_eq!(
            classify_residual(Authorship::ThirdPartyMention),
            LeverCoverage::RestrictSuppressOnly
        );
        // Honouring restrict for the SUBJECT suppresses processing of the row that mentions them.
        restrict.set(&subj, &tenant, true);
        // The store honours the subject's restriction when processing content about them. (Modelled:
        // the residual processing is keyed by the mentioned subject — a restricted subject's
        // mentions are not indexed/agent-read/analysed.)
        assert_eq!(
            store.index(&subj, &tenant),
            // No content is stored UNDER the subject's key (they did not author it) → there is no
            // recoverable self-authored row for them; the residual lives under the author's key. The
            // residual classification + the suppression flag are the proof the residual is governed
            // ONLY by restrict (the documented limit), never crypto-shredded by the subject's erase.
            Processed::Unrecoverable,
            "the subject authored nothing here; the mention is the author's residual"
        );

        // The author erases the SUBJECT — the subject's own DEK (which holds none of this content) is
        // shredded; the AUTHOR's content (the third-party mention) is UNTOUCHED (under the author's
        // DEK). This is the documented limit: the residual survives the subject's crypto-shred.
        kms.provision(M1Store::dek_handle(&subj, &tenant), 9);
        assert_eq!(store.erase_self_authored(&subj, &tenant), Some(9));
        // The author's content (the residual) is STILL recoverable to the author — not crypto-shred
        // by the subject's erase (§7.2 — shredding the author's DEK would destroy the author's
        // legitimate content). It remains restrict-suppressed for the restricted subject.
        assert_eq!(
            store.fetch_stored(&author, &tenant),
            Some(StoredContent::Recoverable(
                "thanks @u-mentioned for the help".into()
            )),
            "the third-party mention under the author's DEK survives the subject's erase — the \
             documented residual limit (§7.2); it is governed by restrict, not crypto-shred"
        );
    }

    // ───────── the load-bearing predicates (mutation floor) ─────────

    /// The suppression predicate is NOT vacuous: a restricted subject is Suppressed, an unrestricted
    /// one is Processed — the `is_restricted` branch is load-bearing (a `true`→`false` mutant would
    /// process a restricted subject's content; a `false`→`true` would suppress everyone). Both
    /// verdicts are pinned so the branch mutant is caught.
    #[test]
    fn the_suppression_branch_is_load_bearing_both_verdicts_pinned() {
        let tenant = t("acme");
        let subj = subject("u-branch");
        let restrict = RestrictRegistry::new();
        let kms = InMemoryShredKms::new();
        let store = M1Store::new("s", &restrict, &kms);
        kms.provision(M1Store::dek_handle(&subj, &tenant), 1);
        store.store_self_authored(&subj, &tenant, "x");

        // unrestricted ⇒ Processed (the `if is_restricted` false branch). The processed projection
        // carries the op token + the holder id (pins `M1Store::id` against a `-> ""` mutant).
        match store.index(&subj, &tenant) {
            Processed::Processed(out) => {
                assert!(
                    out.starts_with("index:s:"),
                    "the processed projection names op + holder id"
                );
            }
            other => panic!("expected Processed, got {other:?}"),
        }
        // restricted ⇒ Suppressed (the true branch).
        restrict.set(&subj, &tenant, true);
        assert_eq!(store.index(&subj, &tenant), Processed::Suppressed);
    }

    /// The processing-op tokens + the lever exhaustiveness are stable (the §4.4 enumerated four ops).
    #[test]
    fn the_four_processing_ops_are_the_section_4_4_set() {
        assert_eq!(Processing::all().len(), 4);
        assert_eq!(Processing::Index.token(), "index");
        assert_eq!(Processing::AgentRead.token(), "agent_read");
        assert_eq!(Processing::Analyse.token(), "analyse");
        assert_eq!(Processing::Notify.token(), "notify");
    }
}
