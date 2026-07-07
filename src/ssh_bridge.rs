use russh::client;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub struct SshClient {
    expected_fingerprint: Option<String>,
    actual_fingerprint: Arc<Mutex<Option<String>>>,
}

impl SshClient {
    pub fn new(expected_fingerprint: Option<String>) -> (Self, Arc<Mutex<Option<String>>>) {
        let actual = Arc::new(Mutex::new(None));
        (
            Self {
                expected_fingerprint,
                actual_fingerprint: actual.clone(),
            },
            actual,
        )
    }
}

impl client::Handler for SshClient {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key.fingerprint(HashAlg::Sha256).to_string();
        *self.actual_fingerprint.lock().await = Some(fingerprint.clone());

        match &self.expected_fingerprint {
            None => Ok(true),
            Some(expected) => {
                if expected == &fingerprint {
                    Ok(true)
                } else {
                    tracing::warn!(
                        "SSH host key mismatch! Expected: {}, Got: {}",
                        expected,
                        fingerprint
                    );
                    Ok(false)
                }
            }
        }
    }
}

pub enum SshCommand {
    Data(Vec<u8>),
    Resize(u32, u32),
    Close,
}

pub struct SshSession {
    pub cmd_tx: mpsc::Sender<SshCommand>,
    pub output_rx: mpsc::Receiver<Vec<u8>>,
    pub host_key_fingerprint: String,
    pub handle: client::Handle<SshClient>,
}

pub async fn connect(
    host: &str,
    port: u16,
    username: &str,
    private_key_pem: &str,
    cols: u32,
    rows: u32,
    expected_fingerprint: Option<String>,
) -> Result<SshSession, String> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    });
    let (handler, actual_fingerprint) = SshClient::new(expected_fingerprint);

    tracing::info!("SSH: connecting to {}:{}", host, port);

    let mut handle = client::connect(config, (host, port), handler)
        .await
        .map_err(|e| format!("SSH connect failed: {}", e))?;

    let fingerprint = actual_fingerprint.lock().await.clone().unwrap_or_default();

    if fingerprint.is_empty() {
        return Err("SSH host key verification failed".to_string());
    }

    tracing::info!("SSH: TCP connected, host key: {}", fingerprint);

    let key_pair = russh::keys::decode_secret_key(private_key_pem, None)
        .map_err(|e| format!("Failed to decode private key: {}", e))?;

    tracing::info!(
        "SSH: key decoded (algo={}), authenticating as '{}'",
        key_pair.algorithm(),
        username
    );

    let key_with_alg = PrivateKeyWithHashAlg::new(Arc::new(key_pair), Some(HashAlg::Sha512));

    let auth_result = handle
        .authenticate_publickey(username, key_with_alg)
        .await
        .map_err(|e| format!("SSH auth failed: {}", e))?;

    if !auth_result.success() {
        tracing::error!(
            "SSH: authentication rejected by server for user '{}'",
            username
        );
        return Err("SSH authentication rejected".to_string());
    }

    tracing::info!("SSH: authenticated, opening channel");

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("Channel open failed: {}", e))?;

    channel
        .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
        .await
        .map_err(|e| format!("PTY request failed: {}", e))?;

    channel
        .request_shell(false)
        .await
        .map_err(|e| format!("Shell request failed: {}", e))?;

    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(256);
    let (cmd_tx, cmd_rx) = mpsc::channel::<SshCommand>(64);

    tokio::spawn(channel_loop(channel, output_tx, cmd_rx));

    Ok(SshSession {
        cmd_tx,
        output_rx,
        host_key_fingerprint: fingerprint,
        handle,
    })
}

async fn channel_loop(
    mut channel: russh::Channel<client::Msg>,
    output_tx: mpsc::Sender<Vec<u8>>,
    mut cmd_rx: mpsc::Receiver<SshCommand>,
) {
    loop {
        tokio::select! {
            msg = channel.wait() => {
                match msg {
                    Some(russh::ChannelMsg::Data { data }) => {
                        if output_tx.send(data.to_vec()).await.is_err() {
                            break;
                        }
                    }
                    Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => {
                        break;
                    }
                    _ => {}
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SshCommand::Data(data)) => {
                        if channel.data(&data[..]).await.is_err() {
                            break;
                        }
                    }
                    Some(SshCommand::Resize(cols, rows)) => {
                        let _ = channel.window_change(cols, rows, 0, 0).await;
                    }
                    Some(SshCommand::Close) | None => {
                        let _ = channel.eof().await;
                        break;
                    }
                }
            }
        }
    }
}
