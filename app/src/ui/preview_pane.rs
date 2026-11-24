use crate::decoder::ImageFilter;
use crate::ui::theme::ThemePalette;
use crate::ui::thumb_pane::draw_checkerboard;
use crate::ui::ThumbCraftApp;
use egui::{Color32, CornerRadius, Rect, RichText, Ui, Vec2};

pub fn show_preview_pane(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    crate::ui::theme::draw_pane_title_bar(ui, "Preview", false, pal);
    // Compact Preview controls
    ui.horizontal(|ui| {
        ui.label(RichText::new("Filter:").size(10.5).color(pal.text_muted));
        let prev = app.active_filter;
        egui::ComboBox::from_id_salt("preview_filter_combo")
            .width(96.0)
            .selected_text(app.active_filter.label())
            .show_ui(ui, |ui| {
                for f in ImageFilter::all() {
                    ui.selectable_value(&mut app.active_filter, *f, f.label());
                }
            });
        if app.active_filter != prev {
            app.rebuild_filtered_preview();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Full")
                .on_hover_text("Open Fullscreen Lightbox")
                .clicked()
            {
                app.open_lightbox();
            }
            if ui
                .small_button("1:1")
                .on_hover_text("Toggle Fit / 100%")
                .clicked()
            {
                app.preview_zoom = if (app.preview_zoom - 1.0).abs() < 0.05 {
                    0.0
                } else {
                    1.0
                };
            }
            if let Some(item) = app.selected_item() {
                if item.page_count > 1 {
                    let max_p = item.page_count as usize;
                    if ui.small_button("▶").clicked() && app.preview_page < max_p {
                        app.preview_page += 1;
                        app.refresh_preview_image(false);
                    }
                    ui.label(
                        RichText::new(format!("{}/{}", app.preview_page, max_p))
                            .size(10.5)
                            .color(pal.accent),
                    );
                    if ui.small_button("◀").clicked() && app.preview_page > 1 {
                        app.preview_page -= 1;
                        app.refresh_preview_image(false);
                    }
                }
            }
        });
    });

    ui.add_space(2.0);

    // Canvas
    let selected_is_media = app.selected_item().map(|it| it.is_media).unwrap_or(true);
    let checker_tex = if selected_is_media && app.settings.layout.checkerboard_alpha {
        Some(app.checkerboard_texture_id(ui.ctx()))
    } else {
        None
    };
    let avail = ui.available_size();
    let canvas_h = avail.y.max(90.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(avail.x, canvas_h), egui::Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), pal.bg_canvas);
    if let Some(tex_id) = checker_tex {
        draw_checkerboard(&painter, rect, tex_id);
    }

    let tex_opt = app.preview_texture.as_ref().or_else(|| {
        app.selected_index
            .and_then(|i| app.items.get(i))
            .and_then(|it| it.texture.as_ref())
    });

    if let Some(tex) = tex_opt {
        let ts = tex.size_vec2();
        let scale = if !selected_is_media {
            let max_fit = (rect.width() / ts.x.max(1.0)).min(rect.height() / ts.y.max(1.0)) * 0.72;
            max_fit.min(1.5)
        } else if app.preview_zoom <= 0.01 {
            (rect.width() / ts.x.max(1.0)).min(rect.height() / ts.y.max(1.0)) * 0.96
        } else {
            app.preview_zoom
        };
        let draw_rect = Rect::from_center_size(rect.center(), ts * scale);
        painter.image(
            tex.id(),
            draw_rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Select an item\nto preview",
            egui::FontId::proportional(11.5),
            pal.text_muted,
        );
    }

    if resp.double_clicked() {
        app.open_lightbox();
    }
}
