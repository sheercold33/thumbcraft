use crate::config::{
    AppTheme, LayoutPreset, SearchScope, ThumbFormat, GS_DPI_PRESETS, GS_MAX_DPI, GS_MIN_DPI,
};
use crate::decoder::ImageFilter;
use crate::ghostscript;
use crate::ui::theme::ThemePalette;
use crate::ui::thumb_pane::format_bytes;
use crate::ui::{PrefsTab, ThumbCraftApp};
use crate::worker::TaskState;
use egui::{Color32, ProgressBar, RichText, Ui, Vec2};

pub fn show_preferences_dialog(ctx: &egui::Context, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    if !app.show_preferences {
        return;
    }

    let mut open = app.show_preferences;
    let mut trigger_rescan = false;
    let mut trigger_compact = false;
    let mut trigger_choose_db = false;
    let mut trigger_open_db_explorer = false;
    let mut trigger_reset_db = false;

    egui::Window::new("Preferences & UI Customization")
        .open(&mut open)
        .resizable(true)
        .default_width(580.0)
        .default_height(500.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut app.prefs_tab,
                    PrefsTab::Ghostscript,
                    "Ghostscript & CDR",
                );
                ui.selectable_value(&mut app.prefs_tab, PrefsTab::Layout, "UI Layout & Theme");
                ui.selectable_value(
                    &mut app.prefs_tab,
                    PrefsTab::Thumbnails,
                    "Thumbnails & Captions",
                );
                ui.selectable_value(&mut app.prefs_tab, PrefsTab::Tasks, "Tasks");
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .id_salt("prefs_body_scroll")
                .max_height(400.0)
                .show(ui, |ui| match app.prefs_tab {
                    PrefsTab::Ghostscript => {
                        show_gs_prefs(ui, app, pal, &mut trigger_rescan);
                    }
                    PrefsTab::Layout => {
                        show_layout_prefs(ui, app);
                    }
                    PrefsTab::Thumbnails => {
                        show_thumb_prefs(
                            ui,
                            app,
                            pal,
                            &mut trigger_rescan,
                            &mut trigger_compact,
                            &mut trigger_choose_db,
                            &mut trigger_open_db_explorer,
                            &mut trigger_reset_db,
                        );
                    }
                    PrefsTab::Tasks => {
                        show_task_prefs(ui, app, pal);
                    }
                });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Save & Apply").clicked() {
                    app.settings.ghostscript.thumbnail_dpi =
                        ghostscript::clamp_dpi(app.settings.ghostscript.thumbnail_dpi);
                    app.settings.ghostscript.preview_dpi =
                        ghostscript::clamp_dpi(app.settings.ghostscript.preview_dpi);
                    app.settings.ghostscript.full_view_dpi =
                        ghostscript::clamp_dpi(app.settings.ghostscript.full_view_dpi);
                    app.settings.thumb_quality = app.settings.thumb_quality.clamp(25, 100);
                    app.settings.thumb_cache_dim = app.settings.thumb_cache_dim.clamp(64, 320);
                    app.gs_version = ghostscript::detect_version(&app.settings.ghostscript.gs_path);
                    app.settings.save();
                    trigger_rescan = true;
                }
                if ui.button("Reset Defaults").clicked() {
                    app.settings = crate::config::AppSettings::default();
                    app.gs_version = ghostscript::detect_version(&app.settings.ghostscript.gs_path);
                    app.settings.save();
                }
            });
        });

    app.show_preferences = open;
    if trigger_choose_db {
        if let Some(new_path) = rfd::FileDialog::new()
            .add_filter("ThumbCraft Database", &["tcdb"])
            .set_file_name("thumbcraft.tcdb")
            .save_file()
        {
            app.switch_database_path(new_path);
        }
    }
    if trigger_open_db_explorer {
        app.open_db_in_explorer();
    }
    if trigger_reset_db {
        let def = app.default_db_path();
        app.switch_database_path(def);
    }
    if trigger_compact {
        app.compact_database();
    }
    if trigger_rescan {
        app.reload_current_source(true);
    }
}

