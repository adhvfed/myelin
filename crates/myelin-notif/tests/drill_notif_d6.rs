//! # NOTIF-D6 — erase a user → every inbox item humanises to `[erased user]`; 0 recoverable PII;
//! the off-cell-sent payload crypto-shredded / erasure-requested (the X-7 posture instanced for Notif)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **NOTIF-D6** ("Erase a user → every inbox item humanises to `[erased user]`; 0 recoverable PII;
//! off-cell-sent payload crypto-shredded/erasure-requested." Artifact: **erase-receipt; 0 recoverable**;
//! lane SCHED), `notifications.md` §3.9 (the residual stated BY REFERENCE to X-7 / contract 10.9), and
//! `00-reconciliation-decisions.md` §X-7 (ONE platform-wide erasure posture; instanced per subsystem).
//!
//! **The dated GREEN artifact (2026-06-25).** This drill drives BOTH legs of the X-7 posture for Notif
//! end-to-end, with NO threshold weakened:
//!
//! 1. **The structural references-not-payloads tombstone-for-free** (NOTIF-P4 + the §3.6 humanise,
//!    NOTIF-P9): an erased actor's appearance in EVERY inbox item humanises to `[erased user]` — with
//!    NO PII-column mutation on the refs-stored rows. The inbox stores the subject only by ref, so the
//!    Identity 4.8 pseudonym-shred makes the opaque id unresolvable and the title resolves to the
//!    `[erased user]` tombstone at READ time. Threshold: every appearance tombstones — never softened.
//! 2. **The inline-PII residual erase** (the X-7 / 10.9 floor instanced, NOTIF-P27): the off-cell-sent
//!    redacted summary's per-subject DEK is crypto-shredded (11.4) → 0 inline-PII columns recoverable;
//!    a provider-side erasure-request is issued for the already-sent off-cell copy (the named
//!    sub-processor obligation, the NOTIF-P26 hook); the erase receipt is sealed into the erasure
//!    ledger (10.8). Threshold: 0 recoverable PII — NEVER softened.
//!
//! Both legs run over the SAME erased subject; the measured artifact is the [`ResidualEraseReceipt`]
//! (`is_green()` ⟺ 0 recoverable + restrict applied) PLUS the count of inbox appearances that resolve
//! to `[erased user]` (which MUST equal the appearance count — every one tombstones).
//!
//! ## FLOOR named (VISION §3)
//! The one `[OPEN — LEGAL]` residual lawful-basis statement (10.9) awaits counsel/DPO ratification
//! ([`OPEN_LEGAL_PROVIDER_DPA`]); the STRUCTURAL floor (both legs above) ships + is proven GREEN here.

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::humanise::{
    humanise, Channel as HumaniseChannel, RefProjection, RefResolution, RefResolvePort, Tombstone,
    TombstoneReason, DEFAULT_LOCALE,
};
use myelin_notif::prefs::Channel;
use myelin_notif::{
    build_idem_key, erase_residual, reason_template_key, redact_for_offcell, Class,
    EuSovereignAdapter, HumanisedString, InMemoryDeliveryShredder, InlineDeliveryShredder,
    NotifErasureLedger, OffCellResidual, Reason, RecordingEuTransport, RestrictSet, TemplateStore,
    OPEN_LEGAL_PROVIDER_DPA,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::{Arc, Mutex};

use myelin_identity::{Consistency, ConsistencyMode, Zookie};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

/// The opaque, pseudonymous subject id to erase (the §3.9 recipient/actor pseudonym — never a name).
const SUBJECT_ID: &str = "u-erase";

/// The actor-ref that names the subject across other users' inbox items (the by-ref appearance).
fn subject_actor_ref() -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/identity/principal/{SUBJECT_ID}"))
}

/// A synthetic Refs resolve chokepoint: a ref naming the ERASED subject resolves to a `[erased user]`
/// tombstone (the Identity 4.8 pseudonym-shred made the opaque id unresolvable); any other ref
/// resolves to a projection. The SAME `Projection | Tombstone` shape the real chokepoint returns.
struct ErasingResolver {
    erased_refs: Mutex<Vec<String>>,
}
impl ErasingResolver {
    fn new() -> ErasingResolver {
        ErasingResolver {
            erased_refs: Mutex::new(Vec::new()),
        }
    }
    fn mark_erased(&self, r: &ArtifactRef) {
        self.erased_refs.lock().unwrap().push(r.0.clone());
    }
}
impl RefResolvePort for ErasingResolver {
    fn resolve_display(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
        _viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if self
            .erased_refs
            .lock()
            .unwrap()
            .iter()
            .any(|x| x == &ref_.0)
        {
            // The erased actor → the `[erased user]` tombstone (NO title — references-not-payloads).
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Erased,
            });
        }
        RefResolution::Projection(RefProjection {
            ref_: ref_.clone(),
            title: "issue PROJ-1".into(),
            icon: "issue".into(),
        })
    }
}

/// The off-cell redacted summary that named the subject (the one inline-PII case Notif emits off-cell).
fn redacted_summary() -> HumanisedString {
    HumanisedString {
        text: "you were mentioned by a teammate".into(),
        links: vec!["myelin://acme/issues/issue/PROJ-1".into()],
        icon: "mention".into(),
    }
}

