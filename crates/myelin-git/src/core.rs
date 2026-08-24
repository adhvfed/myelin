use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoLoc {
    pub tenant: String,
    pub region: String,
    pub repo: String,
}

impl RepoLoc {
    pub fn new(
        tenant: impl Into<String>,
        region: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        Self {
            tenant: tenant.into(),
            region: region.into(),
            repo: repo.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Oid(pub String);

impl Oid {
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn is_canonical_object_id(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Service {
    UploadPack,
    ReceivePack,
}

impl Service {
    pub fn git_subcommand(self) -> &'static str {
        match self {
            Service::UploadPack => "upload-pack",
            Service::ReceivePack => "receive-pack",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Maintenance {
    Repack,
    WriteCommitGraph,
    WriteBitmaps,
    WriteMidx,
    BundleCreate,
}

impl Maintenance {
    pub fn git_subcommand(self) -> &'static str {
        match self {
            Maintenance::Repack => "repack",
            Maintenance::WriteCommitGraph => "commit-graph",
            Maintenance::WriteBitmaps => "repack",
            Maintenance::WriteMidx => "multi-pack-index",
            Maintenance::BundleCreate => "bundle",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReadOp {
    ReadBlob,
    Diff,
    Blame,
    WalkForProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GitOp {
    AdvertiseRefs(Service),
    Serve(Service),
    Maint(Maintenance),
    Read(ReadOp),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Backend {
    Shell,
    Gix,
}

pub fn backend_for(op: GitOp) -> Backend {
    match op {
        GitOp::AdvertiseRefs(_) | GitOp::Serve(_) | GitOp::Maint(_) => Backend::Shell,
        GitOp::Read(_) => Backend::Gix,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireInvocation {
    pub repo: RepoLoc,
    pub argv: Vec<String>,
    pub stdin: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireOutput {
    pub stdout: Vec<u8>,
    pub status: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitCoreError {
    Wire(String),
    Read(String),
    Routing(String),
}

impl std::fmt::Display for GitCoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitCoreError::Wire(m) => write!(f, "wire op failed: {m}"),
            GitCoreError::Read(m) => write!(f, "read op failed: {m}"),
            GitCoreError::Routing(m) => write!(f, "routing error: {m}"),
        }
    }
}

impl std::error::Error for GitCoreError {}

pub trait WireExecutor {
    fn run(&self, inv: &WireInvocation) -> Result<WireOutput, GitCoreError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub origin: char,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameHunk {
    pub final_start_line: usize,
    pub lines: usize,
    pub commit: Oid,
}

pub trait ReadBackend {
    fn read_blob_bounded(
        &self,
        repo: &RepoLoc,
        oid: &Oid,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, GitCoreError>;
    fn diff_blobs_bounded(
        &self,
        repo: &RepoLoc,
        a: &Oid,
        b: &Oid,
        maximum_blob_bytes: usize,
        maximum_lines: usize,
        maximum_output_bytes: usize,
    ) -> Result<Vec<DiffLine>, GitCoreError>;
    fn blame_bounded(
        &self,
        repo: &RepoLoc,
        path: &str,
        at: &Oid,
        maximum_path_bytes: usize,
        maximum_blob_bytes: usize,
        maximum_hunks: usize,
    ) -> Result<Vec<BlameHunk>, GitCoreError>;
}

pub trait GitCore {
    fn route(&self, op: GitOp) -> Backend;

    fn advertise_refs(&self, repo: &RepoLoc, svc: Service) -> Result<WireOutput, GitCoreError>;
    fn serve(
        &self,
        repo: &RepoLoc,
        svc: Service,
        stdin: Vec<u8>,
    ) -> Result<WireOutput, GitCoreError>;
    fn maintenance(&self, repo: &RepoLoc, m: Maintenance) -> Result<WireOutput, GitCoreError>;

    fn read_blob_bounded(
        &self,
        repo: &RepoLoc,
        oid: &Oid,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, GitCoreError>;
    fn diff_blobs_bounded(
        &self,
        repo: &RepoLoc,
        a: &Oid,
        b: &Oid,
        maximum_blob_bytes: usize,
        maximum_lines: usize,
        maximum_output_bytes: usize,
    ) -> Result<Vec<DiffLine>, GitCoreError>;
    fn blame_bounded(
        &self,
        repo: &RepoLoc,
        path: &str,
        at: &Oid,
        maximum_path_bytes: usize,
        maximum_blob_bytes: usize,
        maximum_hunks: usize,
    ) -> Result<Vec<BlameHunk>, GitCoreError>;
}

pub struct ShellGitCore<E: WireExecutor> {
    exec: E,
}

impl<E: WireExecutor> ShellGitCore<E> {
    pub fn new(exec: E) -> Self {
        Self { exec }
    }

    pub fn executor(&self) -> &E {
        &self.exec
    }

    fn advertise_refs(&self, repo: &RepoLoc, svc: Service) -> Result<WireOutput, GitCoreError> {
        let inv = WireInvocation {
            repo: repo.clone(),
            argv: vec![
                svc.git_subcommand().to_string(),
                "--advertise-refs".to_string(),
            ],
            stdin: Vec::new(),
        };
        self.exec.run(&inv)
    }

    fn serve(
        &self,
        repo: &RepoLoc,
        svc: Service,
        stdin: Vec<u8>,
    ) -> Result<WireOutput, GitCoreError> {
        let inv = WireInvocation {
            repo: repo.clone(),
            argv: vec![
                svc.git_subcommand().to_string(),
                "--stateless-rpc".to_string(),
            ],
            stdin,
        };
        self.exec.run(&inv)
    }

    fn maintenance(&self, repo: &RepoLoc, m: Maintenance) -> Result<WireOutput, GitCoreError> {
        let inv = WireInvocation {
            repo: repo.clone(),
            argv: vec![m.git_subcommand().to_string()],
            stdin: Vec::new(),
        };
        self.exec.run(&inv)
    }
}

pub struct RoutedGitCore<E: WireExecutor, R: ReadBackend> {
    wire: ShellGitCore<E>,
    read: R,
}

impl<E: WireExecutor, R: ReadBackend> RoutedGitCore<E, R> {
    pub fn new(exec: E, read: R) -> Self {
        Self {
            wire: ShellGitCore::new(exec),
            read,
        }
    }

    pub fn wire(&self) -> &ShellGitCore<E> {
        &self.wire
    }

    fn assert_backend(op: GitOp, want: Backend) -> Result<(), GitCoreError> {
        let got = backend_for(op);
        if got == want {
            Ok(())
        } else {
            Err(GitCoreError::Routing(format!(
                "op {op:?} routed to {got:?}, method expects {want:?}"
            )))
        }
    }
}

impl<E: WireExecutor, R: ReadBackend> GitCore for RoutedGitCore<E, R> {
    fn route(&self, op: GitOp) -> Backend {
        backend_for(op)
    }

    fn advertise_refs(&self, repo: &RepoLoc, svc: Service) -> Result<WireOutput, GitCoreError> {
        Self::assert_backend(GitOp::AdvertiseRefs(svc), Backend::Shell)?;
        self.wire.advertise_refs(repo, svc)
    }

    fn serve(
        &self,
        repo: &RepoLoc,
        svc: Service,
        stdin: Vec<u8>,
    ) -> Result<WireOutput, GitCoreError> {
        Self::assert_backend(GitOp::Serve(svc), Backend::Shell)?;
        self.wire.serve(repo, svc, stdin)
    }

    fn maintenance(&self, repo: &RepoLoc, m: Maintenance) -> Result<WireOutput, GitCoreError> {
        Self::assert_backend(GitOp::Maint(m), Backend::Shell)?;
        self.wire.maintenance(repo, m)
    }

    fn read_blob_bounded(
        &self,
        repo: &RepoLoc,
        oid: &Oid,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, GitCoreError> {
        Self::assert_backend(GitOp::Read(ReadOp::ReadBlob), Backend::Gix)?;
        self.read.read_blob_bounded(repo, oid, maximum_bytes)
    }

    fn diff_blobs_bounded(
        &self,
        repo: &RepoLoc,
        a: &Oid,
        b: &Oid,
        maximum_blob_bytes: usize,
        maximum_lines: usize,
        maximum_output_bytes: usize,
    ) -> Result<Vec<DiffLine>, GitCoreError> {
        Self::assert_backend(GitOp::Read(ReadOp::Diff), Backend::Gix)?;
        self.read.diff_blobs_bounded(
            repo,
            a,
            b,
            maximum_blob_bytes,
            maximum_lines,
            maximum_output_bytes,
        )
    }

    fn blame_bounded(
        &self,
        repo: &RepoLoc,
        path: &str,
        at: &Oid,
        maximum_path_bytes: usize,
        maximum_blob_bytes: usize,
        maximum_hunks: usize,
    ) -> Result<Vec<BlameHunk>, GitCoreError> {
        Self::assert_backend(GitOp::Read(ReadOp::Blame), Backend::Gix)?;
        self.read.blame_bounded(
            repo,
            path,
            at,
            maximum_path_bytes,
            maximum_blob_bytes,
            maximum_hunks,
        )
    }
}

pub fn routing_table() -> BTreeMap<String, Backend> {
    let ops = [
        GitOp::AdvertiseRefs(Service::UploadPack),
        GitOp::AdvertiseRefs(Service::ReceivePack),
        GitOp::Serve(Service::UploadPack),
        GitOp::Serve(Service::ReceivePack),
        GitOp::Maint(Maintenance::Repack),
        GitOp::Maint(Maintenance::WriteCommitGraph),
        GitOp::Maint(Maintenance::WriteBitmaps),
        GitOp::Maint(Maintenance::WriteMidx),
        GitOp::Maint(Maintenance::BundleCreate),
        GitOp::Read(ReadOp::ReadBlob),
        GitOp::Read(ReadOp::Diff),
        GitOp::Read(ReadOp::Blame),
        GitOp::Read(ReadOp::WalkForProjection),
    ];
    ops.into_iter()
        .map(|op| (format!("{op:?}"), backend_for(op)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_ids_have_one_full_lowercase_spelling() {
        let oid = "0123456789abcdef0123456789abcdef01234567";
        assert!(is_canonical_object_id(oid));
        assert!(!is_canonical_object_id("deadbeef"));
        assert!(!is_canonical_object_id(&oid.to_uppercase()));
    }

    #[test]
    fn routing_splits_wire_to_shell_read_to_gix() {
        assert_eq!(
            backend_for(GitOp::AdvertiseRefs(Service::UploadPack)),
            Backend::Shell
        );
        assert_eq!(
            backend_for(GitOp::AdvertiseRefs(Service::ReceivePack)),
            Backend::Shell
        );
        assert_eq!(
            backend_for(GitOp::Serve(Service::UploadPack)),
            Backend::Shell
        );
        assert_eq!(
            backend_for(GitOp::Serve(Service::ReceivePack)),
            Backend::Shell
        );
        for m in [
            Maintenance::Repack,
            Maintenance::WriteCommitGraph,
            Maintenance::WriteBitmaps,
            Maintenance::WriteMidx,
            Maintenance::BundleCreate,
        ] {
            assert_eq!(backend_for(GitOp::Maint(m)), Backend::Shell);
        }
        for r in [
            ReadOp::ReadBlob,
            ReadOp::Diff,
            ReadOp::Blame,
            ReadOp::WalkForProjection,
        ] {
            assert_eq!(backend_for(GitOp::Read(r)), Backend::Gix);
        }
    }

    #[test]
    fn routing_table_is_total_and_split() {
        let t = routing_table();
        assert_eq!(t.len(), 13, "all ops enumerated");
        let shell = t.values().filter(|b| **b == Backend::Shell).count();
        let gix = t.values().filter(|b| **b == Backend::Gix).count();
        assert_eq!(shell, 9, "wire + maintenance ops route to Shell");
        assert_eq!(gix, 4, "read ops route to Gix");
    }

    #[test]
    fn subcommands_are_canonical_git() {
        assert_eq!(Service::UploadPack.git_subcommand(), "upload-pack");
        assert_eq!(Service::ReceivePack.git_subcommand(), "receive-pack");
        assert_eq!(Maintenance::Repack.git_subcommand(), "repack");
        assert_eq!(
            Maintenance::WriteCommitGraph.git_subcommand(),
            "commit-graph"
        );
    }

    #[test]
    fn shell_core_routes_through_the_executor_port_not_a_host_command() {
        use std::cell::RefCell;

        struct Recorder {
            calls: RefCell<Vec<WireInvocation>>,
        }
        impl WireExecutor for Recorder {
            fn run(&self, inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
                self.calls.borrow_mut().push(inv.clone());
                Ok(WireOutput {
                    stdout: b"PACK-sentinel".to_vec(),
                    status: 0,
                })
            }
        }

        let core = ShellGitCore::new(Recorder {
            calls: RefCell::new(Vec::new()),
        });
        let repo = RepoLoc::new("acme", "fr-par", "widgets");
        let out = core
            .serve(&repo, Service::UploadPack, b"0000".to_vec())
            .expect("served");
        assert_eq!(out.stdout, b"PACK-sentinel");
        let calls = core.executor().calls.borrow();
        assert_eq!(calls.len(), 1, "exactly one sandboxed invocation");
        assert_eq!(calls[0].argv[0], "upload-pack", "canonical git subcommand");
        assert_eq!(calls[0].stdin, b"0000", "client bytes streamed in");
    }

    #[test]
    fn assert_backend_accepts_match_and_rejects_misroute() {
        assert!(RoutedGitCore::<NoExec, NoRead>::assert_backend(
            GitOp::Serve(Service::UploadPack),
            Backend::Shell
        )
        .is_ok());
        let err = RoutedGitCore::<NoExec, NoRead>::assert_backend(
            GitOp::Read(ReadOp::Diff),
            Backend::Shell,
        )
        .unwrap_err();
        assert!(matches!(err, GitCoreError::Routing(_)));
    }

    #[test]
    fn error_display_is_distinct_and_nonempty() {
        let wire = format!("{}", GitCoreError::Wire("boom".into()));
        let read = format!("{}", GitCoreError::Read("nope".into()));
        let routing = format!("{}", GitCoreError::Routing("bad".into()));
        assert!(wire.contains("wire op failed") && wire.contains("boom"));
        assert!(read.contains("read op failed") && read.contains("nope"));
        assert!(routing.contains("routing error") && routing.contains("bad"));
        assert_ne!(wire, read);
        assert_ne!(read, routing);
    }

    struct NoExec;
    impl WireExecutor for NoExec {
        fn run(&self, _inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
            Ok(WireOutput {
                stdout: Vec::new(),
                status: 0,
            })
        }
    }
    struct NoRead;
    impl ReadBackend for NoRead {
        fn read_blob_bounded(
            &self,
            _r: &RepoLoc,
            _o: &Oid,
            _maximum_bytes: usize,
        ) -> Result<Vec<u8>, GitCoreError> {
            Ok(Vec::new())
        }
        fn diff_blobs_bounded(
            &self,
            _r: &RepoLoc,
            _a: &Oid,
            _b: &Oid,
            _maximum_blob_bytes: usize,
            _maximum_lines: usize,
            _maximum_output_bytes: usize,
        ) -> Result<Vec<DiffLine>, GitCoreError> {
            Ok(Vec::new())
        }
        fn blame_bounded(
            &self,
            _r: &RepoLoc,
            _p: &str,
            _a: &Oid,
            _maximum_path_bytes: usize,
            _maximum_blob_bytes: usize,
            _maximum_hunks: usize,
        ) -> Result<Vec<BlameHunk>, GitCoreError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn routed_core_serves_wire_and_reads_in_process() {
        struct OkExec;
        impl WireExecutor for OkExec {
            fn run(&self, _inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
                Ok(WireOutput {
                    stdout: b"ok".to_vec(),
                    status: 0,
                })
            }
        }
        struct StubRead;
        impl ReadBackend for StubRead {
            fn read_blob_bounded(
                &self,
                _r: &RepoLoc,
                _o: &Oid,
                _maximum_bytes: usize,
            ) -> Result<Vec<u8>, GitCoreError> {
                Ok(b"blob".to_vec())
            }
            fn diff_blobs_bounded(
                &self,
                _r: &RepoLoc,
                _a: &Oid,
                _b: &Oid,
                _maximum_blob_bytes: usize,
                _maximum_lines: usize,
                _maximum_output_bytes: usize,
            ) -> Result<Vec<DiffLine>, GitCoreError> {
                Ok(vec![DiffLine {
                    origin: '+',
                    content: "x".into(),
                }])
            }
            fn blame_bounded(
                &self,
                _r: &RepoLoc,
                _p: &str,
                _a: &Oid,
                _maximum_path_bytes: usize,
                _maximum_blob_bytes: usize,
                _maximum_hunks: usize,
            ) -> Result<Vec<BlameHunk>, GitCoreError> {
                Ok(vec![BlameHunk {
                    final_start_line: 1,
                    lines: 1,
                    commit: Oid::new("deadbeef"),
                }])
            }
        }

        let core = RoutedGitCore::new(OkExec, StubRead);
        let repo = RepoLoc::new("acme", "fr-par", "widgets");

        assert_eq!(
            core.route(GitOp::Serve(Service::UploadPack)),
            Backend::Shell
        );
        assert_eq!(core.route(GitOp::Read(ReadOp::Diff)), Backend::Gix);

        assert_eq!(
            core.serve(&repo, Service::ReceivePack, vec![])
                .unwrap()
                .stdout,
            b"ok"
        );
        assert_eq!(
            core.read_blob_bounded(&repo, &Oid::new("abc"), 16).unwrap(),
            b"blob"
        );
        assert_eq!(
            core.diff_blobs_bounded(&repo, &Oid::new("a"), &Oid::new("b"), 1024, 100, 8192,)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            core.blame_bounded(&repo, "f", &Oid::new("c"), 128, 1024, 100)
                .unwrap()
                .len(),
            1
        );
    }
}
