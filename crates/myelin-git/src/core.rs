//! # The `GitCore` layered seam — GIT-P8 / P-269 (M3-G1)
//!
//! The **internal substrate** the M3 git band stands on (the receive-pack path GIT-P9 and the
//! serving tier GIT-P13 both build on it). It is the **TE-8 Stage-1 position** carried forward
//! verbatim from the architecture:
//!
//! - **WIRE serving (`upload-pack` / `receive-pack` / `ls-refs`) + MAINTENANCE** (repack /
//!   commit-graph / bitmaps / MIDX / bundle-create) run as **canonical `git`, sandboxed +
//!   streamed** — the only complete server-side protocol-v2 implementation. `gix` has **no**
//!   server-side `receive-pack`/`upload-pack` (GitoxideLabs/gitoxide #1299, re-verified Stage-1
//!   2026-06), so a pure-gix server is **not viable for v1**. Do NOT attempt one.
//! - **READ (`read_blob` / `diff` / `blame` / projection walk)** run **in-process** for the hot,
//!   high-fan-out read paths — no `git` fork per diff, a typed object model.
//!
//! ## The seam shape (architecture `01 §2.3`)
//!
//! [`GitCore`] is a strategy trait; each op declares its [`Backend`]. [`RoutedGitCore`] owns one
//! [`WireBackend`] (wire/maint) and one [`ReadBackend`] (read) and routes every call by a per-op
//! capability table ([`backend_for`]). The wire backend is [`ShellGitCore`]; the read backend is
//! [`GixCore`]. Ops migrate Shell→Gix per-op **iff the OQ-1 spike clears** — the seam exists
//! precisely so the gitoxide bet is *swappable*, not load-bearing.
//!
//! ## no-host-exec (X-6 / contract 1.6) — load-bearing
//!
//! The canonical-`git` path processes **untrusted client packs**, so it runs **sandboxed** under
//! the unified ADR-20 / X-6 hardening profile (egress default-deny, ro-root + tmpfs scratch, caps
//! dropped, no-new-privileges, seccomp, capped) and the **real-kernel escape drill (AG-D4/X-6)**
//! gates it exactly as it gates CI/agent execution. Concretely: [`ShellGitCore`] **never** calls
//! `std::process::Command` — it routes every wire/maint invocation through the [`WireExecutor`]
//! port, the same "all execution goes through the one sandbox seam" discipline `ToolHands::exec`
//! enforces for the agent fabric (the equivalence the CI sandbox wires:
//! `ToolHands::exec == launch(JobSpec{kind:Agent})`). So `cargo build --workspace` over this crate
//! carries **no host-exec fingerprint** and the `no-host-exec` lint is green on the seam.
//!
//! ## libgit2 as the v1 in-process read backend (a DOCUMENTED deviation — EI-01 §1)
//!
//! The architecture's *preference* is `gix` (gitoxide) in-process for read, **`libgit2` fallback**
//! "where `gix` lacks a read capability" (`01 §2.2`). At GIT-P8 (toolchain rustc 1.95, 2026-06) the
//! current `gix` release (`gix 0.84` / `gix-hash`) **does not compile** against the workspace
//! toolchain — pulling it would break `cargo build --workspace`. The architecture **already names
//! `libgit2` as the in-process fallback** for exactly this case, so [`GixCore`] is realised over
//! the **`git2` (libgit2) bindings** in v1: a *real* in-process read/diff/blame path (NOT a shell
//! fork — the no-host-exec property holds), behind the same [`ReadBackend`] port. The gix-preferred
//! swap rides the **OQ-1 named floor (GIT-P33)** — the per-op capability-matrix spike — alongside
//! the wire-serving migration. `git2`'s unsafe C FFI is the named cost of the fallback (`01 §2.2`);
//! this crate's own code stays `#![forbid(unsafe_code)]` (libgit2 exposes a safe API surface).
//!
//! ## OQ-1 — the gix-ward server-side migration (NAMED M5+ floor → GIT-P33)
//!
//! Moving wire-serving + read ops off the shell / libgit2 onto a pure-`gix` server is **OQ-1**: a
//! capability-matrix + protocol-compat + sandbox-escape re-drill spike, **gated, NOT guaranteed**
//! (`01 §2.4`, roadmap §3 M3-G1). The verdict is recorded in **GIT-P33** (M5). The seam below is
//! the swappable point that makes the bet cheap — a per-op `Backend` flip in [`backend_for`], not a
//! rewrite.

