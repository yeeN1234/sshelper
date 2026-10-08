//! Colour tokens, type scale and spacing.
//!
//! The app blends between the light and dark palette (`t` = 0 → light,
//! 1 → dark) so switching themes fades instead of snapping. Text colours are
//! picked for at least 4.5:1 contrast against the surfaces they sit on.

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Theme, Visuals};

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Duration of the light / dark fade, in seconds.
pub const FADE_SECONDS: f32 = 0.2;

#[derive(Clone, Copy)]
pub struct Palette {
    /// Fill of the primary action; `on_primary` is its text.
    pub primary: Color32,
    pub on_primary: Color32,
    /// Brand / accent text and icons.
    pub accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub text: Color32,
    /// Secondary text.
    pub muted: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub border: Color32,
    pub code_bg: Color32,
    /// Button / control backgrounds: idle, hovered, pressed.
    pub control: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,
}

impl Palette {
    pub const LIGHT: Palette = Palette {
        primary: rgb(0x2563EB), // white text 5.2:1
        on_primary: Color32::WHITE,
        accent: rgb(0x1D4ED8),  // 6.7:1 on white
        success: rgb(0x15803D), // 5.0:1
        warning: rgb(0xB45309), // 5.0:1
        danger: rgb(0xB91C1C),  // 6.5:1
        text: rgb(0x1E293B),
        muted: rgb(0x475569), // 7.6:1
        panel: rgb(0xF8FAFC),
        surface: Color32::WHITE,
        border: rgb(0xCBD5E1),
        code_bg: rgb(0xF1F5F9),
        control: rgb(0xE2E8F0),
        control_hover: rgb(0xCBD5E1),
        control_active: rgb(0x94A3B8),
    };

    pub const DARK: Palette = Palette {
        primary: rgb(0x2563EB),
        on_primary: Color32::WHITE,
        accent: rgb(0x93C5FD),  // 8.8:1 on surface
        success: rgb(0x4ADE80), // 9.2:1
        warning: rgb(0xFBBF24), // 9.8:1
        danger: rgb(0xF87171),  // 5.8:1
        text: rgb(0xE2E8F0),
        muted: rgb(0x94A3B8), // 6.1:1
        panel: rgb(0x0F172A),
        surface: rgb(0x1B2336),
        border: rgb(0x334155),
        code_bg: rgb(0x0F172A),
        control: rgb(0x1E293B),
        control_hover: rgb(0x334155),
        control_active: rgb(0x475569),
    };

    /// The palette `t` of the way from light (0) to dark (1).
    pub fn blend(t: f32) -> Palette {
        let (a, b) = (Self::LIGHT, Self::DARK);
        let mix = |x: Color32, y: Color32| x.lerp_to_gamma(y, t);
        Palette {
            primary: mix(a.primary, b.primary),
            on_primary: mix(a.on_primary, b.on_primary),
            accent: mix(a.accent, b.accent),
            success: mix(a.success, b.success),
            warning: mix(a.warning, b.warning),
            danger: mix(a.danger, b.danger),
            text: mix(a.text, b.text),
            muted: mix(a.muted, b.muted),
            panel: mix(a.panel, b.panel),
            surface: mix(a.surface, b.surface),
            border: mix(a.border, b.border),
            code_bg: mix(a.code_bg, b.code_bg),
            control: mix(a.control, b.control),
            control_hover: mix(a.control_hover, b.control_hover),
            control_active: mix(a.control_active, b.control_active),
        }
    }
}

/// Spacing scale (4 pt rhythm).
pub const GAP_S: f32 = 8.0;
pub const GAP_M: f32 = 16.0;

/// Type scale and spacing, for both egui themes.
pub fn apply_style(ctx: &egui::Context) {
    for theme in [Theme::Light, Theme::Dark] {
        ctx.style_mut_of(theme, |style| {
            style.text_styles.insert(TextStyle::Small, FontId::proportional(12.5));
            style.text_styles.insert(TextStyle::Body, FontId::proportional(15.0));
            style.text_styles.insert(TextStyle::Button, FontId::proportional(15.0));
            style.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.5));
            style.text_styles.insert(TextStyle::Heading, FontId::proportional(20.0));
            let spacing = &mut style.spacing;
            spacing.item_spacing = egui::vec2(GAP_S, GAP_S);
            spacing.button_padding = egui::vec2(12.0, 6.0);
            // Controls at least 30 px tall: comfortable pointer targets.
            spacing.interact_size = egui::vec2(40.0, 30.0);
        });
    }
}

/// Visuals for the blend `t` between light (0) and dark (1).
pub fn visuals(t: f32) -> Visuals {
    let p = Palette::blend(t);
    let mut v = if t < 0.5 { Visuals::light() } else { Visuals::dark() };
    v.panel_fill = p.panel;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.faint_bg_color = p.code_bg;
    v.code_bg_color = p.code_bg;
    v.extreme_bg_color = p.panel;
    v.text_edit_bg_color = Some(p.panel);
    v.weak_text_color = Some(p.muted);
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    v.selection.bg_fill = p.primary.gamma_multiply(0.35);
    // Visible keyboard focus.
    v.selection.stroke = Stroke::new(2.0, p.primary);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke.color = p.text;
    for (widget, fill) in [
        (&mut w.inactive, p.control),
        (&mut w.hovered, p.control_hover),
        (&mut w.active, p.control_active),
        (&mut w.open, p.control_hover),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.fg_stroke.color = p.text;
        widget.bg_stroke = Stroke::new(1.0, p.border);
        widget.corner_radius = CornerRadius::same(6);
    }
    v
}

/// Light / dark switch with a text label (not an emoji-only toggle).
pub fn toggle(ui: &mut egui::Ui, dark: &mut bool) {
    use egui_phosphor::regular::{MOON, SUN};
    let (icon, text) = if *dark { (SUN, "淺色") } else { (MOON, "深色") };
    if ui
        .button(format!("{icon} {text}"))
        .on_hover_text("切換淺色 / 深色主題")
        .clicked()
    {
        *dark = !*dark;
    }
}
