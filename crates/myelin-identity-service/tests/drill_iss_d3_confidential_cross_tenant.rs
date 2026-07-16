//! # P-ID-29 (global P-322) GATE / DRILL — ISS-D3 (authz side): the confidential-exclusion +
//! cross-tenant IDOR, 0 leak INCLUDING under zookie staleness (dated green artifact)
//!
//! **Drill catalogue row ISS-D3 (F1/F2):** *cross-tenant + confidential-issue IDOR → 0 leak INCLUDING
//! under zookie staleness (the confidential exclusion holds BY CONSTRUCTION; the board conjoin holds
//! under the S8 watermark).* This prompt ships the **Id-side authz content** — the two structural
//! invariants a confidential IDOR attempt acts against:
//!
//! 1. **the `− confidential` exclusion (§5):** a confidential issue is REMOVED from a normal
//!    project-reader's `view` set by the Zanzibar Exclusion operator — a confidential issue disappears
//!    from `list_objects(subject, view, issue)` / `check(subject, view, issue)` BY CONSTRUCTION (never a
//!    post-filter, never a count leak); ONLY an explicit `issue#confidential_grant@subject` re-admits.
//! 2. **cross-tenant isolation:** a grant in one tenant never resolves a `view` in another (the
//!    reverse index + the engine read only the verified scope's partition).
//!
//! **F2 — under zookie staleness:** a freshly-marked-confidential issue must not leak even when the S8
//! reverse index LAGS the marking write. The board scan PINNED at the post-marking zookie does not serve
//! the stale (pre-marking) grant: the watermark guard falls back to per-row `check` over the
//! authoritative S3 store ([`ListObjects::list_objects_consistent`], the same new-enemy guard ID-D7
//! proves) — so the confidential exclusion holds even under staleness.
//!
//! Survival signal: **confidential-leak count = 0** AND **cross-tenant-leak count = 0** (incl. under
//! staleness), projected onto the load-bearing [`SignalName::CrossTenantCount`] zero (the same
//! zero-leak survival signal `git_d8` / `ci_d10` assert). A non-zero on EITHER counter means a
//! confidential issue was reachable by an unauthorised viewer, or a grant crossed a tenant — and the
//! drill aborts LOUDLY (EI-01 §3: loud, never swallowed; the threshold is NEVER weakened to pass).
//!
//! Run against the failure-injection harness's telemetry-assertion library (the contract-1.8
//! survival-signal set), exactly as `git_d8` / `ci_d10` do. `myelin-harness` is a DEV-dependency only —
//! it never enters the identity-service production DAG.

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, ListObjectsResult,
    ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName,
    RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
    CONFIDENTIAL, CONFIDENTIAL_GRANT, ISSUE_VIEW,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn principal(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn scope_of(p: &Principal) -> TenantScope {
    TenantScope::from_verified_token(p, p.region.clone())
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn allows(svc: &StoreBackedCheck, actor: &Principal, perm: &str, object: &str) -> bool {
    matches!(
        svc.check(
            actor,
            &Permission(perm.into()),
            &ArtifactRef(object.into()),
            &at_latest(),
            None
        ),
        Ok(Decision::Allow)
    )
}

/// **ISS-D3 (F1) — the confidential-exclusion holds: 0 confidential-issue leaks to a normal reader.**
///
/// Seed a project with a fleet of project readers (each inherits `parent_project->view` on the
/// project's issues), and a confidential issue marked `confidential` against every one of them (CI/Issues
/// stamps the marker from the issue's confidential flag). A batch of normal project readers each attempt
/// `view` on the confidential issue — every one must DENY (the `− confidential` Exclusion removes them
/// from the `view` set BY CONSTRUCTION). Only the ONE principal with a direct `confidential_grant`
/// re-admits. We assert the confidential-leak count is `0`.
#[test]
fn iss_d3_confidential_exclusion_zero_leak() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    const FLEET: usize = 64;
    let mut tuples: Vec<TupleDelta> = vec![
        // The confidential issue belongs to the project (so a reader would otherwise inherit view)…
        add("issue:secret", "parent_project", "project:proj#view"),
        // …and is stamped confidential against every project reader (the marker the exclusion subtracts).
        // The ONE legitimate path: a direct confidential_grant to the incident owner.
        add("issue:secret", CONFIDENTIAL_GRANT, "p:owner"),
        // A NORMAL issue under the same project (the sanity witness a reader CAN see).
        add("issue:normal", "parent_project", "project:proj#view"),
    ];
    for i in 0..FLEET {
        let r = format!("p:reader-{i}");
        // Each reader is a project reader (inherits view) AND is subtracted from the confidential issue.
        tuples.push(add("project:proj", "reader", &r));
        tuples.push(add("issue:secret", CONFIDENTIAL, &r));
    }
    // The owner is also a project reader, but additionally holds the direct confidential_grant.
    tuples.push(add("project:proj", "reader", "p:owner"));

    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &tuples,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed acme issue grants");

    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_issue_fragment() {
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
    }

    // Sanity: a project reader really CAN view the NORMAL issue (the inheritance edge is live)…
    assert!(
        allows(
            &svc,
            &principal("acme", "p:reader-0"),
            ISSUE_VIEW,
            "issue:normal"
        ),
        "a project reader views a normal issue (parent_project->view resolves)"
    );
    // …and the direct confidential_grant owner views the confidential issue (the only path).
    assert!(
        allows(&svc, &principal("acme", "p:owner"), ISSUE_VIEW, "issue:secret"),
        "the direct confidential_grant owner views the confidential issue (the ∪ confidential_grant arm)"
    );

    // THE ATTACK: every project reader attempts `view` on the confidential issue via inheritance.
    let mut confidential_leaks: i64 = 0;
    for i in 0..FLEET {
        if allows(
            &svc,
            &principal("acme", &format!("p:reader-{i}")),
            ISSUE_VIEW,
            "issue:secret",
        ) {
            confidential_leaks += 1;
        }
    }

    signals.set_scalar(SignalName::CrossTenantCount, confidential_leaks);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        confidential_leaks, 0,
        "0 confidential-issue leaks to a normal project reader (the − confidential Exclusion, ISS-D3)"
    );

    println!(
        "[P-322 DRILL GREEN 2026-06-22] ISS-D3 (F1) confidential-exclusion: \
         fleet={FLEET} project readers attempted view on issue:secret via parent_project->view \
         inheritance (view = (parent_project->view − confidential) ∪ confidential_grant) \
         → confidential-leak count=0; only the direct confidential_grant owner views it (§5, the \
         exclusion removes them BY CONSTRUCTION, never a post-filter)"
    );
}

