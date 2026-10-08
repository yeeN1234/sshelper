//! The view shown while deploying and afterwards (steps and result), plus the
//! host key confirmation dialog.

use eframe::egui::{self, Align, Button, CornerRadius, Layout, Margin, RichText, Stroke, Ui};
use egui_phosphor::regular as icon;
use sshelper_core::deploy::Outcome;
use sshelper_core::verify::Verification;

use super::{App, StepState, code_block};
use crate::theme::{GAP_M, GAP_S, Palette};

impl App {
    pub(super) fn run_view(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(run) = &self.run else { return };
        let target = run.target.clone();
        let alias = run.alias.clone();
        let steps = run.steps.clone();
        let remote_lines = run.remote_lines.clone();
        let result = run.result.clone();

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("部署到").color(p.muted));
            ui.label(RichText::new(&target).strong());
            ui.label(RichText::new(format!("· 別名 {alias}")).color(p.muted));
        });
        ui.add_space(GAP_S);

        for (index, (step, state)) in steps.iter().enumerate() {
            let title = format!("{}. {}", index + 1, step.title());
            ui.horizontal_wrapped(|ui| match state {
                StepState::Pending => {
                    ui.label(RichText::new(icon::CIRCLE).color(p.muted));
                    ui.label(RichText::new(title).color(p.muted));
                }
                StepState::Running => {
                    ui.spinner();
                    ui.label(RichText::new(title).strong());
                }
                StepState::Finished(outcome) => {
                    let (glyph, color, message) = match outcome {
                        Outcome::Done(m) => (icon::CHECK_CIRCLE, p.success, m),
                        Outcome::Warning(m) => (icon::WARNING, p.warning, m),
                        Outcome::Skipped(m) => (icon::MINUS_CIRCLE, p.muted, m),
                        Outcome::Failed(m) => (icon::X_CIRCLE, p.danger, m),
                    };
                    ui.label(RichText::new(glyph).color(color));
                    ui.label(RichText::new(title).strong());
                    let message_color = if matches!(outcome, Outcome::Done(_)) {
                        p.muted
                    } else {
                        color
                    };
                    ui.label(RichText::new(message).color(message_color));
                }
            });
        }
        if !remote_lines.is_empty() {
            egui::CollapsingHeader::new(format!("遠端輸出（{} 行）", remote_lines.len()))
                .id_salt("remote-output")
                .show(ui, |ui| code_block(ui, p, &remote_lines.join("\n")));
        }

        let Some(result) = result else { return };
        ui.add_space(GAP_M);
        let outlined = |color| {
            egui::Frame::NONE
                .fill(p.surface)
                .stroke(Stroke::new(1.5, color))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(16))
        };
        match result {
            Ok(report) => {
                let (glyph, color, title) = match &report.verification {
                    Verification::Passed(_) => (icon::CHECK_CIRCLE, p.success, "部署完成，之後只需輸入："),
                    Verification::Skipped(_) => {
                        (icon::CHECK_CIRCLE, p.success, "部署完成（未測試登入），之後只需輸入：")
                    }
                    Verification::Failed(_) => {
                        (icon::WARNING, p.warning, "已部署，但免密碼登入測試未通過；排除後輸入：")
                    }
                };
                outlined(color).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(format!("{glyph} {title}")).strong().color(color));
                    let command = format!("ssh {alias}");
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&command).monospace().size(20.0).strong());
                        self.copy_button(ui, "ssh-command", &command);
                    });
                    if let Verification::Failed(detail) = &report.verification {
                        code_block(ui, p, detail);
                        ui.label(
                            RichText::new(format!(
                                "常見原因：私鑰設有 passphrase、Linux 家目錄權限過寬（chmod go-w ~）。詳細過程：ssh -v {alias}"
                            ))
                            .color(p.muted),
                        );
                    }
                    let mut notes = vec![format!("也可用於 scp / VS Code Remote-SSH，例如 scp ./file {alias}:~/")];
                    if let Some(encoding) = report.config_encoding_fixed {
                        notes.push(format!("原 config 為 {encoding}，已改存為 UTF-8（無 BOM）"));
                    }
                    if let Some(backup) = &report.config_backup {
                        notes.push(format!("原 config 已備份到 {}", backup.display()));
                    }
                    for note in notes {
                        ui.label(RichText::new(note).small().color(p.muted));
                    }
                });
            }
            Err(message) => {
                outlined(p.danger).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(format!("{} 部署失敗", icon::X_CIRCLE))
                            .strong()
                            .color(p.danger),
                    );
                    ui.label(message);
                    ui.label(RichText::new("按「返回修改」更正欄位（例如密碼），或直接「重試」。").color(p.muted));
                });
            }
        }
    }

    pub(super) fn host_key_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(run) = &self.run else { return };
        let Some(info) = run.pending_host_key.clone() else {
            return;
        };
        let target = run.target.clone();
        let mut decision = None;
        let response = egui::Modal::new(egui::Id::new("host-key")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::SHIELD_WARNING).size(26.0).color(p.warning));
                ui.heading("確認主機身分");
            });
            ui.label(format!(
                "第一次連線到 {target}。請確認下列指紋與主機相符；信任後會記錄到 known_hosts，之後不再詢問。"
            ));
            ui.add_space(GAP_S);
            code_block(ui, p, &format!("類型  {}\n指紋  {}", info.algorithm, info.fingerprint));
            ui.horizontal(|ui| {
                self.copy_button(ui, "fingerprint", &info.fingerprint);
                ui.label(
                    RichText::new("在主機上執行 ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub 比對").color(p.muted),
                );
            });
            ui.add_space(GAP_M);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let trust = Button::new(
                    RichText::new(format!("{} 信任並繼續", icon::SHIELD_CHECK))
                        .strong()
                        .color(p.on_primary),
                )
                .fill(p.primary)
                .min_size(egui::vec2(0.0, 36.0));
                if ui.add(trust).clicked() {
                    decision = Some(true);
                }
                if ui.add(Button::new("取消").min_size(egui::vec2(0.0, 36.0))).clicked() {
                    decision = Some(false);
                }
            });
        });
        // Esc or a click outside cancels.
        if response.should_close() && decision.is_none() {
            decision = Some(false);
        }
        if let Some(trust) = decision
            && let Some(run) = &mut self.run
        {
            let _ = run.job.decisions.send(trust);
            run.pending_host_key = None;
        }
    }
}
