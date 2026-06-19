# Phase 7 — Prompt Ledger: Chat (the maximal-consumer subsystem)

> Phase: 07-prompts (per-system file, Phase 7-A). The complete ordered set of implementation prompts that
> operationalize the entire chat roadmap (planning/06-roadmaps/subsystems/chat.md, milestones M2-C0 + M4-C1..M4-C9
> + the M5 follow-ons M5-C-S1/S2/S3/X1/X2 + the E2E-wedge participation + M6 the switch test) into clean-context,
> independently-committable coding tasks. Built to the template in planning/07-prompts/00-ledger-overview.md §2
> (every field present, never implicit) and banded to planning/06-roadmaps/00-master-sequencing.md §2 (M0..M6,
> the gate invariant). Frozen architecture (this file OPERATIONALIZES, it does not redesign):
> planning/04-subsystem-architectures/chat/architecture/ (00..07) + planning/04-subsystem-architectures/chat/design/
> (information-architecture, user-flows, wireframes — PRESERVED from Phase 4) + the build-to contracts in
> planning/05-refined-shared-systems-architecture/contract-index.md + 00-reconciliation-decisions.md
> (X-1/X-2/X-4/X-6/X-7, OQ-E/OQ-F/OQ-G/OQ-I/OQ-J/OQ-K/OQ-L). Drills:
> planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
> (CHAT-D1..CHAT-D19 + the shared families + the E2E wedge). Plain-text identifiers throughout (no
> backticks-as-emphasis). Markdown only; this file makes no commits. Date: 2026-06-19.
>
> The global P-NNN ids are assigned by the consolidated ledger index (Phase 7-B, 01-ledger-index.md) when these
> per-system prompts are interleaved into the single execution order. Here each prompt carries a stable local
> handle CHAT-P<n> so its DEPENDS-ON edges are unambiguous before global numbering; the index rewrites CHAT-P<n>
> to its P-NNN. Where a prompt depends on another system's prompt not yet numbered, it names that system's
> milestone (the index resolves it to the P-NNN).
>
> Chat is the MAXIMAL CONSUMER subsystem (master §2 M4, §3.2): it unfurls everything, references everything, and
> is the most visible surface of the agent-native principle. Its bulk lands in M4, the last subsystem band, with
> a freeze-so-dependents-compile slice in M2 (M2-C0) and world-scale follow-ons in M5 and the dogfood switch test
> in M6. Chat is one hop off the critical-path spine: the agent-native flagship E2E-2 (CI-fail → triage agent →
> issue → chat → fix-PR) terminates in chat, so chat must be green before the M5 whole-system wedge. Two permanent
> gates bound every chat prompt that touches their surface (master §4): STOR-D1/D2 (restore-verify — chat writes
> no real message over a red restore-verify) and AG-D4 / CI-T1 (sandbox escape — chat runs no agent compute over
> a red sandbox gate). Neither is chat-owned; both bound chat's "done".
>
> Coverage (every roadmap milestone → its prompt(s), no gap): M2-C0 → CHAT-P1; M4-C1 → CHAT-P2 + CHAT-P3; M4-C2 →
> CHAT-P4; M4-C3 → CHAT-P5; M4-C4 → CHAT-P6; M4-C5 → CHAT-P7; M4-C6 → CHAT-P8; M4-C7 → CHAT-P9; M4-C8 → CHAT-P10;
> M4-C9 → CHAT-P11; M5-C-S1 (the F6 surge family + the E2E wedge participation) → CHAT-P12; M5-C-S2/S3/X1/X2 (the
> named floor promotions) → CHAT-P13; M6 (the switch test) → CHAT-P14. Fourteen prompts, no milestone gap.

---

### CHAT-P1 — Declare the chat contract surfaces (the chat.* taxonomy, the ReBAC fragment, the #sub mints, the humanise keys, the notif rules, the firehose scope) so the shared layer compiles

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-C0 (planning/06-roadmaps/subsystems/chat.md §4 "M2-C0 — Chat declares its contract
  surfaces"). This is the parallel pre-work slice of chat that ships INSIDE M2 so the shared layer and dependents
  compile and the firehose resume-cursor transport (chat's correctness backbone, shared with Knowledge collab and
  Issues boards) is co-designed once.
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons +
  the twelve lints + the contract-coverage scanner (master §2 M0; substrate roadmap SUB-M0). The M1 Identity
  prompts that ship the ReBAC namespace engine (contract 4.9) into which fragments compile, plus the Bus event
  taxonomy seed (2.9) and the EventEnvelope freeze (2.1). The M2 shared-layer prompts being built in the same
  band: Refs freezes the #sub grammar + the 4-step tombstone ladder (5.7) and project() (5.6); Notif freezes
  humanise (7.3) + define_notif_rule (7.6); Bus freezes the firehose resume-cursor protocol (3.5). The index
  places this alongside the M2 reactive-layer freeze (Identity/Notif/Refs/Bus must accept chat's contributions).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md (always) §3 (name-your-floors; agent-native from the ground up; GDPR-safe by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §5 (the ratchet — an uncommitted gate is no gate),
    §1 (name-your-floors, code-wins-over-docs), §7 (reconcile cross-component contracts at the plan layer before
    either side ships — chat declares its half so Identity/Notif/Refs/Bus compile against it now).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md (the COMPLETE
    chat.* taxonomy §1, the durable-vs-firehose split §1.1/§1.2, the ArtifactRef + #sub mints §2, the ReBAC
    fragment + watcher relation §5, the fanout-class declaration §4, the humanise/notif-rule registration);
    00-overview.md §0 (where chat declares early) + §2.1 (the four owned hot parts vs consumed contracts).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen Chat
    ReBAC fragment — channel.read = member + parent_project->read + the watcher relation), §OQ-J (the firehose
    resume-cursor protocol shape — subscribe/resume/scope, resync_required → *.snapshot), §X-4 (the frozen #sub
    grammar — message-/thread- are the Chat kinds), §OQ-L (humanise is the ONE templating surface).
  - Contracts: contract-index.md rows 4.9 (per-subsystem ReBAC fragment; Chat frozen), 2.9 (event taxonomy +
    token table grammar <subsystem>.<type>.<event>; chat.* tokens; per-aggregate conversation_id), 2.1
    (EventEnvelope the chat.* shapes align to), 5.1/5.7 (ArtifactRef + the frozen #sub grammar; chat mints
    message-/thread-), 5.6 (project REQUIRED), 7.3 (humanise keys), 7.6 (define_notif_rule), 3.5 (the firehose
    resume-cursor protocol + scope=channel:<id> bounding), 1.7 (the cross-language harness shim — the TE-21 BEAM
    hatch is written-but-closed, a no-op in the Rust default), 1.6 (the lints chat compiles against).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M2-C0 work + entry/exit) + §2 (upstream deps table
    rows 4.9, 2.9, 2.1, 5.7, 7.3, 7.6, 3.5, 1.7) + §3 (the contracts chat owns, the "declare" rows).
  - Drills/strategy: testing-strategy/README.md (the strategy) — no chat runtime drill here; the exit is the
    contract-coverage scanner + the compile of the chat fragment.
- **DELIVERABLE (what to build + exactly where in the repo).** In a new chat subsystem implementation crate
  (myelin-chat, under the Cargo workspace) plus its contributions into the shared glue crates and the cell schema:
  - The COMPLETE chat.* event taxonomy as the durable-via-outbox set vs the firehose-only set (arch 03 §1.1/§1.2),
    registered into the Bus taxonomy seed (2.9): durable — chat.message.created/edited/deleted/erased/mentioned,
    chat.reaction.added/removed, chat.thread.created/replied, chat.channel.created/archived/member_added/
    member_removed/linked, chat.read_state.updated (coarse), chat.{channel,message,thread}.snapshot; firehose-only
    — chat.presence.*, chat.typing.*, fine-grained chat.read_state.*, agent.message.partial, the live delivery
    frame. Validate each against the Bus §6.2 singular token table (chat is the canonical subsystem token; types
    channel/message/thread plus reaction/presence/typing/read_state). Freeze aggregate = conversation_id.
  - The Chat ReBAC namespace fragment submitted into the one cell schema Identity compiles (4.9): channel.read =
    member + parent_project->read, plus the watcher relation per watchable type (channel/thread). The fragment
    must COMPILE in the cell schema — that compile is this prompt's gate, not a runtime property.
  - The #sub grammar contribution frozen into the Refs frozen vocabulary (5.7): message-<opaqueid> (single
    message), thread-<opaqueid> (thread root); the <opaqueid> is the immutable message_id / thread_root_id ULID
    (a stable opaque id, not a positional index). State the stability obligation is chat's (a message id / thread
    id is immutable; the #sub survives edits).
  - Register the humanise template keys (7.3) for chat card strings, agent-message strings, and
    chat.message.mentioned — no chat-private string map. Register the define_notif_rule set (7.6): mentioned /
    replied / thread_watched / approval_requested, each with its dedup template and default class, and the
    fanout-class (write-fanout for the bounded high-signal set; read-fanout for the unbounded ambient set).
  - Co-design + validate the firehose resume-cursor protocol (3.5) against chat's scope = channel:<id> bounding:
    the per-view scope shape, the resync_required → *.snapshot fallback contract. No transport implementation here
    — only the validation that chat's scope shape fits the frozen protocol.
  - Pin the connection-tier language call (TE-21): Rust default; the BEAM/Phoenix hatch written-but-closed,
    bounded by the frozen harness shim (1.7). State this is a no-op in the all-Rust default.
  - FLOOR named: this prompt ships CONTRACTS, not a working chat. State in the crate doc that no chat behaviour
    ships here — only the shapes Identity/Notif/Refs/Bus compile against — and name CHAT-P2 (the durable message
    store) as the milestone where the behaviour begins. This is the honest M2-C0 floor.
- **CONTRACTS TO IMPLEMENT.** 4.9 the Chat ReBAC fragment (owned — the fragment definition, compiled by Identity).
  2.9 the chat.* event tokens (owned — registered into the Bus seed). 5.1/5.7 the chat #sub mints message-/thread-
  (owned — the grammar contribution; minting lands in CHAT-P2). 7.3 the humanise keys + 7.6 the define_notif_rule
  set (owned — registered; used in CHAT-P7/P8). 3.5 the firehose scope=channel:<id> shape (consumed — validated
  against the frozen protocol). 5.6 project() declared (owned — implemented in CHAT-P6). 1.7 the harness shim
  (consumed — no-op). Implement to the frozen shapes; a needed change is a whole-workspace contract PR, escalated
  and written down, not a local divergence (code-wins-over-docs).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Chat ReBAC fragment COMPILES in the shared cell schema Identity builds (a build-time gate, not a runtime
    drill) — CI, the compile is the green artifact.
  - The chat.* tokens are present in the Bus taxonomy and parse under the §6.2 grammar (0 ungrammatical tokens) —
    CI, token-grammar signal = 0 violations.
  - The contract-coverage scanner passes for every chat-owned contract row (provider + consumer CDC coverage
    present, even if stubbed) — CI, scanner signal = 0 uncovered chat rows.
  - The firehose scope=channel:<id> shape validates against the frozen 3.5 protocol (0 unbounded-scope
    declarations; scope is never *) — CI. (No chat runtime behaviour drill here — §4 M2-C0 exit is explicitly
    declarations, not behaviour.)
- **TESTS (required).** Unit tests that the Chat ReBAC fragment compiles, that each chat.* token round-trips the
  §6.2 grammar, that the #sub message-/thread- mints parse/format under 5.7, and that the firehose scope shape is
  bounded (never *). The provider/consumer CDC stubs for contract rows 4.9, 2.9, 5.7, 7.3, 7.6, 5.6, 3.5. State
  the cargo-mutants mutation-score floor for any mandatory-core module touched (the fragment-compile + token-parse
  modules); if none is mandatory-core, say so explicitly.
