# Phase 6 — Roadmap: Storage (tiers · KMS · crypto-shred · backup/restore-verification)

> Phase: `06-roadmaps/shared`. The detailed sequenced roadmap for the **storage** shared system. Slots into
> the master sequencing bands M0..M6: [`../00-master-sequencing.md`](../00-master-sequencing.md) (§2 bands, §3
> critical-path/DAG, §4 the gate invariant, §5 name-your-floors). Frozen architecture (this roadmap
> SEQUENCES, it does not redesign):
> [`../../05-refined-shared-systems-architecture/storage.md`](../../05-refined-shared-systems-architecture/storage.md)
> (the refined Storage architecture) + the refined
> [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> §11 (the contracts Storage owns) + §10/§12 (the contracts Storage consumes). Drills owed:
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> §4.2 (STOR-D1..STOR-D8 + the D-S11..D-S13 new assertions) + the families F2/F3/F4 + E2E-4. Doctrine:
> [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> §2 (order-by-non-negotiability — **silent data loss outranks every feature**), §3 (prove-it; a backup that
> has never been restored is not a backup), §5 (the committed ratchet) and
> [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) §1
> (crypto-shred substrate, erasure-vs-immutability), §3 (object-backed git seam), §5 (event-volume seam,
> reindex-from-source). Spine: ADR-10 (datastore tiers), ADR-11 (cells/residency), ADR-12 (PersonalDataHolder/
> crypto-shred), ADR-18 (restore-verification gate). Date: 2026-06-19.
>
> **The shape of this system, and what that means for sequencing.** Storage is **not one database** — it is
> the **tiering + the portability constraint + the cross-cutting KMS/crypto-shred mechanism + the durability
> gate** (architecture §1). That shape sets the whole sequencing thesis: Storage is the **Tier-1
> silent-data-loss floor** of the master ordering (master §1 Tier-1; EI-01 §2 #1). Its headline contract — the
> **backup/restore-verification gate (11.5, ADR-18)** — must be green *before any subsystem writes real tenant
> data*, and it is one of the **two permanent gates** (master §4) that re-run forever, never "done." Three
> consequences for the roadmap: (1) Storage's core lands **early and heavy in M1**, alongside Identity and
> Tenancy, not in a later "infrastructure" band — the substrate it underwrites is the precondition for every
> M2+ write. (2) Storage owns *mechanism*, GDPR/Audit owns *policy* (ADR-12.3): the harness wires every store
> through the KMS, registers it as a `PersonalDataHolder`, and pins it to the cell region, so "residency +
> erasability + recoverability" are **enforced by the seam, not remembered by a service** — meaning Storage's
> correctness story is mostly proven by *how the harness composes it*, drilled the moment a store exists. (3)
> Two of Storage's contracts are **named floors with scheduled follow-ons** that land in M5 (object-backed git
> packs; object-store `BlobStore`) — they are sequenced explicitly here, not left as "someday."

---

## 0. Where Storage lands in the master bands (the one-paragraph map)

Storage's **core build is M1** (the data-loss + partition floor, master band M1). But Storage is **named and
partly built in M0**: the **OLTP tier client + the outbox table live here** (11.1 — the outbox is the Tier-1
0-loss floor that ships *inside* M0/M1 because every later write depends on it, master §2 M0), the
**`fs`-backed `BlobStore` floor** (11.2) so content-addressing exists from the first commit, and the
**`forward-only-migration` + `residency-pin` lints** are part of the M0 committed ratchet (1.6). The **bulk of
Storage is M1**: the KMS hierarchy + per-subject/per-tenant DEKs (11.3/11.4 — the crypto-shred substrate), the
**backup/restore/cross-seam + restore-verify durability gate (11.5 — the silent-data-loss floor, the gate M2
does not start over)**, the reserve/settle cost gate (11.7), the OLAP read store frame (11.6), and the
structural GDPR floor (per-subject DEK shred, crypto-shred-reaches-backups). Storage's **C-delta sharpenings
light up with their consumers across M2/M3/M4**: the trust-scoped cache namespaces + the `(job,step,byte-range)`
log tier + per-subject CI-log DEK (C1/C2/C4) land **with CI in M4**; the within-EU CDN clone/bundle class (C3)
+ the object-backing seam land **with Git in M3 (floor) → M5 (full)**; the OLAP restriction-flag gate (C5)
lands **with Issues in M4**; the outbound-mirror residency gate (C6) lands **with Git in M3/M4**. Storage's
**world-scale hardening + the floor follow-ons are M5** (object-backed packs, object-store `BlobStore`,
multi-cell migration, restore-verify at cell scale, the F6 surge family on the storage lanes). Storage is the
spine of the **M5 E2E-4 DSAR fan-out** (crypto-shred reaches every holder incl. backups + post-restore
re-erasure) and the storage half of **E2E-3 reindex-parity**. It participates in **M6 dogfood** (the
restore-verify CI job runs on the platform's own commits before any real team data lands).

The honest progression: **first runnable** = M0 (OLTP pool + outbox + `fs`-`BlobStore` on one tenant —
something boots and persists); **first useful** = M1 (real per-tenant envelope encryption, crypto-shred,
**and a green restore-verify** — the point real data may be written on top); **production-hardened** = M5 (RPO/
RTO held at cell scale under 30× surge, object-backed packs + object-store swap done, multi-cell migration
drilled, the full DSAR fan-out + post-restore re-erasure green across all H1–H18 holders).

---

## 1. The contracts Storage owns / consumes, mapped to the milestone they land in

From contract-index §11 (owned), §10/§12/§4 (consumed). "Lands" = the milestone by which the contract must be
implemented or callable for Storage's gate (and its consumers' gates) to be green.

### 1.1 Owned by Storage (contract-index §11)

| # | Contract | Lands | Notes / floor |
|---|---|---|---|
| 11.1 | **OLTP tier client** — harness pool + tenant-scoped RLS + encrypted columns; one DB per service; the outbox lives here | **M0** (pool + outbox table + RLS) → **M1** (envelope-encrypted columns under the KMS) | the outbox is the Tier-1 0-loss floor; it ships in M0 so SUB-D1/D2/BUS-D4 can be green to exit M0. Per-subject column encryption is wired once the KMS exists (M1). |
| 11.2 | **`BlobStore{put,get,head,delete}`** — content-addressed (BLAKE3, per-tenant dedup); fs↔object one-line swap; **+ object-backed pack/delta seam** (C/Git); **+ CDN clone/bundle class [C3]**; **+ trust-scoped cache namespaces [C4]** | **M0** (`fs`-floor + trait) → **M3** (Git local-disk packs behind the trait + C3 CDN class) → **M4** (C4 cache namespaces with CI) → **M5** (object-store swap + object-backed packs) | the trait is M0 so nothing is node-pinned by construction (STOR-5). The **object-store `BlobStore`** is the named M5 follow-on of the `fs` floor. |
| 11.3 | **KMS hierarchy + `KeyOrigin` trait** — per-cell root → per-tenant KEK → per-tenant/per-subject DEK; `wrap/unwrap/can_derive_plaintext_index/destroy` | **M1** | the crypto-shred substrate; `can_derive_plaintext_index()=false` structurally skips Search/Agent (HYOK). Must exist before any subsystem writes encrypted data (M2+). |
| 11.4 | **Crypto-shred + GD-4 granularity** — free-text/profile/body/agent-memory/op-log = per-subject DEK (**incl. CI inline-PII log segments [C1]**); bulk = per-tenant DEK; offboard = the KEK | **M1** (structural floor + per-subject/per-tenant rule) → **M4** (C1 per-subject CI-log DEK with CI) | the `erasure` tag drives the key choice (consumed from `classify`, 10.2). The C1 CI-log per-subject extension lands with CI's log tier. |
| 11.5 | **Backup / restore / cross-seam** — WAL+PITR (RPO ≤ 5 min); `restore(to_offset)`; **`restore-verify` (CI-gated, ADR-18)**; event-log offset = the cross-seam cursor (OLTP↔blob↔index↔offset); `post_restore_reerase`. Derived stores rebuilt, not restored | **M1** (the headline durability gate; CI-wired) → **M5** (re-confirmed at cell scale under surge) | **the silent-data-loss floor.** M2 does not start over a red STOR-D1. A **permanent gate** (master §4): re-runs on every change touching a store, forever. |
| 11.6 | **OLAP read store** — CQRS read model fed by the bus; reindex-from-source only; a holder; **+ honours the restriction flag [C5]**; worklog analytics-eligibility per OQ-H | **M1** (frame + holder registration) → **M2** (fed by the bus consumer template) → **M4** (C5 restriction-flag gate live with Issues analytics) | derived store: zero loss blast radius, rebuilt from source. C5 unblocks Issues CFD/cycle-time/velocity without leaking a restricted subject. |
| 11.7 | **Reserve/settle cost gate** — reserve at dispatch, settle on completion, never interrupt in-flight; integer minor-units; wholesale ≠ markup. Fronts every agent run + every CI run | **M1** (the gate mechanism + ledger) → **M2** (fronts agent runs) → **M4** (fronts CI runs) | co-owned (Agent gate + Commercial wallet). Storage holds the durable ledger; the gate is exercised the moment agent runs (M2) and CI runs (M4) exist. |
| 11.8 | **T3 log tier (CI)** — sealing firehose frames into T2 content-addressed segments + an OLTP **`(job, step, byte-range)` index**; per-tenant DEK (per-subject for inline PII, C1); the jump-to-failure `details_ref` (5.9 / `#step-<n>`) resolves through it | **M4** (with CI) | CI is the heaviest log producer; the index shape is frozen now (C2), built when CI lands. Rides the M2 firehose resume-cursor transport (3.5). |

### 1.2 Consumed by Storage — the upstream dependencies that must exist first

| # | Consumed contract | From | Must be green by | Why Storage blocks on it |
|---|---|---|---|---|
| 2.2/2.3 | `OutboxTx::emit` + the `outbox` table | **Bus (M0)** | **M0** | the outbox *physically lives in* the OLTP tier (11.1); the cross-seam consistency cursor (§7.3) IS the outbox `seq` / event-log offset. Storage cannot define the restore-to-consistent-point without it. |
| 2.4/2.6 | the consumer template + `reindex(scope)` re-emit + `*.snapshot` | **Bus + every subsystem (M0 seam; M2/M3/M4 owners)** | **M2** seam; per-owner replay M3/M4 | the OLAP/derived rebuild path (reindex-from-source — *never restore derived stores from their own backups*, §7.3). The restore-verify cross-seam assertion depends on this being the only rebuild path. |
| 10.1 | `PersonalDataHolder{locate/export/rectify/restrict/erase}` | **GDPR (M1)** | **M1** | every Storage tier is a holder, harness-auto-registered (1.4); the exhaustive H1–H18 list must be closed before real data (the DSR fan-out depends on it). |
| 10.2 | `#[personal_data(... erasure, ...)]` classify-derive | **GDPR (M1)** | **M1** | `classify(field)` drives the per-subject-vs-per-tenant **key choice automatically** (§5.1). Without it, the GD-4 granularity rule has no input. |
| 10.5 | `transfer_allowed` (the outbound-mirror gate) | **GDPR + control plane (M3/M4)** | **M3** | the C6 push-mirror residency gate lives *here*, not in Storage; Storage flags the crossing + reports the mirror target region into `residency_verify`. |
| 10.8 | the erasure ledger (PII-free, non-shred-erasable) | **GDPR (M1)** | **M1** | drives §7.5 **post-restore re-erasure** (GD-14). The restore-verify gate is incomplete without it (a restore of an old backup must re-apply every erasure completed since). |
| 12.2/12.4 | `discover`/`placement_of` + `residency_verify` | **control plane (M1)** | **M1** | region-pinning + the no-global-pool attestation (incl. the C3 CDN edge set + the C6 mirror target). The `residency-pin` lint (1.6) is the compile-time half. |
| 1.1/1.4/1.8 | `serve(AppSpec)` + holder auto-registration + telemetry signal set | **substrate (M0)** | **M0** | the harness IS what wires each store through KMS + holder-registration + region-pin; every Storage drill asserts against the 1.8 survival signals. |
| 1.6 | `forward-only-migration` + `residency-pin` lints | **substrate/CI (M0)** | **M0** | compile-time floors; ship in the M0 ratchet (online-migration safety + no out-of-region write). |
| 4.3 | `list_objects(...) → Filter{set_expr}` | **Id (M1)** | **M1** | Storage tiers **never post-filter**; where a tier serves queries it composes the authz pre-filter. Storage consumes, does not own, the leak-free filter. |

**The critical upstream dependency, stated plainly:** Storage's restore-verify gate (11.5) — the silent-data-
loss floor on the master critical path's M1 node — is downstream of **the outbox `seq` / event-log offset
(2.2/2.3, M0)** being the cross-seam linearisation cursor, and of **the erasure ledger (10.8, M1)** for
post-restore re-erasure. The first makes "restore OLTP↔blob↔index↔offset to *one* mutually consistent point"
definable; the second makes "an erased subject stays erased after restoring an old backup" enforceable. Both
must be green in M0/M1 before STOR-D1/D3 can be claimed. The second hard dependency is **`classify(field)`
(10.2, M1)** — without the personal-data tags, the GD-4 per-subject-vs-per-tenant key choice (the crypto-shred
granularity that the whole erasure story rests on) has no driver.

---

## 2. The sequenced milestones (Storage's slice of each band)

Each milestone states **the work**, the **floor-then-full progression** (each floor named with its scheduled
follow-on), the **upstream dependencies** (what must be green first), and the **quantified gates/drills** that
call it done. Drill thresholds carry the Q32 defaults-to-beat; Phase 6 measures + sets the final numbers
(EI-02 §8; testing-strategy §4.1, §5).

---

### S-M0 — The storage floor under the substrate (inside master band M0)

**Master band:** M0 (substrate, harness, the committed gates).

**Thesis.** Storage ships the pieces M0's own exit gate depends on: the **OLTP tier client + the outbox table**
(so SUB-D1/D2/BUS-D4 — the 0-loss/0-ghost outbox floor — can be green to exit M0), the **`fs`-backed
`BlobStore` + the content-addressed trait** (so nothing is ever node-pinned, STOR-5), and the **storage
lints**. Nothing here is a feature; everything is a precondition.

**Work:**
- **OLTP tier client (11.1, partial):** the harness-wired connection pool, the `(tenant, region)`-first RLS
  tenant-scoping guard (the IDOR floor; the `tenant-predicate` lint target), bounded pools + statement
  timeouts. **The `outbox` table physically lives here** — `(event_id UNIQUE, aggregate, seq, subject,
  envelope)`, `UNIQUE(aggregate, seq)` — co-located in the same OLTP database so an outbox row commits in the
  same transaction as the state change (this co-location is what makes the §7.3 cross-seam cursor *exist*).
- **`BlobStore` trait + `fs`-backed floor (11.2, floor):** the narrow content-addressed `put/get/head/delete`
  trait, BLAKE3-on-write with a self-describing multihash prefix, address-by-plaintext-hash-within-a-tenant /
  store-ciphertext. `fs`-backed so it runs from the first commit; **the object-store backing is the named M5
  follow-on** (one-line swap by the trait's design).
- **The storage lints (part of the M0 ratchet, 1.6):** `forward-only-migration` (the migration runner only
  admits expand→backfill→contract; hot-table flags declared — KN `block`/`db_row`/`doc_op` + all high-write),
  `residency-pin` (no out-of-region write compiles), each with a red-fixture + a green-fixture.
- **Forward-only online migration runner** (the expand→backfill→contract machinery; lock-time is measured
  against a *restored* copy once restore-verify exists in M1).

**Floor-then-full (named):**
- **`fs`-backed `BlobStore`** is the floor; **object-store (`MinIO`/`Ceph` RADOS) `BlobStore`** is the M5
  follow-on (trigger: the object-backed git pack work + measured single-node blob ceiling). One-line swap.
- **Per-tenant envelope encryption is NOT yet wired** in S-M0 (the KMS lands M1) — columns are plaintext-at-
  rest on the floor. **Named gap:** real at-rest encryption is the first thing M1 adds; no real tenant data is
  written before then (the M1 gate enforces this).

**Upstream dependencies:** the outbox contract shape (2.2/2.3) + the `EventEnvelope` field list (2.1) frozen
(Bus, M0); `serve(AppSpec)` + holder-registration hook + telemetry (1.1/1.4/1.8, substrate M0).

**Gates / drills to call S-M0 done (contribute to the master M0→M1 boundary):**
- The outbox-bearing OLTP tier makes **SUB-D1** (kill between commit & publish → exactly-once-in-effect, 0
  ghost/0 lost) and **BUS-D4** (crash producer between state-commit and publish → emit-iff-committed) green —
  these are master M0 exit gates; Storage owns the OLTP/outbox half of them.
- **`forward-only-migration` + `residency-pin` lints green** with both fixtures (part of "all twelve lints
  green," master M0 exit).
- **STOR-D7 (blob integrity floor):** corrupt an `fs`-`BlobStore` object → re-hash-on-read detects the
  content-address mismatch; 0 silent serve. *(CI — runnable the moment the trait exists.)*

---

### S-M1 — The data-loss floor + the partition/residency floor + the crypto-shred substrate (master band M1)

**Master band:** M1 (Identity + storage durability + tenancy — Tiers 1, 4, 5 of the thesis). **This is
Storage's keystone band.**

**Thesis.** Prove the **silent-data-loss floor (restore-verify)**, stand up the **KMS hierarchy + crypto-shred
substrate**, and pin every byte to a region — **before any subsystem writes a row** (master §2 M1; EI-01 §2
#1: silent data loss outranks every feature). When this band exits green, "residency + erasability +
recoverability" are properties the seam enforces, and real data may be written on top.

**Work:**
- **KMS key hierarchy (11.3):** the three-level envelope-encryption hierarchy — L0 per-cell root (HSM/sealed,
  never exported), L1 per-`(tenant, region)` KEK (destroying it = tenant-granularity crypto-shred), L2 DEKs
  (AES-256-GCM; per-tenant for bulk, per-subject for the individual-erasure classes). The `pii_key_ref =
  kms://<tenant>/<dek-epoch>/<class>` travels with every ciphertext. Key rotation = envelope re-wrap (O(keys),
  not O(data)). KMS availability degrades fail-static; hard-down → not-ready, **never fail-open**.
- **`KeyOrigin` trait (11.3):** platform-managed | BYOK | HYOK behind one trait;
  `can_derive_plaintext_index()=false` is the **structural** HYOK enforcement (Search/Agent must consult it
  before indexing). The mechanism + the limits are built; the per-content-class HYOK *policy* + the KMIP
  adapter are `[OPEN → P6/LEGAL]` named follow-ons.
- **Crypto-shred + GD-4 granularity (11.4, structural floor):** wire `classify(field)` → key choice (a field
  tagged `personal-data, erasure=subject` is auto-wired to a per-subject DEK by the harness); the per-subject
  DEK classes (free-text/profile/chat-body/agent-memory) and the per-tenant DEK bulk classes. The
  `erase(subject, tenant)` algorithm (§5.2): pseudonym-map shred → `KMS.destroy(per_subject_DEK)` →
  Search purge+reindex → Refs tombstone → Bus erase → erasure receipt. **C1 (per-subject CI-log DEK) is named
  but not yet built** — it lands with CI in M4.
- **OLTP envelope encryption wired (11.1, completing S-M0):** personal-data columns under the tenant DEK,
  free-text/profile columns under a per-subject sub-key — the KMS now exists, so the at-rest encryption gap
  named in S-M0 is closed.
- **Backup / restore / cross-seam + restore-verify (11.5) — THE HEADLINE:** continuous WAL archiving + periodic
  base backups → PITR (RPO target ≤ 5 min); object tier versioned + in-region replicated (content-addressing
  makes integrity re-hash-verifiable); the **cross-seam consistency point** = the per-aggregate outbox `seq` /
  event-log offset (restore OLTP to the WAL position whose outbox rows have `seq ≤ T`, verify every referenced
  `ContentHash` is present, **reindex derived stores from source** to offset T — never from their own backups);
  the **CI-wired restore-verify gate** (spin a clean target, restore T1/T2/T5, reindex T4/Search/Refs from
  source, assert no-loss + cross-seam + erasure-held, emit a green artifact or fail CI); **post-restore
  re-erasure (§7.5)** against the erasure ledger (re-apply every erasure completed after the backup's PIT,
  assert 0 resurrected subjects).
- **OLAP read store frame (11.6, partial):** the holder registration + the CQRS-fed-by-the-bus contract shape;
  the **C5 restriction-flag gate is named** but lights up with Issues analytics in M4. Worklog analytics-
  eligibility tagged `[OPEN — LEGAL]` (OQ-H).
- **Reserve/settle cost gate (11.7):** the durable per-tenant ledger + the reserve-at-dispatch / settle-on-
  completion / never-interrupt-in-flight mechanism (integer minor-units). Exercised by agent runs in M2.
- **The structural GDPR floor (with GDPR/Storage):** per-subject DEK crypto-shred + pseudonym-map shred reach +
  crypto-shred-reaches-backups-by-construction — the structural half (X-7); the residual posture is the ONE
  platform artifact (10.9) handled by reference, `[OPEN — LEGAL]`.

**Floor-then-full (named):**
- **`fs`-`BlobStore` (carried from S-M0)** → object-store `BlobStore` (M5).
- **Single-cell / one home cell per tenant** is the floor; **multi-cell** (cross-cell PII-free pointer bridge
  live; the FLOOR drills GA-D8/CP-D7/CP-D8 owed) is the M5 follow-on (trigger: cross-cell rollup/collab/cross-
  org demand, OQ-I). Storage builds the per-cell KEK + per-cell backup machinery now; the cell→cell migration
  (CP-D7: 0 loss, source crypto-shredded) is M5.
- **RPO ≤ 5 min / RTO ≤ 1h-tenant / ≤ 4h-cell** are the proposed defaults-to-beat; the **measured** numbers are
  set by STOR-D2 in this band and re-confirmed at cell scale in M5.
- **Per-subject DEK covers free-text/profile/chat-body/agent-memory now;** the **CI inline-PII log segment**
  extension (C1) is the named M4 follow-on.

**Upstream dependencies:** `classify(field)` + `PersonalDataHolder` + the erasure ledger (10.1/10.2/10.8, GDPR
M1); `placement_of`/`residency_verify` + the `(tenant, region)` partition key (12.1/12.2/12.4, control plane
M1); the outbox `seq`/offset as the cross-seam cursor (2.2/2.3, Bus M0); reindex-from-source `*.snapshot`
re-emit (2.6, Bus M0 seam); `list_objects` for any query-serving tier (4.3, Id M1).

**Gates / drills to call S-M1 done — the master M1→M2 boundary (M2 does not start over a red STOR-D1):**
- **STOR-D1 (the headline):** rebuild from backups to offset T → **0 loss** (checksum parity); OLTP↔blob↔index↔
  offset at **one mutually consistent point**; 0 dangling refs; cold == live. *(SCHED.)* **This is the silent-
  data-loss floor.**
- **STOR-D2:** kill a cell, restore → **RPO ≤ 5 min** (WAL tail), **RTO ≤ 1h/tenant, ≤ 4h/cell**. *(SCHED.)*
- **STOR-D3 (with GA):** erase a subject, restore an *older* backup → still erased (post-restore re-erasure
  ran); **0 resurrected.** *(SCHED.)*
- **STOR-D4:** erase a subject, attempt recovery from backups → per-subject ciphertext **unrecoverable** (key
  destroyed + excluded from backup); **0 recoverable PII in any backup.** *(SCHED — the GA-D5 sibling.)*
- **STOR-D5:** read/replicate a tenant's data outside its region → impossible by construction (`residency-pin`
  rejects out-of-region writes); **0 cross-region PII egress.** *(SCHED — the CP-D3 storage face.)*
- **STOR-D6:** transient KMS outage → resolved-DEK reads survive (bounded TTL); hard-down → not-ready+shed,
  **never fail-open**; 0 plaintext-without-key. *(CI.)*
- **STOR-D8:** expand→backfill→contract on a restored prod-scale copy under load → no blocking lock beyond
  budget; 0 downtime. *(SCHED — uses the restored copy STOR-D1 produces.)*
- These feed the master M1 exit alongside ID-D1/D2/D3 + CP-D2/D3. **STOR-D1/STOR-D2 become a permanent gate**
  (master §4): the restore-verify CI job re-runs on every change touching a store, forever.

---

### S-M2 — Wiring Storage into the reactive layer (master band M2)

**Master band:** M2 (the reactive shared layer + the safety drills).

**Thesis.** Storage builds little *new* in M2; instead its M1 mechanisms get their **first real consumers** —
the OLAP store is fed by the bus consumer template, the reserve/settle gate fronts the first agent runs, and
the firehose resume-cursor transport (3.5, built by Bus/KN in M2) becomes the live source the **T3 log archive
will seal from** (the archive itself lands with CI in M4). The work is integration + proving the derived-store
rebuild path on a real stream.

**Work:**
- **OLAP read store fed by the bus (11.6):** the idempotent consumer (dedup on `event_id`) populating the
  ClickHouse-class CQRS read model off the durable stream — *never* by scanning OLTP. Reindex-from-source is
  wired as the only rebuild path (no "read OLTP into ClickHouse" backdoor).
- **Reserve/settle fronts agent runs (11.7):** the gate now sits in front of every `AgentRuntime` run + every
  `SCHEDULE_AND_RUN_JOB` (the agent fabric lands in M2). Reserve-at-dispatch → no balance → no run.
- **The T3 firehose-archive seam (prep for 11.8):** the durable archive of the firehose (sealed segments → T2
  content-addressed blobs) is specified against the M2 resume-cursor transport (3.5); the CI-specific
  `(job,step,byte-range)` index (C2) is built with CI in M4, but the sealing mechanism + the per-tenant DEK
  segment encryption are validated here on a non-CI firehose (e.g. a synthetic op-stream).

**Floor-then-full (named):** no new Storage floor in M2; the carried floors (`fs`-`BlobStore`, single-cell)
remain named with their M5 follow-ons.

**Upstream dependencies:** the bus consumer template + `*.snapshot` replay (2.4/2.6, M2 owners coming online);
the firehose resume-cursor transport (3.5, Bus/KN M2); the reserve/settle wallet (Commercial); the agent
fabric (8.x, M2) as the first reserve/settle consumer.

**Gates / drills to call S-M2 done (Storage's contribution to the master M2→M3 boundary):**
- **The OLAP derived-store rebuild is reindex-from-source-only** — proven via the F4 family on the OLAP holder
  (a `reindex(scope)` rebuilds the read model byte-matching live; no bespoke recovery reader). *(SCHED, the
  storage face of BUS-D5/the F4 family.)*
- **Reserve/settle never interrupts in-flight** — exercised by the agent-run drills (AG-D6/AG-D11 reserve
  refusals; FLOW-D6); Storage owns the ledger correctness (one cost event per metered unit). *(CI.)*
- The master M2 exit is gated by AG-D4 (the sandbox-escape GATE) + the reactive-layer leak/loss drills; Storage
  has no new headline gate here but **STOR-D1/STOR-D2 must remain green** (the permanent gate; any M2 store
  change re-runs them).

---

### S-M3 — The Git storage floor: local-disk packs, CDN clone class, the object-backing seam (master band M3)

**Master band:** M3 (the producer subsystems — Git + Knowledge).

**Thesis.** Git is **the single heaviest subsystem to scale** (EI-04 §3) and the heaviest storage consumer.
Storage ships Git's **local-disk pack floor behind the `BlobStore` trait** (so the v1 git data model is *never*
node-pinned — the object-backed transition is a backing swap, not a rewrite), the **within-EU CDN clone/bundle
class (C3)** for clone-storm acceleration, and the **outbound-mirror residency gate seam (C6)**. The
object-backed pack *implementation* is explicitly the M5 follow-on.

**Work:**
- **Local-disk git pack/object storage behind the `BlobStore` trait (11.2, STOR-5 floor):** git packs + loose
  objects addressed through the trait; relocatable placement (repo-granular `placement_of`, region-pinned, not
  node-pinned — 12.2). **Floor named:** packs on local disk behind the trait; **follow-on:** object-backed
  pack/delta management + smart-transport = the M5 deliverable.
- **Within-EU CDN clone/bundle blob class (C3, NEW):** a content-addressed blob class over `BlobStore` for hot-
  repo/clone-storm acceleration; **residency-respecting** (the CDN edge set is within-EU-only for an EU tenant;
  the content-address *is* the cache-validity check — no staleness model). The control plane's `residency_verify`
  covers the CDN edge set.
- **Outbound push-mirror residency gate seam (C6):** Storage flags the crossing (keeps mirror-source blobs
  content-addressed + encrypted) and reports the mirror target region into `residency_verify`; the actual
  allow/deny is the control-plane/`transfer_allowed` gate (10.5). Lands as the Git push-mirror feature lands.
- **Crypto-shred reach into git's structures:** per-tenant blob DEK shreds reflogs/bitmaps/pack-tier backups;
  the commit-object-byte residual is the 10.9 posture (pseudonymous-by-default commits, X-7 — decided *before*
  Git's data model froze). The audited history-rewrite erasure path (10.6) is the named on-demand follow-on.

**Floor-then-full (named):**
- **Local-disk git packs (behind the trait)** → **object-backed packs + smart-transport** (M5; trigger: the
  single-node ceiling *measured*, GIT-D4). The relocatability constraint is decided now; the impl is M5.
- **Pseudonymous-by-default commits** (immutable bytes never bake erasable PII) is the floor; the **audited
  history-rewrite erasure path** (10.6, changed-hash consequence) is the on-demand M5 follow-on.

**Upstream dependencies:** the `BlobStore` trait (11.2, M0); repo-granular `placement_of` + `residency_verify`
(12.2/12.4, control plane); `transfer_allowed` (10.5, GDPR/CP) for the C6 gate; the pseudonym grammar +
`erase` (4.8, Id) for git's crypto-shred reach; Git subsystem (M3) as the consumer.

**Gates / drills to call S-M3 done (Storage's contribution to the master M3→M4 boundary):**
- **D-S6 / STOR-D5 (residency, extended):** the C3 CDN edge set is within-EU + the C6 mirror target is reported
  into `residency_verify` → **0 cross-region PII egress** via the CDN or a mirror. *(SCHED.)*
- **GIT-D2 (storage half):** erase a commit author → crypto-shred reaches backups/reflogs/bitmaps; residual ==
  the ONE platform posture (pseudonymous-by-default). *(SCHED.)*
- **STOR-D7 on git packs:** corrupt a pack object → content-address re-hash detects + recovers from replica/
  backup; 0 silent serve. *(CI.)*
- **GIT-D9** (push outbox emit-iff-committed) relies on the OLTP/outbox tier; **STOR-D1/STOR-D2 remain green**
  on the now-git-bearing stores (the permanent gate re-runs).

---

### S-M4 — The CI storage tier + the OLAP restriction gate (master band M4)

**Master band:** M4 (the consumer subsystems — CI + Issues + Chat).

**Thesis.** CI is **the heaviest storage consumer** (T2/T3) and the first measured-pressure candidate (§8).
Storage ships the **CI log tier (the `(job,step,byte-range)` index, C2)**, the **per-subject CI-log DEK (C1)**,
and the **trust-scoped cache namespaces (C4)** — the storage half of the X-1 poisoned-pipeline defence. With
Issues, Storage lights up the **OLAP restriction-flag gate (C5)**.

**Work:**
- **T3 CI log tier (11.8, C2):** seal firehose frames into T2 content-addressed segments + the OLTP
  **`(job, step, byte-range)` index** mapping to `(segment-blob, offset)` — the resolver behind the X-1
  `CheckStatus.details_ref` `#step-<n>` jump-to-failure sub-anchor (OQ-D). Rides the M2 resume-cursor transport.
- **Per-subject CI-log DEK (11.4, C1):** where a CI log segment's inline PII is isolable per subject, encrypt
  it under that subject's DEK so erasure crypto-shreds exactly their log content (live AND backups); where not
  isolable, fall back to the per-tenant DEK + the 10.9 residual posture.
- **Trust-scoped cache namespaces (11.2, C4):** the scope-key convention `<tenant>/ci/cache/<scope>/...`,
  `<scope> ∈ {trusted, fork:<pr_id>, branch:<protected_branch>}`; an `untrusted_fork` run may **read** the
  trusted scope but may only **write** its own `fork:` scope (a write to `trusted` is refused by the blob
  client). The structural half of the poisoned-cache defence.
- **OLAP restriction-flag gate (11.6, C5):** `restrict(subject)` propagates into T4 — a restricted subject's
  rows are excluded from analytics aggregates (CFD/cycle-time/velocity/delivery-health). A **compliance gate**,
  not a tuning knob; unblocks the Issues analytics ask without leaking a restricted subject.

**Floor-then-full (named):** the C1 per-tenant-fallback for non-isolable interleaved CI-log PII is the
documented residual (per 10.9), not a floor-with-follow-on; the GIN-indexed JSONB facet scan (Issues/KN custom
fields) is the floor whose **generated projection-feeder index** follow-on is M5 (promoted per measured facet
> 5% of view executions, OQ-C).

**Upstream dependencies:** the firehose resume-cursor transport (3.5, M2) the log archive seals from; the X-1
`CheckStatus` seam (5.9) for the `details_ref` resolution; CI's `trust_tier` stamp (CI stamps trust off run
provenance; Storage enforces the write-scope rule); `restrict(subject)` (10.1, GDPR); Issues analytics (M4) as
the C5 consumer; CI (M4) as the C1/C2/C4 consumer.

**Gates / drills to call S-M4 done (Storage's contribution to the master M4→M5 boundary):**
- **D-S11 (trust-scoped cache isolation, C4):** an `untrusted_fork` run writes a cache entry → lands only in
  `fork:<pr_id>`; a trusted run never reads it as `trusted`-scoped. **Gate: 0 cross-scope cache writes;
  `cache_scope_violation` = 0.** *(CI — the CI-D6 storage face.)*
- **D-S12 (restricted-subject OLAP suppression, C5):** `restrict(subject)`; run CFD/cycle-time/velocity → the
  subject's contribution is absent. **Gate: `olap_restricted_subject_leak` = 0.** *(CI.)*
- **STOR-D4 + C1 assertion:** erase a subject incl. **CI log segments** → unrecoverable in backups; 0
  recoverable PII. *(SCHED — the CI-D3 storage face.)*
- **The `details_ref` `#step-<n>` resolves to the exact failing step's bytes** via the `(job,step,byte-range)`
  index (the storage realisation of GIT-D10/CI-D8's jump-to-failure). *(CI.)*
- **STOR-D1/STOR-D2 remain green** on the now-CI-log-bearing stores (T2/T3 are the heaviest; the permanent gate
  re-runs at this newly-heavy scale).

---

### S-M5 — World-scale hardening + the floor follow-ons (master band M5)

**Master band:** M5 (world-scale hardening + the floor follow-ons + the cross-subsystem E2E wedge).

**Thesis.** With all five subsystems on one substrate and the deterministic correctness drills green, prove
Storage **as a whole** under world-scale load, and **ship the named floor follow-ons** — object-store
`BlobStore`, object-backed git packs, multi-cell migration. This is where the floors named in M0/M1/M3 get
their scheduled full answers.

**Work — the floor follow-ons (each named in its band; here is its scheduled follow-on):**
- **Object-store `BlobStore` (11.2):** swap the `fs`-backed floor for `MinIO`/`Ceph` RADOS behind the same
  trait (a one-line backing swap by design). Trigger: measured single-node blob ceiling.
- **Object-backed git packs (11.2 / STOR-5, EI-04 §3):** authoritative git bytes move from node-local disk to
  the object store — delta/pack management, sharding, replication, smart-transport, the within-EU CDN
  clone/bundle class fully wired. The explicit sequenced transition EI-04 §3 insisted on; early choices did
  not pin repos to a node (12.2 relocatable placement). Trigger: GIT-D4 (the single-node ceiling *measured*).
- **Multi-cell (12.6, OQ-I):** the cross-cell PII-free pointer bridge goes live; **cell→cell migration** (same
  region) with 0 loss + source crypto-shredded; DSR fan-out iterates `member_cells` (10.4). The FLOOR drills
  GA-D8/CP-D7/CP-D8 are now owed and run. Trigger: cross-cell rollup/collab/cross-org demand.
- **Event-volume column-store seam (EI-04 §5, BUS-6):** promoted **only once event volume is measured** to
  outgrow the general-purpose DB — not before.
- **The full DSAR / crypto-shred fan-out across all H1–H18 holders:** every holder now exists (incl. CI logs,
  agent memory, chat bodies), so the crypto-shred reach is complete; post-restore re-erasure covers every
  holder.

**Work — world-scale hardening (the F6 surge family + the scheduled scale drills):**
- The **30× surge on the storage lanes** (CI artifact storm one tenant doesn't starve another — reserve/settle
  per-tenant fairness + C4 cache namespaces; the cell bulkhead).
- **Restore-verify at cell scale** under world-scale load (STOR-D2 re-confirmed: RPO/RTO held under surge).
- **Online-migration-under-load** at prod scale (STOR-D8 at cell scale).

**Floor-then-full (named):** by M5 the floors are promoted; what remains designed-not-built is named in the
honesty register (§4): HYOK per-content-class policy + KMIP adapter (`[OPEN → P6/LEGAL]`), the column-store
seam (measured-trigger), the generated projection-feeder index (measured-trigger).

**Upstream dependencies:** all five subsystems on the substrate (M4 green); the cross-cell pointer bridge frame
(12.6, M1 frame → M5 live); measured ceilings (GIT-D4, blob ceiling, event volume) to trigger each follow-on;
the complete H1–H18 holder set (M4) for the full DSAR fan-out.

**Gates / drills to call S-M5 done (Storage's contribution to the master M5→M6 boundary):**
- **STOR-D2 at cell scale re-confirmed** (RPO/RTO under world-scale load). *(SCHED.)*
- **The F6 surge family on the storage lanes** (human lane holds, agent/CI sheds, cross-tenant impact 0) —
  SUB-D3 / GIT-D6 (clone surge + CDN hit) / CI-D2 (CI surge). *(SCHED.)*
- **CP-D7 (FLOOR):** cell→cell migration (same region) → 0 loss across-seam, in-region, source crypto-shredded.
  *(SCHED.)*
- **GA-D8 (FLOOR):** multi-cell erasure fan-out iterates all `member_cells ∪ home_cell`; complete receipt set;
  0 cells missed. *(SCHED.)*
- **E2E-4 (DSAR fan-out)** — Storage is the spine: crypto-shred reaches every holder incl. vectors incl.
  backups (STOR-D4 across all holders), post-restore re-erasure (STOR-D3), residual == the one documented
  posture. **Gate: 0 holders missed; 0 recoverable PII; certificate sealed.** *(SCHED.)*
- **E2E-3 (storage half):** cold-reindex == live for the derived stores (OLAP/Search/Refs rebuilt from source).
  *(SCHED.)*
- **D-S13 (outbound-mirror residency deny, C6):** an extra-EU mirror target for an EU tenant's PII-bearing repo
  → deny-by-default + `residency_verify` reflects no extra-EU PII path. *(SCHED — gate at 10.5.)*

---

### S-M6 — Dogfood: the restore-verify gate runs on Myelin's own commits (master band M6)

**Master band:** M6 (dogfooding — Myelin hosts itself).

**Thesis.** The team's own data is **real tenant data** — so M6 does not begin until Storage's restore-verify
and DSAR fan-out are green (master M6 entry: "you do not dogfood real team data onto a substrate whose
restore-verify and DSAR fan-out are not green," Tier 1 + Tier 6).

**Work:**
- The **restore-verify CI job runs on the platform's own commits** (the dogfood loop: Myelin's own monorepo,
  CI logs, issues, docs are now real tenant data under the same backup/restore/crypto-shred machinery).
- The every-incident-adds-a-drill loop files a Myelin issue + a reproducing storage drill for any storage
  incident discovered during dogfooding.

**Upstream dependencies:** M5 green (restore-verify at cell scale + the full DSAR fan-out + multi-cell drilled);
Myelin git hosting + CI live (M3/M4 dogfooded).

**Gates / drills to call S-M6 done (Storage's contribution to the master done-bar):**
- **The restore-verify gate green on the platform's own stores** (STOR-D1/STOR-D2 on real Myelin data).
- **No earlier-band storage gate is red** (the truth-up pass confirms every PROVEN Storage row rests on a dated
  green artifact — the gate invariant holds end-to-end; code-wins-over-docs).

---

## 3. The world-scale / hard-problem work, scheduled explicitly

Per VISION §3 + EI-04 §4: each hard problem ships as a **named floor before its full answer**, with the
follow-on scheduled — never left as "someday." Storage's hard-problem ledger (consolidated from master §5):

| Hard problem (EI-04) | Floor (shipped) | Band | The full answer (follow-on) | Band | Trigger |
|---|---|---|---|---|---|
| **§3 World-scale git storage** | **Local-disk git packs behind the `BlobStore` trait** (relocatable, not node-pinned) | **M3** | **Object-backed packs** (delta/pack/sharding/replication/smart-transport + the within-EU CDN clone class fully wired) | **M5** | the single-node ceiling **measured** (GIT-D4); never pin repos to one node |
| **§1 Erasure vs immutability (git half)** | **Pseudonymous-by-default commits** (immutable bytes never bake erasable PII) | **M3** | **Audited history-rewrite erasure path** (10.6, changed-hash consequence) | **M5 / on-demand** | a body must be expunged; decided *before* the git data model froze |
| (object store) | **`fs`-backed `BlobStore`** | **M0** | **Object-store `BlobStore`** (`MinIO`/`Ceph`, one-line trait swap, 11.2) | **M5** | with object-backed packs / measured blob ceiling |
| **§1 Crypto-shred substrate** | **Per-subject DEK for free-text/profile/chat-body/agent-memory** | **M1** | **+ per-subject CI inline-PII log segments [C1]** | **M4** | CI log tier lands; the per-subject extension where isolable |
| (multi-region) | **Single-cell / one home cell per tenant** | **M1** | **Multi-cell** (cross-cell PII-free bridge live; cell→cell migration; DSR iterates `member_cells`) | **M5** | cross-cell rollup/collab/cross-org demand (OQ-I); FLOOR drills GA-D8/CP-D7/CP-D8 owed |
| **§5 Event volume** | **Single-region event log (general-purpose DB)** | **M0** | **Column-store/time-series seam** for the highest-volume streams | **post-M5** | event volume **measured** to outgrow the DB — not before |
| (flex-field scan) | **GIN-indexed JSONB facet scan** (Issues/KN custom fields) | **M3/M4** | **Generated projection-feeder index** (promoted per facet) | **M5** | a facet in > 5% of view executions, **measured** (OQ-C) |
| **§1 Free-text/immutable residual** | **Structural floor** (per-subject DEK shred + pseudonym-map shred + crypto-shred-reaches-backups) | **M1→M5** | **Counsel/DPO ratification** of the ONE residual posture (10.9, X-7) | **parallel (legal)** | the structural floor ships regardless; the residual is one ratified statement |
| **EI-02 §11 / ADR-18 Restore-verify** | (no floor — it is the gate itself) | **M1** | **Re-confirmed at cell scale under 30× surge** | **M5** | the permanent gate; re-runs forever on every store-touching change |

**The honest-floor rule binds all of these (EI-04 §4):** each floor is tracked in the gap report with its
claimed/proven status + its linked follow-on; the gap being *invisible* is the only failure. The restore-verify
gate and the sandbox-escape gate are the **two permanent gates** (master §4) — restore-verify is Storage's, and
it never moves to "done."

---

## 4. Honesty register (floors, open items, what this roadmap does and does not yet prove)

Per VISION §3 + EI-04 §4 — named, not silent (dated 2026-06-19):

- **FLOOR drills owed when the follow-on is built** (named here so the gap is visible): **CP-D7** (cell→cell
  migration), **GA-D8** (multi-cell erasure), **D-S11/D-S12/D-S13** wait on their consumers (CI in M4 for
  D-S11; Issues in M4 for D-S12; the Git mirror feature for D-S13). All multi-cell drills are gated on
  multi-cell shipping (single-home-cell is v1; cross-cell is designed-not-built, 12.6/OQ-I).
- **The permanent gate (STOR-D1/STOR-D2, restore-verify):** not band-local — it re-runs on every change
  touching a store, forever (master §4). A green restore-verify in M1 is the **precondition for any real tenant
  write**; M2 does not start over a red STOR-D1.
- **`[OPEN → P6/LEGAL]`** (the structural floor ships regardless; the residual is flagged to counsel/DPO): the
  ONE free-text/immutable erasure posture (10.9, X-7, L-2) — Storage contributes its **reach** (crypto-shred
  into backups/reflogs/bitmaps; the history-rewrite crypto-shred half), counsel ratifies the basis **once** for
  all five subsystems; the **worklog/productivity/estimate sensitivity classification** (OQ-H — the works-
  council consultation trigger that gates per-individual OLAP rollups, C5); the **backup-window-vs-erasure-SLA
  residual number** (§7.6) + the erasure-ledger retention carve-out; whether tenant-offboard KEK-destroy
  suffices for an Art. 17 *tenant-wide* assertion; the C6 outbound-mirror lawful-basis for any permitted
  extra-EU transfer.
- **`[OPEN → P6/LEGAL]`** (the mechanism + limits are DECIDED): BYOK/HYOK per-content-class **policy** (which
  classes may be HYOK; the cross-artifact-reference-spanning case), the KMIP/external-key-store adapter, HYOK as
  a Schrems-III mitigation (GD-7).
- **Thresholds Phase 6 measures and sets (Q32 defaults-to-beat, not contract constants):** **RPO ≤ 5 min**,
  **RTO ≤ 1h/tenant ≤ 4h/cell**, restore-verify cadence, cross-seam-assertion sampling rates, the **sharding
  *trigger* metrics** (the measured table/tenant size at which OLTP shards or a tenant gets a dedicated DB/cell,
  §8), the BUS-6 column-store promotion threshold, the projection-feeder promotion point (> 5% of view
  executions). A drill is not "proven" against a guessed number.
- **What this roadmap does NOT yet prove (named):** the object-backed pack *implementation* (chunking, delta-
  base selection, serving from the object tier — the Git M5 deliverable, `[OPEN → P6 Git]`); the chat
  message-log engine choice (wide-column Scylla vs OLTP+object — the Chat M4 call, TE-13); the flexible-field
  physical storage/query model (JSONB property-bag vs materialised — the Issues/KN M3/M4 call, the
  `myelin-query` field-type enum is frozen, the physical storage is the subsystem call); the real-LLM/real-KMS
  production-backend specifics (drills run against the self-hostable stack first). These are **claimed** until
  their drills emit green artifacts in the band that builds them.

---

## 5. Digest (milestones · floors+follow-ons · critical upstream dependencies)

**The sequenced milestones (Storage's slice of each master band):**
- **S-M0 (in M0):** OLTP tier client + **the outbox table** (the 0-loss floor that makes SUB-D1/BUS-D4 green),
  the `fs`-backed `BlobStore` + content-addressed trait, the `forward-only-migration` + `residency-pin` lints.
- **S-M1 (M1) — the keystone band:** the KMS hierarchy + `KeyOrigin` trait, crypto-shred + GD-4 granularity,
  OLTP envelope encryption, **the backup/restore/cross-seam + restore-verify durability gate (the silent-data-
  loss floor)**, the OLAP frame, reserve/settle, the structural GDPR floor.
- **S-M2 (M2):** OLAP fed by the bus (reindex-from-source only), reserve/settle fronts agent runs, the T3
  firehose-archive seam prepped on the resume-cursor transport.
- **S-M3 (M3):** local-disk git packs behind the trait, the within-EU CDN clone/bundle class (C3), the
  outbound-mirror residency gate seam (C6), git's crypto-shred reach.
- **S-M4 (M4):** the CI log tier `(job,step,byte-range)` index (C2), per-subject CI-log DEK (C1), trust-scoped
  cache namespaces (C4), the OLAP restriction-flag gate (C5).
- **S-M5 (M5):** object-store `BlobStore`, object-backed git packs, multi-cell migration, the full DSAR
  crypto-shred fan-out, restore-verify at cell scale + the F6 surge family.
- **S-M6 (M6):** the restore-verify gate runs on Myelin's own commits.

**Floors + follow-ons (each named, scheduled):** `fs`-`BlobStore` (M0) → object-store `BlobStore` (M5); local-
disk git packs (M3) → object-backed packs (M5); pseudonymous-by-default commits (M3) → audited history-rewrite
(M5/on-demand); per-subject DEK for free-text/chat/profile (M1) → + CI inline-PII log segments (M4); single-cell
(M1) → multi-cell + cell→cell migration (M5); GIN JSONB facet scan (M3/M4) → generated projection-feeder index
(M5); single-region event log (M0) → column-store seam (post-M5, measured-trigger); the free-text/immutable
residual structural floor (M1→M5) → counsel/DPO ratification (parallel-legal).

**Critical upstream dependencies (what must be green first):**
- **The outbox `seq` / event-log offset (2.2/2.3, Bus M0)** — IS the cross-seam linearisation cursor; restore-
  to-one-consistent-point is undefinable without it. The outbox table physically lives in Storage's OLTP tier.
- **`classify(field)` + `PersonalDataHolder` + the erasure ledger (10.1/10.2/10.8, GDPR M1)** — drive the
  per-subject-vs-per-tenant key choice (the crypto-shred granularity) and post-restore re-erasure. The DSAR
  fan-out and STOR-D3 are incomplete without the ledger.
- **`placement_of` / `residency_verify` + the `(tenant, region)` partition key + the `residency-pin` lint
  (12.x + 1.6, control plane / substrate M0/M1)** — region-pinning by construction; STOR-D5/D6/D13.
- **reindex-from-source `*.snapshot` re-emit (2.6, Bus M0 seam + per-owner replay M3/M4)** — the *only* rebuild
  path for derived stores (never restore them from their own backups); the cross-seam restore-verify assertion
  rests on this.
- **`serve(AppSpec)` + holder auto-registration + the telemetry signal set (1.1/1.4/1.8, substrate M0)** — the
  harness IS what wires each store through KMS + holder-registration + region-pin; every Storage drill asserts
  against the 1.8 survival signals (a drill that emits no signal has failed).

**The single load-bearing fact:** Storage's restore-verify gate (11.5) is the **Tier-1 silent-data-loss floor**
on the master critical path's M1 node — it must be green **before any subsystem writes real tenant data**, and
it is one of the **two permanent gates** that re-run forever. Everything else in this roadmap sequences around
keeping that floor — and its sibling crypto-shred-reaches-backups + residency-pin floors — green as each new
subsystem's data lands on top of it.
