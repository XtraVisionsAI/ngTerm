use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, mpsc, Mutex};

use crate::helper_pool::HelperPool;
use crate::scrollback::ScrollbackBuffer;

const BROADCAST_CAPACITY: usize = 256;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(untagged)]
pub enum AgentEvent {
    Line(serde_json::Value),
    Error { error: String },
    Exited { code: Option<u32> },
}

impl AgentEvent {
    /// Wire representation sent to WebSocket clients and stored in history.
    pub fn to_json_string(&self) -> String {
        match self {
            AgentEvent::Line(val) => val.to_string(),
            AgentEvent::Error { error } => {
                serde_json::json!({"type": "error", "error": error}).to_string()
            }
            AgentEvent::Exited { code } => {
                serde_json::json!({"type": "exited", "code": code}).to_string()
            }
        }
    }
}

type SharedScrollback = Arc<std::sync::Mutex<ScrollbackBuffer<String>>>;

pub struct AgentSession {
    /// Platform user who started the agent; every WS/HTTP access must match.
    owner_user_id: String,
    /// Monotonic token so a stale cleanup task cannot remove a newer session
    /// registered under the same agent id.
    generation: u64,
    stdin_tx: mpsc::Sender<Vec<u8>>,
    event_tx: broadcast::Sender<AgentEvent>,
    scrollback: SharedScrollback,
}

pub struct AgentBridge {
    sessions: Arc<Mutex<HashMap<String, AgentSession>>>,
    prepared: Arc<Mutex<HashSet<String>>>,
    generation: AtomicU64,
}

pub struct AgentToolContext {
    pub detect_cmd: String,
    pub install_cmd: String,
    pub config: Option<String>,
    pub config_path: Option<String>,
    pub launch_cmd: String,
    pub env_vars: HashMap<String, String>,
}

