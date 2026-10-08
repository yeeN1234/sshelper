//! The `deploy` command: collect input (flags first, prompts for the rest),
//! connect, confirm the host key, log in and run the deployment.

use std::io::{self, BufRead, IsTerminal};
use std::net::IpAddr;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sshelper_core::config::ConfigFile;
use sshelper_core::deploy::{self, DeployOptions, TargetOs};
use sshelper_core::keys;
use sshelper_core::remote::{self, Connection, HostKeyStatus, RemoteError};
use sshelper_core::verify::Verification;
use sshelper_core::{SshPaths, validate};

use crate::{DeployArgs, output, prompt, ssh_paths};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const PASSWORD_ATTEMPTS: usize = 3;

pub fn deploy(args: DeployArgs) -> Result<ExitCode> {
    let paths = ssh_paths(&args.common)?;
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let wizard = interactive && args.is_empty();
    let require = |flag: &str| -> Result<()> {
        if interactive {
            Ok(())
        } else {
            bail!("非互動模式請以 {flag} 指定")
        }
    };

    output::banner();

    // Key
    let key = match &args.key {
        Some(spec) => keys::resolve(paths.dir(), spec).with_context(|| format!("找不到金鑰「{spec}」"))?,
        None => {
            let pairs = keys::discover(paths.dir()).with_context(|| format!("無法讀取 {}", paths.dir().display()))?;
            if pairs.is_empty() {
                output::no_keys_hint(&paths);
                bail!("找不到任何 SSH 公鑰");
            }
            if pairs.iter().all(|k| k.problem().is_some()) {
                output::key_list(&paths, &pairs);
                bail!("沒有可用的金鑰（公鑰旁需有同名、去掉 .pub 的私鑰）");
            }
            require("--key")?;
            prompt::select_key(&pairs)?
        }
    };
    if let Some(problem) = key.problem() {
        bail!("{}：{problem}", key.file_name());
    }

    // Target
    let host_input = match &args.host {
        Some(host) => validate::host(host).map_err(|e| anyhow!(e))?,
        None => {
            require("--host")?;
            let raw = prompt::text(
                "目標主機 IP 或網域？",
                None,
                Some("例如 192.168.1.10、jetson.local，也可輸入 user@host"),
                |s| validate::host(s).map(|_| ()),
            )?;
            validate::host(&raw).map_err(|e| anyhow!(e))?
        }
    };
    let host = host_input.host;
    let default_user = host_input.user.unwrap_or_else(|| "ubuntu".into());
    let user = match &args.user {
        Some(user) => {
            validate::user(user).map_err(|e| anyhow!(e))?;
            user.clone()
        }
        None if wizard => prompt::text("目標主機帳號？", Some(&default_user), None, validate::user)?,
        None => default_user,
    };
    let port = match args.port {
        Some(port) => port,
        None if wizard => validate::port(&prompt::text("SSH 連接埠？", Some("22"), None, |s| {
            validate::port(s).map(|_| ())
        })?)
        .map_err(|e| anyhow!(e))?,
        None => 22,
    };
    let os = match args.os {
        Some(os) => os.into(),
        None if wizard => prompt::target_os()?,
        None => TargetOs::Auto,
    };
    let (alias, replace) = choose_alias(&args, &paths, &host, interactive)?;

    // Confirmation
    if interactive && !args.yes {
        output::heading("確認部署資訊");
        output::info(&format!("金鑰      {}", key.public_path.display()));
        output::info(&format!(
            "目標      {user}@{}  （{}）",
            remote::known_hosts_name(&host, port),
            os_label(os)
        ));
        output::info(&format!(
            "別名      {alias}{}",
            if replace { "（覆蓋既有設定）" } else { "" }
        ));
        println!();
        if !prompt::confirm("開始部署？", true)? {
            return Err(prompt::Cancelled.into());
        }
    }

    let fixed_password = if args.password_stdin {
        let mut line = String::new();
        io::stdin()
            .lock()
            .read_line(&mut line)
            .context("無法從標準輸入讀取密碼")?;
        // PowerShell pipelines may prepend a UTF-8 BOM and append CRLF.
        Some(
            line.trim_start_matches('\u{feff}')
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        )
    } else {
        std::env::var("SSHELPER_PASSWORD").ok()
    };
    if fixed_password.is_none() && !interactive {
        bail!("非互動模式請以 --password-stdin 或環境變數 SSHELPER_PASSWORD 提供密碼");
    }

    // Connect, host key, login
    output::heading("連線與登入");
    let mut conn = connect(&host, port, &paths)?;
    match conn.host_key().status {
        HostKeyStatus::Known => output::ok(&format!(
            "主機金鑰與 known_hosts 相符（{}）",
            conn.host_key().fingerprint
        )),
        HostKeyStatus::Unknown => {
            output::host_key(conn.host_key(), &remote::known_hosts_name(&host, port));
            let trusted = if args.accept_new_host_key {
                true
            } else if interactive {
                prompt::confirm("信任此主機並記錄到 known_hosts？", false)?
            } else {
                bail!("第一次連線需要確認主機金鑰；無人值守時請加上 --accept-new-host-key");
            };
            if !trusted {
                return Err(prompt::Cancelled.into());
            }
            conn.trust_host_key()?;
            output::ok("已記錄到 known_hosts");
        }
        // `connect` already fails on a changed key.
        HostKeyStatus::Changed { .. } => unreachable!(),
    }

    for attempt in 1..=PASSWORD_ATTEMPTS {
        let password = match &fixed_password {
            Some(p) => p.clone(),
            None => prompt::password(&format!("{user}@{host} 的密碼："))?,
        };
        match conn.authenticate_password(&user, &password) {
            Ok(()) => break,
            Err(RemoteError::AuthFailed) if fixed_password.is_none() && attempt < PASSWORD_ATTEMPTS => {
                output::warn("帳號或密碼錯誤，請再試一次");
                // Servers drop the session after a few failures; start fresh.
                conn = connect(&host, port, &paths)?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    output::ok(&format!("已登入 {user}@{host}"));

    // Deploy
    let opts = DeployOptions {
        key,
        host,
        port,
        user,
        os,
        alias: alias.clone(),
        replace_existing: replace,
        verify: !args.no_verify,
    };
    let report = deploy::run(&mut conn, &paths, &opts, &mut output::event)?;
    conn.close();
    output::report(&report, &alias);

    Ok(match report.verification {
        Verification::Failed(_) => ExitCode::from(3),
        _ => ExitCode::SUCCESS,
    })
}

fn connect(host: &str, port: u16, paths: &SshPaths) -> Result<Connection> {
    output::dim(&format!("連線到 {} ...", remote::known_hosts_name(host, port)));
    Ok(Connection::connect(host, port, &paths.known_hosts(), CONNECT_TIMEOUT)?)
}

/// Returns the alias and whether an existing section may be replaced.
fn choose_alias(args: &DeployArgs, paths: &SshPaths, host: &str, interactive: bool) -> Result<(String, bool)> {
    let config = ConfigFile::load(&paths.config()).with_context(|| format!("無法讀取 {}", paths.config().display()))?;
    let suggestion = suggest_alias(host);
    let ask = || {
        prompt::text(
            "連線別名（~/.ssh/config 的 Host）？",
            suggestion.as_deref(),
            Some("之後用 ssh <別名> 登入，例如 jetson、dev-server"),
            validate::alias,
        )
    };

    let mut alias = match &args.alias {
        Some(alias) => {
            validate::alias(alias).map_err(|e| anyhow!(e))?;
            alias.clone()
        }
        None if interactive => ask()?,
        None => bail!("非互動模式請以 --alias 指定"),
    };
    loop {
        let Some(section) = config.find_host(&alias) else {
            return Ok((alias, false));
        };
        if section.patterns.len() > 1 {
            let message = format!(
                "別名「{alias}」位於多別名區塊 `Host {}`，無法自動覆蓋",
                section.patterns.join(" ")
            );
            if !interactive {
                bail!(message);
            }
            output::warn(&message);
        } else if args.replace {
            return Ok((alias, true));
        } else if !interactive {
            bail!("~/.ssh/config 已有 Host {alias}；加上 --replace 覆蓋，或換一個別名");
        } else {
            output::warn(&format!("~/.ssh/config 已有 Host {alias}："));
            for line in config.section_text(&section).lines() {
                output::dim(&format!("    {line}"));
            }
            if prompt::confirm("部署成功後以新設定覆蓋這個區塊？", false)? {
                return Ok((alias, true));
            }
        }
        alias = ask()?;
    }
}

/// `jetson.local` → `jetson`; IP addresses get no suggestion.
fn suggest_alias(host: &str) -> Option<String> {
    if host.parse::<IpAddr>().is_ok() {
        return None;
    }
    let label = host.split('.').next()?.to_owned();
    validate::alias(&label).ok().map(|()| label)
}

fn os_label(os: TargetOs) -> &'static str {
    match os {
        TargetOs::Auto => "自動偵測作業系統",
        TargetOs::Linux => "Linux",
        TargetOs::Windows => "Windows",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_suggestions() {
        assert_eq!(suggest_alias("jetson.local").as_deref(), Some("jetson"));
        assert_eq!(suggest_alias("dev-server"), Some("dev-server".into()));
        assert_eq!(suggest_alias("192.168.1.10"), None);
        assert_eq!(suggest_alias("fe80::1"), None);
    }
}
