# 03 — Events, Contracts & Glue (against the FROZEN reconciled shapes)

> The complete `git.*` event taxonomy this subsystem owns + the events it consumes (incl. the X-1
> `ci.check.updated` / `ci.result`); and how it implements **every** glue contract against the **frozen**
> shapes: `ArtifactRef` + the unified `#sub` grammar (5.7), `project(ref, viewer)` (5.6), `replay(scope,
> since)` (2.6), the envelope via the outbox (2.1/2.2), Identity `check`/`list_objects` **SetExpr
> push-down** (4.2/4.3) + the frozen ReBAC fragment with `approve_untrusted_ci` (4.9), `PersonalDataHolder`
> (10.1) + the ONE erasure posture by reference (10.9), `ToolDef`s with the **frozen `requires_approval`
> defaults** (8.1) + the four uniform sandbox guarantees (8.4), and reserve/settle (11.7). Date: 2026-06-19.

---

## 1. The complete `git.*` event taxonomy (owned)

Under the Bus §6 grammar `type = <subsystem>.<artifact_type>.<event_name>` (singular, past-tense), the
`git` subsystem token and the `repo`/`pr`/`commit`/`branch`/`tag`/`review`/`comment`/`ref` type tokens (Bus
§6.2 token table — the names authority, contract 14). Every event carries the canonical `EventEnvelope`
(`event_id` ULID, `type`, `schema_ver`, `tenant`, `region`, `actor`, `subject` ArtifactRef, `aggregate`,
`correlation_id`/`causation_id`/`depth`, `contains_personal_data`/`data_role`/`visibility`/`pii_key_ref`,
`occurred_at`/`recorded_at`, `payload` — contract 2.1, the names/units anchor). **All actor identity is
pseudonymous** (GIT-1, contract 4.8).

| Event | Aggregate (ordering key) | Notes |
|---|---|---|
| `git.repo.created` / `git.repo.deleted` / `git.repo.archived` / `git.repo.transferred` | `git/repo/<id>` | |
| `git.repo.visibility_changed` | `git/repo/<id>` | drives Search/Refs ACL recompute |
| `git.repo.forked` | `git/repo/<child>` | fork-network creation |
| `git.branch.created` / `git.branch.deleted` | `git/repo/<id>` | derivable from `ref.updated`; emitted for convenience |
| `git.branch.protection_changed` | `git/repo/<id>` | ruleset change (incl. the `required_contexts` policy) |
| **`git.ref.updated`** | **`git/ref/<repo>:<ref>`** | the core push event: `{repo, ref, old_oid, new_oid, forced, commit_oids[], pusher_pseudonym}`. **Per-ref aggregate** (contract 2.3). CI/Search/Refs/Agents consume. |
| `git.tag.created` / `git.tag.deleted` | `git/repo/<id>` | |
| `git.pr.opened` / `git.pr.updated` / `git.pr.marked_ready` / `git.pr.closed` / `git.pr.reopened` / `git.pr.merged` / `git.pr.synchronized` | `git/pr/<n>` | `synchronized` = PR head moved (re-anchor + re-gate) |
| `git.review.requested` / `git.review.submitted` / `git.review.dismissed` | `git/pr/<n>` | `submitted` carries verdict + `is_agent` |
| `git.comment.created` / `git.comment.resolved` / `git.thread.resolved` | `git/pr/<n>` | inline + thread; sub-ids `#comment-<id>` / `#thread-<id>` |
| `git.pr.merge_blocked` / `git.pr.merge_queued` | `git/pr/<n>` | merge-queue surfacing |
| `git.check.gate_evaluated` | `git/pr/<n>` | the Git-owned merge-gate outcome (NOT a CI fact — Git emits this off its own projection) |
| `git.codeowners.review_required` | `git/pr/<n>` | |
| **`git.protection.bypass_used`** | `git/repo/<id>` | **audit-critical** (a bypass of branch protection; contract 10.6) |
| `git.fork.ci_endorsed` | `git/pr/<n>` | a maintainer endorsed an `untrusted_fork` run via `approve_untrusted_ci` (X-1, audit-relevant) |
| `git.repo.erased` / `git.pr.erased` / `git.comment.erased` | (the erased aggregate) | the cross-cutting `*.erased` tombstone (contract 2.7) |
| `git.repo.snapshot` / `git.pr.snapshot` / `git.blob.snapshot` / `git.comment.snapshot` | (the snapshotted aggregate) | the `*.snapshot` reindex-from-source events for `replay` (contract 2.6) |

