//! # Dogfood: the restore-verify gate runs on Myelin's own commits + the truth-up pass (P-ST-37)
//!
//! **Prompt:** P-ST-37 → global **P-506** (M6). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §2 "S-M6" (*the restore-verify CI
//! job runs on the platform's own commits — Myelin's own monorepo, CI logs, issues, docs are now
//! real tenant data under the same backup/restore/crypto-shred machinery; the every-incident-adds-a-
//! drill loop files a Myelin issue + a reproducing storage drill for any storage incident*) and §7
//! (the restore-verify gate). **Roadmap:** `06-roadmaps/shared/storage.md` §2 "S-M6". **Doctrine:**
//! `external-insights/01-process-and-quality-doctrine.md` §1 (the code wins over the docs — the
//! truth-up pass re-syncs every PROVEN row to a dated green artifact; a claim that outlives its
//! verification misleads the next agent), §3 (prove-it on real data — the team's own data is real
//! tenant data; every incident adds a drill).
//!
//! ## What this module OWNS (new) vs REUSES (coherence, EI-01 §7)
//! This prompt **wires** the existing restore-verify gate (11.5 — [`crate::restore_verify`], the
//! permanent gate P-061) onto Myelin's OWN data + confirms the gate invariant. It does **NOT**
//! re-define the gate, the [`RestoreVerifyGate`], the [`RestoreTarget`], or the cross-seam machinery
//! — it is a CALLER that builds [`GateInputs`] from the platform's own stores and runs the SAME gate
//! a managed-fleet store runs. Three genuinely-new pieces ship here:
//!
//! 1. **[`DogfoodCorpus`] — Myelin's own stores AS real tenant data.** The platform's own monorepo
//!    commits, CI logs, issues, and docs, modeled as the OLTP rows / content-addressed objects /
//!    source-log a backup of the self-host cell holds. It builds [`GateInputs`] over the SAME shapes
//!    a tenant's data uses — there is no dogfood-only data path, only dogfood-class CONTENT (the
//!    dogfood loop's whole point: the team's own data is real tenant data, not a special case).
//! 2. **[`StorageIncident`] / [`IncidentDrillTicket`] — the every-incident-adds-a-drill loop.** Any
//!    storage incident discovered during dogfooding produces a PII-FREE Myelin issue draft + a named
//!    reproducing drill descriptor; the drill descriptor is what an integration test hands to the
//!    harness [`DrillRegistry`] (the T-3 `register_drill` hook) so the repro re-runs forever. The
//!    registry lives ABOVE the substrate (the harness sits above storage in the DAG), so the WIRING
//!    into the registry is the dogfood integration test's job — storage owns the PII-free TICKET.
//! 3. **[`TruthUpPass`] / [`ProvenRow`] — the truth-up pass.** Enumerates every PROVEN Storage row
//!    (the STOR-D* family, D-S11/D-S12/D-S13, the floor follow-ons) and asserts each rests on a DATED
//!    green artifact. A row WITHOUT one is a LOUD failure ([`TruthUpVerdict::Red`]), never a silent
//!    pass (code-wins-over-docs, EI-01 §1: a claim that outlives its verification misleads the next
//!    agent). The pass is the build-layer realisation of the gate invariant — no earlier-band storage
//!    gate is red.
//!
//! ## DEVIATION / FLOOR — the "file a Myelin issue" is a PII-free TICKET, not a cross-crate call (EI-01 §1)
//! `myelin-storage` sits BELOW the subsystem crates in the DAG (00 §2.9) — it MUST NOT depend on
//! `myelin-issues` (that edge is the wrong direction and would not compile). So the every-incident
//! loop's "files a Myelin issue" is modeled as an [`IncidentIssueDraft`] — the PII-free issue BODY a
//! storage incident hands UP to the Issues subsystem (the same posture as the holder/erase seams that
//! emit a PII-free record the consumer subsystem files). The real `Issues::create` call is the
//! consuming subsystem's job; storage owns the structural ticket. The reproducing drill is a
//! [`DrillScenario`]-shaped descriptor the dogfood integration test registers into the harness
//! [`DrillRegistry`] (the harness is a dev-dependency — the SAME posture as every storage drill).
//!
//! ## FLOORS NAMED (the prompt's DEFINITION OF DONE)
//! - **The real self-host CI runner** (a `cargo`/CI invocation that calls
//!   [`run_restore_verify_on_dogfood`] on every Myelin commit) is the dogfood-loop INFRASTRUCTURE —
//!   the gate LOGIC + the dated artifact ship now and re-run as a `cargo test` drill until the CI
//!   graph wires them. The real `pg_restore` + object-store backing is the P-S12/P-S15 floor (named
//!   in [`crate::restore_verify`]); the gate's SHAPE does not change when it lands — the dogfood
//!   corpus simply gets POPULATED off the live self-host stores. The dogfood integration drill against
//!   the live docker-compose stack rides the existing `stage3_drills` restore-verify row (the live
//!   `STOR-D-RESTORE` infra-gate row).

use std::collections::BTreeMap;

use myelin_tenancy::TenantId;

use crate::backup::{ContinuousArchiver, WalOffset, WalSegment};
use crate::blob::ContentHash;
use crate::kms::{KekId, KeyClass, KmsEngine};
use crate::restore::{SourceLog, WalRow};
use crate::restore_verify::{
    ErasureLedger, GateInputs, GreenArtifact, RestoreVerifyGate, RestoredObject,
};
use myelin_tenancy::Region;

