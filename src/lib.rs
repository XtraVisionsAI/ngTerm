pub mod agent_bridge;
pub mod ai_tool_registry;
pub mod audit;
pub mod auth;
pub mod config;
pub mod crypto;
pub mod db;
pub mod extractors;
pub mod helper_pool;
pub mod key_manager;
pub mod local_pty;
pub mod rate_limit;
pub mod scrollback;
pub mod server_registry;
pub mod server_tool_config;
pub mod session_manager;
pub mod sftp_bridge;
pub mod ssh_bridge;
pub mod user_tool_config;
pub mod utils;
pub mod web;
pub mod ws_handler;

use std::sync::Arc;
use tokio::sync::RwLock;

pub struct AppState {
    pub db: db::Database,
    pub sessions: session_manager::SessionManager,
    pub helpers: Arc<helper_pool::HelperPool>,
    pub agents: agent_bridge::AgentBridge,
    pub auth_sessions: RwLock<auth::AuthSessionStore>,
    pub rate_limiter: rate_limit::RateLimiter,
    pub config: config::AppConfig,
    pub admin_terminal_lock: tokio::sync::Mutex<()>,
}

pub async fn build_app_state(
    config: config::AppConfig,
    db: db::Database,
) -> (
    Arc<AppState>,
    tokio::sync::mpsc::UnboundedReceiver<session_manager::SessionEnded>,
) {
    let (session_manager, session_ended_rx) = session_manager::SessionManager::new();

    let mut auth_store = auth::AuthSessionStore::new();
    // Pre-insert admin secret derived from pepper so admin tool configs
    // can be decrypted without requiring a fresh login after server restart
    let admin_secret = crypto::derive_data_key(
        &crypto::derive_data_key(&[0u8; 32], "admin-session-secret"),
        &config.pepper,
    );
    auth_store.insert("admin".to_string(), admin_secret);

    let state = Arc::new(AppState {
        db,
        sessions: session_manager,
        helpers: Arc::new(helper_pool::HelperPool::new()),
        agents: agent_bridge::AgentBridge::new(),
        auth_sessions: RwLock::new(auth_store),
        rate_limiter: rate_limit::RateLimiter::new(),
        config,
        admin_terminal_lock: tokio::sync::Mutex::new(()),
    });

    (state, session_ended_rx)
}
