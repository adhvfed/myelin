# Phase 7 — Prompt Ledger: Notifications (myelin-notif)

> Prompt count: first pass 13 prompts → this finer-grained pass 30 prompts (every multi-deliverable prompt
> split into single-deliverable, clean-context, independently-committable units; all first-pass coverage —
> every milestone N-M2.0..N-M5.3, every contract 7.1–7.8, every drill NOTIF-D1..D10 + D-N11 + the E2E legs +
> STOR-D2, and every named floor — preserved at finer granularity, DEPENDS-ON re-threaded across the new ids).
>
> Phase: 07-prompts (per-system file, Phase 7-A, finer-granularity expansion). The complete ordered set of
> implementation prompts that operationalize the entire notifications roadmap
> (planning/06-roadmaps/shared/notifications.md, milestones N-M2.0..N-M5.3) into clean-context,
> independently-committable coding tasks. Built to the template in planning/07-prompts/00-ledger-overview.md §2
> (every field present, never implicit) and banded to planning/06-roadmaps/00-master-sequencing.md §2 (M0..M6,
> the gate invariant). Frozen architecture (this file OPERATIONALIZES, it does not redesign):
> planning/05-refined-shared-systems-architecture/notifications.md + contract-index.md §7 (Notif 7.1–7.8) + the
> consumed rows (3.1/3.5/3.6, 2.2/2.4, 4.2/4.3/4.4/4.10, 5.2/5.6, 9.1/9.3/9.4, 12.6, 13.1/13.3) +
> 00-reconciliation-decisions.md (X-1/X-2/X-3/X-7, OQ-C/OQ-E/OQ-I/OQ-J/OQ-K/OQ-L). Plain-text identifiers
> throughout (no backticks-as-emphasis). Markdown only; this file makes no commits. Date: 2026-06-19.
>
> The global P-NNN ids are assigned by the consolidated ledger index (Phase 7-B, 01-ledger-index.md) when these
> per-system prompts are interleaved into the single execution order. Here each prompt carries a stable local
> handle NOTIF-P<n> so its DEPENDS-ON edges are unambiguous before global numbering; the index rewrites
> NOTIF-P<n> to its P-NNN. Where a prompt depends on another system's prompt not yet numbered, it names that
> system's milestone (the index resolves it to the P-NNN). Notif is off the critical path but is a maximal
> consumer: it owns zero new contracts vs Phase 3 and consumes frozen platform shapes, so most DEPENDS-ON edges
> point at M0/M1/M2 substrate-and-sibling prompts, and the N-M3/N-M4 milestones are pure registration that
> accretes as producers/consumers come online.
>
> Coverage (finer-grained): N-M2.0 → NOTIF-P1, NOTIF-P2, NOTIF-P3, NOTIF-P4; N-M2.1 → NOTIF-P5, NOTIF-P6,
> NOTIF-P7, NOTIF-P8, NOTIF-P9, NOTIF-P10; N-M2.2 → NOTIF-P11, NOTIF-P12, NOTIF-P13; N-M2.3 → NOTIF-P14,
> NOTIF-P15, NOTIF-P16, NOTIF-P17, NOTIF-P18; N-M3 → NOTIF-P19, NOTIF-P20; N-M4 → NOTIF-P21, NOTIF-P22,
> NOTIF-P23; N-M5.1 → NOTIF-P24; N-M5.2 → NOTIF-P25, NOTIF-P26, NOTIF-P27; N-M5.3 → NOTIF-P28, NOTIF-P29,
> NOTIF-P30. Thirty prompts, no milestone gap; the coverage matrix at the foot maps every milestone, drill, and
> floor to its prompt(s).

---

### NOTIF-P1 — Stand up myelin-notif: the serve(AppSpec) service shell + three ports + the glue-crate contract carriers

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.0 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.0 — Holder + outbox +
  Signal-consumer skeleton"; this prompt is the service-shell slice — the AppSpec boot, the three ports, and the
  myelin-notif glue crate's exposed contract types as compile-time carriers).
- **DEPENDS-ON.** The M0 substrate prompts that ship the Cargo workspace + the eight glue-crate skeletons,
  serve(AppSpec) (1.1), the three ports (1.2), liveness≠readiness (1.3), forward-only migrations (1.5), and the
  twelve lints (1.6) (master §2 M0; substrate roadmap SUB-M0). The M1 prompts that ship the OLTP store + RLS +
  the outbox table (11.1) and the (tenant, region) partition key + residency_verify (12.1/12.4) (master §2 M1).
  The index places this after those — Notif inherits the data-loss floor; it never invents an emit path. **Gate
  invariant inherited:** SUB-D1/SUB-D2/BUS-D4, all twelve lints, ID-D3, CP-D2/CP-D3, STOR-D1/STOR-D2 must be
  green before this prompt starts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (notifications as a shared backend system; the ONE inbox), §3 (name-your-floors,
    references-not-payloads, EU-sovereign, agent-native); ../../external-insights/01-process-and-quality-doctrine.md
    §2 (order-by-non-negotiability — the data-loss floor is below Notif), §1 (code-wins-over-docs +
    name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §1 (purpose, the C-9 resolution),
    §5.1 (cell-local, tenant-partitioned, bus-driven — the router is a stateless replicable consumer pool).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.1 (serve(AppSpec)), 1.2 (three
    ports), 1.3 (liveness≠readiness), 1.5 (forward-only migrations); the Notif §4.1 EXPOSED table (the
    InboxItem/HumanisedString/reason+class enums/DeliveryAdapter trait SHAPES this glue crate must carry as
    compile-time types so a contract change breaks every consumer's build now, ADR-01). Read
    00-reconciliation-decisions.md ADR-01 reference (the glue-crate compile-time contract carriers).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.0 (the serve(AppSpec) service work + the gate),
    §4 (the upstream-dependency list).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §3.3 (assertions read from production telemetry — the shell exports the metrics-health port the later drills
    assert against).
- **DELIVERABLE (what to build + exactly where in the repo).**
  - The glue crate myelin-notif (the M2 skeleton, if not already laid down): the exposed contract TYPES —
    InboxItem, HumanisedString{text, links[], icon}, the reason enum (approval_requested/escalated/sla/
    review_requested/assigned/mentioned/replied/agent_proposal/watched/state_changed/fyi/blocked/unblocked/
    thread_watched/shared/comments), the class enum (critical/direct/participating/watching/fyi), the
    DeliveryAdapter trait SHAPE — as compile-time carriers so a contract change breaks every consumer's build now
    (ADR-01). No bodies yet; just the frozen signatures every later Notif prompt and every consumer compiles
    against.
  - The myelin-notif IMPLEMENTATION crate (the Notif service shell): an AppSpec passed to serve(AppSpec) (1.1) —
    NOT a hand-rolled main — wiring boot → migrate → outbox relay → (consumer registration left as a seam for
    NOTIF-P3) → three ports (public/internal/metrics-health, 1.2) → graceful drain; liveness ≠ readiness (1.3);
    forward-only online migrations registered but empty (the tables land in NOTIF-P2). The crate compiles and
    boots with an empty migration set and no consumers.
  - FLOOR named (write it in the module doc): the data model is NOTIF-P2; the Signal-consumer router is
    NOTIF-P3; the holder registration is NOTIF-P4. The shell is explicitly NOT the working inbox — name the
    follow-on prompts so the skeleton is not mistaken for it.
- **CONTRACTS TO IMPLEMENT.** None owned with a body (the glue-crate types are carriers, not implementations).
  Consumed/wired: 1.1 serve(AppSpec), 1.2 three ports, 1.3 liveness≠readiness, 1.5 forward-only migrations.
  Implement to the frozen signatures — a needed shape change is a whole-workspace contract PR, escalated and
  written down, not a local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The harness boot self-test (CI): myelin-notif boots via serve(AppSpec), the three ports answer
    (public/internal/metrics-health), liveness flips before readiness, graceful drain completes. Threshold: all
    three ports up; readiness gated behind boot; 0 hand-rolled main.
  - All twelve committed lints green with the new myelin-notif glue + impl crates in the tree (esp. the crate
    compiles, no-host-exec, forward-only-migration on the empty set) — CI.
  - The contract-coverage scanner accepts the myelin-notif glue crate's carrier types (the 7.1–7.8 SHAPES exist
    even though bodies land later) — CI.
- **TESTS (required).** A boot/port unit test (the AppSpec wires three ports; liveness≠readiness). A
  compile-time test that the glue crate's carrier types match the contract-index 7.1–7.8 signatures (a wrong
  shape fails the build — ADR-01). No mandatory-core module yet (the shell carries no algorithm); state that
  explicitly.
- **DEFINITION OF DONE.** myelin-notif (glue + impl) compiles in the workspace and boots via serve(AppSpec) with
  three ports and liveness≠readiness; the glue crate carries the frozen 7.1–7.8 contract types; the boot
  self-test + all twelve lints + the contract-coverage scanner are green; the floors (data model NOTIF-P2;
  router NOTIF-P3; holder NOTIF-P4) are named in writing; the work is committed. No gate greened by weakening a
  threshold.
- **COMMIT.** Header: P-<NNN> M2: myelin-notif service shell — serve(AppSpec) + three ports + contract carriers.
  Body lists: the glue-crate 7.1–7.8 carrier types laid down; the boot self-test greened (three ports,
  liveness≠readiness); the floors named (data model NOTIF-P2; router NOTIF-P3; holder NOTIF-P4). Branch first if
  on default; do not push unless asked. End with the workspace Co-Authored-By trailer.

---

### NOTIF-P2 — The Notif data model: the nine tenant-partitioned tables (refs-not-strings, dedup UNIQUE, one state column)

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.0 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.0 — ... the data model";
  this prompt is the data-model slice — the nine migrations + the load-bearing invariants).
- **DEPENDS-ON.** NOTIF-P1 (the service shell + the forward-only migration seam). The M1 prompts that ship the
  (tenant, region) partition key + residency_verify (12.1/12.4) and the OLTP store + RLS + the outbox table
  (11.1), and the residency-pin lint (1.6 / M1). The index places this after NOTIF-P1 and the M1 store prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (references-not-payloads; EU-sovereign — residency-pinned partitioning);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors — the read-state truth is one
    column, named not implicit), §5 (the committed ratchet — the residency-pin + no-untagged-personal-data lints
    are gates).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §2 (the data model: inbox_item,
    notif_pref, quiet_hours, delivery, oncall_schedule, escalation_policy, escalation_run, humanise_template,
    mute), §2.1 (the inbox_item load-bearing invariants: template_args holds ArtifactRefs never strings;
    origin_event+reason provenance; ONE read-state column; UNIQUE(tenant, recipient, dedup_key) for write-time
    collapse), §2.2 (notif_pref/quiet_hours schemas), §2.4 (oncall_schedule/escalation_policy/escalation_run
    schemas), §2.5 (humanise_template ICU MessageFormat; mute).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 11.1 (OLTP tier + RLS + the
    outbox table), 12.1/12.4 (the (tenant, region) partition + residency); 7.1 (inbox_item shape backing
    list_inbox), 7.7 (the holder's tables). Read 00-reconciliation-decisions.md X-7 reference (refs-not-payloads
    makes most erasure free).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.0 (the data model bullet + the residency-pin
    lint).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §3.3 (the schema is what the later drills read/write — no drill of its own, but the lints gate it).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate, the
  forward-only migrations (refined §2):
  - The nine tables: inbox_item, notif_pref, quiet_hours, delivery, oncall_schedule, escalation_policy,
    escalation_run, humanise_template, mute — every table (tenant, region)-partitioned with the partition key as
    the FIRST column (the residency-pin lint, M1).
  - inbox_item: stores template_args as ArtifactRefs (never rendered strings); UNIQUE(tenant, recipient,
    dedup_key) for write-time collapse; a coalesce_count column (the "+N more" counter, used in NOTIF-P11);
    exactly ONE state column (the C-9 read-state truth); origin_event + reason columns (the NOTIF-2 provenance);
    subject_root / subject columns (the read-fanout JOIN target, NOTIF-P13).
  - delivery: UNIQUE(idem_key) for at-least-once + idempotent delivery (NOTIF-P16); a redacted boolean column
    (off-cell minimisation flag).
  - Tag every PII-bearing column with #[personal_data(...)] so the no-untagged-personal-data lint passes.
  - FLOOR named: the rows are written by later prompts (the router NOTIF-P3 UPSERTs inbox_item; prefs NOTIF-P9
    writes notif_pref/quiet_hours; escalation NOTIF-P14 writes escalation_run). This prompt ships the schema
    only; name the writer prompts.
- **CONTRACTS TO IMPLEMENT.** None owned with a body. Consumed: 11.1 OLTP+RLS+outbox table, 12.1/12.4
  partition+residency. The schema realises the storage half of 7.1/7.7 (the bodies land later). Frozen shapes
  only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The migration applies forward-only (CI): the nine tables migrate up; the migration is online and reversible
    only forward (1.5). Threshold: 9 tables created; 0 backward migration.
  - The residency-pin lint green: every table's FIRST column is the (tenant, region) partition key — CI.
  - The no-untagged-personal-data lint green: every PII column carries #[personal_data(...)] — CI.
  - The tenant-predicate lint green: every table is RLS-scoped on tenant — CI.
- **TESTS (required).** A migration test (the nine tables exist with the partition key first; the UNIQUE
  constraints on inbox_item dedup_key and delivery idem_key are present; exactly one state column on
  inbox_item). A lint-fixture test (a deliberately untagged-PII column / non-partition-first column is rejected
  by the lints). No mandatory-core module (schema only); state this.
- **DEFINITION OF DONE.** The nine tables migrate forward-only, all (tenant, region)-partitioned first column;
  inbox_item stores ArtifactRefs (never strings), has UNIQUE(tenant, recipient, dedup_key), one state column,
  origin_event+reason, subject_root/subject; delivery has UNIQUE(idem_key) + redacted; PII columns tagged; the
  residency-pin/tenant-predicate/no-untagged-personal-data lints + the forward-only migration are green; the
  writer-prompt floors are named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Notif data model — nine tenant-partitioned tables (refs-not-strings, dedup
  UNIQUE). Body lists: the nine migrations; the inbox_item invariants (refs-not-strings, dedup UNIQUE, one state
  column); the residency-pin/no-untagged-personal-data/tenant-predicate lints greened; the writer-prompt floors
  named. Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P3 — The Signal-consumer router skeleton (EventHandler, whitelist-never-*, UPSERT, outbox-only emit) + NOTIF-D10

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.0 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.0 — ... Signal-consumer
  skeleton"; this prompt is the router-skeleton slice — the EventHandler consumer + the outbox emit path + the
  NOTIF-D10 head-of-line-isolation gate).
- **DEPENDS-ON.** NOTIF-P1 (the service shell + the consumer-registration seam), NOTIF-P2 (the inbox_item table
  to UPSERT into). The M0 prompts that ship the transactional outbox + idempotent-consumer template
  (2.2/2.3/2.4/2.5), the EventEnvelope (2.1), the failure-injection harness, and the contract-coverage scanner
  (master §2 M0; event-bus roadmap). The Bus M2 prompt that ships define_signal_rule + the sig.<tenant>.> Signal
  stream (3.1). The index places this after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (notifications consume Signals not raw events), §3 (references-not-payloads);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it + observability is part of the pass —
    the lag metric is the green artifact), §5 (an uncommitted contract test is no contract test);
    ../../external-insights/04-hard-problems.md §5.3 (Notif is a projection — storm-control never touches the
    audit/history; here the skeleton just UPSERTs).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.4 (the router loop, step-0
    authorize, idempotent on origin_event; at N-M2.0 the body is the skeleton — UPSERT an inbox_item from a
    Signal, no ranking/storm-control/fanout yet), §5.1 (the router is a stateless replicable consumer pool).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 2.4 (EventHandler consumer
    template — subjects() whitelist never *, ack-after-enqueue, dedup ledger, bounded prefetch, lag metric), 2.5
    (consumer_dedup ledger), 2.2 (OutboxTx::emit — the ONLY emit path), 3.1 (define_signal_rule, the
    sig.<tenant>.> Signal stream), 1.8 (the consumer_lag telemetry signal). Read 00-reconciliation-decisions.md
    ADR-19 reference (Notif consumes Signals, not evt.*).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.0 (the router-as-EventHandler bullet + the
    emit-only-via-outbox bullet + the NOTIF-D10 gate).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D10 (slow/poison Signal → no stall; lag alarm); §3.3 (assertions read from production telemetry).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - The router as an EventHandler (2.4) consumer of Signals, registered into the NOTIF-P1 AppSpec consumer seam:
    subjects() returns the sig.<tenant>.> whitelist (NEVER *); idempotent on origin_event / event_id via the
    consumer_dedup ledger (2.5); ack-after-enqueue; bounded prefetch; the consumer-lag metric exported (1.8). At
    N-M2.0 the router's body is the SKELETON: it UPSERTs an inbox_item from a Signal (no ranking/storm-control/
    fanout/humanise yet — those are NOTIF-P5..P13). It must not stall on a poison/slow Signal type: a
    NonRetryable verdict terminates a poison Signal, the lag alarm fires, other subjects keep flowing
    (head-of-line isolation).
  - The emit path: emit notif.item.created / notif.escalation.acked ONLY via OutboxTx::emit (2.2) — the
    no-raw-publish lint forbids any other path; there is no publish_now in this crate.
  - FLOOR named: the router's classify/score/storm-control/fanout/humanise body is explicitly NOT in this prompt
    — name the follow-on prompts (NOTIF-P5 list_inbox, NOTIF-P8 ranking, NOTIF-P11..P13 storm-control+fanout,
    NOTIF-P6 humanise) so the skeleton is not mistaken for the working router.
- **CONTRACTS TO IMPLEMENT.** None owned with a body (the router consumes; it owns no exposed contract here).
  Consumed: 2.4 EventHandler template, 2.5 consumer_dedup, 2.2 OutboxTx::emit, 3.1 the Signal stream, 1.8 the
  lag signal. Implement to the frozen shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D10 (CI): inject a slow/poison Signal type → the whitelisted-template router does not stall, the
    poison terminates (NonRetryable), other subjects keep flowing, the lag alarm fires. Telemetry green
    artifact: consumer_lag bounded (signal 1.8); no-stall asserted. Threshold: 0 head-of-line stalls; lag below
    the thresholds-file default.
  - The harness self-test (CI): inject a synthetic Signal → assert inbox_item UPSERTed AND the
    telemetry-assertion library reads consumer_lag (observability is part of the pass condition, EI-01 §3).
  - The no-raw-publish lint green (no publish path other than OutboxTx::emit) — CI.
- **TESTS (required).** Unit tests for the router's idempotency (a re-delivered Signal UPSERTs once via the
  origin_event dedup) and the whitelist (subjects() is sig.<tenant>.>, never *). The drill-harness scenario for
  NOTIF-D10. The provider + consumer CDC pair for the Notif consumption of 2.4/3.1. The router is
  mandatory-core: state the cargo-mutants mutation-score floor for the router module in this field and meet it.
  Prefer a Signal-in → UPSERT → re-deliver → assert-single chain over a single-handler test (EI-01 §4).
