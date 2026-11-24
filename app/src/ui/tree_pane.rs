use crate::ui::theme::ThemePalette;
use crate::ui::{BrowseSource, ThumbCraftApp, TreeTab};
use egui::{Color32, RichText, ScrollArea, Ui};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
enum FolderContextAction {
    CreateFolder(PathBuf),
    DeleteFolder(PathBuf),
    CopyFolder(PathBuf),
    MoveFolder(PathBuf),
    RenameFolder(PathBuf),
    OpenInExplorer(PathBuf),
    ScanFolder(PathBuf),
    ScanTree(PathBuf),
    RemoveFromTree(PathBuf),
    RemoveOrphans(PathBuf),
    RemakeFolder(PathBuf),
    RemakeTree(PathBuf),
    ToggleFavorite(PathBuf),
    IncludeChildFolders(PathBuf),
    RefreshFolder(PathBuf),
    Properties(PathBuf),
}

pub fn show_tree_pane(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    crate::ui::theme::draw_pane_title_bar(ui, "Location", false, pal);
    // Tab selector bar
    egui::Frame::NONE
        .fill(pal.bg_toolbar)
        .inner_margin(egui::Margin::symmetric(4, 3))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                for (tab, label) in [
                    (TreeTab::Folders, "Folders"),
                    (TreeTab::Keywords, "Keywords"),
                    (TreeTab::Galleries, "Galleries"),
                    (TreeTab::Filters, "Filters"),
                ] {
                    let active = app.tree_tab == tab;
                    if ui.selectable_label(active, label).clicked() {
                        app.tree_tab = tab;
                    }
                }
            });
        });
    ui.add_space(2.0);

    match app.tree_tab {
        TreeTab::Folders => show_folders_tab(ui, app, pal),
        TreeTab::Keywords => show_keywords_tab(ui, app, pal),
        TreeTab::Galleries => show_galleries_tab(ui, app, pal),
        TreeTab::Filters => show_filters_tab(ui, app, pal),
    }
}