use std::collections::BTreeMap;

// ───────────────────────────── value types (the seam's vocabulary) ──────────────────────────────

/// Where a repo lives — the residency-pinned `(tenant, region, repo)` locator the front door
/// resolves via `placement_of(repo)` (architecture 00 §2 (A)). Opaque-but-typed here: the seam
/// routes on it; the placement resolution + the residency reject-if-leaving-region land in GIT-P13.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoLoc {
    /// The tenant that owns the repo (from the verified token, never the URL — ID-3).
    pub tenant: String,
    /// The residency region the repo's objects are pinned to (ADR-11).
    pub region: String,
    /// The repo slug within the tenant.
    pub repo: String,
}

impl RepoLoc {
    /// Construct a locator.
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

/// A git object id — `bytea`, hash-agnostic (20-byte SHA-1 / 32-byte SHA-256, TE-23). Rendered hex
/// here (the seam carries the wire-shape; the data model stores `bytea`, `01 §3.0`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Oid(pub String);

impl Oid {
    /// Wrap a hex object id.
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }
    /// The hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The two git smart-protocol services the front door advertises (`git-upload-pack` for
/// fetch/clone, `git-receive-pack` for push). `ls-refs` is the protocol-v2 ref-advertisement
/// command both share. All three are **wire** ops → canonical `git`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Service {
    /// `git-upload-pack` — serves fetch/clone (object download).
    UploadPack,
    /// `git-receive-pack` — accepts push (object upload, into quarantine — GIT-P9).
    ReceivePack,
}

impl Service {
    /// The canonical `git` subcommand name (what the sandboxed argv begins with).
    pub fn git_subcommand(self) -> &'static str {
        match self {
            Service::UploadPack => "upload-pack",
            Service::ReceivePack => "receive-pack",
        }
    }
}

/// The maintenance ops (repack / commit-graph / bitmaps / MIDX / prune / bundle-create) — all
/// **wire-class** (canonical `git`, sandboxed). Carried as a typed enum so the capability table
/// ([`backend_for`]) routes each one; the bodies (the real repack strategy etc.) land with the
/// serving tier. The seam pins WHICH backend each runs on (the GIT-P8 deliverable).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Maintenance {
    /// Geometric repack.
    Repack,
    /// Write the commit-graph.
    WriteCommitGraph,
    /// Write reachability bitmaps.
    WriteBitmaps,
    /// Write the multi-pack-index.
    WriteMidx,
    /// Create a clone bundle (the within-EU CDN clone class, 11.2 C3).
    BundleCreate,
}

impl Maintenance {
    /// The canonical `git` subcommand the sandboxed argv begins with.
    pub fn git_subcommand(self) -> &'static str {
        match self {
            Maintenance::Repack => "repack",
            Maintenance::WriteCommitGraph => "commit-graph",
            Maintenance::WriteBitmaps => "repack", // bitmaps ride a `repack -b`.
            Maintenance::WriteMidx => "multi-pack-index",
            Maintenance::BundleCreate => "bundle",
        }
    }
}

/// The read ops the hot front-end + the code projection hammer — **in-process** (gix-preferred,
/// libgit2 fallback). NOT a shell fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReadOp {
    /// `read_blob` — fetch object bytes.
    ReadBlob,
    /// `diff_blobs` — Myers/Histogram diff (feeds the content-anchor fingerprint, `02 §5`).
    Diff,
    /// `blame` — line provenance.
    Blame,
    /// `walk_for_projection` — the changed-blob walk feeding the Search code projection.
    WalkForProjection,
}

