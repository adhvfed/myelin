# Phase 3 — Storage (tiers · KMS · crypto-shred · backup/restore-verification)

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md).
> Doctrine (binding): [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> §8 (minimal storage stack, content-addressing, forward-only migrations, measure-before-shard) and §11
> (a backup that has never been restored is not a backup), [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md)
> §1 (crypto-shred substrate, erasure-vs-immutability), §3 (object-backed git seam), §5 (event-volume
> seam, reindex-from-source).
> Spine bound: **ADR-10** (datastore tiers), **ADR-12** (PersonalDataHolder / crypto-shred), **ADR-18**
> (restore-verification durability gate); cross-refs ADR-11 (cells/residency), ADR-13 (glue), ADR-14
> (tech map), ADR-04 (bus → OLAP feed). Directives: **STOR-1…STOR-4**, **GD-3**, **X-1…X-5**.
> Resolves: **GD-4** (per-subject vs per-tenant crypto-shred granularity), the **cross-seam restore
> consistency point** (STOR-4), **post-restore re-erasure** (GD-14, verification half).
>
> **Foundational docs consumed (their contracts are inputs, not re-invented here):**
> [`00-platform-substrate.md`](./00-platform-substrate.md) — the `BlobStore put/get/head/delete` trait
> (§2.7), the `PersonalDataHolder` auto-registration (§3.4), the bootstrap pool/outbox (§3.3),
> forward-only migrations (§9), the stateful-component register pattern (§11.1), the telemetry contract
> (§10.2), the X-5 unit anchor (§2.10), the cross-seam restore drill **D-6** (§11);
> [`identity-and-access.md`](./identity-and-access.md) — the per-tenant DEK + **per-subject sub-key for
> profile PII** store map (§2), the pseudonym-map erasure lever (S2), `list_objects` (§8.2), the
> fail-static staleness bound **W** (§10); [`event-bus.md`](./event-bus.md) — the envelope's
> `contains_personal_data`/`pii_key_ref`/`data_role` crypto-shred routing pair (§3.1), retention +
> crypto-shred + tombstones (§4.8), the **OLAP CQRS feed off the bus** (§4.9 reindex-from-source), the
> firehose/log tier split (§4.3).
>
> **What this doc is.** Storage is **not one database** — it is the **tiering + the portability
> constraint + the cross-cutting KMS/crypto-shred mechanism + the durability gate** (overview §7.1). It
> owns the *mechanism*; GDPR/Audit owns the *policy* (when to shred, the DSR fan-out — ADR-12.3). This
> doc gives the concrete tier engines, the KMS key hierarchy and crypto-shred algorithm, BYOK/HYOK and
> its hard limits on search/agents, and the backup/restore-verification machinery with the cross-seam
> consistency point. It does **not** re-decide an ADR; where it sharpens one it cites it.
>
> **Altitude.** Phase 3 *detailed*: concrete key hierarchy, schemas, algorithms, wire shapes, failure
> modes + drills owed, and the explicit `[OPEN → P4]` hand-off. Snippets are illustrative
> signatures/schema, not implementations.
>
> **Status convention.** *DECIDED* = committed for P4/P5; *FLOOR* = partial answer + named follow-on;
> *[OPEN → P4/P5/LEGAL]* = handed forward.

---

## 0. Purpose, responsibilities, and the one-paragraph thesis

Storage owns the **durable primitives every subsystem leans on**, with residency, per-tenant envelope
encryption, and crypto-shred baked into the seam so no service can opt out (ADR-10/12; overview §7).
Concretely it owns **eight concerns**, each a named section:

1. **The three tiers + the OLAP read store** (§3) — transactional (Postgres-class OLTP), object/blob
   (S3-compatible, content-addressed, behind the narrow `put/get/head/delete` trait), log/firehose
   (append-mostly tail+archive+range-read), and the **CQRS OLAP read store** (ClickHouse-class) fed by
   the bus.
2. **The KMS key hierarchy** (§4) — the per-cell root → per-tenant KEK → per-tenant/per-subject DEK
   envelope chain that every tier threads.
3. **Crypto-shred as a deletion primitive + GD-4 granularity** (§5) — per-tenant vs per-subject, and the
   decision rule for which data gets which.
4. **BYOK/HYOK and its limits** (§6) — customer-held keys and the hard ceiling they place on what
   search/agents can do over data they cannot decrypt.
5. **Backup/restore-verification as a durability gate** (§7) — continuous log archiving + base backups,
   the automated restore-verify, the **cross-seam consistency point** (STOR-4), and **post-restore
   re-erasure** (GD-14).
6. **Scaling/sharding in the cell topology** (§8) — measure-before-shard; read-replicas + pooling first;
   the authz hot path the likely first replica.
7. **Contracts exposed/consumed** (§9) — the stable tier-client + KMS + backup surface.
8. **Failure modes + drills owed** (§10), **prior art** (§11), **required foundational changes** (§12),
   **open questions for P4** (§13).

**Thesis.** *A subsystem opens its OLTP pool, its blob prefix, and (if it owns one) its index through the
bootstrap harness; the harness wires each through the KMS so every byte at rest is per-tenant
envelope-encrypted, registers each as a `PersonalDataHolder`, and pins it to the cell's region — so
"residency + erasability + recoverability" are properties the storage seam **enforces**, not features a
service remembers to add.* Erasure of anything in an immutable or backup tier is **crypto-shred** (destroy
a key), never byte-mutation; durability is a property only the **restore-verify drill** can make real.

### 0.1 Non-negotiables every tier inherits (not repeated per section)

- **Tenant is the first column / partition key / blob-prefix component of everything** (EI-02 §1;
  ADR-11.2). No cross-tenant query path; tenant from the verified token, never the path (ID-3).
- **Every store is residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, and a
  `PersonalDataHolder`** (ADR-11/12). The harness auto-registers; a store the harness didn't wrap fails
  the `holder-registered` architecture test (substrate §3.4, GD-3).
- **No subsystem reads another subsystem's store** (ADR-01/13). There is **no generic "storage API"
  spanning subsystems** (substrate §2.8) — the boundary is the `no-cross-db` lint, not a shared
  data-access crate.
- **Migrations are forward-only and online** (substrate §9; STOR-2) — Storage owns the rule, the harness
  enforces it.
- **The cache/coordination store (Redis/Valkey-class) is NEVER a source of truth** (STOR-3) — it is a
  derived, rebuildable layer (fail-static cache §8, the dedup ledgers, the userset cache).

---

## 1. Prior art this design stands on (cited once, referenced throughout)

| Concern | Prior art / proven system | Where it lands |
|---|---|---|
| Content-addressed blob storage (hash-on-write, dedup, integrity) | Git object model (content addressing); Merkle DAG (Merkle 1987); **IPFS** CID; Venti (Quinlan & Dorward, *Venti: a new approach to archival storage*, FAST 2002); Plan 9 Fossil | §3.2 |
| S3-compatible object store, self-hostable | **MinIO**, **Ceph RADOS Gateway** (Weil et al., *Ceph*, OSDI 2006); the S3 REST API as the de-facto portable contract | §3.2, §8 |
| Envelope encryption / key hierarchy | AWS KMS / Google Cloud KMS envelope model; NIST SP 800-57 (key management) + SP 800-38D (AES-GCM); **DEK/KEK** two-tier wrapping; HashiCorp Vault Transit (self-hostable) | §4 |
| Crypto-shredding as erasure | NIST SP 800-88r1 (*Guidelines for Media Sanitization* — "cryptographic erase"); Boneh & Lipton, *A Revocable Backup System* (USENIX Security 1996) — destroy-the-key deletion of immutable/backup data | §5 |
| BYOK / HYOK / external key origin | Cloud BYOK/HYOK practice; KMIP (key-management interop); "hold your own key" / external key store patterns | §6 |
| Continuous log archiving + PITR | PostgreSQL **WAL archiving + Point-In-Time Recovery** (PG docs ch. 26); Aries write-ahead logging (Mohan et al., *ARIES*, ACM TODS 1992); pgBackRest / WAL-G base-backup + WAL stream | §7 |
| Restore-verification / "untested backup ≠ backup" | Google **SRE** ch. 26 (*Data Integrity*); the "restore drills" / disaster-recovery testing discipline; chaos-engineering for durability | §7.4 |
| Cross-system / cross-seam consistency on restore | Distributed snapshot (Chandy–Lamport 1985, *consistent global state*); CDC log offsets as the linearisation cursor (Kleppmann, *DDIA* ch. 11) | §7.3 |
| CQRS / read-model off the log | Young/Dahan CQRS; Fowler "CQRS"; the OLAP read store fed by the event log (ADR-10; bus §4.9) | §3.4 |
| Columnar OLAP engine | **ClickHouse** (MergeTree); C-Store / Vertica (Stonebraker et al., VLDB 2005) — column stores for analytic scans | §3.4 |
| Measure-before-shard; replicas + pooling first | EI-02 §8; "premature sharding is its own outage"; read-replica + connection-pool scaling (PgBouncer/pgcat) | §8 |
| Forward-only / expand→contract migrations | Stripe online migrations; gh-ost; Fowler ParallelChange (consumed from substrate §9) | §3.1, §12 |

These are the **defaults-to-beat** and the **justified adoptions**. Deviations (e.g. the GD-4 per-subject
granularity rule, the cross-seam offset cursor) are called out in writing in the relevant section.

---

## 2. The store map (stateful-component register — X-4)

Per X-4, every stateful component Storage owns or governs, with a shared-state/sharding/blast-radius note
and its **crypto-shred unit** (the key whose destruction erases it). Specialized stores (authz tuples,
search index, workflow store) are owned by their systems but inherit Storage's KMS/residency seam and
appear here for the cross-seam picture.

| # | Component | Engine (ADR-14) | Holds | Shard key | Blast radius | Crypto-shred unit |
|---|---|---|---|---|---|---|
| T1 | **OLTP (per-subsystem)** | Postgres-class | domain state: repo/PR metadata, issues, doc blocks/rows, message metadata, run state, **+ each service's `outbox`** | `(tenant, region)` + table PK | one subsystem's writes in one tenant (RLS) | per-tenant DEK; **per-subject sub-key for free-text/profile PII columns** (§5) |
| T2 | **Object/blob** | S3-compatible (MinIO/Ceph) | LFS blobs, CI artifacts/caches, doc media, attachments, avatars, clone bundles, **base backups** | `(tenant, region)` prefix + content hash | one tenant's blobs; dedup-shared within tenant only | per-tenant DEK (object DEK wraps content key) |
| T3 | **Log/firehose** | append-mostly tier (object-backed segments + range index); wide-column (Scylla-class) candidate for chat log | CI logs, chat message log, collab op-stream archive | `(tenant, region)` + stream/run id | one tenant's logs of one stream class | per-tenant DEK; per-subject for chat bodies (§5) |
| T4 | **OLAP read store** | ClickHouse-class columnar | CQRS analytics read model (issue analytics, delivery health), fed by the bus | `(tenant, region)` + table | derived — **rebuildable via reindex-from-source** (bus §4.9) | per-tenant DEK (derived; inherits source) |
| T5 | **KMS** | Vault-Transit-class / HSM-backed | the key hierarchy (§4): per-cell root, per-tenant KEK, DEK custody/wrapping, BYOK/HYOK adapters | `(cell)` root, `(tenant)` KEK | **highest-value component**: a tenant's KEK loss = that tenant's data unrecoverable (by design for shred) | n/a (it *is* the shred lever) |
| T6 | **Backup/archive** | object-tier base backups + continuous WAL/log archive | base backups + the WAL/log stream + the OLAP/log long-term record | `(tenant, region)` | one tenant/cell's recoverability | per-tenant DEK (backups are ciphertext; shred ⇒ unrecoverable) |
| T7 | **Cache/coordination** | Redis/Valkey-class | fail-static cache, dedup ledgers, userset cache, rate-limit counters — **NEVER source of truth** (STOR-3) | `(tenant, region, key)` | one cell; cold cache → brief degrade | ephemeral; TTL ≤ W |
| — | *(specialized, owned elsewhere, inherit the seam)* | authz tuples (Id S3), search index, workflow store | — | — | — | per-tenant DEK (search-index PII purge+reindex on erase; bus history crypto-shred) |

**Derived stores (T4, T7) are reindex-from-source primitives** (SEARCH-1/REF-4; EI-04 §5.3): rebuildable
by replaying the source through the live consumer path (bus §4.9) — no bespoke recovery code. **Systems of
record (T1, T2, T3, the specialized stores)** are gated by the restore-verification drill (§7; ADR-18).
**T5 (KMS) is the single most blast-radius-sensitive component** and gets its own availability/durability
treatment (§4.5).

---

## 3. The three tiers + the OLAP read store

### 3.1 Tier 1 — Transactional (OLTP), Postgres-class

**Decision (DECIDED, per ADR-10/14).** **PostgreSQL-class, one database per service** (system of record;
no shared tables, no cross-service joins — logical references validated through the owning service;
EI-02 §8). JSONB + GIN + generated columns for flexible/user-defined fields (issue custom fields,
knowledge db properties — the TE-17 flexible-field model is each subsystem's P4 call). Distributed-SQL
(CockroachDB/Yugabyte-class) is reserved for **only where a single tenant's shard is *measured* to
outgrow Postgres** (§8; never predicted — EI-02 §8).

What Storage *owns* about the OLTP tier (vs the subsystem owning its schema):

- **The tenant-scoping guard.** Every table is `(tenant, region)`-first with **row-level security (RLS)**
  bound to the injected tenant; a tenant-less query **fails to compile** via the substrate's
  `tenant-predicate` lint (substrate §2.11). This is the IDOR floor (D-7 drill).
- **The per-tenant envelope-encryption seam.** Columns classified personal-data (ADR-12.5) are encrypted
  under the tenant DEK (and **free-text/profile columns under a per-subject sub-key** — §5); the harness's
  thin query layer transparently wraps/unwraps via the KMS client. The classification tag is schema-level
  (`myelin-gdpr classify(field)`, overview §8.4), feeding the generated data map (ADR-12.6).
- **The outbox lives here** (bus §3.2): each service's `outbox` table is in *its own* OLTP database, in
  the same transaction as the state change. This is the **anchor of the cross-seam consistency point**
  (§7.3): the outbox row's `seq` and the OLTP commit are atomic, so OLTP commit order == event order.
- **Forward-only online migrations** (substrate §9; STOR-2): expand→backfill→contract, no rollback files,
  no blocking `ALTER` on a flagged-hot table, **lock time measured against a restored copy first** (§7.4
  produces that copy — the durability machinery and the migration discipline reinforce each other).
- **Read-replica awareness** (§8): the pool can route reads to a replica; the authn/authz hot path is the
  likely first dedicated replica (ID-4).

### 3.2 Tier 2 — Object/blob, S3-compatible, content-addressed

**Decision (DECIDED, per ADR-10/STOR-1).** **S3-compatible object store** (MinIO or Ceph RADOS Gateway —
self-hostable, EU-deployable; ADR-11), behind the **narrow content-addressed `put/get/head/delete`
trait** the substrate defines (substrate §2.7) so filesystem↔object-store is a one-line swap. The S3 REST
API is the portable contract; **no proprietary global managed object service** (ADR-11).

```rust
// Re-stated from substrate §2.7 — this is the trait Storage IMPLEMENTS; subsystems consume it.
pub trait BlobStore {                                    // hash-on-write; fs-vs-object is a one-line swap
    fn put(&self, bytes: &[u8]) -> Result<ContentHash>;  // content address = the hash (Git/Venti model)
    fn get(&self, h: &ContentHash) -> Result<Vec<u8>>;
    fn head(&self, h: &ContentHash) -> Result<BlobMeta>;
    fn delete(&self, h: &ContentHash) -> Result<()>;     // crypto-shred is the real erasure (ADR-12.3)
}
```

**Content-addressing (DECIDED).** The address **is** the hash of the (plaintext) content: `put` computes
`ContentHash = H(bytes)`, giving **dedup and integrity for free** (Git's object model; Venti, FAST 2002).
Two design decisions the content-addressing forces:

- **Hash algorithm: BLAKE3 for new blobs** (fast, parallel, tree-hash; no length-extension), with the
  hash **self-describing** (multihash-style prefix) so SHA-256 can coexist — the git subsystem's SHA-1↔256
  object identity (TE-23) is a *separate* concern (git objects keep git's own hashing; this trait is the
  *blob backing*, not the git object model). `[OPEN → P4 Git]` how git packs ride this (STOR-5 seam, §3.5).
- **Encryption and content-addressing interact (the subtle part).** If we encrypt-then-address, dedup
  breaks (the same plaintext under two tenant keys has two addresses → no cross-tenant dedup — which is
  **correct**: cross-tenant dedup would be a residency/isolation leak). So: **address by plaintext hash
  *within a tenant's keyspace*, store ciphertext.** Dedup is **per-tenant** (a tenant's two identical
  blobs share one ciphertext object); cross-tenant dedup is deliberately forgone for isolation. The object
  is `tenant-prefix/content-hash → AES-GCM(content, per-blob content-key wrapped by tenant DEK)` (§4.4).

**Erasure of a blob is crypto-shred, not `delete`** for anything reachable from an immutable/backup tier:
destroying the per-tenant (or per-subject) key renders the ciphertext unrecoverable in the object store
*and in every base backup that copied it* (§5). `delete` is for live-tier space reclamation and dangling
GC, not for satisfying erasure of backed-up data.

### 3.3 Tier 3 — Log/firehose (append-mostly)

**Decision (DECIDED, per ADR-10/14; consumes bus §4.3).** The high-volume **ephemeral firehose** (CI log
lines, chat presence/typing, collab op-streams) rides a **separate transport from the durable bus** (the
firehose split — bus §4.3; this is mandatory, not optional, ADR-04.5). Storage owns the **durable archive
of the firehose**, not the live ephemeral fan-out (that is the bus's firehose transport). The shape:

- **CI logs / collab-op archive:** **append-mostly object-backed segments** — frames are appended to a
  current segment, sealed segments are flushed to the object tier (T2) as content-addressed blobs with a
  **range index** `(run/step, byte-range) → (segment-blob, offset)` in OLTP for tail + range-read. This
  is the standard "log as immutable segments + a small index" pattern; the segment blobs inherit T2's
  encryption + crypto-shred.
- **Chat message log:** a **wide-column (Scylla/Cassandra-class) candidate** (TE-13, SC-10) — high write
  throughput, partition-by-`(tenant, channel)`, clustering by time — is the directional engine **the Chat
  P4 agent decides** (it may instead keep messages in OLTP + object media). Storage pins only the
  constraint: per-tenant envelope encryption, **chat bodies under a per-subject sub-key** (so a member's
  erasure crypto-shreds their messages — §5), residency-pinned, a `PersonalDataHolder`.

The durable bus carries only **pointer events** into these streams (`ci.log.available`,
`knowledge.doc.updated`) — an agent is never woken per log line (bus §4.3).

### 3.4 The OLAP read store (CQRS, fed by the bus)

**Decision (DECIDED, per ADR-10; consumes bus §4.9).** A **ClickHouse-class columnar read store** holds
the CQRS analytics read model (issue analytics, delivery health, roadmap delivery-state). It is **fed
async off the durable event stream** (the bus is the analytics source of truth; SC-5) via the standard
**idempotent consumer template** (substrate §5; dedup on `event_id`), never by scanning OLTP — so
analytics scans cannot kill the transactional store (ADR-10 §Consequences). Three properties Storage pins:

- **Reindex-from-source is the *only* rebuild path** (SEARCH-1 analogue): the OLAP store is wiped and
  rebuilt by asking owners to re-emit `*.snapshot` events through the live consumer path (bus §4.9). There
  is no "read OLTP into ClickHouse" backdoor. This makes the **reindex-from-cold parity drill** (bus D-5;
  §10 here) the OLAP store's recoverability proof.
- **It is a `PersonalDataHolder`** (it derives from personal data): erasure = drop/rebuild the affected
  rows + crypto-shred any inline-PII columns; it inherits the tenant DEK.
- **It is residency-pinned and crypto-shred-capable** like every tier (it is *not* a global warehouse —
  one tenant's OLAP rows live in that tenant's cell; cross-tenant/cross-cell analytics is a control-plane,
  aggregate-only, no-PII concern, `[OPEN → P4]`).

### 3.5 The git object-backing seam (STOR-5 — FLOOR, designed-not-built)

Per STOR-5 / EI-04 §3, world-scale git wants authoritative objects/packs in the **object tier (T2)**, not
node-local disk. Storage commits the **seam now** so the v1 git data model is **never node-pinned**: git
packs and loose objects are addressed through the `BlobStore` trait (§3.2), so the "local-disk →
object-store-backed packs" transition is a backing swap, not a rewrite. **Floor named:** v1 may run packs
on local disk behind the trait; **follow-on:** object-backed pack/delta management + the smart-transport
paths are the **Git P4 deliverable** (TE-24). The *relocatability* (repos are not pinned to a node) is the
DECIDED constraint here; the object-backed implementation is the named next step.

---

## 4. The KMS key hierarchy (per-tenant envelope encryption)

**Decision (DECIDED, per ADR-12.3).** A **three-level envelope-encryption hierarchy** (AWS/GCP KMS
envelope model; NIST SP 800-57; Vault Transit as the self-hostable engine). Envelope encryption is the
prior art because it makes **crypto-shred a key-destruction operation, not a data-scan** (the whole point
— §5).

### 4.1 The hierarchy

```
  ┌──────────────────────────────────────────────────────────────────────┐
  │  L0  CELL ROOT KEY (RK)  — per cell; in an HSM / sealed KMS;           │
  │      never leaves the KMS boundary; rotated on a long cycle.          │
  └───────────────┬──────────────────────────────────────────────────────┘
                  │ wraps
  ┌───────────────▼──────────────────────────────────────────────────────┐
  │  L1  TENANT KEK (Key-Encryption-Key)  — one per (tenant, region).      │
  │      The crypto-shred lever for TENANT-granularity erasure /          │
  │      tenant offboarding (destroy the KEK ⇒ all the tenant's DEKs       │
  │      unwrappable ⇒ all ciphertext dead, incl. backups).               │
  └───────────────┬──────────────────────────────────────────────────────┘
                  │ wraps
  ┌───────────────▼──────────────────────────────────────────────────────┐
  │  L2  DATA-ENCRYPTION KEYS (DEKs)  — the keys that actually encrypt     │
  │      bytes (AES-256-GCM). Two sub-classes:                            │
  │        • per-tenant DEK (per tier/purpose, rotated)                    │
  │        • PER-SUBJECT DEK  — for free-text/profile PII, chat bodies,   │
  │          agent memory: the crypto-shred lever for SUBJECT-granularity  │
  │          erasure (a single person's erasure — §5 / GD-4).             │
  └──────────────────────────────────────────────────────────────────────┘
```

- **L0 Cell root (RK):** lives in an HSM or a sealed KMS (Vault auto-unseal / PKCS#11); never exported.
  It wraps tenant KEKs. Its blast radius is the cell, so it is the most protected key (§4.5 availability).
- **L1 Tenant KEK:** one per `(tenant, region)`. **Destroying it is tenant-granularity crypto-shred** —
  the offboarding / tenant-decommission lever (Id §11 tenant decommission; `myelin tenant offboard`).
- **L2 DEKs:** the working keys. **Per-tenant DEKs** encrypt the bulk of a tenant's data (rotated on a
  schedule; rotation re-wraps, doesn't re-encrypt — §4.3). **Per-subject DEKs** encrypt the data classes
  whose erasure must be *individual* (one person, not the whole tenant) — this is the GD-4 resolution
  (§5).

### 4.2 Envelope-encryption write/read path (DECIDED)

```
WRITE:  plaintext ──AES-256-GCM(DEK)──► ciphertext + IV + auth-tag
        the DEK is itself stored WRAPPED: wrap(DEK, KEK) alongside the ciphertext (a key-ref, not the key)
        the row/blob stores: { ciphertext, iv, tag, key_ref = (kek_id, dek_id, dek_version) }

READ:   resolve key_ref ► KMS.unwrap(wrapped_DEK, KEK) ► AES-256-GCM-decrypt(ciphertext, DEK)
        DEKs are cached in-process for the request, bounded TTL, zeroized after (never persisted plaintext)
```

The **`key_ref`** (which KEK, which DEK, which version) travels with every ciphertext — in the OLTP row,
the blob metadata, and the **envelope's `pii_key_ref`** for the rare inline-PII event (bus §3.1). This is
the X-5 reconciliation point: `pii_key_ref = kms://<tenant>/<dek-epoch>/<class>` is the single canonical
key-reference shape every tier and the bus share.

### 4.3 Key rotation (forward-only; re-wrap, not re-encrypt)

Rotation is **envelope re-wrap**, not bulk re-encryption: a new DEK version is minted and used for *new*
writes; old ciphertext keeps its old `key_ref` and old DEK (still wrapped under the live KEK). This makes
rotation **O(keys), not O(data)** — the envelope model's central operational win. KEK rotation re-wraps
the (small) set of DEKs, not the (large) ciphertext. Rotation is **forward-only** (a rotated-out key is
retired, never "un-rotated"); a *compromised* key triggers a backfill re-encryption (an expand→backfill
job, substrate §9) rather than a rollback.

### 4.4 Object-tier specifics (per-blob content keys)

For T2 blobs, a **per-blob random content key** encrypts the bytes, and that content key is **wrapped by
the tenant (or per-subject) DEK** (`blob = AES-GCM(bytes, CK); stored: AES-GCM(CK, DEK)`). This keeps the
content-address (plaintext hash, §3.2) stable while letting key rotation/shred operate at the DEK level.
Dedup remains per-tenant (identical plaintext → identical content-hash → one ciphertext object + one
wrapped CK).

### 4.5 KMS availability & blast radius (it is the highest-value component — X-4)

The KMS (T5) is the **single most blast-radius-sensitive component**: if it is unreachable, *nothing*
decrypts. Mitigations (the stateful-component plan for T5):

- **The KMS is in-cell** (residency: a tenant's keys never leave its region/cell). The L0 root is
  HSM/sealed.
- **Read-path DEK caching** (bounded, in-process, zeroized) means a *transient* KMS hiccup degrades like
  the **fail-static** pattern (substrate §8): already-resolved DEKs keep serving within their bounded TTL;
  a hard-down KMS makes the dependent service report **not-ready** (liveness≠readiness, substrate §4.3) —
  it does **not** fail-open (we never serve plaintext we can't authenticate).
- **KMS HA**: the KMS is itself replicated within the cell (Raft-class, Vault HA); the root is recoverable
  from sealed shares (Shamir split — Vault unseal). **Losing a tenant KEK is unrecoverable by design**
  (that *is* crypto-shred), so KEK *backup* is the sensitive inverse of crypto-shred: the KEK is backed up
  **only** under the cell root and **only** while the tenant is live; offboarding/erasure destroys the KEK
  *and its backups* (the §7.5 post-restore re-erasure interplay).
- **Drill owed (§10):** KMS-hiccup drill (degrade, don't cascade) + a crypto-shred-reaches-backups drill
  (the destroyed key stays destroyed across a restore — §7.5).

---

## 5. Crypto-shred as a deletion primitive — GD-4 resolved (per-subject vs per-tenant)

**Crypto-shredding is a first-class deletion primitive** (ADR-12.3; NIST SP 800-88r1 "cryptographic
erase"; Boneh & Lipton 1996): destroy the key, and the ciphertext — in live DBs, **in immutable logs, and
in every backup** — becomes unrecoverable without ever mutating the immutable bytes. This is *the* answer
to "erasure vs append-only/immutable" for everything except non-pseudonymised git history (the named GD-1
floor, owned with Git P4 + Legal; bus §4.8).

### 5.1 The GD-4 decision: a classification-driven granularity rule

GD-4 ("per-subject vs per-tenant crypto-shred granularity") is **resolved by a decision rule keyed on the
data's classification + erasure unit**, not a single global choice:

| Data class | Granularity | Key | Why |
|---|---|---|---|
| **Free-text / profile PII** (names, emails, avatars, bios, comment bodies with PII, knowledge free-text, **chat message bodies**, **agent memory/embeddings**) | **PER-SUBJECT DEK** | one DEK per `(tenant, subject)` | An individual's Art. 17 erasure must delete *that person* without touching the tenant. Per-subject keying makes one person's erasure a single key-destroy — the **subject-granular** lever. |
| **Bulk tenant-content** (issue field values, doc block structure, repo/PR metadata, run state) | **PER-TENANT DEK** (per tier/purpose) | per-tenant DEKs, rotated | Mostly non-personal or pseudonym-referenced (references-not-payloads). Erasure of an individual here is **tombstone/pseudonymise** (delete the *identity* in Id's pseudonym map, not the *fact* — EI-04 §1), so per-subject keying buys nothing and costs key-management. |
| **Tenant-wide** (everything, for offboarding) | **PER-TENANT KEK** (L1) | the tenant KEK | `tenant offboard` / decommission destroys the KEK ⇒ every DEK unwrappable ⇒ the **whole tenant** is crypto-shredded in one operation, backups included. |
| **Inline-PII events** (the rare event that must carry PII inline) | **PER-TENANT, optionally per-subject** | `pii_key_ref` on the envelope | bus §4.8: default is references-not-payloads (no inline PII, nothing to shred); the rare exception is envelope-encrypted and shred-routed. |

**The rule (DECIDED):** *data whose erasure unit is the **individual subject** is keyed per-subject; data
whose erasure is satisfied by **pseudonymisation/tombstoning** is keyed per-tenant; tenant offboarding is
the KEK.* The schema-level personal-data classification (ADR-12.5, `classify(field)`) **drives the key
choice automatically** — a field tagged `personal-data, erasure=subject` is wired to a per-subject DEK by
the harness, so "we forgot to make this erasable per-person" is a classification miss caught by the
generated data map (ADR-12.6), not a silent gap.

### 5.2 Why not always per-subject (the honest trade-off)

Per-subject keying for *everything* would make tenant-wide operations O(subjects) in key management and
explode the DEK count (millions of keys per large tenant), and most bulk tenant-content is
**pseudonym-referenced** (the personal data is in Id's profile record / pseudonym map, not in the bulk
store), so erasing it is already solved by the pseudonym-map delete (Id S2) + references-not-payloads — a
*structural* answer that needs no per-row key. Per-subject keying is reserved for the data classes where
the PII is **genuinely inline and individually erasable** (free text, chat bodies, profile, agent memory).
This is the *measured-minimum* granularity, not a maximalist one.

### 5.3 The crypto-shred algorithm (DECIDED) and its reach

```
erase(subject, tenant):                                    # called by the DSR orchestrator (ADR-12.2)
  1. Id.erase(subject)            → delete the pseudonym map (S2) + profile record;
                                    git/bus/audit history now holds only the opaque pseudonym (EI-04 §1).
  2. KMS.destroy(per_subject_DEK(tenant, subject))         # the crypto-shred: free-text/chat/profile/
                                    agent-memory ciphertext (live AND in backups) is now unrecoverable.
  3. Search.purge+reindex(subject)                         # index isn't key-shred; it PURGES + reindexes
                                    (the index is plaintext-derived; bus tombstone drives it).
  4. Refs.tombstone(subject)      # unfurls degrade gracefully.
  5. Bus.erase(subject)           # crypto-shred any inline-PII event keys + emit *.erased tombstones.
  6. record erase receipt (audit holder, carve-out).
```

- **Crypto-shred reaches backups by construction**: because backups store *ciphertext* (the data was
  encrypted at rest under the now-destroyed key), the destroyed key makes the backup copies unrecoverable
  **without restoring and re-deleting them** — the answer to "backups defeat erasure." This is asserted by
  the **crypto-shred-reaches-backups drill** (§10, D-S4) and reconciled with restore (§7.5: a restore must
  **not** resurrect a destroyed key).
- **The search index is the exception**: it stores *plaintext-derived* tokens, so erasure is **purge +
  reindex** (not key-shred) — the index must drop the subject's documents and rebuild (ADR-12; the
  "we-forgot-the-search-index" structural-failure guard, GD-3). This is why the holder list is exhaustive.
- **Reach is verified, not assumed**: the **erasure-reaches-every-holder drill** (T-5) asserts the DSR
  fan-out hit *every* `PersonalDataHolder` — OLTP, object, log, OLAP, search, refs, bus, agent memory,
  notif history, authz tuples, caches/CDN, and **backups**.

### 5.4 The git-history floor (GD-1 — named, not solved here)

Author name/email **baked into the commit hash** is the genuinely-unsolved half (EI-04 §1). Crypto-shred
**does not** reach it (the bytes are immutable and hash-load-bearing). **Floor named:** Storage provides
the crypto-shred substrate for *everything else*; non-pseudonymised git history's only levers are
**history-rewrite (changed hashes, disruptive)** or a **documented lawful-basis limit**, decided in the
**named "Erasure vs. Immutability reconciliation" deliverable** (GD-1), co-owned with the **Git P4 agent +
Legal/DPO**, gating the git data model (pseudonymous-commit-by-default is a *commit-time prerequisite*,
GIT-1). Storage's contribution to that deliverable: crypto-shred reach into **reflogs, bitmaps, and
backups** of the pack tier (those *are* shreddable via the per-tenant blob DEK), versus the commit-object
bytes (which are not).

---

## 6. BYOK / HYOK and its hard limits (what search/agents can/can't do)

**Decision (DECIDED directional; the *limits* are DECIDED, the *full policy* is `[OPEN → P4/LEGAL]`).**
Customer-managed keys are the natural substrate for sovereign crypto-shred (EI-04 §1) and a real
enterprise/public-sector requirement, offered at three levels with **explicitly different capability
ceilings**:

| Model | Key custody | Crypto-shred control | **Capability ceiling (the hard limit)** |
|---|---|---|---|
| **Platform-managed** (default) | Myelin KMS (T5), per-tenant KEK | Myelin, on DSR/offboard | Full: search indexes plaintext, agents read plaintext, OLAP analytics work — Myelin can decrypt inside the cell to derive indexes/embeddings. |
| **BYOK** (bring your own key) | Customer imports/owns the **KEK material**; Myelin KMS holds it for operational use; customer can **revoke** | Customer holds the ultimate shred lever (revoke the KEK ⇒ crypto-shred) | **Same as platform-managed *while the key is available*** — Myelin still decrypts in-cell to index/embed; the difference is the customer can *instantly* crypto-shred by revoking. Search/agents work normally until revocation. |
| **HYOK** (hold your own key) | Customer holds the key **outside Myelin**; Myelin **never possesses** decryptable key material; decryption requires a customer-controlled call | Customer holds *everything* | **Severely limited by construction**: Myelin **cannot** build a plaintext search index, **cannot** compute embeddings/RAG, and **agents cannot read** HYOK-encrypted content — because Myelin never sees plaintext. Only **metadata not under HYOK** (timestamps, IDs, references) is searchable/agent-visible. This is the inherent, unavoidable trade-off, stated up front. |

### 6.1 The limits, stated honestly (the part teams get wrong)

- **You cannot index what you cannot decrypt.** A full-text/semantic index and agent RAG **require
  plaintext at index/embed time**. Under **HYOK**, that plaintext is never available to Myelin, so HYOK
  content is **opaque to search and agents** — only its non-HYOK metadata (the `ArtifactRef`, timestamps,
  authorship pseudonym, non-PII structured fields the tenant chose to leave platform-keyed) is searchable.
  This is **not a bug to be fixed later**; it is the definitional consequence of "Myelin never sees the
  bytes." We surface it as an explicit per-space/per-repo HYOK setting with a clear "this content will not
  be searchable or agent-readable" warning (a UX honesty requirement, DL).
- **BYOK is the pragmatic middle**: the customer gets the **instant-shred** sovereignty lever (revoke ⇒
  dead) *without* losing search/agents, because Myelin still decrypts *in-cell, in-region* to derive
  indexes — at the cost that, while the key is live, Myelin *can* read the plaintext (the standard
  BYOK-vs-HYOK trade). The residency guarantee (in-region only) bounds the exposure.
- **Crypto-shred under BYOK/HYOK reaches backups identically** (the backup is ciphertext under the
  customer's key; revocation/destruction kills the backup copies too — §5.1).
- **`[OPEN → P4/LEGAL]`**: the per-subsystem policy for *which* content classes may be HYOK (e.g. can a
  repo be HYOK while its issues are platform-keyed? — the cross-artifact reference graph spans the
  boundary), the KMIP/external-key-store adapter, and the legal posture (HYOK as a Schrems-III / sovereign
  mitigation — GD-7) are handed forward. The **mechanism** (three levels, the trait below) and the
  **limits** are DECIDED here.

### 6.2 The KMS adapter contract (swappable key origin)

```rust
pub trait KeyOrigin {                              // platform-managed | BYOK | HYOK behind one trait
    fn wrap(&self, dek: &Dek, tenant: TenantId) -> Result<WrappedDek>;
    fn unwrap(&self, w: &WrappedDek, tenant: TenantId) -> Result<DekHandle>;   // HYOK: a CALL OUT, may deny
    fn can_derive_plaintext_index(&self) -> bool;  // platform/BYOK = true; HYOK = FALSE (drives §6.1)
    fn destroy(&self, key_id: KeyId) -> Result<()>; // crypto-shred (BYOK/HYOK: customer-initiated)
}
```

`can_derive_plaintext_index()` is the **structural enforcement** of §6.1: Search and the Agent Fabric
**must consult it** before indexing/embedding a content class; HYOK content is skipped (and marked
not-searchable), so the limit is enforced by code, not by hope. This is the same swappable-adapter mandate
that swaps mock→real agents (ADR-12.8), generalized to key origin.

---

## 7. Backup / restore-verification as a durability gate (ADR-18)

**"A backup that has never been restored is not a backup"** (EI-02 §11; ADR-18). Storage owns the
mechanism; Phase 5 owns the drill execution + thresholds.

### 7.1 Continuous archiving + base backups (tight RPO)

**Decision (DECIDED, per ADR-18; PostgreSQL WAL-archiving + PITR prior art).**

- **OLTP (T1):** **continuous WAL archiving** + **periodic base backups** (pgBackRest/WAL-G-class) to the
  object tier (T2), giving **point-in-time recovery (PITR)** to any moment in the retention window. **RPO
  target (default-to-beat): ≤ 5 minutes** (continuous WAL ships small segments; the residual is the
  un-archived WAL tail). Base backups daily; WAL continuously.
- **Object tier (T2):** **versioned + replicated** within the cell (and cross-AZ in-region); content
  addressing makes integrity verifiable (re-hash on read). Backups of T2 are the *base backups themselves*
  living here, plus object versioning for accidental-overwrite recovery.
- **Log tier (T3):** sealed segments are already immutable object blobs (T2) → backed up with T2; the
  range index (OLTP) is backed up with T1.
- **OLAP (T4) + caches (T7) + derived indexes:** **NOT backed up — rebuilt** via reindex-from-source (bus
  §4.9). Backing up a derived store is wasteful and drift-prone; the source + the replay path is the
  recovery (this is the SEARCH-1/reindex-from-source resilience primitive).
- **KMS (T5):** keys are backed up **only under the cell root, only while the tenant is live** (§4.5); a
  crypto-shredded key is **excluded from backup** (it must stay dead — §7.5).

**RTO target (default-to-beat):** a single-tenant restore ≤ 1 hour; a cell restore ≤ 4 hours. Phase 5
ratifies the numbers (T-5); they are the gate thresholds.

### 7.2 The RPO/RTO posture per tier (summary)

| Tier | Backup mechanism | Recovery | RPO (default) | RTO (default) |
|---|---|---|---|---|
| T1 OLTP | WAL archive + base backup (PITR) | restore base + replay WAL to cursor | ≤ 5 min | ≤ 1 h/tenant |
| T2 Object | versioned + in-region replicated | object-version restore / re-replicate | ~0 (versioned) | minutes |
| T3 Log | sealed segments in T2 + index in T1 | with T2 + T1 | with T1/T2 | with T1/T2 |
| T4 OLAP | **none — reindex-from-source** | replay `*.snapshot` via bus §4.9 | n/a (derived) | rebuild time |
| T5 KMS | sealed root + KEK backup (live tenants only) | unseal + restore KEKs | ~0 | minutes (critical-path) |
| T7 Cache | **none — rebuildable** | warm from source | n/a | seconds (cold-cache degrade) |

### 7.3 The cross-seam consistency point (STOR-4 — the hard part)

A restore that resurrects a row pointing at a **missing blob**, or an OLTP state **ahead of** the events
that derived the search index, is **silent corruption** (ADR-18; EI-02 §11). The seam to make consistent:
**OLTP rows ↔ object/blob ↔ search index ↔ event-log offsets** must restore to **one mutually consistent
point** (STOR-4).

**Decision (DECIDED): the cross-seam linearisation cursor is the per-aggregate outbox `seq` / event-log
offset.** Because the **outbox row is written in the same OLTP transaction as the state change** (bus
§3.2), and the bus delivers in `(aggregate, seq)` order, the **event-log offset is a consistent cursor
across all seams**:

```
Restore-to-consistent-point(target_offset T):
  1. OLTP  → PITR-restore to the WAL position whose committed outbox rows have seq ≤ T.
  2. Object→ content-addressed: every ContentHash referenced by the restored OLTP rows is present
            (re-hash to verify integrity). A referenced-but-missing hash = FAIL (the silent-corruption case).
  3. Search/Refs/OLAP (derived) → DO NOT restore from backup; REINDEX-FROM-SOURCE up to offset T
            (replay *.snapshot/events through the live consumer path, bus §4.9) → derived == source by
            construction, no drift.
  4. Bus offsets → consumers resume at T; dedup ledgers (T7, rebuildable) prevent double effects.
  5. KMS → restore tenant KEKs EXCEPT any crypto-shredded since the backup (§7.5 post-restore re-erasure).
```

The genius of the design (consumed from bus + Id): **derived stores are never restored from their own
backups — they are rebuilt from the source up to the same offset**, so "OLTP ↔ index consistency" is
*structural* (the index is a deterministic function of the events up to T), not something a backup pair
has to be lucky enough to share. The only true cross-seam pairing to assert is **OLTP rows ↔ blob
presence** (Chandy–Lamport-style: a restored row's referenced content must exist), which content-addressing
makes a hash-existence check.

### 7.4 Automated restore-verification (the durability gate, CI-wired)

**Decision (DECIDED, per ADR-18).** Restore-verification is **automated, periodic, and wired into CI** —
durability is gated continuously, not hoped for. The drill (Phase 5 owns mechanics; Storage owns the
machinery + assertions):

```
restore-verify (scheduled, gated):
  1. Spin a clean target; restore T1 (base + WAL to offset T), T2 (referenced blobs), T5 (KEKs).
  2. Reindex T4/Search/Refs from source to offset T (bus §4.9).
  3. ASSERT no loss:   row count / checksum parity for sampled aggregates vs a pre-backup snapshot.
  4. ASSERT cross-seam: every restored row's blob ContentHash present + integrity-verified (re-hash);
                        derived index == source-replay (reindex-from-cold parity, bus D-5).
  5. ASSERT erasure held: a subject erased BEFORE the backup is STILL erased after restore (no
                        resurrected pseudonym map, no recoverable per-subject-DEK ciphertext) — §7.5.
  6. Emit a GREEN ARTIFACT on pass (T-4: proven, not claimed); RED gate fails CI (T-2: never weaken it).
```

This also produces the **production-scale restored copy** that online migrations rehearse lock-time
against (substrate §9.2) — the durability machinery and the migration-safety discipline share one
artifact.

### 7.5 Post-restore re-erasure (GD-14)

A restore is a **point-in-time** rebuild; it can **resurrect data erased *after* that point** (a backup
from before an erasure still contains the pre-erasure ciphertext/pseudonym). **Decision (DECIDED): every
restore runs a mandatory post-restore re-erasure pass** against the **erasure ledger** (the durable,
tamper-evident record of every completed erasure, held by GDPR/Audit):

```
post-restore-re-erasure(restored_set, erasure_ledger):
  for each erasure E completed AFTER the backup's point-in-time:
     re-apply E to the restored data:
       - re-destroy the per-subject DEK / re-delete the pseudonym map (the key the backup re-introduced)
       - re-purge+reindex Search; re-tombstone Refs; re-emit *.erased
  assert: 0 resurrected subjects (the restore-verify drill step 5 asserts this).
```

The **erasure ledger is itself NOT crypto-shred-erasable** (it must survive to drive re-erasure) — it
stores only the **opaque subject id + erasure timestamp + which keys/holders were shredded**, never the
PII (so the ledger is not itself a PII resurrection vector). This closes GD-14: a restore cannot
silently un-erase a person. Co-owned with GDPR/Audit (they own the ledger; Storage owns the re-apply
mechanism on restore).

### 7.6 The backup-window vs erasure-SLA residual (named, bounded)

Between an erasure and the next base-backup cycle, an *old* backup still holds the pre-erasure ciphertext —
but it is **ciphertext under a now-destroyed key** (per-subject DEK destroyed in step 2 of §5.3), so it is
**already unrecoverable** even before re-erasure. The residual is only the narrow case of a **key that was
backed up before destruction**: §7.5's re-erasure + §4.5's "shredded keys excluded from backup" close it.
The residual exposure window is **bounded by the backup retention period and the post-restore re-erasure
pass**, and is a **DPO-ratified, documented** number (`[OPEN → LEGAL]`, ties to GD-5/GD-14) — not a silent
gap.

---

## 8. Scaling / sharding in the cell topology (measure-before-shard)

**Decision (DECIDED, per ADR-10/EI-02 §8; §(e) prior).** **Measure before you shard; read-replicas +
connection pooling first; premature sharding is its own outage.**

- **In-cell, tenant-partitioned.** Every tier is cell-local and `(tenant, region)`-keyed (ADR-11). Scale
  = **add cells**, not bigger global stores (ADR-11.1). Object/log tiers are content-addressed +
  residency-pinned.
- **First scaling move = read-replicas + pooling, NOT sharding** (EI-02 §8; §(e) prior). The harness's
  bounded pool (substrate §3.3) with statement timeouts + fast-fail-on-saturation is the floor; the
  **authn/authz hot path is the likely first dedicated read-replica** (ID-4 — Id's S5). A connection
  pooler (PgBouncer/pgcat-class) fronts each OLTP database.
- **Sharding is deferred until a hot table/tenant is *measured*** (EI-02 §8). The OLTP tier shards by
  tenant when a single tenant's shard is measured to outgrow Postgres → distributed-SQL
  (CockroachDB/Yugabyte) or tenant-split (a large tenant gets a dedicated DB/cell — ADR-11.3 isolation
  spectrum). **CI is the heaviest storage consumer** (TE-13) and is the first measured-pressure candidate
  on T2/T3.
- **The OLAP read store** absorbs analytics scans **off the bus** (CQRS) so they never touch OLTP (SC-5) —
  this is the primary "don't let analytics kill the transactional store" scaling decision, and it is
  structural, not a tuning knob.
- **Bounded everything** (X-3, ADR-16): bounded pools, statement timeouts, per-tenant in-flight caps,
  bounded prefetch on the OLAP/derived-store consumers. The tenant is the fairness/blast-radius unit (one
  tenant's CI artifact storm can't starve another's — the per-tenant fairness the 30× drill asserts).
- **The column-store/time-series seam (BUS-6) is specified-not-built** for the highest-volume *durable*
  streams (audit-grade firehose, high-cardinality event types) — promotion trigger = **measured** volume
  the OLAP/log tier serves at degraded latency (EI-04 §5.2). Until measured, the 90-day-hot log + OLAP
  long-term holder suffices (bus §4.8).
- **Self-host parity** (ADR-11): the *same* Storage artifacts run a managed cell and an on-prem install
  (Postgres + MinIO/Ceph + the KMS + the backup machinery are all self-hostable, EU-deployable). No hidden
  cloud dependency.

---

## 9. Contracts / APIs exposed & consumed (the glue — stable)

Storage is consumed as **tier clients** wired through the bootstrap harness, **not** as a generic storage
API spanning subsystems (substrate §2.8). Field names + units reconcile against the substrate X-5 anchor
(§2.10). What Storage *exposes* and *consumes*:

| Contract | Surface (illustrative) | Direction | Notes |
|---|---|---|---|
| **OLTP tier client** | the harness pool + thin query layer (tenant-scoped, RLS, encrypted columns) | exposed → every subsystem | one DB per service; no cross-DB (lint); forward-only migrations |
| **`BlobStore`** | `put/get/head/delete` content-addressed (substrate §2.7) | exposed → every blob owner | hash-on-write; per-tenant dedup; crypto-shred via key, not `delete` |
| **Log/firehose archive** | sealed-segment append + `tail(stream, range)` (consumes bus §5.5 firehose seam) | exposed → CI/Chat/Knowledge | durable archive of the firehose; pointer events on the bus |
| **OLAP read store** | CQRS read model fed by the bus (consumes substrate §5 consumer template) | exposed → Issues/analytics | reindex-from-source only; a holder |
| **KMS / envelope encryption** | `wrap/unwrap/destroy`, `key_ref`/`pii_key_ref` shape, `KeyOrigin` (BYOK/HYOK) trait (§4.2, §6.2) | exposed → every tier + the bus (`pii_key_ref`) | the cross-cutting seam; `can_derive_plaintext_index()` gates Search/Agents |
| **`PersonalDataHolder`** | `locate/export/rectify/restrict/erase` per store (consumes `myelin-gdpr`) | exposed → DSR orchestrator | auto-registered by the harness (GD-3); erase = crypto-shred (§5) |
| **Backup/restore** | `backup`, `restore(to_offset)`, `restore-verify`, `post_restore_reerase` (§7) | exposed → ops/DSR; CI gate | cross-seam consistent (STOR-4); GD-14 re-erasure |
| **`classify(field)` consumer** | reads schema-level personal-data tags (consumes `myelin-gdpr`) | consumed ← GDPR/Audit | drives per-subject-vs-per-tenant key choice (§5.1) |
| **`list_objects` (consumed)** | (consumes Id §8.2) | consumed ← Id | Storage tiers never post-filter; reads compose the authz pre-filter where they serve queries |
| **telemetry** | `storage_bytes{tenant,tier}`, `kms_unwrap_latency`, `dek_cache_hit`, `backup_rpo_seconds`, `restore_verify_pass`, `crypto_shred_lag`, `blob_integrity_fail` | exposed → Phase-5 drills (X-1) | the survival signals the drills read |

**CLI/admin surface** (overview §7.5): `myelin storage usage [--tenant --tier]`; `myelin kms key
list|rotate|shred`; `myelin backup list|restore`; `myelin storage restore-verify` (the durability gate);
`myelin storage residency verify <tenant>` (prove region pinning); `myelin tenant offboard <tenant>`
(KEK-destroy fan-out, co-owned with GDPR/Audit).

---

## 10. Failure modes + the drills owed (PROVE-IT)

Per the PROVE-IT mandate (T-1/T-2/T-5), each property that can fail names the **quantified drill** that
proves it. Phase 5 owns mechanics + thresholds; Storage owns the machinery + assertions and the telemetry
the drills read (§9). The headline owed drill is the **restore-verify + cross-seam integrity** (ADR-18,
D-6 in the substrate register).

| # | Property / failure mode | Drill (quantified gate) | Owner | Directive/ADR |
|---|---|---|---|---|
| D-S1 | **Restore + cross-seam integrity** (the headline) | Rebuild from backups to offset T; assert **no loss** (checksum parity) and OLTP rows ↔ blob ↔ search index ↔ event-log offsets restore to **one mutually consistent point** (no row → missing blob; derived == source-replay). **Gate: 0 dangling refs, 0 loss, cold==live.** | Storage + GDPR | ADR-18, STOR-4, T-5 |
| D-S2 | **RPO/RTO met** | Kill a cell; restore; assert **RPO ≤ 5 min** (WAL tail) and **RTO ≤ target**. **Gate: within budget.** | Storage | ADR-18, T-5 |
| D-S3 | **Post-restore re-erasure** (GD-14) | Erase a subject, take a *later* backup-free window, restore an *older* backup; assert the erased subject is **still erased** after restore (re-erasure pass ran). **Gate: 0 resurrected subjects.** | Storage + GDPR | GD-14, ADR-18 |
| D-S4 | **Crypto-shred reaches backups** | Erase a subject; attempt recovery from backups; assert the per-subject ciphertext is **unrecoverable** (key destroyed, excluded from backup). **Gate: 0 recoverable PII in any backup.** | Storage | ADR-12.3, EI-04 §1 |
| D-S5 | **Erasure reaches every holder** | DSR-erase a subject; assert the fan-out hit **every** `PersonalDataHolder` incl. search, OLAP, refs, bus, agent memory, **backups**, caches. **Gate: every holder returns erased; 0 misses.** | GDPR (Storage tiers participate) | ADR-12.1, GD-3, T-5 |
| D-S6 | **Tenant residency pinning** | Attempt to read/replicate a tenant's data outside its region; assert **impossible by construction** (region in partition key, no cross-region path). **Gate: 0 cross-region personal-data egress.** | Storage | ADR-11, T-5 |
| D-S7 | **KMS hiccup degrades, not cascades** | Inject a transient KMS outage; assert resolved-DEK reads survive (bounded TTL), hard-down → not-ready+shed (not fail-open). **Gate: 0 plaintext-without-key; bounded degrade.** | Storage | §4.5, X-2 |
| D-S8 | **Blob integrity** | Corrupt an object; assert re-hash-on-read detects it (content-address mismatch) and recovery from the replica/backup. **Gate: corruption detected, 0 silent serve.** | Storage | §3.2, §7.3 |
| D-S9 | **Online migration safety** | Run expand→backfill→contract on a restored production-scale copy under load; assert **no blocking lock beyond budget**, zero downtime. **Gate: lock ≤ budget.** | Storage | STOR-2, §3.1 |
| D-S10 | **HYOK opacity enforced** | Mark a content class HYOK; assert Search/Agents **skip** it (`can_derive_plaintext_index()=false`), it never appears in an index/RAG, and only non-HYOK metadata is searchable. **Gate: 0 HYOK plaintext in any derived store.** | Storage + Search + Agents | §6, T-4 |

Each drill emits a **green artifact** on pass; until then the property is **claimed, not proven** (T-4).

### 10.1 Blast-radius note (X-4)

The stateful-component register is §2 (the store map). Blast-radius summary: **T5 (KMS) is the apex** — a
tenant KEK loss is that tenant's data unrecoverable (by design for shred; mitigated by sealed-root +
live-tenant KEK backup, §4.5). **T1/T2/T3 (systems of record)** blast radius is one subsystem/tenant
(RLS + tenant prefix); recovery is restore-verify-gated (§7). **T4/T7 (derived)** have *zero* loss blast
radius — rebuilt from source. Everything else (relay workers, query layer, backup runners) is **stateless
and replaceable**. The control plane holds **zero in-region personal data** (ADR-11).

---

## 11. Cited prior art (consolidated)

- **Content-addressing / blob storage.** Git object model; Ralph Merkle, *A Digital Signature Based on a
  Conventional Encryption Function* (CRYPTO 1987 — Merkle trees); Quinlan & Dorward, *Venti: a new
  approach to archival storage* (FAST 2002 — hash-addressed write-once blocks, dedup); IPFS CID;
  BLAKE3 (O'Connor/Aumasson/Neves/Wilcox-O'Hearn, 2020).
- **Object store, self-hostable.** Weil et al., *Ceph: A Scalable, High-Performance Distributed File
  System* (OSDI 2006); MinIO; the S3 REST API as the portable contract.
- **Envelope encryption / key management.** NIST SP 800-57 (key management), SP 800-38D (AES-GCM); the
  AWS/GCP KMS DEK/KEK envelope model; HashiCorp Vault Transit + Shamir unseal (self-hostable);
  KMIP (key-management interop, for BYOK/HYOK).
- **Crypto-shredding.** NIST SP 800-88r1 (*Media Sanitization* — "cryptographic erase"); Boneh & Lipton,
  *A Revocable Backup System* (USENIX Security 1996 — destroy-the-key erasure of immutable/backup data).
- **WAL archiving / PITR / write-ahead logging.** PostgreSQL WAL archiving + PITR (docs ch. 26);
  Mohan et al., *ARIES: A Transaction Recovery Method...* (ACM TODS 1992); pgBackRest, WAL-G.
- **Restore-verification / data integrity.** Google **SRE** ch. 26 (*Data Integrity: What You Read Is
  What You Wrote*) — the "restore drills / a backup is only as good as its last successful restore"
  discipline.
- **Cross-seam / consistent snapshot.** Chandy & Lamport, *Distributed Snapshots: Determining Global
  States of Distributed Systems* (ACM TOCS 1985); Kleppmann, *DDIA* (2017) ch. 11 (log offsets as the
  consistent cursor, CDC).
- **CQRS / columnar OLAP.** Greg Young / Udi Dahan CQRS; Martin Fowler "CQRS"; ClickHouse MergeTree;
  Stonebraker et al., *C-Store: A Column-oriented DBMS* (VLDB 2005).
- **Scaling discipline.** EI-02 §8 (measure-before-shard, replicas+pooling first); the
  premature-sharding-is-an-outage prior; connection-pooler practice (PgBouncer/pgcat).
- **Doctrine.** EI-02 §8 (minimal stack, content-addressing, forward-only migrations, measure-before-
  shard), §11 (restore-verification, cross-seam integrity); EI-04 §1 (crypto-shred substrate, erasure-vs-
  immutability), §3 (object-backed git seam), §5.2/§5.3 (event-volume seam, reindex-from-source).

---

## 12. Required changes to foundational systems (if any)

The foundational Phase-3 docs (`00-platform-substrate`, `identity-and-access`, `event-bus`) already
provide the seams Storage needs; this doc consumes them. **Two small reconciliations** are required (each
a plan-layer X-5 fix, not a redesign):

1. **`pii_key_ref` shape canonicalisation (X-5).** The bus envelope (`event-bus.md §3.1`) uses
   `pii_key_ref: "kms://acme-eu/2026Q2/tenant"`; this doc pins the canonical key-reference shape as
   `kms://<tenant>/<dek-epoch>/<class>` where `<class> ∈ {tenant, subject:<id>, blob}` (§4.2) so the *same*
   reference resolves the per-subject DEK case (§5.1), not only per-tenant. **Required change:** the bus's
   `pii_key_ref` field doc should reference this shape (the field name is unchanged; the value grammar is
   reconciled here). Low-risk: additive grammar, the field already exists.

2. **`BlobStore::put` content-key wrapping (clarification, not a signature change).** The substrate trait
   (`00-platform-substrate §2.7`) is `put(bytes) -> ContentHash`. This doc clarifies that **encryption +
   per-blob content-key wrapping happen *inside* the implementation** (§4.4), so the trait surface is
   unchanged but the implementation note (address-by-plaintext-hash, store-ciphertext, per-tenant dedup) is
   added here. **No signature change required**; the substrate trait stands.

3. **Erasure ledger ownership (cross-doc).** §7.5 (post-restore re-erasure) depends on an **erasure
   ledger** (opaque subject id + timestamp + holders shredded) that survives crypto-shred. This is a
   **GDPR/Audit deliverable** (their tamper-evident audit holder, ADR-12.9 carve-out); Storage **requires**
   it to exist with that non-erasable-but-PII-free property. **Flagged as a required input** to the
   forthcoming GDPR/Audit Phase-3 doc (not a change to an existing doc — it is a dependency this doc
   creates).

No existing ADR or foundational contract is reversed.

---

## 13. Open questions for Phase 4 / Phase 5 / Legal

- **[OPEN → P4 Git]** Object-backed git pack/delta management + smart-transport over the `BlobStore`
  seam (§3.5; STOR-5/TE-24). The *relocatability constraint* is DECIDED; the implementation is the Git P4
  deliverable. Co-owned with the **GD-1 erasure-vs-immutability reconciliation** (crypto-shred reach into
  reflogs/bitmaps/backups vs commit-object bytes — §5.4).
- **[OPEN → P4 Chat]** The chat message-log engine (wide-column vs OLTP+object; §3.3) and whether chat
  bodies are per-subject-keyed in that engine (the §5.1 rule says yes; the Chat agent confirms feasibility
  in the chosen store) — TE-13.
- **[OPEN → P4 Issues+Knowledge]** Flexible-field physical storage/query model (JSONB property-bag vs
  materialised; TE-17) and which fields are per-subject vs per-tenant keyed (driven by `classify`, §5.1).
- **[OPEN → P4/LEGAL]** BYOK/HYOK per-content-class **policy** (which classes may be HYOK; the
  cross-artifact-reference-spanning-the-boundary case), the KMIP/external-key-store adapter, and the legal
  posture of HYOK as a Schrems-III mitigation (GD-7). The **mechanism + the limits** are DECIDED (§6).
- **[OPEN → P4]** Cross-cell / cross-tenant **aggregate** analytics (no-PII, control-plane) over the OLAP
  tier (§3.4) — the multi-cell-tenant analytics case (SC-2/SC-3).
- **[OPEN → P5]** All drill thresholds: the **RPO** number (proposed ≤ 5 min), **RTO** (≤ 1 h/tenant,
  ≤ 4 h/cell), restore-verify cadence, sampling rates for the cross-seam assertion (§7.2, §7.4, §10).
- **[OPEN → P5]** Sharding *trigger* metrics (the measured table/tenant size at which OLTP shards or a
  tenant gets a dedicated DB/cell — §8); the BUS-6 column-store promotion threshold.
- **[OPEN → LEGAL / DPO]** The **backup-window-vs-erasure-SLA residual** number (§7.6) and the
  erasure-ledger retention carve-out (GD-5/GD-14 ratification). Whether tenant-offboard KEK-destroy is
  sufficient for an Art. 17 *tenant-wide* assertion (the §5.1 KEK-as-shred-lever, ratified by counsel).

---

## 14. Cross-references

- **Foundational Phase-3 docs consumed:** [`00-platform-substrate.md`](./00-platform-substrate.md)
  (`BlobStore` trait §2.7, holder auto-registration §3.4, pool/outbox §3.3, migrations §9, telemetry
  §10.2, cross-seam drill D-6 §11, X-5 unit anchor §2.10);
  [`identity-and-access.md`](./identity-and-access.md) (per-tenant DEK + per-subject sub-key store map §2,
  pseudonym lever S2/§11, `list_objects` §8.2, fail-static W §10);
  [`event-bus.md`](./event-bus.md) (`contains_personal_data`/`pii_key_ref`/`data_role` §3.1, retention +
  crypto-shred + tombstones §4.8, OLAP/reindex-from-source §4.9, firehose split §4.3).
- **Spine:** ADR-10 (datastore tiers), ADR-12 (PersonalDataHolder/crypto-shred), ADR-18
  (restore-verification), ADR-11 (cells/residency), ADR-13 (glue), ADR-14 (tech map), ADR-04 (bus→OLAP).
- **Directives:** STOR-1 (content-addressed blob trait), STOR-2 (forward-only migrations), STOR-3 (cache
  never source of truth), STOR-4 (cross-seam restore consistency), GD-3 (holder auto-registration),
  X-1…X-5.
- **Doctrine:** EI-02 §8/§11; EI-04 §1/§3/§5.
- **Sibling Phase-3 docs that consume this:** **GDPR/Audit** (crypto-shred policy, DSR fan-out, erasure
  ledger §7.5), **Search** (`can_derive_plaintext_index` HYOK gate, purge+reindex erasure), **Agent
  Fabric** (HYOK opacity, agent-memory per-subject keying), **all subsystems** (tier clients, blob trait,
  forward-only migrations, backup/restore gate).
- **Seeds Phase 4/5:** §13 open questions are the Storage backlog; §10 drills are the Phase-5 scorecard
  items (D-S1 restore+cross-seam is the headline durability gate).
```
