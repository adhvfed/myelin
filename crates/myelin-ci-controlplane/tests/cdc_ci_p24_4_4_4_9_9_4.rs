//! # The CDC pair for CI-P24 (P-367) — the CONSUMED rows **4.4 + 4.9 + 9.4**.
//!
//! CI-P24 (the in-boundary secret broker + the protected-env HITL gate) is a CONSUMER of three frozen
//! contract surfaces. This CDC freezes the CI-CONSUMER side of each — a rename / shape drift on either
//! side is a CDC break, never a silent divergence (EI-01 §7):
//!
//! - **Row 4.4 — `list_subjects` (the HITL approver set).** The deploy gate resolves the protected-env
//!   approver audience via `list_subjects(environment, approve)`. THIS side pins that the CI consumer
//!   calls the FROZEN `IdentityService::list_subjects` with the FROZEN `approve` permission over the
//!   `environment` object and flattens the returned `SubjectTree::members`. The PROVIDER half (Id's
//!   Expand engine) is `crates/myelin-identity-service/tests/cdc_4_4_list_subjects.rs`.
//!
//! - **Row 4.9 — the `read & !is_untrusted_fork` ABAC edge (fork gets no secrets).** The broker gates
//!   `secret.read` on this edge: a fork resolves to 0 (the structural short-circuit), a trusted run
//!   resolves only its DIRECT-NARROW-granted referenced names. THIS side pins the CI consumer's
//!   `secret.read` permission spelling + the fork-no-secrets withhold. The PROVIDER half (the engine's
//!   compiled `run.read = view − is_untrusted_fork` Exclusion) is the engine `cdc_4_9_ci_fragment.rs`.
//!
//! - **Row 9.4 — the durable approval signal (per-effect `idem_key`, OQ-F).** The deploy gate composes
//!   the FROZEN `myelin_flow::per_effect_idem_key` (a double-click = one apply; a decline = withheld).
//!   THIS side pins that the CI consumer keys its deploy effects on the FROZEN §6.4 rule (`card_id`
//!   single / `card_id:idx` multi) and maps the FROZEN `EffectOutcome` onto the deploy domain. The
//!   PROVIDER half (the durable `wf_signal` consume-once) is `myelin-flow`'s `cdc_9_4_wait.rs`.

use myelin_ci_controlplane::deployment::{
    deploy_outcome_of, resolve_approvers, DeployGate, DeployGateOutcome,
    ENVIRONMENT_APPROVE_PERMISSION,
};
use myelin_ci_controlplane::secret_broker::{
    SecretBroker, SecretCapability, SecretOutcome, WithholdReason, SECRET_READ_PERMISSION,
};
use myelin_ci_sandbox::{SecretRef, TrustTier};
use myelin_flow::{per_effect_idem_key, ApprovalDecision, EffectOutcome};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, ConsistencyMode, Credential, Decision,
    DelegationCaveats, EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService,
    ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission, Precondition,
    Principal, PrincipalId, PrincipalKind, RelName, Result as IdResult, RevokeTarget, RewriteTrace,
    RunId, RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::cell::RefCell;
use std::collections::HashMap;

// ===========================================================================
// Row 4.4 — list_subjects(environment, approve): the HITL approver set.
// ===========================================================================

/// An Identity that records the (object, permission) of every `list_subjects` call, so the CDC can
/// assert the CONSUMER resolves the approver set via the FROZEN `approve` permission on `environment`.
struct RecordingId {
    last: RefCell<Option<(String, String)>>,
    approvers: Vec<String>,
}
impl IdentityService for RecordingId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn check(
        &self,
        _s: &Principal,
        p: &Permission,
        o: &ArtifactRef,
        _at: &Consistency,
        _cav: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        // The broker's `secret.read` gate (row 4.9) — record it for the 4.9 assertion.
        *self.last.borrow_mut() = Some((o.0.clone(), p.0.clone()));
        // Grant only the `h:granted` secret (the DIRECT NARROW grant).
        if o.0.ends_with("h:granted") {
            Ok(Decision::Allow)
        } else {
            Ok(Decision::Deny)
        }
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn list_subjects(
        &self,
        o: &ObjectId,
        p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        *self.last.borrow_mut() = Some((o.0.clone(), p.0.clone()));
        Ok(SubjectTree {
            object: o.clone(),
            relation: RelName(p.0.clone()),
            members: self
                .approvers
                .iter()
                .map(|a| PrincipalId(a.clone()))
                .collect(),
            zookie: Zookie("z0".into()),
        })
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _pre: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _ttl: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
}

fn at() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::Strong,
    }
}

#[test]
fn cdc_4_4_consumer_resolves_approvers_via_list_subjects_environment_approve() {
    let id = RecordingId {
        last: RefCell::new(None),
        approvers: vec!["u:alice".into(), "u:bob".into()],
    };
    let env = ObjectId("environment:prod".into());
    let approvers = resolve_approvers(&id, &env, &at()).expect("resolves");

    // The CONSUMER flattens `SubjectTree::members` into the approver audience.
    assert_eq!(approvers, vec!["u:alice".to_string(), "u:bob".to_string()]);
    // The CONSUMER called list_subjects with the FROZEN `approve` permission over THIS environment.
    let (obj, perm) = id
        .last
        .borrow()
        .clone()
        .expect("a list_subjects call was made");
    assert_eq!(obj, "environment:prod");
    assert_eq!(perm, ENVIRONMENT_APPROVE_PERMISSION);
    assert_eq!(
        ENVIRONMENT_APPROVE_PERMISSION, "approve",
        "the FROZEN §5.2 approve permission (4.4)"
    );
}

