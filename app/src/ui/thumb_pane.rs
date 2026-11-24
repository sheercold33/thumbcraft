use crate::config::{SearchScope, SortField, ViewMode};
use crate::decoder::ImageFilter;
use crate::ui::theme::ThemePalette;
use crate::ui::{BrowseSource, MediaItem, ThumbCraftApp};
use egui::{Color32, CornerRadius, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::path::PathBuf;

#[derive(Debug, Clone)]
enum ContextAction {
    OpenExternal(usize),
    ViewInternal(usize),
    ShowProperties(usize),
    QuickFilter(usize, ImageFilter),
    MoveFile(usize),
    CopyFile(usize),
    RenamePrompt(usize),
    DeletePrompt(usize),
    RebuildSelected,
    SetRating(usize, u8),
    BrowseKeyword(String),
    AddToGallery(usize, String),
    CopyText(String, &'static str),
    ShowSimilar(usize),
    FindDuplicates,
    GoToFolder(PathBuf),
    RevealInExplorer(PathBuf),
    OpenConfigure,
}

pub fn show_thumb_pane(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    app.ensure_visible_indices_cached();
    let visible_count = app.cached_visible_indices.len();

    // Active orange/gold pane title bar matching reference/picture-1.png
    let full_title = app.browse_title();
    crate::ui::theme::draw_pane_title_bar(ui, &full_title, true, pal);

    // Silver-gray sub-toolbar inside Thumbnail Pane (Refresh, View Mode, Zoom %, Filter, Sort)
    egui::Frame::NONE
        .fill(pal.bg_toolbar)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                if ui.small_button("Reload").on_hover_text("Re-scan folder").clicked() {
                    app.reload_current_source(true);
                }
                ui.separator();
                for mode in [ViewMode::Thumbnails, ViewMode::Details, ViewMode::Filmstrip] {
                    if ui
                        .selectable_label(app.settings.view_mode == mode, mode.label())
                        .clicked()
                    {
                        app.settings.view_mode = mode;
                        app.settings.save();
                    }
                }
                ui.separator();
                let pct = ((app.settings.thumb_size / 120.0) * 100.0).round() as i32;
                ui.label(RichText::new(format!("{pct}%")).size(11.0).color(pal.text_primary));
                ui.spacing_mut().slider_width = 68.0;
                if ui
                    .add(
                        egui::Slider::new(&mut app.settings.thumb_size, 64.0..=260.0)
                            .show_value(false),
                    )
                    .changed()
                {
                    app.settings.save();
                }

                ui.separator();
                egui::ComboBox::from_id_salt("thumb_bar_format")
                    .width(90.0)
                    .selected_text(match app.format_filter.as_str() {
                        "CDR" => "CDR only",
                        "PSDPSB" => "PSD / PSB",
                        "PSD" => "PSD only",
                        "PSB" => "PSB only",
                        "GIF" => "GIF only",
                        "GS" => "PDF/AI/EPS",
                        "MODERN" => "AVIF/WEBP/JXL",
                        "SVG" => "SVG only",
                        "RASTER" => "Raster",
                        "OTHER" => "Other files",
                        _ => "All files",
                    })
                    .show_ui(ui, |ui| {
                        for (id, lbl) in [
                            ("ALL", "All files"),
                            ("CDR", "CDR / CMX"),
                            ("PSDPSB", "Photoshop (PSD / PSB)"),
                            ("PSD", "PSD only"),
                            ("PSB", "PSB only"),
                            ("GIF", "GIF"),
                            ("GS", "PDF / AI / EPS"),
                            ("MODERN", "AVIF / WEBP / JXL"),
                            ("SVG", "SVG"),
                            ("RASTER", "Common Raster"),
                            ("OTHER", "Other Files (Icons)"),
                        ] {
                            ui.selectable_value(&mut app.format_filter, id.to_string(), lbl);
                        }
                    });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let dir_lbl = if app.settings.sort_descending {
                        "Desc"
                    } else {
                        "Asc"
                    };
                    if ui.small_button(dir_lbl).clicked() {
                        app.settings.sort_descending = !app.settings.sort_descending;
                        app.settings.save();
                    }
                    egui::ComboBox::from_id_salt("sort_combo")
                        .width(90.0)
                        .selected_text(app.settings.sort_field.label())
                        .show_ui(ui, |ui| {
                            for f in SortField::all() {
                                if ui
                                    .selectable_value(&mut app.settings.sort_field, *f, f.label())
                                    .clicked()
                                {
                                    if *f == SortField::Similarity {
                                        app.similarity_sort_anchor =
                                            app.selected_item().map(|it| it.full_path.clone());
                                        app.similarity_sort_epoch =
                                            app.similarity_sort_epoch.wrapping_add(1);
                                    }
                                    app.settings.save();
                                }
                            }
                        });
                    ui.label(
                        RichText::new(format!("{}/{}", visible_count, app.items.len()))
                            .size(11.0)
                            .color(pal.text_muted),
                    );
                });
            });
        });

    // Dedicated interactive bar when browsing Visual Similarity or Hash Duplicates
    show_special_mode_banner(ui, app, pal);
    ui.add_space(2.0);

    // Re-validate cache in case toolbar/banner buttons modified filters, sort, or items
    app.ensure_visible_indices_cached();
    if app.cached_visible_indices.is_empty() {
        ui.centered_and_justified(|ui| {
            let msg = match &app.browse_source {
                BrowseSource::Duplicates => {
                    format!(
                        "No exact duplicate files found across {} hashed database item(s).\nClick 'Index Folder + Subfolders' or browse more folders to catalog more files.",
                        app.duplicate_scan_count
                    )
                }
                BrowseSource::Similar(_) => {
                    "No cataloged images matched above the current similarity threshold.\nLower the Similarity slider above or catalog more folders.".to_string()
                }
                _ if app.items.is_empty() => {
                    "No supported image, vector, CDR, or PDF files in this view.\nChoose a folder on the left or click 'Open Folder'.".to_string()
                }
                _ => "No items match the current search / filter criteria.".to_string(),
            };
            ui.label(RichText::new(msg).size(13.0).color(pal.text_muted));
        });
        return;
    }

    // Zero-copy swap out of app so view renderers can borrow &mut app alongside &[usize]
    let visible_indices = std::mem::take(&mut app.cached_visible_indices);
    let action = match app.settings.view_mode {
        ViewMode::Thumbnails => show_grid_view(ui, app, &visible_indices, pal),
        ViewMode::Details => show_details_view(ui, app, &visible_indices, pal),
        ViewMode::Filmstrip => show_filmstrip_view(ui, app, &visible_indices, pal),
    };
    app.cached_visible_indices = visible_indices;

    if let Some(act) = action {
        handle_context_action(ui.ctx(), app, act);
    }

    // Click+drag trigger (once pointer drags >= 7px from press origin, or >= 2px when Alt/Win is pressed):
    // Starts internal drag (drop onto Drives & Folders to move) and auto-hands off to native
    // Windows OLE DoDragDrop if dragged outside the window or when Alt+Tabbing into Photoshop, CorelDRAW, etc.
    let (primary_down, cur_pos) =
        ui.input(|i| (i.pointer.primary_down(), i.pointer.interact_pos()));
    let os_primary_down = crate::drag_drop::is_primary_mouse_button_down();
    let switch_mod = crate::drag_drop::is_switch_modifier_down();
    if !primary_down && !os_primary_down {
        app.drag_candidate = None;
    } else if let Some((drag_idx, start_pos)) = app.drag_candidate {
        let dist = cur_pos.map(|pos| (pos - start_pos).length()).unwrap_or(0.0);
        let threshold = if switch_mod { 2.0 } else { 7.0 };
        if dist >= threshold || (switch_mod && !primary_down && os_primary_down) {
            app.drag_candidate = None;
            app.begin_internal_drag(drag_idx);
        } else {
            ui.ctx().request_repaint();
        }
    }
}

