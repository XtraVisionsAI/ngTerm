# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

NGTerm is a single-binary web-based SSH terminal manager (Rust + Vue 3). Deployed on a jump host, it relays SSH connections to internal servers via browser-based terminals. SSH keys are encrypted at rest with AES-256-GCM; decryption requires user password + pepper from .env. AI agent CLI tools (e.g. Claude Code) can be launched against a session over a stream-json WebSocket bridge.

## Development Commands

```bash
# Backend (auto-restart on changes)
cargo watch -x run

# Frontend (Vite HMR on port 3000, proxies /api and /ws to :8080)
cd frontend && pnpm dev

# Lint
cargo fmt --check && cargo clippy -- -D warnings
cd frontend && pnpm lint

# Fix
cargo fmt
cd frontend && pnpm lint:fix

# Production build (frontend auto-built via build.rs)
cargo build --release
```

## Pre-commit Hook

Enabled via `git config core.hooksPath .githooks`. Runs:
1. `cargo fmt --check`
2. `cargo clippy -- -D warnings`
3. `eslint` on frontend (only if frontend/ files staged)

## Architecture

```
Browser (Vue 3 + xterm.js + WebSocket)
    ↕ /ws/terminal/:session_id (bidirectional JSON: output/input/resize)
Axum server (port 8080)
    ├── web.rs          — API router + rust-embed static serving (SPA fallback)
    ├── ws_handler.rs   — WebSocket upgrade, scrollback replay, broadcast subscription
    ├── session_manager.rs — session lifecycle, mpsc→broadcast fan-out, scrollback buffer
    ├── ssh_bridge.rs   — russh client, channel owned by single tokio task, SshCommand enum
    ├── local_pty.rs    — local PTY spawn for admin terminal
    ├── agent_bridge.rs — external CLI agent sessions (spawn, stream-json relay)
    ├── auth.rs         — JWT auth, key-wrapping model, AuthSessionStore (in-memory secrets)
    ├── crypto.rs       — Argon2id, AES-256-GCM, HKDF-SHA256, zeroize
    ├── key_manager.rs  — SSH key CRUD, per-key DEK via HKDF
    └── db.rs           — SQLite (rusqlite bundled)
```

### Library Crate & Extension Hooks

The crate builds as both a lib (`ngterm`) and a binary. Downstream distributions can reuse the whole server and replace selected pieces:

- `build_app_state()` / `web::build_router()` — standard entry points (`main.rs` is a thin wrapper)
- `web::build_router_with_hooks(state, RouterHooks)` — replace the agent start handler
- `web::resolve_agent_launch()` / `web::start_agent_inner()` — shared agent launch plumbing
- `AgentBridge::register_embedded()` — register an in-process agent session driven by an external engine
- `Database::seed_tool_if_missing()` — register bundled tool definitions

### Key Crypto Flow

```
password + pepper(.env) → Argon2id → wrapping_key → AES-GCM(user_secret)
user_secret + key_id → HKDF → DEK → AES-GCM(ssh_private_key)
```

Pepper = SHA-256 hash of master_key, stored in `<data-dir>/.env` as `ONEMUX_MASTER_KEY_HASH=`.

### SSH Channel Ownership

russh `Channel` requires `&mut self` — can't wrap in Arc. Solved by moving channel into a single tokio task (`channel_loop`) that receives `SshCommand` via mpsc. WebSocket handlers never touch the channel directly.

### Auth Roles

- **Admin**: logs in with master_key via `/api/auth/admin-login`. Can manage users/servers but cannot decrypt SSH keys.
- **User**: logs in with username+password via `/api/auth/login`. Password-derived wrapping_key unlocks user_secret in memory for the session.

### AI Agent Tools

Tools are defined in the `ai_tools` table (`type` + JSON `options`) and configured at three levels: admin template, per-user overrides (`user_tool_configs`), per-server overrides (`server_tool_configs`). Secrets in configs are encrypted with the user's secret. `tool_type == "external"` launches a CLI (detect/install/launch commands) over SSH or locally; other tool types are rejected by this distribution.

## Frontend Conventions

- Uses `@xv-shared/vite` plugin preset (auto-router via unplugin-vue-router, auto-layout)
- File-based routing in `src/pages/` — `definePage()` for route meta
- ESLint config: `import defineConfig from '@xv-shared/eslint-config'` (default export, not named)
- `sort-imports` rule disabled (conflicts with perfectionist plugin)
- UnoCSS with presetUno + presetIcons (no CDN, local only)
- Naive UI components, Pinia stores, composables pattern

### Split Pane Architecture

Shared logic lives in composables, consumed by both admin (single terminal) and user (multi-tab) pages:

- `usePaneTree.ts` — tree operations (split, close, ratio, node ID update) via `PaneContext` interface
- `usePaneKeyboard.ts` — Ctrl+Shift+D/E/W shortcuts
- `terminal-pane.vue` — recursive split renderer, injects `createPaneSession`
- `terminal-view.vue` — single terminal instance (xterm.js + WebSocket)

Sessions track parent-child relationship via `parentSessionId` on the backend. Split child sessions are created with `parentSessionId` pointing to the root tab session; on page refresh, only root sessions (`parentSessionId == null`) become tabs.

## Commit Convention

Commitizen conventional commits (`feat:`, `fix:`, `refactor:`, etc.). Version bumped via `cz bump`.

## Important Notes

- `.env` and `*.db` are gitignored — never commit them
- `--master-key <KEY>` CLI flag sets master key on first run only (ignored after init)
- `--data-dir` defaults to `.` (current directory)
- JWT secret is auto-generated on first run and stored as `ONEMUX_JWT_SECRET=` in `<data-dir>/.env`
- rust-embed reads from filesystem in debug builds, embeds in release
- Legacy `ONEMUX_*` .env keys and `onemux.db` filename are kept for deployment compatibility
