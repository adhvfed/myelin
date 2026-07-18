# Worktrees

Run `fed isolate enable` before any other `fed` command in a new Git worktree. Isolation is
directory-scoped and persistent, so every later stack or script command uses that worktree's own
ports, containers, volumes, and state.

Run `fed clean` before removing a worktree so its services and declared volumes are reclaimed.
