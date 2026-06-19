# Phase 5 — GDPR / Audit Machinery (REFINED · canonical) — the PersonalDataHolder spine · DSR orchestrator · tamper-evident log · the ONE free-text/immutable erasure posture

> Phase: `05-refined-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md) (single
> source of truth, never contradicted). Binding doctrine:
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (erasure vs
> immutability) + §5.3 (reindex-from-source), [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) §1/§8/§10/§11.
> Reconciliation spine (binding): [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md)
> (X-1..X-7, OQ-A..OQ-L) + [`contract-index.md`](./contract-index.md) (the frozen build-to surface,
> **supersedes** the Phase-3 index). Phase-3 base this carries forward:
> [`../03-shared-systems-architecture/gdpr-and-audit.md`](../03-shared-systems-architecture/gdpr-and-audit.md).
> Spine: **ADR-12** (PersonalDataHolder spine), ADR-11 (cells/residency), ADR-13 (glue contracts), ADR-17
> (fail-static / GD-3), ADR-18 (restore-verification / post-restore re-erasure). Date: 2026-06-19.
>
> **What this doc is.** The REFINED, canonical GDPR/Audit shared-system architecture Phase 6/7 build on. The
> Phase-3 design is carried forward as the base; this doc **applies the reconciliation decisions and the
> change requests** targeting this system. Where a thing is unchanged from Phase 3 it is cited concisely, not
> re-derived. The contracts this system exposes are made explicit and final, matching the refined
> [`contract-index.md`](./contract-index.md) §10. **No ADR is reversed** (none was requested); the work is
> confirmation + additive sharpening + resolving the genuine seams.
>
> **This doc owns POLICY + ORCHESTRATION; Storage owns MECHANISM** (unchanged from Phase 3 §1.2): the
> crypto-shred *mechanism* (KMS hierarchy, per-subject vs per-tenant DEK granularity GD-4, backup/restore
> cross-seam) is [`storage.md`](./storage.md) §4–§7; the pseudonym-indirection *lever* + `erase(subject)` is
> Identity (contract 4.8). This doc decides *whether, when, and prove*; the owning store decides *how*.

---

## Changes vs Phase 3 (every change, listed)

The Phase-3 GDPR/Audit doc was already substantially DECIDED. Phase 5 changes are **additive sharpening +
one NEW policy artifact + four NEW sub-mechanisms**, none reversing an ADR.

1. **NEW — the ONE platform-wide free-text / immutable-content erasure posture (contract 10.9; X-7/OQ-G).**
   The single biggest change. The same legal seam named five times across Git/CI/Issues/Knowledge/Chat is
   now resolved as **one ratified posture, instantiated per subsystem by reference, never restated five
   times**. Subsumes and generalises the Phase-3 §7 Git-history-only reconciliation. `[OPEN — LEGAL]`. See §7.
2. **NEW — history-rewrite as an audited, tamper-evident, rate-limited tenant op with fork/mirror/clone-cache
   invalidation fan-out (contract 10.6; recon §9).** Phase 3 named history-rewrite as "the supported
   disruptive path"; Phase 5 makes it a **first-class audited op** with a defined invalidation surface (the
   Git erasure-admin tool), tied to the trust-scoped cache namespaces (Storage 11.2). See §6.6, §7.4.
3. **NEW — the outbound push-mirror residency gate (contract 10.5; recon §10).** An outbound mirror config
   targeting an extra-EU host for PII-bearing content is **denied by default** at the `transfer_allowed`
   gate. Generalises the Phase-3 sub-processor transfer gate to cover Git push-mirrors. See §5.3.
4. **SHARPEN — worklog / productivity / estimate field sensitivity classification (contract 10.2; OQ-H).**
   These fields are now tagged `category = behavioural, role = tenant-content, restricted by default`, with a
   defensible works-council / labour-law engineering posture. `[OPEN — LEGAL]`. Plus the two flagged-not-
   foreclosed items: build-data-as-LLM-training (foreclosed by default pending basis) and CD-as-PaaS scope
   (Commercial, not an engineering blocker). See §2.4.
5. **SHARPEN — per-subject DEK reaches CI log segments (contract 11.4; recon §8).** The GD-4 granularity rule
   now explicitly extends per-subject crypto-shred into CI free-text log PII (was per-tenant floor in Phase 3
   H2). The erasure mechanism table (§3.2) and the holder list (H2) are updated. Storage owns the mechanism;
   this doc records the policy.
6. **CONFIRM — `restrict` suppression flows into OLAP analytics (contract 11.6; GA-9).** Phase 3 already
   required Search/Refs/Notif/Agents to honour the restriction flag; Phase 5 confirms OLAP is in that set
   (no analytics for a restricted subject) and that worklog analytics-eligibility is governed by OQ-H.
7. **CONFIRM — the DSR fan-out iterates `member_cells` over the now-frozen cross-cell PII-free pointer bridge
   (contract 12.6 / 10.4; OQ-I).** The Phase-3 multi-cell floor (§4.4) is unchanged in substance; the bridge
   frame it rides is now frozen (`CrossCellPointer{subject, type, correlation_id, home_cell}`, cell-local
   resolution). Cross-cell ordering/atomicity remains the named control-plane floor.
8. **CONFIRM — audit-log actor minimisation aligns to the frozen pseudonym grammar `<pseudonym>@<tenant>.noreply`
   (contract 4.8).** The audit entry's `actor`/`on_behalf_of` are this pseudonym; identity erasure is the
   pseudonym-map shred, leaving the chain+tree intact (§6.4). Grammar pin only — no schema change.
9. **CONFIRM — every erasure receipt that names a destroyed key/cursor still seals into the audit Merkle
   tree (contract 10.6/10.7).** Unchanged; reaffirmed as the proof-not-promise spine.

**Unchanged from Phase 3 and carried forward verbatim in substance** (cited, not re-derived): the
`PersonalDataHolder` five-operation contract (§3.1), the exhaustive holder list H1–H18 (§3.2, two cells
updated), the schema-level `#[personal_data]` classification + generated data-map/RoPA (§2), the DSR state
machine + verifiable receipts (§4), the retention engine (tightest-policy-wins + legal-hold-aware) + consent
+ sub-processor registries + eDiscovery export (§5), the tamper-evident audit log construction (hash-chain +
Merkle + CT-style proofs + external witness; deliberately not a blockchain) (§6), the policy↔mechanism
boundary and the two legal postures (controller vs processor) (§1), the GA-1..GA-9 drills (§9).

---

## 0. Reading map

- **§1** — purpose, the policy↔mechanism boundary, the two legal postures. (CONFIRMED from Phase 3.)
- **§2** — data model: schema-level `data_role` + personal-data classification, generated data-map/RoPA, the
  stateful register; **+ the worklog/build-training/CD-PaaS classification (OQ-H, NEW).**