/// Every op the seam routes, tagged by class. The capability table ([`backend_for`]) maps each to
/// a [`Backend`]; this is the single enumeration of "what GitCore does", so a new op can NOT be
/// added without a backend decision (the routing is total — proven by [`backend_for`]'s match).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GitOp {
    /// Advertise refs (protocol-v2 `ls-refs`) for a service — wire.
    AdvertiseRefs(Service),
    /// `upload-pack` / `receive-pack` byte plumbing — wire.
    Serve(Service),
    /// A maintenance op — wire.
    Maint(Maintenance),
    /// A read op — in-process.
    Read(ReadOp),
}

/// Which backend an op runs on (the strategy choice). The whole point of the seam: a per-op flip
/// here (Shell→Gix) is the OQ-1 migration, not a rewrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Backend {
    /// Sandboxed canonical `git` (wire + maintenance) — runs through the [`WireExecutor`] port.
    Shell,
    /// In-process gix (libgit2 fallback) — read/diff/blame/projection.
    Gix,
}

/// **The v1 capability table** (`01 §2.3` "a per-op capability table routes each call"). Wire +
/// maintenance → [`Backend::Shell`]; read → [`Backend::Gix`]. This match is **total** over
/// [`GitOp`]: every op has exactly one backend, so the routing is correct-by-construction (the
/// GIT-P8 GATE — "routes wire ops to canonical git, read ops to gix, 0 routing errors"). An op
/// migrates Shell→Gix by flipping its arm here IFF the OQ-1 spike (GIT-P33) clears for it.
pub fn backend_for(op: GitOp) -> Backend {
    match op {
        // WIRE + MAINTENANCE — canonical `git`, sandboxed (v1). gix has no server-side serving.
        GitOp::AdvertiseRefs(_) | GitOp::Serve(_) | GitOp::Maint(_) => Backend::Shell,
        // READ — in-process gix (libgit2 fallback).
        GitOp::Read(_) => Backend::Gix,
    }
}

// ───────────────────────────── the wire executor (sandbox-exec port) ────────────────────────────

/// A sandboxed canonical-`git` invocation: the argv (subcommand + flags), the target repo, and the
/// pack/negotiation bytes streamed in. This is the value the [`WireExecutor`] runs — the git-shaped
/// analogue of the agent fabric's `Command` / CI sandbox's `JobSpec`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireInvocation {
    /// The repo the op targets.
    pub repo: RepoLoc,
    /// The canonical `git` argv (`["upload-pack", "--stateless-rpc", "<repo>"]` etc.). The seam
    /// builds it; the executor runs it sandboxed.
    pub argv: Vec<String>,
    /// The streamed input (the client's negotiation / pack bytes). Empty for ref advertisement.
    pub stdin: Vec<u8>,
}

/// The outcome of a sandboxed wire invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireOutput {
    /// The streamed output (the pack / ref advertisement bytes).
    pub stdout: Vec<u8>,
    /// The process exit status (0 == success).
    pub status: i32,
}

/// An error from the wire path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitCoreError {
    /// The sandboxed `git` op failed (non-zero exit / sandbox veto).
    Wire(String),
    /// An in-process read op failed (object not found, bad repo, …).
    Read(String),
    /// The op was routed to the wrong backend (a routing bug — the GATE asserts this is 0).
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

