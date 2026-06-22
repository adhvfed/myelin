//! # The consent registry + the sub-processor registry + the `transfer_allowed` gate
//! (P-GA-23 → P-150 — the consent/sub-processor/transfer legs of contract 10.5)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§5.2** (consent +
//! sub-processor registries — *"**Consent (G5):** versioned, timestamped, granular, withdrawable,
//! per-subject-keyed (own DEK) for controller-posture activities; withdrawal propagates (stops the
//! path, may trigger deletion). **Sub-processors (G6):** versioned public + per-tenant list with
//! region, DPA reference, change-notification + objection workflow. Sovereignty stance: **no
//! personal data leaves the EU/EEA by default; transfers are off and gated** at the adapter seam
//! (`transfer_allowed` denies extra-EU by default …)."*) + **§5.3** (the outbound push-mirror
//! residency gate reads the SAME `transfer_allowed` policy — the policy ships HERE, Tenancy enforces
//! it; a within-EU CDN clone is allowed, an extra-EU replication is denied by default). Prove-it
//! doctrine: `external-insights/01-process-and-quality-doctrine.md` §3 (*`transfer_allowed` denies
//! extra-EU by default — observed, not claimed*).
//!
//! **Contract-index:** OWNS the **consent / sub-processor / transfer-gate legs of row 10.5** —
//! `consent_record/withdraw(subject, activity, version)`; `subprocessors(tenant) → list`;
//! `transfer_allowed(target_region) → bool` (deny extra-EU by default). The **retention** leg of
//! 10.5 (tightest-policy-wins + legal-hold-aware suspend) is [`crate::retention`] (P-GA-22 → P-149);
//! the G5 **consent DEK holder** (the per-subject crypto-shred key) is the EXISTING
//! [`crate::holders::GdprOwnStoreHolder`] (P-GA-05) — this module does NOT re-define it.
//!
//! ## What THIS prompt (P-GA-23) ships
//! 1. **The consent registry (G5)** ([`ConsentRegistry`]) — versioned, timestamped, granular,
//!    withdrawable, per-subject-keyed. A consent is recorded for a `(subject, tenant, activity)`
//!    with a monotone version + timestamp; a later record SUPERSEDES the prior version (the history
//!    is retained — *which version was in force when* is auditable). **Withdrawal propagates**
//!    ([`ConsentRegistry::withdraw`] → a [`WithdrawalEffect`]): it STOPS the path (the consent is no
//!    longer in force) and, for a controller-posture activity processed ON the basis of consent,
//!    MAY TRIGGER DELETION (the §5.2 "may trigger deletion" — the effect carries the
//!    [`EraseScope`] the caller drives an erase over).
//! 2. **The sub-processor registry (G6)** ([`SubProcessorRegistry`]) — a versioned public + per-
//!    tenant list, each entry carrying its **region**, its **DPA reference**, and a **change-
//!    notification / objection workflow** ([`SubProcessor::object` / `SubProcessorRegistry::object`]
//!    — a tenant may OBJECT to a sub-processor; the objection is recorded + surfaced).
//! 3. **The `transfer_allowed` gate** ([`TransferGate::transfer_allowed`]) — **deny extra-EU by
//!    default**: a transfer of PII-bearing content to a region OUTSIDE the EU/EEA is DENIED unless a
//!    valid transfer mechanism is recorded; a within-EU/EEA target is ALLOWED. The future real-LLM
//!    backend is one such gated, EU-preferring, swappable adapter (AG-9 `[OPEN — LEGAL]`). This is
//!    the SAME gate the §5.3 outbound push-mirror residency gate reads (the policy half).
//!
//! ## The EU/EEA membership predicate (the sovereignty boundary)
//! [`is_eea_region`] is the structural "within-EU/EEA" predicate the gate reads. A [`myelin_tenancy::
//! Region`] is a string code (e.g. `fr-par`, `nl-ams`, `us-east`); a region is within the EU/EEA iff
//! its country/area prefix is an EU/EEA member. The EU/EEA member set is enumerated here as the
//! sovereignty boundary; an UNKNOWN region is treated as EXTRA-EU (deny by default — fail-closed,
//! never assume a region is in-EU). This is the policy the §5.3 gate distinguishes within-EU
//! acceleration (allowed) from extra-EU replication (denied) by.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The outbound push-mirror gate's POLICY** (`transfer_allowed`) ships HERE; the Git mirror SEAM
//!   it gates is **M3/M4** and the gate is PROVEN end-to-end at **M5 → P-GA-35 (GA-11)** (an extra-EU
//!   PII-bearing push-mirror denied by default; a within-EU CDN clone admitted). This module ships
//!   the policy + its decision; the control-plane enforcement wire is Tenancy (§5.3 ownership split).
//! - **The durable Postgres `consent` (G5) + `subprocessor_registry` (G6) tables** are the same DB
//!   floor every M0/M1 store carries (P-007 / P-S12) — on this floor the registries are in-memory
//!   models with byte-for-byte the §5.2 semantics; swapping the store is a config wire, not a code
//!   change. The per-subject consent DEK (the G5 own-key) is the EXISTING
//!   [`crate::holders::GdprOwnStoreHolder`] crypto-shred path (P-GA-05) — withdrawal-triggered
//!   deletion drives that EXISTING holder fan-out, it does not add a second erase path.
//! - **The legal-text DPA ratification** (the sub-processor DPA reference's legal sufficiency) is
//!   **`[OPEN — LEGAL]`** — engineering carries the `dpa_ref` string + the region + the objection
//!   workflow; counsel ratifies the underlying DPA. The STRUCTURE ships here.
//!
//! ## Mutation floor (P-GA-23 TESTS — the `transfer_allowed` deny-by-default + the
//! consent-withdrawal-propagation paths are mandatory-core). `cargo mutants -p myelin-gdpr-service
//! -f crates/myelin-gdpr-service/src/registries.rs` (2026-06-20): recorded in the commit body. The
//! behavioral core every mutation must be caught on: [`TransferGate::transfer_allowed`] (the
//! deny-extra-EU-by-default decision + the recorded-mechanism admit), [`is_eea_region`] (the EU/EEA
//! membership predicate + the unknown-region fail-closed), [`ConsentRegistry::withdraw`] (the
//! propagation: stop-the-path + the controller-posture may-trigger-deletion), and
//! [`ConsentRegistry::record`] (the monotone version supersede). **No `--features integration` leg
//! owed:** the registries + the gate are pure in-memory decision models over already-shipped seams —
//! they touch NO new DB / object-store / cache / bus contract (the durable tables are the named
//! DB floor above; the gate's enforcement landing is Tenancy's control-plane wire, §5.3).

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{EraseScope, SubjectRef, TenantId};
use myelin_tenancy::Region;

