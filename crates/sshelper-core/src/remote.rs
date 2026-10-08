//! SSH connection to the target host, built on `russh` (pure Rust, no system
//! `ssh` / `scp` needed, so the GUI can ask for the password itself).
//!
//! [`Connection`] exposes a blocking API; it owns a small tokio runtime whose
//! worker thread keeps the session alive (keepalives) while the UI waits for
//! the user.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::{PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate, known_hosts};
use russh::{ChannelMsg, Disconnect, MethodKind};
use russh_sftp::client::SftpSession;
use tokio::io::AsyncWriteExt;
use tokio::runtime::Runtime;

/// How the presented host key relates to `~/.ssh/known_hosts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// Recorded and matching.
    Known,
    /// Not recorded yet: show the fingerprint and ask the user.
    Unknown,
    /// A different key of the same type is recorded on this line.
    Changed { line: usize },
}

#[derive(Debug, Clone)]
pub struct HostKeyInfo {
    /// e.g. `ssh-ed25519`
    pub algorithm: String,
    /// e.g. `SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU`
    pub fingerprint: String,
    pub status: HostKeyStatus,
    key: PublicKey,
}

/// The remote operating system, which decides the installer script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteOs {
    Linux,
    Windows,
}

impl RemoteOs {
    pub fn label(self) -> &'static str {
        match self {
            RemoteOs::Linux => "Linux",
            RemoteOs::Windows => "Windows",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("連線逾時（{0} 秒），請確認主機位址、連接埠與防火牆")]
    Timeout(u64),
    #[error("無法連線到 {target}：{source}")]
    Connect { target: String, source: russh::Error },
    #[error(
        "主機金鑰與 known_hosts 第 {line} 行的紀錄不符！可能是主機重灌，也可能遭到中間人攻擊。\n\
         確認主機確實更換過金鑰後，移除舊紀錄再重試：ssh-keygen -R {known_hosts_name}"
    )]
    HostKeyChanged {
        line: usize,
        known_hosts_name: String,
        info: Box<HostKeyInfo>,
    },
    #[error("尚未信任此主機的金鑰")]
    HostKeyNotTrusted,
    #[error("帳號或密碼錯誤")]
    AuthFailed,
    #[error("伺服器不接受密碼登入（允許的方式：{0}）")]
    PasswordNotAllowed(String),
    #[error("伺服器拒絕此金鑰")]
    KeyRejected,
    #[error("無法讀取私鑰：{0}")]
    PrivateKey(russh::keys::Error),
    #[error("遠端未提供 SFTP 服務（sshd_config 的 Subsystem sftp）：{0}")]
    SftpUnavailable(String),
    #[error("SFTP 錯誤：{0}")]
    Sftp(#[from] russh_sftp::client::error::Error),
    #[error("SSH 錯誤：{0}")]
    Ssh(#[from] russh::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    seen: Arc<Mutex<Option<HostKeyInfo>>>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = server_key else {
            // Host certificates are not negotiated (Preferred::host_key_certificates is empty).
            return Ok(false);
        };
        let status = match known_hosts::check_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
            Ok(true) => HostKeyStatus::Known,
            Err(russh::keys::Error::KeyChanged { line }) => HostKeyStatus::Changed { line },
            // Unreadable or odd known_hosts lines: treat as unknown and ask.
            Ok(false) | Err(_) => HostKeyStatus::Unknown,
        };
        let accept = !matches!(status, HostKeyStatus::Changed { .. });
        *self.seen.lock().unwrap() = Some(HostKeyInfo {
            algorithm: key.algorithm().to_string(),
            fingerprint: key.fingerprint(Default::default()).to_string(),
            status,
            key: key.clone(),
        });
        // Unknown keys are accepted at the transport level; no credentials are
        // sent until the caller has confirmed the fingerprint (see
        // `authenticate_*`), which is how OpenSSH behaves too.
        Ok(accept)
    }
}

