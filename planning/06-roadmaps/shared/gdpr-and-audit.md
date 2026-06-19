# Phase 6 — Roadmap: GDPR / Audit (`myelin-gdpr`)

> Phase: `06-roadmaps/shared`. The detailed sequenced roadmap for the **gdpr-and-audit** shared system.
> Slots into the master sequencing bands M0..M6:
> [`../00-master-sequencing.md`](../00-master-sequencing.md) (§1 ordering thesis — Tier 1 silent-data-loss +
> Tier 3 the `no-untagged-personal-data` lint + the X-7 structural-floor decision before the git data model
> freezes; §2 bands; §3 critical-path/DAG; §4 the gate invariant; §5 name-your-floors). Frozen architecture
> (this roadmap SEQUENCES, it does not redesign):
> [`../../05-refined-shared-systems-architecture/gdpr-and-audit.md`](../../05-refined-shared-systems-architecture/gdpr-and-audit.md)
> (the refined GDPR/Audit architecture — the `PersonalDataHolder` spine §3, the DSR orchestrator §4, the
> tamper-evident audit log §6, the ONE free-text/immutable erasure posture §7) + the refined
> [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> §10 (the contracts GDPR/Audit owns) + §1/§2/§4/§5/§6/§7/§9/§11/§12 (the contracts it consumes). Drills owed:
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> §4.2 (GA-D1..GA-D8 + the NEW GA-10/GA-11) + the cross-owner erasure/restriction instances that ride GDPR
> (STOR-D3/D4, ID-D8, SRCH-D4, REF-D5, NOTIF-D6, AG-D10, FLOW-D9, GIT-D2, CI-D3, KN-D4/D12, ISS-D11, CHAT-D8)
> + the E2E-3 audit-tamper leg + the E2E-4 DSAR fan-out (the whole-system GDPR proof). Doctrine:
> [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (§2 order-by-non-negotiability; §3 prove-it-or-it-isn't-real; §5 the committed gates; §1 name-your-floors,
> code-wins-over-docs) and
> [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) §1
> (erasure-vs-immutability — GDPR owns POLICY+ORCHESTRATION, Storage owns the crypto-shred MECHANISM, Identity
> owns the pseudonym-map shred LEVER). Spine: ADR-12 (PersonalDataHolder spine), ADR-11 (cells/residency),
> ADR-17 (fail-static / GD-3), ADR-18 (restore-verify / post-restore re-erasure). Date: 2026-06-19.
>
> **The shape of this system, and what that means for sequencing.** GDPR/Audit is a **policy + orchestration**
> layer that **owns no mechanism** (arch §1.2): the crypto-shred mechanism is Storage's (KMS/per-subject DEK,
> 11.3/11.4), the pseudonym-indirection lever is Identity's (4.8), every store's erasure is *that store's*
> `PersonalDataHolder::erase`. GDPR decides **whether, when, and prove**; the owning store decides **how**.
> Three consequences flow from that single fact and dominate this roadmap:
> 1. **Its build is split structural-then-orchestral-then-fan-out across M1 → M5, mirroring the holder list
>    coming online.** The `PersonalDataHolder` **trait + harness auto-registration + the `no-untagged-personal-data`
>    lint + the `#[personal_data]` derive + the data-map generator + the erasure ledger** are the **M1
>    structural spine** — they must exist before any store writes real data (Tier 1: the data map is what makes
>    "we forgot a store" structurally impossible). The **DSR orchestrator + the tamper-evident audit log + the
>    retention/consent/sub-processor engine** light up across M1/M2 as the timer wheel and outbox they ride
>    come online. The **full erasure fan-out across all H1–H18 holders** cannot be *complete* until every holder
>    exists — so GA-D1 (0 holders missed) is an **M5** gate, even though every *piece* of it ships earlier.
> 2. **It is on the critical path only through one decision: the X-7 structural-floor posture must be frozen in
>    M1, before the Git data model freezes in M3.** Pseudonymous-by-default commit identity (the answer to
>    erasure-vs-immutability for git author bytes) is a *commit-time prerequisite* — decided here, consumed by
>    Git. Everything else GDPR builds is *downstream* of the systems it orchestrates, not upstream of them.
> 3. **Almost every GDPR drill is SCHED, not CI** — erasure-reaches-every-holder, crypto-shred-reaches-backups,
>    multi-cell fan-out, audit-tamper, restore-resurrects-nothing are expensive whole-cell drills. The **one
>    CI-cheap GDPR gate is GA-D5** (the `no-untagged-personal-data` lint goes red on an untagged PII field) +
>    **GA-D7** (restriction suppression) — the ratchet floor that ships in M0/M1 and stays green forever.
>
> The corollary that orders the work *inside* GDPR: the **structural guarantee ships regardless of legal
> ratification** (the entire residual lawful-basis story is `[OPEN — LEGAL]`, but per-subject DEK crypto-shred
> + pseudonym-map shred + `restrict` suppression + the tamper-evident audit log are *engineering* and ship on
> the engineering clock). Counsel/DPO ratification runs *in parallel* (it is decision-shaped, EI-01 §8 — a
> sketch + sign-off, not autonomous) and gates **publishing a posture as ratified**, never building the floor.

---

## 0. Where GDPR/Audit lands in the master bands (the one-paragraph map)

GDPR/Audit's **structural spine is M1** (master-sequencing M1 work: "the GDPR/Audit spine, structural half" —
the `PersonalDataHolder` trait + harness auto-registration, the `#[personal_data]` classify-derive + the
`no-untagged-personal-data` lint targets, `data_map()/ropa`, the erasure ledger; the per-subject-DEK
crypto-shred + pseudonym-map shred structural floor built now per X-7). But GDPR is **named and partially
shipped in M0**: the `no-untagged-personal-data` lint is one of the twelve committed lints (with its red+green
fixtures) and the `PersonalDataHolder` auto-registration hook (contract 1.4) is part of the harness contract,
frozen in M0 so every store that opens in M1+ auto-registers. In **M1** the structural spine + the DSR
orchestrator skeleton + the tamper-evident audit log land: the trait, the derive, the data-map generator (CI-
diffed), the erasure ledger, the DSR state machine (over the M0 outbox + the timer wheel — note the durable
timer is M2, so the *deadline timer* arms in M2; the M1 DSR floor is synchronous-checklist), the retention/
consent/sub-processor registries, and the audit hash-chain+Merkle+witness construction. As each subsystem
ships its stores (M1 shared stores; M3 Git/KN; M4 CI/Issues/Chat) **its holder lights up** and is drilled into
the data map. The **full DSR/erasure fan-out across all H1–H18 + the multi-cell floor follow-on (GA-D8) + the
two new mechanisms (history-rewrite invalidation GA-10, outbound-mirror gate GA-11) are M5**, where every
holder exists and the cross-cell bridge goes live. GDPR/Audit is the spine of **E2E-3** (audit tamper-evidence)
and **owns E2E-4** (the DSAR fan-out — the GDPR-by-construction flagship) in M5, and participates in the M6
dogfood (the team's real data is real tenant data — you do not dogfood onto a substrate whose restore-verify
and DSAR fan-out are not green).

