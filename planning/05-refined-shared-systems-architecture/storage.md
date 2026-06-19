# Phase 5 — Storage (REFINED, canonical) — tiers · KMS · crypto-shred · backup/restore-verification

> Phase: `05-refined-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md).
> Reconciliation spine (binding): [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md)
> (resolves X-1..X-7, OQ-A..OQ-L) + [`contract-index.md`](./contract-index.md) (the frozen build-to surface
> that **supersedes** Phase 3). Doctrine (binding): [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> §8 (minimal stack, content-addressing, forward-only migrations, measure-before-shard) + §11 (a backup that
> has never been restored is not a backup); [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md)
> §1 (crypto-shred substrate, erasure-vs-immutability), §3 (object-backed git seam), §5 (event-volume seam,
> reindex-from-source). Spine: **ADR-10** (datastore tiers), **ADR-12** (PersonalDataHolder/crypto-shred),
> **ADR-18** (restore-verification gate); cross-refs ADR-11 (cells/residency), ADR-04 (bus→OLAP).
> Base carried forward: [`../03-shared-systems-architecture/storage.md`](../03-shared-systems-architecture/storage.md).
> Date: 2026-06-19.
>
> **What this doc is.** The REFINED, canonical Storage shared-system architecture Phase 6/7 build on. It
> carries the Phase-3 design as the base and **applies** the Phase-5 reconciliation decisions + the CR §8
> change requests. Storage is **not one database** — it is the **tiering + the portability constraint + the
> cross-cutting KMS/crypto-shred mechanism + the durability gate**. It owns the *mechanism*; GDPR/Audit owns
> the *policy* (when to shred, the DSR fan-out — ADR-12.3). No ADR is reversed.
>
> **Status convention.** *CONFIRMED* = unchanged from Phase 3, ratified. *SHARPENED* = the Phase-3 contract
> stood; its open encoding/shape is now frozen. *NEW* = a contract/sub-shape named for the first time in
> Phase 5. *[OPEN → P6/LEGAL]* = honestly carried forward.
>
> **Units (frozen, never re-litigated):** timestamps = RFC-3339 UTC; budgets/costs = integer minor-units;
> TTLs/staleness/timers = seconds; resilient-client timeouts = milliseconds;
> pii_key_ref = kms://<tenant>/<dek-epoch>/<class>, <class> ∈ {tenant, subject:<id>, blob}.

---

## 0. Changes vs Phase 3 (every change, listed)

Storage's Phase-3 design was confirmed wholesale; the Phase-5 work is additive sharpening + four genuinely
new shapes that fall out of the reconciliation. The seven changes, each tied to its contract-index entry:

| # | Change | Kind | Drives | Contract |
|---|---|---|---|---|
| C1 | **Per-subject DEK granularity now explicitly covers CI log segments** (the T3 log tier), not only OLTP free-text/profile/chat-body columns. A subject's inline PII in a CI log is isolable to a per-subject DEK where it is isolable; erasure crypto-shreds exactly their reachable content incl. backups. | SHARPENED | CI inline-log-PII erasure (GD-6 floor → per-subject) | 11.4 |
| C2 | **T3 log tier `(job, step, byte-range)` index frozen as the CI-specific shape.** Firehose frames seal into T2 content-addressed segments; the OLTP byte-range index keyed by `(job, step)` is the resolver behind the jump-to-failure `details_ref` sub-anchor (X-1 / OQ-D). | SHARPENED | CI log archive + the `CheckStatus.details_ref` `#step-<n>` resolution | 11.8 |
| C3 | **Within-EU CDN-distributable clone/bundle blob class (NEW)** — a named blob class over the `BlobStore` for hot-repo / clone-storm acceleration, content-addressed, residency-respecting (no extra-EU edge for PII; the tenant's region pins the bundles). | NEW | Git clone-storm acceleration | 11.2 |
| C4 | **Trust-tier / branch-scoped cache namespaces in `BlobStore` (NEW)** — a scope-key convention over the per-tenant blob keyspace so an `UntrustedFork` write **cannot reach the trusted cache scope** (the poisoned-cache defence; ties to the X-1 trust tiers). | NEW | CI fork-PR cache isolation | 11.2 |
| C5 | **OLAP read store honours the restriction flag (SHARPENED)** — `restrict(subject)` suppression propagates into T4: **no analytics for a restricted subject**. Worklog/productivity/estimate fields carry the OQ-H sensitivity classification governing analytics-eligibility (`[OPEN — LEGAL]`). | SHARPENED | Issues CFD/cycle-time/velocity (partially blocking, now bounded) + a compliance gate | 11.6 |
| C6 | **Outbound push-mirror to a foreign host is a residency boundary crossing (NEW gate)** — Storage names the constraint; the gate itself lives at GDPR/Audit `transfer_allowed` (10.5) + the control plane. A mirror config targeting an extra-EU host for PII-bearing content is denied by default. | NEW (gate at 10.5) | Git push-mirror residency | 10.5 / §6 |
| C7 | **The free-text/immutable-content erasure residual is instantiated by reference, not restated.** Storage provides the structural floor (per-subject DEK shred, pseudonym-map shred reach, crypto-shred-reaches-backups); the residual posture is the ONE platform artifact (10.9, X-7) — Storage says "the residual is handled per `00 §X-7`," it does not author a Storage-local residual statement. | NEW (by reference) | the one erasure posture | 10.9 |

**Everything else is CONFIRMED unchanged from Phase 3** and is cited, not re-derived: the four tiers + the
store map (§2/§3 below), the three-level KMS hierarchy (§4), the GD-4 per-subject-vs-per-tenant decision
rule (§5), BYOK/HYOK + the `KeyOrigin` trait + `can_derive_plaintext_index()` (§6 here, contract 11.3), the
backup/restore-verification durability gate + the cross-seam consistency cursor + post-restore re-erasure
(§7), measure-before-shard scaling (§8), and all ten failure-mode drills (§10). The object-backed git pack
seam (STOR-5) remains the **named, designed-not-built floor** whose impl is the Git P6 deliverable. The two
Phase-3 reconciliation anchors (the `EventEnvelope` field list + units; the `ArtifactRef` token table) are
unchanged and remain authority.

---

## 1. Purpose, responsibilities, and the one-paragraph thesis (CONFIRMED)

Unchanged from Phase 3 §0. Storage owns the **durable primitives every subsystem leans on**, with residency,
per-tenant envelope encryption, and crypto-shred baked into the seam so no service can opt out (ADR-10/12).
Concretely it owns **eight concerns**, each a named section: (1) the three tiers + the OLAP read store; (2)
the KMS key hierarchy; (3) crypto-shred + GD-4 granularity; (4) BYOK/HYOK and its limits; (5)
backup/restore-verification as a durability gate; (6) scaling/sharding in the cell topology; (7) contracts
exposed/consumed; (8) failure modes + drills owed.

**Thesis (CONFIRMED).** *A subsystem opens its OLTP pool, its blob prefix, and (if it owns one) its index
through the bootstrap harness; the harness wires each through the KMS so every byte at rest is per-tenant
envelope-encrypted, registers each as a `PersonalDataHolder`, and pins it to the cell's region — so
"residency + erasability + recoverability" are properties the storage seam **enforces**, not features a
service remembers to add.* Erasure of anything in an immutable or backup tier is **crypto-shred** (destroy a
key), never byte-mutation; durability is a property only the **restore-verify drill** can make real.

### 1.1 Non-negotiables every tier inherits (CONFIRMED, Phase-3 §0.1)

Unchanged: tenant is the first column / partition key / blob-prefix component of everything (no cross-tenant
query path; tenant from the verified token, never the path); every store is residency-pinned, per-tenant
envelope-encrypted, crypto-shred-capable, and a `PersonalDataHolder` (harness auto-registers); no subsystem
reads another subsystem's store (the `no-cross-db` lint; there is no generic cross-subsystem storage API);
migrations are forward-only and online; the cache/coordination store is NEVER a source of truth (STOR-3).

---

## 2. The store map (CONFIRMED, with C1/C2/C4 sharpenings noted inline)

The Phase-3 store map (T1 OLTP, T2 object/blob, T3 log/firehose, T4 OLAP, T5 KMS, T6 backup/archive, T7
cache/coordination, plus the specialized stores owned elsewhere) stands unchanged in structure. The
Phase-5 changes touch three rows:

| # | Component | Holds | Crypto-shred unit | P5 change |
|---|---|---|---|---|
| T1 | OLTP (per-subsystem) | domain state + each service's `outbox` | per-tenant DEK; **per-subject sub-key for free-text/profile PII columns** | — (CONFIRMED) |
| T2 | Object/blob (S3-compatible) | LFS blobs, CI artifacts/caches, doc media, attachments, **clone bundles**, base backups | per-tenant DEK (object DEK wraps content key) | **C3** CDN clone/bundle class; **C4** trust-scoped cache namespaces |
| T3 | Log/firehose (object-backed segments + range index) | CI logs, chat message log, collab op-stream archive | per-tenant DEK; **per-subject for chat bodies AND CI inline-PII log segments** | **C1** per-subject CI-log DEK; **C2** `(job,step,byte-range)` index frozen |
| T4 | OLAP read store (ClickHouse-class) | CQRS analytics read model, fed by the bus | per-tenant DEK (derived; inherits source) | **C5** honours the `restrict` flag |
| T5 | KMS (Vault-Transit-class / HSM) | the key hierarchy (§4) | n/a (it *is* the shred lever) | — (CONFIRMED) |
| T6 | Backup/archive | base backups + WAL/log stream | per-tenant DEK (backups are ciphertext) | — (CONFIRMED) |
| T7 | Cache/coordination (Redis/Valkey-class) | fail-static cache, dedup ledgers, userset cache, rate counters — NEVER source of truth | ephemeral; TTL ≤ revocation SLA | — (CONFIRMED) |

**Derived stores (T4, T7) are reindex-from-source primitives** (rebuildable by replaying the source through
the live consumer path, bus §4.9 — no bespoke recovery code). **Systems of record (T1, T2, T3 + the
specialized stores)** are gated by the restore-verification drill (§7; ADR-18). **T5 (KMS) is the single
most blast-radius-sensitive component.**

---

## 3. The three tiers + the OLAP read store

### 3.1 Tier 1 — Transactional (OLTP), Postgres-class (CONFIRMED)

Unchanged from Phase-3 §3.1. **PostgreSQL-class, one database per service** (system of record; no shared
tables, no cross-service joins). What Storage owns: the `(tenant, region)`-first RLS tenant-scoping guard
(the IDOR floor; `tenant-predicate` lint); the per-tenant envelope-encryption seam (personal-data columns
under the tenant DEK, **free-text/profile columns under a per-subject sub-key**); **the outbox lives here**
(same transaction as the state change — the anchor of the cross-seam consistency point, §7.3); forward-only
online migrations (expand→backfill→contract, lock time measured against a restored copy); read-replica
awareness (the authn/authz hot path is the likely first dedicated replica, ID-4). The **hot-table flags**
each subsystem declares for the forward-only-migration lint (KN `block`/`db_row`/`doc_op`, and all
high-write subsystems) are CONFIRMED frozen (contract 1.5).

### 3.2 Tier 2 — Object/blob, S3-compatible, content-addressed (CONFIRMED + C3 + C4)

**Decision (CONFIRMED, Phase-3 §3.2).** S3-compatible object store (MinIO or Ceph RADOS Gateway —
self-hostable, EU-deployable), behind the **narrow content-addressed `put/get/head/delete` trait** the
substrate defines, so filesystem↔object-store is a one-line swap. The S3 REST API is the portable contract;
no proprietary global managed object service.

```rust
pub trait BlobStore {                                    // hash-on-write; fs-vs-object is a one-line swap
    fn put(&self, bytes: &[u8]) -> Result<ContentHash>;  // content address = the hash (Git/Venti model)
    fn get(&self, h: &ContentHash) -> Result<Vec<u8>>;
    fn head(&self, h: &ContentHash) -> Result<BlobMeta>;
    fn delete(&self, h: &ContentHash) -> Result<()>;     // crypto-shred is the real erasure (ADR-12.3)
}
```

CONFIRMED unchanged: **BLAKE3 for new blobs** (self-describing multihash prefix so SHA-256 can coexist);
**address by plaintext hash within a tenant's keyspace, store ciphertext** (dedup is per-tenant; cross-tenant
dedup deliberately forgone for isolation — it would be a residency leak); per-blob random content key wrapped
by the tenant (or per-subject) DEK (§4.4); erasure of a blob reachable from an immutable/backup tier is
**crypto-shred, not `delete`**.