/// An SSH session to the target host.
pub struct Connection {
    // Field order matters: the session must be dropped before the runtime.
    sftp: Option<SftpSession>,
    handle: client::Handle<ClientHandler>,
    host_key: HostKeyInfo,
    host: String,
    port: u16,
    known_hosts: PathBuf,
    home: Option<String>,
    rt: Runtime,
}

impl Connection {
    /// Opens the TCP connection and runs the key exchange. Fails with
    /// [`RemoteError::HostKeyChanged`] when the key contradicts known_hosts;
    /// an unknown key is reported through [`Connection::host_key`].
    pub fn connect(host: &str, port: u16, known_hosts: &Path, timeout: Duration) -> Result<Self, RemoteError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let seen = Arc::new(Mutex::new(None));
        let handler = ClientHandler {
            host: host.to_owned(),
            port,
            known_hosts: known_hosts.to_owned(),
            seen: seen.clone(),
        };
        let config = Arc::new(client::Config {
            keepalive_interval: Some(Duration::from_secs(15)),
            ..Default::default()
        });

        // The timer must be created inside the runtime, hence the async block.
        let result =
            rt.block_on(async { tokio::time::timeout(timeout, client::connect(config, (host, port), handler)).await });
        let info = seen.lock().unwrap().take();
        let handle = match result {
            Err(_) => return Err(RemoteError::Timeout(timeout.as_secs())),
            Ok(Ok(handle)) => handle,
            Ok(Err(source)) => {
                if let Some(info) = info
                    && let HostKeyStatus::Changed { line } = info.status
                {
                    return Err(RemoteError::HostKeyChanged {
                        line,
                        known_hosts_name: known_hosts_name(host, port),
                        info: Box::new(info),
                    });
                }
                return Err(RemoteError::Connect {
                    target: known_hosts_name(host, port),
                    source,
                });
            }
        };
        let host_key = info.ok_or(RemoteError::HostKeyNotTrusted)?;
        Ok(Self {
            sftp: None,
            handle,
            host_key,
            host: host.to_owned(),
            port,
            known_hosts: known_hosts.to_owned(),
            home: None,
            rt,
        })
    }

    pub fn host_key(&self) -> &HostKeyInfo {
        &self.host_key
    }

    /// Accepts an unknown host key and records it in known_hosts (OpenSSH
    /// format, `[host]:port` for non-standard ports), so a later `ssh <alias>`
    /// does not ask again.
    pub fn trust_host_key(&mut self) -> Result<(), RemoteError> {
        if self.host_key.status == HostKeyStatus::Unknown {
            learn_host_key(
                &self.known_hosts,
                &known_hosts_name(&self.host, self.port),
                &self.host_key.key,
            )
            .map_err(|e| std::io::Error::other(format!("無法寫入 {}：{e}", self.known_hosts.display())))?;
            self.host_key.status = HostKeyStatus::Known;
        }
        Ok(())
    }

    fn ensure_trusted(&self) -> Result<(), RemoteError> {
        match self.host_key.status {
            HostKeyStatus::Known => Ok(()),
            _ => Err(RemoteError::HostKeyNotTrusted),
        }
    }

    /// Password authentication, falling back to keyboard-interactive (used by
    /// PAM-based servers) with the same password.
    pub fn authenticate_password(&mut self, user: &str, password: &str) -> Result<(), RemoteError> {
        self.ensure_trusted()?;
        let Self { rt, handle, .. } = self;
        rt.block_on(async {
            let mut methods = match handle.authenticate_password(user, password).await? {
                russh::client::AuthResult::Success => return Ok(()),
                russh::client::AuthResult::Failure { remaining_methods, .. } => remaining_methods,
            };
            if methods.contains(&MethodKind::KeyboardInteractive) {
                let mut response = handle
                    .authenticate_keyboard_interactive_start(user, None::<String>)
                    .await?;
                for _ in 0..5 {
                    response = match response {
                        KeyboardInteractiveAuthResponse::Success => return Ok(()),
                        KeyboardInteractiveAuthResponse::Failure { remaining_methods, .. } => {
                            methods = remaining_methods;
                            break;
                        }
                        KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                            let answers = prompts.iter().map(|_| password.to_owned()).collect();
                            handle.authenticate_keyboard_interactive_respond(answers).await?
                        }
                    };
                }
            }
            if methods.contains(&MethodKind::Password) || methods.contains(&MethodKind::KeyboardInteractive) {
                Err(RemoteError::AuthFailed)
            } else {
                let names: Vec<&str> = methods.iter().map(<&str>::from).collect();
                Err(RemoteError::PasswordNotAllowed(names.join(", ")))
            }
        })
    }

    /// Public key authentication with an unencrypted private key file (or a
    /// passphrase), used to verify a deployment when no `ssh` client exists.
    pub fn authenticate_key(
        &mut self,
        user: &str,
        private_key: &Path,
        passphrase: Option<&str>,
    ) -> Result<(), RemoteError> {
        self.ensure_trusted()?;
        let key = russh::keys::load_secret_key(private_key, passphrase).map_err(RemoteError::PrivateKey)?;
        let Self { rt, handle, .. } = self;
        rt.block_on(async {
            let hash = handle.best_supported_rsa_hash().await?.flatten();
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            match handle.authenticate_publickey(user, key).await? {
                russh::client::AuthResult::Success => Ok(()),
                russh::client::AuthResult::Failure { .. } => Err(RemoteError::KeyRejected),
            }
        })
    }

    fn sftp(&mut self) -> Result<&SftpSession, RemoteError> {
        if self.sftp.is_none() {
            let Self { rt, handle, .. } = self;
            let session = rt.block_on(async {
                let channel = handle.channel_open_session().await?;
                channel.request_subsystem(true, "sftp").await?;
                match tokio::time::timeout(Duration::from_secs(15), SftpSession::new(channel.into_stream())).await {
                    Ok(Ok(session)) => Ok(session),
                    Ok(Err(e)) => Err(RemoteError::SftpUnavailable(e.to_string())),
                    Err(_) => Err(RemoteError::SftpUnavailable("逾時".into())),
                }
            })?;
            self.sftp = Some(session);
        }
        Ok(self.sftp.as_ref().expect("initialized above"))
    }

    /// Absolute home directory as SFTP reports it: `/home/ubuntu` on Linux,
    /// `/C:/Users/name` on Windows.
    pub fn home_dir(&mut self) -> Result<String, RemoteError> {
        if let Some(home) = &self.home {
            return Ok(home.clone());
        }
        self.sftp()?;
        let Self { rt, sftp, .. } = self;
        let home = rt.block_on(sftp.as_ref().expect("opened above").canonicalize("."))?;
        self.home = Some(home.clone());
        Ok(home)
    }

    /// Detects the OS from the shape of the home path.
    pub fn detect_os(&mut self) -> Result<RemoteOs, RemoteError> {
        Ok(if is_windows_path(&self.home_dir()?) {
            RemoteOs::Windows
        } else {
            RemoteOs::Linux
        })
    }

    /// Writes `data` byte for byte to `remote_path` (created or truncated).
    pub fn upload(&mut self, remote_path: &str, data: &[u8]) -> Result<(), RemoteError> {
        self.sftp()?;
        let Self { rt, sftp, .. } = self;
        let sftp = sftp.as_ref().expect("opened above");
        rt.block_on(async {
            let mut file = sftp.create(remote_path).await?;
            file.write_all(data).await?;
            file.close().await?;
            Ok(())
        })
    }

    pub fn remove(&mut self, remote_path: &str) -> Result<(), RemoteError> {
        self.sftp()?;
        let Self { rt, sftp, .. } = self;
        let sftp = sftp.as_ref().expect("opened above");
        Ok(rt.block_on(sftp.remove_file(remote_path))?)
    }

    /// Runs `command` through the remote login shell, passing each output line
    /// (stdout and stderr) to `on_line`. Returns the exit status, `None` if the
    /// server did not report one.
    pub fn exec(&mut self, command: &str, on_line: &mut dyn FnMut(&str)) -> Result<Option<u32>, RemoteError> {
        let Self { rt, handle, .. } = self;
        rt.block_on(async {
            let mut channel = handle.channel_open_session().await?;
            channel.exec(true, command).await?;
            let mut status = None;
            let mut pending = Vec::new();
            while let Some(msg) = channel.wait().await {
                match msg {
                    ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                        pending.extend_from_slice(&data);
                        while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
                            let line: Vec<u8> = pending.drain(..=pos).collect();
                            emit_line(&line, on_line);
                        }
                    }
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            if !pending.is_empty() {
                emit_line(&pending, on_line);
            }
            Ok(status)
        })
    }

    pub fn close(self) {
        let _ = self
            .rt
            .block_on(self.handle.disconnect(Disconnect::ByApplication, "", "en"));
    }
}