The honest progression: **first runnable** = early M1 (the `PersonalDataHolder` trait + harness
auto-registration + the `no-untagged-personal-data` lint + the data-map generator — a store cannot open
without being a holder; an untagged PII field cannot compile); **first useful** = late M1 (the DSR orchestrator
fans out over the holders that exist + the per-subject-DEK crypto-shred + pseudonym-map shred floor work
end-to-end + the tamper-evident audit log appends via the outbox + the erasure ledger drives post-restore
re-erasure — a single subject seeded into the M1 stores can be erased and proven erased, surviving a restore);
**production-hardened** = M5 (GA-D1 erasure reaches all H1–H18 with 0 missed; GA-D3 audit tamper detected 100%
at cell scale; STOR-D3/D4 crypto-shred unrecoverable in backups; GA-D8 multi-cell fan-out 0 cells missed;
GA-10 history-rewrite invalidation 0 stale-PII cache hits; GA-11 outbound-mirror gate denies extra-EU by
default; the E2E-4 DSAR certificate seals).

---

## 1. The contracts GDPR/Audit owns / consumes, mapped to the milestone they land in

From contract-index §10 (owned by GDPR/Audit) + §1/§2/§4/§5/§6/§7/§9/§11/§12 (consumed). "Lands" = the
milestone by which the contract must be implemented or callable for the gate that depends on it to be green. A
floor is named inline and tracked in §6.

### 1.1 Owned by GDPR/Audit (contract-index §10) — what every store + tenant consumes