impl Default for AgentBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Persist events at the producer side: a recorder task subscribes to the
/// broadcast channel as soon as the session is created, so history is kept
/// even when no browser is attached and is never duplicated by multiple
/// subscribers. A `gap` marker is recorded when the recorder itself lags.
fn spawn_recorder(mut rx: broadcast::Receiver<AgentEvent>, scrollback: SharedScrollback) {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let json = event.to_json_string();
                    if let Ok(mut sb) = scrollback.lock() {
                        sb.push(json);
                    }
                    if matches!(event, AgentEvent::Exited { .. }) {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("Agent history recorder lagged {} events", n);
                    if let Ok(mut sb) = scrollback.lock() {
                        sb.push(
                            serde_json::json!({"type": "gap", "dropped": n, "source": "history"})
                                .to_string(),
                        );
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

impl AgentBridge {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            prepared: Arc::new(Mutex::new(HashSet::new())),
            generation: AtomicU64::new(1),
        }
    }

    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst)
    }

    pub async fn prepare_tool(
        &self,
        session_id: &str,
        helpers: &HelperPool,
        detect_cmd: &str,
        install_cmd: &str,
    ) -> Result<(), String> {
        if helpers.is_local(session_id).await {
            self.prepared.lock().await.insert(session_id.to_string());
            return Ok(());
        }

        // Detect tool
        let (detect_output, detect_code) = helpers.exec_with_status(session_id, detect_cmd).await?;
        tracing::debug!(
            "Prepare detect '{}': exit={}, output='{}'",
            detect_cmd,
            detect_code,
            detect_output.trim()
        );
        if detect_code != 0 {
            // Install
            let (install_output, install_code) =
                helpers.exec_with_status(session_id, install_cmd).await?;
            tracing::debug!(
                "Prepare install: exit={}, output='{}'",
                install_code,
                &install_output[..install_output.len().min(200)]
            );
            // Verify
            let (verify_output, verify_code) =
                helpers.exec_with_status(session_id, detect_cmd).await?;
            if verify_code != 0 {
                return Err(format!(
                    "Tool not found after install. detect_cmd='{}', output='{}'",
                    detect_cmd,
                    verify_output.trim()
                ));
            }
        }

        self.prepared.lock().await.insert(session_id.to_string());
        tracing::info!("Agent tool prepared for session: {}", session_id);
        Ok(())
    }

    pub async fn is_prepared(&self, session_id: &str) -> bool {
        self.prepared.lock().await.contains(session_id)
    }

    /// Build the launch command line. Environment values are passed through
    /// `env` so they never appear in the (logged) command string itself.
    fn build_launch_cmd(tool: &AgentToolContext, working_dir: Option<&str>) -> String {
        let mut cmd = String::new();
        if !tool.env_vars.is_empty() {
            cmd.push_str("env ");
            for (key, val) in &tool.env_vars {
                cmd.push_str(&format!("{}={} ", key, crate::utils::shell_escape(val)));
            }
        }
        cmd.push_str(&tool.launch_cmd);
        if let Some(dir) = working_dir {
            cmd.push_str(&format!(" --cwd {}", crate::utils::shell_escape(dir)));
        }
        cmd
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        &self,
        agent_id: &str,
        session_id: &str,
        owner_user_id: &str,
        helpers: &HelperPool,
        tool: &AgentToolContext,
        initial_prompt: &str,
        working_dir: Option<&str>,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().await;
        if sessions.contains_key(agent_id) {
            return Err("Agent session already exists".to_string());
        }

        if helpers.is_local(session_id).await {
            let generation = self.next_generation();
            return Self::start_local(
                &mut sessions,
                agent_id,
                owner_user_id,
                generation,
                tool,
                initial_prompt,
                working_dir,
            )
            .await;
        }

        // Run prepare if not already done
        if !self.is_prepared(session_id).await {
            drop(sessions); // Release lock during potentially slow network ops
            self.prepare_tool(session_id, helpers, &tool.detect_cmd, &tool.install_cmd)
                .await?;
            sessions = self.sessions.lock().await;
            if sessions.contains_key(agent_id) {
                return Err("Agent session already exists".to_string());
            }
        }

        // Write config file (user-specific, done each time)
        if let (Some(ref config), Some(ref path)) = (&tool.config, &tool.config_path) {
            let mkdir_cmd = format!(
                "mkdir -p \"$(dirname {})\"",
                crate::utils::shell_escape(path)
            );
            helpers.exec_with_status(session_id, &mkdir_cmd).await?;
            let write_cmd = format!(
                "cat > {} << 'ONEMUX_EOF'\n{}\nONEMUX_EOF",
                crate::utils::shell_escape(path),
                config
            );
            helpers.exec_with_status(session_id, &write_cmd).await?;
        }

        let cmd = Self::build_launch_cmd(tool, working_dir);

        // Open channel and exec. Only the tool's own launch command is logged:
        // the full line carries credentials via `env`.
        let wrapped_cmd = format!("$SHELL -lic {} 2>&1", crate::utils::shell_escape(&cmd));
        tracing::debug!(
            "launching external agent: launch_cmd='{}', env_keys={:?}",
            tool.launch_cmd,
            tool.env_vars.keys().collect::<Vec<_>>()
        );

        let channel = helpers.open_exec_channel(session_id, &wrapped_cmd).await?;

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);
        let (event_tx, _) = broadcast::channel(BROADCAST_CAPACITY);

        // History recorder subscribes before any event can be produced.
        let scrollback: SharedScrollback = Arc::new(std::sync::Mutex::new(ScrollbackBuffer::new()));
        spawn_recorder(event_tx.subscribe(), scrollback.clone());

        let agent_session_id = agent_id.to_string();
        let helper_session_id = session_id.to_string();
        let sessions_ref = Arc::clone(&self.sessions);
        let generation = self.next_generation();

        // Send initial prompt
        let initial_msg = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": initial_prompt
            }
        });
        let mut initial_bytes = serde_json::to_vec(&initial_msg).unwrap();
        initial_bytes.push(b'\n');

        let stream = channel.into_stream();
        let (reader, mut writer) = tokio::io::split(stream);

        if let Err(e) = writer.write_all(&initial_bytes).await {
            return Err(format!("Failed to send initial prompt: {}", e));
        }
        if let Err(e) = writer.flush().await {
            return Err(format!("Failed to flush initial prompt: {}", e));
        }

        // Mark connection as used by agent
        helpers.add_user(session_id).await;

        // Spawn reader task
        let event_tx_reader = event_tx.clone();
        let session_id_reader = agent_session_id.clone();
        let sessions_ref_reader = Arc::clone(&sessions_ref);
        let helper_session_for_reader = helper_session_id.clone();

        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::with_capacity(256 * 1024, reader).lines();

            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if line.contains("cannot set terminal process group")
                            || line.contains("no job control in this shell")
                        {
                            continue;
                        }
                        match serde_json::from_str::<serde_json::Value>(&line) {
                            Ok(val) => {
                                let _ = event_tx_reader.send(AgentEvent::Line(val));
                            }
                            Err(_) => {
                                let _ = event_tx_reader.send(AgentEvent::Error {
                                    error: format!("Non-JSON output: {}", line),
                                });
                            }
                        }
                    }
                    Ok(None) => {
                        let _ = event_tx_reader.send(AgentEvent::Exited { code: Some(0) });
                        break;
                    }
                    Err(e) => {
                        let _ = event_tx_reader.send(AgentEvent::Error {
                            error: format!("Read error: {}", e),
                        });
                        let _ = event_tx_reader.send(AgentEvent::Exited { code: None });
                        break;
                    }
                }
            }

            // Only remove the session this task belongs to.
            let mut sessions = sessions_ref_reader.lock().await;
            if sessions
                .get(&session_id_reader)
                .is_some_and(|s| s.generation == generation)
            {
                sessions.remove(&session_id_reader);
            }
            tracing::info!(
                "Agent session ended: {} (connection: {})",
                session_id_reader,
                helper_session_for_reader
            );
        });

        // Spawn writer task
        tokio::spawn(async move {
            while let Some(data) = stdin_rx.recv().await {
                if writer.write_all(&data).await.is_err() {
                    break;
                }
                let _ = writer.flush().await;
            }
        });

        let initial_user_event = serde_json::json!({
            "type": "user_message",
            "content": initial_prompt
        })
        .to_string();
        if let Ok(mut sb) = scrollback.lock() {
            sb.push(initial_user_event);
        }

        sessions.insert(
            agent_session_id,
            AgentSession {
                owner_user_id: owner_user_id.to_string(),
                generation,
                stdin_tx,
                event_tx,
                scrollback,
            },
        );

        Ok(())
    }

    pub async fn send_message(&self, agent_id: &str, content: &str) -> Result<(), String> {
        let msg = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": content
            }
        });
        self.send_raw(agent_id, &msg).await
    }

    pub async fn send_raw(&self, agent_id: &str, data: &serde_json::Value) -> Result<(), String> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(agent_id)
            .ok_or_else(|| "Agent session not found".to_string())?;

        let mut bytes = serde_json::to_vec(data).unwrap();
        bytes.push(b'\n');

        session
            .stdin_tx
            .send(bytes)
            .await
            .map_err(|_| "Agent stdin closed".to_string())
    }

    pub async fn subscribe(&self, agent_id: &str) -> Option<broadcast::Receiver<AgentEvent>> {
        self.sessions
            .lock()
            .await
            .get(agent_id)
            .map(|s| s.event_tx.subscribe())
    }

    pub async fn is_active(&self, agent_id: &str) -> bool {
        self.sessions.lock().await.contains_key(agent_id)
    }

    /// `None` when the agent does not exist; otherwise whether `user_id`
    /// started it.
    pub async fn is_owner(&self, agent_id: &str, user_id: &str) -> Option<bool> {
        self.sessions
            .lock()
            .await
            .get(agent_id)
            .map(|s| s.owner_user_id == user_id)
    }

    pub async fn owner(&self, agent_id: &str) -> Option<String> {
        self.sessions
            .lock()
            .await
            .get(agent_id)
            .map(|s| s.owner_user_id.clone())
    }

    pub async fn stop(&self, agent_id: &str) {
        self.sessions.lock().await.remove(agent_id);
    }

    /// Remove the session only if it is still the one identified by
    /// `generation`; a task cleaning up an old run cannot delete a new one.
    pub async fn stop_generation(&self, agent_id: &str, generation: u64) -> bool {
        let mut sessions = self.sessions.lock().await;
        if sessions
            .get(agent_id)
            .is_some_and(|s| s.generation == generation)
        {
            sessions.remove(agent_id);
            true
        } else {
            false
        }
    }

    /// Register an in-process (embedded) agent session whose stdin/event
    /// channels are driven by an external engine rather than a spawned CLI.
    ///
    /// The history recorder subscribes here, so the caller must register
    /// *before* producing any event. Returns the generation token to pass to
    /// `stop_generation` when the engine terminates.
    pub async fn register_embedded(
        &self,
        agent_id: &str,
        owner_user_id: &str,
        stdin_tx: mpsc::Sender<Vec<u8>>,
        event_tx: broadcast::Sender<AgentEvent>,
    ) -> Result<u64, String> {
        let mut sessions = self.sessions.lock().await;
        if sessions.contains_key(agent_id) {
            return Err("Agent session already exists".to_string());
        }
        let scrollback: SharedScrollback = Arc::new(std::sync::Mutex::new(ScrollbackBuffer::new()));
        spawn_recorder(event_tx.subscribe(), scrollback.clone());
        let generation = self.next_generation();
        sessions.insert(
            agent_id.to_string(),
            AgentSession {
                owner_user_id: owner_user_id.to_string(),
                generation,
                stdin_tx,
                event_tx,
                scrollback,
            },
        );
        Ok(generation)
    }

    /// Record a client-originated entry (e.g. the user's own message).
    pub async fn push_scrollback(&self, agent_id: &str, data: String) {
        let sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(agent_id) {
            if let Ok(mut sb) = session.scrollback.lock() {
                sb.push(data);
            }
        }
    }

    pub async fn get_scrollback(&self, agent_id: &str) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(agent_id)
            .and_then(|s| s.scrollback.lock().ok().map(|sb| sb.items().to_vec()))
            .unwrap_or_default()
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_local(
        sessions: &mut HashMap<String, AgentSession>,
        agent_id: &str,
        owner_user_id: &str,
        generation: u64,
        tool: &AgentToolContext,
        initial_prompt: &str,
        working_dir: Option<&str>,
    ) -> Result<(), String> {
        let cmd = Self::build_launch_cmd(tool, working_dir);

        let wrapped_cmd = format!("$SHELL -lic {}", crate::utils::shell_escape(&cmd));
        tracing::debug!(
            "launching local external agent: launch_cmd='{}', env_keys={:?}",
            tool.launch_cmd,
            tool.env_vars.keys().collect::<Vec<_>>()
        );

        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&wrapped_cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn local agent: {}", e))?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);
        let (event_tx, _) = broadcast::channel(BROADCAST_CAPACITY);

        let scrollback: SharedScrollback = Arc::new(std::sync::Mutex::new(ScrollbackBuffer::new()));
        spawn_recorder(event_tx.subscribe(), scrollback.clone());

        let session_id = agent_id.to_string();

        // Send initial prompt
        let initial_msg = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": initial_prompt
            }
        });
        let mut initial_bytes = serde_json::to_vec(&initial_msg).unwrap();
        initial_bytes.push(b'\n');

        let mut writer = stdin;
        writer
            .write_all(&initial_bytes)
            .await
            .map_err(|e| format!("Failed to send initial prompt: {}", e))?;
        writer
            .flush()
            .await
            .map_err(|e| format!("Failed to flush initial prompt: {}", e))?;

        // Spawn reader task
        let event_tx_reader = event_tx.clone();
        let session_id_reader = session_id.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::with_capacity(256 * 1024, stdout).lines();

            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<serde_json::Value>(&line) {
                            Ok(val) => {
                                let _ = event_tx_reader.send(AgentEvent::Line(val));
                            }
                            Err(_) => {
                                let _ = event_tx_reader.send(AgentEvent::Error {
                                    error: format!("Non-JSON output: {}", line),
                                });
                            }
                        }
                    }
                    Ok(None) => {
                        let _ = event_tx_reader.send(AgentEvent::Exited { code: Some(0) });
                        break;
                    }
                    Err(e) => {
                        let _ = event_tx_reader.send(AgentEvent::Error {
                            error: format!("Read error: {}", e),
                        });
                        let _ = event_tx_reader.send(AgentEvent::Exited { code: None });
                        break;
                    }
                }
            }
            // Reap the child so it does not linger as a zombie.
            let _ = child.wait().await;
            tracing::info!("Local agent session ended: {}", session_id_reader);
        });

        // Spawn writer task
        tokio::spawn(async move {
            while let Some(data) = stdin_rx.recv().await {
                if writer.write_all(&data).await.is_err() {
                    break;
                }
                let _ = writer.flush().await;
            }
        });

        let initial_user_event = serde_json::json!({
            "type": "user_message",
            "content": initial_prompt
        })
        .to_string();
        if let Ok(mut sb) = scrollback.lock() {
            sb.push(initial_user_event);
        }

        sessions.insert(
            session_id,
            AgentSession {
                owner_user_id: owner_user_id.to_string(),
                generation,
                stdin_tx,
                event_tx,
                scrollback,
            },
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn embedded_sessions_track_owner_generation_and_history() {
        let bridge = AgentBridge::new();
        let (stdin_tx, _stdin_rx) = mpsc::channel::<Vec<u8>>(4);
        let (event_tx, _) = broadcast::channel(16);

        let generation = bridge
            .register_embedded("agent-1", "alice", stdin_tx.clone(), event_tx.clone())
            .await
            .unwrap();

        assert_eq!(bridge.is_owner("agent-1", "alice").await, Some(true));
        assert_eq!(bridge.is_owner("agent-1", "bob").await, Some(false));
        assert_eq!(bridge.is_owner("agent-9", "alice").await, None);

        // Duplicate registration is refused.
        assert!(bridge
            .register_embedded("agent-1", "alice", stdin_tx.clone(), event_tx.clone())
            .await
            .is_err());

        // Events are recorded without any WebSocket subscriber.
        event_tx
            .send(AgentEvent::Line(
                serde_json::json!({"type": "assistant_delta", "text": "hi"}),
            ))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let history = bridge.get_scrollback("agent-1").await;
        assert_eq!(history.len(), 1);
        assert!(history[0].contains("assistant_delta"));

        // A stale generation cannot remove the live session.
        assert!(!bridge.stop_generation("agent-1", generation + 100).await);
        assert!(bridge.is_active("agent-1").await);
        assert!(bridge.stop_generation("agent-1", generation).await);
        assert!(!bridge.is_active("agent-1").await);
    }
}