fn show_special_mode_banner(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    match app.browse_source.clone() {
        BrowseSource::Similar(target_path) => {
            let mut rerun = false;
            let mut open_opts = false;
            egui::Frame::NONE
                .fill(pal.accent_soft)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Similar:")
                                .strong()
                                .size(11.5)
                                .color(pal.text_primary),
                        );
                        for scope in SearchScope::all() {
                            if ui
                                .selectable_value(
                                    &mut app.settings.search_scope,
                                    *scope,
                                    scope.label(),
                                )
                                .clicked()
                            {
                                app.settings.save();
                                rerun = true;
                            }
                        }
                        ui.separator();
                        ui.label(RichText::new("Min:").size(11.0));
                        let mut pct = (app.similarity_threshold * 100.0).round() as i32;
                        if ui
                            .add(egui::Slider::new(&mut pct, 30..=100).suffix("%"))
                            .changed()
                        {
                            app.similarity_threshold = pct as f32 / 100.0;
                            rerun = true;
                        }
                        ui.label(RichText::new("Color:").size(11.0));
                        let mut cw = (app.similarity_color_weight * 100.0).round() as i32;
                        if ui
                            .add(egui::Slider::new(&mut cw, 0..=100).suffix("%"))
                            .on_hover_text("Color metric contribution to similarity (default 40%)")
                            .changed()
                        {
                            app.similarity_color_weight = cw as f32 / 100.0;
                            rerun = true;
                        }
                        if ui.small_button("Options...").clicked() {
                            open_opts = true;
                        }
                        if ui.small_button("Refresh").clicked() {
                            rerun = true;
                        }
                    });
                });
            if open_opts {
                app.open_similar_dialog(target_path);
            } else if rerun {
                app.open_similar_view(target_path);
            }
        }
        BrowseSource::Duplicates => {
            let mut rescan = false;
            let total_dups: usize = app.duplicate_groups.iter().map(|g| g.paths.len()).sum();
            let reclaimable: u64 = app
                .duplicate_groups
                .iter()
                .map(|g| g.file_size * (g.paths.len().saturating_sub(1) as u64))
                .sum();
            egui::Frame::NONE
                .fill(pal.accent_soft)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Duplicates Scope:")
                                .strong()
                                .size(11.5)
                                .color(pal.text_primary),
                        );
                        for scope in SearchScope::all() {
                            if ui
                                .selectable_value(
                                    &mut app.settings.search_scope,
                                    *scope,
                                    scope.label(),
                                )
                                .clicked()
                            {
                                app.settings.save();
                                rescan = true;
                            }
                        }
                        ui.separator();
                        ui.label(
                            RichText::new(format!(
                                "{} group(s), {} file(s) ({} reclaimable) across {} scanned",
                                app.duplicate_groups.len(),
                                total_dups,
                                format_bytes(reclaimable),
                                app.duplicate_scan_count
                            ))
                            .size(11.0)
                            .color(pal.text_primary),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Re-scan Hashes").clicked() {
                                rescan = true;
                            }
                        });
                    });
                });
            if rescan {
                app.open_duplicates_view();
            }
        }
        _ => {}
    }
}

