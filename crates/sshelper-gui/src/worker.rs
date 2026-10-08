//! Runs a deployment on a background thread and reports back over a channel.
//! The only question the worker asks mid-way is whether to trust an unknown
//! host key; the UI answers through a second channel.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use eframe::egui;
use sshelper_core::SshPaths;
use sshelper_core::deploy::{self, DeployOptions, DeployReport, Event};
use sshelper_core::remote::{self, Connection, HostKeyInfo, HostKeyStatus};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub enum Message {
    Status(String),
    ConfirmHostKey(HostKeyInfo),
    Event(Event),
    Finished(Result<DeployReport, String>),
}

pub struct Job {
    pub messages: Receiver<Message>,
    pub decisions: Sender<bool>,
}

pub fn spawn(paths: SshPaths, opts: DeployOptions, password: String, ctx: egui::Context) -> Job {
    let (tx, messages) = mpsc::channel();
    let (decisions, decision_rx) = mpsc::channel();
    thread::spawn(move || {
        let send = |message: Message| {
            let _ = tx.send(message);
            ctx.request_repaint();
        };
        let result = run(&paths, &opts, &password, &send, &decision_rx).map_err(|e| e.to_string());
        send(Message::Finished(result));
    });
    Job { messages, decisions }
}

fn run(
    paths: &SshPaths,
    opts: &DeployOptions,
    password: &str,
    send: &dyn Fn(Message),
    decisions: &Receiver<bool>,
) -> Result<DeployReport, Box<dyn std::error::Error>> {
    send(Message::Status(format!(
        "連線到 {} …",
        remote::known_hosts_name(&opts.host, opts.port)
    )));
    let mut conn = Connection::connect(&opts.host, opts.port, &paths.known_hosts(), CONNECT_TIMEOUT)?;

    if conn.host_key().status == HostKeyStatus::Unknown {
        send(Message::ConfirmHostKey(conn.host_key().clone()));
        if !decisions.recv().unwrap_or(false) {
            return Err("已取消：未信任主機金鑰".into());
        }
        conn.trust_host_key()?;
    }

    send(Message::Status(format!("以 {} 登入 …", opts.user)));
    conn.authenticate_password(&opts.user, password)?;

    send(Message::Status("部署中 …".into()));
    let report = deploy::run(&mut conn, paths, opts, &mut |event| send(Message::Event(event)))?;
    conn.close();
    Ok(report)
}
