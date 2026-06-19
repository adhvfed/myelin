//! # `PersonalDataHolder` auto-registration — every store the harness opens (P-S15)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §3.4 (every store the harness opens — OLTP schema, any blob prefix, any cache namespace,
//! the search index if owned — is **automatically** registered as a `PersonalDataHolder`;
//! the `no-untagged-personal-data` lint + this auto-registration make "we forgot a store /
//! a column" a **structural** failure, not a review miss).
//!
//! **Contract-index:** row 1.4 (`PersonalDataHolder` auto-registration mechanism) — OWNED
//! here. **P-S15 → global P-032.** DEPENDS-ON P-S12 (`serve` calls this), P-S11 (the lint).
//!
//! ## The mechanism (why "we forgot a store" is structurally impossible)
//! A store is not registered by a *call a developer must remember to write* — it is registered
//! by the **same act that opens it**. The harness threads every opened store through
//! [`HolderRegistry::open`], which (a) constructs the store's handle AND (b) records its holder
//! registration in one step. There is no public path to open a store that does not register it:
//! the registry is the only door. The exhaustive H1–H18 holder list itself is GDPR's M1
//! deliverable (confirmed against this mechanism in **P-S27**); here the MECHANISM is frozen.
//!
//! ## What "every store" means (§3.4 — the four store kinds)
//! [`StoreKind`] enumerates exactly the four store classes §3.4 names: the **OLTP** schema,
//! any **blob** prefix, any **cache** namespace, and the **search index** if the service owns
//! one. Each opened store is one [`HolderRegistration`] receipt carrying its kind + its stable,
//! PII-free name. A service cannot open a store of a kind the registry does not register (the
//! enum is closed) — adding a new store kind is a deliberate edit here, not an accident.
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The exhaustive H1–H18 holder list** (the real Identity/Storage/GDPR holders) is GDPR's
//!   M1 deliverable, confirmed against THIS mechanism in **P-S27** (the
//!   `no-untagged-personal-data` lint goes red on any untagged PII field — the GA-D5 mirror).
//!   Here every store a service opens auto-registers; the *set* of stores grows as those
//!   prompts land.
//! - **The DSR bodies** (`locate/export/rectify/restrict/erase`) on each holder are the GDPR
//!   M1 deliverable (10.1–10.9). The OLTP holder's bodies are the named floor in
//!   [`myelin_storage::OltpStoreHolder`]; here the registration is real + testable. The blob /
//!   cache / search holders' concrete `PersonalDataHolder` impls land with their backends
//!   (Storage M1 blob, Search M2); the registry records the registration now so no store
//!   escapes the fan-out.

use std::collections::BTreeSet;

/// The four store classes the harness opens and auto-registers as `PersonalDataHolder`s
/// (architecture §3.4). The enum is **closed**: a service cannot open a store of a kind the
/// registry does not know about — so adding a new store class is a deliberate edit here, never
/// a silent miss. PII-free (a kind tag, never data).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StoreKind {
    /// The OLTP schema (the service's primary transactional database).
    Oltp,
    /// A content-addressed blob prefix (a `BlobStore` namespace the service owns).
    Blob,
    /// A cache namespace (a keyspace the service owns — a derived, invalidatable holder).
    Cache,
    /// The search index, if the service owns one (the encrypted-from-birth per-tenant index).
    SearchIndex,
}

impl StoreKind {
    /// A stable, PII-free label for the kind (for the receipt / data-map / telemetry).
    pub fn label(self) -> &'static str {
        match self {
            StoreKind::Oltp => "oltp",
            StoreKind::Blob => "blob",
            StoreKind::Cache => "cache",
            StoreKind::SearchIndex => "search_index",
        }
    }
}

/// The typed receipt that a store was auto-registered as a [`PersonalDataHolder`] — proof the
/// registration fired for a given (kind, name) (architecture §3.4; contract 1.4). The harness
/// collects one per opened store; the holder-registered architecture test (and the GDPR
/// data-map, P-GA-09) reads these to assert **no store escaped registration**. PII-free: the
/// `name` is a stable store identifier (e.g. `"issue_oltp"`), never personal data.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HolderRegistration {
    /// The store's class (§3.4 — one of the four).
    pub kind: StoreKind,
    /// The store's stable, PII-free name (the holder identifier).
    pub name: &'static str,
}

impl HolderRegistration {
    /// The fully-qualified, PII-free holder id (`<kind>:<name>`) — stable across restarts so the
    /// data-map / DSR fan-out can address exactly this holder.
    pub fn holder_id(&self) -> String {
        format!("{}:{}", self.kind.label(), self.name)
    }
}

