//! # The CDC pair for contract 4.2 (+ 4.10) — Knowledge's per-op authority half (KN-P14 / P-304)
//!
//! **Contract-index row 4.2** (`check` + `CaveatContext`, the per-action fail-closed gate) and **row
//! 4.10** (`Consistency`/zookie, the read-your-writes new-enemy guard). The Identity-side provider+
//! consumer pairs ship in `myelin-identity-service` (`cdc_4_2_check.rs` / `cdc_4_11_fail_static.rs`).
//! THIS is the **KNOWLEDGE consumer half** the KN-P14 TESTS field names — the focused, in-CI evidence
//! that Knowledge's Layer-2 per-op gate (`02-internals-and-algorithms.md` §3.1) consumes the FROZEN
//! `check`/`Consistency` ABI the way the contract specifies and cannot drift from it:
//!
//! - the **PROVIDER** promise (the `check` ABI): a grant ⇒ `Allow`; a grant revoked at-or-after the
//!   read's zookie ⇒ NOT `Allow` (the new-enemy guard, 4.10); uncertainty ⇒ NOT `Allow` (fail-closed);
//! - the **CONSUMER** promise (Knowledge's [`myelin_knowledge::OpAuthorizer`]): before it hands an op
//!   to the merge layer it calls `check(actor, edit|comment, page_ref, Strong@page.acl_zookie)` and
//!   applies the op ONLY on `Allow` — a `Deny`/`Conditional`/error refuses the op (fail-closed), and
//!   the strong read at-or-after `page.acl_zookie` means a just-revoked editor's op is rejected.
//!
//! A change to either side fails this test in the same CI job. The read-side `list_objects` SetExpr
//! push-down is the KN-P16 follow-on (NAMED); this pair is the per-op WRITE-side `check`-gate the
//! prompt requires. The per-mechanism unit tests live in `authority.rs::tests`; the chained
//! just-revoked drill in `tests/drill_kn_p14_just_revoked_editor.rs`.

