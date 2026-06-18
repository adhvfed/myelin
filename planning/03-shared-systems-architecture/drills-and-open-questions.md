# Phase 3 — Drill Inventory, Open Questions & Consistency Pass

> Phase: `03-shared-systems-architecture`. Companion to [`README.md`](./README.md) and
> [`contract-index.md`](./contract-index.md). Canonical brief: [`VISION.md`](../../VISION.md).
> Doctrine: PROVE-IT (EI-01 P3; EI-04 §4 — "a property does not exist until a drill forces the failure and
> observability watches the system survive"). **Complete across all 11 Phase-3 docs.** Date: 2026-06-19.
>
> **What this is.** (a) The **consolidated drill inventory** every shared system owes — quantified, de-duplicated,
> mapped to its owner and the telemetry it reads. This feeds the Phase-5 testing strategy: Phase 5 owns
> execution and sets the final thresholds; Phase 3 enumerates the obligation and proposes defaults-to-beat.
> (b) The **consolidated open questions** carried into Phase 4 / Phase 5 / Legal, de-duplicated, each tagged
> with a resolver. (c) A brief **consistency pass** flagging contradictions across the 11 Phase-3 docs with a
> recommended resolution.
>
> **Status of a property (EI-04 §4 / T-4):** a drill emits a **green artifact** when it passes; until then the
> property is **claimed, not proven**. A *FLOOR* drill is owed only *when its follow-on is built* — named here
> so the gap is visible.

---

## Part A — The consolidated drill inventory

### A.1 The cross-cutting drill families (the recurring "prove-its")

Most per-system drills are instances of **nine families**. The families are listed once; the per-system table
(A.2) maps each system's owed instance to its family, so Phase 5 can run a family across systems with one
harness and one scorecard column.

| Family | The property it proves | The quantified gate (default-to-beat) | Source |
|---|---|---|---|
| **F1 — Zero-escape / no-leak** | A viewer never finds/reads what they can't access (security ∧ GDPR breach) | 0 leaked docs/edges/backlinks/notifications, **0 count/IDF/ranking leak**, across an adversarial corpus; incl. under zookie staleness | ADR-03, SC-1 |
| **F2 — Cross-tenant IDOR** | No cross-tenant/cross-cell read via path-tenant spoofing | 0 cross-tenant rows; `tenant-predicate` lint catches a tenant-less query at compile time | EI-02 §1, ID-3 |
| **F3 — Restore + cross-seam integrity** | Rebuild from backups lands at one consistent point | 0 loss; row ↔ blob ↔ index ↔ event-offset mutually consistent; post-restore re-erasure runs (GD-14) | ADR-18, STOR-4 |
| **F4 — Reindex-from-cold parity** | A derived store rebuilds to match live, via the live consumer path only | cold == live (docs, ACL behaviour, ranking, edges, vectors, inbox); no bespoke recovery reader | EI-04 §5.3, SEARCH-1, REF-4, NOTIF-3 |
| **F5 — Zero-loss-across-reconnect / outbox no-ghost** | No event lost or ghosted across a broker drop or a producer/worker crash | 0 lost, 0 ghost, 0 duplicate effect (durable consumer resumes by name; dedup absorbs redelivery; outbox survives; workflow replays) | BUS-2/3, ADR-04.3 |
| **F6 — 30× agent surge + protected human lane** | A human request survives a machine-speed surge; other tenants unaffected | human-lane latency within budget; agent lane sheds (429 + Retry-After honoured); cross-tenant impact = 0 | ADR-16 |
| **F7 — Id-hiccup / fail-static** | A transient Id/CP hiccup degrades, doesn't cascade; a revoked actor is still denied | already-authenticated traffic survives within W; staleness ≤ `static_max` ≤ revocation SLA; zookie reads bypass cache | ADR-17, ID-1 |
| **F8 — Disabled-user → zero-access-in-N-min / revocation** | A disabled/revoked principal loses all access within N minutes | every surface denies within **N = 5 min** (default); token TTL + denylist + cache expiry all ≤ W; "new enemy" stale re-grant = 0 | ID-1/2, Zanzibar §2.4.4 |
| **F9 — Loop/runaway adversarial** | An agent→agent loop or storm is structurally halted, not by convention | loop halts ≤ depth ceiling; shared-root tripwire trips the per-tenant breaker; bounded pool drops over-cap (never forks); runaway stops at the wallet | AG-6, D8 |

Plus four **non-family** drills (each unique to a small set of systems): **escape drill** (sandbox, A.2 Agent
AG-D4), **erasure-reaches-everything** (holder-completeness, run per holder), **online-migration safety**
(expand→backfill→contract under load), and **audit-tamper-detection** (the Merkle/STH consistency proof).

### A.2 The per-system owed drills (consolidated, de-duplicated by family)

Each row is one drill a system **owes**. The **F#** column maps to A.1 (so Phase 5 runs a family across all its
owners at once). "FLOOR" = owed when the follow-on is built.

