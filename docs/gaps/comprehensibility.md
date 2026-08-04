# Comprehensibility backlog — finding the zen

The standing goal: code whose structure you can follow with ease. Clean APIs,
tear down bad abstractions, split logically, prune verbose comments (they're tech
debt). One piece at a time. Nothing is released — simplify boldly.

## Known offenders
- **`crates/myelin-ci-sandbox/src/gvisor.rs` (~25k lines)** — the biggest single
  module; prime decomposition target (cgroup, checkout-transport, finalize/quiesce,
  workspace, launch are distinct concerns). Load-bearing security code → decompose
  behavior-preserving, with the adversarial-review discipline.
- **Comment density, workspace-wide** — heavy module-doc + inline narration, incl.
  this session's own agent-layer output. Prune to what the code can't say itself.
- **Seam placeholders** — some `myelin-agent` §2.1 types were opaque newtypes filled
  in ad hoc; audit for ones that should now be real or removed.

## Approach
Pair every feature build with a simplification of what it touches. When editing a
file, leave it clearer than found. Prefer deleting a bad abstraction over adding a
wrapper around it. Keep correctness discipline on load-bearing/money/security paths.