fn show_folders_tab(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    // Periodically refresh drive roots only every 15 seconds (never hit disk on every frame)
    if app.cached_roots_at.elapsed() >= Duration::from_secs(15) {
        app.cached_roots = list_roots_uncached();
        app.cached_roots_at = Instant::now();
    }

    let mut folder_to_open: Option<PathBuf> = None;
    let mut ctx_action: Option<FolderContextAction> = None;
    let mut drop_hover_dir: Option<PathBuf> = None;
    let is_dragging_files = !app.internal_drag_files.is_empty();
    let favorites = &app.settings.favorite_folders;
    let scanned_folders = &app.scanned_folders;

    // Quick Access Shortcuts
    ui.label(
        RichText::new("QUICK ACCESS")
            .size(10.5)
            .strong()
            .color(pal.text_muted),
    );
    let samples_dir = &app.sample_dir;
    let is_samples = matches!(&app.browse_source, BrowseSource::Folder(p) if p == samples_dir);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let r_samples = ui.selectable_label(is_samples, "★ Built-in Samples (CDR/GS/AVIF)");
        if is_dragging_files && ui.rect_contains_pointer(r_samples.rect) {
            draw_drop_target_highlight(ui, r_samples.rect, pal);
            drop_hover_dir = Some(samples_dir.clone());
        }
        if r_samples.clicked() {
            folder_to_open = Some(samples_dir.clone());
        }
        r_samples.context_menu(|ui| {
            show_folder_context_menu(ui, samples_dir, favorites, &mut ctx_action);
        });
        if is_dir_scanned(scanned_folders, samples_dir) {
            draw_scanned_checkmark(ui, pal);
        }
    });

    for (label, p) in &app.cached_quick_access {
        let selected = matches!(&app.browse_source, BrowseSource::Folder(cur) if cur == p);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let resp = ui.selectable_label(selected, format!("[Dir] {label}"));
            if is_dragging_files && ui.rect_contains_pointer(resp.rect) {
                draw_drop_target_highlight(ui, resp.rect, pal);
                drop_hover_dir = Some(p.clone());
            }
            if resp.clicked() {
                folder_to_open = Some(p.clone());
            }
            resp.context_menu(|ui| {
                show_folder_context_menu(ui, p, favorites, &mut ctx_action);
            });
            if is_dir_scanned(scanned_folders, p) {
                draw_scanned_checkmark(ui, pal);
            }
        });
    }

    if !favorites.is_empty() {
        ui.add_space(4.0);
        ui.label(
            RichText::new("FAVORITE FOLDERS")
                .size(10.5)
                .strong()
                .color(pal.text_muted),
        );
        for fav in favorites {
            let p = PathBuf::from(fav);
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(fav.as_str());
            let selected = matches!(&app.browse_source, BrowseSource::Folder(cur) if cur == &p);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let resp = ui
                    .selectable_label(selected, format!("★ {name}"))
                    .on_hover_text(fav);
                if is_dragging_files && ui.rect_contains_pointer(resp.rect) && p.is_dir() {
                    draw_drop_target_highlight(ui, resp.rect, pal);
                    drop_hover_dir = Some(p.clone());
                }
                if resp.clicked() && p.is_dir() {
                    folder_to_open = Some(p.clone());
                }
                resp.context_menu(|ui| {
                    show_folder_context_menu(ui, &p, favorites, &mut ctx_action);
                });
                if is_dir_scanned(scanned_folders, &p) {
                    draw_scanned_checkmark(ui, pal);
                }
            });
        }
    }

    ui.add_space(6.0);
    ui.separator();
    ui.label(
        RichText::new(if is_dragging_files {
            "DRIVES & FOLDERS — Drop over a folder to move"
        } else {
            "DRIVES & FOLDERS (Right-click for Scan / Tree)"
        })
        .size(10.0)
        .strong()
        .color(if is_dragging_files {
            pal.accent
        } else {
            pal.text_muted
        }),
    );

    let current_folder = match &app.browse_source {
        BrowseSource::Folder(p) => Some(p.as_path()),
        _ => None,
    };

    let expanded_dirs = &mut app.expanded_dirs;
    let dir_children_cache = &mut app.dir_children_cache;
    let cached_roots = &app.cached_roots;

    ScrollArea::vertical()
        .id_salt("folder_tree_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for root in cached_roots {
                draw_dir_node(
                    ui,
                    root,
                    current_folder,
                    expanded_dirs,
                    dir_children_cache,
                    favorites,
                    scanned_folders,
                    is_dragging_files,
                    pal,
                    &mut folder_to_open,
                    &mut ctx_action,
                    &mut drop_hover_dir,
                    0,
                );
            }
        });

    if let Some(dir) = drop_hover_dir {
        app.internal_drag_hover_dir = Some(dir);
    }
    if let Some(target) = folder_to_open {
        app.open_folder(target);
    }
    if let Some(action) = ctx_action {
        handle_folder_context_action(app, action);
    }
}

fn is_dir_scanned(scanned: &std::collections::HashSet<String>, dir: &Path) -> bool {
    let key = crate::catalog::normalize_dir_key(dir);
    !key.is_empty() && scanned.contains(&key)
}

fn draw_drop_target_highlight(ui: &mut Ui, rect: egui::Rect, pal: &ThemePalette) {
    let highlight = rect.expand2(egui::vec2(3.0, 1.0));
    ui.painter().rect(
        highlight,
        egui::CornerRadius::same(3),
        pal.accent_soft.gamma_multiply(0.65),
        egui::Stroke::new(1.5_f32, Color32::from_rgb(34, 197, 94)),
        egui::StrokeKind::Inside,
    );
}

/// Paints a crisp anti-aliased green checkmark badge next to fully scanned folders.
fn draw_scanned_checkmark(ui: &mut Ui, pal: &ThemePalette) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(13.0, 13.0), egui::Sense::hover());
    let green = if pal.is_light_classic {
        Color32::from_rgb(22, 163, 74)
    } else {
        Color32::from_rgb(34, 197, 94)
    };
    let p = ui.painter();
    p.circle_filled(rect.center(), 5.8, green.gamma_multiply(0.20));
    let stroke = egui::Stroke::new(1.8_f32, green);
    let p1 = egui::pos2(rect.min.x + 3.0, rect.center().y + 0.3);
    let p2 = egui::pos2(rect.min.x + 5.5, rect.max.y - 3.2);
    let p3 = egui::pos2(rect.max.x - 2.6, rect.min.y + 3.4);
    p.line_segment([p1, p2], stroke);
    p.line_segment([p2, p3], stroke);
    resp.on_hover_text("Folder fully scanned in catalog");
}

