//! # The CDC pair for contract 4.9 — the ReBAC namespace engine + the fragment-admit contract
//! (the engine half) (P-ID-10 / P-068)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment — each subsystem declares
//! relations + permissions, compiled into ONE cell schema; Id owns the engine and never invents
//! object ids). This is the dedicated provider+consumer pair the P-ID-10 TESTS field names — the
//! focused, in-CI evidence that the two sides of the fragment-admit + permission-resolution seam
//! cannot drift apart:
//!
//! - the **PROVIDER** ([`NamespaceEngine`] via the [`StoreBackedCheck`] surface) admits a
//!   well-formed fragment (`Admitted{fragment_id}`), rejects a malformed one (`Rejected{reason}`),
//!   and resolves a **compiled permission** (the four userset operators) through `check` over the
//!   raw S3 tuples — never inventing an object id;
//! - the **CONSUMER** is a **subsystem declaring its namespace fragment at build time** and then
//!   gating an action on the resolved permission — exactly the shape every subsystem fragment
//!   (Git / CI / Issues / Knowledge / Chat, the M3/M4 follow-on) uses: declare relations +
//!   permissions, admit them, then `check(actor, permission, object)` and proceed ONLY on `Allow`.
//!
//! The provider's promise (a well-formed fragment admits + its permission resolves through the four
//! operators; a malformed fragment is rejected loudly) and the consumer's promise (it declares its
//! fragment, admits it, and gates iff the permission resolves to a grant) are pinned here so a
//! change to either side fails this test in the same CI job. The five per-subsystem fragments are
//! the M3/M4 follow-on (P-ID-24..P-ID-30, closing the engine-only floor); this pair is the M1
//! engine+admit-contract CDC the prompt requires.

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, ObjectId, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    FragmentDef, NamespaceEngine, PermissionRule, StoreBackedCheck, TupleStore, Userset,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
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

/// The PROVIDER: a [`StoreBackedCheck`] surface (the engine + the core hierarchy) seeded with
/// `tuples`, exposing `admit_fragment` (the contract-4.9 ABI) + the permission-aware `check`.
fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed tuples");
    StoreBackedCheck::new(store)
}

/// **The 4.9 provider promise: the engine admits a well-formed fragment + resolves its compiled
/// permission through the four operators; it rejects a malformed one — never inventing an id.**
#[test]
fn cdc_4_9_provider_admits_well_formed_rejects_malformed() {
    let mut eng = NamespaceEngine::new();

    // A well-formed fragment (a `wiki_page` type: reader/editor relations; read = reader ∪ editor).
    let ok = FragmentDef {
        object_type: ObjectType("wiki_page".into()),
        relations: vec![RelName("reader".into()), RelName("editor".into())],
        permissions: vec![PermissionRule {
            permission: Permission("read".into()),
            rewrite: Userset::Union(vec![
                Userset::Relation(RelName("reader".into())),
                Userset::Relation(RelName("editor".into())),
            ]),
        }],
    };
    assert!(
        matches!(eng.admit(&ok), FragmentAdmit::Admitted { fragment_id } if fragment_id == "wiki_page"),
        "a well-formed fragment admits"
    );

    // A malformed fragment (a permission referencing an UNDECLARED relation) is rejected loudly.
    let bad = FragmentDef {
        object_type: ObjectType("ledger".into()),
        relations: vec![RelName("owner".into())],
        permissions: vec![PermissionRule {
            permission: Permission("read".into()),
            rewrite: Userset::Relation(RelName("auditor".into())), // not declared
        }],
    };
    assert!(
        matches!(eng.admit(&bad), FragmentAdmit::Rejected { .. }),
        "a fragment with an undeclared relation is rejected"
    );

    // A fragment may NOT mint an object id (a type name carrying an id form) — Id never invents ids.
    let id_minting = FragmentDef {
        object_type: ObjectType("wiki_page:home".into()),
        relations: vec![RelName("reader".into())],
        permissions: vec![],
    };
    assert!(
        matches!(eng.admit(&id_minting), FragmentAdmit::Rejected { .. }),
        "a fragment cannot mint object ids"
    );
}

