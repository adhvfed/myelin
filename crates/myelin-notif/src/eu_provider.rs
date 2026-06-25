//! # The EU-sovereign delivery provider follow-on — the real `DeliveryAdapter` (NOTIF-P26 / P-468, M5)
//!
//! **Owning architecture doc:** `notifications.md` §3.6 (the EU-sovereign delivery fabric — ONE trait
//! [`DeliveryAdapter`](crate::DeliveryAdapter), EU-preferring, region-aware, swappable; PII-minimised
//! off-cell payloads; at-least-once + idempotent on `UNIQUE(idem_key)`) + §10 row 2 (the concrete
//! production EU provider + its DPA / sub-processor posture + the **provider-side erasure** mechanism
//! for an already-sent off-cell payload — `[OPEN — LEGAL]`).
//!
//! **Contracts:** **7.8** [`DeliveryAdapter`](crate::DeliveryAdapter) (CONSUMED — the real EU provider
//! swaps into the SAME frozen trait via the strategy pattern, ADR-12.8; NO trait-shape change). The
//! real provider satisfies the SAME at-least-once + idempotent + off-cell-redacted properties the
//! deterministic mock proved (NOTIF-P16). **Drills:** NOTIF-D9 re-run UNDER the real adapter (crash
//! between provider-ack and ledger-write, retry → `UNIQUE(idem_key)` collapses to EXACTLY-ONE effective
//! delivery per (item, channel)).
//!
//! ## What this prompt (NOTIF-P26) ships
//!
//! 1. **The real EU-sovereign adapter** ([`EuSovereignAdapter`]) — the production-shape
//!    [`DeliveryAdapter`](crate::DeliveryAdapter) that the [`DeliveryFabric`](crate::DeliveryFabric)
//!    dispatches to (the mock→real swap point). It is region-aware + EU-preferring (it REFUSES to
//!    egress from a non-EU region — a loud [`EuProviderError::NonEuRegion`], never a silent extra-EU
//!    leak), it carries ONLY a [`RedactedMessage`](crate::RedactedMessage) (the type makes a full body
//!    impossible by construction — there is no `body` field to leak), and it is at-least-once +
//!    idempotent (a re-submit of the SAME `idem_key` returns the SAME stable `provider_ref` — the
//!    provider de-dupes, so the fabric's `UNIQUE(idem_key)` collapse holds the SAME exactly-one
//!    property under the real provider).
//!
//! 2. **The vendor transport seam** ([`EuTransport`]) — the `[OPEN — LEGAL]` boundary. The concrete
//!    EU-hosted email/push vendor's HTTP client implements THIS trait; the engineering posture (the
//!    EU-region guard, the redaction enforcement, the idempotency, the erasure hook) is production code
//!    and ships NOW, the NAMED vendor + its DPA swaps in behind the seam once counsel/DPO ratifies
//!    (the [`OPEN_LEGAL_PROVIDER_DPA`] flag). The deterministic [`RecordingEuTransport`] is the dev/
//!    drill double standing in for the un-named vendor — NOTIF-D9 re-runs through the REAL adapter code
//!    over this deterministic transport.
//!
//! 3. **The provider-side-erasure-request hook** ([`EuSovereignAdapter::request_provider_erasure`]) —
//!    the named sub-processor obligation (§10 row 2 / X-7): for an ALREADY-SENT off-cell payload, the
//!    adapter issues a provider-side erasure request (by the stable `provider_ref` the submit returned)
//!    so the sub-processor purges its copy. This is the off-cell residual hook the erasure-residual
//!    instancing (NOTIF-P27) CONSUMES — the engineering hook ships here, the residual lawful-basis
//!    statement is the one `[OPEN — LEGAL]` line counsel ratifies (10.9).
//!
//! ## NOTIF-D9 re-run under the real provider
//!
//! `tests/drill_notif_d9_real_provider.rs` re-runs the catalogue NOTIF-D9 window — crash between
//! provider-ack and ledger-write, retry — but the channel adapter is the REAL [`EuSovereignAdapter`]
//! (over the deterministic transport), not the mock. The `UNIQUE(tenant, idem_key)` collapse holds: 1
//! effective delivery; the provider (transport) `submit` is invoked exactly once on the recovered
//! path; off-cell stays redacted. The threshold is exactly 1 — never softened.
//!
//! ## FLOORS named (VISION §3 — name your floors)
//!
//! - **The concrete production EU vendor + its DPA / sub-processor posture** is `[OPEN — LEGAL]`
//!   ([`OPEN_LEGAL_PROVIDER_DPA`], dated): counsel/DPO ratifies the NAMED vendor and the DPA. The
//!   ENGINEERING posture (the trait impl + the EU-region guard + the redaction enforcement + the
//!   idempotency + the provider-side-erasure hook) ships NOW; the vendor's HTTP client swaps in behind
//!   [`EuTransport`] with NO code change to the adapter. We are NOT counsel — this is flagged for
//!   human sign-off (EI-01 §8 — a decision-shaped, irreversible-scope surface pauses for counsel/DPO).
//! - **The off-cell-payload erasure RESIDUAL** that USES [`EuSovereignAdapter::request_provider_erasure`]
//!   is **NOTIF-P27** ([`crate::surge::ERASURE_RESIDUAL_FOLLOW_ON`]) — the X-7 / 10.9 posture instanced
//!   for Notif. The HOOK ships here; the residual is built there.
//!
//! ## Mutation floor (the EU-provider adapter — mandatory-core)
//!
//! The real adapter is mandatory-core: a DOUBLE submit is a double off-cell egress (a GDPR exposure +
//! duplicate-notification spam); an egress from a NON-EU region is a sovereignty breach; a dropped
//! erasure request is an un-purged sub-processor copy. The mutation-tested core is the EU-region guard
//! ([`EuSovereignAdapter::guard_region`]), the stable-`provider_ref` idempotency
//! ([`EuSovereignAdapter::send`] / [`EuSovereignAdapter::provider_ref_for`]), and the
//! provider-side-erasure hook ([`EuSovereignAdapter::request_provider_erasure`]). **Floor: ≥ 80%
//! line/branch mutation score on `eu_provider.rs`** (measured with `cargo mutants`; reported in the
//! P-468 commit body).

