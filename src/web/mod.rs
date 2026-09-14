use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Json, Path, Query, State},
    http::{header, StatusCode, Uri},
    response::IntoResponse,
    routing::{delete, get, post, put},
    Router,
};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
#[cfg(debug_assertions)]
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::audit;
use crate::audit_api;
use crate::audit_config::{self, Action, Applied, Change, ObjectKind};
use crate::audit_events;
use crate::audit_events::OperationKind;
use crate::audit_ops::{self, OpError};
use crate::auth;
use crate::config::limits;
use crate::crypto;
use crate::extractors::{AdminUser, AuthUser, Caller};
use crate::file_ops;
use crate::key_manager::{self, CreateKeyRequest};
use crate::metrics;
use crate::server_registry::{self, CreateServerRequest};
use crate::ssh_bridge;
use crate::ws_handler;
use crate::AppState;

mod admin;
mod agents;
mod audit_logs;
mod files;
mod git;
mod keys;
mod login;
mod servers;
mod sessions;
mod tools;
mod ui_state;

use admin::*;
use agents::*;
use audit_logs::*;
use files::*;
use git::*;
use keys::*;
use login::*;
use servers::*;
use sessions::*;
use tools::*;
use ui_state::*;

pub use agents::{
    agent_launch_summary, resolve_agent_launch, start_agent_inner, AgentLaunchContext,
    StartAgentRequest,
};
pub use files::ensure_helper;
pub use servers::ImportSshConfigRequest;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct Assets;

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
}

#[allow(dead_code)]
fn err(status: StatusCode, msg: &str) -> impl IntoResponse {
    (
        status,
        Json(ApiError {
            error: msg.to_string(),
        }),
    )
}

/// Extension points allowing downstream distributions to replace selected
/// handlers while reusing the rest of the router unchanged.
#[derive(Default)]
pub struct RouterHooks {
    /// Replaces the default POST /api/sessions/{id}/agent handler.
    pub start_agent: Option<axum::routing::MethodRouter<Arc<AppState>>>,
    /// Additional routes merged under `/api` (same state, same layers).
    pub extra_api: Option<Router<Arc<AppState>>>,
    /// Feature names the distribution provides, served by `GET /api/features`
    /// so the shared frontend can enable the matching screens.
    pub features: Vec<&'static str>,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    build_router_with_hooks(state, RouterHooks::default())
}

