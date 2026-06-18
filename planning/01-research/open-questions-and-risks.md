# Open Questions, Assumptions & Risks — Consolidated Register

> Phase 1 research deliverable (lead synthesis). Canonical brief:
> [`VISION.md`](../../VISION.md). Companion docs:
> [`README.md`](./README.md), [`personas.md`](./personas.md),
> [`use-cases.md`](./use-cases.md), [`competitive-landscape.md`](./competitive-landscape.md),
> [`gdpr-eu-sovereignty.md`](./gdpr-eu-sovereignty.md),
> [`agent-native-design.md`](./agent-native-design.md),
> [`technical-structuring.md`](./technical-structuring.md), and the five
> [`subsystem-deep-dives/`](./subsystem-deep-dives/).
>
> **Purpose.** This is the single, de-duplicated register of every open question, working
> assumption, risk, and explicit uncertainty surfaced across *all* Phase 1 documents. It
> exists so later phases (2–7) inherit a clean checklist rather than re-discovering the same
> tensions. Each entry states **what** the question/risk is, **why it matters**, and **which
> phase should resolve it**. Where two docs raised the same thing, it appears **once** here,
> with cross-references.
>
> **Honesty about uncertainty (VISION §3)** is the reason this document exists. Nothing here
> is resolved; this is the inventory of what is *not yet* decided.

---

## 0. How to read this

- Entries are grouped by **theme**: Product (§1), Technical/Architecture (§2), GDPR & Legal
  (§3), Agent-native (§4), Scale (§5). A given concern lives in its *primary* theme with
  cross-links where it spans themes.
- **Resolver phase** uses VISION §5's plan: **P2** = `02-holistic-architecture`,
  **P3** = `03-shared-systems-architecture`, **P4** = `04-subsystem-architectures/<sub>`,
  **P5** = `05-refined-shared-systems-architecture`, **P6** = roadmaps, **P7** = prompts.
  **Legal** = needs qualified counsel / DPO sign-off. **Commercial** = positioning/GTM work
  outside the engineering phases.
- **Risk severity** (engineering judgement, not measured): **H** could undermine the
  platform thesis; **M** significant rework if wrong; **L** local/contained.
- The most consequential cross-cutting items are collected in §6 ("Top cross-cutting risks").

A meta-note carried from every doc: **personas, priorities, and competitor facts are
hypotheses, not validated research.** No user interviews were conducted; competitor
feature/funding/sovereignty facts are flagged `[VERIFY]` in their source docs and must be
re-checked before any decision hinges on them.

---

## 1. Product & positioning

| # | Question / risk | Why it matters | Resolver | Sev |
|---|---|---|---|---|
| PR-1 | **Segment priority & willingness-to-pay are unvalidated.** Personas assume scale-up + regulated/public-sector are the strongest fits; no user research backs this. | Drives MVP scope, the wedge framing, and sequencing. If the real wedge is "one subsystem must win standalone first," the whole MVP set shifts. | Commercial → feeds P2 | H |
| PR-2 | **Issue-model duality:** one underlying issue object must serve a Linear-fast engineer sprint-board *and* a co-equal PM roadmap *and* Jira-grade corporate governance — without either view being a second-class projection. | The issue tracker's make-or-break UX bet; named co-equal audiences is a VISION mandate. Get it wrong and the tracker pleases no one (the classic Jira-vs-Linear split Myelin exists to heal). | P4 (Issues) | H |
| PR-3 | **Governance baked-in vs. opt-in schemes.** Can an org start "Linear-simple" and layer on workflow/SLA/permission/field schemes *without data migration*? | Determines whether one product spans startup→enterprise, or whether it forks into two. | P4 (Issues) | H |
| PR-4 | **Designer persona depth (P9): native authoring vs. referencing.** Does Myelin author designs/whiteboards/canvases, or only *reference* external tools (Figma)? | Scopes the Knowledge platform and the design/UX surface; affects P2 design language and P4 Knowledge. | Commercial + P4 (Knowledge) | M |
| PR-5 | **CD scope ambiguity.** Does Myelin ship an EU-sovereign deploy target/PaaS, or only deploy *mechanics* that call out to customer/third-party targets? (Phase-1 assumed the latter.) | Dramatically changes CI/CD scope, infra cost, and the sovereignty pitch. | Commercial + P4 (CI) | M |
| PR-6 | **Config format for CI** — pure declarative YAML vs. programmable (Dagger-style) vs. hybrid. Lean: declarative + schema-validated + dynamic escape-hatch. | Agent-generatability and human/PM-diffability are first-class selection criteria; supply-chain surface differs per choice. | P4 (CI) | M |
| PR-7 | **Folders vs. pure-pages** in Knowledge; **group DM vs. private channel** in Chat; **threads-first vs. channel-first** UX. | Familiarity for corporate buyers vs. model simplicity; small individually, collectively shape the product feel. | P2 (design) + P4 | L |
| PR-8 | **Migration/import fidelity** from Jira/Linear/GitHub/Confluence/Notion/Slack is the adoption gate (lossy rich-text, ID remapping, identity merge, history depth, attachment volume). | A company won't leave an incumbent unless import is high-fidelity; also a credibility signal for the "leave US SaaS cleanly" story. | P4 (per sub) + P6 | M |
| PR-9 | **Pricing / packaging / GTM / certification roadmap** (EUCS/NIS2/DORA/Gaia-X labelling). | Decides which gatekeeper personas can adopt; out of engineering scope but gates revenue. | Commercial | M |
| PR-10 | **The agent-native gap is narrowing.** Competitors ship agent features fast (GitLab Duo, GitHub/MS Copilot, CircleCI "Chunk"). Myelin's durable edge is the *combination* unified ∧ sovereign ∧ agent-native, not agent-native alone. | If positioning leans only on "agent-native," the moat erodes; must lean on the empty intersection. | Commercial | M |