use crate::prefs::Channel;
use crate::{Receipt, RedactedMessage};
use myelin_tenancy::Region;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// **The dated `[OPEN — LEGAL]` flag — the concrete EU vendor + DPA selection awaits counsel/DPO
/// sign-off (architecture §10 row 2; EI-01 §8).** The ENGINEERING posture (the [`EuSovereignAdapter`]
/// trait impl + the EU-region guard + the redaction enforcement + the idempotency + the
/// provider-side-erasure hook) ships NOW; the NAMED vendor's HTTP client swaps in behind
/// [`EuTransport`] once counsel/DPO ratifies the vendor + the DPA / sub-processor posture. This is a
/// VISIBLE, dated scorecard row — never a silently-claimed-done. Read by the scorecard test.
pub const OPEN_LEGAL_PROVIDER_DPA: OpenLegalFlag = OpenLegalFlag {
    id: "NOTIF-P26-OPEN-LEGAL",
    // The decision-shaped, irreversible-scope surface that pauses for human sign-off (EI-01 §8).
    subject: "concrete EU-sovereign delivery vendor + DPA / sub-processor posture",
    // The body that ships regardless of the legal selection (the §10 row 2 engineering posture).
    engineering_posture_ships: "DeliveryAdapter trait impl + EU-region guard + RedactedMessage \
        minimisation + provider-side-erasure-request hook",
    // The residual one-line lawful-basis statement counsel ratifies (10.9) — NOT a Notif-restated posture.
    residual_for_counsel:
        "one ratified lawful-basis statement for the already-sent off-cell payload (10.9)",
    // The owner of the human sign-off.
    owner: "counsel / DPO",
    // The date this flag was raised (the scorecard timestamp — a dated row, not a silent done).
    raised: "2026-06-25",
    resolved: false,
};