pub fn build_router_with_hooks(state: Arc<AppState>, hooks: RouterHooks) -> Router {
    let features = Arc::new(hooks.features.clone());
    let api = Router::new()
        // Liveness/readiness for load balancers and orchestrators.
        .route("/health", get(handle_health))
        .route(
            "/features",
            get(move || {
                let features = features.clone();
                async move { Json(serde_json::json!({ "features": *features })) }
            }),
        )
        // Auth
        .route("/auth/login", post(handle_login))
        .route("/auth/admin-login", post(handle_admin_login))
        .route("/auth/check", get(handle_check))
        .route("/auth/change-password", post(handle_change_password))
        // Admin: user management
        .route("/admin/stats", get(handle_admin_stats))
        .route("/admin/users", get(handle_list_users))
        .route("/admin/users", post(handle_create_user))
        .route("/admin/users/{id}", delete(handle_delete_user))
        .route(
            "/admin/users/{id}/reset-password",
            post(handle_reset_user_password),
        )
        .route("/admin/terminal", post(handle_create_admin_terminal))
        // Servers
        .route("/groups", get(handle_list_groups))
        .route("/servers", get(handle_list_servers))
        .route("/servers", post(handle_create_server))
        .route("/servers/{id}", put(handle_update_server))
        .route("/servers/{id}", delete(handle_delete_server))
        .route("/servers/{id}/test", post(handle_test_server))
        .route("/servers/import-ssh-config", post(handle_import_ssh_config))
        // Keys
        .route("/keys", get(handle_list_keys))
        .route("/keys", post(handle_create_key))
        .route("/keys/{id}", delete(handle_delete_key))
        // Sessions
        .route("/sessions", get(handle_list_sessions))
        .route("/sessions", post(handle_create_session))
        .route("/sessions/{id}", delete(handle_delete_session))
        // Files (SFTP)
        .route("/sessions/{id}/files", get(handle_list_files))
        .route("/sessions/{id}/files/content", get(handle_read_file))
        .route("/sessions/{id}/files/restore", post(handle_restore_file))
        .route(
            "/sessions/{id}/files/content",
            put(handle_write_file).layer(DefaultBodyLimit::max(limits::MAX_FILE_WRITE_BYTES)),
        )
        .route("/sessions/{id}/files", delete(handle_delete_file))
        .route("/sessions/{id}/files/mkdir", post(handle_mkdir))
        .route("/sessions/{id}/files/rename", post(handle_rename_file))
        .route("/sessions/{id}/files/home", get(handle_home_dir))
        .route("/sessions/{id}/files/stat", get(handle_stat_file))
        .route("/sessions/{id}/files/download", get(handle_download_file))
        .route(
            "/sessions/{id}/files/upload",
            post(handle_upload_file).layer(DefaultBodyLimit::max(limits::MAX_UPLOAD_BYTES)),
        )
        // Git
        .route("/sessions/{id}/git/status", get(handle_git_status))
        .route("/sessions/{id}/git/log", get(handle_git_log))
        .route("/sessions/{id}/git/branches", get(handle_git_branches))
        .route("/sessions/{id}/git/diff", get(handle_git_diff))
        // Agent
        .route(
            "/sessions/{id}/agent",
            hooks
                .start_agent
                .unwrap_or_else(|| post(handle_start_agent)),
        )
        .route("/sessions/{id}/agent", delete(handle_stop_agent))
        .route("/sessions/{id}/agent/status", get(handle_agent_status))
        // AI Tools (admin)
        .route("/admin/tools", get(handle_list_tools))
        .route("/admin/tools", post(handle_create_tool))
        .route("/admin/tools/{id}", put(handle_update_tool))
        .route("/admin/tools/{id}", delete(handle_delete_tool))
        // AI Tools (user config)
        .route("/tools", get(handle_list_tools_for_user))
        .route("/tools/{id}/config", get(handle_get_tool_config))
        .route("/tools/{id}/config", put(handle_save_tool_config))
        .route("/tools/{id}/config", delete(handle_delete_tool_config))
        // AI Tools (server-level config)
        .route(
            "/tools/{id}/server-config",
            get(handle_get_server_tool_config),
        )
        .route(
            "/tools/{id}/server-config",
            put(handle_save_server_tool_config),
        )
        .route(
            "/tools/{id}/server-config",
            delete(handle_delete_server_tool_config),
        )
        // Audit
        .route("/audit/logs", get(handle_list_audit_logs))
        .route("/audit/filters", get(handle_audit_filters))
        .route("/audit/sessions", get(audit_api::list_sessions))
        .route("/audit/sessions/{id}", get(audit_api::get_session))
        .route(
            "/audit/sessions/{id}/recordings",
            get(audit_api::list_recordings),
        )
        .route("/audit/operations", get(audit_api::list_operations))
        .route("/audit/operations/{id}", get(audit_api::get_operation))
        .route(
            "/audit/recordings/{id}/events",
            get(audit_api::recording_events),
        )
        .route("/audit/export", get(audit_api::export))
        .route("/admin/metrics", get(handle_metrics))
        .route("/audit/system", get(audit_api::list_system_events))
        .route("/audit/integrity", get(audit_api::integrity))
        // UI State
        .route("/ui-state", get(handle_get_ui_state))
        .route("/ui-state", put(handle_save_ui_state));

    let api = match hooks.extra_api {
        Some(extra) => api.merge(extra),
        None => api,
    };
    let router = Router::new()
        .nest("/api", api)
        .route("/ws/terminal/{session_id}", get(ws_handler::ws_terminal))
        .route("/ws/agent/{agent_id}", get(ws_handler::ws_agent))
        .fallback(static_handler)
        .layer(DefaultBodyLimit::max(limits::MAX_JSON_BODY_BYTES))
        // Every request gets an id (client-supplied `x-request-id` is kept,
        // otherwise a UUID is minted), the id is echoed in the response and
        // is the span field every log line of the request carries. The span
        // records method and path only: query strings and headers can hold
        // tokens and never reach the log.
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state);

    #[cfg(debug_assertions)]
    let router = router.layer(CorsLayer::permissive());

    router
}

fn request_span(req: &axum::http::Request<axum::body::Body>) -> tracing::Span {
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    tracing::info_span!(
        "request",
        id = %request_id,
        method = %req.method(),
        path = %req.uri().path(),
    )
}