fn show_gs_prefs(
    ui: &mut Ui,
    app: &mut ThumbCraftApp,
    pal: &ThemePalette,
    trigger_rescan: &mut bool,
) {
    ui.label(
        RichText::new("Ghostscript Integration (PDF, AI, EPS, PS, PRN & CDR Vector Pipeline)")
            .strong(),
    );
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("Ghostscript CLI:");
        ui.text_edit_singleline(&mut app.settings.ghostscript.gs_path);
        if ui.button("Browse...").clicked() {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Executable", &["exe"])
                .pick_file()
            {
                app.settings.ghostscript.gs_path = p.to_string_lossy().to_string();
                app.gs_version = ghostscript::detect_version(&app.settings.ghostscript.gs_path);
            }
        }
        if ui.button("Auto-Detect").clicked() {
            app.settings.ghostscript.gs_path =
                ghostscript::detect_ghostscript().unwrap_or_default();
            app.gs_version = ghostscript::detect_version(&app.settings.ghostscript.gs_path);
        }
    });

    if let Some(ref ver) = app.gs_version {
        let rev_note = if ghostscript::meets_min_revision(ver) {
            "compatible, rev >= 800"
        } else {
            "warning: rev < 800"
        };
        ui.label(
            RichText::new(format!("Detected: GPL Ghostscript {ver} ({rev_note})"))
                .color(pal.badge_cdr),
        );
    } else {
        ui.label(
            RichText::new("Ghostscript executable not found. Select gswin64c.exe or gswin32c.exe.")
                .color(Color32::from_rgb(248, 113, 113)),
        );
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Optional CDR→PDF Converter:");
        ui.text_edit_singleline(&mut app.settings.ghostscript.cdr_converter_path);
        if ui.button("Browse...").clicked() {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Executable", &["exe"])
                .pick_file()
            {
                app.settings.ghostscript.cdr_converter_path = p.to_string_lossy().to_string();
            }
        }
    });
    ui.label(
        RichText::new("Note: Native CDR preview & CDR keyword extraction (ZIP X4+ & RIFF v3–v13) work directly without an external converter. Setting Inkscape or LibreOffice enables full vector CDR→PDF→Ghostscript re-rendering.")
            .size(10.5)
            .color(pal.text_muted),
    );

    ui.add_space(8.0);
    egui::Grid::new("gs_dpi_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Thumbnail DPI (ThumbDPI, 50–1440):");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut app.settings.ghostscript.thumbnail_dpi)
                        .range(GS_MIN_DPI..=GS_MAX_DPI),
                );
                for &p in GS_DPI_PRESETS {
                    if ui.small_button(p.to_string()).clicked() {
                        app.settings.ghostscript.thumbnail_dpi = p;
                    }
                }
            });
            ui.end_row();

            ui.label("Preview Pane DPI (ViewDPI, 50–1440):");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut app.settings.ghostscript.preview_dpi)
                        .range(GS_MIN_DPI..=GS_MAX_DPI),
                );
                for &p in GS_DPI_PRESETS {
                    if ui.small_button(p.to_string()).clicked() {
                        app.settings.ghostscript.preview_dpi = p;
                    }
                }
            });
            ui.end_row();

            ui.label("Full-View DPI (PrintDPI, 50–1440):");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut app.settings.ghostscript.full_view_dpi)
                        .range(GS_MIN_DPI..=GS_MAX_DPI),
                );
                for &p in GS_DPI_PRESETS {
                    if ui.small_button(p.to_string()).clicked() {
                        app.settings.ghostscript.full_view_dpi = p;
                    }
                }
            });
            ui.end_row();

            ui.label("Anti-Aliasing (Alpha Bits):");
            ui.horizontal(|ui| {
                ui.radio_value(&mut app.settings.ghostscript.antialias_bits, 1, "1 (None)");
                ui.radio_value(&mut app.settings.ghostscript.antialias_bits, 2, "2 (Fast)");
                ui.radio_value(&mut app.settings.ghostscript.antialias_bits, 4, "4 (Best)");
            });
            ui.end_row();

            ui.label("Color Mode:");
            ui.horizontal(|ui| {
                ui.radio_value(&mut app.settings.ghostscript.color_depth, 1, "1-bit B/W");
                ui.radio_value(&mut app.settings.ghostscript.color_depth, 8, "8-bit Gray");
                ui.radio_value(&mut app.settings.ghostscript.color_depth, 24, "24-bit Color");
            });
            ui.end_row();
        });

    ui.checkbox(
        &mut app.settings.ghostscript.eps_crop,
        "Crop EPS to BoundingBox (-dEPSCrop)",
    );
    ui.checkbox(
        &mut app.settings.ghostscript.safer_mode,
        "Run Ghostscript in sandboxed mode (-dSAFER)",
    );
    ui.checkbox(
        &mut app.settings.auto_extract_cdr_keywords,
        "Automatically extract CDR keywords (ZIP metadata.xml + RIFF IKEY/INFO + XMP)",
    );
    if ui
        .checkbox(
            &mut app.settings.write_keywords_to_files,
            "Write keywords into files for Windows Explorer compatibility (DB + File)",
        )
        .on_hover_text(
            "When adding or removing keywords, writes tags to both the catalog database and internally inside the file (Windows Property Store / XMP / EXIF / CDR metadata).",
        )
        .changed()
    {
        app.settings.save();
    }

    if ui.button("Re-render Current Folder with Ghostscript").clicked() {
        *trigger_rescan = true;
    }
}

