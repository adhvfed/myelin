# Phase 7 — Prompt Ledger: Chat (the maximal-consumer subsystem) — finer-granularity (Phase 7-A pass 2)

> Prompt count: 14 (first pass) → 32 (this finer-grained pass). Every multi-deliverable prompt has been split into
> single-deliverable, clean-context, independently-committable units; coverage is preserved (every milestone,
> contract, drill, and floor the first pass covered remains, now at finer granularity, with DEPENDS-ON re-threaded
> across the new ids). No padding — the extra volume is the real repo locations, contract shapes, and drill
> thresholds an isolated agent needs.
>
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
> Coverage (every roadmap milestone → its prompt(s), no gap): M2-C0 → CHAT-P1 + CHAT-P2 + CHAT-P3; M4-C1 →
> CHAT-P4 + CHAT-P5 + CHAT-P6 (store) + CHAT-P7 + CHAT-P8 (conversation/membership); M4-C2 → CHAT-P9 + CHAT-P10;
> M4-C3 → CHAT-P11 + CHAT-P12; M4-C4 → CHAT-P13 + CHAT-P14 + CHAT-P15; M4-C5 → CHAT-P16 + CHAT-P17; M4-C6 →
> CHAT-P18 + CHAT-P19; M4-C7 → CHAT-P20 + CHAT-P21; M4-C8 → CHAT-P22 + CHAT-P23; M4-C9 → CHAT-P24 + CHAT-P25;
> M5-C-S1 → CHAT-P26 + CHAT-P27; M5-C-S2 → CHAT-P28; M5-C-S3 → CHAT-P29; M5-C-X1 → CHAT-P30; M5-C-X2 → CHAT-P31;
> M6 → CHAT-P32. Thirty-two prompts, no milestone gap. (See the coverage matrix at the foot for the authoritative
> milestone→prompt mapping; the inline list above is a reading aid.)

---

### CHAT-P1 — Declare the chat.* event taxonomy (durable-via-outbox vs firehose-only) + freeze the Bus token grammar

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-C0 (planning/06-roadmaps/subsystems/chat.md §4 "M2-C0 — Chat declares its contract
  surfaces") — the event-taxonomy slice. The ReBAC fragment + #sub grammar slice is CHAT-P2; the humanise/notif +
  firehose-scope + TE-21 slice is CHAT-P3.
- **DEPENDS-ON.** The M0 substrate prompts (Cargo workspace + the eight glue-crate skeletons + the twelve lints +
  the contract-coverage scanner; master §2 M0, substrate roadmap SUB-M0). The M1 Bus prompts that ship the event
  taxonomy seed (2.9) + the EventEnvelope freeze (2.1). The index places this alongside the M2 reactive-layer
  freeze (Bus must accept chat's token contributions).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native from the ground up; first-class event propagation/triggers);
    ../../external-insights/01-process-and-quality-doctrine.md §5 (the ratchet — an uncommitted gate is no gate),
    §1 (name-your-floors, code-wins-over-docs), §7 (reconcile cross-component contracts at the plan layer before
    either side ships — chat declares its event half so Bus compiles against it now).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §1 (the COMPLETE
    chat.* taxonomy), §1.1 (the durable-via-outbox set), §1.2 (the firehose-only set + the no-raw-publish + the
    firehose-seam structural separation), §4 (the fanout-class declaration that the tokens carry); 00-overview.md
    §2.1 (the four owned hot parts vs consumed contracts).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (BUS-2 — the
    persist+emit co-commit the durable tokens align to; the per-aggregate conversation_id ordering).
  - Contracts: contract-index.md rows 2.9 (event taxonomy + the token table grammar <subsystem>.<type>.<event>;
    the chat.* tokens; per-aggregate conversation_id), 2.1 (EventEnvelope the chat.* shapes align to), 1.6 (the
    lints chat compiles against, incl. no-raw-publish).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M2-C0 work, the taxonomy bullet) + §3 (the chat.*
    taxonomy "declare" row).
  - Drills/strategy: testing-strategy/README.md — no chat runtime drill here; the exit is the token grammar
    parsing clean + the contract-coverage scanner on the taxonomy row.
- **DELIVERABLE (what to build + exactly where in the repo).** In a new chat subsystem implementation crate
  (myelin-chat, under the Cargo workspace) plus its contribution into the shared Bus glue crate (myelin-events):
  - The COMPLETE chat.* event taxonomy as the durable-via-outbox set vs the firehose-only set (arch 03 §1.1/§1.2),
    registered into the Bus taxonomy seed (2.9): durable — chat.message.created/edited/deleted/erased/mentioned,
    chat.reaction.added/removed, chat.thread.created/replied, chat.channel.created/archived/member_added/
    member_removed/linked, chat.read_state.updated (coarse), chat.{channel,message,thread}.snapshot; firehose-only
    — chat.presence.*, chat.typing.*, fine-grained chat.read_state.*, agent.message.partial, the live delivery
    frame.
  - Validate each token against the Bus §6.2 singular token table (chat is the canonical subsystem token; types
    channel/message/thread plus reaction/presence/typing/read_state). Freeze aggregate = conversation_id.
  - The durable/firehose split is structural: state in the crate doc that the durable set is the ONLY set that may
    ride OutboxTx::emit, and the firehose-only set never touches the durable bus (the no-raw-publish lint, 1.6,
    enforces this when the behaviour lands in CHAT-P9/P10).
  - FLOOR named: this prompt ships TOKENS, not a working emit path. State that the durable set's behaviour begins
    in CHAT-P5 (the outbox co-commit) and the firehose set's in CHAT-P10 (live delivery); name both.
- **CONTRACTS TO IMPLEMENT.** 2.9 the chat.* event tokens (owned — registered into the Bus seed, validated against
  the §6.2 grammar). 2.1 EventEnvelope alignment (consumed — the chat.* shapes align). Implement to the frozen
  shapes; a needed change is a whole-workspace contract PR, escalated and written down, not a local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The chat.* tokens are present in the Bus taxonomy and parse under the §6.2 grammar (0 ungrammatical tokens) —
    CI, token-grammar signal = 0 violations.
  - The contract-coverage scanner passes for the chat-owned 2.9 row (provider + consumer CDC coverage present,
    even if stubbed) — CI, scanner signal = 0 uncovered chat taxonomy rows.
  - The durable-vs-firehose split is declared with no firehose-only token in the durable set and vice versa
    (0 misclassified tokens) — CI, a structural check over the two token sets.
- **TESTS (required).** Unit tests that each chat.* token round-trips the §6.2 grammar and that the
  durable/firehose classification is total + disjoint. The provider/consumer CDC stub for row 2.9. State the
  cargo-mutants mutation-score floor for the token-parse/classify module if mandatory-core; if not, say so
  explicitly.
- **DEFINITION OF DONE.** The myelin-chat crate exists and compiles in the workspace; the chat.* tokens are
  registered and grammatical; the durable/firehose split is disjoint and total; the CDC stub and unit tests pass;
  the contract-coverage scanner is green on the 2.9 row; all twelve committed lints are green; the
  tokens-not-behaviour floor note is written naming CHAT-P5 + CHAT-P10 as the follow-ons; the work is committed.
  No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: Chat event taxonomy (chat.* durable/firehose split + Bus token grammar). Body
  lists: 2.9 (chat tokens) registered + grammatical, the durable/firehose split disjoint, the contract-coverage
  scanner green on 2.9, the tokens-not-behaviour floor named (CHAT-P5/P10 begin behaviour). Branch first if on
  default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### CHAT-P2 — Declare the Chat ReBAC fragment (channel.read + watcher) + freeze the #sub grammar (message-/thread-)

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-C0 (planning/06-roadmaps/subsystems/chat.md §4 "M2-C0") — the ReBAC-fragment +
  #sub-grammar slice (the second committable unit of M2-C0; the taxonomy slice is CHAT-P1).
- **DEPENDS-ON.** CHAT-P1 (the myelin-chat crate + the chat.* tokens). The M1 Identity prompts that ship the
  ReBAC namespace engine (4.9) into which fragments compile. The M2 Refs prompts that freeze the #sub grammar +
  the 4-step tombstone ladder (5.7). The index places this alongside the M2 reactive-layer freeze (Identity/Refs
  must accept chat's contributions).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one permission model; GDPR-safe by construction);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component contracts at the plan
    layer before either side ships — chat declares its ReBAC + #sub half so Identity/Refs compile against it now),
    §1 (name-your-floors).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §5 (the ReBAC
    fragment + watcher relation per watchable type), §2 (the ArtifactRef + the #sub mints; message-/thread-);
    01-tech-and-data-model.md (the conversation/message identity — the ULID message_id / thread_root_id the #sub
    is minted from).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen Chat
    ReBAC fragment — channel.read = member + parent_project->read + the watcher relation), §X-4 (the frozen #sub
    grammar — message-/thread- are the Chat kinds).
  - Contracts: contract-index.md rows 4.9 (per-subsystem ReBAC fragment; Chat frozen — the fragment definition,
    compiled by Identity), 5.1/5.7 (ArtifactRef + the frozen #sub grammar; chat mints message-/thread-; the
    4-step tombstone ladder vocabulary).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M2-C0 ReBAC + #sub bullets) + §3 (the ReBAC-fragment
    "declare" row, the #sub-grammar row) + §2 (upstream rows 4.9, 5.7).
  - Drills/strategy: testing-strategy/README.md — no chat runtime drill; the exit is the fragment compiling in the
    cell schema + the #sub mints parsing.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the contract-declaration
  module) + its contributions into the shared cell schema (Identity) + the Refs frozen vocabulary (myelin-refs):
  - The Chat ReBAC namespace fragment submitted into the one cell schema Identity compiles (4.9): channel.read =
    member + parent_project->read, plus the watcher relation per watchable type (channel/thread). The fragment
    must COMPILE in the cell schema — that compile is this prompt's gate, not a runtime property.
  - The #sub grammar contribution frozen into the Refs frozen vocabulary (5.7): message-<opaqueid> (single
    message), thread-<opaqueid> (thread root); the <opaqueid> is the immutable message_id / thread_root_id ULID
    (a stable opaque id, not a positional index). State the stability obligation is chat's (a message id / thread
    id is immutable; the #sub survives edits — the mint itself lands in CHAT-P6).
  - FLOOR named: this prompt ships the FRAGMENT DEFINITION + the #sub GRAMMAR, not the runtime membership writes
    (CHAT-P8) or the #sub minting (CHAT-P6). State both follow-ons.
- **CONTRACTS TO IMPLEMENT.** 4.9 the Chat ReBAC fragment (owned — the fragment definition, compiled by Identity).
  5.1/5.7 the chat #sub mints message-/thread- (owned — the grammar contribution; minting lands in CHAT-P6).
  Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Chat ReBAC fragment COMPILES in the shared cell schema Identity builds (a build-time gate, not a runtime
    drill) — CI, the compile is the green artifact.
  - The #sub message-/thread- mints parse/format under the frozen 5.7 grammar (0 ungrammatical #sub strings) —
    CI, the #sub-grammar signal = 0.
  - The contract-coverage scanner passes for the 4.9 + 5.7 rows (provider + consumer CDC coverage present, even if
    stubbed) — CI, scanner signal = 0 uncovered rows.
- **TESTS (required).** Unit tests that the Chat ReBAC fragment compiles, that the #sub message-/thread- mints
  parse/format under 5.7, and that the fragment resolves channel.read = member + parent_project->read against a
  fixture schema. The provider/consumer CDC stubs for rows 4.9, 5.7. State the cargo-mutants mutation-score floor
  for the fragment-compile / #sub-parse module if mandatory-core; if not, say so explicitly.
- **DEFINITION OF DONE.** The Chat ReBAC fragment compiles in the cell schema; the #sub mints parse under 5.7; the
  CDC stubs + unit tests pass; the contract-coverage scanner is green on 4.9/5.7; the fragment-vs-runtime-writes
  floor is named (CHAT-P8) and the grammar-vs-minting floor is named (CHAT-P6); all lints green; the work is
  committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M2: Chat ReBAC fragment + #sub grammar (message-/thread-). Body lists: 4.9 (Chat
  fragment) compiled, 5.7 (message-/thread- #sub) frozen; the contract-coverage scanner green on 4.9/5.7; the
  runtime-writes floor named (CHAT-P8), the minting floor named (CHAT-P6). Branch first if on default; do not push
  unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P3 — Register the humanise keys + the define_notif_rule set + the fanout-class, validate the firehose scope, pin the TE-21 language call

- **BAND.** M2.
- **ROADMAP MILESTONE.** M2-C0 (planning/06-roadmaps/subsystems/chat.md §4 "M2-C0") — the Notif-registration +
  firehose-scope-validation + connection-tier-language-pin slice (the third committable unit of M2-C0). This is
  the slice that makes M2-C0 "done": with CHAT-P1 (tokens) + CHAT-P2 (ReBAC + #sub) + this, the full chat-owned
  M2-C0 contract surface is declared.
- **DEPENDS-ON.** CHAT-P1 (the chat.* tokens — the notif rules + fanout-class reference them) + CHAT-P2 (the #sub
  grammar). The M2 Notif prompts that freeze humanise (7.3) + define_notif_rule (7.6). The M2 Bus prompts that
  freeze the firehose resume-cursor protocol (3.5). The M0 substrate prompt that freezes the cross-language
  harness shim (1.7). The index places this alongside the M2 reactive-layer freeze (Notif/Bus must accept chat's
  contributions).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe — humanise is the ONE templating surface, no chat-private string map; world-scale
    — the bounded firehose scope); ../../external-insights/01-process-and-quality-doctrine.md §7 (reconcile at the
    plan layer — chat declares its humanise/notif/scope half), §1 (name-your-floors — the TE-21 language pin is a
    written floor).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §4 (the
    fanout-class declaration — write-fanout the bounded high-signal set, read-fanout the unbounded ambient set),
    §1.1 (chat.message.mentioned + the card/agent-message string registration); 00-overview.md §0 (where chat
    declares early) + (the TE-21 connection-tier divergence call).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-L (humanise is
    the ONE templating surface), §OQ-J (the firehose resume-cursor protocol shape — subscribe/resume/scope,
    resync_required → *.snapshot; scope is a bounded selector, never *).
  - Contracts: contract-index.md rows 7.3 (humanise keys — card strings, agent-message strings,
    chat.message.mentioned), 7.6 (define_notif_rule — mentioned/replied/thread_watched/approval_requested, each
    with its dedup template + default class; the fanout-class), 3.5 (the firehose resume-cursor protocol +
    scope=channel:<id> bounding), 1.7 (the cross-language harness shim — the TE-21 BEAM hatch written-but-closed,
    a no-op in the Rust default).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M2-C0 humanise/notif + firehose-scope + TE-21
    bullets) + §3 (the humanise + define_notif_rule + fanout-class "declare" rows) + §2 (upstream rows 7.3, 7.6,
    3.5, 1.7) + §5 (the TE-21 language floor).
  - Drills/strategy: testing-strategy/README.md — no chat runtime drill; the exit is the firehose scope shape
    validating against 3.5 + the registrations present.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the contract-declaration
  module) + its contributions into the Notif glue crate + the validation harness:
  - Register the humanise template keys (7.3) for chat card strings, agent-message strings, and
    chat.message.mentioned — no chat-private string map. State humanise is the ONE templating surface (OQ-L).
  - Register the define_notif_rule set (7.6): mentioned / replied / thread_watched / approval_requested, each with
    its dedup template and default class, and the fanout-class declaration (write-fanout for the bounded
    high-signal set; read-fanout for the unbounded ambient set, arch 03 §4).
  - Co-design + validate the firehose resume-cursor protocol (3.5) against chat's scope = channel:<id> bounding:
    the per-view scope shape, the resync_required → *.snapshot fallback contract. No transport implementation here
    — only the validation that chat's scope shape fits the frozen protocol (the transport lands in CHAT-P9).
  - Pin the connection-tier language call (TE-21): Rust default; the BEAM/Phoenix hatch written-but-closed,
    bounded by the frozen harness shim (1.7). State this is a no-op in the all-Rust default.
  - FLOOR named: this prompt ships REGISTRATIONS + VALIDATIONS, not behaviour. State that the humanise/notif rules
    are USED in CHAT-P16/P18, the firehose scope is IMPLEMENTED in CHAT-P9, and the TE-21 hatch is written-but-
    closed (opened only if CHAT-D3/D4 prove Rust intractable, CHAT-P26). Name each follow-on. This completes the
    honest M2-C0 contracts-not-behaviour floor.
