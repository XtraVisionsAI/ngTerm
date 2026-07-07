<p align="center">
  <a href="README.md">English</a> | <a href="README_CN.md">中文</a>
</p>

<p align="center">
  <h1 align="center">NGTerm</h1>
  <p align="center">
    基于 Domain 中继的多服务器 SSH Web 终端管理工具 — 部署在跳板机上，通过浏览器管理和连接内网服务器。
  </p>
  <p align="center">
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License"></a>
    <img src="https://img.shields.io/badge/rust-1.75+-orange.svg" alt="Rust">
    <img src="https://img.shields.io/badge/vue-3.5-green.svg" alt="Vue">
  </p>
</p>

---

NGTerm 是一个单二进制 SSH 网关，部署在跳板机上，提供浏览器端的终端访问。SSH 密钥加密存储 — Master Key 只在你的记忆中。

## 特性

- **Domain 中继** — 部署在入口服务器，SSH 连接内网目标机器，浏览器只需访问一个地址
- **密钥加密存储** — SSH 私钥以 AES-256-GCM 加密存入 SQLite，用户登录时提供 Master Key 解密，密钥明文仅存于内存
- **多标签页终端** — 同时连接多台服务器，标签显示别名和 IP，xterm.js 渲染
- **服务器/密钥管理** — Domain 分组、服务器 CRUD、密钥上传与关联
- **单二进制部署** — 前端嵌入 Rust 二进制，拷贝即用，零运行时依赖

## 架构

```
浏览器 (Vue 3 + xterm.js)
    ↕ WebSocket / HTTPS
NGTerm (Rust + Axum，部署在跳板机)
    ↕ SSH (russh)
内网服务器
```

## 技术栈

| 层 | 技术 |
|----|------|
| 后端 | Rust, Axum, Tokio, russh, SQLite |
| 加密 | Argon2id (KDF), AES-256-GCM, HKDF-SHA256 |
| 前端 | Vue 3, Naive UI, xterm.js, UnoCSS, Pinia |
| 构建 | rust-embed (单二进制), Vite |

## 快速开始

### 前置要求

- Rust 1.75+
- Node.js 20+, pnpm 10+

### 开发

```bash
# 安装前端依赖
cd frontend && pnpm install && cd ..

# 启用 git hooks (cargo fmt + clippy + eslint)
git config core.hooksPath .githooks

# 启动后端 (:8080)
cargo run

# 启动前端开发服务器 (:3000, 自动代理 /api 和 /ws 到后端)
cd frontend && pnpm dev
```

### 生产构建

```bash
# 一步构建：build.rs 自动编译前端并嵌入二进制
cargo build --release

# 部署
scp target/release/ngterm user@server:~/
ssh user@server './ngterm --port 8080'
```

### 首次使用

1. 启动服务后访问 `http://<host>:8080`
2. 注册账号（设置用户名、密码、Master Key）
3. 添加 SSH 密钥（上传文件或粘贴 PEM 内容）
4. 添加 Domain 和服务器（关联密钥）
5. 在终端页点击"连接服务器"

## 安全模型

```
Master Key (用户记忆，不存储)
    ↓ Argon2id + salt
Encryption Key (内存，会话期间有效)
    ↓ HKDF per key_id
Data Encryption Key
    ↓ AES-256-GCM
加密后的 SSH 私钥 (SQLite)
```

- Master Key 永不落盘，每次登录由用户输入
- 数据库被盗无法还原私钥
- SSH 私钥解密后仅在连接建立期间存于内存，用后 zeroize
- 生产环境务必启用 HTTPS

## 部署建议

- **EC2 实例**: t3.small (2 vCPU, 2GB) 足够 10 台服务器 / 3 人使用
- **安全组**: 仅开放 443 (HTTPS) + 22 (管理用 SSH)
- **反向代理**: 建议前置 nginx/caddy 提供 TLS 终止
- **systemd**: 以非 root 用户运行，限制文件权限

## 项目结构

```
ngterm/
├── Cargo.toml
├── build.rs                # release 时自动构建前端
├── src/
│   ├── main.rs             # 服务启动
│   ├── web.rs              # API 路由 + 静态文件服务
│   ├── ssh_bridge.rs       # russh SSH 连接管理
│   ├── session_manager.rs  # 终端 session 生命周期
│   ├── ws_handler.rs       # WebSocket ↔ SSH 双向透传
│   ├── auth.rs             # 认证 + Master Key 会话
│   ├── crypto.rs           # 加解密原语
│   ├── key_manager.rs      # SSH 密钥 CRUD
│   ├── server_registry.rs  # Domain/Server CRUD
│   ├── db.rs               # SQLite 初始化
│   └── config.rs           # 配置
└── frontend/
    ├── package.json
    ├── vite.config.ts
    └── src/
        ├── pages/          # 登录、终端、服务器管理、密钥管理
        ├── components/     # terminal-view
        ├── composables/    # useApi, useTerminal, useWebSocket
        └── stores/         # auth, session, server
```

## CLI 参数

```
ngterm [OPTIONS]

Options:
  -p, --port <PORT>              监听端口 [default: 8080]
      --host <HOST>              监听地址 [default: 0.0.0.0]
      --data-dir <DIR>           数据目录 [default: ~/.ngterm]
      --default-cols <COLS>      默认终端列数 [default: 120]
      --default-rows <ROWS>      默认终端行数 [default: 36]
```

## 许可证

MIT