// ───────────────────────────── the dogfood corpus (Myelin's own stores as real tenant data) ─────────────────────────────

/// Which of the platform's OWN stores a dogfood record belongs to. The dogfood loop's premise: the
/// team's own monorepo commits, CI logs, issues, and docs are **real tenant data** under the SAME
/// backup/restore/crypto-shred machinery — so each is restored + verified by the SAME gate. This
/// discriminant exists only so the green artifact can name WHICH platform store each verified row
/// came from (observability is part of the pass, EI-01 §3); it is NOT a data-path fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DogfoodStore {
    /// Myelin's own monorepo — a git commit object (the platform hosting ITS OWN source, M3 dogfood).
    Monorepo,
    /// Myelin's own CI logs — a CI step log segment (the platform running ITS OWN CI, M4 dogfood).
    CiLog,
    /// Myelin's own issues — an issue body (the platform tracking ITS OWN work, M4 dogfood).
    Issue,
    /// Myelin's own docs — a knowledge/doc block (the platform documenting ITSELF, M3 dogfood).
    Doc,
}

impl DogfoodStore {
    /// A short stable label for the green-artifact row (which platform store the record came from).
    pub fn label(self) -> &'static str {
        match self {
            DogfoodStore::Monorepo => "monorepo",
            DogfoodStore::CiLog => "ci-log",
            DogfoodStore::Issue => "issue",
            DogfoodStore::Doc => "doc",
        }
    }

    /// Every dogfood store class (so the corpus / the truth-up pass can assert all four are present —
    /// the dogfood loop covers Myelin's whole own data set, not a subset).
    pub const ALL: [DogfoodStore; 4] = [
        DogfoodStore::Monorepo,
        DogfoodStore::CiLog,
        DogfoodStore::Issue,
        DogfoodStore::Doc,
    ];
}

/// One record of Myelin's OWN data committed to a store (a monorepo commit, a CI log segment, an
/// issue body, a doc block). It is content-addressed (the BYTES land in the object tier, the OLTP
/// ROW references them at a WAL offset) EXACTLY as a tenant's record is — the dogfood premise. The
/// `row_id` is the durable source-log key the derived stores reindex from; the `bytes` are the
/// content the restore must bring back intact (checksum parity).
#[derive(Clone, Debug)]
pub struct DogfoodRecord {
    /// Which of the platform's own stores this record belongs to (for the artifact; not a fork).
    pub store: DogfoodStore,
    /// The durable source-log row id (the cross-seam key — the same handle a tenant row carries).
    pub row_id: String,
    /// The WAL offset the row was written at (the consistency-point coordinate).
    pub written_at: WalOffset,
    /// The content bytes (the monorepo commit / CI log / issue body / doc block). Content-addressed
    /// into the object tier; the restore re-hashes them for checksum parity.
    pub bytes: Vec<u8>,
}

impl DogfoodRecord {
    /// A dogfood record of `store` at `row_id` / `written_at` carrying `bytes`.
    pub fn new(
        store: DogfoodStore,
        row_id: impl Into<String>,
        written_at: WalOffset,
        bytes: impl Into<Vec<u8>>,
    ) -> DogfoodRecord {
        DogfoodRecord {
            store,
            row_id: row_id.into(),
            written_at,
            bytes: bytes.into(),
        }
    }
}

/// **The dogfood corpus — Myelin's OWN stores modeled as real tenant data (S-M6).** Holds the
/// platform's own monorepo commits / CI logs / issues / docs, the self-host tenant they belong to,
/// and the region the install pins to. It builds [`GateInputs`] over the SAME shapes a tenant's data
/// uses (there is no dogfood-only restore path) so [`run_restore_verify_on_dogfood`] runs the SAME
/// [`RestoreVerifyGate`] on the platform's own data the prompt requires.
///
/// The corpus is content-addressed: each [`DogfoodRecord`]'s bytes land in the object tier under
/// their BLAKE3 address, and the OLTP row references that address — so the gate's checksum-parity +
/// cross-seam legs verify Myelin's own data exactly as they verify a tenant's.
#[derive(Clone, Debug)]
pub struct DogfoodCorpus {
    /// The self-host tenant the platform's own data belongs to (the dogfood tenant — real tenant
    /// data, PII-free opaque id).
    tenant: TenantId,
    /// The install's region (the customer's / the team's own region — every store pins here).
    region: Region,
    /// The platform's own records, in write order.
    records: Vec<DogfoodRecord>,
}

impl DogfoodCorpus {
    /// **Stand up a dogfood corpus** for the self-host `tenant` in `region`. Empty until records of
    /// the platform's own data are committed via [`Self::commit`].
    pub fn new(tenant: TenantId, region: Region) -> DogfoodCorpus {
        DogfoodCorpus {
            tenant,
            region,
            records: Vec::new(),
        }
    }

    /// Commit one record of Myelin's own data (a monorepo commit / CI log / issue / doc) into the
    /// corpus — it lands in the object tier + the OLTP row + the source log exactly as a tenant's
    /// record does.
    pub fn commit(&mut self, record: DogfoodRecord) -> &mut Self {
        self.records.push(record);
        self
    }

    /// Commit a record in one call (builder-style sugar over [`Self::commit`]).
    pub fn commit_record(
        &mut self,
        store: DogfoodStore,
        row_id: impl Into<String>,
        written_at: WalOffset,
        bytes: impl Into<Vec<u8>>,
    ) -> &mut Self {
        self.commit(DogfoodRecord::new(store, row_id, written_at, bytes))
    }