- **CONTRACTS TO IMPLEMENT.** 7.3 the humanise keys + 7.6 the define_notif_rule set + the fanout-class (owned —
  registered; used in CHAT-P16/P18). 3.5 the firehose scope=channel:<id> shape (consumed — validated against the
  frozen protocol). 1.7 the harness shim (consumed — no-op). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The humanise keys + the define_notif_rule set + the fanout-class are registered and the contract-coverage
    scanner passes on the 7.3/7.6 rows (provider + consumer CDC coverage present, even if stubbed) — CI, scanner
    signal = 0 uncovered rows.
  - The firehose scope=channel:<id> shape validates against the frozen 3.5 protocol (0 unbounded-scope
    declarations; scope is never *) — CI, the unbounded-scope signal = 0.
  - The TE-21 language pin is recorded as a no-op against the 1.7 shim (the shim's no-op obligation is satisfied) —
    CI. (No chat runtime behaviour drill here — §4 M2-C0 exit is explicitly declarations, not behaviour.)
- **TESTS (required).** Unit tests that each define_notif_rule round-trips its dedup template + default class, that
  the fanout-class is total over the chat.* durable tokens, and that the firehose scope shape is bounded (never *).
  The provider/consumer CDC stubs for rows 7.3, 7.6, 3.5, 1.7. State the cargo-mutants mutation-score floor for any
  mandatory-core module touched; if none, say so explicitly.
- **DEFINITION OF DONE.** The humanise keys + notif rules + fanout-class are registered; the firehose scope shape
  validates against 3.5; the TE-21 no-op is recorded against 1.7; the CDC stubs + unit tests pass; the
  contract-coverage scanner is green on 7.3/7.6/3.5; the registrations-not-behaviour floor is named with each
  follow-on (CHAT-P16/P18 use, CHAT-P9 implements scope, CHAT-P26 the TE-21 hatch); all lints green; the work is
  committed. This closes the M2-C0 contract surface. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M2: Chat humanise keys + notif rules + fanout-class + firehose-scope validation +
  TE-21 pin. Body lists: 7.3/7.6 (humanise/notif + fanout-class) registered, 3.5 (channel scope) validated, 1.7
  (TE-21) no-op recorded; the contract-coverage scanner green on the chat rows; the registrations-not-behaviour
  floor named with follow-ons (CHAT-P16/P18/P9/P26). Branch first if on default; do not push unless asked. End
  with the Co-Authored-By trailer.

---

### CHAT-P4 — The MessageStore trait + the partitioned hot tier + the fs-backed cold-segment tier (the swap seam)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C1 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C1 — The durable message store +
  the outbox co-commit") — the storage-tier slice. The outbox co-commit + idempotent send is CHAT-P5; the
  per-subject-DEK bodies + holder + #sub mint + replay skeleton is CHAT-P6; the Conversation/Membership halves are
  CHAT-P7/P8.
- **DEPENDS-ON.** CHAT-P1 (the chat.* tokens + the myelin-chat crate). The M1 Storage prompts that ship the OLTP
  tier + RLS + encrypted columns (11.1) and the BlobStore content-addressed fs-backed floor (11.2), and — the hard
  blocker — restore-verify STOR-D1/D2 GREEN (chat writes no real message over a red restore-verify). The M1
  Tenancy prompts that ship the (tenant, region) partition + residency-pin (12.1/12.4).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors; world-scalable from day 1);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability — the store is built
    BEFORE any live delivery, any UI), §1 (name-your-floors — the Postgres-hot-tier-now / ScyllaDB-later swap).
  - Architecture: ../04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md (the MessageStore
    trait append/range/tombstone/resync_from; the Postgres-partitioned hot tier by (tenant, region) + time
    sub-partitions; the object-segment cold tier; the message/draft schema); 02-internals-and-algorithms.md §1
    (the message log, ULID order, the per-conversation total order).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (the per-aggregate
    conversation_id ordering).
  - Contracts: contract-index.md rows 11.1 (OLTP tier + RLS + encrypted columns), 11.2 (BlobStore fs-backed floor
    — the cold segments), 12.1 (the (tenant, region) partition key), 12.4 (residency-pin).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C1 MessageStore-trait + tiers work, the ScyllaDB
    floor note) + §1 (non-negotiability item 1: silent message loss) + §6 (first-runnable progression) + §5 (the
    Postgres→ScyllaDB floor row).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D2 (burst sends/edits → per-
    conversation total order — the ordering property this trait's ULID + aggregate keying underpins; the full
    drill is greened in CHAT-P5 where the outbox co-commit lands).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the store module):
  - The Message Service behind a MessageStore trait (append / range / tombstone / resync_from) — the trait is the
    swap seam.
  - The Postgres-partitioned hot tier by (tenant, region) + time sub-partitions, residency-pinned (12.4), and the
    object-segment cold tier (the fs-backed BlobStore floor, 11.2) — IDENTICAL behaviour under either hot engine.
  - k-sortable ULID message_id for intrinsic per-conversation order; aggregate = conversation_id (the keying the
    CHAT-P5 outbox UNIQUE(aggregate, seq) and the CHAT-D2 total-order property build on).
  - The message/draft schema (body_inline / body_nodes columns present; the per-subject-DEK encryption of them is
    wired in CHAT-P6 — here the columns exist but the DEK round-trip is CHAT-P6's).
  - FLOOR named: the message hot tier = Postgres-partitioned; ScyllaDB hot tier is the named M5 follow-on
    (M5-C-S2 / CHAT-P28), triggered by measured per-cell write/partition volume. The MessageStore trait makes it
    a swap; the cold tier + trait are identical either way. The fs-backed BlobStore → object-store swap (11.2) is
    the named M5 follow-on riding the same promotion (CHAT-P28). Name both.
- **CONTRACTS TO IMPLEMENT.** 11.1 the OLTP tier (consumed — chat's message log). 11.2 the BlobStore fs floor
  (consumed — cold segments). 12.1/12.4 the partition key + residency-pin (consumed). Implement to the frozen
  shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The MessageStore trait append/range/tombstone/resync_from round-trips against both the PG hot tier and the fs
    cold tier with IDENTICAL behaviour (0 behavioural divergence between tiers on the trait's surface) — CI.
  - The (tenant, region) partition + residency-pin holds: a write lands in its region's partition (0 cross-region
    rows; the tenant-predicate + residency-pin lints green) — CI.
  - The ULID message_id is monotone per conversation (k-sortable; 0 out-of-order ids within a conversation under
    sequential append) — CI. (The full burst-ordering drill CHAT-D2 is greened in CHAT-P5 with the outbox.)
- **TESTS (required).** Unit tests for the MessageStore trait (append/range/tombstone/resync_from) against both
  tiers, the ULID monotonicity, the partition/residency placement. The CDC provider/consumer pair for rows 11.1,
  11.2, 12.1. State the cargo-mutants mutation-score floor for the MessageStore trait module if mandatory-core; if
  not, say so.
- **DEFINITION OF DONE.** The MessageStore trait + the partitioned hot tier + the fs cold tier exist and compile;
  the trait behaves identically across tiers; the partition/residency-pin holds; the ULID ordering holds; the unit
  + CDC tests pass; the contract-coverage scanner is green; the ScyllaDB + object-store floors are named with their
  follow-on (CHAT-P28); all lints green; the work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat MessageStore trait + partitioned hot tier + fs cold-segment tier. Body
  lists: 11.1 OLTP tier, 11.2 fs BlobStore cold segments, 12.1/12.4 partition + residency-pin; the trait-identical-
  across-tiers + partition + ULID gates greened; the ScyllaDB + object-store floors named (CHAT-P28). Branch first
  if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P5 — The outbox co-commit + idempotent send + per-conversation total order (the silent-data-loss floor for chat)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C1 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C1") — the persist+emit co-commit +
  idempotent-send slice. The storage tiers are CHAT-P4; the bodies/holder/#sub/replay are CHAT-P6.
- **DEPENDS-ON.** CHAT-P4 (the MessageStore trait + the hot tier the persist writes to) + CHAT-P1 (the chat.*
  tokens). The M0 Bus prompts that ship OutboxTx::emit (2.2) + the outbox table with UNIQUE(aggregate, seq) (2.3)
  + the EventHandler template (2.4) + consumer_dedup (2.5). The M1 Storage restore-verify STOR-D1/D2 GREEN (the
  hard blocker — chat writes no real message over a red restore-verify).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable; GDPR-safe — no dual-write);
    ../../external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability — silent data loss
    outranks every feature; built BEFORE any live delivery, any UI), §3 (prove-it — the co-commit drill forces the
    failure and observability watches it survive — observability is part of the pass).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §9 (the envelope
    via the OUTBOX — the only emit path, the co-commit, no dual-write), §1.1 (the durable chat.* set via the
    outbox); 02-internals-and-algorithms.md §1 (idempotent send, the per-conversation total order).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (BUS-2 the
    persist+emit co-commit; the per-aggregate conversation_id ordering).
  - Contracts: contract-index.md rows 2.2 (OutboxTx::emit, the ONLY emit path), 2.3 (the outbox table
    UNIQUE(aggregate, seq), per-conversation aggregate ordering, the D-9 drill at QPS), 2.4 (the EventHandler
    template), 2.5 (consumer_dedup), 11.5 (backup/restore + restore-verify STOR-D1/D2 — the floor chat sits on).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C1 co-commit + idempotent-send work) + §1
    (non-negotiability item 1: silent message loss / dual-write) + §6 (first-runnable).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D13 (crash between persist and
    emit → both or neither), CHAT-D14 (retry same client_nonce → one message), CHAT-D2 (burst sends/edits → per-
    conversation total order).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the store module):
  - The message persist + the chat.message.created outbox row in ONE PG transaction via OutboxTx::emit (BUS-2,
    2.2; no dual-write; the Message Service owns the only emit path — the gateway, built in CHAT-P9, has none).
  - The outbox UNIQUE(aggregate, seq) with aggregate = conversation_id (2.3); per-conversation total order
    preserved under burst from many gateways (the D-9 / CHAT-D2 property), with out-of-order client ops reconciled.
  - Idempotent send: UNIQUE(conv, client_nonce) so a retried send yields exactly one message.
  - FLOOR named: none new — this is the silent-data-loss floor itself. State that the replay(scope, since)
    skeleton + the per-subject-DEK bodies ride on this co-commit and land in CHAT-P6.
- **CONTRACTS TO IMPLEMENT.** 2.2 OutboxTx::emit co-commit (consumed — every chat state change commits its row +
  its event in one tx; the Message Service is the only emit path). 2.3 the outbox table per-conversation ordering
  (consumed). 2.4/2.5 the EventHandler template + consumer_dedup (consumed). Implement to the frozen shapes; no
  local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D13 (crash between message persist and event emit → BOTH committed or NEITHER; 0 orphan messages, 0
    phantom events) — CI; the outbox-depth + consumer-lag telemetry (1.8) is the green artifact (0 orphan/phantom).
  - CHAT-D14 (retry a send with the same client_nonce → exactly ONE message) — CI; the message-count signal = 1.
  - CHAT-D2 (burst sends + edits to one hot channel from many gateways → per-conversation total order preserved;
    ULID + aggregate=conversation_id; out-of-order client ops reconcile) — SCHED; the ordering-violation signal = 0.
  - STOR-D1/D2 (the permanent restore-verify gate) re-confirmed green on the chat message store (RPO ≤ 5 min /
    RTO ≤ 1h-tenant; 0 loss) — SCHED; this prompt does NOT call done over a red STOR-D1.
- **TESTS (required).** Unit tests for the idempotent-send uniqueness and the per-conversation ordering. The CDC
  provider/consumer pair for rows 2.2, 2.3. The drill-harness scenarios for CHAT-D13, CHAT-D14, CHAT-D2. Prefer a
  CHAINED mutation test (send → crash mid-emit → recover → assert exactly-once) over a single-handler test (EI-01
  §4). State the cargo-mutants mutation-score floor for the co-commit + idempotent-send core modules
  (mandatory-core).
- **DEFINITION OF DONE.** The outbox co-commit + idempotent send + per-conversation total order exist and compile;
  CHAT-D13/D14/D2 each emit a dated green artifact (PROVEN, not CLAIMED); STOR-D1 re-confirmed green on the chat
  store; the unit + CDC + drill tests pass; the contract-coverage scanner is green; the bodies/replay follow-on is
  named (CHAT-P6); all lints green; the work is committed. A red gate becomes a dated claimed-not-proven row, never
  a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat outbox co-commit + idempotent send + per-conversation total order. Body
  lists: 2.2/2.3 co-commit + per-conversation order, idempotent send; CHAT-D13/D14/D2 greened with their measured
  numbers (0 orphan/phantom, 1 message, 0 ordering violations); STOR-D1 re-confirmed; the bodies/replay follow-on
  named (CHAT-P6). Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P6 — Per-subject-DEK message bodies + the PersonalDataHolder + the #sub mint + the replay(scope,since) skeleton

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C1 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C1") — the PII-encryption + holder
  + #sub-mint + replay-skeleton slice (the third committable unit of the M4-C1 store half; tiers are CHAT-P4, the
  co-commit is CHAT-P5).
- **DEPENDS-ON.** CHAT-P5 (the persisted message the bodies attach to + the outbox the replay re-emits through) +
  CHAT-P4 (the MessageStore trait) + CHAT-P2 (the frozen #sub grammar). The M1 Storage KMS hierarchy + per-subject
  DEK (11.3/11.4). The M1 GDPR prompts that ship the PersonalDataHolder trait + auto-registration (10.1/1.4) + the
  no-untagged-personal-data lint (10.2).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction); ../../external-insights/04-hard-problems.md §1
    (erasure-vs-immutability — chat bodies ARE PII, the per-subject DEK never bakes erasable plaintext into an
    immutable log); ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors).
  - Architecture: ../04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md (the per-subject-DEK
    body fields body_inline/body_nodes + drafts); 03-events-contracts-and-glue.md §6 (replay — re-emit
    chat.*.snapshot through the outbox via the live consumer; sub-artifact granular); 05-hard-problems.md §5 (chat
    is the most PII-dense holder; the per-subject DEK case).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-4 (the #sub
    grammar — message-/thread-, stable across edits).
  - Contracts: contract-index.md rows 11.3 (KMS hierarchy), 11.4 (crypto-shred / per-subject DEK for
    bodies/drafts), 10.1 (PersonalDataHolder over every store), 10.2 (the #[personal_data] tags + the
    no-untagged-personal-data lint), 5.1/5.7 (the message-/thread- #sub mints, stable across edits), 2.6
    (replay(scope, since) — the skeleton here, full parity in CHAT-P21).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C1 per-subject-DEK + holder + #sub mint + replay
    skeleton bullets) + §3 (the #sub mint row M4-C1, the holder row, the replay skeleton row).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — no standalone drill here; the
    per-subject-DEK round-trip + the no-untagged-personal-data lint are the gate; the full erasure proof is CHAT-D8
    (CHAT-P22) and the full replay parity is CHAT-D15 (CHAT-P21).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the store + holder modules):
  - Per-subject DEK encryption of body_inline / body_nodes + drafts (chat bodies ARE PII, 11.4); the body is
    NEVER stored as erasable plaintext in the immutable log.
  - PersonalDataHolder auto-registration over every chat store opened by the harness (10.1/1.4); PII fields tagged
    #[personal_data(category, role, basis, retention, erasure, subject_locator)] so the no-untagged-personal-data
    lint is green (10.2).
  - The message-<id> / thread-<root> #sub minting (5.7), stable across edits (an edited message keeps message_id).
  - The replay(scope, since) SKELETON (re-emit chat.{message,channel,thread}.snapshot through the outbox via the
    live consumer; sub-artifact granular).
  - FLOOR named: full Search/Refs/Notif replay parity is proven in CHAT-P21 (M4-C7) — this is the skeleton only.
    State this so the next agent does not believe replay parity is done here.
- **CONTRACTS TO IMPLEMENT.** 11.4 the per-subject-DEK body encryption (consumed). 10.1/10.2 the holder + tags
  (owned — over the chat stores). 5.1/5.7 the message-/thread- #sub mint (owned). 2.6 replay skeleton (owned).
  Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The per-subject-DEK round-trip: a body encrypts on write + decrypts on authorized read; 0 plaintext body bytes
    in the immutable log — CI; the plaintext-in-log signal = 0.
  - The no-untagged-personal-data lint green on the chat schema (0 untagged PII fields) — CI.
  - The #sub mint is stable across edits (edit a message → message-<id> unchanged; 0 #sub drift on edit) — CI.
  - The replay skeleton re-emits chat.*.snapshot through the outbox for a sub-artifact scope (the snapshot tokens
    appear on the bus for a replayed scope; 0 snapshots emitted off the outbox) — CI.
- **TESTS (required).** Unit tests for: the per-subject-DEK round-trip, the #sub mint stability across edits, the
  replay-skeleton snapshot emission. The CDC provider/consumer pair for rows 11.4, 5.7, 2.6, 10.1. State the
  cargo-mutants mutation-score floor for the per-subject-DEK module (mandatory-core — the no-plaintext property).
- **DEFINITION OF DONE.** The per-subject-DEK bodies + the holder + the #sub mint + the replay skeleton exist and
  compile; the DEK round-trip leaves 0 plaintext in the log; the no-untagged-personal-data lint is green; the #sub
  is stable across edits; the replay skeleton emits snapshots through the outbox; the unit + CDC tests pass; the
  contract-coverage scanner is green; the replay-parity floor is named (CHAT-P21); all lints green; the work is
  committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat per-subject-DEK bodies + PersonalDataHolder + #sub mint + replay skeleton.
  Body lists: 11.4 per-subject-DEK bodies, 10.1 holder + tags, 5.7 #sub mint, 2.6 replay skeleton; the DEK
  round-trip + no-untagged-PII lint + #sub-stability gates greened; the replay-parity floor named (CHAT-P21).
  Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P7 — The Conversation / Membership entity + the membership_by_principal conversation-list index

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C1 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C1") — the Conversation/Membership
  entity slice (the first committable unit of the membership half; the zookie-in-tx + new-enemy + gate is CHAT-P8).
- **DEPENDS-ON.** CHAT-P5 (the message store + outbox co-commit) + CHAT-P4 (the MessageStore trait). The M1
  Tenancy partition (12.1).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one permission model; world-scale);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors — the single home-cell vs
    cross-org floor), §7 (one primitive — the conversation list is a list_objects-backed view, not a third copy).
  - Architecture: ../04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md (the Conversation
    entity + kinds channel/dm/thread-host; the membership table; the membership_by_principal index that backs the
    conversation list S1; the retention_days / linked_ref fields).
  - Contracts: contract-index.md rows 11.1 (the OLTP tier the conversation/membership rows live in), 12.1 (the
    partition key), 4.3 (list_objects — the conversation list is leak-free / no-N+1; the SetExpr push-down is
    consumed in CHAT-P8/P13, but the index backs it here).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C1 Conversation/Membership Service work) + §6
    (first-runnable).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — no standalone drill here; the membership
    correctness drills (the new-enemy guard) land in CHAT-P8; here the gate is the entity + the conversation-list
    index resolving.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the conversation module):
  - The Conversation entity with kinds (channel / dm / thread-host as the arch defines), with retention_days +
    linked_ref fields.
  - The membership table.
  - The membership_by_principal index that backs the conversation list (S1) — the leak-free, no-N+1 list of
    conversations a principal is in (the list_objects gate is wired in CHAT-P8/P13; this index is the candidate
    set it joins against).
  - FLOOR named: single home-cell is the M4 floor; the cross-org / federated channels follow-on (M5-C-X1 /
    CHAT-P30) rides the cross-cell bridge (12.6) and is designed-not-built — the Conversation model here MUST NOT
    foreclose it. State this explicitly.
- **CONTRACTS TO IMPLEMENT.** 11.1 the OLTP tier (consumed — the conversation/membership rows). 12.1 the partition
  key (consumed). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The Conversation entity persists with its kinds + retention_days + linked_ref (round-trips append/read; 0
    schema-violation rows) — CI.
  - The membership_by_principal index returns exactly the conversations a principal is a member of (0 missing, 0
    extra rows for a fixture principal) — CI.
  - The Conversation model does not foreclose multi-cell (the home-cell is a field, not a hard assumption; a
    structural check that no single-cell assumption is baked into the entity) — CI.
- **TESTS (required).** Unit tests for: the Conversation entity round-trip, the membership_by_principal index
  correctness, the kind discrimination. The CDC pair for rows 11.1, 12.1. State the cargo-mutants mutation floor
  for the conversation entity module if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The Conversation/Membership entity + the membership_by_principal index exist and compile;
  the entity round-trips; the conversation-list index is exact; the model does not foreclose multi-cell; the unit
  + CDC tests pass; the contract-coverage scanner is green; the cross-org floor is named (CHAT-P30); all lints
  green; the work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat Conversation/Membership entity + conversation-list index. Body lists: 11.1
  conversation/membership rows, 12.1 partition; the entity + list-index gates greened; the cross-org floor named
  (CHAT-P30). Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P8 — Membership → write_tuples → zookie in one transaction + the new-enemy guard + the send/membership check gate

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C1 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C1") — the ReBAC-write + zookie +
  gate slice (the second committable unit of the membership half; the entity is CHAT-P7).
- **DEPENDS-ON.** CHAT-P7 (the Conversation/Membership entity the tuple write stamps) + CHAT-P5 (the outbox the
  member_* events ride) + CHAT-P2 (the declared Chat ReBAC fragment). The M1 Identity prompts that ship
  write_tuples / zookie (4.6/4.10), check + CaveatContext (4.2), and the ReBAC engine accepting the Chat fragment
  (4.9, declared in CHAT-P2).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe; one permission model);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the new-enemy zookie guard is a
    drilled property), §7 (one primitive, no third copy of permission-aware reads).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §5 (membership
    writes project tuples via write_tuples in the SAME tx as the membership row + the chat.channel.member_* event,
    stamping the returned zookie — the new-enemy guard), §1.1 (the chat.channel.created/archived/member_added/
    member_removed/linked durable events).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the frozen Chat
    ReBAC fragment: channel.read = member + parent_project->read; the watcher relation).
  - Contracts: contract-index.md rows 4.6 (write_tuples([Δtuple], precondition?) → zookie; atomic; emitted via
    outbox), 4.10 (Consistency/zookie semantics — read-your-writes; the new-enemy guard; zookie-stamped reads
    bypass fail-static), 4.9 (the Chat ReBAC fragment), 4.2 (check + CaveatContext — the membership/send gate),
    2.2 (the outbox co-commit).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C1 membership-writes work) + §3 (the ReBAC-fragment-
    writes row: M2-C0 declare → M4-C1 writes) + §2 (upstream rows 4.6/4.10, 4.9, 4.2).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — the new-enemy / membership-revoke
    correctness is exercised by CHAT-D11 (search-as-non-member, CHAT-P20) and CHAT-D5 (unfurl no-leak, CHAT-P13);
    here the gate is the zookie-stamp-in-tx atomicity + the membership unit drill.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the membership module):
  - Membership change → write_tuples([Δtuple], precondition) → zookie in the SAME PG transaction as the membership
    row + the chat.channel.member_added / member_removed event (via OutboxTx::emit), STAMPING the returned zookie
    on the conversation (conversation.acl_zookie) — the new-enemy guard (4.6/4.10): a just-revoked grant cannot
    read stale on the next unfurl/read.
  - The send / edit / membership permission gate via Id.check(subject, permission, object, zookie?, caveat?)
    (4.2) — every send and membership mutation is gated; the gate is fail-closed.
  - chat.channel.created / archived / linked durable events via the outbox; chat.channel.linked → refs.edge.created
    ("discussed in").
  - FLOOR named: none new — this completes the M4-C1 silent-data-loss floor's membership half. State that the
    cross-org / federated channels follow-on (M5-C-X1 / CHAT-P30) rides the cross-cell bridge (12.6).
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
    CHAT-P13 and CHAT-D11 in CHAT-P20, which depend on this stamp.)
  - The Chat ReBAC fragment runtime writes resolve channel.read = member + parent_project->read correctly (a
    non-member denied; a parent-project reader allowed) — CI.
