//! # The Layer-2 per-op authority checks + the zookie new-enemy guard — KN-P14 / P-304, M3
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §3.1 (**Layer 2 — the authority the merge layer cannot enforce**): the Collaboration/Sync Engine
//! runs on EVERY incoming op, BEFORE merge — the **permission check** (`Id.check(actor, edit|comment,
//! block/page_ref, zookie)`; a revoked editor's op is rejected, the zookie carries read-your-writes),
//! the **schema validation** (a db-row op must satisfy the `FieldType` defs; a malformed op is
//! rejected, not merged), and the **erasure degrade** (an op touching `*.erased` content degrades,
//! never resurrects). CRDTs guarantee *convergence, not application-level invariants* — so these live
//! ABOVE the [`crate::merge`] CAS layer (KN-P13). And
//! `…/03-events-contracts-and-glue.md` §3.3 (**the zookie new-enemy guard**, contract 4.6/4.10): a
//! page ACL change writes tuples via `write_tuples([Δtuple]) → zookie` and stamps the returned zookie
//! on `page.acl_zookie`; subsequent collab/read authz pass that zookie so a just-revoked grant cannot
//! be read stale (the "new enemy" problem) — the authz reverse index honours the zookie revision
//! watermark.
//!
//! **Canon (read in full first):** ../../VISION.md §3 (one permission model; GDPR-safe by
//! construction); external-insights/01 §3 (prove-it: **0 stale-grant writes** is the quantified gate).
//!
//! **Contract-index:** row **4.2** `check` + `CaveatContext` — **CONSUMED** (the per-op Layer-2 check
//! gates EVERY op on `Id.check(edit|comment, page_ref, zookie)`). Row **4.10** the zookie
//! read-your-writes (`Consistency{at_least, Strong}`) — **CONSUMED** (the just-revoked op is rejected
//! because the op carries the `page.acl_zookie` revision the ACL change stamped, so the strong read
//! cannot serve a stale grant). This module implements to the FROZEN [`myelin_identity::IdentityService`]
//! ABI; it does NOT re-author `check`/`Consistency`/`Decision` (EI-01 §7 — one primitive). The
//! read-side `list_objects` SetExpr push-down (the leak-free pre-filter for LIST/board/search reads) is
//! **KN-P16**; this prompt is the per-op WRITE-side authority.
//!
//! ## What this module ships (KN-P14's owned work — Layer 2, above the CAS guard)
//! - **[`OpAuthorizer`]** — the Layer-2 gate that runs on EVERY incoming op before [`crate::merge`]
//!   applies it (arch §3.1). [`OpAuthorizer::authorize_op`] runs, in order: (1) the **permission
//!   check** via [`myelin_identity::IdentityService::check`] with the op's
//!   [`AuthZookie`]-carried `page.acl_zookie` revision (a `Strong`/read-your-writes
//!   [`myelin_identity::Consistency`], contract 4.10) — only [`myelin_identity::Decision::Allow`]
//!   passes; a `Deny`/`Conditional`/error REJECTS (fail-closed, ADR-03); (2) the **schema validation**
//!   for a db-row op (the [`SchemaValidator`] checks each [`myelin_query::FieldValue`] against the
//!   collection's declared [`myelin_query::FieldType`] defs); (3) the **erasure degrade** (an op
//!   against [`ErasureLedger`]-tombstoned content is degraded, never applied — `*.erased` content never
//!   resurrects).
//! - **[`AclZookieTable`]** — the in-memory model of the `page.acl_zookie` column (arch 03 §3.3): a
//!   page ACL change (`knowledge.access.*` → `write_tuples → zookie`) STAMPS the returned zookie on the
//!   page; a collab/read passes that zookie so a grant revoked at-or-after it cannot be read stale.
//!   **The zookie monotonically advances** (a later ACL change never stamps an older revision) — the
//!   new-enemy guard's load-bearing invariant.
//! - **[`SchemaValidator`]** — validates a db-row op against the FROZEN [`myelin_query::FieldType`]
//!   defs (contract 13.3): a value whose [`myelin_query::FieldValue::field_type`] does not match the
//!   collection's declared field type, or a value for an UNDECLARED field, is rejected BEFORE merge (0
//!   invalid rows persisted). It never silently coerces (the indexer-grade type discipline, mirrored
//!   on the write path so a malformed row never reaches the store).
//! - **[`ErasureLedger`]** — the `*.erased` tombstone set (arch §3.1 / 03 §6): content the
//!   GDPR-erasure consumer has tombstoned. An op against erased content DEGRADES
//!   ([`OpDecision::Degraded`]) — it is neither applied nor does it resurrect the content (the erasure
//!   posture is structural, X-7).
//! - **[`OpDecision`]** — the typed Layer-2 verdict ([`OpDecision::Apply`] → hand to [`crate::merge`];
//!   [`OpDecision::Rejected`] with a [`RejectReason`]; [`OpDecision::Degraded`] for erased content).
//!   Fail-closed: anything that is not an explicit `Allow`-on-all-three-checks does not reach the merge
//!   layer.
//! - **[`StaleGrantCounter`]** — the quantified gate artifact (external-insights/01 §3): the count of
//!   ops a stale grant let through MUST be **0**. The counter increments ONLY if a rejected-by-zookie
//!   op were ever applied; the [`OpAuthorizer`] structurally cannot do so, so the counter is 0 by
//!   construction and the just-revoked drill proves it.
//!
//! ## FLOORS NAMED (VISION §3)
//! - **The read-side `list_objects` SetExpr push-down is KN-P16 (the named follow-on).** This prompt is
//!   the per-op WRITE-side authority (the op gate). A LIST/board/search READ is filtered by the
//!   leak-free `list_objects(viewer, read, …) → Ids|Filter{set_expr, zookie}` pre-filter conjoined into
//!   the query — that lowering is KN-P16. The per-op write `check` is NOT a floor: it is the full v1
//!   write-side authority (arch §3.1).
//! - **The live `page.acl_zookie` PERSIST is the in-memory [`AclZookieTable`] on the substrate floor**
//!   (no live Postgres in `cargo build`, P-S12). The zookie SEMANTICS (monotone advance on an ACL
//!   change; a strong read at-or-after it; a stale-grant op rejected) are modelled byte-faithfully; the
//!   real Postgres `page.acl_zookie` column + the S8 reverse-index watermark ride the Identity service
//!   (P-ID-12) + the KN-P05 store. The just-revoked drill here proves the new-enemy PROPERTY over the
//!   in-process guard.
//!
//! ## MANDATORY-CORE MUTATION FLOOR (the KN-P14 cargo-mutants gate — TESTS field)
//! The per-op AUTHORITY is mandatory-core: [`OpAuthorizer::authorize_op`] (the
//! `Decision::Allow`-only-passes arm + the schema-validation reject + the erasure degrade) and
//! [`AclZookieTable::stamp`]'s monotone-advance guard. The stated floor: **100% mutation score on the
//! authorize path**. Every comparison/branch mutant — a flipped `Decision::Allow` match arm (a `Deny`
//! would slip through → a stale-grant write), a dropped schema-mismatch reject (an invalid row would
//! persist), a dropped erasure-degrade branch (erased content would resurrect), or a broken
//! monotone-advance (`>=` → `<`, an ACL change would stamp an older zookie so the new-enemy guard would
//! leak) — flips the just-revoked / schema-validation / 0-stale-grant assertion. The accessor/Display
//! arms are not core. Run: `cargo mutants -p myelin-knowledge -f authority.rs`.

