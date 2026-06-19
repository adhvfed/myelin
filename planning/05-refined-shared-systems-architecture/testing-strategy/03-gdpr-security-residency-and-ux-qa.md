# Phase 5 — Testing Strategy, Part 3: GDPR/Erasure, Security, Residency & UX/Design QA

> Phase: `05-refined-shared-systems-architecture/testing-strategy`. Canonical brief: [`VISION.md`](../../../VISION.md)
> (single source of truth, never contradicted). **The philosophy source** this doc operationalises:
> [`external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (prove-it-or-it-isn't-real; quantified thresholds; observability-as-pass-condition; the failure-injection
> harness; the ratchet / committed gates; name-your-floors; code-wins-over-docs; drive-the-real-UI +
> chained-mutation E2E; order-by-non-negotiability) and
> [`external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) §1 (erasure-vs-immutability),
> §5.1 (untrusted code execution).
> Binding directives: [`02b-doctrine-integration/integration-directives.md`](../../02b-doctrine-integration/integration-directives.md)
> **Phase-5 block T-1..T-9** + the named lints (E-5), ID-1..3, GD-1..3, CI-1, GIT-1, T-7/T-8.
> Spine: [`architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md) — **ADR-11**
> (region-pinning), **ADR-12** (PersonalDataHolder spine), **ADR-16** (backpressure/human-lane), **ADR-17**
> (fail-static), **ADR-18** (backup/restore-verify), **ADR-20** (one sandbox). Frozen contracts under test:
> [`contract-index.md`](../contract-index.md) + [`00-reconciliation-decisions.md`](../00-reconciliation-decisions.md)
> (esp. X-1 fork-trust, X-7 / contract 10.9 erasure posture, OQ-E `list_objects` push-down, OQ-I cross-cell bridge).
> Design-QA source: [`design-language.md`](../../02-holistic-architecture/design-language.md) §4 (a11y baseline)
> + §8b (the day-one testable mandates). Date: 2026-06-19.
>
> **What this is.** The **compliance + security + UX gate set** of the platform testing strategy: the gates
> that prove Myelin is GDPR-by-construction, breach-resistant, EU-sovereign, and actually usable. The
> companion parts of the strategy cover the resilience/correctness drill families and the harness/scorecard
> machinery:
> - **00** — the testing thesis, the failure-injection harness, the source-verified scorecard, the ratchet/CI wiring.
> - **01** — resilience & data-integrity drills (families F3/F4/F5/F6/F7/F9: outbox/reconnect, reindex-from-cold,
>   restore + cross-seam, agent-surge/human-lane, fail-static, loop/runaway).
> - **02** — correctness, determinism & the chained-mutation E2E discipline.
> - **03 (this doc)** — GDPR/erasure, security, residency, UX/design QA.
>
> This doc owns the gate **specification** for its scope; it reuses the **drill-family taxonomy (F1–F9)** and the
> **per-system owed-drill IDs** consolidated in
> [`03-shared-systems-architecture/drills-and-open-questions.md`](../../03-shared-systems-architecture/drills-and-open-questions.md)
> (Part A) and the per-subsystem `07-drills-and-open-questions.md` inventories, so Phase 6/8 wires **one**
> harness, not five.

---

## 0. How to read this document — the gate contract

**Every gate below resolves the same five fields** (doctrine §3, directive T-2/T-4):

1. **Property** — the thing being proven, stated as a falsifiable claim ("a viewer never sees X").
2. **Drill** — the *forced failure* that proves it. A property with no drill is **claimed, not proven**
   (T-4). For erasure/security/residency this means *seeding the adversarial condition*, not asserting the
   happy path.
3. **Quantified threshold** — a number or a zero. "Zero cross-tenant reads." "Every holder hit, 0 PII
   recoverable." "Contrast ≥ 4.5:1." "Keyboard response < 100 ms p99." A target you cannot measure is not a
   gate (doctrine §3).
4. **Green artifact** — the machine-emitted, committable proof the drill passed. A drill that survives but
   emits no signal has **failed** (observability-as-pass-condition, doctrine §3). Examples below: a DSR
   completeness report, an escape-drill attestation, a `residency_verify` signed attestation, a contrast
   report over the token table, a switch-test session transcript.
5. **Owner + CI cadence** — who owns the gate and whether it runs **per-change in CI** (cheap, structural)
   or **scheduled** (expensive, real-kernel / corpus / cross-seam). The ratchet (doctrine §5): a gate that is
   not committed and wired is **no gate**.

**Honesty rules, binding (VISION §3, doctrine §3/§5; T-2):**
- **Never weaken a threshold or invert an assertion to make a check pass.** A red gate is information.
  Record a `needs-human-verification` / `[OPEN — LEGAL]` item rather than softening green it didn't earn.
- **Name the floor.** Where a property ships as a floor (residual third-party free-text PII per contract
  10.9; single-region per ADR-11; pseudonymous-commit residual per GIT-1), the gate asserts **exactly the
  floor**, the residual is named in the artifact, and the follow-on is linked. A floor masquerading as done
  is the failure; an honest floor with a named gap is fine.
- **`[OPEN — LEGAL]` is an engineering-posture gate, not a legal sign-off.** Where counsel/DPO must ratify
  (the X-7 residual basis, the OQ-H worklog classification, the L-1 fail-static bound), the *structural*
  gate (DEK shred reaches it; it is never indexed/agent-read/analytics for a restricted subject) ships and
  is drilled **regardless**; the gate artifact carries a `LEGAL-RATIFICATION-PENDING` flag, not a pass.

**Ordering (doctrine §2 — order by non-negotiability).** Within this doc the gates are ordered by what kills
you first: **(B) the sandbox-escape drill** (one escape = cross-tenant catastrophe; the single hard go/no-go
before any untrusted code runs, ADR-20 / E-9) leads, then the rest of security, then **(A) GDPR/erasure**
(a leak or a missed holder is simultaneously a breach and a GDPR violation), then **(C) residency**, then
**(D) UX/design QA**. The gate-invariant (R-2): **no surface in any later part is "done" while a gate here is
red.**

---

# Part A — GDPR / Erasure gates

> Source spine: ADR-12 (the `PersonalDataHolder` contract is the spine; the holder list is **exhaustive**;
> "we forgot the search index" is a *structural* failure). Frozen contracts: 10.1 (`PersonalDataHolder`
> `{locate, export, rectify, restrict, erase}`), 10.2 (`#[personal_data]` classify derive + the
> `no-untagged-personal-data` lint), 10.3 (`data_map()/ropa()` generated from tags + holders), 10.4 (DSR state
> machine + `dsr_certificate → MerkleProvenBundle`), 10.8 (erasure ledger), 10.9 (the ONE free-text/immutable
> erasure posture, X-7, `[OPEN — LEGAL]`), 11.3/11.4 (KMS hierarchy + per-subject DEK crypto-shred). The
> holder universe is **H1–H18** (gdpr-and-audit.md §H-table). Family = the **erasure-reaches-every-holder**
> non-family drill (GA-1) + F3 (restore) + F1 (no-leak under restriction).