**C3 — within-EU CDN clone/bundle blob class (NEW).** Hot-repo and clone-storm acceleration (CR §8, Git)
needs clone bundles served from edge locations. Storage defines a **named blob class** over the `BlobStore`:

- A clone/bundle blob is **content-addressed** like any T2 blob (BLAKE3), so an edge cache is a pure
  content-address cache — a cache entry is valid iff its hash matches; there is no staleness model to get
  wrong (the content-address *is* the validity check).
- The class is **residency-respecting**: the CDN edge set is **within-EU-only** for any tenant whose region
  is EU; PII-bearing content (repo contents may carry PII) never reaches an extra-EU edge. The CDN is the
  *delivery* of already-encrypted, content-addressed bundles; the tenant's region pins which edge POPs are
  eligible (the control plane's `residency_verify`, contract 12.4, covers the CDN edge set).
- This is **NEW** but rides the existing trait: it is a blob-class tag + an eligible-edge-set policy, not a
  new store. The base `BlobStore` is unchanged; the CDN is a delivery layer in front of it.

**C4 — trust-tier / branch-scoped cache namespaces (NEW).** CI build caches live in T2. A fork PR runs
under `trust_tier = untrusted_fork` (X-1); its cache writes must not poison the cache a trusted run later
reads (the classic poisoned-cache attack). Storage defines a **scope-key convention** over the per-tenant
blob keyspace:

```
cache key prefix = <tenant>/ci/cache/<scope>/...
  <scope> ∈ { trusted, fork:<pr_id>, branch:<protected_branch_name> }
```

- An `untrusted_fork` run may **read** the `trusted` scope (cache hits are fine) but may **only write** to
  its own `fork:<pr_id>` scope — a write to `trusted` is **refused by the blob client** (the scope is stamped
  from the run's `trust_tier`, which CI stamps from run provenance; Storage enforces the write-scope rule, it
  does not recompute trust). A trusted run reads/writes `trusted` (or the `branch:` scope for protected-branch
  caches).
- This is the storage-tier half of the X-1 poisoned-pipeline defence: trust is *decided* by CI/Git off the
  run provenance; the cache namespace makes "a fork cannot reach the trusted cache scope" a **structural**
  property of the blob keyspace, not a check a job must remember.

### 3.3 Tier 3 — Log/firehose, append-mostly (CONFIRMED + C1 + C2)

**Decision (CONFIRMED, Phase-3 §3.3).** The high-volume ephemeral firehose (CI log lines, chat
presence/typing, collab op-streams) rides a **separate transport from the durable bus** (the firehose split,
bus §4.3, mandatory). Storage owns the **durable archive of the firehose**, not the live ephemeral fan-out
(that is the bus's firehose transport, now the OQ-J resume-cursor protocol, contract 3.5). The durable bus
carries only **pointer events** (`ci.log.appended`, `knowledge.doc.updated`) — an agent is never woken per
log line.

**C2 — the CI log tier `(job, step, byte-range)` index, frozen.** CI is the heaviest log producer. The
shape is now frozen (contract 11.8):

- Firehose frames are **appended to a current segment**; sealed segments flush to the object tier (T2) as
  **content-addressed blobs** (inheriting T2 encryption + crypto-shred).
- An **OLTP byte-range index** maps `(job, step, byte-range) → (segment-blob, offset)` for tail + range-read.
  Keying by `(job, step)` is what lets the X-1 `CheckStatus.details_ref` sub-anchor — `myelin://.../ci/run/<id>#step-<n>` (the OQ-D `step-<n>` `#sub` kind) — **resolve to the exact failing step's log bytes** (the
  jump-to-failure path). The `step-<n>` sub-anchor resolves through this index; this is the storage-tier
  realisation of the OQ-D ladder for CI runs.

**C1 — per-subject DEK for CI inline-PII log segments.** Phase 3 keyed chat bodies per-subject; Phase 5
extends the rule to **CI log segments carrying inline PII** (GD-6 floor — now per-subject where a subject's
PII is isolable, not per-tenant). Where a CI log segment's PII is attributable to and isolable per subject,
it is encrypted under that subject's DEK so the subject's erasure crypto-shreds exactly their log content
(live AND in backups), without destroying the rest of the tenant's logs. Where inline PII is *not* isolable
to one subject (interleaved free-text from many), the segment falls back to the per-tenant DEK and the
residual is handled by the platform erasure posture (10.9, X-7) — Storage does not invent a CI-local residual
statement.

**Chat message log (CONFIRMED).** The wide-column (Scylla/Cassandra-class) candidate vs OLTP+object choice
remains **the Chat P6 store-engine decision**; Storage pins only the constraint: per-tenant envelope
encryption, **chat bodies under a per-subject sub-key**, residency-pinned, a `PersonalDataHolder`. KN CRDT
snapshots + media are content-addressed BLAKE3 blobs (T2) with crypto-shred-on-erase; the row↔blob↔op-log↔offset
restore-consistency is the §7.3 cross-seam cursor (CR §8, CONFIRMED).

### 3.4 The OLAP read store (CQRS, fed by the bus) — CONFIRMED + C5

**Decision (CONFIRMED, Phase-3 §3.4).** A ClickHouse-class columnar read store holds the CQRS analytics read
model (issue analytics, delivery health), **fed async off the durable event stream** via the idempotent
consumer template (dedup on `event_id`), never by scanning OLTP. Reindex-from-source is the **only** rebuild
path (no "read OLTP into ClickHouse" backdoor); it is a `PersonalDataHolder`; it is residency-pinned and
crypto-shred-capable (one tenant's OLAP rows live in that tenant's cell — it is *not* a global warehouse).

**C5 — the OLAP store honours the restriction flag (SHARPENED, contract 11.6).** `restrict(subject)`
(contract 10.1) suppresses indexing/agent-use/analytics/notif for a subject pending erasure. Phase 5 pins
that this propagation **reaches T4**: a restricted subject's rows are **excluded from analytics aggregates**
(CFD, cycle-time, velocity, delivery health). Concretely, the OLAP consumer applies the restriction flag as
a filter at query time and the subject's contribution is withheld until restriction lifts or erasure
completes. This is a **compliance gate**, not a tuning knob — it is the storage-tier realisation of the
`restrict` suppression for the analytics holder, and it is what unblocks the partially-blocking Issues ask
(CR §8: `issue.*`/`sla.*`/`cycle.*` reports depend on T4, and they must not leak a restricted subject).

**Worklog/productivity/estimate analytics-eligibility (`[OPEN — LEGAL]`, OQ-H).** These fields are tagged
`category = behavioural, role = tenant-content, restricted by default` (contract 10.2). The engineering
posture (per `00 §OQ-H`): per-individual productivity rollups are **off by default**, gated behind an
explicit tenant-admin enablement the posture flags as requiring works-council consultation in applicable
jurisdictions; cross-individual analytics for a restricted subject is excluded. Storage enforces the
**analytics-eligibility gate** at the OLAP feed; **counsel/DPO ratifies** the special-category classification.
Storage ships the gate regardless.

### 3.5 The git object-backing seam (STOR-5 — FLOOR, designed-not-built) — CONFIRMED

Unchanged from Phase-3 §3.5. World-scale git wants authoritative objects/packs in the object tier (T2), not
node-local disk. The **seam is committed now** so the v1 git data model is **never node-pinned**: git packs
and loose objects are addressed through the `BlobStore` trait, so the "local-disk → object-store-backed
packs" transition is a backing swap, not a rewrite. **Floor named:** v1 may run packs on local disk behind
the trait; **follow-on:** object-backed pack/delta management + the smart-transport paths are the **Git P6
deliverable** (TE-24, contract 11.2). The *relocatability* constraint (repos are not pinned to a node) is
DECIDED here; the implementation is the named next step. This couples to the C3 CDN clone/bundle class
(clone bundles are content-addressed T2 blobs) and the C6 push-mirror residency gate.

---

## 4. The KMS key hierarchy (per-tenant envelope encryption) — CONFIRMED

Unchanged from Phase-3 §4 (contract 11.3). A **three-level envelope-encryption hierarchy** (AWS/GCP KMS
envelope model; NIST SP 800-57; Vault Transit as the self-hostable engine), because it makes **crypto-shred
a key-destruction operation, not a data-scan**.

- **L0 Cell root (RK):** per cell; HSM/sealed KMS; never exported; wraps tenant KEKs; the most protected key.
- **L1 Tenant KEK:** one per `(tenant, region)`; destroying it is **tenant-granularity crypto-shred** (the
  `tenant offboard` lever).
- **L2 DEKs:** the working keys (AES-256-GCM). **Per-tenant DEKs** encrypt the bulk; **per-subject DEKs**
  encrypt the classes whose erasure must be individual (free-text/profile, chat bodies, **CI inline-PII log
  segments per C1**, agent memory) — the GD-4 subject-granular lever (§5).

The **`key_ref`** (which KEK, which DEK, which version) travels with every ciphertext — in the OLTP row, the
blob metadata, and the envelope's `pii_key_ref`. Canonical shape (frozen):
`pii_key_ref = kms://<tenant>/<dek-epoch>/<class>`, `<class> ∈ {tenant, subject:<id>, blob}`. Key rotation
is **envelope re-wrap, not bulk re-encryption** (O(keys), not O(data)); forward-only; a *compromised* key
triggers a backfill re-encryption (expand→backfill), never a rollback. Object-tier per-blob content keys are
wrapped by the tenant/per-subject DEK so the content-address stays stable while rotation/shred operate at the
DEK level. **KMS availability/blast radius (CONFIRMED, §4.5):** in-cell, HSM/sealed L0, read-path DEK caching
degrades like fail-static, hard-down → not-ready (never fail-open), Raft-class HA, Shamir-split root recovery;
a crypto-shredded key is **excluded from backup** (it must stay dead — §7.5).

---

## 5. Crypto-shred + GD-4 granularity (per-subject vs per-tenant) — CONFIRMED + C1

**Crypto-shredding is a first-class deletion primitive** (ADR-12.3; NIST SP 800-88r1; Boneh & Lipton 1996):
destroy the key, and the ciphertext — in live DBs, **in immutable logs, and in every backup** — becomes
unrecoverable without ever mutating the immutable bytes.

### 5.1 The GD-4 decision rule (CONFIRMED, Phase-3 §5.1; C1 extends the per-subject row)

The classification-driven granularity rule stands:

| Data class | Granularity | Why |
|---|---|---|
| **Free-text / profile PII** (names, emails, bios, comment bodies, knowledge free-text, **chat message bodies**, **CI inline-PII log segments [C1]**, agent memory/embeddings) | **PER-SUBJECT DEK** | An individual's Art. 17 erasure must delete *that person* without touching the tenant — one key-destroy. |
| **Bulk tenant-content** (issue field values, doc block structure, repo/PR metadata, run state) | **PER-TENANT DEK** | Mostly non-personal or pseudonym-referenced; erasure here is tombstone/pseudonymise, not key-destroy. |
| **Tenant-wide (offboarding)** | **PER-TENANT KEK (L1)** | One operation crypto-shreds the whole tenant, backups included. |
| **Inline-PII events** (rare) | **per-tenant, optionally per-subject** via `pii_key_ref` | Default is references-not-payloads (nothing inline to shred). |

**The rule (DECIDED):** *data whose erasure unit is the individual subject is keyed per-subject; data whose
erasure is satisfied by pseudonymisation/tombstoning is keyed per-tenant; tenant offboarding is the KEK.* The
schema-level personal-data classification (contract 10.2, `classify(field)`) **drives the key choice
automatically** — a field tagged `personal-data, erasure=subject` is wired to a per-subject DEK by the
harness. This is the *measured-minimum* granularity, not maximalist (per-subject keying for everything would
explode the DEK count and most bulk content is already pseudonym-referenced). **C1's only change:** the
per-subject row now explicitly includes CI inline-PII log segments where isolable.

### 5.2 The crypto-shred algorithm and its reach (CONFIRMED, Phase-3 §5.3)

```
erase(subject, tenant):                                    # called by the DSR orchestrator (ADR-12.2)
  1. Id.erase(subject)            → delete the pseudonym map + profile record (DSR step 1);
                                    git/bus/audit history now holds only the opaque pseudonym
                                    (grammar <pseudonym>@<tenant>.noreply, contract 4.8).
  2. KMS.destroy(per_subject_DEK(tenant, subject))         # crypto-shred: free-text/chat/profile/agent-memory/
                                    CI-inline-log ciphertext (live AND in backups) now unrecoverable.
  3. Search.purge+reindex(subject)                         # index is plaintext-derived: PURGE+reindex, not shred.
  4. Refs.tombstone(subject)      # unfurls degrade via the OQ-D ladder.
  5. Bus.erase(subject)           # crypto-shred inline-PII event keys + emit *.erased tombstones.
  6. record erase receipt (audit holder, carve-out).
```

CONFIRMED: crypto-shred **reaches backups by construction** (backups store ciphertext under the destroyed
key — asserted by drill D-S4); the search index is the exception (plaintext-derived → purge+reindex); reach
is **verified, not assumed** (the erasure-reaches-every-holder drill, D-S5, hits OLTP, object, log, OLAP,
search, refs, bus, agent memory, notif history, authz tuples, caches/CDN, **and backups**).

### 5.3 The free-text / immutable-content residual — by reference, not restated (C7, X-7)

Storage provides the **structural floor** for the residual: (a) per-subject DEK crypto-shred for
self-authored free-text (their messages, comments, blocks, CI-log PII); (b) reach into reflogs, bitmaps, and
pack-tier backups (those *are* shreddable via the per-tenant blob DEK), versus the commit-object bytes (which
are not — the GD-1 hash-load-bearing case); (c) the pseudonym-map shred reach (Id step 1) so immutable
structures hold only an opaque pseudonym. **The residual posture itself — third-party free-text PII typed by
others, and immutable commit-message bodies — is the ONE platform artifact (contract 10.9, `00 §X-7`),
`[OPEN — LEGAL]`.** Storage does **not** author a Storage-local residual statement; it says: *the residual is
handled per the platform erasure posture in `00 §X-7`*, and contributes its structural reach (crypto-shred
into backups/reflogs/bitmaps; the history-rewrite path's crypto-shred half) to that one posture. Counsel/DPO
ratifies the residual basis once, for all five subsystems.

---

## 6. BYOK / HYOK and its hard limits — CONFIRMED + C6

**Decision (CONFIRMED, Phase-3 §6, contract 11.3).** Customer-managed keys at three levels with explicitly
different capability ceilings: **Platform-managed** (full search/agents), **BYOK** (same capability while the
key is live, plus an instant-shred lever by revocation), **HYOK** (severely limited by construction — Myelin
never sees plaintext, so it **cannot** index, embed, or let agents read HYOK content; only non-HYOK metadata
is searchable). The honest limits stand: *you cannot index what you cannot decrypt* — this is the definitional
consequence, surfaced as an explicit per-space/per-repo HYOK setting with a "not searchable/agent-readable"
warning.

```rust
pub trait KeyOrigin {                              // platform-managed | BYOK | HYOK behind one trait
    fn wrap(&self, dek: &Dek, tenant: TenantId) -> Result<WrappedDek>;
    fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle>;   // HYOK: a CALL OUT, may deny
    fn can_derive_plaintext_index(&self) -> bool;  // platform/BYOK = true; HYOK = FALSE
    fn destroy(&self, key_id: KeyId) -> Result<()>; // crypto-shred (BYOK/HYOK: customer-initiated)
}
```

`can_derive_plaintext_index()` is the **structural enforcement**: Search and the Agent Fabric must consult it
before indexing/embedding; HYOK content is skipped (and marked not-searchable), so the limit is enforced by
code. **CONFIRMED `[OPEN → P6/LEGAL]`:** the per-content-class HYOK policy (which classes may be HYOK; the
cross-artifact-reference-spanning case), the KMIP/external-key-store adapter, and HYOK as a
Schrems-III/sovereign mitigation (GD-7) are carried forward; the **mechanism + the limits** are DECIDED.

**C6 — outbound push-mirror residency gate (NEW).** A Git push-mirror that targets a host outside the
tenant's region is a **residency boundary crossing** for PII-bearing content (repo contents may carry PII).
Storage names the constraint; the **gate lives at GDPR/Audit `transfer_allowed` (contract 10.5) + the control
plane**: a mirror config targeting an extra-EU host for an EU tenant's PII-bearing content is **denied by
default**. Storage's role is to (a) keep mirror-source blobs content-addressed + encrypted (the bytes that
leave are ciphertext unless the target is a true external git remote, in which case the *plaintext* repo
crosses — which is exactly what `transfer_allowed` must gate), and (b) report the mirror target region into
`residency_verify` (contract 12.4) so the no-extra-EU-PII property is attestable. The actual allow/deny is
the control-plane gate, not a Storage-local policy — Storage flags the crossing.

---

## 7. Backup / restore-verification as a durability gate (ADR-18) — CONFIRMED

**"A backup that has never been restored is not a backup."** Storage owns the mechanism; Phase 6 owns the
drill thresholds. Unchanged from Phase-3 §7.

- **§7.1 Continuous archiving + base backups (CONFIRMED).** OLTP (T1): continuous WAL archiving + periodic
  base backups → PITR; **RPO target ≤ 5 min** (default-to-beat). Object (T2): versioned + in-region
  replicated; content addressing makes integrity re-hash-verifiable. Log (T3): sealed segments are immutable
  T2 blobs + the range index in T1. **OLAP (T4) + caches (T7) + derived indexes are NOT backed up — rebuilt**
  via reindex-from-source. KMS (T5): keys backed up only under the cell root, only while the tenant is live;
  a crypto-shredded key is **excluded from backup**. **RTO target:** ≤ 1 h/tenant, ≤ 4 h/cell (Phase-6
  ratifies the numbers).
- **§7.3 The cross-seam consistency point (STOR-4, CONFIRMED).** The cross-seam linearisation cursor is the
  per-aggregate outbox `seq` / event-log offset (the outbox row is written in the same OLTP tx as the state
  change, so OLTP commit order == event order). Restore-to-consistent-point(T): PITR-restore OLTP to the WAL
  position whose outbox rows have `seq ≤ T`; verify every `ContentHash` referenced by restored rows is present
  (a referenced-but-missing hash = FAIL, the silent-corruption case); **reindex derived stores from source up
  to offset T** (never restore them from their own backups → derived == source by construction, no drift);
  consumers resume at T; restore tenant KEKs EXCEPT any crypto-shredded since the backup (§7.5).
- **§7.4 Automated restore-verification (the CI-wired durability gate, CONFIRMED).** Spin a clean target;
  restore T1/T2/T5; reindex T4/Search/Refs from source to T; assert **no loss** (checksum parity), **cross-seam**
  (every restored row's blob hash present + integrity-verified; derived == source-replay), and **erasure held**
  (a subject erased before the backup is still erased after restore — §7.5). Green artifact on pass; RED gate
  fails CI. This also produces the production-scale restored copy that online migrations rehearse lock-time
  against.
- **§7.5 Post-restore re-erasure (GD-14, CONFIRMED).** Every restore runs a mandatory re-erasure pass against
  the **erasure ledger** (the durable, tamper-evident, PII-free record of every completed erasure — opaque
  subject id + timestamp + keys/holders shredded — held by GDPR/Audit, contract 10.8): for each erasure
  completed *after* the backup's point-in-time, re-apply it (re-destroy the per-subject DEK, re-delete the
  pseudonym map, re-purge+reindex Search, re-tombstone Refs, re-emit `*.erased`). Assert 0 resurrected
  subjects. The erasure ledger is itself NOT crypto-shred-erasable (it must survive to drive re-erasure) and
  holds no PII.
- **§7.6 The backup-window-vs-erasure-SLA residual (CONFIRMED, `[OPEN → LEGAL]`).** Between an erasure and the
  next base-backup cycle, an old backup holds the pre-erasure *ciphertext* — but under a now-destroyed
  per-subject DEK, so already unrecoverable. The narrow residual (a key backed up before destruction) is
  closed by §7.5 + "shredded keys excluded from backup" (§4). The exposure window is bounded by the retention
  period + the re-erasure pass and is a **DPO-ratified, documented number** — not a silent gap.

---

## 8. Scaling / sharding in the cell topology (measure-before-shard) — CONFIRMED

Unchanged from Phase-3 §8. **Measure before you shard; read-replicas + connection pooling first; premature
sharding is its own outage.** In-cell, tenant-partitioned (scale = add cells, not bigger global stores).
First scaling move = read-replicas + pooling, NOT sharding (the authn/authz hot path — Id's per-tenant authz
reverse index, the OQ-E `list_objects` JOIN target — is the likely first dedicated read-replica, ID-4).
Sharding deferred until a hot table/tenant is *measured* to outgrow Postgres. **CI is the heaviest storage
consumer** (T2/T3) and the first measured-pressure candidate — the C2 log tier + C4 cache namespaces +
reserve/settle per-tenant fairness (contract 11.7) are what keep one tenant's CI artifact storm from starving
another. The OLAP read store absorbs analytics scans off the bus (CQRS) so they never touch OLTP. Bounded
everything (bounded pools, statement timeouts, per-tenant in-flight caps, bounded prefetch). The
column-store/time-series seam (BUS-6) is specified-not-built, promoted on **measured** volume. Self-host
parity: the same Storage artifacts (Postgres + MinIO/Ceph + KMS + backup machinery) run a managed cell and an
on-prem install, all self-hostable + EU-deployable.

---

## 9. Contracts exposed & consumed (the refined, frozen surface)

Storage is consumed as **tier clients** wired through the bootstrap harness, not a generic cross-subsystem
storage API. The refined surface (matching contract-index §11):

| # | Contract | Owner | Consumed by | Status | Site |
|---|---|---|---|---|---|
| 11.1 | **OLTP tier client** — harness pool + thin tenant-scoped RLS query layer, encrypted columns; one DB per service; no cross-DB; the outbox lives here. | Storage | every subsystem | CONFIRMED | §3.1 |
| 11.2 | **`BlobStore{put,get,head,delete}`** — content-addressed (BLAKE3, per-tenant dedup); fs↔object one-line swap; immutable-tier erasure = crypto-shred. **+ object-backed pack/delta seam** (Git, impl P6); **+ within-EU CDN clone/bundle blob class [C3]**; **+ trust-tier/branch-scoped cache namespaces [C4]** (an `UntrustedFork` write cannot reach the trusted scope). | Storage | every blob-holding service; git pack tier; CI cache | SHARPENED (C3 + C4) | §3.2 |
| 11.3 | **KMS hierarchy + `KeyOrigin` trait** — per-cell root → per-tenant KEK → per-tenant/per-subject DEK; `wrap/unwrap/can_derive_plaintext_index/destroy`; `can_derive_plaintext_index()=false` structurally skips Search/Agent indexing (HYOK). | Storage/GDPR | every encrypted store, Search, Agent | CONFIRMED | §4, §6 |
| 11.4 | **Crypto-shred + GD-4 granularity** — free-text/profile/body/agent-memory/op-log = per-subject DEK (**incl. CI inline-PII log segments [C1]**); bulk pseudonym-referenced = per-tenant DEK; tenant offboarding = the KEK. The `erasure` tag drives the key choice. | Storage | DSR orchestrator | SHARPENED (C1) | §5 |
| 11.5 | **Backup / restore / cross-seam** — WAL+PITR (RPO ≤ 5 min); `restore(to_offset)`; `restore-verify` (CI-gated); event-log offset = the cross-seam cursor (OLTP↔blob↔index↔offset); `post_restore_reerase`. Derived stores rebuilt, not restored. | Storage | ops/DSR, CI durability gate | CONFIRMED | §7 |
| 11.6 | **OLAP read store** — CQRS read model fed by the bus; reindex-from-source only; a holder. **+ honours the restriction flag [C5]** (no analytics for a restricted subject); worklog analytics-eligibility per OQ-H. | Storage | Issues/analytics | SHARPENED (C5) | §3.4 |
| 11.7 | **Reserve/settle cost gate** — reserve at dispatch, settle on completion, never interrupt in-flight; integer minor-units; wholesale ≠ markup. Fronts every agent run + every CI run + every `SCHEDULE_AND_RUN_JOB`. | Agent (gate) + Commercial (wallet) | Agent, CI, spend-bearing workflows | CONFIRMED | (Agent §5.4) |
| 11.8 | **T3 log tier (CI)** — sealing firehose frames into T2 content-addressed segments + an OLTP **`(job, step, byte-range)` index**, per-tenant-DEK (per-subject for inline PII, C1); the jump-to-failure `details_ref` (5.9 / `#step-<n>`) resolves through it. | Storage + CI (heaviest consumer) | CI logs, the X-1 check seam | SHARPENED (C2) | §3.3 |

**Consumed by Storage:** `classify(field)` (← GDPR/Audit, drives per-subject-vs-per-tenant key choice); the
`restrict(subject)` flag (← GDPR `PersonalDataHolder`, drives C5); the erasure ledger (← GDPR/Audit, contract
10.8, drives §7.5 re-erasure); the outbound-mirror `transfer_allowed` gate (← GDPR/Audit 10.5 + control
plane, the C6 gate); `residency_verify`/`placement_of` (← control plane 12.2/12.4); the bus consumer template
+ `*.snapshot` replay (← Bus, drives T4/derived reindex-from-source); `list_objects` (← Id; Storage tiers
never post-filter — reads compose the authz pre-filter where they serve queries).

**CLI/admin surface (CONFIRMED):** `myelin storage usage [--tenant --tier]`; `myelin kms key
list|rotate|shred`; `myelin backup list|restore`; `myelin storage restore-verify` (the durability gate);
`myelin storage residency verify <tenant>` (prove region pinning, incl. the C3 CDN edge set + C6 mirror
targets); `myelin tenant offboard <tenant>` (KEK-destroy fan-out, co-owned with GDPR/Audit).

**Telemetry (CONFIRMED + C-deltas):** `storage_bytes{tenant,tier}`, `kms_unwrap_latency`, `dek_cache_hit`,
`backup_rpo_seconds`, `restore_verify_pass`, `crypto_shred_lag`, `blob_integrity_fail`; **+
`cache_scope_violation{tenant}`** (C4, must be 0), **+ `olap_restricted_subject_leak`** (C5, must be 0), **+
`mirror_residency_deny{tenant}`** (C6).

---

## 10. Failure modes + drills owed (PROVE-IT) — CONFIRMED + 3 new assertions

The ten Phase-3 drills (D-S1..D-S10) stand unchanged (each emits a green artifact on pass; until then the
property is claimed, not proven). Phase 5 adds three assertions onto existing drills for the new shapes:

| # | Property / failure mode | Drill (quantified gate) | Status |
|---|---|---|---|
| D-S1 | **Restore + cross-seam integrity** (the headline) | Rebuild to offset T; assert no loss (checksum parity) + OLTP↔blob↔index↔offset restore to one mutually consistent point. **Gate: 0 dangling refs, 0 loss, cold==live.** | CONFIRMED |
| D-S2 | **RPO/RTO met** | Kill a cell; restore; assert RPO ≤ 5 min, RTO ≤ target. | CONFIRMED |
| D-S3 | **Post-restore re-erasure** (GD-14) | Erase a subject, restore an older backup; assert still erased. **Gate: 0 resurrected subjects.** | CONFIRMED |
| D-S4 | **Crypto-shred reaches backups** | Erase a subject (incl. **CI log segments, C1**); attempt recovery from backups; assert unrecoverable. **Gate: 0 recoverable PII in any backup.** | CONFIRMED + C1 assertion |
| D-S5 | **Erasure reaches every holder** | DSR-erase; assert the fan-out hit every holder incl. search, OLAP, refs, bus, agent memory, backups, caches, **the CDN clone-bundle class (C3)**. **Gate: 0 misses.** | CONFIRMED + C3 assertion |
| D-S6 | **Tenant residency pinning** | Attempt cross-region read/replicate; assert impossible by construction, **incl. the C3 CDN edge set (within-EU) and the C6 mirror target**. **Gate: 0 cross-region PII egress.** | CONFIRMED + C3/C6 assertion |
| D-S7 | **KMS hiccup degrades, not cascades** | Inject transient KMS outage; resolved-DEK reads survive (bounded TTL), hard-down → not-ready+shed, never fail-open. | CONFIRMED |
| D-S8 | **Blob integrity** | Corrupt an object; assert re-hash-on-read detects it + recovery from replica/backup. | CONFIRMED |
| D-S9 | **Online migration safety** | expand→backfill→contract on a restored production-scale copy under load; lock ≤ budget. | CONFIRMED |
| D-S10 | **HYOK opacity enforced** | Mark a class HYOK; assert Search/Agents skip it (`can_derive_plaintext_index()=false`); only non-HYOK metadata searchable. | CONFIRMED |
| **D-S11 (NEW)** | **Trust-scoped cache isolation (C4)** | An `untrusted_fork` run writes a cache entry; assert it lands only in `fork:<pr_id>` scope and a trusted run never reads it as `trusted`-scoped. **Gate: 0 cross-scope cache writes; `cache_scope_violation` = 0.** | NEW (X-1 tie) |
| **D-S12 (NEW)** | **Restricted-subject OLAP suppression (C5)** | `restrict(subject)`; run CFD/cycle-time/velocity; assert the subject's contribution is absent. **Gate: `olap_restricted_subject_leak` = 0.** | NEW |
| **D-S13 (NEW)** | **Outbound-mirror residency deny (C6)** | Configure an extra-EU mirror target for an EU tenant's PII-bearing repo; assert deny-by-default + `residency_verify` reflects no extra-EU PII path. **Gate: 0 PII to an ungated extra-EU mirror.** | NEW (gate at 10.5) |

**Blast-radius note (CONFIRMED):** T5 (KMS) is the apex (a tenant KEK loss = that tenant's data unrecoverable,
by design for shred; mitigated by sealed-root + live-tenant KEK backup). T1/T2/T3 (systems of record) blast
radius is one subsystem/tenant; recovery is restore-verify-gated. T4/T7 (derived) have zero loss blast radius
— rebuilt from source. The control plane holds zero in-region personal data.

---

## 11. Cited prior art (CONFIRMED, consolidated)

Unchanged from Phase-3 §11. Content-addressing: Git object model; Merkle (CRYPTO 1987); Quinlan & Dorward,
*Venti* (FAST 2002); IPFS CID; BLAKE3 (2020). Object store: Weil et al., *Ceph* (OSDI 2006); MinIO; the S3
REST API. Envelope encryption: NIST SP 800-57 / 800-38D; the AWS/GCP KMS DEK/KEK model; Vault Transit +
Shamir unseal; KMIP. Crypto-shredding: NIST SP 800-88r1 ("cryptographic erase"); Boneh & Lipton, *A Revocable
Backup System* (USENIX Security 1996). WAL/PITR: PostgreSQL WAL archiving + PITR; Mohan et al., *ARIES* (ACM
TODS 1992); pgBackRest, WAL-G. Restore-verification: Google *SRE* ch. 26 (*Data Integrity*). Cross-seam
snapshot: Chandy & Lamport (ACM TOCS 1985); Kleppmann, *DDIA* ch. 11 (log offsets as the consistent cursor).
CQRS/columnar: Young/Dahan CQRS; Fowler "CQRS"; ClickHouse MergeTree; Stonebraker et al., *C-Store* (VLDB
2005). Scaling discipline: EI-02 §8 (measure-before-shard); PgBouncer/pgcat. **New for Phase 5:** the
content-address-as-cache-validity property the C3 CDN class leans on is the same Git/Venti content-addressing
prior art (an edge cache entry is valid iff its hash matches — no staleness model); the C4 trust-scoped cache
namespace is the standard CI poisoned-cache defence (the build-cache-per-trust-boundary pattern, as in
hardened CI systems isolating fork-PR caches).

---

## 12. Open questions carried to Phase 6 (honesty register)

The Phase-3 open questions narrowed but did not all close. What Phase 6 (and Legal/DPO) still owns:

- **[OPEN → P6 Git]** Object-backed git pack/delta management + smart-transport over the `BlobStore` seam
  (§3.5, STOR-5/TE-24). The relocatability constraint + the C3 CDN clone/bundle class + the C6 mirror gate are
  DECIDED; the object-backed pack *implementation* (chunking, delta-base selection, serving from the object
  tier) is the **Git P6 deliverable**. Crypto-shred reach into reflogs/bitmaps/backups is DECIDED (per-tenant
  blob DEK); the commit-object-byte residual is the 10.9 posture.
- **[OPEN → P6 Chat]** The chat message-log engine (wide-column Scylla vs OLTP+object; §3.3) and confirmation
  that chat bodies are per-subject-keyed in the chosen store (the §5.1 rule says yes; the Chat P6 agent
  confirms feasibility) — TE-13.
- **[OPEN → P6 Issues+Knowledge]** Flexible-field physical storage/query model (JSONB property-bag vs
  materialised; the `myelin-query` field-type enum is frozen, X-3 — the *physical* storage is the subsystem
  call) and which fields are per-subject vs per-tenant keyed (driven by `classify`, §5.1).
- **[OPEN → P6/LEGAL]** BYOK/HYOK per-content-class **policy** (which classes may be HYOK; the
  cross-artifact-reference-spanning-the-boundary case), the KMIP/external-key-store adapter, and HYOK as a
  Schrems-III mitigation (GD-7). The **mechanism + the limits** are DECIDED (§6).
- **[OPEN → P6]** Cross-cell/cross-tenant **aggregate** analytics (no-PII, control-plane) over the OLAP tier
  (§3.4); the cell-local resolution for the multi-cell case rides the OQ-I PII-free pointer bridge.
- **[OPEN → P6]** All drill thresholds: the **RPO** number (proposed ≤ 5 min), **RTO** (≤ 1 h/tenant,
  ≤ 4 h/cell), restore-verify cadence, cross-seam-assertion sampling rates, and the new D-S11..D-S13 gates.
- **[OPEN → P6]** Sharding *trigger* metrics (the measured table/tenant size at which OLTP shards or a tenant
  gets a dedicated DB/cell — §8); the BUS-6 column-store promotion threshold — all measured, not predicted.
- **[OPEN → LEGAL / DPO]** (flagged; the structural floor ships regardless): the ONE free-text/immutable-content
  erasure residual posture (10.9, X-7, L-2) — Storage contributes its reach, counsel ratifies the basis once;
  the **worklog/productivity/estimate sensitivity classification** (OQ-H — special-category vs elevated; the
  works-council consultation trigger that gates per-individual OLAP rollups, C5); the **backup-window-vs-erasure-SLA
  residual number** (§7.6) + the erasure-ledger retention carve-out (GD-5/GD-14); whether tenant-offboard
  KEK-destroy suffices for an Art. 17 *tenant-wide* assertion; the C6 outbound-mirror lawful-basis for any
  permitted extra-EU transfer.

---

## 13. Cross-references

- **Reconciliation spine:** [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) (X-1
  trust-tier/`details_ref`; X-7 erasure posture; OQ-D `#step-<n>` resolution; OQ-H worklog; §8 CR fold);
  [`contract-index.md`](./contract-index.md) (the frozen surface: 11.2/11.4/11.6/11.8 + 10.5/10.9).
- **Base carried forward:** [`../03-shared-systems-architecture/storage.md`](../03-shared-systems-architecture/storage.md)
  (the Phase-3 design this refines).
- **Spine:** ADR-10 (datastore tiers), ADR-12 (PersonalDataHolder/crypto-shred), ADR-18 (restore-verification),
  ADR-11 (cells/residency), ADR-04 (bus→OLAP).
- **Sibling refined docs that consume this:** **GDPR/Audit** (crypto-shred policy, DSR fan-out, the 10.9
  residual posture + 10.8 erasure ledger + 10.5 `transfer_allowed` mirror gate); **Search** (`can_derive_plaintext_index`
  HYOK gate, purge+reindex erasure); **Agent Fabric** (HYOK opacity, agent-memory per-subject keying); **CI**
  (the C2 log tier + C1 per-subject log DEK + C4 cache namespaces); **Git** (the object-backing seam + C3 CDN
  bundles + C6 mirror gate); **Issues** (the C5 restriction-flag OLAP gate); **Tenancy/control plane** (the
  C3 CDN edge set + C6 mirror target in `residency_verify`); **all subsystems** (tier clients, blob trait,
  forward-only migrations, backup/restore gate).
- **Doctrine:** EI-02 §8 (minimal stack, content-addressing, forward-only, measure-before-shard) / §11
  (restore-verification, cross-seam integrity); EI-04 §1 (crypto-shred, erasure-vs-immutability) / §3
  (object-backed git seam) / §5 (event-volume, reindex-from-source).
