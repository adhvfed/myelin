# Phase 7 — Prompt Ledger: Notifications (myelin-notif)

> Phase: 07-prompts (per-system file, Phase 7-A). The complete ordered set of implementation prompts that
> operationalize the entire notifications roadmap (planning/06-roadmaps/shared/notifications.md, milestones
> N-M2.0..N-M5.3) into clean-context, independently-committable coding tasks. Built to the template in
> planning/07-prompts/00-ledger-overview.md §2 (every field present, never implicit) and banded to
> planning/06-roadmaps/00-master-sequencing.md §2 (M0..M6, the gate invariant). Frozen architecture (this file
> OPERATIONALIZES, it does not redesign): planning/05-refined-shared-systems-architecture/notifications.md +
> contract-index.md §7 (Notif 7.1–7.8) + the consumed rows (3.1/3.5/3.6, 2.2/2.4, 4.2/4.3/4.4/4.10, 5.2/5.6,
> 9.1/9.3/9.4, 12.6, 13.1/13.3) + 00-reconciliation-decisions.md (X-1/X-2/X-3/X-7, OQ-C/OQ-E/OQ-I/OQ-J/OQ-K/OQ-L).
> Plain-text identifiers throughout (no backticks-as-emphasis). Markdown only; this file makes no commits.
> Date: 2026-06-19.
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
> Coverage: N-M2.0 → NOTIF-P1; N-M2.1 → NOTIF-P2, NOTIF-P3; N-M2.2 → NOTIF-P4; N-M2.3 → NOTIF-P5, NOTIF-P6,
> NOTIF-P7; N-M3 → NOTIF-P8; N-M4 → NOTIF-P9; N-M5.1 → NOTIF-P10; N-M5.2 → NOTIF-P11, NOTIF-P12; N-M5.3 →
> NOTIF-P13. Thirteen prompts, no milestone gap.

---

### NOTIF-P1 — Stand up myelin-notif: serve(AppSpec) shell + the data model + the Signal-consumer router + the holder

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.0 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.0 — Holder + outbox +
  Signal-consumer skeleton").
- **DEPENDS-ON.** The M0 substrate prompts that ship the Cargo workspace + the eight glue-crate skeletons,
  serve(AppSpec) (1.1), the transactional outbox + idempotent-consumer template (2.2/2.3/2.4/2.5), the twelve
  lints (1.6), the failure-injection harness, the contract-coverage scanner, and the EventEnvelope (2.1)
  (master §2 M0; substrate roadmap SUB-M0; event-bus roadmap). The M1 prompts that ship the (tenant, region)
  partition key + residency_verify (12.1/12.4), the OLTP store + RLS + the outbox table (11.1), the
  PersonalDataHolder trait + auto-registration + KMS per-subject DEK (10.1, 1.4, 11.3/11.4), and the
  restore-verify CI job (11.5) (master §2 M1). The Bus M2 prompt that ships define_signal_rule + the
  sig.<tenant>.> Signal stream (3.1). The index places this after those — Notif inherits the data-loss floor;
  it never invents an emit path. **Gate invariant inherited:** SUB-D1/SUB-D2/BUS-D4, all twelve lints, ID-D3,
  CP-D2/CP-D3, STOR-D1/STOR-D2 must be green before this prompt starts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (notifications as a shared backend system; the ONE inbox), §3 (name-your-floors,
    references-not-payloads, EU-sovereign, agent-native); ../../external-insights/01-process-and-quality-doctrine.md
    §2 (order-by-non-negotiability — the data-loss floor is below Notif), §3 (prove-it + observability is part
    of the pass), §5 (the committed ratchet — an uncommitted contract test is no contract test);
    ../../external-insights/04-hard-problems.md §5.3 (Notif is a projection — storm-control never touches the
    audit/history).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §1 (purpose, the C-9 resolution),
    §2 (the data model + the load-bearing invariants: template_args holds ArtifactRefs never strings;
    UNIQUE(tenant, recipient, dedup_key); one state column), §3.4 (the router loop, step-0 authorize,
    idempotent on origin_event), §3.9 (the PersonalDataHolder, references-not-payloads), §5.1 (cell-local,
    tenant-partitioned, bus-driven).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.7 (PersonalDataHolder +
    replay, references-not-payloads); 1.1 (serve(AppSpec)), 1.2 (three ports), 1.3 (liveness≠readiness), 1.4
    (holder auto-registration), 1.5 (forward-only migrations), 1.8 (telemetry signal set); 2.2 (OutboxTx::emit
    — the ONLY emit path), 2.4 (EventHandler consumer template, subjects() whitelist never *, ack-after-enqueue,
    dedup ledger, bounded prefetch, lag metric), 2.5 (consumer_dedup ledger); 3.1 (define_signal_rule, the
    sig.<tenant>.> Signal stream); 12.1/12.4 (the (tenant, region) partition + residency); 11.1 (OLTP tier +
    RLS + the outbox table); 10.1 (PersonalDataHolder surface). Read 00-reconciliation-decisions.md ADR-19
    reference (Notif consumes Signals, not evt.*).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.0 (the work + the gate), §3 (the contract
    table rows 7.7 + 1.8), §4 (the upstream-dependency list).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D10 (slow/poison Signal → no stall; lag alarm); §3.3 (assertions read from production telemetry).
