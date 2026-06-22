//! # The CDC pair for contract 4.9 — Id's compiled **Git** ReBAC fragment (P-ID-24 / P-247)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment — each subsystem declares
//! relations and permissions, compiled into ONE cell schema; Identity owns the engine, the
//! admit-contract, and the core hierarchy and never invents object ids). The engine half is pinned by
//! `cdc_4_9_namespace_engine.rs` (P-068); the Git names-only carrier is pinned on the Git side, in
//! `myelin-git/tests/cdc_4_9_git_fragment.rs` (GIT-P1). THIS file pins the Identity-side compiled Git
//! fragment (the rich rewrites Id owns, P-ID-24).
//!
//! - The **PROVIDER** is Identity's namespace engine ([`StoreBackedCheck`] over `with_core_hierarchy`):
//!   it admits Id's compiled Git [`FragmentDef`]s, resolves the Git permissions through the four
//!   userset operators, and never invents an id.
//! - The **CONSUMER** is the Git subsystem, which gates an action ONLY on a resolved grant — modelled
//!   here as the action gates run through the 4.2 `check` surface (`pull`, `protected_push`, `merge`,
//!   the X-1 `approve_untrusted_ci` endorsement) + the 4.3 `list_objects` PR/repo conjoin.
//!
//! The two sides are pinned together: Id's compiled fragment ([`myelin_identity_service::git_fragment`])
//! must agree byte-for-byte on the relation/permission NAMES with the Git subsystem's names-only
//! carrier — but `myelin-identity-service` does NOT depend on `myelin-git` (the DAG floor), so the
//! name-agreement is asserted against the architecture §5 frozen vocabulary here, and the Git-side
//! CDC asserts the SAME names from its end (a drift on either side fails its own CI job).

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, ObjectId, Permission,
    Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    git_fragment, ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
    APPROVE_UNTRUSTED_CI, PROTECTED_PUSH,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn subject(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

/// The PROVIDER surface seeded with `tuples` (the core hierarchy is preloaded so `parent_project->view`
/// inheritance has its parent type; Id's compiled Git fragment is admitted on top).
fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-20T00:00:00Z".into()),
        )
        .expect("seed tuples");
    // Feed the S8 reverse index off the bus (the live projection list_subjects / list_objects read).
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env);
    }
    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_git_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Id's compiled Git fragment admits: {admit:?}"
        );
    }
    svc
}

/// **PROVIDER → the compiled Git fragment ADMITS into the cell schema (the engine-only-floor
/// progression).** Id declares + compiles its Git fragment via the fragment-admit contract; every Git
/// object type admits on top of the core hierarchy.
#[test]
fn cdc_4_9_id_compiled_git_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);
    // The four Git types are now in the compiled vocabulary (admitted, not just declared).
    let ns = svc.namespace();
    for ty in ["repo", "ref", "pull_request", "pr_comment"] {
        assert!(
            ns.object_types().contains(&ty.to_string()),
            "`{ty}` admitted into the cell schema"
        );
    }
    // protected_push (admin-only) + merge (parent_repo->protected_push) are compiled permissions.
    assert!(ns.resolve_permission("repo", PROTECTED_PUSH).is_some());
    assert!(ns.resolve_permission("pull_request", "merge").is_some());
}

