## Unreleased

### Feat

- **frontend**: unified load/error/empty state and windowed long transcripts (ENG-08)
- **frontend**: operations flows page — parameter form, step-by-step run with approval points, verification evidence, run history, admin JSON editor (UX-04)
- connection efficiency — SSH config import subset, favourites, environment tags, directory bookmarks (UX-05)
- **frontend**: AI task center page with transcript, linked operations, export and continue (UX-02)
- **frontend**: terminal / file / git → AI context with review and redaction before send (UX-01)
- managed file writes with baseline, backup, read-back verification and restore (UX-03)
- session-scoped managed operations from extra routes; controlled command panel
- **frontend**: approvals inbox, admin approval config, second-person waiting state and session admission handling
- **guard**: neutral pre-execution check hook and router extension points (EE-01)
- **replay**: load recordings chunk by chunk (ENG-08)
- **ops**: request ids, runtime metrics and health warnings (ENG-05)
- **audit**: record agent launches and link tool calls to the commands they cause (AUD-03)
- append-only audit guards, retention purge, integrity check and backup (AUD-10)
- record process start/shutdown and recover stale audit state (AUD-08)
- record user, server, key, tool and config changes as audit operations (AUD-09)
- record platform file and git operations as audit operations (AUD-03 core channels)
- audit UI with session/operation search, export and recording player (AUD-05 UI, AUD-06)
- audit query, export and recording-access API (AUD-05, AUD-07 basics)
- record terminal sessions to chunked, verifiable storage (AUD-02, AUD-04)
- versioned audit event model with sessions, operations and event streams (AUD-01)
- versioned transactional migrations, health endpoint and graceful shutdown (ENG-01/ENG-05 basics)
- CI workflow, toolchain pin, cascade agent stop on session delete
- initial release of NGTerm

### Fix

- add flate2 to standalone Cargo.lock
- single-path session teardown, bounded requests and a sane terminal resync protocol (RUN-10)
- portable ioctl request cast (musl Ioctl is c_int, glibc/macOS c_ulong)
- S1 release-blocking fixes for agent tooling (FIX-01~11)
- use published @xv-shared packages from npm instead of local links
- page title OneMux -> NGTerm

### Refactor

- **web**: split web.rs into domain modules under web/ (ENG-06)
- **audit**: expose redact_json for downstream record builders

### Perf

- keep slow SSH and password KDF off the shared locks and async workers (RUN-09)
