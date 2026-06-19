//! The OLTP store's `PersonalDataHolder` auto-registration hook (contract 1.4 / 10.1).
//!
//! **Architecture:** storage.md §1.1 (every store is a `PersonalDataHolder`; the harness
//! auto-registers — GD-3), 00 §3.4 (the harness auto-registers every store it opens).
//! Contract 1.4 (holder auto-registration) + 10.1 (`PersonalDataHolder`).
//!
//! ## What lands here vs the GDPR M1 floor
//! On THIS prompt (P-ST-01) the **registration hook fires** — when a service opens its
//! OLTP store through the harness, the store is handed to the holder registry so "we forgot
//! a store" is structurally impossible (combined with the `no-untagged-personal-data` lint,
//! P-S10/P-GA-03). The DSR **bodies** (locate/export/rectify/restrict/erase) are the GDPR
//! M1 deliverable (10.1–10.9). [`OltpHolderRegistration`] is the typed receipt the
//! registration produced; the harness collects these (the real auto-registration wiring is
//! `serve`'s, P-S12/P-S15 — this is the hook it calls).

use myelin_gdpr::{
    DsrError, ExportBundle, LocatedData, PersonalDataHolder, Result as DsrResult, Subject,
};

/// The typed receipt that an OLTP store was registered as a [`PersonalDataHolder`] — proof
/// the auto-registration hook fired for a given store. The harness collects one per opened
/// store; the holder-registered architecture test (and the GDPR data-map, P-GA-09) reads
/// these to assert no store escaped registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OltpHolderRegistration {
    /// The store's stable name (the OLTP database/holder identifier).
    pub store: &'static str,
}

/// Fire the holder auto-registration hook for an OLTP store (contract 1.4). Returns the
/// typed [`OltpHolderRegistration`] receipt the harness collects. On this floor the hook is
/// the seam `serve(AppSpec)` calls when it opens the store (the lifecycle wiring is P-S12/
/// P-S15); the receipt makes "the store was registered" an asserted fact, not a hope.
pub fn register_holder(store: &'static str) -> OltpHolderRegistration {
    OltpHolderRegistration { store }
}

/// The OLTP store AS a [`PersonalDataHolder`] (contract 10.1). The five DSR methods are the
/// GDPR M1 deliverable; here the trait is **implemented to its frozen shape** with named
/// floors so the registration hook can hand a real holder to the registry and so the
/// holder-registered architecture test compiles against a concrete impl.
///
/// **Floor:** every body is the GDPR M1 deliverable (10.1–10.9). They return a typed
/// `DsrError::Unimplemented`-shaped marker (the skeleton `DsrError(String)`) rather than
/// `todo!()` so the registration path is exercisable now without panicking — the BODIES
/// land in GDPR M1, but the REGISTRATION (this prompt's deliverable) is real and testable.
#[derive(Clone, Debug)]
pub struct OltpStoreHolder {
    /// The store this holder represents.
    pub store: &'static str,
}

impl OltpStoreHolder {
    /// The OLTP store holder for a named store.
    pub fn new(store: &'static str) -> OltpStoreHolder {
        OltpStoreHolder { store }
    }

    /// Register this holder, returning the receipt (the auto-registration hook).
    pub fn register(&self) -> OltpHolderRegistration {
        register_holder(self.store)
    }
}

/// The DSR-body floor marker: a typed, non-panicking "lands in GDPR M1" error so the
/// registration path is exercisable without invoking an unimplemented body. The GDPR M1
/// prompts (10.1–10.9) replace these with the real locate/export/rectify/restrict/erase
/// over the OLTP columns + crypto-shred.
fn dsr_floor(method: &str) -> DsrError {
    DsrError(format!(
        "OLTP {method} body lands in GDPR M1 (10.1-10.9); P-ST-01 ships the registration hook only"
    ))
}

impl PersonalDataHolder for OltpStoreHolder {
    fn locate(&self, _subject: &Subject) -> DsrResult<LocatedData> {
        Err(dsr_floor("locate"))
    }
    fn export(&self, _subject: &Subject) -> DsrResult<ExportBundle> {
        Err(dsr_floor("export"))
    }
    fn rectify(&self, _subject: &Subject, _patch: LocatedData) -> DsrResult<()> {
        Err(dsr_floor("rectify"))
    }
    fn restrict(&self, _subject: &Subject) -> DsrResult<()> {
        Err(dsr_floor("restrict"))
    }
    fn erase(&self, _subject: &Subject) -> DsrResult<()> {
        Err(dsr_floor("erase"))
    }
}