| # | Contract | Lands | Notes / floor |
|---|---|---|---|
| 10.1 | `PersonalDataHolder{locate, export, rectify, restrict, erase}(subject\|tenant) → Receipt` — every store; harness auto-registers; exhaustive holder list H1–H18; erasure = purge/crypto-shred/pseudonymise, never hide | **M1** (the trait + auto-registration + the M1-store holder impls); **holders complete across M1→M4** | the **trait + the auto-registration hook** are M1 (every store the harness opens registers, contract 1.4). The *implementations* land **per holder as the store ships**: H6/H7/H8/H9/H10/H11/H12/H13/H14/H15/H16/H18 (the shared-layer holders) across M1/M2; H1 (Git) + H4 (Knowledge) + H17 (agent-trace) in M3; H2 (CI) + H3 (Issues) + H5 (Chat) in M4. The list is **complete** (and GA-D1 is provable end-to-end) only at **M5**. The harness `holder-registered` architecture test forbids a store opening outside the harness — the holder list cannot drift below the data map. |
| 10.2 | `#[personal_data(category, role, basis, retention, erasure, subject_locator)]` classify-derive — the `no-untagged-personal-data` lint; **+ worklog `behavioural`/restricted-by-default tags (OQ-H)** | **M0** (the lint + red/green fixtures); **M1** (the derive + the tag schema); **tags applied per store as it ships** | the **lint** is one of the twelve committed M0 lints (with its red-fixture proving it rejects an untagged PII field + a green-fixture proving it admits a tagged one). The **derive macro + the five-tag enum** land M1. Each store applies tags as it ships (the lint forces it). The **worklog/productivity/estimate `Behavioural`+restricted-by-default classification** (OQ-H, `[OPEN — LEGAL]`) lands with Issues in **M4**; the `SpecialCategory` tag (the DPIA router) is M1, applied per-field as fields land. |
| 10.3 | `data_map() → Inventory`; `ropa(tenant) → ProcessingActivities` — generated from tags + holders, CI-diffed; drives DSR fan-out, breach scoping, RoPA, DPIA | **M1** (the generator + the CI diff); **content grows per store** | the **build step that walks every schema + every registered holder + generates the machine-readable inventory + diffs it in CI** lands M1. It is the *substrate for fan-out* — the map, not a hand-written list, drives erasure (GA-D1's "0 holders missed" is a property of the generated map). The RoPA legal text is `[OPEN — LEGAL]` (DPO ratifies the generated rows); the *generation* ships M1. |
| 10.4 | `dsr_submit(kind, subject, scope, posture) → dsr_id`; `dsr_status → {state, deadline, checklist}`; `dsr_certificate → MerkleProvenBundle` — the DSR state machine; 1-month durable timer; **iterates `member_cells` (OQ-I bridge)**; Art. 28 operable by/for tenants | **M1** (the state machine + checklist + the fan-out over M1 holders + the certificate); **deadline timer M2**; **multi-cell fan-out M5** | the **state machine** (received→validated→fanned-out→awaiting-holders→verified→completed) + the **durable, resumable per-holder checklist** + the **verifiable receipt** + the **certificate sealing into the Merkle tree** land M1 (over whatever holders exist). The **1-month deadline durable timer** rides the `myelin-flow` wheel (9.3), which is **M2** — so the M1 floor tracks the deadline synchronously/coarsely and the **durable nearing-deadline Signal arms in M2** (GA-D4). The **multi-cell `member_cells` iteration** is the **M5 FLOOR follow-on** (GA-D8) — single-cell fan-out is fully built M1. |
| 10.5 | `effective_retention` (tightest-wins, legal-hold-aware); `consent_record/withdraw`; `subprocessors`/`transfer_allowed` (deny extra-EU by default); **+ outbound push-mirror gate (NEW §5.3)** | **M1** (retention/consent/sub-processor engine + the transfer gate); **the Git push-mirror leg M3** | the **tightest-policy-wins retention merge + legal-hold-aware suspend + the consent registry (per-subject DEK) + the sub-processor registry + the `transfer_allowed` deny-extra-EU-by-default gate** land M1. The **outbound push-mirror residency gate** (GA-11) reads this doc's `transfer_allowed` truth but the *Git mirror seam it gates* is a Git M3/M4 surface — the **policy ships M1, the gate-on-the-mirror is proven M5** (GA-11). Tenancy owns enforcement at the control plane; GDPR owns the policy the gate reads. |
| 10.6 | Tamper-evident audit log — per-tenant hash-chain + Merkle (CT-style proofs, external witness); minimised; **written via outbox only**; **+ history-rewrite as an audited op (NEW §6.6)** | **M1** (the log construction + proofs + witness); **history-rewrite op M5** | the **per-tenant hash-chain + Merkle tree + CT-style inclusion/consistency proofs + signed-tree-head + the independent-witness anchoring** land M1 (the audit consumer is an infra subscription on the M0 outbox — coverage is a property of the bus, BUS-2). The actor is the **frozen pseudonym grammar `<pseudonym>@<tenant>.noreply`** (4.8, M1). The **`git.history_rewrite` audited op + its fork/mirror/clone-cache invalidation fan-out** (GA-10) is **M5** (it needs Git's cache namespaces + the rewrite tool, M3/M5). |
| 10.7 | eDiscovery / legal-hold export — `ediscovery_export(scope) → MerkleProvenBundle`; content-addressed, inclusion-proof-bearing, legal-hold-frozen | **M1** (the export over M1 records); **completeness grows per holder** | the **same tamper-evident substrate** serves "prove we erased it" (DSR receipt) and "prove this is the unaltered record" (eDiscovery). The export mechanism ships M1; its *scope completeness* grows as holders + subsystems land (a matter-scoped bundle is complete only when all referenced subsystems exist — M4+). |
| 10.8 | Erasure ledger (PII-free, non-shred-erasable) — opaque subject id + holders/keys shredded; drives **post-restore re-erasure** (GD-14) | **M1** | the **PII-free ledger of every completed erasure** (which subject, which holders, which key epochs destroyed) is the **silent-data-loss-meets-erasure floor**: it is what Storage's `post_restore_reerase` (11.5) reads to re-erase a subject who was erased *before* a restored backup's offset. It is itself a recursive holder (PII-free, so not shred-erasable). Ships M1 — drilled by ID-D8/STOR-D3 (M1 partial, M5 at cell scale). |
| 10.9 | **The ONE free-text/immutable-content erasure posture** — structural floor (per-subject DEK shred + pseudonym-map shred + `restrict` suppression) + documented residual limit; instantiated per subsystem by reference; `[OPEN — LEGAL]` | **M1** (the structural floor + the posture written once); **referenced per subsystem M3/M4**; **residual ratification parallel-legal** | the **keystone X-7 artifact**. The **structural floor** (per-subject DEK crypto-shred of self-authored free-text + pseudonym-map shred of identity + the `restrict` suppression that keeps the residual never-indexed/never-agent-read/never-in-analytics) is **fully built M1** and is the engineering answer for the overwhelming majority. The **posture is written ONCE** (here) and each subsystem **references it** (Git M3 — the pseudonymous-by-default commit floor; KN M3; CI/Issues/Chat M4) — never restated five times. The **residual lawful-basis ratification** (third-party / immutable-byte free-text PII authored by others) is `[OPEN — LEGAL]`, ratified in **one statement** by counsel/DPO **in parallel**; the floor ships regardless. |
| (1.8) | telemetry — `dsr_deadline_margin`, `erasure_fanout_coverage`, `audit_append_lag`, `sth_publish_age`, `legal_hold_active_count` | **M1** | every GDPR drill asserts against this signal set; **no signal = failed drill** (EI-01 §3, observability is part of the pass condition). `erasure_fanout_coverage` is the signal GA-D1 reads (= 100% = 0 holders missed); `audit_append_lag`/`sth_publish_age` are the audit-log health SLOs; `dsr_deadline_margin` is what GA-D4's nearing-deadline Signal fires on. |

### 1.2 Consumed by GDPR/Audit — the upstream dependency list (the systems it orchestrates)

GDPR/Audit owns no mechanism, so it consumes a great deal — but almost all of it is **downstream-of-GDPR in
build order** (GDPR registers them as holders; they implement `erase`). The genuine *upstream blockers* (must
exist before GDPR's own structural spine works) are few and bolded at the end.

| # | Consumed contract | From | Must be green by | Why GDPR depends on it |
|---|---|---|---|---|
| 1.1/1.2/1.3 | `serve(AppSpec)` + three-surface + liveness≠readiness | **substrate (M0)** | **M0** | the service shell the GDPR/Audit service (DSR orchestrator + data-map generator + audit consumer) boots from. |
| **1.4** | **`PersonalDataHolder` auto-registration** — every store the harness opens | **harness (M0)** | **M0** | **the load-bearing structural hook**: GDPR *owns* the trait, but the harness *enforces registration*. The hook is frozen in M0 so every store opening M1+ auto-registers — the holder list cannot drift below the data map. |
| 1.6 | the architecture lints — esp. **`no-untagged-personal-data`** (GDPR's own ratchet) + `control-plane-pii-free` | **substrate/CI (M0)** | **M0** | the `no-untagged-personal-data` lint is **GDPR's committed ratchet** (GA-D5); it ships in the M0 twelve-lint set with its red+green fixtures and stays green forever. `control-plane-pii-free` keeps the control-plane schema PII-free (so multi-cell DSR pointers carry no PII, CP-D1). |
| 2.1 | `EventEnvelope` (`contains_personal_data`/`data_role`/`visibility`/`pii_key_ref` fields; the X-5 names/units anchor) | **Bus (M0)** | **M0** | the audit log + the DSR router read `data_role` (controller vs processor) and `contains_personal_data`/`pii_key_ref` off the envelope; these fields are frozen in M0 as the names/units anchor. |
| 2.2/2.7 | `OutboxTx::emit` (the only emit path) + `*.erased` tombstones + inline-PII `pii_key_ref` envelope-encryption | **Bus (M0)** | **M0** | the **audit log is written via the outbox only** (no service writes it directly — coverage is a bus property, BUS-2); the bus is itself a holder (H8) that crypto-shreds inline-PII keys + emits `*.erased` tombstones on erase. |
| 9.3 | Durable timer wheel (`sleep_until` on the minute-bucket index) | **Workflow (M2)** | **M2** | the **1-month DSR deadline timer + the nearing-deadline warning Signal** ride the same wheel as SLA timers (we do not reinvent durable timers). The M1 DSR floor tracks the deadline coarsely; **GA-D4's durable-timer fire is an M2 gate.** |
| 9.2/9.4 | Durable workflow activity + signal (`SCHEDULE_AND_RUN_JOB`, resumable) | **Workflow (M2)** | **M2** (history-rewrite op M5) | the **history-rewrite op is a resumable `myelin-flow` activity** (idempotent, outbox-emitted, §6.6); the multi-cell fan-out wave is sequenced as durable activities. |
| 4.8 | `erase(subject)` + `resolve_pseudonym` + the frozen pseudonym grammar `<pseudonym>@<tenant>.noreply` | **Identity (M1)** | **M1** | **the erasure LEVER**: the pseudonym-map shred is **DSR fan-out step 1** (erasing the map first means every downstream holder sees only the opaque pseudonym); the grammar is the audit actor-minimisation form. Co-lands with Id in M1. |
| 11.3/11.4 | KMS hierarchy + `KeyOrigin{destroy}` + GD-4 granularity (per-subject DEK, **incl. per-subject CI-log DEK, P5**) | **Storage (M1)** | **M1** (CI-log DEK M4) | **the crypto-shred MECHANISM**: the per-subject DEK is what a subject's erasure destroys; receipts record the destroyed key epoch (making "we erased it" independently checkable against the KMS key-destruction log). The per-subject CI-log DEK granularity lands with CI in **M4**. |
| 11.5 | Backup/restore cross-seam + **post-restore re-erasure** (`post_restore_reerase`) | **Storage (M1)** | **M1** | **the silent-data-loss-meets-erasure floor**: erasure reaches backups *by construction* (key destroyed ⇒ ciphertext unrecoverable, H10); restore must not resurrect an erased subject — `post_restore_reerase` reads the erasure ledger (10.8). Drilled STOR-D3/STOR-D4/ID-D8. |
| 11.2 | Trust-tier/branch-scoped cache namespaces + within-EU CDN clone/bundle class | **Storage (M1 frame); Git impl M3** | **M5** | the **history-rewrite invalidation fan-out** (GA-10) purges stale content-addressed clone/bundle blobs across the trust-scoped cache namespaces; the **outbound-mirror gate** (GA-11) distinguishes within-EU acceleration (allowed) from extra-EU replication (denied). Both are M5. |
| 11.6 | OLAP read store **honours the restriction flag** | **Storage (M1)** | **M2** (proven when analytics exists) | `restrict(subject)` must suppress **analytics** too (no OLAP for a restricted subject); the flag-propagation is built M1, proven by GA-D7 when the OLAP read model + a consuming surface exist (M2+). |
| 6.4 / 5.8 / 2.6 | Search `purge+reindex` (incl. embeddings); Refs `tombstone`; reindex-from-source `replay` | **Search/Refs/Bus (M2)** | **M2** | per-derivative erasure: Search **purges+reindexes** (embeddings re-identify, so they are purged not hidden, GA-D2/SRCH-D4); Refs **tombstones** (rebuildable projections); rectification fans out via reindex-from-source, never patched-in-place. |
| 8.8 | Agent execution trace as a content-addressed erasable holder (AG-7) | **Agent/Knowledge (M2 seam, M3 holder)** | **M3** | H17 (agent trace) is a holder **distinct from the audit log** (trace = reasoning record, erasable; audit = the complete who-did-what, carve-out). Drilled KN-D12/AG-D10. |
| 12.3/12.6 | `member_cells` placement + the PII-free `CrossCellPointer` bridge | **Tenancy (M1 frame); live M5** | **M5** | **multi-cell DSR fan-out** iterates `member_cells ∪ home_cell` (all same-region); resolution is **always cell-local** (a cell never reads another cell's PII; each erases its own holders + returns a PII-free receipt). The FLOOR follow-on — GA-D8/CP-D7/CP-D8 owed at M5. |
| 12.x | outbound-mirror residency enforcement at the control plane | **Tenancy (M5)** | **M5** | the control plane *enforces* the §5.3 push-mirror gate; GDPR owns the `transfer_allowed` *policy* the gate reads. GA-11. |
| 4.11 | fail-static staleness bound `W` (DPO-ratified) | **Identity (M1)** | **M1** | GDPR owns the **constraint** (`W ≤ deprovision/revocation SLA`, `W` contains the agent-token TTL); the DPO ratifies the **value** (L-1, `W = 5 min`). Recorded as a dated residual in the RoPA. |

**The critical upstream dependencies, stated plainly:** GDPR/Audit's structural spine has exactly **four hard
blockers, all in M0/M1**: the **M0 harness auto-registration hook (1.4)** + the **M0 outbox (2.2, the audit log
rides it)** + the **M1 Identity pseudonym-map lever (4.8)** + the **M1 Storage KMS/crypto-shred/restore floor
(11.3/11.4/11.5)**. The **durable timer (9.3) is an M2 blocker** for the deadline-timer leg only. Everything
else GDPR consumes — every subsystem's `erase` — is a holder it *registers and orchestrates*, which lands
**after** GDPR's spine, not before it. That is the defining property of an orchestrator: its dependency on the
things it orchestrates is a **fan-out completeness** dependency (GA-D1 needs them all to exist) — **M5** —
not a build-order blocker for its own spine — **M1**.

---

## 2. The milestones (mapped to master-sequencing bands)

Each milestone names its **work**, its **entry dependency**, the **floors it ships (+ their follow-on)**, and
its **exit gate** (the quantified drills that must emit a green artifact to call it done). The bands are the
master-sequencing M0..M6; the gate invariant (R-2) holds — no GDPR milestone is done over a red earlier gate.

### GA-M0 — The committed ratchet floor (inside master M0)

**Maps to:** master M0 (the committed gates; the harness contract).

**Thesis:** before any store writes real data, the *structural impossibility of forgetting a store* must be a
committed, loud gate. GDPR contributes its one lint + its one harness hook to the M0 ratchet. No feature code.

**Work:**
- **The `no-untagged-personal-data` lint** (contract 1.6) — GDPR's committed ratchet — with a **red-fixture**
  (a personal-data-typed field with no `#[personal_data]` tag → build fails) and a **green-fixture** (a tagged
  field → admits). Wired into CI loud, never `|| true`. This is the cheapest, most load-bearing GDPR gate.
- **The `PersonalDataHolder` auto-registration contract** (1.4) frozen as part of the harness `serve(AppSpec)`
  shell: the harness, opening any store, auto-registers it as a holder; a store opened outside the harness
  fails the `holder-registered` architecture test. The *implementation* is M1, but the **contract + the
  enforcement hook** are M0 (so M1 stores register the moment they open).
- The `myelin-gdpr` glue-crate skeleton (ADR-01) as a compile-time contract carrier: the trait signatures, the
  `#[personal_data]` tag enum names, the envelope `data_role` field — frozen so consumers compile against
  GDPR's contracts before its bodies exist.
- Anchor the `data_role` (`tenant-content | platform-operational`) classification to the frozen envelope field
  (2.1) — the X-5 names-and-units reconciliation point.

**Entry dependency:** none beyond the M0 root (the workspace + the harness shell being built alongside).

**Exit gate (the GDPR contribution to the M0 → M1 gate):**
- **GA-D5** (CI) — add an untagged personal-data field → the `no-untagged-personal-data` lint fails the build;
  the data-map diff surfaces it. **Gate: build red on untagged PII** (both fixtures green). This rides in the
  master "all twelve lints green with both fixtures" M0 gate.

### GA-M1 — The structural spine + the DSR orchestrator + the tamper-evident audit log (the bulk of GDPR)

**Maps to:** master M1 ("the GDPR/Audit spine, structural half") — Tier 1 (silent-data-loss meets erasure: the
erasure ledger + restore re-erasure) + the keystone X-7 decision.

**Thesis:** stand up the entire *policy + orchestration + proof* surface over the M1 holders, and **freeze the
ONE free-text/immutable erasure posture (X-7) before the Git data model freezes in M3.** Build the structural
floor (per-subject DEK shred + pseudonym-map shred + `restrict`) so it works end-to-end on the M1 stores; build
the data map that drives fan-out; build the tamper-evident audit log; build the erasure ledger that makes
restore-resurrects-nothing true.

**Work:**
- **The `PersonalDataHolder` trait + the M1-store holder implementations** (10.1, H6 blob, H7 search-stub,
  H8 bus, H9 cache, H10 backup, H14 authz-tuples, H15 identity, H16 audit, H18 GDPR's own stores) — each
  store's `{locate, export, rectify, restrict, erase}`. The harness auto-registers them (the M0 hook).
- **The `#[personal_data]` classify-derive + the five-tag enum** (10.2) — `category / role / basis / retention /
  erasure / subject_locator` — applied across every M1 store's PII fields (the lint forces completeness). The
  `SpecialCategory` tag (the DPIA router) is wired.
- **The data-map / RoPA generator** (10.3): the build step that walks every schema + every registered holder,
  generates the machine-readable inventory, and **diffs it in CI** (a DPO sees any reclassification). This is
  the substrate for fan-out — *the map drives erasure*, so "we forgot the search index" is structurally
  impossible.
- **The DSR orchestrator state machine** (10.4): `dsr_submit/dsr_status/dsr_certificate`; the
  received→validated→fanned-out→awaiting-holders→verified→completed machine; the **durable, resumable per-holder
  checklist** (a crashed orchestrator re-drives only un-receipted holders); the canonical **erase order**
  (Id.erase pseudonym-map first → KMS.destroy per-subject DEK → Search.purge+reindex → Refs.tombstone →
  Bus.erase → notif/authz/agent-memory → record receipt); the **verifiable, content-addressed, signed
  receipts** recording the destroyed key epoch + the purge cursor (so erasure is independently checkable). The
  controller-vs-processor posture gate (refuse a Myelin-initiated erase of tenant content absent instruction).
  *Floor (named):* the **1-month deadline timer is coarse in M1** (synchronous tracking); the durable
  nearing-deadline Signal arms in **M2** on the timer wheel (GA-D4 is the M2 leg).
- **The structural erasure floor (X-7 §7.1)**: per-subject DEK crypto-shred of self-authored free-text (the
  lever Storage provides); pseudonym-map shred of identity (the lever Identity provides); `restrict(subject)`
  suppression (no indexing / agent-use / analytics / notification, retaining storage, reversible). Works
  end-to-end on the M1 stores.
- **The ONE free-text/immutable erasure posture written once** (10.9, X-7) — the structural floor + the
  documented residual limit — as the single platform artifact every subsystem will reference. The
  pseudonymous-by-default commit-identity *requirement* is recorded here as a **commit-time prerequisite Git
  must satisfy in M3** (decided before the git data model freezes, EI-04 §1).
- **The erasure ledger** (10.8): the PII-free record of every completed erasure (subject, holders, key epochs),
  driving Storage's `post_restore_reerase`. A recursive holder, PII-free, non-shred-erasable.
- **The tamper-evident audit log** (10.6): per-tenant hash-chain + Merkle tree + CT-style inclusion/consistency
  proofs + signed tree heads + the independent-witness anchoring (RFC-3161 TSA / a different cell's notary,
  opaque root only — residency-safe). **Written via the outbox only** (the audit consumer is an infra
  subscription — coverage is a bus property). Minimised (actor = the frozen pseudonym grammar, never payloads).
  Deliberately **not a blockchain**.
- **The retention engine + consent + sub-processor registries + the transfer gate** (10.5): tightest-policy-wins
  merge (legal-hold-aware suspend-don't-delete); the consent registry (per-subject DEK); the sub-processor
  registry; `transfer_allowed` deny-extra-EU-by-default.
- **eDiscovery export** (10.7) over the M1 records (content-addressed, inclusion-proof-bearing, legal-hold-frozen).
- The fail-static window constraint (`W ≤ revocation SLA`, GD-3) recorded as a dated RoPA residual.

**Entry dependency:** GA-M0 green (the lint + the auto-registration hook); **Identity M1** (the pseudonym-map
lever 4.8); **Storage M1** (KMS/per-subject DEK 11.3/11.4 + backup/restore + `post_restore_reerase` 11.5);
the **M0 outbox** (the audit log rides it). The data-map generator can run over whatever holders exist.

**Exit gate (the GDPR contribution to the M1 → M2 gate; rides the master STOR-D4/GA-D5 line):**
- **GA-D5** (CI) re-confirmed green (the lint stays a permanent ratchet).
- **STOR-D4 / GA-D5 line** (master M1 exit): per-subject crypto-shred **unrecoverable in backups**;
  `no-untagged-personal-data` red on an untagged PII field — SCHED/CI.
- **GA-D3** (SCHED, partial-at-M1-scale) — retroactively edit/delete an audit entry → the hash-chain breaks +
  the consistency proof against the published STH fails + the external witness mismatches. **Gate: tamper
  detected 100%** over the M1 audit surface.
- **The M1-holder erasure floor proven** (the M1 face of GA-D1): a subject seeded into the M1 stores
  (identity, authz-tuples, blob, bus, audit, GDPR's own) → `dsr_submit` → the data-map-driven fan-out hits
  every *existing* holder; post-erase `locate` returns 0 recoverable PII; **the erasure ledger entry +
  post-restore re-erasure** (ID-D8/STOR-D3 at M1 scale) → an older restore lands the subject **still erased**,
  0 resurrected. The *full* H1–H18 coverage (GA-D1) is the **M5** gate.
- **`erasure_fanout_coverage` + `audit_append_lag` + `sth_publish_age` telemetry signals read green** (no
  signal = failed drill).

### GA-M2 — The deadline timer + restriction-into-analytics + the per-derivative erasure fan-out

**Maps to:** master M2 (the reactive shared layer — Search/Refs/Notif/Workflow/Agents come online).

**Thesis:** as the reactive layer lands, GDPR's orchestration completes its dynamic legs: the durable deadline
timer (on the M2 timer wheel), restriction-into-analytics (when OLAP exists), and the per-derivative erasure
fan-out (Search purge+reindex of embeddings, Refs tombstone, reindex-from-source rectification).

**Work:**
- **The DSR 1-month deadline durable timer + the nearing-deadline warning Signal** (10.4) on the `myelin-flow`
  timer wheel (9.3) — replacing the M1 coarse-tracking floor.
- **`restrict` suppression into Search/Refs/Notif/Agents/OLAP** (11.6, GA-9): no indexing, no agent-use, no
  analytics (incl. OLAP), no notification for a restricted subject. The flag built M1 is now **honoured by the
  derived stores that exist** (Search, Refs, Notif, OLAP read model).
- **Per-derivative erasure**: Search `purge+reindex` (incl. embeddings, purged-not-hidden — they re-identify);
  Refs `tombstone` (rebuildable projections); rectification fan-out via reindex-from-source (never patched-in-
  place-and-drift).
- **The agent-trace holder seam** (8.8, H17) registered as a distinct erasable holder (the impl lands with
  Knowledge in M3; the seam + the distinct-from-audit boundary are wired in M2).
- History-rewrite as a **resumable `myelin-flow` activity skeleton** (the op body; the invalidation fan-out
  surface is M5 when Git's cache namespaces exist).

**Entry dependency:** GA-M1 green; **Workflow M2** (the timer wheel + durable activity); **Search/Refs/Notif
M2** (the derived stores that honour `restrict` + implement purge/tombstone); the OLAP read model (M2).

**Exit gate (the GDPR contribution to the M2 → M3 gate):**
- **GA-D7** (CI) — restrict a subject → no indexing/agent-use/analytics/notification while storage retained;
  reversible. **Gate: 0 processing of a restricted subject.**
- **GA-D4** (SCHED) — open a DSR → the durable timer fires a warning Signal before the 1-month deadline; the
  certificate seals on completion. **Gate: 0 silent misses.**
- **GA-D2** (SCHED, with Search) — the subject's docs **and embeddings** purged+reindexed out (not hidden);
  0 hits, 0 embedding re-identification (SRCH-D4 is its Search face).
- The M2-derivative erasure proven: REF-D5 (refs tombstone, 0 recoverable, no resolve-500), NOTIF-D6 (inbox
  humanises to `[erased user]`) — the cross-owner instances riding the GDPR fan-out.

### GA-M3 — The producer-subsystem holders + the Git pseudonymous-commit instance of X-7

**Maps to:** master M3 (Git + Knowledge — the producer subsystems).

**Thesis:** the two producer subsystems light up their holders and **instantiate the X-7 posture by reference**;
Git's pseudonymous-by-default commit identity (the commit-time prerequisite GDPR froze in M1) is consumed and
proven.

**Work (GDPR's orchestration over the new holders — the subsystems own their `erase` impls):**
- **H1 (Git) + H4 (Knowledge) + H17 (agent-trace) holders register** into the data map; the DSR fan-out now
  reaches them. The data-map diff in CI surfaces the new PII fields (the lint forces tagging).
- **The Git instance of the ONE posture** (10.9 §7.4): Git's erasure section **references** the platform
  posture — self-authored content crypto-shreds via per-subject DEK; identity via pseudonym-map shred;
  **commits are pseudonymous-by-default** (the immutable hash never bakes erasable PII); the third-party /
  immutable residual is the documented limit + `restrict`. GDPR confirms the reference is correct (no
  restatement) and that GIT-D2's residual == the ONE platform-posture residual.
- **The Knowledge instance** (10.9 §7.4): free-text blocks + db-row values crypto-shred via per-subject DEK;
  the agent-trace holder (H17) is distinct from the audit log (KN-D12).

**Entry dependency:** GA-M1/GA-M2 green; **Git M3** (the H1 holder + pseudonymous commits + the LFS blobs);
**Knowledge M3** (the H4 holder + the agent-trace H17 holder); the per-subject DEK crypto-shred mechanism.

**Exit gate (the GDPR contribution to the M3 → M4 gate — these are the subsystems' drills, GDPR-anchored):**
- **GIT-D2** (SCHED) — erase a subject who authored commits/PRs/comments + LFS → every holder hit; **residual
  == the ONE platform-posture residual (10.9)**; crypto-shred reaches backups. (The X-7 Git instance proven.)
- **KN-D4 / KN-D12** (SCHED) — erase → structured PII purged/pseudonymised, free-text per-subject-DEK
  crypto-shredded (unrecoverable in op-log/snapshots/backups), embeddings purged, agent traces crypto-shredded,
  attribution → pseudonym. **0 recoverable incl. vectors; residual per 10.9.**

### GA-M4 — The consumer-subsystem holders + the per-subject CI-log DEK + the worklog classification

**Maps to:** master M4 (CI + Issues + Chat — the consumer subsystems).

**Thesis:** the three consumer subsystems light up their holders; the per-subject DEK reaches CI log segments
(the P5 granularity extension); the worklog/productivity classification (OQ-H) lands with Issues.

**Work:**
- **H2 (CI + log segments) + H3 (Issues, incl. worklog) + H5 (Chat) holders register** into the data map.
- **Per-subject DEK crypto-shred reaches isolable CI log-segment PII** (11.4, the P5 extension — was per-tenant
  floor in Phase 3). CI's `erase` (CI-D3) destroys PII in logs/artifacts/caches/run-state per-subject where
  isolable, per-tenant fallback, incl. backups.
- **The worklog/productivity/estimate classification** (OQ-H, `[OPEN — LEGAL]`): tagged
  `category=Behavioural, role=TenantContent, restricted-by-default`; per-individual rollups OFF by default,
  gated behind explicit tenant-admin enablement that **surfaces the works-council consultation trigger** (the
  platform surfaces it; it does not adjudicate). The `SpecialCategory` route into the DPIA gate is wired.
- The Issues/CI/Chat instances of the ONE posture (10.9 §7.4) — each references it, no restatement.

**Entry dependency:** GA-M3 green; **CI M4** (the H2 holder + per-subject CI-log DEK); **Issues M4** (the H3
holder + worklog fields); **Chat M4** (the H5 holder).

**Exit gate (the GDPR contribution to the M4 → M5 gate — subsystem drills, GDPR-anchored):**
- **CI-D3** (SCHED) — erase fans to CI → PII in logs/artifacts/caches/run-state destroyed (per-subject DEK
  where isolable, per-tenant fallback) incl. backups; structure survives. **0 dangling leak.**
- **ISS-D11** (SCHED) — erase → PII gone from issue row (per-subject DEK), change-log, comments, attachments,
  OLAP (+restriction), Search (incl. embeddings), Refs; post-restore re-erasure catches a restore; third-party
  residual is the `[OPEN — LEGAL]` limit.
- **CHAT-D8** (SCHED) — erase → bodies crypto-shred in hot+cold segments+backups; mentions → `[erased user]`;
  read-state/drafts/unfurl-cache purged; Search/Refs/Notif cascade. **0 recoverable PII.**
- **All H1–H18 holders now exist** — the precondition for GA-D1 (M5) to be a complete fan-out.

### GA-M5 — The full DSR fan-out + multi-cell + the two new mechanisms + the E2E-4 flagship

**Maps to:** master M5 (world-scale hardening + the floor follow-ons + the whole-system E2E wedge).

**Thesis:** every holder now exists, so the **full erasure fan-out is complete** (GA-D1: 0 holders missed); the
**multi-cell floor follow-on** goes live (the cross-cell PII-free bridge, GA-D8); the two NEW Phase-5
mechanisms (history-rewrite invalidation GA-10, outbound-mirror gate GA-11) are proven; GDPR **owns the E2E-4
DSAR fan-out** — the GDPR-by-construction flagship — and is the audit spine of E2E-3.

**Work — the floor follow-ons (each named in its band):**
- **The full DSR / erasure fan-out across all H1–H18** (10.4, GA-D1): every holder exists, so the fan-out is
  complete; the `[OPEN — LEGAL]` posture (10.9) is instantiated per subsystem by reference (all references now
  resolved).
- **Multi-cell DSR fan-out** (the named floor follow-on, OQ-I, 12.6): the orchestrator **iterates
  `member_cells ∪ home_cell`** over the now-live cross-cell PII-free pointer bridge; resolution is cell-local
  (each cell erases its own holders, returns a PII-free receipt, merged into one certificate). The **cross-cell
  ordering/atomicity remains the named control-plane floor** (the control plane sequences the wave; GDPR runs
  in each cell). FLOOR drills GA-D8/CP-D7/CP-D8 now owed and run.
- **History-rewrite as a first-class audited op** (10.6 §6.6, GA-10): the `git.history_rewrite` op — audited,
  rate-limited, with the **fork/mirror/clone-cache invalidation fan-out** tied to Storage's trust-tier/branch-
  scoped cache namespaces (11.2) — proven (the within-EU CDN clone class is purged of stale content-addressed
  blobs).
- **The outbound push-mirror residency gate** (10.5 §5.3, GA-11): a PII-bearing push-mirror to an extra-EU
  host **denied by default** at `transfer_allowed`; a within-EU CDN clone **allowed**.

**Work — world-scale hardening + the E2E wedge:**
- **GA-D1 at cell scale** (the headline GDPR drill): a subject seeded into all H1–H18 → 0 holders missed; 0
  recoverable PII (incl. vectors, incl. backups); residual == the ONE documented posture; certificate sealed.
- **GA-D3 at cell scale** (audit tamper-evidence under world-scale audit volume — the E2E-3 audit leg).
- **Restore-resurrects-nothing at cell scale** (STOR-D3 under world-scale load): post-restore re-erasure from
  the ledger holds.
- **E2E-4 DSAR fan-out** (GDPR owns it) — the whole-system GDPR-by-construction proof (see §4).
- **E2E-3 spec-to-ship** audit-tamper leg (GDPR contributes the tamper-detection proof).

**Entry dependency:** GA-M4 green (all H1–H18 holders exist); **multi-cell tenancy M5** (the cross-cell bridge
live, the control-plane wave-sequencing); **Storage M5** (object-backed packs + the trust-scoped cache +
the CDN class for the invalidation fan-out); **Git M5** (the history-rewrite tool).

**Exit gate (the GDPR contribution to the M5 → M6 gate):**
- **GA-D1** (SCHED) — erase a subject seeded into all H1–H18 → data-map fan-out hit every holder; post-erase
  `locate` returns 0 recoverable PII. **`erasure_fanout_coverage = 100%`; 0 holders missed.**
- **GA-D8** (SCHED, FLOOR) — multi-cell erasure: fan-out iterates all `member_cells ∪ home_cell`; merged a
  complete per-cell receipt set. **0 cells missed.**
- **GA-10** (SCHED, NEW) — history-rewrite-invalidation: run a `git.history_rewrite`; the invalidation fan-out
  reached forks/mirrors/clone-cache, the trust-scoped namespaces purged the stale blobs, the op is audited.
  **0 stale-PII cache/clone hits; op audited.**
- **GA-11** (SCHED, NEW) — outbound-residency-gate: a PII-bearing extra-EU push-mirror is **denied by
  default**; a within-EU CDN clone is **allowed**. **0 default extra-EU PII transfers.**
- **GA-D3 / GA-D6** (SCHED) — audit tamper detected 100% at cell scale; legal-hold defers erasure (0 held-scope
  deletions, resumes on lift).
- **E2E-4 DSAR fan-out green** (the flagship): H1–H18 coverage receipt set + post-erase `locate` = 0 + the
  Merkle certificate sealed.

### GA-M6 — Dogfooding (inside master M6)

**Maps to:** master M6 (Myelin hosts itself).

**Thesis:** the team's own data is **real tenant data** — you do not dogfood onto a substrate whose restore-
verify and DSAR fan-out are not green (master M6 entry dependency). GDPR's M5 green is a *precondition* for
M6.

**Work:** the GDPR/Audit machinery runs on the platform's own commits — the audit log records the team's own
human + agent actions; the every-incident-adds-a-drill loop files a Myelin issue + a reproducing drill; a
self-served DSR over the team's own data exercises the fan-out for real. The RoPA + the data map live as a
Myelin Knowledge space.

**Entry dependency:** GA-M5 green (DSAR fan-out + restore-verify proven before real team data lands).

**Exit gate:** the self-hosting audit graph is green on the platform's own commits; the truth-up pass confirms
no GDPR gate is red (every PROVEN row rests on a dated green artifact — code-wins-over-docs).

---

## 3. The drills GDPR/Audit owes, by band (quantified gates)

The master catalogue §4.2 (GA-D1..GA-D8) + the NEW GA-10/GA-11 (arch §9.2) + the cross-owner erasure/
restriction instances that ride GDPR's fan-out. Every threshold is a **default-to-beat** (Q32); Phase 6
measures + sets the final number. **No signal = failed drill** (the green artifact is the named telemetry
assertion reading green).

| Drill | Band | Freq | Quantified gate | Green artifact |
|---|---|---|---|---|
| **GA-D5** | GA-M0/M1 | CI | Add an untagged personal-data field → `no-untagged-personal-data` lint fails the build; data-map diff surfaces it. | lint red on untagged PII (both fixtures) |
| **GA-D3** | GA-M1 (M5 at scale) | SCHED | Retroactively edit/delete an audit entry → chain breaks + consistency proof vs published STH fails + witness mismatches. Tamper detected 100%. | tamper-detection proof |
| **STOR-D4** (GA face) | GA-M1 | SCHED | Erase a subject; attempt recovery from backups → per-subject ciphertext unrecoverable (key destroyed, excluded from backup). 0 recoverable PII in any backup. | crypto-shred-lag; 0 recoverable |
| **STOR-D3 / ID-D8** (GA face) | GA-M1 (M5 at scale) | SCHED | Erase; restore an older backup → subject still erased (post-restore re-erasure ran from the ledger). 0 resurrected. | re-erasure receipt |
| **GA-D7** | GA-M2 | CI | Restrict a subject → no indexing/agent-use/analytics/notification while storage retained; reversible. 0 processing of a restricted subject. | restriction-suppression proof |
| **GA-D4** | GA-M2 | SCHED | Open a DSR → durable timer fires a warning Signal before the 1-month deadline; certificate seals on completion. 0 silent misses. | DSR-timer fire; sealed cert |
| **GA-D2** | GA-M2 | SCHED | The subject's docs **and embeddings** purged+reindexed out (not hidden). 0 hits, 0 embedding re-identification. | embedding-purge receipt |
| **GA-D6** | GA-M2 (M5 confirm) | SCHED | Set a hold; submit an erase → erasure deferred-by-hold (not run), resumes on hold-lift. 0 held-scope deletions. | hold-defer receipt |
| **GIT-D2** (GA-anchored) | GA-M3 | SCHED | Erase a commit/PR/comment author + LFS → every holder hit; residual == the ONE platform-posture residual (10.9); crypto-shred reaches backups. | DSR receipt set; ledger entry |
| **KN-D4 / KN-D12** (GA-anchored) | GA-M3 | SCHED | Erase → free-text per-subject-DEK shredded (unrecoverable in op-log/snapshots/backups), embeddings purged, agent traces shredded, attribution → pseudonym. 0 recoverable incl. vectors. | holder receipts; key-shred count |
| **CI-D3** (GA-anchored) | GA-M4 | SCHED | Erase fans to CI → PII in logs/artifacts/caches/run-state destroyed (per-subject DEK where isolable) incl. backups. 0 dangling leak. | DSR receipt; 0 recoverable |
| **ISS-D11 / CHAT-D8** (GA-anchored) | GA-M4 | SCHED | Erase → all subsystem holders destroyed (per-subject DEK), OLAP+restriction, Search incl. embeddings, Refs; residual == 10.9 limit. 0 recoverable PII. | holder receipts; re-erasure |
| **GA-D1** | GA-M5 | SCHED | Erase a subject seeded into all H1–H18 → data-map fan-out hit every holder; post-erase `locate` = 0 recoverable PII. **0 holders missed.** | erasure-fanout-coverage = 100% |
| **GA-D8** | GA-M5 (FLOOR) | SCHED | Multi-cell erasure: fan-out iterates all `member_cells ∪ home_cell`; merged a complete receipt set. 0 cells missed. | per-cell receipt set |
| **GA-10** (NEW) | GA-M5 | SCHED | History-rewrite-invalidation: a `git.history_rewrite` → invalidation fan-out reached forks/mirrors/clone-cache; trust-scoped namespaces purged stale blobs; op audited. **0 stale-PII cache/clone hits; op audited.** | invalidation-completeness; audit entry |
| **GA-11** (NEW) | GA-M5 | SCHED | Outbound-residency-gate: PII-bearing extra-EU push-mirror **denied by default**; within-EU CDN clone **allowed**. **0 default extra-EU PII transfers.** | transfer-gate decision; 0 egress |
| **E2E-4 DSAR fan-out** | GA-M5 | SCHED | 0 holders missed; 0 recoverable PII (incl. vectors, incl. backups); residual == the one documented posture; certificate sealed. | H1–H18 coverage receipt set + `locate`=0 + Merkle cert |

**The permanent gates GDPR participates in (re-run forever):** the lint `GA-D5` (every change), and the
restore-verify/re-erasure line (`STOR-D3/STOR-D4`, every change touching a store — silent data loss meets
erasure outranks every feature, master §4 the two permanent gates).

---

## 4. The world-scale / hard-problem work, sequenced explicitly (name what ships as a floor)

The erasure-vs-immutability hard problem (EI-04 §1) is GDPR's defining hard problem. It is split into a **part
with a workable answer (the event-log half)** and a **genuinely hard residual (the git-history / third-party
free-text half)** — and the discipline is to **name the floor and the follow-on** (VISION §3).

| Floor (shipped) | Band | The full answer (follow-on) | Band | The trigger |
|---|---|---|---|---|
| **Per-subject DEK crypto-shred + pseudonym-map shred + `restrict` suppression** (the structural floor — erases the overwhelming majority reliably) | **M1** | (this *is* the answer for the event-log half; it is not a floor-to-be-replaced — it is the GDPR-compliant default) | — | — |
| **Pseudonymous-by-default commit identity** (immutable git bytes never bake erasable PII) — the X-7 decision frozen in M1, consumed by Git | **M1 decision / M3 Git instance** | **Audited history-rewrite erasure path** (GA-10, with the changed-hash consequence + fork/mirror/clone-cache invalidation) | **M5** | a body must be expunged from immutable git bytes (EI-04 §1) — decided *before* the git data model froze |
| **The `[OPEN — LEGAL]` residual posture** (the third-party / immutable-byte free-text PII residual — structural floor built, `restrict`-suppressed, residual flagged) | **M1 (written once) → M3/M4 (referenced)** | **Counsel/DPO ratification** of the ONE residual lawful-basis statement (10.9, not five statements) | **parallel (legal)** | the structural floor ships regardless; the residual is one ratified statement |
| **Single-cell DSR fan-out** (one home cell per tenant — fully designed) | **M1** | **Multi-cell DSR fan-out** (iterates `member_cells` over the cross-cell PII-free bridge; cell-local resolution) | **M5** | cross-cell rollup/collab/cross-org demand (OQ-I); FLOOR drills GA-D8/CP-D7/CP-D8 owed. Cross-cell **ordering/atomicity** remains the named control-plane floor *even at M5* |
| **Coarse DSR deadline tracking** (synchronous, M1 — no durable timer yet) | **M1** | **Durable deadline timer + nearing-deadline Signal** on the `myelin-flow` wheel | **M2** | the timer wheel (9.3) lands in M2 |
| **Per-tenant DEK for CI log PII** (Phase-3 floor) | **M3/M4** | **Per-subject DEK reaching isolable CI log segments** (the P5 granularity extension) | **M4** | CI ships its log tier (11.4/11.8) |

**The honest-floor rule binds all of these** (EI-04 §4): each floor is tracked in the gap report with its
claimed/proven status + its linked follow-on; the gap being *invisible* is the only failure. The two
`[OPEN — LEGAL]` items (the residual posture, the worklog classification) ship the **structural floor on the
engineering clock** and flag the **lawful-basis residual to counsel** — never blocking the build, never
pretending the residual is solved.

---

## 5. The first-runnable / first-useful / production-hardened progression (honest)

- **First runnable (early M1):** the `PersonalDataHolder` trait + the harness auto-registration + the
  `no-untagged-personal-data` lint + the `#[personal_data]` derive + the data-map generator. A store cannot
  open without being a holder; an untagged PII field cannot compile; the data map enumerates where PII lives.
  *Nothing can be erased yet end-to-end — but it is structurally impossible to forget a store.*
- **First useful (late M1):** the DSR orchestrator fans out over the M1 holders + the per-subject-DEK
  crypto-shred + pseudonym-map shred floor + `restrict` + the tamper-evident audit log (over the outbox) + the
  erasure ledger driving post-restore re-erasure. A single subject seeded into the M1 stores can be
  `dsr_submit`-erased, **proven** erased (a Merkle-sealed receipt recording the destroyed key epoch), and
  **stays** erased across a backup restore. *The GDPR-by-construction property is real — for the holders that
  exist.*
- **Production-hardened (M5):** GA-D1 (all H1–H18, 0 holders missed) + GA-D3 (audit tamper detected 100% at
  cell scale) + STOR-D3/D4 (crypto-shred unrecoverable in backups, restore resurrects nothing, under
  world-scale load) + GA-D8 (multi-cell fan-out 0 cells missed) + GA-10 (history-rewrite invalidation 0
  stale-PII hits) + GA-11 (outbound-mirror gate denies extra-EU by default) + **E2E-4 (the DSAR fan-out
  flagship green** — a single `dsr_submit` reaches every holder across all five subsystems, erases reliably,
  survives a restore, and seals a Merkle-proven certificate). *GDPR-by-construction, proven whole-system.*

The compounding-payoff signal (EI-01 closing): because GDPR owns no mechanism and the data map *generates* the
fan-out, **each new holder is a smaller addition than the last** — a new subsystem registers its store, tags
its PII fields (the lint forces it), implements `erase` (per-subject DEK shred + pseudonym shred, the same
levers), and the orchestrator already reaches it. If adding a holder ever got *harder*, the substrate would be
wrong (the `PersonalDataHolder` contract or the data-map generator) — stop and repair it, not the holder.

---

## 6. Floors register (tracked, dated, with follow-ons)

| Floor | Status (2026-06-19) | Follow-on | Band |
|---|---|---|---|
| Coarse DSR deadline tracking (no durable timer) | designed; ships M1 | durable timer + nearing-deadline Signal (GA-D4) | M2 |
| Single-cell DSR fan-out | designed; ships M1 | multi-cell `member_cells` iteration (GA-D8) | M5 |
| Cross-cell ordering/atomicity | named control-plane floor | globally-atomic multi-cell erase (vs resumable-per-cell checklist) | M5+ (control plane) |
| Per-tenant DEK for CI log PII | designed | per-subject DEK reaching isolable CI log segments | M4 |
| The `[OPEN — LEGAL]` residual posture (third-party / immutable free-text PII) | structural floor ships; residual flagged | counsel/DPO ratifies ONE lawful-basis statement (L-2/GD-1) | parallel (legal) |
| Audit-log retention carve-out scope per jurisdiction (H16) | structural carve-out ships | counsel decides duration/fields/basis (GD-5) | parallel (legal) |
| Worklog/productivity special-category classification | `Behavioural`+restricted tags ship; rollups OFF by default | counsel ratifies special-category + works-council trigger (OQ-H) | parallel (legal) |
| Build-data-as-LLM-training | foreclosed by default; no code path feeds tenant content to training | separately-ratified opt-in only (AG-8) | parallel (legal) |
| fail-static `W` value | structural bound ships M1; `W ≤ revocation SLA` enforced | DPO ratifies `W = 5 min` (L-1) | parallel (legal) |
| GA-D1 full H1–H18 fan-out | partial (M1 holders) → grows per band | complete fan-out when all holders exist | M5 |

**The honest-floor rule (EI-04 §4):** the structural floor ships on the engineering clock for every row above;
the `[OPEN — LEGAL]` rows ship the **floor** and flag the **residual** — counsel/DPO ratification gates
*publishing a posture as ratified*, never *building it*. Date every status note (a claim that outlives its
verification misleads the next agent).

---

## 7. Digest

**Where GDPR/Audit lands:** structural spine in **M1** (the `PersonalDataHolder` trait + harness
auto-registration + the `no-untagged-personal-data` lint + the data-map generator + the DSR orchestrator + the
tamper-evident audit log + the erasure ledger + the structural erasure floor); the dynamic legs (deadline
timer, restriction-into-analytics, per-derivative erasure) in **M2**; the holders light up per subsystem
(Git/KN **M3**, CI/Issues/Chat **M4**); the **full fan-out + multi-cell + the two new mechanisms + E2E-4** in
**M5**. GDPR is on the critical path through **one decision** — the X-7 structural-floor posture frozen in M1,
before the Git data model freezes in M3 (pseudonymous-by-default commits).

**Milestones → bands:** GA-M0 (the lint + auto-registration hook, master M0) · GA-M1 (the structural spine +
DSR orchestrator + audit log, master M1) · GA-M2 (deadline timer + restriction + per-derivative erasure,
master M2) · GA-M3 (Git/KN holders + the Git pseudonymous-commit X-7 instance, master M3) · GA-M4 (CI/Issues/
Chat holders + per-subject CI-log DEK + worklog classification, master M4) · GA-M5 (full H1–H18 fan-out +
multi-cell GA-D8 + history-rewrite GA-10 + outbound-mirror GA-11 + E2E-4, master M5) · GA-M6 (dogfood).

**Floors + follow-ons:** per-subject DEK shred + pseudonym shred + `restrict` = the structural floor (M1, the
GDPR-compliant default, not floor-to-be-replaced) · pseudonymous-by-default commits (M1 decision) → audited
history-rewrite (M5) · single-cell DSR (M1) → multi-cell `member_cells` fan-out (M5, GA-D8) · coarse deadline
tracking (M1) → durable timer (M2, GA-D4) · per-tenant CI-log DEK → per-subject CI-log DEK (M4) · the
`[OPEN — LEGAL]` residual posture (structural floor ships M1; ONE lawful-basis statement ratified parallel).

**Critical upstream dependencies (the four hard blockers, all M0/M1):** the **M0 harness auto-registration
hook (1.4)** + the **M0 outbox (2.2, the audit log rides it)** + the **M1 Identity pseudonym-map lever (4.8)** +
the **M1 Storage KMS/crypto-shred/restore floor (11.3/11.4/11.5)**. The **M2 durable timer (9.3)** is the only
later blocker (deadline-timer leg). Every subsystem `erase` GDPR consumes is a **holder it orchestrates** —
landing *after* GDPR's spine — so GA-D1 (0 holders missed) is a **fan-out-completeness** gate at **M5**, not a
build-order blocker for the M1 spine.

**The headline gates:** GA-D5 (CI, the permanent lint ratchet) · GA-D3 (audit tamper 100%) · STOR-D3/D4
(crypto-shred unrecoverable in backups, restore resurrects nothing) · GA-D7 (restriction suppression) · GA-D1
(M5, 0 holders missed) · GA-D8/GA-10/GA-11 (M5 floor follow-ons) · **E2E-4 DSAR fan-out** (the
GDPR-by-construction flagship).
