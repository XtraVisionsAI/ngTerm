use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct Database {
    conn: Mutex<Connection>,
}

type MigrationStep = fn(&Connection) -> Result<(), rusqlite::Error>;

/// Ordered, append-only list of schema migrations. Never edit or remove an
/// entry once released; add a new version instead.
const MIGRATIONS: &[(u32, &str, MigrationStep)] = &[
    (1, "baseline schema", Database::migration_v1_baseline),
    (
        2,
        "versioned audit sessions/operations/events",
        crate::audit_events::migration_v2_audit_events,
    ),
];

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

    /// Apply every schema migration newer than the recorded version, each in
    /// its own transaction, then run the idempotent post-migration hooks
    /// (seeding, option canonicalisation). A migration that fails leaves the
    /// database at the previous version; nothing is ever dropped.
    fn migrate(&self) -> Result<(), rusqlite::Error> {
        let mut conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version    INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )?;
        for (version, name, step) in MIGRATIONS {
            if Self::migration_applied(&conn, *version)? {
                continue;
            }
            Self::apply_migration(&mut conn, *version, name, *step)?;
        }

        // Post-migration hooks: idempotent, not versioned, never destructive.
        conn.execute(
            "INSERT OR IGNORE INTO users (id, username, password, role, kdf_salt, encrypted_secret, secret_nonce, created_at) VALUES ('admin', 'admin', '', 'admin', X'00', X'00', X'00', datetime('now'))",
            [],
        ).ok();
        Self::seed_builtin_tools(&conn);
        match crate::ai_tool_registry::migrate_tool_options_conn(&conn) {
            Ok(n) if n > 0 => tracing::info!("Migrated {} AI tool option document(s)", n),
            Ok(_) => {}
            Err(e) => tracing::warn!("AI tool options migration failed: {}", e),
        }
        Ok(())
    }

    fn migration_applied(conn: &Connection, version: u32) -> Result<bool, rusqlite::Error> {
        conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
            rusqlite::params![version],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
    }

    fn apply_migration(
        conn: &mut Connection,
        version: u32,
        name: &str,
        step: MigrationStep,
    ) -> Result<(), rusqlite::Error> {
        let tx = conn.transaction()?;
        step(&tx)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![version, chrono::Utc::now().to_rfc3339()],
        )?;
        tx.commit()?;
        tracing::info!("Applied schema migration {} ({})", version, name);
        Ok(())
    }

    /// Highest applied schema version (0 for a database without the table).
    pub fn schema_version(&self) -> u32 {
        self.conn()
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|v| v as u32)
            .unwrap_or(0)
    }

    /// Baseline schema. Written with IF NOT EXISTS / add-column-if-missing so
    /// it can be recorded as applied on databases that predate versioning
    /// without touching their data.
    fn migration_v1_baseline(conn: &Connection) -> Result<(), rusqlite::Error> {
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
        Self::add_column_if_missing(conn, "servers", "host_key_fingerprint", "TEXT")?;
        Self::add_column_if_missing(conn, "servers", "idle_timeout_secs", "INTEGER DEFAULT 0")?;
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

    fn add_column_if_missing(
        conn: &Connection,
        table: &str,
        column: &str,
        col_type: &str,
    ) -> Result<(), rusqlite::Error> {
        let sql = format!("SELECT {column} FROM {table} LIMIT 0");
        if conn.execute_batch(&sql).is_err() {
            let alter = format!("ALTER TABLE {table} ADD COLUMN {column} {col_type}");
            conn.execute_batch(&alter)?;
        }
        Ok(())
    }

    pub fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ngterm-db-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn migrations_are_recorded_once_and_reopen_is_a_noop() {
        let dir = temp_dir();
        let db = Database::open(dir.to_str().unwrap()).unwrap();
        assert_eq!(db.schema_version(), MIGRATIONS.last().unwrap().0);
        db.conn()
            .execute(
                "INSERT INTO servers (id, alias, host, username, created_at) VALUES ('s1','a','h','u','now')",
                [],
            )
            .unwrap();
        drop(db);

        let db = Database::open(dir.to_str().unwrap()).unwrap();
        assert_eq!(db.schema_version(), MIGRATIONS.last().unwrap().0);
        let rows: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows as usize, MIGRATIONS.len());
        let servers: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM servers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(servers, 1, "data survives reopen");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pre_versioning_database_is_adopted_without_data_loss() {
        let dir = temp_dir();
        // Simulate a database created before schema_migrations existed: old
        // servers table without the later columns, with a row in it.
        {
            let conn = Connection::open(dir.join("onemux.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE servers (id TEXT PRIMARY KEY, group_name TEXT DEFAULT '', alias TEXT NOT NULL, host TEXT NOT NULL, port INTEGER DEFAULT 22, username TEXT NOT NULL, key_id TEXT, tags TEXT DEFAULT '[]', ai_tool_id TEXT, created_at TEXT NOT NULL);
                 INSERT INTO servers (id, alias, host, username, created_at) VALUES ('old','a','h','u','now');",
            )
            .unwrap();
        }
        let db = Database::open(dir.to_str().unwrap()).unwrap();
        assert_eq!(db.schema_version(), MIGRATIONS.last().unwrap().0);
        let (alias, idle): (String, i64) = db
            .conn()
            .query_row(
                "SELECT alias, COALESCE(idle_timeout_secs, 0) FROM servers WHERE id = 'old'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(alias, "a");
        assert_eq!(idle, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_migration_is_rolled_back_and_not_recorded() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )
        .unwrap();
        fn bad(conn: &Connection) -> Result<(), rusqlite::Error> {
            conn.execute_batch("CREATE TABLE half_done (x);")?;
            conn.execute_batch("THIS IS NOT SQL;")
        }
        assert!(Database::apply_migration(&mut conn, 7, "bad", bad).is_err());
        assert!(!Database::migration_applied(&conn, 7).unwrap());
        assert!(
            conn.execute_batch("SELECT * FROM half_done").is_err(),
            "partial work rolled back"
        );
    }
}