/// A dated `[OPEN — LEGAL]` scorecard row — a decision-shaped, irreversible-scope surface that pauses
/// for human sign-off (EI-01 §8 — the human bottleneck). Recorded VISIBLY (not silently claimed done):
/// the engineering posture ships, the legal selection is flagged + dated + owned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenLegalFlag {
    /// The stable flag id (the scorecard row key).
    pub id: &'static str,
    /// The decision-shaped surface awaiting human sign-off (the `[OPEN — LEGAL]` subject).
    pub subject: &'static str,
    /// The engineering posture that ships NOW regardless of the legal selection.
    pub engineering_posture_ships: &'static str,
    /// The residual one-line statement counsel ratifies (NOT a Notif-restated posture).
    pub residual_for_counsel: &'static str,
    /// The owner of the human sign-off (counsel / DPO).
    pub owner: &'static str,
    /// The date the flag was raised (the dated scorecard row).
    pub raised: &'static str,
    /// Whether counsel/DPO has resolved it (`false` until ratified — never silently flipped).
    pub resolved: bool,
}

/// The outcome of a vendor [`EuTransport::submit`] — the provider's ack for a redacted off-cell send.
/// `provider_ref` is the sub-processor's durable handle for the sent payload (the key the
/// provider-side erasure request later targets); PII-free (an opaque token).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportReceipt {
    /// The sub-processor's opaque, durable handle for the submitted payload (PII-free). The
    /// provider-side erasure request targets THIS ref.
    pub provider_ref: String,
    /// Whether the vendor accepted the submission (a bounce is `false`).
    pub accepted: bool,
}

/// The outcome of a provider-side erasure request (the §10 row 2 sub-processor obligation). LOUD: a
/// failed request is surfaced (the un-purged copy is the residual), never silently swallowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderErasureOutcome {
    /// The sub-processor accepted the erasure request for `provider_ref` (the copy will be purged).
    Requested {
        /// The `provider_ref` whose erasure was requested.
        provider_ref: String,
    },
    /// There was no recorded off-cell submission for this `idem_key` (nothing was sent off-cell —
    /// e.g. an in-cell `in_app` item, or a never-delivered one). A NO-OP, surfaced (not an error):
    /// there is no sub-processor copy to erase.
    NothingToErase,
}

/// **The EU-sovereign vendor transport seam — the `[OPEN — LEGAL]` boundary (architecture §10 row 2).**
/// The concrete EU-hosted email/push vendor's HTTP client implements THIS trait; the
/// [`EuSovereignAdapter`] wraps it with the production posture (the EU-region guard, the redaction
/// enforcement, the idempotency, the erasure hook). The NAMED vendor swaps in here with NO change to
/// the adapter or the [`DeliveryAdapter`](crate::DeliveryAdapter) trait (the strategy-pattern swap,
/// ADR-12.8). The deterministic [`RecordingEuTransport`] is the dev/drill double.
pub trait EuTransport: Send + Sync {
    /// The vendor's transport id (e.g. `eu-mailer:fr-par` — PII-free; used in the adapter id /
    /// ledger `adapter` column). NOT the channel; this is the SUB-PROCESSOR identity.
    fn transport_id(&self) -> &str;

    /// **Submit a redacted off-cell payload to the vendor at-least-once + idempotent on `idem_key`.**
    /// The vendor MUST de-dupe on `idem_key`: a re-submit of the SAME key returns the SAME
    /// `provider_ref` and does NOT re-send (the provider-side half of the exactly-one property). The
    /// `region` is the EU-preferring egress region (already guarded by the adapter).
    fn submit(
        &self,
        message: &RedactedMessage,
        idem_key: &str,
        region: &Region,
    ) -> TransportReceipt;

    /// **Issue a provider-side erasure request for an already-submitted payload (the §10 row 2
    /// sub-processor obligation).** Targets the durable `provider_ref` the [`submit`](EuTransport::submit)
    /// returned. The vendor purges its copy. Returns `true` if the request was accepted.
    fn request_erasure(&self, provider_ref: &str) -> bool;
}

