# Phase 3 — GDPR / Audit Machinery (the PersonalDataHolder spine · DSR orchestrator · tamper-evident log)

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md). Doctrine
> (binding): [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1
> (erasure vs immutability) + §5.3 (reindex-from-source), and
> [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) §1/§10/§11.
> Spine bound: **ADR-12** (PersonalDataHolder spine), ADR-11 (cells/residency), ADR-13 (glue contracts),
> ADR-17 (fail-static, GD-3 staleness bound), ADR-18 (restore-verification, post-restore re-erasure).
> Directives bound: **GD-1, GD-2, GD-3**, X-1…X-5, BUS-2 (outbox-only emit), AG-7 (agent trace is a holder,
> distinct from audit). Resolves: the **DSR orchestrator design** (GD-3 carry-forward), the **data-role
> classification at schema level**, the **generated data-map / RoPA**, the **retention engine**, the
> **consent + sub-processor registries**, the **tamper-evident audit log**, and the named
> **Git-history-erasure reconciliation** (decision-record §(f) → GD-1).
>
> **This doc owns POLICY + ORCHESTRATION; Storage owns MECHANISM.** The crypto-shred *mechanism* (the KMS
> key hierarchy, per-subject vs per-tenant DEK granularity GD-4, the backup/restore cross-seam point) is
> [`storage.md`](./storage.md) §4–§7. The pseudonym-indirection *lever* and `erase(subject)` are
> [`identity-and-access.md`](./identity-and-access.md) §11. This doc is *when to shred, who fans the
> request out, how the deadline is tracked, how erasure is proven, and how every action is made
> tamper-evident.* It does **not** re-decide an ADR; where it sharpens one it cites it.
>
> **Status convention.** *DECIDED* = committed for P4/P5; *FLOOR* = partial answer + named follow-on;
> *[OPEN → P4/P5]* engineering; ***[OPEN — LEGAL]*** = needs counsel/DPO before it binds. Every failable
> property names the **drill** that proves it (Phase 5 executes; this doc enumerates the obligation).
>
> **Prior art this doc builds on (cited inline + §10):** GDPR Arts. 5/6/15–22/28/30/33–34 (the statutory
> shape); **tamper-evident logging** — Haber & Stornetta (*How to Time-Stamp a Digital Document*, J.
> Cryptology 1991), Merkle (*A Digital Signature Based on a Conventional Encryption Function*, CRYPTO 1987)
> for the hash-tree, Crosby & Wallach (*Efficient Data Structures for Tamper-Evident Logging*, USENIX
> Security 2009) for the history/Merkle tree + audit proofs, Google **Certificate Transparency** (RFC
> 6962, Merkle-tree append-only log + consistency/inclusion proofs), **Trillian** (CT's production
> verifiable-log implementation); crypto-shred — NIST SP 800-88r1 ("cryptographic erase"), Boneh & Lipton
> (*A Revocable Backup System*, USENIX Security 1996); pseudonymisation/tombstoning — Kleppmann *DDIA*
> ch. 5; the references-not-payloads + region-pinning posture from `gdpr-eu-sovereignty.md`.

---

## 0. Reading map

- **§1** — purpose, responsibilities, the policy↔mechanism boundary, the two legal postures.
- **§2** — the data model: the **data-role + personal-data classification** (schema-level), the generated
  data-map / RoPA, the DSR/receipt/audit/consent/sub-processor/retention schemas (the stateful register).
- **§3** — the `PersonalDataHolder` contract every store implements + the **exhaustive holder list**.
- **§4** — the **DSR orchestrator** algorithm: fan-out, multi-cell, deadline tracking, verifiable receipts,
  tenant-operability, restriction/rectification/portability.
- **§5** — the **retention engine** (tightest-policy-wins + legal-hold-aware) + consent + sub-processor
  registries + the eDiscovery/legal-hold export (GD-2).
- **§6** — the **tamper-evident audit log** (hash-chain + Merkle, CT-style proofs; every human + agent
  action; retention-bounded carve-out; exportable).
- **§7** — the named **Git-history-erasure reconciliation** (GD-1; decision-record §(f)) — co-owned with
  Git P4 + Legal/DPO. Marks the `[OPEN — LEGAL]` residual.
- **§8** — contracts exposed & consumed (the stable glue).
- **§9** — scaling/sharding in the cell topology; failure modes + drills owed.
- **§10** — cited prior art; required changes to foundational systems; open questions for Phase 4.

**Floors named up front** (VISION §3 / EI-04 §4): **single-cell DSR fan-out + audit are fully designed;
multi-cell fan-out iterates `member_cells` (Tenancy §10.4) — the mechanism is named, the cross-cell
*ordering/atomicity* is a control-plane P4 floor (§4.4).** The **git-history erasure** of
non-pseudonymised author bytes is **NOT solved here** — it is the named reconciliation (§7), with a
documented residual limit. The **RoPA generation** is built; the **legal-text review of the generated
RoPA** is `[OPEN — LEGAL]`. The **per-jurisdiction audit-retention carve-out** is `[OPEN — LEGAL]`.

---

## 1. Purpose, responsibilities, and the policy↔mechanism boundary

### 1.1 The one-paragraph thesis

GDPR-by-construction is a property of the *whole structure*, not a settings page (`gdpr-eu-sovereignty.md`
§0; ADR-12). The platform already makes the hard parts cheap by construction — tenant-first partitioning
(EI-02 §1), references-not-payloads on the bus (ADR-04.4), pseudonym indirection in Id (§ Id-11),
crypto-shred under per-tenant keys in Storage (§ Storage-5), region-pinning in Tenancy (ADR-11). **This
system is the connective tissue that makes those guarantees *operable and provable*:** a single
`PersonalDataHolder` contract every store implements, a **DSR orchestrator** that fans a subject- or
tenant-scoped request to *all* holders and tracks the statutory clock, a **generated data map** so the
holder list can never silently drift, a **retention engine** that ages data out, and a **tamper-evident
audit log** that proves what happened (including that erasure happened). *If we cannot enumerate where a
person's data lives and prove we erased it, none of the rights pipelines is real* (`gdpr-eu-sovereignty.md`
§0, §3.8).

### 1.2 What GDPR/Audit owns (and what it explicitly does not)

| Owns (policy + orchestration) | Does NOT own (mechanism, owned elsewhere) |
|---|---|
| The `PersonalDataHolder` **contract** (`myelin-gdpr` trait; ADR-01) | The crypto-shred **mechanism** — KMS key hierarchy, GD-4 granularity → **Storage** §4–§5 |
| The **DSR orchestrator** (fan-out, deadline, receipts, multi-cell) | The **pseudonym lever** + `erase(subject)` → **Id** §11 |
| The **data-role + personal-data classification** schema tags → the **generated data map / RoPA** | Per-store erasure *internals* (Search purge+reindex; Bus tombstone+key-shred; Refs tombstone) → each **owning system** |
| The **retention engine** (tightest-policy-wins, legal-hold-aware) | The **backup/restore cross-seam consistency point** → **Storage** §7 (ADR-18); GDPR specifies the *post-restore re-erasure* policy |
| **Consent registry** + **sub-processor registry** + transfer gating | Region-pinning enforcement → **Tenancy** §8 (data-layer) |
| The **tamper-evident audit log** (hash-chain/Merkle; every human+agent action) | The **agent execution trace** (a content-addressed Knowledge doc, AG-7) — *distinct* holder, §6.5 |
| The **eDiscovery/legal-hold export** (GD-2) | The **fail-static staleness bound** *value* → DPO ratifies (Id §10, GD-3); GDPR *owns the constraint* (§4.6) |
| The named **Git-history-erasure reconciliation** (GD-1, co-owned) | The git data model itself → **Git P4** (the reconciliation *gates* it) |

