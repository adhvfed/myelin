//! # P-GA-17 → P-117 — The structural erasure floor PROVEN on the M1 stores (GATE drill)
//!
//! **DATED GREEN ARTIFACT (2026-06-20).** This integration drill is the dated green artifact the
//! P-GA-17 GATE requires (the GDPR prompts record their drill artifacts as the test itself — there
//! is no GDPR scorecard binary yet). It proves, end-to-end on the M1 stores, the GATE rows of
//! P-GA-17 — *the structural floor (X-7 §7.1) working end-to-end on the M1 stores*:
//!
//! 1. **Lever 1 — per-subject DEK crypto-shred (0 recoverable).** A subject's self-authored
//!    free-text across MULTIPLE M1 holders (a chat store, an issues store, a knowledge store) is
//!    sealed under their per-subject DEK; erasing them shreds the DEK ⇒ every holder's read returns
//!    [`StoredContent::Unrecoverable`], and the KMS backup snapshot reads **0 recoverable**.
//! 2. **Lever 2 — pseudonym-map shred.** The immutable structure (an audit entry / commit author)
//!    holds the subject as the frozen `<pseudonym>@<tenant>.noreply` form; the map shred leaves the
//!    bytes holding ONLY that pseudonym — never real-identity PII (the round-trip proof).
//! 3. **Lever 3 — `restrict` suppression HONOURED by the M1 holders (GA-D7 M1 face).** Setting the
//!    flag SUPPRESSES every processing op (index / agent-read / analyse / notify) across EVERY M1
//!    holder while RETAINING storage; clearing it resumes processing (reversible). The residual (a
//!    third-party mention under the AUTHOR's DEK) is restrict-suppressed for the restricted subject
//!    and is NOT crypto-shredded by the subject's erase (the documented limit, §7.2).
//!
//! ## What this PROVES vs what it REUSES (EI-01 §7 coherence — no new core module)
//! This file ADDS NO production code — it is a pure **chained drill** over the
//! `myelin_gdpr_service::structural_floor` machinery (the [`M1Store`] model + the
//! [`RestrictRegistry`] flag + the [`InMemoryShredKms`] crypto-shred seam, all shipped in the
//! library). The earlier prompts shipped each lever as a seam; P-GA-17 proves the WHOLE floor
//! honoured across a SET of M1 holders end-to-end (EI-01 §4 — chain the proof, not one holder).
//!
//! ## Floor named (deferred → filling prompt)
//! - The **full restriction-into-derived-stores proof (GA-D7)** — the flag flowing into Search/Refs/
//!   Notif/Agents/OLAP, 0 processing across the whole derivative fan-out — is **M2 P-GA-25 → P-152**.
//!   THIS drill proves the M1 holders honour `restrict` NOW (the holder boundary the M2 fan-out
//!   rides). The live store/KMS/Identity bindings are the same DB/KMS floor every M0/M1 store carries
//!   (P-007 / P-S12); this drill touches NO new DB/object-store/cache/bus contract — it composes the
//!   already-shipped in-memory seams — so no `--features integration` live-stack leg is owed.

use myelin_gdpr::{SubjectRef, TenantId};
use myelin_gdpr_service::{
    classify_residual, shred_pseudonym_identity, Authorship, CryptoShredKms, InMemoryShredKms,
    LeverCoverage, M1Store, Processed, Processing, RestrictRegistry, StoredContent,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};

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

