use russh::client;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg};
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::Mutex;

use crate::sftp_bridge::{FileEntry, SftpBridge};
use crate::ssh_bridge::SshClient;

pub enum ReadableFile {
    Sftp(russh_sftp::client::fs::File),
    Local(tokio::fs::File),
}

impl AsyncRead for ReadableFile {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ReadableFile::Sftp(f) => Pin::new(f).poll_read(cx, buf),
            ReadableFile::Local(f) => Pin::new(f).poll_read(cx, buf),
        }
    }
}

pub enum WritableFile {
    Sftp(russh_sftp::client::fs::File),
    Local(tokio::fs::File),
}

impl AsyncWrite for WritableFile {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            WritableFile::Sftp(f) => Pin::new(f).poll_write(cx, buf),
            WritableFile::Local(f) => Pin::new(f).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WritableFile::Sftp(f) => Pin::new(f).poll_flush(cx),
            WritableFile::Local(f) => Pin::new(f).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WritableFile::Sftp(f) => Pin::new(f).poll_shutdown(cx),
            WritableFile::Local(f) => Pin::new(f).poll_shutdown(cx),
        }
    }
}

/// One SSH connection shared by everything that operates on a session
/// (terminal helpers, SFTP, agent shells). Channel opens are performed under
/// the connection's own lock; waiting for command output happens outside it,
/// so a long-running command on one session never blocks another session and
/// several commands can be in flight on the same connection.
pub struct HelperConnection {
    handle: client::Handle<SshClient>,
    sftp: Option<Arc<SftpBridge>>,
}

/// Read every message of an exec channel until it closes.
async fn drain_exec_channel(
    mut channel: russh::Channel<client::Msg>,
) -> (Vec<u8>, Vec<u8>, Option<u32>) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code: Option<u32> = None;
    loop {
        match channel.wait().await {
            Some(russh::ChannelMsg::Data { data }) => stdout.extend_from_slice(&data),
            Some(russh::ChannelMsg::ExtendedData { data, ext: 1 }) => {
                stderr.extend_from_slice(&data)
            }
            Some(russh::ChannelMsg::ExitStatus { exit_status }) => exit_code = Some(exit_status),
            Some(russh::ChannelMsg::Eof) => {}
            Some(russh::ChannelMsg::Close) | None => break,
            _ => {}
        }
    }
    (stdout, stderr, exit_code)
}

impl HelperConnection {
    async fn ensure_sftp(&mut self) -> Result<Arc<SftpBridge>, String> {
        if let Some(sftp) = &self.sftp {
            return Ok(sftp.clone());
        }
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| format!("SFTP channel open failed: {}", e))?;

        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| format!("SFTP subsystem request failed: {}", e))?;

        let sftp_session = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| format!("SFTP session init failed: {}", e))?;

        let sftp = Arc::new(SftpBridge::new(sftp_session));
        self.sftp = Some(sftp.clone());
        Ok(sftp)
    }
}

enum ChannelKind<'a> {
    Shell,
    Exec(&'a [u8]),
}

struct Slot {
    conn: Arc<Mutex<HelperConnection>>,
    users: u32,
}

/// Registry of helper SSH connections keyed by terminal session id.
///
/// The map lock is only ever held for bookkeeping; every network operation
/// (connecting, opening channels, SFTP calls, waiting for output) runs with
/// at most the per-connection lock held, so a slow or unreachable server
/// cannot stall unrelated sessions.
pub struct HelperPool {
    connections: Mutex<HashMap<String, Slot>>,
    local_sessions: Mutex<HashSet<String>>,
}

pub struct ConnectInfo {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub private_key_pem: String,
}

impl Default for HelperPool {
    fn default() -> Self {
        Self::new()
    }
}

const NOT_FOUND: &str = "Helper connection not found";