/// **The 4.9 consumer promise: a subsystem declares its fragment, admits it through the ABI, and
/// gates an action on the resolved compiled permission.** The consumer declares a `release` type
/// (reader/approver relations; `ship = reader ∩ approver`), admits it via the rich engine path, and
/// gates: a subject who is BOTH reader and approver ships; a reader-only does not (the intersect).
#[test]
fn cdc_4_9_consumer_declares_a_fragment_and_gates_on_it() {
    let s = scope("acme");
    // The consumer's data: alice is reader+approver of release:v2; bob is reader-only.
    let svc = provider(
        &s,
        &[
            add("release:v2", "reader", "p:alice"),
            add("release:v2", "approver", "p:alice"),
            add("release:v2", "reader", "p:bob"),
        ],
    );

    // CONSUMER step 1 — declare the fragment at build time (relations + the intersect permission).
    let release_fragment = FragmentDef {
        object_type: ObjectType("release".into()),
        relations: vec![RelName("reader".into()), RelName("approver".into())],
        permissions: vec![PermissionRule {
            permission: Permission("ship".into()),
            rewrite: Userset::Intersect(vec![
                Userset::Relation(RelName("reader".into())),
                Userset::Relation(RelName("approver".into())),
            ]),
        }],
    };
    // CONSUMER step 2 — admit it into the cell schema (the rich rewrite-carrying path).
    assert!(
        matches!(
            svc.admit_fragment_def(&release_fragment),
            FragmentAdmit::Admitted { .. }
        ),
        "the consumer's fragment admits into the cell schema"
    );

    // CONSUMER step 3 — gate an action on the resolved `ship` permission via the 4.2 check surface.
    let obj = ArtifactRef("release:v2".into());
    let ship = |actor: &Principal| -> bool {
        matches!(
            svc.check(actor, &Permission("ship".into()), &obj, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        ship(&subject("p:alice")),
        "reader ∩ approver ships (alice is both)"
    );
    assert!(
        !ship(&subject("p:bob")),
        "a reader-only does not ship (the intersect denies)"
    );
}

/// **The 4.9 ABI carrier round-trips: `admit_fragment` (names-only) admits a declared-relation
/// permission and rejects a malformed one** — the contract-boundary validator every subsystem hits.
#[test]
fn cdc_4_9_abi_admit_fragment_validates_names_level() {
    let s = scope("acme");
    let svc = provider(&s, &[]);

    // A names-only fragment whose permission name IS a declared relation (the passthrough case).
    let ok = myelin_identity::NamespaceFragment {
        object_type: ObjectType("channel".into()),
        relations: vec![RelName("member".into()), RelName("read".into())],
        permissions: vec![Permission("read".into())], // read is also a declared relation
    };
    assert!(
        matches!(svc.admit_fragment(&ok), Ok(FragmentAdmit::Admitted { .. })),
        "a names-only fragment with a declared-relation permission admits"
    );

    // A names-only fragment whose permission names NO declared relation is rejected (under-spec).
    let bad = myelin_identity::NamespaceFragment {
        object_type: ObjectType("vault".into()),
        relations: vec![RelName("owner".into())],
        permissions: vec![Permission("decrypt".into())], // decrypt is not a declared relation
    };
    assert!(
        matches!(svc.admit_fragment(&bad), Ok(FragmentAdmit::Rejected { .. })),
        "a names-only permission that names no declared relation is rejected"
    );
}

/// **The 4.9 core hierarchy resolves inheritance through the four operators (the headline).** A
/// project reader granted via team membership Allows; a non-member Denies — the `parent_team->view`
/// tuple-to-userset the architecture names, resolved through `check`.
#[test]
fn cdc_4_9_core_hierarchy_inheritance_resolves() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // project:web inherits view from team:eng (the parent_team->view inheritance edge).
            add("project:web", "parent_team", "team:eng#view"),
            // team:eng's view = member ∪ parent_org->view; alice is a direct member.
            add("team:eng", "member", "p:alice"),
        ],
    );
    let obj = ArtifactRef("project:web".into());
    // alice inherits project view via team membership → Allow.
    assert_eq!(
        svc.check(&subject("p:alice"), &Permission("view".into()), &obj, &at_latest(), None),
        Ok(Decision::Allow),
        "a project reader via team membership inherits view (parent_team->view, the four operators)"
    );
    // bob (no membership) → Deny.
    assert_eq!(
        svc.check(
            &subject("p:bob"),
            &Permission("view".into()),
            &obj,
            &at_latest(),
            None
        ),
        Ok(Decision::Deny),
        "a non-member does not inherit project view (fail-closed)"
    );
}