- **DEFINITION OF DONE.** The chat crate exists and compiles in the workspace; the Chat ReBAC fragment compiles in
  the cell schema; the chat.* tokens are registered and grammatical; the #sub mints + humanise keys + notif rules
  are registered; the firehose scope shape validates against 3.5; the CDC stubs and unit tests pass; the
  contract-coverage scanner is green on the touched rows; all twelve committed lints are green; the
  contracts-not-behaviour floor note is written naming CHAT-P2 as the follow-on; the work is committed. No gate is
  greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: Chat contract surfaces (chat.* taxonomy + ReBAC fragment + #sub mints + humanise
  keys + notif rules + firehose scope). Body lists: 4.9 (Chat fragment) compiled, 2.9 (chat tokens) registered,
  5.7 (message-/thread- #sub) frozen, 7.3/7.6 (humanise/notif) registered, 3.5 (channel scope) validated; the
  contract-coverage scanner green on the chat rows; the contracts-not-behaviour floor named (CHAT-P2 begins
  behaviour). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CHAT-P2 — The durable message store + the outbox co-commit + idempotent send (the silent-data-loss floor for chat)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C1 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C1 — The durable message store +
  the outbox co-commit"), the message-persist + outbox + idempotent-send slice. The Conversation/Membership
  service half is CHAT-P3.
- **DEPENDS-ON.** CHAT-P1 (the chat.* tokens + #sub mints + the chat crate). The M1 Storage prompts that ship the
  OLTP tier + RLS + encrypted columns + the outbox (11.1), the BlobStore content-addressed fs-backed floor (11.2),
  the KMS hierarchy + per-subject DEK (11.3/11.4), and — the hard blocker — restore-verify STOR-D1/D2 GREEN (chat
  writes no real message over a red restore-verify). The M1 Tenancy prompts that ship the (tenant, region)
  partition + residency-pin (12.1/12.4). The M0 Bus prompts that ship OutboxTx::emit (2.2) + the outbox table with
  UNIQUE(aggregate, seq) (2.3) + the EventHandler template (2.4) + consumer_dedup (2.5). The M1 GDPR prompts that
  ship the PersonalDataHolder trait + auto-registration (10.1/1.4) + the no-untagged-personal-data lint (10.2).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors; world-scalable from day 1; GDPR-safe by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability — silent data loss
    outranks every feature; this is built BEFORE any live delivery, any UI), §3 (prove-it-or-it-isn't-real — the
    co-commit drill forces the failure and observability watches it survive), §1 (name-your-floors);
    ../../external-insights/04-hard-problems.md §1 (erasure-vs-immutability — chat bodies ARE PII, the per-subject
    DEK never bakes erasable plaintext into an immutable log).
  - Architecture: ../04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md (the MessageStore
    trait, the Postgres-partitioned hot tier by (tenant, region) + time sub-partitions, the cold-segment tier, the
    message/draft schema, the per-subject-DEK body fields); 02-internals-and-algorithms.md §1 (the message log,
    ULID order, idempotent send, the per-conversation total order); 03-events-contracts-and-glue.md §1.1 (the
    durable chat.* set via the outbox), §9 (the envelope via the OUTBOX — the only emit path, the co-commit, no
    dual-write); 05-hard-problems.md §5 (chat is the most PII-dense holder; the per-subject DEK case).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (BUS-2 the
    persist+emit co-commit; the per-aggregate conversation_id ordering).
  - Contracts: contract-index.md rows 2.2 (OutboxTx::emit, the ONLY emit path), 2.3 (the outbox table
    UNIQUE(aggregate, seq), per-conversation aggregate ordering, the D-9 drill at QPS), 11.1 (OLTP tier + RLS +
    encrypted columns + the outbox), 11.2 (BlobStore fs-backed floor — the cold segments), 11.4 (crypto-shred /
    per-subject DEK for bodies/drafts), 11.5 (backup/restore + restore-verify STOR-D1/D2 — the floor chat sits
    on), 5.1/5.7 (the message-/thread- #sub mints, stable across edits), 10.1 (PersonalDataHolder over every
    store), 10.2 (the #[personal_data] tags + the no-untagged-personal-data lint), 12.1 (the (tenant, region)
    partition key), 2.6 (replay(scope, since) — the skeleton here, full parity in CHAT-P9).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C1 work, entry, exit, the ScyllaDB floor note) +
    §1 (the non-negotiability order item 1: silent message loss / dual-write) + §6 (first-runnable progression).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D13 (crash between persist and
    emit → both or neither), CHAT-D14 (retry same client_nonce → one message), CHAT-D2 (burst sends/edits → per-
    conversation total order).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat:
  - The Message Service behind a MessageStore trait (append / range / tombstone / resync_from), with the Postgres-
    partitioned hot tier by (tenant, region) + time sub-partitions and the object-segment cold tier (the fs-backed
    BlobStore floor, 11.2) — IDENTICAL behaviour under either hot engine (the trait is the swap seam).
  - The message persist + the chat.message.created outbox row in ONE PG transaction via OutboxTx::emit (BUS-2,
    2.2; no dual-write; the Message Service owns the only emit path — the gateway, built in CHAT-P4, has none).
  - k-sortable ULID message_id for intrinsic per-conversation order; aggregate = conversation_id with the outbox
    UNIQUE(aggregate, seq); per-conversation total order preserved under burst (the D-9 / CHAT-D2 property).
  - Idempotent send: UNIQUE(conv, client_nonce) so a retried send yields exactly one message.
  - Per-subject DEK encryption of body_inline / body_nodes + drafts (chat bodies ARE PII, 11.4); the body is
    NEVER stored as erasable plaintext in the immutable log.
  - The message-<id> / thread-<root> #sub minting (5.7), stable across edits (an edited message keeps message_id).
  - PersonalDataHolder auto-registration over every chat store opened by the harness (10.1/1.4); PII fields tagged
    #[personal_data(category, role, basis, retention, erasure, subject_locator)] so the no-untagged-personal-data
    lint is green (10.2).
  - The replay(scope, since) SKELETON (re-emit chat.{message,channel,thread}.snapshot through the outbox via the
    live consumer; sub-artifact granular). FLOOR named: full Search/Refs/Notif parity is proven in CHAT-P9 (M4-C7)
    — this is the skeleton only.
  - FLOOR named: the message hot tier = Postgres-partitioned; ScyllaDB hot tier is the named M5 follow-on
    (M5-C-S2 / CHAT-P13), triggered by measured per-cell write/partition volume. The MessageStore trait makes it
    a swap; the cold tier + trait are identical either way.
- **CONTRACTS TO IMPLEMENT.** 2.2 OutboxTx::emit co-commit (consumed — every chat state change commits its row +
  its event in one tx; the Message Service is the only emit path). 2.3 the outbox table per-conversation ordering
  (consumed). 11.1/11.2/11.4 the OLTP + blob + per-subject-DEK stores (consumed — chat's message log, cold
  segments, body encryption). 5.1/5.7 the message-/thread- #sub mint (owned). 10.1/10.2 the holder + tags (owned —
  over the chat stores). 2.6 replay skeleton (owned). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D13 (crash between message persist and event emit → BOTH committed or NEITHER; 0 orphan messages, 0
    phantom events) — CI; the outbox-depth + consumer-lag telemetry (1.8) is the green artifact (0 orphan/phantom).
  - CHAT-D14 (retry a send with the same client_nonce → exactly ONE message) — CI; the message-count signal = 1.
  - CHAT-D2 (burst sends + edits to one hot channel from many gateways → per-conversation total order preserved;
    ULID + aggregate=conversation_id; out-of-order client ops reconcile) — SCHED; the ordering-violation signal = 0.
  - STOR-D1/D2 (the permanent restore-verify gate) re-confirmed green on the chat message store (RPO ≤ 5 min /
    RTO ≤ 1h-tenant; 0 loss) — SCHED; this prompt does NOT call done over a red STOR-D1.
  - The no-untagged-personal-data lint green on the chat schema (0 untagged PII fields) — CI.
- **TESTS (required).** Unit tests for: the MessageStore trait (append/range/tombstone/resync_from), the ULID
  order, the idempotent-send uniqueness, the per-subject-DEK round-trip. The CDC provider/consumer pair for rows
  2.2, 2.3, 11.4, 5.7, 2.6. The drill-harness scenarios for CHAT-D13, CHAT-D14, CHAT-D2 (each a scenario on the
  failure-injection harness asserting against the named survival signals). Prefer a CHAINED mutation test
  (send → crash mid-emit → recover → assert exactly-once) over a single-handler test (EI-01 §4). State the
  cargo-mutants mutation-score floor for the co-commit + idempotent-send core modules (mandatory-core).
- **DEFINITION OF DONE.** The MessageStore + outbox co-commit + idempotent send exist and compile; per-subject-DEK
  bodies and the holder are wired; CHAT-D13/D14/D2 each emit a dated green artifact (PROVEN, not CLAIMED); STOR-D1
  re-confirmed green on the chat store; the no-untagged-personal-data lint green; the unit + CDC + drill tests
  pass; the contract-coverage scanner is green; the ScyllaDB and replay-parity floors are named with their
  follow-ons (CHAT-P13, CHAT-P9); the work is committed. A red gate becomes a dated claimed-not-proven row, never
  a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat durable message store + outbox co-commit + idempotent send. Body lists:
  2.2/2.3 co-commit + per-conversation order, 11.4 per-subject-DEK bodies, 5.7 #sub mint, 2.6 replay skeleton;
  CHAT-D13/D14/D2 greened with their measured numbers (0 orphan/phantom, 1 message, 0 ordering violations);
  STOR-D1 re-confirmed; the ScyllaDB hot-tier floor + the replay-parity floor named with follow-ons. Branch first
  if on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CHAT-P3 — The Conversation / Membership service + membership→write_tuples→zookie in one transaction

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C1 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C1"), the Conversation/Membership
  slice (the second committable unit of M4-C1; the message-store slice is CHAT-P2).
- **DEPENDS-ON.** CHAT-P2 (the message store + outbox co-commit + the chat crate). The M1 Identity prompts that
  ship write_tuples / zookie (4.6/4.10), check + CaveatContext (4.2), and the ReBAC engine accepting the Chat
  fragment (4.9, declared in CHAT-P1). The M0 outbox (2.2). The M1 Tenancy partition (12.1).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe; world-scale; one permission model); ../../external-insights/01-process-and-
    quality-doctrine.md §3 (prove-it — the new-enemy zookie guard is a drilled property), §7 (one primitive, no
    third copy of permission-aware reads).
  - Architecture: ../04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md (the Conversation
    entity + kinds, the membership table, the membership_by_principal index for the conversation list, the
    retention_days / linked_ref fields); 03-events-contracts-and-glue.md §5 (the ReBAC fragment + watcher
    relation; membership writes project tuples via write_tuples in the SAME tx as the membership row + the
    chat.channel.member_* event, stamping the returned zookie — the new-enemy guard); §1.1 (the
    chat.channel.created/archived/member_added/member_removed/linked durable events).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen Chat
    ReBAC fragment: channel.read = member + parent_project->read; the watcher relation).
  - Contracts: contract-index.md rows 4.6 (write_tuples([Δtuple], precondition?) → zookie; atomic; emitted via
    outbox), 4.10 (Consistency/zookie semantics — read-your-writes; the new-enemy guard; zookie-stamped reads
    bypass fail-static), 4.9 (the Chat ReBAC fragment), 4.2 (check + CaveatContext — the membership/send gate),
    2.2 (the outbox co-commit), 12.1 (the partition key).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C1 membership work) + §3 (the ReBAC-fragment-
    writes row: M2-C0 declare → M4-C1 writes) + §2 (upstream rows 4.6/4.10, 4.9, 4.2).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — the new-enemy / membership-revoke
    correctness is exercised by CHAT-D11 (search-as-non-member, CHAT-P9) and CHAT-D5 (unfurl no-leak, CHAT-P6);
    here the gate is the zookie-stamp-in-tx compile + the membership unit drill.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat:
  - The Conversation / Membership Service: one Conversation entity with kinds (channel / dm / thread-host as the
    arch defines); the membership table; the membership_by_principal index that backs the conversation list (S1).
  - Membership change → write_tuples([Δtuple], precondition) → zookie in the SAME PG transaction as the membership
    row + the chat.channel.member_added / member_removed event (via OutboxTx::emit), STAMPING the returned zookie
    on the conversation (conversation.acl_zookie) — the new-enemy guard (4.6/4.10): a just-revoked grant cannot
    read stale on the next unfurl/read.
  - The send / edit / membership permission gate via Id.check(subject, permission, object, zookie?, caveat?)
    (4.2) — every send and membership mutation is gated; the gate is fail-closed.
  - chat.channel.created / archived / linked durable events via the outbox; chat.channel.linked → refs.edge.created
    ("discussed in").
  - FLOOR named: none new — this completes the M4-C1 silent-data-loss floor's membership half. State that the
    cross-org / federated channels follow-on (M5-C-X1 / CHAT-P13) rides the cross-cell bridge (12.6) and is
    designed-not-built; the Conversation model here must not foreclose it (single home-cell is the M4 floor).
- **CONTRACTS TO IMPLEMENT.** 4.6 write_tuples → zookie (consumed — membership tuple write in the same tx). 4.10
  the zookie new-enemy stamp (consumed — stamped on the conversation). 4.9 the Chat fragment (owned — the runtime
  membership writes against the declared fragment). 4.2 check (consumed — the send/membership gate). 2.2 the
  outbox co-commit (consumed — the member_* events). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The membership write commits the row + the write_tuples zookie + the chat.channel.member_* event in ONE tx
    (atomicity gate: kill between the membership row and the tuple write → neither commits; 0 partial membership)
    — CI; the outbox-depth + dedup telemetry is the green artifact.
  - The new-enemy guard: revoke membership, immediately attempt a read → the zookie-stamped read denies (0 stale
    grants readable post-revoke) — CI; the cross-tenant/stale-grant signal = 0. (The full leak proof is CHAT-D5 in
    CHAT-P6 and CHAT-D11 in CHAT-P9, which depend on this stamp.)
  - The Chat ReBAC fragment runtime writes resolve channel.read = member + parent_project->read correctly (a
    non-member denied; a parent-project reader allowed) — CI.
- **TESTS (required).** Unit tests for: the membership-write-in-one-tx atomicity, the zookie stamp, the
  channel.read = member + parent_project->read resolution. The CDC provider/consumer pair for rows 4.6, 4.10, 4.9.
  A CHAINED test (add member → revoke → read) proving the new-enemy guard, not a single-handler test (EI-01 §4).
  State the cargo-mutants mutation floor for the membership-tx + zookie-stamp core module (mandatory-core).