fn show_layout_prefs(ui: &mut Ui, app: &mut ThumbCraftApp) {
    ui.label(RichText::new("Workspace Layout Presets").strong());
    ui.horizontal_wrapped(|ui| {
        for preset in LayoutPreset::all() {
            if ui.button(preset.label()).clicked() {
                app.settings.apply_preset(*preset);
                app.settings.save();
            }
        }
    });

    ui.add_space(8.0);
    ui.label(RichText::new("Color Theme / Skin").strong());
    ui.horizontal_wrapped(|ui| {
        for t in AppTheme::all() {
            if ui
                .selectable_value(&mut app.settings.theme, *t, t.label())
                .clicked()
            {
                app.settings.save();
            }
        }
    });

    ui.add_space(8.0);
    ui.label(RichText::new("Dockable Panes & Chrome").strong());
    ui.checkbox(
        &mut app.settings.layout.show_tree_pane,
        "Show Tree Pane (Folders / Keywords / Galleries)",
    );
    ui.checkbox(&mut app.settings.layout.show_preview_pane, "Show Live Preview Pane");
    ui.checkbox(
        &mut app.settings.layout.show_info_pane,
        "Show File Info & CDR Keywords Pane",
    );
    ui.checkbox(
        &mut app.settings.layout.show_tasks_pane,
        "Show Tasks & Ghostscript Log Pane",
    );
    ui.checkbox(&mut app.settings.layout.show_toolbar, "Show Main Toolbar");
    ui.checkbox(&mut app.settings.layout.show_status_bar, "Show Status Bar");
    ui.checkbox(
        &mut app.settings.layout.preview_on_right,
        "Dock Preview Pane on Right Side (instead of Left Bottom)",
    );
    ui.checkbox(
        &mut app.settings.layout.checkerboard_alpha,
        "Show Transparency Checkerboard",
    );

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Card Spacing:");
        ui.add(egui::Slider::new(
            &mut app.settings.layout.card_spacing,
            4.0..=20.0,
        ));
    });
    ui.horizontal(|ui| {
        ui.label("Card Corner Radius:");
        ui.add(egui::Slider::new(
            &mut app.settings.layout.card_rounding,
            0.0..=14.0,
        ));
    });
}

/// Thumbnail compression & database location preferences modeled after reference/picture-10.png
#[allow(clippy::too_many_arguments)]
fn show_thumb_prefs(
    ui: &mut Ui,
    app: &mut ThumbCraftApp,
    pal: &ThemePalette,
    trigger_rescan: &mut bool,
    trigger_compact: &mut bool,
    trigger_choose_db: &mut bool,
    trigger_open_db_explorer: &mut bool,
    trigger_reset_db: &mut bool,
) {
    ui.label(RichText::new("Thumbnail Database Location (Single-File .tcdb)").strong());
    ui.add_space(3.0);

    let mut path_display = app.db_path.to_string_lossy().to_string();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut path_display)
                .interactive(false)
                .desired_width(320.0),
        );
        if ui
            .button("Open in Explorer")
            .on_hover_text("Open this database location in Windows Explorer")
            .clicked()
        {
            *trigger_open_db_explorer = true;
        }
    });
    ui.horizontal(|ui| {
        if ui
            .button("Change Database Location...")
            .on_hover_text("Choose where to save or open thumbcraft.tcdb (migrates existing data)")
            .clicked()
        {
            *trigger_choose_db = true;
        }
        if app.settings.custom_db_path.is_some() && ui.button("Use Default Folder").clicked() {
            *trigger_reset_db = true;
        }
        if ui
            .button("Compact Database")
            .on_hover_text("Remove dangling thumbnails & unused keywords and reclaim dead space in thumbcraft.tcdb")
            .clicked()
        {
            *trigger_compact = true;
        }
    });

    ui.label(
        RichText::new(format!(
            "Size: {} — {} thumbnail(s), {} folder(s), {} keyword(s).",
            format_bytes(app.catalog_stats.db_size_bytes),
            app.catalog_stats.total_thumbnails,
            app.catalog_stats.total_folders,
            app.catalog_stats.total_keywords,
        ))
        .size(10.5)
        .color(pal.text_muted),
    );

    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new("Thumbnail Compression Format & Quality").strong());
    ui.add_space(4.0);

    egui::Grid::new("thumb_storage_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Compression Format:");
            ui.horizontal(|ui| {
                for fmt in ThumbFormat::all() {
                    if ui
                        .selectable_value(&mut app.settings.thumb_format, *fmt, fmt.short_name())
                        .on_hover_text(fmt.label())
                        .clicked()
                    {
                        app.settings.save();
                    }
                }
            });
            ui.end_row();

            ui.label("Quality (25–100):");
            ui.horizontal(|ui| {
                if app.settings.thumb_format.supports_quality() {
                    if ui
                        .add(egui::Slider::new(&mut app.settings.thumb_quality, 25..=100))
                        .changed()
                    {
                        app.settings.save();
                    }
                    ui.label(
                        RichText::new(if app.settings.thumb_quality == 100 {
                            "Lossless"
                        } else if app.settings.thumb_quality >= 80 {
                            "High (Default 80)"
                        } else {
                            "Compact"
                        })
                        .size(10.5)
                        .color(pal.text_muted),
                    );
                } else {
                    ui.label(
                        RichText::new("PNG is always lossless")
                            .size(11.0)
                            .color(pal.text_muted),
                    );
                }
            });
            ui.end_row();

            ui.label("Stored Thumbnail Max Size:");
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Slider::new(&mut app.settings.thumb_cache_dim, 64..=320)
                            .suffix(" px"),
                    )
                    .changed()
                {
                    app.settings.save();
                }
            });
            ui.end_row();
        });

    if ui
        .button(format!(
            "Rebuild Current View as {} Q{}",
            app.settings.thumb_format.short_name(),
            app.settings.thumb_quality
        ))
        .clicked()
    {
        app.settings.save();
        *trigger_rescan = true;
    }

    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new("Thumbnail Display & Caption Fields").strong());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Display Thumbnail Size (px):");
        if ui
            .add(egui::Slider::new(&mut app.settings.thumb_size, 64.0..=320.0))
            .changed()
        {
            app.settings.save();
        }
    });
    ui.add_space(4.0);
    ui.checkbox(&mut app.settings.captions.show_name, "Show File Name");
    ui.checkbox(&mut app.settings.captions.show_size, "Show File Size");
    ui.checkbox(
        &mut app.settings.captions.show_dimensions,
        "Show Image Dimensions",
    );
    ui.checkbox(
        &mut app.settings.captions.show_format_badge,
        "Show Format Badge (CDR, PDF, AVIF, JXL...)",
    );
    ui.checkbox(&mut app.settings.captions.show_rating, "Show Star Rating");
    ui.checkbox(
        &mut app.settings.captions.show_keywords,
        "Show Extracted Keywords Preview",
    );
    ui.add_space(4.0);
    ui.checkbox(
        &mut app.settings.include_subfolders,
        "Include Subfolders When Browsing (unlimited file count)",
    );
}