/// **CONSUMER → PROVIDER: the Git rewrites resolve through the four operators (a real action gate).**
/// A direct repo admin pulls; a project member inherits via `parent_project->view`; an outsider is
/// denied (fail-closed). `protected_push` is admin-only — the tighter merge/protected-ref gate.
#[test]
fn cdc_4_9_git_rewrites_resolve_through_the_engine() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("repo:core", "admin", "p:alice"),
            // repo:core inherits from project:web (parent_project->view); carol is a project reader.
            add("repo:core", "parent_project", "project:web#view"),
            add("project:web", "reader", "p:carol"),
        ],
    );
    let repo = ArtifactRef("repo:core".into());
    let can = |actor: &Principal, p: &str| {
        matches!(
            svc.check(actor, &Permission(p.into()), &repo, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can(&subject("p:alice"), "pull"),
        "a direct repo admin pulls"
    );
    assert!(
        can(&subject("p:carol"), "pull"),
        "a project member inherits repo pull (parent_project->view)"
    );
    assert!(
        !can(&subject("p:bob"), "pull"),
        "an outsider cannot pull (fail-closed)"
    );
    // protected_push = admin only: alice yes, carol (mere project reader) no.
    assert!(
        can(&subject("p:alice"), PROTECTED_PUSH),
        "admin → protected_push"
    );
    assert!(
        !can(&subject("p:carol"), PROTECTED_PUSH),
        "a project reader does NOT get protected_push (admin-only, §5)"
    );
}

/// **`pull_request.merge = parent_repo->protected_push` resolves end-to-end (§5).** A PR whose parent
/// repo's protected_push the actor holds (admin) can merge; a non-admin cannot.
#[test]
fn cdc_4_9_pull_request_merge_via_protected_push() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("repo:core", "admin", "p:alice"),
            // pr:42 belongs to repo:core (parent_repo); merge = parent_repo->protected_push.
            add("pull_request:42", "parent_repo", "repo:core#protected_push"),
            // bob is only a writer (not admin) — no protected_push.
            add("repo:core", "writer", "p:bob"),
        ],
    );
    let pr = ArtifactRef("pull_request:42".into());
    let can_merge = |actor: &Principal| {
        matches!(
            svc.check(actor, &Permission("merge".into()), &pr, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can_merge(&subject("p:alice")),
        "a repo admin can merge (parent_repo->protected_push)"
    );
    assert!(
        !can_merge(&subject("p:bob")),
        "a writer cannot merge (protected_push is admin-only, §5)"
    );
}

/// **`approve_untrusted_ci` is a plain relation `check` (X-1, C7).** The fork-endorsement gate is
/// `check(subject, approve_untrusted_ci, repo)` — an ordinary direct-relation check, not bespoke
/// logic; Identity never recomputes the CI trust_tier.
#[test]
fn cdc_4_9_approve_untrusted_ci_is_a_plain_relation_check() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[add("repo:core", APPROVE_UNTRUSTED_CI, "p:maintainer")],
    );
    let repo = ArtifactRef("repo:core".into());
    let endorse = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission(APPROVE_UNTRUSTED_CI.into()),
                &repo,
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        endorse(&subject("p:maintainer")),
        "a maintainer endorses an untrusted fork run"
    );
    assert!(
        !endorse(&subject("p:bob")),
        "an outsider cannot endorse (X-1, fail-closed)"
    );
}

/// **A CODEOWNERS path-glob compiles to reviewer-requirement tuples that resolve as a relation.**
/// Compile a CODEOWNERS rule into `ref.code_owner` tuples, write them, and confirm "who must approve
/// this path" is an ordinary `list_subjects(ref, code_owner)` Expand — not a bespoke check.
#[test]
fn cdc_4_9_codeowners_glob_compiles_to_resolvable_reviewer_tuples() {
    let s = scope("acme");
    // Compile a CODEOWNERS rule (path-glob → owners) into ref.code_owner tuples.
    let rules = vec![git_fragment::CodeownersRule {
        path_glob: "/src/payments/**".into(),
        owners: vec![
            PrincipalId("p:alice".into()),
            PrincipalId("team:payments".into()),
        ],
    }];
    let tuples = git_fragment::compile_codeowners("repo:core", &rules);
    assert_eq!(tuples.len(), 2, "two owners → two code_owner tuples");

    // The Git subsystem WRITES these reviewer-requirement tuples through the ordinary write path.
    let deltas: Vec<TupleDelta> = tuples.iter().cloned().map(TupleDelta::Add).collect();
    let svc = provider(&s, &deltas);

    // "Who must approve this path" is list_subjects(ref, code_owner) — an ordinary Expand, member
    // density, not a bespoke CODEOWNERS resolver.
    let ref_obj = ObjectId("ref:repo:core::/src/payments/**".into());
    let owners = svc.list_subjects_in(&s, &ref_obj, &Permission("code_owner".into()), &at_latest());
    let members: Vec<&str> = owners.members.iter().map(|m| m.0.as_str()).collect();
    assert!(
        members.contains(&"p:alice"),
        "alice is a required reviewer for the path"
    );
    assert!(
        members.contains(&"team:payments"),
        "team:payments is a required reviewer for the path"
    );
}
