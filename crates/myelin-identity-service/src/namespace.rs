//! # `namespace` — the ReBAC namespace engine + the fragment-admit contract + the core
//! org/team/project hierarchy (P-ID-10 → P-068)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §5 (the **Zanzibar namespace-configuration model**: each object type declares **relations**
//! (direct edges) and **permissions** (computed usersets — **union / intersection / exclusion +
//! tuple-to-userset rewrites** for inheritance); the **core org→team→project hierarchy** with
//! `parent_team->view` tuple-to-userset inheritance; the **four-operator design rule**: every
//! cross-subsystem visibility need reduces to those four operators, **no bespoke check path**; the
//! **fragment-admit contract**: subsystems declare fragments at build time, **Id owns the engine
//! and never invents object ids**), §6 (the raw tuples the rewrite reads).
//!
//! **Contract-index:** row **4.9** (per-subsystem ReBAC namespace fragment — the **engine +
//! admit-contract + core hierarchy** half) — **OWNED** here. The five per-subsystem fragments
//! (Git / CI / Issues / Knowledge / Chat) are the **M3/M4 follow-on** (P-ID-24..P-ID-30); this
//! prompt ships the engine + the org/team/project core only (the named floor below).
//!
//! ## What this module ships (P-ID-10)
//! 1. **The fragment declaration model** ([`FragmentDef`], [`PermissionRule`]) — the rich,
//!    compile-time shape a subsystem declares: an object type, its **relations** (direct edges),
//!    and its **permissions**, each a [`PermissionRule`] expression over the **four Zanzibar
//!    userset operators** ([`Userset`]: `Relation` direct, `Union`, `Intersect`, `Exclusion`,
//!    `TupleToUserset` inheritance). The frozen ABI carrier [`myelin_identity::NamespaceFragment`]
//!    (names only) **projects from** a `FragmentDef` ([`FragmentDef::to_abi`]) — this is NOT a
//!    re-definition of the frozen type (EI-01 §7); the engine carries the rewrite structure the
//!    names-only ABI cannot.
//! 2. **The admit contract** ([`NamespaceEngine::admit`]) — validate a fragment at build/admit
//!    time and either `Admitted{fragment_id}` or `Rejected{reason}` (the frozen
//!    [`myelin_identity::FragmentAdmit`]). A **well-formed** fragment admits; a **malformed** one
//!    (a permission referencing an undeclared relation, a duplicate object type, an empty object
//!    type, a self-referential permission cycle, a tuple-to-userset naming an undeclared parent
//!    relation, **a fragment attempting to mint an object id**) is **rejected** — loudly, never
//!    silently admitted. **Id never invents object ids:** a fragment declares *types + relations +
//!    permissions*, never concrete object ids; an attempt to carry one is rejected.
//! 3. **The compiled cell schema** ([`NamespaceEngine`]) — admitted fragments compile into ONE
//!    cell schema. [`NamespaceEngine::resolve_permission`] lowers a **permission** name on an
//!    object type to its [`Userset`] rewrite tree, so [`crate::check_engine::CheckEngine`]
//!    evaluates a *permission* by walking the same four operators it already evaluates a relation
//!    through (one primitive, EI-01 §7).
//! 4. **The core org→team→project hierarchy** ([`core_hierarchy`]) — the three namespaces shipped
//!    in the engine: `org` (relations: `member`, `admin`; permission `view = member ∪ admin`),
//!    `team` (relations: `member`, `parent_org`; permission `view = member ∪ parent_org->view` —
//!    a **tuple-to-userset** inheritance from the org), and `project` (relations: `reader`,
//!    `writer`, `parent_team`; permission `view = reader ∪ writer ∪ parent_team->view` — the
//!    `parent_team->view` inheritance the architecture names). A project reader inherits via team
//!    membership; a non-member denies — proven through [`crate::check_engine::CheckEngine`].
//!
//! ## The four Zanzibar userset operators (the design rule, §5)
//! Every permission rewrite is one of:
//! - **`Relation(r)`** — the direct edge `object#r@subject` (a `Userset::Relation`).
//! - **`Union([..])`** — `r1 ∪ r2 ∪ …` (a subject in ANY arm is granted).
//! - **`Intersect([..])`** — `r1 ∩ r2 ∩ …` (a subject in EVERY arm).
//! - **`Exclusion(base, sub)`** — `base − sub` (granted by `base`, NOT by `sub`) — the
//!   `- confidential` exclusion Issues uses, the page-tree `- direct_block` override Knowledge uses.
//! - **`TupleToUserset { tupleset, computed }`** — the inheritance rewrite: "everyone who has
//!   `computed` on the object reached via my `tupleset` relation" (`parent_team->view`).
//!
//! `check(subject, permission, object)` evaluates the permission's rewrite by composing these four
//! over the raw tuples — there is **no fifth operator and no bespoke path** (the architecture's
//! load-bearing design rule).
//!
//! ## ID-D3 (the GATE): no cross-tenant resolution
//! The engine is schema-only (the relations/permissions vocabulary); the *tuples* it resolves over
//! are read through [`crate::check_engine::CheckEngine`], which reads ONLY the verified
//! `(tenant, region)` partition (the `tenant-predicate` floor — there is no cross-tenant query
//! path, §6). So a permission resolved for tenant A over a spoofed path to tenant B's object reads
//! **0** of B's tuples: `cross-tenant-count = 0`. The drill is
//! `tests/drill_id_d3_cross_tenant.rs`.
//!
//! ## Floors named (frozen now → bodies in a later prompt)
//! - **The engine ships with ONLY the org/team/project core.** The five per-subsystem fragments
//!   (Git ref-glob + CODEOWNERS + `approve_untrusted_ci`; CI secret-non-inheritance +
//!   `!is_untrusted_fork`; Issues `confidential` exclusion + field/transition caveats; Knowledge
//!   page-tree-with-overrides + row + field caveat; Chat `channel.read = member + parent_project->read`)
//!   are the **M3/M4 follow-on: P-ID-24 / P-ID-26 / P-ID-27 / P-ID-29 / P-ID-30** (the engine-only
//!   floor is CLOSED by P-ID-30). Named, not silently assumed done. The engine + the
//!   four-operator vocabulary they compile against ship here.
//! - **The `watcher` relation per watchable type** (C8, the Notif read-fanout) is declared by each
//!   watchable type's fragment (M2+), not by the core hierarchy; the engine admits it like any
//!   other relation. **The declaration mechanism LANDED in P-ID-23 (P-134):** [`WATCHER_RELATION`]
//!   (the frozen `watcher` relation name) + [`FragmentDef::watchable`] / [`NamespaceEngine::declare_watchable`]
//!   make a type watchable so `list_subjects(object, watcher)` serves Notif's read-fanout as an
//!   ordinary direct-relation Expand over S8 (one relation name platform-wide; no bespoke fanout
//!   path). Each subsystem's fragment (Chat channel, Issue, KN page — M3/M4) declares watchability
//!   via this helper.

use crate::check_engine::{CheckEngine, USERSET_SEP};
use myelin_identity::{
    FragmentAdmit, NamespaceFragment, ObjectType, Permission, Principal, RelName,
};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;
use std::collections::{BTreeMap, BTreeSet};