- **DEFINITION OF DONE.** The Conversation/Membership service exists and compiles; membership→write_tuples→zookie
  is one transaction with the member_* event; the new-enemy guard denies post-revoke; the Chat fragment resolves
  correctly; the unit + CDC + chained tests pass; the contract-coverage scanner is green; the cross-org floor is
  named (CHAT-P13); all lints green; the work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat Conversation/Membership service + membership→write_tuples→zookie. Body
  lists: 4.6/4.10 tuple-write + zookie in one tx, 4.9 fragment runtime writes, 4.2 send/membership gate; the
  membership atomicity + new-enemy guard greened (0 partial membership, 0 stale grants); the cross-org floor named
  (CHAT-P13). Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P4 — The firehose resume-cursor transport + the Rust connection-tier gateway (the zero-loss-across-reconnect backbone)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C2 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C2 — The firehose resume-cursor
  transport + the connection-tier gateway").
- **DEPENDS-ON.** CHAT-P2 (the durable store + outbox the firehose reads from). The M2 Bus prompts that ship the
  firehose transport + the resume-cursor protocol (3.5) GREEN. The M0 substrate prompts that ship the
  protected-human-lane shed order (1.11), the cross-language harness shim (1.7), and liveness≠readiness (1.3).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; agent-native — humans never queue behind agent runs);
    ../../external-insights/04-hard-problems.md §2 (build the durable resume-cursor transport FIRST — a relay
    without resume cursors silently loses the gap on a reconnect; §2.2 the zero-loss-across-reconnect property);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — sever the gateway↔firehose and watch
    resume recover the gap), §2 (this is the silent-loss floor for the live tier).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §1–2 (the live
    delivery / resume / resync_required → snapshot path), §7.2 (presence), §7 (typing/read-state/streaming over
    the firehose); 01-tech-and-data-model.md (the stateless gateway: live sockets + presence + resume cursors
    only, NO durable store, NO outbox of its own); 03-events-contracts-and-glue.md §1.2 (the firehose-only set;
    the no-raw-publish + firehose-seam structural separation), §9 (the gateway has no emit path — it calls the
    Message Service); 00-overview.md (the TE-21 connection-tier divergence call).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-J (the firehose
    resume-cursor protocol — subscribe(stream, scope, cursor?), resume(stream, scope, last_seq) backfills
    (last_seq, now], resync_required → *.snapshot; scope is a bounded selector, never *), §OQ-K (the per-surface
    shed budgets), ADR-16 (the protected-human-lane shed order).
  - Contracts: contract-index.md rows 3.5 (the firehose transport + the resume-cursor subscription protocol —
    chat's correctness backbone), 1.11 (the protected-human-lane shed order + per-surface shed budget floors),
    1.7 (the cross-language harness shim — the BEAM hatch bounded by it, a no-op in Rust), 1.3 (liveness ≠
    readiness — the gateway readiness-gates new connections), 2.6 (resync_from = MessageStore::resync_from, the
    snapshot fallback), 1.2/1.1 (the three-surface harness the gateway boots from).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C2 work, entry, exit, the Rust/BEAM floor + the
    mega-channel home-node floor) + §1 (non-negotiability item 1b: lost across a gateway reconnect) + §6 (the
    first-runnable bar = end of M4-C2) + §5 (the connection-tier-language floor, the TE-21 build-gate).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D1 (sever gateway↔firehose →
    resume 0 lost/0 dup; last_seq past window → resync_required → snapshot still 0 lost), CHAT-D4 (roll the
    gateway fleet under a connection storm → bounded reconnect; resume completes for all; no loss; readiness gates;
    liveness no restart-storm). CHAT-D4 is the TE-21 build-gate drill.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the gateway sub-crate,
  myelin-chat-gateway, Rust default per TE-21):
  - The connection-tier gateway: WS/SSE termination; STATELESS (live sockets + presence + resume cursors only; NO
    durable store, NO outbox of its own — it calls the Message Service for any write). Boots from serve(AppSpec)
    (1.1) with the three surfaces (1.2) and liveness≠readiness (1.3).
  - subscribe(stream, scope=channel:<id>, cursor?) — bounded, never * (paginated for hot channels);
    resume(stream, scope, last_seq) recovering the gap (last_seq, now]; resync_required → *.snapshot fallback
    (= MessageStore::resync_from) when last_seq exceeds the retention window.
  - Live message delivery, presence, typing, fine-grained read-state, streaming partials — FIREHOSE-ONLY, never
    the durable bus (the no-raw-publish lint + the firehose seam keep them off structurally).
  - The protected-human-lane shed order (ADR-16, 1.11) + the per-surface shed-budget FLOOR (OQ-K): speculative /
    presence shed first, message delivery last; humans never queue behind agent runs. The concrete shed-budget
    numbers are a named floor (tuned by CHAT-D3/D4 in M5-C-S1 / CHAT-P12).
  - The cross-language harness shim (1.7) satisfied as a no-op (Rust); the gateway speaks the Rust EventEnvelope
    on the wire regardless.
  - FLOOR named: connection-tier language = Rust; the BEAM/Phoenix hatch is written-but-closed, bounded by 1.7,
    opened only if CHAT-D3/D4 prove Rust presence-at-scale intractable (a gateway-process swap, not a platform
    rewrite). Mega-channel live delivery = firehose subject fan-out with per-view scope bounding; the
    channel-sharded home-node is the named M5 escalation (M5-C-S3 / CHAT-P13). Per-surface shed budgets = named
    floor numbers, tuned in CHAT-P12. Name each follow-on prompt.
- **CONTRACTS TO IMPLEMENT.** 3.5 the resume-cursor protocol (consumed — chat's live tier subscribes/resumes over
  it with scope=channel:<id>). 1.11 the shed order + per-surface budget floor (owned — chat's connection-storm +
  agent-mention-storm budgets). 1.7 the harness shim (consumed — no-op). 2.6 resync_from (consumed — the snapshot
  fallback). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D1 (sever the gateway↔firehose mid-publish → resume recovers the gap, 0 lost / 0 dup; last_seq past the
    window → resync_required → *.snapshot, still 0 lost) — CI; the consumer-lag + lost-frame telemetry (1.8) is
    the green artifact (0 lost, 0 dup).
  - CHAT-D4 (roll the gateway fleet under a connection storm → bounded reconnect rate; resume completes for ALL;
    no message loss; readiness gates new connections; liveness no restart-storm) — SCHED; the reconnect-rate +
    shed-count signals are the green artifact. This is the TE-21 build-gate drill.
- **TESTS (required).** Unit tests for: subscribe scope-bounding (never *), resume gap recovery, the
  resync_required → snapshot fallback, the shed-order priority. The CDC provider/consumer pair for rows 3.5, 1.11,
  2.6. The drill-harness scenarios for CHAT-D1 and CHAT-D4 (each a scenario on the failure-injection harness). A
  CHAINED test (subscribe → deliver → sever → resume → assert 0 lost/0 dup). If the gateway diverges to BEAM
  (it does not in the default), the 1.7 shim's test obligations stand in for cargo-mutants; in the Rust default
  state the mutation-score floor for the resume-cursor core module (mandatory-core).
- **DEFINITION OF DONE.** The stateless gateway + the resume-cursor live tier exist and compile; CHAT-D1 and
  CHAT-D4 each emit a dated green artifact (0 lost / 0 dup; bounded reconnect); the shed order holds (humans last);
  the firehose-only events never touch the durable bus (no-raw-publish green); the unit + CDC + drill tests pass;
  the Rust/BEAM, mega-channel, and shed-budget floors are named with their follow-ons (CHAT-P12, CHAT-P13); the
  work is committed. This is the first-runnable bar (§6) — a message persists with its event (CHAT-P2/D13) and is
  delivered live with zero loss across a reconnect (CHAT-D1). No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat firehose resume-cursor transport + Rust connection-tier gateway. Body
  lists: 3.5 subscribe/resume/scope, 1.11 shed order + budget floor, 2.6 snapshot fallback; CHAT-D1/D4 greened
  with measured numbers (0 lost/0 dup, bounded reconnect); the Rust/BEAM hatch, mega-channel home-node, and
  shed-budget floors named with follow-ons. Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### CHAT-P5 — The composer + message content over the frozen myelin-content Chat subset (per-message CAS; render(parse(md))===md)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C3 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C3 — The composer + message
  content over the frozen myelin-content subset").
- **DEPENDS-ON.** CHAT-P2 (the message store the composer writes to) + CHAT-P4 (the live delivery for optimistic
  send). The M2 prompts that freeze myelin-content (13.1) + the WASM render core. The M2 Search prompts (the
  @-mention / #-artifact autocomplete is Search-backed). The M2 Refs prompts (the three inline ref nodes produce
  refs.edge.created, 5.4).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (design comes before implementation for anything with a frontend — no frontend code
    without a reviewed design sketch; the wireframes are PRESERVED from Phase 4 and are the build-to);
    ../../external-insights/05-ux-and-design.md (the design-language bar); ../../external-insights/01-process-and-
    quality-doctrine.md §4 (actually try it — drive the composer in a browser before claiming it), §3 (the
    render(parse(md))===md round-trip is a quantified gate).
  - Architecture: ../04-subsystem-architectures/chat/design/wireframes.md (S3 composer — the build-to; with
    empty/loading/error states) + design/user-flows.md + design/information-architecture.md;
    ../04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md §1.4 (the body = markdown-subset
    string + the three structured nodes; the per-message edited_seq CAS); 04-views-cli-and-api.md §1 (S3 the
    composer; the one editor render path); 03-events-contracts-and-glue.md §1.1 (chat.message.edited carries the
    new edited_seq).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-2 (the frozen
    myelin-content taxonomy + the WASM compile target; Chat consumes a strict SUBSET).
  - Contracts: contract-index.md row 13.1 (the myelin-content taxonomy frozen — the Chat subset: paragraph,
    heading(1..3), bullet_list, ordered_list, task_list, blockquote, code_block, callout, table, divider, image +
    the three inline nodes mention/artifact_ref/embed; EXCLUDES db_view, sync_block, toggle; render(parse(md))===md;
    the WASM render core), 5.4 (refs.edge.created — the three inline ref nodes are the producers).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C3 work + exit) + §5 (the per-message-CAS floor:
    chat does NOT promote to CRDT — that is Knowledge's) + §6 (first-useful progression).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — the chat instance of KN-D2 (the
    content-core round-trip gate); the refs.edge.created uniformity check.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the composer + content
  modules) and the chat frontend package (the web app, built to the PRESERVED S3 wireframe):
  - The composer + the message body over the frozen Chat SUBSET of myelin-content (13.1), REUSING the WASM-compiled
    Rust content core (one editor render path; render(parse(md)) === md). The Chat subset is paragraph,
    heading(1..3), bullet_list, ordered_list, task_list, blockquote, code_block, callout, table, divider, image +
    the three inline nodes mention / artifact_ref / embed; it EXCLUDES db_view, sync_block, toggle.
  - Per-message CAS on edit (edited_seq) — NO collaborative-edit engine (chat is single-author-per-message; the
    CRDT is Knowledge's, not chat's). A stale edit (edited_seq mismatch) is rejected with current state.
  - The / slash menu, @-mention + #-artifact autocomplete (Search-backed via the M2 Search query surface),
    paste-URL → unfurl, draft persistence (the draft store from CHAT-P2, per-subject-DEK encrypted).
  - The structured mention / artifact_ref / embed nodes parse to the frozen node shapes and produce
    refs.edge.created uniformly (5.4).
  - FLOOR named: per-message CAS (single-author; no merge) — chat does NOT promote to CRDT (n/a follow-on; the
    related OQ-L comment-threading consolidation is M5-C-X2 / CHAT-P13, not a CRDT). State this explicitly so the
    next agent does not build a chat CRDT.
- **CONTRACTS TO IMPLEMENT.** 13.1 the myelin-content Chat subset + the WASM render core (consumed — the composer
  reuses the one render path; render(parse(md))===md). 5.4 refs.edge.created (owned — the chat inline nodes emit
  edges uniformly). Implement to the frozen shapes; chat must not add a node outside the frozen subset (a needed
  change is a whole-workspace contract PR, escalated).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - render(parse(md)) === md holds 100% for the Chat subset (the chat instance of KN-D2 / the content-core
    round-trip gate) — CI; the round-trip-mismatch signal = 0 over the corpus.
  - Structured mention / artifact_ref / embed nodes parse to the frozen node shapes and produce refs.edge.created
    uniformly (0 nodes producing a malformed or missing edge) — CI.
  - The per-message CAS rejects a stale edit (edited_seq mismatch → rejected with current state; 0 silent
    overwrite of a message) — CI.