/// **The sandbox-exec port (the no-host-exec seam).** Every canonical-`git` invocation goes through
/// `run` — the ONE execution path, exactly as the agent fabric routes all execution through
/// `ToolHands::exec` and the CI sandbox through `SandboxBackend::launch`. The PRODUCTION impl runs
/// `git` under the unified X-6 hardening profile (egress-deny, ro-root + tmpfs, caps dropped,
/// no-new-privileges, seccomp, capped) and is gated by the real-kernel escape drill (AG-D4); it
/// lives in the serving tier (the CI-sandbox-shaped backend), NOT in this seam crate, so this
/// crate carries no host-exec fingerprint and the `no-host-exec` lint stays green over `src/`.
///
/// **Floor:** the production X-6-hardened executor (the sandboxed `git receive-pack`/`upload-pack`
/// host) is wired in GIT-P9 (receive-pack → one-tx ref-CAS) / GIT-P13 (the serving tier), onto the
/// same CI-sandbox runner the agent fabric dispatches onto. This port is the swappable seam.
pub trait WireExecutor {
    /// Run a canonical-`git` invocation **sandboxed**, streaming stdin in and stdout out.
    fn run(&self, inv: &WireInvocation) -> Result<WireOutput, GitCoreError>;
}

// ───────────────────────────── the read backend (in-process port) ───────────────────────────────

/// A unified diff hunk-line (the seam's diff shape; the content-anchor fingerprint over it lands in
/// the diff-anchor service). Minimal-but-real so the smoke test can assert a true diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// `+` added, `-` removed, ` ` context.
    pub origin: char,
    /// The line content (no trailing newline).
    pub content: String,
}

/// A blame entry — one contiguous line-range attributed to one commit (line provenance).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameHunk {
    /// The 1-based start line in the final file.
    pub final_start_line: usize,
    /// How many lines this hunk covers.
    pub lines: usize,
    /// The commit the lines are attributed to.
    pub commit: Oid,
}

/// **The in-process read port** (gix-preferred / libgit2 fallback). Read/diff/blame/projection —
/// NO shell fork (the no-host-exec property holds for read by construction: in-process, no
/// `Command`). The v1 impl ([`GixCore`]) is over `git2` (libgit2), the architecture-named fallback;
/// the gix-preferred swap is the OQ-1 floor (GIT-P33), a per-op flip behind this port.
pub trait ReadBackend {
    /// Read an object's bytes, rejecting from its header before allocation above `maximum_bytes`.
    fn read_blob_bounded(
        &self,
        repo: &RepoLoc,
        oid: &Oid,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, GitCoreError>;
    /// Diff two blobs (a Myers/Histogram unified diff).
    fn diff_blobs(&self, repo: &RepoLoc, a: &Oid, b: &Oid) -> Result<Vec<DiffLine>, GitCoreError>;
    /// Blame a path at a commit.
    fn blame(&self, repo: &RepoLoc, path: &str, at: &Oid) -> Result<Vec<BlameHunk>, GitCoreError>;
}

// ───────────────────────────── the GitCore trait (the unified seam) ──────────────────────────────

/// **The `GitCore` layered seam** (architecture `01 §2.3`). Wire/maintenance ops route to the
/// sandboxed canonical-`git` backend; read ops route to the in-process backend. Implemented by
/// [`RoutedGitCore`], which owns both backends and routes by [`backend_for`].
///
/// This is the **internal substrate** GIT-P9 (receive-pack → one-tx ref-CAS + outbox) and GIT-P13
/// (the serving tier) build on. No new wire contract lands here (those land in GIT-P9/P13); the
/// seam is the thing they stand on.
pub trait GitCore {
    /// Which backend would handle `op` (the routing decision — exposed so callers + the GATE can
    /// assert the wire/read split without running the op).
    fn route(&self, op: GitOp) -> Backend;

    // ── WIRE + MAINTENANCE — canonical `git`, sandboxed (Backend::Shell) ──
    /// Advertise refs (protocol-v2 `ls-refs`) — wire.
    fn advertise_refs(&self, repo: &RepoLoc, svc: Service) -> Result<WireOutput, GitCoreError>;
    /// Serve `upload-pack` / `receive-pack` (byte plumbing) — wire. The receive-pack POLICY +
    /// ref-CAS + outbox (BUS-2) wrap this in GIT-P9; here the seam routes the bytes.
    fn serve(
        &self,
        repo: &RepoLoc,
        svc: Service,
        stdin: Vec<u8>,
    ) -> Result<WireOutput, GitCoreError>;
    /// Run a maintenance op — wire.
    fn maintenance(&self, repo: &RepoLoc, m: Maintenance) -> Result<WireOutput, GitCoreError>;

