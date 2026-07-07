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

pub struct HelperConnection {
    handle: client::Handle<SshClient>,
    sftp: Option<SftpBridge>,
    users: u32,
}

impl HelperConnection {
    pub async fn exec(&mut self, command: &str) -> Result<String, String> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| format!("Channel open failed: {}", e))?;

        channel
            .exec(true, command)
            .await
            .map_err(|e| format!("Exec failed: {}", e))?;

        let mut output = Vec::new();
        let mut channel = channel;
        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Data { data }) => {
                    output.extend_from_slice(&data);
                }
                Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => {
                    break;
                }
                _ => {}
            }
        }

        String::from_utf8(output).map_err(|e| format!("Invalid UTF-8 output: {}", e))
    }

    pub async fn exec_binary(&mut self, command: &str) -> Result<Vec<u8>, String> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| format!("Channel open failed: {}", e))?;

        channel
            .exec(true, command)
            .await
            .map_err(|e| format!("Exec failed: {}", e))?;

        let mut output = Vec::new();
        let mut channel = channel;
        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Data { data }) => {
                    output.extend_from_slice(&data);
                }
                Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => {
                    break;
                }
                _ => {}
            }
        }

        Ok(output)
    }

    async fn ensure_sftp(&mut self) -> Result<&SftpBridge, String> {
        if self.sftp.is_none() {
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

            self.sftp = Some(SftpBridge::new(sftp_session));
        }
        Ok(self.sftp.as_ref().unwrap())
    }
}

pub struct HelperPool {
    connections: Mutex<HashMap<String, HelperConnection>>,
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

    pub async fn open_shell_channel(
        &self,
        session_id: &str,
    ) -> Result<russh::Channel<client::Msg>, String> {
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;

        let channel = conn
            .handle
            .channel_open_session()
            .await
            .map_err(|e| format!("Channel open failed: {}", e))?;

        channel
            .request_shell(false)
            .await
            .map_err(|e| format!("Shell request failed: {}", e))?;

        Ok(channel)
    }

    pub async fn register_handle(&self, session_id: &str, handle: client::Handle<SshClient>) {
        let mut conns = self.connections.lock().await;
        if !conns.contains_key(session_id) {
            conns.insert(
                session_id.to_string(),
                HelperConnection {
                    handle,
                    sftp: None,
                    users: 1,
                },
            );
        }
    }

    pub async fn get_or_connect(&self, session_id: &str, info: &ConnectInfo) -> Result<(), String> {
        let mut conns = self.connections.lock().await;
        if conns.contains_key(session_id) {
            return Ok(());
        }

        let handle = Self::create_connection(info).await?;
        conns.insert(
            session_id.to_string(),
            HelperConnection {
                handle,
                sftp: None,
                users: 1,
            },
        );
        Ok(())
    }

    pub async fn exec(&self, session_id: &str, command: &str) -> Result<String, String> {
        if self.is_local(session_id).await {
            return Self::exec_local(command).await;
        }
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        conn.exec(command).await
    }

    pub async fn exec_binary(&self, session_id: &str, command: &str) -> Result<Vec<u8>, String> {
        if self.is_local(session_id).await {
            return Self::exec_local(command).await.map(|s| s.into_bytes());
        }
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        conn.exec_binary(command).await
    }

    pub async fn remove(&self, session_id: &str) {
        let mut conns = self.connections.lock().await;
        if let Some(conn) = conns.get_mut(session_id) {
            conn.users = conn.users.saturating_sub(1);
            if conn.users == 0 {
                let conn = conns.remove(session_id).unwrap();
                let _ = conn
                    .handle
                    .disconnect(russh::Disconnect::ByApplication, "", "en")
                    .await;
            }
        }
        self.local_sessions.lock().await.remove(session_id);
    }

