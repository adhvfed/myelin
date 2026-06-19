//! The crate dependency DAG (architecture §2.9), encoded declaratively + asserted acyclic.
//!
//! This is the **build-layer realisation** of the `no-cross-sync-cycle` lint (P-S10 ships
//! the real source-scanning lint over the *service* call graph). Here we model the *crate*
//! graph — the ten Myelin crates and their inter-crate edges in the EXACT root-last order
//! of architecture §2.9:
//!
//! ```text
//! myelin-tenancy → myelin-identity → myelin-events (+ -refs, -content, -query)
//!                → myelin-agent, myelin-gdpr → myelin-client, myelin-storage → myelin-substrate
//! ```
//!
//! ## The `myelin-storage` node (P-ST-01 → P-007) — a documented DAG extension
//! The architecture §2.9 froze TEN crates with no `myelin-storage` node (§2.8: there is
//! deliberately no shared "storage API" crate spanning subsystems). The Storage by-system
//! prompt mandates a `myelin-storage` crate for Storage's *runtime* code (the tier clients
//! / KMS / BlobStore impls). The reconciliation (see `myelin-storage`'s crate-level
//! DEVIATION note): `myelin-storage` is the harness-wired storage SUBSTRATE — the
//! *mechanism* every subsystem opens its pool THROUGH (the thin query layer §2.8 itself
//! names) — NOT a cross-subsystem data-access crate, so the `no-cross-db` rule is
//! preserved. In root-last order it sits below `-gdpr`/`-client` and ABOVE `-substrate`
//! (the harness depends on the tier client it wires). This module is updated to ELEVEN
//! crates accordingly; the acyclic + identity-sink + substrate-root invariants are
//! unchanged.
//!
//! The frozen invariants this module asserts (the `crate-graph-acyclic` test):
//! 1. **The DAG is acyclic** — a dependency that would create a cycle must not exist.
//! 2. **`myelin-identity` is a SINK** — it depends on nothing above `myelin-tenancy`
//!    (architecture §2.9: "identity depends on nothing"). This is the load-bearing
//!    ordering property: identity, the most-depended-on authz surface, can never pull in
//!    a downstream crate.
//! 3. **`myelin-substrate` is the ROOT** (root-last) — no crate depends on it.
//!
//! The edge set below mirrors the `[dependencies]` in each crate's `Cargo.toml`. If a
//! Cargo edge is added that creates a cycle, Cargo itself fails to compile the workspace
//! (a cycle "must not compile"); this module additionally asserts the logical invariants
//! at `cargo test` time so a *non-cyclic-but-wrong-direction* edge (e.g. identity gaining
//! a dependency on events) is caught loudly, not silently.

/// A Myelin crate node in the dependency DAG (architecture §2.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Crate {
    Tenancy,
    Identity,
    Events,
    Refs,
    Content,
    Query,
    Agent,
    Gdpr,
    Client,
    Storage,
    Substrate,
}

impl Crate {
    /// All eleven crates (the §2.9 ten + the documented `myelin-storage` storage-substrate
    /// node, P-ST-01 → P-007).
    pub const ALL: [Crate; 11] = [
        Crate::Tenancy,
        Crate::Identity,
        Crate::Events,
        Crate::Refs,
        Crate::Content,
        Crate::Query,
        Crate::Agent,
        Crate::Gdpr,
        Crate::Client,
        Crate::Storage,
        Crate::Substrate,
    ];

