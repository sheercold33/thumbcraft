#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod catalog;
mod cdr;
mod config;
mod cpt;
mod dds;
mod decoder;
mod drag_drop;
mod folder_watch;
mod ghostscript;
mod gpu_accel;
mod image_meta;
mod keyword_writer;
mod psd;
mod sample_gen;
mod shell_icon;
mod similarity;
mod ui;
mod worker;

use std::path::PathBuf;

const APP_ICON_WEBP: &[u8] = include_bytes!("../assets/app_icon.webp");

fn load_app_icon() -> Option<egui::IconData> {
    let decoded = oxideav_webp::decode_rgba8(APP_ICON_WEBP).ok()?;
    let img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(
        decoded.width,
        decoded.height,
        decoded.data,
    )?;
    let dyn_img = image::DynamicImage::ImageRgba8(img);
    let resized = if dyn_img.width() > 256 || dyn_img.height() > 256 {
        gpu_accel::fast_cpu_area_resize(&dyn_img, 256, 256)
    } else {
        dyn_img
    };
    let rgba = resized.to_rgba8();
    Some(egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut screenshot_path: Option<PathBuf> = None;
    let mut initial_folder: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        if args[i] == "--screenshot" {
            if let Some(next) = args.get(i + 1) {
                screenshot_path = Some(PathBuf::from(next));
                i += 2;
                continue;
            }
        } else if let Some(rest) = args[i].strip_prefix("--screenshot=") {
            screenshot_path = Some(PathBuf::from(rest));
        } else if !args[i].starts_with('-') {
            let p = PathBuf::from(&args[i]);
            if p.is_dir() {
                initial_folder = Some(p);
            }
        }
        i += 1;
    }

    let is_screenshot = screenshot_path.is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("ThumbCraft Studio — Thumbnail Viewer & CDR Catalog")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([900.0, 600.0]);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    if is_screenshot {
        viewport = viewport.with_active(false);
        #[cfg(target_os = "windows")]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }

    #[allow(unused_mut)]
    let mut native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    #[cfg(target_os = "macos")]
    if is_screenshot {
        native_options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            builder.with_activate_ignoring_other_apps(false);
        }));
    }

    eframe::run_native(
        "ThumbCraft Studio",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(ui::ThumbCraftApp::new(
                cc,
                initial_folder,
                screenshot_path,
            )))
        }),
    )
}