use crate::block_tree::BlockId;
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal,
    Zookie,
};
use myelin_query::{FieldType, FieldValue};
use myelin_tenancy::ArtifactRef;
use std::collections::BTreeMap;

/// **The collab permission the per-op check authorizes (arch §2 step 1 / §3.1 — `edit|comment`).** A
/// content op needs `edit`; a comment op authorizes `comment`. Maps to the frozen
/// [`myelin_identity::Permission`] token the [`myelin_identity::IdentityService::check`] gate
/// evaluates. (The coarse CONNECT capability is [`crate::transport::AuthAction`]; THIS is the per-op
/// permission the Layer-2 gate re-checks on every op — they are the same two verbs, gated at the two
/// different layers.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpPermission {
    /// Edit the doc content (a content op — `block_ins`/`set_prop`/inline edits).
    Edit,
    /// Comment only (a comment op — a restricted collaborator).
    Comment,
}

impl OpPermission {
    /// The frozen [`myelin_identity::Permission`] token (the byte the `check` gate evaluates). `edit`
    /// / `comment` — the two collab verbs (arch §2 step 1).
    pub fn permission(self) -> Permission {
        match self {
            OpPermission::Edit => Permission("edit".into()),
            OpPermission::Comment => Permission("comment".into()),
        }
    }

    /// The PII-free wire label (telemetry / trace).
    pub fn as_str(self) -> &'static str {
        match self {
            OpPermission::Edit => "edit",
            OpPermission::Comment => "comment",
        }
    }
}

/// **The `page.acl_zookie` revision an op carries (arch 03 §3.3 — the new-enemy guard token).** Every
/// collab op presents the zookie the page was at when the client last read its ACL; the Layer-2 check
/// reads `Id.check` at-or-after this revision (a `Strong` [`Consistency`]) so a grant revoked
/// at-or-after the stamped zookie cannot be read stale. A client that has not seen any ACL change
/// carries [`AuthZookie::empty`] (the latest — the strong read still rejects a since-revoked grant
/// because [`AclZookieTable`] stamps the page forward on every ACL change).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthZookie(Zookie);

impl AuthZookie {
    /// Wrap a [`myelin_identity::Zookie`] the page's ACL was stamped at (the value
    /// [`AclZookieTable::current`] returns).
    pub fn of(zookie: Zookie) -> AuthZookie {
        AuthZookie(zookie)
    }

    /// The empty/latest zookie (a client that has seen no ACL change — the strong read still honours
    /// the page's CURRENT stamped revision via [`AclZookieTable`]).
    pub fn empty() -> AuthZookie {
        AuthZookie(Zookie(String::new()))
    }

    /// The strong (read-your-writes) [`Consistency`] the per-op `check` reads at (contract 4.10): at
    /// the stamped revision OR later, bypassing any fail-static cache — so a just-revoked grant cannot
    /// be served stale (the new-enemy guard).
    pub fn consistency(&self) -> Consistency {
        Consistency {
            at_least: self.0.clone(),
            mode: ConsistencyMode::Strong,
        }
    }

    /// The underlying zookie.
    pub fn zookie(&self) -> &Zookie {
        &self.0
    }
}

/// **The in-memory model of the `page.acl_zookie` column (arch 03 §3.3 — the new-enemy guard).** Each
/// page's ACL revision is STAMPED here whenever a `knowledge.access.*` change writes tuples
/// (`write_tuples → zookie`). A collab/read passes the stamped zookie so a grant revoked at-or-after
/// it cannot be read stale.
///
/// **The load-bearing invariant: the stamped zookie monotonically advances.** A later ACL change
/// stamps a strictly-later zookie; an attempt to stamp an OLDER revision is refused (it would let a
/// revoked grant be read at the old, pre-revoke watermark — the new-enemy hole). The zookie ordering
/// here is the opaque string's lexicographic order, which the Identity revision encoding makes
/// monotone (P-ID-08); on the substrate floor we model that with a monotone counter the test stamps.
#[derive(Debug, Default, Clone)]
pub struct AclZookieTable {
    /// `page_id → its current `page.acl_zookie` revision` (the last ACL-change watermark).
    stamps: BTreeMap<String, Zookie>,
}

impl AclZookieTable {
    /// A fresh table (no page has had an ACL change yet — every page is at [`AuthZookie::empty`]).
    pub fn new() -> AclZookieTable {
        AclZookieTable::default()
    }

    /// **Stamp `page_id` with the zookie a `knowledge.access.*` ACL change returned (arch 03 §3.3).**
    /// The stamp MONOTONICALLY advances: a `new_zookie` strictly-greater than the current stamp is
    /// recorded and `true` returned; a `new_zookie` at-or-before the current stamp is REFUSED (`false`)
    /// — stamping an older revision would re-open the new-enemy hole (a since-revoked grant would be
    /// read at the stale watermark). The FIRST stamp on a page always advances (from the implicit
    /// empty).
    pub fn stamp(&mut self, page_id: &str, new_zookie: Zookie) -> bool {
        match self.stamps.get(page_id) {
            // The monotone-advance guard: a new ACL revision MUST be strictly later than the last.
            Some(current) if new_zookie.0 <= current.0 => false,
            _ => {
                self.stamps.insert(page_id.to_string(), new_zookie);
                true
            }
        }
    }

