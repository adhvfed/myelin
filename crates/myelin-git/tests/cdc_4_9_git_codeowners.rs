//! # The CDC pair for the **CODEOWNERS half of contract 4.9** — the Git-owned resolver (GIT-P16 / P-277)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment; the **CODEOWNERS-as-relations**
//! slice). The Git fragment NAMES freeze is GIT-P1 (`cdc_4_9_git_fragment.rs`); THIS file pins the
//! **CODEOWNERS RESOLVER** GIT-P16 ships — the consumer half the architecture assigns to Git
//! (00-overview §1.1 / 03 §5.2: "the resolver compiles CODEOWNERS path globs into `code_owner`
//! relations per ref pattern", and "Git decides which glob a change matches").
//!
//! - the **CONSUMER** is the **Git CODEOWNERS resolver** ([`myelin_git::lifecycle::CodeOwners`]): it
//!   PARSES a repo's CODEOWNERS file, MATCHES a path to its owners (last-match-wins), and COMPILES each
//!   rule to a `ref:<repo>::<glob>#code_owner@<owner>` reviewer-requirement [`TupleDelta`]. The
//!   consumer's promise: it produces the byte-identical tuple shape the engine resolves, and "who must
//!   approve this path" is then the ordinary `list_subjects(ref, code_owner)` Expand — never a bespoke
//!   glob-matcher in the authz hot path (the glob is baked into the `ref` id at write time, §5.2).
//! - the **PROVIDER** is Identity's namespace engine ([`StoreBackedCheck`]): it admits the Git fragment
//!   and resolves the written `code_owner` tuples through `list_subjects`. The provider's `compile`
//!   half ([`myelin_identity_service::git_fragment::compile_codeowners`]) is the encoding the consumer
//!   must MATCH — a Git-written tuple is the exact tuple the engine's Expand resolves.
//!
//! The two are pinned here so a drift on either (Git's resolver emits a different ref-id encoding /
//! relation; Identity's `compile_codeowners` changes shape; the engine stops resolving `code_owner`)
//! fails the same CI job. **The GIT-P16 gate — 0 mis-resolved owners on a fixture CODEOWNERS** — is
//! reified as the resolve-then-Expand assertion below.

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

// A fixture CODEOWNERS file (the GATE fixture — 0 mis-resolved owners must hold over it).
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

/// The PROVIDER surface: the engine with the Git fragment admitted, seeded with `deltas`, and the S8
/// reverse index fed off the bus (the live `list_subjects` Expand reads the S8 index, not the raw
/// tuple store — mirrors the Identity-side CDC `provider`).
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
    // Feed the S8 reverse index off the bus (the live projection list_subjects read).
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env);
    }
    let svc = StoreBackedCheck::with_index(store, index);
    // Admit the Git fragment (the `ref` type carrying the `code_owner` relation must be in the schema).
    for admit in svc.admit_git_fragment() {
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Git fragment admits (the ref/code_owner relation is in the cell schema): {admit:?}"
        );
    }
    svc
}

/// **CONSUMER ↔ PROVIDER: the Git resolver's tuple encoding MATCHES the engine's `compile_codeowners`.**
/// The Git-owned resolver (parse → compile) and Identity's engine `compile_codeowners` (the provider's
/// compile half) must emit the byte-identical `ref:<repo>::<glob>#code_owner@<owner>` shape — a
/// Git-written tuple is the exact tuple the engine resolves. A drift on either fails here.
#[test]
fn cdc_4_9_git_resolver_encoding_matches_the_engine_compile() {
    let co = CodeOwners::parse(FIXTURE).expect("valid fixture");
    let git_deltas = co.resolve(REPO_ID);

    // The SAME rules fed to the engine's compile half (the provider's encoding authority).
    let engine_rules: Vec<git_fragment::CodeownersRule> = co
        .rules
        .iter()
        .map(|r| git_fragment::CodeownersRule {
            path_glob: r.pattern.clone(),
            owners: r.owners.iter().map(|o| PrincipalId(o.clone())).collect(),
        })
        .collect();
    // The engine encodes the repo as a string id; the Git resolver uses the numeric repo id. They
    // agree on the SHAPE `ref:<repo>::<glob>` and the `code_owner` relation; pin both for the SAME
    // repo token so the produced tuples are identical.
    let engine_tuples =
        git_fragment::compile_codeowners(&REPO_ID.to_string(), &engine_rules);

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
         (a Git-written code_owner tuple IS the tuple the engine resolves — no drift)"
    );
}

