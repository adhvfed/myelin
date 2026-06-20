//! # P-GA-24 → P-151 — The per-derivative erasure fan-out GATE drill (GA-D2 + REF-D5 + NOTIF-D6)
//!
//! **DATED GREEN ARTIFACT (2026-06-20).** This integration drill is the dated green artifact the
//! P-GA-24 GATE requires (the GDPR prompts record their drill artifacts as the test itself — there
//! is no GDPR scorecard binary yet). It proves, end-to-end over the M2 derived stores, the GATE rows:
//!
//! 1. **GA-D2 (GA+SRCH) — the subject's docs AND embeddings purged+reindexed out, NOT hidden.** A
//!    subject is indexed in Search (doc projection + embedding). Erasing them PURGES the doc AND
//!    compacts the embedding out of the doc-id space — a **re-identification probe returns 0**
//!    (`0 hits, 0 embedding re-identification`). A *hidden* doc would leave the embedding re-
//!    identifiable; the model has no hide path. The **embedding-purge receipt** is the green artifact.
//! 2. **REF-D5 (REF) — refs tombstone, 0 recoverable PII, no resolve-500.** A subject's edge resolves
//!    Live before erase; erasing them TOMBSTONES the edge — a resolve returns the tombstone (`0
//!    recoverable`), it does **NOT 500**.
//! 3. **NOTIF-D6 (NOTIF) — inbox humanises to `[erased user]`.** An inbox item mentions the subject;
//!    erasing them humanises the mention to `[erased user]` (never PII, never a 500).
//! 4. **Rectification fans out via reindex-from-source (§4.4).** Correcting the source REBUILDS the
//!    derived projection (drift = 0) — never patched-in-place.
//!
//! ## What this PROVES vs what it REUSES (EI-01 §7 coherence — no new core module)
//! This file ADDS NO production code — it is a pure **chained drill** over the
//! `myelin_gdpr_service::derivative_erasure` machinery (the faithful Search/Refs/Notif models + their
//! `PersonalDataHolder` seams + the [`DerivativeErasureDriver`], all shipped in the library). The
//! derivative holders register through the SAME `RegisteredHolder` seam the upstream orchestration
//! uses (P-GA-06) — this drill proves the per-derivative ERASE + RECTIFY honoured across the SET of
//! derived stores end-to-end (EI-01 §4 — chain the proof, not one holder).
//!
//! ## Floors named (deferred → filling prompt)
//! - The **`restrict` suppression INTO these same derived stores (GA-D7)** — the flag flowing into
//!   Search/Refs/Notif/Agents/OLAP, 0 processing across the whole derivative fan-out — is
//!   **M2 P-GA-25 → P-152** (this drill proves the per-derivative ERASE + RECTIFY; the restriction
//!   rides this fan-out).
//! - The **agent-trace H17 seam** the per-derivative erase reaches → **M2 P-GA-26 → P-153**.
//! - The **live Search/Refs/Notif `erase` bindings** behind the seam are a config swap at boot; the
//!   models here have byte-for-byte the GA-D2/REF-D5/NOTIF-D6 post-conditions. No new DB/object-store/
//!   cache/bus contract is touched — **no `--features integration` leg owed**.

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    DerivativeErasureDriver, NotifHistoryHolder, NotifHistoryModel, RefsGraphHolder, RefsGraphModel,
    RefsResolve, SearchIndexHolder, SearchIndexModel, ERASED_USER,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

fn scope(id: &str) -> EraseScope {
    EraseScope::Subject { subject: subject(id), tenant: TenantId::from_token("acme") }
}