- **DEFINITION OF DONE.** The router consumes Signals idempotently with the whitelist (never *); emits only via
  OutboxTx::emit; NOTIF-D10 emits its dated green artifact (PROVEN: no stall + lag alarm); the harness self-test
  passes with the telemetry assertion; the no-raw-publish lint + the CDC + the coverage scanner are green; the
  algorithm-body floors (NOTIF-P5..P13, NOTIF-P6) are named in writing; the work is committed. No gate greened
  by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: Signal-consumer router skeleton — whitelist EventHandler + outbox emit.
  Body lists: the router consumes sig.<tenant>.> idempotently (never *); NOTIF-D10 greened (0 stalls, lag-alarm
  fired, measured lag); the router mutation-score measured; the no-raw-publish lint green; the algorithm-body
  floors named. Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P4 — Register Notif as a PersonalDataHolder (references-not-payloads tombstone-for-free; the holder half of 7.7)

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.0 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.0 — Holder + ..."; this
  prompt is the holder-registration slice — the PersonalDataHolder surface + the structural references-not-
  payloads erase; the reindex/replay half of 7.7 lands in NOTIF-P17, the erasure residual in NOTIF-P27).
- **DEPENDS-ON.** NOTIF-P2 (the inbox_item table storing ArtifactRefs — the structural basis of tombstone-for-
  free). The M1 prompts that ship the PersonalDataHolder trait + auto-registration + KMS per-subject DEK (10.1,
  1.4, 11.3/11.4) and the restore-verify CI job (11.5). The index places this after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction — "we forgot notification history" structurally impossible;
    references-not-payloads); ../../external-insights/04-hard-problems.md §1 (erasure: an erased actor
    humanises to [erased user] with no stored PII to scrub — references-not-payloads does the work);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors — the off-cell residual is a
    named floor, NOT silently claimed done here).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.9 (the PersonalDataHolder:
    Notif IS the notification-history holder locate/export/rectify/restrict/erase, auto-registered by
    serve(AppSpec); references-not-payloads tombstones an erased person's appearance in every inbox for free;
    most of the inbox needs no mutation on erasure), §2.1 (template_args holds refs not strings — what makes
    erasure free).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.7 (PersonalDataHolder + replay
    — the holder half here; the replay half is NOTIF-P17), 10.1 (PersonalDataHolder surface), 1.4 (holder
    auto-registration), 11.3/11.4 (KMS per-subject DEK). Read 00-reconciliation-decisions.md X-7 reference (the
    one erasure posture — the off-cell residual is by-reference, instanced in NOTIF-P27).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.0 (the holder bullet + the Floor note: the
    off-cell-payload residual → N-M5.2) + §3 (contract 7.7 holder half).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D6 context (the full erasure drill is NOTIF-P27; here the structural tombstone-for-free is the unit
    property).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - Register Notif as a PersonalDataHolder (notification history) via the harness auto-registration (1.4 / 10.1)
    so "we forgot notification history" is structurally impossible. Implement locate/export/rectify/restrict the
    holder surface (7.7) over the inbox_item/delivery tables.
  - The structural erase (references-not-payloads): because inbox_item stores ArtifactRefs (NOTIF-P2), erasing a
    person tombstones their appearance in every inbox FOR FREE — the erase wires structurally with no PII
    mutation on the refs-stored items. The DEK-crypto-shred of inline-PII delivery columns + the provider-side
    erasure-request + the restrict-suppression completion are deferred (named below).
  - FLOOR named (write it in the module doc): the holder's off-cell-payload erasure residual is handled BY
    REFERENCE to the platform posture (X-7 / contract 10.9), instanced for Notif in NOTIF-P27 (N-M5.2); the
    reindex/replay half of 7.7 is NOTIF-P17. Name them so the holder is not mistaken for the full erasure path.
- **CONTRACTS TO IMPLEMENT.** 7.7 PersonalDataHolder (owned, the holder half — registration + locate/export/
  rectify/restrict + the references-not-payloads tombstone-for-free; the replay half lands in NOTIF-P17, the
  erasure residual in NOTIF-P27). Consumed: 10.1/1.4 holder auto-reg, 11.3/11.4 KMS DEK. Frozen signatures only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The holder auto-registration self-test (CI): serve(AppSpec) auto-registers Notif as a holder over the
    inbox_item/delivery tables; locate returns the subject's items. Threshold: holder present in the registry; 0
    forgotten store.
  - The structural-erase property (CI): erase a subject → a refs-stored inbox_item tombstones with NO PII
    mutation (the title resolves to a tombstone at read time via the stored ref, not by scrubbing a column).
    Threshold: 0 PII columns mutated on a refs-stored item; the item still tombstones.
  - The contract-coverage scanner passes on row 7.7 (provider + consumer CDC for the holder half) — CI.
- **TESTS (required).** Unit tests for the structural erase (a refs-stored item tombstones with no PII mutation)
  and the holder registration (auto-registered, locate finds items). The provider + consumer CDC pair for the
  7.7 holder half. The holder module is mandatory-core (erasure correctness): state the cargo-mutants
  mutation-score floor and meet it.
- **DEFINITION OF DONE.** Notif is auto-registered as a PersonalDataHolder; locate/export/rectify/restrict are
  implemented over the inbox tables; the structural references-not-payloads erase tombstones an erased person's
  appearance for free (0 PII mutation on refs-stored items); the holder self-test + the structural-erase
  property + the 7.7 holder-half CDC + the coverage scanner are green; the floors (off-cell residual NOTIF-P27;
  reindex/replay half NOTIF-P17) are named in writing; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: Notif PersonalDataHolder registration — references-not-payloads tombstone.
  Body lists: contract 7.7 (holder half) implemented; the structural-erase property greened (0 PII mutation on
  refs-stored items); the holder mutation-score measured; the floors named (off-cell residual NOTIF-P27;
  reindex/replay NOTIF-P17). Branch first if on default; do not push unless asked. End with the Co-Authored-By
  trailer.

---

### NOTIF-P5 — list_inbox (the ONE inbox) + the scoped-view filter grammar (the C-9 invariant) + CLI list/show

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.1 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.1 — The read surface +
  humanisation + ranking"; this prompt is the list_inbox read-surface slice — the ONE inbox + the filter grammar
  that makes scoped views filters, never stores).
- **DEPENDS-ON.** NOTIF-P3 (the router UPSERTs inbox_items to read), NOTIF-P2 (the inbox_item table). The M1
  Identity prompts that ship check + zookie (4.2/4.10) for step-0 read authorize. The index places this after
  the router skeleton and the Identity M1 read-path prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (the ONE inbox), §3 (one store → one read-state truth);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the C-9 invariant test forces the
    "a view is a subset" property).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §1.3 (the C-9 resolution — views are
    filters over reason/subject, never a second store; the table of Issues "My Work" / Chat "Activity" / Git
    "Review requests" as filters; one read-state truth), §1.4 (agents have inboxes too).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 7.1 (list_inbox, the ONE inbox,
    scoped views are filters); 4.2 (check — step-0 read authorize), 4.10 (zookie). Read
    00-reconciliation-decisions.md §5 (the C-9 resolution).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.1 (the list_inbox + C-9 work) + §4 (the Id
    read-path upstream deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D1 context (ranking is NOTIF-P7; here the read surface + the filter-subset invariant).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - list_inbox(principal, filter?, page?) -> [InboxItem] (7.1) — the ONE inbox. (Ranking is layered in NOTIF-P7;
    here return items in a stable order with the page cursor.) The filter grammar over reason + subject so a
    subsystem adds a SAVED VIEW, never a second store (the C-9 invariant): implement Issues "My Work" = filter
    subsystem∈{issue} ∧ reason∈{assigned, mentioned, review_requested, sla, watched, blocked, approval_requested};
    Chat "Activity/Mentions" = filter subsystem∈{chat} ∧ reason∈{mentioned, replied, thread_watched,
    approval_requested}; Git "Review requests" = filter subsystem∈{git} ∧ reason∈{review_requested, mentioned}.
    These are filters, not stores — assert in a test that they read the SAME rows as the unfiltered inbox.
  - Step-0 read authorize: list_inbox obeys check (4.2) — an item the recipient cannot see is not returned; a
    security-sensitive read carries the zookie (4.10).
  - CLI: myelin inbox list|show (the read surface; read-state is NOTIF-P6, prefs NOTIF-P10, watch NOTIF-P15).
  - FLOOR named: ranking is NOTIF-P7 (here items return unranked-but-stable); name it so the read surface is not
    mistaken for the ranked inbox.
- **CONTRACTS TO IMPLEMENT.** 7.1 list_inbox (owned). Consumed: 4.2 check, 4.10 zookie. Frozen signatures only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The C-9 invariant test (CI): a scoped filtered view returns a STRICT SUBSET of list_inbox(filter=∅) rows (a
    view is a filter, not a store). Threshold: every view's rows ⊆ the unfiltered inbox's rows; 0 rows in a view
    absent from the unfiltered inbox.
  - The contract-coverage scanner passes on 7.1 (provider + consumer CDC) — CI.
- **TESTS (required).** Unit tests for the filter grammar (each view is a subset; an unauthorized item is not
  returned). A chained test: ingest a mixed batch → list_inbox(filter=∅) and list_inbox(each view) → assert each
  view ⊆ the full inbox. The provider + consumer CDC pair for 7.1. The list-inbox module is mandatory-core:
  state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** list_inbox returns the ONE inbox; scoped views are filters (proven a subset); step-0
  authorize drops unseeable items; the C-9 invariant test + the 7.1 CDC + the coverage scanner + lints are
  green; the ranking floor (NOTIF-P7) is named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: list_inbox (the ONE inbox) + the scoped-view filter grammar (C-9). Body lists:
  contract 7.1 implemented; the C-9 invariant proven (views are subsets); the list-inbox mutation-score
  measured; the ranking floor named (NOTIF-P7). Branch first if on default; do not push unless asked. End with
  the Co-Authored-By trailer.

---

### NOTIF-P6 — Read-state: mark / snooze / mark_all_read (the one read-state truth) + CLI read

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.1 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.1 — ..."; this prompt is
  the read-state slice — mark/snooze/mark_all_read over the one state column).
- **DEPENDS-ON.** NOTIF-P5 (list_inbox + the filtered views — read-state flips across views), NOTIF-P2 (the one
  state column on inbox_item). The index places this after NOTIF-P5.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one read-state truth — read it in chat, it is read in the unified inbox);
    ../../external-insights/01-process-and-quality-doctrine.md §4 (chain mutations — mark-read in one view, assert
    read in another).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §2.1 (one read-state store, the
    state column is the same row across every view), §1.3 (the C-9 read-state truth).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 7.2 (mark/snooze/mark_all_read,
    one read-state truth). Read 00-reconciliation-decisions.md §5 (C-9, one store one read-state).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.1 (the mark/snooze/mark_all_read bullet).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §3.3 (read-state is asserted in the C-9 leg of the read-surface drills).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - mark(item_id, state) / snooze(item_id, until) / mark_all_read(filter) (7.2): ONE read-state truth — read it
    in a scoped view, it is read in the unified inbox (the state column is the same row). mark_all_read(filter)
    flips state on exactly the rows the filter (NOTIF-P5) selects.
  - snooze records the until and surfaces the snoozed-state semantics (the item is suppressed from the active
    inbox until its until). The durable re-surface TIMER wiring lands in NOTIF-P14 (the same myelin-flow wheel);
    here record the until and the snoozed state only — name that follow-on.
  - CLI: myelin inbox read|snooze (mark-read + snooze).
  - FLOOR named: the durable snooze re-surface timer is NOTIF-P14 (the myelin-flow wheel); here only the until
    is recorded. Name it.
- **CONTRACTS TO IMPLEMENT.** 7.2 mark/snooze/mark_all_read (owned). Frozen signatures only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The one-read-state-truth test (CI): mark an item read in a scoped view → it is read in the unified inbox
    (and vice versa); the state column is the same row. Threshold: 1 state per item across all views; 0
    divergence.
  - The snooze-state test (CI): snooze(item, until) → the item is suppressed from the active inbox; the until is
    recorded. Threshold: snoozed item absent from the active inbox; until persisted.
  - The contract-coverage scanner passes on 7.2 — CI.
- **TESTS (required).** Unit tests for mark/snooze/mark_all_read (the state column is one row; mark_all_read hits
  exactly the filtered rows). A chained test (EI-01 §4): ingest a batch → mark_all_read(filter) → re-list both
  the view and the full inbox → assert read-state consistent across views. The provider + consumer CDC for 7.2.
  The read-state module is mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** mark/snooze/mark_all_read flip the one read-state column consistently across every
  view; snooze records the until and suppresses the item; the one-read-state-truth test + the snooze-state test
  + the 7.2 CDC + the coverage scanner + lints are green; the durable-snooze-timer floor (NOTIF-P14) is named;
  the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: read-state — mark/snooze/mark_all_read (one read-state truth). Body lists:
  contract 7.2 implemented; the one-read-state-truth proven across views; the read-state mutation-score
  measured; the durable-snooze-timer floor named (NOTIF-P14). Branch first if on default; do not push unless
  asked. End with the Co-Authored-By trailer.

---

### NOTIF-P7 — The deterministic explainable ranking function (priority 0..100, the reason→base→class table, explain-trace) + NOTIF-D1

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.1 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.1 — ... + ranking"; this
  prompt is the ranking slice — the deterministic scoring function behind a strategy interface + the NOTIF-D1
  important-buried gate).
- **DEPENDS-ON.** NOTIF-P5 (list_inbox — ranking sorts its results). The M1 Identity prompts that ship
  list_objects/relations (4.3) for affinity/role_weight, and the M2 Refs prompt that ships backlinks (5.x) for
  affinity. The index places this after NOTIF-P5 and the Identity M1 list_objects prompt.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — deterministic-v1 ranking with ML as the named follow-on; honesty
    about uncertainty); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it: a target you
    cannot measure is not a gate — the explain-trace is the observability of the rank), §1 (name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.1 (the deterministic explainable
    scoring function priority ∈ 0..100; the reason → base → class table; affinity/role_weight from Id
    list_objects/relations + Refs backlinks behind a strategy interface; ML is the named follow-on; every rank
    carries an explain-trace, NOTIF-2).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 4.3 (list_objects SetExpr — the
    affinity/role derivation source); 7.1 (list_inbox — ranking orders it); 1.8 (the important_buried_rate +
    inbox_read_latency telemetry signals). Read 00-reconciliation-decisions.md OQ-E (the SetExpr push-down for
    affinity).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.1 (the v1 ranking work + the NOTIF-D1 gate) +
    §2 (the ranking floor → ML follow-on row).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D1 (replay a mixed week → every critical/direct ranks above every fyi; first-important latency in
    budget; explain-trace per rank; important-buried-rate 0).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - The v1 ranking function (refined §3.1): the deterministic, explainable scoring (priority ∈ 0..100); the
    reason → base → class table EXACT (approval_requested/escalated/sla = 90/critical; review_requested/assigned/
    mentioned = 70/direct; replied/agent_proposal = 55/participating; watched/state_changed = 35/watching;
    team/project fyi = 15/fyi). affinity/role_weight derived from Id list_objects/relations + Refs backlinks
    BEHIND a strategy interface (so the ML ranker swaps in without a rewrite). EVERY rank carries an explain-trace
    ("why am I seeing this, ranked here" — NOTIF-2). Wire the function into list_inbox (NOTIF-P5) as the ordering.
  - FLOOR named: ML-tuned ranking is the post-M5 follow-on behind the same scoring interface; the promotion
    trigger is a measured important-buried signal (NOTIF-D1), not a prediction. Name it.
- **CONTRACTS TO IMPLEMENT.** None owned (ranking is internal to 7.1 list_inbox). Consumed: 4.3 list_objects
  SetExpr (affinity), 5.x Refs backlinks (affinity). Frozen signatures only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D1 (SCHED): replay a mixed week of Signals → every critical/direct ranks above every fyi;
    first-important latency within the thresholds-file budget; an explain-trace present on every rank. Telemetry
    green artifact: important_buried_rate = 0 (signal 1.8); inbox_read_latency within budget. The threshold is
    important_buried_rate = 0 — never weakened.
  - The explain-trace-present check (CI): every ranked item carries a deterministic explain-trace. Threshold:
    100% of ranks carry a trace.
- **TESTS (required).** Unit tests for the scoring function (the reason → base → class table is exact; the
  explain-trace is present and deterministic; the strategy interface is swappable). The drill-harness scenario
  for NOTIF-D1. A chained test: ingest a mixed batch → list_inbox → assert every critical/direct ranks above
  every fyi + an explain-trace per rank (EI-01 §4). The ranking module is mandatory-core: state the cargo-mutants
  mutation-score floor and meet it.
- **DEFINITION OF DONE.** The deterministic ranking emits an explain-trace per rank with the exact
  reason→base→class table behind a swappable strategy interface; NOTIF-D1 emits its dated green artifact
  (important_buried_rate = 0, PROVEN); the explain-trace-present check + lints + coverage scanner are green; the
  ML-ranking floor is named in writing; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: deterministic explainable ranking + NOTIF-D1. Body lists: the v1 scoring
  function + explain-trace implemented (behind a strategy interface); NOTIF-D1 greened (important_buried_rate =
  0, measured first-important latency); the ranking mutation-score measured; the ML-ranking floor named. Branch
  first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P8 — define_notif_rule (the registration seam) + the stubbed Notif-owned default reason set

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.1 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.1 — ..."; this prompt is
  the define_notif_rule seam slice — the registration surface each subsystem calls in M3/M4, with the default
  set stubbed).
- **DEPENDS-ON.** NOTIF-P3 (the router classifies a Signal via the reason set), NOTIF-P7 (ranking reads the
  default_class). The index places this after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — the stubbed default set → per-subsystem enumerations);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (the inverse-signal — the seam must accept new
    registrations without a Notif change; if it gets harder each time, the seam is wrong).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.1 (the reason set the subsystems
    register), §3.4 (the router classifies a Signal's reason via define_notif_rule).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 7.6 (define_notif_rule(reason,
    dedup_tpl, default_class) — Signal class → inbox reason/priority; each subsystem registers its set). Read
    00-reconciliation-decisions.md OQ1 reference (the default set content is the M3/M4 per-subsystem enumeration).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.1 (the define_notif_rule seam bullet + the
    stubbed-default-set floor) + §2 (the stubbed reason set → per-subsystem enumerations floor row).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §3.3 (the seam is exercised by the M3/M4 accretion drills; here a seam-accepts-registration contract test).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - define_notif_rule(reason, dedup_tpl, default_class) (7.6) — the registration seam each subsystem calls in
    M3/M4: a subsystem registers a reason with its dedup template and its default class; the router (NOTIF-P3)
    classifies a Signal's reason through the registered rule; the ranking (NOTIF-P7) reads the default_class.
  - Ship the Notif-owned DEFAULT reason set STUBBED (the platform-default reasons exist as a registry, but the
    per-subsystem enumeration of the default set is the N-M3/N-M4 accretion, NOTIF-P19..P23).
  - FLOOR named: the stubbed default set → per-subsystem enumerations (Git/KN NOTIF-P19/P20; Issues/Chat/CI
    NOTIF-P21/P22/P23). Name them.
- **CONTRACTS TO IMPLEMENT.** 7.6 define_notif_rule (owned, the seam). Frozen signatures only — this is the seam
  every subsystem registers against; it must NOT diverge locally.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The seam-accepts-registration test (CI): a synthetic subsystem registers a reason via define_notif_rule with
    ZERO Notif code change; the router classifies a Signal carrying that reason. Threshold: 0 Notif code change
    to accept a new registration (the inverse-signal property).
  - The contract-coverage scanner passes on 7.6 (provider + consumer CDC) — CI.
