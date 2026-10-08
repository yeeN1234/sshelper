//! Core library of **sshelper**: deploy a local SSH public key to a remote host
//! and register the host in `~/.ssh/config`, so that `ssh <alias>` logs in
//! without a password.
//!
//! The crate has no UI; `sshelper-cli` and `sshelper-gui` drive it. A typical
//! session looks like this:
//!
//! 1. [`keys::discover`] the local key pairs and check user input with [`validate`].
//! 2. [`remote::Connection::connect`], inspect [`remote::Connection::host_key`]
//!    and, after the user accepted an unknown key, [`remote::Connection::trust_host_key`].
//! 3. [`remote::Connection::authenticate_password`].
//! 4. [`deploy::run`]: fixes the private key permissions, uploads the key and an
//!    installer script over SFTP, runs the installer, writes `~/.ssh/config`
//!    and verifies the passwordless login.

pub mod config;
pub mod deploy;
pub mod keys;
pub mod remote;
pub mod validate;
pub mod verify;

mod process;

use std::path::{Path, PathBuf};

/// Location of the local OpenSSH directory (`~/.ssh`) and the files in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshPaths {
    dir: PathBuf,
    is_default: bool,
}

impl SshPaths {
    /// `~/.ssh` of the current user (`%USERPROFILE%\.ssh` on Windows).
    pub fn for_current_user() -> Option<Self> {
        dirs::home_dir().map(|home| Self {
            dir: home.join(".ssh"),
            is_default: true,
        })
    }

    /// A custom directory, mainly for tests. Tools such as the system `ssh`
    /// are then pointed at the files inside it explicitly.
    pub fn custom(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            is_default: false,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn config(&self) -> PathBuf {
        self.dir.join("config")
    }

    pub fn known_hosts(&self) -> PathBuf {
        self.dir.join("known_hosts")
    }

    /// Whether this is the user's real `~/.ssh`, so `~/.ssh/...` paths in the
    /// config resolve to it.
    pub fn is_default(&self) -> bool {
        self.is_default
    }
}
