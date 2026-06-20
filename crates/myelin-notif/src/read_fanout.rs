//! # Read-fanout for the unbounded ambient set (NOTIF-P13 / P-191, M2) — the `SetExpr` watcher
//! push-down JOIN + the zookie watermark
//!
//! **Owning architecture doc:** `notifications.md` §3.5 ("Write-fanout for mentions vs read-fanout
//! for bodies — the frozen `list_objects` push-down (C1)"): **fan-out on READ** for the large
//! UNBOUNDED ambient set (every watcher of a hot PR, every member of a 50k-person channel) —
//! **store ONE coalesced marker, materialise per-watcher LAZILY on inbox open.** A celebrity subject
//! with 50k watchers costs **zero write amplification** (the §3.5 Twitter timeline read-fanout). The
//! watcher resolution is the **frozen `list_objects(recipient, watch, type) → Filter{set_expr,
//! zookie}`** (contract 4.3) lowered into a **SQL JOIN against the `authz_visible` reverse index**
//! over Notif's own `inbox_item.subject_root`/`subject` column — **one query, no N+1, no
//! post-filter** (the `search-requires-acl-filter` discipline generalised to the inbox read, §3.5).
//! The **zookie watermark** (contract 4.10): a security-sensitive read passes the zookie so a
//! just-revoked `watch` grant is reflected (the JOIN reads the reverse index at-or-after the zookie's
//! revision watermark); an item is **held, not leaked** if a check can't resolve fresh (§5.3).
//! **External insight:** `04-hard-problems.md` §5.3 (Notif is a projection; held, not leaked),
//! `01-process-and-quality-doctrine.md` §3 (prove-it — the 50k-watcher amplification check), §1
//! (name-your-floors — the synthetic-watcher floor). **Drill:** NOTIF-D2 (the read-fanout
//! amplification leg: a 50k-watcher subject → 0 per-watcher write rows; one JOIN on inbox open).
//!
//! ## Write-fanout (NOTIF-P12) vs read-fanout (this prompt) — the §3.5 hybrid, BOTH legs
//!
//! [`crate::write_fanout`] (NOTIF-P12) materialises one `inbox_item` per recipient for the **bounded
//! DIRECT set** (mentioned/assigned/reviewer/escalation), bounded even there by the hot-subject cap.
//! This module is the **other leg**: the **UNBOUNDED AMBIENT set** (watchers / 50k-channel members)
//! is NOT exploded into writes — it is **fanned out on READ**:
//!
//! 1. **One coalesced marker per `(tenant, subject_root)`** ([`ReadFanoutMarker`] in
//!    [`AmbientMarkerStore`]): an ambient event on a watched subject stores ONE marker keyed by the
//!    subject_root — NOT one row per watcher. A 50k-watcher subject costs ONE marker write, never
//!    50k. **Zero write amplification** (the load-bearing §3.5 scale answer).
//! 2. **Materialise the viewer's slice LAZILY on inbox open** ([`read_fanout`]): when a watcher opens
//!    their inbox, resolve the subject_roots they WATCH via the frozen `list_objects(recipient,
//!    watch, type) → Filter{set_expr, zookie}` push-down ([`WatcherResolvePort`]), lower the
//!    `SetExpr` (`InRelation{relation: watcher, via_column}` / `TupleSet`) into a **SQL JOIN** against
//!    the per-tenant `authz_visible` reverse index over Notif's OWN `subject_root` column, and
//!    project the marker set down to the viewer's reachable subject_roots — **ONE query, no N+1, no
//!    post-filter**. The result is the ambient items THIS viewer sees, materialised on read.
//!
//! ## The zookie watermark (contract 4.10, §3.5 / §5.3) — held, not leaked
//!
//! A security-sensitive read carries the `zookie` so it does NOT serve from the fail-static cache.
//! The reverse-index JOIN reads at a revision **≥** the watermark derived from the `list_objects`
//! answer's zookie ([`RevisionWatermark`]) — so a just-revoked `watch` grant (a newer zookie from
//! `write_tuples`) IS reflected, and a reverse-index revision OLDER than the watermark is REJECTED
//! loudly (a stale revision could re-admit a just-revoked watcher — the new-enemy problem). When the
//! resolver cannot answer fresh (unavailable / a revision below the watermark), the ambient item is
//! **HELD, not leaked** (§5.3, ADR-03 deny-when-unsure): the read-fanout returns the bounded set it
//! could prove, never a stale grant.
//!
//! ## FLOOR named (per EI-01 §1) — the synthetic-watcher floor
//!
//! The read-fanout depends on every WATCHABLE subsystem declaring its `watcher` ReBAC fragment
//! (contract 4.9, C8) — those fragments land WITH their subsystems: **Git/Knowledge in NOTIF-P19/P20,
//! Issues/Chat in NOTIF-P21/P22** (the master ledger's M3/M4 real-watcher prompts). Until then, the
//! read-fanout is drilled against **SYNTHETIC watcher tuples** ([`SyntheticReverseIndex`]) — a
//! deterministic stand-in for the real `authz_visible` reverse index, exercising the SAME JOIN
//! lowering + the SAME zookie watermark the production resolver will. Named so the synthetic fixture
//! is never mistaken for the live watcher graph. The live OLTP backing of the marker store + the
//! production `list_objects` reverse-index resolver wire in when the OLTP/Identity clients land in
//! `serve` (P-007 / P-S12); the DECISION shape (one marker, one JOIN, the watermark, held-not-leaked)
//! does not change.
//!
//! ## Mutation floor (the read-fanout decision module — mandatory-core)
//! Read-fanout is mandatory-core (a wrong verdict either write-amplifies a celebrity subject or LEAKS
//! a just-revoked watcher's ambient items). **Floor: ≥ 80% line/branch mutation score on
//! `read_fanout.rs`** (measured with `cargo mutants`; reported in the P-191 commit body). The
//! mutation-tested core is [`AmbientMarkerStore::record`] (ONE marker per subject_root — never N
//! rows), the [`SetExpr`] lowering in [`read_fanout`] (the `InRelation`/`TupleSet` JOIN, the
//! `Union`/`Intersect`/`Difference` boolean composition, `Ids`/`NotIds`/`All`/`None`), and the
//! **watermark gate** ([`ReverseIndexAnswer::honours`]) — a mutant that drops the cap-to-one marker,
//! widens a `None`/`NotIds` to admit, skips the watermark check (serving a stale revision), or
//! resolves a denied watcher is caught by the unit + chained tests below.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use myelin_events::ArtifactRef;
use myelin_identity::{
    AuthzError, ColRef, Consistency, ConsistencyMode, ObjectType, Permission, Principal, RelName,
    Result as AuthzResult, SetExpr,
};
use myelin_tenancy::TenantId;

