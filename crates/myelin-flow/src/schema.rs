//! The myelin-flow OLTP row types — the SIX tables of the data model (architecture §3.1..§3.6),
//! `(tenant, region)`-first and carrying the `#[personal_data(...)]` classification tags
//! (contract 10.2) on every PII-bearing column. P-FLOW-01 / P-197, M2.
//!
//! **Owning architecture doc:** `durable-workflow.md` §3 (the data model — `workflow_run`,
//! `wf_history`, `wf_timer`, `wf_signal`, `wf_activity_attempt`, `wf_definition`). The exact column
//! shapes are Phase-3 §3.1..§3.6 (cited-not-restated by refined §3).
//!
//! **These are the frozen-shape tag-carriers + the column lists the migrations build.** Every
//! tenant row LEADS with `(tenant, region)` (12.1, ADR-11) — the partition key from the verified
//! token, never the path (the residency-pin lint floor). The load-bearing invariants:
//!
//! - **`input` / `result` / `payload` are references-not-payloads** (architecture §3.1/§3.2/§3.4):
//!   a workflow about a PR carries the PR's [`ArtifactRef`], never the PR body. Personal data stays
//!   in the owning subsystem's erasable store, so erasing a person rarely touches the workflow
//!   (§4.8). This is what makes the ONE erasure posture (X-7) apply "for free": the engine stores
//!   refs, not payloads, so erasing a person tombstones their appearance with NO mutation.
//! - **The RARE inline-PII result/payload is envelope-encrypted** (`result_key_ref` on `wf_history`,
//!   `payload_key_ref` on `wf_signal`): the ONLY PII locators in the engine. They name the
//!   per-subject DEK so erasure = crypto-shred (§4.8; ADR-12.3) — tagged
//!   `Content / CryptoShred(subject_dek)`.
//! - **`command_id` is deterministic from the workflow position** (the replay-match key) and the
//!   `UNIQUE(tenant, run_id, command_id)` makes journaling idempotent (§3.2).
//! - **The `wf_signal` PK `(tenant, run_id, signal_name, idem_key)`** makes signal delivery
//!   at-least-once-safe (a re-posted approval is a no-op) — the per-effect idempotency anchor (§6.4).
//!
//! The PII-bearing columns are tagged with the canonical multi-line six-tag form
//! (`category | role | basis | retention | erasure | subject_locator`, gdpr §2.1). myelin-flow is
//! almost PII-FREE by construction (references-not-payloads); the ONLY tagged columns are the
//! inline-PII envelope key refs (the crypto-shred levers).
//!
//! ## Floor named (the WRITERS land later; this prompt ships the SCHEMA only)
//! The rows are written by later prompts: `workflow_run`/`wf_history` by the WfCtx core +
//! journal/outbox co-commit (**P-FLOW-04**); the lease-dispatch columns by the replay/lease loop
//! (**P-FLOW-05**); `wf_signal` by durable signals (**P-FLOW-09**); `wf_timer` by durable timers
//! (**P-FLOW-13**); `wf_activity_attempt` by activity execution (**P-FLOW-04..05**); `wf_definition`
//! by the definition registry (**P-FLOW-06**). This module is the schema shape + the classification
//! tags ONLY — an empty journal is not a working engine. There is **no mandatory-core algorithm
//! module** here (schema only), so there is no mutation-score floor on this prompt (stated
//! explicitly per the template's TESTS field).

use myelin_gdpr::PersonalData;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

