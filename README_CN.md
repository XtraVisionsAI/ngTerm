<p align="center">
  <a href="README.md">English</a> | <a href="README_CN.md">中文</a>
</p>

<p align="center">
  <h1 align="center">NGTerm</h1>
  <p align="center">
    具备 Agentic 能力的 Web SSH 终端管理平台 —— 浏览器管理内网服务器，让 AI Agent 替你执行运维操作。
  </p>
  <p align="center">
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License"></a>
    <img src="https://img.shields.io/badge/rust-1.75+-orange.svg" alt="Rust">
    <img src="https://img.shields.io/badge/vue-3.5-green.svg" alt="Vue">
  </p>
</p>

---

NGTerm 是一个单二进制 SSH 网关，部署在跳板机上，提供浏览器端的内网服务器终端访问——并把 AI Agent 融入运维工作流：打开会话，交给 agent（如 Claude Code）执行，全程可见每条命令，危险操作需你审批。SSH 私钥加密存储，只有密钥所有者的密码才能解锁。

<!-- TODO: 截图占位 —— 分屏终端 / agent 对话 / 服务器管理 -->
<!-- ![Terminal](docs/screenshots/terminal.png) -->

## 功能特性

- **AI Agent 集成** —— 对任意会话启动 CLI agent（如 Claude Code）：流式对话界面、工具调用渲染、执行审批，可通过 SSH 在目标机自动安装
- **跳板机中继** —— 部署在网关服务器上向内网机器发起 SSH，用户只需能访问一个浏览器端点
- **密钥加密存储** —— 每用户的 SSH 密钥以 AES-256-GCM 加密存入 SQLite，只有所有者本人的密码能解锁，管理员也不行
- **多标签 + 分屏** —— 多服务器标签页；任意终端横/纵向分屏（Ctrl+Shift+D/E/W），布局在刷新后保持
- **文件管理器（SFTP）** —— 浏览器内浏览、上传、下载、编辑远程文件
- **Git 面板** —— 远程仓库的 status / log / 分支 / diff
- **审计日志** —— 连接与会话历史，支持筛选
- **管理员终端** —— 跳板机本地 shell，同样支持分屏
- **工具三级配置** —— 管理员定义 AI 工具模板，用户级、服务器级配置逐层覆盖；敏感参数按用户加密
- **单二进制** —— 前端经 rust-embed 嵌入，一个文件即可部署，零运行时依赖

## NGTerm EE

开源版集成外部 CLI agent。**NGTerm EE**（企业版）额外内置了运行在服务进程内的 agent 引擎——LLM 驱动的 ReAct 循环（Anthropic / OpenAI 兼容）、服务端风险分级与命令 denylist、MCP 工具服务器、可复用 skills。管理界面的工具类型中有一个 **"Native Engine"** 选项：它属于两版共用的 UI，在开源后端上启动 native 工具会返回 *"Native engine tools require NGTerm EE"*。本仓库的其余全部功能均可独立使用。

## 架构

```
浏览器 (Vue 3 + xterm.js)
    ↕ WebSocket / HTTPS
NGTerm (Rust + Axum，跳板机)
    ↕ SSH (russh)          ↘ 启动 CLI agent (stream-json)
内网服务器
```

## 技术栈

| 层 | 技术 |
|-------|-----------|
| 后端 | Rust, Axum, Tokio, russh, SQLite |
| 加密 | Argon2id (KDF), AES-256-GCM, HKDF-SHA256 |
| 前端 | Vue 3, Naive UI, xterm.js, UnoCSS, Pinia |
| 构建 | rust-embed（单二进制）, Vite |

## 快速开始

### 环境要求

- Rust 1.75+
- Node.js 20+，pnpm 10+

### 开发

```bash
# 安装前端依赖
cd frontend && pnpm install && cd ..

# 启用 git hooks（cargo fmt + clippy + eslint）
git config core.hooksPath .githooks

# 启动后端 (:8080)
cargo run

# 启动前端开发服务器 (:3000，/api 和 /ws 代理到后端)
cd frontend && pnpm dev
```

### 生产构建

```bash
# 一步完成：build.rs 自动构建前端并嵌入二进制
cargo build --release

# 部署
scp target/release/ngterm user@server:~/
ssh user@server './ngterm --port 8080 --data-dir ~/.ngterm'
```

### 首次使用

1. 启动服务 —— 首次运行会生成 **master key** 并打印到日志（也可用 `--master-key` 指定）。务必保存，仅显示一次。
2. 访问 `http://<host>:8080`，用 master key 以**管理员**身份登录。
3. 在管理页面创建用户账号、服务器分组和服务器。
4. 以**用户**身份登录，添加自己的 SSH 密钥（上传或粘贴），在终端页连接。

## 安全模型

双角色、信任分离：

- **管理员**用 master key 登录，管理用户与服务器——但**无法解密任何人的 SSH 密钥**。
- **用户**密钥由用户自己密码派生的密钥包裹：

```
用户密码 + pepper（.env，master key 的 SHA-256）
    ↓ Argon2id
wrapping key  →  解开 user_secret（仅会话期内存中）
    ↓ HKDF（按 key_id）
数据加密密钥
    ↓ AES-256-GCM
加密的 SSH 私钥（SQLite）
```

- 仅窃取数据库无法恢复私钥（pepper 在 `.env`，密码不落盘）
- 解密后的私钥仅在建立连接期间存于内存，随后清零（zeroize）
- 生产环境务必置于 HTTPS 之后

## 命令行参数

```
ngterm [OPTIONS]

Options:
  -p, --port <PORT>              监听端口 [默认: 8080]
      --host <HOST>              监听地址 [默认: 0.0.0.0]
      --data-dir <DIR>           数据目录（SQLite + .env）[默认: .]
      --default-cols <COLS>      终端默认列数 [默认: 120]
      --default-rows <ROWS>      终端默认行数 [默认: 36]
      --master-key <KEY>         首次运行时指定 master key（初始化后忽略）
```

## 部署建议

- **规格**：2 vCPU / 2GB 内存足以支撑约 10 台服务器、数名并发用户
- **网络**：仅暴露 443（HTTPS）+ 22（管理 SSH）；前置 nginx 或 Caddy 做 TLS
- **systemd**：以非 root 用户运行；`--data-dir` 放在权限受限的路径（其中有 SQLite 库和 `.env`）

## 许可证

MIT
