# Git hosting — Key user flows

> Phase-4 design sketch. The key flows including the **agent/HITL flows** (proposed effects, approval
> cards, attribution) and the **cross-subsystem flows** git hosting participates in. Each flow names the
> shared contracts it rides so the architecture stage builds to them. Date: 2026-06-19.

Legend: `[Id]` Identity, `[Bus]` event bus/outbox, `[Refs]` reference graph, `[Search]`, `[Notif]`,
`[Agent]` agent fabric, `[Flow]` durable workflow, `[CI]`. Plan-then-apply = agents propose effects;
`EffectApi` validates+applies (never a direct write).

---

## Flow 1 — Clone / fetch / push (the wire path)

1. `git clone git@myelin…:acme/app` → front door takes the SSH pubkey → `[Id].authenticate` →
   `Principal`.
2. `[Id].check(principal, pull, repo)` → allow → front door resolves placement (`[Tenancy].placement_of`,
   residency-checked: reject any route leaving region) → streams `upload-pack` (protocol v2).
3. `git push origin feat/login` → `[Id].check(principal, push, ref)` → **in-process receive-pack**:
   objects to quarantine → **policy gate** (branch protection, secret-scan, size, signed, agent rules)
   → if accepted: **one DB txn** {migrate objects via `BlobStore`, CAS ref tip, insert `git.ref.updated`
   outbox row} → ack. (sketch 03)
4. The relay drains the outbox → `[Bus]` delivers `git.ref.updated` (per-ref ordered) → CI/Search/Refs/
   Agents react async. Commit author is the **pseudonym** (sketch 09); envelope `contains_personal_data
   = false`.

**States:** rejected push → non-fast-forward / policy-violation error (clear, actionable; what rule,
how to fix). Residency-blocked route → explicit "this repo is region-pinned" error. Backpressure: a
clone-storm of agents/CI sheds before an interactive human (the protected-human lane, contract 1.11).

---

## Flow 2 — Open a PR, review, merge (the centrepiece)

1. Dev pushes `feat/login`; opens **PR create** (base `main`, head `feat/login`). The `Closes ISSUE-412`
   trailer → Git emits `ref.created` (PR→issue edge) → `[Refs]`. PR row + `git.pr.opened` outbox.
2. **PR overview** shows: description (shared `myelin-content` editor), linked issue (CONTEXT PANE via
   `[Refs]`+projection, permission-filtered), required-checks summary (from `[CI]` `ci.run.*`),
   merge-readiness, timeline.
3. Reviewer opens **Files changed**: diff (unified/split), places inline comments → **batches** them
   (start → batch → submit verdict, §5.5) → `git.pr.review_submitted`. Comments anchor on blob-SHA+line
   (sketch 07); on a later force-push they relocate or go **outdated**.
4. CODEOWNERS "who must still approve" computed in the merge gate (sketch 05); required reviewers
   resolved via `[Id].list_subjects`.
5. Dev enables **auto-merge-when-green** → a `[Flow]` durable workflow waits on the check aggregator;
   on all-required-checks-green + approvals-met + threads-resolved → **merge gate** passes → linearizable
   protected-ref update → `git.pr.merged` → `[Refs]` auto-closes the linked issue (cross-subsystem).

**States:** empty PR list (onboarding: "open your first PR"); merge **blocked** (which gate is unmet,
inline); conflict (indicated; resolve locally — in-UI resolution is the named follow-on, sketch 08);
permission-denied review target → context-pane stub.

---

## Flow 3 — Request an agent review (agent-native, plan-then-apply, attribution)

1. On the PR, dev clicks **"Request agent review"** → `git.agent_review_requested` arms a trigger
   (`EventMatcher` over the query AST) for the `code-review` agent.
2. The agent wakes on `git.pr.opened`/`synchronized` via `[Bus]` dispatch → `[Agent]` runs
   plan-then-apply → proposes effects `{git.comment ×N, git.submit_review}` (it **proposes, never
   acts**).
3. `[Agent].EffectApi.apply` validates each effect: schema → capability → `agent.policy ∩ delegation ∩
   tenant.policy` → budget (reserve/settle) → **apply via the public Git endpoint** (no carve-out) →
   meter.
4. The review appears on the **agent-aware review surface**: a **visually distinct agent comment**
   (the agent treatment — never disguised as human; no sparkle/magic iconography, DL §8b.3), showing
   **which agent, why (provenance), delegation/scope**, and a link to the **tamper-evident audit
   trail**. Humans can **dismiss/override** it.

**Attribution rule (P7/AI-Act):** every agent action shows its actor incl. on-behalf-of; the agent
badge is the §5.11 identity treatment with the agent variant. Agent volume is kept **out of the primary
notification stream** (calm-by-default).

---

## Flow 4 — Sensitive agent action → HITL approval card (withhold → approve → resume)

The agent-native flagship (Phase-2 §6.2):

1. `[CI]` emits `ci.run.failed` on `main` → a triage agent (via `[Bus]` dispatch, plan-then-apply) opens
   `ISSUE-412`, links it `[Refs]`, posts to `[Chat]`.
