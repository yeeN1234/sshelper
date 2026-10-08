//! Post-deployment check that `ssh <alias>` logs in without a password.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::SshPaths;
use crate::process::{command, tail};
use crate::remote::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// Login succeeded; the string says how it was checked.
    Passed(String),
    Failed(String),
    Skipped(String),
}

/// What the fallback check needs when no `ssh` client is installed.
#[derive(Debug, Clone)]
pub struct Target {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub private_key: PathBuf,
}

/// Prefers the system `ssh` client in batch mode — this exercises the real
/// config entry, known_hosts and the private key permissions, exactly what
/// `ssh <alias>` will do. Without an `ssh` binary, falls back to a public key
/// login through russh.
pub fn run(paths: &SshPaths, target: &Target) -> Verification {
    match system_ssh(paths, &target.alias) {
        Some(result) => result,
        None => builtin(paths, target),
    }
}

fn system_ssh(paths: &SshPaths, alias: &str) -> Option<Verification> {
    let mut cmd = command("ssh");
    cmd.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "PreferredAuthentications=publickey",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ConnectTimeout=10",
    ]);
    if !paths.is_default() {
        cmd.arg("-F").arg(paths.config());
        cmd.arg("-o")
            .arg(format!("UserKnownHostsFile={}", paths.known_hosts().display()));
    }
    cmd.args([alias, "exit"]).stdin(Stdio::null());
    match cmd.output() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(Verification::Failed(format!("無法執行 ssh：{e}"))),
        Ok(out) if out.status.success() => Some(Verification::Passed(format!("ssh {alias}"))),
        Ok(out) => Some(Verification::Failed(tail(&out.stderr, 3))),
    }
}

fn builtin(paths: &SshPaths, target: &Target) -> Verification {
    if crate::keys::is_encrypted(&target.private_key) == Some(true) {
        return Verification::Skipped("找不到 ssh 用戶端，且私鑰設有 passphrase，無法自動驗證".into());
    }
    let result = Connection::connect(&target.host, target.port, &paths.known_hosts(), Duration::from_secs(15))
        .and_then(|mut conn| {
            let result = conn.authenticate_key(&target.user, Path::new(&target.private_key), None);
            conn.close();
            result
        });
    match result {
        Ok(()) => Verification::Passed("內建 SSH 用戶端金鑰登入（系統未安裝 ssh）".into()),
        Err(e) => Verification::Failed(e.to_string()),
    }
}