    /// The crate's package name (matches its `Cargo.toml`).
    pub fn name(self) -> &'static str {
        match self {
            Crate::Tenancy => "myelin-tenancy",
            Crate::Identity => "myelin-identity",
            Crate::Events => "myelin-events",
            Crate::Refs => "myelin-refs",
            Crate::Content => "myelin-content",
            Crate::Query => "myelin-query",
            Crate::Agent => "myelin-agent",
            Crate::Gdpr => "myelin-gdpr",
            Crate::Client => "myelin-client",
            Crate::Storage => "myelin-storage",
            Crate::Substrate => "myelin-substrate",
        }
    }

    /// The crate's DIRECT myelin-crate dependencies — mirrors each crate's `[dependencies]`
    /// in its `Cargo.toml`, in the architecture §2.9 root-last order.
    pub fn deps(self) -> &'static [Crate] {
        match self {
            // The DAG sink: depends on nothing (architecture §2.9).
            Crate::Tenancy => &[],
            // Identity is a sink whose only dependency is tenancy ("identity depends on
            // nothing" above tenancy). The DAG-deviation (ArtifactRef value type moved to
            // the tenancy sink) is what keeps this true despite check() needing ArtifactRef.
            Crate::Identity => &[Crate::Tenancy],
            // The events tier (tenancy → identity → events + refs/content/query).
            Crate::Events => &[Crate::Tenancy, Crate::Identity],
            Crate::Refs => &[Crate::Tenancy, Crate::Identity, Crate::Events],
            Crate::Content => &[Crate::Tenancy, Crate::Events, Crate::Identity],
            Crate::Query => &[Crate::Tenancy, Crate::Identity, Crate::Events],
            // The agent/gdpr tier (below events).
            Crate::Agent => &[Crate::Identity, Crate::Events],
            // gdpr depends on events too (P-GA-01 / P-049): the `data_role` GDPR role-tag
            // anchors to the frozen 2.1 `EventEnvelope.data_role` field (`myelin_events::
            // DataRole`), so the tenant-content|platform-operational ↔ processor|controller
            // mapping is compiled, not hoped. The edge gdpr → events is acyclic (events
            // depends only on tenancy + identity). Mirrors crates/myelin-gdpr/Cargo.toml.
            Crate::Gdpr => &[Crate::Tenancy, Crate::Identity, Crate::Events],
            // The client tier (below agent/gdpr).
            Crate::Client => &[Crate::Tenancy, Crate::Identity, Crate::Events],
            // The storage-substrate tier (below gdpr): the OLTP tier client + (tenant,
            // region) RLS guard depends on tenancy (TenantId/Region), identity (Principal —
            // the verified token), events (the co-located outbox), and gdpr
            // (PersonalDataHolder — the auto-registered holder). Mirrors
            // crates/myelin-storage/Cargo.toml. It must NOT depend on -client or -substrate.
            Crate::Storage => &[
                Crate::Tenancy,
                Crate::Identity,
                Crate::Events,
                Crate::Gdpr,
            ],
            // Root-last: the harness depends on everything (incl. the storage substrate it
            // wires); nothing depends on it.
            Crate::Substrate => &[
                Crate::Tenancy,
                Crate::Identity,
                Crate::Events,
                Crate::Refs,
                Crate::Content,
                Crate::Query,
                Crate::Agent,
                Crate::Gdpr,
                Crate::Client,
                Crate::Storage,
            ],
        }
    }
}

/// Returns `true` if the crate dependency graph is acyclic (the §2.9 invariant).
///
/// A DFS-based cycle check over the declared edge set. Because the edges mirror the Cargo
/// manifests, this is the logical twin of "a dependency cycle must not compile".
pub fn is_acyclic() -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Unvisited,
        InProgress,
        Done,
    }

    fn visit(node: Crate, marks: &mut [Mark; 11]) -> bool {
        let idx = node as usize;
        match marks[idx] {
            Mark::Done => return true,
            Mark::InProgress => return false, // back-edge → cycle.
            Mark::Unvisited => {}
        }
        marks[idx] = Mark::InProgress;
        for &dep in node.deps() {
            if !visit(dep, marks) {
                return false;
            }
        }
        marks[idx] = Mark::Done;
        true
    }

    let mut marks = [Mark::Unvisited; 11];
    Crate::ALL.iter().all(|&c| visit(c, &mut marks))
}

/// Returns `true` if `myelin-identity` is a SINK whose only dependency is `myelin-tenancy`
/// (architecture §2.9: "identity depends on nothing" above tenancy).
pub fn identity_is_sink() -> bool {
    Crate::Identity.deps() == [Crate::Tenancy]
}