- **TESTS (required).** Unit tests for: the membership-write-in-one-tx atomicity, the zookie stamp, the
  channel.read = member + parent_project->read resolution. The CDC provider/consumer pair for rows 4.6, 4.10, 4.9.
  A CHAINED test (add member → revoke → read) proving the new-enemy guard, not a single-handler test (EI-01 §4).
  State the cargo-mutants mutation floor for the membership-tx + zookie-stamp core module (mandatory-core).
- **DEFINITION OF DONE.** Membership→write_tuples→zookie is one transaction with the member_* event; the new-enemy
  guard denies post-revoke; the Chat fragment resolves correctly; the send/membership gate is fail-closed; the unit
  + CDC + chained tests pass; the contract-coverage scanner is green; the cross-org floor is named (CHAT-P30); all
  lints green; the work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat membership→write_tuples→zookie + new-enemy guard + send/membership gate.
  Body lists: 4.6/4.10 tuple-write + zookie in one tx, 4.9 fragment runtime writes, 4.2 send/membership gate; the
  membership atomicity + new-enemy guard greened (0 partial membership, 0 stale grants); the cross-org floor named
  (CHAT-P30). Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P9 — The stateless Rust connection-tier gateway + subscribe/resume/resync_required (the zero-loss-across-reconnect backbone)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C2 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C2 — The firehose resume-cursor
  transport + the connection-tier gateway") — the gateway-shell + resume-cursor slice. The firehose-only live
  delivery + the shed order is CHAT-P10.
- **DEPENDS-ON.** CHAT-P5 (the durable store + outbox the firehose reads from) + CHAT-P6 (resync_from =
  MessageStore::resync_from) + CHAT-P3 (the validated firehose scope shape). The M2 Bus prompts that ship the
  firehose transport + the resume-cursor protocol (3.5) GREEN. The M0 substrate prompts that ship the
  cross-language harness shim (1.7), liveness≠readiness (1.3), and serve(AppSpec) + the three surfaces (1.1/1.2).
  The M1 Identity authenticate (4.1 — the gateway resolves a Principal; tenant from token never path, ID-3).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; agent-native);
    ../../external-insights/04-hard-problems.md §2 (build the durable resume-cursor transport FIRST — a relay
    without resume cursors silently loses the gap on a reconnect; §2.2 the zero-loss-across-reconnect property);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — sever the gateway↔firehose and watch
    resume recover the gap), §2 (this is the silent-loss floor for the live tier).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §1–2 (the live
    delivery / resume / resync_required → snapshot path); 01-tech-and-data-model.md (the stateless gateway: live
    sockets + presence + resume cursors only, NO durable store, NO outbox of its own); 03-events-contracts-and-
    glue.md §9 (the gateway has no emit path — it calls the Message Service); 00-overview.md (the TE-21
    connection-tier divergence call).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-J (the firehose
    resume-cursor protocol — subscribe(stream, scope, cursor?), resume(stream, scope, last_seq) backfills
    (last_seq, now], resync_required → *.snapshot; scope is a bounded selector, never *).
  - Contracts: contract-index.md rows 3.5 (the firehose transport + the resume-cursor subscription protocol —
    chat's correctness backbone), 1.7 (the cross-language harness shim — the BEAM hatch bounded by it, a no-op in
    Rust), 1.3 (liveness ≠ readiness — the gateway readiness-gates new connections), 2.6 (resync_from =
    MessageStore::resync_from, the snapshot fallback), 1.1/1.2 (the harness + three surfaces the gateway boots
    from), 4.1 (authenticate — the gateway resolves a Principal).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C2 work, the Rust/BEAM floor) + §1
    (non-negotiability item 1b: lost across a gateway reconnect) + §6 (the first-runnable bar = end of M4-C2) + §5
    (the connection-tier-language floor, the TE-21 build-gate).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D1 (sever gateway↔firehose →
    resume 0 lost/0 dup; last_seq past window → resync_required → snapshot still 0 lost). (CHAT-D4 — the fleet-roll
    drill — is greened in CHAT-P10 where the readiness/shed surface lands.)
- **DELIVERABLE (what to build + exactly where in the repo).** In a gateway sub-crate myelin-chat-gateway (Rust
  default per TE-21):
  - The connection-tier gateway: WS/SSE termination; STATELESS (live sockets + presence + resume cursors only; NO
    durable store, NO outbox of its own — it calls the Message Service for any write). Boots from serve(AppSpec)
    (1.1) with the three surfaces (1.2) and liveness≠readiness (1.3); resolves a Principal via authenticate (4.1),
    tenant from the token never the path (ID-3).
  - subscribe(stream, scope=channel:<id>, cursor?) — bounded, never * (paginated for hot channels);
    resume(stream, scope, last_seq) recovering the gap (last_seq, now]; resync_required → *.snapshot fallback
    (= MessageStore::resync_from) when last_seq exceeds the retention window.
  - The cross-language harness shim (1.7) satisfied as a no-op (Rust); the gateway speaks the Rust EventEnvelope
    on the wire regardless.
  - FLOOR named: connection-tier language = Rust; the BEAM/Phoenix hatch is written-but-closed, bounded by 1.7,
    opened only if CHAT-D3/D4 prove Rust presence-at-scale intractable (a gateway-process swap, not a platform
    rewrite, CHAT-P26). Mega-channel live delivery = firehose subject fan-out with per-view scope bounding; the
    channel-sharded home-node is the named M5 escalation (M5-C-S3 / CHAT-P29). Name each follow-on.
- **CONTRACTS TO IMPLEMENT.** 3.5 the resume-cursor protocol (consumed — chat's live tier subscribes/resumes over
  it with scope=channel:<id>). 1.7 the harness shim (consumed — no-op). 2.6 resync_from (consumed — the snapshot
  fallback). 4.1 authenticate (consumed — the gateway Principal). Implement to the frozen shapes; no local
  divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D1 (sever the gateway↔firehose mid-publish → resume recovers the gap, 0 lost / 0 dup; last_seq past the
    window → resync_required → *.snapshot, still 0 lost) — CI; the consumer-lag + lost-frame telemetry (1.8) is
    the green artifact (0 lost, 0 dup).
  - subscribe scope is bounded (never *; 0 unbounded subscriptions) and the gateway readiness-gates new
    connections (liveness no restart-storm under load) — CI.
- **TESTS (required).** Unit tests for: subscribe scope-bounding (never *), resume gap recovery, the
  resync_required → snapshot fallback. The CDC provider/consumer pair for rows 3.5, 2.6. The drill-harness scenario
  for CHAT-D1. A CHAINED test (subscribe → deliver → sever → resume → assert 0 lost/0 dup). In the Rust default
  state the mutation-score floor for the resume-cursor core module (mandatory-core); if the gateway diverges to
  BEAM (it does not in the default) the 1.7 shim's test obligations stand in for cargo-mutants.
- **DEFINITION OF DONE.** The stateless gateway + the resume-cursor live tier exist and compile; CHAT-D1 emits a
  dated green artifact (0 lost / 0 dup); subscribe is bounded and readiness-gates; the unit + CDC + drill tests
  pass; the contract-coverage scanner is green; the Rust/BEAM + mega-channel floors are named with their follow-ons
  (CHAT-P26, CHAT-P29); all lints green; the work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat stateless Rust gateway + subscribe/resume/resync_required. Body lists: 3.5
  subscribe/resume/scope, 2.6 snapshot fallback, 1.7 TE-21 no-op, 4.1 gateway Principal; CHAT-D1 greened (0 lost/0
  dup); the Rust/BEAM + mega-channel floors named (CHAT-P26, CHAT-P29). Branch first if on default; do not push
  unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P10 — Firehose-only live delivery (message/presence/typing/read-state/partials) + the protected-human-lane shed order

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C2 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C2") — the firehose-only-delivery +
  shed-order slice (the second committable unit of M4-C2; the gateway shell + resume cursor is CHAT-P9). This is
  the first-runnable bar (§6): a message persists with its event (CHAT-P5/D13) and is delivered live with zero loss
  across a reconnect (CHAT-P9/D1).
- **DEPENDS-ON.** CHAT-P9 (the gateway + the resume-cursor live tier) + CHAT-P1 (the firehose-only token set). The
  M0 substrate prompts that ship the protected-human-lane shed order (1.11) + the no-raw-publish lint (1.6).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — humans never queue behind agent runs);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — roll the fleet under a connection
    storm and watch the human lane hold + the agent lane shed; observability is part of the pass).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §7.2 (presence),
    §7 (typing/read-state/streaming over the firehose); 03-events-contracts-and-glue.md §1.2 (the firehose-only
    set; the no-raw-publish + firehose-seam structural separation).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-K (the per-surface
    shed budgets), ADR-16 (the protected-human-lane shed order).
  - Contracts: contract-index.md rows 1.11 (the protected-human-lane shed order + per-surface shed budget floors),
    3.5 (the firehose the live frames ride), 1.6 (the no-raw-publish lint — the firehose-only set never touches the
    durable bus), 1.3 (readiness gates new connections under storm).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C2 firehose-only delivery + shed-order work) + §1
    (non-negotiability item 1b) + §6 (first-runnable) + §5 (the per-surface shed-budget floor).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D4 (roll the gateway fleet under
    a connection storm → bounded reconnect; resume completes for all; no loss; readiness gates; liveness no
    restart-storm). CHAT-D4 is the TE-21 build-gate drill.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat-gateway:
  - Live message delivery, presence, typing, fine-grained read-state, streaming partials — FIREHOSE-ONLY, never
    the durable bus (the no-raw-publish lint + the firehose seam keep them off structurally).
  - The protected-human-lane shed order (ADR-16, 1.11) + the per-surface shed-budget FLOOR (OQ-K): speculative /
    presence shed first, message delivery last; humans never queue behind agent runs.
  - FLOOR named: the connection-tier language = Rust; per-surface shed budgets = named floor numbers, tuned by
    CHAT-D3/D4 in M5-C-S1 (CHAT-P26). Name the follow-on.
- **CONTRACTS TO IMPLEMENT.** 1.11 the shed order + per-surface budget floor (owned — chat's connection-storm +
  agent-mention-storm budgets). 3.5 the firehose (consumed — the live frames). Implement to the frozen shapes; no
  local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D4 (roll the gateway fleet under a connection storm → bounded reconnect rate; resume completes for ALL;
    no message loss; readiness gates new connections; liveness no restart-storm) — SCHED; the reconnect-rate +
    shed-count signals are the green artifact. This is the TE-21 build-gate drill.
  - The firehose-only events never touch the durable bus (the no-raw-publish lint green; 0 firehose frames on the
    durable bus) — CI.
  - The shed order holds: under shed pressure, speculative/presence shed before message delivery; the human lane
    is last (0 human-lane drops while an agent lane has budget) — CI/SCHED.
- **TESTS (required).** Unit tests for: the firehose-only routing (no durable-bus publish), the shed-order
  priority. The CDC provider/consumer pair for rows 1.11, 3.5. The drill-harness scenario for CHAT-D4. State the
  cargo-mutants mutation-score floor for the shed-order module if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The firehose-only live delivery + the shed order exist and compile; CHAT-D4 emits a dated
  green artifact (bounded reconnect, no loss); the no-raw-publish lint is green (firehose frames off the durable
  bus); the shed order holds (humans last); the unit + CDC + drill tests pass; the contract-coverage scanner is
  green; the shed-budget floor is named (CHAT-P26); all lints green; the work is committed. This is the
  first-runnable bar (§6). No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat firehose-only live delivery + protected-human-lane shed order. Body lists:
  1.11 shed order + budget floor, 3.5 firehose live frames; CHAT-D4 greened (bounded reconnect, no loss); the
  no-raw-publish + shed-order gates greened; the shed-budget floor named (CHAT-P26). Branch first if on default;
  do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P11 — The message body over the frozen myelin-content Chat subset (render(parse(md))===md) + the inline nodes → refs.edge.created

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C3 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C3 — The composer + message content
  over the frozen myelin-content subset") — the content-core + inline-edges slice. The composer UI (slash menu,
  autocomplete, paste-unfurl, draft) + per-message CAS is CHAT-P12.
- **DEPENDS-ON.** CHAT-P6 (the message body fields + #sub the content writes into). The M2 prompts that freeze
  myelin-content (13.1) + the WASM render core. The M2 Refs prompts (the three inline ref nodes produce
  refs.edge.created, 5.4).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (design comes before implementation for anything with a frontend; the wireframes are
    PRESERVED from Phase 4 and are the build-to); ../../external-insights/01-process-and-quality-doctrine.md §3
    (the render(parse(md))===md round-trip is a quantified gate).
  - Architecture: ../04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md §1.4 (the body =
    markdown-subset string + the three structured nodes); 04-views-cli-and-api.md §1 (the one editor render path);
    03-events-contracts-and-glue.md §1.1 (chat.message.edited carries the new edited_seq — the CAS lands in
    CHAT-P12).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-2 (the frozen
    myelin-content taxonomy + the WASM compile target; Chat consumes a strict SUBSET).
  - Contracts: contract-index.md row 13.1 (the myelin-content taxonomy frozen — the Chat subset: paragraph,
    heading(1..3), bullet_list, ordered_list, task_list, blockquote, code_block, callout, table, divider, image +
    the three inline nodes mention/artifact_ref/embed; EXCLUDES db_view, sync_block, toggle;
    render(parse(md))===md; the WASM render core), 5.4 (refs.edge.created — the three inline ref nodes are the
    producers).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C3 content-subset work + exit) + §6
    (first-useful progression).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — the chat instance of KN-D2 (the
    content-core round-trip gate); the refs.edge.created uniformity check.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the content module):
  - The message body over the frozen Chat SUBSET of myelin-content (13.1), REUSING the WASM-compiled Rust content
    core (one editor render path; render(parse(md)) === md). The Chat subset is paragraph, heading(1..3),
    bullet_list, ordered_list, task_list, blockquote, code_block, callout, table, divider, image + the three inline
    nodes mention / artifact_ref / embed; it EXCLUDES db_view, sync_block, toggle.
  - The structured mention / artifact_ref / embed nodes parse to the frozen node shapes and produce
    refs.edge.created uniformly (5.4).
  - FLOOR named: chat consumes a strict SUBSET; chat must NOT add a node outside the frozen subset (a needed change
    is a whole-workspace contract PR, escalated). State this so no agent extends the subset locally.
- **CONTRACTS TO IMPLEMENT.** 13.1 the myelin-content Chat subset + the WASM render core (consumed — the message
  body reuses the one render path; render(parse(md))===md). 5.4 refs.edge.created (owned — the chat inline nodes
  emit edges uniformly). Implement to the frozen shapes; chat must not add a node outside the frozen subset.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - render(parse(md)) === md holds 100% for the Chat subset (the chat instance of KN-D2 / the content-core
    round-trip gate) — CI; the round-trip-mismatch signal = 0 over the corpus.
  - Structured mention / artifact_ref / embed nodes parse to the frozen node shapes and produce refs.edge.created
    uniformly (0 nodes producing a malformed or missing edge) — CI.
  - The Chat subset excludes db_view/sync_block/toggle (0 excluded nodes accepted by the chat parser) — CI.
- **TESTS (required).** Unit tests for: the Chat-subset parse/render round-trip, the three inline nodes →
  refs.edge.created, the excluded-node rejection. The CDC pair for 13.1 (the Chat subset) and 5.4. State the
  cargo-mutants mutation floor for the content-subset parse module if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The message body over the frozen subset exists and compiles; render(parse(md)) === md is
  green 100% for the Chat subset; the inline nodes produce edges uniformly; excluded nodes are rejected; the unit +
  CDC tests pass; the contract-coverage scanner is green; the strict-subset floor is named; all lints green; the
  work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat message body over the frozen myelin-content subset + inline nodes → edges.
  Body lists: 13.1 the Chat subset + WASM render core, 5.4 inline nodes → edges; render(parse(md))===md greened
  100%; the strict-subset floor named. Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### CHAT-P12 — The composer UI (slash menu + @/# autocomplete + paste-URL→unfurl + draft) + the per-message CAS (no CRDT)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C3 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C3") — the composer-UI + edit-CAS
  slice (the second committable unit of M4-C3; the content core is CHAT-P11).
- **DEPENDS-ON.** CHAT-P11 (the content subset the composer edits) + CHAT-P6 (the draft store, per-subject-DEK
  encrypted) + CHAT-P10 (the live delivery for optimistic send). The M2 Search prompts (the @-mention /
  #-artifact autocomplete is Search-backed).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (design comes before implementation; the wireframes are PRESERVED and are the build-to;
    top-of-the-line UX); ../../external-insights/05-ux-and-design.md (the design-language bar);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — drive the composer in a
    browser before claiming it).
  - Architecture: ../04-subsystem-architectures/chat/design/wireframes.md (S3 composer — the build-to; with
    empty/loading/error states) + design/user-flows.md + design/information-architecture.md;
    ../04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md §1.4 (the per-message edited_seq
    CAS); 04-views-cli-and-api.md §1 (S3 the composer); 03-events-contracts-and-glue.md §1.1
    (chat.message.edited carries the new edited_seq).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-2 (the frozen
    myelin-content taxonomy — the composer edits the SUBSET).
  - Contracts: contract-index.md row 13.1 (the Chat subset the composer edits), 5.4 (the inline nodes the
    autocomplete inserts), 6.1 (the Search query surface backing @/# autocomplete — consumed).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C3 composer work + exit) + §5 (the per-message-CAS
    floor: chat does NOT promote to CRDT — that is Knowledge's) + §6 (first-useful progression).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — the per-message CAS rejection check; the
    browser-driven composer check (EI-01 §4).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the composer module) and the
  chat frontend package (the web app, built to the PRESERVED S3 wireframe):
  - The composer over the frozen Chat subset (CHAT-P11's content core): the / slash menu, @-mention + #-artifact
    autocomplete (Search-backed via the M2 Search query surface, 6.1), paste-URL → unfurl, draft persistence (the
    draft store from CHAT-P6, per-subject-DEK encrypted).
  - Per-message CAS on edit (edited_seq) — NO collaborative-edit engine (chat is single-author-per-message; the
    CRDT is Knowledge's, not chat's). A stale edit (edited_seq mismatch) is rejected with current state.
  - FLOOR named: per-message CAS (single-author; no merge) — chat does NOT promote to CRDT (n/a follow-on; the
    related OQ-L comment-threading consolidation is M5-C-X2 / CHAT-P31, not a CRDT). State this explicitly so the
    next agent does not build a chat CRDT.
- **CONTRACTS TO IMPLEMENT.** 13.1 the Chat subset (consumed — the composer edits it). 5.4 refs.edge.created
  (consumed — the autocomplete inserts inline ref nodes). 6.1 the Search query surface (consumed — @/#
  autocomplete). Implement to the frozen shapes; chat must not add a node outside the frozen subset.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The per-message CAS rejects a stale edit (edited_seq mismatch → rejected with current state; 0 silent
    overwrite of a message) — CI.
  - The @/# autocomplete is Search-backed (0 chat-private mention/artifact index; the autocomplete query goes
    through the Search surface) — CI/structural.
  - The S3 composer is driven in a browser (the / slash menu, the @/# autocomplete, paste-URL→unfurl, draft
    persistence) — recorded yes/no/partial per EI-01 §4.