- **TESTS (required).** Unit tests for: the Chat-subset parse/render round-trip, the three inline nodes →
  refs.edge.created, the edited_seq CAS rejection. A browser-driven check of the S3 composer (the / slash menu,
  the @/# autocomplete, paste-URL→unfurl) per EI-01 §4 — record yes/no/partial honestly. The CDC pair for 13.1
  (the Chat subset) and 5.4. State the cargo-mutants mutation floor for the content-subset parse module if
  mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The composer + message body over the frozen subset exist and compile; render(parse(md))
  === md is green 100% for the Chat subset; the inline nodes produce edges uniformly; the per-message CAS rejects
  stale edits; the S3 composer is driven in a browser (yes/no/partial recorded); the unit + CDC tests pass; the
  no-chat-CRDT floor is named; all lints green; the work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat composer over the frozen myelin-content subset + per-message CAS. Body
  lists: 13.1 the Chat subset + WASM render core, 5.4 inline nodes → edges; render(parse(md))===md greened 100%;
  the per-message-CAS-no-CRDT floor named; the S3 composer browser-driven (yes/no/partial). Branch first if on
  default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P6 — The unfurl service: cheap per-viewer permission-aware unfurls + project(ref, viewer) (the wedge differentiator)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C4 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C4 — The unfurl service: cheap
  per-viewer permission-aware unfurls").
- **DEPENDS-ON.** CHAT-P3 (membership + the zookie stamp the unfurl permission reads) + CHAT-P4 (the live firehose
  for card busting) + CHAT-P5 (the artifact_ref/embed nodes that produce the refs). The M2 Refs prompts that ship
  resolve(ref, viewer, mode) + the 4-step tombstone ladder (5.7) + project REQUIRED (5.6) + refs.edge.created
  (5.4), GREEN. The M1 Identity prompt that ships list_objects with the SetExpr push-down (4.3), GREEN. The M3 Git
  + Knowledge prompts producing the artifacts to unfurl (commits/PRs + project; docs/pages + project). CI's
  ci.check.updated producer (5.9, lands in M4, CI sequenced first within M4) for unfurl invalidation. Issues'
  project for issue unfurls (parallel in M4).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (chat references any other artifact — the differentiator) §3 (top-of-the-line UX);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the no-leak drill forces a
    confidential title to a viewer lacking access and watches the tombstone), §7 (abstract at the third copy — the
    unfurl reuses the Refs resolve chokepoint, chat never re-implements permission-aware resolution).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §4 (the Unfurl
    Service — the shared per-ArtifactRef projection cache viewer-independent, gated by a per-viewer
    list_objects/check; lazy-on-viewport; the bus-driven invalidation; ONE cache entry per ref, never per (ref,
    viewer)); 03-events-contracts-and-glue.md §2 (the #sub ladder outcomes for chat: live/gone/erased), §3
    (project(ref, viewer) — the per-viewer pre-permission-checked projection, never the body), §1.3 (the
    unfurl-invalidation consumer matching *.updated / *.erased / ci.check.updated); 05-hard-problems.md §4 (the
    no-leak subtlety that separates a real implementation from a demo).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-E (the
    list_objects SetExpr push-down lowering to a JOIN over the candidate id column), §X-4 (the 4-step tombstone
    ladder), §OQ-I (cross-cell resolution is always cell-local).
  - Contracts: contract-index.md rows 5.2 (resolve(ref, viewer, mode) → Projection | Tombstone; cell-local), 5.7
    (the 4-step tombstone ladder), 5.6 (project REQUIRED on every subsystem — chat implements it for
    chat/{channel,message,thread}), 4.3 (list_objects SetExpr — the membership-class precompute + the no-N+1
    candidate filter), 4.2 (check — the per-viewer gate), 5.4 (refs.edge.created — chat is the densest producer),
    5.9 (ci.check.updated — the unfurl-invalidation event), 3.5 (the firehose for the live card update).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C4 work, entry, exit) + §1 (non-negotiability item
    2: PII leak through unfurls) + §6 (first-useful) + §5 (the canvas = embedded Knowledge page floor, M4-C4).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D5 (confidential unfurl →
    tombstone, title never present — ladder step 1), CHAT-D6 (erase a third party in a card → tombstone on next
    render, 0 recoverable PII, no durable snapshot, re-resolves live → erased), CHAT-D7 (an artifact's
    ci.check.updated / *.updated → the shared per-ref cache busts; viewers showing the card get a live firehose
    update within budget), CHAT-D18 (edit a referenced message → the message-<id> anchor stays stable/live; delete
    → embed degrades to Tombstone carrying the root, never dangles).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the unfurl module):
  - The Unfurl Service: a shared, per-ArtifactRef projection cache (viewer-INDEPENDENT content — ONE cache entry
    per ref, never per (ref, viewer)), gated by a per-viewer list_objects / check (lowering the frozen SetExpr to
    a JOIN over the candidate id column) — no leak.
  - Lazy-on-viewport resolution (resolve only what is on screen); calls Refs resolve(ref, viewer, mode) over the
    one 4-step tombstone ladder (5.7). For chat refs the ladder outcomes are live / gone / erased (a message is
    content-addressed by stable id, no moved/outdated).
  - Membership-as-permission class precompute via the frozen list_objects Filter (one class decision, not N).
  - Bus-driven invalidation on *.updated / ci.check.updated / *.erased pointer events (precise; TTL the backstop);
    viewers currently showing the card get a live firehose update.
  - project(ref, viewer) for chat/{channel,message,thread} (5.6) — the ONLY way other subsystems read about a chat
    artifact (no cross-DB); per-viewer pre-permission-checked → Projection | Tombstone (never the body); title
    humanised via humanise (7.3).
  - Chat as the densest refs.edge.created producer (artifact-linked channels, embeds, mentions) — wired from the
    CHAT-P5 inline nodes + the chat.channel.linked event.
  - FLOOR named: the canvas = an embedded/pinned Knowledge page (ArtifactRef, not a Chat editor) — the joint
    Chat↔Knowledge review of the pin/embed mechanism is M4/M5 (M5-C-X2-adjacent); the lean is firm: embed, not
    editor. State this so no agent builds a chat-side canvas editor.
- **CONTRACTS TO IMPLEMENT.** 5.6 project(ref, viewer) for chat artifacts (owned). 5.2/5.7 resolve + the ladder
  (consumed — the unfurl chokepoint). 4.3 list_objects SetExpr (consumed — the per-viewer gate + the membership
  class precompute, lowered to a JOIN). 4.2 check (consumed). 5.4 refs.edge.created (owned — chat the densest
  producer). 5.9 ci.check.updated (consumed — unfurl invalidation only). Implement to the frozen shapes; no local
  divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D5 (notify/unfurl a confidential artifact to a viewer lacking access → tombstone rendered, title NEVER
    present — the 4-step ladder step 1) — CI; the leaked-title signal = 0.
  - CHAT-D6 (erase a third party rendered in a card → tombstone on next render, 0 recoverable PII; NO durable
    snapshot; the cache re-resolves live → erased) — CI; the recoverable-PII-in-cache signal = 0.
  - CHAT-D7 (an artifact's ci.check.updated / *.updated → the shared per-ref cache busts; viewers showing the card
    get a live firehose update within budget) — CI; the cache-staleness + update-latency signals within budget.
  - CHAT-D18 (edit a referenced message → message-<id> anchor stays stable/live; delete → embed degrades to a
    Tombstone carrying the root, never dangles) — CI; the dangling-anchor signal = 0.
- **TESTS (required).** Unit tests for: the one-cache-entry-per-ref (never per (ref, viewer)) invariant, the
  per-viewer gate via list_objects SetExpr → JOIN, the 4-step ladder outcomes (live/gone/erased), project()
  returning Projection|Tombstone never the body. The CDC pair for 5.6, 5.2, 5.7, 4.3, 5.9. The drill-harness
  scenarios for CHAT-D5/D6/D7/D18. A CHAINED test (resolve as member → revoke / erase → re-resolve → assert
  tombstone, 0 leak) per EI-01 §4. State the cargo-mutants mutation floor for the per-viewer-gate core module
  (mandatory-core — the no-leak property).
- **DEFINITION OF DONE.** The Unfurl Service + project() exist and compile; CHAT-D5/D6/D7/D18 each emit a dated
  green artifact (0 leaked title, 0 recoverable PII, live bust within budget, 0 dangling anchor); the cache is
  one-entry-per-ref with no leak; the unit + CDC + drill tests pass; the contract-coverage scanner is green; the
  canvas-is-an-embed floor is named; all lints green; the work is committed. No gate greened by a weakened
  threshold or an inverted assertion.
- **COMMIT.** Header: P-<NNN> M4: Chat unfurl service + project(ref, viewer) (the wedge differentiator). Body
  lists: 5.6 project, 5.2/5.7 the resolve chokepoint + ladder, 4.3 the SetExpr JOIN gate, 5.4 chat the densest
  edge producer, 5.9 unfurl invalidation; CHAT-D5/D6/D7/D18 greened with measured numbers (0 leak, 0 recoverable
  PII, live bust within budget, 0 dangling); the canvas-is-an-embed floor named. Branch first if on default; do
  not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P7 — The read-state hot path (Valkey+PG, cache-never-authoritative) + the fanout-class boundary + Activity-as-view

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C5 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C5 — The read-state hot path +
  Activity-as-view").
- **DEPENDS-ON.** CHAT-P2 (the conversation log unread derives from) + CHAT-P4 (the firehose for
  read_state.updated). The M2 Notif prompts that ship list_inbox (7.1), read-state truth (7.2), humanise (7.3),
  define_notif_rule (7.6), GREEN. The M1 Identity prompt that ships list_subjects against the authz reverse index
  (4.4) for watcher resolution at 50k-member density.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale — the celebrity-fanout mitigation; one inbox);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — drop Valkey and watch PG be
    authoritative), §7 (Activity is a VIEW into the one inbox, never a second store — no third copy of read-state).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §5 (the
    Read-state Service — Valkey hot markers + counters; batched eventually-consistent flush to the PG durable
    record; Valkey NEVER authoritative; unread as a bounded range read count(id > last_read); firehose-only
    chat.read_state.updated); §5.3 (Activity / Mentions = a list_inbox filter, never a 2nd store);
    03-events-contracts-and-glue.md §4 (the fanout boundary chat owns — write-fanout the bounded high-signal set,
    read-fanout the unbounded ambient set; the celebrity-fanout mitigation — a 100k-member post does ZERO
    per-member inbox writes); 04-views-cli-and-api.md §1 (S6 Activity = Notif.list_inbox(filter)).
  - Contracts: contract-index.md rows 7.1 (list_inbox — the ONE inbox; Activity is a filter), 7.2 (read-state
    truth), 7.6 (define_notif_rule — mentioned/replied/thread_watched/approval_requested), 4.4
    (list_subjects(channel, watcher) against the authz reverse index, performant at 50k-member density), 3.5 (the
    firehose-only read_state.updated), 10.1 (the read-state store is a PersonalDataHolder).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the watcher
    relation; list_subjects density).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C5 work + exit) + §3 (the fanout-class declaration
    row; humanise/notif-rule wire) + §6 (first-useful) + §5 (the read-state batched-flush cadence tunable R-C3).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D12 (flush + drop Valkey
    mid-session → the PG record is authoritative; a marker is at-worst slightly stale; unread counts recompute
    correctly).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the read-state + activity
  modules):
  - The Read-state Service: Valkey hot markers + counters; batched eventually-consistent flush to the PG durable
    record (Valkey NEVER authoritative); unread derived as a bounded range read (count(id > last_read)), never
    write-fanned-out; firehose-only chat.read_state.updated events; the store registered as a PersonalDataHolder.
  - The fanout-class declaration (arch 03 §4): WRITE-FANOUT the bounded high-signal set (mentions via the
    structured mention(Principal) node, DMs, thread-replies-to-you, HITL-for-you, keyword matches) → Signals →
    Notif; READ-FANOUT the unbounded ambient set (channel/thread activity, unread) via the per-conversation log +
    lazy unread, watchers resolved by list_subjects(channel, watcher) (4.4). A 100k-member announcement does ZERO
    per-member inbox writes on a post (the celebrity-fanout mitigation).
  - Activity / Mentions (S6) = a list_inbox filter (subsystem ∈ {chat} ∧ reason ∈ {mentioned, replied,
    thread_watched, approval_requested}) — NEVER a second store; one read-state truth, linked to chat's
    scroll-state at the mention.
  - FLOOR named: none new — the read-state batched-flush cadence + the Notif.mark(item, read) trigger are
    measured-not-predicted tunables (R-C3), tuned against telemetry, not a separate milestone. State this.
- **CONTRACTS TO IMPLEMENT.** 7.1 list_inbox as the Activity filter (consumed — Activity is a view). 7.2
  read-state truth (consumed). 7.6 define_notif_rule (consumed — the wire of the M2-C0-declared rules). 4.4
  list_subjects(channel, watcher) (consumed — read-fanout watcher resolution). 3.5 firehose read_state.updated
  (consumed). 10.1 the holder (owned — over the read-state store). Implement to the frozen shapes; no local
  divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D12 (flush + drop Valkey mid-session → the PG record is authoritative; a marker is at-worst slightly
    stale; unread counts recompute correctly) — CI; the lost-read-state signal = 0 (PG authoritative).
  - The celebrity-fanout property: a 100k-member channel post does ZERO per-member inbox writes (the write-fanout
    counter on an ambient post = 0) — CI; the per-member-write signal = 0 for ambient.
  - Activity is a list_inbox filter, not a second store (0 chat-private activity store) — CI (a lint/structural
    check that no second read-state store exists).
