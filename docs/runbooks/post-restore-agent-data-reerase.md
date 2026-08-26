# Reapply privacy erasures after a database restore

This runbook is for a restored cell that may predate one or more completed
agent-data, Chat-message, or Issue-title erasures. It reads the preserved live
ledger and replays each scope through the same durable holder used by the
privacy API.

For `agent_data`, a successful pass deletes restored trace, model-replay, and
tool-effect rows, destroys the scoped key, and restores the absorbing marker
that prevents future agent processing. For `chat_messages`, it destroys the
restored Chat key, empties the authored message bodies, retains their immutable
tombstone coordinates, and co-commits the corresponding erasure events. A
person may write new Chat messages afterward under fresh key material.
For `issue_titles`, it destroys the restored Issues-scoped key, replaces each
authored title with the explicit erased placeholder, removes its encrypted
material and direct creator identity, and co-commits one content-free update
event. The Issue itself and a colleague's work remain.

The three ledger queries are scope-specific. No record can be routed to another
holder. Git is not included until its durable erasure path writes this ledger
and can return an equally strong proof.

## Before running

1. Keep Edge, hosted-agent workers, and every other process that can use the
   restored cell stopped. Do not make the cell ready or route traffic to it.
2. Preserve a database containing the live `post_pit_erasure_ledger`. This must
   be a different PostgreSQL host, port, or database from the restored target.
   Restoring the ledger from the same backup would remove precisely the later
   erasures this pass needs to replay.
3. Configure the normal Myelin database and migration environment for the
   restored target. Configure the same cell ID and KMS seal material that belong
   to the restored backup.
4. Determine the Unix second immediately before the restore point. If the exact
   second is uncertain, choose an earlier value: selecting an already-applied
   erasure is safe and idempotent; choosing a later value can omit required
   work.
5. Set `MYELIN_POST_RESTORE_LEDGER_DATABASE_URL` to the preserved live ledger
   database. Keep connection material in the process environment or the
   deployment secret mechanism; never place it in the command line or a ticket.

## Run the pass

With the repository build corresponding to the restored cell:

```sh
cargo run --quiet -p myelin-edge --bin edge -- privacy-reerase \
  --restored-before-unix 1787687000 \
  --confirm-cell cell-eu-1 \
  --confirm-services-stopped yes
```

Replace the timestamp and cell ID with the restore's values. The command refuses
non-canonical timestamps, a mismatched cell, an unconfirmed maintenance state,
a missing live-ledger connection, or a live-ledger URL that resolves to the same
PostgreSQL database as the restored target.

The only success output is a bounded JSON receipt. It contains aggregate counts
for every supported scope, never subject identifiers:

```json
{
  "restore_reerase": {
    "restored_before_unix": 1787687000,
    "scopes": {
      "agent_data": {
        "selected_subjects": 3,
        "newly_re_erased_subjects": 3,
        "already_erased_subjects": 0,
        "records_erased": 14,
        "new_processing_blocked": true
      },
      "chat_messages": {
        "selected_subjects": 2,
        "newly_re_erased_subjects": 2,
        "already_erased_subjects": 0,
        "messages_erased": 9,
        "erasure_events_co_committed": 9
      },
      "issue_titles": {
        "selected_subjects": 2,
        "newly_re_erased_subjects": 2,
        "already_erased_subjects": 0,
        "titles_erased": 4,
        "erasure_events_co_committed": 4
      }
    },
    "complete": true
  }
}
```

Do not start the cell unless `complete` is `true`. Any non-zero exit is a failed
recovery step, even if earlier subjects were already processed.

## Retry and verify

The pass is resumable. Re-run the exact command after repairing a dependency.
A converged retry selects the same subjects in each scope, reports them under
that scope's `already_erased_subjects`, and preserves the original durable
deletion counts. It does not recreate keys, data, or erasure events.

Before routing traffic, retain the aggregate receipt with the restore record and
run the ordinary cell readiness checks. The agent-data restore drill in
`integration_erasure_survives_restore.rs` exercises the full sequence against a
real `pg_dump` and `pg_restore`; it also attempts new agent work after replay and
requires the restored holder to refuse it. The Chat PostgreSQL restore story in
`integration_chat_p22_erase_cascade.rs` exercises scoped selection, key
destruction, message/event co-commit, neighboring-subject isolation, and replay.
The Issues holder has real PostgreSQL lifecycle coverage and is composed into
this operator, but its full dump/restore disaster story remains required before
calling that scope restore-drilled.

## Failure interpretation

- “preserved live erasure ledger is unavailable” means the source-of-truth
  ledger could not be read. Do not substitute the restored database.
- “restored agent-data holder is unavailable” means one subject did not finish
  its production erasure path. Repair the target database or KMS and retry.
- “restored Chat re-erasure failed” means one subject did not finish its scoped
  key destruction and message/event transaction. Repair the target database or
  KMS and retry with the same restore command.
- “restored Issue re-erasure failed” means one subject did not finish its
  scoped title-key destruction and title/event transaction. Repair the target
  database or KMS and retry with the same restore command.
- “incomplete erasure proof” means the holder could not prove both durable
  deletion counts and an unrecoverable agent-data key. Treat this as a hard restore
  failure; do not waive it based on manual row inspection.