- **TESTS (required).** Unit tests for: the edited_seq CAS rejection, the draft persistence round-trip. A
  browser-driven check of the S3 composer (the / slash menu, the @/# autocomplete, paste-URL→unfurl) per EI-01 §4
  — record yes/no/partial honestly. The CDC pair for 6.1 (the Search-backed autocomplete) consumption. State the
  cargo-mutants mutation floor for the edited_seq CAS module if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The composer + the per-message CAS exist and compile; the CAS rejects stale edits; the
  @/# autocomplete is Search-backed; the S3 composer is driven in a browser (yes/no/partial recorded); the unit +
  CDC tests pass; the no-chat-CRDT floor is named (CHAT-P31 for the related OQ-L follow-on); all lints green; the
  work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat composer (slash + @/# autocomplete + paste-unfurl + draft) + per-message
  CAS. Body lists: 13.1 the Chat subset, 6.1 Search-backed autocomplete, the edited_seq CAS; the per-message-CAS-
  no-CRDT floor named; the S3 composer browser-driven (yes/no/partial). Branch first if on default; do not push
  unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P13 — The Unfurl Service: the shared per-ref projection cache + the per-viewer list_objects/check gate (the no-leak floor)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C4 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C4 — The unfurl service: cheap
  per-viewer permission-aware unfurls") — the cache + per-viewer-gate slice (the no-leak core). The erasure-safe +
  invalidation + anchor-stability slice is CHAT-P14; the project() + edge-producer slice is CHAT-P15.
- **DEPENDS-ON.** CHAT-P8 (membership + the zookie stamp the unfurl permission reads) + CHAT-P11 (the
  artifact_ref/embed nodes that produce the refs). The M2 Refs prompts that ship resolve(ref, viewer, mode) + the
  4-step tombstone ladder (5.7), GREEN. The M1 Identity prompt that ships list_objects with the SetExpr push-down
  (4.3) + check (4.2), GREEN. The M3 Git + Knowledge prompts producing the artifacts to unfurl.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (chat references any other artifact — the differentiator) §3 (top-of-the-line UX);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the no-leak drill forces a
    confidential title to a viewer lacking access and watches the tombstone), §7 (abstract at the third copy — the
    unfurl reuses the Refs resolve chokepoint, chat never re-implements permission-aware resolution).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §4 (the Unfurl
    Service — the shared per-ArtifactRef projection cache viewer-independent, gated by a per-viewer
    list_objects/check; lazy-on-viewport; ONE cache entry per ref, never per (ref, viewer)); 03-events-contracts-
    and-glue.md §2 (the #sub ladder outcomes for chat: live/gone/erased); 05-hard-problems.md §4 (the no-leak
    subtlety that separates a real implementation from a demo).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-E (the
    list_objects SetExpr push-down lowering to a JOIN over the candidate id column), §X-4 (the 4-step tombstone
    ladder).
  - Contracts: contract-index.md rows 5.2 (resolve(ref, viewer, mode) → Projection | Tombstone; cell-local), 5.7
    (the 4-step tombstone ladder), 4.3 (list_objects SetExpr — the membership-class precompute + the no-N+1
    candidate filter), 4.2 (check — the per-viewer gate).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C4 work, the unfurl-cache + per-viewer-gate
    bullets) + §1 (non-negotiability item 2: PII leak through unfurls) + §6 (first-useful) + §5 (the canvas =
    embedded Knowledge page floor, M4-C4).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D5 (confidential unfurl →
    tombstone, title never present — ladder step 1). (CHAT-D6/D7/D18 are greened in CHAT-P14.)
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the unfurl module):
  - The Unfurl Service: a shared, per-ArtifactRef projection cache (viewer-INDEPENDENT content — ONE cache entry
    per ref, never per (ref, viewer)), gated by a per-viewer list_objects / check (lowering the frozen SetExpr to
    a JOIN over the candidate id column) — no leak.
  - Lazy-on-viewport resolution (resolve only what is on screen); calls Refs resolve(ref, viewer, mode) over the
    one 4-step tombstone ladder (5.7). For chat refs the ladder outcomes are live / gone / erased (a message is
    content-addressed by stable id, no moved/outdated).
  - Membership-as-permission class precompute via the frozen list_objects Filter (one class decision, not N).
  - FLOOR named: the canvas = an embedded/pinned Knowledge page (ArtifactRef, not a Chat editor) — the lean is
    firm: embed, not editor (M4/M5, M5-C-X2-adjacent). State this so no agent builds a chat-side canvas editor.
- **CONTRACTS TO IMPLEMENT.** 5.2/5.7 resolve + the ladder (consumed — the unfurl chokepoint). 4.3 list_objects
  SetExpr (consumed — the per-viewer gate + the membership class precompute, lowered to a JOIN). 4.2 check
  (consumed). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D5 (notify/unfurl a confidential artifact to a viewer lacking access → tombstone rendered, title NEVER
    present — the 4-step ladder step 1) — CI; the leaked-title signal = 0.
  - The cache is one-entry-per-ref (never per (ref, viewer)): a confidential ref has ONE cache entry, gated per
    viewer (0 per-viewer cache entries; 0 viewer-content baked into the cache) — CI.
  - The per-viewer gate lowers the SetExpr to a JOIN over the candidate id column (no N+1; 0 post-filter passes) —
    CI.
- **TESTS (required).** Unit tests for: the one-cache-entry-per-ref (never per (ref, viewer)) invariant, the
  per-viewer gate via list_objects SetExpr → JOIN, the 4-step ladder outcomes (live/gone/erased). The CDC pair for
  5.2, 5.7, 4.3. The drill-harness scenario for CHAT-D5. A CHAINED test (resolve as member → revoke → re-resolve →
  assert tombstone, 0 leak) per EI-01 §4. State the cargo-mutants mutation floor for the per-viewer-gate core
  module (mandatory-core — the no-leak property).
- **DEFINITION OF DONE.** The Unfurl Service + the per-viewer gate exist and compile; CHAT-D5 emits a dated green
  artifact (0 leaked title); the cache is one-entry-per-ref with no leak; the gate lowers to a JOIN with no N+1;
  the unit + CDC + drill tests pass; the contract-coverage scanner is green; the canvas-is-an-embed floor is named;
  all lints green; the work is committed. No gate greened by a weakened threshold or an inverted assertion.
- **COMMIT.** Header: P-<NNN> M4: Chat unfurl service + per-viewer list_objects/check gate (the no-leak floor).
  Body lists: 5.2/5.7 the resolve chokepoint + ladder, 4.3 the SetExpr JOIN gate, 4.2 check; CHAT-D5 greened (0
  leak); the one-cache-entry-per-ref invariant proven; the canvas-is-an-embed floor named. Branch first if on
  default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P14 — Erasure-safe unfurls + bus-driven cache invalidation + #sub anchor stability (CHAT-D6 / D7 / D18)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C4 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C4") — the erasure-safe +
  invalidation + anchor-stability slice (the second committable unit of M4-C4; the cache + gate is CHAT-P13).
- **DEPENDS-ON.** CHAT-P13 (the unfurl cache + the per-viewer gate the invalidation busts) + CHAT-P10 (the live
  firehose for card busting) + CHAT-P6 (the message-<id> #sub the anchor stability rides). CI's ci.check.updated
  producer (5.9, lands in M4, CI sequenced first within M4) for unfurl invalidation. The M2 Refs *.erased pointer
  events.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction — a card never freezes a third party's PII);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — erase a third party in a card and
    watch the tombstone on next render; bust a card on a check change and watch the live update).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §4 (the bus-driven
    invalidation; the cache re-resolves live; never a durable snapshot); 03-events-contracts-and-glue.md §1.3 (the
    unfurl-invalidation consumer matching *.updated / *.erased / ci.check.updated), §2 (the #sub ladder outcomes
    live/gone/erased; the message-<id> anchor stays stable across edits, degrades to Tombstone on delete).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-4 (the 4-step
    tombstone ladder — the erased outcome).
  - Contracts: contract-index.md rows 5.7 (the 4-step ladder — the erased step), 5.9 (ci.check.updated — the
    unfurl-invalidation event), 3.5 (the firehose for the live card update), 2.7 (*.erased tombstones — consumed
    for invalidation).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C4 invalidation + erasure-safe + anchor-stability
    bullets) + §1 (non-negotiability item 2: PII leak) + §6 (first-useful).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D6 (erase a third party in a
    card → tombstone on next render, 0 recoverable PII, no durable snapshot, re-resolves live → erased), CHAT-D7
    (an artifact's ci.check.updated / *.updated → the shared per-ref cache busts; viewers showing the card get a
    live firehose update within budget), CHAT-D18 (edit a referenced message → the message-<id> anchor stays
    stable/live; delete → embed degrades to Tombstone carrying the root, never dangles).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the unfurl-invalidation
  module):
  - Bus-driven invalidation on *.updated / ci.check.updated / *.erased pointer events (precise; TTL the backstop);
    viewers currently showing the card get a live firehose update within budget.
  - Erasure-safe cards: erasing a third party rendered in a card produces a tombstone on next render, 0 recoverable
    PII, NO durable snapshot; the cache re-resolves live → erased.
  - The #sub anchor stability: editing a referenced message keeps the message-<id> anchor stable/live; deleting it
    degrades the embed to a Tombstone carrying the root, never dangles.
  - FLOOR named: none new — the unfurl cache TTL + the membership-class refresh cadence are measured-not-predicted
    tunables (R-C4), tuned against telemetry, not a separate milestone. State this.
- **CONTRACTS TO IMPLEMENT.** 5.7 the ladder erased outcome (consumed). 5.9 ci.check.updated (consumed — unfurl
  invalidation only). 3.5 the firehose (consumed — the live card update). 2.7 *.erased (consumed). Implement to the
  frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D6 (erase a third party rendered in a card → tombstone on next render, 0 recoverable PII; NO durable
    snapshot; the cache re-resolves live → erased) — CI; the recoverable-PII-in-cache signal = 0.
  - CHAT-D7 (an artifact's ci.check.updated / *.updated → the shared per-ref cache busts; viewers showing the card
    get a live firehose update within budget) — CI; the cache-staleness + update-latency signals within budget.
  - CHAT-D18 (edit a referenced message → message-<id> anchor stays stable/live; delete → embed degrades to a
    Tombstone carrying the root, never dangles) — CI; the dangling-anchor signal = 0.
- **TESTS (required).** Unit tests for: the bus-driven invalidation precision, the erasure-safe re-resolve (no
  durable snapshot), the anchor stability across edit/delete. The CDC pair for 5.9, 2.7. The drill-harness
  scenarios for CHAT-D6/D7/D18. A CHAINED test (resolve → erase third party → re-resolve → assert tombstone, 0
  recoverable PII) per EI-01 §4. State the cargo-mutants mutation floor for the invalidation/erasure-safe core
  module (mandatory-core — the no-recoverable-PII property).
- **DEFINITION OF DONE.** The bus-driven invalidation + the erasure-safe cards + the anchor stability exist and
  compile; CHAT-D6/D7/D18 each emit a dated green artifact (0 recoverable PII, live bust within budget, 0 dangling
  anchor); the unit + CDC + drill tests pass; the contract-coverage scanner is green; the cache-TTL tunable is
  named; all lints green; the work is committed. No gate greened by a weakened threshold or an inverted assertion.
- **COMMIT.** Header: P-<NNN> M4: Chat erasure-safe unfurls + bus-driven invalidation + #sub anchor stability.
  Body lists: 5.9 unfurl invalidation, 2.7 *.erased, 3.5 the live bust; CHAT-D6/D7/D18 greened (0 recoverable PII,
  live bust within budget, 0 dangling); the cache-TTL tunable named. Branch first if on default; do not push unless
  asked. End with the Co-Authored-By trailer.

---

### CHAT-P15 — project(ref, viewer) for chat/{channel,message,thread} + chat as the densest refs.edge.created producer

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C4 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C4") — the project() + edge-producer
  slice (the third committable unit of M4-C4; the cache+gate is CHAT-P13, the invalidation is CHAT-P14).
- **DEPENDS-ON.** CHAT-P13 (the per-viewer gate project() reuses) + CHAT-P8 (membership for the per-viewer check) +
  CHAT-P11 (the inline nodes + chat.channel.linked that produce the edges). The M2 Refs prompt that ships project
  REQUIRED (5.6) + refs.edge.created (5.4), GREEN. The M2 Notif humanise (7.3) for the projected title.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §2 (chat references any other artifact); ../../external-insights/01-process-and-quality-
    doctrine.md §7 (one primitive — project() is the ONLY way other subsystems read about a chat artifact; no
    cross-DB).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §3
    (project(ref, viewer) — the per-viewer pre-permission-checked projection, never the body), §1.1 (the
    chat.channel.linked event); 02-internals-and-algorithms.md §4 (chat the densest edge producer).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-I (cross-cell
    resolution is always cell-local — project() never reaches across cells).
  - Contracts: contract-index.md rows 5.6 (project REQUIRED on every subsystem — chat implements it for
    chat/{channel,message,thread}), 5.4 (refs.edge.created — chat is the densest producer), 4.2 (check — the
    per-viewer gate project() applies), 7.3 (humanise — the projected title).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C4 project + edge-producer bullets) + §3 (the
    project row M4-C4) + §6 (first-useful).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — project() returning Projection|Tombstone
    never the body is asserted here; the no-leak proof is CHAT-D5 (CHAT-P13) which project() inherits the gate
    from.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the project + edge-producer
  modules):
  - project(ref, viewer) for chat/{channel,message,thread} (5.6) — the ONLY way other subsystems read about a chat
    artifact (no cross-DB); per-viewer pre-permission-checked → Projection | Tombstone (never the body); title
    humanised via humanise (7.3).
  - Chat as the densest refs.edge.created producer (artifact-linked channels, embeds, mentions) — wired from the
    CHAT-P11 inline nodes + the chat.channel.linked event.
  - FLOOR named: none new — project() resolution is always cell-local (OQ-I); the cross-cell pointer follow-on
    (cross-org channels, CHAT-P30) consumes the bridge, not project() directly. State this.
- **CONTRACTS TO IMPLEMENT.** 5.6 project(ref, viewer) for chat artifacts (owned). 5.4 refs.edge.created (owned —
  chat the densest producer). 4.2 check (consumed — the per-viewer gate). 7.3 humanise (consumed — the projected
  title). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - project(ref, viewer) returns Projection | Tombstone, NEVER the body (0 body bytes in any projection) — CI; the
    body-in-projection signal = 0.
  - project() is per-viewer pre-permission-checked (a non-member viewer gets a Tombstone; 0 leaked projections to
    non-members) — CI.
  - Chat produces refs.edge.created uniformly from every inline node + chat.channel.linked (0 missing edges for a
    fixture corpus) — CI.
- **TESTS (required).** Unit tests for: project() returning Projection|Tombstone never the body, the per-viewer
  permission check, the edge-producer uniformity. The CDC pair for 5.6, 5.4. State the cargo-mutants mutation floor
  for the project() core module (mandatory-core — the never-the-body property).
- **DEFINITION OF DONE.** project() + the edge producer exist and compile; project() never returns the body and is
  per-viewer gated; chat produces edges uniformly; the unit + CDC tests pass; the contract-coverage scanner is
  green; the cell-local floor is named; all lints green; the work is committed. No gate greened by a weakened
  threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat project(ref, viewer) + densest refs.edge.created producer. Body lists: 5.6
  project, 5.4 chat the densest edge producer, 4.2 the per-viewer gate, 7.3 humanise title; project()
  never-the-body + per-viewer-gate greened; the cell-local floor named. Branch first if on default; do not push
  unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P16 — The read-state hot path (Valkey hot markers + batched PG flush; cache-never-authoritative)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C5 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C5 — The read-state hot path +
  Activity-as-view") — the read-state-service slice. The fanout-class boundary + Activity-as-view is CHAT-P17.
- **DEPENDS-ON.** CHAT-P5 (the conversation log unread derives from) + CHAT-P10 (the firehose for
  read_state.updated). The M2 Notif prompts that ship read-state truth (7.2), GREEN. The M1 GDPR holder trait
  (10.1) for the read-state store registration.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; one inbox); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it — drop Valkey and watch PG be authoritative).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §5 (the
    Read-state Service — Valkey hot markers + counters; batched eventually-consistent flush to the PG durable
    record; Valkey NEVER authoritative; unread as a bounded range read count(id > last_read); firehose-only
    chat.read_state.updated).
  - Contracts: contract-index.md rows 7.2 (read-state truth), 3.5 (the firehose-only read_state.updated), 10.1
    (the read-state store is a PersonalDataHolder).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C5 read-state-service work + exit) + §6
    (first-useful) + §5 (the read-state batched-flush cadence tunable R-C3).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D12 (flush + drop Valkey
    mid-session → the PG record is authoritative; a marker is at-worst slightly stale; unread counts recompute
    correctly).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the read-state module):
  - The Read-state Service: Valkey hot markers + counters; batched eventually-consistent flush to the PG durable
    record (Valkey NEVER authoritative); unread derived as a bounded range read (count(id > last_read)), never
    write-fanned-out; firehose-only chat.read_state.updated events; the store registered as a PersonalDataHolder.
  - FLOOR named: none new — the read-state batched-flush cadence + the Notif.mark(item, read) trigger are
    measured-not-predicted tunables (R-C3), tuned against telemetry, not a separate milestone. State this.
- **CONTRACTS TO IMPLEMENT.** 7.2 read-state truth (consumed). 3.5 firehose read_state.updated (consumed). 10.1
  the holder (owned — over the read-state store). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D12 (flush + drop Valkey mid-session → the PG record is authoritative; a marker is at-worst slightly
    stale; unread counts recompute correctly) — CI; the lost-read-state signal = 0 (PG authoritative).
  - Unread is a bounded range read count(id > last_read), never write-fanned-out (0 per-member unread writes on an
    ambient post) — CI.
- **TESTS (required).** Unit tests for: the Valkey-never-authoritative flush, the unread bounded-range-read, the
  holder registration. The CDC pair for 7.2, 10.1. The drill-harness scenario for CHAT-D12 (a CHAINED
  flush→drop→recompute). State the cargo-mutants mutation floor for the flush module if mandatory-core; if not, say
  so.
- **DEFINITION OF DONE.** The Read-state Service exists and compiles; CHAT-D12 emits a dated green artifact (PG
  authoritative, counts recompute); unread is a bounded range read; the store is a holder; the unit + CDC + drill
  tests pass; the contract-coverage scanner is green; the read-state-cadence tunable is named; all lints green; the
  work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat read-state hot path (Valkey+PG, cache-never-authoritative). Body lists: 7.2
  read-state truth, 3.5 firehose read_state.updated, 10.1 the holder; CHAT-D12 greened (PG authoritative); the
  flush-cadence tunable named. Branch first if on default; do not push unless asked. End with the Co-Authored-By
  trailer.

---

### CHAT-P17 — The fanout-class boundary (write-fanout vs read-fanout; celebrity-fanout mitigation) + Activity-as-view

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C5 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C5") — the fanout-boundary +
  Activity-as-view slice (the second committable unit of M4-C5; the read-state service is CHAT-P16).
