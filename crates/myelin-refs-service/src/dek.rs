//! The Refs per-tenant DEK pin into the KMS hierarchy + the per-subject backstop (REF-P4 / P-121;
//! contracts 11.3 / 11.4 consumed).
//!
//! **Owning architecture doc:** `reference-graph.md` §3 (every Refs store is per-tenant
//! envelope-encrypted under the KMS hierarchy; the per-tenant DEK is the tenant-decommission
//! crypto-shred unit), §3.6 (the R2 projection cache "may hold a name in a title" — the per-subject
//! DEK backstop so a cached title naming an erased subject is crypto-shred-able at subject grain).
//! **Reconciliation / VISION §3:** GDPR-safe by construction; name-your-floors.
//!
//! **Contract-index rows 11.3 / 11.4 (consumed, NOT re-implemented):** the three-level KMS hierarchy
//! `per-cell root (L0) → per-tenant KEK (L1) → per-tenant DEK (L2)` + the per-subject DEK backstop +
//! crypto-shred — all frozen in [`myelin_storage::KmsEngine`] (storage P-058). REF-P4 PINS the Refs
//! key classes INTO that one engine. It does **not** stand up a second KMS (no parallel crypto): the
//! whole point of the hierarchy is that one cell root governs every store's keys, so Refs reserves
//! ITS classes in the SAME engine the cell's other stores resolve through.
//!
//! ## What REF-P4 (P-121) ships — the per-tenant DEK reservation
//! No edge index / R2 cache exists yet (the schema is REF-P5, the cache REF-P12). So this prompt
//! **reserves the key CLASS** so R-M2's index is **encrypted-from-birth** and proves **destroy is
//! callable** on it:
//! - [`RefsDekPin::reserve`] provisions, under the cell's [`KmsEngine`], the per-`(tenant, region)`
//!   KEK (L1) and the **per-tenant DEK** (L2, [`KeyClass::Tenant`]) — THE Refs crypto-shred /
//!   backup-backstop unit. The returned [`PiiKeyRef`] (`kms://<tenant>/<epoch>/tenant`) is the ref
//!   the REF-P5 edge table + the REF-P12 R2 cache will travel with every ciphertext (the
//!   `encrypted-from-birth` anchor).
//! - The **per-subject DEK backstop** ([`KeyClass::Subject`], §3.6) is RESERVED on demand for a name
//!   that lands in a cached title: [`RefsDekPin::reserve_subject_backstop`] provisions a distinct
//!   per-subject DEK so that subject's cached title is crypto-shred-able WITHOUT touching the tenant
//!   DEK (the GD-4 individual lever). It is the backstop the §3.6 "name in a title" case needs.
//! - **Destroy is callable** on both classes — [`RefsDekPin::destroy_tenant_dek`] (the
//!   tenant-decommission shred) and [`RefsDekPin::destroy_subject_backstop`] (the per-subject shred)
//!   — proven STRUCTURALLY here (the key class exists + destroy returns true once + is then
//!   unrecoverable). The *real* crypto-shred OVER REAL INDEX DATA (REF-D5) is REF-P15/REF-P25; here
//!   the check is that the lever exists and fires.
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **THE floor is the per-tenant DEK as the crypto-shred + backup-backstop UNIT** — reserved +
//!   destroyable here. The structural erasure SURFACE that USES the DEK (purge R2-cache PII + reindex
//!   tombstones + reliance on Identity's pseudonym shred for `origin_actor`) is the follow-on,
//!   landing in **REF-P15** once the edge index exists. The DEK is NOT the whole erasure answer —
//!   it is the key class that makes the answer crypto-shred-able.
//! - **No index / cache / migration / real ciphertext ships here.** The edge schema is REF-P5, the
//!   R2 cache REF-P12, the real erase body REF-P15, the world-scale shred drill REF-P25. This prompt
//!   reserves keys; nothing is encrypted yet (there is no data to encrypt).
//!
//! ## The inherited M1 platform gates (named as the REF-P5 precondition — GATE/DRILLS)
//! Refs does NOT re-prove the platform's M1 gates; it INHERITS them and cannot begin its M2 edge
//! engine (REF-P5) over a red one. They are named here as machine-readable facts ([`InheritedGate`] /
//! [`ref_p5_inherited_gates`]) so the REF-P5 agent has the precondition list in code, not prose:
//! STOR-D1/D2 (restore-verify), ID-D3 (cross-tenant 0), ID-D2 (fail-static), ID-D1 (disabled-user
//! N≥5 min), CP-D2/CP-D3 (misroute + residency-pin). The `DEPENDS-ON` edge (REF-P5 → REF-P4 + M1
//! fully green) makes this concrete; Refs reads them, never re-runs them.

