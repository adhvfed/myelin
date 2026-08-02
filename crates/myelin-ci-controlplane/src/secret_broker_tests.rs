//! Unit tests for the in-boundary secret broker (CI-P24 / P-367) — the fork-no-secrets boundary
//! (CI-D7) + the trusted-tier DIRECT-NARROW scope + the OIDC short-lived mint.
//!
//! These are the security-load-bearing (mandatory-core) tests; they pin every branch of the broker so
//! the `cargo-mutants` ≥ 90% floor (the module doc) is killable: the fork short-circuit, the
//! `Decision::Allow` gate, the per-job scope filter, and the OIDC fork-refusal.

use super::*;
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, ConsistencyMode, Credential, Decision,
    DelegationCaveats, EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService,
    ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission, Precondition,
    Principal, PrincipalId, PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId,
    RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::{BTreeMap, HashSet};

fn launch_spec(tier: TrustTier) -> JobSpec {
    use myelin_ci_sandbox::{
        EgressPolicy, IdemToken, ImageRef, JobKind, MeterTarget, ResourceLimits,
        RunTokenCredential, WorkspaceSpec,
    };

    JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned(format!("registry.example/job@sha256:{}", "a".repeat(64))).unwrap(),
        vec!["/bin/print-env".into()],
        Vec::new(),
        refs(),
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 * 1024 * 1024,
            disk_bytes: 1024 * 1024 * 1024,
            tmpfs_bytes: 64 * 1024 * 1024,
            pids_max: 64,
            timeout_secs: 30,
        },
        WorkspaceSpec::default(),
        tier,
        RunTokenCredential::new("ephemeral-bearer", "jti:secret-test", 60).unwrap(),
        MeterTarget {
            reserve_id: "reserve:secret-test".into(),
        },
        IdemToken("idem:secret-test".into()),
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// Test doubles (the FROZEN consumed surfaces — never a second real impl).
// ---------------------------------------------------------------------------

/// A secret capability that resolves any KNOWN handle to a deterministic material; an unknown handle
/// resolves to `None` (a stale/absent binding → withheld, never an error).
struct FakeCapability {
    known: HashSet<String>,
}
impl FakeCapability {
    fn with(handles: &[&str]) -> Self {
        FakeCapability {
            known: handles.iter().map(|h| h.to_string()).collect(),
        }
    }
}
impl SecretCapability for FakeCapability {
    fn resolve_handle(
        &self,
        tenant: &TenantId,
        object: &ArtifactRef,
        _binding_name: &str,
        handle: &str,
    ) -> Option<zeroize::Zeroizing<String>> {
        let parsed = parse_canonical_secret_handle(handle)?;
        if parsed.tenant == tenant.0 && object.0 == handle && self.known.contains(handle) {
            Some(zeroize::Zeroizing::new(format!("material:{handle}")))
        } else {
            None
        }
    }
}

#[test]
fn strict_secret_handles_round_trip_to_one_tenant_object_key() {
    let clean = "myelin://acme/ci/secret/deploy";
    let parsed = parse_canonical_secret_handle(clean).expect("clean handle must be accepted");
    assert_eq!(parsed.tenant, "acme");
    assert_eq!(parsed.id, "deploy");
    let key = myelin_refs::object_key(&ArtifactRef(clean.into())).unwrap();
    assert_eq!(key.tenant.as_deref(), Some("acme"));
    assert_eq!(key.tuple_key(), "secret:deploy");

    for refused in [
        "myelin://acme/ci/secret/secret:deploy",
        "myelin://acme/ci/secret/..",
        "myelin://acme/ci/secret/%2f",
        "myelin://acme/ci/secret/deploy\0",
        "junkmyelin://acme/ci/secret/deploy",
        "myelin://acme/ci/secret/deployjunk/",
        "myelin:///ci/secret/deploy",
        "myelin://acme/ci/secret/",
        "myelin://ac.me/ci/secret/deploy",
        "myelin://acme/ci/secret/de.ploy",
        "myelin://acme/ci/secret/deploy#anchor",
        "myelin://acme/ci/secret/deploy/extra",
        "myelin://acme/ci/secret/deploy\n",
    ] {
        assert!(
            parse_canonical_secret_handle(refused).is_none(),
            "noncanonical handle must be refused: {refused:?}"
        );
    }
}

