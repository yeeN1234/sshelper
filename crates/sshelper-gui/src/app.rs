//! Main window: one compact form that fits the default window without
//! scrolling. Deploying switches the same area to a progress / result view;
//! the primary action stays in a sticky footer.

mod run_view;

use eframe::egui::{self, Align, Button, CornerRadius, Key, Layout, Margin, Response, RichText, Stroke, Theme, Ui};
use egui_phosphor::regular as icon;
use sshelper_core::config::ConfigFile;
use sshelper_core::deploy::{DeployOptions, DeployReport, Event, Outcome, Step, TargetOs};
use sshelper_core::keys::{self, KeyPair};
use sshelper_core::remote::{self, HostKeyInfo};
use sshelper_core::{SshPaths, validate};

use crate::theme::{self, GAP_M, GAP_S, Palette};
use crate::worker::{self, Job, Message};
use crate::{logo, titlebar};

/// Width of the field column's main inputs.
const FIELD_WIDTH: f32 = 260.0;
const KEY_COMBO_WIDTH: f32 = 380.0;

#[derive(Clone)]
enum StepState {
    Pending,
    Running,
    Finished(Outcome),
}

struct Run {
    job: Job,
    target: String,
    alias: String,
    status: String,
    steps: Vec<(Step, StepState)>,
    remote_lines: Vec<String>,
    pending_host_key: Option<HostKeyInfo>,
    result: Option<Result<DeployReport, String>>,
}

impl Run {
    fn is_running(&self) -> bool {
        self.result.is_none()
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Started(step) => self.set(step, StepState::Running),
            Event::Finished(step, outcome) => self.set(step, StepState::Finished(outcome)),
            Event::Remote(line) => self.remote_lines.push(line),
        }
    }

    fn set(&mut self, step: Step, state: StepState) {
        if let Some(entry) = self.steps.iter_mut().find(|(s, _)| *s == step) {
            entry.1 = state;
        }
    }
}

/// How the alias relates to the existing config.
enum AliasState {
    Free,
    Exists(String),
    MultiHost(String),
}

/// Fields whose errors are shown once the user has left them (or tried to
/// submit), not while still typing.
#[derive(Default)]
struct Touched {
    host: bool,
    port: bool,
    user: bool,
    alias: bool,
}

impl Touched {
    fn all() -> Self {
        Self {
            host: true,
            port: true,
            user: true,
            alias: true,
        }
    }
}

pub struct App {
    paths: Option<SshPaths>,
    keys: Vec<KeyPair>,
    selected: Option<usize>,
    config: Option<ConfigFile>,
    host: String,
    user: String,
    port: String,
    os: TargetOs,
    alias: String,
    password: String,
    show_password: bool,
    replace: bool,
    verify: bool,
    run: Option<Run>,
    font_missing: bool,
    touched: Touched,
    submit_requested: bool,
    /// Copy button that was just used, and when (for the "已複製" feedback).
    copied: Option<(String, f64)>,
    /// Requested theme; the visuals fade towards it.
    dark: bool,
    /// Blend value whose visuals are currently installed.
    applied_blend: Option<f32>,
    /// Theme last requested for the native title bar.
    window_dark: Option<bool>,
    /// Header logo (the application icon); None if it failed to decode.
    logo: Option<egui::TextureHandle>,
    /// Whether the native window icon was taken from the executable yet.
    window_icon_set: bool,
}

impl App {
    pub fn new(ctx: &egui::Context, font_found: bool) -> Self {
        // SSHELPER_SSH_DIR points the app at another .ssh directory (testing).
        let paths = std::env::var_os("SSHELPER_SSH_DIR")
            .map(SshPaths::custom)
            .or_else(SshPaths::for_current_user);
        Self::with_paths(ctx, paths, font_found)
    }

    pub fn with_paths(ctx: &egui::Context, paths: Option<SshPaths>, font_found: bool) -> Self {
        theme::apply_style(ctx);
        let mut app = Self {
            paths,
            keys: Vec::new(),
            selected: None,
            config: None,
            host: String::new(),
            user: "ubuntu".into(),
            port: "22".into(),
            os: TargetOs::Auto,
            alias: String::new(),
            password: String::new(),
            show_password: false,
            replace: false,
            verify: true,
            run: None,
            font_missing: !font_found,
            touched: Touched::default(),
            submit_requested: false,
            copied: None,
            // Start with the system theme.
            dark: ctx.theme() == Theme::Dark,
            applied_blend: None,
            window_dark: None,
            logo: logo::header_texture(ctx),
            window_icon_set: false,
        };
        app.reload();
        app
    }

