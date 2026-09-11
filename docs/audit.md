# Audit and Terminal Recording

This document describes what NGTerm records, where it is stored, who can
read it, how it is kept and removed, and what the built-in integrity checks
can and cannot show. It reflects what is implemented; planned work is
marked as such.

## What is recorded

| Record | Table | Content |
|---|---|---|
| Session | `audit_sessions` | Who connected (platform user, source address), to what (server, remote account), when it started and ended, why it ended, and whether its recording is complete, has gaps, or was truncated. |
| Operation | `audit_operations` | Something the platform executed on a user's behalf through its own channel: SFTP file read/write/delete/rename/mkdir/upload/download, git queries, agent launches, native-engine tool calls with the commands they caused linked through `parent_operation_id`, and configuration changes. Each has an intent (written **before** execution), a truthful outcome, an exit status (`known` code or `unknown` with reason) and an evidence level (`executor_confirmed`, `parsed_from_output`, `declared`, `none`). |
| Event | `audit_events` | Append-only, per-stream sequenced records: operation lifecycle, configuration-change snapshots (`config.change`), audit access (`audit.export`, `audit.recording_read`, `audit.integrity_check`), and process events (`system.startup`, `system.shutdown`, `system.retention`). |
| Recording | `audit_recordings` + `audit_recording_chunks` | Terminal output of every session as gzip-compressed NDJSON chunks with SHA-256 hashes, plus resize events and (by default) input *metadata* only. |
| Legacy connection log | `audit_logs` | Kept for the historical connection list. |

### What is deliberately not recorded

- **Human keystrokes are not split into commands.** A person typing in a
  terminal produces a recording, not a list of operations. Shell-level
  command boundaries are not knowable from a PTY stream and NGTerm does
  not pretend otherwise. Use the recording player.
- **External CLI agents are recorded as a launch, not as tool calls.** A CLI
  such as Claude Code runs its own tools inside the PTY; the platform cannot
  see them individually. The `agent_launch` operation states what was
  started and what approval mode was requested, and the terminal recording
  holds the rest. Only the embedded engine (enterprise edition) records each
  tool call with the commands it caused.
- **Passwords never reach the audit store.** Password changes and resets
  are recorded as events without the password.
- **SSH private keys** appear only as metadata: name, fingerprint, key type.
- **Tool environment values** appear only as key names; the values stay
  encrypted in their own table.
- **Terminal input content** is not recorded unless
  `NGTERM_RECORDING_INPUT=content` is set. The default records the time and
  byte length of each input, which is enough to locate activity without
  capturing typed secrets.

### Redaction

Summaries and event payloads pass through a redaction pass before they are
stored: `password=`, `token=`, bearer headers, `sk-…` keys, AWS access keys
and `user:pass@host` URLs are replaced by `[REDACTED]`, and JSON keys that
contain `password`, `secret`, `token` or `api_key` have their values
masked (boolean flags such as `"secret": true` are kept). This is a safety
net for command lines and paths, not a guarantee.

## Who can read what

| Caller | Sessions and operations | Recordings | Exports | System events, integrity |
|---|---|---|---|---|
| User | Own rows only, enforced at the query level; other users' rows return 404 | Own sessions only | Own rows only | Refused (403) |
| Admin | All | All | All | Yes |

Every export, recording read and integrity check writes an event on the
`audit-access` stream naming the caller. Exports are bounded at 10 000
rows and flag truncation both in the body and with an
`x-audit-truncated: true` header.

## Truthfulness rules

- An operation is registered as `intended`, then `running`, **before** it
  executes. If the intent cannot be written, the operation is refused with
  HTTP 503 rather than executed unaudited.
- If the handler stops before observing the outcome (client gone, server
  shutting down), the operation is `interrupted`, never `succeeded`.
- If the recorder's queue overflows, dropped events are counted and a gap
  marker is written; the recording is marked `gap` with the count. If the
  storage cannot be written or the size cap is hit, the recording is
  marked `truncated` and the session continues.
- Every server start is recorded. If the previous process did not shut
  down cleanly, sessions it left open are closed with reason
  `server_restart` and `truncated` integrity, operations still running
  become `interrupted`, recordings become `interrupted`. The counts are in
  the `system.startup` event and under `audit` in `/api/health`.

## Configuration

| Variable | Default | Effect |
|---|---|---|
| `NGTERM_RECORDING` | on | `off`, `0` or `false` disables terminal recording (sessions are then marked `truncated: session was not recorded`). |
| `NGTERM_RECORDING_INPUT` | `metadata` | `none`, `metadata` (time + length), or `content`. |
| `NGTERM_RECORDING_MAX_MB` | unlimited | Total size cap for recording files; oldest finished recordings are removed first, then the live recording is truncated if still over. |
| `NGTERM_RECORDING_RETENTION_DAYS` | forever | Recordings whose session ended before the cutoff are removed hourly (files first, then index rows). |
| `NGTERM_DISK_LOW_MB` | 512 | Free space in the data directory below which `/api/health` reports `low_disk` and `/api/admin/metrics` marks the disk as low. |
| `NGTERM_AUDIT_RETENTION_DAYS` | forever | Finished sessions, finished operations, their events and legacy logs older than the cutoff are removed hourly. Live sessions, running operations and `system` events are never removed. Each purge is recorded as `system.retention`. |

Recording files live under `<data-dir>/recordings/<recording-id>/` with
mode 0600 in 0700 directories.

## Append-only protection