- **§3** — the `PersonalDataHolder` contract + the EXHAUSTIVE holder list (H1–H18; H2 + erasure-table
  updated for per-subject CI-log DEK).
- **§4** — the DSR orchestrator (fan-out · deadline · receipts · multi-cell over the frozen OQ-I bridge ·
  tenant-operable). CONFIRMED.
- **§5** — retention engine + consent + sub-processors + eDiscovery/legal-hold export; **+ the outbound
  push-mirror residency gate (NEW).**
- **§6** — the tamper-evident audit log; **+ history-rewrite as a first-class audited op (NEW, §6.6).**
- **§7** — **the ONE free-text / immutable-content erasure posture (X-7/OQ-G, NEW)** — the keystone change;
  the Phase-3 Git-history reconciliation is subsumed as one instance.
- **§8** — contracts exposed & consumed (the stable glue, final, matching contract-index §10).
- **§9** — scaling/sharding; failure modes + drills owed (GA-1..GA-9 CONFIRMED + GA-10/GA-11 NEW).
- **§10** — cited prior art; open questions remaining for Phase 6.

**Floors named up front** (VISION §3 / EI-04 §4): single-cell DSR fan-out + audit are **fully designed**;
multi-cell fan-out **iterates `member_cells`** over the frozen OQ-I bridge — the mechanism is named, the
cross-cell **ordering/atomicity** remains a control-plane floor (§4.3). The **third-party / immutable-byte
free-text PII residual** is **not crypto-shreddable by the subject's own key** — it is handled under the ONE
documented lawful-basis posture (§7), `[OPEN — LEGAL]`, with the structural guarantee (`restrict`: never
indexed / never agent-readable / never in analytics) shipping regardless. The **RoPA legal text**, the
**audit-retention carve-out per jurisdiction**, the **worklog special-category classification**, and the
**Art. 17 reach into immutable git bytes** are `[OPEN — LEGAL]` (counsel/DPO ratify; the structural floor
ships).

---

## 1. Purpose, responsibilities, and the policy↔mechanism boundary — CONFIRMED (Phase 3 §1)

Unchanged. The one-paragraph thesis (Phase 3 §1.1): GDPR-by-construction is a property of the *whole
structure*, not a settings page (ADR-12). This system is the connective tissue that makes the structural
guarantees **operable and provable** — one `PersonalDataHolder` contract every store implements, a DSR
orchestrator that fans out and tracks the statutory clock, a generated data map so the holder list cannot
silently drift, a retention engine, and a tamper-evident audit log that proves erasure happened. *If we
cannot enumerate where a person's data lives and prove we erased it, none of the rights pipelines is real.*

The ownership split (Phase 3 §1.2 table, unchanged) and **the boundary rule** are CONFIRMED: GDPR/Audit
decides *whether, when, and prove*; the owning store decides *how*; the orchestrator never reaches into a
store — it calls the holder contract (the no-cross-store-read law, ADR-01/ADR-13).