- **TESTS (required).** Unit tests for the seam (a registered reason classifies correctly; the default_class
  drives ranking; the dedup_tpl drives the dedup_key). A contract test that a new registration needs no Notif
  change. The provider + consumer CDC for 7.6. The define_notif_rule module is mandatory-core: state the
  cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** define_notif_rule exists as the seam each subsystem registers against; the default
  reason set is stubbed (a registry, enumerated per subsystem in M3/M4); a new registration needs zero Notif
  code change; the seam test + the 7.6 CDC + the coverage scanner + lints are green; the per-subsystem-set
  floors (NOTIF-P19..P23) are named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: define_notif_rule seam + stubbed default reason set. Body lists: contract 7.6
  implemented (the seam); the seam-accepts-registration proven (0 Notif change); the define_notif_rule
  mutation-score measured; the per-subsystem-set floors named (NOTIF-P19..P23). Branch first if on default; do
  not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P9 — humanise (the ONE templating surface, per-viewer-safe) + the template store + NOTIF-D4 (0 title/PII leak)

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.1 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.1 — ... + humanisation";
  this prompt is the humanisation slice — the sole templating surface + the NOTIF-D4 leak gate; prefs/quiet-hours
  is NOTIF-P10).
- **DEPENDS-ON.** NOTIF-P5 (list_inbox — the read surface humanise renders). The M2 Refs prompts that ship
  resolve(ref, viewer, Display) + project (5.2/5.6) and have greened the Refs leak drills (REF-D1/REF-D2) —
  Notif's humanise leak drill cannot be honest before Refs' resolve-as-tombstone is proven. The M2 frozen-content
  prompt that ships myelin-content taxonomy + the WASM render target (13.1). The index places this after Refs' M2
  resolve prompt and the content freeze prompt.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction — per-viewer permission-safe; EU-sovereign);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the leak drill forces the failure),
    §4 (chain mutations — re-rendering after a permission change), §1 (name-your-floors);
    ../../external-insights/04-hard-problems.md §1 (erasure: an erased actor humanises to [erased user] with no
    stored PII to scrub — references-not-payloads).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.3 (the humanise render pipeline
    step-by-step: look up template → for each ArtifactRef arg resolve(ref, viewer, Display) → tombstone on deny
    → ICU-format; the ONE platform templating surface, no second template engine; the four load-bearing
    properties: permission-safe, erasure-safe, always-current, agent-inherited; markdown through the one
    myelin-content WASM render path; email = sanitised-HTML, CLI = plain-text), §2.5 (humanise_template, ICU
    MessageFormat, platform-defaulted + tenant/locale-overridable).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.3 (humanise — the ONE
    templating surface, OQ-L; resolves each ArtifactRef per-viewer via Refs resolve(Display); permission/
    erasure-safe; ICU), 5.2 (resolve(ref, viewer, Display) → Projection|Tombstone), 5.6 (project), 13.1
    (myelin-content taxonomy + WASM render target, render(parse(md)) === md). Read
    00-reconciliation-decisions.md OQ-L (sole templating surface), OQ-I (resolution is cell-local).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.1 (the humanise work + the NOTIF-D4 gate) + §4
    (the Refs + content upstream deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D4 (notify on a confidential subject to a viewer lacking access → humanised tombstone; title never
    appears; item suppressed if recipient can't see subject; 0 title/PII leak).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - humanise(item | (template_key, args), viewer, locale) -> HumanisedString{text, links[], icon} (7.3) — the
    ONE platform templating surface. The render pipeline (refined §3.3): (1) look up
    humanise_template[(tenant|default), template_key, viewer.locale]; (2) for EACH ArtifactRef arg, call Refs
    resolve(ref, viewer, mode=Display) — PER-VIEWER, permission-checked; if the projection is a Tombstone, bind
    the slot to the tombstone display ("a restricted issue" / "[erased user]"); else bind to proj.title (+
    proj.icon + a click-route to the ArtifactRef); (3) ICU-format → the final string + the routable links.
    Markdown in humanised strings renders through the ONE myelin-content WASM render path (13.1) — NEVER leaked
    raw; email gets a sanitised-HTML projection, CLI gets plain-text (one content model, many channel
    projections — never per-channel string maps). The render is permission-safe BY CONSTRUCTION: a confidential
    subject humanises to a tombstone, the title never leaks; and the router (NOTIF-P3) suppresses an item whose
    subject the recipient cannot see.
  - The humanise_template store (refined §2.5): ICU MessageFormat, platform-defaulted + tenant/locale-
    overridable (the humanise_template table from NOTIF-P2).
  - FLOOR named: cross-cell humanisation is single-home-cell here; the always-cell-local resolution rule (OQ-I)
    is built into the resolve-call shape but the multi-cell aggregation is NOTIF-P24 (N-M5.1). Name it.
- **CONTRACTS TO IMPLEMENT.** 7.3 humanise (owned, the sole templating surface). Consumed: 5.2 resolve(Display),
  5.6 project, 13.1 myelin-content + WASM render. Frozen signatures only — the humanise signature is the sole
  templating surface every other subsystem registers against (CI HumanisedRef, KN/Issues templates), so it must
  NOT diverge locally.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D4 (CI): notify on a confidential issue / private channel to a viewer lacking access → humanise
    returns a tombstone, the title NEVER appears in the output; the item is suppressed if the recipient cannot
    see the subject. Telemetry/assert green artifact: 0 title/PII leak (the F1 leak floor). The threshold is 0 —
    never inverted, never softened. This is the M2-exit obligation for Notif (master §2 M2 exit gate names
    NOTIF-D4) and re-runs against real subjects in NOTIF-P19/P20.
  - A render-determinism check (CI): render(parse(md)) === md for humanised markdown through the one WASM path
    (the content-crate round-trip generalised to humanise output).
- **TESTS (required).** Unit tests for the render pipeline (a tombstone binds on deny; an erased actor →
  [erased user]; ICU plural/locale formatting; the markdown path never leaks raw). A chained test (EI-01 §4):
  render an item for a viewer WITH access (title shown) → revoke access (a new zookie) → re-render → assert the
  title is now a tombstone (the per-viewer property under a mid-flight permission change). The drill-harness
  scenario for NOTIF-D4. The provider + consumer CDC pair for 7.3. humanise is mandatory-core (every channel
  renderer leans on it): state the cargo-mutants mutation-score floor for the render module and meet it.
- **DEFINITION OF DONE.** humanise resolves each ref per-viewer and tombstones on deny; the title never leaks;
  markdown renders through the one WASM path; NOTIF-D4 emits its dated green artifact (0 title/PII leak, PROVEN
  — the F1 floor proven before any real subsystem subject flows); the render-determinism + CDC + lints +
  coverage scanner are green; the cross-cell floor (NOTIF-P24) is named; the work is committed. A red leak gate
  is never made green by inverting the assertion.
- **COMMIT.** Header: P-<NNN> M2: humanise (the ONE templating surface) + NOTIF-D4. Body lists: contract 7.3
  implemented; NOTIF-D4 greened (0 title/PII leak, measured); the render mutation-score measured; the floor
  named (cross-cell humanise NOTIF-P24). Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### NOTIF-P10 — prefs / quiet-hours over the frozen QueryAst (pierce_classes; recipient-tz evaluation) + CLI prefs

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.1 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.1 — ..."; this prompt is
  the prefs/quiet-hours slice — the matcher binds the frozen QueryAst, with pierce_classes).
- **DEPENDS-ON.** NOTIF-P2 (the notif_pref/quiet_hours tables), NOTIF-P3 (the router routes through prefs). The
  M2 frozen-shared-crate prompt that ships myelin-query QueryAst (13.3) (= the EventMatcher core, 3.4). The index
  places this after the query-freeze prompt.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one predicate language — Notif invents no second matcher);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the cost-bound is a static
    property), §1 (name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §2.2 (the preference matcher binds
    the frozen QueryAst = the EventMatcher core; quiet-hours in the recipient tz; critical/escalated pierce by
    default via pierce_classes — the one deliberate quiet-hours override; you cannot silence an on-call page).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.4 (get_prefs/set_prefs —
    matcher reuses the QueryAst core), 13.3 (myelin-query QueryAst), 3.4 (EventMatcher = the QueryAst). Read
    00-reconciliation-decisions.md OQ-C/X-3 (the frozen QueryAst).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.1 (the get_prefs/set_prefs bullet + the
    QueryAst pin + pierce_classes) + §4 (the query upstream dep).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §3.3 (the matcher cost-bound + the pierce property; the full pierce drill is NOTIF-D8 in NOTIF-P14).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - get_prefs/set_prefs(principal, routing, quiet_hours, digest) (7.4): the matcher reuses the frozen
    myelin-query QueryAst core (13.3 = the EventMatcher 3.4) — Notif does NOT invent a second predicate language.
    Quiet-hours evaluated in the recipient's tz; critical/escalated pierce by default (pierce_classes — the one
    deliberate quiet-hours override; you cannot silence an on-call page). The router (NOTIF-P3) routes a Signal's
    delivery through route(prefs, reason, class) ∩ ¬quiet_hours (unless pierce).
  - CLI: myelin inbox prefs; myelin notify prefs|test.
  - FLOOR named: the digest cadence/batching UX is a Phase-6 product surface (refined §10 OQ5); here the digest
    field is stored but the compose/batch flow is out of scope. Name it.
- **CONTRACTS TO IMPLEMENT.** 7.4 get_prefs/set_prefs (owned). Consumed: 13.3/3.4 the QueryAst. Frozen
  signatures only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The QueryAst-matcher cost-bound (CI): a preference matcher predicate is statically cost-bounded, no
    UDFs/loops/recursion (the frozen QueryAst property). Threshold: every matcher predicate is statically
    cost-bounded; 0 unbounded predicate accepted.
  - The pierce-class unit property (CI): a critical/escalated item pierces quiet-hours by default; a
    non-critical item in quiet-hours is suppressed. Threshold: critical pierces; non-crit suppressed (the full
    drill NOTIF-D8 is NOTIF-P14).
  - The contract-coverage scanner passes on 7.4 — CI.
- **TESTS (required).** Unit tests for the matcher (binds the frozen QueryAst; quiet-hours in recipient tz;
  pierce_classes pierces critical, suppresses non-crit). A cost-bound test (an unbounded predicate is rejected).
  The provider + consumer CDC for 7.4. The prefs/matcher module is mandatory-core: state the cargo-mutants
  mutation-score floor and meet it.
- **DEFINITION OF DONE.** prefs/quiet-hours bind the frozen QueryAst with pierce_classes; quiet-hours evaluate
  in the recipient tz; the cost-bound check + the pierce-class property + the 7.4 CDC + the coverage scanner +
  lints are green; the digest-UX floor is named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: prefs/quiet-hours over the frozen QueryAst (pierce_classes). Body lists:
  contract 7.4 implemented; the QueryAst cost-bound + pierce-class properties proven; the prefs mutation-score
  measured; the digest-UX floor named. Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### NOTIF-P11 — The five write-time storm-control mechanisms (suppresses delivery/ranking, never the audit) + NOTIF-D2

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.2 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.2 — Storm-control + the
  write/read fanout split"; this prompt is the storm-control slice — the five write-time mechanisms + the
  NOTIF-D2 gate; the fanout split is NOTIF-P12/P13).
- **DEPENDS-ON.** NOTIF-P3 (the router pipeline, between classify and UPSERT), NOTIF-P2 (the dedup UNIQUE +
  coalesce_count). NOTIF-P10 (prefs/mute — the mute/DND mechanism reads prefs). The index places this after
  those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable from day 1); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it — the storm drill forces the storm); ../../external-insights/04-hard-problems.md §5.3 (Notif is a
    projection — storm-control suppresses delivery and ranking, NEVER the audit/history).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.2 (the five write-time
    storm-control mechanisms: self-suppression actor==recipient → drop; dedup-key collapse ON CONFLICT DO UPDATE
    SET coalesce_count+1 → "+N more"; thread/subject coalescing — digest the participating, break out the direct;
    per-(recipient, subject_root) token-bucket rate damping; mute/DND honoring; storm-control suppresses delivery
    and ranking, never the audit/history).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 1.8 (the dedup_collapse_ratio
    telemetry signal); the inbox_item dedup UNIQUE + coalesce_count from NOTIF-P2. Read
    00-reconciliation-decisions.md the Notif-is-a-projection reference (EI-04 §5.3).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.2 (the five mechanisms + the NOTIF-D2 gate).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D2 (1000 near-identical CI failures + a 30-comment PR burst → bounded items, coalesce_count
    correct; 0 self-notifications; dedup-collapse-ratio).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate, in the
  router pipeline (NOTIF-P3) between classify and UPSERT:
  - The five write-time storm-control mechanisms (refined §3.2): (1) self-suppression (actor.principal ==
    recipient → drop); (2) dedup-key collapse (INSERT ... ON CONFLICT (tenant, recipient, dedup_key) DO UPDATE
    SET coalesce_count = coalesce_count + 1 → "+N more"); (3) thread/subject coalescing (digest the
    participating, break out the direct); (4) per-(recipient, subject_root) token-bucket rate damping; (5)
    mute/DND honoring (reads prefs/mute from NOTIF-P10/P2). Storm-control suppresses DELIVERY and RANKING only —
    NEVER the audit/history (the events still exist on the bus; Notif is a projection, EI-04 §5.3).
  - FLOOR named: the hot-subject cap that bounds the write-fanout side is NOTIF-P12; the read-fanout is
    NOTIF-P13. Name them so storm-control is not mistaken for the full scale answer.
- **CONTRACTS TO IMPLEMENT.** None owned (internal router scaling). The dedup-collapse uses the inbox_item dedup
  UNIQUE (NOTIF-P2); the lag/collapse signals are 1.8. Frozen shapes only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D2 (CI): 1000 near-identical CI failures + a 30-comment PR burst → bounded items (coalesce_count
    correct, "+N more"); self-notifications suppressed (actor==recipient). Telemetry green artifact:
    dedup-collapse-ratio measured + asserted; 0 self-notifications. Threshold: N identical → 1 item; 0 self.
  - The audit-untouched check (CI): storm-control suppresses delivery/ranking but the underlying events still
    exist on the bus / in the audit. Threshold: 0 audit/history rows suppressed.
- **TESTS (required).** Unit tests for each of the five mechanisms (self-suppression, dedup-collapse,
  coalescing, token-bucket, mute). A chained test (EI-01 §4): emit a burst → assert coalesce_count increments on
  the SINGLE row (not N rows); a separate burst from the recipient themselves → 0 items. The drill-harness
  scenario for NOTIF-D2. The storm-control module is mandatory-core: state the cargo-mutants mutation-score
  floor and meet it.
- **DEFINITION OF DONE.** The five storm-control mechanisms suppress delivery/ranking but never the audit;
  NOTIF-D2 emits its dated green artifact (N→1, 0 self, measured collapse-ratio, PROVEN); the audit-untouched
  check + lints + coverage scanner are green; the fanout floors (NOTIF-P12/P13) are named; the work is
  committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: five-mechanism storm-control + NOTIF-D2. Body lists: the five mechanisms
  implemented (audit untouched); NOTIF-D2 greened (N→1, 0 self, measured collapse-ratio); the storm-control
  mutation-score measured; the fanout floors named (NOTIF-P12/P13). Branch first if on default; do not push
  unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P12 — Write-fanout for the bounded high-signal set (the frozen mention(Principal) structured node + the hot-subject cap)

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.2 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.2 — ... + the write/read
  fanout split"; this prompt is the write-fanout slice — materialise one inbox_item per recipient from the
  frozen mention node, bounded by the hot-subject cap).
- **DEPENDS-ON.** NOTIF-P3 (the router), NOTIF-P11 (storm-control — write-fanout sits after classify). The M2
  frozen-content prompt that ships the mention(Principal) inline node (13.1). The index places this after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable — the fan-out scale axis); ../../external-insights/04-hard-problems.md §5.3
    (Notif reads the structured node, never parses free text — AG-6); §2.2 reference (the bounded high-signal
    set);  ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.5 (write-fanout for the bounded
    high-signal set: mentioned/assigned/reviewer/escalation targets; the mention(Principal) frozen inline
    structured node — Notif reads the STRUCTURED node, never parses free text, AG-6; materialise one inbox_item
    per recipient; the hot-subject cap §3.2.4 bounds even the write-fanout side so a mention-storm can't
    write-amplify).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 13.1 (the mention(Principal)
    inline node in the myelin-content taxonomy, frozen, identical across Chat/Issues/Knowledge). Read
    00-reconciliation-decisions.md X-2 (the mention node byte-identical, C10).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.2 (the write-fanout bullet + the hot-subject
    cap).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D2 (the mention-storm side — the hot-subject cap bounds write-amplification; asserted with NOTIF-P13's
    read-fanout check).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate, in the
  router pipeline (NOTIF-P3):
  - Write-fanout for the bounded high-signal set (mentioned/assigned/reviewer/escalation targets): read the
    mention(Principal) frozen inline structured node from the myelin-content taxonomy (13.1) — Notif reads the
    STRUCTURED node, it does NOT parse free text (AG-6 — only a structured ref re-triggers) — and materialise one
    inbox_item per recipient (UPSERT through the NOTIF-P11 storm-control collapse).
  - The hot-subject cap (§3.2.4) bounds even the write-fanout side so a mention-storm can't write-amplify: beyond
    the cap, a hot subject's further mentions coalesce rather than materialise N new rows.
  - FLOOR named: the read-fanout for the unbounded ambient set is NOTIF-P13; name it.
- **CONTRACTS TO IMPLEMENT.** None owned. Consumed: 13.1 the mention(Principal) node. Implement to the frozen
  node — no free-text parsing, no local re-invention of the mention shape.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The mention-write-fanout check (CI): a Signal carrying mention(Principal) nodes materialises exactly one
    inbox_item per mentioned recipient; Notif reads the structured node, never free text. Threshold: 1 item per
    mentioned recipient; 0 free-text parse.
  - The hot-subject-cap check (CI): past the cap, a mention-storm on a hot subject coalesces rather than
    write-amplifies. Threshold: write rows bounded by the cap; 0 unbounded write amplification (part of NOTIF-D2,
    proven jointly with NOTIF-P13).
- **TESTS (required).** Unit tests for the write-fanout (one item per mentioned recipient from the structured
  node; no free-text parse; the hot-subject cap bounds the burst). A chained test: a mention-storm → assert
  bounded write rows. The provider + consumer CDC for the Notif consumption of 13.1. The write-fanout module is
  mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** Write-fanout materialises one inbox_item per recipient from the frozen mention node
  (never free text); the hot-subject cap bounds write-amplification; the mention-write-fanout + hot-subject-cap
  checks + the 13.1 CDC + lints + coverage scanner are green; the read-fanout floor (NOTIF-P13) is named; the
  work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: write-fanout (frozen mention node) + hot-subject cap. Body lists: write-fanout
  from the structured mention(Principal) node (no free-text parse) + the hot-subject cap; the write-fanout
  mutation-score measured; the read-fanout floor named (NOTIF-P13). Branch first if on default; do not push
  unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P13 — Read-fanout for the unbounded ambient set (the SetExpr watcher push-down JOIN + the zookie watermark)

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.2 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.2 — ..."; this prompt is
  the read-fanout slice — one coalesced marker materialised per-watcher lazily via the SetExpr push-down, the
  load-bearing 50k-watcher scale answer).