/// **ISS-D3 (F1) — cross-tenant isolation: 0 cross-tenant issue-view leaks.**
///
/// acme grants a viewer `view` on an issue (via project membership); a globex principal with the SAME
/// principal-id string attempts `view` on the acme issue in the globex scope — it must DENY (the engine
/// + reverse index read only the verified scope's partition). 0 cross-tenant leaks.
#[test]
fn iss_d3_cross_tenant_zero_leak() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&principal("acme", "p-admin"));
    let globex = scope_of(&principal("globex", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    // acme: a project reader sees an acme issue.
    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &[
                add("project:acme-proj", "reader", "p:viewer"),
                add(
                    "issue:acme-issue",
                    "parent_project",
                    "project:acme-proj#view",
                ),
            ],
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed acme");
    // globex: the same project-reader id, but a DIFFERENT project/issue — no acme grant.
    store
        .write_tuples(
            &globex,
            &principal("globex", "p-admin"),
            &[add("project:globex-proj", "reader", "p:viewer")],
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed globex");

    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_issue_fragment() {
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
    }

    // Sanity: the acme viewer sees the acme issue.
    assert!(
        allows(
            &svc,
            &principal("acme", "p:viewer"),
            ISSUE_VIEW,
            "issue:acme-issue"
        ),
        "the acme project reader views the acme issue (in-tenant)"
    );

    // THE ATTACK: the globex principal (same id string) attempts view on the ACME issue.
    let cross_tenant_leak = allows(
        &svc,
        &principal("globex", "p:viewer"),
        ISSUE_VIEW,
        "issue:acme-issue",
    );
    let cross_tenant_leaks: i64 = i64::from(cross_tenant_leak);

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_leaks);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        cross_tenant_leaks, 0,
        "0 cross-tenant issue-view leaks (the engine + reverse index read only the verified scope)"
    );

    println!(
        "[P-322 DRILL GREEN 2026-06-22] ISS-D3 (F1) cross-tenant: a globex principal with the same \
         id string attempted view on the acme issue:acme-issue → cross-tenant-leak count=0 (the \
         verified (tenant, region) scope is the partition; no cross-tenant read path)"
    );
}