fn show_grid_view(
    ui: &mut Ui,
    app: &mut ThumbCraftApp,
    visible: &[usize],
    pal: &ThemePalette,
) -> Option<ContextAction> {
    let thumb_px = app.settings.thumb_size.clamp(64.0, 320.0);
    let card_w = thumb_px + 22.0;
    let mut caption_h = 12.0;
    if app.settings.captions.show_name {
        caption_h += 18.0;
    }
    if app.settings.captions.show_size || app.settings.captions.show_dimensions {
        caption_h += 15.0;
    }
    if app.settings.captions.show_rating {
        caption_h += 15.0;
    }
    if app.settings.captions.show_keywords {
        caption_h += 18.0;
    }
    let card_h = thumb_px + caption_h;
    let spacing = app.settings.layout.card_spacing.clamp(4.0, 20.0);
    let avail_w = ui.available_width().max(card_w + spacing);
    let cols = ((avail_w + spacing) / (card_w + spacing)).floor().max(1.0) as usize;

    let mut clicked_idx: Option<(usize, bool, bool)> = None;
    let mut right_clicked_idx: Option<usize> = None;
    let mut double_clicked_idx: Option<usize> = None;
    let mut drag_started: Option<(usize, egui::Pos2)> = None;
    let mut context_action: Option<ContextAction> = None;
    let sel_count = app.selected_indices.len().max(1);
    let total_rows = (visible.len() + cols - 1) / cols;
    let row_stride = card_h + spacing;
    let ctx = ui.ctx().clone();
    let checker_tex = if app.settings.layout.checkerboard_alpha {
        Some(app.checkerboard_texture_id(&ctx))
    } else {
        None
    };
    let (ctrl_held, shift_held) = ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
    let mut prefetch_range: Option<std::ops::Range<usize>> = None;
    let mut cold_icons_loaded = 0usize;
    let mut need_more_icons = false;

    ScrollArea::vertical()
        .id_salt("thumb_grid_scroll")
        .auto_shrink([false, false])
        .show_rows(ui, row_stride, total_rows, |ui, row_range| {
            prefetch_range = Some(
                row_range.start.saturating_sub(2)..(row_range.end + 2).min(total_rows),
            );
            for row_idx in row_range {
                let start = row_idx * cols;
                let end = (start + cols).min(visible.len());
                let row_slice = &visible[start..end];

                // Touch visible items and enqueue off-thread WEBP/JXL decoding (or native OS icon for non-media)
                for &item_idx in row_slice {
                    let Some(item) = app.items.get_mut(item_idx) else {
                        continue;
                    };
                    item.last_drawn_frame = app.ui_frame;
                    let is_media = item.is_media;
                    let is_loaded = item.loaded;
                    let has_tex = item.texture.is_some();
                    let has_blob = item.thumb_blob.is_some();
                    if is_media && has_blob {
                        if !has_tex {
                            app.request_item_texture_async(&ctx, item_idx);
                        }
                    } else if (!is_media || is_loaded) && !has_tex {
                        if cold_icons_loaded < 12 {
                            if app.ensure_native_icon(&ctx, item_idx) {
                                cold_icons_loaded += 1;
                            }
                        } else {
                            need_more_icons = true;
                        }
                    }
                }

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = spacing;
                    for &item_idx in row_slice {
                        let Some(item) = app.items.get(item_idx) else {
                            continue;
                        };
                        let is_selected = app.is_item_selected(item_idx);
                        let resp = draw_thumb_card(
                            ui,
                            item,
                            card_w,
                            card_h,
                            thumb_px,
                            is_selected,
                            &app.settings.captions,
                            app.settings.layout.card_rounding,
                            checker_tex,
                            pal,
                        );
                        if resp.clicked() {
                            clicked_idx = Some((item_idx, ctrl_held, shift_held));
                        }
                        if resp.drag_started_by(egui::PointerButton::Primary) {
                            if let Some(origin) = ui.input(|i| {
                                i.pointer.press_origin().or(i.pointer.interact_pos())
                            }) {
                                drag_started = Some((item_idx, origin));
                            }
                        }
                        if resp.secondary_clicked() {
                            right_clicked_idx = Some(item_idx);
                        }
                        if resp.double_clicked() {
                            double_clicked_idx = Some(item_idx);
                        }
                        resp.context_menu(|ui| {
                            show_item_context_menu(
                                ui,
                                item_idx,
                                item,
                                sel_count,
                                &app.all_galleries,
                                &mut context_action,
                            );
                        });
                    }
                });
                ui.add_space(spacing);
            }
        });

    // Prefetch 2 rows ahead & behind off-thread so scrolling lands on pre-decoded textures
    if let Some(p_range) = prefetch_range {
        let mut prefetched = 0usize;
        for row_idx in p_range {
            let start = row_idx * cols;
            let end = (start + cols).min(visible.len());
            for &item_idx in &visible[start..end] {
                if let Some(it) = app.items.get(item_idx) {
                    if it.texture.is_none() && it.thumb_blob.is_some() {
                        app.request_item_texture_async(&ctx, item_idx);
                        prefetched += 1;
                        if prefetched >= 32 {
                            break;
                        }
                    }
                }
            }
            if prefetched >= 32 {
                break;
            }
        }
    }

    if need_more_icons {
        ctx.request_repaint();
    }
    app.evict_distant_textures();

    if let Some(d) = drag_started {
        app.drag_candidate = Some(d);
    }
    if let Some((idx, ctrl, shift)) = clicked_idx {
        if shift {
            app.select_range_to(idx, visible);
        } else if ctrl {
            app.toggle_select_item(idx);
        } else {
            app.select_item(idx);
        }
    }
    if let Some(idx) = right_clicked_idx {
        if !app.is_item_selected(idx) {
            app.select_item(idx);
        }
    }
    if let Some(idx) = double_clicked_idx {
        app.select_item(idx);
        app.open_selected_externally();
    }
    context_action
}