- **TESTS (required).** Unit tests for: the Valkey-never-authoritative flush, the unread bounded-range-read, the
  write-fanout-vs-read-fanout class decision, Activity = a list_inbox filter. The CDC pair for 7.1, 7.2, 4.4. The
  drill-harness scenario for CHAT-D12. A test proving a 100k-member ambient post does 0 per-member writes. State
  the cargo-mutants mutation floor for the fanout-class core module if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The Read-state Service + the fanout boundary + Activity-as-view exist and compile;
  CHAT-D12 emits a dated green artifact (PG authoritative, counts recompute); the celebrity-fanout property holds
  (0 per-member writes on an ambient post); Activity is a filter not a store; the unit + CDC + drill tests pass;
  the read-state-cadence tunable is named; all lints green; the work is committed. No gate greened by a weakened
  threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat read-state hot path (Valkey+PG) + fanout boundary + Activity-as-view. Body
  lists: 7.1/7.2 the inbox + read-state truth, 7.6 notif rules wired, 4.4 watcher resolution; CHAT-D12 greened
  (PG authoritative); the celebrity-fanout 0-per-member-write property proven; the flush-cadence tunable named.
  Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P8 — The HITL approval-card bridge (per-effect idem_key) + the agent ToolDef set (frozen X-6 defaults, routed through EffectApi)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C6 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C6 — The HITL approval-card bridge
  + the agent ToolDef set").
- **DEPENDS-ON.** CHAT-P3 (the check gate) + CHAT-P5 (the card renders in a message) + CHAT-P7 (the card renders
  in the inbox). The M2 Workflow prompts that ship DurableExecutor::signal + the per-effect idem_key + the durable
  signal for multi-day HITL (9.1/9.4), GREEN. The M2 Agent prompts that ship EffectApi::apply plan-then-apply
  (8.2), ToolSurface::register_tool + the frozen requires_approval defaults (8.1, X-6), and the four uniform
  sandbox guarantees (8.4), GREEN. The M1 Storage prompt that ships reserve/settle (11.7). The M2 Notif humanise
  (7.3) for the card strings. The M1 Identity mint_run_token / revoke (4.7) for the resume token.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native; the strategy pattern at the agent plug-in; HITL where security/cost/
    irreversible-scope implications exist); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it — a gated effect that mutates before approval, or runs twice across a kill, is the failure the drill
    forces), §8 (the human sign-off is the bottleneck — the approval card IS the decision surface); §4 (chain the
    mutations — request → kill → approve days later → exactly-once).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §5 (the HITL Card
    Service — render the card in thread + Notif inbox; gate the click with check(human, approve, run); post
    DurableExecutor::signal with the per-effect idem_key; a declined effect is WITHHELD; timeout auto-denies;
    resume under a freshly-minted attenuated token); 03-events-contracts-and-glue.md §8 (the chat ToolDef set +
    the frozen requires_approval defaults; all side-effecting tools route through EffectApi, NEVER
    ToolHands::exec — the routing split is the safety boundary; the four uniform guarantees), §9 (reserve/settle —
    chat surfaces cost but never holds the wallet); 04-views-cli-and-api.md §1 (S11 the HITL card, S3).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-6 (the frozen
    requires_approval defaults — Chat post/react/reply = no, create_channel/invite/archive = yes, cross-subsystem
    inherits the target's default; the four uniform guarantees; the EffectApi-vs-ToolHands routing split), §OQ-F
    (the per-effect idem_key rule — card_id single, card_id:<effect_idx> multi/partial; a double-click is one
    approval, a partial approval is well-defined).
  - Contracts: contract-index.md rows 9.1 (DurableExecutor::signal idempotent on idem_key, the per-effect rule),
    9.4 (the durable signal for multi-day HITL), 8.1 (ToolSurface::register_tool + the frozen defaults), 8.2
    (EffectApi::apply plan-then-apply — schema→capability→delegation→tenant→budget→HITL→apply→meter; a withheld
    gated tool does not mutate), 8.4 (ToolHands::exec is for compute, not chat mutation — the routing split), 4.2
    (check(human, approve, run) — the approve gate), 4.7 (mint_run_token — the resume token), 11.7 (reserve/settle
    — fronts every spend-bearing agent post), 8.7 (run --dry-run → ProposedEffects without applying), 7.3
    (humanise — the card strings).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C6 work, entry, exit) + §1 (non-negotiability item
    4: HITL approval correctness) + §6 (first-useful = end of M4-C6) + §3 (the ToolDef + idem_key rows).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D9 (request approval, kill Chat
    + Workflow mid-wait, approve days later → the gated tool runs exactly once; double-click is one approval; deny
    withholds with no mutation; timeout auto-denies; resume under a fresh token), CHAT-D10 (a multi-effect card
    approved 2-of-3 → the 2 resume, the 1 withheld, each independent idem_key=card_id:<idx>; no effect runs twice;
    the withheld never mutates).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the HITL + tool modules):
  - The HITL Card Service: render the approval card (in thread + Notif inbox, the one inbox C-9); gate the click
    with Id.check(human, approve, run) (4.2); post DurableExecutor::signal(run, name, payload, idem_key) with the
    frozen per-effect idem_key (card_id single / card_id:<effect_idx> multi); a declined effect is WITHHELD (one
    EffectApi::apply per APPROVED effect); timeout auto-denies; resume runs under a freshly-minted attenuated token
    (4.7). Chat owns the CARD, not the wait/timer/budget/sandbox.
  - The chat ToolDef set (8.1, frozen X-6 defaults): chat.post / reply_in_thread / react / start_dm =
    requires_approval false; chat.create_channel / invite / archive_channel = true; any cross-subsystem effect
    INHERITS the TARGET subsystem's default. ALL side-effecting tools route through EffectApi (plan-then-apply,
    reserves), NEVER ToolHands::exec (the routing split is the safety boundary).
  - Reserve/settle on every spend-bearing agent post (11.7); chat surfaces cost (the card's live estimate) but
    never holds the wallet.
  - run --dry-run (8.7) on chat tools returns ProposedEffects without applying.
  - FLOOR named: none new — chat owns the card; the wait/timer/budget/sandbox are the M2 Workflow/Agent/Storage
    primitives. State that chat must not re-implement a wait or a budget.
- **CONTRACTS TO IMPLEMENT.** 9.1/9.4 DurableExecutor::signal + the per-effect idem_key + the durable HITL signal
  (consumed — the card posts the signal). 8.1 the chat ToolDef set + the frozen defaults (owned). 8.2 EffectApi
  routing (consumed — every chat mutation routes through it). 4.2 the approve gate (consumed). 4.7 mint_run_token
  (consumed — the resume token). 11.7 reserve/settle (consumed). 8.7 run --dry-run (owned — on chat tools). 7.3
  humanise (consumed — the card strings). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D9 (request approval, kill Chat + Workflow mid-wait, approve days later → the gated tool runs EXACTLY
    ONCE; double-click is one approval; deny withholds with NO mutation; timeout auto-denies; resume under a fresh
    token) — CI; the duplicate-apply signal = 0, the pre-approval-mutation signal = 0.
  - CHAT-D10 (a multi-effect card approved 2-of-3 → the 2 resume approved, the 1 withheld, each independent
    idem_key=card_id:<idx>; no effect runs twice; the withheld never mutates) — CI; the per-effect duplicate +
    withheld-mutation signals = 0.
  - The routing split: every side-effecting chat tool routes through EffectApi, NEVER ToolHands::exec (the
    no-host-exec lint + a structural check; 0 chat mutations via ToolHands) — CI.
- **TESTS (required).** Unit tests for: the per-effect idem_key (single vs multi), the withhold semantics (a
  declined effect never mutates), the double-click → one approval, the EffectApi routing (no ToolHands mutation).
  The CDC pair for 9.1, 9.4, 8.1, 8.2. The drill-harness scenarios for CHAT-D9 and CHAT-D10 — each a CHAINED
  scenario (request → kill → approve later → assert exactly-once) per EI-01 §4. State the cargo-mutants mutation
  floor for the idem_key + withhold core module (mandatory-core — the exactly-once HITL property).
- **DEFINITION OF DONE.** The HITL Card Service + the chat ToolDef set exist and compile; CHAT-D9/D10 each emit a
  dated green artifact (exactly-once, 0 pre-approval mutation, withheld never mutates); every chat mutation routes
  through EffectApi (no ToolHands mutation); reserve/settle fronts every spend-bearing post; the unit + CDC + drill
  tests pass; the contract-coverage scanner is green; the chat-owns-only-the-card boundary is stated; all lints
  green (incl. no-host-exec); the work is committed. This completes the first-useful bar (§6). No gate greened by
  a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat HITL approval-card bridge (per-effect idem_key) + agent ToolDef set. Body
  lists: 9.1/9.4 the durable signal + per-effect idem_key, 8.1 the frozen ToolDef defaults, 8.2 EffectApi routing,
  11.7 reserve/settle; CHAT-D9/D10 greened with measured numbers (exactly-once, 0 pre-approval mutation); the
  EffectApi-not-ToolHands routing split proven. Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### CHAT-P9 — ACL-filtered Search indexing + reindex-from-source parity (the embeddings-as-PII erasure-aware index)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C7 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C7 — Search indexing (ACL-filtered)
  + reindex-from-source parity").
- **DEPENDS-ON.** CHAT-P2 (the replay skeleton + the message source) + CHAT-P3 (membership for the ACL conjoin) +
  CHAT-P6 (the Refs read-model that rebuilds via replay). The M2 Search prompts that ship query/semantic always
  conjoining the list_objects Filter (6.1/6.2), declare_indexable (6.3), reindex (6.4), GREEN. The M1 Identity
  list_objects (4.3). The M1 Storage KMS / can_derive_plaintext_index (11.3) for the HYOK skip.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe — embeddings ARE personal data; one search ACL model);
    ../../external-insights/04-hard-problems.md §5 (reindex-from-source — Search is a derived store, never reads
    the owner DB; steady-state and recovery share one path), §1 (erasure reaches embeddings, never hides);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — search as a non-member returns 0
    results, the lint fails any query path without the Filter).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §6 (replay —
    the only recovery path; sub-artifact granular; erased subjects → tombstones; steady-state and recovery share
    one path), §7 (declare_indexable — the chat/message index spec; Search ALWAYS conjoins the frozen list_objects
    Filter over message.id; the search-as-non-member drill; embeddings erasure-aware; the HYOK skip);
    02-internals-and-algorithms.md §4.4 (the reindex consumer).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-E (the
    list_objects Filter conjoin lowering to a JOIN against the authz reverse index).
  - Contracts: contract-index.md rows 6.3 (declare_indexable(IndexSpec{subsystem:"chat", type:"message",
    ft_fields, struct_fields, semantic, acl_object_type:"message"})), 6.1 (query always conjoins the list_objects
    Filter before scoring — the search-requires-acl-filter lint), 6.2 (semantic ACL-filtered k-NN), 6.4 (reindex
    — the only rebuild path), 4.3 (list_objects SetExpr → JOIN), 2.6 (replay(scope, since) — full parity here),
    11.3 (can_derive_plaintext_index()=false structurally skips indexing for an HYOK tenant).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C7 work + exit) + §1 (non-negotiability item 2:
    search ACL) + §3 (the replay full-parity row M4-C7) + §6 (production-hardened).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D11 (search as a non-member →
    0 results from channels you're not in; the search-requires-acl-filter lint fails any query path reaching the
    index without the Filter conjoined), CHAT-D15 (wipe + replay(scope, since) → Search/Refs/Notif read-models
    rebuild; steady-state and recovery share one path; erased subjects → tombstones; reindex-parity hash matches).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the search-projection +
  replay modules):
  - declare_indexable(IndexSpec{subsystem:"chat", type:"message", ft_fields:["body"], struct_fields:["channel",
    "author", "thread_root", "created_at", "kind"], semantic: Some(EmbeddingSpec), acl_object_type:"message"})
    (6.3).
  - Search ALWAYS conjoins the frozen list_objects Filter over the message.id column before scoring (the
    search-requires-acl-filter lint, 6.1) — the SetExpr lowers to a JOIN against Id's authz reverse index; no
    N+1, no post-filter. The chat search-projection feeder emits the index spec; chat is never read directly.
  - Embeddings-as-personal-data: on erasure, Search PURGES + reindexes embeddings (not just FT) — never hides; an
    HYOK tenant whose can_derive_plaintext_index()=false structurally SKIPS message indexing (11.3).
  - replay(scope, since) FULL parity (completing the CHAT-P2 skeleton): Search/Refs/Notif read-models rebuild from
    chat.*.snapshot; steady-state and recovery share ONE path (the outbox → consumer template); erased subjects
    emit tombstones (no PII resurrected); a reindexing consumer composes the frozen list_objects Filter so a
    rebuild stays ACL-correct.
  - FLOOR named: none new — the embeddings erasure cascade's holder-completeness is proven in CHAT-P10 (M4-C8 /
    CHAT-D8). State that here the index is wired and ACL-correct; the full multi-holder erasure receipt is CHAT-P10.