#[allow(clippy::too_many_arguments)]
fn draw_dir_node(
    ui: &mut Ui,
    path: &Path,
    current: Option<&Path>,
    expanded: &mut std::collections::HashSet<PathBuf>,
    dir_children_cache: &mut HashMap<PathBuf, (Instant, Vec<PathBuf>)>,
    favorites: &[String],
    scanned_folders: &std::collections::HashSet<String>,
    is_dragging_files: bool,
    pal: &ThemePalette,
    to_open: &mut Option<PathBuf>,
    ctx_action: &mut Option<FolderContextAction>,
    drop_hover_dir: &mut Option<PathBuf>,
    depth: usize,
) {
    if depth > 24 {
        return;
    }
    let is_selected = current.map(|c| c == path).unwrap_or(false);
    let is_expanded = expanded.contains(path);

    let label = if depth == 0 {
        path.to_string_lossy().to_string()
    } else {
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string()
    };

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add_space((depth as f32) * 12.0);
        let arrow = if is_expanded { "v" } else { ">" };
        if ui.small_button(arrow).clicked() {
            if is_expanded {
                expanded.remove(path);
            } else {
                expanded.insert(path.to_path_buf());
                dir_children_cache.remove(path);
            }
        }
        let resp = ui.selectable_label(is_selected, &label);
        if is_dragging_files && ui.rect_contains_pointer(resp.rect) {
            draw_drop_target_highlight(ui, resp.rect, pal);
            *drop_hover_dir = Some(path.to_path_buf());
        }
        if resp.clicked() {
            *to_open = Some(path.to_path_buf());
            expanded.insert(path.to_path_buf());
        }
        resp.context_menu(|ui| {
            show_folder_context_menu(ui, path, favorites, ctx_action);
        });
        if is_dir_scanned(scanned_folders, path) {
            draw_scanned_checkmark(ui, pal);
        }
    });

    if is_expanded {
        let needs_refresh = dir_children_cache
            .get(path)
            .map(|(ts, _)| ts.elapsed() >= Duration::from_secs(8))
            .unwrap_or(true);
        if needs_refresh {
            let subdirs = read_subdirs_sorted(path);
            dir_children_cache.insert(path.to_path_buf(), (Instant::now(), subdirs));
        }
        let subdirs = dir_children_cache
            .get(path)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        for sub in subdirs.iter().take(2000) {
            draw_dir_node(
                ui,
                sub,
                current,
                expanded,
                dir_children_cache,
                favorites,
                scanned_folders,
                is_dragging_files,
                pal,
                to_open,
                ctx_action,
                drop_hover_dir,
                depth + 1,
            );
        }
    }
}

fn read_subdirs_sorted(path: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut subdirs: Vec<PathBuf> = rd
        .flatten()
        .filter_map(|e| {
            let ft = e.file_type().ok()?;
            if ft.is_dir() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') && !name.starts_with('$') {
                    return Some(e.path());
                }
            }
            None
        })
        .collect();
    subdirs.sort_by_cached_key(|p| {
        p.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    });
    subdirs
}