use crate::storm_control::subject_root_of;

/// **The frozen `watch` permission Notif lists ambient subjects for** (the read-fanout half of
/// contract 4.3/4.9). The viewer is shown the ambient items of the subjects they hold the `watch`
/// permission over (the `watcher` relation, §3.5). Notif never WIDENS this — it lists for `watch`,
/// nothing else (the search-requires-acl-filter discipline: one bounded permission, never `*`).
pub const WATCH_PERMISSION: &str = "watch";

/// **The frozen `watcher` relation** (contract 4.9 — "`watcher` relation per watchable type", C8).
/// The relation every watchable subsystem declares in its ReBAC namespace fragment; the read-fanout
/// `SetExpr::InRelation{relation: watcher, ..}` JOIN resolves it. Notif does NOT invent it — it reads
/// the frozen relation name (§3.5: "Notif does not invent it; it reads it").
pub const WATCHER_RELATION: &str = "watcher";

/// **The object-type discriminant the read-fanout lists over** — Notif's own `subject_root` id
/// space (the §3.5 "Notif's own `inbox_item.subject_root`/`subject` column"). The `list_objects`
/// pre-filter is scoped to this type; the JOIN is keyed by the `via_column` the `SetExpr` names.
pub const SUBJECT_ROOT_TYPE: &str = "subject_root";

/// Notif's OWN id column the relational `SetExpr` JOIN lowers over (the §7.1 [`ColRef`] / the §3.5
/// "JOIN … over Notif's own `inbox_item.subject_root` column"). The watcher push-down is a JOIN keyed
/// by THIS column, never an N+1 per-subject `check`.
pub fn subject_root_col() -> ColRef {
    ColRef { table: "notif_inbox_item".into(), column: "subject_root".into() }
}

/// **ONE coalesced ambient marker for a watched `subject_root` (the §3.5 read-fanout store).** An
/// ambient event on a watched subject stores ONE of these keyed by `(tenant, subject_root)` — NOT
/// one row per watcher. The "+N more happened on this subject" counter ([`ReadFanoutMarker::count`])
/// accumulates ambient activity into the single marker, so a celebrity subject with 50k watchers
/// costs ONE marker write, never 50k inbox rows (**zero write amplification**, the load-bearing
/// scale answer). References-not-payloads (NOTIF-1): `subject` is an [`ArtifactRef`], never a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadFanoutMarker {
    /// The tenant the marker is partitioned under (the partition-key half; the region rides the
    /// subject ref / the router's verified envelope — the model carries the tenant scope here).
    pub tenant: TenantId,
    /// The `#sub`-stripped subject root (the coalescing key, §3.2.3) — all ambient activity on the
    /// same thread/PR shares ONE marker. A ref, never a payload.
    pub subject_root: String,
    /// The artifact the ambient activity is about (a ref, never a payload). Resolved per-viewer at
    /// humanise time (NOTIF-P9); a denied/erased ref renders as a tombstone, never leaked.
    pub subject: ArtifactRef,
    /// The structured why-it-fired for the ambient stream ([`crate::Reason::Watched`] — the
    /// read-fanout watcher reason, §3.1). The C-9 scoped-view filter pins on this.
    pub reason: crate::Reason,
    /// The "+N more happened on this subject" counter — accumulated ambient activity coalesced into
    /// the ONE marker (NOT one row per event, NOT one row per watcher). The §3.5 "store ONE coalesced
    /// marker": the count is preserved, the write amplification is bounded to ONE.
    pub count: u64,
    /// The latest originating event ref (the NOTIF-2 provenance for the coalesced marker). A ref.
    pub latest_origin: ArtifactRef,
}

