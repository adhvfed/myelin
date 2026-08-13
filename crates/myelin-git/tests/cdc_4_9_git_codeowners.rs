use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_git::lifecycle::CodeOwners;
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, Permission, Principal, PrincipalId, PrincipalKind,
    RelName, TupleDelta, Zookie,
};
use myelin_identity_service::{
    git_fragment, ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

const REPO_ID: u128 = 7;

const FIXTURE: &str = "\
# default owners
*               @acme/core-team
*.ts            @acme/frontend
/src/payments/  @acme/payments @alice
/docs/**        @acme/writers
";

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

fn provider(s: &TenantScope, deltas: &[TupleDelta]) -> StoreBackedCheck {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    if !deltas.is_empty() {
        store
            .write_tuples(
                s,
                &subject("p-admin"),
                deltas,
                None,
                None,
                Timestamp("2026-06-21T00:00:00Z".into()),
            )
            .expect("seed code_owner tuples");
    }
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_git_fragment() {
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Git fragment admits (the ref/code_owner relation is in the cell schema): {admit:?}"
        );
    }
    svc
}

#[test]
fn cdc_4_9_git_resolver_encoding_matches_the_engine_compile() {
    let co = CodeOwners::parse(FIXTURE).expect("valid fixture");
    let git_deltas = co.resolve(REPO_ID);

    let engine_rules: Vec<git_fragment::CodeownersRule> = co
        .rules
        .iter()
        .map(|r| git_fragment::CodeownersRule {
            path_glob: r.pattern.clone(),
            owners: r.owners.iter().map(|o| PrincipalId(o.clone())).collect(),
        })
        .collect();
    let engine_tuples = git_fragment::compile_codeowners(&REPO_ID.to_string(), &engine_rules);

    let git_tuples: Vec<_> = git_deltas
        .iter()
        .map(|d| match d {
            TupleDelta::Add(t) => t.clone(),
            TupleDelta::Remove(_) => panic!("resolve emits only Add deltas"),
        })
        .collect();

    assert_eq!(
        git_tuples, engine_tuples,
        "the Git resolver emits the byte-identical tuple shape the engine's compile_codeowners does \
         (a Git-written code_owner tuple IS the tuple the engine resolves - no drift)"
    );
}

#[test]
fn cdc_4_9_codeowners_resolves_zero_mis_resolved_via_list_subjects() {
    let s = scope("acme");
    let co = CodeOwners::parse(FIXTURE).expect("valid fixture");
    let svc = provider(&s, &co.resolve(REPO_ID));

    let code_owner = Permission(git_fragment::CODE_OWNER.into());

    let cases: &[(&str, &str)] = &[
        ("src/payments/charge.ts", "/src/payments/"),
        ("web/app.ts", "*.ts"),
        ("src/core/lib.rs", "*"),
        ("docs/guide/intro.md", "/docs/**"),
    ];

    for (path, expected_glob) in cases {
        let matched_owners = co.owners_for(path);
        assert!(
            !matched_owners.is_empty(),
            "path `{path}` is owned by some rule"
        );

        let ref_obj = ObjectId(format!("ref:{REPO_ID}::{expected_glob}"));
        let tree = svc
            .list_subjects_in(&s, &ref_obj, &code_owner, &at_latest())
            .expect("read code-owner relationships");
        assert_eq!(
            tree.relation,
            RelName(git_fragment::CODE_OWNER.into()),
            "the Expand is over the code_owner relation"
        );
        let resolved: Vec<&str> = tree.members.iter().map(|m| m.0.as_str()).collect();

        let mut want: Vec<&str> = matched_owners.iter().map(String::as_str).collect();
        want.sort_unstable();
        let mut got = resolved.clone();
        got.sort_unstable();
        assert_eq!(
            got, want,
            "0 mis-resolved owners for `{path}`: list_subjects(ref, code_owner) == the matcher's owners"
        );
    }
}

#[test]
fn cdc_4_9_unowned_path_requires_no_codeowner() {
    let s = scope("acme");
    let co = CodeOwners::parse("/src/payments/  @acme/payments\n").expect("valid");
    let svc = provider(&s, &co.resolve(REPO_ID));

    assert!(
        co.owners_for("README.md").is_empty(),
        "an unowned path has no required CODEOWNERS reviewer"
    );

    let other = ObjectId(format!("ref:{REPO_ID}::*"));
    let tree = svc
        .list_subjects_in(
            &s,
            &other,
            &Permission(git_fragment::CODE_OWNER.into()),
            &at_latest(),
        )
        .expect("read relationships for the unowned path");
    assert!(
        tree.members.is_empty(),
        "no code_owner tuple on a glob the unowned path matches (no spurious requirement)"
    );
}
