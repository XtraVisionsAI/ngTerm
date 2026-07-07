use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{broadcast, mpsc};

use crate::scrollback::ScrollbackBuffer;
use crate::ssh_bridge::SshCommand;

const BROADCAST_CAPACITY: usize = 512;

#[derive(Clone)]
pub enum SessionEvent {
    Data(Vec<u8>),
    Disconnected,
}

pub enum SessionInput {
    Data(Vec<u8>),
    Resize(u16, u16),
}

pub struct SessionEnded {
    pub session_id: String,
}

struct Session {
    pub id: String,
    pub server_id: String,
    pub server_alias: String,
    pub server_host: String,
    pub ai_tool_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub user_id: String,
    event_tx: broadcast::Sender<SessionEvent>,
    input_tx: mpsc::Sender<SessionInput>,
    scrollback: ScrollbackBuffer<Vec<u8>>,
    ws_count: u32,
    last_activity: Instant,
    idle_timeout_secs: u32,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub server_id: String,
    pub server_alias: String,
    pub server_host: String,
    pub ai_tool_id: Option<String>,
    pub parent_session_id: Option<String>,
}

pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    ended_tx: mpsc::UnboundedSender<SessionEnded>,
}

impl SessionManager {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<SessionEnded>) {
        let (ended_tx, ended_rx) = mpsc::unbounded_channel();
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        Self::start_timeout_checker(sessions.clone());
        (Self { sessions, ended_tx }, ended_rx)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_session(
        &self,
        server_id: String,
        server_alias: String,
        server_host: String,
        ai_tool_id: Option<String>,
        parent_session_id: Option<String>,
        user_id: String,
        mut output_rx: mpsc::Receiver<Vec<u8>>,
        ssh_cmd_tx: mpsc::Sender<SshCommand>,
        idle_timeout_secs: u32,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let (event_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (input_tx, mut input_rx) = mpsc::channel::<SessionInput>(64);

        let event_tx_clone = event_tx.clone();
        let session_id = id.clone();
        let sessions_ref = self.sessions.clone();
        let ended_tx = self.ended_tx.clone();

        tokio::spawn(async move {
            let mut ssh_disconnected = false;
            loop {
                tokio::select! {
                    data = output_rx.recv() => {
                        match data {
                            Some(bytes) => {
                                // Push scrollback before broadcasting
                                {
                                    let mut sessions = sessions_ref.lock().unwrap();
                                    if let Some(s) = sessions.get_mut(&session_id) {
                                        s.scrollback.push(bytes.clone());
                                    }
                                }
                                let _ = event_tx_clone.send(SessionEvent::Data(bytes));
                            }
                            None => {
                                let _ = event_tx_clone.send(SessionEvent::Disconnected);
                                ssh_disconnected = true;
                                break;
                            }
                        }
                    }
                    input = input_rx.recv() => {
                        match input {
                            Some(SessionInput::Data(bytes)) => {
                                let _ = ssh_cmd_tx.send(SshCommand::Data(bytes)).await;
                            }
                            Some(SessionInput::Resize(cols, rows)) => {
                                let _ = ssh_cmd_tx.send(SshCommand::Resize(cols as u32, rows as u32)).await;
                            }
                            None => {
                                let _ = ssh_cmd_tx.send(SshCommand::Close).await;
                                break;
                            }
                        }
                    }
                }
            }

            // Clean up and notify
            sessions_ref.lock().unwrap().remove(&session_id);
            if ssh_disconnected {
                let _ = ended_tx.send(SessionEnded {
                    session_id: session_id.clone(),
                });
            }
            tracing::info!("Session fanout ended: {}", session_id);
        });

        let session = Session {
            id: id.clone(),
            server_id,
            server_alias,
            server_host,
            ai_tool_id,
            parent_session_id,
            user_id,
            event_tx,
            input_tx,
            scrollback: ScrollbackBuffer::new(),
            ws_count: 0,
            last_activity: Instant::now(),
            idle_timeout_secs,
        };

        self.sessions.lock().unwrap().insert(id.clone(), session);
        id
    }

    pub fn subscribe(&self, id: &str) -> Option<broadcast::Receiver<SessionEvent>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.event_tx.subscribe())
    }

    pub fn input_tx(&self, id: &str) -> Option<mpsc::Sender<SessionInput>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.input_tx.clone())
    }

    pub fn event_sender(&self, id: &str) -> Option<broadcast::Sender<SessionEvent>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.event_tx.clone())
    }

    pub fn get_scrollback(&self, id: &str) -> Vec<Vec<u8>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.scrollback.items().to_vec())
            .unwrap_or_default()
    }

    pub fn list_sessions(&self, user_id: &str) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.user_id == user_id)
            .map(|s| SessionInfo {
                id: s.id.clone(),
                server_id: s.server_id.clone(),
                server_alias: s.server_alias.clone(),
                server_host: s.server_host.clone(),
                ai_tool_id: s.ai_tool_id.clone(),
                parent_session_id: s.parent_session_id.clone(),
            })
            .collect()
    }

    pub fn list_all_session_count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    pub fn remove_session(&self, id: &str, user_id: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(session) = sessions.get(id) {
            if session.user_id != user_id {
                return false;
            }
            let _ = session.event_tx.send(SessionEvent::Disconnected);
            sessions.remove(id);
            true
        } else {
            false
        }
    }

    pub fn is_owner(&self, session_id: &str, user_id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|s| s.user_id == user_id)
            .unwrap_or(false)
    }

    pub fn get_server_id(&self, session_id: &str) -> Option<String> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|s| s.server_id.clone())
    }

    pub fn ws_connected(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(s) = sessions.get_mut(session_id) {
            s.ws_count += 1;
            s.last_activity = Instant::now();
        }
    }

    pub fn ws_disconnected(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(s) = sessions.get_mut(session_id) {
            s.ws_count = s.ws_count.saturating_sub(1);
            s.last_activity = Instant::now();
        }
    }

    pub fn touch_activity(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(s) = sessions.get_mut(session_id) {
            s.last_activity = Instant::now();
        }
    }

    fn start_timeout_checker(sessions: Arc<Mutex<HashMap<String, Session>>>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let timed_out: Vec<String> = {
                    let sessions = sessions.lock().unwrap();
                    sessions
                        .values()
                        .filter(|s| {
                            s.idle_timeout_secs > 0
                                && s.ws_count == 0
                                && s.last_activity.elapsed().as_secs() > s.idle_timeout_secs as u64
                        })
                        .map(|s| s.id.clone())
                        .collect()
                };
                for id in timed_out {
                    let event_tx = {
                        let mut sessions = sessions.lock().unwrap();
                        if let Some(session) = sessions.get(&id) {
                            let tx = session.event_tx.clone();
                            let _ = tx.send(SessionEvent::Disconnected);
                            sessions.remove(&id);
                            Some(())
                        } else {
                            None
                        }
                    };
                    if event_tx.is_some() {
                        tracing::info!("Session {} timed out, closing", id);
                    }
                }
            }
        });
    }
}