// ───────────────────────── telemetry signals (PII-free) ─────────────────────────

/// The `transfer_gate_extra_eu_denials` telemetry: the count of extra-EU transfer attempts the gate
/// DENIED by default. PII-free (a count, never a payload). The GA-D6-sibling green artifact for
/// P-GA-23 is that an extra-EU transfer with no recorded mechanism is denied (this count rises),
/// while a within-EU transfer is admitted (it does not).
pub const TRANSFER_GATE_EXTRA_EU_DENIALS: (&str, &str) =
    ("gdpr.transfer_gate_extra_eu_denials", "count");

/// The `consent_withdrawals` telemetry: the count of consent withdrawals recorded (the withdrawal
/// propagation is observable — §5.2). PII-free: a count, never a withdrawn subject.
pub const CONSENT_WITHDRAWALS: (&str, &str) = ("gdpr.consent_withdrawals", "count");

/// The `subprocessor_objections` telemetry: the count of recorded tenant objections to a
/// sub-processor (the §5.2 objection workflow is observable). PII-free.
pub const SUBPROCESSOR_OBJECTIONS: (&str, &str) = ("gdpr.subprocessor_objections", "count");

// ───────────────────────── the EU/EEA sovereignty boundary ─────────────────────────

/// **The EU/EEA member areas — the sovereignty boundary the `transfer_allowed` gate reads (§5.2).**
/// A [`Region`] code is `<area>-<locality>` (e.g. `fr-par`, `nl-ams`, `us-east`). A region is within
/// the EU/EEA iff its `<area>` prefix is an EU/EEA member area. The set is the 27 EU members + the 3
/// EEA-EFTA states (Iceland, Liechtenstein, Norway). An UNKNOWN area is treated as EXTRA-EU
/// (fail-closed — never assume in-EU; §5.2 "transfers are off and gated by default").
///
/// Two-letter ISO-3166 area codes; lower-cased to match the [`Region`] code convention (the cell
/// region codes Tenancy assigns, e.g. `fr-par`).
const EEA_AREAS: &[&str] = &[
    // EU-27
    "at", "be", "bg", "hr", "cy", "cz", "dk", "ee", "fi", "fr", "de", "gr", "hu", "ie", "it", "lv",
    "lt", "lu", "mt", "nl", "pl", "pt", "ro", "sk", "si", "es", "se",
    // EEA-EFTA (Iceland, Liechtenstein, Norway)
    "is", "li", "no",
];

/// **`is_eea_region(region) → bool` — the within-EU/EEA membership predicate (§5.2).** A
/// [`Region`] code's `<area>` prefix (the part before the first `-`, or the whole code if there is
/// no `-`) is checked against the [`EEA_AREAS`] member set, case-insensitively. An UNKNOWN area is
/// **NOT** in the EEA (fail-closed — the gate denies it by default). This is the structural boundary
/// the §5.3 gate distinguishes within-EU acceleration (allowed) from extra-EU replication (denied)
/// by.
///
/// Examples: `fr-par` → `fr` ∈ EEA → `true`; `nl-ams` → `nl` ∈ EEA → `true`; `us-east` → `us` ∉ EEA
/// → `false`; `xx-nowhere` (unknown) → `false` (fail-closed).
pub fn is_eea_region(region: &Region) -> bool {
    let code = region.as_str();
    let area = code.split('-').next().unwrap_or(code).to_ascii_lowercase();
    EEA_AREAS.contains(&area.as_str())
}

// ───────────────────────── the consent registry (G5) ─────────────────────────

/// **A recorded consent (G5) — versioned, timestamped, granular (§5.2).** One consent of a `subject`
/// (within a `tenant`) to a named processing `activity`, at a monotone `version`, recorded at a
/// `recorded_at_secs` timestamp. A later [`ConsentRegistry::record`] of the same
/// `(subject, tenant, activity)` SUPERSEDES this version (the history is retained — *which version
/// was in force when* is auditable). PII-free: the subject is the opaque `principal_id`, the
/// activity is a granular activity token, never a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentRecord {
    /// The opaque subject token (the `principal_id`) — never PII.
    pub subject_token: String,
    /// The opaque tenant token the subject's data lives under.
    pub tenant_token: String,
    /// The granular processing activity this consent covers (a granular activity token — §5.2
    /// "granular": consent is per-activity, not a blanket grant).
    pub activity: String,
    /// The monotone consent version (a later record bumps it — *which version was in force when*).
    pub version: u64,
    /// Whether this consent is currently IN FORCE (a withdrawal sets it `false` — the path is
    /// stopped). The history is retained, so a withdrawn consent stays recorded with
    /// `in_force = false`.
    pub in_force: bool,
    /// The timestamp (seconds) the consent was recorded / last changed — the §5.2 "timestamped".
    pub recorded_at_secs: u64,
}