The boundary rule: **GDPR/Audit decides *whether, when, and prove*; the owning store decides *how*.** The
orchestrator never reaches into a store; it calls the holder contract (§3) — exactly the no-cross-store-read
law (ADR-01, ADR-13). This is why every holder is *self*-describing and *self*-erasing.

### 1.3 The two legal postures (controller vs processor) drive everything

Myelin operates in two postures simultaneously (`gdpr-eu-sovereignty.md` §1.1), and the *architecture* must
distinguish them because the obligations differ (who answers a DSR, what lawful basis, what retention,
*who has deletion authority*):

- **Processor** for **tenant content** — repos, issues, docs, chat, CI logs, and the personal data of the
  customer's employees/end-users embedded therein. The *customer org is the controller*; Myelin processes
  on documented instructions (Art. 28). A DSR for tenant content is **answered by/for the tenant** (Art. 28
  assistance); Myelin **must not unilaterally erase** tenant content except on tenant instruction or
  offboarding.
- **Controller** for **platform-operational** data — tenant-admin contact details, billing, the security
  audit log Myelin keeps for its own legitimate interest, product telemetry. Myelin *decides the purposes*
  and is the **first-line DSR responder** for this category.

This is encoded as a **schema-level `data_role` tag** (§2.1) on every personal-data-bearing field, **not** a
runtime guess — so DSR routing, lawful basis, retention, and deletion authority **cannot drift** from the
declared posture. It is the same `data_role` the event envelope carries (`event-bus.md` §3.1:
`tenant-content` | `platform-operational`) and the same one Storage classifies on (`storage.md` §2). One
classification, threaded through the bus, the data map, and the DSR router — **the X-5 reconciliation
point** for this system.

---

## 2. The data model / schemas

### 2.1 Schema-level personal-data classification (the spine of the data map — ADR-12.5/.6; GD-12)

Every personal-data-bearing field, in *every* service's schema, carries a **compile-time classification
tag** (not a row, not a runtime annotation — a declaration the `myelin-gdpr` macro/derive emits into a
generated registry). This is the single mechanism that makes the data map *generated, not curated*
(`gdpr-eu-sovereignty.md` §3.8; ADR-12.6) so it cannot silently drift from reality.

```rust
// Applied at the field level via a derive in myelin-gdpr; emitted into the generated data map.
#[personal_data(
    category   = ContactInfo,            // ContactInfo | Identifier | Content | Behavioural | SpecialCategory(...)
    role       = TenantContent,          // TenantContent (processor) | PlatformOperational (controller)
    basis      = Contract,               // Art.6: Contract | LegitimateInterest(lia_ref) | Consent(consent_id) | LegalObligation
    retention  = TenantPolicy,           // TenantPolicy | Fixed(Duration) | UntilContractEnd | AuditCarveOut(Duration)
    erasure    = Pseudonymise,           // Pseudonymise | CryptoShred(key_class) | PurgeReindex | CarveOut
    subject_locator = "principal_id",    // how this row is reached from a subject ref (for locate/export/erase)
)]
email: EncryptedField<Email>,
```

The five tags answer the five questions every rights pipeline asks: **category** (Art. 15 access + Art. 9
special-category detection), **role** (who responds, §1.3), **basis** (Art. 30 RoPA + Art. 6 lawful basis),
**retention** (the engine §5 + the tightest-policy-wins merge), **erasure** (which mechanism the DSR
orchestrator invokes for this field — pseudonymise vs crypto-shred vs purge+reindex vs carve-out). The
**`subject_locator`** is what makes `locate(subject)` a structural operation, not a per-store hand-written
query.

**The `SpecialCategory` tag is load-bearing** (Art. 9): worklog/productivity data that could reveal health,
union membership, etc. (GD-13, `[OPEN — LEGAL]`) is tagged so the data map *flags* it for the DPIA gate
(§2.3) rather than letting it pass as ordinary content.

**Enforcement (E-5 lint, committed to CI):** a `no-untagged-personal-data` architecture test — a field
whose type is in the personal-data type set (`Email`, `Name`, `EncryptedField<…>`, free-text content
columns flagged by the schema owner) **fails to compile** without a `#[personal_data(...)]` tag. This is the
mechanical embodiment of "we forgot the search index is a *structural* failure" (ADR-12.1; GD-3) pushed all
the way down to the field.

### 2.2 The generated data map (the inventory; ADR-12.6)

A build step walks every service's schema + every registered holder and **generates** the data map: a
machine-readable inventory of *what personal data exists, where, under what role/basis/category, with what
retention, reachable by which locator*. It is regenerated on every build and **diffed in CI** — a schema
change that adds/removes/reclassifies personal data shows up as a data-map diff a reviewer (and the DPO)
sees. The map is the substrate for:

```jsonc
// generated; one entry per (service, store, field-class). Illustrative.
{
  "service": "issues", "store": "issue.field_value", "field": "text",
  "category": "Content", "role": "tenant-content", "basis": "contract",
  "retention": "tenant-policy", "erasure": "crypto-shred:tenant-content-dek",
  "holder": "issues", "subject_locator": "mentions[].principal_id",
  "residency": "follows-tenant", "encrypted": true
}
```

- **RoPA (Art. 30)** is a *projection* of the data map grouped by processing activity (§2.3).
- **Erasure fan-out (Art. 17)** reads the map to know which holders + which mechanism per field.
- **Breach scoping (Arts. 33–34)** reads the map + tenant isolation (EI-02 §1) to answer "*whose* data,
  *which* categories, *which* tenants" fast — the 72-hour-clock enabler.
- **Access (Art. 15)** reads the map to enumerate every store before fan-out.

### 2.3 RoPA, DPIA inputs, and the consent / sub-processor / retention / DSR stores (the stateful register — X-4)

GDPR/Audit owns one Postgres-class DB per cell (residency-pinned, per-tenant envelope-encrypted, a holder —
yes, *recursively*, with the audit carve-out §6.4). The stateful components:

| # | Table / store | Holds | Shard key | Blast radius | Crypto-shred unit |
|---|---|---|---|---|---|
| G1 | **`dsr_request`** | a DSR's kind, subject, scope, posture, statutory deadline, state, holder-fan-out checklist | `(tenant, region)` | one tenant | per-tenant DEK |
| G2 | **`dsr_receipt`** | per-holder verifiable completion receipts (signed, hash-linked to the audit log) | `(tenant, region)` | one tenant | per-tenant DEK |
| G3 | **`retention_policy`** | per-(category, tenant, store) TTLs; the tightest-policy-wins inputs; legal-hold flags | `(tenant, region)` | one tenant | per-tenant DEK |
| G4 | **`legal_hold`** | active holds (subject/tenant/artifact-scoped) that **suspend** retention + erasure | `(tenant, region)` | one tenant | n/a (operational) |
| G5 | **`consent`** | versioned, timestamped, withdrawable consent records (controller-data activities) | `(tenant, region)` + subject | one tenant | per-subject DEK |
| G6 | **`subprocessor_registry`** | versioned sub-processor list, region, DPA ref, change-notification + objection state | `(tenant, region)` + global default set | one tenant | per-tenant DEK |
| G7 | **`processing_activity`** | RoPA rows (purpose, basis, categories, recipients, transfers, retention) — *validated against* the generated data map | `(tenant, region)` | one tenant | per-tenant DEK |
| G8 | **The audit log** (§6) | the tamper-evident hash-chain/Merkle log of every human + agent action | `(tenant, region)` + per-tenant tree | one tenant | **audit key** (carve-out expiry; §6.4) |

