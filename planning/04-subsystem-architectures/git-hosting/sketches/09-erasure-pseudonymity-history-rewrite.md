# Sketch 09 — Git-history erasure: pseudonymous-by-default, history-rewrite, the residual (GIT-1 / GD-1)

> Exploration note. The single hardest problem in the platform (Phase-1 §11.1). We OWN git-history
> author/email erasure (Phase-3 handoff). Pseudonymous-commit-by-default is a **commit-time
> prerequisite that gates the git data model** (GIT-1) — decided BEFORE the data model is fixed. This
> is co-owned with Legal/DPO and is the named GD-1 reconciliation (gdpr-and-audit §7). Date: 2026-06-19.

## What the platform already solved (consume, don't re-derive)

The spine solves the **event-log half** and everything keyed under a destroyable DEK (storage §5,
gdpr §7.1): crypto-shred + references-not-payloads + pseudonym indirection ("delete the identity, not
the fact"). `Id.erase(subject)` (contract 4.8) deletes the pseudonym map; `KMS.destroy(per_subject_DEK)`
shreds free-text. The DSR orchestrator fans out to every `PersonalDataHolder` (we are **H1**, gdpr
§3.4). **What the spine did NOT solve: the git-history half** — author name/email **baked into the
commit hash** (gdpr §7.1; storage §5.4). That is *our* problem to reconcile.

## The personal data we hold (the H1 holder map)

| Data | Where | Erasure lever |
|---|---|---|
| Commit author/committer **name+email** | **baked into commit-object bytes → the SHA** | pseudonymous-by-default (below) makes the bytes hold only an opaque pseudonym; else history-rewrite |
| PR/review/comment **authorship** | control-plane rows | pseudonymise (Id pseudonym-map delete) |
| PR/review/comment **free-text bodies** | control-plane OLTP | crypto-shred per-subject DEK |
| **File content / commit messages** with PII | commit-object bytes | history-rewrite (no crypto-shred reaches it — immutable, hash-load-bearing) |
| **LFS blobs** with PII | object tier (BlobStore) | crypto-shred per-tenant/per-subject blob DEK (storage §5) |
| SSH-key fingerprints / push records | control-plane | pseudonymise + short-TTL retention |
| git-identity ↔ Myelin-user mapping | Id pseudonym map | the erasure lever itself (delete the map) |

## The decision that gates the data model — pseudonymous-by-default (GIT-1, DECIDED)

**Commits are authored to a stable opaque pseudonym at commit time.** The commit object's
author/committer identity is `<pseudonym>@<tenant>.noreply` (a stable opaque id per
`(tenant, person)`); the **person↔pseudonym mapping lives only in Id's erasable pseudonym map** (S2).
Erasing the person deletes the map ⇒ the immutable commit bytes hold **only the opaque pseudonym** —
no PII baked into any hash, ever (gdpr §7.2 row 1).

- **This MUST be enforced at commit time** (the GIT-1 prerequisite) — it cannot be bolted on later
  because the bytes are immutable. So the **push path** (sketch 03) is where it lands: on
  `receive-pack`, the incoming commit's author identity is **resolved to the pseudonym** before the
  objects leave quarantine. Two enforcement modes:
  - **Server-side rewrite-to-pseudonym at push** (the commit is authored to the pseudonym as it lands)
    — guarantees the property regardless of client config, at the cost of the client's local sha ≠
    server sha (a known, documented consequence; the server is authoritative).
  - **Client-cooperative** (the Myelin CLI / git config sets the pseudonymous identity) — preserves
    client/server sha equality, but can't be *guaranteed* for arbitrary stock clients.
  - **Leaning:** **default to pseudonymous identity surfaced to clients** (the CLI configures it; the
    UI/web-edit authors as the pseudonym), with **server-side enforcement as a per-repo policy** for
    tenants who require the guarantee. The architecture stage finalises the enforcement mode; the
    *property* (no raw PII in commit bytes by default) is DECIDED now.
- **The display name** (`git log` showing a real name) is a **render-time projection** (Refs
  `resolve`/Notif humanise resolve the pseudonym → current display name per viewer, contract 5.2/7.3)
  — so the developer still *sees* real names in the UI, but the *stored bytes* are pseudonymous. This
  resolves the "developers expect their real name in git log" tension (Phase-1 §9.2): the name is shown,
  not stored. (For raw `git log` over the wire, the committed author is the pseudonym; the Myelin UI
  projects the display name.)

## The residual — PII in file content / legacy history (the half that needs rewrite)

Pseudonymity handles **metadata**. It does **not** handle PII committed into **file content** or
**commit messages**, or **legacy non-pseudonymised history** (imports). For that, the only lever is:

- **Supported history-rewrite** (filter-repo-class): tenant-initiated, audited, rate-limited; **changes
  every downstream hash** (invalidates clones/signatures/refs/forks/mirrors/CDN clone caches). Emits
  `git.repo.history_rewritten` (audit-critical). The admin surface warns explicitly about fork/mirror/
  CDN invalidation (Phase-2 §4.3 erasure/redaction admin).
- **Crypto-shred reaches the *pack tier's* reflogs/bitmaps/backups** (those are shreddable via the
  per-tenant blob DEK — storage §5.4) — but **NOT the commit-object bytes themselves** (immutable,
  hash-load-bearing). So history-rewrite is the only path to the bytes.