fn show_task_prefs(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    ui.label(RichText::new("Background Task & Search Preferences").strong());
    ui.add_space(6.0);

    if ui
        .checkbox(
            &mut app.settings.auto_scan,
            "Automatically scan & generate thumbnails when browsing folders (Auto-Scan)",
        )
        .on_hover_text(
            "When unchecked, browsing folders still displays already-cached thumbnails from the database without scanning uncached files until you manually scan or press F3.",
        )
        .changed()
    {
        app.settings.save();
    }

    if ui
        .checkbox(
            &mut app.settings.auto_clear_completed_tasks,
            "Automatically clear completed tasks from the Tasks list",
        )
        .changed()
    {
        app.settings.save();
    }

    ui.add_space(8.0);
    ui.label(RichText::new("Default Scope for Similar & Duplicate Searches:").strong());
    ui.horizontal(|ui| {
        for scope in SearchScope::all() {
            if ui
                .radio_value(&mut app.settings.search_scope, *scope, scope.label())
                .clicked()
            {
                app.settings.save();
            }
        }
    });

    ui.add_space(10.0);
    ui.separator();
    ui.label(RichText::new("Performance (Hybrid GPU + Multi-Core CPU)").strong());
    ui.add_space(4.0);
    if ui
        .checkbox(
            &mut app.settings.use_gpu_acceleration,
            "Use GPU compute acceleration for large image downscaling (>= 0.5 MP)",
        )
        .on_hover_text(
            "Offloads downscaling of large decoded rasters (PNG, JPG, PSD/PSB, TIFF, WEBP) to a WGSL GPU compute shader while decoding smaller images and CDR previews in parallel across CPU cores.",
        )
        .changed()
    {
        app.settings.save();
    }
    let gpu_status = match &app.gpu_info {
        Some(g) => format!("Detected GPU: {} ({})", g.name, g.backend),
        None => "Detected GPU: None (using fast multi-core CPU box downscaler)".to_string(),
    };
    ui.label(RichText::new(gpu_status).size(10.5).color(pal.text_muted));
    ui.add_space(4.0);
    let max_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);
    ui.horizontal(|ui| {
        ui.label("CPU Worker Threads (0 = Auto):");
        if ui
            .add(egui::Slider::new(
                &mut app.settings.cpu_worker_threads,
                0..=max_cores,
            ))
            .changed()
        {
            app.settings.save();
        }
        if app.settings.cpu_worker_threads == 0 {
            ui.label(
                RichText::new(format!("Auto ({max_cores} logical cores)"))
                    .size(10.5)
                    .color(pal.text_muted),
            );
        }
    });

    ui.add_space(10.0);
    ui.separator();
    ui.label(RichText::new("Quick Task Controls").strong());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Pause All Tasks").clicked() {
            app.pause_all_tasks();
        }
        if ui.button("Resume All Tasks").clicked() {
            app.resume_all_tasks();
        }
        if ui.button("Quit All Tasks").clicked() {
            app.quit_all_tasks();
        }
    });
    ui.add_space(6.0);
    ui.label(
        RichText::new("Tip: Right-click any running, paused, or queued task in the Tasks pane for Task Properties, Pause, Resume, Quit, and Error inspection.")
            .size(10.5)
            .color(pal.text_muted),
    );
}