#[allow(clippy::too_many_arguments)]
fn draw_thumb_card(
    ui: &mut Ui,
    item: &MediaItem,
    card_w: f32,
    card_h: f32,
    thumb_px: f32,
    selected: bool,
    captions: &crate::config::CaptionOptions,
    rounding: f32,
    checker_tex: Option<egui::TextureId>,
    pal: &ThemePalette,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(card_w, card_h), Sense::click_and_drag());
    let painter = ui.painter_at(rect);

    let bg = if pal.is_light_classic {
        if resp.hovered() && !selected {
            Color32::from_rgb(243, 247, 252)
        } else {
            Color32::TRANSPARENT
        }
    } else if selected {
        pal.bg_card_selected
    } else if resp.hovered() {
        pal.bg_card.linear_multiply(1.12)
    } else {
        pal.bg_card
    };
    let border = if selected {
        Stroke::new(
            if pal.is_light_classic { 1.5_f32 } else { 2.0_f32 },
            pal.accent,
        )
    } else if pal.is_light_classic {
        if resp.hovered() {
            Stroke::new(1.0_f32, Color32::from_rgb(190, 210, 235))
        } else {
            Stroke::NONE
        }
    } else {
        Stroke::new(1.0_f32, pal.border_subtle)
    };
    let cr = CornerRadius::same(rounding.clamp(0.0, 14.0) as u8);
    painter.rect(rect, cr, bg, border, StrokeKind::Inside);

    // Thumbnail image viewport inside card
    let img_rect = Rect::from_min_size(
        egui::pos2(rect.min.x + 11.0, rect.min.y + 8.0),
        Vec2::new(thumb_px, thumb_px),
    );
    painter.rect_filled(img_rect, CornerRadius::same(3), pal.bg_canvas);

    if item.is_media {
        if let Some(tex_id) = checker_tex {
            draw_checkerboard(&painter, img_rect, tex_id);
        }
    }

    if let Some(ref tex) = item.texture {
        let tex_size = tex.size_vec2();
        let draw_sz = if item.is_media {
            let scale = (img_rect.width() / tex_size.x.max(1.0))
                .min(img_rect.height() / tex_size.y.max(1.0));
            tex_size * scale
        } else {
            let max_icon = (img_rect.width() * 0.72)
                .min(img_rect.height() * 0.72)
                .min(tex_size.x.max(tex_size.y).max(64.0));
            let scale = (max_icon / tex_size.x.max(1.0))
                .min(max_icon / tex_size.y.max(1.0));
            tex_size * scale
        };
        let draw_rect = Rect::from_center_size(img_rect.center(), draw_sz);
        painter.image(
            tex.id(),
            draw_rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        let placeholder_text = if !item.is_media {
            item.filetype.clone()
        } else if item.loaded {
            format!("{}\nNo Preview", item.filetype)
        } else {
            "Loading...".to_string()
        };
        painter.text(
            img_rect.center(),
            egui::Align2::CENTER_CENTER,
            placeholder_text,
            egui::FontId::proportional(11.5),
            pal.text_muted,
        );
    }

    // Top-left Similarity or Duplicate group badge when in Similar/Duplicates view
    if let Some(sim) = item.similarity_score {
        let pct = (sim * 100.0).round() as i32;
        let col = if pct >= 90 {
            Color32::from_rgb(22, 163, 74)
        } else if pct >= 75 {
            Color32::from_rgb(13, 148, 136)
        } else {
            Color32::from_rgb(217, 119, 6)
        };
        draw_corner_badge(&painter, img_rect, false, &format!("{pct}%"), col);
    } else if let Some(grp) = item.duplicate_group {
        draw_corner_badge(
            &painter,
            img_rect,
            false,
            &format!("DUP #{grp}"),
            Color32::from_rgb(220, 38, 38),
        );
    }

    // Format badge pill at top-right of thumbnail
    if captions.show_format_badge {
        let badge_col = pal.format_badge_color(&item.filetype);
        let badge_txt = if item.page_count > 1 {
            format!("{} ({}p)", item.filetype, item.page_count)
        } else {
            item.filetype.clone()
        };
        draw_corner_badge(&painter, img_rect, true, &badge_txt, badge_col);
    }

    // Caption area below thumbnail
    let mut y = img_rect.max.y + 5.0;
    let x_left = rect.min.x + 8.0;
    let max_text_w = card_w - 16.0;

    if captions.show_name {
        let name = truncate_chars(&item.file_name, (max_text_w / 6.8) as usize);
        if selected && pal.is_light_classic {
            let pill = Rect::from_min_size(
                egui::pos2(rect.min.x + 5.0, y - 1.0),
                Vec2::new(card_w - 10.0, 16.0),
            );
            painter.rect_filled(pill, CornerRadius::same(2), pal.accent);
            painter.text(
                egui::pos2(rect.center().x, y),
                egui::Align2::CENTER_TOP,
                name,
                egui::FontId::proportional(11.5),
                Color32::WHITE,
            );
        } else {
            painter.text(
                egui::pos2(rect.center().x, y),
                egui::Align2::CENTER_TOP,
                name,
                egui::FontId::proportional(11.5),
                pal.text_primary,
            );
        }
        y += 16.0;
    }

    if captions.show_size || captions.show_dimensions {
        let mut parts = Vec::new();
        if captions.show_dimensions && item.width > 0 && item.height > 0 {
            parts.push(format!("{}×{}", item.width, item.height));
        }
        if captions.show_size {
            parts.push(format_bytes(item.file_length));
        }
        if !parts.is_empty() {
            painter.text(
                egui::pos2(rect.center().x, y),
                egui::Align2::CENTER_TOP,
                parts.join(" • "),
                egui::FontId::proportional(10.0),
                pal.text_muted,
            );
            y += 14.0;
        }
    }

    if captions.show_rating {
        let stars: String = (1..=5)
            .map(|i| if i <= item.rating { '★' } else { '☆' })
            .collect();
        painter.text(
            egui::pos2(rect.center().x, y),
            egui::Align2::CENTER_TOP,
            stars,
            egui::FontId::proportional(10.5),
            if item.rating > 0 {
                pal.star_gold
            } else {
                pal.text_muted.gamma_multiply(0.45)
            },
        );
        y += 14.0;
    }

    if captions.show_keywords && !item.keywords.is_empty() {
        let kw_preview = item
            .keywords
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let kw_txt = format!(
            "# {}",
            truncate_chars(&kw_preview, (max_text_w / 6.2) as usize)
        );
        painter.text(
            egui::pos2(x_left, y),
            egui::Align2::LEFT_TOP,
            kw_txt,
            egui::FontId::proportional(9.5),
            pal.accent,
        );
    }

    // Hover tooltip with full details + keywords + double-click hint
    resp.clone().on_hover_ui(|ui| {
        ui.label(RichText::new(&item.file_name).strong());
        if item.width > 0 && item.height > 0 {
            ui.label(format!(
                "{} | {}×{} | {} | {}",
                item.filetype,
                item.width,
                item.height,
                format_bytes(item.file_length),
                item.source_note
            ));
        } else {
            ui.label(format!(
                "{} | {} | {}",
                item.filetype,
                format_bytes(item.file_length),
                item.source_note
            ));
        }
        if let Some(sim) = item.similarity_score {
            ui.label(format!("Visual similarity: {:.1}%", sim * 100.0));
        }
        if let Some(grp) = item.duplicate_group {
            ui.label(format!("Exact duplicate group #{grp} (MD5: {})", item.md5));
        }
        if !item.keywords.is_empty() {
            ui.label(
                RichText::new(format!("Keywords: {}", item.keywords.join(", ")))
                    .color(pal.accent),
            );
        }
        ui.label(
            RichText::new("Double-click: Open externally • Right-click: Options")
                .size(10.0)
                .color(pal.text_muted),
        );
    });

    resp
}