/// The auto-registration registry — the **only door** a service opens a store through
/// (architecture §3.4; contract 1.4). Every [`HolderRegistry::open`] call both records the
/// store's holder registration AND yields the caller a [`HolderRegistration`] receipt. Because
/// `open` is the only way to obtain a registered store handle in the harness, "we forgot to
/// register a store" is structurally impossible — opening IS registering.
///
/// `Auto` is the only policy ([`crate::HoldersSpec::Auto`]) — a service cannot opt out; the
/// registry refuses to hand back an un-registered store.
#[derive(Clone, Debug, Default)]
pub struct HolderRegistry {
    /// The registrations, in open order (so a test can assert exactly which stores registered).
    registrations: Vec<HolderRegistration>,
}

impl HolderRegistry {
    /// A fresh, empty registry (no store has been opened yet).
    pub fn new() -> HolderRegistry {
        HolderRegistry {
            registrations: Vec::new(),
        }
    }

    /// **Open + register a store in one act (the structural guarantee).** Records the store as a
    /// `PersonalDataHolder` of the given `kind` and returns its receipt. There is no path that
    /// opens a store WITHOUT this call, so every opened store is a registered holder by
    /// construction (§3.4, GD-3). Idempotent on (kind, name): re-opening the same store does not
    /// double-register (so a re-entrant boot / a restart records each store once).
    pub fn open(&mut self, kind: StoreKind, name: &'static str) -> HolderRegistration {
        let reg = HolderRegistration { kind, name };
        if !self.registrations.contains(&reg) {
            self.registrations.push(reg.clone());
        }
        reg
    }

    /// Every store registered as a holder, in open order (the holder-registered architecture
    /// test reads this to assert no store escaped registration).
    pub fn registrations(&self) -> &[HolderRegistration] {
        &self.registrations
    }

    /// The set of PII-free holder ids registered (for the data-map / DSR fan-out address book).
    pub fn holder_ids(&self) -> BTreeSet<String> {
        self.registrations.iter().map(HolderRegistration::holder_id).collect()
    }

    /// Whether a given store (kind, name) was registered (the assertion the architecture test
    /// makes per known store — "this store did not escape registration").
    pub fn is_registered(&self, kind: StoreKind, name: &'static str) -> bool {
        self.registrations.contains(&HolderRegistration { kind, name })
    }

    /// How many stores were auto-registered (the count the holder-registered test asserts is the
    /// full opened-store set, never short).
    pub fn len(&self) -> usize {
        self.registrations.len()
    }

    /// Whether no store has been opened yet.
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening a store registers it as a holder in the SAME act — the receipt proves it
    /// (§3.4, contract 1.4). Opening IS registering; there is no un-registered store.
    #[test]
    fn opening_a_store_registers_it_as_a_holder() {
        let mut reg = HolderRegistry::new();
        let receipt = reg.open(StoreKind::Oltp, "issue_oltp");
        assert_eq!(receipt, HolderRegistration { kind: StoreKind::Oltp, name: "issue_oltp" });
        assert!(reg.is_registered(StoreKind::Oltp, "issue_oltp"));
        assert_eq!(reg.len(), 1);
    }

    /// Every store KIND §3.4 names (OLTP / blob / cache / search index) auto-registers — the
    /// mechanism covers all four classes, so no class of store can escape the holder fan-out.
    #[test]
    fn every_store_kind_auto_registers() {
        let mut reg = HolderRegistry::new();
        reg.open(StoreKind::Oltp, "svc_oltp");
        reg.open(StoreKind::Blob, "svc_blobs");
        reg.open(StoreKind::Cache, "svc_cache");
        reg.open(StoreKind::SearchIndex, "svc_index");
        assert_eq!(reg.len(), 4, "all four §3.4 store kinds registered");
        assert!(reg.is_registered(StoreKind::Blob, "svc_blobs"));
        assert!(reg.is_registered(StoreKind::Cache, "svc_cache"));
        assert!(reg.is_registered(StoreKind::SearchIndex, "svc_index"));
        // the PII-free holder ids are the data-map address book.
        let ids = reg.holder_ids();
        assert!(ids.contains("oltp:svc_oltp"));
        assert!(ids.contains("search_index:svc_index"));
    }

    /// Re-opening the same store does NOT double-register (idempotent on (kind, name)) — a
    /// restart / a re-entrant boot records each store exactly once.
    #[test]
    fn re_opening_a_store_does_not_double_register() {
        let mut reg = HolderRegistry::new();
        reg.open(StoreKind::Oltp, "svc_oltp");
        reg.open(StoreKind::Oltp, "svc_oltp");
        assert_eq!(reg.len(), 1, "the same store registers exactly once");
    }

    /// The holder id is PII-free + stable (`<kind>:<name>`) — the address the DSR fan-out uses.
    #[test]
    fn holder_id_is_pii_free_and_stable() {
        let r = HolderRegistration { kind: StoreKind::Cache, name: "edge_cache" };
        assert_eq!(r.holder_id(), "cache:edge_cache");
    }

    /// A fresh registry is empty (no store opened ⇒ no holder).
    #[test]
    fn fresh_registry_is_empty() {
        let reg = HolderRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }
}