/// The maximum permission-rewrite nesting depth admitted (a fragment whose permission tree nests
/// deeper than this is rejected — a structural bound on the schema, distinct from the per-request
/// evaluation bound [`crate::check_engine::MAX_REWRITE_DEPTH`]). Sixteen comfortably exceeds the
/// deepest legitimate `a ∪ (b ∩ (c − d))` rewrite while staying a hard ceiling.
pub const MAX_RULE_DEPTH: usize = 16;

/// **The frozen `watcher` relation name — the cross-cutting read-fanout relation a WATCHABLE type
/// declares (C4/C8, architecture §5: "a `watcher` relation per watchable type … `watcher: user`,
/// so Notif's read-fanout (`list_subjects(object, watcher)`) is served by the same engine + reverse
/// index (S8)").**
///
/// Declared by P-ID-23 (the watcher relation): a watchable type (a Chat channel, an Issue, a KN
/// page — anything Notif can deliver a read-fanout notification about) adds THIS relation through
/// [`FragmentDef::watchable`] / [`NamespaceEngine::declare_watchable`], making
/// `list_subjects(object, watcher)` an **ordinary direct-relation Expand** over S8 (the density path
/// proven in `expand`), NOT a bespoke fanout query. One relation name, platform-wide (EI-01 §7 — one
/// primitive): every watchable type uses the SAME `watcher` relation, so Notif never branches per
/// subsystem and the 50k-density expand is the same indexed reverse lookup regardless of type.
pub const WATCHER_RELATION: &str = "watcher";

/// A Zanzibar **userset** rewrite expression — the four operators (§5) a permission compiles to.
/// `check(subject, permission, object)` evaluates a permission by walking this tree over the raw
/// tuples. There is **no fifth variant** (the architecture's four-operator design rule): every
/// cross-subsystem visibility need reduces to these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Userset {
    /// The **direct edge**: subjects with the `RelName` relation tuple on this object
    /// (`object#relation@subject`). The leaf of every rewrite.
    Relation(RelName),
    /// **Union** `a ∪ b ∪ …`: a subject granted by ANY arm (logical OR).
    Union(Vec<Userset>),
    /// **Intersection** `a ∩ b ∩ …`: a subject granted by EVERY arm (logical AND).
    Intersect(Vec<Userset>),
    /// **Exclusion** `base − subtracted`: granted by `base` and NOT by `subtracted` (the
    /// `- confidential` / `- direct_block` override). The crux of confidential-disappears-by-
    /// construction.
    Exclusion {
        base: Box<Userset>,
        subtracted: Box<Userset>,
    },
    /// **Tuple-to-userset** (the inheritance rewrite, `parent_team->view`): "everyone who has
    /// `computed` on the object reached through my `tupleset` relation". `tupleset` is a relation
    /// whose subjects are the parent objects (as usersets `parent#…`); `computed` is the
    /// permission/relation evaluated on that parent.
    TupleToUserset {
        /// The relation naming the parent object(s) (e.g. `parent_team`).
        tupleset: RelName,
        /// The permission/relation evaluated on the parent (e.g. `view`).
        computed: RelName,
    },
}

/// One permission's compiled rewrite: a permission name → its [`Userset`] expression. A subsystem
/// declares these in its [`FragmentDef`]; the engine compiles + validates them at admit time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRule {
    /// The permission name a `check`/`list_objects` caller asks for (e.g. `view`).
    pub permission: Permission,
    /// Its rewrite over the four operators.
    pub rewrite: Userset,
}

/// A subsystem's **rich** namespace-fragment declaration (the engine-side shape). It carries the
/// permission **rewrite structure** the frozen names-only ABI carrier
/// [`myelin_identity::NamespaceFragment`] cannot — the engine compiles THIS, and the ABI carrier
/// **projects from** it ([`FragmentDef::to_abi`]). A subsystem declares its fragment at build
/// time (the architecture's "declared at build time"); `admit` validates + compiles it.
///
/// **Id never invents object ids:** a `FragmentDef` declares an object TYPE + its relations +
/// permissions — never a concrete object id. (The frozen carrier's fields are all type/relation/
/// permission NAMES for the same reason; [`NamespaceEngine::admit`] rejects any fragment whose
/// names smuggle a concrete id form.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentDef {
    /// The object type this fragment defines (e.g. `org`, `team`, `project`, `repo`, `issue`).
    pub object_type: ObjectType,
    /// The declared **relations** (direct edges) — the tuple relations callers may write.
    pub relations: Vec<RelName>,
    /// The declared **permissions** — computed usersets over the relations (the four operators).
    pub permissions: Vec<PermissionRule>,
}

impl FragmentDef {
    /// Project this rich declaration onto the **frozen** ABI carrier
    /// [`myelin_identity::NamespaceFragment`] (names only). This is the contract shape consumers
    /// admit through; the engine keeps the rewrite structure. (EI-01 §7 — never redefine a frozen
    /// type; the ABI is the names, the engine is the structure.)
    pub fn to_abi(&self) -> NamespaceFragment {
        NamespaceFragment {
            object_type: self.object_type.clone(),
            relations: self.relations.clone(),
            permissions: self.permissions.iter().map(|r| r.permission.clone()).collect(),
        }
    }

    /// **Declare this object type WATCHABLE (P-ID-23, C4/C8): add the cross-cutting
    /// [`WATCHER_RELATION`] (`watcher`) so Notif's read-fanout `list_subjects(object, watcher)` is an
    /// ordinary direct-relation Expand over S8.** A watchable subsystem (Chat channel, Issue, KN page)
    /// calls THIS on its fragment so the `watcher` relation is declared exactly like any other relation
    /// — admitted, validated, and reverse-indexed identically (no bespoke fanout path). Idempotent: a
    /// type that already declares `watcher` is unchanged (so a fragment that already named it, or a
    /// double-call, does not duplicate the relation). Returns `self` for builder chaining.
    pub fn watchable(mut self) -> FragmentDef {
        if !self.relations.iter().any(|r| r.0 == WATCHER_RELATION) {
            self.relations.push(RelName(WATCHER_RELATION.to_string()));
        }
        self
    }

    /// Whether this fragment declares the [`WATCHER_RELATION`] (i.e. its object type is watchable —
    /// Notif can fan out a read-notification over it).
    pub fn is_watchable(&self) -> bool {
        self.relations.iter().any(|r| r.0 == WATCHER_RELATION)
    }
}