- **DEPENDS-ON.** NOTIF-P5 (list_inbox — the read-fanout materialises on inbox open), NOTIF-P11 (storm-control —
  the coalesced marker), NOTIF-P12 (write-fanout — the bounded set is materialised, the ambient set is read).
  The M1 Identity prompts that ship list_subjects + list_objects SetExpr + the per-tenant authz reverse index +
  zookie (4.3/4.4/4.10), pinned performant at 50k-member channel density. The index places this after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable from day 1 — the read-fanout scale axis);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the 50k-watcher amplification
    check), §1 (name-your-floors — synthetic-watcher floor → real fragments);
    ../../external-insights/04-hard-problems.md §5.3 (Notif is a projection).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.5 (read-fanout for the unbounded
    ambient set: store ONE coalesced marker, materialise per-watcher LAZILY on inbox open, a 50k-watcher
    celebrity costs zero write amplification; the watcher resolution via list_objects(recipient, watch, type) →
    Filter{set_expr, zookie} lowered into a SQL JOIN against the authz_visible reverse index over Notif's own
    subject_root/subject column — one query, no N+1, no post-filter; the zookie watermark; an item held not
    leaked if a check can't resolve fresh), §5.3 (fail-static: held, not leaked).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.3 (list_objects SetExpr
    push-down, lowered to a SQL JOIN over the consumer's own id column via the authz reverse index, no N+1, no
    post-filter), 4.4 (list_subjects, performant at 50k-member channel density, served by the same reverse
    index), 4.10 (zookie — a just-revoked watch reflected at-or-after the zookie watermark). Read
    00-reconciliation-decisions.md OQ-E (the watcher push-down), the search-requires-acl-filter discipline
    generalised.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.2 (the read-fanout bullet + the
    synthetic-watcher floor) + §2 (the synthetic-watcher → real-fragment floor row).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D2 (the read-fanout amplification leg: a 50k-watcher subject → 0 per-watcher write rows; one JOIN
    on inbox open).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate, in the
  router pipeline (NOTIF-P3) + the list_inbox read path (NOTIF-P5):
  - Read-fanout for the unbounded ambient set (every watcher of a hot PR, every member of a 50k-channel): store
    ONE coalesced marker, materialise per-watcher LAZILY on inbox open. Resolve watchers via
    list_objects(recipient, watch, type) → Filter{set_expr, zookie} (4.3) and lower the SetExpr
    (InRelation{relation: watcher, via_column} / TupleSet forms) into a SQL JOIN against the authz_visible
    reverse index over Notif's own inbox_item.subject_root / subject column — ONE query, no N+1, no post-filter
    (the search-requires-acl-filter discipline generalised to the inbox read). A 50k-watcher celebrity subject
    costs ZERO write amplification.
  - The zookie watermark (4.10): a security-sensitive read passes the zookie so a just-revoked watch grant is
    reflected (the JOIN reads the reverse index at-or-after the zookie watermark); an item is HELD, not leaked,
    if a check can't resolve fresh (§5.3).
  - FLOOR named: the read-fanout depends on every watchable subsystem declaring its watcher ReBAC fragment (4.9,
    C8) — those fragments land WITH their subsystems in M3/M4 (NOTIF-P19/P20 Git/KN; NOTIF-P21/P22 Issues/Chat).
    Until then the read-fanout is drilled against SYNTHETIC watcher tuples. Name the follow-on.
- **CONTRACTS TO IMPLEMENT.** None owned. Consumed: 4.3 list_objects SetExpr (the watcher push-down — the
  highest-fan-in dependency), 4.4 list_subjects (50k-member density), 4.10 zookie. Implement to the frozen
  SetExpr lowering — no local re-invention of a watcher resolution path.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The read-fanout-amplification check (CI, with the synthetic-watcher fixture): a 50k-watcher subject produces
    ZERO per-watcher write rows (one coalesced marker), and an inbox open materialises only the viewer's slice
    via one JOIN (no N+1). Threshold: 0 write amplification; 1 JOIN per inbox open.
  - The zookie-watermark check (CI): a just-revoked watch is reflected at-or-after the zookie watermark; an item
    is held, not leaked, on a stale check. Threshold: 0 leaked item on a revoked watch.
- **TESTS (required).** A read-fanout test against synthetic watcher tuples asserting one JOIN, zero write
  amplification, and the zookie watermark reflecting a revoked watch. A chained test: revoke a watch (new
  zookie) → open the inbox → assert the revoked subject is absent (held, not leaked). The provider + consumer
  CDC for the Notif consumption of 4.3/4.4. The read-fanout module is mandatory-core: state the cargo-mutants
  mutation-score floor and meet it.
- **DEFINITION OF DONE.** The read-fanout resolves watchers via the SetExpr JOIN (one query, no N+1) with the
  zookie watermark (held, not leaked); a 50k-watcher subject costs zero write amplification; the
  read-fanout-amplification + zookie-watermark checks + the 4.3/4.4 CDC + lints + coverage scanner are green; the
  synthetic-watcher floor (real fragments NOTIF-P19..P22) is named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: read-fanout (SetExpr watcher push-down JOIN + zookie watermark). Body lists:
  the read-fanout JOIN (one query, no N+1, 0 write amplification) + the zookie watermark; the read-fanout
  mutation-score measured; the synthetic-watcher floor named (real fragments NOTIF-P19..P22). Branch first if on
  default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P14 — Escalation on the myelin-flow durable wheel (the frozen chain shape; ack-as-event) + NOTIF-D7 + NOTIF-D8

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.3 — Escalation on the
  durable wheel + ..."; this prompt is the escalation slice — page/oncall_now on the durable workflow + the
  NOTIF-D7 exactly-once and NOTIF-D8 pierce gates; snooze re-surfacing on the same wheel is NOTIF-P18).
- **DEPENDS-ON.** NOTIF-P3 (the router + outbox), NOTIF-P10 (prefs/pierce_classes — the critical-class pierce),
  NOTIF-P2 (escalation_policy/escalation_run tables). The M2 myelin-flow prompts that ship
  DurableExecutor{start, signal, describe, cancel} + the durable timer wheel + the durable signal (9.1/9.3/9.4)
  and have greened FLOW-D1/FLOW-D2/FLOW-D5 (the durable-execution drills) — NOTIF-D7's exactly-once page rests on
  durable timers, which must be proven first. The index places this after the myelin-flow M2 prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — agent escalations too; honesty about uncertainty);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — kill mid-ack_window forces the
    failure; observability is part of the pass).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §2.4 (the escalation-chain config
    shape FROZEN: page → oncall_now → notify(class=critical, pierces quiet-hours) → escalate-after-timer
    (ack_window) → if !acked next-step / if acked stop; Issues passes the chain definition; Notif owns POLICY
    evaluation, the workflow engine owns DURABILITY; the timers are myelin-flow durable timers not in-process
    sleeps; ack is an event notif.escalation.acked via outbox, the workflow signal-wait resolves on it; on-call
    cannot be silenced, pierce_classes default critical), §3.7 (escalation on the durable-workflow substrate).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.5 (oncall_now(schedule) →
    principal; page(target, reason) starts an escalation durable workflow; the chain shape frozen), 9.1
    (DurableExecutor, signal idempotent on idem_key), 9.3 (the durable timer wheel — millions of timers as an
    indexed range read, effectively-once), 9.4 (durable signal — state=waiting holds no runtime, an ack/cancel
    signal arrives later, idempotent), 2.2 (OutboxTx::emit — the ack event). Read 00-reconciliation-decisions.md
    §5 (the escalation chain) + OQ-F (the per-effect idem_key).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.3 (the escalation work + the NOTIF-D7/D8
    gates) + §4 (the myelin-flow upstream dep).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows NOTIF-D7 (start escalation; kill Notif mid-ack_window → durable workflow resumes, pages next step
    exactly once; ack stops the chain; exactly-once page; ack-halt) and NOTIF-D8 (set DND; fire a critical
    escalation → it pierces quiet-hours; a watching item is suppressed; critical pierces; non-crit suppressed).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - oncall_now(schedule) -> principal and page(target, reason) (7.5): page starts an escalation DURABLE WORKFLOW
    on the myelin-flow substrate (9.1/9.3/9.4) walking the frozen chain shape (refined §2.4): page(target,
    reason) → oncall_now(schedule) resolves the rotation at fire time → notify(principal, channels,
    class=critical) which pierces quiet-hours → escalate-after-timer(ack_window) a myelin-flow DURABLE TIMER that
    survives a Notif restart and fires effectively-once → if !acked walk to the next step; if acked stop. Notif
    owns the POLICY evaluation (which step, which target, which channels); the workflow engine owns the
    DURABILITY (the timer is a 9.3 durable timer, not an in-process sleep). Ack is an EVENT
    (notif.escalation.acked emitted via OutboxTx::emit, 2.2); the workflow's signal-wait (9.4) resolves on it.
    On-call cannot be silenced (pierce_classes default critical — you cannot silence an on-call page).
  - Wire the escalation_policy / escalation_run tables (NOTIF-P2) to the workflow: the escalation_run row holds
    the durable handle, so a restart resumes the chain, never misses or double-pages.
  - CLI: myelin oncall show|page.
  - FLOOR named: Issues passes its real SLA escalation chain definition in N-M4 (NOTIF-P21); here the chain shape
    is exercised with a Notif-defined test chain. snooze re-surfacing on the same wheel is NOTIF-P18. Name both.
- **CONTRACTS TO IMPLEMENT.** 7.5 oncall_now/page (owned, the escalation durable workflow + the frozen chain
  shape). Consumed: 9.1 DurableExecutor, 9.3 the durable timer wheel, 9.4 the durable signal, 2.2 OutboxTx::emit
  (the ack event). Frozen chain shape only — Issues passes the chain, Notif evaluates it; the durability is the
  engine's.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D7 (CI): start an escalation; kill Notif mid-ack_window → the durable workflow resumes and pages the
    next step EXACTLY ONCE; an ack stops the chain. Telemetry green artifact: exactly-once page (0 missed, 0
    duplicate); ack-halt asserted; escalation_ack_latency measured (1.8). Threshold: 0 missed, 0 duplicate pages
    — never softened.
  - NOTIF-D8 (CI): set DND; fire a critical escalation → it PIERCES quiet-hours; a watching (non-critical) item
    is suppressed. Telemetry green artifact: critical pierces; non-crit suppressed; quiet_hours_pierce_count
    incremented (signal 1.8). Threshold: critical pierces; non-crit suppressed.
- **TESTS (required).** Unit tests for the chain-walk policy (step ordering, target resolution at fire time,
  pierce_classes). A chained durability test (EI-01 §4): start → kill the worker mid-ack_window → resume →
  assert one page to the next step (not zero, not two) → deliver the ack event → assert the chain halts. The
  drill-harness scenarios for NOTIF-D7 and NOTIF-D8. The escalation module is mandatory-core: state the
  cargo-mutants mutation-score floor and meet it. The provider + consumer CDC pair for 7.5.
- **DEFINITION OF DONE.** page starts a durable-workflow escalation walking the frozen chain on the myelin-flow
  wheel; ack is an outbox event the signal-wait resolves on; on-call cannot be silenced; NOTIF-D7 (exactly-once
  page, 0 missed/0 dup) and NOTIF-D8 (critical pierces, non-crit suppressed) emit their dated green artifacts
  (PROVEN); CDC + lints + coverage scanner are green; the floors (Issues real chain NOTIF-P21; snooze re-surface
  NOTIF-P18) are named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: escalation on the durable wheel (frozen chain, ack-as-event) + NOTIF-D7/D8.
  Body lists: contract 7.5 implemented; NOTIF-D7 greened (exactly-once page, 0 missed/0 dup) + NOTIF-D8 greened
  (critical pierces, non-crit suppressed); the escalation mutation-score measured; the floors named (Issues real
  chain NOTIF-P21; snooze re-surface NOTIF-P18). Branch first if on default; do not push unless asked. End with
  the Co-Authored-By trailer.

---

### NOTIF-P15 — The inbox watch live transport (the frozen firehose resume-cursor protocol) + the D-N11 resume leg

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.3 — ... + the live
  transport + ..."; this prompt is the live-transport slice and ships the inbox-watch resume leg, D-N11).
- **DEPENDS-ON.** NOTIF-P3 (the router emits notif.item.created), NOTIF-P5 (list_inbox — the cold-rebuild
  fallback). The M2 Bus prompt that ships the firehose transport + the resume-cursor subscription protocol
  (subscribe/resume/scope, 3.5) — built once for the shared transport (EI-04 §2: the durable resume-cursor
  transport is built first). The index places this after the Bus firehose prompt.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable; honesty about uncertainty — a cold rebuild is named, not silent);
    ../../external-insights/04-hard-problems.md §2.2 (build the durable resume-cursor transport first — Notif
    consumes it, it does not build a bespoke live path);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — drop the connection, reconnect,
    zero items lost).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §7 (the inbox watch live transport:
    subscribe(stream=fan.<tenant>.inbox.<principal>, scope=inbox:<principal>, cursor?) → SubStream yielding
    Frame{seq, item_id}; resume(stream, scope, last_seq) backfills (last_seq, now] then live — a reconnect loses
    zero items; per-(stream, scope) monotonic seq; an over-old cursor → resync_required → full list_inbox cold
    rebuild named not silent; the scope is a BOUNDED selector inbox:<principal>, never *, the transport rejects
    an unbounded scope, BUS-3 generalised; per-connection in-flight frame caps, a slow consumer dropped to
    resync_required rather than buffering unboundedly — the connection-tier shed budget; the durable bus carries
    only the pointer event, the firehose carries the live frame, the in-app path stays in-cell).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 3.5 (the firehose transport + the
    resume-cursor subscription protocol: subscribe(stream, scope, cursor?) → SubStream, frames carry per-(stream,
    scope) monotonic seq, resume(stream, scope, last_seq) backfills then live, resync_required → snapshot
    fallback, scope is a bounded selector never *). Read 00-reconciliation-decisions.md OQ-J (the firehose
    resume-cursor protocol) + OQ-K (the connection-tier shed budget).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.3 (the inbox watch work + the resume-leg gate)
    + §4 (the firehose upstream dep) + §5 (D-N11 / the OQ-J resume-cursor family).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    the OQ-J resume-cursor family (Notif's leg = D-N11 in the refined doc §6: drop the inbox watch connection
    mid-stream, reconnect with last_seq → backfill (last_seq, now] then live, zero items lost; over-old cursor →
    resync_required → snapshot rebuild).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - The inbox watch live transport over the FROZEN firehose resume-cursor protocol (3.5) — Notif consumes the
    shared transport, it does NOT build a bespoke live path: subscribe(stream = fan.<tenant>.inbox.<principal>,
    scope = inbox:<principal>, cursor?) → SubStream yielding Frame{seq: u64, item_id, ...}; resume(stream, scope,
    last_seq) backfills (last_seq, now] from the bounded firehose retention window then resumes live — a
    reconnect loses ZERO items. The seq is per-(stream, scope) monotonic. An over-old last_seq → resync_required
    → the client falls back to a full list_inbox cold rebuild (the named, NOT-silent recovery path).
  - Per-view scope bounding: scope is the bounded selector inbox:<principal>, NEVER * — the transport rejects an
    unbounded scope (the whitelist-not-* rule, BUS-3, generalised). One client gets only its own inbox slice's
    frames, never the whole tenant's firehose.
  - Backpressure: per-connection in-flight frame caps; a slow consumer is dropped to resync_required rather than
    buffering unboundedly (the connection-tier shed budget, OQ-K). The durable bus still carries only the pointer
    event (notif.item.created); the firehose carries the live frame — the in-app delivery path stays in-cell.
  - CLI: myelin inbox watch (streams new items live over the resume-cursor path).
  - FLOOR named: the wire mechanism (long-poll vs SSE vs WebSocket) is the connection tier's, NOT Notif's; Notif
    consumes only the subscribe/resume/scope contract. State this so no bespoke wire transport is built here.
- **CONTRACTS TO IMPLEMENT.** None owned. Consumed: 3.5 the firehose subscribe/resume/scope protocol. Implement
  to the frozen protocol — no bespoke Notif live transport; the scope must be bounded (never *).
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The inbox-watch resume leg (D-N11, the OQ-J family applied to scope=inbox:<principal>) (CI): drop the inbox
    watch connection mid-stream; reconnect with last_seq → backfill (last_seq, now] then live, ZERO items lost;
    an over-old cursor → resync_required → cold rebuild via list_inbox. Telemetry green artifact: 0 items lost
    across a reconnect; resync_required path exercised. Threshold: 0 lost — never softened.
  - The bounded-scope rejection (CI): a subscribe with scope=* (or any unbounded selector) is REJECTED by the
    transport (the BUS-3-generalised whitelist property). Threshold: 0 unbounded scope accepted.
- **TESTS (required).** Unit tests for the resume-cursor math (backfill range (last_seq, now]; the
  resync_required boundary at the retention window edge). A chained test (EI-01 §4): subscribe → receive frames
  1..k → drop → emit frames k+1..m while disconnected → reconnect with last_seq=k → assert frames k+1..m
  backfilled in order then live (0 lost, 0 dup). A test that an unbounded scope is rejected. The drill-harness
  scenario for the D-N11 resume leg. The provider + consumer CDC for the Notif consumption of 3.5. The
  watch-transport module is mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** inbox watch rides the frozen firehose resume-cursor protocol (no bespoke transport); a
  reconnect loses zero items; an over-old cursor falls back to a named cold rebuild; an unbounded scope is
  rejected; the D-N11 resume leg emits its dated green artifact (0 lost, PROVEN); the bounded-scope rejection +
  CDC + lints + coverage scanner are green; the floor (the wire mechanism is the connection tier's) is named;
  the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: inbox watch live transport (firehose resume-cursor) + D-N11. Body lists: the
  subscribe/resume/scope consumption of contract 3.5 wired; the D-N11 resume leg greened (0 items lost across a
  reconnect, measured); the bounded-scope rejection proven; the watch mutation-score measured; the floor named
  (wire mechanism = connection tier). Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### NOTIF-P16 — The delivery fabric (the idempotent DeliveryAdapter trait + the deterministic mock; in-app stays in-cell) + NOTIF-D9

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.3 — ... + delivery
  idempotency"; this prompt is the delivery slice — the DeliveryAdapter trait + mock + the NOTIF-D9 exactly-once
  gate; reindex-from-source is NOTIF-P17).