## A.1 — DSR fan-out COMPLETENESS (the "we forgot the search index" structural test)

**Property.** A subject- or tenant-scoped DSR (`locate`/`export`/`erase`) reaches **every**
`PersonalDataHolder` that the **generated data map** says holds that subject's data — no holder is silently
missed. Completeness is asserted **against the generated map, not a hand-written checklist** (ADR-12.6, 10.3):
the same artifact that drives the fan-out is the oracle that scores it, so the two cannot drift.

**Why structural, not enumerated.** The classic failure is a new store (a new cache namespace, a new
embedding table, the search index) that holds PII but was never added to the DSR list. Two mechanical bars
make the omission impossible *before* the drill even runs:
- **`no-untagged-personal-data` lint (contract 10.2 / E-5)** — a column/field carrying personal data that
  lacks a `#[personal_data(category, role, basis, retention, erasure, subject_locator)]` tag **fails the
  build**. Quantified: 0 untagged personal-data fields workspace-wide.
- **Harness auto-registration (contract 1.4)** — every store the service shell opens (OLTP DB, every blob
  prefix, every cache namespace, the search index a service owns) **auto-registers** as a holder. A store
  opened outside the harness is itself a lint failure.

**Drill (GA-1, the holder-completeness drill).** Seed one synthetic subject into **all H1–H18 holders**
(Git DB free-text + authorship; CI run-state/logs/artifacts/caches; Issues rows/comments/change-log/OLAP;
Knowledge blocks/db-rows/op-log/snapshots/agent-traces; Chat bodies/read-state/drafts/unfurl-cache;
object store; **search index incl. embeddings/vectors**; **event-bus history** (inline-PII events); caches/CDN;
**backups**; **agent memory/embeddings**; reference graph projections + edges; notification history; authz
tuples; audit (carve-out)). Then:
1. `data_map(tenant)` / `ropa(tenant)` enumerates the holders the subject touches.
2. `dsr_submit(erase, subject)` fans out over **exactly** that generated set (10.4); for multi-cell it
   **iterates `member_cells`** over the OQ-I bridge (12.6).
3. A post-erase `locate(subject)` across **every** holder returns **zero** recoverable PII.

**Quantified threshold.** **0 holders missed** (the fan-out set == the data-map set, asserted by set
difference); **0 recoverable PII** on post-erase `locate` across all H1–H18; the DSR completes inside the
**1-month statutory durable timer** (10.4). The completeness assertion is **map-driven**: |holders-reached Δ
holders-in-data-map| = 0.

**Green artifact.** The `dsr_certificate → MerkleProvenBundle` (10.4) listing every holder receipt; an
**erasure-ledger entry** (10.8, PII-free) per holder; a **map-vs-fanout set-diff report** = ∅; the
`reindex` completion records for the derived stores (search/refs/notif rebuilt, not restored). A non-empty
set-diff is a **hard red** — it means a holder exists that the map doesn't know about (the exact
"we forgot the search index" failure) and the DSR is incomplete by construction.

**Owner / cadence.** GDPR/Audit owns; the **lint runs per-change** (structural, cheap); the **full seeded
fan-out drill runs scheduled** (it touches backups + reindex). Every subsystem owns its holder leg
(Git D-2, CI D-3, Issues D11, Knowledge KD-4/KD-12, Chat D-C8) — all map to this one family with the **same
gate**, run per-holder.

## A.2 — Crypto-shred VERIFICATION (key destroyed → ciphertext unrecoverable in DBs, backups, immutable logs)

**Property.** Destroying a subject's (or tenant's) DEK/KEK renders their ciphertext **unrecoverable** in the
live DB **and in backups and in the append-only/immutable logs** — crypto-shred is the deletion primitive
that reaches the places hard-delete cannot (ADR-12.3, contract 11.3/11.4; EI-04 §1 event-log half).

**Drill (BUS-D8 + GA-3 + the per-subsystem erasure legs).** With the subject's free-text/body/op-log/
agent-memory/CI-log-segment columns encrypted under a **per-subject DEK** (11.4 granularity; bulk
pseudonym-referenced under per-tenant DEK; tenant-offboarding = the KEK):
1. `erase(subject)` destroys the per-subject DEK via `KeyOrigin::destroy` (11.3).
2. **Attempt to read the ciphertext** in: the OLTP row, the event-bus inline-PII events (the bus is a
   holder — 2.7), the CI log segments (11.4 per-subject CI-log DEK), the knowledge op-log/snapshots, and a
   **restored backup** (the GA-3 restore leg). Assert it is **undecipherable noise** — the key is gone, no
   path reconstructs plaintext.
3. Assert the `*.erased` tombstones are emitted on the log (2.7) and consumers degrade (the projection
   resolves to a `Tombstone{reason: erased}` per the OQ-D ladder, contract 5.7), not a 500.
4. Assert **per-subject key-shred granularity** holds: shredding subject A's DEK does **not** destroy
   subject B's content nor the author's legitimately-distinct content (the X-7 residual boundary — a
   subject's erasure shreds *their* DEK, not the author's).

**Quantified threshold.** **0 bytes of plaintext recoverable** for the erased subject across {live DB,
backups, immutable bus log, CI log segments, snapshots} after key destruction; **key-shred count is bounded
= one key per subject** (Knowledge CR-I, KD-4) so the operation is O(1), not a scan; `*.erased` tombstone
emitted for **100%** of the subject's inline-PII events; **0 collateral** (other subjects' / authors'
plaintext intact).

**Green artifact.** Key-destruction receipt (`KeyOrigin::destroy` returns the destroyed `dek-epoch`);
a **ciphertext-undecipherable assertion** per store (the drill attempts decrypt with all surviving keys →
fail); the tombstone-emission count; the **restore-then-read** result from GA-3 = unrecoverable. The
`pii_key_ref = kms://<tenant>/<dek-epoch>/<class>` of the destroyed key recorded in the erasure ledger.

**Owner / cadence.** Storage + GDPR/Audit own; **scheduled** (touches backups + KMS). Couples to the
restore-verify gate (A.4 / ADR-18) so a restore cannot resurrect a destroyed key.

## A.3 — Restriction-flag HONOURING (no index / agent / analytics / notification for a restricted subject)

**Property.** A subject under `restrict` (10.1 — the pending-erasure / objection state, distinct from
`erase`) is **suppressed everywhere a derived consumer might surface them**: their data is **not indexed,
not agent-readable, not in cross-individual analytics, and not in notifications** — for the *duration of the
restriction*, reversibly, before any destructive erase.

**Drill (an F1/no-leak instance scoped to the restriction flag).** Mark a subject `restrict`. Then assert,
across the derived-store fan-out:
- **Search (6.1/6.3):** the subject's content is absent from `query`/`semantic` results, **including counts,
  IDF contribution, "more results" pagination, and RAG retrieval** — suppression is structural via the
  index honouring the flag, not a post-filter. (`can_derive_plaintext_index()=false` already structurally
  skips HYOK content; the restriction flag is the per-subject analogue.)
- **Agent (8.x):** the restricted subject's free-text/memory is not returned to any `AgentRuntime::step`
  context (RAG, tool reads); agent-use suppressed.