- **DEPENDS-ON.** CHAT-P16 (the read-state service the ambient read-fanout derives from) + CHAT-P3 (the registered
  define_notif_rule set + the fanout-class declaration). The M2 Notif prompts that ship list_inbox (7.1), humanise
  (7.3), define_notif_rule (7.6), GREEN. The M1 Identity prompt that ships list_subjects against the authz reverse
  index (4.4) for watcher resolution at 50k-member density.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale — the celebrity-fanout mitigation; one inbox);
    ../../external-insights/01-process-and-quality-doctrine.md §7 (Activity is a VIEW into the one inbox, never a
    second store — no third copy of read-state).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §4 (the fanout
    boundary chat owns — write-fanout the bounded high-signal set, read-fanout the unbounded ambient set; the
    celebrity-fanout mitigation — a 100k-member post does ZERO per-member inbox writes); 02-internals-and-
    algorithms.md §5.3 (Activity / Mentions = a list_inbox filter, never a 2nd store); 04-views-cli-and-api.md §1
    (S6 Activity = Notif.list_inbox(filter)).
  - Contracts: contract-index.md rows 7.1 (list_inbox — the ONE inbox; Activity is a filter), 7.6
    (define_notif_rule — mentioned/replied/thread_watched/approval_requested), 4.4 (list_subjects(channel,
    watcher) against the authz reverse index, performant at 50k-member density), 7.3 (humanise — the notify
    strings).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §1 (the watcher
    relation; list_subjects density).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C5 fanout-class + Activity-as-view bullets) + §3
    (the fanout-class declaration row; humanise/notif-rule wire) + §6 (first-useful).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — the celebrity-fanout property (a
    100k-member post does 0 per-member inbox writes) + the Activity-is-a-filter structural check.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the fanout + activity
  modules):
  - The fanout-class declaration (arch 03 §4) wired to behaviour: WRITE-FANOUT the bounded high-signal set
    (mentions via the structured mention(Principal) node, DMs, thread-replies-to-you, HITL-for-you, keyword
    matches) → Signals → Notif; READ-FANOUT the unbounded ambient set (channel/thread activity, unread) via the
    per-conversation log + lazy unread, watchers resolved by list_subjects(channel, watcher) (4.4). A 100k-member
    announcement does ZERO per-member inbox writes on a post (the celebrity-fanout mitigation).
  - Activity / Mentions (S6) = a list_inbox filter (subsystem ∈ {chat} ∧ reason ∈ {mentioned, replied,
    thread_watched, approval_requested}) — NEVER a second store; one read-state truth, linked to chat's
    scroll-state at the mention; the wire of the M2-C0-declared notif rules (7.6) + humanise (7.3).
  - FLOOR named: none new. State that Activity is a view, never a store, and must remain so.
- **CONTRACTS TO IMPLEMENT.** 7.1 list_inbox as the Activity filter (consumed — Activity is a view). 7.6
  define_notif_rule (consumed — the wire of the M2-C0-declared rules). 4.4 list_subjects(channel, watcher)
  (consumed — read-fanout watcher resolution). 7.3 humanise (consumed — the notify strings). Implement to the
  frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The celebrity-fanout property: a 100k-member channel post does ZERO per-member inbox writes (the write-fanout
    counter on an ambient post = 0) — CI; the per-member-write signal = 0 for ambient.
  - The write-fanout vs read-fanout class decision is correct (a mention/DM/HITL → write-fanout; channel/thread
    activity → read-fanout; 0 misclassified events) — CI.
  - Activity is a list_inbox filter, not a second store (0 chat-private activity store) — CI (a lint/structural
    check that no second read-state store exists).
- **TESTS (required).** Unit tests for: the write-fanout-vs-read-fanout class decision, the celebrity-fanout
  0-per-member-write, Activity = a list_inbox filter, watcher resolution via list_subjects. The CDC pair for 7.1,
  7.6, 4.4. A test proving a 100k-member ambient post does 0 per-member writes. State the cargo-mutants mutation
  floor for the fanout-class core module if mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The fanout boundary + Activity-as-view exist and compile; the celebrity-fanout property
  holds (0 per-member writes on an ambient post); the class decision is correct; Activity is a filter not a store;
  the unit + CDC tests pass; the contract-coverage scanner is green; all lints green; the work is committed. No
  gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat fanout boundary (write/read-fanout + celebrity mitigation) + Activity-as-
  view. Body lists: 7.1/7.6 the inbox + notif rules wired, 4.4 watcher resolution, 7.3 humanise; the
  celebrity-fanout 0-per-member-write property proven; Activity-is-a-filter proven. Branch first if on default; do
  not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P18 — The HITL approval-card bridge (per-effect idem_key; withhold→approve→resume; exactly-once across a multi-day kill)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C6 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C6 — The HITL approval-card bridge
  + the agent ToolDef set") — the HITL-card slice (the exactly-once correctness core). The agent ToolDef set +
  EffectApi routing + reserve/settle + dry-run is CHAT-P19.
- **DEPENDS-ON.** CHAT-P8 (the check gate) + CHAT-P12 (the card renders in a message) + CHAT-P16 (the card renders
  in the inbox). The M2 Workflow prompts that ship DurableExecutor::signal + the per-effect idem_key + the durable
  signal for multi-day HITL (9.1/9.4), GREEN. The M1 Identity mint_run_token / revoke (4.7) for the resume token.
  The M2 Notif humanise (7.3) for the card strings.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native; HITL where security/cost/irreversible-scope implications exist);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — a gated effect that mutates before
    approval, or runs twice across a kill, is the failure the drill forces), §8 (the human sign-off is the
    bottleneck — the approval card IS the decision surface); §4 (chain the mutations — request → kill → approve
    days later → exactly-once).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §5 (the HITL Card
    Service — render the card in thread + Notif inbox; gate the click with check(human, approve, run); post
    DurableExecutor::signal with the per-effect idem_key; a declined effect is WITHHELD; timeout auto-denies;
    resume under a freshly-minted attenuated token); 04-views-cli-and-api.md §1 (S11 the HITL card, S3).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-F (the per-effect
    idem_key rule — card_id single, card_id:<effect_idx> multi/partial; a double-click is one approval, a partial
    approval is well-defined).
  - Contracts: contract-index.md rows 9.1 (DurableExecutor::signal idempotent on idem_key, the per-effect rule),
    9.4 (the durable signal for multi-day HITL), 4.2 (check(human, approve, run) — the approve gate), 4.7
    (mint_run_token — the resume token), 7.3 (humanise — the card strings), 8.2 (EffectApi::apply — one apply per
    APPROVED effect; the routing the card posts into, owned in CHAT-P19).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C6 HITL-card work) + §1 (non-negotiability item 4:
    HITL approval correctness) + §6 (first-useful = end of M4-C6).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D9 (request approval, kill Chat
    + Workflow mid-wait, approve days later → the gated tool runs exactly once; double-click is one approval; deny
    withholds with no mutation; timeout auto-denies; resume under a fresh token), CHAT-D10 (a multi-effect card
    approved 2-of-3 → the 2 resume, the 1 withheld, each independent idem_key=card_id:<idx>; no effect runs twice;
    the withheld never mutates).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the HITL module):
  - The HITL Card Service: render the approval card (in thread + Notif inbox, the one inbox C-9); gate the click
    with Id.check(human, approve, run) (4.2); post DurableExecutor::signal(run, name, payload, idem_key) with the
    frozen per-effect idem_key (card_id single / card_id:<effect_idx> multi); a declined effect is WITHHELD (one
    EffectApi::apply per APPROVED effect); timeout auto-denies; resume runs under a freshly-minted attenuated token
    (4.7). Chat owns the CARD, not the wait/timer/budget/sandbox.
  - FLOOR named: none new — chat owns the card; the wait/timer/budget/sandbox are the M2 Workflow/Agent/Storage
    primitives. State that chat must not re-implement a wait or a budget.
- **CONTRACTS TO IMPLEMENT.** 9.1/9.4 DurableExecutor::signal + the per-effect idem_key + the durable HITL signal
  (consumed — the card posts the signal). 4.2 the approve gate (consumed). 4.7 mint_run_token (consumed — the
  resume token). 7.3 humanise (consumed — the card strings). 8.2 EffectApi::apply (consumed — one apply per
  approved effect; the full routing is owned in CHAT-P19). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D9 (request approval, kill Chat + Workflow mid-wait, approve days later → the gated tool runs EXACTLY
    ONCE; double-click is one approval; deny withholds with NO mutation; timeout auto-denies; resume under a fresh
    token) — CI; the duplicate-apply signal = 0, the pre-approval-mutation signal = 0.
  - CHAT-D10 (a multi-effect card approved 2-of-3 → the 2 resume approved, the 1 withheld, each independent
    idem_key=card_id:<idx>; no effect runs twice; the withheld never mutates) — CI; the per-effect duplicate +
    withheld-mutation signals = 0.
- **TESTS (required).** Unit tests for: the per-effect idem_key (single vs multi), the withhold semantics (a
  declined effect never mutates), the double-click → one approval. The CDC pair for 9.1, 9.4, 4.2, 4.7. The
  drill-harness scenarios for CHAT-D9 and CHAT-D10 — each a CHAINED scenario (request → kill → approve later →
  assert exactly-once) per EI-01 §4. State the cargo-mutants mutation floor for the idem_key + withhold core
  module (mandatory-core — the exactly-once HITL property).
- **DEFINITION OF DONE.** The HITL Card Service exists and compiles; CHAT-D9/D10 each emit a dated green artifact
  (exactly-once, 0 pre-approval mutation, withheld never mutates); the unit + CDC + drill tests pass; the
  contract-coverage scanner is green; the chat-owns-only-the-card boundary is stated; all lints green; the work is
  committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat HITL approval-card bridge (per-effect idem_key; exactly-once across a kill).
  Body lists: 9.1/9.4 the durable signal + per-effect idem_key, 4.2 the approve gate, 4.7 the resume token;
  CHAT-D9/D10 greened with measured numbers (exactly-once, 0 pre-approval mutation); the chat-owns-only-the-card
  boundary stated. Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P19 — The agent ToolDef set (frozen X-6 defaults) routed through EffectApi + reserve/settle + run --dry-run (the routing-split safety boundary)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C6 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C6") — the ToolDef-set + EffectApi-
  routing + reserve/settle + dry-run slice (the second committable unit of M4-C6; the HITL card is CHAT-P18). This
  completes the first-useful bar (§6).
- **DEPENDS-ON.** CHAT-P18 (the card posts EffectApi::apply per approved effect) + CHAT-P8 (the check gate). The M2
  Agent prompts that ship EffectApi::apply plan-then-apply (8.2), ToolSurface::register_tool + the frozen
  requires_approval defaults (8.1, X-6), the four uniform sandbox guarantees (8.4), and run --dry-run (8.7), GREEN.
  The M1 Storage prompt that ships reserve/settle (11.7).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native; the strategy pattern at the agent plug-in);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — a chat mutation that bypasses
    EffectApi via ToolHands is the failure a structural check forbids), §8 (cost is decision-shaped — reserve
    fronts every spend-bearing post).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §8 (the chat
    ToolDef set + the frozen requires_approval defaults; all side-effecting tools route through EffectApi, NEVER
    ToolHands::exec — the routing split is the safety boundary; the four uniform guarantees), §9 (reserve/settle —
    chat surfaces cost but never holds the wallet); 04-views-cli-and-api.md §1 (S11/S3).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-6 (the frozen
    requires_approval defaults — Chat post/react/reply = no, create_channel/invite/archive = yes, cross-subsystem
    inherits the target's default; the four uniform guarantees; the EffectApi-vs-ToolHands routing split).
  - Contracts: contract-index.md rows 8.1 (ToolSurface::register_tool + the frozen defaults), 8.2 (EffectApi::apply
    plan-then-apply — schema→capability→delegation→tenant→budget→HITL→apply→meter; a withheld gated tool does not
    mutate), 8.4 (ToolHands::exec is for compute, not chat mutation — the routing split), 8.7 (run --dry-run →
    ProposedEffects without applying), 11.7 (reserve/settle — fronts every spend-bearing agent post).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C6 ToolDef-set work) + §3 (the ToolDef + frozen
    defaults row) + §1 (non-negotiability item 4) + §6 (first-useful = end of M4-C6).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — the routing-split structural check (every
    side-effecting chat tool routes through EffectApi, never ToolHands::exec); the no-host-exec lint.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the tool module):
  - The chat ToolDef set (8.1, frozen X-6 defaults): chat.post / reply_in_thread / react / start_dm =
    requires_approval false; chat.create_channel / invite / archive_channel = true; any cross-subsystem effect
    INHERITS the TARGET subsystem's default. ALL side-effecting tools route through EffectApi (plan-then-apply,
    reserves), NEVER ToolHands::exec (the routing split is the safety boundary).
  - Reserve/settle on every spend-bearing agent post (11.7); chat surfaces cost (the card's live estimate) but
    never holds the wallet.
  - run --dry-run (8.7) on chat tools returns ProposedEffects without applying.
  - FLOOR named: none new — chat owns the tool DEFINITIONS + the routing; the sandbox guarantees + the budget are
    the M2 Agent/M1 Storage primitives. State that chat must not re-implement a sandbox or a budget.
- **CONTRACTS TO IMPLEMENT.** 8.1 the chat ToolDef set + the frozen defaults (owned). 8.2 EffectApi routing
  (consumed — every chat mutation routes through it). 8.7 run --dry-run (owned — on chat tools). 11.7
  reserve/settle (consumed). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The routing split: every side-effecting chat tool routes through EffectApi, NEVER ToolHands::exec (the
    no-host-exec lint + a structural check; 0 chat mutations via ToolHands) — CI.
  - The frozen requires_approval defaults hold (post/react/reply/start_dm = false; create_channel/invite/archive =
    true; cross-subsystem inherits target; 0 default divergences) — CI.
  - Reserve/settle fronts every spend-bearing post (no balance → no post; 0 unreserved spend-bearing posts) — CI;
    run --dry-run returns ProposedEffects without applying (0 mutations on a dry-run) — CI.
- **TESTS (required).** Unit tests for: the frozen requires_approval defaults, the EffectApi routing (no ToolHands
  mutation), the reserve gate, the dry-run no-apply. The CDC pair for 8.1, 8.2, 11.7. State the cargo-mutants
  mutation floor for the routing-split core module (mandatory-core — the no-ToolHands-mutation property).
- **DEFINITION OF DONE.** The chat ToolDef set exists and compiles; every chat mutation routes through EffectApi
  (no ToolHands mutation); the frozen defaults hold; reserve/settle fronts every spend-bearing post; dry-run
  applies nothing; the unit + CDC tests pass; the contract-coverage scanner is green; all lints green (incl.
  no-host-exec); the work is committed. This completes the first-useful bar (§6). No gate greened by a weakened
  threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat agent ToolDef set (frozen X-6 defaults) + EffectApi routing + reserve/settle
  + dry-run. Body lists: 8.1 the frozen ToolDef defaults, 8.2 EffectApi routing, 11.7 reserve/settle, 8.7 dry-run;
  the EffectApi-not-ToolHands routing split proven; the frozen defaults proven. Branch first if on default; do not
  push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P20 — ACL-filtered Search indexing (declare_indexable + the Filter conjoin) + embeddings-as-PII + the HYOK skip

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C7 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C7 — Search indexing (ACL-filtered)
  + reindex-from-source parity") — the ACL-filtered-index + HYOK-skip slice. The full replay-from-source parity is
  CHAT-P21.
- **DEPENDS-ON.** CHAT-P6 (the message source the index feeds from) + CHAT-P8 (membership for the ACL conjoin) +
  CHAT-P15 (the Refs read-model). The M2 Search prompts that ship query/semantic always conjoining the
  list_objects Filter (6.1/6.2), declare_indexable (6.3), GREEN. The M1 Identity list_objects (4.3). The M1
  Storage KMS / can_derive_plaintext_index (11.3) for the HYOK skip.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe — embeddings ARE personal data; one search ACL model);
    ../../external-insights/04-hard-problems.md §1 (erasure reaches embeddings, never hides);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — search as a non-member returns 0
    results; the lint fails any query path without the Filter).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §7
    (declare_indexable — the chat/message index spec; Search ALWAYS conjoins the frozen list_objects Filter over
    message.id; the search-as-non-member drill; embeddings erasure-aware; the HYOK skip); 02-internals-and-
    algorithms.md §4.4 (the reindex consumer — the feeder).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-E (the
    list_objects Filter conjoin lowering to a JOIN against the authz reverse index).
  - Contracts: contract-index.md rows 6.3 (declare_indexable(IndexSpec{subsystem:"chat", type:"message",
    ft_fields, struct_fields, semantic, acl_object_type:"message"})), 6.1 (query always conjoins the list_objects
    Filter before scoring — the search-requires-acl-filter lint), 6.2 (semantic ACL-filtered k-NN), 4.3
    (list_objects SetExpr → JOIN), 11.3 (can_derive_plaintext_index()=false structurally skips indexing for an
    HYOK tenant).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C7 ACL-index + HYOK work + exit) + §1
    (non-negotiability item 2: search ACL) + §6 (production-hardened).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D11 (search as a non-member →
    0 results from channels you're not in; the search-requires-acl-filter lint fails any query path reaching the
    index without the Filter conjoined). (CHAT-D15 is greened in CHAT-P21.)
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the search-projection
  module):
  - declare_indexable(IndexSpec{subsystem:"chat", type:"message", ft_fields:["body"], struct_fields:["channel",
    "author", "thread_root", "created_at", "kind"], semantic: Some(EmbeddingSpec), acl_object_type:"message"})
    (6.3).
  - Search ALWAYS conjoins the frozen list_objects Filter over the message.id column before scoring (the
    search-requires-acl-filter lint, 6.1) — the SetExpr lowers to a JOIN against Id's authz reverse index; no
    N+1, no post-filter. The chat search-projection feeder emits the index spec; chat is never read directly.
  - Embeddings-as-personal-data: on erasure, Search PURGES + reindexes embeddings (not just FT) — never hides; an
    HYOK tenant whose can_derive_plaintext_index()=false structurally SKIPS message indexing (11.3).
  - FLOOR named: the embeddings erasure cascade's holder-completeness is proven in CHAT-P22 (M4-C8 / CHAT-D8).
    State that here the index is wired and ACL-correct; the full multi-holder erasure receipt is CHAT-P22. The full
    replay-from-source parity is CHAT-P21.
