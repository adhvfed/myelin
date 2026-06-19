# Sketch 05 — Authz (ReBAC), the merge gate, and wire authentication

> Exploration note. How git-specific authorization (push to protected ref, who-can-merge, CODEOWNERS)
> compiles onto the Phase-3 Identity ReBAC engine, and how SSH/HTTPS wire auth resolves a `Principal`.
> Identity owns the engine; we own our namespace fragment + the merge-gate logic. Date: 2026-06-19.

## What Identity already gives us (consume, don't reinvent)

- **`authenticate(credential) → Principal`** (contract 4.1) resolves SSH key / PAT / OAuth / agent
  token to a polymorphic `Principal{tenant, region, principal_id, kind, ...}`. **SSH-pubkey→Principal
  is Id's** (Phase-3 Id §4.2 row "SSH keys (git wire)") — we are the *consumer*, the front door.
- **`check(subject, permission, object, zookie?)`** (4.2) — per-action gate, fail-closed.
- **`list_objects(subject, permission, type, zookie?)`** (4.3) — the leak-free pre-filter for repo/PR
  lists and the PR context pane.
- **The Git ReBAC namespace fragment is ALREADY SEEDED** by Id §5 (we extend it):
  ```
  definition repo { reader/writer/admin; pull/push/administer/protected_push; parent_project rewrites }
  definition pull_request { author/reviewer; view/review/merge = parent_repo->protected_push }
  ```

## Our namespace fragment (what Git P4 declares — extends Id's seed)

We refine the seed with the constructs git review actually needs:

```
definition repo {
  relation parent_project: project
  relation reader: user | team#member
  relation writer: user | team#member
  relation maintainer: user | team#member          // can manage protections, dismiss reviews
  relation admin:  user
  permission pull          = reader + writer + maintainer + admin + parent_project->read
  permission push          = writer + maintainer + admin + parent_project->write
  permission administer    = admin + parent_project->admin
  permission manage_protect = maintainer + admin
}

definition ref_rule {                              // a branch-protection rule, scoped by ref glob
  relation parent_repo: repo
  relation bypass: user | team#member              // explicit bypass list (audited on use)
  // "protected_push to a matched ref requires the rule's gates met" — evaluated by the merge gate,
  // not a pure ReBAC permission (gates need check-state + review-count, which are ABAC/state, below)
}

definition pull_request {
  relation parent_repo: repo
  relation author:   user
  relation reviewer: user | team#member
  permission view   = parent_repo->pull
  permission comment = parent_repo->pull
  permission review = reviewer + parent_repo->push
  permission merge  = parent_repo->push            // GATED further by the merge gate (below)
}
```

- **CODEOWNERS as relations vs as evaluated policy** — the design call. CODEOWNERS maps **path globs →
  required reviewers**. Two options:
  - *(A) compile CODEOWNERS into ReBAC tuples* (`codeowner_of(path_glob)@team`) — clean for
    `list_subjects` ("who must approve") but explodes tuple count on big CODEOWNERS files and churns on
    every edit.
  - *(B) keep CODEOWNERS as an evaluated rule in the merge gate* — parse CODEOWNERS, compute required
    reviewers per changed-path-set at PR time, check each via `Id.check`. Less tuple churn; the
    "who must still approve" surface is computed.
  - **Leaning: (B)** — CODEOWNERS is *which paths need review by whom*, a function of the **diff**
    (changed paths), so it is inherently PR-state-dependent, not a static relation. We evaluate it in
    the merge gate and use `Id.list_subjects` to resolve a team to its members for the "who must still
    approve" UI.

## The merge gate — where "what is allowed to land" is decided

Branch protection is **not** a pure ReBAC permission, because it depends on **mutable PR/check state**
(required-review count met? required checks green? linear history? signed commits? stale reviews
dismissed?). So the merge gate is a **policy evaluator over (ReBAC permission ∩ ruleset ∩ live check
state)**:

