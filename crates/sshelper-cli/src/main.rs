//! `sshelper` — command line interface.
//!
//! Without arguments it runs an interactive wizard. Every answer can also be
//! given as a flag; then only the missing required values are asked, and with
//! all of them present (plus `--password-stdin` / `SSHELPER_PASSWORD` and
//! `--accept-new-host-key`) it runs unattended.

mod flow;
mod output;
mod prompt;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use sshelper_core::SshPaths;
use sshelper_core::deploy::TargetOs;
use sshelper_core::keys;

const ABOUT: &str = "部署 SSH 公鑰到遠端主機並寫入 ~/.ssh/config，之後只要 `ssh <別名>` 即可免密碼登入";

const AFTER_HELP: &str = "\
範例：
  sshelper                                     互動式精靈
  sshelper -H 192.168.1.10 -a jetson           只詢問缺少的項目（金鑰、密碼）
  sshelper -k id_ed25519 -H ubuntu@10.0.0.5 -a dev --accept-new-host-key --password-stdin < pw.txt
  sshelper keys                                列出 ~/.ssh 中的金鑰
  sshelper gui                                 開啟圖形介面

結束代碼：0 成功、1 失敗、3 已部署但免密碼登入測試未通過、130 已取消";

const HELP_TEMPLATE: &str = "{about}\n\n用法：{usage}\n\n{all-args}{after-help}";

#[derive(Parser)]
#[command(
    name = "sshelper",
    version,
    about = ABOUT,
    after_help = AFTER_HELP,
    help_template = HELP_TEMPLATE,
    subcommand_help_heading = "指令",
    subcommand_value_name = "指令",
    next_help_heading = "選項",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    deploy: DeployArgs,

    /// 顯示說明
    #[arg(short, long, action = clap::ArgAction::Help, global = true)]
    help: Option<bool>,

    /// 顯示版本
    #[arg(short = 'V', long, action = clap::ArgAction::Version)]
    version: Option<bool>,
}

#[derive(Subcommand)]
enum Command {
    /// 部署公鑰並設定 ~/.ssh/config（未指定指令時的預設動作）
    Deploy(DeployArgs),
    /// 列出 ~/.ssh 中的金鑰
    Keys {
        #[command(flatten)]
        common: CommonArgs,
    },
    /// 開啟圖形介面（sshelper-gui）
    Gui,
}

#[derive(Args, Clone, Default)]
struct CommonArgs {
    /// 使用其他 .ssh 目錄（預設：目前使用者的 ~/.ssh）
    #[arg(long, value_name = "DIR", env = "SSHELPER_SSH_DIR")]
    ssh_dir: Option<PathBuf>,
}

#[derive(Args, Clone, Default)]
pub struct DeployArgs {
    /// 要部署的金鑰：~/.ssh 中的名稱（例如 id_ed25519）或檔案路徑
    #[arg(short, long, value_name = "KEY")]
    key: Option<String>,

    /// 目標主機 IP 或網域，可寫成 user@host
    #[arg(short = 'H', long, value_name = "HOST")]
    host: Option<String>,

    /// 目標主機帳號 [預設：ubuntu]
    #[arg(short, long)]
    user: Option<String>,

    /// SSH 連接埠 [預設：22]
    #[arg(short, long, value_parser = clap::value_parser!(u16).range(1..))]
    port: Option<u16>,

    /// 目標主機作業系統 [預設：auto]
    #[arg(long, value_enum)]
    os: Option<OsArg>,

    /// 連線別名，即 ~/.ssh/config 的 Host（例如 jetson、dev-server）
    #[arg(short, long)]
    alias: Option<String>,

    /// 別名已存在時直接覆蓋該區塊
    #[arg(long)]
    replace: bool,

    /// 自動信任尚未記錄的主機金鑰（無人值守用；金鑰「變更」時仍會拒絕）
    #[arg(long)]
    accept_new_host_key: bool,

    /// 從標準輸入讀取遠端密碼（也可用環境變數 SSHELPER_PASSWORD）
    #[arg(long)]
    password_stdin: bool,

    /// 部署後不測試免密碼登入
    #[arg(long)]
    no_verify: bool,

    /// 略過最後的確認提示
    #[arg(short, long)]
    yes: bool,

    #[command(flatten)]
    common: CommonArgs,
}

impl DeployArgs {
    /// No deployment value given at all: run the full wizard.
    fn is_empty(&self) -> bool {
        self.key.is_none()
            && self.host.is_none()
            && self.user.is_none()
            && self.port.is_none()
            && self.os.is_none()
            && self.alias.is_none()
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum OsArg {
    Auto,
    Linux,
    Windows,
}

impl From<OsArg> for TargetOs {
    fn from(os: OsArg) -> Self {
        match os {
            OsArg::Auto => TargetOs::Auto,
            OsArg::Linux => TargetOs::Linux,
            OsArg::Windows => TargetOs::Windows,
        }
    }
}

/// Like `Cli::parse()`, with the Chinese help layout applied to subcommands
/// too (derive attributes do not propagate to them).
fn parse_cli() -> Cli {
    let command = Cli::command().mut_subcommands(|sub| {
        sub.help_template(HELP_TEMPLATE).mut_args(|arg| {
            if arg.get_help_heading().is_none() {
                arg.help_heading("選項")
            } else {
                arg
            }
        })
    });
    Cli::from_arg_matches(&command.get_matches()).unwrap_or_else(|e| e.exit())
}

fn main() -> ExitCode {
    let cli = parse_cli();
    let result = match cli.command {
        None => flow::deploy(cli.deploy),
        Some(Command::Deploy(args)) => flow::deploy(args),
        Some(Command::Keys { common }) => list_keys(&common),
        Some(Command::Gui) => launch_gui(),
    };
    match result {
        Ok(code) => code,
        Err(e) if e.is::<prompt::Cancelled>() => {
            output::warn("已取消");
            ExitCode::from(130)
        }
        Err(e) => {
            output::error(&format!("{e:#}"));
            ExitCode::FAILURE
        }
    }
}

fn ssh_paths(common: &CommonArgs) -> Result<SshPaths> {
    match &common.ssh_dir {
        Some(dir) => Ok(SshPaths::custom(dir)),
        None => SshPaths::for_current_user().context("找不到使用者家目錄"),
    }
}

fn list_keys(common: &CommonArgs) -> Result<ExitCode> {
    let paths = ssh_paths(common)?;
    let pairs = keys::discover(paths.dir()).with_context(|| format!("無法讀取 {}", paths.dir().display()))?;
    if pairs.is_empty() {
        output::no_keys_hint(&paths);
        return Ok(ExitCode::FAILURE);
    }
    output::key_list(&paths, &pairs);
    Ok(ExitCode::SUCCESS)
}

fn launch_gui() -> Result<ExitCode> {
    let exe = std::env::current_exe()?;
    let gui = exe.with_file_name(if cfg!(windows) {
        "sshelper-gui.exe"
    } else {
        "sshelper-gui"
    });
    if !gui.is_file() {
        bail!(
            "找不到 {}，請將 sshelper-gui 與 sshelper 放在同一個資料夾",
            gui.display()
        );
    }
    std::process::Command::new(&gui)
        .spawn()
        .with_context(|| format!("無法啟動 {}", gui.display()))?;
    Ok(ExitCode::SUCCESS)
}