- **CONTRACTS TO IMPLEMENT.** 6.3 declare_indexable (owned — the chat/message spec). 6.1/6.2 the ACL-conjoined
  query/semantic (consumed — chat's search feeder + the Filter conjoin). 4.3 list_objects (consumed — the JOIN).
  11.3 the HYOK skip (consumed). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D11 (search as a non-member → 0 results from channels you're not in; the search-requires-acl-filter lint
    fails ANY query path reaching the index without the Filter conjoined) — CI; the non-member-result signal = 0;
    the lint signal = 0 unfiltered query paths.
  - The HYOK skip: a tenant with can_derive_plaintext_index()=false produces 0 indexed message bodies — CI; the
    HYOK-indexed-body signal = 0.
  - On erasure, embeddings PURGE + reindex (not just FT; 0 recoverable embeddings for an erased subject) — CI.
- **TESTS (required).** Unit tests for: the ACL-conjoined query (the Filter is always present), the embeddings
  purge-on-erasure, the HYOK skip. The CDC pair for 6.3, 6.1. The drill-harness scenario for CHAT-D11. State the
  cargo-mutants mutation floor for the ACL-conjoin core module (mandatory-core — the no-leak search property).
- **DEFINITION OF DONE.** The chat search projection + the ACL conjoin + the embeddings-as-PII purge + the HYOK
  skip exist and compile; CHAT-D11 emits a dated green artifact (0 non-member results); the HYOK skip produces 0
  indexed bodies; the search-requires-acl-filter lint is green; the unit + CDC + drill tests pass; the
  contract-coverage scanner is green; the full-replay-parity follow-on is named (CHAT-P21) and the
  full-erasure-receipt follow-on is named (CHAT-P22); all lints green; the work is committed. No gate greened by a
  weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat ACL-filtered search indexing + embeddings-as-PII + HYOK skip. Body lists:
  6.3 the chat/message index spec, 6.1 the Filter conjoin, 11.3 the HYOK skip; CHAT-D11 greened (0 non-member
  results); the full-replay-parity (CHAT-P21) + full-erasure-receipt (CHAT-P22) follow-ons named. Branch first if
  on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P21 — replay(scope, since) full parity: Search/Refs/Notif read-models rebuild, steady-state and recovery share one path (CHAT-D15)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C7 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C7") — the full replay-from-source
  parity slice (the second committable unit of M4-C7; the ACL-filtered index is CHAT-P20), completing the CHAT-P6
  replay skeleton.
- **DEPENDS-ON.** CHAT-P6 (the replay skeleton it completes) + CHAT-P20 (the search read-model) + CHAT-P15 (the
  Refs read-model) + CHAT-P17 (the Notif read-model). The M2 Search prompts that ship reindex (6.4), GREEN. The M2
  Refs/Notif read-models that rebuild via replay.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe — erased subjects → tombstones on rebuild);
    ../../external-insights/04-hard-problems.md §5 (reindex-from-source — Search is a derived store, never reads
    the owner DB; steady-state and recovery share one path); §1 (erasure reaches embeddings, never hides);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — wipe + replay + hash-compare).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §6 (replay —
    the only recovery path; sub-artifact granular; erased subjects → tombstones; steady-state and recovery share
    one path); 02-internals-and-algorithms.md §4.4 (the reindex consumer).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-E (the reindexing
    consumer composes the frozen list_objects Filter so a rebuild stays ACL-correct).
  - Contracts: contract-index.md rows 2.6 (replay(scope, since) — full parity here), 6.4 (reindex — the only
    rebuild path), 6.1 (the Filter conjoin a rebuild stays ACL-correct under), 5.2 (the Refs read-model rebuilt),
    7.1 (the Notif read-model rebuilt).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C7 replay full-parity work + exit) + §3 (the
    replay full-parity row M4-C7) + §6 (production-hardened).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D15 (wipe + replay(scope, since)
    → Search/Refs/Notif read-models rebuild; steady-state and recovery share one path; erased subjects →
    tombstones; reindex-parity hash matches).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the replay module, completing
  the CHAT-P6 skeleton):
  - replay(scope, since) FULL parity: Search/Refs/Notif read-models rebuild from chat.*.snapshot; steady-state and
    recovery share ONE path (the outbox → consumer template); erased subjects emit tombstones (no PII resurrected);
    a reindexing consumer composes the frozen list_objects Filter so a rebuild stays ACL-correct.
  - FLOOR named: none new — this completes the replay floor named in CHAT-P6. State that the full multi-holder
    erasure receipt remains CHAT-P22.
- **CONTRACTS TO IMPLEMENT.** 2.6 replay full parity (owned). 6.4 reindex (consumed). 6.1 the Filter conjoin under
  rebuild (consumed). 5.2/7.1 the Refs/Notif read-models (consumed — rebuilt). Implement to the frozen shapes; no
  local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D15 (wipe + replay(scope, since) → Search/Refs/Notif read-models rebuild; steady-state and recovery share
    one path; erased subjects → tombstones; the reindex-parity hash matches the live hash) — SCHED; the
    reindex-parity-hash mismatch signal = 0.
  - The steady-state-vs-recovery one-path identity (the rebuild uses the same outbox→consumer template as
    steady-state; 0 recovery-only code paths) — CI.
  - A rebuild stays ACL-correct (the reindexing consumer conjoins the Filter; 0 unfiltered rebuilt rows) — CI.
- **TESTS (required).** Unit tests for: the steady-state-vs-recovery one-path identity, the erased-subject
  tombstone on rebuild, the ACL-correct rebuild. The CDC pair for 2.6, 6.4. The drill-harness scenario for CHAT-D15
  (a CHAINED wipe→replay→hash-compare). State the cargo-mutants mutation floor for the replay core module if
  mandatory-core; if not, say so.
- **DEFINITION OF DONE.** The full replay parity exists and compiles; CHAT-D15 emits a dated green artifact
  (reindex-parity hash matches); steady-state and recovery share one path; a rebuild stays ACL-correct; the unit +
  CDC + drill tests pass; the contract-coverage scanner is green; the full-erasure-receipt follow-on is named
  (CHAT-P22); all lints green; the work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat replay(scope, since) full parity (Search/Refs/Notif rebuild; one path).
  Body lists: 2.6 the full replay parity, 6.4 reindex; CHAT-D15 greened (reindex-parity hash matches); the
  one-path identity proven; the full-erasure-receipt follow-on named (CHAT-P22). Branch first if on default; do
  not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P22 — The GDPR holder + author crypto-shred across hot/cold/backups + the DSR cascade (CHAT-D8; 0 recoverable PII)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C8 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C8 — The erasure cascade across
  every chat holder") — the holder + author-crypto-shred + cascade slice (the 0-recoverable-PII core). The mention
  pseudonym-shred + the restriction flag + the LEGAL residual is CHAT-P23.
- **DEPENDS-ON.** CHAT-P6 (the bodies/drafts stores + the holder) + CHAT-P14 (the unfurl cache) + CHAT-P16
  (read-state) + CHAT-P20 + CHAT-P21 (the search/refs read-models incl. embeddings). The M1 GDPR prompts that ship
  the holder trait + the erasure ledger (10.1/10.8) + the DSR fan-out (10.4). The M1 Storage KMS per-subject DEK
  (11.3/11.4). The M1 Storage backup/restore (11.5, the backups the crypto-shred must reach).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction — data subject erasure); ../../external-insights/04-hard-
    problems.md §1 (erasure-vs-immutability — the per-subject DEK crypto-shred + restrict; the third-party
    free-text residual handled BY REFERENCE, not restated — the residual is CHAT-P23);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — erase a person and watch 0
    recoverable PII across hot + cold + backups + embeddings).
  - Architecture: ../04-subsystem-architectures/chat/architecture/05-hard-problems.md §5 (chat is the most
    PII-dense holder, the canonical GD-4 crypto-shred case); 03-events-contracts-and-glue.md §10 (the holder over
    every chat store; the free-text residual BY REFERENCE — the residual is CHAT-P23), §1.1 (chat.message.erased
    — the *.erased cross-cutting tombstone); 06-reconciliation-compliance.md (the holder list + the DSR cascade).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-7 (the ONE
    free-text/immutable erasure posture — the structural floor here; the residual is CHAT-P23).
  - Contracts: contract-index.md rows 10.1 (PersonalDataHolder{locate, export, rectify, restrict, erase} — every
    store; erasure = purge/crypto-shred/pseudonymise, never hide), 11.4 (crypto-shred / per-subject DEK —
    bodies/drafts), 10.4 (the DSR fan-out — the cascade reaches Search/Refs/Notif via the bus, never a backdoor),
    10.8 (the erasure ledger — drives post-restore re-erasure), 2.7 (*.erased tombstones on the log), 11.5 (the
    backups the crypto-shred must reach).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C8 holder + author-erasure + cascade work) + §1
    (non-negotiability item 3: erasure that misses a Chat holder) + §6 (production-hardened).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D8 (erase a person → bodies
    crypto-shred in hot + cold + backups; mentions → [erased user]; read-state/drafts/unfurl-cache purged; Search
    incl. embeddings / Refs / Notif cascade → 0 recoverable PII; holder receipts).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the GDPR-holder module):
  - The GDPR holder: locate / export / rectify / restrict / erase over EVERY chat store (10.1). (The restrict
    behaviour at every read path is CHAT-P23; here the holder surface + the erase path ship.)
  - Author erasure: crypto-shred P's per-subject DEK → every body P authored unrecoverable in hot + cold segments
    + backups SIMULTANEOUSLY (WITHOUT rewriting the immutable log, 11.4); tombstone the record (chat.message.erased,
    2.7).
  - The cascade reaches Search (incl. embeddings) / Refs / Notif via the bus + DSR (10.4), NEVER a backdoor;
    read-state / drafts / unfurl-cache purged.
  - The erasure ledger (10.8) drives post-restore re-erasure (a restored backup re-applies the shred).
  - FLOOR named: none new — the mention pseudonym-shred + the restriction flag + the LEGAL free-text residual are
    CHAT-P23 (M4-C8's second unit). State this.
- **CONTRACTS TO IMPLEMENT.** 10.1 the holder over every chat store (owned — the erase path). 11.4 crypto-shred
  per-subject DEK (consumed — bodies/drafts). 10.4 the DSR fan-out (consumed — the cascade). 10.8 the erasure
  ledger (consumed — post-restore re-erasure). 2.7 *.erased tombstones (owned — chat.message.erased). Implement to
  the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D8 (erase a person → bodies crypto-shred in hot + cold + backups; read-state/drafts/unfurl-cache purged;
    Search incl. embeddings / Refs / Notif cascade → 0 recoverable PII; holder receipts) — SCHED; the
    recoverable-PII signal = 0 across hot + cold + backups + embeddings; the holder-receipt set is complete (0
    holders missed). (The mention → [erased user] half is asserted in CHAT-D8 via CHAT-P23.)
  - The cascade reaches every holder via the bus + DSR, never a backdoor (0 backdoor erasure paths; the holder
    receipts cover every registered chat store) — CI/SCHED.
- **TESTS (required).** Unit tests for: the per-subject-DEK crypto-shred (every authored body unrecoverable across
  hot/cold/backups), the unfurl-cache/read-state/drafts purge, the cascade-via-bus (no backdoor). The CDC pair for
  10.1, 11.4, 10.4. The drill-harness scenario for CHAT-D8 (a CHAINED erase → assert 0 recoverable PII across
  hot/cold/backups/embeddings + a complete holder-receipt set). State the cargo-mutants mutation floor for the
  crypto-shred core module (mandatory-core — the 0-recoverable-PII property).
- **DEFINITION OF DONE.** The chat GDPR holder + the author crypto-shred + the cascade exist and compile; CHAT-D8
  emits a dated green artifact (0 recoverable PII across hot + cold + backups + embeddings; complete holder
  receipts); the cascade reaches every holder via the bus (no backdoor); the unit + CDC + drill tests pass; the
  contract-coverage scanner is green; the mention-shred + restriction + LEGAL-residual follow-on is named
  (CHAT-P23); all lints green; the work is committed. A red CHAT-D8 is a dated scorecard row, not a softened
  check.
- **COMMIT.** Header: P-<NNN> M4: Chat GDPR holder + author crypto-shred (hot/cold/backups) + DSR cascade. Body
  lists: 10.1 the holder, 11.4 per-subject-DEK body shred, 10.4 the DSR cascade, 2.7 chat.message.erased;
  CHAT-D8 greened (0 recoverable PII, complete holder receipts); the mention-shred + restriction + LEGAL-residual
  follow-on named (CHAT-P23). Branch first if on default; do not push unless asked. End with the Co-Authored-By
  trailer.

---

### CHAT-P23 — Mention pseudonym-shred (→ [erased user]) + the Art.18 restriction flag at every read path + the LEGAL free-text residual (BY REFERENCE)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C8 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C8") — the mention-shred +
  restriction-flag + LEGAL-residual slice (the second committable unit of M4-C8; the holder + author crypto-shred
  is CHAT-P22).
- **DEPENDS-ON.** CHAT-P22 (the holder + the erase path it extends) + CHAT-P11 (the structured mention(Principal)
  node the pseudonym-shred targets). The M1 Identity resolve_pseudonym / erase (4.8). The M1 GDPR the ONE erasure
  posture (10.9).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction); ../../external-insights/04-hard-problems.md §1 (the ONE
    posture: per-subject DEK crypto-shred + pseudonym-map shred + restrict; the third-party free-text residual
    handled BY REFERENCE, not restated); ../../external-insights/01-process-and-quality-doctrine.md §1
    (name-your-floors — the residual is a named LEGAL floor).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §10 (the
    restriction flag Art.18; the free-text residual BY REFERENCE to the ONE posture), §1.1 (the mention(Principal)
    → [erased user]); 05-hard-problems.md §5 (the pseudonym-map shred is free because the node is structured +
    pseudonymous).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §X-7 (the ONE
    free-text/immutable erasure posture — instantiated per subsystem BY REFERENCE; the residual is one ratified
    statement, [OPEN — LEGAL]).
  - Contracts: contract-index.md rows 4.8 (resolve_pseudonym / erase — the pseudonym-map shred → [erased user]),
    10.1 (restrict suppresses indexing/agent-use/analytics/notif — the restriction flag), 10.9 (the ONE posture,
    BY REFERENCE; the [OPEN — LEGAL] residual).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C8 mention-shred + restriction + residual work +
    the LEGAL residual floor) + §1 (non-negotiability item 3) + §5 (the free-text-residual floor → the ONE
    posture, R-C5).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D8 (the mention → [erased user]
    half + the restriction-flag suppression — asserted here within the CHAT-D8 erase scenario seeded by CHAT-P22).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the GDPR-holder module,
  extending CHAT-P22):
  - Mentioned erasure: the structured mention(Principal) → pseudonym-map shred (4.8) → renders [erased user] on
    next render (free, because the node is structured + pseudonymous).
  - The restriction flag (Art. 18) honoured at EVERY read path: a restricted subject is excluded from indexing /
    agent-use / new notification routing / analytics (a distinct state from erasure).
  - The free-text third-party residual handled BY REFERENCE to the ONE platform posture (10.9, X-7) — chat writes
    NO fifth chat-specific residual statement; it supplies only the structural floor (per-subject DEK shred from
    CHAT-P22 + pseudonym-map shred + restrict).
  - FLOOR named: the free-text third-party residual → the ONE platform posture (10.9), [OPEN — LEGAL], ratified
    ONCE by counsel/DPO (R-C5) — the structural floor ships REGARDLESS; the residual is one ratified statement,
    parallel-tracked (LEGAL), not a chat blocker. State it as an untested-but-named LEGAL floor.
- **CONTRACTS TO IMPLEMENT.** 4.8 resolve_pseudonym / erase (consumed — mention shred). 10.1 restrict (owned — the
  restriction flag at every read path). 10.9 the ONE posture (consumed — BY REFERENCE). Implement to the frozen
  shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The mention pseudonym-shred: erasing a mentioned subject renders [erased user] on next render (0 recoverable
    mentioned-PII; the mention is structured + pseudonymous) — CI; the recoverable-mention-PII signal = 0. (This
    is the mention half of CHAT-D8, seeded by CHAT-P22.)
  - The restriction flag: a restricted subject is excluded from indexing / agent-use / notif-routing / analytics
    (0 processings on a restricted subject) — CI; the restricted-processing signal = 0.
- **TESTS (required).** Unit tests for: the mention pseudonym-shred → [erased user], the restriction-flag
  suppression at every read path (indexing / agent-use / notif / analytics). The CDC pair for 4.8, 10.1. The
  CHAT-D8 mention-half scenario (assert mention → [erased user]). State the cargo-mutants mutation floor for the
  restriction core module (mandatory-core — the no-processing-on-restricted property).
- **DEFINITION OF DONE.** The mention pseudonym-shred + the restriction flag exist and compile; the mention →
  [erased user] half of CHAT-D8 is green (0 recoverable mentioned-PII); the restriction flag suppresses every
  processing for a restricted subject; the unit + CDC tests pass; the contract-coverage scanner is green; the
  LEGAL residual is named as an [OPEN — LEGAL] floor (untested-but-named, R-C5); the no-untagged-personal-data
  lint is green; the work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat mention pseudonym-shred + Art.18 restriction flag + LEGAL free-text
  residual (BY REFERENCE). Body lists: 4.8 mention pseudonym shred → [erased user], 10.1 the restriction flag,
  10.9 the residual BY REFERENCE; the mention-shred + restriction gates greened; the LEGAL residual named
  ([OPEN — LEGAL]). Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P24 — Agent presence classes + streaming partials (mock-provable; final replaces partial; reconnect resumes the final) (CHAT-D16)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C9 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C9 — Agent presence, streaming +
  explicit-first dispatch") — the presence + streaming slice. The explicit-first dispatch + provenance popover is
  CHAT-P25.
- **DEPENDS-ON.** CHAT-P10 (the firehose for presence + partials) + CHAT-P1 (the agent.message.partial
  firehose-only token). The M2 Agent prompts that ship AgentRuntime::step --use-mock (8.3), EffectApi (8.2), and —
  the hard blocker — AG-D4 GREEN (no agent compute over a red sandbox gate).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native; mock implementations during development — the strategy pattern, --use-mock,
    no real agents); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — a mid-stream
    reconnect must resume the final, never a half-message; the drill forces it), §4 (drive the streaming UX).
  - Architecture: ../04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md §7.2 (agent
    presence classes available/busy/rate-limited/offline on the firehose), §7.3 (streaming partials
    agent.message.partial; final replaces partial); 03-events-contracts-and-glue.md §1.2 (agent.message.partial
    firehose-only); 04-views-cli-and-api.md §1 (S5 thread streaming, S8 agent presence).
  - Contracts: contract-index.md rows 8.3 (AgentRuntime::step --use-mock — the strategy seam, a real runtime
    flag), 8.2 (EffectApi — the agent's chat output path), 8.4 (ToolHands::exec + AG-D4 — no agent compute over a
    red AG-D4), 3.5 (the firehose — presence + partials).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C9 presence + streaming work) + §6
    (production-hardened) + §5 (the mock-runtime floor → real LlmAgentRuntime, post-M5).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D16 (drive the streaming UX
    against the mock runtime → partials stream; final replaces partial; a mid-stream reconnect resumes the final,
    never a half-message). (CHAT-D17 is greened in CHAT-P25.)
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the agent-presence module),
  built and proven against the MOCK runtime (--use-mock; no real agents during development):
  - Agent presence classes (available / busy / rate-limited / offline) on the firehose; streaming partials
    (agent.message.partial) on the firehose, final replaces partial.
  - The mid-stream-reconnect resume: a reconnect resumes the FINAL, never a half-message (rides the CHAT-P9
    resume-cursor for the agent.message.partial stream).
  - FLOOR named: the agent runtime = the mock (--use-mock, scripted-deterministic); the real LlmAgentRuntime is
    the post-M5 follow-on (a config/impl swap, not a rewrite, after AG-D4/D2/D3/D5 green — VISION §3). Name it.
- **CONTRACTS TO IMPLEMENT.** 8.3 AgentRuntime::step --use-mock (consumed — mock-provable streaming). 3.5 the
  firehose presence/partials (consumed). 8.4 AG-D4 (consumed — no compute over a red sandbox gate). Implement to
  the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D16 (drive the streaming UX against the mock runtime → partials stream; final replaces partial; a
    mid-stream reconnect resumes the FINAL, never a half-message) — CI; the half-message-on-reconnect signal = 0.
  - AG-D4 (the permanent sandbox-escape gate) re-confirmed green before any agent compute chat dispatches — the
    drill is upstream (M2); chat asserts it is green, runs no compute over a red AG-D4.
- **TESTS (required).** Unit tests for: the presence-class transitions, the partial→final replacement, the
  mid-stream-reconnect resume-the-final. The CDC pair for 8.3, 3.5. The drill-harness scenario for CHAT-D16 (proven
  against --use-mock). State the cargo-mutants mutation floor for the partial→final core module if mandatory-core;
  if not, say so.
- **DEFINITION OF DONE.** Agent presence + streaming exist and compile against the mock runtime; CHAT-D16 emits a
  dated green artifact (no half-message on reconnect); AG-D4 is green (no compute over a red sandbox gate); the
  unit + CDC + drill tests pass; the contract-coverage scanner is green; the mock-runtime floor is named; all lints
  green; the work is committed. No gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat agent presence + streaming partials (mock-provable; final replaces partial).
  Body lists: 8.3 --use-mock streaming, 3.5 firehose presence/partials; CHAT-D16 greened (0 half-message); the
  mock-runtime floor named; AG-D4 confirmed green. Branch first if on default; do not push unless asked. End with
  the Co-Authored-By trailer.

---

### CHAT-P25 — Explicit-first agent dispatch (no auto-spawn on mention; reserve-gated) + the agent provenance popover (CHAT-D17)