- **Distributed-erasure reach** (Phase-1 §9.2/§11.1, Phase-2 §8.5): erasure must reach **replicas,
  reflogs, packs, bitmaps, backups, mirrors, and CDN clone caches** with defined SLAs. Our design: the
  rewrite is applied at the authoritative repo + propagated to replicas (sketch 01); reflogs/bitmaps
  expired + repacked; bundle/CDN caches invalidated (content-addressed → new content = new cache key);
  **push-mirrors to foreign hosts are a residency boundary and policy-gated** (Phase-1 §9.3) — a mirror
  we don't control is a documented limit.

## The documented lawful-basis residual (`[OPEN — LEGAL]`, GD-1/L-2)

Per gdpr §7.3: the **exact Art. 17 reach into immutable commit-object bytes** — and whether a
documented "technically infeasible / disproportionate effort" lawful-basis limit suffices for the
residual — is **decided by counsel/DPO**, not engineering. Our engineering posture (gdpr §7.3):
**minimise PII in immutable history so the legal question rarely bites** — pseudonymous-by-default does
exactly that for metadata; history-rewrite is the (disruptive) tool for content. The **residual limit
is a named gap-report floor with Legal as the follow-on owner** (E-3).

## Restriction (Art. 18) — the often-forgotten obligation

A *restricted* subject (not erased, but processing-suspended) must have **no indexing/agent-use/
analytics/notification** while storage is retained (contract: honour the restriction flag; gdpr §4.6 /
drill GA-9). For us: a restricted author's commits stay served (storage retained) but their code
projection is **not indexed**, agents don't act on their behalf, and their activity doesn't notify —
reversibly. The push path checks the restriction flag before emitting indexable projections.

## Leaning (committed in findings)

**Pseudonymous-by-default commit identity is DECIDED and gates the data model (GIT-1):** commit bytes
carry a stable opaque pseudonym; the erasable person↔pseudonym map lives in Id; the **display name is a
render-time projection** so developers still see real names. Enforced on the **push path** (CLI/UI
author as pseudonym; server-side enforcement is a per-repo policy). **History-rewrite** is the
supported, audited, hash-changing, rate-limited path for PII-in-content / legacy history, with full
distributed-erasure reach (replicas/reflogs/bitmaps/backups/bundles/CDN; foreign mirrors policy-gated &
documented). **The residual Art. 17 reach into immutable bytes is `[OPEN — LEGAL]`** — a named gap-report
floor co-owned with DPO; engineering minimises PII-in-history so it rarely bites.

## Prior art / sources

- EI-04 §1 (erasure-vs-immutability; the git-history half is "genuinely unsolved"); gdpr-and-audit §7
  (the named GD-1 reconciliation), storage §5.4 (crypto-shred reach into pack tier vs commit bytes).
- `git filter-repo` / filter-branch (history rewrite); content-addressing as the immutability source.
- Pseudonymisation / tombstoning (Kleppmann *DDIA* ch.5); references-not-payloads (ADR-04.4).
- GDPR Art. 17 (erasure) + Art. 18 (restriction) + the "technically infeasible/disproportionate effort"
  limit; Phase-1 git-hosting §9; Phase-2 git-hosting §7.5.