    pub async fn add_user(&self, session_id: &str) {
        let mut conns = self.connections.lock().await;
        if let Some(conn) = conns.get_mut(session_id) {
            conn.users += 1;
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
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;

        let channel = conn
            .handle
            .channel_open_session()
            .await
            .map_err(|e| format!("Channel open failed: {}", e))?;

        let wrapped = format!("$SHELL -lic {}", crate::utils::shell_escape(command));
        channel
            .exec(true, wrapped.as_bytes())
            .await
            .map_err(|e| format!("Exec failed: {}", e))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code: Option<u32> = None;
        let mut channel = channel;
        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Data { data }) => stdout.extend_from_slice(&data),
                Some(russh::ChannelMsg::ExtendedData { data, ext: 1 }) => {
                    stderr.extend_from_slice(&data)
                }
                Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                    exit_code = Some(exit_status)
                }
                Some(russh::ChannelMsg::Eof) => {}
                Some(russh::ChannelMsg::Close) | None => break,
                _ => {}
            }
        }

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
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;

        let channel = conn
            .handle
            .channel_open_session()
            .await
            .map_err(|e| format!("Agent channel open failed: {}", e))?;

        channel
            .exec(true, command.as_bytes())
            .await
            .map_err(|e| format!("Agent exec failed: {}", e))?;

        Ok(channel)
    }

    // --- SFTP operations ---

    pub async fn sftp_list(&self, session_id: &str, path: &str) -> Result<Vec<FileEntry>, String> {
        if self.is_local(session_id).await {
            return local_fs::list_dir(path).await;
        }
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        let sftp = conn.ensure_sftp().await?;
        sftp.list_dir(path).await
    }

    pub async fn sftp_read(&self, session_id: &str, path: &str) -> Result<Vec<u8>, String> {
        if self.is_local(session_id).await {
            return local_fs::read_file(path).await;
        }
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        let sftp = conn.ensure_sftp().await?;
        sftp.read_file(path).await
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
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        let sftp = conn.ensure_sftp().await?;
        sftp.write_file(path, data).await
    }

    pub async fn sftp_delete(&self, session_id: &str, path: &str) -> Result<(), String> {
        if self.is_local(session_id).await {
            return local_fs::delete(path).await;
        }
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        let sftp = conn.ensure_sftp().await?;
        sftp.delete(path).await
    }

    pub async fn sftp_mkdir(&self, session_id: &str, path: &str) -> Result<(), String> {
        if self.is_local(session_id).await {
            return local_fs::mkdir(path).await;
        }
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        let sftp = conn.ensure_sftp().await?;
        sftp.mkdir(path).await
    }

    pub async fn sftp_rename(&self, session_id: &str, from: &str, to: &str) -> Result<(), String> {
        if self.is_local(session_id).await {
            return local_fs::rename(from, to).await;
        }
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        let sftp = conn.ensure_sftp().await?;
        sftp.rename(from, to).await
    }

    pub async fn sftp_stat(&self, session_id: &str, path: &str) -> Result<FileEntry, String> {
        if self.is_local(session_id).await {
            return local_fs::stat(path).await;
        }
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        let sftp = conn.ensure_sftp().await?;
        sftp.stat(path).await
    }

    pub async fn sftp_realpath(&self, session_id: &str, path: &str) -> Result<String, String> {
        if self.is_local(session_id).await {
            return local_fs::realpath(path).await;
        }
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        let sftp = conn.ensure_sftp().await?;
        sftp.realpath(path).await
    }

    pub async fn open_read(&self, session_id: &str, path: &str) -> Result<ReadableFile, String> {
        if self.is_local(session_id).await {
            let expanded = local_fs::expand_path(path);
            let file = tokio::fs::File::open(&expanded)
                .await
                .map_err(|e| format!("open file failed: {}", e))?;
            return Ok(ReadableFile::Local(file));
        }
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        let sftp = conn.ensure_sftp().await?;
        let file = sftp.open_for_read(path).await?;
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
        let mut conns = self.connections.lock().await;
        let conn = conns
            .get_mut(session_id)
            .ok_or_else(|| "Helper connection not found".to_string())?;
        let sftp = conn.ensure_sftp().await?;
        let file = sftp.open_for_write(path).await?;
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