use std::sync::Arc;

use myelin_storage::{DekId, KekId, KeyClass, KmsEngine, KmsError, PiiKeyRef};
use myelin_tenancy::{Region, TenantId};

/// The Refs DEK pin — reserves the Refs key classes in the cell's one [`KmsEngine`] (contract
/// 11.3 / 11.4) so the (future) edge index + R2 cache are **encrypted-from-birth** and the
/// per-tenant DEK is a **destroyable** crypto-shred unit.
///
/// It holds an `Arc<KmsEngine>` — the SAME engine the cell's other stores resolve DEKs through
/// (there is ONE cell root governing one hierarchy; Refs does not own a second KMS). At REF-P4 no
/// store opens at runtime; `serve` will hold this pin and provision the DEK when the edge schema
/// lands (REF-P5+). The pin proves the key class exists + destroy fires.
#[derive(Clone)]
pub struct RefsDekPin {
    /// The cell-wide KMS engine (the per-cell root → per-tenant KEK → per-tenant DEK hierarchy,
    /// storage P-058). Shared, not Refs-owned — the one engine the whole cell uses.
    kms: Arc<KmsEngine>,
}

impl RefsDekPin {
    /// Build a Refs DEK pin over the cell's KMS engine (the SAME `Arc<KmsEngine>` the other cell
    /// stores share — one cell root, one hierarchy).
    pub fn new(kms: Arc<KmsEngine>) -> RefsDekPin {
        RefsDekPin { kms }
    }

    /// The Refs **per-tenant DEK class** (contract 11.4) — [`KeyClass::Tenant`]. The bulk content of
    /// the Refs edge index and R2 cache (pseudonymous opaque ids + derived projection structure)
    /// seals under the per-tenant DEK; destroying it is **tenant-decommission crypto-shred** (the
    /// tenant-offboard lever). THE floor unit this prompt reserves.
    pub fn tenant_dek_class() -> KeyClass {
        KeyClass::Tenant
    }

    /// The Refs **per-subject DEK backstop class** (contract 11.4, §3.6) — `subject:<id>`, DISTINCT
    /// from the per-tenant DEK. Reserved for the §3.6 "a name in a cached title" case: a cached
    /// projection title that names a subject is sealed under THAT subject's DEK, so erasing the
    /// subject crypto-shreds exactly their cached title (the GD-4 individual lever) without touching
    /// the tenant DEK. The id is the pseudonymous opaque `origin_actor` / principal id (never a
    /// name — EI-04 §1).
    pub fn subject_dek_class(subject_id: &str) -> KeyClass {
        KeyClass::Subject(subject_id.to_string())
    }

    /// **Reserve the per-tenant Refs DEK in the KMS hierarchy (11.3 / 11.4).** Provisions the L1
    /// per-`(tenant, region)` KEK (idempotently) and the L2 per-tenant DEK ([`KeyClass::Tenant`])
    /// under it, returning the [`PiiKeyRef`] (`kms://<tenant>/<epoch>/tenant`) that the REF-P5 edge
    /// table and REF-P12 cache will travel with every ciphertext (the **encrypted-from-birth**
    /// anchor). Idempotent: a second call returns the same key ref (the DEK is not silently rotated).
    ///
    /// This is the floor unit — the per-tenant DEK as the crypto-shred + backup-backstop unit. No
    /// data is encrypted yet (no index exists); this RESERVES the key so the index is
    /// encrypted-from-birth and the tenant-offboard lever is in place from day one.
    pub fn reserve(&self, tenant: &TenantId, region: &Region) -> Result<PiiKeyRef, KmsError> {
        // L1: provision the per-(tenant, region) KEK (idempotent — never rotates an existing one).
        self.kms
            .ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        // L2: reserve the PER-TENANT DEK under it — the Refs bulk crypto-shred unit. The returned
        // ref is what the REF-P5 edge table / REF-P12 cache key on (encrypted-from-birth).
        self.kms
            .ensure_dek(tenant, region, Self::tenant_dek_class())
    }