/// The `workflow_run` row (architecture §3.1) — the run lifecycle + the durable handle;
/// `(tenant, region)`-first. The state, the replay cursor, the owned budget, the causality, the
/// lease-dispatch handles. `input` is references-not-payloads. Carries NO PII column: the workflow
/// is about a ref, never a body.
#[derive(PersonalData)]
pub struct WorkflowRunRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag (the residency-pin floor).
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the RESIDENCY PIN (architecture §3). No tag.
    pub region: Region,
    /// the ULID-ordered durable run handle — an opaque id, not PII.
    pub run_id: String,
    /// the registered definition name (e.g. `agent.run`, `ci.pipeline`) — a wf-type token, not PII.
    pub wf_type: String,
    /// the definition version PINNED at start (§4.6) so a deploy cannot diverge an in-flight run.
    pub wf_version: i32,
    /// the run input as **`ArtifactRef`s, never PII bodies** (the references-not-payloads invariant,
    /// §3.1). Carried as the structured `input` jsonb ref-array; the payload lives in the owning
    /// subsystem's erasable store. Refs → erasure-for-free.
    pub input: Vec<ArtifactRef>,
    /// the ONE lifecycle state column (running|waiting|completed|failed|nondeterministic|terminated)
    /// — the durable run state. Not PII.
    pub state: String,
    /// the highest applied history seq (the replay short-circuit floor, §3.1) — not PII.
    pub cursor: i64,
    /// the owned `RunBudget` as JSON (integer minor-units, never floats — §5.1) — config, no PII.
    pub budget_json: Option<String>,
    /// the causal ROOT (BUS-5) that carries to every emitted event — an opaque correlation id, no PII.
    pub correlation_id: String,
    /// the event that STARTED this workflow (BUS-5) — an opaque event id, nullable, no PII.
    pub causation_id: Option<String>,
    /// the human session/action that caused this run (distinct from causation, BUS-5) — an opaque
    /// session ref, nullable, no PII (it is a session handle, not an identity).
    pub caused_by: Option<String>,
    /// the inherited loop-cap counter (AG-6) — an integer, not PII.
    pub depth: i32,
    /// the worker-shard key = hash(run_id) % N (§7.2) — a shard index, not PII.
    pub partition: i16,
    /// the worker currently driving this run (§4.7) — an opaque worker id, nullable, no PII.
    pub lease_owner: Option<String>,
    /// the lease TTL; expiry → another worker may steal (crash recovery, §4.7) — a timestamp, no PII.
    pub lease_expires: Option<String>,
}

/// The `wf_history` row (architecture §3.2) — the append-only journal, the SOURCE OF TRUTH;
/// `(tenant, region)`-first. `command_id` deterministic from the workflow position (the replay-match
/// key); the `UNIQUE(tenant, run_id, command_id)` makes journaling idempotent. `result` is
/// references-not-payloads; `result_key_ref` is the ONLY PII locator — the inline-PII crypto-shred
/// envelope key (§4.8).
#[derive(PersonalData, Clone, Debug, PartialEq)]
pub struct WfHistoryRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the run this journal row belongs to — an opaque run id, no PII.
    pub run_id: String,
    /// the per-run monotonic replay-order seq (§3.2) — not PII.
    pub seq: i64,
    /// the history kind (activity_scheduled|activity_completed|…|side_marker) — a taxonomy token,
    /// not PII.
    pub kind: String,
    /// the DETERMINISTIC command id from the workflow position — the replay-match key (§3.2). An
    /// opaque deterministic id, not PII.
    pub command_id: String,
    /// the activity result / signal payload / fired-timer marker as **refs, never PII bodies**
    /// (§3.2). Carried as the structured `result` jsonb ref-array; the rare inline-PII result is
    /// crypto-shred-erased via `result_key_ref`. Refs → erasure-for-free.
    pub result: Option<Vec<ArtifactRef>>,
    /// the envelope-encryption key id IF a result must carry inline PII (the RARE case, §3.2/§4.8) —
    /// the ONLY PII locator in `wf_history`. It NAMES the per-subject DEK; the bytes hold only the
    /// key ref, so erasing a subject crypto-shreds the result (ADR-12.3). Tagged
    /// `Content / CryptoShred(subject_dek)` — the inline-PII history rows are GD-4 erased by
    /// destroying the per-subject key, never by a row mutation.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "result_key_ref",
    )]
    pub result_key_ref: Option<String>,
}

/// The `wf_timer` row (architecture §3.3) — the durable timer (powers SC-11: millions of timers);
/// `(tenant, region)`-first. `bucket = epoch_minute(fire_at)` + the partial index on `NOT fired` is
/// the world-scale move. Carries NO PII column.
#[derive(PersonalData)]
pub struct WfTimerRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the stable opaque timer id — not PII.
    pub timer_id: String,
    /// the workflow to wake (NULL for a bare SLA timer, §3.3) — an opaque run id, nullable, no PII.
    pub run_id: Option<String>,
    /// the `wf_history` command this timer satisfies — an opaque command id, no PII.
    pub command_id: String,
    /// the durable deadline (RFC-3339 UTC, §5.1) — a timestamp, not PII.
    pub fire_at: String,
    /// the coarse time bucket = epoch_minute(fire_at) — the SC-11 scan index (§3.3). Not PII.
    pub bucket: i32,
    /// whether the timer has fired — the partial-index pivot. Not PII.
    pub fired: bool,
    /// = the run's partition (co-located dispatch, §3.3) — a shard index, not PII.
    pub partition: i16,
}