/// Find Similar Images dialog matching reference/picture-8.png
pub fn show_find_similar_dialog(ctx: &egui::Context, app: &mut ThumbCraftApp) {
    let Some(target_path) = app.similar_dialog_target.clone() else {
        return;
    };

    let mut open = true;
    let mut run_search = false;
    let mut cancel = false;

    egui::Window::new("Find Similar Images")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(410.0)
        .show(ctx, |ui| {
            ui.label(
                RichText::new(format!(
                    "Target: {}",
                    target_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                ))
                .strong(),
            );
            ui.add_space(6.0);

            ui.columns(2, |cols| {
                // Left column: Threshold & Color weight (matching picture-8.png)
                cols[0].group(|ui| {
                    ui.label(RichText::new("Threshold").strong());
                    ui.add_space(4.0);
                    let mut pct = (app.similarity_threshold * 100.0).round() as i32;
                    if ui.button("Identical Metrics (100%)").clicked() {
                        pct = 98;
                    }
                    ui.add_space(2.0);
                    ui.add(egui::Slider::new(&mut pct, 30..=100).suffix("%"));
                    app.similarity_threshold = pct as f32 / 100.0;
                    if ui.button("Less similar (50%)").clicked() {
                        app.similarity_threshold = 0.50;
                    }
                    ui.add_space(6.0);
                    ui.label("Color similarity weight:");
                    let mut cw = (app.similarity_color_weight * 100.0).round() as i32;
                    ui.add(egui::Slider::new(&mut cw, 0..=100).suffix("%"));
                    app.similarity_color_weight = cw as f32 / 100.0;
                });

                // Right column: Restrict search to & Include file types (matching picture-8.png)
                cols[1].group(|ui| {
                    ui.label(RichText::new("Restrict search to:").strong());
                    for scope in [
                        SearchScope::CurrentFolder,
                        SearchScope::CurrentTree,
                        SearchScope::EntireDatabase,
                    ] {
                        if ui
                            .radio_value(&mut app.settings.search_scope, scope, scope.label())
                            .clicked()
                        {
                            app.settings.save();
                        }
                    }
                });
                cols[1].add_space(6.0);
                cols[1].group(|ui| {
                    ui.label(RichText::new("Include file types:").strong());
                    ui.radio_value(&mut app.similar_only_same_type, false, "All file types");
                    ui.radio_value(
                        &mut app.similar_only_same_type,
                        true,
                        "Selected type only",
                    );
                });
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("   OK   ").clicked() {
                    run_search = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

    if run_search {
        app.similar_dialog_target = None;
        app.open_similar_view(target_path);
    } else if cancel || !open {
        app.similar_dialog_target = None;
    }
}

/// Task Properties dialog matching reference/picture-12.png
pub fn show_task_properties_dialog(ctx: &egui::Context, app: &mut ThumbCraftApp) {
    let Some(task_id) = app.task_props_id else {
        return;
    };
    let Some(task) = app.managed_tasks.iter().find(|t| t.id == task_id).cloned() else {
        app.task_props_id = None;
        return;
    };

    let mut open = true;
    let mut do_pause = false;
    let mut do_resume = false;
    let mut do_kill = false;
    let mut do_close = false;

    let max_win_w = (ctx.screen_rect().width() - 40.0).max(560.0);

    egui::Window::new("Task Properties")
        .id(egui::Id::new("thumbcraft_task_properties_dialog"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(Vec2::new(580.0, 450.0))
        .min_width(500.0)
        .min_height(350.0)
        .max_width(max_win_w)
        .show(ctx, |ui| {
            let content_w = ui.available_width().max(480.0);
            let path_field_w = (content_w - 92.0).max(260.0);

            egui::Grid::new("task_props_header_grid")
                .num_columns(4)
                .spacing([8.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Task name:");
                    ui.add(
                        egui::TextEdit::singleline(&mut task.task_name.clone())
                            .interactive(false)
                            .desired_width(180.0),
                    );
                    ui.label("");
                    ui.label("");
                    ui.end_row();

                    ui.label("Started at:");
                    ui.add(
                        egui::TextEdit::singleline(&mut task.started_at.clone())
                            .interactive(false)
                            .desired_width(180.0),
                    );
                    ui.label("Elapsed:");
                    ui.add(
                        egui::TextEdit::singleline(&mut format_duration_ms(task.elapsed_ms))
                            .interactive(false)
                            .desired_width(110.0),
                    );
                    ui.end_row();

                    ui.label("Completed at:");
                    ui.add(
                        egui::TextEdit::singleline(&mut task.completed_at.clone())
                            .interactive(false)
                            .desired_width(180.0),
                    );
                    ui.label("CPU time:");
                    ui.add(
                        egui::TextEdit::singleline(&mut format_duration_ms(task.active_ms))
                            .interactive(false)
                            .desired_width(110.0),
                    );
                    ui.end_row();
                });

            ui.add_space(4.0);
            egui::Grid::new("task_props_paths_grid")
                .num_columns(2)
                .spacing([8.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Task path:");
                    ui.add(
                        egui::TextEdit::singleline(&mut task.root_dir.to_string_lossy().to_string())
                            .interactive(false)
                            .desired_width(path_field_w),
                    );
                    ui.end_row();

                    ui.label("Current file:");
                    ui.add(
                        egui::TextEdit::singleline(&mut task.current_file.clone())
                            .interactive(false)
                            .desired_width(path_field_w),
                    );
                    ui.end_row();

                    ui.label("Display text:");
                    ui.add(
                        egui::TextEdit::singleline(&mut task.display_text.clone())
                            .interactive(false)
                            .desired_width(path_field_w),
                    );
                    ui.end_row();
                });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Messages: ({} / {} error{})",
                    task.messages.len(),
                    task.errors.len(),
                    if task.errors.len() == 1 { "" } else { "s" }
                ));
                if !task.errors.is_empty()
                    && ui
                        .small_button("Copy Errors")
                        .on_hover_text("Copy all error lines to clipboard")
                        .clicked()
                {
                    let all_errs = task
                        .errors
                        .iter()
                        .map(|e| format!("[Error] {e}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.ctx().copy_text(all_errs);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut task.state.label().to_string())
                            .interactive(false)
                            .desired_width(90.0),
                    );
                    ui.label("Current state:");
                    ui.add(
                        egui::TextEdit::singleline(&mut task.thread_id.to_string())
                            .interactive(false)
                            .desired_width(72.0),
                    );
                    ui.label("Thread ID:");
                });
            });

            ui.add_space(4.0);
            let btn_col_w = 92.0;
            let list_w = (content_w - btn_col_w - 10.0).max(320.0);
            let list_h = (ui.available_height() - 68.0).clamp(140.0, 460.0);

            ui.horizontal(|ui| {
                // Strictly bounded scrollable list box so long error paths never stretch the dialog
                ui.allocate_ui_with_layout(
                    Vec2::new(list_w, list_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_clip_rect(ui.max_rect());
                        egui::Frame::group(ui.style())
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                let inner_w = (list_w - 14.0).max(200.0);
                                let inner_h = (list_h - 14.0).max(100.0);
                                let total_rows = task.errors.len() + task.messages.len();
                                ui.spacing_mut().item_spacing.y = 3.0;
                                let row_h = 15.0;

                                egui::ScrollArea::both()
                                    .id_salt("task_props_msgs_scroll")
                                    .max_width(inner_w)
                                    .max_height(inner_h)
                                    .auto_shrink([false, false])
                                    .show_rows(ui, row_h, total_rows, |ui, row_range| {
                                        if total_rows == 0 {
                                            ui.label(
                                                RichText::new("No messages or errors.")
                                                    .size(11.0)
                                                    .weak(),
                                            );
                                            return;
                                        }
                                        let err_count = task.errors.len();
                                        for idx in row_range {
                                            if idx < err_count {
                                                let err = &task.errors[idx];
                                                let line = format!("[Error] {err}");
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(&line)
                                                            .size(11.0)
                                                            .color(Color32::from_rgb(200, 40, 40)),
                                                    )
                                                    .extend(),
                                                )
                                                .on_hover_text(&line);
                                            } else {
                                                let msg = &task.messages[idx - err_count];
                                                ui.add(
                                                    egui::Label::new(RichText::new(msg).size(11.0))
                                                        .extend(),
                                                )
                                                .on_hover_text(msg);
                                            }
                                        }
                                    });
                            });
                    },
                );

                // Right-side Pause / Resume / Kill buttons matching picture-12.png
                ui.vertical(|ui| {
                    ui.spacing_mut().button_padding = Vec2::new(14.0, 5.0);
                    if ui
                        .add_enabled(
                            matches!(task.state, TaskState::Running | TaskState::Queued),
                            egui::Button::new("Pause").min_size(Vec2::new(82.0, 26.0)),
                        )
                        .clicked()
                    {
                        do_pause = true;
                    }
                    ui.add_space(4.0);
                    if ui
                        .add_enabled(
                            task.state == TaskState::Paused,
                            egui::Button::new("Resume").min_size(Vec2::new(82.0, 26.0)),
                        )
                        .clicked()
                    {
                        do_resume = true;
                    }
                    ui.add_space(4.0);
                    if ui
                        .add_enabled(
                            task.state.is_active(),
                            egui::Button::new("Kill").min_size(Vec2::new(82.0, 26.0)),
                        )
                        .clicked()
                    {
                        do_kill = true;
                    }
                });
            });

            ui.add_space(8.0);
            let frac = if task.total > 0 {
                (task.done as f32 / task.total as f32).clamp(0.0, 1.0)
            } else if task.state == TaskState::Completed {
                1.0
            } else {
                0.0
            };
            let bar_w = (content_w - 60.0).max(240.0);
            ui.horizontal(|ui| {
                ui.add(
                    ProgressBar::new(frac)
                        .desired_width(bar_w)
                        .desired_height(18.0),
                );
                ui.label(RichText::new(format!("{:.1}%", frac * 100.0)).strong());
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let to_do = task.total.saturating_sub(task.done);
                ui.label(format!(
                    "Done: {}   To do: {}   Seen: {}   (Indexed: {}, Up-to-date: {})",
                    task.done, to_do, task.seen, task.indexed, task.skipped
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("Close").min_size(Vec2::new(78.0, 24.0)))
                        .clicked()
                    {
                        do_close = true;
                    }
                });
            });
        });

    if do_pause {
        app.pause_task(task_id);
    }
    if do_resume {
        app.resume_task(task_id);
    }
    if do_kill {
        app.quit_task(task_id);
    }
    if do_close || !open {
        app.task_props_id = None;
    }
}