    /// **Reserve the per-subject DEK backstop for a name landing in a cached title (§3.6 / 11.4).**
    /// Provisions a DISTINCT per-subject DEK so the subject's cached title is crypto-shred-able at
    /// subject grain (the GD-4 individual lever) — without touching the tenant DEK. Requires the L1
    /// KEK (provisioned via [`Self::reserve`] or here). Idempotent per `(tenant, subject)`.
    pub fn reserve_subject_backstop(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject_id: &str,
    ) -> Result<PiiKeyRef, KmsError> {
        self.kms
            .ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        self.kms
            .ensure_dek(tenant, region, Self::subject_dek_class(subject_id))
    }

    /// **Tenant-decommission crypto-shred (destroy is callable on the per-tenant key class).**
    /// Destroys the tenant KEK ⇒ EVERY Refs DEK under it (the per-tenant DEK + every per-subject
    /// backstop) is unrecoverable, live AND in every backup (§7.5: a shredded key is excluded from
    /// backup, so a restore never resurrects it). Returns `true` if a KEK was present to destroy.
    ///
    /// This is the STRUCTURAL proof REF-P4 owes: the lever EXISTS and FIRES. The real crypto-shred
    /// over real Refs index data (REF-D5, 0 recoverable) is REF-P15 / REF-P25 — it cannot be proven
    /// here because no index data exists yet (named floor).
    pub fn destroy_tenant_dek(&self, tenant: &TenantId, region: &Region) -> bool {
        self.kms
            .destroy_kek(&KekId::new(tenant.clone(), region.clone()))
    }

    /// **Is the per-subject DEK backstop for `subject_id` STILL LIVE (resolvable)?** A READ-ONLY probe
    /// (it does NOT reserve/resurrect the key — unlike [`Self::reserve_subject_backstop`]) — so a
    /// post-restore re-erase drill (REF-P25) can assert "0 resurrected per-subject DEKs" WITHOUT itself
    /// resurrecting the very key it is checking. Resolves the deterministic `kms://<tenant>/0/subject:<id>`
    /// ref; `true` iff it resolves (the cached title sealed under it is still decryptable), `false` iff
    /// it is crypto-shredded (LOUD `KmsError` → the title is unrecoverable). PII-free: an opaque id probe.
    pub fn subject_backstop_is_live(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject_id: &str,
    ) -> bool {
        // The per-subject DEK epoch is 0 by construction (ensure_dek mints at epoch 0; Refs never
        // rotates a subject backstop). Build the deterministic ref WITHOUT reserving, then probe.
        let key_ref = PiiKeyRef::new(tenant.clone(), 0, Self::subject_dek_class(subject_id));
        self.kms.resolve_dek(&key_ref, region).is_ok()
    }

    /// **Per-subject crypto-shred (destroy is callable on the per-subject backstop class).**
    /// Destroys ONE subject's backstop DEK ⇒ that subject's cached-title ciphertext is unrecoverable
    /// while the tenant DEK + every other subject is untouched (the GD-4 individual lever). Returns
    /// `true` if the backstop DEK was present to destroy.
    pub fn destroy_subject_backstop(&self, tenant: &TenantId, subject_id: &str) -> bool {
        self.kms.destroy_dek(&DekId::new(
            tenant.clone(),
            Self::subject_dek_class(subject_id),
        ))
    }

    /// Resolve the per-tenant DEK named by `key_ref` (the read-path key-resolution step the REF-P5
    /// edge reads / REF-P12 cache reads will call). A destroyed-key / wrong-key resolve fails LOUDLY
    /// ([`KmsError`]) — **never** a plaintext-without-key fall-through (the 0-fail-open invariant the
    /// storage engine enforces). Exposed so the REF-P5 read path resolves through the SAME pin.
    pub fn resolve(
        &self,
        key_ref: &PiiKeyRef,
        region: &Region,
    ) -> Result<myelin_storage::DekHandle, KmsError> {
        self.kms.resolve_dek(key_ref, region)
    }

    /// Borrow the underlying shared engine (so `serve` wires the SAME engine into the edge store +
    /// cache when REF-P5 lands — one cell root, one hierarchy, never a second KMS).
    pub fn engine(&self) -> &Arc<KmsEngine> {
        &self.kms
    }
}

