use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;

use crate::agent_bridge::AgentEvent;
use crate::auth;
use crate::session_manager::{SessionEvent, SessionInput};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type")]
enum ClientMsg {
    #[serde(rename = "input")]
    Input { data: String },
    #[serde(rename = "resize")]
    Resize { cols: u16, rows: u16 },
}

pub async fn ws_terminal(
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    Query(query): Query<WsQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let user_id = match query
        .token
        .as_ref()
        .and_then(|t| auth::verify_token(t, &state.config.jwt_secret))
    {
        Some((uid, _role)) => uid,
        None => {
            return Response::builder()
                .status(401)
                .body(axum::body::Body::from("Unauthorized"))
                .unwrap();
        }
    };

    if !state.sessions.is_owner(&session_id, &user_id) {
        return Response::builder()
            .status(403)
            .body(axum::body::Body::from("Forbidden"))
            .unwrap();
    }

    ws.on_upgrade(move |socket| handle_ws(socket, session_id, state))
}

async fn handle_ws(socket: WebSocket, session_id: String, state: Arc<AppState>) {
    let mut event_rx = match state.sessions.subscribe(&session_id) {
        Some(rx) => rx,
        None => return,
    };

    let input_tx = match state.sessions.input_tx(&session_id) {
        Some(tx) => tx,
        None => return,
    };

    let (mut ws_sink, mut ws_stream) = socket.split();

    state.sessions.ws_connected(&session_id);

    // Replay scrollback
    let scrollback = state.sessions.get_scrollback(&session_id);
    for data in scrollback {
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
        let msg = serde_json::json!({"type": "output", "data": b64});
        if ws_sink
            .send(Message::Text(msg.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }

    loop {
        tokio::select! {
            result = event_rx.recv() => {
                match result {
                    Ok(SessionEvent::Data(bytes)) => {
                        let b64 = base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD, &bytes);
                        let msg = serde_json::json!({"type": "output", "data": b64});
                        if ws_sink.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Ok(SessionEvent::Disconnected) => {
                        let msg = serde_json::json!({"type": "disconnected"});
                        let _ = ws_sink.send(Message::Text(msg.to_string().into())).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WS client lagged {} messages for session {}, resending scrollback", n, session_id);
                        let scrollback = state.sessions.get_scrollback(&session_id);
                        for data in scrollback {
                            let b64 = base64::Engine::encode(
                                &base64::engine::general_purpose::STANDARD, &data);
                            let msg = serde_json::json!({"type": "reset"});
                            let _ = ws_sink.send(Message::Text(msg.to_string().into())).await;
                            let msg = serde_json::json!({"type": "output", "data": b64});
                            if ws_sink.send(Message::Text(msg.to_string().into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let msg = serde_json::json!({"type": "disconnected"});
                        let _ = ws_sink.send(Message::Text(msg.to_string().into())).await;
                        break;
                    }
                }
            }
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(client_msg) = serde_json::from_str::<ClientMsg>(&text) {
                            match client_msg {
                                ClientMsg::Input { data } => {
                                    if let Ok(bytes) = base64::Engine::decode(
                                        &base64::engine::general_purpose::STANDARD, &data) {
                                        state.sessions.touch_activity(&session_id);
                                        let _ = input_tx.send(SessionInput::Data(bytes)).await;
                                    }
                                }
                                ClientMsg::Resize { cols, rows } => {
                                    let cols = cols.clamp(1, 500);
                                    let rows = rows.clamp(1, 200);
                                    let _ = input_tx.send(SessionInput::Resize(cols, rows)).await;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
    state.sessions.ws_disconnected(&session_id);
}

// --- Agent WebSocket ---

#[derive(serde::Deserialize)]
#[serde(tag = "type")]
enum AgentClientMsg {
    #[serde(rename = "message")]
    Message { content: String },
    #[serde(rename = "permission_response")]
    PermissionResponse { payload: serde_json::Value },
}

pub async fn ws_agent(
    ws: WebSocketUpgrade,
    Path(agent_id): Path<String>,
    Query(query): Query<WsQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    if query
        .token
        .as_ref()
        .and_then(|t| auth::verify_token(t, &state.config.jwt_secret))
        .is_none()
    {
        return Response::builder()
            .status(401)
            .body(axum::body::Body::from("Unauthorized"))
            .unwrap();
    }

    ws.on_upgrade(move |socket| handle_agent_ws(socket, agent_id, state))
}

async fn handle_agent_ws(socket: WebSocket, agent_id: String, state: Arc<AppState>) {
    let mut event_rx = match state.agents.subscribe(&agent_id).await {
        Some(rx) => rx,
        None => return,
    };

    let (mut ws_sink, mut ws_stream) = socket.split();

    // Replay scrollback history
    let history = state.agents.get_scrollback(&agent_id).await;
    let has_history = !history.is_empty();
    for json in history {
        if ws_sink.send(Message::Text(json.into())).await.is_err() {
            return;
        }
    }
    if has_history {
        let done_msg = serde_json::json!({"type": "replay_done"}).to_string();
        let _ = ws_sink.send(Message::Text(done_msg.into())).await;
    }

    loop {
        tokio::select! {
            result = event_rx.recv() => {
                match result {
                    Ok(event) => {
                        let json = match &event {
                            AgentEvent::Line(val) => val.to_string(),
                            AgentEvent::Error { error } => {
                                serde_json::json!({"type": "error", "error": error}).to_string()
                            }
                            AgentEvent::Exited { code } => {
                                serde_json::json!({"type": "exited", "code": code}).to_string()
                            }
                        };
                        state.agents.push_scrollback(&agent_id, json.clone()).await;
                        if ws_sink.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                        if matches!(event, AgentEvent::Exited { .. }) {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Agent WS client lagged {} messages for {}", n, agent_id);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let msg = serde_json::json!({"type": "exited", "code": null});
                        let _ = ws_sink.send(Message::Text(msg.to_string().into())).await;
                        break;
                    }
                }
            }
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(client_msg) = serde_json::from_str::<AgentClientMsg>(&text) {
                            match client_msg {
                                AgentClientMsg::Message { ref content } => {
                                    let user_event = serde_json::json!({
                                        "type": "user_message",
                                        "content": content
                                    });
                                    state.agents.push_scrollback(&agent_id, user_event.to_string()).await;
                                    let _ = state.agents.send_message(&agent_id, content).await;
                                }
                                AgentClientMsg::PermissionResponse { payload } => {
                                    let _ = state.agents.send_raw(&agent_id, &payload).await;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}