/// Why a fragment was rejected at admit time (the structured reason carried in
/// [`FragmentAdmit::Rejected`]). Each is a way a fragment is malformed — surfaced LOUDLY, never
/// silently admitted (EI-01 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmitReject {
    /// The object type name was empty/whitespace.
    EmptyObjectType,
    /// This object type was already admitted (a duplicate definition would shadow the first).
    DuplicateObjectType { object_type: String },
    /// A name that must be a bare identifier carries an object-id form (a `:`/`/`/`#` segment) —
    /// the engine **never invents object ids**, so a fragment may not smuggle one into a
    /// type/relation/permission name.
    NameMintsObjectId { name: String, kind: &'static str },
    /// A permission's rewrite references a relation this fragment did not declare.
    UndeclaredRelation { permission: String, relation: String },
    /// A tuple-to-userset names a `tupleset` relation this fragment did not declare as a relation.
    UndeclaredTupleset { permission: String, tupleset: String },
    /// A permission's rewrite is self-referential (a permission referencing its own name as a
    /// relation, or a duplicate permission name) — a schema cycle.
    PermissionCycle { permission: String },
    /// A permission's rewrite nests deeper than [`MAX_RULE_DEPTH`] (an unbounded schema).
    RuleTooDeep { permission: String },
    /// Two permissions in this fragment share a name (an ambiguous definition).
    DuplicatePermission { permission: String },
}

impl core::fmt::Display for AdmitReject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AdmitReject::EmptyObjectType => write!(f, "the object type name is empty"),
            AdmitReject::DuplicateObjectType { object_type } => {
                write!(f, "object type `{object_type}` is already admitted (duplicate definition)")
            }
            AdmitReject::NameMintsObjectId { name, kind } => write!(
                f,
                "the {kind} name `{name}` carries an object-id form (`:`/`/`/`#`) — Id never \
                 invents object ids; a fragment declares types/relations/permissions, never ids"
            ),
            AdmitReject::UndeclaredRelation { permission, relation } => write!(
                f,
                "permission `{permission}` references relation `{relation}`, which this fragment \
                 did not declare"
            ),
            AdmitReject::UndeclaredTupleset { permission, tupleset } => write!(
                f,
                "permission `{permission}` inherits via tupleset `{tupleset}`, which this fragment \
                 did not declare as a relation"
            ),
            AdmitReject::PermissionCycle { permission } => {
                write!(f, "permission `{permission}` is self-referential (a schema cycle)")
            }
            AdmitReject::RuleTooDeep { permission } => write!(
                f,
                "permission `{permission}` nests deeper than the admit bound ({MAX_RULE_DEPTH}) — \
                 an unbounded schema"
            ),
            AdmitReject::DuplicatePermission { permission } => {
                write!(f, "permission `{permission}` is declared twice in this fragment")
            }
        }
    }
}

/// The compiled cell schema (architecture §5: admitted fragments compile into ONE cell schema).
/// Holds every admitted object type's relations + compiled permission rewrites. The engine is
/// schema-only — it carries **no tuples and no tenant state**; tuple resolution is
/// [`CheckEngine`], which reads only the verified `(tenant, region)` partition (so the engine
/// adds no cross-tenant path: ID-D3).
#[derive(Clone, Debug, Default)]
pub struct NamespaceEngine {
    /// object_type → (relations, permission_name → rewrite). A `BTreeMap` so admit order is
    /// irrelevant and the schema is deterministic.
    schema: BTreeMap<String, CompiledType>,
}

/// One compiled object type in the schema: its declared relations + its permission rewrites.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CompiledType {
    relations: BTreeSet<String>,
    permissions: BTreeMap<String, Userset>,
}

impl NamespaceEngine {
    /// A new, empty engine (no fragments admitted). Use [`NamespaceEngine::with_core_hierarchy`]
    /// for the org/team/project core the cell ships with.
    pub fn new() -> NamespaceEngine {
        NamespaceEngine {
            schema: BTreeMap::new(),
        }
    }

    /// The engine pre-loaded with the **core org→team→project hierarchy** (the three namespaces
    /// the architecture ships in the engine, §5). Every cell starts from this; subsystem fragments
    /// (M3/M4) are admitted on top.
    pub fn with_core_hierarchy() -> NamespaceEngine {
        let mut eng = NamespaceEngine::new();
        for frag in core_hierarchy() {
            // The core hierarchy is well-formed by construction; an admit failure here is a bug in
            // the core declaration, so we surface it loudly rather than silently dropping it.
            match eng.admit(&frag) {
                FragmentAdmit::Admitted { .. } => {}
                FragmentAdmit::Rejected { reason } => {
                    panic!("the core hierarchy fragment must admit, but was rejected: {reason}")
                }
            }
        }
        eng
    }

    /// **Admit a subsystem's namespace fragment into the cell schema (contract 4.9).** Validate it
    /// at build/admit time and either compile it into the schema (`Admitted{fragment_id}`) or
    /// reject it loudly (`Rejected{reason}`). The validation (each a [`AdmitReject`]):
    /// - the object type is non-empty and not already admitted;
    /// - no type/relation/permission name smuggles an object-id form (**Id never invents ids**);
    /// - every relation a permission references is declared by this fragment;
    /// - every tuple-to-userset `tupleset` is a declared relation;
    /// - no permission is self-referential / duplicated; no rewrite exceeds the depth bound.
    ///
    /// Idempotent: re-admitting the SAME object type is a [`AdmitReject::DuplicateObjectType`]
    /// (a fragment is admitted once; a re-declaration is a schema error, not a silent overwrite).
    pub fn admit(&mut self, frag: &FragmentDef) -> FragmentAdmit {
        match self.validate(frag) {
            Err(reject) => FragmentAdmit::Rejected {
                reason: reject.to_string(),
            },
            Ok(compiled) => {
                let ot = frag.object_type.0.clone();
                self.schema.insert(ot.clone(), compiled);
                FragmentAdmit::Admitted {
                    // The compiled fragment id is the object type (one fragment == one type's
                    // schema slice). Stable + deterministic.
                    fragment_id: ot,
                }
            }
        }
    }

    /// **Admit the names-only ABI carrier [`NamespaceFragment`]** (the contract-4.9 boundary
    /// shape) — the validator [`crate::IdentityService::admit_fragment`] runs. The ABI carries
    /// type + relation + permission NAMES but not the rewrite structure; this validates the
    /// names-level well-formedness (non-empty type, no id-minting, no duplicate type/permission)
    /// and admits each permission as a **direct relation of the same name** when that relation is
    /// declared, rejecting a permission that names no declared relation (an under-specified
    /// fragment). The RICH rewrite-carrying declaration is [`NamespaceEngine::admit`] over a
    /// [`FragmentDef`] (the path the M3/M4 fragment prompts use); the two share this engine.
    pub fn admit_abi(&mut self, frag: &NamespaceFragment) -> FragmentAdmit {
        // Project the names-only fragment onto a FragmentDef whose permissions default to a direct
        // relation of the same name (the only rewrite a names-only carrier can express). A
        // permission whose name is not a declared relation is rejected by `validate` (an undeclared
        // relation) — the names-only carrier cannot under-specify a permission silently.
        let def = FragmentDef {
            object_type: frag.object_type.clone(),
            relations: frag.relations.clone(),
            permissions: frag
                .permissions
                .iter()
                .map(|p| PermissionRule {
                    permission: p.clone(),
                    rewrite: Userset::Relation(RelName(p.0.clone())),
                })
                .collect(),
        };
        self.admit(&def)
    }

