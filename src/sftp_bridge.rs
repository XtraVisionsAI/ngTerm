#![allow(dead_code)]

use chrono::{DateTime, TimeZone, Utc};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileType, OpenFlags};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub file_type: String,
    pub size: u64,
    pub permissions: String,
    pub owner: String,
    pub group: String,
    pub modified: String,
}

pub struct SftpBridge {
    sftp: SftpSession,
}

impl SftpBridge {
    pub fn new(sftp: SftpSession) -> Self {
        Self { sftp }
    }

    pub async fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, String> {
        let entries = self
            .sftp
            .read_dir(path)
            .await
            .map_err(|e| format!("SFTP read_dir failed: {}", e))?;

        let mut result = Vec::new();
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let metadata = entry.metadata();
            let file_type = match entry.file_type() {
                FileType::Dir => "directory",
                FileType::File => "file",
                FileType::Symlink => "symlink",
                FileType::Other => "unknown",
            };

            let permissions = format_permissions(metadata.permissions);
            let owner = metadata.user.clone().unwrap_or_default();
            let group = metadata.group.clone().unwrap_or_default();
            let modified = metadata
                .mtime
                .map(|t| {
                    Utc.timestamp_opt(t as i64, 0)
                        .single()
                        .map(|dt: DateTime<Utc>| dt.to_rfc3339())
                        .unwrap_or_default()
                })
                .unwrap_or_default();

            result.push(FileEntry {
                name,
                file_type: file_type.to_string(),
                size: metadata.size.unwrap_or(0),
                permissions,
                owner,
                group,
                modified,
            });
        }
        Ok(result)
    }

    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        let mut file = self
            .sftp
            .open(path)
            .await
            .map_err(|e| format!("SFTP open failed: {}", e))?;

        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .await
            .map_err(|e| format!("SFTP read failed: {}", e))?;
        Ok(buf)
    }

    pub async fn write_file(&self, path: &str, data: &[u8]) -> Result<(), String> {
        let mut file = self
            .sftp
            .create(path)
            .await
            .map_err(|e| format!("SFTP create failed: {}", e))?;

        file.write_all(data)
            .await
            .map_err(|e| format!("SFTP write failed: {}", e))?;
        file.flush()
            .await
            .map_err(|e| format!("SFTP flush failed: {}", e))?;
        Ok(())
    }

    pub async fn delete(&self, path: &str) -> Result<(), String> {
        let metadata = self
            .sftp
            .metadata(path)
            .await
            .map_err(|e| format!("SFTP stat failed: {}", e))?;

        if metadata.is_dir() {
            self.sftp
                .remove_dir(path)
                .await
                .map_err(|e| format!("SFTP rmdir failed: {}", e))
        } else {
            self.sftp
                .remove_file(path)
                .await
                .map_err(|e| format!("SFTP remove failed: {}", e))
        }
    }

    pub async fn mkdir(&self, path: &str) -> Result<(), String> {
        self.sftp
            .create_dir(path)
            .await
            .map_err(|e| format!("SFTP mkdir failed: {}", e))
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        self.sftp
            .rename(from, to)
            .await
            .map_err(|e| format!("SFTP rename failed: {}", e))
    }

    pub async fn stat(&self, path: &str) -> Result<FileEntry, String> {
        let metadata = self
            .sftp
            .metadata(path)
            .await
            .map_err(|e| format!("SFTP stat failed: {}", e))?;

        let file_type = if metadata.is_dir() {
            "directory"
        } else if metadata.is_symlink() {
            "symlink"
        } else {
            "file"
        };

        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let permissions = format_permissions(metadata.permissions);

        Ok(FileEntry {
            name,
            file_type: file_type.to_string(),
            size: metadata.size.unwrap_or(0),
            permissions,
            owner: metadata.user.clone().unwrap_or_default(),
            group: metadata.group.clone().unwrap_or_default(),
            modified: metadata
                .mtime
                .map(|t| {
                    Utc.timestamp_opt(t as i64, 0)
                        .single()
                        .map(|dt: DateTime<Utc>| dt.to_rfc3339())
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
        })
    }

    pub async fn realpath(&self, path: &str) -> Result<String, String> {
        self.sftp
            .canonicalize(path)
            .await
            .map_err(|e| format!("SFTP realpath failed: {}", e))
    }

    pub async fn open_for_read(&self, path: &str) -> Result<russh_sftp::client::fs::File, String> {
        self.sftp
            .open(path)
            .await
            .map_err(|e| format!("SFTP open failed: {}", e))
    }

    pub async fn open_for_write(&self, path: &str) -> Result<russh_sftp::client::fs::File, String> {
        self.sftp
            .open_with_flags(
                path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|e| format!("SFTP open_write failed: {}", e))
    }
}

fn format_permissions(raw: Option<u32>) -> String {
    let Some(mode) = raw else {
        return "---------".to_string();
    };

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
