# GDPR & EU-Sovereignty — Architectural Constraints

> Phase: `01-research`. Status: research deliverable.
> Scope: GDPR obligations and EU digital-sovereignty requirements *as architectural
> constraints* on the Myelin platform (git hosting, CI, issue tracker, knowledge
> platform, chat) and its shared systems (identity/access, event bus, agent fabric,
> storage, search, notifications, cross-artifact reference graph).
>
> **This is not a feature list.** Everything here is a constraint the later
> architecture phases (`02`–`07`) must satisfy or explicitly justify deviating from
> (per VISION.md §3, §6). Where a constraint shapes a subsystem, it is flagged.
>
> **Legal disclaimer / honesty about uncertainty:** I am not a lawyer and this is not
> legal advice. This document encodes a competent engineer's reading of the GDPR, the
> EU AI Act, and the EU-sovereignty discourse as of early 2026. Several points marked
> **[OPEN — LEGAL]** require qualified counsel and/or DPO sign-off before they become
> binding architecture. I have flagged genuine uncertainty rather than guessing.

---

## 0. Why this is upstream of everything

GDPR and sovereignty cannot be retrofitted. The "right to erasure reaching every
subsystem and the event bus and the search index" is an architectural property of the
*whole platform*, not a button in a settings page. If the shared systems
(`03-shared-systems-architecture`) are not designed for it from the start, no amount of
subsystem polish will recover it. Therefore this document is intended to be a hard input
to phases `02` and `03` especially.

Three load-bearing ideas drive the rest of the document:

1. **Every piece of personal data must be addressable and reachable.** If we cannot
   enumerate *where* a person's data lives across five subsystems + shared infra, we
   cannot honour access, erasure, or portability. This forces a **data inventory /
   data map** that is *generated from the system*, not maintained by hand.
2. **Tenancy is the unit of sovereignty.** Residency, key custody, sub-processor
   exposure, and isolation are all expressed per-tenant. The tenant model is therefore
   a GDPR/sovereignty primitive, not just a billing/auth primitive.
3. **Immutability and event-sourcing fight erasure.** Git history, append-only event
   logs, and immutable audit trails are all in direct tension with "right to be
   forgotten." This tension must be *designed for* (crypto-shredding, tombstones,
   pseudonymisation), not discovered late. See §6.

---

## 1. GDPR obligations relevant to a multi-tenant dev platform

### 1.1 Roles: controller vs processor (the foundational distinction)

Myelin operates in **two distinct legal postures**, and the architecture must support
both because they impose different obligations:

- **Myelin-the-product as a processor.** For *customer content* — repos, issues, docs,
  chat messages, CI logs, and the personal data of the customer's employees/end-users
  embedded therein — the customer organisation is the **controller** and Myelin is the
  **processor**. Myelin processes that data on documented instructions (Art. 28).
- **Myelin-the-company as a controller.** For *account/operational data* it decides the
  purposes of — the tenant admin's contact details, billing, security/audit logs Myelin
  keeps for its own legitimate interest, product telemetry — Myelin is the **controller**.

Architectural consequence: data must be **classifiable by legal role**, because the
obligations (who answers a data-subject request, what lawful basis applies, retention,
deletion authority) differ. A practical encoding: tag every data category as
`tenant-content` (processor) vs `platform-operational` (controller). This tag drives
DSAR routing, retention, and deletion authority.

> **Self-hosted wrinkle:** when a customer runs Myelin on their own EU infrastructure,
> Myelin (the vendor) may be **neither** controller nor processor for their content —
> just a software supplier. But Myelin-hosted (SaaS, multi-tenant) is the demanding case
> and the one we design for; self-hosting is then a strict subset. **[ASSUMPTION]** We
> design for SaaS-processor as the worst case and self-host inherits it.

### 1.2 Lawful basis (Art. 6)

For each processing activity we must be able to *name* a lawful basis. Likely mapping:

| Data / activity | Likely basis | Notes |
|---|---|---|
| Tenant content (repos/issues/docs/chat) | **Contract** (Art. 6(1)(b)) — performing the service | Controller is the customer; basis flows from the DPA + service contract |
| Account & billing | Contract | |
| Security/audit logging, abuse prevention | **Legitimate interest** (Art. 6(1)(f)) | Needs a documented LIA (legitimate-interest assessment) |
| Product analytics / telemetry | **Consent** or legitimate interest | Default to *off* / minimal; see privacy-by-default §1.10 |
| Marketing comms | **Consent** | Separate, withdrawable |
| Agent processing of personal data | depends — see §7 | Inherits the basis of the activity it serves; AI Act adds duties |

Architectural consequence: a **processing register** (machine-readable, §1.6) keyed by
processing activity, each row carrying its lawful basis. Consent (where used) must be
**recorded, versioned, timestamped, and withdrawable**, and withdrawal must propagate
(stop processing, and possibly trigger deletion).

### 1.3 Data minimisation (Art. 5(1)(c))

Collect/retain only what each purpose needs. Constraints:

- No "log everything forever by default." Logs and telemetry are **scoped and TTL'd**.
- Avoid incidental personal data sprawl: e.g. full request bodies in traces, IPs in
  every log line, email addresses copied into search payloads "just in case."
- **Field-level intent:** every stored field should be justifiable against a purpose.
  This is a design-review gate, not a runtime check, but the architecture should make
  the *default* path minimal.