`key.added` / `token.created` are **echoed from Identity** (Id owns them); git hosting does not originate
them. **Git does NOT emit `ci.*`** — the check facts are CI's (the dependency is acyclic: CI emits, Git
reads — EI-02 §3).

### 1.1 Events CONSUMED (idempotent on `event_id`, `consumer_dedup` ledger)

| Event | From | Effect |
|---|---|---|
| **`ci.check.updated`** | **CI** | the X-1 consumer: apply `run_attempt` supersession into the `check_status` projection (`02 §6.1`); carries the frozen `CheckStatus` struct. **The single most load-bearing cross-subsystem seam** (contract 5.9). |
| **`ci.result`** (durable signal, not a consumed event per se) | **CI** | the rollup signal that wakes the merge-queue durable workflow (`02 §6.4`, contract 9.4) — `{commit_oid, overall, contexts, idem_token}`, idempotent on `idem_token`. |
| `ci.artifact.published` (SCIP/LSIF) | CI | (GF-3 follow-on) consume code-intelligence indices for "find usages" (contract 6.5). |
| `identity.permission.granted\|revoked` / `identity.member.added\|removed` | Identity | recompute who-can-review/merge; invalidate CODEOWNERS resolution cache; the authz reverse index Identity maintains keeps `list_objects` fresh (contract 4.3). |
| `*.erased` (subject) | GDPR/Bus | the erasure path — see §6. |
| `issue.issue.closed` | Issues | reflect `Closes <ISSUEKEY>` auto-close linkage state. |
| Agent `ProposedEffect`s (open PR / review / comment / merge) | Agent Fabric | arrive via `EffectApi::apply` (plan-then-apply), never as direct writes — see §7. |

---

## 2. `ArtifactRef` + the unified `#sub` scheme (contracts 5.1, 5.7) — FROZEN

Git hosting mints `myelin://<tenant>/git/<type>/<id>[#<sub>]` using the canonical `git` token + the type
tokens above (contract 14 token table). The **`<id>` segment is the stable mintable canonical key Git owns**
— the commit sha, the PR number, the repo id — **never a render-time display form** (the REF-3
reconciliation, contract 5.1: display keys are render-time, never the stored link). This is Git's parallel
of the Issues `<PROJECTKEY>-<seqno>` decision: Git's sha / PR-number *is already* its stable canonical key,
so there is no contradiction to reconcile beyond pinning that the stored ref uses it.

The **frozen unified `#sub` grammar** (contract 5.7 — Git mints these stable opaque sub-ids; Refs owns the
grammar + the resolution ladder):