/// **ISS-D3 (F2) — 0 leak INCLUDING under zookie staleness: a freshly-marked-confidential issue does
/// not leak even when the S8 reverse index LAGS the marking write.**
///
/// (1) alice holds a direct `confidential_grant` on `issue:hot` (she is a candidate + sees it) — S8 is
/// up to date. (2) The issue is then marked `confidential` against alice (a NEWER zookie) AND her grant
/// removed — but S8 is held BEHIND (the marking write is not yet projected). (3) The board scan PINNED
/// at the post-marking zookie must NOT serve the stale (pre-marking) grant: the watermark guard falls
/// back to per-row `check` over the authoritative S3 store, which reflects the marking — so the
/// confidential issue is ABSENT from alice's board under staleness. 0 leak under staleness.
#[test]
fn iss_d3_zero_leak_under_zookie_staleness() {
    let mut signals = SignalSource::new();
    let s = scope_of(&principal("acme", "p-admin"));

    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in myelin_identity_service::issue_fragment::issue_fragment_defs() {
        assert!(matches!(
            namespace.admit(&def),
            FragmentAdmit::Admitted { .. }
        ));
    }

    // Drain the relay into S8 (project the pending writes) — used to keep S8 fresh for the GRANT only.
    let feed = |consumer: &ReverseIndexConsumer| {
        let bus = InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
        relay.drain_to_empty();
        for env in bus.consume("") {
            consumer.handle(&env, &mut myelin_events::HandlerTx::none());
        }
    };

    // (1) GRANT: alice holds confidential_grant on issue:hot (a direct candidate she can view). Project
    //     it into S8 (the index is up to date for the grant).
    let _z_grant = store
        .write_tuples(
            &s,
            &principal("acme", "p-admin"),
            &[add("issue:hot", CONFIDENTIAL_GRANT, "p:alice")],
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("grant");
    feed(&consumer);

    // (2) MARK CONFIDENTIAL + REVOKE the grant (a NEWER zookie). The S3 store now reflects the marking,
    //     but S8 is held BEHIND (we do NOT feed this write — the index lags).
    let z_mark = store
        .write_tuples(
            &s,
            &principal("acme", "p-admin"),
            &[
                TupleDelta::Remove(RelationTuple {
                    object: ObjectId("issue:hot".into()),
                    relation: RelName(CONFIDENTIAL_GRANT.into()),
                    subject: PrincipalId("p:alice".into()),
                    caveat: None,
                }),
                add("issue:hot", CONFIDENTIAL, "p:alice"),
            ],
            None,
            None,
            Timestamp("2026-06-22T00:01:00Z".into()),
        )
        .expect("mark confidential + revoke");
    // S8 is BEHIND the marking revision (the index lags the write).
    assert!(
        index.watermark(&s).0 < z_mark.0,
        "S8 is BEHIND the confidential-marking revision (watermark={:?} < mark={:?})",
        index.watermark(&s),
        z_mark
    );

    // (3) The security-sensitive board scan PINNED at the post-marking zookie.
    let lo = ListObjects::with_cap(store, namespace, index, 0); // cap 0 → Filter (the S8 push-down path)
    let post_mark = Consistency {
        at_least: z_mark.clone(),
        mode: ConsistencyMode::Strong,
    };
    let result = lo.list_objects_consistent(
        &s,
        &principal("acme", "p:alice"),
        &Permission(ISSUE_VIEW.into()),
        &ObjectType("issue".into()),
        &post_mark,
    );

    // Under staleness the consistent path must NOT serve the stale grant — it falls back to per-row
    // check over the authoritative S3 (which reflects the marking), returning the re-checked Ids with
    // issue:hot ABSENT (alice no longer has the grant + is now subtracted by the exclusion).
    let stale_leaks: i64 = match result {
        ListObjectsResult::Ids { ids, .. } => i64::from(ids.iter().any(|o| o.0 == "issue:hot")),
        ListObjectsResult::Filter { .. } => {
            // A Filter served directly from the BEHIND index would be the stale-grant leak — the guard
            // must have fallen back to Ids. If we got a Filter here the watermark guard failed.
            panic!(
                "the watermark guard must fall back to per-row check under staleness, not serve a \
                    Filter from the behind index"
            )
        }
    };

    signals.set_scalar(SignalName::CrossTenantCount, stale_leaks);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        stale_leaks, 0,
        "0 confidential-issue leaks UNDER zookie staleness (the watermark guard falls back to check \
         over the authoritative S3 — ISS-D3 F2, the new-enemy guard)"
    );

    println!(
        "[P-322 DRILL GREEN 2026-06-22] ISS-D3 (F2) under-staleness: issue:hot marked confidential + \
         alice's confidential_grant revoked at a NEWER zookie, S8 held BEHIND; the board scan pinned \
         at the post-marking zookie fell back to per-row check over authoritative S3 (the watermark \
         guard) → stale-confidential-leak count=0 (the confidential exclusion holds under the S8 \
         watermark, never a stale allow)"
    );
}