    /// The self-host tenant the corpus belongs to.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// The install's region.
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// The committed records (the platform's own data).
    pub fn records(&self) -> &[DogfoodRecord] {
        &self.records
    }

    /// The highest WAL offset any record was written at (the latest consistency point a backup of the
    /// corpus can restore to — the dogfood gate restores to THIS point).
    pub fn latest_offset(&self) -> WalOffset {
        self.records.iter().map(|r| r.written_at).max().unwrap_or(0)
    }

    /// Which dogfood stores the corpus actually carries (so a caller can assert it covers Myelin's
    /// whole own data set — all four of [`DogfoodStore::ALL`]).
    pub fn stores_present(&self) -> std::collections::BTreeSet<DogfoodStore> {
        self.records.iter().map(|r| r.store).collect()
    }

    /// The restored objects (the platform's own bytes, content-addressed) — the object tier a backup
    /// of the corpus brings back. Each is `integral` (its address IS the BLAKE3 hash of its bytes), so
    /// a whole restore checksum-parity-verifies.
    fn restored_objects(&self) -> Vec<RestoredObject> {
        self.records
            .iter()
            .map(|r| RestoredObject::integral(r.bytes.clone()))
            .collect()
    }

    /// The OLTP rows (each platform record references its content-addressed object).
    fn wal_rows(&self) -> Vec<WalRow> {
        self.records
            .iter()
            .map(|r| WalRow {
                id: r.row_id.clone(),
                written_at: r.written_at,
                blob_ref: Some(ContentHash::blake3(&r.bytes)),
            })
            .collect()
    }

    /// The durable source log the derived stores reindex FROM (each record's row id at its offset).
    fn source_log(&self) -> SourceLog {
        let mut source = SourceLog::new();
        for r in &self.records {
            source.append(r.written_at, r.row_id.clone());
        }
        source
    }

    /// An archiver whose base + WAL tail makes the corpus's whole offset range reachable (a PITR
    /// reachable to [`Self::latest_offset`] — the dogfood backup's reach).
    fn archiver(&self) -> ContinuousArchiver {
        let mut arch = ContinuousArchiver::new();
        // base at 0 …
        arch.archive_segment(WalSegment {
            end_offset: 0,
            committed_at: 0,
        })
        .expect("base segment");
        arch.take_base_backup(1);
        // … WAL tail covering the corpus.
        arch.archive_segment(WalSegment {
            end_offset: self.latest_offset(),
            committed_at: 10,
        })
        .expect("tail segment");
        arch
    }

    /// A KMS engine holding the self-host tenant's KEK + DEK (so a restore brings back the key the
    /// platform's own envelope-encrypted data is wrapped under — a live, non-erased dogfood tenant).
    fn kms(&self) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(self.tenant.clone(), self.region.clone()));
        kms.ensure_dek(&self.tenant, &self.region, KeyClass::Tenant)
            .expect("the self-host tenant's DEK");
        kms
    }
}

/// **Run the restore-verify gate on the platform's OWN stores (S-M6 — the dogfood loop).** Spins a
/// clean target, restores Myelin's own monorepo commits / CI logs / issues / docs to the corpus's
/// latest consistency point, and runs the SAME three §7.4 assertions ([`RestoreVerifyGate::run`])
/// the managed fleet runs — emitting a [`DogfoodGreenArtifact`] (the gate's dated green artifact +
/// the dogfood-store breakdown) on PASS or a [`crate::restore_verify::GateFailure`] on RED.
///
/// This is the build-layer realisation of *the restore-verify CI job runs on the platform's own
/// commits*: the SAME permanent gate, the SAME no-loss/cross-seam/erasure-held legs, on real team
/// data. A RED here FAILs the dogfood CI job loudly (loud-never-swallowed, EI-01 §5) via
/// [`run_restore_verify_on_dogfood`]'s `Result` — never a silent pass.
///
/// `now_iso` is the caller-supplied date (the harness `today_iso()` at the run) so the artifact is
/// DATED at the dogfood run — a claim that outlives its verification misleads the next agent
/// (EI-01 §1).
pub fn run_restore_verify_on_dogfood(
    corpus: &DogfoodCorpus,
    now_iso: &str,
) -> Result<DogfoodGreenArtifact, crate::restore_verify::GateFailure> {
    let archiver = corpus.archiver();
    let objects = corpus.restored_objects();
    let rows = corpus.wal_rows();
    let source = corpus.source_log();
    let kms = corpus.kms();
    // No dogfood tenant was crypto-shredded before this backup (the self-host install is live) — the
    // erasure-held leg has an empty ledger here. (A dogfood-erasure incident drives the FULL DSAR
    // fan-out E2E-4, P-ST-35; the every-incident loop below registers any such incident's repro.)
    let erasure_ledger = ErasureLedger::new();

    let inputs = GateInputs {
        archiver: &archiver,
        target: corpus.latest_offset(),
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &erasure_ledger,
    };

    // The SAME permanent gate the fleet runs — loud-never-swallowed (a RED is a returned Err).
    let artifact = RestoreVerifyGate::new().run_or_fail_ci(&inputs)?;

    // Break the verified rows down by which of Myelin's own stores they came from (observability is
    // part of the pass — the dogfood artifact names that ALL of Myelin's own data was verified).
    let mut by_store: BTreeMap<DogfoodStore, usize> = BTreeMap::new();
    for r in corpus.records() {
        *by_store.entry(r.store).or_insert(0) += 1;
    }

    Ok(DogfoodGreenArtifact {
        gate: artifact,
        date: now_iso.to_string(),
        tenant: corpus.tenant().clone(),
        region: corpus.region().clone(),
        records_by_store: by_store,
    })
}