/// **The P-GA-24 GATE: GA-D2 + REF-D5 + NOTIF-D6, chained end-to-end over the derived stores.** A
/// subject seeded into Search (doc + embedding), Refs (an edge), and Notif (an inbox mention) → the
/// per-derivative fan-out erases all three through the contract → 0 search hits + 0 embedding
/// re-identification (GA-D2), the ref tombstones with no resolve-500 (REF-D5), the inbox humanises to
/// `[erased user]` (NOTIF-D6). The embedding-purge receipt is the dated green artifact.
#[test]
fn ga_d2_ref_d5_notif_d6_derivative_erasure_fan_out_is_green() {
    let search = SearchIndexModel::new();
    let refs = RefsGraphModel::new();
    let notif = NotifHistoryModel::new();

    // Seed the subject across the three derived stores (FROM SOURCE — the live indexers' step).
    search.index_from_source("victim", "victim@example.com");
    refs.add_edge_from_source("victim", "issue:99");
    notif.add_item_from_source("inbox-1", "victim");
    notif.add_item_from_source("inbox-2", "bystander");

    // BEFORE erase: indexed + re-identifiable, edge resolves Live, mention renders the token.
    assert_eq!(search.hits("victim"), 1, "indexed before erase");
    assert_eq!(search.reidentify_hits("victim"), 1, "embedding re-identifies before erase");
    assert_eq!(refs.resolve("victim"), RefsResolve::Live("issue:99".into()));
    assert_eq!(notif.render_mention("inbox-1").as_deref(), Some("victim"));

    let sh = SearchIndexHolder::new(&search);
    let rh = RefsGraphHolder::new(&refs);
    let nh = NotifHistoryHolder::new(&notif);

    // FAN OUT the per-derivative erase (the orchestration leg — each via the contract, no store reach).
    let receipt = DerivativeErasureDriver::fan_out_erase(
        &scope("victim"),
        &search,
        &sh as &dyn PersonalDataHolder,
        &refs,
        &rh as &dyn PersonalDataHolder,
        &notif,
        &nh as &dyn PersonalDataHolder,
    )
    .expect("the per-derivative fan-out succeeds");

    // ── GA-D2: docs AND embeddings purged (not hidden) — 0 hits, 0 embedding re-identification.
    assert_eq!(search.hits("victim"), 0, "GA-D2: 0 search hits after purge");
    assert_eq!(
        search.reidentify_hits("victim"),
        0,
        "GA-D2: 0 embedding re-identification (purged, NOT hidden) — the measured number"
    );
    assert!(receipt.embeddings_purged, "GA-D2: the embedding-purge receipt records the purge");

    // ── REF-D5: refs tombstone, 0 recoverable PII, no resolve-500.
    assert_eq!(
        refs.resolve("victim"),
        RefsResolve::Tombstone,
        "REF-D5: a resolve returns the tombstone, NOT a 500"
    );
    assert_eq!(refs.recoverable_edges("victim"), 0, "REF-D5: 0 recoverable edges");
    assert!(receipt.refs_tombstoned, "REF-D5: the receipt records the tombstone");

    // ── NOTIF-D6: the inbox humanises to `[erased user]`; other mentions are untouched.
    assert_eq!(
        notif.render_mention("inbox-1").as_deref(),
        Some(ERASED_USER),
        "NOTIF-D6: the erased subject's mention humanises to [erased user]"
    );
    assert_eq!(notif.render_mention("inbox-1").as_deref(), Some("[erased user]"));
    assert_eq!(
        notif.render_mention("inbox-2").as_deref(),
        Some("bystander"),
        "a bystander's mention is untouched (only the erased subject humanises)"
    );

    // The green artifact: the embedding-purge receipt records all three post-conditions + the
    // per-holder receipts (Search + Refs + Notif), each content-addressed.
    assert_eq!(receipt.holder_receipts.len(), 3, "Search + Refs + Notif receipts collected");
    for hr in &receipt.holder_receipts {
        assert!(hr.receipt.content_hash.starts_with("blake3:"), "each derivative receipt is content-addressed");
    }
}

/// **Rectification fans out via reindex-from-source (§4.4 — never patched-in-place).** Correcting the
/// source REBUILDS the Search projection + the Refs edge from the corrected source; the derived value
/// equals the rebuilt value (drift = 0). There is NO patch entry point — the structural foreclosure
/// of patch-and-drift.
#[test]
fn rectification_via_reindex_from_source_rebuilds_drift_is_zero() {
    let search = SearchIndexModel::new();
    let refs = RefsGraphModel::new();
    // The stale projection from the original (incorrect) source.
    search.index_from_source("subj", "wrong name");
    refs.add_edge_from_source("subj", "wrong-target");

    // Art. 16: the source is corrected → the derived stores REBUILD from source (reindex), not patch.
    let outcome = DerivativeErasureDriver::rectify_via_reindex_from_source(
        "subj",
        "corrected name",
        "corrected-target",
        &search,
        &refs,
    );

    // The derived value equals the REBUILT (corrected-source) value — drift = 0, never patched-in-place.
    assert_eq!(
        outcome.search_projection.as_deref(),
        Some("corrected name"),
        "Search reindexed from the corrected source"
    );
    assert_eq!(
        outcome.refs_target.as_deref(),
        Some("corrected-target"),
        "Refs rebuilt the edge from the corrected source"
    );
    assert_eq!(search.projection("subj").as_deref(), Some("corrected name"));
    assert_eq!(refs.resolve("subj"), RefsResolve::Live("corrected-target".into()));
}