- **DELIVERABLE (what to build + exactly where in the repo).** Two crates under the Cargo workspace:
  - The glue crate myelin-notif (the M2 skeleton, if not already laid down): the exposed contract types
    (InboxItem, HumanisedString, the reason/class enums, DeliveryAdapter trait shape) as compile-time carriers
    so a contract change breaks every consumer's build now (ADR-01).
  - The myelin-notif implementation crate (the Notif service): an AppSpec passed to serve(AppSpec) (1.1) — NOT
    a hand-rolled main — wiring boot → migrate → outbox relay → the router consumer → three ports
    (public/internal/metrics-health, 1.2) → graceful drain; liveness ≠ readiness (1.3); forward-only online
    migrations (1.5).
  - The data model migrations (refined §2): inbox_item, notif_pref, quiet_hours, delivery, oncall_schedule,
    escalation_policy, escalation_run, humanise_template, mute — every table (tenant, region)-partitioned with
    the partition key as the FIRST column (the residency-pin lint, M1). inbox_item stores template_args as
    ArtifactRefs (never rendered strings); UNIQUE(tenant, recipient, dedup_key) for write-time collapse; exactly
    ONE state column (the C-9 read-state truth); origin_event + reason columns (the NOTIF-2 provenance). Tag
    every PII-bearing column with #[personal_data(...)] so the no-untagged-personal-data lint passes.
  - The router as an EventHandler (2.4) consumer of Signals: subjects() returns the sig.<tenant>.> whitelist
    (NEVER *); idempotent on origin_event / event_id via the consumer_dedup ledger (2.5); ack-after-enqueue;
    bounded prefetch; the consumer-lag metric exported (1.8). At N-M2.0 the router's body is the skeleton: it
    UPSERTs an inbox_item from a Signal (no ranking/storm-control/fanout yet — those are NOTIF-P2..P4). It must
    not stall on a poison/slow Signal type: a NonRetryable verdict terminates a poison Signal, the lag alarm
    fires, other subjects keep flowing (head-of-line isolation).
  - The emit path: emit notif.item.created / notif.escalation.acked ONLY via OutboxTx::emit (2.2) — the
    no-raw-publish lint forbids any other path; there is no publish_now in this crate.
  - Register Notif as a PersonalDataHolder (notification history) via the harness auto-registration (1.4 / 10.1)
    so "we forgot notification history" is structurally impossible. References-not-payloads from day 1 (the
    holder's erase is wired structurally — tombstone-for-free because items store refs).
  - FLOOR named (write it in the module doc): the holder's off-cell-payload erasure residual is handled BY
    REFERENCE to the platform posture (X-7 / contract 10.9), instanced for Notif in NOTIF-P12 (N-M5.2). The
    ranking, storm-control, fanout, humanise, escalation, live-transport, and delivery surfaces are explicitly
    NOT in this prompt — name their follow-on prompts (NOTIF-P2..P7) so the skeleton is not mistaken for the
    working inbox.
- **CONTRACTS TO IMPLEMENT.** 7.7 PersonalDataHolder + replay (owned; the holder registration + the
  references-not-payloads tombstone-for-free; the replay/reindex half lands in NOTIF-P7). Consumed: 2.2
  OutboxTx::emit, 2.4 EventHandler template, 2.5 consumer_dedup, 3.1 the Signal stream, 1.1–1.5/1.8 the harness,
  10.1/1.4 holder auto-reg, 11.1 OLTP, 12.1/12.4 partition+residency. Implement to the frozen signatures — a
  needed shape change is a whole-workspace contract PR, escalated and written down, not a local divergence.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D10 (CI): inject a slow/poison Signal type → the whitelisted-template router does not stall, the
    poison terminates (NonRetryable), other subjects keep flowing, the lag alarm fires. Telemetry green
    artifact: consumer_lag bounded (signal 1.8); no-stall asserted. Threshold: 0 head-of-line stalls; lag below
    the thresholds-file default.
  - The harness self-test: inject a synthetic Signal → assert inbox_item UPSERTed AND the telemetry-assertion
    library reads consumer_lag and dedup_collapse_ratio (observability is part of the pass condition, EI-01 §3).
  - The contract-coverage scanner passes on the Notif rows 7.1–7.8 (provider + consumer CDC present, even where
    a contract's body lands in a later prompt — the stub must exist) — CI.
  - All twelve committed lints green (esp. no-raw-publish, tenant-predicate, residency-pin,
    no-untagged-personal-data) with the Notif crate in the tree — CI.
- **TESTS (required).** Unit tests for the router's idempotency (a re-delivered Signal UPSERTs once), the
  dedup_key write-time collapse, and the holder's structural erase (a refs-stored item tombstones with no PII
  mutation). The drill-harness scenario for NOTIF-D10. The provider + consumer CDC pair for contract row 7.7
  (and CDC stubs for 7.1–7.8 so the scanner passes). The router is mandatory-core: state the cargo-mutants
  mutation-score floor for the router module in this field and meet it. Prefer a test that chains
  Signal-in → UPSERT → re-deliver → assert-single over a single-handler test (EI-01 §4).
- **DEFINITION OF DONE.** myelin-notif compiles in the workspace and boots via serve(AppSpec); the data model
  migrates forward-only; the router consumes Signals idempotently with the whitelist (never *); emits only via
  OutboxTx::emit; registers as a holder; NOTIF-D10 emits its dated green artifact (PROVEN: no stall + lag
  alarm); the harness self-test passes with the telemetry assertion; the contract-coverage scanner + all twelve
  lints are green; the floor (off-cell erasure residual → NOTIF-P12; the algorithm surfaces → NOTIF-P2..P7) is
  named in writing; the work is committed. No gate is greened by weakening a threshold.
- **COMMIT.** Header: P-<NNN> M2: myelin-notif shell — data model + Signal-consumer router + holder. Body lists:
  contract 7.7 (holder half) implemented; NOTIF-D10 greened (0 stalls, lag-alarm fired, measured lag); the
  router mutation-score measured; the floors named (erasure residual NOTIF-P12; algorithm surfaces
  NOTIF-P2..P7). Branch first if on default; do not push unless asked. End with the workspace Co-Authored-By
  trailer.

---

### NOTIF-P2 — list_inbox (the ONE inbox) + read-state + deterministic explainable ranking + the define_notif_rule seam + CLI

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.1 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.1 — The read surface +
  humanisation + ranking"; this prompt is the read-surface + ranking slice; humanisation is NOTIF-P3).
- **DEPENDS-ON.** NOTIF-P1 (the myelin-notif shell, data model, router, holder). The M1 Identity prompts that
  ship list_objects/list_subjects SetExpr + check + zookie (4.2/4.3/4.4/4.10). The index places this after
  NOTIF-P1 and the Identity M1 read-path prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §1 (the ONE inbox), §3 (name-your-floors — deterministic-v1 ranking with ML as the named
    follow-on; honesty about uncertainty); ../../external-insights/01-process-and-quality-doctrine.md §3
    (prove-it: a target you cannot measure is not a gate — the explain-trace is the observability of the rank),
    §1 (name-your-floors).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §1.3 (the C-9 resolution — views
    are filters over reason/subject, never a second store; the table of Issues "My Work" / Chat "Activity" /
    Git "Review requests" as filters), §1.4 (agents have inboxes too), §3.1 (the deterministic explainable
    scoring function priority ∈ 0..100; the reason → base → class table; affinity/role_weight from
    Id list_objects/relations + Refs backlinks behind a strategy interface; ML is the named follow-on), §2.1
    (one read-state store, the state column is the same row across every view).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.1 (list_inbox, the ONE inbox,
    scoped views are filters), 7.2 (mark/snooze/mark_all_read, one read-state truth), 7.6 (define_notif_rule,
    the registration seam), 4.3 (list_objects SetExpr push-down — affinity/role + step-0 candidate filtering),
    4.2 (check), 4.10 (zookie). Read 00-reconciliation-decisions.md §5 (C-9 resolution) + OQ-E (the SetExpr
    push-down).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.1 (the work + the NOTIF-D1 gate) + §2 (the
    ranking floor → ML follow-on row) + §4 (upstream deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D1 (replay a mixed week → every critical/direct ranks above every fyi; first-important latency in
    budget; explain-trace per rank; important-buried-rate 0).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - list_inbox(principal, filter?, page?) -> [InboxItem] ranked by priority (7.1) — the ONE inbox. The filter
    grammar over reason + subject so a subsystem adds a SAVED VIEW, never a second store (the C-9 invariant):
    implement Issues "My Work" = filter subsystem∈{issue} ∧ reason∈{assigned, mentioned, review_requested, sla,
    watched, blocked, approval_requested}; Chat "Activity/Mentions" = filter subsystem∈{chat} ∧
    reason∈{mentioned, replied, thread_watched, approval_requested}; Git "Review requests" = filter
    subsystem∈{git} ∧ reason∈{review_requested, mentioned}. These are filters, not stores — assert in a test
    that they read the same rows as the unfiltered inbox.
  - mark(item_id, state) / snooze(item_id, until) / mark_all_read(filter) (7.2): ONE read-state truth — read it
    in a scoped view, it is read in the unified inbox (the state column is the same row). snooze schedules a
    re-surface (the durable timer wiring lands in NOTIF-P5; here record the until and surface the snoozed-state
    semantics).
  - The v1 ranking function (refined §3.1): the deterministic, explainable scoring (priority ∈ 0..100); the
    reason → base → class table (approval_requested/escalated/sla = 90/critical; review_requested/assigned/
    mentioned = 70/direct; replied/agent_proposal = 55/participating; watched/state_changed = 35/watching;
    team/project fyi = 15/fyi). affinity/role_weight derived from Id list_objects/relations + Refs backlinks
    BEHIND a strategy interface (so the ML ranker swaps in without a rewrite). EVERY rank carries an
    explain-trace ("why am I seeing this, ranked here" — NOTIF-2). FLOOR named: ML-tuned ranking is the
    post-M5 follow-on behind the same scoring interface; the promotion trigger is a measured important-buried
    signal (NOTIF-D1), not a prediction.
  - define_notif_rule(reason, dedup_tpl, default_class) (7.6) — the registration seam each subsystem calls in
    M3/M4. Ship the Notif-owned DEFAULT reason set STUBBED (the per-subsystem enumeration of the default set is
    the N-M3/N-M4 accretion, NOTIF-P8/P9). FLOOR named: the stubbed default set → per-subsystem enumerations.
  - CLI: myelin inbox list|show|read|snooze (the read+state surface; prefs lands in NOTIF-P3, watch in
    NOTIF-P5).
- **CONTRACTS TO IMPLEMENT.** 7.1 list_inbox (owned), 7.2 mark/snooze/mark_all_read (owned), 7.6
  define_notif_rule (owned, the seam). Consumed: 4.3 list_objects SetExpr (affinity + step-0 candidate
  filtering), 4.2 check, 4.10 zookie. Frozen signatures only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D1 (SCHED): replay a mixed week of Signals → every critical/direct ranks above every fyi;
    first-important latency within the thresholds-file budget; an explain-trace present on every rank.
    Telemetry green artifact: important_buried_rate = 0 (signal 1.8); inbox_read_latency within budget. The
    threshold is important_buried_rate = 0 — never weakened.
  - The C-9 invariant test: a scoped filtered view returns a strict subset of list_inbox(filter=∅) rows, and
    marking-read in one view flips state in the other (one read-state truth) — CI.
  - The contract-coverage scanner passes on 7.1/7.2/7.6 (provider + consumer CDC) — CI.
- **TESTS (required).** Unit tests for the scoring function (the reason → base → class table is exact; the
  explain-trace is present and deterministic) and the filter grammar (a view is a subset). The drill-harness
  scenario for NOTIF-D1. A chained test: ingest a mixed batch → list_inbox → mark_all_read(filter) → re-list →
  assert read-state consistent across views (EI-01 §4, chain not single-handler). The ranking module is
  mandatory-core: state the cargo-mutants mutation-score floor and meet it. The provider + consumer CDC pair
  for 7.1/7.2/7.6.
- **DEFINITION OF DONE.** list_inbox returns ranked items; the scoped views are filters (proven a subset, one
  read-state truth); the deterministic ranking emits an explain-trace per rank; define_notif_rule exists as the
  seam; NOTIF-D1 emits its dated green artifact (important_buried_rate = 0, PROVEN); the C-9 invariant test +
  CDC + lints + coverage scanner are green; the floors (ML ranking; stubbed default reason set) are named in
  writing; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: list_inbox (the ONE inbox) + read-state + deterministic ranking. Body lists:
  contracts 7.1/7.2/7.6 implemented; NOTIF-D1 greened (important_buried_rate = 0, measured first-important
  latency); the ranking mutation-score measured; the floors named (ML ranking; per-subsystem reason sets
  NOTIF-P8/P9). Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P3 — humanise (the ONE templating surface, per-viewer-safe) + prefs/quiet-hours over the frozen QueryAst

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.1 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.1 — The read surface +
  humanisation + ranking"; this prompt is the humanisation + prefs slice and ships the NOTIF-D4 leak gate).
- **DEPENDS-ON.** NOTIF-P1 (the shell + holder), NOTIF-P2 (list_inbox + the read surface humanise renders). The
  M2 Refs prompts that ship resolve(ref, viewer, Display) + project (5.2/5.6) and have greened the Refs leak
  drills (REF-D1/REF-D2) — Notif's humanise leak drill cannot be honest before Refs' resolve-as-tombstone is
  proven. The M2 frozen-shared-crate prompts: myelin-content taxonomy + WASM render target (13.1) and
  myelin-query QueryAst (13.3). The index places this after Refs' M2 resolve prompt and the content/query freeze
  prompts.
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
    myelin-content WASM render path; email = sanitised-HTML, CLI = plain-text), §2.2 (the preference matcher
    binds the frozen QueryAst = the EventMatcher core; quiet-hours in the recipient tz; critical/escalated
    pierce by default via pierce_classes), §2.5 (humanise_template, ICU MessageFormat, platform-defaulted +
    tenant/locale-overridable).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.3 (humanise — the ONE
    templating surface, OQ-L; resolves each ArtifactRef per-viewer via Refs resolve(Display); permission/
    erasure-safe; ICU), 7.4 (get_prefs/set_prefs — matcher reuses the QueryAst core), 5.2 (resolve(ref, viewer,
    Display) → Projection|Tombstone), 5.6 (project), 13.1 (myelin-content taxonomy + WASM render target,
    render(parse(md)) === md), 13.3 (myelin-query QueryAst), 3.4 (EventMatcher = the QueryAst). Read
    00-reconciliation-decisions.md OQ-L (sole templating surface), OQ-C/X-3 (the frozen QueryAst), OQ-I
    (resolution is cell-local).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.1 (the humanise + prefs work + the NOTIF-D4
    gate) + §4 (the Refs + content/query upstream deps).
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
    subject humanises to a tombstone, the title never leaks; and the router (NOTIF-P1) suppresses an item whose
    subject the recipient cannot see.
  - The humanise_template store (refined §2.5): ICU MessageFormat, platform-defaulted + tenant/locale-overridable.
  - get_prefs/set_prefs(principal, routing, quiet_hours, digest) (7.4): the matcher reuses the frozen
    myelin-query QueryAst core (13.3 = the EventMatcher 3.4) — Notif does NOT invent a second predicate
    language. Quiet-hours evaluated in the recipient's tz; critical/escalated pierce by default (pierce_classes
    — the one deliberate quiet-hours override; you cannot silence an on-call page). CLI: myelin inbox prefs;
    myelin notify prefs|test.
  - FLOOR named: cross-cell humanisation is single-home-cell here; the always-cell-local resolution rule (OQ-I)
    is built into the resolve-call shape but the multi-cell aggregation is NOTIF-P10 (N-M5.1). Name it.