fn draw_corner_badge(
    painter: &egui::Painter,
    img_rect: Rect,
    right_side: bool,
    text: &str,
    bg: Color32,
) {
    let galley = painter.layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(9.5),
        Color32::WHITE,
    );
    let badge_sz = galley.size() + Vec2::new(8.0, 3.0);
    let x = if right_side {
        img_rect.max.x - badge_sz.x - 3.0
    } else {
        img_rect.min.x + 3.0
    };
    let badge_rect = Rect::from_min_size(egui::pos2(x, img_rect.min.y + 3.0), badge_sz);
    painter.rect_filled(badge_rect, CornerRadius::same(3), bg);
    painter.galley(
        egui::pos2(badge_rect.min.x + 4.0, badge_rect.min.y + 1.5),
        galley,
        Color32::WHITE,
    );
}

/// Right-click context menu matching reference/picture-7.png + F3 Rebuild Selected Thumbnails
fn show_item_context_menu(
    ui: &mut Ui,
    idx: usize,
    item: &MediaItem,
    selected_count: usize,
    galleries: &[(String, usize)],
    out: &mut Option<ContextAction>,
) {
    ui.set_min_width(230.0);

    if ui.button("Open Externally       (Double-Click)").clicked() {
        *out = Some(ContextAction::OpenExternal(idx));
        ui.close_menu();
    }
    if ui.button("View                         Ctrl+Enter").clicked() {
        *out = Some(ContextAction::ViewInternal(idx));
        ui.close_menu();
    }
    if ui.button("Edit                         Shift+Enter").clicked() {
        *out = Some(ContextAction::OpenExternal(idx));
        ui.close_menu();
    }
    if ui.button("Properties                 Alt+Enter").clicked() {
        *out = Some(ContextAction::ShowProperties(idx));
        ui.close_menu();
    }
    ui.menu_button("Quick Process", |ui| {
        for f in ImageFilter::all() {
            if ui.button(f.label()).clicked() {
                *out = Some(ContextAction::QuickFilter(idx, *f));
                ui.close_menu();
            }
        }
    });

    ui.separator();

    let move_lbl = if selected_count > 1 {
        format!("Move Selected ({selected_count}) to Folder...")
    } else {
        "Move to Folder...".to_string()
    };
    if ui.button(&move_lbl).clicked() {
        *out = Some(ContextAction::MoveFile(idx));
        ui.close_menu();
    }
    let copy_lbl = if selected_count > 1 {
        format!("Copy Selected ({selected_count}) to Folder...")
    } else {
        "Copy to Folder...".to_string()
    };
    if ui.button(&copy_lbl).clicked() {
        *out = Some(ContextAction::CopyFile(idx));
        ui.close_menu();
    }
    if ui.button("Rename...                            F2").clicked() {
        *out = Some(ContextAction::RenamePrompt(idx));
        ui.close_menu();
    }
    let delete_lbl = if selected_count > 1 {
        format!("Delete Selected ({selected_count})...  Del / Ctrl+D")
    } else {
        "Delete...                  Del / Ctrl+D".to_string()
    };
    if ui.button(&delete_lbl).clicked() {
        *out = Some(ContextAction::DeletePrompt(idx));
        ui.close_menu();
    }

    ui.separator();

    let rebuild_lbl = if selected_count > 1 {
        format!("Rebuild Selected Thumbnails ({selected_count})   F3")
    } else {
        "Rebuild Selected Thumbnail        F3".to_string()
    };
    if ui.button(RichText::new(&rebuild_lbl).strong()).clicked() {
        *out = Some(ContextAction::RebuildSelected);
        ui.close_menu();
    }

    ui.menu_button("Thumbnail", |ui| {
        if ui.button(&rebuild_lbl).clicked() {
            *out = Some(ContextAction::RebuildSelected);
            ui.close_menu();
        }
        ui.separator();
        ui.label(RichText::new("Set Rating").strong().size(11.0));
        for r in 0..=5u8 {
            let lbl = if r == 0 {
                "Clear Rating".to_string()
            } else {
                format!("{r} ★")
            };
            if ui.button(lbl).clicked() {
                *out = Some(ContextAction::SetRating(idx, r));
                ui.close_menu();
            }
        }
    });

    ui.menu_button("Keywords", |ui| {
        if ui.button("Edit Keywords in Metadata Pane").clicked() {
            *out = Some(ContextAction::ShowProperties(idx));
            ui.close_menu();
        }
        if !item.keywords.is_empty() {
            ui.separator();
            ui.label(RichText::new("Find by Keyword:").size(10.5));
            for kw in &item.keywords {
                if ui.button(format!("# {kw}")).clicked() {
                    *out = Some(ContextAction::BrowseKeyword(kw.clone()));
                    ui.close_menu();
                }
            }
        }
    });

    ui.menu_button("Gallery", |ui| {
        if galleries.is_empty() {
            ui.label("No galleries yet");
        } else {
            for (g, _) in galleries {
                if ui.button(format!("Add to '{g}'")).clicked() {
                    *out = Some(ContextAction::AddToGallery(idx, g.clone()));
                    ui.close_menu();
                }
            }
        }
    });

    ui.menu_button("Clipboard Copy", |ui| {
        if ui.button("Copy Full Path").clicked() {
            *out = Some(ContextAction::CopyText(
                item.full_path.to_string_lossy().to_string(),
                "full path",
            ));
            ui.close_menu();
        }
        if ui.button("Copy File Name").clicked() {
            *out = Some(ContextAction::CopyText(item.file_name.clone(), "file name"));
            ui.close_menu();
        }
        if !item.keywords.is_empty() && ui.button("Copy Keywords").clicked() {
            *out = Some(ContextAction::CopyText(
                item.keywords.join(", "),
                "keywords",
            ));
            ui.close_menu();
        }
        if !item.md5.is_empty() && ui.button("Copy MD5 Hash").clicked() {
            *out = Some(ContextAction::CopyText(item.md5.clone(), "MD5 hash"));
            ui.close_menu();
        }
    });

    ui.separator();

    if ui
        .button(RichText::new("Show Similar                     Alt+0").strong())
        .clicked()
    {
        *out = Some(ContextAction::ShowSimilar(idx));
        ui.close_menu();
    }
    if ui.button("Find Exact Duplicates (Hash Scan)").clicked() {
        *out = Some(ContextAction::FindDuplicates);
        ui.close_menu();
    }
    if let Some(parent) = item.full_path.parent() {
        if ui.button("Go to Folder").clicked() {
            *out = Some(ContextAction::GoToFolder(parent.to_path_buf()));
            ui.close_menu();
        }
    }
    if ui.button("Reveal in Windows Explorer").clicked() {
        *out = Some(ContextAction::RevealInExplorer(item.full_path.clone()));
        ui.close_menu();
    }
    if ui.button("Configure File Type...         Alt+Y").clicked() {
        *out = Some(ContextAction::OpenConfigure);
        ui.close_menu();
    }
}