/// Unauthenticated health check. Reports whether the database answers, how
/// much is live and which operator warnings are raised (codes only; the
/// numbers behind them are admin-only in `/api/admin/metrics`). Returns 503
/// when the database is unusable so an orchestrator stops routing traffic
/// here; warnings alone keep 200 with `status: "degraded"`.
async fn handle_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db_ok = state
        .db
        .conn()
        .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
        .map(|v| v == 1)
        .unwrap_or(false);
    let warnings = metrics::snapshot(&state).await.warnings;
    let body = serde_json::json!({
        "status": if !db_ok || !warnings.is_empty() { "degraded" } else { "ok" },
        "warnings": warnings,
        "db": if db_ok { "ok" } else { "error" },
        "activeSessions": state.sessions.list_all_session_count(),
        "activeAgents": state.agents.active_count().await,
        "schemaVersion": state.db.schema_version(),
        "audit": {
            "previousShutdownClean": state.startup.previous_shutdown_clean,
            "recoveredAtStartup": {
                "sessions": state.startup.sessions_closed,
                "operations": state.startup.operations_interrupted,
                "recordings": state.startup.recordings_interrupted,
            },
            "recordingEnabled": state.startup.recording_enabled,
        },
    });
    let code = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body))
}

/// Full operational snapshot for administrators: counters, disk headroom,
/// running/stale operations and the warnings derived from them.
async fn handle_metrics(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
) -> impl IntoResponse {
    Json(metrics::snapshot(&state).await)
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                file.data.to_vec(),
            )
                .into_response()
        }
        None => {
            // SPA fallback: serve index.html for client-side routing
            match Assets::get("index.html") {
                Some(index) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/html".to_string())],
                    index.data.to_vec(),
                )
                    .into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
    }
}

// --- Helpers ---

fn extract_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

fn extract_user_id(headers: &axum::http::HeaderMap, jwt_secret: &[u8]) -> Option<String> {
    extract_token(headers).and_then(|t| auth::verify_token(&t, jwt_secret).map(|(uid, _)| uid))
}

/// HTTP status for a managed operation that did not succeed: 503 when the
/// audit intent could not be written (nothing ran), 500 when it ran and failed.
/// Sentinel error for "the object does not exist": recorded as a failed
/// change, answered with 404.
const NOT_FOUND: &str = "not found";

/// Like [`op_error_response`], but a failed configuration change answers
/// with `status` (usually 400 for validation problems, 404 for missing
/// objects) instead of 500.
fn config_error_response(e: OpError, status: StatusCode) -> axum::response::Response {
    match e {
        OpError::Failed(msg) if msg == NOT_FOUND => StatusCode::NOT_FOUND.into_response(),
        OpError::Failed(msg) => (status, Json(ApiError { error: msg })).into_response(),
        refused => op_error_response(refused),
    }
}

/// Answer for a guard decision other than `Proceed`: 403 with the reason,
/// or 202 with the approval request the caller has to wait for.
pub fn guard_response(decision: crate::guard::GuardDecision) -> axum::response::Response {
    use crate::guard::GuardDecision;
    match decision {
        GuardDecision::Proceed => StatusCode::NO_CONTENT.into_response(),
        GuardDecision::Refuse { reason } => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": reason, "decision": "refuse" })),
        )
            .into_response(),
        GuardDecision::AwaitApproval {
            request_id,
            message,
        } => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "error": message,
                "decision": "await_approval",
                "approvalRequired": true,
                "requestId": request_id,
            })),
        )
            .into_response(),
    }
}

/// HTTP answer for a managed-operation error: guard decisions map to 403 /
/// 202, an unrecordable intent to 503, an executor failure to 500.
pub fn op_error_response(e: OpError) -> axum::response::Response {
    match e {
        OpError::Blocked(decision) => guard_response(decision),
        OpError::AuditRefused(msg) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError { error: msg }),
        )
            .into_response(),
        OpError::Failed(msg) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: msg }),
        )
            .into_response(),
    }
}

fn is_admin(headers: &axum::http::HeaderMap, jwt_secret: &[u8]) -> bool {
    extract_token(headers)
        .and_then(|t| auth::verify_token(&t, jwt_secret))
        .map(|(_, role)| role == "admin")
        .unwrap_or(false)
}