    /// Validate + compile a fragment without mutating the schema (so a rejection changes nothing).
    fn validate(&self, frag: &FragmentDef) -> Result<CompiledType, AdmitReject> {
        let ot = frag.object_type.0.trim();
        if ot.is_empty() {
            return Err(AdmitReject::EmptyObjectType);
        }
        // Id never invents object ids: a TYPE name is a bare identifier, never an id form.
        if mints_object_id(ot) {
            return Err(AdmitReject::NameMintsObjectId {
                name: ot.to_string(),
                kind: "object type",
            });
        }
        if self.schema.contains_key(ot) {
            return Err(AdmitReject::DuplicateObjectType {
                object_type: ot.to_string(),
            });
        }

        // The declared relation set (deduped). A relation name is a bare identifier (no id form).
        let mut relations: BTreeSet<String> = BTreeSet::new();
        for r in &frag.relations {
            if mints_object_id(&r.0) {
                return Err(AdmitReject::NameMintsObjectId {
                    name: r.0.clone(),
                    kind: "relation",
                });
            }
            relations.insert(r.0.clone());
        }

        // Compile every permission rewrite, validating its referenced relations against the
        // declared set + its tuple-to-userset tuplesets + its depth + self-reference.
        let mut permissions: BTreeMap<String, Userset> = BTreeMap::new();
        for rule in &frag.permissions {
            let pname = rule.permission.0.clone();
            if mints_object_id(&pname) {
                return Err(AdmitReject::NameMintsObjectId {
                    name: pname,
                    kind: "permission",
                });
            }
            if permissions.contains_key(&pname) {
                return Err(AdmitReject::DuplicatePermission { permission: pname });
            }
            // A permission may not reference its own name as a relation (a direct self-cycle).
            // (Tuple-to-userset inheritance to a DIFFERENT object's same-named permission is fine —
            // that terminates on the parent's tuples; only a same-object self-relation is a cycle.)
            self.validate_rewrite(&pname, &relations, &rule.rewrite, 0)?;
            permissions.insert(pname, rule.rewrite.clone());
        }

        Ok(CompiledType {
            relations,
            permissions,
        })
    }

    /// Validate one rewrite expression: every `Relation`/tupleset it names is a declared relation,
    /// it does not self-reference `permission`, and it does not nest past [`MAX_RULE_DEPTH`].
    fn validate_rewrite(
        &self,
        permission: &str,
        relations: &BTreeSet<String>,
        rewrite: &Userset,
        depth: usize,
    ) -> Result<(), AdmitReject> {
        if depth > MAX_RULE_DEPTH {
            return Err(AdmitReject::RuleTooDeep {
                permission: permission.to_string(),
            });
        }
        match rewrite {
            Userset::Relation(r) => {
                // The leaf must be a DECLARED relation. A permission referencing a name that is
                // NOT a declared relation is undeclared; if that name is the permission's OWN name
                // (and not a declared relation), it is a self-cycle (a permission defined in terms
                // of itself). When the permission name IS also a declared relation, `Relation(name)`
                // is a legitimate direct passthrough (the names-only ABI's permission==relation
                // case), not a cycle.
                if !relations.contains(&r.0) {
                    if r.0 == permission {
                        return Err(AdmitReject::PermissionCycle {
                            permission: permission.to_string(),
                        });
                    }
                    return Err(AdmitReject::UndeclaredRelation {
                        permission: permission.to_string(),
                        relation: r.0.clone(),
                    });
                }
                Ok(())
            }
            Userset::Union(arms) | Userset::Intersect(arms) => {
                for arm in arms {
                    self.validate_rewrite(permission, relations, arm, depth + 1)?;
                }
                Ok(())
            }
            Userset::Exclusion { base, subtracted } => {
                self.validate_rewrite(permission, relations, base, depth + 1)?;
                self.validate_rewrite(permission, relations, subtracted, depth + 1)
            }
            Userset::TupleToUserset { tupleset, .. } => {
                // The tupleset (the relation naming the parent objects) must be a declared
                // relation; the `computed` permission/relation is resolved on the PARENT object's
                // schema at check-time (it need not be declared here).
                if !relations.contains(&tupleset.0) {
                    return Err(AdmitReject::UndeclaredTupleset {
                        permission: permission.to_string(),
                        tupleset: tupleset.0.clone(),
                    });
                }
                Ok(())
            }
        }
    }

    /// Resolve a **permission** name on an object type to its compiled [`Userset`] rewrite (so
    /// [`CheckEngine`] can evaluate it through the four operators). Returns `None` if the object
    /// type is unknown OR the name is not a declared permission — in which case the caller treats
    /// the name as a **direct relation** (a bare relation check is `Userset::Relation(name)`).
    ///
    /// `object_type` is the object's type (e.g. `project`); `permission` is the name a `check`
    /// caller asked for (e.g. `view`).
    pub fn resolve_permission(&self, object_type: &str, permission: &str) -> Option<Userset> {
        self.schema
            .get(object_type)
            .and_then(|t| t.permissions.get(permission).cloned())
    }