- **DEPENDS-ON.** NOTIF-P3 (the router enqueues deliveries), NOTIF-P9 (humanise — the RedactedMessage summary is
  a humanise render), NOTIF-P2 (the delivery table with UNIQUE(idem_key) + redacted). The index places this
  after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign by construction — region-aware, EU-preferring delivery, data-minimisation;
    name-your-floors — mock adapter → real EU provider; agents choose, strategy pattern for the swappable
    adapter); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — crash between provider-ack
    and ledger-write).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.6 (the EU-sovereign delivery
    fabric: one trait DeliveryAdapter{channel, region, send(RedactedMessage, idem_key), receipts} — EU-preferring,
    region-aware, swappable; RedactedMessage = a humanised summary + a deep link, never the full body where
    avoidable, delivery.redacted=true off-cell, GDPR Art. 5(1)(c) data-minimisation; in-app channels
    inbox/web_push/desktop never leave the cell; at-least-once + idempotent on UNIQUE(idem_key); FLOOR: the trait
    + EU-preferring posture + redaction ship, the concrete production EU provider is a sovereignty/legal selection
    deferred; v1 dev uses a deterministic mock adapter --use-mock).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.8 (DeliveryAdapter,
    region-aware, EU-preferring, swappable, PII-minimised off-cell, at-least-once + idempotent), 1.8 (the
    delivery_success/bounce telemetry signal). Read 00-reconciliation-decisions.md the X-7 erasure posture
    reference (off-cell payloads) and the EU-sovereign delivery floor.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.3 (the delivery work + the NOTIF-D9 gate) + §2
    (the mock → EU-provider floor row).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D9 (crash between provider-ack and ledger-write, retry → UNIQUE(idem_key) collapses to exactly-one
    delivery per (item, channel); 1 effective delivery).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - The delivery fabric (7.8): the one trait DeliveryAdapter{channel, region, send(RedactedMessage, idem_key),
    receipts} — EU-preferring, region-aware, swappable (the same strategy-pattern mandate that swaps mock→real
    agents, generalised to sub-processors). RedactedMessage = a humanised summary (via NOTIF-P9's humanise) + a
    deep link, never the full body where avoidable; delivery.redacted = true for off-cell (Art. 5(1)(c)
    data-minimisation). In-app channels (inbox, web_push, desktop) NEVER leave the cell. Delivery is
    at-least-once + idempotent on UNIQUE(idem_key) in the delivery table (NOTIF-P2). Ship the DETERMINISTIC MOCK
    adapter (--use-mock-as-runtime).
  - FLOOR named: the concrete production EU email/push provider (with its DPA/sub-processor posture) is N-M5.2
    (NOTIF-P25), a sovereignty/legal [OPEN — LEGAL] selection; the trait + EU-preferring posture + redaction
    discipline ship NOW. Name it.
- **CONTRACTS TO IMPLEMENT.** 7.8 DeliveryAdapter (owned, the trait + mock). Frozen signatures only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D9 (CI): crash between provider-ack and ledger-write, retry → UNIQUE(idem_key) collapses to
    EXACTLY-ONE effective delivery per (item, channel). Telemetry green artifact: 1 effective delivery;
    delivery_success measured (signal 1.8). Threshold: exactly 1 — never softened.
  - The in-app-stays-in-cell assertion (CI): inbox/web_push/desktop channels produce no off-cell egress; an
    off-cell channel sends only a RedactedMessage with delivery.redacted=true. Threshold: 0 in-app egress; 0
    off-cell full-body.
- **TESTS (required).** Unit tests for the idem_key collapse (a retry after provider-ack is a no-op) and the
  RedactedMessage minimisation (off-cell carries summary + link, never the body; in-app stays in-cell). The
  drill-harness scenario for NOTIF-D9. The delivery module is mandatory-core: state the cargo-mutants
  mutation-score floor and meet it. The provider + consumer CDC pair for 7.8.
- **DEFINITION OF DONE.** The DeliveryAdapter trait + mock deliver at-least-once + idempotent (exactly-one per
  (item, channel)); off-cell is redacted, in-app stays in-cell; NOTIF-D9 (1 effective delivery) emits its dated
  green artifact (PROVEN); the in-app-stays-in-cell assertion + the 7.8 CDC + lints + coverage scanner are
  green; the floor (real EU provider NOTIF-P25) is named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: delivery fabric (idempotent DeliveryAdapter + mock) + NOTIF-D9. Body lists:
  contract 7.8 implemented; NOTIF-D9 greened (1 effective delivery, measured); the delivery mutation-score
  measured; the floor named (real EU provider NOTIF-P25). Branch first if on default; do not push unless asked.
  End with the Co-Authored-By trailer.

---

### NOTIF-P17 — reindex-from-source (the only recovery path; cold == live; the replay half of 7.7) + NOTIF-D3

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.3 — ... + delivery
  idempotency"; this prompt is the reindex slice — events::reindex(scope=notif) through the live consumer + the
  NOTIF-D3 parity gate; this completes the 7.7 holder contract started in NOTIF-P4. This prompt + NOTIF-P14/P15/P16
  clear Notif's M2-exit drill set).
- **DEPENDS-ON.** NOTIF-P3 (the router — reindex re-ingests through it), NOTIF-P4 (the holder), NOTIF-P16 (the
  delivery table reconstructed by reindex). The M0/M2 reindex-from-source prompt (events::reindex + the
  replay-through-the-live-consumer path, 2.6). The M1 restore-verify CI job (11.5). The index places this after
  those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — the ~90-day retention floor); ../../external-insights/04-hard-problems.md
    §5.3 (the inbox is a projection; reindex-from-source is the only recovery path — no second read path so
    steady-state and recovery share one code path, cannot drift);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — wipe and rebuild, assert cold ==
    live).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.8 (reindex-from-source:
    events::reindex(scope=notif) → owners replay *.snapshot → the SAME router re-ingests idempotently
    (origin_event dedup) → inbox_item/delivery reconstructed; cold == live; the only recovery path; doubles as
    new-recipient backfill + schema-upcaster; retention floor ~90-day item window, prefs/on-call/templates
    permanent restore-verify gated).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.7 (PersonalDataHolder + replay
    — the reindex replay half completes the holder started in NOTIF-P4), 2.6 (reindex-from-source — the only
    recovery path), 11.5 (restore-verify on the system-of-record tables). Read 00-reconciliation-decisions.md the
    reindex-is-the-only-recovery-path reference.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.3 (the reindex work + the NOTIF-D3 gate + the
    retention floor) + the M2-exit context (Notif's full M2-exit drill set is NOTIF-D7/D8/D9/D3 + the resume leg).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D3 (wipe inbox_item, reindex(notif) → rebuilt inbox matches live; reindex-parity hash).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - reindex-from-source (7.7 replay / 2.6): events::reindex(scope=notif) → owners replay *.snapshot events
    through outbox→bus→Signal → the SAME router (NOTIF-P3) re-ingests idempotently (origin_event dedup) →
    inbox_item/delivery reconstructed; cold == live. This is the ONLY recovery path (no second read path → cannot
    drift). It doubles as new-recipient backfill + the schema-upcaster path. Retention floor: ~90-day item window
    (older items age out, reconstructable from the OLAP/Audit long-term holder); prefs/on-call/templates are
    permanent and restore-verify gated (11.5).
  - FLOOR named: the ~90-day item retention window is a floor — older items reconstruct from the OLAP/Audit
    long-term holder; prefs/on-call/templates are permanent. Name the boundary.
- **CONTRACTS TO IMPLEMENT.** 7.7 replay (owned, the reindex half — completes the holder contract started in
  NOTIF-P4). Consumed: 2.6 reindex-from-source, 11.5 restore-verify (the system-of-record tables). Frozen
  signatures only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D3 (SCHED): wipe inbox_item, run reindex(notif) → the rebuilt inbox matches live (items + read-state
    from source events). Telemetry green artifact: reindex-parity hash equal (cold == live). Threshold: parity
    hash identical.
  - The single-code-path check (CI): reindex re-ingests through the SAME router (NOTIF-P3), not a second read
    path. Threshold: 0 second read path (steady-state and recovery share one code path).
- **TESTS (required).** A chained test (EI-01 §4): ingest a batch → reindex(notif) on a wiped store → assert the
  rebuilt inbox + read-state hash-equal to live. The drill-harness scenario for NOTIF-D3. The provider +
  consumer CDC pair for the 7.7 replay half. The reindex module is mandatory-core: state the cargo-mutants
  mutation-score floor and meet it.
- **DEFINITION OF DONE.** reindex-from-source rebuilds cold == live through the same router as the only recovery
  path; the ~90-day retention floor is named; NOTIF-D3 (reindex parity) emits its dated green artifact (PROVEN);
  the single-code-path check + the 7.7-replay CDC + lints + coverage scanner are green; the work is committed.
  This prompt, with NOTIF-P14/P15/P16, clears Notif's M2-exit drill set — but Notif is "done in M2" only when
  the band-wide M2 gate (incl. the hard AG-D4 sandbox-escape GATE, owned by the agent fabric) is green (the gate
  invariant, EI-01 §2). No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: reindex-from-source (only recovery path, cold == live) + NOTIF-D3. Body lists:
  contract 7.7 replay half implemented (holder completed); NOTIF-D3 greened (reindex parity hash equal); the
  reindex mutation-score measured; the retention floor named; note that Notif's M2-exit drills are green but the
  band closes only when AG-D4 is green. Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### NOTIF-P18 — Snooze re-surfacing on the same myelin-flow durable timer wheel (one substrate, three uses)

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.3 — Escalation on the
  durable wheel + ..."; this prompt is the snooze-re-surface slice — the durable timer for snooze, sharing the
  NOTIF-P14 wheel).
- **DEPENDS-ON.** NOTIF-P6 (snooze records the until), NOTIF-P14 (the myelin-flow durable timer wheel wired for
  escalation — snooze rides the same wheel). The M2 myelin-flow prompt that ships the durable timer wheel (9.3).
  The index places this after NOTIF-P14.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (honesty about uncertainty — the timer is durable, not an in-process sleep);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — kill before the until, assert the
    re-surface still fires).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.7 (snooze re-surfacing and SLA
    timers ride the same minute-bucket wheel as escalation — one substrate, three uses), §2.4 (the durable timer
    wheel is myelin-flow's, not in-process sleeps).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 9.3 (the durable timer wheel —
    millions of timers, effectively-once), 7.2 (snooze — the until recorded in NOTIF-P6). Read
    00-reconciliation-decisions.md OQ-F (the per-effect idem_key).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.3 (snooze re-surfacing rides the same wheel —
    one substrate three uses).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §3.3 (the snooze re-surface durability is asserted on the same wheel as NOTIF-D7; no separate drill row, but
    the durability property is gated).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - snooze re-surfacing on the SAME durable timer wheel (9.3): a snoozed item (NOTIF-P6's snooze records the
    until) re-surfaces at its until via a myelin-flow DURABLE TIMER that survives a Notif restart and fires
    effectively-once — one substrate, three uses (escalation NOTIF-P14, snooze here, SLA timers). At the until,
    the item's snoozed state clears and it re-enters the active inbox.
  - FLOOR named: SLA timers (the third use of the wheel) are driven by Issues' real SLA policy in N-M4
    (NOTIF-P21). Name it.
- **CONTRACTS TO IMPLEMENT.** None owned (snooze's surface is 7.2, owned in NOTIF-P6; here the durable timer is
  consumed). Consumed: 9.3 the durable timer wheel, 7.2 snooze. Frozen signatures only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The snooze-durability check (CI): snooze an item; kill Notif before the until → on restart the durable timer
    still fires effectively-once at the until and the item re-surfaces. Threshold: 0 missed re-surface, 0
    duplicate re-surface across a restart.
  - The one-substrate check (CI): snooze re-surfacing uses the SAME myelin-flow wheel as escalation (NOTIF-P14),
    not a second timer mechanism. Threshold: 0 in-process sleep; 0 second timer substrate.
- **TESTS (required).** A chained durability test (EI-01 §4): snooze → kill the worker before the until → resume
  → assert exactly one re-surface at the until (not zero, not two). The drill-harness scenario for the snooze
  re-surface durability. The snooze-timer module is mandatory-core: state the cargo-mutants mutation-score floor
  and meet it.
- **DEFINITION OF DONE.** snooze re-surfaces on the same myelin-flow durable wheel (effectively-once across a
  restart); no in-process sleep, no second timer substrate; the snooze-durability + one-substrate checks + lints
  + coverage scanner are green; the SLA-timer floor (NOTIF-P21) is named; the work is committed. No threshold
  weakened.
- **COMMIT.** Header: P-<NNN> M2: snooze re-surfacing on the durable wheel (one substrate, three uses). Body
  lists: snooze re-surface on the myelin-flow wheel (effectively-once across a restart); the snooze-timer
  mutation-score measured; the SLA-timer floor named (NOTIF-P21). Branch first if on default; do not push unless
  asked. End with the Co-Authored-By trailer.

---

### NOTIF-P19 — Producer accretion: Git registers reasons + the watcher ReBAC fragment; re-confirm NOTIF-D4 on real Git subjects (GIT-D8)

- **BAND.** M3.
- **ROADMAP MILESTONE.** N-M3 (planning/06-roadmaps/shared/notifications.md §1 "N-M3 — Producer accretion: Git +
  Knowledge register their reasons + watchers"; this prompt is the Git half — Git's define_notif_rule set + its
  watcher fragment + NOTIF-D4 re-confirmed on a real Git private repo; the Knowledge half is NOTIF-P20).
