<p align="center">
  <a href="README.md">English</a> | <a href="README_CN.md">中文</a>
</p>

<p align="center">
  <h1 align="center">NGTerm</h1>
  <p align="center">
    Web-based multi-server SSH terminal manager — deploy on a jump host, manage internal servers from a browser.
  </p>
  <p align="center">
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License"></a>
    <img src="https://img.shields.io/badge/rust-1.75+-orange.svg" alt="Rust">
    <img src="https://img.shields.io/badge/vue-3.5-green.svg" alt="Vue">
  </p>
</p>

---

NGTerm is a single-binary SSH gateway that sits on your jump host and provides browser-based terminal access to internal servers. SSH private keys are encrypted at rest and can only be unlocked by their owner's password.

<!-- TODO: screenshots — terminal with split panes / agent chat / server management -->
<!-- ![Terminal](docs/screenshots/terminal.png) -->

## Features

- **Jump-Host Relay** — deploy on a gateway server, SSH into internal machines; users only need browser access to one endpoint
- **Encrypted Key Storage** — per-user SSH keys encrypted with AES-256-GCM in SQLite; only the owner's password can unlock them, even the admin cannot
- **Multi-Tab & Split Panes** — multiple servers in labeled tabs; split any terminal horizontally/vertically (Ctrl+Shift+D/E/W), layout survives page refresh
- **AI Agent Integration** — launch CLI agents (e.g. Claude Code) against any session: streaming chat UI, tool-call rendering, and execution approval, auto-installed on the target over SSH
- **File Explorer (SFTP)** — browse, upload, download, and edit remote files from the browser
- **Git Panel** — status, log, branches, and diff for repositories on the remote machine
- **Audit Logs** — connection and session history with filtering
- **Admin Terminal** — local shell on the jump host for administrators, with the same split-pane experience
- **Three-Level Tool Config** — admin defines AI tool templates; users and per-server settings override them; secrets encrypted per user
- **Single Binary** — frontend embedded via rust-embed; one file to deploy, zero runtime dependencies

## NGTerm EE

The open-source edition integrates external CLI agents. **NGTerm EE** (enterprise edition) additionally ships a built-in agent engine that runs inside the server process — LLM-driven ReAct loop (Anthropic / OpenAI-compatible), server-side risk classification and command denylists, MCP tool servers, and reusable skills. The tool type selector in the admin UI includes a **"Native Engine"** option: it is part of the shared UI, and starting a native tool on the open-source backend returns *"Native engine tools require NGTerm EE"*. Everything else in this repository is fully functional standalone.

## Architecture

```
Browser (Vue 3 + xterm.js)
    ↕ WebSocket / HTTPS
NGTerm (Rust + Axum, on jump host)
    ↕ SSH (russh)          ↘ spawns CLI agents (stream-json)
Internal Servers
```

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust, Axum, Tokio, russh, SQLite |
| Crypto | Argon2id (KDF), AES-256-GCM, HKDF-SHA256 |
| Frontend | Vue 3, Naive UI, xterm.js, UnoCSS, Pinia |
| Build | rust-embed (single binary), Vite |

## Quick Start

### Prerequisites

- Rust 1.75+
- Node.js 20+, pnpm 10+

### Development

```bash
# Install frontend dependencies
cd frontend && pnpm install && cd ..

# Enable git hooks (cargo fmt + clippy + eslint)
git config core.hooksPath .githooks

# Start backend (:8080)
cargo run

# Start frontend dev server (:3000, proxies /api and /ws to backend)
cd frontend && pnpm dev
```

### Production Build

```bash
# Single step: build.rs compiles frontend and embeds into binary
cargo build --release

# Deploy
scp target/release/ngterm user@server:~/
ssh user@server './ngterm --port 8080 --data-dir ~/.ngterm'
```

### First Use

1. Start the service — on first run a **master key** is generated and printed to the log (or provide one via `--master-key`). Save it; it is shown only once.
2. Visit `http://<host>:8080` and sign in as **admin** with the master key.
3. Create user accounts, server groups, and servers in the admin pages.
4. Sign in as a **user**, add your SSH keys (upload or paste), and connect from the terminal page.

## Security Model

Two roles with separated trust:

- **Admin** authenticates with the master key and manages users/servers — but **cannot decrypt anyone's SSH keys**.
- **User** keys are wrapped by a key derived from the user's own password:

```
user password + pepper (.env, SHA-256 of master key)
    ↓ Argon2id
wrapping key  →  unwraps user_secret (in-memory for the session)
    ↓ HKDF per key_id
data encryption key
    ↓ AES-256-GCM
encrypted SSH private key (SQLite)
```

- A stolen database alone cannot recover private keys (pepper lives in `.env`, passwords live nowhere)
- Decrypted keys exist in memory only while establishing connections, then are zeroized
- Always front with HTTPS in production

## CLI Options

```
ngterm [OPTIONS]

Options:
  -p, --port <PORT>              Listen port [default: 8080]
      --host <HOST>              Listen address [default: 0.0.0.0]
      --data-dir <DIR>           Data directory (SQLite + .env) [default: .]
      --default-cols <COLS>      Default terminal columns [default: 120]
      --default-rows <ROWS>      Default terminal rows [default: 36]
      --master-key <KEY>         Set master key on first run (ignored after init)
```

## Deployment

- **Sizing**: 2 vCPU / 2 GB RAM is sufficient for ≈10 servers / a handful of concurrent users
- **Network**: only expose 443 (HTTPS) + 22 (management SSH); put nginx or Caddy in front for TLS
- **systemd**: run as a non-root user; keep `--data-dir` on a restricted-permission path (it holds the SQLite DB and `.env`)

## License

MIT