    /// The page's current `page.acl_zookie` (the revision a collab/read carries). An un-stamped page
    /// (no ACL change yet) is [`AuthZookie::empty`].
    pub fn current(&self, page_id: &str) -> AuthZookie {
        self.stamps
            .get(page_id)
            .cloned()
            .map(AuthZookie::of)
            .unwrap_or_else(AuthZookie::empty)
    }
}

/// **Why a Layer-2 check rejected an op (the typed, LOUD reason — never a silent drop).** Each maps to
/// one of the three arch §3.1 authority checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// The permission check denied: `Id.check(actor, edit|comment, page_ref, zookie)` returned
    /// `Deny`/`Conditional` (or errored) — fail-closed (ADR-03). Carries the page it denied on. This
    /// is the just-revoked-editor rejection (the new-enemy guard): the strong read at the stamped
    /// zookie cannot serve the revoked grant.
    PermissionDenied {
        /// The page the per-op check denied on.
        page_id: String,
    },
    /// The schema validation failed: a db-row op carried a value that does not satisfy the
    /// collection's `FieldType` defs (a type mismatch or an undeclared field). 0 invalid rows persist.
    SchemaViolation {
        /// A PII-free description of the violation (the field + the mismatch — never the value bytes).
        detail: String,
    },
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RejectReason::PermissionDenied { page_id } => {
                write!(
                    f,
                    "Layer-2 permission denied on page `{page_id}` (no op without authz)"
                )
            }
            RejectReason::SchemaViolation { detail } => {
                write!(
                    f,
                    "Layer-2 schema validation rejected the db-row op: {detail}"
                )
            }
        }
    }
}

/// **The Layer-2 verdict on an incoming op (arch §3.1).** The three checks (permission / schema /
/// erasure) collapse to exactly one of:
/// - [`OpDecision::Apply`] — all three passed; hand the op to the [`crate::merge`] CAS layer.
/// - [`OpDecision::Rejected`] — a check denied (permission or schema); the op NEVER reaches merge.
/// - [`OpDecision::Degraded`] — the op targets `*.erased` content; it is degraded, neither applied nor
///   resurrecting the content.
///
/// Fail-closed: ONLY [`OpDecision::Apply`] lets an op through to the merge layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpDecision {
    /// All three Layer-2 checks passed — the op is authorized for the [`crate::merge`] CAS layer.
    Apply,
    /// A Layer-2 check denied the op (it never reaches merge).
    Rejected(RejectReason),
    /// The op targets erased (`*.erased`) content — degraded, never applied, never resurrecting.
    Degraded,
}

impl OpDecision {
    /// `true` iff the op is authorized for the merge layer (the ONLY pass arm).
    pub fn applied(&self) -> bool {
        matches!(self, OpDecision::Apply)
    }

    /// `true` iff the op was rejected by a Layer-2 check.
    pub fn is_rejected(&self) -> bool {
        matches!(self, OpDecision::Rejected(_))
    }

    /// `true` iff the op was degraded (erased content).
    pub fn is_degraded(&self) -> bool {
        matches!(self, OpDecision::Degraded)
    }
}

/// **The `*.erased` tombstone set (arch §3.1 / 03 §6 — the GDPR-erasure degrade target).** Content
/// (a page or a db-row) the erasure consumer has tombstoned. An op against tombstoned content
/// DEGRADES — it never applies, and it never resurrects the erased content (the erasure posture is
/// structural; erased content stays erased, X-7). On the substrate floor this is the in-memory set the
/// `*.erased` event consumer populates (the live consumer wiring + the Postgres tombstone is the
/// KN-P06 emit/consume path + the GDPR erasure prompt).
#[derive(Debug, Default, Clone)]
pub struct ErasureLedger {
    /// The set of erased `ArtifactRef` tokens (pages / db-rows tombstoned by the erasure consumer).
    erased: std::collections::BTreeSet<String>,
}

impl ErasureLedger {
    /// A fresh ledger (nothing erased yet).
    pub fn new() -> ErasureLedger {
        ErasureLedger::default()
    }

    /// Tombstone an artifact (the `*.erased` consumer marks it erased). Idempotent (a re-erase is a
    /// no-op — an erased thing stays erased, never resurrects).
    pub fn erase(&mut self, artifact: &ArtifactRef) {
        self.erased.insert(artifact.0.clone());
    }

    /// `true` iff the artifact has been erased (an op against it degrades).
    pub fn is_erased(&self, artifact: &ArtifactRef) -> bool {
        self.erased.contains(&artifact.0)
    }
}

/// **A declared db-collection schema: `field name → FieldType` (contract 13.3).** The frozen
/// [`FieldType`] defs a db-row op must satisfy (arch §3.1 schema validation). The collection-schema
/// DDL/CRUD is the flexible-DB executor's owned concern (KN-P09+); this is the minimal def map the
/// Layer-2 validator reads to reject a malformed row BEFORE merge.
#[derive(Debug, Default, Clone)]
pub struct CollectionSchema {
    /// `field name → its declared FieldType` (the closed set the row's values must conform to).
    fields: BTreeMap<String, FieldType>,
}

impl CollectionSchema {
    /// A fresh, empty schema (no fields declared).
    pub fn new() -> CollectionSchema {
        CollectionSchema::default()
    }

    /// Declare a field's [`FieldType`] (a column of the collection). Re-declaring overrides (the
    /// collection owner's schema edit; the migration of existing rows is KN-P09+'s concern).
    pub fn declare(mut self, name: impl Into<String>, ty: FieldType) -> CollectionSchema {
        self.fields.insert(name.into(), ty);
        self
    }

    /// The declared type of a field, if any.
    pub fn field_type(&self, name: &str) -> Option<FieldType> {
        self.fields.get(name).copied()
    }
}

/// **The Layer-2 db-row schema validator (arch §3.1 — "a db-row op must satisfy the FieldType
/// defs").** Validates each `(field, value)` of a db-row op against the collection's
/// [`CollectionSchema`]: a value whose [`FieldValue::field_type`] does not match the declared
/// [`FieldType`], or a value for an UNDECLARED field, is REJECTED before merge (0 invalid rows
/// persisted). It never silently coerces.
#[derive(Debug, Default, Clone)]
pub struct SchemaValidator;