---

## 2. Technical / architecture

### 2.1 Cross-cutting structural

| # | Question / risk | Why it matters | Resolver | Sev |
|---|---|---|---|---|
| TE-1 | **The glue rotting into "integrated-by-API."** If the shared contracts (event envelope, `ArtifactRef`, identity, `PersonalDataHolder`) drift or get bypassed (subsystem-to-subsystem DB access), Myelin becomes the stitched suite it exists to beat. | This is the platform thesis. Mitigation: contracts as shared crates in a monorepo; no cross-subsystem DB access ever. | P2 + P3 | H |
| TE-2 | **Permission model formalism.** Multiple deep-dives independently conclude simple RBAC is insufficient; a **relationship-based / Zanzibar-style tuple store** is the leading candidate. Needed for issue field/transition/confidential visibility, knowledge page-tree inheritance, chat per-viewer unfurls, and permission-filtered search/refs. | The single most pervasive correctness hazard (see §5). A leak here is both a security and a GDPR breach. Must be co-designed with Search and Refs from the start. | P3 (Identity) | H |
| TE-3 | **Monorepo vs. polyrepo.** Lean: monorepo + Cargo workspace so the glue crates can't drift. | Directly mitigates TE-1; affects build tooling, dogfooding, team blast-radius. | P2 | M |
| TE-4 | **Shared rich-content/block model** across Chat messages, Issue descriptions/comments, and Knowledge blocks. | Big upside (consistent rendering, mentions/refs as first-class nodes, one editor) vs. real coordination cost. Cross-subsystem. | P2 + P4 | M |
| TE-5 | **Shared "database/views" + field-definition primitive** between Issues and Knowledge (both are "typed records + table/board/calendar/timeline views + filters"). | Highest-impact reuse boundary in the platform. Too shared → can't get subsystem-specific performance; too separate → drift. Needs joint Issues↔Knowledge resolution. | P4 (Issues+Knowledge, joint) | H |
| TE-6 | **Single query AST** serving UI, CLI, API, automations, and agent triggers (recommended by Issues; useful platform-wide). | Avoids per-surface query languages and JQL-style parser footguns; agents need a machine-constructable form. | P3 / P4 | M |
| TE-7 | **Does Refs own issue hierarchy/relations and knowledge db-relations, or do subsystems keep local materialised structures projected into Refs?** | World-scale rollups may force a tracker-local materialised tree; affects the reference-graph design and rollup performance. | P3 (Refs) + P4 | M |
| TE-8 | **Build vs. embed the git core** (libgit2 / gitoxide `gix` / JGit / shell out to canonical git / Mononoke-class backend). `gix` server-side serving maturity is unverified. | Determines feasibility of the Rust steer for git; shelling out is the pragmatic baseline. | P4 (Git) | M |

### 2.2 Event bus & storage

