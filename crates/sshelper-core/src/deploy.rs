//! The deployment steps that run after the user is logged in.

use std::fs;
use std::path::PathBuf;

use crate::SshPaths;
use crate::config::{self, ConfigError, ConfigFile, HostEntry, MergeAction};
use crate::keys::{self, KeyPair};
use crate::remote::{self, Connection, RemoteError, RemoteOs};
use crate::verify::{self, Verification};

/// Installer run on Linux / Unix targets (`sh`).
pub const INSTALL_SH: &str = include_str!("../assets/remote/install-key.sh");
/// Installer run on Windows targets (`powershell -File`).
pub const INSTALL_PS1: &str = include_str!("../assets/remote/install-key.ps1");

/// The OS the user picked; `Auto` asks the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetOs {
    #[default]
    Auto,
    Linux,
    Windows,
}

impl TargetOs {
    pub const ALL: [TargetOs; 3] = [TargetOs::Auto, TargetOs::Linux, TargetOs::Windows];

    pub fn label(self) -> &'static str {
        match self {
            TargetOs::Auto => "自動偵測",
            TargetOs::Linux => "Linux（Ubuntu / Debian / Jetson / RHEL ...）",
            TargetOs::Windows => "Windows（OpenSSH Server）",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeployOptions {
    pub key: KeyPair,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub os: TargetOs,
    pub alias: String,
    /// Replace an existing `Host <alias>` section.
    pub replace_existing: bool,
    pub verify: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Permissions,
    DetectOs,
    Upload,
    Install,
    Config,
    Verify,
}

impl Step {
    pub const ALL: [Step; 6] = [
        Step::Permissions,
        Step::DetectOs,
        Step::Upload,
        Step::Install,
        Step::Config,
        Step::Verify,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Step::Permissions => "修正本機私鑰權限",
            Step::DetectOs => "確認遠端作業系統",
            Step::Upload => "上傳公鑰（SFTP）",
            Step::Install => "寫入遠端 authorized_keys",
            Step::Config => "更新本機 SSH config",
            Step::Verify => "測試免密碼登入",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done(String),
    Warning(String),
    Skipped(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Started(Step),
    Finished(Step, Outcome),
    /// A line printed by the remote installer.
    Remote(String),
}

#[derive(Debug, Clone)]
pub struct DeployReport {
    pub remote_os: RemoteOs,
    pub config_action: MergeAction,
    pub config_backup: Option<PathBuf>,
    pub config_encoding_fixed: Option<&'static str>,
    pub config_entry: Vec<String>,
    pub verification: Verification,
}

#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    #[error(transparent)]
    Remote(#[from] RemoteError),
    #[error("{0}")]
    Config(#[from] ConfigError),
    #[error("遠端安裝失敗：{0}")]
    InstallFailed(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Runs every step on an authenticated connection. Progress is reported
/// through `on_event`; the first hard failure aborts with an error (after a
/// matching `Finished(step, Failed)` event).
pub fn run(
    conn: &mut Connection,
    paths: &SshPaths,
    opts: &DeployOptions,
    on_event: &mut dyn FnMut(Event),
) -> Result<DeployReport, DeployError> {
    if let Some(problem) = opts.key.problem() {
        return Err(DeployError::Invalid(format!("{}：{problem}", opts.key.file_name())));
    }
    // Fail before touching anything when the config cannot take the alias.
    if let Some(existing) = ConfigFile::load(&paths.config())?.find_host(&opts.alias) {
        if existing.patterns.len() > 1 {
            return Err(ConfigError::AliasInMultiHost {
                alias: opts.alias.clone(),
                patterns: existing.patterns.join(" "),
            }
            .into());
        }
        if !opts.replace_existing {
            return Err(ConfigError::AliasExists(opts.alias.clone()).into());
        }
    }

    let mut step = Stepper { on_event };

    // 1. Local private key permissions. Not fatal: the key is still deployed
    //    and the verification step reports the consequence.
    step.start(Step::Permissions);
    let _ = keys::protect_ssh_dir(paths.dir());
    match keys::protect_private_key(&opts.key.private_path) {
        Ok(()) => step.done(Step::Permissions, permissions_message(&opts.key)),
        Err(e) => step.warn(
            Step::Permissions,
            format!("{e}；若登入時出現 UNPROTECTED PRIVATE KEY FILE，即為此原因"),
        ),
    }

    // 2. Remote OS.
    step.start(Step::DetectOs);
    let detected = step.check(Step::DetectOs, conn.detect_os())?;
    let home = step.check(Step::DetectOs, conn.home_dir())?;
    let remote_os = match (opts.os, detected) {
        (TargetOs::Auto, os) => {
            step.done(Step::DetectOs, format!("{}（家目錄 {home}）", os.label()));
            os
        }
        (TargetOs::Linux, RemoteOs::Linux) | (TargetOs::Windows, RemoteOs::Windows) => {
            step.done(Step::DetectOs, format!("{}（家目錄 {home}）", detected.label()));
            detected
        }
        (_, os) => {
            step.warn(
                Step::DetectOs,
                format!("選擇的系統與偵測結果不同，改用 {} 流程（家目錄 {home}）", os.label()),
            );
            os
        }
    };

    // 3. Upload the .pub file unchanged plus the installer, under random names.
    step.start(Step::Upload);
    let token = random_token();
    let (script, script_ext) = match remote_os {
        RemoteOs::Linux => (INSTALL_SH, "sh"),
        RemoteOs::Windows => (INSTALL_PS1, "ps1"),
    };
    let pub_name = format!(".sshelper-{token}.pub");
    let script_name = format!(".sshelper-{token}.{script_ext}");
    let remote_pub = format!("{}/{pub_name}", home.trim_end_matches('/'));
    let remote_script = format!("{}/{script_name}", home.trim_end_matches('/'));
    let pub_bytes = step.check(Step::Upload, fs::read(&opts.key.public_path).map_err(DeployError::from))?;
    let upload = conn
        .upload(&remote_pub, &pub_bytes)
        .and_then(|()| conn.upload(&remote_script, normalize_script(script).as_bytes()));
    if let Err(e) = upload {
        cleanup(conn, &[&remote_pub, &remote_script]);
        return Err(step.fail(Step::Upload, e.into()));
    }
    step.done(Step::Upload, format!("~/{pub_name}、~/{script_name}"));

    // 4. Run the installer; it strips CR/BOM, appends the key, fixes the
    //    permissions and deletes both temp files.
    step.start(Step::Install);
    let command = match remote_os {
        RemoteOs::Linux => format!("sh {} {}", sh_quote(&remote_script), sh_quote(&remote_pub)),
        RemoteOs::Windows => format!(
            "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\" {pub_name}",
            remote::to_windows_path(&remote_script)
        ),
    };
    let mut remote_lines = Vec::new();
    let status = conn.exec(&command, &mut |line| {
        remote_lines.push(line.to_owned());
        (step.on_event)(Event::Remote(line.to_owned()));
    });
    cleanup(conn, &[&remote_pub, &remote_script]);
    match status {
        Ok(Some(0)) => step.done(Step::Install, "遠端 ~/.ssh/authorized_keys 設定完成".into()),
        Ok(code) => {
            let detail = remote_lines
                .iter()
                .rev()
                .find(|l| l.contains("ERROR"))
                .cloned()
                .unwrap_or_else(|| match code {
                    Some(code) => format!("exit {code}"),
                    None => "遠端未回報結束代碼".into(),
                });
            return Err(step.fail(Step::Install, DeployError::InstallFailed(detail)));
        }
        Err(e) => return Err(step.fail(Step::Install, e.into())),
    }

    // 5. Local ~/.ssh/config.
    step.start(Step::Config);
    let entry = HostEntry {
        alias: opts.alias.clone(),
        host_name: opts.host.clone(),
        user: opts.user.clone(),
        port: opts.port,
        identity_file: config::identity_file_value(paths, &opts.key.private_path),
    };
    let entry_lines = entry.to_lines(&chrono::Local::now().format("%Y-%m-%d").to_string());
    let config = step.check(
        Step::Config,
        ConfigFile::load(&paths.config()).map_err(DeployError::from),
    )?;
    let merged = step.check(
        Step::Config,
        config
            .merge(&opts.alias, &entry_lines, opts.replace_existing)
            .map_err(DeployError::from),
    )?;
    let backup = step.check(Step::Config, config.write(&merged.lines).map_err(DeployError::from))?;
    let message = match merged.action {
        MergeAction::Replaced => format!("已覆蓋原有的 Host {}", opts.alias),
        MergeAction::InsertedBeforeWildcard => {
            format!("已寫入 Host {}（放在 Host * 之前，避免被萬用設定蓋掉）", opts.alias)
        }
        MergeAction::Appended => format!("已新增 Host {}", opts.alias),
    };
    step.done(Step::Config, message);

    // 6. Verify.
    step.start(Step::Verify);
    let verification = if opts.verify {
        let target = verify::Target {
            alias: opts.alias.clone(),
            host: opts.host.clone(),
            port: opts.port,
            user: opts.user.clone(),
            private_key: opts.key.private_path.clone(),
        };
        verify::run(paths, &target)
    } else {
        Verification::Skipped("已略過".into())
    };
    match &verification {
        Verification::Passed(how) => step.done(Step::Verify, how.clone()),
        Verification::Failed(why) => step.warn(Step::Verify, why.clone()),
        Verification::Skipped(why) => step.skip(Step::Verify, why.clone()),
    }

    Ok(DeployReport {
        remote_os,
        config_action: merged.action,
        config_backup: backup,
        config_encoding_fixed: config.encoding_issue,
        config_entry: entry_lines,
        verification,
    })
}

struct Stepper<'a> {
    on_event: &'a mut dyn FnMut(Event),
}

impl Stepper<'_> {
    fn start(&mut self, step: Step) {
        (self.on_event)(Event::Started(step));
    }
    fn done(&mut self, step: Step, message: String) {
        (self.on_event)(Event::Finished(step, Outcome::Done(message)));
    }
    fn warn(&mut self, step: Step, message: String) {
        (self.on_event)(Event::Finished(step, Outcome::Warning(message)));
    }
    fn skip(&mut self, step: Step, message: String) {
        (self.on_event)(Event::Finished(step, Outcome::Skipped(message)));
    }
    fn fail(&mut self, step: Step, error: DeployError) -> DeployError {
        (self.on_event)(Event::Finished(step, Outcome::Failed(error.to_string())));
        error
    }
    fn check<T, E: Into<DeployError>>(&mut self, step: Step, result: Result<T, E>) -> Result<T, DeployError> {
        result.map_err(|e| self.fail(step, e.into()))
    }
}

fn permissions_message(key: &KeyPair) -> String {
    if cfg!(windows) {
        format!("{}：已關閉繼承，僅目前使用者可讀寫（icacls）", key.private_file_name())
    } else {
        format!("{}：chmod 600", key.private_file_name())
    }
}

/// Best-effort removal of the temp files (the installer removes them itself;
/// this covers failures before it ran).
fn cleanup(conn: &mut Connection, paths: &[&str]) {
    for path in paths {
        let _ = conn.remove(path);
    }
}

/// The embedded scripts must reach the target with LF line endings and no
/// BOM, whatever a Windows checkout did to them.
fn normalize_script(script: &str) -> String {
    script.trim_start_matches('\u{feff}').replace('\r', "")
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn random_token() -> String {
    let mut bytes = [0u8; 6];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        bytes.copy_from_slice(&nanos.to_le_bytes()[..6]);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_ascii_and_normalized() {
        for script in [INSTALL_SH, INSTALL_PS1] {
            assert!(script.is_ascii(), "remote scripts must stay ASCII");
            assert!(!normalize_script(script).contains('\r'));
        }
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("/home/a b/x"), "'/home/a b/x'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn token_is_hex() {
        let t = random_token();
        assert_eq!(t.len(), 12);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