- **CONTRACTS TO IMPLEMENT.** 6.3 declare_indexable (owned — the chat/message spec). 6.1/6.2 the ACL-conjoined
  query/semantic (consumed — chat's search feeder + the Filter conjoin). 6.4 reindex (consumed). 4.3 list_objects
  (consumed — the JOIN). 2.6 replay full parity (owned). 11.3 the HYOK skip (consumed). Implement to the frozen
  shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D11 (search as a non-member → 0 results from channels you're not in; the search-requires-acl-filter lint
    fails ANY query path reaching the index without the Filter conjoined) — CI; the non-member-result signal = 0;
    the lint signal = 0 unfiltered query paths.
  - CHAT-D15 (wipe + replay(scope, since) → Search/Refs/Notif read-models rebuild; steady-state and recovery share
    one path; erased subjects → tombstones; the reindex-parity hash matches the live hash) — SCHED; the
    reindex-parity-hash mismatch signal = 0.
  - The HYOK skip: a tenant with can_derive_plaintext_index()=false produces 0 indexed message bodies — CI; the
    HYOK-indexed-body signal = 0.
- **TESTS (required).** Unit tests for: the ACL-conjoined query (the Filter is always present), the embeddings
  purge-on-erasure, the HYOK skip, the steady-state-vs-recovery one-path identity. The CDC pair for 6.3, 6.1, 2.6.
  The drill-harness scenarios for CHAT-D11 and CHAT-D15 (CHAT-D15 a CHAINED wipe→replay→hash-compare). State the
  cargo-mutants mutation floor for the ACL-conjoin core module (mandatory-core — the no-leak search property).
- **DEFINITION OF DONE.** The chat search projection + the ACL conjoin + the full replay parity exist and compile;
  CHAT-D11 and CHAT-D15 each emit a dated green artifact (0 non-member results, reindex-parity hash matches); the
  HYOK skip produces 0 indexed bodies; the search-requires-acl-filter lint is green; the unit + CDC + drill tests
  pass; the contract-coverage scanner is green; the full-erasure-receipt follow-on is named (CHAT-P10); all lints
  green; the work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat ACL-filtered search indexing + reindex-from-source parity. Body lists: 6.3
  the chat/message index spec, 6.1 the Filter conjoin, 2.6 the full replay parity, 11.3 the HYOK skip; CHAT-D11/D15
  greened (0 non-member results, reindex-parity hash matches); the full-erasure-receipt follow-on named (CHAT-P10).
  Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P10 — The erasure cascade across every chat holder (crypto-shred bodies + pseudonym-shred mentions + embeddings + backups)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C8 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C8 — The erasure cascade across
  every chat holder").
- **DEPENDS-ON.** CHAT-P2 (the bodies/drafts stores) + CHAT-P6 (the unfurl cache) + CHAT-P7 (read-state) + CHAT-P9
  (the search/refs read-models incl. embeddings). The M1 GDPR prompts that ship the holder trait + the erasure
  ledger (10.1/10.8) + the no-untagged-personal-data lint (10.2) + the ONE erasure posture (10.9) + the DSR
  fan-out (10.4). The M1 Storage KMS per-subject DEK (11.3/11.4). The M1 Identity resolve_pseudonym / erase (4.8).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction — data subject erasure); ../../external-insights/04-hard-
    problems.md §1 (erasure-vs-immutability — the ONE posture: per-subject DEK crypto-shred + pseudonym-map shred
    + restrict; the third-party free-text residual handled BY REFERENCE, not restated);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — erase a person and watch 0
    recoverable PII across hot + cold + backups + embeddings), §1 (name-your-floors — the residual is a named
    LEGAL floor).
  - Architecture: ../04-subsystem-architectures/chat/architecture/05-hard-problems.md §5 (chat is the most
    PII-dense holder, the canonical GD-4 crypto-shred case); 03-events-contracts-and-glue.md §10 (the holder over
    every chat store; the restriction flag Art.18; the free-text residual BY REFERENCE to the ONE posture),
    §1.1 (chat.message.erased — the *.erased cross-cutting tombstone; the mention(Principal) → [erased user]);
    06-reconciliation-compliance.md (the holder list + the DSR cascade).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-7 (the ONE
    free-text/immutable erasure posture — instantiated per subsystem BY REFERENCE; the residual is one ratified
    statement, [OPEN — LEGAL]).
  - Contracts: contract-index.md rows 10.1 (PersonalDataHolder{locate, export, rectify, restrict, erase} — every
    store; erasure = purge/crypto-shred/pseudonymise, never hide; restrict suppresses indexing/agent-use/analytics/
    notif), 11.4 (crypto-shred / per-subject DEK — bodies/drafts), 4.8 (resolve_pseudonym / erase — the
    pseudonym-map shred → [erased user]), 10.4 (the DSR fan-out — the cascade reaches Search/Refs/Notif via the
    bus, never a backdoor), 10.8 (the erasure ledger — drives post-restore re-erasure), 10.9 (the ONE posture, BY
    REFERENCE; the [OPEN — LEGAL] residual), 2.7 (*.erased tombstones on the log), 11.5 (the backups the
    crypto-shred must reach).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C8 work, entry, exit, the LEGAL residual floor) +
    §1 (non-negotiability item 3: erasure that misses a Chat holder) + §6 (production-hardened) + §5 (the
    free-text-residual floor → the ONE posture, R-C5).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D8 (erase a person → bodies
    crypto-shred in hot + cold + backups; mentions → [erased user]; read-state/drafts/unfurl-cache purged; Search
    incl. embeddings / Refs / Notif cascade → 0 recoverable PII; holder receipts).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the GDPR-holder module):
  - The GDPR holder: locate / export / rectify / restrict / erase over EVERY chat store (10.1).
  - Author erasure: crypto-shred P's per-subject DEK → every body P authored unrecoverable in hot + cold segments
    + backups SIMULTANEOUSLY (WITHOUT rewriting the immutable log, 11.4); tombstone the record (chat.message.erased,
    2.7).
  - Mentioned erasure: the structured mention(Principal) → pseudonym-map shred (4.8) → renders [erased user] on
    next render (free, because the node is structured + pseudonymous).
  - The cascade reaches Search (incl. embeddings) / Refs / Notif via the bus + DSR (10.4), NEVER a backdoor;
    read-state / drafts / unfurl-cache purged.
  - The restriction flag (Art. 18) honoured at EVERY read path: a restricted subject is excluded from indexing /
    agent-use / new notification routing / analytics (a distinct state from erasure).
  - The free-text third-party residual handled BY REFERENCE to the ONE platform posture (10.9, X-7) — chat writes
    NO fifth chat-specific residual statement; it supplies only the structural floor (per-subject DEK shred +
    pseudonym-map shred + restrict).
  - FLOOR named: the free-text third-party residual → the ONE platform posture (10.9), [OPEN — LEGAL], ratified
    ONCE by counsel/DPO (R-C5) — the structural floor ships REGARDLESS; the residual is one ratified statement,
    parallel-tracked (LEGAL), not a chat blocker. State it as an untested-but-named LEGAL floor.
- **CONTRACTS TO IMPLEMENT.** 10.1 the holder over every chat store (owned). 11.4 crypto-shred per-subject DEK
  (consumed — bodies/drafts). 4.8 resolve_pseudonym / erase (consumed — mention shred). 10.4 the DSR fan-out
  (consumed — the cascade). 10.8 the erasure ledger (consumed — post-restore re-erasure). 10.9 the ONE posture
  (consumed — BY REFERENCE). 2.7 *.erased tombstones (owned — chat.message.erased). Implement to the frozen
  shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D8 (erase a person → bodies crypto-shred in hot + cold + backups; mentions → [erased user];
    read-state/drafts/unfurl-cache purged; Search incl. embeddings / Refs / Notif cascade → 0 recoverable PII;
    holder receipts) — SCHED; the recoverable-PII signal = 0 across hot + cold + backups + embeddings; the
    holder-receipt set is complete (0 holders missed).
  - The restriction flag: a restricted subject is excluded from indexing / agent-use / notif-routing / analytics
    (0 processings on a restricted subject) — CI; the restricted-processing signal = 0.
- **TESTS (required).** Unit tests for: the per-subject-DEK crypto-shred (every authored body unrecoverable), the
  mention pseudonym-shred → [erased user], the unfurl-cache/read-state/drafts purge, the restriction-flag
  suppression at every read path. The CDC pair for 10.1, 11.4, 4.8, 10.4. The drill-harness scenario for CHAT-D8
  (a CHAINED erase → assert 0 recoverable PII across hot/cold/backups/embeddings + a complete holder-receipt set).
  State the cargo-mutants mutation floor for the crypto-shred + restriction core module (mandatory-core — the
  0-recoverable-PII property).
- **DEFINITION OF DONE.** The chat GDPR holder + the full erasure cascade exist and compile; CHAT-D8 emits a dated
  green artifact (0 recoverable PII across hot + cold + backups + embeddings; complete holder receipts); the
  restriction flag suppresses every processing for a restricted subject; the unit + CDC + drill tests pass; the
  contract-coverage scanner is green; the LEGAL residual is named as an [OPEN — LEGAL] floor (untested-but-named,
  R-C5); the no-untagged-personal-data lint is green; the work is committed. No gate greened by a weakened
  threshold — a red CHAT-D8 is a dated scorecard row, not a softened check.
- **COMMIT.** Header: P-<NNN> M4: Chat erasure cascade across every holder (crypto-shred + pseudonym-shred +
  embeddings + backups). Body lists: 10.1 the holder, 11.4 per-subject-DEK body shred, 4.8 mention pseudonym
  shred, 10.4 the DSR cascade; CHAT-D8 greened (0 recoverable PII, complete holder receipts); the LEGAL residual
  named BY REFERENCE to the ONE posture (10.9, [OPEN — LEGAL]). Branch first if on default; do not push unless
  asked. End with the Co-Authored-By trailer.

---

### CHAT-P11 — Agent presence, streaming + explicit-first dispatch (mock-provable; no auto-spawn)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C9 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C9 — Agent presence, streaming +
  explicit-first dispatch").
- **DEPENDS-ON.** CHAT-P4 (the firehose for presence + partials) + CHAT-P8 (EffectApi + the agent ToolDef set) +
  CHAT-P7 (the mention is the notify-not-dispatch producer). The M2 Agent prompts that ship AgentRuntime::step
  --use-mock (8.3), explicit-first dispatch (8.6, CHAT-1), EffectApi (8.2), and — the hard blocker — AG-D4 GREEN
  (no agent compute over a red sandbox gate). The M1 Storage reserve/settle (11.7). The M1 Identity mint_run_token
  (4.7).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native; mock implementations during development — the strategy pattern, --use-mock,
    no real agents; first-class event propagation/triggers); ../../external-insights/01-process-and-quality-
    doctrine.md §3 (prove-it — a casual @agent mention must NOT auto-spawn a costed run; the drill forces it), §8
    (cost/abuse is decision-shaped — explicit-first, no auto-spawn until counsel-gated).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §7.2 (agent
    presence classes available/busy/rate-limited/offline on the firehose), §7.3 (streaming partials
    agent.message.partial; final replaces partial), §7.5 (the agent provenance popover from causation_id /
    correlation_id / on_behalf_of); 03-events-contracts-and-glue.md §1.2 (agent.message.partial firehose-only),
    §1.1 (chat.message.mentioned = the agent notify-not-dispatch signal, the explicit-first reference gate);
    04-views-cli-and-api.md §1 (S5 thread streaming, S8 agent presence, S12 the provenance popover).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §6 (explicit-first
    dispatch pinned — a mention notifies, does not auto-spawn; implicit auto-dispatch is L-3, counsel-gated).
  - Contracts: contract-index.md rows 8.3 (AgentRuntime::step --use-mock — the strategy seam, a real runtime
    flag), 8.6 (EventInbox::deliver + explicit-first dispatch CHAT-1), 8.2 (EffectApi — the agent's chat output
    path), 8.4 (ToolHands::exec + AG-D4 — the four uniform guarantees; no agent compute over a red AG-D4), 11.7
    (reserve/settle — gates even the explicit run; no balance → no run), 4.7 (mint_run_token — the per-run token),
    3.5 (the firehose — presence + partials).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C9 work, entry, exit) + §1 (non-negotiability item
    5: explicit-first dispatch) + §6 (production-hardened) + §5 (the mock-runtime floor → real LlmAgentRuntime,
    post-M5).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D16 (drive the streaming UX
    against the mock runtime → partials stream; final replaces partial; a mid-stream reconnect resumes the final,
    never a half-message), CHAT-D17 (a casual @agent mention → notifies the agent's inbox, does NOT spawn a costed
    run; only an explicit action / structured trigger dispatches; reserve/settle gates even the explicit run).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the agent-presence +
  dispatch modules), built and proven against the MOCK runtime (--use-mock; no real agents during development):
  - Agent presence classes (available / busy / rate-limited / offline) on the firehose; streaming partials
    (agent.message.partial) on the firehose, final replaces partial.
  - Explicit-first dispatch (CHAT-1, 8.6): a casual @agent mention NOTIFIES the agent's inbox, does NOT spawn a
    costed run; only an explicit action / structured trigger dispatches; reserve/settle gates even the explicit
    run (no balance → no run). NO auto-spawn path is wired (L-3, counsel-gated) — state this is deliberately
    absent.
  - The agent provenance popover (S12): "why did this agent post?" from causation_id / correlation_id /
    on_behalf_of.
  - FLOOR named: the agent runtime = the mock (--use-mock, scripted-deterministic); the real LlmAgentRuntime is
    the post-M5 follow-on (a config/impl swap, not a rewrite, after AG-D4/D2/D3/D5 green — VISION §3). State the
    no-auto-spawn path is a deliberate, counsel-gated absence (L-3), not an omission.