    /// Re-reads ~/.ssh (keys and config), keeping the selected key if possible.
    fn reload(&mut self) {
        let Some(paths) = &self.paths else { return };
        let previous = self.selected.and_then(|i| self.keys.get(i)).map(KeyPair::file_name);
        self.keys = keys::discover(paths.dir()).unwrap_or_default();
        self.selected = previous
            .and_then(|name| {
                self.keys
                    .iter()
                    .position(|k| k.file_name() == name && k.problem().is_none())
            })
            .or_else(|| self.keys.iter().position(|k| k.problem().is_none()));
        self.config = ConfigFile::load(&paths.config()).ok();
    }

    fn running(&self) -> bool {
        self.run.as_ref().is_some_and(Run::is_running)
    }

    fn alias_state(&self) -> AliasState {
        let Some(config) = &self.config else {
            return AliasState::Free;
        };
        match config.find_host(self.alias.trim()) {
            None => AliasState::Free,
            Some(section) if section.patterns.len() > 1 => AliasState::MultiHost(section.patterns.join(" ")),
            Some(section) => AliasState::Exists(config.section_text(&section)),
        }
    }

    /// Required fields that are still empty, in form order.
    fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.selected.is_none() {
            missing.push("金鑰");
        }
        if self.host.trim().is_empty() {
            missing.push("主機");
        }
        if self.password.is_empty() {
            missing.push("密碼");
        }
        if self.alias.trim().is_empty() {
            missing.push("別名");
        }
        missing
    }

    /// The deployment options, or the first reason the form is not ready.
    fn options(&self) -> Result<(SshPaths, DeployOptions), String> {
        let paths = self.paths.clone().ok_or("找不到使用者家目錄")?;
        let key = self.selected.and_then(|i| self.keys.get(i)).ok_or("請選擇金鑰")?;
        if self.host.trim().is_empty() {
            return Err("請輸入目標主機".into());
        }
        let host = validate::host(&self.host)?;
        let user = host.user.clone().unwrap_or_else(|| self.user.trim().to_owned());
        validate::user(&user)?;
        let port = validate::port(&self.port)?;
        if self.alias.trim().is_empty() {
            return Err("請輸入連線別名".into());
        }
        validate::alias(self.alias.trim())?;
        match self.alias_state() {
            AliasState::MultiHost(_) => return Err("別名位於多別名區塊，請換一個".into()),
            AliasState::Exists(_) if !self.replace => return Err("別名已存在，請勾選覆蓋或換一個".into()),
            _ => {}
        }
        if self.password.is_empty() {
            return Err("請輸入遠端密碼".into());
        }
        Ok((
            paths,
            DeployOptions {
                key: key.clone(),
                host: host.host,
                port,
                user,
                os: self.os,
                alias: self.alias.trim().to_owned(),
                replace_existing: self.replace,
                verify: self.verify,
            },
        ))
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Ok((paths, opts)) = self.options() else { return };
        let target = format!("{}@{}", opts.user, remote::known_hosts_name(&opts.host, opts.port));
        let alias = opts.alias.clone();
        let job = worker::spawn(paths, opts, self.password.clone(), ctx.clone());
        self.run = Some(Run {
            job,
            target,
            alias,
            status: "準備中 …".into(),
            steps: Step::ALL.iter().map(|s| (*s, StepState::Pending)).collect(),
            remote_lines: Vec::new(),
            pending_host_key: None,
            result: None,
        });
    }

    /// Enter in a field or the deploy button: deploy, or reveal what is wrong.
    fn submit(&mut self, ctx: &egui::Context) {
        if self.running() {
            return;
        }
        if self.options().is_ok() {
            self.start(ctx);
        } else {
            self.run = None;
            self.touched = Touched::all();
        }
    }

    fn poll(&mut self) {
        let Some(run) = &mut self.run else { return };
        let mut finished_ok = false;
        while let Ok(message) = run.job.messages.try_recv() {
            match message {
                Message::Status(status) => run.status = status,
                Message::ConfirmHostKey(info) => {
                    run.status = "等待確認主機指紋 …".into();
                    run.pending_host_key = Some(info);
                }
                Message::Event(event) => run.apply(event),
                Message::Finished(result) => {
                    finished_ok = result.is_ok();
                    run.result = Some(result);
                }
            }
        }
        if finished_ok {
            // Ready for the next host; the result view keeps showing `ssh <alias>`.
            self.password.clear();
            self.alias.clear();
            self.replace = false;
            self.touched = Touched::default();
            self.reload();
        }
    }

    /// Fades the visuals (and the native title bar) towards the requested
    /// theme; returns the palette for this frame.
    fn update_theme(&mut self, ctx: &egui::Context, frame: &eframe::Frame) -> Palette {
        let blend = ctx.animate_bool_with_time(egui::Id::new("sshelper-dark"), self.dark, theme::FADE_SECONDS);
        let palette = Palette::blend(blend);
        // Where the title bar cannot take a colour, flip its theme halfway,
        // together with the window content.
        let window_dark = blend >= 0.5;
        if self.window_dark != Some(window_dark) {
            let theme = if window_dark {
                egui::SystemTheme::Dark
            } else {
                egui::SystemTheme::Light
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
            self.window_dark = Some(window_dark);
        }
        if self.applied_blend != Some(blend) {
            let visuals = theme::visuals(blend);
            ctx.set_visuals_of(Theme::Light, visuals.clone());
            ctx.set_visuals_of(Theme::Dark, visuals);
            // Title bar in the header's colour, fading along.
            titlebar::apply(frame, window_dark, palette.surface, palette.text);
            self.applied_blend = Some(blend);
            // The new style is picked up from the next frame on.
            ctx.request_repaint();
        }
        palette
    }

    /// A copy button that briefly confirms with "已複製".
    fn copy_button(&mut self, ui: &mut Ui, id: &str, text: &str) {
        let now = ui.input(|i| i.time);
        let copied = self
            .copied
            .as_ref()
            .is_some_and(|(key, at)| key == id && now - at < 1.5);
        let label = if copied {
            format!("{} 已複製", icon::CHECK)
        } else {
            format!("{} 複製", icon::COPY)
        };
        if ui.button(label).clicked() {
            ui.ctx().copy_text(text.to_owned());
            self.copied = Some((id.to_owned(), now));
        }
        if copied {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        self.poll();
        let p = self.update_theme(ui.ctx(), frame);
        if !self.window_icon_set {
            self.window_icon_set = titlebar::set_window_icon(frame);
        }
        let bar = egui::Frame::NONE
            .fill(p.surface)
            .inner_margin(Margin::symmetric(24, 12));

        egui::Panel::top("header").frame(bar).show(ui, |ui| self.header(ui, &p));
        egui::Panel::bottom("actions")
            .frame(bar)
            .show(ui, |ui| self.action_bar(ui, &p));
        egui::CentralPanel::default_margins()
            .frame(egui::Frame::NONE.fill(p.panel).inner_margin(Margin::symmetric(24, 20)))
            .show(ui, |ui| {
                // Only scrolls when the window is made smaller than the default.
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    if self.font_missing {
                        status(
                            ui,
                            p.warning,
                            icon::WARNING,
                            "找不到中文字型，部分文字可能無法顯示；可設定環境變數 SSHELPER_FONT 指向字型檔。",
                        );
                    }
                    if self.run.is_some() {
                        self.run_view(ui, &p);
                    } else {
                        self.form(ui, &p);
                    }
                });
            });

        self.host_key_modal(ui.ctx(), &p);
        if std::mem::take(&mut self.submit_requested) {
            self.submit(ui.ctx());
        }
    }
}

