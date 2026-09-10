use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::audit_events::Integrity;
use crate::recording::RecordingStore;
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

/// Emitted exactly once per session when its fan-out task ends, whatever
/// the cause. Consumers release per-session resources (helper SSH
/// connection, audit row) here rather than at each individual exit path.
pub struct SessionEnded {
    pub session_id: String,
    /// `ssh_closed`, `user_closed`, `idle_timeout`, `input_closed` or
    /// `server_shutdown`.
    pub reason: String,
    /// What the terminal recording achieved; `None` when the session was
    /// not recorded at all.
    pub recording: Option<Integrity>,
}

pub const REASON_SSH_CLOSED: &str = "ssh_closed";
pub const REASON_USER_CLOSED: &str = "user_closed";
pub const REASON_IDLE_TIMEOUT: &str = "idle_timeout";
pub const REASON_INPUT_CLOSED: &str = "input_closed";
pub const REASON_SERVER_SHUTDOWN: &str = "server_shutdown";

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
    /// Tells the fan-out task to shut the transport down; carries the reason.
    close_tx: Option<oneshot::Sender<String>>,
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
    recordings: Option<Arc<RecordingStore>>,
}

impl SessionManager {
    pub fn new(
        recordings: Option<Arc<RecordingStore>>,
    ) -> (Self, mpsc::UnboundedReceiver<SessionEnded>) {
        let (ended_tx, ended_rx) = mpsc::unbounded_channel();
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        Self::start_timeout_checker(sessions.clone());
        (
            Self {
                sessions,
                ended_tx,
                recordings,
            },
            ended_rx,
        )
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
        initial_size: (u16, u16),
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        // Recording starts before the first byte can arrive and runs in the
        // fan-out task, so it does not depend on a browser being attached.
        let recorder = self
            .recordings
            .as_ref()
            .and_then(|r| r.start(&id, initial_size.0, initial_size.1));
        let (event_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (input_tx, mut input_rx) = mpsc::channel::<SessionInput>(64);
        let (close_tx, mut close_rx) = oneshot::channel::<String>();

        let event_tx_clone = event_tx.clone();
        let session_id = id.clone();
        let sessions_ref = self.sessions.clone();
        let ended_tx = self.ended_tx.clone();

        tokio::spawn(async move {
            let reason: String;
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
                                if let Some(r) = &recorder {
                                    r.output(&bytes);
                                }
                                let _ = event_tx_clone.send(SessionEvent::Data(bytes));
                            }
                            None => {
                                reason = REASON_SSH_CLOSED.to_string();
                                break;
                            }
                        }
                    }
                    input = input_rx.recv() => {
                        match input {
                            Some(SessionInput::Data(bytes)) => {
                                if let Some(r) = &recorder {
                                    r.input(&bytes);
                                }
                                let _ = ssh_cmd_tx.send(SshCommand::Data(bytes)).await;
                            }
                            Some(SessionInput::Resize(cols, rows)) => {
                                if let Some(r) = &recorder {
                                    r.resize(cols, rows);
                                }
                                let _ = ssh_cmd_tx.send(SshCommand::Resize(cols as u32, rows as u32)).await;
                            }
                            None => {
                                reason = REASON_INPUT_CLOSED.to_string();
                                break;
                            }
                        }
                    }
                    requested = &mut close_rx => {
                        reason = requested.unwrap_or_else(|_| REASON_INPUT_CLOSED.to_string());
                        break;
                    }
                }
            }

            // Tear down in a fixed order regardless of the trigger: close the
            // transport, tell subscribers, drop the registry entry, then let
            // the owner of per-session resources know exactly once.
            if reason != REASON_SSH_CLOSED {
                let _ = ssh_cmd_tx.send(SshCommand::Close).await;
            }
            let _ = event_tx_clone.send(SessionEvent::Disconnected);
            sessions_ref.lock().unwrap().remove(&session_id);
            let recording = match recorder {
                Some(r) => Some(r.finish().await),
                None => None,
            };
            let _ = ended_tx.send(SessionEnded {
                session_id: session_id.clone(),
                reason: reason.clone(),
                recording,
            });
            tracing::info!("Session fanout ended: {} ({})", session_id, reason);
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
            close_tx: Some(close_tx),
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

    /// Ask the session to close. Returns false if it does not exist or is
    /// owned by someone else. Resource release happens when the fan-out task
    /// reports `SessionEnded`, so callers must not clean up themselves.
    pub fn remove_session(&self, id: &str, user_id: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get_mut(id) {
            Some(session) if session.user_id == user_id => {
                Self::request_close(session, REASON_USER_CLOSED);
                true
            }
            _ => false,
        }
    }

    fn request_close(session: &mut Session, reason: &str) {
        if let Some(tx) = session.close_tx.take() {
            let _ = tx.send(reason.to_string());
        }
    }

    /// Request closure of every live session (server shutdown). Returns how
    /// many were asked to close; each reports `SessionEnded` when done.
    pub fn close_all(&self, reason: &str) -> usize {
        let mut sessions = self.sessions.lock().unwrap();
        let mut n = 0;
        for s in sessions.values_mut() {
            if s.close_tx.is_some() {
                Self::request_close(s, reason);
                n += 1;
            }
        }
        n
    }

    pub fn session_exists(&self, id: &str) -> bool {
        self.sessions.lock().unwrap().contains_key(id)
    }

    pub fn session_count_for_user(&self, user_id: &str) -> usize {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.user_id == user_id)
            .count()
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
                Self::close_idle_sessions(&sessions);
            }
        });
    }

    /// Request closure of every session that has no WebSocket attached and
    /// has been idle longer than its configured timeout. Closing goes through
    /// the fan-out task so the transport, subscribers and per-session
    /// resources are released the same way as for any other exit.
    fn close_idle_sessions(sessions: &Mutex<HashMap<String, Session>>) {
        let mut sessions = sessions.lock().unwrap();
        for s in sessions.values_mut() {
            let idle = s.idle_timeout_secs > 0
                && s.ws_count == 0
                && s.last_activity.elapsed()
                    >= std::time::Duration::from_secs(s.idle_timeout_secs as u64);
            if idle && s.close_tx.is_some() {
                tracing::info!(
                    "Session {} idle for {}s, closing",
                    s.id,
                    s.idle_timeout_secs
                );
                Self::request_close(s, REASON_IDLE_TIMEOUT);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Fixture {
        mgr: SessionManager,
        ended_rx: mpsc::UnboundedReceiver<SessionEnded>,
        output_tx: mpsc::Sender<Vec<u8>>,
        cmd_rx: mpsc::Receiver<SshCommand>,
        id: String,
    }

    fn fixture(idle_timeout_secs: u32) -> Fixture {
        let (mgr, ended_rx) = SessionManager::new(None);
        let (output_tx, output_rx) = mpsc::channel(8);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let id = mgr.create_session(
            "srv".into(),
            "alias".into(),
            "host".into(),
            None,
            None,
            "u1".into(),
            output_rx,
            cmd_tx,
            idle_timeout_secs,
            (80, 24),
        );
        Fixture {
            mgr,
            ended_rx,
            output_tx,
            cmd_rx,
            id,
        }
    }

    async fn recv_ended(rx: &mut mpsc::UnboundedReceiver<SessionEnded>) -> SessionEnded {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("SessionEnded not emitted")
            .unwrap()
    }

    #[tokio::test]
    async fn user_close_tears_down_transport_and_reports_once() {
        let mut f = fixture(0);
        let mut events = f.mgr.subscribe(&f.id).unwrap();
        let held_input = f.mgr.input_tx(&f.id).unwrap(); // a lingering WS/agent clone

        assert!(!f.mgr.remove_session(&f.id, "someone-else"));
        assert!(f.mgr.remove_session(&f.id, "u1"));

        let ended = recv_ended(&mut f.ended_rx).await;
        assert_eq!(ended.session_id, f.id);
        assert_eq!(ended.reason, REASON_USER_CLOSED);
        assert!(matches!(f.cmd_rx.recv().await, Some(SshCommand::Close)));
        assert!(matches!(
            events.recv().await,
            Ok(SessionEvent::Disconnected)
        ));
        assert!(!f.mgr.session_exists(&f.id));
        assert_eq!(f.mgr.session_count_for_user("u1"), 0);

        // Second removal is a no-op and does not emit again.
        assert!(!f.mgr.remove_session(&f.id, "u1"));
        drop(held_input);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), f.ended_rx.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn transport_exit_reports_ssh_closed() {
        let mut f = fixture(0);
        f.output_tx.send(b"hello".to_vec()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(f.mgr.get_scrollback(&f.id), vec![b"hello".to_vec()]);

        drop(f.output_tx);
        let ended = recv_ended(&mut f.ended_rx).await;
        assert_eq!(ended.reason, REASON_SSH_CLOSED);
        assert!(!f.mgr.session_exists(&f.id));
    }

    #[tokio::test]
    async fn sessions_are_recorded_without_any_client_attached() {
        let dir = std::env::temp_dir().join(format!("ngterm-sm-rec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Database::open(dir.to_str().unwrap()).unwrap();
        let store = RecordingStore::new(
            db,
            crate::recording::RecordingConfig::for_data_dir(dir.to_str().unwrap()),
        )
        .unwrap();
        let (mgr, mut ended_rx) = SessionManager::new(Some(store.clone()));
        let (output_tx, output_rx) = mpsc::channel(8);
        let (cmd_tx, _cmd_rx) = mpsc::channel(8);
        let id = mgr.create_session(
            "srv".into(),
            "alias".into(),
            "host".into(),
            None,
            None,
            "u1".into(),
            output_rx,
            cmd_tx,
            0,
            (120, 40),
        );
        // No subscriber, no WebSocket: output still has to be recorded.
        output_tx.send(b"login: ".to_vec()).await.unwrap();
        let input = mgr.input_tx(&id).unwrap();
        input
            .send(SessionInput::Data(b"root\r".to_vec()))
            .await
            .unwrap();
        input.send(SessionInput::Resize(100, 30)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(output_tx);

        let ended = recv_ended(&mut ended_rx).await;
        assert_eq!(ended.reason, REASON_SSH_CLOSED);
        assert_eq!(ended.recording, Some(Integrity::Complete));

        let recs = store.list_for_session(&id).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!((recs[0].cols, recs[0].rows), (120, 40));
        let events = store.read_events(&recs[0].recording_id).unwrap();
        use crate::recording::EventKind;
        assert!(matches!(
            events[0].kind,
            EventKind::Resize {
                cols: 120,
                rows: 40
            }
        ));
        assert!(events
            .iter()
            .any(|e| e.kind == EventKind::Output(b"login: ".to_vec())));
        assert!(events.iter().any(|e| matches!(
            e.kind,
            EventKind::Input {
                bytes: 5,
                data: None
            }
        )));
        assert!(events.iter().any(|e| matches!(
            e.kind,
            EventKind::Resize {
                cols: 100,
                rows: 30
            }
        )));
    }

    #[tokio::test]
    async fn idle_sessions_without_clients_are_closed() {
        let mut f = fixture(1);
        // Attached client: never idle-closed.
        f.mgr.ws_connected(&f.id);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        SessionManager::close_idle_sessions(&f.mgr.sessions);
        assert!(f.mgr.session_exists(&f.id));

        // Detached and idle past the timeout: closed with a distinct reason.
        f.mgr.ws_disconnected(&f.id);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        SessionManager::close_idle_sessions(&f.mgr.sessions);
        let ended = recv_ended(&mut f.ended_rx).await;
        assert_eq!(ended.reason, REASON_IDLE_TIMEOUT);
        assert!(matches!(f.cmd_rx.recv().await, Some(SshCommand::Close)));
    }
}