| ArtifactRef | `#sub` kind | Meaning |
|---|---|---|
| `myelin://acme-eu/git/repo/<repo_id>` | — | a repository |
| `myelin://acme-eu/git/pr/<repo>:<n>` | — | a pull request |
| `myelin://acme-eu/git/pr/<repo>:<n>#comment-<comment_id>` | `comment-` | a PR review comment (stable opaque id, survives edits) |
| `myelin://acme-eu/git/pr/<repo>:<n>#thread-<thread_id>` | `thread-` | a review thread root (shared with Chat's `thread-` kind, OQ-L) |
| `myelin://acme-eu/git/commit/<repo>:<sha>` | — | a commit (the sha is immutable) |
| `myelin://acme-eu/git/blob/<repo>:<ref>:<path>#L42-L88` | `L<a>-L<b>` | a **content-anchored** file line-range (the OQ-D fingerprint anchor, `02 §5`) |
| `myelin://acme-eu/git/ref/<repo>:<ref>` | — | a branch/ref |

`#sub` ids are **opaque and stable** (the stability obligation is Git's, Refs §3.5): a comment's
`#comment-<id>` does not change when the comment is edited; the *anchor position* may transition
`live→moved→outdated→gone`, but the comment id and the parent PR ref persist. **Line-range subs are
content-anchored**: Git is the owner's sub-anchor resolver the Refs ladder calls — it returns
`LIVE/MOVED/OUTDATED/GONE` and Refs maps that to `projection / projection+moved / projection(partial)+
outdated / Tombstone{root, content_gone}` (the one 4-step ladder, contract 5.7; `02 §5`). The
`check-<context>` and `step-<n>` sub kinds belong to **CI** (they appear in `CheckStatus.details_ref`); Git
resolves a `details_ref` only by rendering it as a link into CI's run view, never by reading CI's DB.

---

## 3. `project(ref, viewer) → {title, state, icon, render_hint, sub_anchor?}` (contract 5.6) — REQUIRED

The **only** way Refs/Search/Notif read about a git artifact (no cross-DB). Per-viewer,
pre-permission-checked. Git implements `project` for every `git` type:

```rust
fn project(ref: ArtifactRef, viewer: Principal) -> Projection | Tombstone {
    // 1. permission: Id.check(viewer, view, ref.acl_object())  → deny ⇒ Tombstone (never leak)
    // 2. load the artifact (own DB), build a per-viewer projection:
    match ref.typ {
      Pr     => { title: pr.title, state: pr.state /*open|merged|draft*/, icon: "pr",
                  render_hint: { checks: gate_state_summary(pr) /*green/red/neutral*/,
                                 approvals: n/m, is_draft, trust: pr.trust_tier },
                  sub_anchor: ref.sub.map(|c| comment_excerpt(c)) }
      Commit => { title: short_sha + first_line(message), state: verified?, icon: "commit", ... }
      Blob   => { title: path, state: ref+line_range, icon: "file",
                  render_hint: { language, snippet }, sub_anchor: line_range_state /*live/moved/.../*/ }
      Repo   => { title: slug, state: visibility, icon: "repo", ... }
    }
}
```

- **Display mode** returns the humanisation projection Notif uses (NOTIF-1 / contract 7.3: humanise at the
  backend with a routable `ArtifactRef` + a `(template_key, args)` pair; raw ids / `"merge_request merged"`
  are forbidden). The `CheckStatus.summary` is itself a `HumanisedRef` (template_key, args), so the PR
  checks panel never renders a CI-supplied raw string.
- **Erasure-safe**: a projection of an erased/tombstoned artifact returns a tombstone ("(deleted)"), never
  leaking erased content (contract 5.2).
- **Restriction-safe**: if the viewer's subject is restricted (the GDPR `restrict` flag, §6), the projection
  omits restricted content.
- **Cross-cell**: resolution is **always cell-local** (contract 5.2 / OQ-I) — a viewer in cell A resolving a
  PR homed in cell B has cell B run `project` (permission-checked in B); only the rendered projection
  crosses, never raw rows. (Single-home-cell is v1; this is the named multi-cell floor.)

This API serves both the **live unfurl/embed** (Refs `resolve` on cache miss) and the **Search text
projection** fetch (Search §5.3).

---

## 4. `replay(scope, since)` (contract 2.6) — reindex-from-source

Git emits `git.*.snapshot` events via the outbox **through the live consumer path** (never reading owner
DBs from outside; the rebuild uses the same code path as steady state):

- `replay(repo, since)` → re-emit `git.repo.snapshot`, `git.pr.snapshot` per PR, `git.blob.snapshot` per
  indexed blob (the code projection, `02 §9`), `git.comment.snapshot` per comment. **Sub-artifact-granular**
  (contract 2.6): a single comment or a single blob line-range can be replayed.
- Search rebuilds the code index from cold (drill D-3); Refs rebuilds edges; Notif read-models rebuild.
- **The `check_status` projection rebuilds the other way**: it is Git's *consumer* projection of CI's facts,
  so Git rebuilds it by asking the bus to `reindex` CI's `ci.check.updated` for the scope (CI's `replay`
  re-emits the per-run snapshots; Git's consumer re-applies supersession). The projection is **derived,
  never restored** (contract 2.6/11.5: derived stores are rebuilt, not restored).
- Because `replay` re-emits through the outbox, there is no drift between steady-state indexing and cold
  rebuild (the reindex-from-source invariant, EI-04 §5).

---

## 5. Identity: `check` / `list_objects` + the ReBAC namespace fragment (FROZEN, contract 4.9)

### 5.1 Authz at every entrypoint

`Id.check(subject, permission, object, zookie?, caveat?)` runs at **every** entrypoint — SSH, smart-HTTP,
API, UI, CLI, and the event-triggered agent path. Fail-closed on uncertainty (contract 4.2). The front door
authenticates to a `Principal` (`Id.authenticate`, contract 4.1; Id owns the SSH-key/deploy-key/PAT/token→
principal map) and gates each action. Security-sensitive transitions (protected-ref merge, fork
endorsement) carry a **zookie** (read-your-writes; bypasses the fail-static cache, contract 4.10).

### 5.2 The ReBAC namespace fragment (contract 4.9) — declared by Git, engine owned by Id, **frozen**

Git declares its namespace fragment (compiled into the one cell schema; Id owns the engine, never invents
object IDs). Reconciliation **froze** this fragment (identity §5 C4) — ref-glob-scoped relations +
CODEOWNERS-as-relations + the new **`approve_untrusted_ci`** relation:

```
definition repo {
  relation parent_project: project
  relation reader: user | team#member
  relation writer: user | team#member
  relation admin:  user
  relation approve_untrusted_ci: user | team#member     // X-1 fork-endorsement (FROZEN, identity §5 C7)
  permission pull           = reader + writer + admin + parent_project->read
  permission push           = writer + admin + parent_project->write
  permission administer     = admin + parent_project->admin
  permission protected_push = admin                      // tighter; the merge/protected-ref gate
}
definition ref {                                          // ref-PATTERN-scoped (the ref-glob relation)
  relation parent_repo: repo
  relation bypass:      user | team#member               // the audited bypass list
  relation code_owner:  user | team#member               // CODEOWNERS path-glob → reviewer-requirement tuples
  permission push_protected = bypass + parent_repo->administer
}
definition pull_request {
  relation parent_repo: repo
  relation author:      user
  relation reviewer:    user | team#member
  permission view    = parent_repo->pull
  permission review  = reviewer + parent_repo->push
  permission merge   = parent_repo->protected_push        // agent_needs_human enforced in the merge gate
}
definition pr_comment {
  relation parent_pr: pull_request
  permission view = parent_pr->view
}
// watcher relation per watchable type (Notif read-fanout, identity §5 C8):
//   repo, pull_request each declare `relation watcher: user`
```

- **`approve_untrusted_ci` is an ordinary relation**, so the fork-endorsement gate (`02 §6.3`) is a plain
  `check(subject, approve_untrusted_ci, repo)` — not bespoke logic (identity §5 C7). The `trust_tier` is
  stamped by **CI** from run provenance + the `read & !is_untrusted_fork` ABAC edge; **Git reads it off the
  `CheckStatus` fact and never recomputes it.**
- **Roles compiled to relations** — `collaborator`/`maintainer`/`admin` in the settings UI write tuples via
  `Id.write_tuples` (contract 4.6), returning a zookie to stamp (`page.acl_zookie`-style).
- **CODEOWNERS-as-relations**: the resolver compiles `CODEOWNERS` path globs into `code_owner` relations per
  ref pattern, so "who must approve this path" is a `list_subjects(pr, review)` query (contract 4.4, served
  by the authz reverse index — performant at member density).

### 5.3 `list_objects` — the frozen `SetExpr` push-down (contract 4.3, OQ-E)

`list_objects(viewer, pull, repo)` / `(viewer, view, pull_request)` returns **`Ids{ids, zookie}`** (small
sets, materialised) **or** **`Filter{set_expr, zookie}`** (large/unbounded). For the `Filter` path, Git's
query compiler **lowers the `SetExpr` into a SQL predicate / JOIN over Git's own id column** (`repo.id` /
`pr.id`) against Identity's **per-tenant authz reverse index** (`authz_visible`):

```sql
-- repo/PR list, board, context-pane pre-filter — ONE query, no N+1, no post-filter:
SELECT ... FROM pull_request p
  JOIN authz_visible av
    ON av.object_id = p.id AND av.subject = $viewer AND av.relation = 'view'
 WHERE p.tenant = $tenant ...
-- (Ids/NotIds lower to IN/NOT IN; Union/Intersect/Difference to AND/OR/EXCEPT — contract 4.3)
```

This is the SC-1 leak-and-slowness fix (the `search-requires-acl-filter` / `tenant-predicate` lints, ADR-03:
pre-filter not post-filter). The returned `zookie` bounds staleness; a security-sensitive scan passes the
zookie so the read reflects a just-revoked grant. **Field/transition ABAC is not needed here** — Git has no
field-level row hiding; its only `CaveatContext` use (contract 4.2) would be a future per-path visibility
overlay, off this hot path.

---

## 6. `PersonalDataHolder` (contract 10.1, holder H1) — the hardest in the platform

Git registers as a `PersonalDataHolder` (auto-registered by `serve`) implementing `locate/export/rectify/
restrict/erase`. Erasure is **purge/crypto-shred/pseudonymise, never hide**.

```
locate(subject)  → all PRs/reviews/comments authored by subject's pseudonym; repos owned; refs/reflog
                   entries; LFS blobs uploaded; the git-identity↔user pseudonym mapping ref.
export(subject)  → a portable bundle (Art. 20): the subject's content + a `git clone` of repos they may
                   export, as a MerkleProvenBundle (via GDPR/Audit, contract 10.4).
rectify(subject) → update hosting-layer text the subject controls (their comment bodies, PR titles).
restrict(subject)→ set the restriction flag: NO indexing / NO agent-use / NO analytics / NO notification
                   for the restricted subject (the platform-wide `restrict` suppression, contract 10.1).
erase(subject)   → the algorithm below.
```

### 6.1 The erasure algorithm (DSR fan-out, Git's part)

```
erase(subject, tenant):                       # invoked by the DSR orchestrator (GDPR §4)
  1. (Id step 1, platform) Id.erase(subject)  → deletes the pseudonym map ⇒ commit-object bytes, reflog,
                                                 and event-log now hold only the opaque pseudonym (4.8).
  2. KMS.destroy(DEK[subject:<id>])           → crypto-shreds the subject's inline-PII: PR/review/comment
                                                 BODIES + titles (ciphertext under the PER-SUBJECT DEK, 11.4)
                                                 live AND in backups; reflogs/bitmaps/pack backups shreddable
                                                 via the per-tenant blob DEK (Storage §5); cache/CDN (H9).
  3. Search.purge + reindex(subject)          → drop the subject's code/PR/comment index docs + rebuild.
  4. Refs.tombstone(subject)                  → unfurls/backlinks degrade to "(deleted)" (relies on the
                                                 pseudonym shred; backlinks are projections, rebuilt).
  5. emit git.*.erased tombstones via outbox  → consumers drop derived state.
  6. record an erase receipt (GDPR/Audit, the carve-out audit holder H16).
  RESIDUAL: third-party free-text PII (a name typed by SOMEONE ELSE into their own un-erased commit message
            / comment body) and immutable commit-message bytes authored by others → handled by THE ONE
            PLATFORM ERASURE POSTURE (contract 10.9 / recon §X-7). NOT restated here. See `05 §HP-7`.
```

### 6.2 The ONE platform erasure posture — instantiated BY REFERENCE (contract 10.9 / X-7)

Per the reconciliation directive, this subsystem does **not** author a Git-local residual statement. The
residual — third-party/immutable free-text PII authored by others — is handled per the **ONE platform-wide
posture in `00-reconciliation §X-7` / contract 10.9**:

- **Structural floor (built now):** per-subject DEK crypto-shred (self-authored bodies, step 2) +
  pseudonym-map shred (identity, step 1) + the `restrict` suppression (never indexed, never agent-readable,
  never in analytics for a restricted subject).
- **Git's instantiation of the residual levers:** (a) **pseudonymous-commit-by-default (GIT-1)** so the
  immutable hash never bakes erasable author PII in the first place; (b) the **history-rewrite erasure path**
  for the rare case a body must be expunged — an **audited, tamper-evident, rate-limited tenant op** (contract
  10.6) with **fork/mirror/clone-cache invalidation fan-out** (the trust-scoped cache namespaces, Storage
  11.2 C4), with the understood consequence of changed hashes.
- **`[OPEN — LEGAL]`:** the lawful basis + documented limit for residual third-party/immutable free-text
  PII, and the Art. 17 reach into immutable git bytes, are ratified by **counsel/DPO as ONE statement, not
  five** (recon §X-7). The structural floor ships regardless.

### 6.3 The restriction flag

A **restricted** subject (GDPR Art. 18) gets **no indexing, no agent-use, no analytics, no notification**.
Git enforces it at each seam: the code-projection emitter (`02 §9`) skips restricted content; the agent
dispatch path will not act on a restricted subject's artifacts; the OLAP feed excludes them; `project` omits
restricted content. A single `restricted` flag keyed on the subject's pseudonym, checked at each seam.

---

## 7. `ToolDef` registrations (contract 8.1) + the agent action path — frozen defaults

Git registers typed `ToolDef`s into the shared `ToolSurface` (name + JSON-schema input + required caps +
effect kind + side-effecting flag + `requires_approval` + `exposed_over_mcp`). The **`requires_approval`
defaults are the frozen X-6 table** (contract 8.1):

| Tool | Effect kind | Side-effecting | `requires_approval` (FROZEN, X-6) | Notes |
|---|---|---|---|---|
| `git.read_file` | read | no | no | read a blob (ACL-checked) |
| `git.read_diff` | read | no | no | read a PR/compare diff |
| `git.search_code` | read | no | no | the code-search projection (ACL pre-filtered) |
| `git.open_pr` | mutate | yes | **no** (reversible — X-6 table) | proposes a PR (plan-then-apply) |
| `git.comment` | mutate | yes | no | inline/thread comment (agent legibly labelled) |
| `git.submit_review` | mutate | yes | no | approve/request-changes/comment |
| `git.suggest_change` | mutate | yes | no | a committable suggestion |
| `git.resolve_thread` | mutate | yes | no | |
| `git.merge` | mutate | **yes — consequential** | **yes** (the consequential gate — X-6 table, AG-8) | the merge gate enforces `agent_needs_human` |

**Plan-then-apply (ADR-08, contract 8.2).** Agents never write directly. An agent proposes a
`ProposedEffect`; `EffectApi::apply` validates `schema → capability → delegation → tenant → budget → HITL
gate → apply via the public endpoint → meter`. A denied effect returns an ordinary `Denied` tool error
(AG-8) — agents are subject to branch protection like any principal; a **withheld** gated tool (e.g.
`git.merge` whose approval card is not approved) does **not** mutate. **Agent legibility (ADR-08 AI-Act):**
agent authors/reviewers render visually distinct with provenance (which agent, why, the run) and are never
disguised as humans — `review.is_agent` + `agent_run` carry this.

**The four uniform sandbox guarantees (X-6, contract 8.4)** apply by construction to any Git tool that
executes code (the history-rewrite activity, SCIP indexing if run as a job, any future tenant check): (1)
the reserve/settle cost gate (11.7); (2) execution under a per-run attenuated token (`mint_run_token`, 4.7);
(3) HITL withhold (privileged mutation goes through `EffectApi`, never `ToolHands::exec`); (4) the isolation
floor + the real-kernel escape drill. Git does not re-implement these — `ToolHands::exec` **is** the CI
runner's `kind=agent` job on the unified sandbox.

**Triggers** ("on `git.pr.opened` matching pattern, dispatch agent review") are first-class via the shared
reactive tier: the `EventMatcher` **= the frozen `myelin-query` `QueryAst`** (contract 3.4), wired to a
**Signal** (consumers subscribe to curated Signals, not the raw `evt.*` firehose), with the structural loop
guards (AG-6) applied by the dispatch tier. **Explicit-first dispatch** (CHAT-1, contract 8.6): a mention of
an agent reviewer *notifies*; it does not auto-spawn a costed run — the human (or an explicit automation)
triggers it, and reserve/settle gates even the explicit run.

---

## 8. reserve/settle (contract 11.7) — where git runs spend-bearing work

Git is mostly **not** spend-bearing in the per-request path (a push/clone is not metered compute). The
spend-bearing surfaces:

- **Agent runs invoked from git** (an agent review/PR) — metered by the Agent Fabric's reserve/settle gate,
  not by git; git emits the trigger.
- **Heavy maintenance compute** (large repacks, bundle generation, **history-rewrite**, SCIP indexing) — run
  as **CI jobs / durable-workflow activities** through the universal reserve/settle gate (contract 11.7 /
  CI-2): reserve at dispatch, settle on completion, refuse-start-on-exhaustion, never interrupt in flight,
  meter one cost event per unit. These meter into the **same wallet** as CI/agent runs (Commercial C-1).
- **A `CheckStatus` is not "final" until `cost_settled = true`** (the X-1 field): the merge gate may treat a
  not-yet-settled check as still-in-progress, so the reserve/settle bookend is visible to the gate.

Git **declares** its spend-bearing activities to the gate and relies on the shared gate; it does not
implement metering or own the wallet.