fn format_duration_ms(ms: u64) -> String {
    let total_secs = ms / 1000;
    let rem_ms = ms % 1000;
    let hrs = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    format!("{hrs}:{mins:02}:{secs:02}.{rem_ms:03}")
}

pub fn show_lightbox_modal(ctx: &egui::Context, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    if !app.show_lightbox {
        return;
    }

    let Some(item) = app.selected_item().cloned() else {
        app.show_lightbox = false;
        return;
    };

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.show_lightbox = false;
        return;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
        app.step_selection(1);
    } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
        app.step_selection(-1);
    }

    let mut open = app.show_lightbox;
    egui::Window::new(format!("Viewer — {} ({})", item.file_name, item.filetype))
        .id(egui::Id::new("thumbcraft_lightbox_viewer"))
        .open(&mut open)
        .resizable(true)
        .default_size(Vec2::new(980.0, 680.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("◀ Prev").clicked() {
                    app.step_selection(-1);
                }
                if ui.button("Next ▶").clicked() {
                    app.step_selection(1);
                }
                ui.separator();
                if ui.button("Fit").clicked() {
                    app.lightbox_zoom = 0.0;
                }
                if ui.button("100%").clicked() {
                    app.lightbox_zoom = 1.0;
                }
                if ui.button("Zoom +").clicked() {
                    app.lightbox_zoom = if app.lightbox_zoom <= 0.01 {
                        1.25
                    } else {
                        (app.lightbox_zoom * 1.25).min(8.0)
                    };
                }
                if ui.button("Zoom -").clicked() {
                    app.lightbox_zoom = if app.lightbox_zoom <= 0.01 {
                        0.75
                    } else {
                        (app.lightbox_zoom / 1.25).max(0.1)
                    };
                }
                if item.page_count > 1 {
                    ui.separator();
                    let max_p = item.page_count as usize;
                    if ui.button("Page ◀").clicked() && app.preview_page > 1 {
                        app.preview_page -= 1;
                        app.refresh_preview_image(true);
                    }
                    ui.label(format!("Page {}/{}", app.preview_page, max_p));
                    if ui.button("Page ▶").clicked() && app.preview_page < max_p {
                        app.preview_page += 1;
                        app.refresh_preview_image(true);
                    }
                }
                ui.separator();
                let prev_f = app.active_filter;
                egui::ComboBox::from_id_salt("lightbox_filter")
                    .selected_text(app.active_filter.label())
                    .show_ui(ui, |ui| {
                        for f in ImageFilter::all() {
                            ui.selectable_value(&mut app.active_filter, *f, f.label());
                        }
                    });
                if app.active_filter != prev_f {
                    app.rebuild_filtered_preview();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !item.keywords.is_empty() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("Tags: {}", item.keywords.join(", ")))
                                    .size(11.0)
                                    .color(pal.accent),
                            )
                            .truncate(),
                        );
                    }
                });
            });
            ui.separator();

            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let tex_opt = app.preview_texture.as_ref().or(item.texture.as_ref());
                    if let Some(tex) = tex_opt {
                        let ts = tex.size_vec2();
                        let avail = ui.available_size();
                        let scale = if app.lightbox_zoom <= 0.01 {
                            (avail.x / ts.x.max(1.0)).min(avail.y / ts.y.max(1.0)) * 0.96
                        } else {
                            app.lightbox_zoom
                        };
                        ui.centered_and_justified(|ui| {
                            ui.image((tex.id(), ts * scale));
                        });
                    }
                });
        });

    app.show_lightbox = open;
}