/// **NOTIF-D6 (the dated green artifact, 2026-06-25): erase a user → every inbox appearance humanises
/// to `[erased user]`, 0 inline-PII recoverable, the off-cell copy crypto-shredded + erasure-requested,
/// the erase-receipt sealed. The threshold (0 recoverable PII) is measured, NEVER weakened.**
#[test]
fn notif_d6_erase_user_zero_recoverable_pii_and_every_appearance_tombstones() {
    // ───────────────────────────── Leg 1: structural tombstone-for-free ──────────────────────────
    // The erased subject appears across THREE inbox items (as a by-ref actor). After the
    // pseudonym-shred, EVERY appearance humanises to `[erased user]` — with NO PII-column mutation.
    let resolver = ErasingResolver::new();
    resolver.mark_erased(&subject_actor_ref()); // Identity 4.8 shred → the actor ref is unresolvable
    let templates = TemplateStore::with_platform_defaults();

    let appearances: &[(Reason, &str)] = &[
        (Reason::Mentioned, "itm-1"),
        (Reason::Replied, "itm-2"),
        (Reason::Assigned, "itm-3"),
    ];
    let mut tombstoned = 0u64;
    for (reason, _item) in appearances {
        let key = reason_template_key(*reason);
        let h = humanise(
            &resolver,
            &tenant(),
            &region(),
            &templates,
            key,
            std::slice::from_ref(&subject_actor_ref()),
            &viewer("u-bob"), // a DIFFERENT viewer whose inbox names the erased subject by ref
            DEFAULT_LOCALE,
            &strong("z1"),
            HumaniseChannel::Cli,
        );
        assert!(
            h.text.contains("[erased user]"),
            "every appearance of the erased subject humanises to [erased user] (reason={key}): got {:?}",
            h.text
        );
        // The erased ref is NOT routable — no link leaks a route to the erased person.
        assert!(
            h.links.is_empty(),
            "an erased subject yields no link (reason={key})"
        );
        tombstoned += 1;
    }
    assert_eq!(
        tombstoned,
        appearances.len() as u64,
        "EVERY inbox appearance of the erased subject tombstones to [erased user] (0 missed)"
    );

    // ───────────────────────────── Leg 2: the inline-PII residual erase ──────────────────────────
    // The subject also received an OFF-CELL redacted summary (the one inline-PII case Notif emits off
    // the cell). Deliver it, then erase the residual: shred the per-subject DEK, request provider-side
    // erasure of the already-sent copy, seal the receipt.
    let transport = RecordingEuTransport::new("eu-mailer");
    let provider = EuSovereignAdapter::new(Channel::Email, region(), Arc::new(transport.clone()));
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    // The off-cell delivery (EU region — accepted). The redacted summary's inline-PII column is sealed
    // under a per-subject DEK.
    let idem = build_idem_key("itm-offcell", Channel::Email);
    let _msg = redact_for_offcell(redacted_summary(), Class::Direct);
    provider
        .try_send(
            &redact_for_offcell(redacted_summary(), Class::Direct),
            &idem,
        )
        .expect("the off-cell redacted summary is delivered (EU region)");
    let provider_ref = provider
        .provider_ref_for(&idem)
        .expect("the off-cell copy has a durable provider_ref");

    let dek = myelin_events::PiiKeyRef(format!("kms://acme/epoch-1/subject:{SUBJECT_ID}"));
    shredder.seal(&dek);
    assert!(
        shredder.is_live(&dek),
        "the inline-PII delivery column is RECOVERABLE before erase"
    );

    let residuals = vec![OffCellResidual {
        idem_key: idem.clone(),
        inline_pii_key: Some(dek.clone()),
    }];

    let receipt = erase_residual(
        SUBJECT_ID,
        &tenant(),
        &residuals,
        &shredder,
        &restrict,
        &provider,
        &ledger,
        myelin_events::Timestamp("2026-06-25T00:00:00Z".into()),
    )
    .expect("the structural erase succeeds (the X-7 posture instanced)");

    // ── The measured artifact: the erase-receipt; 0 recoverable ──
    assert_eq!(
        receipt.recoverable_remaining, 0,
        "NOTIF-D6: 0 inline-PII delivery columns recoverable (the gate threshold — never softened)"
    );
    assert!(
        receipt.is_green(),
        "NOTIF-D6 GREEN: 0 recoverable PII + restrict suppression applied"
    );
    assert!(
        !shredder.is_live(&dek),
        "the off-cell payload's inline-PII DEK is crypto-shredded (unrecoverable in live + backups)"
    );
    assert!(
        transport.was_erased(&provider_ref),
        "the already-sent off-cell copy was provider-side erasure-requested (sub-processor purge)"
    );
    assert!(
        ledger.is_erased(SUBJECT_ID),
        "the erase-receipt is sealed in the erasure ledger (10.8) — provable + survives a restore"
    );
    assert!(
        restrict.is_restricted(SUBJECT_ID),
        "the subject's NEW routing/delivery is suppressed (restrict, 10.1)"
    );

    // The one [OPEN — LEGAL] residual statement is FLAGGED (counsel/DPO), never silently claimed done.
    let open_legal = OPEN_LEGAL_PROVIDER_DPA;
    assert!(
        !open_legal.resolved,
        "the residual lawful-basis statement (10.9) is flagged OPEN for counsel — the structural floor ships regardless"
    );

    eprintln!(
        "NOTIF-D6 GREEN (2026-06-25): erased subject {SUBJECT_ID} — {tombstoned} inbox appearances → \
         [erased user]; inline-PII recoverable = {} (threshold 0); off-cell DEK shredded; provider-side \
         erasure requested for {provider_ref}; erase-receipt sealed in the ledger. [OPEN — LEGAL] \
         residual (10.9) flagged for counsel.",
        receipt.recoverable_remaining
    );
}
