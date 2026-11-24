use crate::ui::theme::ThemePalette;
use crate::ui::thumb_pane::format_bytes;
use crate::ui::{ManagedTask, ThumbCraftApp};
use crate::worker::TaskState;
use egui::{Color32, CornerRadius, ProgressBar, Rect, RichText, ScrollArea, Sense, Ui, Vec2};

#[derive(Debug, Clone)]
enum TaskContextAction {
    Properties(u64),
    Pause(u64),
    Resume(u64),
    Quit(u64),
    PauseAll,
    ResumeAll,
    QuitAll,
    ShowErrors(u64),
    ClearErrors(u64),
    ClearAllErrors,
}

pub fn show_tasks_pane(ui: &mut Ui, app: &mut ThumbCraftApp, pal: &ThemePalette) {
    crate::ui::theme::draw_pane_title_bar(ui, "Tasks", false, pal);
    ui.horizontal(|ui| {
        if let Some(ref ver) = app.gs_version {
            ui.label(
                RichText::new(format!(
                    "[OK] GS {ver} • {} Q{}",
                    app.settings.thumb_format.short_name(),
                    app.settings.thumb_quality
                ))
                .size(10.5)
                .color(pal.badge_cdr),
            );
        } else {
            ui.label(
                RichText::new("[!] Ghostscript missing")
                    .size(10.5)
                    .color(Color32::from_rgb(205, 42, 42)),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Clear").on_hover_text("Clear finished tasks and log").clicked() {
                app.task_log.clear();
                app.managed_tasks.retain(|t| t.state.is_active());
            }
            if ui
                .small_button("Prefs")
                .on_hover_text("Task & Thumbnail Database Preferences")
                .clicked()
            {
                app.show_preferences = true;
                app.prefs_tab = crate::ui::PrefsTab::Tasks;
            }
            if ui.small_button("GS").on_hover_text("Ghostscript Preferences").clicked() {
                app.show_preferences = true;
                app.prefs_tab = crate::ui::PrefsTab::Ghostscript;
            }
        });
    });

    // Clickable DB status bar with Open in Explorer
    ui.horizontal(|ui| {
        let db_name = app
            .db_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("thumbcraft.tcdb");
        ui.label(
            RichText::new(format!(
                "DB: {} ({}) • {} thumbs",
                db_name,
                format_bytes(app.catalog_stats.db_size_bytes),
                app.catalog_stats.total_thumbnails
            ))
            .size(10.0)
            .color(pal.text_muted),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Open DB")
                .on_hover_text(format!("Reveal {} in Windows Explorer", app.db_path.display()))
                .clicked()
            {
                app.open_db_in_explorer();
            }
            let accel_lbl = match (&app.gpu_info, app.settings.use_gpu_acceleration) {
                (Some(g), true) => format!("GPU+CPU ({})", g.backend),
                _ => "Multi-CPU".to_string(),
            };
            let accel_hover = match (&app.gpu_info, app.settings.use_gpu_acceleration) {
                (Some(g), true) => format!(
                    "Hybrid GPU + Multi-Core CPU active: {} ({})",
                    g.name, g.backend
                ),
                (Some(g), false) => format!(
                    "GPU acceleration disabled in Preferences (detected: {} ({}))",
                    g.name, g.backend
                ),
                (None, _) => "Multi-core CPU thumbnailing active (no compute GPU detected)".into(),
            };
            ui.label(
                RichText::new(accel_lbl)
                    .size(9.5)
                    .strong()
                    .color(if app.gpu_info.is_some() && app.settings.use_gpu_acceleration {
                        pal.badge_cdr
                    } else {
                        pal.text_muted
                    }),
            )
            .on_hover_text(accel_hover);
        });
    });

    // Progress bar when center-pane worker is indexing
    if app.worker_total > 0 && app.worker_done < app.worker_total {
        let frac = app.worker_done as f32 / app.worker_total.max(1) as f32;
        ui.add(
            ProgressBar::new(frac)
                .text(format!(
                    "View {}/{}: {}",
                    app.worker_done, app.worker_total, app.worker_current_file
                ))
                .desired_height(14.0),
        );
    }

    // Active / Queued / Recent Tasks list with right-click menu matching reference/picture-11.png
    let mut ctx_action: Option<TaskContextAction> = None;
    let mut select_task: Option<u64> = None;
    let mut open_props: Option<u64> = None;

    if !app.managed_tasks.is_empty() {
        ui.separator();
        let running_count = app
            .managed_tasks
            .iter()
            .filter(|t| t.state == TaskState::Running)
            .count();
        let paused_count = app
            .managed_tasks
            .iter()
            .filter(|t| t.state == TaskState::Paused)
            .count();
        let queued_count = app
            .managed_tasks
            .iter()
            .filter(|t| t.state == TaskState::Queued)
            .count();

        let header = if running_count > 0 || paused_count > 0 || queued_count > 0 {
            format!(
                "SCAN TASKS ({running_count} running{}{}) — Right-click task for Pause/Resume/Cancel",
                if paused_count > 0 {
                    format!(", {paused_count} paused")
                } else {
                    String::new()
                },
                if queued_count > 0 {
                    format!(", {queued_count} queued")
                } else {
                    String::new()
                },
            )
        } else {
            "SCAN TASKS — Right-click task for options".to_string()
        };
        ui.label(RichText::new(header).size(9.5).strong().color(pal.text_muted));

        let has_any_running = app
            .managed_tasks
            .iter()
            .any(|t| matches!(t.state, TaskState::Running | TaskState::Queued));
        let has_any_paused = app
            .managed_tasks
            .iter()
            .any(|t| t.state == TaskState::Paused);
        let has_any_active = app.managed_tasks.iter().any(|t| t.state.is_active());
        let has_any_errors = app.managed_tasks.iter().any(|t| !t.errors.is_empty());

        let tasks_snapshot: Vec<ManagedTask> = app.managed_tasks.clone();
        ScrollArea::vertical()
            .id_salt("managed_tasks_scroll")
            .max_height(136.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                for task in &tasks_snapshot {
                    let is_selected = app.selected_task_id == Some(task.id);
                    let resp = draw_task_row(ui, task, is_selected, pal);
                    if resp.clicked() || resp.secondary_clicked() {
                        select_task = Some(task.id);
                    }
                    if resp.double_clicked() {
                        open_props = Some(task.id);
                    }
                    let task_id = task.id;
                    let task_state = task.state;
                    let task_has_err = !task.errors.is_empty();
                    resp.context_menu(|ui| {
                        show_task_context_menu(
                            ui,
                            task_id,
                            task_state,
                            task_has_err,
                            has_any_running,
                            has_any_paused,
                            has_any_active,
                            has_any_errors,
                            &mut ctx_action,
                        );
                    });
                }
            });
    }

    if let Some(tid) = select_task {
        app.selected_task_id = Some(tid);
    }
    if let Some(tid) = open_props {
        app.selected_task_id = Some(tid);
        app.task_props_id = Some(tid);
    }
    if let Some(act) = ctx_action {
        handle_task_context_action(app, act);
    }

    ui.separator();

    ScrollArea::vertical()
        .id_salt("tasks_log_scroll")
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &app.task_log {
                ui.label(RichText::new(entry).size(10.5).color(pal.text_muted));
            }
        });
}

