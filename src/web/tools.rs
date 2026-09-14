//! AI tool registry (admin) and per-user / per-server tool configuration.

use super::*;

// --- AI Tool handlers (admin) ---

pub(super) async fn handle_list_tools(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
) -> impl IntoResponse {
    match crate::ai_tool_registry::list_tools(&state.db) {
        Ok(tools) => Json(tools).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

pub(super) async fn handle_create_tool(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
    caller: Caller,
    Json(req): Json<crate::ai_tool_registry::CreateAiToolRequest>,
) -> impl IntoResponse {
    let change = Change::new(
        ObjectKind::AiTool,
        Action::Create,
        format!("create ai tool {} ({})", req.name, req.tool_type),
    );
    let mut registry_err = None;
    let result = audit_config::run(&state, &caller, change, async {
        match crate::ai_tool_registry::create_tool(&state.db, &req) {
            Ok(tool) => Ok(Applied::new(tool.clone())
                .object_id(tool.id.clone())
                .after(Some(audit_config::tool_snapshot(&tool)))),
            Err(e) => {
                let msg = e.to_string();
                registry_err = Some(e);
                Err(msg)
            }
        }
    })
    .await;
    match (result, registry_err) {
        (Ok(tool), _) => (StatusCode::CREATED, Json(tool)).into_response(),
        (Err(OpError::Failed(_)), Some(e)) => registry_error_response(e),
        (Err(e), _) => config_error_response(e, StatusCode::BAD_REQUEST),
    }
}

/// Map registry errors to HTTP: validation problems carry field-level
/// details so the admin form can highlight the offending inputs.
pub(super) fn registry_error_response(
    e: crate::ai_tool_registry::RegistryError,
) -> axum::response::Response {
    use crate::ai_tool_registry::RegistryError;
    match e {
        RegistryError::Validation(fields) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": RegistryError::Validation(fields.clone()).to_string(),
                "fields": fields,
            })),
        )
            .into_response(),
        RegistryError::NotFound => StatusCode::NOT_FOUND.into_response(),
        RegistryError::Db(msg) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: msg }),
        )
            .into_response(),
    }
}