| # | Question / risk | Why it matters | Resolver | Sev |
|---|---|---|---|---|
| TE-9 | **Bus delivery guarantee** — at-least-once + idempotent consumers (working assumption) vs. exactly-once. | Drives idempotency requirements on *every* consumer in *every* subsystem; load-bearing. | P3 (Bus) | H |
| TE-10 | **Event-sourcing vs. transactional-outbox per subsystem.** Lean: outbox/CDC by default, true event-sourcing reserved for high-audit aggregates (issue transitions, permission changes). | The platform contract ("reliable ordered stream of canonical events") is the same either way, which lets each subsystem defer — but replay/audit implications differ. | P3 + P4 | M |
| TE-11 | **The firehose-vs-control-event split.** CI logs (`ci.log.appended`), chat presence/typing/read-state, and collab op-streams must NOT traverse the durable bus the same way → likely **two transports**. | A major bus-design input; getting it wrong melts the durable bus or loses ordering on control events. | P3 (Bus) | H |
| TE-12 | **Per-aggregate ordering** (all events for one PR/issue/run ordered); global ordering explicitly not required. | Consumers and agents need causal ordering within a run/PR; the chosen transport must guarantee it. | P3 (Bus) | M |
| TE-13 | **Three storage tiers** (transactional / object-blob / log-firehose) and the concrete engines (Postgres-class, S3-compatible, wide-column for chat log, columnar OLAP for analytics, Tantivy/OpenSearch for search). CI is the heaviest consumer and will drive requirements. | EU-deployable, self-hostable, portable primitives are mandated; hyperscaler-locked managed services are forbidden. | P3 + P4 (CI) | M |
| TE-14 | **Human-readable monotonic keys at scale** (`ENG-1421` per-team counters). Gapless + distributed + high-throughput is a contention hotspot; is gaplessness even a real requirement? | Users perceive gaps as bugs; a single sequence row is a write hotspot. | P4 (Issues) | L |

### 2.3 Real-time, collaboration & workflow

| # | Question / risk | Why it matters | Resolver | Sev |
|---|---|---|---|---|
| TE-15 | **CRDT vs. OT** for Knowledge collaborative editing (and shared with Issue-tracker sync). Lean: CRDT (Yrs/Yjs) for offline-first + Rust alignment, server as relay+authority; high uncertainty, must be prototyped. | Dominates the Knowledge subsystem's architecture; block-tree (not just text) CRDTs add move/interleaving complexity (Peritext/Fugue/Kleppmann move-op). | P4 (Knowledge), prototype | H |
| TE-16 | **Block-tree storage model** — per-block rows (adjacency list) vs. document-as-single-CRDT-blob vs. hybrid. | Doc size & block-level features/permissions vs. collaboration simplicity. | P4 (Knowledge) | M |
| TE-17 | **Flexible-DB query model** — JSONB property-bag + derived indexable projection vs. per-database materialised tables vs. external query store. | The JQL performance trap; arbitrary user fields + flexible query is the single biggest query-perf risk in Issues and Knowledge databases. | P4 (Issues+Knowledge) | H |
| TE-18 | **Formula/rollup engine** — synchronous vs. async incremental dataflow; cycle detection; recompute fan-out limits; consistency guarantees. | A spreadsheet-style dependency graph; editing one cell can cascade across databases. A known Notion scaling pain. | P4 (Knowledge) | M |
| TE-19 | **Drag-to-reorder ranking at scale** (LexoRank / fractional indexing) with concurrent reorders by users *and agents*. | Rank exhaustion/rebalancing + concurrent-edit conflicts; needs CRDT-ish or server-arbitrated story. | P4 (Issues+Knowledge) | L |
| TE-20 | **Durable-workflow engine: build vs. adopt vs. Temporal** for multi-step, human-gated automations/agent workflows (deterministic workflow + non-deterministic activities, durable timers, HITL signals). | Large effort with sovereignty implications (Temporal-the-service vs. self-hosted vs. Rust-native lib). The right substrate for agent runs that pause days on a HITL gate. | P3 | H |
| TE-21 | **Real-time chat transport backplane** — WebSocket gateway + which pub/sub (NATS / Redis / Kafka-hybrid / channel-sharded actor model / BEAM-style). Millions of concurrent connections. | The single biggest Chat architecture decision; Rust gives a real edge (memory/conn, no GC) but BEAM/Elixir is a credible alternative for the connection tier specifically. | P4 (Chat) | H |
| TE-22 | **Diff-anchoring across rewrites** — anchoring review comments to (file, line, side, commit/diff-position) that survive force-push/rebase/base-branch movement. | A primary correctness/UX battleground in code review where competitors visibly differ. | P4 (Git) | M |