/// **The real EU-sovereign [`DeliveryAdapter`](crate::DeliveryAdapter) (NOTIF-P26).** The production
/// adapter the [`DeliveryFabric`](crate::DeliveryFabric) dispatches off-cell deliveries to (the
/// mock→real swap point). It is:
///
/// - **region-aware + EU-preferring** — it REFUSES to egress from a non-EU region
///   ([`guard_region`](EuSovereignAdapter::guard_region) → a loud failure, never a silent extra-EU
///   leak; the `DeliveryAdapter::send` shape returns a `Receipt`, so a guarded send returns a
///   `accepted=false` bounce-shaped receipt AND the adapter records the refusal for the loud-path
///   [`try_send`](EuSovereignAdapter::try_send));
/// - **redaction-enforcing by construction** — it only ever sees a [`RedactedMessage`] (the type has
///   no full-body field), so a full-body egress is impossible;
/// - **at-least-once + idempotent** — a re-submit of the SAME `idem_key` returns the SAME stable
///   `provider_ref` (the vendor de-dupes); combined with the fabric's `UNIQUE(idem_key)` ledger
///   collapse, the SAME exactly-one-per-(item, channel) property holds under the real provider.
///
/// It exposes the **provider-side-erasure-request hook**
/// ([`request_provider_erasure`](EuSovereignAdapter::request_provider_erasure)) — the named
/// sub-processor obligation NOTIF-P27 consumes.
pub struct EuSovereignAdapter {
    channel: Channel,
    region: Region,
    transport: Arc<dyn EuTransport>,
    adapter_id: String,
    // idem_key → the provider_ref the vendor returned (the durable handle for the erasure hook). The
    // adapter remembers each accepted submission so request_provider_erasure can target the copy.
    submitted: Arc<Mutex<BTreeMap<String, String>>>,
}

