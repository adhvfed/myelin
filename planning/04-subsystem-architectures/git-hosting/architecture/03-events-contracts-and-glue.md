# 03 — Events, Contracts & Glue

> The complete `git.*` event taxonomy this subsystem owns (under the Bus §6 grammar) + the events it
> consumes; and how it implements **every** glue contract: `ArtifactRef`, `project(ref, viewer)`,
> `replay(scope, since)`, the envelope via the outbox, Identity `check`/`list_objects` + the ReBAC
> namespace fragment, `PersonalDataHolder` (locate/export/rectify/restrict/erase + the restriction flag),
> `ToolDef` registrations, and reserve/settle. Date: 2026-06-19.

---

## 1. The complete `git.*` event taxonomy (owned)

Under the Bus §6 grammar `type = <subsystem>.<artifact_type>.<event_name>` (singular, past-tense), the
`git` subsystem token and the `repo`/`pr`/`commit`/`branch`/`tag`/`review`/`comment`/`ref` type tokens
(Bus §6.2). Every event carries the canonical envelope (`event_id` ULID, `type`, `schema_ver`, `tenant`,
`region`, `actor{principal,kind,on_behalf_of,session,run}`, `subject` ArtifactRef, `aggregate`,
`correlation_id`/`causation_id`/`depth`, `contains_personal_data`/`data_role`/`visibility`/`pii_key_ref`,
`occurred_at`/`recorded_at`, `payload`). **All actor identity is pseudonymous** (GIT-1).

| Event | Aggregate (ordering key) | Notes |
|---|---|---|
| `git.repo.created` / `git.repo.deleted` / `git.repo.archived` / `git.repo.transferred` | `git/repo/<id>` | |
| `git.repo.visibility_changed` | `git/repo/<id>` | drives Search/Refs ACL recompute |
| `git.repo.forked` | `git/repo/<child>` | fork-network creation |
| `git.branch.created` / `git.branch.deleted` | `git/repo/<id>` | derivable from `ref.updated`; emitted for convenience |
| `git.branch.protection_changed` | `git/repo/<id>` | ruleset change |
| **`git.ref.updated`** | **`git/ref/<repo>:<ref>`** | the core push event: `{repo, ref, old_oid, new_oid, forced, commit_oids[], pusher_pseudonym}`. **Per-ref aggregate** (Bus §2.3). CI/Search/Refs/Agents all consume. |
| `git.tag.created` / `git.tag.deleted` | `git/repo/<id>` | |
| `git.pr.opened` / `git.pr.updated` / `git.pr.marked_ready` / `git.pr.closed` / `git.pr.reopened` / `git.pr.merged` / `git.pr.synchronized` | `git/pr/<n>` | `synchronized` = PR head moved |
| `git.review.requested` / `git.review.submitted` / `git.review.dismissed` | `git/pr/<n>` | `submitted` carries verdict + `is_agent` |
| `git.comment.created` / `git.comment.resolved` / `git.thread.resolved` | `git/pr/<n>` | inline + thread |
| `git.check.required_failed` | `git/pr/<n>` | merge-gate signal |
| `git.pr.merge_blocked` / `git.pr.merge_queued` | `git/pr/<n>` | merge-queue surfacing |
| `git.codeowners.review_required` | `git/pr/<n>` | |
| **`git.protection.bypass_used`** | `git/repo/<id>` | **audit-critical** (a bypass of branch protection) |
| `git.repo.erased` / `git.pr.erased` / `git.comment.erased` | (the erased aggregate) | the cross-cutting `*.erased` tombstone (Bus §6.3) |
| `git.repo.snapshot` / `git.pr.snapshot` / `git.blob.snapshot` | (the snapshotted aggregate) | the `*.snapshot` reindex-from-source events for `replay` (Search/Refs cold rebuild) |

`key.added` / `token.created` are **echoed from Identity** (Id owns them); git hosting does not originate
them.

### 1.1 Events CONSUMED (idempotent on `event_id`, `consumer_dedup` ledger)

| Event | From | Effect |
|---|---|---|
| `ci.run.passed` / `ci.run.failed` / `ci.run.started` | CI | update `check_status`; feed the merge gate; signal the merge-queue workflow. **The Git↔CI checks contract is the most load-bearing cross-subsystem seam** (jointly owned, `06 §CR-CI`). |
| `ci.artifact.published` (SCIP/LSIF) | CI | (GF-3 follow-on) consume code-intelligence indices for "find usages". |
| `identity.permission.granted|revoked` / `identity.member.added|removed` | Identity | recompute who-can-review/merge; invalidate CODEOWNERS resolution cache. |
| `*.erased` (subject) | GDPR/Bus | the erasure path — see §6. |
| `issue.issue.closed` | Issues | reflect `Closes #N` auto-close linkage state. |
| Agent `ProposedEffect`s (open PR / review / comment / merge) | Agent Fabric | arrive via `EffectApi` (plan-then-apply), never as direct writes — see §5. |

