use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tracing::Instrument;

use crate::agent_bridge::AgentEvent;
use crate::auth;
use crate::config::limits;
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

    ws.max_message_size(limits::MAX_WS_MESSAGE_BYTES)
        .max_frame_size(limits::MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            let span = tracing::info_span!("ws_terminal", session = %session_id, user = %user_id);
            handle_ws(socket, session_id, state).instrument(span)
        })
}

/// Keeps the session's attached-client count balanced on every exit path,
/// including early returns while replaying scrollback.
struct WsAttachment<'a> {
    state: &'a AppState,
    session_id: &'a str,
}

impl<'a> WsAttachment<'a> {
    fn attach(state: &'a AppState, session_id: &'a str) -> Self {
        state.sessions.ws_connected(session_id);
        Self { state, session_id }
    }
}

impl Drop for WsAttachment<'_> {
    fn drop(&mut self) {
        self.state.sessions.ws_disconnected(self.session_id);
    }
}

fn output_message(bytes: &[u8]) -> Message {
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    Message::Text(
        serde_json::json!({"type": "output", "data": b64})
            .to_string()
            .into(),
    )
}

fn control_message(kind: &str) -> Message {
    Message::Text(serde_json::json!({"type": kind}).to_string().into())
}

/// Build the full replay sequence for a (re)connecting client: one `reset`
/// so the client discards whatever it already rendered, the retained
/// scrollback, then `replay_done`. Used both on connect and after the client
/// fell behind the live broadcast, so there is exactly one way to resync.
fn replay_messages(scrollback: &[Vec<u8>]) -> Vec<Message> {
    let mut out = Vec::with_capacity(scrollback.len() + 2);
    out.push(control_message("reset"));
    out.extend(scrollback.iter().map(|d| output_message(d)));
    out.push(control_message("replay_done"));
    out
}

