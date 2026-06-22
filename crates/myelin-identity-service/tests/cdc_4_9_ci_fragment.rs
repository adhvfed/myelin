//! # The CDC pair for contract 4.9 — Id's compiled **CI** ReBAC fragment (P-ID-27 / P-320)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment — each subsystem declares
//! relations and permissions, compiled into ONE cell schema; Identity owns the engine, the
//! admit-contract, and the core hierarchy and never invents object ids). The engine half is pinned by
//! `cdc_4_9_namespace_engine.rs` (P-068); the Git fragment by `cdc_4_9_git_fragment.rs` (P-247); the
//! Knowledge fragment by `cdc_4_9_knowledge_fragment.rs` (P-249). THIS file pins the Identity-side
//! compiled CI fragment (the rich rewrites Id owns, P-ID-27 / P-320).
//!
//! - The **PROVIDER** is Identity's namespace engine ([`StoreBackedCheck`] over `with_core_hierarchy`):
//!   it admits Id's compiled Git + CI [`FragmentDef`]s, resolves the CI permissions through the four
//!   userset operators, and never invents an id.
//! - The **CONSUMER** is the CI subsystem, which gates an action ONLY on a resolved grant — modelled
//!   here as the action gates run through the 4.2 `check` surface (`run.view`, `run.trigger`,
//!   `run.read`, `secret.read`).
//!
//! The two sides are pinned together: Id's compiled fragment ([`myelin_identity_service::ci_fragment`])
//! must agree byte-for-byte on the relation/permission NAMES with the CI subsystem's names-only carrier
//! — but `myelin-identity-service` does NOT depend on a CI leaf crate (the DAG floor), so the
//! name-agreement is asserted against the architecture §5 frozen vocabulary here.
//!
//! **The two security-critical invariants this CDC behaviourally pins (CI-D10 fragment side):**
//! - **`secret.read` is NON-inherited (CI-1, §1):** a `ci_project` reader/admin gets NO secret read; a
//!   secret is reachable ONLY by a direct `secret#direct_reader@subject` grant.
//! - **`run.read = run.view − is_untrusted_fork` (C7, §X-1):** a stamped untrusted-fork run's output is
//!   gated by construction even for a subject who can `view` the run.

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, ObjectId, Permission,
    Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore, CI_READ, CI_TRIGGER, CI_VIEW,
    IS_UNTRUSTED_FORK, SECRET_DIRECT_READER,
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

/// The PROVIDER surface seeded with `tuples` — the core hierarchy is preloaded, then **both** Id's
/// compiled Git fragment (so `parent_repo->pull`/`push` resolves for `run`) AND the CI fragment are
/// admitted on top.
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
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env);
    }
    let svc = StoreBackedCheck::with_index(store, index);
    // The Git fragment first (the CI `run.view = parent_repo->pull` inheritance terminates on the Git
    // repo's compiled `pull`/`push`), then the CI fragment.
    for admit in svc.admit_git_fragment() {
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
    }
    for admit in svc.admit_ci_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Id's compiled CI fragment admits: {admit:?}"
        );
    }
    svc
}

/// **PROVIDER → the compiled CI fragment ADMITS into the cell schema (the engine-only-floor
/// progression).** Id declares + compiles its CI fragment via the fragment-admit contract; every CI
/// object type admits on top of the core hierarchy + Git fragment.
#[test]
fn cdc_4_9_id_compiled_ci_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);
    let ns = svc.namespace();
    for ty in ["ci_project", "environment", "secret", "run"] {
        assert!(
            ns.object_types().contains(&ty.to_string()),
            "`{ty}` admitted into the cell schema"
        );
    }
    assert!(ns.resolve_permission("secret", CI_READ).is_some());
    assert!(ns.resolve_permission("run", CI_READ).is_some());
    assert!(ns.resolve_permission("run", CI_VIEW).is_some());
}