pub(super) async fn handle_update_tool(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    _admin: AdminUser,
    caller: Caller,
    Json(req): Json<crate::ai_tool_registry::UpdateAiToolRequest>,
) -> impl IntoResponse {
    let existing = crate::ai_tool_registry::get_tool(&state.db, &id)
        .ok()
        .flatten();
    let name = existing
        .as_ref()
        .map(|t| t.name.clone())
        .unwrap_or_else(|| id.clone());
    let change = Change::new(
        ObjectKind::AiTool,
        Action::Update,
        format!("update ai tool {}", name),
    )
    .object_id(&id)
    .before(existing.as_ref().map(audit_config::tool_snapshot));
    let mut registry_err = None;
    let result = audit_config::run(&state, &caller, change, async {
        match crate::ai_tool_registry::update_tool(&state.db, &id, &req) {
            Ok(true) => {
                let after = crate::ai_tool_registry::get_tool(&state.db, &id)
                    .ok()
                    .flatten()
                    .map(|t| audit_config::tool_snapshot(&t));
                Ok(Applied::new(()).after(after))
            }
            Ok(false) => Err(NOT_FOUND.to_string()),
            Err(e) => {
                let msg = e.to_string();
                registry_err = Some(e);
                Err(msg)
            }
        }
    })
    .await;
    match (result, registry_err) {
        (Ok(()), _) => StatusCode::NO_CONTENT.into_response(),
        (Err(OpError::Failed(_)), Some(e)) => registry_error_response(e),
        (Err(e), _) => config_error_response(e, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub(super) async fn handle_delete_tool(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    _admin: AdminUser,
    caller: Caller,
) -> impl IntoResponse {
    let existing = crate::ai_tool_registry::get_tool(&state.db, &id)
        .ok()
        .flatten();
    let name = existing
        .as_ref()
        .map(|t| t.name.clone())
        .unwrap_or_else(|| id.clone());
    let change = Change::new(
        ObjectKind::AiTool,
        Action::Delete,
        format!("delete ai tool {}", name),
    )
    .object_id(&id)
    .before(existing.as_ref().map(audit_config::tool_snapshot));
    let result = audit_config::run(&state, &caller, change, async {
        match crate::ai_tool_registry::delete_tool(&state.db, &id) {
            Ok(true) => Ok(Applied::new(())),
            Ok(false) => Err(NOT_FOUND.to_string()),
            Err(e) => Err(e),
        }
    })
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => config_error_response(e, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

// --- AI Tool config handlers (user) ---

pub(super) async fn handle_list_tools_for_user(
    State(state): State<Arc<AppState>>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    let user_secret = state.auth_sessions.read().await.get_user_secret(&user_id);

    let tools = match crate::ai_tool_registry::list_tools(&state.db) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    };
    let configs = crate::user_tool_config::list_configs(&state.db, &user_id).unwrap_or_default();
    let configured_tool_ids: std::collections::HashSet<String> =
        configs.iter().map(|c| c.tool_id.clone()).collect();

    let result: Vec<serde_json::Value> = tools
        .into_iter()
        .map(|t| {
            let tool_id = t.id.clone();
            let configured = configured_tool_ids.contains(&tool_id);
            let mut val = serde_json::to_value(&t).unwrap_or_default();
            val.as_object_mut().unwrap().insert(
                "configured".to_string(),
                serde_json::Value::Bool(configured),
            );

            // Mask default values of secret params
            if let Some(params_arr) = val
                .get_mut("options")
                .and_then(|o| o.get_mut("params"))
                .and_then(|v| v.as_array_mut())
            {
                for param_def in params_arr.iter_mut() {
                    let is_secret = param_def
                        .get("secret")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                        || param_def
                            .get("key")
                            .and_then(|v| v.as_str())
                            .is_some_and(is_secret_key_name);
                    if is_secret {
                        if let Some(obj) = param_def.as_object_mut() {
                            if obj
                                .get("default")
                                .and_then(|v| v.as_str())
                                .is_some_and(|s| !s.is_empty())
                            {
                                obj.insert(
                                    "default".to_string(),
                                    serde_json::Value::String("***".to_string()),
                                );
                            }
                        }
                    }
                }
            }

            // Include user's configured param values (secrets masked)
            if let Some(ref secret) = user_secret {
                if let Ok(env) = crate::user_tool_config::get_decrypted_env(
                    &state.db, &user_id, secret, &tool_id,
                ) {
                    let tool_params = t.params();
                    let secret_keys: std::collections::HashSet<&str> = tool_params
                        .iter()
                        .filter(|k| k.secret)
                        .map(|k| k.key.as_str())
                        .collect();
                    let masked: std::collections::HashMap<&String, String> = env
                        .iter()
                        .map(|(k, v)| {
                            let is_secret =
                                secret_keys.contains(k.as_str()) || is_secret_key_name(k);
                            let display = if is_secret {
                                "***".to_string()
                            } else {
                                v.clone()
                            };
                            (k, display)
                        })
                        .collect();
                    val.as_object_mut().unwrap().insert(
                        "configuredValues".to_string(),
                        serde_json::to_value(&masked).unwrap(),
                    );
                }
            }

            // Include user's execution preferences from config_override
            if let Some(cfg) = configs.iter().find(|c| c.tool_id == tool_id) {
                if let Some(ref override_str) = cfg.config_override {
                    if let Ok(prefs) = serde_json::from_str::<serde_json::Value>(override_str) {
                        val.as_object_mut()
                            .unwrap()
                            .insert("userPrefs".to_string(), prefs);
                    }
                }
            }

            val
        })
        .collect();

    Json(result).into_response()
}

pub(super) async fn handle_get_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    match crate::user_tool_config::get_config(&state.db, &user_id, &tool_id) {
        Ok(Some(cfg)) => {
            let env_values = crate::user_tool_config::get_decrypted_env(
                &state.db,
                &user_id,
                &user_secret,
                &tool_id,
            )
            .unwrap_or_default();

            let secret_keys: std::collections::HashSet<String> =
                crate::ai_tool_registry::get_tool(&state.db, &tool_id)
                    .ok()
                    .flatten()
                    .map(|t| {
                        t.params()
                            .iter()
                            .filter(|k| k.secret)
                            .map(|k| k.key.clone())
                            .collect()
                    })
                    .unwrap_or_default();

            let masked: std::collections::HashMap<&String, String> = env_values
                .iter()
                .map(|(k, v)| {
                    let is_secret = secret_keys.contains(k) || is_secret_key_name(k);
                    let display = if is_secret {
                        "***".to_string()
                    } else {
                        v.clone()
                    };
                    (k, display)
                })
                .collect();

            let mut response = serde_json::to_value(&cfg).unwrap();
            response["envValues"] = serde_json::to_value(&masked).unwrap();
            Json(response).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

pub(super) async fn handle_save_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    caller: Caller,
    Json(req): Json<crate::user_tool_config::SaveToolConfigRequest>,
) -> impl IntoResponse {
    let user_id = caller.user_id.clone();
    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let existing = crate::user_tool_config::get_config(&state.db, &user_id, &tool_id)
        .ok()
        .flatten();
    let env_keys = audit_config::env_key_names(req.env_values.as_ref());
    let change = Change::new(
        ObjectKind::UserToolConfig,
        if existing.is_some() {
            Action::Update
        } else {
            Action::Create
        },
        format!("save own config for tool {}", tool_id),
    )
    .before(
        existing
            .as_ref()
            .map(|c| audit_config::user_tool_config_snapshot(c, None)),
    );
    let result = audit_config::run(&state, &caller, change, async {
        let cfg = crate::user_tool_config::save_config(
            &state.db,
            &user_id,
            &user_secret,
            &tool_id,
            &req,
        )?;
        Ok(Applied::new(cfg.clone())
            .object_id(cfg.id.clone())
            .after(Some(audit_config::user_tool_config_snapshot(
                &cfg, env_keys,
            ))))
    })
    .await;
    match result {
        Ok(cfg) => Json(cfg).into_response(),
        Err(e) => config_error_response(e, StatusCode::BAD_REQUEST),
    }
}

pub(super) async fn handle_delete_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    caller: Caller,
) -> impl IntoResponse {
    let user_id = caller.user_id.clone();
    let existing = crate::user_tool_config::get_config(&state.db, &user_id, &tool_id)
        .ok()
        .flatten();
    let mut change = Change::new(
        ObjectKind::UserToolConfig,
        Action::Delete,
        format!("delete own config for tool {}", tool_id),
    )
    .before(
        existing
            .as_ref()
            .map(|c| audit_config::user_tool_config_snapshot(c, None)),
    );
    if let Some(c) = &existing {
        change = change.object_id(&c.id);
    }
    let result = audit_config::run(&state, &caller, change, async {
        match crate::user_tool_config::delete_config(&state.db, &user_id, &tool_id) {
            Ok(true) => Ok(Applied::new(())),
            Ok(false) => Err(NOT_FOUND.to_string()),
            Err(e) => Err(e),
        }
    })
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => config_error_response(e, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ServerConfigQuery {
    server_id: String,
}

pub(super) async fn handle_get_server_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    Query(query): Query<ServerConfigQuery>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    match crate::server_tool_config::get_config(&state.db, &user_id, &query.server_id, &tool_id) {
        Ok(Some(cfg)) => {
            let env_overrides = crate::server_tool_config::get_decrypted_env(
                &state.db,
                &user_id,
                &user_secret,
                &query.server_id,
                &tool_id,
            )
            .unwrap_or_default();

            let secret_keys: std::collections::HashSet<String> =
                crate::ai_tool_registry::get_tool(&state.db, &tool_id)
                    .ok()
                    .flatten()
                    .map(|t| {
                        t.params()
                            .iter()
                            .filter(|k| k.secret)
                            .map(|k| k.key.clone())
                            .collect()
                    })
                    .unwrap_or_default();

            let masked: std::collections::HashMap<&String, String> = env_overrides
                .iter()
                .map(|(k, v)| {
                    let is_secret = secret_keys.contains(k) || is_secret_key_name(k);
                    let display = if is_secret {
                        "***".to_string()
                    } else {
                        v.clone()
                    };
                    (k, display)
                })
                .collect();

            let mut response = serde_json::to_value(&cfg).unwrap();
            response["envOverrides"] = serde_json::to_value(&masked).unwrap();
            Json(response).into_response()
        }
        Ok(None) => Json(serde_json::json!(null)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response(),
    }
}

pub(super) async fn handle_save_server_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    caller: Caller,
    Json(req): Json<crate::server_tool_config::SaveServerToolConfigRequest>,
) -> impl IntoResponse {
    let user_id = caller.user_id.clone();
    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let existing =
        crate::server_tool_config::get_config(&state.db, &user_id, &req.server_id, &tool_id)
            .ok()
            .flatten();
    let env_keys = audit_config::env_key_names(req.env_overrides.as_ref());
    let change = Change::new(
        ObjectKind::ServerToolConfig,
        if existing.is_some() {
            Action::Update
        } else {
            Action::Create
        },
        format!(
            "save server config for tool {} on server {}",
            tool_id, req.server_id
        ),
    )
    .target(audit_events::Target {
        server_id: Some(req.server_id.clone()),
        ..Default::default()
    })
    .before(
        existing
            .as_ref()
            .map(|c| audit_config::server_tool_config_snapshot(c, None)),
    );
    let result = audit_config::run(&state, &caller, change, async {
        let cfg = crate::server_tool_config::save_config(
            &state.db,
            &user_id,
            &user_secret,
            &tool_id,
            &req,
        )?;
        Ok(Applied::new(cfg.clone())
            .object_id(cfg.id.clone())
            .after(Some(audit_config::server_tool_config_snapshot(
                &cfg, env_keys,
            ))))
    })
    .await;
    match result {
        Ok(cfg) => Json(cfg).into_response(),
        Err(e) => config_error_response(e, StatusCode::BAD_REQUEST),
    }
}

pub(super) async fn handle_delete_server_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    Query(query): Query<ServerConfigQuery>,
    caller: Caller,
) -> impl IntoResponse {
    let user_id = caller.user_id.clone();
    let existing =
        crate::server_tool_config::get_config(&state.db, &user_id, &query.server_id, &tool_id)
            .ok()
            .flatten();
    let mut change = Change::new(
        ObjectKind::ServerToolConfig,
        Action::Delete,
        format!(
            "delete server config for tool {} on server {}",
            tool_id, query.server_id
        ),
    )
    .target(audit_events::Target {
        server_id: Some(query.server_id.clone()),
        ..Default::default()
    })
    .before(
        existing
            .as_ref()
            .map(|c| audit_config::server_tool_config_snapshot(c, None)),
    );
    if let Some(c) = &existing {
        change = change.object_id(&c.id);
    }
    let result = audit_config::run(&state, &caller, change, async {
        match crate::server_tool_config::delete_config(
            &state.db,
            &user_id,
            &query.server_id,
            &tool_id,
        ) {
            Ok(true) => Ok(Applied::new(())),
            Ok(false) => Err(NOT_FOUND.to_string()),
            Err(e) => Err(e),
        }
    })
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => config_error_response(e, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub(super) fn is_secret_key_name(key: &str) -> bool {
    let upper = key.to_uppercase();
    upper.contains("KEY")
        || upper.contains("SECRET")
        || upper.contains("TOKEN")
        || upper.contains("PASSWORD")
}