impl std::fmt::Debug for RefsDekPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The engine redacts all key material; this only names the pin (never key bytes).
        f.debug_struct("RefsDekPin")
            .field("kms", &self.kms)
            .finish()
    }
}

// ───────────────────────── the inherited M1 platform gates (the REF-P5 precondition) ─────────────

/// One inherited M1 platform gate Refs depends on but does NOT re-prove — the precondition for
/// REF-P5 (the edge engine). Named in code (not prose) so the REF-P5 agent reads the precondition
/// list machine-readably. Refs cannot build the edge index over a red one of these (the `DEPENDS-ON`
/// edge makes it concrete). PII-free: a gate id + a one-line description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InheritedGate {
    /// The gate's stable id (e.g. `STOR-D1`, `ID-D3`).
    pub id: &'static str,
    /// What the gate guarantees (the precondition Refs leans on).
    pub guarantees: &'static str,
}

/// The inherited M1 platform gates that must be green before REF-P5 (the edge inverse-index
/// migration) may begin. Refs INHERITS these — it does not re-run them; a regression on any halts
/// REF-P5 (the index cannot be built over a red restore-verify / cross-tenant / fail-static /
/// disabled-user / misroute / residency gate). This is the GATE/DRILLS "name them as the REF-P5
/// precondition" requirement, in code.
pub fn ref_p5_inherited_gates() -> Vec<InheritedGate> {
    vec![
        InheritedGate {
            id: "STOR-D1",
            guarantees: "restore-verify: a restored copy is byte-faithful + the cross-seam \
                         consistency point holds (the permanent store gate; the edge index cannot \
                         be built over an unrestorable store)",
        },
        InheritedGate {
            id: "STOR-D2",
            guarantees: "cell-kill RTO restore-verify: a cell can be rebuilt within RTO from \
                         backups (the permanent store gate; re-run on every store-touching change)",
        },
        InheritedGate {
            id: "ID-D3",
            guarantees: "cross-tenant authz = 0: no check ever leaks across tenants (Refs' \
                         per-viewer resolution leans on this; a cross-tenant edge read is impossible)",
        },
        InheritedGate {
            id: "ID-D2",
            guarantees: "fail-static authz: a KMS/Identity hiccup degrades to bounded-staleness, \
                         never fail-open (Refs' DEK resolve + ACL filter inherit this posture)",
        },
        InheritedGate {
            id: "ID-D1",
            guarantees: "disabled-user revocation within N≥5 min: a revoked principal stops \
                         resolving edges (Refs' per-viewer chokepoint inherits the revocation SLA)",
        },
        InheritedGate {
            id: "CP-D2",
            guarantees: "misroute rejection: a request to the wrong cell is rejected, never served \
                         (Refs state is cell-local; a cross-cell edge read is impossible)",
        },
        InheritedGate {
            id: "CP-D3",
            guarantees: "residency-pin: no cross-region read path (the Refs edge table + R2 cache \
                         are residency-pinned; the per-tenant DEK is region-scoped via the KEK)",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kms() -> Arc<KmsEngine> {
        Arc::new(KmsEngine::new())
    }
    fn t() -> TenantId {
        TenantId("acme".into())
    }
    fn r() -> Region {
        Region("fr-par".into())
    }

    /// **The per-tenant Refs DEK class is reserved in the KMS hierarchy + the ref is the
    /// encrypted-from-birth anchor (11.3).** `reserve` provisions the L1 KEK + the L2 per-tenant DEK
    /// and returns `kms://<tenant>/<epoch>/tenant` — the ref the REF-P5 edge table / REF-P12 cache
    /// will key on. The DEK is a real, resolvable key (a payload sealed under it round-trips).
    #[test]
    fn per_tenant_dek_is_reserved_and_resolvable() {
        let pin = RefsDekPin::new(kms());
        let key_ref = pin
            .reserve(&t(), &r())
            .expect("reserve the per-tenant Refs DEK");
        assert_eq!(
            key_ref.class,
            KeyClass::Tenant,
            "the Refs bulk class is per-tenant"
        );
        assert_eq!(
            key_ref.to_uri(),
            "kms://acme/0/tenant",
            "the encrypted-from-birth key ref"
        );

        // The reserved DEK is a REAL key: a payload sealed under it round-trips (encrypted-from-birth
        // is not a stub — the key is usable from the moment the index lands).
        let dek = pin
            .resolve(&key_ref, &r())
            .expect("resolve the reserved per-tenant DEK");
        let (nonce, ct) = dek.seal(b"a future edge row's bulk column");
        assert_eq!(
            dek.open(&nonce, &ct).as_deref(),
            Some(&b"a future edge row's bulk column"[..])
        );
    }

    /// **Reserving the per-tenant DEK is idempotent** — `serve` re-running the reservation on a
    /// restart returns the SAME key ref (it does NOT silently rotate the DEK, which would orphan
    /// existing ciphertext).
    #[test]
    fn reserve_is_idempotent() {
        let pin = RefsDekPin::new(kms());
        let a = pin.reserve(&t(), &r()).expect("first reserve");
        let b = pin.reserve(&t(), &r()).expect("second reserve");
        assert_eq!(
            a, b,
            "re-reserving returns the same per-tenant DEK ref (no silent rotation)"
        );
    }

    /// **The per-subject backstop is a DISTINCT key from the per-tenant DEK (§3.6 / GD-4).** A name
    /// in a cached title seals under the subject's backstop; a payload sealed under it does NOT open
    /// under the tenant DEK — so destroying the subject backstop erases exactly that one cached
    /// title, the tenant untouched.
    #[test]
    fn per_subject_backstop_is_distinct_from_the_tenant_dek() {
        let pin = RefsDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant dek");
        let sk = pin
            .reserve_subject_backstop(&t(), &r(), "u-1")
            .expect("subject backstop");
        assert_ne!(tk, sk, "the per-subject backstop is a distinct key ref");
        assert_eq!(sk.class, KeyClass::Subject("u-1".into()));

        let tdek = pin.resolve(&tk, &r()).expect("resolve tenant");
        let sdek = pin.resolve(&sk, &r()).expect("resolve subject");
        let (nonce, ct) = sdek.seal(b"a name in a cached title");
        assert!(
            tdek.open(&nonce, &ct).is_none(),
            "the tenant DEK must not open a subject-backstop ciphertext (GD-4 subject grain)"
        );
    }

    /// **Destroy is callable on the per-tenant key class — tenant-decommission crypto-shred (the
    /// REF-P4 structural GATE).** After `destroy_tenant_dek`, the reserved DEK is unrecoverable — a
    /// resolve fails LOUDLY ([`KmsError`]), NEVER a plaintext fall-through. The lever exists and
    /// fires; the real shred over real index data is REF-P15/REF-P25 (named floor).
    #[test]
    fn destroy_tenant_dek_is_callable_and_renders_the_key_unrecoverable() {
        let pin = RefsDekPin::new(kms());
        let key_ref = pin.reserve(&t(), &r()).expect("reserve");
        assert!(
            pin.resolve(&key_ref, &r()).is_ok(),
            "resolvable before the shred"
        );

        // The structural proof: destroy fires exactly once on a present key class.
        assert!(
            pin.destroy_tenant_dek(&t(), &r()),
            "destroy is callable + a key was present"
        );
        assert!(
            !pin.destroy_tenant_dek(&t(), &r()),
            "a second destroy reports nothing left"
        );

        // Post-shred the key is unrecoverable — LOUD failure, never plaintext-without-key.
        assert!(
            matches!(
                pin.resolve(&key_ref, &r()),
                Err(KmsError::KekUnavailable(_))
            ),
            "a crypto-shredded per-tenant DEK resolves to a LOUD error, never a plaintext"
        );
    }

    /// **Destroying the tenant KEK shreds every Refs subject backstop under it** (tenant offboard =
    /// one operation erases the tenant + all its subjects' cached titles).
    #[test]
    fn tenant_decommission_shreds_every_subject_backstop() {
        let pin = RefsDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant dek");
        let s1 = pin.reserve_subject_backstop(&t(), &r(), "u-1").expect("s1");
        let s2 = pin.reserve_subject_backstop(&t(), &r(), "u-2").expect("s2");

        assert!(pin.destroy_tenant_dek(&t(), &r()), "tenant KEK destroyed");

        for kr in [&tk, &s1, &s2] {
            assert!(
                pin.resolve(kr, &r()).is_err(),
                "every Refs DEK under the destroyed tenant KEK is unrecoverable"
            );
        }
    }

    /// **Per-subject crypto-shred leaves the tenant + other subjects intact (the GD-4 individual
    /// lever; destroy callable on the per-subject backstop).** One person's cached title is erased;
    /// the tenant DEK and every other subject keep resolving.
    #[test]
    fn destroy_subject_backstop_is_individual_grained() {
        let pin = RefsDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant");
        let s1 = pin.reserve_subject_backstop(&t(), &r(), "u-1").expect("s1");
        let s2 = pin.reserve_subject_backstop(&t(), &r(), "u-2").expect("s2");

        assert!(
            pin.destroy_subject_backstop(&t(), "u-1"),
            "subject backstop present to destroy"
        );
        assert!(
            !pin.destroy_subject_backstop(&t(), "u-1"),
            "a second destroy finds nothing"
        );

        assert!(
            pin.resolve(&s1, &r()).is_err(),
            "u-1's cached-title key is shredded"
        );
        assert!(
            pin.resolve(&tk, &r()).is_ok(),
            "the tenant DEK is untouched"
        );
        assert!(
            pin.resolve(&s2, &r()).is_ok(),
            "u-2's backstop is untouched"
        );
    }

    /// **A crypto-shredded Refs tenant is excluded from the KMS backup snapshot** (§7.5 / STOR-D3 by
    /// reference) — the per-tenant DEK stays dead across a restore (the backup-backstop half of the
    /// floor unit). The live tenant's wrapped DEK is backed up; the shredded one is not.
    #[test]
    fn shredded_refs_tenant_is_excluded_from_backup() {
        let kms = kms();
        let pin = RefsDekPin::new(Arc::clone(&kms));
        let live = TenantId("live-co".into());
        let dead = TenantId("offboarded-co".into());
        pin.reserve(&live, &r()).expect("live");
        pin.reserve(&dead, &r()).expect("dead");

        assert!(
            pin.destroy_tenant_dek(&dead, &r()),
            "offboard the dead tenant"
        );

        let snap = kms.backup_snapshot();
        assert!(
            snap.iter().any(|(d, _)| d.tenant == live),
            "live tenant DEK backed up"
        );
        assert!(
            !snap.iter().any(|(d, _)| d.tenant == dead),
            "a crypto-shredded Refs tenant is EXCLUDED from backup (stays dead across restore)"
        );
    }

    /// **The Refs key classes share the cell's ONE engine — no second KMS (11.3 one-hierarchy).** The
    /// pin holds the SAME `Arc<KmsEngine>` it was built over; a DEK reserved through the pin is
    /// visible to the cell engine directly (one cell root governs one hierarchy).
    #[test]
    fn refs_uses_the_one_cell_engine_not_a_second_kms() {
        let kms = kms();
        let pin = RefsDekPin::new(Arc::clone(&kms));
        let key_ref = pin.reserve(&t(), &r()).expect("reserve through the pin");
        // The cell engine (NOT the pin) resolves the DEK the pin reserved — proof it is one engine.
        assert!(
            kms.resolve_dek(&key_ref, &r()).is_ok(),
            "the shared cell engine resolves the DEK the Refs pin reserved (one hierarchy)"
        );
        assert!(
            Arc::ptr_eq(pin.engine(), &kms),
            "the pin holds the very same cell engine"
        );
    }

    /// **The REF-P5 inherited-gate precondition list is complete + names the load-bearing gates (the
    /// GATE/DRILLS requirement in code).** Refs reads these; it never re-runs them. The list must
    /// name the restore-verify / cross-tenant / fail-static / disabled-user / misroute / residency
    /// gates so the REF-P5 agent has the precondition machine-readably.
    #[test]
    fn ref_p5_inherited_gates_name_every_precondition() {
        let gates = ref_p5_inherited_gates();
        let ids: Vec<&str> = gates.iter().map(|g| g.id).collect();
        for required in [
            "STOR-D1", "STOR-D2", "ID-D3", "ID-D2", "ID-D1", "CP-D2", "CP-D3",
        ] {
            assert!(
                ids.contains(&required),
                "the REF-P5 precondition list names {required}"
            );
        }
        // Every gate carries a non-empty guarantee (a named gate without its meaning is useless).
        for g in &gates {
            assert!(
                !g.guarantees.is_empty(),
                "gate {} states what it guarantees",
                g.id
            );
        }
    }
}