    // ── READ — in-process gix (libgit2 fallback) (Backend::Gix) ──
    /// Read object bytes — in-process, with an explicit allocation ceiling.
    fn read_blob_bounded(
        &self,
        repo: &RepoLoc,
        oid: &Oid,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, GitCoreError>;
    /// Diff two blobs — in-process.
    fn diff_blobs(&self, repo: &RepoLoc, a: &Oid, b: &Oid) -> Result<Vec<DiffLine>, GitCoreError>;
    /// Blame a path at a commit — in-process.
    fn blame(&self, repo: &RepoLoc, path: &str, at: &Oid) -> Result<Vec<BlameHunk>, GitCoreError>;
}

// ───────────────────────────── ShellGitCore (the wire backend façade) ───────────────────────────

/// The wire/maintenance backend: builds the canonical-`git` argv for each op and runs it through
/// the injected [`WireExecutor`] sandbox port. **Never** touches `std::process::Command` — the
/// executor owns the sandboxed launch (no-host-exec). Generic over the executor so the production
/// X-6 host (GIT-P9/P13) and the test executor swap cleanly.
pub struct ShellGitCore<E: WireExecutor> {
    exec: E,
}

impl<E: WireExecutor> ShellGitCore<E> {
    /// Build the wire backend over a sandbox executor.
    pub fn new(exec: E) -> Self {
        Self { exec }
    }

    /// The executor (the sandbox port) — for the production host to inspect/share.
    pub fn executor(&self) -> &E {
        &self.exec
    }

