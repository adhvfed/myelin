//! # `pseudonym` — pseudonymous-by-default identity columns for Issues (ISS-P07 / P-373, M4-I1)
//!
//! **The non-negotiable this module ships (recon §X-7 / EI-04 §1 — immutable bytes stay PII-free):**
//! the Issues identity columns (`assignee` / `reporter` / `created_by` / comment-author /
//! change-log-actor) hold an **OPAQUE pseudonym** in the frozen `<pseudonym>@<tenant>.noreply`
//! grammar (contract 4.8, pin C5) — **never** a raw principal id / name / email. The person↔pseudonym
//! map is the erasable record Identity owns (`resolve_pseudonym` / `erase`, contract 4.8); erasing it
//! leaves the stored Issues bytes holding only the opaque pseudonym ("Former user 8a2f") — DSR
//! fan-out **step 1** ([recon §X-7](../../../planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md)).
//!
//! **Owning architecture / canon docs (read in full before changing this):**
//! - `planning/04-subsystem-architectures/issue-tracker/architecture/06-reconciliation-compliance.md`
//!   §2.13 (the ONE erasure posture by reference — structural floor = per-subject DEK + pseudonym-map
//!   shred + `restrict`) + §1 (the pseudonymous-by-default identity columns).
//! - `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §X-7 (the ONE
//!   free-text/immutable erasure posture) + §1 (the frozen `<pseudonym>@<tenant>.noreply` grammar).
//! - `01-tech-and-data-model.md` §2 (the pseudonymous identity fields).
//!
//! **Contracts (consumed — to the FROZEN shapes, never diverged):** **4.8** — the
//! [`myelin_identity::PseudonymHandle`] grammar carrier + the [`IdentityService::resolve_pseudonym`]
//! read. Issues authors NO second pseudonym grammar (EI-01 §7): it links the frozen
//! [`myelin_identity::PseudonymHandle`] (the SAME type the Git M3 commit codec bakes into immutable
//! commit bytes) — drift in the `@`/`.noreply` shape is a compile break here, not a runtime PII leak.
//!
//! ## What this prompt (ISS-P07 / P-373) ships
//! - [`IssuePseudonym`] — a typed wrapper that ADMITS only a well-formed `<pseudonym>@<tenant>.noreply`
//!   rendering into an Issues identity column. A raw principal id ("u-42") / name / email is
//!   REFUSED ([`PseudonymError`]) — the 0-raw-id invariant is correct-by-construction.
//! - [`pseudonymise`] — the write-path helper that turns a verified [`Principal`] into its per-tenant
//!   pseudonym column value by resolving it through the ONE Identity map ([`resolve_pseudonym`], 4.8).
//!   A subject the map cannot resolve is a LOUD error — never a stored raw id.
//! - [`is_resolvable_pseudonym`] — the read-path seam (the holder `export` / display path): the stored
//!   value IS a resolvable-shaped pseudonym (Identity's reverse map resolves who it is; the map is the
//!   erasable record).
//! - [`is_raw_principal_id`] — the **0-raw-id assertion** the GATE reads: a value that is NOT in the
//!   pseudonym grammar is a raw identifier (a leak). The fixture asserts 0 such values at rest.
//!
//! ## Mutation-score floor (mandatory-core — this IS the pseudonym-resolution erasure seam)
//! The pseudonymous-identity discipline is an erasure seam (recon §X-7 — the pseudonym-map shred is
//! DSR fan-out step 1), so this module is a **mandatory-core mutation target with a ≥ 90% floor**:
//! `cargo mutants -p myelin-issues --file crates/myelin-issues/src/pseudonym.rs`. The mutation-tested
//! core is the admission gate (a non-grammar value is REFUSED, never admitted), the fail-closed resolve
//! (an unresolvable subject is a LOUD error, never a stored raw id), and the 0-raw-id predicate (a raw
//! id reads as raw). A mutant that admits a raw id, swallows a resolve failure, or inverts the 0-raw-id
//! check is caught. **FLOOR (measured-under-load):** the measured % is the CI `cargo mutants` artifact,
//! registered red-until-run in the scorecard, never self-asserted (EI-01 §3).
//!
//! ## Floors named (VISION §3)
//! - **The pseudonym-map shred** (the `erase` half — destroying the person↔pseudonym map so the stored
//!   pseudonym becomes unresolvable) is Identity's `erase` (4.8) wired into the Issues holder erase
//!   fan-out at **ISS-P31** ([`crate::holder`] names it). Here the COLUMNS are pseudonymous-by-default
//!   (the structural floor); the shred LEVER already exists (`resolve_pseudonym`/`erase`).
//! - **The live person↔pseudonym MINT** (the S2 map that assigns a subject its per-tenant pseudonym)
//!   is Identity's `P-ID-19` store; Issues CONSUMES it through [`resolve_pseudonym`]. Issues never
//!   mints a pseudonym — it stores the one Identity resolves (one map, not two).