- **OLAP / analytics (11.6):** the restricted subject is excluded from analytics; **worklog/productivity/
  estimate fields are excluded from cross-individual analytics by default** (OQ-H, restricted-by-default
  `data_role`).
- **Notifications (7.x):** no new notification *about* the restricted subject's restricted data fires; a
  humanised string referencing them degrades to the tombstone projection (NOTIF-D4), 0 title/PII leak.

**Quantified threshold.** **0 appearances** of a restricted subject in any search result/count/IDF/RAG,
agent context, analytics rollup, or new notification, for the full restriction window; **reversibility:**
lifting `restrict` re-admits the subject via `reindex(scope)` to **byte-parity** with the pre-restrict state
(F4 reindex-from-source — the suppression left no destructive residue).

**Green artifact.** A per-consumer **suppression-assertion report** (search-count-delta = 0, RAG-hit = 0,
analytics-row = 0, notif-fire = 0); the reversibility `reindex` parity record. This is the same machinery as
the erase gate minus key-destruction, so it reuses the holder fan-out harness.

**Owner / cadence.** GDPR/Audit owns the flag; each derived consumer (Search, Agent, OLAP, Notif) owns its
suppression leg; **per-change for the structural wiring** (the consumer must read the flag — a lint-able
property), **scheduled for the seeded fan-out**.

## A.4 — Post-restore RE-ERASURE (a restore must not resurrect an erased/restricted subject)

**Property.** Restoring a backup taken *before* an erasure does **not** resurrect the erased subject's PII or
their previously-revoked grants; the **post-restore re-erasure** (GD-14, contract 10.8) re-applies the
erasure ledger so backups cannot defeat Art. 17.

**Drill (GA-3 / F3 restore leg + ID-D8).** (1) Seed + erase a subject (A.1/A.2). (2) Restore the cell to a
`to_offset` *before* the erasure (11.5). (3) Assert the **erasure ledger re-applies** automatically:
`post_restore_reerase` (11.5) destroys the re-materialised keys and re-tombstones, so the restored state has
**0 resurrected PII** and **0 resurrected grants** (a revoked authz tuple does not come back live —
ID-D8). (4) Assert **cross-seam consistency** of the restore itself (the ADR-18 row↔blob↔index↔offset point)
so the re-erasure operates on a coherent snapshot, not a torn one.

**Quantified threshold.** **0 resurrected erased subjects**, **0 resurrected grants past an erasure**,
post-restore; the re-erasure runs **automatically** (gated, not manual) within the restore procedure; the
restore lands at **one mutually-consistent** OLTP↔blob↔index↔event-offset point (the F3 gate, shared with
strategy Part 01 / ADR-18).

**Green artifact.** The restore-verify CI artifact (ADR-18, CI-gated) **extended with a re-erasure
assertion**: erasure-ledger-replay receipt + post-restore `locate(subject)` = 0. This is the durability
gate's GDPR leg.

**Owner / cadence.** Storage + GDPR/Audit; **scheduled restore-verify** (ADR-18 wires it into CI as a
durability gate; the re-erasure leg is its compliance assertion).

## A.5 — The git-history erasure POSTURE test (the immutability half, named as a floor)

**Property (a *floor* gate — names exactly what the structural mechanism does and does not erase).** For
immutable git commit bytes, the platform's **one posture** (X-7 / contract 10.9, instantiated by Git by
reference) holds **structurally**: (a) commit **author identity** is **pseudonymous-by-default** (GIT-1,
`<pseudonym>@<tenant>.noreply`), so the immutable hash never bakes erasable PII into author metadata in the
first place; (b) self-authored free-text (commit *bodies* written by the subject) is reachable by per-subject
DEK shred where isolable; (c) the **residual** — third-party PII typed into a commit message *body by a
different author* — is handled by the documented `[OPEN — LEGAL]` lawful-basis limit + best-effort
`rectify`/tombstone + the audited **history-rewrite path** (10.6) for the rare must-expunge case.

**Drill (Git D-2 + the posture residual).** Erase a subject who authored commits/PRs/comments + uploaded
LFS:
1. Assert the **pseudonym-map shred** (4.8) makes the author unresolvable: the immutable commit bytes now
   carry only the opaque pseudonym; `resolve_pseudonym(subject)` → gone. **0 recoverable author PII** in the
   immutable structure.
2. Assert **crypto-shred reaches the shreddable git surfaces** — reflogs, bitmaps, and **pack-tier backups**
   are encrypted under the per-tenant blob DEK and become unrecoverable (gdpr-and-audit §; STOR-5 keeps the
   pack tier relocatable so its shreddable reflogs/bitmaps/backups move with it). **Not** the commit-object
   bytes themselves — that is the named residual.
3. Assert the residual is **EXACTLY the platform-posture residual (10.9), nothing more** (Git D-2 gate): a
   third-party name in a commit *body* is the only thing left, it is **never indexed / never agent-read /
   never in analytics** (the `restrict` suppression, A.3), and the **history-rewrite tool** (10.6) — when a
   tenant invokes it — is **audited, rate-limited, and fans out clone/mirror/fork-cache invalidation**, with
   the understood hash-changing consequence.

**Quantified threshold.** **0 recoverable author-identity PII** in immutable bytes after pseudonym shred;
crypto-shred reaches reflogs/bitmaps/pack-backups (**0 recoverable there**); the residual is **bounded to the
10.9 residual** (set-equality assertion: residual-PII-found ⊆ {third-party free-text in commit bodies}, and
that set is `[OPEN — LEGAL]`-flagged, never indexed/agent/analytics); a history-rewrite op emits an audited,
rate-limited record with **complete** cache-invalidation fan-out.

**Green artifact.** The DSR receipt set + erasure-ledger entry scoped to the **ONE-posture residual**;
the reindex completion; for a history-rewrite: the **tamper-evident audit-log entry** (10.6) + the
invalidation fan-out manifest (clones/mirrors/fork-caches). The artifact carries the
`LEGAL-RATIFICATION-PENDING (L-2)` flag on the residual basis — the **structural floor passes; the legal
residual is named, not claimed solved**.

**Owner / cadence.** Git + GDPR/Audit + Legal/DPO (L-2). Pseudonym/crypto-shred legs **scheduled** with the
holder fan-out; the history-rewrite path **scheduled** (disruptive); the `[OPEN — LEGAL]` residual is a
standing gap-report item (E-3), not a CI green.

---

# Part B — Security gates

> Source spine: ADR-20 (one sandbox; the real-kernel escape drill is the single hard go/no-go), ADR-03
> (ReBAC, leak-free `list_objects`), ADR-11 (cell isolation, tenant-from-token). Frozen contracts: 4.3
> (`list_objects` `SetExpr` push-down, the leak-free pre-filter — the most load-bearing inter-system
> contract), 5.9 (Git↔CI `CheckStatus` seam + fork trust-tier), 8.4 (`ToolHands::exec` = the CI runner's
> `kind=agent` job), 11.2 (trust-tier/branch-scoped cache namespaces). Families: the **escape** non-family
> drill (AG-D4 / CI T-1), F1 (no-leak), F2 (cross-tenant IDOR), F9 (delegation/loop confinement). Lints
> (E-5): `no-host-exec`, `tenant-predicate`, `search-requires-acl-filter`, `no-cross-db`.

