# R-19 — Sovereignty-as-UX: residency / GDPR / DSR / audit legibility patterns

> **Phase 4 research corpus** · deliverable of prompt **R-19** (workstream
> [`ws-i-sovereignty-as-ux.md`](../../02-research-roadmap/ws-i-sovereignty-as-ux.md)).
> **File date: 2026-06-20.** No real users exist; personas P1–P15 are HYPOTHESES. This file
> **surfaces** the existing GDPR/Audit machinery
> ([`gdpr-and-audit.md`](../../../planning/05-refined-shared-systems-architecture/gdpr-and-audit.md),
> [`system-overview.md §8.3`](../../../planning/02-holistic-architecture/system-overview.md)) as
> **legible first-class UI**; it does **not** redesign the backend. It builds ON the DPO DSR
> cross-surface flow drawn in **[R-04 §6 (F-GOV-1)](../jtbd-flows/cross-surface-flows.md)** — that
> file owns the *flow*; this file owns the *console UX, the residency cues, the audit explorer, and
> the erased-state craft*. Feeds **rubric D9** and **sketch-funnel Axis 6**.

## 0. The honesty frame (read this first)

**Sovereignty-as-UX is the most under-evidenced area in this corpus.** Phase-1 README §5.7 flags it
as novel: **no external design playbook exists** for "make GDPR/residency/audit *felt*, not fine
print." The *legal/architectural* claims here are **PROVEN** against GDPR articles, the cited
sources, and the frozen `gdpr-and-audit.md` contracts. The *UX choices* — where a cue sits, how a
DPO reads completeness at a glance, whether a region badge calms or clutters — are **HOUSE STYLE**
and **under-evidenced**, falsifiable only by the deferred regulated-buyer review (§9). Where a claim
is design taste dressed as settled, that is a failure (VISION §3); we flag it explicitly.

**Tag legend.** **PROVEN** = a GDPR article / EN 301 549 / cited source / an existing architecture
mechanism we surface. **HOUSE STYLE** = our design synthesis. **`[UNDER-EVIDENCED]`** = HOUSE STYLE
with no external pattern library behind it — the highest-risk class, concentrated in §1, §3, §5.
**`[DEFERRED-UNTIL-USERS]`** = validated only by §9.

**The P9 sovereignty-as-UX heuristic (the lens applied throughout, HOUSE STYLE).** *Three questions
must be answerable in the UI, by the right role, without leaving the product or reading a policy
page (P9, design-language L115–122):*
1. **"Where does this data live?"** → residency cue (§1).
2. **"Who / what processed this, and may see it?"** → visibility chip + provenance + audit (§1, §4).
3. **"Show me everything about this subject."** → DSR console, both sides (§2–§3).
A surface passes only if all three are reachable *and* none of them taxes the calm budget (P8) more
than its risk warrants — that tension **is** Axis 6 (§7).

---

## 1. Residency & visibility cue patterns (always-on, near the data)

The architecture already pins data: **cells are residency-bound** (ADR-11; every cell *is* a
region), indices/blobs/backups are **residency-pinned** (system-overview §95/§98), and the
cross-cell bridge is **PII-free with cell-local resolution** (gdpr-and-audit §4.3). So residency is
*true* by construction; the UX job is to make the true thing **legible without nagging**. We surface
it through the **existing scope indicator**, which design-language §5.1 already says *"doubles as a
residency/visibility cue (P9)"* — we do **not** add a new chrome element.

### 1.1 The residency cue ladder (four tiers, escalating only with risk) `[UNDER-EVIDENCED]`

| Tier | Where it sits | Form | When it's the right tier |
|---|---|---|---|
| **T0 ambient** | The shell scope indicator (§5.1), always visible | A quiet region token, e.g. `EU · Frankfurt` next to the tenant/space name; neutral-weight, **not** an accent or a flag emoji (§8b.3 anti-aesthetic) | Default for every screen. Answers "where does this live" at a glance without a click. **PROVEN** the data is region-pinned; **HOUSE STYLE** that a quiet always-on token is the right calm/legibility balance. |
| **T1 on-hover detail** | Popover off the scope token | "This tenant's data resides in **EU (Frankfurt cell)**. Operated by `<operator>`. Keys: `<BYOK / platform-managed>`. No data leaves the EU/EEA by default." | When a user wants the *full* residency story (the DPO/security persona, P12/P13). Surfaces operator + key-control — the **"residency ≠ sovereignty"** distinction (sources below: who can decrypt, not just where). **PROVEN.** |
| **T2 cross-boundary warning** | Inline, at the action that would cross a boundary | A blocking/confirming banner: e.g. configuring an **outbound push-mirror to an extra-EU host** → "This target is outside the EU. PII-bearing content transfer is **denied by default** (`transfer_allowed`); permitted only with a recorded transfer mechanism + TIA." | Surfaces the **§5.3 outbound-mirror residency gate** at the exact moment of risk — HAX "convey consequences" applied to residency, not just erasure. **PROVEN** (the gate exists). |
| **T3 cross-cell provenance** | On a reference chip/unfurl that resolves to another cell | The unfurl carries a **residency tag** ("Lives in EU-West cell") and export stays in-region (R-04 §6.2 cross-cell row). | When a subject/artifact spans cells. The chip never *fetches* cross-region PII; it shows the cell + a PII-free pointer (gdpr-and-audit §4.3). **PROVEN.** |