- **CONTRACTS TO IMPLEMENT.** 7.3 humanise (owned, the sole templating surface), 7.4 get_prefs/set_prefs
  (owned). Consumed: 5.2 resolve(Display), 5.6 project, 13.1 myelin-content + WASM render, 13.3/3.4 the QueryAst.
  Frozen signatures only — the humanise signature is the sole templating surface every other subsystem registers
  against (CI HumanisedRef, KN/Issues templates), so it must NOT diverge locally.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D4 (CI): notify on a confidential issue / private channel to a viewer lacking access → humanise
    returns a tombstone, the title NEVER appears in the output; the item is suppressed if the recipient cannot
    see the subject. Telemetry/assert green artifact: 0 title/PII leak (the F1 leak floor). The threshold is 0
    — never inverted, never softened. This is the M2-exit obligation for Notif (master §2 M2 exit gate names
    NOTIF-D4) and re-runs against real subjects in NOTIF-P8.
  - A render-determinism check: render(parse(md)) === md for humanised markdown through the one WASM path (the
    content-crate round-trip generalised to humanise output) — CI.
  - The QueryAst-matcher cost-bound: a preference matcher predicate is statically cost-bounded, no UDFs/loops/
    recursion (the frozen QueryAst property) — CI.
- **TESTS (required).** Unit tests for the render pipeline (a tombstone binds on deny; an erased actor →
  [erased user]; ICU plural/locale formatting; the markdown path never leaks raw). A chained test (EI-01 §4):
  render an item for a viewer WITH access (title shown) → revoke access (a new zookie) → re-render → assert the
  title is now a tombstone (the per-viewer property under a mid-flight permission change). The drill-harness
  scenario for NOTIF-D4. The provider + consumer CDC pair for 7.3/7.4. humanise is mandatory-core (every
  channel renderer leans on it): state the cargo-mutants mutation-score floor for the render module and meet it.
- **DEFINITION OF DONE.** humanise resolves each ref per-viewer and tombstones on deny; the title never leaks;
  prefs/quiet-hours bind the frozen QueryAst with pierce_classes; NOTIF-D4 emits its dated green artifact (0
  title/PII leak, PROVEN — the F1 floor proven before any real subsystem subject flows); the render-determinism
  + CDC + lints + coverage scanner are green; the cross-cell floor (NOTIF-P10) is named; the work is committed.
  A red leak gate is never made green by inverting the assertion.
- **COMMIT.** Header: P-<NNN> M2: humanise (the ONE templating surface) + prefs/quiet-hours. Body lists:
  contracts 7.3/7.4 implemented; NOTIF-D4 greened (0 title/PII leak, measured); the render mutation-score
  measured; the floor named (cross-cell humanise NOTIF-P10). Branch first if on default; do not push unless
  asked. End with the Co-Authored-By trailer.

---

### NOTIF-P4 — Storm-control + the write/read fanout split (the scale-axis floor; the SetExpr watcher push-down)

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.2 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.2 — Storm-control + the
  write/read fanout split").
- **DEPENDS-ON.** NOTIF-P1 (the router + data model), NOTIF-P2 (list_inbox — the read-fanout materialises on
  inbox open). The M1 Identity prompts that ship list_subjects + list_objects SetExpr + the per-tenant authz
  reverse index + zookie (4.3/4.4/4.10), pinned performant at 50k-member channel density. The M2
  frozen-content prompt that ships the mention(Principal) inline node (13.1). The index places this after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable from day 1 — the fan-out scale axis);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the storm drill forces the storm),
    §1 (name-your-floors — synthetic-watcher floor → real fragments);
    ../../external-insights/04-hard-problems.md §5.3 (Notif is a projection — storm-control suppresses delivery
    and ranking, NEVER the audit/history).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.2 (the five write-time
    storm-control mechanisms: self-suppression actor==recipient → drop; dedup-key collapse ON CONFLICT DO UPDATE
    SET coalesce_count+1 → "+N more"; thread/subject coalescing; per-(recipient, subject_root) token-bucket rate
    damping; mute/DND honoring), §3.5 (the hybrid fanout: write-fanout for the bounded high-signal set via the
    mention(Principal) frozen inline structured node — Notif reads the structured node, never parses free text,
    the agent-loop reference gate AG-6; read-fanout for the unbounded ambient set — ONE coalesced marker,
    materialise per-watcher lazily on inbox open, a 50k-watcher celebrity costs zero write amplification; the
    watcher resolution via list_objects(recipient, watch, type) → Filter{set_expr, zookie} lowered into a SQL
    JOIN against the authz_visible reverse index over Notif's own subject_root/subject column — one query, no
    N+1, no post-filter; the hot-subject cap §3.2.4), §5.2 (per-tenant in-flight caps).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 4.3 (list_objects SetExpr
    push-down, lowered to a SQL JOIN over the consumer's own id column via the authz reverse index, no N+1, no
    post-filter), 4.4 (list_subjects, performant at 50k-member channel density, served by the same reverse
    index), 4.10 (zookie — a just-revoked watch reflected at-or-after the zookie watermark), 13.1 (the
    mention(Principal) inline node). Read 00-reconciliation-decisions.md OQ-E (the watcher push-down), X-2 (the
    mention node byte-identical), the search-requires-acl-filter discipline generalised.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.2 (the five mechanisms + the hybrid fanout +
    the NOTIF-D2 gate + the synthetic-watcher floor) + §2 (the synthetic-watcher → real-fragment floor row).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D2 (1000 near-identical CI failures + a 30-comment PR burst → bounded items, coalesce_count
    correct; 0 self-notifications; dedup-collapse-ratio).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate, in the
  router pipeline (NOTIF-P1) between classify and UPSERT:
  - The five write-time storm-control mechanisms (refined §3.2): (1) self-suppression (actor.principal ==
    recipient → drop); (2) dedup-key collapse (INSERT ... ON CONFLICT (tenant, recipient, dedup_key) DO UPDATE
    SET coalesce_count = coalesce_count + 1 → "+N more"); (3) thread/subject coalescing (digest the
    participating, break out the direct); (4) per-(recipient, subject_root) token-bucket rate damping; (5)
    mute/DND honoring. Storm-control suppresses DELIVERY and RANKING only — NEVER the audit/history (the events
    still exist on the bus; Notif is a projection, EI-04 §5.3).
  - The hybrid fanout (refined §3.5):
    - Write-fanout for the bounded high-signal set (mentioned/assigned/reviewer/escalation targets): read the
      mention(Principal) frozen inline structured node from the myelin-content taxonomy (13.1) — Notif reads the
      STRUCTURED node, it does NOT parse free text (AG-6 — only a structured ref re-triggers) — and materialise
      one inbox_item per recipient. The hot-subject cap (§3.2.4) bounds even the write-fanout side so a
      mention-storm can't write-amplify.
    - Read-fanout for the unbounded ambient set (every watcher of a hot PR, every member of a 50k-channel): store
      ONE coalesced marker, materialise per-watcher LAZILY on inbox open. Resolve watchers via
      list_objects(recipient, watch, type) → Filter{set_expr, zookie} (4.3) and lower the SetExpr
      (InRelation{relation: watcher, via_column} / TupleSet forms) into a SQL JOIN against the authz_visible
      reverse index over Notif's own inbox_item.subject_root / subject column — ONE query, no N+1, no
      post-filter (the search-requires-acl-filter discipline generalised to the inbox read). A 50k-watcher
      celebrity subject costs ZERO write amplification. A security-sensitive read passes the zookie (4.10) so a
      just-revoked watch grant is reflected (the JOIN reads the reverse index at-or-after the zookie watermark);
      an item is held, not leaked, if a check can't resolve fresh.
  - FLOOR named: the read-fanout depends on every watchable subsystem declaring its watcher ReBAC fragment
    (4.9, C8) — those fragments land WITH their subsystems in M3/M4 (NOTIF-P8/P9). Until then the read-fanout is
    drilled against SYNTHETIC watcher tuples. Name the follow-on (real fragments land in N-M3/N-M4).
- **CONTRACTS TO IMPLEMENT.** None owned (this is internal router scaling). Consumed: 4.3 list_objects SetExpr
  (the watcher push-down — the highest-fan-in dependency), 4.4 list_subjects (50k-member density), 4.10 zookie,
  13.1 the mention(Principal) node. Implement to the frozen SetExpr lowering — no local re-invention of a
  watcher resolution path.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D2 (CI): 1000 near-identical CI failures + a 30-comment PR burst → bounded items (coalesce_count
    correct, "+N more"); self-notifications suppressed (actor==recipient). Telemetry green artifact:
    dedup-collapse-ratio measured + asserted; 0 self-notifications. Threshold: N identical → 1 item; 0 self.
  - A read-fanout-amplification check: a 50k-watcher subject produces ZERO per-watcher write rows (one coalesced
    marker), and an inbox open materialises only the viewer's slice via one JOIN (no N+1) — CI, with the
    synthetic-watcher fixture.
- **TESTS (required).** Unit tests for each of the five storm-control mechanisms (self-suppression,
  dedup-collapse, coalescing, token-bucket, mute). A chained test (EI-01 §4): emit a burst → assert
  coalesce_count increments on the single row (not N rows); a separate burst from the recipient themselves →
  0 items. The drill-harness scenario for NOTIF-D2. A read-fanout test against synthetic watcher tuples
  asserting one JOIN, zero write amplification, and the zookie watermark reflecting a revoked watch. The
  storm-control + fanout modules are mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** The five storm-control mechanisms suppress delivery/ranking but never the audit; the
  hybrid fanout splits write/read; the read-fanout resolves watchers via the SetExpr JOIN (one query, no N+1)
  with the zookie watermark; NOTIF-D2 emits its dated green artifact (N→1, 0 self, measured collapse-ratio,
  PROVEN); the read-fanout-amplification + CDC + lints + coverage scanner are green; the synthetic-watcher floor
  (real fragments NOTIF-P8/P9) is named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: storm-control + the write/read fanout split. Body lists: the five
  storm-control mechanisms + the hybrid fanout (SetExpr watcher push-down) implemented; NOTIF-D2 greened (N→1,
  0 self, measured collapse-ratio); the fanout mutation-score measured; the floor named (synthetic-watcher →
  real fragments NOTIF-P8/P9). Branch first if on default; do not push unless asked. End with the Co-Authored-By
  trailer.

---