### 1.4 Purpose limitation (Art. 5(1)(b))

Data collected for one purpose may not be silently repurposed. The sharp cases for
Myelin:

- **Customer content must not be used to train shared/global AI models** without an
  explicit, separate lawful basis and contractual permission. This is a *huge*
  sovereignty/trust point. **Default: tenant content is never used to train cross-tenant
  models.** (See §7, §8.) Per-tenant fine-tuning, if offered, stays within the tenant
  boundary.
- Telemetry collected for reliability must not leak into sales/marketing.

Architectural consequence: **purpose tags travel with data flows**; the event bus and
agent fabric must be able to enforce "this data may only be used for purposes X, Y."

### 1.5 Data subject rights (Arts. 12–22) — the operational core

These are the rights the architecture must *mechanically* satisfy. Each becomes a
cross-cutting pipeline (see §5 and the checklist §9).

- **Right of access (Art. 15):** produce a copy of all personal data about a subject +
  metadata (purposes, recipients, retention, source). Requires the **data map** to
  enumerate every store. Hard part: search indices, caches, backups, derived data,
  agent memory.
- **Right to rectification (Art. 16):** correct inaccurate data. Usually
  straightforward in primary stores; tricky for *derived/denormalised* copies (search
  index, reference graph, notification history, agent context). Rectification must fan
  out or invalidate derivatives.
- **Right to erasure / "right to be forgotten" (Art. 17):** delete personal data when
  the basis ends, consent is withdrawn, or on request. **The hardest constraint in the
  whole platform.** Must reach: primary DBs, search index, event bus history, caches,
  blob/object storage, backups (eventually), audit logs (with carve-outs), agent
  memory, the reference graph, notifications, and CDN. See §6 for the immutability
  conflicts and the crypto-shredding strategy.
- **Right to data portability (Art. 20):** export personal data the subject provided, in
  a **structured, commonly used, machine-readable** format (JSON/CSV; git already is
  portable via clone; docs as Markdown/HTML; issues as JSON). Distinct from access:
  portability is about *re-usable export*, and only covers data provided by the subject
  under consent/contract.
- **Right to restriction (Art. 18):** "freeze" processing without deleting — e.g. while
  a dispute is resolved. Architecturally this is a **per-record/per-subject state flag**
  that suppresses processing (no indexing, no agent use, no analytics) while retaining
  storage. Non-trivial: the event bus and agent fabric must honour a "restricted" flag.
- **Right to object (Art. 21):** object to processing based on legitimate interest or to
  direct marketing. Architecturally similar to restriction + consent withdrawal:
  must stop the objected-to processing path.
- **Automated decision-making / profiling (Art. 22):** right not to be subject to solely
  automated decisions with legal/significant effect. **Directly relevant to the agent
  fabric.** If an agent can auto-close an issue, auto-reject a contribution, gate access,
  or make HR-ish decisions, we may be in Art. 22 territory → need human-in-the-loop,
  meaningful information, and contestability. See §7.

**Cross-cutting requirement:** a **Data Subject Request (DSR) orchestrator** — a shared
service that fans a request (access/erasure/rectification/restriction/portability) out
to every subsystem and shared store via a registered handler interface, tracks
completion, and enforces the **one-month statutory response deadline** (extendable to
three for complex requests). Every subsystem must implement the handler interface. This
is a primary input to `03-shared-systems-architecture`.

### 1.6 Records of processing activities (RoPA, Art. 30)

Controllers and processors must maintain records of processing. Architectural
consequence: keep the RoPA **as data, generated/validated against the running system**,
not a stale spreadsheet. Each processing activity: purpose, lawful basis, categories of
subjects/data, recipients, transfers, retention, security measures. The data map (§5.1)
feeds this. **Aspiration:** drift between declared RoPA and actual data flows should be
detectable.

### 1.7 Data Processing Agreements (Art. 28)

When Myelin is processor, a **DPA** with each customer is mandatory, specifying
subject-matter, duration, nature/purpose, data types, controller instructions,
confidentiality, security, sub-processor terms, assistance with DSRs and breaches,
deletion/return at end of contract, and audit rights. Architectural consequences:

- **"Return or delete all personal data at end of contract"** → a **tenant offboarding
  pipeline**: full export + verifiable, complete deletion across all stores. This is
  erasure at tenant granularity and must be just as thorough as per-subject erasure.
- **"Assist the controller with DSRs"** → the DSR orchestrator must be operable *by or on
  behalf of* a tenant for *their* data subjects (the customer's employees/end-users).
  Tenants need self-service or assisted DSAR tooling.
- **Audit rights** → audit logs and the data map must be exportable/inspectable per
  tenant.

### 1.8 Sub-processors & transparency

Any third party processing personal data on Myelin's behalf (cloud host, managed DB,
email/SMS provider, error tracking, the *real* agent/LLM providers later) is a
**sub-processor**. Obligations:

- Maintain a **public, versioned sub-processor list**; notify customers of changes with
  a right to object.
- Each sub-processor must be under a back-to-back DPA and provide adequate guarantees.
- **Sovereignty constraint:** prefer EU-domiciled, EU-operated sub-processors;
  US-controlled sub-processors are a CLOUD Act exposure even if they run EU regions
  (see §2). The architecture must let us **swap sub-processors per region/tenant** —
  reinforcing the strategy-pattern mandate from VISION.md (mock vs real agents is one
  instance of a broader "pluggable processor" need).