/// Right-click context menu for folders in Drives & Folders
fn show_folder_context_menu(
    ui: &mut Ui,
    path: &Path,
    favorites: &[String],
    out: &mut Option<FolderContextAction>,
) {
    ui.set_min_width(240.0);
    let is_drive_root = path.parent().is_none();
    let path_str = path.to_string_lossy().to_string();
    let is_fav = favorites.iter().any(|f| f.eq_ignore_ascii_case(&path_str));

    if ui.button("Create Folder             Ctrl+Shift+F2").clicked() {
        *out = Some(FolderContextAction::CreateFolder(path.to_path_buf()));
        ui.close_menu();
    }
    if ui
        .add_enabled(!is_drive_root, egui::Button::new("Delete Folder             Ctrl+Shift+F6"))
        .clicked()
    {
        *out = Some(FolderContextAction::DeleteFolder(path.to_path_buf()));
        ui.close_menu();
    }
    if ui
        .add_enabled(!is_drive_root, egui::Button::new("Copy Folder...           Ctrl+Shift+F5"))
        .clicked()
    {
        *out = Some(FolderContextAction::CopyFolder(path.to_path_buf()));
        ui.close_menu();
    }
    if ui
        .add_enabled(!is_drive_root, egui::Button::new("Move Folder...            Shift+Alt+F5"))
        .clicked()
    {
        *out = Some(FolderContextAction::MoveFolder(path.to_path_buf()));
        ui.close_menu();
    }
    if ui
        .add_enabled(!is_drive_root, egui::Button::new("Rename Folder                 Shift+F5"))
        .clicked()
    {
        *out = Some(FolderContextAction::RenameFolder(path.to_path_buf()));
        ui.close_menu();
    }
    if ui.button("Open in Explorer").clicked() {
        *out = Some(FolderContextAction::OpenInExplorer(path.to_path_buf()));
        ui.close_menu();
    }

    ui.separator();

    if ui
        .button(RichText::new("Scan Folder                  Shift+Alt+F3").strong())
        .on_hover_text("Catalog all supported files in this folder in the background without navigating")
        .clicked()
    {
        *out = Some(FolderContextAction::ScanFolder(path.to_path_buf()));
        ui.close_menu();
    }
    if ui
        .button(RichText::new("Scan Tree                    Ctrl+Shift+F3").strong())
        .on_hover_text("Catalog this folder and all subfolders in the background without navigating")
        .clicked()
    {
        *out = Some(FolderContextAction::ScanTree(path.to_path_buf()));
        ui.close_menu();
    }
    if ui
        .button("Remove from Tree      Ctrl+Shift+Del")
        .on_hover_text("Remove this folder and subfolders from the thumbnail database (keeps files on disk)")
        .clicked()
    {
        *out = Some(FolderContextAction::RemoveFromTree(path.to_path_buf()));
        ui.close_menu();
    }
    if ui
        .button("Remove Orphans...           Ctrl+Del")
        .on_hover_text("Remove missing files under this folder from the thumbnail database")
        .clicked()
    {
        *out = Some(FolderContextAction::RemoveOrphans(path.to_path_buf()));
        ui.close_menu();
    }

    ui.menu_button("Remake", |ui| {
        if ui
            .button("Remake Folder Thumbnails")
            .on_hover_text("Force re-decode and re-compress thumbnails in this folder using current settings")
            .clicked()
        {
            *out = Some(FolderContextAction::RemakeFolder(path.to_path_buf()));
            ui.close_menu();
        }
        if ui
            .button("Remake Tree Thumbnails")
            .on_hover_text("Force re-decode and re-compress thumbnails in this entire folder tree")
            .clicked()
        {
            *out = Some(FolderContextAction::RemakeTree(path.to_path_buf()));
            ui.close_menu();
        }
    });

    let fav_label = if is_fav {
        "Remove from Favorites          Ctrl+R"
    } else {
        "Add to Favorites...               Ctrl+R"
    };
    if ui.button(fav_label).clicked() {
        *out = Some(FolderContextAction::ToggleFavorite(path.to_path_buf()));
        ui.close_menu();
    }
    if ui
        .button("Include Child Folders  Ctrl+Shift+N")
        .on_hover_text("Browse this folder and all subfolders in the center pane")
        .clicked()
    {
        *out = Some(FolderContextAction::IncludeChildFolders(path.to_path_buf()));
        ui.close_menu();
    }
    if ui.button("Refresh                           Ctrl+F5").clicked() {
        *out = Some(FolderContextAction::RefreshFolder(path.to_path_buf()));
        ui.close_menu();
    }

    ui.separator();

    if ui.button("Properties").clicked() {
        *out = Some(FolderContextAction::Properties(path.to_path_buf()));
        ui.close_menu();
    }
}