/// **CONSUMER → PROVIDER: `run.view = parent_repo->pull` / `run.trigger = parent_repo->push` resolve
/// through the engine (§5).** A repo puller can view a run; a repo pusher can trigger it; an outsider
/// is denied (fail-closed).
#[test]
fn cdc_4_9_run_view_and_trigger_inherit_the_repo() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // The Git repo grants: alice can pull (reader), bob can push (writer).
            add("repo:core", "reader", "p:alice"),
            add("repo:core", "writer", "p:bob"),
            // run:99 belongs to repo:core (parent_repo) — run.view/trigger inherit the repo's pull/push.
            add("run:99", "parent_repo", "repo:core#pull"),
            add("run:99", "parent_repo", "repo:core#push"),
        ],
    );
    let run = ArtifactRef("run:99".into());
    let can = |actor: &Principal, p: &str| {
        matches!(
            svc.check(actor, &Permission(p.into()), &run, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can(&subject("p:alice"), CI_VIEW),
        "a repo puller views the run (run.view = parent_repo->pull)"
    );
    // bob (writer) can push → can trigger; a writer can also pull → can view.
    assert!(
        can(&subject("p:bob"), CI_TRIGGER),
        "a repo pusher triggers the run (run.trigger = parent_repo->push)"
    );
    assert!(
        !can(&subject("p:carol"), CI_VIEW),
        "an outsider cannot view the run (fail-closed)"
    );
    assert!(
        !can(&subject("p:alice"), CI_TRIGGER),
        "a mere puller cannot trigger (push-only, §5)"
    );
}

/// **`secret.read` is NON-inherited (CI-1, §1) — a project reader/admin gets NO secret read; only a
/// DIRECT grant reads a secret.** This is the headline CI-D10 fragment-side invariant.
#[test]
fn cdc_4_9_secret_read_is_not_inherited_from_the_project() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // alice is a CI-project ADMIN (so she can `administer` / `view` the project).
            add("ci_project:web", "admin", "p:alice"),
            // The secret belongs to the project (a lifecycle/listing relation) — but read does NOT
            // inherit from it (CI-1). alice has NO direct secret grant.
            add(
                "secret:db-password",
                "parent_ci_project",
                "ci_project:web#view",
            ),
            // bob has a DIRECT secret read grant (the ONLY path to a secret).
            add("secret:db-password", SECRET_DIRECT_READER, "p:bob"),
        ],
    );
    let secret = ArtifactRef("secret:db-password".into());
    let can_read = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission(CI_READ.into()),
                &secret,
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    // Sanity: alice really CAN administer the project (the inheritance edge IS present)…
    assert!(
        matches!(
            svc.check(
                &subject("p:alice"),
                &Permission("administer".into()),
                &ArtifactRef("ci_project:web".into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        ),
        "alice administers the CI project (the project edge resolves)"
    );
    // …but she CANNOT read the secret — secret.read is the direct narrow relation, NOT inherited.
    assert!(
        !can_read(&subject("p:alice")),
        "a CI-project ADMIN cannot read a secret via project inheritance (CI-1 secret-non-inheritance)"
    );
    // bob, with the DIRECT grant, reads it (the only path).
    assert!(
        can_read(&subject("p:bob")),
        "a DIRECT secret#direct_reader grant reads the secret (the only path, CI-1)"
    );
    assert!(
        !can_read(&subject("p:carol")),
        "an outsider cannot read the secret (fail-closed)"
    );
}

/// **`run.read = run.view − is_untrusted_fork` (the `read & !is_untrusted_fork` ABAC edge, C7).** A
/// subject who can `view` a run reads its output UNLESS the run is stamped `is_untrusted_fork` (a fork
/// PR / untrusted contributor code), in which case the Exclusion gates them by construction.
#[test]
fn cdc_4_9_run_read_is_gated_by_the_is_untrusted_fork_edge() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // alice can pull the repo → can view both runs.
            add("repo:core", "reader", "p:alice"),
            // A TRUSTED run (no fork stamp): alice can both view AND read its output.
            add("run:trusted", "parent_repo", "repo:core#pull"),
            // An UNTRUSTED-FORK run: alice can VIEW it, but CI stamped her out of `read` via the edge.
            add("run:fork", "parent_repo", "repo:core#pull"),
            add("run:fork", IS_UNTRUSTED_FORK, "p:alice"),
        ],
    );
    let trusted = ArtifactRef("run:trusted".into());
    let fork = ArtifactRef("run:fork".into());
    let chk = |actor: &Principal, obj: &ArtifactRef, p: &str| {
        matches!(
            svc.check(actor, &Permission(p.into()), obj, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    // The trusted run: alice views AND reads.
    assert!(
        chk(&subject("p:alice"), &trusted, CI_VIEW),
        "views trusted run"
    );
    assert!(
        chk(&subject("p:alice"), &trusted, CI_READ),
        "reads a trusted run's output (view − ∅)"
    );
    // The untrusted-fork run: alice still VIEWS it (provenance is visible)…
    assert!(
        chk(&subject("p:alice"), &fork, CI_VIEW),
        "views the fork run (run.view is unconditional)"
    );
    // …but is GATED from reading its output by the !is_untrusted_fork edge (the ABAC exclusion).
    assert!(
        !chk(&subject("p:alice"), &fork, CI_READ),
        "an untrusted-fork run's output is gated by construction (read = view − is_untrusted_fork, C7)"
    );
}