/// An Identity gate that `Allow`s `read` ONLY for the secret objects in `granted` (the DIRECT NARROW
/// `secret#direct_reader` grant). Everything else is `Deny` (fail-closed). It also records every
/// `check` it received, so a test can prove a fork-tier resolution made ZERO authz calls (the
/// structural short-circuit).
struct FakeIdentity {
    granted: HashSet<String>,
    checks: std::sync::Mutex<Vec<String>>,
    subjects: std::sync::Mutex<Vec<String>>,
}
impl FakeIdentity {
    fn granting(objects: &[&str]) -> Self {
        FakeIdentity {
            granted: objects.iter().map(|o| o.to_string()).collect(),
            checks: std::sync::Mutex::new(Vec::new()),
            subjects: std::sync::Mutex::new(Vec::new()),
        }
    }
    fn check_count(&self) -> usize {
        self.checks.lock().unwrap().len()
    }

    fn checked_subjects(&self) -> Vec<String> {
        self.subjects.lock().unwrap().clone()
    }
}
impl IdentityService for FakeIdentity {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn check(
        &self,
        s: &Principal,
        p: &Permission,
        o: &ArtifactRef,
        _at: &Consistency,
        _cav: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        self.checks.lock().unwrap().push(o.0.clone());
        self.subjects
            .lock()
            .unwrap()
            .push(s.principal_id.0.clone());
        // The broker only ever asks for `read` on a secret object.
        assert_eq!(
            p.0, SECRET_READ_PERMISSION,
            "broker checks only `secret.read`"
        );
        if self.granted.contains(&o.0) {
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
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _pre: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _ttl: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
}

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

fn subject() -> Principal {
    Principal::stub(
        PrincipalId("u:dev".into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    )
}

fn at() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::Strong,
    }
}

/// The job's referenced secrets (NAMES + opaque handles — never material).
fn refs() -> Vec<SecretRef> {
    vec![
        SecretRef {
            name: "REGISTRY_TOKEN".into(),
            handle: "myelin://acme/ci/secret/registry".into(),
        },
        SecretRef {
            name: "DEPLOY_KEY".into(),
            handle: "myelin://acme/ci/secret/deploy".into(),
        },
    ]
}

/// Map a `SecretRef` to its `ci_secret` ArtifactRef (the gate object) — the per-job scope is the
/// handle, so the object id is derived from it (deterministic).
fn secret_object_of(r: &SecretRef) -> ArtifactRef {
    ArtifactRef(r.handle.clone())
}

// ---------------------------------------------------------------------------
// DEFENCE #1 — the structural fork short-circuit (CI-D7).
// ---------------------------------------------------------------------------

#[test]
fn fork_resolves_to_zero_secrets() {
    // A fork run whose subject WOULD be granted both secrets if it were trusted — the grant is a
    // misconfiguration the structural defence must survive.
    let cap = FakeCapability::with(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    let id = FakeIdentity::granting(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(
            TrustTier::UntrustedFork,
            &subject(),
            secret_object_of,
            &refs(),
            &at(),
        )
        .expect("resolution does not error");

    // The quantified CI-D7 gate: 0 secrets resolved by a fork-tier run.
    assert_eq!(
        res.secret_count(),
        0,
        "a fork resolves ZERO secrets (CI-D7)"
    );
    assert!(res.is_empty());
    assert!(!res.all_resolved());
    // Every referenced name is withheld with the structural reason.
    for o in &res.outcomes {
        assert!(matches!(
            o,
            SecretOutcome::Withheld {
                reason: WithholdReason::UntrustedFork,
                ..
            }
        ));
    }
    // The STRUCTURAL property: the fork never reached the authz gate at all (0 checks) — a
    // misconfigured grant cannot leak because a fork never asks.
    assert_eq!(
        id.check_count(),
        0,
        "a fork short-circuits BEFORE any authz check (the `!is_untrusted_fork` arm by construction)"
    );
}

// ---------------------------------------------------------------------------
// DEFENCE #2 — the trusted-tier DIRECT-NARROW authz gate.
// ---------------------------------------------------------------------------

#[test]
fn trusted_resolves_only_referenced_granted_names() {
    let cap = FakeCapability::with(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    // Grant ONLY the registry secret (not the deploy key) — the DIRECT NARROW grant.
    let id = FakeIdentity::granting(&["myelin://acme/ci/secret/registry"]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(
            TrustTier::Trusted,
            &subject(),
            secret_object_of,
            &refs(),
            &at(),
        )
        .expect("resolution does not error");

    // Exactly ONE secret resolves (the granted one); the ungranted one is withheld.
    assert_eq!(res.secret_count(), 1);
    let resolved: Vec<_> = res.resolved().collect();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].name, "REGISTRY_TOKEN");
    assert_eq!(
        resolved[0].value.as_str(),
        "material:myelin://acme/ci/secret/registry"
    );

    // The ungranted deploy key is withheld (NotGranted), never leaked.
    assert!(matches!(
        res.outcomes[1],
        SecretOutcome::Withheld {
            reason: WithholdReason::NotGranted,
            ..
        }
    ));
    // The scope is EXACTLY the 2 referenced names — the broker never enumerated the project.
    assert_eq!(
        id.check_count(),
        2,
        "the broker checks ONLY the referenced names (per-job scope)"
    );
}

#[test]
fn protected_without_grant_withholds_all() {
    let cap = FakeCapability::with(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    // No grants at all — a protected secret without an explicit DIRECT grant.
    let id = FakeIdentity::granting(&[]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(
            TrustTier::Trusted,
            &subject(),
            secret_object_of,
            &refs(),
            &at(),
        )
        .expect("resolution does not error");

    assert_eq!(
        res.secret_count(),
        0,
        "no grant → no secret (DIRECT NARROW, CI-1)"
    );
    assert!(res.outcomes.iter().all(|o| matches!(
        o,
        SecretOutcome::Withheld {
            reason: WithholdReason::NotGranted,
            ..
        }
    )));
}

#[test]
fn granted_but_stale_handle_withholds_observably() {
    // The subject IS granted, but the handle does not resolve (a stale binding) — withheld, never a
    // panic or a silent leak.
    let cap = FakeCapability::with(&[]); // no handle resolves
    let id = FakeIdentity::granting(&["myelin://acme/ci/secret/registry"]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(
            TrustTier::Trusted,
            &subject(),
            secret_object_of,
            &refs(),
            &at(),
        )
        .expect("resolution does not error");

    assert_eq!(res.secret_count(), 0);
    assert!(matches!(
        res.outcomes[0],
        SecretOutcome::Withheld {
            reason: WithholdReason::NotGranted,
            ..
        }
    ));
}

#[test]
fn cross_tenant_handle_cannot_use_an_authorized_local_object() {
    let victim_handle = "myelin://victim/ci/secret/deploy";
    let cap = FakeCapability::with(&[victim_handle]);
    let id = FakeIdentity::granting(&["myelin://acme/ci/secret/deploy"]);
    let broker = SecretBroker::new(&cap, &id);
    let forged = vec![SecretRef {
        name: "DEPLOY_KEY".into(),
        handle: victim_handle.into(),
    }];

    let resolution = broker
        .resolve(
            TrustTier::Trusted,
            &subject(),
            |_| ArtifactRef("myelin://acme/ci/secret/deploy".into()),
            &forged,
            &at(),
        )
        .unwrap();

    assert_eq!(resolution.secret_count(), 0);
    assert!(matches!(
        resolution.outcomes[0],
        SecretOutcome::Withheld {
            reason: WithholdReason::NotGranted,
            ..
        }
    ));
    assert_eq!(
        id.check_count(),
        0,
        "scope mismatch is refused before authz"
    );
}

#[test]
fn capability_resolution_refuses_foreign_handle_for_authorized_local_object() {
    let foreign = "myelin://victim/ci/secret/deploy";
    let cap = FakeCapability::with(&[foreign]);
    assert_eq!(
        cap.resolve_handle(
            &TenantId("acme".into()),
            &ArtifactRef("myelin://acme/ci/secret/deploy".into()),
            "DEPLOY_KEY",
            foreign,
        ),
        None,
        "the capability interface must tenant-bind the requested handle to the authorized object"
    );
}

#[test]
fn self_hosted_is_trusted_for_secret_resolution() {
    // A self-hosted member run is trusted CODE — it resolves its granted secrets (it is NOT a fork).
    let cap = FakeCapability::with(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    let id = FakeIdentity::granting(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(
            TrustTier::SelfHosted,
            &subject(),
            secret_object_of,
            &refs(),
            &at(),
        )
        .expect("resolution does not error");

    assert!(
        res.all_resolved(),
        "a self-hosted run resolves its granted secrets"
    );
    assert_eq!(res.secret_count(), 2);
    assert!(
        id.check_count() > 0,
        "self-hosted goes THROUGH the authz gate (it is not a fork)"
    );
}

#[test]
fn empty_refs_resolve_to_empty() {
    // A job referencing no secrets resolves to an empty (vacuously 0) resolution.
    let cap = FakeCapability::with(&[]);
    let id = FakeIdentity::granting(&[]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(TrustTier::Trusted, &subject(), secret_object_of, &[], &at())
        .expect("resolution does not error");

    assert_eq!(res.secret_count(), 0);
    assert!(res.is_empty());
    assert!(
        res.all_resolved(),
        "vacuously all-resolved when there are no refs"
    );
}

#[test]
fn resolve_for_launch_couples_every_resolved_value_to_sandbox_injection() {
    let cap = FakeCapability::with(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    let id = FakeIdentity::granting(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    let broker = SecretBroker::new(&cap, &id);

    let spec = broker
        .resolve_for_launch(
            launch_spec(TrustTier::Trusted),
            &subject(),
            secret_object_of,
            &at(),
        )
        .expect("all granted handles resolve into one covered launch value");

    assert_eq!(spec.resolved_secret_count(), 2);
    assert!(spec.validate_secret_coverage().is_ok());
    let rendered = format!("{spec:?}");
    assert!(!rendered.contains("material:myelin://acme/ci/secret/registry"));
    assert!(!rendered.contains("material:myelin://acme/ci/secret/deploy"));
}

#[test]
fn production_secret_resolver_calls_broker_with_claim_bound_subject() {
    use myelin_ci_sandbox::{CiJobAuthorizationContext, RunTokenAuthorizationContext};
    use std::sync::Arc;

    let cap = Arc::new(FakeCapability::with(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]));
    let id = Arc::new(FakeIdentity::granting(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]));
    let resolver = crate::secret_broker_ci_job_resolver(cap, id.clone(), at());
    let mut spec = launch_spec(TrustTier::Trusted);
    spec.run_token_authorization = Some(RunTokenAuthorizationContext::CiJob(
        CiJobAuthorizationContext {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            principal_id: "ci-job".into(),
            project_id: "11111111-1111-4111-8111-111111111111".into(),
            wf_run_id: "wf".into(),
            job_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
            lease_owner: "runner".into(),
            lease_epoch: 1,
            claim_nonce: "nonce".into(),
            claim_started_at_epoch_secs: 1,
            claim_expires_at_epoch_secs: 2,
            reserve_id: "reserve:secret-test".into(),
            required_capabilities: Vec::new(),
            checkout_scope: None,
            credential_binding: None,
        },
    ));

    let resolved = resolver(&TenantId::from_token("acme"), spec).unwrap();
    assert_eq!(resolved.resolved_secret_count(), 2);
    assert!(resolved.validate_secret_coverage().is_ok());
    assert_eq!(id.check_count(), 2);
    assert_eq!(
        id.checked_subjects(),
        vec![
            "svc:ci:project:11111111-1111-4111-8111-111111111111:job:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
            2
        ],
        "the resolver must never authorize secrets as tenant-global svc:ci"
    );
}

#[test]
fn injected_material_is_absent_from_durable_ci_records_and_result_records() {
    use crate::ci_drive_manifest::{
        CiDriveManifestV1, CiManifestLaneV1, CiManifestLimitsV1, CiManifestSchedulingV1,
        CiManifestTrustTierV1, CiManifestWorkspaceV1, GrantedCiJobV1,
    };
    use crate::ci_run_store::CiRunInsert;
    use crate::job_spec_store::DurableCiJobLaunchTemplate;
    use myelin_ci_sandbox::{ResourceUsage, SandboxResult};

    let material = "material:myelin://acme/ci/secret/registry";
    let cap = FakeCapability::with(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    let id = FakeIdentity::granting(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    let broker = SecretBroker::new(&cap, &id);
    let injected = broker
        .resolve_for_launch(
            launch_spec(TrustTier::Trusted),
            &subject(),
            secret_object_of,
            &at(),
        )
        .unwrap();

    let (template, _credential) = injected.into_template();
    let durable_spec = DurableCiJobLaunchTemplate {
        spec: template,
        project_id: "55555555-5555-4555-8555-555555555555".into(),
        ci_run_id: "44444444-4444-8444-8444-444444444444".into(),
        token_authority_handle: "mint:job".into(),
    };
    let durable_spec_json = serde_json::to_string(&durable_spec).unwrap();

    let manifest = CiDriveManifestV1 {
        schema_version: 1,
        tenant_id: "acme".into(),
        region: "fr-par".into(),
        project_id: "55555555-5555-4555-8555-555555555555".into(),
        wf_run_id: "33333333-3333-8333-8333-333333333333".into(),
        ci_run_id: "44444444-4444-8444-8444-444444444444".into(),
        source_snapshot_ref: format!(
            "myelin://acme/ci/artifact/snapshot-blake3:{}",
            "b".repeat(64)
        ),
        source_plan_schema_version: 2,
        launch_request_digest: "c".repeat(64),
        workflow_type: crate::CI_PIPELINE_WF_TYPE.into(),
        workflow_definition_version: 1,
        workflow_code_hash: "d".repeat(64),
        authority_policy_revision: "policy:v1".into(),
        repo_ref: "myelin://acme/git/repo/core".into(),
        commit_oid: "deadbeef".into(),
        run_ref: "myelin://acme/ci/run/44444444-4444-8444-8444-444444444444".into(),
        started_at: "2026-08-02T00:00:00Z".into(),
        trust_tier: CiManifestTrustTierV1::Trusted,
        check_attempts: BTreeMap::from([("build".into(), 1)]),
        merge_waiter: None,
        jobs: vec![GrantedCiJobV1 {
            job_id: "11111111-1111-8111-8111-111111111111".into(),
            stage: "build".into(),
            name: "build".into(),
            check_context: "build".into(),
            needs: Vec::new(),
            matrix_key: BTreeMap::new(),
            image: format!("registry.example/job@sha256:{}", "a".repeat(64)),
            command: vec!["/bin/print-env".into()],
            env: BTreeMap::new(),
            secret_handles: BTreeMap::from([
                ("DEPLOY_KEY".into(), "myelin://acme/ci/secret/deploy".into()),
                (
                    "REGISTRY_TOKEN".into(),
                    "myelin://acme/ci/secret/registry".into(),
                ),
            ]),
            egress_allow: Vec::new(),
            limits: CiManifestLimitsV1 {
                cpu_millis: 1000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                pids_max: 64,
                timeout_secs: 30,
            },
            workspace: CiManifestWorkspaceV1 {
                repo_ref: "myelin://acme/git/repo/core".into(),
                commit_oid: "deadbeef".into(),
                read_only_root: true,
                tmpfs_scratch: true,
            },
            scheduling: CiManifestSchedulingV1 {
                lane: CiManifestLaneV1::Batch,
                labels: Vec::new(),
                concurrency_group: None,
                fair_key: "project:core".into(),
            },
            reserve_handle: "reserve:secret-test".into(),
            token_authority_handle: "mint:job".into(),
            continue_on_error: false,
        }],
    };
    let manifest_json = serde_json::to_string(&manifest).unwrap();
    let ci_run = CiRunInsert {
        tenant_id: "acme".into(),
        region: "fr-par".into(),
        run_id: "44444444-4444-8444-8444-444444444444".into(),
        project_id: "55555555-5555-8555-8555-555555555555".into(),
        pipeline_id: "66666666-6666-8666-8666-666666666666".into(),
        wf_run_id: "33333333-3333-8333-8333-333333333333".into(),
        definition_snapshot: "myelin://acme/ci/artifact/snapshot".into(),
        trigger_kind: "push".into(),
        concurrency_group: None,
        pr_head_generation: None,
        trust_tier: "trusted".into(),
        state: "queued".into(),
        correlation_id: "correlation".into(),
        cause_event_id: None,
        cause_depth: 0,
        caused_by: None,
        repo_ref: Some("myelin://acme/git/repo/core".into()),
        commit_oid: Some("deadbeef".into()),
        triggered_by: None,
    };
    let result = SandboxResult::stub_ok(ResourceUsage {
        cpu_seconds: 1,
        mem_byte_seconds: 1,
    });

    for record in [
        durable_spec_json,
        manifest_json,
        format!("{ci_run:?}"),
        format!("{result:?}"),
    ] {
        assert!(!record.contains(material));
        assert!(!record.contains("material:myelin://acme/ci/secret/deploy"));
    }
}

#[test]
fn withheld_secret_rejects_launch_with_observable_reason() {
    let cap = FakeCapability::with(&[
        "myelin://acme/ci/secret/registry",
        "myelin://acme/ci/secret/deploy",
    ]);
    let id = FakeIdentity::granting(&["myelin://acme/ci/secret/registry"]);
    let broker = SecretBroker::new(&cap, &id);

    let error = broker
        .resolve_for_launch(
            launch_spec(TrustTier::Trusted),
            &subject(),
            secret_object_of,
            &at(),
        )
        .expect_err("partial secret resolution must reject the whole launch");

    assert_eq!(
        error.to_string(),
        "secret launch withheld: DEPLOY_KEY=not_granted"
    );
    assert!(matches!(
        error,
        SecretLaunchError::Withheld(ref withheld)
            if withheld == &[WithheldSecret {
                name: "DEPLOY_KEY".into(),
                reason: WithholdReason::NotGranted,
            }]
    ));
}

// ---------------------------------------------------------------------------
// OIDC short-lived audience-scoped credentials (contract 4.7).
// ---------------------------------------------------------------------------

#[test]
fn trusted_mints_audience_scoped_oidc() {
    let cap = FakeCapability::with(&[]);
    let id = FakeIdentity::granting(&[]);
    let broker = SecretBroker::new(&cap, &id);

    let cred = broker
        .mint_oidc(TrustTier::Trusted, "registry.fr-par", 900, |aud, ttl| {
            Some(format!("oidc:{aud}:{ttl}"))
        })
        .expect("a trusted run mints an OIDC credential");

    assert_eq!(cred.audience, "registry.fr-par");
    assert_eq!(cred.ttl_secs, 900);
    assert_eq!(cred.token, "oidc:registry.fr-par:900");
}

#[test]
fn fork_is_refused_an_oidc_credential() {
    let cap = FakeCapability::with(&[]);
    let id = FakeIdentity::granting(&[]);
    let broker = SecretBroker::new(&cap, &id);

    // The mint closure would succeed — but a fork never reaches it (the same boundary).
    let cred = broker.mint_oidc(
        TrustTier::UntrustedFork,
        "registry.fr-par",
        900,
        |aud, ttl| Some(format!("oidc:{aud}:{ttl}")),
    );
    assert!(
        cred.is_none(),
        "a fork gets NO audience-scoped cloud credential (CI-D7)"
    );
}

#[test]
fn withhold_reason_tokens_are_stable() {
    assert_eq!(WithholdReason::UntrustedFork.as_token(), "untrusted_fork");
    assert_eq!(WithholdReason::NotGranted.as_token(), "not_granted");
    assert_eq!(
        WithholdReason::CapabilityUnavailable.as_token(),
        "capability_unavailable"
    );
}

#[test]
fn debug_output_redacts_secret_and_oidc_material_recursively() {
    let material = "unique-static-secret-material";
    let token = "unique-short-lived-oidc-token";
    let resolved = ResolvedSecret {
        name: "DEPLOY_KEY".into(),
        value: zeroize::Zeroizing::new(material.to_owned()),
    };
    let resolution = SecretResolution {
        outcomes: vec![SecretOutcome::Resolved(resolved.clone())],
    };
    let credential = OidcCredential {
        audience: "registry.fr-par".into(),
        token: token.into(),
        ttl_secs: 900,
    };

    for rendered in [
        format!("{resolved:?}"),
        format!("{:?}", resolution.outcomes[0]),
        format!("{resolution:?}"),
        format!("{credential:?}"),
    ] {
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains(material));
        assert!(!rendered.contains(token));
    }
}