impl EuSovereignAdapter {
    /// Build the real EU-sovereign adapter for `channel`, egressing from `region` (which MUST be EU —
    /// the platform default `fr-par` is Scaleway Paris), over the vendor `transport` (the
    /// `[OPEN — LEGAL]` seam — the deterministic [`RecordingEuTransport`] in dev/drills, the named
    /// vendor's client in prod). The adapter id is `eu:<transport_id>:<channel>`.
    pub fn new(
        channel: Channel,
        region: Region,
        transport: Arc<dyn EuTransport>,
    ) -> EuSovereignAdapter {
        let adapter_id = format!("eu:{}:{}", transport.transport_id(), channel.token());
        EuSovereignAdapter {
            channel,
            region,
            transport,
            adapter_id,
            submitted: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// The adapter id — `eu:<transport_id>:<channel>` (the region-aware §3.6 adapter identity; the
    /// sub-processor identity surfaced into the ledger `adapter` column / audit). PII-free.
    pub fn adapter_id(&self) -> &str {
        &self.adapter_id
    }

    /// **The EU-region guard (the sovereignty invariant; architecture §3.6/§10).** Returns
    /// `Err(EuProviderError::NonEuRegion)` if the configured egress region is NOT EU-preferring — the
    /// real provider NEVER egresses from a non-EU region (the EU-sovereign-by-construction posture).
    /// A non-EU region is a misconfiguration surfaced LOUDLY, never a silent extra-EU leak (EI-01 §3).
    pub fn guard_region(&self) -> Result<(), EuProviderError> {
        if crate::delivery::is_eu_region(&self.region) {
            Ok(())
        } else {
            Err(EuProviderError::NonEuRegion(self.region.0.clone()))
        }
    }

    /// The stable `provider_ref` an accepted submission for `idem_key` produced, if any (the durable
    /// handle the provider-side erasure hook targets). `None` if nothing was submitted off-cell for
    /// this key.
    pub fn provider_ref_for(&self, idem_key: &str) -> Option<String> {
        self.submitted
            .lock()
            .expect("eu adapter submitted-map lock")
            .get(idem_key)
            .cloned()
    }

    /// **The loud send path — submit a redacted off-cell payload, guarding the EU-region invariant
    /// FIRST.** Unlike the [`DeliveryAdapter::send`](crate::DeliveryAdapter::send) shape (which must
    /// return a `Receipt`), this surfaces the EU-region guard as an `Err` (the loud path the fabric
    /// can be wired to honour). On success it remembers the `(idem_key → provider_ref)` for the
    /// erasure hook. Idempotent: a re-submit returns the SAME `provider_ref` (the vendor de-dupes).
    pub fn try_send(
        &self,
        message: &RedactedMessage,
        idem_key: &str,
    ) -> Result<Receipt, EuProviderError> {
        self.guard_region()?;
        let receipt = self.transport.submit(message, idem_key, &self.region);
        if receipt.accepted {
            // Remember the durable handle for the provider-side erasure hook (NOTIF-P27).
            self.submitted
                .lock()
                .expect("eu adapter submitted-map lock")
                .insert(idem_key.to_string(), receipt.provider_ref.clone());
        }
        Ok(Receipt {
            idem_key: idem_key.to_string(),
            accepted: receipt.accepted,
        })
    }

    /// **The provider-side-erasure-request hook (the §10 row 2 sub-processor obligation — NOTIF-P27
    /// consumes this).** For an already-SENT off-cell payload (keyed by `idem_key`), issue a
    /// provider-side erasure request against the durable `provider_ref` the vendor returned — so the
    /// sub-processor purges its copy. If nothing was sent off-cell for this key (an in-cell item, or a
    /// never-delivered one), this is a surfaced [`ProviderErasureOutcome::NothingToErase`] NO-OP
    /// (there is no sub-processor copy). A LOUD failure if the vendor REJECTS the request (an
    /// un-purged copy is the residual — never silently swallowed, EI-01 §3).
    pub fn request_provider_erasure(
        &self,
        idem_key: &str,
    ) -> Result<ProviderErasureOutcome, EuProviderError> {
        let provider_ref = match self.provider_ref_for(idem_key) {
            Some(r) => r,
            None => return Ok(ProviderErasureOutcome::NothingToErase),
        };
        if self.transport.request_erasure(&provider_ref) {
            // The sub-processor accepted the erasure — drop our handle (the copy is being purged).
            self.submitted
                .lock()
                .expect("eu adapter submitted-map lock")
                .remove(idem_key);
            Ok(ProviderErasureOutcome::Requested { provider_ref })
        } else {
            Err(EuProviderError::ErasureRejected(provider_ref))
        }
    }
}

impl crate::DeliveryAdapter for EuSovereignAdapter {
    fn channel(&self) -> &str {
        self.channel.token()
    }

    fn region(&self) -> &Region {
        &self.region
    }

    /// Deliver a redacted off-cell payload (contract 7.8). The EU-region guard runs FIRST: a non-EU
    /// region yields a NON-accepted receipt (the sovereignty refusal — surfaced as a bounce, the loud
    /// `Err` path is [`try_send`](EuSovereignAdapter::try_send)). Idempotent on `idem_key` (the vendor
    /// de-dupes — a re-submit returns the SAME `provider_ref`).
    fn send(&self, message: &RedactedMessage, idem_key: &str) -> Receipt {
        match self.try_send(message, idem_key) {
            Ok(receipt) => receipt,
            // A non-EU region NEVER egresses — the receipt is a refusal (accepted=false). The fabric
            // records it as a bounce, not a silent success (no off-cell egress happened).
            Err(EuProviderError::NonEuRegion(_)) => Receipt {
                idem_key: idem_key.to_string(),
                accepted: false,
            },
            Err(_) => Receipt {
                idem_key: idem_key.to_string(),
                accepted: false,
            },
        }
    }
}

/// An EU-sovereign provider error (loud, never a silent drop — EI-01 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EuProviderError {
    /// The configured egress region is NOT EU-preferring — the provider refuses to egress (the
    /// sovereignty invariant). Carries the offending region code.
    NonEuRegion(String),
    /// The sub-processor REJECTED a provider-side erasure request — the copy is un-purged (the
    /// residual surfaced, never silently swallowed). Carries the `provider_ref`.
    ErasureRejected(String),
}

impl std::fmt::Display for EuProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EuProviderError::NonEuRegion(r) => {
                write!(
                    f,
                    "EU-sovereign provider refuses to egress from non-EU region `{r}`"
                )
            }
            EuProviderError::ErasureRejected(p) => {
                write!(
                    f,
                    "sub-processor rejected the provider-side erasure request for `{p}`"
                )
            }
        }
    }
}

impl std::error::Error for EuProviderError {}