fn handle_context_action(ctx: &egui::Context, app: &mut ThumbCraftApp, action: ContextAction) {
    match action {
        ContextAction::OpenExternal(idx) => {
            app.select_item(idx);
            app.open_selected_externally();
        }
        ContextAction::ViewInternal(idx) => {
            app.select_item(idx);
            app.open_lightbox();
        }
        ContextAction::ShowProperties(idx) => {
            app.select_item(idx);
            app.settings.layout.show_info_pane = true;
            app.settings.save();
        }
        ContextAction::QuickFilter(idx, filter) => {
            app.select_item(idx);
            app.settings.layout.show_preview_pane = true;
            app.active_filter = filter;
            app.rebuild_filtered_preview();
        }
        ContextAction::MoveFile(idx) => {
            if !app.is_item_selected(idx) {
                app.select_item(idx);
            }
            app.move_selected_files_to_folder();
        }
        ContextAction::CopyFile(idx) => {
            if !app.is_item_selected(idx) {
                app.select_item(idx);
            }
            app.copy_selected_files_to_folder();
        }
        ContextAction::RenamePrompt(idx) => {
            app.select_item(idx);
            app.begin_rename_selected();
        }
        ContextAction::DeletePrompt(idx) => {
            if !app.is_item_selected(idx) {
                app.select_item(idx);
            }
            app.begin_delete_selected();
        }
        ContextAction::RebuildSelected => {
            app.rebuild_selected_thumbnails();
        }
        ContextAction::SetRating(idx, r) => {
            app.select_item(idx);
            app.set_selected_rating(r);
        }
        ContextAction::BrowseKeyword(kw) => {
            app.open_keyword_view(kw);
        }
        ContextAction::AddToGallery(idx, gal) => {
            app.select_item(idx);
            app.add_selected_to_gallery(&gal);
        }
        ContextAction::CopyText(text, what) => {
            ctx.copy_text(text);
            app.push_log(format!("Copied {what} to clipboard."));
        }
        ContextAction::ShowSimilar(idx) => {
            app.select_item(idx);
            if let Some(item) = app.selected_item().cloned() {
                app.open_similar_dialog(item.full_path);
            }
        }
        ContextAction::FindDuplicates => {
            app.open_duplicates_view();
        }
        ContextAction::GoToFolder(dir) => {
            app.open_folder(dir);
        }
        ContextAction::RevealInExplorer(path) => {
            app.reveal_in_explorer(&path);
        }
        ContextAction::OpenConfigure => {
            app.show_preferences = true;
            app.prefs_tab = crate::ui::PrefsTab::Ghostscript;
        }
    }
}