Architectural consequence: every external dependency that touches personal data is a
**registered, swappable, region-aware adapter**. No hard-wired third party in a hot path
that handles personal data.

### 1.9 Breach notification (Arts. 33–34)

- **To the supervisory authority within 72 hours** of becoming aware, unless unlikely to
  result in risk.
- **To affected data subjects** without undue delay if high risk.
- As **processor**, Myelin must notify the **controller (customer) without undue delay**
  so they can meet *their* 72h clock.

Architectural consequences:

- **Detection + scoping must be fast.** To notify, we must answer "*whose* data, *which*
  categories, *which* tenants" quickly. This again depends on the data map and on
  **per-tenant isolation** (so a breach can be bounded to specific tenants).
- **Tamper-evident audit logging** to establish what happened and when "awareness" began.
- A **breach runbook + notification tooling** (templated controller notifications,
  tracked deadlines). Partly process, but the system must surface the data needed.

### 1.10 Privacy by design & by default (Art. 25)

- **By design:** privacy controls built into architecture from the start (this whole
  document).
- **By default:** the *default* configuration is the most privacy-protective —
  minimal data, narrowest sharing, shortest retention, telemetry off/opt-in, private
  visibility by default, agents *not* granted broad data access unless configured.

Architectural consequence: defaults are a design deliverable. Every subsystem's default
settings get a privacy review. "Secure/private by default, open by choice."

### 1.11 DPIA triggers (Art. 35)

A **Data Protection Impact Assessment** is required for processing likely to result in
high risk — notably: large-scale processing, systematic monitoring, **innovative use of
technology**, and **automated decision-making**. Myelin almost certainly triggers a DPIA
because of:

- The **agent fabric** processing personal data at scale (innovative tech + automated
  decisions → strong DPIA trigger, reinforced by the EU AI Act, §7).
- Large-scale processing of employee/contributor data across many tenants.
- Cross-artifact reference graph (systematic linking of personal data across contexts).

Architectural consequence: features that materially change data processing — especially
new agent capabilities — should carry a **DPIA gate** in the product process. The
architecture should expose the inputs a DPIA needs (data flows, recipients, risks).

---

## 2. What "EU digital sovereignty" means operationally

Sovereignty is broader than GDPR. GDPR is *lawful processing*; sovereignty is *who can
ultimately compel, access, or cut off the data and the service*. A system can be
GDPR-compliant and still not sovereign (e.g. EU data hosted by a US-controlled provider
subject to the CLOUD Act).

### 2.1 Data residency / regionalisation

- Personal (and ideally all) data **stays within a chosen region** (EU, or a specific
  member state for stricter customers — e.g. public sector). Region is selected
  **per tenant** and is a hard boundary: storage, compute, backups, search indices,
  caches, logs, and **agent processing** all stay in-region.
- **No silent cross-region replication.** DR/backups stay in-region (or in another EU
  region only if the tenant allows). Telemetry and aggregates must not exfiltrate
  region-bound personal data.
- Implies a **region-pinned, cell-based deployment topology** (see §4) and a routing
  layer that *cannot* send a tenant's data to the wrong region.

### 2.2 EU-controlled infrastructure & avoiding US CLOUD Act exposure

- The **US CLOUD Act** can compel US-headquartered providers to produce data they hold
  *regardless of where it is stored*, including EU regions. "Data in Frankfurt on a US
  provider" is not automatically sovereign.
- Sovereignty posture options (architecture must support a spectrum):
  1. **EU-domiciled providers / sovereign cloud** (e.g. operators not subject to US
     jurisdiction; "sovereign cloud" offerings; **Gaia-X**-aligned providers). Strongest.
  2. **Hyperscaler EU "sovereign" partnerships** where an EU entity operates the cloud and
     holds keys (weaker; legal jurisdiction debated — **[OPEN — LEGAL]**).
  3. **Self-hosting / on-prem** by the customer (full control, our software supplied).
- **Goal:** the platform must be **portable across EU infrastructure providers** and
  **self-hostable**, so we are never locked to a single (possibly non-sovereign)
  provider. This pushes toward open, standard components (Postgres, S3-compatible object
  storage, OCI containers, Kubernetes-or-equivalent) over proprietary managed services
  that bind us to one hyperscaler.

> **Tension with "world-scalable from day 1" (VISION.md §3):** the easy path to global
> scale is a US hyperscaler's managed services. Sovereignty forbids leaning on those.
> The resolution is a **portable, cell-based architecture on commodity primitives** that
> can be deployed into EU-sovereign regions and self-hosted — scale via *many cells*, not
> via proprietary global services. This is a key cross-cutting decision for `02`/`03`.

### 2.3 Encryption & key custody (BYOK / HYOK)

- **Encryption in transit** (TLS everywhere, internal + external) and **at rest**
  (storage, DB, backups, object store, search index) is table stakes.
- **Key custody is the sovereignty lever.** Tiers the architecture should support:
  - **Provider-managed keys** — baseline.
  - **Per-tenant keys** — each tenant's data encrypted under a tenant-scoped key
    (envelope encryption). Enables **crypto-shredding** (destroy key → data
    unrecoverable) which is the linchpin of erasure-on-immutable-stores (§6).
  - **BYOK (Bring Your Own Key)** — tenant supplies/controls the key material in a KMS;
    Myelin uses it but the tenant can revoke.
  - **HYOK (Hold Your Own Key)** — key never leaves the tenant's custody; strongest
    sovereignty, but constrains what server-side processing/search/agents can do over
    the data (you can't index plaintext you can't decrypt). **Trade-off must be explicit.**
