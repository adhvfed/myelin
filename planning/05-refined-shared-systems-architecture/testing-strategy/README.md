# Phase 5-C — Testing Strategy: Index & Executive Summary

> Phase: `05-refined-shared-systems-architecture/testing-strategy`. **The system-wide testing strategy index**
> (VISION §5: "specifies a testing strategy for the system as a whole and in parts"). Canonical brief:
> [`VISION.md`](../../../VISION.md) (never contradicted). THE philosophy source:
> [`external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (prove-it-or-it-isn't-real · quantified thresholds · observability-as-pass-condition · the failure-injection
> harness · the ratchet / committed gates · name-your-floors · code-wins-over-docs · drive-the-real-UI +
> chained-mutation E2E · order-by-non-negotiability). Hard problems:
> [`external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md). Binding directives:
> [`integration-directives.md`](../../02b-doctrine-integration/integration-directives.md) (Phase-5 T-1..T-9,
> Phase-8 E-1..E-9, Phase-6 R-1..R-6, the named lints E-5). Spine:
> [`architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md) (ADR-16 backpressure/
> human-lane, ADR-17 fail-static, ADR-18 backup/restore-verify, ADR-19 four-primitive, ADR-20 one sandbox).
> Frozen build-to surface: [`../contract-index.md`](../contract-index.md) +
> [`../00-reconciliation-decisions.md`](../00-reconciliation-decisions.md). Design-QA gates:
> [`../../02-holistic-architecture/design-language.md`](../../02-holistic-architecture/design-language.md) §8b.
> Date: 2026-06-19.
>
> **What this is.** The one-page front door to the testing strategy: it frames the strategy, indexes the four
> facet docs, presents the **consolidated master drill catalogue** as a single table, fixes the **definition of
> done** for a capability, and states the **Phase-6 sequencing handoff** (which gates must come early). Plain
> text identifiers throughout (no backticks-as-emphasis). Markdown only; no commits.

---

## 1. The strategy in one frame — whole + parts, prove-it, the ratchet

Myelin is an EU-sovereign, GDPR-by-construction, world-scale, agent-native platform: five subsystems (Git
hosting, CI, Issues, Knowledge, Chat) on one refined shared layer (one identity model, one event bus, one
agent fabric, one reference graph, one storage/residency/GDPR spine). The testing strategy must let us test
the system **as a whole** and **in parts**, and it rests on three load-bearing ideas.

**Whole + parts.** The system is tested at eight levels (a pyramid for a Rust-on-Postgres workspace): static
lints → unit → property → mutation → contract/seam → per-service integration → cross-subsystem E2E → load/chaos
drills. The **parts** are proven in isolation (each shared system and each subsystem testable alone, every
cross-system dependency replaced by a CDC-verified contract double); the **whole** is proven by chained-mutation
E2E scenarios that drive ≥3 subsystems over one session against a full cell, and by whole-cell failure-injection
drills. Neither replaces the other: a part passing in isolation does not prove the seam; a whole-system green
does not localise the part that owns a regression.

**Prove-it-or-it-isn't-real.** The atomic unit of proof is a **drill**, not a test: a forced fault + a workload
on the **real** code path + an assertion **read from production telemetry** (contract 1.8, the survival-signal
set). A passing unit test that never injected the fault proves the code, not the property. **Observability is
part of the pass condition:** a system that survives a drill but emits no signal that it survived has FAILED the
drill. Every gate resolves to a **single quantified threshold** you could read off a dashboard; a target you
cannot measure is not a gate. A capability is **proven** only when a drill emitted a **green artifact** —
otherwise it is **claimed**, and saying "claimed" is honest while saying "done" is the failure.

**The ratchet.** Gates are committed and mechanical — **an uncommitted gate is no gate**. Twelve architecture
lints make whole bug-classes impossible to compile; the restore-verify and the mandatory-core mutation gate
are wired into CI of the same status. Violations are **loud, never swallowed** (no `|| true`, no silent
filter). Thresholds live in one versioned file; a red gate stays red and becomes a "claimed, not proven"
scorecard row — it is **never edited green**, never inverted, never loosened to pass. Work is ordered by **what
kills you first**: silent data-loss and sandbox-escape gates outrank every feature, and **no later phase is
done over a red earlier gate**. And because Myelin **hosts itself** (one CI graph, dogfooded), the gates run on
the platform's own commits — the cheapest, most honest load generator we have.

---

## 2. The four facet docs (the strategy in full)

| Doc | Scope (one line) |
|---|---|
| [`00-philosophy-levels-and-gates.md`](./00-philosophy-levels-and-gates.md) | The keystone: the doctrine made concrete for Myelin, the eight-level test pyramid (incl. the mandatory cargo-mutants cores), and the committed-gate / ratchet model (the twelve lints, CI-vs-scheduled split, the gate invariant, loud-never-swallowed, the no-threshold-weakening gate, dogfooding). |
| [`01-whole-system-e2e-and-drill-catalogue.md`](./01-whole-system-e2e-and-drill-catalogue.md) | Testing AS A WHOLE: the four cross-subsystem chained-mutation E2E scenarios (PR context pane, CI-fail→triage-agent→fix-PR, spec-to-ship traceability, DSAR fan-out), the failure-injection harness (1×/10×/30× load gen, scoped-reversible dependency-break, telemetry assertions), and **the consolidated master drill catalogue**. |
| [`02-parts-contracts-and-mock-agents.md`](./02-parts-contracts-and-mock-agents.md) | Testing IN PARTS: the per-shared-system and per-subsystem isolation suites, the consumer-driven contract (CDC) suite per frozen seam (CheckStatus, list_objects Filter, myelin-content round-trip, the #sub tombstone ladder, ToolDef defaults), and the mock-agent determinism layer (scripted-queue → byte-identical effects, skeleton zero-spend, no-host-exec, plan-then-apply intersection). |
| [`03-gdpr-security-residency-and-ux-qa.md`](./03-gdpr-security-residency-and-ux-qa.md) | The compliance + security + UX gate set: GDPR/erasure (DSR fan-out completeness, crypto-shred, restriction, post-restore re-erasure, the git-history floor), security (the sandbox-escape gate, authz leak, cross-tenant IDOR, poisoned-pipeline, secrets, the ratchet lints), residency/EU-sovereignty, and UX/design QA (switch test, render(parse(md))===md, measured contrast, latency, overlay/state/mobile). |

---

## 3. The consolidated MASTER DRILL CATALOGUE

The master merge of **Phase-3 Part A** (the shared-system drills, 9 families F1–F9 + the non-family drills) and
the **five subsystems' `architecture/07`** drills, plus the additional gates the GDPR/security/UX facet (doc 03)
names. Each drill resolves to a **quantified threshold**, the **green artifact** (the named telemetry signal /
report that must read green — a drill with no green artifact is a claim), an **owner**, and **CI-vs-scheduled**
placement (CI = cheap, every change; SCHED = expensive, nightly/weekly/milestone; **GATE** = the single hard
go/no-go). Subsystem rows that are instances of a shared family name the family so Phase 5 runs the family across
owners with one harness and one scorecard column.

**Count: 174 catalogue drills** (103 shared-system + 71 subsystem) **+ 4 whole-system E2E scenarios = 178 proof
obligations.** Family thresholds are Q32 defaults-to-beat; Phase 6 measures and sets the binding numbers.

### 3.1 The nine reusable families (the recurring prove-its)

| Family | Property | Quantified gate (v1 default-to-beat) |
|---|---|---|
| F1 — Zero-escape / no-leak | A viewer never finds/reads what they can't access. | 0 leaked docs/edges/backlinks/notifications/results, **0 count/IDF/ranking/RAG leak**, incl. under zookie staleness. |
| F2 — Cross-tenant IDOR | No cross-tenant/cross-cell read via path-tenant spoof. | 0 cross-tenant rows/edges/results/bytes; `tenant-predicate` lint catches a tenant-less query at compile. |
| F3 — Restore + cross-seam integrity | Rebuild from backups lands at one consistent point. | 0 loss; OLTP↔blob↔index↔offset mutually consistent; post-restore re-erasure runs. |
| F4 — Reindex-from-cold parity | A derived store rebuilds to match live, via the live consumer path only. | cold == live (docs/ACL/ranking/edges/vectors/inbox); no bespoke recovery reader. |
| F5 — Zero-loss-across-reconnect / outbox no-ghost | No event lost/ghosted across a broker drop or producer/worker crash. | 0 lost, 0 ghost, 0 duplicate effect. |
| F6 — 30× surge + protected human lane | A human request survives a machine-speed surge; other tenants unaffected. | human-lane latency within budget; agent lane sheds (429 + Retry-After); cross-tenant impact = 0. |
| F7 — Id-hiccup / fail-static | A transient Id/CP hiccup degrades, doesn't cascade; a revoked actor still denied. | authenticated traffic survives within W; staleness ≤ static_max ≤ revocation SLA; zookie reads bypass cache. |
| F8 — Disabled-user → zero-access-in-N-min | A disabled/revoked principal loses all access within N min. | every surface denies within **N = 5 min**; token TTL + denylist + cache expiry ≤ W; stale re-grant = 0. |
| F9 — Loop/runaway adversarial | An agent→agent loop/storm is structurally halted. | loop halts ≤ depth ceiling (agent 12 / traversal 16); tripwire trips the per-tenant breaker; runaway stops at the wallet. |

**Named v1 thresholds (Q32 defaults-to-beat, measured & set in Phase 6):** N = 5 min revocation; surge = 30×;
fail-static W = 5 min (DPO-ratified, L-1); RPO ≤ 5 min; RTO ≤ 1h/tenant, ≤ 4h/cell; depth ceilings 12 (agent) /
16 (traversal); `order_key` rebalance at 48 chars; projection-feeder promotion at > 5% of view executions;
sandbox escapes = **0** (the hard go/no-go); cross-tenant / unauthorized-visibility leak = **0**.

### 3.2 Shared-system drills (Phase-3 Part A — 103 rows)

| Drill | Owner | Fam | Quantified threshold | Green artifact | Freq |
|---|---|---|---|---|---|
| SUB-D1 | SUB | F5 | Kill service between commit & publish → exactly-once-in-effect (0 ghost, 0 lost). | outbox-depth drains; dedup ledger | CI |
| SUB-D2 | SUB | F5 | Drop broker mid-stream → 0 lost across reconnect; slow subject doesn't block others. | consumer-lag; no HoL stall | CI |
| SUB-D3 | SUB | F6 | 30× agent surge one tenant → human lane holds, agent sheds, others unaffected. | shed-counts/lane; per-tenant RED | SCHED |
| SUB-D4 | SUB | F7 | Id-hiccup → already-authenticated survives within W; revoked denied when window closes. | fail-static fresh/stale/closed | CI |
| SUB-D5 | SUB | retry-storm | Trip a downstream breaker → fail fast + honour Retry-After; no amplification. | breaker-state; Retry-After issuance | CI |
| SUB-D6 | SUB+STOR | F3 | Rebuild from backups → no loss; OLTP↔blob↔index↔offsets one consistent point. | restore-verify-pass | SCHED |
| SUB-D7 | SUB | F2 | Cross-tenant read via path≠token tenant → 0; lint catches tenant-less query at compile. | misroute-count 0; lint green | CI |
| SUB-D8 | SUB | F9 | Adversarial agent→agent loop → depth ceiling + tripwire + bounded pool halt it. | causal-depth histogram; tripwire | CI |
| SUB-D9 | SUB | liveness | Kill a critical dependency → instance not-ready + sheds; no restart-storm. | readiness flips; no liveness churn | CI |
| SUB-D10 | SUB | migration | expand→backfill→contract under load → no blocking lock beyond budget; 0 downtime. | lock-wait p99; 0 errored writes | SCHED |
| ID-D1 | ID | F8 | SCIM-disable → every surface denies within **N=5 min**; cache+token+denylist ≤ W. | deny-latency histogram | SCHED |
| ID-D2 | ID | F7 | Break Id dependency → authenticated survives on coarse cache; just-revoked still denied. | fail-static ratios | CI |
| ID-D3 | ID | F2 | Cross-tenant check/list/read via path spoof → 0 cross-tenant tuples readable. | cross-tenant count 0 | CI |
| ID-D4 | ID | F1 | Confidential issue/page/channel absent from any list_objects/search/refs for an unauthorized viewer. | zero-escape counter | CI |
| ID-D5 | ID | F9 | Agent confined to agent.policy ∩ delegation ∩ tenant.policy, incl. via a delegator who lost the right. | denial counter; intersection proof | CI |
| ID-D6 | ID | F8 | Kill a run mid-flight → per-run token revoked + auto-expires within run-life ≤ W. | token-revocation lag | CI |
| ID-D7 | ID | F8 | Revoke then re-read with post-revoke zookie → no stale allow ("new enemy"). | zookie-watermark honoured | CI |
| ID-D8 | ID | F3 | Restore to a consistent point → no resurrected grants past an erasure; re-erasure runs. | re-erasure receipt | SCHED |
| ID-D9 | ID | F6 | 30× agent surge on the authz hot path → human lane holds, agent sheds. | shed-counts; authz p99 | SCHED |
| BUS-D1 | BUS | F5 | Kill consumer + sever broker during publish → 0 lost, 0 duplicate effects on reconnect. | lost/dup = 0; lag drains | CI |
| BUS-D2 | BUS | F5/HoL | Flood unhandled types at a `*`-subscribed consumer → no stall; lag alarm fires. | lag alarm; no stall | CI |
| BUS-D3 | BUS | replay | Replay a correlation_id tree → deterministic, idempotent, causality preserved. | replay-equals-original hash | CI |
| BUS-D4 | BUS | F5 | Crash producer between state-commit and publish → event still delivered, never without state. | outbox emit-iff-committed | CI |
| BUS-D5 | BUS | F4 | Wipe a derived store, reindex(scope) → rebuilt store byte-matches live. | reindex-parity hash | SCHED |
| BUS-D6 | BUS | F9 | Self-triggering automation → depth ceiling + shared-root tripwire trip the per-tenant breaker. | tripwire firing; breaker trip | CI |
| BUS-D7 | BUS | F6 | 30× agent publish surge one tenant → human/control lane holds, agent sheds. | shed-counts/lane | SCHED |
| BUS-D8 | BUS | erasure | Erase a subject → inline-PII events unrecoverable; `*.erased` tombstones emitted; consumers degrade. | erase-receipt; tombstone count | SCHED |
| BUS-D9 | BUS | per-ref order | Burst force-pushes to one hot ref → events in push order per ref, parallel across refs, at QPS. | per-aggregate order preserved | SCHED |
| REF-D1 | REF | F1 | Confidential artifact referencing a public one absent from backlinks/traverse for an unauthorized viewer. | zero-escape counter | CI |
| REF-D2 | REF | F2 | Cross-tenant edge read via path spoof / crafted URN → 0 cross-tenant edge. | cross-tenant edge 0 | CI |
| REF-D3 | REF | F6 | "Referenced-by-50,000" under filtered reads → paginated p99 within budget; R4 post-promotion. | fan-out read p99 | SCHED |
| REF-D4 | REF | F4 | Wipe edge index, reindex → byte-matches live; a TE-7 drift reconverges (typed wins). | reindex-parity hash | SCHED |
| REF-D5 | REF | erasure | Erase a subject + referenced artifact → references tombstone, 0 recoverable PII, no 500 on resolve. | erase-receipt; 0 resolve-error | SCHED |
| REF-D6 | REF | F8 | Revoke access, re-read backlinks with post-revoke zookie → no stale allow. | zookie-bypass honoured | CI |
| REF-D7 | REF | F5 | Crash producer between content/relation commit and publish → edge event delivered, never an edge without content. | outbox emit-iff-committed | CI |
| REF-D8 | REF | traversal bound | Cycle + 1000-deep chain → CTE terminates (visited-set + depth 16), cycle surfaced, timeout respected. | depth-bound honoured | CI |
| REF-D9 | REF | sub-tombstone | Delete an embedded block / PR comment → embed degrades to partial/relocated, not 404; 0 dangling. | tombstone ladder state dist. | CI |
| REF-D10 | REF | F6 | 30× agent ref-creation + backlink-read surge → human read lane holds, agent sheds. | shed-counts; read p99 | SCHED |
| SRCH-D1 | SRCH | F1 | Confidential/private artifact never in any query/semantic result (incl. counts, IDF, "more results", RAG). | zero-escape counter | CI |
| SRCH-D2 | SRCH | F1/F8 | Revoke, re-search with post-revoke zookie → excluded; default-consistency search excludes within W. | exclusion within W | CI |
| SRCH-D3 | SRCH | F2 | Search scoped to another tenant via path spoof → 0 cross-tenant results. | cross-tenant results 0 | CI |
| SRCH-D4 | SRCH | erasure | Erase a subject → every doc/field/vector/embedding purged (not hidden), unrecoverable; 0 orphan embedding. | embedding-purge receipt | SCHED |
| SRCH-D5 | SRCH | F4 | Wipe index, reindex(scope) → rebuilt index matches live (docs, ACL, ranking, vectors), live consumer path. | reindex-parity hash | SCHED |
| SRCH-D6 | SRCH | F6 | 30× agent/CI query surge → human search lane holds, agent sheds, others unaffected. | shed-counts; search p99 | SCHED |
| SRCH-D7 | SRCH | freshness | Under load, event→searchable p99 within seconds-grade budget; index-lag alarms before user-visible staleness. | index-lag alarm; freshness p99 | SCHED |
| SRCH-D8 | SRCH | filtered-ANN | Selective ACL/structured filter → k nearest visible neighbours, recall@k ≥ threshold; no leak. | recall@k; zero-escape | SCHED |
| SRCH-D9 | SRCH | F3 | Restore index with OLTP/blob/offsets → no resurrected erased docs; no row↔doc↔vector mismatch. | restore-verify; re-erasure | SCHED |
| SRCH-D10 | SRCH+STOR+AG | HYOK | Mark a content class HYOK → Search/Agents skip it; 0 HYOK plaintext in any derived store. | 0 HYOK plaintext indexed | SCHED |
| NOTIF-D1 | NOTIF | ranking | Replay a mixed week → every critical/direct ranks above every fyi; first-important latency within budget. | important-buried-rate 0 | SCHED |
| NOTIF-D2 | NOTIF | storm-control | 1000 near-identical CI failures + a 30-comment burst → bounded items; self-notifications suppressed. | dedup-collapse-ratio; 0 self | CI |
| NOTIF-D3 | NOTIF | F4 | Wipe inbox_item, reindex(notif) → rebuilt inbox matches live (items + read-state from source events). | reindex-parity hash | SCHED |
| NOTIF-D4 | NOTIF | F1 | Notify on a confidential subject to a viewer lacking access → humanised tombstone; title never appears. | 0 title/PII leak | CI |
| NOTIF-D5 | NOTIF | F6 | 30× agent notification surge → human inbox-read lane holds, agent sheds, delivery-adapter bulkhead bounds load. | shed-counts; delivery-success | SCHED |
| NOTIF-D6 | NOTIF | erasure | Erase a user → every inbox item humanises to `[erased user]`; 0 recoverable PII; off-cell payload crypto-shredded. | erase-receipt; 0 recoverable | SCHED |
| NOTIF-D7 | NOTIF | F5 | Kill Notif mid-ack_window → durable workflow resumes, pages next step exactly once; an ack stops the chain. | exactly-once page; ack-halt | CI |
| NOTIF-D8 | NOTIF | quiet-hours | Set DND; fire a critical escalation → it pierces quiet-hours; a watching item is suppressed. | critical pierces; non-crit suppressed | CI |
| NOTIF-D9 | NOTIF | delivery idempotency | Crash between provider-ack and ledger-write, retry → exactly-one delivery per (item, channel). | 1 effective delivery | CI |
| NOTIF-D10 | NOTIF | F5/HoL | Inject a slow/poison Signal type → whitelisted-template router doesn't stall, terminates poison, lag-alarm fires. | no stall; lag alarm | CI |
| AG-D1 | AG | plan-then-apply | A tool tries to write outside EffectApi → structurally impossible (no-host-exec + no-cross-db lints green). | lints green | CI |
| AG-D2 | AG | F9 | Effect outside the ∩ → Denied returns to the loop, no privileged fallback fires. | denial counter; 0 fallback | CI |
| AG-D3 | AG | F9 | Agent attempts an effect policy allows but delegation/tenant forbids → confined to the intersection. | intersection proof | CI |
| **AG-D4** | **AG (CI owns)** | **escape** | **compute tool attempts a kernel escape on a real kernel → ZERO escapes. The single hard gate before any untrusted code runs.** | **green escape attestation** | **GATE** |
| AG-D5 | AG | HITL | Gated tool → withheld (does NOT mutate); card shows action+risk+cost; approval resumes + applies once; rejection halts. | 0 mutation pre-approval; 1 apply | CI |
| AG-D6 | AG | F6/F9 | 30× agent dispatch surge → human lane holds, agent sheds, reserve/settle refuses over-budget runs. | shed-counts; reserve refusals | SCHED |
| AG-D7 | AG | F9 | Adversarial agent→agent self-trigger → depth ceiling (12) + tripwire + bounded pool halt; per-tenant breaker trips. | causal-depth; tripwire; breaker | CI |
| AG-D8 | AG | F8 | Kill a run mid-flight → token revoked on teardown + auto-expires ≤ W; 0 shared token leaked into the child env. | token-revocation lag; 0 leak | CI |
| AG-D9 | AG | determinism | Run a scripted mock twice → identical proposed-effect sequences; cargo-mutants over event→trigger→effect→event ≥ threshold. | identical effect seq; mutation score | CI |
| AG-D10 | AG | erasure | Erase a subject → run trace + agent memory/embeddings crypto-shredded/purged; attribution → opaque pseudonym. | erase-receipt; 0 recoverable | SCHED |
| AG-D11 | AG | F9 | Runaway loop vs an exhausted wallet → reserve refuses new runs (never interrupts in-flight); loop stops at the wallet. | reserve refusals; 0 interrupt | CI |
| FLOW-D1 | FLOW | F5 | Kill a worker at activity 5/10 → another re-leases, replays, resumes at step 6 with 0 re-executed side effects, exactly-once. | replay-rate; 0 double-effect | CI |
| FLOW-D2 | FLOW | determinism | Replay against a divergent/wrong-version definition → divergence guard halts as nondeterministic + dead-letters. | nondeterministic-halt count | CI |
| FLOW-D3 | FLOW | timer scale | Arm 1M+ timers + a burst due in one minute → due fire within tick budget; a crash re-fires unfired. 0 lost/0 double-fire. | timer-wheel lag; 0 lost/dup | SCHED |
| FLOW-D4 | FLOW | multi-day HITL | A gated workflow waits across a worker restart + a deploy; deliver approval days later (double-click) → resumes, consumes once. | 1 consume; withhold = 0 mutation | CI |
| FLOW-D5 | FLOW | F5 | Crash between journaling a DB write and emitting its event → journal + outbox committed together; 0 ghost, 0 lost. | co-commit proof | CI |
| FLOW-D6 | FLOW | F9 | Runaway agent loop vs a depleting wallet → new spend-bearing activity refused at reserve; in-flight never interrupted. | reserve refusals; 0 interrupt | CI |
| FLOW-D7 | FLOW | F9 | Adversarial workflow→event→workflow loop → depth ceiling + bus tripwire + bounded pool stop it (never forks). | causal-depth; 0 fork | CI |
| FLOW-D8 | FLOW | F6 | 30× surge of agent-initiated workflows → human-initiated lane holds, agent sheds, others unaffected. | shed-counts/lane | SCHED |
| FLOW-D9 | FLOW | erasure | Erase a subject with inline-PII history → keys destroyed (unrecoverable incl. backups), references tombstoned. | crypto-shred-lag; 0 recoverable | SCHED |
| FLOW-D10 | FLOW | F3 | Restore myelin-flow PG to a consistent point → in-flight runs resume; store↔outbox↔rows at one consistent point. | restore-verify; consistent point | SCHED |
| STOR-D1 | STOR | F3 | Rebuild from backups to offset T → 0 loss (checksum parity); OLTP↔blob↔index↔offset one consistent point. **The headline durability gate.** | restore-verify-pass | SCHED |
| STOR-D2 | STOR | RPO/RTO | Kill a cell; restore → **RPO ≤ 5 min** (WAL tail); **RTO ≤ 1h/tenant, ≤ 4h/cell**. | backup-RPO-seconds; restore-time | SCHED |
| STOR-D3 | STOR+GA | F3 | Erase a subject; restore an older backup → still erased (post-restore re-erasure ran). 0 resurrected. | re-erasure receipt | SCHED |
| STOR-D4 | STOR | crypto-shred reach | Erase a subject; attempt recovery from backups → per-subject ciphertext unrecoverable. 0 recoverable PII in any backup. | crypto-shred-lag; 0 recoverable | SCHED |
| STOR-D5 | STOR | residency | Read/replicate a tenant's data outside its region → impossible (region in partition key). 0 cross-region PII egress. | residency-attestation; 0 egress | SCHED |
| STOR-D6 | STOR | KMS degrade | Transient KMS outage → resolved-DEK reads survive (bounded TTL); hard-down → not-ready+shed (not fail-open). | fail-static; 0 fail-open | CI |
| STOR-D7 | STOR | blob integrity | Corrupt an object → re-hash-on-read detects it; recover from replica/backup. 0 silent serve. | integrity-check; 0 silent serve | CI |
| STOR-D8 | STOR | migration | expand→backfill→contract under load → no blocking lock beyond budget; 0 downtime. | lock-wait p99; 0 downtime | SCHED |
| GA-D1 | GA | erasure-fanout | Erase a subject seeded into all H1–H18 → fan-out hit every holder; post-erase locate returns 0 recoverable PII. **0 holders missed.** | erasure-fanout-coverage = 100% | SCHED |
| GA-D2 | GA+SRCH | erasure-search | The subject's docs and embeddings purged+reindexed out (not hidden). 0 hits, 0 embedding re-identification. | embedding-purge receipt | SCHED |
| GA-D3 | GA | audit tamper | Retroactively edit/delete an audit entry → chain breaks + STH consistency-proof fails + external witness mismatches. Tamper detected 100%. | tamper-detection proof | SCHED |
| GA-D4 | GA | DSR deadline | Open a DSR → durable timer fires a warning before the 1-month deadline; certificate seals on completion. 0 silent misses. | DSR-timer fire; sealed cert | SCHED |
| GA-D5 | GA | data-map drift | Add an untagged personal-data field → no-untagged-personal-data lint fails the build. Build red on untagged PII. | lint red on untagged PII | CI |
| GA-D6 | GA | legal-hold | Set a hold over a subject; submit an erase → erasure deferred-by-hold, resumes on hold-lift. 0 held-scope deletions. | hold-defer receipt | SCHED |
| GA-D7 | GA+BUS/AG | restriction | Restrict a subject → no indexing/agent-use/analytics/notification while storage retained; reversible. 0 processing. | restriction-suppression proof | CI |
| GA-D8 | GA+CP | F2 (**FLOOR**) | Multi-cell erasure: fan-out iterates all member_cells ∪ home_cell; complete receipt set. 0 cells missed. | per-cell receipt set | SCHED |
| CP-D1 | CP | PII-free | Data-map over the control-plane schema → 0 is_personal columns; writing a name/email → build fails. | lint green; 0 PII columns | CI |
| CP-D2 | CP | F2 | Request to a cell for a tenant_id it doesn't host → misroute rejection, 0 cross-tenant/cross-cell read, audited. | misroute-count; audit entry | CI |
| CP-D3 | CP | residency | Write where row.region ≠ cell.region → residency-pin rejects; residency_verify attestation passes. | residency-attestation | CI |
| CP-D4 | CP | F7 | Hard-down the control plane → already-placed tenants keep serving; only signup/provisioning degrades. | serving-uptime; degrade scope | SCHED |
| CP-D5 | CP | bulkhead | Fatal fault / 30× surge in one cell → other cells unaffected; noisy tenant contained to its cell. | cross-cell impact 0 | SCHED |
| CP-D6 | CP | F3 | Provision a fresh cell → passes restore-verify + readiness before accepting any tenant. | restore-verify + readiness gate | SCHED |
| CP-D7 | CP | F3 (**FLOOR**) | Migrate a tenant cell→cell (same region) → 0 loss across-seam, lands in-region, source crypto-shredded. | migration receipt; 0 loss | SCHED |
| CP-D8 | CP | F1 (**FLOOR**) | Cross-cell ref → bridge carries only subject/type/correlation_id; target resolves per-viewer; unauthorized → tombstone. | PII-free bridge proof | SCHED |

### 3.3 Subsystem drills (the five `architecture/07` sets — 71 rows)

| Drill | Owner | Fam | Quantified threshold | Green artifact | Freq |
|---|---|---|---|---|---|
| GIT-D1 | Git | F5/per-ref | Burst force-pushes to one hot ref (1×/10×/30×) → git.ref.updated in push order per ref; refs parallel; 0 lost/ghost. | per-aggregate order; outbox depth | SCHED |
| GIT-D2 | Git | erasure | Erase a subject who authored commits/PRs/comments + LFS → every holder hit; residual == the ONE platform posture (10.9). | DSR receipt set; ledger entry | SCHED |
| GIT-D3 | Git | F4 | Wipe Search index + Refs edges + check_status projection; reindex/replay → cold rebuild byte-matches live; no cross-DB read. | reindex-parity hash | SCHED |
| GIT-D4 | Git | ceiling | Grow a synthetic monorepo until partial-clone/sparse/bitmaps degrade → documented v1 ceiling; clone/fetch p99 held below it. | ceiling numbers; clone p99 | SCHED |
| GIT-D5 | Git | linearizable | Concurrent merges + force-push + replica failover + node recovery mid-merge → linearizable on ref CAS; no split-brain; 0 lost merge. | 0 conflicting tips; reconcile log | SCHED |
| GIT-D6 | Git | F6 | 30× agent/CI clone surge on a hot repo → human fetch p99 held; agent/CI sheds (429 + Retry-After); 0 cross-tenant starvation. | shed-counts; fetch p99; CDN hit | SCHED |
| GIT-D7 | Git | sub-anchor | Force-push/rebase a PR with open inline threads → anchors resolve LIVE/MOVED/OUTDATED/GONE correctly; 0 mis-anchored. | per-anchor state distribution | CI |
| GIT-D8 | Git | F2 | Cross-tenant repo access via token tenant ≠ URL-path tenant → tenant from token; 0 cross-tenant read; rejected at front door. | authz deny; lint green | CI |
| GIT-D9 | Git | F5 | Crash serving tier mid-push → git.ref.updated emitted iff the ref move committed; no ghost/lost; quarantine objects discarded on abort. | outbox emit-iff-committed | CI |
| GIT-D10 | Git+CI | X-1 check seam | (a) out-of-order/dup ci.check.updated → run_attempt-monotonic supersession; (b) fork PR self-greens → neutral for gating; (c) endorse → green; (d) doubly-delivered ci.result → workflow wakes exactly once; 0 double-merge. | 1 current row/key; merge-count == 1 | CI |
| GIT-D11 | Git | F1 | Viewer with partial visibility lists a 100k-PR tenant → SetExpr JOIN returns only visible rows (0 leak), one query (no N+1); revoke reflected (zookie). | 0 leak; 1 SQL query; revoke latency | SCHED |
| CI-T1 | CI | **escape** | **= AG-D4.** Real-kernel adversarial corpus → **ZERO escapes** or CI is no-go for untrusted code. Re-run on every backend/image/kernel change. | **green escape attestation** | **GATE** |
| CI-D1 | CI | F5 | Kill the runner mid-job; kill the control plane mid-run → run resumes (idempotent re-dispatch); effectively-once; 0 lost runs/double-deploys. | replay-rate; 0 double-effect | CI |
| CI-D2 | CI | F6 | 30× CI surge one tenant → interactive lane holds; batch sheds (429 + Retry-After); reserve/settle refuses over-budget; killed-runner jobs re-queue, 0 orphans. | shed-counts; reaper; lease TTL | SCHED |
| CI-D3 | CI | erasure | erase(subject) fans to CI → PII in logs/artifacts/caches/run-state destroyed (incl. backups); structure survives; 0 dangling leak. | DSR receipt; 0 recoverable | SCHED |
| CI-D4 | CI | supply-chain | Floating tag / tampered-unsigned component → digest-pin + sign-verify fail closed; verification_failed emitted. 0 un-pinned/unsigned executions. | 0 un-pinned runs; audit event | CI |
| CI-D5 | CI | reserve/settle | Exhaust the wallet, start a CI run + an agent compute job; replay across a pricing change → refuse-start (never interrupt in-flight); 0 starts past exhaustion. | 0 over-exhaustion starts; cost parity | CI |
| CI-D6 | CI | cache-poison | Adversarial UntrustedFork run writes the default-branch cache scope → trust-tier/branch-scoped namespace holds. 0 trusted-cache writes from a fork. | 0 fork→trusted writes | CI |
| CI-D7 | CI | F1/secrets | Adversarial fork run reads protected secrets → read & !is_untrusted_fork ABAC holds. 0 secret reads by a fork-tier run. | 0 fork secret reads | CI |
| CI-R3 | CI | residency | An EU-resident tenant's run → claimed only by an in-region runner; logs/artifacts/caches never leave region; residency_verify attests. | residency-attestation; lint green | SCHED |
| CI-D8 | CI | X-1 (= GIT-D10) | push→check→green→merge; out-of-order/re-delivered; fork success neutral; re-run → projection holds correct current row; merge-queue wakes idempotently; 0 spurious unblocks. | correct row; 0 double-merge | CI |
| CI-D9 | CI | determinism | The ci.pipeline workflow body → no clock/RNG/IO outside WfCtx; flow-determinism lint passes; replay bit-identical. | lint green; bit-identical replay | CI |
| CI-D10 | CI | F2 | A compromised self-hosted runner → scoped job token bounds it to its own tenant's SelfHosted jobs; 0 cross-tenant job/secret reads. | 0 cross-tenant reads | SCHED |
| CI-D11 | CI | F5/OQ-J | Drop the live-tail mid-run, reconnect with last_seq → firehose backfills (last_seq, now]; 0 log lines lost; over-window → resync_required; scope bounded. | 0 lost lines; resync fallback | CI |
| ISS-D1 | Issues | co-equal view | Edit an issue's date/scope on the board → roadmap reflects the same row, 0 drift, and vice-versa (same ViewSpec/table). | same-row-id assertion | CI |
| ISS-D2 | Issues | flex-field latency | 50+ custom fields, 1M+ issues board query → under the **<1s keyboard budget** with the SetExpr JOIN; planner never emits a full JSONB scan. | query p99 < 1s; no full scan | SCHED |
| ISS-D3 | Issues | F1 | Cross-tenant + confidential-issue IDOR → not in any board/JOIN/search/backlink/context-pane, incl. under zookie staleness. 0 leak. | zero-escape counter | CI |
| ISS-D4 | Issues | human-key | Create-storm (import + incident burst on one hot prefix) → no duplicate key, monotonic per prefix, per-prefix isolation. | 0 dup key; monotonic | SCHED |
| ISS-D5 | Issues | reorder | N humans + an agent re-ranking the same backlog region → 0 silent clobber, bounded re-base churn, converges with order_key. | 0 clobber; converged order | CI |
| ISS-D6 | Issues | SLA durability | Breach fires after a restart; business-calendar corpus (DST, holiday, pause/resume) → fire_at matches wall-clock to the second; breach starts escalation. | fire-at accuracy; chain start | CI |
| ISS-D7 | Issues | trigger | Arm "remind me when unblocked"; resolve last blocker across a restart → fires exactly once; after stale_after, stale nudge fires once. | 1 fire; stale-once | CI |
| ISS-D8 | Issues | F4/rollup | Rollup freshness under a 10k-issue import (debounce); replay rebuilds rollup + Refs edge projection drift-free vs live. | reindex-parity; debounce bound | SCHED |
| ISS-D9 | Issues | import | export→import→export round-trips (ADF lossy-map nodes named); a large import resumes after a crash, no dup creates; doesn't starve other tenants. | round-trip oracle; 0 dup; lane p99 | SCHED |
| ISS-D10 | Issues | editor round-trip | render(parse(md)) === md over a corpus for issue bodies + comments (identical WASM parser). | 100% round-trip | CI |
| ISS-D11 | Issues | erasure | Erase a subject → PII gone from issue row, change-log, comments, attachments, OLAP, Search (incl. embeddings), Refs; post-restore re-erasure; residual is the [OPEN — LEGAL] limit. | holder receipts; re-erasure | SCHED |
| ISS-D12 | Issues | guard | "Can't mark Done while CI red" (reads CheckStatus + trust posture) + "can't close while blocked_by open" → blocked with reason; an agent at a governed transition is HITL-gated (withheld). | transition blocked; 0 pre-approval mutation | CI |
| ISS-D13 | Issues | F5/OQ-J | A board at scope=board:<id> drops mid-edit-storm → resume backfill then live loses zero ops; over-window → resync_required → snapshot. | 0 ops lost; resync fallback | CI |
| ISS-D14 | Issues | switch-test/UI | Can a Jira/Linear user complete create→triage→plan→board→done without a manual? + measured contrast/latency on primary screens incl. all states. | switch-test pass; contrast/latency | SCHED |
| KN-D1 | Knowledge | F5/OQ-J (headline) | Kill a collab client mid-edit + sever during multi-author edit; on resume(scope=doc, last_seq) → **0 ops lost, 0 duplicate**. Re-run across the CAS→CRDT engine_promote boundary. | 0 lost/dup; resume-gap size | CI |
| KN-D2 | Knowledge | editor round-trip | render(parse(md)) === md over a markdown-subset corpus (3 structured nodes, nesting, code, IME/paste). **100% round-trip; 0 regressions.** | corpus pass rate 100% | CI |
| KN-D3 | Knowledge | CAS floor | Two clients edit the same block concurrently → the loser is rejected with current state (never silently overwritten); different blocks parallel. | 0 silent overwrites | CI |
| KN-D4 | Knowledge | erasure | Erase a subject → structured PII purged/pseudonymised, free-text crypto-shredded (unrecoverable in op-log/snapshots/backups), embeddings purged. 0 recoverable incl. vectors. | holder receipts; key-shred count | SCHED |
| KN-D5 | Knowledge | F1 | A confidential page / overridden sub-page / row-restricted db / field-hidden column never in any view/backlink/search/embed/RAG, incl. an aggregate COUNT. 0 leaked; 0 count-leak. | zero-escape counters | CI |
| KN-D6 | Knowledge | F4 | Wipe Knowledge's derived state; replay(scope) (block-granular snapshot) → rebuilt matches live; live consumer path only. | reindex-parity hash | SCHED |
| KN-D7 | Knowledge | F5 | Crash between the block/row commit and relay-publish → event delivered (outbox), never without the state change. 0 ghost, 0 lost. | outbox emit-iff-committed | CI |
| KN-D8 | Knowledge | F6 | An all-hands doc with thousands of concurrent readers/editors → per-doc op cap + read-fanout bound + active-editor lane hold within budget; LexoRank insert storm (0 reorder). | per-tenant in-flight; op fan-out | SCHED |
| KN-D9 | Knowledge | flex-DB latency | Filter/sort/group a large multi-tenant database (JSONB + projection + SetExpr conjoin) → read-time p99 within budget; measure the >5% facet-promotion trigger. | db-query p99; facet frequency | SCHED |
| KN-D10 | Knowledge | rollup latency | A rollup over a large related set, computed at read time (permission-filtered) → p99 within budget; measure when materialisation is needed. | rollup p99 | SCHED |
| KN-D11 | Knowledge | HITL | An agent edits a doc via EffectApi → attributed; a consequential edit (publish/confidential) is HITL-withheld until approval; double-click is one approval. 0 ungoverned/0 pre-approval/0 double-apply. | gate-state; idem-key dedup | CI |
| KN-D12 | Knowledge | erasure (trace) | Erase a subject → content-addressed agent traces crypto-shredded/purged; attribution falls back to the pseudonym. 0 recoverable PII. | trace holder receipts | SCHED |
| KN-D13 | Knowledge | F2 | Read a page/db/row across tenants via path-tenant spoofing → 0 cross-tenant read; tenant-predicate lint catches a tenant-less query at compile. | 0 cross-tenant; lint green | CI |
| CHAT-D1 | Chat | F5/OQ-J | Sever the gateway↔firehose mid-publish → resume(stream, scope, last_seq) recovers the gap (0 lost, 0 dup); over-window → resync_required → snapshot. | 0 lost/dup; resync fallback | CI |
| CHAT-D2 | Chat | per-conv order | Burst sends + edits to one hot channel from many gateways → per-conversation total order (ULID); resume gap-free; out-of-order client ops reconcile. | total order; gap-free | SCHED |
| CHAT-D3 | Chat | F6 | 30× agent message/connection surge one tenant → human latency in budget; agent lane sheds; others unaffected. **(TE-21 build-gate.)** | shed-counts; connection p99 | SCHED |
| CHAT-D4 | Chat | deploy herd | Roll the gateway fleet under a connection storm → bounded reconnect rate; resume completes for all; no message loss. **(TE-21 build-gate.)** | reconnect rate; 0 loss | SCHED |
| CHAT-D5 | Chat | F1 | Notify/unfurl a confidential artifact to a viewer lacking access → tombstone rendered, title never present (ladder step 1). | 0 title leak | CI |
| CHAT-D6 | Chat | erasure (unfurl) | Erase a third party rendered in a card → tombstone on next render, 0 recoverable PII (cache re-resolves live → erased). | 0 recoverable; live re-resolve | CI |
| CHAT-D7 | Chat | live-update | An artifact's ci.check.updated/*.updated → the shared per-ref cache busts; viewers get a live firehose update within budget. | cache-bust; update latency | CI |
| CHAT-D8 | Chat | erasure | Erase a person → bodies crypto-shred in hot+cold segments+backups; mentions → [erased user]; read-state/drafts/cache purged; cascades. 0 recoverable PII. | holder receipts; 0 recoverable | SCHED |
| CHAT-D9 | Chat | HITL bridge | Request approval, kill Chat + Workflow mid-wait, approve days later → the gated tool runs exactly once; double-click is one approval; deny withholds with no mutation. | 1 apply; 0 pre-approval mutation | CI |
| CHAT-D10 | Chat | batch HITL | A multi-effect card approved 2-of-3 → the 2 resume approved, the 1 withheld, each independent; no effect runs twice. | per-effect idempotency | CI |
| CHAT-D11 | Chat | F1 | Search as a non-member → 0 results from channels you're not in; search-requires-acl-filter lint fails any query reaching the index without the Filter. | 0 leak; lint green | CI |
| CHAT-D12 | Chat | cache-loss | Flush + drop Valkey mid-session → the PG record is authoritative; a marker is at-worst slightly stale; unread counts recompute correctly. | PG authoritative; counts correct | CI |
| CHAT-D13 | Chat | F5 | Crash between message persist and event emit → both committed or neither; message and chat.message.created atomic; no orphan/phantom. | co-commit proof | CI |
| CHAT-D14 | Chat | idempotent send | Retry a send with the same client_nonce → one message (UNIQUE(conv, client_nonce)). | 1 message | CI |
| CHAT-D15 | Chat | F4 | Wipe + replay(scope, since) → Search/Refs/Notif read-models rebuild from chat.*.snapshot; erased subjects emit tombstones. | reindex-parity hash | SCHED |
| CHAT-D16 | Chat | agent mock | Drive the streaming UX against the mock runtime → partials stream on the firehose; final replaces partial; a mid-stream reconnect resumes the final. | partial→final; 0 half-message | CI |
| CHAT-D17 | Chat | explicit-first | A casual @agent mention → notifies the inbox, does NOT spawn a costed run; only an explicit action dispatches; reserve/settle gates even the explicit run. | 0 auto-spawn; reserve gate | CI |
| CHAT-D18 | Chat | sub-anchor | Edit a message referenced by another artifact → the message-<id> anchor stays stable; delete it → the embed degrades to a Tombstone carrying the root. | anchor stability; tombstone | CI |
| CHAT-D19 | Chat | switch-test/UI | Drive the real Chat UI → a team could move without hitting a wall; measured-contrast tokens; latency budgets; flip-popovers against the real bottom-pinned composer. | switch-test; contrast/latency | SCHED |

### 3.4 The four whole-system E2E scenarios (scorecard rows)

| Scenario | Crosses | Quantified gate | Green artifact | Freq |
|---|---|---|---|---|
| **E2E-1 PR context pane** (UC-X-3) | Git, CI, Issues, Knowledge, Refs, Search, Id, Notif | every connected artifact resolves per-viewer; **0 leak** to the unauthorized viewer; live check-update within freshness budget; tombstone carries root. | pane-resolution trace + zero-leak = 0 + per-viewer diff | SCHED |
| **E2E-2 CI-fail → triage agent → issue → chat → fix-PR** (flagship) | CI, Agent, Workflow, Issues, Chat, Git, Id, Notif, Storage | 0 effect outside the ∩; 0 mutation before approval; exactly-once approval + merge across a kill; reserve/settle balanced. | deterministic run trace + HITL withhold→approve→apply ledger + reserve/settle parity + merge-count == 1 | SCHED |
| **E2E-3 Spec-to-ship traceability** | Knowledge, Issues, Git, CI, Chat, Refs, Search, GDPR/Audit, Id | complete lineage per-viewer; **cold-reindex == live**; audit tamper detected 100%. | lineage diff (live vs cold) at 0 drift + tamper-detection proof | SCHED |
| **E2E-4 DSAR fan-out** | GDPR/Audit, Storage, Id, all 5 subsystems, Search, Refs, Notif, Workflow, Bus | **0 holders missed**; 0 recoverable PII (incl. vectors, incl. backups); residual == the one documented posture; certificate sealed. | H1–H18 coverage receipt set + post-erase locate = 0 + Merkle certificate | SCHED |

---

## 4. The definition of done for a capability

A capability has exactly two honest states.

- **PROVEN** — a drill from the catalogue (§3) **emitted a green artifact**: a dated, signed, scorecard-linked
  record in which (a) the behavioural assertion passed (0 lost / 0 leaked / latency-within-budget) **AND** (b)
  the telemetry assertion passed — the survival signal (contract 1.8) was present, correct, and alarmed where
  expected. A drill that survives but emits no signal has **failed**. A capability whose green artifact rests
  on a guessed number is not proven; the threshold must be the measured Q32 value.
- **CLAIMED** — everything else: the doc says it does X, the code may even do X, but no drill has forced the
  failure and watched it survive. "Claimed" is the honest word; saying "done" here is the failure mode the
  whole strategy exists to prevent.

Three corollaries bind this:
1. **Source-verified, never doc-verified.** A row is PROVEN only against the running code and its observable
   telemetry. When a doc and the code disagree, the code wins — fix the doc, then proceed.
2. **A named floor is not a failure; a floor masquerading as done is.** Shipping a partial/deferred capability
   is correct — record it in the gap report with its linked follow-on and its claimed/proven status. The FLOOR
   drills (GA-D8 multi-cell erasure, CP-D7 cell→cell migration, CP-D8 cross-cell ref) are owed only when their
   follow-on is built, and are listed now so the gap is visible.
3. **The gate invariant.** No later phase is done over a red earlier gate. A green feature row over a red
   substrate row is a contradiction the truth-up pass flags — a beautiful surface on a substrate that silently
   corrupts is not done, regardless of how complete it looks.

---

## 5. Handoff to Phase 6 — the gates that must be sequenced EARLY (order-by-non-negotiability)

Phase 6 sequences the build. Per order-by-non-negotiability (R-1/R-2), it must build and green these **before**
the feature surfaces that sit on top of them — they are keystone milestones, not side-cars, and a feature whose
substrate gate is red is not done.

1. **The failure-injection harness itself (R-3).** The unit of proof. The 1×/10×/30× load generator (mixed
   actor.kind), the scoped-reversible dependency-break primitive, and the telemetry-assertion library that reads
   the metrics port (contract 1.8). Nothing else in §3 is drillable until this exists, so it is sequenced first.
2. **The real-kernel sandbox-escape drill (AG-D4 / CI-T1).** The single hard go/no-go (GATE frequency) before
   **any** untrusted customer code runs — CI step or agent ToolHands::exec, one unified runner (ADR-20). Zero
   escapes, re-run on every backend/image/kernel change. Until it is green on the production backend, no
   untrusted CI step and no agent compute call runs. It sequences everything downstream of untrusted execution.
3. **Restore-verify + the silent-data-loss gates (STOR-D1/D2/D3, F3; the F5 zero-loss family).** Order-by-
   non-negotiability #1: silent data loss outranks every feature. The restore + cross-seam-integrity check is a
   **CI job, not an aspiration** (ADR-18) — RPO ≤ 5 min, RTO ≤ 1h-tenant/4h-cell, 0 loss, 0 resurrected erased
   subject. Built and green before the surfaces that write data on top of it.
4. **The committed architecture lints (the ratchet floor).** The twelve compile-time lints (no-cross-db,
   no-raw-publish, tenant-predicate, no-host-exec, residency-pin, no-llm-in-platform, no-untagged-personal-data,
   flow-determinism, search-requires-acl-filter, forward-only-migration, control-plane-pii-free,
   no-cross-sync-cycle), each shipped with a red-fixture + a green-fixture so the lint is proven to reject. They
   are the cheapest ratchet and make whole bug-classes impossible to compile, so they come early and stay green.

Also early, as Phase-6 sequencing prerequisites (R-3): the shared **overlay/state primitives** (built before any
feature consumes them, so the off-screen-picker / clipped-dialog / focus-leak bug-classes are foreclosed) and
the **contract-coverage scanner** (CI fails the workspace if any contract-index row lacks provider+consumer CDC
coverage — an uncommitted contract test is no contract test). Everything else (breadth, then polish/scale)
follows behind these keystones.

---

## 6. Cross-references

- Philosophy: [`external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
  · hard problems [`external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md).
- Directives: [`integration-directives.md`](../../02b-doctrine-integration/integration-directives.md)
  (Phase-5 T-1..T-9, Phase-8 E-1..E-9, Phase-6 R-1..R-6, lints E-5).
- Frozen surface under test: [`../contract-index.md`](../contract-index.md) +
  [`../00-reconciliation-decisions.md`](../00-reconciliation-decisions.md).
- Drill inventory consolidated here: [`../../03-shared-systems-architecture/drills-and-open-questions.md`](../../03-shared-systems-architecture/drills-and-open-questions.md)
  (Part A families F1–F9 + survival signals; Part B Q32/Q33) + the five subsystems'
  `04-subsystem-architectures/<slug>/architecture/07-drills-and-open-questions.md`.
- Spine: [`../../02-holistic-architecture/architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md)
  (ADR-16/17/18/19/20) · design-QA [`../../02-holistic-architecture/design-language.md`](../../02-holistic-architecture/design-language.md) §8b.
- The facet docs: [`00`](./00-philosophy-levels-and-gates.md) · [`01`](./01-whole-system-e2e-and-drill-catalogue.md)
  · [`02`](./02-parts-contracts-and-mock-agents.md) · [`03`](./03-gdpr-security-residency-and-ux-qa.md).