fn show_details_view(
    ui: &mut Ui,
    app: &mut ThumbCraftApp,
    visible: &[usize],
    pal: &ThemePalette,
) -> Option<ContextAction> {
    let mut clicked: Option<(usize, bool, bool)> = None;
    let mut right_clicked: Option<usize> = None;
    let mut drag_started: Option<(usize, egui::Pos2)> = None;
    let mut dbl_clicked = None;
    let mut context_action = None;
    let sel_count = app.selected_indices.len().max(1);
    let ctx = ui.ctx().clone();
    let (ctrl_held, shift_held) = ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
    let mut prefetch_range: Option<std::ops::Range<usize>> = None;
    let mut cold_icons_loaded = 0usize;
    let mut need_more_icons = false;

    // Sticky table header
    egui::Frame::NONE
        .fill(pal.bg_toolbar)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add_sized([42.0, 16.0], egui::Label::new(RichText::new("Thumb").strong().size(11.0).color(pal.text_muted)));
                ui.add_sized([220.0, 16.0], egui::Label::new(RichText::new("Name").strong().size(11.0).color(pal.text_muted)));
                ui.add_sized([54.0, 16.0], egui::Label::new(RichText::new("Type").strong().size(11.0).color(pal.text_muted)));
                ui.add_sized([84.0, 16.0], egui::Label::new(RichText::new("Dimensions").strong().size(11.0).color(pal.text_muted)));
                ui.add_sized([72.0, 16.0], egui::Label::new(RichText::new("Size").strong().size(11.0).color(pal.text_muted)));
                ui.add_sized([68.0, 16.0], egui::Label::new(RichText::new("Rating").strong().size(11.0).color(pal.text_muted)));
                ui.label(RichText::new("Keywords").strong().size(11.0).color(pal.text_muted));
            });
        });

    let row_h = 30.0;
    ScrollArea::vertical()
        .id_salt("details_table_scroll")
        .auto_shrink([false, false])
        .show_rows(ui, row_h, visible.len(), |ui, row_range| {
            prefetch_range = Some(
                row_range.start.saturating_sub(8)..(row_range.end + 8).min(visible.len()),
            );
            for row_pos in row_range {
                let idx = visible[row_pos];
                let Some(it) = app.items.get_mut(idx) else {
                    continue;
                };
                it.last_drawn_frame = app.ui_frame;
                let is_media = it.is_media;
                let is_loaded = it.loaded;
                let has_tex = it.texture.is_some();
                let has_blob = it.thumb_blob.is_some();
                if is_media && has_blob {
                    if !has_tex {
                        app.request_item_texture_async(&ctx, idx);
                    }
                } else if (!is_media || is_loaded) && !has_tex {
                    if cold_icons_loaded < 12 {
                        if app.ensure_native_icon(&ctx, idx) {
                            cold_icons_loaded += 1;
                        }
                    } else {
                        need_more_icons = true;
                    }
                }

                let Some(item) = app.items.get(idx) else {
                    continue;
                };
                let selected = app.is_item_selected(idx);
                let row_bg = if selected {
                    pal.accent_soft
                } else if row_pos % 2 == 1 {
                    pal.bg_panel
                } else {
                    Color32::TRANSPARENT
                };

                egui::Frame::NONE
                    .fill(row_bg)
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.set_min_height(row_h - 4.0);
                        ui.horizontal(|ui| {
                            if let Some(ref tex) = item.texture {
                                let img = egui::Image::new((tex.id(), Vec2::new(34.0, 24.0)))
                                    .maintain_aspect_ratio(true);
                                let r_img = ui.add_sized(
                                    [42.0, 24.0],
                                    img.sense(Sense::click_and_drag()),
                                );
                                if r_img.clicked() {
                                    clicked = Some((idx, ctrl_held, shift_held));
                                }
                                if r_img.drag_started_by(egui::PointerButton::Primary) {
                                    if let Some(origin) = ui.input(|i| {
                                        i.pointer.press_origin().or(i.pointer.interact_pos())
                                    }) {
                                        drag_started = Some((idx, origin));
                                    }
                                }
                                if r_img.double_clicked() {
                                    dbl_clicked = Some(idx);
                                }
                            } else {
                                ui.add_sized([42.0, 24.0], egui::Label::new("—"));
                            }

                            let mut display_name = item.file_name.clone();
                            if let Some(sim) = item.similarity_score {
                                display_name = format!("[{:.0}%] {}", sim * 100.0, display_name);
                            } else if let Some(grp) = item.duplicate_group {
                                display_name = format!("[DUP #{grp}] {}", display_name);
                            }
                            let short_name = truncate_chars(&display_name, 30);

                            let r = ui.add_sized(
                                [220.0, 22.0],
                                egui::Button::new(short_name)
                                    .selected(selected)
                                    .frame(false)
                                    .sense(Sense::click_and_drag()),
                            );
                            if r.clicked() {
                                clicked = Some((idx, ctrl_held, shift_held));
                            }
                            if r.drag_started_by(egui::PointerButton::Primary) {
                                if let Some(origin) = ui.input(|i| {
                                    i.pointer.press_origin().or(i.pointer.interact_pos())
                                }) {
                                    drag_started = Some((idx, origin));
                                }
                            }
                            if r.secondary_clicked() {
                                right_clicked = Some(idx);
                            }
                            if r.double_clicked() {
                                dbl_clicked = Some(idx);
                            }
                            r.context_menu(|ui| {
                                show_item_context_menu(
                                    ui,
                                    idx,
                                    item,
                                    sel_count,
                                    &app.all_galleries,
                                    &mut context_action,
                                );
                            });

                            ui.add_sized(
                                [54.0, 20.0],
                                egui::Label::new(
                                    RichText::new(&item.filetype)
                                        .strong()
                                        .color(pal.format_badge_color(&item.filetype)),
                                ),
                            );
                            ui.add_sized(
                                [84.0, 20.0],
                                egui::Label::new(if item.width > 0 {
                                    format!("{}×{}", item.width, item.height)
                                } else {
                                    "—".to_string()
                                }),
                            );
                            ui.add_sized(
                                [72.0, 20.0],
                                egui::Label::new(format_bytes(item.file_length)),
                            );
                            let stars: String = (1..=5)
                                .map(|i| if i <= item.rating { '★' } else { '☆' })
                                .collect();
                            ui.add_sized(
                                [68.0, 20.0],
                                egui::Label::new(RichText::new(stars).color(pal.star_gold)),
                            );
                            ui.label(
                                RichText::new(truncate_chars(&item.keywords.join(", "), 64))
                                    .size(11.0)
                                    .color(pal.accent),
                            );
                        });
                    });
            }
        });

    if let Some(p_range) = prefetch_range {
        for &idx in &visible[p_range] {
            if let Some(it) = app.items.get(idx) {
                if it.texture.is_none() && it.thumb_blob.is_some() {
                    app.request_item_texture_async(&ctx, idx);
                }
            }
        }
    }
    if need_more_icons {
        ctx.request_repaint();
    }
    app.evict_distant_textures();

    if let Some((i, ctrl, shift)) = clicked {
        if shift {
            app.select_range_to(i, visible);
        } else if ctrl {
            app.toggle_select_item(i);
        } else {
            app.select_item(i);
        }
    }
    if let Some(i) = right_clicked {
        if !app.is_item_selected(i) {
            app.select_item(i);
        }
    }
    if let Some(d) = drag_started {
        app.drag_candidate = Some(d);
    }
    if let Some(i) = dbl_clicked {
        app.select_item(i);
        app.open_selected_externally();
    }
    context_action
}

