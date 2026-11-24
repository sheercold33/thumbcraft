use crate::config::AppTheme;
use egui::{Color32, CornerRadius, Stroke, Visuals};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub is_light_classic: bool,
    pub bg_canvas: Color32,
    pub bg_panel: Color32,
    pub bg_card: Color32,
    pub bg_card_selected: Color32,
    pub bg_chrome_dark: Color32,
    pub bg_toolbar: Color32,
    pub pane_header_active: Color32,
    pub pane_header_active_text: Color32,
    pub pane_header_inactive: Color32,
    pub pane_header_inactive_text: Color32,
    pub section_band: Color32,
    pub border_subtle: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub text_primary: Color32,
    pub text_muted: Color32,
    pub badge_cdr: Color32,
    pub badge_gs: Color32,
    pub badge_modern: Color32,
    pub badge_vector: Color32,
    pub badge_raster: Color32,
    pub star_gold: Color32,
}

impl ThemePalette {
    pub fn for_theme(theme: AppTheme) -> Self {
        match theme {
            AppTheme::ClassicLight => Self {
                is_light_classic: true,
                bg_canvas: Color32::from_rgb(255, 255, 255),
                bg_panel: Color32::from_rgb(255, 255, 255),
                bg_card: Color32::from_rgb(255, 255, 255),
                bg_card_selected: Color32::from_rgb(232, 243, 255),
                bg_chrome_dark: Color32::from_rgb(76, 78, 82),
                bg_toolbar: Color32::from_rgb(196, 201, 209),
                pane_header_active: Color32::from_rgb(244, 179, 89),
                pane_header_active_text: Color32::from_rgb(30, 25, 20),
                pane_header_inactive: Color32::from_rgb(138, 142, 149),
                pane_header_inactive_text: Color32::from_rgb(250, 250, 252),
                section_band: Color32::from_rgb(225, 228, 232),
                border_subtle: Color32::from_rgb(176, 181, 188),
                accent: Color32::from_rgb(0, 120, 215),
                accent_soft: Color32::from_rgb(218, 235, 252),
                text_primary: Color32::from_rgb(24, 26, 30),
                text_muted: Color32::from_rgb(100, 108, 118),
                badge_cdr: Color32::from_rgb(22, 148, 68),
                badge_gs: Color32::from_rgb(205, 42, 42),
                badge_modern: Color32::from_rgb(136, 56, 214),
                badge_vector: Color32::from_rgb(208, 120, 16),
                badge_raster: Color32::from_rgb(0, 120, 215),
                star_gold: Color32::from_rgb(222, 158, 12),
            },
            AppTheme::StudioNeutral => Self {
                is_light_classic: false,
                bg_canvas: Color32::from_rgb(56, 60, 66),
                bg_panel: Color32::from_rgb(47, 51, 57),
                bg_card: Color32::from_rgb(66, 71, 79),
                bg_card_selected: Color32::from_rgb(58, 92, 138),
                bg_chrome_dark: Color32::from_rgb(38, 41, 47),
                bg_toolbar: Color32::from_rgb(52, 56, 63),
                pane_header_active: Color32::from_rgb(228, 162, 74),
                pane_header_active_text: Color32::from_rgb(22, 20, 18),
                pane_header_inactive: Color32::from_rgb(70, 75, 84),
                pane_header_inactive_text: Color32::from_rgb(235, 240, 246),
                section_band: Color32::from_rgb(60, 65, 73),
                border_subtle: Color32::from_rgb(82, 88, 98),
                accent: Color32::from_rgb(96, 165, 250),
                accent_soft: Color32::from_rgb(45, 70, 105),
                text_primary: Color32::from_rgb(238, 242, 246),
                text_muted: Color32::from_rgb(168, 178, 192),
                badge_cdr: Color32::from_rgb(34, 197, 94),
                badge_gs: Color32::from_rgb(248, 113, 113),
                badge_modern: Color32::from_rgb(192, 132, 252),
                badge_vector: Color32::from_rgb(251, 191, 36),
                badge_raster: Color32::from_rgb(96, 165, 250),
                star_gold: Color32::from_rgb(250, 204, 21),
            },
            AppTheme::Darkroom => Self {
                is_light_classic: false,
                bg_canvas: Color32::from_rgb(22, 25, 31),
                bg_panel: Color32::from_rgb(28, 32, 40),
                bg_card: Color32::from_rgb(34, 39, 49),
                bg_card_selected: Color32::from_rgb(35, 62, 102),
                bg_chrome_dark: Color32::from_rgb(18, 21, 27),
                bg_toolbar: Color32::from_rgb(26, 30, 38),
                pane_header_active: Color32::from_rgb(56, 132, 198),
                pane_header_active_text: Color32::WHITE,
                pane_header_inactive: Color32::from_rgb(40, 46, 58),
                pane_header_inactive_text: Color32::from_rgb(215, 224, 236),
                section_band: Color32::from_rgb(38, 44, 56),
                border_subtle: Color32::from_rgb(52, 60, 74),
                accent: Color32::from_rgb(56, 189, 248),
                accent_soft: Color32::from_rgb(24, 56, 82),
                text_primary: Color32::from_rgb(236, 241, 248),
                text_muted: Color32::from_rgb(148, 163, 184),
                badge_cdr: Color32::from_rgb(34, 197, 94),
                badge_gs: Color32::from_rgb(239, 68, 68),
                badge_modern: Color32::from_rgb(168, 85, 247),
                badge_vector: Color32::from_rgb(245, 158, 11),
                badge_raster: Color32::from_rgb(59, 130, 246),
                star_gold: Color32::from_rgb(250, 204, 21),
            },
            AppTheme::HighContrast => Self {
                is_light_classic: false,
                bg_canvas: Color32::from_rgb(10, 10, 12),
                bg_panel: Color32::from_rgb(18, 18, 22),
                bg_card: Color32::from_rgb(26, 26, 32),
                bg_card_selected: Color32::from_rgb(0, 82, 165),
                bg_chrome_dark: Color32::from_rgb(0, 0, 0),
                bg_toolbar: Color32::from_rgb(22, 22, 28),
                pane_header_active: Color32::from_rgb(255, 214, 10),
                pane_header_active_text: Color32::BLACK,
                pane_header_inactive: Color32::from_rgb(50, 50, 60),
                pane_header_inactive_text: Color32::WHITE,
                section_band: Color32::from_rgb(38, 38, 46),
                border_subtle: Color32::from_rgb(140, 140, 155),
                accent: Color32::from_rgb(255, 214, 10),
                accent_soft: Color32::from_rgb(65, 58, 10),
                text_primary: Color32::from_rgb(255, 255, 255),
                text_muted: Color32::from_rgb(210, 215, 225),
                badge_cdr: Color32::from_rgb(46, 230, 108),
                badge_gs: Color32::from_rgb(255, 92, 92),
                badge_modern: Color32::from_rgb(216, 130, 255),
                badge_vector: Color32::from_rgb(255, 200, 40),
                badge_raster: Color32::from_rgb(80, 180, 255),
                star_gold: Color32::from_rgb(255, 220, 0),
            },
        }
    }