- Architectural consequence: a **KMS abstraction** with per-tenant key hierarchies and
  envelope encryption baked into the storage layer. **Crypto-shredding must be a
  first-class deletion primitive**, not a workaround.

### 2.4 Schrems II / international transfer mechanisms

- *Schrems II* (CJEU, 2020) invalidated Privacy Shield and raised the bar for transfers
  to third countries (esp. the US). The **EU–US Data Privacy Framework (DPF)** (2023)
  currently provides an adequacy route for certified US importers, **but it is under
  legal challenge and could be invalidated** ("Schrems III" risk). **[OPEN — LEGAL]**
- **Design stance:** do **not** depend on transfer mechanisms. The default and strongly
  preferred posture is **no transfer of personal data outside the EU/EEA at all.** If any
  transfer ever occurs, it requires: a valid mechanism (adequacy/SCCs +
  supplementary measures), a documented Transfer Impact Assessment, and tenant
  transparency. Architecturally: **transfers are off by default and gated.**
- This is why the *real* agent/LLM providers chosen later **must be EU-hostable** — a US
  LLM API would be a transfer + CLOUD Act + purpose-limitation problem all at once. The
  mock-agent strategy pattern (VISION.md §3) is what lets us defer and then choose an
  EU-sovereign agent backend.

### 2.5 Gaia-X / EU cloud context

- **Gaia-X** is the EU initiative for a federated, sovereign data infrastructure with
  standards for transparency, portability, and sovereignty (self-descriptions,
  conformity). Not a cloud itself; a framework/label. Relevance: aligning with Gaia-X
  principles (portability, provider transparency, no lock-in, verifiable claims) is a
  credible way to *signal and structure* sovereignty, and may be procurement-relevant for
  EU public-sector customers. **[UNCERTAIN]** how much formal Gaia-X conformance buys us
  commercially vs. simply being demonstrably EU-sovereign; treat as "align with
  principles, pursue formal labels only if customers demand."
