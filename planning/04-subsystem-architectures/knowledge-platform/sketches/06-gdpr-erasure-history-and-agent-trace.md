# Sketch 06 — GDPR erasure over history/op-logs + the agent-trace holder (AG-7)

> Phase 4, Knowledge, **exploration**. Canonical: ADR-12 / `gdpr-and-audit.md` (PersonalDataHolder),
> `storage.md` §5 (GD-4 per-subject vs per-tenant crypto-shred), deep-dive §8 (the hardest GDPR
> surface), AG-7 (the agent trace is a content-addressed Knowledge document; an erasable holder).
> Knowledge is "the hardest GDPR surface in Myelin" (`knowledge-platform.md` §1) and must accept a
> content-addressed agent-trace write.

---

## 0. Why this is the hardest surface, and what's already solved

Free-text knowledge embeds personal data in unpredictable places (names in meeting notes, a customer
email pasted into a page), *and* it has history, an append-only collab op-log, references, and exports.
The doctrine already gives the substrate: **crypto-shred + references-not-payloads + pseudonym
indirection** (`storage.md` §5; identity §11). My job is to apply it to the **block tree + op-log +
history + db rows**, decide the per-subject/per-tenant key split, and accept the agent trace.

The **honest limitation** (deep-dive §8, GD-6 `[OPEN → LEGAL]`) stands: full *automated* free-text PII
*detection* is not perfectly solvable. The realistic design: (a) erase/anonymise **structured**
personal references reliably; (b) provide **tooling** (DSAR export, search, flagged-content review) +
a **documented process** for free-text; (c) **crypto-shred** the data classes that carry inline PII.
State it, don't over-promise.

## 1. The GD-4 key split applied to Knowledge (per-subject vs per-tenant)

Per `storage.md` §5.1, the classification tag drives the key choice. For Knowledge:

