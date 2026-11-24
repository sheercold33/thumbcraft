use crate::decoder::format_category;
use crate::ui::theme::ThemePalette;
use crate::ui::thumb_pane::format_bytes;
use crate::ui::ThumbCraftApp;
use egui::{Color32, CornerRadius, RichText, ScrollArea, Ui};

pub fn show_info_pane(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    crate::ui::theme::draw_pane_title_bar(ui, "Metadata", false, pal);
    ui.add_space(3.0);

    let Some(idx) = app.selected_index else {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("Select a file to inspect\nproperties, CDR keywords,\nand XMP / EXIF metadata.")
                    .size(12.0)
                    .color(pal.text_muted),
            );
        });
        return;
    };

    let Some(item) = app.items.get(idx).cloned() else {
        return;
    };

    let mut set_rating: Option<u8> = None;
    let mut add_kw: Option<String> = None;
    let mut remove_kw: Option<String> = None;
    let mut click_kw_filter: Option<String> = None;
    let mut add_to_gallery: Option<String> = None;
    let mut save_annotation = false;
    let mut open_external = false;
    let mut find_similar = false;
    let mut find_dups = false;

    ScrollArea::vertical()
        .id_salt("info_pane_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // File Header + Format Badge
            ui.horizontal_wrapped(|ui| {
                let col = pal.format_badge_color(&item.filetype);
                egui::Frame::NONE
                    .fill(col)
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.label(RichText::new(&item.filetype).strong().size(10.5).color(Color32::WHITE));
                    });
                ui.label(RichText::new(&item.file_name).strong().size(13.0));
            });

            let cat_label = if item.is_media {
                format_category(&item.filetype)
            } else {
                "Native File"
            };
            ui.label(
                RichText::new(format!("{} • {}", cat_label, item.source_note))
                    .size(11.0)
                    .color(pal.text_muted),
            );

            ui.add_space(6.0);

            // Interactive 5-Star Rating
            ui.horizontal(|ui| {
                ui.label(RichText::new("Rating:").size(11.5).color(pal.text_muted));
                for star in 1..=5u8 {
                    let filled = star <= item.rating;
                    let txt = if filled { "★" } else { "☆" };
                    if ui
                        .button(
                            RichText::new(txt)
                                .size(13.5)
                                .color(if filled { pal.star_gold } else { pal.text_muted }),
                        )
                        .clicked()
                    {
                        let next = if item.rating == star { 0 } else { star };
                        set_rating = Some(next);
                    }
                }
            });

            ui.add_space(6.0);
            ui.separator();

            // KEYWORDS SECTION (CDR + XMP + IPTC + Windows Explorer Tags)
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("KEYWORDS ({})", item.keywords.len()))
                        .size(10.5)
                        .strong()
                        .color(pal.accent),
                );
                if item.filetype == "CDR" || item.filetype == "CMX" {
                    ui.label(
                        RichText::new("CDR Extracted")
                            .size(9.5)
                            .color(pal.badge_cdr),
                    );
                }
                if app.settings.write_keywords_to_files {
                    ui.label(
                        RichText::new("DB + File Sync")
                            .size(9.5)
                            .color(pal.text_muted),
                    )
                    .on_hover_text(
                        "Adding or removing keywords writes to both the catalog database and internally inside the file for Windows Explorer compatibility.",
                    );
                }
            });
            ui.add_space(3.0);

            if item.keywords.is_empty() {
                ui.label(
                    RichText::new("No keywords yet. Add tags below.")
                        .size(11.0)
                        .color(pal.text_muted),
                );
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                    for kw in &item.keywords {
                        egui::Frame::NONE
                            .fill(pal.accent_soft)
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(egui::Margin::symmetric(7, 2))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 3.0;
                                    if ui
                                        .add(
                                            egui::Label::new(
                                                RichText::new(kw).size(11.0).color(pal.text_primary),
                                            )
                                            .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text("Click to find all files with this keyword")
                                        .clicked()
                                    {
                                        click_kw_filter = Some(kw.clone());
                                    }
                                    if ui
                                        .small_button(RichText::new("×").size(10.0).color(pal.text_muted))
                                        .on_hover_text("Remove keyword")
                                        .clicked()
                                    {
                                        remove_kw = Some(kw.clone());
                                    }
                                });
                            });
                    }
                });
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut app.new_keyword_input)
                        .hint_text("Add keyword(s)...")
                        .desired_width(ui.available_width() - 48.0),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("+").clicked() || enter) && !app.new_keyword_input.trim().is_empty() {
                    add_kw = Some(app.new_keyword_input.clone());
                    app.new_keyword_input.clear();
                }
            });
            if let Some((ok, ref msg)) = app.keyword_sync_status {
                ui.label(
                    RichText::new(msg)
                        .size(10.0)
                        .color(if ok { pal.accent } else { Color32::from_rgb(220, 120, 60) }),
                );
            }

            // Quick add to gallery
            if !app.all_galleries.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Gallery:").size(11.0).color(pal.text_muted));
                    egui::ComboBox::from_id_salt("add_to_gallery_combo")
                        .selected_text("Add to gallery...")
                        .show_ui(ui, |ui| {
                            for (g, _) in &app.all_galleries {
                                if ui.selectable_label(false, g).clicked() {
                                    add_to_gallery = Some(g.clone());
                                }
                            }
                        });
                });
            }

            ui.add_space(6.0);
            ui.separator();

            // Core File Properties (matching Thumbs10.exe 0xaba360 template)
            ui.label(
                RichText::new("FILE PROPERTIES")
                    .size(10.5)
                    .strong()
                    .color(pal.text_muted),
            );
            egui::Grid::new("file_props_grid")
                .num_columns(2)
                .spacing([8.0, 3.0])
                .show(ui, |ui| {
                    prop_row(
                        ui,
                        "Dimensions",
                        &format_dimensions_with_mp(item.width, item.height),
                        pal,
                    );
                    prop_row(ui, "File Size", &format_bytes(item.file_length), pal);
                    prop_row(
                        ui,
                        "Color Depth",
                        &if item.color_depth > 0 {
                            format!("{}-bit", item.color_depth)
                        } else {
                            "—".into()
                        },
                        pal,
                    );
                    prop_row(
                        ui,
                        "Bits Per Channel",
                        item.metadata
                            .get(crate::image_meta::KEY_BITS_PER_CHANNEL)
                            .map(|s| s.as_str())
                            .unwrap_or("—"),
                        pal,
                    );
                    prop_row(
                        ui,
                        "Subsampling",
                        item.metadata
                            .get(crate::image_meta::KEY_SUBSAMPLING)
                            .map(|s| s.as_str())
                            .unwrap_or("—"),
                        pal,
                    );
                    prop_row(
                        ui,
                        "Estimated Quality",
                        item.metadata
                            .get(crate::image_meta::KEY_ESTIMATED_QUALITY)
                            .map(|s| s.as_str())
                            .unwrap_or("—"),
                        pal,
                    );
                    prop_row(
                        ui,
                        "DPI",
                        item.metadata
                            .get(crate::image_meta::KEY_DPI)
                            .map(|s| s.as_str())
                            .unwrap_or("—"),
                        pal,
                    );
                    prop_row(
                        ui,
                        "Pages",
                        &if item.page_count > 0 {
                            item.page_count.to_string()
                        } else {
                            "—".into()
                        },
                        pal,
                    );
                    prop_row(ui, "Modified", &item.file_time_iso, pal);
                    if !item.md5.is_empty() {
                        let short_md5: String = item.md5.chars().take(16).collect();
                        prop_row(ui, "MD5 Hash", &format!("{short_md5}…"), pal);
                    }
                });

            let extra_meta: Vec<(&String, &String)> = item
                .metadata
                .iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        crate::image_meta::KEY_SUBSAMPLING
                            | crate::image_meta::KEY_ESTIMATED_QUALITY
                            | crate::image_meta::KEY_BITS_PER_CHANNEL
                            | crate::image_meta::KEY_DPI
                    )
                })
                .collect();

            if !extra_meta.is_empty() {
                ui.add_space(6.0);
                ui.separator();
                ui.label(
                    RichText::new("METADATA (CDR / XMP / EXIF / GS)")
                        .size(10.5)
                        .strong()
                        .color(pal.text_muted),
                );
                egui::Grid::new("extra_meta_grid")
                    .num_columns(2)
                    .spacing([8.0, 3.0])
                    .show(ui, |ui| {
                        for (k, v) in extra_meta {
                            prop_row(ui, k, v, pal);
                        }
                    });
            }

            ui.add_space(6.0);
            ui.separator();
            ui.label(
                RichText::new("ANNOTATION")
                    .size(10.5)
                    .strong()
                    .color(pal.text_muted),
            );
            ui.add(
                egui::TextEdit::multiline(&mut app.annotation_draft)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text("Notes for this catalog item..."),
            );
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("Save Note").clicked() {
                    save_annotation = true;
                }
                if ui.small_button("Open Externally").clicked() {
                    open_external = true;
                }
                if ui.small_button("Show Similar").clicked() {
                    find_similar = true;
                }
                if ui.small_button("Find Duplicates").clicked() {
                    find_dups = true;
                }
            });
        });

    if let Some(r) = set_rating {
        app.set_selected_rating(r);
    }
    if let Some(raw) = add_kw {
        let batch: Vec<String> = raw
            .split([',', ';'])
            .map(|part| part.trim().to_string())
            .filter(|clean| !clean.is_empty())
            .collect();
        if !batch.is_empty() {
            app.add_keywords_to_selected(&batch);
        }
    }
    if let Some(kw) = remove_kw {
        app.remove_keyword_from_selected(&kw);
    }
    if let Some(kw) = click_kw_filter {
        app.open_keyword_view(kw);
    }
    if let Some(g) = add_to_gallery {
        app.add_selected_to_gallery(&g);
    }
    if save_annotation {
        app.save_selected_annotation();
    }
    if open_external {
        app.open_path_externally(&item.full_path);
    }
    if find_similar {
        app.open_similar_dialog(item.full_path.clone());
    }
    if find_dups {
        app.open_duplicates_view();
    }
}

fn format_dimensions_with_mp(width: u32, height: u32) -> String {
    if width == 0 || height == 0 {
        return "—".to_string();
    }
    let mp = (width as f64 * height as f64) / 1_000_000.0;
    if mp >= 100.0 {
        format!("{width} × {height} px ({mp:.1} MP)")
    } else if mp >= 0.01 {
        format!("{width} × {height} px ({mp:.2} MP)")
    } else {
        format!("{width} × {height} px ({mp:.3} MP)")
    }
}

fn prop_row(ui: &mut Ui, key: &str, val: &str, pal: &ThemePalette) {
    ui.label(RichText::new(key).size(11.0).color(pal.text_muted));
    ui.label(RichText::new(val).size(11.0).color(pal.text_primary));
    ui.end_row();
}