/// The dated GREEN ARTIFACT the dogfood restore-verify job emits on PASS — the gate's own
/// [`GreenArtifact`] (the measured no-loss/cross-seam/erasure-held numbers) PLUS the dogfood context
/// (the run date, the self-host tenant + region, and the per-store breakdown of Myelin's own data
/// that was verified). The dogfood loop's proof: the restore-verify gate ran green on the platform's
/// own stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DogfoodGreenArtifact {
    /// The gate's own dated green artifact (the measured numbers — 0 dangling, 0 checksum mismatch,
    /// 0 cross-seam mismatch, 0 resurrected).
    pub gate: GreenArtifact,
    /// The date the dogfood run emitted this artifact (the caller's `today_iso()`).
    pub date: String,
    /// The self-host tenant whose own data was restored + verified.
    pub tenant: TenantId,
    /// The install's region (the team's own region).
    pub region: Region,
    /// How many of Myelin's own records of each store class were verified (monorepo / ci-log /
    /// issue / doc) — the dogfood coverage breakdown.
    pub records_by_store: BTreeMap<DogfoodStore, usize>,
}

impl DogfoodGreenArtifact {
    /// Render the dated dogfood green-artifact line a self-host CI run prints on PASS (the
    /// measured-numbers proof, scoped to Myelin's own data).
    pub fn summary(&self) -> String {
        let breakdown: Vec<String> = self
            .records_by_store
            .iter()
            .map(|(store, n)| format!("{}={n}", store.label()))
            .collect();
        format!(
            "[P-506 DOGFOOD RESTORE-VERIFY GREEN {date}] tenant={tenant} region={region}: {gate} \
             — verified Myelin's OWN data: {breakdown}",
            date = self.date,
            tenant = self.tenant.0,
            region = self.region.as_str(),
            gate = self.gate.summary(),
            breakdown = breakdown.join(", "),
        )
    }
}

// ───────────────────────────── the every-incident-adds-a-drill loop (T-3) ─────────────────────────────

/// A storage incident discovered during dogfooding — a PII-FREE record of a storage fault the team's
/// own use surfaced. The every-incident-adds-a-drill loop (EI-01 §3: *every real incident ends by
/// adding a drill that reproduces it*) turns each incident into (a) a Myelin issue draft + (b) a
/// reproducing storage drill that joins the harness suite and re-runs forever.
///
/// PII-free by construction: an incident names the storage FAULT (a class + a one-line human
/// summary + the gate/drill it touches), never a payload — so it can be filed as a Myelin issue and
/// registered as a drill without carrying personal data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageIncident {
    /// A stable, PII-free incident id (e.g. `"INC-STOR-001"`).
    pub id: String,
    /// The storage gate/drill the incident touches (e.g. `"STOR-D1"`) — the reproducing drill rejoins
    /// THIS gate's lane in the permanent suite.
    pub gate_id: String,
    /// A one-line, PII-free human summary of the fault (the issue title).
    pub summary: String,
    /// The stable name the reproducing drill registers under (the [`DrillRegistry`] key).
    pub repro_drill_name: String,
}

impl StorageIncident {
    /// Record a storage incident `id` against `gate_id`, summarised by `summary`, whose reproducing
    /// drill registers under `repro_drill_name`.
    pub fn new(
        id: impl Into<String>,
        gate_id: impl Into<String>,
        summary: impl Into<String>,
        repro_drill_name: impl Into<String>,
    ) -> StorageIncident {
        StorageIncident {
            id: id.into(),
            gate_id: gate_id.into(),
            summary: summary.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    /// **The Myelin issue draft this incident files (the every-incident loop's "files a Myelin
    /// issue" leg).** A PII-FREE issue BODY a storage incident hands UP to the Issues subsystem —
    /// storage sits below Issues in the DAG, so it owns the structural draft, not the
    /// `Issues::create` call (see the module DEVIATION note). The title is the summary; the body
    /// names the gate + the reproducing drill so the issue is actionable + traceable.
    pub fn issue_draft(&self) -> IncidentIssueDraft {
        IncidentIssueDraft {
            title: format!("[storage incident {}] {}", self.id, self.summary),
            body: format!(
                "A storage incident surfaced during dogfooding.\n\nGate touched: {}\nReproducing \
                 drill (registered into the permanent harness suite, re-runs forever): {}\n\nThe \
                 every-incident-adds-a-drill loop (EI-01 §3) requires this incident's repro join the \
                 suite — the drill below IS that repro.",
                self.gate_id, self.repro_drill_name
            ),
            gate_id: self.gate_id.clone(),
        }
    }

    /// **The reproducing-drill TICKET this incident registers (the every-incident loop's "a
    /// reproducing storage drill that joins the harness" leg).** Names the drill the dogfood
    /// integration test hands to the harness [`DrillRegistry::register_drill`] so the repro re-runs
    /// forever (the T-3 hook). Storage owns the PII-free ticket; the WIRING into the registry is the
    /// dogfood integration test's job (the harness sits above the substrate in the DAG).
    pub fn drill_ticket(&self) -> IncidentDrillTicket {
        IncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
            incident_id: self.id.clone(),
        }
    }
}

/// The PII-free Myelin issue draft a [`StorageIncident`] files (the body the Issues subsystem turns
/// into a real issue). PII-free by construction — it names the FAULT, never a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentIssueDraft {
    /// The issue title (the incident summary).
    pub title: String,
    /// The issue body (names the gate + the reproducing drill — actionable + traceable).
    pub body: String,
    /// The gate the incident touches (so the issue routes to the right lane).
    pub gate_id: String,
}