fn show_filmstrip_view(
    ui: &mut Ui,
    app: &mut ThumbCraftApp,
    visible: &[usize],
    pal: &ThemePalette,
) -> Option<ContextAction> {
    let strip_h = 132.0;
    let stage_h = (ui.available_height() - strip_h - 14.0).max(160.0);

    // Top main stage preview
    egui::Frame::NONE
        .fill(pal.bg_canvas)
        .corner_radius(CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_min_size(Vec2::new(ui.available_width(), stage_h));
            ui.set_max_height(stage_h);
            if let Some(ref tex) = app.preview_texture {
                let avail = ui.available_size();
                let ts = tex.size_vec2();
                let sc = (avail.x / ts.x.max(1.0)).min(avail.y / ts.y.max(1.0)) * 0.95;
                ui.centered_and_justified(|ui| {
                    ui.image((tex.id(), ts * sc));
                });
            } else if let Some(idx) = app.selected_index {
                if let Some(ref tex) = app.items.get(idx).and_then(|it| it.texture.as_ref()) {
                    let avail = ui.available_size();
                    let ts = tex.size_vec2();
                    let sc = (avail.x / ts.x.max(1.0)).min(avail.y / ts.y.max(1.0)) * 0.9;
                    ui.centered_and_justified(|ui| {
                        ui.image((tex.id(), ts * sc));
                    });
                }
            }
        });

    ui.add_space(6.0);
    // Horizontal filmstrip along bottom
    let mut clicked: Option<(usize, bool, bool)> = None;
    let mut right_clicked: Option<usize> = None;
    let mut drag_started: Option<(usize, egui::Pos2)> = None;
    let mut dbl = None;
    let mut context_action = None;
    let sel_count = app.selected_indices.len().max(1);
    let ctx = ui.ctx().clone();
    let checker_tex = if app.settings.layout.checkerboard_alpha {
        Some(app.checkerboard_texture_id(&ctx))
    } else {
        None
    };
    let (ctrl_held, shift_held) = ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
    let filmstrip_captions = crate::config::CaptionOptions {
        show_name: true,
        show_size: false,
        show_dimensions: false,
        show_format_badge: true,
        show_rating: false,
        show_keywords: false,
    };
    let mut cold_icons_loaded = 0usize;
    let mut need_more_icons = false;
    ScrollArea::horizontal()
        .id_salt("filmstrip_scroll")
        .show(ui, |ui| {
            let clip = ui.clip_rect();
            let card_w = 108.0_f32;
            let card_h = strip_h - 8.0;
            ui.horizontal(|ui| {
                for &idx in visible {
                    // Virtualize off-screen filmstrip cards so large folders stay 60+ FPS
                    let next_x = ui.cursor().min.x;
                    if next_x + card_w < clip.min.x - 240.0 || next_x > clip.max.x + 240.0 {
                        ui.allocate_space(Vec2::new(card_w, card_h));
                        continue;
                    }
                    let Some(it) = app.items.get_mut(idx) else {
                        continue;
                    };
                    it.last_drawn_frame = app.ui_frame;
                    let is_media = it.is_media;
                    let has_tex = it.texture.is_some();
                    let has_blob = it.thumb_blob.is_some();
                    if is_media {
                        if !has_tex && has_blob {
                            app.request_item_texture_async(&ctx, idx);
                        }
                    } else if !has_tex {
                        if cold_icons_loaded < 12 {
                            if app.ensure_native_icon(&ctx, idx) {
                                cold_icons_loaded += 1;
                            }
                        } else {
                            need_more_icons = true;
                        }
                    }
                    let selected = app.is_item_selected(idx);
                    let Some(item) = app.items.get(idx) else {
                        continue;
                    };
                    let resp = draw_thumb_card(
                        ui,
                        item,
                        card_w,
                        card_h,
                        82.0,
                        selected,
                        &filmstrip_captions,
                        5.0,
                        checker_tex,
                        pal,
                    );
                    if resp.clicked() {
                        clicked = Some((idx, ctrl_held, shift_held));
                    }
                    if resp.drag_started_by(egui::PointerButton::Primary) {
                        if let Some(origin) = ui.input(|i| {
                            i.pointer.press_origin().or(i.pointer.interact_pos())
                        }) {
                            drag_started = Some((idx, origin));
                        }
                    }
                    if resp.secondary_clicked() {
                        right_clicked = Some(idx);
                    }
                    if resp.double_clicked() {
                        dbl = Some(idx);
                    }
                    resp.context_menu(|ui| {
                        show_item_context_menu(
                            ui,
                            idx,
                            item,
                            sel_count,
                            &app.all_galleries,
                            &mut context_action,
                        );
                    });
                }
            });
        });

    if need_more_icons {
        ctx.request_repaint();
    }
    app.evict_distant_textures();

    if let Some(d) = drag_started {
        app.drag_candidate = Some(d);
    }
    if let Some((i, ctrl, shift)) = clicked {
        if shift {
            app.select_range_to(i, visible);
        } else if ctrl {
            app.toggle_select_item(i);
        } else {
            app.select_item(i);
        }
    }
    if let Some(i) = right_clicked {
        if !app.is_item_selected(i) {
            app.select_item(i);
        }
    }
    if let Some(i) = dbl {
        app.select_item(i);
        app.open_selected_externally();
    }
    context_action
}

pub fn draw_checkerboard(painter: &egui::Painter, rect: Rect, checker_tex: egui::TextureId) {
    // Render tiled 20x20 checkerboard texture as a single GPU quad
    let uv = Rect::from_min_max(
        egui::pos2(0.0, 0.0),
        egui::pos2(rect.width() / 20.0, rect.height() / 20.0),
    );
    painter.image(checker_tex, rect, uv, Color32::WHITE);
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let limit = max_chars.max(8);
    if s.chars().count() <= limit {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(limit.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