## B.1 — The sandbox-ESCAPE drill on a REAL kernel (ADR-20 / E-9 — the single hard go/no-go) **[LEADS by non-negotiability]**

**Property.** Untrusted customer code — **CI jobs OR agent `ToolHands::exec` runs** (they are the *same*
runner under one job spec `kind ∈ {ci, agent}`, contract 8.4 / ADR-20) — **cannot escape the isolation
boundary** on a **real production-backend kernel**. One escape is a cross-tenant catastrophe; an undrilled
isolation property is a **claim, not a fact** (EI-04 §5.1).

**Drill (CI T-1 / AG-D4 — the adversarial corpus on a real kernel).** Run an adversarial corpus inside the
production-backend sandbox (gVisor-class userspace-kernel **or** microVM, ADR-20 floor) on a real kernel:
- kernel-exploit primitives (the named CVEs/escape techniques for the chosen backend);
- **cloud-metadata SSRF** (169.254.169.254) → credential theft;
- **control-plane / internal-RPC reach** from inside the guest (the public↔internal security boundary,
  contract 1.2);
- **cross-tenant network / storage** reach;
- fork bomb / disk fill / `pids.max` + zero-swap exhaustion;
- **secret exfil via egress** (egress is default-deny; the run must not reach the network to leak a secret).

Assert the named hardening profile holds: egress default-deny, read-only root + tmpfs, all caps dropped,
no-new-privileges, seccomp, **digest-pinned images fail-closed on an un-digested tag**, whole-guest kill on
teardown.

**Quantified threshold.** **Zero escapes.** Not "low" — **zero**. Re-run on **every backend / image / kernel
change** (the property is per-kernel; a kernel bump invalidates the prior green).

**Green artifact.** A **green escape-drill attestation** (signed, dated, naming the kernel/backend/image
digests it was true for). **Absent a current green attestation, CI is NO-GO for untrusted code** (E-9): the
go/no-go is mechanical — the deploy gate reads the attestation freshness, not a human's word.

**Owner / cadence.** CI owns the runner + the corpus (TE-28 threat model); Agent Fabric feeds agent runs
through the same spec. **Scheduled + on-every-backend/kernel-change**; it is the **Phase-6 milestone**, the
**Phase-8 go/no-go** (E-9), and gates B.4 below. The failure-injection harness reuses this sandbox substrate.

## B.2 — Authz LEAK tests (a viewer never finds/sees what `list_objects` must exclude) — F1

**Property.** A viewer **never finds or reads** an object they lack permission for, through **any** read
surface — board/list scan, search (FT + structured + vector), reference backlinks/traverse, chat unfurl,
notification, context pane — because every such surface **conjoins the `list_objects` `Filter{set_expr,
zookie}` pre-filter into its native query** (contract 4.3 / OQ-E) rather than post-filtering. A leak here is
simultaneously a security breach and a GDPR breach (SC-1).

**Drill (F1, run across all five subsystems' read surfaces — ID-D4, REF-D1, SRCH-D1, NOTIF-D4, Issues D3,
the Git/CI/KN/Chat leak scans).** Seed an adversarial corpus: a confidential issue, a knowledge page with an
inheritance *override* (visible parent, restricted child), a private channel, a fork-restricted CI run, a
confidential artifact that *references* a public one. For an unauthorised viewer assert the restricted
object is absent from:
- the board/backlog `SetExpr` JOIN result (the push-down conjoin, not a post-filter);
- **search results AND their second-order signals** — result rows, **facet counts, IDF/ranking
  contribution, "N more results" pagination, and RAG/`semantic` k-NN retrieval** (the count/IDF/ranking-leak
  bar, SRCH-D1/D8: k nearest *visible* neighbours, filter-during-traversal, not k-then-filter);