- **BAND.** M4.
- **ROADMAP MILESTONE.** M4-C9 (planning/06-roadmaps/subsystems/chat.md §4 "M4-C9") — the explicit-first-dispatch +
  provenance slice (the second committable unit of M4-C9; presence + streaming is CHAT-P24). This completes the M4
  chat surface.
- **DEPENDS-ON.** CHAT-P24 (the agent-presence module) + CHAT-P19 (EffectApi + the agent ToolDef set) + CHAT-P17
  (the mention is the notify-not-dispatch producer). The M2 Agent prompts that ship explicit-first dispatch (8.6,
  CHAT-1), GREEN. The M1 Storage reserve/settle (11.7). The M1 Identity mint_run_token (4.7).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native; first-class event propagation/triggers);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — a casual @agent mention must NOT
    auto-spawn a costed run; the drill forces it), §8 (cost/abuse is decision-shaped — explicit-first, no
    auto-spawn until counsel-gated).
  - Architecture: ../04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md §1.1
    (chat.message.mentioned = the agent notify-not-dispatch signal, the explicit-first reference gate);
    02-internals-and-algorithms.md §7.5 (the agent provenance popover from causation_id / correlation_id /
    on_behalf_of); 04-views-cli-and-api.md §1 (S12 the provenance popover).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §6 (explicit-first
    dispatch pinned — a mention notifies, does not auto-spawn; implicit auto-dispatch is L-3, counsel-gated).
  - Contracts: contract-index.md rows 8.6 (EventInbox::deliver + explicit-first dispatch CHAT-1), 11.7
    (reserve/settle — gates even the explicit run; no balance → no run), 4.7 (mint_run_token — the per-run token),
    8.2 (EffectApi — the agent's chat output path).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M4-C9 explicit-first + provenance work) + §1
    (non-negotiability item 5: explicit-first dispatch) + §6 (production-hardened).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md row CHAT-D17 (a casual @agent mention →
    notifies the agent's inbox, does NOT spawn a costed run; only an explicit action / structured trigger
    dispatches; reserve/settle gates even the explicit run).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the dispatch module):
  - Explicit-first dispatch (CHAT-1, 8.6): a casual @agent mention NOTIFIES the agent's inbox, does NOT spawn a
    costed run; only an explicit action / structured trigger dispatches; reserve/settle gates even the explicit
    run (no balance → no run). NO auto-spawn path is wired (L-3, counsel-gated) — state this is deliberately
    absent.
  - The agent provenance popover (S12): "why did this agent post?" from causation_id / correlation_id /
    on_behalf_of.
  - FLOOR named: the no-auto-spawn path is a deliberate, counsel-gated absence (L-3), not an omission. State this.
- **CONTRACTS TO IMPLEMENT.** 8.6 explicit-first dispatch (consumed — the mention notifies, no auto-spawn). 11.7
  reserve/settle (consumed — gates the explicit run). 4.7 mint_run_token (consumed). 8.2 EffectApi (consumed — the
  agent chat output). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D17 (a casual @agent mention → notifies the agent's inbox, does NOT spawn a costed run; only an explicit
    action / structured trigger dispatches; reserve/settle gates even the explicit run) — CI; the
    auto-spawn-on-mention signal = 0; the unreserved-run signal = 0.
  - No auto-spawn path is wired (a structural check that no mention→dispatch edge exists; 0 auto-spawn paths) — CI.
- **TESTS (required).** Unit tests for: the explicit-first dispatch (mention notifies, no auto-spawn), the
  reserve-gate (no balance → no run), the provenance popover derivation. The CDC pair for 8.6, 11.7. The
  drill-harness scenario for CHAT-D17 (proven against --use-mock). State the cargo-mutants mutation floor for the
  explicit-first-dispatch core module (mandatory-core — the no-auto-spawn cost-abuse property).
- **DEFINITION OF DONE.** Explicit-first dispatch + the provenance popover exist and compile against the mock
  runtime; CHAT-D17 emits a dated green artifact (no auto-spawn-on-mention, reserve gates the run); no auto-spawn
  path is wired; the unit + CDC + drill tests pass; the contract-coverage scanner is green; the no-auto-spawn L-3
  absence is named; all lints green; the work is committed. This completes the M4 chat surface. No gate greened by
  a weakened threshold.
- **COMMIT.** Header: P-<NNN> M4: Chat explicit-first agent dispatch (no auto-spawn; reserve-gated) + provenance
  popover. Body lists: 8.6 explicit-first dispatch, 11.7 reserve/settle gate; CHAT-D17 greened (0 auto-spawn,
  reserve-gated); the no-auto-spawn L-3 absence named. Branch first if on default; do not push unless asked. End
  with the Co-Authored-By trailer.

---

### CHAT-P26 — World-scale surge hardening: the 30x agent-surge + deploy-herd (F6) + tuning the per-surface shed budgets (CHAT-D3 / D4-at-scale)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-C-S1 (planning/06-roadmaps/subsystems/chat.md §4 "M5 follow-ons" → "M5-C-S1 — 30×
  agent-surge + deploy-herd (the F6 family)") — the surge-drill + shed-budget-tuning slice. Chat's participation
  in the whole-system E2E wedge is CHAT-P27.
- **DEPENDS-ON.** CHAT-P5..CHAT-P25 (the full M4 chat surface — all deterministic correctness drills green). The M5
  platform prompts that ship the F6 surge harness profiles + the cell bulkhead.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale means world-scale); ../../external-insights/01-process-and-quality-doctrine.md
    §3 (prove-it — the 30× surge with mixed principal kinds; observability is part of the pass — the human lane
    held + the agent lane shed must be SEEN).
  - Architecture: ../04-subsystem-architectures/chat/architecture/07-drills-and-open-questions.md (the chat drills
    + the surge family + the tunables D-C3/D-C4); 02-internals-and-algorithms.md §7 (presence at scale).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-K (the per-surface
    shed budgets — tuned here from the surge results), ADR-16 (the protected-human-lane shed order).
  - Contracts: contract-index.md rows 1.11 (the protected-human-lane shed order + the per-surface shed budget
    numbers, tuned here), 1.8 (the telemetry survival-signal set the drills assert against — RED/USE per
    principal-kind, shed-counts, breaker-state), 11.7 (reserve/settle — the agent lane gated under surge).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M5-C-S1 surge work) + §5 (the tunables R-C2/OQ-K —
    tuned from D-C3/D-C4) + §6 (production-hardened progression).
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows CHAT-D3 (30× agent message/connection
    surge on one tenant → human connection/read latency in budget; the agent lane sheds 429 + Retry-After honoured;
    other tenants unaffected) — the TE-21 build-gate; CHAT-D4 re-run at scale (deploy-herd).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the surge-hardening
  harness-scenario modules):
  - The CHAT-D3 surge scenario on the failure-injection harness: 30× agent message/connection surge on one tenant
    → assert human connection/read latency in budget; the agent lane sheds 429 + Retry-After honoured; other
    tenants unaffected (the cross-tenant impact = 0).
  - The CHAT-D4 re-run at scale (the deploy-herd / gateway-fleet roll under a connection storm — the M4-C2 drill
    re-run at world scale).
  - Tune the per-surface shed budget NUMBERS (R-C2 / OQ-K) from the CHAT-D3/D4 (D-C3/D-C4) results — promote the
    named-floor shed budgets from CHAT-P10 to tuned values in the thresholds file.
  - FLOOR named: the shed-budget numbers are now TUNED (promoting the CHAT-P10 floor). The Scylla / home-node /
    cross-org / comment-consolidation promotions are CHAT-P28/P29/P30/P31 (triggered, not unconditional). State
    which floors remain and their landing prompts.
- **CONTRACTS TO IMPLEMENT.** 1.11 the shed budgets tuned (owned — chat's connection-storm + agent-mention-storm
  numbers, promoted from floor to tuned). 1.8 the telemetry assertions (consumed — the surge survival signals).
  11.7 reserve/settle under surge (consumed). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D3 (30× agent surge → human latency in budget; agent lane sheds 429 + Retry-After; cross-tenant impact
    = 0) — SCHED; the human-lane-latency + agent-shed-count + cross-tenant-impact signals (1.8) are the green
    artifact (human in budget, agent shed > 0, cross-tenant = 0).
  - CHAT-D4 at scale (deploy-herd → bounded reconnect; resume completes for all; no loss) — SCHED.
  - The tuned shed-budget numbers hold the human lane under 30× while the agent lane sheds (the thresholds file
    carries the tuned values; 0 human-lane drops under the tuned budget) — SCHED.
- **TESTS (required).** The drill-harness scenarios for CHAT-D3 and CHAT-D4 at scale (each a surge scenario with
  mixed principal kinds asserting against the 1.8 survival signals). A test that the tuned shed-budget numbers hold
  the human lane under 30× while the agent lane sheds. No new core module; if the surge harness touches a
  mandatory-core module, state its mutation floor.
- **DEFINITION OF DONE.** The CHAT-D3/D4 surge scenarios exist and run; CHAT-D3, CHAT-D4-at-scale each emit a dated
  green artifact (human in budget, agent shed, cross-tenant = 0); the shed-budget numbers are tuned in the
  thresholds file; the remaining floor promotions are named (CHAT-P28/P29/P30/P31); the tests pass; the work is
  committed. A red surge gate is a dated scorecard row, never a weakened budget.
- **COMMIT.** Header: P-<NNN> M5: Chat 30x agent-surge + deploy-herd hardening + tuned shed budgets. Body lists:
  1.11 shed budgets tuned, 1.8 surge survival signals; CHAT-D3/D4-at-scale greened (human in budget, agent shed,
  cross-tenant 0); the remaining floor promotions named (CHAT-P28/P29/P30/P31). Branch first if on default; do not
  push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P27 — The whole-system E2E wedge participation (E2E-1 pane + E2E-2 the agent-native flagship terminal surface + E2E-4 DSAR holder)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-C-S1 (planning/06-roadmaps/subsystems/chat.md §4 "M5 follow-ons") — chat's
  participation in the whole-system E2E wedge (E2E-1 / E2E-2 / E2E-4), the second committable unit of M5-C-S1 (the
  surge hardening is CHAT-P26).
- **DEPENDS-ON.** CHAT-P26 (the surge-hardened chat surface) + CHAT-P14 (the unfurl/live-update pane for E2E-1) +
  CHAT-P18/P19/P25 (the HITL card + explicit-first dispatch for E2E-2) + CHAT-P22 (the erasure holder for E2E-4).
  The M4 producers/consumers (Git, CI, Issues, Knowledge) green so the E2E wedge has real artifacts. The E2E-wedge
  driver prompts (testing-strategy §2) that orchestrate the four chained-mutation scenarios.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — the flagship E2E-2 terminates in chat);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (chain the mutations — the E2E wedge chains
    operations end-to-end, the bugs live where state updates mid-flight), §3 (observability is part of the pass).
  - Architecture: ../04-subsystem-architectures/chat/architecture/06-reconciliation-compliance.md (chat's E2E
    participation); 02-internals-and-algorithms.md §7 (presence at scale, the terminal surface).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (the E2E wedge
    composition — chat the terminal surface of E2E-2).
  - Contracts: contract-index.md rows 1.8 (the telemetry survival-signal set the E2E scenarios assert against),
    11.7 (reserve/settle — metered through one wallet in E2E-2), 5.9 (ci.check.updated — the E2E-1/E2E-2 unfurl
    bust).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M5-C-S1 E2E wedge participation E2E-1/E2E-2/E2E-4) +
    §6 (production-hardened). Read testing-strategy/README.md §2/§3.4 for the wedge.
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md rows E2E-1 (PR context pane — chat's
    unfurl/live-update analog), E2E-2 (CI-fail → triage agent → issue → chat → fix-PR — the agent-native FLAGSHIP,
    chat the terminal surface), E2E-4 (DSAR fan-out — chat's CHAT-D8 erasure is a named holder in the
    0-holders-missed certificate).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat (the E2E harness-scenario
  modules) and the E2E wedge harness:
  - Chat's participation in the whole-system E2E wedge: E2E-1 (chat's unfurl/live-update pane via CHAT-D7), E2E-2
    (the agent-native FLAGSHIP — chat is the terminal surface: the explicit-first dispatch, the HITL
    withhold→approve→apply card, the unfurl of the issue + fix-PR, all metered through one wallet), E2E-4 (chat's
    CHAT-D8 erasure as a named holder in the 0-holders-missed DSAR certificate). Wire chat's scenario contributions
    into the wedge harness.
  - FLOOR named: none new — this is chat's contribution to the shared wedge; the floor promotions remain
    CHAT-P28/P29/P30/P31. State this.
- **CONTRACTS TO IMPLEMENT.** 1.8 the telemetry assertions (consumed — the E2E survival signals). 11.7
  reserve/settle (consumed — one wallet in E2E-2). 5.9 ci.check.updated (consumed — the E2E unfurl bust).
  Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-2 (the flagship — CI-fail → triage agent → issue → chat → fix-PR terminating in chat: exactly-once HITL +
    merge, 0 leak) green — the chat terminal surface contributes its green artifact; the exactly-once + no-leak
    signals = 0 violations.
  - E2E-1 (chat's unfurl/live-update pane via CHAT-D7) green for chat's pane — CI/SCHED.
  - E2E-4 (chat's CHAT-D8 erasure as a named holder in the 0-holders-missed DSAR certificate) green — chat appears
    in the certificate with 0 holders missed.
- **TESTS (required).** The E2E wedge scenario contributions for E2E-1 / E2E-2 / E2E-4 (chained-mutation scenarios
  against a full cell with mock agents per EI-01 §4). No new core module; if the E2E harness touches a
  mandatory-core module, state its mutation floor.
- **DEFINITION OF DONE.** The E2E wedge contributions exist and run; E2E-1/E2E-2/E2E-4 are green for chat's
  surfaces (the flagship E2E-2 terminates green in chat); the tests pass; the work is committed. A red E2E gate is
  a dated scorecard row, never a weakened assertion.
- **COMMIT.** Header: P-<NNN> M5: Chat whole-system E2E wedge participation (E2E-1/E2E-2/E2E-4). Body lists:
  E2E-1/E2E-2/E2E-4 green for chat (the flagship terminates in chat); 1.8 survival signals, 11.7 one wallet, 5.9
  unfurl bust. Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### CHAT-P28 — ScyllaDB hot-tier promotion (M5-C-S2; the named M4-C1 floor; a MessageStore trait swap) + the object-store BlobStore swap

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-C-S2 (planning/06-roadmaps/subsystems/chat.md §4 "M5 follow-ons" → "M5-C-S2 — ScyllaDB
  hot-tier promotion"). Conditional on its trigger (measured per-cell write/partition volume); built only if the
  trigger has fired, otherwise named as still-floored with the measured signal that would fire it.
- **DEPENDS-ON.** CHAT-P4 (the MessageStore trait the Scylla swap rides) + CHAT-P5 (the co-commit) + CHAT-P22 (the
  crypto-shred the swap must preserve) + CHAT-P26 (the surge measurements that fire the trigger). The M1 Storage
  object-store BlobStore promotion (11.2). The M5 platform multi-cell prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; name-your-floors — promote a floor only when its trigger fires, and on the
    seam the floor was built to swap); ../../external-insights/01-process-and-quality-doctrine.md §1
    (name-your-floors; a floor promotes on measured signal, not on prediction), §3 (re-run the floor's drill
    across the promotion boundary — the drill was written to survive the swap).
  - Architecture: ../04-subsystem-architectures/chat/architecture/05-hard-problems.md (the Scylla hot tier);
    01-tech-and-data-model.md (the MessageStore trait — the Scylla swap seam; the cold tier identical).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md (the residency-pinned
    + crypto-shred-capable-per-cell requirement on the swap).
  - Contracts: contract-index.md rows 11.2 (BlobStore object-store swap — the cold segments move with the
    promotion; the cold tier + MessageStore trait are identical either way), 11.4 (the per-subject DEK crypto-shred
    the Scylla tier must preserve), 12.1/12.4 (the (tenant, region) partition + residency-pin per cell).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M5-C-S2 work + trigger) + §5 (the floor table —
    Postgres → ScyllaDB, the trigger measured per-cell write/partition volume, R-C6/R-5) + §7.
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — CHAT-D2 + CHAT-D8 re-run across the
    Scylla swap (M5-C-S2).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat — if the trigger has fired,
  build the promotion; if not, name it as still-floored:
  - ScyllaDB hot-tier promotion (the named M4-C1 floor, R-C6/R-5): triggered by measured per-cell write/partition
    volume; a MessageStore trait swap (the cold tier + trait identical); residency-pinned + crypto-shred-capable
    per cell. Re-run CHAT-D2 + CHAT-D8 across the swap.
  - The fs-backed → object-store BlobStore swap (11.2) for the cold segments moves with the Scylla promotion (a
    one-line swap; the cold tier is identical).
  - FLOOR named: if the trigger has NOT fired, leave it as a named floor with its measured trigger signal
    (per-cell write/partition volume) and this prompt's id as where it would land — the gap must be VISIBLE
    (EI-04 §4). State explicitly.
- **CONTRACTS TO IMPLEMENT.** 11.2 the object-store BlobStore swap (consumed — cold segments). 11.4 the
  crypto-shred preserved across the swap (consumed). 12.1/12.4 partition + residency-pin per cell (consumed).
  Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - If triggered: CHAT-D2 (per-conversation total order) + CHAT-D8 (0 recoverable PII) re-run GREEN across the
    Scylla swap — SCHED; the order-violation + recoverable-PII signals = 0 post-swap.
  - If NOT triggered: a dated gap-report row naming the measured trigger signal (per-cell write/partition volume)
    — not a drill, an honest named floor (EI-04 §4).
- **TESTS (required).** If built: the re-run drill scenario (CHAT-D2/D8 across the swap, each written to survive
  the swap). The CDC pair for 11.2. State the cargo-mutants mutation floor for any mandatory-core module touched.
  If NOT triggered: a recorded gap-report row (untested-but-named).
- **DEFINITION OF DONE.** If triggered: the Scylla hot tier is promoted on the MessageStore-trait seam and CHAT-D2
  + CHAT-D8 re-run emit dated green artifacts (order preserved / 0 recoverable PII); the object-store swap rides
  with it; the CDC pair + tests pass; all lints green; committed. If NOT triggered: the floor is named in the gap
  report with its measured trigger signal and this prompt as its landing; committed. No floor masquerades as done;
  no gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M5: Chat ScyllaDB hot-tier promotion + object-store BlobStore swap (where triggered).
  Body lists: whether the trigger fired (with the measured signal) and built or named-floored; 11.2 consumed where
  built; CHAT-D2/D8 re-run greened where built. Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### CHAT-P29 — Mega-channel channel-sharded home-node (M5-C-S3; the named M4-C2 delivery floor; Phoenix/Discord guild model)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-C-S3 (planning/06-roadmaps/subsystems/chat.md §4 "M5 follow-ons" → "M5-C-S3 —
  Mega-channel channel-sharded home-node"). Conditional on its trigger (subscriber count exceeding the
  subject-fan-out budget); built only if triggered, otherwise named as still-floored.