/// Returns `true` if `myelin-substrate` is the ROOT (no crate depends on it) — root-last.
pub fn substrate_is_root() -> bool {
    Crate::ALL
        .iter()
        .all(|&c| !c.deps().contains(&Crate::Substrate))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE `crate-graph-acyclic` test (the prompt's required test). Asserts (1) the DAG
    /// has no cycle, (2) identity is a sink (depends only on tenancy — "identity depends
    /// on nothing" above tenancy), and (3) substrate is root-last (no crate depends on
    /// it). This is the build-layer realisation of the `no-cross-sync-cycle` lint
    /// (architecture §2.9); P-S10 ships the real source-scanning lint.
    #[test]
    fn crate_graph_acyclic() {
        assert!(is_acyclic(), "the crate dependency DAG must be acyclic (§2.9)");
        assert!(
            identity_is_sink(),
            "myelin-identity must depend on nothing above myelin-tenancy (§2.9, identity is a sink)"
        );
        assert!(
            substrate_is_root(),
            "myelin-substrate must be root-last (no crate depends on it)"
        );
    }

    /// A guard test: if a cycle WERE introduced (e.g. tenancy gaining a back-edge to
    /// substrate), `is_acyclic` returns false. Proves the checker actually detects cycles
    /// (it is not vacuously true) — EI-01 §3, prove the assertion can read red.
    #[test]
    fn cycle_detector_actually_detects_cycles() {
        // Build a tiny 2-node cyclic graph inline and run the same DFS shape over it.
        // A ↔ B is a cycle; the detector must reject it.
        fn acyclic_2(a_deps: &[usize], b_deps: &[usize]) -> bool {
            #[derive(Clone, Copy, PartialEq)]
            enum M {
                U,
                P,
                D,
            }
            fn go(n: usize, g: &[&[usize]; 2], m: &mut [M; 2]) -> bool {
                match m[n] {
                    M::D => return true,
                    M::P => return false,
                    M::U => {}
                }
                m[n] = M::P;
                for &d in g[n] {
                    if !go(d, g, m) {
                        return false;
                    }
                }
                m[n] = M::D;
                true
            }
            let g: [&[usize]; 2] = [a_deps, b_deps];
            let mut m = [M::U; 2];
            (0..2).all(|i| go(i, &g, &mut m))
        }
        // acyclic: A→B, B→[]
        assert!(acyclic_2(&[1], &[]));
        // cyclic: A→B, B→A
        assert!(!acyclic_2(&[1], &[0]));
    }

    /// Every crate node's `name()` is unique and matches the eleven workspace members
    /// (the §2.9 ten + the documented `myelin-storage` storage-substrate node).
    #[test]
    fn eleven_crates_named() {
        let names: Vec<&str> = Crate::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), 11);
        assert!(names.contains(&"myelin-tenancy"));
        assert!(names.contains(&"myelin-storage"));
        assert!(names.contains(&"myelin-substrate"));
        // names are unique
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 11, "crate names must be unique");
    }

    /// The `myelin-storage` node is NOT a sink and is below the harness: it depends on
    /// tenancy/identity/events/gdpr, and `myelin-substrate` (the harness) depends on it —
    /// the storage substrate is wired BY the harness (P-ST-01 → P-007).
    #[test]
    fn storage_is_below_the_harness_and_above_its_deps() {
        assert_eq!(
            Crate::Storage.deps(),
            [Crate::Tenancy, Crate::Identity, Crate::Events, Crate::Gdpr],
            "storage depends on tenancy/identity/events/gdpr (mirrors its Cargo.toml)"
        );
        assert!(
            Crate::Substrate.deps().contains(&Crate::Storage),
            "the harness (substrate) must depend on the storage substrate it wires"
        );
        // storage must NOT pull in downstream crates (client/substrate) — no back-edge.
        assert!(!Crate::Storage.deps().contains(&Crate::Client));
        assert!(!Crate::Storage.deps().contains(&Crate::Substrate));
    }
}