- backlinks / `traverse` (including filter-mode and the confidential→public reference case, REF-D1);
- notifications (the humanised string is the tombstone "a restricted issue"; **title never appears**; item
  suppressed if the recipient can't see the subject — NOTIF-D4);
- **under zookie staleness** — a scan passing the post-revoke zookie reads the authz reverse index
  at-or-after that revision watermark (contract 4.10), so a just-revoked grant does not leak through the
  fail-static cache.

**Quantified threshold.** **0 leaked docs / edges / backlinks / notifications**, **0 count/IDF/ranking
leak**, **0 RAG leak**, across the adversarial corpus, **including under zookie staleness**. The
`search-requires-acl-filter` lint (E-5) makes a search query that omits the ACL `Filter` a **build
failure** (0 such queries workspace-wide) — the structural complement to the runtime drill.

**Green artifact.** A per-surface **leak-scan report** (restricted-object-appearances = 0 in each of:
list/search/refs/notif/context-pane, and in counts/IDF/RAG); the lint result (0 un-filtered search queries);
authz-deny counters from telemetry (the survival signal — the deny *fired and was observed*).

**Owner / cadence.** Identity owns the `list_objects` contract + the per-tenant authz reverse index; each
consumer owns its leak scan; the **lint runs per-change**; the **seeded adversarial corpus runs scheduled**
(and on any authz-engine change).

## B.3 — Cross-tenant IDOR / no cross-tenant query path — F2

**Property.** There is **no cross-tenant (and no cross-cell) query path**. Tenant is taken **from the token,
never from the URL path** (ID-3); a request whose token-tenant ≠ path-tenant is an IDOR and is rejected at
the front door.

**Drill (F2 — SUB-D7, ID-D3, REF-D2, SRCH-D3, Git D-8, and every front door).** Issue reads/writes with a
**valid token for tenant A but a URL path naming tenant B** (and crafted cross-tenant `ArtifactRef`/URN, and
a search scoped to another tenant, and a git-wire fetch for another tenant's repo). Assert every entry point
(public gateway, internal RPC behind it, git smart-transport wire, CLI, agent runtime) resolves the tenant
from the **credential** (contract 4.1) and rejects the cross-tenant access.

**Quantified threshold.** **0 cross-tenant rows / edges / search results / tuples / repo bytes** readable;
the **`tenant-predicate` lint** (E-5) catches a **tenant-less query at compile time** (0 tenant-less queries
in the workspace — the structural complement). Cross-cell: the OQ-I bridge carries only the opaque
`CrossCellPointer` (12.6) — **0 PII crosses a cell boundary** (only the already-permission-filtered,
cell-local-rendered projection does).

**Green artifact.** Per-front-door **IDOR-rejection report** (cross-tenant attempts = 0 successful);
authz-deny counts; the `tenant-predicate` lint result; for cross-cell, a **bridge-frame assertion** (the
pointer carried no name/email/body — the `control-plane-pii-free` lint, B.6 / C.2).

**Owner / cadence.** Identity + every gateway/subsystem; **lint per-change**, **spoof drill scheduled + on
any routing/gateway change**.

## B.4 — The poisoned-pipeline-execution test (a fork PR cannot turn its own gate green) — X-1

**Property.** A pull request **from a fork** (running attacker-controlled CI config / untrusted contributor
code) **cannot satisfy a required merge-gate check by itself**, cannot read protected secrets, and cannot
write the trusted (default-branch) cache scope. The classic poisoned-pipeline-execution attack is structurally
foreclosed by the **fork trust-tier** half of the frozen `CheckStatus` seam (contract 5.9 / X-1).

**Drill (Git D-10 + CI D-6/D-7/D-8 — the fork-trust correctness drill).** With a fork PR whose run is stamped
`trust_tier = untrusted_fork` by CI from run provenance:
1. **Self-green attempt:** the fork's CI run reports `state = success` for a `required` context → assert the
   merge gate treats it as **`neutral` for gating** (the merge is **blocked**) until a trusted principal
   **endorses** via `check(subject, approve_untrusted_ci, repo)` **or** the context is **re-run under
   `trust_tier = trusted`**. A fork **cannot self-green**.
2. **Secret-read attempt** (CI D-7): the fork run tries to read protected secrets → the `read &
   !is_untrusted_fork` ABAC edge **denies**; protected-env secrets require explicit grant/approval. **0
   secret reads.**
3. **Cache-poison attempt** (CI D-6): the fork run tries to write the default-branch cache scope → the
   **trust-tier/branch-scoped cache namespace** (11.2) holds structurally. **0 trusted-cache writes** from a
   fork-tier run.
4. **Supersession correctness** (Git D-10 / CI D-8): deliver `ci.check.updated` **out of order + duplicated**
   for one `(commit_oid, context)` → the **`run_attempt`-monotonic supersession** (clocks are *not*
   authority) holds exactly one current row; a **lower** `run_attempt` arriving late is **dropped**; the
   merge-queue workflow wakes on a **doubly-delivered `ci.result` exactly once** (idempotent on
   `idem_token`). **0 double-merge, 0 spurious unblock.**

**Quantified threshold.** Fork **cannot self-green** (0 fork-only merges of a required-context gate); **0
secret reads** by a fork-tier run; **0 trusted-cache writes** by a fork-tier run; **exactly one** current
`check_status` row per `(commit_oid, context)`; **exactly one** merge-queue wake per `ci.result`; **0
double-merge**.

**Green artifact.** Gate-state-transition log (fork-success → neutral → endorsed/re-run-trusted → green);
the secret-deny counter; the cache-scope-deny counter; `check_status` row-churn + dropped-stale-attempt
count; merge count == 1 per PR. Depends on B.1 (the run executes inside the proven sandbox).

**Owner / cadence.** Git (gate + projection + endorsement) + CI (producer + trust-tier stamp) + Identity
(`approve_untrusted_ci` relation). **Per-change for the seam contract test** (cheap, no real kernel),
**scheduled for the full adversarial fork run** (needs the sandbox).

## B.5 — Secret-handling (resolved inside the boundary, never forwarded)

**Property.** Secrets are referenced **by name only**, **resolved inside the sandbox boundary**, scoped to
the job's references, **never baked into images**, and **never handed to the agent runtime to forward**
(ADR-20 / CI-1). A compromised agent runtime or a job cannot exfiltrate a secret it was never given the
plaintext of.

**Drill.** (1) A job/agent run references a secret by name → assert the plaintext is materialised **only
inside the guest**, never in the dispatching runtime, never in the agent's `AgentRuntime::step` context, never
in logs. (2) An adversarial run attempts to read a secret outside its reference scope → denied. (3) The
egress-default-deny + exfil leg of B.1 confirms a resolved secret cannot leave the boundary over the
network. (4) Per-run token (4.7): the shared platform token is **scrubbed from the child environment**
(ID-2, anti-leak) and the run executes under the per-run attenuated token only.

**Quantified threshold.** **0 secret plaintext** outside the guest boundary (not in the runtime, the agent
context, logs, or images); **0 shared platform tokens** leaked into the child env (ID-2); secret scope =
exactly the job's declared references (0 out-of-scope reads).

**Green artifact.** A **secret-flow assertion** (plaintext appears only inside the guest — a taint/scan over
the runtime + agent-context + log surfaces = 0 hits); the env-scrub assertion (AG-D8: 0 shared token in
child env); the egress-exfil leg green from B.1.

**Owner / cadence.** CI (runner secret resolution) + Agent Fabric (no-forward) + Identity (per-run token,
scrub-on-teardown). **Scheduled** with the escape drill (shares the sandbox harness); the `no-host-exec`
lint (E-5) per-change forecloses the bypass path.

## B.6 — Supporting structural gates (committed lints — the ratchet)

These are **per-change CI lints** (E-5, the ratchet, doctrine §5) that make whole bug-classes impossible
*before* a drill runs. Each is a committed gate with a **0-violation threshold**; an uncommitted lint is no
gate.

| Lint (contract 1.6 / E-5) | Forecloses | Threshold |
|---|---|---|
| `no-host-exec` | any execution path bypassing `ToolHands::exec` (AG-2) | 0 host-exec bypasses |
| `no-cross-db` | a subsystem reading another subsystem's store | 0 cross-subsystem DB deps |
| `tenant-predicate` | a query without a tenant predicate (the IDOR root) | 0 tenant-less queries |
| `search-requires-acl-filter` | a search query omitting the `list_objects` `Filter` | 0 un-filtered search queries |
| `no-raw-publish` | a bus emit outside the outbox helper (BUS-2) | 0 raw publishes |
| `no-llm-in-platform` | an LLM SDK/prompt/model-name in platform code (ADR-08) | 0 occurrences |
| `no-untagged-personal-data` | a personal-data field without a `#[personal_data]` tag | 0 untagged fields |
| `control-plane-pii-free` | PII in a control-plane store/bridge frame | 0 PII in control plane |
| `residency-pin` | a write that could land cross-region | 0 unpinned writes (→ Part C) |

**Green artifact.** The committed CI lint run (all 0). Violations are **loud, never `|| true`-swallowed**
(doctrine §5).

---

# Part C — Residency / EU-sovereignty gates

> Source spine: ADR-11 (region binding is **immutable-by-default and enforced at the data layer** — misrouting
> a tenant's personal data is *impossible*, not discouraged; the global control plane **holds no in-region
> personal data**; **no cross-region query path for personal data**). Frozen contracts: 12.1
> (`(tenant, region)` first-class partition key), 12.2 (`discover`/`placement_of` PII-free routing,
> repo-granular region-pinned placement), 12.4 (`residency_verify → SignedAttestation`, no-global-pool
> attestable incl. CI runner/log/artifact/cache region), 12.6 (cross-cell PII-free pointer bridge). Lint:
> `residency-pin` (E-5).

## C.1 — Region-pinning ENFORCED (a write asserts row.region == cell.region; misrouting impossible)

**Property.** Every write of personal data lands in the tenant's **immutable** region; a misroute is a
**compile-/admission-time error, not an operational risk** (ADR-11.2). The region is a **compiled-in shard
key validated at the write boundary** — `row.region == cell.region` is an invariant, not a hope.

**Drill (CI R-3 + the residency write-boundary test).** (1) Attempt a write whose `region` ≠ the serving
cell's region → assert it is **rejected at the write boundary** (admission error), not silently accepted.
(2) The **`residency-pin` lint** (E-5) catches a write path that could land cross-region **at compile time**.
(3) For CI specifically (R-3): an EU-resident tenant's run is **claimed only by an in-region runner**, and
its **logs / artifacts / caches / run-state never leave the region** (the within-EU CDN clone/bundle class,
11.2, is EU-edge only).

**Quantified threshold.** **0 cross-region writes** of personal data accepted; the `residency-pin` lint
passes on **every** write path (0 unpinned writes); for CI, the job is claimed **only** in-region and
logs/artifacts/caches **never leave the region** (0 out-of-region placements).

**Green artifact.** The write-boundary rejection assertion (cross-region write → admission error); the
`residency-pin` lint result; the CI `residency_verify` attestation covering runner pool + log/artifact/cache
region.

**Owner / cadence.** Tenancy/control plane owns the partition key + admission check; **lint per-change**;
the **write-boundary + CI residency drill scheduled**.

## C.2 — The control-plane-PII-FREE attestation

**Property.** The **global control plane holds zero in-region personal data** (ADR-11.4): it carries only
the PII-free routing/placement registry (`tenant → {cell(s), region}`, `discover`/`placement_of`, 12.2/12.3)
and the PII-free cross-cell `CrossCellPointer` bridge frame (12.6, opaque `subject` — **never a name/email/
body**).

**Drill (the control-plane-pii-free structural test).** Scan every control-plane store + every cross-cell
bridge frame for personal data; assert the `control-plane-pii-free` lint (E-5) holds at compile time (a
control-plane schema field tagged `#[personal_data]` is a build failure), and a runtime scan of the routing
registry + bridge frames finds **0 PII**. Assert cross-cell resolution is **always cell-local** (OQ-I): a
viewer in cell A rendering a pointer to cell B gets only B's **already-permission-filtered, already-rendered
projection** back — never B's raw rows.

**Quantified threshold.** **0 personal-data fields** in any control-plane store or bridge frame (lint = 0,
runtime scan = 0); **0 PII crosses a cell boundary** (only the cell-local projection does).

**Green artifact.** The `control-plane-pii-free` lint result + a runtime **PII-scan report** over the routing
registry and a sample of cross-cell bridge frames (= 0); the cell-local-resolution assertion (the projection
came from B, permission-checked in B).

**Owner / cadence.** Tenancy/control plane; **lint per-change**, **runtime scan scheduled**.

## C.3 — No-cross-region-QUERY (residency attestable, no global pool)

**Property.** There is **no cross-region query path for personal data** (ADR-11 consequence); the
no-global-pool property is **attestable** — every store reports the tenant's region and a signed attestation
proves the tenant's data plane is wholly in-region, **including the CI runner/log/artifact/cache region**
(12.4).

**Drill (the residency-verify attestation drill).** (1) `residency_verify(tenant_id) → SignedAttestation`
(12.4): every store (OLTP, blob, search, authz tuples, bus history, CI log/artifact/cache, OLAP) reports its
region; assert all == the tenant's pinned region. (2) Attempt a query that would join/read across regions for
personal data → assert **no such path exists** (the data layer has no cross-region read for PII; the bridge
is projection-only, C.2). (3) `myelin tenant residency verify` (12.4) produces the attestation an auditor can
check.

**Quantified threshold.** `residency_verify` returns a **signed attestation** in which **100% of the
tenant's stores (incl. CI runner/log/artifact/cache) report the pinned region**; **0 cross-region query paths
for personal data**.

**Green artifact.** The **`SignedAttestation`** (12.4) — the auditor-checkable, dated artifact naming every
store's region; the no-cross-region-path assertion (the data layer rejects/lacks a cross-region PII read).

**Owner / cadence.** Tenancy/control plane + every store (reports its region); **scheduled** (the attestation
is an operable command; CI runs it per tenant-shape in the residency drill).

---

# Part D — UX / Design QA gates

> Source spine: design-language.md §4 (a11y baseline: WCAG 2.2 AA, EN 301 549 public-sector readiness, full
> keyboard operability, contrast as a token constraint, status-never-by-colour) + §8b (the day-one *testable*
> mandates). Directives: **T-7** (switch test = drive the REAL UI), **T-8** (measured-contrast over the token
> table; latency budgets; popovers tested against the real anchor), KN-4 (one editor render path), NOTIF-1
> (humanise at the backend). These are the **frontend definition-of-done** (8b.7) and bind in Phase 8.

## D.1 — The SWITCH TEST (drive the real UI in a browser; done = a team could move without hitting a wall)

**Property (the frontend definition-of-done, T-7 / 8b.7 / doctrine §4).** A surface is **done only when, by
driving the REAL UI in a browser, a team could move to it from the incumbent tool without hitting a wall the
old tool didn't have.** This verdict is reached by **driving it**, not by reading a feature list — a
"does this feel finished?" pass over the real surface finds a dozen-plus issues a checklist misses.

**Drill (T-7 + T-6 chained-mutation E2E).** For each subsystem's core loop, drive the **real UI in a real
browser** through the incumbent-equivalent workflow as a **chained-mutation session** (real sessions chain
mutations and update state mid-flight — doctrine §4 / T-6 — which is exactly where the bugs live), not
isolated single-handler calls:
- **Issues** (D14): a Jira/Linear user completes create → triage → plan → board → done **without a manual**.
- **Git**: open PR → review → see checks (incl. the X-1 fork-trust + checks panel + merge-queue affordances)
  → merge.
- **Knowledge**: create page → edit (the real editor) → embed an artifact → publish.
- **Chat** (D-C19): join channel → message → reference an artifact (unfurl) → thread.
- **CI**: trigger → watch logs → jump-to-failure (the `details_ref` `#step-<n>` anchor) → re-run.

**Quantified threshold.** The core loop completes **end-to-end in a browser without hitting a missing-capability
wall**; **every primary screen** exercises its **empty / loading / error / permission / erased / agent-pending
states** as **tested states** (D.4) — not just the happy path. (The design folders already enumerate these
states per primary screen — Git OQ-12, Issues/Chat S-screens; the switch test *drives* them.)

**Green artifact.** A **switch-test session transcript/recording** per core loop (the drive-through is the
proof, doctrine §4), naming any wall found (→ a gap-report item, E-3) and confirming each state was reached.
The verdict is **driven, not asserted from the feature list**.

**Owner / cadence.** Each subsystem's frontend owner; **scheduled** (human/agent-driven browser session) + a
**Playwright-class automated chained-mutation E2E** per loop in CI for the regression-able parts.

## D.2 — Design-language gates — render(parse(md)) === md round-trip over a corpus (KN-4 / 8b.2)

**Property.** The editor has **one render path** (read and edit run the **same** inline parser, KN-4); the
round-trip invariant **`render(parse(md)) === md`** holds over a corpus — the correctness bar for the editor
regardless of which concurrency engine Knowledge picks (CAS floor → CRDT).

**Drill (KD-2 / Issues D10 — the corpus round-trip gate).** Run `render(parse(md)) === md` over a
markdown-subset corpus exercising the frozen `myelin-content` inline grammar (13.1): `**bold**`, `*italic*`,
`` `code` ``, `~~strike~~`, `[text](url)`, and the **three structured nodes** (`mention`/`artifact_ref`/`embed`,
`U+FFFC`-anchored) **nested inside bold/lists/tables**, `code_block` (raw, not md-parsed), plus **IME / paste /
Enter-splits-block** edge cases. Assert the consumed-subset variants (Chat, Issues) round-trip on the **same
WASM parser** (one render path, 13.1).

**Quantified threshold.** **100% round-trip; 0 corpus regressions.** A single non-round-tripping corpus entry
is a **hard red** (it means read and edit diverge — the "not a real editor" tell).

**Green artifact.** The **corpus pass-rate report** (100%) + the regression count (0), committed and run
**per-change in CI** (it is a cheap, deterministic gate — 8b.2 names it a Phase-5 CI gate). The editor
primitives (serializer, offset model, Enter-splits/caret-after-split DOM surgery) are **unit-tested
standalone before the integrated editor** (KN-4) — those unit suites are the upstream green.

**Owner / cadence.** Knowledge leads the parser; Chat/Issues run the subset corpus; **per-change in CI**.

## D.3 — Design-language gates — measured contrast ≥ AA + latency budgets (T-8 / 8b.3 / §4)

**Property.** Contrast is **measured over the real token table, never trusted from a stated ratio** (a brand
accent at ~2.8:1 fails AA); interaction latency meets **hard numeric budgets**.

**Drill + thresholds (T-8 / 8b.3 / 8b.6 / §4).**
- **Measured contrast (PROVEN — WCAG 2.2 AA, EN 301 549):** compute the contrast ratio for **every** semantic
  text/background and focus-ring pair in the token table, in **light / dark / high-contrast** themes.
  Threshold: **≥ 4.5:1 normal text, ≥ 3:1 large text & UI/focus indicators** (AA); AAA pursued on primary
  reading + code surfaces where feasible. The **focus token is asserted distinct from the brand accent**
  (8b.3) when the accent fails AA. **0 token pairs below AA.**
- **Status never by colour alone (PROVEN — colour-blind):** assert every functional/status treatment carries
  **glyph + text label + position**, not colour alone; **no saturated status fills**. 0 colour-only status.
- **Keyboard latency (8b.6, measured in Phase 5):** keyboard response **< ~100 ms p99**; the Issues
  flexible-field board query meets the **< 1 s keyboard budget** with the `SetExpr` JOIN conjoined (D2 —
  never an unbounded JSONB scan).
- **Suppress-flash / render-not-animate (8b.6):** flash-of-spinner suppressed under **~1 s** (loading shows
  **structure/skeletons matching final layout**, never a spinner on blank); **pages render, they don't
  animate in**.

**Green artifact.** A **measured-contrast report** over the token table (every pair, every theme, ≥ AA, 0
failures) — emitted from the **product's real tokens** (the live styleguide, 8b.6, runnable with the stack
down, so the report can't drift from the app); a **latency report** (keyboard p99 < 100 ms; board query <
1 s; no spinner < 1 s). The **`no-inline-colour-on-interactive` lint** (E-5 / 8b.3, inline style beats
`hover:`/`focus:` specificity) is the per-change structural complement.

**Owner / cadence.** Design-system owner; **contrast + inline-colour lint per-change** (cheap, deterministic);
**latency budgets measured scheduled** under load.

## D.4 — Empty / loading / error states are TESTED states + overlay portal/focus/z-index + hover-isn't-touch/mobile

**Property.** The non-happy states and the overlay/responsive bug-classes that 8b names are **tested states**,
not afterthoughts — they are the exact net-new bug-classes that make a feature pass every unit test and be
unusable the first time a human opens it (doctrine §4).

**Drill + thresholds (8b.1 / 8b.4 / §5.10).**
- **Empty/loading/error/permission/erased as tested states (8b.6 / §5.10):** every primary screen renders all
  six states correctly — loading shows **structure** (skeleton matching final layout, never a blank spinner);
  error blames the **system** in one quiet line + a path (never the user); a degraded surface **fails static**
  ("temporarily unavailable" for that surface only); permission/erased render the **tombstone** (consistent
  with the OQ-D ladder + the leak gate B.2 — an erased/denied subject shows a tombstone, never PII). Threshold:
  **all 6 states tested per primary screen; 0 raw-id / unrendered-markdown / blank-spinner tells** (the
  NOTIF-1 "feels unfinished" #1 tell — humanisation lives at the backend, so the frontend can't regress it).
- **Overlay primitives (8b.1, PROVEN — WCAG/ARIA + transform-clipping):** Dialog/Confirm/Popover/Dropdown/
  Tooltip/Toast **portal to document root** (the "dialog renders inside the 240px sidebar" bug is forbidden by
  construction); **one documented z-index scale** (chrome < popover < modal < toast — 0 magic per-component
  z-indexes); **focus-trap + return-focus, scroll-lock with scrollbar-width compensation, Escape + backdrop
  dismiss, correct ARIA** inherited from the primitive (consumers never re-implement). Threshold: **0
  clipped/mis-layered/focus-leaking overlays**.
- **Popovers tested against the REAL anchor (T-8 / 8b.4):** flip-above + max-height when off-screen; **a
  picker under a bottom-pinned chat composer must render on-screen** (D-C19). Threshold: **0 off-screen
  popovers** against the real anchor.
- **Hover-isn't-touch + mobile width-takeover (8b.4, PROVEN bugs):** row actions (issue-list, chat
  message-hover, knowledge backlinks) are **touch-reachable** by default or behind an explicit mobile
  affordance; **`width:100%` is not a takeover** (collapse the sibling column at the breakpoint, not a
  clipped-off-screen panel); the shell is pinned to the viewport (`100vh`/`overflow:hidden`, scrolling flex
  child has `min-height:0`). Threshold: **0 hover-only actions unreachable on touch; 0 clipped-off-screen
  mobile panels**.

**Green artifact.** A **state-matrix report** (6 states × every primary screen, all tested); an **overlay
conformance report** (portal-root, z-scale, focus-trap, real-anchor flip — all pass); a **responsive/touch
report** (0 hover-only-unreachable, 0 width-takeover clip). Driven in a real browser (D.1) + automated for the
regression-able parts.

**Owner / cadence.** Design-system owner (the shared overlay/state primitives, built **before any feature
consumes them** — a Phase-6 sequencing prerequisite, R-3) + each subsystem frontend; **automated overlay/
state E2E per-change**, **the full driven responsive pass scheduled**.

---

## E. Coverage map — every in-scope gate → its family, drill ID(s), threshold, artifact

| Gate | Family / kind | Drill ID(s) | Quantified threshold | Green artifact | Cadence |
|---|---|---|---|---|---|
| A.1 DSR fan-out completeness | erasure-reaches-every-holder | GA-1; Git D-2, CI D-3, ISS D11, KD-4/12, Chat D-C8 | 0 holders missed (map-driven), 0 PII recoverable, ≤ 1-month | `dsr_certificate` Merkle bundle + map-vs-fanout set-diff = ∅ + erasure-ledger | lint per-change; fan-out scheduled |
| A.2 Crypto-shred verification | erasure (key) + F3 | BUS-D8, GA-3, KD-4 | 0 plaintext in {DB, backups, immutable log, CI logs}; 1 key/subject; 0 collateral | key-destroy receipt + ciphertext-undecipherable assertion + tombstone count | scheduled |
| A.3 Restriction-flag honouring | F1 (scoped to restrict) | SRCH/AG/OLAP/NOTIF restrict legs | 0 appearances (search/IDF/RAG/agent/analytics/notif); reversible to byte-parity | per-consumer suppression report + reindex parity | wiring per-change; fan-out scheduled |
| A.4 Post-restore re-erasure | F3 | GA-3, ID-D8, SUB-D6 | 0 resurrected PII, 0 resurrected grants; one consistent restore point | restore-verify artifact + re-erasure assertion | scheduled (ADR-18 CI gate) |
| A.5 Git-history erasure posture (FLOOR) | erasure + `[OPEN — LEGAL]` | Git D-2 + 10.9 residual | 0 author PII in immutable bytes; residual ⊆ third-party body PII (never indexed/agent/analytics) | DSR receipts scoped to 10.9 residual + audited history-rewrite manifest; `LEGAL-PENDING (L-2)` | scheduled; gap-report item |
| **B.1 Sandbox escape (real kernel)** | **escape (the hard gate)** | **CI T-1 / AG-D4** | **Zero escapes** (per kernel/backend/image) | **green escape attestation; absent → CI NO-GO** | **scheduled + every backend/kernel change** |
| B.2 Authz leak (list_objects) | F1 | ID-D4, REF-D1, SRCH-D1/D8, NOTIF-D4, ISS D3 | 0 leaked docs/edges/notifs, 0 count/IDF/ranking/RAG leak, incl. zookie staleness | per-surface leak-scan = 0 + `search-requires-acl-filter` lint + deny telemetry | lint per-change; corpus scheduled |
| B.3 Cross-tenant IDOR | F2 | SUB-D7, ID-D3, REF-D2, SRCH-D3, Git D-8 | 0 cross-tenant rows/edges/results/tuples/bytes; `tenant-predicate` lint = 0 | IDOR-rejection report + tenant-predicate lint + bridge-frame PII = 0 | lint per-change; spoof drill scheduled |
| B.4 Poisoned-pipeline (fork self-green) | X-1 fork-trust | Git D-10, CI D-6/D-7/D-8 | fork cannot self-green; 0 secret reads; 0 trusted-cache writes; 1 row/key, 1 wake, 0 double-merge | gate-state log + deny counters + supersession churn + merge==1 | seam test per-change; fork run scheduled |
| B.5 Secret handling | secret-flow | AG-D8 + B.1 exfil leg | 0 secret plaintext outside guest; 0 shared token in child env; scope-exact | secret-flow taint scan = 0 + env-scrub assertion | scheduled (+ `no-host-exec` lint per-change) |
| B.6 Structural lints | ratchet | E-5 lint suite | 0 violations each | committed CI lint run (loud, no `\|\| true`) | per-change |
| C.1 Region-pinning enforced | residency | CI R-3 + write-boundary | 0 cross-region PII writes; `residency-pin` lint = 0; CI in-region only | write-boundary rejection + residency-pin lint + CI attestation | lint per-change; drill scheduled |
| C.2 Control-plane PII-free | residency (structural) | control-plane-pii-free scan | 0 PII in control plane / bridge frames; cell-local resolution | `control-plane-pii-free` lint + runtime PII scan = 0 | lint per-change; scan scheduled |
| C.3 No-cross-region-query / attestation | residency | residency_verify | 100% stores report pinned region (incl. CI); 0 cross-region PII path | `SignedAttestation` (12.4) | scheduled |
| D.1 Switch test | switch test (T-7) + T-6 | ISS D14, Chat D-C19, per-loop | core loop done in browser, no wall; 6 states reached | switch-test session transcript + automated chained E2E | scheduled + per-change (automated parts) |
| D.2 Editor round-trip | render(parse(md))===md (T-5/KN-4) | KD-2, ISS D10 | 100% round-trip, 0 regressions | corpus pass-rate report (100%) | per-change in CI |
| D.3 Measured contrast + latency | T-8 / §4 | contrast + latency drills | 0 token pairs < AA; keyboard < 100ms p99; board < 1s; no spinner < 1s | measured-contrast report (real tokens) + latency report + inline-colour lint | lint/contrast per-change; latency scheduled |
| D.4 States + overlay + mobile | T-8 / 8b.1/8b.4 | overlay/state/responsive E2E | 6 states/screen; 0 clipped/mis-layered/focus-leak overlays; 0 off-screen popovers; 0 hover-only-unreachable | state-matrix + overlay-conformance + responsive/touch reports | E2E per-change; driven pass scheduled |

---

## F. Cross-references
- [`external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
  — the philosophy this doc operationalises (prove-it; quantified gates; observability-as-pass; the ratchet).
- [`external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) §1 (erasure vs
  immutability — the git-history half, A.5), §5.1 (untrusted code execution — B.1).
- [`02b-doctrine-integration/integration-directives.md`](../../02b-doctrine-integration/integration-directives.md)
  — Phase-5 block (T-1..T-9), ID-1..3, GD-1..3, CI-1, GIT-1, E-5 lints, L-1..4.
- [`02-holistic-architecture/architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md)
  — ADR-11/12 (residency + PersonalDataHolder spine), ADR-16/17/18/20 (human-lane, fail-static,
  restore-verify, one sandbox).
- [`02-holistic-architecture/design-language.md`](../../02-holistic-architecture/design-language.md) §4
  (a11y baseline) + §8b (the day-one testable UX mandates: 8b.1 overlays, 8b.2 round-trip, 8b.3 contrast,
  8b.4 mobile, 8b.7 switch test).
- [`contract-index.md`](../contract-index.md) + [`00-reconciliation-decisions.md`](../00-reconciliation-decisions.md)
  — the frozen contracts under test (4.3 OQ-E push-down, 5.9 X-1 fork-trust, 10.9 X-7 erasure posture, 12.x
  residency, 13.1 `myelin-content` round-trip).
- [`03-shared-systems-architecture/drills-and-open-questions.md`](../../03-shared-systems-architecture/drills-and-open-questions.md)
  — Part A: the F1–F9 family taxonomy + per-system owed-drill IDs this doc's gates map onto.
- Per-subsystem inventories: `04-subsystem-architectures/<slug>/architecture/07-drills-and-open-questions.md`
  (Git D-1..D-10, CI T-1/D-2..D-10/R-3, Issues D2..D14, Knowledge KD-2..KD-12, Chat D-C3..D-C19).
- **Companion strategy parts (this folder):** `00` (thesis + harness + scorecard + ratchet), `01`
  (resilience/data-integrity drills F3–F9), `02` (correctness/determinism + chained-mutation E2E).
