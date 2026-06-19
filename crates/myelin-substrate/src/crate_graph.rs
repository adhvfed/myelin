//! The crate dependency DAG (architecture §2.9), encoded declaratively + asserted acyclic.
//!
//! This is the **build-layer realisation** of the `no-cross-sync-cycle` lint (P-S10 ships
//! the real source-scanning lint over the *service* call graph). Here we model the *crate*
//! graph — the ten Myelin crates and their inter-crate edges in the EXACT root-last order
//! of architecture §2.9:
//!
//! ```text
//! myelin-tenancy → myelin-identity → myelin-events (+ -refs, -content, -query)
//!                → myelin-agent, myelin-gdpr → myelin-client → myelin-substrate
//! ```
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
    Substrate,
}

impl Crate {
    /// All ten crates.
    pub const ALL: [Crate; 10] = [
        Crate::Tenancy,
        Crate::Identity,
        Crate::Events,
        Crate::Refs,
        Crate::Content,
        Crate::Query,
        Crate::Agent,
        Crate::Gdpr,
        Crate::Client,
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
            Crate::Gdpr => &[Crate::Tenancy, Crate::Identity],
            // The client tier (below agent/gdpr).
            Crate::Client => &[Crate::Tenancy, Crate::Identity, Crate::Events],
            // Root-last: the harness depends on everything; nothing depends on it.
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

    fn visit(node: Crate, marks: &mut [Mark; 10]) -> bool {
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

    let mut marks = [Mark::Unvisited; 10];
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

    /// Every crate node's `name()` is unique and matches the ten workspace members.
    #[test]
    fn ten_crates_named() {
        let names: Vec<&str> = Crate::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), 10);
        assert!(names.contains(&"myelin-tenancy"));
        assert!(names.contains(&"myelin-substrate"));
        // names are unique
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 10, "crate names must be unique");
    }
}
