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

    pub fn deps(self) -> &'static [Crate] {
        match self {
            Crate::Tenancy => &[],
            Crate::Identity => &[Crate::Tenancy],
            Crate::Events => &[Crate::Tenancy, Crate::Identity],
            Crate::Refs => &[Crate::Tenancy, Crate::Identity, Crate::Events],
            Crate::Content => &[Crate::Tenancy, Crate::Events, Crate::Identity],
            Crate::Query => &[Crate::Tenancy, Crate::Identity, Crate::Events],
            Crate::Agent => &[Crate::Identity, Crate::Events],
            Crate::Gdpr => &[Crate::Tenancy, Crate::Identity, Crate::Events],
            Crate::Client => &[Crate::Tenancy, Crate::Identity, Crate::Events],
            Crate::Storage => &[Crate::Tenancy, Crate::Identity, Crate::Events, Crate::Gdpr],
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
            Mark::InProgress => return false,
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

pub fn identity_is_sink() -> bool {
    Crate::Identity.deps() == [Crate::Tenancy]
}

pub fn substrate_is_root() -> bool {
    Crate::ALL
        .iter()
        .all(|&c| !c.deps().contains(&Crate::Substrate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_graph_acyclic() {
        assert!(
            is_acyclic(),
            "the crate dependency DAG must be acyclic (§2.9)"
        );
        assert!(
            identity_is_sink(),
            "myelin-identity must depend on nothing above myelin-tenancy (§2.9, identity is a sink)"
        );
        assert!(
            substrate_is_root(),
            "myelin-substrate must be root-last (no crate depends on it)"
        );
    }

    #[test]
    fn cycle_detector_actually_detects_cycles() {
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
        assert!(acyclic_2(&[1], &[]));
        assert!(!acyclic_2(&[1], &[0]));
    }

    #[test]
    fn eleven_crates_named() {
        let names: Vec<&str> = Crate::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), 11);
        assert!(names.contains(&"myelin-tenancy"));
        assert!(names.contains(&"myelin-storage"));
        assert!(names.contains(&"myelin-substrate"));
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 11, "crate names must be unique");
    }

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
        assert!(!Crate::Storage.deps().contains(&Crate::Client));
        assert!(!Crate::Storage.deps().contains(&Crate::Substrate));
    }
}