/// **The ambient marker store (the §3.5 read-fanout write side) — ONE marker per `(tenant,
/// subject_root)`.** The model of the `notif_ambient_marker` row (the durable backing wires in with
/// the OLTP client, P-007 / P-S12). An ambient event UPSERTs the ONE marker for its subject_root
/// ([`AmbientMarkerStore::record`]); `count += 1` on a repeat — it NEVER opens a per-watcher row, so
/// the write amplification of a celebrity subject is ONE, regardless of watcher count. A cloneable
/// handle so the router pool + the inbox-open read share one truth.
#[derive(Clone, Default)]
pub struct AmbientMarkerStore {
    inner: Arc<Mutex<HashMap<(String, String), ReadFanoutMarker>>>,
}

impl AmbientMarkerStore {
    /// A fresh, empty ambient marker store.
    pub fn new() -> AmbientMarkerStore {
        AmbientMarkerStore::default()
    }

    /// **Record an ambient event on a watched subject — UPSERT the ONE marker for its `subject_root`
    /// (§3.5, zero write amplification).** The subject's `#sub` fragment is stripped to the root
    /// ([`subject_root_of`]) so all activity on the same thread/PR coalesces into ONE marker. A FRESH
    /// root inserts the marker (`count = 1`); a REPEAT root bumps `count += 1` and updates the latest
    /// origin — it NEVER opens a second marker and NEVER a per-watcher row. So N ambient events on a
    /// subject watched by M watchers cost exactly ONE marker, never `N` and never `M` rows — the
    /// load-bearing read-fanout property. This is the read-side analogue of the write-fanout's UPSERT
    /// collapse; here the collapse is to ONE row for the WHOLE watcher set.
    pub fn record(
        &self,
        tenant: &TenantId,
        subject: &ArtifactRef,
        reason: crate::Reason,
        origin: &ArtifactRef,
    ) {
        let root = subject_root_of(&subject.0);
        let key = (tenant.0.clone(), root.clone());
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match g.get_mut(&key) {
            Some(existing) => {
                // A REPEAT on the same root: bump the +N counter + refresh the latest origin. NO new
                // marker, NO per-watcher row — the write amplification stays ONE.
                existing.count += 1;
                existing.latest_origin = origin.clone();
            }
            None => {
                g.insert(
                    key,
                    ReadFanoutMarker {
                        tenant: tenant.clone(),
                        subject_root: root,
                        subject: subject.clone(),
                        reason,
                        count: 1,
                        latest_origin: origin.clone(),
                    },
                );
            }
        }
    }

    /// The number of DISTINCT ambient markers under `tenant` (one per watched subject_root). A drill
    /// asserts a 50k-watcher subject hit N times produces exactly ONE marker (not N, not 50k).
    pub fn marker_count(&self, tenant: &TenantId) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter(|(t, _)| t == &tenant.0)
            .count()
    }

    /// Read one marker by `(tenant, subject_root)` (for a test / a drill).
    pub fn get(&self, tenant: &TenantId, subject_root: &str) -> Option<ReadFanoutMarker> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(tenant.0.clone(), subject_root.to_string()))
            .cloned()
    }

    /// **A snapshot of all ambient markers under one tenant** (the inbox-open read scans this, the
    /// model of `SELECT … WHERE tenant_id = $1`). The read-fanout JOINs the viewer's reachable
    /// subject_roots against THIS set — it never reads another tenant's markers (the partition key).
    fn snapshot_for_tenant(&self, tenant: &TenantId) -> Vec<ReadFanoutMarker> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|m| m.tenant == *tenant)
            .cloned()
            .collect()
    }
}

/// **The authz reverse-index revision watermark (contract 4.10, §3.5).** A monotone revision the
/// reverse-index JOIN honours: the JOIN must read at a revision **≥** the watermark derived from the
/// `list_objects` answer's zookie, so it NEVER composes a reverse-index revision OLDER than the ACL
/// snapshot — a stale revision could re-admit a just-revoked watch grant (the new-enemy problem). A
/// resolver returning a revision below the watermark is rejected (the item is held, not leaked).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RevisionWatermark(pub u64);

/// **The reverse-index JOIN answer (the watcher push-down result, §3.5).** The set of `subject_root`s
/// the subject reaches via the relational `SetExpr` leaf + the **revision** the reverse index served
/// it at (contract 4.10). The pipeline checks `revision >= required` ([`ReverseIndexAnswer::honours`])
/// BEFORE composing the set into the visible filter — a revision below the watermark is rejected
/// (never read stale → held, not leaked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseIndexAnswer {
    /// The `subject_root`s the subject WATCHES (the `LookupResources` reverse index as a conjoinable
    /// membership set — the JOIN result, not an N+1 per-subject probe).
    pub subject_roots: BTreeSet<String>,
    /// The reverse-index revision this answer was served at (contract 4.10). The watermark check
    /// asserts `revision >= required`.
    pub revision: RevisionWatermark,
}