/// **The deterministic recording transport — the dev/drill double standing in for the `[OPEN — LEGAL]`
/// vendor.** A record-only [`EuTransport`] (NO network): it de-dupes on `idem_key` (a re-submit
/// returns the SAME `provider_ref` and is NOT re-sent — the provider-side exactly-one property), it
/// logs every submit (so a drill can assert exactly-once submission), and it records erasure requests
/// (so NOTIF-P27 can assert the copy was purged). This is the SAME deterministic posture the
/// [`MockAdapter`](crate::MockAdapter) is for the fabric — but it sits one layer DOWN (the vendor
/// wire), so the REAL [`EuSovereignAdapter`] code path is exercised over it.
#[derive(Clone)]
pub struct RecordingEuTransport {
    transport_id: String,
    // idem_key → provider_ref (the de-dupe map — a re-submit returns the SAME ref, no re-send).
    refs: Arc<Mutex<BTreeMap<String, String>>>,
    // The log of idem_keys submitted (to assert exactly-once submission).
    submitted: Arc<Mutex<Vec<String>>>,
    // provider_refs whose erasure was requested (so NOTIF-P27 can assert the purge).
    erased: Arc<Mutex<std::collections::BTreeSet<String>>>,
    // idem_keys that should BOUNCE (return accepted=false).
    bounce: Arc<Mutex<std::collections::BTreeSet<String>>>,
}

impl RecordingEuTransport {
    /// A deterministic recording transport with vendor id `transport_id` (e.g. `eu-mailer`).
    pub fn new(transport_id: &str) -> RecordingEuTransport {
        RecordingEuTransport {
            transport_id: transport_id.to_string(),
            refs: Arc::new(Mutex::new(BTreeMap::new())),
            submitted: Arc::new(Mutex::new(Vec::new())),
            erased: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
            bounce: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
        }
    }

    /// Mark an `idem_key` as a BOUNCE (the next `submit` for it returns `accepted = false`).
    pub fn with_bounce(self, idem_key: &str) -> RecordingEuTransport {
        self.bounce
            .lock()
            .expect("bounce set lock")
            .insert(idem_key.to_string());
        self
    }

    /// How many times the vendor was asked to `submit` for `idem_key` (the exactly-once submission
    /// proof; a drill asserts this is 1 — the adapter never double-submits a deduped key).
    pub fn submit_count(&self, idem_key: &str) -> usize {
        self.submitted
            .lock()
            .expect("submitted log lock")
            .iter()
            .filter(|k| *k == idem_key)
            .count()
    }

    /// Was an erasure request issued for `provider_ref`? (so NOTIF-P27 can assert the copy was purged).
    pub fn was_erased(&self, provider_ref: &str) -> bool {
        self.erased
            .lock()
            .expect("erased set lock")
            .contains(provider_ref)
    }
}

impl EuTransport for RecordingEuTransport {
    fn transport_id(&self) -> &str {
        &self.transport_id
    }

    fn submit(
        &self,
        _message: &RedactedMessage,
        idem_key: &str,
        _region: &Region,
    ) -> TransportReceipt {
        // The provider-side de-dupe: a re-submit of the SAME idem_key returns the SAME provider_ref
        // and is NOT re-sent (the exactly-one property, provider-side half).
        let mut refs = self.refs.lock().expect("refs lock");
        if let Some(existing) = refs.get(idem_key) {
            return TransportReceipt {
                provider_ref: existing.clone(),
                accepted: true,
            };
        }
        // First submission for this key — record it (the exactly-once submission log) and mint a
        // stable, deterministic provider_ref (PII-free; derived from the idem_key + vendor id).
        self.submitted
            .lock()
            .expect("submitted log lock")
            .push(idem_key.to_string());
        let bounced = self
            .bounce
            .lock()
            .expect("bounce set lock")
            .contains(idem_key);
        if bounced {
            // A bounce is NOT remembered (no copy was accepted) — a retry re-submits.
            return TransportReceipt {
                provider_ref: String::new(),
                accepted: false,
            };
        }
        let provider_ref = format!("{}:{}", self.transport_id, idem_key);
        refs.insert(idem_key.to_string(), provider_ref.clone());
        TransportReceipt {
            provider_ref,
            accepted: true,
        }
    }

    fn request_erasure(&self, provider_ref: &str) -> bool {
        self.erased
            .lock()
            .expect("erased set lock")
            .insert(provider_ref.to_string());
        true
    }
}

#[cfg(test)]
mod tests;