### 2.4 Git-specific

| # | Question / risk | Why it matters | Resolver | Sev |
|---|---|---|---|---|
| TE-23 | **SHA-1 vs. SHA-256** default object format (interop maturity vs. collision-resistance/future-proofing). | Strategic; SHA-256 buys security but risks tooling/client incompatibility. | P4 (Git) | M |
| TE-24 | **Storage/replication backend** — bare repos on replicated filesystem (Gitaly/Spokes-style) vs. object-store-backed packs (Mononoke/JGit-DFS); quorum-voting vs. primary+WAL vs. Raft for ref updates. | Major architecture fork governing consistency (linearizable protected-ref merges, no split-brain) and residency. | P4 (Git) | H |
| TE-25 | **Monorepo ambition** — how big a monorepo must Myelin support gracefully (partial clone/sparse/commit-graph), and where is the "use a Google-scale system" line? | Avoids over-building a Mononoke-class system in v1 while not failing large-but-normal monorepos. | P4 (Git) | M |
| TE-26 | **Forks & shared object storage** (alternates/dedup) vs. independent copies; **merge queue** in v1 or later; **in-UI conflict resolution / web editing** scope. | Storage economics & busy-repo UX vs. complexity (dedup complicates erasure + residency). | P4 (Git) | L |
| TE-27 | **World-scale code search & code intelligence** — its own multi-year effort (Blackbird/Sourcegraph class). v1 likely per-repo/per-tenant lexical only; defers global semantic/symbol nav. | Scoping this honestly prevents an unbounded subsystem from sinking the schedule. | P4 (Git+Search) | M |

### 2.5 CI-specific

| # | Question / risk | Why it matters | Resolver | Sev |
|---|---|---|---|---|
| TE-28 | **Default isolation model** for untrusted code — microVM (Firecracker/Cloud Hypervisor) vs. hardened containers (gVisor). Lean: microVM for untrusted; needs a threat model + perf/cost study. | The #1 CI problem: multi-tenant execution of arbitrary customer code (escape, SSRF to metadata, secret exfil, cross-tenant theft). The start-latency-vs-isolation tension is unresolved. | P4 (CI), security track | H |
| TE-29 | **Runner ownership & EU infra** — which sovereign substrate (Hetzner/OVH/Scaleway/Exoscale/bare-metal); self-hosted runner trust/attestation; macOS/Windows targets. | EU-sovereignty forecloses hyperscaler autoscaling primitives, making elasticity materially harder; self-hosted runners are non-negotiable for EU enterprise. | P4 (CI) | M |
| TE-30 | **Component/action registry** — does Myelin host an EU-sovereign reusable-component registry, and what is its supply-chain trust model (pin-by-digest, signatures, SLSA provenance)? | Reusable components are essential, but the registry is a supply-chain attack surface (cf. GitHub Actions incidents). | P4 (CI) | M |
| TE-31 | **CI ↔ agent execution substrate unification.** A CI job and an agent run have nearly the same shape (event → sandboxed work → results+events). | Efficient and shares the untrusted-code threat model (an agent running tool calls *is* untrusted code) — but a real decision with security implications. | P4 (CI) + P3 (Agents) | M |
| TE-32 | **Metering/billing unit** (build-minutes vs. credits vs. resource-seconds per runner class). | Affects scheduler and quota design. | P4 (CI) + Commercial | L |

---

## 3. GDPR & legal

> All flagged `[OPEN — LEGAL]` items need qualified counsel / DPO sign-off before they become
> binding architecture. The companion `gdpr-eu-sovereignty.md` is the authoritative source;
> this is the de-duplicated index.