/// **The effect of a consent withdrawal (§5.2 — *withdrawal propagates; stops the path, may trigger
/// deletion*).** A withdrawal ALWAYS stops the path (the consent is no longer in force); for a
/// **controller-posture** activity processed ON the basis of consent it MAY ALSO trigger deletion
/// (the data has no other lawful basis once consent is withdrawn). The caller drives the EXISTING
/// erase fan-out over the carried [`EraseScope`] (this module does NOT add a second erase path — the
/// scope feeds [`crate::orchestration::UpstreamHolderOrchestrator::fan_out_erase`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WithdrawalEffect {
    /// The path is stopped (consent no longer in force) and NO deletion is triggered — the activity
    /// has another lawful basis (e.g. a legal obligation / contract), so withdrawing consent stops
    /// future consent-based processing but does not delete the existing record. PII-free.
    StoppedOnly,
    /// The path is stopped AND a deletion is triggered: the activity was processed ON the basis of
    /// consent with no other lawful basis, so the withdrawal leaves no lawful ground to retain the
    /// data (§5.2 "may trigger deletion"). Carries the [`EraseScope`] the caller drives the EXISTING
    /// erase fan-out over.
    StoppedAndTriggersDeletion(EraseScope),
}

impl WithdrawalEffect {
    /// Whether this withdrawal triggers a deletion (the §5.2 "may trigger deletion" leg fired).
    pub fn triggers_deletion(&self) -> bool {
        matches!(self, WithdrawalEffect::StoppedAndTriggersDeletion(_))
    }
}

/// **The consent registry (G5).** A versioned, timestamped, granular, withdrawable, per-subject
/// consent store (§5.2). On the M1 floor an in-memory model (the durable Postgres `consent` table is
/// a named DB floor); the SEMANTICS — monotone version supersede, withdrawal propagation, the
/// per-subject keying — are byte-for-byte what the durable engine backs. The per-subject **consent
/// DEK** (the own-key, so a per-subject crypto-shred = that person's consent record unrecoverable)
/// is the EXISTING [`crate::holders::GdprOwnStoreHolder`] G5 path (P-GA-05) — this registry holds the
/// records; the holder owns the key.
#[derive(Default)]
pub struct ConsentRegistry {
    /// Keyed `(subject_token, tenant_token, activity)` → the CURRENT in-force record. The retained
    /// history (superseded versions) is the `history` map; the current map holds the latest.
    current: Mutex<BTreeMap<(String, String, String), ConsentRecord>>,
    /// The retained version history (every recorded version, including withdrawn ones) — *which
    /// version was in force when* is auditable (§5.2 "versioned").
    history: Mutex<Vec<ConsentRecord>>,
    /// The count of withdrawals recorded (the [`CONSENT_WITHDRAWALS`] telemetry).
    withdrawals: Mutex<u64>,
}

impl ConsentRegistry {
    /// A fresh consent registry (no recorded consents).
    pub fn new() -> ConsentRegistry {
        ConsentRegistry::default()
    }

    /// **`consent_record(subject, tenant, activity, at) → version` (§5.2).** Record a consent of the
    /// subject (within the tenant) to the granular activity, at the timestamp `at_secs`. The version
    /// is **monotone**: the first record is version `1`; a later record of the same
    /// `(subject, tenant, activity)` bumps to the prior version + 1 (the history is retained). The
    /// new record is IN FORCE. Returns the assigned version.
    pub fn record(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        activity: &str,
        at_secs: u64,
    ) -> u64 {
        let key = (
            subject.principal.principal_id.0.clone(),
            tenant.0.clone(),
            activity.to_string(),
        );
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let version = current.get(&key).map(|r| r.version + 1).unwrap_or(1);
        let record = ConsentRecord {
            subject_token: key.0.clone(),
            tenant_token: key.1.clone(),
            activity: key.2.clone(),
            version,
            in_force: true,
            recorded_at_secs: at_secs,
        };
        self.history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(record.clone());
        current.insert(key, record);
        version
    }

    /// The current in-force consent for a `(subject, tenant, activity)`, if any (and still in force).
    pub fn in_force(&self, subject: &SubjectRef, tenant: &TenantId, activity: &str) -> bool {
        let key = (
            subject.principal.principal_id.0.clone(),
            tenant.0.clone(),
            activity.to_string(),
        );
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .map(|r| r.in_force)
            .unwrap_or(false)
    }