```
can_merge(principal, pr) =
     Id.check(principal, merge, pr)                         # ReBAC: are you allowed to merge at all?
  && ruleset(pr.target_ref).required_approvals_met(pr)      # count of approving reviews from eligible reviewers
  && ruleset.required_checks_all_green(pr)                  # consumes ci.run.* check state (sketch, the Git↔CI seam)
  && ruleset.codeowners_satisfied(pr)                       # option B above
  && ruleset.linear_history_ok(pr) && ruleset.signatures_ok(pr)
  && !pr.has_unresolved_required_threads
```

- **Required checks** come from CI via `ci.run.passed|failed` events (the **Git↔CI checks/commit-status
  contract** — the most load-bearing cross-subsystem seam, Phase-2 §9; jointly designed in the
  architecture stage). The check aggregator (control plane) maintains per-commit check state; the merge
  gate reads it.
- **Bypass** is audited: using a `ref_rule.bypass` grant emits `git.protection.bypass_used`
  (audit-critical — sketch 03/08).

## Agent-vs-human merge policy (rides delegation, AG-2)

Agents are **principals subject to the same gate** (Phase-2 §7.4 — "agents are subject to branch
protection like any principal"). An "agent PRs require human approval" rule is a **ruleset predicate on
`actor.kind = agent`**: when the PR author (or merger) is an agent, the rule adds a *required human
approval* gate. The agent's effective policy is `agent.policy ∩ delegation ∩ tenant.policy`
(`delegation` contract 4.5) — an agent literally cannot merge a protected ref unless policy grants it,
because the **same merge gate stands in front of it as a human** (EI-02 §2). A sensitive agent merge
returns `Gated` from `EffectApi` (contract 8.2) → a durable HITL card (sketch — user-flows).

## Wire authentication (the front door)

- **SSH:** the front door runs an in-process SSH server (or `AuthorizedKeysCommand`-equivalent) that
  takes the client pubkey → calls `Id.authenticate(ssh_key)` → `Principal`; then for the requested repo
  op runs `Id.check(principal, pull|push, repo)` **before** invoking `upload-pack`/`receive-pack`. Deploy
  keys are machine `Principal`s (Id §4 machine identities).
- **Smart-HTTP:** `Authorization: Bearer <PAT>` / basic-with-token → `Id.authenticate` → `Principal` →
  `Id.check`. Protocol v2 default (better for huge ref counts — Phase-1 §6).
- **Every entrypoint is gated** — SSH, HTTPS, API, UI, CLI, and the event-triggered agent path
  (Phase-1 §8). The front door also enforces **residency** (reject any route leaving the region — ADR-11)
  and **backpressure** (the protected-human-lane shed order, contract 1.11 — a clone-storm of agents/CI
  sheds before an interactive human push).

## Leaning (committed in findings)

Compile collaborator/team roles → the **Git ReBAC namespace fragment** (extending Id's seed) with
`reader/writer/maintainer/admin` + `pull/push/manage_protect`. **CODEOWNERS evaluated in the merge gate**
(diff-dependent), not compiled to tuples. The **merge gate = ReBAC permission ∩ ruleset ∩ live check
state** (required checks from CI events). **Agents ride the same gate**; "agent-needs-human" is a
ruleset predicate on `actor.kind`. Wire auth via **Id.authenticate (SSH pubkey / token)** then
**Id.check per op** at a residency- and backpressure-enforcing front door.

## Prior art / sources

- Zanzibar ReBAC (Pang et al., USENIX ATC 2019); SpiceDB userset rewrites; Phase-3 Id §5 (the seeded
  Git namespace), §8 (`check`/`list_objects`/`list_subjects`), §7 (delegation algebra).
- CODEOWNERS-as-policy (GitHub/GitLab evaluate required reviewers from the changed path set).
- EI-02 §2 (one identity model for humans + agents; same gate in front of both).
- Phase-2 git-hosting §7.3 (touchpoints), §7.4 (agents subject to protection).
