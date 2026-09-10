use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(data_dir: &str) -> Result<Self, rusqlite::Error> {
        let db_path = Path::new(data_dir).join("onemux.db");
        let conn = Connection::open(db_path)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS users (
                id                  TEXT PRIMARY KEY,
                username            TEXT UNIQUE NOT NULL,
                password            TEXT NOT NULL,
                role                TEXT NOT NULL DEFAULT 'user',
                kdf_salt            BLOB NOT NULL,
                encrypted_secret    BLOB NOT NULL,
                secret_nonce        BLOB NOT NULL,
                created_at          TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS keys (
                id          TEXT PRIMARY KEY,
                user_id     TEXT NOT NULL REFERENCES users(id),
                name        TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                key_type    TEXT NOT NULL,
                encrypted   BLOB NOT NULL,
                nonce       BLOB NOT NULL,
                created_at  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS servers (
                id          TEXT PRIMARY KEY,
                group_name  TEXT DEFAULT '',
                alias       TEXT NOT NULL,
                host        TEXT NOT NULL,
                port        INTEGER DEFAULT 22,
                username    TEXT NOT NULL,
                key_id      TEXT REFERENCES keys(id),
                tags        TEXT DEFAULT '[]',
                ai_tool_id  TEXT,
                created_at  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS audit_logs (
                id              TEXT PRIMARY KEY,
                user_id         TEXT NOT NULL,
                username        TEXT NOT NULL,
                server_id       TEXT NOT NULL,
                server_alias    TEXT NOT NULL,
                server_host     TEXT NOT NULL,
                session_id      TEXT NOT NULL,
                connected_at    TEXT NOT NULL,
                disconnected_at TEXT,
                duration_secs   INTEGER,
                disconnect_reason TEXT DEFAULT 'active'
            );

            CREATE TABLE IF NOT EXISTS ai_tools (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL UNIQUE,
                display_name  TEXT NOT NULL,
                type          TEXT NOT NULL DEFAULT 'external',
                options       TEXT DEFAULT '{}',
                created_at    TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS user_tool_configs (
                id               TEXT PRIMARY KEY,
                user_id          TEXT NOT NULL REFERENCES users(id),
                tool_id          TEXT NOT NULL REFERENCES ai_tools(id),
                config_override  TEXT,
                disabled_keys    TEXT DEFAULT '[]',
                env_values_enc   BLOB,
                env_values_nonce BLOB,
                created_at       TEXT NOT NULL,
                UNIQUE(user_id, tool_id)
            );

            CREATE TABLE IF NOT EXISTS server_tool_configs (
                id                  TEXT PRIMARY KEY,
                user_id             TEXT NOT NULL,
                server_id           TEXT NOT NULL,
                tool_id             TEXT NOT NULL,
                env_overrides_enc   BLOB,
                env_overrides_nonce BLOB,
                disabled_keys       TEXT DEFAULT '[]',
                config_override     TEXT,
                created_at          TEXT NOT NULL,
                UNIQUE(user_id, server_id, tool_id)
            );

            CREATE TABLE IF NOT EXISTS user_ui_state (
                user_id     TEXT PRIMARY KEY REFERENCES users(id),
                state_json  TEXT NOT NULL DEFAULT '{}',
                updated_at  TEXT NOT NULL
            );
",
        )?;

        // Ensure admin placeholder exists for foreign key compatibility
        conn.execute(
            "INSERT OR IGNORE INTO users (id, username, password, role, kdf_salt, encrypted_secret, secret_nonce, created_at) VALUES ('admin', 'admin', '', 'admin', X'00', X'00', X'00', datetime('now'))",
            [],
        ).ok();

        // Seed built-in AI tools
        Self::seed_builtin_tools(&conn);

        // Canonicalise stored tool options (legacy camelCase → snake_case).
        match crate::ai_tool_registry::migrate_tool_options_conn(&conn) {
            Ok(n) if n > 0 => tracing::info!("Migrated {} AI tool option document(s)", n),
            Ok(_) => {}
            Err(e) => tracing::warn!("AI tool options migration failed: {}", e),
        }

        // Incremental migrations: add columns if missing
        Self::add_column_if_missing(&conn, "servers", "host_key_fingerprint", "TEXT");
        Self::add_column_if_missing(&conn, "servers", "idle_timeout_secs", "INTEGER DEFAULT 0");

        Ok(())
    }

    fn seed_builtin_tools(conn: &Connection) {
        struct BuiltinTool {
            name: &'static str,
            display_name: &'static str,
            tool_type: &'static str,
            options: &'static str,
        }

        let builtins = [BuiltinTool {
            name: "claude-code",
            display_name: "Claude Code",
            tool_type: "external",
            options: r#"{
  "external": {
    "detect_cmd": "command -v claude",
    "install_cmd": "curl -fsSL https://claude.ai/install.sh | bash",
    "launch_cmd": "claude -p --output-format stream-json --input-format stream-json --verbose --dangerously-skip-permissions",
    "config_tpl": "{}",
    "config_path": null
  },
  "params": [
    {"key": "ANTHROPIC_API_KEY", "label": "API Key", "required": true, "secret": true, "usage": "env"},
    {"key": "ANTHROPIC_BASE_URL", "label": "Base URL", "required": false, "secret": false, "default": "https://api.anthropic.com", "usage": "env"},
    {"key": "CLAUDE_MODEL", "label": "Model", "required": false, "secret": false, "usage": "env"}
  ],
  "execution": {
    "target": "chat",
    "force_approval_above": "high"
  }
}"#,
        }];

        for tool in &builtins {
            Self::seed_tool_if_missing_conn(
                conn,
                tool.name,
                tool.display_name,
                tool.tool_type,
                tool.options,
            );
        }
    }

    /// Insert an AI tool definition unless one with the same name exists.
    /// Used for built-in seeding and by downstream distributions to register
    /// their own bundled tools.
    pub fn seed_tool_if_missing(
        &self,
        name: &str,
        display_name: &str,
        tool_type: &str,
        options: &str,
    ) {
        Self::seed_tool_if_missing_conn(&self.conn(), name, display_name, tool_type, options);
    }

    fn seed_tool_if_missing_conn(
        conn: &Connection,
        name: &str,
        display_name: &str,
        tool_type: &str,
        options: &str,
    ) {
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM ai_tools WHERE name = ?1",
                rusqlite::params![name],
                |r| r.get(0),
            )
            .unwrap_or(false);

        if !exists {
            let id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO ai_tools (id, name, display_name, type, options, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![id, name, display_name, tool_type, options, now],
            ).ok();
        }
    }

    fn add_column_if_missing(conn: &Connection, table: &str, column: &str, col_type: &str) {
        let sql = format!("SELECT {column} FROM {table} LIMIT 0");
        if conn.execute_batch(&sql).is_err() {
            let alter = format!("ALTER TABLE {table} ADD COLUMN {column} {col_type}");
            conn.execute_batch(&alter).ok();
        }
    }

    pub fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }
}