    /// **`consent_withdraw(subject, tenant, activity, posture, at) → WithdrawalEffect` — the
    /// withdrawal that PROPAGATES (§5.2).** Withdrawing consent ALWAYS stops the path (the current
    /// record is marked `in_force = false`, retained in the history) and bumps a new
    /// withdrawal-version record. The **propagation** (§5.2 "may trigger deletion"):
    ///
    /// - if the activity was processed under a **controller posture** on the basis of consent
    ///   ([`WithdrawalBasis::ControllerConsentOnly`]) — there is NO other lawful ground to retain the
    ///   data once consent is withdrawn, so the withdrawal TRIGGERS DELETION
    ///   ([`WithdrawalEffect::StoppedAndTriggersDeletion`] carrying the subject [`EraseScope`] the
    ///   caller drives the EXISTING erase fan-out over);
    /// - otherwise (another lawful basis backs the activity —
    ///   [`WithdrawalBasis::HasOtherLawfulBasis`]) the path is STOPPED but no deletion is triggered
    ///   ([`WithdrawalEffect::StoppedOnly`]).
    ///
    /// Idempotent: withdrawing an already-withdrawn (or never-recorded) consent still records a
    /// withdrawal version (the path stays stopped) and returns the same effect class.
    pub fn withdraw(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        activity: &str,
        basis: WithdrawalBasis,
        at_secs: u64,
    ) -> WithdrawalEffect {
        let key = (
            subject.principal.principal_id.0.clone(),
            tenant.0.clone(),
            activity.to_string(),
        );
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        // Stop the path: bump a withdrawal version, mark not-in-force, retain in history.
        let next_version = current.get(&key).map(|r| r.version + 1).unwrap_or(1);
        let withdrawn = ConsentRecord {
            subject_token: key.0.clone(),
            tenant_token: key.1.clone(),
            activity: key.2.clone(),
            version: next_version,
            in_force: false,
            recorded_at_secs: at_secs,
        };
        self.history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(withdrawn.clone());
        current.insert(key, withdrawn);
        *self.withdrawals.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        drop(current);

        // Propagate: a controller-posture consent-only activity has no lawful ground to retain the
        // data once consent is withdrawn — trigger deletion over the subject scope (§5.2).
        match basis {
            WithdrawalBasis::ControllerConsentOnly => {
                WithdrawalEffect::StoppedAndTriggersDeletion(EraseScope::Subject {
                    subject: subject.clone(),
                    tenant: tenant.clone(),
                })
            }
            WithdrawalBasis::HasOtherLawfulBasis => WithdrawalEffect::StoppedOnly,
        }
    }

    /// The retained version history for a `(subject, tenant, activity)` — *which version was in force
    /// when* (§5.2 "versioned"). Ordered by version ascending.
    pub fn history_for(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        activity: &str,
    ) -> Vec<ConsentRecord> {
        let st = subject.principal.principal_id.0.clone();
        let tt = tenant.0.clone();
        let mut hist: Vec<ConsentRecord> = self
            .history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|r| r.subject_token == st && r.tenant_token == tt && r.activity == activity)
            .cloned()
            .collect();
        hist.sort_by_key(|r| r.version);
        hist
    }

    /// The count of withdrawals recorded (the [`CONSENT_WITHDRAWALS`] telemetry value).
    pub fn withdrawal_count(&self) -> u64 {
        *self.withdrawals.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// **The lawful-basis context a consent withdrawal propagates against (§5.2).** Whether the
/// withdrawn activity has ANOTHER lawful basis (so stopping the consent-path is enough) or was
/// processed under a **controller posture on the basis of consent ONLY** (so the withdrawal leaves
/// no lawful ground and TRIGGERS DELETION). This is the [`myelin_gdpr::LawfulBasis`] read at
/// withdrawal time, reduced to the deletion-relevant distinction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WithdrawalBasis {
    /// The activity was processed under a controller posture on the basis of consent ONLY — no other
    /// lawful ground. Withdrawing consent TRIGGERS DELETION (§5.2 "may trigger deletion").
    ControllerConsentOnly,
    /// The activity has another lawful basis (contract / legal obligation / legitimate interest) —
    /// withdrawing consent STOPS the consent-path but does not delete the existing record.
    HasOtherLawfulBasis,
}

// ───────────────────────── the sub-processor registry (G6) ─────────────────────────

/// **A sub-processor entry (G6) — versioned + region + DPA ref + objection workflow (§5.2).** One
/// sub-processor on the public + per-tenant list, carrying its **region** (where it processes — the
/// transfer-gate input), its **DPA reference** (the data-processing agreement ref — the `[OPEN —
/// LEGAL]` ratification carries the legal sufficiency; engineering carries the ref string), a
/// monotone **version**, and the recorded tenant **objections** (the §5.2 change-notification /
/// objection workflow). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubProcessor {
    /// The sub-processor's stable id (e.g. `eu-llm-adapter`, `valkey-cache`).
    pub id: String,
    /// The region the sub-processor processes in (the [`TransferGate`] input — an extra-EU region is
    /// gated). e.g. `fr-par` (within-EU), `us-east` (extra-EU, gated).
    pub region: Region,
    /// The DPA (data-processing agreement) reference (the §5.2 "DPA reference"). Engineering carries
    /// the ref; the legal sufficiency is `[OPEN — LEGAL]` (counsel ratifies).
    pub dpa_ref: String,
    /// The monotone registry version this entry is at (a later [`SubProcessorRegistry::register`] of
    /// the same id bumps it — the §5.2 "versioned"; change-notification reads the version delta).
    pub version: u64,
    /// The recorded tenant objections to this sub-processor (opaque tenant tokens — the §5.2
    /// objection workflow). PII-free.
    pub objections: Vec<String>,
}

/// **The sub-processor registry (G6).** A versioned public + per-tenant list (§5.2). On the M1 floor
/// an in-memory model (the durable Postgres `subprocessor_registry` table is a named DB floor); the
/// SEMANTICS — the monotone version, the region + DPA ref per entry, the objection workflow — are
/// byte-for-byte what the durable engine backs.
#[derive(Default)]
pub struct SubProcessorRegistry {
    /// Keyed by sub-processor id → the current entry. A re-register of the same id bumps its version
    /// (the §5.2 change-notification reads the delta).
    entries: Mutex<BTreeMap<String, SubProcessor>>,
    /// The count of recorded objections (the [`SUBPROCESSOR_OBJECTIONS`] telemetry).
    objection_count: Mutex<u64>,
}