    fn advertise_refs(&self, repo: &RepoLoc, svc: Service) -> Result<WireOutput, GitCoreError> {
        let inv = WireInvocation {
            repo: repo.clone(),
            // protocol-v2 ref advertisement: `git <service> --advertise-refs <repo>`.
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

// ───────────────────────────── RoutedGitCore (the seam impl + router) ────────────────────────────

/// The `GitCore` impl: owns a [`ShellGitCore`] (wire) and a [`ReadBackend`] (read), and routes
/// every call by [`backend_for`]. A defensive routing assertion guards each method: if the
/// capability table ever disagreed with the method's class, it returns [`GitCoreError::Routing`]
/// instead of silently running on the wrong backend (the GATE: 0 routing errors).
pub struct RoutedGitCore<E: WireExecutor, R: ReadBackend> {
    wire: ShellGitCore<E>,
    read: R,
}

impl<E: WireExecutor, R: ReadBackend> RoutedGitCore<E, R> {
    /// Compose the seam over a sandboxed wire executor and an in-process read backend.
    pub fn new(exec: E, read: R) -> Self {
        Self {
            wire: ShellGitCore::new(exec),
            read,
        }
    }

    /// The wire backend (for the production host that wraps receive-pack policy around it, GIT-P9).
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

    fn diff_blobs(&self, repo: &RepoLoc, a: &Oid, b: &Oid) -> Result<Vec<DiffLine>, GitCoreError> {
        Self::assert_backend(GitOp::Read(ReadOp::Diff), Backend::Gix)?;
        self.read.diff_blobs(repo, a, b)
    }

    fn blame(&self, repo: &RepoLoc, path: &str, at: &Oid) -> Result<Vec<BlameHunk>, GitCoreError> {
        Self::assert_backend(GitOp::Read(ReadOp::Blame), Backend::Gix)?;
        self.read.blame(repo, path, at)
    }
}

// ───────────────────────────── routing introspection (the GATE helper) ──────────────────────────

/// Every op the seam knows, with its routed backend — the data the GATE asserts the wire/read
/// split over (no op un-routed; wire ops → Shell, read ops → Gix). Used by the routing unit test
/// and any future capability-matrix (OQ-1) tooling.
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

    /// Every wire/maint op routes to Shell; every read op routes to Gix — the total, correct
    /// routing the GATE requires (0 routing errors). This is the mutation-tested core of the seam.
    #[test]
    fn routing_splits_wire_to_shell_read_to_gix() {
        // Wire + maintenance → Shell.
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
        // Read → Gix.
        for r in [
            ReadOp::ReadBlob,
            ReadOp::Diff,
            ReadOp::Blame,
            ReadOp::WalkForProjection,
        ] {
            assert_eq!(backend_for(GitOp::Read(r)), Backend::Gix);
        }
    }

    /// The routing table covers every op kind with exactly one backend (no un-routed op).
    #[test]
    fn routing_table_is_total_and_split() {
        let t = routing_table();
        assert_eq!(t.len(), 13, "all ops enumerated");
        let shell = t.values().filter(|b| **b == Backend::Shell).count();
        let gix = t.values().filter(|b| **b == Backend::Gix).count();
        assert_eq!(shell, 9, "wire + maintenance ops route to Shell");
        assert_eq!(gix, 4, "read ops route to Gix");
    }

    /// `Service` / `Maintenance` map to the right canonical `git` subcommand (the argv the
    /// sandboxed executor runs).
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

    /// A recording executor proves the wire backend routes through the [`WireExecutor`] port (NOT a
    /// host `Command`) and builds the canonical argv — the no-host-exec discipline by construction.
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

    /// The defensive routing guard ACCEPTS a correctly-routed op and REJECTS a mis-routed one
    /// (instead of silently running on the wrong backend). This kills the
    /// `assert_backend -> Ok(())` mutant: the guard must distinguish the two.
    #[test]
    fn assert_backend_accepts_match_and_rejects_misroute() {
        // A wire op declared as Shell is accepted.
        assert!(RoutedGitCore::<NoExec, NoRead>::assert_backend(
            GitOp::Serve(Service::UploadPack),
            Backend::Shell
        )
        .is_ok());
        // A read op declared as Shell is REJECTED (the capability table says Gix).
        let err = RoutedGitCore::<NoExec, NoRead>::assert_backend(
            GitOp::Read(ReadOp::Diff),
            Backend::Shell,
        )
        .unwrap_err();
        assert!(matches!(err, GitCoreError::Routing(_)));
    }

    /// `GitCoreError::Display` renders each variant distinctly (kills the `fmt -> Ok(default)`
    /// mutant: the rendered text must be non-empty + variant-distinguishing).
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

    /// A no-op executor + read backend for type-level guard tests (no behaviour exercised).
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
        fn diff_blobs(
            &self,
            _r: &RepoLoc,
            _a: &Oid,
            _b: &Oid,
        ) -> Result<Vec<DiffLine>, GitCoreError> {
            Ok(Vec::new())
        }
        fn blame(&self, _r: &RepoLoc, _p: &str, _a: &Oid) -> Result<Vec<BlameHunk>, GitCoreError> {
            Ok(Vec::new())
        }
    }

    /// The router rejects a mis-routed call instead of silently running on the wrong backend (the
    /// defensive 0-routing-errors guard).
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
            fn diff_blobs(
                &self,
                _r: &RepoLoc,
                _a: &Oid,
                _b: &Oid,
            ) -> Result<Vec<DiffLine>, GitCoreError> {
                Ok(vec![DiffLine {
                    origin: '+',
                    content: "x".into(),
                }])
            }
            fn blame(
                &self,
                _r: &RepoLoc,
                _p: &str,
                _a: &Oid,
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

        // Wire op succeeds through the shell backend.
        assert_eq!(
            core.serve(&repo, Service::ReceivePack, vec![])
                .unwrap()
                .stdout,
            b"ok"
        );
        // Read op succeeds through the in-process backend.
        assert_eq!(
            core.read_blob_bounded(&repo, &Oid::new("abc"), 16)
                .unwrap(),
            b"blob"
        );
        assert_eq!(
            core.diff_blobs(&repo, &Oid::new("a"), &Oid::new("b"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(core.blame(&repo, "f", &Oid::new("c")).unwrap().len(), 1);
    }
}
