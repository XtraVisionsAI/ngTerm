//! Agent launch plumbing shared with the enterprise router hooks, and the CLI agent handlers.

use super::*;

// --- Agent handlers ---

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartAgentRequest {
    pub prompt: String,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub require_approval: bool,
    #[serde(default)]
    pub tool_id: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    /// User-chosen approval level: `none`, `low`, `medium`, `high`,
    /// `critical` or `all`. It can only tighten the administrator's policy.
    #[serde(default)]
    pub approval_level: Option<String>,
}

/// Everything resolved from the request before an agent is launched:
/// ownership-checked identity, the tool definition, and the merged
/// (admin defaults + user + server overrides) parameter values.
pub struct AgentLaunchContext {
    pub user_id: String,
    pub server_id: String,
    pub tool: crate::ai_tool_registry::AiTool,
    pub params: Vec<crate::ai_tool_registry::ParamDef>,
    pub param_values: std::collections::HashMap<String, String>,
    pub exec_opts: crate::ai_tool_registry::ExecutionOptions,
}

/// Auth + tool loading + three-level param merge shared by all agent
/// start handlers. Returns a ready-to-use response on failure.
pub async fn resolve_agent_launch(
    state: &Arc<AppState>,
    headers: &axum::http::HeaderMap,
    session_id: &str,
    tool_id: &str,
) -> Result<AgentLaunchContext, axum::response::Response> {
    let user_id = match extract_user_id(headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return Err(StatusCode::UNAUTHORIZED.into_response()),
    };
    if !state.sessions.is_owner(session_id, &user_id) {
        return Err(StatusCode::FORBIDDEN.into_response());
    }

    let server_id = match state.sessions.get_server_id(session_id) {
        Some(id) => id,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: "Session not found".into(),
                }),
            )
                .into_response())
        }
    };

    // Get user_secret for tool config (needed for env var decryption)
    let user_secret_opt = state.auth_sessions.read().await.get_user_secret(&user_id);

    let tool = match crate::ai_tool_registry::get_tool(&state.db, tool_id) {
        Ok(Some(t)) => t,
        _ => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: "Tool not found".into(),
                }),
            )
                .into_response())
        }
    };

    // Load user param values (decrypted from user_tool_configs)
    let mut param_values = if let Some(ref us) = user_secret_opt {
        crate::user_tool_config::get_decrypted_env(&state.db, &user_id, us, tool_id)
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    // Fill in tool definition defaults for params not already set
    let params = tool.params();
    for param in &params {
        if !param_values.contains_key(&param.key) {
            if let Some(ref default_val) = param.default {
                if !default_val.is_empty() {
                    param_values.insert(param.key.clone(), default_val.clone());
                }
            }
        }
    }

    // Apply per-user disabled keys
    if let Ok(Some(ref user_cfg)) =
        crate::user_tool_config::get_config(&state.db, &user_id, tool_id)
    {
        for key in &user_cfg.disabled_keys {
            param_values.remove(key);
        }
    }

    // Apply per-server overrides
    if let (Some(ref us), Ok(Some(server_cfg))) = (
        &user_secret_opt,
        crate::server_tool_config::get_config(&state.db, &user_id, &server_id, tool_id),
    ) {
        for key in &server_cfg.disabled_keys {
            param_values.remove(key);
        }
        if let Ok(overrides) = crate::server_tool_config::get_decrypted_env(
            &state.db, &user_id, us, &server_id, tool_id,
        ) {
            param_values.extend(overrides);
        }
    }

    let exec_opts = match tool.execution_options() {
        Ok(opts) => opts,
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    error: format!("Tool policy configuration is invalid: {}", e),
                }),
            )
                .into_response())
        }
    };

    Ok(AgentLaunchContext {
        user_id,
        server_id,
        tool,
        params,
        param_values,
        exec_opts,
    })
}