impl ReverseIndexAnswer {
    /// **Does this answer honour the watermark (contract 4.10)?** `true` iff the revision the reverse
    /// index served at is **≥** the `required` watermark — i.e. the JOIN did NOT read a revision older
    /// than the ACL snapshot. A `false` here means the resolver answered from a stale revision (which
    /// could re-admit a just-revoked watch): the read-fanout must HOLD, not leak (§5.3). This is the
    /// load-bearing watermark gate the mutation floor pins.
    pub fn honours(&self, required: RevisionWatermark) -> bool {
        self.revision >= required
    }
}

/// **A relational `SetExpr` leaf the reverse-index JOIN resolves (§3.5, the OQ-E forms).** The two
/// relational forms of the frozen algebra: `InRelation{relation: watcher, via_column}` (subject_roots
/// where the viewer is a `watcher`, JOINed by Notif's own `subject_root` column) and `TupleSet{index}`
/// (a server-materialised tuple set to JOIN against — the big-result path). Lifted out of
/// [`myelin_identity::SetExpr`] so the resolver port takes JUST the relational leaf (the boolean
/// composition is resolved by [`read_fanout`], not the port — mirroring the search pipeline).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelationalLeaf {
    /// `InRelation{relation, via_column}` — subject_roots where the viewer is the subject of
    /// `relation` (the `watcher` relation, §3.5), JOINed by Notif's own `via_column`.
    InRelation { relation: RelName, via_column: ColRef },
    /// `TupleSet{index}` — a server-materialised `(subject, watcher, subject_root)` tuple set to
    /// semijoin against (the big-result path, §7.1).
    TupleSet { index: myelin_identity::AuthzIndexRef },
}

/// **The narrow Identity port the read-fanout consumes** — the `list_objects` slice of contract 4.3
/// (Notif is one of the five named `SetExpr` consumers; NO Id signature change) + the relational
/// reverse-index `resolve` (the watcher push-down). A seam (not a dependency on the whole
/// eleven-method [`myelin_identity::IdentityService`]) so the read-fanout is testable with a
/// deterministic authz fake AND so the no-N+1 invariant is observable: [`read_fanout`] calls
/// [`list_objects`](WatcherResolvePort::list_objects) **exactly once** per inbox open, and
/// [`resolve_relation`](WatcherResolvePort::resolve_relation) **once per relational leaf** (no
/// per-subject probe). The production wiring binds this to `IdentityService::list_objects` through
/// the resilient client; the read-fanout only needs the `Ids|Filter` answer + the JOIN resolution.
pub trait WatcherResolvePort {
    /// **List the subject_roots the `subject` may `watch`, at consistency `at` (contract 4.3).**
    /// Returns the leak-free pre-filter: `Ids{ids, zookie}` (materialised, the S4 path — a viewer who
    /// watches a small bounded set) or `Filter{set_expr, zookie}` (pushed down, the S8 path — the
    /// 50k-density watcher graph). The ONLY authz call the read-fanout makes per inbox open.
    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> AuthzResult<myelin_identity::ListObjectsResult>;

    /// **Resolve a relational `SetExpr` leaf to the co-located watched-subject_root set (the §3.5
    /// reverse-index JOIN, contract 4.3 / 4.10).** When `list_objects` returns a `Filter` whose
    /// algebra contains the relational forms `InRelation{relation: watcher, via_column}` /
    /// `TupleSet{index}`, the read-fanout JOINs against the **per-tenant `authz_visible` reverse
    /// index** — Identity's materialised `(subject, watcher, subject_root)` projection, queried per
    /// cell, kept fresh off the bus. Resolves ONE such leaf for `subject` to the subject_roots they
    /// reach + the **revision** the reverse index served the answer at (the watermark — the JOIN never
    /// reads a revision staler than `required`). ONE resolve per relational leaf, no N+1.
    ///
    /// **Default = unavailable (deny-when-unsure, ADR-03, §5.3).** A port wired ONLY for the
    /// bounded-set path (a small `Ids` answer) has no reverse index; resolving a relational form
    /// against it is a loud `Unavailable`, never a silent widen — the ambient item is HELD, not
    /// leaked. The production wiring + the NOTIF-P13 tests provide a real resolver.
    fn resolve_relation(
        &self,
        _subject: &Principal,
        _leaf: &RelationalLeaf,
        _required: RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        Err(AuthzError::Unavailable(
            "the authz reverse index is not wired for this read-fanout path — a relational watcher \
             SetExpr leaf cannot be resolved (deny-when-unsure, ADR-03; the ambient item is HELD, \
             not leaked, §5.3)"
                .into(),
        ))
    }
}

/// Why the read-fanout could not materialise the viewer's ambient slice (held, not leaked). NEVER a
/// silent widen: an error here means the read-fanout returns ONLY the markers it could prove fresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadFanoutError {
    /// The authz resolver was unavailable (an Id hiccup / no reverse index) — the ambient items are
    /// HELD, not leaked (§5.3, ADR-03 deny-when-unsure). The bounded set the read-fanout COULD prove
    /// is still returned; the unprovable ambient set is withheld.
    Unavailable(String),
    /// The reverse index served a revision BELOW the zookie watermark (contract 4.10) — a stale
    /// revision could re-admit a just-revoked watch grant (the new-enemy problem). REJECTED loudly:
    /// the ambient slice is held, not leaked, rather than served from a stale revision.
    StaleReverseIndex { required: RevisionWatermark, served: RevisionWatermark },
}