| # | Question / risk | Why it matters | Resolver | Sev |
|---|---|---|---|---|
| GD-1 | **Right-to-erasure vs. immutability/integrity** of git history, append-only event log, and audit logs. *The single hardest GDPR design question.* Surfaced by every subsystem. | Full erasure from immutable VCS history may be technically impossible to fully guarantee. Mitigation: minimise PII entering immutable stores (pseudonymous/no-reply commit identities, references-not-payloads) + crypto-shredding + documented best-effort. | P3 + Legal | H |
| GD-2 | **Exact scope of Art. 17 erasure into immutable git history** vs. the "technically infeasible / disproportionate effort" limits. | Determines how much history-rewrite (filter-repo) tooling to offer and what residual obligation remains; needs counsel. | Legal → P4 (Git) | H |
| GD-3 | **`PersonalDataHolder` contract** must be implemented by *every* store/subsystem (5 subsystem DBs + search + event-bus history + caches/CDN + backups + agent memory/embeddings + reference graph + notifications + audit-with-carve-outs). "We forgot the search index" is a structural failure. | The spine of GDPR-by-construction; the DSR orchestrator fans out to all holders against the statutory deadline with verifiable receipts. | P3 | H |
| GD-4 | **Crypto-shredding as a first-class deletion primitive** (per-tenant, optionally per-subject envelope keys) — feasibility/cost of per-*subject* key granularity at scale. | The practical answer for backups, append-only logs, CI logs/artifacts, chat bodies, knowledge history. Per-subject keys are heavier; per-tenant is cleaner but coarser. | P3 | H |
| GD-5 | **Audit-log retention carve-out** — lawful retention period and erasure carve-out for audit/security logs per jurisdiction. | Audit logs contain personal data but must persist to evidence compliance; the carve-out scope is fact-specific and jurisdictional. | Legal | M |
| GD-6 | **Free-text PII erasure completeness.** Personal data hides in prose (commit messages, comments, docs, chat, CI logs, test fixtures) and cannot be found by foreign key. | Full automated detection is not solvable perfectly; the realistic design is reliable structured-reference erasure + tooling + a documented residual-risk statement (do not over-promise). | P3 + P4 + Legal | M |
| GD-7 | **Schrems II / EU–US DPF stability** ("Schrems III" risk). | Design stance: do NOT depend on transfer mechanisms; default no extra-EU transfer, gated if ever enabled. Need it confirmed as policy. | Legal | M |
| GD-8 | **CLOUD Act exposure of hyperscaler "EU sovereign cloud" partnerships** — actually severed, or only mitigated? | Affects which infra providers are allowed for the strongest-assurance tenants; "AWS Frankfurt" is not sovereign. | Legal | M |
| GD-9 | **EU AI Act final classification** of agent capabilities and the phased obligation timeline; confirm the "design-safe minimums" (agent labelling, HITL, logging, transparency) are sufficient. Issue-tracker workflows touching HR-like decisions could escalate to high-risk. | Even mock agents must build the logging/oversight/labelling hooks now (cheap to stub, expensive to retrofit). | Legal → P3 | M |
| GD-10 | **Gaia-X / EUCS / NIS2 / DORA / eIDAS 2.0 / EU Data Act** — hard requirements for target segments (esp. public sector) or merely advantageous? | Procurement-readiness for regulated/public-sector buyers; Gaia-X membership is NOT a sovereignty guarantee. | Legal + Commercial | M |
| GD-11 | **Controller vs. processor classification** per data category (`tenant-content` = processor; `platform-operational` = controller), driving DSAR routing, lawful basis, retention, and deletion authority. | The obligations differ by role; the tag must be schema-level so it can't drift. | P3 | M |
| GD-12 | **Data map / RoPA generated from schema-level tags vs. manually curated** (drift risk), and enforceable in CI. | If the inventory drifts from reality, no rights pipeline can be guaranteed complete. | P3 | M |
| GD-13 | **Worklog/productivity metrics & special-category data** — EU works-council/labour-law constraints; field-level sensitivity classification. | Some EU states restrict productivity surveillance; incident reports may carry health data. | Legal → P4 (Issues) | L |
| GD-14 | **Backup-window length vs. erasure SLA** — the residual exposure window we accept and document; post-restore re-erasure so restores don't resurrect deleted data. | Backups can't be surgically edited; crypto-shred + bounded retention + re-erasure is the answer, but the window is a documented residual risk. | P3 | M |

---

## 4. Agent-native