- Related context to track (not hard requirements yet): **EU Data Act**,
  **EUCS** (EU Cloud Services certification scheme, with its debated "sovereignty
  requirements"), **NIS2** (security obligations for certain sectors), and **eIDAS 2.0**
  (EU digital identity wallets) which could matter for the identity subsystem.
  **[DEFERRED]** — flagged for `02`/`03` to assess applicability per target customer.

---

## 3. From obligations to architectural requirements (the translation)

This section maps the above onto concrete, cross-cutting architectural requirements.
Subsystem-specific impacts are flagged inline.

### 3.1 Tenant data isolation

- **Tenant is the primary isolation and sovereignty boundary.** Every personal-data
  record is attributable to exactly one tenant (plus the platform-operational set Myelin
  controls).
- Isolation spectrum (architecture should support more than one, by tenant tier):
  **logical isolation** (shared infra, tenant_id scoping + row-level security) →
  **schema/DB-per-tenant** → **cell/stack-per-tenant** (dedicated infra, strongest
  isolation + cleanest residency/erasure/breach-scoping). High-assurance / public-sector
  tenants likely need a dedicated cell.
- Isolation must hold across **all** shared systems: event bus topics, search indices,
  caches, blob prefixes, agent context, reference graph partitions. A cross-tenant data
  leak is both a GDPR breach and a sovereignty failure.
- **Cross-tenant references** (e.g. a public open-source repo referenced from another
  tenant) are a special case — the **reference graph must not become a personal-data
  side-channel** across tenant boundaries.

### 3.2 Region pinning

- **Tenant → region** binding, immutable-by-default, enforced at the routing/data layer
  so it is *impossible* (not merely discouraged) to write a tenant's personal data into
  the wrong region.
- All derived stores (search, cache, analytics) inherit the tenant's region.
- **Region is part of every data-bearing service's address.** Cross-region calls in hot
  paths handling personal data are prohibited by construction.

### 3.3 Per-tenant encryption keys & crypto-shredding

- Envelope encryption with a **per-tenant key hierarchy** (tenant root key → per-store /
  per-purpose data keys). Optionally per-*subject* sub-keys where feasible to enable
  per-subject crypto-shredding (heavier; evaluate in `03`).
- **Crypto-shredding as a deletion primitive:** destroying a key renders the ciphertext
  (in DB, backups, object store, immutable logs) unrecoverable — the practical answer to
  "erase from backups and append-only logs." (Caveat in §6.)
- BYOK/HYOK support per §2.3, with explicit documentation of the feature trade-offs HYOK
  imposes on search and agents.

### 3.4 Audit logging

- **Tamper-evident, append-only audit log** of security- and privacy-relevant events:
  access to personal data, DSR handling, exports, deletions, admin actions, sub-processor
  changes, key operations, agent actions on personal data.
- Needed for: breach scoping (§1.9), accountability (Art. 5(2)), DPA audit rights,
  AI Act logging (§7), and proving erasure happened.
- **Conflict:** audit logs themselves contain personal data (who did what). Resolution:
  audit logs are kept under a **distinct legitimate-interest basis with a defined
  retention period**, are minimised (log identifiers/pseudonyms, not payloads), and are
  **carved out of erasure** to the extent law permits (you may retain what you need to
  evidence compliance/defend claims), then deleted on retention expiry — ideally via
  crypto-shredding of the audit key. **[OPEN — LEGAL]** exact carve-out scope per
  jurisdiction.

### 3.5 Deletion / erasure pipeline that reaches everything

The flagship requirement. A **DeletionRequest** (subject-scoped or tenant-scoped) must
fan out to a registered handler in **every** store, and prove completion:

- Primary databases (all five subsystems)
- Object/blob storage (attachments, avatars, CI artifacts, doc media)
- **Search indices** (must re-index/purge, not just hide)
- **Event bus history** (see §6 — tombstone + crypto-shred, can't hard-delete an
  append-only log cheaply)
- Caches & CDN (purge/expire)
- **Backups & snapshots** (cannot hard-delete a point-in-time backup in place → rely on
  crypto-shredding + bounded backup retention so the data ages out)
- Audit logs (carve-out + retention expiry, §3.4)
- **Agent memory / context / derived embeddings** (§7 — easy to forget; embeddings can
  re-identify)
- Reference graph (remove/anonymise nodes/edges referencing the subject)
- Notification history

Requirements: **idempotent**, **resumable**, **verifiable** (proof/receipt of deletion),
**fan-out via a registered subsystem interface** (each subsystem implements
`erase(subject, tenant)` / `export(subject)` / `restrict(subject)`), and **bounded by the
statutory deadline**. This interface is a *core* shared-systems contract — every
subsystem and every store registers as a "personal-data holder."

> **Cross-ref:** this is the single most important reason the event bus, search, and
> storage in `03` must be co-designed with erasure in mind. See §6 for the immutability
> mechanics.

### 3.6 Consent & retention policies

- **Consent store:** versioned, timestamped, granular, withdrawable; withdrawal triggers
  stop-processing and possibly deletion. Mostly for *Myelin-as-controller* data
  (telemetry, marketing); tenant-content basis is contract, not consent.
- **Retention engine:** per data category, a defined retention period and an automated
  expiry/deletion job. "Delete when no longer needed for the purpose." Tenants may
  configure stricter retention for their content (e.g. delete chat after N days).
  CI logs, ephemeral build data, and notification history are prime candidates for short
  default TTLs (also helps §1.3 minimisation).

### 3.7 Processor / sub-processor transparency

- Machine-readable, **versioned sub-processor registry**, surfaced publicly and per
  tenant, with change notifications + objection workflow (§1.8).
- Every personal-data-touching external dependency is a **region-aware, swappable
  adapter** (strategy pattern) — including the future real-agent provider.

### 3.8 The data map / inventory (the connective requirement)

A **system-generated inventory** of *what personal data exists, where, under what basis,
for what purpose, with what retention, flowing to which recipients.* It is the substrate
for RoPA (§1.6), access (§1.5), erasure fan-out (§3.5), breach scoping (§1.9), and DPIAs
(§1.11). **If this does not exist, none of the rights pipelines can be guaranteed
complete.** Treat as a first-class shared system. Ideally derived from a schema-level
**personal-data classification** (every column/field tagged: is-personal, category,
sensitivity, basis, retention) so the map can't silently drift from reality.

---

## 4. Deployment topology implications (sovereignty + scale)

- **Cell-based, region-pinned topology.** A "cell" = a self-contained stack
  (all subsystems + shared systems) in one region. Tenants are assigned to a cell.
  Benefits: residency by construction, breach blast-radius bounded to a cell, clean
  per-tenant/per-cell erasure and offboarding, horizontal scale by adding cells
  (squares the circle of "world-scale without a US hyperscaler's global services"),
  and a natural unit for dedicated/sovereign deployments.
- **Portable substrate:** commodity, EU-deployable, self-hostable primitives
  (containers/OCI, Postgres, S3-compatible object storage, an event log that can run on
  EU infra) over proprietary hyperscaler-locked managed services. **[Decision deferred to
  `02`/`03`]** — but the *constraint* is portability + EU-deployability.
- **Control plane vs data plane:** a global control plane is acceptable *only if it holds
  no in-region personal data* — it orchestrates, the data planes (cells) hold personal
  data within their region. The control plane must itself be EU-sovereign.
- **Self-host parity:** the same artifacts that run a cell run a customer's on-prem
  install. This forces clean packaging and no hidden cloud dependencies.

---

## 5. The cross-cutting pipelines (concrete shared-system contracts)

To make §3 actionable, these shared contracts are proposed for `03`:

### 5.1 PersonalDataHolder interface (every store/subsystem implements)
```
locate(subjectRef, tenant)      -> records + metadata   // for access
export(subjectRef, tenant)      -> portable bundle       // for portability
rectify(subjectRef, patch)      -> applied + derivatives invalidated
restrict(subjectRef, on/off)    -> processing suppressed while stored
erase(subjectRef|tenant)        -> deleted/crypto-shredded + receipt
```
Implemented by: all 5 subsystems, search, event bus, object store, agent fabric,
reference graph, notifications, caches, audit (with carve-outs).

### 5.2 DSR Orchestrator
Receives a request, resolves the subject across tenants, fans out to all
PersonalDataHolders, tracks completion against the statutory deadline, produces a
verifiable result/receipt, logs to audit. Operable by Myelin (for its controller data)
and *by/for tenants* (for their data subjects, per Art. 28 assistance).

### 5.3 Retention/Expiry engine
Drives automated deletion by category TTL; integrates with crypto-shredding for
immutable stores.

### 5.4 Consent & lawful-basis registry
Versioned consent; machine-readable lawful-basis-per-activity feeding RoPA.

### 5.5 Sub-processor & transfer registry
Versioned list + change notifications; transfer gating (default: no extra-EU transfer).

### 5.6 KMS / key-custody service
Per-tenant key hierarchy, envelope encryption, BYOK/HYOK, crypto-shred operations.

### 5.7 Tamper-evident audit log
Append-only, minimised, retention-bounded, exportable per tenant.

### 5.8 Data map / classification registry
Schema-level personal-data tagging → generated inventory → RoPA, DPIA inputs, breach
scoping.

---

## 6. Special wrinkles (the hard conflicts)

### 6.1 Erasure vs git immutability/history
Git is a content-addressed, immutable DAG; rewriting history changes every downstream
hash, breaks clones, and is destructive. Yet a commit author's email/name is personal
data, and PII can be committed into files. Options, none perfect:

- **Personal data in commit *metadata* (author/committer email/name):** mitigate up front
  with **pseudonymous/no-reply commit identities** (a per-tenant noreply email mapped to
  the real identity *outside* git), so git itself holds little/no raw personal data and
  the mapping table is erasable. This is the cleanest design choice — *prevent* PII
  entering immutable history. **(Strong recommendation, cross-ref git-hosting subsystem.)**
- **Personal data committed into file *content*:** true erasure needs **history rewrite**
  (filter-repo style) — disruptive but sometimes legally required. Must be a supported,
  audited, tenant-initiated operation, with clear communication that hashes change.
- **Crypto-shredding the repo/tenant:** for tenant offboarding, destroying the tenant key
  renders the whole repo unrecoverable — clean at tenant granularity, not per-subject.
- **[OPEN — LEGAL]** How far must per-subject erasure go into immutable VCS history vs.
  what is "technically infeasible / disproportionate effort" (Art. 17 has limits)? Needs
  counsel. Design to *minimise* PII in history so the question rarely bites.

### 6.2 Erasure across an event-sourced bus / append-only log
Event sourcing keeps an append-only log as the source of truth — directly hostile to
deletion. Strategies (likely combined):

- **Keep personal data *out* of event payloads.** Events carry **references/IDs**; the
  personal data lives in an erasable store the event points to. Erasing the store satisfies
  erasure; the event becomes a dangling reference (acceptable). *Strong default.*
- **Crypto-shredding per subject/tenant:** encrypt personal fields in events under a
  per-subject/tenant key; destroy the key to render them unreadable ("forgotten payload").
- **Tombstone + compaction:** for log-compacted topics, write a tombstone and let
  compaction drop the prior value (works for keyed/latest-value topics, not for full
  immutable history).
- **Bounded retention:** event history TTL'd so personal data ages out; long-term state
  lives in erasable read models, not the raw log.

Architectural consequence for `03`: **event schema design must classify personal data and
prefer references-not-payloads; the bus must support crypto-shred and bounded retention.**

### 6.3 Audit logs vs erasure
Audit logs need to persist *to prove* compliance/security, but contain personal data.
Resolution (also §3.4): minimise (pseudonymise, log IDs not content), keep under
legitimate-interest with a fixed retention, **lawfully carve out from erasure** what is
needed to evidence compliance/defend legal claims, then expire (crypto-shred the audit
key). Balance is fact-specific → **[OPEN — LEGAL]**.

### 6.4 AI/agent processing of personal data + EU AI Act
See §7 — large enough to be its own section.

### 6.5 Backups
You cannot surgically delete one subject from an immutable point-in-time backup.
Resolution: **crypto-shredding** (key destroyed → backup ciphertext unrecoverable) +
**bounded backup retention** (the subject's pre-deletion data ages out within the backup
window). Document the backup-window lag as the residual exposure. Restores must re-apply
the deletion log so a restore doesn't "resurrect" erased data — a **post-restore
re-erasure** step.

### 6.6 Search indices & derived/denormalised data
Search indices, embeddings, caches, the reference graph, and notification history are
*copies/derivatives* of personal data and are routinely forgotten in erasure. Every
derivative is a PersonalDataHolder (§5.1) and must purge/re-index on erasure and
invalidate on rectification. **Embeddings are personal data** if they derive from and can
re-identify a person — they must be erasable too.

---

## 7. Agent fabric: GDPR + EU AI Act (the agent-native wrinkle)

VISION.md mandates an agent-native platform with mock agents now (strategy pattern) and
real agents later. Both GDPR and the **EU AI Act** constrain this. Designing for the
constraints *now* (even with mock agents) is mandatory so the later swap is purely an
implementation change.

### 7.1 GDPR angles for agents
- **Agents are processing of personal data.** An agent reading issues/chat/docs/code to
  act is processing — it inherits the lawful basis and **purpose limitation** of the
  activity it serves. It must not repurpose data (e.g. an agent summarising a channel must
  not feed that into cross-tenant model training).
- **Agent memory/context is a personal-data store** → a PersonalDataHolder (§5.1):
  erasable, restrictable, region-pinned, encrypted. Embeddings and retrieved context
  count.
- **Data minimisation for agents:** agents get the **least data necessary**, scoped to the
  task and tenant. No standing god-mode access to all tenant data. Access goes through the
  same identity/permission model as humans, plus purpose scoping.
- **Automated decision-making (Art. 22):** if an agent makes a decision with legal/
  significant effect on a person *without meaningful human involvement* (auto-reject a
  contribution, gate access, flag/penalise a contributor), Art. 22 safeguards apply:
  human-in-the-loop, ability to contest, meaningful information about the logic. Design
  agents as **suggest-by-default, human-confirm for consequential actions.**
- **Transfers/sub-processor:** the real LLM/agent backend is a sub-processor and a
  potential transfer. **Must be EU-hostable / EU-sovereign.** The mock-vs-real strategy
  pattern is exactly the seam that lets us pick an EU-sovereign backend without a rewrite.
- **Training on tenant content:** default **no** cross-tenant training (§1.4). Any opt-in
  training stays within the tenant boundary and needs a documented basis.

### 7.2 EU AI Act angles
The EU AI Act is risk-tiered (prohibited / high-risk / limited-risk/transparency /
minimal) and is **phasing in (2025–2027)**, with **GPAI (general-purpose AI) model
obligations** and transparency duties. Relevance to Myelin's agent fabric:

- **Most dev-assistant agents are likely "limited/minimal risk"** → primarily
  **transparency obligations**: users must know they're interacting with AI; AI-generated
  content may need to be marked as such. Architecture: **agents are identifiable as agents**
  in chat/issues/reviews (cross-ref chat & identity subsystems — agent identities are
  first-class and clearly labelled).
- **High-risk** classification could arise if agents are used for things like employment
  decisions, access to essential services, etc. Myelin's *core* dev workflows probably
  aren't high-risk, but **tenants could configure agents into high-risk uses** → the
  platform should not *prevent* compliance and should provide the **logging, human-
  oversight hooks, and documentation** high-risk systems need (Art. 22 overlap).
- **GPAI providers** (the future LLM backend) carry their own obligations (technical docs,
  training-data summaries, copyright policy). As a **deployer**, Myelin should pick GPAI
  backends that meet these and surface the needed transparency.
- **Logging & human oversight as design hooks:** even for mock agents now, build
  **agent-action audit logging**, **human-confirmation gates for consequential actions**,
  and **clear agent labelling**. These satisfy both Art. 22 and AI Act transparency/
  oversight and cost little to stub now, a lot to retrofit later.

**[UNCERTAIN / track]** Exact AI Act obligations depend on final classification and the
phased timeline; treat the above as design-safe minimums and revisit in architecture
phases with the then-current legal position.

---

## 8. Sharp default decisions (recommended, to be ratified by `02`/`03` + counsel)

These are opinionated defaults that make the constraints concrete. Each can be overridden
with written justification (VISION.md §3).

1. **No personal data leaves the EU/EEA. Ever, by default.** Transfers are off and gated.
2. **No cross-tenant model training on tenant content.** Period, unless separately and
   explicitly permitted.
3. **Region is per-tenant, immutable, and enforced at the data layer.**
4. **Per-tenant envelope encryption with crypto-shredding as a first-class deletion
   primitive.** BYOK supported; HYOK supported with documented feature trade-offs.
5. **Keep personal data out of immutable structures** (git history, event payloads):
   references + pseudonymous identities, with the erasable mapping outside the immutable
   store.
6. **Every store/subsystem implements the PersonalDataHolder interface.** No store is
   exempt; "we forgot the search index" is a design failure.
7. **Privacy-by-default settings:** private visibility, telemetry opt-in, minimal
   retention, agents least-privilege.
8. **Agents: suggest-by-default; human-confirm consequential actions; always labelled as
   agents; all agent actions audited.**
9. **Every external personal-data-touching dependency is a swappable, region-aware,
   EU-preferring adapter** (extends the mock/real-agent strategy pattern platform-wide).
10. **Cell-based, region-pinned, portable, self-hostable topology** as the reconciliation
    of world-scale with EU-sovereignty.

---

## 9. Requirements checklist for the architecture phases

The architecture (`02`–`05`) and roadmaps (`06`) must satisfy or explicitly justify
deviating from each:

**Identity, tenancy, isolation**
- [ ] Tenant is the primary isolation + sovereignty boundary; isolation holds across all
      shared systems (bus, search, cache, blob, agent context, reference graph).
- [ ] Data classified by legal role (`tenant-content` / processor vs
      `platform-operational` / controller).
- [ ] Cross-tenant references can't leak personal data.

**Residency & sovereignty**
- [ ] Per-tenant, immutable region binding enforced at the data layer (impossible to
      misroute).
- [ ] All derived stores (search/cache/analytics/backups/agent) inherit region.
- [ ] Portable, EU-deployable, self-hostable substrate; no hard hyperscaler lock-in.
- [ ] EU-sovereign control plane holding no in-region personal data.
- [ ] No extra-EU transfer by default; transfers gated + assessed if ever enabled.

**Encryption & keys**
- [ ] Encryption in transit + at rest everywhere (incl. search index, backups).
- [ ] Per-tenant envelope encryption; BYOK; HYOK (with documented trade-offs).
- [ ] Crypto-shredding implemented as a deletion primitive.

**Data subject rights pipelines**
- [ ] PersonalDataHolder interface implemented by every store/subsystem.
- [ ] DSR orchestrator: access, rectification, erasure, restriction, portability;
      deadline-tracked; verifiable receipts; tenant-operable (Art. 28 assistance).
- [ ] Erasure reaches DBs, object store, **search**, **event bus**, caches/CDN,
      **backups** (crypto-shred + window), **agent memory/embeddings**, reference graph,
      notifications, audit (carve-out + expiry).
- [ ] Rectification invalidates/fans out to all derivatives.
- [ ] Restriction/objection suppress processing (no indexing/agents/analytics) while
      retaining storage.
- [ ] Portability exports in structured machine-readable formats per subsystem.
- [ ] Post-restore re-erasure so backups don't resurrect deleted data.

**Records, consent, retention, transparency**
- [ ] System-generated data map / personal-data classification (schema-level tags).
- [ ] Machine-readable RoPA derived from the data map.
- [ ] Versioned consent store (withdrawable, propagating).
- [ ] Retention/expiry engine with per-category TTLs and automated deletion.
- [ ] Versioned, public + per-tenant sub-processor registry with change
      notification/objection.
- [ ] Tenant offboarding pipeline (export + complete, verifiable deletion).

**Security, audit, breach**
- [ ] Tamper-evident, append-only, minimised, retention-bounded audit log; per-tenant
      exportable.
- [ ] Breach detection/scoping fast enough for 72h (controller-notify) chain; bounded by
      tenant/cell isolation.
- [ ] DPIA inputs (data flows, recipients, risks) exposed by the architecture.

**Agents (GDPR + AI Act)**
- [ ] Agent memory/context/embeddings are erasable PersonalDataHolders, region-pinned,
      encrypted.
- [ ] Agents least-privilege; access via the human identity/permission model + purpose
      scoping; no god-mode.
- [ ] Agents labelled as agents everywhere (AI Act transparency).
- [ ] Consequential agent actions require human confirmation (Art. 22); all agent actions
      audited.
- [ ] Real-agent/LLM backend is an EU-sovereign, swappable sub-processor adapter (mock
      now via strategy pattern).
- [ ] No cross-tenant training on tenant content by default.

**Defaults**
- [ ] Privacy-by-default config across all subsystems (visibility, telemetry, retention,
      agent access) reviewed and documented.

---

## 10. Open questions (genuine — for counsel and/or later phases)

**Legal [OPEN — LEGAL] / [UNCERTAIN]:**
1. Exact scope of Art. 17 erasure into **immutable git history** vs.
   "technically infeasible/disproportionate" limits. (Mitigate by keeping PII out of
   history; confirm the residual obligation with counsel.)
2. Lawful **retention period and erasure carve-out** for audit/security logs per relevant
   jurisdictions.
3. Stability of the **EU–US DPF** ("Schrems III" risk) — we design to not depend on it,
   but need it confirmed as policy.
4. Whether **Gaia-X / EUCS / NIS2 / eIDAS 2.0 / EU Data Act** impose hard requirements for
   our target customer segments (esp. public sector), or are merely advantageous.
   **[DEFERRED to `02`/`03` with counsel.]**
5. Final **EU AI Act classification** of Myelin's agent capabilities and the phased
   obligation timeline; confirm "design-safe minimums" (§7.2) are sufficient.
6. For hyperscaler EU "sovereign cloud" partnerships — is the **CLOUD Act exposure**
   actually severed, or only mitigated? Affects allowed infra providers.

**Technical / architectural (for `02`/`03`):**
7. Per-**subject** crypto-shredding granularity vs per-tenant — feasibility/cost of
   per-subject keys at scale.
8. Cell topology specifics: cell sizing, tenant→cell assignment, multi-cell tenants,
   control-plane design holding zero in-region personal data.
9. Event-bus technology that supports **bounded retention + crypto-shred + tombstones**
   on EU-deployable infra (constrains the `03` bus choice).
10. How HYOK limits **search and agent functionality** (can't index/embed plaintext you
    can't decrypt) — what feature degradation is acceptable, and how to communicate it.
11. Whether the **data map** can be reliably *generated* from schema-level tags (and
    enforced in CI), or needs manual curation (drift risk).
12. Backup-window length vs. erasure SLA — the residual exposure window we accept and
    document.

---

## 11. Cross-references
- **VISION.md §3** — non-negotiables (sovereignty, agent-native + strategy pattern,
  world-scale). This doc operationalises the sovereignty/GDPR non-negotiable and uses the
  strategy-pattern mandate to justify swappable, EU-preferring processor adapters.
- **`02-holistic-architecture`** — must adopt the cell-based region-pinned topology (§4),
  the data-role classification, and the privacy-by-default stance.
- **`03-shared-systems-architecture`** — owns the PersonalDataHolder interface (§5.1), DSR
  orchestrator, KMS/crypto-shred, audit log, data map, retention engine, consent &
  sub-processor registries, and the erasure-aware event bus/search/storage design (§6).
- **Subsystem architectures (`04`)** — each implements PersonalDataHolder; **git hosting**
  (pseudonymous commit identities, history-rewrite erasure §6.1), **chat & identity**
  (agent labelling, Art. 22 §7), **CI** (short log TTLs), **search** (re-index on erasure),
  **knowledge platform** (media/blob erasure + export formats).
- Other research docs in `01-research` (personas, competitive landscape, use-cases,
  technical structuring) should be read alongside this — sovereignty is a *positioning*
  differentiator as much as a constraint.