/// **`read_fanout(viewer, markers, resolver, at)` — materialise the viewer's ambient slice LAZILY on
/// inbox open (the §3.5 read-fanout, contract 4.3/4.4/4.10).**
///
/// The crux of the unbounded-ambient-set scale answer. On inbox open, in order:
///
/// 1. **Resolve the viewer's watched subject_roots** via the frozen `list_objects(viewer, watch,
///    subject_root, at) → Ids | Filter{set_expr, zookie}` push-down — **ONE authz call** (no per-
///    subject N+1). The returned zookie's revision is the **watermark** the JOIN must honour.
/// 2. **Lower the `SetExpr` into the reverse-index JOIN over Notif's OWN `subject_root` column** —
///    `Ids`/`NotIds`/`All`/`None` resolve in-process; `InRelation{watcher}`/`TupleSet` resolve via
///    [`WatcherResolvePort::resolve_relation`] (the per-tenant `authz_visible` reverse index);
///    `Union`/`Intersect`/`Difference` compose the leaves (the boolean algebra). The result is the
///    SET of subject_roots the viewer reaches — computed by JOIN, never by post-filtering N markers.
/// 3. **Project the ambient markers down to that set** — `markers ⋈ reachable_roots`: ONE pass over
///    the tenant's markers selecting those whose `subject_root` is in the viewer's reachable set. A
///    50k-watcher celebrity subject is ONE marker; this viewer either reaches it (one row) or does
///    not — no per-watcher materialisation ever happened.
///
/// The **zookie watermark** is honoured at every relational leaf: a reverse-index revision below the
/// watermark is [`ReadFanoutError::StaleReverseIndex`] (held, not leaked, §5.3); an unavailable
/// resolver is [`ReadFanoutError::Unavailable`] (the proven bounded set is returned, the unprovable
/// ambient set withheld). A just-revoked `watch` (a newer zookie) is reflected because the JOIN reads
/// at-or-after the watermark — the revoked subject_root is absent from the reachable set, so its
/// marker is NOT in the viewer's slice (the held-not-leaked property the drill proves).
///
/// Returns the ambient [`ReadFanoutMarker`]s THIS viewer sees (the lazily-materialised slice), in a
/// stable `subject_root` order. The caller folds these into [`crate::list_inbox`] (the ambient stream
/// of the ONE inbox); the direct/mentioned rows come from the write-fanout projection.
pub fn read_fanout(
    viewer: &Principal,
    markers: &AmbientMarkerStore,
    resolver: &dyn WatcherResolvePort,
    at: &Consistency,
) -> std::result::Result<Vec<ReadFanoutMarker>, ReadFanoutError> {
    // (1) ONE authz call: list the subject_roots the viewer may `watch` (no per-subject N+1).
    let permission = Permission(WATCH_PERMISSION.into());
    let ty = ObjectType(SUBJECT_ROOT_TYPE.into());
    let result = resolver
        .list_objects(viewer, &permission, &ty, at)
        .map_err(|e| ReadFanoutError::Unavailable(format!("{e:?}")))?;

    // The zookie's revision is the watermark the relational JOIN must honour (contract 4.10). For a
    // security-sensitive read (Strong) a just-revoked watch MUST be reflected; we derive the required
    // revision from the answer's zookie (the production zookie is opaque; here the synthetic index
    // parses the monotone revision from it — see SyntheticReverseIndex). BoundedStale reads may serve
    // from a lower watermark (the fail-static path); Strong reads pin the watermark exactly.
    let (set_expr, zookie) = match result {
        // The S4 materialised path: a bounded watched set, already the leak-free id list. No JOIN.
        myelin_identity::ListObjectsResult::Ids { ids, .. } => {
            let reachable: BTreeSet<String> = ids.into_iter().map(|o| o.0).collect();
            return Ok(project_with(
                markers,
                &viewer.tenant,
                &Reachable::Some(reachable),
            ));
        }
        // The S8 pushed-down path: lower the SetExpr into the reverse-index JOIN.
        myelin_identity::ListObjectsResult::Filter { set_expr, zookie } => (set_expr, zookie),
    };

    let required = watermark_for(&zookie, at);
    // (2) Lower the SetExpr into the reachable subject_root set (the JOIN — no post-filter, no N+1).
    let reachable = lower(viewer, &set_expr, resolver, required)?;
    // (3) Project the tenant's markers down to the reachable set (markers ⋈ reachable_roots).
    Ok(project_with(markers, &viewer.tenant, &reachable))
}