| # | Question / risk | Why it matters | Resolver | Sev |
|---|---|---|---|---|
| AG-1 | **`Agent` vs. `Service` principal kinds** — one type with a flag, or two distinct types? | Affects how much governance plumbing (budgets, loop protection, AI-Act duties) services inherit vs. agents. | P3 (Identity) | M |
| AG-2 | **Delegation / on-behalf-of algebra.** Effective permissions = `agent.policy ∩ delegation ∩ tenant.policy`; the exact intersection-vs-additive semantics and "agent may do X only when triggered by someone who can do X" are under-specified. | The least-privilege/data-minimisation story for agents (answers P12's deepest fear); needs a dedicated authz design pass. | P3 (Identity+Agents) | H |
| AG-3 | **Plan-then-apply boundary is provisional.** Agents return `Vec<Effect>`; the platform validates against permissions/budget/HITL then applies. The exact `Agent::handle` signature (single call vs. driven multi-turn loop with intermediate read-tool results), streaming, and context management may force revisions when `LlmAgentRuntime` is built. | The single most important safety+testability choice; the plan-then-apply core should survive, but the trait surface is the most likely thing to change. | P3 (Agents), revisit P5 | M |
| AG-4 | **Loop / runaway protection** — causation-depth caps + cycle detection + idempotent tools + per-tenant circuit breakers. Designed defensively but **unproven**; agents emit events that wake agents (the scariest failure mode). | A novel scale+safety concern under-specified industry-wide; wants adversarial design + load testing before production trust. | P3 + testing (P5) | H |
| AG-5 | **Agent-generated load governance** — per-run/per-agent/per-tenant budgets and quotas so agents can't runaway-drive the bus/CI/chat (cost + safety). Agents generate volume far beyond humans. | Without it, a misbehaving rule can fan-out-bomb a channel or rack up cost; a *novel* concern because agents are not rate-limited like humans. | P3 (Agents+Bus) | M |
| AG-6 | **MCP protocol-level compatibility** — the tool-surface *shape* (name + schema + description + invoke) is confident; current MCP wire specifics are unverified. | The same permissioned tool registry is meant to be exposed over MCP to external agents and consumed internally — verify before promising protocol compatibility. | P3 (Agents) | L |
| AG-7 | **`EventMatcher` predicate language** for triggers — CEL / JSONLogic / custom; must be safe and cheap to evaluate (no Turing-complete predicates on the hot path). | Trigger matching runs on every event at world scale; an unsafe/expensive language is a DoS and a footgun. | P3 (Bus+Agents) | M |
| AG-8 | **Agent memory / context / embeddings are a `PersonalDataHolder`** — erasable, restrictable, region-pinned, encrypted. Embeddings derived from personal data are themselves personal data. Erasing data that fed an LLM decision is hard. | GDPR reaches agent-produced artifacts and run logs; mitigation is "no fine-tuning on tenant data + erasable I/O logs," which needs a dedicated GDPR-vs-LLM note. | P3 + Legal | M |
| AG-9 | **Real LLM/agent backend must be an EU-sovereign, swappable sub-processor adapter** (mock now via strategy pattern). | A US LLM API would be a transfer + CLOUD Act + purpose-limitation problem at once; the mock/real seam is exactly what lets us pick an EU-sovereign backend without a rewrite. | P3 + Legal | M |
| AG-10 | **Suggest-by-default; human-confirm consequential actions; always labelled as agents; all agent actions audited** (GDPR Art. 22 + AI Act transparency). HITL gates surfaced as chat approval cards (durable workflow waits). | The default safety posture; consequential auto-decisions on people risk Art. 22 territory. Cheap to design now, expensive to retrofit. | P3 (Agents+Chat) | M |

---

## 5. Scale & operations

| # | Question / risk | Why it matters | Resolver | Sev |
|---|---|---|---|---|
| SC-1 | **Permission-aware reads at scale** (search, refs/backlinks, chat unfurls, issue lists). "A user must never find/see what they cannot access." Post-filtering large result sets leaks and is slow. | THE recurring correctness hazard across every subsystem; a leak is both a security and a GDPR breach. Must be co-designed with the authorization service (see TE-2). | P3 (Identity+Search+Refs) | H |
| SC-2 | **World-scale without a hyperscaler — the cell topology.** Cell = a region-pinned stack (all subsystems + shared systems) on commodity EU primitives; scale = many cells; global control plane holds no in-region personal data. | The reconciliation of "world-scale from day 1" with "EU-sovereign by construction." Multi-region collab/latency vs. residency, CI runner-fleet elasticity on EU infra, and chat's millions-of-connections tier are each hard on commodity substrate. | P2 + P3 | H |
| SC-3 | **Tenancy ↔ residency model** — pooled vs. siloed-per-region; tenant→cell assignment; multi-cell tenants; isolation spectrum (logical row-level → schema/DB-per-tenant → cell-per-tenant). Isolation must hold across *all* shared systems (bus topics, search indices, caches, blob prefixes, agent context, reference-graph partitions). | One architecture must serve a 3-person startup and a 10,000-person enterprise; a cross-tenant leak is a breach + sovereignty failure. | P3 | H |
| SC-4 | **Event fan-out at scale without loss** + the firehose split (see TE-9, TE-11). | The bus must reliably reach many consumers/triggers under load; drives idempotency everywhere. | P3 (Bus) | H |
| SC-5 | **Reporting/analytics over huge datasets** — a separate columnar/OLAP read store fed by the event stream (CQRS-style); the durable event log *is* the analytics source. OLTP sharded by tenant. | Cycle-time/CFD/velocity/SLA scans over years × millions of issues would kill the OLTP store. | P3 + P4 (Issues) | M |
| SC-6 | **Hierarchy rollups & dependency computation** — incremental/materialised rollups, debounced recompute, cycle detection across deep/wide trees + cross-team parents + a `blocked-by` link graph. | Naive recompute is O(bad); a prime candidate for event-driven async recompute rather than synchronous writes. | P4 (Issues) | M |
| SC-7 | **Reference graph on hot artifacts** — a popular issue/PR may have thousands of inbound edges; backlink queries must stay fast and permission-filtered. | Graph × access-control join is the recurring hard problem (overlaps SC-1). | P3 (Refs) | M |
| SC-8 | **Git hot-repo & clone-storm handling** — clone-bundle caching, replica fan-out, rate limiting, CDN distribution while respecting residency (residency forecloses non-EU replicas, conflicting with global latency). | A handful of repos get disproportionate traffic; residency vs. latency is a deliberate trade-off. | P4 (Git) | M |
| SC-9 | **CI log firehose** — live tail (low-latency fan-out) + durable archive + range read + search + secret-redaction-in-flight + erasure, bridged by one API. | A top-3 storage challenge for the whole platform; the prime driver of the firehose-vs-control-event split. | P4 (CI) + P3 (Bus) | M |
| SC-10 | **Chat write-fanout vs. read-fanout** (bodies vs. mentions/unreads), message-store substrate + hot/cold tiering, read-state hot path, presence O(N×M) fan-out. | Channel-centric read-fanout is the natural default with targeted write-fanout for mentions; presence/read-state must not pollute the durable bus. | P4 (Chat) | M |
| SC-11 | **SLA timers & scheduling at scale** — millions of running timers with business calendars, pauses, breach-at-exact-time firing, durable across restarts. | Breach events must fire reliably under load; needs a shared distributed scheduler/timer. | P3 (scheduler) + P4 (Issues) | M |
| SC-12 | **Accessibility (a11y) & i18n across all surfaces** — keyboard-complete, screen-reader-correct, RTL, multilingual (EU = many languages, legal a11y requirements). VISION mandates top-tier UX. | Easy to defer and expensive to retrofit; a11y is an MVP-priority use case (UC-EDGE-27). | P2 (design) + P4 | M |
| SC-13 | **Offboarding / decommission / break-glass / ownership-transfer of orphaned artifacts** at tenant and user granularity. | Easy to forget, expensive to retrofit, and a gatekeeper (P15) requirement; tenant offboarding = erasure at tenant granularity. | P3 + P4 | M |

---

## 6. Top cross-cutting risks (the ones that decide the platform)

These recur across the most documents and most threaten the thesis. They are the entries any
later phase should treat as load-bearing.

1. **The glue must not rot into "integrated-by-API"** (TE-1) — the entire differentiator.
   Mitigation: shared contract crates in a monorepo; no cross-subsystem DB access.
2. **Permission-aware reads at scale** (TE-2 + SC-1) — the most pervasive correctness/leak
   hazard; a Zanzibar-style authorization service co-designed with Search and Refs is the
   leading answer.
3. **Erasure vs. immutability** (GD-1/GD-2/GD-3/GD-4) — genuinely hard, partly legally open;
   minimise PII in immutable stores + crypto-shred + documented best-effort, with the
   `PersonalDataHolder` contract reaching *every* store.
4. **The issue-model duality** (PR-2) — one model as co-equal sprint-board and roadmap is the
   tracker's make-or-break UX bet and a VISION mandate.
5. **World-scale without a hyperscaler via the cell topology** (SC-2/SC-3) — the
   reconciliation of the two hardest VISION non-negotiables; multi-region collab/latency vs.
   residency is the deep unknown.
6. **Event-bus delivery semantics + the firehose split** (TE-9/TE-11/SC-4) — under-specified,
   drives idempotency everywhere and the entire CI-log/chat-stream design.
7. **Agent loop/runaway + agent-generated load** (AG-4/AG-5) — novel and under-specified
   industry-wide; agents waking agents can cascade; wants adversarial load testing.
8. **CRDT vs. OT for collaboration** (TE-15) — high-uncertainty, must be prototyped, dominates
   the Knowledge subsystem (and shared with Issue-tracker sync).
9. **Durable-workflow build-vs-adopt** (TE-20) — large effort with sovereignty implications;
   the substrate for human-gated agent workflows.
10. **Untrusted-code execution in CI** (TE-28) — the most security-sensitive subsystem;
    multi-tenant arbitrary-code execution justifies a dedicated security track.

---

## 7. Assumptions taken as working premises (validate or refute later)

Carried, de-duplicated, from across the docs. Each can be overridden with written
justification (VISION §3).

- **Cell-based, region-pinned topology** reconciles world-scale + sovereignty; control plane
  holds no in-region personal data. (P2/P3 to ratify.)
- **Subsystems own their state**; all cross-subsystem interaction via shared contracts
  (Id + Bus + Refs + holders); **no subsystem-to-subsystem DB access**.
- **Bus is at-least-once + idempotent consumers**, per-aggregate ordering, transactional
  outbox emission, firehose streams on a separate transport.
- **Automations and agents are one trigger engine** with different action handlers.
- **Monorepo + Cargo workspace**; the glue lives in shared crates.
- **Rust for hot-path cores** (default steer), per-subsystem choice otherwise; design boundary
  matters more than the language.
- **Reference graph is built from `ref.created` events** (events authoritative for edges).
- **No personal data leaves the EU/EEA by default**; transfers off and gated.
- **No cross-tenant model training on tenant content** by default.
- **CI references shared identities rather than copying PII**; "CD" = deploy *mechanics* (not
  a hosted runtime) in Phase 1.
- **Myelin references specialist authoring tools (e.g. Figma) rather than replacing them**
  (designer-depth undecided).
- **Agents are mock implementations behind the strategy pattern** in Phase-1 build; what
  matters is that the interaction model (trigger → scoped action → audit → optional HITL) is
  designed correctly so real agents drop in later.
- **MVP's job is to prove the cross-subsystem wedge**, not reach feature parity with any
  incumbent (priorities are sequencing guidance, not commitments).

---

## 8. Explicitly deferred entirely (out of Phase-1 scope)

- Concrete schemas, sharding internals, scheduler/config-language specs, CRDT/OT decision,
  storage-engine choices → P3/P4.
- UI flows, screens, empty/loading/error states, the shared design language → P2 design +
  P4 sketches.
- GDPR mechanism details (DSR orchestrator design, KMS key hierarchy, crypto-shred
  granularity) → P3.
- Detailed permission matrices / role definitions → P3.
- Concrete agent capability specs and mock implementations → P4 + build.
- Quantitative market sizing, pricing/packaging, GTM, certification roadmap → Commercial.
- Reference deployment architectures on named EU clouds → architecture phases.

---

## 9. Cross-references

- [`README.md`](./README.md) — Phase 1 index & executive summary.
- [`personas.md`](./personas.md) §7, [`use-cases.md`](./use-cases.md) §9,
  [`competitive-landscape.md`](./competitive-landscape.md) §9,
  [`gdpr-eu-sovereignty.md`](./gdpr-eu-sovereignty.md) §10–11,
  [`agent-native-design.md`](./agent-native-design.md) §7,
  [`technical-structuring.md`](./technical-structuring.md) §12–13 — the per-doc open-question
  sections this register consolidates.
- Subsystem deep-dives' "open questions / hardest problems" sections — the starting checklist
  for each P4 agent.
- **Seeds P2/P3:** §6 (top cross-cutting risks), §2 (technical), §5 (scale) are the direct
  inputs to holistic + shared-systems architecture.
- **Needs Legal:** §3 (GDPR) and AG-8/AG-9 — flagged for counsel/DPO before they bind.