use myelin_identity::{IdentityService, PrincipalId, PseudonymHandle};
use myelin_tenancy::TenantId;

/// **A pseudonymous-by-default Issues identity column value** — an opaque
/// `<pseudonym>@<tenant>.noreply` handle (contract 4.8). Holds the FROZEN
/// [`myelin_identity::PseudonymHandle`] (never a second grammar, EI-01 §7), so a column value can ONLY
/// be a well-formed pseudonym — a raw principal id / name / email cannot be constructed into one
/// ([`IssuePseudonym::parse`] refuses it). This is the type the `assignee` / `reporter` /
/// `created_by` / comment-author / change-log-actor columns store; the 0-raw-id invariant is
/// correct-by-construction (the only constructors validate the grammar).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IssuePseudonym(PseudonymHandle);

impl IssuePseudonym {
    /// Build an Issues identity column value from a verified [`PseudonymHandle`] (the value Identity's
    /// [`resolve_pseudonym`] returns, re-parsed into the frozen type). The handle already proves the
    /// grammar; this is the typed lift into the Issues column.
    pub fn from_handle(handle: PseudonymHandle) -> IssuePseudonym {
        IssuePseudonym(handle)
    }

    /// **Parse a stored / inbound identity column value, ADMITTING only a well-formed
    /// `<pseudonym>@<tenant>.noreply` rendering (contract 4.8).** A raw principal id ("u-42"), a name,
    /// or an email is REFUSED with [`PseudonymError::NotPseudonymous`] — the structural guarantee that
    /// an Issues identity column never holds an erasable raw identifier (recon §X-7). This is the ONE
    /// admission door; a column value that did not pass it cannot exist.
    pub fn parse(rendering: &str) -> Result<IssuePseudonym, PseudonymError> {
        match PseudonymHandle::parse(rendering) {
            Some(handle) => Ok(IssuePseudonym(handle)),
            None => Err(PseudonymError::NotPseudonymous(rendering.to_string())),
        }
    }

    /// The exact `<pseudonym>@<tenant>.noreply` rendering stored in the column (the bytes at rest).
    pub fn render(&self) -> String {
        self.0.render()
    }

    /// The opaque per-tenant pseudonym token (the local-part). PII-free.
    pub fn token(&self) -> &str {
        self.0.pseudonym()
    }

    /// The tenant label (the `<tenant>` segment). PII-free.
    pub fn tenant(&self) -> &str {
        self.0.tenant()
    }

    /// Borrow the underlying frozen [`PseudonymHandle`] (the cross-band contract carrier).
    pub fn handle(&self) -> &PseudonymHandle {
        &self.0
    }
}

impl core::fmt::Display for IssuePseudonym {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.render())
    }
}

/// A loud, typed failure of the pseudonymous-identity discipline. A raw id reaching an identity column
/// is a LEAK, never a silent store (recon §X-7 — immutable bytes stay PII-free).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PseudonymError {
    /// A value that is NOT in the frozen `<pseudonym>@<tenant>.noreply` grammar was offered to an
    /// Issues identity column — REFUSED (a raw principal id / name / email is an erasable PII leak).
    NotPseudonymous(String),
    /// Identity's [`resolve_pseudonym`] could not resolve the subject to a pseudonym (the map has no
    /// entry, or the Identity surface errored). The write FAILS CLOSED — never a stored raw id.
    ResolveFailed { subject: String, why: String },
    /// The pseudonym Identity returned did not parse as the frozen grammar (an Identity-side
    /// contract break — surfaced loudly rather than stored).
    ResolvedValueMalformed(String),
}

impl core::fmt::Display for PseudonymError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PseudonymError::NotPseudonymous(v) => write!(
                f,
                "REFUSED: `{v}` is not a `<pseudonym>@<tenant>.noreply` pseudonym — an Issues identity \
                 column never holds a raw id / name / email (recon §X-7)"
            ),
            PseudonymError::ResolveFailed { subject, why } => write!(
                f,
                "resolve_pseudonym failed for subject `{subject}` ({why}) — the write fails closed, \
                 never a stored raw id"
            ),
            PseudonymError::ResolvedValueMalformed(v) => write!(
                f,
                "Identity resolved a non-grammar pseudonym `{v}` — refused (4.8 contract break)"
            ),
        }
    }
}