/// **Lower a `SetExpr` to the set of subject_roots the viewer reaches (the §3.5 JOIN lowering).** The
/// monotone set algebra (contract 4.3): the relational leaves resolve via the reverse-index JOIN
/// ([`WatcherResolvePort::resolve_relation`], one resolve per leaf, watermark-honoured); the boolean
/// forms compose. `All` is the unbounded-but-type-scoped set (every marker of this tenant — the
/// caller's tenant scope bounds it); `None` is the empty set (`WHERE false`). This is the leak-
/// critical surface the mutation floor pins: a mutant that widens `None`/`NotIds`/`Difference` or
/// drops the watermark check is caught.
fn lower(
    viewer: &Principal,
    expr: &SetExpr,
    resolver: &dyn WatcherResolvePort,
    required: RevisionWatermark,
) -> std::result::Result<Reachable, ReadFanoutError> {
    match expr {
        // Every subject_root of this tenant (the type-and-tenant scope bounds it — the caller's
        // `project` only ever scans ONE tenant's markers).
        SetExpr::All => Ok(Reachable::All),
        // Deny — the empty set (`WHERE false`).
        SetExpr::None => Ok(Reachable::Some(BTreeSet::new())),
        // An explicit allow-set, inlined (`WHERE subject_root IN (...)`).
        SetExpr::Ids(ids) => Ok(Reachable::Some(ids.iter().map(|o| o.0.clone()).collect())),
        // An explicit deny-set over the otherwise-visible space (`subject_root NOT IN (...)`).
        SetExpr::NotIds(ids) => {
            Ok(Reachable::AllExcept(ids.iter().map(|o| o.0.clone()).collect()))
        }
        // The relational watcher JOIN: resolve via the reverse index (the watermark-honoured leaf).
        SetExpr::InRelation { relation, via_column } => {
            let leaf = RelationalLeaf::InRelation {
                relation: relation.clone(),
                via_column: via_column.clone(),
            };
            resolve_leaf(viewer, &leaf, resolver, required)
        }
        SetExpr::TupleSet { index } => {
            let leaf = RelationalLeaf::TupleSet { index: index.clone() };
            resolve_leaf(viewer, &leaf, resolver, required)
        }
        // Boolean composition (the §7.2 AND/OR/EXCEPT).
        SetExpr::Union(parts) => {
            let mut acc = Reachable::Some(BTreeSet::new());
            for p in parts {
                acc = acc.union(lower(viewer, p, resolver, required)?);
            }
            Ok(acc)
        }
        SetExpr::Intersect(parts) => {
            // The intersection identity is "all"; narrow with each part.
            let mut acc = Reachable::All;
            for p in parts {
                acc = acc.intersect(lower(viewer, p, resolver, required)?);
            }
            Ok(acc)
        }
        SetExpr::Difference(a, b) => {
            let left = lower(viewer, a, resolver, required)?;
            let right = lower(viewer, b, resolver, required)?;
            Ok(left.difference(right))
        }
    }
}

/// Resolve a relational leaf via the reverse-index JOIN, honouring the watermark (contract 4.10). An
/// unavailable resolver → held, not leaked; a revision below the watermark → rejected (never stale).
fn resolve_leaf(
    viewer: &Principal,
    leaf: &RelationalLeaf,
    resolver: &dyn WatcherResolvePort,
    required: RevisionWatermark,
) -> std::result::Result<Reachable, ReadFanoutError> {
    let answer = resolver
        .resolve_relation(viewer, leaf, required)
        .map_err(|e| ReadFanoutError::Unavailable(format!("{e:?}")))?;
    // THE WATERMARK GATE (contract 4.10): never read a revision below the ACL snapshot. A stale
    // revision could re-admit a just-revoked watch — REJECT it (held, not leaked), do NOT serve.
    if !answer.honours(required) {
        return Err(ReadFanoutError::StaleReverseIndex {
            required,
            served: answer.revision,
        });
    }
    Ok(Reachable::Some(answer.subject_roots))
}

/// **The set of subject_roots a viewer reaches** — a small set algebra that defers materialising the
/// universe. `All` (every marker of the tenant), `AllExcept(deny)` (every marker except a deny-set —
/// the `NotIds` form), or an explicit `Some(set)`. The boolean ops compose these without ever
/// enumerating the (unbounded) universe — `project` evaluates membership against the tenant's actual
/// marker set at the end (one pass, no post-filter beyond the membership test the JOIN defines).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Reachable {
    /// Every subject_root of the tenant (type-and-tenant scoped).
    All,
    /// Every subject_root EXCEPT the deny-set (the `NotIds` form).
    AllExcept(BTreeSet<String>),
    /// An explicit reachable set (the `Ids` / resolved-relation form).
    Some(BTreeSet<String>),
}

impl Reachable {
    /// Is `root` reachable under this set?
    fn contains(&self, root: &str) -> bool {
        match self {
            Reachable::All => true,
            Reachable::AllExcept(deny) => !deny.contains(root),
            Reachable::Some(set) => set.contains(root),
        }
    }

    /// Set union (`OR`).
    fn union(self, other: Reachable) -> Reachable {
        match (self, other) {
            (Reachable::All, _) | (_, Reachable::All) => Reachable::All,
            // (all-except A) ∪ (all-except B) = all-except (A ∩ B).
            (Reachable::AllExcept(a), Reachable::AllExcept(b)) => {
                Reachable::AllExcept(a.intersection(&b).cloned().collect())
            }
            // (all-except A) ∪ S = all-except (A \ S).
            (Reachable::AllExcept(a), Reachable::Some(s))
            | (Reachable::Some(s), Reachable::AllExcept(a)) => {
                Reachable::AllExcept(a.difference(&s).cloned().collect())
            }
            (Reachable::Some(a), Reachable::Some(b)) => {
                Reachable::Some(a.union(&b).cloned().collect())
            }
        }
    }

