use std::os::fd::{AsRawFd, IntoRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use tokio::io::unix::AsyncFd;
use tokio::io::Interest;
use tokio::sync::mpsc;

use crate::ssh_bridge::SshCommand;

pub struct LocalPtySession {
    pub cmd_tx: mpsc::Sender<SshCommand>,
    pub output_rx: mpsc::Receiver<Vec<u8>>,
}

pub fn spawn(cols: u16, rows: u16) -> Result<LocalPtySession, String> {
    let ws = nix::pty::Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let pty = nix::pty::openpty(Some(&ws), None).map_err(|e| format!("openpty failed: {}", e))?;
    let master = pty.master;
    let slave = pty.slave;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

    // Consume slave OwnedFd to get the raw fd without auto-closing
    let slave_fd = slave.into_raw_fd();

    let child = unsafe {
        std::process::Command::new(&shell)
            .arg("-l")
            .env("TERM", "xterm-256color")
            .pre_exec(move || {
                nix::unistd::setsid().map_err(std::io::Error::other)?;
                libc::dup2(slave_fd, 0);
                libc::dup2(slave_fd, 1);
                libc::dup2(slave_fd, 2);
                if slave_fd > 2 {
                    libc::close(slave_fd);
                }
                libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0);
                Ok(())
            })
            .spawn()
            .map_err(|e| format!("Failed to spawn shell: {}", e))?
    };

    // Close slave fd in parent
    unsafe { libc::close(slave_fd) };

    let master_fd = master.as_raw_fd();
    set_nonblocking(master_fd).map_err(|e| format!("set_nonblocking: {}", e))?;

    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(256);
    let (cmd_tx, cmd_rx) = mpsc::channel::<SshCommand>(64);

    let async_master =
        AsyncFd::new(master).map_err(|e| format!("AsyncFd creation failed: {}", e))?;

    tokio::spawn(pty_loop(async_master, output_tx, cmd_rx, child));

    Ok(LocalPtySession { cmd_tx, output_rx })
}

fn set_nonblocking(fd: i32) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err("fcntl F_GETFL failed".to_string());
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        return Err("fcntl F_SETFL failed".to_string());
    }
    Ok(())
}

async fn pty_loop(
    master: AsyncFd<OwnedFd>,
    output_tx: mpsc::Sender<Vec<u8>>,
    mut cmd_rx: mpsc::Receiver<SshCommand>,
    mut child: std::process::Child,
) {
    let master_fd = master.as_raw_fd();
    let mut buf = [0u8; 4096];

    loop {
        tokio::select! {
            readable = master.ready(Interest::READABLE) => {
                match readable {
                    Ok(mut guard) => {
                        loop {
                            let n = unsafe {
                                libc::read(
                                    master_fd,
                                    buf.as_mut_ptr() as *mut libc::c_void,
                                    buf.len(),
                                )
                            };
                            if n > 0 {
                                let data = buf[..n as usize].to_vec();
                                if output_tx.send(data).await.is_err() {
                                    kill_child(&mut child);
                                    return;
                                }
                            } else if n == 0 {
                                return;
                            } else {
                                let err = std::io::Error::last_os_error();
                                if err.kind() == std::io::ErrorKind::WouldBlock {
                                    guard.clear_ready();
                                    break;
                                } else if err.raw_os_error() == Some(libc::EIO) {
                                    // PTY closed (child exited)
                                    return;
                                } else {
                                    tracing::error!("PTY read error: {}", err);
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("PTY ready error: {}", e);
                        return;
                    }
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SshCommand::Data(data)) => {
                        let mut offset = 0;
                        while offset < data.len() {
                            let n = unsafe {
                                libc::write(
                                    master_fd,
                                    data[offset..].as_ptr() as *const libc::c_void,
                                    data.len() - offset,
                                )
                            };
                            if n > 0 {
                                offset += n as usize;
                            } else {
                                break;
                            }
                        }
                    }
                    Some(SshCommand::Resize(cols, rows)) => {
                        let ws = libc::winsize {
                            ws_row: rows as u16,
                            ws_col: cols as u16,
                            ws_xpixel: 0,
                            ws_ypixel: 0,
                        };
                        unsafe {
                            libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws);
                        }
                    }
                    Some(SshCommand::Close) | None => {
                        kill_child(&mut child);
                        return;
                    }
                }
            }
        }
    }
}

fn kill_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}