fn emit_line(bytes: &[u8], on_line: &mut dyn FnMut(&str)) {
    let text = String::from_utf8_lossy(bytes);
    let line = text.trim_end_matches(['\r', '\n']);
    if !line.trim().is_empty() {
        on_line(line);
    }
}

/// Appends `<name> <key>` to known_hosts. (russh's `learn_known_hosts_path`
/// starts a new file with an empty line.)
fn learn_host_key(path: &Path, name: &str, key: &PublicKey) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read(path).unwrap_or_default();
    let key = key.to_openssh().map_err(std::io::Error::other)?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if existing.last().is_some_and(|&b| b != b'\n') {
        file.write_all(b"\n")?;
    }
    writeln!(file, "{name} {key}")
}

/// How OpenSSH names the host in known_hosts and `ssh-keygen -R`.
pub fn known_hosts_name(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}

/// `/C:/Users/x` or `C:/Users/x`.
pub fn is_windows_path(path: &str) -> bool {
    let p = path.strip_prefix('/').unwrap_or(path).as_bytes();
    p.len() >= 2 && p[0].is_ascii_alphabetic() && p[1] == b':'
}

/// `/C:/Users/x` → `C:/Users/x` (accepted by Windows programs).
pub fn to_windows_path(path: &str) -> String {
    if is_windows_path(path) {
        path.strip_prefix('/').unwrap_or(path).to_owned()
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths() {
        assert!(is_windows_path("/C:/Users/me"));
        assert!(is_windows_path("D:/x"));
        assert!(!is_windows_path("/home/ubuntu"));
        assert!(!is_windows_path("/"));
        assert_eq!(to_windows_path("/C:/Users/John Doe"), "C:/Users/John Doe");
        assert_eq!(to_windows_path("/home/a"), "/home/a");
    }

    #[test]
    fn learned_host_keys_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("known_hosts");
        let key = russh::keys::parse_public_key_base64(
            "AAAAC3NzaC1lZDI1NTE5AAAAIBjic8iOL38Mx0OaC2MVG+1ba0oiJFgKhOoiN4mFGK8G",
        )
        .unwrap();

        learn_host_key(&path, &known_hosts_name("10.0.0.1", 2222), &key).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("[10.0.0.1]:2222 ssh-ed25519 AAAA"), "{text:?}");
        assert!(known_hosts::check_known_hosts_path("10.0.0.1", 2222, &key, &path).unwrap());
        assert!(!known_hosts::check_known_hosts_path("10.0.0.1", 22, &key, &path).unwrap());

        // Appending to a file without a trailing newline.
        std::fs::write(&path, "other ssh-ed25519 AAAA").unwrap();
        learn_host_key(&path, "host", &key).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("other ssh-ed25519 AAAA\nhost ssh-ed25519 "),
            "{text:?}"
        );
    }

    #[test]
    fn known_hosts_names() {
        assert_eq!(known_hosts_name("10.0.0.1", 22), "10.0.0.1");
        assert_eq!(known_hosts_name("10.0.0.1", 2222), "[10.0.0.1]:2222");
    }
}