**Design rule (HOUSE STYLE):** residency escalates *with risk*, never by default — T0 is calm,
T2/T3 appear only at a boundary. This is the calm-vs-legible resolution and it is the Axis-6 hinge
(§7). **Falsifiable:** if a DPO in the §9 review cannot find residency in ≤1 interaction from any
screen, the T0 cue failed; if engineers report the region token as noise, T0 is mis-tuned.

### 1.2 The per-artifact visibility chip (who/what can see this) `[UNDER-EVIDENCED]`

Distinct from residency (*where*), visibility answers *who/what can see a thing* (P9 question 2).
We reuse the **existing permission-aware reference chip** (§5.3) and the **`list-objects`
pre-filter** (ADR-03) rather than inventing a new control:

- **Visibility chip** sits on an artifact header (PR, issue, doc, channel, run): `Private` /
  `Team: Payments` / `Org` / `Public` — derived from the *effective* ReBAC tuples, not a guessed
  label. Clicking opens the **effective-access view** ("who can see/do what", §7.6 L626–628),
  which is the RBAC face over ReBAC — already a planned surface; we make it the chip's destination.
  **PROVEN** mechanism (ADR-03 effective access); **HOUSE STYLE** that a header chip is the surface.
- **Agent-scope sub-cue (HOUSE STYLE):** when an agent has standing access to an artifact class,
  the visibility view adds an **agent row** ("`ReviewerAgent` may read PRs in this repo, on behalf
  of @maintainer, until `<expiry>`") — sovereignty includes *agent* scope (P9 names "agent scope"
  explicitly, L117). This threads to the agent governance console (§6).
- **The privacy-by-default honest default (PROVEN, ADR-12):** new artifacts default to **Private**;
  the chip shows the honest default, not an aspirational "secure" badge. Opt-in telemetry and
  minimal retention are reflected the same way in the identity/profile surface.