- **DEPENDS-ON.** NOTIF-P8 (define_notif_rule seam), NOTIF-P9 (humanise — the leak surface), NOTIF-P13
  (read-fanout over real watcher tuples). The M3 Git prompts that ship the Git ReBAC namespace fragment incl. the
  watcher relation (4.9 — Git ref-glob + CODEOWNERS) + the Git event taxonomy + project(ref, viewer) (5.6). The
  index places this after Git ships its M3 fragment. Notif itself is unchanged — this is pure registration.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — synthetic watcher → real fragment; honesty);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (the inverse-signal: if wiring Git's reasons
    gets HARDER, the define_notif_rule/watcher seam is wrong — stop and repair, don't add surface), §3 (prove-it
    — re-run the leak drill against REAL confidential subjects).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.5 (the watcher relation is a
    frozen ReBAC-fragment obligation, C8 — every watchable subsystem declares it, Notif reads it never invents
    it), §3.1 (the reason set the subsystems register), §3.3 (humanise per-viewer against real subjects).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.6 (define_notif_rule — Git
    registers review_requested/mentioned), 4.9 (the per-subsystem ReBAC namespace fragment — the watcher relation
    per watchable type; Git ref-glob + CODEOWNERS), 5.6 (project(ref, viewer) — the humanise projection),
    4.3/4.4 (the read-fanout watcher resolution over Git's real fragment). Read 00-reconciliation-decisions.md C8
    (watcher fragment frozen obligation).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M3 (the Git registration work + the gate) + §4
    (the M3 accretion deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D4 (re-run against a REAL Git private repo, not synthetic) + the Git confidential-leak row GIT-D8
    (Notif's resolve(Display) path is the leak surface it exercises).
- **DELIVERABLE (what to build + exactly where in the repo).** Mostly in Git's crate (the registration), with
  the verification in myelin-notif:
  - Git registers its define_notif_rule set (review_requested / mentioned) via the 7.6 seam + declares its
    watcher ReBAC fragment (4.9, Git ref-glob + CODEOWNERS) → the Git "Review requests" filtered view (NOTIF-P5)
    becomes a REAL list_inbox view; read-fanout (NOTIF-P13) over REAL PR watchers goes live (replacing the
    synthetic tuples for Git subjects).
  - In myelin-notif: verify the define_notif_rule + watcher seams accept Git's registration WITHOUT any Notif
    code change (the inverse-signal check — if it needs a change, the seam is wrong; record this explicitly).
    Re-run the humanise-per-viewer property (NOTIF-D4) against a REAL confidential subject (a Git private repo) —
    not synthetic — confirming the tombstone holds.
  - FLOOR named: Knowledge reasons + watchers are NOTIF-P20; Issues / Chat / CI reasons + watchers are M4
    (NOTIF-P21/P22/P23); cross-cell is still single-home (NOTIF-P24). Name them.
- **CONTRACTS TO IMPLEMENT.** None NEW owned by Notif (Notif gains zero new contracts). Git owns its 4.9 watcher
  fragment + 7.6 registration + 5.6 project; Notif consumes them. Verify against the frozen 7.6 / 4.9 / 5.6
  shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D4 re-confirmed on a REAL Git subject (CI): notify on a real Git private repo to a viewer lacking
    access → humanised tombstone, the title never appears. Telemetry green artifact: 0 title/PII leak against a
    real subject. Threshold: 0 — never softened.
  - GIT-D8 (CI): cross-tenant repo access denied — green with Notif's humanise path exercised. Threshold: 0
    cross-tenant leak.
  - The inverse-signal record: a written note that Git registered via the unchanged define_notif_rule / watcher
    seam with ZERO Notif code change (the seam is right) — observability of the compounding-payoff property
    (EI-01 closing).
- **TESTS (required).** Integration tests that Git's registration produces a real "Review requests" filtered
  view and real read-fanout (replacing the NOTIF-P13 synthetic fixtures for Git). The drill-harness scenario for
  NOTIF-D4 on a real Git subject. A contract test that the 7.6 / 4.9 seams accept Git's set unchanged. The CDC
  pairs for the Git side of 7.6 / 4.9 (provider Git, consumer Notif).
- **DEFINITION OF DONE.** Git registers reasons + the watcher fragment via the unchanged seams (ZERO Notif code
  change recorded); Git "Review requests" is a real view; read-fanout runs over real Git watchers; NOTIF-D4
  re-confirmed on a real Git confidential subject (0 leak, PROVEN); GIT-D8 is green with humanise exercised; the
  Git-side CDC + lints + coverage scanner are green; the floors (Knowledge NOTIF-P20; Issues/Chat/CI
  NOTIF-P21/P22/P23; cross-cell NOTIF-P24) are named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M3: producer accretion — Git reasons/watchers; NOTIF-D4 on real Git subjects.
  Body lists: Git registered via 7.6 + 4.9 (no Notif code change recorded); NOTIF-D4 re-confirmed on a real Git
  subject (0 leak); GIT-D8 greened; the floors named (Knowledge NOTIF-P20; M4 NOTIF-P21/P22/P23; cross-cell
  NOTIF-P24). Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P20 — Producer accretion: Knowledge registers reasons + the watcher ReBAC fragment; re-confirm NOTIF-D4 on real KN subjects (KN-D5/KN-D13)

- **BAND.** M3.
- **ROADMAP MILESTONE.** N-M3 (planning/06-roadmaps/shared/notifications.md §1 "N-M3 — Producer accretion: Git +
  Knowledge register their reasons + watchers"; this prompt is the Knowledge half — KN's define_notif_rule set +
  its watcher fragment + NOTIF-D4 re-confirmed on a real KN confidential page).
- **DEPENDS-ON.** NOTIF-P8 (define_notif_rule seam), NOTIF-P9 (humanise — the leak surface), NOTIF-P13
  (read-fanout over real watcher tuples), NOTIF-P19 (Git accretion — the seam proven once already, this is the
  second producer; the inverse-signal must still hold). The M3 Knowledge prompts that ship the KN ReBAC fragment
  incl. the watcher relation (4.9 — KN page-tree inherit-with-overrides) + the KN event taxonomy + project. The
  index places this after Knowledge ships its M3 fragment.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — synthetic watcher → real fragment; honesty);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (the inverse-signal — registration must not get
    harder for the second producer than the first), §3 (prove-it — re-run the leak drill against REAL KN
    confidential subjects).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.5 (the watcher relation frozen
    obligation, C8), §3.1 (the reason set the subsystems register), §3.3 (humanise per-viewer against real KN
    subjects).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.6 (define_notif_rule — KN
    registers mentions/comments/shares/watched), 4.9 (the watcher relation per watchable type; KN page-tree
    inherit-with-overrides), 5.6 (project(ref, viewer) — the humanise projection), 4.3/4.4 (the read-fanout
    watcher resolution over KN's real fragment). Read 00-reconciliation-decisions.md C8.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M3 (the Knowledge registration work + the gate) +
    §4 (the M3 accretion deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D4 (re-run against a REAL KN confidential page, not synthetic) + the KN confidential-leak rows
    KN-D5/KN-D13 (confidential page/row/field 0 leak incl. COUNT — Notif's resolve(Display) path is the leak
    surface they exercise).
- **DELIVERABLE (what to build + exactly where in the repo).** Mostly in Knowledge's crate (the registration),
  with the verification in myelin-notif:
  - Knowledge registers its set (mentions / comments / shares / watched) via the 7.6 seam + declares its watcher
    fragment (4.9, KN page-tree inherit-with-overrides) → KN mentions/comments flow as real inbox items; the
    agent-trace-adjacent reasons land; read-fanout (NOTIF-P13) over REAL KN page watchers goes live.
  - In myelin-notif: verify the seams accept KN's registration WITHOUT any Notif code change (the inverse-signal
    check, again — record explicitly that the SECOND producer needed no more change than the first). Re-run the
    humanise-per-viewer property (NOTIF-D4) against a REAL confidential KN page — confirming the tombstone holds.
  - FLOOR named: Issues / Chat / CI reasons + watchers are M4 (NOTIF-P21/P22/P23); cross-cell is still
    single-home (NOTIF-P24). Name them.
- **CONTRACTS TO IMPLEMENT.** None NEW owned by Notif. Knowledge owns its 4.9 watcher fragment + 7.6
  registration + 5.6 project; Notif consumes them. Verify against the frozen 7.6 / 4.9 / 5.6 shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D4 re-confirmed on a REAL KN subject (CI): notify on a real KN confidential page to a viewer lacking
    access → humanised tombstone, the title never appears. Telemetry green artifact: 0 title/PII leak against a
    real subject. Threshold: 0 — never softened.
  - KN-D5 / KN-D13 (CI/SCHED): confidential page/row/field 0 leak (incl. COUNT) green with Notif's humanise path
    exercised. Threshold: 0 leak incl. COUNT.
  - The inverse-signal record: a written note that KN registered via the unchanged seam with ZERO Notif code
    change, and that the second producer was no harder than the first (the compounding-payoff property,
    EI-01 closing).
- **TESTS (required).** Integration tests that KN's registration produces real KN mentions/comments views and
  real read-fanout. The drill-harness scenario for NOTIF-D4 on a real KN subject. A contract test that the 7.6 /
  4.9 seams accept KN's set unchanged. The CDC pairs for the KN side of 7.6 / 4.9 (provider KN, consumer Notif).
- **DEFINITION OF DONE.** Knowledge registers reasons + the watcher fragment via the unchanged seams (ZERO Notif
  code change recorded; second producer no harder than the first); KN mentions/comments are real views;
  read-fanout runs over real KN watchers; NOTIF-D4 re-confirmed on a real KN confidential subject (0 leak,
  PROVEN); KN-D5/KN-D13 are green with humanise exercised; the KN-side CDC + lints + coverage scanner are green;
  the floors (Issues/Chat/CI NOTIF-P21/P22/P23; cross-cell NOTIF-P24) are named; the work is committed. No
  threshold weakened.
- **COMMIT.** Header: P-<NNN> M3: producer accretion — Knowledge reasons/watchers; NOTIF-D4 on real KN subjects.
  Body lists: Knowledge registered via 7.6 + 4.9 (no Notif code change recorded; second producer no harder);
  NOTIF-D4 re-confirmed on a real KN subject (0 leak); KN-D5/KN-D13 greened; the floors named (M4
  NOTIF-P21/P22/P23; cross-cell NOTIF-P24). Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### NOTIF-P21 — Consumer accretion: Issues registers reasons + passes the real SLA escalation chain (ISS-D6)

- **BAND.** M4.
- **ROADMAP MILESTONE.** N-M4 (planning/06-roadmaps/shared/notifications.md §1 "N-M4 — Consumer accretion:
  Issues SLA/escalation + ..."; this prompt is the Issues half — Issues' reason set + the real escalation chain
  definition driving the NOTIF-P14 machinery + the watcher fragment + ISS-D6; Chat is NOTIF-P22, CI is NOTIF-P23).
- **DEPENDS-ON.** NOTIF-P8 (define_notif_rule + the filtered views), NOTIF-P5 ("My Work" filtered view),
  NOTIF-P14 (escalation chains — Issues passes the real SLA chain to the frozen-shape machinery), NOTIF-P18 (the
  SLA timer — the third use of the wheel). The M4 Issues prompts (reason set + the escalation chain definition +
  watcher fragment). The index places this after Issues ships its M4 registration.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors); ../../external-insights/01-process-and-quality-doctrine.md §1 (the
    inverse-signal — registration must not get harder each time).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §2.4 (Issues passes the frozen
    escalation chain definition; Notif owns policy, the engine owns durability), §1.3 (the C-9 invariant — "My
    Work" is a filter not a store), §3.7 (SLA timers ride the same durable wheel).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.6 (define_notif_rule — Issues
    registers assigned/blocked/needs-approval/overdue/sla/unblocked), 7.5 (Issues passes the escalation chain
    definition to the NOTIF-P14 durable workflow), 4.9 (Issues' watcher fragment), 9.4 (the durable signal for
    the multi-day HITL/escalation wait). Read 00-reconciliation-decisions.md §5 (the escalation chain).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M4 (the Issues registration work + the ISS-D6
    gate) + §4 (the M4 deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row ISS-D6 (SLA breach starts the escalation chain — Notif's chain-start integration with Issues).
- **DELIVERABLE (what to build + exactly where in the repo).** Mostly in Issues' crate (the registration), with
  the integration verified in myelin-notif:
  - Issues registers its reasons (assigned/blocked/needs-approval/overdue/sla/unblocked) via 7.6 + passes its
    REAL escalation chain definition to Notif (7.5, the frozen chain shape from NOTIF-P14) + declares its watcher
    fragment (4.9) → Issues "My Work" becomes a real filtered view (a FILTER, not a store — the C-9 invariant);
    SLA breaches start real escalation chains on the durable wheel (the NOTIF-P14 machinery, now driven by
    Issues' SLA policy; the SLA timer is the NOTIF-P18 wheel's third use).
  - In myelin-notif: verify the registration + the chain-pass land via the unchanged seams (the inverse-signal
    check); the C-9 invariant holds for Issues "My Work".
  - FLOOR named: Chat (NOTIF-P22) + CI (NOTIF-P23) are the other M4 consumers; cross-cell is still single-home
    (NOTIF-P24); surge/erasure hardening is NOTIF-P25/P26/P27. Name them.
- **CONTRACTS TO IMPLEMENT.** None NEW owned by Notif. Issues owns its 7.6 registration + the 7.5 chain
  definition + the 4.9 watcher fragment; Notif consumes/evaluates them against the frozen 7.5/7.6/4.9 shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - ISS-D6 (CI): an SLA breach starts the escalation chain (Notif's chain-start integration with Issues — the
    NOTIF-P14 durable workflow driven by Issues' real SLA policy). Threshold: the chain starts and walks per the
    frozen shape; exactly-once page (inherits NOTIF-D7's property under Issues' real chain).
  - The C-9 invariant for "My Work" (CI): Issues "My Work" is a filter returning a strict subset of the
    unfiltered inbox, not a store. Threshold: rows ⊆ list_inbox(filter=∅).
  - The inverse-signal record: a written note that Issues registered + passed its chain via the unchanged seams
    with ZERO Notif code change.
- **TESTS (required).** Integration tests that Issues' SLA breach starts a real escalation chain on the durable
  wheel; the C-9 invariant holds for "My Work". The drill-harness scenario for ISS-D6. The CDC pairs for the
  Issues side of 7.6 / 7.5 / 4.9 (provider Issues, consumer Notif).
- **DEFINITION OF DONE.** Issues registers via the unchanged seams and passes its real SLA chain; SLA breaches
  drive real escalation chains on the durable wheel; "My Work" is a real filtered view (C-9 invariant proven);
  ISS-D6 emits its dated green artifact (chain starts + exactly-once, PROVEN); the Issues-side CDC + lints +
  coverage scanner are green; the floors (Chat NOTIF-P22; CI NOTIF-P23; cross-cell NOTIF-P24; hardening
  NOTIF-P25/P26/P27) are named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M4: consumer accretion — Issues SLA chains + "My Work" filter. Body lists: Issues
  registered via 7.6/7.5/4.9 (no Notif code change); ISS-D6 greened (chain starts, exactly-once); the C-9
  "My Work" invariant proven; the floors named (Chat NOTIF-P22; CI NOTIF-P23; cross-cell NOTIF-P24). Branch
  first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P22 — Consumer accretion: Chat registers activity/mentions + the explicit-first agent dispatch boundary + HITL cards (CHAT-D5, CHAT-D17)

- **BAND.** M4.
- **ROADMAP MILESTONE.** N-M4 (planning/06-roadmaps/shared/notifications.md §1 "N-M4 — ... + Chat
  activity/mentions + explicit-first agents"; this prompt is the Chat half — Chat's reason set + the
  explicit-first dispatch boundary + the HITL approval cards + CHAT-D5/CHAT-D17).
- **DEPENDS-ON.** NOTIF-P8 (define_notif_rule + the filtered views), NOTIF-P9 (humanise — the HITL card + the
  leak surface), NOTIF-P5 ("Activity/Mentions" filtered view), NOTIF-P7 (ranking — approval_requested at high
  priority). The M4 Chat prompts (reason set + explicit-first dispatch boundary + watcher fragment). The M2
  myelin-flow durable signal for the multi-day HITL wait (9.4). The index places this after Chat ships its M4
  registration.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — agents have inboxes; explicit-first dispatch — a casual @agent notifies,
    does not spawn a costed run); ../../external-insights/01-process-and-quality-doctrine.md §1 (the
    inverse-signal).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §1.4 (agents have inboxes; an HITL
    approval card is a Notif item reason=approval_requested at high priority), §1.3 (the C-9 invariant — Chat
    "Activity/Mentions" is a filter not a store).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.6 (define_notif_rule — Chat
    registers mentioned/replied/thread_watched/approval_requested), 8.6 (explicit-first dispatch — a mention
    notifies, does not auto-spawn a costed run), 7.3 (humanise — the HITL card renders through it), 9.4 (the
    durable signal for multi-day HITL), 4.9 (Chat's watcher fragment). Read 00-reconciliation-decisions.md the
    explicit-first dispatch pinning.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M4 (the Chat registration work + the CHAT-D5 /
    CHAT-D17 gate touch-points) + §4 (the M4 deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows CHAT-D5 (notify/unfurl a confidential artifact to a viewer lacking access → tombstone, title never
    present — Notif's humanise leak surface at the Chat seam), CHAT-D17 (casual @agent → notifies the agent's
    inbox, does NOT auto-spawn a costed run).
- **DELIVERABLE (what to build + exactly where in the repo).** Mostly in Chat's crate (the registration), with
  the integration verified in myelin-notif:
  - Chat registers its reasons (mentioned/replied/thread_watched/approval_requested) via 7.6 + declares its
    watcher fragment (4.9) → Chat "Activity/Mentions" becomes a real filtered view (a FILTER, not a store — the
    C-9 invariant).
  - The explicit-first agent dispatch boundary (8.6): a casual @agent mention posts a Notif item to the agent's
    inbox (reason=mentioned) but does NOT spawn a costed run (CHAT-D17 — Notif is the notify side of that
    boundary).
  - HITL approval cards: an agent HITL approval surfaced to a human is a Notif item with
    reason=approval_requested at high priority (refined §1.4); the card humanises via the ONE templating surface
    (NOTIF-P9 — action + risk + cost). Agents have inboxes too — the same model, no parallel system. The
    multi-day HITL wait uses the durable signal (9.4).
  - In myelin-notif: verify the registration lands via the unchanged seam; the C-9 invariant holds for Chat
    "Activity".
  - FLOOR named: CI is NOTIF-P23; cross-cell is NOTIF-P24; surge/erasure hardening is NOTIF-P25/P26/P27. Name
    them.
- **CONTRACTS TO IMPLEMENT.** None NEW owned by Notif. Chat owns its 7.6 registration, the 8.6 explicit-first
  boundary, the 4.9 watcher fragment; Notif consumes/evaluates them against the frozen 7.6/8.6/7.3/9.4/4.9
  shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D5 (CI): notify/unfurl a confidential artifact to a viewer lacking access → tombstone, the title never
    present (Notif's humanise leak surface at the Chat seam — re-confirms NOTIF-D4 at the Chat seam). Threshold:
    0 title/PII leak.
  - CHAT-D17 (CI): a casual @agent mention → notifies the agent's inbox (reason=mentioned), does NOT auto-spawn
    a costed run. Threshold: 0 auto-spawn from a casual mention.
  - The C-9 invariant for "Activity" (CI): Chat "Activity/Mentions" is a filter (a strict subset), not a store.
    Threshold: rows ⊆ list_inbox(filter=∅).
- **TESTS (required).** Integration tests that Chat's casual @agent notifies without spawning a run; the HITL
  approval card renders through humanise (action + risk + cost); the C-9 invariant holds for Chat "Activity".
  The drill-harness scenarios for CHAT-D5, CHAT-D17. The CDC pairs for the Chat side of 7.6 / 8.6 (provider Chat,
  consumer Notif).
- **DEFINITION OF DONE.** Chat registers via the unchanged seam; Chat's casual @agent notifies without spawning
  a run (explicit-first); the HITL approval card resolves through the ONE templating surface; "Activity" is a
  real filtered view (C-9 invariant proven); CHAT-D5 + CHAT-D17 emit their dated green artifacts (PROVEN); the
  Chat-side CDC + lints + coverage scanner are green; the floors (CI NOTIF-P23; cross-cell NOTIF-P24; hardening
  NOTIF-P25/P26/P27) are named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M4: consumer accretion — Chat activity/mentions + explicit-first + HITL cards.
  Body lists: Chat registered via 7.6/8.6/4.9 (no Notif code change); CHAT-D5 + CHAT-D17 greened; the C-9 Chat
  "Activity" invariant proven; the floors named (CI NOTIF-P23; cross-cell NOTIF-P24). Branch first if on
  default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P23 — Consumer accretion: CI registers status-summary reasons; the CheckStatus.summary HumanisedRef resolves through humanise (X-1)

- **BAND.** M4.
- **ROADMAP MILESTONE.** N-M4 (planning/06-roadmaps/shared/notifications.md §1 "N-M4 — ... + CI registers"; this
  prompt is the CI half — CI's status-summary reasons + the CheckStatus.summary HumanisedRef resolving through
  the ONE templating surface, never a raw string).
- **DEPENDS-ON.** NOTIF-P8 (define_notif_rule), NOTIF-P9 (humanise — the HumanisedRef resolves through it). The
  M4 CI prompts (the CheckStatus.summary HumanisedRef, X-1 / 5.9). The index places this after CI ships its M4
  registration.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (one templating surface — CI registers templates, never raw strings);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (the inverse-signal — CI's registration must
    not get harder than the prior consumers').
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.3 (CI's CheckStatus.summary is a
    HumanisedRef = a (template_key, args) pair, resolves through humanise, never a raw string — X-1).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.6 (define_notif_rule — CI
    registers its status-summary reasons), 7.3 (humanise — the CI HumanisedRef registers here), 5.9 (the Git↔CI
    CheckStatus seam — CheckStatus.summary is a HumanisedRef). Read 00-reconciliation-decisions.md X-1 (the
    CheckStatus seam + the HumanisedRef).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M4 (the CI registration work) + §4 (the M4 deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §3.3 (the HumanisedRef-resolves-through-humanise property; the X-1 seam is a cross-band split contract, CI
    producer half M4).
- **DELIVERABLE (what to build + exactly where in the repo).** Mostly in CI's crate (the registration), with
  the integration verified in myelin-notif:
  - CI registers its status-summary reasons via 7.6; the CheckStatus.summary (X-1 / 5.9) is a HumanisedRef =
    a (template_key, args) pair that resolves through humanise (NOTIF-P9) — CI registers its templates on the ONE
    surface, NEVER a raw string.
  - In myelin-notif: verify the CI registration lands via the unchanged seam (the inverse-signal check); the
    CheckStatus.summary resolves through humanise (a raw-string summary is rejected at the seam).
  - FLOOR named: this completes the M4 consumer accretion (Issues NOTIF-P21, Chat NOTIF-P22, CI here); cross-cell
    is NOTIF-P24; surge/erasure hardening is NOTIF-P25/P26/P27. Name them.
- **CONTRACTS TO IMPLEMENT.** None NEW owned by Notif. CI owns its 7.6 registration + the 5.9 CheckStatus.summary
  HumanisedRef; Notif consumes/resolves them against the frozen 7.6/7.3/5.9 shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The HumanisedRef-resolves-through-humanise check (CI): CI's CheckStatus.summary is a (template_key, args)
    pair that resolves through humanise (NOTIF-P9), never a raw string; a raw-string summary is rejected.
    Threshold: 0 raw-string summary accepted; 100% resolve through humanise.
  - The inverse-signal record: a written note that CI registered via the unchanged seam with ZERO Notif code
    change (the third+ consumer no harder than the first).
- **TESTS (required).** Integration tests that CI's HumanisedRef summary resolves through humanise (never a raw
  string; a raw string is rejected). A contract test that the 7.6 / 5.9 seams accept CI's set unchanged. The CDC
  pairs for the CI side of 7.6 / 5.9 (provider CI, consumer Notif — the X-1 cross-band seam's M4 producer half).
- **DEFINITION OF DONE.** CI registers via the unchanged seam; CI's CheckStatus.summary HumanisedRef resolves
  through the ONE templating surface (never a raw string); the HumanisedRef-resolution check + the CI-side CDC +
  lints + coverage scanner are green; the floors (cross-cell NOTIF-P24; hardening NOTIF-P25/P26/P27) are named;
  the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M4: consumer accretion — CI HumanisedRef summaries through humanise (X-1). Body
  lists: CI registered via 7.6/5.9 (no Notif code change); the CheckStatus.summary HumanisedRef resolves through
  humanise (never raw); the floors named (cross-cell NOTIF-P24; hardening NOTIF-P25/P26/P27). Branch first if on
  default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P24 — Cross-cell inbox aggregation (the multi-cell floor's follow-on; always-cell-local resolution)

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.1 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.1 — Cross-cell inbox
  aggregation").
- **DEPENDS-ON.** NOTIF-P5 (list_inbox), NOTIF-P9 (humanise — cell-local resolution). The M5 multi-cell control
  plane prompts that ship the CrossCellPointer bridge going live (12.6) and the multi-cell DSR fan-out iterating
  member_cells (10.4). The index places this after the M5 multi-cell tenancy prompts. The single-home-cell path
  has been complete since NOTIF-P2 (the §4 contracts were written cell-agnostic so this extends without a
  rewrite).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign by construction — no PII crosses cells; residency-preserving);
    ../../external-insights/04-hard-problems.md §1 (residency: humanisation always resolves locally in the cell
    that holds the artifact); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the
    cross-cell legs of GA-D8/CP-D7/CP-D8).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §5.4 (cross-cell inbox aggregation
    — FLOOR, the bridge frame now frozen: the inbox is materialised per home-cell; a multi-cell recipient's
    unified view aggregates across their cells via the frozen CrossCellPointer{subject(opaque), type,
    correlation_id, home_cell}, the control plane carries ONLY the pointer never name/email/body; resolution is
    ALWAYS cell-local — to render a pointer to an artifact homed in cell B, cell A's gateway asks cell B to
    resolve(ref, viewer, Display) IN B, permission-checked in B against B's tuples, returning only the
    already-rendered already-permission-filtered projection or a tombstone, never raw rows, never PII that should
    stay in B; the DSR orchestrator iterates member_cells over the same bridge).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 12.6 (the CrossCellPointer
    PII-free pointer bridge, resolution always cell-local), 5.2 (resolve(ref, viewer, Display) — cell-local
    resolution pinned, OQ-I), 10.4 (dsr_submit iterates member_cells over the bridge). Read
    00-reconciliation-decisions.md OQ-I (the cross-cell bridge + the always-cell-local resolution rule).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M5.1 (the cross-cell work + the
    GA-D8/CP-D7/CP-D8 gate) + §2 (the single-home-cell → cross-cell floor row) + §4 (the M5 multi-cell deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    the cross-cell legs of GA-D8 / CP-D7 / CP-D8 (a cross-cell inbox view resolves cell-locally with 0 PII
    crossing cells; cell→cell migration loses 0 inbox items).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate (the
  multi-cell aggregation path; the single-cell path stays unchanged):
  - The cross-cell aggregation: a multi-cell recipient's unified inbox aggregates across every cell they belong
    to via the frozen CrossCellPointer{subject(opaque), type, correlation_id, home_cell} (12.6) — the control
    plane carries ONLY the pointer, NEVER name/email/body. The inbox stays materialised per home-cell; the
    unified view stitches the per-cell slices via the bridge.
  - Cell-local resolution (the frozen OQ-I rule, 5.2): to render a pointer to an artifact homed in cell B, cell
    A's gateway (holding the viewer's identity) asks CELL B to resolve(ref, viewer, Display) IN B —
    permission-checked in B against B's tuples — returning ONLY the already-rendered, already-permission-filtered
    projection (or a tombstone), never raw rows, never PII that should stay in B. Humanisation ALWAYS resolves
    locally in the cell that holds the artifact (residency-preserving; no PII crosses cells, ADR-11).
  - The DSR orchestrator iterates member_cells (10.4) over the same bridge for the cross-cell erasure leg.
  - FLOOR named: this is the named multi-cell floor follow-on; the single-home-cell path remains the default and
    is complete. State that the cell-agnostic §4 contracts made this an extension, not a rewrite.
- **CONTRACTS TO IMPLEMENT.** None NEW owned. Consumed: 12.6 the CrossCellPointer bridge, 5.2 cell-local
  resolve(Display), 10.4 member_cells iteration. Implement to the frozen always-cell-local rule — no raw rows or
  PII ever cross a cell boundary.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - The cross-cell legs of GA-D8 / CP-D7 / CP-D8 (SCHED) for the inbox-aggregation path: a cross-cell inbox view
    resolves cell-locally with 0 PII crossing cells; a cell→cell migration loses 0 inbox items. Telemetry green
    artifact: 0 PII crossing cells; 0 inbox items lost on migration. Threshold: 0 PII crossing, 0 loss — never
    softened.
- **TESTS (required).** Unit tests for the cross-cell aggregation stitch (only pointers cross; resolution is
  cell-local). A cross-cell integration test: a viewer in cell A with an item homed in cell B → assert cell B
  renders the projection (or tombstone) and only the projection crosses, never raw rows/PII. A cell→cell
  migration test asserting 0 inbox items lost. The drill-harness scenarios for the GA-D8/CP-D7/CP-D8 inbox legs.
  The CDC for the Notif consumption of 12.6. The cross-cell module is mandatory-core: state the cargo-mutants
  mutation-score floor and meet it.
- **DEFINITION OF DONE.** A multi-cell recipient's inbox aggregates across cells via the PII-free pointer bridge
  with always-cell-local resolution (0 PII crosses cells); cell→cell migration loses 0 items; the
  GA-D8/CP-D7/CP-D8 inbox legs emit their dated green artifacts (PROVEN); the 12.6 CDC + lints + coverage
  scanner are green; the floor framing (single-home-cell remains the default; this is the follow-on) is
  recorded; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: cross-cell inbox aggregation (always-cell-local resolution). Body lists:
  contract 12.6 consumed (the CrossCellPointer bridge, always-cell-local resolution); the GA-D8/CP-D7/CP-D8
  inbox legs greened (0 PII crossing, 0 items lost on migration); the cross-cell mutation-score measured. Branch
  first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P25 — The 30×-agent-surge shed budget (the F6 surge family; human-last lane) + NOTIF-D5

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.2 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.2 — The 30×-agent-surge
  shed budget + ..."; this prompt is the surge slice and ships NOTIF-D5; the EU delivery provider is NOTIF-P26,
  the erasure residual NOTIF-P27).
- **DEPENDS-ON.** NOTIF-P3 (the router/consumer), NOTIF-P11 (storm-control), NOTIF-P13 (the per-tenant in-flight
  caps on the read-fanout path). The M1 reserve/settle wallet (11.7) gating agent runs. The M2 agent runtime
  honouring 429 + Retry-After (ADR-16.3). The index places this after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable; name-your-floors — the shed budget is a named floor tuned by the drill,
    not a claimed-final number); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the
    30× surge drill; the budget is asserted, observability is part of the pass — shed-counts + delivery-success
    signals); ../../external-insights/02-platform-substrate.md §5 (an unbounded lane is the cascade).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §5.2 (the fan-out scale axis + the
    agent-mention-storm shed budget: bounded consumer prefetch, bounded handler pool, per-tenant in-flight caps —
    one tenant's storm can't starve another's, bounded delivery-adapter concurrency a bulkhead per provider,
    per-recipient rate damping; the protected-human-lane shed order speculative → batch/CI → agent → human-last,
    ADR-16, concretised: a per-tenant agent-run in-flight cap reserve/settle refuses over-cap, humans never queue
    behind agent runs a separate lane, the agent-generated notification lane sheds first with 429 + Retry-After
    the agent runtime honours it ADR-16.3, a human's interactive inbox read is last-to-shed; these are named
    floors tuned by the drill T-5, not claimed-final numbers).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.11 (the protected-human-lane
    shed order + per-surface shed budgets named v1 floors — the agent-mention-storm row, OQ-K), 11.7
    (reserve/settle cost gate — refuses over-cap at dispatch), 1.8 (shed-counts + delivery-success telemetry).
    Read 00-reconciliation-decisions.md OQ-K (the shed budgets).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M5.2 (the surge budget work + the NOTIF-D5 gate)
    + §4 (the reserve/settle + 429-honouring deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D5 (30× agent-generated notification surge → human inbox-read lane holds, agent sheds,
    delivery-adapter bulkhead bounds provider load; shed-counts; delivery-success — part of the master M5 F6
    surge family); §3.1 (the 30× load generator with mixed principal types).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - The agent-mention-storm shed budget (refined §5.2): concretise the protected-human-lane shed order
    (speculative → batch/CI → agent → human-last, ADR-16) for the agent-mention-storm profile — a per-tenant
    agent-run in-flight cap (reserve/settle, 11.7, refuses over-cap at dispatch); humans NEVER queue behind agent
    runs (a SEPARATE lane); the agent-generated notification lane sheds FIRST with 429 + Retry-After (the agent
    runtime honours it, ADR-16.3); a human's interactive inbox read is LAST-to-shed. Plus: bounded consumer
    prefetch, bounded handler pool, per-tenant in-flight caps (one tenant's storm can't starve another's), a
    delivery-adapter bulkhead per provider, per-recipient rate damping.
  - FLOOR named: these are named floors tuned BY the drill (T-5), not claimed-final numbers — the concrete cap is
    the budget call NOTIF-D5 asserts against; record the chosen v1 numbers in the thresholds file. Name the floor.
- **CONTRACTS TO IMPLEMENT.** None NEW owned. Consumed: 1.11 the shed order + per-surface budget, 11.7
  reserve/settle. Implement to the frozen shed order (human-last) — the human lane is always reserved.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D5 (SCHED): a 30× agent-generated notification surge on one tenant → the human inbox-read lane holds
    within budget; the agent lane sheds (429 + Retry-After); cross-tenant unaffected; the delivery-adapter
    bulkhead bounds provider load. Telemetry green artifact: shed-counts + delivery-success measured and asserted
    against the §5.2 named shed budget (in the thresholds file). Threshold: human inbox-read latency within the
    named budget; cross-tenant impact 0. This is part of the master M5 F6 surge family — never weaken the budget
    to pass; a missed budget is a dated scorecard row.
- **TESTS (required).** Unit tests for the lane separation (a human read is served while the agent lane is
  shedding) and the per-tenant in-flight cap (over-cap → reserve/settle refuses). The drill-harness scenario for
  NOTIF-D5 (the 30× surge generator). A cross-tenant isolation test: tenant A's surge does not affect tenant B's
  human-read latency. State the cargo-mutants mutation-score floor for the shed-lane module and meet it.
- **DEFINITION OF DONE.** The agent-mention-storm shed budget is implemented (human-last, separate lane,
  per-tenant cap, bulkhead, 429+Retry-After); the v1 budget numbers are in the thresholds file; NOTIF-D5 emits
  its dated green artifact (human lane in budget, agent sheds, cross-tenant 0, PROVEN — or a dated scorecard row
  if the budget is not yet met); lints + coverage scanner are green; the floor (the budget numbers tuned by the
  drill) is named; the work is committed. No budget weakened to manufacture a green.
- **COMMIT.** Header: P-<NNN> M5: 30×-agent-surge shed budget (human-last lane) + NOTIF-D5. Body lists: the
  agent-mention-storm shed budget (human-last lane, per-tenant cap, bulkhead); NOTIF-D5 greened (human lane in
  budget, agent sheds, cross-tenant 0, measured shed-counts); the budget numbers recorded in the thresholds
  file; the shed-lane mutation-score measured. Branch first if on default; do not push unless asked. End with
  the Co-Authored-By trailer.

---

### NOTIF-P26 — The EU-sovereign delivery provider follow-on (swap the real provider into the DeliveryAdapter trait; [OPEN — LEGAL])

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.2 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.2 — ... + the
  EU-sovereign delivery follow-on + ..."; this prompt is the EU-provider slice — swap the concrete production EU
  provider into the NOTIF-P16 trait; the erasure residual is NOTIF-P27).
- **DEPENDS-ON.** NOTIF-P16 (the DeliveryAdapter trait + mock — the real provider swaps in here), NOTIF-P9
  (humanise — the RedactedMessage minimisation). The chosen EU provider + DPA (legal, parallel). The index
  places this after NOTIF-P16 and the legal selection.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign — the concrete EU provider + DPA; name-your-floors — the provider is the
    deferred selection, the engineering posture ships now);
    ../../external-insights/01-process-and-quality-doctrine.md §8 (the human sign-off is the bottleneck — a
    decision-shaped, irreversible-scope surface pauses for counsel/DPO sign-off), §3 (prove-it — the real
    provider's idempotency holds under the same NOTIF-D9 property).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.6 (the EU-sovereign delivery
    fabric FLOOR — the concrete production EU provider with its DPA/sub-processor posture is the deferred
    selection; the trait + EU-preferring posture + RedactedMessage minimisation + crypto-shred +
    provider-side-erasure-request hook ship), §10 (the [OPEN — LEGAL] provider selection).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 7.8 (DeliveryAdapter — the real
    EU provider swaps in, region-aware, EU-preferring, at-least-once + idempotent). Read
    00-reconciliation-decisions.md the EU-sovereign delivery floor (§10).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M5.2 (the EU provider follow-on work) + §2 (the
    mock → EU-provider floor row) + §4 (the EU-provider dep).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D9 (the real provider must hold the same exactly-one-per-(item, channel) idempotency the mock did)
    + §3.3 (delivery_success telemetry under the real provider).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - The EU-sovereign delivery provider follow-on (refined §3.6/§10): swap the concrete production EU email/push
    provider into the DeliveryAdapter trait (NOTIF-P16) — region-aware, EU-preferring, at-least-once + idempotent
    on UNIQUE(idem_key), RedactedMessage off-cell (summary + deep link, never the full body). The real provider
    honours the same exactly-one-per-(item, channel) property the mock proved (NOTIF-D9 re-run).
  - The provider-side-erasure-request hook: build the hook in the adapter that issues a provider-side erasure
    request for an already-sent off-cell payload (the named sub-processor obligation — consumed by NOTIF-P27).
  - [OPEN — LEGAL]: the engineering posture (trait + EU-preferring + RedactedMessage minimisation + crypto-shred
    + the provider-side-erasure-request hook) ships HERE; counsel/DPO ratifies the chosen provider + the DPA/
    sub-processor posture. We are not counsel — flag the provider selection + the residual statement for
    counsel/DPO sign-off (EI-01 §8 — a decision-shaped, irreversible-scope surface pauses for human sign-off).
  - FLOOR named: the erasure residual that USES the provider-side-erasure-request hook is NOTIF-P27; the
    counsel/DPO ratification of the provider + DPA is the [OPEN — LEGAL] gate. Name both.
- **CONTRACTS TO IMPLEMENT.** None NEW owned. Consumed: 7.8 DeliveryAdapter (the real EU provider
  implementation). Implement to the frozen trait — the real provider satisfies the same at-least-once +
  idempotent property; no trait shape change.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D9 re-run under the real provider (SCHED): crash between provider-ack and ledger-write, retry →
    UNIQUE(idem_key) collapses to EXACTLY-ONE effective delivery per (item, channel) — under the real EU
    provider. Telemetry green artifact: 1 effective delivery; delivery_success measured. Threshold: exactly 1.
  - The EU-preferring + off-cell-redacted check (CI): the real provider is region-aware/EU-preferring; off-cell
    payloads carry only a RedactedMessage (summary + link), delivery.redacted=true. Threshold: 0 off-cell
    full-body; EU-preferring routing asserted.
  - The [OPEN — LEGAL] flag recorded: the provider + DPA selection is flagged for counsel/DPO sign-off (a dated
    scorecard row, not a silently-claimed-done). Threshold: the flag is present and dated.
- **TESTS (required).** Unit tests for the real adapter's idem_key collapse (a retry after provider-ack is a
  no-op) and the RedactedMessage minimisation under the real provider. The drill-harness scenario for NOTIF-D9
  re-run. The CDC for the real DeliveryAdapter against 7.8. The real-provider adapter is mandatory-core: state
  the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The real EU-sovereign provider is swapped into the DeliveryAdapter trait (region-aware,
  EU-preferring, exactly-one per (item, channel)); the provider-side-erasure-request hook is built; the
  [OPEN — LEGAL] provider+DPA flag is recorded for counsel/DPO sign-off; NOTIF-D9 re-run emits its dated green
  artifact (1 effective delivery, PROVEN); the EU-preferring/off-cell-redacted check + the 7.8 CDC + lints +
  coverage scanner are green; the floors (erasure residual NOTIF-P27; counsel ratification) are named; the work
  is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: EU-sovereign delivery provider follow-on (real DeliveryAdapter). Body lists:
  the real EU provider swapped into 7.8 + the provider-side-erasure-request hook; NOTIF-D9 re-run greened (1
  effective delivery); the real-provider mutation-score measured; the [OPEN — LEGAL] provider+DPA flagged for
  counsel/DPO; the erasure-residual floor named (NOTIF-P27). Branch first if on default; do not push unless
  asked. End with the Co-Authored-By trailer.

---

### NOTIF-P27 — The erasure residual instanced (the X-7 posture for Notif) + NOTIF-D6

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.2 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.2 — ... + the erasure
  residual"; this prompt is the erasure-residual slice and ships NOTIF-D6).
- **DEPENDS-ON.** NOTIF-P4 (the holder + references-not-payloads), NOTIF-P9 (humanise — an erased actor →
  [erased user]), NOTIF-P16 (the DeliveryAdapter — the off-cell-payload erasure-request hook), NOTIF-P26 (the EU
  provider — the provider-side erasure mechanism). The M1 per-subject DEK (11.4) for crypto-shred. The M1 GDPR
  erasure ledger (10.8) + the one platform erasure posture (10.9). The index places this after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction — data subject erasure; honesty about uncertainty —
    [OPEN — LEGAL] residual flagged, not claimed-resolved); ../../external-insights/04-hard-problems.md §1
    (erasure vs immutability — the residual is the one inline-PII case in an already-delivered off-cell payload);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — erase a user, assert 0 recoverable
    PII incl. backups).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.9 (the PersonalDataHolder — the
    residual stated BY REFERENCE to the platform posture X-7 / contract 10.9: the structural floor is
    per-subject DEK crypto-shred of any inline-PII delivery columns + the restrict suppression (stop new
    routing/delivery for a restricted subject) + a provider-side erasure request for the off-cell payload, the
    named sub-processor obligation; Notif does NOT restate the posture, the residual third-party free-text case
    is governed where the content lives, the authoring subsystem; restrict also suppresses
    indexing/agent-use/analytics/notification for a restricted subject, 10.1).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.7 (PersonalDataHolder erase/
    restrict — references-not-payloads tombstones for free), 10.9 (the ONE free-text/immutable-content erasure
    posture — instantiated per subsystem BY REFERENCE, the [OPEN — LEGAL] residual), 11.4 (per-subject DEK
    crypto-shred incl. the inline-PII delivery columns), 10.8 (the erasure ledger), 10.1 (restrict suppresses
    indexing/agent-use/analytics/notif). Read 00-reconciliation-decisions.md X-7 (the one erasure posture).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M5.2 (the erasure residual work + the NOTIF-D6
    gate) + §2 (the by-reference erasure residual floor row) + §4 (the per-subject DEK dep).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D6 (erase a user → every inbox item humanises to [erased user]; 0 recoverable PII; off-cell-sent
    payload crypto-shredded/erasure-requested; erase-receipt; 0 recoverable — the X-7 posture instanced for
    Notif).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - The erasure residual instanced (X-7 / 10.9) — the structural floor (built since NOTIF-P4, completed here):
    (1) references-not-payloads already tombstones an erased actor's appearance in every inbox item for free
    (every item humanises to [erased user] via NOTIF-P9); (2) per-subject-DEK crypto-shred (11.4) of any
    inline-PII delivery columns (the one place Notif emits free text outside the cell is an off-cell redacted
    summary); (3) restrict suppression — stop NEW routing/delivery for a restricted subject (and suppress
    indexing/agent-use/analytics/notification, 10.1); (4) a provider-side erasure request for the already-sent
    off-cell payload (the named sub-processor obligation — the hook built into the DeliveryAdapter in
    NOTIF-P16/P26). Notif does NOT restate the platform posture; the residual third-party free-text case is
    governed where the content lives (the authoring subsystem), referenced not duplicated.
  - The erase path contributes its receipt to the erasure ledger (10.8) so the DSAR fan-out (NOTIF-P30) can prove
    Notif's holder coverage.
  - FLOOR named: the provider-side erasure mechanism for an already-sent off-cell payload depends on the chosen
    EU provider's capability (NOTIF-P26) and is [OPEN — LEGAL] — counsel/DPO ratifies the one residual
    lawful-basis statement (10.9). The structural floor ships regardless; the residual is one ratified statement,
    not a Notif-restated posture. Flag it.
- **CONTRACTS TO IMPLEMENT.** 7.7 PersonalDataHolder erase/restrict (owned, completed — the residual instanced).
  Consumed: 10.9 the one erasure posture (by reference), 11.4 per-subject DEK crypto-shred, 10.8 the erasure
  ledger, 10.1 restrict. Implement to the frozen posture — Notif adds NO new [OPEN — LEGAL] residual beyond the
  one platform statement.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D6 (SCHED): erase a user → every inbox item humanises to [erased user]; 0 recoverable PII (incl. in
    backups); the off-cell-sent payload crypto-shredded / erasure-requested. Telemetry green artifact:
    erase-receipt sealed; 0 recoverable PII. Threshold: 0 recoverable PII — never softened. This is the X-7
    posture instanced for Notif.
- **TESTS (required).** Unit tests for the structural erase (a refs-stored item → [erased user] with no PII
  mutation; an inline-PII delivery column crypto-shredded; restrict stops new routing). A chained test (EI-01
  §4): deliver an off-cell redacted item → erase the subject → assert the inline-PII column is unrecoverable
  (DEK destroyed) AND a provider-side erasure-request was issued AND the receipt is in the erasure ledger. The
  drill-harness scenario for NOTIF-D6. The CDC for the Notif side of 7.7 erase/restrict + 10.9. The erase module
  is mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The erasure residual is instanced (references-not-payloads tombstone + per-subject-DEK
  crypto-shred + restrict suppression + the provider-side erasure-request); the erase receipt is in the ledger;
  NOTIF-D6 emits its dated green artifact (0 recoverable PII, PROVEN); the 7.7 erase/restrict + 10.9 CDC + lints
  + coverage scanner are green; the floor ([OPEN — LEGAL] residual statement awaits counsel ratification; the
  structural floor ships regardless) is named; the work is committed. No threshold weakened; Notif restates no
  platform posture.
- **COMMIT.** Header: P-<NNN> M5: erasure residual instanced (the X-7 posture for Notif) + NOTIF-D6. Body lists:
  contract 7.7 erase/restrict completed (the residual instanced); NOTIF-D6 greened (0 recoverable PII,
  erase-receipt sealed); the erase mutation-score measured; the [OPEN — LEGAL] residual statement flagged for
  counsel/DPO. Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P28 — The E2E wedge: Notif's E2E-1 leg (the PR context pane — per-viewer humanise + live firehose updates)

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.3 — The whole-system E2E
  wedge: Notif's legs"; this prompt is the E2E-1 PR-context-pane leg; E2E-2 is NOTIF-P29, E2E-4 + STOR-D2 is
  NOTIF-P30).
- **DEPENDS-ON.** NOTIF-P9 (humanise — the pane's per-viewer resolution), NOTIF-P15 (the firehose — the checks
  panel live-updates). All five subsystems live (the M4 producer/consumer prompts NOTIF-P19..P23). The M5
  whole-system E2E harness prompts (the chained-mutation scenarios against a full cell with mock agents). The
  index places this in Notif's M5 set after the hardening prompts and the E2E harness.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (prove the differentiator — the whole-system pane);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the whole-system chained-mutation
    drill), §4 (chain mutations end-to-end, not single handlers).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.3 (humanise per-viewer — the
    pane leg), §7 (the firehose — the checks panel live-updates via the resume-cursor transport).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.3 (humanise — the pane's
    notification/status strings per-viewer), 3.5 (the firehose — the live checks-panel updates). Read the
    testing-strategy E2E section.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M5.3 (the E2E-1 leg) + §4 (the M5 deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §2 (the chained-mutation E2E scenarios) + the E2E-1 row (PR context pane: humanise resolves the pane's
    notification/status strings per-viewer, 0 leak to the unauthorized viewer; the checks panel live-updates via
    the firehose, the shared per-ref cache busts).
- **DELIVERABLE (what to build + exactly where in the repo).** In the whole-system E2E harness (the M5 wedge),
  Notif's E2E-1 leg wired against the full myelin-notif surface:
  - E2E-1 PR context pane: humanise (NOTIF-P9) resolves the pane's notification/status strings per-viewer with 0
    leak to the unauthorized viewer; the checks panel live-updates via the firehose (NOTIF-P15 — the shared
    per-ref cache busts on a ref change).
  - FLOOR named: the E2E-2 HITL flagship leg is NOTIF-P29; the E2E-4 DSAR leg + STOR-D2 is NOTIF-P30. Name them.
- **CONTRACTS TO IMPLEMENT.** None NEW owned (the E2E wedge exercises the full surface). Consumed/exercised: 7.3
  humanise, 3.5 the firehose. Implement the E2E-1 leg to assert the named green artifacts; do not re-implement
  Notif logic.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-1 (SCHED): the pane-resolution trace + zero-leak = 0 + the per-viewer diff for Notif's notification/
    status strings; the checks panel live-updates via the firehose (the per-ref cache busts). Threshold: 0 leak
    to the unauthorized viewer; the live update arrives over the resume-cursor path.
- **TESTS (required).** The E2E harness scenario for E2E-1 (Notif's leg), emitting its named green artifact. This
  chains mutations end-to-end across the full cell with mock agents (EI-01 §4) — not a single-handler test (a ref
  change → cache bust → live pane update → per-viewer re-resolve). No new mandatory-core module (the wedge
  exercises existing modules); confirm the NOTIF-P9/P15 mutation floors still hold under the E2E exercise.
- **DEFINITION OF DONE.** Notif's E2E-1 leg emits its dated green artifact (0 leak to the unauthorized viewer;
  the checks panel live-updates via the firehose, PROVEN); lints + coverage scanner are green; any
  untested-but-named surface is honestly recorded; the E2E-2/E2E-4 floors (NOTIF-P29/P30) are named; the work is
  committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Notif E2E-1 leg (PR context pane — per-viewer humanise + live firehose). Body
  lists: the E2E-1 leg greened (0 leak; live firehose pane update); the E2E-2/E2E-4 floors named (NOTIF-P29/P30).
  Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P29 — The E2E wedge: Notif's E2E-2 leg (the HITL flagship — approval card + explicit-first + exactly-once across a kill)

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.3 — ... Notif's legs";
  this prompt is the E2E-2 HITL-flagship leg — the CI-fail → triage agent → issue → chat → fix-PR flagship).