    /// Set intersection (`AND`).
    fn intersect(self, other: Reachable) -> Reachable {
        match (self, other) {
            (Reachable::All, x) | (x, Reachable::All) => x,
            // (all-except A) ∩ (all-except B) = all-except (A ∪ B).
            (Reachable::AllExcept(a), Reachable::AllExcept(b)) => {
                Reachable::AllExcept(a.union(&b).cloned().collect())
            }
            // (all-except A) ∩ S = S \ A.
            (Reachable::AllExcept(a), Reachable::Some(s))
            | (Reachable::Some(s), Reachable::AllExcept(a)) => {
                Reachable::Some(s.difference(&a).cloned().collect())
            }
            (Reachable::Some(a), Reachable::Some(b)) => {
                Reachable::Some(a.intersection(&b).cloned().collect())
            }
        }
    }

    /// Set difference (`EXCEPT`) — `self \ other`.
    fn difference(self, other: Reachable) -> Reachable {
        match (self, other) {
            // self \ all = ∅.
            (_, Reachable::All) => Reachable::Some(BTreeSet::new()),
            // all \ S = all-except S.
            (Reachable::All, Reachable::Some(s)) => Reachable::AllExcept(s),
            // all \ (all-except B) = B (the things B excluded are exactly what remains).
            (Reachable::All, Reachable::AllExcept(b)) => Reachable::Some(b),
            // (all-except A) \ S = all-except (A ∪ S).
            (Reachable::AllExcept(a), Reachable::Some(s)) => {
                Reachable::AllExcept(a.union(&s).cloned().collect())
            }
            // (all-except A) \ (all-except B) = B \ A.
            (Reachable::AllExcept(a), Reachable::AllExcept(b)) => {
                Reachable::Some(b.difference(&a).cloned().collect())
            }
            // S \ T.
            (Reachable::Some(s), Reachable::Some(t)) => {
                Reachable::Some(s.difference(&t).cloned().collect())
            }
            (Reachable::Some(s), Reachable::AllExcept(b)) => {
                // S \ (all-except B) = S ∩ B.
                Reachable::Some(s.intersection(&b).cloned().collect())
            }
        }
    }
}

/// **Project the tenant's ambient markers down to the viewer's reachable subject_root set (the
/// `markers ⋈ reachable_roots` JOIN result).** ONE pass over the tenant's markers selecting those
/// whose `subject_root` is reachable (the [`Reachable`] set algebra is the JOIN's `ON` clause) — the
/// read-side of the §3.5 read-fanout: the celebrity subject's ONE marker either is in the viewer's
/// slice or is not, with NO per-watcher materialisation. Returns in a stable `subject_root` order
/// (deterministic paging downstream).
fn project_with(
    markers: &AmbientMarkerStore,
    tenant: &TenantId,
    reachable: &Reachable,
) -> Vec<ReadFanoutMarker> {
    let mut out: Vec<ReadFanoutMarker> = markers
        .snapshot_for_tenant(tenant)
        .into_iter()
        .filter(|m| reachable.contains(&m.subject_root))
        .collect();
    out.sort_by(|a, b| a.subject_root.cmp(&b.subject_root));
    out
}

/// **Derive the required reverse-index revision watermark from a `list_objects` zookie + the read's
/// consistency mode (contract 4.10, §3.5).** A `Strong` read pins the watermark to the zookie's
/// revision EXACTLY (a just-revoked watch — a newer zookie — must be reflected; the JOIN must read
/// at-or-after it). A `BoundedStale` read may serve from a lower watermark (the fail-static path, §10)
/// — modelled as watermark 0 (the JOIN may read any non-stale revision). The zookie's monotone
/// revision is parsed from the opaque token (the production zookie encodes it; the synthetic index
/// uses the `zk-<n>` form). A zookie that does not encode a revision is treated as revision 0 (the
/// most conservative — every revision satisfies it; the production resolver decodes the real
/// revision).
fn watermark_for(zookie: &myelin_identity::Zookie, at: &Consistency) -> RevisionWatermark {
    match at.mode {
        ConsistencyMode::Strong => RevisionWatermark(parse_revision(&zookie.0)),
        ConsistencyMode::BoundedStale => RevisionWatermark(0),
    }
}

/// Parse the monotone revision out of a zookie token of the form `zk-<n>` (the synthetic/dev form).
/// A token without that shape → 0 (conservative). The production zookie decoder is the Identity
/// client's; the read-fanout only needs the monotone ordering for the watermark comparison.
fn parse_revision(zookie: &str) -> u64 {
    zookie
        .strip_prefix("zk-")
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
}

/// **A deterministic SYNTHETIC reverse index (the named floor) — a stand-in for the live
/// `authz_visible` reverse index until the real watcher ReBAC fragments land (NOTIF-P19..P22).** It
/// holds, per `(tenant, subject)` the set of subject_roots the subject WATCHES + the current monotone
/// revision. It answers `list_objects` with the pushed-down `Filter{InRelation{watcher}}` (the S8
/// path the read-fanout exercises) and `resolve_relation` with the watched set at the current
/// revision — so the JOIN lowering + the watermark gate are drilled against the SAME shapes the
/// production resolver will serve. `revoke_watch` bumps the revision (a newer zookie), so a read at
/// the new watermark reflects the revocation (held, not leaked). NOT a security bypass — it is the
/// synthetic fixture the prompt names; the production resolver substitutes behind the SAME port.
#[derive(Clone, Default)]
pub struct SyntheticReverseIndex {
    inner: Arc<Mutex<SyntheticState>>,
}