fn handle_folder_context_action(app: &mut ThumbCraftApp, action: FolderContextAction) {
    match action {
        FolderContextAction::CreateFolder(parent) => {
            app.begin_create_folder(parent);
        }
        FolderContextAction::DeleteFolder(dir) => {
            app.begin_delete_folder(dir);
        }
        FolderContextAction::CopyFolder(dir) => {
            app.copy_folder_to_dest(&dir);
        }
        FolderContextAction::MoveFolder(dir) => {
            app.move_folder_to_dest(&dir);
        }
        FolderContextAction::RenameFolder(dir) => {
            app.begin_rename_folder(dir);
        }
        FolderContextAction::OpenInExplorer(dir) => {
            app.open_folder_in_explorer(&dir);
        }
        FolderContextAction::ScanFolder(dir) => {
            app.start_folder_scan(dir, false, false);
        }
        FolderContextAction::ScanTree(dir) => {
            app.start_folder_scan(dir, true, false);
        }
        FolderContextAction::RemoveFromTree(dir) => {
            app.remove_folder_from_tree(&dir);
        }
        FolderContextAction::RemoveOrphans(dir) => {
            app.remove_folder_orphans(Some(&dir));
        }
        FolderContextAction::RemakeFolder(dir) => {
            app.start_folder_scan(dir, false, true);
        }
        FolderContextAction::RemakeTree(dir) => {
            app.start_folder_scan(dir, true, true);
        }
        FolderContextAction::ToggleFavorite(dir) => {
            app.toggle_favorite_folder(&dir);
        }
        FolderContextAction::IncludeChildFolders(dir) => {
            app.settings.include_subfolders = true;
            app.settings.save();
            app.open_folder(dir);
        }
        FolderContextAction::RefreshFolder(dir) => {
            app.invalidate_dir_tree_cache(Some(&dir));
            app.expanded_dirs.insert(dir.clone());
            if matches!(&app.browse_source, BrowseSource::Folder(cur) if cur == &dir) {
                app.reload_current_source(false);
            } else {
                app.task_log.push(format!("Refreshed folder tree at {}", dir.display()));
            }
        }
        FolderContextAction::Properties(dir) => {
            app.open_folder_properties(dir);
        }
    }
}

pub fn list_quick_access_uncached() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let base = PathBuf::from(profile);
        for (label, sub) in [
            ("Pictures", "Pictures"),
            ("Documents", "Documents"),
            ("Desktop", "Desktop"),
            ("Downloads", "Downloads"),
        ] {
            let p = base.join(sub);
            if p.is_dir() {
                out.push((label.to_string(), p));
            }
        }
    }
    out
}

pub fn list_roots_uncached() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut out = Vec::new();
        for letter in b'A'..=b'Z' {
            let s = format!("{}:\\", letter as char);
            let p = PathBuf::from(&s);
            if p.exists() {
                out.push(p);
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        vec![PathBuf::from("/")]
    }
}

fn show_keywords_tab(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    ui.horizontal(|ui| {
        ui.label("Filter:");
        ui.text_edit_singleline(&mut app.keyword_filter_text);
    });
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!(
            "INDEXED KEYWORDS ({}) — Click to browse",
            app.all_keywords.len()
        ))
        .size(10.5)
        .strong()
        .color(pal.text_muted),
    );
    ui.add_space(2.0);

    let filter_low = app.keyword_filter_text.trim().to_ascii_lowercase();
    let active_kw = match &app.browse_source {
        BrowseSource::Keyword(k) => Some(k.clone()),
        _ => None,
    };

    let mut open_kw: Option<String> = None;
    ScrollArea::vertical()
        .id_salt("keywords_tree_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if app.all_keywords.is_empty() {
                ui.label(
                    RichText::new("No keywords indexed yet.\nOpen folders with CDR, PDF, EPS, SVG, or PNG files.")
                        .size(11.5)
                        .color(pal.text_muted),
                );
            }
            for (kw, count) in &app.all_keywords {
                if !filter_low.is_empty() && !kw.to_ascii_lowercase().contains(&filter_low) {
                    continue;
                }
                let selected = active_kw
                    .as_deref()
                    .map(|a| a.eq_ignore_ascii_case(kw))
                    .unwrap_or(false);
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(selected, format!("# {}", kw))
                        .clicked()
                    {
                        open_kw = Some(kw.clone());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{count}"))
                                .size(11.0)
                                .color(pal.accent),
                        );
                    });
                });
            }
        });

    if let Some(kw) = open_kw {
        app.open_keyword_view(kw);
    }
}