`dsr_request` + `dsr_receipt` are the **DSR state machine** (§4.2). `processing_activity` (RoPA) is
**generated-then-reviewed**: rows are seeded from the data map (§2.2) so they cannot omit a real flow; the
DPO reviews + ratifies the legal text (`[OPEN — LEGAL]`). A **DPIA gate** (Art. 35) fires when the data-map
diff introduces a new `SpecialCategory` flow, a new agent capability over personal data (EI AI-Act overlap,
§7 of `gdpr-eu-sovereignty.md`), or large-scale systematic monitoring — the gate surfaces the data-flow /
recipient / risk inputs a DPIA needs (`gdpr-eu-sovereignty.md` §1.11).

---

## 3. The `PersonalDataHolder` contract + the EXHAUSTIVE holder list

### 3.1 The contract (ADR-12.1; the `myelin-gdpr` trait)

Every store and subsystem registers as a **holder** implementing five operations for a subject (or a
tenant, for offboarding). The contract is the *only* way the orchestrator touches a store (no cross-store
read):

```rust
pub trait PersonalDataHolder {
    /// Enumerate (don't move) the subject's records + metadata (purposes, basis, retention, source).
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> Result<LocateReport>;     // Art. 15
    /// Produce a portable bundle of data the subject PROVIDED (structured, machine-readable).
    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> Result<PortableBundle>;    // Art. 20
    /// Correct inaccurate data + invalidate/fan-out to derivatives.
    fn rectify(&self, subject: &SubjectRef, patch: Patch) -> Result<RectifyReceipt>;       // Art. 16
    /// Suppress processing (no indexing/agents/analytics) while RETAINING storage. Reversible.
    fn restrict(&self, subject: &SubjectRef, on: bool) -> Result<RestrictReceipt>;         // Art. 18/21
    /// Delete / crypto-shred / pseudonymise per the field's classification; return a verifiable receipt.
    fn erase(&self, subject_or_tenant: EraseScope) -> Result<EraseReceipt>;                // Art. 17
}
```

- **The harness auto-registers every store it opens** (`00 §3.4`; ADR-12.1): the OLTP schema, every blob
  prefix, every cache namespace, the search index a service owns. A service that opens a store the harness
  didn't wrap **fails the `holder-registered` architecture test** — so the holder list cannot drift below
  the data map.
- Each operation returns a **receipt** that is *hash-linked into the audit log* (§6) so completion is
  tamper-evidently provable, not asserted.