/// The `wf_signal` row (architecture §3.4) — durably-BUFFERED inbound signals (powers multi-day HITL
/// waits); `(tenant, region)`-first. The PK `(tenant, run_id, signal_name, idem_key)` makes signal
/// delivery at-least-once-safe (the per-effect idempotency anchor, §6.4). `payload` is references-
/// not-payloads; `payload_key_ref` is the ONLY PII locator — the inline-PII crypto-shred envelope
/// key.
#[derive(PersonalData)]
pub struct WfSignalRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the run this signal is buffered for — an opaque run id, no PII.
    pub run_id: String,
    /// the signal name (e.g. `approval`, `cancel`, `job.done`) — a taxonomy token, not PII.
    pub signal_name: String,
    /// the caller-supplied idempotency key — dedups a re-delivered signal (§3.4 / §6.4). The PK's
    /// idem dimension. Not PII (a derived dedup key, often = the deterministic `idem_token`).
    pub idem_key: String,
    /// the signal body as **refs, never PII bodies** (§3.4, e.g. `{approved:true, by:<ArtifactRef>}`).
    /// Carried as the structured `payload` jsonb; the rare inline-PII payload is crypto-shred-erased
    /// via `payload_key_ref`. Refs → erasure-for-free.
    pub payload: Vec<ArtifactRef>,
    /// the crypto-shred key id IF the payload carries inline PII (the RARE case, §3.4) — the ONLY
    /// PII locator in `wf_signal`. It NAMES the per-subject DEK; erasing a subject crypto-shreds the
    /// signal payload. Tagged `Content / CryptoShred(subject_dek)`.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "payload_key_ref",
    )]
    pub payload_key_ref: Option<String>,
    /// the `wf_history` seq that consumed it (NULL = buffered, unconsumed, §3.4) — not PII.
    pub consumed_seq: Option<i64>,
}

/// The `wf_activity_attempt` row (architecture §3.5) — the idempotency ledger; `(tenant, region)`-
/// first. `idem_token` bridges to BUS-2 so a retried emit is broker-deduped (§3.5). Carries NO PII
/// column.
#[derive(PersonalData, Clone, Debug, PartialEq)]
pub struct WfActivityAttemptRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the run this attempt belongs to — an opaque run id, no PII.
    pub run_id: String,
    /// the `wf_history` command this attempt executes — an opaque command id, no PII.
    pub command_id: String,
    /// the attempt counter (1, 2, … on retry) — not PII.
    pub attempt: i32,
    /// the BUS-2 dedup bridge token passed to the activity so ITS downstream write/emit is
    /// dedup-keyed (§3.5) — an opaque derived token, not PII.
    pub idem_token: String,
    /// the attempt state (scheduled|running|succeeded|failed|retrying) — not PII.
    pub state: String,
    /// the failure reason (if failed) — a machine error string (no subject data; the activity's PII
    /// stays in its own erasable store, references-not-payloads). Not PII.
    pub error: Option<String>,
    /// when the attempt started — a timestamp, not PII.
    pub started_at: Option<String>,
    /// when the attempt ended — a timestamp, not PII.
    pub ended_at: Option<String>,
}

/// The `wf_definition` row (architecture §3.6) — the GLOBAL versioned definition registry.
/// Definitions are CODE (deterministic Rust functions registered at boot), NOT tenant data: NO
/// `tenant`/`region`/PII column, PK `(wf_type, version)`. A run pins to its `wf_version` at start
/// (§4.6) so a deploy cannot diverge an in-flight run. The ONE non-tenant row type here, by
/// construction.
#[derive(PersonalData)]
pub struct WfDefinitionRow {
    /// the registered definition name — a wf-type token, not PII.
    pub wf_type: String,
    /// the definition version — not PII.
    pub version: i32,
    /// the content hash of the compiled definition (drift detection, §3.6) — a code hash, not PII.
    pub code_hash: String,
    /// the registry status (active|draining|retired) — not PII.
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> TenantId {
        TenantId::from_token("acme")
    }
    fn r() -> Region {
        Region::new("fr-par")
    }