- **CONTRACTS TO IMPLEMENT.** 8.3 AgentRuntime::step --use-mock (consumed — mock-provable streaming). 8.6
  explicit-first dispatch (consumed — the mention notifies, no auto-spawn). 8.2 EffectApi (consumed — the agent
  chat output). 11.7 reserve/settle (consumed — gates the explicit run). 4.7 mint_run_token (consumed). 3.5 the
  firehose presence/partials (consumed). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D16 (drive the streaming UX against the mock runtime → partials stream; final replaces partial; a
    mid-stream reconnect resumes the FINAL, never a half-message) — CI; the half-message-on-reconnect signal = 0.
  - CHAT-D17 (a casual @agent mention → notifies the agent's inbox, does NOT spawn a costed run; only an explicit
    action / structured trigger dispatches; reserve/settle gates even the explicit run) — CI; the
    auto-spawn-on-mention signal = 0; the unreserved-run signal = 0.
  - AG-D4 (the permanent sandbox-escape gate) re-confirmed green before any agent compute chat dispatches — the
    drill is upstream (M2); chat asserts it is green, runs no compute over a red AG-D4.
- **TESTS (required).** Unit tests for: the presence-class transitions, the partial→final replacement, the
  mid-stream-reconnect resume-the-final, the explicit-first dispatch (mention notifies, no auto-spawn), the
  reserve-gate (no balance → no run). The CDC pair for 8.3, 8.6, 8.2, 11.7. The drill-harness scenarios for
  CHAT-D16 and CHAT-D17 (each proven against --use-mock). State the cargo-mutants mutation floor for the
  explicit-first-dispatch core module (mandatory-core — the no-auto-spawn cost-abuse property).
- **DEFINITION OF DONE.** Agent presence + streaming + explicit-first dispatch exist and compile against the mock
  runtime; CHAT-D16/D17 each emit a dated green artifact (no half-message, no auto-spawn-on-mention, reserve gates
  the run); AG-D4 is green (no compute over a red sandbox gate); the unit + CDC + drill tests pass; the
  contract-coverage scanner is green; the mock-runtime floor + the no-auto-spawn L-3 absence are named; all lints
  green; the work is committed. This completes the M4 chat surface. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat agent presence + streaming + explicit-first dispatch (mock-provable). Body
  lists: 8.3 --use-mock streaming, 8.6 explicit-first dispatch, 11.7 reserve/settle gate; CHAT-D16/D17 greened
  (0 half-message, 0 auto-spawn, reserve-gated); the mock-runtime floor + the no-auto-spawn L-3 absence named;
  AG-D4 confirmed green. Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P12 — World-scale hardening: the 30x agent-surge + deploy-herd F6 family + the whole-system E2E wedge participation

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-C-S1 (planning/06-roadmaps/subsystems/chat.md §4 "M5 follow-ons" → "M5-C-S1 — 30×
  agent-surge + deploy-herd (the F6 family)") + chat's participation in the whole-system E2E wedge (E2E-1 / E2E-2
  / E2E-4).
- **DEPENDS-ON.** CHAT-P2..CHAT-P11 (the full M4 chat surface — all deterministic correctness drills green). The
  M4 producers/consumers (Git, CI, Issues, Knowledge) green so the E2E wedge has real artifacts. The M5 platform
  prompts that ship the F6 surge harness profiles + the cell bulkhead. The E2E-wedge driver prompts
  (testing-strategy §2) that orchestrate the four chained-mutation scenarios.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale means world-scale; agent-native — the flagship E2E-2 terminates in chat);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the 30× surge with mixed principal
    kinds; observability is part of the pass — the human lane held + the agent lane shed must be SEEN), §4 (chain
    the mutations — the E2E wedge chains operations end-to-end, the bugs live where state updates mid-flight).
  - Architecture: ../04-subsystem-architectures/chat/architecture/07-drills-and-open-questions.md (the chat
    drills + the surge family + the tunables D-C3/D-C4); 02-internals-and-algorithms.md §7 (presence at scale);
    06-reconciliation-compliance.md (chat's E2E participation).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-K (the
    per-surface shed budgets — tuned here from the surge results), ADR-16 (the protected-human-lane shed order).
  - Contracts: contract-index.md rows 1.11 (the protected-human-lane shed order + the per-surface shed budget
    numbers, tuned here), 1.8 (the telemetry survival-signal set the drills assert against — RED/USE per
    principal-kind, shed-counts, breaker-state), 11.7 (reserve/settle — the agent lane gated under surge).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M5-C-S1 work + the E2E wedge participation E2E-1/
    E2E-2/E2E-4) + §5 (the tunables R-C2/OQ-K — tuned from D-C3/D-C4) + §6 (production-hardened progression).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D3 (30× agent message/connection
    surge on one tenant → human connection/read latency in budget; the agent lane sheds 429 + Retry-After honoured;
    other tenants unaffected) — the TE-21 build-gate; CHAT-D4 re-run at scale (deploy-herd); and the E2E rows
    E2E-1 (PR context pane — chat's unfurl/live-update analog), E2E-2 (CI-fail → triage agent → issue → chat →
    fix-PR — the agent-native FLAGSHIP, chat the terminal surface), E2E-4 (DSAR fan-out — chat's CHAT-D8 erasure
    is a named holder in the 0-holders-missed certificate). Read testing-strategy/README.md §2/§3.4 for the wedge.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the surge-hardening + E2E
  harness-scenario modules) and the E2E wedge harness:
  - The CHAT-D3 surge scenario on the failure-injection harness: 30× agent message/connection surge on one tenant
    → assert human connection/read latency in budget; the agent lane sheds 429 + Retry-After honoured; other
    tenants unaffected (the cross-tenant impact = 0).
  - The CHAT-D4 re-run at scale (the deploy-herd / gateway-fleet roll under a connection storm — the M4-C2 drill
    re-run at world scale).
  - Tune the per-surface shed budget NUMBERS (R-C2 / OQ-K) from the CHAT-D3/D4 (D-C3/D-C4) results — promote the
    named-floor shed budgets from CHAT-P4 to tuned values in the thresholds file.
  - Chat's participation in the whole-system E2E wedge: E2E-1 (chat's unfurl/live-update pane via CHAT-D7), E2E-2
    (the agent-native FLAGSHIP — chat is the terminal surface: the explicit-first dispatch, the HITL
    withhold→approve→apply card, the unfurl of the issue + fix-PR, all metered through one wallet), E2E-4 (chat's
    CHAT-D8 erasure as a named holder in the 0-holders-missed DSAR certificate). Wire chat's scenario contributions
    into the wedge harness.
  - FLOOR named: the shed-budget numbers are now TUNED (promoting the CHAT-P4 floor). The Scylla / home-node /
    cross-org promotions are CHAT-P13 (triggered, not unconditional). State which floors remain (CHAT-P13).
- **CONTRACTS TO IMPLEMENT.** 1.11 the shed budgets tuned (owned — chat's connection-storm + agent-mention-storm
  numbers, promoted from floor to tuned). 1.8 the telemetry assertions (consumed — the surge survival signals).
  11.7 reserve/settle under surge (consumed). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D3 (30× agent surge → human latency in budget; agent lane sheds 429 + Retry-After; cross-tenant impact
    = 0) — SCHED; the human-lane-latency + agent-shed-count + cross-tenant-impact signals (1.8) are the green
    artifact (human in budget, agent shed > 0, cross-tenant = 0).
  - CHAT-D4 at scale (deploy-herd → bounded reconnect; resume completes for all; no loss) — SCHED.
  - E2E-2 (the flagship — CI-fail → triage agent → issue → chat → fix-PR terminating in chat: exactly-once HITL +
    merge, 0 leak) green — the chat terminal surface contributes its green artifact. E2E-1 and E2E-4 green for
    chat's panes.
- **TESTS (required).** The drill-harness scenarios for CHAT-D3 and CHAT-D4 at scale (each a surge scenario with
  mixed principal kinds asserting against the 1.8 survival signals). The E2E wedge scenario contributions for
  E2E-1 / E2E-2 / E2E-4 (chained-mutation scenarios against a full cell with mock agents per EI-01 §4). A test
  that the tuned shed-budget numbers hold the human lane under 30× while the agent lane sheds. No new core module;
  if the surge harness touches a mandatory-core module, state its mutation floor.
- **DEFINITION OF DONE.** The CHAT-D3/D4 surge scenarios + the E2E wedge contributions exist and run; CHAT-D3,
  CHAT-D4-at-scale each emit a dated green artifact (human in budget, agent shed, cross-tenant = 0); E2E-1/E2E-2/
  E2E-4 are green for chat's surfaces (the flagship E2E-2 terminates green in chat); the shed-budget numbers are
  tuned in the thresholds file; the remaining floor promotions are named (CHAT-P13); the tests pass; the work is
  committed. A red surge gate is a dated scorecard row, never a weakened budget.
- **COMMIT.** Header: P-<NNN> M5: Chat 30x agent-surge + deploy-herd hardening + E2E wedge participation. Body
  lists: 1.11 shed budgets tuned, 1.8 surge survival signals; CHAT-D3/D4-at-scale greened (human in budget, agent
  shed, cross-tenant 0); E2E-1/E2E-2/E2E-4 green for chat (the flagship terminates in chat); the remaining floor
  promotions named (CHAT-P13). Branch first if on default; do not push unless asked. End with the Co-Authored-By
  trailer.

---

### CHAT-P13 — The named floor promotions: ScyllaDB hot tier + channel-sharded home-node + cross-org channels + comment-threading consolidation (each where triggered)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-C-S2 + M5-C-S3 + M5-C-X1 + M5-C-X2 (planning/06-roadmaps/subsystems/chat.md §4 "M5
  follow-ons" — the named floor promotions). Each is conditional on its trigger (measured volume / subscriber
  count / cross-org demand + the bridge / OQ-L demand); this prompt ships the ones whose trigger has fired and
  names the rest as still-floored.
- **DEPENDS-ON.** CHAT-P2 (the MessageStore trait the Scylla swap rides) + CHAT-P4 (the firehose subject fan-out
  the home-node escalates) + CHAT-P12 (the surge measurements that fire the triggers). The M1 Tenancy cross-cell
  PII-free pointer bridge (12.6) for cross-org channels. The M3/M4 Knowledge + Issues anchored-comment owners for
  the OQ-L consolidation. The M5 platform multi-cell prompts (the bridge goes live).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; name-your-floors — promote a floor only when its trigger fires, and on the
    seam the floor was built to swap); ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-
    floors; the code wins — a floor promotes on measured signal, not on prediction), §3 (re-run the floor's drill
    across the promotion boundary — KN-D1-style: the drill was written to survive the swap).
  - Architecture: ../04-subsystem-architectures/chat/architecture/05-hard-problems.md (the Scylla hot tier, the
    channel-sharded home-node Phoenix/Discord guild model, the cross-org bridge consumption, the comment-threading
    consolidation); 01-tech-and-data-model.md (the MessageStore trait — the Scylla swap seam);
    02-internals-and-algorithms.md §7 (mega-channel fan-out → the home-node).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-I (the cross-cell
    PII-free pointer bridge — resolution always cell-local; cross-org channels ride it), §OQ-L (comment-threading
    consolidation onto the Chat threading primitive — a store/transport swap over the shared #thread-/#comment-
    #sub + content + refs scheme, NOT a rewrite).
  - Contracts: contract-index.md rows 11.2 (BlobStore object-store swap — the cold segments move with the
    promotion; the cold tier + MessageStore trait are identical either way), 12.6 (the cross-cell PII-free pointer
    bridge — cross-org channels; per-viewer resolution cell-local; multi-cell DSR iterates member_cells), 10.4
    (the DSR fan-out iterates member_cells for multi-cell), 5.7 (the shared #sub scheme the comment consolidation
    rides), 3.5 (the firehose the home-node + the consolidation ride).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M5-C-S2/S3/X1/X2 work + triggers) + §5 (the floor
    table — each floor, its band, its follow-on band, its trigger) + §7 (the floors + named follow-ons digest).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — CHAT-D2 + CHAT-D8 re-run across the
    Scylla swap (M5-C-S2); CHAT-D1 re-run across the home-node escalation (M5-C-S3); the multi-cell DSR drills
    (GA-D8 / CP-D7 / CP-D8) for cross-org; the cross-cell-resolution-cell-local property for M5-C-X1.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat — for EACH floor whose trigger
  has fired (build it) and EACH whose trigger has not (name it as still-floored with the measured signal that
  would fire it):
  - M5-C-S2 — ScyllaDB hot-tier promotion (the named M4-C1 floor, R-C6/R-5): triggered by measured per-cell
    write/partition volume; a MessageStore trait swap (the cold tier + trait identical); residency-pinned +
    crypto-shred-capable per cell. Re-run CHAT-D2 + CHAT-D8 across the swap.
  - M5-C-S3 — mega-channel channel-sharded home-node (the named M4-C2 delivery floor, R-5): triggered by
    subscriber count exceeding the subject-fan-out budget; the Phoenix/Discord guild model in Rust +
    consistent-hash. Re-run CHAT-D1 across the escalation.
  - M5-C-X1 — cross-org / federated channels (designed-not-built → on the frozen cross-cell bridge, R-C9): rides
    CrossCellPointer (12.6); per-viewer resolution always cell-local; multi-cell DSR iterates member_cells (10.4);
    needs an explicit cross-tenant capability + residency policy (→ P6 control plane + LEGAL). Built only if the
    bridge ships in M5; otherwise NAMED as designed-not-built.
  - M5-C-X2 — comment-threading consolidation (OQ-L named floor, R-C8): when document-anchored comments
    (Knowledge/Issues) need real-time presence, promote them onto the Chat threading primitive + the firehose
    transport — a store/transport swap over the shared #thread-/#comment- #sub + content + refs scheme, NOT a
    rewrite. Built only if the OQ-L trigger fires; otherwise NAMED in the gap report (E-3).
  - The fs-backed → object-store BlobStore swap (11.2) for the cold segments moves with the Scylla promotion (a
    one-line swap; the cold tier is identical).
  - FLOOR named: any promotion whose trigger has NOT fired is left as a named floor with its measured trigger
    signal and this prompt's id as where it would land — the gap must be VISIBLE, never invisible (EI-04 §4).
    State each explicitly.
