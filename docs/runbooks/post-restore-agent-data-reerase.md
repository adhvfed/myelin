# Reapply agent-data erasures after a database restore

This runbook is for a restored cell that may predate one or more completed
agent-data erasures. It replays the preserved live erasure ledger through the
same durable holder used by the privacy API. A successful pass deletes restored
trace, model-replay, and tool-effect rows, destroys the subject key, and writes
the absorbing subject marker that prevents future agent processing.

The command is intentionally limited to the production agent-data holder. It
does not claim to re-erase the current in-memory Chat, Issues, or Git holder
prototypes.

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

The only success output is a bounded JSON receipt. It contains aggregate counts,
not subject identifiers:

```json
{
  "restore_reerase": {
    "scope": "agent_data",
    "restored_before_unix": 1787687000,
    "selected_subjects": 3,
    "newly_re_erased_subjects": 3,
    "already_erased_subjects": 0,
    "records_erased": 14,
    "new_processing_blocked": true,
    "complete": true
  }
}
```

Do not start the cell unless `complete` is `true`. Any non-zero exit is a failed
recovery step, even if earlier subjects were already processed.

## Retry and verify

The pass is resumable. Re-run the exact command after repairing a dependency.
A converged retry selects the same subjects, reports them under
`already_erased_subjects`, and preserves the original durable deletion counts.
It does not recreate keys or data.

Before routing traffic, retain the aggregate receipt with the restore record and
run the ordinary cell readiness checks. The restore drill in
`integration_erasure_survives_restore.rs` exercises the full sequence against a
real `pg_dump` and `pg_restore`; it also attempts new agent work after replay and
requires the restored holder to refuse it.

## Failure interpretation

- “preserved live erasure ledger is unavailable” means the source-of-truth
  ledger could not be read. Do not substitute the restored database.
- “restored agent-data holder is unavailable” means one subject did not finish
  its production erasure path. Repair the target database or KMS and retry.
- “incomplete erasure proof” means the holder could not prove both durable
  deletion counts and an unrecoverable subject key. Treat this as a hard restore
  failure; do not waive it based on manual row inspection.