### NOTIF-P5 — Escalation on the myelin-flow durable wheel (the frozen chain shape) + snooze re-surfacing

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.3 — Escalation on the
  durable wheel + the live transport + delivery idempotency"; this prompt is the escalation slice and ships
  NOTIF-D7 + NOTIF-D8).
- **DEPENDS-ON.** NOTIF-P1 (the shell, router, holder), NOTIF-P3 (humanise + prefs/pierce_classes — the
  critical-class pierce). The M2 myelin-flow prompts that ship DurableExecutor{start, signal, describe, cancel}
  + the durable timer wheel + the durable signal (9.1/9.3/9.4) and have greened FLOW-D1/FLOW-D2/FLOW-D5 (the
  durable-execution drills) — NOTIF-D7's exactly-once page rests on durable timers, which must be proven first.
  The index places this after the myelin-flow M2 prompts.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — agent escalations too; honesty about uncertainty);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — kill mid-ack_window forces the
    failure; observability is part of the pass).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §2.4 (the escalation-chain config
    shape FROZEN: page → oncall_now → notify(class=critical, pierces quiet-hours) → escalate-after-timer
    (ack_window) → if !acked next-step / if acked stop; Issues passes the chain definition; Notif owns POLICY
    evaluation, the workflow engine owns DURABILITY; the timers are myelin-flow durable timers not in-process
    sleeps; ack is an event notif.escalation.acked via outbox, the workflow signal-wait resolves on it; on-call
    cannot be silenced, pierce_classes default critical), §3.7 (escalation on the durable-workflow substrate;
    snooze re-surfacing and SLA timers ride the same minute-bucket wheel — one substrate three uses).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.5 (oncall_now(schedule) →
    principal; page(target, reason) starts an escalation durable workflow; the chain shape frozen), 9.1
    (DurableExecutor, signal idempotent on idem_key), 9.3 (the durable timer wheel — millions of timers as an
    indexed range read, effectively-once), 9.4 (durable signal — state=waiting holds no runtime, an
    ack/cancel signal arrives later, idempotent), 2.2 (OutboxTx::emit — the ack event). Read
    00-reconciliation-decisions.md §5 (the escalation chain) + OQ-F (the per-effect idem_key).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.3 (the escalation work + the NOTIF-D7/D8
    gates) + §4 (the myelin-flow upstream dep).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows NOTIF-D7 (start escalation; kill Notif mid-ack_window → durable workflow resumes, pages next step
    exactly once; ack stops the chain; exactly-once page; ack-halt) and NOTIF-D8 (set DND; fire a critical
    escalation → it pierces quiet-hours; a watching item is suppressed; critical pierces; non-crit suppressed).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - oncall_now(schedule) -> principal and page(target, reason) (7.5): page starts an escalation DURABLE
    WORKFLOW on the myelin-flow substrate (9.1/9.3/9.4) walking the frozen chain shape (refined §2.4):
    page(target, reason) → oncall_now(schedule) resolves the rotation at fire time → notify(principal, channels,
    class=critical) which pierces quiet-hours → escalate-after-timer(ack_window) a myelin-flow DURABLE TIMER
    that survives a Notif restart and fires effectively-once → if !acked walk to the next step; if acked stop.
    Notif owns the POLICY evaluation (which step, which target, which channels); the workflow engine owns the
    DURABILITY (the timer is a 9.3 durable timer, not an in-process sleep). Ack is an EVENT
    (notif.escalation.acked emitted via OutboxTx::emit, 2.2); the workflow's signal-wait (9.4) resolves on it.
    On-call cannot be silenced (pierce_classes default critical — you cannot silence an on-call page).
  - Wire the escalation_policy / escalation_run tables (from NOTIF-P1's data model) to the workflow: the
    escalation_run row holds the durable handle, so a restart resumes the chain, never misses or double-pages.
  - snooze re-surfacing on the SAME durable timer wheel (9.3): a snoozed item (NOTIF-P2's snooze) re-surfaces at
    its until via a durable timer — one substrate, three uses (escalation, snooze, SLA timers).
  - CLI: myelin oncall show|page.
  - FLOOR named: Issues passes its real SLA escalation chain definition in N-M4 (NOTIF-P9); here the chain
    shape is exercised with a Notif-defined test chain. Name it.
- **CONTRACTS TO IMPLEMENT.** 7.5 oncall_now/page (owned, the escalation durable workflow + the frozen chain
  shape). Consumed: 9.1 DurableExecutor, 9.3 the durable timer wheel, 9.4 the durable signal, 2.2 OutboxTx::emit
  (the ack event). Frozen chain shape only — Issues passes the chain, Notif evaluates it; the durability is the
  engine's.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D7 (CI): start an escalation; kill Notif mid-ack_window → the durable workflow resumes and pages the
    next step EXACTLY ONCE; an ack stops the chain. Telemetry green artifact: exactly-once page (0 missed, 0
    duplicate); ack-halt asserted. Threshold: 0 missed, 0 duplicate pages — never softened.
  - NOTIF-D8 (CI): set DND; fire a critical escalation → it PIERCES quiet-hours; a watching (non-critical) item
    is suppressed. Telemetry green artifact: critical pierces; non-crit suppressed; quiet_hours_pierce_count
    incremented (signal 1.8).
- **TESTS (required).** Unit tests for the chain-walk policy (step ordering, target resolution at fire time,
  pierce_classes). A chained durability test (EI-01 §4): start → kill the worker mid-ack_window → resume →
  assert one page to the next step (not zero, not two) → deliver the ack event → assert the chain halts. The
  drill-harness scenarios for NOTIF-D7 and NOTIF-D8. The escalation module is mandatory-core: state the
  cargo-mutants mutation-score floor and meet it. The provider + consumer CDC pair for 7.5.
- **DEFINITION OF DONE.** page starts a durable-workflow escalation walking the frozen chain on the myelin-flow
  wheel; ack is an outbox event the signal-wait resolves on; on-call cannot be silenced; snooze re-surfaces on
  the same wheel; NOTIF-D7 (exactly-once page, 0 missed/0 dup) and NOTIF-D8 (critical pierces, non-crit
  suppressed) emit their dated green artifacts (PROVEN); CDC + lints + coverage scanner are green; the floor
  (Issues passes the real chain in NOTIF-P9) is named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: escalation on the durable wheel (frozen chain) + snooze. Body lists: contract
  7.5 implemented (durable-workflow escalation, ack-as-event); NOTIF-D7 greened (exactly-once page, 0 missed/0
  dup) + NOTIF-D8 greened (critical pierces, non-crit suppressed); the escalation mutation-score measured; the
  floor named (Issues real chain NOTIF-P9). Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### NOTIF-P6 — The inbox watch live transport (the frozen firehose resume-cursor protocol)

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.3 — ... + the live
  transport + ..."; this prompt is the live-transport slice and ships the inbox-watch resume leg, D-N11).
- **DEPENDS-ON.** NOTIF-P1 (the router emits notif.item.created), NOTIF-P2 (list_inbox — the cold-rebuild
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
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md row 3.5 (the firehose transport +
    the resume-cursor subscription protocol: subscribe(stream, scope, cursor?) → SubStream, frames carry
    per-(stream, scope) monotonic seq, resume(stream, scope, last_seq) backfills then live, resync_required →
    snapshot fallback, scope is a bounded selector never *). Read 00-reconciliation-decisions.md OQ-J (the
    firehose resume-cursor protocol) + OQ-K (the connection-tier shed budget).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.3 (the inbox watch work + the resume-leg gate)
    + §4 (the firehose upstream dep) + §5 (D-N11 / the OQ-J resume-cursor family).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    the OQ-J resume-cursor family (Notif's leg = D-N11 in the refined doc §6: drop the inbox watch connection
    mid-stream, reconnect with last_seq → backfill (last_seq, now] then live, zero items lost; over-old cursor →
    resync_required → snapshot rebuild).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - The inbox watch live transport over the FROZEN firehose resume-cursor protocol (3.5) — Notif consumes the
    shared transport, it does NOT build a bespoke live path: subscribe(stream = fan.<tenant>.inbox.<principal>,
    scope = inbox:<principal>, cursor?) → SubStream yielding Frame{seq: u64, item_id, ...}; resume(stream,
    scope, last_seq) backfills (last_seq, now] from the bounded firehose retention window then resumes live — a
    reconnect loses ZERO items. The seq is per-(stream, scope) monotonic. An over-old last_seq → resync_required
    → the client falls back to a full list_inbox cold rebuild (the named, NOT-silent recovery path).
  - Per-view scope bounding: scope is the bounded selector inbox:<principal>, NEVER * — the transport rejects an
    unbounded scope (the whitelist-not-* rule, BUS-3, generalised). One client gets only its own inbox slice's
    frames, never the whole tenant's firehose.
  - Backpressure: per-connection in-flight frame caps; a slow consumer is dropped to resync_required rather than
    buffering unboundedly (the connection-tier shed budget, OQ-K). The durable bus still carries only the
    pointer event (notif.item.created); the firehose carries the live frame — the in-app delivery path stays
    in-cell.
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
  - The bounded-scope rejection: a subscribe with scope=* (or any unbounded selector) is REJECTED by the
    transport — CI (the BUS-3-generalised whitelist property).
- **TESTS (required).** Unit tests for the resume-cursor math (backfill range (last_seq, now]; the
  resync_required boundary at the retention window edge). A chained test (EI-01 §4): subscribe → receive frames
  1..k → drop → emit frames k+1..m while disconnected → reconnect with last_seq=k → assert frames k+1..m
  backfilled in order then live (0 lost, 0 dup). A test that an unbounded scope is rejected. The drill-harness
  scenario for the D-N11 resume leg. The provider + consumer CDC for the Notif consumption of 3.5. The
  watch-transport module is mandatory-core: state the cargo-mutants mutation-score floor and meet it.
- **DEFINITION OF DONE.** inbox watch rides the frozen firehose resume-cursor protocol (no bespoke transport);
  a reconnect loses zero items; an over-old cursor falls back to a named cold rebuild; an unbounded scope is
  rejected; the D-N11 resume leg emits its dated green artifact (0 lost, PROVEN); the bounded-scope rejection +
  CDC + lints + coverage scanner are green; the floor (the wire mechanism is the connection tier's) is named;
  the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M2: inbox watch live transport (firehose resume-cursor). Body lists: the
  subscribe/resume/scope consumption of contract 3.5 wired; the D-N11 resume leg greened (0 items lost across a
  reconnect, measured); the bounded-scope rejection proven; the watch mutation-score measured; the floor named
  (wire mechanism = connection tier). Branch first if on default; do not push unless asked. End with the
  Co-Authored-By trailer.

---

### NOTIF-P7 — The delivery fabric (idempotent DeliveryAdapter + mock) + reindex-from-source (clears the M2 exit)

- **BAND.** M2.
- **ROADMAP MILESTONE.** N-M2.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M2.3 — ... + delivery
  idempotency"; this prompt is the delivery + reindex slice and ships NOTIF-D9 + NOTIF-D3, clearing Notif's
  M2-exit drill set together with NOTIF-P3/P5/P6).
- **DEPENDS-ON.** NOTIF-P1 (the router, the holder, the delivery table), NOTIF-P3 (humanise — the
  RedactedMessage summary is a humanise render). The M0/M2 reindex-from-source prompt (events::reindex + the
  replay-through-the-live-consumer path, 2.6). The index places this after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (EU-sovereign by construction — region-aware, EU-preferring delivery, data-minimisation;
    name-your-floors — mock adapter → real EU provider; agents choose, strategy pattern for the swappable
    adapter); ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — crash between
    provider-ack and ledger-write); ../../external-insights/04-hard-problems.md §5.3 (the inbox is a projection;
    reindex-from-source is the only recovery path — no second read path so steady-state and recovery share one
    code path, cannot drift).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.6 (the EU-sovereign delivery
    fabric: one trait DeliveryAdapter{channel, region, send(RedactedMessage, idem_key), receipts} — EU-preferring,
    region-aware, swappable; RedactedMessage = a humanised summary + a deep link, never the full body where
    avoidable, delivery.redacted=true off-cell, GDPR Art. 5(1)(c) data-minimisation; in-app channels
    inbox/web_push/desktop never leave the cell; at-least-once + idempotent on UNIQUE(idem_key); FLOOR: the
    trait + EU-preferring posture + redaction ship, the concrete production EU provider is a sovereignty/legal
    selection deferred; v1 dev uses a deterministic mock adapter --use-mock), §3.8 (reindex-from-source:
    events::reindex(scope=notif) → owners replay *.snapshot → the SAME router re-ingests idempotently
    (origin_event dedup) → inbox_item/delivery reconstructed; cold == live; the only recovery path; doubles as
    new-recipient backfill + schema-upcaster; retention floor ~90-day item window, prefs/on-call/templates
    permanent restore-verify gated).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.8 (DeliveryAdapter,
    region-aware, EU-preferring, swappable, PII-minimised off-cell, at-least-once + idempotent), 7.7
    (PersonalDataHolder + replay — the reindex replay half), 2.6 (reindex-from-source — the only recovery path),
    11.5 (restore-verify on the system-of-record tables). Read 00-reconciliation-decisions.md the X-7 erasure
    posture reference (off-cell payloads) and the EU-sovereign delivery floor.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M2.3 (the delivery + reindex work + the
    NOTIF-D9/D3 gates) + §2 (the mock → EU-provider floor row) + the M2-exit context (NOTIF-D4 + NOTIF-D7 are
    the master-named M2 exit, but Notif's full M2-exit drill set is NOTIF-D7/D8/D9/D3 + the resume leg).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows NOTIF-D9 (crash between provider-ack and ledger-write, retry → UNIQUE(idem_key) collapses to
    exactly-one delivery per (item, channel); 1 effective delivery) and NOTIF-D3 (wipe inbox_item,
    reindex(notif) → rebuilt inbox matches live; reindex-parity hash).
- **DELIVERABLE (what to build + exactly where in the repo).** In the myelin-notif implementation crate:
  - The delivery fabric (7.8): the one trait DeliveryAdapter{channel, region, send(RedactedMessage, idem_key),
    receipts} — EU-preferring, region-aware, swappable (the same strategy-pattern mandate that swaps mock→real
    agents, generalised to sub-processors). RedactedMessage = a humanised summary (via NOTIF-P3's humanise) + a
    deep link, never the full body where avoidable; delivery.redacted = true for off-cell (Art. 5(1)(c)
    data-minimisation). In-app channels (inbox, web_push, desktop) NEVER leave the cell. Delivery is
    at-least-once + idempotent on UNIQUE(idem_key) in the delivery table. Ship the DETERMINISTIC MOCK adapter
    (--use-mock-as-runtime) — FLOOR named: the concrete production EU email/push provider (with its DPA/
    sub-processor posture) is N-M5.2 (NOTIF-P11/P12), a sovereignty/legal [OPEN — LEGAL] selection; the trait +
    EU-preferring posture + redaction discipline ship NOW.
  - reindex-from-source (7.7 replay / 2.6): events::reindex(scope=notif) → owners replay *.snapshot events
    through outbox→bus→Signal → the SAME router (NOTIF-P1) re-ingests idempotently (origin_event dedup) →
    inbox_item/delivery reconstructed; cold == live. This is the ONLY recovery path (no second read path → cannot
    drift). It doubles as new-recipient backfill + the schema-upcaster path. Retention floor: ~90-day item
    window (older items age out, reconstructable from the OLAP/Audit long-term holder); prefs/on-call/templates
    are permanent and restore-verify gated (11.5).
- **CONTRACTS TO IMPLEMENT.** 7.8 DeliveryAdapter (owned, the trait + mock), 7.7 replay (owned, the reindex
  half — completes the holder contract started in NOTIF-P1). Consumed: 2.6 reindex-from-source, 11.5
  restore-verify (the system-of-record tables). Frozen signatures only.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D9 (CI): crash between provider-ack and ledger-write, retry → UNIQUE(idem_key) collapses to
    EXACTLY-ONE effective delivery per (item, channel). Telemetry green artifact: 1 effective delivery;
    delivery_success measured (signal 1.8). Threshold: exactly 1 — never softened.
  - NOTIF-D3 (SCHED): wipe inbox_item, run reindex(notif) → the rebuilt inbox matches live (items + read-state
    from source events). Telemetry green artifact: reindex-parity hash equal (cold == live). Threshold: parity
    hash identical.
  - The in-app-stays-in-cell assertion: inbox/web_push/desktop channels produce no off-cell egress; an off-cell
    channel sends only a RedactedMessage with delivery.redacted=true — CI.
- **TESTS (required).** Unit tests for the idem_key collapse (a retry after provider-ack is a no-op) and the
  RedactedMessage minimisation (off-cell carries summary + link, never the body). A chained test (EI-01 §4):
  ingest a batch → reindex(notif) on a wiped store → assert the rebuilt inbox + read-state hash-equal to live.
  The drill-harness scenarios for NOTIF-D9 and NOTIF-D3. The delivery + reindex modules are mandatory-core:
  state the cargo-mutants mutation-score floor and meet it. The provider + consumer CDC pair for 7.8/7.7.
- **DEFINITION OF DONE.** The DeliveryAdapter trait + mock deliver at-least-once + idempotent (exactly-one per
  (item, channel)); off-cell is redacted, in-app stays in-cell; reindex-from-source rebuilds cold == live as the
  only recovery path; NOTIF-D9 (1 effective delivery) and NOTIF-D3 (reindex parity) emit their dated green
  artifacts (PROVEN); the in-app-stays-in-cell assertion + CDC + lints + coverage scanner are green; the floor
  (real EU provider NOTIF-P11/P12) is named; the work is committed. This prompt, with NOTIF-P3/P5/P6, clears
  Notif's M2-exit drill set — but Notif is "done in M2" only when the band-wide M2 gate (incl. the hard
  AG-D4 sandbox-escape GATE, owned by the agent fabric) is green (the gate invariant, EI-01 §2). No threshold
  weakened.
- **COMMIT.** Header: P-<NNN> M2: delivery fabric (idempotent + mock) + reindex-from-source. Body lists:
  contracts 7.8/7.7 implemented; NOTIF-D9 greened (1 effective delivery) + NOTIF-D3 greened (reindex parity
  hash equal); the delivery/reindex mutation-score measured; the floor named (real EU provider NOTIF-P11/P12);
  note that Notif's M2-exit drills are green but the band closes only when AG-D4 is green. Branch first if on
  default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P8 — Producer accretion: Git + Knowledge register reasons + watchers; re-confirm NOTIF-D4 on real subjects

- **BAND.** M3.
- **ROADMAP MILESTONE.** N-M3 (planning/06-roadmaps/shared/notifications.md §1 "N-M3 — Producer accretion: Git +
  Knowledge register their reasons + watchers").
- **DEPENDS-ON.** NOTIF-P2 (define_notif_rule seam), NOTIF-P3 (humanise — the leak surface), NOTIF-P4
  (read-fanout over real watcher tuples). The M3 Git prompts that ship the Git ReBAC namespace fragment incl.
  the watcher relation (4.9) + the Git event taxonomy + project(ref, viewer) (5.6). The M3 Knowledge prompts
  that ship the KN ReBAC fragment incl. watcher + the KN event taxonomy + project. The index places this after
  Git and Knowledge ship their M3 fragments. Notif itself is unchanged — this is pure registration.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (name-your-floors — synthetic watcher → real fragment; honesty);
    ../../external-insights/01-process-and-quality-doctrine.md §1 (the inverse-signal: if wiring a new
    subsystem's reasons gets HARDER each time, the define_notif_rule/watcher seam is wrong — stop and repair,
    don't add surface), §3 (prove-it — re-run the leak drill against REAL confidential subjects).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §3.5 (the watcher relation is a
    frozen ReBAC-fragment obligation, C8 — every watchable subsystem declares it, Notif reads it never invents
    it), §3.1 (the reason set the subsystems register), §3.3 (humanise per-viewer against real subjects).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.6 (define_notif_rule — Git/KN
    register their sets), 4.9 (the per-subsystem ReBAC namespace fragment — the watcher relation per watchable
    type; Git ref-glob + CODEOWNERS; KN page-tree inherit-with-overrides), 5.6 (project(ref, viewer) — the
    humanise projection), 4.3/4.4 (the read-fanout watcher resolution over the real fragments). Read
    00-reconciliation-decisions.md C8 (watcher fragment frozen obligation).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M3 (the Git + KN registration work + the gate) +
    §4 (the M3 accretion deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row NOTIF-D4 (re-run against REAL Git private repo / KN confidential page subjects, not synthetic) + the
    Git/KN confidential-leak rows GIT-D8, KN-D5/KN-D13 (Notif's resolve(Display) path is the leak surface they
    exercise).
- **DELIVERABLE (what to build + exactly where in the repo).** Mostly in Git's and Knowledge's crates (the
  registrations), with the verification in myelin-notif:
  - Git registers its define_notif_rule set (review_requested / mentioned) via the 7.6 seam + declares its
    watcher ReBAC fragment (4.9) → the Git "Review requests" filtered view (NOTIF-P2) becomes a REAL list_inbox
    view; read-fanout (NOTIF-P4) over REAL PR watchers goes live (replacing synthetic tuples).
  - Knowledge registers its set (mentions / comments / shares / watched) + declares its watcher fragment → KN
    mentions/comments flow; the agent-trace-adjacent reasons land.
  - In myelin-notif: verify the define_notif_rule + watcher seams accept the Git/KN registrations WITHOUT any
    Notif code change (the inverse-signal check — if it needs a change, the seam is wrong; record this
    explicitly). Re-run the humanise-per-viewer property (NOTIF-D4) against REAL confidential subjects (a Git
    private repo, a KN confidential page) — not synthetic — confirming the tombstone holds.
  - FLOOR named: Issues / Chat / CI reasons + watchers are M4 (NOTIF-P9); cross-cell is still single-home
    (NOTIF-P10). Name them.
- **CONTRACTS TO IMPLEMENT.** None NEW owned by Notif (Notif gains zero new contracts — refined "Changes vs
  Phase 3"). Git and Knowledge own their 4.9 watcher fragments + 7.6 registrations + 5.6 project; Notif consumes
  them. Verify against the frozen 7.6 / 4.9 / 5.6 shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D4 re-confirmed on REAL subjects (CI): notify on a real Git private repo / KN confidential page to a
    viewer lacking access → humanised tombstone, the title never appears. Telemetry green artifact: 0 title/PII
    leak against real subjects. Threshold: 0 — never softened.
  - The relevant Git/KN exit-gate rows that touch Notif's resolve(Display) leak surface: GIT-D8 (cross-tenant
    repo access denied) and KN-D5/KN-D13 (confidential page/row/field 0 leak incl. COUNT) green with Notif's
    humanise path exercised — CI/SCHED.
  - The inverse-signal record: a written note that Git + KN registered via the unchanged define_notif_rule /
    watcher seam with ZERO Notif code change (the seam is right) — observability of the compounding-payoff
    property (EI-01 closing).
- **TESTS (required).** Integration tests that Git's and KN's registrations produce real filtered views and
  real read-fanout (replacing the NOTIF-P4 synthetic fixtures). The drill-harness scenario for NOTIF-D4 on real
  subjects. No new Notif unit logic (pure registration) — but a contract test that the 7.6 / 4.9 seams accept
  the Git/KN sets unchanged. The CDC pairs for the Git/KN sides of 7.6 / 4.9 (provider Git/KN, consumer Notif).
- **DEFINITION OF DONE.** Git + KN register reasons + watcher fragments via the unchanged seams (ZERO Notif code
  change recorded); Git "Review requests" + KN mentions are real views; read-fanout runs over real watchers;
  NOTIF-D4 re-confirmed on real confidential subjects (0 leak, PROVEN); the Git/KN leak rows that touch
  humanise are green; CDC + lints + coverage scanner are green; the floors (Issues/Chat/CI NOTIF-P9; cross-cell
  NOTIF-P10) are named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M3: producer accretion — Git + KN reasons/watchers; NOTIF-D4 on real subjects.
  Body lists: Git + KN registered via 7.6 + 4.9 (no Notif code change recorded); NOTIF-D4 re-confirmed on real
  subjects (0 leak); the Git/KN leak rows greened; the floors named (Issues/Chat/CI NOTIF-P9; cross-cell
  NOTIF-P10). Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P9 — Consumer accretion: Issues SLA chains + Chat activity/mentions/explicit-first agents + CI HumanisedRef summaries

- **BAND.** M4.
- **ROADMAP MILESTONE.** N-M4 (planning/06-roadmaps/shared/notifications.md §1 "N-M4 — Consumer accretion:
  Issues SLA/escalation + Chat activity/mentions + explicit-first agents").
- **DEPENDS-ON.** NOTIF-P2 (define_notif_rule + the filtered views), NOTIF-P3 (humanise — the HumanisedRef
  resolves through it), NOTIF-P5 (escalation chains — Issues passes the real SLA chain). The M4 Issues prompts
  (reason set + the escalation chain definition + watcher fragment), the M4 Chat prompts (reason set +
  explicit-first dispatch boundary + watcher fragment), the M4 CI prompts (the CheckStatus.summary HumanisedRef,
  X-1 / 5.9). The M2 myelin-flow durable signal for the multi-day HITL wait (9.4). The index places this after
  Issues/Chat/CI ship their M4 registrations.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native — agents have inboxes; explicit-first dispatch — a casual @agent notifies,
    does not spawn a costed run); ../../external-insights/01-process-and-quality-doctrine.md §1 (the
    inverse-signal — registration must not get harder each time).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §1.4 (agents have inboxes; an HITL
    approval card is a Notif item reason=approval_requested at high priority), §3.3 (CI's CheckStatus.summary is
    a HumanisedRef = a (template_key, args) pair, resolves through humanise, never a raw string — X-1), §1.3 (the
    C-9 invariant — Chat "Activity/Mentions" is a filter not a store), §2.4 (Issues passes the frozen escalation
    chain definition).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.6 (define_notif_rule —
    Issues/Chat/CI register), 7.5 (Issues passes the escalation chain definition), 7.3 (humanise — the CI
    HumanisedRef registers here), 5.9 (the Git↔CI CheckStatus seam — CheckStatus.summary is a HumanisedRef),
    8.6 (explicit-first dispatch — a mention notifies, does not auto-spawn a costed run), 9.4 (the durable signal
    for multi-day HITL). Read 00-reconciliation-decisions.md X-1 (the CheckStatus seam + the HumanisedRef) +
    the explicit-first dispatch pinning.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M4 (the Issues/Chat/CI registration work + the
    CHAT-D5/CHAT-D17/ISS-D6 gate touch-points) + §4 (the M4 deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows CHAT-D5 (notify/unfurl a confidential artifact to a viewer lacking access → tombstone, title never
    present — Notif's humanise leak surface at the Chat seam), CHAT-D17 (casual @agent → notifies the agent's
    inbox, does NOT auto-spawn a costed run), ISS-D6 (SLA breach starts the escalation chain — Notif's
    chain-start integration with Issues).
- **DELIVERABLE (what to build + exactly where in the repo).** Mostly in Issues', Chat's, and CI's crates (the
  registrations), with the integration verified in myelin-notif:
  - Issues registers its reasons (assigned/blocked/needs-approval/overdue/sla/unblocked) via 7.6 + passes its
    REAL escalation chain definition to Notif (7.5, the frozen chain shape from NOTIF-P5) + declares its watcher
    fragment → Issues "My Work" becomes a real filtered view; SLA breaches start real escalation chains on the
    durable wheel (the NOTIF-P5 machinery, now driven by Issues' SLA policy).
  - Chat registers its reasons (mentioned/replied/thread_watched/approval_requested) via 7.6 + declares its
    watcher fragment → Chat "Activity/Mentions" becomes a real filtered view (a FILTER, not a store — the C-9
    invariant). The explicit-first agent dispatch boundary (8.6): a casual @agent mention posts a Notif item to
    the agent's inbox (reason=mentioned) but does NOT spawn a costed run (CHAT-D17 — Notif is the notify side of
    that boundary).
  - HITL approval cards: an agent HITL approval surfaced to a human is a Notif item with
    reason=approval_requested at high priority (refined §1.4); the card humanises via the ONE templating surface
    (NOTIF-P3 — action + risk + cost). Agents have inboxes too — the same model, no parallel system. The
    multi-day HITL wait uses the durable signal (9.4).
  - CI registers its status-summary reasons; the CheckStatus.summary (X-1 / 5.9) is a HumanisedRef =
    a (template_key, args) pair that resolves through humanise (NOTIF-P3) — CI registers its templates on the
    ONE surface, NEVER a raw string.
  - In myelin-notif: verify all M4 registrations land via the unchanged seams (the inverse-signal check again);
    the C-9 invariant holds for Chat "Activity" (a filter, not a store).
  - FLOOR named: cross-cell aggregation is still single-home (NOTIF-P10); the surge/erasure hardening is
    NOTIF-P11/P12. Name them.
- **CONTRACTS TO IMPLEMENT.** None NEW owned by Notif. Issues/Chat/CI own their 7.6 registrations, the 7.5 chain
  definition (Issues), the 5.9 CheckStatus.summary HumanisedRef (CI), the 8.6 explicit-first boundary (Chat);
  Notif consumes/evaluates them against the frozen 7.5/7.3/7.6/8.6 shapes.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - CHAT-D5 (CI): notify/unfurl a confidential artifact to a viewer lacking access → tombstone, the title never
    present (Notif's humanise leak surface at the Chat seam — re-confirms NOTIF-D4 at the Chat seam). Threshold:
    0 title/PII leak.
  - CHAT-D17 (CI): a casual @agent mention → notifies the agent's inbox (reason=mentioned), does NOT auto-spawn
    a costed run. Threshold: 0 auto-spawn from a casual mention.
  - ISS-D6 (CI): an SLA breach starts the escalation chain (Notif's chain-start integration with Issues — the
    NOTIF-P5 durable workflow driven by Issues' real SLA policy). Threshold: the chain starts and walks per the
    frozen shape.
- **TESTS (required).** Integration tests that Issues' SLA breach starts a real escalation chain; Chat's casual
  @agent notifies without spawning a run; CI's HumanisedRef summary resolves through humanise (never a raw
  string); the C-9 invariant holds for Chat "Activity". The drill-harness scenarios for CHAT-D5, CHAT-D17,
  ISS-D6. The CDC pairs for the Issues/Chat/CI sides of 7.6 / 7.5 / 5.9 (provider subsystem, consumer Notif).
- **DEFINITION OF DONE.** Issues/Chat/CI register via the unchanged seams; Issues' SLA breaches drive real
  escalation chains; Chat's casual @agent notifies without spawning a run (explicit-first); CI's HumanisedRef
  resolves through the ONE templating surface; CHAT-D5 / CHAT-D17 / ISS-D6 emit their dated green artifacts
  (PROVEN); CDC + lints + coverage scanner are green; the floors (cross-cell NOTIF-P10; surge/erasure
  NOTIF-P11/P12) are named; the work is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M4: consumer accretion — Issues SLA chains + Chat activity/explicit-first + CI
  HumanisedRef. Body lists: Issues/Chat/CI registered via 7.6/7.5/5.9; CHAT-D5 + CHAT-D17 + ISS-D6 greened; the
  C-9 Chat-filter invariant proven; the floors named (cross-cell NOTIF-P10; surge/erasure NOTIF-P11/P12).
  Branch first if on default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P10 — Cross-cell inbox aggregation (the multi-cell floor's follow-on; cell-local resolution)

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.1 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.1 — Cross-cell inbox
  aggregation").
- **DEPENDS-ON.** NOTIF-P2 (list_inbox), NOTIF-P3 (humanise — cell-local resolution). The M5 multi-cell control
  plane prompts that ship the CrossCellPointer bridge going live (12.6) and the multi-cell DSR fan-out iterating
  member_cells (10.4). The index places this after the M5 multi-cell tenancy prompts. The single-home-cell path
  has been complete since NOTIF-P1 (the §4 contracts were written cell-agnostic so this extends without a
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
    already-rendered already-permission-filtered projection or a tombstone, never raw rows, never PII that
    should stay in B; the DSR orchestrator iterates member_cells over the same bridge).
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
  The CDC for the Notif consumption of 12.6.
- **DEFINITION OF DONE.** A multi-cell recipient's inbox aggregates across cells via the PII-free pointer bridge
  with always-cell-local resolution (0 PII crosses cells); cell→cell migration loses 0 items; the
  GA-D8/CP-D7/CP-D8 inbox legs emit their dated green artifacts (PROVEN); CDC + lints + coverage scanner are
  green; the floor framing (single-home-cell remains the default; this is the follow-on) is recorded; the work
  is committed. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: cross-cell inbox aggregation (cell-local resolution). Body lists: contract
  12.6 consumed (the CrossCellPointer bridge, always-cell-local resolution); the GA-D8/CP-D7/CP-D8 inbox legs
  greened (0 PII crossing, 0 items lost on migration). Branch first if on default; do not push unless asked.
  End with the Co-Authored-By trailer.

---

### NOTIF-P11 — The 30×-agent-surge shed budget (the F6 surge family) + the EU-sovereign delivery provider follow-on

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.2 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.2 — The 30×-agent-surge
  shed budget + the EU-sovereign delivery follow-on + the erasure residual"; this prompt is the surge + delivery
  slice and ships NOTIF-D5; the erasure residual is NOTIF-P12).
- **DEPENDS-ON.** NOTIF-P1 (the router/consumer), NOTIF-P4 (the per-tenant in-flight caps), NOTIF-P7 (the
  DeliveryAdapter trait + mock — the real provider swaps in here). The M1 reserve/settle wallet (11.7) gating
  agent runs. The M2 agent runtime honouring 429 + Retry-After (ADR-16.3). The chosen EU provider + DPA (legal,
  parallel). The index places this after those.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (world-scalable; EU-sovereign — the concrete EU provider + DPA; name-your-floors — the
    shed budget is a named floor tuned by the drill, not a claimed-final number);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the 30× surge drill; the budget is
    asserted, observability is part of the pass — shed-counts + delivery-success signals);
    ../../external-insights/02-platform-substrate.md §5 (an unbounded lane is the cascade).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §5.2 (the fan-out scale axis + the
    agent-mention-storm shed budget: bounded consumer prefetch, bounded handler pool, per-tenant in-flight caps
    — one tenant's storm can't starve another's, bounded delivery-adapter concurrency a bulkhead per provider,
    per-recipient rate damping; the protected-human-lane shed order speculative → batch/CI → agent → human-last,
    ADR-16, concretised: a per-tenant agent-run in-flight cap reserve/settle refuses over-cap, humans never queue
    behind agent runs a separate lane, the agent-generated notification lane sheds first with 429 + Retry-After
    the agent runtime honours it ADR-16.3, a human's interactive inbox read is last-to-shed; these are named
    floors tuned by the drill T-5, not claimed-final numbers), §3.6 (the EU-sovereign delivery fabric FLOOR —
    the concrete production EU provider with its DPA/sub-processor posture is the deferred selection).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 1.11 (the protected-human-lane
    shed order + per-surface shed budgets named v1 floors — the agent-mention-storm row, OQ-K), 11.7
    (reserve/settle cost gate — refuses over-cap at dispatch), 7.8 (DeliveryAdapter — the real EU provider swaps
    in). Read 00-reconciliation-decisions.md OQ-K (the shed budgets) + the EU-sovereign delivery floor (§10).
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M5.2 (the surge budget + the EU provider
    follow-on work + the NOTIF-D5 gate) + §2 (the mock → EU-provider floor row) + §4 (the reserve/settle +
    429-honouring + EU-provider deps).
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
    delivery-adapter bulkhead per provider, per-recipient rate damping. FLOOR named: these are named floors
    tuned BY the drill (T-5), not claimed-final numbers — the concrete cap is the budget call NOTIF-D5 asserts
    against; record the chosen v1 numbers in the thresholds file.
  - The EU-sovereign delivery provider follow-on (refined §3.6/§10): swap the concrete production EU email/push
    provider into the DeliveryAdapter trait (NOTIF-P7) — region-aware, EU-preferring. [OPEN — LEGAL]: the
    engineering posture (trait + EU-preferring + RedactedMessage minimisation + crypto-shred + a
    provider-side-erasure-request hook) ships HERE; counsel/DPO ratifies the chosen provider + the DPA/
    sub-processor posture. We are not counsel — flag the provider selection + the residual statement for
    counsel/DPO sign-off (EI-01 §8 — a decision-shaped, irreversible-scope surface pauses for human sign-off).
- **CONTRACTS TO IMPLEMENT.** None NEW owned. Consumed: 1.11 the shed order + per-surface budget, 11.7
  reserve/settle, 7.8 DeliveryAdapter (the real EU provider implementation). Implement to the frozen shed order
  (human-last) — the human lane is always reserved.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - NOTIF-D5 (SCHED): a 30× agent-generated notification surge on one tenant → the human inbox-read lane holds
    within budget; the agent lane sheds (429 + Retry-After); cross-tenant unaffected; the delivery-adapter
    bulkhead bounds provider load. Telemetry green artifact: shed-counts + delivery-success measured and
    asserted against the §5.2 named shed budget (in the thresholds file). Threshold: human inbox-read latency
    within the named budget; cross-tenant impact 0. This is part of the master M5 F6 surge family — never
    weaken the budget to pass; a missed budget is a dated scorecard row.
- **TESTS (required).** Unit tests for the lane separation (a human read is served while the agent lane is
  shedding) and the per-tenant in-flight cap (over-cap → reserve/settle refuses). The drill-harness scenario for
  NOTIF-D5 (the 30× surge generator). A cross-tenant isolation test: tenant A's surge does not affect tenant B's
  human-read latency. State the cargo-mutants mutation-score floor for the shed-lane module and meet it. The CDC
  for the real DeliveryAdapter against 7.8.
- **DEFINITION OF DONE.** The agent-mention-storm shed budget is implemented (human-last, separate lane,
  per-tenant cap, bulkhead, 429+Retry-After); the v1 budget numbers are in the thresholds file; the real
  EU-sovereign provider is swapped into the DeliveryAdapter trait (with the [OPEN — LEGAL] flag for counsel/DPO
  sign-off recorded); NOTIF-D5 emits its dated green artifact (human lane in budget, agent sheds, cross-tenant
  0, PROVEN — or a dated scorecard row if the budget is not yet met); CDC + lints + coverage scanner are green;
  the floors (the budget numbers tuned by the drill; the provider awaits counsel ratification) are named; the
  work is committed. No budget weakened to manufacture a green.
- **COMMIT.** Header: P-<NNN> M5: 30×-agent-surge shed budget + EU-sovereign delivery provider. Body lists: the
  agent-mention-storm shed budget (human-last lane, per-tenant cap, bulkhead) + the real EU provider implemented;
  NOTIF-D5 greened (human lane in budget, agent sheds, cross-tenant 0, measured shed-counts); the budget numbers
  recorded in the thresholds file; the [OPEN — LEGAL] provider+DPA flagged for counsel/DPO. Branch first if on
  default; do not push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P12 — The erasure residual instanced (the X-7 posture for Notif)

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.2 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.2 — ... + the erasure
  residual"; this prompt is the erasure-residual slice and ships NOTIF-D6).
- **DEPENDS-ON.** NOTIF-P1 (the holder + references-not-payloads), NOTIF-P3 (humanise — an erased actor →
  [erased user]), NOTIF-P7 (the DeliveryAdapter — the off-cell-payload erasure-request hook), NOTIF-P11 (the EU
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
  - The erasure residual instanced (X-7 / 10.9) — the structural floor (built since NOTIF-P1, completed here):
    (1) references-not-payloads already tombstones an erased actor's appearance in every inbox item for free
    (every item humanises to [erased user] via NOTIF-P3); (2) per-subject-DEK crypto-shred (11.4) of any
    inline-PII delivery columns (the one place Notif emits free text outside the cell is an off-cell redacted
    summary); (3) restrict suppression — stop NEW routing/delivery for a restricted subject (and suppress
    indexing/agent-use/analytics/notification, 10.1); (4) a provider-side erasure request for the
    already-sent off-cell payload (the named sub-processor obligation — the hook built into the DeliveryAdapter
    in NOTIF-P7/P11). Notif does NOT restate the platform posture; the residual third-party free-text case is
    governed where the content lives (the authoring subsystem), referenced not duplicated.
  - The erase path contributes its receipt to the erasure ledger (10.8) so the DSAR fan-out (NOTIF-P13) can
    prove Notif's holder coverage.
  - FLOOR named: the provider-side erasure mechanism for an already-sent off-cell payload depends on the chosen
    EU provider's capability (NOTIF-P11) and is [OPEN — LEGAL] — counsel/DPO ratifies the one residual
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
  NOTIF-D6 emits its dated green artifact (0 recoverable PII, PROVEN); CDC + lints + coverage scanner are green;
  the floor ([OPEN — LEGAL] residual statement awaits counsel ratification; the structural floor ships
  regardless) is named; the work is committed. No threshold weakened; Notif restates no platform posture.
- **COMMIT.** Header: P-<NNN> M5: erasure residual instanced (the X-7 posture for Notif). Body lists: contract
  7.7 erase/restrict completed (the residual instanced); NOTIF-D6 greened (0 recoverable PII, erase-receipt
  sealed); the [OPEN — LEGAL] residual statement flagged for counsel/DPO. Branch first if on default; do not
  push unless asked. End with the Co-Authored-By trailer.

---

### NOTIF-P13 — The whole-system E2E wedge: Notif's legs (E2E-1 pane, E2E-2 HITL flagship, E2E-4 DSAR) + STOR-D2 at cell scale

- **BAND.** M5.
- **ROADMAP MILESTONE.** N-M5.3 (planning/06-roadmaps/shared/notifications.md §1 "N-M5.3 — The whole-system
  E2E wedge: Notif's legs").
- **DEPENDS-ON.** All prior Notif prompts NOTIF-P1..P12 (the full Notif surface). All five subsystems live (the
  M4 producer/consumer prompts). NOTIF-P10 (cross-cell) for the multi-cell DSAR leg; NOTIF-P11 (surge) +
  NOTIF-P12 (erasure) green. The M5 whole-system E2E harness prompts (the four chained-mutation scenarios
  against a full cell with mock agents). The M1 restore-verify at cell scale (11.5 / STOR-D2). The index places
  this last in Notif's set, after the M5 hardening prompts and the E2E harness.
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../VISION.md §3 (agent-native from the ground up — the E2E-2 flagship; prove the differentiator);
    ../../external-insights/01-process-and-quality-doctrine.md §3 (prove-it — the whole-system chained-mutation
    drills), §4 (actually try it — chain mutations end-to-end, not single handlers).
  - Architecture: ../05-refined-shared-systems-architecture/notifications.md §5.5 (the system-of-record tables
    prefs/on-call/templates are restore-verify gated), §3.3 (humanise per-viewer — the pane/HITL-card legs),
    §1.4 (the HITL approval card is a Notif item reason=approval_requested).
  - Contracts: ../05-refined-shared-systems-architecture/contract-index.md rows 7.3 (humanise — the pane/HITL/
    DSAR legs), 7.7 (the holder — Notif is one of the H1–H18 holders in the DSAR), 11.5 (restore-verify /
    STOR-D2 at cell scale on the system-of-record tables), 9.4 (the durable signal — the HITL withhold→approve
    in E2E-2). Read the testing-strategy E2E section.
  - Roadmap: planning/06-roadmaps/shared/notifications.md §1 N-M5.3 (Notif's E2E legs + the gate + the STOR-D2
    re-confirmation) + §4 (the M5 deps).
  - Drill source: ../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    §2 (the four chained-mutation E2E scenarios) + the E2E scorecard rows: E2E-1 PR context pane (humanise
    resolves the pane's notification/status strings per-viewer, 0 leak to the unauthorized viewer; the checks
    panel live-updates via the firehose, the shared per-ref cache busts), E2E-2 CI-fail → triage agent → issue →
    chat → fix-PR (the HITL approval card is a Notif item reason=approval_requested showing action+risk+cost; the
    agent's inbox notification on the casual mention does not spawn a run; the escalation/notify legs are
    exactly-once across a kill), E2E-4 DSAR fan-out (Notif is one of the H1–H18 holders; locate→erase over
    notification history contributes its receipt; post-erase locate = 0 recoverable PII; inbox items show
    [erased user]).
- **DELIVERABLE (what to build + exactly where in the repo).** In the whole-system E2E harness (the M5 wedge),
  Notif's legs wired against the full myelin-notif surface:
  - E2E-1 PR context pane: humanise (NOTIF-P3) resolves the pane's notification/status strings per-viewer with
    0 leak to the unauthorized viewer; the checks panel live-updates via the firehose (NOTIF-P6 — the shared
    per-ref cache busts).
  - E2E-2 CI-fail → triage agent → issue → chat → fix-PR (the flagship): the HITL approval card is a Notif item
    (reason=approval_requested, NOTIF-P2/P9) showing action + risk + cost (humanised, NOTIF-P3); the agent's
    inbox notification on the casual mention does NOT spawn a run (the explicit-first boundary, NOTIF-P9); the
    escalation/notify legs are exactly-once across a kill (NOTIF-P5).
  - E2E-4 DSAR fan-out: Notif is one of the H1–H18 holders; locate→erase over notification history (NOTIF-P12)
    contributes its receipt; post-erase locate = 0 recoverable PII; inbox items show [erased user].
  - STOR-D2 at cell scale re-confirmed for Notif's system-of-record tables (prefs/on-call/templates — RPO/RTO
    under world-scale load, 11.5). This is a PERMANENT gate (master §4) — say so; it re-runs on every
    store-touching change.
- **CONTRACTS TO IMPLEMENT.** None NEW owned (the E2E wedge exercises the full surface). Consumed/exercised: 7.3
  humanise, 7.7 the holder, 9.4 the durable signal, 11.5 restore-verify. Implement the E2E legs to assert the
  named green artifacts; do not re-implement Notif logic.
- **GATE / DRILLS (quantified; must be green to call this done).**
  - E2E-1 (SCHED): the pane-resolution trace + zero-leak = 0 + the per-viewer diff for Notif's notification/
    status strings. Threshold: 0 leak to the unauthorized viewer.
  - E2E-2 (SCHED): the HITL withhold→approve→apply ledger (the Notif approval card is the notify side);
    exactly-once across a kill; the casual @agent mention → 0 auto-spawn. Threshold: 0 mutation pre-approval, 1
    apply, exactly-once.
  - E2E-4 (SCHED): the H1–H18 coverage receipt set INCLUDING Notif; post-erase locate = 0 recoverable PII;
    inbox items show [erased user]. Threshold: 0 holders missed, 0 recoverable PII.
  - STOR-D2 at cell scale (SCHED, PERMANENT gate): restore Notif's system-of-record tables under world-scale
    load → RPO ≤ 5 min / RTO ≤ 1h-tenant / 4h-cell, 0 loss. Threshold: the master §2 M1 STOR-D2 thresholds —
    never weakened.
- **TESTS (required).** The E2E harness scenarios for E2E-1, E2E-2, E2E-4 (Notif's legs), each emitting its
  named green artifact. A restore-verify scenario for Notif's system-of-record tables at cell scale (STOR-D2).
  These chain mutations end-to-end across the full cell with mock agents (EI-01 §4) — not single-handler tests.
  No new mandatory-core module (the wedge exercises existing modules); confirm the prior mutation floors still
  hold under the E2E exercise.
- **DEFINITION OF DONE.** Notif's E2E legs (E2E-1 pane, E2E-2 HITL flagship, E2E-4 DSAR) emit their dated green
  artifacts (0 leak; exactly-once HITL+notify; 0 holders missed + 0 recoverable PII, PROVEN); STOR-D2 at cell
  scale is re-confirmed for Notif's system-of-record tables (the permanent gate); lints + coverage scanner are
  green; any untested-but-named surface is honestly recorded; the work is committed. This is the last Notif
  prompt — Notif's roadmap is fully covered. No threshold weakened.
- **COMMIT.** Header: P-<NNN> M5: Notif E2E wedge legs (E2E-1/E2E-2/E2E-4) + STOR-D2 at cell scale. Body lists:
  the E2E-1/E2E-2/E2E-4 Notif legs greened (0 leak; exactly-once HITL+notify; H1–H18 receipt incl. Notif +
  post-erase locate = 0); STOR-D2 re-confirmed at cell scale (permanent gate, measured RPO/RTO). Branch first if
  on default; do not push unless asked. End with the Co-Authored-By trailer.

---

## Coverage matrix (every notifications roadmap milestone → its prompt(s))

| Roadmap milestone (planning/06-roadmaps/shared/notifications.md) | Band | Prompt(s) | Primary drills greened |
|---|---|---|---|
| N-M2.0 — holder + outbox + Signal-consumer skeleton | M2 | NOTIF-P1 | NOTIF-D10; harness self-test; contract-coverage scanner |
| N-M2.1 — read surface + humanise + ranking | M2 | NOTIF-P2 (list_inbox + ranking + define_notif_rule seam), NOTIF-P3 (humanise + prefs) | NOTIF-D1; NOTIF-D4 |
| N-M2.2 — storm-control + write/read fanout split | M2 | NOTIF-P4 | NOTIF-D2 |
| N-M2.3 — escalation + live transport + delivery idempotency | M2 | NOTIF-P5 (escalation), NOTIF-P6 (inbox watch), NOTIF-P7 (delivery + reindex) | NOTIF-D7, NOTIF-D8 (P5); D-N11 resume leg (P6); NOTIF-D9, NOTIF-D3 (P7) |
| N-M3 — Git + Knowledge register reasons + watchers | M3 | NOTIF-P8 | NOTIF-D4 on real subjects; GIT-D8, KN-D5/D13 (Notif leak surface) |
| N-M4 — Issues + Chat + CI register | M4 | NOTIF-P9 | CHAT-D5, CHAT-D17, ISS-D6 |
| N-M5.1 — cross-cell inbox aggregation | M5 | NOTIF-P10 | GA-D8 / CP-D7 / CP-D8 (inbox legs) |
| N-M5.2 — surge shed budget + EU delivery + erasure residual | M5 | NOTIF-P11 (surge + EU provider), NOTIF-P12 (erasure residual) | NOTIF-D5 (P11); NOTIF-D6 (P12) |
| N-M5.3 — the E2E wedge legs | M5 | NOTIF-P13 | E2E-1, E2E-2, E2E-4; STOR-D2 at cell scale |

**Notif's M2-exit obligations (master §2 M2 exit gate):** NOTIF-D4 (NOTIF-P3) + NOTIF-D7 (NOTIF-P5) — both
named in the master M2 exit gate; the band also requires the hard AG-D4 sandbox-escape GATE (owned by the agent
fabric) to be green before Notif is "done in M2" (the gate invariant, EI-01 §2). **Permanent gate re-run by
Notif:** STOR-D2 at cell scale (NOTIF-P13), re-run on every store-touching change.

**Floors and their follow-on prompts (name-your-floors, VISION §3 / EI-04 §4):** deterministic-v1 ranking
(NOTIF-P2) → ML ranking (post-M5, measured); stubbed default reason set (NOTIF-P2) → per-subsystem enumerations
(NOTIF-P8/P9); synthetic-watcher read-fanout (NOTIF-P4) → real watcher fragments (NOTIF-P8/P9); mock
DeliveryAdapter (NOTIF-P7) → real EU-sovereign provider (NOTIF-P11); single-home-cell inbox (NOTIF-P1) →
cross-cell aggregation (NOTIF-P10); by-reference erasure residual / structural crypto-shred floor (NOTIF-P1) →
provider-side erasure + counsel ratification (NOTIF-P12). Each floor pair is visible here so the gap is never
invisible.