// ------------------------------------------------------------------ helpers

/// Icon + text in one colour, so state is never conveyed by colour alone.
fn status(ui: &mut Ui, color: egui::Color32, glyph: &str, text: impl Into<String>) -> Response {
    ui.label(RichText::new(format!("{glyph} {}", text.into())).color(color))
}

fn field_error(ui: &mut Ui, p: &Palette, show: bool, result: Result<(), String>) {
    if show && let Err(message) = result {
        status(ui, p.danger, icon::X_CIRCLE, message);
    }
}

/// Enter in a single-line field submits the form.
fn submitted(ui: &Ui, response: &Response) -> bool {
    response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
}

/// Monospace text on a code background.
fn code_block(ui: &mut Ui, p: &Palette, text: &str) {
    egui::Frame::NONE
        .fill(p.code_bg)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).monospace());
        });
}

impl App {
    fn header(&mut self, ui: &mut Ui, p: &Palette) {
        ui.horizontal(|ui| {
            match &self.logo {
                Some(logo) => {
                    ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(24.0, 24.0)));
                }
                None => {
                    ui.label(RichText::new(icon::TERMINAL_WINDOW).size(24.0).color(p.accent));
                }
            }
            ui.label(RichText::new("sshelper").size(20.0).strong());
            ui.label(RichText::new("SSH 公鑰部署 · 免密碼登入設定").color(p.muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                theme::toggle(ui, &mut self.dark)
            });
        });
    }

    fn action_bar(&mut self, ui: &mut Ui, p: &Palette) {
        let button_size = egui::vec2(156.0, 40.0);
        ui.horizontal(|ui| {
            // The text gets whatever the buttons leave and wraps there. Only a
            // failed run shows two buttons (返回修改 + 重試).
            let failed = self.run.as_ref().is_some_and(|r| matches!(r.result, Some(Err(_))));
            let buttons = if failed { 2.0 } else { 1.0 };
            let text_width = (ui.available_width() - buttons * (button_size.x + GAP_S) - GAP_M).max(0.0);
            ui.allocate_ui_with_layout(
                egui::vec2(text_width, button_size.y),
                Layout::left_to_right(Align::Center).with_main_wrap(true),
                |ui| self.footer_status(ui, p),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.footer_buttons(ui, p, button_size)
            });
        });
    }

    fn footer_status(&mut self, ui: &mut Ui, p: &Palette) {
        if let Some(run) = &self.run {
            match &run.result {
                None => {
                    ui.spinner();
                    ui.label(&run.status);
                }
                Some(Ok(_)) => {
                    status(ui, p.success, icon::CHECK_CIRCLE, "部署完成");
                }
                Some(Err(_)) => {
                    status(ui, p.danger, icon::X_CIRCLE, "部署失敗，可修改後重試");
                }
            }
            return;
        }
        match self.options() {
            Ok((_, opts)) => status(
                ui,
                p.success,
                icon::CHECK_CIRCLE,
                format!(
                    "準備就緒：{}@{}",
                    opts.user,
                    remote::known_hosts_name(&opts.host, opts.port)
                ),
            ),
            Err(_) if !self.missing().is_empty() => status(
                ui,
                p.muted,
                icon::INFO,
                format!("尚未填寫：{}", self.missing().join("、")),
            ),
            Err(reason) => status(ui, p.warning, icon::WARNING, reason),
        };
    }

    fn primary_button(p: &Palette, text: String, size: egui::Vec2) -> Button<'static> {
        Button::new(RichText::new(text).strong().color(p.on_primary))
            .fill(p.primary)
            .min_size(size)
    }

    fn footer_buttons(&mut self, ui: &mut Ui, p: &Palette, size: egui::Vec2) {
        let finished = self.run.as_ref().map(|run| run.result.as_ref().map(Result::is_ok));
        match finished {
            // Form view.
            None => {
                let button = Self::primary_button(p, format!("{}  開始部署", icon::ROCKET_LAUNCH), size);
                if ui
                    .add_enabled(self.options().is_ok(), button)
                    .on_hover_text("也可以在任一欄位按 Enter")
                    .clicked()
                {
                    self.submit_requested = true;
                }
            }
            // Running.
            Some(None) => {
                ui.add_enabled(false, Button::new("部署中 …").min_size(size));
            }
            Some(Some(true)) => {
                let button = Self::primary_button(p, format!("{}  部署另一台", icon::PLUS), size);
                if ui.add(button).clicked() {
                    self.run = None;
                }
            }
            Some(Some(false)) => {
                let retry = Self::primary_button(p, format!("{}  重試", icon::ARROW_CLOCKWISE), size);
                if ui.add_enabled(self.options().is_ok(), retry).clicked() {
                    self.submit_requested = true;
                }
                if ui
                    .add(Button::new(format!("{} 返回修改", icon::PENCIL_SIMPLE)).min_size(size))
                    .clicked()
                {
                    self.run = None;
                }
            }
        }
    }

    fn form(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(paths) = self.paths.clone() else {
            status(ui, p.danger, icon::X_CIRCLE, "找不到使用者家目錄，無法定位 ~/.ssh");
            return;
        };
        egui::Grid::new("form")
            .num_columns(2)
            .spacing([GAP_M, 14.0])
            .min_col_width(52.0)
            .show(ui, |ui| {
                ui.label("金鑰");
                self.key_row(ui, p, &paths);
                ui.end_row();

                let label = ui.label("主機");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let response = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.host)
                                    .hint_text("例如 192.168.1.10 或 user@host")
                                    .desired_width(FIELD_WIDTH),
                            )
                            .labelled_by(label.id);
                        if response.lost_focus() {
                            self.touched.host = true;
                            // Split a pasted "user@host" into the two fields.
                            if let Ok(validate::HostInput { host, user: Some(user) }) = validate::host(&self.host) {
                                self.host = host;
                                self.user = user;
                            }
                        }
                        if submitted(ui, &response) {
                            self.submit_requested = true;
                        }
                        ui.add_space(GAP_S);
                        let port_label = ui.label("連接埠");
                        let response = ui
                            .add(egui::TextEdit::singleline(&mut self.port).desired_width(64.0))
                            .labelled_by(port_label.id);
                        self.touched.port |= response.lost_focus();
                        if submitted(ui, &response) {
                            self.submit_requested = true;
                        }
                    });
                    let show_host = self.touched.host && !self.host.trim().is_empty();
                    field_error(ui, p, show_host, validate::host(&self.host).map(|_| ()));
                    field_error(ui, p, self.touched.port, validate::port(&self.port).map(|_| ()));
                });
                ui.end_row();

                let label = ui.label("帳號");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let response = ui
                            .add(egui::TextEdit::singleline(&mut self.user).desired_width(FIELD_WIDTH))
                            .labelled_by(label.id);
                        self.touched.user |= response.lost_focus();
                        if submitted(ui, &response) {
                            self.submit_requested = true;
                        }
                    });
                    field_error(ui, p, self.touched.user, validate::user(self.user.trim()));
                });
                ui.end_row();

                ui.label("系統");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.os, TargetOs::Auto, "自動偵測");
                    ui.radio_value(&mut self.os, TargetOs::Linux, "Linux");
                    ui.radio_value(&mut self.os, TargetOs::Windows, "Windows");
                });
                ui.end_row();

                let label = ui.label("密碼");
                ui.horizontal(|ui| {
                    let response = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.password)
                                .password(!self.show_password)
                                .hint_text("只用於這次部署，不會儲存")
                                .desired_width(FIELD_WIDTH),
                        )
                        .labelled_by(label.id);
                    if submitted(ui, &response) {
                        self.submit_requested = true;
                    }
                    let (glyph, text) = if self.show_password {
                        (icon::EYE_SLASH, "隱藏")
                    } else {
                        (icon::EYE, "顯示")
                    };
                    ui.toggle_value(&mut self.show_password, format!("{glyph} {text}"))
                        .on_hover_text("顯示或隱藏密碼");
                });
                ui.end_row();

                let label = ui.label("別名");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let response = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.alias)
                                    .hint_text("例如 jetson，之後用 ssh 別名 登入")
                                    .desired_width(FIELD_WIDTH),
                            )
                            .labelled_by(label.id);
                        self.touched.alias |= response.lost_focus();
                        if submitted(ui, &response) {
                            self.submit_requested = true;
                        }
                        self.alias_status(ui, p);
                    });
                    if self.touched.alias
                        && !self.alias.trim().is_empty()
                        && let Err(message) = validate::alias(self.alias.trim())
                    {
                        status(ui, p.danger, icon::X_CIRCLE, message);
                    }
                });
                ui.end_row();

                ui.label("");
                ui.checkbox(&mut self.verify, "部署後測試免密碼登入");
                ui.end_row();
            });
    }

    fn key_row(&mut self, ui: &mut Ui, p: &Palette, paths: &SshPaths) {
        if self.keys.is_empty() {
            ui.vertical(|ui| {
                status(
                    ui,
                    p.warning,
                    icon::WARNING,
                    format!("{} 中沒有任何 .pub 公鑰", paths.dir().display()),
                );
                ui.horizontal(|ui| {
                    ui.label("先執行");
                    ui.code("ssh-keygen -t ed25519");
                    self.copy_button(ui, "keygen", "ssh-keygen -t ed25519");
                    if ui.button(format!("{} 重新整理", icon::ARROWS_CLOCKWISE)).clicked() {
                        self.reload();
                    }
                });
            });
            return;
        }
        ui.horizontal(|ui| {
            let selected = self.selected.and_then(|i| self.keys.get(i));
            let selected_text = selected.map_or_else(
                || "請選擇".to_owned(),
                |k| match &k.public {
                    Ok(info) => format!("{}  {}", k.file_name(), info.algorithm),
                    Err(_) => k.file_name(),
                },
            );
            let hover = selected.map(|k| match &k.public {
                Ok(info) if !info.comment.is_empty() => {
                    format!("{}\n私鑰：{}", info.comment, k.private_path.display())
                }
                _ => format!("私鑰：{}", k.private_path.display()),
            });
            let combo = egui::ComboBox::from_id_salt("key")
                .selected_text(selected_text)
                .width(KEY_COMBO_WIDTH)
                .truncate()
                .show_ui(ui, |ui| {
                    for (i, key) in self.keys.iter().enumerate() {
                        let mut text = key.describe();
                        if let Some(problem) = key.problem() {
                            text.push_str(&format!("（{problem}）"));
                        }
                        let button = Button::selectable(self.selected == Some(i), text);
                        if ui.add_enabled(key.problem().is_none(), button).clicked() {
                            self.selected = Some(i);
                        }
                    }
                });
            if let Some(hover) = hover {
                combo.response.on_hover_text(hover);
            }
            if ui
                .button(format!("{} 重新整理", icon::ARROWS_CLOCKWISE))
                .on_hover_text("重新讀取 ~/.ssh")
                .clicked()
            {
                self.reload();
            }
        });
    }

    fn alias_status(&mut self, ui: &mut Ui, p: &Palette) {
        if self.alias.trim().is_empty() || validate::alias(self.alias.trim()).is_err() {
            return;
        }
        match self.alias_state() {
            AliasState::Free => {
                status(ui, p.success, icon::CHECK_CIRCLE, "可以使用");
            }
            AliasState::Exists(text) => {
                status(ui, p.warning, icon::WARNING, "已存在").on_hover_text(text);
                ui.checkbox(&mut self.replace, "覆蓋既有設定");
            }
            AliasState::MultiHost(patterns) => {
                status(ui, p.danger, icon::X_CIRCLE, "位於多別名區塊，請換一個")
                    .on_hover_text(format!("Host {patterns}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use eframe::egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{By, Queryable};
    use sshelper_core::config::MergeAction;
    use sshelper_core::remote::RemoteOs;
    use sshelper_core::verify::Verification;

    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBlah test@sshelper";

    fn ssh_dir_with_key(config: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("id_test.pub"), KEY).unwrap();
        fs::write(dir.path().join("id_test"), "private").unwrap();
        if let Some(config) = config {
            fs::write(dir.path().join("config"), config).unwrap();
        }
        dir
    }

    /// The default window size from `main.rs`.
    fn harness(dir: &std::path::Path) -> Harness<'static, App> {
        let paths = SshPaths::custom(dir);
        Harness::builder()
            .with_size(egui::vec2(760.0, 600.0))
            .build_eframe(move |cc| App::with_paths(&cc.egui_ctx, Some(paths), true))
    }

    /// The text field labelled `label` (the same words may also appear as
    /// plain text elsewhere).
    fn input(label: &str) -> By<'_> {
        By::new().label(label).predicate(|node| {
            matches!(
                node.role(),
                Role::TextInput | Role::PasswordInput | Role::MultilineTextInput
            )
        })
    }

    fn focus(harness: &mut Harness<'static, App>, label: &str) {
        harness.get(input(label)).focus();
        harness.run_steps(2);
    }

    fn type_into(harness: &mut Harness<'static, App>, label: &str, text: &str) {
        focus(harness, label);
        harness.get(input(label)).type_text(text);
        harness.run_steps(2);
    }

    fn has_text(harness: &Harness<'static, App>, text: &str) -> bool {
        harness.query_all_by_label_contains(text).next().is_some()
    }

    fn click_button(harness: &mut Harness<'static, App>, text: &str) {
        harness
            .get(By::new().role(Role::Button).label_contains(text))
            .click_accesskit();
        harness.run_steps(2);
    }

    /// Puts the app into the result view without a network.
    fn finished_run(app: &mut App, result: Result<DeployReport, String>) {
        let (_tx, messages) = std::sync::mpsc::channel();
        let (decisions, _rx) = std::sync::mpsc::channel();
        app.run = Some(Run {
            job: Job { messages, decisions },
            target: "ubuntu@10.0.0.5".into(),
            alias: "dev".into(),
            status: String::new(),
            steps: Step::ALL
                .iter()
                .map(|s| (*s, StepState::Finished(Outcome::Done("完成".into()))))
                .collect(),
            remote_lines: vec!["[remote] done".into()],
            pending_host_key: None,
            result: Some(result),
        });
    }

    fn report() -> DeployReport {
        DeployReport {
            remote_os: RemoteOs::Linux,
            config_action: MergeAction::Appended,
            config_backup: None,
            config_encoding_fixed: None,
            config_entry: vec![],
            verification: Verification::Passed("ssh dev".into()),
        }
    }

    #[test]
    fn footer_lists_missing_fields_until_complete() {
        let dir = ssh_dir_with_key(None);
        let mut harness = harness(dir.path());
        harness.run_steps(2);
        assert!(has_text(&harness, "尚未填寫：主機、密碼、別名"));

        type_into(&mut harness, "主機", "10.0.0.5");
        type_into(&mut harness, "別名", "dev");
        assert!(has_text(&harness, "尚未填寫：密碼"));
        type_into(&mut harness, "密碼", "secret");

        let (_, opts) = harness.state().options().expect("form complete");
        assert_eq!(
            (opts.host.as_str(), opts.user.as_str(), opts.port),
            ("10.0.0.5", "ubuntu", 22)
        );
        assert_eq!(opts.key.file_name(), "id_test.pub");
        assert!(has_text(&harness, "準備就緒：ubuntu@10.0.0.5"));
    }

    #[test]
    fn host_error_waits_for_blur() {
        let dir = ssh_dir_with_key(None);
        let mut harness = harness(dir.path());
        type_into(&mut harness, "主機", "bad host");
        assert!(!has_text(&harness, "主機格式不正確"), "no error while typing");
        focus(&mut harness, "別名");
        assert!(has_text(&harness, "主機格式不正確"));
    }

    #[test]
    fn pasted_user_at_host_is_split() {
        let dir = ssh_dir_with_key(None);
        let mut harness = harness(dir.path());
        type_into(&mut harness, "主機", "tester@10.0.0.5");
        focus(&mut harness, "別名");
        assert_eq!(harness.state().host, "10.0.0.5");
        assert_eq!(harness.state().user, "tester");
    }

    #[test]
    fn password_can_be_revealed() {
        let dir = ssh_dir_with_key(None);
        let mut harness = harness(dir.path());
        assert!(!harness.state().show_password);
        click_button(&mut harness, "顯示");
        assert!(harness.state().show_password);
        click_button(&mut harness, "隱藏");
        assert!(!harness.state().show_password);
    }

    #[test]
    fn existing_alias_needs_replace() {
        let dir = ssh_dir_with_key(Some("Host jetson\n    HostName 1.2.3.4\n"));
        let mut harness = harness(dir.path());
        type_into(&mut harness, "主機", "10.0.0.5");
        type_into(&mut harness, "密碼", "secret");
        type_into(&mut harness, "別名", "jetson");
        assert!(has_text(&harness, "已存在"));
        assert!(has_text(&harness, "別名已存在，請勾選覆蓋或換一個"));
        assert!(harness.state().options().is_err());

        harness.get_by_label("覆蓋既有設定").click_accesskit();
        harness.run_steps(2);
        assert!(harness.state().options().unwrap().1.replace_existing);
    }

    #[test]
    fn no_keys_shows_hint() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = harness(dir.path());
        harness.run_steps(2);
        assert!(has_text(&harness, "沒有任何 .pub 公鑰"));
        harness.get_by_label("ssh-keygen -t ed25519");
    }

    #[test]
    fn theme_toggle_fades_to_dark_and_back() {
        let dir = ssh_dir_with_key(None);
        let mut harness = harness(dir.path());
        harness.state_mut().dark = false;
        harness.run_steps(30);
        assert!(!harness.ctx.global_style().visuals.dark_mode);

        click_button(&mut harness, "深色");
        assert!(harness.state().dark);
        harness.run_steps(30);
        assert!(harness.ctx.global_style().visuals.dark_mode);

        click_button(&mut harness, "淺色");
        harness.run_steps(30);
        assert!(!harness.ctx.global_style().visuals.dark_mode);
    }

    #[test]
    fn success_view_returns_to_form() {
        let dir = ssh_dir_with_key(None);
        let mut harness = harness(dir.path());
        finished_run(harness.state_mut(), Ok(report()));
        harness.run_steps(2);
        assert!(has_text(&harness, "部署完成"));
        assert!(has_text(&harness, "ssh dev"));
        click_button(&mut harness, "部署另一台");
        assert!(harness.state().run.is_none());
        assert!(harness.query_all(input("主機")).next().is_some(), "form is back");
    }

    #[test]
    fn failure_view_keeps_the_form_values() {
        let dir = ssh_dir_with_key(None);
        let mut harness = harness(dir.path());
        type_into(&mut harness, "主機", "10.0.0.5");
        finished_run(harness.state_mut(), Err("帳號或密碼錯誤".into()));
        harness.run_steps(2);
        assert!(has_text(&harness, "帳號或密碼錯誤"));
        click_button(&mut harness, "返回修改");
        assert!(harness.state().run.is_none());
        assert_eq!(harness.state().host, "10.0.0.5");
    }

    /// Full deployment through the UI against a real server:
    ///
    /// ```text
    /// SSHELPER_E2E_TARGET=tester@127.0.0.1 SSHELPER_E2E_PORT=2222 SSHELPER_E2E_PASSWORD=... \
    ///   cargo test -p sshelper-gui -- --ignored
    /// ```
    #[test]
    #[ignore = "needs an SSH server, see doc comment"]
    fn e2e_deploy_through_ui() {
        let target = std::env::var("SSHELPER_E2E_TARGET").expect("SSHELPER_E2E_TARGET");
        let port = std::env::var("SSHELPER_E2E_PORT").unwrap_or_else(|_| "22".into());
        let password = std::env::var("SSHELPER_E2E_PASSWORD").expect("SSHELPER_E2E_PASSWORD");

        // A real key pair: the verification step logs in with it.
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id_e2e");
        let status = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "gui-e2e@sshelper", "-f"])
            .arg(&key)
            .status()
            .expect("ssh-keygen");
        assert!(status.success());

        let mut harness = harness(dir.path());
        type_into(&mut harness, "主機", &target);
        harness.state_mut().port = port;
        type_into(&mut harness, "密碼", &password);
        type_into(&mut harness, "別名", "gui-e2e");
        click_button(&mut harness, "開始部署");

        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            harness.step();
            let trust = By::new().role(Role::Button).label_contains("信任並繼續");
            if harness.query_all(trust.clone()).next().is_some() {
                harness.get(trust).click_accesskit();
            }
            if harness.state().run.as_ref().is_some_and(|run| run.result.is_some()) {
                break;
            }
            assert!(Instant::now() < deadline, "deployment timed out");
            std::thread::sleep(Duration::from_millis(50));
        }
        harness.run_steps(2);

        let run = harness.state().run.as_ref().unwrap();
        let report = run.result.as_ref().unwrap().as_ref().expect("deployment succeeded");
        assert!(
            matches!(report.verification, Verification::Passed(_)),
            "{:?}",
            report.verification
        );
        assert!(
            run.steps
                .iter()
                .all(|(_, s)| matches!(s, StepState::Finished(Outcome::Done(_))))
        );
        for text in ["部署完成", "ssh gui-e2e"] {
            assert!(has_text(&harness, text), "missing {text}");
        }
        let config = fs::read_to_string(dir.path().join("config")).unwrap();
        assert!(config.contains("Host gui-e2e"), "{config}");
    }
}