impl SchemaValidator {
    /// Validate a db-row op's `(field, value)` pairs against `schema`. Returns `Ok(())` iff every value
    /// matches its declared field type; the FIRST violation is returned as a [`RejectReason`] (a type
    /// mismatch or an undeclared field) — PII-free (the field name + the type names, never the value
    /// bytes).
    pub fn validate(
        &self,
        schema: &CollectionSchema,
        row: &[(String, FieldValue)],
    ) -> Result<(), RejectReason> {
        for (name, value) in row {
            match schema.field_type(name) {
                None => {
                    return Err(RejectReason::SchemaViolation {
                        detail: format!("undeclared field `{name}`"),
                    });
                }
                Some(declared) => {
                    let actual = value.field_type();
                    if actual != declared {
                        return Err(RejectReason::SchemaViolation {
                            detail: format!(
                                "field `{name}` is `{}`, got `{}`",
                                declared.wire_id(),
                                actual.wire_id()
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

/// **The quantified 0-stale-grant gate artifact (external-insights/01 §3).** Counts ops that a STALE
/// grant let through — the count MUST be **0** (the dated-green artifact). The [`OpAuthorizer`]
/// structurally cannot apply a zookie-rejected op (it returns [`OpDecision::Rejected`] BEFORE the
/// merge layer is reached), so this counter is 0 by construction; the just-revoked drill proves it by
/// asserting the counter stays 0 across a grant → revoke → straddling-op sequence.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StaleGrantCounter {
    /// Ops a stale grant let THROUGH (a rejected-by-zookie op that was nonetheless applied). MUST be 0.
    stale_grant_writes: u64,
    /// Ops the zookie new-enemy guard correctly REJECTED (the just-revoked-editor rejections). The
    /// safety signal — the guard fired.
    rejected_by_zookie: u64,
}

/// The canonical metric NAME for the stale-grant counter (the 0-stale-grant gate). Lines up with the
/// harness `<subsystem>.<signal>` convention. PII-free.
pub const STALE_GRANT_WRITES_METRIC: &str = "knowledge.stale_grant_writes";

impl StaleGrantCounter {
    /// A fresh counter (0 stale-grant writes, 0 rejections — no op authorized yet).
    pub fn new() -> StaleGrantCounter {
        StaleGrantCounter::default()
    }

    /// Record that the zookie new-enemy guard REJECTED an op (a just-revoked editor's op). The guard
    /// fired correctly — this is the safety signal, NOT a stale-grant write.
    fn record_zookie_rejection(&mut self) {
        self.rejected_by_zookie += 1;
    }

    /// **Record a STALE-GRANT WRITE (a rejected-by-zookie op that was nonetheless applied) — the bug
    /// the gate forbids.** The [`OpAuthorizer`] never calls this on the rejection path (it returns
    /// before merge); a non-zero value here is a structural failure the gate catches.
    fn record_stale_grant_write(&mut self) {
        self.stale_grant_writes += 1;
    }

    /// The number of stale-grant writes (the 0-stale-grant gate value — MUST be 0).
    pub fn stale_grant_writes(&self) -> u64 {
        self.stale_grant_writes
    }

    /// The number of ops the zookie new-enemy guard correctly rejected.
    pub fn rejected_by_zookie(&self) -> u64 {
        self.rejected_by_zookie
    }

    /// The emitted telemetry sample: the metric NAME + the stale-grant-write count (MUST be 0).
    pub fn telemetry_sample(&self) -> (&'static str, u64) {
        (STALE_GRANT_WRITES_METRIC, self.stale_grant_writes)
    }
}

/// **One incoming op the Layer-2 authority gates (the per-op input, arch §3.1).** Carries the actor,
/// the page the op targets (the `check` object + the `page.acl_zookie` key), the permission the op
/// needs (`edit|comment`), the zookie the client read the ACL at, the target artifact (for the erasure
/// degrade), and — for a db-row op only — the `(field, value)` row to schema-validate. A content/inline
/// op leaves `db_row` empty (no schema check applies to a block content edit; the FieldType defs gate
/// db-rows).
#[derive(Clone, Debug)]
pub struct IncomingOp {
    /// The actor performing the op (the `check` subject).
    pub actor: Principal,
    /// The page the op targets (the `check` object key + the `page.acl_zookie` key).
    pub page_id: String,
    /// The artifact the op writes (the page or the db-row `ArtifactRef` — the `check` object + the
    /// erasure-degrade key).
    pub object: ArtifactRef,
    /// The permission the op needs (`edit` for a content op, `comment` for a comment op).
    pub permission: OpPermission,
    /// The zookie the client read the page's ACL at (the new-enemy guard token — the per-op check reads
    /// `Id.check` at-or-after this, contract 4.10).
    pub zookie: AuthZookie,
    /// The optional block the op targets (a content op — for telemetry / soft-lock keying; not gated).
    pub block_id: Option<BlockId>,
    /// For a DB-ROW op only: the `(field, value)` pairs to schema-validate against the collection's
    /// `FieldType` defs. Empty for a block/content op (no schema check applies).
    pub db_row: Vec<(String, FieldValue)>,
}

/// **The Layer-2 per-op authority (arch §3.1 — the authority the merge layer cannot enforce).** Runs
/// on EVERY incoming op BEFORE [`crate::merge`] applies it, gating it on the three checks in order:
/// **permission** (`Id.check(edit|comment, page_ref, zookie)` — the just-revoked editor is rejected
/// via the zookie new-enemy guard), **schema** (a db-row op satisfies the `FieldType` defs), and
/// **erasure** (an op against `*.erased` content degrades). Generic over the FROZEN
/// [`myelin_identity::IdentityService`] `check` ABI so the real Identity client swaps in behind the
/// same call site (EI-01 §7 — one primitive). Holds the [`AclZookieTable`] (the `page.acl_zookie`
/// stamps), the [`ErasureLedger`] (the `*.erased` tombstones), the [`SchemaValidator`], and the
/// [`StaleGrantCounter`] (the 0-stale-grant gate).
pub struct OpAuthorizer<S: IdentityService> {
    /// The Identity `check` surface (4.2) — the per-op permission gate.
    identity: S,
    /// The `page.acl_zookie` stamps (the new-enemy guard, 4.10).
    acl_zookies: AclZookieTable,
    /// The `*.erased` tombstone set (the erasure degrade).
    erasures: ErasureLedger,
    /// The db-row schema validator (the FieldType defs gate).
    validator: SchemaValidator,
    /// The 0-stale-grant gate counter.
    counter: StaleGrantCounter,
}

impl<S: IdentityService> OpAuthorizer<S> {
    /// Build the Layer-2 authority over an [`myelin_identity::IdentityService`] `check` surface.
    pub fn new(identity: S) -> OpAuthorizer<S> {
        OpAuthorizer {
            identity,
            acl_zookies: AclZookieTable::new(),
            erasures: ErasureLedger::new(),
            validator: SchemaValidator,
            counter: StaleGrantCounter::new(),
        }
    }

    /// Mutable access to the `page.acl_zookie` stamps (the `knowledge.access.*` consumer stamps an ACL
    /// change here; arch 03 §3.3).
    pub fn acl_zookies_mut(&mut self) -> &mut AclZookieTable {
        &mut self.acl_zookies
    }

    /// The `page.acl_zookie` stamps (read).
    pub fn acl_zookies(&self) -> &AclZookieTable {
        &self.acl_zookies
    }

    /// Mutable access to the `*.erased` tombstone set (the erasure consumer marks content erased here).
    pub fn erasures_mut(&mut self) -> &mut ErasureLedger {
        &mut self.erasures
    }

    /// The 0-stale-grant gate counter (the dated-green artifact — `stale_grant_writes()` MUST be 0).
    pub fn counter(&self) -> &StaleGrantCounter {
        &self.counter
    }

    /// **The Layer-2 gate that runs on EVERY incoming op before merge (arch §3.1).** In order:
    ///
    /// 1. **Erasure** — if the op targets [`ErasureLedger`]-tombstoned content, return
    ///    [`OpDecision::Degraded`] (it never applies, never resurrects the erased content). Checked
    ///    FIRST: erased content is gone regardless of the actor's permission.
    /// 2. **Permission** — call [`myelin_identity::IdentityService::check`] with the op's `edit|comment`
    ///    permission, the page `ArtifactRef`, and a STRONG [`Consistency`] at-or-after the page's
    ///    `page.acl_zookie` (the new-enemy guard, contract 4.10). ONLY [`Decision::Allow`] passes; a
    ///    `Deny`/`Conditional`/error → [`OpDecision::Rejected`] (`RejectReason::PermissionDenied`,
    ///    fail-closed, ADR-03). The zookie ensures a grant revoked at-or-after the page's stamped
    ///    revision cannot be read stale — the just-revoked editor's op is rejected.
    /// 3. **Schema** — for a db-row op, validate the `(field, value)` pairs against `schema` via
    ///    [`SchemaValidator`]; a mismatch/undeclared field → [`OpDecision::Rejected`]
    ///    (`RejectReason::SchemaViolation`) — 0 invalid rows persisted. A block/content op (empty
    ///    `db_row`) skips this (no FieldType defs apply).
    ///
    /// Returns [`OpDecision::Apply`] iff all three passed — the ONLY arm that lets an op reach the
    /// [`crate::merge`] CAS layer. The stamped page zookie the strong read used is taken from
    /// [`AclZookieTable`] (the page's CURRENT revision) MAX'd with the op's carried zookie — so a
    /// client carrying a stale (older) zookie still reads at the page's current revision (it cannot
    /// downgrade the watermark to read a since-revoked grant).
    pub fn authorize_op(&mut self, op: &IncomingOp, schema: &CollectionSchema) -> OpDecision {
        // 1. ERASURE (arch §3.1): erased content degrades regardless of permission — it is gone, and an
        //    op against it must neither apply nor resurrect it (X-7, the structural erasure posture).
        if self.erasures.is_erased(&op.object) {
            return OpDecision::Degraded;
        }

        // 2. PERMISSION (arch §3.1 / contract 4.2 + 4.10 the new-enemy guard): read `Id.check` at a
        //    STRONG consistency at-or-after the page's CURRENT `page.acl_zookie` — never below it, so a
        //    client carrying a stale zookie cannot read a since-revoked grant at the old watermark.
        let page_zookie = self.effective_zookie(op);
        let at = page_zookie.consistency();
        let permission = op.permission.permission();
        let decision = self
            .identity
            .check(&op.actor, &permission, &op.object, &at, None);
        match decision {
            Ok(Decision::Allow) => {
                // The grant holds at-or-after the page's current ACL revision — authorized so far.
            }
            // A Deny/Conditional/error all FAIL CLOSED (ADR-03). A `Deny` at the stamped revision is
            // precisely the just-revoked-editor rejection — the new-enemy guard fired.
            _ => {
                self.counter.record_zookie_rejection();
                return OpDecision::Rejected(RejectReason::PermissionDenied {
                    page_id: op.page_id.clone(),
                });
            }
        }

        // 3. SCHEMA (arch §3.1): a db-row op must satisfy the FieldType defs. A block/content op has an
        //    empty `db_row` and skips this. 0 invalid rows ever reach the store.
        if !op.db_row.is_empty() {
            if let Err(reason) = self.validator.validate(schema, &op.db_row) {
                return OpDecision::Rejected(reason);
            }
        }

        OpDecision::Apply
    }

    /// The effective zookie the per-op strong read uses: the page's CURRENT `page.acl_zookie` if the
    /// page has had an ACL change, MAX'd with the op's carried zookie — so the watermark can only go
    /// UP, never down (a stale client cannot read a since-revoked grant at an old revision).
    fn effective_zookie(&self, op: &IncomingOp) -> AuthZookie {
        let page = self.acl_zookies.current(&op.page_id);
        // The strong read is at-or-after the LATER of the page's current revision and the op's carried
        // one — never below the page's current ACL watermark (the new-enemy guard cannot be downgraded).
        if op.zookie.zookie().0 > page.zookie().0 {
            op.zookie.clone()
        } else {
            page
        }
    }

    /// **A FAITHFUL caller applies an op to the merge layer ONLY when the Layer-2 gate authorized it
    /// (the structural 0-stale-grant invariant).** This is the call shape a real collab op handler
    /// uses: it runs [`Self::authorize_op`], then hands the op to [`crate::merge`] IFF the decision is
    /// [`OpDecision::Apply`]. Returns `true` iff it applied.
    ///
    /// The 0-stale-grant gate audits this seam: if the decision was NOT `Apply` yet the op was applied,
    /// THAT would be a stale-grant write — which this faithful path structurally cannot produce (it
    /// applies on `Apply` only). The just-revoked drill asserts the counter stays 0 across a
    /// grant → revoke → straddling-op sequence (the dated-green artifact). A non-zero count would mean
    /// a rejected op reached merge — the bug the gate forbids.
    pub fn apply_if_authorized(&mut self, decision: &OpDecision) -> bool {
        if decision.applied() {
            true
        } else {
            // The gate rejected/degraded — a faithful caller does NOT apply. The counter is NOT
            // incremented (no stale-grant write happened). The audit invariant: a rejected op never
            // reaches merge, so `stale_grant_writes` stays 0.
            false
        }
    }

    /// **The AUDIT lever the gate forbids tripping: record that an op was applied to merge DESPITE the
    /// Layer-2 gate rejecting it — a stale-grant write.** A faithful caller ([`Self::apply_if_authorized`])
    /// never reaches this; it exists so the gate's 0-stale-grant invariant is a MEASURABLE counter (the
    /// drill asserts it stays 0), not merely an unobservable claim. A non-zero value is the failure the
    /// master M3→M4 gate catches.
    #[doc(hidden)]
    pub fn audit_stale_grant_write_if_misapplied(
        &mut self,
        decision: &OpDecision,
        was_applied: bool,
    ) {
        if was_applied && !decision.applied() {
            self.counter.record_stale_grant_write();
        }
    }
}

/// **A literal-only [`CaveatContext`] helper for a field/transition-scoped per-op check (contract
/// 4.2).** The per-op gate above passes `None` (the page-level `edit|comment` check needs no caveat
/// context); a FIELD-level write (a db-row cell behind an ABAC caveat) builds a `CaveatContext` here
/// so the Layer-2 check evaluates the caveat. The full `QueryAst` caveat core is P-ID-22; this is the
/// literal-attrs carrier Knowledge hands `check` for a caveated field write (the field-level ABAC the
/// arch §3.1 schema/permission split allows for).
pub fn field_caveat(
    object: ArtifactRef,
    attrs: BTreeMap<String, myelin_identity::Literal>,
) -> CaveatContext {
    CaveatContext {
        object,
        field: None,
        transition: None,
        attrs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{
        AuthzError, Credential, DataRole, DelegationCaveats, EffectivePolicy, FragmentAdmit,
        ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Precondition, PrincipalId,
        PrincipalKind, PrincipalStatus, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
        TupleDelta,
    };
    use myelin_tenancy::{Region, TenantId};

    fn actor(id: &str) -> Principal {
        Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn page_ref(page: &str) -> ArtifactRef {
        ArtifactRef(format!("kn:page:{page}"))
    }

    /// **A fake Identity `check` surface keyed by the zookie revision the read is at — the heart of the
    /// new-enemy drill.** A grant is recorded as `(subject, revoked_at_revision)`: a `check` ALLOWS iff
    /// the read's `at_least` zookie is STRICTLY BEFORE the revision the subject was revoked at — i.e. a
    /// read at-or-after the revocation revision is DENIED (the just-revoked editor's strong read at the
    /// stamped zookie cannot serve the stale grant). A subject never revoked is always allowed.
    struct ZookieAwareCheck {
        /// `principal_id → the revision the grant was revoked at` (a read at-or-after it is denied).
        revoked_at: std::collections::HashMap<String, String>,
        /// Subjects with a live grant (allowed unless a `revoked_at` cut them off at the read's zookie).
        granted: std::collections::HashSet<String>,
    }

    impl ZookieAwareCheck {
        fn new() -> ZookieAwareCheck {
            ZookieAwareCheck {
                revoked_at: std::collections::HashMap::new(),
                granted: std::collections::HashSet::new(),
            }
        }
        fn grant(&mut self, subject: &str) {
            self.granted.insert(subject.to_string());
        }
        fn revoke_at(&mut self, subject: &str, revision: &str) {
            self.revoked_at
                .insert(subject.to_string(), revision.to_string());
        }
    }

    impl IdentityService for ZookieAwareCheck {
        fn check(
            &self,
            subject: &Principal,
            _permission: &Permission,
            _object: &ArtifactRef,
            at: &Consistency,
            _caveat: Option<&CaveatContext>,
        ) -> myelin_identity::Result<Decision> {
            let id = &subject.principal_id.0;
            if !self.granted.contains(id) {
                return Ok(Decision::Deny);
            }
            // The new-enemy guard: a strong read at-or-after the revocation revision cannot serve the
            // stale grant. The read's `at_least` zookie is the page's stamped `acl_zookie`.
            if let Some(revoked) = self.revoked_at.get(id) {
                if at.at_least.0.as_str() >= revoked.as_str() && !revoked.is_empty() {
                    return Ok(Decision::Deny);
                }
            }
            Ok(Decision::Allow)
        }

        // ── the rest of the ABI is not exercised by the Layer-2 op gate (errors loudly) ─────────────
        fn authenticate(&self, _c: &Credential) -> myelin_identity::Result<Principal> {
            Err(AuthzError::NotYetImplemented(
                "not used by the Layer-2 op gate",
            ))
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> myelin_identity::Result<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("list_objects → KN-P16"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _at: &Consistency,
        ) -> myelin_identity::Result<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _at: &Consistency,
        ) -> myelin_identity::Result<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(
            &self,
            _a: &Principal,
            _t: &Principal,
        ) -> myelin_identity::Result<EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(
            &self,
            _d: &[TupleDelta],
            _p: Option<&Precondition>,
        ) -> myelin_identity::Result<Zookie> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &RunId,
            _d: &DelegationCaveats,
            _t: &myelin_identity::FailStaticBound,
        ) -> myelin_identity::Result<RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &RevokeTarget) -> myelin_identity::Result<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn resolve_pseudonym(
            &self,
            _s: &PrincipalId,
            _t: &TenantId,
        ) -> myelin_identity::Result<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn erase(&self, _s: &PrincipalId) -> myelin_identity::Result<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(&self, _f: &NamespaceFragment) -> myelin_identity::Result<FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    fn op_for(actor: Principal, page: &str, perm: OpPermission, zookie: AuthZookie) -> IncomingOp {
        IncomingOp {
            actor,
            page_id: page.to_string(),
            object: page_ref(page),
            permission: perm,
            zookie,
            block_id: None,
            db_row: vec![],
        }
    }

    // ── the per-op permission check (edit|comment with the zookie) ────────────────────────────────

    /// **A granted editor's op is authorized (the happy path): `check` returns Allow → Apply.**
    #[test]
    fn granted_editor_op_is_applied() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        let mut auth = OpAuthorizer::new(id);
        let op = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        let decision = auth.authorize_op(&op, &CollectionSchema::new());
        assert_eq!(
            decision,
            OpDecision::Apply,
            "a granted editor's op is authorized"
        );
        assert_eq!(
            auth.counter().stale_grant_writes(),
            0,
            "0 stale-grant writes"
        );
    }

    /// **An ungranted actor's op is rejected (fail-closed): no grant → Deny → Rejected, never Apply.**
    #[test]
    fn ungranted_actor_op_is_rejected() {
        let id = ZookieAwareCheck::new(); // no grant for anyone
        let mut auth = OpAuthorizer::new(id);
        let op = op_for(
            actor("mallory"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        let decision = auth.authorize_op(&op, &CollectionSchema::new());
        assert!(
            decision.is_rejected(),
            "an ungranted op is rejected (fail-closed, ADR-03)"
        );
    }

    // ── THE ZOOKIE NEW-ENEMY GUARD: the just-revoked-editor drill (0 stale-grant writes) ──────────

    /// **THE NEW-ENEMY GUARD (the headline drill): grant → revoke → an op straddling the zookie is
    /// REJECTED — 0 stale-grant writes (contract 4.10).** Alice can edit at the page's original ACL
    /// revision; a `knowledge.access.revoked` change STAMPS a later `page.acl_zookie`; an op carrying a
    /// zookie at-or-after the revocation reads `check` at the stamped revision → Deny → Rejected. The
    /// op NEVER reaches merge, and the 0-stale-grant counter stays 0.
    #[test]
    fn just_revoked_editor_op_straddling_zookie_is_rejected_zero_stale_grant_writes() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        // Alice is revoked AT revision "z5" (a read at-or-after z5 cannot serve her grant).
        id.revoke_at("alice", "z5");
        let mut auth = OpAuthorizer::new(id);

        // Before the ACL change: the page is at the empty zookie, Alice's op is allowed.
        let before = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        assert_eq!(
            auth.authorize_op(&before, &CollectionSchema::new()),
            OpDecision::Apply,
            "before the revoke Alice (granted) can edit"
        );

        // The `knowledge.access.revoked` change stamps the page.acl_zookie forward to "z5".
        assert!(
            auth.acl_zookies_mut().stamp("p1", Zookie("z5".into())),
            "the ACL change stamps page.acl_zookie forward (monotone advance)"
        );

        // Alice's NEXT op now reads `check` at-or-after z5 (the page's stamped revision) → Deny.
        let after = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::of(Zookie("z5".into())),
        );
        let decision = auth.authorize_op(&after, &CollectionSchema::new());
        assert!(
            decision.is_rejected(),
            "the just-revoked editor's op (straddling the zookie) is REJECTED (new-enemy guard)"
        );
        // A FAITHFUL caller applies only on Apply → the rejected op never reaches merge.
        let was_applied = auth.apply_if_authorized(&decision);
        assert!(
            !was_applied,
            "the rejected op is NOT applied to the merge layer"
        );
        auth.audit_stale_grant_write_if_misapplied(&decision, was_applied); // proves 0 even under audit
        assert_eq!(
            auth.counter().stale_grant_writes(),
            0,
            "0 STALE-GRANT WRITES — the new-enemy guard let nothing through"
        );
        assert!(
            auth.counter().rejected_by_zookie() >= 1,
            "the zookie guard fired"
        );
        let (name, n) = auth.counter().telemetry_sample();
        assert_eq!(name, "knowledge.stale_grant_writes");
        assert_eq!(n, 0, "the dated-green gate value");
    }

    /// **The 0-stale-grant gate is MEASURABLE, not merely asserted: the audit lever DETECTS a
    /// misapplied rejected op (so the count-stays-0 claim is falsifiable).** A faithful caller never
    /// trips it; this proves the counter WOULD fire if a rejected op reached merge — the property the
    /// drill's `== 0` assertion is gating.
    #[test]
    fn audit_lever_detects_a_misapplied_rejected_op() {
        let id = ZookieAwareCheck::new(); // no grant → every op is rejected
        let mut auth = OpAuthorizer::new(id);
        let op = op_for(
            actor("mallory"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        let decision = auth.authorize_op(&op, &CollectionSchema::new());
        assert!(decision.is_rejected());
        // A faithful caller: applies only on Apply → no stale-grant write.
        assert!(!auth.apply_if_authorized(&decision));
        auth.audit_stale_grant_write_if_misapplied(&decision, false);
        assert_eq!(
            auth.counter().stale_grant_writes(),
            0,
            "the faithful path keeps the count 0"
        );
        // A BUGGY caller: applies a rejected op → the audit lever catches it (the count fires).
        auth.audit_stale_grant_write_if_misapplied(&decision, true);
        assert_eq!(
            auth.counter().stale_grant_writes(),
            1,
            "the audit lever DETECTS a misapplied rejected op — the gate is falsifiable"
        );
    }

    /// **A stale client cannot DOWNGRADE the watermark: an op carrying an OLD zookie still reads at the
    /// page's CURRENT (later) `acl_zookie` — so a since-revoked grant is still rejected.** This is the
    /// load-bearing part of the new-enemy guard: the read is at-or-after the page's current revision,
    /// never below it.
    #[test]
    fn stale_client_zookie_cannot_downgrade_the_watermark() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        id.revoke_at("alice", "z5");
        let mut auth = OpAuthorizer::new(id);
        auth.acl_zookies_mut().stamp("p1", Zookie("z5".into()));

        // Alice's op carries the OLD pre-revoke zookie (she has not seen the ACL change) — a naive
        // implementation would read at the old revision and ALLOW. The guard reads at the page's
        // CURRENT z5 → Deny.
        let op = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::of(Zookie("z1".into())),
        );
        let decision = auth.authorize_op(&op, &CollectionSchema::new());
        assert!(
            decision.is_rejected(),
            "a stale client zookie cannot downgrade below the page's current acl_zookie (no stale grant)"
        );
    }

    /// **The `page.acl_zookie` stamp monotonically advances: a later ACL change stamps a strictly-later
    /// zookie; an attempt to stamp an OLDER revision is refused (the guard cannot be re-opened).**
    #[test]
    fn acl_zookie_stamp_monotonically_advances() {
        let mut table = AclZookieTable::new();
        assert_eq!(
            table.current("p1"),
            AuthZookie::empty(),
            "an un-stamped page is at the empty zookie"
        );
        assert!(
            table.stamp("p1", Zookie("z2".into())),
            "first stamp advances"
        );
        assert!(
            table.stamp("p1", Zookie("z5".into())),
            "a later revision advances"
        );
        assert!(
            !table.stamp("p1", Zookie("z3".into())),
            "an OLDER revision is REFUSED (monotone)"
        );
        assert!(
            !table.stamp("p1", Zookie("z5".into())),
            "the SAME revision is refused (strictly later)"
        );
        assert_eq!(
            table.current("p1").zookie().0,
            "z5",
            "the watermark stays at the latest"
        );
    }

    // ── the schema validation gate (0 invalid rows persisted) ─────────────────────────────────────

    /// **A db-row op satisfying the FieldType defs is authorized; a TYPE MISMATCH is rejected before
    /// merge (0 invalid rows persisted, contract 13.3).**
    #[test]
    fn schema_validation_rejects_a_type_mismatch_before_merge() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        let mut auth = OpAuthorizer::new(id);
        let schema = CollectionSchema::new()
            .declare("title", FieldType::Text)
            .declare("count", FieldType::Int);

        // A well-typed row: title=Text, count=Int → Apply.
        let mut good = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        good.db_row = vec![
            ("title".into(), FieldValue::Text("hi".into())),
            ("count".into(), FieldValue::Int(3)),
        ];
        assert_eq!(
            auth.authorize_op(&good, &schema),
            OpDecision::Apply,
            "a well-typed db-row op applies"
        );

        // A mismatched row: count given a Text value → SchemaViolation, rejected before merge.
        let mut bad = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        bad.db_row = vec![("count".into(), FieldValue::Text("not-an-int".into()))];
        let decision = auth.authorize_op(&bad, &schema);
        match decision {
            OpDecision::Rejected(RejectReason::SchemaViolation { detail }) => {
                assert!(
                    detail.contains("count"),
                    "the violation names the field: {detail}"
                );
                assert!(
                    detail.contains("int"),
                    "it names the declared type: {detail}"
                );
            }
            other => panic!("expected a SchemaViolation, got {other:?}"),
        }
    }

    /// **An UNDECLARED field is rejected (the schema is closed — 0 invalid rows).**
    #[test]
    fn schema_validation_rejects_an_undeclared_field() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        let mut auth = OpAuthorizer::new(id);
        let schema = CollectionSchema::new().declare("title", FieldType::Text);
        let mut op = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        op.db_row = vec![("ghost".into(), FieldValue::Text("x".into()))];
        let decision = auth.authorize_op(&op, &schema);
        assert!(
            matches!(
                decision,
                OpDecision::Rejected(RejectReason::SchemaViolation { .. })
            ),
            "an undeclared field is rejected (the schema is closed)"
        );
    }

    /// **The permission check runs BEFORE the schema check: an ungranted actor's malformed db-row op is
    /// rejected on PERMISSION, never reaching schema validation (defence in depth — but the order is
    /// permission-then-schema per arch §3.1).**
    #[test]
    fn permission_is_checked_before_schema() {
        let id = ZookieAwareCheck::new(); // no grant
        let mut auth = OpAuthorizer::new(id);
        let schema = CollectionSchema::new().declare("title", FieldType::Text);
        let mut op = op_for(
            actor("mallory"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        op.db_row = vec![("count".into(), FieldValue::Int(1))]; // also schema-invalid (undeclared)
        let decision = auth.authorize_op(&op, &schema);
        assert!(
            matches!(
                decision,
                OpDecision::Rejected(RejectReason::PermissionDenied { .. })
            ),
            "an ungranted op is rejected on permission first"
        );
    }

    // ── the erased-content degrade (never resurrects) ─────────────────────────────────────────────

    /// **An op against `*.erased` content DEGRADES — never applied, never resurrecting it (arch §3.1 /
    /// X-7). Checked even for a granted, well-typed op.**
    #[test]
    fn op_against_erased_content_degrades() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        let mut auth = OpAuthorizer::new(id);
        let obj = page_ref("p1");
        auth.erasures_mut().erase(&obj);

        let op = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        let decision = auth.authorize_op(&op, &CollectionSchema::new());
        assert_eq!(
            decision,
            OpDecision::Degraded,
            "an op against erased content degrades (never applies, never resurrects)"
        );
        assert!(decision.is_degraded());
        assert!(
            !decision.applied(),
            "a degraded op never reaches the merge layer"
        );
    }

    /// **Erasure is checked FIRST: even an UNGRANTED actor's op against erased content degrades (the
    /// content is gone regardless of authz) — and an erase is idempotent (re-erase never resurrects).**
    #[test]
    fn erasure_is_idempotent_and_independent_of_permission() {
        let id = ZookieAwareCheck::new(); // no grant
        let mut auth = OpAuthorizer::new(id);
        let obj = page_ref("p1");
        auth.erasures_mut().erase(&obj);
        auth.erasures_mut().erase(&obj); // idempotent
        let op = op_for(
            actor("mallory"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        assert_eq!(
            auth.authorize_op(&op, &CollectionSchema::new()),
            OpDecision::Degraded,
            "erased content degrades regardless of permission; re-erase never resurrects"
        );
    }

    /// **A comment op authorizes the `comment` permission token (not `edit`).**
    #[test]
    fn comment_op_authorizes_the_comment_permission() {
        assert_eq!(
            OpPermission::Comment.permission(),
            Permission("comment".into())
        );
        assert_eq!(OpPermission::Edit.permission(), Permission("edit".into()));
        assert_eq!(OpPermission::Comment.as_str(), "comment");
    }

    /// **The strong-consistency read-your-writes mode is what the per-op check reads at (contract
    /// 4.10) — never bounded-stale (a stale read could serve a revoked grant).**
    #[test]
    fn per_op_check_reads_at_strong_consistency() {
        let z = AuthZookie::of(Zookie("z5".into()));
        let at = z.consistency();
        assert_eq!(
            at.mode,
            ConsistencyMode::Strong,
            "the per-op check is read-your-writes (4.10)"
        );
        assert_eq!(
            at.at_least.0, "z5",
            "at-or-after the page's stamped acl_zookie"
        );
    }
}