| Data class | Key (GD-4) | Erasure mechanism |
|---|---|---|
| **Authorship/attribution** (created/edited/commented by) | pseudonym (identity S2), not a DEK | **Anonymise**: reassign to a pseudonymous "Deleted user" — *delete the identity, not the fact* (EI-04 §1). The block/edit survives (preserves others' work + doc integrity); the person becomes the opaque `principal_id`, unresolvable after Id's pseudonym shred. |
| **Free-text inline content with PII** (prose bodies, comment text, db free-text fields) | **per-subject DEK** (GD-4 free-text class) | **crypto-shred** the per-subject DEK → the ciphertext in the live block rows, **the op-log, history snapshots, and backups** becomes unrecoverable without mutating immutable bytes (`storage.md` §5.3). This is how erasure reaches the append-only collab op-log — the deep-dive §8 "can't just delete an op" answer. |
| **Structured personal references** (a `person` db field, a `mention(Principal)` node) | pseudonym indirection | the node/field stores the opaque `principal_id`; pseudonym shred makes it unresolvable; the mention tombstones (Refs). No content mutation. |
| **Bulk tenant-content** (block structure, db schema, non-PII props) | per-tenant DEK | erasure of an individual here = tombstone/pseudonymise (references-not-payloads); per-subject keying buys nothing. |

**The op-log erasure answer (the genuinely-hard part, deep-dive §8)**: the op-log is append-only and
merge-dependent — you *cannot* delete an op. So **inline content carried in ops is encrypted under the
per-subject DEK at rest**; erasing the subject **crypto-shreds the DEK**, rendering the personal segments
in *every* op (and every snapshot, and every backup) unrecoverable — without rewriting the immutable
op-log. This is the crypto-shred-over-immutable-logs technique the deep-dive §8 names as leading, now
keyed per-subject so an *individual's* erasure is one key-destroy, not a whole-doc rewrite.

**The residual, named**: a per-subject DEK encrypts *that subject's authored content*; but personal
data *about* a subject written by *someone else* in free prose (Alice's name in Bob's meeting notes) is
under *Bob's* key, not Alice's — crypto-shred can't individually reach it. That's the free-text-PII
honest limitation (GD-6): structured references about Alice (mentions, person fields) erase reliably;
free-prose about Alice needs the tooling+process path. Documented, not hidden.

## 2. The PersonalDataHolder implementation (locate/export/rectify/restrict/erase)

Knowledge is an exhaustive-list holder (auto-registered by the harness, `00` §3.4). Across blocks,
db rows, history snapshots, the op-log, mentions, authorship:

- **`locate(subject)`** → structured: rows where the subject is a `person` field / `created_by` /
  `mention`; the subject's authored pages/blocks/comments; the subject's per-subject-DEK-encrypted
  content. Free-text *about* the subject → best-effort via Search + flagged for the tooling path.
- **`export(subject)`** → the lossless JSON export (deep-dive §2.10) scoped to the subject — the Art.
  20 portability mechanism *and* the DSAR-export spine. Markdown/HTML/PDF/CSV are projections of the
  same lossless JSON.
- **`rectify` / `restrict`** → update structured fields; the **restriction flag** suppresses
  indexing/agent-use/analytics/notification for a restricted subject (the platform-wide restriction
  honour, `README` §5).
- **`erase(subject)`** → (1) **anonymise authorship** (reassign to pseudonymous "Deleted user");
  (2) **crypto-shred the per-subject DEK** (reaches live rows + op-log + history snapshots + backups);
  (3) **tombstone mentions** (Refs degrades backlinks); (4) **purge + reindex Search** in lockstep
  (embeddings of personal data are personal data, erased with source — `search-and-indexing.md` §4.8);
  (5) **publish/CDN purge** for any published page (sketch below); (6) emit `knowledge.subject.erased`
  + the receipt. This is the DSR fan-out's Knowledge step (`gdpr-and-audit.md` §4).

## 3. History, snapshots, and the version UI

- **History** = named version snapshots + the op-log tail (sketch 01). Erasure reaches both via
  crypto-shred (the per-subject DEK encrypts personal content *inside* snapshots and ops). A history
  view of an erased segment renders a **redacted placeholder** ("content erased per data-subject
  request"), not the personal data (the History-UI erased state, `knowledge-platform.md` §4.6).
- **Restore** must not resurrect erased content: a restore runs the **post-restore re-erasure pass**
  against the erasure ledger (`storage.md` §7.5 / GD-14) — re-destroying any per-subject DEK the backup
  re-introduced. So "restore to version N" can't un-erase a person.

## 4. Published / public pages (the high-risk export)

Publish-to-web exposes personal data outside access controls — a high-risk export (deep-dive §8,
`knowledge-platform.md` §7.6). Rules: **explicit personal-data warning + lawful-basis prompt at publish
time** (the §7 sharing-dialog state); `publish` is a distinct tight permission (sketch 04); on unpublish
or erasure, a **CDN/cache purge** is part of the operation; published pages are tracked so erasure can
reach them. No personal data leaves the cell to a third-party CDN (residency, `00`/ADR-11).

## 5. The agent-trace holder (AG-7) — a required change Knowledge accepts

The Agent Fabric requires Knowledge to **accept a content-addressed write of an agent execution
trace** and register it as an erasable holder (AG-7; `agent-fabric.md` §4.5/§11). The trace is "just a
document" (content-addressed, immutable, reusing `myelin-content` — "reusing it saves an entire schema
and projection"). My deliverable:

- **A write path for an agent-authored, content-addressed trace document** → returns an `ArtifactRef`
  (`run.trace_ref`). The trace is immutable (content-addressed blob, `storage.md` §3.2) and is a
  Knowledge page-shaped document (the block model) holding the conversation (system context, tool
  inputs/results, surfaced reasoning).
- **It is a `PersonalDataHolder`** (AG-7): residency-pinned, crypto-shred-capable, erasable. Some trace
  content is personal data → the per-subject DEK class; erasing a subject crypto-shreds their trace
  content; attribution falls back to the opaque pseudonym (the agent-fabric D-10 drill: "erasure
  reaches the trace"). It is **distinct from the tamper-evident audit log** (GDPR/Audit owns that; the
  trace is the human-readable narrative).
- **The agent author is a `Principal` with `kind=agent`** (identity §3) — the trace's authorship is the
  agent, on-behalf-of the human (the agent edits, like human edits, flow through the same model so
  attribution/undo/history treat them first-class, `knowledge-platform.md` §7.5).

## 6. What this sketch commits to the findings

- **GD-4 split**: per-subject DEK for free-text/inline-content/comment-bodies (so an individual's
  erasure crypto-shreds across live rows + op-log + history + backups); pseudonym indirection for
  authorship + structured personal references (anonymise, don't delete); per-tenant DEK for bulk
  structure. Op-log erasure = crypto-shred the per-subject DEK, never delete an op.
- **Honest limitation (GD-6)**: structured personal references erase reliably; free-prose-about-someone
  needs tooling+documented process. Named, not hidden.
- **PersonalDataHolder**: locate/export(lossless JSON)/rectify/restrict(flag)/erase (anonymise +
  crypto-shred + tombstone mentions + purge-reindex Search + CDN purge + receipt). History renders
  erased segments as redacted; restore runs post-restore re-erasure.
- **Published pages**: lawful-basis prompt + warning at publish; tight `publish` permission; CDN purge
  on unpublish/erase; in-cell only.
- **Agent trace (AG-7)**: Knowledge accepts a content-addressed agent-trace write → `ArtifactRef`;
  registers it as an erasable `PersonalDataHolder` (per-subject DEK class); distinct from the audit log;
  agent is the `kind=agent` author.

## Cited prior art

- Crypto-shred over immutable/append-only data: NIST SP 800-88r1 (cryptographic erase); Boneh & Lipton,
  *A Revocable Backup System* (USENIX Security 1996); `storage.md` §5; EI-04 §1.
- Delete-the-identity-not-the-fact / pseudonymisation + tombstones: Kleppmann, *DDIA* ch. 5; EI-04 §1;
  identity §11 (the pseudonym-map lever).
- Erasure reaching search + embeddings: `search-and-indexing.md` §4.8 (embeddings are personal data).
- Agent trace as a content-addressed document: AG-7 / EI-03 §4.4; `agent-fabric.md` §4.5.