fn show_galleries_tab(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.new_gallery_name)
                .hint_text("New gallery name...")
                .desired_width(ui.available_width() - 56.0),
        );
        if ui.button("Add").clicked() && !app.new_gallery_name.trim().is_empty() {
            let name = app.new_gallery_name.trim().to_string();
            app.new_gallery_name.clear();
            app.create_gallery(&name);
        }
    });
    ui.add_space(4.0);
    ui.label(
        RichText::new("CATALOG GALLERIES")
            .size(10.5)
            .strong()
            .color(pal.text_muted),
    );

    let active_gal = match &app.browse_source {
        BrowseSource::Gallery(g) => Some(g.clone()),
        _ => None,
    };
    let mut open_gal = None;
    let mut del_gal = None;

    ScrollArea::vertical()
        .id_salt("galleries_tree_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if app.all_galleries.is_empty() {
                ui.label(
                    RichText::new("Create galleries to group CDR designs, vectors, and photos across folders.")
                        .size(11.5)
                        .color(pal.text_muted),
                );
            }
            for (name, count) in &app.all_galleries {
                let selected = active_gal.as_deref() == Some(name.as_str());
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(selected, format!("[G] {name} ({count})"))
                        .clicked()
                    {
                        open_gal = Some(name.clone());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button(RichText::new("×").color(Color32::from_rgb(239, 68, 68)))
                            .on_hover_text("Delete gallery")
                            .clicked()
                        {
                            del_gal = Some(name.clone());
                        }
                    });
                });
            }
        });

    if let Some(g) = open_gal {
        app.open_gallery_view(g);
    }
    if let Some(g) = del_gal {
        app.delete_gallery(&g);
    }
}

fn show_filters_tab(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    ui.label(
        RichText::new("FORMAT FILTER")
            .size(10.5)
            .strong()
            .color(pal.text_muted),
    );
    for (id, label) in [
        ("ALL", "All Supported Formats"),
        ("CDR", "CorelDRAW (CDR / CMX)"),
        ("PSDPSB", "Photoshop (PSD / PSB)"),
        ("PSD", "PSD only"),
        ("PSB", "PSB only"),
        ("GIF", "GIF"),
        ("GS", "Ghostscript (PDF / AI / EPS / PS)"),
        ("MODERN", "Modern (AVIF / WEBP / JXL)"),
        ("SVG", "SVG Vector"),
        ("RASTER", "Common Raster (PNG / JPG / TIFF)"),
        ("OTHER", "Other Files (Native Icons)"),
    ] {
        if ui
            .selectable_label(app.format_filter == id, label)
            .clicked()
        {
            app.format_filter = id.to_string();
        }
    }

    ui.add_space(8.0);
    ui.separator();
    ui.label(
        RichText::new("MINIMUM RATING")
            .size(10.5)
            .strong()
            .color(pal.text_muted),
    );
    ui.horizontal(|ui| {
        for r in 0..=5u8 {
            let label = if r == 0 {
                "Any".to_string()
            } else {
                format!("{r}★+")
            };
            if ui.selectable_label(app.min_rating_filter == r, label).clicked() {
                app.min_rating_filter = r;
            }
        }
    });

    ui.add_space(8.0);
    ui.checkbox(&mut app.only_with_keywords, "Only items with keywords");
    if ui.button("Reset Filters").clicked() {
        app.format_filter = "ALL".to_string();
        app.min_rating_filter = 0;
        app.only_with_keywords = false;
        app.search_query.clear();
    }
}