| Drill ID | System | Family | Quantified gate (proposed default-to-beat) |
|---|---|---|---|
| **SUB-D1** | Substrate (Bus+sub) | F5 | Kill a service between commit and publish → outbox delivers every committed event exactly-once-in-effect (0 ghost, 0 lost). |
| **SUB-D2** | Substrate (consumer template) | F5 | Drop the broker mid-stream → 0 lost across reconnect (bind-by-name, dedup); a slow subject does not block others. |
| **SUB-D3** | Substrate (§7) | F6 | 30× agent surge on one tenant → human lane holds, agent lane sheds, other tenants unaffected. |
| **SUB-D4** | Substrate (§8) | F7 | Inject an Id-dependency hiccup → already-authenticated traffic survives within W; revoked actor denied when window closes. |
| **SUB-D5** | Substrate (§6) | (retry storm) | Trip a downstream breaker → callers fail fast (no retry through tripped breaker) + honour Retry-After; no amplification. |
| **SUB-D6** | Substrate + Storage/GDPR | F3 | Rebuild from backups → no loss; OLTP ↔ blob ↔ index ↔ offsets at one consistent point. |
| **SUB-D7** | Substrate (§2.11, §4.1) | F2 | Cross-tenant read via path≠token tenant → 0 cross-tenant read; lint catches tenant-less query at compile. |
| **SUB-D8** | Substrate (§5.3, §7.4) | F9 | Adversarial agent→agent loop → depth ceiling + tripwire + bounded pool halt it. |
| **SUB-D9** | Substrate (§4.3) | (liveness) | Kill a critical dependency → instance reports not-ready + sheds; liveness does not restart-storm. |
| **SUB-D10** | Substrate (§9) | (migration) | expand→backfill→contract on a restored prod-scale copy under load → no blocking lock beyond budget; 0 downtime. |
| **ID-D1** | Identity | F8 | SCIM-disable a user → every surface (UI/API/git wire/agent) denies within N=5 min; cache+token+denylist ≤ W. |
| **ID-D2** | Identity | F7 | Break Id dependency → authenticated traffic survives on coarse cache; just-revoked principal still denied (zookie bypass). |
| **ID-D3** | Identity | F2 | Cross-tenant check/list/read via path spoof → 0 cross-tenant tuples readable. |
| **ID-D4** | Identity | F1 | Confidential issue / overridden page / private channel absent from any `list_objects`/search/refs for an unauthorized viewer. |
| **ID-D5** | Identity | F9 | Adversarial delegation: agent cannot act outside `agent.policy ∩ delegation ∩ tenant.policy`, incl. via a delegator who lost the right. |
| **ID-D6** | Identity | F8 | Kill a run mid-flight → per-run token revoked (teardown) AND auto-expires (tuple `expires_at`) within run-life ≤ W. |
| **ID-D7** | Identity | F8 | Revoke then re-read with the post-revoke zookie → no stale allow ("new enemy"). |
| **ID-D8** | Identity | F3 | Restore S1/S3 to a consistent point → no resurrected grants past an erasure; post-restore re-erasure runs (GD-14). |
| **ID-D9** | Identity | F6 | 30× agent surge on the authz hot path → human lane holds, agent lane sheds. |
| **BUS-D1** | Event Bus | F5 | Kill consumer + sever broker during sustained publish → 0 lost, 0 duplicate effects on reconnect. |
| **BUS-D2** | Event Bus | F5/(HoL) | Flood unhandled types at a `*`-subscribed consumer → whitelist-template consumer does not stall; lag alarm fires. |
| **BUS-D3** | Event Bus | (replay) | Replay a `correlation_id` tree → deterministic re-drive, idempotent, causality preserved (replay == original, exactly once). |
| **BUS-D4** | Event Bus | F5 | Crash producer between state-commit and publish → event still delivered (outbox), never without the state change. |
| **BUS-D5** | Event Bus | F4 | Wipe a derived store, `reindex(scope)` → rebuilt store byte-matches live. |
| **BUS-D6** | Event Bus | F9 | Self-triggering automation → depth ceiling + shared-root tripwire trip the per-tenant breaker before runaway. |
| **BUS-D7** | Event Bus | F6 | 30× agent publish surge on one tenant → human/control lane holds, agent lane sheds, other tenants unaffected. |
| **BUS-D8** | Event Bus | (erasure) | Erase a subject → inline-PII events unrecoverable (key destroyed); `*.erased` tombstones emitted; consumers degrade. |
| **BUS-D9** | Event Bus | (per-ref order) | Burst force-pushes to one hot ref → `git.ref.updated` delivered in push order per ref, parallel across refs, at target QPS. |
| **REF-D1** | Reference Graph | F1 | Confidential artifact referencing a public one does not appear in backlinks/traverse for an unauthorized viewer (incl. filter-mode, zookie staleness). |
| **REF-D2** | Reference Graph | F2 | Cross-tenant edge read via path spoof / crafted cross-tenant URN → 0 cross-tenant edge readable. |
| **REF-D3** | Reference Graph | F6 | "Referenced-by-50,000" artifact under concurrent permission-filtered reads → paginated p99 within budget; R4 serves post-promotion. |
| **REF-D4** | Reference Graph | F4 | Wipe `edge` index, `reindex` → byte-matches live; a TE-7 drift reconverges to the typed table (typed wins). |
| **REF-D5** | Reference Graph | (erasure) | Erase a subject + a referenced artifact → references become tombstones, person unresolvable (pseudonym shred), 0 recoverable PII, no 500 on resolve. |
| **REF-D6** | Reference Graph | F8 | Revoke access, re-read backlinks with post-revoke zookie → no stale allow (bypasses fail-static). |
| **REF-D7** | Reference Graph | F5 | Crash producer between content/relation commit and publish → edge event still delivered, never an edge without its content. |
| **REF-D8** | Reference Graph | (traversal bound) | Dependency cycle + 1000-deep chain → CTE terminates (visited-set + depth ceiling 16), cycle surfaced as a diagnostic, statement timeout respected. |
| **REF-D9** | Reference Graph | (sub-artifact tombstone) | Delete an embedded doc block / PR comment → embed degrades to a partial/relocated projection, not a 404; 0 dangling embed. |
| **REF-D10** | Reference Graph | F6 | 30× agent reference-creation + backlink-read surge → human read lane holds, agent lane sheds. |
| **SRCH-D1** | Search | F1 | Confidential/overridden/private artifact never in any `query`/`semantic` result (incl. counts, IDF, "more results", RAG). |
| **SRCH-D2** | Search | F1/F8 | Revoke, re-search with post-revoke zookie → excluded; default-consistency search excludes within W. |
| **SRCH-D3** | Search | F2 | Search scoped to another tenant via path spoof → 0 cross-tenant results. |
| **SRCH-D4** | Search | (erasure) | Erase a subject → every doc/field/**vector/embedding** purged (not hidden), unrecoverable; 0 orphan embedding. |
| **SRCH-D5** | Search | F4 | Wipe index, `reindex(scope)` → rebuilt index matches live (docs, ACL, ranking, vectors), live consumer path only. |
| **SRCH-D6** | Search | F6 | 30× agent/CI query surge → human search lane holds, agent lane sheds, other tenants unaffected. |
| **SRCH-D7** | Search | (freshness) | Under load, event→searchable p99 within the seconds-grade budget; index-lag alarms before user-visible staleness. |
| **SRCH-D8** | Search | (filtered-ANN) | Selective ACL/structured filter → k nearest **visible** neighbours (filter-during-traversal), not k-then-filter; recall@k ≥ threshold; no leak. |
| **SRCH-D9** | Search | F3 | Restore index with OLTP/blob/offsets → no resurrected erased docs (re-erasure runs); no row↔doc↔vector mismatch. |
| **SRCH-D10** | Search + Storage + Agents | (HYOK) | Mark a content class HYOK → Search/Agents skip it (`can_derive_plaintext_index()=false`); 0 HYOK plaintext in any derived store; only non-HYOK metadata searchable. |
| **NOTIF-D1** | Notifications | (ranking) | Replay a mixed week → every `critical`/`direct` item ranks above every `fyi`; inbox-read-latency-to-first-important within budget; an explain-trace per rank. Gate: 0 critical below an fyi. |
| **NOTIF-D2** | Notifications | (storm-control) | 1000 near-identical CI failures + a 30-comment PR burst → collapse to bounded items (`coalesce_count` correct, "+N more"); self-notifications suppressed. Gate: N identical → 1 item; 0 self-notifications. |
| **NOTIF-D3** | Notifications | F4 | Wipe `inbox_item`, `reindex(notif)` → rebuilt inbox matches live (items + read-state from source events). |
| **NOTIF-D4** | Notifications | F1 | Notify on a confidential issue/private channel to a viewer lacking access → humanised string is the tombstone ("a restricted issue"); title never appears; item suppressed if the recipient can't see the subject. Gate: 0 title/PII leak. |
| **NOTIF-D5** | Notifications | F6 | 30× agent-generated notification surge → human inbox-read lane holds, agent lane sheds, delivery-adapter bulkhead bounds provider load, other tenants unaffected. |
| **NOTIF-D6** | Notifications | (erasure) | Erase a user → every inbox item humanises to `[erased user]` (refs resolve to tombstone); 0 recoverable PII; off-cell-sent payload crypto-shredded/erasure-requested. |
| **NOTIF-D7** | Notifications | F5 | Start an escalation; kill Notif mid-`ack_window` → durable workflow resumes, pages the next step exactly once (no miss, no double); an ack stops the chain. |
| **NOTIF-D8** | Notifications | (quiet-hours) | Set DND; fire a `critical` escalation → it pierces quiet-hours and delivers, while a `watching` item is suppressed. Gate: critical pierces; non-critical suppressed. |
| **NOTIF-D9** | Notifications | (delivery idempotency) | Crash between provider-ack and ledger-write, retry → `UNIQUE(idem_key)` collapses it to exactly-one effective delivery per (item, channel). |
| **NOTIF-D10** | Notifications | F5/(HoL) | Inject a slow/poison Signal type → the whitelisted-template router does not stall, terminates poison, lag-alarm fires. |
| **AG-D1** | Agent Fabric | (plan-then-apply) | A tool tries to write outside `EffectApi` → structurally impossible (`no-host-exec` + `no-cross-db` lints green). |
| **AG-D2** | Agent Fabric | F9 | Effect outside the `∩` → `Denied` returns to the loop, no privileged fallback fires. |
| **AG-D3** | Agent Fabric | F9 | Agent attempts an effect its policy allows but delegation/tenant forbids (and vice-versa) → confined to the intersection. |
| **AG-D4** | Agent Fabric **(CI owns)** | **escape** | `compute` tool attempts a kernel escape on a **real kernel** → **zero escapes**. *The single hard gate before any agent runs untrusted code.* |
| **AG-D5** | Agent Fabric | (HITL) | Gated tool → withheld (returns error, does NOT mutate); card shows action+risk+cost; approval resumes + applies once; rejection halts. |
| **AG-D6** | Agent Fabric | F6/F9 | 30× agent dispatch surge → human lane holds, agent lane sheds, reserve/settle refuses over-budget runs, other tenants unaffected. |
| **AG-D7** | Agent Fabric | F9 | Adversarial agent→agent self-trigger → depth ceiling (12) + tripwire + bounded pool halt ≤ ceiling; per-tenant breaker trips. |
| **AG-D8** | Agent Fabric | F8 | Kill a run mid-flight → token revoked on teardown AND auto-expires ≤ W; 0 shared token leaked into the child env. |
| **AG-D9** | Agent Fabric | (determinism) | Run a scripted mock twice → identical proposed-effect sequences; `cargo-mutants` over event→trigger→effect→event ≥ mutation threshold. |
| **AG-D10** | Agent Fabric | (erasure) | Erase a subject → run trace + agent memory/embeddings crypto-shredded/purged; attribution falls back to opaque pseudonym. |
| **AG-D11** | Agent Fabric | F9 | Runaway loop vs an exhausted wallet → reserve refuses to start new runs (never interrupts in-flight); loop stops at the wallet. |
| **FLOW-D1** | Durable Workflow | F5 | Kill a worker at activity 5 of 10 mid-run → another re-leases, replays, resumes at step 6 with 0 re-executed side effects, 0 lost progress, exactly-once-in-effect. |
| **FLOW-D2** | Durable Workflow | (determinism) | Replay against a divergent/wrong-version definition → the divergence guard halts the run as `nondeterministic` + dead-letters; 0 silent divergence/double-effect. |
| **FLOW-D3** | Durable Workflow | (timer scale, SC-11) | Arm 1M+ durable timers over far-future buckets + a burst due in one minute → due timers fire within the tick budget; far-future cost ~nothing; a crash re-fires unfired (effectively-once). 0 lost/0 double-fire. |
| **FLOW-D4** | Durable Workflow | (multi-day HITL) | A gated workflow waits across a worker restart + a deploy; deliver `approval` hours/days later (double-click) → resumes, consumes once, runs/withholds correctly. Withheld tool does not mutate. |
| **FLOW-D5** | Durable Workflow | F5 | Crash between journaling an activity's DB write and emitting its event → journal + outbox committed together (one txn); 0 ghost, 0 lost. |
| **FLOW-D6** | Durable Workflow | F9 | Runaway agent loop vs a depleting wallet → a new spend-bearing activity refused at reserve; an in-flight one never interrupted. |
| **FLOW-D7** | Durable Workflow | F9 | Adversarial workflow→event→workflow loop → depth ceiling + bus tripwire + bounded activity pool stop it (drops/parks, never forks). |
| **FLOW-D8** | Durable Workflow | F6 | 30× surge of agent-initiated workflows → human-initiated lane holds, agent lane sheds, other tenants unaffected. |
| **FLOW-D9** | Durable Workflow | (erasure) | Erase a subject with inline-PII history/signal rows → keys destroyed (unrecoverable incl. backups), references tombstoned, structure preserved. |
| **FLOW-D10** | Durable Workflow | F3 | Restore `myelin-flow` PG to a consistent point → in-flight runs resume correctly; workflow store ↔ outbox offsets ↔ referenced rows at one consistent point; no run pointing at a vanished activity result. |
| **STOR-D1** | Storage | F3 | Rebuild from backups to offset T → 0 loss (checksum parity); OLTP↔blob↔index↔offset at one consistent point (no row→missing blob; derived==source-replay). **The headline durability gate.** |
| **STOR-D2** | Storage | (RPO/RTO) | Kill a cell; restore → RPO ≤ 5 min (WAL tail); RTO ≤ target (≤1h/tenant, ≤4h/cell). |
| **STOR-D3** | Storage + GDPR | F3 | Erase a subject; restore an *older* backup → the erased subject is still erased (post-restore re-erasure ran). 0 resurrected subjects. |
| **STOR-D4** | Storage | (crypto-shred reach) | Erase a subject; attempt recovery from backups → per-subject ciphertext unrecoverable (key destroyed, excluded from backup). 0 recoverable PII in any backup. |
| **STOR-D5** | Storage | (residency) | Attempt to read/replicate a tenant's data outside its region → impossible by construction (region in partition key; `residency-pin` rejects out-of-region writes). 0 cross-region personal-data egress. |
| **STOR-D6** | Storage | (KMS degrade) | Transient KMS outage → resolved-DEK reads survive (bounded TTL); hard-down → not-ready+shed (not fail-open). 0 plaintext-without-key. |
| **STOR-D7** | Storage | (blob integrity) | Corrupt an object → re-hash-on-read detects it (content-address mismatch); recover from replica/backup. 0 silent serve. |
| **STOR-D8** | Storage | (migration) | expand→backfill→contract on a restored prod-scale copy under load → no blocking lock beyond budget; 0 downtime. |
| **GA-D1** | GDPR/Audit | (erasure-reaches-every-holder) | Erase a subject seeded into *all* H1–H18 holders → the data-map-driven fan-out hit every holder; post-erase `locate` returns 0 recoverable PII. **0 holders missed.** |
| **GA-D2** | GDPR/Audit + Search | (erasure-reaches-search) | The subject's docs **and embeddings** purged+reindexed out (not hidden). 0 hits, 0 embedding re-identification. |
| **GA-D3** | GDPR/Audit | (audit tamper) | Retroactively edit/delete an audit entry → the chain breaks + a consistency proof against the published STH fails + the external witness mismatches. Tamper detected 100%. |
| **GA-D4** | GDPR/Audit | (DSR deadline) | Open a DSR → the durable timer fires a warning Signal before the 1-month deadline; the certificate seals on completion. 0 silent misses. |
| **GA-D5** | GDPR/Audit | (data-map drift) | Add an untagged personal-data field → `no-untagged-personal-data` lint fails the build; the data-map diff surfaces it. Build red on untagged PII. |
| **GA-D6** | GDPR/Audit | (legal-hold) | Set a hold over a subject; submit an erase → erasure is deferred-by-hold (not run), then resumes on hold-lift. 0 held-scope deletions. |
| **GA-D7** | GDPR/Audit + Bus/Agents | (restriction) | Restrict a subject → no indexing/agent-use/analytics/notification while storage is retained; reversible. 0 processing of a restricted subject. |
| **GA-D8** | GDPR/Audit + Tenancy | F2 | **FLOOR** — multi-cell erasure: fan-out iterates all `member_cells ∪ home_cell`; merged a complete receipt set. 0 cells missed. |
| **CP-D1** | Tenancy/CP | (PII-free) | Data-map over the control-plane schema → 0 `is_personal=true` columns; writing a name/email → build fails (`control-plane-pii-free`). |
| **CP-D2** | Tenancy/CP | F2 | Request to a cell for a `tenant_id` it doesn't host → misroute rejection, 0 cross-tenant/cross-cell read, audited. |
| **CP-D3** | Tenancy/CP | (residency) | Write where `row.region ≠ cell.region` → `residency-pin` boundary check rejects it; `residency_verify` attestation passes. |
| **CP-D4** | Tenancy/CP | F7 | Hard-down the control plane → already-placed tenants keep serving; only signup/provisioning degrades. |
| **CP-D5** | Tenancy/CP | (bulkhead) | Fatal fault / 30× surge in one cell → other cells unaffected; noisy tenant contained to its cell. |
| **CP-D6** | Tenancy/CP | F3 | Provision a fresh cell → it passes restore-verify + readiness before accepting any tenant; failing cell stays `provisioning`. |
| **CP-D7** | Tenancy/CP | F3 | **FLOOR** — migrate a tenant cell→cell (same region) → 0 loss across-seam, lands in-region, source crypto-shredded. |
| **CP-D8** | Tenancy/CP | F1 | **FLOOR** — cross-cell ref (multi-cell) → bridge carries only `subject`/`type`/`correlation_id`; target cell resolves per-viewer; unauthorized → tombstone. |

**Count: 101 enumerated drills** across 11 systems (10 substrate, 9 Id, 9 Bus, 10 Refs, 10 Search, 10 Notif,
11 Agent, 10 Workflow, 8 Storage, 8 GDPR/Audit, 8 Tenancy — minus a few shared-owner rows counted once at
their owner). Of these, **4 are FLOOR drills** (GA-D8/CP-D7/CP-D8 multi-cell+migration, owed when built) and
**1 is the hard gate** (AG-D4 sandbox escape, owned by CI — the single go/no-go before any untrusted code
runs). They collapse into **9 reusable families + ~7 unique drills** for Phase-5 scorecard purposes.

### A.3 The non-negotiable observability that makes a drill "proven" (T-1/T-4)

A drill does not pass by *not failing* — it passes by **asserting against the telemetry** that the system
survived. Every system exports its survival signals on the metrics port (`00 §10.2`): RED/USE per
principal-kind + tenant, consumer-lag, outbox-depth/dead-letter, breaker state + Retry-After issuance,
fail-static fresh/stale/closed ratios, shed counts per lane, causal-depth histogram + tripwire firings,
reindex parity hash, erase receipts, residency attestation, misroute count, **timer-wheel lag + replay rate +
nondeterministic-halt count (Workflow), important-buried-rate + dedup-collapse-ratio + delivery-success
(Notif), erasure-fanout-coverage + audit-append-lag + STH-publish-age (GDPR/Audit), backup-RPO-seconds +
restore-verify-pass + crypto-shred-lag (Storage)**. A Phase-3 doc that omits its signals fails X-1; a drill
that doesn't read them isn't a proof.

---

## Part B — Consolidated open questions (de-duplicated, tagged by resolver)

The same open items recur across docs (most visibly: multi-cell, the `list_objects` push-down encoding, drill
thresholds). They are de-duplicated here, each tagged with its **single resolver** and the docs that raised it.

### B.1 → Phase 4 (subsystems)

| # | Open question | Resolver | Raised by |
|---|---|---|---|
| Q1 | **Per-subsystem complete event taxonomy** (full dotted-name lists, `schema_ver` lineage, payload shapes) under the Bus §6 grammar | each subsystem | Bus §10.1 |
| Q2 | **Per-subsystem `requires_approval` defaults + the `list_subjects` "agent approver" role-bundle** | Issues/Git (+ each subsystem) | Agent §12.1 |
| Q3 | **`#sub` minting scheme per subsystem**, stable-across-edits (a block that moves keeps its id) | each subsystem | Refs §9, `00 §13 Q4` |
| Q4 | **TE-7 typed-relation schemas + per-subsystem `rel` enumerations** (transition guards, relation field type) | Issues + Knowledge | Refs §9 |
| Q5 | **Git indexable code projection** for code-search v1 (per-file/per-symbol projection event) + the AST/cross-ref follow-on | Git (+ Search) | Search §9.3, §10 |
| Q6 | **Collab op-stream durable transport** (resume-cursor protocol; the reconnect-loses-zero-ops drill) | Knowledge (KN-1) | Bus §10.3 |
| Q7 | **CRDT-vs-OT + block-tree storage** for Knowledge collaborative editing (TE-15/16) | Knowledge | ADR-05 |
| Q8 | **Flexible-field query execution model** (the JQL performance trap, TE-17) + formula/rollup engine (TE-18) + drag-rank at scale (TE-19); which fields are per-subject vs per-tenant keyed (driven by `classify`) | Issues + Knowledge (joint) | ADR-06; Storage §13 |
| Q9 | **Per-table "hot" flagging** for the forward-only-migration lint; **per-surface shed budgets** (CI/Chat heaviest) | each subsystem | `00 §13 Q2/Q3` |
| Q10 | **Cross-language harness parity** — the minimum wire contract + per-language shim (Chat connection tier, TE-21) | the diverging subsystem | `00 §13 Q1` |
| Q11 | **Explicit-vs-implicit agent dispatch policy** (CHAT-1) — implicit auto-wake on mention is a separate product feature (+ DPO sign-off) | Chat + Commercial | Agent §12.2 |
| Q12 | **Knowledge accepts a content-addressed agent-trace write** (AG-7); **trace verbosity/reasoning-capture policy** | Knowledge (+ Legal for verbosity) | Agent §11.1, §12.6 |
| Q13 | **Bridge-tier per-tenant index/DB provisioning** (schema-per-tenant vs DB-per-tenant cut-over + quota) | Search/Storage | Tenancy §16; Storage §13 |
| Q14 | **Git-history author/email erasure (GD-1)** — pseudonymous-commit-by-default (commit-time prerequisite) + history-rewrite limit + documented lawful-basis residual (the half the Bus did NOT solve) | Git (+ Legal) | Bus §4.8; GDPR §7 |
| Q15 | **Chat message-log engine** (wide-column vs OLTP+object) + per-subject keying of chat bodies (TE-13) | Chat | Storage §13 |
| Q16 | **Default Signal/notify-reason rule set + admin authoring UX** (which events are `direct`/`ambient`/`fyi`; the Zapier-class builder); **digest cadence**; **push-token lifecycle / multi-device**; **`inbox watch` live transport** (co-decided with TE-21) | Notif (+ design language) | Notif §9 |
| Q17 | **CI pipeline as a workflow** — the exact stage/step activity granularity, and how the `kind=ci` job spec maps to an activity | CI (+ Workflow) | Workflow §11.7 |

### B.2 → Phase 4 (shared-system co-design, joint)

| # | Open question | Resolver | Raised by |
|---|---|---|---|
| Q18 | **`list_objects` ↔ index/edge push-down encoding** — filter-clause shape vs enumerated id-set vs bloom-membership, and the size threshold Id uses. *Contract frozen (zookie-stamped `Filter`); encoding open.* | Id + Search + Refs (joint) | Id §15, Search §10, Refs §9 |
| Q19 | **Filtered-ANN traversal strategy** (filter-during-traversal vs brute-force fallback; HNSW vs IVF-PQ promotion) | Search | Search §10 |
| Q20 | **`EffectApi` ↔ `delegation()` call ergonomics** (single composed decision vs decomposed terms) co-finalised with `Agent::handle` | Id + Agent (joint) | Id §15, Agent §12.4 |
| Q21 | **Embedding model adapter** (which EU-hostable model, dimension, mock→real swap) | Search (runtime) | Search §10 |
| Q22 | **Initial EU multilingual analyzer set + CJK/non-segmented strategy** (mechanism decided; the list open) | Search | Search §10 |
| Q23 | **Saved-view ↔ EventMatcher AST shared grammar finalisation** (full operators/types, `ref_in`/`list_objects` composition) | Issues + Search + Bus | Bus §10.8, ADR-07 |
| Q24 | **`Agent` vs `Service` in the dispatch path** — resolved in Id as one kind/three faces; the dispatch loop guards must respect `actor.kind` | Id (frames) + Bus/Agent (respect) | Bus §10.7, Agent §12.3 |
| Q25 | **MCP wire + external-agent rate-limit lane + per-external-tenant budget** | Agent (+ Legal) | Agent §12.5 |
| Q26 | **HITL approval-card data model + UX** (batch-approval semantics, live-cost-estimate rendering, DL §11 overlay) | Chat + Agent Fabric + Workflow | Workflow §6.3, §11.1 |
| Q27 | **Mid-workflow token re-mint on resume** — `mint_run_token` callable when a multi-day workflow resumes (pre-wait token expired) | Id + Agent Fabric | Workflow §10.2, §11.2 |
| Q28 | **EU-sovereign delivery providers** (which EU email/push vendor + DPA; provider-side erasure of an already-sent off-cell payload) | Notif (+ DPO) | Notif §9.2 |
| Q29 | **BYOK/HYOK per-content-class policy** (which classes may be HYOK; the cross-artifact-reference-spanning case) + the KMIP/external-key-store adapter | Storage (+ Legal) | Storage §13 |

### B.3 → Phase 4 / control plane (the deepest unknown — multi-cell, de-duplicated to ONE item + migration)

| # | Open question | Resolver | Raised by (all converge here) |
|---|---|---|---|
| Q30 | **Multi-cell tenants — the BUILD.** The single deepest deferred item, raised identically by six docs: the **cross-cell PII-free pointer bridge** concrete protocol (bridged-field set + residency proof), **cross-cell DSR fan-out** (iterate `member_cells`), **cross-cell zookie consistency** (home-cell-minted zookie read in a member cell), **cross-cell search scatter-gather + residency-free merge**, **cross-cell backlink fan-out**, **cross-cell inbox aggregation**, **cross-cell workflow spanning**, and **multi-cell rebalancing**. *The design is decided (home-cell-authoritative + read-through + project-grain sharding); the build is deferred.* | P4 control plane (+ Storage/GDPR for DSR/migration; Id for zookie semantics) | Bus §7.4/§10.2, Refs §6.5/§9, Search §6.4/§10, Notif §5.4, Workflow §7.4/§11.3, GDPR §4.4, Id §14/§15, Tenancy §10/§16 (SC-2/SC-3) |
| Q31 | **Live tenant migration / rebalancing** (online cell→cell, same region; reindex-from-source + crypto-shred cut-over). Promotion trigger = a measured hot cell sealing cannot relieve. | P4 control plane + Storage/GDPR | Tenancy §6.4, §16 |

### B.4 → Phase 5 (testing strategy — measured thresholds)

| # | Open question | Resolver |
|---|---|---|
| Q32 | **All drill thresholds** — N in "N-min revocation" (proposed 5), the surge multiplier (proposed 30×), staleness window W's measured headroom (proposed 5 min), freshness p99 budget (seconds-grade), recall@k under filter, **RPO (proposed ≤5 min) / RTO (≤1h/tenant, ≤4h/cell)**, **timer-wheel fire-latency at 1M+ outstanding**, **important-buried threshold (Notif ML promotion)**, hot-fanout read budget (Refs R4 promotion), Bus column-store promotion volume (BUS-6), OpenSearch/IVF-PQ/Temporal promotion points. **Phase 3 proposes defaults-to-beat; Phase 5 measures and sets the numbers.** |
| Q33 | **Cell sizing-band numbers** (`tenants_max` per class; which capacity dimension binds first) — conservative now, tightened from load-test + per-cell telemetry (measure-before-shard). |

### B.5 → Legal / DPO

| # | Open question | Resolver |
|---|---|---|
| Q34 | **Ratify W** (fail-static staleness ≤ revocation SLA — L-1/GD-3): the residual GDPR-revocation exposure window, recorded in the RoPA. **Mechanism + `≤ revocation-SLA` constraint are fixed; the bound value is a DPO-ratified, written, dated call.** | DPO |
| Q35 | **EU AI Act classification** of the agent-governance/delegation labelling + HITL oversight (GD-9, L-3/L-4); trace reasoning-capture verbosity | Legal |
| Q36 | **Art. 17 erasure scope into immutable git history** (GD-1/GD-2); **audit-log retention carve-out** per jurisdiction (GD-5); **Schrems-III posture + HYOK as a mitigation** (GD-7); **EUCS/NIS2/DORA/Gaia-X applicability** (GD-10); **worklog/productivity as special-category** (GD-13) | Legal/DPO |
| Q37 | **Region change as new-tenant-+-DSR** (not in-place UPDATE); **tenant-slug PII screening**; **cross-cell pointer-bridge residency proof** (that `subject`/`type`/`correlation_id` are not personal data for a given tenant before multi-cell ships); **backup-window-vs-erasure-SLA residual + erasure-ledger retention** (GD-5/GD-14); **DPO ratification of the generated RoPA legal text** | Legal/DPO |
| Q38 | **Cross-tenant inbound public-ref visibility policy** (the `public` userset *mechanism* is decided; *when* an inbound public ref is shown is product/legal) | Product + Legal |
| Q39 | **EU-sovereign LLM sub-processor** for `LlmAgentRuntime` (AG-9) | Legal/DPO |

---

## Part C — Consistency pass (contradictions across the 11 Phase-3 docs)

A targeted cross-check for places where one system *exposes* an interface another *assumes differently*, or
where two docs disagree on a token/shape/owner. The Phase-3 docs are **strongly consistent** — they were
written consuming each other's contracts explicitly. **The single real gap flagged in the earlier (incomplete)
pass — "Notifications has no dedicated Phase-3 doc" — is now CLOSED** (`notifications.md` is a full design).
The remaining findings are seams to confirm; **no blocking contradiction.**

### C-A — **RESOLVED: Notifications is now a full Phase-3 doc.**
**What.** Refs, Agent (HITL card humanisation, NOTIF-1), and the overview treat Notif as a first-class shared
system and a `PersonalDataHolder`; directives NOTIF-1/2/3 bind it. The earlier synthesis (against an incomplete
set) flagged the missing doc as a real gap.
**Status.** [`notifications.md`](./notifications.md) now specifies the ONE inbox (C-9 resolved: "My Work"/
"Activity" are scoped `filter`s, one read-state truth), the Signal-driven router (step-0 authorize, never
leak), two-tier storm-control/dedup, the write-fanout/read-fanout split, **backend humanisation via Refs
`resolve` Display mode (the producer the Agent HITL card AG-D5 depends on)**, on-call/escalation on the durable
workflow, the EU-sovereign delivery fabric, and reindex-from-source (NOTIF-3). It introduces **no new
contracts** — it consumes Bus Signals (3.1), Refs `resolve` (5.2), Id `check`/`list_objects`/`list_subjects`
(4.2–4.4), and the holder contract (10.1). **Gap closed; confirmed.**

### C-B — **CONFIRM: `list_objects` `Filter` must be composable over an arbitrary id column.**
**What.** Id §8.2 returns `{ids | Filter{set_expr, zookie}}`. Search §9.1 assumes the `Filter` `set_expr` is
**facet-expressible** (compiles to a native posting-list predicate over `doc_id`/project facets); Refs §8.1
assumes the same `Filter` composes into `WHERE source IN (…)` over an **edge's source column**; Notif §3.5's
read-fanout uses `list_subjects` (a distinct but sibling concern). Two different id columns consume one
contract.
**Recommended resolution.** Captured as **README §4 S-10** and **Q18**: name it explicitly in the X-5
reconciliation — Id's `Filter` is **consumer-composable over an arbitrary id column** (no signature change). No
contradiction; the property must be a *committed* anchor, not an assumed one.

### C-C — **CONFIRM: the "Trigger" vocabulary collision is resolved consistently.**
**What.** ADR-08.5 / Phase-1 used "Trigger" for a matcher→target binding. ADR-19 / Bus §1.2 **renamed** that to
a *subscription/automation binding* and reserved "Trigger" for the stateful per-person promise.
**Check.** Agent §4.1's `run` uses `binding_id` (not "trigger"); Bus `automation_rule.run_as` carries the old
`run_as`; the Trigger `stale_after` is a `myelin-flow` timer (Workflow §3.3/§5.3, consistent). **No
contradiction**; the rename held across all docs including the new Workflow/Notif docs.

### C-D — **CONFIRM: `ToolHands::exec` vs `EffectApi` routing is unambiguous.**
**What.** The substrate stub (`00 §2.4`) shows `ToolHands::exec(cmd) → ToolResult` as "the hands," which could
read as *all* tool actions. Agent §2.2/§5.0 disambiguates: **side-effecting platform mutations go through
`EffectApi`; only untrusted *computation* (test/build/lint/script) goes through `ToolHands::exec`.** Workflow
§4.4/§6.1 routes both as activities consistently (`AGENT_STEP`/`TOOL_EXEC`).
**Recommended resolution.** **A one-line cross-reference in `00 §2.4`** pointing to Agent §5.0 so the stub is
not read in isolation. Contract-index 8.4 now encodes the split inline. No contradiction.

### C-E — **CONFIRM: the cross-cell pointer bridge is owned once, consumed six times.**
**What.** Bus §7.4 defines the PII-free pointer-event bridge (`subject`/`type`/`correlation_id`, never PII).
Refs §6.5, Search §6.4, **Notif §5.4, Workflow §7.4, GDPR §4.4**, and Tenancy §10 all *consume* it for their
own cross-cell fan-out; Tenancy §15.4 states "none required; consumes Bus §7.4 as written."
**Check.** All six name the **same field set** and the **same residency-proof obligation**. **Consistent** —
one owner (Bus/control plane), six named consumers; the concrete protocol is the single deferred Q30.

### C-F — **CONFIRM: fail-static staleness window W is one bound, referenced everywhere.**
**What.** `00 §8.2` defines `static_max ≤ revocation SLA ≥ agent-token TTL`, DPO-ratified. Id §10 proposes
**W = 5 min**; Search §4.2.3, Refs §4.4, **Notif §5.3** require zookie-stamped reads to **bypass** the cache;
Agent §5.7 / Workflow §10.2 tie agent-token TTL ≤ W; **GDPR §4.7 records W in the RoPA as the residual
revocation-exposure window** and names the DPO as the ratifier.
**Check.** All consumers reference *the same* bound and *the same* zookie-bypass rule. **Consistent.** The
single open item is the *value* of W (Q34, DPO) — one decision, not multiple.

### C-G — **MINOR: depth-ceiling default differs (12 vs 16) — intentional, but worth a note.**
**What.** The **causal/agent-loop** depth ceiling is **12** (Bus §4.7, Agent §5.5, Workflow §6.2 dispatch/run
depth cap). The **Refs traversal** depth ceiling is **16** (Refs §3.4/§4.5: CTE walk depth). These are
**different concepts** (an agent-causality loop cap vs a graph-traversal walk bound) and the values are
independent by design.
**Recommended resolution.** No change to values; **a one-line clarifying note** in each that the two ceilings
govern different things. Both are Phase-5-tunable defaults-to-beat (Q32).

### C-H — **CONFIRM: reserve/settle is one gate, fronting Agent, CI, and spend-bearing workflow activities.**
**What.** Agent §5.4 defines the universal reserve/settle gate (D8); Bus §4.7 says the dispatch tier calls it
before any run; Workflow §6.2/§4.4 wraps a *spend-bearing activity* in the same gate; CI-2 says the gate fronts
CI. The wallet is Commercial's (C-1); the gate is the Agent Fabric substrate.
**Check.** **Consistent** — one gate, three call-sites (agent runs, CI runs, spend-bearing activities), one
wallet owner. Contract-index 11.7 encodes it. The open item is only the wallet/pricing model (Commercial).

### C-I — **CONFIRM: the durable-timer substrate is owned once (Workflow), consumed by Bus/Issues/Notif.**
**What.** Workflow §3.3/§4.2 owns the durable-timer wheel (SC-11). Bus §3.6/§4.6 says the stateful Trigger's
`stale_after` "is delegated to the durable-workflow engine (ADR-09)"; Notif §2.4/§3.7 runs escalation +
snooze + SLA timers on it; Issues' SLA timers (ISS-1) ride it.
**Check.** Workflow §10.1 binds the delegation explicitly ("the bus's `stale_after` is a `myelin-flow`
timer"). **Consistent** — one timer substrate, four consumers, no second durable-timer implementation. The
build-vs-adopt (TE-20) is **resolved** in Workflow §2 (BUILD DBOS-class, not self-hosted Temporal).

### C-J — **CONFIRM: the policy/mechanism split between GDPR/Audit and Storage is clean.**
**What.** GDPR/Audit §1.2 owns *whether/when/prove* (the holder contract, DSR orchestrator, classification,
retention, audit log, the GD-1 reconciliation); Storage §0/§5 owns *how* (KMS hierarchy, GD-4 per-subject-vs-
per-tenant granularity, crypto-shred algorithm, cross-seam restore, post-restore re-erasure). The **erasure
ledger** is owned by GDPR/Audit (§7/§4.4, H16-adjacent, PII-free + non-shreddable) and *consumed* by Storage's
restore (§7.5).
**Check.** The boundary rule ("GDPR decides; the owning store does") and the erasure-ledger ownership are
stated identically in both docs (GDPR §1.2 / Storage §12.3). **Consistent**, no double-ownership. The single
X-5 reconciliation is the `pii_key_ref` value grammar (Storage §12.1 → README S-8).

### C-K — **CONFIRM: Notif is not a parallel agent-notification system.**
**What.** EI-02 §2 warns against a parallel agent path. Notif §1.4 makes an agent's "things addressed to me" a
**specialised consumer of the same Signal/inbox model** (an HITL card to a human is a Notif item with
`reason=approval_requested`), and the Agent HITL flow (Agent §5.3, Workflow §6.3) posts the `approval` signal
— it does not invent a second notification channel.
**Check.** **Consistent** — one inbox/routing model for humans and agents; the HITL card is a Notif item +
a durable-workflow signal, not a bespoke path.

**Consistency verdict:** **no blocking contradiction across all 11 docs.** The one previously-real gap (C-A,
Notifications) is **closed by the now-complete doc**; two recommended one-line cross-reference clarifications
remain (C-D into `00 §2.4`; C-G the two-ceilings note); the rest are "confirm the seam" findings captured as
README spine-changes (S-8/S-10/S-11) and Q18. The Phase-3 docs interlock cleanly because each consumer doc
explicitly cites the producer doc's frozen contract.

---

## Cross-references
- [`README.md`](./README.md) — the Phase-3 index, committed designs, spine changes, Phase-4 handoff.
- [`contract-index.md`](./contract-index.md) — the build-to contract surface (owner/consumer/definition).
- The 11 Phase-3 system docs (`00-platform-substrate`, `identity-and-access`, `event-bus`, `reference-graph`,
  `search-and-indexing`, `notifications`, `agent-fabric`, `durable-workflow`, `storage`, `gdpr-and-audit`,
  `tenancy-and-control-plane`).
- Spine: [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md);
  [`integration-directives.md`](../02b-doctrine-integration/integration-directives.md);
  [`consistency-review.md`](../02-holistic-architecture/consistency-review.md).
