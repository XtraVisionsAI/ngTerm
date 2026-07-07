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

NGTerm is a single-binary SSH gateway that sits on your jump host and provides browser-based terminal access to internal servers. SSH keys are encrypted at rest — the master key lives only in your head.

## Features

- **Domain-Based Relay** — deploy on a gateway server, SSH into internal machines; users only need browser access to one endpoint
- **Encrypted Key Storage** — SSH private keys stored with AES-256-GCM encryption in SQLite; decrypted in-memory only during active sessions
- **Multi-Tab Terminal** — connect to multiple servers simultaneously with labeled tabs showing alias and IP
- **Server & Key Management** — organize servers by domain groups, manage SSH keys with upload or paste
- **Single Binary** — frontend embedded via rust-embed; one file to deploy, zero runtime dependencies

## Architecture

```
Browser (Vue 3 + xterm.js)
    ↕ WebSocket / HTTPS
NGTerm (Rust + Axum, on jump host)
    ↕ SSH (russh)
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
ssh user@server './ngterm --port 8080'
```

### First Use

1. Visit `http://<host>:8080` after starting the service
2. Register an account (set username, password, and Master Key)
3. Add SSH keys (upload file or paste PEM content)
4. Add domains and servers (associate keys)
5. Click "Connect" on the terminal page

## Security Model

```
Master Key (user memory, never stored)
    ↓ Argon2id + salt
Encryption Key (in-memory, valid for session duration)
    ↓ HKDF per key_id
Data Encryption Key
    ↓ AES-256-GCM
Encrypted SSH private key (SQLite)
```

- Master Key is never written to disk — entered by user on every login
- A stolen database cannot recover private keys
- Decrypted SSH keys exist in memory only during connection setup, then zeroized
- Always use HTTPS in production

## Deployment

- **EC2**: t3.small (2 vCPU, 2GB RAM) is sufficient for ≤10 servers / 3 users
- **Security Group**: only expose 443 (HTTPS) + 22 (management SSH)
- **Reverse Proxy**: nginx or Caddy in front for TLS termination
- **systemd**: run as non-root user with restricted file permissions

## Project Structure

```
ngterm/
├── Cargo.toml
├── build.rs                # Auto-builds frontend on release
├── src/
│   ├── main.rs             # Server startup
│   ├── web.rs              # API routes + static file serving
│   ├── ssh_bridge.rs       # russh SSH connection management
│   ├── session_manager.rs  # Terminal session lifecycle
│   ├── ws_handler.rs       # WebSocket ↔ SSH bidirectional relay
│   ├── auth.rs             # Authentication + Master Key sessions
│   ├── crypto.rs           # Encryption primitives
│   ├── key_manager.rs      # SSH key CRUD
│   ├── server_registry.rs  # Domain/Server CRUD
│   ├── db.rs               # SQLite initialization
│   └── config.rs           # Configuration
└── frontend/
    ├── package.json
    ├── vite.config.ts
    └── src/
        ├── pages/          # Login, terminal, server mgmt, key mgmt
        ├── components/     # terminal-view
        ├── composables/    # useApi, useTerminal, useWebSocket
        └── stores/         # auth, session, server
```

## CLI Options

```
ngterm [OPTIONS]

Options:
  -p, --port <PORT>              Listen port [default: 8080]
      --host <HOST>              Listen address [default: 0.0.0.0]
      --data-dir <DIR>           Data directory [default: ~/.ngterm]
      --default-cols <COLS>      Default terminal columns [default: 120]
      --default-rows <ROWS>      Default terminal rows [default: 36]
```

## License

MIT