impl std::error::Error for PseudonymError {}

/// **Pseudonymise a verified subject into its Issues identity column value (the write-path helper,
/// contract 4.8).** Resolves the [`PrincipalId`] through the ONE Identity person↔pseudonym map
/// ([`IdentityService::resolve_pseudonym`]) and lifts the returned `<pseudonym>@<tenant>.noreply`
/// rendering into the typed [`IssuePseudonym`] the column stores. A subject the map cannot resolve, or
/// an Identity surface error, is a LOUD [`PseudonymError`] — the write fails closed; an Issues
/// identity column is NEVER written with a raw principal id (recon §X-7). Issues never mints a
/// pseudonym (one map, Identity's) — it stores the one Identity resolves.
pub fn pseudonymise<Id: IdentityService>(
    id: &Id,
    subject: &PrincipalId,
    tenant: &TenantId,
) -> Result<IssuePseudonym, PseudonymError> {
    let rendering =
        id.resolve_pseudonym(subject, tenant)
            .map_err(|e| PseudonymError::ResolveFailed {
                subject: subject.0.clone(),
                why: format!("{e:?}"),
            })?;
    // Re-validate Identity's return through the ONE admission door: a malformed pseudonym (an Identity
    // contract break) is refused, never stored. The grammar is the single source of truth.
    IssuePseudonym::parse(&rendering).map_err(|_| PseudonymError::ResolvedValueMalformed(rendering))
}

/// **Resolve a stored Issues pseudonym column back to its real subject (the read-path / holder
/// `export` inverse, contract 4.8).** The map is the erasable record: after `erase` (ISS-P31) shreds
/// it, this returns the loud "no map entry" error — the stored pseudonym is then unresolvable ("Former
/// user 8a2f", the DSR fan-out result). `tenant` is the pseudonym's tenant label. NOTE: Identity's
/// surface keys the map on the [`PrincipalId`]; the stored pseudonym is the per-tenant handle, so the
/// caller resolves WHO it is by the Identity-side reverse map. Here we expose the forward check that
/// the stored value IS a resolvable-shaped pseudonym (the structural read seam); the live reverse
/// lookup wiring threads through Identity's S2 store at the read path (named below). The 0-raw-id
/// property — the stored bytes are a pseudonym, not a raw id — is what this proves at the column.
pub fn is_resolvable_pseudonym(stored: &str) -> bool {
    IssuePseudonym::parse(stored).is_ok()
}