    /// The declared relations of an object type, sorted (the vocabulary `list_objects` unions the S8
    /// candidate lookup over — P-ID-11). An unknown type has no relations (empty). Returned as owned
    /// `String`s so the caller (the S4 candidate path) does not hold the schema lock.
    pub fn relations_of(&self, object_type: &str) -> Vec<String> {
        self.schema
            .get(object_type)
            .map(|t| t.relations.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Whether `relation` is a declared relation on `object_type` (used by `check` to decide a
    /// bare relation vs an unknown name).
    pub fn has_relation(&self, object_type: &str, relation: &str) -> bool {
        self.schema
            .get(object_type)
            .map(|t| t.relations.contains(relation))
            .unwrap_or(false)
    }

    /// The admitted object types (the compiled cell schema's vocabulary), sorted.
    pub fn object_types(&self) -> Vec<String> {
        self.schema.keys().cloned().collect()
    }

    /// **Whether `object_type` is WATCHABLE (P-ID-23, C8) — i.e. it declares the
    /// [`WATCHER_RELATION`], so `list_subjects(object, watcher)` serves Notif's read-fanout over it.**
    /// A watchable type's `watcher` relation is an ordinary declared relation (it reverse-indexes into
    /// S8 like any other), so this is simply [`NamespaceEngine::has_relation`] on `watcher` — exposed
    /// as a named query so a watchable subsystem / a drill can assert the relation is declared (the
    /// fanout is wired) without reaching into the relation vocabulary.
    pub fn is_watchable(&self, object_type: &str) -> bool {
        self.has_relation(object_type, WATCHER_RELATION)
    }

    /// The admitted object types that are WATCHABLE (declare the [`WATCHER_RELATION`]), sorted — the
    /// set of types Notif can fan a read-notification out over via `list_subjects(object, watcher)`.
    pub fn watchable_types(&self) -> Vec<String> {
        self.schema
            .iter()
            .filter(|(_, t)| t.relations.contains(WATCHER_RELATION))
            .map(|(ot, _)| ot.clone())
            .collect()
    }

    /// **Declare an ALREADY-ADMITTED object type WATCHABLE (P-ID-23, C8): add the
    /// [`WATCHER_RELATION`] to its relation set in place.** This is the engine-side of "every watchable
    /// type declares `watcher: user`" for a type whose fragment was admitted without it (e.g. the
    /// core hierarchy, or a subsystem fragment that declares watchability separately). Returns:
    /// - `FragmentAdmit::Admitted{fragment_id}` (the object type) when the relation is added (or was
    ///   already present — idempotent, so re-declaring is a no-op success, not a duplicate error);
    /// - `FragmentAdmit::Rejected{reason}` when the type is not admitted (a watcher relation cannot be
    ///   attached to a type the cell schema does not know — loudly, never silently created).
    ///
    /// The added `watcher` relation participates in `check`/`list_subjects`/`list_objects` exactly like
    /// any declared relation (one primitive — no bespoke watcher path).
    pub fn declare_watchable(&mut self, object_type: &str) -> FragmentAdmit {
        match self.schema.get_mut(object_type) {
            Some(t) => {
                t.relations.insert(WATCHER_RELATION.to_string());
                FragmentAdmit::Admitted {
                    fragment_id: object_type.to_string(),
                }
            }
            None => FragmentAdmit::Rejected {
                reason: format!(
                    "cannot declare `{object_type}` watchable: it is not an admitted object type (a \
                     watcher relation attaches to a known type, never invents one)"
                ),
            },
        }
    }

    /// **Evaluate a permission through the engine's compiled rewrite over the raw tuples** — the
    /// bridge that lets a `check(subject, permission, object)` resolve a *compiled permission* (not
    /// just a raw relation) by walking the four operators. This composes the [`Userset`] tree over
    /// [`CheckEngine`]'s direct-relation resolution (one primitive, EI-01 §7: the operators reduce
    /// to direct/userset relation checks the engine already evaluates).
    ///
    /// - `engine` is the live [`CheckEngine`] over the verified scope (the only tuple-reading path;
    ///   it reads ONLY `scope`'s `(tenant, region)` partition — no cross-tenant path, ID-D3).
    /// - `object_type` is the object's type (to look up the permission's rewrite); if the
    ///   permission is not a declared permission of that type, it is resolved as a **direct
    ///   relation** (`object#permission@subject`).
    #[allow(clippy::too_many_arguments)]
    pub fn permits(
        &self,
        engine: &CheckEngine,
        scope: &TenantScope,
        subject: &Principal,
        object_type: &str,
        permission: &str,
        object: &ArtifactRef,
        at: &myelin_identity::Consistency,
    ) -> bool {
        self.eval(engine, scope, subject, object_type, permission, object, at, 0)
    }

    /// Resolve a `permission`/relation on `object` (of `object_type`) for `subject`, permission-
    /// aware. A **compiled permission** is rewritten through its [`Userset`] tree (the four
    /// operators); a name that is NOT a compiled permission is a **direct relation** check served
    /// by the raw [`CheckEngine`] (one primitive — the engine composes over the same rewrite the
    /// raw check already evaluates). Depth-bounded ([`MAX_RULE_DEPTH`]) so a pathological
    /// permission graph cannot diverge.
    #[allow(clippy::too_many_arguments)]
    fn eval(
        &self,
        engine: &CheckEngine,
        scope: &TenantScope,
        subject: &Principal,
        object_type: &str,
        permission: &str,
        object: &ArtifactRef,
        at: &myelin_identity::Consistency,
        depth: usize,
    ) -> bool {
        if depth > MAX_RULE_DEPTH {
            // Genuine uncertainty (too deep) → fail-closed (false), never allow-by-exhaustion.
            return false;
        }
        match self.resolve_permission(object_type, permission) {
            // A compiled permission → walk its rewrite over the four operators.
            Some(rewrite) => {
                self.eval_userset(engine, scope, subject, object, at, &rewrite, depth)
            }
            // Not a compiled permission → a direct relation check (the raw-tuple floor P-ID-09).
            None => matches!(
                engine.check(scope, subject, &RelName(permission.to_string()), object, at, None),
                myelin_identity::Decision::Allow
            ),
        }
    }

    /// Walk a [`Userset`] rewrite over the tuples. `Relation` and the operators reduce to
    /// direct-relation checks the raw engine evaluates; `TupleToUserset` walks the inheritance edge
    /// into the **parent's compiled permission** (permission-aware, so `parent_team->view` resolves
    /// the parent team's `view = member ∪ parent_org->view`, not a raw `view` tuple).
    #[allow(clippy::too_many_arguments)]
    fn eval_userset(
        &self,
        engine: &CheckEngine,
        scope: &TenantScope,
        subject: &Principal,
        object: &ArtifactRef,
        at: &myelin_identity::Consistency,
        rewrite: &Userset,
        depth: usize,
    ) -> bool {
        if depth > MAX_RULE_DEPTH {
            return false;
        }
        match rewrite {
            Userset::Relation(r) => {
                // A direct relation check on this object via the depth-bounded, memoised,
                // fail-closed raw engine (Allow ⇒ true; Deny/Conditional ⇒ false).
                matches!(
                    engine.check(scope, subject, r, object, at, None),
                    myelin_identity::Decision::Allow
                )
            }
            Userset::Union(arms) => arms.iter().any(|a| {
                self.eval_userset(engine, scope, subject, object, at, a, depth + 1)
            }),
            Userset::Intersect(arms) => arms.iter().all(|a| {
                self.eval_userset(engine, scope, subject, object, at, a, depth + 1)
            }),
            Userset::Exclusion { base, subtracted } => {
                self.eval_userset(engine, scope, subject, object, at, base, depth + 1)
                    && !self.eval_userset(
                        engine, scope, subject, object, at, subtracted, depth + 1,
                    )
            }
            Userset::TupleToUserset { tupleset, computed } => {
                // Inheritance (`parent_team->view`): read the parent objects named by `tupleset`
                // (the child's tuple `child#<tupleset>@(parent#<computed>)`), then resolve the
                // parent's COMPILED `computed` permission on each parent (permission-aware — this
                // is what makes the parent's own union/inheritance resolve, not a raw tuple). The
                // parent objects are the userset subjects of `child#<tupleset>`.
                let parents = engine.direct_subjects(scope, object, tupleset, at);
                parents.iter().any(|parent_subject| {
                    // A parent named as a userset `parent#computed` — walk the parent's `computed`
                    // permission. The data writes the inheritance edge with the SAME `computed`
                    // relation name as the rewrite asks for (the typed-edge discipline); we resolve
                    // the parent's compiled permission of that name.
                    match crate::check_engine::parse_userset(parent_subject) {
                        Some((parent_id, parent_rel)) if parent_rel == computed.0 => {
                            let parent_type = type_of_object_id(parent_id);
                            self.eval(
                                engine,
                                scope,
                                subject,
                                &parent_type,
                                computed.0.as_str(),
                                &ArtifactRef(parent_id.to_string()),
                                at,
                                depth + 1,
                            )
                        }
                        // A concrete-principal subject on the tupleset edge (a degenerate direct
                        // grant) → the subject is granted iff it IS this principal.
                        _ => parent_subject == &subject.principal_id.0,
                    }
                })
            }
        }
    }
}

/// Infer an object's TYPE from its id by the leading `type:` prefix (`team:eng` → `team`,
/// `project:web` → `project`). A bare id with no prefix has no inferable type (returns the id
/// itself, so a missing-type lookup simply resolves no compiled permission and falls through to a
/// direct relation check — the safe floor). The owning subsystem mints the id; the `type:` prefix
/// is the convention the core hierarchy + the M3/M4 fragments follow.
fn type_of_object_id(object_id: &str) -> String {
    object_id
        .split_once(':')
        .map(|(ty, _)| ty.to_string())
        .unwrap_or_else(|| object_id.to_string())
}

/// Infer an object's TYPE from a contract-boundary [`ArtifactRef`] — the namespace-engine bridge
/// `check` uses to look up a compiled permission. A full URN
/// (`myelin://acme/issues/issue/issue:PROJ-1`) carries the type as the second-to-last path segment;
/// a bare `type:id` carries it as the `type:` prefix. Returns `""` when no type is inferable, so
/// the permission resolves to no compiled rewrite and falls through to a direct relation check (the
/// safe floor — never an over-broad grant).
pub fn type_of_object_ref(object: &ArtifactRef) -> String {
    let raw = object.0.trim();
    if raw.is_empty() {
        return String::new();
    }
    // Strip a trailing `#sub` anchor (the sub-artifact shares the root object's type).
    let root = raw.split(USERSET_SEP).next().unwrap_or(raw);
    // A full URN: the type is the path segment before the final id segment
    // (`…/<type>/<id>`). A bare `type:id`: the `type:` prefix.
    if root.contains('/') {
        let segs: Vec<&str> = root.rsplit('/').collect();
        // segs[0] = id, segs[1] = type (the §7.3 id-column type discriminant).
        if segs.len() >= 2 {
            return type_of_object_id(segs[1]);
        }
    }
    type_of_object_id(root)
}

/// Whether a name carries an **object-id form** — a `:`, `/`, or `#` segment that would make it a
/// concrete object id rather than a bare type/relation/permission identifier. The engine **never
/// invents object ids**, so a fragment may not declare one (architecture §5; the test
/// `a_fragment_cannot_mint_object_ids`).
fn mints_object_id(name: &str) -> bool {
    name.contains(':') || name.contains('/') || name.contains(USERSET_SEP)
}

/// The **core org→team→project hierarchy** (architecture §5 — the three namespaces shipped in the
/// engine). Returned as [`FragmentDef`]s so the engine admits them through the SAME admit path a
/// subsystem fragment uses (no special core path — one primitive).
///
/// - **`org`** — relations `member`, `admin`; permission `view = member ∪ admin`.
/// - **`team`** — relations `member`, `parent_org`; permission
///   `view = member ∪ parent_org->view` (a **tuple-to-userset** inheritance from the org).
/// - **`project`** — relations `reader`, `writer`, `parent_team`; permission
///   `view = reader ∪ writer ∪ parent_team->view` (the `parent_team->view` inheritance the
///   architecture names). A project reader inherits view via team membership; a non-member denies.
pub fn core_hierarchy() -> Vec<FragmentDef> {
    vec![
        // org: the top of the hierarchy.
        FragmentDef {
            object_type: ObjectType("org".into()),
            relations: vec![RelName("member".into()), RelName("admin".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("member".into())),
                    Userset::Relation(RelName("admin".into())),
                ]),
            }],
        },
        // team: inherits view from its parent org (tuple-to-userset).
        FragmentDef {
            object_type: ObjectType("team".into()),
            relations: vec![RelName("member".into()), RelName("parent_org".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("member".into())),
                    Userset::TupleToUserset {
                        tupleset: RelName("parent_org".into()),
                        computed: RelName("view".into()),
                    },
                ]),
            }],
        },
        // project: inherits view from its parent team (the `parent_team->view` rewrite §5 names).
        FragmentDef {
            object_type: ObjectType("project".into()),
            relations: vec![
                RelName("reader".into()),
                RelName("writer".into()),
                RelName("parent_team".into()),
            ],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("reader".into())),
                    Userset::Relation(RelName("writer".into())),
                    Userset::TupleToUserset {
                        tupleset: RelName("parent_team".into()),
                        computed: RelName("view".into()),
                    },
                ]),
            }],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{OutboxStore, Timestamp};
    use myelin_identity::{
        ConsistencyMode, Consistency, ObjectId, PrincipalId, PrincipalKind, RelationTuple,
        TupleDelta, Zookie,
    };
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region, TenantId};

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn subject(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
        TupleDelta::Add(RelationTuple {
            object: ObjectId(object.into()),
            relation: RelName(relation.into()),
            subject: PrincipalId(subject.into()),
            caveat: None,
        })
    }

    fn latest() -> Consistency {
        Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn engine_with(scope: &TenantScope, tuples: &[TupleDelta]) -> CheckEngine {
        let store = TupleStore::new(OutboxStore::new());
        store
            .write_tuples(scope, &subject("p-admin"), tuples, None, None, now())
            .expect("seed tuples");
        CheckEngine::new(store)
    }

    use crate::tuple_store::TupleStore;
    fn now() -> Timestamp {
        Timestamp("2026-06-19T00:00:00Z".into())
    }

    /// **The core org→team→project hierarchy resolves inheritance: a project-reader granted via
    /// team membership Allows; a non-member Denies** (the headline engine test).
    ///
    /// Data: `project:web#parent_team@(team:eng#view)` (the project inherits view from team:eng) +
    /// `team:eng#member@p:alice`. Then `permits(alice, view, project:web)` must hold (alice is a
    /// member of the team the project inherits from), while bob (no membership) denies.
    #[test]
    fn core_hierarchy_project_reader_via_team_membership_allows_nonmember_denies() {
        let ns = NamespaceEngine::with_core_hierarchy();
        let s = scope("acme");
        let eng = engine_with(
            &s,
            &[
                // project:web inherits view from team:eng (the parent_team->view edge, as data:
                // project:web#parent_team@(team:eng#view)).
                add("project:web", "parent_team", "team:eng#view"),
                // team:eng's view is `member ∪ parent_org->view`; alice is a direct member.
                add("team:eng", "member", "p:alice"),
            ],
        );

        // alice inherits project:web view via team:eng membership → Allow.
        assert!(
            ns.permits(
                &eng,
                &s,
                &subject("p:alice"),
                "project",
                "view",
                &ArtifactRef("project:web".into()),
                &latest(),
            ),
            "a project reader granted via team membership inherits view (parent_team->view)"
        );
        // bob is not a member → denies.
        assert!(
            !ns.permits(
                &eng,
                &s,
                &subject("p:bob"),
                "project",
                "view",
                &ArtifactRef("project:web".into()),
                &latest(),
            ),
            "a non-member does not inherit project view"
        );
    }

    /// **A direct project reader Allows (the union's direct arm).** alice has the `reader` relation
    /// directly on project:web → `view = reader ∪ writer ∪ parent_team->view` grants.
    #[test]
    fn core_hierarchy_direct_reader_allows() {
        let ns = NamespaceEngine::with_core_hierarchy();
        let s = scope("acme");
        let eng = engine_with(&s, &[add("project:web", "reader", "p:alice")]);
        assert!(ns.permits(
            &eng,
            &s,
            &subject("p:alice"),
            "project",
            "view",
            &ArtifactRef("project:web".into()),
            &latest(),
        ));
    }

    /// **The four Zanzibar operators each evaluate.** Build an ad-hoc fragment exercising
    /// `Union`, `Intersect`, `Exclusion`, and `TupleToUserset`, admit it, and assert each operator's
    /// semantics through `permits`.
    #[test]
    fn the_four_userset_operators_each_evaluate() {
        let mut ns = NamespaceEngine::new();
        // A `doc` type: reader (direct), `secret` (a relation), `blocked` (a relation), parent.
        let frag = FragmentDef {
            object_type: ObjectType("doc".into()),
            relations: vec![
                RelName("reader".into()),
                RelName("editor".into()),
                RelName("blocked".into()),
                RelName("parent".into()),
            ],
            permissions: vec![
                // union: read = reader ∪ editor
                PermissionRule {
                    permission: Permission("read".into()),
                    rewrite: Userset::Union(vec![
                        Userset::Relation(RelName("reader".into())),
                        Userset::Relation(RelName("editor".into())),
                    ]),
                },
                // intersect: review = reader ∩ editor (must be BOTH)
                PermissionRule {
                    permission: Permission("review".into()),
                    rewrite: Userset::Intersect(vec![
                        Userset::Relation(RelName("reader".into())),
                        Userset::Relation(RelName("editor".into())),
                    ]),
                },
                // exclusion: view = reader − blocked (reader but NOT blocked)
                PermissionRule {
                    permission: Permission("view".into()),
                    rewrite: Userset::Exclusion {
                        base: Box::new(Userset::Relation(RelName("reader".into()))),
                        subtracted: Box::new(Userset::Relation(RelName("blocked".into()))),
                    },
                },
                // tuple-to-userset: inherit = parent->read
                PermissionRule {
                    permission: Permission("inherit".into()),
                    rewrite: Userset::TupleToUserset {
                        tupleset: RelName("parent".into()),
                        computed: RelName("read".into()),
                    },
                },
            ],
        };
        assert!(matches!(ns.admit(&frag), FragmentAdmit::Admitted { .. }));

        let s = scope("acme");
        let eng = engine_with(
            &s,
            &[
                add("doc:1", "reader", "p:alice"),
                add("doc:1", "editor", "p:alice"),
                add("doc:1", "reader", "p:bob"),
                add("doc:1", "blocked", "p:bob"),
                // doc:2 inherits read from doc:1 (parent->read): doc:2#parent@(doc:1#reader)
                add("doc:2", "parent", "doc:1#reader"),
            ],
        );
        let obj1 = ArtifactRef("doc:1".into());

        // UNION read = reader ∪ editor: alice (both) and bob (reader) both read.
        assert!(ns.permits(&eng, &s, &subject("p:alice"), "doc", "read", &obj1, &latest()));
        assert!(ns.permits(&eng, &s, &subject("p:bob"), "doc", "read", &obj1, &latest()));

        // INTERSECT review = reader ∩ editor: only alice (both); bob is reader-only → deny.
        assert!(ns.permits(&eng, &s, &subject("p:alice"), "doc", "review", &obj1, &latest()));
        assert!(!ns.permits(&eng, &s, &subject("p:bob"), "doc", "review", &obj1, &latest()));

        // EXCLUSION view = reader − blocked: alice (reader, not blocked) allows; bob (reader BUT
        // blocked) denies (the confidential-disappears-by-construction crux).
        assert!(ns.permits(&eng, &s, &subject("p:alice"), "doc", "view", &obj1, &latest()));
        assert!(
            !ns.permits(&eng, &s, &subject("p:bob"), "doc", "view", &obj1, &latest()),
            "exclusion: a blocked reader is excluded (− blocked)"
        );
    }

    /// **The admit-contract validates a well-formed fragment and rejects a malformed one.**
    #[test]
    fn admit_validates_well_formed_and_rejects_malformed() {
        let mut ns = NamespaceEngine::new();
        // well-formed: admits.
        let ok = FragmentDef {
            object_type: ObjectType("repo".into()),
            relations: vec![RelName("reader".into()), RelName("writer".into())],
            permissions: vec![PermissionRule {
                permission: Permission("read".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("reader".into())),
                    Userset::Relation(RelName("writer".into())),
                ]),
            }],
        };
        assert!(matches!(ns.admit(&ok), FragmentAdmit::Admitted { fragment_id } if fragment_id == "repo"));

        // malformed: a permission references an UNDECLARED relation → Rejected.
        let bad = FragmentDef {
            object_type: ObjectType("ci_run".into()),
            relations: vec![RelName("triggerer".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                // `reader` is not declared on ci_run.
                rewrite: Userset::Relation(RelName("reader".into())),
            }],
        };
        match ns.admit(&bad) {
            FragmentAdmit::Rejected { reason } => {
                assert!(reason.contains("reader"), "the rejection names the undeclared relation: {reason}");
            }
            FragmentAdmit::Admitted { .. } => panic!("a malformed fragment must be rejected"),
        }
        // The rejected fragment did NOT enter the schema.
        assert!(!ns.object_types().contains(&"ci_run".to_string()));
    }

    /// **A fragment cannot mint object ids** (architecture §5 — Id never invents object ids). A
    /// type/relation/permission name carrying an id form (`:`/`/`/`#`) is rejected.
    #[test]
    fn a_fragment_cannot_mint_object_ids() {
        let mut ns = NamespaceEngine::new();
        // A type name carrying a concrete id form.
        let id_type = FragmentDef {
            object_type: ObjectType("repo:core".into()), // an OBJECT ID, not a type
            relations: vec![RelName("reader".into())],
            permissions: vec![],
        };
        assert!(
            matches!(ns.admit(&id_type), FragmentAdmit::Rejected { .. }),
            "a type name that is actually an object id is rejected (Id never invents ids)"
        );
        // A relation name carrying an id form.
        let id_rel = FragmentDef {
            object_type: ObjectType("repo".into()),
            relations: vec![RelName("repo:core#reader".into())],
            permissions: vec![],
        };
        assert!(matches!(ns.admit(&id_rel), FragmentAdmit::Rejected { .. }));
    }

    /// **A self-referential permission (a schema cycle) is rejected.**
    #[test]
    fn self_referential_permission_is_rejected() {
        let mut ns = NamespaceEngine::new();
        let cyclic = FragmentDef {
            object_type: ObjectType("page".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![PermissionRule {
                permission: Permission("read".into()),
                // read references `read` as a relation → a self-cycle.
                rewrite: Userset::Relation(RelName("read".into())),
            }],
        };
        match ns.admit(&cyclic) {
            FragmentAdmit::Rejected { reason } => assert!(reason.contains("cycle") || reason.contains("self")),
            FragmentAdmit::Admitted { .. } => panic!("a self-referential permission must be rejected"),
        }
    }

    /// **A duplicate object type is rejected** (a re-declaration must not silently shadow).
    #[test]
    fn duplicate_object_type_is_rejected() {
        let mut ns = NamespaceEngine::new();
        let frag = FragmentDef {
            object_type: ObjectType("space".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![],
        };
        assert!(matches!(ns.admit(&frag), FragmentAdmit::Admitted { .. }));
        assert!(
            matches!(ns.admit(&frag), FragmentAdmit::Rejected { .. }),
            "re-admitting the same object type is a duplicate-definition rejection"
        );
    }

    /// **The frozen ABI carrier projects from the rich FragmentDef (names only).** The engine
    /// carries the rewrite structure; `to_abi()` is the names-only `NamespaceFragment` consumers
    /// admit through — proving the engine extends, not re-defines, the frozen shape.
    #[test]
    fn fragment_def_projects_onto_the_frozen_abi_names() {
        let frag = &core_hierarchy()[2]; // project
        let abi = frag.to_abi();
        assert_eq!(abi.object_type, ObjectType("project".into()));
        assert!(abi.relations.contains(&RelName("parent_team".into())));
        // the ABI carries permission NAMES (not the rewrite) — the structure lives in the engine.
        assert_eq!(abi.permissions, vec![Permission("view".into())]);
    }

    /// **The core hierarchy admits through the engine + exposes its vocabulary.**
    #[test]
    fn core_hierarchy_admits_and_exposes_vocabulary() {
        let ns = NamespaceEngine::with_core_hierarchy();
        assert_eq!(ns.object_types(), vec!["org", "project", "team"]);
        assert!(ns.has_relation("project", "parent_team"));
        assert!(ns.resolve_permission("project", "view").is_some());
        // a non-declared permission resolves to None (the caller treats it as a direct relation).
        assert!(ns.resolve_permission("project", "delete").is_none());
    }

    /// **No cross-tenant resolution through the engine (ID-D3 unit-level).** A grant under tenant
    /// `acme` does not permit the same `permits` evaluated under tenant `globex` — the engine adds
    /// no cross-tenant path (it resolves through CheckEngine, which reads only the verified scope).
    #[test]
    fn engine_adds_no_cross_tenant_path() {
        let ns = NamespaceEngine::with_core_hierarchy();
        let acme = scope("acme");
        let globex = scope("globex");
        let store = TupleStore::new(OutboxStore::new());
        store
            .write_tuples(&acme, &subject("p-admin"), &[add("project:web", "reader", "p:alice")], None, None, now())
            .expect("acme grant");
        let eng = CheckEngine::new(store);
        // Under acme: allow.
        assert!(ns.permits(&eng, &acme, &subject("p:alice"), "project", "view", &ArtifactRef("project:web".into()), &latest()));
        // Under globex: the acme grant is invisible → deny (0 cross-tenant tuples).
        assert!(
            !ns.permits(&eng, &globex, &subject("p:alice"), "project", "view", &ArtifactRef("project:web".into()), &latest()),
            "a grant in one tenant does not permit a resolution in another (ID-D3)"
        );
    }

    /// **P-ID-23 — a watchable type declares the `watcher` relation (C4/C8).** `FragmentDef::watchable`
    /// adds the cross-cutting `watcher` relation; once admitted, `is_watchable` holds and the relation
    /// is an ordinary declared relation (so `list_subjects(object, watcher)` is an ordinary Expand).
    #[test]
    fn a_watchable_fragment_declares_the_watcher_relation() {
        let mut ns = NamespaceEngine::new();
        // A `channel` type made watchable (the Chat M4 shape: channel + watcher).
        let frag = FragmentDef {
            object_type: ObjectType("channel".into()),
            relations: vec![RelName("member".into())],
            permissions: vec![],
        }
        .watchable();
        assert!(frag.is_watchable(), "the fragment declares the watcher relation");
        assert!(matches!(ns.admit(&frag), FragmentAdmit::Admitted { .. }));
        // The admitted type is watchable + the `watcher` relation is an ordinary declared relation.
        assert!(ns.is_watchable("channel"));
        assert!(ns.has_relation("channel", WATCHER_RELATION));
        assert_eq!(ns.watchable_types(), vec!["channel".to_string()]);
        // A non-watchable type is not in the watchable set.
        let plain = FragmentDef {
            object_type: ObjectType("secret".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![],
        };
        assert!(matches!(ns.admit(&plain), FragmentAdmit::Admitted { .. }));
        assert!(!ns.is_watchable("secret"));
        assert_eq!(ns.watchable_types(), vec!["channel".to_string()]);
    }

    /// **P-ID-23 — `watchable()` is idempotent (a fragment that already names `watcher` is unchanged).**
    /// A subsystem fragment that declared `watcher` itself, or a double `.watchable()` call, does not
    /// duplicate the relation (the declared relation set is deduped, one `watcher` edge).
    #[test]
    fn watchable_is_idempotent() {
        let frag = FragmentDef {
            object_type: ObjectType("issue".into()),
            relations: vec![RelName(WATCHER_RELATION.into()), RelName("assignee".into())],
            permissions: vec![],
        }
        .watchable()
        .watchable();
        let watcher_count = frag.relations.iter().filter(|r| r.0 == WATCHER_RELATION).count();
        assert_eq!(watcher_count, 1, "watcher is declared exactly once (idempotent)");
        assert!(frag.is_watchable());
    }

    /// **P-ID-23 — `declare_watchable` attaches `watcher` to an already-admitted type, and rejects an
    /// unknown one (Id never invents a type for a watcher relation).** The core `project` type is made
    /// watchable in place; an undeclared type is rejected loudly.
    #[test]
    fn declare_watchable_attaches_to_admitted_type_rejects_unknown() {
        let mut ns = NamespaceEngine::with_core_hierarchy();
        // project is admitted (core hierarchy) but not yet watchable.
        assert!(!ns.is_watchable("project"));
        let admit = ns.declare_watchable("project");
        assert!(matches!(admit, FragmentAdmit::Admitted { fragment_id } if fragment_id == "project"));
        assert!(ns.is_watchable("project"), "project is now watchable");
        // Idempotent: re-declaring is a no-op success (not a duplicate-relation error).
        assert!(matches!(ns.declare_watchable("project"), FragmentAdmit::Admitted { .. }));
        // An unknown type cannot be made watchable (Id never invents a type for the relation).
        match ns.declare_watchable("nonexistent_type") {
            FragmentAdmit::Rejected { reason } => {
                assert!(reason.contains("not an admitted object type"), "rejection names why: {reason}")
            }
            FragmentAdmit::Admitted { .. } => panic!("an unknown type must not be made watchable"),
        }
    }

    /// **A too-deep rewrite is rejected at admit (the schema bound).**
    #[test]
    fn a_too_deep_rewrite_is_rejected() {
        let mut ns = NamespaceEngine::new();
        // Build a Union nested deeper than MAX_RULE_DEPTH.
        let mut rw = Userset::Relation(RelName("reader".into()));
        for _ in 0..(MAX_RULE_DEPTH + 2) {
            rw = Userset::Union(vec![rw]);
        }
        let frag = FragmentDef {
            object_type: ObjectType("deep".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![PermissionRule {
                permission: Permission("read".into()),
                rewrite: rw,
            }],
        };
        assert!(
            matches!(ns.admit(&frag), FragmentAdmit::Rejected { .. }),
            "a rewrite nested past the admit bound is rejected (bounded schema)"
        );
    }
}