---

## 2. `ArtifactRef` + sub-artifact `#sub` scheme (contract 5.1, 5.7)

Git hosting mints `myelin://<tenant>/git/<type>/<id>[#sub]` using the canonical `git` token and the
type tokens above (Bus §6.2). The **`#sub` scheme** (stable opaque sub-ids, stable across edits so embeds
don't dangle, contract 5.7):

| ArtifactRef | Meaning |
|---|---|
| `myelin://acme-eu/git/repo/<repo_id>` | a repository |
| `myelin://acme-eu/git/pr/<repo>:<n>` | a pull request |
| `myelin://acme-eu/git/pr/<repo>:<n>#comment-<comment_id>` | a PR comment (stable UUID, survives edits) |
| `myelin://acme-eu/git/pr/<repo>:<n>#thread-<thread_id>` | a comment thread |
| `myelin://acme-eu/git/commit/<repo>:<sha>` | a commit (the sha is immutable) |
| `myelin://acme-eu/git/blob/<repo>:<ref>:<path>#L42-L88` | a file line-range (the code-search anchor) |
| `myelin://acme-eu/git/ref/<repo>:<ref>` | a branch/ref |

`#sub` ids are **opaque and stable**: a comment's `#comment-<uuid>` does not change when the comment is
edited or when the diff is re-anchored (the *anchor position* may go `outdated`, but the ref still
resolves to the parent PR's projection with the sub-anchor — Refs §3.5). Line-range subs
(`#L42-L88`) are content-anchored; if the line range no longer exists, Refs returns a partial/tombstone
projection (`02 §5`, Refs §4.6).

---

## 3. `project(ref, viewer) → {title, state, icon, render_hint, sub_anchor?}` (contract 5.6) — REQUIRED

The **only** way Refs/Search/Notif read about a git artifact (no cross-DB). Per-viewer,
pre-permission-checked. Git hosting implements `project` for every `git` type:

```rust
fn project(ref: ArtifactRef, viewer: Principal) -> Projection | Tombstone {
    // 1. permission: Id.check(viewer, view, ref.acl_object())  → deny ⇒ Tombstone (never leak)
    // 2. load the artifact (own DB), build a per-viewer projection:
    match ref.typ {
      Pr     => { title: pr.title, state: pr.state /*open|merged|draft*/, icon: "pr",
                  render_hint: { checks: green/red, approvals: n/m, is_draft },
                  sub_anchor: ref.sub.map(|c| comment_excerpt(c)) }
      Commit => { title: short_sha + first_line(message), state: verified?, icon: "commit", ... }
      Blob   => { title: path, state: ref+line_range, icon: "file",
                  render_hint: { language, snippet }, sub_anchor: line_range }
      Repo   => { title: slug, state: visibility, icon: "repo", ... }
      // …
    }
}
```

- **Display mode** returns the humanisation projection Notif uses (NOTIF-1: humanise at the backend with
  a routable `ArtifactRef`; raw ids/`"merge_request merged"` are forbidden).
- **Erasure-safe**: a projection of an erased/tombstoned artifact returns a tombstone ("(deleted)"),
  never leaking the erased content (Refs §4.6).
- **Restriction-safe**: if the viewer's subject is restricted (the GDPR restriction flag, §6), the
  projection omits restricted content.

This API serves both the **live unfurl/embed** (Refs `resolve` on cache miss) and the **Search text
projection** fetch (Search §5.3).

---

## 4. `replay(scope, since)` (contract 2.6) — reindex-from-source

Git hosting implements `replay(scope, since)` emitting `git.*.snapshot` events via the outbox through the
**live consumer path** (never reading owner DBs from outside; the rebuild uses the same code path as
steady state — SEARCH-1/REF-4):

- `replay(repo, since)` → re-emit `git.repo.snapshot`, `git.pr.snapshot` per PR, `git.blob.snapshot` per
  indexed blob (the code projection, `02 §9`), `git.comment.snapshot` per comment. **Sub-artifact-
  granular** (contract 2.6 requires it): a single comment or a single blob line-range can be replayed.
- Search uses it to rebuild the code index from cold (the **reindex-from-cold parity drill**, `07
  §D-3`); Refs uses it to rebuild edges; Notif read-models rebuild from it.
- Because `replay` re-emits through the outbox, it is the *same path* a live push takes — there is no
  drift between steady-state indexing and cold rebuild (the doctrine's reindex-from-source invariant).

---

## 5. Identity: `check` / `list_objects` + the ReBAC namespace fragment

### 5.1 Authz at every entrypoint

`Id.check(subject, permission, object, zookie?)` runs at **every** entrypoint — SSH, smart-HTTP, API,
UI, CLI, and the event-triggered agent path (Phase-1 §8). Fail-closed on uncertainty (contract 4.2). The
front door authenticates to a `Principal` (`Id.authenticate`, contract 4.1; Id owns the SSH-key/PAT/
token→principal map) and gates each action. Security-sensitive transitions (protected-ref merge) carry a
**zookie** (read-your-writes; bypasses the fail-static cache, contract 4.10) so a just-granted permission
is honoured.

### 5.2 The ReBAC namespace fragment (contract 4.9) — declared by Git, engine owned by Id

Git hosting declares its namespace fragment (compiled into the one cell schema; Id owns the engine, never
invents object IDs). This **builds on the Id-seeded git fragment** (identity-and-access.md §5) and
extends it for branch protection, CODEOWNERS-as-relations, and reviews:

```
definition repo {
  relation parent_project: project
  relation reader: user | team#member
  relation writer: user | team#member
  relation admin:  user
  permission pull           = reader + writer + admin + parent_project->read
  permission push           = writer + admin + parent_project->write
  permission administer     = admin + parent_project->admin
  permission protected_push = admin                  // tighter; the merge/protected-ref gate
}
definition protected_ref {                            // a ref-PATTERN-scoped object (NEW — REQUEST CR-ID-1)
  relation parent_repo: repo
  relation bypass:      user | team#member            // the audited bypass list
  // CODEOWNERS compiles to per-path-pattern required-reviewer relations:
  relation code_owner:  user | team#member
  permission push_protected = bypass + parent_repo->administer
}
definition pull_request {
  relation parent_repo: repo
  relation author:      user
  relation reviewer:    user | team#member
  permission view    = parent_repo->pull
  permission review  = reviewer + parent_repo->push
  permission merge   = parent_repo->protected_push    // agent_needs_human is enforced in the merge gate
}
```

- **Roles are the authoring face, compiled to ReBAC relations** — `collaborator`/`maintainer`/`admin` in
  the settings UI write tuples via `Id.write_tuples` (contract 4.6), returning a zookie to stamp.
- **CODEOWNERS-as-relations**: the resolver compiles `CODEOWNERS` path globs into `code_owner` relations
  per protected-ref pattern, so "who must approve this path" is a `list_subjects(pr, review)` query —
  but **efficient ref-glob-scoped relations are a shared-system request** (CR-ID-1, `06`).
- `list_objects(viewer, pull, repo)` powers permission-aware repo/PR lists and the PR context pane
  pre-filter (no leak/no N+1, contract 4.3) — the single most load-bearing inter-system contract here.

---

## 6. `PersonalDataHolder` (contract 10.1) — the hardest in the platform

Git hosting registers as a `PersonalDataHolder` (auto-registered by `serve`) implementing
`locate/export/rectify/restrict/erase` over the inventory in `01 §4.4`. Erasure is **purge/crypto-shred/
pseudonymise, never hide**.

```
locate(subject)  → all PRs/reviews/comments authored by subject's pseudonym; repos owned; refs/reflog
                   entries; LFS blobs uploaded; the subject's git-identity↔user pseudonym mapping ref.
export(subject)  → a portable bundle of the above (Art. 20) as a MerkleProvenBundle (via GDPR/Audit).
rectify(subject) → update hosting-layer text the subject controls (their comment bodies, PR titles).
restrict(subject)→ set the restriction flag: NO indexing / NO agent-use / NO analytics / NO notification
                   for the restricted subject (the platform-wide restriction semantics, §below).
erase(subject)   → the algorithm below.
```

### 6.1 The erasure algorithm (DSR fan-out, git's part)

```
erase(subject, tenant):                       # invoked by the DSR orchestrator (GDPR §4)
  1. (Id step 1, platform) Id.erase(subject)  → deletes the pseudonym map ⇒ commit-object bytes,
                                                 reflog, and event-log now hold only the opaque pseudonym.
  2. KMS.destroy(per_subject_DEK)             → crypto-shreds the subject's inline-PII: PR/review/comment
                                                 BODIES (ciphertext under the subject DEK) live AND in
                                                 backups; reflogs/bitmaps/backups of the pack tier
                                                 (shreddable via the per-tenant blob DEK — Storage §5.4).
  3. Search.purge + reindex(subject)          → drop the subject's code/PR/comment index docs + rebuild.
  4. Refs.tombstone(subject)                  → unfurls/backlinks degrade to "(deleted)" placeholders.
  5. emit git.*.erased tombstones via outbox  → consumers drop derived state.
  6. record an erase receipt (GDPR/Audit, the carve-out audit holder).
  RESIDUAL: personal data in non-pseudonymised file CONTENT / commit messages → the GD-1 levers
            (history-rewrite OR documented lawful-basis limit), see 05 §HP-7. NOT solved by 1-6.
```

### 6.2 The restriction flag

Per VISION/GDPR Art. 18, a **restricted** subject must get **no indexing, no agent-use, no analytics, no
notification** (the platform-wide restriction semantics, README §5). Git hosting enforces it: the
code-projection emitter (`02 §9`) skips restricted subjects' content; the agent dispatch path will not
act on a restricted subject's artifacts; the OLAP feed excludes them; `project` omits restricted content.
A single `restricted` boolean keyed on the subject's pseudonym, checked at each of those seams.

---

## 7. `ToolDef` registrations (contract 8.1) + the agent action path

Git hosting registers typed `ToolDef`s into the shared `ToolSurface` (name + JSON-schema input + required
caps + effect kind + side-effecting flag + `requires_approval` default + `exposed_over_mcp`). Governed
once, exposable over MCP later (GF-9).

| Tool | Effect kind | Side-effecting | `requires_approval` default | Notes |
|---|---|---|---|---|
| `git.read_file` | read | no | no | read a blob (ACL-checked) |
| `git.read_diff` | read | no | no | read a PR/compare diff |
| `git.search_code` | read | no | no | the code-search projection (ACL pre-filtered) |
| `git.open_pr` | mutate | yes | **on protected repos / for agents under `agent_needs_human`** | proposes a PR (plan-then-apply) |
| `git.comment` | mutate | yes | no | inline/thread comment (agent legibly labelled) |
| `git.submit_review` | mutate | yes | no | approve/request-changes/comment |
| `git.suggest_change` | mutate | yes | no | a committable suggestion |
| `git.resolve_thread` | mutate | yes | no | |
| `git.merge` | mutate | **yes — sensitive** | **yes on protected refs** (HITL-gateable) | the merge gate enforces `agent_needs_human` |

**Plan-then-apply (ADR-08).** Agents never write directly. An agent proposes a `ProposedEffect`;
`EffectApi::apply` validates `schema → capability → delegation → tenant → budget → HITL gate → apply via
the public endpoint → meter` (contract 8.2). A denied effect returns an ordinary `Denied` tool error
(AG-5) — agents are subject to branch protection like any principal. **Agent legibility (ADR-08 AI-Act):**
agent authors/reviewers are rendered visually distinct with provenance (which agent, why, the run) and
are never disguised as humans — `review.is_agent` + `agent_run` carry this.

**Triggers** ("on `git.pr.opened` matching pattern, dispatch agent review") are first-class via the
shared reactive tier (`EventMatcher` over the query AST, ADR-07/Bus §4.7), wired to a **Signal**
(consumers subscribe to curated Signals, not the raw `evt.*` firehose, BUS-4), with the structural loop
guards (AG-6: self-guard, reference gate, causal-depth ceiling) applied by the dispatch tier.

---

## 8. reserve/settle (contract 11.7) — where git runs spend-bearing work

Git hosting is mostly **not** spend-bearing in the per-request path (a push/clone is not metered compute).
The spend-bearing surfaces are:

- **Agent runs invoked from git** (an agent review/PR) — metered by the Agent Fabric's reserve/settle
  gate, not by git hosting; git just emits the trigger.
- **Heavy maintenance compute** (large repacks, bundle generation, history-rewrite, SCIP indexing if it
  runs as a job) — these run as **CI jobs / durable-workflow activities** and pass through the universal
  reserve/settle gate (CI-2/D8): reserve at dispatch, settle on completion, refuse-start-on-exhaustion,
  never interrupt in flight, meter one cost event per unit. Git hosting declares these as spend-bearing
  workflow activities; it does not own the wallet (Commercial C-1).

So git hosting **declares** its spend-bearing activities to the gate and otherwise relies on the shared
gate; it does not implement metering itself.