#[derive(Default)]
struct SyntheticState {
    /// Per-`(tenant, subject_principal)`: the subject_roots that principal watches.
    watches: HashMap<(String, String), BTreeSet<String>>,
    /// The monotone revision (bumped on every write/revoke — the zookie watermark source).
    revision: u64,
    /// If set, the index reports as unavailable (an Id hiccup — held, not leaked is exercised).
    unavailable: bool,
    /// If set, the index serves answers at THIS revision regardless of the current one (the stale-
    /// revision drill: a resolver lagging behind the watermark → StaleReverseIndex, never served).
    served_revision_override: Option<u64>,
}

impl SyntheticReverseIndex {
    /// A fresh synthetic reverse index at revision 0.
    pub fn new() -> SyntheticReverseIndex {
        SyntheticReverseIndex::default()
    }

    /// **Grant `principal` a `watch` on `subject_root` (a `write_tuples` — bumps the revision).** The
    /// new revision is the watermark a subsequent strong read pins (a fresh grant reflected at-or-
    /// after it). Returns the new zookie (the `zk-<rev>` form the read passes back).
    pub fn grant_watch(
        &self,
        tenant: &TenantId,
        principal: &str,
        subject_root: &str,
    ) -> myelin_identity::Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        g.watches
            .entry((tenant.0.clone(), principal.to_string()))
            .or_default()
            .insert(subject_root.to_string());
        myelin_identity::Zookie(format!("zk-{}", g.revision))
    }

    /// **Revoke `principal`'s `watch` on `subject_root` (a `write_tuples` — bumps the revision).** A
    /// read at the NEW watermark reflects the revocation: the JOIN reads the reverse index at-or-after
    /// the new revision, so the revoked subject_root is absent from the reachable set (held, not
    /// leaked). Returns the new zookie (the watermark a strong read must honour).
    pub fn revoke_watch(
        &self,
        tenant: &TenantId,
        principal: &str,
        subject_root: &str,
    ) -> myelin_identity::Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        if let Some(set) = g.watches.get_mut(&(tenant.0.clone(), principal.to_string())) {
            set.remove(subject_root);
        }
        myelin_identity::Zookie(format!("zk-{}", g.revision))
    }

    /// The current monotone revision (the zookie watermark source) — `zk-<rev>`.
    pub fn current_zookie(&self) -> myelin_identity::Zookie {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        myelin_identity::Zookie(format!("zk-{}", g.revision))
    }

    /// Make the index report UNAVAILABLE (an Id hiccup) — the held-not-leaked drill.
    pub fn set_unavailable(&self, on: bool) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).unavailable = on;
    }

    /// Pin the index to serve answers at `revision` regardless of the current one (the STALE-revision
    /// drill — a resolver lagging behind the watermark must be REJECTED, never served).
    pub fn pin_served_revision(&self, revision: Option<u64>) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .served_revision_override = revision;
    }
}

impl WatcherResolvePort for SyntheticReverseIndex {
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<myelin_identity::ListObjectsResult> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.unavailable {
            return Err(AuthzError::Unavailable("synthetic reverse index unavailable".into()));
        }
        // The S8 PUSHED-DOWN path: the read-fanout always lowers the watcher relation via the JOIN
        // (the 50k-density path the prompt exercises) — return the Filter{InRelation{watcher}} the
        // production resolver returns, stamped with the current revision's zookie.
        Ok(myelin_identity::ListObjectsResult::Filter {
            set_expr: SetExpr::InRelation {
                relation: RelName(WATCHER_RELATION.into()),
                via_column: subject_root_col(),
            },
            zookie: myelin_identity::Zookie(format!("zk-{}", g.revision)),
        })
    }

    fn resolve_relation(
        &self,
        subject: &Principal,
        leaf: &RelationalLeaf,
        _required: RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.unavailable {
            return Err(AuthzError::Unavailable("synthetic reverse index unavailable".into()));
        }
        // Only the watcher relation is served by THIS synthetic index (a different relation → empty).
        let watched = match leaf {
            RelationalLeaf::InRelation { relation, .. } if relation.0 == WATCHER_RELATION => g
                .watches
                .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                .cloned()
                .unwrap_or_default(),
            // The TupleSet form also resolves to the same watched set (the big-result path).
            RelationalLeaf::TupleSet { .. } => g
                .watches
                .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                .cloned()
                .unwrap_or_default(),
            _ => BTreeSet::new(),
        };
        // The revision the index serves the answer at — the current one, UNLESS a stale revision is
        // pinned (the stale-revision drill). The watermark gate in `resolve_leaf` rejects a served
        // revision below the required watermark (never read stale).
        let served = g.served_revision_override.unwrap_or(g.revision);
        Ok(ReverseIndexAnswer {
            subject_roots: watched,
            revision: RevisionWatermark(served),
        })
    }
}

#[cfg(test)]
mod tests;