2. A fix agent proposes **`git.open_pr` on a protected repo** — a *sensitive* effect → `[Agent].EffectApi`
   returns **`Gated(gate_id)`** → opens a `[Flow]` durable HITL gate. The gated write tool is **withheld**
   (returns an error, does not mutate) until approval (AG-8).
3. The **approval card** renders (primarily in `[Chat]`, also in the **Notif inbox** and inline on the
   PR area): shows the agent's **proposed effects** (the plan), its **identity/scope/delegation**, and a
   **live cost estimate**, with **Approve / Edit / Reject** (§5.4 / §6.3). Humanised strings come from
   the backend (`[Notif].humanise` + `[Refs]` display resolution — DL §8b.5), never a frontend map.
4. A human approves **days later** → the `[Flow]` workflow resumes (durable signal), re-mints the
   agent's short-lived run token mid-workflow (S-11), re-runs the step → `git.open_pr` **applies** →
   `git.pr.opened`. The PR shows the agent author legibly with the approval provenance.

**States:** pending-approval (the §5.10 agent-pending state); rejected → the effect is discarded, the
run records the denial; expired gate (HITL timeout on the timer wheel) → re-surfaces / lapses per policy.

**Branch-protection invariant:** the agent is subject to the **same merge gate as a human** (sketch 05)
— an "agent PRs require human approval" ruleset means the agent *cannot* merge the protected ref without
the human gate. There is no privileged agent path (AG-5: a denied effect is an ordinary tool error).

---

## Flow 5 — The PR context pane (the wedge, cross-subsystem, permission-filtered)

`GET pr #88` → Git checks viewer authz → asks `[Refs]` for PR #88's edges → `[Refs]` pre-filters targets
via `[Id].list_objects` → Git resolves each surviving `ArtifactRef` via the **owning subsystem's
projection API** (Issues issue, Knowledge doc section, CI run, Chat thread) → assembles a pane showing
**only what the viewer may see**, kept **live** by bus update events. **No subsystem touched another's
DB** (Phase-2 §6.3). A reference the viewer can't see → permission-stub. An erased target → tombstone.

---

## Flow 6 — Code search (permission-pre-filtered)

1. User types in ⌘K or the search view → query compiled to the shared query AST.
2. `[Search].query(ast, viewer)` **conjoins `[Id].list_objects(viewer, read, repo)` before scoring**
   (the `search-requires-acl-filter` lint — you can only find what you may see, P9).
3. Results: path/symbol/literal/trigram hits across the repos the viewer can see, with type facets.
   The index was built from **Git's code projection** (sketch 06), incremental on push; Git owns *what*
   to index, Search owns the index.

**States:** empty (no matches / no repos visible); restricted author's code is **not indexed** (Art. 18
restriction); HYOK content → "not searchable" (storage §6.1, surfaced honestly).

---

## Flow 7 — Erasure / restriction (DSR fan-out; we are holder H1)

1. `[GDPR].dsr_submit(erase, subject)` → DSR orchestrator → fan-out step 1 = `[Id].erase(subject)`
   (pseudonym-map delete) → every downstream holder now sees only the opaque pseudonym.
2. Git's `PersonalDataHolder.erase`: pseudonymise authorship (already pseudonymous — the map delete did
   it) + **crypto-shred** PR/review/comment free-text (per-subject DEK) + tombstone refs in `[Refs]` +
   `[Search]` purges+reindexes the subject's projections.
3. **Residual (PII in file content / legacy history):** the **history-rewrite admin tool** (audited,
   hash-changing, rate-limited; emits `git.repo.history_rewritten`) — surfaced with explicit
   fork/mirror/CDN-invalidation warnings. Reaches replicas/reflogs/bitmaps/backups/bundles; foreign
   mirrors are policy-gated & documented (sketch 09).
4. **Restriction** (Art. 18): a restricted subject's code is **not indexed/agent-used/analysed/notified**
   while storage is retained — reversibly.

**States:** the erasure admin surface is **destructive + confirmed** (the §6.3 carve-out: GDPR/agent/
irreversible actions still confirm despite the reversibility-over-confirmation default). Tombstoned
artifacts render the §5.10 erased state.

---

## Flow 8 — Branch protection / ruleset edit (admin, progressive disclosure)

Maintainer opens **Settings → Branch protection** → ruleset editor (ref patterns, required approvals,
required checks, dismiss-stale, signed/linear, force/delete bans, **bypass lists**, **agent rules**).
Saving emits `git.branch.protection_changed`. Using a bypass later emits `git.protection.bypass_used`
(audit-critical → audit log). The editor is a deep progressive-disclosure surface (simple defaults,
power on demand — P4).

## Cross-references
- design-language §5.3/§5.4/§5.5/§6 (chip/unfurl, HITL card, comments, agent UX), §8b.5 (humanisation).
- Contracts: 4.x (Id), 2.x (Bus/outbox), 5.x (Refs/project), 6.x (Search), 7.x (Notif), 8.x (Agent/
  EffectApi), 9.x (Flow/HITL), 10.x (GDPR/holder). Sketches 03/05/06/07/09.