impl HelperPool {
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
            local_sessions: Mutex::new(HashSet::new()),
        }
    }

    pub async fn register_local(&self, session_id: &str) {
        self.local_sessions
            .lock()
            .await
            .insert(session_id.to_string());
    }

    pub async fn is_local(&self, session_id: &str) -> bool {
        self.local_sessions.lock().await.contains(session_id)
    }

    /// Look up the connection for a session, holding the map lock only for
    /// the lookup itself.
    async fn connection(&self, session_id: &str) -> Result<Arc<Mutex<HelperConnection>>, String> {
        self.connections
            .lock()
            .await
            .get(session_id)
            .map(|slot| slot.conn.clone())
            .ok_or_else(|| NOT_FOUND.to_string())
    }

    /// Open a channel of the given kind. The connection lock is released as
    /// soon as the channel exists.
    async fn open_channel(
        &self,
        session_id: &str,
        kind: ChannelKind<'_>,
    ) -> Result<russh::Channel<client::Msg>, String> {
        let conn = self.connection(session_id).await?;
        let guard = conn.lock().await;
        let channel = guard
            .handle
            .channel_open_session()
            .await
            .map_err(|e| format!("Channel open failed: {}", e))?;
        match kind {
            ChannelKind::Shell => channel
                .request_shell(false)
                .await
                .map_err(|e| format!("Shell request failed: {}", e))?,
            ChannelKind::Exec(command) => channel
                .exec(true, command)
                .await
                .map_err(|e| format!("Exec failed: {}", e))?,
        }
        Ok(channel)
    }

    async fn sftp(&self, session_id: &str) -> Result<Arc<SftpBridge>, String> {
        let conn = self.connection(session_id).await?;
        let mut guard = conn.lock().await;
        guard.ensure_sftp().await
    }

    pub async fn open_shell_channel(
        &self,
        session_id: &str,
    ) -> Result<russh::Channel<client::Msg>, String> {
        self.open_channel(session_id, ChannelKind::Shell).await
    }

    fn insert_if_absent(
        conns: &mut HashMap<String, Slot>,
        session_id: &str,
        handle: client::Handle<SshClient>,
    ) -> Option<client::Handle<SshClient>> {
        if conns.contains_key(session_id) {
            return Some(handle);
        }
        conns.insert(
            session_id.to_string(),
            Slot {
                conn: Arc::new(Mutex::new(HelperConnection { handle, sftp: None })),
                users: 1,
            },
        );
        None
    }

    pub async fn register_handle(&self, session_id: &str, handle: client::Handle<SshClient>) {
        let mut conns = self.connections.lock().await;
        if let Some(extra) = Self::insert_if_absent(&mut conns, session_id, handle) {
            drop(conns);
            let _ = extra
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        }
    }

    /// Ensure a helper connection exists for the session. The SSH handshake
    /// runs without holding the map lock; if another caller won the race the
    /// redundant connection is closed again.
    pub async fn get_or_connect(&self, session_id: &str, info: &ConnectInfo) -> Result<(), String> {
        if self.connections.lock().await.contains_key(session_id) {
            return Ok(());
        }

        let handle = Self::create_connection(info).await?;
        let extra = {
            let mut conns = self.connections.lock().await;
            Self::insert_if_absent(&mut conns, session_id, handle)
        };
        if let Some(extra) = extra {
            let _ = extra
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        }
        Ok(())
    }

    pub async fn exec(&self, session_id: &str, command: &str) -> Result<String, String> {
        if self.is_local(session_id).await {
            return Self::exec_local(command).await;
        }
        let channel = self
            .open_channel(session_id, ChannelKind::Exec(command.as_bytes()))
            .await?;
        let (stdout, _, _) = drain_exec_channel(channel).await;
        String::from_utf8(stdout).map_err(|e| format!("Invalid UTF-8 output: {}", e))
    }

    pub async fn exec_binary(&self, session_id: &str, command: &str) -> Result<Vec<u8>, String> {
        if self.is_local(session_id).await {
            return Self::exec_local(command).await.map(|s| s.into_bytes());
        }
        let channel = self
            .open_channel(session_id, ChannelKind::Exec(command.as_bytes()))
            .await?;
        let (stdout, _, _) = drain_exec_channel(channel).await;
        Ok(stdout)
    }

    pub async fn remove(&self, session_id: &str) {
        let to_close = {
            let mut conns = self.connections.lock().await;
            match conns.get_mut(session_id) {
                Some(slot) => {
                    slot.users = slot.users.saturating_sub(1);
                    if slot.users == 0 {
                        conns.remove(session_id).map(|slot| slot.conn)
                    } else {
                        None
                    }
                }
                None => None,
            }
        };
        self.local_sessions.lock().await.remove(session_id);

        if let Some(conn) = to_close {
            // Waits only for in-flight channel opens on this very connection.
            let guard = conn.lock().await;
            let _ = guard
                .handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        }
    }

    pub async fn add_user(&self, session_id: &str) {
        let mut conns = self.connections.lock().await;
        if let Some(slot) = conns.get_mut(session_id) {
            slot.users += 1;
        }
    }

    pub async fn remove_user(&self, session_id: &str) {
        self.remove(session_id).await;
    }

    pub async fn exec_with_status(
        &self,
        session_id: &str,
        command: &str,
    ) -> Result<(String, u32), String> {
        if self.is_local(session_id).await {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output()
                .await
                .map_err(|e| format!("Local exec failed: {}", e))?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let code = output.status.code().unwrap_or(1) as u32;
            return Ok((stdout, code));
        }

        let wrapped = format!("$SHELL -lic {}", crate::utils::shell_escape(command));
        let channel = self
            .open_channel(session_id, ChannelKind::Exec(wrapped.as_bytes()))
            .await?;
        let (stdout, stderr, exit_code) = drain_exec_channel(channel).await;

        let output = if stdout.is_empty() {
            String::from_utf8_lossy(&stderr).to_string()
        } else {
            String::from_utf8_lossy(&stdout).to_string()
        };

        Ok((output, exit_code.unwrap_or(1)))
    }

    pub async fn open_exec_channel(
        &self,
        session_id: &str,
        command: &str,
    ) -> Result<russh::Channel<client::Msg>, String> {
        self.open_channel(session_id, ChannelKind::Exec(command.as_bytes()))
            .await
    }

    // --- SFTP operations ---

    pub async fn sftp_list(&self, session_id: &str, path: &str) -> Result<Vec<FileEntry>, String> {
        if self.is_local(session_id).await {
            return local_fs::list_dir(path).await;
        }
        self.sftp(session_id).await?.list_dir(path).await
    }

    pub async fn sftp_read(&self, session_id: &str, path: &str) -> Result<Vec<u8>, String> {
        if self.is_local(session_id).await {
            return local_fs::read_file(path).await;
        }
        self.sftp(session_id).await?.read_file(path).await
    }

    pub async fn sftp_write(
        &self,
        session_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), String> {
        if self.is_local(session_id).await {
            return local_fs::write_file(path, data).await;
        }
        self.sftp(session_id).await?.write_file(path, data).await
    }

    pub async fn sftp_delete(&self, session_id: &str, path: &str) -> Result<(), String> {
        if self.is_local(session_id).await {
            return local_fs::delete(path).await;
        }
        self.sftp(session_id).await?.delete(path).await
    }

    pub async fn sftp_mkdir(&self, session_id: &str, path: &str) -> Result<(), String> {
        if self.is_local(session_id).await {
            return local_fs::mkdir(path).await;
        }
        self.sftp(session_id).await?.mkdir(path).await
    }

    pub async fn sftp_rename(&self, session_id: &str, from: &str, to: &str) -> Result<(), String> {
        if self.is_local(session_id).await {
            return local_fs::rename(from, to).await;
        }
        self.sftp(session_id).await?.rename(from, to).await
    }

    pub async fn sftp_stat(&self, session_id: &str, path: &str) -> Result<FileEntry, String> {
        if self.is_local(session_id).await {
            return local_fs::stat(path).await;
        }
        self.sftp(session_id).await?.stat(path).await
    }

    pub async fn sftp_realpath(&self, session_id: &str, path: &str) -> Result<String, String> {
        if self.is_local(session_id).await {
            return local_fs::realpath(path).await;
        }
        self.sftp(session_id).await?.realpath(path).await
    }

    pub async fn open_read(&self, session_id: &str, path: &str) -> Result<ReadableFile, String> {
        if self.is_local(session_id).await {
            let expanded = local_fs::expand_path(path);
            let file = tokio::fs::File::open(&expanded)
                .await
                .map_err(|e| format!("open file failed: {}", e))?;
            return Ok(ReadableFile::Local(file));
        }
        let file = self.sftp(session_id).await?.open_for_read(path).await?;
        Ok(ReadableFile::Sftp(file))
    }

    pub async fn open_write(&self, session_id: &str, path: &str) -> Result<WritableFile, String> {
        if self.is_local(session_id).await {
            let expanded = local_fs::expand_path(path);
            let file = tokio::fs::File::create(&expanded)
                .await
                .map_err(|e| format!("create file failed: {}", e))?;
            return Ok(WritableFile::Local(file));
        }
        let file = self.sftp(session_id).await?.open_for_write(path).await?;
        Ok(WritableFile::Sftp(file))
    }

    async fn exec_local(command: &str) -> Result<String, String> {
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await
            .map_err(|e| format!("Local exec failed: {}", e))?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn create_connection(info: &ConnectInfo) -> Result<client::Handle<SshClient>, String> {
        let config = Arc::new(client::Config {
            keepalive_interval: Some(std::time::Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        });

        let (handler, _fingerprint) = SshClient::new(None);

        let mut handle = client::connect(config, (&*info.host, info.port), handler)
            .await
            .map_err(|e| format!("Helper SSH connect failed: {}", e))?;

        let key_pair = russh::keys::decode_secret_key(&info.private_key_pem, None)
            .map_err(|e| format!("Failed to decode private key: {}", e))?;

        let key_with_alg = PrivateKeyWithHashAlg::new(Arc::new(key_pair), Some(HashAlg::Sha512));

        let auth_result = handle
            .authenticate_publickey(&info.username, key_with_alg)
            .await
            .map_err(|e| format!("Helper SSH auth failed: {}", e))?;

        if !auth_result.success() {
            return Err("Helper SSH authentication rejected".to_string());
        }

        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A TCP listener that accepts and then never speaks: the SSH handshake
    /// against it hangs until the client gives up.
    async fn silent_listener() -> (tokio::net::TcpListener, u16) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    #[tokio::test]
    async fn slow_connect_does_not_block_other_sessions() {
        let (listener, port) = silent_listener().await;
        let accept = tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let pool = Arc::new(HelperPool::new());
        let slow_pool = pool.clone();
        let slow = tokio::spawn(async move {
            let info = ConnectInfo {
                host: "127.0.0.1".into(),
                port,
                username: "nobody".into(),
                private_key_pem: String::new(),
            };
            slow_pool.get_or_connect("slow", &info).await
        });

        // Give the slow connect time to reach the (hanging) handshake.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !slow.is_finished(),
            "handshake against a silent peer must still be pending"
        );

        // Unrelated session bookkeeping and lookups must not wait for it.
        let unrelated = async {
            pool.register_local("local").await;
            assert!(pool.is_local("local").await);
            let (out, code) = pool.exec_with_status("local", "echo ok").await.unwrap();
            assert_eq!(out.trim(), "ok");
            assert_eq!(code, 0);
            let err = pool.open_shell_channel("other").await.unwrap_err();
            assert_eq!(err, NOT_FOUND);
            pool.add_user("other").await;
            pool.remove("other").await;
        };
        tokio::time::timeout(Duration::from_secs(2), unrelated)
            .await
            .expect("operations on unrelated sessions stalled behind a slow SSH connect");

        slow.abort();
        accept.abort();
    }
}

mod local_fs {
    use super::FileEntry;
    use chrono::{DateTime, Utc};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    pub(super) fn expand_path(path: &str) -> String {
        if path == "~" || path.starts_with("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            if path == "~" {
                home
            } else {
                format!("{}{}", home, &path[1..])
            }
        } else if path == "." || path.is_empty() {
            std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
        } else {
            path.to_string()
        }
    }

    pub async fn list_dir(path: &str) -> Result<Vec<FileEntry>, String> {
        let path = expand_path(path);
        let mut entries = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| format!("read_dir failed: {}", e))?;

        let mut result = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| format!("read entry failed: {}", e))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry
                .metadata()
                .await
                .map_err(|e| format!("metadata failed: {}", e))?;

            let file_type = if metadata.is_dir() {
                "directory"
            } else if metadata.file_type().is_symlink() {
                "symlink"
            } else {
                "file"
            };

            let mode = metadata.permissions().mode();
            let permissions = format_permissions(mode);

            let modified: DateTime<Utc> = metadata.modified().map(|t| t.into()).unwrap_or_default();

            result.push(FileEntry {
                name,
                file_type: file_type.to_string(),
                size: metadata.len(),
                permissions,
                owner: metadata.uid().to_string(),
                group: metadata.gid().to_string(),
                modified: modified.to_rfc3339(),
            });
        }
        Ok(result)
    }

    pub async fn read_file(path: &str) -> Result<Vec<u8>, String> {
        let path = expand_path(path);
        tokio::fs::read(&path)
            .await
            .map_err(|e| format!("read file failed: {}", e))
    }

    pub async fn write_file(path: &str, data: &[u8]) -> Result<(), String> {
        let path = expand_path(path);
        tokio::fs::write(&path, data)
            .await
            .map_err(|e| format!("write file failed: {}", e))
    }

    pub async fn delete(path: &str) -> Result<(), String> {
        let path = expand_path(path);
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|e| format!("stat failed: {}", e))?;
        if metadata.is_dir() {
            tokio::fs::remove_dir_all(&path)
                .await
                .map_err(|e| format!("rmdir failed: {}", e))
        } else {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| format!("remove file failed: {}", e))
        }
    }

    pub async fn mkdir(path: &str) -> Result<(), String> {
        let path = expand_path(path);
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| format!("mkdir failed: {}", e))
    }

    pub async fn rename(from: &str, to: &str) -> Result<(), String> {
        let from = expand_path(from);
        let to = expand_path(to);
        tokio::fs::rename(&from, &to)
            .await
            .map_err(|e| format!("rename failed: {}", e))
    }

    pub async fn stat(path: &str) -> Result<FileEntry, String> {
        let path = expand_path(path);
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|e| format!("stat failed: {}", e))?;

        let file_type = if metadata.is_dir() {
            "directory"
        } else if metadata.file_type().is_symlink() {
            "symlink"
        } else {
            "file"
        };

        let name = std::path::Path::new(path.as_str())
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());

        let mode = metadata.permissions().mode();
        let permissions = format_permissions(mode);

        let modified: DateTime<Utc> = metadata.modified().map(|t| t.into()).unwrap_or_default();

        Ok(FileEntry {
            name,
            file_type: file_type.to_string(),
            size: metadata.len(),
            permissions,
            owner: metadata.uid().to_string(),
            group: metadata.gid().to_string(),
            modified: modified.to_rfc3339(),
        })
    }

    pub async fn realpath(path: &str) -> Result<String, String> {
        let path = expand_path(path);
        tokio::fs::canonicalize(&path)
            .await
            .map(|p| p.to_string_lossy().to_string())
            .map_err(|e| format!("realpath failed: {}", e))
    }

    fn format_permissions(mode: u32) -> String {
        let mut s = String::with_capacity(9);
        let flags = [
            (0o400, 'r'),
            (0o200, 'w'),
            (0o100, 'x'),
            (0o040, 'r'),
            (0o020, 'w'),
            (0o010, 'x'),
            (0o004, 'r'),
            (0o002, 'w'),
            (0o001, 'x'),
        ];
        for (bit, ch) in flags {
            s.push(if mode & bit != 0 { ch } else { '-' });
        }
        s
    }
}