use myelin_identity::{
    AuthzError, CaveatContext, Consistency, ConsistencyMode, Credential, DataRole, Decision,
    DelegationCaveats, EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService,
    ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal,
    PrincipalId, PrincipalKind, PrincipalStatus, RevokeTarget, RewriteTrace, RunId, RunToken,
    SubjectTree, TupleDelta, Zookie,
};
use myelin_knowledge::{
    AuthZookie, CollectionSchema, IncomingOp, OpAuthorizer, OpDecision, OpPermission,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

// ─────────────────────────────── the PROVIDER: the frozen `check` ABI ───────────────────────────

/// The PROVIDER half: a `check` surface honouring the 4.2/4.10 promise — a grant allows unless the
/// read's zookie is at-or-after the revocation (the new-enemy guard); no grant / uncertainty is NOT
/// `Allow` (fail-closed). This is the contract the Identity service's `StoreBackedCheck` provider also
/// promises (its own CDC pins the engine; this pins Knowledge's consumption of the SAME shape).
struct ProviderCheck {
    granted: std::collections::HashSet<String>,
    revoked_at: std::collections::HashMap<String, String>,
    /// A subject the provider is GENUINELY UNSURE about (returns `Conditional` — must be treated as
    /// not-`Allow` by the consumer, fail-closed).
    uncertain: std::collections::HashSet<String>,
}

impl ProviderCheck {
    fn new() -> ProviderCheck {
        ProviderCheck {
            granted: std::collections::HashSet::new(),
            revoked_at: std::collections::HashMap::new(),
            uncertain: std::collections::HashSet::new(),
        }
    }
    fn grant(&mut self, s: &str) {
        self.granted.insert(s.to_string());
    }
    fn revoke_at(&mut self, s: &str, rev: &str) {
        self.revoked_at.insert(s.to_string(), rev.to_string());
    }
    fn make_uncertain(&mut self, s: &str) {
        self.uncertain.insert(s.to_string());
    }
}

impl IdentityService for ProviderCheck {
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        // The CONSUMER's promise pinned from the provider side: Knowledge always passes the frozen
        // shape — a non-empty page object, an `edit`|`comment` permission, a STRONG consistency, no
        // caveat for a page-level check.
        assert!(!object.0.is_empty(), "the consumer passes a non-empty page ArtifactRef");
        assert!(
            permission == &Permission("edit".into()) || permission == &Permission("comment".into()),
            "the consumer authorizes edit|comment (the two collab verbs)"
        );
        assert_eq!(at.mode, ConsistencyMode::Strong, "the consumer reads at Strong consistency (4.10)");
        assert!(caveat.is_none(), "a page-level per-op check passes no CaveatContext (the field core is KN-P16/P-ID-22)");

        let id = &subject.principal_id.0;
        if self.uncertain.contains(id) {
            return Ok(Decision::Conditional); // genuine uncertainty → NOT Allow (consumer fail-closes)
        }
        if !self.granted.contains(id) {
            return Ok(Decision::Deny);
        }
        if let Some(rev) = self.revoked_at.get(id) {
            if !rev.is_empty() && at.at_least.0.as_str() >= rev.as_str() {
                return Ok(Decision::Deny); // the new-enemy guard (4.10)
            }
        }
        Ok(Decision::Allow)
    }
    fn authenticate(&self, _c: &Credential) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("list_objects → KN-P16"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> myelin_identity::Result<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> myelin_identity::Result<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> myelin_identity::Result<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(
        &self,
        _d: &[TupleDelta],
        _p: Option<&Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> myelin_identity::Result<RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(
        &self,
        _s: &PrincipalId,
        _t: &TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> myelin_identity::Result<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

fn actor(id: &str) -> Principal {
    Principal::new(
        TenantId("acme".into()),
        Region("eu-west".into()),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn op(actor: Principal, page: &str, zookie: AuthZookie) -> IncomingOp {
    IncomingOp {
        actor,
        page_id: page.to_string(),
        object: ArtifactRef(format!("kn:page:{page}")),
        permission: OpPermission::Edit,
        zookie,
        block_id: None,
        db_row: vec![],
    }
}

/// **The CONSUMER applies an op IFF `check` returned `Allow` (contract 4.2, fail-closed).** The pair:
/// a granted actor → Allow → applied; an ungranted actor → Deny → refused; a `Conditional` (genuine
/// uncertainty) → refused (fail-closed, never a silent allow).
#[test]
fn cdc_4_2_consumer_applies_only_on_allow() {
    let mut p = ProviderCheck::new();
    p.grant("alice");
    p.make_uncertain("carol");
    let mut auth = OpAuthorizer::new(p);

    // Allow → applied.
    let a = op(actor("alice"), "p1", AuthZookie::empty());
    let d_a = auth.authorize_op(&a, &CollectionSchema::new());
    assert_eq!(d_a, OpDecision::Apply, "a granted op is authorized");
    assert!(auth.apply_if_authorized(&d_a), "the consumer applies on Allow");

    // Deny → refused.
    let b = op(actor("bob"), "p1", AuthZookie::empty());
    let d_b = auth.authorize_op(&b, &CollectionSchema::new());
    assert!(d_b.is_rejected(), "an ungranted op is refused (fail-closed)");
    assert!(!auth.apply_if_authorized(&d_b));

    // Conditional (uncertainty) → refused (fail-closed — NOT a silent allow).
    let c = op(actor("carol"), "p1", AuthZookie::empty());
    let d_c = auth.authorize_op(&c, &CollectionSchema::new());
    assert!(d_c.is_rejected(), "a Conditional decision is treated as not-Allow (fail-closed, ADR-03)");
}

/// **The CONSUMER consumes the 4.10 zookie correctly: it reads `check` at-or-after `page.acl_zookie`,
/// so a grant revoked at-or-after the stamp is rejected (read-your-writes / new-enemy).** This pins
/// Knowledge's 4.10 consumption: the just-revoked editor's op cannot be served stale.
#[test]
fn cdc_4_10_consumer_reads_at_or_after_page_acl_zookie() {
    let mut p = ProviderCheck::new();
    p.grant("alice");
    p.revoke_at("alice", "z7");
    let mut auth = OpAuthorizer::new(p);

    // Before the ACL change (page at empty zookie) — Alice's grant holds.
    let before = op(actor("alice"), "p1", AuthZookie::empty());
    assert_eq!(auth.authorize_op(&before, &CollectionSchema::new()), OpDecision::Apply);

    // The ACL change stamps page.acl_zookie forward to z7.
    assert!(auth.acl_zookies_mut().stamp("p1", Zookie("z7".into())));

    // Alice's op now reads at-or-after z7 → Deny (the new-enemy guard); refused, 0 stale-grant writes.
    let after = op(actor("alice"), "p1", AuthZookie::of(Zookie("z7".into())));
    let d = auth.authorize_op(&after, &CollectionSchema::new());
    assert!(d.is_rejected(), "the just-revoked op is refused (4.10 read-your-writes)");
    assert!(!auth.apply_if_authorized(&d));
    assert_eq!(auth.counter().stale_grant_writes(), 0, "0 stale-grant writes");
}