async fn send_all<S>(sink: &mut S, messages: Vec<Message>) -> bool
where
    S: SinkExt<Message> + Unpin,
{
    for msg in messages {
        if sink.send(msg).await.is_err() {
            return false;
        }
    }
    true
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

    // Counted as attached from here on; the guard undoes it on every exit.
    let _attachment = WsAttachment::attach(&state, &session_id);

    let scrollback = state.sessions.get_scrollback(&session_id);
    if !send_all(&mut ws_sink, replay_messages(&scrollback)).await {
        return;
    }

    loop {
        tokio::select! {
            result = event_rx.recv() => {
                match result {
                    Ok(SessionEvent::Data(bytes)) => {
                        if ws_sink.send(output_message(&bytes)).await.is_err() {
                            break;
                        }
                    }
                    Ok(SessionEvent::Disconnected) => {
                        let _ = ws_sink.send(control_message("disconnected")).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WS client lagged {} messages for session {}, resyncing from scrollback", n, session_id);
                        let scrollback = state.sessions.get_scrollback(&session_id);
                        if !send_all(&mut ws_sink, replay_messages(&scrollback)).await {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let _ = ws_sink.send(control_message("disconnected")).await;
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
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

// --- Agent WebSocket ---

#[derive(serde::Deserialize)]
#[serde(tag = "type")]
enum AgentClientMsg {
    #[serde(rename = "message")]
    Message { content: String },
    /// Approval decision. Accepts both the flat form
    /// `{type, id, approved}` and the legacy nested `{type, payload: {...}}`.
    #[serde(rename = "permission_response")]
    PermissionResponse {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        approved: Option<bool>,
        #[serde(default)]
        payload: Option<serde_json::Value>,
    },
    /// Answer to an `ask_user` question.
    #[serde(rename = "user_answer")]
    UserAnswer { id: String, answer: String },
}

/// Normalise a permission response into the flat wire form the agent side
/// understands. Returns `None` when no request id can be found.
fn normalize_permission_response(
    id: Option<String>,
    approved: Option<bool>,
    payload: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let from_payload = |key: &str| payload.as_ref().and_then(|p| p.get(key).cloned());
    let id = id
        .filter(|s| !s.is_empty())
        .or_else(|| from_payload("id").and_then(|v| v.as_str().map(|s| s.to_string())))
        .filter(|s| !s.is_empty())?;
    let approved = approved
        .or_else(|| from_payload("approved").and_then(|v| v.as_bool()))
        .unwrap_or(false);
    Some(serde_json::json!({
        "type": "permission_response",
        "id": id,
        "approved": approved,
    }))
}

pub async fn ws_agent(
    ws: WebSocketUpgrade,
    Path(agent_id): Path<String>,
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

    // Ownership is checked before the upgrade so the caller gets a definite
    // HTTP status instead of a silently closed socket.
    match state.agents.is_owner(&agent_id, &user_id).await {
        None => {
            return Response::builder()
                .status(404)
                .body(axum::body::Body::from("Agent not found"))
                .unwrap();
        }
        Some(false) => {
            return Response::builder()
                .status(403)
                .body(axum::body::Body::from("Forbidden"))
                .unwrap();
        }
        Some(true) => {}
    }

    ws.max_message_size(limits::MAX_WS_MESSAGE_BYTES)
        .max_frame_size(limits::MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            let span = tracing::info_span!("ws_agent", agent = %agent_id, user = %user_id);
            handle_agent_ws(socket, agent_id, user_id, state).instrument(span)
        })
}

async fn handle_agent_ws(
    socket: WebSocket,
    agent_id: String,
    user_id: String,
    state: Arc<AppState>,
) {
    let (mut ws_sink, mut ws_stream) = socket.split();

    let mut event_rx = match state.agents.subscribe(&agent_id).await {
        Some(rx) => rx,
        None => {
            let msg = serde_json::json!({"type": "error", "error": "Agent not found"});
            let _ = ws_sink.send(Message::Text(msg.to_string().into())).await;
            let _ = ws_sink.send(Message::Close(None)).await;
            return;
        }
    };

    // Replay history (recorded at the producer side, independent of clients).
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
                        let json = event.to_json_string();
                        if ws_sink.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                        if matches!(event, AgentEvent::Exited { .. }) {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Agent WS client lagged {} messages for {}", n, agent_id);
                        // Tell the client its view is incomplete instead of pretending.
                        let msg = serde_json::json!({"type": "gap", "dropped": n, "source": "live"});
                        if ws_sink.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
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
                        // Re-check ownership per message: the agent may have been
                        // stopped and restarted by another principal meanwhile.
                        if state.agents.is_owner(&agent_id, &user_id).await != Some(true) {
                            let msg = serde_json::json!({"type": "error", "error": "Agent no longer available"});
                            let _ = ws_sink.send(Message::Text(msg.to_string().into())).await;
                            break;
                        }
                        match serde_json::from_str::<AgentClientMsg>(&text) {
                            Ok(AgentClientMsg::Message { ref content }) => {
                                let user_event = serde_json::json!({
                                    "type": "user_message",
                                    "content": content
                                });
                                state.agents.push_scrollback(&agent_id, user_event.to_string()).await;
                                let _ = state.agents.send_message(&agent_id, content).await;
                            }
                            Ok(AgentClientMsg::PermissionResponse { id, approved, payload }) => {
                                match normalize_permission_response(id, approved, payload) {
                                    Some(normalized) => {
                                        let _ = state.agents.send_raw(&agent_id, &normalized).await;
                                    }
                                    None => {
                                        let msg = serde_json::json!({
                                            "type": "permission_ack", "id": "", "status": "invalid",
                                            "error": "permission_response requires an id"
                                        });
                                        let _ = ws_sink.send(Message::Text(msg.to_string().into())).await;
                                    }
                                }
                            }
                            Ok(AgentClientMsg::UserAnswer { id, answer }) => {
                                let normalized = serde_json::json!({
                                    "type": "user_answer", "id": id, "answer": answer
                                });
                                state.agents.push_scrollback(
                                    &agent_id,
                                    serde_json::json!({"type": "user_message", "content": answer}).to_string(),
                                ).await;
                                let _ = state.agents.send_raw(&agent_id, &normalized).await;
                            }
                            Err(_) => {
                                let msg = serde_json::json!({"type": "error", "error": "Unrecognised client message"});
                                let _ = ws_sink.send(Message::Text(msg.to_string().into())).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(msg: &Message) -> serde_json::Value {
        match msg {
            Message::Text(t) => serde_json::from_str(t).unwrap(),
            _ => panic!("expected text frame"),
        }
    }

    #[test]
    fn replay_is_reset_then_chunks_then_done() {
        let msgs = replay_messages(&[b"ab".to_vec(), b"cd".to_vec()]);
        let kinds: Vec<String> = msgs
            .iter()
            .map(|m| text_of(m)["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["reset", "output", "output", "replay_done"]);
        assert_eq!(text_of(&msgs[1])["data"], "YWI=");

        // Empty scrollback still tells the client to clear stale content.
        let kinds: Vec<String> = replay_messages(&[])
            .iter()
            .map(|m| text_of(m)["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["reset", "replay_done"]);
    }

    #[test]
    fn permission_response_accepts_flat_and_nested_forms() {
        let flat = normalize_permission_response(Some("t1".into()), Some(true), None).unwrap();
        assert_eq!(flat["id"], "t1");
        assert_eq!(flat["approved"], true);

        let nested = normalize_permission_response(
            None,
            None,
            Some(serde_json::json!({"type": "permission_response", "id": "t2", "approved": false})),
        )
        .unwrap();
        assert_eq!(nested["id"], "t2");
        assert_eq!(nested["approved"], false);

        assert!(normalize_permission_response(None, Some(true), None).is_none());
        assert!(normalize_permission_response(Some(String::new()), Some(true), None).is_none());
    }
}