    pub fn format_badge_color(&self, filetype: &str) -> Color32 {
        match filetype.to_ascii_uppercase().as_str() {
            "CDR" | "CMX" | "CCX" | "PAT" => self.badge_cdr,
            "PSD" | "PSB" => Color32::from_rgb(38, 128, 235),
            "GIF" => Color32::from_rgb(14, 159, 148),
            "PDF" | "EPS" | "AI" | "PS" | "PRN" => self.badge_gs,
            "AVIF" | "WEBP" | "JXL" => self.badge_modern,
            "SVG" | "SVGZ" => self.badge_vector,
            ext if crate::decoder::is_common_raster_ext(ext) => self.badge_raster,
            _ => Color32::from_rgb(100, 110, 124),
        }
    }
}

pub fn apply_theme(ctx: &egui::Context, theme: AppTheme) -> ThemePalette {
    let pal = ThemePalette::for_theme(theme);
    let mut visuals = match theme {
        AppTheme::ClassicLight => Visuals::light(),
        _ => Visuals::dark(),
    };
    visuals.panel_fill = pal.bg_panel;
    visuals.window_fill = pal.bg_panel;
    visuals.extreme_bg_color = pal.bg_canvas;
    visuals.faint_bg_color = pal.section_band;
    visuals.selection.bg_fill = pal.accent;
    visuals.selection.stroke = Stroke::new(
        1.0_f32,
        if pal.is_light_classic {
            Color32::WHITE
        } else {
            pal.text_primary
        },
    );
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, pal.border_subtle);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(3);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(3);
    visuals.widgets.active.corner_radius = CornerRadius::same(3);
    ctx.set_visuals(visuals);
    pal
}

pub fn draw_pane_title_bar(ui: &mut egui::Ui, title: &str, active: bool, pal: &ThemePalette) {
    let (fill, text_col) = if active {
        (pal.pane_header_active, pal.pane_header_active_text)
    } else {
        (pal.pane_header_inactive, pal.pane_header_inactive_text)
    };
    egui::Frame::NONE
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(title).strong().size(11.5).color(text_col))
                        .truncate(),
                );
            });
        });
}