> **Source grounding (residency cues):** data residency = *where* data lives; sovereignty = *who can
> access/decrypt it and under which legal framework* — "true sovereignty requires controlling not
> just where data lives, but who can decrypt it." Region badges should communicate **compliance
> status + operator + key control**, not just location.
> [CNCF — From data residency to digital sovereignty (2026-06-16)](https://www.cncf.io/blog/2026/06/16/from-data-residency-to-digital-sovereignty-architectural-patterns-for-cloud-native-platforms/);
> [DigitalApplied — AI Data Residency patterns 2026](https://www.digitalapplied.com/blog/ai-data-residency-architecture-patterns-2026);
> [Petronella — Sovereign by design: BYOK & data residency](https://petronellatech.com/blog/sovereign-by-design-byok-geo-fencing-and-data-residency-at-global/).
> The CLOUD-Act point (EU-region hosting by US vendors ≠ sovereignty) is in `competitive-landscape.md`
> §6.2 — which is *why* T1 names the operator and key control, not just the region.

---

## 2. The DSR orchestrator UI — the DPO view (locate / export / rectify / restrict / erase)

This **surfaces** the DSR orchestrator (gdpr-and-audit §4; the `PersonalDataHolder` five-op
contract §3.1) as the DPO's (P13) primary console (§7.6 L633–635). It is the UI realisation of the
R-04 §6 (F-GOV-1) blueprint — that flow is the spine; here is the screen-by-screen console.

### 2.1 Console structure (service blueprint → screens, method #8)

```
DSR console (§7.6 GDPR/data-rights)
├─ Request list        — all open/closed DSRs, each with a DEADLINE CLOCK (Art. 12(3))
├─ New request         — resolve subject → pick right(s) → scope → posture
└─ Request detail (the working surface) — five tabs over ONE subject:
   [ Locate ] [ Export ] [ Rectify ] [ Restrict ] [ Erase ]   + a persistent RECEIPT rail
```

The five tabs map **one-to-one** to the `PersonalDataHolder` operations (`locate / export / rectify
/ restrict / erase` = Arts. 15/20/16/18/17) — **PROVEN** (gdpr-and-audit §3.1). The console **never
reaches into a store**; it drives the orchestrator, which fans out via the holder contract (the
no-cross-store-read law, ADR-01/13). The UI makes this honest: every tab shows **per-holder rows**,
because the *holder list is the legal completeness unit*.

### 2.2 The screens, with the load-bearing UX moves

| Screen / tab | What it shows (surfaced mechanic) | The sovereignty-as-UX move |
|---|---|---|
| **Subject resolve** | Search by email/handle → `SubjectRef` + pseudonym map (Id 4.8). **Ambiguous-subject disambiguation, never auto-pick** (R-04 §6.2). | Resolution is **DPO/admin-role-gated**; others get a clean no-access (P9: the console itself is permission-aware). |
| **Locate (inventory)** | **"Everywhere this subject appears"** — the *generated data map* drives the fan-out (gdpr-and-audit §2.2/§4.1), so "we forgot the search index" is structurally impossible. Per-holder rows (H1–H18): Git, CI, Issues, Knowledge, Chat, Search, Refs, Bus-history, Agent-memory, Authz, Identity, Audit-carveout… | This is the literal answer to **"show me everything about this subject"** (P9 q3). **PROVEN** the inventory is map-driven; **HOUSE STYLE** that per-holder rows (not a flat list) are how a DPO reads *completeness*. |
| **Locate — completeness & partial failure** | **Per-holder progress** ("9/10 holders complete · Chat holder timed out — retry"). **Never a silent partial** (R-04 §6.2). Cross-cell rows tagged with their cell + residency, export stays in-region. | **The completeness guarantee IS the product** here. A DPO trusts a DSR only if it visibly enumerates every holder and shows what's outstanding — completeness is the legal point, not a nicety. **PROVEN** (saga-style resumable fan-out, §4.1). |
| **Export** | `PortableBundle` per holder (Art. 20): JSON/CSV structured, git via clone, docs as Markdown (§4.4). | One **download with a manifest** of what's included per holder + a **Merkle-proof** the bundle is unaltered (eDiscovery substrate, §5.4). "Proof, not promise." **PROVEN.** |
| **Rectify** | `rectify(subject, patch)` corrects the **primary store + reindex-from-source** to derivatives (§4.4) — never patch-in-place-and-drift. | The UI shows the patch *propagating* (primary → derivatives) so the DPO sees rectification reach Search/Refs, not just the source row. **PROVEN.** |
| **Restrict** | `restrict(subject, on)` sets the per-subject suppression every holder honours: **no indexing, no agent-use, no analytics (incl. OLAP), no notification**, storage retained, reversible (Arts. 18/21; §4.4). | A prominent **"Restricted" banner** on the subject + a checklist of *what suppression now covers* (esp. **agent-use OFF** and **OLAP OFF** — the two a regulator probes). Reversible toggle, audited. **PROVEN.** |
| **Erase — consequence dialog** | Before fan-out: **state the consequence first** (HAX "convey consequences"). Shows exactly **what crypto-shreds (self-authored, per-subject DEK) vs. what tombstones (Refs) vs. what keeps pseudonymised authorship (git/audit) vs. the documented third-party residual** (§7.1–§7.3). | This is the **keystone HAX moment**: erasure is irreversible and legally consequential, so the dialog is *explanatory, not a confirm*. It names the `[OPEN — LEGAL]` residual honestly ("a name another user typed into their own message is handled under the documented lawful-basis limit + restrict suppression"). **PROVEN** mechanics; the *dialog framing* is **HOUSE STYLE**. |
| **Erase — fan-out progress** | Per-holder, **failure-isolated saga** ("Search re-index failed — other holders' erasure stands; retry Search"), idempotent, **resume-after-crash** (§4.1 canonical order: Id pseudonym-map → KMS DEK destroy → Search purge+reindex → Refs tombstone → Bus → notif/authz/agent-memory → receipt). | Partial-holder failure is a **designed state**, never a 500 — a regulator would catch a silent partial. **PROVEN** (the saga + canonical order exist). |
| **Receipt rail (persistent)** | The verifiable receipt: `sign(hash(request ∥ holder ∥ scope ∥ outcome ∥ key_epoch_destroyed ∥ ts))`, sealed into the per-tenant audit Merkle tree (§4.2). Records **key epoch destroyed** + **purge cursor** so "we erased it" is independently checkable against the KMS + index. | **"Verifiable, not asserted."** The receipt links into the audit explorer (§4) and exports as a tamper-evident completion certificate. This is the single artifact a supervisory authority / Art. 28 audit verifies. **PROVEN.** |
| **Deadline clock (on every DSR)** | Durable `myelin-flow` timer = `now + 1 month`, extendable to 3 for complex (recorded reason); nearing-deadline emits a warning Signal (§4.1). | The clock is **always visible and escalates** (calm → amber < 1 week → surfaced in the DPO inbox < 72h, R-04 §6.2). **PROVEN** the timer exists; **HOUSE STYLE** the escalation ladder. |

> **Source grounding (DSR/DSAR UX):** the **one-month deadline** (calendar days, extendable +2
> months for complex), and that a DSAR log must record **request date, identity-verification, search
> scope, exemptions, delivery date** — *"without timestamped, tamper-proof records, compliance
> assertions are just claims; cryptographic evidence becomes verifiable proof."* This is exactly the
> receipt rail's job.
> [GDPRLocal — DSAR rules & deadlines](https://gdprlocal.com/dsar-rules-and-deadlines/);
> [PrivacyCache — DSAR response deadlines](https://privacycache.com/blog/gdpr-dsar-response-deadlines-guide);
> [WatchDog — DSAR handling / Art. 12 modalities](https://watchdogsecurity.io/gdpr/data-subject-access-request-dsar-handling);
> [Complydog — DSAR complete guide](https://complydog.com/blog/dsar-complete-guide-data-subject-access-requests-gdpr).
> **Tenant-operability (Art. 28):** the console is operable **by Myelin (controller data) and by/for
> tenants (processor assistance)** — gdpr-and-audit §4.4. The tenant self-service UX is an
> `[OPEN → P6]` engineering item there; this file specifies the *shape*, P6 builds the tenant lens.

---

## 3. The DSR — the data-subject view (the same graph, own-scope)

The **D4 dual-audience pair** (R-04 §6.2): the DPO sees everyone-in-scope; the data subject sees
**their own data only**. Same inventory mechanic, two lenses — proving neither is a degraded
compromise is **R-16's** job; here we draw the subject lens. `[UNDER-EVIDENCED]` — consumer privacy
self-service portals exist, but a *unified-platform* subject view over five subsystems has no
external precedent.

| Subject capability | Surface | Mechanic surfaced |
|---|---|---|
| **"Show me everything you hold about me"** | A self-service privacy page (identity/profile area, §7.6) → the same per-holder inventory, **own-scope only** | `locate(self)` via the holder contract; permission-pre-filtered to self (ADR-03). **PROVEN.** |
| **Download my data** | One portable bundle, with manifest + integrity proof | `export(self)` (Art. 20). **PROVEN.** |
| **Request correction / erasure / restriction** | Buttons that **open the DPO-side DSR flow** (don't self-execute destructive ops) | Subject *requests*; the DPO/tenant *adjudicates* posture (controller vs processor, §4.1). The subject never directly crypto-shreds. **PROVEN** posture; **HOUSE STYLE** the request→DPO handoff UX. |
| **See where my data lives** | The residency tier T1 (§1.1), scoped to self | Cell/region + operator + key control. **PROVEN.** |
| **Already-erased state** | "No retained personal data for this subject" + prior-receipt link (R-04 §6.2) | The tombstone-as-honest-default (§5). **PROVEN.** |

**HOUSE STYLE rule:** the subject view is **read + request**, never **erase-others / adjudicate** —
the power asymmetry is *visible in the UI* (the subject cannot see a "fan out erasure" button at
all; it is not just disabled). This is privacy-by-default expressed as affordance, not copy.

---

## 4. The audit-log explorer (provenance / correlation threading)

Surfaces the **tamper-evident audit log** (gdpr-and-audit §6; one log records **every human AND
agent action**, agents through the same path as humans). The §7.6 surface "audit log explorer …
with provenance/correlation threading" (L631–632). **PROVEN** substrate throughout; the *explorer
UX* is HOUSE STYLE.

### 4.1 What the explorer must make legible

| Capability | Surfaced mechanic | UX move |
|---|---|---|
| **Search who-did-what** | Per-tenant hash-chain + Merkle tree; entries minimised (`actor`/`on_behalf_of`/`subject` are **pseudonyms / `ArtifactRef`s, never payloads**, frozen grammar `<pseudonym>@<tenant>.noreply`, §6.3) | Faceted search (actor / artifact / action-kind / time). Renders **humanised strings** (no raw IDs, §8b.5): "`@dana` transitioned ISSUE-413 → Done · 2026-06-18 14:02". |
| **"Why did this happen?"** (provenance walk) | `correlation_id` / `causation_id` carried on every entry — *the audit log IS the why-walk* (§6.3); one mechanism for audit + provenance + the agent loop-guard | A **causal thread view**: click any action → see the chain that caused it (the F-AGT-1 agent chain renders as one `correlation_id` thread across CI→Issues→Chat→Git, R-04 §7). This is the R-22 "one correlation_id across surfaces" wedge, *made inspectable*. |
| **Agent attribution** | An agent's *applied effect* lands in the audit log like a human's; its *reasoning* lands in the distinct **agent execution trace** (H17, §6.5) | Agent actions carry the **`agent` treatment** (badge, color-blind-safe, never colour-alone, R-14) + "on behalf of `<delegator>`" + a link to the run trace. The three holders (telemetry / trace / audit) are **kept visibly distinct** so none masquerades as another. |
| **Tamper-evidence as a UX claim** | CT-style **inclusion + consistency proofs**; signed tree heads anchored to an independent witness (RFC 6962; Crosby & Wallach 2009; §6.1) | An explorer affordance: "Verify this entry" → shows the inclusion proof; "Verify the log wasn't rewritten" → consistency proof between two STHs. **Provable, not just displayed.** **PROVEN.** |
| **The erasure-vs-audit honesty** | Audit is a **carve-out holder** (H16): on erasure, Id's pseudonym shred already ran, so the audit retains only the opaque-pseudonym minimised record, then expires via audit-key crypto-shred (§6.4) | The explorer **never silently rewrites** an entry (that breaks the chain); an erased subject's past actions show as a **pseudonymised actor**, the *fact* preserved, the *identity* gone. The UI states this is by design (delete-the-identity-not-the-fact). **PROVEN.** |

> **Source grounding (audit explorer):** an audit trail is *"a tamper-evident record of who did
> what, when, where, and why,"* linking each action to an accountable identity + timestamp;
> provenance = reconstructing the history to find root cause via **causal dependency** (A causes B);
> 2025 best practice = consistent who/what/when/where/why events + **correlation IDs** + append-only
> /immutable + cryptographic signing for chain-of-custody.
> [Spendflo — audit trail guide 2025](https://www.spendflo.com/blog/audit-trail-complete-guide);
> [Galileo — AI agent compliance & audit trails 2025](https://galileo.ai/blog/ai-agent-compliance-governance-audit-trails-risk-management);
> [UC Berkeley — security audit log analysis guideline](https://security.berkeley.edu/security-audit-log-analysis-guideline).
> The Myelin substrate (Merkle/CT, witness, non-blockchain) is gdpr-and-audit §6.1 — we surface it.

---

## 5. The data-map / RoPA & residency console; and the erased/tombstoned UX

### 5.1 Data-map / RoPA & residency console (§7.6 L636–638)

Surfaces the **generated data map** — a build step walks every `#[personal_data(...)]`-tagged field
+ every registered holder and **generates** the machine-readable inventory (PII, where, role/basis
/category, retention, locator, residency), **regenerated every build and diffed in CI** (§2.2). The
DPO sees any reclassification. **PROVEN.**

| Console view | Surfaced mechanic | UX move |
|---|---|---|
| **Data map** | The generated inventory grouped by holder/field | "What PII exists, where, under what basis, retained how long" — a *living* table the DPO trusts because it **can't drift** (the `no-untagged-personal-data` lint fails the build, §2.1). The UI shows the **CI diff** ("3 fields reclassified since last release") so the DPO reviews drift. |
| **RoPA (Art. 30)** | A projection grouped by **processing activity** (§2.2/§2.3 G7), generated-then-DPO-reviewed | Each activity row: purpose · categories · basis · retention · recipients · transfers · residency. Exportable for a supervisory authority. **The RoPA legal text is `[OPEN — LEGAL]`** — the DPO ratifies the characterisation; the platform generates the *substrate*, not the legal prose. |
| **Residency view** | "Where does this tenant's data live" — cells/regions, isolation tier, retention policy (§7.6 tenant/cell settings; ADR-11) | The macro answer to question 1, aggregating the §1 cues to tenant scope. Shows the **outbound-transfer gate** state (extra-EU denied by default, §5.3) and any recorded TIAs. |
| **Lawful-basis / consent / sub-processor registries** | `consent` (G5, per-subject, versioned, withdrawable), `subprocessor_registry` (G6, per-region + DPA ref + change-notification/objection) (§5.2) | Sub-processor changes surface a **notification + objection workflow** to the tenant — sovereignty as an *ongoing* relationship, not a one-time badge. **PROVEN.** |
| **DPIA gate** | Fires on a data-map diff introducing a new `SpecialCategory` flow, a new agent capability over personal data, or large-scale monitoring (§2.3) | The console **prompts a DPIA** when the diff trips the gate — the platform surfaces the obligation; the DPO adjudicates. The **worklog/productivity sensitivity** (OQ-H) and its **works-council consultation trigger** surface here, `[OPEN — LEGAL]`. **PROVEN** the gate; legal call deferred. |

> **Source grounding (RoPA):** RoPA (Art. 30) = a written record of every way data is collected/
> used/stored/shared/protected, by department + legal basis, with **version history + timestamped
> change tracking** and **exportable reports for supervisory authorities** — and increasingly linked
> to risk/DPIA. Our generated-then-reviewed, CI-diffed model matches this.
> [Securiti — what is RoPA](https://securiti.ai/blog/what-is-ropa/);
> [Secure Privacy — Process Register / Art. 30](https://support.secureprivacy.ai/article/governance-core-module-process-register/);
> [Irish DPC — RoPA guidance note (PDF)](https://www.dataprotection.ie/sites/default/files/uploads/2023-04/Records%20of%20Processing%20Activities%20%28RoPA%29%20under%20Article%2030%20GDPR.pdf).

### 5.2 The erased / tombstoned UX — the GDPR-aware degraded state (§5.3, ADR-12)

The reference chip/unfurl (R-09 owns it in depth) **tombstones gracefully** on erasure — "never a
dangling leak" (design-language §5.3). R-19 specifies the *sovereignty meaning* of each degraded
state, because **the degraded state is where sovereignty is most felt** (a leak here is the failure
mode P9 exists to prevent):

| State | What the user sees | Why (mechanic) |
|---|---|---|
| **Tombstoned artifact** | "This item was erased on `<date>`" — type-shaped placeholder, the *edge preserved for integrity*, never the content/title | Refs tombstones nodes/edges (H12); the backlink survives as a projection, the payload is gone. **PROVEN.** |
| **Pseudonymised author** | Erased author shows as an opaque pseudonym; the commit/comment **stands**, authorship neutralised | Id pseudonym-map shred (delete-identity-not-fact); git history holds only pseudonyms (§7.1). **PROVEN.** |
| **No-access (not erased)** | Graceful "you don't have access — request access" card, **never the title** | `list-objects` pre-filter / per-viewer check (ADR-03). The UI **does not distinguish** "erased" from "you can't see it" *to an unauthorised viewer* — that distinction itself can leak existence. **HOUSE STYLE / PROVEN-adjacent** (the non-leak invariant is PROVEN; the conflation choice is ours). |
| **Restricted (Art. 18)** | Visible to the holder/DPO as "Restricted — suppressed from indexing/agent/analytics/notification," retained but inert | `restrict` flag honoured by all holders incl. OLAP (§4.4, contract 11.6). **PROVEN.** |
| **Crypto-shredded body** | "Content no longer available (erased)" where a self-authored body was DEK-shredded | Per-subject DEK destroyed → ciphertext unrecoverable, incl. backups (§7.1). **PROVEN.** |

**The honesty rule for tombstones (HOUSE STYLE):** a tombstone **states erasure happened** (to a
viewer who may see the edge) rather than silently vanishing — accountability beats tidiness. But it
**never reveals what was erased or about whom** beyond the date. This trade (legibility of *the
fact* vs. protection of *the content*) is the sovereignty-as-UX judgment call; `[DEFERRED]` whether
a DPO finds it sufficient and a subject finds it respectful.

---

## 6. The agent governance / kill-switch surface (sovereignty over agents)

Sovereignty explicitly includes **agent scope** (P9, L117). Surfaces the **agent governance
console** (§7.6 L629–630; §6.4) — which agents exist, identities/scopes/delegation/budgets,
autonomy policy, kill switches, agent audit. R-15 owns the full provenance/calm-volume spec; R-19
records only the **sovereignty-facing** controls:

- **Scope/delegation inspector** — for each agent: what it may do, on whose authority, on which
  artifact classes, until when (ties to the §1.2 agent-scope visibility sub-cue). Agent perms =
  human-perms ∩ delegation ∩ tenant (R-04 §7.1). **PROVEN** mechanic.
- **Budget / autonomy policy** — per-agent effect/cost/wall-clock caps; the **budget-exceeded** and
  **loop-guard-tripped** states (R-04 §7.2) are *governance-visible*, not just runtime errors.
- **Kill-switch (per-tenant, per-agent)** — a prominent, audited control to halt an agent or all
  automation. **PROVEN** (governance console G4). The UI treats it as a **calm, reachable** control
  (not buried, not alarmist) — a regulated buyer asks "can we stop it?" and the answer is one
  screen away.
- **Agent action audit** — agents' applied effects are in the **same audit log** as humans (§4);
  the governance console deep-links into the audit explorer filtered to that agent.

**`[UNDER-EVIDENCED]`** — agentic-governance UX is nascent. 2025 sources confirm AI-agent actions
*should* be audit-trailed with correlation IDs and accountable identity
([Galileo, 2025](https://galileo.ai/blog/ai-agent-compliance-governance-audit-trails-risk-management)),
which our same-audit-path-as-humans posture satisfies; but the *console UX* for agent sovereignty
has no settled pattern.

---

## 7. Axis 6 — the always-on-cues ↔ on-demand-consoles trade-off (the core design tension)

This is the **defining R-19 contribution to the sketch funnel** (sketch-funnel Axis 6, L74–80).
P9 demands sovereignty be *felt*; P8 demands attention be sacred. **How much sovereignty is ambient
vs. summoned is a real, axis-worthy choice** for the regulated/public-sector buyer.

### 7.1 The two poles, with what each gets right and wrong

| | **Always-on cues** (ambient) | **On-demand consoles** (summoned) |
|---|---|---|
| **What it is** | Residency token persistent in the shell (§1.1 T0), visibility chip on every artifact header (§1.2), agent badges everywhere | Sovereignty lives in excellent dedicated consoles (DSR/RoPA/residency/audit, §2–§6), reached when needed; daily UI stays clean |
| **Serves** | P9 maximally — sovereignty is *unavoidably* visible; reassures the regulated buyer at every glance | P8 maximally — calm daily surface; depth on demand for the persona who needs it (DPO/admin) |
| **Risks** | **Cue fatigue** — badges become wallpaper, stop being read; clutters the engineer's dense surface (P1/P5 tension) | Sovereignty becomes *de facto* "fine print" again — the exact P9 failure ("buried in settings") if the console is the *only* place it lives |
| **Rubric D9** | Scores high on "cues present where data is" | Scores high on "a DPO trusts it at a glance" *in the console* — but risks 0 on "not fine print" |

### 7.2 Our recommended resolution (HOUSE STYLE — the calibrated middle)

**A risk-calibrated hybrid: ambient at T0 (calm), escalate to console on risk or on the
governance persona.** Concretely:

1. **Always-on but minimal:** only **two** ambient cues survive the calm budget — the **shell
   residency token** (T0) and the **per-artifact visibility chip**. Both are quiet, neutral-weight,
   non-accent (§8b.3). Everything else (lawful basis, retention, sub-processors, audit) is
   **on-demand** in consoles.
2. **Escalate ambient → inline at the boundary:** residency/transfer warnings (T2) appear *only* at
   the action that crosses a boundary — ambient where safe, loud where risky.
3. **Persona-adaptive density (ties to R-16 / §2 dual-audience):** the **engineer** sees T0 + the
   visibility chip and nothing more; the **DPO/admin** gets the consoles surfaced in their nav and a
   richer scope indicator (T1 inline). Same data, role-adapted visibility — *configuration, not a
   fork* (the persona-adaptive mechanism). This lets a single product occupy a **deliberate Axis-6
   position per persona** rather than one global setting.

> **Phase-6 instruction (HOUSE STYLE, actionable):** sketch the **corporate/governance approachable
> surface** (sketch-funnel L98) at **both poles** — (a) a maximally-ambient shell where residency/
> visibility cues are everywhere, and (b) a maximally-clean shell whose sovereignty lives in a
> beautiful DSR/RoPA console — so the human reviewer sees the Axis-6 edges, not just our recommended
> middle. The governance console can itself be the "approachable corporate surface" a finalist must
> include. **`[DEFERRED-UNTIL-USERS]`** which pole the regulated buyer actually trusts more — §9.

---

## 8. Completeness-critic (README §9) — the gloss-risks R-19 owns

| §9 gloss-risk | Status | Where |
|---|---|---|
| **DSR from the data-subject side AND the DPO side** (the prompt's explicit dual mandate) | **OWNED & covered** | §2 (DPO) + §3 (subject); both over one graph; power-asymmetry made visible |
| **Erased / tombstoned GDPR-aware degraded state** | **OWNED & covered** | §5.2 (five degraded states + the tombstone-honesty rule); R-09 owns the chip mechanics |
| **Permission-denied never leaks** | **covered** | §1.2, §5.2 (no-access ≠ erased, non-leak invariant, ADR-03) |
| **Cross-cell residency in a DSR / on a ref** | **covered** | §1.1 T3, §2.2 (cross-cell inventory rows, in-region export) |
| **Partial-holder-failure (the branch a happy-path DSR demo skips, a regulator catches)** | **covered** | §2.2 (failure-isolated saga, per-holder progress, never-silent-partial) |
| **Agent scope as a sovereignty concern** | **covered** | §1.2 agent sub-cue, §6 governance/kill-switch |
| **Sovereignty honestly tagged under-evidenced where HOUSE STYLE** | **covered** | §0 frame + `[UNDER-EVIDENCED]` tags concentrated in §1/§3/§5/§6 |
| **`[OPEN — LEGAL]` residual not pretended-solved** | **covered** | §2.2 erase dialog + §5.1 DPIA/worklog name the residual honestly |
| **Touch/mobile sovereignty layout; per-jurisdiction console variation** | **consciously deferred** | to R-21 (state-craft) / R-18 (i18n — RoPA/consoles must survive long-word + RTL) and P6 legal ratification; named, not hidden |

---

## 9. `[DEFERRED-UNTIL-USERS]` — the regulated-buyer (P13/P14) review plan

**This file is an expert blueprint + heuristic audit (the no-user substitute), NOT a validated
design.** Sovereignty-as-UX is the one area where **user testing is replaced by a DPO/procurement
review** (README §5.7; ws-i). Recorded as a concrete, executable plan:

- **What to put in front of them (the artifacts):** (1) the DSR console — locate→export→erase
  with the consequence dialog + receipt rail; (2) the audit explorer with a real `correlation_id`
  provenance walk (the F-AGT-1 agent chain); (3) the data-map/RoPA console with a CI drift diff;
  (4) the residency cue ladder (T0–T3) on a normal engineer screen *and* a governance screen;
  (5) the erased/tombstoned states; (6) **both Axis-6 poles** (§7.2) side by side.
- **With whom:** **P13 (DPO)** + **P14 (procurement/legal buyer)** primarily; **P12 (security)** for
  the agent-governance/kill-switch and audit-tamper-evidence; recruit **2–3 real DPOs across an
  EU SMB, a regulated enterprise, and a public-sector body** (the three Axis-6 buyer profiles).
  Run **jointly with the R-04 §11 F-GOV-1 walkthrough** (same artifact, same recruits).
- **The method:** a structured **trust-at-a-glance review** + think-aloud on three tasks —
  (T1) "answer a DSAR for this departed contributor and prove it's complete"; (T2) "show me where
  this tenant's data lives and what leaves the EU"; (T3) "show me everything an agent did last
  week and who authorised it." Score each on the **P9 three-questions heuristic** (§0).
- **What would FALSIFY our hypotheses (the bar):**
  1. **"A DPO trusts this at a glance" (rubric D9 = 4) is FALSE** if a DPO cannot answer the three
     P9 questions in ≤1 console each, or does not believe the completeness claim without a manual
     cross-check (the per-holder inventory failed to convey completeness).
  2. **The receipt is not trusted as proof** — if a DPO would still demand an external audit rather
     than accept the Merkle-proven receipt, "verifiable, not asserted" failed.
  3. **The Axis-6 middle is wrong** — if regulated buyers strongly prefer one pole (e.g. always-on
     cues read as "serious," consoles read as "hidden"), our §7.2 hybrid is mis-calibrated.
  4. **Ambient cues are noise** — if engineers (P1) report T0/visibility chips as clutter while
     DPOs report them as essential, the persona-adaptive density (§7.2) is the only viable answer
     and a single global Axis-6 position is falsified.
  5. **The tombstone honesty rule backfires** — if subjects find "erased on `<date>`" tombstones
     *less* respectful than silent removal, the §5.2 trade-off must flip.
- **Caveat (carried from R-04 §11):** the DSR/agent flows are drawn against the **mock** runtime.
  The *contract* (map-driven fan-out, verifiable receipts, restrict-suppression, same-audit-path)
  is designed to be trustworthy **regardless of runtime** — that is what the review validates, not
  the mock's specific outputs.

> **Why a DPO review substitutes for users (PROVEN):** DPOs evaluate privacy tooling on **real-time
> compliance visibility, deadline management, evidence/accountability for regulators, and DSR/DPIA
> workflow automation** — exactly the surfaces here; "DPOs cannot function with manual processes."
> A regulated-buyer review is the realistic proxy for "does this earn trust."
> [Secure Privacy — privacy governance software for DPOs](https://secureprivacy.ai/blog/privacy-governance-software-for-dpos);
> [IAPP — DPO compliance tool](https://iapp.org/resources/article/data-protection-officer-compliance-tool/).

---

## 10. Actionability toward the control artifacts

| Control artifact | What R-19 equips | Where |
|---|---|---|
| **rubric.md D9** (sovereignty/GDPR-as-UX legibility, 8%) | Makes D9 *checkable*: the P9 three-questions heuristic (§0) is the scoring probe; residency cues placed near data (§1); DSR both sides (§2–§3); audit provenance (§4); the "DPO trusts at a glance" bar is the §9 falsification test, not aspiration | §0, §1–§6, §9 |
| **sketch-funnel Axis 6** (always-on cues ↔ on-demand consoles) | The trade-off articulated with both poles' merits/risks + a recommended persona-adaptive hybrid + the **explicit Phase-6 instruction to sketch both poles** | §7 |
| **sketch-funnel comparable screens** | The **DSR console / governance console** is specced concretely enough to be a finalist's "approachable corporate surface"; the residency cue ladder applies to *every* finalist's shell | §1, §2, §7.2 |
| **R-09 (chip/unfurl)** | R-19 specifies the *sovereignty meaning* of each tombstone/no-access state R-09 renders | §5.2 |
| **R-15 (agent attribution/calm)** | R-19 records the sovereignty-facing agent controls; R-15 owns the full provenance + calm-volume + trust-calibration spec | §6 |
| **R-16 (dual-audience)** | The DSR D4 pair + persona-adaptive Axis-6 density are dual-audience surfaces R-16 critiques per-lens | §3, §7.2 |
| **R-18 (i18n)** | Flag: RoPA/consoles/audit strings must survive German expansion + RTL (deferred there, named here) | §8 |

---

## 11. Self-check against R-19 acceptance criteria

| Criterion (prompt R-19 / ws-i) | Status | Evidence |
|---|---|---|
| **Residency/visibility cues placed concretely near data** | ✅ Met | §1.1 four-tier residency ladder on the shell scope indicator (the existing §5.1 element); §1.2 per-artifact visibility chip + agent sub-cue |
| **DSR console blueprinted from BOTH data-subject and DPO sides** | ✅ Met | §2 (DPO: 5 tabs = 5 holder ops, consequence dialog, receipt rail, deadline clock); §3 (subject: own-scope, request-not-execute, visible power-asymmetry) |
| **Data-map / RoPA & residency console** | ✅ Met | §5.1 (generated, CI-diffed data map; RoPA by processing activity; residency view; consent/sub-processor; DPIA gate) |
| **Audit-log explorer surfaces provenance/correlation** | ✅ Met | §4 (who-did-what search; `correlation_id` causal-thread walk; agent attribution; inclusion/consistency proofs as a UX claim) |
| **Erased/tombstoned UX specified** | ✅ Met | §5.2 (five degraded states + the tombstone-honesty rule + no-access≠erased non-leak) |
| **Agent governance / kill-switch surface** | ✅ Met | §6 (scope/delegation inspector, budget/autonomy, per-tenant kill-switch, agent audit deep-link) |
| **Axis-6 trade-off articulated (always-on ↔ on-demand)** | ✅ Met | §7 (both poles' merits/risks + recommended persona-adaptive hybrid + Phase-6 sketch-both-poles instruction) |
| **Patterns SURFACE existing mechanics, don't invent** | ✅ Met | Every mechanic cited to gdpr-and-audit §§2–7 / system-overview §8.3 / ADR-03/11/12/13; tagged PROVEN where surfaced |
| **Tag PROVEN vs HOUSE STYLE; honestly mark under-evidenced** | ✅ Met | §0 frame; `[UNDER-EVIDENCED]` on §1/§3/§5/§6 HOUSE-STYLE UX; PROVEN on all legal/architectural claims with cited URLs |
| **Build ON R-04 (F-GOV-1), don't duplicate** | ✅ Met | §2 realises the R-04 §6 blueprint as console UX; references it, doesn't re-draw the flow |
| **Deferred regulated-buyer (P13/P14) review recorded as a plan, not faked** | ✅ Met | §9 (artifacts, recruits, method, 5 falsification bars, mock-runtime caveat) |
| **Completeness-critic §9 gloss-risks (DSR both sides etc.)** | ✅ Met | §8 (owned: DSR both sides, tombstone, partial-holder-failure, cross-cell, agent scope; deferred: touch/jurisdiction, named) |
| **Date; cited current (2024–2026) web sources** | ✅ Met | Dated 2026-06-20; sources cited inline in §1/§2/§4/§5/§9 (CNCF 2026, GDPRLocal, Securiti, Galileo, Microsoft HAX, IAPP, etc.) |

**Honest partials / top uncertainties.**
1. **The whole UX layer is `[UNDER-EVIDENCED]`** — there is no external playbook for sovereignty-as-
   UX (§0). The legal/architectural floor is PROVEN; the *legibility* is HOUSE STYLE and stands or
   falls on the §9 DPO review. This is the corpus's single most under-evidenced deliverable, by
   design (it is the defining differentiator, rubric D9).
2. **"A DPO trusts it at a glance" is the unproven keystone** — D9's top score depends on it; only
   §9's falsification tests resolve it. Until then it is a HYPOTHESIS, not a result.
3. **The Axis-6 calibration (§7.2) is a bet** — the persona-adaptive hybrid is reasoned, not
   measured; the funnel sketching both poles is the de-risking move.
4. **The tombstone-honesty trade-off (§5.2) and the receipt-as-proof claim (§2.2)** are HOUSE STYLE
   judgment calls a regulator/subject could reject; both are in the §9 falsification set.
5. **`[OPEN — LEGAL]` residuals** (third-party free-text PII, audit carve-out scope, worklog
   special-category, Art. 17 reach into immutable git bytes) are surfaced honestly but **not
   resolved by us** — counsel/DPO ratify; the structural floor ships regardless (gdpr-and-audit §7).

---

*End of R-19 deliverable. Date: 2026-06-20. Legal/architectural claims PROVEN (GDPR arts. + cited
sources + gdpr-and-audit/system-overview mechanics surfaced, not invented); all sovereignty-as-UX
legibility choices HOUSE STYLE and flagged `[UNDER-EVIDENCED]`; no design user-validated — the
regulated-buyer review is the deferred test (§9). Feeds rubric D9 + sketch-funnel Axis 6; builds on
R-04 F-GOV-1; hands sovereignty-facing agent controls to R-15, dual-audience lenses to R-16,
tombstone-meaning to R-09, console i18n to R-18.*