impl SubProcessorRegistry {
    /// A fresh sub-processor registry (empty list).
    pub fn new() -> SubProcessorRegistry {
        SubProcessorRegistry::default()
    }

    /// **`register(id, region, dpa_ref) → version` (§5.2).** Register (or re-version) a sub-processor
    /// with its region + DPA reference. The version is **monotone**: the first register is version
    /// `1`; a re-register of the same id bumps to the prior + 1 (the change-notification reads the
    /// delta). A re-register PRESERVES the recorded objections (a region/DPA change does not clear a
    /// standing objection). Returns the assigned version.
    pub fn register(&self, id: &str, region: Region, dpa_ref: &str) -> u64 {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let (version, objections) = entries
            .get(id)
            .map(|e| (e.version + 1, e.objections.clone()))
            .unwrap_or((1, Vec::new()));
        entries.insert(
            id.to_string(),
            SubProcessor {
                id: id.to_string(),
                region,
                dpa_ref: dpa_ref.to_string(),
                version,
                objections,
            },
        );
        version
    }

    /// **`object(tenant, subprocessor_id)` — the objection workflow (§5.2).** Record a tenant's
    /// objection to a sub-processor (the tenant does not consent to this sub-processor). The
    /// objection is recorded on the entry + surfaced ([`SubProcessor::objections`]); the platform
    /// reads it to honour the objection (e.g. route the tenant away from the objected sub-processor).
    /// Idempotent: a duplicate objection by the same tenant is not double-recorded. Returns `true`
    /// iff the sub-processor exists and the objection was newly recorded.
    pub fn object(&self, tenant: &TenantId, subprocessor_id: &str) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = entries.get_mut(subprocessor_id) else {
            return false;
        };
        let token = tenant.0.clone();
        if entry.objections.contains(&token) {
            return false;
        }
        entry.objections.push(token);
        *self
            .objection_count
            .lock()
            .unwrap_or_else(|e| e.into_inner()) += 1;
        true
    }

    /// The current sub-processor list (the §5.2 "versioned public + per-tenant list"), ordered by id.
    pub fn list(&self) -> Vec<SubProcessor> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// The current entry for a sub-processor id, if registered.
    pub fn get(&self, id: &str) -> Option<SubProcessor> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    /// The count of recorded objections (the [`SUBPROCESSOR_OBJECTIONS`] telemetry value).
    pub fn objection_count(&self) -> u64 {
        *self
            .objection_count
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

// ───────────────────────── the transfer_allowed gate ─────────────────────────

/// The verdict of the `transfer_allowed` gate (§5.2 / §5.3). A transfer of PII-bearing content to a
/// target region is either **allowed** (within-EU/EEA, OR an extra-EU target WITH a recorded valid
/// transfer mechanism) or **denied** (extra-EU by default — no recorded mechanism). The deny is the
/// DEFAULT (§5.2 "transfers are off and gated by default").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferVerdict {
    /// The transfer is allowed — the target is within the EU/EEA (no transfer boundary crossed), OR
    /// an extra-EU target with a recorded valid transfer mechanism (a recorded TIA + SCCs etc.).
    Allowed,
    /// The transfer is DENIED — an extra-EU target with NO recorded valid transfer mechanism (the
    /// §5.2 deny-by-default). The future real-LLM backend defaults here until a mechanism is recorded.
    Denied,
}

impl TransferVerdict {
    /// Whether the transfer is allowed.
    pub fn is_allowed(&self) -> bool {
        matches!(self, TransferVerdict::Allowed)
    }
}

/// **The `transfer_allowed` gate (contract 10.5 — §5.2 / §5.3).** The policy half of the cross-region
/// PII-transfer gate: a transfer of PII-bearing content to a region OUTSIDE the EU/EEA is **denied by
/// default** unless a valid transfer mechanism is recorded for that target; a within-EU/EEA target is
/// **allowed** (no transfer boundary crossed). The SAME gate the §5.3 outbound push-mirror residency
/// gate reads (it distinguishes within-EU acceleration — allowed — from extra-EU replication —
/// denied). GDPR owns the POLICY (this gate); Tenancy/control-plane owns ENFORCEMENT (the cross-region
/// check landing — §5.3 ownership split; the end-to-end proof is P-GA-35).
///
/// The set of extra-EU targets WITH a recorded valid transfer mechanism is the
/// [`Self::valid_transfer_mechanisms`] set — a region added there (a recorded TIA + transfer
/// mechanism) flips from denied to allowed. An EMPTY set ⇒ EVERY extra-EU target is denied (the §5.2
/// default posture: transfers off).
#[derive(Default)]
pub struct TransferGate {
    /// The extra-EU regions WITH a recorded valid transfer mechanism (a recorded TIA + transfer
    /// mechanism). Empty by default (every extra-EU target denied — §5.2). PII-free: region codes.
    valid_transfer_mechanisms: Mutex<std::collections::BTreeSet<Region>>,
    /// The count of extra-EU transfers DENIED by default (the [`TRANSFER_GATE_EXTRA_EU_DENIALS`]
    /// telemetry — the P-GA-23 green artifact's value).
    extra_eu_denials: Mutex<u64>,
}

impl TransferGate {
    /// A fresh transfer gate — **deny extra-EU by default** (no recorded transfer mechanisms).
    pub fn new() -> TransferGate {
        TransferGate::default()
    }