The **two legal postures** (Phase 3 §1.3, CONFIRMED) drive everything and are encoded as the schema-level
`data_role` tag (§2.1), not a runtime guess:
- **Processor** for **tenant content** (repos, issues, docs, chat, CI logs + embedded personal data of the
  customer's people). The customer org is the controller; a DSR is **answered by/for the tenant** (Art. 28);
  Myelin **must not unilaterally erase tenant content** except on tenant instruction or offboarding.
- **Controller** for **platform-operational** data (tenant-admin contacts, billing, the security audit log,
  product telemetry). Myelin is the **first-line DSR responder** here.

One classification (`tenant-content | platform-operational`), threaded identically through the bus envelope
(contract 2.1 `data_role`), the data map, and the DSR router — the X-5 names-and-units reconciliation point,
unchanged and binding.

---

## 2. The data model / schemas

### 2.1 Schema-level personal-data classification — CONFIRMED (Phase 3 §2.1)

Unchanged. Every personal-data-bearing field in every service carries a compile-time `#[personal_data(...)]`
classification (the `myelin-gdpr` derive), emitted into a **generated** registry so the data map cannot drift
(contract 10.2):

```rust
#[personal_data(
    category   = ContactInfo,   // ContactInfo | Identifier | Content | Behavioural | SpecialCategory(...)
    role       = TenantContent, // TenantContent (processor) | PlatformOperational (controller)
    basis      = Contract,      // Contract | LegitimateInterest(lia_ref) | Consent(consent_id) | LegalObligation
    retention  = TenantPolicy,  // TenantPolicy | Fixed(Duration) | UntilContractEnd | AuditCarveOut(Duration)
    erasure    = Pseudonymise,  // Pseudonymise | CryptoShred(key_class) | PurgeReindex | CarveOut
    subject_locator = "principal_id",
)]
email: EncryptedField<Email>,
```

The five tags answer the five questions every rights pipeline asks (category / role / basis / retention /
erasure); `subject_locator` makes `locate(subject)` structural. Enforcement is the **`no-untagged-personal-data`
lint** (contract 1.6): a personal-data-typed field without a tag **fails the build** — "we forgot a store/
field is a *structural* failure" (ADR-12.1; GA-6 drill).

### 2.2 The generated data map + RoPA — CONFIRMED (Phase 3 §2.2)

Unchanged (contract 10.3). A build step walks every schema + every registered holder and **generates** the
machine-readable inventory (what PII exists, where, role/basis/category, retention, locator, residency); it
is regenerated every build and **diffed in CI** so a DPO sees any reclassification. It is the substrate for
RoPA (Art. 30, a projection grouped by processing activity), erasure fan-out (Art. 17, the per-holder + per-
field-mechanism checklist), breach scoping (Arts. 33–34, the 72-hour enabler), and access (Art. 15).

### 2.3 The stateful register (G1–G8) — CONFIRMED (Phase 3 §2.3)

Unchanged. GDPR/Audit owns one Postgres-class DB per cell (residency-pinned, per-tenant envelope-encrypted, a
recursive holder with the audit carve-out): `dsr_request` (G1) + `dsr_receipt` (G2) = the DSR state machine;
`retention_policy` (G3) + `legal_hold` (G4); `consent` (G5, per-subject DEK); `subprocessor_registry` (G6);
`processing_activity` (G7, RoPA, generated-then-DPO-reviewed); the audit log (G8, §6). The DPIA gate (Art.
35) fires on a data-map diff introducing a new `SpecialCategory` flow, a new agent capability over personal
data, or large-scale systematic monitoring.

### 2.4 NEW — worklog/productivity sensitivity + build-training + CD-PaaS classification (OQ-H) — `[OPEN — LEGAL]`

This is the Phase-5 classification addition (recon §OQ-H, contract 10.2). We specify the **defensible
engineering posture and flag for counsel/DPO; we are not counsel.**

**Worklog / productivity / estimate fields** (Issues worklog, time-tracking, per-individual velocity inputs)
are tagged:

```rust
#[personal_data(
    category = Behavioural, role = TenantContent, basis = TBD_LEGAL,
    retention = TenantPolicy, erasure = CryptoShred(subject_dek),
    data_role_default = Restricted,            // restricted-by-default in cross-individual processing
)]
```

Engineering posture — treat them as **potentially works-council-consultable / elevated-sensitivity** in EU
jurisdictions:
1. **Excluded from cross-individual analytics and agent-use for a restricted subject by default** — the
   `restrict` suppression (§4.4) already covers this; OLAP honours the flag (contract 11.6, GA-9).
2. **Per-individual productivity rollups are OFF by default**, gated behind an explicit tenant-admin
   enablement that the posture **flags as requiring works-council consultation** in applicable jurisdictions
   (the platform surfaces the consultation trigger; it does not adjudicate it).
3. They carry the **same per-subject DEK crypto-shred** as other free-text PII (§3.2).

**Counsel must ratify** whether these are special-category (Art. 9) or merely elevated, and the per-
jurisdiction works-council consultation trigger. The `SpecialCategory` tag (Phase 3 §2.1) remains the
mechanical flag that routes such a field into the DPIA gate (§2.3) rather than letting it pass as ordinary
content.

**Build-data-as-LLM-training (AG-8):** **foreclosed by default** pending lawful basis. Tenant build data is
`role = tenant-content` (processor); training a model on it is a **new purpose** needing its own basis.
Engineering posture: **no platform code path feeds tenant content to model training**; the future real-LLM
adapter is a region-aware, EU-hostable sub-processor (ADR-12.8, AG-9) and training-on-tenant-data is a
**separately-ratified opt-in**, never a default. Flag for counsel.

**CD-as-PaaS scope (PR-5):** a **product/commercial** scope question, **not an engineering blocker** — the CI
sandbox + reserve/settle + residency primitives already support it. Flagged to Commercial, not foreclosed; no
GDPR/Audit action beyond classifying any new PaaS-tenant data when it ships.

---

## 3. The `PersonalDataHolder` contract + the EXHAUSTIVE holder list

### 3.1 The contract — CONFIRMED (Phase 3 §3.1)

Unchanged (contract 10.1). Every store and subsystem registers as a holder implementing five operations for a
subject (or a tenant, for offboarding); this is the **only** way the orchestrator touches a store:

```rust
pub trait PersonalDataHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> Result<LocateReport>;  // Art. 15
    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> Result<PortableBundle>; // Art. 20
    fn rectify(&self, subject: &SubjectRef, patch: Patch) -> Result<RectifyReceipt>;    // Art. 16
    fn restrict(&self, subject: &SubjectRef, on: bool) -> Result<RestrictReceipt>;      // Art. 18/21
    fn erase(&self, subject_or_tenant: EraseScope) -> Result<EraseReceipt>;             // Art. 17
}
```

CONFIRMED unchanged: the **harness auto-registers every store it opens** (contract 1.4); a store opened
outside the harness fails the `holder-registered` architecture test (the holder list cannot drift below the
data map). Each operation returns a **receipt hash-linked into the audit log** (§6) — completion is tamper-
evidently provable. `erase` is **purge / crypto-shred / pseudonymise, never hide** (ADR-12; storage §5.3):
Search purges+reindexes (incl. embeddings), Bus crypto-shreds inline-PII keys + emits `*.erased` tombstones,
Refs tombstones, OLTP free-text crypto-shreds the per-subject DEK, structured pseudonym-referenced rows rely
on Id's pseudonym-map delete (the delete-the-identity-not-the-fact split, EI-04 §1).

### 3.2 The exhaustive holder list (H1–H18) — CONFIRMED, two cells updated

The list is exhaustive and enforced by the data map. **Two updates vs Phase 3** are marked **[P5]**; all else
is unchanged.

| # | Holder | Personal data it holds | Erasure mechanism | Owner (impl) |
|---|---|---|---|---|
| H1 | Git subsystem DB | PR/review/comment authorship (pseudonym), free-text bodies | pseudonymise (Id lever) + crypto-shred inline bodies (per-subject DEK) | Git P4 |
| H2 **[P5]** | CI subsystem DB + **log segments** | run actors (pseudonym), log refs, **inline free-text PII in log lines** | pseudonymise + **per-subject DEK crypto-shred of isolable log-segment PII** (was per-tenant floor) + short-TTL log retention | CI P4 / Storage 11.4 |
| H3 | Issues subsystem DB | assignees/watchers/mentions (pseudonym), free-text fields, **worklog (restricted, §2.4)** | pseudonymise + crypto-shred free-text (per-subject DEK) | Issues P4 |
| H4 | Knowledge subsystem DB | page authorship (pseudonym), free-text content, db-row values | pseudonymise + crypto-shred content | Knowledge P4 |
| H5 | Chat subsystem DB | message authorship (pseudonym), message bodies | pseudonymise + crypto-shred bodies (per-subject DEK) | Chat P4 |
| H6 | Object/blob store | avatars, attachments, doc media, CI artifacts | crypto-shred (per-tenant/-subject DEK; immutable-tier → key destroy) | Storage §3 |
| H7 | Search index | plaintext-derived tokens + **embeddings** | **purge + reindex** (plaintext-derived, not key-shred) | Search §9 |
| H8 | Event-bus history | pseudonymous actor; rare inline-PII events | crypto-shred inline-PII keys + `*.erased` tombstones; references-not-payloads makes most events erasure-free | Bus §4.8 |
| H9 | Caches / CDN | derived copies, unfurl renders, **clone/bundle blob class (§5.3)** | TTL expiry + targeted purge on erase; **trust-scoped cache namespace invalidation on history-rewrite (§6.6)** | substrate / each service / Storage 11.2 |
| H10 | Backups / snapshots | ciphertext of all of the above | **crypto-shred by construction** (key destroyed ⇒ ciphertext unrecoverable) + bounded retention + **post-restore re-erasure** | Storage §7 (ADR-18) |
| H11 | Agent memory / embeddings | retrieved context, derived embeddings, RAG state | crypto-shred per-subject DEK + purge embeddings (they re-identify) | Agent Fabric §11 |
| H12 | Reference graph | edges referencing the subject; unfurl projections | tombstone (relies on pseudonym shred); backlinks are projections, rebuilt | Refs §4 |
| H13 | Notification history | recipient + actor pseudonyms, humanised strings | crypto-shred inline-PII + purge read-models (reindex-from-source) | Notif (NOTIF-3) |
| H14 | Authz tuples | `…@subject` tuples referencing the subject (incl. the **authz reverse index**, OQ-E) | delete the subject's tuples + pseudonym shred; the reverse index rebuilds off the bus | Id §6 |
| H15 | Identity (Principal/Auth DB + pseudonym map) | the **erasable profile** + the **pseudonym↔real-identity map** (the erasure lever) | delete pseudonym map (S2) + crypto-shred per-subject profile DEK | Id §11 |
| H16 | Audit log (carve-out) | who-did-what (minimised: IDs/pseudonyms, not payloads) | **carve-out** — retain what's lawfully needed, then expire via audit-key crypto-shred | this doc §6.4 |
| H17 | Agent execution trace (AG-7) | a content-addressed Knowledge doc of a run's trace | crypto-shred (distinct from audit; §6.5) | Agent Fabric / Knowledge |
| H18 | GDPR/Audit own stores (G1–G7) | DSR subjects, consent records, RoPA | crypto-shred per-tenant/-subject DEK (consent G5 = per-subject) | this doc |

H16 is **carved out, not exempt** (§6.4); the carve-out scope per jurisdiction is `[OPEN — LEGAL]` (GD-5).
H17 (agent trace) is deliberately **distinct** from the audit log (AG-7): trace = the run's reasoning record;
audit = the complete tamper-evident who-did-what. Keeping the three telemetry/trace/audit holders distinct
means none weakens another.

---

## 4. The DSR orchestrator (fan-out · deadline · receipts · multi-cell · tenant-operable) — CONFIRMED (Phase 3 §4)

The orchestrator (contract 10.4) is unchanged in substance. It receives a rights request, resolves the
subject (possibly across cells), **fans out to all holders via the §3 contract**, tracks completion against
the statutory deadline, produces a verifiable receipt, and logs every step to the tamper-evident audit log.
It is operable by **Myelin** (controller data) and **by/for tenants** (Art. 28 assistance).

### 4.1 The state machine + algorithm — CONFIRMED

Unchanged (Phase 3 §4.2):

```
received → validated → fanned-out → {awaiting-holders} → verified → completed
   └ deadline = now + 1 month (Art. 12(3)); extendable to 3 for complex (recorded reason)
```

1. **Validate + decide posture** (controller vs processor). Refuse a Myelin-initiated erase of *tenant
   content* absent tenant instruction or offboarding.
2. **Resolve scope from the data map** → a per-holder checklist with the per-field erasure mechanism. *The
   map, not a hand-written list, drives fan-out* — "we forgot the search index" is impossible.
3. **Legal-hold gate**: an active `legal_hold` (G4) suspends erasure + retention-expiry for the held scope
   (Art. 17(3)(e)); the request is recorded *partially deferred*. Access/portability still proceed.
4. **Fan out** through the holder contract — each call **idempotent + resumable** (the durable checklist is
   the state; a crashed orchestrator re-drives only un-receipted holders). The canonical **erase order**
   (storage §5.3, unchanged): **Id.erase (pseudonym map first)** → KMS.destroy per-subject DEK →
   Search.purge+reindex → Refs.tombstone → Bus.erase → notif/authz/agent-memory → record receipt. Erasing
   the pseudonym map first means every downstream holder already sees only the opaque pseudonym.
5. **Collect + verify receipts**; seal a DSR completion receipt (G2) hash-linked into the audit log — the
   proof an Art. 28 audit or supervisory authority can verify.
6. **Track the deadline** on a **durable timer** (contract 9.3, the same `myelin-flow` timer wheel as SLA
   timers and Trigger `stale_after` — we do not reinvent durable timers); a nearing-deadline emits a warning
   `Signal`.

### 4.2 Verifiable receipts — CONFIRMED

Unchanged (Phase 3 §4.3). A receipt is content-addressed + signed:
`receipt = sign( hash(request_id ∥ holder ∥ scope ∥ outcome ∥ key_epoch_destroyed? ∥ timestamp) )`, appended
to the per-tenant audit Merkle tree (§6). Recording the **key epoch destroyed** (crypto-shred holders) and
the **purge+reindex cursor** (Search) makes "we erased it" **independently checkable** against the KMS key-
destruction log and the index state — not asserted. The receipt bundle is exportable as a tamper-evident DSR
completion certificate (ties to eDiscovery, §5.4).

### 4.3 Multi-cell fan-out over the frozen OQ-I bridge — CONFIRMED, bridge frame pinned

Unchanged in substance (Phase 3 §4.4); the bridge it rides is now **frozen** (contract 12.6, OQ-I). For a
multi-cell tenant the orchestrator **iterates `member_cells`** (contract 10.4: `tenant_placement.member_cells
∪ home_cell`, all same-region) and fans out to each cell's holders, then merges per-cell receipts into one
certificate. The cross-cell carrier is the **PII-free pointer bridge**:

```
CrossCellPointer { subject: OpaqueSubjectId, type: ArtifactType, correlation_id, home_cell }
```

**Resolution is always cell-local** (OQ-I): a cell never reads another cell's personal data; each cell
**erases its own holders** and returns only a **receipt (PII-free)** to the merge. Residency is preserved by
construction (EI-02 §1; ADR-11 no-cross-region-PII). **Single-cell fan-out is fully designed; the cross-cell
ordering/atomicity** (a globally-atomic multi-cell erase, vs the resumable-per-cell checklist) **remains the
control-plane floor** — the orchestrator runs in each cell; the control plane sequences the wave, never
holding personal data (Tenancy §3.1). Named follow-on owner: P6 control-plane + multi-cell tenancy.

### 4.4 Tenant-operability + the non-erasure rights — CONFIRMED

Unchanged (Phase 3 §4.5–§4.6). The orchestrator is exposed **to tenants** for *their* data subjects (Art. 28
assistance). **Tenant offboarding** = a tenant-scoped erase: full export bundle + **tenant-granularity
crypto-shred** (destroy the tenant KEK ⇒ every DEK unwrappable ⇒ whole tenant unrecoverable, backups
included) + a sealed offboarding certificate — just `erase(EraseScope::Tenant)` over the holder list.
**Restriction (Art. 18/21):** `restrict(subject, on)` sets a per-subject suppression flag every holder
honours — no indexing, no agent-use, no analytics (incl. **OLAP**, contract 11.6), no notification — while
retaining storage; reversible. **Rectification (Art. 16):** corrects the primary store + fans out to
derivatives via **reindex-from-source** (EI-04 §5.3), never patched-in-place-and-drift. **Portability (Art.
20):** `export` returns subject-provided data structured (JSON/CSV; git via clone; docs as Markdown).

### 4.5 Fail-static window interaction (GD-3) — CONFIRMED, DPO ratifies the value

Unchanged (Phase 3 §4.7; contract 4.11). The fail-static staleness bound `W` (Id, ADR-17) is the residual
GDPR-revocation exposure window: a just-disabled actor may be served a stale "active" answer for ≤ `W`.
**GDPR/Audit owns the *constraint*** (`W ≤ deprovision/revocation SLA`, and `W` must contain the agent-token
TTL); **the DPO ratifies the *value*** (L-1, proposed `W = 5 min`). The value is recorded as a named, dated,
DPO-ratified residual in the RoPA (it is a processing characteristic) — written down, not silent.

---

## 5. Retention engine · consent · sub-processors · eDiscovery — CONFIRMED + one NEW gate

### 5.1 Retention engine — CONFIRMED (Phase 3 §5.1)

Unchanged (contract 10.5). **Tightest-policy-wins merge**: effective retention = the most restrictive that
does not violate a legal-retention floor (a tenant's "delete chat after 30 days" beats a 90-day platform
default; a lawful 6-month security-log floor beats a tenant's "delete logs immediately"). The merge is
deterministic and recorded (auditable which input won). **Legal-hold-aware (suspend, don't delete):** an
active hold suspends both retention-expiry and erasure for the held scope (Art. 17(3)(e)); on hold-lift the
deferred deletion resumes. Expiry uses the same erasure mechanisms (§3).

### 5.2 Consent + sub-processor registries — CONFIRMED (Phase 3 §5.2–§5.3)

Unchanged. **Consent (G5):** versioned, timestamped, granular, withdrawable, per-subject-keyed (own DEK) for
controller-posture activities; withdrawal propagates (stops the path, may trigger deletion). **Sub-processors
(G6):** versioned public + per-tenant list with region, DPA reference, change-notification + objection
workflow. Sovereignty stance: **no personal data leaves the EU/EEA by default; transfers are off and gated**
at the adapter seam (`transfer_allowed` denies extra-EU by default; the future real-LLM backend is one such
gated, EU-preferring, swappable adapter, AG-9 `[OPEN — LEGAL]`).

### 5.3 NEW — the outbound push-mirror residency gate (recon §10, contract 10.5)

A Git **push-mirror** (or any outbound replication config) that targets a foreign host is a **residency
boundary crossing** for any PII-bearing content it carries. Phase 5 makes this a first-class gate:

- A mirror config whose target region is extra-EU for PII-bearing content is **denied by default** at the
  same `transfer_allowed` gate that governs sub-processors (contract 10.5). The mirror is permitted only
  with a valid transfer mechanism + a recorded Transfer Impact Assessment + tenant transparency — the same
  discipline as any other extra-EU transfer.
- This extends the Phase-3 transfer gate (which covered sub-processor adapters) to **cover the Git mirror
  seam** specifically (recon §10). The control plane is where the cross-region check lands; this doc owns the
  *policy* the gate reads (the `subprocessor_registry` / `transfer_allowed` truth), Tenancy owns enforcement.
- **Within-EU CDN clone/bundle distribution is permitted** (Storage 11.2 NEW blob class): clone bundles are
  content-addressed, region-pinned to the tenant's region; **no extra-EU edge serves PII**. The gate
  distinguishes within-EU acceleration (allowed) from extra-EU replication (denied by default).

### 5.4 eDiscovery / legal-hold export — CONFIRMED (Phase 3 §5.4)

Unchanged (contract 10.7). A subject-, tenant-, or matter-scoped bundle of records + audit-log proofs (§6)
establishing chain-of-custody; **content-addressed and Merkle-proof-bearing** (each record carries its
inclusion proof against the per-tenant audit tree), so a recipient can *verify* the bundle was not altered. A
legal-hold freezes the scope while the export is assembled. The same tamper-evident substrate serves both
"prove we erased it" (DSR receipt) and "prove this is the unaltered record" (eDiscovery).

---

## 6. The tamper-evident audit log — CONFIRMED + history-rewrite as a NEW audited op

### 6.1–6.3 Construction + proofs — CONFIRMED (Phase 3 §6.1–§6.3)

Unchanged (contract 10.6). **One tamper-evident, append-only audit log records every human *and* agent
action** (agents flow through the same audit path as humans, EI-02 §2). Construction: a **per-tenant hash-
chain whose entries are also leaves of a Merkle tree**, with **CT-style inclusion and consistency proofs**
(RFC 6962; Crosby & Wallach 2009; Haber & Stornetta 1991; Merkle 1987). Why both: the hash-chain gives cheap
append-only tamper-evidence; the Merkle tree gives `O(log n)` inclusion proofs ("this action is in the log")
and consistency proofs ("the log wasn't forked/rewritten between two signed tree heads"). We adopt the
**Trillian/CT model** (signed tree heads, self-hosted in-cell) and **deliberately do NOT use a blockchain**
(global byzantine consensus we don't need + a residency problem replicating the log off-cell). The signed
tree head is periodically anchored to an **independent witness** (RFC-3161 TSA / a different cell's notary),
so even a fully-compromised cell cannot rewrite history undetectably (the witness sees only an opaque root
hash — no personal data crosses, residency-safe).

The `audit_entry` + `audit_sth` schema (Phase 3 §6.2) is unchanged. **Minimised by design**: `actor` /
`on_behalf_of` / `subject` are **pseudonymous IDs / `ArtifactRef`s, never payloads** — and `actor` now uses
the **frozen pseudonym grammar `<pseudonym>@<tenant>.noreply`** (contract 4.8; pin only, no schema change).
Written **via the outbox only** (BUS-2, contract 2.2): the audit consumer is an infra subscription; no
service writes the audit log directly — coverage is a property of the bus. Causality-carried
(`correlation_id` / `causation_id`): the audit log *is* the "why did this happen" walk — one mechanism for
audit + provenance + the loop guard (EI-02 §6).

### 6.4 The audit carve-out (H16) — CONFIRMED, `[OPEN — LEGAL]` (Phase 3 §6.4)

Unchanged. Audit logs contain personal data yet must persist to prove compliance/security. Kept under a
**distinct legitimate-interest basis** with a defined retention, **minimised** (IDs/pseudonyms), **carved out
of erasure to the extent law permits**: when a subject is erased, Id's pseudonym shred already ran, so the
audit log retains only the opaque-pseudonym minimised record of what was needed to evidence compliance /
defend claims, then **expires via crypto-shred of the audit key** at retention end. The exact carve-out scope
per jurisdiction is **`[OPEN — LEGAL]` (GD-5)**. The carve-out does **not** weaken tamper-evidence: we never
rewrite an audit entry (that would break the chain); the real identity was never in the entry — it lived in
Id's erasable pseudonym map.

### 6.5 Distinct from agent trace + telemetry — CONFIRMED (Phase 3 §6.5)

Unchanged. Three separate holders kept separate on purpose: **telemetry** (operational, sampled, RED/USE),
**agent execution trace** (AG-7, H17 — a content-addressed Knowledge doc of one run's reasoning, erasable),
**audit log** (H16 — the complete tamper-evident who-did-what). An agent's *applied effect* lands in the
audit log (like a human's action); its *reasoning* lands in its trace.

### 6.6 NEW — history-rewrite as a first-class audited op (recon §9, contract 10.6)

Phase 3 named history-rewrite (filter-repo-class) as "the supported disruptive path" for residual PII in
immutable content. Phase 5 makes it a **first-class, audited, tamper-evident, rate-limited tenant op** — the
**Git erasure-admin tool** — with a defined invalidation surface:

- **Audited.** Every history-rewrite is an action in the tamper-evident audit log (kind
  `git.history_rewrite`, actor = the tenant-admin pseudonym, subject = the repo `ArtifactRef`, outcome
  recorded), so a hash-changing erasure is itself provable and accountable.
- **Rate-limited.** It is a tenant-initiated op, not an automated one; rate-limited to prevent it being used
  as a denial/disruption vector (it changes every downstream hash).
- **Invalidation fan-out (the NEW surface).** A rewrite emits an invalidation fan-out to **forks, mirrors,
  and the clone-cache** — tied to the **trust-tier / branch-scoped cache namespaces** (Storage 11.2, X-1):
  an `UntrustedFork`-written cache scope cannot poison the trusted scope, and a rewrite purges the stale
  content-addressed clone/bundle blobs (the within-EU CDN class, §5.3) so a rewritten history is not served
  from a cache. The outbound-mirror gate (§5.3) plus this invalidation surface together mean a rewrite
  reaches the replicas it can reach, and the residual (independent off-platform clones a third party holds)
  is named, not pretended-solved.
- **Crypto-shred reaches the pack tier's shreddables** (reflogs, bitmaps, pack backups) via the per-tenant
  blob DEK (Storage §5.4); it does **not** reach the commit-object bytes themselves — that is what the
  rewrite (changed hashes) is for. This is the honest split.

This op is the concrete mechanism behind the residual posture in §7.4. It is a resumable `myelin-flow`
activity (contract 9.2), idempotent, emitted via outbox so derived stores resubscribe.

---

## 7. The ONE free-text / immutable-content erasure posture (X-7 / OQ-G) — NEW policy artifact · `[OPEN — LEGAL]`

This is the keystone Phase-5 change (contract 10.9). The same legal seam was named **five times** — Git
(immutable commit bytes), CI (inline log PII), Issues (third-party free-text mentions), Knowledge (free-text
blocks), Chat (a name typed into another user's un-erased message body). Phase 3 wrote up only the Git-history
instance (Phase-3 §7); Phase 5 **generalises it to ONE platform-wide posture, instantiated per subsystem by
reference, never restated five times** — the named "Erasure vs. Immutability reconciliation" deliverable
(GD-1, L-2). The Phase-3 §7 Git reconciliation is now **one instance of this one posture** (§7.4).

### 7.1 The structural floor (built now, no legal dependency)

For **all** free-text and immutable content the engineering guarantee is the same and is **fully built**:

1. **Per-subject DEK crypto-shred (the lever).** Free-text / body / op-log / agent-trace columns — and, new
   in P5, **CI log segments** (§3.2 H2) — are encrypted with a **per-subject DEK** (contract 11.4, GD-4). A
   subject's erasure destroys their DEK; their **self-authored** content in DBs, **backups, and immutable
   logs** becomes unrecoverable ciphertext. This is the primary erasure mechanism for *their own* content
   (their messages, comments, blocks, worklog).
2. **Pseudonym-map shred (identity erasure).** Author/subject identity in immutable structures is a **stable
   opaque pseudonym** (`<pseudonym>@<tenant>.noreply`, frozen, contract 4.8); the person↔pseudonym map is the
   erasable record (contract 4.8 `resolve_pseudonym`/`erase`) — DSR fan-out **step 1**. Erasing the map means
   the immutable bytes (commit author, event actor, audit entry) hold only a pseudonym. This is the answer
   for **Git commit-author metadata**: commits are **pseudonymous-by-default** (GIT-1, a commit-time
   prerequisite) so the immutable hash never bakes in erasable PII in the first place.
3. **Structural holder coverage.** Every store auto-registers as a `PersonalDataHolder` (contract 1.4 / 10.1);
   `restrict` suppresses indexing / agent-use / analytics / notification for a subject pending erasure. "We
   forgot a store" is structurally impossible (the `no-untagged-personal-data` lint + harness auto-
   registration).

### 7.2 The residual (the part the floor does NOT erase — for counsel)

The residual is **third-party free-text PII**: a person's name/email **typed by someone else** into that
other person's content (a Chat message body, an issue comment, a doc block, a CI log line, a commit message
written by a different author). This content is encrypted under the **author's** DEK, not the subject's, so
the subject's erasure does not crypto-shred it (shredding the author's DEK would destroy the author's
legitimate content). The same residual exists in **immutable commit-message bodies authored by others**.

### 7.3 The ratified engineering posture (defensible; FLAG FOR COUNSEL)

`[OPEN — LEGAL]` — a defensible engineering posture pending DPO/counsel ratification (L-2); we are not
counsel:

- **Primary basis.** Structured PII and self-authored free-text erase **reliably** via per-subject DEK shred
  + pseudonym-map shred. This covers the overwhelming majority and is the GDPR-compliant default.
- **Residual posture.** Third-party free-text mentions and immutable-byte content authored by others are
  handled under a **documented lawful-basis limit** — best-effort on-request redaction (a targeted `rectify`
  / tombstone of the specific span where the subject identifies it), **plus the standing structural guarantee
  that the residual is never indexed, never agent-readable, never in analytics for a restricted subject** (the
  `restrict` suppression). For git history specifically: (a) the **pseudonymous-by-default floor** covers
  author identity, and (b) the **history-rewrite erasure path** (§6.6 — audited, tamper-evident, rate-limited,
  with fork/mirror/clone-cache invalidation fan-out) covers the rare case where a body must be expunged, with
  the understood disruptive consequence of changed hashes (EI-04 §1).
- **What counsel must ratify (one statement, not five):** the lawful basis and documented limit for residual
  third-party / immutable free-text PII; the Art. 17 reach into immutable git bytes; the history-rewrite-vs-
  documented-limit choice; the audit-log retention carve-out (GD-5); and the worklog-sensitivity
  classification (OQ-H / §2.4). The **DPO ratifies; the structural floor ships regardless.**

### 7.4 Instantiation per subsystem — BY REFERENCE (no restatement)

This is ONE posture. Each subsystem doc, when it reaches its erasure section, **says only**: *"free-text /
immutable-content erasure follows the platform posture in `00-reconciliation-decisions.md §X-7` /
`gdpr-and-audit.md §7`; self-authored content crypto-shreds via per-subject DEK; identity via pseudonym-map
shred; the third-party / immutable residual is handled under the documented lawful-basis limit + `restrict`
suppression."* The Phase-3 Git-history reconciliation (Phase-3 §7: pseudonymous-by-default commit identities
as a commit-time prerequisite GIT-1; history-rewrite as the disruptive path; the residual limit
`[OPEN — LEGAL]`) is now **exactly the Git instance** of this one posture — DECIDED for the structural floor,
`[OPEN — LEGAL]` for the residual, with §6.6 as the concrete history-rewrite mechanism. No subsystem doc
restates the posture; each references it.

---

## 8. Contracts exposed & consumed (the stable glue) — final, matches contract-index §10

### 8.1 Exposed (what other systems + tenants consume) — FINAL

| Contract | Index # | Signature (illustrative) | Status |
|---|---|---|---|
| **`PersonalDataHolder`** | 10.1 | `{locate, export, rectify, restrict, erase}(subject\|tenant) → Receipt` | CONFIRMED |
| **classify** | 10.2 | `#[personal_data(category, role, basis, retention, erasure, subject_locator)]` derive; **+ worklog `behavioural`/restricted tags (OQ-H)** | SHARPENED |
| **data map / RoPA** | 10.3 | `data_map() → Inventory`; `ropa(tenant) → ProcessingActivities` (generated, CI-diffed) | CONFIRMED |
| **DSR submit / status / certificate** | 10.4 | `dsr_submit(kind, subject, scope, posture) → dsr_id`; `dsr_status → {state, deadline, checklist}`; `dsr_certificate → MerkleProvenBundle`; **iterates `member_cells` (OQ-I bridge)** | CONFIRMED |
| **retention / legal-hold** | 10.5 | `effective_retention(category, tenant, store) → Policy` (tightest-wins); `legal_hold_set(scope, on)` | CONFIRMED |
| **consent** | 10.5 | `consent_record/withdraw(subject, activity, version)` | CONFIRMED |
| **sub-processor / transfer gate** | 10.5 | `subprocessors(tenant) → list`; `transfer_allowed(target_region) → bool` (deny extra-EU by default); **+ gates the outbound Git push-mirror (NEW, §5.3)** | SHARPENED |
| **tamper-evident audit log** | 10.6 | append via outbox only; per-tenant hash-chain + Merkle; **+ `git.history_rewrite` as an audited op with invalidation fan-out (NEW, §6.6)** | SHARPENED |
| **audit proofs** | 10.6 | `inclusion_proof(action) → MerklePath`; `consistency_proof(t1,t2) → Proof`; `signed_tree_head(tenant) → STH` | CONFIRMED |
| **eDiscovery export** | 10.7 | `ediscovery_export(scope) → MerkleProvenBundle` (legal-hold-frozen) | CONFIRMED |
| **erasure ledger** | 10.8 | PII-free opaque-subject + shredded-holders/keys record; drives post-restore re-erasure (GD-14) | CONFIRMED |
| **the ONE free-text/immutable erasure posture** | 10.9 | the structural floor + documented residual limit; instantiated per subsystem by reference | **NEW**, `[OPEN — LEGAL]` |
| **telemetry signals** | 1.8 | `dsr_deadline_margin`, `erasure_fanout_coverage`, `audit_append_lag`, `sth_publish_age`, `legal_hold_active_count` | CONFIRMED |

### 8.2 Consumed (what this system depends on) — FINAL

| Consumed | From (contract) | Used for |
|---|---|---|
| `erase(subject)` + `resolve_pseudonym` + frozen pseudonym grammar `<pseudonym>@<tenant>.noreply` | **Id** 4.8 | the erasure lever; pseudonym-map delete = fan-out step 1; audit actor minimisation |
| KMS `destroy(key)` + hierarchy + GD-4 granularity (**incl. per-subject CI-log DEK, P5**) | **Storage** 11.3/11.4 | crypto-shred mechanism; receipts record the destroyed key epoch |
| Backup/restore cross-seam + **post-restore re-erasure** | **Storage** 11.5 (ADR-18) | erasure reaches backups by construction; restore must not resurrect |
| Trust-tier/branch-scoped cache namespaces + within-EU CDN clone/bundle class | **Storage** 11.2 | history-rewrite invalidation fan-out (§6.6); the outbound-mirror gate's within-EU exception |
| Bus `erase` (tombstone + inline-PII key-shred) + outbox emit + consumer template | **Bus** 2.2/2.7 | bus is a holder; audit appended via outbox; `*.erased` tombstones |
| Search `purge+reindex` (incl. embeddings); Refs `tombstone`; reindex-from-source | **Search** 6.4 / **Refs** 5.8 / **Bus** 2.6 | per-derivative erasure + rectification fan-out |
| `restrict` suppression honoured by Search/Refs/Notif/Agents/**OLAP** | **all** (10.1 / 11.6) | no indexing/agent-use/analytics/notification for a restricted subject |
| `member_cells` placement + the PII-free cross-cell bridge (`CrossCellPointer`) | **Tenancy** 12.3/12.6 | multi-cell DSR fan-out, cell-local resolution (OQ-I) |
| outbound-mirror residency enforcement at the control plane | **Tenancy** 12.x (gate reads this doc's `transfer_allowed`) | the §5.3 push-mirror gate |
| durable timer / signal | **Workflow** 9.3/9.4 | the 1-month deadline timer + nearing-deadline Signal; history-rewrite as a resumable activity |
| fail-static staleness bound `W` (DPO-ratified) | **Id** 4.11 (GD-3) | the residual revocation-exposure window recorded in RoPA |
| agent execution trace as a content-addressed holder (AG-7) | **Agent/Knowledge** 8.8 | H17; kept distinct from the audit log |

---

## 9. Scaling/sharding · failure modes + drills owed

### 9.1 Scaling — CONFIRMED (Phase 3 §9.1)

Unchanged. In-cell, tenant-partitioned: GDPR/Audit stores are `(tenant, region)`-keyed; the audit Merkle
tree is **per-tenant** (proofs + crypto-shred tenant-scoped; offboarding shreds one tenant's tree). Audit
append rides the same firehose + outbox the bus scales (`O(1)` amortised append; `O(log n)` proofs); a
column-store/time-series seam (BUS-6) only **when measured**. DSR fan-out is bounded + resumable (the
checklist is durable state) and not latency-critical (a 1-month deadline). The **authz reverse index** (H14,
OQ-E) is itself a holder and rebuilds off the bus — erasing a subject's tuples removes them from the index.

### 9.2 Failure modes + drills owed (PROVE-IT; T-5) — CONFIRMED + 2 NEW

The GA-1..GA-9 drills (Phase 3 §9.2) are CONFIRMED unchanged (erasure-reaches-every-holder; erasure-reaches-
search; crypto-shred-reaches-backups; audit-tamper-detection; dsr-deadline; data-map-drift; legal-hold;
multi-cell-erasure; restriction-leak). **Two NEW drills** for the Phase-5 mechanisms:

| # | Property / failure mode | Drill (quantified gate) | Owner | Source |
|---|---|---|---|---|
| GA-10 | **History-rewrite leaves stale PII in a cache/mirror/fork** | **history-rewrite-invalidation**: run a `git.history_rewrite` expunging a content span; assert the invalidation fan-out reached forks/mirrors/clone-cache, the trust-scoped cache namespaces purged the stale content-addressed blobs, and the op is in the audit log. Gate: **0 stale-PII cache/clone hits; op audited.** | this doc §6.6 + Storage 11.2 + Git | recon §9 |
| GA-11 | **An extra-EU mirror/transfer slips through** | **outbound-residency-gate**: configure a PII-bearing push-mirror to an extra-EU host; assert `transfer_allowed` **denies by default** and a within-EU CDN clone is **allowed**. Gate: **0 default extra-EU PII transfers; within-EU clone permitted.** | this doc §5.3 + Tenancy | recon §10 |

Each drill emits a **green artifact** when it passes; until then the property is **claimed, not proven** (T-4).
The §8.1 telemetry signals are the assertions the drills read.

### 9.3 Stateful-component register + blast-radius — CONFIRMED (Phase 3 §9.3)

Unchanged. `dsr_request`/`dsr_receipt` (resumable checklist — no loss). Retention/consent/sub-processor/RoPA
(expiry pauses → over-retains briefly, **fails safe toward retention, not deletion**). **Audit log + STH**
(append stalls → outbox buffers, no loss; tamper still detectable via the witnessed STH). **Legal-hold**
(unreachable store **fails safe to suspend** — never auto-deletes held data). Everything else (orchestrator
workers, data-map generator, audit consumer) is stateless and replaceable, recovered by replaying the durable
checklist + the bus.

---

## 10. Cited prior art · open questions remaining for Phase 6

### 10.1 Cited prior art — CONFIRMED (Phase 3 §10.1)

Unchanged. **Tamper-evident / verifiable logs:** Haber & Stornetta (*How to Time-Stamp a Digital Document*, J.
Cryptology 1991, the linked-timestamp hash-chain); Merkle (*A Digital Signature Based on a Conventional
Encryption Function*, CRYPTO 1987, the hash tree); Crosby & Wallach (*Efficient Data Structures for Tamper-
Evident Logging*, USENIX Security 2009, the history tree + efficient inclusion/consistency proofs — the model
adopted); Certificate Transparency (RFC 6962, append-only Merkle log + signed tree heads + witness/auditor
split); Trillian (the production verifiable-log class); deliberate non-adoption of blockchain. **Crypto-shred
/ erasure:** NIST SP 800-88r1 ("cryptographic erase"); Boneh & Lipton (*A Revocable Backup System*, USENIX
Security 1996); Kleppmann *DDIA* ch. 5 (tombstones/pseudonymisation — delete the identity, not the fact).
**GDPR engineering:** Arts. 5/6/9/12–22/17(3)/28/30/33–34/35/44–49; Schrems II (CJEU 2020) motivating no-
transfer-by-default; the platform's own `gdpr-eu-sovereignty.md` (the requirements this doc operationalises).
**Doctrine:** EI-04 §1 (erasure vs immutability — the event-log half "workable", the git-history half the
hard residual), §5.3 (reindex-from-source); EI-02 §1/§8/§10/§11.

### 10.2 Open questions remaining for Phase 6 (honesty register)

**`[OPEN — LEGAL]`** (flagged to counsel/DPO; the structural floor ships regardless):
- **L-2 / GD-1 — the ONE free-text/immutable-content erasure residual posture (§7).** Counsel/DPO ratify the
  lawful basis + documented limit for third-party / immutable-byte free-text PII, in **one statement** (not
  five). Includes the Art. 17 reach into immutable git commit-object bytes (history-rewrite vs documented
  limit). The structural floor (per-subject DEK + pseudonym shred + `restrict`) ships now.
- **GD-5 — the audit-log retention carve-out scope per jurisdiction (§6.4).** Counsel decides how long, what
  minimised fields, under what basis.
- **OQ-H / GD-13 — worklog/productivity special-category classification + works-council consultation trigger
  per jurisdiction (§2.4).** The `SpecialCategory` tag flags it; counsel decides the obligation; the works-
  council consultation trigger is surfaced by the platform, not adjudicated by it.
- **OQ-H / AG-8 — build-data-as-LLM-training lawful basis (§2.4).** Foreclosed by default; no code path feeds
  tenant content to training; a separately-ratified opt-in is the only path. Flag for counsel.
- **L-1 — the fail-static staleness bound `W` value (§4.5).** DPO ratifies the proposed `W = 5 min`; recorded
  in the RoPA.
- **AG-9 — the EU-sovereign real-LLM sub-processor + AI-Act classification of agent processing of personal
  data.** Design-safe minimums hold now (the adapter is region-aware, EU-preferring, transfer-gated).
- **RoPA legal text** — generated rows seeded from the data map; the DPO reviews + ratifies the legal
  characterisation.

**`[OPEN → P6]`** (engineering, not legal):
- **Multi-cell DSR cross-cell ordering/atomicity (§4.3).** Single-cell is fully designed; a globally-atomic
  multi-cell erase (vs the resumable-per-cell checklist) is the named control-plane floor. Owner: P6 control-
  plane + multi-cell tenancy.
- **The DSR tenant self-service UX (Art. 28 assistance surface) + breach-scoping + DPIA admin surfaces** —
  design-language, data-map-driven.
- **All drill thresholds (§9.2, GA-1..GA-11)** — the N in the deadline-margin, the audit-tamper-detection
  coverage, the multi-cell completeness gate, the history-rewrite-invalidation completeness, the outbound-
  residency-gate coverage. Phase 6/7 set the quantified gates the drills assert.

### 10.3 Cross-references
- Reconciliation spine: [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) (X-7/OQ-G the
  erasure posture, OQ-H worklog, §9 history-rewrite, §10 outbound-mirror gate, OQ-I cross-cell bridge);
  [`contract-index.md`](./contract-index.md) §10 (the frozen exposed surface this matches).
- Phase-3 base (carried forward): [`../03-shared-systems-architecture/gdpr-and-audit.md`](../03-shared-systems-architecture/gdpr-and-audit.md).
- Spine: **ADR-12** (PersonalDataHolder), ADR-11 (cells/residency), ADR-13 (glue), ADR-17 (fail-static),
  ADR-18 (restore + post-restore re-erasure); directives GD-1/GD-2/GD-3, X-5, BUS-2, AG-7.
- Doctrine: [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1/§5.3;
  [`../../external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) §1/§8/§10/§11.
- Consumed Phase-5 docs: identity-and-access (pseudonym lever, fail-static `W`, authz reverse index), storage
  (KMS/crypto-shred/GD-4 incl. per-subject CI-log DEK, restore, trust-scoped cache, CDN class), event-bus
  (outbox-only audit, `*.erased`, durable timer), search/reference-graph (purge+reindex / tombstone),
  tenancy-and-control-plane (member_cells, the cross-cell bridge, the residency gate enforcement).