fn draw_task_row(
    ui: &mut Ui,
    task: &ManagedTask,
    selected: bool,
    pal: &ThemePalette,
) -> egui::Response {
    let row_h = 24.0;
    let avail_w = ui.available_width().max(120.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(avail_w, row_h), Sense::click());
    let painter = ui.painter_at(rect);

    let pct = if task.total > 0 {
        (task.done as f32 / task.total as f32).clamp(0.0, 1.0)
    } else if task.state == TaskState::Completed {
        1.0
    } else {
        0.0
    };

    let bg = if selected {
        Color32::from_rgb(35, 38, 44)
    } else if resp.hovered() {
        pal.bg_toolbar
    } else {
        pal.bg_canvas
    };
    let border_col = if selected {
        pal.accent
    } else {
        pal.border_subtle
    };
    painter.rect(
        rect,
        CornerRadius::same(3),
        bg,
        egui::Stroke::new(1.0_f32, border_col),
        egui::StrokeKind::Inside,
    );

    // Dedicated progress bar track & fill for each task
    let bar_rect = Rect::from_min_max(
        egui::pos2(rect.min.x + 20.0, rect.min.y + 2.0),
        egui::pos2(rect.max.x - 2.0, rect.max.y - 2.0),
    );
    let track_col = if selected {
        Color32::from_rgb(48, 52, 60)
    } else {
        pal.section_band
    };
    painter.rect_filled(bar_rect, CornerRadius::same(2), track_col);

    let fill_w = (bar_rect.width() * pct).max(if task.state == TaskState::Running { 4.0 } else { 0.0 });
    let fill_col = match task.state {
        TaskState::Running => {
            if selected {
                Color32::from_rgb(245, 136, 42)
            } else {
                Color32::from_rgb(56, 138, 224)
            }
        }
        TaskState::Paused => Color32::from_rgb(202, 138, 4),
        TaskState::Completed => Color32::from_rgb(34, 154, 82),
        TaskState::Cancelled => Color32::from_rgb(160, 62, 62),
        TaskState::Queued => Color32::from_rgb(120, 128, 140),
    };
    if fill_w > 0.5 {
        let fill_rect = Rect::from_min_size(bar_rect.min, Vec2::new(fill_w, bar_rect.height()));
        painter.rect_filled(fill_rect, CornerRadius::same(2), fill_col.gamma_multiply(0.85));
    }

    // Status icon on the left
    let (icon, icon_col) = match task.state {
        TaskState::Running => ("▶", Color32::from_rgb(34, 197, 94)),
        TaskState::Paused => ("||", Color32::from_rgb(234, 179, 8)),
        TaskState::Queued => ("..", pal.text_muted),
        TaskState::Completed => ("OK", Color32::from_rgb(34, 197, 94)),
        TaskState::Cancelled => ("×", Color32::from_rgb(220, 38, 38)),
    };
    painter.text(
        egui::pos2(rect.min.x + 10.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(9.5),
        icon_col,
    );

    let err_suffix = if task.errors.is_empty() {
        String::new()
    } else {
        format!(" [{} err]", task.errors.len())
    };
    let count_part = if task.total > 0 {
        format!(" ({}/{})", task.done, task.total)
    } else {
        String::new()
    };
    let state_tag = match task.state {
        TaskState::Paused => " [Paused]",
        TaskState::Queued => " [Queued]",
        TaskState::Cancelled => " [Cancelled]",
        _ => "",
    };
    let text = format!(
        "{:.0}%{count_part}{state_tag} — {}{}",
        pct * 100.0,
        task.display_text,
        err_suffix
    );
    let txt_col = if selected || pct > 0.35 {
        Color32::WHITE
    } else {
        pal.text_primary
    };
    painter.text(
        egui::pos2(bar_rect.min.x + 5.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(10.5),
        txt_col,
    );

    resp.on_hover_text(
        "Double-click: Task Properties • Right-click: Pause / Resume / Cancel / Errors",
    )
}

#[allow(clippy::too_many_arguments)]
fn show_task_context_menu(
    ui: &mut Ui,
    task_id: u64,
    state: TaskState,
    has_errors: bool,
    has_any_running: bool,
    has_any_paused: bool,
    has_any_active: bool,
    has_any_errors: bool,
    out: &mut Option<TaskContextAction>,
) {
    ui.set_min_width(180.0);

    if ui.button("Task Properties").clicked() {
        *out = Some(TaskContextAction::Properties(task_id));
        ui.close_menu();
    }
    ui.separator();

    if ui
        .add_enabled(
            matches!(state, TaskState::Running | TaskState::Queued),
            egui::Button::new("Pause Task"),
        )
        .clicked()
    {
        *out = Some(TaskContextAction::Pause(task_id));
        ui.close_menu();
    }
    if ui
        .add_enabled(state == TaskState::Paused, egui::Button::new("Resume Task"))
        .clicked()
    {
        *out = Some(TaskContextAction::Resume(task_id));
        ui.close_menu();
    }
    if ui
        .add_enabled(state.is_active(), egui::Button::new("Cancel Task"))
        .clicked()
    {
        *out = Some(TaskContextAction::Quit(task_id));
        ui.close_menu();
    }

    ui.separator();

    if ui
        .add_enabled(has_any_running, egui::Button::new("Pause All Tasks"))
        .clicked()
    {
        *out = Some(TaskContextAction::PauseAll);
        ui.close_menu();
    }
    if ui
        .add_enabled(has_any_paused, egui::Button::new("Resume All Tasks"))
        .clicked()
    {
        *out = Some(TaskContextAction::ResumeAll);
        ui.close_menu();
    }
    if ui
        .add_enabled(has_any_active, egui::Button::new("Cancel All Tasks"))
        .clicked()
    {
        *out = Some(TaskContextAction::QuitAll);
        ui.close_menu();
    }

    ui.separator();

    if ui
        .add_enabled(has_errors, egui::Button::new("Show Errors"))
        .clicked()
    {
        *out = Some(TaskContextAction::ShowErrors(task_id));
        ui.close_menu();
    }
    if ui
        .add_enabled(has_errors, egui::Button::new("Clear Errors"))
        .clicked()
    {
        *out = Some(TaskContextAction::ClearErrors(task_id));
        ui.close_menu();
    }
    if ui
        .add_enabled(has_any_errors, egui::Button::new("Clear All Errors"))
        .clicked()
    {
        *out = Some(TaskContextAction::ClearAllErrors);
        ui.close_menu();
    }
}

fn handle_task_context_action(app: &mut ThumbCraftApp, action: TaskContextAction) {
    match action {
        TaskContextAction::Properties(id) | TaskContextAction::ShowErrors(id) => {
            app.task_props_id = Some(id);
        }
        TaskContextAction::Pause(id) => app.pause_task(id),
        TaskContextAction::Resume(id) => app.resume_task(id),
        TaskContextAction::Quit(id) => app.quit_task(id),
        TaskContextAction::PauseAll => app.pause_all_tasks(),
        TaskContextAction::ResumeAll => app.resume_all_tasks(),
        TaskContextAction::QuitAll => app.quit_all_tasks(),
        TaskContextAction::ClearErrors(id) => {
            if let Some(t) = app.managed_tasks.iter_mut().find(|t| t.id == id) {
                t.errors.clear();
            }
        }
        TaskContextAction::ClearAllErrors => {
            for t in &mut app.managed_tasks {
                t.errors.clear();
            }
        }
    }
}