pub fn show_file_action_dialogs(ctx: &egui::Context, app: &mut ThumbCraftApp) {
    // Rename File Modal (F2) — auto-selects base filename minus extension
    if let Some(target) = app.rename_target.clone() {
        let mut open = true;
        let mut do_rename = false;
        let mut cancel_rename = false;
        egui::Window::new("Rename File")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Current: {}",
                    target.file_name().and_then(|n| n.to_str()).unwrap_or("")
                ));
                let mut output = egui::TextEdit::singleline(&mut app.rename_input)
                    .desired_width(330.0)
                    .show(ui);
                if app.rename_just_opened {
                    app.rename_just_opened = false;
                    output.response.request_focus();
                    let stem_chars = crate::ui::rename_stem_char_len(&app.rename_input);
                    output.state.cursor.set_char_range(Some(
                        egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(stem_chars),
                        ),
                    ));
                    output.state.store(ctx, output.response.id);
                }
                let resp = output.response;
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    do_rename = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel_rename = true;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked() {
                        do_rename = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel_rename = true;
                    }
                });
            });
        if do_rename {
            app.commit_rename();
        } else if cancel_rename || !open {
            app.rename_target = None;
            app.rename_just_opened = false;
        }
    }

    // Extension Change Confirmation Yes/No Dialog
    if let Some((ref old_path, ref new_path)) = app.rename_ext_confirm.clone() {
        let mut open = true;
        let mut confirm_yes = false;
        let mut confirm_no = false;
        egui::Window::new("Rename")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(430.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "If you change a file name extension, the file may become unusable. Are you sure you want to change it?",
                    )
                    .strong(),
                );
                ui.add_space(4.0);
                let old_name = old_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                let new_name = new_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                ui.label(format!("{old_name}  ->  {new_name}"));
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    confirm_no = true;
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Yes").clicked() {
                        confirm_yes = true;
                    }
                    if ui.button("No").clicked() {
                        confirm_no = true;
                    }
                });
            });
        if confirm_yes {
            app.confirm_rename_with_extension_change();
        } else if confirm_no || !open {
            app.rename_ext_confirm = None;
        }
    }

    // Delete File(s) Confirmation Modal (Del)
    if !app.delete_targets.is_empty() {
        let targets = app.delete_targets.clone();
        let count = targets.len();
        let mut open = true;
        let mut do_delete = false;
        let title = if count == 1 {
            "Confirm Delete"
        } else {
            "Confirm Delete Multiple Files"
        };
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                if count == 1 {
                    ui.label(
                        RichText::new("Permanently delete this file from disk?")
                            .strong(),
                    );
                    ui.label(targets[0].to_string_lossy().to_string());
                } else {
                    ui.label(
                        RichText::new(format!(
                            "Permanently delete {count} selected files from disk?"
                        ))
                        .strong(),
                    );
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt("delete_targets_list")
                        .max_height(150.0)
                        .show(ui, |ui| {
                            for p in &targets {
                                let name = p
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| p.to_string_lossy().to_string());
                                ui.label(format!("• {name}"))
                                    .on_hover_text(p.to_string_lossy().to_string());
                            }
                        });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let btn_label = if count == 1 {
                        "Delete File".to_string()
                    } else {
                        format!("Delete {count} Files")
                    };
                    if ui
                        .button(RichText::new(btn_label).color(Color32::from_rgb(220, 38, 38)))
                        .clicked()
                    {
                        do_delete = true;
                    }
                    if ui.button("Cancel").clicked() {
                        app.delete_targets.clear();
                    }
                });
            });
        if do_delete {
            app.commit_delete();
        } else if !open {
            app.delete_targets.clear();
        }
    }

    // Create Folder Modal (Ctrl+Shift+F2)
    if let Some(parent) = app.folder_create_parent.clone() {
        let mut open = true;
        let mut do_create = false;
        egui::Window::new("Create Folder")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.label(format!("Inside: {}", parent.display()));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.folder_create_input)
                        .hint_text("New folder name...")
                        .desired_width(350.0),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    do_create = true;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        do_create = true;
                    }
                    if ui.button("Cancel").clicked() {
                        app.folder_create_parent = None;
                    }
                });
            });
        if do_create {
            app.commit_create_folder();
        } else if !open {
            app.folder_create_parent = None;
        }
    }

    // Rename Folder Modal (Shift+F5)
    if let Some(target) = app.folder_rename_target.clone() {
        let mut open = true;
        let mut do_rename = false;
        egui::Window::new("Rename Folder")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.label(format!("Folder: {}", target.display()));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.folder_rename_input).desired_width(350.0),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    do_rename = true;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked() {
                        do_rename = true;
                    }
                    if ui.button("Cancel").clicked() {
                        app.folder_rename_target = None;
                    }
                });
            });
        if do_rename {
            app.commit_rename_folder();
        } else if !open {
            app.folder_rename_target = None;
        }
    }

    // Delete Folder Confirmation Modal (Ctrl+Shift+F6)
    if let Some(target) = app.folder_delete_target.clone() {
        let mut open = true;
        let mut do_delete = false;
        egui::Window::new("Confirm Delete Folder")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Permanently delete this folder and all its contents from disk?")
                        .strong(),
                );
                ui.label(target.to_string_lossy().to_string());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            RichText::new("Delete Folder").color(Color32::from_rgb(220, 38, 38)),
                        )
                        .clicked()
                    {
                        do_delete = true;
                    }
                    if ui.button("Cancel").clicked() {
                        app.folder_delete_target = None;
                    }
                });
            });
        if do_delete {
            app.commit_delete_folder();
        } else if !open {
            app.folder_delete_target = None;
        }
    }

    // Folder Properties Modal
    if let Some(props) = app.folder_props_modal.clone() {
        let mut open = true;
        let mut scan_folder = false;
        let mut scan_tree = false;
        let mut open_explorer = false;
        egui::Window::new("Folder Properties")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                egui::Grid::new("folder_props_grid")
                    .num_columns(2)
                    .spacing([12.0, 5.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Path:").strong());
                        ui.label(props.path.to_string_lossy().to_string());
                        ui.end_row();

                        ui.label(RichText::new("Supported files (folder):").strong());
                        ui.label(format!("{}", props.direct_supported));
                        ui.end_row();

                        ui.label(RichText::new("Subfolders:").strong());
                        ui.label(format!("{}", props.direct_subdirs));
                        ui.end_row();

                        ui.label(RichText::new("Cataloged in DB (tree):").strong());
                        ui.label(format!("{} thumbnail(s)", props.cataloged_in_tree));
                        ui.end_row();

                        ui.label(RichText::new("Direct folder size:").strong());
                        ui.label(format_bytes(props.direct_bytes));
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Scan Folder").clicked() {
                        scan_folder = true;
                    }
                    if ui.button("Scan Tree").clicked() {
                        scan_tree = true;
                    }
                    if ui.button("Open in Explorer").clicked() {
                        open_explorer = true;
                    }
                    if ui.button("Close").clicked() {
                        app.folder_props_modal = None;
                    }
                });
            });
        if scan_folder {
            app.start_folder_scan(props.path.clone(), false, false);
            app.folder_props_modal = None;
        } else if scan_tree {
            app.start_folder_scan(props.path.clone(), true, false);
            app.folder_props_modal = None;
        } else if open_explorer {
            app.open_folder_in_explorer(&props.path);
        } else if !open {
            app.folder_props_modal = None;
        }
    }
}