/// **THE GIT-P16 GATE: 0 mis-resolved owners — the resolver's tuples resolve through
/// `list_subjects(ref, code_owner)` to exactly the right owner set, last-match-wins.**
///
/// Write the Git resolver's `code_owner` tuples into the real engine, then ask "who must approve this
/// path" via the ordinary Expand — for several fixture paths the resolved owner set must match the
/// resolver's own matcher ([`CodeOwners::owners_for`]), proving the parse → match → compile → Expand
/// chain is consistent (0 mis-resolved).
#[test]
fn cdc_4_9_codeowners_resolves_zero_mis_resolved_via_list_subjects() {
    let s = scope("acme");
    let co = CodeOwners::parse(FIXTURE).expect("valid fixture");
    let svc = provider(&s, &co.resolve(REPO_ID));

    let code_owner = Permission(git_fragment::CODE_OWNER.into());

    // For each fixture path, the engine's `list_subjects(ref:<repo>::<matched-glob>, code_owner)`
    // returns exactly the owners the Git matcher resolves — the ref id the GATE keys on is the
    // matched RULE's pattern (last-match-wins). 0 mis-resolved owners is: the two agree, for every
    // path, on exactly which owners must approve.
    let cases: &[(&str, &str)] = &[
        // a .ts under /src/payments/ → payments owns it (last match wins over *.ts).
        ("src/payments/charge.ts", "/src/payments/"),
        // a .ts elsewhere → frontend.
        ("web/app.ts", "*.ts"),
        // a .rs not under payments → the catch-all.
        ("src/core/lib.rs", "*"),
        // docs → writers.
        ("docs/guide/intro.md", "/docs/**"),
    ];

    for (path, expected_glob) in cases {
        // 1) the Git matcher picks the rule (its owners are the truth).
        let matched_owners = co.owners_for(path);
        assert!(!matched_owners.is_empty(), "path `{path}` is owned by some rule");

        // 2) the engine resolves the SAME rule's ref object via list_subjects(ref, code_owner).
        let ref_obj = ObjectId(format!("ref:{REPO_ID}::{expected_glob}"));
        let tree = svc.list_subjects_in(&s, &ref_obj, &code_owner, &at_latest());
        assert_eq!(
            tree.relation,
            RelName(git_fragment::CODE_OWNER.into()),
            "the Expand is over the code_owner relation"
        );
        let resolved: Vec<&str> = tree.members.iter().map(|m| m.0.as_str()).collect();

        // 3) the GATE: 0 mis-resolved — the engine-resolved owner set == the matcher's owner set.
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

/// **A path with no matching rule resolves to NO owners (no spurious requirement).** An unowned path
/// (no rule matches) must not invent a reviewer — the resolver returns an empty owner set, and the
/// engine has no `code_owner` tuple on a glob that path matches. (The fixture's `*` catch-all owns
/// everything, so we test against a fixture WITHOUT a catch-all to exercise the unowned case.)
#[test]
fn cdc_4_9_unowned_path_requires_no_codeowner() {
    let s = scope("acme");
    // a CODEOWNERS with NO catch-all: only /src/payments/ is owned.
    let co = CodeOwners::parse("/src/payments/  @acme/payments\n").expect("valid");
    let svc = provider(&s, &co.resolve(REPO_ID));

    // an unowned path → the matcher returns no owners.
    assert!(
        co.owners_for("README.md").is_empty(),
        "an unowned path has no required CODEOWNERS reviewer"
    );

    // the engine has a code_owner tuple ONLY on the payments glob; a different glob resolves empty.
    let other = ObjectId(format!("ref:{REPO_ID}::*"));
    let tree = svc.list_subjects_in(
        &s,
        &other,
        &Permission(git_fragment::CODE_OWNER.into()),
        &at_latest(),
    );
    assert!(
        tree.members.is_empty(),
        "no code_owner tuple on a glob the unowned path matches (no spurious requirement)"
    );
}