- **DEPENDS-ON.** CHAT-P9 + CHAT-P10 (the firehose subject fan-out the home-node escalates) + CHAT-P26 (the surge
  measurements that fire the trigger). The M5 platform multi-cell prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; name-your-floors — promote on the seam the floor was built to swap);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors — a floor promotes on measured
    signal), §3 (re-run the floor's drill across the promotion boundary — CHAT-D1 was written to survive the swap).
  - Architecture: ../04-subsystem-architectures/chat/architecture/05-hard-problems.md (the channel-sharded
    home-node Phoenix/Discord guild model); 02-internals-and-algorithms.md §7 (mega-channel fan-out → the
    home-node).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-J (the firehose
    resume-cursor protocol the home-node still satisfies).
  - Contracts: contract-index.md rows 3.5 (the firehose the home-node rides — the resume-cursor protocol unchanged
    across the escalation), 1.7 (the harness shim — the home-node is a gateway-process escalation bounded by it).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M5-C-S3 work + trigger) + §5 (the floor table —
    firehose subject fan-out → channel-sharded home-node, the trigger subscriber count exceeds the subject-fan-out
    budget, R-5) + §7.
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — CHAT-D1 re-run across the home-node
    escalation (M5-C-S3).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat-gateway — if the trigger has
  fired, build the escalation; if not, name it as still-floored:
  - Mega-channel channel-sharded home-node (the named M4-C2 delivery floor, R-5): triggered by subscriber count
    exceeding the subject-fan-out budget; the Phoenix/Discord guild model in Rust + consistent-hash. Re-run CHAT-D1
    across the escalation.
  - FLOOR named: if the trigger has NOT fired, leave it as a named floor with its measured trigger signal
    (subscriber count vs the subject-fan-out budget) and this prompt's id as where it would land. State explicitly.
    Also state the BEAM/Phoenix-gateway hatch (the connection-tier-language floor from CHAT-P9) is opened only if
    CHAT-D3/D4 proved Rust intractable — a sibling, separately-triggered floor.
- **CONTRACTS TO IMPLEMENT.** 3.5 the firehose the home-node rides (consumed — the resume-cursor protocol unchanged
  across the escalation). 1.7 the harness shim (consumed — the gateway-process escalation bounded by it).
  Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - If triggered: CHAT-D1 (resume 0 lost / 0 dup) re-run GREEN across the home-node escalation — CI; the lost/dup
    signal = 0 post-escalation.
  - If NOT triggered: a dated gap-report row naming the measured trigger signal (subscriber count vs the
    subject-fan-out budget) — an honest named floor (EI-04 §4).
- **TESTS (required).** If built: the CHAT-D1 re-run scenario across the escalation (written to survive the swap).
  State the cargo-mutants mutation floor for any mandatory-core module touched. If NOT triggered: a recorded
  gap-report row (untested-but-named).
- **DEFINITION OF DONE.** If triggered: the channel-sharded home-node is built on the firehose-subject-fan-out seam
  and CHAT-D1 re-run emits a dated green artifact (0 lost-dup post-escalation); tests pass; all lints green;
  committed. If NOT triggered: the floor is named in the gap report with its measured trigger signal and this
  prompt as its landing; committed. No floor masquerades as done; no gate greened by a weakened threshold.
- **COMMIT.** Header: P-<NNN> M5: Chat mega-channel channel-sharded home-node (where triggered). Body lists:
  whether the trigger fired (with the measured signal) and built or named-floored; CHAT-D1 re-run greened where
  built; the BEAM-gateway sibling floor noted. Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### CHAT-P30 — Cross-org / federated channels (M5-C-X1; designed-not-built → on the frozen cross-cell PII-free pointer bridge)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-C-X1 (planning/06-roadmaps/subsystems/chat.md §4 "M5 follow-ons" → "M5-C-X1 —
  Cross-org / federated channels"). Conditional on its trigger (cross-org demand + the cross-tenant capability +
  the bridge shipping); built only if the bridge ships in M5, otherwise named as designed-not-built.
- **DEPENDS-ON.** CHAT-P7 (the Conversation model that must not foreclose multi-cell) + CHAT-P8 (the per-viewer
  membership resolution) + CHAT-P15 (project(), always cell-local) + CHAT-P22 (the DSR cascade that must iterate
  member_cells). The M1 Tenancy cross-cell PII-free pointer bridge (12.6). The M5 platform multi-cell prompts (the
  bridge goes live). P6 control plane + LEGAL (the cross-tenant capability + residency policy).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scale; name-your-floors — a floor promotes on measured demand);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors), §3 (cross-cell resolution
    always cell-local — the property re-drilled across the boundary).
  - Architecture: ../04-subsystem-architectures/chat/architecture/05-hard-problems.md (the cross-org bridge
    consumption); 01-tech-and-data-model.md (the Conversation model's non-foreclosure of multi-cell).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-I (the cross-cell
    PII-free pointer bridge — resolution always cell-local; cross-org channels ride it).
  - Contracts: contract-index.md rows 12.6 (the cross-cell PII-free pointer bridge — cross-org channels; per-viewer
    resolution cell-local; multi-cell DSR iterates member_cells), 10.4 (the DSR fan-out iterates member_cells for
    multi-cell), 5.6 (project() always cell-local — only the projection crosses, never the raw row).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M5-C-X1 work + trigger) + §5 (the floor table —
    single home-cell → cross-org on the cross-cell bridge, the trigger cross-org demand + the cross-tenant
    capability + multi-cell DSR, R-C9) + §7.
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — the multi-cell DSR drills (GA-D8 / CP-D7
    / CP-D8) for cross-org; the cross-cell-resolution-cell-local property for M5-C-X1.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat — if the bridge ships in M5,
  build cross-org channels; if not, name it as designed-not-built:
  - Cross-org / federated channels (designed-not-built → on the frozen cross-cell bridge, R-C9): rides
    CrossCellPointer (12.6); per-viewer resolution always cell-local; multi-cell DSR iterates member_cells (10.4);
    needs an explicit cross-tenant capability + residency policy (→ P6 control plane + LEGAL).
  - FLOOR named: built only if the bridge ships in M5; otherwise NAMED as designed-not-built with its trigger
    (cross-org demand + the cross-tenant capability + the bridge) and this prompt's id as its landing. State
    explicitly; the Conversation model from CHAT-P7 already does not foreclose it.
- **CONTRACTS TO IMPLEMENT.** 12.6 the cross-cell bridge consumption (consumed — cross-org channels, cell-local
  resolution). 10.4 multi-cell DSR (consumed — iterate member_cells). 5.6 project() cell-local (consumed — only
  the projection crosses). Implement to the frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - If triggered: cross-cell resolution is always cell-local (0 raw cross-cell rows crossing; only the projection
    crosses) + multi-cell DSR iterates member_cells (0 holders missed across cells) — SCHED.
  - If NOT triggered: a dated gap-report row naming the measured trigger (cross-org demand + the cross-tenant
    capability + the bridge) — an honest named floor (EI-04 §4).
- **TESTS (required).** If built: the cross-cell-cell-local + multi-cell-DSR scenarios. The CDC pair for 12.6.
  State the cargo-mutants mutation floor for any mandatory-core module touched. If NOT triggered: a recorded
  gap-report row (untested-but-named).
- **DEFINITION OF DONE.** If triggered: cross-org channels are built on the cross-cell bridge and the
  cross-cell-cell-local + multi-cell-DSR drills emit dated green artifacts (0 raw rows crossing, 0 holders missed);
  the CDC pair + tests pass; all lints green; committed. If NOT triggered: the floor is named designed-not-built in
  the gap report with its trigger and this prompt as its landing; committed. No floor masquerades as done.
- **COMMIT.** Header: P-<NNN> M5: Chat cross-org / federated channels on the cross-cell bridge (where triggered).
  Body lists: whether the bridge shipped + built or named designed-not-built; 12.6/10.4 consumed where built; the
  cross-cell-cell-local + multi-cell-DSR drills greened where built. Branch first if on default; do not push unless
  asked. End with the Co-Authored-By trailer.

---

### CHAT-P31 — Comment-threading consolidation onto the Chat threading primitive (M5-C-X2; OQ-L; a store/transport swap, not a rewrite)

- **BAND.** M5.
- **ROADMAP MILESTONE.** M5-C-X2 (planning/06-roadmaps/subsystems/chat.md §4 "M5 follow-ons" → "M5-C-X2 —
  Comment-threading consolidation"). Conditional on its trigger (document-anchored comments needing real-time
  presence, OQ-L); built only if the OQ-L trigger fires, otherwise named in the gap report (E-3).
- **DEPENDS-ON.** CHAT-P2 (the shared #thread-/#comment- #sub scheme the consolidation rides) + CHAT-P10 (the
  firehose transport the anchored comments promote onto) + CHAT-P11 (the shared content + refs scheme). The M3/M4
  Knowledge + Issues anchored-comment owners (the comment-threads being consolidated).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — promote on demand, on the shared scheme, NOT a rewrite);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors), §3 (re-run the relevant
    content/refs/#sub drills across the store/transport swap).
  - Architecture: ../04-subsystem-architectures/chat/architecture/05-hard-problems.md (the comment-threading
    consolidation — a store/transport swap over the shared scheme); 02-internals-and-algorithms.md (the threading
    primitive + the firehose transport).
  - Reconciliation: ../05-refined-shared-systems-architecture/00-reconciliation-decisions.md §OQ-L
    (comment-threading consolidation onto the Chat threading primitive — a store/transport swap over the shared
    #thread-/#comment- #sub + content + refs scheme, NOT a rewrite).
  - Contracts: contract-index.md rows 5.7 (the shared #sub scheme the comment consolidation rides), 3.5 (the
    firehose the consolidation rides), 13.1 (the shared content scheme), 5.4 (the shared refs scheme).
  - Roadmap: planning/06-roadmaps/subsystems/chat.md §4 (the M5-C-X2 work + trigger) + §5 (the floor table —
    comment-threading consolidation named-not-built, the trigger anchored comments need real-time multi-party
    presence, OQ-L, R-C8, gap report E-3) + §7.
  - Drills: testing-strategy/01-whole-system-e2e-and-drill-catalogue.md — the content + refs + #sub drills re-run
    across the store/transport swap (M5-C-X2).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-chat — if the OQ-L trigger fires,
  build the consolidation; if not, name it in the gap report:
  - Comment-threading consolidation (OQ-L named floor, R-C8): when document-anchored comments (Knowledge/Issues)
    need real-time presence, promote them onto the Chat threading primitive + the firehose transport — a
    store/transport swap over the shared #thread-/#comment- #sub + content + refs scheme, NOT a rewrite.
  - FLOOR named: built only if the OQ-L trigger fires; otherwise NAMED in the gap report (E-3) with its trigger
    (anchored comments need real-time multi-party presence) and this prompt's id as its landing. State this is a
    store/transport swap, NOT a CRDT and NOT a rewrite (the per-message CAS from CHAT-P12 is unchanged).
- **CONTRACTS TO IMPLEMENT.** 5.7 the shared #sub scheme (owned — the comment consolidation rides it). 3.5 the
  firehose (consumed — the transport). 13.1/5.4 the shared content + refs scheme (consumed). Implement to the
  frozen shapes; no local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - If triggered: the consolidation re-runs the relevant content + refs + #sub drills GREEN across the
    store/transport swap (0 round-trip / edge / #sub regressions post-swap) — CI.
  - If NOT triggered: a dated gap-report row (E-3) naming the trigger (anchored comments need real-time
    multi-party presence) — an honest named floor (EI-04 §4).
- **TESTS (required).** If built: the content/refs/#sub re-run drills across the swap (each written to survive the
  swap). The CDC pair for any newly-consumed row. State the cargo-mutants mutation floor for any mandatory-core
  module touched. If NOT triggered: a recorded gap-report row (untested-but-named).
- **DEFINITION OF DONE.** If triggered: anchored comments are consolidated onto the Chat threading primitive +
  firehose transport and the content/refs/#sub drills re-run emit dated green artifacts (0 regressions post-swap);
  the CDC pairs + tests pass; all lints green; committed. If NOT triggered: the floor is named in the gap report
  (E-3) with its trigger and this prompt as its landing; committed. No floor masquerades as done; this is a
  store/transport swap, not a rewrite.
- **COMMIT.** Header: P-<NNN> M5: Chat comment-threading consolidation onto the Chat threading primitive (where
  triggered). Body lists: whether the OQ-L trigger fired + built or named-floored (E-3); 5.7 the shared #sub scheme;
  the content/refs/#sub re-run drills greened where built. Branch first if on default; do not push unless asked.
  End with the Co-Authored-By trailer.

---

### CHAT-P32 — The switch test: drive the real Chat UI in a browser (the 13 screens + the responsive cases)

- **BAND.** M6.
- **ROADMAP MILESTONE.** M6 (planning/06-roadmaps/subsystems/chat.md §4 "M6 — Dogfooding: the switch test").
- **DEPENDS-ON.** CHAT-P5..CHAT-P27 (the full chat surface, world-scale-ready) + CHAT-P28..CHAT-P31 (the floors
  promoted where triggered). The M5 platform prompts that ship the world-scale-readiness gate + the E2E wedge green
  (you do not dogfood real team data over a red restore-verify or DSAR fan-out). The M6 dogfood prompts (Myelin
  hosts itself; the team talks in Myelin's own Chat).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (top-of-the-line UX and design — the product, not an internal tool; design comes before
    implementation); ../../external-insights/01-process-and-quality-doctrine.md §4 (actually try it — the switch
    test is reached by DRIVING the real UI in a browser, not by reading the feature list; the
    modal-in-the-wrong-place / picker-off-screen class of bug only appears when a human drives it);
    ../../external-insights/05-ux-and-design.md (the design-language §8b — measured contrast + latency budgets +
    render(parse(md))===md + overlays against the real anchor).
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
## Coverage matrix (every chat roadmap milestone → its prompt(s); no gap — finer granularity preserves all first-pass coverage)

| Roadmap milestone (planning/06-roadmaps/subsystems/chat.md) | Band | Prompt(s) | Primary drills greened |
|---|---|---|---|
| M2-C0 — declare the contract surfaces | M2 | CHAT-P1 (chat.* taxonomy) + CHAT-P2 (ReBAC fragment + #sub grammar) + CHAT-P3 (humanise/notif + fanout-class + firehose-scope + TE-21) | (contract-coverage scanner; fragment compile; token + #sub grammar; scope-bounded) |
| M4-C1 — durable message store + outbox co-commit | M4 | CHAT-P4 (MessageStore trait + tiers) + CHAT-P5 (outbox co-commit + idempotent send + order) + CHAT-P6 (per-subject-DEK bodies + holder + #sub mint + replay skeleton) + CHAT-P7 (Conversation/Membership entity + list index) + CHAT-P8 (membership→write_tuples→zookie + new-enemy + gate) | CHAT-D13, CHAT-D14, CHAT-D2 |
| M4-C2 — firehose resume-cursor transport + gateway | M4 | CHAT-P9 (stateless gateway + subscribe/resume/resync) + CHAT-P10 (firehose-only delivery + shed order) | CHAT-D1, CHAT-D4 |
| M4-C3 — composer over the frozen content subset | M4 | CHAT-P11 (content subset + round-trip + inline nodes→edges) + CHAT-P12 (composer UI + per-message CAS) | render(parse(md))===md (chat KN-D2) |
| M4-C4 — per-viewer permission-aware unfurls | M4 | CHAT-P13 (cache + per-viewer gate) + CHAT-P14 (erasure-safe + invalidation + anchor stability) + CHAT-P15 (project() + edge producer) | CHAT-D5, CHAT-D6, CHAT-D7, CHAT-D18 |
| M4-C5 — read-state hot path + Activity-as-view | M4 | CHAT-P16 (read-state service) + CHAT-P17 (fanout-class + Activity-as-view) | CHAT-D12 |
| M4-C6 — HITL approval-card bridge + ToolDef set | M4 | CHAT-P18 (HITL card bridge) + CHAT-P19 (ToolDef set + EffectApi routing + reserve/settle + dry-run) | CHAT-D9, CHAT-D10 |
| M4-C7 — ACL-filtered search + reindex parity | M4 | CHAT-P20 (ACL-filtered index + embeddings + HYOK skip) + CHAT-P21 (full replay parity) | CHAT-D11, CHAT-D15 |
| M4-C8 — erasure cascade across every holder | M4 | CHAT-P22 (holder + author crypto-shred + cascade) + CHAT-P23 (mention pseudonym-shred + restriction flag + LEGAL residual) | CHAT-D8 |
| M4-C9 — agent presence/streaming + explicit-first | M4 | CHAT-P24 (presence + streaming) + CHAT-P25 (explicit-first dispatch + provenance) | CHAT-D16, CHAT-D17 |
| M5-C-S1 — 30× surge + deploy-herd + E2E wedge | M5 | CHAT-P26 (surge + shed-budget tuning) + CHAT-P27 (E2E wedge participation) | CHAT-D3, CHAT-D4-at-scale, E2E-1/E2E-2/E2E-4 |
| M5-C-S2 — ScyllaDB hot-tier promotion | M5 | CHAT-P28 | CHAT-D2/D8 re-run across the Scylla swap |
| M5-C-S3 — mega-channel channel-sharded home-node | M5 | CHAT-P29 | CHAT-D1 re-run across the home-node escalation |
| M5-C-X1 — cross-org / federated channels | M5 | CHAT-P30 | cross-cell-cell-local + multi-cell DSR (GA-D8/CP-D7/CP-D8) |
| M5-C-X2 — comment-threading consolidation | M5 | CHAT-P31 | content/refs/#sub re-run across the store/transport swap |
| M6 — the switch test | M6 | CHAT-P32 | CHAT-D19 |

**Prompt count: 14 → 32.** (The first pass's 14 prompts split into 32 single-deliverable prompts: M2-C0 1→3,
M4-C1 store 1→3 + membership 1→2, M4-C2 1→2, M4-C3 1→2, M4-C4 1→3, M4-C5 1→2, M4-C6 1→2, M4-C7 1→2, M4-C8 1→2,
M4-C9 1→2, M5-C-S1 1→2, the four M5 floor promotions 1→4, M6 1→1. ~2.3x where bundling existed; the genuinely
atomic M6 switch test stays atomic.) Every milestone, contract, drill, and floor the first pass covered is
preserved at finer granularity; no drill is dropped (CHAT-D1..D19 + E2E-1/E2E-2/E2E-4 all still greened by some
prompt's GATE/DRILLS field).

**Permanent gates inherited (re-confirmed by the prompts that touch their surface):** STOR-D1/D2 (restore-verify —
CHAT-P5, CHAT-P28) and AG-D4 / CI-T1 (sandbox escape — CHAT-P19, CHAT-P24, CHAT-P25). Neither is chat-owned; both
bound chat's "done".

**Floors named, each with its follow-on prompt (all preserved from the first pass, re-threaded to the finer ids):**
- Postgres-partitioned hot tier → ScyllaDB (CHAT-P4 → CHAT-P28), triggered by measured per-cell write/partition
  volume.
- fs-backed BlobStore cold segments → object-store BlobStore (CHAT-P4 → CHAT-P28), riding the Scylla promotion.
- Firehose subject fan-out → channel-sharded home-node (CHAT-P9/P10 → CHAT-P29), triggered by subscriber count.
- Rust gateway → BEAM/Phoenix hatch (CHAT-P9, written-but-closed; opened only if CHAT-D3/D4 prove Rust
  intractable, measured in CHAT-P26).
- Single home-cell → multi-cell cross-org channels on the cross-cell bridge (CHAT-P7/P8/P15 non-foreclosure →
  CHAT-P30), triggered by cross-org demand + the cross-tenant capability + the bridge shipping.
- Mock agent runtime → real LlmAgentRuntime (CHAT-P24 → post-M5/execution, after AG-D4/D2/D3/D5 green).
- Free-text third-party erasure residual → the ONE platform posture, counsel/DPO ratified once (CHAT-P23, LEGAL,
  parallel — the structural floor ships regardless).
- Comment-threading → consolidation onto the Chat threading primitive (CHAT-P2/P10/P11 shared scheme → CHAT-P31),
  triggered by anchored comments needing real-time presence (OQ-L).
- Per-surface shed budgets → tuned numbers (CHAT-P10 floor → CHAT-P26 tuned).
- Per-message CAS → no CRDT (CHAT-P12; chat is single-author; n/a follow-on, stated so no agent builds one; the
  related OQ-L consolidation is CHAT-P31, not a CRDT).
- Replay skeleton → full parity (CHAT-P6 → CHAT-P21).
- Canvas = embedded/pinned Knowledge page → embed not editor (CHAT-P13; joint Chat↔Knowledge review, the lean is
  firm).