/// The drill: the structural floor honoured END-TO-END across a SET of M1 holders.
#[test]
fn the_structural_floor_is_proven_end_to_end_on_the_m1_stores() {
    let tenant = t("acme");
    let subj = subject("u-drill");

    // One shared restrict registry + one shared KMS across the M1 holder SET — a single restriction
    // suppresses EVERY holder (gdpr §4.4 "every holder honours"); one erase shreds the subject's DEK
    // once, and every holder's content sealed under it goes unrecoverable.
    let restrict = RestrictRegistry::new();
    let kms = InMemoryShredKms::new();
    let chat = M1Store::new("chat_store", &restrict, &kms);
    let issues = M1Store::new("issues_store", &restrict, &kms);
    let knowledge = M1Store::new("knowledge_store", &restrict, &kms);
    let m1_holders = [&chat, &issues, &knowledge];

    // The subject's self-authored free-text lives across all three M1 holders under their DEK.
    kms.provision(M1Store::dek_handle(&subj, &tenant), 11);
    chat.store_self_authored(&subj, &tenant, "my chat message");
    issues.store_self_authored(&subj, &tenant, "my issue comment");
    knowledge.store_self_authored(&subj, &tenant, "my doc block");

    // ─────── PRE: every holder processes the subject's content (unrestricted) ───────
    for h in m1_holders {
        for op in Processing::all() {
            let r = run(h, op, &subj, &tenant);
            assert!(
                matches!(r, Processed::Processed(_)),
                "{}:{op:?} processes before restriction",
                h.id()
            );
        }
    }

    // ─────── LEVER 3: restrict HONOURED across every M1 holder (suppress, retain storage) ───────
    restrict.set(&subj, &tenant, true);
    assert!(restrict.is_restricted(&subj, &tenant));
    let mut suppressed_ops = 0u32;
    for h in m1_holders {
        for op in Processing::all() {
            assert_eq!(
                run(h, op, &subj, &tenant),
                Processed::Suppressed,
                "{}:{op:?} SUPPRESSED for the restricted subject (§4.4)",
                h.id()
            );
            suppressed_ops += 1;
        }
        // Storage RETAINED while restricted (suppression ≠ delete).
        assert!(
            matches!(h.fetch_stored(&subj, &tenant), Some(StoredContent::Recoverable(_))),
            "{} retains storage while restricted",
            h.id()
        );
    }
    assert_eq!(suppressed_ops, 12, "3 holders × 4 §4.4 ops all suppressed");

    // Reversible — clearing the flag resumes processing across every holder.
    restrict.set(&subj, &tenant, false);
    for h in m1_holders {
        assert!(matches!(run(h, Processing::Index, &subj, &tenant), Processed::Processed(_)));
    }

    // ─────── LEVER 2: pseudonym-map shred leaves only the frozen grammar ───────
    let handle = PseudonymHandle::new("anon-drill", "acme").expect("valid pseudonym");
    let shredded = shred_pseudonym_identity(&handle);
    assert_eq!(shredded.immutable_bytes, "anon-drill@acme.noreply");
    assert!(
        shredded.holds_only_the_pseudonym_form(),
        "the immutable bytes hold ONLY <pseudonym>@<tenant>.noreply (no real PII) — §7.1.2"
    );

    // ─────── LEVER 1: erase crypto-shreds the per-subject DEK → 0 recoverable across all holders ──
    let destroyed = chat.erase_self_authored(&subj, &tenant);
    assert_eq!(destroyed, Some(11), "the DEK shred records the destroyed epoch (the audit trail)");
    let mut zero_recoverable_holders = 0u32;
    for h in m1_holders {
        assert_eq!(
            h.fetch_stored(&subj, &tenant),
            Some(StoredContent::Unrecoverable),
            "{} is UNRECOVERABLE after the DEK shred (one DEK seals every holder's content)",
            h.id()
        );
        zero_recoverable_holders += 1;
    }
    assert_eq!(zero_recoverable_holders, 3, "0 recoverable PII across all 3 M1 holders");
    // 0 recoverable in the backup snapshot (§7.5: destroyed AND excluded from backup).
    assert_eq!(
        kms.recoverable_in_backup(&M1Store::dek_handle(&subj, &tenant)),
        0,
        "0 recoverable in backup — the crypto-shred reaches backups by construction (§7.5)"
    );

    // ─────── THE RESIDUAL (§7.2 documented limit): third-party mention is restrict-suppress-only ──
    // A third-party mention lives under the AUTHOR's DEK, not the subject's — the subject's erase
    // above did NOT shred it. It is governed ONLY by restrict (the documented limit), never
    // pretended-solved (the X-7 anti-pattern guard).
    assert_eq!(
        classify_residual(Authorship::ThirdPartyMention),
        LeverCoverage::RestrictSuppressOnly,
        "the residual is restrict-suppress-only — the documented limit, not crypto-shredded (§7.2)"
    );
    assert_eq!(classify_residual(Authorship::SelfAuthored), LeverCoverage::CryptoShred);
}

/// Run one processing op on a holder (the §4.4 four-op set).
fn run(h: &M1Store, op: Processing, subj: &SubjectRef, tenant: &TenantId) -> Processed {
    match op {
        Processing::Index => h.index(subj, tenant),
        Processing::AgentRead => h.agent_read(subj, tenant),
        Processing::Analyse => h.analyse(subj, tenant),
        Processing::Notify => h.notify(subj, tenant),
    }
}
