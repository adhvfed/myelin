# Comprehensibility backlog — finding the zen

The standing goal: code whose structure you can follow with ease. Clean APIs,
tear down bad abstractions, split logically, prune verbose comments (they're tech
debt). One piece at a time. Nothing is released — simplify boldly.

## Progress
- **agent-host crate** — pruned + the dispatch web collapsed (five entry points → two,
  a `Tools` bundle). Reads clean. Done.
- **`gvisor.rs`** — decomposing a clean piece at a time via pure moves into `gvisor/`,
  each verified by identical test counts (550/0/36 + 588/0/49) before merge:
  cgroup enforcement → `gvisor/cgroup.rs`, git wire-protocol codec →
  `gvisor/git_wire_codec.rs`. 25871 → 24239 so far.

## Known offenders
- **`gvisor.rs` (~24k lines)** — still the biggest module. The remaining clusters
  (finalize/quiesce, workspace/mount, runsc-launch, checkout-transport) are more
  coupled — threaded through `GvisorBackend`, sharing types — so they likely need
  real decoupling, not just a move. Load-bearing security code → behavior-preserving.
- **Comment density, workspace-wide** — heavy module-doc + inline narration. Prune to
  what the code can't say.
- **Seam placeholders** — some `myelin-agent` §2.1 types were opaque newtypes filled
  in ad hoc; audit for ones that should now be real or removed.

## The move that works
Delegate one cohesive cluster to a fresh agent on a worktree → pure move (relocate +
minimum visibility + re-export to keep paths) → prove identical test counts → merge.
If a cluster only extracts by dragging half the file's shared types, don't — that
trades one tangle for another; it needs decoupling first.

## Approach
Pair every feature build with a simplification of what it touches. When editing a
file, leave it clearer than found. Prefer deleting a bad abstraction over adding a
wrapper around it. Keep correctness discipline on load-bearing/money/security paths.