    /// **`record_transfer_mechanism(region)` (§5.2).** Record a valid transfer mechanism for an
    /// extra-EU target region (a recorded TIA + SCCs / adequacy etc.). After this, a transfer to that
    /// region is ALLOWED (the deny-by-default is lifted for that one target with a recorded
    /// mechanism). The legal sufficiency of the mechanism is `[OPEN — LEGAL]` (counsel ratifies; the
    /// gate carries the recorded-mechanism fact). Within-EU regions need no mechanism (they are
    /// allowed structurally).
    pub fn record_transfer_mechanism(&self, region: Region) {
        self.valid_transfer_mechanisms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(region);
    }

    /// **`transfer_allowed(target) → TransferVerdict` — deny extra-EU by default (§5.2 / §5.3).**
    /// The decision:
    ///
    /// 1. a target **within the EU/EEA** ([`is_eea_region`]) is **allowed** — no residency boundary
    ///    is crossed (a within-EU CDN clone, a within-EU sub-processor — §5.3 "within-EU acceleration
    ///    is permitted");
    /// 2. an **extra-EU** target is **denied** UNLESS a valid transfer mechanism is recorded for it
    ///    ([`Self::record_transfer_mechanism`]) — the §5.2 deny-by-default (the future real-LLM
    ///    backend defaults to denied until an EU-hostable mechanism is recorded; an extra-EU
    ///    replication / push-mirror is denied — §5.3).
    ///
    /// Each extra-EU DENY bumps the [`TRANSFER_GATE_EXTRA_EU_DENIALS`] telemetry (the green
    /// artifact's value — 0 default extra-EU transfers slip through).
    pub fn transfer_allowed(&self, target: &Region) -> TransferVerdict {
        // Within-EU/EEA — no transfer boundary crossed; allowed structurally (§5.3 within-EU clone).
        if is_eea_region(target) {
            return TransferVerdict::Allowed;
        }
        // Extra-EU — denied by default UNLESS a valid transfer mechanism is recorded for the target.
        let has_mechanism = self
            .valid_transfer_mechanisms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(target);
        if has_mechanism {
            TransferVerdict::Allowed
        } else {
            *self
                .extra_eu_denials
                .lock()
                .unwrap_or_else(|e| e.into_inner()) += 1;
            TransferVerdict::Denied
        }
    }