Schema migration 4 installs SQLite triggers: `audit_events` rejects every
UPDATE, and DELETE on any audit table is refused unless a maintenance
window is open. The window is a row that exists only inside the retention
transaction, so neither the application nor ad-hoc SQL against the
database file can remove history outside the retention policy. The
integrity report checks that the triggers are still present.

## Integrity check

`GET /api/audit/integrity?recordings=N` (admin) reports:

- whether the append-only triggers are present,
- sequence holes inside any event stream (a missing prefix removed by
  retention is not a hole),
- chunk-by-chunk verification of the newest `N` finished recordings:
  missing files, size or hash mismatches, unreadable headers, index gaps.

The player skips unverifiable chunks and says so; it never renders
unverified bytes as if they were fine.

### Limitation, stated plainly

These checks detect application bugs, accidental edits, truncation and
bit rot. **They do not protect against an actor with full control of the
host**, who can drop the triggers, rewrite chunk files together with their
hashes, or replace the database. If that is in your threat model, ship the
audit tables and recording directory to a system such an actor cannot
reach (for example, a scheduled `--backup-to` to write-once storage).

## Backup and restore

```
ngterm --data-dir /var/lib/ngterm --backup-to /backups/ngterm-2026-09-10
```

The command writes into an **empty** directory (it refuses to overwrite):

- `onemux.db`: a consistent snapshot taken with SQLite `VACUUM INTO`
  (single transaction, safe while the server is running),
- `recordings/`: every finished chunk file (`.part` files in progress are
  skipped; the index describes only finished chunks anyway),
- `MANIFEST.json`: schema version, sizes and counts.

`.env` (pepper and JWT secret) is **not** included: it is a credential and
must be backed up through your secret store. Without the original `.env`
the restored instance cannot decrypt users' SSH keys or verify existing
tokens.

**Restore procedure**

1. Stop the server.
2. Copy `onemux.db` and `recordings/` from the backup into the data
   directory; restore `.env` from your secret store.
3. Start the server. Startup recovery closes anything that was live at
   backup time as `server_restart`/`interrupted`.
4. Run the integrity check (系统事件 → 完整性检查, or `GET
   /api/audit/integrity`). It confirms the chunk index and the files on disk
   agree and that the append-only triggers are in place.

Rehearse this before relying on it.

## API summary

| Method and path | Purpose |
|---|---|
| `GET /api/audit/sessions`, `/api/audit/sessions/{id}` | Session search and detail (operations, recordings). |
| `GET /api/audit/operations`, `/api/audit/operations/{id}` | Operation search (`q`, `status`, `kind`, `actorKind`, `session`, `server`, `task`, `parent`, time range) and detail with events, recording offset and the lower-level operations it caused (`children`). |
| `GET /api/audit/sessions/{id}/recordings` | Recording metadata, chunks and verification problems. |
| `GET /api/audit/recordings/{id}/events` | Playback events (access is recorded). `fromSeq`/`toSeq` select a chunk window so the player loads long recordings progressively; the response always lists all chunks and the verification problems of the chunks it read. |
| `GET /api/audit/export?type=sessions\|operations&format=json\|csv` | Bounded export with the same filters as the lists. |
| `GET /api/audit/system` | Start/shutdown/retention events (admin). |
| `GET /api/audit/integrity` | Integrity report (admin, recorded). |
| `GET /api/health` | Unauthenticated liveness: database check, live counts, recovery summary and `warnings` codes (`audit_write_failures`, `recording_drops`, `low_disk`, `disk_unknown`, `stale_operations`, `unclean_previous_shutdown`, `recording_disabled`). 503 only when the database is unusable. |
| `GET /api/admin/metrics` | The numbers behind the warnings (admin): failed audit writes since start with the last error, recording events dropped, disk free/total, running and stale operations, active sessions and agents. |

## Operational visibility

Every HTTP request carries an `x-request-id` (kept when the client sends
one, otherwise generated), echoed in the response and present as the `id`
field of the request span on every log line the request produces. Terminal
and agent WebSocket tasks log under spans carrying the session or agent id,
and the embedded engine's tasks under the agent id. Spans record method and
path only; query strings and headers never reach the log. Audit records that
could not be written are counted in `/api/admin/metrics` (and the action that
needed them is refused, see "Truthfulness"), recording events dropped under
load are counted process-wide as well as marked as gaps in the recording, and
operations still `running` after 15 minutes are reported as stale so a
handler that died without reporting is visible. Counters reset on restart;
the audit store is the durable record.

## Pre-execution check hook

Before the platform opens a session on a user's behalf or starts a managed
operation through its own channel (file, git, upload, agent launch), it asks
an installed `ExecutionGuard` (`src/guard.rs`) whether to proceed. The guard
sees only server-held facts: the authenticated user id and admin flag, and
either `SessionAdmission {server_id, remote_user}` or `Operation {intent}`
with the same `OperationIntent` that is written to the audit store. It answers
`Proceed`, `Refuse {reason}` (HTTP 403) or `AwaitApproval {request_id,
message}` (HTTP 202 with `{"decision": "await_approval", "approvalRequired":
true, "requestId": ...}`; the client repeats the same request once whatever
process the distribution runs has approved it).

The open-source build installs no guard and everything proceeds as before.
Distributions install one with `AppState::install_guard`, add their own
routes under `/api` through `RouterHooks::extra_api`, and list what they
provide in `RouterHooks::features`, served by the unauthenticated
`GET /api/features` so the shared frontend can show the matching screens.
Nothing a client, a model or a tool result says can enter the decision
except through the recorded intent. Refused and blocked actions never start,
so they leave no `running` operation behind; a denied managed operation is
recorded with status `denied` and the reason.

## Planned, not yet implemented