/// Launch an external CLI agent tool (e.g. Claude Code) for the session.
pub async fn start_agent_inner(
    state: &Arc<AppState>,
    session_id: &str,
    ctx: &AgentLaunchContext,
    body: &StartAgentRequest,
) -> axum::response::Response {
    let tool_id = &ctx.tool.id;
    let ext = match ctx.tool.external_options() {
        Ok(ext) => ext,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    error: format!("Tool configuration is invalid: {}", e),
                }),
            )
                .into_response()
        }
    };
    if ext.launch_cmd.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "Tool has no launch command configured".into(),
            }),
        )
            .into_response();
    }

    // External CLIs run their own permission model. Only tools that declare
    // `supports_approval` get platform-driven flags; for the rest the UI must
    // not promise per-operation approval and we pass the command unchanged.
    let mut launch_cmd = ext.launch_cmd.clone();
    if ext.supports_approval && !body.require_approval {
        launch_cmd.push_str(" --permission-mode acceptEdits");
    }

    // Collect usage=env params as environment variables
    let env_vars: std::collections::HashMap<String, String> = ctx
        .params
        .iter()
        .filter(|p| p.usage == "env")
        .filter_map(|p| {
            ctx.param_values
                .get(&p.key)
                .map(|v| (p.key.clone(), v.clone()))
        })
        .collect();

    // Build config from config_tpl + user/server overrides
    let user_config_override =
        crate::user_tool_config::get_config(&state.db, &ctx.user_id, tool_id)
            .ok()
            .flatten()
            .and_then(|c| c.config_override);

    let server_config_override =
        crate::server_tool_config::get_config(&state.db, &ctx.user_id, &ctx.server_id, tool_id)
            .ok()
            .flatten()
            .and_then(|c| c.config_override);

    let config_tpl = ext.config_tpl.as_deref().unwrap_or("{}");
    let config =
        if user_config_override.is_some() || server_config_override.is_some() || config_tpl != "{}"
        {
            let merged = merge_json_config(config_tpl, user_config_override.as_deref());
            Some(merge_json_config(
                &merged,
                server_config_override.as_deref(),
            ))
        } else {
            None
        };

    let tool_context = crate::agent_bridge::AgentToolContext {
        detect_cmd: ext.detect_cmd,
        install_cmd: ext.install_cmd,
        config,
        config_path: ext.config_path,
        launch_cmd,
        env_vars,
    };

    let agent_id = format!("agent-{}", session_id);

    // The launch itself is an operation. External CLIs run their own tools
    // inside the PTY; the platform cannot observe those individually, so the
    // record states the capability declaration instead of pretending to.
    let launch = audit_ops::begin(
        state,
        session_id,
        &ctx.user_id,
        OperationKind::AgentLaunch,
        agent_launch_summary(&ctx.tool.name, ext.supports_approval, body.require_approval),
        body.working_dir
            .as_deref()
            .filter(|w| !w.trim().is_empty())
            .map(|w| w.to_string()),
    );
    let launch = match launch {
        Ok(op) => op,
        Err(e) => {
            return (StatusCode::SERVICE_UNAVAILABLE, Json(ApiError { error: e })).into_response()
        }
    };

    match state
        .agents
        .start(
            &agent_id,
            session_id,
            &ctx.user_id,
            &state.helpers,
            &tool_context,
            &body.prompt,
            body.working_dir.as_deref(),
        )
        .await
    {
        Ok(()) => {
            launch.succeeded();
            Json(serde_json::json!({
                "agentId": agent_id,
                "supportsApproval": ext.supports_approval,
            }))
            .into_response()
        }
        Err(e) => {
            launch.failed(&e);
            tracing::error!("Agent start failed [{}]: {}", agent_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    }
}

/// Summary of an external CLI agent launch: what was started and what the
/// platform can and cannot see of it.
pub fn agent_launch_summary(
    tool_name: &str,
    supports_approval: bool,
    require_approval: bool,
) -> String {
    let approval = match (supports_approval, require_approval) {
        (true, true) => "per-operation approval requested from the CLI",
        (true, false) => "CLI approval prompts relaxed (acceptEdits)",
        (false, _) => "CLI uses its own permission model; platform cannot enforce approval",
    };
    format!(
        "launch external CLI agent \"{}\" ({}); its internal tool calls are not observed individually, only the terminal recording",
        tool_name, approval
    )
}

pub(super) async fn handle_start_agent(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<StartAgentRequest>,
) -> impl IntoResponse {
    let Some(ref tool_id) = body.tool_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "tool_id is required".into(),
            }),
        )
            .into_response();
    };

    let ctx = match resolve_agent_launch(&state, &headers, &session_id, tool_id).await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };

    if ctx.tool.tool_type == "native" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "Native engine tools require NGTerm EE".into(),
            }),
        )
            .into_response();
    }

    start_agent_inner(&state, &session_id, &ctx, &body).await
}

pub(super) fn merge_json_config(base: &str, override_json: Option<&str>) -> String {
    let mut base_val: serde_json::Value =
        serde_json::from_str(base).unwrap_or(serde_json::Value::Object(Default::default()));
    if let Some(ovr) = override_json {
        if let Ok(ovr_val) = serde_json::from_str::<serde_json::Value>(ovr) {
            if let (Some(b), Some(o)) = (base_val.as_object_mut(), ovr_val.as_object()) {
                for (k, v) in o {
                    b.insert(k.clone(), v.clone());
                }
            }
        }
    }
    serde_json::to_string_pretty(&base_val).unwrap_or_else(|_| "{}".to_string())
}

pub(super) async fn handle_stop_agent(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let agent_id = format!("agent-{}", session_id);
    state.agents.stop(&agent_id).await;
    state.helpers.remove_user(&session_id).await;
    StatusCode::NO_CONTENT.into_response()
}

pub(super) async fn handle_agent_status(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let agent_id = format!("agent-{}", session_id);
    let active = state.agents.is_active(&agent_id).await;
    Json(serde_json::json!({ "active": active, "agentId": agent_id })).into_response()
}