    /// The count of extra-EU transfers DENIED by default (the [`TRANSFER_GATE_EXTRA_EU_DENIALS`]
    /// telemetry value — the P-GA-23 green artifact: extra-EU transfers do not slip through).
    pub fn extra_eu_denial_count(&self) -> u64 {
        *self
            .extra_eu_denials
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            t("acme"),
        ))
    }

    // ───────────── the EU/EEA membership predicate ─────────────

    /// **`is_eea_region` admits within-EU/EEA areas and denies extra-EU (fail-closed on unknown).**
    #[test]
    fn is_eea_region_admits_eu_eea_denies_extra_eu_and_unknown() {
        assert!(
            is_eea_region(&Region::new("fr-par")),
            "fr (France) is in the EU"
        );
        assert!(
            is_eea_region(&Region::new("nl-ams")),
            "nl (Netherlands) is in the EU"
        );
        assert!(
            is_eea_region(&Region::new("de-fra")),
            "de (Germany) is in the EU"
        );
        assert!(
            is_eea_region(&Region::new("no-osl")),
            "no (Norway) is EEA-EFTA"
        );
        assert!(
            is_eea_region(&Region::new("is-rey")),
            "is (Iceland) is EEA-EFTA"
        );
        assert!(!is_eea_region(&Region::new("us-east")), "us is extra-EU");
        assert!(
            !is_eea_region(&Region::new("uk-lon")),
            "uk (post-Brexit) is extra-EU"
        );
        assert!(
            !is_eea_region(&Region::new("xx-nowhere")),
            "an unknown area is extra-EU (fail-closed)"
        );
        // case-insensitive on the area prefix.
        assert!(
            is_eea_region(&Region::new("FR-PAR")),
            "the area match is case-insensitive"
        );
    }

    // ───────────── the transfer_allowed gate (deny extra-EU by default) ─────────────

    /// **`transfer_allowed` DENIES extra-EU by default + ADMITS within-EU** (the headline P-GA-23
    /// drill — §5.2 / §5.3 — the green artifact: 0 default extra-EU transfers; within-EU allowed).
    #[test]
    fn transfer_allowed_denies_extra_eu_by_default_admits_within_eu() {
        let gate = TransferGate::new();
        // within-EU — allowed structurally (no boundary crossed; the §5.3 within-EU clone).
        assert_eq!(
            gate.transfer_allowed(&Region::new("fr-par")),
            TransferVerdict::Allowed
        );
        assert_eq!(
            gate.transfer_allowed(&Region::new("nl-ams")),
            TransferVerdict::Allowed
        );
        assert!(gate.transfer_allowed(&Region::new("de-fra")).is_allowed());
        // extra-EU — DENIED by default (no recorded mechanism). The deny-by-default posture.
        assert_eq!(
            gate.transfer_allowed(&Region::new("us-east")),
            TransferVerdict::Denied
        );
        assert_eq!(
            gate.transfer_allowed(&Region::new("ap-tokyo")),
            TransferVerdict::Denied
        );
        // the green artifact: extra-EU denials counted (2 above), within-EU allowed (not counted).
        assert_eq!(
            gate.extra_eu_denial_count(),
            2,
            "0 default extra-EU transfers slipped through"
        );
    }

    /// **An extra-EU target WITH a recorded valid transfer mechanism is ALLOWED** (the deny-by-default
    /// is lifted ONLY for a target with a recorded mechanism — §5.2; the future EU-hostable real-LLM
    /// adapter path).
    #[test]
    fn an_extra_eu_target_with_a_recorded_mechanism_is_allowed() {
        let gate = TransferGate::new();
        // before recording — denied.
        assert_eq!(
            gate.transfer_allowed(&Region::new("us-east")),
            TransferVerdict::Denied
        );
        // record a valid transfer mechanism for us-east — now allowed.
        gate.record_transfer_mechanism(Region::new("us-east"));
        assert_eq!(
            gate.transfer_allowed(&Region::new("us-east")),
            TransferVerdict::Allowed
        );
        // a DIFFERENT extra-EU target (no mechanism) is still denied — the mechanism is per-target.
        assert_eq!(
            gate.transfer_allowed(&Region::new("ap-tokyo")),
            TransferVerdict::Denied
        );
    }

    // ───────────── the consent registry (withdrawal propagates) ─────────────

    /// **Consent withdrawal propagates: it STOPS the path AND (controller-posture consent-only)
    /// TRIGGERS DELETION** (the §5.2 "withdrawal propagates; may trigger deletion" — the core
    /// P-GA-23 drill).
    #[test]
    fn consent_withdrawal_propagates_and_triggers_deletion_for_controller_consent_only() {
        let reg = ConsentRegistry::new();
        let s = subject("u-1");
        let tenant = t("acme");
        let v = reg.record(&s, &tenant, "marketing-emails", 1000);
        assert_eq!(v, 1, "first consent is version 1");
        assert!(
            reg.in_force(&s, &tenant, "marketing-emails"),
            "consent is in force"
        );

        // withdraw a controller-posture, consent-ONLY activity — the path stops AND deletion fires.
        let effect = reg.withdraw(
            &s,
            &tenant,
            "marketing-emails",
            WithdrawalBasis::ControllerConsentOnly,
            2000,
        );
        assert!(
            effect.triggers_deletion(),
            "controller consent-only ⇒ may-trigger-deletion fired"
        );
        match effect {
            WithdrawalEffect::StoppedAndTriggersDeletion(EraseScope::Subject {
                subject,
                tenant: tn,
            }) => {
                assert_eq!(
                    subject.principal.principal_id.0, "u-1",
                    "the erase scope is the subject"
                );
                assert_eq!(tn.0, "acme");
            }
            other => panic!("expected StoppedAndTriggersDeletion(Subject), got {other:?}"),
        }
        // the path is STOPPED (consent no longer in force).
        assert!(
            !reg.in_force(&s, &tenant, "marketing-emails"),
            "the consent-path is stopped"
        );
        assert_eq!(
            reg.withdrawal_count(),
            1,
            "the withdrawal is observable (telemetry)"
        );

        // a SECOND withdrawal bumps the count to 2 (the count is a running total, not a constant —
        // this pins the accessor against a `-> 1` mutant).
        reg.record(&s, &tenant, "analytics", 3000);
        reg.withdraw(
            &s,
            &tenant,
            "analytics",
            WithdrawalBasis::HasOtherLawfulBasis,
            4000,
        );
        assert_eq!(
            reg.withdrawal_count(),
            2,
            "the second withdrawal bumps the running total"
        );
    }

    /// **A withdrawal of an activity with ANOTHER lawful basis STOPS the path but does NOT trigger
    /// deletion** (§5.2 — stopping the consent-path is enough when another lawful ground retains the
    /// data).
    #[test]
    fn consent_withdrawal_with_another_lawful_basis_stops_without_deletion() {
        let reg = ConsentRegistry::new();
        let s = subject("u-2");
        let tenant = t("acme");
        reg.record(&s, &tenant, "service-telemetry", 1000);
        let effect = reg.withdraw(
            &s,
            &tenant,
            "service-telemetry",
            WithdrawalBasis::HasOtherLawfulBasis,
            2000,
        );
        assert_eq!(
            effect,
            WithdrawalEffect::StoppedOnly,
            "another basis ⇒ stop only, no deletion"
        );
        assert!(!effect.triggers_deletion());
        assert!(
            !reg.in_force(&s, &tenant, "service-telemetry"),
            "the path is still stopped"
        );
    }

    /// **Consent is versioned + the history retained** (§5.2 — *which version was in force when*). A
    /// re-record bumps the monotone version; the withdrawal bumps again; the whole history is
    /// auditable.
    #[test]
    fn consent_is_versioned_and_history_is_retained() {
        let reg = ConsentRegistry::new();
        let s = subject("u-3");
        let tenant = t("acme");
        assert_eq!(reg.record(&s, &tenant, "a", 1000), 1);
        assert_eq!(
            reg.record(&s, &tenant, "a", 2000),
            2,
            "re-record bumps the monotone version"
        );
        reg.withdraw(&s, &tenant, "a", WithdrawalBasis::HasOtherLawfulBasis, 3000);

        // Records for a DIFFERENT activity, a DIFFERENT subject, and a DIFFERENT tenant — the
        // `history_for` filter must EXCLUDE every one of these (the AND of all three coordinates;
        // an `||` would wrongly pull them in, so this pins the `&&`).
        reg.record(&s, &tenant, "other-activity", 1500);
        reg.record(&subject("u-other"), &tenant, "a", 1500);
        reg.record(&s, &t("globex"), "a", 1500);

        let hist = reg.history_for(&s, &tenant, "a");
        assert_eq!(
            hist.len(),
            3,
            "exactly the (u-3, acme, a) versions retained — other subject/activity/tenant excluded"
        );
        assert_eq!(hist[0].version, 1);
        assert!(hist[0].in_force, "v1 was in force when recorded");
        assert_eq!(hist[2].version, 3);
        assert!(!hist[2].in_force, "the withdrawal version is not-in-force");
        assert_eq!(hist[2].recorded_at_secs, 3000, "timestamped");
        // every retained record IS for this exact coordinate (the filter excluded the others).
        for r in &hist {
            assert_eq!(r.subject_token, "u-3");
            assert_eq!(r.tenant_token, "acme");
            assert_eq!(r.activity, "a");
        }
    }

    /// **Consent is granular (per-activity)** — a withdrawal of one activity does NOT withdraw another.
    #[test]
    fn consent_is_granular_per_activity() {
        let reg = ConsentRegistry::new();
        let s = subject("u-4");
        let tenant = t("acme");
        reg.record(&s, &tenant, "emails", 1000);
        reg.record(&s, &tenant, "analytics", 1000);
        reg.withdraw(
            &s,
            &tenant,
            "emails",
            WithdrawalBasis::HasOtherLawfulBasis,
            2000,
        );
        assert!(!reg.in_force(&s, &tenant, "emails"), "emails withdrawn");
        assert!(
            reg.in_force(&s, &tenant, "analytics"),
            "analytics still in force (granular)"
        );
    }

    // ───────────── the sub-processor registry (versioned + region + DPA ref + objection) ─────────────

    /// **A sub-processor entry records region + DPA ref + version + the objection workflow** (§5.2).
    #[test]
    fn subprocessor_registry_records_region_dpa_ref_version_and_objection() {
        let reg = SubProcessorRegistry::new();
        let v = reg.register("eu-llm-adapter", Region::new("fr-par"), "DPA-2026-001");
        assert_eq!(v, 1, "first register is version 1");
        let entry = reg.get("eu-llm-adapter").expect("registered");
        assert_eq!(entry.region, Region::new("fr-par"), "region recorded");
        assert_eq!(entry.dpa_ref, "DPA-2026-001", "DPA ref recorded");
        assert!(entry.objections.is_empty(), "no objections yet");

        // re-register bumps the version (change-notification reads the delta).
        let v2 = reg.register("eu-llm-adapter", Region::new("nl-ams"), "DPA-2026-002");
        assert_eq!(v2, 2, "re-register bumps the monotone version");

        // the objection workflow: a tenant objects; it is recorded + surfaced + counted.
        assert_eq!(
            reg.objection_count(),
            0,
            "no objections yet (count starts at 0)"
        );
        assert!(
            reg.object(&t("acme"), "eu-llm-adapter"),
            "the objection is newly recorded"
        );
        assert!(
            !reg.object(&t("acme"), "eu-llm-adapter"),
            "a duplicate objection is not double-recorded"
        );
        let entry = reg.get("eu-llm-adapter").expect("registered");
        assert_eq!(
            entry.objections,
            vec!["acme".to_string()],
            "the objection is surfaced on the entry"
        );
        assert_eq!(
            reg.objection_count(),
            1,
            "the objection is observable (telemetry)"
        );

        // a DIFFERENT tenant objecting bumps the count to 2 (a running total, not a constant — this
        // pins the accessor against a `-> 1` mutant).
        assert!(
            reg.object(&t("globex"), "eu-llm-adapter"),
            "a second tenant's objection is recorded"
        );
        assert_eq!(
            reg.objection_count(),
            2,
            "the second objection bumps the running total"
        );

        // an objection to an unregistered sub-processor is a no-op (returns false).
        assert!(
            !reg.object(&t("acme"), "ghost-adapter"),
            "no entry to object to"
        );
    }

    /// **A re-register PRESERVES a standing objection** (a region/DPA change does not clear a
    /// tenant's recorded objection).
    #[test]
    fn reregister_preserves_a_standing_objection() {
        let reg = SubProcessorRegistry::new();
        reg.register("x", Region::new("us-east"), "DPA-1");
        reg.object(&t("acme"), "x");
        reg.register("x", Region::new("fr-par"), "DPA-2"); // moved in-EU.
        let entry = reg.get("x").expect("registered");
        assert_eq!(entry.version, 2);
        assert_eq!(
            entry.objections,
            vec!["acme".to_string()],
            "the objection survives the re-version"
        );
    }

    /// **The registry lists every entry (the §5.2 versioned list).**
    #[test]
    fn subprocessor_registry_lists_entries() {
        let reg = SubProcessorRegistry::new();
        reg.register("a", Region::new("fr-par"), "DPA-a");
        reg.register("b", Region::new("us-east"), "DPA-b");
        let list = reg.list();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|e| e.id == "a"));
        assert!(list.iter().any(|e| e.id == "b"));
    }

    // ───────────── telemetry NAME/UNIT anchors ─────────────

    #[test]
    fn telemetry_signal_names_and_units_are_anchored() {
        assert_eq!(
            TRANSFER_GATE_EXTRA_EU_DENIALS.0,
            "gdpr.transfer_gate_extra_eu_denials"
        );
        assert_eq!(TRANSFER_GATE_EXTRA_EU_DENIALS.1, "count");
        assert_eq!(CONSENT_WITHDRAWALS.0, "gdpr.consent_withdrawals");
        assert_eq!(CONSENT_WITHDRAWALS.1, "count");
        assert_eq!(SUBPROCESSOR_OBJECTIONS.0, "gdpr.subprocessor_objections");
        assert_eq!(SUBPROCESSOR_OBJECTIONS.1, "count");
    }
}