- **CONTRACTS TO IMPLEMENT.** 11.2 the object-store BlobStore swap (consumed — cold segments). 12.6 the cross-cell
  bridge consumption (consumed — cross-org channels, cell-local resolution). 10.4 multi-cell DSR (consumed). 5.7
  the shared #sub scheme (owned — the comment consolidation). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - For M5-C-S2 (if triggered): CHAT-D2 (per-conversation total order) + CHAT-D8 (0 recoverable PII) re-run GREEN
    across the Scylla swap — SCHED; the order-violation + recoverable-PII signals = 0 post-swap.
  - For M5-C-S3 (if triggered): CHAT-D1 (resume 0 lost / 0 dup) re-run GREEN across the home-node escalation —
    CI; the lost/dup signal = 0 post-escalation.
  - For M5-C-X1 (if triggered): cross-cell resolution is always cell-local (0 raw cross-cell rows crossing; only
    the projection crosses) + multi-cell DSR iterates member_cells (0 holders missed across cells) — SCHED.
  - For M5-C-X2 (if triggered): the consolidation re-runs the relevant content + refs + #sub drills GREEN across
    the store/transport swap — CI.
  - For any floor NOT triggered: a dated gap-report row naming the measured trigger signal — not a drill, an
    honest named floor (EI-04 §4).
- **TESTS (required).** For each built promotion: the re-run drill scenario (CHAT-D2/D8 for Scylla; CHAT-D1 for
  home-node; the cross-cell-cell-local + multi-cell-DSR for cross-org; the content/refs/#sub drills for the
  consolidation) — each written to survive the swap. The CDC pair for any newly-consumed row (11.2, 12.6). State
  the cargo-mutants mutation floor for any mandatory-core module touched by a built promotion. For each
  NOT-triggered floor, a recorded gap-report row (untested-but-named).
- **DEFINITION OF DONE.** Each TRIGGERED floor is promoted on its built-to seam and its re-run drill emits a dated
  green artifact (order preserved / 0 recoverable PII / 0 lost-dup / cell-local resolution); each NOT-triggered
  floor is named in the gap report with its measured trigger signal and this prompt as its landing; the CDC pairs
  and tests pass; all lints green; the work is committed. No floor masquerades as done; no gate greened by a
  weakened threshold.
- **COMMIT.** Header: P-<NNN> M5: Chat floor promotions (Scylla hot tier / home-node / cross-org / comment
  consolidation, where triggered). Body lists: which floors were triggered + built (with the measured signal that
  fired each) and which remain named-floored (with their trigger signal); the re-run drills greened (CHAT-D2/D8/
  D1 / cross-cell); 11.2/12.6 consumed where built. Branch first if on default; do not push unless asked. End with
  the Co-Authored-By trailer.

---

### CHAT-P14 — The switch test: drive the real Chat UI in a browser (the 13 screens + the responsive cases)

- **BAND.** M6.
- **ROADMAP MILESTONE.** M6 (planning/06-roadmaps/subsystems/chat.md §4 "M6 — Dogfooding: the switch test").
- **DEPENDS-ON.** CHAT-P2..CHAT-P12 (the full chat surface, world-scale-ready) + CHAT-P13 (the floors promoted
  where triggered). The M5 platform prompts that ship the world-scale-readiness gate + the E2E wedge green (you do
  not dogfood real team data over a red restore-verify or DSAR fan-out). The M6 dogfood prompts (Myelin hosts
  itself; the team talks in Myelin's own Chat).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX and design — the product, not an internal tool; design comes before
    implementation); ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — the switch
    test is reached by DRIVING the real UI in a browser, not by reading the feature list; the modal-in-the-wrong-
    place / picker-off-screen class of bug only appears when a human drives it); ../../external-insights/05-ux-and-
    design.md (the design-language §8b — measured contrast + latency budgets + render(parse(md))===md + overlays
    against the real anchor).
  - Architecture: ../04-subsystem-architectures/chat/design/wireframes.md (the 13 screens S1–S13 + their
    empty/loading/error states — the build-to); design/user-flows.md (the primary flows); design/information-
    architecture.md (the one shell composition); ../04-subsystem-architectures/chat/architecture/04-views-cli-and-
    api.md §1 (the 13 screens + the responsive cases Chat owns — the hover-action case, the width-takeover case,
    the flip-popover case).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (the design-language
    §8b done-bar L5 — the switch test).
  - Contracts: contract-index.md rows 13.1 (render(parse(md))===md — the content round-trip held against the real
    anchor), 1.11 (the shed order — the human lane held under the dogfood load).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M6 work, entry, exit) + §6 (production-hardened — the
    switch test is the final bar) + the master sequencing M6 ("a team could move off Slack without hitting a wall
    the old tool didn't have").
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D19 (drive the real Chat UI →
    a team could move to it without hitting a wall the old tool didn't have; measured-contrast tokens; latency
    budgets — optimistic send < ~100ms perceived; flip-popovers against the real bottom-pinned composer anchor).
- **DELIVERABLE (what to build + exactly where in the repo).** In the chat frontend package + the M6 dogfood
  harness:
  - Drive the real Chat UI (the 13 screens S1–S13) in a browser for the switch test — the team talks in Myelin's
    own Chat (the cheapest, most honest load generator is the platform's own development).
  - The responsive cases chat owns (SUB-X), tested against the REAL anchor: the hover-action case (message-row
    actions are a default ⋯ / long-press on touch, never hover-only); the width-takeover case (rail + secondary
    nav collapse to drawers at the mobile breakpoint so timeline+composer fill the viewport); the flip-popover
    case (the @ / # / slash pickers flip ABOVE a bottom-pinned composer with a max-height when there's no room
    below — tested against the real bottom-pinned composer anchor). The shell is pinned 100vh / overflow:hidden
    with min-height:0 scrollers so the composer never drops below the fold.
  - FLOOR named: any screen lacking a switch-test pass is recorded honestly (yes / no / partial — EI-01 §4); a
    surface is done only when someone could move to it without hitting a wall the old tool didn't have, and that
    verdict is reached by DRIVING it. State the honest per-screen record.
- **CONTRACTS TO IMPLEMENT.** 13.1 render(parse(md))===md against the real anchor (consumed). 1.11 the human lane
  held under dogfood load (consumed). No new owned contract — this is the drive-the-real-thing done-bar.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D19 (drive the real Chat UI → a team could move to it without hitting a wall the old tool didn't have;
    measured-contrast tokens meet the design-language threshold; latency budgets — optimistic send < ~100ms
    perceived; flip-popovers correct against the real bottom-pinned composer anchor) — SCHED; the
    measured-contrast + perceived-send-latency + popover-placement signals meet their thresholds; the per-screen
    switch-test verdict is recorded (yes/no/partial).
- **TESTS (required).** A browser-driven switch-test pass over the 13 screens (driven, not read — EI-01 §4),
  recording yes/no/partial per screen. The responsive-case checks (hover-action, width-takeover, flip-popover)
  against the real anchor. A measured-contrast + send-latency assertion against the design-language thresholds.
  No new core module; this is the drive-the-real-thing gate. Record any wall honestly as a named floor with a
  follow-on.
- **DEFINITION OF DONE.** The real Chat UI is driven in a browser over the 13 screens; CHAT-D19 emits a dated
  green artifact (measured contrast + latency budgets met; flip-popovers correct against the real anchor); the
  responsive cases pass against the real anchor; the per-screen switch-test verdict is honestly recorded
  (yes/no/partial); any wall is named with a follow-on; the work is committed. The switch test is the platform
  done-bar for chat — reached by driving it, not by reading the feature list. No gate greened by a weakened
  threshold.
- **COMMIT.** Header: P-<NNN> M6: Chat switch test (drive the real UI; the 13 screens + responsive cases). Body
  lists: CHAT-D19 greened (measured contrast + < ~100ms perceived send + flip-popovers against the real anchor);
  the per-screen switch-test verdict recorded (yes/no/partial); any wall named with a follow-on. Branch first if
  on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

## Coverage matrix (every chat roadmap milestone → its prompt(s); no gap)

| Roadmap milestone (planning/06-roadmaps/subsystems/chat.md) | Band | Prompt(s) | Primary drills greened |
|---|---|---|---|
| M2-C0 — declare the contract surfaces | M2 | CHAT-P1 | (contract-coverage scanner; fragment compile) |
| M4-C1 — durable message store + outbox co-commit | M4 | CHAT-P2 (store) + CHAT-P3 (conversation/membership) | CHAT-D13, CHAT-D14, CHAT-D2 |
| M4-C2 — firehose resume-cursor transport + gateway | M4 | CHAT-P4 | CHAT-D1, CHAT-D4 |
| M4-C3 — composer over the frozen content subset | M4 | CHAT-P5 | render(parse(md))===md (chat KN-D2) |
| M4-C4 — per-viewer permission-aware unfurls | M4 | CHAT-P6 | CHAT-D5, CHAT-D6, CHAT-D7, CHAT-D18 |
| M4-C5 — read-state hot path + Activity-as-view | M4 | CHAT-P7 | CHAT-D12 |
| M4-C6 — HITL approval-card bridge + ToolDef set | M4 | CHAT-P8 | CHAT-D9, CHAT-D10 |
| M4-C7 — ACL-filtered search + reindex parity | M4 | CHAT-P9 | CHAT-D11, CHAT-D15 |
| M4-C8 — erasure cascade across every holder | M4 | CHAT-P10 | CHAT-D8 |
| M4-C9 — agent presence/streaming + explicit-first | M4 | CHAT-P11 | CHAT-D16, CHAT-D17 |
| M5-C-S1 — 30× surge + deploy-herd + E2E wedge | M5 | CHAT-P12 | CHAT-D3, CHAT-D4-at-scale, E2E-1/E2E-2/E2E-4 |
| M5-C-S2/S3/X1/X2 — named floor promotions | M5 | CHAT-P13 | CHAT-D2/D8/D1 re-run across swaps; cross-cell DSR |
| M6 — the switch test | M6 | CHAT-P14 | CHAT-D19 |

**Permanent gates inherited (re-confirmed by the prompts that touch their surface):** STOR-D1/D2 (restore-verify —
CHAT-P2, CHAT-P13) and AG-D4 / CI-T1 (sandbox escape — CHAT-P8, CHAT-P11). Neither is chat-owned; both bound chat's
"done".

**Floors named, each with its follow-on prompt:** Postgres hot tier → ScyllaDB (CHAT-P2 → CHAT-P13);
firehose subject fan-out → channel-sharded home-node (CHAT-P4 → CHAT-P13); Rust gateway → BEAM/Phoenix hatch
(CHAT-P4, only if CHAT-D3/D4 prove Rust intractable); single home-cell → multi-cell cross-org (CHAT-P3/P4 →
CHAT-P13); fs BlobStore → object-store (CHAT-P2 → CHAT-P13); mock agent runtime → real LlmAgentRuntime (CHAT-P11 →
post-M5/execution); free-text third-party erasure residual → the ONE platform posture, counsel/DPO ratified
(CHAT-P10, LEGAL, parallel); cross-org channels → on the cross-cell bridge (CHAT-P3 → CHAT-P13);
comment-threading → consolidation onto the Chat threading primitive (CHAT-P5 → CHAT-P13); per-surface shed budgets
→ tuned numbers (CHAT-P4 → CHAT-P12); per-message CAS → no CRDT (chat is single-author; n/a follow-on, stated so
no agent builds one); replay skeleton → full parity (CHAT-P2 → CHAT-P9).
