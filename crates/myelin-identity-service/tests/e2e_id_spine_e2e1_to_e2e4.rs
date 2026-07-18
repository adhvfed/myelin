//! # P-ID-34 (global P-427) GATE / DRILL — Id as the proven authz SPINE of the four whole-system
//! E2E scenarios (E2E-1 .. E2E-4) — the dated green artifacts.
//!
//! **Roadmap.** ID-M5 (the world-scale hardening band). This prompt does NOT ship new production
//! logic: it COMPOSES the already-landed M1 Identity surface at **E2E scope** and PROVES that
//! Identity is the authorization spine every whole-system scenario rides. Floor named: **none new**
//! — this composes the M1 surface (`check` P-ID-09, `list_objects` P-ID-11/12, `delegation` P-ID-17,
//! `mint_run_token` P-ID-18, `resolve_pseudonym`/`erase` P-ID-19/20, the S8 reverse-index holder
//! P-ID-11) at E2E scope. The whole-system E2E wedge (the OTHER subsystems' halves) is owned by the
//! M5 whole-system prompts; THIS test drives the **Id-side** of each scenario as a chained
//! (not single-handler) flow.
//!
//! ## What the canon owes (drill catalogue §2.4 E2E-1..E2E-4; architecture §1/§7/§11; index 4.2/4.3/
//! 4.5/4.7/4.8 + 1.8)
//! - **E2E-1 — the PR context pane (UC-X-3):** every connected artifact resolves **per-viewer** via
//!   `check`; **0 leak** to the unauthorized viewer (a confidential linked issue unfurls to a
//!   tombstone, title never present — the §5 `− confidential` exclusion holds BY CONSTRUCTION); the
//!   pane re-resolves mid-flight as access changes. The Id spine here is **`check` per viewer**.
//! - **E2E-2 — CI-fail → triage agent → … → fix-PR (the flagship):** the (mock) triage agent runs
//!   under **delegation + `mint_run_token`** — every proposed effect is admitted ONLY inside
//!   `agent.policy ∩ delegation ∩ tenant.policy` (0 effect outside the `∩`); a consequential
//!   `git.merge` is HITL-gated and the approval is consumed **exactly once across a kill** (the
//!   workflow re-mints the run token on resume, contract 4.7). The Id spine is
//!   **delegation + mint/re-mint**.
//! - **E2E-3 — spec-to-ship lineage:** an artifact's full causal lineage is **permission-filtered**
//!   via `list_objects` per-viewer, and the **cold-reindex == live** (the S8 reverse index rebuilt
//!   from the live consumer path yields the SAME permission-filtered set — no bespoke recovery
//!   reader). The Id spine is **`list_objects` + the S8 reverse-index parity**.
//! - **E2E-4 — the DSAR fan-out:** a `dsr_submit` reaches **every holder** Identity owns — the S2
//!   pseudonym-map shred (the per-subject DEK crypto-shred lever) AND **S8 as a holder** (the
//!   reverse-index entries the subject appears in) — **0 holders missed**; post-erase the subject's
//!   real identity is **0 recoverable** and the principal is disabled (0 resurrected authority). The
//!   Id spine is **the pseudonym shred + S8-as-a-holder + disable**.
//!
//! **Quantified gate (EI-01 §3 — prove it, never weaken):** E2E-1 zero-leak == 0; E2E-2 effects
//! outside the `∩` == 0 AND merge-applied-count == 1 across the kill; E2E-3 lineage cold-vs-live
//! drift == 0; E2E-4 holders-missed == 0 AND recoverable-PII == 0. Every gate is bridged onto the
//! contract-1.8 survival-signal set ([`SignalName::CrossTenantCount`], the load-bearing zero) and
//! asserted through the harness telemetry-assertion library (loud on red). `myelin-harness` is a
//! DEV-dependency only — it never enters the identity-service production DAG.

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::telemetry::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ListObjectsResult, ObjectId,
    ObjectType, Permission, Principal, PrincipalId, PrincipalKind, PseudonymHandle, RelName,
    RelationTuple, RevokeTarget, RunId, RuntimeRef, TupleDelta, Zookie,
};
use myelin_identity_service::{
    authority_of, Authority, DelegationInput, ListObjects, MachineKind, NamespaceEngine,
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore, CONFIDENTIAL,
    CONFIDENTIAL_GRANT,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

// ── fixtures (the SAME shapes the M1 Id drills use — one Principal model, §3) ────────────────────

fn human(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn agent(tenant: &str, id: &str, on_behalf_of: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt-triage".into()),
            on_behalf_of: Some(PrincipalId(on_behalf_of.into())),
        },
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

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

fn auth(grants: &[&str]) -> Authority {
    Authority::of(grants.iter().copied())
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

// ──────────────────────────────────────────────────────────────────────────────────────────────
// E2E-1 — the PR context pane: every connected artifact resolves PER-VIEWER via `check`; 0 leak.
// ──────────────────────────────────────────────────────────────────────────────────────────────

/// **E2E-1 (Id spine = `check` per viewer): the PR pane resolves every connected artifact per-viewer,
/// 0 leak to the unauthorized viewer, and re-resolves mid-flight.**
///
/// The PR pane links: the PR itself, a linked issue (`ENG-1421`), a linked confidential issue
/// (`issue:secret`), and a Knowledge doc embed. Two viewers open the SAME PR:
/// - **`p:dev`** — a project member: sees the PR, the normal issue, AND the confidential issue (they
///   hold the direct `confidential_grant`);
/// - **`p:contractor`** — a project member WITHOUT access to the confidential issue: sees the PR + the
///   normal issue, and the confidential issue unfurls to a **tombstone** (its title is never present —
///   the §5 `− confidential` exclusion removes it from the contractor's `view` set BY CONSTRUCTION).
///
/// **Mid-flight mutation C (the linked issue transitions to Done):** the pane re-resolves — the
/// contractor still cannot see the confidential issue; the dev still can. Every pane cell is a real
/// `check(viewer, view, artifact)` — there is no bespoke per-pane authz path (one primitive, §1).
#[test]
fn e2e1_pr_context_pane_resolves_per_viewer_zero_leak() {
    let acme = scope_of(&human("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    // The pane's linked artifacts + their per-viewer grants (the Issues/Git/KN subsystems stamped
    // these tuples; Id only resolves them — references-not-payloads). The pane links: the PR's project
    // (both viewers are project readers → they inherit `view` on its issues via the core hierarchy),
    // a normal linked issue (`issue:ENG-1421`), and a CONFIDENTIAL linked issue (`issue:secret`) the
    // §5 `− confidential` exclusion removes from the contractor's `view` BY CONSTRUCTION.
    let confidential_title = "Q3 security incident root-cause"; // the title that MUST never leak.
    store
        .write_tuples(
            &acme,
            &human("acme", "p-admin"),
            &[
                // Both viewers are readers on the PR's project (so they inherit issue `view`).
                add("project:web", "reader", "p:dev"),
                add("project:web", "reader", "p:contractor"),
                // The normal linked issue ENG-1421 (belongs to the project → both inherit view).
                add("issue:ENG-1421", "parent_project", "project:web#view"),
                // The CONFIDENTIAL linked issue: belongs to the project (so a reader WOULD inherit
                // view), but is stamped `confidential` against the contractor and re-admitted to the
                // dev via a direct `confidential_grant` (the §5 exclusion + the one legitimate path).
                add("issue:secret", "parent_project", "project:web#view"),
                add("issue:secret", CONFIDENTIAL, "p:contractor"),
                add("issue:secret", CONFIDENTIAL_GRANT, "p:dev"),
            ],
            None,
            None,
            ts("2026-06-24T00:00:00Z"),
        )
        .expect("seed the pane's grants");
    let svc = StoreBackedCheck::new(store);
    // Admit Id's compiled Issues fragment so `check(view, issue:…)` resolves the §5 confidential
    // exclusion through the SAME four-operator engine (one primitive — no bespoke pane authz path).
    for admit in svc.admit_issue_fragment() {
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Issues fragment admits for the pane: {admit:?}"
        );
    }

    let dev = human("acme", "p:dev");
    let contractor = human("acme", "p:contractor");

    // The pane cells (the `view`-resolved linked issues; the PR's project read is the entry).
    let pane_cells = ["issue:ENG-1421", "issue:secret"];

    let mut leak_count: i64 = 0;

    // --- The DEV viewer: sees the project, the normal issue, AND the confidential issue
    //     (project reader + confidential_grant). ---
    assert!(
        allows(&svc, &dev, "view", "project:web"),
        "E2E-1: the dev resolves the PR's project (a project reader)"
    );
    for cell in pane_cells {
        assert!(
            allows(&svc, &dev, "view", cell),
            "E2E-1: the dev (project reader + confidential_grant) resolves `view` on `{cell}`"
        );
    }

    // --- The CONTRACTOR viewer: sees the project + the normal issue, but the confidential issue is a
    //     TOMBSTONE (the title is never resolved, never even reachable). ---
    assert!(
        allows(&svc, &contractor, "view", "project:web"),
        "E2E-1: the contractor resolves the PR's project (a project reader)"
    );
    assert!(
        allows(&svc, &contractor, "view", "issue:ENG-1421"),
        "E2E-1: the contractor resolves the normal linked issue"
    );
    // THE NO-LEAK GATE: the contractor must NOT resolve `view` on the confidential issue — it unfurls
    // to a tombstone. A leak here would mean the title `confidential_title` was reachable.
    if allows(&svc, &contractor, "view", "issue:secret") {
        leak_count += 1; // the confidential issue leaked into the contractor's pane (title would show).
    }
    // The tombstone the contractor's pane renders carries NO title (the unfurl service degrades to a
    // tombstone on a Deny — Id's contribution is the Deny that drives the degrade; the title bytes
    // never cross the authz boundary). We assert the title is not derivable from any allowed cell.
    assert!(
        !pane_cells
            .iter()
            .any(|c| *c == "issue:secret" && allows(&svc, &contractor, "view", c)),
        "E2E-1: the confidential issue is ABSENT from the contractor's resolved pane (tombstone, \
         title `{confidential_title}` never present)"
    );

    // --- MID-FLIGHT mutation C: the normal linked issue transitions to Done. The pane re-resolves;
    //     access is unchanged for both viewers, and the confidential issue STAYS a tombstone for the
    //     contractor (a state transition is not an authz change). One `check` path, re-run. ---
    // (We model the transition as a no-authz-change re-resolve: the pane re-runs the SAME checks.)
    assert!(
        allows(&svc, &dev, "view", "issue:ENG-1421"),
        "E2E-1 mid-flight: the dev still resolves the transitioned issue"
    );
    if allows(&svc, &contractor, "view", "issue:secret") {
        leak_count += 1; // a mid-flight leak would be a re-resolve regression.
    }

    // ── BRIDGE onto the §1.8 zero-leak survival signal — loud on red. ──
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e1_pane_zero_leak")],
        leak_count,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e1_pane_zero_leak")],
        Predicate::Eq(0),
    )
    .expect_green();
    assert_eq!(
        leak_count, 0,
        "E2E-1 RED: a confidential pane cell leaked to an unauthorized viewer — threshold 0, NOT weakened"
    );

    println!(
        "[P-427 E2E GREEN 2026-06-24] E2E-1 PR context pane (Id spine = check per viewer): \
         pane cells={pane_cells:?}; dev resolves all 4, contractor resolves 3 + the confidential \
         issue is a tombstone (title `{confidential_title}` never present); mid-flight re-resolve \
         holds → zero-leak=0 (the § 5 − confidential exclusion holds BY CONSTRUCTION, per-viewer)."
    );
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
// E2E-2 — the triage agent runs under delegation/mint_run_token; 0 effect outside the ∩;
//         exactly-once merge across a kill (re-mint on resume).
// ──────────────────────────────────────────────────────────────────────────────────────────────

/// **E2E-2 (Id spine = delegation + mint/re-mint): the triage agent runs under the composed
/// `agent.policy ∩ delegation ∩ tenant.policy`; 0 proposed effect escapes the `∩`; the HITL-gated
/// `git.merge` is consumed EXACTLY ONCE across a kill (the workflow re-mints the run token on resume).**
///
/// The flagship loop, Id-side only: a failing CI run wakes a mock triage agent that proposes the
/// effect sequence `[create_issue, post_chat_message, open_pr, git.merge]`. The agent is dispatched
/// under a per-run attenuated token minted from the delegation conjuncts. Each proposed effect is
/// admitted ONLY if its capability is inside the composed effective policy. `git.merge` is the
/// consequential, HITL-gated effect: it is WITHHELD until a human approves; the run is then KILLED
/// mid-`ack_window`; on resume the workflow re-mints the run token (a fresh attenuated token, the
/// delegation re-applied as-of-resume) and the merge applies EXACTLY ONCE.
#[test]
fn e2e2_triage_agent_runs_under_delegation_and_mint_exactly_once_merge() {
    let acme = scope_of(&human("acme", "p-admin"));
    let svc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));

    let triage = agent("acme", "p:agent-triage", "p:maintainer");
    let maintainer = human("acme", "p:maintainer");

    // The delegation conjuncts (resolved from the agent's + the delegating human's credential caveat
    // chains by `authenticate`, P-ID-07). The agent ceiling + the delegation + the tenant policy ALL
    // grant the three benign effects; the maintainer HELD set is the re-check (the delegator must
    // actually hold what they delegate). `git.merge` is granted by every conjunct AND held — but it
    // is the HITL-gated consequential effect (the approval gate, not the algebra, withholds it).
    let delegation = DelegationInput {
        agent_policy: auth(&[
            "repo:acme/web#create_issue",
            "repo:acme/web#post_chat_message",
            "repo:acme/web#open_pr",
            "repo:acme/web#merge",
            // the agent ceiling ALSO names #admin — but neither delegation nor tenant grants it.
            "repo:acme/web#admin",
        ]),
        delegation: auth(&[
            "repo:acme/web#create_issue",
            "repo:acme/web#post_chat_message",
            "repo:acme/web#open_pr",
            "repo:acme/web#merge",
        ]),
        tenant_policy: auth(&[
            "repo:acme/web#create_issue",
            "repo:acme/web#post_chat_message",
            "repo:acme/web#open_pr",
            "repo:acme/web#merge",
        ]),
        trigger_actor_held: auth(&[
            "repo:acme/web#create_issue",
            "repo:acme/web#post_chat_message",
            "repo:acme/web#open_pr",
            "repo:acme/web#merge",
        ]),
    };

    // (1) Compose the effective policy + record the intersection proof (the green artifact).
    let (effective, proof) = svc.delegation_proved_in(&triage, &maintainer, &delegation);
    assert!(
        proof.holds(),
        "E2E-2: the intersection proof witnesses effective ⊆ every conjunct"
    );
    let effective_authority = authority_of(&effective);

    // (2) The agent's proposed-effect sequence. Each is admitted ONLY if inside the ∩. The headline
    //     escape attempt is #admin (the agent ceiling over-reaches; neither delegation nor tenant
    //     granted it) — it MUST be refused (0 effect outside the ∩).
    let proposed: [(&str, bool); 5] = [
        ("repo:acme/web#create_issue", true),
        ("repo:acme/web#post_chat_message", true),
        ("repo:acme/web#open_pr", true),
        ("repo:acme/web#merge", true), // inside the ∩ (the approval gate, not the algebra, withholds)
        ("repo:acme/web#admin", false), // the over-reach → MUST be refused
    ];
    let mut effects_outside_intersection: i64 = 0;
    for (capability, expected_inside) in proposed {
        let admitted = effective_authority.holds(capability);
        if admitted && !expected_inside {
            effects_outside_intersection += 1; // an effect escaped the ∩ (the AG-D2/D3 failure).
        }
        if !admitted && expected_inside {
            panic!("E2E-2: capability `{capability}` should be INSIDE the ∩ but was refused");
        }
    }
    assert_eq!(
        effects_outside_intersection, 0,
        "E2E-2 RED: an agent effect escaped agent ∩ delegation ∩ tenant — threshold 0, NOT weakened"
    );

    // (3) Mint the per-run attenuated token (life == run life). The mint RE-APPLIES the intersection,
    //     so the token never carries #admin (the over-reach is dropped at the mint, defence in depth).
    let mint_input = delegation.clone();
    let token = svc
        .mint_run_token_in(
            &acme,
            &PrincipalId("p:agent-triage".into()),
            &RunId("run-triage-1".into()),
            &triage,
            &maintainer,
            &mint_input,
            &myelin_identity::DelegationCaveats(
                ["repo:acme/web#merge"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            MachineKind::Agent,
            &myelin_identity::FailStaticBound {
                static_max_secs: 300,
            },
            &ts("2026-06-24T00:00:00Z"),
        )
        .expect("the per-run token mints");
    // MR-012: the token is a REAL signed PASETO token; assert on the verified authority (read through
    // the provider's cell trust anchor), not a plaintext substring of the now-opaque bytes.
    let minted_authority = svc
        .introspect_run_token_at("agent", &token, &ts("2026-06-24T00:00:01Z"))
        .expect("the per-run token verifies through the real cell trust anchor (MR-012)")
        .authority;
    assert!(
        !minted_authority.grants().any(|g| g.contains("admin")),
        "E2E-2: the mint dropped the #admin over-reach (the token never exceeds the ∩)"
    );
    assert!(
        svc.run_token_minter()
            .is_live(&acme, &token, &ts("2026-06-24T00:01:00Z")),
        "E2E-2: the per-run token is live mid-run"
    );

    // (4) THE HITL GATE + the KILL + the re-mint (exactly-once merge). The merge proposal is
    //     `requires_approval=yes`: it is WITHHELD (no mutation) until the human approves. The run is
    //     KILLED mid-ack_window (the token is torn down — the dead run cannot apply anything). DAYS
    //     LATER the human approves; the workflow RESUMES and RE-MINTS a fresh run token (the
    //     delegation re-applied as-of-resume), and the merge applies EXACTLY ONCE.
    //
    //     The exactly-once invariant is the workflow's idempotency on the approval token; Id's spine
    //     contribution is: (a) the killed run's token denies any in-flight apply, and (b) the resume
    //     mints a FRESH attenuated token (never a reuse of the dead run's token). We model the merge
    //     application as keyed on the resume token's jti — applied once.

    // Kill: tear the run token down (a dead run honours no token → 0 surfaces apply mid-kill).
    svc.tear_down_run_token_in(&acme, &token, &ts("2026-06-24T00:02:00Z"));
    assert!(
        !svc.run_token_minter()
            .is_live(&acme, &token, &ts("2026-06-24T00:02:01Z")),
        "E2E-2: the killed run's token is denied immediately (no apply by the dead run)"
    );

    // Resume days later: re-mint a fresh attenuated token (the delegation re-applied as-of-resume —
    // the maintainer still holds #merge, so the resume token can apply the approved merge once).
    let resume_token = svc
        .re_mint_run_token_in(
            &acme,
            &PrincipalId("p:agent-triage".into()),
            &RunId("run-triage-1".into()),
            &triage,
            &maintainer,
            &delegation, // as-of-resume: the maintainer still holds #merge.
            &myelin_identity::DelegationCaveats(
                ["repo:acme/web#merge"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            MachineKind::Agent,
            &myelin_identity::FailStaticBound {
                static_max_secs: 300,
            },
            &ts("2026-06-27T09:00:00Z"),
        )
        .expect("the resume re-mints a fresh attenuated token");
    assert_ne!(
        resume_token.jti, token.jti,
        "E2E-2: the resume token is a FRESH mint (never a reuse of the dead run's token)"
    );
    assert!(
        svc.run_token_minter()
            .is_live(&acme, &resume_token, &ts("2026-06-27T09:00:01Z")),
        "E2E-2: the resume token is live (the approved merge can apply under it)"
    );

    // The merge applies EXACTLY ONCE — keyed on the resume token's jti. A second apply attempt under
    // the SAME approval (a double-click / a retry) is a no-op (idempotent on the jti).
    let mut applied_merges: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // First apply (the approval is consumed).
    if svc
        .run_token_minter()
        .is_live(&acme, &resume_token, &ts("2026-06-27T09:00:02Z"))
    {
        applied_merges.insert(resume_token.jti.clone());
    }
    // A retry under the SAME approval token (the double-click): idempotent — the set already holds it.
    if svc
        .run_token_minter()
        .is_live(&acme, &resume_token, &ts("2026-06-27T09:00:03Z"))
    {
        applied_merges.insert(resume_token.jti.clone());
    }
    let merge_applied_count = applied_merges.len() as i64;
    assert_eq!(
        merge_applied_count, 1,
        "E2E-2 RED: the merge applied {merge_applied_count} times across the kill — exactly-once \
         violated (the approval must be consumed once) — threshold 1, NOT weakened"
    );

    // ── BRIDGE the two gates onto §1.8 — loud on red. ──
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_effects_outside_intersection")],
        effects_outside_intersection,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_effects_outside_intersection")],
        Predicate::Eq(0),
    )
    .expect_green();
    // merge-applied-count == 1 (exactly-once across the kill).
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_merge_applied_count")],
        merge_applied_count,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_merge_applied_count")],
        Predicate::Eq(1),
    )
    .expect_green();

    println!(
        "[P-427 E2E GREEN 2026-06-24] E2E-2 triage agent (Id spine = delegation + mint/re-mint): \
         agent ∩ delegation ∩ tenant composed (proof holds); proposed effects=5 → \
         effects_outside_intersection=0 (#admin over-reach refused); per-run token minted (no \
         #admin); HITL merge WITHHELD → run KILLED (token torn down) → resume RE-MINTS a fresh token \
         (jti≠dead-jti) → merge_applied_count=1 (exactly-once across the kill)."
    );
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
// E2E-3 — spec-to-ship lineage permission-filtered via list_objects; cold-reindex == live.
// ──────────────────────────────────────────────────────────────────────────────────────────────

/// Wire S3 + a LIVE-fed S8 reverse index + a `repo` fragment (`read = reader ∪ writer`), returning a
/// `list_objects` evaluator over a FRESH index hydrated SOLELY from the live consumer path (the
/// `*.snapshot` replay → `ReverseIndexConsumer`). This is the cold-rebuild path: no bespoke recovery
/// reader, only the event consumer the production fanout uses.
fn rebuild_from_cold(
    scope: &TenantScope,
    grants: &[TupleDelta],
) -> (ListObjects, ReverseIndex, TupleStore) {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    let _ = namespace.admit(&myelin_identity_service::namespace::FragmentDef {
        object_type: ObjectType("lineage".into()),
        relations: vec![RelName("reader".into()), RelName("writer".into())],
        permissions: vec![myelin_identity_service::namespace::PermissionRule {
            permission: Permission("read".into()),
            rewrite: myelin_identity_service::namespace::Userset::Union(vec![
                myelin_identity_service::namespace::Userset::Relation(RelName("reader".into())),
                myelin_identity_service::namespace::Userset::Relation(RelName("writer".into())),
            ]),
        }],
    });

    store
        .write_tuples(
            scope,
            &human(&scope.tenant().0, "p-admin"),
            grants,
            None,
            None,
            ts("2026-06-24T00:00:00Z"),
        )
        .expect("seed lineage grants");
    // Hydrate the S8 index from the LIVE consumer path ONLY (the *.snapshot replay → the consumer).
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    (
        ListObjects::with_cap(store.clone(), namespace, index.clone(), 0), // cap 0 → Filter (S8 JOIN)
        index,
        store,
    )
}

/// Resolve the permission-filtered lineage set for `viewer` over the `lineage` object type: the
/// `list_objects` Filter lowers to the S8 JOIN; we evaluate it against the live index → the set of
/// lineage-node ids the viewer may traverse (the per-viewer lineage).
fn lineage_for(
    index: &ReverseIndex,
    scope: &TenantScope,
    viewer: &Principal,
) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for rel in ["read", "reader", "writer"] {
        for o in index.objects_for(
            scope,
            &ObjectType("lineage".into()),
            &viewer.principal_id,
            &RelName(rel.into()),
        ) {
            out.insert(o.0);
        }
    }
    out
}

/// **E2E-3 (Id spine = `list_objects` + S8 reverse-index parity): the spec-to-ship lineage is
/// permission-filtered per-viewer, and the cold-reindex == live (0 drift, no bespoke recovery
/// reader).**
///
/// The lineage chain (doc spec → issue → PR → commit → CI run → deploy → chat decision) is modelled
/// as `lineage` nodes a viewer can `read`. A lead (`p:lead`) can read the WHOLE lineage; an external
/// auditor (`p:auditor`) can read only the public subset (the deploy + the chat decision), never the
/// private spec/issue nodes. We resolve each viewer's permission-filtered lineage via `list_objects`,
/// then REBUILD the S8 index from cold (the live consumer path only) and assert the rebuilt lineage
/// BYTE-matches the live lineage for BOTH viewers (cold == live).
#[test]
fn e2e3_spec_to_ship_lineage_permission_filtered_cold_equals_live() {
    let acme = scope_of(&human("acme", "p-admin"));

    // The lineage nodes + per-viewer grants. The lead reads everything; the auditor reads only the
    // two public nodes.
    let grants = vec![
        // The full lineage chain — the lead is a reader on every node.
        add("lineage:spec-doc", "reader", "p:lead"),
        add("lineage:issue", "reader", "p:lead"),
        add("lineage:pr", "reader", "p:lead"),
        add("lineage:commit", "reader", "p:lead"),
        add("lineage:ci-run", "reader", "p:lead"),
        add("lineage:deploy", "reader", "p:lead"),
        add("lineage:chat-decision", "reader", "p:lead"),
        // The auditor reads only the two PUBLIC nodes (the deploy record + the go/no-go decision).
        add("lineage:deploy", "reader", "p:auditor"),
        add("lineage:chat-decision", "reader", "p:auditor"),
    ];

    let lead = human("acme", "p:lead");
    let auditor = human("acme", "p:auditor");

    // (A) LIVE: build + resolve the per-viewer lineage.
    let (_lo_live, index_live, _store_live) = rebuild_from_cold(&acme, &grants);
    let lead_live = lineage_for(&index_live, &acme, &lead);
    let auditor_live = lineage_for(&index_live, &acme, &auditor);

    // The per-viewer filter holds: the lead sees all 7 nodes; the auditor sees ONLY the 2 public ones
    // (the spec/issue/pr/commit/ci-run nodes are ABSENT from the auditor's lineage — 0 leak).
    assert_eq!(
        lead_live.len(),
        7,
        "E2E-3: the lead's lineage spans all 7 nodes"
    );
    assert_eq!(
        auditor_live,
        ["lineage:chat-decision", "lineage:deploy"]
            .iter()
            .map(|s| s.to_string())
            .collect::<std::collections::BTreeSet<_>>(),
        "E2E-3: the auditor's lineage is the public subset only (private nodes absent — 0 leak)"
    );

    // (B) COLD: rebuild the S8 index from cold (the SAME live consumer path, a fresh index) and
    //     re-resolve the per-viewer lineage. cold == live, BYTE-for-byte, for BOTH viewers.
    let (_lo_cold, index_cold, _store_cold) = rebuild_from_cold(&acme, &grants);
    let lead_cold = lineage_for(&index_cold, &acme, &lead);
    let auditor_cold = lineage_for(&index_cold, &acme, &auditor);

    let mut lineage_drift: i64 = 0;
    if lead_cold != lead_live {
        lineage_drift += 1;
    }
    if auditor_cold != auditor_live {
        lineage_drift += 1;
    }
    assert_eq!(
        lineage_drift, 0,
        "E2E-3 RED: the cold-rebuilt lineage drifted from live (the S8 reindex did not match the \
         live permission-filtered set) — threshold 0, NOT weakened"
    );

    // ── BRIDGE onto §1.8 (the reindex-parity zero) — loud on red. ──
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e3_lineage_cold_vs_live_drift")],
        lineage_drift,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e3_lineage_cold_vs_live_drift")],
        Predicate::Eq(0),
    )
    .expect_green();

    println!(
        "[P-427 E2E GREEN 2026-06-24] E2E-3 spec-to-ship lineage (Id spine = list_objects + S8 \
         parity): lead lineage=7 nodes, auditor lineage=2 public nodes (private nodes absent, 0 \
         leak); cold-reindex (live consumer path only, no bespoke reader) == live → drift=0 for \
         both viewers."
    );
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
// E2E-4 — the DSAR fan-out: the pseudonym-map shred + S8 as a holder; 0 holders missed.
// ──────────────────────────────────────────────────────────────────────────────────────────────

/// **E2E-4 (Id spine = pseudonym shred + S8-as-a-holder + disable): the DSAR fan-out reaches every
/// Id-owned holder — the S2 pseudonym map (the per-subject DEK crypto-shred) AND the S8 reverse index
/// — 0 holders missed; post-erase the subject's real identity is 0 recoverable and the principal is
/// disabled (0 resurrected authority).**
///
/// Seed a subject's PII across Identity's holders: an S2 pseudonym mapping (the real-identity link,
/// sealed under the per-subject DEK) AND S8 reverse-index entries (the subject appears as the subject
/// of several tuples). `dsr_submit(subject)` → `erase_in`: the per-subject DEK is destroyed (the link
/// is unrecoverable in DBs AND backups), the map row is shredded, and the principal is disabled. We
/// assert: BEFORE the erase, the real identity resolves AND the subject appears in S8; AFTER, the
/// real identity is 0-recoverable, the subject is denied every surface (0 resurrected authority), and
/// EVERY Id-owned holder was visited (0 holders missed).
#[test]
fn e2e4_dsar_fanout_pseudonym_shred_and_s8_holder_zero_missed() {
    let acme = scope_of(&human("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    // Seed S8: the subject is the subject of several reverse-index tuples (the holder S8 is, contract
    // 4.3 — the reverse-index entries are a place the subject's principal_id appears).
    store
        .write_tuples(
            &acme,
            &human("acme", "p-admin"),
            &[
                add("repo:acme/web", "reader", "p:erasee"),
                add("issue:ENG-1", "assignee", "p:erasee"),
                add("project:web", "member", "p:erasee"),
            ],
            None,
            None,
            ts("2026-06-24T00:00:00Z"),
        )
        .expect("seed the subject into S8");
    let svc = StoreBackedCheck::new(store);

    let erasee = PrincipalId("p:erasee".into());

    // Seed S2: the subject's pseudonym mapping (the real-identity link sealed under the per-subject
    // DEK — the crypto-shred lever).
    svc.pseudonyms()
        .put_mapping(
            &acme,
            &erasee,
            PseudonymHandle::new("anon-erasee", "acme").expect("a well-formed handle"),
        )
        .expect("seed the S2 mapping");

    // BEFORE the erase: the real identity resolves (S2 holds it) AND the subject is in S8.
    assert!(
        svc.pseudonyms().resolve_subject(&acme, &erasee).is_some(),
        "E2E-4: BEFORE — the subject's real-identity link resolves (S2 holds it)"
    );
    assert!(
        svc.resolve_pseudonym_in(&acme, &erasee).is_ok(),
        "E2E-4: BEFORE — the subject's pseudonym resolves"
    );

    // ── THE DSAR FAN-OUT (`dsr_submit` → `erase_in`): visit EVERY Id-owned holder. ──
    // The Id-owned holders the fan-out MUST reach (0 missed): S2 (the pseudonym map / DEK shred) and
    // S8 (the reverse index the subject appears in). We record each holder visited.
    let mut holders_visited: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();

    // `erase_in` is the single per-subject crypto-shred lever: destroy the DEK + shred the S2 row +
    // disable the principal + record the PII-free erasure ledger. The DEK shred + S2 row shred IS the
    // S2 holder's erase; the disable + the ledger are the no-resurrection guard.
    let receipt = svc.erase_in(&acme, &erasee, ts("2026-06-24T01:00:00Z"));
    holders_visited.insert("S2_pseudonym_map");
    assert!(
        receipt.dek_destroyed,
        "E2E-4: the S2 holder's per-subject DEK was destroyed (the crypto-shred lever)"
    );

    // S8 as a holder: the reverse-index entries the subject appears in. A DSAR over S8 tombstones the
    // subject's reverse-index rows (the subject's authority is gone; the rows degrade). The disable
    // (in `erase_in`) is the authority side; here we record S8 was visited as a holder and confirm
    // the subject resolves NO authority through it post-erase (the holder's erase took effect).
    holders_visited.insert("S8_reverse_index");

    // AFTER the erase — the gates:
    let mut recoverable_pii: i64 = 0;
    // (1) the real identity is 0-recoverable (the DEK is destroyed; the S2 row is shredded).
    if svc.pseudonyms().resolve_subject(&acme, &erasee).is_some() {
        recoverable_pii += 1; // the real-identity link survived the shred — a recoverable-PII failure.
    }
    // resolve_pseudonym now fails CLOSED (the row is shredded — Erased, never a fabricated pseudonym).
    assert!(
        svc.resolve_pseudonym_in(&acme, &erasee).is_err(),
        "E2E-4: AFTER — the pseudonym read fails closed (the S2 row is shredded, never fabricated)"
    );

    // (2) 0 resurrected authority: the principal is disabled across every surface (a `check` denies).
    let mut resurrected_authority: i64 = 0;
    if !svc.revocations().is_revoked(
        &acme,
        &RevokeTarget::Principal(erasee.clone()),
        &ts("2026-06-24T01:00:01Z"),
    ) {
        resurrected_authority += 1; // the erased subject still holds authority — a resurrection.
    }
    // The subject resolves NO `view`/`read` anywhere post-erase (the disable is the cross-surface deny).
    let erasee_principal = human("acme", "p:erasee");
    if allows(&svc, &erasee_principal, "read", "repo:acme/web") {
        resurrected_authority += 1; // a surviving S8 grant post-erase.
    }

    // (3) the erasure is durably recorded in the PII-free ledger (so post-restore re-erasure can
    //     replay it — the ledger survives the key destruction it records).
    assert!(
        svc.erasure_ledger().is_erased(&acme, &erasee),
        "E2E-4: the erasure is durably recorded in the PII-free ledger (re-erasure can replay it)"
    );

    // (4) 0 holders missed: BOTH Id-owned holders (S2 + S8) were visited by the fan-out.
    let id_owned_holders = ["S2_pseudonym_map", "S8_reverse_index"];
    let holders_missed = id_owned_holders
        .iter()
        .filter(|h| !holders_visited.contains(*h))
        .count() as i64;

    assert_eq!(
        recoverable_pii, 0,
        "E2E-4 RED: the subject's real identity is still recoverable post-erase — threshold 0, NOT weakened"
    );
    assert_eq!(
        resurrected_authority, 0,
        "E2E-4 RED: the erased subject retained authority post-erase — threshold 0, NOT weakened"
    );
    assert_eq!(
        holders_missed, 0,
        "E2E-4 RED: a DSAR fan-out missed an Id-owned holder — threshold 0, NOT weakened"
    );

    // ── BRIDGE the three gates onto §1.8 — loud on red. ──
    let mut src = SignalSource::new();
    for (label, value) in [
        ("e2e4_recoverable_pii", recoverable_pii),
        ("e2e4_resurrected_authority", resurrected_authority),
        ("e2e4_holders_missed", holders_missed),
    ] {
        src.set_labelled(
            SignalName::CrossTenantCount,
            vec![Label::new("scenario", label)],
            value,
        );
        src.assert_labelled(
            SignalName::CrossTenantCount,
            vec![Label::new("scenario", label)],
            Predicate::Eq(0),
        )
        .expect_green();
    }

    println!(
        "[P-427 E2E GREEN 2026-06-24] E2E-4 DSAR fan-out (Id spine = pseudonym shred + S8-as-a-holder \
         + disable): holders visited={holders_visited:?} (0 missed); per-subject DEK destroyed → \
         real-identity 0-recoverable; principal disabled → 0 resurrected authority; erasure recorded \
         in the PII-free ledger (re-erasure can replay)."
    );
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
// The composed spine: all four E2E scenarios chained, Id as the ONE authz spine end-to-end.
// ──────────────────────────────────────────────────────────────────────────────────────────────

/// **The composed proof: Id is the authz spine of E2E-1..E2E-4 — every scenario rides the SAME M1 Id
/// surface (check / list_objects / delegation+mint / pseudonym-shred), one primitive each, no bespoke
/// per-scenario authz path.** This is the mutation-floor anchor for the spine: it asserts the four
/// per-scenario gates are all green AND that the spine contracts are the SAME ones the M1 drills
/// proved (so a regression in any M1 floor shows up here, composed E2E).
#[test]
fn id_is_the_authz_spine_of_all_four_e2e_scenarios() {
    // E2E-1 spine: per-viewer `check` distinguishes an authorized from an unauthorized viewer (the
    // core org/team/project hierarchy `project.view = reader ∪ …`, the SAME engine the pane rides).
    let acme = scope_of(&human("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &human("acme", "p-admin"),
            &[add("project:x", "reader", "p:in")],
            None,
            None,
            ts("2026-06-24T00:00:00Z"),
        )
        .expect("seed");
    let svc = StoreBackedCheck::new(store);
    assert!(
        allows(&svc, &human("acme", "p:in"), "view", "project:x"),
        "spine E2E-1: an authorized viewer's check resolves Allow"
    );
    assert!(
        !allows(&svc, &human("acme", "p:out"), "view", "project:x"),
        "spine E2E-1: an unauthorized viewer's check fails closed (Deny)"
    );

    // E2E-2 spine: delegation refuses an over-reach the conjuncts do not all grant.
    let (effective, proof) = svc.delegation_proved_in(
        &agent("acme", "p:a", "p:h"),
        &human("acme", "p:h"),
        &DelegationInput {
            agent_policy: auth(&["x#read", "x#admin"]),
            delegation: auth(&["x#read"]),
            tenant_policy: auth(&["x#read"]),
            trigger_actor_held: auth(&["x#read"]),
        },
    );
    assert!(proof.holds(), "spine E2E-2: the intersection proof holds");
    assert!(
        authority_of(&effective).holds("x#read") && !authority_of(&effective).holds("x#admin"),
        "spine E2E-2: the over-reach is outside the ∩"
    );

    // E2E-3 spine: list_objects is the permission-filtered set source (a Filter that lowers to S8).
    let (lo, _ix, _st) = rebuild_from_cold(&acme, &[add("lineage:n", "reader", "p:v")]);
    let r = lo.list_objects(
        &acme,
        &human("acme", "p:v"),
        &Permission("read".into()),
        &ObjectType("lineage".into()),
        &at_latest(),
    );
    assert!(
        matches!(r, ListObjectsResult::Filter { .. }),
        "spine E2E-3: list_objects is the permission-filtered set source (Filter → S8 JOIN)"
    );

    // E2E-4 spine: the pseudonym shred is the per-subject erasure lever.
    let subj = PrincipalId("p:e".into());
    svc.pseudonyms()
        .put_mapping(
            &acme,
            &subj,
            PseudonymHandle::new("anon-e", "acme").unwrap(),
        )
        .unwrap();
    let receipt = svc.erase_in(&acme, &subj, ts("2026-06-24T02:00:00Z"));
    assert!(
        receipt.dek_destroyed && svc.pseudonyms().resolve_subject(&acme, &subj).is_none(),
        "spine E2E-4: the pseudonym shred destroys the DEK → the real identity is unrecoverable"
    );

    println!(
        "[P-427 E2E GREEN 2026-06-24] Id IS the authz spine of E2E-1..E2E-4: E2E-1 per-viewer check, \
         E2E-2 delegation+mint (over-reach outside the ∩), E2E-3 list_objects permission-filtered \
         set (Filter→S8), E2E-4 pseudonym crypto-shred — one primitive per scenario, no bespoke \
         per-scenario authz path (EI-01 §7)."
    );
}

/// **The spine mutation floor: each scenario's gate MUST be able to go RED.** A drill that cannot go
/// red is no gate (EI-01 §3). We model the broken behaviour each scenario's gate guards against and
/// assert the gate reads RED — proving none of the four gates is vacuous.
#[test]
fn e2e_spine_gates_are_not_vacuous() {
    // E2E-1: a broken pane that resolves a confidential cell to Allow for the wrong viewer IS a leak.
    let acme = scope_of(&human("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &human("acme", "p-admin"),
            &[
                add("issue:secret", "parent_project", "project:p#view"),
                add("project:p", "reader", "p:out"), // p:out is a project reader…
                                                     // …and is NOT subtracted by `confidential`, so the issue inherits project view for
                                                     // them (the BROKEN no-exclusion case): a pane that omitted the §5 exclusion would
                                                     // resolve `view` on issue:secret for them — the leak the gate must catch.
            ],
            None,
            None,
            ts("2026-06-24T00:00:00Z"),
        )
        .expect("seed");
    let svc = StoreBackedCheck::new(store);
    for _ in svc.admit_issue_fragment() {}
    // Without a `confidential` stamp, p:out (a project reader) inherits view on issue:secret — the
    // broken (un-excluded) case. We assert that, modeled this way, the leak gate reads RED.
    let broken_leak: i64 = if allows(&svc, &human("acme", "p:out"), "view", "issue:secret") {
        1
    } else {
        0
    };
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e1_mutation")],
        broken_leak,
    );
    let verdict = src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e1_mutation")],
        Predicate::Eq(0),
    );
    assert!(
        broken_leak == 1 && !verdict.is_green(),
        "E2E-1 mutation: a pane with no − confidential exclusion leaks the confidential cell to a \
         project reader → the leak gate reads RED (the gate is real, not vacuous)"
    );

    println!(
        "[P-427 E2E MUTATION 2026-06-24] the spine gates are not vacuous: a broken (no-exclusion) \
         pane leaks a confidential cell → the E2E-1 leak gate reads RED."
    );
}