- `erase` is **purge/crypto-shred/pseudonymise, never hide** (ADR-12; storage §5.3): Search *purges +
  reindexes* (incl. embeddings — they re-identify, `gdpr-eu-sovereignty.md` §6.6), Bus *crypto-shreds
  inline-PII keys + emits `*.erased` tombstones*, Refs *tombstones* (relies on Id's pseudonym shred), OLTP
  free-text *crypto-shreds the per-subject DEK*, structured pseudonym-referenced rows *rely on Id's
  pseudonym-map delete* (the "delete the identity, not the fact" split, EI-04 §1).

### 3.2 The exhaustive holder list (ADR-12.1 — "we forgot X is a structural failure")

The holder list is **exhaustive and enforced by the data map** (any personal-data field whose holder is not
registered fails CI, §2.1). Per the prompt's mandate, every one is named with its erasure mechanism and its
owning system:

| # | Holder | Personal data it holds | Erasure mechanism | Owner (impl) |
|---|---|---|---|---|
| H1 | **Git subsystem DB** | PR/review/comment authorship (pseudonym), free-text bodies | pseudonymise (Id lever) + crypto-shred inline bodies | Git P4 |
| H2 | **CI subsystem DB** | run actors (pseudonym), log refs | pseudonymise + short-TTL log retention | CI P4 |
| H3 | **Issues subsystem DB** | assignees/watchers/mentions (pseudonym), free-text fields | pseudonymise + crypto-shred free-text | Issues P4 |
| H4 | **Knowledge subsystem DB** | page authorship (pseudonym), free-text content, db-row values | pseudonymise + crypto-shred content | Knowledge P4 |
| H5 | **Chat subsystem DB** | message authorship (pseudonym), message bodies | pseudonymise + crypto-shred bodies (per-subject DEK) | Chat P4 |
| H6 | **Object/blob store** | avatars, attachments, doc media, CI artifacts | crypto-shred (per-tenant/-subject DEK; immutable-tier → key destroy) | Storage §3 |
| H7 | **Search index** | plaintext-derived tokens + **embeddings** | **purge + reindex** (plaintext-derived, not key-shred) | Search §9 |
| H8 | **Event-bus history** | pseudonymous actor; rare inline-PII events | crypto-shred inline-PII keys + `*.erased` tombstones; references-not-payloads makes most events erasure-free | Bus §4.8 |
| H9 | **Caches / CDN** | derived copies, unfurl renders | TTL expiry + targeted purge on erase event | substrate / each service |
| H10 | **Backups / snapshots** | ciphertext of all of the above | **crypto-shred by construction** (key destroyed ⇒ backup ciphertext unrecoverable) + bounded retention window + **post-restore re-erasure** | Storage §7 (ADR-18) |
| H11 | **Agent memory / embeddings** | retrieved context, derived embeddings, RAG state | crypto-shred per-subject DEK + purge embeddings (they re-identify) | Agent Fabric §11 |
| H12 | **Reference graph** | edges referencing the subject; unfurl projections | tombstone (relies on pseudonym shred); backlinks are projections, rebuilt | Refs §4 |
| H13 | **Notification history** | recipient + actor pseudonyms, humanised strings | crypto-shred inline-PII + purge read-models (reindex-from-source) | Notif (NOTIF-3) |
| H14 | **Authz tuples** | `…@subject` tuples referencing the subject | delete the subject's tuples + pseudonym shred | Id §6 |
| H15 | **Identity (Principal/Auth DB + pseudonym map)** | the **erasable profile** + the **pseudonym↔real-identity map** (the erasure lever) | delete pseudonym map (S2) + crypto-shred per-subject profile DEK | Id §11 |
| H16 | **Audit log** (carve-out) | who-did-what (minimised: IDs/pseudonyms, not payloads) | **carve-out** — retain what's lawfully needed to evidence compliance/defend claims; then expire via audit-key crypto-shred | this doc §6.4 |
| H17 | **Agent execution trace** (AG-7) | a content-addressed Knowledge doc of a run's trace | crypto-shred (distinct from audit; §6.5) | Agent Fabric / Knowledge |
| H18 | **GDPR/Audit own stores** (G1–G7) | DSR subjects, consent records, RoPA | crypto-shred per-tenant/-subject DEK (consent G5 = per-subject) | this doc |

**H16 is carved out, not exempt** (§6.4): the audit log retains the *minimised* record of an action under a
distinct legitimate-interest basis with a fixed retention, then crypto-shreds. The carve-out scope per
jurisdiction is `[OPEN — LEGAL]` (GD-5). **H17 (the agent trace) is deliberately distinct from the audit log**
(AG-7, `00 §10.3`): telemetry/trace is sampled+operational; the agent trace is the run's reasoning record;
the audit log is the complete, tamper-evident who-did-what. Keeping them three distinct holders means none
weakens another.

---

## 4. The DSR orchestrator (fan-out · deadline · receipts · multi-cell · tenant-operable)

### 4.1 Responsibility (ADR-12.2; `gdpr-eu-sovereignty.md` §5.2)

The orchestrator receives a rights request, resolves the subject (possibly across cells), **fans out to all
holders via the §3 contract**, tracks completion against the **statutory deadline**, produces a
**verifiable receipt**, and logs every step to the tamper-evident audit log. It is operable by **Myelin**
(for controller data) and **by/for tenants** (Art. 28 assistance, for their data subjects).

### 4.2 The DSR state machine + algorithm

```
received → validated → fanned-out → {awaiting-holders} → verified → completed
   │            │            │              │                │           │
   │            │            │              │                │           └─ receipt sealed into audit log (§6)
   │            │            │              │                └─ every holder receipt present + checked
   │            │            │              └─ per-holder checklist (from the data map §2.2); retries; legal-hold check
   │            │            └─ orchestrator calls holder.{locate|export|rectify|restrict|erase} per the map
   │            └─ identity-proof of the requester; posture decided (controller vs processor, §1.3)
   └─ deadline computed: now + 1 month (Art. 12(3)); extendable to 3 for complex (recorded reason)
```

1. **Validate + decide posture.** Verify the requester (a subject for controller data; a tenant admin on
   behalf of a data subject for tenant content, §1.3). Refuse a Myelin-initiated erase of *tenant content*
   absent tenant instruction or offboarding (processor posture).
2. **Resolve scope from the data map** (§2.2): the request becomes a **per-holder checklist** — exactly the
   holders the map says hold this subject's data, with the per-field erasure mechanism. *The map, not a
   hand-written list, drives the fan-out* — this is what makes "we forgot the search index" impossible.
3. **Legal-hold gate** (§5.3): if an active `legal_hold` (G4) covers the subject/tenant/artifact, **erasure
   and retention-expiry are suspended** for the held scope (Art. 17(3)(e) — legal claims); the request is
   recorded as *partially deferred* with the hold reason. Access/portability still proceed.
4. **Fan out** through the holder contract. Each call is **idempotent + resumable** (the checklist is the
   durable state; a crashed orchestrator resumes from the checklist, re-driving only un-receipted holders).
   For **erase**, the canonical order (storage §5.3) is: **Id.erase (pseudonym map first)** → KMS.destroy
   per-subject DEK → Search.purge+reindex → Refs.tombstone → Bus.erase → notif/authz/agent-memory → record
   receipt. Erasing the pseudonym map *first* means every downstream holder already sees only the opaque
   pseudonym.
5. **Collect + verify receipts.** Each holder returns a signed receipt; the orchestrator checks the
   checklist is complete and seals a **DSR completion receipt** (G2) that is hash-linked into the audit log
   (§6) — the *proof of erasure* an Art. 28 audit or a supervisory authority can verify.
6. **Track the deadline.** The 1-month clock (Art. 12(3), extendable to 3 with a recorded reason) is a
   **durable timer** (delegated to the durable-workflow engine, ADR-09 — the same substrate as SLA timers
   and the Trigger `stale_after`, `event-bus.md` §4.6; we do **not** reinvent durable timers). A
   nearing-deadline emits a `Signal` (warning severity) to the operator/tenant.

### 4.3 Verifiable receipts (the proof, not the promise)

A receipt is **content-addressed and signed**: `receipt = sign( hash(request_id ∥ holder ∥ scope ∥ outcome
∥ key_epoch_destroyed? ∥ timestamp) )`, appended to the per-tenant audit Merkle tree (§6). Because the
receipt records the *key epoch destroyed* for crypto-shred holders and the *purge+reindex cursor* for
Search, "we erased it" is **independently checkable** against the KMS key-destruction log and the search
index state — not merely asserted. The bundle of receipts for a request is exportable as a **tamper-evident
DSR completion certificate** (ties to the eDiscovery export, §5.4 / GD-2).

### 4.4 Multi-cell fan-out (the floor named honestly)

For a **multi-cell tenant** (a 10k-person org spanning cells in one region, SC-2/SC-3), the orchestrator
**iterates `member_cells`** from the placement record (Tenancy §10.4: `tenant_placement.member_cells ∪
home_cell`, all in the same region) and fans out to each cell's holders, then merges the per-cell receipts
into one certificate. **Single-cell fan-out is fully designed; the cross-cell *ordering and atomicity***
(what if cell B's erase succeeds but cell A's crashes mid-flight — the checklist makes it resumable, but a
*globally-atomic* multi-cell erase is harder) **is a control-plane P4 floor** (the orchestrator runs in
each cell; the control plane sequences the wave, never holding personal data — ADR-11.4, Tenancy §3.1).
Follow-on owner: **P4 control-plane + multi-cell tenancy resolution (SC-2/SC-3)**. Residency is preserved:
no cell reads another cell's personal data; each cell erases its own holders and returns only a receipt
(no PII) to the merge.

### 4.5 Tenant-operability (Art. 28 assistance)

The orchestrator is exposed **to tenants** (self-service or assisted) for *their* data subjects: a tenant
admin opens a DSR for one of their employees/end-users; the orchestrator runs the same fan-out scoped to
that tenant. The **tenant offboarding pipeline** (Art. 28 "return or delete at end of contract") is a
*tenant-scoped erase*: full export bundle + **tenant-granularity crypto-shred** (destroy the tenant KEK
⇒ every DEK unwrappable ⇒ whole tenant unrecoverable, backups included — storage §5, ADR-12.3) + a sealed
offboarding certificate. This is just `erase(EraseScope::Tenant)` over the holder list.

### 4.6 Restriction, rectification, portability (the non-erasure rights)

- **Restriction (Art. 18) / objection (Art. 21):** `restrict(subject, on=true)` sets a per-subject
  suppression flag every holder honours — **no indexing, no agent use, no analytics, no notification
  generation** while *retaining* storage. The event bus and agent fabric must read the flag (a
  cross-cutting obligation, §10 required-changes). Reversible.
- **Rectification (Art. 16):** `rectify` corrects the primary store **and fans out to derivatives** — the
  search index reindexes, the reference graph re-renders, notification read-models rebuild (all via
  **reindex-from-source**, EI-04 §5.3, so the derivative is rebuilt from the corrected source, never
  patched in place and left to drift).
- **Portability (Art. 20):** `export` returns *data the subject provided* in structured machine-readable
  form (JSON/CSV; git via clone; docs as Markdown — `gdpr-eu-sovereignty.md` §1.5). Distinct from access
  (Art. 15, which is *all* data + metadata): the orchestrator runs `locate` for access and `export` for
  portability, and labels the bundle accordingly.

### 4.7 Interaction with the fail-static window (GD-3)

The fail-static staleness bound (Id §10, ADR-17) is the **residual GDPR-revocation exposure window**: a
just-disabled actor may be served a stale "active" answer for ≤ `W`. **GDPR/Audit owns the *constraint*
(`W ≤ deprovision/revocation SLA`, and `W` must contain the agent-token TTL); the DPO ratifies the *value*
(L-1, proposed `W = 5 min`).** This doc records `W` as a *named, dated, DPO-ratified* residual in the RoPA
(it is a processing characteristic), so the exposure is written down, not silent (GD-3). The DSR
"disabled-user → zero-access within N min" drill (Id §13 D1) shares the bound.

---

## 5. Retention engine · consent · sub-processors · eDiscovery/legal-hold export

### 5.1 The retention engine (tightest-policy-wins + legal-hold-aware; GD-2)

Every personal-data category has a retention policy (the `retention` tag, §2.1). The engine drives
**automated expiry/deletion** by category TTL (`gdpr-eu-sovereignty.md` §3.6). Two disciplines:

- **Tightest-policy-wins merge.** When multiple policies apply to one datum (a Myelin platform default, a
  tenant-configured stricter retention, a category statutory minimum, a *retention floor* from a legal
  obligation), the engine computes the **effective retention = the *most restrictive* that does not violate
  a legal-retention floor**. E.g. a tenant who configures "delete chat after 30 days" wins over a 90-day
  platform default; but a lawful 6-month security-log floor wins over a tenant's "delete logs immediately."
  The merge is deterministic and **recorded** (the effective policy and which input won is auditable).
- **Legal-hold-aware (suspend, don't delete).** An active `legal_hold` (G4) over a subject/tenant/artifact
  **suspends both retention-expiry and erasure** for the held scope (Art. 17(3)(e)). When the hold lifts,
  the suspended expiry/erasure **resumes** (the engine re-evaluates and runs the deferred deletion). A
  pending DSR erase that hit a hold is recorded as *deferred-by-hold* (§4.2 step 3), not silently dropped.

Expiry uses the same erasure mechanisms (§3): crypto-shred for immutable/backup-reachable data,
purge+reindex for the search index, bounded-retention TTL for the bus log (`event-bus.md` §4.8: 90-day-hot
default, OLAP/audit are the long-term holders).

### 5.2 Consent registry (G5)

For **controller-posture** activities that rest on consent (telemetry, marketing — tenant-content rests on
*contract*, not consent, §1.3): **versioned, timestamped, granular, withdrawable** consent records.
Withdrawal **propagates**: it stops the consented processing path and may trigger deletion
(`gdpr-eu-sovereignty.md` §3.6). Each consent record is per-subject-keyed (its own DEK) so withdrawal +
erasure are crypto-shred-clean. The lawful-basis-per-activity feeds the RoPA (G7).

### 5.3 Sub-processor registry + transfer gating (G6)

A **versioned, public + per-tenant sub-processor list** (`gdpr-eu-sovereignty.md` §1.8, §3.7): each entry
carries region, DPA reference, and the **change-notification + objection** workflow. Sovereignty stance
(`gdpr-eu-sovereignty.md` §8.1): **no personal data leaves the EU/EEA by default; transfers are off and
gated.** A would-be transfer requires a valid mechanism + a recorded Transfer Impact Assessment + tenant
transparency — and is **denied by default** at the adapter seam (every personal-data-touching external
dependency is a region-aware, EU-preferring, swappable adapter — ADR-12.8; the future real-LLM backend is
one such adapter, AG-9 `[OPEN — LEGAL]`). The registry is the source of truth the transfer gate reads.

### 5.4 Tamper-evident eDiscovery / legal-hold export (GD-2)

Alongside DSR export receipts, GDPR/Audit provides a **tamper-evident eDiscovery export**: a subject-,
tenant-, or matter-scoped bundle of records + the audit-log proofs (§6) that establish chain-of-custody.
The export is **content-addressed and Merkle-proof-bearing** (each included record carries its inclusion
proof against the per-tenant audit tree, §6.3), so an eDiscovery/audit recipient can *verify* the bundle was
not altered. This is the GD-2 companion to the DSR receipt (§4.3): the same tamper-evident substrate serves
*"prove we erased it"* (DSR receipt) and *"prove this is the unaltered record"* (eDiscovery). A legal-hold
freezes the scope (§5.1) so the matter's data is preserved while the export is assembled.

---

## 6. The tamper-evident audit log (hash-chain + Merkle; CT-style proofs)

### 6.1 Decision (DECIDED — ADR-12.9; the design and its prior art)

**One tamper-evident, append-only audit log records every human *and* agent action** (ADR-12.9; EI-02 §2
"agents flow through the same audit path as humans"). It is its own retention-bounded `PersonalDataHolder`
(H16), **distinct from telemetry** (`00 §10.3`) and **distinct from the agent execution trace** (AG-7,
§6.5). The construction is a **per-tenant hash-chain whose entries are also leaves of a Merkle tree**, with
**Certificate-Transparency-style inclusion and consistency proofs** — the proven design for an append-only
verifiable log (RFC 6962; Crosby & Wallach 2009; Haber & Stornetta 1991; Merkle 1987).

**Why hash-chain *and* Merkle, not one or the other** (the written why):
- A **hash-chain** (`entry_n.prev_hash = H(entry_{n-1})`) gives cheap **append-only tamper-evidence**: any
  retroactive edit/deletion breaks the chain from that point forward. This is the Haber–Stornetta / Bitcoin
  linked-timestamp construction.
- A **Merkle tree over the entries** (Crosby–Wallach "history tree"; RFC 6962) additionally gives
  **efficient proofs**: an **inclusion proof** ("this exact action is in the log") is `O(log n)`, and a
  **consistency proof** ("the log at time `t2` is an append-only extension of the log at `t1`" — it wasn't
  forked or rewritten) is `O(log n)`. A bare hash-chain forces an `O(n)` re-scan to prove either. The
  Merkle root is what we **publish/notarise** (§6.2) to make tampering externally detectable.

We adopt the **Trillian/CT model** (a verifiable Merkle log with signed tree heads) as the reference
implementation class, self-hosted in-cell (ADR-11 portability). **We deliberately do NOT use a blockchain**
(the written deviation): a public/permissioned chain buys global byzantine consensus we do not need
(the cell is the trust + residency boundary), at the cost of throughput, operational weight, and a
residency problem (replicating the log off-cell). A per-cell signed Merkle log + periodic external
notarisation of the signed tree head gives the tamper-evidence property without the chain's costs.

### 6.2 The entry schema + the signed tree head

```sql
CREATE TABLE audit_entry (
  tenant       text NOT NULL,
  region       text NOT NULL,
  seq          bigint NOT NULL,            -- per-tenant monotonic; the chain + tree leaf index
  prev_hash    bytea  NOT NULL,            -- H(prev entry) — the hash-chain link
  leaf_hash    bytea  NOT NULL,            -- H(canonical(entry)) — the Merkle leaf
  occurred_at  timestamptz NOT NULL,
  actor        text NOT NULL,              -- pseudonymous principal ref (human|agent|service); MINIMISED — never payload
  actor_kind   text NOT NULL,             -- human | agent | service (agents audited identically — EI-02 §2)
  on_behalf_of text,                       -- the human a delegated agent acted for (caused-by anchor)
  action       text NOT NULL,             -- e.g. 'dsr.erase', 'authz.tuple_written', 'break_glass', 'agent.effect_applied'
  subject      text NOT NULL,             -- ArtifactRef the action targeted (an ID, not content)
  correlation_id text NOT NULL,           -- the causal root (BUS-5) — ties the audit walk to the "why" view
  causation_id text,                       -- immediate parent (nested causality)
  outcome      text NOT NULL,             -- allowed | denied | applied | failed
  detail_ref   text,                       -- pointer to an erasable detail blob if any (references-not-payloads)
  PRIMARY KEY (tenant, seq)
);
-- The signed tree head: the per-tenant Merkle root, signed, published periodically (notarisation point).
CREATE TABLE audit_sth (
  tenant text NOT NULL, region text NOT NULL,
  tree_size bigint NOT NULL, root_hash bytea NOT NULL,
  signed_at timestamptz NOT NULL, signature bytea NOT NULL,
  PRIMARY KEY (tenant, tree_size)
);
```

- **Minimised by design** (`gdpr-eu-sovereignty.md` §3.4): `actor`/`subject` are **pseudonymous IDs /
  `ArtifactRef`s, never payloads** — so erasing the person (Id pseudonym shred) tombstones the *identity*
  in the audit log while the *fact* (an action of kind K happened at time T) survives for accountability.
  This is the same delete-the-identity-not-the-fact split (EI-04 §1).
- **Written via the outbox only** (BUS-2): every action-taking service `emit`s an audit event; an audit
  consumer (an infra subscription on the firehose, `event-bus.md` §1.2) appends it to the chain+tree. *No
  service writes the audit log directly* — it goes through the one sanctioned emit path, so audit coverage
  is a property of the bus, not of each service remembering to log.
- **Causality-carried** (`00 §10.1`): `correlation_id`/`causation_id` mean the audit log *is* the "why did
  this happen" walk — one mechanism for audit + provenance + the loop guard, not three (EI-02 §6).

### 6.3 The proofs (inclusion + consistency) and verification

- **Inclusion proof**: given an action and the signed tree head, an `O(log n)` Merkle path proves the
  action is in the log at that tree size. Used by the eDiscovery export (§5.4) and any DSR receipt that
  needs to show "this erase was logged."
- **Consistency proof**: given two signed tree heads (`size t1 < t2`), an `O(log n)` proof shows the `t2`
  log is an **append-only superset** of the `t1` log — i.e. nobody rewrote history between the two. An
  auditor periodically fetching the signed tree head and checking consistency detects any tampering.
- **External notarisation**: the signed tree head is periodically anchored to an **independent
  witness** (a second-party timestamping authority / a different cell's notary / an RFC-3161 TSA), so even
  a fully-compromised cell cannot rewrite history undetectably — the witness holds a root that won't match a
  forged log. (The witness sees only an opaque root hash — no personal data crosses, residency-safe.)

### 6.4 The audit carve-out (H16 — retention vs erasure; `[OPEN — LEGAL]`)

Audit logs contain personal data (who did what) yet must persist to *prove* compliance/security
(`gdpr-eu-sovereignty.md` §3.4, §6.3). Resolution (ADR-12.9):
- Kept under a **distinct legitimate-interest basis** with a **defined retention period**.
- **Minimised** (IDs/pseudonyms, not payloads — §6.2).
- **Carved out of erasure** to the extent law permits: when a subject is erased, the audit log retains the
  *minimised* record (now referencing only the opaque pseudonym, since Id's pseudonym shred already ran) of
  what was *needed* to evidence compliance / defend claims, then **expires via crypto-shred of the audit
  key** at retention end. The exact carve-out scope per jurisdiction is **`[OPEN — LEGAL]` (GD-5)** —
  flagged for counsel/DPO before it binds.

Crucially, the carve-out does **not** weaken tamper-evidence: erasing the *identity* (pseudonym) leaves the
chain+tree intact (the leaf hash is over the pseudonymous entry, which never contained the real identity).
We never rewrite an audit entry to satisfy erasure — that would break the chain; instead the real identity
was never in the entry (it lived in Id's erasable pseudonym map).

### 6.5 Distinct from the agent execution trace (AG-7) and telemetry

Three separate holders, kept separate on purpose:
- **Telemetry** (`00 §10`) — operational, sampled, RED/USE signals; *not* a personal-data record of intent.
- **Agent execution trace** (AG-7, H17) — a content-addressed *Knowledge document* of a single run's
  reasoning/tool-calls; erasable; reused from `myelin-content`. It records *what the agent thought/did
  within a run*.
- **Audit log** (H16) — the complete, tamper-evident, who-did-what across *all* actors. It records *that an
  action with effect happened*.

An agent's *applied effect* lands in the audit log (like a human's action); the agent's *reasoning* lands
in its trace. Neither weakens the other.

---

## 7. The named Git-history-erasure reconciliation (GD-1; decision-record §(f)) — co-owned with Legal/DPO

This is the **first-class write-up** the doctrine (EI-04 §1) and decision-record §(f) demand — *not a
checkbox*. It is co-owned with the **Git P4 agent + Legal/DPO** and **gates the Git P4 data model**.

### 7.1 The problem, stated honestly

The platform's erasure answer (crypto-shred + references-not-payloads + pseudonym indirection) solves the
**event-log half** (EI-04 §1 "workable") and everything keyed under a destroyable DEK. It does **NOT** solve
the **git-history half**: a commit's **author name/email is baked into the commit hash** (the SHA is over
the commit object including author/committer identity). You cannot tombstone or crypto-shred those bytes
without **rewriting history and changing every downstream hash** — breaking clones, signatures, and every
reference to the old SHA. Pretending crypto-shred reaches it is the trap (EI-04 §1; storage §5.4).

### 7.2 The only levers — none free

| Lever | What it does | Cost / residual |
|---|---|---|
| **Pseudonymous-by-default commit identities** (the *prerequisite*, GIT-1) | Commits are authored to a **stable opaque author id** (`<pseudonym>@<tenant>.noreply`); the person↔pseudonym mapping lives in Id's erasable pseudonym map (S2). Erasing the person deletes the map ⇒ the immutable commit bytes hold only the opaque pseudonym. | **Must be enforced at COMMIT TIME** — it is a *commit-time prerequisite*, nearly impossible to bolt on later. Gates the Git P4 data model. Residual: **PII committed into file *content*** (not metadata) is unaffected. |
| **Supported history-rewrite** (filter-repo-class) | For PII in *file content* (or non-pseudonymised legacy history), a tenant-initiated, audited rewrite removes it — **changing every downstream hash**. | Disruptive: invalidates clones/signatures/refs; communicated as a hash-changing operation; a tenant-initiated, audited, rate-limited op. Crypto-shred reaches **reflogs, bitmaps, and backups of the pack tier** (those *are* shreddable via the per-tenant blob DEK — storage §5.4), but **not** the commit-object bytes themselves. |
| **Documented lawful-basis limit** | Treat commit author metadata under a documented basis with **Art. 17 "technically infeasible / disproportionate effort"** limits, with the residual exposure written down. | **`[OPEN — LEGAL]` (GD-1/L-2)** — counsel/DPO decide how far per-subject erasure must reach into immutable VCS history vs. the documented limit. The *mitigation* (pseudonymous-by-default, §7.2 row 1) makes this question *rarely bite*. |

### 7.3 The decision (what binds now)

- **Pseudonymous-by-default commit identities are a commit-time prerequisite (GIT-1), DECIDED** — the Git
  P4 data model **must** mint commits to a stable opaque author id, with the erasable mapping in Id (S2).
  This is the lever that makes git-history erasure *usually* a pseudonym-map delete, not a history rewrite.
- **History-rewrite is the supported (disruptive) path** for the residual (PII in content / legacy
  non-pseudonymised history) — tenant-initiated, audited, hash-changing, rate-limited.
- **The residual limit is `[OPEN — LEGAL]`** (GD-1/L-2): the exact Art. 17 reach into immutable commit-object
  bytes, and whether the documented-lawful-basis-limit suffices, is decided by counsel/DPO before it binds.
  The engineering posture is *minimise PII in immutable history so the legal question rarely bites*.
- **The git data model keeps an object-backing migration seam** (STOR-5 / GIT-1): repos stay relocatable
  (never node-pinned), so the pack tier (and its shreddable reflogs/bitmaps/backups) can move to
  object-backed storage — orthogonal to but consistent with this reconciliation.

This reconciliation is **dated and carried into the gap report** (E-3): the *pseudonymous-commit residual
limit* is a named shipped-floor with Legal as the follow-on owner.

---

## 8. Contracts exposed & consumed (the stable glue)

### 8.1 Exposed (what other systems + tenants consume)

| Contract | Signature (illustrative) | Consumed by | Semantics |
|---|---|---|---|
| **`PersonalDataHolder`** | `{locate, export, rectify, restrict, erase}(subject\|tenant) → Receipt` | DSR orchestrator (calls); every store (implements) | the §3 contract; erasure is purge/shred/pseudonymise, never hide; receipt hash-linked to audit (§6). |
| **DSR submit** | `dsr_submit(kind, subject, scope, posture) → dsr_id` | Myelin ops, **tenant admins** (Art. 28) | opens the §4 state machine; deadline timer armed. |
| **DSR status / certificate** | `dsr_status(dsr_id) → {state, deadline, checklist}`; `dsr_certificate(dsr_id) → MerkleProvenBundle` | requester, auditor | verifiable completion certificate (§4.3). |
| **classify** | `classify(field) → PersonalDataTag` (the `#[personal_data]` derive) | every schema owner | feeds the generated data map (§2.1); `no-untagged-personal-data` lint. |
| **data map / RoPA** | `data_map() → Inventory`; `ropa(tenant) → ProcessingActivities` | DPO, breach-scoping, DSR fan-out | generated, diffed in CI (§2.2). |
| **retention** | `effective_retention(category, tenant, store) → Policy` (tightest-policy-wins) | retention engine, every store | §5.1; legal-hold-aware. |
| **legal-hold** | `legal_hold_set(scope, on)` | ops/legal | suspends retention + erasure for scope (§5.1/§5.3). |
| **consent** | `consent_record/withdraw(subject, activity, version)` | controller-posture activities | §5.2; withdrawal propagates. |
| **sub-processor / transfer gate** | `subprocessors(tenant) → list`; `transfer_allowed(target_region) → bool` | adapters, tenant transparency UI | §5.3; deny extra-EU by default. |
| **eDiscovery export** | `ediscovery_export(scope) → MerkleProvenBundle` | legal/auditors | tamper-evident, legal-hold-frozen (§5.4 / GD-2). |
| **audit append** | (via outbox) `emit(audit_event)` → appended to chain+tree | every action-taking service | the ONLY way audit is written (BUS-2); §6.2. |
| **audit proofs** | `inclusion_proof(action) → MerklePath`; `consistency_proof(t1,t2) → Proof`; `signed_tree_head(tenant) → STH` | auditors, eDiscovery, the witness | §6.3 verifiable proofs. |
| **telemetry (X-1)** | `dsr_deadline_margin`, `erasure_fanout_coverage`, `audit_append_lag`, `sth_publish_age`, `legal_hold_active_count` | Phase-5 drills | the survival signals the drills read. |

### 8.2 Consumed (what this system depends on)

| Consumed | From | Used for |
|---|---|---|
| `erase(subject)` + `resolve_pseudonym` (the pseudonym lever) | **Id** §11/§12 | the erasure lever; pseudonym-map delete is fan-out step 1 (§4.2). |
| KMS `destroy(key)` + key hierarchy + GD-4 granularity | **Storage** §4–§5 | crypto-shred mechanism; receipts record the destroyed key epoch (§4.3). |
| Backup/restore cross-seam point + **post-restore re-erasure** | **Storage** §7 (ADR-18) | erasure reaches backups by construction; restore must not resurrect a destroyed key/erased subject. |
| Bus `erase` (tombstone + inline-PII key-shred) + outbox emit + consumer template | **Bus** §4.8/§5 | bus is a holder; audit appended via outbox; `*.erased` tombstones. |
| Search `purge+reindex` (incl. embeddings); Refs `tombstone`; reindex-from-source | **Search** §9 / **Refs** §4 | per-derivative erasure + rectification fan-out. |
| `member_cells` placement; PII-free control plane | **Tenancy** §10 | multi-cell DSR fan-out (§4.4). |
| durable timer / signal | **Bus/workflow** (ADR-09) | the 1-month deadline timer + nearing-deadline Signal (§4.2). |
| fail-static staleness bound `W` (DPO-ratified) | **Id** §10 (GD-3) | the residual revocation-exposure window recorded in RoPA (§4.7). |
| agent execution trace as a content-addressed holder | **Agent Fabric / Knowledge** (AG-7) | H17; kept distinct from the audit log (§6.5). |

---

## 9. Scaling/sharding in the cell topology · failure modes + drills owed

### 9.1 Scaling (ADR-11)

- **In-cell, tenant-partitioned.** GDPR/Audit stores are `(tenant, region)`-keyed; the audit Merkle tree is
  **per-tenant** so proofs and crypto-shred are tenant-scoped (and tenant offboarding shreds one tenant's
  tree). No cross-tenant query path (EI-02 §1).
- **Audit append throughput** rides the same JetStream firehose + outbox the bus scales (Bus §7); the
  audit consumer is an infra subscription. Append is `O(1)` amortised (chain link + tree leaf); proofs are
  `O(log n)`. The audit log is a candidate for the **column-store/time-series seam** (BUS-6) *only when
  measured* — until then the per-tenant tree + 90-day-hot + long-term archive suffices.
- **DSR fan-out** is bounded + resumable (the checklist is durable state); it is *not* latency-critical (a
  1-month deadline), so it runs off the hot path with bounded concurrency (X-3).
- **Multi-cell** iterates `member_cells` (§4.4); cross-cell atomicity is the named P4 floor.

### 9.2 Failure modes + drills owed (PROVE-IT; T-5)

| # | Property / failure mode | Drill (quantified gate) | Owner | Directive/ADR |
|---|---|---|---|---|
| GA-1 | **Erasure misses a holder** | **erasure-reaches-every-holder**: erase a subject seeded into *all* H1–H18 holders; assert the data-map-driven fan-out hit **every** holder and a post-erase `locate` returns **zero** recoverable PII. Gate: **0 holders missed, 0 PII recoverable.** | this doc + every holder | ADR-12.1, GD-3, T-5 |
| GA-2 | **Erasure doesn't reach search** | **erasure-reaches-search**: assert the subject's docs **and embeddings** are purged+reindexed out of the index (not merely hidden). Gate: **0 hits, 0 embedding re-identification.** | Search | T-5 (named) |
| GA-3 | **Crypto-shred doesn't reach backups** | **crypto-shred-reaches-backups** (storage D-S4): destroy the key; restore a backup; assert the subject's ciphertext is **unrecoverable** and **post-restore re-erasure** ran (no resurrection). Gate: **0 resurrected.** | Storage + this doc | ADR-18, GD-14 |
| GA-4 | **Audit log tampered undetectably** | **audit-tamper-detection**: retroactively edit/delete an entry; assert the chain breaks + a **consistency proof against the published STH fails** (and the external witness mismatches). Gate: **tamper detected, 100%.** | this doc §6 | ADR-12.9 |
| GA-5 | **DSR deadline missed silently** | **dsr-deadline**: open a DSR; assert the durable timer fires a warning Signal before the 1-month deadline and the certificate seals on completion. Gate: **0 silent misses.** | this doc §4 | ADR-12.2 |
| GA-6 | **Data map drifts from reality** | **data-map-drift**: add an untagged personal-data field; assert the `no-untagged-personal-data` lint fails the build and the data-map diff surfaces it. Gate: **build red on untagged PII.** | this doc §2.1 | ADR-12.6, E-5 |
| GA-7 | **Legal-hold not honoured** | **legal-hold**: set a hold over a subject; submit an erase; assert erasure is **deferred-by-hold** (not run), then resumes on hold-lift. Gate: **0 held-scope deletions; resume correct.** | this doc §5 | GD-2 |
| GA-8 | **Multi-cell fan-out misses a cell** | **multi-cell-erasure**: erase a multi-cell tenant's subject; assert fan-out iterated **all** `member_cells ∪ home_cell` and merged a complete receipt set. Gate: **0 cells missed.** | this doc §4.4 + Tenancy | SC-2/SC-3 |
| GA-9 | **Restriction leaks into processing** | **restriction**: restrict a subject; assert no indexing/agent-use/analytics/notification occurs while storage is retained; reversible. Gate: **0 processing of a restricted subject.** | this doc §4.6 + Bus/Agents | Art. 18/21 |

Each drill emits a **green artifact** when it passes; until then the property is **claimed, not proven**
(T-4). The §8.1 telemetry signals are the assertions the drills read (T-1: observability is part of the
pass condition).

### 9.3 Stateful-component register + blast-radius note (X-4)

| Stateful component | Shard / blast-radius plan | Blast radius if it dies |
|---|---|---|
| `dsr_request`/`dsr_receipt` (G1/G2) | per-tenant PG; resumable checklist | a DSR pauses; resumes from the durable checklist — **no loss, no silent miss** |
| Retention/consent/sub-processor/RoPA (G3–G7) | per-tenant PG | expiry jobs pause; data over-retained briefly (fails *safe* — toward retention, not deletion) |
| **Audit log + STH** (G8) | per-tenant Merkle tree; STH externally notarised | append stalls (outbox buffers — no loss); proofs unavailable until recovery; tamper still detectable via the witnessed STH |
| Legal-hold (G4) | per-tenant PG | **fails safe**: an unreachable hold store defaults to *suspend* (never auto-deletes held data) |
Everything else (the orchestrator workers, the data-map generator, the audit consumer) is **stateless and
replaceable** — recoverable by replaying the durable checklist + the bus.

---

## 10. Cited prior art · required changes to foundational systems · open questions for Phase 4

### 10.1 Cited prior art

- **Tamper-evident / verifiable logs.** Stuart Haber & W. Scott Stornetta, *How to Time-Stamp a Digital
  Document* (J. Cryptology, 1991) — the linked-timestamp hash-chain. Ralph Merkle, *A Digital Signature
  Based on a Conventional Encryption Function* (CRYPTO 1987) — the hash tree. Scott Crosby & Dan Wallach,
  *Efficient Data Structures for Tamper-Evident Logging* (USENIX Security 2009) — the history/Merkle tree +
  efficient inclusion/consistency proofs (the model we adopt). Ben Laurie et al., **Certificate
  Transparency** (RFC 6962) — append-only Merkle log, signed tree heads, inclusion/consistency proofs, the
  witness/auditor split; **Trillian** as the production verifiable-log class. Deliberate non-adoption of
  blockchain (§6.1 — global byzantine consensus we don't need + a residency problem).
- **Crypto-shred / erasure.** NIST SP 800-88r1 (*Guidelines for Media Sanitization* — "cryptographic
  erase"). Boneh & Lipton, *A Revocable Backup System* (USENIX Security 1996) — destroy-the-key deletion of
  immutable/backup data. Kleppmann, *DDIA* ch. 5 — tombstones/pseudonymisation as the append-only erasure
  posture (delete the identity, not the fact).
- **GDPR engineering.** GDPR Arts. 5 (principles), 6 (lawful basis), 9 (special category), 12–22 (data
  subject rights), 17(3) (erasure limits/legal claims), 28 (processor/sub-processor + assistance), 30
  (RoPA), 33–34 (breach), 35 (DPIA), 44–49 (transfers); Schrems II (CJEU 2020) + the DPF instability that
  motivates "no-transfer-by-default." The platform's own `gdpr-eu-sovereignty.md` (§5 shared contracts, §6
  the hard conflicts) is the requirements source this doc operationalises.
- **Doctrine.** EI-04 §1 (erasure vs immutability — the event-log half "workable", the git-history half
  unsolved), §5.3 (reindex-from-source as a resilience primitive — rectification/derivative rebuild);
  EI-02 §1 (tenant-first breach scoping), §10 (fail-static residual window), §11 (restore + cross-seam).

### 10.2 Required changes to foundational systems (cross-references that must hold)

- **Substrate (`00`):** the `#[personal_data]` derive + `no-untagged-personal-data` lint (§2.1) extend the
  `00 §2.11` lint table; the harness's holder auto-registration (`00 §3.4`) must register against *this*
  orchestrator. **No change to a committed contract** — additive.
- **Id (§11/§12):** `erase(subject)` (pseudonym-map delete) is the orchestrator's fan-out **step 1**; Id
  must keep the pseudonym map the single erasure lever. The fail-static bound `W` (§10) is *recorded by this
  doc in the RoPA* as the residual revocation window (GD-3).
- **Storage (§4–§7):** owns the crypto-shred mechanism + the cross-seam restore point + **post-restore
  re-erasure** (this doc specifies the *policy*: a restore re-applies the deletion log so backups don't
  resurrect erased data — `gdpr-eu-sovereignty.md` §6.5; ADR-18/GD-14).
- **Bus (§4.8):** audit is appended **via the outbox** (BUS-2) — the audit consumer is an infra
  subscription; `*.erased` tombstones + inline-PII key-shred are the bus's holder impl.
- **Search/Refs/Notif/Agents:** each must honour the **restriction flag** (§4.6) — a cross-cutting new
  obligation (no indexing/agent-use/analytics/notification for a restricted subject) — and implement
  rectification fan-out via reindex-from-source.
- **Git P4 (the gated change):** **pseudonymous-by-default commit identities are a commit-time prerequisite**
  the Git data model must enforce (§7; GIT-1) — this *gates* the Git P4 data model.

### 10.3 Open questions for Phase 4 / Phase 5 / Legal

- **`[OPEN — LEGAL]` (GD-5):** the exact audit-log **retention carve-out** scope per jurisdiction (§6.4).
- **`[OPEN — LEGAL]` (GD-1/L-2):** the residual **Art. 17 reach into non-pseudonymised git-history**
  (commit-object bytes) — history-rewrite vs documented lawful-basis limit (§7.3).
- **`[OPEN — LEGAL]` (GD-13):** whether worklog/productivity data is **special-category** (Art. 9) — the
  `SpecialCategory` tag flags it; counsel decides the obligation.
- **`[OPEN — LEGAL]` (AG-9):** the EU-sovereign **real-LLM sub-processor** + the AI-Act classification of
  agent processing of personal data (`gdpr-eu-sovereignty.md` §7; design-safe minimums hold now).
- **`[OPEN — LEGAL]`:** DPO **ratification of the RoPA legal text** (generated rows are seeded from the data
  map; the legal characterisation is reviewed) and of the **fail-static `W`** (L-1).
- **`[OPEN → P4]` (control plane):** **multi-cell DSR cross-cell ordering/atomicity** (§4.4) — single-cell
  is designed; globally-atomic multi-cell erase is the floor.
- **`[OPEN → P4]`:** the **DSR tenant self-service UX** (Art. 28 assistance surface) + the breach-scoping +
  DPIA admin surfaces (design-language; data-map-driven).
- **`[OPEN → P5]`:** all drill thresholds (§9.2) — the N in the deadline-margin, the audit-tamper-detection
  coverage, the multi-cell completeness gate.

---

## 11. Cross-references

- Spine: **ADR-12** (PersonalDataHolder spine + the 9 commitments this doc realises), ADR-11 (cells/
  residency), ADR-13 (glue/no-cross-store-read), ADR-17 (fail-static residual window), ADR-18 (restore +
  post-restore re-erasure).
- Directives: **GD-1** (git-history reconciliation), **GD-2** (eDiscovery/legal-hold export + retention),
  **GD-3** (fail-static bound ≤ revocation SLA), X-1…X-5, BUS-2, AG-7.
- Doctrine: [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1
  (erasure vs immutability), §5.3 (reindex-from-source);
  [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) §1/§10/§11.
- Foundational Phase-3 docs this consumes: [`00-platform-substrate.md`](./00-platform-substrate.md)
  (holder auto-registration, classification lint, telemetry), [`identity-and-access.md`](./identity-and-access.md)
  (pseudonym lever, fail-static bound), [`storage.md`](./storage.md) (KMS/crypto-shred/GD-4, restore),
  [`event-bus.md`](./event-bus.md) (outbox-only audit append, `*.erased` tombstones, durable timer),
  [`search-and-indexing.md`](./search-and-indexing.md) + [`reference-graph.md`](./reference-graph.md)
  (purge+reindex / tombstone derivatives), [`tenancy-and-control-plane.md`](./tenancy-and-control-plane.md)
  (multi-cell `member_cells` fan-out, PII-free control plane).
- Research source: [`01-research/gdpr-eu-sovereignty.md`](../01-research/gdpr-eu-sovereignty.md) (the
  requirements this doc operationalises — §5 shared contracts, §6 hard conflicts, §9 checklist).
- Seeds Phase 4: every subsystem implements `PersonalDataHolder`; **Git** is gated by §7 (GIT-1); the
  DSR/breach/DPIA admin surfaces are P4 + design-language.
