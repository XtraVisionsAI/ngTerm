use std::collections::{HashMap, HashSet};
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

pub struct AgentSession {
    stdin_tx: mpsc::Sender<Vec<u8>>,
    event_tx: broadcast::Sender<AgentEvent>,
    scrollback: std::sync::Mutex<ScrollbackBuffer<String>>,
}

pub struct AgentBridge {
    sessions: Arc<Mutex<HashMap<String, AgentSession>>>,
    prepared: Arc<Mutex<HashSet<String>>>,
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

impl AgentBridge {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            prepared: Arc::new(Mutex::new(HashSet::new())),
        }
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

    pub async fn start(
        &self,
        agent_id: &str,
        session_id: &str,
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
            return Self::start_local(&mut sessions, agent_id, tool, initial_prompt, working_dir)
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

        // Build launch command with env vars
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

        // Step 5: Open channel and exec
        let wrapped_cmd = format!("$SHELL -lic {} 2>&1", crate::utils::shell_escape(&cmd));
        tracing::debug!("launch_command: {}", wrapped_cmd);

        let channel = helpers.open_exec_channel(session_id, &wrapped_cmd).await?;

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);
        let (event_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let event_tx_clone = event_tx.clone();

        let agent_session_id = agent_id.to_string();
        let helper_session_id = session_id.to_string();
        let sessions_ref = Arc::clone(&self.sessions);

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
        let event_tx_reader = event_tx_clone.clone();
        let session_id_reader = agent_session_id.clone();
        let sessions_ref_reader = Arc::clone(&sessions_ref);
        let helper_session_for_reader = helper_session_id.clone();
        let helpers_for_reader = Arc::new(tokio::sync::Notify::new());
        // We need a reference to HelperPool in the reader task for cleanup.
        // Since HelperPool is behind Arc<AppState>, we store the session_id and
        // handle cleanup externally. Instead, use a simpler approach: store the
        // helpers reference via the sessions map removal triggering cleanup in web.rs.
        // Actually, we can't hold &HelperPool across spawn. Use a different approach:
        // store the helper_session_id and let the caller handle remove_user.
        drop(helpers_for_reader);

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

            sessions_ref_reader.lock().await.remove(&session_id_reader);
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

        let mut scrollback = ScrollbackBuffer::new();
        scrollback.push(initial_user_event);

        sessions.insert(
            agent_session_id,
            AgentSession {
                stdin_tx,
                event_tx: event_tx_clone,
                scrollback: std::sync::Mutex::new(scrollback),
            },
        );

        Ok(())
    }

    pub async fn send_message(&self, agent_id: &str, content: &str) -> Result<(), String> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(agent_id)
            .ok_or_else(|| "Agent session not found".to_string())?;

        let msg = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": content
            }
        });
        let mut bytes = serde_json::to_vec(&msg).unwrap();
        bytes.push(b'\n');

        session
            .stdin_tx
            .send(bytes)
            .await
            .map_err(|_| "Agent stdin closed".to_string())
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

    pub async fn stop(&self, agent_id: &str) {
        self.sessions.lock().await.remove(agent_id);
    }

    /// Register an in-process (embedded) agent session whose stdin/event
    /// channels are driven by an external engine rather than a spawned CLI.
    /// The caller is responsible for removing the session (via `stop`) when
    /// the engine terminates.
    pub async fn register_embedded(
        &self,
        agent_id: &str,
        stdin_tx: mpsc::Sender<Vec<u8>>,
        event_tx: broadcast::Sender<AgentEvent>,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().await;
        if sessions.contains_key(agent_id) {
            return Err("Agent session already exists".to_string());
        }
        sessions.insert(
            agent_id.to_string(),
            AgentSession {
                stdin_tx,
                event_tx,
                scrollback: std::sync::Mutex::new(ScrollbackBuffer::new()),
            },
        );
        Ok(())
    }

    pub async fn push_scrollback(&self, agent_id: &str, data: String) {
        let sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(agent_id) {
            session.scrollback.lock().unwrap().push(data);
        }
    }

    pub async fn get_scrollback(&self, agent_id: &str) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(agent_id)
            .map(|s| s.scrollback.lock().unwrap().items().to_vec())
            .unwrap_or_default()
    }

    async fn start_local(
        sessions: &mut HashMap<String, AgentSession>,
        agent_id: &str,
        tool: &AgentToolContext,
        initial_prompt: &str,
        working_dir: Option<&str>,
    ) -> Result<(), String> {
        // Build launch command with env vars
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

        let wrapped_cmd = format!("$SHELL -lic {}", crate::utils::shell_escape(&cmd));
        tracing::debug!("local agent launch: {}", wrapped_cmd);

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
        let event_tx_clone = event_tx.clone();

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
        let event_tx_reader = event_tx_clone.clone();
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

        let mut scrollback = ScrollbackBuffer::new();
        scrollback.push(initial_user_event);

        sessions.insert(
            session_id,
            AgentSession {
                stdin_tx,
                event_tx: event_tx_clone,
                scrollback: std::sync::Mutex::new(scrollback),
            },
        );

        Ok(())
    }
}