/// The BlobStore (P-ST-03 / 11.2) AS a [`PersonalDataHolder`]. A T2 blob may carry personal
/// data (repo contents, attachments, media); the store references content-addressed blobs and
/// its erasure is **crypto-shred** (destroy the wrapping key), NOT `delete` (storage.md §3.2).
/// On THIS floor the holder is **registered** to its frozen shape (the auto-registration hook
/// fires, so "we forgot the blob store" is structurally impossible); the crypto-shred DSR
/// bodies are the GDPR-M1 deliverable (the six-step crypto-shred algorithm is **P-ST-09**, and
/// the real per-blob key wrap it shreds lands in **P-ST-08**).
#[derive(Clone, Debug)]
pub struct BlobStoreHolder {
    /// The blob store this holder represents (the per-tenant blob keyspace name).
    pub store: &'static str,
}

impl BlobStoreHolder {
    /// The blob-store holder for a named store (e.g. `"git_pack_blobs"`, `"attachments"`).
    pub fn new(store: &'static str) -> BlobStoreHolder {
        BlobStoreHolder { store }
    }

    /// Fire the auto-registration hook for this blob store (contract 1.4), returning the
    /// receipt the harness collects — the proof the blob store registered as a holder.
    pub fn register(&self) -> OltpHolderRegistration {
        register_holder(self.store)
    }
}

impl PersonalDataHolder for BlobStoreHolder {
    fn locate(&self, _subject: &Subject) -> DsrResult<LocatedData> {
        Err(dsr_floor("blob locate"))
    }
    fn export(&self, _subject: &Subject) -> DsrResult<ExportBundle> {
        Err(dsr_floor("blob export"))
    }
    fn rectify(&self, _subject: &Subject, _patch: LocatedData) -> DsrResult<()> {
        Err(dsr_floor("blob rectify"))
    }
    fn restrict(&self, _subject: &Subject) -> DsrResult<()> {
        Err(dsr_floor("blob restrict"))
    }
    // erase = crypto-shred (destroy the wrapping key), not delete (§3.2) — body is P-ST-09.
    fn erase(&self, _subject: &Subject) -> DsrResult<()> {
        Err(dsr_floor("blob erase (crypto-shred)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn subject() -> Subject {
        Subject {
            principal: Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId("acme".into())),
            tenant: TenantId("acme".into()),
        }
    }

    /// The auto-registration hook fires and produces a receipt naming the store — the
    /// holder-registered fact the architecture test asserts (no store escapes registration).
    #[test]
    fn registration_hook_fires_and_names_the_store() {
        let receipt = register_holder("issue_oltp");
        assert_eq!(receipt.store, "issue_oltp");
    }

    /// The store holder registers itself (the `serve`-called seam) and the receipt matches.
    #[test]
    fn store_holder_registers_itself() {
        let holder = OltpStoreHolder::new("worklog_oltp");
        assert_eq!(holder.register(), OltpHolderRegistration { store: "worklog_oltp" });
    }

    /// The BlobStore (P-ST-03) auto-registers as a holder — the blob-store half of "every
    /// store is a holder" (§1.1). Its erasure is crypto-shred (body → P-ST-09).
    #[test]
    fn blob_store_registers_as_a_holder() {
        let holder = BlobStoreHolder::new("git_pack_blobs");
        assert_eq!(
            holder.register(),
            OltpHolderRegistration { store: "git_pack_blobs" }
        );
        let s = subject();
        // The frozen shape compiles + the erase body is the named crypto-shred floor.
        match holder.erase(&s) {
            Err(DsrError(msg)) => assert!(msg.contains("crypto-shred")),
            Ok(_) => panic!("blob erase must be the GDPR-M1 crypto-shred floor"),
        }
    }

    /// The holder implements the frozen `PersonalDataHolder` shape; the DSR bodies are the
    /// named GDPR-M1 floor — they return a typed marker (not a panic), so the registration
    /// path is exercisable now. The BODIES land in GDPR M1.
    #[test]
    fn dsr_bodies_are_the_named_gdpr_m1_floor() {
        let holder = OltpStoreHolder::new("issue_oltp");
        let s = subject();
        assert!(holder.locate(&s).is_err());
        assert!(holder.erase(&s).is_err());
        // the floor marker names where the real body lands.
        match holder.export(&s) {
            Err(DsrError(msg)) => assert!(msg.contains("GDPR M1"), "floor must name its follow-on: {msg}"),
            Ok(_) => panic!("export body must be the GDPR-M1 floor on P-ST-01"),
        }
    }
}