- **DEPENDS-ON.** NOTIF-P5 + NOTIF-P7 (the approval card is a ranked Notif item reason=approval_requested),
  NOTIF-P9 (humanise — the card's action+risk+cost), NOTIF-P14 (escalation/notify exactly-once across a kill),
  NOTIF-P22 (the explicit-first boundary — the casual mention does not spawn a run). The M2 myelin-flow durable
  signal for the HITL withhold→approve (9.4). All five subsystems live + the E2E harness. The index places this
  after NOTIF-P28.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native from the ground up — the E2E-2 flagship; prove the differentiator);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the whole-system chained-mutation
    drill), §4 (chain mutations end-to-end — the HITL withhold→approve across a kill).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §1.4 (the HITL approval card is a
    Notif item reason=approval_requested at high priority), §2.4 (the escalation/notify exactly-once across a
    kill), §3.3 (humanise — the card's action+risk+cost render).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.3 (humanise — the HITL card),
    8.6 (explicit-first — the casual mention does not auto-spawn), 9.4 (the durable signal — the HITL
    withhold→approve), 7.5 (the escalation/notify legs exactly-once). Read the testing-strategy E2E section + X-1.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M5.3 (the E2E-2 leg) + §4 (the M5 deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    the E2E-2 row (CI-fail → triage agent → issue → chat → fix-PR: the HITL approval card is a Notif item
    reason=approval_requested showing action+risk+cost; the agent's inbox notification on the casual mention does
    not spawn a run; the escalation/notify legs are exactly-once across a kill).
- **DELIVERABLE (what to build + exactly where in the repo).** In the whole-system E2E harness (the M5 wedge),
  Notif's E2E-2 leg wired against the full myelin-notif surface:
  - E2E-2 CI-fail → triage agent → issue → chat → fix-PR (the flagship): the HITL approval card is a Notif item
    (reason=approval_requested, NOTIF-P5/P7) showing action + risk + cost (humanised, NOTIF-P9); the agent's
    inbox notification on the casual mention does NOT spawn a run (the explicit-first boundary, NOTIF-P22); the
    escalation/notify legs are exactly-once across a kill (NOTIF-P14); the multi-step HITL withhold→approve rides
    the durable signal (9.4).
  - FLOOR named: the E2E-4 DSAR leg + STOR-D2 is NOTIF-P30. Name it.
- **CONTRACTS TO IMPLEMENT.** None NEW owned. Consumed/exercised: 7.3 humanise, 8.6 explicit-first, 9.4 the
  durable signal, 7.5 escalation. Implement the E2E-2 leg to assert the named green artifacts; do not
  re-implement Notif logic.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-2 (SCHED): the HITL withhold→approve→apply ledger (the Notif approval card is the notify side);
    exactly-once across a kill; the casual @agent mention → 0 auto-spawn. Threshold: 0 mutation pre-approval, 1
    apply, exactly-once across a kill; 0 auto-spawn from a casual mention.
- **TESTS (required).** The E2E harness scenario for E2E-2 (Notif's leg), emitting its named green artifact.
  These chain mutations end-to-end across the full cell with mock agents (EI-01 §4) — a CI fail → triage agent →
  HITL card withheld → human approves → apply, with a kill mid-escalation asserting exactly-once. No new
  mandatory-core module; confirm the NOTIF-P7/P9/P14/P22 mutation floors still hold under the E2E exercise.
- **DEFINITION OF DONE.** Notif's E2E-2 leg emits its dated green artifact (the HITL approval card is the notify
  side; exactly-once across a kill; 0 auto-spawn from a casual mention, PROVEN); lints + coverage scanner are
  green; any untested-but-named surface is honestly recorded; the E2E-4 floor (NOTIF-P30) is named; the work is
  committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Notif E2E-2 leg (HITL flagship — approval card + explicit-first +
  exactly-once). Body lists: the E2E-2 leg greened (HITL withhold→approve, exactly-once across a kill, 0
  auto-spawn); the E2E-4 floor named (NOTIF-P30). Branch first if on default; do not push unless asked. End with
  the Co-Authored-By trailer.

---

### NOTIF-P30 — The E2E wedge: Notif's E2E-4 DSAR leg + STOR-D2 at cell scale (the permanent gate; the last Notif prompt)

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.3 — ... Notif's legs";
  this prompt is the E2E-4 DSAR leg + the STOR-D2 permanent-gate re-confirmation; it is the LAST Notif prompt —
  Notif's roadmap is fully covered when this is green).
- **DEPENDS-ON.** NOTIF-P27 (the erasure residual — Notif's DSAR holder coverage), NOTIF-P24 (cross-cell — the
  multi-cell DSAR leg). All five subsystems live + the M5 E2E harness. The M1 restore-verify at cell scale (11.5
  / STOR-D2). The index places this last in Notif's set, after NOTIF-P28/P29 and the E2E harness.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (GDPR-safe by construction — the DSAR fan-out);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the DSAR chained-mutation drill +
    the STOR-D2 restore at cell scale), §4 (chain mutations end-to-end).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §5.5 (the system-of-record tables
    prefs/on-call/templates are restore-verify gated), §3.9 (the holder — Notif is one of the H1–H18 holders in
    the DSAR; locate→erase over notification history contributes its receipt).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.7 (the holder — Notif is one of
    the H1–H18 holders), 11.5 (restore-verify / STOR-D2 at cell scale on the system-of-record tables), 10.4 (the
    multi-cell DSAR member_cells iteration). Read the testing-strategy E2E section.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M5.3 (Notif's E2E-4 leg + the STOR-D2
    re-confirmation) + §4 (the M5 deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    the E2E-4 row (DSAR fan-out: Notif is one of the H1–H18 holders; locate→erase over notification history
    contributes its receipt; post-erase locate = 0 recoverable PII; inbox items show [erased user]) + STOR-D2 at
    cell scale.
- **DELIVERABLE (what to build + exactly where in the repo).** In the whole-system E2E harness (the M5 wedge),
  Notif's E2E-4 leg + the STOR-D2 re-confirmation:
  - E2E-4 DSAR fan-out: Notif is one of the H1–H18 holders; locate→erase over notification history (NOTIF-P27)
    contributes its receipt; post-erase locate = 0 recoverable PII; inbox items show [erased user]. The
    multi-cell DSAR leg iterates member_cells over the cross-cell bridge (NOTIF-P24 / 10.4).
  - STOR-D2 at cell scale re-confirmed for Notif's system-of-record tables (prefs/on-call/templates — RPO/RTO
    under world-scale load, 11.5). This is a PERMANENT gate (master §4) — say so; it re-runs on every
    store-touching change.
  - FLOOR named: this is the LAST Notif prompt — name that Notif's roadmap is fully covered when this is green;
    no further Notif floor remains open except the named post-M5 follow-ons (ML ranking; counsel/DPO ratification
    of the EU provider + erasure residual). Name them.
- **CONTRACTS TO IMPLEMENT.** None NEW owned (the E2E wedge exercises the full surface). Consumed/exercised: 7.7
  the holder, 11.5 restore-verify, 10.4 member_cells. Implement the E2E-4 leg + the STOR-D2 re-confirmation to
  assert the named green artifacts; do not re-implement Notif logic.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-4 (SCHED): the H1–H18 coverage receipt set INCLUDING Notif; post-erase locate = 0 recoverable PII;
    inbox items show [erased user]. Threshold: 0 holders missed, 0 recoverable PII.
  - STOR-D2 at cell scale (SCHED, PERMANENT gate): restore Notif's system-of-record tables under world-scale
    load → RPO ≤ 5 min / RTO ≤ 1h-tenant / 4h-cell, 0 loss. Threshold: the master §2 M1 STOR-D2 thresholds —
    never weakened.
- **TESTS (required).** The E2E harness scenario for E2E-4 (Notif's leg), emitting its named green artifact. A
  restore-verify scenario for Notif's system-of-record tables at cell scale (STOR-D2). These chain mutations
  end-to-end across the full cell with mock agents (EI-01 §4) — not single-handler tests. No new mandatory-core
  module (the wedge exercises existing modules); confirm the prior mutation floors (esp. NOTIF-P27 erase) still
  hold under the E2E exercise.
- **DEFINITION OF DONE.** Notif's E2E-4 DSAR leg emits its dated green artifact (the H1–H18 receipt set includes
  Notif; post-erase locate = 0 recoverable PII; inbox items show [erased user], PROVEN); STOR-D2 at cell scale is
  re-confirmed for Notif's system-of-record tables (the permanent gate, measured RPO/RTO); lints + coverage
  scanner are green; any untested-but-named surface is honestly recorded; the post-M5 follow-ons (ML ranking;
  counsel ratification) are named; the work is committed. This is the LAST Notif prompt — Notif's roadmap is
  fully covered. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Notif E2E-4 DSAR leg + STOR-D2 at cell scale (permanent gate). Body lists: the
  E2E-4 DSAR leg greened (H1–H18 receipt incl. Notif + post-erase locate = 0); STOR-D2 re-confirmed at cell
  scale (permanent gate, measured RPO/RTO); the post-M5 follow-ons named (ML ranking; counsel ratification);
  note this is the last Notif prompt. Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

## Coverage matrix (every notifications roadmap milestone → its prompt(s))

| Roadmap milestone (planning/06-roadmaps/shared/notifications.md) | Band | Prompt(s) | Primary drills greened |
|---|---|---|---|
| N-M2.0 — holder + outbox + Signal-consumer skeleton | M2 | NOTIF-P1 (service shell + contract carriers), NOTIF-P2 (data model), NOTIF-P3 (router skeleton), NOTIF-P4 (holder registration) | NOTIF-D10 (P3); harness self-test (P3); holder/structural-erase (P4); contract-coverage scanner |
| N-M2.1 — read surface + humanise + ranking | M2 | NOTIF-P5 (list_inbox + C-9 filters), NOTIF-P6 (read-state), NOTIF-P7 (ranking), NOTIF-P8 (define_notif_rule seam), NOTIF-P9 (humanise), NOTIF-P10 (prefs/quiet-hours) | NOTIF-D1 (P7); NOTIF-D4 (P9); C-9 invariant (P5); one-read-state-truth (P6) |
| N-M2.2 — storm-control + write/read fanout split | M2 | NOTIF-P11 (five-mechanism storm-control), NOTIF-P12 (write-fanout), NOTIF-P13 (read-fanout) | NOTIF-D2 (P11 + P12/P13 amplification leg) |
| N-M2.3 — escalation + live transport + delivery idempotency | M2 | NOTIF-P14 (escalation), NOTIF-P15 (inbox watch), NOTIF-P16 (delivery), NOTIF-P17 (reindex), NOTIF-P18 (snooze re-surface) | NOTIF-D7, NOTIF-D8 (P14); D-N11 resume leg (P15); NOTIF-D9 (P16); NOTIF-D3 (P17); snooze durability (P18) |
| N-M3 — Git + Knowledge register reasons + watchers | M3 | NOTIF-P19 (Git), NOTIF-P20 (Knowledge) | NOTIF-D4 on real Git subjects + GIT-D8 (P19); NOTIF-D4 on real KN subjects + KN-D5/D13 (P20) |
| N-M4 — Issues + Chat + CI register | M4 | NOTIF-P21 (Issues SLA chains), NOTIF-P22 (Chat activity/explicit-first), NOTIF-P23 (CI HumanisedRef) | ISS-D6 (P21); CHAT-D5, CHAT-D17 (P22); HumanisedRef-resolution (P23) |
| N-M5.1 — cross-cell inbox aggregation | M5 | NOTIF-P24 | GA-D8 / CP-D7 / CP-D8 (inbox legs) |
| N-M5.2 — surge shed budget + EU delivery + erasure residual | M5 | NOTIF-P25 (surge), NOTIF-P26 (EU provider), NOTIF-P27 (erasure residual) | NOTIF-D5 (P25); NOTIF-D9 re-run under real provider (P26); NOTIF-D6 (P27) |
| N-M5.3 — the E2E wedge legs | M5 | NOTIF-P28 (E2E-1 pane), NOTIF-P29 (E2E-2 HITL flagship), NOTIF-P30 (E2E-4 DSAR + STOR-D2) | E2E-1 (P28); E2E-2 (P29); E2E-4 + STOR-D2 at cell scale (P30) |

**Contract → prompt(s) (every Notif-owned contract 7.1–7.8 + the consumed surfaces covered):** 7.1 list_inbox →
NOTIF-P5; 7.2 mark/snooze/mark_all_read → NOTIF-P6 (+ snooze timer NOTIF-P18); 7.3 humanise → NOTIF-P9 (+ CI
HumanisedRef registration NOTIF-P23); 7.4 get_prefs/set_prefs → NOTIF-P10; 7.5 oncall_now/page → NOTIF-P14 (+
Issues real chain NOTIF-P21); 7.6 define_notif_rule → NOTIF-P8 (seam) → NOTIF-P19/P20/P21/P22/P23
(registrations); 7.7 PersonalDataHolder + replay → NOTIF-P4 (holder half) → NOTIF-P17 (replay half) → NOTIF-P27
(erasure residual); 7.8 DeliveryAdapter → NOTIF-P16 (trait + mock) → NOTIF-P26 (real EU provider); telemetry 1.8
→ NOTIF-P3/P7/P11/P14/P16/P25 (the survival signals). The consumed read-fanout SetExpr push-down (4.3/4.4) →
NOTIF-P13; the firehose 3.5 → NOTIF-P15; the durable timers/signals 9.1/9.3/9.4 → NOTIF-P14/P18; the
CrossCellPointer 12.6 → NOTIF-P24.

**Notif's M2-exit obligations (master §2 M2 exit gate):** NOTIF-D4 (NOTIF-P9) + NOTIF-D7 (NOTIF-P14) — both
named in the master M2 exit gate; the band also requires the hard AG-D4 sandbox-escape GATE (owned by the agent
fabric) to be green before Notif is "done in M2" (the gate invariant, EI-01 §2). Notif's full M2-exit drill set
(NOTIF-D7/D8/D9/D3 + the D-N11 resume leg) is cleared jointly by NOTIF-P14/P15/P16/P17. **Permanent gate re-run
by Notif:** STOR-D2 at cell scale (NOTIF-P30), re-run on every store-touching change.

**Floors and their follow-on prompts (name-your-floors, VISION §3 / EI-04 §4):** single-home-cell inbox
(NOTIF-P1/P2) → cross-cell aggregation (NOTIF-P24); deterministic-v1 ranking (NOTIF-P7) → ML ranking (post-M5,
measured by NOTIF-D1); stubbed default reason set (NOTIF-P8) → per-subsystem enumerations
(NOTIF-P19/P20/P21/P22/P23); synthetic-watcher read-fanout (NOTIF-P13) → real watcher fragments
(NOTIF-P19/P20/P21/P22); the durable-snooze-timer seam (NOTIF-P6) → the durable re-surface (NOTIF-P18); the
Notif-defined escalation test chain (NOTIF-P14) → Issues' real SLA chain (NOTIF-P21); mock DeliveryAdapter
(NOTIF-P16) → real EU-sovereign provider (NOTIF-P26); the off-cell-payload erasure residual / structural
crypto-shred floor (NOTIF-P4) → provider-side erasure + counsel ratification (NOTIF-P27, with the EU provider
hook from NOTIF-P26). Each floor pair is visible here so the gap is never invisible.
