pub mod agent_bridge;
pub mod ai_tool_registry;
pub mod audit;
pub mod audit_api;
pub mod audit_events;
pub mod audit_ops;
pub mod auth;
pub mod config;
pub mod crypto;
pub mod db;
pub mod extractors;
pub mod helper_pool;
pub mod key_manager;
pub mod local_pty;
pub mod rate_limit;
pub mod recording;
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
    /// `None` when recording is disabled or its storage could not be opened.
    pub recordings: Option<Arc<recording::RecordingStore>>,
    pub admin_terminal_lock: tokio::sync::Mutex<()>,
}

pub async fn build_app_state(
    config: config::AppConfig,
    db: db::Database,
) -> (
    Arc<AppState>,
    tokio::sync::mpsc::UnboundedReceiver<session_manager::SessionEnded>,
) {
    let recordings = if config.recording.enabled {
        match recording::RecordingStore::new(db.clone(), config.recording.clone()) {
            Ok(store) => {
                if let Err(e) = store.recover() {
                    tracing::error!("Recording recovery failed: {}", e);
                }
                store
                    .clone()
                    .spawn_retention_task(std::time::Duration::from_secs(3600));
                Some(store)
            }
            Err(e) => {
                tracing::error!("Terminal recording disabled: {}", e);
                None
            }
        }
    } else {
        tracing::warn!("Terminal recording is disabled by configuration");
        None
    };
    let (session_manager, session_ended_rx) =
        session_manager::SessionManager::new(recordings.clone());

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
        recordings,
        admin_terminal_lock: tokio::sync::Mutex::new(()),
    });

    (state, session_ended_rx)
}

/// Release per-session resources when a session's fan-out task ends. This is
/// the single place helper connections are dropped and the audit row is
/// closed, so every exit path (SSH closed, user closed, idle, input closed)
/// is accounted for exactly once.
pub fn spawn_session_reaper(
    state: Arc<AppState>,
    mut session_ended_rx: tokio::sync::mpsc::UnboundedReceiver<session_manager::SessionEnded>,
) {
    tokio::spawn(async move {
        while let Some(ended) = session_ended_rx.recv().await {
            // An agent bound to this terminal cannot outlive it.
            state
                .agents
                .stop(&format!("agent-{}", ended.session_id))
                .await;
            state.helpers.remove(&ended.session_id).await;
            if let Err(e) = audit::log_disconnect(&state.db, &ended.session_id, &ended.reason) {
                tracing::error!("Failed to write disconnect audit log: {}", e);
            }
            let integrity = ended
                .recording
                .clone()
                .unwrap_or(audit_events::Integrity::Truncated {
                    reason: "session was not recorded".into(),
                });
            if let Err(e) =
                audit_events::session_ended(&state.db, &ended.session_id, &ended.reason, integrity)
            {
                tracing::error!("Failed to close audit session record: {}", e);
            }
        }
    });
}

/// Resolves on SIGINT or, on unix, SIGTERM.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

/// How long shutdown waits for sessions to report their end before the
/// remaining audit rows are closed administratively.
pub const SHUTDOWN_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Serve until a shutdown signal arrives, then close every live session,
/// give the reaper a bounded time to release resources and write audit
/// rows, and finally mark whatever is still open as `server_shutdown` so no
/// session is left looking active after the process exits.
pub async fn run_server(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    state: Arc<AppState>,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    tracing::info!("Shutdown requested; closing sessions");
    shutdown_state(&state).await;
    Ok(())
}

/// The shutdown sequence, separated from signal handling so it can be
/// exercised in tests.
pub async fn shutdown_state(state: &Arc<AppState>) {
    let requested = state
        .sessions
        .close_all(session_manager::REASON_SERVER_SHUTDOWN);
    let deadline = tokio::time::Instant::now() + SHUTDOWN_DRAIN_TIMEOUT;
    while state.sessions.list_all_session_count() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // Let the reaper process the SessionEnded events that were just emitted.
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let leftover = state.sessions.list_all_session_count();
    match audit::close_open_sessions(&state.db, session_manager::REASON_SERVER_SHUTDOWN) {
        Ok(n) if n > 0 => tracing::warn!(
            "{} audit row(s) were still open at shutdown and have been closed",
            n
        ),
        Err(e) => tracing::error!("Failed to close audit rows at shutdown: {}", e),
        _ => {}
    }
    tracing::info!(
        "Shutdown complete: {} session(s) asked to close, {} did not confirm in time",
        requested,
        leftover
    );
}
