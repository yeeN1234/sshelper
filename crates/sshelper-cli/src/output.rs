//! Styled terminal output. Symbols fall back to ASCII on consoles without
//! emoji support (e.g. the legacy Windows console).

use console::{Emoji, style};
use sshelper_core::SshPaths;
use sshelper_core::deploy::{DeployReport, Event, Outcome, Step};
use sshelper_core::keys::KeyPair;
use sshelper_core::remote::HostKeyInfo;
use sshelper_core::verify::Verification;

static OK: Emoji = Emoji("✔", "OK");
static WARN: Emoji = Emoji("⚠", "!");
static FAIL: Emoji = Emoji("✘", "X");
static SKIP: Emoji = Emoji("–", "-");

pub fn banner() {
    println!();
    println!(
        "  {}  {}",
        style("sshelper").cyan().bold(),
        style("SSH 公鑰部署 / 免密碼登入設定").dim()
    );
    println!();
}

pub fn heading(text: &str) {
    println!();
    println!("{}", style(text).cyan().bold());
}

pub fn info(text: &str) {
    println!("  {text}");
}

pub fn ok(text: &str) {
    println!("  {} {text}", style(OK).green());
}

pub fn warn(text: &str) {
    for (i, line) in text.lines().enumerate() {
        let mark = if i == 0 {
            style(WARN).yellow().to_string()
        } else {
            " ".into()
        };
        println!("  {mark} {}", style(line).yellow());
    }
}

pub fn error(text: &str) {
    eprintln!();
    for (i, line) in text.lines().enumerate() {
        let mark = if i == 0 {
            style(FAIL).red().bold().to_string()
        } else {
            " ".into()
        };
        eprintln!("  {mark} {}", style(line).red());
    }
}

pub fn dim(text: &str) {
    println!("  {}", style(text).dim());
}

pub fn no_keys_hint(paths: &SshPaths) {
    warn(&format!("在 {} 找不到任何 .pub 公鑰。", paths.dir().display()));
    info("請先產生一組金鑰，例如：");
    println!();
    println!("      {}", style("ssh-keygen -t ed25519").bold());
    println!();
}

pub fn key_list(paths: &SshPaths, pairs: &[KeyPair]) {
    println!("{}", style(paths.dir().display()).dim());
    let width = pairs.iter().map(|p| p.file_name().chars().count()).max().unwrap_or(0);
    for (i, pair) in pairs.iter().enumerate() {
        let (algorithm, comment) = match &pair.public {
            Ok(info) => (info.algorithm.as_str(), info.comment.as_str()),
            Err(_) => ("?", ""),
        };
        let line = format!("{:<width$}  {algorithm:<12} {comment}", pair.file_name());
        match pair.problem() {
            None => println!("  {:>2}. {line}", i + 1),
            Some(problem) => println!(
                "  {:>2}. {}  {}",
                i + 1,
                style(line).dim(),
                style(format!("（{problem}）")).yellow()
            ),
        }
    }
}

pub fn host_key(key: &HostKeyInfo, name: &str) {
    warn(&format!("第一次連線到 {name}，尚未記錄此主機的金鑰。"));
    info(&format!("  類型      {}", key.algorithm));
    info(&format!("  指紋      {}", style(&key.fingerprint).bold()));
    dim("  若無法確定，可在主機上執行 ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub 比對指紋。");
}

pub fn event(event: Event) {
    match event {
        Event::Started(step) => {
            let index = Step::ALL.iter().position(|s| *s == step).unwrap_or(0) + 1;
            println!();
            println!(
                "{} {}",
                style(format!("[{index}/{}]", Step::ALL.len())).cyan(),
                style(step.title()).bold()
            );
        }
        Event::Finished(_, outcome) => match outcome {
            Outcome::Done(message) => ok(&message),
            Outcome::Warning(message) => warn(&message),
            Outcome::Skipped(message) => println!("  {} {}", style(SKIP).dim(), style(message).dim()),
            Outcome::Failed(message) => {
                for (i, line) in message.lines().enumerate() {
                    let mark = if i == 0 {
                        style(FAIL).red().to_string()
                    } else {
                        " ".into()
                    };
                    println!("  {mark} {}", style(line).red());
                }
            }
        },
        Event::Remote(line) => println!("    {}", style(line).dim()),
    }
}

pub fn report(report: &DeployReport, alias: &str) {
    println!();
    if let Some(encoding) = report.config_encoding_fixed {
        dim(&format!("原 config 編碼為 {encoding}，已改存為 UTF-8（無 BOM）"));
    }
    if let Some(backup) = &report.config_backup {
        dim(&format!("原 config 已備份到 {}", backup.display()));
    }
    for line in &report.config_entry {
        dim(&format!("  {line}"));
    }
    println!();
    match &report.verification {
        Verification::Passed(_) => {
            println!(
                "  {} {}",
                style(OK).green().bold(),
                style("部署完成！之後只需輸入：").green().bold()
            );
        }
        Verification::Skipped(_) => {
            println!(
                "  {} {}",
                style(OK).green().bold(),
                style("部署完成（未自動測試登入），之後只需輸入：").green().bold()
            );
        }
        Verification::Failed(_) => {
            warn("金鑰已部署並寫入 config，但免密碼登入測試未通過。常見原因：");
            info("  - 私鑰設有 passphrase：登入時輸入的是 passphrase（可搭配 ssh-agent）");
            info("  - Linux 目標：家目錄不可被群組/他人寫入（chmod go-w ~），sshd 需允許 PubkeyAuthentication");
            info("  - Windows 目標：確認 sshd 服務設定；管理員帳號使用 administrators_authorized_keys");
            info(&format!("  - 查看詳細過程：ssh -v {alias}"));
            println!();
            info("排除問題後即可使用：");
        }
    }
    println!();
    println!("      {}", style(format!("ssh {alias}")).bold().white());
    println!();
    dim(&format!(
        "也可用於 scp / sftp / VS Code Remote-SSH，例如：scp ./file {alias}:~/"
    ));
    println!();
}