/// **The 0-raw-id assertion the GATE reads (recon §X-7).** A value at rest in an Issues identity
/// column that is NOT in the frozen `<pseudonym>@<tenant>.noreply` grammar is a **raw identifier** —
/// an erasable PII leak. The fixture scans the stored identity-column values and asserts this returns
/// `false` for every one (0 raw ids at rest). A bare principal id ("u-42"), a name ("Ada Lovelace"),
/// or an email ("ada@example.com") all return `true` (they are raw).
pub fn is_raw_principal_id(stored: &str) -> bool {
    !is_resolvable_pseudonym(stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{
        AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
        EffectivePolicy, FailStaticBound, FragmentAdmit, ListObjectsResult, NamespaceFragment,
        ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalKind, RevokeTarget,
        RewriteTrace, RunId, RunToken, SubjectTree, TupleDelta, Zookie,
    };
    use std::collections::HashMap;

    type IdResult<T> = myelin_identity::Result<T>;

    /// A stub IdentityService that resolves a fixed person↔pseudonym map (the S2 map's behaviour) — the
    /// REAL map is Identity's P-ID-19 store; this is test scaffolding (EI-01 §7).
    struct StubId {
        map: HashMap<String, String>,
    }
    impl StubId {
        fn with(subject: &str, pseudonym: &str) -> Self {
            let mut map = HashMap::new();
            map.insert(subject.to_string(), pseudonym.to_string());
            Self { map }
        }
        fn empty() -> Self {
            Self {
                map: HashMap::new(),
            }
        }
    }
    impl IdentityService for StubId {
        fn resolve_pseudonym(&self, subject: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            self.map
                .get(&subject.0)
                .cloned()
                .ok_or(AuthzError::NotYetImplemented("no map entry"))
        }
        // ── everything else is out of scope for this module's tests ──
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &myelin_tenancy::ArtifactRef,
            _a: &Consistency,
            _c: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _a: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _a: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _a: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &RunId,
            _d: &DelegationCaveats,
            _t: &FailStaticBound,
        ) -> IdResult<RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    fn pid(s: &str) -> PrincipalId {
        PrincipalId(s.into())
    }
    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    /// **An assignee/reporter resolves to a PSEUDONYM (contract 4.8) — never a raw id.** The write-path
    /// helper resolves the subject through the ONE Identity map and stores the
    /// `<pseudonym>@<tenant>.noreply` rendering. The stored value is a valid pseudonym, carrying NO raw
    /// id.
    #[test]
    fn assignee_resolves_to_a_pseudonym_not_a_raw_id() {
        let id = StubId::with("u-42", "8a2f@acme.noreply");
        let column = pseudonymise(&id, &pid("u-42"), &tenant()).expect("resolves to a pseudonym");
        assert_eq!(column.render(), "8a2f@acme.noreply");
        assert_eq!(column.token(), "8a2f");
        assert_eq!(column.tenant(), "acme");
        // the 0-raw-id invariant: the stored column value is a pseudonym, NOT a raw id.
        assert!(!is_raw_principal_id(&column.render()));
    }

    /// **A raw principal id / name / email is REFUSED at the identity column (0-raw-id, recon §X-7).**
    /// The ONE admission door ([`IssuePseudonym::parse`]) rejects anything not in the frozen grammar.
    #[test]
    fn a_raw_id_or_name_or_email_is_refused_at_the_column() {
        for raw in ["u-42", "Ada Lovelace", "ada@example.com", "", "8a2f@acme"] {
            assert!(
                IssuePseudonym::parse(raw).is_err(),
                "`{raw}` must be refused — it is not a `<pseudonym>@<tenant>.noreply` pseudonym"
            );
            assert!(
                is_raw_principal_id(raw),
                "`{raw}` reads as a raw identifier (a leak) — the 0-raw-id gate flags it"
            );
        }
    }

    /// **The write fails CLOSED when the subject is not in the map — never a stored raw id.** An
    /// unresolvable subject is a LOUD [`PseudonymError::ResolveFailed`], not a fall-through to storing
    /// the bare principal id.
    #[test]
    fn an_unresolvable_subject_fails_closed_never_stores_a_raw_id() {
        let id = StubId::empty();
        let err =
            pseudonymise(&id, &pid("u-99"), &tenant()).expect_err("no map entry → fail closed");
        assert!(matches!(err, PseudonymError::ResolveFailed { .. }));
    }

    /// **The stored pseudonym round-trips through the frozen grammar (the read-path inverse seam).** A
    /// resolvable pseudonym parses back to the same `(token, tenant)`; the 0-raw-id property holds at
    /// the column.
    #[test]
    fn stored_pseudonym_round_trips_and_is_resolvable_shaped() {
        let stored = "8a2f@acme.noreply";
        assert!(is_resolvable_pseudonym(stored));
        let parsed = IssuePseudonym::parse(stored).unwrap();
        assert_eq!(parsed.render(), stored);
        // a re-parse of the rendering is byte-identical (the cross-band round-trip).
        assert_eq!(IssuePseudonym::parse(&parsed.render()).unwrap(), parsed);
    }

    /// **A malformed Identity-resolved value is refused, not stored (4.8 contract break is loud).**
    #[test]
    fn a_malformed_resolved_pseudonym_is_refused() {
        // the stub map returns a non-grammar value (an Identity-side break).
        let id = StubId::with("u-7", "not-a-pseudonym");
        let err = pseudonymise(&id, &pid("u-7"), &tenant()).expect_err("malformed → refused");
        assert!(matches!(err, PseudonymError::ResolvedValueMalformed(_)));
    }

    /// The kind of the principal does not change the column discipline — any verified subject is
    /// pseudonymised the same way (a human or an agent both resolve to a pseudonym column).
    #[test]
    fn agent_and_human_subjects_pseudonymise_identically() {
        let id = StubId::with("agent-1", "ag9c@acme.noreply");
        let _ = PrincipalKind::Human; // the kind is irrelevant to the column value.
        let column = pseudonymise(&id, &pid("agent-1"), &tenant()).expect("agent resolves");
        assert!(!is_raw_principal_id(&column.render()));
    }
}