// ===========================================================================
// Row 4.9 — the read & !is_untrusted_fork edge: the broker's secret.read gate.
// ===========================================================================

struct ResolvingCap;
impl SecretCapability for ResolvingCap {
    fn resolve_handle(
        &self,
        tenant: &TenantId,
        object: &ArtifactRef,
        handle: &str,
    ) -> Option<String> {
        let expected_prefix = format!("myelin://{}/ci/secret/", tenant.0);
        let id = handle.strip_prefix(&expected_prefix)?;
        (!id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            && object.0 == handle)
            .then(|| format!("material:{handle}"))
    }
}

fn secret_object_of(r: &SecretRef) -> ArtifactRef {
    ArtifactRef(r.handle.clone())
}

fn subject(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    )
}

#[test]
fn cdc_4_9_broker_gates_secret_read_and_fork_short_circuits() {
    let cap = ResolvingCap;
    let id = RecordingId {
        last: RefCell::new(None),
        approvers: vec![],
    };
    let broker = SecretBroker::new(&cap, &id);

    let refs = vec![
        SecretRef {
            name: "GRANTED".into(),
            handle: "myelin://acme/ci/secret/granted".into(),
        },
        SecretRef {
            name: "DENIED".into(),
            handle: "myelin://acme/ci/secret/denied".into(),
        },
    ];

    // TRUSTED: the broker gates `secret.read` via the FROZEN `read` permission (4.9 DIRECT NARROW).
    let trusted = broker
        .resolve(
            TrustTier::Trusted,
            &subject("u:member"),
            secret_object_of,
            &refs,
            &at(),
        )
        .expect("resolves");
    assert_eq!(
        trusted.secret_count(),
        1,
        "only the DIRECT-granted name resolves (4.9 DIRECT NARROW)"
    );
    let (_obj, perm) = id.last.borrow().clone().expect("a check was made");
    assert_eq!(perm, SECRET_READ_PERMISSION);
    assert_eq!(
        SECRET_READ_PERMISSION, "read",
        "the FROZEN §5.2 secret.read permission (4.9)"
    );

    // FORK: the `!is_untrusted_fork` arm — 0 resolved, all withheld with the structural reason.
    let fork = broker
        .resolve(
            TrustTier::UntrustedFork,
            &subject("u:fork"),
            secret_object_of,
            &refs,
            &at(),
        )
        .expect("resolves");
    assert_eq!(
        fork.secret_count(),
        0,
        "the !is_untrusted_fork arm: 0 fork secret reads (4.9)"
    );
    assert!(fork.outcomes.iter().all(|o| matches!(
        o,
        SecretOutcome::Withheld {
            reason: WithholdReason::UntrustedFork,
            ..
        }
    )));
}

// ===========================================================================
// Row 9.4 — the durable approval signal: the per-effect idem_key (OQ-F).
// ===========================================================================

#[test]
fn cdc_9_4_deploy_gate_keys_on_the_frozen_per_effect_idem_key() {
    // The CONSUMER keys its deploy effects on the FROZEN §6.4 rule — a single-effect card on the bare
    // `card_id`, a multi-effect card on `card_id:idx`. This is the SAME `per_effect_idem_key`
    // myelin-flow's durable `wf_signal` dedups on (a double-delivery = one buffered row = one wake).
    assert_eq!(per_effect_idem_key("dep-card", 0, 1), "dep-card");
    assert_eq!(per_effect_idem_key("dep-card", 0, 3), "dep-card:0");
    assert_eq!(per_effect_idem_key("dep-card", 2, 3), "dep-card:2");

    // The CONSUMER maps the FROZEN myelin-flow `EffectOutcome` onto the deploy domain (the seam to the
    // durable `apply_approved_effects` loop in the ci.pipeline body).
    assert_eq!(
        deploy_outcome_of(&EffectOutcome::Applied("dep-7".into())),
        DeployGateOutcome::Approved("dep-7".into())
    );
    assert_eq!(
        deploy_outcome_of(&EffectOutcome::Withheld("decline".into())),
        DeployGateOutcome::Withheld("decline".into())
    );

    // The double-click invariant the durable signal guarantees (per-effect idem_key dedup): one apply.
    let mut applied: HashMap<String, String> = HashMap::new();
    let mut applies = 0;
    for _ in 0..3 {
        DeployGate::gate_deploy(
            "dep-card",
            0,
            1,
            ApprovalDecision::Approve,
            &mut applied,
            || {
                applies += 1;
                "dep-7".to_string()
            },
        );
    }
    assert_eq!(
        applies, 1,
        "three deliveries of the SAME per-effect key = ONE apply (9.4 / OQ-F)"
    );
}