/// The PII-free reproducing-drill ticket a [`StorageIncident`] registers — the name + the gate it
/// rejoins. The dogfood integration test builds a [`crate::dogfood`]-shaped [`DrillScenario`] under
/// [`Self::drill_name`] and `register_drill`s it (the T-3 hook), so the incident's repro re-runs
/// forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentDrillTicket {
    /// The stable name the reproducing drill registers under (the registry key).
    pub drill_name: String,
    /// The gate/drill lane the repro rejoins (e.g. `"STOR-D1"`).
    pub gate_id: String,
    /// The incident this repro reproduces (the traceability link).
    pub incident_id: String,
}

// ───────────────────────────── the truth-up pass (every PROVEN row rests on a dated green artifact) ─────────────────────────────

/// One PROVEN Storage row the truth-up pass enumerates — a storage gate/drill the ledger claims
/// PROVEN (the STOR-D* family, D-S11/D-S12/D-S13, the floor follow-ons). The truth-up pass asserts
/// each rests on a DATED green artifact: an `artifact_date` of `Some(date)` is a row whose proof is
/// dated + present; `None` is a CLAIMED-NOT-PROVEN row the pass FAILs on loudly (code-wins-over-docs,
/// EI-01 §1 — a claim that outlives its verification misleads the next agent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenRow {
    /// The stable gate/drill id (e.g. `"STOR-D1"`, `"D-S11"`).
    pub id: &'static str,
    /// A one-line human title (what the row proves).
    pub title: &'static str,
    /// The proof command that emits this row's dated green artifact (the `cargo test` target that
    /// lives with the feature prompt — the truth-up pass names it so the artifact is reproducible).
    pub proof_command: &'static str,
    /// The DATE the row's green artifact was last emitted, if any. `Some(date)` ⇒ dated + proven;
    /// `None` ⇒ CLAIMED-NOT-PROVEN (a loud red, never a silent pass).
    pub artifact_date: Option<String>,
}

impl ProvenRow {
    /// `true` iff this row rests on a dated green artifact (the truth-up invariant for one row).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }
}