    /// All six row types compile with their `#[derive(PersonalData)]` + (where present)
    /// `#[personal_data(...)]` tags (contract 10.2) and the tenant rows lead with `(tenant, region)`
    /// (12.1). The structs being constructable with their fields readable proves the no-op derive
    /// left the items unchanged — and that the flow store CAN tag its inline-PII key-ref columns
    /// today against the frozen classification (it will not compile against drift later). `input` /
    /// `result` / `payload` carry `ArtifactRef`s (never strings) — the references-not-payloads
    /// invariant at the type level. `wf_definition` is the global registry (NO tenant/PII).
    #[test]
    fn the_six_tables_compile_with_tags_and_tenant_first_keys() {
        let run = WorkflowRunRow {
            tenant: t(),
            region: r(),
            run_id: "01J-run".into(),
            wf_type: "agent.run".into(),
            wf_version: 1,
            input: vec![ArtifactRef("myelin://acme/git/pr/PR-1".into())],
            state: "running".into(),
            cursor: 0,
            budget_json: Some("{\"minor_units\":10000}".into()),
            correlation_id: "corr-1".into(),
            causation_id: Some("evt-1".into()),
            caused_by: Some("sess-1".into()),
            depth: 0,
            partition: 3,
            lease_owner: None,
            lease_expires: None,
        };
        // (tenant, region) FIRST — the partition key, from the verified token.
        assert_eq!(run.tenant, t());
        assert_eq!(run.region, r());
        // input is refs, never a PII body (the references-not-payloads invariant at the type level).
        let _input: &Vec<ArtifactRef> = &run.input;
        assert_eq!(run.state, "running"); // the ONE lifecycle state column.
        assert_eq!(run.cursor, 0); // the replay short-circuit floor starts at 0.

        let history = WfHistoryRow {
            tenant: t(),
            region: r(),
            run_id: "01J-run".into(),
            seq: 1,
            kind: "activity_completed".into(),
            command_id: "agent.run:0".into(),
            result: Some(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())]),
            // the RARE inline-PII case: the result names a per-subject DEK (crypto-shred lever).
            result_key_ref: Some("kms://acme/subject/u1".into()),
        };
        let _result: &Option<Vec<ArtifactRef>> = &history.result; // refs, never a PII body.
        assert!(history.result_key_ref.is_some()); // the inline-PII crypto-shred locator.
        assert_eq!(history.command_id, "agent.run:0"); // deterministic from the workflow position.

        let timer = WfTimerRow {
            tenant: t(),
            region: r(),
            timer_id: "tmr-1".into(),
            run_id: Some("01J-run".into()),
            command_id: "agent.run:1".into(),
            fire_at: "2026-07-21T00:00:00Z".into(),
            bucket: 29_000_000,
            fired: false,
            partition: 3,
        };
        assert!(!timer.fired); // the partial-index pivot (the unfired bucket the wheel scans).

        let signal = WfSignalRow {
            tenant: t(),
            region: r(),
            run_id: "01J-run".into(),
            signal_name: "job.done".into(),
            idem_key: "tok-1".into(),
            payload: vec![ArtifactRef("myelin://acme/ci/job/J1".into())],
            payload_key_ref: None,
            consumed_seq: None,
        };
        // the per-effect idempotency anchor: (signal_name, idem_key) dedups a re-delivered signal.
        assert_eq!(signal.idem_key, "tok-1");
        let _payload: &Vec<ArtifactRef> = &signal.payload; // refs, never a PII body.

        let attempt = WfActivityAttemptRow {
            tenant: t(),
            region: r(),
            run_id: "01J-run".into(),
            command_id: "agent.run:0".into(),
            attempt: 1,
            idem_token: "tok-1".into(),
            state: "succeeded".into(),
            error: None,
            started_at: Some("2026-06-21T00:00:00Z".into()),
            ended_at: Some("2026-06-21T00:00:01Z".into()),
        };
        assert_eq!(attempt.idem_token, "tok-1"); // the BUS-2 dedup bridge.

        let def = WfDefinitionRow {
            wf_type: "agent.run".into(),
            version: 1,
            code_hash: "blake3:deadbeef".into(),
            status: "active".into(),
        };
        // the global registry: definitions are code, pinned at start (§4.6) — no tenant/PII.
        assert_eq!(def.version, 1);
        assert_eq!(def.status, "active");
    }
}
