# 06 — Required Shared-System Changes (Phase-5 reconciliation list)

> Itemized, explicit changes git hosting needs from the shared systems that are **not already in the
> Phase-3 contracts** (or that sharpen/confirm a Phase-3 seam). Each is tagged for the Phase-5
> reconciliation agent with: what, why, the consuming git surface, and whether it is a NEW request or a
> CONFIRMATION of an existing Phase-3 seam. Date: 2026-06-19.

---

## Identity & Access

| # | Request | Why / consuming surface | Status |
|---|---|---|---|
| **CR-ID-1** | **Efficient ref-pattern-scoped relations + CODEOWNERS-as-relations.** ReBAC must express "relation scoped by a ref glob" (e.g. `protected_ref` matching `release/*`) and CODEOWNERS path-glob → required-reviewer relations, and answer `list_subjects(pr, review)` / `check(push_protected)` at push QPS without materialising a relation per ref. | The branch-protection evaluator + merge gate + "who must still approve" (`03 §5.2`, `04 §2.2`). A naive tuple-per-ref explodes. | **NEW** (sharpens Id §5 namespace + contract 4.9) |
| **CR-ID-2** | **`list_objects` push-down over a repo/PR id column at scale** for permission-aware repo/PR lists and the context-pane pre-filter (S-10 confirmed for Search/Refs; confirm for git's repo/PR lists). | repo list, PR list, the context pane (`04 §2.2`). | CONFIRM (contract 4.3 / S-10) |
| **CR-ID-3** | **`resolve_pseudonym` / `erase` keyed on the git author pseudonym** must be the DSR step-1 lever for git (delete the map ⇒ commit bytes hold only the pseudonym). Confirm the pseudonym grammar `<pseudonym>@<tenant>.noreply` and that Id mints/owns it. | GIT-1 commit-time pseudonymisation (`05 §HP-7`). | CONFIRM (contract 4.8 / Id §11) + **NEW** pseudonym grammar pin |
| **CR-ID-4** | **SSH-pubkey / deploy-key / PAT → Principal** machine-identity resolution with the git front door as the consumer; deploy keys are repo-scoped machine principals. | the SSH + smart-HTTP front door (`02 §1`). | CONFIRM (Id §4 / contract 4.1) |

## Event Bus

| # | Request | Why | Status |
|---|---|---|---|
| **CR-BUS-1** | **Per-ref aggregate ordering at git-push QPS** sustained without lost/ghost events — the aggregate is the ref, not the PR. | `git.ref.updated` order per ref (`02 §2-3`). | CONFIRM (Bus §2.3 — explicitly designed; the **D-9 drill** is shared) |
| **CR-BUS-2** | **`git.*` taxonomy registration** — git hosting owns the complete dotted-name list in §1 of `03`; Bus registers them under the grammar. | the taxonomy (`03 §1`). | CONFIRM (Bus §6 grammar; P4 completes the list — done here) |

## Reference Graph

| # | Request | Why | Status |
|---|---|---|---|
| **CR-REF-1** | **Git sub-artifact `#sub` grammar + outdated/tombstoned-anchor semantics**: `#comment-<uuid>`, `#thread-<uuid>`, `#L42-L88` (content-anchored line range). A line-range sub whose content no longer exists resolves to a **partial/tombstone** projection of the parent. | diff-anchored comments, code-line embeds (`02 §5`, `03 §2`). | CONFIRM (Refs §3.5 / contract 5.7 — git's `#sub` minting is git's; the *outdated-line-range* case is the new specificity) |
| **CR-REF-2** | **Edge production from commit trailers + PR links** (`Closes ISSUE-412`, `Co-authored-by`) → `refs.edge.created` via the outbox. | linked issues, the context pane (`04 §2.2`). | CONFIRM (Refs §4.1 / contract 5.4) |

## Search

| # | Request | Why | Status |
|---|---|---|---|
| **CR-SRCH-1** | **Code-shaped `IndexSpec`** accepting the git code projection (path/symbols/literals/commit-message + trigram text) with a camel/snake code tokenizer, incremental on push, ACL-aware via `list_objects` over the repo object. | code search v1 (`02 §9`, `05 §HP-9`). | CONFIRM (Search §4.4 / contract 6.5 — the exact per-blob/per-symbol projection event is the **git P4 deliverable**, defined here) |
| **CR-SRCH-2** | **Consume CI-produced SCIP/LSIF** as a later index input for "find usages". | GF-3 follow-on. | NEW (future; named) |

## Storage

| # | Request | Why | Status |
|---|---|---|---|
| **CR-STOR-1** | **Object-backed pack/delta management over `BlobStore`** + the smart-transport read path served from object-tier blobs (the STOR-5 follow-on). | HP-1/HP-6, GF-1 (`02 §4`). | CONFIRM the seam (Storage §3.5 / STOR-5); the **implementation is the git P4 deliverable** (TE-24) |
| **CR-STOR-2** | **Clone/bundle artifacts as a cached, residency-pinned, CDN-within-EU distributable** for hot-repo/clone-storm. | bundle-URI accelerated clone (`02 §1.4`). | **NEW** (a named blob class + a within-EU CDN posture beyond the base `BlobStore`) |
| **CR-STOR-3** | **Crypto-shred granularity reaching reflogs, bitmaps, and pack-tier backups** via the per-tenant blob DEK; **per-subject DEK for PR/review/comment bodies**. | the erasure algorithm (`03 §6.1`, `05 §HP-7`). | CONFIRM (Storage §5.3-5.4 / GD-4) |

## Durable Workflow

| # | Request | Why | Status |
|---|---|---|---|
| **CR-WF-1** | **The merge queue + auto-merge-when-green as first-class durable state machines**, woken by `ci.result`/`approval` durable **signals** (possibly days later for a HITL gate). | the merge gate + queue (`02 §6.2`). | CONFIRM (Workflow §3.4 / contract 9.4) |
| **CR-WF-2** | **Maintenance (large repack / bundle gen / history-rewrite) as resumable activities** through reserve/settle. | GC/maintenance at fleet scale (`02 §8`). | CONFIRM (contract 9.5 + 11.7) |

## Agent Fabric

| # | Request | Why | Status |
|---|---|---|---|
| **CR-AG-1** | **`agent_needs_human` enforced as a HITL gate** in `git.merge`/`git.open_pr` on protected refs (an agent cannot bypass required human approval unless delegation allows). | the agent-vs-human merge policy (`02 §6.1`, `03 §7`). | CONFIRM (ADR-08 / AG-8 / contract 8.2) |
| **CR-AG-2** | **Agent legibility metadata** (`is_agent`, `agent_run`, provenance) carried through so the agent-aware review surface renders agents distinctly. | the agent-aware review surface (`04 §2.2`). | CONFIRM (ADR-08 AI-Act labelling) |

## GDPR / Audit + Legal

| # | Request | Why | Status |
|---|---|---|---|
| **CR-GDPR-1** | **The GD-1 reconciliation gates the git data model** (pseudonymous-commit-by-default is a commit-time prerequisite); Legal/DPO must **decide the Art. 17 residual** (reach into immutable commit bytes vs. documented limit). | `05 §HP-7`. | CONFIRM (GDPR §7 / GD-1 / L-2) — **`[OPEN — LEGAL]`** owed |
| **CR-GDPR-2** | **History-rewrite as an audited, tamper-evident-logged, rate-limited tenant op** with fork/mirror/clone-cache invalidation. | the erasure-admin tool (`04 §2.3`). | **NEW** (the audited-op + invalidation fan-out is a git-specific audit surface) |

## Tenancy / Control plane

| # | Request | Why | Status |
|---|---|---|---|
| **CR-TEN-1** | **`placement_of(repo)` → cell + placement group**, region-pinned, with repos **relocatable** (the STOR-5 relocatability), and `discover` usable by the **git wire** front door. | front-door routing, residency reject (`02 §1`, `00 §2A`). | CONFIRM (Tenancy contract 12.2/12.3) — repo-granular placement is the new specificity |
| **CR-TEN-2** | **Push-mirror to a foreign host is a residency boundary crossing → policy-gated** at the control plane. | mirror config (`04 §2.3`). | **NEW** (residency policy gate on outbound mirror) |

---

**Summary of genuinely NEW asks** (vs. confirmations): CR-ID-1 (ref-glob-scoped relations + CODEOWNERS),
CR-ID-3 pseudonym-grammar pin, CR-STOR-2 (within-EU clone-bundle CDN class), CR-SRCH-2 (SCIP, future),
CR-GDPR-2 (audited history-rewrite + invalidation fan-out), CR-TEN-2 (mirror residency gate). Everything
else confirms an existing Phase-3 seam.