/// **The FROZEN set of PROVEN Storage rows the truth-up pass enumerates.** This is the single source
/// of which storage gates/drills the ledger claims PROVEN — the STOR-D* permanent + correctness
/// family, the D-S11/D-S12/D-S13 trust-boundary gates, and the named floor follow-ons that SHIPPED
/// (the cross-cell bridge CP-D7, the multi-cell DSR GA-D8, the E2E spine legs). The truth-up pass
/// asserts EVERY id here rests on a dated green artifact; a row without one is a loud failure.
///
/// The id/title/proof-command triples below are the storage rows greened by P-ST-01..P-ST-36 (the
/// coverage map in `by-system/storage.md`). The `date` is supplied by the truth-up runner (the
/// dogfood run's `today_iso()`) — the pass DATES every row at the run so a claim never outlives its
/// verification (EI-01 §1). A row whose proof command did NOT emit a green at the run gets `None` and
/// reds the pass.
pub fn proven_storage_rows(date: &str) -> Vec<ProvenRow> {
    // Each entry is (id, title, proof_command). The truth-up runner stamps `date` on every row it
    // confirms green; the frozen triples below are the storage ledger's PROVEN set.
    fn row(id: &'static str, title: &'static str, cmd: &'static str, date: &str) -> ProvenRow {
        ProvenRow {
            id,
            title,
            proof_command: cmd,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        row(
            "STOR-D1",
            "restore-verify gate — the silent-data-loss floor (the permanent gate)",
            "cargo test -p myelin-storage --test stor_d1_restore_verify_gate_drill",
            date,
        ),
        row(
            "STOR-D2",
            "RPO ≤ 5 min / RTO — continuous archiving + PITR + cell-kill restore",
            "cargo test -p myelin-storage --test stor_d2_cell_kill_rto_drill",
            date,
        ),
        row(
            "STOR-D2-cell",
            "restore-verify at cell scale under world-scale load (RPO/RTO held under surge)",
            "cargo test -p myelin-storage --test stor_d2_d8_cell_scale_under_world_scale_load_drill",
            date,
        ),
        row(
            "STOR-D3",
            "post-restore re-erasure — 0 resurrected subjects across a restore",
            "cargo test -p myelin-storage --test stor_d3_post_restore_reerase_drill",
            date,
        ),
        row(
            "STOR-D4",
            "crypto-shred erase — 0 recoverable PII in backups",
            "cargo test -p myelin-storage --test stor_d4_crypto_shred_drill",
            date,
        ),
        row(
            "STOR-D5",
            "residency end-to-end — 0 cross-region egress",
            "cargo test -p myelin-storage --test stor_d5_cross_region_egress_drill",
            date,
        ),
        row(
            "STOR-D7",
            "blob integrity — BLAKE3 re-hash-on-read, 0 silent serve of a corrupt object",
            "cargo test -p myelin-storage blob",
            date,
        ),
        row(
            "STOR-D8",
            "online migration on a restored copy — lock-time bound held",
            "cargo test -p myelin-storage --test stor_d8_online_migration_under_load_drill",
            date,
        ),
        row(
            "D-S11",
            "trust-scoped cache — cache_scope_violation == 0",
            "cargo test -p myelin-storage ci_cache_scope",
            date,
        ),
        row(
            "D-S12",
            "OLAP restriction gate — olap_restricted_subject_leak == 0",
            "cargo test -p myelin-storage olap_restrict",
            date,
        ),
        row(
            "D-S13",
            "outbound-mirror seam — mirror deny holds",
            "cargo test -p myelin-storage mirror",
            date,
        ),
        row(
            "CP-D7",
            "cross-cell pointer bridge + cell→cell migration — 0 loss",
            "cargo test -p myelin-storage --test cp_d7_cell_to_cell_migration_drill",
            date,
        ),
        row(
            "GA-D8",
            "multi-cell DSR erase fan-out — per-cell receipt set complete",
            "cargo test -p myelin-storage --test ga_d8_multi_cell_erase_fanout_drill",
            date,
        ),
        row(
            "E2E-4",
            "full DSAR crypto-shred fan-out — 0 holders missed, 0 recoverable",
            "cargo test -p myelin-storage holder_fanout",
            date,
        ),
        row(
            "E2E-3",
            "cold-reindex == live for the derived stores (the reindex-parity half)",
            "cargo test -p myelin-storage e2e3_reindex_parity",
            date,
        ),
    ]
}

/// The verdict of a truth-up pass — GREEN (every PROVEN row rests on a dated green artifact) or RED
/// (one or more rows are CLAIMED-NOT-PROVEN: a claim that outlives its verification). `#[must_use]`:
/// a dropped verdict is a swallowed truth-up failure — the docs would silently drift from the code
/// (the exact EI-01 §1 failure mode), so the compiler flags a dropped red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a truth-up verdict must be checked — a dropped RED means a CLAIMED-NOT-PROVEN storage \
              row silently drifts the docs from the code (EI-01 §1: a claim that outlives its \
              verification misleads the next agent)"]
pub enum TruthUpVerdict {
    /// Every enumerated PROVEN Storage row rests on a dated green artifact (the gate invariant holds
    /// end-to-end — no earlier-band storage gate is red).
    Green {
        /// How many PROVEN rows were confirmed dated + green.
        rows_confirmed: usize,
        /// The date the truth-up pass ran (every confirmed row is dated at this run).
        date: String,
    },
    /// One or more PROVEN rows are CLAIMED-NOT-PROVEN (no dated green artifact). Names them so the
    /// failure points at exactly which storage claim outran its verification.
    Red {
        /// The ids of the rows lacking a dated green artifact (the loud failure list).
        undated_rows: Vec<&'static str>,
    },
}

impl TruthUpVerdict {
    /// `true` iff the truth-up pass is green (every PROVEN row dated). The ONLY way to read a pass — a
    /// RED is never silently a pass.
    pub fn is_green(&self) -> bool {
        matches!(self, TruthUpVerdict::Green { .. })
    }

    /// The ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass).
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            TruthUpVerdict::Green { .. } => &[],
            TruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The truth-up pass (S-M6 / EI-01 §1).** Enumerates every PROVEN Storage row and confirms each
/// rests on a DATED green artifact. A row WITHOUT one is a LOUD failure ([`TruthUpVerdict::Red`]),
/// never a silent pass — the code-wins-over-docs discipline made mechanical: the pass re-syncs every
/// PROVEN row to a dated green artifact so a claim never outlives its verification.
///
/// A zero-sized orchestrator — the truth-up pass is `TruthUpPass::run(rows)` over the frozen
/// [`proven_storage_rows`] set (each row dated at the run by [`proven_storage_rows`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct TruthUpPass;

impl TruthUpPass {
    /// A new truth-up pass (stateless).
    pub fn new() -> TruthUpPass {
        TruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`TruthUpVerdict::Green`] (every row dated) or
    /// [`TruthUpVerdict::Red`] (the undated rows named). `date` is the run date stamped onto the
    /// green verdict (so the pass itself is dated — observability of the gate invariant).
    pub fn run(&self, rows: &[ProvenRow], date: &str) -> TruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            TruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            TruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    /// **The loud-never-swallowed truth-up CI entrypoint (EI-01 §5).** Run the pass and turn a RED
    /// verdict into a process-failing `Err` — so a CI invocation `pass.run_or_fail_ci(&rows, date)?`
    /// FAILS the dogfood truth-up job if ANY PROVEN row lacks a dated green artifact, with no swallow.
    /// On GREEN it returns the number of confirmed rows (`Ok`).
    pub fn run_or_fail_ci(&self, rows: &[ProvenRow], date: &str) -> Result<usize, TruthUpRed> {
        match self.run(rows, date) {
            TruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            TruthUpVerdict::Red { undated_rows } => Err(TruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

/// A RED truth-up pass surfaced as an `Err` — the CLAIMED-NOT-PROVEN storage rows, loud + specific
/// (the process exits non-zero, never a silent docs drift).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruthUpRed {
    /// The ids of the rows lacking a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for TruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL — {} storage row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} \
             — a claim that outlives its verification misleads the next agent (EI-01 §1); fix the \
             doc or re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for TruthUpRed {}

#[cfg(test)]
mod tests {
    use super::*;

    fn region() -> Region {
        Region::new("fr-par")
    }

    fn self_host_corpus() -> DogfoodCorpus {
        let mut corpus = DogfoodCorpus::new(TenantId("myelin-self".into()), region());
        // Myelin's OWN data — one record of each store class (monorepo / ci-log / issue / doc).
        corpus
            .commit_record(
                DogfoodStore::Monorepo,
                "commit-abc123",
                10,
                b"fn main() {}".to_vec(),
            )
            .commit_record(
                DogfoodStore::CiLog,
                "ci-run-42-step-3",
                20,
                b"cargo test ... ok".to_vec(),
            )
            .commit_record(
                DogfoodStore::Issue,
                "issue-P-506",
                30,
                b"dogfood the restore gate".to_vec(),
            )
            .commit_record(
                DogfoodStore::Doc,
                "doc-storage-arch",
                40,
                "# Storage §7".as_bytes().to_vec(),
            );
        corpus
    }

    // ───────── (1) the restore-verify gate runs GREEN on the platform's own stores ─────────

    /// **THE HEADLINE: the restore-verify gate runs GREEN on Myelin's OWN data (S-M6).** Restores the
    /// platform's own monorepo commits / CI logs / issues / docs to the latest consistency point and
    /// emits a dated dogfood green artifact — the SAME permanent gate, on real team data.
    #[test]
    fn restore_verify_greens_on_myelins_own_stores() {
        let corpus = self_host_corpus();
        let artifact = run_restore_verify_on_dogfood(&corpus, "2026-06-25")
            .expect("the restore-verify gate must GREEN on Myelin's own data");

        // Every tier landed at the latest consistency point; 0 loss / 0 mismatch / 0 resurrected.
        assert_eq!(artifact.gate.restored_to_offset, 40);
        assert_eq!(
            artifact.gate.oltp_row_count, 4,
            "all four own-data records restored"
        );
        assert_eq!(
            artifact.gate.objects_verified, 4,
            "all four checksum-parity-verified"
        );
        assert_eq!(artifact.gate.dangling_ref_count, 0);
        assert_eq!(artifact.gate.checksum_mismatches, 0);
        assert_eq!(artifact.gate.cross_seam_mismatches, 0);
        assert_eq!(artifact.gate.resurrected_subjects, 0);

        // The dogfood artifact is DATED + names the self-host tenant + the per-store breakdown.
        assert_eq!(artifact.date, "2026-06-25");
        assert_eq!(artifact.tenant.0, "myelin-self");
        assert_eq!(
            artifact.records_by_store.len(),
            4,
            "all four store classes verified"
        );
        let s = artifact.summary();
        assert!(
            s.contains("P-506 DOGFOOD RESTORE-VERIFY GREEN 2026-06-25"),
            "dated: {s}"
        );
        assert!(
            s.contains("monorepo=1")
                && s.contains("ci-log=1")
                && s.contains("issue=1")
                && s.contains("doc=1"),
            "breakdown: {s}"
        );
    }

    /// The dogfood corpus covers Myelin's WHOLE own data set — all four store classes present.
    #[test]
    fn the_dogfood_corpus_covers_all_four_own_stores() {
        let corpus = self_host_corpus();
        assert_eq!(
            corpus.stores_present(),
            DogfoodStore::ALL.into_iter().collect(),
            "the dogfood loop covers monorepo + ci-logs + issues + docs"
        );
    }

    /// **MANDATORY-CORE: a CORRUPT record in Myelin's own data FAILs the dogfood gate (not a silent
    /// pass).** If the platform's own monorepo commit re-hashes wrong after a restore, the dogfood
    /// CI job FAILs loudly — the same checksum-parity floor, on real team data. We drive the SAME
    /// gate the dogfood runner drives, over the corpus's OWN rows, but with one object's bytes
    /// tampered (re-hash ≠ the row's referenced address) — a presence check would PASS; the gate
    /// catches the silent corruption. Kills any mutant that would let a corrupt own-data restore pass.
    #[test]
    fn a_corrupt_own_data_record_fails_the_dogfood_gate() {
        use crate::restore_verify::GateFailure;

        let corpus = self_host_corpus();
        // Sanity: the honest corpus greens (no false RED).
        assert!(run_restore_verify_on_dogfood(&corpus, "2026-06-25").is_ok());

        // Now CORRUPT one of Myelin's own records: the monorepo commit's restored bytes no longer
        // re-hash to the address its OLTP row references. Drive the SAME gate over the corpus's own
        // rows + the tampered object set (the dogfood runner's exact inputs, one object corrupted).
        let archiver = corpus.archiver();
        let rows = corpus.wal_rows();
        let source = corpus.source_log();
        let kms = corpus.kms();
        let ledger = ErasureLedger::new();
        let mut objects = corpus.restored_objects();
        // Tamper the first object's bytes but keep its content-address (silent corruption).
        let original_addr = objects[0].content_address.clone();
        objects[0] = RestoredObject {
            content_address: original_addr.clone(),
            bytes: b"CORRUPTED-MONOREPO-COMMIT".to_vec(),
        };
        let inputs = GateInputs {
            archiver: &archiver,
            target: corpus.latest_offset(),
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };
        let err = RestoreVerifyGate::new()
            .run_or_fail_ci(&inputs)
            .expect_err("a corrupt own-data record MUST fail the dogfood restore-verify gate");
        assert!(
            matches!(err, GateFailure::ChecksumMismatch { ref content_address, .. } if *content_address == original_addr),
            "the gate names the corrupt own-data object: {err}"
        );
        assert!(err.to_string().contains("CHECKSUM MISMATCH"), "loud: {err}");
    }

    // ───────── (2) the every-incident-adds-a-drill loop ─────────

    /// A storage incident files a PII-FREE Myelin issue draft + a reproducing-drill ticket (the
    /// every-incident loop's two legs). The issue body names the gate + the repro drill (actionable +
    /// traceable); the ticket names the drill + the gate it rejoins.
    #[test]
    fn a_storage_incident_files_an_issue_and_registers_a_drill() {
        let incident = StorageIncident::new(
            "INC-STOR-001",
            "STOR-D1",
            "a restored CI log re-hashed wrong after a base-backup boundary",
            "repro_stor_d1_ci_log_rehash_at_base_boundary",
        );

        // (a) the Myelin issue draft — PII-free, names the gate + the repro drill.
        let draft = incident.issue_draft();
        assert!(draft.title.contains("INC-STOR-001"));
        assert!(draft.title.contains("re-hashed wrong"));
        assert_eq!(draft.gate_id, "STOR-D1");
        assert!(draft
            .body
            .contains("repro_stor_d1_ci_log_rehash_at_base_boundary"));
        assert!(draft.body.contains("every-incident-adds-a-drill"));
        // PII-free: the draft names a FAULT, never a payload (no bytes/personal data threaded in).

        // (b) the reproducing-drill ticket — the harness DrillRegistry key + the gate it rejoins.
        let ticket = incident.drill_ticket();
        assert_eq!(
            ticket.drill_name,
            "repro_stor_d1_ci_log_rehash_at_base_boundary"
        );
        assert_eq!(ticket.gate_id, "STOR-D1");
        assert_eq!(ticket.incident_id, "INC-STOR-001");
    }

    // ───────── (3) the truth-up pass ─────────

    /// **The truth-up pass GREENS when every PROVEN Storage row rests on a dated green artifact.**
    /// Enumerates the frozen PROVEN set (dated at the run) and confirms each is dated → green, with
    /// the run date stamped onto the verdict.
    #[test]
    fn truth_up_greens_when_every_proven_row_is_dated() {
        let rows = proven_storage_rows("2026-06-25");
        assert!(!rows.is_empty(), "the PROVEN set is non-empty");
        let verdict = TruthUpPass::new().run(&rows, "2026-06-25");
        assert!(
            verdict.is_green(),
            "every proven row is dated → green: {:?}",
            verdict.undated_rows()
        );
        match verdict {
            TruthUpVerdict::Green {
                rows_confirmed,
                date,
            } => {
                assert_eq!(rows_confirmed, rows.len());
                assert_eq!(date, "2026-06-25");
            }
            TruthUpVerdict::Red { .. } => unreachable!(),
        }
        // The frozen set covers the STOR-D* family + the trust-boundary gates + the shipped floors.
        let ids: Vec<&str> = rows.iter().map(|r| r.id).collect();
        for must in [
            "STOR-D1", "STOR-D2", "STOR-D3", "STOR-D4", "STOR-D5", "STOR-D7", "STOR-D8", "D-S11",
            "D-S12", "D-S13", "CP-D7", "GA-D8", "E2E-4", "E2E-3",
        ] {
            assert!(
                ids.contains(&must),
                "the truth-up set must enumerate {must}"
            );
        }
    }

    /// **MANDATORY-CORE: a PROVEN row WITHOUT a dated green artifact FAILs the truth-up pass LOUDLY
    /// (a row without one is a loud failure, not a silent pass — EI-01 §1).** We inject a
    /// claimed-not-proven row (`artifact_date: None`) and assert the pass reds + names it. Kills any
    /// mutant that drops the undated check or inverts the verdict.
    #[test]
    fn truth_up_reds_loudly_on_a_claimed_not_proven_row() {
        let mut rows = proven_storage_rows("2026-06-25");
        // STOR-D1 (the permanent gate) loses its dated artifact — a claim that outran its verification.
        let undated = rows
            .iter_mut()
            .find(|r| r.id == "STOR-D1")
            .expect("STOR-D1 present");
        undated.artifact_date = None;

        let verdict = TruthUpPass::new().run(&rows, "2026-06-25");
        assert!(
            !verdict.is_green(),
            "a claimed-not-proven row MUST red the truth-up pass"
        );
        assert_eq!(verdict.undated_rows(), &["STOR-D1"]);

        // The loud-never-swallowed CI entrypoint FAILs with the named row.
        let err = TruthUpPass::new()
            .run_or_fail_ci(&rows, "2026-06-25")
            .expect_err("a claimed-not-proven row MUST fail the truth-up CI job");
        assert!(err.to_string().contains("TRUTH-UP FAIL"), "loud: {err}");
        assert!(
            err.to_string().contains("STOR-D1"),
            "names the undated row: {err}"
        );
    }

    /// `run_or_fail_ci` returns `Ok(count)` when the whole PROVEN set is dated (the dogfood truth-up
    /// CI job continues — 0 red earlier-band storage gates).
    #[test]
    fn truth_up_run_or_fail_ci_returns_ok_when_all_dated() {
        let rows = proven_storage_rows("2026-06-25");
        let count = TruthUpPass::new()
            .run_or_fail_ci(&rows, "2026-06-25")
            .expect("a fully-dated PROVEN set must not fail the truth-up CI job");
        assert_eq!(count, rows.len());
    }
}
